//! Project avatars: a glossy little head with a dark visor and two eyes, one of eight shapes in the
//! project's colour. The body is an SVG drawn once per shape, colour and size; the eyes are painted
//! on it, so a blink or a glance costs nothing. Saved as `appearance` (`color`, `eyes`, and the
//! shape in `accessory`); a value saved by the older pixel avatars falls back to the name's default.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::Arc;

use gpui::{AnyElement, Hsla, Image, ImageFormat, PathBuilder, point, prelude::*, px};
use serde_json::Value;

/// A working avatar's hop changes every 200 ms.
const WORKING_TICK_S: f32 = 0.2;
/// Blink rounds come every 2.25 s; in a round only the avatars of one residue blink, each at its
/// own phase, and a blink lasts 140 ms.
const BLINK_EVERY_S: f32 = 2.25;
const BLINK_FOR_S: f32 = 0.14;
const BLINK_SPREAD: u64 = 3;
/// An idle avatar glances to one side for a moment every 8 s, at its own phase.
const GLANCE_EVERY_S: f32 = 8.0;

const EYES: [&str; 4] = ["bars", "dots", "sleepy", "happy"];
const SHAPES: [&str; 8] = [
    "round", "square", "cloud", "ghost", "hex", "capsule", "tv", "bean",
];
const SWATCHES: [u32; 12] = [
    0x8b7cf6, 0x5b8def, 0x38bdf8, 0x2dd4bf, 0x4ade80, 0xa3c94a, 0xfacc15, 0xfb923c, 0xf87171,
    0xf472b6, 0xc084fc, 0x94a3b8,
];

/// What the avatar is doing: resting, working on a turn, or waiting for the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarState {
    Idle,
    Working,
    Waiting,
}

/// One frame of the animation: what moves, not how it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    happy: bool,
    blink: bool,
    /// -1, 0 or 1: the eyes look left, straight or right.
    gaze: i32,
    /// The working hop: the body rises a little.
    bob: bool,
}

/// The look of a project: colour as a hex value, eyes and shape as their names.
struct Look {
    color: u32,
    eyes: &'static str,
    shape: &'static str,
}

/// The resting avatar.
pub fn project_avatar(name: &str, appearance: &Value, side: f32) -> AnyElement {
    project_avatar_state(name, appearance, side, AvatarState::Idle, 0.0)
}

