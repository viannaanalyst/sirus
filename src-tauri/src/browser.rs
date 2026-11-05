//! Embedded browser: native WKWebView tabs owned by Rust (ADR-029).
//! Pure policy and geometry live here so they stay testable; the platform
//! module owns WebKit objects and only runs on the macOS main thread.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTabState {
    pub id: String,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub favicon_url: String,
    /// The browser profile the tab opened in (ADR-150).
    #[serde(default = "crate::browser_profiles::default_profile_id")]
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSessionState {
    pub session_id: String,
    pub open: bool,
    pub tabs: Vec<BrowserTabState>,
    pub active_tab_id: Option<String>,
    /// The tab shown in the floating picture-in-picture window (ADR-111), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture_in_picture: Option<String>,
    /// Device mode: the CSS size pages lay out at (ADR-111), set by the person or an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<BrowserDevice>,
    /// The person's page zoom on top of device scaling (ADR-112), 0.5–2.
    #[serde(default = "unit_zoom")]
    pub zoom: f64,
    /// Colour scheme pages see: `system`, `light` or `dark` (ADR-112).
    #[serde(default = "system_appearance")]
    pub appearance: String,
    /// A JavaScript alert, confirm or prompt waiting for an answer (ADR-150), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<crate::browser_agent::BrowserDialog>,
}

fn unit_zoom() -> f64 {
    1.0
}

fn system_appearance() -> String {
    "system".into()
}

/// A device size in CSS pixels, and the preset it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDevice {
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

/// The device list (as Chrome DevTools has it); mirrors `src/lib/browser-device.ts`.
pub const DEVICE_PRESETS: [(&str, &str, u32, u32); 17] = [
    ("iphone-se", "iPhone SE", 375, 667),
    ("iphone-xr", "iPhone XR", 414, 896),
    ("iphone-12-pro", "iPhone 12 Pro", 390, 844),
    ("iphone-14-pro-max", "iPhone 14 Pro Max", 430, 932),
    ("pixel-7", "Pixel 7", 412, 915),
    ("galaxy-s8-plus", "Samsung Galaxy S8+", 360, 740),
    ("galaxy-s20-ultra", "Samsung Galaxy S20 Ultra", 412, 915),
    ("ipad-mini", "iPad Mini", 768, 1024),
    ("ipad-air", "iPad Air", 820, 1180),
    ("ipad-pro", "iPad Pro", 1024, 1366),
    ("surface-pro-7", "Surface Pro 7", 912, 1368),
    ("surface-duo", "Surface Duo", 540, 720),
    ("galaxy-z-fold-5", "Galaxy Z Fold 5", 344, 882),
    ("zenbook-fold", "Asus Zenbook Fold", 853, 1280),
    ("galaxy-a51-71", "Samsung Galaxy A51/71", 412, 914),
    ("nest-hub", "Nest Hub", 1024, 600),
    ("nest-hub-max", "Nest Hub Max", 1280, 800),
];

/// A device from a preset id or a size, turned to landscape when asked; sides 240–3840.
pub fn device_from(
    preset: Option<&str>,
    width: Option<u32>,
    height: Option<u32>,
    landscape: bool,
) -> Result<BrowserDevice> {
    let (mut width, mut height, preset) = match preset.filter(|id| !id.is_empty()) {
        Some(id) => {
            let (id, _, width, height) = DEVICE_PRESETS
                .iter()
                .find(|(known, label, _, _)| *known == id || label.eq_ignore_ascii_case(id))
                .ok_or_else(|| Error::new("invalid", "Unknown device preset."))?;
            (*width, *height, Some((*id).to_string()))
        }
        None => match (width, height) {
            (Some(width), Some(height)) => (width, height, None),
            _ => {
                return Err(Error::new(
                    "invalid",
                    "Give a device preset, or both width and height.",
                ));
            }
        },
    };
    if landscape != (width > height) && width != height {
        std::mem::swap(&mut width, &mut height);
    }
    Ok(BrowserDevice {
        width: width.clamp(240, 3840),
        height: height.clamp(240, 3840),
        preset,
    })
}

/// The person's page zoom (⌘+ / ⌘−), separate from device scaling.
pub fn clamp_user_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        (zoom * 100.0).round().clamp(50.0, 200.0) / 100.0
    } else {
        1.0
    }
}

impl BrowserSessionState {
    pub fn closed(session_id: &str) -> Self {
        Self {
            session_id: session_id.into(),
            open: false,
            tabs: vec![],
            active_tab_id: None,
            picture_in_picture: None,
            device: None,
            zoom: 1.0,
            appearance: system_appearance(),
            dialog: None,
        }
    }
}

/// A crashed page reloads by itself at most this often: attempts in the last
/// 30 s, with a growing wait (250 ms, 500 ms, 1 s). `None` once the page keeps crashing.
pub fn recovery_delay(
    recent: &mut Vec<std::time::Instant>,
    now: std::time::Instant,
) -> Option<std::time::Duration> {
    const WINDOW: std::time::Duration = std::time::Duration::from_secs(30);
    const ATTEMPTS: usize = 3;
    recent.retain(|at| now.duration_since(*at) < WINDOW);
    if recent.len() >= ATTEMPTS {
        return None;
    }
    let delay = std::time::Duration::from_millis(250 << recent.len());
    recent.push(now);
    Some(delay)
}

/// Device mode scales the page so a phone's CSS width fits the pane (ADR-111); never larger.
pub fn clamp_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        zoom.clamp(0.1, 1.0)
    } else {
        1.0
    }
}

/// Load failures that are not errors for the person: a navigation the page or
/// Sirus cancelled (`NSURLErrorCancelled`), or one WebKit stopped because it
/// became a download or was refused by the allowlist (frame load interrupted).
#[cfg_attr(not(all(target_os = "macos", feature = "desktop")), allow(dead_code))]
fn ignorable_load_error(domain: &str, code: isize) -> bool {
    (domain == "NSURLErrorDomain" && code == -999) || (domain == "WebKitErrorDomain" && code == 102)
}

#[cfg_attr(not(all(target_os = "macos", feature = "desktop")), allow(dead_code))]
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page shown when an address does not load (server down, redirect to a
/// closed port, no network), instead of a blank tab: the address that failed,
/// the system's reason and a retry that requests it again.
#[cfg_attr(not(all(target_os = "macos", feature = "desktop")), allow(dead_code))]
fn load_error_page(failed_url: &str, reason: &str, portuguese: bool) -> String {
    let (title, retry) = if portuguese {
        ("Não foi possível abrir a página", "Tentar de novo")
    } else {
        ("This page could not be opened", "Try again")
    };
    let url = escape_html(failed_url);
    let reason = escape_html(reason);
    // A JS string inside a single-quoted attribute: no quote can close it.
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

/// Top-level navigation allowlist. `file:`, `data:` and every other scheme are
/// refused; no user or page input reaches the OS.
pub fn is_allowed_browser_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return false;
    }
    if trimmed.eq_ignore_ascii_case("about:blank") {
        return true;
    }
    match trimmed.split_once(':') {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            (scheme == "http" || scheme == "https") && !rest.is_empty()
        }
        None => false,
    }
}

/// Favicons are display-only: http(s) or a bounded inline image, never a
/// script-capable scheme. The app webview renders them under the CSP image rule.
pub fn is_allowed_favicon_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.len() > 16 * 1024 || trimmed.chars().any(char::is_control) {
        return false;
    }
    if trimmed.starts_with("data:image/") {
        return true;
    }
    is_allowed_browser_url(trimmed)
}

/// Keys an agent may press on a page (ADR-087); anything else is refused.
pub const BROWSER_KEYS: &[&str] = &[
    "Enter",
    "Escape",
    "Tab",
    "Backspace",
    "Delete",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "PageUp",
    "PageDown",
    "Home",
    "End",
    " ",
];

pub fn is_allowed_browser_key(key: &str) -> bool {
    BROWSER_KEYS.contains(&key)
}

/// Element annotations collected by the fixed in-page annotation script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserAnnotation {
    pub selector: String,
    pub label: String,
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        value.into()
    } else {
        value.chars().take(limit).collect()
    }
}

/// Bounded, ownerless parse of the script result: at most 20 entries with a
/// selector, each field truncated before it reaches the renderer.
pub fn parse_browser_annotations(raw: &str) -> Vec<BrowserAnnotation> {
    let Ok(entries) = serde_json::from_str::<Vec<BrowserAnnotation>>(raw) else {
        return vec![];
    };
    entries
        .into_iter()
        .take(20)
        .filter(|annotation| !annotation.selector.trim().is_empty())
        .map(|annotation| BrowserAnnotation {
            selector: truncate_chars(annotation.selector.trim(), 400),
            label: truncate_chars(annotation.label.trim(), 300),
        })
        .collect()
}

/// Converts a logical top-left rect (CSS px, window content coordinates) into
/// an AppKit frame with a bottom-left origin. Sizes are at least 1×1.
pub fn appkit_frame(content_height: f64, bounds: &BrowserBounds) -> (f64, f64, f64, f64) {
    let width = bounds.width.max(1.0);
    let height = bounds.height.max(1.0);
    let x = bounds.x.max(0.0);
    let y = (content_height - bounds.y - height).max(0.0);
    (x, y, width, height)
}

// ---- Annotations with a screenshot (ADR-151) ----

/// An annotated element and where it sits in the viewport when the person finishes (CSS px).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotatedElement {
    pub selector: String,
    pub label: String,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
}

/// What finishing an annotation returns: the page, its viewport, the elements and a PNG
/// of the visible page (with the numbered markers) the window crops for the composer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BrowserAnnotationCapture {
    pub url: String,
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub elements: Vec<AnnotatedElement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub png: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AnnotationMeasure {
    url: String,
    width: f64,
    height: f64,
    entries: Vec<AnnotatedElement>,
}

fn finite(value: f64, limit: f64) -> f64 {
    if value.is_finite() {
        value.clamp(-limit, limit)
    } else {
        0.0
    }
}

/// Bounded parse of the fixed measure script's result: at most 20 elements with a
/// selector, text truncated, numbers finite, and the URL only when it is a page URL.
pub fn parse_annotation_measure(raw: &str) -> BrowserAnnotationCapture {
    let measure: AnnotationMeasure = serde_json::from_str(raw).unwrap_or_default();
    let url = truncate_chars(measure.url.trim(), 2048);
    BrowserAnnotationCapture {
        url: if is_allowed_browser_url(&url) {
            url
        } else {
            String::new()
        },
        viewport_width: finite(measure.width, 16_384.0).max(0.0),
        viewport_height: finite(measure.height, 16_384.0).max(0.0),
        elements: measure
            .entries
            .into_iter()
            .filter(|entry| !entry.selector.trim().is_empty())
            .take(20)
            .map(|entry| AnnotatedElement {
                selector: truncate_chars(entry.selector.trim(), 400),
                label: truncate_chars(entry.label.trim(), 300),
                x: finite(entry.x, 1e6),
                y: finite(entry.y, 1e6),
                width: finite(entry.width, 1e6).max(0.0),
                height: finite(entry.height, 1e6).max(0.0),
            })
            .collect(),
        png: None,
    }
}

#[cfg(test)]
mod annotation_capture_tests {
    use super::*;

