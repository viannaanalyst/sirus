//! Fixed, read-only provider CLI version checks (npm registry) and explicit,
//! user-confirmed `npm install -g` updates for the allowlisted packages.
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::runtime::State;
use parking_lot::Mutex;

use crate::commands::AppState;
use crate::detect;
use crate::error::{Error, Result};
use crate::models::{AgentProviderId, ProviderUpdate, ProviderUpdateResult};

/// Only CLIs with a fixed, first-party npm package can be checked or updated.
const NPM_PACKAGES: &[(AgentProviderId, &str)] = &[
    (AgentProviderId::Codex, "@openai/codex"),
    (AgentProviderId::Claude, "@anthropic-ai/claude-code"),
    (AgentProviderId::OpenCode, "opencode-ai"),
];

const CHECK_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_REGISTRY_BYTES: usize = 256 * 1024;
const UPDATE_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_MESSAGE_CHARS: usize = 240;

static CACHE: Mutex<Option<(Instant, Vec<ProviderUpdate>)>> = Mutex::new(None);

fn npm_package(provider: &AgentProviderId) -> Option<&'static str> {
    NPM_PACKAGES
        .iter()
        .find(|(candidate, _)| *candidate == *provider)
        .map(|(_, package)| *package)
}

/// True when the detected binary resolves inside an npm-managed tree.
fn npm_managed(path: &str) -> bool {
    std::fs::canonicalize(path)
        .map(|canonical| {
            canonical
                .components()
                .any(|component| component.as_os_str() == "node_modules")
        })
        .unwrap_or(false)
}

/// The npm that owns an npm-installed binary: `<prefix>/bin/npm` for the prefix whose
/// `lib/node_modules` holds the package, else npm beside the binary's link, else npm on the
/// CLI search path. Another npm (for example Homebrew's beside a `~/.local` install) would
/// install the update into its own prefix and leave this binary as it was.
fn npm_for(path: &str) -> Option<std::path::PathBuf> {
    let executable = |candidate: std::path::PathBuf| candidate.is_file().then_some(candidate);
    let canonical = std::fs::canonicalize(path).ok();
    let prefix = canonical.as_deref().and_then(|canonical| {
        canonical
            .ancestors()
            .find(|dir| dir.ends_with("lib/node_modules"))
            .and_then(|dir| dir.parent()?.parent().map(std::path::Path::to_path_buf))
    });
    prefix
        .and_then(|prefix| executable(prefix.join("bin").join("npm")))
        .or_else(|| {
            std::path::Path::new(path)
                .parent()
                .and_then(|dir| executable(dir.join("npm")))
        })
        .or_else(|| which::which_in("npm", Some(crate::detect::cli_path()), "/").ok())
}

/// The binary sits in `~/.opencode/bin`, where OpenCode's install script puts it.
fn curl_installed_opencode(path: &str) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let root = std::path::Path::new(&home).join(".opencode").join("bin");
    std::fs::canonicalize(path)
        .ok()
        .zip(std::fs::canonicalize(root).ok())
        .is_some_and(|(binary, root)| binary.starts_with(root))
}

enum UpdatePlan {
    /// The package, and the npm of the prefix the binary was installed in.
    Npm(&'static str, Option<std::path::PathBuf>),
    Command(String, Vec<String>),
}

/// Fixed update strategy per detection: npm-managed installs update through
/// npm; a natively installed Claude Code updates through its own `update`
/// subcommand, and a script-installed OpenCode through `upgrade --method curl`. Nothing else gets a one-click update.
fn update_plan(install: &crate::models::AgentInstall) -> Option<UpdatePlan> {
    if !install.installed {
        return None;
    }
    let path = install.path.as_deref()?;
    if npm_managed(path) {
        if let Some(package) = npm_package(&install.id) {
            return Some(UpdatePlan::Npm(package, npm_for(path)));
        }
    }
    if install.id == AgentProviderId::Claude {
        return Some(UpdatePlan::Command(
            path.to_string(),
            vec!["update".to_string()],
        ));
    }
    // OpenCode's own installer puts it in ~/.opencode/bin; it upgrades itself.
    if install.id == AgentProviderId::OpenCode && curl_installed_opencode(path) {
        return Some(UpdatePlan::Command(
            path.to_string(),
            vec![
                "upgrade".to_string(),
                "--method".to_string(),
                "curl".to_string(),
            ],
        ));
    }
    None
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

pub async fn provider_updates(
    state: State<'_, Arc<AppState>>,
    refresh: bool,
) -> Result<Vec<ProviderUpdate>> {
    let (enabled, overrides) = {
        let data = state.data.lock();
        (
            data.settings.enable_provider_update_checks,
            data.settings.provider_paths.clone(),
        )
    };
    if !enabled {
        return Ok(Vec::new());
    }
    if !refresh {
        if let Some((checked_at, cached)) = CACHE.lock().clone() {
            if checked_at.elapsed() < CHECK_TTL {
                return Ok(cached);
            }
        }
    }
    let installs = crate::runtime::async_runtime::spawn_blocking(move || {
        detect::detect_with_overrides(&overrides)
    })
    .await
    .map_err(|error| Error::new("native", error.to_string()))?;
    let mut updates = Vec::new();
    for install in installs {
        let installed_version = match &install.path {
            Some(path) => detect::probe_version(path).await,
            None => None,
        };
        let latest_version = match npm_package(&install.id) {
            Some(package) => fetch_latest(package).await.unwrap_or(None),
            None => None,
        };
        let update_available = install.installed
            && installed_version
                .as_deref()
                .and_then(version_parts)
                .zip(latest_version.as_deref().and_then(version_parts))
                .is_some_and(|(installed, latest)| newer(&latest, &installed));
        let update_supported = update_plan(&install).is_some();
        updates.push(ProviderUpdate {
            provider: install.id,
            installed: install.installed,
            installed_version,
            latest_version,
            update_available,
            update_supported,
        });
    }
    *CACHE.lock() = Some((Instant::now(), updates.clone()));
    Ok(updates)
}

pub async fn update_providers(
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
        let Some(plan) = update_plan(install) else {
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
        let run = match plan {
            UpdatePlan::Npm(package, npm) => match &npm {
                Some(npm) => {
                    // The CLI search path, so npm's `#!/usr/bin/env node` finds node when the
                    // app was opened from Finder.
                    let mut command = crate::detect::command(npm);
                    command.args([
                        "install",
                        "-g",
                        "--no-fund",
                        "--no-audit",
                        &format!("{package}@latest"),
                    ]);
                    crate::cli_output::capture_command(command, UPDATE_TIMEOUT).await
                }
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "npm is not available on PATH.",
                )),
            },
            UpdatePlan::Command(binary, args) => {
                let mut command = crate::detect::command(&binary);
                command.args(&args);
                crate::cli_output::capture_command(command, UPDATE_TIMEOUT).await
            }
        };
        let (ok, message) = match run {
            Ok(output) if output.status.success() => (true, String::new()),
            Ok(output) => (false, last_line(&output.stderr)),
            Err(error) => (false, error.to_string()),
        };
        results.push(ProviderUpdateResult {
            provider,
            ok,
            message,
        });
    }
    *CACHE.lock() = None;
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let npm = npm_for(&link.to_string_lossy()).unwrap();
        assert_eq!(
            npm.canonicalize().unwrap(),
            root.join("bin/npm").canonicalize().unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
