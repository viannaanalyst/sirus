//! One tab of the browser pane: a WKWebView with the delegate that reports what the page does
//! (dialogs, popups, sign-in forms, load errors, crashes), and the page calls the agent tools
//! and the pane use. Everything here runs on the main thread. Ported from `src-tauri/src/browser.rs`
//! and `browser_agent.rs` (the same scripts, the same delegate rules).
use crate::browser_js;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, ProtocolObject};
use objc2::{
    AllocAnyThread, DeclaredClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
use objc2_foundation::{
    NSDictionary, NSError, NSMutableDictionary, NSNumber, NSObjectProtocol, NSString, NSURL,
    NSURLRequest,
};
use objc2_web_kit::{
    WKContentWorld, WKFrameInfo, WKNavigation, WKNavigationAction, WKNavigationActionPolicy,
    WKNavigationDelegate, WKScriptMessage, WKScriptMessageHandler, WKUIDelegate,
    WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebView,
    WKWebViewConfiguration, WKWebsiteDataStore, WKWindowFeatures,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    ffi::c_void,
    rc::Rc,
};

/// What a page asked of the pane; the pane drains these when woken.
pub enum TabEvent {
    /// `target=_blank` and OAuth popups become tabs; the popup itself is cancelled.
    Popup(String),
    Dialog(PendingDialog),
    /// A submitted password form: the page's own address and the capture script's JSON.
    Login {
        frame_url: String,
        body: String,
    },
    /// The WebContent process died.
    Crashed,
    /// The person closed the picture-in-picture window.
    PipClosed,
}

/// What the tabs report to the pane. Callbacks (delegate methods, key-value observers) run on the
/// main thread outside GPUI: they queue the event and wake the pane's task, so nothing polls.
pub struct Queue {
    items: RefCell<VecDeque<(String, TabEvent)>>,
    /// A tab's address, title, loading or history state changed since the pane last looked.
    dirty: Cell<bool>,
    wake: async_channel::Sender<()>,
}

pub type Events = Rc<Queue>;

impl Queue {
    pub fn new() -> (Events, async_channel::Receiver<()>) {
        let (wake, woken) = async_channel::bounded(1);
        (
            Rc::new(Self {
                items: RefCell::new(VecDeque::new()),
                dirty: Cell::new(false),
                wake,
            }),
            woken,
        )
    }

    pub fn push(&self, tab_id: &str, event: TabEvent) {
        self.items
            .borrow_mut()
            .push_back((tab_id.to_owned(), event));
        let _ = self.wake.try_send(());
    }

    /// Notes that some tab's state changed.
    pub fn touch(&self) {
        self.dirty.set(true);
        let _ = self.wake.try_send(());
    }

    /// The queued events, and whether a tab's state changed.
    pub fn take(&self) -> (Vec<(String, TabEvent)>, bool) {
        (
            self.items.borrow_mut().drain(..).collect(),
            self.dirty.replace(false),
        )
    }
}

pub enum DialogReply {
    Alert(RcBlock<dyn Fn()>),
    Confirm(RcBlock<dyn Fn(Bool)>),
    Prompt(RcBlock<dyn Fn(*mut NSString)>, String),
}

/// A JavaScript alert, confirm or prompt waiting for an answer. Dropping it dismisses it, so
/// a page is never left frozen.
pub struct PendingDialog {
    pub tab_id: String,
    pub kind: &'static str,
    pub message: String,
    pub default_text: Option<String>,
    pub url: String,
    reply: Option<DialogReply>,
}

impl PendingDialog {
    pub fn answer(mut self, accept: bool, text: Option<&str>) {
        if let Some(reply) = self.reply.take() {
            call_reply(reply, accept, text);
        }
    }
}

impl Drop for PendingDialog {
    fn drop(&mut self) {
        if let Some(reply) = self.reply.take() {
            call_reply(reply, false, None);
        }
    }
}

fn call_reply(reply: DialogReply, accept: bool, text: Option<&str>) {
    match reply {
        DialogReply::Alert(handler) => handler.call(()),
        DialogReply::Confirm(handler) => handler.call((Bool::new(accept),)),
        DialogReply::Prompt(handler, default) => {
            if accept {
                let value = NSString::from_str(text.unwrap_or(&default));
                handler.call((Retained::as_ptr(&value) as *mut NSString,));
            } else {
                handler.call((std::ptr::null_mut(),));
            }
        }
    }
}

pub fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// The address without its query or fragment, for what the agent is told about a dialog.
pub fn redact(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or_default().to_owned()
}

/// Top-level navigation allowlist (`browser.rs`): http(s) and the blank page only.
pub fn allowed_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower == "about:blank"
        || ((lower.starts_with("http://") || lower.starts_with("https://")) && lower.len() > 8)
}

