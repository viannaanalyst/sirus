//! Site favicons for link chips (ADR-149). The renderer asks for a host — never a full
//! URL — and gets a small image as a data URL, or none. Sources, in order: Google's
//! favicon service, DuckDuckGo's, then the site's own `/favicon.ico`. Answers are kept
//! in memory and in `favicons/` next to the state (a day; a failure for an hour, at most
//! 500 hosts), and concurrent asks for one host share a single lookup. Local, private
//! and single-label hosts are refused before anything leaves the Mac.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use base64::Engine;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::commands::AppState;
use crate::error::Result;
use crate::runtime::State;

/// Favicons are tiny; anything larger is not one.
pub(crate) const MAX_BYTES: usize = 64 * 1024;
pub(crate) const MAX_ENTRIES: usize = 500;
const FOUND_TTL: i64 = 24 * 60 * 60;
const MISSING_TTL: i64 = 60 * 60;
/// Private names a lookup would leak to the favicon services, or that only resolve locally.
const LOCAL_SUFFIXES: &[&str] = &[
    "localhost",
    "local",
    "internal",
    "lan",
    "home",
    "corp",
    "intranet",
    "test",
    "example",
    "invalid",
    "home.arpa",
    "ts.net",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    /// Unix seconds of the lookup.
    pub at: i64,
    pub data_url: Option<String>,
}

static MEMORY: LazyLock<Mutex<HashMap<String, Entry>>> = LazyLock::new(Default::default);
/// One gate per host being looked up: later asks wait for it and read the cache.
static GATES: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

/// The host's favicon as a data URL, or none (unknown site, local host, offline).
pub async fn site_favicon(state: State<'_, Arc<AppState>>, host: String) -> Result<Option<String>> {
    let Some(host) = normalize_host(&host) else {
        return Ok(None);
    };
    let dir = state
        .data_path
        .parent()
        .map(|parent| parent.join("favicons"))
        .unwrap_or_else(|| PathBuf::from("favicons"));
    Ok(resolve(dir, host).await)
}

async fn resolve(dir: PathBuf, host: String) -> Option<String> {
    if let Some(hit) = cached(&dir, &host).await {
        return hit;
    }
    let gate = GATES.lock().entry(host.clone()).or_default().clone();
    let _turn = gate.lock().await;
    // Whoever held the gate just stored the answer.
    if let Some(hit) = cached(&dir, &host).await {
        return hit;
    }
    let entry = Entry {
        at: chrono::Utc::now().timestamp(),
        data_url: fetch(&host).await,
    };
    remember(&host, entry.clone());
    let saved = entry.clone();
    let key = host.clone();
    let _ = crate::commands::native_task(move || {
        save(&dir, &key, &saved);
        Ok(())
    })
    .await;
    GATES.lock().remove(&host);
    entry.data_url
}

/// A fresh answer from memory, else from disk; `None` when the host must be looked up.
async fn cached(dir: &Path, host: &str) -> Option<Option<String>> {
    let now = chrono::Utc::now().timestamp();
    if let Some(entry) = MEMORY.lock().get(host).filter(|entry| fresh(entry, now)) {
        return Some(entry.data_url.clone());
    }
    let path = dir.join(format!("{host}.json"));
    let entry = crate::commands::native_task(move || {
        Ok(std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Entry>(&bytes).ok()))
    })
    .await
    .ok()
    .flatten()
    .filter(|entry| fresh(entry, now) && entry.data_url.as_deref().is_none_or(valid_data_url))?;
    remember(host, entry.clone());
    Some(entry.data_url)
}

fn remember(host: &str, entry: Entry) {
    let mut memory = MEMORY.lock();
    memory.insert(host.to_owned(), entry);
    evict(&mut memory);
}

/// Drops the oldest lookups beyond `MAX_ENTRIES`.
pub(crate) fn evict(entries: &mut HashMap<String, Entry>) {
    if entries.len() <= MAX_ENTRIES {
        return;
    }
    let mut ages: Vec<(i64, String)> = entries
        .iter()
        .map(|(host, entry)| (entry.at, host.clone()))
        .collect();
    ages.sort();
    for (_, host) in ages.into_iter().take(entries.len() - MAX_ENTRIES) {
        entries.remove(&host);
    }
}

pub(crate) fn fresh(entry: &Entry, now: i64) -> bool {
    let ttl = if entry.data_url.is_some() {
        FOUND_TTL
    } else {
        MISSING_TTL
    };
    // A clock that moved backwards makes the entry stale rather than eternal.
    entry.at <= now + 60 && now - entry.at < ttl
}

fn save(dir: &Path, host: &str, entry: &Entry) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec(entry) else {
        return;
    };
    let _ = std::fs::write(dir.join(format!("{host}.json")), bytes);
    // The folder keeps at most `MAX_ENTRIES` hosts; the least recently written go first.
    let Ok(files) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = files
        .flatten()
        .filter(|file| file.file_name().to_string_lossy().ends_with(".json"))
        .map(|file| {
            let modified = file
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (modified, file.path())
        })
        .collect();
    if files.len() > MAX_ENTRIES {
        files.sort();
        let extra = files.len() - MAX_ENTRIES;
        for (_, path) in files.into_iter().take(extra) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A public DNS name in lower case without `www.`, or `None` for IP literals, ports,
/// single labels and local names. Only these characters reach a file name or a request.
pub(crate) fn normalize_host(input: &str) -> Option<String> {
    let host = input.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    if host.is_empty() || host.len() > 253 {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2
        || labels.iter().any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return None;
    }
    // A top-level label with a letter: rules out IPv4 literals like 10.0.0.1.
    let top = labels.last()?;
    if top.len() < 2 || !top.bytes().any(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    if LOCAL_SUFFIXES
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
    {
        return None;
    }
    Some(host)
}

/// Addresses the direct `/favicon.ico` request must never reach.
pub(crate) fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..=127).contains(&b))
                || (a == 198 && (18..=19).contains(&b))
                || a >= 240
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
        }
    }
}

