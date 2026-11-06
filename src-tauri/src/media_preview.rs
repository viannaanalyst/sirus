//! Video previews in the file pane (ADR-108). The renderer asks for a token
//! for a workspace file; the `sirus-media` scheme then serves that file by byte ranges, so the
//! player can seek without loading the whole video. Only regular files inside the session's
//! workspace with a known video extension get a token, tokens are random and bounded, and the
//! scheme serves nothing else.
//!
//! Replies use the same scheme for the images and videos they link to (ADR-119): images under
//! the reply-image rule, videos only from the session's workspace. A request without a range
//! (an `<img>`) gets the whole file when it is small enough; larger files answer by ranges.
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::runtime::State;
use parking_lot::Mutex;
use serde::Serialize;

use crate::commands::{native_task, session_path, AppState};
use crate::error::{Error, Result};

pub const SCHEME: &str = "sirus-media";
/// Tokens kept at once; the oldest is forgotten first.
/// Inline reply media holds one token per preview on screen, so this leaves room for several.
const MAX_TOKENS: usize = 64;
/// Largest range answered in one response; the player asks again for the rest.
const MAX_CHUNK: u64 = 4 * 1024 * 1024;
/// A request without a `Range` header gets the whole file up to this size (reply images).
const MAX_WHOLE: u64 = 25 * 1024 * 1024;

