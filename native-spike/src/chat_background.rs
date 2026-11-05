//! Settings → Aparência → Fundo do chat: the picture behind the conversation card.
//!
//! The effects and the fade are Monocode's (project_appearance.rs and project_background.rs,
//! MIT, see LICENSE-MONOCODE-MIT). The texture is prepared once per (picture, effect, screen
//! size, fade curve) on a background thread through GPUI's asset cache; opacity is not part of
//! the key, so changing the visibility only changes how the finished texture is painted.
use crate::backend;
use gpui::{App, Asset, ObjectFit, RenderImage, Window, prelude::*};
use image::{ImageReader, Limits, RgbaImage, imageops};
use serde_json::{Value, json};
use std::{
    future::Future,
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
};

/// The Monocode picture copied on first load, as the chat background's file name.
const SEEDED_NAME: &str = "chat-monocode.png";
/// Where Monocode kept its chat background.
const MONOCODE_PICTURE: &str =
    "Library/Application Support/com.monocode.desktop/backgrounds/chat-background.png";
/// Written once the first load has been handled, so a picture removed in Settings stays removed.
const MARKER: &str = ".monocode-default";
/// Artwork larger than this is shrunk on decode, as Monocode's read_image does.
const SOURCE_CAP: u32 = 2048;
/// Pictures above this size are refused, as in Monocode.
const SOURCE_BYTES_CAP: u64 = 30 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Effect {
    None,
    Dither,
    Ascii,
    Halftone,
    Scanlines,
    /// Monocode's "Haze" (GradientBlur).
    Haze,
}

impl Effect {
    /// The values the Appearance page saves in `chatBackgroundEffect`; unknown ones draw the original.
    pub fn from_setting(value: &str) -> Self {
        match value {
            "dither" => Self::Dither,
            "ascii" => Self::Ascii,
            "halftone" => Self::Halftone,
            "scanlines" => Self::Scanlines,
            "haze" => Self::Haze,
            _ => Self::None,
        }
    }
}

/// The chat background's settings as the Appearance page saves them.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub name: String,
    pub effect: Effect,
    /// `chatBackgroundShowOn` is "empty": conversations with messages show no picture.
    pub empty_only: bool,
    /// `chatBackgroundEmptyVisibility`, 0 to 100.
    pub empty_visibility: u32,
    /// `chatBackgroundSessionVisibility`, 0 to 100.
    pub session_visibility: u32,
}

impl Look {
    /// None when no picture is chosen. Missing or invalid values take the React defaults.
    pub fn from_settings(settings: &Value) -> Option<Self> {
        let name = settings["chatBackground"]
            .as_str()
            .filter(|n| !n.is_empty())?;
        let visibility =
            |key: &str| settings[key].as_u64().filter(|v| *v <= 100).unwrap_or(24) as u32;
        Some(Self {
            name: name.to_owned(),
            effect: Effect::from_setting(
                settings["chatBackgroundEffect"].as_str().unwrap_or("none"),
            ),
            empty_only: settings["chatBackgroundShowOn"].as_str() == Some("empty"),
            empty_visibility: visibility("chatBackgroundEmptyVisibility"),
            session_visibility: visibility("chatBackgroundSessionVisibility"),
        })
    }

    /// The paint opacity: the empty conversation's visibility before it has messages, the
    /// session's after. With "Só vazias" a conversation with messages shows no picture.
    pub fn opacity(&self, empty: bool) -> f32 {
        if empty {
            self.empty_visibility as f32 / 100.
        } else if self.empty_only {
            0.
        } else {
            self.session_visibility as f32 / 100.
        }
    }
}

