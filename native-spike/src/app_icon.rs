//! The Dock icon (Settings → Aparência → Ícone do app): a halftone Greek bust in six looks,
//! drawn here with a small rasteriser (ADR-198) and handed to NSApplication at runtime. The
//! bundle's own icon is variant "face" (assets/AppIcon.icns, built from the PNG that the ignored test `write_pngs` writes).

use gpui::RenderImage;
use image::{Frame, RgbaImage, imageops};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// The icon canvas and the squircle inside it: macOS leaves about 10% around the shape.
pub const CANVAS: u32 = 1024;
const SQUIRCLE: f32 = 824.;
const SOURCE: &[u8] = include_bytes!("../assets/app-icons/bust-source.png");
/// The pink the preview used for the star and the glass; only "accent" follows the user's color.
const PINK: [f32; 3] = [229., 138., 176.];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum AppIcon {
    Classic,
    #[default]
    Face,
    Accent,
    Marble,
    Sirius,
    Glass,
}

impl AppIcon {
    pub const ALL: [AppIcon; 6] = [
        AppIcon::Classic,
        AppIcon::Face,
        AppIcon::Accent,
        AppIcon::Marble,
        AppIcon::Sirius,
        AppIcon::Glass,
    ];

    pub fn key(self) -> &'static str {
        match self {
            AppIcon::Classic => "classic",
            AppIcon::Face => "face",
            AppIcon::Accent => "accent",
            AppIcon::Marble => "marble",
            AppIcon::Sirius => "sirius",
            AppIcon::Glass => "glass",
        }
    }

    /// A missing or unknown value is the default, "face".
    pub fn parse(value: &Value) -> AppIcon {
        value
            .as_str()
            .and_then(|name| Self::ALL.into_iter().find(|v| v.key() == name))
            .unwrap_or_default()
    }

    /// The tile's name under its picture.
    pub fn label(self) -> String {
        use crate::i18n::t;
        match self {
            AppIcon::Classic => t("Clássico"),
            AppIcon::Face => t("Rosto"),
            AppIcon::Accent => t("Cor de destaque"),
            AppIcon::Marble => t("Mármore"),
            AppIcon::Sirius => t("Sírius"),
            AppIcon::Glass => t("Vidro"),
        }
        .to_string()
    }
}

type Rgb = [f32; 3];

/// What a render depends on: only "accent" looks at the user's color.
pub fn cache_key(icon: AppIcon, accent: [u8; 3]) -> (AppIcon, Option<[u8; 3]>) {
    (icon, (icon == AppIcon::Accent).then_some(accent))
}

/// The accent as bytes, from a `#rrggbb` (`theme::accent_hex`).
pub fn accent_bytes(hex: &str) -> [u8; 3] {
    let n = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0xe58ab0);
    [(n >> 16) as u8, (n >> 8) as u8, n as u8]
}

// ---------------------------------------------------------------- the picture

/// The bust's luminance as alpha (`max(0, (l - 30) * 1.15)`), one byte per source pixel.
struct Mask {
    side: usize,
    alpha: Vec<u8>,
}

fn mask() -> &'static Mask {
    static MASK: OnceLock<Mask> = OnceLock::new();
    MASK.get_or_init(|| {
        let source = image::load_from_memory(SOURCE)
            .expect("the bust is a PNG")
            .into_rgb8();
        let side = source.width() as usize;
        let alpha = source
            .pixels()
            .map(|p| {
                let l = (p[0] as f32 + p[1] as f32 + p[2] as f32) / 3.;
                ((l - 30.) * 1.15).clamp(0., 255.) as u8
            })
            .collect();
        Mask { side, alpha }
    })
}

impl Mask {
    /// Bilinear coverage at a source position, 0 outside the picture.
    fn at(&self, u: f32, v: f32) -> f32 {
        let max = self.side as f32;
        if u < 0. || v < 0. || u >= max || v >= max {
            return 0.;
        }
        let (x, y) = ((u - 0.5).max(0.), (v - 0.5).max(0.));
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.side - 1), (y0 + 1).min(self.side - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |x: usize, y: usize| self.alpha[y * self.side + x] as f32 / 255.;
        let top = p(x0, y0) * (1. - fx) + p(x1, y0) * fx;
        let bottom = p(x0, y1) * (1. - fx) + p(x1, y1) * fx;
        top * (1. - fy) + bottom * fy
    }
}

fn hex(n: u32) -> Rgb {
    [
        ((n >> 16) & 255) as f32,
        ((n >> 8) & 255) as f32,
        (n & 255) as f32,
    ]
}

fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0., 1.);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// A gradient through (position, color) stops.
fn ramp(stops: &[(f32, Rgb)], t: f32) -> Rgb {
    if t <= stops[0].0 {
        return stops[0].1;
    }
    for pair in stops.windows(2) {
        if t <= pair[1].0 {
            let span = (pair[1].0 - pair[0].0).max(1e-6);
            return lerp(pair[0].1, pair[1].1, (t - pair[0].0) / span);
        }
    }
    stops[stops.len() - 1].1
}

fn over(dst: &mut Rgb, color: Rgb, alpha: f32) {
    for i in 0..3 {
        dst[i] += (color[i] - dst[i]) * alpha;
    }
}

/// Where the bust sits inside the squircle, in units of its side, and how it is colored.
struct Bust {
    x: f32,
    y: f32,
    size: f32,
}

#[derive(Clone, Copy)]
enum Background {
    Solid(Rgb),
    /// Top to bottom.
    Vertical(Rgb, Rgb),
    /// Corner to corner.
    Diagonal(Rgb, Rgb),
    /// The preview's two-circle radial gradient for the night sky.
    Sky,
}

#[derive(Clone, Copy)]
enum Tint {
    Solid(Rgb),
    /// Corner to corner of the source picture: light, accent, darker accent.
    Diagonal([Rgb; 3]),
    /// Top to bottom of the source picture.
    Vertical(Rgb, Rgb),
}

struct Look {
    background: Background,
    bust: Bust,
    tint: Tint,
    star: bool,
    glass: bool,
}

fn look_of(icon: AppIcon, accent: [u8; 3]) -> Look {
    let near = Bust {
        x: -0.3,
        y: -0.12,
        size: 1.4,
    };
    match icon {
        AppIcon::Classic => Look {
            background: Background::Solid(hex(0x050505)),
            bust: Bust {
                x: 0.04,
                y: 0.06,
                size: 0.92,
            },
            tint: Tint::Solid(hex(0xf2ece6)),
            star: false,
            glass: false,
        },
        AppIcon::Face => Look {
            background: Background::Solid(hex(0x050505)),
            bust: Bust {
                x: -0.42,
                y: -0.2,
                size: 1.55,
            },
            tint: Tint::Solid(hex(0xf2ece6)),
            star: false,
            glass: false,
        },
        AppIcon::Accent => {
            let base = [accent[0] as f32, accent[1] as f32, accent[2] as f32];
            Look {
                background: Background::Vertical(hex(0x1a1218), hex(0x070507)),
                bust: near,
                tint: Tint::Diagonal([
                    lerp(base, [255., 255., 255.], 0.55),
                    base,
                    [base[0] * 0.7, base[1] * 0.7, base[2] * 0.7],
                ]),
                star: false,
                glass: false,
            }
        }
        AppIcon::Marble => Look {
            background: Background::Diagonal(hex(0xf7f3ee), hex(0xd9d2ca)),
            bust: near,
            tint: Tint::Solid(hex(0x141214)),
            star: false,
            glass: false,
        },
        AppIcon::Sirius => Look {
            background: Background::Sky,
            bust: Bust {
                x: -0.22,
                y: 0.02,
                size: 1.3,
            },
            tint: Tint::Solid(hex(0xefe9f2)),
            star: true,
            glass: false,
        },
        AppIcon::Glass => Look {
            background: Background::Vertical(hex(0x3a3640), hex(0x121116)),
            bust: near,
            tint: Tint::Vertical(hex(0xffffff), PINK),
            star: false,
            glass: true,
        },
    }
}

/// The squircle `|x|^5 + |y|^5 = r^5` (the preview's superellipse): its signed distance in
/// pixels, near enough, positive outside.
fn squircle_distance(x: f32, y: f32, r: f32) -> f32 {
    const N: f32 = 5.;
    let (a, b) = (x.abs() / r, y.abs() / r);
    let f = a.powf(N) + b.powf(N) - 1.;
    let slope = N / r * (a.powf(2. * N - 2.) + b.powf(2. * N - 2.)).sqrt();
    f / slope.max(1e-4)
}