    #[test]
    fn measures_are_bounded_and_urls_checked() {
        let raw = serde_json::json!({
            "url": "http://localhost:5173/app",
            "width": 1280.0,
            "height": 800.0,
            "entries": [
                { "selector": "button#save", "label": "button \"Save\"", "x": 10.5, "y": 20.0, "width": 80.0, "height": 32.0 },
                { "selector": "  ", "label": "ignored" },
                { "selector": "div", "label": "div", "x": -5.0, "y": 1e12, "width": -3.0, "height": 4.0 }
            ]
        })
        .to_string();
        let capture = parse_annotation_measure(&raw);
        assert_eq!(capture.url, "http://localhost:5173/app");
        assert_eq!(capture.viewport_width, 1280.0);
        assert_eq!(capture.elements.len(), 2);
        assert_eq!(capture.elements[0].label, "button \"Save\"");
        assert_eq!(capture.elements[1].y, 1e6);
        assert_eq!(capture.elements[1].width, 0.0);
        assert!(capture.png.is_none());
        let hostile = parse_annotation_measure(r#"{"url":"javascript:alert(1)","entries":[]}"#);
        assert_eq!(hostile.url, "");
        assert_eq!(
            parse_annotation_measure("not json"),
            BrowserAnnotationCapture::default()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_errors_show_the_failed_address_escaped_and_skip_cancellations() {
        assert!(ignorable_load_error("NSURLErrorDomain", -999));
        assert!(ignorable_load_error("WebKitErrorDomain", 102));
        assert!(!ignorable_load_error("NSURLErrorDomain", -1004));
        let page = load_error_page(
            "http://localhost:3001/login?a=1&b=<x>",
            "Could not connect to the server.",
            false,
        );
        assert!(page.contains("http://localhost:3001/login?a=1&amp;b=&lt;x&gt;"));
        assert!(page.contains("Could not connect to the server."));
        assert!(page.contains("Try again"));
        assert!(!page.contains("<x>"));
        let quoted = load_error_page("http://x/'onload", "r", true);
        assert!(quoted.contains("Não foi possível abrir a página"));
        assert!(
            quoted.contains(r#"onclick='location.href=&quot;http://x/\u0027onload&quot;'"#),
            "the retry target stays inside its attribute"
        );
    }

    #[test]
    fn crashed_pages_reload_three_times_per_window_with_backoff() {
        let start = std::time::Instant::now();
        let mut recent = vec![];
        let ms = |delay: Option<std::time::Duration>| delay.map(|d| d.as_millis());
        assert_eq!(ms(recovery_delay(&mut recent, start)), Some(250));
        assert_eq!(ms(recovery_delay(&mut recent, start)), Some(500));
        assert_eq!(ms(recovery_delay(&mut recent, start)), Some(1000));
        assert_eq!(recovery_delay(&mut recent, start), None);
        let later = start + std::time::Duration::from_secs(31);
        assert_eq!(ms(recovery_delay(&mut recent, later)), Some(250));
    }

    #[test]
    fn devices_come_from_presets_or_sizes_in_either_orientation() {
        let phone = device_from(Some("iphone-se"), None, None, false).unwrap();
        assert_eq!((phone.width, phone.height), (375, 667));
        assert_eq!(phone.preset.as_deref(), Some("iphone-se"));
        let by_label = device_from(Some("Pixel 7"), None, None, true).unwrap();
        assert_eq!((by_label.width, by_label.height), (915, 412));
        let hub = device_from(Some("nest-hub"), None, None, false).unwrap();
        assert_eq!((hub.width, hub.height), (600, 1024));
        let free = device_from(None, Some(100), Some(9000), false).unwrap();
        assert_eq!((free.width, free.height, free.preset), (240, 3840, None));
        assert!(device_from(Some("nokia"), None, None, false).is_err());
        assert!(device_from(None, Some(500), None, false).is_err());
    }

    #[test]
    fn user_zoom_steps_between_half_and_double() {
        assert_eq!(clamp_user_zoom(1.1), 1.1);
        assert_eq!(clamp_user_zoom(0.2), 0.5);
        assert_eq!(clamp_user_zoom(5.0), 2.0);
        assert_eq!(clamp_user_zoom(f64::INFINITY), 1.0);
    }

    #[test]
    fn zoom_stays_between_a_tenth_and_actual_size() {
        assert_eq!(clamp_zoom(0.5), 0.5);
        assert_eq!(clamp_zoom(3.0), 1.0);
        assert_eq!(clamp_zoom(0.0), 0.1);
        assert_eq!(clamp_zoom(f64::NAN), 1.0);
    }

    #[test]
    fn browser_keys_are_bounded() {
        assert!(is_allowed_browser_key("Enter"));
        assert!(!is_allowed_browser_key("Meta"));
    }

    #[test]
    fn browser_url_allowlist_accepts_only_http_https_and_blank() {
        assert!(is_allowed_browser_url("https://example.com/a?b=1#c"));
        assert!(is_allowed_browser_url("http://localhost:1420/"));
        assert!(is_allowed_browser_url("  https://example.com  "));
        assert!(is_allowed_browser_url("about:blank"));
        assert!(is_allowed_browser_url("ABOUT:BLANK"));
        assert!(!is_allowed_browser_url("file:///etc/passwd"));
        assert!(!is_allowed_browser_url("data:text/html,hello"));
        assert!(!is_allowed_browser_url("javascript:alert(1)"));
        assert!(!is_allowed_browser_url("about:config"));
        assert!(!is_allowed_browser_url("https:"));
        assert!(!is_allowed_browser_url(""));
        assert!(!is_allowed_browser_url("https://exa\nmple.com"));
        assert!(!is_allowed_browser_url(
            "custom-local-preview://token/x.html"
        ));
    }

    #[test]
    fn favicon_allowlist_accepts_http_and_inline_images_only() {
        assert!(is_allowed_favicon_url("https://example.com/favicon.ico"));
        assert!(is_allowed_favicon_url("data:image/png;base64,AAAA"));
        assert!(!is_allowed_favicon_url("javascript:alert(1)"));
        assert!(!is_allowed_favicon_url("file:///tmp/icon.png"));
        assert!(!is_allowed_favicon_url(&format!(
            "data:image/png;base64,{}",
            "A".repeat(20_000)
        )));
        assert!(!is_allowed_favicon_url(""));
    }

    #[test]
    fn annotation_parse_is_bounded_and_requires_a_selector() {
        let raw = r#"[{"selector":"button#x","label":"button"},{"selector":"  ","label":"skip"},{"selector":"a","label":"y"}]"#;
        let parsed = parse_browser_annotations(raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].selector, "button#x");
        assert!(parse_browser_annotations("not json").is_empty());
        let long = format!(r#"[{{"selector":"{}","label":""}}]"#, "a".repeat(500));
        assert_eq!(
            parse_browser_annotations(&long)[0].selector.chars().count(),
            400
        );
    }

    #[test]
    fn appkit_frame_flips_the_origin_and_clamps_edges() {
        let bounds = BrowserBounds {
            x: 10.0,
            y: 100.0,
            width: 200.0,
            height: 300.0,
        };
        assert_eq!(appkit_frame(800.0, &bounds), (10.0, 400.0, 200.0, 300.0));
        let negative = BrowserBounds {
            x: -5.0,
            y: 700.0,
            width: 0.0,
            height: -10.0,
        };
        assert_eq!(appkit_frame(800.0, &negative), (0.0, 99.0, 1.0, 1.0));
        let nan = BrowserBounds {
            x: f64::NAN,
            y: f64::NAN,
            width: f64::NAN,
            height: f64::NAN,
        };
        assert_eq!(appkit_frame(800.0, &nan), (0.0, 0.0, 1.0, 1.0));
    }
}

#[cfg(all(target_os = "macos", feature = "desktop"))]
pub(crate) mod platform {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, NSObject, ProtocolObject};
    use objc2::{define_class, msg_send, AllocAnyThread, DeclaredClass, MainThreadOnly};
    use objc2_app_kit::{
        NSBackingStoreType, NSBitmapImageFileType, NSBitmapImageRep, NSFloatingWindowLevel,
        NSImage, NSImageCompressionFactor, NSPanel, NSPasteboard, NSPasteboardTypeString, NSScreen,
        NSView, NSWindow, NSWindowCollectionBehavior, NSWindowDelegate, NSWindowStyleMask,
    };
    use objc2_foundation::{
        MainThreadMarker, NSDictionary, NSError, NSMutableDictionary, NSNotification, NSNumber,
        NSObjectProtocol, NSOperatingSystemVersion, NSPoint, NSProcessInfo, NSRect, NSSize,
        NSString, NSURLRequest, NSURL,
    };
    use objc2_web_kit::{
        WKContentWorld, WKFrameInfo, WKNavigation, WKNavigationAction, WKNavigationActionPolicy,
        WKNavigationDelegate, WKScriptMessage, WKScriptMessageHandler, WKSnapshotConfiguration,
        WKUIDelegate, WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebView,
        WKWebViewConfiguration, WKWebsiteDataStore, WKWindowFeatures,
    };
    use tauri::{AppHandle, Emitter, Manager};

    use super::{
        appkit_frame, clamp_user_zoom, clamp_zoom, ignorable_load_error, is_allowed_browser_url,
        is_allowed_favicon_url, load_error_page, parse_browser_annotations, recovery_delay,
        BrowserAnnotation, BrowserBounds, BrowserDevice, BrowserSessionState, BrowserTabState,
    };
    use crate::error::{Error, Result};

    struct TabRuntime {
        webview: Retained<WKWebView>,
        _delegate: Retained<BrowserNavigationDelegate>,
        url: String,
        title: String,
        loading: bool,
        can_go_back: bool,
        can_go_forward: bool,
        favicon_url: String,
        bounds: Option<BrowserBounds>,
        /// The profile the tab opened in (ADR-150).
        profile: String,
    }

    struct SessionRuntime {
        content_view: Retained<NSView>,
        tabs: Vec<(String, TabRuntime)>,
        active: Option<String>,
        next_tab: u64,
        /// Manager clock value of the last open; the least recent browser is released first.
        used: u64,
        /// Page zoom of device mode (ADR-111); 1 is actual size.
        zoom: f64,
        /// Device mode's CSS size (ADR-111), shared by the pane and agents.
        device: Option<BrowserDevice>,
        /// The person's own zoom on top (ADR-112).
        user_zoom: f64,
        /// `system`, `light` or `dark` (ADR-112).
        appearance: String,
        /// The tab floating in its own window (ADR-111).
        pip: Option<Pip>,
        /// The Private profile's store, shared by this session's private tabs and dropped with
        /// the browser (ADR-150).
        private_store: Option<Retained<WKWebsiteDataStore>>,
        /// A page's JavaScript dialog waiting for an answer (ADR-150).
        dialog: Option<crate::browser_agent::platform::PendingDialog>,
    }

    /// Picture in picture: one tab's view moved into an always-on-top panel.
    struct Pip {
        panel: Retained<NSPanel>,
        tab_id: String,
        _delegate: Retained<PipWindowDelegate>,
    }

    /// Each WKWebView keeps its own WebContent memory. A session keeps at most
    /// this many tabs (the oldest closes first), and at most this many sessions
    /// keep live browsers; an older one is released and reopens at its last page.
    const MAX_TABS_PER_SESSION: usize = 8;
    const MAX_LIVE_SESSIONS: usize = 4;
    const MAX_REMEMBERED_PAGES: usize = 64;

    #[derive(Default)]
    struct BrowserManager {
        sessions: HashMap<String, SessionRuntime>,
        clock: u64,
        /// Active page of a released browser, restored when its session reopens it.
        released: HashMap<String, String>,
    }

    // Main-thread-only state: every access below is inside a
    // `run_on_main_thread` closure or a WebKit delegate callback.
    thread_local! {
        static MANAGER: RefCell<Option<BrowserManager>> = const { RefCell::new(None) };
    }

    fn with_manager<R>(operation: impl FnOnce(&mut BrowserManager) -> R) -> Result<R> {
        MANAGER.with(|cell| {
            let mut borrowed = cell
                .try_borrow_mut()
                .map_err(|_| Error::new("native", "browser state is busy"))?;
            let manager = borrowed.get_or_insert_with(BrowserManager::default);
            Ok(operation(manager))
        })
    }

    pub(crate) fn on_main<T: Send + 'static>(
        app: &AppHandle,
        operation: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T> {
        let (sender, receiver) = mpsc::channel();
        app.run_on_main_thread(move || {
            let _ = sender.send(operation());
        })
        .map_err(|_| Error::new("native", "the main thread is unavailable"))?;
        receiver
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::new("native", "the browser did not answer in time"))
    }

    /// The raw NSWindow pointer, resolved on a worker thread: the window
    /// handle dispatch must never run on the main thread (deadlock).
    #[derive(Clone, Copy)]
    struct WindowPointer(*mut std::ffi::c_void);
    unsafe impl Send for WindowPointer {}

    fn main_window_pointer(app: &AppHandle) -> Result<WindowPointer> {
        let window = app
            .get_webview_window("main")
            .ok_or_else(|| Error::new("native", "the main window is not open"))?;
        let pointer = window
            .ns_window()
            .map_err(|_| Error::new("native", "the main window is unavailable"))?;
        Ok(WindowPointer(pointer))
    }

    fn window_content(pointer: WindowPointer) -> Result<(Retained<NSView>, f64)> {
        let window: &NSWindow = unsafe { &*pointer.0.cast::<NSWindow>() };
        let content = window
            .contentView()
            .ok_or_else(|| Error::new("native", "the window has no content view"))?;
        let height = content.bounds().size.height;
        Ok((content, height))
    }

    /// WKWebView's stock user agent omits the `Version/… Safari/…` tokens, so
    /// sites serve legacy pages without dark-mode support. Send a real Safari UA.
    fn safari_user_agent() -> String {
        let version: NSOperatingSystemVersion =
            NSProcessInfo::processInfo().operatingSystemVersion();
        format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{}.0 Safari/605.1.15",
            version.majorVersion
        )
    }

    fn session_state(session_id: &str, session: &SessionRuntime) -> BrowserSessionState {
        BrowserSessionState {
            session_id: session_id.into(),
            open: true,
            tabs: session
                .tabs
                .iter()
                .map(|(id, tab)| BrowserTabState {
                    id: id.clone(),
                    url: tab.url.clone(),
                    title: tab.title.clone(),
                    loading: tab.loading,
                    can_go_back: tab.can_go_back,
                    can_go_forward: tab.can_go_forward,
                    favicon_url: tab.favicon_url.clone(),
                    profile: tab.profile.clone(),
                })
                .collect(),
            active_tab_id: session.active.clone(),
            picture_in_picture: session.pip.as_ref().map(|pip| pip.tab_id.clone()),
            device: session.device.clone(),
            zoom: session.user_zoom,
            appearance: session.appearance.clone(),
            dialog: session.dialog.as_ref().map(|pending| pending.info.clone()),
        }
    }

    impl BrowserManager {
        fn sync_from_webview(
            &mut self,
            session_id: &str,
            tab_id: &str,
            webview: &WKWebView,
            loading: bool,
        ) -> Option<BrowserSessionState> {
            let session = self.sessions.get_mut(session_id)?;
            let tab = &mut session.tabs.iter_mut().find(|(id, _)| id == tab_id)?.1;
            unsafe {
                if let Some(url) = webview.URL().and_then(|value| value.absoluteString()) {
                    tab.url = url.to_string();
                }
                if let Some(title) = webview.title() {
                    tab.title = title.to_string();
                }
                tab.can_go_back = webview.canGoBack();
                tab.can_go_forward = webview.canGoForward();
            }
            tab.loading = loading;
            Some(session_state(session_id, session))
        }

        fn set_favicon(
            &mut self,
            session_id: &str,
            tab_id: &str,
            favicon_url: &str,
        ) -> Option<BrowserSessionState> {
            let session = self.sessions.get_mut(session_id)?;
            let tab = &mut session.tabs.iter_mut().find(|(id, _)| id == tab_id)?.1;
            if tab.favicon_url == favicon_url {
                return None;
            }
            tab.favicon_url = favicon_url.to_string();
            Some(session_state(session_id, session))
        }

        fn show_active(&mut self, session_id: &str, content_height: f64) {
            let Some(session) = self.sessions.get_mut(session_id) else {
                return;
            };
            let content = session.content_view.bounds().size;
            let active = session.active.clone();
            let floating = session.pip.as_ref().map(|pip| pip.tab_id.clone());
            for (id, tab) in &session.tabs {
                // The floating tab belongs to its panel.
                if floating.as_ref() == Some(id) {
                    continue;
                }
                let visible = Some(id) == active.as_ref() && tab.bounds.is_some();
                // A hidden page in device mode keeps the device's layout for agents (ADR-112).
                if tab.bounds.is_none() {
                    if let Some(device) = &session.device {
                        let scale = session.zoom;
                        tab.webview.setFrame(NSRect::new(
                            NSPoint::new(0.0, 0.0),
                            NSSize::new(device.width as f64 * scale, device.height as f64 * scale),
                        ));
                    }
                }
                if visible {
                    if let Some(bounds) = &tab.bounds {
                        // Never let a mis-measured rect escape the content view.
                        let clamped = BrowserBounds {
                            x: bounds.x.clamp(0.0, content.width.max(1.0)),
                            y: bounds.y.clamp(0.0, content.height.max(1.0)),
                            width: bounds.width.clamp(0.0, content.width.max(1.0)),
                            height: bounds.height.clamp(0.0, content.height.max(1.0)),
                        };
                        let (x, y, width, height) = appkit_frame(content_height, &clamped);
                        tab.webview
                            .setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
                    }
                }
                tab.webview.setHidden(!visible);
            }
        }

        fn open(
            &mut self,
            app: &AppHandle,
            session_id: &str,
            content: &Retained<NSView>,
            content_height: f64,
            mtm: MainThreadMarker,
            profile: &str,
        ) -> Result<BrowserSessionState> {
            self.clock += 1;
            let used = self.clock;
            self.sessions
                .entry(session_id.into())
                .or_insert_with(|| SessionRuntime {
                    content_view: content.clone(),
                    tabs: vec![],
                    active: None,
                    next_tab: 1,
                    used,
                    zoom: 1.0,
                    device: None,
                    user_zoom: 1.0,
                    appearance: "system".into(),
                    pip: None,
                    private_store: None,
                    dialog: None,
                })
                .used = used;
            if self
                .sessions
                .get(session_id)
                .is_some_and(|session| session.tabs.is_empty())
            {
                let page = self.released.remove(session_id);
                self.new_tab(app, session_id, page, profile, content_height, mtm)?;
            } else {
                self.show_active(session_id, content_height);
            }
            self.release_idle(app, session_id);
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| Error::new("native", "the browser session was not created"))?;
            Ok(session_state(session_id, session))
        }

        fn new_tab(
            &mut self,
            app: &AppHandle,
            session_id: &str,
            url: Option<String>,
            profile: &str,
            content_height: f64,
            mtm: MainThreadMarker,
        ) -> Result<BrowserSessionState> {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| Error::not_found("the browser is not open for this session"))?;
            if session.tabs.len() >= MAX_TABS_PER_SESSION {
                // The new tab becomes active, so the oldest one can go.
                let (_, oldest) = session.tabs.remove(0);
                oldest.webview.removeFromSuperview();
            }
            let content = session.content_view.clone();
            let zoom = session.zoom * session.user_zoom;
            let appearance = session.appearance.clone();
            let tab_id = format!("tab-{}", session.next_tab);
            session.next_tab += 1;
            let store =
                crate::browser_profiles::platform::store(profile, &mut session.private_store, mtm);
            let (webview, delegate) = unsafe {
                let config = WKWebViewConfiguration::new(mtm);
                config.setWebsiteDataStore(&store);
                // A laptop-sized page until the dock pane measures it, so a tab opened by
                // an agent lays out and captures like a real window (ADR-087).
                let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 800.0));
                let webview = WKWebView::initWithFrame_configuration(
                    mtm.alloc::<WKWebView>(),
                    frame,
                    &config,
                );
                webview.setCustomUserAgent(Some(&NSString::from_str(&safari_user_agent())));
                if zoom != 1.0 {
                    webview.setPageZoom(zoom);
                }
                apply_appearance(&webview, &appearance);
                if NSObject::respondsToSelector(&webview, objc2::sel!(setInspectable:)) {
                    webview.setInspectable(true);
                }
                // The menu's Web Inspector needs WebKit's developer extras (as Tauri's devtools).
                let preferences = config.preferences();
                let _: () = msg_send![&*preferences, setValue: &*objc2_foundation::NSNumber::new_bool(true), forKey: &*NSString::from_str("developerExtrasEnabled")];
                let bridge = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    mtm.alloc::<WKUserScript>(),
                    &NSString::from_str(PAGE_BRIDGE_SCRIPT),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    true,
                );
                config.userContentController().addUserScript(&bridge);
                crate::browser_agent::platform::install_scripts(&config, mtm);
                crate::browser_login::install_capture(&config, mtm);
                let delegate = BrowserNavigationDelegate::new(app, session_id, &tab_id, mtm);
                // Submitted sign-ins reach the app through a handler only the capture
                // script's own (client) content world can call; the page cannot (ADR-186).
                webview
                    .configuration()
                    .userContentController()
                    .addScriptMessageHandler_contentWorld_name(
                        ProtocolObject::from_ref(&*delegate),
                        &WKContentWorld::defaultClientWorld(mtm),
                        &NSString::from_str("sirusLogin"),
                    );
                webview.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                webview.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                (webview, delegate)
            };
            webview.setHidden(true);
            content.addSubview(&webview);
            session.tabs.push((
                tab_id.clone(),
                TabRuntime {
                    webview,
                    _delegate: delegate,
                    url: String::new(),
                    title: String::new(),
                    loading: false,
                    can_go_back: false,
                    can_go_forward: false,
                    favicon_url: String::new(),
                    bounds: None,
                    profile: profile.to_string(),
                },
            ));
            session.active = Some(tab_id);
            if let Some(url) = url.filter(|value| is_allowed_browser_url(value)) {
                if let Some((_, tab)) = session.tabs.last() {
                    unsafe {
                        if let Some(ns_url) = NSURL::URLWithString(&NSString::from_str(&url)) {
                            tab.webview
                                .loadRequest(&NSURLRequest::requestWithURL(&ns_url));
                        }
                    }
                }
            }
            self.show_active(session_id, content_height);
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| Error::new("native", "the browser tab was not created"))?;
            Ok(session_state(session_id, session))
        }

        fn webview(&self, session_id: &str, tab_id: &str) -> Result<Retained<WKWebView>> {
            self.sessions
                .get(session_id)
                .and_then(|session| session.tabs.iter().find(|(id, _)| id == tab_id))
                .map(|(_, tab)| tab.webview.clone())
                .ok_or_else(|| Error::not_found("browser tab not found"))
        }

        fn tab_url(&self, session_id: &str, tab_id: &str) -> Option<String> {
            self.sessions
                .get(session_id)
                .and_then(|session| session.tabs.iter().find(|(id, _)| id == tab_id))
                .map(|(_, tab)| tab.url.clone())
        }

        fn tab_profile(&self, session_id: &str, tab_id: &str) -> String {
            self.sessions
                .get(session_id)
                .and_then(|session| session.tabs.iter().find(|(id, _)| id == tab_id))
                .map(|(_, tab)| tab.profile.clone())
                .unwrap_or_else(crate::browser_profiles::default_profile_id)
        }

        fn open_popup_tab(
            &mut self,
            app: &AppHandle,
            session_id: &str,
            opener: &str,
            url: &str,
            mtm: MainThreadMarker,
        ) -> Option<BrowserSessionState> {
            let height = self
                .sessions
                .get(session_id)?
                .content_view
                .bounds()
                .size
                .height;
            let profile = self.tab_profile(session_id, opener);
            self.new_tab(
                app,
                session_id,
                Some(url.to_string()),
                &profile,
                height,
                mtm,
            )
            .ok()
        }

        fn mark_loading(&mut self, session_id: &str, tab_id: &str, url: &str) {
            if let Some(tab) = self
                .sessions
                .get_mut(session_id)
                .and_then(|session| session.tabs.iter_mut().find(|(id, _)| id == tab_id))
                .map(|(_, tab)| tab)
            {
                tab.loading = true;
                if !url.is_empty() {
                    tab.url = url.to_string();
                }
            }
        }

        fn close_tab(
            &mut self,
            session_id: &str,
            tab_id: &str,
            content_height: f64,
        ) -> Result<BrowserSessionState> {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| Error::not_found("the browser is not open for this session"))?;
            let position = session
                .tabs
                .iter()
                .position(|(id, _)| id == tab_id)
                .ok_or_else(|| Error::not_found("browser tab not found"))?;
            if session.pip.as_ref().is_some_and(|pip| pip.tab_id == tab_id) {
                if let Some(pip) = session.pip.take() {
                    dismiss_panel(&pip);
                }
            }
            let (_, removed) = session.tabs.remove(position);
            removed.webview.removeFromSuperview();
            if session.active.as_deref() == Some(tab_id) {
                session.active = session.tabs.last().map(|(id, _)| id.clone());
            }
            self.show_active(session_id, content_height);
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| Error::new("native", "the browser session was closed"))?;
            Ok(session_state(session_id, session))
        }

        fn select_tab(
            &mut self,
            session_id: &str,
            tab_id: &str,
            content_height: f64,
        ) -> Result<BrowserSessionState> {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| Error::not_found("the browser is not open for this session"))?;
            if !session.tabs.iter().any(|(id, _)| id == tab_id) {
                return Err(Error::not_found("browser tab not found"));
            }
            session.active = Some(tab_id.into());
            self.show_active(session_id, content_height);
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| Error::not_found("the browser is not open for this session"))?;
            Ok(session_state(session_id, session))
        }

        fn navigate(
            &mut self,
            session_id: &str,
            tab_id: &str,
            url: &str,
        ) -> Result<BrowserSessionState> {
            if !is_allowed_browser_url(url) {
                return Err(Error::new(
                    "invalid",
                    "This URL cannot be opened in the browser.",
                ));
            }
            let webview = self.webview(session_id, tab_id)?;
            let request = NSURL::URLWithString(&NSString::from_str(url))
                .map(|ns_url| NSURLRequest::requestWithURL(&ns_url))
                .ok_or_else(|| {
                    Error::new("invalid", "This URL cannot be opened in the browser.")
                })?;
            unsafe {
                webview.loadRequest(&request);
            }
            self.mark_loading(session_id, tab_id, url);
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| Error::not_found("the browser is not open for this session"))?;
            Ok(session_state(session_id, session))
        }

        fn history(
            &mut self,
            session_id: &str,
            tab_id: &str,
            forward: bool,
        ) -> Result<BrowserSessionState> {
            let webview = self.webview(session_id, tab_id)?;
            unsafe {
                if forward {
                    webview.goForward();
                } else {
                    webview.goBack();
                }
            }
            self.mark_loading(session_id, tab_id, "");
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| Error::not_found("the browser is not open for this session"))?;
            Ok(session_state(session_id, session))
        }

        fn reload(&mut self, session_id: &str, tab_id: &str) -> Result<BrowserSessionState> {
            let webview = self.webview(session_id, tab_id)?;
            // An error page is a local HTML string: reloading it would show it again,
            // so the address it stands for is requested instead.
            let showing_error = ERROR_PAGES
                .lock()
                .remove(&format!("{session_id}\u{0}{tab_id}"));
            unsafe {
                match webview.URL().filter(|_| showing_error) {
                    Some(url) => {
                        webview.loadRequest(&NSURLRequest::requestWithURL(&url));
                    }
                    None => {
                        webview.reload();
                    }
                }
            }
            self.mark_loading(session_id, tab_id, "");
            let session = self
                .sessions
                .get(session_id)
                .ok_or_else(|| Error::not_found("the browser is not open for this session"))?;
            Ok(session_state(session_id, session))
        }

        fn set_bounds(
            &mut self,
            session_id: &str,
            bounds: Option<BrowserBounds>,
            content_height: f64,
        ) {
            let Some(session) = self.sessions.get_mut(session_id) else {
                return;
            };
            for (_, tab) in &mut session.tabs {
                tab.bounds = bounds.clone();
            }
            self.show_active(session_id, content_height);
        }

        /// Releases the least recently opened browsers beyond the live limit, never
        /// `current`. The renderer learns through `browser-state` and reopens on demand.
        fn release_idle(&mut self, app: &AppHandle, current: &str) {
            while self.sessions.len() > MAX_LIVE_SESSIONS {
                let Some(oldest) = self
                    .sessions
                    .iter()
                    .filter(|(id, _)| id.as_str() != current)
                    .min_by_key(|(_, session)| session.used)
                    .map(|(id, _)| id.clone())
                else {
                    return;
                };
                if let Some(session) = self.sessions.get(&oldest) {
                    let page = session
                        .active
                        .as_ref()
                        .and_then(|active| session.tabs.iter().find(|(id, _)| id == active))
                        .map(|(_, tab)| tab.url.clone())
                        .filter(|url| is_allowed_browser_url(url));
                    if let Some(page) = page {
                        if self.released.len() >= MAX_REMEMBERED_PAGES {
                            self.released.clear();
                        }
                        self.released.insert(oldest.clone(), page);
                    }
                }
                self.close(&oldest);
                let _ = app.emit("browser-state", BrowserSessionState::closed(&oldest));
            }
        }

        fn close(&mut self, session_id: &str) {
            if let Some(mut session) = self.sessions.remove(session_id) {
                if let Some(pip) = session.pip.take() {
                    dismiss_panel(&pip);
                }
                for (_, tab) in session.tabs {
                    tab.webview.removeFromSuperview();
                }
            }
        }

        fn set_zoom(&mut self, session_id: &str, zoom: f64) {
            let Some(session) = self.sessions.get_mut(session_id) else {
                return;
            };
            session.zoom = clamp_zoom(zoom);
            apply_zoom(session);
            let height = session.content_view.bounds().size.height;
            self.show_active(session_id, height);
        }

        fn set_user_zoom(&mut self, session_id: &str, zoom: f64) -> Option<BrowserSessionState> {
            let session = self.sessions.get_mut(session_id)?;
            session.user_zoom = clamp_user_zoom(zoom);
            apply_zoom(session);
            Some(session_state(session_id, session))
        }

        /// Device mode from the pane or an agent; a hidden pane sizes the page itself, so an
        /// agent's screenshots and layout follow the device too.
        fn set_device(
            &mut self,
            session_id: &str,
            device: Option<BrowserDevice>,
        ) -> Option<BrowserSessionState> {
            let session = self.sessions.get_mut(session_id)?;
            if device.is_none() {
                session.zoom = 1.0;
            }
            session.device = device;
            apply_zoom(session);
            let height = session.content_view.bounds().size.height;
            self.show_active(session_id, height);
            self.sessions
                .get(session_id)
                .map(|session| session_state(session_id, session))
        }

        fn set_appearance(&mut self, session_id: &str, mode: &str) -> Option<BrowserSessionState> {
            let session = self.sessions.get_mut(session_id)?;
            session.appearance = match mode {
                "light" | "dark" => mode.to_string(),
                _ => "system".into(),
            };
            for (_, tab) in &session.tabs {
                apply_appearance(&tab.webview, &session.appearance);
            }
            Some(session_state(session_id, session))
        }

        /// Moves the active tab into a floating panel at the screen's lower right.
        fn open_pip(
            &mut self,
            app: &AppHandle,
            session_id: &str,
            mtm: MainThreadMarker,
        ) -> Result<()> {
            let session = self
                .sessions
                .get_mut(session_id)
                .ok_or_else(|| Error::not_found("the browser is not open for this session"))?;
            if session.pip.is_some() {
                return Ok(());
            }
            let tab_id = session
                .active
                .clone()
                .ok_or_else(|| Error::new("invalid", "No browser tab is open."))?;
            let tab = &session
                .tabs
                .iter()
                .find(|(id, _)| *id == tab_id)
                .ok_or_else(|| Error::not_found("browser tab not found"))?
                .1;
            let size = NSSize::new(480.0, 320.0);
            let origin = NSScreen::mainScreen(mtm)
                .map(|screen| screen.visibleFrame())
                .map(|frame| {
                    NSPoint::new(
                        frame.origin.x + frame.size.width - size.width - 24.0,
                        frame.origin.y + 24.0,
                    )
                })
                .unwrap_or(NSPoint::new(80.0, 80.0));
            let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSPanel>(),
                NSRect::new(origin, size),
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
            panel.setContentMinSize(NSSize::new(240.0, 160.0));
            let title = if tab.title.is_empty() {
                tab.url.as_str()
            } else {
                tab.title.as_str()
            };
            panel.setTitle(&NSString::from_str(title));
            let delegate = PipWindowDelegate::new(app, session_id, mtm);
            panel.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            tab.webview.removeFromSuperview();
            tab.webview.setHidden(false);
            panel.setContentView(Some(&tab.webview));
            panel.orderFrontRegardless();
            session.pip = Some(Pip {
                panel,
                tab_id,
                _delegate: delegate,
            });
            Ok(())
        }

        /// Puts the floating tab back in the pane.
        fn close_pip(&mut self, session_id: &str, content_height: f64) {
            let Some(session) = self.sessions.get_mut(session_id) else {
                return;
            };
            let Some(pip) = session.pip.take() else {
                return;
            };
            dismiss_panel(&pip);
            if let Some((_, tab)) = session.tabs.iter().find(|(id, _)| *id == pip.tab_id) {
                tab.webview.removeFromSuperview();
                tab.webview.setHidden(true);
                session.content_view.addSubview(&tab.webview);
            }
            self.show_active(session_id, content_height);
        }

        /// Keeps a page's dialog until someone answers it; dropping an unanswered one dismisses it.
        fn queue_dialog(
            &mut self,
            session_id: &str,
            pending: crate::browser_agent::platform::PendingDialog,
        ) -> Option<BrowserSessionState> {
            let session = self.sessions.get_mut(session_id)?;
            session.dialog = Some(pending);
            Some(session_state(session_id, session))
        }

        /// Takes the open dialog of `tab_id` (or of any tab) to answer it.
        fn take_dialog(
            &mut self,
            session_id: &str,
            tab_id: Option<&str>,
        ) -> Option<(
            crate::browser_agent::platform::PendingDialog,
            BrowserSessionState,
        )> {
            let session = self.sessions.get_mut(session_id)?;
            let wanted = session
                .dialog
                .as_ref()
                .is_some_and(|pending| tab_id.is_none_or(|tab| pending.info.tab_id == tab));
            if !wanted {
                return None;
            }
            let pending = session.dialog.take()?;
            let session = self.sessions.get(session_id)?;
            Some((pending, session_state(session_id, session)))
        }

        /// Closes every tab opened in `profile`, in every session.
        fn close_profile_tabs(&mut self, profile: &str) -> Vec<BrowserSessionState> {
            let targets: Vec<(String, String)> = self
                .sessions
                .iter()
                .flat_map(|(session_id, session)| {
                    session
                        .tabs
                        .iter()
                        .filter(|(_, tab)| tab.profile == profile)
                        .map(move |(tab_id, _)| (session_id.clone(), tab_id.clone()))
                })
                .collect();
            let mut states = Vec::new();
            for (session_id, tab_id) in targets {
                let height = self
                    .sessions
                    .get(&session_id)
                    .map_or(0.0, |session| session.content_view.bounds().size.height);
                if let Ok(state) = self.close_tab(&session_id, &tab_id, height) {
                    states.push(state);
                }
            }
            states
        }

        fn state(&self, session_id: &str) -> BrowserSessionState {
            self.sessions.get(session_id).map_or_else(
                || BrowserSessionState::closed(session_id),
                |session| session_state(session_id, session),
            )
        }
    }

    /// Device scale times the person's zoom, on every tab of the session.
    fn apply_zoom(session: &SessionRuntime) {
        let zoom = session.zoom * session.user_zoom;
        for (_, tab) in &session.tabs {
            unsafe { tab.webview.setPageZoom(zoom) };
        }
    }

    /// Pages follow the view's appearance for `prefers-color-scheme`.
    fn apply_appearance(webview: &WKWebView, mode: &str) {
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

    /// Tabs whose last load failed and show the error page (`session\0tab`). A stale
    /// entry only makes the next reload request the current address, the same thing.
    static ERROR_PAGES: parking_lot::Mutex<std::collections::BTreeSet<String>> =
        parking_lot::Mutex::new(std::collections::BTreeSet::new());

    struct BrowserDelegateIvars {
        session_id: String,
        tab_id: String,
        app: AppHandle,
        /// When the page's WebContent process last crashed, for `recovery_delay`.
        crashes: RefCell<Vec<std::time::Instant>>,
    }

    /// Closes a panel without its delegate hearing about it.
    fn dismiss_panel(pip: &Pip) {
        pip.panel.setDelegate(None);
        pip.panel.setContentView(None);
        pip.panel.close();
    }

    struct PipIvars {
        session_id: String,
        app: AppHandle,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[ivars = PipIvars]
        struct PipWindowDelegate;

        unsafe impl NSObjectProtocol for PipWindowDelegate {}

        unsafe impl NSWindowDelegate for PipWindowDelegate {
            /// The person closed the floating window: the tab goes back to the pane, on the
            /// next turn of the run loop (the panel is still closing now).
            #[unsafe(method(windowWillClose:))]
            fn will_close(&self, _notification: &NSNotification) {
                let ivars = self.ivars();
                let (app, session_id) = (ivars.app.clone(), ivars.session_id.clone());
                let _ = ivars.app.run_on_main_thread(move || {
                    let height = main_window_pointer(&app)
                        .ok()
                        .and_then(|pointer| window_content(pointer).ok())
                        .map(|(_, height)| height)
                        .unwrap_or(0.0);
                    let state = with_manager(|manager| {
                        manager.close_pip(&session_id, height);
                        manager.state(&session_id)
                    });
                    if let Ok(state) = state {
                        let _ = app.emit("browser-state", state);
                    }
                });
            }
        }
    );

    impl PipWindowDelegate {
        fn new(app: &AppHandle, session_id: &str, mtm: MainThreadMarker) -> Retained<Self> {
            let delegate = mtm.alloc::<Self>().set_ivars(PipIvars {
                session_id: session_id.into(),
                app: app.clone(),
            });
            unsafe { msg_send![super(delegate), init] }
        }
    }

    const FAVICON_SCRIPT: &str = r#"
(() => {
  const links = Array.from(document.querySelectorAll("link[rel]"));
  const icon = links.find((link) => /(^|\s)(icon|shortcut icon|apple-touch-icon)(\s|$)/i.test(link.rel) && link.href);
  return icon ? String(icon.href) : (location.origin + "/favicon.ico");
})()
"#;

    /// Fixed annotation overlay: hover outline + element HUD, click records an
    /// entry, Escape cancels. No renderer-provided script ever reaches the page.
    const ANNOTATE_START_SCRIPT: &str = r##"
(() => {
  const KEY = "__sirusAnnotations";
  const current = window[KEY];
  if (current && current.active) return "already";
  const entries = [];
  const nodes = [];
  const targets = [];
  const box = document.createElement("div");
  box.style.cssText = "position:fixed;pointer-events:none;z-index:2147483646;border:2px solid #3b82f6;border-radius:3px;box-shadow:0 0 0 1px rgba(59,130,246,.35);display:none";
  const hud = document.createElement("div");
  hud.style.cssText = "position:fixed;pointer-events:none;z-index:2147483647;display:none;background:rgba(10,10,10,.94);color:#f5f5f5;font:11px/17px ui-monospace,SFMono-Regular,Menlo,monospace;border-radius:8px;padding:7px 10px;white-space:pre;box-shadow:0 8px 24px rgba(0,0,0,.4)";
  const describe = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    const tag = el.tagName.toLowerCase();
    const name = el.getAttribute("aria-label") || el.getAttribute("placeholder") || el.getAttribute("title") || (el.innerText || "").trim().replace(/\s+/g, " ").slice(0, 60);
    return (name ? tag + " \"" + name + "\"" : tag)
      + "\n" + Math.round(rect.width) + "x" + Math.round(rect.height)
      + "\ncolor " + style.color
      + "\nfont " + style.fontSize + " " + style.fontFamily
      + "\npadding " + style.padding;
  };
  const selectorFor = (el) => {
    const parts = [];
    let node = el;
    while (node && node.nodeType === 1 && parts.length < 6) {
      let part = node.tagName.toLowerCase();
      if (node.id) { parts.unshift(part + "#" + node.id); break; }
      const parent = node.parentElement;
      if (parent) {
        const siblings = Array.from(parent.children).filter((child) => child.tagName === node.tagName);
        if (siblings.length > 1) part += ":nth-of-type(" + (siblings.indexOf(node) + 1) + ")";
      }
      parts.unshift(part);
      node = node.parentElement;
    }
    return parts.join(" > ").slice(0, 400);
  };
  const labelFor = (el) => {
    const tag = el.tagName.toLowerCase();
    const rect = el.getBoundingClientRect();
    const name = (el.getAttribute("aria-label") || el.getAttribute("placeholder") || el.getAttribute("title") || (el.innerText || "").trim().replace(/\s+/g, " ")).slice(0, 80);
    return name ? tag + " \"" + name + "\"" : tag + " " + Math.round(rect.width) + "x" + Math.round(rect.height);
  };
  const move = (event) => {
    const target = document.elementFromPoint(event.clientX, event.clientY);
    if (!target || target === box || target === hud) { box.style.display = "none"; hud.style.display = "none"; return; }
    const rect = target.getBoundingClientRect();
    box.style.display = "block";
    box.style.left = rect.left + "px"; box.style.top = rect.top + "px";
    box.style.width = rect.width + "px"; box.style.height = rect.height + "px";
    hud.textContent = describe(target);
    hud.style.display = "block";
    hud.style.left = Math.max(8, Math.min(event.clientX + 14, window.innerWidth - 260)) + "px";
    hud.style.top = Math.max(8, Math.min(event.clientY + 16, window.innerHeight - 130)) + "px";
  };
  const click = (event) => {
    const target = event.target;
    if (!target || target === box || target === hud) return;
    event.preventDefault();
    event.stopPropagation();
    const rect = target.getBoundingClientRect();
    const marker = document.createElement("div");
    marker.style.cssText = "position:fixed;pointer-events:none;z-index:2147483647;background:#3b82f6;color:#fff;border-radius:999px;font:600 10px/16px -apple-system,sans-serif;min-width:16px;height:16px;text-align:center;padding:0 4px;box-shadow:0 1px 4px rgba(0,0,0,.4)";
    marker.textContent = String(entries.length + 1);
    marker.style.left = Math.max(0, rect.left - 6) + "px";
    marker.style.top = Math.max(0, rect.top - 6) + "px";
    document.body.appendChild(marker);
    nodes.push(marker);
    targets.push(target);
    entries.push({ selector: selectorFor(target), label: labelFor(target) });
  };
  // Hides the hover outline, puts each marker back on its element and returns where they are.
  const measure = () => {
    box.style.display = "none";
    hud.style.display = "none";
    return {
      url: location.href, width: window.innerWidth, height: window.innerHeight,
      entries: entries.map((entry, index) => {
        const target = targets[index];
        const rect = target && target.isConnected ? target.getBoundingClientRect() : null;
        if (rect && nodes[index]) {
          nodes[index].style.left = Math.max(0, rect.left - 6) + "px";
          nodes[index].style.top = Math.max(0, rect.top - 6) + "px";
        }
        return { selector: entry.selector, label: entry.label, x: rect ? rect.left : 0, y: rect ? rect.top : 0, width: rect ? rect.width : 0, height: rect ? rect.height : 0 };
      }),
    };
  };
  const cancel = () => {
    document.removeEventListener("mousemove", move, true);
    document.removeEventListener("click", click, true);
    document.removeEventListener("keydown", key, true);
    box.remove();
    hud.remove();
    nodes.forEach((node) => node.remove());
    window[KEY] = null;
  };
  const key = (event) => { if (event.key === "Escape") cancel(); };
  document.addEventListener("mousemove", move, true);
  document.addEventListener("click", click, true);
  document.addEventListener("keydown", key, true);
  document.body.appendChild(box);
  document.body.appendChild(hud);
  window[KEY] = { active: true, entries, cancel, measure };
  return "started";
})()
"##;

    const ANNOTATE_FINISH_SCRIPT: &str = r#"