/// Draws the project's avatar `side` px wide and tall, in `state`, at `clock` seconds.
pub fn project_avatar_state(
    name: &str,
    appearance: &Value,
    side: f32,
    state: AvatarState,
    clock: f32,
) -> AnyElement {
    let look = resolve_look(name, appearance);
    let pose = pose_at(hash_name(name), state, clock);
    let body = body_image(look.shape, look.color, side);
    let eyes = look.eyes;
    gpui::div()
        .relative()
        .flex_shrink_0()
        .size(px(side))
        .top(px(if pose.bob { -side * 0.04 } else { 0. }))
        .child(gpui::img(body).absolute().top_0().left_0().size_full())
        .child(
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let unit = f32::from(bounds.size.width) / 64.;
                    paint_eyes(window, bounds.origin, unit, eyes, pose);
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .into_any_element()
}

/// The eyes on the visor, in the avatar's 64-unit square.
fn paint_eyes(
    window: &mut gpui::Window,
    origin: gpui::Point<gpui::Pixels>,
    unit: f32,
    eyes: &str,
    pose: Pose,
) {
    let ink: Hsla = gpui::rgb(0xf4f5f8).into();
    let shift = pose.gaze as f32 * 1.6;
    let at = |x: f32, y: f32| origin + point(px((x + shift) * unit), px(y * unit));
    let bar = |window: &mut gpui::Window, cx: f32, cy: f32, w: f32, h: f32| {
        let bounds = gpui::Bounds::new(
            at(cx - w / 2., cy - h / 2.),
            gpui::size(px(w * unit), px(h * unit)),
        );
        window.paint_quad(gpui::fill(bounds, ink).corner_radii(px(w.min(h) * unit / 2.)));
    };
    for cx in [24.5, 39.5] {
        if pose.blink || eyes == "sleepy" {
            bar(window, cx, 32., 9., 3.2);
        } else if pose.happy || eyes == "happy" {
            let mut path = PathBuilder::stroke(px(3.2 * unit));
            path.move_to(at(cx - 5., 34.));
            path.curve_to(at(cx + 5., 34.), at(cx, 27.));
            if let Ok(path) = path.build() {
                window.paint_path(path, ink);
            }
        } else if eyes == "dots" {
            bar(window, cx, 32., 8.4, 8.4);
        } else {
            bar(window, cx, 32., 7.6, 11.5);
        }
    }
}

thread_local! {
    static BODIES: RefCell<HashMap<(&'static str, u32, u32), Arc<Image>>> = RefCell::new(HashMap::new());
}

/// The head and visor as an image at twice `side`, for the Retina screens the app runs on.
fn body_image(shape: &'static str, color: u32, side: f32) -> Arc<Image> {
    let pixels = (side * 2.).ceil() as u32;
    BODIES.with(|cache| {
        cache
            .borrow_mut()
            .entry((shape, color, pixels))
            .or_insert_with(|| {
                Arc::new(Image::from_bytes(
                    ImageFormat::Svg,
                    body_svg(shape, color, pixels).into_bytes(),
                ))
            })
            .clone()
    })
}

fn shape_path(shape: &str) -> &'static str {
    match shape {
        "square" => r#"<rect x="7" y="7" width="50" height="50" rx="16"/>"#,
        "cloud" => {
            r#"<path d="M20 50a13 13 0 0 1-2.6-25.7A16 16 0 0 1 47 21.5 14.5 14.5 0 0 1 45 50z"/>"#
        }
        "ghost" => {
            r#"<path d="M10 30a22 22 0 0 1 44 0v22c0 3-3 4-5 2l-4-3-4 3a4 4 0 0 1-5 0l-4-3-4 3a4 4 0 0 1-5 0l-4-3-4 3c-2 2-5 1-5-2z"/>"#
        }
        "hex" => {
            r#"<path d="M28 6.5a8 8 0 0 1 8 0l16 9.3a8 8 0 0 1 4 7V41a8 8 0 0 1-4 7l-16 9.3a8 8 0 0 1-8 0L12 48a8 8 0 0 1-4-7V22.8a8 8 0 0 1 4-7z"/>"#
        }
        "capsule" => r#"<rect x="13" y="5" width="38" height="54" rx="19"/>"#,
        "tv" => r#"<rect x="6" y="15" width="52" height="42" rx="13"/>"#,
        "bean" => {
            r#"<path d="M32 6c15 0 26 10 26 25 0 8-3 12-6 15-4 4-10 12-22 12C16 58 6 48 6 33 6 17 17 6 32 6z"/>"#
        }
        _ => r#"<circle cx="32" cy="32" r="26"/>"#,
    }
}

fn hex_of(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

fn body_svg(shape: &str, color: u32, pixels: u32) -> String {
    let (l, c, h) = hex_to_oklch(color);
    let top = hex_of(oklch_to_rgb((l + 0.09).min(0.95), c, h));
    let bottom = hex_of(oklch_to_rgb((l - 0.11).max(0.3), c, h));
    let path = shape_path(shape);
    let antennas = if shape == "tv" {
        format!(
            r#"<path d="M23 15 16 6M41 15l7-9" fill="none" stroke="{bottom}" stroke-width="3.5" stroke-linecap="round"/>"#
        )
    } else {
        String::new()
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="{pixels}" height="{pixels}"><defs><linearGradient id="b" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{top}"/><stop offset="1" stop-color="{bottom}"/></linearGradient><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ffffff" stop-opacity="0.45"/><stop offset="0.55" stop-color="#ffffff" stop-opacity="0"/></linearGradient></defs>{antennas}<g fill="url(#b)">{path}</g><g fill="url(#s)" opacity="0.55">{path}</g><rect x="13" y="22" width="38" height="20" rx="10" fill="#14151b"/></svg>"##
    )
}

/// Whether a point of the 64-unit square falls inside a shape, for the new tab's field of letters.
pub fn shape_contains(shape: &str, x: f32, y: f32) -> bool {
    let circle = |cx: f32, cy: f32, r: f32| (x - cx).powi(2) + (y - cy).powi(2) <= r * r;
    let rounded = |left: f32, top: f32, right: f32, bottom: f32, r: f32| {
        let dx = (left + r - x).max(x - (right - r)).max(0.);
        let dy = (top + r - y).max(y - (bottom - r)).max(0.);
        x >= left && x <= right && y >= top && y <= bottom && dx * dx + dy * dy <= r * r
    };
    match shape {
        "square" => rounded(7., 7., 57., 57., 16.),
        "cloud" => {
            circle(20., 37., 13.)
                || circle(32., 26., 16.)
                || circle(45., 35.5, 14.5)
                || rounded(20., 30., 45., 50., 0.)
        }
        "ghost" => circle(32., 30., 22.) || rounded(10., 30., 54., 54., 2.),
        "hex" => (x - 32.).abs() <= 24. && (y - 32.).abs() + (x - 32.).abs() * 0.55 <= 27.,
        "capsule" => rounded(13., 5., 51., 59., 19.),
        "tv" => rounded(6., 15., 58., 57., 13.),
        "bean" => circle(32., 32., 26.),
        _ => circle(32., 32., 26.),
    }
}

/// A project's shape and colour, for what is drawn in its colour around the avatar.
pub fn look_of(name: &str, appearance: &Value) -> (&'static str, u32) {
    let look = resolve_look(name, appearance);
    (look.shape, look.color)
}

/// The colours, shapes and eyes offered in the project's menu.
pub fn appearance_choices() -> (
    &'static [u32],
    &'static [&'static str],
    &'static [&'static str],
) {
    (&SWATCHES, &SHAPES, &EYES)
}

