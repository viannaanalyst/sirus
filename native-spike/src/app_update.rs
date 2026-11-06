//! In-app updates of Sirus (ADR-140, ADR-190). A release publishes `Sirus-arm64.app.tar.gz`, its
//! minisign signature and `sirus-latest.json` under a fixed release tag, so this app reads its
//! own feed and never the one Sirus Code installs read. Nothing is installed that the key in
//! `tauri.conf.json` did not sign. Installing swaps the bundle on disk; the relaunch happens only
//! when the person asks, and the shell puts it behind the running-agents confirmation first.
//!
//! The network and the unpacking use the tools macOS ships (`curl`, `tar`), so this adds no HTTP
//! stack; the pure parts (versions, the feed, the signature) are tested.

use base64::Engine;
use minisign_verify::{PublicKey, Signature};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Where this app reads its updates: a fixed release tag whose feed file every release replaces.
/// Sirus Code installs read `releases/latest/download/latest.json` and never see this one.
pub const FEED_URL: &str =
    "https://github.com/viannaanalyst/sirus-code/releases/download/sirus-channel/sirus-latest.json";
/// The updater key's public half, as in `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`);
/// a test keeps the two equal.
pub const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDZGNDdGMUM5M0JBOEVDMjgKUldRbzdLZzd5ZkZIYjRHWVNZcHA4NjdXK2lWUXdpTUlKRnNGWmhhWWV0MStrTTdRY1Q4VGZmdWEK";
const PLATFORM: &str = "darwin-aarch64";
/// The bundle every release holds, and the one an update leaves in place.
pub const BUNDLE_NAME: &str = "Sirus.app";
const BUNDLE_ID: &str = "com.siruscode.native";
const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// The first automatic check waits for the window and the host to settle.
pub const START_DELAY: Duration = Duration::from_secs(20);
/// Automatic checks repeat this often while the app stays open.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);
const MAX_NOTES: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    /// On disk; the new version runs after the next launch.
    Installed,
    Failed,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub phase: Phase,
    pub current: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub error: Option<String>,
    /// Where the installed bundle is, once an update was installed.
    pub installed_at: Option<PathBuf>,
}

/// What a feed offers for this Mac.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub signature: String,
}

struct Inner {
    status: Status,
    offer: Option<Offer>,
    last_attempt: Option<Instant>,
}

static STATE: Mutex<Option<Inner>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Inner) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|p| p.into_inner());
    let inner = guard.get_or_insert_with(|| Inner {
        status: Status {
            phase: Phase::Idle,
            current: CURRENT.to_owned(),
            version: None,
            notes: None,
            error: None,
            installed_at: None,
        },
        offer: None,
        last_attempt: None,
    });
    f(inner)
}

pub fn status() -> Status {
    with(|i| i.status.clone())
}

/// Development builds (`cargo run`, `cargo test`) never check on their own.
pub fn automatic_allowed() -> bool {
    !cfg!(debug_assertions)
}

/// Whether an automatic check is due: never tried, or the last try is older than the interval.
pub fn due(last: Option<Instant>, now: Instant, interval: Duration) -> bool {
    last.is_none_or(|at| now.saturating_duration_since(at) >= interval)
}

pub fn automatic_due() -> bool {
    with(|i| due(i.last_attempt, Instant::now(), CHECK_INTERVAL))
}

// ---- Versions --------------------------------------------------------------------------------

/// `1.2.3`, `v1.2.3` or `1.2.3-beta.1`; None for anything else.
pub fn parse_version(raw: &str) -> Option<(u64, u64, u64, Vec<String>)> {
    let raw = raw.trim();
    let raw = raw.strip_prefix('v').unwrap_or(raw);
    let (core, pre) = match raw.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (raw, None),
    };
    let mut numbers = core.split('.');
    let mut next = || {
        let part = numbers.next()?;
        (!part.is_empty()
            && part.bytes().all(|b| b.is_ascii_digit())
            && (part == "0" || !part.starts_with('0')))
        .then(|| part.parse::<u64>().ok())
        .flatten()
    };
    let (major, minor, patch) = (next()?, next()?, next()?);
    if numbers.next().is_some() {
        return None;
    }
    let pre = match pre {
        None => Vec::new(),
        Some(pre) => {
            let parts: Vec<String> = pre.split('.').map(str::to_owned).collect();
            let fine = parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
            if !fine {
                return None;
            }
            parts
        }
    };
    Some((major, minor, patch, pre))
}