fn ignorable_load_error(domain: &str, code: isize) -> bool {
    (domain == "NSURLErrorDomain" && code == -999) || (domain == "WebKitErrorDomain" && code == 102)
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page shown when an address does not load, with the address, the reason and a retry.
pub fn load_error_page(failed_url: &str, reason: &str, portuguese: bool) -> String {
    let (title, retry) = if portuguese {
        ("Não foi possível abrir a página", "Tentar de novo")
    } else {
        ("This page could not be opened", "Try again")
    };
    let url = escape_html(failed_url);
    let reason = escape_html(reason);
    let target = escape_html(
        &serde_json::to_string(failed_url)
            .unwrap_or_else(|_| "\"\"".into())
            .replace('\'', "\\u0027"),
    );
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>{title}</title><style>
:root{{color-scheme:light dark}}body{{margin:0;min-height:100vh;display:grid;place-items:center;font:14px/1.5 -apple-system,BlinkMacSystemFont,sans-serif;background:Canvas;color:CanvasText}}
main{{max-width:520px;padding:32px}}h1{{font-size:17px;margin:0 0 8px}}p{{margin:0 0 6px;opacity:.75}}code{{font:12.5px ui-monospace,Menlo,monospace;word-break:break-all;opacity:1}}
button{{margin-top:14px;padding:6px 14px;border-radius:7px;border:1px solid color-mix(in srgb,CanvasText 20%,transparent);background:transparent;color:inherit;font:inherit;cursor:pointer}}
</style></head><body><main><h1>{title}</h1><p><code>{url}</code></p><p>{reason}</p>
<button type="button" onclick='location.href={target}'>{retry}</button></main></body></html>"#
    )
}

pub struct Ivars {
    tab_id: String,
    events: Events,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub struct TabDelegate;

    unsafe impl NSObjectProtocol for TabDelegate {}

