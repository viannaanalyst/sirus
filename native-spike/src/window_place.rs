//! Where the main window was: its size, place and whether it was maximized or full screen are
//! kept in `window.json`, and the next launch opens it there, as long as it still lands on a
//! screen that is connected.

use gpui::{App, Bounds, Pixels, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The size of a first launch, centered on the main screen.
pub const FIRST: (f32, f32) = (1320., 860.);
/// The smallest the window may be.
pub const MIN: (f32, f32) = (820., 520.);
/// How much of the window must stay on a screen for the saved place to count.
const VISIBLE: (f32, f32) = (160., 80.);
/// Writes while a resize or a move is under way at most this often; the last one waits for quit.
const WRITE_EVERY: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub maximized: bool,
    #[serde(default)]
    pub fullscreen: bool,
}

fn file() -> std::path::PathBuf {
    crate::backend::data_dir().join("window.json")
}

fn saved() -> Option<Saved> {
    let bytes = std::fs::read(file()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Where the window opens: the saved place when it fits a connected screen, else the first-launch
/// size centered.
pub fn opening(cx: &App) -> WindowBounds {
    let screens: Vec<Bounds<Pixels>> = cx.displays().iter().map(|d| d.bounds()).collect();
    match saved().and_then(|saved| fit(saved, &screens).map(|bounds| (saved, bounds))) {
        Some((saved, bounds)) if saved.fullscreen => WindowBounds::Fullscreen(bounds),
        Some((saved, bounds)) if saved.maximized => WindowBounds::Maximized(bounds),
        Some((_, bounds)) => WindowBounds::Windowed(bounds),
        None => WindowBounds::Windowed(Bounds::centered(None, size(px(FIRST.0), px(FIRST.1)), cx)),
    }
}

/// The saved place, no smaller than the minimum and no larger than the screen it is on, or None
/// when too little of it would show on any screen.
pub fn fit(saved: Saved, screens: &[Bounds<Pixels>]) -> Option<Bounds<Pixels>> {
    if ![saved.x, saved.y, saved.w, saved.h]
        .iter()
        .all(|n| n.is_finite())
    {
        return None;
    }
    let (w, h) = (saved.w.max(MIN.0), saved.h.max(MIN.1));
    let screen = screens.iter().find(|screen| {
        let (sx, sy) = (f32::from(screen.origin.x), f32::from(screen.origin.y));
        let (sw, sh) = (f32::from(screen.size.width), f32::from(screen.size.height));
        let across = (saved.x + w).min(sx + sw) - saved.x.max(sx);
        let down = (saved.y + h).min(sy + sh) - saved.y.max(sy);
        across >= VISIBLE.0 && down >= VISIBLE.1
    })?;
    let (sx, sy) = (f32::from(screen.origin.x), f32::from(screen.origin.y));
    let (sw, sh) = (f32::from(screen.size.width), f32::from(screen.size.height));
    let (w, h) = (w.min(sw), h.min(sh));
    let x = saved.x.clamp(sx, sx + sw - w);
    let y = saved.y.clamp(sy, sy + sh - h);
    Some(Bounds::new(point(px(x), px(y)), size(px(w), px(h))))
}

struct Pending {
    latest: Option<Saved>,
    written: Option<Instant>,
}

static PENDING: Mutex<Pending> = Mutex::new(Pending {
    latest: None,
    written: None,
});

/// The window moved or changed size: keep where it is now.
pub fn remember(bounds: WindowBounds) {
    let (rect, maximized, fullscreen) = match bounds {
        WindowBounds::Windowed(rect) => (rect, false, false),
        WindowBounds::Maximized(rect) => (rect, true, false),
        WindowBounds::Fullscreen(rect) => (rect, false, true),
    };
    let now = Saved {
        x: f32::from(rect.origin.x),
        y: f32::from(rect.origin.y),
        w: f32::from(rect.size.width),
        h: f32::from(rect.size.height),
        maximized,
        fullscreen,
    };
    let Ok(mut pending) = PENDING.lock() else {
        return;
    };
    pending.latest = Some(now);
    if pending
        .written
        .is_none_or(|written| written.elapsed() >= WRITE_EVERY)
    {
        pending.written = Some(Instant::now());
        write(now);
    }
}

/// Writes the last place before the app leaves.
pub fn flush() {
    if let Some(latest) = PENDING.lock().ok().and_then(|pending| pending.latest) {
        write(latest);
    }
}

fn write(saved: Saved) {
    if let Ok(json) = serde_json::to_vec(&saved) {
        let _ = std::fs::create_dir_all(crate::backend::data_dir());
        let _ = std::fs::write(file(), json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    fn at(x: f32, y: f32, w: f32, h: f32) -> Saved {
        Saved {
            x,
            y,
            w,
            h,
            maximized: false,
            fullscreen: false,
        }
    }

    #[test]
    fn a_place_on_a_screen_comes_back_as_it_was() {
        let screens = [screen(0., 0., 1728., 1117.)];
        let bounds = fit(at(100., 50., 1400., 900.), &screens).unwrap();
        assert_eq!(bounds, screen(100., 50., 1400., 900.));
    }

    #[test]
    fn a_place_on_a_screen_that_left_opens_centered() {
        let screens = [screen(0., 0., 1728., 1117.)];
        assert!(fit(at(2000., 100., 1200., 800.), &screens).is_none());
        // A second screen to the right still counts while it is connected.
        let both = [
            screen(0., 0., 1728., 1117.),
            screen(1728., 0., 2560., 1440.),
        ];
        assert!(fit(at(2000., 100., 1200., 800.), &both).is_some());
    }

    #[test]
    fn a_window_larger_than_its_screen_shrinks_and_moves_inside_it() {
        let screens = [screen(0., 0., 1440., 900.)];
        let bounds = fit(at(1200., -40., 2000., 1200.), &screens).unwrap();
        assert_eq!(bounds, screen(0., 0., 1440., 900.));
        let small = fit(at(10., 10., 100., 100.), &screens).unwrap();
        assert_eq!(small.size, size(px(MIN.0), px(MIN.1)));
    }
}
