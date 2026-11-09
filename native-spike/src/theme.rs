//! The dark tokens (theme.rs): translucent ink over a blurred window.
use gpui::{AnyElement, Hsla, Rgba, Svg, canvas, hsla, prelude::*, px, rgb, svg};
use std::cell::Cell;
pub const TITLE_BAR: f32 = 40.;
pub const FOOTER: f32 = 30.;
pub const PLATE: f32 = 27.;
/// The app's three corners: buttons, chips and rows; fields, menus and inline cards; the
/// composer, popups, floating panels and drawers. A full pill is only for tiny badges and dots.
pub const RADIUS_SM: f32 = 7.;
pub const RADIUS_MD: f32 = 10.;
pub const RADIUS_LG: f32 = 14.;
pub const TAB_PLATE_HEIGHT: f32 = PLATE;
pub fn content(alpha: f32) -> Hsla {
    let mut color = current().ink;
    color.a = alpha;
    color
}
/// The one edge every surface wears: 1 px at 10% ink. Accent, amber, red and selected
/// borders keep their own colors.
pub fn border() -> Hsla {
    content(0.1)
}
pub fn white(alpha: f32) -> Hsla {
    gpui::hsla(0., 0., 1., alpha)
}
pub fn black(alpha: f32) -> Hsla {
    gpui::hsla(0., 0., 0., alpha)
}
pub fn is_light() -> bool {
    current().ink.l < 0.5
}
/// The colors a workspace theme sets; they ease from one theme to the next.
#[derive(Clone, Copy)]
pub struct Palette {
    pub shell: Hsla,
    pub pane: Hsla,
    pub field: Hsla,
    pub floating: Hsla,
    pub accent: Hsla,
    pub ink: Hsla,
}

/// Everything Aparência sets: theme, workspace theme, accent,
/// diff colors, a custom tint, the pane's glass and the interface scale.
#[derive(Clone, Copy, PartialEq)]
pub struct Look {
    pub preset: usize,
    pub accent: Option<usize>,
    /// A color of one's own, hue (0–360) and lightness; it wins over `accent`.
    pub custom: Option<(f32, f32)>,
    /// 0 follows the system, 1 dark, 2 light.
    pub mode: usize,
    pub diff: usize,
    pub hue: Option<f32>,
    pub saturation: Option<f32>,
    pub glass: bool,
    pub scale: f32,
    /// The Dock icon (app_icon.rs); it lives with the look, in this Mac's own file.
    pub app_icon: crate::app_icon::AppIcon,
}

impl Default for Look {
    /// The owner's look: Ink, the pink accent, following the system, with glass.
    fn default() -> Self {
        Self {
            preset: PRESETS.len() - 1,
            accent: Some(ROSE),
            custom: None,
            mode: 0,
            diff: 0,
            hue: None,
            saturation: None,
            glass: true,
            scale: 1.,
            app_icon: crate::app_icon::AppIcon::default(),
        }
    }
}

/// A workspace theme: a tint for the chrome and surfaces, and its accent.
pub struct Preset {
    pub name: &'static str,
    hue: f32,
    saturation: f32,
    pub accent: u32,
    /// The dark chrome's lightness and its ink.
    dark_lightness: f32,
    dark_ink: u32,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "Sirus",
        hue: 255.,
        saturation: 0.10,
        accent: 0xbdb4f2,
        dark_lightness: 0.06,
        dark_ink: 0xebebeb,
    },
    Preset {
        name: "Carvão",
        hue: 0.,
        saturation: 0.,
        accent: 0xe6e6e6,
        dark_lightness: 0.06,
        dark_ink: 0xebebeb,
    },
    Preset {
        name: "Oceano",
        hue: 212.,
        saturation: 0.28,
        accent: 0x6fb3f2,
        dark_lightness: 0.06,
        dark_ink: 0xebebeb,
    },
    Preset {
        name: "Floresta",
        hue: 150.,
        saturation: 0.20,
        accent: 0x74c69d,
        dark_lightness: 0.06,
        dark_ink: 0xebebeb,
    },
    Preset {
        name: "Brasa",
        hue: 18.,
        saturation: 0.24,
        accent: 0xf0925e,
        dark_lightness: 0.06,
        dark_ink: 0xebebeb,
    },
    Preset {
        name: "Ameixa",
        hue: 300.,
        saturation: 0.20,
        accent: 0xd08bd9,
        dark_lightness: 0.06,
        dark_ink: 0xebebeb,
    },
    Preset {
        name: "Areia",
        hue: 42.,
        saturation: 0.18,
        accent: 0xdcbf7a,
        dark_lightness: 0.06,
        dark_ink: 0xebebeb,
    },
    Preset {
        name: "Coral",
        hue: 350.,
        saturation: 0.20,
        accent: 0xee8a96,
        dark_lightness: 0.06,
        dark_ink: 0xebebeb,
    },
    Preset {
        name: "Aurora",
        hue: 176.,
        saturation: 0.24,
        accent: 0x5fd4c4,
        dark_lightness: 0.06,
        dark_ink: 0xebebeb,
    },
    Preset {
        name: "Neon",
        hue: 285.,
        saturation: 0.42,
        accent: 0xe45fd8,
        dark_lightness: 0.06,
        dark_ink: 0xebebeb,
    },
    Preset {
        name: "Tinta",
        hue: 0.,
        saturation: 0.,
        accent: 0x7aa7ff,
        dark_lightness: 0.,
        dark_ink: 0xf5f5f5,
    },
];