(() => {
  const current = window.__sirusAnnotations;
  if (!current) return "[]";
  const raw = JSON.stringify(current.entries || []);
  current.cancel();
  return raw;
})()
"#;

    /// Annotations with a screenshot (ADR-151): element rects before the snapshot.
    const ANNOTATE_MEASURE_SCRIPT: &str = r#"
(() => {
  const current = window.__sirusAnnotations;
  const empty = { url: location.href, width: window.innerWidth, height: window.innerHeight, entries: [] };
  return JSON.stringify(current && current.measure ? current.measure() : empty);
})()
"#;

    const ANNOTATE_CANCEL_SCRIPT: &str = r#"
(() => {
  const current = window.__sirusAnnotations;
  if (current) current.cancel();
  return "cancelled";
})()
"#;

    /// Document-start bridge: human-input timestamps for takeover detection and
    /// a bounded console log buffer for `browser_logs`.
    const PAGE_BRIDGE_SCRIPT: &str = r##"
(() => {
  window.__sirusHumanAt = window.__sirusHumanAt || 0;
  // Trusted events only: the agent's own synthetic keys (a typed Enter) must not read as the person.
  const mark = (event) => { if (event.isTrusted) window.__sirusHumanAt = Date.now(); };
  document.addEventListener("pointerdown", mark, true);
  document.addEventListener("keydown", mark, true);
  document.addEventListener("wheel", mark, true);
  window.__sirusLogs = window.__sirusLogs || [];
  const push = (level, args) => {
    try {
      window.__sirusLogs.push({ level, text: Array.from(args).map(String).join(" ").slice(0, 500), at: Date.now() });
      if (window.__sirusLogs.length > 200) window.__sirusLogs.shift();
    } catch {}
  };
  for (const level of ["log", "warn", "error", "info"]) {
    const original = console[level];
    console[level] = (...args) => { push(level, args); original.apply(console, args); };
  }
  window.addEventListener("error", (event) => push("error", [event.message]));
  window.addEventListener("unhandledrejection", (event) => push("error", [String(event.reason)]));
  // Key presses and clicks for recordings (ADR-112); never what is typed in a password field.
  window.__sirusInput = window.__sirusInput || [];
  const note = (entry) => { window.__sirusInput.push(entry); if (window.__sirusInput.length > 60) window.__sirusInput.shift(); };
  const named = { " ": "Space", ArrowLeft: "←", ArrowRight: "→", ArrowUp: "↑", ArrowDown: "↓", Enter: "↩", Backspace: "⌫", Delete: "⌦", Escape: "Esc", Tab: "⇥" };
  document.addEventListener("keydown", (event) => {
    if (["Meta", "Control", "Alt", "Shift"].includes(event.key)) return;
    const target = event.target;
    if (target && target.type === "password") { note({ kind: "key", key: "•", at: Date.now() }); return; }
    const key = named[event.key] || event.key;
    const mods = (event.metaKey ? "⌘" : "") + (event.ctrlKey ? "⌃" : "") + (event.altKey ? "⌥" : "") + (event.shiftKey && event.key.length > 1 ? "⇧" : "");
    // A letter with ⌘ or ⌃ reads as a shortcut, upper case (⌘K).
    const shortcut = (event.metaKey || event.ctrlKey) && event.key.length === 1;
    note({ kind: "key", key: (mods + (shortcut ? key.toUpperCase() : key)).slice(0, 24), at: Date.now() });
  }, true);
  document.addEventListener("pointerdown", (event) => note({ kind: "click", x: event.clientX, y: event.clientY, at: Date.now() }), true);
})()
"##;

    const SNAPSHOT_SCRIPT: &str = r##"
