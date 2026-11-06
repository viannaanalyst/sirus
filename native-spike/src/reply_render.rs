//! Draws Mermaid diagrams and KaTeX formulas for replies. One hidden WKWebView loads a small page
//! that imports the committed `mermaid.js` and `katex.js` from `assets/reply-render` (the host
//! serves that folder at `/assets/` on the window's loopback port), renders the source, reports its size, and the view is
//! snapshotted to a PNG. The result is cached by content key, in memory (`reply_rich`) and on disk,
//! so each diagram and formula is drawn once.
//!
//! Agent output is untrusted: Mermaid runs in `strict` mode, KaTeX with `trust: false`, and the
//! page's policy lets it load only that folder's scripts, styles and fonts and fetch nothing.
//! A Rust Mermaid renderer was not added: it needs `mermaid-rs-renderer`, which this build does not
//! have, and it draws no formulas.
use crate::reply_rich::Want;
use block2::RcBlock;
use gpui::AsyncApp;
use objc2::rc::Retained;
use objc2::{AllocAnyThread, MainThreadMarker, msg_send};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
use objc2_foundation::{NSDictionary, NSError, NSNumber, NSPoint, NSRect, NSSize, NSString, NSURL};
use objc2_web_kit::{
    WKSnapshotConfiguration, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Diagram source above this size stays a code block (Sirus's limit).
pub const MAX_SOURCE: usize = 20_000;
const MAX_SIDE: f64 = 4000.;
const POLL: Duration = Duration::from_millis(60);
const POLLS: usize = 170;

// ---- The pages (pure) --------------------------------------------------------------------

fn js_string(text: &str) -> String {
    // `</` could end the script early; JSON allows the escaped slash.
    serde_json::to_string(text)
        .unwrap_or_else(|_| "\"\"".into())
        .replace("</", "<\\/")
}

fn policy(base: &str) -> String {
    format!(
        "default-src 'none'; script-src 'unsafe-inline' {base}; style-src 'unsafe-inline' {base}; font-src {base} data:; img-src data:"
    )
}

const SHELL: &str = "html,body{margin:0;padding:0;background:transparent}#c{display:inline-block;font:14px -apple-system,BlinkMacSystemFont,sans-serif;line-height:1.3}";

/// The page that draws `want`, loading its libraries from `base/assets/`. `ink` is a CSS color.
pub fn page(want: &Want, base: &str, ink: &str) -> Option<String> {
    let key = js_string(&want.key());
    let head = |style: &str| {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{}\"><style>{SHELL}#c{{color:{ink}}}{style}</style></head><body><div id=\"c\"></div>",
            policy(base)
        )
    };
    // Reports `key:{json}`, so a stale page's answer is never taken for this one's.
    let tail = "const c=document.getElementById('c');const out=(v)=>{window.__r=KEY+':'+JSON.stringify(v)};";
    match want {
        Want::Diagram { source, light } => {
            let theme = if *light { "default" } else { "dark" };
            Some(format!(
                "{}<script type=\"module\">{}try{{const m=(await import({})).default;m.initialize({{startOnLoad:false,securityLevel:'strict',suppressErrorRendering:true,theme:'{theme}',htmlLabels:false,flowchart:{{htmlLabels:false}},maxTextSize:{MAX_SOURCE},fontFamily:'-apple-system, sans-serif'}});const {{svg}}=await m.render('m1',{});c.innerHTML=svg;const s=c.querySelector('svg');const b=s.viewBox.baseVal;s.style.maxWidth='none';s.setAttribute('width',b.width);s.setAttribute('height',b.height);s.style.width=b.width+'px';s.style.height=b.height+'px';await document.fonts.ready;const r=c.getBoundingClientRect();out({{w:r.width,h:r.height}})}}catch(e){{out({{error:String(e)}})}}</script></body></html>",
                head(""),
                tail.replace("KEY", &key),
                js_string(&format!("{base}/assets/mermaid.js")),
                js_string(source)
            ))
        }
        Want::Math { tex, display, .. } => {
            let font = if *display { "17px" } else { "15px" };
            Some(format!(
                "{}<script type=\"module\">{}try{{await new Promise((ok,no)=>{{const l=document.createElement('link');l.rel='stylesheet';l.href={};l.onload=ok;l.onerror=no;document.head.append(l)}});const k=await import({});c.style.fontSize='{font}';c.innerHTML=k.renderToString({},{{displayMode:{display},throwOnError:true,trust:false,strict:'ignore',output:'html',maxSize:50,maxExpand:500}});c.offsetWidth;await document.fonts.ready;const r=c.getBoundingClientRect();out({{w:r.width,h:r.height}})}}catch(e){{out({{error:String(e)}})}}</script></body></html>",
                head(".katex-display{margin:0!important}"),
                tail.replace("KEY", &key),
                js_string(&format!("{base}/assets/katex.css")),
                js_string(&format!("{base}/assets/katex.js")),
                js_string(tex),
            ))
        }
        _ => None,
    }
}

/// `(width, height)` out of the page's answer `key:{"w":..,"h":..}`; `None` while it is not there.
pub fn parse_answer(key: &str, answer: &str) -> Option<Result<(f64, f64), String>> {
    let json = answer.strip_prefix(key)?.strip_prefix(':')?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if let Some(error) = value["error"].as_str() {
        return Some(Err(error.to_owned()));
    }
    let (w, h) = (value["w"].as_f64()?, value["h"].as_f64()?);
    Some(if w >= 1. && h >= 1. && w <= MAX_SIDE && h <= MAX_SIDE {
        Ok((w.ceil(), h.ceil()))
    } else {
        Err("size".to_owned())
    })
}

// ---- The disk cache ------------------------------------------------------------------------

fn cache_dir() -> PathBuf {
    crate::backend::data_dir().join("reply-cache")
}

fn size_text(size: (f32, f32)) -> String {
    format!("{} {}", size.0, size.1)
}

fn parse_size(text: &str) -> Option<(f32, f32)> {
    let (w, h) = text.trim().split_once(' ')?;
    Some((w.parse().ok()?, h.parse().ok()?)).filter(|(w, h): &(f32, f32)| *w >= 1. && *h >= 1.)
}

fn cache_read(dir: &Path, key: &str) -> Option<(Vec<u8>, (f32, f32))> {
    let size = parse_size(&std::fs::read_to_string(dir.join(format!("{key}.size"))).ok()?)?;
    Some((std::fs::read(dir.join(format!("{key}.png"))).ok()?, size))
}

fn cache_write(dir: &Path, key: &str, png: &[u8], size: (f32, f32)) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if std::fs::write(dir.join(format!("{key}.png")), png).is_ok() {
        let _ = std::fs::write(dir.join(format!("{key}.size")), size_text(size));
    }
}

