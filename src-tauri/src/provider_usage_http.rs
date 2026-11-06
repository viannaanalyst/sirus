//! Owner-authorized, read-only quota adapters. Secrets never cross IPC or enter argv/logs.
use crate::models::AgentProviderId;
use crate::provider_usage::{account_text, ProviderUsage, UsageAccount, UsageWindow};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest::header::{HeaderValue, AUTHORIZATION, COOKIE};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

const LIMIT: usize = 256 * 1024;
const CURSOR_USAGE: &str = "https://cursor.com/api/usage-summary";
const CURSOR_ACCOUNT: &str = "https://cursor.com/api/auth/me";
const GO_USAGE: &str = "https://opencode.ai/zen/go/v1/usage";

// No Debug/Serialize: this native-owned value contains a credential.
pub(crate) struct Credential {
    secret: String,
    pub stamp: String,
    account: UsageAccount,
}

#[derive(Clone, Copy)]
pub(crate) enum Failure {
    Missing,
    Expired,
    Rejected,
    NoPlan,
    Network,
    Invalid,
}

impl Failure {
    pub fn usage(self, provider: AgentProviderId) -> ProviderUsage {
        let note = match self {
            Self::Missing => "No existing CLI credential was found. Sign in with the provider's own CLI, then refresh.",
            Self::Expired => "The existing CLI credential expired. Sign in with the provider's own CLI, then refresh.",
            Self::Rejected => "The provider rejected the existing credential. Check its CLI login, then refresh.",
            Self::NoPlan => "This key has no OpenCode Go subscription. Other OpenCode backends have separate limits.",
            Self::Network => "Could not reach the provider's usage service. Refresh usage to try again.",
            Self::Invalid => "The provider's quota response or credential format is unsupported. Update the CLI and refresh.",
        };
        let mut usage = ProviderUsage::unavailable(provider, note);
        if matches!(self, Self::Network | Self::Invalid) {
            usage.status = "error".into();
        }
        usage
    }
}

fn fingerprint(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn credential(secret: String) -> Credential {
    let stamp = fingerprint(&secret);
    Credential {
        secret,
        account: UsageAccount {
            email: None,
            name: None,
            plan: None,
            key_fingerprint: Some(stamp[..12].to_owned()),
        },
        stamp,
    }
}

fn parse_go_auth(value: &Value) -> Result<Credential, Failure> {
    let row = &value["opencode-go"];
    if !row.is_object() {
        return Err(Failure::Missing);
    }
    let key = row["key"].as_str().ok_or(Failure::Invalid)?;
    if row["type"] != "api"
        || !(16..=4096).contains(&key.len())
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(Failure::Invalid);
    }
    Ok(credential(key.to_owned()))
}

fn cursor_credential(token: &str, now: i64) -> Result<Credential, Failure> {
    if token.len() > 16_384
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(Failure::Invalid);
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return Err(Failure::Invalid);
    }
    let payload = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| Failure::Invalid)?;
    let value: Value = serde_json::from_slice(&payload).map_err(|_| Failure::Invalid)?;
    if value["exp"].as_i64().is_none_or(|exp| exp <= now + 60) {
        return Err(Failure::Expired);
    }
    let subject = value["sub"]
        .as_str()
        .and_then(|s| s.rsplit('|').next())
        .ok_or(Failure::Invalid)?;
    if subject.is_empty()
        || subject.len() > 200
        || !subject
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(Failure::Invalid);
    }
    // Claims only construct the vendor's cookie format; HTTPS authenticates the account.
    let mut result = credential(token.to_owned());
    result.secret = format!("WorkosCursorSessionToken={subject}%3A%3A{token}");
    Ok(result)
}

fn read_json(path: &Path) -> Result<Value, Failure> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| Failure::Missing)?;
    let meta = file.metadata().map_err(|_| Failure::Invalid)?;
    if !meta.is_file() || meta.len() > LIMIT as u64 {
        return Err(Failure::Invalid);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } {
            return Err(Failure::Invalid);
        }
    }
    let mut bytes = Vec::new();
    file.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::Invalid)?;
    if bytes.len() > LIMIT {
        return Err(Failure::Invalid);
    }
    serde_json::from_slice(&bytes).map_err(|_| Failure::Invalid)
}