/// `ACCENTS`' index of the pink.
const ROSE: usize = 3;

pub const ACCENTS: &[(&str, u32)] = &[
    ("Lilás", 0xbdb4f2),
    ("Branco", 0xf2f2f2),
    ("Azul", 0x74a4f7),
    ("Rosa", 0xec4899),
    ("Coral", 0xe0654d),
    ("Âmbar", 0xe0a526),
    ("Verde", 0x62b67f),
    ("Ciano", 0x4fc3cf),
];

/// The `sidebar_opacity` as the owner has it: how much of the desktop the tint hides.
const SHELL_ALPHA: f32 = 0.81;

pub fn palette(look: &Look, system_dark: bool) -> Palette {
    let p = &PRESETS[look.preset];
    let hue = look.hue.unwrap_or(p.hue) / 360.;
    let saturation = look.saturation.unwrap_or(p.saturation);
    let dark = match look.mode {
        1 => true,
        2 => false,
        _ => system_dark,
    };
    let tint = |s: f32, lightness: f32, alpha: f32| hsla(hue, s, lightness, alpha);
    // Dark glass (#060606 at 40% over the window's tint): the conversation sits a step
    // darker than the chrome around it.
    let pane_alpha = match (look.glass, dark) {
        (true, true) => 0.4,
        (true, false) => 0.9,
        (false, _) => 1.,
    };
    let accent: Hsla = match look.custom {
        Some((hue, light)) => hsla(hue / 360., 0.75, light, 1.),
        None => rgb(look.accent.map_or(p.accent, |a| ACCENTS[a].1)).into(),
    };
    if dark {
        Palette {
            shell: tint(
                saturation * 0.7,
                p.dark_lightness,
                if look.glass { SHELL_ALPHA } else { 1. },
            ),
            pane: tint(saturation, p.dark_lightness * 0.4, pane_alpha),
            field: tint(saturation * 0.8, (p.dark_lightness + 0.075).max(0.11), 1.),
            floating: tint(saturation * 0.7, (p.dark_lightness + 0.08).max(0.11), 1.),
            accent,
            ink: rgb(p.dark_ink).into(),
        }
    } else {
        Palette {
            shell: tint(saturation * 0.5, 0.92, if look.glass { 0.86 } else { 1. }),
            pane: tint(saturation * 0.4, 0.985, pane_alpha),
            field: tint(saturation * 0.3, 1., 1.),
            floating: tint(saturation * 0.3, 0.99, 1.),
            accent,
            ink: rgb(0x1c1c20).into(),
        }
    }
}

/// The accent a look resolves to (a custom colour included) as `#rrggbb`, which the phone wears.
pub fn accent_hex(look: &Look) -> String {
    let color = palette(look, true).accent.to_rgb();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(color.r),
        byte(color.g),
        byte(color.b)
    )
}

pub fn mix(from: Palette, to: Palette, t: f32) -> Palette {
    let one = |a: Hsla, b: Hsla| -> Hsla {
        let (a, b) = (a.to_rgb(), b.to_rgb());
        Rgba {
            r: a.r + (b.r - a.r) * t,
            g: a.g + (b.g - a.g) * t,
            b: a.b + (b.b - a.b) * t,
            a: a.a + (b.a - a.a) * t,
        }
        .into()
    };
    Palette {
        shell: one(from.shell, to.shell),
        pane: one(from.pane, to.pane),
        field: one(from.field, to.field),
        floating: one(from.floating, to.floating),
        accent: one(from.accent, to.accent),
        ink: one(from.ink, to.ink),
    }
}