/// Semantic-version order: `Less` when `a` is older than `b`. None when either is not a version.
pub fn compare_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering::*;
    let (a, b) = (parse_version(a)?, parse_version(b)?);
    let core = (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2));
    if core != Equal {
        return Some(core);
    }
    // A release is newer than any of its prereleases.
    match (a.3.is_empty(), b.3.is_empty()) {
        (true, true) => return Some(Equal),
        (true, false) => return Some(Greater),
        (false, true) => return Some(Less),
        _ => {}
    }
    for (left, right) in a.3.iter().zip(&b.3) {
        let order = match (left.parse::<u64>(), right.parse::<u64>()) {
            (Ok(l), Ok(r)) => l.cmp(&r),
            (Ok(_), Err(_)) => Less,
            (Err(_), Ok(_)) => Greater,
            (Err(_), Err(_)) => left.cmp(right),
        };
        if order != Equal {
            return Some(order);
        }
    }
    Some(a.3.len().cmp(&b.3.len()))
}

/// Whether `offered` is a version newer than `current`.
pub fn is_newer(offered: &str, current: &str) -> bool {
    compare_versions(offered, current) == Some(std::cmp::Ordering::Greater)
}

// ---- The feed --------------------------------------------------------------------------------

/// The offer in a feed (`sirus-latest.json`: version, notes, pub_date, platforms), or why it is
/// unusable. Whether it is newer than this app is `is_newer`'s question.
pub fn parse_feed(text: &str) -> Result<Offer, String> {
    let feed: Value = serde_json::from_str(text).map_err(|_| "The update feed is not valid JSON.".to_owned())?;
    let version = feed["version"].as_str().unwrap_or_default().trim();
    let version = version.strip_prefix('v').unwrap_or(version);
    if parse_version(version).is_none() {
        return Err("The update feed has no valid version.".into());
    }
    let platform = &feed["platforms"][PLATFORM];
    let url = platform["url"].as_str().unwrap_or_default().trim();
    if !url.starts_with("https://") {
        return Err("The update feed has no https download for this Mac.".into());
    }
    let signature = platform["signature"].as_str().unwrap_or_default().trim();
    if signature.is_empty() {
        return Err("The update feed has no signature for this Mac.".into());
    }
    let notes: String = feed["notes"].as_str().unwrap_or_default().trim().chars().take(MAX_NOTES).collect();
    Ok(Offer { version: version.to_owned(), notes, url: url.to_owned(), signature: signature.to_owned() })
}

// ---- The signature ---------------------------------------------------------------------------

fn minisign_text(encoded: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| "malformed minisign data".to_owned())?;
    String::from_utf8(bytes).map_err(|_| "malformed minisign data".to_owned())
}

/// Checks `data` against a Tauri-style signature (base64 of a minisign signature file) and public
/// key (base64 of a minisign public key file). Any failure rejects the update.
pub fn verify(data: &[u8], signature: &str, public_key: &str) -> Result<(), String> {
    let key = PublicKey::decode(&minisign_text(public_key)?).map_err(|e| format!("public key: {e}"))?;
    let signature = Signature::decode(&minisign_text(signature)?).map_err(|e| format!("signature: {e}"))?;
    key.verify(data, &signature, false)
        .map_err(|e| format!("The update's signature does not match: {e}"))
}

// ---- Network and disk ------------------------------------------------------------------------

fn curl(url: &str, to: Option<&Path>) -> Result<Vec<u8>, String> {
    let mut command = Command::new("curl");
    command.args(["--fail", "--silent", "--show-error", "--location", "--proto", "=https", "--max-time", "600"]);
    if let Some(path) = to {
        command.arg("--output").arg(path);
    }
    let output = command.arg(url).output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr);
        return Err(short(why.trim()));
    }
    Ok(output.stdout)
}

