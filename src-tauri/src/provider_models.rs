use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::detect;
use crate::models::AgentProviderId;

const DISCOVER_TIMEOUT: Duration = Duration::from_secs(12);

/// A Claude catalog the CLI reported is reused this long before the CLI is asked again.
pub const CLAUDE_CATALOG_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Identifies the Claude executable a catalog was read from. Its `--version` is not read: the
/// install resolver does not know it, and reading it costs a spawn on every call. The path and
/// modified time stand in for it, since a CLI update rewrites the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableStamp {
    path: PathBuf,
    modified: SystemTime,
}

impl ExecutableStamp {
    /// One canonicalize and one stat. `None` leaves the catalog uncached.
    pub fn read(binary: &str) -> Option<Self> {
        let path = std::fs::canonicalize(binary).ok()?;
        let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
        Some(Self { path, modified })
    }
}

struct ClaudeCatalogEntry {
    stamp: ExecutableStamp,
    fetched_at: Instant,
    list: ProviderModelList,
}

/// Claude catalogs keyed by account. Only successful CLI reads are stored.
#[derive(Default)]
pub struct ClaudeCatalogCache {
    entries: HashMap<String, ClaudeCatalogEntry>,
}

impl ClaudeCatalogCache {
    /// The account's catalog, if it came from the same executable and is younger than the TTL.
    pub fn get(
        &self,
        account: &str,
        stamp: &ExecutableStamp,
        now: Instant,
    ) -> Option<ProviderModelList> {
        let entry = self.entries.get(account)?;
        (entry.stamp == *stamp
            && now.saturating_duration_since(entry.fetched_at) < CLAUDE_CATALOG_TTL)
            .then(|| entry.list.clone())
    }

    pub fn insert(
        &mut self,
        account: &str,
        stamp: ExecutableStamp,
        now: Instant,
        list: ProviderModelList,
    ) {
        self.entries.insert(
            account.to_owned(),
            ClaudeCatalogEntry {
                stamp,
                fetched_at: now,
                list,
            },
        );
    }
}

static CLAUDE_CATALOGS: parking_lot::Mutex<Option<ClaudeCatalogCache>> =
    parking_lot::Mutex::new(None);

/// Only a catalog the CLI reported with at least one model may be reused; fallbacks and empty
/// reads are failures and must be asked again.
fn cacheable(list: &ProviderModelList) -> bool {
    list.source == "cli" && !list.models.is_empty()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ModelAvailability {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: String,
    pub availability: ModelAvailability,
    #[serde(default)]
    pub effort_levels: Vec<String>,
    #[serde(default)]
    pub default_effort: Option<String>,
    #[serde(default)]
    pub fast_mode: bool,
    #[serde(default)]
    pub fast_unavailable_reason: Option<String>,
    #[serde(default)]
    pub parameterized: bool,
    #[serde(default)]
    pub default_fast: bool,
    #[serde(default, skip_serializing)]
    pub effort_parameter: Option<String>,
    #[serde(default, skip_serializing)]
    pub effort_values: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelList {
    pub provider: AgentProviderId,
    pub source: String,
    pub note: String,
    pub models: Vec<DiscoveredModel>,
}

fn offered_efforts(value: Option<&serde_json::Value>, objects: bool) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let effort = if objects {
                        row.get("effort")?.as_str()?
                    } else {
                        row.as_str()?
                    };
                    crate::execution::EFFORTS
                        .contains(&effort)
                        .then(|| effort.to_owned())
                })
                .take(8)
                .collect()
        })
        .unwrap_or_default()
}

/// `account` keys the Claude cache; `refresh` reads the CLI again instead of reusing the cache.
pub async fn list_for_provider(
    provider: AgentProviderId,
    overrides: &std::collections::HashMap<AgentProviderId, String>,
    account: &str,
    refresh: bool,
) -> ProviderModelList {
    let install = detect::resolve_with_overrides(&provider, overrides);
    let Some(install) = install else {
        return empty(provider, "none", "Provider is not registered.");
    };
    let binary = install
        .path
        .clone()
        .unwrap_or_else(|| install.binary.clone());

    if !install.installed {
        return match provider {
            AgentProviderId::Claude => claude_docs_catalog(false),
            _ => empty(
                provider,
                "none",
                "CLI not detected. Models cannot be listed until the executable is on PATH.",
            ),
        };
    }

    match provider {
        AgentProviderId::Codex => list_codex(&binary).await,
        AgentProviderId::Cursor => list_cursor(&binary).await,
        AgentProviderId::OpenCode => list_opencode(&binary).await,
        AgentProviderId::Claude => list_claude_cached(&binary, account, refresh).await,
        AgentProviderId::Grok => list_grok(&binary).await,
        AgentProviderId::Antigravity
        | AgentProviderId::Droid
        | AgentProviderId::Pi
        | AgentProviderId::Devin => list_additional(provider, &binary).await,
        AgentProviderId::Hermes => list_hermes(&binary).await,
    }
}

fn empty(provider: AgentProviderId, source: &str, note: &str) -> ProviderModelList {
    ProviderModelList {
        provider,
        source: source.into(),
        note: note.into(),
        models: Vec::new(),
    }
}

fn catalog_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c))
}

