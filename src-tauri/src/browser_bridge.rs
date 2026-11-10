//! The window bridge (ADR-189). A host with no window of its own (`sirus-host`) can still give
//! agents a browser when a window client is attached: Sirus Native owns the web views and
//! registers itself over its authenticated socket. Each `browser_*` tool call is validated and
//! shaped here, exactly as `browser_mcp.rs` does for the Mac app, but the page work is sent to
//! the client as a `browser_tool_request` event and answered with a `browser_tool_result`
//! command. The client is the one local connection whose token is the one in `native-token`
//! in the data folder, never a phone, whatever address it comes from. A headless host (no window
//! of its own, ADR-201) also accepts a Sirus window on another computer, paired as a desktop and
//! connected over the remote socket; it may only register, unregister and answer, never run the
//! window's other local commands.
//!
//! Secrets never leave the host except to the registered client: `browser_fill_login` hands the
//! credentials to it as the arguments of the fixed fill script, and only the fields filled
//! (never values) go back to the agent.

// The Mac app runs its own browser; the bridge is for hosts with a window client.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::runtime::{AppHandle, Manager};

/// Set to `1` for the MCP child when a window client can run the browser tools.
pub const WINDOW_ENV: &str = "SIRUS_BROWSER_WINDOW";
/// Set on the host by the window that starts it: the client will attach, so list the tools.
pub const CLIENT_ENV: &str = "SIRUS_BROWSER_CLIENT";
/// The event a registered client receives.
pub const REQUEST_EVENT: &str = "browser_tool_request";
/// Events only the local window may listen to.
pub const LOCAL_EVENTS: &[&str] = &["computer-state", crate::computer_mcp::POINTER_EVENT];
/// The error without a window client, unchanged from the Mac-app-only host.
pub const NO_WINDOW: &str = "the embedded browser is only available in the Mac app";
/// The error on a headless host whose paired Sirus window is not attached right now (ADR-201).
pub const NO_CLIENT: &str = "the embedded browser needs Sirus open on a Mac with this computer selected; ask the person to switch to it";

const TIMEOUT: Duration = Duration::from_secs(30);
const RECORD_STOP_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECTION_COMMANDS: &[&str] = &[
    "register_browser_client",
    "unregister_browser_client",
    "browser_tool_result",
];
const VAULT_COMMANDS: &[&str] = &[
    "login_vault_list",
    "login_vault_save",
    "login_vault_delete",
    "login_vault_set_agent_use",
    "login_vault_matches",
    "login_vault_prompts",
    "login_vault_answer_offer",
    "login_vault_answer_approval",
    "login_vault_note_submission",
    "login_vault_autofill",
    "computer_action",
    "remote_action",
    "remote_pair",
    "dictation_status",
    "dictation_level",
    "start_dictation",
    "stop_dictation",
];

type Frames = tokio::sync::mpsc::Sender<String>;
type Reply = Result<Value, String>;

struct Registered {
    connection: u64,
    frames: Frames,
}

struct Waiting {
    connection: u64,
    reply: std::sync::mpsc::Sender<Reply>,
}

static CLIENT: Mutex<Option<Registered>> = Mutex::new(None);
static WAITING: Mutex<Option<HashMap<String, Waiting>>> = Mutex::new(None);
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

/// An id for a new socket connection.
pub fn next_connection() -> u64 {
    NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed)
}

/// Whether a window client is attached right now.
pub fn has_client() -> bool {
    CLIENT.lock().is_some()
}

/// Whether the MCP children should list the browser tools: a client is attached, the window
/// that started this host said it will attach, or this headless host has a Sirus window paired
/// that attaches whenever it switches to this computer (ADR-201).
pub fn window_expected() -> bool {
    has_client()
        || std::env::var(CLIENT_ENV).as_deref() == Ok("1")
        || crate::remote::desktop_client_possible()
}

/// The error when no client is attached: a paired window can come back on a headless host.
fn no_window() -> String {
    if crate::remote::desktop_client_possible() {
        NO_CLIENT.into()
    } else {
        NO_WINDOW.into()
    }
}

/// Who a socket connection is, for the window's own commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    /// Sirus Native on this computer (`native-token`).
    Window,
    /// A Sirus window on another computer, on a host without a window of its own (ADR-201).
    Desktop,
    /// A phone, a page, or a desktop on a host that has its own window.
    Device,
}

/// `local`: the connection holds `native-token`; `desktop`: its device paired as a Sirus window;
/// `headless`: this host has no window listener of its own.
pub fn client_kind(local: bool, desktop: bool, headless: bool) -> Client {
    if local {
        Client::Window
    } else if desktop && headless {
        Client::Desktop
    } else {
        Client::Device
    }
}

