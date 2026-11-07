//! Browser MCP: the app hosts the browser; each provider CLI spawns this same
//! binary with `--mcp-browser`, which speaks MCP over stdio and forwards tool
//! calls to the app over a private Unix socket with a per-session token.
//! The same bridge carries the session tools (`sirus_*`, secrets, PR watch) on
//! every platform; only the browser tools need the Mac app (ADR-131).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde_json::{json, Value};

pub const SOCKET_ENV: &str = "SIRUS_BROWSER_SOCKET";
pub const TOKEN_ENV: &str = "SIRUS_BROWSER_TOKEN";
/// Set to `0` for the MCP child when agents may not use the browser (ADR-130).
const BROWSER_TOOLS_ENV: &str = "SIRUS_BROWSER_TOOLS";
pub(crate) const MCP_SERVER_NAME: &str = "sirus_browser";

/// Settings → Agent browser access (ADR-130). On by default; off, the `browser_*` tools are
/// neither listed to new agents nor run for kept ones. Session tools, secrets and PR watch
/// share this server and stay.
static AGENT_ACCESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_agent_access(value: bool) {
    AGENT_ACCESS.store(value, std::sync::atomic::Ordering::SeqCst);
}

fn agent_access() -> bool {
    AGENT_ACCESS.load(std::sync::atomic::Ordering::SeqCst)
}

/// Variables for the MCP child: whether it lists the session tools and the browser tools.
pub fn child_env() -> Vec<(&'static str, &'static str)> {
    let mut env: Vec<_> = crate::sirus_tools::child_env().into_iter().collect();
    if !agent_access() {
        env.push((BROWSER_TOOLS_ENV, "0"));
    }
    if crate::browser_bridge::window_expected() {
        env.push((crate::browser_bridge::WINDOW_ENV, "1"));
    }
    env
}

#[derive(Clone)]
pub struct Endpoint {
    pub token: String,
    pub socket: PathBuf,
}

static ENDPOINTS: Mutex<Option<HashMap<String, Endpoint>>> = Mutex::new(None);
static APP: OnceLock<(crate::runtime::AppHandle, PathBuf)> = OnceLock::new();

fn endpoint_map() -> parking_lot::MutexGuard<'static, Option<HashMap<String, Endpoint>>> {
    ENDPOINTS.lock()
}

pub(crate) fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Registers the app handle and data directory, then starts the socket server.
pub fn init(app: crate::runtime::AppHandle, data_dir: PathBuf) {
    let dir = data_dir.join("browser-mcp");
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
    #[cfg(unix)]
    crate::mcp_http::start();
}

pub(crate) fn socket_path() -> Option<PathBuf> {
    APP.get().map(|(_, socket)| socket.clone())
}

pub(crate) fn token_known(token: &str) -> bool {
    endpoint_map().as_ref().is_some_and(|map| {
        map.values()
            .any(|endpoint| constant_time_eq(endpoint.token.as_bytes(), token.as_bytes()))
    })
}

/// The server as the host's HTTP route answers it: what it lists follows the host's own state,
/// since there is no child started with a snapshot of it in its environment.
pub(crate) fn http_server() -> crate::mcp_stdio::Server {
    crate::mcp_stdio::Server {
        name: MCP_SERVER_NAME,
        socket_env: SOCKET_ENV,
        token_env: TOKEN_ENV,
        tools: live_tool_definitions,
        failure: "the browser operation failed",
        instructions: Some(crate::secrets::INSTRUCTIONS),
    }
}

fn live_tool_definitions() -> Vec<Value> {
    let browser = agent_access() && crate::browser_bridge::window_expected();
    browser_tool_definitions()
        .into_iter()
        .filter(|_| browser)
        .chain(crate::sirus_tools::live_tool_definitions())
        .chain(crate::secrets::tool_definitions())
        .chain(crate::pr_watch::tool_definitions())
        .collect()
}

/// The app handle once the bridge is initialised (never in unit tests).
pub(crate) fn app_handle() -> Option<crate::runtime::AppHandle> {
    APP.get().map(|(app, _)| app.clone())
}

/// Returns (creating once) the endpoint bound to a session.
pub fn ensure_endpoint(session_id: &str) -> Option<Endpoint> {
    let (_, socket) = APP.get()?;
    let mut guard = endpoint_map();
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(endpoint) = map.get(session_id) {
        return Some(endpoint.clone());
    }
    let endpoint = Endpoint {
        token: new_token(),
        socket: socket.clone(),
    };
    map.insert(session_id.to_string(), endpoint.clone());
    Some(endpoint)
}

