//! OpenCode ACP: exact native continuation and scoped once-only decisions.
use crate::codex::{Event, Run, Wire};
use crate::error::{Error, Result};
use crate::models::*;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    process::Stdio,
    time::Duration,
};
#[cfg(test)]
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

pub fn validate_binding(session: &Session, cwd: &str) -> Result<()> {
    if session.native_thread.as_ref().is_some_and(|n| {
        session.agent != AgentProviderId::OpenCode
            || n.session_id != session.id
            || n.project_id != session.project_id
            || n.cwd != cwd
            || n.model != session.model
            || n.provider_account_id != session.provider_account_id
            || n.thread_id.is_empty()
            || n.thread_id.len() > 1024
    }) {
        return Err(Error::agent(
            "OpenCode native identity does not match session, project, workspace or model",
        ));
    }
    Ok(())
}
pub fn validate_response(pending: &PendingRequest, response: &AgentResponse) -> Result<()> {
    if !matches!(
        (&pending.kind, response),
        (
            PendingRequestKind::FileChange { .. },
            AgentResponse::Approval { .. }
        )
    ) {
        return Err(Error::agent(
            "OpenCode accepts only a decision for a native file approval",
        ));
    }
    Ok(())
}
fn policy() -> Value {
    json!({"*":"deny","edit":"ask","read":"allow","glob":"allow","grep":"allow","list":"allow","external_directory":"deny","bash":"deny","task":"deny","webfetch":"deny","websearch":"deny","question":"deny","skill":"deny","todowrite":"deny","lsp":"deny","doom_loop":"deny"})
}
fn config(inline: Option<&str>) -> Result<String> {
    let mut v: Value = serde_json::from_str(inline.unwrap_or("{}"))
        .map_err(|_| Error::agent("Invalid OpenCode inline configuration"))?;
    let o = v
        .as_object_mut()
        .ok_or_else(|| Error::agent("Invalid OpenCode inline configuration"))?;
    o.insert("share".into(), json!("disabled"));
    o.insert("default_agent".into(), json!("build"));
    // Child-only approval request policy; managed configuration/plugins remain vendor authority.
    o.insert("permission".into(), policy());
    let agents = o
        .entry("agent")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::agent("Invalid OpenCode agent configuration"))?;
    let build = agents
        .entry("build")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::agent("Invalid OpenCode build configuration"))?;
    build.insert("permission".into(), policy());
    // Deprecated modes are merged into agents after inline config by this CLI.
    let modes = o
        .entry("mode")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::agent("Invalid OpenCode mode configuration"))?;
    let build = modes
        .entry("build")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| Error::agent("Invalid OpenCode build mode configuration"))?;
    build.insert("permission".into(), policy());
    Ok(v.to_string())
}
fn execution_config(inline: Option<&str>, planning: bool) -> Result<Value> {
    let mut value: Value = serde_json::from_str(&config(inline)?)?;
    if planning {
        let mut read_only = policy();
        read_only["edit"] = json!("deny");
        value["default_agent"] = json!("plan");
        value["permission"] = read_only.clone();
        for key in ["agent", "mode"] {
            let entries = value[key]
                .as_object_mut()
                .ok_or_else(|| Error::agent("Invalid OpenCode execution configuration"))?;
            for name in ["build", "plan"] {
                let entry = entries
                    .entry(name)
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or_else(|| Error::agent("Invalid OpenCode planning configuration"))?;
                entry.insert("permission".into(), read_only.clone());
            }
        }
    }
    Ok(value)
}

/// `OPENCODE_PERMISSION` as OpenCode 1.19 parses it: a rule object. A bare action
/// (Full access's `"allow"`) makes `session/new` fail with an internal error, so it
/// becomes the same action for every permission (`{"*": "allow"}`).
fn permission_env(permission: &Value) -> Value {
    match permission {
        Value::String(action) => json!({ "*": action }),
        other => other.clone(),
    }
}