const nodes = Array.from(document.querySelectorAll("a,button,input,textarea,select,[role='button'],[role='link'],[contenteditable='true']"));
const elements = [];
let count = 0;
for (const el of nodes) {
  if (elements.length >= 200) break;
  const rect = el.getBoundingClientRect();
  if (rect.width < 1 || rect.height < 1) continue;
  const ref = "e" + (++count);
  el.setAttribute("data-sirus-ref", ref);
  elements.push({
    ref,
    tag: el.tagName.toLowerCase(),
    role: el.getAttribute("role") || "",
    name: (el.getAttribute("aria-label") || el.getAttribute("placeholder") || el.innerText || el.value || "").trim().slice(0, 120),
    value: (el.value ?? "").toString().slice(0, 80),
    disabled: !!el.disabled,
  });
}
return JSON.stringify({ url: location.href, title: document.title, elements });
"##;

    const CLICK_SCRIPT: &str = r##"
const input = JSON.parse(args);
const ref = input.ref;
const selector = input.selector;
if (Date.now() - (window.__sirusHumanAt || 0) < 1500) return JSON.stringify({ interrupted: true });
const target = ref ? document.querySelector("[data-sirus-ref='" + ref + "']") : (selector ? document.querySelector(selector) : null);
if (!target) return JSON.stringify({ ok: false, error: "element not found" });
target.scrollIntoView({ block: "center" });
target.click();
return JSON.stringify({ ok: true });
"##;

    const TYPE_SCRIPT: &str = r##"