fn plain_model(id: &str, name: &str) -> DiscoveredModel {
    DiscoveredModel {
        id: id.into(),
        display_name: name.chars().take(200).collect(),
        availability: ModelAvailability::Available,
        effort_levels: Vec::new(),
        default_effort: None,
        fast_mode: false,
        fast_unavailable_reason: None,
        parameterized: false,
        default_fast: false,
        effort_parameter: None,
        effort_values: Default::default(),
    }
}

fn parse_pi_catalog(text: &str) -> Vec<DiscoveredModel> {
    text.lines()
        .filter_map(|line| {
            let cols: Vec<_> = line.split_whitespace().collect();
            // First-party CLI table: provider model context max-out thinking images.
            if cols.len() != 6
                || !["yes", "no"].contains(&cols[4])
                || !["yes", "no"].contains(&cols[5])
            {
                return None;
            }
            let id = format!("{}/{}", cols[0], cols[1]);
            catalog_id(&id).then(|| plain_model(&id, cols[1]))
        })
        .take(512)
        .collect()
}

fn parse_droid_catalog(text: &str) -> Vec<DiscoveredModel> {
    let Some((_, section)) = text.split_once("Available Models:") else {
        return Vec::new();
    };
    section
        .split("Model details:")
        .next()
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let index = line.find(char::is_whitespace)?;
            let (id, name) = line.split_at(index);
            catalog_id(id).then(|| plain_model(id, name.trim().trim_end_matches(" (default)")))
        })
        .take(512)
        .collect()
}

fn parse_agy_catalog(text: &str) -> Vec<DiscoveredModel> {
    text.lines()
        .filter_map(|line| {
            let line = line
                .trim()
                .trim_matches('│')
                .trim()
                .trim_start_matches(['*', '•'])
                .trim();
            let (id, name) = line
                .split_once(" - ")
                .or_else(|| line.split_once('│'))
                .or_else(|| line.split_once("  "))?;
            let id = id.trim();
            if !catalog_id(id)
                || ["id", "model", "models", "name", "slug"]
                    .contains(&id.to_ascii_lowercase().as_str())
            {
                return None;
            }
            Some(plain_model(id, name.trim()))
        })
        .take(512)
        .collect()
}

fn parse_devin_catalog(value: &serde_json::Value) -> Vec<DiscoveredModel> {
    fn visit(value: &serde_json::Value, rows: &mut Vec<DiscoveredModel>, depth: usize) {
        if depth > 6 || rows.len() >= 512 {
            return;
        }
        if let Some(items) = value.as_array() {
            for item in items.iter().take(512) {
                visit(item, rows, depth + 1);
            }
        } else if value.is_object() {
            // Traverse only documented catalog containers; credentials/metadata are ignored.
            let mut container = false;
            for key in ["models", "families", "model_families", "variants"] {
                if let Some(child) = value.get(key) {
                    container = true;
                    visit(child, rows, depth + 1);
                }
            }
            if container || value["disabled"] == true || value["available"] == false {
                return;
            }
            if let Some(id) = value["id"]
                .as_str()
                .or_else(|| value["slug"].as_str())
                .or_else(|| value["model_id"].as_str())
                .filter(|id| catalog_id(id))
            {
                if !rows.iter().any(|row| row.id == id) {
                    let name = value["display_name"]
                        .as_str()
                        .or_else(|| value["displayName"].as_str())
                        .or_else(|| value["name"].as_str())
                        .unwrap_or(id);
                    rows.push(plain_model(id, name));
                }
            }
        }
    }
    let mut rows = Vec::new();
    visit(value, &mut rows, 0);
    rows
}