/// Monocode's background_fade: the share of the picture kept at `y` (0 at the top, 1 at the
/// bottom) of the texture.
pub fn fade(y: f32, empty: bool, haze: bool) -> f32 {
    let stops: &[(f32, f32)] = match (empty, haze) {
        (true, true) => &[
            (0., 1.),
            (0.19, 1.),
            (0.30, 0.94),
            (0.42, 0.72),
            (0.54, 0.42),
            (0.66, 0.16),
            (0.78, 0.04),
            (0.90, 0.),
        ],
        (false, true) => &[(0., 1.), (0.18, 1.), (0.76, 0.)],
        (true, false) => &[
            (0., 1.),
            (0.19, 1.),
            (0.30, 0.79),
            (0.42, 0.50),
            (0.54, 0.20),
            (0.64, 0.05),
            (0.72, 0.),
        ],
        (false, false) => &[
            (0., 1.),
            (0.18, 1.),
            (0.32, 0.61),
            (0.47, 0.32),
            (0.62, 0.11),
            (0.72, 0.),
        ],
    };
    for pair in stops.windows(2) {
        if y <= pair[1].0 {
            let t = ((y - pair[0].0) / (pair[1].0 - pair[0].0)).clamp(0., 1.);
            return pair[0].1 + (pair[1].1 - pair[0].1) * t;
        }
    }
    0.
}

/// What the texture is made for. The fade curve follows `empty`, so it is part of the key;
/// the opacity is not.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub path: PathBuf,
    pub effect: Effect,
    /// The screen the picture covers at Retina scale (the largest display).
    pub screen: (u32, u32),
    pub empty: bool,
}

/// The texture job, shared by every view that asks for the same key.
struct Texture;

impl Asset for Texture {
    type Source = Key;
    type Output = Result<Arc<RenderImage>, String>;

    fn load(key: Key, _cx: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        async move {
            let pixels = prepare(&key)?;
            Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
        }
    }
}

