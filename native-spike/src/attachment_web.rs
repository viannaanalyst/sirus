//! Native views the attachment viewer lays over its body on macOS: a WKWebView for converted
//! Word, PowerPoint and HTML pages (text selection, copy, links, pinch zoom come from WebKit) and
//! a PDFKit `PDFView` for PDFs (selection, copy, pinch and wheel zoom, page tracking). The
//! technique is the browser pane's (`panel_browser.rs`): the view is a subview of the window's
//! content view, placed over the body's bounds each frame and hidden while anything of GPUI's
//! (a dialog, the fade of the viewer) must show above it.
//!
//! What the views report goes through a channel (`Event`); a local event monitor forwards the keys
//! and wheel gestures GPUI would not see while the view has the focus (Esc, ⌘+ ⌘- ⌘0, ⌘ wheel)
//! and does ⌘C and ⌘A through the responder chain.
use crate::panel_browser::parent_of;
use async_channel::{Receiver, Sender};
use block2::RcBlock;
use gpui::{Bounds, Pixels, Window};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSView, NSWindowOrderingMode};
use objc2_foundation::{NSData, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationType,
    WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKWebView,
    WKWebViewConfiguration, WKWebsiteDataStore,
};
use std::cell::{Cell, RefCell};

#[link(name = "PDFKit", kind = "framework")]
unsafe extern "C" {}

/// What a native view tells the viewer.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// The slide the page shows now (0-based).
    Slide(usize),
    /// The PDF page at the top (0-based).
    Page(usize),
    /// The PDF's scale changed.
    Scale,
    Escape,
    /// ⌘+ (1), ⌘- (-1) or ⌘0 (0), or the ⌘ wheel.
    Zoom(i8),
    /// A link the person clicked: the app opens it in the browser.
    Link(String),
}

pub struct BridgeIvars {
    events: Sender<Event>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = BridgeIvars]
    struct Bridge;

    unsafe impl NSObjectProtocol for Bridge {}

    unsafe impl WKScriptMessageHandler for Bridge {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            let body = unsafe { message.body() };
            if let Some(text) = body.downcast_ref::<NSString>()
                && let Ok(slide) = text.to_string().parse::<usize>()
            {
                let _ = self.ivars().events.try_send(Event::Slide(slide));
            }
        }
    }

    unsafe impl WKNavigationDelegate for Bridge {
        /// The page itself loads from a string; anything else is a link, which opens outside.
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide(
            &self,
            _web: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::Block<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let url = unsafe {
                action
                    .request()
                    .URL()
                    .and_then(|u| u.absoluteString())
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            };
            let policy = if url.starts_with("about:") {
                WKNavigationActionPolicy::Allow
            } else {
                if unsafe { action.navigationType() } == WKNavigationType::LinkActivated {
                    let _ = self.ivars().events.try_send(Event::Link(url));
                }
                WKNavigationActionPolicy::Cancel
            };
            handler.call((policy,));
        }
    }
);

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Web,
    Pdf,
}

pub struct Overlay {
    view: Retained<NSView>,
    kind: Kind,
    parent: RefCell<Option<Retained<NSView>>>,
    pub events: Receiver<Event>,
    // Kept alive for as long as the view: the script handler and the observers.
    _bridge: Option<Retained<Bridge>>,
    monitor: Option<Retained<AnyObject>>,
    observers: Vec<Retained<AnyObject>>,
    zoom: Cell<f64>,
    shown: Cell<bool>,
}

fn frame_rect() -> NSRect {
    NSRect::new(NSPoint::new(0., 0.), NSSize::new(640., 480.))
}