const input = JSON.parse(args);
const ref = input.ref;
const selector = input.selector;
const text = input.text;
const submit = input.submit;
if (Date.now() - (window.__sirusHumanAt || 0) < 1500) return JSON.stringify({ interrupted: true });
const target = ref ? document.querySelector("[data-sirus-ref='" + ref + "']") : (selector ? document.querySelector(selector) : null);
if (!target) return JSON.stringify({ ok: false, error: "element not found" });
target.focus();
const next = text ?? "";
if (target.isContentEditable) {
  target.textContent = next;
} else if (target instanceof HTMLSelectElement) {
  target.value = next;
} else {
  const proto = target instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  if (setter) setter.call(target, next); else target.value = next;
}
target.dispatchEvent(new Event("input", { bubbles: true }));
target.dispatchEvent(new Event("change", { bubbles: true }));
if (submit) {
  target.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  if (target.form && typeof target.form.requestSubmit === "function") target.form.requestSubmit();
}
return JSON.stringify({ ok: true });
"##;

    const SCROLL_SCRIPT: &str = r##"
const input = JSON.parse(args);
const dy = input.dy;
if (Date.now() - (window.__sirusHumanAt || 0) < 1500) return JSON.stringify({ interrupted: true });
window.scrollBy({ top: dy ?? 0, left: 0, behavior: "auto" });
return JSON.stringify({ ok: true, y: window.scrollY });
"##;

    /// A key on the focused element (ADR-087).
    const PRESS_SCRIPT: &str = r##"
const input = JSON.parse(args);
const key = input.key;
if (Date.now() - (window.__sirusHumanAt || 0) < 1500) return JSON.stringify({ interrupted: true });
const target = document.activeElement && document.activeElement !== document.documentElement ? document.activeElement : document.body;
const init = { key, code: key === " " ? "Space" : key, bubbles: true, cancelable: true };
const proceed = target.dispatchEvent(new KeyboardEvent("keydown", init));
if (proceed) {
  const field = target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement;
  if (key === "Enter") {
    if (target instanceof HTMLTextAreaElement) {
      target.setRangeText("\n", target.selectionStart ?? target.value.length, target.selectionEnd ?? target.value.length, "end");
      target.dispatchEvent(new Event("input", { bubbles: true }));
    } else {
      const form = target.form || (target.closest ? target.closest("form") : null);
      if (form) { if (form.requestSubmit) form.requestSubmit(); else form.submit(); }
      else if (target instanceof HTMLElement && !field) target.click();
    }
  } else if (key === "Backspace" && field && target.value) {
    const end = target.selectionEnd ?? target.value.length;
    const start = target.selectionStart === end ? Math.max(0, end - 1) : (target.selectionStart ?? end - 1);
    target.setRangeText("", start, end, "end");
    target.dispatchEvent(new Event("input", { bubbles: true }));
  } else if (key === "Tab") {
    const items = [...document.querySelectorAll("a[href],button,input,select,textarea,[tabindex]:not([tabindex='-1']),[contenteditable='true']")].filter((item) => !item.disabled && item.offsetParent !== null);
    const next = items[(items.indexOf(target) + 1) % Math.max(1, items.length)];
    if (next) next.focus();
  } else if (!field) {
    const page = window.innerHeight * 0.85;
    const moves = { ArrowDown: 80, ArrowUp: -80, PageDown: page, PageUp: -page, " ": page, Home: -1e9, End: 1e9 };
    if (key in moves) window.scrollBy({ top: moves[key], behavior: "auto" });
  }
}
target.dispatchEvent(new KeyboardEvent("keyup", init));
return JSON.stringify({ ok: true });
"##;

    /// Waits until a text or selector shows up, at most 8 s (ADR-087).
    const WAIT_SCRIPT: &str = r##"
const input = JSON.parse(args);
const deadline = Date.now() + Math.min(8000, Math.max(100, input.timeoutMs || 5000));
const found = () => input.selector ? Boolean(document.querySelector(input.selector)) : (document.body ? document.body.innerText : "").includes(input.text);
try { found(); } catch { return JSON.stringify({ ok: false, error: "invalid selector" }); }
while (Date.now() < deadline) {
  if (found()) return JSON.stringify({ ok: true });
  await new Promise((resolve) => setTimeout(resolve, 150));
}
return JSON.stringify({ ok: false, error: "timed out waiting" });
"##;

    /// Recent key presses and clicks since the last read, with the viewport they refer to.
    const INPUT_SCRIPT: &str = r#"(() => { const events = window.__sirusInput || []; window.__sirusInput = []; return JSON.stringify({ width: window.innerWidth, height: window.innerHeight, events }); })()"#;

    const LOGS_SCRIPT: &str = r##"