/// The picture behind the conversation card, cover-fitted and faded, or None when it is off.
/// The caller places it between the card's corner masks so it is clipped to the card.
pub fn layer(
    settings: &Value,
    empty: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<gpui::AnyElement> {
    let look = Look::from_settings(settings)?;
    let opacity = look.opacity(empty);
    if opacity <= 0. {
        return None;
    }
    let key = Key {
        path: backend::background_path(&look.name),
        effect: look.effect,
        screen: screen_size(cx),
        empty,
    };
    match window.use_asset::<Texture>(&key, cx) {
        Some(Ok(image)) => Some(
            gpui::img(image)
                .absolute()
                .size_full()
                .object_fit(ObjectFit::Cover)
                .opacity(opacity)
                .into_any_element(),
        ),
        _ => None,
    }
}

/// Twice the largest display's size: scaled modes draw windows at twice their logical size.
fn screen_size(cx: &App) -> (u32, u32) {
    cx.displays()
        .iter()
        .fold((0, 0), |(width, height), display| {
            let size = display.bounds().size;
            (
                width.max((size.width.as_f32() * 2.).ceil() as u32),
                height.max((size.height.as_f32() * 2.).ceil() as u32),
            )
        })
}

/// Decodes with bounded allocations, then caps the longer side at `SOURCE_CAP`.
fn read_source(path: &Path) -> Result<RgbaImage, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("Cannot read image: {e}"))?;
    if metadata.len() > SOURCE_BYTES_CAP {
        return Err("Choose an image smaller than 30 MB.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|_| "Choose a PNG, JPEG, GIF, WebP, or BMP image.".to_owned())?;
    let image = if image.width() > SOURCE_CAP || image.height() > SOURCE_CAP {
        image.thumbnail(SOURCE_CAP, SOURCE_CAP)
    } else {
        image
    };
    Ok(image.into_rgba8())
}

/// The texture's pixels in GPUI's channel order (BGRA), ready for `RenderImage`.
fn prepare(key: &Key) -> Result<RgbaImage, String> {
    let source = read_source(&key.path)?;
    let mut pixels = texture(&source, key.effect, key.screen, key.empty);
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(pixels)
}

/// The smallest size that still covers `screen`, when the artwork is larger than the screen;
/// otherwise the artwork's own size.
fn covering(artwork: (u32, u32), screen: (u32, u32)) -> (u32, u32) {
    let (width, height) = artwork;
    if screen.0 == 0 || screen.1 == 0 || width == 0 || height == 0 {
        return artwork;
    }
    let scale = (screen.0 as f64 / width as f64).max(screen.1 as f64 / height as f64);
    if scale >= 1. {
        return artwork;
    }
    (
        ((width as f64 * scale).ceil() as u32).clamp(1, width),
        ((height as f64 * scale).ceil() as u32).clamp(1, height),
    )
}

/// Monocode's order: the pattern effects run at the artwork's resolution and the result is
/// cover-fitted; Haze blurs the cover-fitted picture. The fade goes into the alpha last.
pub fn texture(source: &RgbaImage, effect: Effect, screen: (u32, u32), empty: bool) -> RgbaImage {
    let target = covering(source.dimensions(), screen);
    let pixels = match effect {
        Effect::Haze => haze(&cover(source, target), empty),
        Effect::None => cover(source, target),
        pattern => cover(&pattern_effect(source, pattern), target),
    };
    let mut pixels = pixels;
    let height = pixels.height();
    let haze_curve = effect == Effect::Haze;
    for (_, y, pixel) in pixels.enumerate_pixels_mut() {
        let keep = fade((y as f32 + 0.5) / height as f32, empty, haze_curve);
        pixel[3] = (pixel[3] as f32 * keep).round() as u8;
    }
    pixels
}

fn cover(source: &RgbaImage, (width, height): (u32, u32)) -> RgbaImage {
    let aspect = width as f64 / height as f64;
    let crop_width = source
        .width()
        .min((source.height() as f64 * aspect).round().max(1.) as u32);
    let crop_height = source
        .height()
        .min((source.width() as f64 / aspect).round().max(1.) as u32);
    let cropped = imageops::crop_imm(
        source,
        (source.width() - crop_width) / 2,
        (source.height() - crop_height) / 2,
        crop_width,
        crop_height,
    )
    .to_image();
    if cropped.dimensions() == (width, height) {
        cropped
    } else {
        imageops::resize(&cropped, width, height, imageops::FilterType::Triangle)
    }
}

const BAYER: [[f32; 4]; 4] = [
    [0., 8., 2., 10.],
    [12., 4., 14., 6.],
    [3., 11., 1., 9.],
    [15., 7., 13., 5.],
];

// 5×7 glyph rows, one byte per row (the low five bits), for the ASCII ramp.
const GLYPHS: [[u8; 7]; 10] = [
    [0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 4, 0],
    [0, 4, 0, 0, 4, 0, 0],
    [0, 0, 0, 14, 0, 0, 0],
    [0, 0, 14, 0, 14, 0, 0],
    [0, 4, 4, 31, 4, 4, 0],
    [0, 21, 14, 31, 14, 21, 0],
    [10, 10, 31, 10, 31, 10, 10],
    [17, 2, 4, 4, 8, 16, 17],
    [14, 17, 23, 21, 23, 16, 14],
];

fn luma(p: &image::Rgba<u8>) -> f32 {
    (p[0] as f32 * 0.2126 + p[1] as f32 * 0.7152 + p[2] as f32 * 0.0722).round()
}

/// Dither, ASCII, Halftone and Scanlines, at the source's resolution (Monocode's effect_pixels).
fn pattern_effect(source: &RgbaImage, effect: Effect) -> RgbaImage {
    let (width, height) = source.dimensions();
    let mut output = source.clone();
    let sample = |x: u32, y: u32| source.get_pixel(x.min(width - 1), y.min(height - 1));
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        match effect {
            Effect::Dither => {
                let p = sample(x / 2 * 2 + 1, y / 2 * 2 + 1);
                let peak = p[0].max(p[1]).max(p[2]) as f32;
                let gain = if peak / 255.
                    > (BAYER[(y / 2 % 4) as usize][(x / 2 % 4) as usize] + 0.5) / 16.
                {
                    255. / peak.max(1.)
                } else {
                    0.08
                };
                for c in 0..3 {
                    pixel[c] = (p[c] as f32 * gain).round() as u8;
                }
                pixel[3] = p[3];
            }
            Effect::Ascii => {
                let p = sample(x / 6 * 6 + 3, y / 8 * 8 + 4);
                let glyph = &GLYPHS[((luma(p) / 255.).sqrt() * 9.) as usize];
                let ink =
                    x % 6 < 5 && y % 8 < 7 && (glyph[(y % 8) as usize] & (1 << (4 - x % 6))) != 0;
                for c in 0..3 {
                    pixel[c] = (pixel[c] as f32 * 0.6 + if ink { p[c] as f32 * 0.4 } else { 0. })
                        .round() as u8;
                }
            }
            Effect::Halftone => {
                let radius = 2. * (0.3 + 0.7 * (luma(sample(x / 4 * 4, y / 4 * 4)) / 255.).sqrt());
                let p = sample(x / 4 * 4 + 2, y / 4 * 4 + 2);
                let distance = (x as f32 % 4. - 1.5).hypot(y as f32 % 4. - 1.5);
                let coverage = (radius + 0.5 - distance).clamp(0., 1.) * p[3] as f32 / 255.;
                for c in 0..3 {
                    pixel[c] = (pixel[c] as f32 * 0.6 + p[c] as f32 * coverage * 0.4).round() as u8;
                }
            }
            Effect::Scanlines => {
                if y % 3 == 0 {
                    for c in 0..3 {
                        pixel[c] = (pixel[c] as f32 * 0.52).round() as u8;
                    }
                }
            }
            Effect::None | Effect::Haze => {}
        }
    }
    output
}

