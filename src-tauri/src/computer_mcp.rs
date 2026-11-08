//! Computer use MCP bridge (ADR-038). Provider CLIs spawn this binary with
//! `--mcp-computer`; tool calls reach the app over a private socket with a
//! per-session token. The app owns every policy decision: the Settings switch,
//! a fixed blocklist, per-session app approval, planning read-only, the
//! physical Escape kill switch and the visible control indicator.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use crate::runtime::Manager;
use crate::runtime::{AppHandle, Emitter, State};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::commands::AppState;
#[cfg(target_os = "macos")]
use crate::computer::MouseButton;
use crate::computer::{self, AppInfo};
use crate::error::{Error, Result as IpcResult};

pub const SOCKET_ENV: &str = "SIRUS_COMPUTER_SOCKET";
pub const TOKEN_ENV: &str = "SIRUS_COMPUTER_TOKEN";
pub const SERVER_NAME: &str = "sirus_computer";
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const HISTORY_LIMIT: usize = 200;
/// The person must have left keyboard and mouse alone this long before an app is brought forward.
const FOREGROUND_IDLE: f64 = 1.5;

/// Never controllable: secrets, system security, shells/automation (typing into
/// them would bypass provider sandboxes and approvals) and Sirus itself.
const BLOCKED: &[(&str, &str)] = &[
    ("com.1password.1password", "1Password"),
    ("com.agilebits.onepassword7", "1Password 7"),
    ("com.bitwarden.desktop", "Bitwarden"),
    ("com.lastpass.lastpassmacdesktop", "LastPass"),
    ("com.apple.keychainaccess", "Keychain Access"),
    ("com.apple.passwords", "Passwords"),
    ("com.apple.systempreferences", "System Settings"),
    ("com.apple.securityagent", "SecurityAgent"),
    ("com.apple.terminal", "Terminal"),
    ("com.googlecode.iterm2", "iTerm"),
    ("dev.warp.warp-stable", "Warp"),
    ("net.kovidgoyal.kitty", "kitty"),
    ("com.github.wez.wezterm", "WezTerm"),
    ("org.alacritty", "Alacritty"),
    ("com.mitchellh.ghostty", "Ghostty"),
    ("com.apple.scripteditor2", "Script Editor"),
    ("com.apple.automator", "Automator"),
    ("com.apple.shortcuts", "Shortcuts"),
    ("com.siruscode.app", "Sirus"),
    ("com.siruscode.native", "Sirus Native"),
];

pub fn is_blocked(bundle_id: &str) -> bool {
    let id = bundle_id.to_ascii_lowercase();
    if SELF_ALLOWED.load(Ordering::SeqCst) && id.starts_with("com.siruscode.") {
        return false;
    }
    BLOCKED.iter().any(|(blocked, _)| *blocked == id)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub at: String,
    pub session_id: String,
    pub app: String,
    pub action: String,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: String,
    pub session_id: String,
    pub app: String,
    pub bundle_id: String,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub session_id: String,
    pub bundle_id: String,
    pub app: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub supported: bool,
    pub enabled: bool,
    pub accessibility: bool,
    pub screen_recording: bool,
    pub requests: Vec<Request>,
    pub grants: Vec<Grant>,
    pub history: Vec<HistoryEntry>,
    pub blocked: Vec<String>,
}

struct Pending {
    request: Request,
    reply: mpsc::Sender<bool>,
}