/// Whether `client` may run the local-only `command`: the window all of them, a desktop only
/// the browser client's three, a device none.
pub fn command_allowed(command: &str, client: Client) -> bool {
    match client {
        Client::Window => is_local_command(command),
        Client::Desktop => CONNECTION_COMMANDS.contains(&command),
        Client::Device => false,
    }
}

/// Makes `connection` the window client. A later registration replaces an earlier one, whose
/// pending calls fail at once.
pub fn register(connection: u64, frames: Frames) {
    let replaced = CLIENT.lock().replace(Registered { connection, frames });
    if let Some(old) = replaced {
        if old.connection != connection {
            fail_pending(old.connection, "the Sirus window was replaced");
        }
    }
}

/// The connection went away or unregistered; its pending calls fail immediately.
pub fn unregister(connection: u64) {
    let mut client = CLIENT.lock();
    if client
        .as_ref()
        .is_some_and(|client| client.connection == connection)
    {
        *client = None;
    }
    drop(client);
    fail_pending(connection, "the Sirus window disconnected");
}

fn fail_pending(connection: u64, reason: &str) {
    let gone: Vec<Waiting> = {
        let mut waiting = WAITING.lock();
        let Some(map) = waiting.as_mut() else { return };
        let ids: Vec<String> = map
            .iter()
            .filter(|(_, call)| call.connection == connection)
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter().filter_map(|id| map.remove(&id)).collect()
    };
    for call in gone {
        let _ = call.reply.send(Err(reason.to_string()));
    }
}

/// The client's answer. Ignored unless it comes from the registered connection and names a call
/// still waiting.
pub fn deliver(connection: u64, id: &str, result: Reply) -> bool {
    if CLIENT
        .lock()
        .as_ref()
        .is_none_or(|client| client.connection != connection)
    {
        return false;
    }
    let call = {
        let mut waiting = WAITING.lock();
        match waiting.as_mut().and_then(|map| map.get(id)) {
            Some(call) if call.connection == connection => {}
            _ => return false,
        }
        waiting.as_mut().and_then(|map| map.remove(id))
    };
    match call {
        Some(call) => call.reply.send(result).is_ok(),
        None => false,
    }
}

/// Sends one tool call to the window client and waits for its answer. Blocks: call it from a
/// blocking thread (the MCP bridge already does).
pub fn call_with_timeout(
    session_id: &str,
    tool: &str,
    args: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let (reply, wait) = std::sync::mpsc::channel();
    let (connection, frames) = {
        let client = CLIENT.lock();
        let client = client.as_ref().ok_or_else(no_window)?;
        (client.connection, client.frames.clone())
    };
    WAITING
        .lock()
        .get_or_insert_with(HashMap::new)
        .insert(id.clone(), Waiting { connection, reply });
    let frame = json!({
        "type": "event",
        "event": REQUEST_EVENT,
        "payload": { "id": id, "sessionId": session_id, "tool": tool, "args": args },
    })
    .to_string();
    if frames.try_send(frame).is_err() {
        WAITING.lock().as_mut().and_then(|map| map.remove(&id));
        return Err("the Sirus window is not reachable".into());
    }
    let result = wait
        .recv_timeout(timeout)
        .unwrap_or_else(|_| Err("the Sirus window did not answer".into()));
    WAITING.lock().as_mut().and_then(|map| map.remove(&id));
    result
}

fn call(session_id: &str, tool: &str, args: Value) -> Reply {
    call_with_timeout(session_id, tool, args, TIMEOUT)
}

// ---- Who the local window is ----

/// Whether `token` is the one Sirus Native keeps in `<data dir>/native-token`.
pub fn token_is_local(file_contents: &str, token: &str) -> bool {
    let expected = file_contents.trim();
    !expected.is_empty()
        && crate::browser_mcp::constant_time_eq(expected.as_bytes(), token.trim().as_bytes())
}

/// Whether a connection that said hello with `token` is the local window.
pub fn is_local_window(app: &AppHandle, token: &str) -> bool {
    {
        std::fs::read_to_string(app.data_dir().join("native-token"))
            .is_ok_and(|contents| token_is_local(&contents, token))
    }
}

/// Commands only the local window may run, and that are not in the host's command table.
pub fn is_local_command(command: &str) -> bool {
    CONNECTION_COMMANDS.contains(&command) || VAULT_COMMANDS.contains(&command)
}

fn refuse(message: &str) -> Value {
    json!({ "code": "remote", "message": message })
}