/// Haze: the picture blurs and darkens toward the bottom, with the lines of a display.
fn haze(source: &RgbaImage, empty: bool) -> RgbaImage {
    let soft = imageops::blur(source, if empty { 7. } else { 5. });
    let strong = imageops::blur(source, if empty { 18. } else { 12. });
    let height = source.height();
    let mut output = source.clone();
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        let at = y as f32 / height as f32;
        let soft_mix = ((at - 0.18) / if empty { 0.42 } else { 0.52 }).clamp(0., 1.);
        let strong_mix = ((at - if empty { 0.28 } else { 0.22 }) / if empty { 0.48 } else { 0.54 })
            .clamp(0., 1.);
        let darken = if empty {
            ((at - 0.16) / 0.59).clamp(0., 1.) * 0.78
        } else {
            ((at - 0.15) / 0.61).clamp(0., 1.) * 0.70
        };
        for c in 0..3 {
            let a = pixel[c] as f32 * (1. - soft_mix) + soft.get_pixel(x, y)[c] as f32 * soft_mix;
            let b = a * (1. - strong_mix) + strong.get_pixel(x, y)[c] as f32 * strong_mix;
            let mix = if y % 3 == 0 {
                1. - (1. - darken) * 0.64
            } else {
                darken
            };
            pixel[c] = (b * (1. - mix) + 23. * mix).round() as u8;
        }
    }
    output
}

/// Monocode's default for a first load: Dither, empty conversations only, 24% and 65%.
/// Copies the Monocode picture into the back end's backgrounds folder and sets it, only when no
/// chat background is set and the picture can be read. Returns true when `settings` changed;
/// the caller saves them through the normal settings save.
pub fn seed_monocode_default(settings: &mut Value) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let source = PathBuf::from(home).join(MONOCODE_PICTURE);
    seed_from(settings, &source, &backend::data_dir().join("backgrounds"))
}