fn approval_config(
    inline: Option<&str>,
    options: &crate::models::ExecutionOptions,
) -> Result<Value> {
    let mut value = execution_config(inline, options.planning)?;
    if options.planning {
        return Ok(value);
    }
    let auto = matches!(options.approval, Some(crate::models::ApprovalMode::Auto));
    let mut helpers = None;
    let permission = match options.approval {
        Some(crate::models::ApprovalMode::Full) => json!("allow"),
        Some(crate::models::ApprovalMode::Auto) => {
            let mut permissions = policy();
            permissions["edit"] = json!("allow");
            helpers = Some(permissions.clone());
            // Delegation reaches only the built-in helpers pinned below; repository-defined
            // agents can grant themselves wider permissions and stay unreachable.
            permissions["task"] = json!({"*": "deny", "general": "allow", "explore": "allow"});
            permissions
        }
        _ => return Ok(value),
    };
    value["permission"] = permission.clone();
    for key in ["agent", "mode"] {
        // Deprecated `mode` entries merge into agents after inline config, so both layers are pinned.
        let names: &[&str] = if auto {
            &["build", "plan", "general", "explore"]
        } else {
            &["build", "plan"]
        };
        for name in names {
            let entries = value[key]
                .as_object_mut()
                .ok_or_else(|| Error::agent("Invalid OpenCode execution configuration"))?;
            let entry = entries
                .entry(*name)
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| Error::agent("Invalid OpenCode execution configuration"))?;
            // Helpers keep the same Auto limits and cannot delegate further.
            let pinned = match (&helpers, *name) {
                (Some(helper), "general" | "explore") => helper.clone(),
                _ => permission.clone(),
            };
            entry.insert("permission".into(), pinned);
        }
    }
    Ok(value)
}
pub async fn start(
    session: Session,
    cwd: String,
    prompt: String,
    overrides: HashMap<AgentProviderId, String>,
) -> Result<(crate::agent::AgentProcess, crate::agent::StartedAgent)> {
    validate_binding(&session, &cwd)?;
    let install = crate::detect::resolve_with_overrides(&AgentProviderId::OpenCode, &overrides)
        .ok_or_else(|| Error::agent("Unknown provider"))?;
    if !install.installed {
        return Err(Error::agent("OpenCode is not installed"));
    }
    let binary = install.path.unwrap_or(install.binary);
    let inline_config = approval_config(
        std::env::var("OPENCODE_CONFIG_CONTENT").ok().as_deref(),
        &session.execution,
    )?;
    // The permission policy is fixed when OpenCode starts: it is part of a kept process's key.
    let key = format!(
        "{binary}\u{0}{cwd}\u{0}{inline_config}\u{0}{}",
        crate::computer_mcp::endpoint(&session.id).is_some()
    );
    if let Some((child, wire)) =
        crate::agent_pool::take(&session.id, &crate::agent_pool::with_thread(&key, &session))
    {
        let generation = uuid::Uuid::new_v4().to_string();
        let (cancel, receiver) = watch::channel(false);
        let (sender, replies) = mpsc::channel(16);
        return Ok((
            crate::agent::AgentProcess {
                cancel,
                replies: Some(sender),
                generation: Some(generation.clone()),
                #[cfg(unix)]
                pid: child.id(),
            },
            crate::agent::StartedAgent {
                review: None,
                wire: Some(wire),
                pool_key: Some(key),
                child,
                cancel: receiver,
                codex: Some(Run {
                    session,
                    cwd,
                    prompt,
                    attachments: Vec::new(),
                    generation,
                    replies,
                }),
            },
        ));
    }
    let mut command = crate::detect::command(binary);
    command
        // Error-level logs are the only place OpenCode reports provider failures it keeps retrying.
        .args(["acp", "--print-logs", "--log-level", "ERROR", "--cwd", &cwd])
        .current_dir(&cwd)
        .env("OPENCODE_AUTO_SHARE", "false")
        .env(
            "OPENCODE_PERMISSION",
            permission_env(&inline_config["permission"]).to_string(),
        )
        .env("OPENCODE_CONFIG_CONTENT", inline_config.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    crate::process_priority::lower_async(&mut command);
    let child = command
        .spawn()
        .map_err(|_| Error::agent("Cannot start OpenCode ACP"))?;
    let generation = uuid::Uuid::new_v4().to_string();
    let (cancel, receiver) = watch::channel(false);
    let (sender, replies) = mpsc::channel(16);
    Ok((
        crate::agent::AgentProcess {
            cancel,
            replies: Some(sender),
            generation: Some(generation.clone()),
            #[cfg(unix)]
            pid: child.id(),
        },
        crate::agent::StartedAgent {
            review: None,
            wire: None,
            pool_key: Some(key),
            child,
            cancel: receiver,
            codex: Some(Run {
                session,
                cwd,
                prompt,
                attachments: Vec::new(),
                generation,
                replies,
            }),
        },
    ))
}
pub(crate) fn denied(id: &Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}})
}
pub(crate) fn unsupported(value: &Value) -> Value {
    if value["method"] == "session/request_permission" {
        denied(&value["id"])
    } else {
        json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32601,"message":"Unsupported client capability"}})
    }
}
async fn request(
    wire: &mut Wire,
    method: &str,
    params: Value,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Value> {
    let id = uuid::Uuid::new_v4().to_string();
    wire.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
        .await?;
    tokio::time::timeout(Duration::from_secs(30),async {loop {
        let v=tokio::select!{v=wire.read()=>v?,_=cancel.changed()=>return Err(Error::agent("OpenCode startup cancelled"))};
        if v["id"]==id && v.get("method").is_none() {if v.get("error").is_some(){return Err(Error::agent(format!("OpenCode rejected {method}{}; verify native identity and CLI compatibility",rpc_code(&v))));} return v.get("result").cloned().ok_or_else(||Error::agent("OpenCode response lacks result"));}
        // In particular, session/load history is deliberately discarded.
        if v.get("id").is_some() && v.get("method").is_some() {wire.send(unsupported(&v)).await?;}
    }}).await.map_err(|_|Error::agent("OpenCode startup timed out"))?
}
pub(crate) fn approval(params: &Value, cwd: &str) -> Option<(PendingRequestKind, String, String)> {
    let call = &params["toolCall"];
    if !call["toolCallId"]
        .as_str()
        .is_some_and(|id| !id.is_empty() && id.len() <= 1024)
    {
        return None;
    }
    if call["kind"] != "edit" || params.to_string().len() > 128 * 1024 {
        return None;
    }
    let options = params["options"].as_array()?;
    if options.len() < 2 || options.len() > 16 {
        return None;
    }
    let mut ids = HashSet::new();
    for option in options {
        let id = option["optionId"].as_str()?;
        if id.is_empty() || id.len() > 1024 || !ids.insert(id) {
            return None;
        }
        if !matches!(
            option["kind"].as_str(),
            Some("allow_once" | "reject_once" | "allow_always" | "reject_always")
        ) {
            return None;
        }
    }
    let mut allows = options.iter().filter(|o| o["kind"] == "allow_once");
    let allow = allows.next()?["optionId"].as_str()?.to_owned();
    if allows.next().is_some() {
        return None;
    }
    let mut rejects = options.iter().filter(|o| o["kind"] == "reject_once");
    let reject = rejects.next()?["optionId"].as_str()?.to_owned();
    if rejects.next().is_some() {
        return None;
    }
    let mut changes = Vec::new();
    for c in call["content"].as_array()? {
        if c["type"] != "diff" {
            return None;
        }
        let path = c["path"].as_str()?;
        let target = std::path::Path::new(path);
        if !target.is_absolute()
            || !target.starts_with(cwd)
            || target
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return None;
        }
        let ancestor = target
            .ancestors()
            .find(|p| p.exists())?
            .canonicalize()
            .ok()?;
        if !ancestor.starts_with(cwd) {
            return None;
        }
        let old = c["oldText"].as_str()?;
        let new = c["newText"].as_str()?;
        changes.push(ProposedFileChange {
            path: path.into(),
            kind: if old.is_empty() { "add" } else { "update" }.into(),
            diff: format!("--- proposed original\n{old}\n+++ proposed replacement\n{new}"),
            move_path: None,
        });
    }
    if changes.is_empty() {
        return None;
    }
    Some((
        PendingRequestKind::FileChange {
            reason: Some("Native OpenCode proposed edit".into()),
            changes,
        },
        allow,
        reject,
    ))
}
pub(crate) fn rpc_code(value: &Value) -> String {
    value["error"]["code"]
        .as_i64()
        .map(|code| format!(" (JSON-RPC {code})"))
        .unwrap_or_default()
}
fn prompt_error(value: &Value) -> Error {
    if value["error"]["code"] == -32603
        && value["error"]["data"]["service"] == "session"
        && value["error"]["data"]["errorName"] == "APIError"
    {
        Error::agent(format!(
            "OpenCode provider API request failed (APIError){}; check the provider CLI and model availability",
            rpc_code(value)
        ))
    } else {
        Error::agent(format!(
            "OpenCode prompt failed{}; see CLI diagnostics",
            rpc_code(value)
        ))
    }
}
fn confirm_config(configured: &Value, id: &Value, value: &str) -> Result<()> {
    if configured["configOptions"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row["id"] == *id && row["currentValue"] == value)
    }) {
        Ok(())
    } else {
        Err(Error::agent(
            "OpenCode did not confirm the selected execution option",
        ))
    }
}
fn offered_effort(configured: &Value, effort: &str) -> Result<Value> {
    if effort != "default" && !crate::execution::EFFORTS.contains(&effort) {
        return Err(Error::agent("Unknown OpenCode reasoning level"));
    }
    let options = configured["configOptions"]
        .as_array()
        .ok_or_else(|| Error::agent("OpenCode reasoning selection is unavailable"))?;
    let mut choices = options.iter().filter(|row| {
        row["type"] == "select"
            && (row["category"] == "thought_level" || row["id"] == "variant")
            && row["options"]
                .as_array()
                .is_some_and(|values| values.iter().any(|value| value["value"] == effort))
    });
    let row = choices.next().ok_or_else(|| {
        Error::agent("OpenCode selected model does not offer this reasoning variant")
    })?;
    if choices.next().is_some()
        || !row["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && id.len() <= 100)
    {
        return Err(Error::agent("OpenCode reasoning option is ambiguous"));
    }
    Ok(row["id"].clone())
}
/// Where each session's log reader reports provider errors: the turn running now.
static ERROR_ROUTES: parking_lot::Mutex<Option<HashMap<String, mpsc::Sender<String>>>> =
    parking_lot::Mutex::new(None);

pub(crate) fn route_errors(session_id: &str, errors: mpsc::Sender<String>) {
    ERROR_ROUTES
        .lock()
        .get_or_insert_with(HashMap::new)
        .insert(session_id.to_owned(), errors);
}

/// Reads OpenCode's error-level logs. Only the main agent's provider stream errors are kept, as
/// bounded secret-free text, and go to the session's current turn; every other log line is
/// discarded rather than shown.
pub(crate) fn watch_logs<R>(reader: R, session_id: String) -> tokio::task::JoinHandle<bool>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        // Keep draining every line so the child never blocks on a full pipe.
        let mut lines = tokio::io::BufReader::new(reader);
        while let Ok(Some(line)) = crate::agent::bounded_line(&mut lines).await {
            if let Some(error) = provider_stream_error(&line) {
                let route = ERROR_ROUTES
                    .lock()
                    .as_ref()
                    .and_then(|routes| routes.get(&session_id).cloned());
                if let Some(route) = route {
                    let _ = route.try_send(error);
                }
            }
        }
        false
    })
}
fn provider_stream_error(line: &str) -> Option<String> {
    if line.len() > 16 * 1024
        || !line.contains("level=ERROR")
        || !line.contains("message=\"stream error\"")
        || !line.contains(" small=false ")
    {
        return None;
    }
    let start = line.find("error.error=\"")? + "error.error=\"".len();
    let text = &line[start..];
    let end = text.find('"').unwrap_or(text.len());
    let text = text[..end].trim();
    let text = text
        .strip_prefix("AI_APICallError: ")
        .or_else(|| text.strip_prefix("AI_RetryError: "))
        .unwrap_or(text);
    let text: String = text.chars().filter(|c| !c.is_control()).take(300).collect();
    (!text.is_empty()).then_some(text)
}
/// OpenCode retries every provider error with backoff, including ones that cannot succeed
/// (exhausted plans, missing credit, rejected credentials). Those end the turn visibly.
pub(crate) fn fatal_provider_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "usage limit",
        "limit exceeded",
        "quota",
        "insufficient",
        "credit",
        "balance",
        "billing",
        "payment",
        "unauthorized",
        "forbidden",
        "invalid api key",
        "api key",
        "authentication",
        "free tier",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// The app's own MCP servers (browser, Sirus tools, computer use) for an ACP session, which
