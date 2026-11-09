//! Provider CLI version checks (read-only npm registry reads) and explicit, user-confirmed
//! updates. How a binary was installed comes from its canonical path (`InstallMethod`), and each
//! method has one fixed argv; nothing runs unless the person asks (ADR-020). A background pass
//! checks every six hours and announces changes as the `provider-updates` event (ADR-198).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::runtime::{AppHandle, Emitter, Manager, State};
use parking_lot::Mutex;
use serde::Serialize;

use crate::commands::AppState;
use crate::detect;
use crate::error::{Error, Result};
use crate::models::{AgentInstall, AgentProviderId, ProviderUpdate, ProviderUpdateResult};
use crate::persist;

/// Provider, the npm package whose registry `latest` is the newest version (None: no package),
/// and the argv that updates a native install (None: no native update). A provider without a row
/// gets no check and no update, so adding a provider means adding a row.
type ProviderRow = (
    AgentProviderId,
    Option<&'static str>,
    Option<&'static [&'static str]>,
);

const PROVIDERS: &[ProviderRow] = &[
    (
        AgentProviderId::Codex,
        Some("@openai/codex"),
        Some(&["update"]),
    ),
    (
        AgentProviderId::Claude,
        Some("@anthropic-ai/claude-code"),
        Some(&["update"]),
    ),
    (
        AgentProviderId::OpenCode,
        Some("opencode-ai"),
        Some(&["upgrade", "--method", "curl"]),
    ),
    (
        AgentProviderId::Pi,
        Some("@earendil-works/pi-coding-agent"),
        Some(&["update", "--self"]),
    ),
    (AgentProviderId::Grok, None, Some(&["update"])),
];

/// Where a provider's own installer puts its binary, relative to the home folder. A trailing `/`
/// marks a folder; anything else is the binary itself.
const NATIVE_ROOTS: &[(AgentProviderId, &[&str])] = &[
    (
        AgentProviderId::Claude,
        &[
            ".local/bin/claude",
            ".local/share/claude/versions/",
            ".claude/local/",
        ],
    ),
    (AgentProviderId::OpenCode, &[".opencode/bin/"]),
];

/// Also the period of the background pass.
const CHECK_TTL: Duration = Duration::from_secs(6 * 60 * 60);
/// The background pass waits this long after start-up before its first check.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_REGISTRY_BYTES: usize = 256 * 1024;
const UPDATE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_MESSAGE_CHARS: usize = 240;
const MAX_VERSION_CHARS: usize = 64;

static CACHE: Mutex<Option<(Instant, Vec<ProviderUpdate>)>> = Mutex::new(None);
/// The available set last sent as `provider-updates`; a check sends only when it changes.
static ANNOUNCED: Mutex<Vec<AvailableUpdate>> = Mutex::new(Vec::new());

fn provider_row(provider: &AgentProviderId) -> Option<&'static ProviderRow> {
    PROVIDERS
        .iter()
        .find(|(candidate, _, _)| candidate == provider)
}

fn npm_package(provider: &AgentProviderId) -> Option<&'static str> {
    provider_row(provider).and_then(|(_, package, _)| *package)
}

fn native_update_args(provider: &AgentProviderId) -> Option<&'static [&'static str]> {
    provider_row(provider).and_then(|(_, _, args)| *args)
}

/// How a CLI got onto this Mac, from the canonical path of the binary it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallMethod {
    /// npm's global tree; `prefix` is the folder whose `lib/node_modules` holds the package.
    Npm {
        prefix: Option<PathBuf>,
    },
    /// Bun's global bin, or the global tree its links point into.
    Bun,
    Homebrew {
        name: String,
        cask: bool,
    },
    /// The provider's own installer (Claude's `~/.local`, OpenCode's `~/.opencode`).
    Native,
    Unknown,
}

impl InstallMethod {
    fn label(&self) -> &'static str {
        match self {
            Self::Npm { .. } => "npm",
            Self::Bun => "bun",
            Self::Homebrew { .. } => "homebrew",
            Self::Native => "native",
            Self::Unknown => "unknown",
        }
    }
}

