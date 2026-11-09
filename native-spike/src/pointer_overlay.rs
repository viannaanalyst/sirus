//! The agent's pointer over other Mac apps (ADR-198). The host has no AppKit main thread
//! (ADR-189), so for each computer-use action that clicks, types, presses a key or scrolls it
//! sends `computer-pointer` (kind, global screen point, provider, the target window's frame) and
//! this window app shows a transparent, click-through, borderless window over that screen, above
//! normal windows, drawing `agent_pointer::element`. It closes a few seconds after the last
//! action, or at once on `kind: "hide"` (stop, Escape, computer use turned off).
use crate::agent_pointer::{self, Kind, Pointer, Rect};
use crate::backend::Backend;
use gpui::{
    App, AsyncApp, Bounds, Context, DisplayId, Task, Window, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, point, prelude::*, px, size,
};
use serde_json::Value;
use std::{cell::RefCell, time::Duration, time::Instant};

thread_local! {
    static OPEN: RefCell<Option<(WindowHandle<Overlay>, Rect)>> = const { RefCell::new(None) };
}

pub fn start(backend: &Backend, cx: &mut App) {
    let events = backend.subscribe("computer-pointer");
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(event) = events.recv().await {
            let _ = cx.update(|cx| handle(&event, cx));
        }
    })
    .detach();
}

/// One host event.
fn handle(event: &Value, cx: &mut App) {
    let kind = event["kind"].as_str().unwrap_or_default();
    if kind == "hide" {
        close(cx);
        return;
    }
    let Some(kind) = Kind::parse(kind, event["dy"].as_f64().unwrap_or(0.) >= 0.) else {
        return;
    };
    let provider = event["provider"].as_str().unwrap_or_default().to_owned();
    let target = match (event["x"].as_f64(), event["y"].as_f64()) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some((x, y)),
        _ => None,
    };
    let frame = rect(&event["frame"]);
    let open = OPEN.with(|o| o.borrow().clone());
    let Some(anchor) = target
        .or(frame.map(|f| f.center()))
        .or(open.as_ref().map(|(_, screen)| screen.center()))
    else {
        return;
    };
    let screens = displays();
    let rects: Vec<Rect> = screens.iter().map(|(_, r)| *r).collect();
    let Some(index) = agent_pointer::screen_for(anchor, &rects) else {
        return;
    };
    let (display, screen) = screens[index];
    let reduced = crate::prefs::reduce_motion();
    let local = target.map(|p| agent_pointer::local_point(p, screen));
    if let Some((handle, current)) = open {
        if current == screen {
            let updated = handle.update(cx, |overlay, _, cx| {
                overlay.act(&provider, local, kind, reduced, cx);
            });
            if updated.is_ok() {
                return;
            }
        }
        close(cx);
    }
    let at = local.unwrap_or_else(|| agent_pointer::local_point(anchor, screen));
    open_on(display, screen, &provider, at, kind, reduced, cx);
}

fn rect(value: &Value) -> Option<Rect> {
    let n = |key: &str| value[key].as_f64().filter(|v| v.is_finite());
    let r = Rect {
        x: n("x")?,
        y: n("y")?,
        width: n("width")?,
        height: n("height")?,
    };
    (r.width > 0. && r.height > 0.).then_some(r)
}

fn close(cx: &mut App) {
    if let Some((handle, _)) = OPEN.with(|o| o.borrow_mut().take()) {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

pub struct Overlay {
    pointer: Pointer,
    epoch: Instant,
    _hide: Task<()>,
}

impl Overlay {
    fn now(&self) -> f32 {
        self.epoch.elapsed().as_secs_f32()
    }

    fn act(
        &mut self,
        provider: &str,
        at: Option<(f32, f32)>,
        kind: Kind,
        reduced: bool,
        cx: &mut Context<Self>,
    ) {
        let now = self.now();
        self.pointer.set_reduced(reduced);
        self.pointer.aim(provider, at, kind, now);
        self.schedule_hide(cx);
        cx.notify();
    }

    /// Closes the window once the pointer has faded out; a later action replaces this.
    fn schedule_hide(&mut self, cx: &mut Context<Self>) {
        let wait = self.pointer.remaining(self.now()) + 0.05;
        self._hide = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f32(wait))
                .await;
            let gone = this
                .update(cx, |overlay, _| !overlay.pointer.visible(overlay.now()))
                .unwrap_or(true);
            if gone {
                let _ = cx.update(close);
            }
        });
    }
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let now = self.now();
        if self.pointer.animating(now) {
            window.request_animation_frame();
        }
        div().size_full().child(agent_pointer::element(
            &self.pointer,
            now,
            crate::theme::accent(),
        ))
    }
}