/// Runs a local-only command for the connection `connection`, whose outgoing frames are `frames`.
pub async fn run_local(
    app: &AppHandle,
    connection: u64,
    frames: &Frames,
    command: &str,
    args: &Value,
) -> Result<String, Value> {
    match command {
        "register_browser_client" => {
            register(connection, frames.clone());
            return Ok(json!({ "registered": true }).to_string());
        }
        "unregister_browser_client" => {
            unregister(connection);
            return Ok("null".into());
        }
        "browser_tool_result" => {
            let id = args.get("id").and_then(Value::as_str).unwrap_or_default();
            let result = if args.get("ok").and_then(Value::as_bool) == Some(true) {
                Ok(args.get("value").cloned().unwrap_or(Value::Null))
            } else {
                Err(args
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("the Sirus window could not do that")
                    .chars()
                    .take(1000)
                    .collect())
            };
            deliver(connection, id, result);
            return Ok("null".into());
        }
        _ => {}
    }
    vault_command(app, command, args).await
}

fn text(args: &Value, key: &str) -> Result<String, Value> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| refuse(&format!("`{key}` is required")))
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn reply<T: serde::Serialize, E: serde::Serialize>(result: Result<T, E>) -> Result<String, Value> {
    match result {
        Ok(value) => serde_json::to_string(&value)
            .map_err(|error| json!({ "code": "serialize", "message": error.to_string() })),
        Err(error) => Err(serde_json::to_value(error).unwrap_or(Value::Null)),
    }
}

async fn vault_command(app: &AppHandle, command: &str, args: &Value) -> Result<String, Value> {
    use crate::login_vault as vault;
    match command {
        "login_vault_list" => reply(vault::login_vault_list().await),
        "login_vault_save" => {
            let input: vault::LoginInput =
                serde_json::from_value(args.get("input").cloned().unwrap_or(Value::Null))
                    .map_err(|error| refuse(&format!("invalid `input`: {error}")))?;
            reply(vault::login_vault_save(input).await)
        }
        "login_vault_delete" => reply(vault::login_vault_delete(text(args, "id")?).await),
        "login_vault_set_agent_use" => {
            reply(vault::login_vault_set_agent_use(text(args, "id")?, flag(args, "enabled")).await)
        }
        "login_vault_matches" => reply(vault::login_vault_matches(text(args, "url")?).await),
        "login_vault_prompts" => reply(vault::login_vault_prompts().await),
        "login_vault_answer_offer" => reply(
            vault::login_vault_answer_offer(
                app.clone(),
                text(args, "id")?,
                flag(args, "save"),
                flag(args, "agentUse"),
            )
            .await,
        ),
        "login_vault_answer_approval" => reply(
            vault::login_vault_answer_approval(app.clone(), text(args, "id")?, flag(args, "allow"))
                .await,
        ),
        "login_vault_note_submission" => {
            vault::note_submission(
                app,
                &text(args, "sessionId")?,
                &text(args, "frameUrl")?,
                &text(args, "raw")?,
            );
            Ok("null".into())
        }
        "login_vault_autofill" => {
            let (app, session, login, part) = (
                app.clone(),
                text(args, "sessionId")?,
                text(args, "loginId")?,
                args.get("part").and_then(Value::as_str).map(str::to_string),
            );
            let done = crate::runtime::async_runtime::spawn_blocking(move || {
                let part = if part.as_deref() == Some("totp") {
                    "totp"
                } else {
                    "login"
                };
                person_fill(&app, &session, &login, part).map(|_| ())
            })
            .await
            .unwrap_or_else(|_| Err("the sign-in did not complete".into()));
            reply(done.map_err(|error| crate::error::Error::new("login-vault", error)))
        }
        "computer_action" => {
            let action: crate::computer_mcp::Action =
                serde_json::from_value(args.get("action").cloned().unwrap_or(Value::Null))
                    .map_err(|error| refuse(&format!("invalid `action`: {error}")))?;
            reply(
                crate::computer_mcp::computer_action(
                    app.state::<std::sync::Arc<crate::commands::AppState>>(),
                    action,
                )
                .await,
            )
        }
        // The window is this Mac, so what a device may not do it may (ADR-189).
        "remote_action" => {
            let action = args.get("action").cloned().unwrap_or(Value::Null);
            // Without a listener of its own the window rides the one for devices (an older
            // launch): turning that off would strand it.
            if action["type"] == "setEnabled"
                && action["enabled"] == false
                && !crate::remote::has_local_listener()
            {
                return Err(refuse(
                    "In Sirus Native the host stays on while the app is open.",
                ));
            }
            let action = serde_json::from_value(action)
                .map_err(|error| refuse(&format!("invalid `action`: {error}")))?;
            reply(crate::remote::remote_action(app.clone(), action).await)
        }
        "remote_pair" => reply(crate::remote::remote_pair().await),
        "dictation_status" => reply(Ok::<_, crate::error::Error>(
            crate::dictation::dictation_status(),
        )),
        "dictation_level" => reply(Ok::<_, crate::error::Error>(
            crate::dictation::dictation_level(),
        )),
        "start_dictation" => {
            let locale = args["locale"].as_str().unwrap_or("pt-BR").to_owned();
            reply(crate::dictation::start_dictation(locale).await)
        }
        "stop_dictation" => {
            let cancel = args["cancel"].as_bool().unwrap_or(false);
            reply(crate::dictation::stop_dictation(cancel).await)
        }
        _ => Err(refuse("This action is only available on the Mac.")),
    }
}

