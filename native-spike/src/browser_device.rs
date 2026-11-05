//! Device mode's sizing, after Sirus Code's `browser-device.ts` and `BrowserDevice.tsx`: typed
//! sizes and the rails around the frame that resize it by dragging. Pure functions.

pub const MIN_SIDE: u32 = 240;
pub const MAX_SIDE: u32 = 3840;
/// Space kept around a device frame for its rails (`GUTTER`).
pub const GUTTER: f64 = 12.;
/// How thick a rail is (`RAIL`).
pub const RAIL: f64 = 10.;

/// A frame's side clamped to what the host accepts; anything unusable becomes the minimum.
pub fn clamp_side(value: f64) -> u32 {
    if value.is_finite() {
        value.round().clamp(MIN_SIDE as f64, MAX_SIDE as f64) as u32
    } else {
        MIN_SIDE
    }
}

/// A typed side: digits only, at most four, `None` when empty.
pub fn digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).take(4).collect()
}

/// The number in a field, if it holds one.
pub fn parse_side(text: &str) -> Option<u32> {
    digits(text).parse().ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Bottom,
    BottomLeft,
    BottomRight,
}

pub const EDGES: [Edge; 5] = [
    Edge::Left,
    Edge::Right,
    Edge::Bottom,
    Edge::BottomLeft,
    Edge::BottomRight,
];

/// The CSS size after dragging an edge by `(dx, dy)` screen pixels while the frame is shown at
/// `scale`. The frame stays centred, so a side drag grows both sides (`drag`).
pub fn dragged(edge: Edge, start: (u32, u32), dx: f64, dy: f64, scale: f64) -> (u32, u32) {
    let scale = scale.max(0.1);
    let (dx, dy) = (dx / scale, dy / scale);
    let (width, height) = (start.0 as f64, start.1 as f64);
    let width = match edge {
        Edge::Bottom => width,
        Edge::Left | Edge::BottomLeft => width - dx * 2.,
        Edge::Right | Edge::BottomRight => width + dx * 2.,
    };
    let height = match edge {
        Edge::Left | Edge::Right => height,
        _ => height + dy,
    };
    (clamp_side(width), clamp_side(height))
}

/// A rail's rectangle `(x, y, width, height)` relative to the page area, for a frame of
/// `frame` size centred at `left` and starting `GUTTER` from the top.
pub fn rail(edge: Edge, left: f64, frame: (f64, f64)) -> (f64, f64, f64, f64) {
    let bottom = GUTTER + frame.1;
    match edge {
        Edge::Left => (left - RAIL, GUTTER, RAIL, frame.1),
        Edge::Right => (left + frame.0, GUTTER, RAIL, frame.1),
        Edge::Bottom => (left, bottom, frame.0, RAIL),
        Edge::BottomLeft => (left - RAIL, bottom, RAIL, RAIL),
        Edge::BottomRight => (left + frame.0, bottom, RAIL, RAIL),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides_are_clamped_and_typed_text_keeps_four_digits() {
        assert_eq!(clamp_side(10.), MIN_SIDE);
        assert_eq!(clamp_side(99999.), MAX_SIDE);
        assert_eq!(clamp_side(f64::NAN), MIN_SIDE);
        assert_eq!(clamp_side(500.4), 500);
        assert_eq!(digits("12a3456"), "1234");
        assert_eq!(parse_side("  800px"), Some(800));
        assert_eq!(parse_side(""), None);
    }

    #[test]
    fn dragging_edges_resizes_around_the_centre_at_the_shown_scale() {
        let start = (400, 800);
        // The right edge moving 50 screen px at half scale: 100 CSS px each side.
        assert_eq!(dragged(Edge::Right, start, 50., 0., 0.5), (600, 800));
        assert_eq!(dragged(Edge::Left, start, -50., 0., 0.5), (600, 800));
        assert_eq!(dragged(Edge::Left, start, 50., 0., 1.), (300, 800));
        // The bottom only changes the height; corners change both.
        assert_eq!(dragged(Edge::Bottom, start, 90., 40., 1.), (400, 840));
        assert_eq!(dragged(Edge::BottomRight, start, 10., 20., 1.), (420, 820));
        assert_eq!(dragged(Edge::BottomLeft, start, 10., 20., 1.), (380, 820));
        // Never below the minimum or past the maximum.
        assert_eq!(dragged(Edge::Right, start, -900., 0., 1.), (MIN_SIDE, 800));
        assert_eq!(dragged(Edge::Bottom, start, 0., 9000., 1.), (400, MAX_SIDE));
    }

    #[test]
    fn rails_hug_the_frame() {
        let frame = (200., 400.);
        assert_eq!(rail(Edge::Left, 50., frame), (40., GUTTER, RAIL, 400.));
        assert_eq!(rail(Edge::Right, 50., frame), (250., GUTTER, RAIL, 400.));
        assert_eq!(rail(Edge::Bottom, 50., frame), (50., 412., 200., RAIL));
        assert_eq!(rail(Edge::BottomRight, 50., frame), (250., 412., RAIL, RAIL));
    }
}
