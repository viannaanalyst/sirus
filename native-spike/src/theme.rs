//! Monocode's dark tokens (theme.rs): translucent ink over a blurred window.
use gpui::{AnyElement, Hsla, Rgba, Svg, canvas, hsla, prelude::*, px, rgb, svg};
use std::cell::Cell;
pub const TITLE_BAR: f32 = 40.;
pub const FOOTER: f32 = 30.;
pub const PLATE: f32 = 27.;
pub const TAB_PLATE_HEIGHT: f32 = PLATE;
pub fn content(alpha: f32) -> Hsla {
    let mut color = current().ink;
    color.a = alpha;
    color
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

/// Everything Aparência sets, as Monocode's appearance page: theme, workspace theme, accent,
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
}

impl Default for Look {
    /// The owner's Monocode: Ink, the pink accent, following the system, with glass.
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
        }
    }
}

/// A workspace theme: a tint for the chrome and surfaces, and its accent.
pub struct Preset {
    pub name: &'static str,
    hue: f32,
    saturation: f32,
    pub accent: u32,
    /// The dark chrome's lightness (Monocode's `theme_dark_lightness`) and its ink.
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

/// Monocode's `sidebar_opacity` as the owner has it: how much of the desktop the tint hides.
const SHELL_ALPHA: f32 = 0.77;

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
    // Dark glass as Monocode: the window's tint is the only coat, and the conversation's card
    // adds none, so the desktop shows through it instead of being hidden twice.
    let pane_alpha = match (look.glass, dark) {
        (true, true) => 0.,
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
            pane: tint(saturation, p.dark_lightness * 0.45, pane_alpha),
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
/// Black or white on the accent, by its relative luminance (Monocode's `primary_action`).
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
/// The composer: ink at 3% over a 24 px frost (Monocode's `[data-composer-box]`).
pub fn composer_surface(radius: f32) -> impl IntoElement {
    frosted(radius, content(0.03), field_fill(), 24.)
}
pub fn rail_color() -> Hsla {
    let (base, ink) = (pane_opaque().to_rgb(), current().ink.to_rgb());
    let mix = |b: f32, i: f32| b + (i - b) * 0.14;
    Rgba {
        r: mix(base.r, ink.r),
        g: mix(base.g, ink.g),
        b: mix(base.b, ink.b),
        a: 1.,
    }
    .into()
}
pub fn icon(path: &'static str, side: f32) -> Svg {
    svg()
        .path(path)
        .size(px(side))
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
        .text_color(gpui::white())
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
        assert_eq!(dark.pane.a, 0.);
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
    fn ink_on_the_accent_follows_its_luminance() {
        apply(palette(&Look::default(), true));
        assert_eq!(on_accent(), gpui::black());
    }
}