async fn list_additional(provider: AgentProviderId, binary: &str) -> ProviderModelList {
    let args: &[&str] = match provider {
        AgentProviderId::Antigravity => &["models"],
        AgentProviderId::Droid => &["exec", "--help"],
        AgentProviderId::Pi => &[
            "--no-extensions",
            "--no-approve",
            "--offline",
            "--list-models",
        ],
        AgentProviderId::Devin => &["models", "list", "--format", "json"],
        _ => unreachable!(),
    };
    let mut command = detect::command(binary);
    command.args(args).current_dir(std::env::temp_dir());
    let output = crate::cli_output::capture_command(command, DISCOVER_TIMEOUT).await;
    let Ok(output) = output else {
        return empty(
            provider,
            "cli",
            "CLI catalog probe failed or timed out. Check the executable and sign in through its CLI, then refresh.",
        );
    };
    if !output.status.success() {
        return empty(
            provider,
            "cli",
            "CLI catalog is unavailable. Sign in through the provider CLI, then refresh. No login was started by Sirus.",
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let models = match provider {
        AgentProviderId::Antigravity => parse_agy_catalog(&text),
        AgentProviderId::Droid => parse_droid_catalog(&text),
        AgentProviderId::Pi => parse_pi_catalog(&text),
        AgentProviderId::Devin => serde_json::from_str(&text)
            .map(|value| parse_devin_catalog(&value))
            .unwrap_or_default(),
        _ => unreachable!(),
    };
    let note = if models.is_empty() {
        "No models were offered by this CLI. Sign in through the provider CLI and refresh; no models were invented."
    } else {
        "Models advertised by the installed CLI. Catalog presence does not verify authentication, quota or model access."
    };
    ProviderModelList {
        provider,
        source: "cli".into(),
        note: note.into(),
        models,
    }
}

fn claude_docs_catalog(cli_installed: bool) -> ProviderModelList {
    // Legacy CLIs may lack host initialization; keep documented aliases explicitly unknown.
    let availability = if cli_installed {
        ModelAvailability::Unknown
    } else {
        ModelAvailability::Unavailable
    };
    let models = ["sonnet", "opus", "haiku", "fable"]
        .into_iter()
        .map(|id| DiscoveredModel {
            id: id.into(),
            display_name: match id {
                "sonnet" => "Sonnet (latest alias)".into(),
                "opus" => "Opus (latest alias)".into(),
                "haiku" => "Haiku (latest alias)".into(),
                "fable" => "Fable (latest alias)".into(),
                _ => id.into(),
            },
            parameterized: false,
            default_fast: false,
            effort_parameter: None,
            effort_values: Default::default(),
            effort_levels: Vec::new(),
            default_effort: None,
            fast_unavailable_reason: None,
            fast_mode: false,
            availability: availability.clone(),
        })
        .collect();
    ProviderModelList {
        provider: AgentProviderId::Claude,
        source: "adapter-docs".into(),
        note: if cli_installed {
            "The CLI catalog could not be initialized. These are documented `--model` aliases; account access is unknown."
                .into()
        } else {
            "CLI not detected. Documented aliases are shown as unavailable.".into()
        },
        models,
    }
}

/// Claude's catalog costs a CLI spawn, so a successful read is reused per account for six hours
/// or until the executable changes. A refresh always reads again; a failed read is not stored.
async fn list_claude_cached(binary: &str, account: &str, refresh: bool) -> ProviderModelList {
    let stamp = ExecutableStamp::read(binary);
    if let (false, Some(stamp)) = (refresh, &stamp) {
        let cached = CLAUDE_CATALOGS
            .lock()
            .get_or_insert_with(Default::default)
            .get(account, stamp, Instant::now());
        if let Some(list) = cached {
            return list;
        }
    }
    let list = list_claude(binary).await;
    if let (true, Some(stamp)) = (cacheable(&list), stamp) {
        CLAUDE_CATALOGS
            .lock()
            .get_or_insert_with(Default::default)
            .insert(account, stamp, Instant::now(), list.clone());
    }
    list
}

async fn list_claude(binary: &str) -> ProviderModelList {
    // Safe mode keeps the user's authentication, but skips project/global hooks,
    // customizations and MCP servers. No user message or inference is sent.
    let request = br#"{"type":"control_request","request_id":"sirus_catalog","request":{"subtype":"initialize","hooks":null}}
"#;
    let result = crate::cli_output::request(
        binary,
        &[
            "-p",
            "--safe-mode",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-prompts",
            "none",
        ],
        request,
        DISCOVER_TIMEOUT,
        |value| {
            if value.get("type")?.as_str()? != "control_response"
                || value.pointer("/response/request_id")?.as_str()? != "sirus_catalog"
            {
                return None;
            }
            let response = value.pointer("/response/response")?;
            Some(serde_json::json!({"models":response.get("models")?, "fastRestriction": match response.get("fast_mode_disabled_reason").and_then(|v| v.as_str()) { Some("extra_usage_disabled") => "usage-credits", Some("org_disabled" | "disabled_by_org" | "disabled_by_settings") => "organization", Some(_) => "unavailable", None => "model" }, "fastAvailable":response.get("fast_mode_disabled_reason").is_some_and(|reason| reason.is_null()) && response.get("fast_mode_state").and_then(|state| state.as_str()).is_some_and(|state| ["on","off"].contains(&state))}))
        },
    )
    .await;
    match result.and_then(|models| parse_claude_models(&models).map_err(std::io::Error::other)) {
        Ok(models) => ProviderModelList { provider: AgentProviderId::Claude, source: "cli".into(),
            note: "From Claude Code host initialization (safe mode, no inference). Catalog presence does not verify account access.".into(), models },
        Err(_) => claude_docs_catalog(true),
    }
}

fn parse_claude_models(value: &serde_json::Value) -> Result<Vec<DiscoveredModel>, String> {
    let rows = value
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Claude initialization did not return models[]".to_string())?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let id = row.get("value")?.as_str()?;
            if id.is_empty() || id.len() > 200 || id.chars().any(char::is_control) {
                return None;
            }
            Some(DiscoveredModel {
                id: id.into(),
                display_name: row
                    .get("displayName")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(id)
                    .chars()
                    .take(200)
                    .collect(),
                parameterized: false,
                default_fast: false,
                effort_parameter: None,
                effort_values: Default::default(),
                effort_levels: offered_efforts(row.get("supportedEffortLevels"), false),
                default_effort: None,
                fast_unavailable_reason: Some(
                    if row["supportsFastMode"] != true {
                        "model"
                    } else {
                        value["fastRestriction"].as_str().unwrap_or("unavailable")
                    }
                    .to_owned(),
                ),
                fast_mode: row.get("supportsFastMode").and_then(|v| v.as_bool()) == Some(true)
                    && value.get("fastAvailable").and_then(|v| v.as_bool()) == Some(true),
                availability: ModelAvailability::Available,
            })
        })
        .collect())
}