#[derive(Default)]
struct Bridge {
    tokens: HashMap<String, String>,
    grants: HashMap<String, Vec<(String, String)>>,
    pending: Vec<Pending>,
    history: VecDeque<HistoryEntry>,
    last_action: Option<Instant>,
    synthetic_escape: Option<Instant>,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
/// The person chose to let agents drive Sirus itself (Settings → Computador).
static SELF_ALLOWED: AtomicBool = AtomicBool::new(false);

pub fn set_self_allowed(value: bool) {
    SELF_ALLOWED.store(value, Ordering::SeqCst);
}
static OVERLAY_GENERATION: AtomicU64 = AtomicU64::new(0);
static BRIDGE: Mutex<Option<Bridge>> = Mutex::new(None);
static APP: OnceLock<(AppHandle, PathBuf)> = OnceLock::new();

fn with_bridge<T>(f: impl FnOnce(&mut Bridge) -> T) -> T {
    let mut guard = BRIDGE.lock();
    f(guard.get_or_insert_with(Bridge::default))
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// Mirrors the persisted Settings switch; turning it off revokes everything at once.
pub fn set_enabled(value: bool) {
    let was = ENABLED.swap(value, Ordering::SeqCst);
    if was && !value {
        revoke_everything("turned off");
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn record(session_id: &str, app: &str, action: &str, outcome: &str) {
    with_bridge(|bridge| {
        bridge.history.push_front(HistoryEntry {
            at: now(),
            session_id: session_id.into(),
            app: app.into(),
            action: action.into(),
            outcome: outcome.into(),
        });
        bridge.history.truncate(HISTORY_LIMIT);
    });
}

pub fn snapshot() -> Snapshot {
    let permissions = computer::permissions();
    with_bridge(|bridge| Snapshot {
        supported: permissions.supported,
        enabled: enabled(),
        accessibility: permissions.accessibility,
        screen_recording: permissions.screen_recording,
        requests: bridge
            .pending
            .iter()
            .map(|pending| pending.request.clone())
            .collect(),
        grants: bridge
            .grants
            .iter()
            .flat_map(|(session_id, apps)| {
                apps.iter().map(|(bundle_id, app)| Grant {
                    session_id: session_id.clone(),
                    bundle_id: bundle_id.clone(),
                    app: app.clone(),
                })
            })
            .collect(),
        history: bridge.history.iter().cloned().collect(),
        blocked: BLOCKED
            .iter()
            .map(|(_, name)| (*name).to_string())
            .collect(),
    })
}

fn publish() {
    if let Some((app, _)) = APP.get() {
        let _ = app.emit("computer-state", snapshot());
    }
}

fn revoke_everything(reason: &str) {
    let pending = with_bridge(|bridge| {
        bridge.grants.clear();
        bridge.last_action = None;
        std::mem::take(&mut bridge.pending)
    });
    for item in pending {
        let _ = item.reply.send(false);
    }
    record("", "", "stop", reason);
    hide_overlay();
    publish();
}

/// Stop button in the native indicator.
pub fn emergency_stop() {
    revoke_everything("stopped by the person");
}

/// Physical Escape stops control while an agent was acting recently, but not
/// the Escape an agent itself just sent.
pub fn escape_pressed() {
    let active = with_bridge(|bridge| {
        bridge
            .last_action
            .is_some_and(|at| at.elapsed() < Duration::from_secs(15))
            && !bridge
                .synthetic_escape
                .is_some_and(|at| at.elapsed() < Duration::from_millis(600))
    });
    if active {
        revoke_everything("stopped with Escape");
    }
}

// ---------- Endpoints and provider wiring ----------

pub fn init(app: AppHandle, data_dir: PathBuf, enabled: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
    let dir = data_dir.join("computer-mcp");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let socket = dir.join("bridge.sock");
    if APP.set((app.clone(), socket.clone())).is_err() {
        return;
    }
    start_server(app, socket);
}

pub struct Endpoint {
    pub executable: String,
    pub socket: String,
    pub token: String,
}

/// The computer server is offered to a turn only while the Settings switch is on,
/// and only in the macOS app, the one build whose bridge server listens.
pub fn endpoint(session_id: &str) -> Option<Endpoint> {
    if !cfg!(target_os = "macos") || !enabled() {
        return None;
    }
    let (_, socket) = APP.get()?;
    let executable = std::env::current_exe().ok()?.to_string_lossy().to_string();
    let token = with_bridge(|bridge| {
        bridge
            .tokens
            .entry(session_id.to_string())
            .or_insert_with(crate::browser_mcp::new_token)
            .clone()
    });
    Some(Endpoint {
        executable,
        socket: socket.to_string_lossy().to_string(),
        token,
    })
}

pub(crate) fn socket_path() -> Option<PathBuf> {
    APP.get().map(|(_, socket)| socket.clone())
}

pub(crate) fn token_known(token: &str) -> bool {
    with_bridge(|bridge| {
        bridge
            .tokens
            .values()
            .any(|known| crate::browser_mcp::constant_time_eq(known.as_bytes(), token.as_bytes()))
    })
}

pub(crate) fn http_server() -> crate::mcp_stdio::Server {
    crate::mcp_stdio::Server {
        name: SERVER_NAME,
        socket_env: SOCKET_ENV,
        token_env: TOKEN_ENV,
        tools: tool_definitions,
        failure: "the computer action failed",
        instructions: None,
    }
}

/// Claude `mcpServers` entry (merged into the browser config file).
pub fn claude_server(session_id: &str) -> Option<Value> {
    let endpoint = endpoint(session_id)?;
    #[cfg(unix)]
    if let Some(entry) = crate::mcp_http::claude_entry(SERVER_NAME, &endpoint.token) {
        return Some(entry);
    }
    Some(
        json!({ "command": endpoint.executable, "args": ["--mcp-computer"], "env": { SOCKET_ENV: endpoint.socket, TOKEN_ENV: endpoint.token } }),
    )
}

/// Codex `-c` overrides plus the secret environment for the Codex process; the
/// app gate replaces Codex's per-call MCP prompt. The token never goes on the
/// command line (visible in `ps`): Codex forwards it to the server through
/// `env_vars` (ADR-100).
pub fn codex_overrides(session_id: &str) -> (Vec<String>, Vec<(&'static str, String)>) {
    let Some(endpoint) = endpoint(session_id) else {
        return (Vec::new(), Vec::new());
    };
    codex_overrides_for(&endpoint)
}

fn codex_overrides_for(endpoint: &Endpoint) -> (Vec<String>, Vec<(&'static str, String)>) {
    let prefix = format!("mcp_servers.{SERVER_NAME}");
    (
        vec![
            format!("{prefix}.command={:?}", endpoint.executable),
            format!("{prefix}.args=[\"--mcp-computer\"]"),
            format!("{prefix}.env.{SOCKET_ENV}={:?}", endpoint.socket),
            format!("{prefix}.env_vars=[{TOKEN_ENV:?}]"),
            format!("{prefix}.default_tools_approval_mode=\"approve\""),
        ],
        vec![(TOKEN_ENV, endpoint.token.clone())],
    )
}

/// ACP (OpenCode) MCP server entry.
pub fn acp_server(session_id: &str) -> Option<Value> {
    let endpoint = endpoint(session_id)?;
    Some(json!({
        "name": SERVER_NAME,
        "command": endpoint.executable,
        "args": ["--mcp-computer"],
        "env": [ { "name": SOCKET_ENV, "value": endpoint.socket }, { "name": TOKEN_ENV, "value": endpoint.token } ]
    }))
}

#[cfg(target_os = "macos")]
fn start_server(app: AppHandle, socket: PathBuf) {
    let _ = std::fs::remove_file(&socket);
    crate::runtime::async_runtime::spawn(async move {
        let listener = match tokio::net::UnixListener::bind(&socket) {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(%error, "computer mcp: cannot bind bridge socket");
                return;
            }
        };
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));
        }
        while let Ok((stream, _)) = listener.accept().await {
            let app = app.clone();
            crate::runtime::async_runtime::spawn(
                async move { handle_connection(stream, app).await },
            );
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn start_server(_app: AppHandle, _socket: PathBuf) {}

#[cfg(target_os = "macos")]
async fn handle_connection(stream: tokio::net::UnixStream, app: AppHandle) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.len() > 256 * 1024 {
            break;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let token = request
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let session = with_bridge(|bridge| {
            bridge
                .tokens
                .iter()
                .find(|(_, known)| {
                    crate::browser_mcp::constant_time_eq(known.as_bytes(), token.as_bytes())
                })
                .map(|(session, _)| session.clone())
        });
        let response = match session {
            None => json!({ "id": id, "ok": false, "error": "unknown computer session" }),
            Some(session) => {
                let tool = request
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let arguments = request
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let owned = app.clone();
                match tokio::task::spawn_blocking(move || {
                    let result = execute(&owned, &session, &tool, &arguments);
                    if let Ok(value) = &result {
                        crate::tool_images::remember(&session, &tool, value);
                    }
                    result
                })
                .await
                {
                    Ok(Ok(value)) => json!({ "id": id, "ok": true, "result": value }),
                    Ok(Err(error)) => json!({ "id": id, "ok": false, "error": error }),
                    Err(_) => {
                        json!({ "id": id, "ok": false, "error": "the computer action did not complete" })
                    }
                }
            }
        };
        let Ok(encoded) = serde_json::to_string(&response) else {
            continue;
        };
        if write.write_all(encoded.as_bytes()).await.is_err()
            || write.write_all(b"\n").await.is_err()
        {
            break;
        }
    }
}

// ---------- Tool execution ----------

fn arg_text(args: &Value, key: &str, limit: usize) -> Option<String> {
    let value = args.get(key)?.as_str()?;
    (value.chars().count() <= limit).then(|| value.to_string())
}

fn arg_name(args: &Value, key: &str) -> Result<String, String> {
    arg_text(args, key, 200)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .ok_or_else(|| format!("provide `{key}`"))
}

fn arg_number(args: &Value, key: &str, limit: f64) -> Result<Option<f64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|n| n.is_finite() && n.abs() <= limit)
            .map(Some)
            .ok_or_else(|| format!("invalid `{key}`")),
    }
}

fn valid_ref(reference: &str) -> bool {
    (2..=5).contains(&reference.len())
        && reference.starts_with('e')
        && reference[1..].chars().all(|c| c.is_ascii_digit())
}

#[cfg(target_os = "macos")]
fn session_context(app: &AppHandle, session_id: &str) -> Option<(String, bool, String)> {
    let state = app.state::<Arc<AppState>>();
    let data = state.data.lock();
    let session = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)?;
    Some((
        session.agent.title().to_string(),
        session.execution.planning,
        data.settings.locale.clone(),
    ))
}

#[cfg(target_os = "macos")]
fn resolve(name: &str) -> Result<AppInfo, String> {
    let apps = computer::running_apps();
    let lower = name.to_lowercase();
    apps.iter()
        .find(|app| app.bundle_id.to_lowercase() == lower)
        .or_else(|| apps.iter().find(|app| app.name.to_lowercase() == lower))
        .or_else(|| apps.iter().find(|app| app.name.to_lowercase().contains(&lower)))
        .or_else(|| {
            apps.iter().find(|app| {
                computer::app_file_name(app.pid).is_some_and(|file| file.to_lowercase() == lower)
            })
        })
        .cloned()
        .or_else(|| computer::app_by_bundle(name))
        .ok_or_else(|| format!("{name} is not running. Use computer_apps to list apps or computer_launch with a bundle id."))
}

#[cfg(any(target_os = "macos", test))]
fn granted(session_id: &str, bundle_id: &str) -> bool {
    with_bridge(|bridge| {
        bridge.grants.get(session_id).is_some_and(|apps| {
            apps.iter()
                .any(|(id, _)| id.eq_ignore_ascii_case(bundle_id))
        })
    })
}

/// Blocks the tool call until the person allows or declines this app for the session.
#[cfg(target_os = "macos")]
fn authorize(
    app: &AppHandle,
    session_id: &str,
    target: &AppInfo,
    provider: &str,
) -> Result<(), String> {
    let _ = app;
    if target.pid == std::process::id() as i32 || is_blocked(&target.bundle_id) {
        record(session_id, &target.name, "access", "blocked");
        publish();
        return Err(format!(
            "{} cannot be controlled by agents; it is blocked for safety.",
            target.name
        ));
    }
    if granted(session_id, &target.bundle_id) {
        return Ok(());
    }
    let (reply, wait) = mpsc::channel();
    let request = Request {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.into(),
        app: target.name.clone(),
        bundle_id: target.bundle_id.clone(),
        provider: provider.into(),
    };
    let id = request.id.clone();
    with_bridge(|bridge| bridge.pending.push(Pending { request, reply }));
    publish();
    // The host has no window to bounce; its window client shows the request from `computer-state`.
    let allowed = wait.recv_timeout(APPROVAL_TIMEOUT).unwrap_or(false);
    with_bridge(|bridge| bridge.pending.retain(|pending| pending.request.id != id));
    if allowed && enabled() {
        with_bridge(|bridge| {
            let apps = bridge.grants.entry(session_id.into()).or_default();
            if !apps
                .iter()
                .any(|(bundle, _)| bundle.eq_ignore_ascii_case(&target.bundle_id))
            {
                apps.push((target.bundle_id.clone(), target.name.clone()));
            }
        });
        record(session_id, &target.name, "access", "allowed");
        publish();
        Ok(())
    } else {
        record(session_id, &target.name, "access", "declined");
        publish();
        Err(format!(
            "The person did not allow control of {}.",
            target.name
        ))
    }
}

fn hide_overlay() {
    // The indicator is the window app's; the host has no AppKit main thread (ADR-189).
}

/// Shows the indicator over the controlled window and hides it after a quiet period.
fn show_overlay(provider: &str, target: &AppInfo, locale: &str, frame: Option<computer::Frame>) {
    let _ = (provider, target, locale, frame);
}

#[cfg(target_os = "macos")]
fn observation(
    session_id: &str,
    target: &AppInfo,
    window: Option<String>,
    limit: usize,
    screenshot: bool,
) -> Result<Value, String> {
    let session = session_id.to_string();
    let app = target.clone();
    let observed = computer::with_driver(move |driver| {
        driver.observe(&session, &app, window.as_deref(), limit)
    })??;
    let mut value = serde_json::to_value(&observed).unwrap_or(Value::Null);
    if screenshot {
        let image = computer::screenshot(target.pid, observed.window_frame, 1280.0)?;
        use base64::Engine;
        value["image"] = json!({ "mimeType": "image/jpeg", "data": base64::engine::general_purpose::STANDARD.encode(&image.jpeg) });
        value["imageSize"] = json!({ "width": image.width, "height": image.height });
    }
    Ok(value)
}

#[cfg(target_os = "macos")]
fn execute(app: &AppHandle, session_id: &str, tool: &str, args: &Value) -> Result<Value, String> {
    if !enabled() {
        return Err(
            "Computer use is turned off. The person can enable it in Sirus Settings → Computer."
                .into(),
        );
    }
    let (provider, planning, locale) = session_context(app, session_id)
        .ok_or_else(|| "this session no longer exists".to_string())?;
    if tool == "computer_apps" {
        let apps: Vec<Value> = computer::running_apps()
            .into_iter()
            .map(|app| json!({ "name": app.name, "bundleId": app.bundle_id, "active": app.active, "blocked": is_blocked(&app.bundle_id) }))
            .collect();
        return Ok(json!({ "apps": apps }));
    }
    let mutating = !matches!(tool, "computer_observe" | "computer_screenshot");
    if mutating && planning {
        return Err(
            "Planning mode is read-only; computer actions are unavailable until planning is off."
                .into(),
        );
    }
    let target = if tool == "computer_launch" {
        let bundle = arg_name(args, "bundleId")?;
        if !bundle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        {
            return Err("invalid bundle id".into());
        }
        if let Ok(running) = resolve(&bundle) {
            running
        } else {
            let placeholder = AppInfo {
                pid: 0,
                bundle_id: bundle.clone(),
                name: computer::installed_name(&bundle).unwrap_or_else(|| bundle.clone()),
                active: false,
            };
            authorize(app, session_id, &placeholder, &provider)?;
            let launched = computer::launch(&bundle)?;
            record(session_id, &launched.name, "launch", "done");
            publish();
            return Ok(
                json!({ "launched": launched.name, "bundleId": launched.bundle_id, "hint": "Call computer_observe next." }),
            );
        }
    } else {
        resolve(&arg_name(args, "app")?)?
    };
    authorize(app, session_id, &target, &provider)?;
    let observe_after = args.get("observe").and_then(Value::as_bool).unwrap_or(true);
    let pid = target.pid;
    let session = session_id.to_string();
    let result: Result<Value, String> = match tool {
        "computer_observe" => {
            let limit = arg_number(args, "maxElements", 400.0)?
                .map(|n| n.max(20.0) as usize)
                .unwrap_or(200);
            let window =
                arg_text(args, "window", 200).filter(|title| !title.chars().any(char::is_control));
            let screenshot = args
                .get("screenshot")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            observation(session_id, &target, window, limit, screenshot)
        }
        "computer_screenshot" => {
            let frame = computer::with_driver(move |driver| driver.window_frame(pid))?
                .ok_or_else(|| "the application has no open window".to_string())?;
            let image = computer::screenshot(pid, frame, 1280.0)?;
            use base64::Engine;
            Ok(
                json!({ "image": { "mimeType": "image/jpeg", "data": base64::engine::general_purpose::STANDARD.encode(&image.jpeg) }, "window": target.name, "imageSize": { "width": image.width, "height": image.height }, "windowSize": { "width": frame.width, "height": frame.height } }),
            )
        }
        "computer_launch" | "computer_focus" => {
            if computer::idle_seconds() < FOREGROUND_IDLE {
                return Err(
                    "The person is using the computer right now; try again in a moment.".into(),
                );
            }
            computer::activate(pid).map(|_| json!({ "focused": target.name }))
        }
        "computer_click" => {
            let count = arg_number(args, "count", 3.0)?
                .map(|n| n.max(1.0) as u32)
                .unwrap_or(1);
            let button = if args.get("button").and_then(Value::as_str) == Some("right") {
                MouseButton::Right
            } else {
                MouseButton::Left
            };
            if let Some(reference) = arg_text(args, "ref", 8) {
                if !valid_ref(&reference) {
                    return Err("invalid element ref".into());
                }
                computer::with_driver(move |driver| {
                    driver.press(&session, pid, &reference, count, button)
                })?
                .map(|how| json!({ "done": how }))
            } else {
                let (Some(x), Some(y)) = (
                    arg_number(args, "x", 20_000.0)?,
                    arg_number(args, "y", 20_000.0)?,
                ) else {
                    return Err(
                        "provide `ref` from computer_observe or window-relative `x`/`y`".into(),
                    );
                };
                let (gx, gy) =
                    computer::with_driver(move |driver| driver.global_point(&session, pid, x, y))??;
                computer::click_at(pid, gx, gy, count, button);
                Ok(json!({ "done": "clicked" }))
            }
        }
        "computer_type" => {
            let text = arg_text(args, "text", 8_000).ok_or_else(|| "provide `text`".to_string())?;
            let replace = args
                .get("replace")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
            let reference = arg_text(args, "ref", 8);
            if reference
                .as_deref()
                .is_some_and(|reference| !valid_ref(reference))
            {
                return Err("invalid element ref".into());
            }
            let typed = text.clone();
            computer::with_driver(move |driver| -> Result<(), String> {
                if let Some(reference) = reference {
                    if replace {
                        return driver.set_value(&session, pid, &reference, &typed);
                    }
                    driver.focus(&session, pid, &reference)?;
                }
                computer::type_text(pid, &typed);
                Ok(())
            })??;
            if submit {
                computer::press_key(pid, 36, computer::modifier_flags(&[])?);
            }
            Ok(json!({ "done": "typed", "characters": text.chars().count() }))
        }
        "computer_key" => {
            let key = arg_name(args, "key")?;
            let code = computer::key_code(&key).ok_or_else(|| format!("unknown key {key}"))?;
            let modifiers: Vec<String> = args
                .get("modifiers")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .take(4)
                        .collect()
                })
                .unwrap_or_default();
            let flags = computer::modifier_flags(&modifiers)?;
            if code == 53 {
                with_bridge(|bridge| bridge.synthetic_escape = Some(Instant::now()));
            }
            computer::press_key(pid, code, flags);
            Ok(json!({ "done": "pressed", "key": key }))
        }
        "computer_scroll" => {
            let dy = arg_number(args, "dy", 20_000.0)?.unwrap_or(0.0) as i32;
            let dx = arg_number(args, "dx", 20_000.0)?.unwrap_or(0.0) as i32;
            let reference = arg_text(args, "ref", 8).filter(|reference| valid_ref(reference));
            let (x, y) = computer::with_driver(move |driver| match reference {
                Some(reference) => driver.element_center(&session, pid, &reference),
                None => driver
                    .window_frame(pid)
                    .map(|frame| frame.center())
                    .ok_or_else(|| "the application has no open window".to_string()),
            })??;
            computer::scroll(pid, x, y, dx, dy);
            Ok(json!({ "done": "scrolled" }))
        }
        _ => Err("unknown computer tool".into()),
    };
    let action = tool.trim_start_matches("computer_");
    match &result {
        Ok(_) => record(session_id, &target.name, action, "done"),
        Err(_) => record(session_id, &target.name, action, "failed"),
    }
    publish();
    let mut value = result?;
    if mutating {
        with_bridge(|bridge| bridge.last_action = Some(Instant::now()));
        let frame = computer::with_driver(move |driver| driver.window_frame(pid))
            .ok()
            .flatten();
        show_overlay(&provider, &target, &locale, frame);
        // Act-then-see in one round trip: the fresh state after the UI settles.
        if observe_after
            && matches!(
                tool,
                "computer_click" | "computer_type" | "computer_key" | "computer_scroll"
            )
        {
            std::thread::sleep(Duration::from_millis(180));
            if let Ok(after) = observation(session_id, &target, None, 200, false) {
                value["after"] = after;
            }
        }
    }
    Ok(value)
}