const input = JSON.parse(args);
const clear = input.clear;
const logs = window.__sirusLogs || [];
const result = JSON.stringify(logs.slice(-200));
if (clear) window.__sirusLogs = [];
return result;
"##;

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[ivars = BrowserDelegateIvars]
        struct BrowserNavigationDelegate;

        unsafe impl NSObjectProtocol for BrowserNavigationDelegate {}

        unsafe impl WKNavigationDelegate for BrowserNavigationDelegate {
            #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
            fn decide_policy(
                &self,
                _webview: &WKWebView,
                action: &WKNavigationAction,
                handler: &block2::Block<dyn Fn(WKNavigationActionPolicy)>,
            ) {
                let allowed = unsafe {
                    !action.shouldPerformDownload()
                        && action
                            .request()
                            .URL()
                            .and_then(|value| value.absoluteString())
                            .is_some_and(|value| is_allowed_browser_url(&value.to_string()))
                };
                (*handler).call((if allowed {
                    WKNavigationActionPolicy::Allow
                } else {
                    WKNavigationActionPolicy::Cancel
                },));
            }

            #[unsafe(method(webView:didStartProvisionalNavigation:))]
            fn did_start(&self, webview: &WKWebView, _navigation: Option<&WKNavigation>) {
                self.publish(webview, true);
            }

            #[unsafe(method(webView:didFinishNavigation:))]
            fn did_finish(&self, webview: &WKWebView, _navigation: Option<&WKNavigation>) {
                self.publish(webview, false);
                self.request_favicon(webview);
            }

            #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
            fn did_fail_provisional(
                &self,
                webview: &WKWebView,
                _navigation: Option<&WKNavigation>,
                error: &NSError,
            ) {
                self.publish(webview, false);
                self.show_load_error(webview, error);
            }

            #[unsafe(method(webView:didFailNavigation:withError:))]
            fn did_fail(
                &self,
                webview: &WKWebView,
                _navigation: Option<&WKNavigation>,
                _error: &NSError,
            ) {
                self.publish(webview, false);
            }

            /// The page's WebContent process died (memory, a crash): it reloads by itself, a
            /// few times at most (ADR-111).
            #[unsafe(method(webViewWebContentProcessDidTerminate:))]
            fn did_terminate(&self, webview: &WKWebView) {
                self.publish(webview, false);
                let ivars = self.ivars();
                let Some(delay) =
                    recovery_delay(&mut ivars.crashes.borrow_mut(), std::time::Instant::now())
                else {
                    return;
                };
                let (app, session_id, tab_id) = (
                    ivars.app.clone(),
                    ivars.session_id.clone(),
                    ivars.tab_id.clone(),
                );
                std::thread::spawn(move || {
                    std::thread::sleep(delay);
                    let main = app.clone();
                    let _ = main.run_on_main_thread(move || {
                        let target = with_manager(|manager| {
                            manager
                                .webview(&session_id, &tab_id)
                                .ok()
                                .zip(manager.tab_url(&session_id, &tab_id))
                        })
                        .ok()
                        .flatten();
                        let Some((webview, url)) = target else {
                            return;
                        };
                        unsafe {
                            if webview.reload().is_none() && is_allowed_browser_url(&url) {
                                if let Some(ns_url) =
                                    NSURL::URLWithString(&NSString::from_str(&url))
                                {
                                    webview.loadRequest(&NSURLRequest::requestWithURL(&ns_url));
                                }
                            }
                        }
                    });
                });
            }
        }

        unsafe impl WKScriptMessageHandler for BrowserNavigationDelegate {
            /// A password form the person submitted (ADR-186). The site is the address the
            /// webview loaded for that frame, not anything the page says.
            #[unsafe(method(userContentController:didReceiveScriptMessage:))]
            fn did_receive_script_message(
                &self,
                _controller: &WKUserContentController,
                message: &WKScriptMessage,
            ) {
                let ivars = self.ivars();
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
                if body.len() > 4096 {
                    return;
                }
                // No offers in a private tab; if the profile cannot be read, no offer either.
                let private = with_manager(|manager| {
                    manager.tab_profile(&ivars.session_id, &ivars.tab_id)
                        == crate::browser_profiles::PRIVATE_PROFILE
                })
                .unwrap_or(true);
                if !private {
                    crate::login_vault::note_submission(
                        &ivars.app,
                        &ivars.session_id,
                        &frame_url,
                        &body,
                    );
                }
            }
        }

        unsafe impl WKUIDelegate for BrowserNavigationDelegate {
            /// Popups (`target=_blank`, OAuth) become internal tabs instead of
            /// native windows; the popup itself is cancelled.
            #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
            fn create_webview(
                &self,
                _webview: &WKWebView,
                _configuration: &WKWebViewConfiguration,
                action: &WKNavigationAction,
                _features: &WKWindowFeatures,
            ) -> Option<Retained<WKWebView>> {
                let ivars = self.ivars();
                let url = unsafe {
                    action
                        .request()
                        .URL()
                        .and_then(|value| value.absoluteString())
                        .map(|value| value.to_string())
                        .unwrap_or_default()
                };
                if is_allowed_browser_url(&url) {
                    if let Some(mtm) = MainThreadMarker::new() {
                        let state = with_manager(|manager| {
                            manager.open_popup_tab(
                                &ivars.app,
                                &ivars.session_id,
                                &ivars.tab_id,
                                &url,
                                mtm,
                            )
                        })
                        .ok()
                        .flatten();
                        if let Some(state) = state {
                            let _ = ivars.app.emit("browser-state", state);
                        }
                    }
                }
                None
            }

            /// JavaScript dialogs are queued for the person or an agent to answer (ADR-150).
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
                    crate::browser_agent::platform::DialogHandler::Alert(handler),
                    message.to_string(),
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
                    crate::browser_agent::platform::DialogHandler::Confirm(handler),
                    message.to_string(),
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
                    crate::browser_agent::platform::DialogHandler::Prompt(handler, default),
                    prompt.to_string(),
                );
            }
        }
    );

    impl BrowserNavigationDelegate {
        fn dialog(
            &self,
            webview: &WKWebView,
            handler: crate::browser_agent::platform::DialogHandler<'_>,
            message: String,
        ) {
            let ivars = self.ivars();
            let url = unsafe {
                webview
                    .URL()
                    .and_then(|value| value.absoluteString())
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            };
            crate::browser_agent::platform::on_dialog(
                &ivars.app,
                &ivars.session_id,
                &ivars.tab_id,
                handler,
                message,
                url,
            );
        }

        fn new(
            app: &AppHandle,
            session_id: &str,
            tab_id: &str,
            mtm: MainThreadMarker,
        ) -> Retained<Self> {
            let delegate = mtm.alloc::<Self>().set_ivars(BrowserDelegateIvars {
                session_id: session_id.into(),
                tab_id: tab_id.into(),
                app: app.clone(),
                crashes: RefCell::new(vec![]),
            });
            unsafe { msg_send![super(delegate), init] }
        }

        /// A page that did not load shows why, with the address that failed (a
        /// redirect to a closed port names that port), instead of a blank tab.
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
                .filter(|url| is_allowed_browser_url(url) && url != "about:blank");
            let Some(failed) = failed else {
                return;
            };
            let ivars = self.ivars();
            let portuguese = {
                use tauri::Manager;
                let state = ivars
                    .app
                    .state::<std::sync::Arc<crate::commands::AppState>>();
                let locale = state.data.lock().settings.locale.clone();
                locale.starts_with("pt")
            };
            let reason = error.localizedDescription().to_string();
            let page = load_error_page(&failed, &reason, portuguese);
            let Some(base) = NSURL::URLWithString(&NSString::from_str(&failed)) else {
                return;
            };
            ERROR_PAGES
                .lock()
                .insert(format!("{}\u{0}{}", ivars.session_id, ivars.tab_id));
            unsafe {
                webview.loadHTMLString_baseURL(&NSString::from_str(&page), Some(&base));
            }
        }

        fn publish(&self, webview: &WKWebView, loading: bool) {
            let ivars = self.ivars();
            let state = with_manager(|manager| {
                manager.sync_from_webview(&ivars.session_id, &ivars.tab_id, webview, loading)
            })
            .ok()
            .flatten();
            if let Some(state) = state {
                let _ = ivars.app.emit("browser-state", state);
            }
        }

        fn request_favicon(&self, webview: &WKWebView) {
            let ivars = self.ivars();
            let app = ivars.app.clone();
            let session_id = ivars.session_id.clone();
            let tab_id = ivars.tab_id.clone();
            let handler = RcBlock::new(move |value: *mut AnyObject, _error: *mut NSError| {
                if value.is_null() {
                    return;
                }
                let text = unsafe { (*value.cast::<NSString>()).to_string() };
                if !is_allowed_favicon_url(&text) {
                    return;
                }
                let state =
                    with_manager(|manager| manager.set_favicon(&session_id, &tab_id, &text))
                        .ok()
                        .flatten();
                if let Some(state) = state {
                    let _ = app.emit("browser-state", state);
                }
            });
            unsafe {
                webview.evaluateJavaScript_completionHandler(
                    &NSString::from_str(FAVICON_SCRIPT),
                    Some(&handler),
                );
            }
        }
    }

    fn evaluate_script(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
        script: &'static str,
    ) -> Result<String> {
        let (sender, receiver) = mpsc::channel::<String>();
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        on_main(app, move || -> Result<()> {
            let webview = with_manager(|manager| manager.webview(&session_id, &tab_id))??;
            let handler = RcBlock::new(move |value: *mut AnyObject, _error: *mut NSError| {
                let text = if value.is_null() {
                    String::new()
                } else {
                    unsafe { (*value.cast::<NSString>()).to_string() }
                };
                let _ = sender.send(text);
            });
            unsafe {
                webview.evaluateJavaScript_completionHandler(
                    &NSString::from_str(script),
                    Some(&handler),
                );
            }
            Ok(())
        })??;
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| Error::new("native", "the page did not answer in time"))
    }

    pub fn annotate_start(app: &AppHandle, session_id: &str, tab_id: &str) -> Result<()> {
        evaluate_script(app, session_id, tab_id, ANNOTATE_START_SCRIPT).map(|_| ())
    }

    pub fn annotate_finish(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
    ) -> Result<Vec<BrowserAnnotation>> {
        let raw = evaluate_script(app, session_id, tab_id, ANNOTATE_FINISH_SCRIPT)?;
        Ok(parse_browser_annotations(&raw))
    }

    pub fn annotate_cancel(app: &AppHandle, session_id: &str, tab_id: &str) -> Result<()> {
        evaluate_script(app, session_id, tab_id, ANNOTATE_CANCEL_SCRIPT).map(|_| ())
    }

    /// Annotations with a screenshot (ADR-151): measures the marked elements, takes the
    /// visible page with its numbered markers, then removes the overlay.
    pub fn annotate_capture(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
    ) -> Result<super::BrowserAnnotationCapture> {
        let raw = evaluate_script(app, session_id, tab_id, ANNOTATE_MEASURE_SCRIPT)?;
        let mut result = super::parse_annotation_measure(&raw);
        if !result.elements.is_empty() {
            result.png = capture(app, session_id, tab_id).ok();
        }
        let _ = evaluate_script(app, session_id, tab_id, ANNOTATE_FINISH_SCRIPT);
        Ok(result)
    }

    /// Agent-opened pages (ADR-151): the window shows them in its floating player.
    fn reveal_agent_page(app: &AppHandle, session_id: &str, url: &str) {
        if is_allowed_browser_url(url) && !url.trim().eq_ignore_ascii_case("about:blank") {
            let _ = app.emit(
                "browser-agent-page",
                serde_json::json!({ "sessionId": session_id, "url": url }),
            );
        }
    }

    pub fn copy_link(app: &AppHandle, session_id: &str, tab_id: &str) -> Result<()> {
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        on_main(app, move || -> Result<()> {
            let url = with_manager(|manager| manager.tab_url(&session_id, &tab_id))
                .ok()
                .flatten()
                .filter(|url| is_allowed_browser_url(url))
                .ok_or_else(|| Error::new("invalid", "There is no page to copy."))?;
            unsafe {
                let pasteboard = NSPasteboard::generalPasteboard();
                pasteboard.clearContents();
                pasteboard.setString_forType(&NSString::from_str(&url), NSPasteboardTypeString);
            }
            Ok(())
        })?
    }

    pub fn capture(app: &AppHandle, session_id: &str, tab_id: &str) -> Result<String> {
        use base64::Engine as _;
        let (sender, receiver) = mpsc::channel::<Result<String>>();
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        on_main(app, move || -> Result<()> {
            let webview = with_manager(|manager| manager.webview(&session_id, &tab_id))??;
            let handler = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
                let result = unsafe {
                    if image.is_null() {
                        let message = error
                            .as_ref()
                            .map(|error| error.localizedDescription().to_string())
                            .unwrap_or_else(|| "Could not capture the page.".into());
                        Err(Error::new("native", message))
                    } else {
                        let image = image.as_ref().expect("checked");
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
                            Some(png) if png.len() <= 8 * 1024 * 1024 => {
                                Ok(base64::engine::general_purpose::STANDARD.encode(png.to_vec()))
                            }
                            Some(_) => Err(Error::new("native", "The screenshot is too large.")),
                            None => Err(Error::new("native", "Could not encode the screenshot.")),
                        }
                    }
                };
                let _ = sender.send(result);
            });
            unsafe {
                webview.takeSnapshotWithConfiguration_completionHandler(None, &handler);
            }
            Ok(())
        })??;
        receiver
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::new("native", "the page did not answer in time"))?
    }

    pub(crate) fn call_page(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
        body: String,
        args: serde_json::Value,
    ) -> Result<String> {
        let (sender, receiver) = mpsc::channel::<String>();
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        let json = serde_json::to_string(&args)
            .map_err(|_| Error::new("invalid", "Invalid page arguments."))?;
        on_main(app, move || -> Result<()> {
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| Error::new("native", "the browser must run on the main thread"))?;
            let webview = with_manager(|manager| manager.webview(&session_id, &tab_id))??;
            let dictionary: Retained<NSMutableDictionary<NSString, AnyObject>> =
                NSMutableDictionary::new();
            let value = NSString::from_str(&json);
            let object: &AnyObject = value.as_ref();
            let key = NSString::from_str("args");
            // SAFETY: this dictionary has NSString keys, and key implements NSCopying.
            unsafe { dictionary.setObject_forKey(object, ProtocolObject::from_ref(&*key)) };
            let handler = RcBlock::new(move |value: *mut AnyObject, _error: *mut NSError| {
                let text = if value.is_null() {
                    String::new()
                } else {
                    unsafe { (*value.cast::<NSString>()).to_string() }
                };
                let _ = sender.send(text);
            });
            unsafe {
                webview.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                    &NSString::from_str(&body),
                    Some(&dictionary),
                    None,
                    &WKContentWorld::pageWorld(mtm),
                    Some(&handler),
                );
            }
            Ok(())
        })??;
        receiver
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::new("native", "the page did not answer in time"))
    }

    fn active_tab(app: &AppHandle, session_id: &str) -> Result<String> {
        let session_id = session_id.to_string();
        on_main(app, move || -> Result<String> {
            let state = with_manager(|manager| manager.state(&session_id))?;
            state
                .active_tab_id
                .ok_or_else(|| Error::new("invalid", "No browser tab is open."))
        })?
    }

    pub(crate) fn opened_active_tab(app: &AppHandle, session_id: &str) -> Result<String> {
        if let Ok(tab_id) = active_tab(app, session_id) {
            return Ok(tab_id);
        }
        let state = open(app, session_id)?;
        state
            .active_tab_id
            .ok_or_else(|| Error::new("invalid", "No browser tab is open."))
    }

    pub(crate) fn page_action(
        app: &AppHandle,
        session_id: &str,
        body: &'static str,
        args: serde_json::Value,
    ) -> Result<String> {
        let tab_id = opened_active_tab(app, session_id)?;
        call_page(app, session_id, &tab_id, body.to_string(), args)
    }

    pub fn mcp_status(app: &AppHandle, session_id: &str) -> Result<BrowserSessionState> {
        state(app, session_id)
    }

    pub fn mcp_open(
        app: &AppHandle,
        session_id: &str,
        url: Option<String>,
    ) -> Result<BrowserSessionState> {
        let state = open(app, session_id)?;
        let Some(url) = url.filter(|url| is_allowed_browser_url(url)) else {
            return Ok(state);
        };
        let opened = match state.active_tab_id {
            Some(tab_id)
                if !state
                    .tabs
                    .iter()
                    .any(|tab| tab.id == tab_id && tab.url == url) =>
            {
                navigate(app, session_id, &tab_id, &url)
            }
            _ => Ok(state),
        }?;
        reveal_agent_page(app, session_id, &url);
        Ok(opened)
    }

    pub fn mcp_navigate(
        app: &AppHandle,
        session_id: &str,
        url: &str,
    ) -> Result<BrowserSessionState> {
        let tab_id = opened_active_tab(app, session_id)?;
        let state = navigate(app, session_id, &tab_id, url)?;
        reveal_agent_page(app, session_id, url);
        Ok(state)
    }

    pub fn mcp_back(app: &AppHandle, session_id: &str) -> Result<BrowserSessionState> {
        let tab_id = active_tab(app, session_id)?;
        back(app, session_id, &tab_id)
    }

    pub fn mcp_forward(app: &AppHandle, session_id: &str) -> Result<BrowserSessionState> {
        let tab_id = active_tab(app, session_id)?;
        forward(app, session_id, &tab_id)
    }

    pub fn mcp_reload(app: &AppHandle, session_id: &str) -> Result<BrowserSessionState> {
        let tab_id = active_tab(app, session_id)?;
        reload(app, session_id, &tab_id)
    }

    pub fn mcp_close(app: &AppHandle, session_id: &str) -> Result<()> {
        close(app, session_id)
    }

    pub fn mcp_capture(app: &AppHandle, session_id: &str) -> Result<String> {
        let tab_id = active_tab(app, session_id)?;
        capture(app, session_id, &tab_id)
    }

    pub fn mcp_snapshot(app: &AppHandle, session_id: &str) -> Result<String> {
        page_action(app, session_id, SNAPSHOT_SCRIPT, serde_json::json!({}))
    }

    pub fn mcp_click(
        app: &AppHandle,
        session_id: &str,
        reference: &str,
        selector: &str,
    ) -> Result<String> {
        page_action(
            app,
            session_id,
            CLICK_SCRIPT,
            serde_json::json!({ "ref": reference, "selector": selector }),
        )
    }

    pub fn mcp_type(
        app: &AppHandle,
        session_id: &str,
        reference: &str,
        selector: &str,
        text: &str,
        submit: bool,
    ) -> Result<String> {
        page_action(
            app,
            session_id,
            TYPE_SCRIPT,
            serde_json::json!({ "ref": reference, "selector": selector, "text": text, "submit": submit }),
        )
    }

    pub fn mcp_press(app: &AppHandle, session_id: &str, key: &str) -> Result<String> {
        page_action(
            app,
            session_id,
            PRESS_SCRIPT,
            serde_json::json!({ "key": key }),
        )
    }

    pub fn mcp_wait_for(
        app: &AppHandle,
        session_id: &str,
        text: &str,
        selector: &str,
        timeout_ms: u64,
    ) -> Result<String> {
        page_action(
            app,
            session_id,
            WAIT_SCRIPT,
            serde_json::json!({ "text": text, "selector": selector, "timeoutMs": timeout_ms }),
        )
    }

    pub fn mcp_scroll(app: &AppHandle, session_id: &str, dy: f64) -> Result<String> {
        page_action(
            app,
            session_id,
            SCROLL_SCRIPT,
            serde_json::json!({ "dy": dy }),
        )
    }

    pub fn mcp_logs(app: &AppHandle, session_id: &str, clear: bool) -> Result<String> {
        page_action(
            app,
            session_id,
            LOGS_SCRIPT,
            serde_json::json!({ "clear": clear }),
        )
    }

    pub fn open(app: &AppHandle, session_id: &str) -> Result<BrowserSessionState> {
        let profile = crate::browser_profiles::resolve_for(app, None)?;
        let main_app = app.clone();
        let session_id = session_id.to_string();
        let pointer = main_window_pointer(app)?;
        on_main(app, move || -> Result<BrowserSessionState> {
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| Error::new("native", "the browser must start on the main thread"))?;
            let (content, height) = window_content(pointer)?;
            with_manager(|manager| {
                manager.open(&main_app, &session_id, &content, height, mtm, &profile)
            })?
        })?
    }

    pub fn close(app: &AppHandle, session_id: &str) -> Result<()> {
        let session_id = session_id.to_string();
        on_main(app, move || {
            with_manager(|manager| manager.close(&session_id))
        })?
    }

    pub fn state(app: &AppHandle, session_id: &str) -> Result<BrowserSessionState> {
        let session_id = session_id.to_string();
        on_main(app, move || {
            with_manager(|manager| manager.state(&session_id))
                .unwrap_or_else(|_| BrowserSessionState::closed(&session_id))
        })
    }

    fn session_operation(
        app: &AppHandle,
        operation: impl FnOnce(&mut BrowserManager, f64) -> Result<BrowserSessionState> + Send + 'static,
    ) -> Result<BrowserSessionState> {
        let pointer = main_window_pointer(app)?;
        on_main(app, move || -> Result<BrowserSessionState> {
            let (_, height) = window_content(pointer)?;
            with_manager(|manager| operation(manager, height))?
        })?
    }

    /// A tab in `profile` (a profile id), or the default profile from settings (ADR-150).
    pub fn new_tab(
        app: &AppHandle,
        session_id: &str,
        url: Option<String>,
        profile: Option<String>,
    ) -> Result<BrowserSessionState> {
        let profile = crate::browser_profiles::resolve_for(app, profile.as_deref())?;
        let main_app = app.clone();
        let session_id = session_id.to_string();
        let pointer = main_window_pointer(app)?;
        on_main(app, move || -> Result<BrowserSessionState> {
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| Error::new("native", "the browser must start on the main thread"))?;
            let (_, height) = window_content(pointer)?;
            with_manager(|manager| {
                manager.new_tab(&main_app, &session_id, url.clone(), &profile, height, mtm)
            })?
        })?
    }

    pub fn close_tab(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
    ) -> Result<BrowserSessionState> {
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        session_operation(app, move |manager, height| {
            manager.close_tab(&session_id, &tab_id, height)
        })
    }

    pub fn select_tab(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
    ) -> Result<BrowserSessionState> {
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        session_operation(app, move |manager, height| {
            manager.select_tab(&session_id, &tab_id, height)
        })
    }

    pub fn navigate(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
        url: &str,
    ) -> Result<BrowserSessionState> {
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        let url = url.to_string();
        session_operation(app, move |manager, _| {
            manager.navigate(&session_id, &tab_id, &url)
        })
    }

    pub fn reload(app: &AppHandle, session_id: &str, tab_id: &str) -> Result<BrowserSessionState> {
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        session_operation(app, move |manager, _| manager.reload(&session_id, &tab_id))
    }

    pub fn back(app: &AppHandle, session_id: &str, tab_id: &str) -> Result<BrowserSessionState> {
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        session_operation(app, move |manager, _| {
            manager.history(&session_id, &tab_id, false)
        })
    }

    pub fn forward(app: &AppHandle, session_id: &str, tab_id: &str) -> Result<BrowserSessionState> {
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        session_operation(app, move |manager, _| {
            manager.history(&session_id, &tab_id, true)
        })
    }

    pub fn set_zoom(app: &AppHandle, session_id: &str, zoom: f64) -> Result<()> {
        let session_id = session_id.to_string();
        on_main(app, move || {
            with_manager(|manager| manager.set_zoom(&session_id, zoom))
        })?
    }

    fn emit_state(app: &AppHandle, state: &BrowserSessionState) {
        let _ = app.emit("browser-state", state.clone());
    }

    pub fn set_device(
        app: &AppHandle,
        session_id: &str,
        device: Option<BrowserDevice>,
    ) -> Result<BrowserSessionState> {
        let session_id = session_id.to_string();
        let state = on_main(app, move || {
            with_manager(|manager| manager.set_device(&session_id, device))
        })??
        .ok_or_else(|| Error::not_found("the browser is not open for this session"))?;
        emit_state(app, &state);
        Ok(state)
    }

    pub fn set_user_zoom(
        app: &AppHandle,
        session_id: &str,
        zoom: f64,
    ) -> Result<BrowserSessionState> {
        let session_id = session_id.to_string();
        on_main(app, move || {
            with_manager(|manager| manager.set_user_zoom(&session_id, zoom))
        })??
        .ok_or_else(|| Error::not_found("the browser is not open for this session"))
    }

    pub fn set_appearance(
        app: &AppHandle,
        session_id: &str,
        mode: &str,
    ) -> Result<BrowserSessionState> {
        let session_id = session_id.to_string();
        let mode = mode.to_string();
        on_main(app, move || {
            with_manager(|manager| manager.set_appearance(&session_id, &mode))
        })??
        .ok_or_else(|| Error::not_found("the browser is not open for this session"))
    }

    /// Reload bypassing the cache (ADR-112).
    pub fn hard_reload(app: &AppHandle, session_id: &str, tab_id: &str) -> Result<()> {
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        on_main(app, move || -> Result<()> {
            let webview = with_manager(|manager| manager.webview(&session_id, &tab_id))??;
            unsafe { webview.reloadFromOrigin() };
            Ok(())
        })?
    }

    /// Opens Safari's Web Inspector for the tab (WebKit's private `_inspector`, as Tauri does).
    pub fn devtools(app: &AppHandle, session_id: &str, tab_id: &str) -> Result<()> {
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        on_main(app, move || -> Result<()> {
            let webview = with_manager(|manager| manager.webview(&session_id, &tab_id))??;
            if !NSObject::respondsToSelector(&webview, objc2::sel!(_inspector)) {
                return Err(Error::new(
                    "unsupported",
                    "The Web Inspector is unavailable.",
                ));
            }
            unsafe {
                let inspector: *mut AnyObject = msg_send![&*webview, _inspector];
                if inspector.is_null() {
                    return Err(Error::new(
                        "unsupported",
                        "The Web Inspector is unavailable.",
                    ));
                }
                let _: () = msg_send![inspector, show];
            }
            Ok(())
        })?
    }

    /// Clears cookies or the cache of the shared browser data store (every session's browser).
    pub fn clear_data(app: &AppHandle, cookies: bool) -> Result<()> {
        use objc2_foundation::{NSDate, NSSet};
        use objc2_web_kit::{
            WKWebsiteDataTypeCookies, WKWebsiteDataTypeDiskCache, WKWebsiteDataTypeFetchCache,
            WKWebsiteDataTypeMemoryCache,
        };
        let (sender, receiver) = mpsc::channel::<()>();
        on_main(app, move || -> Result<()> {
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| Error::new("native", "the browser must run on the main thread"))?;
            let types = unsafe {
                if cookies {
                    NSSet::from_slice(&[WKWebsiteDataTypeCookies])
                } else {
                    NSSet::from_slice(&[
                        WKWebsiteDataTypeDiskCache,
                        WKWebsiteDataTypeMemoryCache,
                        WKWebsiteDataTypeFetchCache,
                    ])
                }
            };
            let done = RcBlock::new(move || {
                let _ = sender.send(());
            });
            unsafe {
                WKWebsiteDataStore::defaultDataStore(mtm)
                    .removeDataOfTypes_modifiedSince_completionHandler(
                        &types,
                        &NSDate::distantPast(),
                        &done,
                    );
            }
            Ok(())
        })??;
        receiver
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::new("native", "the browser did not answer in time"))
    }

    /// Key presses and clicks since the last read (JSON), for recordings (ADR-112).
    pub fn input_events(app: &AppHandle, session_id: &str, tab_id: &str) -> Result<String> {
        evaluate_script(app, session_id, tab_id, INPUT_SCRIPT)
    }

    /// A page's dialog, queued on its session. `None` when the session is gone; the dialog
    /// is then dismissed by its drop.
    pub fn queue_dialog(
        session_id: &str,
        pending: crate::browser_agent::platform::PendingDialog,
    ) -> Option<BrowserSessionState> {
        let session_id = session_id.to_string();
        with_manager(|manager| manager.queue_dialog(&session_id, pending))
            .ok()
            .flatten()
    }

    /// Takes the session's open dialog to answer it (main thread).
    pub fn take_dialog(
        session_id: &str,
        tab_id: Option<&str>,
    ) -> Option<(
        crate::browser_agent::platform::PendingDialog,
        BrowserSessionState,
    )> {
        let session_id = session_id.to_string();
        let tab_id = tab_id.map(str::to_string);
        with_manager(|manager| manager.take_dialog(&session_id, tab_id.as_deref()))
            .ok()
            .flatten()
    }

    /// Closes the tabs of a deleted profile (main thread); the states go to the pane.
    pub fn close_profile_tabs(profile: &str) -> Vec<BrowserSessionState> {
        let profile = profile.to_string();
        with_manager(|manager| manager.close_profile_tabs(&profile)).unwrap_or_default()
    }

    /// Device mode for an agent: opens the browser if needed.
    pub fn mcp_device(
        app: &AppHandle,
        session_id: &str,
        device: Option<BrowserDevice>,
    ) -> Result<BrowserSessionState> {
        opened_active_tab(app, session_id)?;
        set_device(app, session_id, device)
    }

    pub fn picture_in_picture(
        app: &AppHandle,
        session_id: &str,
        open: bool,
    ) -> Result<BrowserSessionState> {
        let main_app = app.clone();
        let session_id = session_id.to_string();
        let pointer = main_window_pointer(app)?;
        on_main(app, move || -> Result<BrowserSessionState> {
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| Error::new("native", "the browser must run on the main thread"))?;
            let (_, height) = window_content(pointer)?;
            with_manager(|manager| -> Result<BrowserSessionState> {
                if open {
                    manager.open_pip(&main_app, &session_id, mtm)?;
                } else {
                    manager.close_pip(&session_id, height);
                }
                Ok(manager.state(&session_id))
            })?
        })?
    }

    /// One JPEG frame of the page at most `max_width` wide, for recording (ADR-111).
    pub fn frame(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
        max_width: f64,
    ) -> Result<String> {
        use base64::Engine as _;
        let (sender, receiver) = mpsc::channel::<Result<String>>();
        let session_id = session_id.to_string();
        let tab_id = tab_id.to_string();
        on_main(app, move || -> Result<()> {
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| Error::new("native", "the browser must run on the main thread"))?;
            let webview = with_manager(|manager| manager.webview(&session_id, &tab_id))??;
            let width = webview
                .frame()
                .size
                .width
                .min(max_width.clamp(160.0, 1920.0));
            let handler = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
                let result = unsafe { image.as_ref() }
                    .and_then(|image| image.TIFFRepresentation())
                    .and_then(|data| {
                        NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &data)
                    })
                    .and_then(|rep| unsafe {
                        let quality = NSNumber::new_f64(0.7);
                        let properties = NSDictionary::from_slices(
                            &[NSImageCompressionFactor],
                            &[&*quality as &AnyObject],
                        );
                        rep.representationUsingType_properties(
                            NSBitmapImageFileType::JPEG,
                            &properties,
                        )
                    })
                    .map(|jpeg| base64::engine::general_purpose::STANDARD.encode(jpeg.to_vec()))
                    .ok_or_else(|| Error::new("native", "Could not capture the page."));
                let _ = sender.send(result);
            });
            unsafe {
                let config = WKSnapshotConfiguration::new(mtm);
                // A hidden page (pane closed, an agent recording) paints only when asked.
                config.setAfterScreenUpdates(webview.isHidden());
                config.setSnapshotWidth(Some(&NSNumber::new_f64(width)));
                webview.takeSnapshotWithConfiguration_completionHandler(Some(&config), &handler);
            }
            Ok(())
        })??;
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| Error::new("native", "the page did not answer in time"))?
    }

    pub fn set_bounds(
        app: &AppHandle,
        session_id: &str,
        bounds: Option<BrowserBounds>,
    ) -> Result<()> {
        let session_id = session_id.to_string();
        let pointer = main_window_pointer(app).ok();
        on_main(app, move || {
            let height = pointer
                .and_then(|pointer| window_content(pointer).ok())
                .map(|(_, height)| height)
                .unwrap_or(0.0);
            with_manager(|manager| manager.set_bounds(&session_id, bounds.clone(), height))
        })?
    }
}