/// Classifies a canonical binary path: npm, then Bun, Homebrew, the provider's native folders,
/// and anything else is unknown. Pure; the caller canonicalizes the path and reads `HOME`.
pub fn install_method(
    provider: &AgentProviderId,
    canonical: &str,
    home: Option<&str>,
) -> InstallMethod {
    if canonical.contains("/lib/node_modules/") {
        return InstallMethod::Npm {
            prefix: npm_prefix(canonical),
        };
    }
    // Bun links its global bins from `~/.bun/bin` into `~/.bun/install/global`, so a resolved
    // Bun install usually sits under the second folder.
    if canonical.contains("/.bun/bin/") || canonical.contains("/.bun/install/") {
        return InstallMethod::Bun;
    }
    if let Some((name, cask)) = homebrew_name(canonical) {
        return InstallMethod::Homebrew { name, cask };
    }
    if is_native(provider, canonical, home) {
        return InstallMethod::Native;
    }
    InstallMethod::Unknown
}

/// The folder above `lib/node_modules` that holds the package: `/opt/homebrew` for
/// `/opt/homebrew/lib/node_modules/...`.
fn npm_prefix(canonical: &str) -> Option<PathBuf> {
    let (prefix, _) = canonical.split_once("/lib/node_modules/")?;
    (!prefix.is_empty()).then(|| PathBuf::from(prefix))
}

/// The formula (or cask) name in a `Cellar/<name>/` or `Caskroom/<name>/` path.
fn homebrew_name(canonical: &str) -> Option<(String, bool)> {
    let parts: Vec<&str> = canonical.split('/').collect();
    parts.windows(3).find_map(|window| {
        let cask = match window[0] {
            "Cellar" => false,
            "Caskroom" => true,
            _ => return None,
        };
        (!window[1].is_empty()).then(|| (window[1].to_string(), cask))
    })
}

fn is_native(provider: &AgentProviderId, canonical: &str, home: Option<&str>) -> bool {
    let Some(home) = home else {
        return false;
    };
    let home = home.trim_end_matches('/');
    NATIVE_ROOTS
        .iter()
        .filter(|(candidate, _)| candidate == provider)
        .flat_map(|(_, roots)| roots.iter().copied())
        .map(|root| format!("{home}/{}", root.trim_end_matches('/')))
        .any(|root| canonical == root || canonical.starts_with(&format!("{root}/")))
}

/// The install method of one detected installation, from its binary's canonical path.
fn method_of(install: &AgentInstall) -> InstallMethod {
    let Some(path) = install.path.as_deref().filter(|_| install.installed) else {
        return InstallMethod::Unknown;
    };
    let canonical = std::fs::canonicalize(path)
        .map(|resolved| resolved.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());
    let home = std::env::var("HOME").ok();
    install_method(&install.id, &canonical, home.as_deref())
}

/// The npm that owns an npm-installed binary: `<prefix>/bin/npm` for the prefix whose
/// `lib/node_modules` holds the package, else npm beside the binary's link, else npm on the
/// CLI search path. Another npm (for example Homebrew's beside a `~/.local` install) would
/// install the update into its own prefix and leave this binary as it was.
fn npm_for(prefix: Option<&Path>, path: &str) -> Option<PathBuf> {
    let executable = |candidate: PathBuf| candidate.is_file().then_some(candidate);
    prefix
        .and_then(|prefix| executable(prefix.join("bin").join("npm")))
        .or_else(|| {
            Path::new(path)
                .parent()
                .and_then(|dir| executable(dir.join("npm")))
        })
        .or_else(|| tool_on_path("npm"))
}

/// A tool on the CLI search path, the same one the provider CLIs run with.
fn tool_on_path(name: &str) -> Option<PathBuf> {
    which::which_in(name, Some(detect::cli_path()), "/").ok()
}

