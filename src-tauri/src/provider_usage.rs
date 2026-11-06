//! Fixed account-usage probes. Authorized HTTP readers stay native; no login or inference.
use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::AgentProviderId;
use crate::provider_accounts::AccountScope;
use crate::runtime::State;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Child;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub id: String,
    pub used_percent: Option<f64>,
    pub resets_at: Option<i64>, // UTC milliseconds
    pub duration_minutes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageAccount {
    pub email: Option<String>,
    pub name: Option<String>,
    pub plan: Option<String>,
    pub key_fingerprint: Option<String>,
}

pub(crate) fn account_text(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty() && text.len() <= 200 && !text.chars().any(char::is_control))
        .then(|| text.to_owned())
}

fn cli_account(value: &Value, provider: &AgentProviderId) -> Option<UsageAccount> {
    let (account, plan) = if *provider == AgentProviderId::Codex {
        (&value["account"], "planType")
    } else {
        (&value["account"], "subscriptionType")
    };
    if !account.is_object() {
        return None;
    }
    let email = account_text(&account["email"])
        .filter(|email| email.contains('@') && !email.chars().any(char::is_whitespace));
    let plan = account_text(&account[plan]);
    (email.is_some() || plan.is_some()).then_some(UsageAccount {
        email,
        name: None,
        plan,
        key_fingerprint: None,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub provider_account_id: String,
    pub provider: AgentProviderId,
    pub status: String,
    pub windows: Vec<UsageWindow>,
    pub updated_at: i64,
    pub note: Option<String>,
    pub account: Option<UsageAccount>,
    pub reset_count: u64,
    /// A short-lived native offer, never a vendor credit or account identifier.
    pub reset_offer: Option<String>,
}
impl ProviderUsage {
    pub(crate) fn unavailable(provider: AgentProviderId, note: &str) -> Self {
        Self {
            provider_account_id: "default".into(),
            provider,
            status: "unavailable".into(),
            windows: Vec::new(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            note: Some(note.into()),
            account: None,
            reset_count: 0,
            reset_offer: None,
        }
    }
}

struct CachedUsage {
    value: ProviderUsage,
    executable: Option<String>,
    fetched: Instant,
    auth_stamp: Option<String>,
}
struct ResetOffer {
    token: String,
    credit_id: String,
    executable: Option<String>,
    issued: Instant,
    account_email: String,
    account_id: String,
}
impl ResetOffer {
    fn validate_account(&self, account: Option<&UsageAccount>) -> Result<()> {
        if account.and_then(|a| a.email.as_ref()) != Some(&self.account_email) {
            return Err(Error::agent("CLI account changed; refresh usage again."));
        }
        Ok(())
    }
}
pub struct UsageState {
    cache: Mutex<HashMap<(AgentProviderId, String), CachedUsage>>,
    profile_versions: Mutex<HashMap<(AgentProviderId, String), u64>>,
    offer: Mutex<HashMap<String, ResetOffer>>,
    operation: tokio::sync::Mutex<()>,
    cancel: tokio::sync::watch::Sender<bool>,
}
impl Default for UsageState {
    fn default() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            profile_versions: Mutex::new(HashMap::new()),
            offer: Mutex::new(HashMap::new()),
            operation: tokio::sync::Mutex::new(()),
            cancel: tokio::sync::watch::channel(false).0,
        }
    }
}
impl UsageState {
    /// Whether this account's last usage read named its signed-in identity; `None` before any
    /// read (or after a sign-in invalidated it).
    pub(crate) fn signed_in(&self, provider: &AgentProviderId, id: &str) -> Option<bool> {
        self.cache
            .lock()
            .get(&(provider.clone(), id.into()))
            .map(|cached| {
                cached
                    .value
                    .account
                    .as_ref()
                    .is_some_and(|account| account.email.is_some())
            })
    }
    /// Test fixture: a cached read for an account, signed in when `email` is set.
    #[cfg(test)]
    pub(crate) fn remember_account(
        &self,
        provider: AgentProviderId,
        id: &str,
        email: Option<&str>,
    ) {
        let mut value = ProviderUsage::unavailable(provider.clone(), "fixture");
        value.provider_account_id = id.into();
        value.account = Some(UsageAccount {
            email: email.map(str::to_owned),
            name: None,
            plan: None,
            key_fingerprint: None,
        });
        self.cache.lock().insert(
            (provider, id.into()),
            CachedUsage {
                value,
                executable: None,
                fetched: Instant::now(),
                auth_stamp: None,
            },
        );
    }
    pub(crate) fn invalidate(&self, provider: &AgentProviderId, id: &str) {
        let mut versions = self.profile_versions.lock();
        let version = versions.entry((provider.clone(), id.into())).or_default();
        *version = version.wrapping_add(1);
        self.cache.lock().remove(&(provider.clone(), id.into()));
        if *provider == AgentProviderId::Codex {
            self.offer.lock().remove(id);
        }
    }
    fn publish_profile<T>(
        &self,
        key: &(AgentProviderId, String),
        version: u64,
        publish: impl FnOnce() -> T,
    ) -> Result<T> {
        let versions = self.profile_versions.lock();
        if *versions.get(key).unwrap_or(&0) != version {
            return Err(Error::agent("CLI account changed; refresh usage again."));
        }
        Ok(publish())
    }
    pub fn stop(&self) {
        self.cancel.send_replace(true);
    }
    async fn run<T>(
        &self,
        seconds: u64,
        work: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        let mut cancel = self.cancel.subscribe();
        if *cancel.borrow() {
            return Err(Error::agent("Sirus Code is closing"));
        }
        tokio::select! {
            biased;
            _ = cancel.changed() => Err(Error::agent("Sirus Code is closing")),
            result = tokio::time::timeout(Duration::from_secs(seconds), work) => result.map_err(|_| Error::agent("Usage operation timed out. Refresh usage before trying again."))?,
        }
    }
    fn take_offer(
        &self,
        token: &str,
        executable: &Option<String>,
        confirm: bool,
    ) -> Result<ResetOffer> {
        if !confirm || token.len() != 36 || uuid::Uuid::parse_str(token).is_err() {
            return Err(Error::agent(
                "Confirm the available Codex reset before using it.",
            ));
        }
        let mut current = self.offer.lock();
        let account_id = current
            .iter()
            .find_map(|(id, g)| {
                (g.token == token
                    && &g.executable == executable
                    && g.issued.elapsed() < Duration::from_secs(600))
                .then(|| id.clone())
            })
            .ok_or_else(|| {
                Error::agent("Codex reset offer expired or was already used. Refresh usage.")
            })?;
        Ok(current.remove(&account_id).expect("validated offer"))
    }
}

fn percent(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
}
fn seconds(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .filter(|n| (1..=253_402_300_799).contains(n))
        .and_then(|n| n.checked_mul(1000))
}

fn parse_codex(value: &Value) -> ProviderUsage {
    let mut usage = ProviderUsage::unavailable(
        AgentProviderId::Codex,
        "No account usage was returned by the CLI.",
    );
    let buckets = value["rateLimitsByLimitId"].as_object();
    let rows: Vec<(&str, &Value)> = if let Some(buckets) = buckets.filter(|b| !b.is_empty()) {
        buckets
            .iter()
            .take(16)
            .map(|(id, bucket)| (id.as_str(), bucket))
            .collect()
    } else {
        vec![("codex", &value["rateLimits"])]
    };
    for (id, bucket) in rows {
        if id.len() > 128 || id.chars().any(char::is_control) {
            continue;
        }
        for window in ["primary", "secondary"] {
            let row = &bucket[window];
            if row.is_object() {
                usage.windows.push(UsageWindow {
                    id: format!("{id}/{window}"),
                    used_percent: percent(&row["usedPercent"]),
                    resets_at: seconds(&row["resetsAt"]),
                    duration_minutes: row["windowDurationMins"].as_u64().filter(|n| *n <= 525_600),
                });
            }
        }
    }
    usage.reset_count = value["rateLimitResetCredits"]["availableCount"]
        .as_u64()
        .unwrap_or(0)
        .min(100_000);
    if !usage.windows.is_empty() || value["rateLimitResetCredits"].is_object() {
        usage.status = "available".into();
        usage.note = None;
    }
    usage
}
fn offered_credit(value: &Value, credit_id: Option<&str>) -> Option<String> {
    if value["rateLimitResetCredits"]["availableCount"]
        .as_u64()
        .unwrap_or(0)
        == 0
    {
        return None;
    }
    value["rateLimitResetCredits"]["credits"]
        .as_array()?
        .iter()
        .take(128)
        .find_map(|credit| {
            let id = credit["id"].as_str()?;
            (credit["status"] == "available"
                && credit["resetType"] == "codexRateLimits"
                && !id.is_empty()
                && id.len() <= 1024
                && !id.chars().any(char::is_control)
                && credit_id.is_none_or(|wanted| wanted == id)
                && (credit["expiresAt"].is_null()
                    || seconds(&credit["expiresAt"])
                        .is_some_and(|date| date > chrono::Utc::now().timestamp_millis())))
            .then(|| id.to_owned())
        })
}
fn parse_claude(value: &Value) -> ProviderUsage {
    let mut usage = ProviderUsage::unavailable(AgentProviderId::Claude, "The CLI did not provide subscription usage. Update Claude Code or check its existing login.");
    if value["rate_limits_available"] != true {
        return usage;
    }
    for id in [
        "five_hour",
        "seven_day",
        "seven_day_opus",
        "seven_day_sonnet",
        "seven_day_oauth_apps",
    ] {
        let row = &value["rate_limits"][id];
        if !row.is_object() {
            continue;
        }
        usage.windows.push(UsageWindow {
            id: id.into(),
            used_percent: percent(&row["utilization"]),
            resets_at: row["resets_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp_millis()),
            duration_minutes: Some(if id == "five_hour" { 300 } else { 10_080 }),
        });
    }
    if !usage.windows.is_empty() {
        usage.status = "available".into();
        usage.note = None;
    }
    usage
}

/// The unreaped child owns its group even on cancellation/deadline/runtime drop. A probe is
/// never killed outright: reading usage can make the CLI refresh an expired OAuth token, and
/// the rotated credential is written by a `security` child after the answer arrives. Killing
/// the group then loses it and signs the login out for every process (ADR-165), so the input
/// is closed first and the CLI gets `agent::GRACEFUL_EXIT` to finish and exit.
struct Probe {
    account: Option<UsageAccount>,
    child: Option<Child>,
    wire: crate::codex::Wire,
    errors: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Drop for Probe {
    fn drop(&mut self) {
        // Cancelled or past its deadline: the wire (its input) closes with the probe, and the
        // process is retired in the background.
        if let Some(child) = self.child.take() {
            crate::runtime::async_runtime::spawn(crate::agent::retire_owned(child));
        }
        self.errors.abort();
    }
}
impl Probe {
    async fn start(
        binary: &str,
        provider: &AgentProviderId,
        home: Option<&std::path::Path>,
    ) -> Result<Self> {
        let mut command = crate::detect::command(binary);
        crate::provider_accounts::apply(&mut command, provider, home);
        if *provider == AgentProviderId::Codex {
            command.args(["app-server", "--stdio"]);
        } else {
            command.args([
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "manual",
                "--permission-prompts",
                "host",
                "--permission-prompt-tool",
                "stdio",
                "--no-session-persistence",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
                "--settings",
                "{\"disableAllHooks\":true}",
                "--setting-sources",
                "user",
            ]);
        }
        command
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|_| Error::agent("Cannot start the provider usage probe."))?;
        let wire = crate::codex::Wire::new(
            child.stdin.take().expect("piped input"),
            child.stdout.take().expect("piped output"),
        );
        let stderr = child.stderr.take().expect("piped errors");
        let errors = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.take(65_537).read_to_end(&mut bytes).await?;
            if bytes.len() > 65_536 {
                return Err(std::io::Error::other(
                    "Usage probe error output exceeded limit",
                ));
            }
            Ok(())
        });
        Ok(Self {
            account: None,
            child: Some(child),
            wire,
            errors,
        })
    }
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = uuid::Uuid::new_v4().to_string();
        self.wire
            .send(json!({"id":id,"method":method,"params":params}))
            .await?;
        let mut bytes = 0;
        for _ in 0..128 {
            let value = self.wire.read().await?;
            bytes += value.to_string().len();
            if bytes > 2 * 1024 * 1024 {
                break;
            }
            if value["id"] == id && value.get("method").is_none() {
                if value.get("error").is_some() {
                    return Err(Error::agent("The CLI rejected the account usage request."));
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| Error::agent("Usage response is missing its result."));
            }
            if value.get("id").is_some() && value.get("method").is_some() {
                self.wire.send(json!({"id":value["id"],"error":{"code":-32601,"message":"Unsupported during usage probe"}})).await?;
            }
        }
        Err(Error::agent("Usage probe exceeded its safe output limit."))
    }
    async fn claude_control(&mut self, id: &str, request: Value) -> Result<Value> {
        self.wire
            .send(json!({"type":"control_request","request_id":id,"request":request}))
            .await?;
        let mut bytes = 0;
        for _ in 0..128 {
            let value = self.wire.read().await?;
            bytes += value.to_string().len();
            if bytes > 2 * 1024 * 1024 {
                break;
            }
            if value["type"] == "control_response" && value["response"]["request_id"] == id {
                if value["response"]["subtype"] != "success" {
                    return Err(Error::agent(
                        "Claude usage control is unavailable in this CLI version.",
                    ));
                }
                return Ok(value["response"]["response"].clone());
            }
            if value["type"] == "control_request" {
                self.wire.send(json!({"type":"control_response","response":{"subtype":"error","request_id":value["request_id"],"error":"Unsupported during usage probe"}})).await?;
            }
        }
        Err(Error::agent("Usage probe exceeded its safe output limit."))
    }
    async fn initialize(&mut self, provider: &AgentProviderId) -> Result<()> {
        if *provider == AgentProviderId::Codex {
            self.rpc("initialize", json!({"clientInfo":{"name":"sirus_usage","title":"Sirus Code","version":"0.1.0"},"capabilities":{"experimentalApi":true}})).await?;
            self.wire.send(json!({"method":"initialized"})).await?;
            // Fixed, read-only identity request. Never refresh or manage authentication.
            if let Ok(value) = self
                .rpc("account/read", json!({"refreshToken":false}))
                .await
            {
                self.account = cli_account(&value, provider);
            }
        } else {
            let value = self
                .claude_control(
                    "sy_usage_init",
                    json!({"subtype":"initialize","hooks":null}),
                )
                .await?;
            self.account = cli_account(&value, provider);
        }
        Ok(())
    }
    async fn limits(&mut self, provider: &AgentProviderId) -> Result<Value> {
        if *provider == AgentProviderId::Codex {
            self.rpc("account/rateLimits/read", json!({})).await
        } else {
            self.claude_control(
                "sy_usage_read",
                json!({"subtype":"get_usage","skip_behaviors":true}),
            )
            .await
        }
    }
    async fn snapshot(
        mut self,
        provider: &AgentProviderId,
    ) -> (Option<Value>, Option<UsageAccount>) {
        let raw = async {
            self.initialize(provider).await?;
            self.limits(provider).await
        }
        .await
        .ok();
        // Account metadata is confirmed independently of quota endpoint availability.
        let account = self.account.clone();
        self.close().await;
        (raw, account)
    }
    /// Ends the probe without waiting for its exit: the answer is already read, and the
    /// process may still be persisting a refreshed login.
    async fn close(self) {
        drop(self);
    }
}