    unsafe impl WKNavigationDelegate for TabDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            _webview: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::Block<dyn Fn(objc2_web_kit::WKNavigationActionPolicy)>,
        ) {
            let allowed = unsafe {
                !action.shouldPerformDownload()
                    && action
                        .request()
                        .URL()
                        .and_then(|value| value.absoluteString())
                        .is_some_and(|value| allowed_url(&value.to_string()))
            };
            (*handler).call((if allowed {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(
            &self,
            webview: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            self.show_load_error(webview, error);
        }

        /// The WebContent process died (memory, a crash): the pane reloads, a few times at most.
        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn did_terminate(&self, _webview: &WKWebView) {
            self.push(TabEvent::Crashed);
        }
    }

    unsafe impl WKScriptMessageHandler for TabDelegate {
        /// A password form the person submitted (ADR-186). The site is the address the webview
        /// loaded for that frame, not anything the page says.
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive_script_message(
            &self,
            _controller: &WKUserContentController,
            message: &WKScriptMessage,
        ) {
            let (body, frame_url, main_frame) = unsafe {
                let frame = message.frameInfo();
                (
                    message
                        .body()
                        .downcast_ref::<NSString>()
                        .map(|text| text.to_string()),
                    frame
                        .request()
                        .URL()
                        .and_then(|url| url.absoluteString())
                        .map(|url| url.to_string()),
                    frame.isMainFrame(),
                )
            };
            let (Some(body), Some(frame_url), true) = (body, frame_url, main_frame) else {
                return;
            };
            if body.len() <= 4096 {
                self.push(TabEvent::Login { frame_url, body });
            }
        }
    }

    unsafe impl WKUIDelegate for TabDelegate {
        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_webview(
            &self,
            _webview: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            let url = unsafe {
                action
                    .request()
                    .URL()
                    .and_then(|value| value.absoluteString())
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            };
            if allowed_url(&url) {
                self.push(TabEvent::Popup(url));
            }
            None
        }

        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn run_alert(
            &self,
            webview: &WKWebView,
            message: &NSString,
            _frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            self.dialog(
                webview,
                "alert",
                message.to_string(),
                None,
                DialogReply::Alert(handler.copy()),
            );
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn run_confirm(
            &self,
            webview: &WKWebView,
            message: &NSString,
            _frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn(Bool)>,
        ) {
            self.dialog(
                webview,
                "confirm",
                message.to_string(),
                None,
                DialogReply::Confirm(handler.copy()),
            );
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        fn run_prompt(
            &self,
            webview: &WKWebView,
            prompt: &NSString,
            default_text: Option<&NSString>,
            _frame: &WKFrameInfo,
            handler: &block2::DynBlock<dyn Fn(*mut NSString)>,
        ) {
            let default = default_text
                .map(|text| text.to_string())
                .unwrap_or_default();
            self.dialog(
                webview,
                "prompt",
                prompt.to_string(),
                Some(truncate(&default, 500)),
                DialogReply::Prompt(handler.copy(), default),
            );
        }
    }
);

/// The properties of a web view the pane shows; each change wakes the pane (key-value observing).
const OBSERVED: [&str; 5] = ["URL", "title", "loading", "canGoBack", "canGoForward"];

pub struct ObserverIvars {
    events: Events,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ObserverIvars]
    pub struct TabObserver;

    unsafe impl NSObjectProtocol for TabObserver {}

    impl TabObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&AnyObject>,
            _context: *mut c_void,
        ) {
            self.ivars().events.touch();
        }
    }
);

impl TabObserver {
    /// Starts observing the web view. Pair with `detach` before the view is released: WebKit
    /// objects must not die while observed.
    fn attach(web: &WKWebView, events: Events, mtm: MainThreadMarker) -> Retained<Self> {
        let partial = mtm.alloc::<Self>().set_ivars(ObserverIvars { events });
        let observer: Retained<Self> = unsafe { msg_send![super(partial), init] };
        for key in OBSERVED {
            let key = NSString::from_str(key);
            unsafe {
                let _: () = msg_send![web, addObserver: &*observer, forKeyPath: &*key, options: 0usize, context: std::ptr::null_mut::<c_void>()];
            }
        }
        observer
    }

    pub fn detach(&self, web: &WKWebView) {
        for key in OBSERVED {
            let key = NSString::from_str(key);
            unsafe {
                let _: () = msg_send![web, removeObserver: self, forKeyPath: &*key];
            }
        }
    }
}

impl TabDelegate {
    fn new(tab_id: &str, events: Events, mtm: MainThreadMarker) -> Retained<Self> {
        let delegate = mtm.alloc::<Self>().set_ivars(Ivars {
            tab_id: tab_id.into(),
            events,
        });
        unsafe { msg_send![super(delegate), init] }
    }

    fn push(&self, event: TabEvent) {
        let ivars = self.ivars();
        ivars.events.push(&ivars.tab_id, event);
    }

    fn dialog(
        &self,
        webview: &WKWebView,
        kind: &'static str,
        message: String,
        default_text: Option<String>,
        reply: DialogReply,
    ) {
        let url = unsafe {
            webview
                .URL()
                .and_then(|value| value.absoluteString())
                .map(|value| value.to_string())
                .unwrap_or_default()
        };
        self.push(TabEvent::Dialog(PendingDialog {
            tab_id: self.ivars().tab_id.clone(),
            kind,
            message: truncate(&message, 1000),
            default_text,
            url: redact(&url),
            reply: Some(reply),
        }));
    }

    /// A page that did not load shows why, with the address that failed.
    fn show_load_error(&self, webview: &WKWebView, error: &NSError) {
        let domain = error.domain().to_string();
        if ignorable_load_error(&domain, error.code()) {
            return;
        }
        let failed = error
            .userInfo()
            .objectForKey(&NSString::from_str("NSErrorFailingURLStringKey"))
            .and_then(|value| value.downcast::<NSString>().ok())
            .map(|value| value.to_string())
            .filter(|url| allowed_url(url) && url != "about:blank");
        let Some(failed) = failed else {
            return;
        };
        let reason = error.localizedDescription().to_string();
        let page = load_error_page(&failed, &reason, !crate::i18n::is_en());
        let Some(base) = NSURL::URLWithString(&NSString::from_str(&failed)) else {
            return;
        };
        unsafe {
            webview.loadHTMLString_baseURL(&NSString::from_str(&page), Some(&base));
        }
    }
}

/// Pages follow the view's appearance for `prefers-color-scheme`.
pub fn apply_appearance(webview: &WKWebView, mode: &str) {
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    };
    let appearance = match mode {
        "light" => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }),
        "dark" => NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }),
        _ => None,
    };
    webview.setAppearance(appearance.as_deref());
}

