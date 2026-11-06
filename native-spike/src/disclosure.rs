//! A fold's reveal, from Monocode (crates/monocode-gpui/src/reveal.rs, MIT, LICENSE-MONOCODE-MIT):
//! the child is measured at its full height at the current width and the height follows
//! `progress`, so nothing snaps when the motion ends. Put it inside a parent that clips.
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Style, Window, size,
};

pub(crate) const REVEAL: f32 = 0.32;
pub(crate) const FADE: f32 = 0.22;
pub(crate) const TURN: f32 = 0.2;

/// CSS `cubic-bezier(x1, y1, x2, y2)` at `t`.
pub(crate) fn bezier(t: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    if t <= 0. {
        return 0.;
    }
    if t >= 1. {
        return 1.;
    }
    let f = |s: f32, a: f32, b: f32| {
        3. * (1. - s) * (1. - s) * s * a + 3. * (1. - s) * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0., 1.);
    for _ in 0..12 {
        let middle = (low + high) * 0.5;
        if f(middle, x1, x2) < t {
            low = middle;
        } else {
            high = middle;
        }
    }
    f((low + high) * 0.5, y1, y2)
}

/// The reveal's curve: fast out, settling softly.
pub(crate) fn settle(t: f32) -> f32 {
    bezier(t, 0.22, 1., 0.36, 1.)
}

/// CSS `ease`, for the fade that runs under the height.
pub(crate) fn ease(t: f32) -> f32 {
    bezier(t, 0.25, 0.1, 0.25, 1.)
}

pub(crate) struct Disclosure {
    child: AnyElement,
    width: Pixels,
    progress: f32,
}

impl Disclosure {
    pub(crate) fn new(child: impl IntoElement, width: Pixels, progress: f32) -> Self {
        Self {
            child: child.into_any_element(),
            width,
            progress,
        }
    }
}

impl IntoElement for Disclosure {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Disclosure {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let natural = self.child.layout_as_root(
            size(
                AvailableSpace::Definite(self.width),
                AvailableSpace::MinContent,
            ),
            window,
            cx,
        );
        let mut style = Style::default();
        style.size.width = self.width.into();
        style.size.height = (natural.height * self.progress).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint_at(bounds.origin, window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curves_start_and_end_in_place_and_settle_early() {
        for curve in [settle, ease] {
            assert_eq!(curve(0.), 0.);
            assert_eq!(curve(1.), 1.);
        }
        assert!(settle(0.5) > 0.8);
    }
}
