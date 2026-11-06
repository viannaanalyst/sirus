//! The composer's submit button as one morphing control. Empty, it is a
//! small square holding its glyph; with something to send it grows
//! into a wider button that names what will happen (Send, Queue, Compact and send),
//! and while a turn runs with nothing typed it is a square with a stop mark.
//! When the action changes, say a held modifier turns Queue into Steer, the
//! glyph and label roll to the new one, as the title tabs roll their pane.
//!
//! From Monocode's send_morph.rs (see LICENSE-MONOCODE-MIT).
use crate::motion::{Curve, eased};
use crate::theme::*;
use gpui::{AnyElement, Hsla, MouseButton, div, prelude::*, px};
use std::time::{Duration, Instant};

const HEIGHT: f32 = 28.;
/// The same soft corner square or pill: it widens, it never turns round.
const RADIUS: f32 = RADIUS_MD;
/// The collapsed square, and the glyph's slot inside the pill.
const SQUARE: f32 = 28.;
const GLYPH: f32 = 14.;
const LABEL_GAP: f32 = 6.;
const LABEL_SIZE: f32 = 12.;
const PAD_RIGHT: f32 = 10.;
const MORPH: Duration = Duration::from_millis(320);
const ROLL: Duration = Duration::from_millis(240);

/// What the button offers right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Face {
    pub label: &'static str,
    pub glyph: &'static str,
    /// A stop square instead of the glyph.
    pub stop: bool,
    /// Grown into a pill that shows the label.
    pub expanded: bool,
}

impl Face {
    fn same_line(&self, other: &Face) -> bool {
        self.label == other.label && self.glyph == other.glyph && self.stop == other.stop
    }
}

/// The button's motion between renders.
#[derive(Default)]
pub(crate) struct Morph {
    shown: Option<Face>,
    width: Option<(f32, f32, Instant)>,
    roll: Option<(Face, f32, Instant)>,
}

/// One render's worth of the morph.
pub(crate) struct Frame {
    width: f32,
    /// How grown the pill is, 0 square to 1 full label.
    open: f32,
    /// The line rolling out, its direction and the eased progress.
    roll: Option<(Face, f32, f32)>,
    pub moving: bool,
}

fn ease(start: Instant, length: Duration, now: Instant) -> f32 {
    let t = now.saturating_duration_since(start).as_secs_f32() / length.as_secs_f32();
    eased(t, Curve::Expo)
}

/// The pill's width for `label`: an average advance, so the pill never clips a word.
pub(crate) fn expanded_width(label: &str) -> f32 {
    // The glyph's square already holds its own inset and the gap.
    SQUARE + label.chars().count() as f32 * 6.3 + PAD_RIGHT
}

impl Morph {
    /// Steps toward `face`, whose pill is `target` wide when expanded.
    pub(crate) fn frame(&mut self, face: Face, target: f32, animate: bool) -> Frame {
        let now = Instant::now();
        let to = if face.expanded { target } else { SQUARE };
        let previous = self.shown.replace(face);
        // Width: ease from wherever it is now toward the new size.
        let current = self.width.map_or(to, |(from, goal, start)| {
            from + (goal - from) * ease(start, MORPH, now)
        });
        match self.width {
            Some((_, goal, _)) if (goal - to).abs() < 0.5 => {}
            _ => self.width = Some((if animate { current } else { to }, to, now)),
        }
        // The line rolls when the action changes, not when it only grows.
        if let Some(previous) = previous.filter(|p| animate && !p.same_line(&face)) {
            self.roll = Some((previous, 1., now));
        }
        let (from, goal, start) = self.width.unwrap_or((to, to, now));
        let grow = ease(start, MORPH, now);
        let width = if now.saturating_duration_since(start) < MORPH {
            from + (goal - from) * grow
        } else {
            goal
        };
        let roll = self.roll.and_then(|(face, dir, start)| {
            let p = ease(start, ROLL, now);
            (p < 1.).then_some((face, dir, p))
        });
        if roll.is_none() {
            self.roll = None;
        }
        let span = (target - SQUARE).max(1.);
        Frame {
            width,
            open: ((width - SQUARE) / span).clamp(0., 1.),
            roll,
            // By the clock: the curve's solver may stop just short of 1, which would redraw forever.
            moving: now.saturating_duration_since(start) < MORPH || roll.is_some(),
        }
    }
}

fn palette(enabled: bool) -> (Hsla, Hsla) {
    primary_action(enabled)
}

/// The flat surface: the accent, or its dim wash with nothing to do.
fn surface(fill: Hsla) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(px(RADIUS))
        .bg(fill)
}

/// One line of the button: its glyph (or stop square) and, as the pill
/// opens, its label.
fn line(face: Face, ink: Hsla, open: f32) -> gpui::Div {
    div()
        .h(px(HEIGHT))
        .flex()
        .items_center()
        .pl(px((SQUARE - GLYPH) / 2.))
        .gap(px(LABEL_GAP))
        .child(
            div()
                .size(px(GLYPH))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(if face.stop {
                    div()
                        .size(px(10.))
                        .rounded(px(2.))
                        .bg(ink)
                        .into_any_element()
                } else {
                    icon(face.glyph, GLYPH).text_color(ink).into_any_element()
                }),
        )
        .child(
            div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .text_size(px(LABEL_SIZE))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ink)
                .opacity(open)
                .child(face.label),
        )
}

/// The button for `face` at `frame`.
pub(crate) fn button(
    id: &'static str,
    face: Face,
    frame: &Frame,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    let (fill, ink) = palette(enabled);
    let open = frame.open;
    let lines: Vec<AnyElement> = match frame.roll {
        // The new action rises in as the old one leaves, or drops back.
        Some((old, dir, p)) => vec![
            line(old, ink, open)
                .absolute()
                .left_0()
                .top(px(-dir * p * HEIGHT))
                .opacity(1. - p)
                .into_any_element(),
            line(face, ink, open)
                .absolute()
                .left_0()
                .top(px(dir * (1. - p) * HEIGHT))
                .opacity(p.max(0.2))
                .into_any_element(),
        ],
        None => vec![
            line(face, ink, open)
                .absolute()
                .left_0()
                .top_0()
                .into_any_element(),
        ],
    };
    div()
        .id(id)
        .group(id)
        .cursor_pointer()
        .flex_shrink_0()
        .relative()
        .w(px(frame.width))
        .h(px(HEIGHT))
        .child(surface(fill))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .overflow_hidden()
                .children(lines),
        )
        // A brighter face under the pointer.
        .when(enabled, |d| {
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .rounded(px(RADIUS))
                    .group_hover(id, |s| s.bg(white(0.08))),
            )
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}