impl Overlay {
    /// A page of `html` (see `attachment_html`). `scripts` lets the page's own script run: only
    /// the presentation's, which carries a nonce the page's policy checks.
    pub fn web(window: &Window, html: &str) -> Option<Overlay> {
        let mtm = MainThreadMarker::new()?;
        let (tx, rx) = async_channel::unbounded();
        unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            // Nothing the page does is kept: no cookies, caches or storage.
            config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            let partial = mtm.alloc::<Bridge>().set_ivars(BridgeIvars { events: tx.clone() });
            let bridge: Retained<Bridge> = msg_send![super(partial), init];
            config.userContentController().addScriptMessageHandler_name(
                ProtocolObject::from_ref(&*bridge),
                &NSString::from_str("sirus"),
            );
            let web = WKWebView::initWithFrame_configuration(mtm.alloc::<WKWebView>(), frame_rect(), &config);
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*bridge)));
            web.setAllowsMagnification(true);
            web.setHidden(true);
            web.loadHTMLString_baseURL(&NSString::from_str(html), None);
            let view: Retained<NSView> = Retained::cast_unchecked(web);
            let mut overlay = Overlay {
                view,
                kind: Kind::Web,
                parent: RefCell::new(parent_of(window)),
                events: rx,
                _bridge: Some(bridge),
                monitor: None,
                observers: Vec::new(),
                zoom: Cell::new(1.),
                shown: Cell::new(false),
            };
            overlay.monitor = install_monitor(&overlay.view, tx);
            Some(overlay)
        }
    }

    /// A video file in the web view's own player. Only a file the host authorized comes here
    /// (`reply_rich::video_file`); no page of ours is involved, so the view has no script bridge
    /// and no navigation delegate, and keeps nothing.
    pub fn file(window: &Window, path: &std::path::Path) -> Option<Overlay> {
        let mtm = MainThreadMarker::new()?;
        let (tx, rx) = async_channel::unbounded();
        let (file, folder) = (path.to_str()?, path.parent()?.to_str()?);
        unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            let web = WKWebView::initWithFrame_configuration(mtm.alloc::<WKWebView>(), frame_rect(), &config);
            web.setHidden(true);
            let url = NSURL::fileURLWithPath(&NSString::from_str(file));
            let root = NSURL::fileURLWithPath(&NSString::from_str(folder));
            let _ = web.loadFileURL_allowingReadAccessToURL(&url, &root);
            let view: Retained<NSView> = Retained::cast_unchecked(web);
            let mut overlay = Overlay {
                view,
                kind: Kind::Web,
                parent: RefCell::new(parent_of(window)),
                events: rx,
                _bridge: None,
                monitor: None,
                observers: Vec::new(),
                zoom: Cell::new(1.),
                shown: Cell::new(false),
            };
            overlay.monitor = install_monitor(&overlay.view, tx);
            Some(overlay)
        }
    }

    /// A PDF in PDFKit's own view.
    pub fn pdf(window: &Window, bytes: &[u8]) -> Option<Overlay> {
        MainThreadMarker::new()?;
        let (tx, rx) = async_channel::unbounded();
        unsafe {
            let (view_class, document_class) =
                (AnyClass::get(c"PDFView")?, AnyClass::get(c"PDFDocument")?);
            let data = NSData::with_bytes(bytes);
            let allocated: Allocated<AnyObject> = msg_send![document_class, alloc];
            let document: Option<Retained<AnyObject>> = msg_send![allocated, initWithData: &*data];
            let document = document?;
            let allocated: Allocated<NSView> = msg_send![view_class, alloc];
            let view: Retained<NSView> = msg_send![allocated, initWithFrame: frame_rect()];
            let _: () = msg_send![&*view, setDocument: &*document];
            // Single column, scrolling continuously, fitted to the width until the person zooms.
            let _: () = msg_send![&*view, setDisplayMode: 1isize];
            let _: () = msg_send![&*view, setAutoScales: true];
            let _: () = msg_send![&*view, setDisplaysPageBreaks: true];
            let _: () = msg_send![&*view, setMinScaleFactor: 0.25f64];
            let _: () = msg_send![&*view, setMaxScaleFactor: 6.0f64];
            view.setHidden(true);
            let mut observers = Vec::new();
            for (name, make) in [
                ("PDFViewPageChangedNotification", 0u8),
                ("PDFViewScaleChangedNotification", 1u8),
            ] {
                let sender = tx.clone();
                let target = Retained::as_ptr(&view) as usize;
                let block = RcBlock::new(move |_note: *mut AnyObject| {
                    let event = if make == 0 {
                        // SAFETY: the observer is removed before the view goes.
                        current_page(&*(target as *const NSView)).map(Event::Page)
                    } else {
                        Some(Event::Scale)
                    };
                    if let Some(event) = event {
                        let _ = sender.try_send(event);
                    }
                });
                let center: Retained<AnyObject> = msg_send![AnyClass::get(c"NSNotificationCenter")?, defaultCenter];
                let token: Retained<AnyObject> = msg_send![
                    &*center,
                    addObserverForName: &*NSString::from_str(name),
                    object: &*view,
                    queue: std::ptr::null::<AnyObject>(),
                    usingBlock: &*block
                ];
                observers.push(token);
            }
            let mut overlay = Overlay {
                view,
                kind: Kind::Pdf,
                parent: RefCell::new(parent_of(window)),
                events: rx,
                _bridge: None,
                monitor: None,
                observers,
                zoom: Cell::new(1.),
                shown: Cell::new(false),
            };
            overlay.monitor = install_monitor(&overlay.view, tx);
            Some(overlay)
        }
    }

    /// Puts the view over `bounds` (GPUI's pixels of the window) or hides it.
    pub fn place(&self, bounds: Bounds<Pixels>, window: &Window, visible: bool) {
        if self.parent.borrow().is_none() {
            *self.parent.borrow_mut() = parent_of(window);
        }
        let Some(parent) = self.parent.borrow().clone() else {
            return;
        };
        let zoom = window.content_zoom();
        let bounds = bounds.map(|v| v * zoom);
        let (w, h) = (f64::from(bounds.size.width), f64::from(bounds.size.height));
        if !visible || w <= 1. || h <= 1. {
            self.set_hidden(true);
            return;
        }
        let (x, y) = (f64::from(bounds.origin.x), f64::from(bounds.origin.y));
        let y = if parent.isFlipped() {
            y
        } else {
            parent.bounds().size.height - y - h
        };
        let on_top = parent
            .subviews()
            .lastObject()
            .is_some_and(|last| std::ptr::eq(&*last, &*self.view));
        if !on_top {
            parent.addSubview_positioned_relativeTo(&self.view, NSWindowOrderingMode::Above, None);
        }
        self.view.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)));
        self.set_hidden(false);
    }

    pub fn hide(&self) {
        self.set_hidden(true);
    }

    fn set_hidden(&self, hidden: bool) {
        if self.shown.get() == hidden {
            self.view.setHidden(hidden);
            self.shown.set(!hidden);
        }
    }

    /// Runs a script in the page (web views only).
    pub fn eval(&self, script: &str) {
        if self.kind == Kind::Web {
            let web: &WKWebView = unsafe { &*(Retained::as_ptr(&self.view) as *const WKWebView) };
            unsafe {
                web.evaluateJavaScript_completionHandler(&NSString::from_str(script), None)
            };
        }
    }

    /// Zooms by one step (`1`, `-1`) or back to fit (`0`).
    pub fn zoom(&self, by: i8) {
        match self.kind {
            Kind::Pdf => unsafe {
                match by {
                    0 => {
                        let _: () = msg_send![&*self.view, setAutoScales: true];
                    }
                    b if b > 0 => {
                        let _: () = msg_send![&*self.view, zoomIn: std::ptr::null::<AnyObject>()];
                    }
                    _ => {
                        let _: () = msg_send![&*self.view, zoomOut: std::ptr::null::<AnyObject>()];
                    }
                }
            },
            Kind::Web => {
                let next = match by {
                    0 => 1.,
                    b if b > 0 => (self.zoom.get() + 0.1).min(3.),
                    _ => (self.zoom.get() - 0.1).max(0.4),
                };
                self.zoom.set(next);
                let web: &WKWebView = unsafe { &*(Retained::as_ptr(&self.view) as *const WKWebView) };
                unsafe { web.setPageZoom(next) };
            }
        }
    }

    /// The PDF's scale as a percentage of the fit-to-width scale.
    pub fn percent(&self) -> u32 {
        match self.kind {
            Kind::Pdf => unsafe {
                let scale: f64 = msg_send![&*self.view, scaleFactor];
                let fit: f64 = msg_send![&*self.view, scaleFactorForSizeToFit];
                if fit > 0. { (scale / fit * 100.).round() as u32 } else { 100 }
            },
            Kind::Web => (self.zoom.get() * 100.).round() as u32,
        }
    }

    /// Scrolls the PDF to a page (0-based).
    pub fn go_to_page(&self, index: usize) {
        unsafe {
            let document: Option<Retained<AnyObject>> = msg_send![&*self.view, document];
            let Some(document) = document else { return };
            let page: Option<Retained<AnyObject>> = msg_send![&*document, pageAtIndex: index];
            if let Some(page) = page {
                let _: () = msg_send![&*self.view, goToPage: &*page];
            }
        }
    }
}