fn open_on(
    display: u32,
    screen: Rect,
    provider: &str,
    at: (f32, f32),
    kind: Kind,
    reduced: bool,
    cx: &mut App,
) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(screen.width as f32), px(screen.height as f32)),
        })),
        titlebar: None,
        kind: WindowKind::PopUp,
        focus: false,
        show: true,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: gpui::WindowBackgroundAppearance::Transparent,
        display_id: Some(DisplayId::new(u64::from(display))),
        ..Default::default()
    };
    let provider = provider.to_owned();
    let opened = cx.open_window(options, |window, cx| {
        click_through(window, screen);
        cx.new(|cx| {
            let mut overlay = Overlay {
                pointer: Pointer::new(&provider, at, kind, 0., reduced),
                epoch: Instant::now(),
                _hide: Task::ready(()),
            };
            overlay.schedule_hide(cx);
            overlay
        })
    });
    if let Ok(handle) = opened {
        OPEN.with(|o| *o.borrow_mut() = Some((handle, screen)));
    }
}

/// The AppKit frame (origin bottom-left of the main display, y up) of a global rectangle (origin
/// top-left, y down), given the main display's height.
pub fn appkit_frame(rect: Rect, main_height: f64) -> Rect {
    Rect {
        y: main_height - (rect.y + rect.height),
        ..rect
    }
}

/// Makes the window a borderless, shadowless, click-through layer over the whole screen, on
/// every Space, above normal windows (GPUI's pop-up level).
#[cfg(target_os = "macos")]
fn click_through(window: &Window, screen: Rect) {
    use objc2_app_kit::{NSWindowCollectionBehavior, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    let Some(view) = crate::panel_browser::parent_of(window) else {
        return;
    };
    let Some(ns_window) = view.window() else {
        return;
    };
    let main_height = displays()
        .iter()
        .find(|(_, r)| r.x == 0. && r.y == 0.)
        .map_or(screen.height, |(_, r)| r.height);
    let frame = appkit_frame(screen, main_height);
    ns_window.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
    ns_window.setIgnoresMouseEvents(true);
    ns_window.setHasShadow(false);
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    ns_window.setFrame_display(
        NSRect::new(
            NSPoint::new(frame.x, frame.y),
            NSSize::new(frame.width, frame.height),
        ),
        true,
    );
}

#[cfg(not(target_os = "macos"))]
fn click_through(_: &Window, _: Rect) {}

/// The active displays and their global rectangles.
#[cfg(target_os = "macos")]
fn displays() -> Vec<(u32, Rect)> {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGDisplayBounds(display: u32) -> CGRect;
    }
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    // SAFETY: the buffer holds `max` ids and CoreGraphics writes at most that many.
    if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) } != 0 {
        return Vec::new();
    }
    ids[..count.min(16) as usize]
        .iter()
        .map(|&id| {
            // SAFETY: a plain query of an active display id.
            let b = unsafe { CGDisplayBounds(id) };
            (
                id,
                Rect {
                    x: b.x,
                    y: b.y,
                    width: b.width,
                    height: b.height,
                },
            )
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn displays() -> Vec<(u32, Rect)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn global_rects_flip_into_appkit_frames() {
        let main = Rect {
            x: 0.,
            y: 0.,
            width: 1512.,
            height: 982.,
        };
        assert_eq!(appkit_frame(main, 982.), main);
        // A display above the main one: AppKit puts it at positive y.
        let above = Rect {
            x: 0.,
            y: -1080.,
            width: 1920.,
            height: 1080.,
        };
        assert_eq!(appkit_frame(above, 982.).y, 982.);
    }

    #[test]
    fn frames_need_numbers_and_an_area() {
        assert!(rect(&json!({ "x": 1, "y": 2, "width": 3, "height": 4 })).is_some());
        assert!(rect(&json!({ "x": 1, "y": 2, "width": 0, "height": 4 })).is_none());
        assert!(rect(&json!(null)).is_none());
    }
}
