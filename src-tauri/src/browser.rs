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
#[allow(dead_code)]
fn ignorable_load_error(domain: &str, code: isize) -> bool {
    (domain == "NSURLErrorDomain" && code == -999) || (domain == "WebKitErrorDomain" && code == 102)
}

#[allow(dead_code)]
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page shown when an address does not load (server down, redirect to a
/// closed port, no network), instead of a blank tab: the address that failed,
/// the system's reason and a retry that requests it again.
#[allow(dead_code)]
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
