//! Chat background image (ADR-089): an image behind the conversation panes. The picked file is decoded with bounds, scaled to at most 2560 px and
//! saved as a JPEG in `backgrounds/` next to `state.json`; settings keep only its
//! file name, so the image stays on this Mac and never weighs on state saves.

use std::path::PathBuf;
use std::sync::Arc;

use crate::runtime::{AppHandle, State};
use base64::Engine;
use serde::Deserialize;
use uuid::Uuid;

use crate::commands::AppState;
use crate::error::{Error, Result};

/// Effects the renderer draws the image with.
pub const EFFECTS: [&str; 6] = ["none", "dither", "ascii", "halftone", "scanlines", "haze"];
const MAX_FILE: u64 = 30 * 1024 * 1024;
const MAX_SIDE: u32 = 2560;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    /// Opens the native picker; returns the saved file name, or none when cancelled.
    Pick,
    /// Removes every saved background file.
    Clear,
}

/// `chat-<uuid>.jpg`, the only names this module writes.
pub fn valid_name(name: &str) -> bool {
    name.strip_prefix("chat-")
        .and_then(|rest| rest.strip_suffix(".jpg"))
        .is_some_and(|id| Uuid::parse_str(id).is_ok())
}

fn dir(state: &AppState) -> PathBuf {
    state
        .data_path
        .parent()
        .map(|parent| parent.join("backgrounds"))
        .unwrap_or_else(|| PathBuf::from("backgrounds"))
}

fn remove_all_except(state: &AppState, keep: Option<&str>) {
    let Ok(entries) = std::fs::read_dir(dir(state)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if valid_name(&name) && Some(name.as_str()) != keep {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub async fn chat_background_action(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    action: Action,
) -> Result<Option<String>> {
    let state = state.inner().clone();
    match action {
        Action::Clear => {
            crate::commands::native_task(move || {
                remove_all_except(&state, None);
                Ok(None)
            })
            .await
        }
        Action::Pick => {
            let Some(path) = pick_image(&app).await else {
                return Ok(None);
            };
            crate::commands::native_task(move || {
                let jpeg = scaled_jpeg(&path)?;
                let name = format!("chat-{}.jpg", Uuid::new_v4());
                let folder = dir(&state);
                let failed = |_| Error::new("io", "Could not save the background.");
                std::fs::create_dir_all(&folder).map_err(failed)?;
                std::fs::write(folder.join(&name), jpeg).map_err(failed)?;
                remove_all_except(&state, Some(&name));
                Ok(Some(name))
            })
            .await
        }
    }
}

/// The saved background as a data URL for the renderer, or none.
pub async fn chat_background_image(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<Option<String>> {
    let state = state.inner().clone();
    crate::commands::native_task(move || {
        if !valid_name(&name) {
            return Ok(None);
        }
        let Ok(bytes) = std::fs::read(dir(&state).join(&name)) else {
            return Ok(None);
        };
        Ok(Some(format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )))
    })
    .await
}

/// The headless host has no screen to show a picker on (ADR-131).
async fn pick_image(_app: &AppHandle) -> Option<PathBuf> {
    None
}

fn scaled_jpeg(path: &std::path::Path) -> Result<Vec<u8>> {
    let unreadable = || Error::new("invalid", "Could not read that image.");
    let size = std::fs::metadata(path).map_err(|_| unreadable())?.len();
    if size > MAX_FILE {
        return Err(Error::new("invalid", "The image must be at most 30 MiB."));
    }
    let bytes = std::fs::read(path).map_err(|_| unreadable())?;
    jpeg_from_bytes(&bytes)
}

fn jpeg_from_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    let invalid = || Error::new("invalid", "Use a PNG or JPEG image.");
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| invalid())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| invalid())?;
    if decoded.width() == 0 || decoded.height() == 0 {
        return Err(Error::new("invalid", "The image is empty."));
    }
    let scaled = if decoded.width().max(decoded.height()) > MAX_SIDE {
        decoded.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 82)
        .encode_image(&scaled.to_rgb8())
        .map_err(|_| invalid())?;
    Ok(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_images_are_bounded() {
        assert!(valid_name(&format!("chat-{}.jpg", Uuid::new_v4())));
        assert!(!valid_name("chat-../../state.jpg"));
        assert!(!valid_name("state.json"));
        let mut png = Vec::new();
        image::DynamicImage::new_rgb8(3000, 1000)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let jpeg = jpeg_from_bytes(&png).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2560, 853));
        assert!(jpeg_from_bytes(b"not an image").is_err());
    }
}
