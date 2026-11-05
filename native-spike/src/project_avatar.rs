//! Project avatars, ported from Sirus Code (ADR-175 v2): a 16x16 rounded block with a spare row
//! under it, drawn pixel by pixel. Port of `src/lib/project-avatar.ts`: the look, the sprite, and
//! the animation poses (blink, glance, the working bob and glance, the waiting notch and happy
//! eyes). The clock is driven by the main app; the pointer gaze of the React avatar is not ported.

use std::f64::consts::PI;

use gpui::{AnyElement, prelude::*};
use serde_json::Value;

/// Sprite width in sprite pixels (`AVATAR_SIZE`).
const SIZE: usize = 16;
/// Sprite rows: the sprite plus the spare bob row (`AVATAR_ROWS`).
const ROWS: usize = 17;
/// The working pose changes every 200 ms (`WORKING_TICK_MS`).
const WORKING_TICK_S: f32 = 0.2;
/// Blink rounds come every 2.25 s; in a round only the avatars of one residue blink (`BLINK_SPREAD`),
/// each at its own phase, and a blink lasts 140 ms (`BLINK_EVERY_MS`, `BLINK_FOR_MS`).
const BLINK_EVERY_S: f32 = 2.25;
const BLINK_FOR_S: f32 = 0.14;
const BLINK_SPREAD: u64 = 3;
/// An idle avatar glances to one side for a moment every 8 s, at its own phase.
const GLANCE_EVERY_S: f32 = 8.0;

const EYES: [&str; 4] = ["bars", "dots", "sleepy", "happy"];
const ACCESSORIES: [&str; 9] = [
    "none", "tophat", "crown", "antenna", "bow", "flower", "beanie", "headset", "scarf",
];
const SWATCHES: [u32; 12] = [
    0x8b7cf6, 0x5b8def, 0x38bdf8, 0x2dd4bf, 0x4ade80, 0xa3c94a, 0xfacc15, 0xfb923c, 0xf87171,
    0xf472b6, 0xc084fc, 0x94a3b8,
];

/// Rounded block, rows 3..13, columns 2..13 (`MASK`).
const MASK: [&str; SIZE] = [
    "................",
    "................",
    "................",
    "....########....",
    "...##########...",
    "..############..",
    "..############..",
    "..############..",
    "..############..",
    "..############..",
    "..############..",
    "..############..",
    "...##########...",
    "....########....",
    "................",
    "................",
];

type Color = [u8; 3];
type Grid = [[Option<Color>; SIZE]; ROWS];

/// What the avatar is doing (`AvatarState`): resting, working on a turn, or waiting for the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarState {
    Idle,
    Working,
    Waiting,
}

/// One frame of the animation (`AvatarPose`): what moves, not how it is drawn.
#[derive(Clone, Copy)]
struct Pose {
    state: AvatarState,
    blink: bool,
    /// -1, 0 or 1: the eyes look left, straight or right.
    gaze: i32,
    /// The working hop: the body rises one row.
    bob: bool,
}

/// The look of a project: colour as a hex value, eyes and accessory as their names.
struct Look {
    color: u32,
    eyes: &'static str,
    accessory: &'static str,
}

/// The ramp roles of one colour (`AvatarRamp`, without the notch pair).
struct Ramp {
    edge: Color,
    side: Color,
    base: Color,
    light: Color,
    shine: Color,
    eye: Color,
    eye_shade: Color,
}

/// The resting avatar: `project_avatar_state` in the idle state at clock 0.
pub fn project_avatar(name: &str, appearance: &Value, side: f32) -> AnyElement {
    project_avatar_state(name, appearance, side, AvatarState::Idle, 0.0)
}

