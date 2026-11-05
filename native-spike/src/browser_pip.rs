//! Picture in picture (ADR-111), after `browser.rs` `open_pip`: the active tab's web view moves
//! into an always-on-top, resizable panel at the screen's lower right, on every Space; closing
//! the panel or the pane's button puts it back. Everything runs on the main thread.
use crate::browser_tab::{Events, TabEvent};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSFloatingWindowLevel, NSPanel, NSScreen, NSWindowCollectionBehavior,
    NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use objc2_web_kit::WKWebView;

/// The floating window's size when it opens.
const SIZE: (f64, f64) = (480., 320.);
const MIN_SIZE: (f64, f64) = (240., 160.);
const MARGIN: f64 = 24.;

pub struct Ivars {
    tab_id: String,
    events: Events,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct PipDelegate;

    unsafe impl NSObjectProtocol for PipDelegate {}

    unsafe impl NSWindowDelegate for PipDelegate {
        /// The person closed the floating window: the pane puts the tab back, when it next runs.
        #[unsafe(method(windowWillClose:))]
        fn will_close(&self, _notification: &NSNotification) {
            let ivars = self.ivars();
            ivars.events.push(&ivars.tab_id, TabEvent::PipClosed);
        }
    }
);

/// The floating panel holding one tab's web view.
pub struct Pip {
    panel: Retained<NSPanel>,
    pub tab_id: String,
    _delegate: Retained<PipDelegate>,
}

/// The corner of the visible screen a window of `size` opens at.
pub fn lower_right(visible: (f64, f64, f64, f64), size: (f64, f64)) -> (f64, f64) {
    (visible.0 + visible.2 - size.0 - MARGIN, visible.1 + MARGIN)
}

impl Pip {
    /// Moves the web view into a new floating panel.
    pub fn open(
        web: &WKWebView,
        tab_id: &str,
        title: &str,
        events: Events,
        mtm: MainThreadMarker,
    ) -> Self {
        let origin = NSScreen::mainScreen(mtm)
            .map(|screen| {
                let frame = screen.visibleFrame();
                lower_right(
                    (
                        frame.origin.x,
                        frame.origin.y,
                        frame.size.width,
                        frame.size.height,
                    ),
                    SIZE,
                )
            })
            .unwrap_or((80., 80.));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            mtm.alloc::<NSPanel>(),
            NSRect::new(NSPoint::new(origin.0, origin.1), NSSize::new(SIZE.0, SIZE.1)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::UtilityWindow
                | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setFloatingPanel(true);
        panel.setLevel(NSFloatingWindowLevel);
        panel.setHidesOnDeactivate(false);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setContentMinSize(NSSize::new(MIN_SIZE.0, MIN_SIZE.1));
        panel.setTitle(&NSString::from_str(title));
        let partial = mtm.alloc::<PipDelegate>().set_ivars(Ivars {
            tab_id: tab_id.to_owned(),
            events,
        });
        let delegate: Retained<PipDelegate> = unsafe { msg_send![super(partial), init] };
        panel.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        web.removeFromSuperview();
        web.setHidden(false);
        panel.setContentView(Some(web));
        panel.orderFrontRegardless();
        Self {
            panel,
            tab_id: tab_id.to_owned(),
            _delegate: delegate,
        }
    }

    pub fn set_title(&self, title: &str) {
        self.panel.setTitle(&NSString::from_str(title));
    }

    /// Closes the panel without its delegate hearing about it; the web view is free afterwards.
    pub fn dismiss(&self) {
        self.panel.setDelegate(None);
        self.panel.setContentView(None);
        self.panel.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_opens_at_the_lower_right_of_the_visible_screen() {
        // A 1440x900 screen with a 25 px menu bar on top and a 70 px Dock below.
        assert_eq!(
            lower_right((0., 70., 1440., 805.), SIZE),
            (1440. - 480. - 24., 70. + 24.)
        );
    }
}