fn background_at(background: Background, x: f32, y: f32, s: f32) -> Rgb {
    match background {
        Background::Solid(c) => c,
        Background::Vertical(a, b) => lerp(a, b, y / s),
        Background::Diagonal(a, b) => lerp(a, b, (x + y) / (2. * s)),
        Background::Sky => {
            // Circle 0 is a point at (.85, .1); circle 1 is centred at (.6, .5), radius .9.
            let (c0, d, r) = ((0.85 * s, 0.1 * s), (-0.25 * s, 0.4 * s), 0.9 * s);
            let q = (x - c0.0, y - c0.1);
            let a = d.0 * d.0 + d.1 * d.1 - r * r;
            let b = q.0 * d.0 + q.1 * d.1;
            let disc = (b * b - a * (q.0 * q.0 + q.1 * q.1)).max(0.);
            let t = (b - disc.sqrt()) / a;
            lerp(hex(0x2c2440), hex(0x06060c), t)
        }
    }
}

fn tint_at(tint: Tint, u: f32, v: f32, side: f32) -> Rgb {
    match tint {
        Tint::Solid(c) => c,
        Tint::Diagonal([light, accent, dark]) => ramp(
            &[(0., light), (0.6, accent), (1., dark)],
            (u + v) / (2. * side),
        ),
        Tint::Vertical(top, bottom) => lerp(top, bottom, v / side),
    }
}

/// The sparkle: a glow and a four-pointed star (`star` in the preview), as (color, alpha).
fn star_at(x: f32, y: f32, s: f32) -> (Rgb, f32) {
    let (cx, cy, r) = (0.8 * s, 0.2 * s, 0.07 * s);
    let (dx, dy) = ((x - cx).abs(), (y - cy).abs());
    let reach = (dx * dx + dy * dy).sqrt() / (r * 2.2);
    let mut color = [0.; 3];
    let mut alpha = 0.;
    if reach < 1. {
        let stops = [
            (0., [255., 240., 250.], 0.9),
            (0.25, PINK, 0.35),
            (1., PINK, 0.),
        ];
        for pair in stops.windows(2) {
            if reach <= pair[1].0 {
                let t = (reach - pair[0].0) / (pair[1].0 - pair[0].0);
                color = lerp(pair[0].1, pair[1].1, t);
                alpha = pair[0].2 + (pair[1].2 - pair[0].2) * t;
                break;
            }
        }
    }
    let mut out = color;
    let mut total = alpha;
    // Inside the star: sqrt(|dx|/r) + sqrt(|dy|/r) <= 1; one pixel of soft edge.
    let edge = ((dx / r).sqrt() + (dy / r).sqrt() - 1.) * r * 0.35;
    let cover = (0.5 - edge).clamp(0., 1.);
    if cover > 0. {
        over(&mut out, [255., 255., 255.], cover);
        total = total + (1. - total) * cover;
        if alpha == 0. {
            out = [255., 255., 255.];
        }
    }
    (out, total)
}

/// One pixel of the squircle's inside, at (x, y) from its top left corner.
fn shade(look: &Look, x: f32, y: f32, s: f32, mask: &Mask) -> Rgb {
    let mut color = background_at(look.background, x, y, s);
    // The bust: 2x2 samples of its picture per pixel keep the dots from beating.
    let scale = mask.side as f32 / (look.bust.size * s);
    let (left, top) = (look.bust.x * s, look.bust.y * s);
    let mut cover = 0.;
    let mut u_mid = 0.;
    let mut v_mid = 0.;
    for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
        let (u, v) = ((x - left + ox - 0.5) * scale, (y - top + oy - 0.5) * scale);
        cover += mask.at(u, v) / 4.;
        u_mid = (x - left) * scale;
        v_mid = (y - top) * scale;
    }
    if cover > 0. {
        over(
            &mut color,
            tint_at(look.tint, u_mid, v_mid, mask.side as f32),
            cover,
        );
    }
    if look.glass && y < s * 0.5 {
        over(&mut color, [255., 255., 255.], 0.18 * (1. - y / (s * 0.5)));
    }
    if look.star {
        let (c, a) = star_at(x, y, s);
        if a > 0. {
            over(&mut color, c, a);
        }
    }
    if look.glass {
        // The reflection on the rim: an inset squircle outline with a diagonal fade.
        let (inset, half) = (s * 0.006, s * 0.006);
        let d = squircle_distance(x - s / 2., y - s / 2., s / 2. - inset);
        let ring = (half + 0.5 - d.abs()).clamp(0., 1.);
        if ring > 0. {
            let t = (x + y) / (2. * s);
            let alpha = if t < 0.5 {
                0.55 + (0.08 - 0.55) * (t / 0.5)
            } else {
                0.08 + (0.3 - 0.08) * ((t - 0.5) / 0.5)
            };
            over(&mut color, [255., 255., 255.], alpha * ring);
        }
    }
    color
}