// ---- Saved logins through the window ----

/// The active tab's id and address, from the client.
fn active_page(session_id: &str) -> Result<(String, String), String> {
    let state = call(session_id, "browser_status", json!({}))?;
    let tab_id = state["activeTabId"]
        .as_str()
        .ok_or("No browser tab is open.")?;
    let url = state["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["id"].as_str() == Some(tab_id)))
        .and_then(|tab| tab["url"].as_str())
        .ok_or("No browser tab is open.")?;
    Ok((tab_id.to_string(), url.to_string()))
}

fn fill(
    session_id: &str,
    tab_id: &str,
    origin: &str,
    part: &str,
    credentials: &crate::login_vault::Credentials,
) -> Result<Value, String> {
    let code = match part {
        "totp" => Some(
            credentials
                .totp
                .as_deref()
                .and_then(|seed| {
                    crate::login_vault::totp_code(seed, crate::login_vault::unix_now())
                })
                .ok_or("This login has no authenticator secret saved.")?,
        ),
        _ => None,
    };
    // Whatever the page might echo back is masked from the transcript for this turn.
    crate::secrets::note_value(session_id, &credentials.password);
    if let Some(code) = &code {
        crate::secrets::note_value(session_id, code);
    }
    let raw = call(
        session_id,
        "fill_login_page",
        json!({
            "tabId": tab_id,
            "script_args": crate::browser_login::fill_arguments(origin, part, credentials, code.as_deref()),
        }),
    )?;
    let raw = match raw {
        Value::String(text) => text,
        other => other.to_string(),
    };
    crate::browser_login::parse_fill_result(&raw, origin)
}

/// `browser_fill_login`: the model names at most a username; the values never pass through it.
fn agent_fill(app: &AppHandle, session_id: &str, args: &Value) -> Result<Value, String> {
    let part = match args.get("part").and_then(Value::as_str) {
        None | Some("login") => "login",
        Some("totp") => "totp",
        Some(_) => return Err("`part` must be \"login\" or \"totp\"".into()),
    };
    let username = args
        .get("username")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.len() <= 256);
    let (tab_id, url) = active_page(session_id)?;
    let (summary, _) = crate::login_vault::agent_credentials(&url, username)?;
    crate::login_vault::require_approval(app, session_id, &summary.origin, &summary.username)?;
    // The person may have changed the login, or the page, while the card was open.
    let (tab_after, url_after) = active_page(session_id)?;
    let (again, credentials) = crate::login_vault::agent_credentials(&url_after, username)?;
    if tab_after != tab_id || again.id != summary.id {
        return Err("The page or the login changed while waiting; call the tool again.".into());
    }
    fill(session_id, &tab_id, &again.origin, part, &credentials)
}

/// The person's key icon: fills the chosen login into the active tab.
fn person_fill(
    _app: &AppHandle,
    session_id: &str,
    login_id: &str,
    part: &str,
) -> Result<Value, String> {
    let (tab_id, url) = active_page(session_id)?;
    let (summary, credentials) = crate::login_vault::person_credentials(&url, login_id)?;
    fill(session_id, &tab_id, &summary.origin, part, &credentials)
}

// ---- The tools ----

const SCREENSHOT_NOTE: &str =
    "Pages lay out at this CSS size; screenshots show it. The user agent is unchanged.";
const RESIZE_NOTE: &str = "Pages lay out at this CSS size; screenshots show it. Use browser_device with fill: true to return to the pane size.";