/// Keeps the `keep` newest drawings; older ones go.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort();
    for (_, path) in files.iter().take(files.len() - keep) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("size"));
    }
}

// ---- The engine ----------------------------------------------------------------------------

thread_local! {
    static ENGINE: RefCell<Option<Retained<WKWebView>>> = const { RefCell::new(None) };
    static PRUNED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The one hidden view, made on first use. Nothing it loads is kept: no cookies, caches or storage.
fn engine() -> Result<Retained<WKWebView>, String> {
    ENGINE.with_borrow_mut(|slot| {
        if let Some(web) = slot {
            return Ok(web.clone());
        }
        let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
        let web = unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(900., 600.));
            let web = WKWebView::initWithFrame_configuration(mtm.alloc::<WKWebView>(), frame, &config);
            // A transparent page, so the picture sits on the transcript's own background.
            let _: () = msg_send![&*web, setValue: &*NSNumber::new_bool(false), forKey: &*NSString::from_str("drawsBackground")];
            web.setHidden(true);
            web
        };
        *slot = Some(web.clone());
        Ok(web)
    })
}

/// The page's answer text, or empty.
async fn ask(web: &Retained<WKWebView>, script: &str) -> String {
    match crate::browser_tab::evaluate(web, script).recv().await {
        Ok(Ok(text)) => text,
        _ => String::new(),
    }
}