/// Three box blurs of the squircle's coverage: a soft, Gaussian-like shadow.
fn blur(values: &mut [f32], side: usize, radius: usize) {
    let mut scratch = vec![0.; values.len()];
    let width = (2 * radius + 1) as f32;
    for _ in 0..3 {
        for vertical in [false, true] {
            for line in 0..side {
                let at = |i: usize| {
                    if vertical {
                        i * side + line
                    } else {
                        line * side + i
                    }
                };
                let mut sum: f32 = (0..=radius.min(side - 1)).map(|i| values[at(i)]).sum();
                for i in 0..side {
                    scratch[at(i)] = sum / width;
                    if i + radius + 1 < side {
                        sum += values[at(i + radius + 1)];
                    }
                    if i >= radius {
                        sum -= values[at(i - radius)];
                    }
                }
            }
            values.copy_from_slice(&scratch);
        }
    }
}

/// The icon at `CANVAS` pixels: the squircle with its soft drop shadow, straight RGBA.
pub fn render(icon: AppIcon, accent: [u8; 3]) -> RgbaImage {
    let side = CANVAS as usize;
    let look = look_of(icon, accent);
    let mask = mask();
    let s = SQUIRCLE;
    let margin = (CANVAS as f32 - s) / 2.;
    let mut cover = vec![0f32; side * side];
    let mut rgb = vec![[0f32; 3]; side * side];
    let lo = (margin - 3.).max(0.) as usize;
    let hi = ((margin + s + 3.) as usize).min(side);
    for py in lo..hi {
        for px in lo..hi {
            let (x, y) = (px as f32 + 0.5 - margin, py as f32 + 0.5 - margin);
            let alpha = (0.5 - squircle_distance(x - s / 2., y - s / 2., s / 2.)).clamp(0., 1.);
            if alpha > 0. {
                cover[py * side + px] = alpha;
                rgb[py * side + px] = shade(&look, x, y, s, mask);
            }
        }
    }
    // Apple's template: a faint shadow that falls a little below the shape.
    let mut shadow = vec![0f32; side * side];
    let drop = 10;
    for py in 0..side - drop {
        let from = py * side;
        let to = (py + drop) * side;
        shadow[to..to + side].copy_from_slice(&cover[from..from + side]);
    }
    blur(&mut shadow, side, 8);
    let mut out = RgbaImage::new(CANVAS, CANVAS);
    for (i, pixel) in out.pixels_mut().enumerate() {
        let (a, sh) = (cover[i], shadow[i] * 0.32);
        let total = a + sh * (1. - a);
        if total <= 0. {
            continue;
        }
        let c = rgb[i];
        let mix = |v: f32| (v * a / total).round().clamp(0., 255.) as u8;
        *pixel = image::Rgba([
            mix(c[0]),
            mix(c[1]),
            mix(c[2]),
            (total * 255.).round() as u8,
        ]);
    }
    out
}

// ------------------------------------------------------------------- the cache

type Key = (AppIcon, Option<[u8; 3]>);

fn cache() -> &'static Mutex<HashMap<Key, Arc<RgbaImage>>> {
    static CACHE: OnceLock<Mutex<HashMap<Key, Arc<RgbaImage>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The 1024 px icon, drawn once per (variant, accent).
pub fn icon_image(icon: AppIcon, accent: [u8; 3]) -> Arc<RgbaImage> {
    let key = cache_key(icon, accent);
    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let made = Arc::new(render(icon, accent));
    if let Ok(mut c) = cache().lock() {
        // Older accents of the accent variant will not come back soon.
        c.retain(|k, _| !(k.0 == AppIcon::Accent && k.1 != key.1));
        c.insert(key, made.clone());
    }
    made
}

/// A picture for the settings tile, the cached icon scaled down, as GPUI wants it (BGRA).
pub fn tile_image(icon: AppIcon, accent: [u8; 3]) -> Arc<RenderImage> {
    type Tiles = Mutex<HashMap<Key, Arc<RenderImage>>>;
    static TILES: OnceLock<Tiles> = OnceLock::new();
    let tiles = TILES.get_or_init(Default::default);
    let key = cache_key(icon, accent);
    if let Some(hit) = tiles.lock().ok().and_then(|t| t.get(&key).cloned()) {
        return hit;
    }
    let full = icon_image(icon, accent);
    let mut small = imageops::resize(&*full, 160, 160, imageops::FilterType::Triangle);
    for pixel in small.pixels_mut() {
        pixel.0 = [
            (pixel[2] as u16 * pixel[3] as u16 / 255) as u8,
            (pixel[1] as u16 * pixel[3] as u16 / 255) as u8,
            (pixel[0] as u16 * pixel[3] as u16 / 255) as u8,
            pixel[3],
        ];
    }
    let made = Arc::new(RenderImage::new(vec![Frame::new(small)]));
    if let Ok(mut t) = tiles.lock() {
        t.retain(|k, _| !(k.0 == AppIcon::Accent && k.1 != key.1));
        t.insert(key, made.clone());
    }
    made
}