async fn list_grok(binary: &str) -> ProviderModelList {
    match run(binary, &["--no-auto-update", "models"]).await {
        Ok(stdout) => {
            let mut models = parse_grok_list(&stdout);
            let effort_flag = run(binary, &["--no-auto-update", "--help"])
                .await
                .is_ok_and(|help| help.contains("--effort"));
            if effort_flag {
                for row in &mut models {
                    row.effort_levels = grok_effort_levels(&row.id)
                        .iter()
                        .map(|level| (*level).to_owned())
                        .collect();
                    if !row.effort_levels.is_empty() {
                        row.default_effort = Some("high".into());
                    }
                }
            }
            let note = if stdout.contains("You are not authenticated.") {
                "From `grok models`. The CLI reports no authentication; sign in using Grok's own CLI to select these models."
            } else {
                "From `grok models`. Catalog presence does not verify account access."
            };
            ProviderModelList {
                provider: AgentProviderId::Grok,
                source: "cli".into(),
                note: note.into(),
                models,
            }
        }
        Err(err) => empty(AgentProviderId::Grok, "cli", &err),
    }
}

/// Effort ladder by Grok generation: 4.6 and newer offer Extra High, 4.5
/// stops at High. A versioned id keeps its generation's ladder whatever follows the version
/// (`grok-4.7-build-fast` is Grok 4.7's fast tier). The retired grok-build line (4.3 and
/// older, `grok-build`, `grok-code-fast`, unversioned aliases) takes no `--effort`.
fn grok_effort_levels(id: &str) -> &'static [&'static str] {
    let id = id.trim().to_ascii_lowercase();
    let version = id.find("grok-").and_then(|start| {
        let rest = &id[start + 5..];
        let (major, rest) = rest.split_once('.')?;
        let minor: String = rest.chars().take_while(char::is_ascii_digit).collect();
        Some((major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?))
    });
    match version {
        Some((major, minor)) if major > 4 || (major == 4 && minor >= 6) => {
            &["low", "medium", "high", "xhigh"]
        }
        Some((4, 5)) => &["low", "medium", "high"],
        _ => &[],
    }
}

fn parse_grok_list(stdout: &str) -> Vec<DiscoveredModel> {
    let availability = if stdout.contains("You are not authenticated.") {
        ModelAvailability::Unavailable
    } else {
        ModelAvailability::Available
    };
    let Some((_, rows)) = stdout.split_once("Available models:") else {
        return Vec::new();
    };
    rows.lines()
        .filter_map(|line| {
            let line = line.trim();
            let row = line
                .strip_prefix("* ")
                .or_else(|| line.strip_prefix("- "))?;
            let id = row.strip_suffix(" (default)").unwrap_or(row);
            if id.is_empty()
                || id.len() > 200
                || id.chars().any(char::is_whitespace)
                || !id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_.:/-".contains(c))
            {
                return None;
            }
            Some(DiscoveredModel {
                id: id.into(),
                display_name: id.into(),
                parameterized: false,
                default_fast: false,
                effort_parameter: None,
                effort_values: Default::default(),
                effort_levels: Vec::new(),
                default_effort: None,
                fast_unavailable_reason: None,
                fast_mode: false,
                availability: availability.clone(),
            })
        })
        .collect()
}

async fn list_codex(binary: &str) -> ProviderModelList {
    match run(binary, &["debug", "models"]).await {
        Ok(stdout) => match parse_codex_json(&stdout) {
            Ok(models) => ProviderModelList {
                provider: AgentProviderId::Codex,
                source: "cli".into(),
                note: "From `codex debug models`.".into(),
                models,
            },
            Err(err) => empty(AgentProviderId::Codex, "cli", &err),
        },
        Err(err) => empty(AgentProviderId::Codex, "cli", &err),
    }
}

fn parse_codex_json(stdout: &str) -> Result<Vec<DiscoveredModel>, String> {
    let value: serde_json::Value = serde_json::from_str(stdout)
        .map_err(|err| format!("codex debug models is not JSON: {err}"))?;
    let list = value
        .get("models")
        .and_then(|item| item.as_array())
        .ok_or_else(|| "codex debug models JSON missing models[]".to_string())?;
    Ok(list
        .iter()
        .filter_map(|item| {
            if item.get("visibility").and_then(|v| v.as_str()) == Some("hide") {
                return None;
            }
            let id = item.get("slug")?.as_str()?.to_string();
            let display_name = item
                .get("display_name")
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            Some(DiscoveredModel {
                id,
                display_name,
                parameterized: false,
                default_fast: false,
                effort_parameter: None,
                effort_values: Default::default(),
                effort_levels: offered_efforts(item.get("supported_reasoning_levels"), true),
                default_effort: item
                    .get("default_reasoning_level")
                    .and_then(|v| v.as_str())
                    .filter(|v| crate::execution::EFFORTS.contains(v))
                    .map(str::to_owned),
                fast_unavailable_reason: None,
                fast_mode: item
                    .get("service_tiers")
                    .and_then(|v| v.as_array())
                    .is_some_and(|tiers| tiers.iter().any(|tier| tier["id"] == "priority")),
                availability: ModelAvailability::Available,
            })
        })
        .collect())
}