fn seed_from(settings: &mut Value, source: &Path, folder: &Path) -> bool {
    if !settings.is_object() || folder.join(MARKER).exists() {
        return false;
    }
    let _ = std::fs::create_dir_all(folder);
    let _ = std::fs::write(folder.join(MARKER), b"");
    let unset = settings["chatBackground"]
        .as_str()
        .is_none_or(str::is_empty);
    if !unset || std::fs::copy(source, folder.join(SEEDED_NAME)).is_err() {
        return false;
    }
    settings["chatBackground"] = json!(SEEDED_NAME);
    settings["chatBackgroundEffect"] = json!("dither");
    settings["chatBackgroundShowOn"] = json!("empty");
    settings["chatBackgroundEmptyVisibility"] = json!(24);
    settings["chatBackgroundSessionVisibility"] = json!(65);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_keeps_the_picture_then_fades_to_nothing() {
        // Session, no haze: full until 18%, gone at 72%, and the stops interpolate between.
        assert_eq!(fade(0., false, false), 1.);
        assert_eq!(fade(0.18, false, false), 1.);
        assert!((fade(0.47, false, false) - 0.32).abs() < 1e-6);
        assert_eq!(fade(0.72, false, false), 0.);
        assert_eq!(fade(0.9, false, false), 0.);
        // Empty conversations keep the picture longer than sessions do.
        assert!(fade(0.4, true, false) > fade(0.4, false, false));
        assert!((fade(0.30, true, false) - 0.79).abs() < 1e-6);
        // Haze runs to the bottom of the texture.
        assert!((fade(0.76, false, true)).abs() < 1e-6);
        assert!(fade(0.5, false, true) > 0.);
    }

    #[test]
    fn visibility_follows_show_on_and_whether_the_chat_has_messages() {
        let settings = json!({
            "chatBackground": "chat-x.jpg",
            "chatBackgroundShowOn": "all",
            "chatBackgroundEmptyVisibility": 24,
            "chatBackgroundSessionVisibility": 65,
        });
        let look = Look::from_settings(&settings).unwrap();
        assert!(
            (look.opacity(true) - 0.24).abs() < 1e-6,
            "empty uses the empty visibility"
        );
        assert!(
            (look.opacity(false) - 0.65).abs() < 1e-6,
            "sessions use the session visibility"
        );

        let only_empty = Look::from_settings(&json!({
            "chatBackground": "chat-x.jpg",
            "chatBackgroundShowOn": "empty",
            "chatBackgroundEmptyVisibility": 24,
            "chatBackgroundSessionVisibility": 65,
        }))
        .unwrap();
        assert_eq!(
            only_empty.opacity(false),
            0.,
            "Só vazias hides the picture after the first message"
        );
        assert!((only_empty.opacity(true) - 0.24).abs() < 1e-6);

        assert_eq!(
            Look::from_settings(&json!({ "chatBackground": null })),
            None
        );
        let defaults = Look::from_settings(&json!({ "chatBackground": "a.jpg" })).unwrap();
        assert_eq!(defaults.effect, Effect::None);
        assert!(!defaults.empty_only);
        assert_eq!(
            (defaults.empty_visibility, defaults.session_visibility),
            (24, 24)
        );
    }

    #[test]
    fn effect_values_match_the_settings_page() {
        for (saved, effect) in [
            ("none", Effect::None),
            ("dither", Effect::Dither),
            ("ascii", Effect::Ascii),
            ("halftone", Effect::Halftone),
            ("scanlines", Effect::Scanlines),
            ("haze", Effect::Haze),
            ("bogus", Effect::None),
        ] {
            assert_eq!(Effect::from_setting(saved), effect, "{saved}");
        }
    }

    #[test]
    fn cache_key_changes_with_path_effect_size_and_fade_but_not_opacity() {
        let key = |path: &str, effect, screen, empty| Key {
            path: PathBuf::from(path),
            effect,
            screen,
            empty,
        };
        let base = key("/d/a.png", Effect::Dither, (2880, 1800), true);
        assert_eq!(base, key("/d/a.png", Effect::Dither, (2880, 1800), true));
        assert_ne!(base, key("/d/b.png", Effect::Dither, (2880, 1800), true));
        assert_ne!(base, key("/d/a.png", Effect::Ascii, (2880, 1800), true));
        assert_ne!(base, key("/d/a.png", Effect::Dither, (1440, 900), true));
        assert_ne!(base, key("/d/a.png", Effect::Dither, (2880, 1800), false));
        // Opacity is not an input of the key: two looks that differ only in visibility share it.
        let first = Look::from_settings(
            &json!({ "chatBackground": "a.png", "chatBackgroundEmptyVisibility": 10 }),
        )
        .unwrap();
        let second = Look::from_settings(
            &json!({ "chatBackground": "a.png", "chatBackgroundEmptyVisibility": 90 }),
        )
        .unwrap();
        assert_ne!(first.opacity(true), second.opacity(true));
        assert_eq!((first.name, first.effect), (second.name, second.effect));
    }

    #[test]
    fn covering_size_shrinks_only_artwork_larger_than_the_screen() {
        assert_eq!(covering((3360, 2038), (2880, 1800)), (2968, 1800));
        assert_eq!(covering((2048, 1242), (2880, 1800)), (2048, 1242));
        assert_eq!(covering((2048, 1242), (0, 0)), (2048, 1242));
    }

    #[test]
    fn scanlines_darken_every_third_row_of_a_tiny_picture() {
        let source = RgbaImage::from_pixel(2, 3, image::Rgba([200, 200, 200, 255]));
        let result = pattern_effect(&source, Effect::Scanlines);
        assert_eq!(result.get_pixel(0, 0).0, [104, 104, 104, 255]);
        assert_eq!(result.get_pixel(0, 1).0, [200, 200, 200, 255]);
        assert_eq!(result.get_pixel(0, 2).0, [200, 200, 200, 255]);
        assert_eq!(pattern_effect(&source, Effect::Dither).dimensions(), (2, 3));
    }

    #[test]
    fn texture_fades_the_bottom_of_a_tiny_picture_to_transparent() {
        let source = RgbaImage::from_pixel(4, 10, image::Rgba([120, 80, 40, 255]));
        let result = texture(&source, Effect::None, (4, 10), false);
        assert_eq!(result.get_pixel(0, 0)[3], 255);
        assert_eq!(result.get_pixel(0, 9)[3], 0);
    }

    #[test]
    fn seeding_copies_once_and_respects_the_marker() {
        let root = std::env::temp_dir().join(format!("sirus-chat-bg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source = root.join("monocode.png");
        let folder = root.join("backgrounds");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&source, b"png bytes").unwrap();

        let mut settings = json!({ "locale": "en" });
        assert!(seed_from(&mut settings, &source, &folder));
        assert_eq!(settings["chatBackground"], SEEDED_NAME);
        assert_eq!(settings["chatBackgroundEffect"], "dither");
        assert_eq!(settings["chatBackgroundShowOn"], "empty");
        assert_eq!(settings["chatBackgroundEmptyVisibility"], 24);
        assert_eq!(settings["chatBackgroundSessionVisibility"], 65);
        assert_eq!(settings["locale"], "en", "other settings are kept");
        assert_eq!(
            std::fs::read(folder.join(SEEDED_NAME)).unwrap(),
            b"png bytes"
        );

        // The user removes the picture: the marker keeps it removed.
        let mut removed = json!({ "chatBackground": null });
        assert!(!seed_from(&mut removed, &source, &folder));
        assert_eq!(removed["chatBackground"], Value::Null);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn seeding_leaves_a_chosen_picture_and_an_unreadable_source_unset() {
        let root = std::env::temp_dir().join(format!("sirus-chat-bg-b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let folder = root.join("backgrounds");

        let mut chosen =
            json!({ "chatBackground": "chat-mine.jpg", "chatBackgroundEffect": "ascii" });
        assert!(!seed_from(&mut chosen, &root.join("missing.png"), &folder));
        assert_eq!(chosen["chatBackground"], "chat-mine.jpg");
        assert_eq!(chosen["chatBackgroundEffect"], "ascii");
        assert!(
            folder.join(MARKER).exists(),
            "the first load is handled even when nothing is copied"
        );

        let mut unreadable = json!({ "chatBackground": null });
        assert!(!seed_from(
            &mut unreadable,
            &root.join("missing.png"),
            &root.join("other")
        ));
        assert_eq!(unreadable["chatBackground"], Value::Null);

        let _ = std::fs::remove_dir_all(&root);
    }
}