/// One `browser_*` tool call: validated here, run by the window client, shaped here.
pub fn browser_tool(
    app: &AppHandle,
    session_id: &str,
    tool: &str,
    args: Value,
) -> Result<Value, String> {
    use crate::browser_mcp::{action_value, arg_str, element_arg, valid_selector};
    if !has_client() {
        return Err(no_window());
    }
    let run = |name: &str, args: Value| call(session_id, name, args);
    match tool {
        "browser_status" | "browser_tabs" => run(tool, json!({})),
        "browser_open" => {
            let url = match args.get("port") {
                Some(port) => Some(crate::browser_agent::localhost_url(port)?),
                None => args
                    .get("url")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|url| crate::browser::is_allowed_browser_url(url)),
            };
            run(tool, json!({ "url": url }))
        }
        "browser_navigate" => {
            let url = arg_str(&args, "url", 2048)
                .filter(|url| crate::browser::is_allowed_browser_url(url))
                .ok_or_else(|| "only http(s) URLs can be opened".to_string())?;
            run(tool, json!({ "url": url }))
        }
        "browser_fill_login" => agent_fill(app, session_id, &args),
        "browser_back" | "browser_forward" | "browser_reload" => run(tool, json!({})),
        "browser_device" => {
            let device = if args.get("fill").and_then(Value::as_bool) == Some(true) {
                Value::Null
            } else {
                let size = |key: &str| {
                    args.get(key)
                        .and_then(Value::as_u64)
                        .map(|value| value.min(10_000) as u32)
                };
                let device = crate::browser::device_from(
                    args.get("preset").and_then(Value::as_str),
                    size("width"),
                    size("height"),
                    args.get("orientation").and_then(Value::as_str) == Some("landscape"),
                )
                .map_err(|error| error.to_string())?;
                serde_json::to_value(device).unwrap_or(Value::Null)
            };
            let state = run(tool, json!({ "device": device }))?;
            Ok(json!({ "device": state["device"], "note": SCREENSHOT_NOTE }))
        }
        "browser_record_start" | "browser_record_stop" => {
            let stop = tool == "browser_record_stop";
            call_with_timeout(
                session_id,
                "browser_record",
                json!({ "action": if stop { "stop" } else { "start" } }),
                if stop { RECORD_STOP_TIMEOUT } else { TIMEOUT },
            )
        }
        "browser_close" => run(tool, json!({})).map(|_| json!({ "closed": true })),
        // Only the model receives the image: an agent's screenshots never land in the
        // person's composer (the pane's camera button does that).
        "browser_screenshot" => {
            let value = run(tool, json!({}))?;
            let data = value
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| "the window returned no image".to_string())?;
            Ok(json!({ "image": { "mimeType": "image/png", "data": data } }))
        }
        "browser_snapshot" => run(tool, json!({})),
        "browser_click" => {
            let (reference, selector) = element_arg(&args)?;
            run(tool, json!({ "ref": reference, "selector": selector })).and_then(action_value)
        }
        "browser_type" => {
            let (reference, selector) = element_arg(&args)?;
            let value = arg_str(&args, "text", 16 * 1024).unwrap_or_default();
            let submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
            run(
                tool,
                json!({ "ref": reference, "selector": selector, "text": value, "submit": submit }),
            )
            .and_then(action_value)
        }
        "browser_press" => {
            let mut key = arg_str(&args, "key", 16).unwrap_or_default();
            if key.is_empty() || key.eq_ignore_ascii_case("space") {
                key = " ".into();
            }
            if !crate::browser::is_allowed_browser_key(&key) {
                return Err(format!(
                    "key must be one of: {}",
                    crate::browser::BROWSER_KEYS.join(", ")
                ));
            }
            run(tool, json!({ "key": key })).and_then(action_value)
        }
        "browser_wait_for" => {
            let text = arg_str(&args, "text", 500).unwrap_or_default();
            let selector = arg_str(&args, "selector", 300).unwrap_or_default();
            if !valid_selector(&selector) {
                return Err("invalid selector".into());
            }
            if text.is_empty() && selector.is_empty() {
                return Err("provide text or a selector".into());
            }
            let timeout = args
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(5000)
                .clamp(100, 8000);
            run(
                tool,
                json!({ "text": text, "selector": selector, "timeoutMs": timeout }),
            )
            .and_then(action_value)
        }
        "browser_scroll" => {
            let dy = args.get("dy").and_then(Value::as_f64).unwrap_or(0.0);
            if !dy.is_finite() || dy.abs() > 20_000.0 {
                return Err("invalid scroll amount".into());
            }
            run(tool, json!({ "dy": dy })).and_then(action_value)
        }
        "browser_logs" => {
            let clear = args.get("clear").and_then(Value::as_bool).unwrap_or(false);
            run(tool, json!({ "clear": clear }))
        }
        "browser_hover" => {
            let (reference, selector) = element_arg(&args)?;
            run(tool, json!({ "ref": reference, "selector": selector })).and_then(action_value)
        }
        "browser_select" => {
            let (reference, selector) = element_arg(&args)?;
            let values = crate::browser_agent::select_values(&args)?;
            run(
                tool,
                json!({ "ref": reference, "selector": selector, "values": values }),
            )
            .and_then(action_value)
        }
        "browser_drag" => {
            let from = crate::browser_agent::parse_drag_end(args.get("from"), "from")?;
            let to = crate::browser_agent::parse_drag_end(args.get("to"), "to")?;
            run(tool, json!({ "from": from.to_json(), "to": to.to_json() })).and_then(action_value)
        }
        "browser_upload" => {
            let (reference, selector) = element_arg(&args)?;
            let paths: Vec<String> = args
                .get("paths")
                .and_then(Value::as_array)
                .ok_or_else(|| "`paths` must list files".to_string())?
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| "`paths` must be strings".to_string())
                })
                .collect::<Result<_, _>>()?;
            let (workspace, attachments) = crate::browser_mcp::upload_roots(app, session_id);
            let files = crate::browser_agent::validate_upload_paths(
                &paths,
                workspace.as_deref(),
                &attachments,
            )?;
            let files = encode_files(&files)?;
            run(
                tool,
                json!({ "ref": reference, "selector": selector, "files": files }),
            )
            .and_then(action_value)
        }
        "browser_dialog" => {
            let accept = match args.get("action").and_then(Value::as_str) {
                Some("accept") => true,
                Some("dismiss") => false,
                _ => return Err("`action` must be accept or dismiss".into()),
            };
            let text =
                match args.get("text") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(arg_str(&args, "text", 16 * 1024).ok_or_else(|| {
                        "`text` is too long or has control characters".to_string()
                    })?),
                };
            let answer = run(tool, json!({ "accept": accept, "text": text }))?;
            Ok(json!({ "answered": answer["answered"], "status": answer["status"] }))
        }
        "browser_evaluate" => {
            let source = args
                .get("expression")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|source| {
                    !source.is_empty()
                        && source.chars().count() <= crate::browser_agent::EVALUATE_MAX_SOURCE
                })
                .ok_or_else(|| {
                    format!(
                        "`expression` is required (at most {} characters)",
                        crate::browser_agent::EVALUATE_MAX_SOURCE
                    )
                })?;
            evaluate(session_id, source)
        }
        "browser_network" => {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(50)
                .clamp(1, 200) as usize;
            let clear = args.get("clear").and_then(Value::as_bool).unwrap_or(false);
            let parsed = run(tool, json!({ "limit": limit, "clear": clear }))?;
            Ok(json!({ "entries": crate::browser_agent::network_entries(&parsed, limit) }))
        }
        "browser_resize" => {
            let (width, height) = crate::browser_agent::resize_size(&args)?;
            let device =
                crate::browser::device_from(None, Some(width), Some(height), width > height)
                    .map_err(|error| error.to_string())?;
            let state = run(
                "browser_device",
                json!({ "device": serde_json::to_value(device).unwrap_or(Value::Null) }),
            )?;
            Ok(json!({ "device": state["device"], "note": RESIZE_NOTE }))
        }
        "browser_color_scheme" => {
            let scheme = crate::browser_agent::color_scheme(&args)?;
            let state = run(tool, json!({ "scheme": scheme }))?;
            Ok(json!({ "appearance": state["appearance"] }))
        }
        _ => Err("unknown browser tool".into()),
    }
}