/// Hermes lists its models only inside a session: a probe session in the temp folder, no prompt.
async fn list_hermes(binary: &str) -> ProviderModelList {
    let cwd = serde_json::to_string(&std::env::temp_dir().to_string_lossy())
        .unwrap_or_else(|_| "\"/tmp\"".into());
    let input = format!(
        "{}\n{}\n",
        r#"{"jsonrpc":"2.0","id":"sirus_init","method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"Sirus","version":"0.1.0"}}}"#,
        format_args!(
            r#"{{"jsonrpc":"2.0","id":"sirus_session","method":"session/new","params":{{"cwd":{cwd},"mcpServers":[]}}}}"#
        )
    );
    let result =
        crate::cli_output::request(binary, &["acp"], input.as_bytes(), DISCOVER_TIMEOUT, |v| {
            (v["id"] == "sirus_session").then(|| v.clone())
        })
        .await;
    let Ok(reply) = result else {
        return empty(
            AgentProviderId::Hermes,
            "cli",
            "Hermes did not answer. Check `hermes acp --check`, then refresh.",
        );
    };
    if reply.get("error").is_some() {
        return empty(
            AgentProviderId::Hermes,
            "cli",
            "Hermes has no model provider yet. Run `hermes model` in a terminal, then refresh. No login was started by Sirus.",
        );
    }
    let models = parse_hermes_catalog(&reply["result"]);
    ProviderModelList {
        provider: AgentProviderId::Hermes,
        source: "cli".into(),
        note: if models.is_empty() { "Hermes offered no models. Run `hermes model`, then refresh; no models were invented." } else { "Models offered by Hermes for its configured provider. Presence does not verify quota or access." }.into(),
        models,
    }
}

fn parse_hermes_catalog(result: &serde_json::Value) -> Vec<DiscoveredModel> {
    let mut rows: Vec<DiscoveredModel> = Vec::new();
    for item in result["models"]["availableModels"]
        .as_array()
        .into_iter()
        .flatten()
        .take(512)
    {
        let Some(id) = item["modelId"]
            .as_str()
            .or_else(|| item["id"].as_str())
            .filter(|id| catalog_id(id))
        else {
            continue;
        };
        if rows.iter().any(|row| row.id == id) {
            continue;
        }
        rows.push(plain_model(id, item["name"].as_str().unwrap_or(id)));
    }
    rows
}

async fn list_cursor(binary: &str) -> ProviderModelList {
    // Read-only ACP extension: no session, prompt, auth mutation or parameter writes.
    let input = br#"{"jsonrpc":"2.0","id":"sirus_init","method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"Sirus","version":"0.1.0"}}}
{"jsonrpc":"2.0","id":"sirus_models","method":"cursor/list_available_models","params":{}}
"#;
    let metadata = crate::cli_output::request(binary, &["acp"], input, DISCOVER_TIMEOUT, |v| {
        (v["id"] == "sirus_models")
            .then(|| v.get("result").cloned().unwrap_or(serde_json::Value::Null))
    })
    .await
    .ok()
    .and_then(|value| parse_cursor_parameters(&value).ok());
    let legacy = run(binary, &["--list-models"])
        .await
        .ok()
        .map(|stdout| parse_cursor_list(&stdout))
        .unwrap_or_default();
    let parameter_catalog_available = metadata.is_some();
    let mut models = metadata.unwrap_or_default();
    for row in legacy {
        if !models.iter().any(|existing| existing.id == row.id) {
            models.push(row);
        }
    }
    ProviderModelList { provider: AgentProviderId::Cursor, source: "cli".into(),
        note: if parameter_catalog_available { "From Cursor's read-only ACP model parameter catalog and legacy CLI presets. No inference or session is created." } else { "From legacy Cursor CLI presets. Parameter metadata is unavailable on this CLI; only offered exact presets can be selected." }.into(), models }
}

fn normalized_effort(value: &str) -> Option<&str> {
    let value = if value == "extra-high" {
        "xhigh"
    } else {
        value
    };
    crate::execution::EFFORTS.contains(&value).then_some(value)
}
fn parse_cursor_parameters(value: &serde_json::Value) -> Result<Vec<DiscoveredModel>, String> {
    let rows = value["models"]
        .as_array()
        .ok_or("Cursor did not return model metadata")?;
    Ok(rows
        .iter()
        .take(512)
        .filter_map(|row| {
            let id = row["value"].as_str()?;
            if id.is_empty()
                || id.len() > 200
                || !id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            {
                return None;
            }
            let options = row["configOptions"].as_array();
            let effort = options.and_then(|options| {
                options.iter().find(|o| {
                    o["type"] == "select"
                        && ["effort", "reasoning", "reasoning_effort"]
                            .iter()
                            .any(|id| o["id"] == *id)
                })
            });
            let effort_values: std::collections::HashMap<String, String> = effort
                .and_then(|o| o["options"].as_array())
                .into_iter()
                .flatten()
                .take(16)
                .filter_map(|v| {
                    let raw = v["value"].as_str()?;
                    Some((normalized_effort(raw)?.into(), raw.into()))
                })
                .collect();
            let fast = options.and_then(|options| {
                options
                    .iter()
                    .find(|o| o["id"] == "fast" && o["type"] == "select")
            });
            let fast_mode = fast
                .and_then(|o| o["options"].as_array())
                .is_some_and(|values| {
                    ["true", "false"]
                        .iter()
                        .all(|b| values.iter().any(|v| v["value"] == *b))
                });
            Some(DiscoveredModel {
                id: id.into(),
                display_name: row["name"]
                    .as_str()
                    .unwrap_or(id)
                    .chars()
                    .take(200)
                    .collect(),
                availability: ModelAvailability::Available,
                effort_levels: crate::execution::EFFORTS
                    .iter()
                    .filter(|level| effort_values.contains_key(**level))
                    .map(|level| (*level).into())
                    .collect(),
                default_effort: effort
                    .and_then(|o| o["currentValue"].as_str())
                    .and_then(normalized_effort)
                    .map(str::to_owned),
                effort_parameter: effort.and_then(|o| o["id"].as_str()).map(str::to_owned),
                effort_values,
                fast_unavailable_reason: None,
                fast_mode,
                default_fast: fast_mode && fast.is_some_and(|o| o["currentValue"] == "true"),
                parameterized: true,
            })
        })
        .collect())
}