/// The fixed argv of an update, program first. None when this installation has no one-click
/// update; its notice says how to update it instead.
fn update_argv(
    method: &InstallMethod,
    package: Option<&str>,
    native_args: Option<&[&str]>,
    binary: &str,
) -> Option<Vec<String>> {
    let argv = match method {
        InstallMethod::Npm { prefix } => {
            let mut argv = vec![
                "npm".to_string(),
                "install".into(),
                "-g".into(),
                "--no-fund".into(),
                "--no-audit".into(),
            ];
            // The tree the binary lives in, not the prefix of whichever npm runs (Homebrew's npm
            // would otherwise update `/opt/homebrew` and leave a `~/.local` install as it was).
            if let Some(prefix) = prefix {
                argv.push("--prefix".into());
                argv.push(prefix.to_string_lossy().into_owned());
            }
            argv.push(format!("{}@latest", package?));
            argv
        }
        InstallMethod::Bun => vec![
            "bun".to_string(),
            "add".into(),
            "-g".into(),
            format!("{}@latest", package?),
        ],
        InstallMethod::Homebrew { name, cask } => {
            let mut argv = vec!["brew".to_string(), "upgrade".into()];
            if *cask {
                argv.push("--cask".into());
            }
            argv.push(name.clone());
            argv
        }
        InstallMethod::Native => {
            let mut argv = vec![binary.to_string()];
            argv.extend(native_args?.iter().map(|arg| arg.to_string()));
            argv
        }
        InstallMethod::Unknown => return None,
    };
    Some(argv)
}

/// What an update runs: the detected binary, how it was installed, and the fixed argv.
struct UpdatePlan {
    binary: String,
    method: InstallMethod,
    argv: Vec<String>,
}

fn update_plan(install: &AgentInstall, method: &InstallMethod) -> Option<UpdatePlan> {
    let binary = install.path.clone().filter(|_| install.installed)?;
    let argv = update_argv(
        method,
        npm_package(&install.id),
        native_update_args(&install.id),
        &binary,
    )?;
    Some(UpdatePlan {
        binary,
        method: method.clone(),
        argv,
    })
}

/// The executable an update's argv starts: npm of the binary's own prefix, bun or brew on the
/// search path, or the binary itself for a native install.
fn update_program(plan: &UpdatePlan) -> Option<PathBuf> {
    match &plan.method {
        InstallMethod::Npm { prefix } => npm_for(prefix.as_deref(), &plan.binary),
        InstallMethod::Bun => tool_on_path("bun"),
        InstallMethod::Homebrew { .. } => tool_on_path("brew"),
        InstallMethod::Native => Some(PathBuf::from(&plan.binary)),
        InstallMethod::Unknown => None,
    }
}

/// The notice for an update Sirus cannot run itself.
fn manual_notice(latest: &str, path: &str) -> String {
    format!("A versão {latest} saiu. O Sirus roda {path}; atualize pela ferramenta que o instalou.")
}

/// First numeric version token in CLI output, e.g. "codex-cli 0.42.0" -> 0.42.0.
fn version_parts(text: &str) -> Option<Vec<u64>> {
    let start = text.find(|character: char| character.is_ascii_digit())?;
    let rest = &text[start..];
    let end = rest
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(rest.len());
    let parts: Vec<u64> = rest[..end]
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok())
        .collect();
    (!parts.is_empty()).then_some(parts)
}

fn newer(latest: &[u64], installed: &[u64]) -> bool {
    let length = latest.len().max(installed.len());
    for index in 0..length {
        let left = latest.get(index).copied().unwrap_or(0);
        let right = installed.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

/// `failed` when the command failed; `updated` when `--version` now reads another version;
/// otherwise `unchanged`.
fn run_status(ok: bool, before: Option<&str>, after: Option<&str>) -> &'static str {
    if !ok {
        "failed"
    } else if after.is_some() && after != before {
        "updated"
    } else {
        "unchanged"
    }
}

fn registry_client() -> Result<&'static reqwest::Client> {
    static HTTP: OnceLock<std::result::Result<reqwest::Client, ()>> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REGISTRY_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent("sirus")
            .build()
            .map_err(|_| ())
    })
    .as_ref()
    .map_err(|_| Error::new("native", "Cannot start the provider update check."))
}