/// The added and removed line counts' colors and type, as Monocode draws them: soft mint and
/// rose, 10 px medium, with tabular digits so the numbers do not shift as they change.
pub const DIFF_ADD: u32 = 0x6ee7b7;
pub const DIFF_DEL: u32 = 0xfda4af;
pub const DIFF_COUNT_SIZE: f32 = 10.;

pub fn tabular() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".to_owned(), 1)]))
}

/// A count with thousands separators: 1,234.
pub fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// "+added -removed" as small tabular text in the diff colors; a side with nothing is dropped
/// when `skip_zero`.
pub fn diff_counts(added: u64, removed: u64, skip_zero: bool) -> gpui::Div {
    use gpui::FontWeight;
    let mut row = gpui::div()
        .flex()
        .items_center()
        .gap(px(6.))
        .whitespace_nowrap()
        .text_size(px(DIFF_COUNT_SIZE))
        .font_weight(FontWeight::MEDIUM)
        .font_features(tabular());
    if !(skip_zero && added == 0) {
        row = row.child(
            gpui::div()
                .text_color(rgb(DIFF_ADD))
                .child(format!("+{}", format_count(added))),
        );
    }
    if !(skip_zero && removed == 0) {
        row = row.child(
            gpui::div()
                .text_color(rgb(DIFF_DEL))
                .child(format!("-{}", format_count(removed))),
        );
    }
    row
}

/// The diff's added and removed colors: default, colorblind or high contrast.
pub fn diff_colors(diff: usize) -> (Hsla, Hsla) {
    let (add, remove) = match diff {
        1 => (0x6aa9f4, 0xf0a04b),
        2 => (0x58a6ff, 0xff9a3c),
        _ => (0x7ad69a, 0xee8fa2),
    };
    (rgb(add).into(), rgb(remove).into())
}

thread_local! {
    static CURRENT: Cell<Palette> = Cell::new(palette(&Look::default(), true));
}
pub fn apply(palette: Palette) {
    CURRENT.with(|c| c.set(palette));
}
pub fn current_palette() -> Palette {
    current()
}
fn current() -> Palette {
    CURRENT.with(|c| c.get())
}

pub fn accent() -> Hsla {
    current().accent
}
/// The accent at `alpha`: message bubbles.
pub fn accent_wash(alpha: f32) -> Hsla {
    let mut color = accent();
    color.a = alpha;
    color
}
/// Black or white on the accent, by its relative luminance.
pub fn on_accent() -> Hsla {
    let color = accent().to_rgb();
    let linear = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b);
    if luminance > 0.179 {
        gpui::black()
    } else {
        gpui::white()
    }
}
/// The send button: the accent with its ink, dimmed with nothing to send.
pub fn primary_action(enabled: bool) -> (Hsla, Hsla) {
    if enabled {
        (accent(), on_accent())
    } else {
        (accent_wash(0.28), content(0.35))
    }
}
/// Selected text is tinted with the accent; a paler wash keeps dark ink readable on light.
pub fn text_selection() -> Hsla {
    accent_wash(if is_light() { 0.2 } else { 0.35 })
}
/// The text cursor: solid, in the text's own ink; it never blinks.
pub const CARET_WIDTH: f32 = 1.5;
pub fn caret_color() -> Hsla {
    content(1.)
}
pub fn shell_fill() -> Hsla {
    current().shell
}
/// The chrome's wash over the glass: title bar, sidebar, gutters and footer.
pub fn chrome_wash() -> Hsla {
    content(0.05)
}
/// The card the conversation sits on.
pub fn pane_fill() -> Hsla {
    current().pane
}
pub fn pane_opaque() -> Hsla {
    let mut pane = current().pane;
    pane.a = 1.;
    pane
}
/// The composer's raised card.
pub fn field_fill() -> Hsla {
    current().field
}
pub fn floating_fill() -> Hsla {
    current().floating
}
/// The floating fill with no translucency, for small surfaces that must not show what is beneath.
pub fn opaque_floating() -> Hsla {
    let mut fill = current().floating;
    fill.a = 1.;
    fill
}
/// A frosted surface: `tint` over a backdrop blur, or `opaque` in light themes. Put it first
/// inside a card with the same corners; it fills the card and paints under the children.
fn frosted(radius: f32, tint: Hsla, opaque: Hsla, blur: f32) -> impl IntoElement {
    let light = is_light();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let quad = if light {
                gpui::fill(bounds, opaque)
            } else {
                gpui::fill(bounds, tint)
                    .border_color(gpui::backdrop_blur_marker(blur * window.scale_factor()))
            };
            window.paint_quad(quad.corner_radii(px(radius)));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}