/// Runs the agent's JavaScript in the page; a syntax error as an expression retries it as a
/// function body (statements with `return`).
fn evaluate(session_id: &str, source: &str) -> Result<Value, String> {
    let mut last = String::new();
    for statements in [false, true] {
        let raw = call(
            session_id,
            "page_script",
            json!({ "body": crate::browser_agent::evaluate_body(source, statements), "args": {} }),
        )?;
        let parsed = match raw {
            Value::String(text) => serde_json::from_str(&text).unwrap_or(Value::Null),
            other => other,
        };
        if parsed.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(parsed);
        }
        last = parsed
            .get("error")
            .and_then(Value::as_str)
            .map(|text| crate::browser_agent::truncate(text, 600))
            .unwrap_or_else(|| "the script could not run".into());
        if !last.to_ascii_lowercase().contains("syntax") {
            break;
        }
    }
    Err(last)
}

/// The upload script's `files` argument: name, MIME type (`type`) and base64 `data`.
fn encode_files(files: &[std::path::PathBuf]) -> Result<Vec<Value>, String> {
    use base64::Engine as _;
    files
        .iter()
        .map(|path| {
            let bytes = read_bounded(path)?;
            Ok(json!({
                "name": path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_else(|| "file".into()),
                "type": crate::browser_agent::upload_mime(path),
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }))
        })
        .collect()
}