fn short(message: &str) -> String {
    let line = message.lines().next().unwrap_or_default();
    line.chars().take(200).collect()
}

fn set(f: impl FnOnce(&mut Inner)) -> Status {
    with(|i| {
        f(i);
        i.status.clone()
    })
}

fn fail(message: String) -> Status {
    set(|i| {
        i.status.phase = Phase::Failed;
        i.status.error = Some(message);
    })
}

fn busy() -> bool {
    matches!(status().phase, Phase::Checking | Phase::Downloading)
}

/// Asks the feed whether a newer version exists. Blocks; run it off the UI thread.
pub fn check() -> Status {
    if busy() {
        return status();
    }
    set(|i| {
        i.status.phase = Phase::Checking;
        i.status.error = None;
        i.last_attempt = Some(Instant::now());
    });
    let text = match curl(FEED_URL, None) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => return fail(e),
    };
    match parse_feed(&text) {
        Err(e) => fail(e),
        Ok(offer) if is_newer(&offer.version, CURRENT) => set(|i| {
            i.status.phase = Phase::Available;
            i.status.version = Some(offer.version.clone());
            i.status.notes = Some(offer.notes.clone());
            i.offer = Some(offer);
        }),
        Ok(_) => set(|i| {
            i.status.phase = Phase::UpToDate;
            i.status.version = None;
            i.status.notes = None;
            i.offer = None;
        }),
    }
}