/// Popups and dialogs: frosted glass.
pub fn floating_surface(radius: f32) -> impl IntoElement {
    // A light film, so a popup reads a step above whatever sits beneath, even over black.
    frosted(radius, content(0.08), floating_fill(), 30.)
}
/// The sidebar opened over the workspace: a faint wash over a 24 px frost.
pub fn peek_surface(radius: f32) -> impl IntoElement {
    frosted(radius, content(0.05), floating_fill(), 24.)
}
/// The composer: ink at 3% over a 24 px frost.
pub fn composer_surface(radius: f32) -> impl IntoElement {
    frosted(radius, content(0.03), field_fill(), 24.)
}
/// 14% ink. Translucent, because the conversation can sit on glass, where
/// a colour mixed from the opaque pane reads as a black line.
pub fn rail_color() -> Hsla {
    content(0.14)
}
/// The one scale icons come in. A size asked for between two steps takes the nearest.
const ICON_SIZES: [f32; 5] = [12., 14., 16., 18., 24.];

fn icon_size(side: f32) -> f32 {
    ICON_SIZES
        .into_iter()
        .min_by(|a, b| (a - side).abs().total_cmp(&(b - side).abs()))
        .unwrap_or(side)
}

pub fn icon(path: &'static str, side: f32) -> Svg {
    svg()
        .path(path)
        .size(px(icon_size(side)))
        .text_color(content(0.5))
        .flex_shrink_0()
}
pub fn provider_tile(provider: &str) -> Option<(Hsla, Hsla)> {
    let (top, bottom) = match provider {
        "claude" => (0xE3876A, 0xD06A49),
        "codex" => (0x7A8CFF, 0x4048FF),
        "droid" => (0xE0733A, 0xB9521F),
        "antigravity" => (0x4D4D55, 0x2C2C33),
        "cursor" | "grok" | "pi" => (0x343434, 0x161616),
        "hermes" | "devin" => (0x3A3A3E, 0x1E1E22),
        _ => (0x3B3636, 0x1F1C1C),
    };
    Some((rgb(top).into(), rgb(bottom).into()))
}
pub fn provider_glyph(provider: &str, side: f32) -> AnyElement {
    provider_glyph_in(provider, side, gpui::white())
}
/// A provider's one-colour mark in `color`.
pub fn provider_glyph_in(provider: &str, side: f32, color: Hsla) -> AnyElement {
    let path = match provider {
        "claude" => "claude.svg",
        "codex" => "codex-glyph.svg",
        "cursor" => "cursor",
        "grok" => "grok",
        "antigravity" => "antigravity",
        "droid" => "droid",
        "pi" => "pi",
        "devin" => "devin",
        "hermes" => "hermes",
        _ => "opencode.svg",
    };
    svg()
        .path(path)
        .size(px(side))
        .text_color(color)
        .flex_shrink_0()
        .into_any_element()
}

/// The look on disk, so Aparência survives a restart.
pub fn look_to_json(look: &Look) -> serde_json::Value {
    serde_json::json!({
        "preset": PRESETS[look.preset].name,
        "accent": look.accent,
        "custom": look.custom,
        "mode": look.mode,
        "diff": look.diff,
        "hue": look.hue,
        "saturation": look.saturation,
        "glass": look.glass,
        "scale": look.scale,
        "appIcon": look.app_icon.key(),
    })
}
/// Anything missing or out of range keeps the default.
pub fn look_from_json(value: &serde_json::Value) -> Look {
    let mut look = Look::default();
    if let Some(at) = value["preset"]
        .as_str()
        .and_then(|name| PRESETS.iter().position(|p| p.name == name))
    {
        look.preset = at;
    }
    if value.get("accent").is_some() {
        look.accent = value["accent"]
            .as_u64()
            .map(|a| a as usize)
            .filter(|a| *a < ACCENTS.len());
    }
    look.custom = serde_json::from_value(value["custom"].clone()).unwrap_or(None);
    look.mode = value["mode"]
        .as_u64()
        .map_or(look.mode, |m| (m as usize).min(2));
    look.diff = value["diff"]
        .as_u64()
        .map_or(look.diff, |d| (d as usize).min(2));
    look.hue = value["hue"].as_f64().map(|v| v as f32);
    look.saturation = value["saturation"].as_f64().map(|v| v as f32);
    look.glass = value["glass"].as_bool().unwrap_or(look.glass);
    look.scale = value["scale"].as_f64().map_or(look.scale, |v| v as f32);
    look.app_icon = crate::app_icon::AppIcon::parse(&value["appIcon"]);
    look
}

