//! Gives memory back while Sirus sits in the background: after five minutes without the window
//! in front, the decoded images (attachments, tool screenshots, avatars) leave GPUI's cache and
//! the GPU atlas, and the allocator returns its free pages to macOS. Whatever is still on screen
//! decodes again when the window comes back. Once per stretch in the background.

use std::time::{Duration, Instant};

use gpui::{AnyWindowHandle, App};

const CHECK: Duration = Duration::from_secs(30);
const AFTER: Duration = Duration::from_secs(300);

unsafe extern "C" {
    fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
}

/// Whether to trim now: in the background long enough, and not already trimmed this time.
pub(crate) fn due(background_for: Option<Duration>, trimmed: bool) -> bool {
    !trimmed && background_for.is_some_and(|d| d >= AFTER)
}

pub(crate) fn start(window: AnyWindowHandle, cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut since: Option<Instant> = None;
        let mut trimmed = false;
        loop {
            cx.background_executor().timer(CHECK).await;
            let Ok(active) = window.update(cx, |_, window, _| window.is_window_active()) else {
                return;
            };
            if active {
                since = None;
                trimmed = false;
                continue;
            }
            let began = *since.get_or_insert_with(Instant::now);
            if due(Some(began.elapsed()), trimmed) {
                trimmed = true;
                let _ = cx.update(|cx| cx.trim_images());
                // SAFETY: a null zone asks every malloc zone; goal 0 frees as much as it can.
                unsafe {
                    malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
                }
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_once_after_five_minutes_away() {
        assert!(!due(None, false));
        assert!(!due(Some(Duration::from_secs(120)), false));
        assert!(due(Some(Duration::from_secs(301)), false));
        assert!(!due(Some(Duration::from_secs(900)), true));
    }
}