fn absolute_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn go_auth_path() -> Result<PathBuf, Failure> {
    if let Some(path) = absolute_env("OPENCODE_DATA_DIR") {
        return Ok(path.join("auth.json"));
    }
    let home = absolute_env("HOME")
        .or_else(|| absolute_env("USERPROFILE"))
        .ok_or(Failure::Missing)?;
    let root = absolute_env("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
    let app = std::env::var("OPENCODE_APPNAME").unwrap_or_else(|_| "opencode".into());
    if app.is_empty()
        || app.len() > 64
        || app == "."
        || app == ".."
        || !app
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(Failure::Invalid);
    }
    Ok(root.join(app).join("auth.json"))
}

fn load_go() -> Result<Credential, Failure> {
    // Explicit injected auth is authoritative. Never fall through to another account.
    if let Ok(raw) = std::env::var("OPENCODE_AUTH_CONTENT") {
        if raw.len() > LIMIT {
            return Err(Failure::Invalid);
        }
        return parse_go_auth(&serde_json::from_str::<Value>(&raw).map_err(|_| Failure::Invalid)?);
    }
    parse_go_auth(&read_json(&go_auth_path()?)?)
}

async fn load_cursor() -> Result<Credential, Failure> {
    #[cfg(target_os = "macos")]
    {
        // Exact CLI-owned item only. No keychain enumeration, refresh token or login changes.
        let output = crate::cli_output::capture(
            "/usr/bin/security",
            &[
                "find-generic-password",
                "-s",
                "cursor-access-token",
                "-a",
                "cursor-user",
                "-w",
            ],
            Duration::from_secs(5),
        )
        .await
        .map_err(|_| Failure::Missing)?;
        if !output.status.success() {
            return Err(Failure::Missing);
        }
        let token = std::str::from_utf8(&output.stdout)
            .map_err(|_| Failure::Invalid)?
            .trim();
        cursor_credential(token, chrono::Utc::now().timestamp())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let path = if cfg!(target_os = "windows") {
            absolute_env("APPDATA")
                .ok_or(Failure::Missing)?
                .join("Cursor/auth.json")
        } else {
            absolute_env("XDG_CONFIG_HOME")
                .or_else(|| absolute_env("HOME").map(|p| p.join(".config")))
                .ok_or(Failure::Missing)?
                .join("cursor/auth.json")
        };
        let value = tokio::task::spawn_blocking(move || read_json(&path))
            .await
            .map_err(|_| Failure::Invalid)??;
        cursor_credential(
            value["accessToken"].as_str().ok_or(Failure::Missing)?,
            chrono::Utc::now().timestamp(),
        )
    }
}

pub(crate) async fn load(provider: &AgentProviderId) -> Result<Credential, Failure> {
    match provider {
        AgentProviderId::Cursor => load_cursor().await,
        AgentProviderId::OpenCode => tokio::task::spawn_blocking(load_go)
            .await
            .map_err(|_| Failure::Invalid)?,
        _ => Err(Failure::Invalid),
    }
}

fn client() -> Result<&'static reqwest::Client, Failure> {
    static HTTP: OnceLock<Result<reqwest::Client, ()>> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .user_agent("Sirus/0.1.0")
            .pool_max_idle_per_host(1)
            .build()
            .map_err(|_| ())
    })
    .as_ref()
    .map_err(|_| Failure::Network)
}

async fn get(
    endpoint: &'static str,
    credential: &Credential,
    cookie: bool,
) -> Result<Value, Failure> {
    // This function and its three endpoints are private; IPC accepts no URL or headers.
    let text = if cookie {
        credential.secret.clone()
    } else {
        format!("Bearer {}", credential.secret)
    };
    let mut header = HeaderValue::from_str(&text).map_err(|_| Failure::Invalid)?;
    header.set_sensitive(true);
    let mut response = client()?
        .get(endpoint)
        .header(if cookie { COOKIE } else { AUTHORIZATION }, header)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| Failure::Network)?;
    match response.status().as_u16() {
        200 => {}
        401 => return Err(Failure::Rejected),
        403 if endpoint == GO_USAGE => return Err(Failure::NoPlan),
        403 => return Err(Failure::Rejected),
        _ => return Err(Failure::Network),
    }
    if response
        .content_length()
        .is_some_and(|len| len > LIMIT as u64)
    {
        return Err(Failure::Invalid);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Network)? {
        if bytes.len() + chunk.len() > LIMIT {
            return Err(Failure::Invalid);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Failure::Invalid)
}

