//! Monocode's dark tokens (theme.rs): translucent ink over a blurred window.
use gpui::{AnyElement, Hsla, Rgba, Svg, hsla, prelude::*, px, rgb, svg};
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
    fn default() -> Self {
        Self {
            preset: 0,
            accent: None,
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
}

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "Sirus",
        hue: 255.,
        saturation: 0.10,
        accent: 0xbdb4f2,
    },
    Preset {
        name: "Carvão",
        hue: 0.,
        saturation: 0.,
        accent: 0xe6e6e6,
    },
    Preset {
        name: "Oceano",
        hue: 212.,
        saturation: 0.28,
        accent: 0x6fb3f2,
    },
    Preset {
        name: "Floresta",
        hue: 150.,
        saturation: 0.20,
        accent: 0x74c69d,
    },
    Preset {
        name: "Brasa",
        hue: 18.,
        saturation: 0.24,
        accent: 0xf0925e,
    },
    Preset {
        name: "Ameixa",
        hue: 300.,
        saturation: 0.20,
        accent: 0xd08bd9,
    },
    Preset {
        name: "Areia",
        hue: 42.,
        saturation: 0.18,
        accent: 0xdcbf7a,
    },
    Preset {
        name: "Coral",
        hue: 350.,
        saturation: 0.20,
        accent: 0xee8a96,
    },
    Preset {
        name: "Aurora",
        hue: 176.,
        saturation: 0.24,
        accent: 0x5fd4c4,
    },
    Preset {
        name: "Neon",
        hue: 285.,
        saturation: 0.42,
        accent: 0xe45fd8,
    },
];

pub const ACCENTS: &[(&str, u32)] = &[
    ("Lilás", 0xbdb4f2),
    ("Branco", 0xf2f2f2),
    ("Azul", 0x74a4f7),
    ("Rosa", 0xd964ae),
    ("Coral", 0xe0654d),
    ("Âmbar", 0xe0a526),
    ("Verde", 0x62b67f),
    ("Ciano", 0x4fc3cf),
];

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
    let pane_alpha = if look.glass {
        if dark { 0.95 } else { 0.9 }
    } else {
        1.
    };
    let accent: Hsla = match look.custom {
        Some((hue, light)) => hsla(hue / 360., 0.75, light, 1.),
        None => rgb(look.accent.map_or(p.accent, |a| ACCENTS[a].1)).into(),
    };
    if dark {
        Palette {
            shell: tint(saturation * 0.7, 0.06, if look.glass { 0.88 } else { 1. }),
            pane: tint(saturation, 0.03, pane_alpha),
            field: tint(saturation * 0.8, 0.135, 1.),
            floating: tint(saturation * 0.7, 0.14, 1.),
            accent,
            ink: rgb(0xebebeb).into(),
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
/// Dark ink on a light accent, white on a dark one.
pub fn on_accent() -> Hsla {
    if accent().l > 0.62 {
        rgb(0x1d1c24).into()
    } else {
        gpui::white()
    }
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