fn overrides(state: &AppState) -> HashMap<AgentProviderId, String> {
    state.data.lock().settings.provider_paths.clone()
}
fn cache_result(
    state: &AppState,
    mut value: ProviderUsage,
    executable: Option<String>,
    raw: Option<&Value>,
    scope: &AccountScope,
) -> ProviderUsage {
    value.provider_account_id = scope.id.clone();
    if value.provider == AgentProviderId::Codex {
        let mut offer = state.usage.offer.lock();
        offer.remove(&scope.id);
        if let Some((credit_id, account_email)) = raw
            .and_then(|v| offered_credit(v, None))
            .zip(value.account.as_ref().and_then(|a| a.email.clone()))
        {
            let token = uuid::Uuid::new_v4().to_string();
            value.reset_offer = Some(token.clone());
            offer.insert(
                scope.id.clone(),
                ResetOffer {
                    token,
                    credit_id,
                    executable: executable.clone(),
                    issued: Instant::now(),
                    account_email,
                    account_id: scope.id.clone(),
                },
            );
        }
    }
    state.usage.cache.lock().insert(
        (value.provider.clone(), scope.id.clone()),
        CachedUsage {
            value: value.clone(),
            executable,
            fetched: Instant::now(),
            auth_stamp: None,
        },
    );
    value
}

