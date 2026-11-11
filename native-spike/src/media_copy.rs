//! Copies of videos that live on another computer (ADR-202 addendum). The player is the web
//! view's, which reads a file on this Mac; on a remote computer the video is on that computer's
//! disk. After the host has granted the file (`reply_media`, `attachment_media`), its bytes come
//! over the socket in pieces (`media_chunk`, 4 MiB each) straight into a private copy here, which
//! the player then opens. The copies are kept by what names the file on the host, so a video is
//! copied once, and the oldest are deleted when they would pass `MAX_CACHE`.
use crate::backend::Backend;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// The largest video copied from another computer. Reply videos load as soon as they show.
pub const MAX_VIDEO: u64 = 100 * 1024 * 1024;
/// What all copies may add up to before the oldest go.
const MAX_CACHE: u64 = 512 * 1024 * 1024;

fn dir() -> PathBuf {
    std::env::temp_dir().join("sirus-remote-media")
}

/// The copy's file name: a stable hash of `key` (what names the file on its host) and the
/// original's extension, which the player needs to know the format.
pub fn copy_name(key: &str, name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    let extension = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric()))
        .map(|e| format!(".{}", e.to_ascii_lowercase()))
        .unwrap_or_default();
    format!("{:016x}{extension}", hasher.finish())
}

/// The copies to delete, oldest first, so `incoming` more bytes fit under `cap`.
pub fn evictions(
    mut copies: Vec<(PathBuf, u64, SystemTime)>,
    incoming: u64,
    cap: u64,
) -> Vec<PathBuf> {
    copies.sort_by_key(|(_, _, modified)| *modified);
    let mut total: u64 = copies.iter().map(|(_, size, _)| size).sum::<u64>() + incoming;
    let mut gone = Vec::new();
    for (path, size, _) in copies {
        if total <= cap {
            break;
        }
        total = total.saturating_sub(size);
        gone.push(path);
    }
    gone
}

fn make_room(dir: &Path, incoming: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // This run's copies in progress stay; a crashed run's leftovers go like any copy.
    let ours = format!(".part-{}-", std::process::id());
    let copies = entries
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with(&ours))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| {
                (
                    entry.path(),
                    meta.len(),
                    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                )
            })
        })
        .collect();
    for path in evictions(copies, incoming, MAX_CACHE) {
        let _ = std::fs::remove_file(path);
    }
}

/// Copies the `size`-byte file that `token` grants on the active computer and returns the copy.
/// `key` names the file on its host, `name` gives the extension. Blocks; the caller releases the
/// token afterwards.
pub fn fetch(backend: &Backend, token: &str, size: u64, key: &str, name: &str) -> Option<PathBuf> {
    use base64::Engine as _;
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
    if size == 0 || size > MAX_VIDEO {
        return None;
    }
    let dir = dir();
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .ok()?;
    let target = dir.join(copy_name(key, name));
    if std::fs::metadata(&target).is_ok_and(|m| m.is_file() && m.len() == size) {
        // Used again: the newest, the last to be deleted.
        if let Ok(file) = std::fs::File::options().append(true).open(&target) {
            let _ = file.set_modified(SystemTime::now());
        }
        return Some(target);
    }
    make_room(&dir, size);
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let part = dir.join(format!(
        ".part-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let copied = (|| {
        let mut file = std::fs::File::options()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&part)
            .ok()?;
        let mut offset = 0u64;
        while offset < size {
            let reply = backend
                .request("media_chunk", json!({ "token": token, "offset": offset }))
                .recv_blocking();
            let Ok(Ok(chunk)) = reply else { return None };
            // The file changed on the host since it was granted.
            if chunk["size"].as_u64() != Some(size) {
                return None;
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(chunk["data"].as_str()?)
                .ok()?;
            if bytes.is_empty() || offset + bytes.len() as u64 > size {
                return None;
            }
            file.write_all(&bytes).ok()?;
            offset += bytes.len() as u64;
        }
        file.sync_all().ok()?;
        std::fs::rename(&part, &target).ok()
    })();
    if copied.is_none() {
        let _ = std::fs::remove_file(&part);
        return None;
    }
    Some(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_copy_is_named_by_its_key_and_keeps_a_plain_extension() {
        let a = copy_name("attachment:session:1:abc", "WhatsApp Video.MP4");
        assert!(a.ends_with(".mp4"));
        assert_eq!(a, copy_name("attachment:session:1:abc", "other.mp4"));
        assert_ne!(
            a,
            copy_name("attachment:session:1:abd", "WhatsApp Video.mp4")
        );
        assert!(!copy_name("k", "clip").contains('.'));
        assert!(!copy_name("k", "x.m p4").contains(' '));
        assert!(!copy_name("k", "../../x.mov/..").contains('/'));
    }

    #[test]
    fn the_oldest_copies_go_until_the_new_one_fits() {
        let at = |s| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let copies = vec![
            (PathBuf::from("new"), 40, at(30)),
            (PathBuf::from("old"), 40, at(10)),
            (PathBuf::from("mid"), 40, at(20)),
        ];
        assert!(evictions(copies.clone(), 0, 120).is_empty());
        assert_eq!(
            evictions(copies.clone(), 30, 120),
            vec![PathBuf::from("old")]
        );
        assert_eq!(
            evictions(copies.clone(), 70, 120),
            vec![PathBuf::from("old"), PathBuf::from("mid")]
        );
        assert_eq!(evictions(copies, 500, 120).len(), 3);
    }
}