/// The `.app` this process runs from (`X.app/Contents/MacOS/exe`), None outside a bundle.
pub fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.ancestors().nth(3)?;
    (bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

/// Where an update goes when the running app is `running`: `Sirus.app` beside it, so an old
/// `Sirus Native.app` is replaced by `Sirus.app` in the same folder.
pub fn target_for(running: &Path) -> PathBuf {
    running.with_file_name(BUNDLE_NAME)
}

/// Downloads the offer, verifies its signature, unpacks it beside the installed app and swaps the
/// bundle. Blocks; `progress` is called when the phase changes. Never restarts.
pub fn install(progress: impl Fn()) -> Status {
    if busy() {
        return status();
    }
    let Some(offer) = with(|i| i.offer.clone()) else {
        return fail("No update is waiting.".into());
    };
    set(|i| {
        i.status.phase = Phase::Downloading;
        i.status.error = None;
    });
    progress();
    let result = running_bundle()
        .ok_or_else(|| "This copy does not run from an app bundle, so it cannot update itself.".to_owned())
        .and_then(|running| install_bundle(&offer, &running));
    match result {
        Ok(target) => set(|i| {
            i.status.phase = Phase::Installed;
            i.status.installed_at = Some(target);
        }),
        Err(e) => fail(e),
    }
}

fn install_bundle(offer: &Offer, running: &Path) -> Result<PathBuf, String> {
    let target = target_for(running);
    let folder = target.parent().ok_or("No folder for the app.")?;
    // Beside the app: the swap is then a rename on one volume.
    let staging = folder.join(format!(".Sirus-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging)
        .map_err(|e| format!("{} is not writable ({e}); move Sirus to a folder you can write to.", folder.display()))?;
    let outcome = swap_in(offer, running, &target, &staging);
    let _ = std::fs::remove_dir_all(&staging);
    outcome.map(|()| target)
}

fn swap_in(offer: &Offer, running: &Path, target: &Path, staging: &Path) -> Result<(), String> {
    let archive = staging.join("update.tar.gz");
    curl(&offer.url, Some(&archive))?;
    let data = std::fs::read(&archive).map_err(|e| e.to_string())?;
    verify(&data, &offer.signature, PUBLIC_KEY)?;
    drop(data);
    let unpacked = staging.join("new");
    std::fs::create_dir(&unpacked).map_err(|e| e.to_string())?;
    let tar = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&unpacked)
        .output()
        .map_err(|e| e.to_string())?;
    if !tar.status.success() {
        return Err(short(String::from_utf8_lossy(&tar.stderr).trim()));
    }
    let fresh = unpacked.join(BUNDLE_NAME);
    check_bundle(&fresh)?;
    let old = staging.join("old");
    let replacing = target.exists();
    if replacing {
        std::fs::rename(target, &old).map_err(|e| e.to_string())?;
    }
    if let Err(e) = std::fs::rename(&fresh, target) {
        if replacing {
            let _ = std::fs::rename(&old, target);
        }
        return Err(e.to_string());
    }
    // The previous name (Sirus Native.app) goes too: Sirus.app replaces it.
    if running != target && running.exists() {
        let _ = std::fs::rename(running, staging.join("renamed"));
    }
    Ok(())
}

/// A bundle that is the Sirus app: its executable and host are there and the id is ours.
fn check_bundle(bundle: &Path) -> Result<(), String> {
    for part in ["Contents/MacOS/sirus-native-spike", "Contents/MacOS/sirus-host"] {
        if !bundle.join(part).is_file() {
            return Err("The update does not hold the Sirus app.".into());
        }
    }
    let plist = std::fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap_or_default();
    if !plist.contains(BUNDLE_ID) {
        return Err("The update is for a different app.".into());
    }
    Ok(())
}

/// Opens the installed app once this process is gone, then lets the caller quit. The shell calls it
/// only after the person asked and the running-agents confirmation passed.
pub fn relaunch() -> Result<(), String> {
    let Some(target) = status().installed_at else {
        return Err("No update is waiting for a restart.".into());
    };
    Command::new("sh")
        .arg("-c")
        // Wait (at most ~20 s) for this process to exit, so the host shuts down first.
        .arg("i=0; while kill -0 \"$1\" 2>/dev/null && [ $i -lt 100 ]; do sleep 0.2; i=$((i+1)); done; open -n \"$2\"")
        .arg("sirus-relaunch")
        .arg(std::process::id().to_string())
        .arg(target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    // A throwaway key pair made with `tauri signer generate`; it signs only this payload.
    const TEST_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDVGRTg3NjJFMkQ2RTY2NkUKUldSdVptNHRMbmJvWDlQWkRmQVo1ODltTFllbHo3OHdYQkpwenVtSTJYZEpHVFpGbUpiNkVDa0kK";
    const TEST_SIGNATURE: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSdVptNHRMbmJvWHhRVjVMK3hhbHBvY09CdTloSDlSNUhQWmRTMWVESmVoUExya3JIdHNkRVRQc2ZNbk0xVWJJZWNmQklrUW5rbE9zQU9CbVZJdDRYaEpMOXBJRVF1ckFzPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkzOTMzODE2CWZpbGU6cGF5bG9hZC5iaW4KNktWQWJ4b0ZVdXJBOWNhWWhhNmRNM3c1SGkvb3VJNXZwRTBQRURGdmlWdkp2akIyVEdZUmlZOHZ4Smt0d1JrQklEUEcxV093VVIxS2c5cmxnbzN3QXc9PQo=";
    const TEST_PAYLOAD: &[u8] = b"sirus update test payload";

    #[test]
    fn public_key_is_the_one_in_tauri_conf() {
        let conf: Value = serde_json::from_str(include_str!("../../src-tauri/tauri.conf.json")).unwrap();
        assert_eq!(conf["plugins"]["updater"]["pubkey"].as_str(), Some(PUBLIC_KEY));
        assert!(PublicKey::decode(&minisign_text(PUBLIC_KEY).unwrap()).is_ok());
    }

    #[test]
    fn versions_compare_as_semver() {
        assert_eq!(compare_versions("0.0.2", "0.0.1"), Some(Greater));
        assert_eq!(compare_versions("0.1.0", "0.0.9"), Some(Greater));
        assert_eq!(compare_versions("1.0.0", "0.99.99"), Some(Greater));
        assert_eq!(compare_versions("v0.0.1", "0.0.1"), Some(Equal));
        assert_eq!(compare_versions("1.0.0", "1.0.0-beta.1"), Some(Greater));
        assert_eq!(compare_versions("1.0.0-beta.2", "1.0.0-beta.10"), Some(Less));
        assert_eq!(compare_versions("1.0.0-alpha", "1.0.0-beta"), Some(Less));
        assert_eq!(compare_versions("1.0.0-1", "1.0.0-alpha"), Some(Less));
        assert_eq!(compare_versions("1.0.0-beta", "1.0.0-beta.1"), Some(Less));
        assert_eq!(compare_versions("1.2", "1.2.0"), None);
        assert_eq!(compare_versions("01.2.0", "1.2.0"), None);
        assert!(is_newer("0.0.2", "0.0.1"));
        assert!(!is_newer("0.0.1", "0.0.1"));
        assert!(!is_newer("0.0.1", "0.0.2"));
        assert!(!is_newer("garbage", "0.0.1"));
    }

    fn feed(version: &str, url: &str, signature: &str) -> String {
        serde_json::json!({
            "version": version,
            "notes": "  - Faster  ",
            "pub_date": "2026-11-05T12:00:00Z",
            "platforms": { "darwin-aarch64": { "url": url, "signature": signature } }
        })
        .to_string()
    }

    #[test]
    fn feed_parses_and_refuses_what_is_unusable() {
        let offer = parse_feed(&feed("v0.0.2", "https://example.com/a.tar.gz", " sig ")).unwrap();
        assert_eq!(
            offer,
            Offer {
                version: "0.0.2".into(),
                notes: "- Faster".into(),
                url: "https://example.com/a.tar.gz".into(),
                signature: "sig".into(),
            }
        );
        assert!(parse_feed("not json").is_err());
        assert!(parse_feed(&feed("soon", "https://x/a", "sig")).is_err());
        assert!(parse_feed(&feed("0.0.2", "http://x/a", "sig")).is_err());
        assert!(parse_feed(&feed("0.0.2", "https://x/a", " ")).is_err());
        let other = r#"{"version":"0.0.2","platforms":{"linux-x86_64":{"url":"https://x/a","signature":"s"}}}"#;
        assert!(parse_feed(other).is_err());
        let long = serde_json::json!({
            "version": "0.0.2", "notes": "x".repeat(MAX_NOTES + 50),
            "platforms": { "darwin-aarch64": { "url": "https://x/a", "signature": "s" } }
        });
        assert_eq!(parse_feed(&long.to_string()).unwrap().notes.chars().count(), MAX_NOTES);
    }

    #[test]
    fn a_valid_signature_is_accepted() {
        assert_eq!(verify(TEST_PAYLOAD, TEST_SIGNATURE, TEST_KEY), Ok(()));
    }

    #[test]
    fn signatures_that_do_not_hold_are_rejected() {
        // Changed bytes.
        assert!(verify(b"sirus update test payloae", TEST_SIGNATURE, TEST_KEY).is_err());
        // Signed by another key than the one the app trusts.
        assert!(verify(TEST_PAYLOAD, TEST_SIGNATURE, PUBLIC_KEY).is_err());
        // Not a signature at all, and not base64.
        assert!(verify(TEST_PAYLOAD, "", TEST_KEY).is_err());
        assert!(verify(TEST_PAYLOAD, "bm90IGEgc2lnbmF0dXJl", TEST_KEY).is_err());
        assert!(verify(TEST_PAYLOAD, "%%%", TEST_KEY).is_err());
        assert!(verify(TEST_PAYLOAD, TEST_SIGNATURE, "").is_err());
    }

    #[test]
    fn automatic_checks_wait_for_the_interval() {
        let start = Instant::now();
        assert!(due(None, start, CHECK_INTERVAL));
        assert!(!due(Some(start), start + Duration::from_secs(60), CHECK_INTERVAL));
        assert!(due(Some(start), start + CHECK_INTERVAL, CHECK_INTERVAL));
    }

    #[test]
    fn an_update_lands_as_sirus_app_beside_the_old_name() {
        assert_eq!(
            target_for(Path::new("/Applications/Sirus Native.app")),
            Path::new("/Applications/Sirus.app")
        );
        assert_eq!(target_for(Path::new("/Applications/Sirus.app")), Path::new("/Applications/Sirus.app"));
    }
}