async fn fetch_latest(package: &str) -> Result<Option<String>> {
    let mut response = registry_client()?
        .get(format!("https://registry.npmjs.org/{package}/latest"))
        .send()
        .await
        .map_err(|_| Error::new("native", "Cannot reach the provider registry."))?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::new("native", "Cannot read the provider registry response."))?
    {
        if body.len() + chunk.len() > MAX_REGISTRY_BYTES {
            return Err(Error::new(
                "native",
                "Provider registry response exceeds the safe limit.",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| Error::new("native", "Provider registry response is not valid JSON."))?;
    Ok(value
        .get("version")
        .and_then(|version| version.as_str())
        .map(str::to_string))
}

fn last_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string();
    line.chars().take(MAX_MESSAGE_CHARS).collect()
}

/// One check of every detected provider. An installed provider gets its version read and, when
/// it has a package, the registry's latest; a provider that is not installed is not looked up.
async fn collect(overrides: HashMap<AgentProviderId, String>) -> Result<Vec<ProviderUpdate>> {
    let installs = crate::runtime::async_runtime::spawn_blocking(move || {
        detect::detect_with_overrides(&overrides)
    })
    .await
    .map_err(|error| Error::new("native", error.to_string()))?;
    let mut updates = Vec::new();
    for install in installs {
        let method = method_of(&install);
        let installed_version = match &install.path {
            Some(path) if install.installed => detect::probe_version(path).await,
            _ => None,
        };
        let latest_version = match npm_package(&install.id) {
            Some(package) if install.installed => fetch_latest(package).await.unwrap_or(None),
            _ => None,
        };
        let update_available = install.installed
            && installed_version
                .as_deref()
                .and_then(version_parts)
                .zip(latest_version.as_deref().and_then(version_parts))
                .is_some_and(|(installed, latest)| newer(&latest, &installed));
        let plan = update_plan(&install, &method);
        let notice = (update_available && plan.is_none()).then(|| {
            manual_notice(
                latest_version.as_deref().unwrap_or_default(),
                install.path.as_deref().unwrap_or_default(),
            )
        });
        updates.push(ProviderUpdate {
            provider: install.id,
            installed: install.installed,
            installed_version,
            latest_version,
            update_available,
            update_supported: plan.is_some(),
            method: method.label().to_string(),
            update_command: plan.map(|plan| plan.argv.join(" ")),
            notice,
        });
    }
    Ok(updates)
}

/// Runs a check now, caches it and announces the available updates.
async fn check_now(app: &AppHandle, state: &AppState) -> Result<Vec<ProviderUpdate>> {
    let (enabled, overrides) = {
        let data = state.data.lock();
        (
            data.settings.enable_provider_update_checks,
            data.settings.provider_paths.clone(),
        )
    };
    if !enabled {
        announce(app, &[]);
        return Ok(Vec::new());
    }
    let updates = collect(overrides).await?;
    *CACHE.lock() = Some((Instant::now(), updates.clone()));
    let dismissed = state.data.lock().provider_update_dismissals.clone();
    announce(app, &available_updates(&updates, &dismissed));
    Ok(updates)
}

/// The updates to announce: the available ones whose latest version the person has not dismissed.
fn available_updates(
    updates: &[ProviderUpdate],
    dismissed: &HashMap<AgentProviderId, String>,
) -> Vec<AvailableUpdate> {
    updates
        .iter()
        .filter(|update| update.update_available)
        .filter(|update| {
            dismissed.get(&update.provider).map(String::as_str) != update.latest_version.as_deref()
        })
        .map(|update| AvailableUpdate {
            provider: update.provider.clone(),
            installed_version: update.installed_version.clone(),
            latest_version: update.latest_version.clone(),
            method: update.method.clone(),
            update_supported: update.update_supported,
            update_command: update.update_command.clone(),
            notice: update.notice.clone(),
        })
        .collect()
}

/// Sends `provider-updates` when the available set differs from the last one sent.
fn announce(app: &AppHandle, updates: &[AvailableUpdate]) {
    {
        let mut announced = ANNOUNCED.lock();
        if announced.as_slice() == updates {
            return;
        }
        *announced = updates.to_vec();
    }
    let _ = app.emit(
        "provider-updates",
        ProviderUpdatesEvent::Available {
            updates: updates.to_vec(),
        },
    );
}

/// One entry of the available set, as the window shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AvailableUpdate {
    provider: AgentProviderId,
    installed_version: Option<String>,
    latest_version: Option<String>,
    method: String,
    update_supported: bool,
    update_command: Option<String>,
    notice: Option<String>,
}