/// receives them from the client rather than from the CLI configuration.
pub(crate) fn acp_mcp_servers(session_id: &str) -> Vec<Value> {
    crate::browser_mcp::ensure_endpoint(session_id)
            .and_then(|endpoint| {
                let exe = std::env::current_exe().ok()?;
                let mut env = vec![
                    json!({ "name": crate::browser_mcp::SOCKET_ENV, "value": endpoint.socket.to_string_lossy() }),
                    json!({ "name": crate::browser_mcp::TOKEN_ENV, "value": endpoint.token }),
                ];
                for (key, value) in crate::browser_mcp::child_env() {
                    env.push(json!({ "name": key, "value": value }));
                }
                Some(json!({
                    "name": "sirus_browser",
                    "command": exe.to_string_lossy(),
                    "args": ["--mcp-browser"],
                    "env": env
                }))
            })
            .map(|server| vec![server])
            .unwrap_or_default()
            .into_iter()
            .chain(crate::computer_mcp::acp_server(session_id))
            .collect()
}

pub(crate) async fn execute(
    wire: &mut Wire,
    run: &mut Run,
    cancel: &mut watch::Receiver<bool>,
    provider_errors: &mut mpsc::Receiver<String>,
    emit: &mut impl FnMut(Event) -> Result<()>,
) -> Result<bool> {
    // A kept process (agent_pool) was initialized by its first turn.
    let init = if wire.initialized {
        wire.acp_init.clone()
    } else {
        let init=request(wire,"initialize",json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"Sirus Code","version":"0.1.0"}}),cancel).await?;
        wire.initialized = true;
        wire.acp_init = init.clone();
        init
    };
    if init["protocolVersion"] != 1 {
        return Err(Error::agent("Unsupported OpenCode ACP version"));
    }
    let existing = run
        .session
        .native_thread
        .as_ref()
        .map(|n| n.thread_id.clone());
    if existing.is_some() && init["agentCapabilities"]["loadSession"] != true {
        return Err(Error::agent(
            "OpenCode does not support exact session loading",
        ));
    }
    // ACP sessions receive MCP servers from the client, not from the CLI config.
    // A kept process already holds this session loaded: no reload, the last setup stands.
    let reused = existing.is_some() && wire.loaded == existing;
    let mut mcp_servers = acp_mcp_servers(&run.session.id);
    // Connectors (ADR-137), when this OpenCode takes HTTP MCP servers.
    mcp_servers.extend(crate::connectors::acp_servers(
        &crate::connectors::for_session(&run.session).await,
        &init,
    ));
    let setup = if reused {
        wire.acp_setup.clone()
    } else {
        request(
            wire,
            if existing.is_some() {
                "session/load"
            } else {
                "session/new"
            },
            if let Some(id) = &existing {
                json!({"sessionId":id,"cwd":run.cwd,"mcpServers":mcp_servers})
            } else {
                json!({"cwd":run.cwd,"mcpServers":mcp_servers})
            },
            cancel,
        )
        .await?
    };
    let native = existing
        .or_else(|| setup["sessionId"].as_str().map(str::to_owned))
        .filter(|id| !id.is_empty() && id.len() <= 1024)
        .ok_or_else(|| Error::agent("OpenCode returned invalid native identity"))?;
    wire.loaded = Some(native.clone());
    wire.acp_setup = setup.clone();
    let mut configured = setup.clone();
    let mode_value = if run.session.execution.planning {
        "plan"
    } else {
        "build"
    };
    if let Some(mode) = setup["configOptions"].as_array().and_then(|options| {
        options
            .iter()
            .find(|o| o["category"] == "mode" && o["type"] == "select")
    }) {
        if !mode["options"]
            .as_array()
            .is_some_and(|options| options.iter().any(|o| o["value"] == mode_value))
        {
            return Err(Error::agent("OpenCode selected mode is unavailable"));
        }
        configured = request(
            wire,
            "session/set_config_option",
            json!({"sessionId":native,"configId":mode["id"],"value":mode_value}),
            cancel,
        )
        .await?;
        confirm_config(&configured, &mode["id"], mode_value)?;
    } else if run.session.execution.planning {
        return Err(Error::agent("OpenCode planning is unavailable"));
    }
    if let Some(model) = &run.session.model {
        let offered = setup["configOptions"]
            .as_array()
            .and_then(|options| {
                options
                    .iter()
                    .find(|o| o["category"] == "model" && o["type"] == "select")
            })
            .ok_or_else(|| Error::agent("OpenCode model selection is unsupported"))?;
        if !offered["options"]
            .as_array()
            .is_some_and(|options| options.iter().any(|o| o["value"] == *model))
        {
            return Err(Error::agent("OpenCode selected model is not offered"));
        }
        configured = request(
            wire,
            "session/set_config_option",
            json!({"sessionId":native,"configId":offered["id"],"value":model}),
            cancel,
        )
        .await?;
        if !configured["configOptions"]
            .as_array()
            .is_some_and(|options| {
                options
                    .iter()
                    .any(|o| o["id"] == offered["id"] && o["currentValue"] == *model)
            })
        {
            return Err(Error::agent("OpenCode did not confirm the selected model"));
        }
    }
    // Clear retained reasoning on exact resume when the caller requests vendor defaults.
    let effort = run.session.execution.effort.as_deref().or_else(|| {
        configured["configOptions"]
            .as_array()
            .filter(|rows| rows.iter().any(|row| row["category"] == "thought_level"))
            .map(|_| "default")
    });
    if let Some(effort) = effort {
        let option = offered_effort(&configured, effort)?;
        let next = request(
            wire,
            "session/set_config_option",
            json!({"sessionId":native,"configId":option,"value":effort}),
            cancel,
        )
        .await?;
        confirm_config(&next, &option, effort)?;
    }
    emit(Event::Identity(NativeThread {
        provider_account_id: run.session.provider_account_id.clone(),
        thread_id: native.clone(),
        session_id: run.session.id.clone(),
        project_id: run.session.project_id.clone(),
        cwd: run.cwd.clone(),
        model: run.session.model.clone(),
    }))?;
    let turn = uuid::Uuid::new_v4().to_string();
    wire.send(json!({"jsonrpc":"2.0","id":turn,"method":"session/prompt","params":{"sessionId":native,"prompt":crate::attachments::opencode_input(&run.prompt, &run.attachments, &init["agentCapabilities"]["promptCapabilities"])?}})).await?;
    let mut pending: HashMap<String, (Value, PendingRequest, String, String)> = HashMap::new();
    let mut seen = HashSet::new();
    let mut total = 0usize;
    let mut interrupted = false;
    let mut deadline = None;
    loop {
        let v = tokio::select! {
            v=wire.read()=>v?,
            _=cancel.changed(),if !interrupted=>{for (opaque,(id,_,_,_)) in pending.drain(){wire.send(denied(&id)).await?;emit(Event::Answered(opaque))?;}interrupted=true;wire.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":native}})).await?;deadline=Some(tokio::time::Instant::now()+Duration::from_secs(2));continue;},
            _=async{if let Some(d)=deadline{tokio::time::sleep_until(d).await;}else{std::future::pending::<()>().await;}},if interrupted=>return Ok(true),
            Some(message)=provider_errors.recv(),if !interrupted=>{
                if !fatal_provider_error(&message) {continue;}
                // Stop OpenCode's retry loop; the turn fails with the provider's own reason.
                for (opaque,(id,_,_,_)) in pending.drain(){wire.send(denied(&id)).await?;emit(Event::Answered(opaque))?;}
                let _=wire.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":native}})).await;
                return Err(Error::agent(format!("OpenCode provider error: {message}")));
            },
            Some(inbound)=run.replies.recv(),if !interrupted=>{
                let reply=match inbound {
                    crate::codex::Inbound::Answer(reply)=>reply,
                    crate::codex::Inbound::Steer{result,..}=>{let _=result.send(Err(Error::agent("OpenCode does not accept instructions during a turn")));continue;}
                    crate::codex::Inbound::StopTask{result,..}=>{let _=result.send(Err(Error::agent("OpenCode has no background tasks to stop")));continue;}
                    crate::codex::Inbound::SteerSubagent{result,..}=>{let _=result.send(Err(Error::agent("OpenCode subagents cannot receive messages")));continue;}
                    crate::codex::Inbound::BackgroundTask{result,..}=>{let _=result.send(Err(Error::agent("OpenCode cannot move a subagent to the background")));continue;}
                };
                let r=&reply.request;
                let mut delivery_failed=false;
                let result=if r.session_id!=run.session.id||r.generation!=run.generation||r.turn_id!=turn {Err(Error::agent("Stale or foreign OpenCode reply"))}
                else if let Some((id,entry,allow,reject))=pending.get(&r.request_id){match validate_response(entry,&r.response){Err(e)=>Err(e),Ok(())=>{
                    let payload=match &r.response{AgentResponse::Approval{decision:ApprovalDecision::Accept}=>json!({"outcome":"selected","optionId":allow}),AgentResponse::Approval{decision:ApprovalDecision::Decline}=>json!({"outcome":"selected","optionId":reject}),_=>json!({"outcome":"cancelled"})};
                    let id=id.clone();pending.remove(&r.request_id);emit(Event::Answered(r.request_id.clone()))?;let result=wire.send(json!({"jsonrpc":"2.0","id":id,"result":{"outcome":payload}})).await;delivery_failed=result.is_err();
                    if matches!(r.response,AgentResponse::Approval{decision:ApprovalDecision::Cancel}) {for (opaque,(id,_,_,_)) in pending.drain(){wire.send(denied(&id)).await?;emit(Event::Answered(opaque))?;}interrupted=true;wire.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":native}})).await?;deadline=Some(tokio::time::Instant::now()+Duration::from_secs(2));}result
                }}}else{Err(Error::agent("OpenCode request already answered"))};
                let _=reply.result.send(result);if delivery_failed {return Err(Error::agent("OpenCode reply delivery failed"));}continue;
            }
        };
        if v["id"] == turn && v.get("method").is_none() {
            if interrupted {
                #[cfg(test)]
                eprintln!(
                    "ACP interrupted prompt settled: {}",
                    v["result"]["stopReason"]
                );
                return Ok(true);
            }
            if v.get("error").is_some() {
                return Err(prompt_error(&v));
            }
            return match v["result"]["stopReason"].as_str() {
                Some("end_turn") => {
                    // The session is idle again: the process can be kept for the next turn.
                    wire.reusable = true;
                    Ok(false)
                }
                Some("cancelled") => Ok(true),
                _ => Err(Error::agent(
                    "OpenCode stopped without successful completion",
                )),
            };
        }
        if v.get("id").is_some() && v.get("method").is_some() {
            let id = v["id"].clone();
            let key = id.to_string();
            if !(id.is_string() || id.is_i64() || id.is_u64())
                || id.as_str().is_some_and(str::is_empty)
                || key.len() > 1024
                || seen.len() >= 4096
                || !seen.insert(key)
            {
                return Err(Error::agent("Invalid or duplicate OpenCode callback"));
            }
            if interrupted
                || v["method"] != "session/request_permission"
                || v["params"]["sessionId"] != native
            {
                wire.send(unsupported(&v)).await?;
                continue;
            }
            let Some((kind, allow, reject)) = approval(&v["params"], &run.cwd) else {
                wire.send(denied(&id)).await?;
                continue;
            };
            if pending.len() >= 32 {
                return Err(Error::agent("Too many OpenCode callbacks"));
            }
            let opaque = uuid::Uuid::new_v4().to_string();
            let entry = PendingRequest {
                request_id: opaque.clone(),
                generation: run.generation.clone(),
                turn_id: turn.clone(),
                item_id: v["params"]["toolCall"]["toolCallId"]
                    .as_str()
                    .unwrap_or("native-edit")
                    .into(),
                kind,
            };
            pending.insert(opaque, (id, entry.clone(), allow, reject));
            emit(Event::Pending(entry))?;
        } else if v["method"] == "session/update"
            && v["params"]["sessionId"] == native
            && !interrupted
        {
            let items = crate::activity::opencode(&v["params"]["update"]);
            if !items.is_empty() {
                emit(Event::Activity(items))?;
            }
            if v["params"]["update"]["sessionUpdate"] != "agent_message_chunk" {
                continue;
            }
            let content = &v["params"]["update"]["content"];
            if content["type"] == "text" {
                if let Some(chunk) = content["text"].as_str() {
                    total += chunk.len();
                    if total > 8 * 1024 * 1024 {
                        return Err(Error::agent("OpenCode output exceeds limit"));
                    }
                    emit(Event::Delta(chunk.into()))?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(cwd: &str) -> Session {
        serde_json::from_value(json!({"id":"s","projectId":"p","title":"fixture","agent":"opencode","status":"running","createdAt":"time","lastActivityAt":"time","worktree":{"path":cwd,"branch":"main","isolated":false},"messages":[]})).unwrap()
    }
    #[test]
    fn selected_permissions_override_global_agent_and_deprecated_mode_only_for_the_child() {
        let inline = r#"{"permission":{"edit":"ask","bash":"deny"},"agent":{"build":{"permission":{"edit":"deny"}}},"mode":{"build":{"permission":{"edit":"deny"}}}}"#;
        let original: Value = serde_json::from_str(inline).unwrap();
        for mode in ["ask", "auto", "full"] {
            let options = serde_json::from_value(json!({"approval":mode})).unwrap();
            let value = approval_config(Some(inline), &options).unwrap();
            assert_eq!(value["share"], "disabled");
            for permission in [
                &value["permission"],
                &value["agent"]["build"]["permission"],
                &value["mode"]["build"]["permission"],
            ] {
                if mode == "full" {
                    assert_eq!(permission, "allow");
                } else {
                    assert_eq!(
                        permission["edit"],
                        if mode == "auto" { "allow" } else { "ask" }
                    );
                    assert_eq!(permission["bash"], "deny");
                }
            }
        }
        let plan = serde_json::from_value(json!({"approval":"full","planning":true})).unwrap();
        assert_eq!(
            approval_config(Some(inline), &plan).unwrap()["permission"]["edit"],
            "deny"
        );
        assert_eq!(serde_json::from_str::<Value>(inline).unwrap(), original);
    }
    #[test]
    fn provider_stream_errors_are_bounded_and_classified() {
        let line = r#"timestamp=2026-10-03T12:22:53.747Z level=ERROR run=c1 message="stream error" providerID=opencode-go modelID=deepseek-v4.1-flash session.id=ses_x small=false agent=build mode=primary error.error="AI_APICallError: Go usage limit exceeded""#;
        assert_eq!(
            provider_stream_error(line).as_deref(),
            Some("Go usage limit exceeded")
        );
        assert!(fatal_provider_error("Go usage limit exceeded"));
        // Title generation (small=true), other levels and other messages are ignored.
        assert!(provider_stream_error(&line.replace(" small=false ", " small=true ")).is_none());
        assert!(provider_stream_error(&line.replace("level=ERROR", "level=INFO")).is_none());
        assert!(provider_stream_error(&line.replace("stream error", "other")).is_none());
        // Transient failures keep OpenCode's own retry behavior.
        assert!(!fatal_provider_error("Connection reset by peer"));
        assert!(!fatal_provider_error("Overloaded"));
        assert!(fatal_provider_error("Insufficient balance"));
    }
    #[test]
    fn auto_delegates_only_to_pinned_built_in_helpers() {
        // A repository can redefine helpers (including through deprecated `mode`) with wider grants.
        let inline = r#"{"agent":{"general":{"permission":{"bash":"allow"}},"sneaky":{"permission":"allow"}},"mode":{"explore":{"permission":{"webfetch":"allow"}}}}"#;
        let auto = serde_json::from_value(json!({"approval":"auto"})).unwrap();
        let value = approval_config(Some(inline), &auto).unwrap();
        let task = json!({"*":"deny","general":"allow","explore":"allow"});
        for parent in [
            &value["permission"],
            &value["agent"]["build"]["permission"],
            &value["mode"]["build"]["permission"],
        ] {
            assert_eq!(parent["task"], task);
        }
        for key in ["agent", "mode"] {
            for helper in ["general", "explore"] {
                let pinned = &value[key][helper]["permission"];
                assert_eq!(pinned["task"], "deny", "{key}.{helper}");
                assert_eq!(pinned["bash"], "deny", "{key}.{helper}");
                assert_eq!(pinned["webfetch"], "deny", "{key}.{helper}");
                assert_eq!(pinned["external_directory"], "deny", "{key}.{helper}");
                assert_eq!(pinned["edit"], "allow", "{key}.{helper}");
            }
        }
        // Ask and planning keep delegation denied; Full keeps its explicit allow.
        for options in [
            json!({"approval":"ask"}),
            json!({"approval":"auto","planning":true}),
        ] {
            let options = serde_json::from_value(options).unwrap();
            assert_eq!(
                approval_config(Some(inline), &options).unwrap()["permission"]["task"],
                "deny"
            );
        }
    }
    #[test]
    fn planning_overrides_all_effective_edit_policies_without_panics_or_vendor_writes() {
        let value = execution_config(
            Some(r#"{"agent":{"plan":{"permission":{"edit":"allow"}}}}"#),
            true,
        )
        .unwrap();
        for key in ["agent", "mode"] {
            for mode in ["build", "plan"] {
                assert_eq!(value[key][mode]["permission"]["edit"], "deny");
            }
        }
        assert_eq!(value["permission"]["edit"], "deny");
        assert!(execution_config(Some(r#"{"agent":{"plan":"invalid"}}"#), true).is_err());
        assert_eq!(
            execution_config(None, false).unwrap()["permission"]["edit"],
            "ask"
        );
    }
    #[test]
    fn reasoning_options_require_one_native_offered_value_and_confirmation() {
        let offered = json!({"configOptions":[{"id":"effort","type":"select","category":"thought_level","currentValue":"high","options":[{"value":"low"},{"value":"high"}]}]});
        assert_eq!(offered_effort(&offered, "high").unwrap(), json!("effort"));
        assert!(offered_effort(&offered, "max").is_err());
        assert!(offered_effort(&offered, "--bad").is_err());
        assert!(confirm_config(&offered, &json!("effort"), "high").is_ok());
        assert!(confirm_config(&offered, &json!("effort"), "low").is_err());
        let duplicate =
            json!({"configOptions":[offered["configOptions"][0],offered["configOptions"][0]]});
        assert!(offered_effort(&duplicate, "high").is_err());
    }
    #[test]
    fn foreign_resume_fails() {
        let mut s = session("/fixture");
        s.native_thread = Some(NativeThread {
            provider_account_id: "default".into(),
            thread_id: "native".into(),
            session_id: "s".into(),
            project_id: "p".into(),
            cwd: "/fixture".into(),
            model: None,
        });
        assert!(validate_binding(&s, "/fixture").is_ok());
        assert!(validate_binding(&s, "/outside").is_err());
    }
    #[test]
    fn approval_requires_native_once_options_and_jailed_diff() {
        let root = std::env::temp_dir().join(format!("sirus-acp-diff-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let mut params = json!({"toolCall":{"toolCallId":"edit","kind":"edit","content":[{"type":"diff","path":format!("{cwd}/new.txt"),"oldText":"","newText":"new"}]},"options":[{"kind":"allow_once","optionId":"once"},{"kind":"reject_once","optionId":"reject"}]});
        assert!(approval(&params, &cwd).is_some());
        params["options"][0]["kind"] = json!("allow_always");
        assert!(approval(&params, &cwd).is_none());
        params["options"][0]["kind"] = json!("allow_once");
        params["toolCall"]["content"][0]["path"] = json!("/outside/new.txt");
        assert!(approval(&params, &cwd).is_none());
        params["toolCall"]["content"][0]["path"] = json!(format!("{cwd}/../outside.txt"));
        assert!(approval(&params, &cwd).is_none());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/tmp", root.join("escape")).unwrap();
            params["toolCall"]["content"][0]["path"] = json!(format!("{cwd}/escape/new.txt"));
            assert!(approval(&params, &cwd).is_none());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn approval_rejects_ambiguous_and_unbounded_native_options() {
        let root = std::env::temp_dir().join(format!("sirus-acp-options-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let base = json!({"toolCall":{"toolCallId":"edit","kind":"edit","content":[{"type":"diff","path":format!("{cwd}/new.txt"),"oldText":"","newText":"new"}]},"options":[{"kind":"allow_once","optionId":"once"},{"kind":"reject_once","optionId":"reject"}]});
        for options in [
            json!([{"kind":"allow_once","optionId":""},{"kind":"reject_once","optionId":"reject"}]),
            json!([{"kind":"allow_once","optionId":"same"},{"kind":"reject_once","optionId":"same"}]),
            json!([{"kind":"allow_once","optionId":"once"},{"kind":"reject_once","optionId":"reject"},{"kind":"allow_always","optionId":"once"}]),
            json!([{"kind":"allow_once","optionId":"once"},{"kind":"allow_once","optionId":"second"},{"kind":"reject_once","optionId":"reject"}]),
            json!((0..17)
                .map(|n| json!({"kind":"allow_once","optionId":format!("id{n}")}))
                .collect::<Vec<_>>()),
        ] {
            let mut p = base.clone();
            p["options"] = options;
            assert!(approval(&p, &cwd).is_none());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn child_policy_replaces_hostile_inline_global_and_build_rules() {
        let v:Value=serde_json::from_str(&config(Some(r#"{"permission":"allow","agent":{"build":{"permission":{"edit":"allow","bash":"allow"}}},"mode":{"build":{"permission":{"edit":"allow","bash":"allow"}}}}"#)).unwrap()).unwrap();
        assert_eq!(v["permission"]["edit"], "ask");
        assert_eq!(v["permission"]["bash"], "deny");
        assert_eq!(v["agent"]["build"]["permission"]["edit"], "ask");
        assert_eq!(v["mode"]["build"]["permission"]["edit"], "ask");
        assert_eq!(v["mode"]["build"]["permission"]["bash"], "deny");
        assert_eq!(v["share"], "disabled");
        assert!(config(Some("[]")).is_err());
    }
    #[test]
    fn permission_env_is_always_a_rule_object() {
        assert_eq!(permission_env(&json!("allow")), json!({"*": "allow"}));
        let rules = json!({"edit": "allow", "bash": "ask"});
        assert_eq!(permission_env(&rules), rules);
    }
    #[test]
    fn provider_api_failure_is_explicit_without_echoing_upstream_payload() {
        let value = json!({"error":{"code":-32603,"message":"private upstream detail","data":{"service":"session","errorName":"APIError"}}});
        let text = prompt_error(&value).to_string();
        assert!(text.contains("APIError"));
        assert!(text.contains("JSON-RPC -32603"));
        assert!(!text.contains("private upstream"));
    }
    #[tokio::test]
    async fn native_replay_once_only_and_foreign_reply() {
        let root = std::env::temp_dir().join(format!("sirus-acp-wire-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let script = r#"import json,sys,time
read=lambda:json.loads(sys.stdin.readline())
send=lambda v:print(json.dumps(v),flush=True)
a=read();assert a['params']['clientCapabilities']=={};send({'id':a['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True}}})
a=read();assert a['method']=='session/load';assert a['params']['sessionId']=='native';cwd=a['params']['cwd'];send({'method':'session/update','params':{'sessionId':'native','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'REPLAY_MUST_NOT_APPEND'}}}});send({'id':a['id'],'result':{}})
a=read();assert a['method']=='session/prompt';assert a['params']['prompt']==[{'type':'text','text':'new prompt'}];turn=a['id']
send({'id':7,'method':'session/request_permission','params':{'sessionId':'native','toolCall':{'kind':'edit','toolCallId':'edit7','content':[{'type':'diff','path':cwd+'/file.txt','oldText':'','newText':'new'}]},'options':[{'optionId':'once','kind':'allow_once'},{'optionId':'always','kind':'allow_always'},{'optionId':'reject','kind':'reject_once'}]}})
a=read();assert a['id']==7;assert a['result']['outcome']=={'outcome':'selected','optionId':'once'};time.sleep(.2)
send({'method':'session/update','params':{'sessionId':'native','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'fresh'}}}});send({'id':turn,'result':{'stopReason':'end_turn'}})
"#;
        let mut child = Command::new("python3")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut wire = Wire::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
        let mut session = session(&cwd);
        session.native_thread = Some(NativeThread {
            provider_account_id: "default".into(),
            thread_id: "native".into(),
            session_id: "s".into(),
            project_id: "p".into(),
            cwd: cwd.clone(),
            model: None,
        });
        let (sender, replies) = mpsc::channel(16);
        let mut run = Run {
            session,
            cwd,
            prompt: "new prompt".into(),
            attachments: Vec::new(),
            generation: "generation".into(),
            replies,
        };
        let (_cancel, mut cancel) = watch::channel(false);
        let mut output = String::new();
        let mut acks = Vec::new();
        let mut callback = |event| {
            match event {
                Event::Pending(p) => {
                    for generation in ["foreign", "generation", "generation"] {
                        let (result, ack) = tokio::sync::oneshot::channel();
                        sender
                            .try_send(crate::codex::Inbound::Answer(crate::codex::Reply {
                                request: RespondAgentRequest {
                                    session_id: "s".into(),
                                    request_id: p.request_id.clone(),
                                    generation: generation.into(),
                                    turn_id: p.turn_id.clone(),
                                    response: AgentResponse::Approval {
                                        decision: ApprovalDecision::Accept,
                                    },
                                },
                                result,
                            }))
                            .unwrap();
                        acks.push(ack);
                    }
                }
                Event::Delta(c) => output.push_str(&c),
                _ => {}
            }
            Ok(())
        };
        assert!(!execute(
            &mut wire,
            &mut run,
            &mut cancel,
            &mut mpsc::channel(1).1,
            &mut callback
        )
        .await
        .unwrap());
        assert_eq!(output, "fresh");
        let mut it = acks.into_iter();
        assert!(it.next().unwrap().await.unwrap().is_err());
        assert!(it.next().unwrap().await.unwrap().is_ok());
        assert!(it.next().unwrap().await.unwrap().is_err());
        assert!(child.wait().await.unwrap().success());
    }
    async fn live_turn(
        session: &Session,
        prompt: &str,
        decision: ApprovalDecision,
    ) -> (NativeThread, String, usize) {
        let (process, mut started) = start(
            session.clone(),
            session.worktree.path.clone(),
            prompt.into(),
            HashMap::new(),
        )
        .await
        .unwrap();
        let sender = process.replies.as_ref().unwrap().clone();
        let mut run = started.codex.take().unwrap();
        let stderr = started.child.stderr.take().unwrap();
        let diagnostics = tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stderr);
            while crate::agent::bounded_line(&mut reader)
                .await
                .is_ok_and(|line| line.is_some())
            {}
        });
        let mut wire = Wire::new(
            started.child.stdin.take().unwrap(),
            started.child.stdout.take().unwrap(),
        );
        let mut native = None;
        let mut output = String::new();
        let mut approvals = 0;
        let mut callback = |event| {
            match event {
                Event::Identity(identity) => native = Some(identity),
                Event::Delta(chunk) => output.push_str(&chunk),
                Event::Pending(pending) => {
                    approvals += 1;
                    if !matches!(decision, ApprovalDecision::Decline)
                        && !matches!(&pending.kind,PendingRequestKind::FileChange{changes,..} if changes.iter().all(|c|c.path.ends_with("/sirus-opencode.txt")))
                    {
                        return Err(Error::agent("Unexpected tool in harmless fixture"));
                    }
                    let (result, _receiver) = tokio::sync::oneshot::channel();
                    sender
                        .try_send(crate::codex::Inbound::Answer(crate::codex::Reply {
                            request: RespondAgentRequest {
                                session_id: session.id.clone(),
                                generation: pending.generation,
                                turn_id: pending.turn_id,
                                request_id: pending.request_id,
                                response: AgentResponse::Approval {
                                    decision: decision.clone(),
                                },
                            },
                            result,
                        }))
                        .map_err(|_| Error::agent("Fixture response unavailable"))?;
                }
                Event::Answered(_)
                | Event::Activity(_)
                | Event::Model(_)
                | Event::Context(_)
                | Event::Compacted(_)
                | Event::UsageLimit(_)
                | Event::Feed(_) => {}
            }
            Ok(())
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(120),
            execute(
                &mut wire,
                &mut run,
                &mut started.cancel,
                &mut mpsc::channel(1).1,
                &mut callback,
            ),
        )
        .await;
        drop(wire);
        process.stop().unwrap();
        let _ = started.child.wait().await;
        diagnostics.abort();
        let _ = diagnostics.await;
        assert!(
            matches!(outcome, Ok(Ok(stopped)) if stopped==matches!(decision,ApprovalDecision::Cancel)),
            "OpenCode native turn failed: {outcome:?}; native metadata omitted"
        );
        eprintln!("Disposable fixture approvals: {approvals}; assistant: {output}");
        (native.expect("native identity"), output, approvals)
    }

    #[tokio::test]
    #[ignore = "real existing OpenCode login; isolated disposable fixture"]
    async fn live_opencode_write_and_exact_followup() {
        let root = std::env::temp_dir().join(format!("sirus-acp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = root.canonicalize().unwrap().to_string_lossy().into_owned();
        std::fs::write(
            root.join("opencode.json"),
            r#"{"permission":{"*":"allow","edit":"allow","bash":"allow"},"agent":{"build":{"permission":{"*":"allow","edit":"allow","bash":"allow"}}}}"#,
        )
        .unwrap();
        let mut s = session(&cwd);
        s.model = Some("opencode/big-pickle".into());
        let (native,text,count)=live_turn(&s,"Use the write or apply_patch tool to create sirus-opencode.txt containing exactly ACP_NATIVE_7. Do not run shell commands. Remember secret word TANGERINE7 for the next turn.",ApprovalDecision::Accept).await;
        assert!(count > 0, "No host approval. Fixture output: {text}");
        assert_eq!(
            std::fs::read_to_string(root.join("sirus-opencode.txt"))
                .unwrap()
                .trim(),
            "ACP_NATIVE_7"
        );
        s.native_thread = Some(native);
        let (_,text,_)=live_turn(&s,"What secret word did I ask you to remember? Reply only with that word. Do not use tools.",ApprovalDecision::Decline).await;
        assert!(
            text.contains("TANGERINE7"),
            "exact resumed context unavailable"
        );
    }
    #[tokio::test]
    #[ignore = "real existing OpenCode login; isolated disposable fixture"]
    async fn live_opencode_refusal() {
        let root = std::env::temp_dir().join(format!("sirus-acp-deny-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let (_,_,n)=live_turn(&session(&cwd),"Use write or apply_patch to create sirus-opencode.txt containing ACP_NATIVE_7. Do not run shell commands.",ApprovalDecision::Decline).await;
        assert!(n > 0);
        assert!(!root.join("sirus-opencode.txt").exists());
    }
    #[tokio::test]
    #[ignore = "real existing OpenCode login; isolated disposable fixture"]
    async fn live_opencode_cancel_pending_edit() {
        let root = std::env::temp_dir().join(format!("sirus-acp-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let (_,_,count)=live_turn(&session(&cwd),"Use write or apply_patch to create sirus-opencode.txt containing ACP_NATIVE_7. Do not run shell commands.",ApprovalDecision::Cancel).await;
        assert!(count > 0);
        assert!(!root.join("sirus-opencode.txt").exists());
    }
}
