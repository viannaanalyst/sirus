//! What a reply shows besides text: images and videos it links to on the Mac (ADR-085, ADR-119),
//! Mermaid diagrams, KaTeX formulas and HTML pages (ADR-072). This file holds the rules and the
//! store of results; `reply_render` draws diagrams and formulas, `markdown` lays everything out
//! and `attachment_view` opens it in the viewer.
//!
//! Nothing here reads a path on its own: images come from the host's `reply_image`, and a video
//! plays only after the host's `reply_media` authorized it (an image extension and PNG, JPEG, GIF
//! or WebP bytes anywhere; a video extension and a file inside the session's workspace).
//!
//! Results live in a store keyed by content, so a frame never redraws or refetches anything:
//! `asset` answers from the store and queues a request only the first time a key is seen.
use gpui::Image;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MediaKind {
    Image,
    Video,
}

// ---- Which links are media ---------------------------------------------------------------

/// A scheme other than `file:///` (`http:`, `data:`, `mailto:`): the link is not on this Mac.
/// The same test as Sirus Code's `isLocalImageLink`.
fn foreign_scheme(value: &str) -> bool {
    let bytes = value.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_alphabetic) {
        return false;
    }
    let Some(colon) = value.find(|c: char| !(c.is_ascii_alphanumeric() || "+.-".contains(c)))
    else {
        return false;
    };
    if bytes[colon] != b':' {
        return false;
    }
    let rest = &value[colon + 1..];
    !rest.starts_with("///") && !value.to_ascii_lowercase().starts_with("file:///")
}

fn without_query(value: &str) -> &str {
    value.split(['?', '#']).next().unwrap_or("")
}

/// The path the host resolves for a local media link: no query or fragment.
pub fn media_path(url: &str) -> String {
    without_query(url.trim()).to_owned()
}

/// What a reply's link shows inline: an image, a video the system can play, or nothing.
pub fn local_media_kind(url: &str) -> Option<MediaKind> {
    let value = url.trim();
    if value.is_empty() || foreign_scheme(value) {
        return None;
    }
    let path = without_query(value).to_ascii_lowercase();
    let extension = path.rsplit_once('.')?.1;
    match extension {
        "png" | "jpg" | "jpeg" | "gif" | "webp" => Some(MediaKind::Image),
        "mp4" | "m4v" | "mov" | "webm" => Some(MediaKind::Video),
        _ => None,
    }
}

/// A link to an HTML file on the Mac, offered as a preview.
pub fn is_html_file(url: &str) -> bool {
    let value = url.trim();
    if value.is_empty() || foreign_scheme(value) {
        return false;
    }
    let path = without_query(value).to_ascii_lowercase();
    path.ends_with(".html") || path.ends_with(".htm")
}

/// The path a link names: `file://` and `~/` are understood, relative paths sit in `cwd`.
/// The host's `resolve_link`. Nothing is checked here.
pub fn resolve_link(cwd: &Path, home: Option<&Path>, link: &str) -> PathBuf {
    let link = link.trim();
    let link = link.strip_prefix("file://").unwrap_or(link);
    let path = match (link.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(link),
    };
    if path.is_absolute() { path } else { cwd.join(path) }
}

/// Whether `path` stays inside `root` once `.` and `..` are folded (the host also resolves
/// symlinks, and has the last word).
pub fn lexically_within(root: &Path, path: &Path) -> bool {
    let mut folded = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                folded.pop();
            }
            Component::CurDir => {}
            other => folded.push(other),
        }
    }
    folded.starts_with(root)
}

/// The file of a video link, for the player: only after the host authorized it. Relative links
/// need the session's folder; without one there is nothing to resolve against.
pub fn video_file(cwd: &str, link: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cwd = Path::new(cwd);
    let relative = !link.trim().starts_with(['/', '~']) && !link.trim().starts_with("file://");
    if relative && cwd.as_os_str().is_empty() {
        return None;
    }
    let path = resolve_link(cwd, home.as_deref(), link);
    (cwd.as_os_str().is_empty() || lexically_within(cwd, &path)).then_some(path)
}

// ---- Picture sizes -----------------------------------------------------------------------