fn current_page(view: &NSView) -> Option<usize> {
    unsafe {
        let document: Option<Retained<AnyObject>> = msg_send![view, document];
        let page: Option<Retained<AnyObject>> = msg_send![view, currentPage];
        let index: usize = msg_send![&*document?, indexForPage: &*page?];
        Some(index)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            if let Some(monitor) = self.monitor.take()
                && let Some(event) = AnyClass::get(c"NSEvent")
            {
                let _: () = msg_send![event, removeMonitor: &*monitor];
            }
            if let Some(center) = AnyClass::get(c"NSNotificationCenter") {
                let center: Retained<AnyObject> = msg_send![center, defaultCenter];
                for token in self.observers.drain(..) {
                    let _: () = msg_send![&*center, removeObserver: &*token];
                }
            }
            if self.kind == Kind::Web {
                let web: &WKWebView = &*(Retained::as_ptr(&self.view) as *const WKWebView);
                web.setNavigationDelegate(None);
                web.configuration()
                    .userContentController()
                    .removeScriptMessageHandlerForName(&NSString::from_str("sirus"));
            }
        }
        self.view.removeFromSuperview();
    }
}

/// Watches keys and the ⌘ wheel while the view has the focus: GPUI's view does not get them then.
unsafe fn install_monitor(view: &Retained<NSView>, tx: Sender<Event>) -> Option<Retained<AnyObject>> {
    let target = Retained::as_ptr(view) as usize;
    let block = RcBlock::new(move |event: *mut AnyObject| -> *mut AnyObject {
        // SAFETY: the monitor is removed (Drop) before the view is released.
        let view = unsafe { &*(target as *const NSView) };
        let event_ref = unsafe { &*event };
        let Some(window) = view.window() else {
            return event;
        };
        let event_window: *const AnyObject = unsafe { msg_send![event_ref, window] };
        if !std::ptr::eq(event_window, Retained::as_ptr(&window).cast()) || view.isHidden() {
            return event;
        }
        let kind: usize = unsafe { msg_send![event_ref, type] };
        let flags: usize = unsafe { msg_send![event_ref, modifierFlags] };
        let command = flags & (1 << 20) != 0;
        // Keys go to the view only while it holds the focus.
        let in_focus = || unsafe {
            let responder: Option<Retained<AnyObject>> = msg_send![&*window, firstResponder];
            responder.is_some_and(|r| {
                let is_view: bool = msg_send![&*r, isKindOfClass: AnyClass::get(c"NSView").unwrap()];
                is_view && msg_send![&*r, isDescendantOf: view]
            })
        };
        let consume = |what: Option<Event>| -> *mut AnyObject {
            if let Some(event) = what {
                let _ = tx.try_send(event);
            }
            std::ptr::null_mut()
        };
        match kind {
            // NSEventTypeKeyDown
            10 if in_focus() => {
                let code: u16 = unsafe { msg_send![event_ref, keyCode] };
                if code == 53 {
                    return consume(Some(Event::Escape));
                }
                if !command {
                    return event;
                }
                let chars: Option<Retained<NSString>> =
                    unsafe { msg_send![event_ref, charactersIgnoringModifiers] };
                match chars.map(|c| c.to_string()).as_deref() {
                    Some("=" | "+") => consume(Some(Event::Zoom(1))),
                    Some("-") => consume(Some(Event::Zoom(-1))),
                    Some("0") => consume(Some(Event::Zoom(0))),
                    Some(key @ ("c" | "a")) => {
                        let app = NSApplication::sharedApplication(unsafe { MainThreadMarker::new_unchecked() });
                        let action = if key == "c" { sel!(copy:) } else { sel!(selectAll:) };
                        let _: bool = unsafe {
                            msg_send![&*app, sendAction: action, to: std::ptr::null::<AnyObject>(), from: std::ptr::null::<AnyObject>()]
                        };
                        std::ptr::null_mut()
                    }
                    _ => event,
                }
            }
            // NSEventTypeScrollWheel with ⌘ over the view
            22 if command => {
                let at: NSPoint = unsafe { msg_send![event_ref, locationInWindow] };
                let local = view.convertPoint_fromView(at, None);
                let b = view.bounds();
                if local.x < b.origin.x
                    || local.y < b.origin.y
                    || local.x > b.origin.x + b.size.width
                    || local.y > b.origin.y + b.size.height
                {
                    return event;
                }
                let delta: f64 = unsafe { msg_send![event_ref, scrollingDeltaY] };
                if delta.abs() < 0.5 {
                    return std::ptr::null_mut();
                }
                consume(Some(Event::Zoom(if delta > 0. { 1 } else { -1 })))
            }
            _ => event,
        }
    });
    let monitor: Option<Retained<AnyObject>> = msg_send![
        AnyClass::get(c"NSEvent")?,
        addLocalMonitorForEventsMatchingMask: (1usize << 10) | (1usize << 22),
        handler: &*block
    ];
    monitor
}