/// The body as a data URL when it is a non-empty image within `MAX_BYTES`.
pub(crate) fn image_data_url(content_type: &str, bytes: &[u8]) -> Option<String> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if bytes.is_empty()
        || bytes.len() > MAX_BYTES
        || !mime.starts_with("image/")
        || !mime
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'+' | b'-'))
    {
        return None;
    }
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn valid_data_url(value: &str) -> bool {
    value.starts_with("data:image/") && value.len() <= MAX_BYTES * 2
}

async fn fetch(host: &str) -> Option<String> {
    let services = [
        format!("https://www.google.com/s2/favicons?domain={host}&sz=64"),
        format!("https://icons.duckduckgo.com/ip3/{host}.ico"),
    ];
    if let Some(client) = service_client() {
        for url in services {
            if let Some(found) = get(client, &url).await {
                return Some(found);
            }
        }
    }
    direct(host).await
}

/// The favicon services may redirect (Google answers from gstatic), over https only.
fn service_client() -> Option<&'static reqwest::Client> {
    static HTTP: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    HTTP.get_or_init(|| {
        builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 3 || attempt.url().scheme() != "https" {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .timeout(Duration::from_secs(5))
            .build()
            .ok()
    })
    .as_ref()
}

fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .https_only(true)
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(4))
        .user_agent("Sirus Code")
        .pool_max_idle_per_host(1)
}

/// The site's own `/favicon.ico`, only when every address of the host is public. The
/// request is pinned to the checked address and follows no redirect.
async fn direct(host: &str) -> Option<String> {
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, 443)).await.ok()?.collect();
    let first = *addresses.first()?;
    if addresses.iter().any(|address| is_private_ip(address.ip())) {
        return None;
    }
    let client = builder()
        .redirect(reqwest::redirect::Policy::none())
        .resolve(host, first)
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    get(&client, &format!("https://{host}/favicon.ico")).await
}

async fn get(client: &reqwest::Client, url: &str) -> Option<String> {
    let mut response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "image/*")
        .send()
        .await
        .ok()?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return None;
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > MAX_BYTES {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    image_data_url(&content_type, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_public_names_only() {
        assert_eq!(normalize_host("GitHub.com").as_deref(), Some("github.com"));
        assert_eq!(
            normalize_host("www.example.org").as_deref(),
            Some("example.org")
        );
        assert_eq!(normalize_host("docs.rs.").as_deref(), Some("docs.rs"));
        for refused in [
            "",
            "localhost",
            "app.localhost",
            "printer.local",
            "build.internal",
            "mac.tail1234.ts.net",
            "router.home.arpa",
            "intranet",
            "10.0.0.1",
            "127.0.0.1",
            "192.168.1.20",
            "[::1]",
            "::1",
            "example.com:8080",
            "example.com/path",
            "../etc/passwd",
            "a..b.com",
            "-bad.com",
            "bad-.com",
            "exa mple.com",
            "x.c",
        ] {
            assert_eq!(normalize_host(refused), None, "{refused}");
        }
        assert_eq!(normalize_host(&format!("{}.com", "a".repeat(64))), None);
    }

    #[test]
    fn private_addresses_are_refused() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.0.10",
            "169.254.169.254",
            "100.100.100.100",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_private_ip(private.parse().unwrap()), "{private}");
        }
        for public in ["140.82.112.3", "1.1.1.1", "2606:4700::1111"] {
            assert!(!is_private_ip(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn only_small_images_become_data_urls() {
        assert_eq!(
            image_data_url("image/png; charset=binary", &[1, 2, 3]).as_deref(),
            Some("data:image/png;base64,AQID")
        );
        assert!(image_data_url("image/x-icon", &vec![0; MAX_BYTES]).is_some());
        assert!(image_data_url("image/x-icon", &vec![0; MAX_BYTES + 1]).is_none());
        assert!(image_data_url("text/html", b"<html>").is_none());
        assert!(image_data_url("image/png", &[]).is_none());
        assert!(image_data_url("image/png\"><script>", &[1]).is_none());
    }

    #[test]
    fn found_icons_last_a_day_and_failures_an_hour() {
        let now = 1_000_000;
        let found = |at| Entry {
            at,
            data_url: Some("data:image/png;base64,AA".into()),
        };
        let missing = |at| Entry { at, data_url: None };
        assert!(fresh(&found(now - FOUND_TTL + 1), now));
        assert!(!fresh(&found(now - FOUND_TTL), now));
        assert!(fresh(&missing(now - MISSING_TTL + 1), now));
        assert!(!fresh(&missing(now - MISSING_TTL), now));
        assert!(!fresh(&found(now + 3600), now));
    }

    #[test]
    fn the_cache_keeps_the_newest_entries() {
        let mut entries: HashMap<String, Entry> = (0..MAX_ENTRIES + 3)
            .map(|index| {
                (
                    format!("site{index}.com"),
                    Entry {
                        at: index as i64,
                        data_url: None,
                    },
                )
            })
            .collect();
        evict(&mut entries);
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert!(!entries.contains_key("site0.com") && !entries.contains_key("site2.com"));
        assert!(entries.contains_key("site3.com"));
    }

    #[test]
    fn saved_entries_stay_within_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..MAX_ENTRIES + 2 {
            save(
                dir.path(),
                &format!("site{index}.com"),
                &Entry {
                    at: 1,
                    data_url: None,
                },
            );
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), MAX_ENTRIES);
    }
}