fn percent(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 10_000.0)
        .map(|n| n.min(100.0))
}
fn date(value: &Value) -> Option<i64> {
    value
        .as_str()
        .filter(|s| s.len() <= 64)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis())
        .filter(|n| *n > 0)
}

fn parse_go(value: &Value) -> ProviderUsage {
    let mut usage = ProviderUsage::unavailable(
        AgentProviderId::OpenCode,
        "OpenCode Go returned no quota windows.",
    );
    for (name, duration) in [
        ("rolling", Some(300)),
        ("weekly", Some(10_080)),
        ("monthly", None),
    ] {
        let row = &value["usage"][name];
        if !row.is_object()
            || !["ok", "rate-limited"].contains(&row["status"].as_str().unwrap_or(""))
        {
            continue;
        }
        usage.windows.push(UsageWindow {
            id: format!("opencode-go/{name}"),
            used_percent: percent(&row["percent"]),
            resets_at: date(&row["resetsAt"]),
            duration_minutes: duration,
        });
    }
    if usage.windows.iter().any(|w| w.used_percent.is_some()) {
        usage.status = "available".into();
        usage.note = None;
    }
    usage
}

fn parse_cursor(value: &Value) -> ProviderUsage {
    let mut usage = ProviderUsage::unavailable(
        AgentProviderId::Cursor,
        "Cursor returned no supported quota windows for this account.",
    );
    let end = date(&value["billingCycleEnd"]);
    let duration = date(&value["billingCycleStart"])
        .zip(end)
        .and_then(|(a, b)| u64::try_from((b - a) / 60_000).ok())
        .filter(|d| *d > 0 && *d <= 525_600);
    let plan = &value["individualUsage"]["plan"];
    for (field, id) in [
        ("autoPercentUsed", "cursor/models"),
        ("apiPercentUsed", "cursor/other-models"),
    ] {
        if let Some(used) = percent(&plan[field]) {
            usage.windows.push(UsageWindow {
                id: id.into(),
                used_percent: Some(used),
                resets_at: end,
                duration_minutes: duration,
            });
        }
    }
    if usage.windows.is_empty() {
        for (row, id) in [
            (plan, "cursor/included"),
            (&value["individualUsage"]["overall"], "cursor/member"),
            (&value["teamUsage"]["pooled"], "cursor/team"),
        ] {
            let percentage = percent(&row["totalPercentUsed"]).or_else(|| {
                let used = row["used"].as_f64()?;
                let limit = row["limit"].as_f64()?;
                (used.is_finite() && limit.is_finite() && used >= 0.0 && limit > 0.0)
                    .then_some((used / limit * 100.0).min(100.0))
            });
            if let Some(used) = percentage {
                usage.windows.push(UsageWindow {
                    id: id.into(),
                    used_percent: Some(used),
                    resets_at: end,
                    duration_minutes: duration,
                });
                break;
            }
        }
    }
    if !usage.windows.is_empty() {
        usage.status = "available".into();
        usage.note = None;
    }
    usage
}