/// Payloads of the `provider-updates` event: the available set, or one update run's result.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum ProviderUpdatesEvent {
    Available { updates: Vec<AvailableUpdate> },
    Result { run: ProviderUpdateResult },
}

/// Starts the background pass: 30 s after start-up, then every six hours while Sirus runs.
pub fn start(app: AppHandle) {
    crate::runtime::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if state.ensure_running().is_err() {
                return;
            }
            if let Err(error) = check_now(&app, &state).await {
                tracing::debug!(%error, "provider update check skipped");
            }
            tokio::time::sleep(CHECK_TTL).await;
        }
    });
}

pub async fn provider_updates(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    refresh: bool,
) -> Result<Vec<ProviderUpdate>> {
    if !state.data.lock().settings.enable_provider_update_checks {
        return Ok(Vec::new());
    }
    if !refresh {
        if let Some((checked_at, cached)) = CACHE.lock().clone() {
            if checked_at.elapsed() < CHECK_TTL {
                return Ok(cached);
            }
        }
    }
    check_now(&app, &state).await
}

/// Stops announcing one version of a provider's update and keeps that choice across restarts.
pub fn dismiss_provider_update(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    provider: AgentProviderId,
    version: String,
) -> Result<()> {
    let version = version.trim().to_string();
    if version.is_empty() || version.chars().count() > MAX_VERSION_CHARS {
        return Err(Error::new("invalid", "Provider version is not valid."));
    }
    let dismissed = {
        let mut data = state.data.lock();
        state.ensure_running()?;
        let previous = data
            .provider_update_dismissals
            .insert(provider.clone(), version);
        if let Err(error) = persist::save(&state.data_path, &data) {
            match previous {
                Some(earlier) => data.provider_update_dismissals.insert(provider, earlier),
                None => data.provider_update_dismissals.remove(&provider),
            };
            return Err(error);
        }
        data.provider_update_dismissals.clone()
    };
    let updates = CACHE
        .lock()
        .as_ref()
        .map(|(_, updates)| updates.clone())
        .unwrap_or_default();
    announce(&app, &available_updates(&updates, &dismissed));
    Ok(())
}

