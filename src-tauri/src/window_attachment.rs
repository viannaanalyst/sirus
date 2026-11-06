//! One explicit OS-selected window becomes a bounded private image attachment.
//! No renderer-selected window/PID, screen stream, app enumeration or control.
use crate::error::{Error, Result};

#[cfg(test)]
type CaptureResult = Result<Option<Vec<u8>>>;
#[cfg(test)]
struct Completion {
    sender: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<CaptureResult>>>,
    selected: std::sync::atomic::AtomicBool,
}
#[cfg(test)]
impl Completion {
    fn begin_capture(&self) -> bool {
        self.sender.lock().is_some()
            && !self
                .selected
                .swap(true, std::sync::atomic::Ordering::AcqRel)
    }
    fn finish(&self, result: Result<Option<Vec<u8>>>) -> bool {
        if let Some(sender) = self.sender.lock().take() {
            let _ = sender.send(result);
            true
        } else {
            false
        }
    }
}

pub async fn pick(_app: &crate::runtime::AppHandle) -> Result<Option<Vec<u8>>> {
    Err(Error::new(
        "unsupported",
        "Window attachments are available on macOS only.",
    ))
}

#[cfg(test)]
fn valid_dimensions(width: f64, height: f64) -> bool {
    width.is_finite()
        && height.is_finite()
        && width > 0.0
        && height > 0.0
        && width <= 32768.0
        && height <= 32768.0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn window_sizes_must_be_finite_positive_and_bounded() {
        assert!(valid_dimensions(1920.0, 1080.0));
        assert!(!valid_dimensions(f64::INFINITY, 2.0));
        assert!(!valid_dimensions(2.0, f64::NAN));
        assert!(!valid_dimensions(-1.0, 2.0));
        assert!(!valid_dimensions(40000.0, 2.0));
    }

    #[test]
    fn cancel_or_timeout_rejects_late_capture_and_completes_once() {
        for result in [Ok(None), Err(Error::new("attachment", "Timed out"))] {
            let (sender, mut receiver) = tokio::sync::oneshot::channel();
            let completion = Completion {
                sender: parking_lot::Mutex::new(Some(sender)),
                selected: std::sync::atomic::AtomicBool::new(false),
            };
            let canceled = result.is_ok();
            assert!(completion.finish(result));
            assert!(!completion.begin_capture());
            assert!(!completion.finish(Ok(Some(vec![1, 2]))));
            assert_eq!(receiver.try_recv().unwrap().is_ok(), canceled);
        }
    }

    #[test]
    fn selected_window_is_captured_at_most_once() {
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        let completion = Completion {
            sender: parking_lot::Mutex::new(Some(sender)),
            selected: std::sync::atomic::AtomicBool::new(false),
        };
        assert!(completion.begin_capture());
        assert!(!completion.begin_capture());
        assert!(completion.finish(Ok(Some(vec![1]))));
        assert!(!completion.finish(Ok(None)));
        assert_eq!(receiver.try_recv().unwrap().unwrap(), Some(vec![1]));
    }
}