/// Claude reads a JSON config file; the token travels in the file (0600).
pub fn claude_mcp_config(
    session_id: &str,
    connectors: serde_json::Map<String, Value>,
) -> Option<PathBuf> {
    let endpoint = ensure_endpoint(session_id)?;
    let (_, socket) = APP.get()?;
    let exe = std::env::current_exe().ok()?;
    let dir = socket.parent()?;
    let path = dir.join(format!("{session_id}.claude.json"));
    let mut config = json!({
        "mcpServers": {
            MCP_SERVER_NAME: {
                "command": exe.to_string_lossy(),
                "args": ["--mcp-browser"],
                "env": {
                    SOCKET_ENV: endpoint.socket.to_string_lossy(),
                    TOKEN_ENV: endpoint.token,
                }
            }
        }
    });
    for (key, value) in child_env() {
        config["mcpServers"][MCP_SERVER_NAME]["env"][key] = json!(value);
    }
    #[cfg(unix)]
    if let Some(entry) = crate::mcp_http::claude_entry(MCP_SERVER_NAME, &endpoint.token) {
        config["mcpServers"][MCP_SERVER_NAME] = entry;
    }
    if let Some(computer) = crate::computer_mcp::claude_server(session_id) {
        config["mcpServers"][crate::computer_mcp::SERVER_NAME] = computer;
    }
    // Connectors (ADR-137): their tokens make this file a secret, so it is created private.
    for (name, server) in connectors {
        config["mcpServers"][name] = server;
    }
    let bytes = serde_json::to_vec(&config).ok()?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .ok()?;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        file.write_all(&bytes).ok()?;
    }
    #[cfg(not(unix))]
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

#[cfg(unix)]
fn start_server(app: crate::runtime::AppHandle, socket: PathBuf) {
    let _ = std::fs::remove_file(&socket);
    crate::runtime::async_runtime::spawn(async move {
        let listener = match tokio::net::UnixListener::bind(&socket) {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(%error, "browser mcp: cannot bind bridge socket");
                return;
            }
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));
        }
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let app = app.clone();
            crate::runtime::async_runtime::spawn(async move {
                handle_bridge_connection(stream, app).await;
            });
        }
    });
}

#[cfg(not(unix))]
fn start_server(_app: crate::runtime::AppHandle, _socket: PathBuf) {}

#[cfg(unix)]
async fn handle_bridge_connection(stream: tokio::net::UnixStream, app: crate::runtime::AppHandle) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        // Room for a 512 KiB inline visual after JSON escaping (ADR-123).
        if line.len() > 4 * 1024 * 1024 {
            break;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let response = bridge_request(&app, &request).await;
        let Ok(encoded) = serde_json::to_string(&response) else {
            continue;
        };
        if write.write_all(encoded.as_bytes()).await.is_err() {
            break;
        }
        if write.write_all(b"\n").await.is_err() {
            break;
        }
    }
}

#[cfg(unix)]
async fn bridge_request(app: &crate::runtime::AppHandle, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(token) = request.get("token").and_then(Value::as_str) else {
        return json!({ "id": id, "ok": false, "error": "missing token" });
    };
    let session_id = {
        let guard = endpoint_map();
        guard.as_ref().and_then(|map| {
            map.iter()
                .find(|(_, endpoint)| constant_time_eq(endpoint.token.as_bytes(), token.as_bytes()))
                .map(|(session_id, _)| session_id.clone())
        })
    };
    let Some(session_id) = session_id else {
        return json!({ "id": id, "ok": false, "error": "unknown browser session" });
    };
    let tool = request
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let arguments = request
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let owned_app = app.clone();
    let result = tokio::task::spawn_blocking(move || {
        let result = execute(&owned_app, &session_id, &tool, arguments);
        if let Ok(value) = &result {
            crate::tool_images::remember(&session_id, &tool, value);
        }
        result
    })
    .await
    .unwrap_or_else(|_| Err("the browser operation did not complete".into()));
    match result {
        Ok(value) => json!({ "id": id, "ok": true, "result": value }),
        Err(error) => json!({ "id": id, "ok": false, "error": error }),
    }
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

pub(crate) fn session_owned(app: &crate::runtime::AppHandle, session_id: &str) -> bool {
    use crate::runtime::Manager;
    let state = app.state::<std::sync::Arc<crate::commands::AppState>>();
    let owned = state
        .data
        .lock()
        .sessions
        .iter()
        .any(|session| session.id == session_id);
    owned
}

pub(crate) fn arg_str(args: &Value, key: &str, limit: usize) -> Option<String> {
    let value = args.get(key)?.as_str()?.trim();
    if value.chars().count() > limit || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_string())
}

