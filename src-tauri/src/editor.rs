use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use crate::runtime::{AppHandle, State};

use crate::commands::{native_task, session_path, AppState};
use crate::error::{Error, Result};
use crate::models::{EditorAppIcon, EditorInstall, TextFileSnapshot};
use crate::paths;

struct EditorSpec {
    id: &'static str,
    name: &'static str,
    bundle: &'static str,
    /// A terminal opens a working directory, never a single file.
    directory_only: bool,
}

/// Fixed allowlist. The renderer can only pick one of these ids; it cannot
/// provide an app name, a bundle path or a launch flag.
const EDITORS: &[EditorSpec] = &[
    EditorSpec {
        id: "finder",
        name: "Finder",
        bundle: "/System/Library/CoreServices/Finder.app",
        directory_only: false,
    },
    EditorSpec {
        id: "terminal",
        name: "Terminal",
        bundle: "/System/Applications/Utilities/Terminal.app",
        directory_only: true,
    },
    EditorSpec {
        id: "cursor",
        name: "Cursor",
        bundle: "/Applications/Cursor.app",
        directory_only: false,
    },
    EditorSpec {
        id: "vscode",
        name: "Visual Studio Code",
        bundle: "/Applications/Visual Studio Code.app",
        directory_only: false,
    },
    EditorSpec {
        id: "xcode",
        name: "Xcode",
        bundle: "/Applications/Xcode.app",
        directory_only: false,
    },
];

const RESERVED_SEGMENT: &str = ".git";
const MAX_EDITOR_READ_BYTES: u64 = 2 * 1024 * 1024;
const MAX_EDITOR_WRITE_BYTES: usize = 1024 * 1024;

pub(crate) fn is_reserved_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if name == RESERVED_SEGMENT)
    })
}

pub fn detect_editors() -> Vec<EditorInstall> {
    EDITORS
        .iter()
        .map(|editor| EditorInstall {
            id: editor.id.to_string(),
            name: editor.name.to_string(),
            installed: Path::new(editor.bundle).exists(),
        })
        .collect()
}

/// Real app icons for the fixed editor allowlist. The renderer receives a bounded
/// base64 PNG per installed editor and never chooses the bundle path.
pub fn editor_app_icons() -> Vec<EditorAppIcon> {
    EDITORS
        .iter()
        .map(|editor| EditorAppIcon {
            id: editor.id.to_string(),
            png: app_icon_png(editor.bundle).unwrap_or_default(),
        })
        .collect()
}

/// The app's own icon as a small PNG, in base64, from the icon file its bundle names.
#[cfg(target_os = "macos")]
fn app_icon_png(bundle: &str) -> Option<String> {
    use base64::Engine;
    let root = Path::new(bundle);
    let plist = root.join("Contents/Info.plist");
    let named = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleIconFile", "raw"])
        .arg(&plist)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let name = String::from_utf8_lossy(&named.stdout).trim().to_owned();
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let file = root
        .join("Contents/Resources")
        .join(format!("{}.icns", name.trim_end_matches(".icns")));
    let out = std::env::temp_dir().join(format!(
        "sirus-app-icon-{}-{}.png",
        std::process::id(),
        root.file_stem()?.to_string_lossy().replace(' ', "-")
    ));
    let made = std::process::Command::new("/usr/bin/sips")
        .args(["-s", "format", "png", "-Z", "64"])
        .arg(&file)
        .arg("--out")
        .arg(&out)
        .output()
        .is_ok_and(|output| output.status.success());
    let bytes = made.then(|| std::fs::read(&out).ok()).flatten();
    let _ = std::fs::remove_file(&out);
    bytes.map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[cfg(not(target_os = "macos"))]
fn app_icon_png(_bundle: &str) -> Option<String> {
    None
}

pub async fn open_in_editor(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    editor: String,
    path: Option<String>,
) -> Result<()> {
    let spec = EDITORS
        .iter()
        .find(|item| item.id == editor)
        .ok_or_else(|| Error::new("invalid", "unknown editor"))?;
    if !Path::new(spec.bundle).exists() {
        return Err(Error::new("invalid", "editor is not installed"));
    }
    let cwd = session_path(state.inner(), &session_id)?;
    let target = match path.as_deref() {
        Some(path) => paths::ensure_within(&cwd, Path::new(path))?,
        None => cwd.clone(),
    };
    let target = if spec.directory_only && target.is_file() {
        target
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| Error::invalid_path("no directory to open"))?
    } else {
        target
    };
    // Finder cannot "open" a file as an editor would (macOS refuses it); it shows the file instead.
    let _ = app;
    open_with(spec.bundle, &target)
}

/// Opens `target` with the app at `bundle`.
#[cfg(target_os = "macos")]
fn open_with(bundle: &str, target: &Path) -> Result<()> {
    let opened = std::process::Command::new("/usr/bin/open")
        .arg("-a")
        .arg(bundle)
        .arg(target)
        .status()?;
    if opened.success() {
        Ok(())
    } else {
        Err(Error::new("invalid", "the app could not open this folder"))
    }
}

#[cfg(not(target_os = "macos"))]
fn open_with(_bundle: &str, _target: &Path) -> Result<()> {
    Err(Error::new("invalid", "opening an app needs a Mac"))
}

pub async fn read_text_file(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> Result<TextFileSnapshot> {
    let state = state.inner().clone();
    native_task(move || {
        let cwd = session_path(&state, &session_id)?;
        if is_reserved_path(Path::new(&path)) {
            return Err(Error::invalid_path("cannot open reserved paths"));
        }
        let file = paths::open_regular_within(&cwd, Path::new(&path))?;
        let size = file.metadata()?.len();
        if size > MAX_EDITOR_READ_BYTES {
            return Err(Error::invalid_path(
                "file is too large to edit (2 MiB limit)",
            ));
        }
        let mut bytes = Vec::with_capacity(size as usize);
        file.take(MAX_EDITOR_READ_BYTES + 1)
            .read_to_end(&mut bytes)?;
        let binary = bytes.iter().take(8192).any(|byte| *byte == 0);
        if binary {
            return Ok(TextFileSnapshot {
                path,
                content: String::new(),
                size,
                binary: true,
            });
        }
        match String::from_utf8(bytes) {
            Ok(content) => Ok(TextFileSnapshot {
                path,
                content,
                size,
                binary: false,
            }),
            Err(_) => Ok(TextFileSnapshot {
                path,
                content: String::new(),
                size,
                binary: true,
            }),
        }
    })
    .await
}

pub async fn write_text_file(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
    content: String,
) -> Result<()> {
    if content.len() > MAX_EDITOR_WRITE_BYTES {
        return Err(Error::new(
            "invalid",
            "file content exceeds the 1 MiB limit",
        ));
    }
    let state = state.inner().clone();
    native_task(move || {
        let cwd = session_path(&state, &session_id)?;
        if is_reserved_path(Path::new(&path)) {
            return Err(Error::invalid_path("cannot write reserved paths"));
        }
        let mut file = paths::open_regular_for_write_within(&cwd, Path::new(&path))?;
        file.set_len(0)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })
    .await
}