#[cfg(test)]
mod look_tests {
    use super::*;

    #[test]
    fn the_default_look_is_ink_with_the_pink_accent() {
        let look = Look::default();
        assert_eq!(PRESETS[look.preset].name, "Tinta");
        assert_eq!(ACCENTS[look.accent.unwrap()].1, 0xec4899);
        let dark = palette(&look, true);
        assert_eq!(dark.pane.a, 0.4);
        assert_eq!(dark.shell.a, SHELL_ALPHA);
        assert_eq!(dark.shell.l, 0.);
    }

    #[test]
    fn a_saved_look_comes_back_and_a_broken_one_keeps_defaults() {
        let look = Look {
            preset: 2,
            accent: None,
            custom: Some((120., 0.5)),
            glass: false,
            ..Look::default()
        };
        assert!(look_from_json(&look_to_json(&look)) == look);
        let plain = Look {
            preset: 2,
            accent: Some(1),
            ..Look::default()
        };
        assert!(look_from_json(&look_to_json(&plain)) == plain);
        assert!(
            look_from_json(&serde_json::json!({"preset": "?", "accent": 99, "mode": "x"}))
                .accent
                .is_none()
        );
        assert!(look_from_json(&serde_json::json!({})) == Look::default());
    }

    #[test]
    fn the_phone_accent_is_the_resolved_colour() {
        // Lilás is the first accent of the list, whatever the preset.
        let lilac = Look {
            accent: Some(0),
            custom: None,
            ..Look::default()
        };
        assert_eq!(accent_hex(&lilac), "#bdb4f2");
        // A preset's own accent when none is picked (Tinta: 0x7aa7ff).
        let tinta = Look {
            preset: PRESETS.len() - 1,
            accent: None,
            custom: None,
            ..Look::default()
        };
        assert_eq!(accent_hex(&tinta), "#7aa7ff");
        // A custom colour wins over the picked one and is not a swatch of the list.
        let custom = Look {
            accent: Some(0),
            custom: Some((120., 0.5)),
            ..Look::default()
        };
        let hex = accent_hex(&custom);
        assert_ne!(hex, "#bdb4f2");
        assert_eq!(hex.len(), 7);
        assert!(hex.starts_with('#') && hex[1..].bytes().all(|b| b.is_ascii_hexdigit()));
        // Light or dark, the accent is the same.
        assert_eq!(
            palette(&custom, true).accent,
            palette(&custom, false).accent
        );
    }

    #[test]
    fn ink_on_the_accent_follows_its_luminance() {
        apply(palette(&Look::default(), true));
        assert_eq!(on_accent(), gpui::black());
    }
}

#[cfg(test)]
mod icon_size_tests {
    use super::*;

    #[test]
    fn every_size_lands_on_the_scale() {
        assert_eq!(icon_size(9.), 12.);
        assert_eq!(icon_size(13.), 12.);
        assert_eq!(icon_size(14.), 14.);
        assert_eq!(icon_size(15.), 14.);
        assert_eq!(icon_size(22.), 24.);
        assert_eq!(icon_size(40.), 24.);
    }
}

/// How a button answers a press: it sinks a point and dims. Margins that cancel out move it with
/// no neighbour shifting, since gpui can't scale an element.
pub trait Sinks {
    fn sinks(self) -> Self;
}

impl Sinks for gpui::Stateful<gpui::Div> {
    fn sinks(self) -> Self {
        use gpui::StatefulInteractiveElement as _;
        self.active(|style| style.mt(px(1.)).mb(px(-1.)).opacity(0.78))
    }
}

#[cfg(test)]
mod count_tests {
    use super::format_count;

    #[test]
    fn counts_get_thousands_separators() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234567), "1,234,567");
    }
}