/// The view as a PNG at twice its size in points, so the drawing is sharp on a Retina screen.
async fn snapshot(web: &Retained<WKWebView>, width: f64) -> Result<Vec<u8>, String> {
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let (sender, receiver) = async_channel::bounded::<Result<Vec<u8>, String>>(1);
    let handler = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let result = unsafe {
            match image.as_ref() {
                None => Err(error
                    .as_ref()
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "snapshot".into())),
                Some(image) => image
                    .TIFFRepresentation()
                    .and_then(|data| {
                        NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &data)
                    })
                    .and_then(|rep| {
                        rep.representationUsingType_properties(
                            NSBitmapImageFileType::PNG,
                            &NSDictionary::new(),
                        )
                    })
                    .map(|png| png.to_vec())
                    .ok_or_else(|| "encode".to_owned()),
            }
        };
        let _ = sender.try_send(result);
    });
    unsafe {
        let config = WKSnapshotConfiguration::new(mtm);
        // A hidden view paints only when asked.
        config.setAfterScreenUpdates(true);
        config.setSnapshotWidth(Some(&NSNumber::new_f64(width * 2.)));
        web.takeSnapshotWithConfiguration_completionHandler(Some(&config), &handler);
    }
    receiver.recv().await.map_err(|_| "snapshot".to_owned())?
}