/// Providers whose quota this module can read (CLI probes or the scoped HTTP reads).
pub fn reports_usage(provider: &AgentProviderId) -> bool {
    matches!(
        provider,
        AgentProviderId::Codex
            | AgentProviderId::Claude
            | AgentProviderId::Cursor
            | AgentProviderId::OpenCode
    )
}

pub async fn provider_usage(
    state: State<'_, Arc<AppState>>,
    provider: AgentProviderId,
    refresh: bool,
    account_id: Option<String>,
) -> Result<ProviderUsage> {
    state.ensure_running()?;
    let _operation = tokio::time::timeout(Duration::from_secs(20), state.usage.operation.lock())
        .await
        .map_err(|_| Error::agent("Usage is already being refreshed."))?;
    state.ensure_running()?;
    let account_id = account_id
        .unwrap_or_else(|| crate::provider_accounts::selected(&state.data.lock(), &provider));
    let scope = crate::provider_accounts::scope(&state, &provider, &account_id)?;
    let key = (provider.clone(), scope.id.clone());
    let profile_version = *state.usage.profile_versions.lock().get(&key).unwrap_or(&0);
    let paths = overrides(&state);
    let executable = paths.get(&provider).cloned();
    if matches!(
        provider,
        AgentProviderId::Cursor | AgentProviderId::OpenCode
    ) {
        use crate::provider_usage_http as http;
        let install = crate::detect::resolve_with_overrides(&provider, &paths)
            .ok_or_else(|| Error::agent("Unknown provider"))?;
        if !install.installed {
            return Ok(cache_result(
                &state,
                ProviderUsage::unavailable(provider, "Provider CLI is not installed."),
                executable,
                None,
                &scope,
            ));
        }
        let credential = match state
            .usage
            .run(8, async { Ok(http::load(&provider).await) })
            .await?
        {
            Ok(value) => value,
            Err(failure) => {
                return Ok(cache_result(
                    &state,
                    failure.usage(provider),
                    executable,
                    None,
                    &scope,
                ))
            }
        };
        if let Some(cached) = state.usage.cache.lock().get(&key) {
            if cached.executable == executable
                && cached.auth_stamp.as_ref() == Some(&credential.stamp)
                && cached.fetched.elapsed() < Duration::from_secs(if refresh { 2 } else { 60 })
            {
                return Ok(cached.value.clone());
            }
        }
        let usage = state
            .usage
            .run(12, async { Ok(http::fetch(&provider, &credential).await) })
            .await?;
        // Revalidate identity before publishing. Never join one account's profile with another's quota.
        let current = state
            .usage
            .run(8, async { Ok(http::load(&provider).await) })
            .await?;
        state.ensure_running()?;
        if overrides(&state).get(&provider) != executable.as_ref()
            || current.ok().is_none_or(|c| c.stamp != credential.stamp)
        {
            state.usage.cache.lock().remove(&key);
            return Err(Error::agent("CLI account changed; refresh usage again."));
        }
        let value = cache_result(&state, usage, executable, None, &scope);
        if let Some(cached) = state.usage.cache.lock().get_mut(&key) {
            cached.auth_stamp = Some(credential.stamp.clone());
        }
        return Ok(value);
    }
    if let Some(cached) = state.usage.cache.lock().get(&key) {
        if cached.executable == executable
            && cached.fetched.elapsed() < Duration::from_secs(if refresh { 2 } else { 60 })
        {
            return Ok(cached.value.clone());
        }
    }
    if !matches!(provider, AgentProviderId::Codex | AgentProviderId::Claude) {
        return Ok(cache_result(&state, ProviderUsage::unavailable(provider, "This adapter does not expose account quota through its CLI. Check the provider dashboard."), executable, None, &scope));
    }
    let install = crate::detect::resolve_with_overrides(&provider, &paths)
        .ok_or_else(|| Error::agent("Unknown provider"))?;
    if !install.installed {
        return Ok(cache_result(
            &state,
            ProviderUsage::unavailable(provider, "Provider CLI is not installed."),
            executable,
            None,
            &scope,
        ));
    }
    let result = state
        .usage
        .run(15, async {
            let probe = Probe::start(
                &install.path.unwrap_or(install.binary),
                &provider,
                scope.home.as_deref(),
            )
            .await?;
            Ok(probe.snapshot(&provider).await)
        })
        .await;
    state.ensure_running()?;
    if overrides(&state).get(&provider) != executable.as_ref() {
        return Err(Error::agent(
            "Provider executable changed; refresh usage again.",
        ));
    }
    let (raw, account) = result.ok().unwrap_or((None, None));
    let mut value = match raw.as_ref() {
        Some(value) if provider == AgentProviderId::Codex => parse_codex(value),
        Some(value) => parse_claude(value),
        None => {
            let mut value = ProviderUsage::unavailable(
                provider,
                "Could not read usage. Check the existing CLI login and version, then refresh.",
            );
            value.status = "error".into();
            value
        }
    };
    value.account = account;
    // Login can overlap an earlier read-only probe. Never cache its old-account result
    // after login invalidation, including when the browser flow already finished.
    state.usage.publish_profile(&key, profile_version, || {
        cache_result(&state, value, executable, raw.as_ref(), &scope)
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetResult {
    outcome: String,
    usage: ProviderUsage,
}

pub async fn consume_codex_reset(
    state: State<'_, Arc<AppState>>,
    offer: String,
    confirm: bool,
) -> Result<ResetResult> {
    state.ensure_running()?;
    if !confirm || offer.len() != 36 || uuid::Uuid::parse_str(&offer).is_err() {
        return Err(Error::agent(
            "Confirm the available Codex reset before using it.",
        ));
    }
    let _operation = tokio::time::timeout(Duration::from_secs(20), state.usage.operation.lock())
        .await
        .map_err(|_| Error::agent("Usage is already being refreshed."))?;
    let paths = overrides(&state);
    let executable = paths.get(&AgentProviderId::Codex).cloned();
    state.ensure_running()?;
    let grant = state.usage.take_offer(&offer, &executable, confirm)?;
    let scope =
        crate::provider_accounts::scope(&state, &AgentProviderId::Codex, &grant.account_id)?;
    if let Some(cached) = state
        .usage
        .cache
        .lock()
        .get_mut(&(AgentProviderId::Codex, scope.id.clone()))
    {
        cached.value.reset_offer = None;
    }
    let install = crate::detect::resolve_with_overrides(&AgentProviderId::Codex, &paths)
        .filter(|i| i.installed)
        .ok_or_else(|| Error::agent("Codex CLI is unavailable."))?;
    let mut known_outcome = None;
    let mut known_account = None;
    let result = state
        .usage
        .run(20, async {
            let mut probe = Probe::start(
                &install.path.unwrap_or(install.binary),
                &AgentProviderId::Codex,
                scope.home.as_deref(),
            )
            .await?;
            let result = async {
                probe.initialize(&AgentProviderId::Codex).await?;
                known_account = probe.account.clone();
                grant.validate_account(probe.account.as_ref())?;
                let fresh = probe.limits(&AgentProviderId::Codex).await?;
                if offered_credit(&fresh, Some(&grant.credit_id)).is_none() {
                    return Err(Error::agent(
                        "This Codex reset is no longer available. Refresh usage.",
                    ));
                }
                state.ensure_running()?;
                if overrides(&state).get(&AgentProviderId::Codex) != executable.as_ref() {
                    return Err(Error::agent("Provider executable changed. Refresh usage."));
                }
                let response = probe
                    .rpc(
                        "account/rateLimitResetCredit/consume",
                        json!({"creditId":grant.credit_id,"idempotencyKey":grant.token}),
                    )
                    .await?;
                let outcome = response["outcome"]
                    .as_str()
                    .filter(|o| {
                        ["reset", "nothingToReset", "noCredit", "alreadyRedeemed"].contains(o)
                    })
                    .ok_or_else(|| {
                        Error::agent(
                            "Codex reset result is unknown. Refresh usage before trying again.",
                        )
                    })?
                    .to_owned();
                known_outcome = Some(outcome.clone());
                let raw = probe.limits(&AgentProviderId::Codex).await?;
                Ok((outcome, raw))
            }
            .await;
            probe.close().await;
            result
        })
        .await;
    let (outcome, usage) =
        match result {
            Ok((outcome, raw)) => {
                let mut value = parse_codex(&raw);
                value.account = known_account;
                (
                    outcome,
                    cache_result(&state, value, executable, Some(&raw), &scope),
                )
            }
            Err(_) if known_outcome.is_some() => {
                let mut value = ProviderUsage::unavailable(
                    AgentProviderId::Codex,
                    "The CLI confirmed the reset request, but usage refresh failed. Refresh usage.",
                );
                value.status = "error".into();
                value.account = known_account;
                (
                    known_outcome.expect("confirmed outcome"),
                    cache_result(&state, value, executable, None, &scope),
                )
            }
            Err(_) => return Err(Error::agent(
                "Codex reset did not return a confirmed result. Refresh usage before trying again.",
            )),
        };
    Ok(ResetResult { outcome, usage })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[tokio::test]
    async fn usage_probe_supports_desktop_path() {
        use std::os::unix::fs::PermissionsExt;
        // Isolate the desktop environment in a child test process, never mutate
        // global PATH/HOME while other tests run or read real vendor credentials.
        if let Some(binary) = std::env::var_os("SIRUS_USAGE_PATH_FIXTURE") {
            let value = tokio::time::timeout(Duration::from_secs(15), async {
                let mut probe =
                    Probe::start(binary.to_str().unwrap(), &AgentProviderId::Codex, None)
                        .await
                        .unwrap();
                probe.initialize(&AgentProviderId::Codex).await.unwrap();
                let raw = probe.limits(&AgentProviderId::Codex).await.unwrap();
                probe.close().await;
                parse_codex(&raw)
            })
            .await
            .unwrap();
            assert_eq!(value.status, "available");
            assert_eq!(value.windows[0].used_percent, Some(17.0));
            return;
        }
        let repo = crate::git::tests::Repo::new();
        let bin = repo.0.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let runtime = bin.join("sirus-usage-runtime");
        std::fs::write(&runtime, "#!/bin/sh\nexec /usr/bin/python3 \"$@\"\n").unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        let binary = repo.0.join("codex-fixture");
        std::fs::write(&binary, r#"#!/usr/bin/env sirus-usage-runtime
import json,sys
for line in sys.stdin:
    v=json.loads(line)
    if 'id' not in v: continue
    method=v.get('method')
    if method=='account/read': result={'account':{'email':'fixture@example.invalid','planType':'fixture'}}
    elif method=='account/rateLimits/read': result={'rateLimits':{'primary':{'usedPercent':17}}}
    else: result={}
    print(json.dumps({'id':v['id'],'result':result}),flush=True)
"#).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider_usage::tests::usage_probe_supports_desktop_path",
                "--nocapture",
            ])
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &repo.0)
            .env("SIRUS_USAGE_PATH_FIXTURE", &binary)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Desktop usage probe failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn verified_account_survives_a_quota_only_protocol_failure() {
        use std::os::unix::fs::PermissionsExt;
        let repo = crate::git::tests::Repo::new();
        let binary = repo.0.join("quota-error-fixture");
        std::fs::write(&binary, r#"#!/usr/bin/env python3
import json,sys
for line in sys.stdin:
    v=json.loads(line)
    m=v.get('method')
    if m=='initialized': continue
    if m=='initialize': result={}
    elif m=='account/read':
        assert v['params']=={'refreshToken':False}
        result={'account':{'email':'fixture@example.test','planType':'pro'}}
    elif m=='account/rateLimits/read':
        print(json.dumps({'id':v['id'],'error':{'code':-32000,'message':'Quota unavailable'}}),flush=True)
        continue
    else: raise AssertionError(m)
    print(json.dumps({'id':v['id'],'result':result}),flush=True)
"#).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (raw, account) = Probe::start(binary.to_str().unwrap(), &AgentProviderId::Codex, None)
            .await
            .unwrap()
            .snapshot(&AgentProviderId::Codex)
            .await;
        assert!(raw.is_none());
        assert_eq!(
            account.unwrap().email.as_deref(),
            Some("fixture@example.test")
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn account_wire_denies_callbacks_and_refreshes_after_a_fixture_reset() {
        use std::os::unix::fs::PermissionsExt;
        let repo = crate::git::tests::Repo::new();
        let binary = repo.0.join("quota-fixture");
        std::fs::write(&binary, r#"#!/usr/bin/env python3
import json,sys
used=17
for line in sys.stdin:
    value=json.loads(line)
    if 'method' not in value:
        assert value.get('error',{}).get('code') == -32601
        continue
    method=value['method']
    if method == 'initialized': continue
    if method == 'initialize':
        print(json.dumps({'id':'foreign','method':'account/chatgptAuthTokens/refresh','params':{'reason':'unauthorized'}}),flush=True)
        result={}
    elif method == 'account/read':
        assert value['params'] == {'refreshToken':False}
        result={'account':{'type':'chatgpt','email':'fixture@example.test','planType':'pro'}}
    elif method == 'account/rateLimits/read':
        result={'rateLimits':{'primary':{'usedPercent':used}},'rateLimitResetCredits':{'availableCount':1 if used else 0,'credits':[{'id':'native-credit','status':'available','resetType':'codexRateLimits','expiresAt':None}] if used else []}}
    elif method == 'account/rateLimitResetCredit/consume':
        assert set(value['params']) == {'creditId','idempotencyKey'}
        assert value['params']['creditId'] == 'native-credit'
        assert len(value['params']['idempotencyKey']) == 36
        used=0
        result={'outcome':'reset'}
    else: raise Exception('Unexpected account operation')
    print(json.dumps({'id':value['id'],'result':result}),flush=True)
"#).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut probe = Probe::start(binary.to_str().unwrap(), &AgentProviderId::Codex, None)
                .await
                .unwrap();
            probe.initialize(&AgentProviderId::Codex).await.unwrap();
            let before = probe.limits(&AgentProviderId::Codex).await.unwrap();
            assert_eq!(
                probe.account.as_ref().and_then(|a| a.email.as_deref()),
                Some("fixture@example.test")
            );
            let credit = offered_credit(&before, None).unwrap();
            assert_eq!(parse_codex(&before).windows[0].used_percent, Some(17.0));
            let result = probe
                .rpc(
                    "account/rateLimitResetCredit/consume",
                    json!({"creditId":credit,"idempotencyKey":uuid::Uuid::new_v4().to_string()}),
                )
                .await
                .unwrap();
            assert_eq!(result["outcome"], "reset");
            let after = probe.limits(&AgentProviderId::Codex).await.unwrap();
            assert_eq!(parse_codex(&after).windows[0].used_percent, Some(0.0));
            assert!(offered_credit(&after, None).is_none());
            probe.close().await;
        })
        .await
        .unwrap();
    }
    #[test]
    fn reset_offer_is_confirmed_single_use_and_bound_to_executable_and_expiry() {
        let usage = UsageState::default();
        let token = uuid::Uuid::new_v4().to_string();
        let executable = Some("/owned/codex".into());
        usage.offer.lock().insert(
            "default".into(),
            ResetOffer {
                token: token.clone(),
                credit_id: "native-credit".into(),
                executable: executable.clone(),
                issued: Instant::now(),
                account_email: "fixture@example.test".into(),
                account_id: "default".into(),
            },
        );
        assert!(usage.take_offer(&token, &executable, false).is_err());
        assert!(usage
            .take_offer(&uuid::Uuid::new_v4().to_string(), &executable, true)
            .is_err());
        assert!(usage.take_offer(&token, &None, true).is_err());
        {
            let offer = usage.offer.lock();
            let offer = offer.get("default").unwrap();
            let mut account = UsageAccount {
                email: Some("fixture@example.test".into()),
                name: None,
                plan: None,
                key_fingerprint: None,
            };
            assert!(offer.validate_account(Some(&account)).is_ok());
            account.email = Some("another@example.test".into());
            assert!(offer.validate_account(Some(&account)).is_err());
            assert!(offer.validate_account(None).is_err());
        }
        assert_eq!(
            usage
                .take_offer(&token, &executable, true)
                .unwrap()
                .credit_id,
            "native-credit"
        );
        assert!(usage.take_offer(&token, &executable, true).is_err());
        usage.offer.lock().insert(
            "default".into(),
            ResetOffer {
                token: token.clone(),
                credit_id: "expired".into(),
                executable: executable.clone(),
                issued: Instant::now() - Duration::from_secs(601),
                account_email: "fixture@example.test".into(),
                account_id: "default".into(),
            },
        );
        assert!(usage.take_offer(&token, &executable, true).is_err());
    }
    #[tokio::test]
    async fn shutdown_cancels_active_usage_and_refuses_future_operations() {
        let usage = UsageState::default();
        let work = usage.run(15, async { std::future::pending::<Result<()>>().await });
        let cancel = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            usage.stop();
        };
        let (result, _) = tokio::join!(work, cancel);
        assert!(result.is_err());
        assert!(usage.run(15, async { Ok(()) }).await.is_err());
    }
    #[test]
    fn profile_invalidation_and_redemption_do_not_remove_another_accounts_offer() {
        let usage = UsageState::default();
        let executable = Some("/owned/codex".into());
        let mut tokens = HashMap::new();
        for account_id in ["default", "another"] {
            let token = uuid::Uuid::new_v4().to_string();
            tokens.insert(account_id, token.clone());
            usage.offer.lock().insert(
                account_id.into(),
                ResetOffer {
                    token,
                    credit_id: format!("{account_id}-credit"),
                    executable: executable.clone(),
                    issued: Instant::now(),
                    account_email: format!("{account_id}@example.test"),
                    account_id: account_id.into(),
                },
            );
        }
        usage.invalidate(&AgentProviderId::Claude, "default");
        assert_eq!(usage.offer.lock().len(), 2);
        usage.invalidate(&AgentProviderId::Codex, "default");
        assert!(usage
            .take_offer(&tokens["default"], &executable, true)
            .is_err());
        assert_eq!(
            usage
                .take_offer(&tokens["another"], &executable, true)
                .unwrap()
                .account_id,
            "another"
        );
        assert!(usage.offer.lock().is_empty());
    }
    #[test]
    fn a_probe_started_before_login_cannot_publish_after_login_finishes() {
        let usage = UsageState::default();
        let key = (AgentProviderId::Codex, "named".into());
        assert!(usage.publish_profile(&key, 0, || ()).is_ok());
        usage.invalidate(&key.0, &key.1); // Login starts.
        usage.invalidate(&key.0, &key.1); // Login finishes; no active login remains.
        let mut published = false;
        assert!(usage.publish_profile(&key, 0, || published = true).is_err());
        assert!(!published);
        assert!(usage
            .publish_profile(&(AgentProviderId::Codex, "default".into()), 0, || ())
            .is_ok());
        assert!(usage.publish_profile(&key, 2, || ()).is_ok());
    }
    #[test]
    fn codex_windows_are_real_bounded_and_multibucket() {
        let raw = json!({"account":"secret-not-exported","rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":17,"windowDurationMins":300,"resetsAt":1790000000},"secondary":{"usedPercent":101}},"other":{"primary":{"usedPercent":null}}},"rateLimitResetCredits":{"availableCount":2}});
        let parsed = parse_codex(&raw);
        assert_eq!(parsed.windows.len(), 3);
        assert_eq!(parsed.windows[0].used_percent, Some(17.0));
        assert_eq!(parsed.windows[0].resets_at, Some(1790000000000));
        assert_eq!(parsed.windows[1].used_percent, None);
        assert_eq!(parsed.reset_count, 2);
        assert!(parsed.reset_offer.is_none());
        assert!(!serde_json::to_string(&parsed)
            .unwrap()
            .contains("secret-not-exported"));
    }
    #[test]
    fn reset_requires_a_named_unexpired_native_credit() {
        let mut raw = json!({"rateLimitResetCredits":{"availableCount":1,"credits":[{"id":"credit","status":"available","resetType":"codexRateLimits","expiresAt":null}]}});
        assert_eq!(offered_credit(&raw, None).as_deref(), Some("credit"));
        assert!(offered_credit(&raw, Some("foreign")).is_none());
        raw["rateLimitResetCredits"]["credits"][0]["expiresAt"] = json!(1);
        assert!(offered_credit(&raw, None).is_none());
        assert!(offered_credit(
            &json!({"rateLimitResetCredits":{"availableCount":3,"credits":null}}),
            None
        )
        .is_none());
    }
    #[test]
    fn claude_percentage_is_not_a_ratio_and_missing_data_stays_unknown() {
        let value = parse_claude(
            &json!({"rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":0.5,"resets_at":"2026-10-01T12:00:00Z"},"seven_day":{"utilization":null}}}),
        );
        assert_eq!(value.windows[0].used_percent, Some(0.5));
        assert_eq!(value.windows[1].used_percent, None);
        assert!(value.windows[0].resets_at.is_some());
        assert!(parse_claude(&json!({})).windows.is_empty());
    }
    #[tokio::test]
    #[ignore = "initialize isolated empty vendor profiles only; no login, prompt or credential copy"]
    async fn empty_profiles_do_not_inherit_the_default_cli_account() {
        let root = crate::git::tests::Repo::new();
        for provider in [AgentProviderId::Codex, AgentProviderId::Claude] {
            let install =
                crate::detect::resolve_with_overrides(&provider, &HashMap::new()).unwrap();
            assert!(install.installed);
            let path = root.0.join(provider.key());
            std::fs::create_dir(&path).unwrap();
            let usage = UsageState::default();
            usage
                .run(20, async {
                    let mut probe = Probe::start(
                        &install.path.unwrap_or(install.binary),
                        &provider,
                        Some(&path),
                    )
                    .await?;
                    let result = probe.initialize(&provider).await;
                    assert!(probe
                        .account
                        .as_ref()
                        .and_then(|a| a.email.as_ref())
                        .is_none());
                    probe.close().await;
                    result
                })
                .await
                .unwrap();
            println!(
                "{}: empty named profile has no default account identity",
                provider.key()
            );
        }
    }
    #[tokio::test]
    #[ignore = "read-only installed CLIs; no prompt or reset consumption"]
    async fn live_usage_without_inference() {
        for provider in [AgentProviderId::Codex, AgentProviderId::Claude] {
            let install =
                crate::detect::resolve_with_overrides(&provider, &HashMap::new()).unwrap();
            let result = tokio::time::timeout(Duration::from_secs(15), async {
                let mut probe =
                    Probe::start(&install.path.unwrap_or(install.binary), &provider, None)
                        .await
                        .unwrap();
                probe.initialize(&provider).await.unwrap();
                assert!(
                    probe
                        .account
                        .as_ref()
                        .and_then(|a| a.email.as_ref())
                        .is_some(),
                    "CLI did not return its account identity"
                );
                let value = probe.limits(&provider).await.unwrap();
                probe.close().await;
                let parsed = if provider == AgentProviderId::Codex {
                    parse_codex(&value)
                } else {
                    parse_claude(&value)
                };
                assert_eq!(
                    parsed.status,
                    "available",
                    "{} did not return quota",
                    provider.key()
                );
                assert!(!parsed.windows.is_empty());
                println!(
                    "{}: {} real quota windows; no inference or reset",
                    provider.key(),
                    parsed.windows.len()
                );
            })
            .await;
            assert!(result.is_ok(), "{} usage probe timed out", provider.key());
        }
    }
}
