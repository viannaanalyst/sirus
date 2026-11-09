//! The agent's pointer (ADR-198): when an agent clicks, types or scrolls in the browser panel or
//! in another Mac app, an arrow in the provider's colour glides to the target with a small pill
//! naming it, leaves a short fading trail, rings each click and hides a few seconds after the
//! last action. Never takes clicks.
//!
//! Two surfaces draw it: the browser page, as an overlay injected by `browser_js/pointer.js`
//! (its timings, colour and mark come from `page_config` here), and the click-through window over
//! the screen for computer use (`pointer_overlay.rs`), which paints a `Pointer` with `element`.
//! The timing model, colours and coordinate mapping below are shared and pure.
use gpui::{
    AnyElement, Bounds, Hsla, PathBuilder, Pixels, Window, canvas, div, fill, point, prelude::*,
    px, size,
};
use serde_json::{Value, json};

/// The glide to a new target, in seconds, eased with `EASE` (cubic-bezier(.3, .7, .2, 1)).
pub const GLIDE: f32 = 0.6;
pub const EASE: [f32; 4] = [0.3, 0.7, 0.2, 1.];
/// A click's ring: from `RIPPLE_FROM` to `RIPPLE_TO` px across, fading out in `RIPPLE` seconds.
pub const RIPPLE: f32 = 0.6;
pub const RIPPLE_FROM: f32 = 10.;
pub const RIPPLE_TO: f32 = 46.;
/// The trail: this many dots, each this many seconds behind the last, gone this long after landing.
pub const TRAIL_DOTS: usize = 6;
pub const TRAIL_GAP: f32 = 0.04;
pub const TRAIL_FADE: f32 = 0.3;
/// The pointer hides this long after the last action, fading out in `FADE`.
pub const HIDE_AFTER: f32 = 3.;
/// With Reduce motion: no glide and no trail, the arrow fades in where it acts.
pub const FADE: f32 = 0.18;

/// What the agent did at the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Click,
    Type,
    Key,
    Scroll { down: bool },
}

impl Kind {
    /// The host's `kind` field (`computer-pointer`) or a browser tool's name.
    pub fn parse(kind: &str, down: bool) -> Option<Self> {
        Some(match kind {
            "click" | "drag" | "browser_click" | "browser_drag" => Self::Click,
            "type" | "browser_type" => Self::Type,
            "key" => Self::Key,
            "scroll" | "browser_scroll" => Self::Scroll { down },
            _ => return None,
        })
    }
}

/// The arrow's colour: Claude's coral, Codex's near-white, Gemini's blue; `None` takes the accent.
pub fn color(provider: &str) -> Option<u32> {
    match provider {
        "claude" => Some(0xd97757),
        "codex" => Some(0xe5e5e5),
        "antigravity" | "gemini" => Some(0x8fb4f5),
        _ => None,
    }
}

/// The pill's name, short as the user reads it ("Claude", not "Claude Code").
pub fn name(provider: &str) -> &'static str {
    match provider {
        "claude" => "Claude",
        "codex" => "Codex",
        "gemini" => "Gemini",
        "antigravity" => "Antigravity",
        "cursor" => "Cursor",
        "grok" => "Grok",
        "opencode" => "OpenCode",
        "droid" => "Droid",
        "pi" => "Pi",
        "devin" => "Devin",
        "hermes" => "Hermes",
        _ => "Agent",
    }
}

/// `#rrggbb` of a colour.
pub fn hex(rgb: u32) -> String {
    format!("#{:06x}", rgb & 0xff_ffff)
}

/// The accent as `0xrrggbb`.
pub fn rgb_of(color: Hsla) -> u32 {
    let c = color.to_rgb();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    (byte(c.r) << 16) | (byte(c.g) << 8) | byte(c.b)
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    if t == 0. || t == 1. {
        return t;
    }
    crate::motion::bezier(t, EASE[0], EASE[1], EASE[2], EASE[3])
}