#[cfg(all(not(target_os = "macos"), feature = "desktop"))]
mod platform {
    use tauri::AppHandle;

    use super::{BrowserAnnotation, BrowserAnnotationCapture, BrowserBounds, BrowserSessionState};
    use crate::error::{Error, Result};

    fn unsupported() -> Error {
        Error::new(
            "unsupported",
            "The embedded browser is only available on macOS.",
        )
    }

    pub fn open(_app: &AppHandle, _session_id: &str) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn close(_app: &AppHandle, _session_id: &str) -> Result<()> {
        Err(unsupported())
    }

    pub fn state(_app: &AppHandle, session_id: &str) -> Result<BrowserSessionState> {
        Ok(BrowserSessionState::closed(session_id))
    }

    pub fn new_tab(
        _app: &AppHandle,
        _session_id: &str,
        _url: Option<String>,
        _profile: Option<String>,
    ) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn close_tab(
        _app: &AppHandle,
        _session_id: &str,
        _tab_id: &str,
    ) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn select_tab(
        _app: &AppHandle,
        _session_id: &str,
        _tab_id: &str,
    ) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn navigate(
        _app: &AppHandle,
        _session_id: &str,
        _tab_id: &str,
        _url: &str,
    ) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn reload(
        _app: &AppHandle,
        _session_id: &str,
        _tab_id: &str,
    ) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn back(_app: &AppHandle, _session_id: &str, _tab_id: &str) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn forward(
        _app: &AppHandle,
        _session_id: &str,
        _tab_id: &str,
    ) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn set_bounds(
        _app: &AppHandle,
        _session_id: &str,
        _bounds: Option<BrowserBounds>,
    ) -> Result<()> {
        Err(unsupported())
    }