fn safari_user_agent() -> String {
    // The generic Safari string WebKit itself sends; sites key their layout on it.
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15".into()
}

pub struct NewTab {
    pub web: Retained<WKWebView>,
    pub delegate: Retained<TabDelegate>,
    pub observer: Retained<TabObserver>,
}

/// A web view with every document-start script of the reference browser: the bridge (human
/// input, console), the network log and the sign-in capture, which only the client content
/// world can post from.
pub fn create(
    tab_id: &str,
    events: Events,
    store: &WKWebsiteDataStore,
    zoom: f64,
    appearance: &str,
    mtm: MainThreadMarker,
) -> NewTab {
    unsafe {
        let config = WKWebViewConfiguration::new(mtm);
        config.setWebsiteDataStore(store);
        let frame = objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(0., 0.),
            objc2_foundation::NSSize::new(1280., 800.),
        );
        let web = WKWebView::initWithFrame_configuration(mtm.alloc::<WKWebView>(), frame, &config);
        web.setCustomUserAgent(Some(&NSString::from_str(&safari_user_agent())));
        if zoom != 1.0 {
            web.setPageZoom(zoom);
        }
        apply_appearance(&web, appearance);
        let preferences = config.preferences();
        let _: () = msg_send![&*preferences, setValue: &*NSNumber::new_bool(true), forKey: &*NSString::from_str("developerExtrasEnabled")];
        let controller = config.userContentController();
        for source in [browser_js::PAGE_BRIDGE, browser_js::NETWORK] {
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                mtm.alloc::<WKUserScript>(),
                &NSString::from_str(source),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
            );
            controller.addUserScript(&script);
        }
        let capture = WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
            mtm.alloc::<WKUserScript>(),
            &NSString::from_str(browser_js::LOGIN_CAPTURE),
            WKUserScriptInjectionTime::AtDocumentStart,
            true,
            &WKContentWorld::defaultClientWorld(mtm),
        );
        controller.addUserScript(&capture);
        let observer = TabObserver::attach(&web, events.clone(), mtm);
        let delegate = TabDelegate::new(tab_id, events, mtm);
        controller.addScriptMessageHandler_contentWorld_name(
            ProtocolObject::from_ref(&*delegate),
            &WKContentWorld::defaultClientWorld(mtm),
            &NSString::from_str("sirusLogin"),
        );
        web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        web.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        web.setHidden(true);
        NewTab {
            web,
            delegate,
            observer,
        }
    }
}

pub fn load(web: &WKWebView, url: &str) {
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
        unsafe { web.loadRequest(&NSURLRequest::requestWithURL(&url)) };
    }
}

/// A one-shot answer, filled by a completion handler on the main thread.
pub type Answer<T> = async_channel::Receiver<Result<T, String>>;

fn channel<T>() -> (async_channel::Sender<Result<T, String>>, Answer<T>) {
    async_channel::bounded(2)
}

