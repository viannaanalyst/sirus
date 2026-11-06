//! Project icon customization (ADR-059): a folder colour, an emoji or a logo
//! image that replaces the folder in the sidebar. Display metadata only; the
//! project folder is never touched. The logo comes from a file the person picks
//! in the native dialog, decoded with bounds and stored as a small PNG data URL.
use crate::runtime::AppHandle;
use base64::Engine;

use crate::error::{Error, Result};
use crate::models::ProjectAstroIcon;

/// Preset colours the renderer maps to tokens; anything else must be `#rrggbb`.
const PRESETS: [&str; 9] = [
    "blue", "red", "yellow", "green", "pink", "purple", "teal", "orange", "gray",
];
/// Astro cosmic icons a project can show instead of its folder (ADR-116 kept them); the renderer
/// draws each one on a canvas.
pub const ASTRO_ICONS: [&str; 16] = [
    "orbita",
    "saturno",
    "lua",
    "sol",
    "galaxia",
    "nebulosa",
    "cometa",
    "buraco",
    "estrela",
    "pulsar",
    "constelacao",
    "satelite",
    "foguete",
    "planeta",
    "asteroide",
    "eclipse",
];
/// How an Astro icon is drawn: silver Metal or Neon in the project colour.
pub const ASTRO_STYLES: [&str; 2] = ["metal", "neon"];
/// Line icons a project can show instead of its folder; the renderer maps each name.
pub const ICONS: [&str; 58] = [
    "folder",
    "code",
    "terminal",
    "book-open",
    "graduation-cap",
    "pencil-simple",
    "feather",
    "briefcase",
    "music-notes",
    "chart-bar",
    "barbell",
    "scales",
    "globe",
    "airplane-tilt",
    "wrench",
    "paw-print",
    "flask",
    "brain",
    "heart",
    "tree",
    "rocket",
    "lightbulb",
    "star",
    "coffee",
    "camera",
    "shopping-cart",
    "house",
    "car",
    "game-controller",
    "palette",
    "database",
    "cloud",
    "lock",
    "key",
    "currency-dollar",
    "bank",
    "stethoscope",
    "chat-circle",
    "envelope",
    "device-mobile",
    "calendar-blank",
    "robot",
    "cpu",
    "bug",
    "leaf",
    "sun",
    "moon",
    "fire",
    "lightning",
    "cube",
    "puzzle-piece",
    "trophy",
    "users",
    "storefront",
    "film-slate",
    "newspaper",
    "atom",
    "cat",
];
/// The logo is cropped square and scaled to this many pixels per side.
const LOGO_SIDE: u32 = 96;
/// Largest picked file that is decoded.
const MAX_FILE: u64 = 10 * 1024 * 1024;
/// Largest stored data URL.
const MAX_LOGO: usize = 96 * 1024;

/// Opens the native image picker and returns the picked image as a small square PNG data
/// URL, for icons that are not a project's (project folders). `None` when cancelled.
pub async fn pick_look_image(app: AppHandle) -> Result<Option<String>> {
    let Some(path) = pick_image(&app).await else {
        return Ok(None);
    };
    tokio::task::spawn_blocking(move || logo_from_file(&path))
        .await
        .map_err(|_| Error::new("invalid", "Could not read that image."))?
        .map(Some)
}

/// Checks a look that arrives whole (a project folder's): known colour, emoji, Astro and
/// line icon, and a logo that is a bounded PNG data URL like the ones made here.
pub fn validate_look(look: &crate::models::ProjectLook) -> Result<()> {
    if let Some(color) = &look.color {
        valid_color(color)?;
    }
    if let Some(emoji) = &look.emoji {
        valid_emoji(emoji)?;
    }
    if let Some(astro) = &look.astro {
        valid_astro(astro.clone())?;
    }
    if look
        .icon
        .as_deref()
        .is_some_and(|name| !ICONS.contains(&name))
    {
        return Err(Error::new("invalid", "Unknown project icon."));
    }
    if look
        .logo
        .as_deref()
        .is_some_and(|logo| !logo.starts_with("data:image/png;base64,") || logo.len() > MAX_LOGO)
    {
        return Err(Error::new("invalid", "Use a PNG or JPEG image."));
    }
    Ok(())
}

pub(crate) fn valid_color(value: &str) -> Result<String> {
    let hex = value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit());
    if PRESETS.contains(&value) || hex {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(Error::new(
            "invalid",
            "Choose one of the colours or a #rrggbb value.",
        ))
    }
}

