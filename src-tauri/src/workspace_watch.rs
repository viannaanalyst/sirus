//! The file pane follows the disk (ADR-109): while a workspace's tree is shown, a native watcher
//! (FSEvents on macOS) reports changes to it, batched over a short window, as
//! `workspace-changed { path }`. Changes inside `.git`, dependency and build folders are left
//! out. Each workspace has one watcher however many panes show it, and at most MAX_WATCHED
//! workspaces are watched; the oldest is dropped first.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::runtime::{AppHandle, Emitter, State};
use notify::{RecursiveMode, Watcher};
use parking_lot::Mutex;
use serde::Serialize;

use crate::commands::{session_path, AppState};
use crate::error::{Error, Result};

const MAX_WATCHED: usize = 8;
/// Changes within this window become one refresh.
const BATCH: Duration = Duration::from_millis(300);
/// Folders whose churn never changes what the tree shows at a glance.
const IGNORED: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".cache",
    "__pycache__",
    ".venv",
];

struct Watched {
    _watcher: notify::RecommendedWatcher,
    views: usize,
    serial: u64,
}

static WATCHED: Mutex<Option<HashMap<PathBuf, Watched>>> = Mutex::new(None);
static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct WorkspaceChanged {
    path: String,
}

/// Whether a change under `root` can alter the tree (not inside an ignored folder).
pub fn relevant(root: &Path, changed: &Path) -> bool {
    let Ok(relative) = changed.strip_prefix(root) else {
        return false;
    };
    !relative
        .components()
        .any(|part| IGNORED.iter().any(|name| part.as_os_str() == *name))
}

/// Starts (or stops) following a session's workspace; returns the path its events carry.
pub async fn workspace_watch(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    watch: bool,
) -> Result<String> {
    let root = session_path(&state, &session_id)?;
    let display = root.to_string_lossy().into_owned();
    let mut guard = WATCHED.lock();
    let watched = guard.get_or_insert_with(HashMap::new);
    if !watch {
        if let Some(entry) = watched.get_mut(&root) {
            entry.views = entry.views.saturating_sub(1);
            if entry.views == 0 {
                watched.remove(&root);
            }
        }
        return Ok(display);
    }
    if let Some(entry) = watched.get_mut(&root) {
        entry.views += 1;
        return Ok(display);
    }
    let (sender, receiver) = std::sync::mpsc::channel::<PathBuf>();
    let watched_root = root.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            for path in event.paths {
                if relevant(&watched_root, &path) {
                    let _ = sender.send(path);
                }
            }
        }
    })
    .map_err(|error| Error::new("watch", error.to_string()))?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|error| Error::new("watch", error.to_string()))?;
    // One refresh per burst: wait for the first change, gather the rest of the window, emit.
    let event_path = display.clone();
    std::thread::spawn(move || {
        while receiver.recv().is_ok() {
            std::thread::sleep(BATCH);
            while receiver.try_recv().is_ok() {}
            let _ = app.emit(
                "workspace-changed",
                WorkspaceChanged {
                    path: event_path.clone(),
                },
            );
        }
    });
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    watched.insert(
        root,
        Watched {
            _watcher: watcher,
            views: 1,
            serial,
        },
    );
    while watched.len() > MAX_WATCHED {
        let oldest = watched
            .iter()
            .min_by_key(|(_, entry)| entry.serial)
            .map(|(path, _)| path.clone());
        match oldest {
            Some(path) => {
                watched.remove(&path);
            }
            None => break,
        }
    }
    Ok(display)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_in_dependency_build_and_git_folders_are_ignored() {
        let root = Path::new("/w");
        assert!(relevant(root, Path::new("/w/src/app.tsx")));
        assert!(relevant(root, Path::new("/w/clip.mp4")));
        assert!(!relevant(root, Path::new("/w/.git/index")));
        assert!(!relevant(root, Path::new("/w/node_modules/react/index.js")));
        assert!(!relevant(root, Path::new("/w/src-tauri/target/debug/app")));
        assert!(!relevant(root, Path::new("/elsewhere/file")));
    }
}