/// A click's ring `elapsed` seconds after it started: its diameter and opacity, or none once gone.
pub fn ripple(elapsed: f32, reduced: bool) -> Option<(f32, f32)> {
    if !(0. ..RIPPLE).contains(&elapsed) {
        return None;
    }
    let t = elapsed / RIPPLE;
    // Reduce motion keeps the ring still at its middle size and only fades it.
    let diameter = if reduced {
        (RIPPLE_FROM + RIPPLE_TO) / 2.
    } else {
        RIPPLE_FROM + (RIPPLE_TO - RIPPLE_FROM) * (1. - (1. - t).powi(3))
    };
    Some((diameter, 1. - t))
}

/// The pointer on one surface. Times are seconds from any fixed start.
#[derive(Clone, Debug)]
pub struct Pointer {
    pub provider: String,
    from: (f32, f32),
    to: (f32, f32),
    moved_at: f32,
    last_action: f32,
    shown_at: f32,
    kind: Kind,
    ripple_at: Option<f32>,
    reduced: bool,
}

/// What to draw at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub at: (f32, f32),
    /// Behind the arrow, oldest last: position and opacity.
    pub trail: Vec<(f32, f32, f32)>,
    /// Centred on the target: diameter and opacity.
    pub ripple: Option<(f32, f32)>,
    pub scroll: Option<bool>,
    pub opacity: f32,
}

impl Pointer {
    /// A pointer that appears at `at`, acting there.
    pub fn new(provider: &str, at: (f32, f32), kind: Kind, now: f32, reduced: bool) -> Self {
        Self {
            provider: provider.to_owned(),
            from: at,
            to: at,
            moved_at: now,
            last_action: now,
            shown_at: now,
            kind,
            ripple_at: (kind == Kind::Click).then_some(now),
            reduced,
        }
    }

    /// The next action: glide from where the arrow is now to `target` (or stay, for a key press).
    pub fn aim(&mut self, provider: &str, target: Option<(f32, f32)>, kind: Kind, now: f32) {
        if !self.visible(now) {
            *self = Self::new(provider, target.unwrap_or(self.to), kind, now, self.reduced);
            return;
        }
        let here = self.position(now);
        let to = target.unwrap_or(self.to);
        let moves = (to.0 - here.0).hypot(to.1 - here.1) > 0.5;
        self.provider = provider.to_owned();
        self.from = here;
        self.to = to;
        if moves {
            self.moved_at = now;
        }
        self.last_action = now;
        self.kind = kind;
        if kind == Kind::Click {
            // The ring starts as the arrow lands.
            let landing = if moves && !self.reduced { GLIDE } else { 0. };
            self.ripple_at = Some(now + landing);
        }
    }

    pub fn set_reduced(&mut self, reduced: bool) {
        self.reduced = reduced;
    }

    /// The arrow's tip at `now`.
    pub fn position(&self, now: f32) -> (f32, f32) {
        if self.reduced {
            return self.to;
        }
        let t = ease((now - self.moved_at) / GLIDE);
        (
            self.from.0 + (self.to.0 - self.from.0) * t,
            self.from.1 + (self.to.1 - self.from.1) * t,
        )
    }

    /// Shown until `HIDE_AFTER` after the last action, then fading out.
    pub fn visible(&self, now: f32) -> bool {
        now - self.last_action < HIDE_AFTER + FADE
    }

    /// Something still moves: the glide, the ring, the trail or a fade.
    pub fn animating(&self, now: f32) -> bool {
        let glide_end = self.moved_at
            + if self.reduced {
                FADE
            } else {
                GLIDE + TRAIL_FADE
            };
        let ring_end = self.ripple_at.map_or(0., |at| at + RIPPLE);
        let hiding = now - self.last_action >= HIDE_AFTER;
        self.visible(now)
            && (now < glide_end || now < ring_end || now < self.shown_at + FADE || hiding)
    }

    /// Seconds until the pointer is fully gone.
    pub fn remaining(&self, now: f32) -> f32 {
        (self.last_action + HIDE_AFTER + FADE - now).max(0.)
    }

    pub fn frame(&self, now: f32) -> Frame {
        let appear = ((now - self.shown_at) / FADE).clamp(0., 1.);
        let hide = 1. - ((now - self.last_action - HIDE_AFTER) / FADE).clamp(0., 1.);
        // Reduce motion: each move is a short fade in at the new place instead of a glide.
        let moved = if self.reduced {
            ((now - self.moved_at) / FADE).clamp(0., 1.)
        } else {
            1.
        };
        Frame {
            at: self.position(now),
            trail: self.trail(now),
            ripple: self
                .ripple_at
                .and_then(|at| ripple(now - at, self.reduced))
                .map(|(d, a)| (d, a * hide)),
            scroll: match self.kind {
                Kind::Scroll { down } => Some(down),
                _ => None,
            },
            opacity: appear.min(hide).min(moved),
        }
    }

