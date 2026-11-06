//! Pictures a tool result carried, such as a browser or computer screenshot. Each one is saved
//! once under `<app data dir>/tool-images/<sha256>.<png|jpg>` (private permissions), so the
//! activity step can name the file and the person can open it. The bytes never reach the
//! activity or a saved session: only the path does.

use base64::Engine;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The folder under the app data dir that holds the saved pictures.
const FOLDER: &str = "tool-images";
/// The size limit `reply_image` opens images within (25 MiB, decoded).
const MAX_BYTES: usize = 25 * 1024 * 1024;

static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sets the app data dir the pictures are saved under (once at start-up).
pub fn init(data_dir: &Path) {
    if let Ok(mut dir) = DIR.lock() {
        *dir = Some(data_dir.join(FOLDER));
    }
}

/// Saves a base64 picture and returns its path. PNG and JPEG only, by their bytes; a picture
/// already saved (same bytes) is not written again. None when there is no folder, the picture
/// is not PNG or JPEG or is too large, or the write fails.
pub fn save(base64_data: &str) -> Option<String> {
    let dir = DIR.lock().ok()?.clone()?;
    // Four base64 characters carry three bytes: refuse before decoding anything huge.
    if base64_data.len() > MAX_BYTES / 3 * 4 + 4 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .ok()?;
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let extension = match crate::reply_image::image_type(&bytes)? {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        _ => return None,
    };
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let path = dir.join(format!("{hash}.{extension}"));
    if path.is_file() {
        return Some(path.to_string_lossy().into_owned());
    }
    write_private(&dir, &path, &hash, extension, &bytes)
        .then(|| path.to_string_lossy().into_owned())
}

/// How long a saved picture waits for the activity row that claims it.
const PENDING_TTL: std::time::Duration = std::time::Duration::from_secs(300);
const PENDING_MAX: usize = 8;
const IMAGE_TOOLS: [&str; 3] = [
    "browser_screenshot",
    "computer_screenshot",
    "computer_observe",
];

type Pending = std::collections::HashMap<(String, String), Vec<(std::time::Instant, String)>>;
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// Whether a tool may return a picture the activity row should show.
pub fn is_image_tool(tool: &str) -> bool {
    IMAGE_TOOLS.contains(&tool)
}

/// Saves the picture a screenshot tool is about to return and remembers its path for the
/// session and tool. Codex truncates a tool result's text near 64 KiB, so the image cannot
/// be recovered from its `mcpToolCall` item; the tool saves it where it is produced and the
/// activity row claims the path with `take`. Only the path is kept, never the bytes.
pub fn remember(session_id: &str, tool: &str, result: &serde_json::Value) {
    if !is_image_tool(tool) {
        return;
    }
    let Some(path) = result["image"]["data"].as_str().and_then(save) else {
        return;
    };
    if let Ok(mut pending) = PENDING.lock() {
        let queue = pending
            .get_or_insert_with(Default::default)
            .entry((session_id.to_string(), tool.to_string()))
            .or_default();
        queue.retain(|(at, _)| at.elapsed() < PENDING_TTL);
        if queue.len() >= PENDING_MAX {
            queue.remove(0);
        }
        queue.push((std::time::Instant::now(), path));
    }
}

/// The oldest remembered picture for this session and tool, once.
pub fn take(session_id: &str, tool: &str) -> Option<String> {
    let mut pending = PENDING.lock().ok()?;
    let queue = pending
        .as_mut()?
        .get_mut(&(session_id.to_string(), tool.to_string()))?;
    queue.retain(|(at, _)| at.elapsed() < PENDING_TTL);
    (!queue.is_empty()).then(|| queue.remove(0).1)
}

/// Writes through a temporary file and renames it into place, so a half-written picture never
/// carries the final name.
fn write_private(dir: &Path, path: &Path, hash: &str, extension: &str, bytes: &[u8]) -> bool {
    use std::fs::{DirBuilder, OpenOptions};
    #[cfg(unix)]
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    if builder.create(dir).is_err() {
        return false;
    }
    let temp = dir.join(format!(".{hash}.{}.{extension}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let written = options
        .open(&temp)
        .and_then(|mut file| file.write_all(bytes))
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
        return false;
    }
    true
}

#[cfg(test)]
pub(crate) fn set_dir_for_tests(dir: Option<PathBuf>) {
    if let Ok(mut slot) = DIR.lock() {
        *slot = dir;
    }
}