/// Draws `want` (a diagram or a formula): the PNG and its size in points.
pub async fn draw(
    want: &Want,
    ink: &str,
    cx: &mut AsyncApp,
) -> Result<(Vec<u8>, (f32, f32)), String> {
    let key = want.key();
    let dir = cache_dir();
    if let Some(hit) = cache_read(&dir, &key) {
        return Ok(hit);
    }
    let (_, base) = crate::backend::reply_assets().ok_or("the bundled libraries are missing")?;
    let page = page(want, &base, ink).ok_or("not drawable")?;
    let web = engine()?;
    unsafe {
        web.setFrameSize(NSSize::new(900., 600.));
        let base = NSURL::URLWithString(&NSString::from_str(&format!("{base}/")));
        web.loadHTMLString_baseURL(&NSString::from_str(&page), base.as_deref());
    }
    let mut size = None;
    for _ in 0..POLLS {
        cx.background_executor().timer(POLL).await;
        let answer = ask(&web, "window.__r||''").await;
        if let Some(done) = parse_answer(&key, &answer) {
            size = Some(done?);
            break;
        }
    }
    let (w, h) = size.ok_or("timeout")?;
    web.setFrameSize(NSSize::new(w, h));
    // The view re-lays out at its new size before it is painted.
    cx.background_executor()
        .timer(Duration::from_millis(90))
        .await;
    let png = snapshot(&web, w).await?;
    unsafe { web.loadHTMLString_baseURL(&NSString::from_str("<html></html>"), None) };
    let size = (w as f32, h as f32);
    if !PRUNED.replace(true) {
        prune(&dir, 600);
    }
    cache_write(&dir, &key, &png, size);
    Ok((png, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every file the entry files reach (static and dynamic imports in either quote style,
    /// `url(...)` in the CSS) is in the committed folder, and every file in it is reached: a clean
    /// load never meets a missing chunk, and nothing stale is shipped.
    #[test]
    fn the_committed_libraries_are_complete() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/reply-render");
        let mut pending: Vec<PathBuf> = ["mermaid.js", "katex.js", "katex.css"]
            .iter()
            .map(|name| root.join(name))
            .collect();
        let mut seen = std::collections::HashSet::new();
        while let Some(path) = pending.pop() {
            assert!(path.is_file(), "{} is missing", path.display());
            if !seen.insert(path.clone()) || path.extension().is_some_and(|x| x == "woff2") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let mut targets: Vec<&str> = Vec::new();
            for quote in ["\"", "`"] {
                let opening = format!("{quote}./");
                let mut before = String::new();
                for (index, part) in text.split(opening.as_str()).enumerate() {
                    if index > 0 {
                        let before = before.trim_end();
                        if before.ends_with("from")
                            || before.ends_with("import")
                            || before.ends_with("import(")
                        {
                            targets.extend(part.split(quote).next());
                        }
                    }
                    before = part
                        .chars()
                        .rev()
                        .take(12)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                }
            }
            targets.extend(
                text.split("url(./")
                    .skip(1)
                    .filter_map(|rest| rest.split(')').next()),
            );
            for target in targets {
                pending.push(path.parent().unwrap().join(target));
            }
        }
        let mut shipped = Vec::new();
        let mut folders = vec![root.clone()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() && entry.file_name() != "build" {
                    folders.push(path);
                } else if path.is_file() && entry.file_name() != "LICENSES.txt" {
                    shipped.push(path);
                }
            }
        }
        let unreached: Vec<_> = shipped
            .iter()
            .filter(|path| !seen.contains(*path))
            .collect();
        assert!(unreached.is_empty(), "never loaded: {unreached:?}");
        assert!(seen.len() > 100, "only {} files reached", seen.len());
    }

    #[test]
    fn pages_embed_the_source_safely_and_lock_the_page_down() {
        let want = Want::Diagram {
            source: "graph TD\nA[\"</script><b>\"]".into(),
            light: false,
        };
        let html = page(&want, "http://127.0.0.1:7811", "white").unwrap();
        assert!(!html.contains("</script><b>"));
        assert!(html.contains("securityLevel:'strict'") && html.contains("theme:'dark'"));
        assert!(html.contains("default-src 'none'") && !html.contains("connect-src"));
        assert!(html.contains("http://127.0.0.1:7811/assets/mermaid.js"));
        let math = Want::Math {
            tex: "x^2".into(),
            display: true,
            light: true,
        };
        let html = page(&math, "http://127.0.0.1:7811", "black").unwrap();
        assert!(html.contains("trust:false") && html.contains("displayMode:true"));
        assert!(html.contains("/assets/katex.css") && html.contains("/assets/katex.js"));
        let image = Want::Image {
            session: "s".into(),
            path: "p".into(),
        };
        assert!(page(&image, "b", "c").is_none());
    }

    #[test]
    fn answers_are_taken_only_for_their_own_key() {
        assert_eq!(
            parse_answer("k1", "k1:{\"w\":10.2,\"h\":20}"),
            Some(Ok((11., 20.)))
        );
        assert_eq!(parse_answer("k1", "k0:{\"w\":10,\"h\":20}"), None);
        assert_eq!(parse_answer("k1", ""), None);
        assert_eq!(
            parse_answer("k1", "k1:{\"error\":\"bad\"}"),
            Some(Err("bad".into()))
        );
        assert!(matches!(
            parse_answer("k1", "k1:{\"w\":0,\"h\":20}"),
            Some(Err(_))
        ));
        assert!(matches!(
            parse_answer("k1", "k1:{\"w\":9000,\"h\":20}"),
            Some(Err(_))
        ));
    }

    #[test]
    fn the_disk_cache_round_trips_and_prunes() {
        let dir = std::env::temp_dir().join(format!("sirus-reply-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(cache_read(&dir, "a").is_none());
        for key in ["a", "b", "c"] {
            cache_write(&dir, key, b"png", (12.5, 30.));
            std::thread::sleep(Duration::from_millis(15));
        }
        assert_eq!(cache_read(&dir, "b"), Some((b"png".to_vec(), (12.5, 30.))));
        prune(&dir, 1);
        assert!(cache_read(&dir, "a").is_none() && cache_read(&dir, "b").is_none());
        assert!(cache_read(&dir, "c").is_some());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(parse_size("0 5"), None);
    }
}