/// Draws the project's avatar at `side` px wide (the sprite is 16 wide, 17 rows with the spare row)
/// in `state`, at `clock` seconds: the blink, glance and bob follow the clock. `appearance` is the
/// project's JSON `appearance` value from the backend, or Null for defaults.
pub fn project_avatar_state(
    name: &str,
    appearance: &Value,
    side: f32,
    state: AvatarState,
    clock: f32,
) -> AnyElement {
    let look = resolve_look(name, appearance);
    let pose = pose_at(hash_name(name), state, clock);
    let cells = sprite_cells(&look, pose);
    let pixel = side / SIZE as f32;
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            for &(x, y, color) in &cells {
                let origin = bounds.origin
                    + gpui::point(gpui::px(x as f32 * pixel), gpui::px(y as f32 * pixel));
                window.paint_quad(gpui::fill(
                    gpui::Bounds::new(origin, gpui::size(gpui::px(pixel), gpui::px(pixel))),
                    color,
                ));
            }
        },
    )
    .w(gpui::px(side))
    .h(gpui::px(side * ROWS as f32 / SIZE as f32))
    .into_any_element()
}

// ---------------------------------------------------------------------------------------------
// Defaults and saved appearance

/// The swatches and accessories offered, and a project's current choices as saved values.
pub fn appearance_choices() -> (
    &'static [u32],
    &'static [&'static str],
    &'static [&'static str],
) {
    (
        &SWATCHES,
        &ACCESSORIES,
        &["bars", "dots", "sleepy", "happy"],
    )
}

pub fn saved_look(name: &str, appearance: &Value) -> Value {
    let look = resolve_look(name, appearance);
    serde_json::json!({ "color": format!("#{:06x}", look.color), "eyes": look.eyes, "accessory": look.accessory })
}

/// FNV-1a over the UTF-16 first unit of each code point, as `hashName` does with `charCodeAt(0)`.
fn hash_name(name: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for ch in name.chars() {
        let mut buf = [0u16; 2];
        let unit = ch.encode_utf16(&mut buf)[0];
        h ^= unit as u32;
        h = h.wrapping_mul(16_777_619);
    }
    h
}

/// `resolveLook`: the saved choices over the name defaults; unknown values fall back to the default.
fn resolve_look(name: &str, appearance: &Value) -> Look {
    let h = hash_name(name);
    let base = Look {
        color: SWATCHES[h as usize % SWATCHES.len()],
        eyes: "bars",
        accessory: ACCESSORIES[(h >> 5) as usize % ACCESSORIES.len()],
    };
    let color = appearance
        .get("color")
        .and_then(Value::as_str)
        .and_then(parse_hex)
        .unwrap_or(base.color);
    let eyes = appearance
        .get("eyes")
        .and_then(Value::as_str)
        .and_then(eyes_from_saved)
        .unwrap_or(base.eyes);
    let accessory = appearance
        .get("accessory")
        .and_then(Value::as_str)
        .and_then(|s| ACCESSORIES.iter().find(|a| **a == s).copied())
        .unwrap_or(base.accessory);
    Look {
        color,
        eyes,
        accessory,
    }
}