    fn trail(&self, now: f32) -> Vec<(f32, f32, f32)> {
        if self.reduced || self.from == self.to {
            return Vec::new();
        }
        let landed = now - (self.moved_at + GLIDE);
        let fade = 1. - (landed / TRAIL_FADE).clamp(0., 1.);
        if fade <= 0. {
            return Vec::new();
        }
        (1..=TRAIL_DOTS)
            .filter_map(|k| {
                let at = now - k as f32 * TRAIL_GAP;
                if at < self.moved_at {
                    return None;
                }
                let (x, y) = self.position(at);
                let alpha = 0.55 * (1. - k as f32 / (TRAIL_DOTS + 1) as f32) * fade;
                Some((x, y, alpha))
            })
            .collect()
    }
}

// ---- Coordinates ----

/// A screen rectangle in global display points (top-left of the main display is 0, 0; y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2., self.y + self.height / 2.)
    }
}

/// The screen a global point is on, or the nearest one to it.
pub fn screen_for(point: (f64, f64), screens: &[Rect]) -> Option<usize> {
    screens.iter().position(|s| s.contains(point)).or_else(|| {
        let distance = |s: &Rect| {
            let (cx, cy) = s.center();
            (cx - point.0).hypot(cy - point.1)
        };
        (0..screens.len()).min_by(|a, b| distance(&screens[*a]).total_cmp(&distance(&screens[*b])))
    })
}

/// A global point inside the overlay window that covers `screen`.
pub fn local_point(point: (f64, f64), screen: Rect) -> (f32, f32) {
    ((point.0 - screen.x) as f32, (point.1 - screen.y) as f32)
}

/// The size of one screen point in the page's CSS pixels, so the arrow keeps its size whatever
/// the zoom: the zoom applied to the web view (person's zoom times the device frame's fit), or the
/// person's zoom while the panel isn't painting the page.
pub fn page_scale(applied_zoom: f64, user_zoom: f64) -> f64 {
    let zoom = if applied_zoom.is_finite() && applied_zoom > 0. {
        applied_zoom
    } else {
        user_zoom
    };
    if zoom.is_finite() && zoom > 0. {
        1. / zoom
    } else {
        1.
    }
}

// ---- The page overlay ----

/// A provider mark as plain path data, for a page that may forbid markup (Trusted Types): its
/// view box, the `d` of each path, and whether they fill even-odd. `None` for marks drawn with
/// masks or groups, which then show as a dot.
pub fn mark_paths(svg: &str) -> Option<(String, Vec<String>, bool)> {
    if svg.contains("<g") || svg.contains("mask") || svg.contains("<ellipse") {
        return None;
    }
    let attr = |tag: &str, name: &str| -> Option<String> {
        let key = format!("{name}=\"");
        let start = tag.find(&key)? + key.len();
        Some(tag[start..].split('"').next()?.to_owned())
    };
    let head = &svg[svg.find("<svg")?..];
    let view_box = attr(&head[..head.find('>')?], "viewBox")?;
    let mut paths = Vec::new();
    let mut rest = svg;
    while let Some(at) = rest.find("<path") {
        let tag = &rest[at..];
        let end = tag.find('>')?;
        paths.push(attr(&tag[..end], "d")?);
        rest = &tag[end..];
    }
    let even_odd = svg.contains("evenodd");
    (!paths.is_empty()).then_some((view_box, paths, even_odd))
}

fn mark_svg(provider: &str) -> &'static str {
    match provider {
        "claude" => include_str!("../assets/claude.svg"),
        "codex" => include_str!("../assets/codex-glyph.svg"),
        "grok" => include_str!("../assets/grok.svg"),
        "droid" => include_str!("../assets/droid.svg"),
        "opencode" => include_str!("../assets/opencode.svg"),
        _ => "",
    }
}