pub(crate) fn valid_ref(value: &str) -> bool {
    value.is_empty()
        || ((2..=5).contains(&value.len())
            && value.starts_with('e')
            && value[1..].chars().all(|c| c.is_ascii_digit()))
}

pub(crate) fn valid_selector(value: &str) -> bool {
    value.chars().count() <= 300 && !value.chars().any(char::is_control)
}

fn action_result(raw: String) -> Result<Value, String> {
    let parsed: Value =
        serde_json::from_str(&raw).map_err(|_| "invalid page response".to_string())?;
    action_value(parsed)
}

/// The same check for a page result the window already decoded (the window bridge).
pub(crate) fn action_value(parsed: Value) -> Result<Value, String> {
    if parsed.get("interrupted").and_then(Value::as_bool) == Some(true) {
        return Err("the page was used by the person and the action was interrupted".into());
    }
    if parsed.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(parsed
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("the action failed")
            .to_string());
    }
    Ok(parsed)
}

fn execute(
    app: &crate::runtime::AppHandle,
    session_id: &str,
    tool: &str,
    args: Value,
) -> Result<Value, String> {
    if !session_owned(app, session_id) {
        return Err("this session no longer exists".into());
    }
    if tool.starts_with("sirus_") || crate::visuals::NATIVE_TOOLS.contains(&tool) {
        // Session tools for every session, behind the Settings switch (ADR-076).
        return crate::sirus_tools::execute(app, session_id, tool, &args);
    }
    if matches!(tool, "request_secret" | "use_secret") {
        // Private secret requests (ADR-077); blocks while the person answers.
        return crate::secrets::execute(app, session_id, tool, &args);
    }
    if matches!(tool, "watch_pull_request" | "unwatch_pull_request") {
        // PR watch (ADR-079): only ever for the session that owns this token.
        return crate::pr_watch::agent_tool(app, session_id, tool);
    }
    if tool.starts_with("browser_") && !agent_access() {
        return Err("Agent browser access is off in Sirus's settings.".into());
    }
    browser_tool(app, session_id, tool, args)
}

/// The embedded browser lives in a window: the Mac app's own, or a window client attached to
/// the host (`browser_bridge.rs`, ADR-189). With neither, the tools are refused.
fn browser_tool(
    app: &crate::runtime::AppHandle,
    session_id: &str,
    tool: &str,
    args: Value,
) -> Result<Value, String> {
    crate::browser_bridge::browser_tool(app, session_id, tool, args)
}

/// The element an agent names: a snapshot ref or a CSS selector, never empty.
pub(crate) fn element_arg(args: &Value) -> Result<(String, String), String> {
    let reference = arg_str(args, "ref", 8).unwrap_or_default();
    let selector = arg_str(args, "selector", 300).unwrap_or_default();
    if !valid_ref(&reference) || !valid_selector(&selector) {
        return Err("invalid element reference".into());
    }
    if reference.is_empty() && selector.is_empty() {
        return Err("provide a ref from browser_snapshot or a selector".into());
    }
    Ok((reference, selector))
}

/// The session's workspace and its attachment snapshots: the only places an upload may read.
pub(crate) fn upload_roots(
    app: &crate::runtime::AppHandle,
    session_id: &str,
) -> (Option<PathBuf>, Vec<PathBuf>) {
    use crate::runtime::Manager;
    let state = app.state::<std::sync::Arc<crate::commands::AppState>>();
    let workspace = {
        let data = state.data.lock();
        data.sessions
            .iter()
            .find(|session| session.id == session_id)
            .and_then(|session| crate::commands::session_cwd(&data, session).ok())
    };
    (
        workspace,
        state
            .attachments
            .paths_for(&format!("session:{session_id}")),
    )
}

fn tool_definitions() -> Vec<Value> {
    // The host lists the browser tools only when a window client can run them (ADR-189).
    let browser = std::env::var(BROWSER_TOOLS_ENV).as_deref() != Ok("0")
        && std::env::var(crate::browser_bridge::WINDOW_ENV).as_deref() == Ok("1");
    browser_tool_definitions()
        .into_iter()
        .filter(|_| browser)
        .chain(crate::sirus_tools::tool_definitions())
        .chain(crate::secrets::tool_definitions())
        .chain(crate::pr_watch::tool_definitions())
        .collect()
}

