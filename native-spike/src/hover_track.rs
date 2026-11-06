//! One sliding hover highlight per list, after the web app's SidebarHoverTrack.
//!
//! A list owns one quiet pill that glides between hovered rows instead of each row lighting
//! up on its own. The first hover appears in place and only later moves slide, so the list
//! never seems to move by itself; leaving fades it out; a selected row keeps its own fill, so
//! the pill fades instead of doubling it; scrolling hides it until the next mouse move; and
//! reduced motion turns the slide off. The state holds no timer: it asks for another frame
//! only while a tween runs, and nothing redraws at rest.
//!
//! Use: `let track = hover_track::list("add-menu");`, put `track.pill()` as the first child of
//! the container, and wrap each row with `track.row(row, ix, selected)`. The row then drops its
//! own `.hover(|s| s.bg(content(..)))`.
use crate::motion::{Curve, Tween};
use crate::theme::content;
use gpui::{
    Bounds, DispatchPhase, Hitbox, HitboxBehavior, MouseMoveEvent, ParentElement, Pixels,
    ScrollWheelEvent, Styled, Window, canvas, fill, point, px, size,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

/// The pill's fill, the same alpha rows used for their own hover.
pub const FILL: f32 = 0.07;
const RADIUS: f32 = crate::theme::RADIUS_MD;
/// The web pill: 320 ms moves (--motion-slow), 140 ms fades (--motion-fast), ease-out.
const SLIDE: f32 = 0.32;
const FADE: f32 = 0.14;
const CURVE: Curve = Curve::Expo;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The pill's own state, free of any window so it can be tested.
pub struct Pill {
    x: Tween,
    y: Tween,
    w: Tween,
    h: Tween,
    shown: Tween,
    covered: Tween,
    row: Option<usize>,
    hidden: bool,
}

impl Pill {
    pub fn new(now: Instant) -> Self {
        Self {
            x: Tween::still(0., now),
            y: Tween::still(0., now),
            w: Tween::still(0., now),
            h: Tween::still(0., now),
            shown: Tween::still(0., now),
            covered: Tween::still(0., now),
            row: None,
            hidden: false,
        }
    }

    fn snap(&mut self, r: Rect, now: Instant) {
        self.x.snap(r.x, now);
        self.y.snap(r.y, now);
        self.w.snap(r.w, now);
        self.h.snap(r.h, now);
    }

    fn target(&self) -> Rect {
        Rect {
            x: self.x.to,
            y: self.y.to,
            w: self.w.to,
            h: self.h.to,
        }
    }

    /// The row under the mouse now (its index, bounds and whether it is selected), or none.
    pub fn point(&mut self, hover: Option<(usize, Rect, bool)>, now: Instant, reduce_motion: bool) {
        let hover = if self.hidden { None } else { hover };
        let Some((row, rect, selected)) = hover else {
            self.row = None;
            self.shown.set(0., now, FADE, CURVE);
            return;
        };
        let first = self.shown.to < 0.5;
        if first || reduce_motion {
            self.snap(rect, now);
        } else if self.row != Some(row) {
            self.x.set(rect.x, now, SLIDE, CURVE);
            self.y.set(rect.y, now, SLIDE, CURVE);
            self.w.set(rect.w, now, SLIDE, CURVE);
            self.h.set(rect.h, now, SLIDE, CURVE);
        } else if self.target() != rect {
            // Same row, new place (a scroll, a relayout): follow it, don't glide.
            self.snap(rect, now);
        }
        self.row = Some(row);
        self.shown.set(1., now, FADE, CURVE);
        self.covered
            .set(if selected { 1. } else { 0. }, now, FADE, CURVE);
    }

    /// A scroll hides the pill until the mouse moves again.
    pub fn scrolled(&mut self, now: Instant) {
        self.hidden = true;
        self.row = None;
        self.shown.snap(0., now);
    }

    /// The mouse moved; returns true when this brings a hidden pill back.
    pub fn moved(&mut self) -> bool {
        std::mem::replace(&mut self.hidden, false)
    }

    pub fn hidden(&self) -> bool {
        self.hidden
    }

    pub fn row(&self) -> Option<usize> {
        self.row
    }

    pub fn visible(&self) -> bool {
        self.shown.to > 0.5
    }

    /// What to paint now: the rect and the opacity, 0 to 1.
    pub fn current(&self, now: Instant) -> Option<(Rect, f32)> {
        let alpha = self.shown.at(now) * (1. - self.covered.at(now));
        (alpha > 0.002).then(|| {
            (
                Rect {
                    x: self.x.at(now),
                    y: self.y.at(now),
                    w: self.w.at(now),
                    h: self.h.at(now),
                },
                alpha,
            )
        })
    }

    pub fn animating(&self, now: Instant) -> bool {
        ![
            &self.x,
            &self.y,
            &self.w,
            &self.h,
            &self.shown,
            &self.covered,
        ]
        .iter()
        .all(|t| t.done(now))
    }
}

struct RowInfo {
    ix: usize,
    hitbox: Hitbox,
    bounds: Bounds<Pixels>,
    selected: bool,
}

struct Inner {
    pill: Pill,
    rows: Vec<RowInfo>,
}

impl Inner {
    fn hovered(&self, window: &Window) -> Option<(usize, Rect, bool)> {
        self.rows
            .iter()
            .find(|r| r.hitbox.is_hovered(window))
            .map(|r| {
                let b = r.bounds;
                let rect = Rect {
                    x: b.origin.x.into(),
                    y: b.origin.y.into(),
                    w: b.size.width.into(),
                    h: b.size.height.into(),
                };
                (r.ix, rect, r.selected)
            })
    }
}

/// A handle to one list's track. Cloning shares the state.
#[derive(Clone)]
pub struct HoverTrack(Rc<RefCell<Inner>>);

thread_local! {
    static TRACKS: RefCell<HashMap<String, HoverTrack>> = RefCell::new(HashMap::new());
}

/// The track of the list named `key`, made on first use. The UI is single-threaded, so the
/// registry is thread-local and rows need no field on the view.
pub fn list(key: &str) -> HoverTrack {
    TRACKS.with(|tracks| {
        tracks
            .borrow_mut()
            .entry(key.to_string())
            .or_insert_with(|| {
                HoverTrack(Rc::new(RefCell::new(Inner {
                    pill: Pill::new(Instant::now()),
                    rows: Vec::new(),
                })))
            })
            .clone()
    })
}

impl HoverTrack {
    /// The pill itself: the container's first child, so it paints under every row.
    pub fn pill(&self) -> impl gpui::IntoElement + use<> {
        let track = self.0.clone();
        let reset = self.0.clone();
        canvas(
            move |_, _, _| reset.borrow_mut().rows.clear(),
            move |bounds, _, window, cx| {
                let now = Instant::now();
                {
                    let mut inner = track.borrow_mut();
                    let hover = inner.hovered(window);
                    inner.pill.point(hover, now, cx.reduce_motion());
                    if let Some((r, alpha)) = inner.pill.current(now) {
                        window.paint_quad(
                            fill(
                                Bounds::new(point(px(r.x), px(r.y)), size(px(r.w), px(r.h))),
                                content(FILL * alpha),
                            )
                            .corner_radii(px(RADIUS)),
                        );
                    }
                    if inner.pill.animating(now) {
                        window.request_animation_frame();
                    }
                }
                let moves = track.clone();
                window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, _| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let mut inner = moves.borrow_mut();
                    let woke = inner.pill.moved();
                    let row = inner.hovered(window).map(|h| h.0);
                    if woke || row != inner.pill.row() {
                        window.refresh();
                    }
                });
                let scrolls = track.clone();
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, _| {
                    if phase != DispatchPhase::Capture || !bounds.contains(&e.position) {
                        return;
                    }
                    let mut inner = scrolls.borrow_mut();
                    if !inner.pill.hidden() && inner.pill.visible() {
                        inner.pill.scrolled(Instant::now());
                        window.refresh();
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// `row` for rows with no number, keyed by a name.
    pub fn row_named<E: ParentElement + Styled>(&self, row: E, name: &str, selected: bool) -> E {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut hasher);
        self.row(row, hasher.finish() as usize, selected)
    }

    /// Wraps a row: reports its bounds and hover to the track. The row keeps no hover fill.
    pub fn row<E: ParentElement + Styled>(&self, row: E, ix: usize, selected: bool) -> E {
        let track = self.0.clone();
        row.child(
            canvas(
                move |bounds, window, _| {
                    let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                    track.borrow_mut().rows.push(RowInfo {
                        ix,
                        hitbox,
                        bounds,
                        selected,
                    });
                },
                |_, _, _, _| {},
            )
            // Pinned to the corner: left to its static place, it would start after the row's
            // padding yet keep the row's full width, and the pill would stick out on the right.
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn rect(y: f32) -> Rect {
        Rect {
            x: 0.,
            y,
            w: 100.,
            h: 28.,
        }
    }
    fn later(t: Instant, ms: u64) -> Instant {
        t + Duration::from_millis(ms)
    }

    #[test]
    fn first_hover_appears_in_place() {
        let t = Instant::now();
        let mut p = Pill::new(t);
        p.point(Some((2, rect(56.), false)), t, false);
        let (r, _) = p.current(later(t, 1)).unwrap();
        assert_eq!(r, rect(56.));
    }

    #[test]
    fn later_hover_slides() {
        let t = Instant::now();
        let mut p = Pill::new(t);
        p.point(Some((0, rect(0.), false)), t, false);
        let t2 = later(t, 500);
        p.point(Some((3, rect(84.), false)), t2, false);
        let (mid, _) = p.current(later(t2, 60)).unwrap();
        assert!(mid.y > 0. && mid.y < 84.);
        assert!(p.animating(later(t2, 60)));
        let (end, _) = p.current(later(t2, 400)).unwrap();
        assert!((end.y - 84.).abs() < 0.01);
        assert!(!p.animating(later(t2, 400)));
    }

    #[test]
    fn leaving_fades_out_and_the_next_hover_is_in_place_again() {
        let t = Instant::now();
        let mut p = Pill::new(t);
        p.point(Some((0, rect(0.), false)), t, false);
        let t2 = later(t, 500);
        p.point(None, t2, false);
        let (_, mid) = p.current(later(t2, 60)).unwrap();
        assert!(mid > 0. && mid < 1.);
        assert!(p.current(later(t2, 400)).is_none());
        let t3 = later(t, 1000);
        p.point(Some((4, rect(112.), false)), t3, false);
        assert_eq!(p.current(later(t3, 1)).unwrap().0, rect(112.));
    }

    #[test]
    fn selected_row_fades_the_pill_instead_of_doubling() {
        let t = Instant::now();
        let mut p = Pill::new(t);
        p.point(Some((1, rect(28.), false)), t, false);
        let t2 = later(t, 500);
        p.point(Some((2, rect(56.), true)), t2, false);
        assert!(p.current(later(t2, 600)).is_none());
        let t3 = later(t, 1500);
        p.point(Some((3, rect(84.), false)), t3, false);
        let (_, a) = p.current(later(t3, 400)).unwrap();
        assert!(a > 0.99);
    }

    #[test]
    fn scroll_hides_until_the_next_move() {
        let t = Instant::now();
        let mut p = Pill::new(t);
        p.point(Some((1, rect(28.), false)), t, false);
        p.scrolled(later(t, 10));
        assert!(p.current(later(t, 10)).is_none());
        p.point(Some((1, rect(0.), false)), later(t, 20), false);
        assert!(p.current(later(t, 600)).is_none());
        assert!(p.moved());
        assert!(!p.moved());
        p.point(Some((1, rect(0.), false)), later(t, 700), false);
        assert_eq!(p.current(later(t, 701)).unwrap().0, rect(0.));
    }

    #[test]
    fn same_row_that_moves_follows_without_gliding() {
        let t = Instant::now();
        let mut p = Pill::new(t);
        p.point(Some((1, rect(28.), false)), t, false);
        let t2 = later(t, 500);
        p.point(Some((1, rect(10.), false)), t2, false);
        assert_eq!(p.current(t2).unwrap().0, rect(10.));
    }

    #[test]
    fn reduced_motion_never_slides() {
        let t = Instant::now();
        let mut p = Pill::new(t);
        p.point(Some((0, rect(0.), false)), t, true);
        let t2 = later(t, 500);
        p.point(Some((3, rect(84.), false)), t2, true);
        assert_eq!(p.current(later(t2, 1)).unwrap().0, rect(84.));
    }

    #[test]
    fn at_rest_nothing_animates() {
        let t = Instant::now();
        let mut p = Pill::new(t);
        p.point(Some((0, rect(0.), false)), t, false);
        assert!(!p.animating(later(t, 1000)));
        let idle = Pill::new(t);
        assert!(!idle.animating(t));
    }
}
