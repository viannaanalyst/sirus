//! Draws Mermaid diagrams and KaTeX formulas for replies. One hidden WKWebView loads a small page
//! that imports Sirus's own bundled `mermaid` and `katex` chunks from the UI folder (the host
//! serves it on the window's loopback port), renders the source, reports its size, and the view is
//! snapshotted to a PNG. The result is cached by content key, in memory (`reply_rich`) and on disk,
//! so each diagram and formula is drawn once.
//!
//! Agent output is untrusted: Mermaid runs in `strict` mode, KaTeX with `trust: false`, and the
//! page's policy lets it load only the UI folder's scripts, styles and fonts and fetch nothing.
//! Monocode's Rust Mermaid was not ported: it needs `mermaid-rs-renderer`, which this build does not
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

/// Diagram source above this size stays a code block (Sirus Code's limit).
pub const MAX_SOURCE: usize = 20_000;
const MAX_SIDE: f64 = 4000.;
const POLL: Duration = Duration::from_millis(60);
const POLLS: usize = 170;

// ---- The pages (pure) --------------------------------------------------------------------

/// The file in `names` that starts with `prefix` and ends with `.ext` (the build hashes names).
pub fn pick_chunk<'a>(names: &'a [String], prefix: &str, ext: &str) -> Option<&'a str> {
    let suffix = format!(".{ext}");
    names
        .iter()
        .map(String::as_str)
        .filter(|name| name.starts_with(prefix) && name.ends_with(&suffix))
        .min()
}

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
pub fn page(want: &Want, names: &[String], base: &str, ink: &str) -> Option<String> {
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
            let module = pick_chunk(names, "mermaid.core-", "js")?;
            let theme = if *light { "default" } else { "dark" };
            Some(format!(
                "{}<script type=\"module\">{}try{{const m=(await import({})).default;m.initialize({{startOnLoad:false,securityLevel:'strict',suppressErrorRendering:true,theme:'{theme}',htmlLabels:false,flowchart:{{htmlLabels:false}},maxTextSize:{MAX_SOURCE},fontFamily:'-apple-system, sans-serif'}});const {{svg}}=await m.render('m1',{});c.innerHTML=svg;const s=c.querySelector('svg');const b=s.viewBox.baseVal;s.style.maxWidth='none';s.setAttribute('width',b.width);s.setAttribute('height',b.height);s.style.width=b.width+'px';s.style.height=b.height+'px';await document.fonts.ready;const r=c.getBoundingClientRect();out({{w:r.width,h:r.height}})}}catch(e){{out({{error:String(e)}})}}</script></body></html>",
                head(""),
                tail.replace("KEY", &key),
                js_string(&format!("{base}/assets/{module}")),
                js_string(source)
            ))
        }
        Want::Math { tex, display, .. } => {
            let module = pick_chunk(names, "katex-", "js")?;
            let css = pick_chunk(names, "katex-", "css")?;
            let font = if *display { "17px" } else { "15px" };
            Some(format!(
                "{}<script type=\"module\">{}try{{await new Promise((ok,no)=>{{const l=document.createElement('link');l.rel='stylesheet';l.href={};l.onload=ok;l.onerror=no;document.head.append(l)}});const k=await import({});c.style.fontSize='{font}';c.innerHTML=k.renderToString({},{{displayMode:{display},throwOnError:true,trust:false,strict:'ignore',output:'html',maxSize:50,maxExpand:500}});c.offsetWidth;await document.fonts.ready;const r=c.getBoundingClientRect();out({{w:r.width,h:r.height}})}}catch(e){{out({{error:String(e)}})}}</script></body></html>",
                head(".katex-display{margin:0!important}"),
                tail.replace("KEY", &key),
                js_string(&format!("{base}/assets/{css}")),
                js_string(&format!("{base}/assets/{module}")),
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
    let (ui, base) = crate::backend::ui_assets().ok_or("the UI folder is missing")?;
    let names: Vec<String> = std::fs::read_dir(ui.join("assets"))
        .map_err(|e| e.to_string())?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    let page = page(want, &names, &base, ink).ok_or("the bundled library is missing")?;
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

    fn names() -> Vec<String> {
        [
            "index-a.js",
            "mermaid-B.js",
            "mermaid.core-zbbo.js",
            "katex--I9w.js",
            "katex-DEc.css",
            "katex-DEc2.js",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn bundled_chunks_are_found_by_prefix() {
        let names = names();
        assert_eq!(
            pick_chunk(&names, "mermaid.core-", "js"),
            Some("mermaid.core-zbbo.js")
        );
        assert_eq!(pick_chunk(&names, "katex-", "css"), Some("katex-DEc.css"));
        assert_eq!(pick_chunk(&names, "katex-", "js"), Some("katex--I9w.js"));
        assert_eq!(pick_chunk(&names, "nope-", "js"), None);
    }

    #[test]
    fn pages_embed_the_source_safely_and_lock_the_page_down() {
        let want = Want::Diagram {
            source: "graph TD\nA[\"</script><b>\"]".into(),
            light: false,
        };
        let html = page(&want, &names(), "http://127.0.0.1:7811", "white").unwrap();
        assert!(!html.contains("</script><b>"));
        assert!(html.contains("securityLevel:'strict'") && html.contains("theme:'dark'"));
        assert!(html.contains("default-src 'none'") && !html.contains("connect-src"));
        assert!(html.contains("http://127.0.0.1:7811/assets/mermaid.core-zbbo.js"));
        let math = Want::Math {
            tex: "x^2".into(),
            display: true,
            light: true,
        };
        let html = page(&math, &names(), "http://127.0.0.1:7811", "black").unwrap();
        assert!(html.contains("trust:false") && html.contains("displayMode:true"));
        assert!(html.contains("katex-DEc.css"));
        assert!(page(&math, &[], "http://127.0.0.1:7811", "black").is_none());
        let image = Want::Image {
            session: "s".into(),
            path: "p".into(),
        };
        assert!(page(&image, &names(), "b", "c").is_none());
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