pub fn saved_look(name: &str, appearance: &Value) -> Value {
    let look = resolve_look(name, appearance);
    serde_json::json!({ "color": format!("#{:06x}", look.color), "eyes": look.eyes, "accessory": look.shape })
}

/// FNV-1a over the UTF-16 first unit of each code point.
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

/// The saved choices over the name's defaults; unknown values fall back to them.
fn resolve_look(name: &str, appearance: &Value) -> Look {
    let h = hash_name(name);
    let color = appearance
        .get("color")
        .and_then(Value::as_str)
        .and_then(parse_hex)
        .unwrap_or(SWATCHES[h as usize % SWATCHES.len()]);
    let eyes = appearance
        .get("eyes")
        .and_then(Value::as_str)
        .and_then(eyes_from_saved)
        .unwrap_or("bars");
    let shape = appearance
        .get("accessory")
        .and_then(Value::as_str)
        .and_then(|s| SHAPES.iter().find(|a| **a == s).copied())
        .unwrap_or(SHAPES[(h >> 5) as usize % SHAPES.len()]);
    Look { color, eyes, shape }
}

/// `/^#[0-9a-f]{6}$/i`, as a colour value.
fn parse_hex(s: &str) -> Option<u32> {
    let digits = s.strip_prefix('#')?;
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

fn eyes_from_saved(value: &str) -> Option<&'static str> {
    if let Some(eyes) = EYES.iter().find(|e| **e == value).copied() {
        return Some(eyes);
    }
    match value {
        "dot" | "round" => Some("dots"),
        "wide" => Some("bars"),
        _ => None,
    }
}

// -----------------------------------------------------------------------------
// Colour

type Color = [u8; 3];

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
/// The frame at `clock`: a blink, a glance or a hop, each at the project's own phase.
fn pose_at(seed: u32, state: AvatarState, clock: f32) -> Pose {
    let seed = seed as u64;
    let round = (clock / BLINK_EVERY_S).floor();
    let phase = (seed % 97) as f32 / 97. * (BLINK_EVERY_S - BLINK_FOR_S);
    let in_round = clock - round * BLINK_EVERY_S;
    let blink = (round as u64 + seed) % BLINK_SPREAD == 0
        && in_round >= phase
        && in_round < phase + BLINK_FOR_S;
    match state {
        AvatarState::Waiting => Pose {
            happy: true,
            blink: false,
            gaze: 0,
            bob: false,
        },
        AvatarState::Working => {
            let tick = (clock / WORKING_TICK_S).floor() as i64;
            Pose {
                happy: false,
                blink,
                gaze: [-1, 0, 1, 0][(tick.rem_euclid(16) / 4) as usize],
                bob: tick.rem_euclid(2) == 0,
            }
        }
        AvatarState::Idle => {
            let at = (clock + (seed % 800) as f32 / 100.) % GLANCE_EVERY_S;
            Pose {
                happy: false,
                blink,
                gaze: if at < 0.6 {
                    if seed % 2 == 0 { -1 } else { 1 }
                } else {
                    0
                },
                bob: false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_saved_look_wins_and_an_old_accessory_falls_back_to_a_shape() {
        let look = resolve_look(
            "p",
            &json!({"color":"#112233","eyes":"happy","accessory":"cloud"}),
        );
        assert_eq!(
            (look.color, look.eyes, look.shape),
            (0x112233, "happy", "cloud")
        );
        let old = resolve_look("p", &json!({"accessory":"crown"}));
        assert!(SHAPES.contains(&old.shape));
    }

    #[test]
    fn the_same_name_keeps_its_default_look() {
        let (a, b) = (
            resolve_look("site", &Value::Null),
            resolve_look("site", &Value::Null),
        );
        assert_eq!((a.color, a.shape), (b.color, b.shape));
    }

    #[test]
    fn every_shape_contains_its_middle_and_not_the_corner() {
        for shape in SHAPES {
            assert!(shape_contains(shape, 32., 32.), "{shape}");
            assert!(!shape_contains(shape, 0.5, 0.5), "{shape}");
        }
    }

    #[test]
    fn a_body_is_a_complete_svg_in_the_project_colour_family() {
        let svg = body_svg("tv", 0x5b8def, 112);
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(r#"width="112""#) && svg.contains("stroke-linecap"));
    }

    #[test]
    fn a_waiting_avatar_smiles_and_a_working_one_moves() {
        assert!(pose_at(1, AvatarState::Waiting, 3.).happy);
        let frames: Vec<bool> = (0..4)
            .map(|i| pose_at(1, AvatarState::Working, i as f32 * WORKING_TICK_S).bob)
            .collect();
        assert!(frames.contains(&true) && frames.contains(&false));
    }
}
