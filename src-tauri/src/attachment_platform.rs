//! Fixed native picker/clipboard operations. No caller-selected filesystem path.
use crate::error::{Error, Result};
use std::path::PathBuf;
pub enum ClipboardFile {
    Path(PathBuf),
    Bytes(String, Vec<u8>),
    /// Raw clipboard TIFF, converted to PNG by [`tiff_to_png`] off the AppKit main thread.
    #[allow(dead_code)]
    Tiff(Vec<u8>),
}

/// Decodes clipboard TIFF and re-encodes it as PNG. Runs on a blocking worker, never
/// on the main thread: large screenshots take long enough to stall the UI.
pub fn tiff_to_png(_tiff: &[u8]) -> Result<Vec<u8>> {
    Err(Error::new("attachment", "Cannot read clipboard image."))
}

/// The headless host has no screen to show a picker on (ADR-131).
pub async fn pick(_app: &crate::runtime::AppHandle, _folder: Option<bool>) -> Result<Vec<PathBuf>> {
    Err(Error::new(
        "attachment",
        "File pickers are not available on this host.",
    ))
}
/// Text paths are admitted only when the current native clipboard equals the pasted text.
/// This is not a filesystem resolver for renderer strings.
fn clipboard_path(text: &str) -> Option<PathBuf> {
    if text.len() > 8192 || text.contains(['\n', '\r', '\0']) {
        return None;
    }
    let text = text.trim().trim_matches('"').trim_matches('\'');
    if text.starts_with("file://") {
        return reqwest::Url::parse(text).ok()?.to_file_path().ok();
    }
    text.starts_with('/').then(|| PathBuf::from(text))
}
pub async fn paste(
    _app: &crate::runtime::AppHandle,
    _state: std::sync::Arc<crate::commands::AppState>,
    _expected_text: Option<String>,
) -> Result<Vec<ClipboardFile>> {
    Ok(Vec::new())
}

/// Observe only this app's native Command-V event. A compromised renderer cannot
/// manufacture a native clipboard read by writing a path to its own clipboard.
pub fn install_paste_listener(
    _app: &crate::runtime::AppHandle,
    _state: std::sync::Arc<crate::commands::AppState>,
) {
}
pub fn remove_paste_listener() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_paths_are_single_local_values_not_remote_or_multiline() {
        assert_eq!(
            clipboard_path("\"/tmp/a b.png\""),
            Some(PathBuf::from("/tmp/a b.png"))
        );
        assert_eq!(
            clipboard_path("file:///tmp/a%20b.csv"),
            Some(PathBuf::from("/tmp/a b.csv"))
        );
        for text in [
            "https://example.com/file",
            "relative.png",
            "/tmp/a\n/tmp/b",
            "/tmp/\0",
        ] {
            assert!(clipboard_path(text).is_none());
        }
    }
}