/// What `browser_js/pointer.js` draws with, passed in the tool's arguments as `sirusPointer`.
pub fn page_config(provider: &str, accent: u32, reduced: bool, scale: f64) -> Value {
    let color = hex(color(provider).unwrap_or(accent));
    let mark = mark_paths(mark_svg(provider))
        .map(|(view_box, paths, even_odd)| json!({ "viewBox": view_box, "paths": paths, "evenOdd": even_odd }));
    json!({
        "provider": provider,
        "color": color,
        "label": name(provider),
        "mark": mark,
        "glideMs": GLIDE * 1000.,
        "ease": EASE,
        "rippleMs": RIPPLE * 1000.,
        "rippleFrom": RIPPLE_FROM,
        "rippleTo": RIPPLE_TO,
        "trailDots": TRAIL_DOTS,
        "trailGapMs": TRAIL_GAP * 1000.,
        "trailFadeMs": TRAIL_FADE * 1000.,
        "hideMs": HIDE_AFTER * 1000.,
        "fadeMs": FADE * 1000.,
        "reduced": reduced,
        "scale": scale,
    })
}

// ---- Drawing in GPUI ----

/// The arrow's outline, tip at 0, 0, in points.
pub const ARROW: [(f32, f32); 7] = [
    (0., 0.),
    (0., 17.),
    (4.6, 13.2),
    (7.6, 20.),
    (10.4, 18.8),
    (7.5, 12.2),
    (13., 12.2),
];