fn parse_cursor_list(stdout: &str) -> Vec<DiscoveredModel> {
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with("Available models") || line.starts_with("Tip:") {
                return None;
            }
            let (id, name) = line.split_once(" - ")?;
            let id = id.trim();
            if id.is_empty() || id.contains(' ') {
                return None;
            }
            Some(DiscoveredModel {
                id: id.to_string(),
                display_name: name.trim().to_string(),
                parameterized: false,
                default_fast: false,
                effort_parameter: None,
                effort_values: Default::default(),
                effort_levels: Vec::new(),
                default_effort: None,
                fast_unavailable_reason: None,
                fast_mode: false,
                availability: ModelAvailability::Available,
            })
        })
        .collect()
}

async fn list_opencode(binary: &str) -> ProviderModelList {
    match run(binary, &["models", "--verbose"]).await {
        Ok(stdout) => ProviderModelList { provider: AgentProviderId::OpenCode, source: "cli".into(),
            note: "From `opencode models --verbose`; reasoning variants are offered by the installed CLI. Catalog presence does not verify quota.".into(), models: parse_opencode_verbose(&stdout) },
        Err(_) => match run(binary, &["models"]).await {
            Ok(stdout) => ProviderModelList { provider: AgentProviderId::OpenCode, source:"cli".into(), note:"Legacy model catalog without execution metadata.".into(), models:parse_opencode_list(&stdout) },
            Err(err) => empty(AgentProviderId::OpenCode,"cli",&err),
        }
    }
}
fn parse_opencode_verbose(stdout: &str) -> Vec<DiscoveredModel> {
    let mut rest = stdout;
    let mut models = Vec::new();
    while let Some((line, next)) = rest.split_once('\n') {
        rest = next.trim_start();
        let Some(mut model) = parse_opencode_list(line).into_iter().next() else {
            continue;
        };
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<serde_json::Value>();
        let Some(Ok(value)) = stream.next() else {
            continue;
        };
        rest = &rest[stream.byte_offset()..];
        model.display_name = value["name"]
            .as_str()
            .unwrap_or(&model.id)
            .chars()
            .take(200)
            .collect();
        if let Some(variants) = value["variants"].as_object() {
            model.effort_levels = crate::execution::EFFORTS
                .iter()
                .filter(|level| {
                    variants
                        .get(**level)
                        .is_some_and(|v| v["disabled"] != true && v.is_object())
                })
                .map(|level| (*level).into())
                .collect();
        }
        models.push(model);
        if models.len() >= 512 {
            break;
        }
    }
    models
}

fn parse_opencode_list(stdout: &str) -> Vec<DiscoveredModel> {
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (provider, model) = line.split_once('/')?;
            if provider.is_empty()
                || model.is_empty()
                || model.starts_with('/')
                || line.chars().any(char::is_whitespace)
                || !provider
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
            {
                return None;
            }
            Some(DiscoveredModel {
                id: line.to_string(),
                display_name: line.to_string(),
                parameterized: false,
                default_fast: false,
                effort_parameter: None,
                effort_values: Default::default(),
                effort_levels: Vec::new(),
                default_effort: None,
                fast_unavailable_reason: None,
                fast_mode: false,
                availability: ModelAvailability::Available,
            })
        })
        .collect()
}