/// Width and height of a PNG, JPEG, GIF or WebP, read from its header.
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be16 = |at: usize| Some(u32::from(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?])));
    let le16 = |at: usize| Some(u32::from(u16::from_le_bytes([*bytes.get(at)?, *bytes.get(at + 1)?])));
    let size = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let be32 = |at: usize| {
            Some(u32::from_be_bytes([
                *bytes.get(at)?,
                *bytes.get(at + 1)?,
                *bytes.get(at + 2)?,
                *bytes.get(at + 3)?,
            ]))
        };
        (be32(16)?, be32(20)?)
    } else if bytes.starts_with(b"GIF8") {
        (le16(6)?, le16(8)?)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut at = 2;
        loop {
            if *bytes.get(at)? != 0xFF {
                at += 1;
                continue;
            }
            let marker = *bytes.get(at + 1)?;
            match marker {
                0xFF => at += 1,
                0xD8 | 0x01 | 0xD0..=0xD7 => at += 2,
                0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                    break (be16(at + 7)?, be16(at + 5)?);
                }
                _ => at += 2 + be16(at + 2)? as usize,
            }
        }
    } else if bytes.len() > 30 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        match &bytes[12..16] {
            b"VP8X" => {
                let le24 = |at: usize| {
                    Some(
                        u32::from(*bytes.get(at)?)
                            | u32::from(*bytes.get(at + 1)?) << 8
                            | u32::from(*bytes.get(at + 2)?) << 16,
                    )
                };
                (le24(24)? + 1, le24(27)? + 1)
            }
            b"VP8L" => {
                let packed = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
                ((packed & 0x3FFF) + 1, ((packed >> 14) & 0x3FFF) + 1)
            }
            b"VP8 " => (le16(26)? & 0x3FFF, le16(28)? & 0x3FFF),
            _ => return None,
        }
    } else {
        return None;
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// `size` scaled down to fit `max` (never up), keeping its proportions.
pub fn fit(size: (f32, f32), max: (f32, f32)) -> (f32, f32) {
    let scale = (max.0 / size.0).min(max.1 / size.1).min(1.);
    ((size.0 * scale).max(1.), (size.1 * scale).max(1.))
}

// ---- Streaming ---------------------------------------------------------------------------

/// True when `text` ends inside a fenced code block that has not been closed yet: while a reply
/// streams, the block shows as plain code until its closing fence arrives.
pub fn ends_in_open_fence(text: &str) -> bool {
    let mut open: Option<(char, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start_matches([' ', '>']).trim_start();
        let Some(mark) = trimmed.chars().next().filter(|c| matches!(c, '`' | '~')) else {
            continue;
        };
        let run = trimmed.chars().take_while(|c| *c == mark).count();
        if run < 3 {
            continue;
        }
        match open {
            None => open = Some((mark, run)),
            Some((m, n)) if m == mark && run >= n && trimmed[run..].trim().is_empty() => {
                open = None;
            }
            _ => {}
        }
    }
    open.is_some()
}

// ---- What a reply asks for, and what came back ------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Want {
    Image { session: String, path: String },
    Video { session: String, path: String, cwd: String },
    Diagram { source: String, light: bool },
    Math { tex: String, display: bool, light: bool },
}

