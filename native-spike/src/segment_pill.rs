//! The thumb of a segmented control: one pill that slides to the chosen segment, whatever each
//! segment's width. Segments report where they are with `probe`, and the pill, painted first,
//! reads that in the same frame. Asks for frames only while it moves.
//!
//! Use: a `.relative()` container with `pill(key, chosen, fill, radius)` as its first child, and
//! `probe(key, ix)` as a child of each `.relative()` segment, which drops its own selected fill.
use crate::motion::{Curve, Tween};
use gpui::{App, Bounds, Hsla, IntoElement, Pixels, Styled, Window, canvas, fill, point, px, size};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

const SLIDE: f32 = 0.22;
const BAR_WIDTH: f32 = 2.5;
const BAR_HEIGHT: f32 = 18.;
/// A list's selection, as quick as its hover (hover_track.rs).
const ROW_SLIDE: f32 = 0.13;

#[derive(Clone, Copy)]
struct Thumb {
    x: Tween,
    y: Tween,
    w: Tween,
    h: Tween,
}

impl Thumb {
    fn at(rest: [f32; 4], now: Instant) -> Self {
        Self {
            x: Tween::still(rest[0], now),
            y: Tween::still(rest[1], now),
            w: Tween::still(rest[2], now),
            h: Tween::still(rest[3], now),
        }
    }

    /// Heads for `rest`; a new size of the same place follows at once, only a new place glides.
    fn head_for(&mut self, rest: [f32; 4], now: Instant, length: f32) {
        self.x.set(rest[0], now, length, Curve::Expo);
        self.w.set(rest[2], now, length, Curve::Expo);
        self.y.snap(rest[1], now);
        self.h.snap(rest[3], now);
    }

    /// The same for rows of a list, which move up and down.
    fn head_down(&mut self, rest: [f32; 4], now: Instant, length: f32) {
        self.y.set(rest[1], now, length, Curve::Expo);
        self.h.set(rest[3], now, length, Curve::Expo);
        self.x.snap(rest[0], now);
        self.w.snap(rest[2], now);
    }

    fn done(&self, now: Instant) -> bool {
        self.x.done(now) && self.w.done(now) && self.y.done(now) && self.h.done(now)
    }
}

thread_local! {
    static PLACES: RefCell<HashMap<(&'static str, usize), Bounds<Pixels>>> = RefCell::default();
    static THUMBS: RefCell<HashMap<&'static str, Thumb>> = RefCell::default();
}

/// Reports where a segment is, before anything paints.
pub fn probe(key: &'static str, ix: usize) -> impl IntoElement {
    canvas(
        move |bounds, _: &mut Window, _: &mut App| {
            PLACES.with(|places| places.borrow_mut().insert((key, ix), bounds));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Forgets a thumb, so the next time it shows it starts in place instead of sliding in.
pub fn forget(key: &'static str) {
    THUMBS.with(|thumbs| thumbs.borrow_mut().remove(key));
}

/// A list's selection: a fill across the row and a short bar at its edge, gliding together
/// between rows. The same `probe`, on each `.relative()` row.
pub fn row_thumb(key: &'static str, chosen: usize, color: Hsla, bar: Hsla) -> impl IntoElement {
    canvas(
        |_, _: &mut Window, _: &mut App| {},
        move |bounds, _, window: &mut Window, cx: &mut App| {
            let Some(target) = PLACES.with(|places| places.borrow().get(&(key, chosen)).copied())
            else {
                return;
            };
            let rest = [
                f32::from(target.origin.x - bounds.origin.x),
                f32::from(target.origin.y - bounds.origin.y),
                f32::from(target.size.width),
                f32::from(target.size.height),
            ];
            let now = Instant::now();
            let length = if cx.reduce_motion() { 0. } else { ROW_SLIDE };
            let thumb = THUMBS.with(|thumbs| {
                let mut thumbs = thumbs.borrow_mut();
                let thumb = thumbs.entry(key).or_insert_with(|| Thumb::at(rest, now));
                thumb.head_down(rest, now, length);
                *thumb
            });
            if !thumb.done(now) {
                window.request_animation_frame();
            }
            let (y, h) = (thumb.y.at(now), thumb.h.at(now));
            let origin = bounds.origin + point(px(thumb.x.at(now)), px(y));
            window.paint_quad(fill(
                Bounds::new(origin, size(px(thumb.w.at(now)), px(h))),
                color,
            ));
            window.paint_quad(
                fill(
                    Bounds::new(
                        origin + point(px(0.), px((h - BAR_HEIGHT) / 2.)),
                        size(px(BAR_WIDTH), px(BAR_HEIGHT)),
                    ),
                    bar,
                )
                .corner_radii(px(BAR_WIDTH / 2.)),
            );
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The thumb, under the segments.
pub fn pill(key: &'static str, chosen: usize, color: Hsla, radius: f32) -> impl IntoElement {
    pill_for(key, chosen, color, radius, SLIDE)
}

/// The same thumb with its own slide length, in seconds.
pub fn pill_for(
    key: &'static str,
    chosen: usize,
    color: Hsla,
    radius: f32,
    slide: f32,
) -> impl IntoElement {
    canvas(
        |_, _: &mut Window, _: &mut App| {},
        move |bounds, _, window: &mut Window, cx: &mut App| {
            let Some(target) = PLACES.with(|places| places.borrow().get(&(key, chosen)).copied())
            else {
                return;
            };
            let rest = [
                f32::from(target.origin.x - bounds.origin.x),
                f32::from(target.origin.y - bounds.origin.y),
                f32::from(target.size.width),
                f32::from(target.size.height),
            ];
            let now = Instant::now();
            let length = if cx.reduce_motion() { 0. } else { slide };
            let thumb = THUMBS.with(|thumbs| {
                let mut thumbs = thumbs.borrow_mut();
                let thumb = thumbs.entry(key).or_insert_with(|| Thumb::at(rest, now));
                thumb.head_for(rest, now, length);
                *thumb
            });
            if !thumb.done(now) {
                window.request_animation_frame();
            }
            window.paint_quad(
                fill(
                    Bounds::new(
                        bounds.origin + point(px(thumb.x.at(now)), px(thumb.y.at(now))),
                        size(px(thumb.w.at(now)), px(thumb.h.at(now))),
                    ),
                    color,
                )
                .corner_radii(px(radius)),
            );
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_thumb_starts_in_place_then_glides_to_a_new_segment() {
        let now = Instant::now();
        let mut thumb = Thumb::at([0., 2., 60., 24.], now);
        assert!(thumb.done(now));
        thumb.head_for([60., 2., 80., 24.], now, SLIDE);
        assert!(!thumb.done(now));
        assert_eq!(thumb.x.at(now), 0.);
        let later = now + Duration::from_secs(1);
        assert!(thumb.done(later));
        assert_eq!((thumb.x.at(later), thumb.w.at(later)), (60., 80.));
    }

    #[test]
    fn reduced_motion_jumps() {
        let now = Instant::now();
        let mut thumb = Thumb::at([0., 0., 10., 10.], now);
        thumb.head_for([40., 0., 10., 10.], now, 0.);
        assert_eq!(thumb.x.at(now), 40.);
    }
}