async fn run(binary: &str, args: &[&str]) -> Result<String, String> {
    let output = crate::cli_output::capture(binary, args, DISCOVER_TIMEOUT)
        .await
        .map_err(|err| format!("CLI catalog could not be read: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(stderr
            .lines()
            .next()
            .unwrap_or("command failed")
            .to_string());
    }
    Ok(if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn additional_catalogs_only_admit_explicit_model_rows() {
        let pi = parse_pi_catalog(
            "provider model context max-out thinking images\nopenai gpt-6-sol 400K 64K yes yes\nNo models available. Use /login\n",
        );
        assert_eq!(pi.len(), 1);
        assert_eq!(pi[0].id, "openai/gpt-6-sol");
        assert!(pi[0].effort_levels.is_empty());
        let droid = parse_droid_catalog(
            "Usage: droid exec\nAvailable Models:\n  claude-sonnet-5    Sonnet 5\n  gpt-6-sol         GPT-6 Sol (default)\nModel details:\n  unrelated metadata\n",
        );
        assert_eq!(droid.len(), 2);
        assert_eq!(droid[1].display_name, "GPT-6 Sol");
        assert!(parse_droid_catalog("No model catalog\n").is_empty());
        let agy = parse_agy_catalog(
            "ID  Name\ngemini-3.8-flash  Gemini 3.8 Flash\nFetching available models...\n",
        );
        assert_eq!(agy.len(), 1);
        assert_eq!(agy[0].id, "gemini-3.8-flash");
        let devin = parse_devin_catalog(
            &serde_json::json!({"families":[{"name":"OpenAI","models":[{"id":"gpt-6-sol","display_name":"GPT-6 Sol"},{"id":"--unsafe"},{"id":"denied","available":false}]}],"credentials":{"id":"private"}}),
        );
        assert_eq!(devin.len(), 1);
        assert_eq!(devin[0].id, "gpt-6-sol");
    }
    #[test]
    fn cursor_parameter_catalog_retains_exact_known_controls_and_rejects_unsafe_ids() {
        let value = serde_json::json!({"models":[{"value":"composer-2.5","name":"Composer 2.5","configOptions":[{"id":"fast","type":"select","currentValue":"true","options":[{"value":"true"},{"value":"false"}]}]}, {"value":"gpt-5.5","name":"GPT-5.5","configOptions":[{"id":"reasoning","type":"select","currentValue":"extra-high","options":[{"value":"low"},{"value":"extra-high"},{"value":"xhigh,fast=true"}]}]}, {"value":"evil[fast=true]","configOptions":[]}]});
        let rows = parse_cursor_parameters(&value).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].parameterized && rows[0].fast_mode && rows[0].default_fast);
        assert!(rows[0].effort_levels.is_empty());
        assert_eq!(rows[1].effort_levels, vec!["low", "xhigh"]);
        assert_eq!(rows[1].default_effort.as_deref(), Some("xhigh"));
        assert_eq!(rows[1].effort_values.get("xhigh").unwrap(), "extra-high");
        assert!(serde_json::to_value(&rows[1])
            .unwrap()
            .get("effortValues")
            .is_none());
    }
    #[test]
    fn opencode_verbose_catalog_uses_actual_variants_and_never_returns_provider_config() {
        let rows = parse_opencode_verbose(
            "provider/model\n{\"name\":\"Model\",\"variants\":{\"low\":{},\"high\":{},\"max\":{\"disabled\":true},\"arbitrary\":{}},\"headers\":{\"Authorization\":\"not-for-ui\"}}\nprovider/second\n{\"name\":\"Second\",\"variants\":{}}\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].effort_levels, vec!["low", "high"]);
        assert!(!rows[0].fast_mode);
        assert!(rows[1].effort_levels.is_empty());
        assert!(!serde_json::to_string(&rows).unwrap().contains("not-for-ui"));
    }
    #[tokio::test]
    #[ignore = "installed Cursor catalog only; no session or inference"]
    async fn live_cursor_parameter_catalog() {
        let install =
            detect::resolve_with_overrides(&AgentProviderId::Cursor, &Default::default()).unwrap();
        let list = list_cursor(install.path.as_deref().unwrap()).await;
        let composer = list
            .models
            .iter()
            .find(|row| row.id == "composer-2.5")
            .unwrap();
        assert!(composer.parameterized && composer.fast_mode);
        assert!(composer.effort_levels.is_empty());
        assert!(list
            .models
            .iter()
            .any(|row| row.parameterized && !row.effort_levels.is_empty()));
    }
    #[test]
    fn grok_effort_follows_the_version_not_the_suffix() {
        let xhigh: &[&str] = &["low", "medium", "high", "xhigh"];
        assert_eq!(grok_effort_levels("grok-4.7"), xhigh);
        assert_eq!(grok_effort_levels("grok-4.6"), xhigh);
        assert_eq!(grok_effort_levels("grok-4.7-build-fast"), xhigh);
        assert_eq!(grok_effort_levels("Grok-4.10-fast"), xhigh);
        assert_eq!(grok_effort_levels("grok-5.0"), xhigh);
        assert_eq!(grok_effort_levels("grok-4.5"), ["low", "medium", "high"]);
        assert_eq!(
            grok_effort_levels("grok-4.5-fast"),
            ["low", "medium", "high"]
        );
        for retired in [
            "grok-build",
            "grok-4.3",
            "grok-4.3-build",
            "grok-4",
            "grok-code-fast-1",
            "custom/grok-fast",
        ] {
            assert!(grok_effort_levels(retired).is_empty(), "{retired}");
        }
    }

    #[test]
    fn grok_catalog_does_not_claim_authentication_and_ignores_non_rows() {
        let models = parse_grok_list(
            "You are not authenticated.\nDefault model: grok-4.6\nAvailable models:\n * grok-4.6 (default)\n - grok-4.5\n - invalid row\n",
        );
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "grok-4.6");
        assert_eq!(models[0].availability, ModelAvailability::Unavailable);
        assert!(parse_grok_list(" - not-a-catalog").is_empty());
    }
    #[test]
    fn claude_catalog_uses_cli_selectable_ids_and_ignores_account_metadata() {
        let value = serde_json::json!({"models":[{"value":"opus","displayName":"Opus","resolvedModel":"claude-opus-wire"}],"account":{"email":"not-for-ui"}});
        let models = parse_claude_models(&value).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "opus");
        assert_eq!(models[0].display_name, "Opus");
        assert_eq!(models[0].availability, ModelAvailability::Available);
        assert!(parse_claude_models(&serde_json::json!({"account":{}})).is_err());
    }
    #[test]
    fn execution_capabilities_require_explicit_native_metadata() {
        let models=parse_codex_json(r#"{"models":[{"slug":"m","supported_reasoning_levels":[{"effort":"high"},{"effort":"injected"}],"default_reasoning_level":"high","service_tiers":[{"id":"priority"}]}]}"#).unwrap();
        assert_eq!(models[0].effort_levels, vec!["high"]);
        assert!(models[0].fast_mode);
        let payload = serde_json::json!({"models":[{"value":"opus","supportsFastMode":true,"supportedEffortLevels":["low","max","unknown"]}],"fastAvailable":false});
        let unavailable = parse_claude_models(&payload).unwrap();
        assert!(!unavailable[0].fast_mode);
        assert_eq!(unavailable[0].effort_levels, vec!["low", "max"]);
        let mut available = payload;
        available["fastAvailable"] = serde_json::json!(true);
        assert!(parse_claude_models(&available).unwrap()[0].fast_mode);
    }
    #[tokio::test]
    #[ignore = "real CLI initialization, no inference; requires installed Claude"]
    async fn live_claude_catalog() {
        let install =
            detect::resolve_with_overrides(&AgentProviderId::Claude, &Default::default()).unwrap();
        assert!(install.installed);
        let list = list_claude(install.path.as_deref().unwrap()).await;
        assert_eq!(list.source, "cli");
        assert!(!list.models.is_empty());
        assert!(list
            .models
            .iter()
            .all(|model| model.availability == ModelAvailability::Available));
    }
    #[test]
    fn codex_catalog_rejects_bad_shape_and_omits_hidden_models() {
        assert!(parse_codex_json(r#"{"anything":[]}"#).is_err());
        let models = parse_codex_json(r#"{"models":[{"slug":"visible","display_name":"Visible"},{"slug":"internal","visibility":"hide"}]}"#).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "visible");
    }
    #[test]
    fn cursor_and_opencode_parse_cli_rows_without_inventing_models() {
        let cursor = parse_cursor_list(
            "Available models\n\nauto - Auto (default)\nclaude-opus - Claude Opus\nTip: try - something\n",
        );
        assert_eq!(cursor.len(), 2);
        assert_eq!(cursor[1].id, "claude-opus");
        let models = parse_opencode_list(
            "provider/model\nnot a model\nWarning: /home/config missing\nhttps://example.com\n{\"id\":\"other/model\"}\n",
        );
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "provider/model");
    }

    fn cli_catalog() -> ProviderModelList {
        let models = parse_claude_models(
            &serde_json::json!({"models":[{"value":"opus","displayName":"Opus"}]}),
        )
        .unwrap();
        ProviderModelList {
            provider: AgentProviderId::Claude,
            source: "cli".into(),
            note: String::new(),
            models,
        }
    }

    fn stamp(path: &str, secs: u64) -> ExecutableStamp {
        ExecutableStamp {
            path: PathBuf::from(path),
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
        }
    }

    #[test]
    fn a_claude_catalog_is_reused_within_six_hours_for_the_same_account() {
        let base = Instant::now();
        let mut cache = ClaudeCatalogCache::default();
        cache.insert("default", stamp("/bin/claude", 1), base, cli_catalog());
        let hit = cache.get(
            "default",
            &stamp("/bin/claude", 1),
            base + Duration::from_secs(3600),
        );
        assert_eq!(hit.map(|list| list.models.len()), Some(1));
        assert!(cache.get("other", &stamp("/bin/claude", 1), base).is_none());
    }

    #[test]
    fn a_claude_catalog_expires_after_six_hours() {
        let base = Instant::now();
        let mut cache = ClaudeCatalogCache::default();
        cache.insert("default", stamp("/bin/claude", 1), base, cli_catalog());
        let just_before = base + CLAUDE_CATALOG_TTL - Duration::from_secs(1);
        assert!(cache
            .get("default", &stamp("/bin/claude", 1), just_before)
            .is_some());
        let at_six_hours = base + CLAUDE_CATALOG_TTL;
        assert!(cache
            .get("default", &stamp("/bin/claude", 1), at_six_hours)
            .is_none());
    }

    #[test]
    fn a_changed_claude_cli_invalidates_its_catalog() {
        let base = Instant::now();
        let mut cache = ClaudeCatalogCache::default();
        cache.insert("default", stamp("/bin/claude", 1), base, cli_catalog());
        // A CLI update rewrites the binary: the modified time moves.
        assert!(cache
            .get("default", &stamp("/bin/claude", 2), base)
            .is_none());
        // A different executable path is a different CLI.
        assert!(cache
            .get("default", &stamp("/opt/claude", 1), base)
            .is_none());
    }

    #[test]
    fn failed_or_empty_claude_reads_are_not_cacheable() {
        let fallback = claude_docs_catalog(true);
        assert!(!cacheable(&fallback));
        let mut empty = cli_catalog();
        empty.models.clear();
        assert!(!cacheable(&empty));
        assert!(cacheable(&cli_catalog()));
    }
}