fn browser_tool_definitions() -> Vec<Value> {
    let element = json!({
        "type": "object",
        "properties": {
            "ref": { "type": "string", "description": "Element ref from browser_snapshot (preferred)" },
            "selector": { "type": "string", "description": "CSS selector fallback" }
        }
    });
    vec![
        json!({ "name": "browser_status", "description": "Current browser session: open state, tabs, active tab, and any JavaScript alert, confirm or prompt the page is waiting on (answer it with browser_dialog).", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "browser_tabs", "description": "List open browser tabs.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "browser_open", "description": "Open the session browser (creates a tab) and optionally navigate to an http(s) URL.", "inputSchema": { "type": "object", "properties": { "url": { "type": "string" } } } }),
        json!({ "name": "browser_navigate", "description": "Navigate the active tab to an http(s) URL.", "inputSchema": { "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] } }),
        json!({ "name": "browser_back", "description": "Go back in the active tab.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "browser_forward", "description": "Go forward in the active tab.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "browser_reload", "description": "Reload the active tab.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "browser_screenshot", "description": "PNG screenshot of the visible page.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "browser_snapshot", "description": "Bounded list of interactive elements with refs (use the refs for click/type).", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "browser_click", "description": "Click an element by ref or selector. Interrupted if the person is using the page.", "inputSchema": element.clone() }),
        json!({ "name": "browser_type", "description": "Type into an element by ref or selector, optional submit.", "inputSchema": { "type": "object", "properties": { "ref": { "type": "string" }, "selector": { "type": "string" }, "text": { "type": "string" }, "submit": { "type": "boolean" } } } }),
        json!({ "name": "browser_scroll", "description": "Scroll the page by dy pixels.", "inputSchema": { "type": "object", "properties": { "dy": { "type": "number" } }, "required": ["dy"] } }),
        json!({ "name": "browser_press", "description": "Press a key on the focused element (Enter submits its form). Keys: Enter, Escape, Tab, Backspace, Delete, arrows, PageUp, PageDown, Home, End, space.", "inputSchema": { "type": "object", "properties": { "key": { "type": "string" } }, "required": ["key"] } }),
        json!({ "name": "browser_wait_for", "description": "Wait until text appears on the page or a selector matches (at most 8 s).", "inputSchema": { "type": "object", "properties": { "text": { "type": "string" }, "selector": { "type": "string" }, "timeoutMs": { "type": "number" } } } }),
        json!({ "name": "browser_logs", "description": "Bounded console/error log buffer; optionally clear it.", "inputSchema": { "type": "object", "properties": { "clear": { "type": "boolean" } } } }),
        json!({ "name": "browser_close", "description": "Close all tabs of the session browser.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "browser_device", "description": "Responsive device mode: pages lay out at a phone or tablet CSS size, so you can check mobile layouts (screenshots show it). Use a preset (iphone-se, iphone-xr, iphone-12-pro, iphone-14-pro-max, pixel-7, galaxy-s8-plus, galaxy-s20-ultra, ipad-mini, ipad-air, ipad-pro, surface-pro-7, surface-duo, galaxy-z-fold-5, zenbook-fold, galaxy-a51-71, nest-hub, nest-hub-max) or width and height (240–3840), with orientation portrait or landscape; fill: true goes back to the normal size. It changes CSS breakpoints, not the user agent. The person sees the same device in their browser pane.", "inputSchema": { "type": "object", "properties": { "preset": { "type": "string" }, "width": { "type": "integer" }, "height": { "type": "integer" }, "orientation": { "type": "string", "enum": ["portrait", "landscape"] }, "fill": { "type": "boolean" } } } }),
        json!({ "name": "browser_record_start", "description": "Start recording the active tab to a video, for example to show the person a flow you tested. Clicks and key presses appear in it when the person's settings say so. Stop with browser_record_stop; recordings end by themselves after 10 minutes.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "browser_record_stop", "description": "Stop the recording and save it; returns the video file's path (in Movies/Sirus) and shows it to the person.", "inputSchema": { "type": "object", "properties": {} } }),
        json!({ "name": "browser_hover", "description": "Hover an element by ref or selector (pointer and mouse events), to reveal menus and tooltips.", "inputSchema": element.clone() }),
        json!({ "name": "browser_select", "description": "Choose options in a native select, or in a listbox or combobox with role=option items, by option text or value. Errors list the available options.", "inputSchema": { "type": "object", "properties": { "ref": { "type": "string" }, "selector": { "type": "string" }, "values": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 20 } }, "required": ["values"] } }),
        json!({ "name": "browser_drag", "description": "Drag from one element or point to another. Each end is a ref, a selector, or {x, y} in CSS pixels of the visible page. Works with pointer events and HTML5 drag and drop.", "inputSchema": { "type": "object", "properties": { "from": { "type": "object" }, "to": { "type": "object" } }, "required": ["from", "to"] } }),
        json!({ "name": "browser_upload", "description": "Attach up to 5 files (10 MB in total) to a file input, or drop them on a drop area. Paths may be relative to the conversation's workspace or the absolute path of one of the conversation's attachments; anything else is refused.", "inputSchema": { "type": "object", "properties": { "ref": { "type": "string" }, "selector": { "type": "string" }, "paths": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 5 } }, "required": ["paths"] } }),
        json!({ "name": "browser_dialog", "description": "Answer the JavaScript alert, confirm or prompt that the page is waiting on (browser_status shows it under dialog). accept confirms or submits (with text for a prompt); dismiss cancels.", "inputSchema": { "type": "object", "properties": { "action": { "type": "string", "enum": ["accept", "dismiss"] }, "text": { "type": "string" } }, "required": ["action"] } }),
        json!({ "name": "browser_evaluate", "description": "Run JavaScript in the active page and return its JSON value (bounded to 32 KB). Runs with the page's own permissions and can read what the page shows; it is available only while Agent browser access is on in Settings. Use an expression, or statements that end with return.", "inputSchema": { "type": "object", "properties": { "expression": { "type": "string" } }, "required": ["expression"] } }),
        json!({ "name": "browser_network", "description": "Recent requests of the current page since it loaded (fetch and XHR calls, plus resource timings): method, URL without query values, status, type, duration. Responses and bodies are not recorded; some requests made before the page's scripts ran may be missing.", "inputSchema": { "type": "object", "properties": { "limit": { "type": "number" }, "clear": { "type": "boolean" } } } }),
        json!({ "name": "browser_resize", "description": "Set the page's viewport to width by height CSS pixels (240–3840), like device mode with a custom size. Use browser_device with fill: true to go back to the pane's size.", "inputSchema": { "type": "object", "properties": { "width": { "type": "integer" }, "height": { "type": "integer" } }, "required": ["width", "height"] } }),
        crate::browser_login::tool_definition(),
        json!({ "name": "browser_color_scheme", "description": "Set the colour scheme pages see (prefers-color-scheme): light, dark, or system to follow the app.", "inputSchema": { "type": "object", "properties": { "scheme": { "type": "string", "enum": ["light", "dark", "system"] } }, "required": ["scheme"] } }),
    ]
}

/// Child mode: MCP over stdio, tool calls forwarded to the app socket.
pub fn run_stdio() -> i32 {
    crate::mcp_stdio::run(&crate::mcp_stdio::Server {
        name: MCP_SERVER_NAME,
        socket_env: SOCKET_ENV,
        token_env: TOKEN_ENV,
        tools: tool_definitions,
        failure: "the browser operation failed",
        instructions: Some(crate::secrets::INSTRUCTIONS),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_are_well_formed() {
        let tools = tool_definitions();
        assert!(tools.len() >= 3);
        for tool in &tools {
            assert!(tool["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("browser_")
                    || name.starts_with("sirus_")
                    || name.starts_with("visual_")
                    || name.ends_with("_secret")
                    || name.ends_with("_pull_request")));
            assert!(tool["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty()));
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn agent_browser_access_off_hides_only_the_browser_tools() {
        let browser = browser_tool_definitions();
        assert!(browser.iter().all(|tool| {
            tool["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("browser_"))
        }));
        set_agent_access(false);
        assert!(child_env().contains(&(BROWSER_TOOLS_ENV, "0")));
        set_agent_access(true);
        assert!(!child_env().iter().any(|(key, _)| *key == BROWSER_TOOLS_ENV));
    }

    #[test]
    fn action_results_map_interruption_and_failures() {
        assert!(action_result("{\"ok\":true}".into()).is_ok());
        assert!(action_result("{\"interrupted\":true}".into()).is_err());
        assert!(action_result("{\"ok\":false,\"error\":\"element not found\"}".into()).is_err());
        assert!(action_result("not json".into()).is_err());
    }

    #[test]
    fn capability_and_reference_validation_is_strict() {
        assert!(constant_time_eq(b"token", b"token"));
        assert!(!constant_time_eq(b"token", b"tokeN"));
        assert!(!constant_time_eq(b"token", b"token "));
        assert!(valid_ref("") && valid_ref("e1") && valid_ref("e123"));
        assert!(!valid_ref("e") && !valid_ref("x1") && !valid_ref("e123456"));
        assert!(valid_selector("button.primary"));
        assert!(!valid_selector("button\n.x"));
    }
}