// -------------------------------------------------------------------- the Dock

/// What the Dock shows now, so a frame that changes nothing does nothing.
static APPLIED: Mutex<Option<Key>> = Mutex::new(None);

/// Sets the Dock icon, when it is not already this one. Main thread only.
pub fn apply(icon: AppIcon, accent: [u8; 3]) {
    let key = cache_key(icon, accent);
    if APPLIED.lock().map(|a| *a == Some(key)).unwrap_or(false) {
        return;
    }
    let image = icon_image(icon, accent);
    if set_dock_icon(&image) {
        if let Ok(mut applied) = APPLIED.lock() {
            *applied = Some(key);
        }
    }
}

#[cfg(target_os = "macos")]
fn set_dock_icon(image: &RgbaImage) -> bool {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let mut png = Vec::new();
    if image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .is_err()
    {
        return false;
    }
    let data = NSData::with_bytes(&png);
    let Some(icon) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return false;
    };
    // SAFETY: on the main thread, with an image we own.
    unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&icon)) };
    true
}

#[cfg(not(target_os = "macos"))]
fn set_dock_icon(_: &RgbaImage) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_setting_parses_and_a_missing_one_is_face() {
        for icon in AppIcon::ALL {
            assert_eq!(AppIcon::parse(&json!(icon.key())), icon);
        }
        assert_eq!(AppIcon::parse(&Value::Null), AppIcon::Face);
        assert_eq!(AppIcon::parse(&json!("nope")), AppIcon::Face);
        assert_eq!(AppIcon::parse(&json!(3)), AppIcon::Face);
        assert_eq!(AppIcon::default(), AppIcon::Face);
    }

    #[test]
    fn only_the_accent_variant_is_keyed_by_the_accent() {
        let (pink, blue) = ([229, 138, 176], [60, 120, 255]);
        for icon in AppIcon::ALL {
            let changes = cache_key(icon, pink) != cache_key(icon, blue);
            assert_eq!(changes, icon == AppIcon::Accent, "{icon:?}");
        }
    }

    #[test]
    fn the_accent_reads_from_a_hex() {
        assert_eq!(accent_bytes("#e58ab0"), [229, 138, 176]);
    }

    #[test]
    fn every_variant_renders_a_full_icon_with_clear_corners() {
        for icon in AppIcon::ALL {
            let image = render(icon, [229, 138, 176]);
            assert_eq!((image.width(), image.height()), (1024, 1024));
            for (x, y) in [(0, 0), (1023, 0), (0, 1023), (1023, 1023), (30, 30)] {
                assert_eq!(image.get_pixel(x, y)[3], 0, "{icon:?} at {x},{y}");
            }
            // The squircle's middle is opaque and not blank.
            let middle = image.get_pixel(512, 512);
            assert_eq!(middle[3], 255, "{icon:?}");
            let inked = image.pixels().filter(|p| p[3] == 255).count();
            assert!(inked > 500_000, "{icon:?}: {inked}");
            let colors: std::collections::HashSet<[u8; 4]> =
                image.pixels().step_by(997).map(|p| p.0).collect();
            assert!(colors.len() > 8, "{icon:?}");
        }
    }

    #[test]
    fn the_accent_variant_wears_the_accent() {
        let a = render(AppIcon::Accent, [255, 0, 0]);
        let b = render(AppIcon::Accent, [0, 0, 255]);
        assert!(a.as_raw() != b.as_raw());
    }

    /// `SIRUS_ICON_DIR=/tmp/sirus-icons cargo test --offline write_pngs -- --ignored`
    #[test]
    #[ignore]
    fn write_pngs() {
        let dir = std::env::var("SIRUS_ICON_DIR").unwrap_or_else(|_| "/tmp/sirus-icons".into());
        std::fs::create_dir_all(&dir).unwrap();
        let accent = accent_bytes(&std::env::var("SIRUS_ICON_ACCENT").unwrap_or("#e58ab0".into()));
        for icon in AppIcon::ALL {
            render(icon, accent)
                .save(format!("{dir}/{}.png", icon.key()))
                .unwrap();
        }
    }
}