/// `/^#[0-9a-f]{6}$/i`, as a colour value.
fn parse_hex(s: &str) -> Option<u32> {
    let digits = s.strip_prefix('#')?;
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// `eyesFromSaved`: a v2 eye name, or a pre-v2 one mapped to its v2 equivalent.
fn eyes_from_saved(value: &str) -> Option<&'static str> {
    if let Some(eyes) = EYES.iter().find(|e| **e == value).copied() {
        return Some(eyes);
    }
    match value {
        "dot" | "round" => Some("dots"),
        "wide" => Some("bars"),
        "sleepy" => Some("sleepy"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Colour: hex -> OKLCH -> role ramp -> sRGB

fn hex_to_oklch(n: u32) -> (f64, f64, f64) {
    let linear = |v: u32| {
        let x = v as f64 / 255.0;
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (
        linear((n >> 16) & 255),
        linear((n >> 8) & 255),
        linear(n & 255),
    );
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let lab_l = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
    let lab_a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
    let lab_b = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
    let h = ((lab_b.atan2(lab_a) * 180.0) / PI + 360.0) % 360.0;
    (lab_l, lab_a.hypot(lab_b), h)
}

fn oklch_to_rgb(l: f64, c: f64, h: f64) -> Color {
    let a = c * ((h * PI) / 180.0).cos();
    let b = c * ((h * PI) / 180.0).sin();
    let l_ = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m_ = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s_ = (l - 0.0894841775 * a - 1.291485548 * b).powi(3);
    let lin = [
        4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
        -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
        -0.0041960863 * l_ - 0.7034186147 * m_ + 1.707614701 * s_,
    ];
    let byte = |v: f64| {
        let x = v.clamp(0.0, 1.0);
        let gamma = if x <= 0.0031308 {
            12.92 * x
        } else {
            1.055 * x.powf(1.0 / 2.4) - 0.055
        };
        (gamma * 255.0).round() as u8
    };
    [byte(lin[0]), byte(lin[1]), byte(lin[2])]
}

/// Moves hue `h` a fraction `amount` of the way to `target` along the short arc.
fn hue_toward(h: f64, target: f64, amount: f64) -> f64 {
    let delta = ((target - h + 540.0) % 360.0) - 180.0;
    (h + delta * amount + 360.0) % 360.0
}

/// Shadows drift cool (towards blue) and lights warm (towards yellow).
fn ramp(hex: u32) -> Ramp {
    let (_, c, h) = hex_to_oklch(hex);
    let chroma = c.clamp(0.05, 0.17);
    let k = |x: f64| (chroma * x).max(0.02);
    Ramp {
        edge: oklch_to_rgb(0.36, k(0.8), hue_toward(h, 265.0, 0.3)),
        side: oklch_to_rgb(0.47, k(0.95), hue_toward(h, 265.0, 0.2)),
        base: oklch_to_rgb(0.64, k(1.0), h),
        light: oklch_to_rgb(0.73, k(0.9), hue_toward(h, 85.0, 0.1)),
        shine: oklch_to_rgb(0.93, k(0.35), hue_toward(h, 85.0, 0.25)),
        eye: oklch_to_rgb(0.99, 0.01, h),
        eye_shade: oklch_to_rgb(0.3, k(0.6), hue_toward(h, 265.0, 0.3)),
    }
}

/// Accessory inks. No pure black: dark parts are graphite with a lit top edge. "E" is the edge colour.
fn ink(name: &str, ramp: &Ramp) -> Color {
    match name {
        "H" => oklch_to_rgb(0.34, 0.015, 285.0),
        "h" => oklch_to_rgb(0.5, 0.015, 285.0),
        "R" => oklch_to_rgb(0.63, 0.2, 25.0),
        "r" => oklch_to_rgb(0.5, 0.18, 25.0),
        "Y" => oklch_to_rgb(0.86, 0.15, 88.0),
        "y" => oklch_to_rgb(0.7, 0.14, 70.0),
        "P" => oklch_to_rgb(0.76, 0.16, 350.0),
        "p" => oklch_to_rgb(0.52, 0.15, 350.0),
        "W" => oklch_to_rgb(0.98, 0.01, 90.0),
        "B" => oklch_to_rgb(0.6, 0.14, 255.0),
        "b" => oklch_to_rgb(0.47, 0.12, 255.0),
        "bl" => oklch_to_rgb(0.7, 0.12, 255.0),
        // "E" is the bubble's edge colour; no other name reaches here.
        _ => ramp.edge,
    }
}

// ---------------------------------------------------------------------------------------------
// Sprite

fn inside(x: i32, y: i32) -> bool {
    y >= 0
        && y < SIZE as i32
        && x >= 0
        && x < SIZE as i32
        && MASK[y as usize].as_bytes()[x as usize] == b'#'
}

/// Accessory rectangles in drawing order: (ink, x, y, w, h).
fn accessory_rects(name: &str) -> &'static [(&'static str, i32, i32, i32, i32)] {
    match name {
        "tophat" => &[
            ("H", 5, 0, 6, 3),
            ("h", 5, 0, 6, 1),
            ("R", 5, 2, 6, 1),
            ("H", 3, 3, 10, 1),
            ("h", 3, 3, 10, 1),
        ],
        "crown" => &[
            ("Y", 4, 0, 1, 1),
            ("Y", 7, 0, 2, 1),
            ("Y", 11, 0, 1, 1),
            ("Y", 4, 1, 8, 2),
            ("y", 4, 3, 8, 1),
            ("R", 7, 1, 2, 1),
            ("W", 5, 1, 1, 1),
        ],
        "antenna" => &[("R", 7, 0, 2, 2), ("W", 7, 0, 1, 1), ("E", 8, 2, 1, 1)],
        "bow" => &[
            ("P", 10, 1, 2, 3),
            ("P", 13, 1, 2, 3),
            ("p", 12, 2, 1, 1),
            ("P", 12, 1, 1, 1),
            ("W", 10, 1, 1, 1),
        ],
        "flower" => &[
            ("W", 12, 0, 1, 1),
            ("W", 11, 1, 1, 1),
            ("W", 13, 1, 1, 1),
            ("W", 12, 2, 1, 1),
            ("Y", 12, 1, 1, 1),
        ],
        "beanie" => &[
            ("B", 4, 1, 8, 2),
            ("b", 3, 3, 10, 1),
            ("W", 7, 0, 2, 1),
            ("bl", 5, 1, 1, 1),
            ("bl", 8, 1, 1, 1),
        ],
        "headset" => &[
            ("H", 3, 1, 10, 1),
            ("h", 3, 1, 10, 1),
            ("H", 2, 2, 1, 3),
            ("H", 13, 2, 1, 3),
            ("H", 0, 6, 2, 4),
            ("h", 0, 6, 2, 1),
            ("H", 14, 6, 2, 4),
            ("h", 14, 6, 2, 1),
            ("H", 1, 10, 1, 2),
            ("H", 2, 11, 3, 1),
            ("R", 5, 11, 1, 1),
        ],
        "scarf" => &[
            ("R", 2, 11, 12, 2),
            ("r", 2, 12, 12, 1),
            ("R", 3, 13, 2, 2),
            ("r", 3, 14, 2, 1),
        ],
        _ => &[],
    }
}

/// Paints a rectangle clipped to the sprite, as `put` does.
fn put(grid: &mut Grid, x: i32, y: i32, color: Color, w: i32, h: i32) {
    for sy in y.max(0)..(y + h).min(ROWS as i32) {
        for sx in x.max(0)..(x + w).min(SIZE as i32) {
            grid[sy as usize][sx as usize] = Some(color);
        }
    }
}

/// The pose at `clock` seconds for a project with name hash `seed` (`workingPose`, and the idle
/// blink and glance of the scheduler, which here are a fixed function of the clock).
fn pose_at(seed: u32, state: AvatarState, clock: f32) -> Pose {
    let clock = clock.max(0.0);
    match state {
        AvatarState::Waiting => Pose {
            state,
            blink: false,
            gaze: 0,
            bob: false,
        },
        AvatarState::Working => {
            let tick = (clock / WORKING_TICK_S) as u64;
            Pose {
                state,
                blink: false,
                gaze: [0, 1, 0, -1][((tick / 4) % 4) as usize],
                bob: (tick / 2) % 2 == 1,
            }
        }
        AvatarState::Idle => {
            let seed = seed as u64;
            let round = (clock / BLINK_EVERY_S).floor() as u64;
            // The phase in the round is fixed per project, so a blink lands in the same place each time.
            let phase = clock - round as f32 * BLINK_EVERY_S;
            let at = ((seed >> 2) % 8) as f32 * 0.15;
            let blink = seed % BLINK_SPREAD == round % BLINK_SPREAD
                && phase >= at
                && phase < at + BLINK_FOR_S;
            let glance = (clock + (seed % 5) as f32 * 1.1) % GLANCE_EVERY_S;
            let gaze = if (5.5..6.6).contains(&glance) {
                if seed % 2 == 0 { 1 } else { -1 }
            } else {
                0
            };
            Pose {
                state,
                blink,
                gaze,
                bob: false,
            }
        }
    }
}

/// Eye pixels for a pose: (x, y, shade). Blinks, sleepy and happy eyes (also the waiting pose) and the look.
fn eye_pixels(eyes: &str, pose: Pose) -> Vec<(i32, i32, bool)> {
    let happy = pose.state == AvatarState::Waiting || eyes == "happy";
    let mut out = Vec::new();
    for ex in [6, 9] {
        let x = ex + pose.gaze;
        if pose.blink || eyes == "sleepy" {
            out.extend([
                (x - 1, 9, false),
                (x, 9, false),
                (x - 1, 10, true),
                (x, 10, true),
            ]);
        } else if happy {
            out.extend([(x - 1, 9, false), (x, 8, false), (x + 1, 9, false)]);
        } else if eyes == "dots" {
            out.extend([
                (x - 1, 8, false),
                (x, 8, false),
                (x - 1, 9, false),
                (x, 9, false),
                (x - 1, 10, true),
                (x, 10, true),
            ]);
        } else {
            out.extend([(x, 7, false), (x, 8, false), (x, 9, false), (x, 10, true)]);
        }
    }
    out
}

/// The sprite in `pose` as (column, row, colour) for every drawn pixel.
fn sprite_cells(look: &Look, pose: Pose) -> Vec<(usize, usize, gpui::Hsla)> {
    let ramp = ramp(look.color);
    let mut grid: Grid = [[None; SIZE]; ROWS];
    // The body rests one row down; the working hop lifts it.
    let top: i32 = if pose.state == AvatarState::Working && pose.bob {
        0
    } else {
        1
    };

    for y in 0..SIZE as i32 {
        for x in 0..SIZE as i32 {
            if !inside(x, y) {
                continue;
            }
            if !inside(x - 1, y) || !inside(x, y - 1) || !inside(x + 1, y) || !inside(x, y + 1) {
                put(&mut grid, x, y + top, ramp.edge, 1, 1);
                continue;
            }
            // Extruded right side (2 px) and bottom (1 px).
            if (4..=13).contains(&y) && (!inside(x + 3, y) || !inside(x, y + 2)) {
                put(&mut grid, x, y + top, ramp.side, 1, 1);
                continue;
            }
            // Light from the top left: a solid lit corner, then a checker that fades into the base.
            let d = x - 2 + (y - 3);
            let color = if d < 4 || (d < 9 && (x + y) % 2 == 0) {
                ramp.light
            } else {
                ramp.base
            };
            put(&mut grid, x, y + top, color, 1, 1);
        }
    }

    // Catch-light.
    put(&mut grid, 4, 5 + top, ramp.shine, 1, 1);
    put(&mut grid, 5, 5 + top, ramp.shine, 1, 1);
    put(&mut grid, 4, 6 + top, ramp.shine, 1, 1);

    for (x, y, shade) in eye_pixels(look.eyes, pose) {
        put(
            &mut grid,
            x,
            y + top,
            if shade { ramp.eye_shade } else { ramp.eye },
            1,
            1,
        );
    }
    for &(name, x, y, w, h) in accessory_rects(look.accessory) {
        put(&mut grid, x, y + top, ink(name, &ramp), w, h);
    }
    // The waiting notch in the top right corner.
    if pose.state == AvatarState::Waiting {
        put(&mut grid, 12, 0, oklch_to_rgb(0.3, 0.08, 288.0), 4, 4);
        put(&mut grid, 13, 0, oklch_to_rgb(0.84, 0.12, 288.0), 3, 3);
    }

    let mut cells = Vec::new();
    for (y, row) in grid.iter().enumerate() {
        for (x, cell) in row.iter().enumerate() {
            if let Some(c) = cell {
                let hex = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32;
                cells.push((x, y, gpui::Hsla::from(gpui::rgb(hex))));
            }
        }
    }
    cells
}
