//! HTML an agent wrote, rendered live in the transcript (ADR-072). The page is served from
//! memory on its own `sirus-preview://` origin with a CSP that allows its inline scripts and
//! styles but no network fetches, inside a sandboxed iframe (no same-origin): it cannot reach
//! the app, its IPC or the person's files. Nothing is written to disk.
//!
//! Inline visuals (ADR-123) use the same store under `/v/<id>`, with a stricter policy: the
//! front end builds their document (theme, bridge script, then the agent's HTML), and the
//! page may also load public https scripts and stylesheets, but never connect, frame,
//! submit forms or start workers.
use std::sync::Mutex;

use crate::runtime::http::{Request, Response};
use sha2::{Digest, Sha256};

const MAX_PAGE: usize = 2 * 1024 * 1024;
const MAX_PAGES: usize = 48;
const CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob: https:; font-src data: https:; media-src data: blob: https:";
/// Inline visuals (ADR-123). `connect-src 'none'` blocks fetch, XHR, WebSockets and
/// EventSource; https covers public images, fonts, stylesheets and scripts (chart
/// libraries); eval adds nothing an inline script cannot already do in an opaque origin.
pub const VISUAL_CSP: &str = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' https:; style-src 'unsafe-inline' https:; img-src data: blob: https:; font-src data: https:; media-src data: blob: https:; connect-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; object-src 'none'; manifest-src 'none'; base-uri 'none'; form-action 'none'";
/// Reports the page height to the transcript so the frame fits its content.
const SIZE_REPORTER: &str = "<script>(()=>{const post=()=>parent.postMessage({sirusPreviewHeight:Math.ceil(document.documentElement.scrollHeight)},'*');new ResizeObserver(post).observe(document.documentElement);addEventListener('load',post);post();})()</script>";

#[derive(Default)]
pub struct HtmlPreviews(Mutex<Vec<(String, String)>>);

impl HtmlPreviews {
    /// Keeps the page under a content id (the same HTML reuses its entry); oldest pages leave first.
    pub fn put(&self, html: String) -> Result<String, String> {
        if html.len() > MAX_PAGE {
            return Err("The HTML preview is larger than 2 MiB.".into());
        }
        let id = hex(&Sha256::digest(html.as_bytes())[..12]);
        let mut pages = self
            .0
            .lock()
            .map_err(|_| "preview store unavailable".to_string())?;
        pages.retain(|(key, _)| key != &id);
        if pages.len() >= MAX_PAGES {
            pages.remove(0);
        }
        pages.push((id.clone(), html));
        Ok(id)
    }
    fn get(&self, id: &str) -> Option<String> {
        let pages = self.0.lock().ok()?;
        pages
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, html)| html.clone())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Registers an HTML page for the transcript and returns its id.
pub fn html_preview(
    state: crate::runtime::State<'_, HtmlPreviews>,
    html: String,
) -> Result<String, String> {
    state.put(html)
}

/// `sirus-preview://localhost/<id>`: the page with the size reporter, under the preview CSP.
/// `sirus-preview://localhost/v/<id>`: an inline visual's document as registered, under
/// the visual CSP.
pub fn serve(previews: &HtmlPreviews, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = request.uri().path().trim_start_matches('/');
    let (visual, id) = match path.strip_prefix("v/") {
        Some(id) => (true, id),
        None => (false, path),
    };
    let valid = id.len() == 24 && id.bytes().all(|byte| byte.is_ascii_hexdigit());
    match previews.get(id).filter(|_| valid) {
        Some(html) if visual => Response::builder()
            .status(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Security-Policy", VISUAL_CSP)
            .header("X-Content-Type-Options", "nosniff")
            .header("Referrer-Policy", "no-referrer")
            .body(html.into_bytes())
            .unwrap_or_default(),
        Some(html) => Response::builder()
            .status(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Security-Policy", CSP)
            .header("X-Content-Type-Options", "nosniff")
            .body(format!("{SIZE_REPORTER}{html}").into_bytes())
            .unwrap_or_default(),
        None => Response::builder()
            .status(404)
            .body(Vec::new())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_bounded_reused_and_served_with_their_csp() {
        let previews = HtmlPreviews::default();
        let id = previews.put("<p>oi</p>".into()).unwrap();
        assert_eq!(previews.put("<p>oi</p>".into()).unwrap(), id);
        assert!(previews.put("x".repeat(MAX_PAGE + 1)).is_err());
        let request = Request::builder()
            .uri(format!("sirus-preview://localhost/{id}"))
            .body(Vec::new())
            .unwrap();
        let response = serve(&previews, &request);
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["Content-Security-Policy"], CSP);
        assert!(String::from_utf8(response.body().clone())
            .unwrap()
            .ends_with("<p>oi</p>"));
        let missing = Request::builder()
            .uri("sirus-preview://localhost/../etc")
            .body(Vec::new())
            .unwrap();
        assert_eq!(serve(&previews, &missing).status(), 404);
        let visual = Request::builder()
            .uri(format!("sirus-preview://localhost/v/{id}?x=1"))
            .body(Vec::new())
            .unwrap();
        let response = serve(&previews, &visual);
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["Content-Security-Policy"], VISUAL_CSP);
        assert_eq!(response.body(), b"<p>oi</p>");
        for directive in [
            "connect-src 'none'",
            "frame-src 'none'",
            "form-action 'none'",
            "worker-src 'none'",
            "base-uri 'none'",
        ] {
            assert!(VISUAL_CSP.contains(directive), "{directive}");
        }
        assert!(!VISUAL_CSP.contains("http:"));
        for path in ["v/", "v/../x", "v/v/abc", "x/0123456789abcdef01234567"] {
            let request = Request::builder()
                .uri(format!("sirus-preview://localhost/{path}"))
                .body(Vec::new())
                .unwrap();
            assert_eq!(serve(&previews, &request).status(), 404, "{path}");
        }
        for index in 0..MAX_PAGES + 2 {
            previews.put(format!("<p>{index}</p>")).unwrap();
        }
        assert_eq!(previews.0.lock().unwrap().len(), MAX_PAGES);
    }
}