/// Reads a checked file without following a link swapped in since the check.
fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "Cannot read the file to upload.".to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Only regular files can be uploaded.".into());
    }
    let mut bytes = Vec::new();
    file.take(crate::browser_agent::UPLOAD_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > crate::browser_agent::UPLOAD_MAX_BYTES {
        return Err("Files may total at most 10 MB.".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// The registry is process-wide, so these tests take turns.
    static TURN: StdMutex<()> = StdMutex::new(());

    fn clear() {
        *CLIENT.lock() = None;
        WAITING.lock().take();
    }

    /// A fake window: answers each request through `answer`.
    fn window(
        connection: u64,
        answer: impl Fn(&Value) -> Option<(bool, Value)> + Send + 'static,
    ) -> std::thread::JoinHandle<()> {
        let (frames, mut inbox) = tokio::sync::mpsc::channel::<String>(8);
        register(connection, frames);
        std::thread::spawn(move || {
            while let Some(frame) = inbox.blocking_recv() {
                let frame: Value = serde_json::from_str(&frame).unwrap();
                assert_eq!(frame["event"], REQUEST_EVENT);
                let payload = &frame["payload"];
                if let Some((ok, value)) = answer(payload) {
                    let id = payload["id"].as_str().unwrap();
                    let result = if ok {
                        Ok(value)
                    } else {
                        Err(value.as_str().unwrap().to_string())
                    };
                    deliver(connection, id, result);
                }
            }
        })
    }

    #[test]
    fn without_a_window_the_tools_keep_todays_error() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let dir = tempfile::tempdir().unwrap();
        let app = AppHandle::new(dir.path().to_path_buf());
        assert_eq!(
            browser_tool(
                &app,
                "s",
                "browser_navigate",
                json!({ "url": "https://a.test" })
            ),
            Err("the embedded browser is only available in the Mac app".into())
        );
        assert!(!window_expected() || std::env::var(CLIENT_ENV).is_ok());
    }

    #[test]
    fn a_registered_client_answers_calls_and_only_it() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let handle = window(7, |request| {
            assert_eq!(request["sessionId"], "s1");
            Some((
                true,
                json!({ "echo": request["tool"], "args": request["args"] }),
            ))
        });
        assert!(has_client());
        let value = call("s1", "browser_back", json!({ "x": 1 })).unwrap();
        assert_eq!(value["echo"], "browser_back");
        assert_eq!(value["args"]["x"], 1);
        // An answer from another connection, or for an unknown call, is ignored.
        assert!(!deliver(8, "nope", Ok(Value::Null)));
        assert!(!deliver(7, "nope", Ok(Value::Null)));
        unregister(7);
        assert!(!has_client());
        handle.join().unwrap();
        assert_eq!(call("s1", "browser_back", json!({})), Err(NO_WINDOW.into()));
    }

    #[test]
    fn an_answer_from_the_wrong_connection_does_not_complete_a_call() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let (frames, mut inbox) = tokio::sync::mpsc::channel::<String>(8);
        register(1, frames);
        let thief = std::thread::spawn(move || {
            let frame: Value = serde_json::from_str(&inbox.blocking_recv().unwrap()).unwrap();
            let id = frame["payload"]["id"].as_str().unwrap().to_string();
            assert!(!deliver(2, &id, Ok(json!("stolen"))));
            assert!(deliver(1, &id, Ok(json!("real"))));
        });
        assert_eq!(call("s", "browser_status", json!({})), Ok(json!("real")));
        thief.join().unwrap();
        unregister(1);
    }

    #[test]
    fn a_silent_window_times_out() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let _quiet = window(3, |_| None);
        let started = std::time::Instant::now();
        let error = call_with_timeout("s", "browser_status", json!({}), Duration::from_millis(150))
            .unwrap_err();
        assert_eq!(error, "the Sirus window did not answer");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(WAITING.lock().as_ref().is_none_or(HashMap::is_empty));
        unregister(3);
    }

    #[test]
    fn disconnecting_fails_pending_calls_at_once() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let _quiet = window(4, |_| None);
        let dropper = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(100));
            unregister(4);
        });
        let started = std::time::Instant::now();
        let error = call("s", "browser_status", json!({})).unwrap_err();
        assert_eq!(error, "the Sirus window disconnected");
        assert!(started.elapsed() < Duration::from_secs(5));
        dropper.join().unwrap();
    }

    #[test]
    fn a_newer_registration_replaces_the_older_one() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let _old = window(5, |_| None);
        let waiter = std::thread::spawn(|| call("s", "browser_status", json!({})));
        std::thread::sleep(Duration::from_millis(100));
        let _new = window(6, |_| Some((true, json!("new"))));
        assert_eq!(
            waiter.join().unwrap(),
            Err("the Sirus window was replaced".into())
        );
        assert_eq!(call("s", "browser_status", json!({})), Ok(json!("new")));
        // The old connection closing later does not unregister the new one.
        unregister(5);
        assert!(has_client());
        unregister(6);
    }

    #[test]
    fn only_the_native_token_is_the_local_window() {
        assert!(token_is_local("abc123\n", "abc123"));
        assert!(!token_is_local("abc123", "abc124"));
        assert!(!token_is_local("abc123", ""));
        assert!(!token_is_local("", ""));
        assert!(!token_is_local("   \n", "   "));
    }

    #[test]
    fn local_commands_are_named_and_not_for_devices() {
        for command in [
            "register_browser_client",
            "browser_tool_result",
            "login_vault_list",
            "login_vault_autofill",
            "computer_action",
        ] {
            assert!(is_local_command(command), "{command}");
        }
        assert!(!is_local_command("load_state"));
        assert!(!is_local_command("browser_open"));
    }

    #[test]
    fn a_desktop_on_a_headless_host_may_only_be_the_browser_client() {
        assert_eq!(client_kind(true, false, false), Client::Window);
        assert_eq!(client_kind(true, true, true), Client::Window);
        assert_eq!(client_kind(false, true, true), Client::Desktop);
        // A host with a window of its own keeps its browser there.
        assert_eq!(client_kind(false, true, false), Client::Device);
        // Phones are never the client, headless or not.
        assert_eq!(client_kind(false, false, true), Client::Device);
        for command in CONNECTION_COMMANDS {
            assert!(command_allowed(command, Client::Window));
            assert!(command_allowed(command, Client::Desktop));
            assert!(!command_allowed(command, Client::Device));
        }
        for command in [
            "login_vault_list",
            "login_vault_autofill",
            "computer_action",
            "remote_action",
        ] {
            assert!(command_allowed(command, Client::Window), "{command}");
            assert!(!command_allowed(command, Client::Desktop), "{command}");
            assert!(!command_allowed(command, Client::Device), "{command}");
        }
        assert!(!command_allowed("load_state", Client::Window));
    }

    #[test]
    fn tool_arguments_are_validated_before_the_window_sees_them() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let dir = tempfile::tempdir().unwrap();
        let app = AppHandle::new(dir.path().to_path_buf());
        let _quiet = window(9, |request| {
            panic!("the window must not be asked: {request}");
        });
        for (tool, args) in [
            ("browser_navigate", json!({ "url": "file:///etc/passwd" })),
            ("browser_click", json!({})),
            ("browser_click", json!({ "ref": "x1" })),
            ("browser_press", json!({ "key": "Meta+Q" })),
            ("browser_wait_for", json!({})),
            ("browser_scroll", json!({ "dy": 1e9 })),
            ("browser_dialog", json!({ "action": "maybe" })),
            ("browser_evaluate", json!({ "expression": "  " })),
            ("browser_upload", json!({ "ref": "e1" })),
            ("browser_nope", json!({})),
        ] {
            assert!(browser_tool(&app, "s", tool, args).is_err(), "{tool}");
        }
        unregister(9);
    }

    #[test]
    fn results_are_shaped_like_the_mac_app_shapes_them() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        let dir = tempfile::tempdir().unwrap();
        let app = AppHandle::new(dir.path().to_path_buf());
        let _window = window(10, |request| {
            let value = match request["tool"].as_str().unwrap() {
                "browser_screenshot" => json!({ "data": "QUJD" }),
                "browser_close" => json!({ "open": false }),
                "browser_click" if request["args"]["ref"] == "e9" => {
                    json!({ "ok": false, "error": "element not found" })
                }
                "browser_click" => json!({ "ok": true }),
                "browser_device" => json!({ "device": request["args"]["device"] }),
                "browser_color_scheme" => json!({ "appearance": request["args"]["scheme"] }),
                "browser_dialog" => json!({ "answered": true, "status": { "open": true } }),
                "page_script" => json!("{\"ok\":true,\"type\":\"number\",\"json\":\"2\"}"),
                other => panic!("unexpected {other}"),
            };
            Some((true, value))
        });
        let shot = browser_tool(&app, "s", "browser_screenshot", json!({})).unwrap();
        assert_eq!(shot["image"]["mimeType"], "image/png");
        assert_eq!(shot["image"]["data"], "QUJD");
        assert_eq!(
            browser_tool(&app, "s", "browser_close", json!({})).unwrap(),
            json!({ "closed": true })
        );
        assert!(browser_tool(&app, "s", "browser_click", json!({ "ref": "e1" })).is_ok());
        assert_eq!(
            browser_tool(&app, "s", "browser_click", json!({ "ref": "e9" })),
            Err("element not found".into())
        );
        let resized = browser_tool(
            &app,
            "s",
            "browser_resize",
            json!({ "width": 390, "height": 844 }),
        )
        .unwrap();
        assert_eq!(resized["device"]["width"], 390);
        assert!(resized["note"].as_str().unwrap().contains("browser_device"));
        assert_eq!(
            browser_tool(
                &app,
                "s",
                "browser_color_scheme",
                json!({ "scheme": "dark" })
            )
            .unwrap(),
            json!({ "appearance": "dark" })
        );
        assert_eq!(
            browser_tool(&app, "s", "browser_dialog", json!({ "action": "accept" })).unwrap(),
            json!({ "answered": true, "status": { "open": true } })
        );
        let evaluated = browser_tool(
            &app,
            "s",
            "browser_evaluate",
            json!({ "expression": "1+1" }),
        )
        .unwrap();
        assert_eq!(evaluated["json"], "2");
        unregister(10);
    }
}