fn hash_of(parts: &[&str]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for part in parts {
        part.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

impl Want {
    /// The content key: the same request always has the same key, so results are cached.
    pub fn key(&self) -> String {
        match self {
            Want::Image { session, path } => format!("img-{}", hash_of(&[session, path])),
            Want::Video { session, path, .. } => format!("vid-{}", hash_of(&[session, path])),
            Want::Diagram { source, light } => {
                format!("mmd-{}", hash_of(&[source, if *light { "l" } else { "d" }]))
            }
            Want::Math { tex, display, light } => format!(
                "tex-{}",
                hash_of(&[
                    tex,
                    if *display { "D" } else { "I" },
                    if *light { "l" } else { "d" }
                ])
            ),
        }
    }

    /// Diagrams and formulas are drawn in the page engine; the rest come from the host.
    pub fn is_drawn(&self) -> bool {
        matches!(self, Want::Diagram { .. } | Want::Math { .. })
    }
}

#[derive(Clone)]
pub enum Asset {
    Pending,
    /// A picture and its size in points.
    Picture { image: Arc<Image>, size: (f32, f32) },
    /// A video the host authorized: where it plays from, and a still when one could be made.
    Video { path: PathBuf, poster: Option<(Arc<Image>, (f32, f32))> },
    Failed,
}

const MAX_ENTRIES: usize = 400;

thread_local! {
    static STORE: RefCell<HashMap<String, Asset>> = Default::default();
    static WANTED: RefCell<Vec<(String, Want)>> = Default::default();
}

/// The result for `want`. The first time a key is seen it is queued (see `take_wanted`) and
/// `Pending` is returned; asking again changes nothing.
pub fn asset(want: &Want) -> Asset {
    let key = want.key();
    STORE.with_borrow_mut(|store| {
        if let Some(found) = store.get(&key) {
            return found.clone();
        }
        if store.len() >= MAX_ENTRIES {
            store.retain(|_, asset| matches!(asset, Asset::Pending));
        }
        store.insert(key.clone(), Asset::Pending);
        WANTED.with_borrow_mut(|wanted| wanted.push((key, want.clone())));
        Asset::Pending
    })
}

/// The result already known, without asking for anything.
pub fn peek(want: &Want) -> Option<Asset> {
    STORE.with_borrow(|store| store.get(&want.key()).cloned())
}

/// Queued requests, oldest first: up to `host` for the host and `drawn` for the page engine
/// (`Want::is_drawn`). The rest stay queued.
pub fn take_wanted(host: usize, drawn: usize) -> Vec<(String, Want)> {
    WANTED.with_borrow_mut(|wanted| {
        let (mut host, mut drawn) = (host, drawn);
        let mut taken = Vec::new();
        wanted.retain(|entry| {
            let room = if entry.1.is_drawn() { &mut drawn } else { &mut host };
            if *room == 0 {
                return true;
            }
            *room -= 1;
            taken.push(entry.clone());
            false
        });
        taken
    })
}

/// True while requests wait to be started.
pub fn has_wanted() -> bool {
    WANTED.with_borrow(|wanted| !wanted.is_empty())
}

pub fn finish(key: &str, asset: Asset) {
    STORE.with_borrow_mut(|store| {
        store.insert(key.to_owned(), asset);
    });
}

/// A decoded PNG with its size, for the store.
pub fn picture(png: Vec<u8>, size: (f32, f32)) -> Asset {
    Asset::Picture {
        image: Arc::new(Image::from_bytes(gpui::ImageFormat::Png, png)),
        size,
    }
}

// ---- What a click asks the window to do ---------------------------------------------------

#[derive(Clone, Debug)]
pub enum Act {
    /// An image of the reply, in the viewer.
    Image { name: String, want: Want },
    /// A video of the reply, played from `path`.
    Video { name: String, path: PathBuf },
    /// HTML written in the reply.
    Html { source: String },
    /// An HTML file the reply links to; the host reads it.
    HtmlFile { session: String, link: String },
}

// ---- The page of a preview ---------------------------------------------------------------

/// Sirus Code's policy for agent HTML (`html_preview.rs`): the page's own inline scripts and
/// styles run, images, fonts and media may come from https, and nothing else may be fetched, so
/// the page can neither send data out nor load code from anywhere.
pub const PREVIEW_POLICY: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob: https:; font-src data: https:; media-src data: blob: https:";
const MAX_PAGE: usize = 2 * 1024 * 1024;

/// The document the viewer shows for agent HTML. The policy comes first, so the page cannot
/// loosen it. `None` when the page is larger than 2 MiB (Sirus Code's limit).
pub fn preview_page(source: &str) -> Option<String> {
    (source.len() <= MAX_PAGE).then(|| {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{PREVIEW_POLICY}\"></head><body>{source}</body></html>"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_links_follow_sirus_codes_rules() {
        assert_eq!(local_media_kind("/tmp/a.png"), Some(MediaKind::Image));
        assert_eq!(local_media_kind("shots/a.JPEG?x=1#y"), Some(MediaKind::Image));
        assert_eq!(local_media_kind("~/clip.mov"), Some(MediaKind::Video));
        assert_eq!(local_media_kind("file:///tmp/a.webp"), Some(MediaKind::Image));
        assert_eq!(local_media_kind("https://x.com/a.png"), None);
        assert_eq!(local_media_kind("data:image/png;base64,AAAA.png"), None);
        assert_eq!(local_media_kind("/tmp/a.svg"), None);
        assert_eq!(local_media_kind("/tmp/archive"), None);
        assert_eq!(local_media_kind(""), None);
        assert_eq!(media_path(" /tmp/a.png?x#y "), "/tmp/a.png");
        assert!(is_html_file("out/index.html"));
        assert!(is_html_file("/tmp/p.HTM#top"));
        assert!(!is_html_file("https://x.com/index.html"));
        assert!(!is_html_file("notes.md"));
    }

    #[test]
    fn links_resolve_like_the_host_and_videos_stay_in_the_workspace() {
        let cwd = Path::new("/work/app");
        let home = Path::new("/Users/me");
        assert_eq!(resolve_link(cwd, Some(home), "~/a.png"), PathBuf::from("/Users/me/a.png"));
        assert_eq!(resolve_link(cwd, Some(home), "file:///tmp/a.png"), PathBuf::from("/tmp/a.png"));
        assert_eq!(resolve_link(cwd, Some(home), "x/a.mp4"), PathBuf::from("/work/app/x/a.mp4"));
        assert!(lexically_within(cwd, Path::new("/work/app/x/../y/a.mp4")));
        assert!(!lexically_within(cwd, Path::new("/work/app/../other/a.mp4")));
        assert_eq!(video_file("/work/app", "out/a.mp4"), Some(PathBuf::from("/work/app/out/a.mp4")));
        assert_eq!(video_file("/work/app", "/tmp/a.mp4"), None);
        assert_eq!(video_file("/work/app", "../a.mp4"), None);
        assert_eq!(video_file("", "a.mp4"), None);
    }

    #[test]
    fn picture_sizes_come_from_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&[0, 0, 1, 0, 0, 0, 0, 200]);
        assert_eq!(image_size(&png), Some((256, 200)));
        assert_eq!(image_size(b"GIF89a\x40\x01\xf0\0"), Some((320, 240)));
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 11, 8, 0, 100, 0, 200, 3];
        assert_eq!(image_size(&jpeg), Some((200, 100)));
        assert_eq!(image_size(b"<svg>"), None);
        assert_eq!(fit((800., 400.), (400., 400.)), (400., 200.));
        assert_eq!(fit((100., 50.), (400., 400.)), (100., 50.));
    }

    #[test]
    fn a_fence_is_open_until_its_closing_line() {
        assert!(ends_in_open_fence("text\n```mermaid\ngraph TD"));
        assert!(!ends_in_open_fence("```mermaid\ngraph TD\n```\n"));
        assert!(ends_in_open_fence("```mermaid\ngraph TD\n```\n\n~~~html\n<p>"));
        assert!(!ends_in_open_fence("````md\n```\ninner\n```\n````"));
        assert!(ends_in_open_fence("````md\n```\ninner\n```\n"));
        assert!(!ends_in_open_fence("no fences here"));
    }

    #[test]
    fn keys_are_stable_and_tell_requests_apart() {
        let a = Want::Diagram { source: "graph TD".into(), light: false };
        let b = Want::Diagram { source: "graph TD".into(), light: true };
        assert_eq!(a.key(), a.clone().key());
        assert_ne!(a.key(), b.key());
        let m = |display| Want::Math { tex: "x^2".into(), display, light: false };
        assert_ne!(m(true).key(), m(false).key());
        assert!(a.key().starts_with("mmd-") && m(true).key().starts_with("tex-"));
        assert_ne!(
            Want::Image { session: "s".into(), path: "a.png".into() }.key(),
            Want::Image { session: "t".into(), path: "a.png".into() }.key()
        );
    }

    #[test]
    fn the_store_queues_a_request_once() {
        let want = Want::Math { tex: "store-test".into(), display: false, light: true };
        assert!(matches!(asset(&want), Asset::Pending));
        assert!(matches!(asset(&want), Asset::Pending));
        let queued: Vec<_> = take_wanted(10, 10).into_iter().filter(|(_, w)| *w == want).collect();
        assert_eq!(queued.len(), 1);
        finish(&want.key(), Asset::Failed);
        assert!(matches!(asset(&want), Asset::Failed));
        assert!(take_wanted(10, 10).iter().all(|(_, w)| *w != want));
    }

    #[test]
    fn previews_carry_the_policy_first_and_refuse_huge_pages() {
        let page = preview_page("<p>hi</p><script>1</script>").unwrap();
        let policy = page.find("Content-Security-Policy").unwrap();
        assert!(policy < page.find("<p>hi").unwrap());
        assert!(page.contains("default-src 'none'") && !page.contains("connect-src"));
        assert!(preview_page(&"x".repeat(MAX_PAGE + 1)).is_none());
    }
}