/// Runs a script body as an async function in the page (`callAsyncJavaScript`), with `args`
/// as the text of its one argument; the answer is the string the body returned.
pub fn call_page(
    web: &WKWebView,
    body: &str,
    args: &serde_json::Value,
    mtm: MainThreadMarker,
) -> Answer<String> {
    let (sender, receiver) = channel::<String>();
    let json = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());
    let dictionary: Retained<NSMutableDictionary<NSString, AnyObject>> = NSMutableDictionary::new();
    let value = NSString::from_str(&json);
    let object: &AnyObject = value.as_ref();
    let key = NSString::from_str("args");
    // SAFETY: NSString keys, and NSString implements NSCopying.
    unsafe { dictionary.setObject_forKey(object, ProtocolObject::from_ref(&*key)) };
    let handler = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        let result = if value.is_null() {
            match unsafe { error.as_ref() } {
                Some(error) => Err(error.localizedDescription().to_string()),
                None => Ok(String::new()),
            }
        } else {
            let object = unsafe { &*value };
            Ok(object
                .downcast_ref::<NSString>()
                .map(|text| text.to_string())
                .unwrap_or_default())
        };
        let _ = sender.try_send(result);
    });
    unsafe {
        web.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(body),
            Some(&dictionary),
            None,
            &WKContentWorld::pageWorld(mtm),
            Some(&handler),
        );
    }
    receiver
}

/// Evaluates a plain expression and answers its string value (or empty).
pub fn evaluate(web: &WKWebView, script: &str) -> Answer<String> {
    let (sender, receiver) = channel::<String>();
    let handler = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        let result = if value.is_null() {
            match unsafe { error.as_ref() } {
                Some(error) => Err(error.localizedDescription().to_string()),
                None => Ok(String::new()),
            }
        } else {
            let object = unsafe { &*value };
            Ok(object
                .downcast_ref::<NSString>()
                .map(|text| text.to_string())
                .unwrap_or_default())
        };
        let _ = sender.try_send(result);
    });
    unsafe {
        web.evaluateJavaScript_completionHandler(&NSString::from_str(script), Some(&handler))
    };
    receiver
}

/// The visible page as PNG bytes.
pub fn snapshot(web: &WKWebView) -> Answer<Vec<u8>> {
    let (sender, receiver) = channel::<Vec<u8>>();
    let handler = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let result = unsafe {
            if image.is_null() {
                Err(error
                    .as_ref()
                    .map(|error| error.localizedDescription().to_string())
                    .unwrap_or_else(|| "Could not capture the page.".into()))
            } else {
                let image = &*image;
                let png = image
                    .TIFFRepresentation()
                    .and_then(|data| {
                        NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &data)
                    })
                    .and_then(|rep| {
                        rep.representationUsingType_properties(
                            NSBitmapImageFileType::PNG,
                            &NSDictionary::new(),
                        )
                    });
                match png {
                    Some(png) if png.len() <= 8 * 1024 * 1024 => Ok(png.to_vec()),
                    Some(_) => Err("The screenshot is too large.".into()),
                    None => Err("Could not encode the screenshot.".into()),
                }
            }
        };
        let _ = sender.try_send(result);
    });
    unsafe { web.takeSnapshotWithConfiguration_completionHandler(None, &handler) };
    receiver
}

/// Parses what a page script returned: JSON when it is JSON, otherwise the text itself.
pub fn decode(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_addresses_and_the_blank_page_load() {
        assert!(allowed_url("https://example.com/a?b=1"));
        assert!(allowed_url("http://localhost:3000"));
        assert!(allowed_url("about:blank"));
        assert!(!allowed_url("file:///etc/passwd"));
        assert!(!allowed_url("javascript:alert(1)"));
        assert!(!allowed_url("data:text/html,x"));
        assert!(!allowed_url("https://"));
        assert!(!allowed_url("https://a\nb"));
    }

    #[test]
    fn dialog_urls_lose_their_query() {
        assert_eq!(redact("https://a.com/p?token=1#x"), "https://a.com/p");
    }

    #[test]
    fn the_error_page_escapes_the_address() {
        let page = load_error_page("https://x.com/\"><script>", "down", false);
        assert!(!page.contains("<script>"));
        assert!(page.contains("Try again"));
    }

    #[test]
    fn script_answers_decode_json_or_stay_text() {
        assert_eq!(decode("{\"ok\":true}")["ok"], true);
        assert_eq!(decode("already"), serde_json::json!("already"));
    }
}