pub async fn update_providers(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    providers: Vec<AgentProviderId>,
    confirm: bool,
) -> Result<Vec<ProviderUpdateResult>> {
    if !confirm {
        return Err(Error::new(
            "confirmation_required",
            "Provider updates require confirmation.",
        ));
    }
    let overrides = state.data.lock().settings.provider_paths.clone();
    let installs = crate::runtime::async_runtime::spawn_blocking(move || {
        detect::detect_with_overrides(&overrides)
    })
    .await
    .map_err(|error| Error::new("native", error.to_string()))?;
    let mut selected: Vec<(AgentProviderId, UpdatePlan)> = Vec::new();
    for provider in providers {
        let Some(install) = installs.iter().find(|install| install.id == provider) else {
            return Err(Error::new("invalid", "Unknown provider."));
        };
        let Some(plan) = update_plan(install, &method_of(install)) else {
            return Err(Error::new(
                "invalid",
                "This installation has no supported one-click update.",
            ));
        };
        if !selected.iter().any(|(candidate, _)| *candidate == provider) {
            selected.push((provider, plan));
        }
    }
    if selected.is_empty() {
        return Err(Error::new("invalid", "No providers selected."));
    }
    let mut results = Vec::new();
    for (provider, plan) in selected {
        let before = detect::probe_version(&plan.binary).await;
        let run = match update_program(&plan) {
            Some(program) => {
                // The CLI search path, so npm's `#!/usr/bin/env node` finds node when the
                // app was opened from Finder.
                let mut command = detect::command(program);
                command.args(&plan.argv[1..]);
                crate::cli_output::capture_command(command, UPDATE_TIMEOUT).await
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is not available on PATH.", plan.argv[0]),
            )),
        };
        let (ok, message) = match run {
            Ok(output) if output.status.success() => (true, String::new()),
            Ok(output) => {
                // Some CLIs report their failure on stdout.
                let line = last_line(&output.stderr);
                (
                    false,
                    if line.is_empty() {
                        last_line(&output.stdout)
                    } else {
                        line
                    },
                )
            }
            Err(error) => (false, error.to_string()),
        };
        let installed_version = detect::probe_version(&plan.binary).await;
        let result = ProviderUpdateResult {
            provider,
            status: run_status(ok, before.as_deref(), installed_version.as_deref()).into(),
            message,
            installed_version,
        };
        let _ = app.emit(
            "provider-updates",
            ProviderUpdatesEvent::Result {
                run: result.clone(),
            },
        );
        results.push(result);
    }
    *CACHE.lock() = None;
    // The installed versions moved, so the available set may have too.
    let _ = check_now(&app, &state).await;
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn method(provider: AgentProviderId, canonical: &str) -> InstallMethod {
        install_method(&provider, canonical, Some("/Users/me"))
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    fn update(
        provider: AgentProviderId,
        latest: Option<&str>,
        available: bool,
        notice: Option<&str>,
    ) -> ProviderUpdate {
        ProviderUpdate {
            provider,
            installed: true,
            installed_version: Some("0.42.0".into()),
            latest_version: latest.map(str::to_string),
            update_available: available,
            update_supported: notice.is_none(),
            method: "npm".into(),
            update_command: notice.is_none().then(|| "npm install -g x@latest".into()),
            notice: notice.map(str::to_string),
        }
    }

    #[test]
    fn npm_is_checked_first_and_names_its_prefix() {
        assert_eq!(
            method(
                AgentProviderId::Codex,
                "/opt/homebrew/lib/node_modules/@openai/codex/bin/codex.js"
            ),
            InstallMethod::Npm {
                prefix: Some("/opt/homebrew".into())
            }
        );
        // npm's own tree under a Homebrew Cellar is still an npm install.
        assert_eq!(
            method(
                AgentProviderId::Codex,
                "/opt/homebrew/Cellar/node/22.18.0/lib/node_modules/npm/bin/npm-cli.js"
            ),
            InstallMethod::Npm {
                prefix: Some("/opt/homebrew/Cellar/node/22.18.0".into())
            }
        );
        assert_eq!(
            method(AgentProviderId::Codex, "/lib/node_modules/codex.js"),
            InstallMethod::Npm { prefix: None }
        );
    }

    #[test]
    fn bun_global_bin_and_its_tree_are_bun() {
        assert_eq!(
            method(AgentProviderId::Codex, "/Users/me/.bun/bin/codex"),
            InstallMethod::Bun
        );
        assert_eq!(
            method(
                AgentProviderId::Claude,
                "/Users/me/.bun/install/global/node_modules/@anthropic-ai/claude-code/cli.js"
            ),
            InstallMethod::Bun
        );
    }

    #[test]
    fn homebrew_cellar_and_caskroom_give_the_name() {
        assert_eq!(
            method(
                AgentProviderId::Codex,
                "/opt/homebrew/Cellar/codex/0.42.0/bin/codex"
            ),
            InstallMethod::Homebrew {
                name: "codex".into(),
                cask: false
            }
        );
        assert_eq!(
            method(
                AgentProviderId::Claude,
                "/opt/homebrew/Caskroom/claude-code/2.1.0/claude"
            ),
            InstallMethod::Homebrew {
                name: "claude-code".into(),
                cask: true
            }
        );
        assert_eq!(
            method(AgentProviderId::Codex, "/opt/homebrew/Cellar"),
            InstallMethod::Unknown
        );
    }

    #[test]
    fn native_folders_belong_to_their_provider() {
        assert_eq!(
            method(
                AgentProviderId::Claude,
                "/Users/me/.local/share/claude/versions/2.1.319"
            ),
            InstallMethod::Native
        );
        assert_eq!(
            method(AgentProviderId::Claude, "/Users/me/.local/bin/claude"),
            InstallMethod::Native
        );
        assert_eq!(
            method(AgentProviderId::Claude, "/Users/me/.claude/local/claude"),
            InstallMethod::Native
        );
        assert_eq!(
            method(
                AgentProviderId::OpenCode,
                "/Users/me/.opencode/bin/opencode"
            ),
            InstallMethod::Native
        );
        // The same folder means nothing to another provider, and a name that only starts the
        // same is not inside the folder.
        assert_eq!(
            method(AgentProviderId::Claude, "/Users/me/.opencode/bin/claude"),
            InstallMethod::Unknown
        );
        assert_eq!(
            method(AgentProviderId::Claude, "/Users/me/.local/bin/claude-old"),
            InstallMethod::Unknown
        );
        assert_eq!(
            install_method(
                &AgentProviderId::Claude,
                "/Users/me/.local/share/claude/versions/2.1.319",
                None
            ),
            InstallMethod::Unknown
        );
    }

    #[test]
    fn anywhere_else_is_unknown() {
        assert_eq!(
            method(AgentProviderId::Claude, "/usr/local/bin/claude"),
            InstallMethod::Unknown
        );
        assert_eq!(
            method(AgentProviderId::Codex, "/Users/me/tools/codex"),
            InstallMethod::Unknown
        );
    }

    #[test]
    fn the_table_gives_packages_and_native_arguments_and_nothing_else() {
        assert_eq!(
            npm_package(&AgentProviderId::Claude),
            Some("@anthropic-ai/claude-code")
        );
        assert_eq!(
            native_update_args(&AgentProviderId::OpenCode),
            Some(&["upgrade", "--method", "curl"][..])
        );
        assert_eq!(
            npm_package(&AgentProviderId::Pi),
            Some("@earendil-works/pi-coding-agent")
        );
        assert_eq!(
            native_update_args(&AgentProviderId::Pi),
            Some(&["update", "--self"][..])
        );
        assert_eq!(npm_package(&AgentProviderId::Grok), None);
        assert_eq!(
            native_update_args(&AgentProviderId::Grok),
            Some(&["update"][..])
        );
        // A provider without a row gets neither a package nor a native command.
        assert_eq!(npm_package(&AgentProviderId::Cursor), None);
        assert_eq!(native_update_args(&AgentProviderId::Cursor), None);
        assert_eq!(npm_package(&AgentProviderId::Hermes), None);
        assert_eq!(native_update_args(&AgentProviderId::Hermes), None);
    }

    #[test]
    fn each_method_has_one_fixed_argv() {
        let npm = InstallMethod::Npm { prefix: None };
        assert_eq!(
            update_argv(&npm, Some("@openai/codex"), None, "/x/codex"),
            Some(argv(&[
                "npm",
                "install",
                "-g",
                "--no-fund",
                "--no-audit",
                "@openai/codex@latest"
            ]))
        );
        assert_eq!(update_argv(&npm, None, None, "/x/codex"), None);
        let local = InstallMethod::Npm {
            prefix: Some(PathBuf::from("/Users/me/.local")),
        };
        assert_eq!(
            update_argv(&local, Some("pkg"), None, "/x/pi"),
            Some(argv(&[
                "npm",
                "install",
                "-g",
                "--no-fund",
                "--no-audit",
                "--prefix",
                "/Users/me/.local",
                "pkg@latest"
            ]))
        );
        assert_eq!(
            update_argv(&InstallMethod::Bun, Some("@openai/codex"), None, "/x"),
            Some(argv(&["bun", "add", "-g", "@openai/codex@latest"]))
        );
        assert_eq!(
            update_argv(
                &InstallMethod::Homebrew {
                    name: "codex".into(),
                    cask: false
                },
                None,
                None,
                "/x"
            ),
            Some(argv(&["brew", "upgrade", "codex"]))
        );
        assert_eq!(
            update_argv(
                &InstallMethod::Homebrew {
                    name: "claude-code".into(),
                    cask: true
                },
                None,
                None,
                "/x"
            ),
            Some(argv(&["brew", "upgrade", "--cask", "claude-code"]))
        );
        assert_eq!(
            update_argv(
                &InstallMethod::Native,
                None,
                Some(&["update"]),
                "/Users/me/.local/bin/claude"
            ),
            Some(argv(&["/Users/me/.local/bin/claude", "update"]))
        );
        assert_eq!(
            update_argv(&InstallMethod::Native, None, None, "/x/claude"),
            None
        );
        // Unknown never runs anything, whatever the provider's table says.
        assert_eq!(
            update_argv(
                &InstallMethod::Unknown,
                Some("@openai/codex"),
                Some(&["update"]),
                "/x"
            ),
            None
        );
    }

    #[test]
    fn a_run_is_updated_unchanged_or_failed() {
        assert_eq!(
            run_status(true, Some("codex-cli 0.42.0"), Some("codex-cli 0.43.0")),
            "updated"
        );
        assert_eq!(
            run_status(true, Some("codex-cli 0.43.0"), Some("codex-cli 0.43.0")),
            "unchanged"
        );
        assert_eq!(
            run_status(true, Some("codex-cli 0.43.0"), None),
            "unchanged"
        );
        assert_eq!(run_status(false, Some("0.42.0"), Some("0.43.0")), "failed");
    }

    #[test]
    fn a_dismissed_version_is_hidden_and_a_newer_one_is_not() {
        let updates = vec![
            update(AgentProviderId::Codex, Some("0.43.0"), true, None),
            update(
                AgentProviderId::Claude,
                Some("2.1.4"),
                true,
                Some(
                    "A versão 2.1.4 saiu. O Sirus roda /usr/local/bin/claude; atualize pela ferramenta que o instalou.",
                ),
            ),
            update(AgentProviderId::OpenCode, Some("1.0.0"), false, None),
        ];
        let mut dismissed = HashMap::new();
        assert_eq!(available_updates(&updates, &dismissed).len(), 2);
        dismissed.insert(AgentProviderId::Codex, "0.43.0".to_string());
        let shown = available_updates(&updates, &dismissed);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].provider, AgentProviderId::Claude);
        assert!(shown[0].notice.is_some());
        assert!(!shown[0].update_supported);
        dismissed.insert(AgentProviderId::Codex, "0.42.5".to_string());
        assert_eq!(available_updates(&updates, &dismissed).len(), 2);
    }

    #[test]
    fn a_manual_notice_names_the_version_and_the_binary() {
        assert_eq!(
            manual_notice("0.50.0", "/usr/local/bin/codex"),
            "A versão 0.50.0 saiu. O Sirus roda /usr/local/bin/codex; atualize pela ferramenta que o instalou."
        );
    }

    #[test]
    fn dismissals_are_saved_with_the_app_data() {
        let mut data = crate::models::AppData::default();
        data.provider_update_dismissals
            .insert(AgentProviderId::Claude, "2.1.319".into());
        let value = serde_json::to_value(&data).unwrap();
        let loaded: crate::models::AppData = serde_json::from_value(value).unwrap();
        assert_eq!(
            loaded
                .provider_update_dismissals
                .get(&AgentProviderId::Claude)
                .map(String::as_str),
            Some("2.1.319")
        );
        // A state file written before the field existed loads with nothing dismissed.
        let mut older = serde_json::to_value(crate::models::AppData::default()).unwrap();
        older
            .as_object_mut()
            .unwrap()
            .remove("providerUpdateDismissals");
        let loaded: crate::models::AppData = serde_json::from_value(older).unwrap();
        assert!(loaded.provider_update_dismissals.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn npm_comes_from_the_prefix_that_installed_the_binary() {
        let root = std::env::temp_dir().join(format!("sirus-npm-{}", uuid::Uuid::new_v4()));
        let package = root.join("lib/node_modules/@openai/codex/bin");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(package.join("codex.js"), "").unwrap();
        std::fs::write(root.join("bin/npm"), "").unwrap();
        std::os::unix::fs::symlink(package.join("codex.js"), root.join("bin/codex")).unwrap();
        let link = root.join("bin/codex");
        let canonical = link.canonicalize().unwrap();
        let InstallMethod::Npm { prefix } =
            install_method(&AgentProviderId::Codex, &canonical.to_string_lossy(), None)
        else {
            panic!("an npm-managed binary is classified as npm");
        };
        let npm = npm_for(prefix.as_deref(), &link.to_string_lossy()).unwrap();
        assert_eq!(
            npm.canonicalize().unwrap(),
            root.join("bin/npm").canonicalize().unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