static TOKENS: Mutex<VecDeque<(String, PathBuf, &'static str)>> = Mutex::new(VecDeque::new());

/// The MIME type of a supported video file, by extension.
pub fn video_mime(path: &str) -> Option<&'static str> {
    let extension = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match extension.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "ogv" => "video/ogg",
        _ => return None,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaPreview {
    pub token: String,
    pub size: u64,
    pub mime: String,
}

/// A token for a video inside the session's workspace.
pub async fn media_preview(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> Result<MediaPreview> {
    let state = state.inner().clone();
    native_task(move || {
        let mime = video_mime(&path).ok_or_else(|| Error::invalid_path("not a video file"))?;
        let cwd = session_path(&state, &session_id)?;
        let canonical = crate::paths::ensure_within(&cwd, std::path::Path::new(&path))?;
        let metadata = canonical.metadata()?;
        if !metadata.is_file() {
            return Err(Error::invalid_path("preview supports regular files only"));
        }
        Ok(register(canonical, mime, metadata.len()))
    })
    .await
}

/// A fresh token for an authorized file; the oldest tokens are forgotten past MAX_TOKENS.
fn register(path: PathBuf, mime: &'static str, size: u64) -> MediaPreview {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let mut tokens = TOKENS.lock();
    tokens.push_back((token.clone(), path, mime));
    while tokens.len() > MAX_TOKENS {
        tokens.pop_front();
    }
    MediaPreview {
        token,
        size,
        mime: mime.into(),
    }
}

/// Whether a reply's media link may be shown inline (ADR-119), as its canonical path, type and
/// size. Images follow the gallery's rule for reply images (ADR-085: an image extension, a
/// regular file under 25 MiB, PNG/JPEG/GIF/WebP bytes, wherever on the Mac the link points).
/// Videos are stricter: a known video extension and a regular file inside the session's
/// workspace, after symlinks are resolved.
pub fn authorize_reply_media(
    cwd: &Path,
    home: Option<&Path>,
    link: &str,
) -> Result<(PathBuf, &'static str, u64)> {
    let path = crate::reply_image::resolve_link(cwd, home, link);
    if let Some(mime) = path.to_str().and_then(video_mime) {
        let canonical = crate::paths::ensure_within(cwd, &path)?;
        let metadata = canonical.metadata()?;
        if !metadata.is_file() {
            return Err(Error::invalid_path("preview supports regular files only"));
        }
        return Ok((canonical, mime, metadata.len()));
    }
    let path = crate::reply_image::resolve(cwd, home, link)?;
    let canonical = crate::paths::canonicalize_existing(&path)
        .map_err(|_| Error::not_found("The image is no longer on this Mac."))?;
    let metadata = crate::reply_image::checked_metadata(&canonical)?;
    let mime = crate::reply_image::sniff_file(&canonical)?;
    Ok((canonical, mime, metadata.len()))
}

/// A token for an image or video a reply links to, shown inline in the transcript (ADR-119).
pub async fn reply_media(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> Result<MediaPreview> {
    let state = state.inner().clone();
    native_task(move || {
        let cwd = session_path(&state, &session_id)?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let (canonical, mime, size) = authorize_reply_media(&cwd, home.as_deref(), &path)?;
        Ok(register(canonical, mime, size))
    })
    .await
}

/// Forgets a token when its preview closes.
pub fn media_release(token: String) {
    TOKENS.lock().retain(|(known, _, _)| *known != token);
}

/// `bytes=start-end` (end optional) within `size`, as an inclusive range bounded to MAX_CHUNK.
pub fn parse_range(header: Option<&str>, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return None;
    }
    let (start, end) = match header {
        None => (0, size - 1),
        Some(value) => {
            let spec = value.trim().strip_prefix("bytes=")?;
            let (start, end) = spec.split_once('-')?;
            if start.is_empty() {
                // `bytes=-N`: the last N bytes.
                let suffix: u64 = end.parse().ok()?;
                (size.saturating_sub(suffix.max(1)), size - 1)
            } else {
                let start: u64 = start.parse().ok()?;
                let end = if end.is_empty() {
                    size - 1
                } else {
                    end.parse::<u64>().ok()?.min(size - 1)
                };
                (start, end)
            }
        }
    };
    if start > end || start >= size {
        return None;
    }
    Some((start, end.min(start + MAX_CHUNK - 1)))
}

/// `sirus-media://localhost/<token>` (macOS) or `http://sirus-media.localhost/<token>`.
pub fn serve(
    request: &crate::runtime::http::Request<Vec<u8>>,
) -> crate::runtime::http::Response<Vec<u8>> {
    use crate::runtime::http::{header, Response, StatusCode};
    let empty = |status: StatusCode| {
        Response::builder()
            .status(status)
            .body(Vec::new())
            .unwrap_or_default()
    };
    let token = request.uri().path().trim_start_matches('/');
    let Some((path, mime)) = TOKENS
        .lock()
        .iter()
        .find(|(known, _, _)| known == token)
        .map(|(_, path, mime)| (path.clone(), *mime))
    else {
        return empty(StatusCode::NOT_FOUND);
    };
    let Ok(mut file) = std::fs::File::open(&path) else {
        return empty(StatusCode::NOT_FOUND);
    };
    let Ok(size) = file.metadata().map(|metadata| metadata.len()) else {
        return empty(StatusCode::NOT_FOUND);
    };
    let range_header = request
        .headers()
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    if range_header.is_none() && size <= MAX_WHOLE {
        // An image asks without a range and needs the whole file in one answer.
        let mut bytes = Vec::with_capacity(size as usize);
        if file.take(MAX_WHOLE).read_to_end(&mut bytes).is_err() {
            return empty(StatusCode::INTERNAL_SERVER_ERROR);
        }
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, bytes.len().to_string())
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .header(header::CACHE_CONTROL, "no-store")
            .body(bytes)
            .unwrap_or_default();
    }
    let Some((start, end)) = parse_range(range_header, size) else {
        return Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{size}"))
            .body(Vec::new())
            .unwrap_or_default();
    };
    let mut bytes = vec![0; (end - start + 1) as usize];
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut bytes).is_err() {
        return empty(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"))
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(bytes)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn videos_are_known_by_extension_only() {
        assert_eq!(video_mime("clip.MP4"), Some("video/mp4"));
        assert_eq!(video_mime("a/b.mov"), Some("video/quicktime"));
        assert_eq!(video_mime("x.webm"), Some("video/webm"));
        assert_eq!(video_mime("x.mkv"), Some("video/x-matroska"));
        assert_eq!(video_mime("notes.txt"), None);
        assert_eq!(video_mime("mp4"), None);
    }

    #[test]
    fn ranges_are_bounded_and_invalid_ones_refused() {
        assert_eq!(parse_range(None, 10), Some((0, 9)));
        assert_eq!(parse_range(Some("bytes=0-"), 10), Some((0, 9)));
        assert_eq!(parse_range(Some("bytes=2-5"), 10), Some((2, 5)));
        assert_eq!(parse_range(Some("bytes=8-100"), 10), Some((8, 9)));
        assert_eq!(parse_range(Some("bytes=-3"), 10), Some((7, 9)));
        assert_eq!(parse_range(Some("bytes=10-"), 10), None);
        assert_eq!(parse_range(Some("bytes=5-2"), 10), None);
        assert_eq!(parse_range(Some("items=0-1"), 10), None);
        let big = 100 * 1024 * 1024;
        assert_eq!(parse_range(Some("bytes=0-"), big), Some((0, MAX_CHUNK - 1)));
    }

    #[test]
    fn the_scheme_serves_only_known_tokens_with_partial_content() {
        let dir = std::env::temp_dir().join(format!("sirus-media-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip.mp4");
        std::fs::write(&file, b"0123456789").unwrap();
        TOKENS
            .lock()
            .push_back(("known".into(), file.clone(), "video/mp4"));
        let request = crate::runtime::http::Request::builder()
            .uri("sirus-media://localhost/known")
            .header("range", "bytes=2-4")
            .body(Vec::new())
            .unwrap();
        let response = serve(&request);
        assert_eq!(response.status(), 206);
        assert_eq!(response.body(), b"234");
        assert_eq!(response.headers()["content-range"], "bytes 2-4/10");
        let unknown = crate::runtime::http::Request::builder()
            .uri("sirus-media://localhost/other")
            .body(Vec::new())
            .unwrap();
        assert_eq!(serve(&unknown).status(), 404);
        media_release("known".into());
        assert_eq!(serve(&request).status(), 404);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_request_without_a_range_gets_a_small_file_whole() {
        let dir = std::env::temp_dir().join(format!("sirus-media-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shot.png");
        std::fs::write(&file, b"\x89PNG\r\n\x1a\nbody").unwrap();
        TOKENS
            .lock()
            .push_back(("whole".into(), file.clone(), "image/png"));
        let request = crate::runtime::http::Request::builder()
            .uri("sirus-media://localhost/whole")
            .body(Vec::new())
            .unwrap();
        let response = serve(&request);
        assert_eq!(response.status(), 200);
        assert_eq!(response.body(), b"\x89PNG\r\n\x1a\nbody");
        assert_eq!(response.headers()["content-type"], "image/png");
        media_release("whole".into());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reply_media_is_authorized_like_reply_images_and_videos_stay_in_the_workspace() {
        let root = std::env::temp_dir().join(format!("sirus-reply-media-{}", uuid::Uuid::new_v4()));
        let work = root.join("work");
        let outside = root.join("outside");
        std::fs::create_dir_all(work.join("shots")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let png = b"\x89PNG\r\n\x1a\nrest";
        std::fs::write(work.join("shots/a.png"), png).unwrap();
        std::fs::write(outside.join("b.png"), png).unwrap();
        std::fs::write(work.join("fake.png"), b"#!/bin/sh\n").unwrap();
        std::fs::write(work.join("clip.mp4"), b"video").unwrap();
        std::fs::write(outside.join("far.mov"), b"video").unwrap();
        std::fs::create_dir_all(work.join("dir.mp4")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.join("far.mov"), work.join("link.mov")).unwrap();

        // Images: relative to the workspace, or anywhere on the Mac, but real images only.
        let (path, mime, size) = authorize_reply_media(&work, None, "shots/a.png").unwrap();
        assert_eq!(path, work.join("shots/a.png").canonicalize().unwrap());
        assert_eq!((mime, size), ("image/png", png.len() as u64));
        let absolute = outside.join("b.png");
        let (_, mime, _) = authorize_reply_media(&work, None, absolute.to_str().unwrap()).unwrap();
        assert_eq!(mime, "image/png");
        let as_file_url = format!("file://{}", absolute.display());
        assert!(authorize_reply_media(&work, None, &as_file_url).is_ok());
        assert!(
            authorize_reply_media(&work, None, "fake.png").is_err(),
            "bytes decide"
        );
        assert!(authorize_reply_media(&work, None, "missing.png").is_err());
        assert!(authorize_reply_media(&work, None, "/etc/passwd").is_err());
        assert!(authorize_reply_media(&work, None, "notes.svg").is_err());

        // Videos: only regular files inside the workspace, symlinks resolved first.
        let (path, mime, _) = authorize_reply_media(&work, None, "clip.mp4").unwrap();
        assert_eq!(path, work.join("clip.mp4").canonicalize().unwrap());
        assert_eq!(mime, "video/mp4");
        let inside = work.join("clip.mp4");
        assert!(authorize_reply_media(&work, None, inside.to_str().unwrap()).is_ok());
        let far = outside.join("far.mov");
        assert!(authorize_reply_media(&work, None, far.to_str().unwrap()).is_err());
        assert!(authorize_reply_media(&work, None, "../outside/far.mov").is_err());
        #[cfg(unix)]
        assert!(authorize_reply_media(&work, None, "link.mov").is_err());
        assert!(authorize_reply_media(&work, None, "dir.mp4").is_err());
        assert!(authorize_reply_media(&work, None, "missing.webm").is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