pub(crate) fn valid_astro(astro: ProjectAstroIcon) -> Result<ProjectAstroIcon> {
    if is_valid_astro(&astro) {
        Ok(astro)
    } else {
        Err(Error::new("invalid", "Unknown Astro icon or style."))
    }
}

pub(crate) fn is_valid_astro(astro: &ProjectAstroIcon) -> bool {
    ASTRO_ICONS.contains(&astro.icon.as_str()) && ASTRO_STYLES.contains(&astro.style.as_str())
}

/// A short printable symbol (an emoji, including joined sequences); no spaces or control characters.
pub(crate) fn valid_emoji(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 32
        || value.chars().count() > 10
        || value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(Error::new("invalid", "Use a single emoji."));
    }
    Ok(value.to_owned())
}

/// The headless host has no screen to show a picker on (ADR-131).
async fn pick_image(_app: &AppHandle) -> Option<std::path::PathBuf> {
    None
}

/// Decodes a bounded PNG/JPEG, crops it to the centre square and returns a small PNG data URL.
fn logo_from_file(path: &std::path::Path) -> Result<String> {
    let size = std::fs::metadata(path)
        .map_err(|_| Error::new("invalid", "Could not read that image."))?
        .len();
    if size > MAX_FILE {
        return Err(Error::new("invalid", "The image must be at most 10 MiB."));
    }
    let bytes =
        std::fs::read(path).map_err(|_| Error::new("invalid", "Could not read that image."))?;
    logo_from_bytes(&bytes)
}

fn logo_from_bytes(bytes: &[u8]) -> Result<String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| Error::new("invalid", "Use a PNG or JPEG image."))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|_| Error::new("invalid", "Use a PNG or JPEG image."))?;
    let side = decoded.width().min(decoded.height());
    if side == 0 {
        return Err(Error::new("invalid", "The image is empty."));
    }
    let square = decoded.crop_imm(
        (decoded.width() - side) / 2,
        (decoded.height() - side) / 2,
        side,
        side,
    );
    let small = square.resize_exact(LOGO_SIDE, LOGO_SIDE, image::imageops::FilterType::Lanczos3);
    let mut png = Vec::new();
    small
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|_| Error::new("invalid", "Could not convert that image."))?;
    let url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    if url.len() > MAX_LOGO {
        return Err(Error::new(
            "invalid",
            "That image is too detailed for a logo.",
        ));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_presets_or_hex() {
        assert_eq!(valid_color("blue").unwrap(), "blue");
        assert_eq!(valid_color("#AABBCC").unwrap(), "#aabbcc");
        assert!(
            valid_color("url(x)").is_err()
                && valid_color("#abc").is_err()
                && valid_color("navy").is_err()
        );
    }

    #[test]
    fn emojis_are_short_and_printable() {
        assert_eq!(valid_emoji(" 🚂 ").unwrap(), "🚂");
        assert!(valid_emoji("👨‍👩‍👧‍👦").is_ok());
        assert!(
            valid_emoji("a b").is_err()
                && valid_emoji("").is_err()
                && valid_emoji(&"x".repeat(40)).is_err()
        );
    }

    #[test]
    fn astro_icons_are_known_icons_and_styles() {
        let icon = |icon: &str, style: &str| ProjectAstroIcon {
            icon: icon.into(),
            style: style.into(),
        };
        assert!(valid_astro(icon("foguete", "neon")).is_ok());
        assert!(valid_astro(icon("../x", "metal")).is_err());
        assert!(valid_astro(icon("foguete", "pixel")).is_err());
        let look = crate::models::ProjectLook {
            astro: Some(icon("planeta", "metal")),
            ..Default::default()
        };
        assert!(validate_look(&look).is_ok());
    }

    #[test]
    fn logos_are_square_small_pngs() {
        let mut source = Vec::new();
        image::DynamicImage::new_rgb8(300, 120)
            .write_to(
                &mut std::io::Cursor::new(&mut source),
                image::ImageFormat::Png,
            )
            .unwrap();
        let url = logo_from_bytes(&source).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        let png = base64::engine::general_purpose::STANDARD
            .decode(&url["data:image/png;base64,".len()..])
            .unwrap();
        let back = image::load_from_memory(&png).unwrap();
        assert_eq!((back.width(), back.height()), (LOGO_SIDE, LOGO_SIDE));
        assert!(logo_from_bytes(b"not an image").is_err());
    }
}