#[cfg(not(target_os = "macos"))]
fn execute(
    _app: &AppHandle,
    _session_id: &str,
    _tool: &str,
    _args: &Value,
) -> Result<Value, String> {
    Err("Computer use is only available on macOS".into())
}

pub fn tool_definitions() -> Vec<Value> {
    let app =
        json!({ "type": "string", "description": "App name or bundle id from computer_apps" });
    let observe = json!({ "type": "boolean", "description": "Return the app state after the action (default true)" });
    vec![
        json!({ "name": "computer_apps", "description": "List running Mac apps (name, bundle id, active, blocked). Start here.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "computer_observe", "description": "Read one app window as a bounded list of elements with refs (role, label, value, window-relative frame). Prefer this over screenshots. The person must allow each app once per session.", "inputSchema": { "type": "object", "properties": { "app": app, "window": { "type": "string", "description": "Optional window title filter" }, "screenshot": { "type": "boolean", "description": "Also attach a JPEG of the window" }, "maxElements": { "type": "integer", "minimum": 20, "maximum": 400 } }, "required": ["app"] } }),
        json!({ "name": "computer_screenshot", "description": "JPEG of the app's window (longest side ≤ 1280 px). Use when the element list is not enough.", "inputSchema": { "type": "object", "properties": { "app": app }, "required": ["app"] } }),
        json!({ "name": "computer_click", "description": "Click an element by ref (semantic press, works in the background) or at window-relative x/y from the last observation.", "inputSchema": { "type": "object", "properties": { "app": app, "ref": { "type": "string" }, "x": { "type": "number" }, "y": { "type": "number" }, "count": { "type": "integer", "minimum": 1, "maximum": 3 }, "button": { "type": "string", "enum": ["left", "right"] }, "observe": observe }, "required": ["app"] } }),
        json!({ "name": "computer_type", "description": "Type text into the app (into `ref` if given). `replace` sets the field value directly; `submit` presses Return.", "inputSchema": { "type": "object", "properties": { "app": app, "text": { "type": "string" }, "ref": { "type": "string" }, "replace": { "type": "boolean" }, "submit": { "type": "boolean" }, "observe": observe }, "required": ["app", "text"] } }),
        json!({ "name": "computer_key", "description": "Press a key with optional modifiers (cmd, shift, option, control). Keys: letters, digits, return, tab, space, delete, escape, arrows, home, end, pageup, pagedown, f1–f12.", "inputSchema": { "type": "object", "properties": { "app": app, "key": { "type": "string" }, "modifiers": { "type": "array", "items": { "type": "string" } }, "observe": observe }, "required": ["app", "key"] } }),
        json!({ "name": "computer_scroll", "description": "Scroll the window (or the element `ref`) by pixels; positive dy scrolls down.", "inputSchema": { "type": "object", "properties": { "app": app, "dy": { "type": "number" }, "dx": { "type": "number" }, "ref": { "type": "string" }, "observe": observe }, "required": ["app"] } }),
        json!({ "name": "computer_launch", "description": "Open an app by bundle id (e.g. com.apple.calculator) in the background.", "inputSchema": { "type": "object", "properties": { "bundleId": { "type": "string" } }, "required": ["bundleId"] } }),
        json!({ "name": "computer_focus", "description": "Bring an app to the front. Only when the task needs it visible; most actions work in the background.", "inputSchema": { "type": "object", "properties": { "app": app }, "required": ["app"] } }),
    ]
}

/// Child mode used by provider CLIs.
pub fn run_stdio() -> i32 {
    crate::mcp_stdio::run(&crate::mcp_stdio::Server {
        name: SERVER_NAME,
        socket_env: SOCKET_ENV,
        token_env: TOKEN_ENV,
        tools: tool_definitions,
        failure: "the computer action failed",
        instructions: None,
    })
}

// ---------- IPC ----------

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    Status,
    RequestAccessibility,
    RequestScreenRecording,
    OpenAccessibilitySettings,
    OpenScreenRecordingSettings,
    Respond {
        request_id: String,
        allow: bool,
    },
    Revoke {
        session_id: String,
        bundle_id: String,
    },
    Stop,
}

/// Closed computer-use control surface for the renderer: status, fixed
/// permission prompts/panes, approval answers, revocation and stop.
pub async fn computer_action(
    state: State<'_, Arc<AppState>>,
    action: Action,
) -> IpcResult<Snapshot> {
    state.ensure_running()?;
    match action {
        Action::Status => {}
        #[cfg(target_os = "macos")]
        Action::RequestAccessibility => {
            computer::request_accessibility();
        }
        #[cfg(target_os = "macos")]
        Action::RequestScreenRecording => {
            computer::request_screen_recording();
        }
        #[cfg(target_os = "macos")]
        Action::OpenAccessibilitySettings => computer::open_privacy_pane(false),
        #[cfg(target_os = "macos")]
        Action::OpenScreenRecordingSettings => computer::open_privacy_pane(true),
        #[cfg(not(target_os = "macos"))]
        Action::RequestAccessibility
        | Action::RequestScreenRecording
        | Action::OpenAccessibilitySettings
        | Action::OpenScreenRecordingSettings => {
            return Err(Error::agent("Computer use is only available on macOS"));
        }
        Action::Respond { request_id, allow } => {
            // Out of the list as it is answered, so the returned snapshot cannot reopen the card.
            let reply = with_bridge(|bridge| {
                let at = bridge
                    .pending
                    .iter()
                    .position(|pending| pending.request.id == request_id)?;
                Some(bridge.pending.remove(at).reply)
            });
            let reply = reply.ok_or_else(|| Error::agent("That request is no longer waiting."))?;
            let _ = reply.send(allow);
        }
        Action::Revoke {
            session_id,
            bundle_id,
        } => {
            with_bridge(|bridge| {
                if let Some(apps) = bridge.grants.get_mut(&session_id) {
                    apps.retain(|(bundle, _)| !bundle.eq_ignore_ascii_case(&bundle_id));
                }
                bridge.grants.retain(|_, apps| !apps.is_empty());
            });
            publish();
        }
        Action::Stop => emergency_stop(),
    }
    Ok(snapshot())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_overrides_keep_the_token_out_of_argv() {
        let endpoint = Endpoint {
            executable: "/Applications/Sirus Code.app/sirus".into(),
            socket: "/tmp/sirus-computer.sock".into(),
            token: "computer-secret-token".into(),
        };
        let (args, env) = codex_overrides_for(&endpoint);
        assert!(args
            .iter()
            .all(|arg| !arg.contains("computer-secret-token")));
        assert!(args.contains(&format!(
            "mcp_servers.{SERVER_NAME}.env_vars=[\"{TOKEN_ENV}\"]"
        )));
        assert_eq!(env, vec![(TOKEN_ENV, "computer-secret-token".to_string())]);
    }

    #[test]
    fn blocklist_covers_secrets_shells_and_sirus() {
        for id in [
            "com.apple.Terminal",
            "com.googlecode.iterm2",
            "com.apple.keychainaccess",
            "com.siruscode.app",
            "com.apple.systempreferences",
        ] {
            assert!(is_blocked(id), "{id} must be blocked");
        }
        assert!(!is_blocked("com.apple.calculator"));
    }

    #[test]
    fn tools_are_few_named_and_schematized() {
        let tools = tool_definitions();
        assert!(tools.len() <= 10);
        for tool in &tools {
            assert!(tool["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("computer_")));
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn refs_and_numbers_are_validated() {
        assert!(valid_ref("e1") && valid_ref("e400"));
        assert!(!valid_ref("e") && !valid_ref("x1") && !valid_ref("e1a"));
        assert!(arg_number(&json!({ "x": 10.5 }), "x", 100.0).unwrap() == Some(10.5));
        assert!(arg_number(&json!({ "x": 1e9 }), "x", 100.0).is_err());
        assert!(arg_name(&json!({ "app": "  " }), "app").is_err());
    }

    #[test]
    fn approval_and_stop_are_published_and_revocable() {
        set_enabled(true);
        with_bridge(|bridge| {
            bridge.grants.insert(
                "s".into(),
                vec![("com.apple.calculator".into(), "Calculator".into())],
            )
        });
        assert!(granted("s", "COM.APPLE.CALCULATOR"));
        let (reply, wait) = mpsc::channel();
        with_bridge(|bridge| {
            bridge.pending.push(Pending {
                request: Request {
                    id: "r".into(),
                    session_id: "s".into(),
                    app: "Notes".into(),
                    bundle_id: "com.apple.Notes".into(),
                    provider: "Codex".into(),
                },
                reply,
            })
        });
        emergency_stop();
        assert!(!granted("s", "com.apple.calculator"));
        assert_eq!(
            wait.recv().ok(),
            Some(false),
            "a stop declines pending approvals"
        );
        assert!(snapshot()
            .history
            .iter()
            .any(|entry| entry.action == "stop"));
    }
}