    pub fn annotate_start(_app: &AppHandle, _session_id: &str, _tab_id: &str) -> Result<()> {
        Err(unsupported())
    }

    pub fn annotate_finish(
        _app: &AppHandle,
        _session_id: &str,
        _tab_id: &str,
    ) -> Result<Vec<BrowserAnnotation>> {
        Err(unsupported())
    }

    pub fn annotate_cancel(_app: &AppHandle, _session_id: &str, _tab_id: &str) -> Result<()> {
        Err(unsupported())
    }

    pub fn annotate_capture(
        _app: &AppHandle,
        _session_id: &str,
        _tab_id: &str,
    ) -> Result<super::BrowserAnnotationCapture> {
        Err(unsupported())
    }

    pub fn copy_link(_app: &AppHandle, _session_id: &str, _tab_id: &str) -> Result<()> {
        Err(unsupported())
    }

    pub fn capture(_app: &AppHandle, _session_id: &str, _tab_id: &str) -> Result<String> {
        Err(unsupported())
    }

    pub fn set_zoom(_app: &AppHandle, _session_id: &str, _zoom: f64) -> Result<()> {
        Err(unsupported())
    }

    pub fn set_device(
        _app: &AppHandle,
        _session_id: &str,
        _device: Option<super::BrowserDevice>,
    ) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn set_user_zoom(
        _app: &AppHandle,
        _session_id: &str,
        _zoom: f64,
    ) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn set_appearance(
        _app: &AppHandle,
        _session_id: &str,
        _mode: &str,
    ) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn hard_reload(_app: &AppHandle, _session_id: &str, _tab_id: &str) -> Result<()> {
        Err(unsupported())
    }

    pub fn devtools(_app: &AppHandle, _session_id: &str, _tab_id: &str) -> Result<()> {
        Err(unsupported())
    }

    pub fn clear_data(_app: &AppHandle, _cookies: bool) -> Result<()> {
        Err(unsupported())
    }

    pub fn input_events(_app: &AppHandle, _session_id: &str, _tab_id: &str) -> Result<String> {
        Err(unsupported())
    }

    pub fn picture_in_picture(
        _app: &AppHandle,
        _session_id: &str,
        _open: bool,
    ) -> Result<BrowserSessionState> {
        Err(unsupported())
    }

    pub fn frame(
        _app: &AppHandle,
        _session_id: &str,
        _tab_id: &str,
        _max_width: f64,
    ) -> Result<String> {
        Err(unsupported())
    }
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_open(
    app: tauri::AppHandle,
    state: tauri::State<'_, std::sync::Arc<crate::commands::AppState>>,
    session_id: String,
) -> Result<BrowserSessionState> {
    let app_state = state.inner().clone();
    if !app_state
        .data
        .lock()
        .sessions
        .iter()
        .any(|session| session.id == session_id)
    {
        return Err(Error::not_found("session not found"));
    }
    crate::commands::native_task(move || platform::open(&app, &session_id)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_close(app: tauri::AppHandle, session_id: String) -> Result<()> {
    crate::commands::native_task(move || platform::close(&app, &session_id)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_state(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<BrowserSessionState> {
    crate::commands::native_task(move || platform::state(&app, &session_id)).await
}

/// A new tab, in a browser profile (ADR-150); `None` is the profile set in Settings.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_new_tab(
    app: tauri::AppHandle,
    session_id: String,
    url: Option<String>,
    profile: Option<String>,
) -> Result<BrowserSessionState> {
    crate::commands::native_task(move || platform::new_tab(&app, &session_id, url, profile)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_close_tab(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<BrowserSessionState> {
    crate::commands::native_task(move || platform::close_tab(&app, &session_id, &tab_id)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_select_tab(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<BrowserSessionState> {
    crate::commands::native_task(move || platform::select_tab(&app, &session_id, &tab_id)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_navigate(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
    url: String,
) -> Result<BrowserSessionState> {
    crate::commands::native_task(move || platform::navigate(&app, &session_id, &tab_id, &url)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_reload(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<BrowserSessionState> {
    crate::commands::native_task(move || platform::reload(&app, &session_id, &tab_id)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_back(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<BrowserSessionState> {
    crate::commands::native_task(move || platform::back(&app, &session_id, &tab_id)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_forward(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<BrowserSessionState> {
    crate::commands::native_task(move || platform::forward(&app, &session_id, &tab_id)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_set_bounds(
    app: tauri::AppHandle,
    session_id: String,
    bounds: Option<BrowserBounds>,
) -> Result<()> {
    crate::commands::native_task(move || platform::set_bounds(&app, &session_id, bounds)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_annotate_start(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<()> {
    crate::commands::native_task(move || platform::annotate_start(&app, &session_id, &tab_id)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_annotate_finish(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<Vec<BrowserAnnotation>> {
    crate::commands::native_task(move || platform::annotate_finish(&app, &session_id, &tab_id))
        .await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_annotate_cancel(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<()> {
    crate::commands::native_task(move || platform::annotate_cancel(&app, &session_id, &tab_id))
        .await
}

/// Annotations with a screenshot (ADR-151): the marked elements, the page URL and a PNG.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_annotate_capture(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<BrowserAnnotationCapture> {
    crate::commands::native_task(move || platform::annotate_capture(&app, &session_id, &tab_id))
        .await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_copy_link(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<()> {
    crate::commands::native_task(move || platform::copy_link(&app, &session_id, &tab_id)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_capture(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<String> {
    crate::commands::native_task(move || platform::capture(&app, &session_id, &tab_id)).await
}

/// Device mode (ADR-111): the page lays out at the device's CSS width inside a smaller frame.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_set_zoom(app: tauri::AppHandle, session_id: String, zoom: f64) -> Result<()> {
    crate::commands::native_task(move || platform::set_zoom(&app, &session_id, zoom)).await
}

/// Floats the active tab in an always-on-top window, or puts it back (ADR-111).
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_picture_in_picture(
    app: tauri::AppHandle,
    session_id: String,
    open: bool,
) -> Result<BrowserSessionState> {
    crate::commands::native_task(move || platform::picture_in_picture(&app, &session_id, open))
        .await
}

/// One JPEG frame of the page, base64, for the renderer's recorder (ADR-111).
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_frame(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
    max_width: f64,
) -> Result<String> {
    crate::commands::native_task(move || platform::frame(&app, &session_id, &tab_id, max_width))
        .await
}

/// Device mode from the pane (ADR-111): `None` is the pane's own size.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_set_device(
    app: tauri::AppHandle,
    session_id: String,
    device: Option<BrowserDevice>,
) -> Result<BrowserSessionState> {
    let device = device.map(|device| BrowserDevice {
        width: device.width.clamp(240, 3840),
        height: device.height.clamp(240, 3840),
        preset: device.preset,
    });
    crate::commands::native_task(move || platform::set_device(&app, &session_id, device)).await
}

/// The person's page zoom (ADR-112).
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_set_user_zoom(
    app: tauri::AppHandle,
    session_id: String,
    zoom: f64,
) -> Result<BrowserSessionState> {
    crate::commands::native_task(move || platform::set_user_zoom(&app, &session_id, zoom)).await
}

/// `system`, `light` or `dark` for pages' `prefers-color-scheme` (ADR-112).
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_set_appearance(
    app: tauri::AppHandle,
    session_id: String,
    mode: String,
) -> Result<BrowserSessionState> {
    crate::commands::native_task(move || platform::set_appearance(&app, &session_id, &mode)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_hard_reload(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<()> {
    crate::commands::native_task(move || platform::hard_reload(&app, &session_id, &tab_id)).await
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_devtools(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<()> {
    crate::commands::native_task(move || platform::devtools(&app, &session_id, &tab_id)).await
}

/// `cookies` or `cache`, for every session's browser (they share one data store).
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_clear_data(app: tauri::AppHandle, kind: String) -> Result<()> {
    let cookies = match kind.as_str() {
        "cookies" => true,
        "cache" => false,
        _ => return Err(Error::new("invalid", "Unknown browser data.")),
    };
    crate::commands::native_task(move || platform::clear_data(&app, cookies)).await
}

/// Key presses and clicks since the last read, for recordings (ADR-112).
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_input_events(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
) -> Result<String> {
    crate::commands::native_task(move || platform::input_events(&app, &session_id, &tab_id)).await
}