pub(crate) async fn fetch(provider: &AgentProviderId, credential: &Credential) -> ProviderUsage {
    let (response, profile) = if *provider == AgentProviderId::Cursor {
        let (usage, profile) = tokio::join!(
            get(CURSOR_USAGE, credential, true),
            get(CURSOR_ACCOUNT, credential, true)
        );
        (usage, profile.ok())
    } else {
        (get(GO_USAGE, credential, false).await, None)
    };
    let mut usage = match response {
        Ok(ref value) if *provider == AgentProviderId::Cursor => parse_cursor(value),
        Ok(ref value) => parse_go(value),
        Err(failure) => failure.usage(provider.clone()),
    };
    let mut account = credential.account.clone();
    if let Some(profile) = profile {
        account.email = account_text(&profile["email"])
            .filter(|email| email.contains('@') && !email.chars().any(char::is_whitespace));
        account.name = account_text(&profile["name"]);
    }
    if let Ok(value) = response {
        account.plan = if *provider == AgentProviderId::Cursor {
            account_text(&value["membershipType"])
        } else {
            Some("OpenCode Go".into())
        };
    }
    usage.account = Some(account);
    usage
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn go_windows_use_server_percentages_and_never_export_auth() {
        let raw = json!({"usage":{"rolling":{"status":"ok","percent":0.5,"resetsAt":"2026-10-01T14:00:00Z"},"weekly":{"status":"rate-limited","percent":104},"monthly":{"status":"ok","percent":null}},"key":"secret"});
        let usage = parse_go(&raw);
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(usage.windows[0].used_percent, Some(0.5));
        assert_eq!(usage.windows[1].used_percent, Some(100.0));
        assert_eq!(usage.windows[2].used_percent, None);
        assert!(usage.windows[0].resets_at.is_some());
        assert!(!serde_json::to_string(&usage).unwrap().contains("secret"));
        assert!(
            parse_go(&json!({"usage":{"weekly":{"status":"future","percent":10}}}))
                .windows
                .is_empty()
        );
    }
    #[test]
    fn cursor_preserves_two_pools_fractional_percent_and_real_caps() {
        let raw = json!({"billingCycleStart":"2026-09-15T12:00:00Z","billingCycleEnd":"2026-10-15T12:00:00Z","individualUsage":{"plan":{"autoPercentUsed":0.36,"apiPercentUsed":51}}});
        let usage = parse_cursor(&raw);
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].used_percent, Some(0.36));
        assert_eq!(usage.windows[1].used_percent, Some(51.0));
        assert_eq!(usage.windows[0].duration_minutes, Some(43_200));
        assert!(
            parse_cursor(&json!({"individualUsage":{"plan":{"used":0,"limit":0}}}))
                .windows
                .is_empty()
        );
        assert_eq!(
            parse_cursor(&json!({"individualUsage":{"overall":{"used":8,"limit":20}}})).windows[0]
                .used_percent,
            Some(40.0)
        );
    }
    #[test]
    fn credentials_are_scoped_and_identity_is_a_digest_not_a_key() {
        let secret = "sk-go-disposable-fixture-not-real";
        let credential = parse_go_auth(&json!({"opencode-go":{"type":"api","key":secret}}))
            .ok()
            .unwrap();
        assert_eq!(credential.stamp, fingerprint(secret));
        assert_eq!(
            credential.account.key_fingerprint.as_ref().unwrap().len(),
            12
        );
        assert!(!serde_json::to_string(&credential.account)
            .unwrap()
            .contains(secret));
        assert!(parse_go_auth(&json!({"anthropic":{"type":"api","key":secret}})).is_err());
        assert!(
            parse_go_auth(&json!({"opencode-go":{"type":"api","key":"key\ninjection"}})).is_err()
        );
        let token = format!(
            "e30.{}.fixture",
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&json!({"sub":"auth|user_fixture","exp":5000})).unwrap()
            )
        );
        assert!(cursor_credential(&token, 5001).is_err());
        assert!(cursor_credential(&token, 1000).is_ok());
        assert!(cursor_credential("bad;cookie=foreign", 1000).is_err());
    }
    #[test]
    fn reader_is_bounded_and_does_not_follow_links_or_accept_special_files() {
        let repo = crate::git::tests::Repo::new();
        let path = repo.0.join("auth.json");
        std::fs::write(&path, b"{}").unwrap();
        assert!(read_json(&path).is_ok());
        std::fs::write(&path, vec![b' '; LIMIT + 1]).unwrap();
        assert!(read_json(&path).is_err());
        assert!(read_json(&repo.0).is_err());
        #[cfg(unix)]
        {
            let link = repo.0.join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(read_json(&link).is_err());
        }
    }
    #[tokio::test]
    #[ignore = "read-only owner-authorized Cursor/OpenCode quotas; no inference or account changes"]
    async fn live_http_quotas_without_inference() {
        for provider in [AgentProviderId::Cursor, AgentProviderId::OpenCode] {
            let credential = load(&provider)
                .await
                .ok()
                .expect("Existing CLI credential unavailable");
            let usage = fetch(&provider, &credential).await;
            assert_eq!(usage.status, "available");
            assert!(!usage.windows.is_empty());
            assert!(usage.account.is_some());
            let account = usage.account.as_ref().unwrap();
            if provider == AgentProviderId::Cursor {
                assert!(account.email.is_some());
            } else {
                assert!(account.email.is_none());
                assert!(account.key_fingerprint.is_some());
            }
            println!(
                "{}: {} real quota windows; account metadata present; no inference",
                provider.key(),
                usage.windows.len()
            );
        }
    }
}