fn arrow_path(
    at: (f32, f32),
    scale: f32,
    grow: f32,
    stroke: Option<f32>,
) -> Option<gpui::Path<Pixels>> {
    let mut path = match stroke {
        Some(width) => PathBuilder::stroke(px(width)),
        None => PathBuilder::fill(),
    };
    for (index, (x, y)) in ARROW.iter().enumerate() {
        let p = point(px(at.0 + (x * scale) * grow), px(at.1 + (y * scale) * grow));
        if index == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    path.close();
    path.build().ok()
}

fn paint_frame(bounds: Bounds<Pixels>, frame: &Frame, color: Hsla, window: &mut Window) {
    let origin = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let with = |alpha: f32| Hsla {
        a: color.a * alpha * frame.opacity,
        ..color
    };
    for &(x, y, alpha) in frame.trail.iter().rev() {
        let r = 2.5;
        window.paint_quad(
            fill(
                Bounds::new(
                    point(px(origin.0 + x - r), px(origin.1 + y - r)),
                    size(px(r * 2.), px(r * 2.)),
                ),
                with(alpha),
            )
            .corner_radii(px(r)),
        );
    }
    let at = (origin.0 + frame.at.0, origin.1 + frame.at.1);
    if let Some((diameter, alpha)) = frame.ripple {
        let r = diameter / 2.;
        window.paint_quad(
            fill(
                Bounds::new(
                    point(px(at.0 - r), px(at.1 - r)),
                    size(px(diameter), px(diameter)),
                ),
                with(alpha * 0.12),
            )
            .corner_radii(px(r))
            .border_widths(px(2.))
            .border_color(with(alpha)),
        );
    }
    if let Some(down) = frame.scroll {
        // Two chevrons under the arrow, the leading one brighter.
        let (cx, cy) = (at.0 + 6., at.1 + 30.);
        for (step, alpha) in [(0., 1.), (5., 0.5)] {
            let dy = if down { step } else { -step };
            let tip = if down { 4. } else { -4. };
            let mut path = PathBuilder::stroke(px(1.8));
            path.move_to(point(px(cx - 5.), px(cy + dy - tip / 2.)));
            path.line_to(point(px(cx), px(cy + dy + tip / 2.)));
            path.line_to(point(px(cx + 5.), px(cy + dy - tip / 2.)));
            if let Ok(path) = path.build() {
                window.paint_path(path, with(alpha));
            }
        }
    }
    let shadow = Hsla {
        h: 0.,
        s: 0.,
        l: 0.,
        a: 0.28 * frame.opacity,
    };
    if let Some(path) = arrow_path((at.0 + 0.6, at.1 + 1.2), 1., 1., None) {
        window.paint_path(path, shadow);
    }
    if let Some(path) = arrow_path(at, 1., 1., None) {
        window.paint_path(path, with(1.));
    }
    let edge = Hsla {
        h: 0.,
        s: 0.,
        l: 1.,
        a: 0.95 * frame.opacity,
    };
    if let Some(path) = arrow_path(at, 1., 1., Some(1.4)) {
        window.paint_path(path, edge);
    }
}

/// The pointer as elements over a surface the size of the screen: arrow, trail, ring, scroll hint
/// and the pill with the provider's mark and name.
pub fn element(pointer: &Pointer, now: f32, accent: Hsla) -> AnyElement {
    let frame = pointer.frame(now);
    let color: Hsla = color(&pointer.provider).map_or(accent, |c| gpui::rgb(c).into());
    let (x, y) = frame.at;
    let opacity = frame.opacity;
    let painted = frame.clone();
    div()
        .size_full()
        .relative()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| paint_frame(bounds, &painted, color, window),
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        .child(
            div()
                .absolute()
                .left(px(x + 15.))
                .top(px(y + 17.))
                .opacity(opacity)
                .flex()
                .items_center()
                .gap(px(4.))
                .pl(px(5.))
                .pr(px(7.))
                .py(px(2.))
                .rounded_full()
                .bg(Hsla {
                    h: 0.,
                    s: 0.,
                    l: 0.08,
                    a: 0.9,
                })
                .border_1()
                .border_color(Hsla { a: 0.7, ..color })
                .child(crate::theme::provider_glyph_in(
                    &pointer.provider,
                    11.,
                    color,
                ))
                .child(
                    div()
                        .text_size(px(11.))
                        .line_height(px(14.))
                        .text_color(gpui::white())
                        .child(name(&pointer.provider)),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_follow_the_provider_and_fall_back_to_the_accent() {
        assert_eq!(color("claude"), Some(0xd97757));
        assert_eq!(color("codex"), Some(0xe5e5e5));
        assert_eq!(color("antigravity"), Some(0x8fb4f5));
        assert_eq!(color("gemini"), Some(0x8fb4f5));
        assert_eq!(color("cursor"), None);
        assert_eq!(hex(0xd97757), "#d97757");
        assert_eq!(name("claude"), "Claude");
        let config = page_config("opencode", 0x112233, false, 1.);
        assert_eq!(config["color"], "#112233");
        assert_eq!(
            page_config("claude", 0x112233, false, 1.)["color"],
            "#d97757"
        );
    }

    #[test]
    fn the_glide_eases_out_and_lands_on_time() {
        let mut p = Pointer::new("claude", (0., 0.), Kind::Click, 0., false);
        p.aim("claude", Some((100., 0.)), Kind::Click, 1.);
        assert_eq!(p.position(1.), (0., 0.));
        let half = p.position(1. + GLIDE / 2.).0;
        assert!(half > 60., "ease-out covers most of the way early: {half}");
        assert_eq!(p.position(1. + GLIDE), (100., 0.));
        assert_eq!(p.position(5.), (100., 0.));
    }

    #[test]
    fn the_ripple_grows_from_10_to_46_and_fades_in_its_time() {
        assert_eq!(ripple(0., false), Some((RIPPLE_FROM, 1.)));
        let (d, a) = ripple(RIPPLE * 0.5, false).unwrap();
        assert!(d > 28. && d < RIPPLE_TO && (a - 0.5).abs() < 1e-5);
        assert!(ripple(RIPPLE * 0.999, false).unwrap().0 > 45.9);
        assert_eq!(ripple(RIPPLE, false), None);
        assert_eq!(ripple(-0.1, false), None);
        // Reduce motion: the ring stays one size.
        assert_eq!(ripple(0.3, true).unwrap().0, ripple(0., true).unwrap().0);
    }

    #[test]
    fn a_click_rings_when_the_arrow_lands() {
        let mut p = Pointer::new("codex", (0., 0.), Kind::Type, 0., false);
        p.aim("codex", Some((50., 50.)), Kind::Click, 1.);
        assert_eq!(p.frame(1.2).ripple, None);
        assert_eq!(p.frame(1. + GLIDE).ripple.map(|r| r.0), Some(RIPPLE_FROM));
        // Typing rests at the field: no ring, no move.
        let mut q = Pointer::new("codex", (5., 5.), Kind::Type, 0., false);
        q.aim("codex", None, Kind::Key, 0.5);
        assert_eq!(q.frame(0.5).ripple, None);
        assert_eq!(q.frame(0.5).at, (5., 5.));
    }

    #[test]
    fn the_trail_follows_the_glide_then_fades() {
        let mut p = Pointer::new("claude", (0., 0.), Kind::Click, 0., false);
        p.aim("claude", Some((200., 0.)), Kind::Click, 1.);
        let mid = p.frame(1.3);
        assert_eq!(mid.trail.len(), TRAIL_DOTS);
        assert!(
            mid.trail
                .windows(2)
                .all(|w| w[0].0 > w[1].0 && w[0].2 > w[1].2)
        );
        assert!(mid.trail.iter().all(|d| d.0 < mid.at.0));
        assert!(p.frame(1. + GLIDE + TRAIL_FADE).trail.is_empty());
        assert!(
            p.frame(1.02).trail.len() < 2,
            "no dots from before the move"
        );
    }

    #[test]
    fn reduce_motion_skips_glide_and_trail_for_a_short_fade() {
        let mut p = Pointer::new("claude", (0., 0.), Kind::Click, 0., true);
        p.aim("claude", Some((200., 0.)), Kind::Click, 1.);
        let f = p.frame(1. + FADE / 2.);
        assert_eq!(f.at, (200., 0.));
        assert!(f.trail.is_empty());
        assert!((f.opacity - 0.5).abs() < 1e-4);
        assert_eq!(p.frame(1.).ripple.map(|r| r.1), Some(1.));
    }

    #[test]
    fn it_hides_a_few_seconds_after_the_last_action() {
        let mut p = Pointer::new("claude", (0., 0.), Kind::Scroll { down: true }, 0., false);
        assert_eq!(p.frame(1.).scroll, Some(true));
        assert_eq!(p.frame(1.).opacity, 1.);
        assert!(p.visible(HIDE_AFTER));
        assert!(p.frame(HIDE_AFTER + FADE / 2.).opacity < 0.6);
        assert!(!p.visible(HIDE_AFTER + FADE));
        assert!(!p.animating(2.));
        p.aim("claude", Some((10., 10.)), Kind::Click, 10.);
        assert_eq!(
            p.frame(10.).at,
            (10., 10.),
            "a hidden pointer reappears at the target"
        );
    }

    #[test]
    fn screen_points_map_into_the_overlay_of_their_screen() {
        let screens = [
            Rect {
                x: 0.,
                y: 0.,
                width: 1512.,
                height: 982.,
            },
            Rect {
                x: -1920.,
                y: -200.,
                width: 1920.,
                height: 1080.,
            },
        ];
        assert_eq!(screen_for((100., 100.), &screens), Some(0));
        assert_eq!(screen_for((-10., 0.), &screens), Some(1));
        assert_eq!(
            screen_for((5000., 0.), &screens),
            Some(0),
            "off screen: the nearest"
        );
        assert_eq!(screen_for((0., 0.), &[]), None);
        assert_eq!(local_point((-1900., -100.), screens[1]), (20., 100.));
        assert_eq!(local_point((756., 491.), screens[0]), (756., 491.));
    }

    #[test]
    fn the_page_arrow_keeps_its_size_through_zoom_and_devices() {
        assert_eq!(page_scale(f64::NAN, 1.), 1.);
        assert_eq!(page_scale(f64::NAN, 2.), 0.5);
        // A device frame fit at 0.5 with the person's zoom at 100 %.
        assert_eq!(page_scale(0.5, 1.), 2.);
        assert_eq!(page_scale(0., 0.), 1.);
    }

    #[test]
    fn marks_become_plain_paths_or_a_dot() {
        let (view_box, paths, _) = mark_paths(mark_svg("claude")).unwrap();
        assert_eq!(view_box, "0 0 125 125");
        assert_eq!(paths.len(), 1);
        assert!(mark_paths(include_str!("../assets/antigravity.svg")).is_none());
        assert!(page_config("antigravity", 0, false, 1.)["mark"].is_null());
        assert_eq!(
            Kind::parse("browser_scroll", true),
            Some(Kind::Scroll { down: true })
        );
        assert_eq!(Kind::parse("screenshot", false), None);
    }
}
