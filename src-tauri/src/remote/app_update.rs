//! Updates for the native iPhone app (ADR-135 addendum), served in the Expo Updates protocol
//! (v1): `npm run publish:update` in `mobile/` exports the JavaScript and its assets into
//! `<data dir>/app-updates/ios/<runtime version>/` with a `manifest.json`. The app asks
//! `/api/app-update/manifest` with its runtime version and the update it runs; this answers 204
//! when it already has the published one, else the manifest with each file's URL here. Both the
//! manifest and the files need a device token (sent by the app as `Authorization`).

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Runtime versions are short names (`1`, `0.2.0`, `2026.10-1`); anything else is refused.
pub(super) fn safe_runtime(runtime: &str) -> bool {
    !runtime.is_empty()
        && runtime.len() <= 64
        && runtime
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        && !runtime.starts_with('.')
}

/// A file path inside an update: relative, no parent steps or hidden parts.
pub(super) fn safe_file(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && path.split('/').all(|part| {
            !part.is_empty() && !part.starts_with('.') && !part.contains(['\\', '\0', '%'])
        })
}

/// The folder of the update published for `runtime`, under the app's data folder.
pub(super) fn update_dir(data_dir: &Path, platform: &str, runtime: &str) -> Option<PathBuf> {
    (platform == "ios" && safe_runtime(runtime))
        .then(|| data_dir.join("app-updates").join(platform).join(runtime))
}

/// The manifest to send: the published one with each `path` turned into a URL under `base`
/// (`https://host`), or `None` when nothing is published for this runtime or the app already
/// runs it.
pub(super) fn manifest(
    dir: &Path,
    base: &str,
    platform: &str,
    runtime: &str,
    current: Option<&str>,
) -> Option<Value> {
    let text = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    let mut manifest: Value = serde_json::from_str(&text).ok()?;
    let id = manifest.get("id")?.as_str()?.to_ascii_lowercase();
    if manifest.get("runtimeVersion")?.as_str()? != runtime {
        return None;
    }
    if current.is_some_and(|current| current.eq_ignore_ascii_case(&id)) {
        return None;
    }
    let base = base.trim_end_matches('/');
    let url = |asset: &mut Value| -> Option<()> {
        let path = asset.get("path")?.as_str()?.to_string();
        if !safe_file(&path) {
            return None;
        }
        let object = asset.as_object_mut()?;
        object.remove("path");
        object.insert(
            "url".into(),
            Value::String(format!(
                "{base}/api/app-update/file/{platform}/{runtime}/{path}"
            )),
        );
        Some(())
    };
    url(manifest.get_mut("launchAsset")?)?;
    for asset in manifest.get_mut("assets")?.as_array_mut()? {
        url(asset)?;
    }
    Some(manifest)
}

/// A published file and its content type, when the path is one of the update's own.
pub(super) fn file(dir: &Path, path: &str) -> Option<(Vec<u8>, &'static str)> {
    if !safe_file(path) {
        return None;
    }
    let root = dir.join("files").canonicalize().ok()?;
    let target = root.join(path).canonicalize().ok()?;
    if !target.starts_with(&root) || !target.is_file() {
        return None;
    }
    let bytes = std::fs::read(&target).ok()?;
    let kind = match target
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
    {
        "hbc" | "bundle" | "js" => "application/javascript",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    };
    Some((bytes, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publish(dir: &Path) {
        std::fs::create_dir_all(dir.join("files/assets")).unwrap();
        std::fs::write(dir.join("files/bundle.hbc"), b"code").unwrap();
        std::fs::write(dir.join("files/assets/abc"), b"font").unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"id":"0A1B2C3D-0000-4000-8000-000000000001","createdAt":"2026-10-15T00:00:00.000Z","runtimeVersion":"1",
               "launchAsset":{"hash":"h","key":"k","contentType":"application/javascript","fileExtension":".bundle","path":"bundle.hbc"},
               "assets":[{"hash":"h2","key":"abc","contentType":"font/ttf","fileExtension":".ttf","path":"assets/abc"}],"metadata":{},"extra":{}}"#,
        )
        .unwrap();
    }

    #[test]
    fn the_manifest_points_at_this_computer_and_skips_the_running_update() {
        let root = std::env::temp_dir().join(format!("sirus-app-update-{}", std::process::id()));
        let dir = update_dir(&root, "ios", "1").unwrap();
        publish(&dir);
        let manifest = manifest(&dir, "https://mac.example.com/", "ios", "1", None).unwrap();
        assert_eq!(
            manifest["launchAsset"]["url"],
            "https://mac.example.com/api/app-update/file/ios/1/bundle.hbc"
        );
        assert!(manifest["launchAsset"].get("path").is_none());
        assert_eq!(
            manifest["assets"][0]["url"],
            "https://mac.example.com/api/app-update/file/ios/1/assets/abc"
        );
        assert!(super::manifest(
            &dir,
            "https://h",
            "ios",
            "1",
            Some("0a1b2c3d-0000-4000-8000-000000000001")
        )
        .is_none());
        assert!(super::manifest(&dir, "https://h", "ios", "2", None).is_none());
        assert_eq!(
            file(&dir, "bundle.hbc").unwrap().1,
            "application/javascript"
        );
        assert!(file(&dir, "../manifest.json").is_none());
        assert!(file(&dir, "assets/../../manifest.json").is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn odd_runtimes_and_paths_are_refused() {
        assert!(update_dir(Path::new("/d"), "ios", "../x").is_none());
        assert!(update_dir(Path::new("/d"), "android", "1").is_none());
        assert!(!safe_file(".hidden"));
        assert!(!safe_file("a//b"));
        assert!(safe_file("_expo/static/js/ios/entry-1.hbc"));
    }
}
