//! ACP command-line agents: Devin (`devin acp`) and Hermes Agent
//! (`hermes acp`). Native sessions resume exactly; mode, model and reasoning are selected and
//! confirmed; cancellation rejects leftover requests; approvals are answered once only.
//!
//! Each CLI uses its own sign-in or provider setup on this Mac; Sirus starts no login. Modes map
//! to each agent's own (`mode`): Devin's `plan`, `bypass` and `accept-edits` (it accepts edits and
//! asks before commands); Hermes' `default` (asks for everything), `accept_edits` and `dont_ask`.
//! An approval is answered with the once-only option, never an "always" one, so a single yes
//! never widens the permission mode. Planning denies writes whatever the agent asks.
use crate::codex::{Event, Run, Wire};
use crate::error::{Error, Result};
use crate::models::*;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    process::Stdio,
    time::Duration,
};
use tokio::sync::{mpsc, watch};

/// Providers that run through this module.
pub fn handles(provider: &AgentProviderId) -> bool {
    matches!(
        provider,
        AgentProviderId::Devin | AgentProviderId::Hermes | AgentProviderId::Cursor
    )
}

fn name(provider: &AgentProviderId) -> &'static str {
    match provider {
        AgentProviderId::Hermes => "Hermes",
        AgentProviderId::Cursor => "Cursor",
        _ => "Devin",
    }
}

/// What to run when setup fails for want of sign-in or a configured model provider.
fn sign_in_help(provider: &AgentProviderId) -> &'static str {
    match provider {
        AgentProviderId::Hermes => "Hermes has no model provider yet. Run `hermes model` in a terminal, check it with `hermes acp --check`, then send again.",
        AgentProviderId::Cursor => "Cursor is not signed in. Run `cursor-agent login` in a terminal, then send again.",
        _ => "Devin is not signed in. Run `devin auth login` in a terminal, then send again.",
    }
}

pub fn validate_binding(session: &Session, cwd: &str) -> Result<()> {
    if session.native_thread.as_ref().is_some_and(|n| {
        !handles(&session.agent)
            || n.session_id != session.id
            || n.project_id != session.project_id
            || n.cwd != cwd
            || n.provider_account_id != session.provider_account_id
            || n.thread_id.is_empty()
            || n.thread_id.len() > 1024
    }) {
        return Err(Error::agent(format!(
            "{} native identity does not match session, project or workspace",
            name(&session.agent)
        )));
    }
    Ok(())
}

/// These agents ask about commands, file edits and other tools; each takes one decision.
pub fn validate_response(pending: &PendingRequest, response: &AgentResponse) -> Result<()> {
    if !matches!(
        (&pending.kind, response),
        (
            PendingRequestKind::FileChange { .. }
                | PendingRequestKind::Command { .. }
                | PendingRequestKind::Tool { .. },
            AgentResponse::Approval { .. }
        )
    ) {
        return Err(Error::agent(
            "The agent accepts only a decision for its request",
        ));
    }
    Ok(())
}

/// The agent's own session mode for the chosen execution options.
pub fn mode(provider: &AgentProviderId, options: &ExecutionOptions) -> &'static str {
    match provider {
        // Cursor asks before tools in Agent mode; Sirus answers by access mode (`auto_answer`).
        AgentProviderId::Cursor => {
            if options.planning {
                "plan"
            } else {
                "agent"
            }
        }
        AgentProviderId::Hermes => {
            if options.planning || options.approval == Some(ApprovalMode::Ask) {
                "default"
            } else if options.approval == Some(ApprovalMode::Full) {
                "dont_ask"
            } else {
                "accept_edits"
            }
        }
        _ => {
            if options.planning {
                "plan"
            } else if options.approval == Some(ApprovalMode::Full) {
                "bypass"
            } else {
                "accept-edits"
            }
        }
    }
}

/// Requests Sirus answers itself for agents without a matching mode (Cursor): Full
/// access allows every request, Auto-review allows file edits; anything else asks the person.
/// Planning never auto-allows.
pub fn auto_answer(
    provider: &AgentProviderId,
    options: &ExecutionOptions,
    kind: &PendingRequestKind,
) -> bool {
    if *provider != AgentProviderId::Cursor || options.planning {
        return false;
    }
    match options.approval {
        Some(ApprovalMode::Full) => true,
        Some(ApprovalMode::Auto) => matches!(kind, PendingRequestKind::FileChange { .. }),
        _ => false,
    }
}

/// The offered model value for a catalog ID: the exact value, else the one whose base name (before
/// `[`) matches, as Cursor lists one parameterized value per model. A legacy preset ID such as
/// `gpt-5.5-high-fast` falls back to its base model.
pub fn offered_model(row: &Value, model: &str) -> Option<String> {
    let values: Vec<String> = row["options"]
        .as_array()?
        .iter()
        .flat_map(|option| {
            let mut found = vec![option["value"].as_str().map(str::to_owned)];
            for inner in option["options"].as_array().into_iter().flatten() {
                found.push(inner["value"].as_str().map(str::to_owned));
            }
            found
        })
        .flatten()
        .collect();
    if values.iter().any(|value| value == model) {
        return Some(model.to_owned());
    }
    let base = |value: &str| value.split('[').next().unwrap_or(value).to_owned();
    let by_base = |wanted: &str| values.iter().find(|value| base(value) == wanted).cloned();
    by_base(model).or_else(|| {
        let mut stripped = model
            .trim_end_matches("-fast")
            .trim_end_matches("-thinking")
            .to_owned();
        for effort in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
            if let Some(rest) = stripped.strip_suffix(&format!("-{effort}")) {
                stripped = rest.to_owned();
                break;
            }
        }
        by_base(&stripped)
    })
}

pub async fn start(
    session: Session,
    cwd: String,
    prompt: String,
    overrides: HashMap<AgentProviderId, String>,
) -> Result<(crate::agent::AgentProcess, crate::agent::StartedAgent)> {
    validate_binding(&session, &cwd)?;
    let provider = session.agent.clone();
    let install = crate::detect::resolve_with_overrides(&provider, &overrides)
        .ok_or_else(|| Error::agent("Unknown provider"))?;
    if !install.installed {
        return Err(Error::agent(format!(
            "{} is not installed",
            name(&provider)
        )));
    }
    let binary = install.path.unwrap_or(install.binary);
    // MCP servers go with the session, so computer use turning on or off needs a new process.
    let key = format!(
        "{binary}\u{0}{cwd}\u{0}{}",
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
        .arg("acp")
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Logs and provider warnings on stderr are drained, never shown as messages.
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    crate::process_priority::lower_async(&mut command);
    let child = command
        .spawn()
        .map_err(|_| Error::agent(format!("Cannot start {} ACP", name(&provider))))?;
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

async fn request(
    provider: &AgentProviderId,
    wire: &mut Wire,
    method: &str,
    params: Value,
    cancel: &mut watch::Receiver<bool>,
    updates: &mut Vec<Value>,
) -> Result<Value> {
    let id = uuid::Uuid::new_v4().to_string();
    wire.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
        .await?;
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let v = tokio::select! {
                v = wire.read() => v?,
                _ = cancel.changed() => return Err(Error::agent(format!("{} startup cancelled", name(provider)))),
            };
            if v["id"] == id && v.get("method").is_none() {
                if v.get("error").is_some() {
                    return Err(setup_error(provider, method, &v));
                }
                if let Some(options) = v["result"]["configOptions"].as_array() {
                    if updates.len() < 64 {
                        updates.push(json!({"configOptions": options}));
                    }
                }
                return v
                    .get("result")
                    .cloned()
                    .ok_or_else(|| Error::agent(format!("{} response lacks result", name(provider))));
            }
            if v.get("id").is_some() && v.get("method").is_some() {
                wire.send(crate::opencode::unsupported(&v)).await?;
            } else if v["method"] == "session/update" && updates.len() < 64 {
                // Option updates (the model list arrives after sign-in checks) are kept;
                // replayed history of a loaded session is discarded.
                if v["params"]["update"]["sessionUpdate"] == "config_option_update" {
                    updates.push(v["params"]["update"].clone());
                }
            }
        }
    })
    .await
    .map_err(|_| Error::agent(format!("{} startup timed out", name(provider))))?
}

fn setup_error(provider: &AgentProviderId, method: &str, value: &Value) -> Error {
    // Hermes puts the reason in `data.details` under a generic "Internal error".
    let message = format!(
        "{} {}",
        value["error"]["message"].as_str().unwrap_or_default(),
        value["error"]["data"]["details"]
            .as_str()
            .unwrap_or_default()
    )
    .to_ascii_lowercase();
    if [
        "auth",
        "log in",
        "logged in",
        "no llm provider",
        "provider configured",
        "hermes model",
        "api key",
    ]
    .iter()
    .any(|needle| message.contains(needle))
    {
        return Error::agent(sign_in_help(provider));
    }
    Error::agent(format!(
        "{} rejected {method}{}; check the CLI and its setup",
        name(provider),
        crate::opencode::rpc_code(value)
    ))
}

/// The latest config options: the setup result, then any later option updates.
fn options_of(setup: &Value, updates: &[Value]) -> Vec<Value> {
    updates
        .iter()
        .rev()
        .find_map(|update| update["configOptions"].as_array().cloned())
        .or_else(|| setup["configOptions"].as_array().cloned())
        .unwrap_or_default()
}

fn select_option<'a>(options: &'a [Value], category: &str) -> Option<&'a Value> {
    options.iter().find(|row| {
        row["type"] == "select" && (row["category"] == category || row["id"] == category)
    })
}

fn offers(row: &Value, value: &str) -> bool {
    row["options"].as_array().is_some_and(|values| {
        values.iter().any(|option| {
            option["value"] == value
                || option["options"]
                    .as_array()
                    .is_some_and(|group| group.iter().any(|inner| inner["value"] == value))
        })
    })
}

fn confirmed(result: &Value, id: &Value, value: &str) -> bool {
    result["configOptions"].as_array().is_none_or(|rows| {
        rows.iter()
            .any(|row| row["id"] == *id && row["currentValue"] == value)
    })
}

/// A permission request as a Sirus request, with the once-only allow and reject options.
fn permission(
    provider: &AgentProviderId,
    params: &Value,
    cwd: &str,
    planning: bool,
) -> Option<(PendingRequestKind, String, String)> {
    if params.to_string().len() > 128 * 1024 {
        return None;
    }
    let call = &params["toolCall"];
    if call["kind"] == "edit" {
        // Planning denies writes, whatever the CLI asks.
        if planning {
            return None;
        }
        let (kind, allow, reject) = crate::opencode::approval(params, cwd)?;
        let kind = match kind {
            PendingRequestKind::FileChange { changes, .. } => PendingRequestKind::FileChange {
                reason: Some(format!("{} proposed edit", name(provider))),
                changes,
            },
            other => other,
        };
        return Some((kind, allow, reject));
    }
    let options = params["options"].as_array()?;
    let once = |kind: &str| {
        let mut rows = options.iter().filter(|row| row["kind"] == kind);
        let id = rows.next()?["optionId"].as_str()?.to_owned();
        (rows.next().is_none() && !id.is_empty() && id.len() <= 1024).then_some(id)
    };
    let (allow, reject) = (once("allow_once")?, once("reject_once")?);
    let title: String = call["title"]
        .as_str()
        .unwrap_or("Agent tool")
        .chars()
        .filter(|c| !c.is_control())
        .take(500)
        .collect();
    let raw = &call["rawInput"];
    let kind = if call["kind"] == "execute" {
        let command = raw["command"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                raw["command"].as_array().map(|parts| {
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
            })
            .unwrap_or_else(|| title.clone());
        PendingRequestKind::Command {
            command: command.chars().take(8 * 1024).collect(),
            cwd: raw["cwd"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| Some(cwd.to_owned())),
            reason: None,
        }
    } else {
        PendingRequestKind::Tool {
            name: title,
            input: if raw.to_string().len() <= 16 * 1024 {
                raw.clone()
            } else {
                Value::Null
            },
            reason: None,
            asked_by: None,
        }
    };
    Some((kind, allow, reject))
}

pub(crate) async fn execute(
    wire: &mut Wire,
    run: &mut Run,
    cancel: &mut watch::Receiver<bool>,
    emit: &mut impl FnMut(Event) -> Result<()>,
) -> Result<bool> {
    let mut updates = Vec::new();
    let provider = run.session.agent.clone();
    let label = name(&provider);
    // A kept process (agent_pool) was initialized by its first turn.
    let init = if wire.initialized {
        wire.acp_init.clone()
    } else {
        let init = request(
            &provider,
            wire,
            "initialize",
            json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"Sirus","version":"0.1.0"}}),
            cancel,
            &mut updates,
        )
        .await?;
        wire.initialized = true;
        wire.acp_init = init.clone();
        init
    };
    if init["protocolVersion"] != 1 {
        return Err(Error::agent(format!("Unsupported {label} ACP version")));
    }
    let existing = run
        .session
        .native_thread
        .as_ref()
        .map(|n| n.thread_id.clone());
    if existing.is_some() && init["agentCapabilities"]["loadSession"] != true {
        return Err(Error::agent(format!(
            "{label} does not support exact session loading"
        )));
    }
    // A kept process already holds this session loaded: no reload, the last setup stands.
    let reused = existing.is_some() && wire.loaded == existing;
    let mut mcp_servers = crate::opencode::acp_mcp_servers(&run.session.id);
    // Connectors (ADR-137), when this agent takes HTTP MCP servers.
    mcp_servers.extend(crate::connectors::acp_servers(
        &crate::connectors::for_session(&run.session).await,
        &init,
    ));
    let setup = if reused {
        wire.acp_setup.clone()
    } else {
        request(
            &provider,
            wire,
            if existing.is_some() {
                "session/load"
            } else {
                "session/new"
            },
            match &existing {
                Some(id) => json!({"sessionId":id,"cwd":run.cwd,"mcpServers":mcp_servers}),
                None => json!({"cwd":run.cwd,"mcpServers":mcp_servers}),
            },
            cancel,
            &mut updates,
        )
        .await?
    };
    let native = existing
        .or_else(|| setup["sessionId"].as_str().map(str::to_owned))
        .filter(|id| !id.is_empty() && id.len() <= 1024)
        .ok_or_else(|| Error::agent(format!("{label} returned invalid native identity")))?;
    wire.loaded = Some(native.clone());
    wire.acp_setup = setup.clone();
    let planning = run.session.execution.planning;
    let wanted_mode = mode(&provider, &run.session.execution);
    // Devin selects through config options; Hermes through session modes and models.
    let options = options_of(&setup, &updates);
    if let Some(row) = select_option(&options, "mode").cloned() {
        if !offers(&row, wanted_mode) {
            return Err(Error::agent(format!(
                "{label} does not offer the selected mode"
            )));
        }
        let result = request(
            &provider,
            wire,
            "session/set_config_option",
            json!({"sessionId":native,"configId":row["id"],"value":wanted_mode}),
            cancel,
            &mut updates,
        )
        .await?;
        if !confirmed(&result, &row["id"], wanted_mode) {
            return Err(Error::agent(format!(
                "{label} did not confirm the selected mode"
            )));
        }
    } else if setup["modes"]["availableModes"]
        .as_array()
        .is_some_and(|modes| modes.iter().any(|row| row["id"] == wanted_mode))
    {
        if reused || setup["modes"]["currentModeId"] != wanted_mode {
            request(
                &provider,
                wire,
                "session/set_mode",
                json!({"sessionId":native,"modeId":wanted_mode}),
                cancel,
                &mut updates,
            )
            .await?;
        }
    } else {
        return Err(Error::agent(format!(
            "{label} does not offer the selected mode"
        )));
    }
    if let Some(model) = run.session.model.clone() {
        let options = options_of(&setup, &updates);
        if let Some(row) = select_option(&options, "model").cloned() {
            // An empty list means the catalog has not loaded yet; the agent still validates the value.
            let listed = row["options"]
                .as_array()
                .is_some_and(|values| !values.is_empty());
            let model = if listed {
                offered_model(&row, &model).ok_or_else(|| {
                    Error::agent(format!("{label} does not offer the selected model"))
                })?
            } else {
                model
            };
            let result = request(
                &provider,
                wire,
                "session/set_config_option",
                json!({"sessionId":native,"configId":row["id"],"value":model}),
                cancel,
                &mut updates,
            )
            .await?;
            if !confirmed(&result, &row["id"], &model) {
                return Err(Error::agent(format!(
                    "{label} did not confirm the selected model"
                )));
            }
        } else if let Some(models) = setup["models"]["availableModels"].as_array() {
            if !models.iter().any(|row| row["modelId"] == model) {
                return Err(Error::agent(format!(
                    "{label} does not offer the selected model"
                )));
            }
            if reused || setup["models"]["currentModelId"] != model {
                request(
                    &provider,
                    wire,
                    "session/set_model",
                    json!({"sessionId":native,"modelId":model}),
                    cancel,
                    &mut updates,
                )
                .await?;
            }
        } else {
            return Err(Error::agent(format!(
                "{label} model selection is unavailable"
            )));
        }
    }
    // Reasoning follows what the selected model offers; nothing is sent when it offers none.
    if let Some(effort) = run.session.execution.effort.clone() {
        let options = options_of(&setup, &updates);
        if let Some(row) = select_option(&options, "thought_level").cloned() {
            if offers(&row, &effort) {
                let result = request(
                    &provider,
                    wire,
                    "session/set_config_option",
                    json!({"sessionId":native,"configId":row["id"],"value":effort}),
                    cancel,
                    &mut updates,
                )
                .await?;
                if !confirmed(&result, &row["id"], &effort) {
                    return Err(Error::agent(format!(
                        "{label} did not confirm the reasoning level"
                    )));
                }
            }
        }
    }
    // Fast mode is its own option on models that offer it (Cursor).
    let options = options_of(&setup, &updates);
    if let Some(row) = options
        .iter()
        .find(|row| row["id"] == "fast" && row["type"] == "select")
        .cloned()
    {
        let fast = if run.session.execution.fast {
            "true"
        } else {
            "false"
        };
        if offers(&row, fast) && row["currentValue"] != fast {
            request(
                &provider,
                wire,
                "session/set_config_option",
                json!({"sessionId":native,"configId":row["id"],"value":fast}),
                cancel,
                &mut updates,
            )
            .await?;
        }
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
    let prompt = crate::attachments::opencode_input(
        &run.prompt,
        &run.attachments,
        &init["agentCapabilities"]["promptCapabilities"],
    )?;
    wire.send(json!({"jsonrpc":"2.0","id":turn,"method":"session/prompt","params":{"sessionId":native,"prompt":prompt}}))
        .await?;
    let mut pending: HashMap<String, (Value, PendingRequest, String, String)> = HashMap::new();
    let mut seen = HashSet::new();
    let mut total = 0usize;
    let mut interrupted = false;
    let mut deadline = None;
    loop {
        let v = tokio::select! {
            v = wire.read() => v?,
            _ = cancel.changed(), if !interrupted => {
                // Leftover permission requests are rejected, then the turn is cancelled.
                for (opaque, (id, _, _, _)) in pending.drain() {
                    wire.send(crate::opencode::denied(&id)).await?;
                    emit(Event::Answered(opaque))?;
                }
                interrupted = true;
                wire.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":native}})).await?;
                deadline = Some(tokio::time::Instant::now() + Duration::from_secs(2));
                continue;
            },
            _ = async { if let Some(d) = deadline { tokio::time::sleep_until(d).await; } else { std::future::pending::<()>().await; } }, if interrupted => return Ok(true),
            Some(inbound) = run.replies.recv(), if !interrupted => {
                let reply = match inbound {
                    crate::codex::Inbound::Answer(reply) => reply,
                    crate::codex::Inbound::Steer { result, .. } => { let _ = result.send(Err(Error::agent(format!("{label} does not accept instructions during a turn")))); continue; }
                    crate::codex::Inbound::StopTask { result, .. } => { let _ = result.send(Err(Error::agent(format!("{label} has no background tasks to stop")))); continue; }
                    crate::codex::Inbound::SteerSubagent { result, .. } => { let _ = result.send(Err(Error::agent(format!("{label} subagents cannot receive messages")))); continue; }
                    crate::codex::Inbound::BackgroundTask { result, .. } => { let _ = result.send(Err(Error::agent(format!("{label} cannot move a subagent to the background")))); continue; }
                };
                let r = &reply.request;
                let mut delivery_failed = false;
                let result = if r.session_id != run.session.id || r.generation != run.generation || r.turn_id != turn {
                    Err(Error::agent(format!("Stale or foreign {label} reply")))
                } else if let Some((id, entry, allow, reject)) = pending.get(&r.request_id) {
                    match validate_response(entry, &r.response) {
                        Err(error) => Err(error),
                        Ok(()) => {
                            let outcome = match &r.response {
                                AgentResponse::Approval { decision: ApprovalDecision::Accept } => json!({"outcome":"selected","optionId":allow}),
                                AgentResponse::Approval { decision: ApprovalDecision::Decline } => json!({"outcome":"selected","optionId":reject}),
                                _ => json!({"outcome":"cancelled"}),
                            };
                            let id = id.clone();
                            pending.remove(&r.request_id);
                            emit(Event::Answered(r.request_id.clone()))?;
                            let sent = wire.send(json!({"jsonrpc":"2.0","id":id,"result":{"outcome":outcome}})).await;
                            delivery_failed = sent.is_err();
                            if matches!(r.response, AgentResponse::Approval { decision: ApprovalDecision::Cancel }) {
                                for (opaque, (id, _, _, _)) in pending.drain() {
                                    wire.send(crate::opencode::denied(&id)).await?;
                                    emit(Event::Answered(opaque))?;
                                }
                                interrupted = true;
                                wire.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":native}})).await?;
                                deadline = Some(tokio::time::Instant::now() + Duration::from_secs(2));
                            }
                            sent
                        }
                    }
                } else {
                    Err(Error::agent(format!("{label} request already answered")))
                };
                let _ = reply.result.send(result);
                if delivery_failed { return Err(Error::agent(format!("{label} reply delivery failed"))); }
                continue;
            }
        };
        if v["id"] == turn && v.get("method").is_none() {
            if interrupted {
                return Ok(true);
            }
            if v.get("error").is_some() {
                return Err(setup_error(&provider, "the prompt", &v));
            }
            return match v["result"]["stopReason"].as_str() {
                Some("end_turn") => {
                    // The session is idle again: the process can be kept for the next turn.
                    wire.reusable = true;
                    Ok(false)
                }
                Some("cancelled") => Ok(true),
                Some("max_tokens") | Some("max_turn_requests") => {
                    Err(Error::agent(format!("{label} stopped at its turn limit")))
                }
                Some("refusal") => Err(Error::agent(format!("{label} declined this request"))),
                _ => Err(Error::agent(format!(
                    "{label} stopped without successful completion"
                ))),
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
                return Err(Error::agent(format!(
                    "Invalid or duplicate {label} callback"
                )));
            }
            if interrupted
                || v["method"] != "session/request_permission"
                || v["params"]["sessionId"] != native
            {
                wire.send(crate::opencode::unsupported(&v)).await?;
                continue;
            }
            let Some((kind, allow, reject)) =
                permission(&provider, &v["params"], &run.cwd, planning)
            else {
                wire.send(crate::opencode::denied(&id)).await?;
                continue;
            };
            if auto_answer(&provider, &run.session.execution, &kind) {
                wire.send(json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"selected","optionId":allow}}})).await?;
                continue;
            }
            if pending.len() >= 32 {
                return Err(Error::agent(format!("Too many {label} requests")));
            }
            let opaque = uuid::Uuid::new_v4().to_string();
            let entry = PendingRequest {
                request_id: opaque.clone(),
                generation: run.generation.clone(),
                turn_id: turn.clone(),
                item_id: v["params"]["toolCall"]["toolCallId"]
                    .as_str()
                    .unwrap_or("devin-request")
                    .chars()
                    .take(1024)
                    .collect(),
                kind,
            };
            pending.insert(opaque, (id, entry.clone(), allow, reject));
            emit(Event::Pending(entry))?;
        } else if v["method"] == "session/update"
            && v["params"]["sessionId"] == native
            && !interrupted
        {
            let update = &v["params"]["update"];
            // Tool calls, plans and subagent work become activity rows, as for OpenCode.
            let items = crate::activity::opencode(update);
            if !items.is_empty() {
                emit(Event::Activity(items))?;
            }
            if update["sessionUpdate"] != "agent_message_chunk" {
                continue;
            }
            if update["content"]["type"] == "text" {
                if let Some(chunk) = update["content"]["text"].as_str() {
                    total += chunk.len();
                    if total > 8 * 1024 * 1024 {
                        return Err(Error::agent(format!("{label} output exceeds limit")));
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
    use tokio::process::Command;

    fn session(cwd: &str) -> Session {
        serde_json::from_value(json!({"id":"s","projectId":"p","title":"fixture","agent":"devin","status":"running","createdAt":"time","lastActivityAt":"time","worktree":{"path":cwd,"branch":"main","isolated":false},"messages":[]})).unwrap()
    }

    #[test]
    fn modes_follow_planning_and_access() {
        let options = |value: Value| serde_json::from_value::<ExecutionOptions>(value).unwrap();
        let devin = AgentProviderId::Devin;
        let hermes = AgentProviderId::Hermes;
        assert_eq!(mode(&devin, &options(json!({}))), "accept-edits");
        assert_eq!(
            mode(&devin, &options(json!({"approval":"ask"}))),
            "accept-edits"
        );
        assert_eq!(
            mode(&devin, &options(json!({"approval":"auto"}))),
            "accept-edits"
        );
        assert_eq!(mode(&devin, &options(json!({"approval":"full"}))), "bypass");
        assert_eq!(mode(&devin, &options(json!({"planning":true}))), "plan");
        assert_eq!(
            mode(&hermes, &options(json!({"approval":"ask"}))),
            "default"
        );
        assert_eq!(
            mode(&hermes, &options(json!({"approval":"auto"}))),
            "accept_edits"
        );
        assert_eq!(
            mode(&hermes, &options(json!({"approval":"full"}))),
            "dont_ask"
        );
        assert_eq!(
            mode(
                &hermes,
                &options(json!({"approval":"full","planning":true}))
            ),
            "default"
        );
    }

    #[test]
    fn permissions_use_once_options_and_planning_denies_edits() {
        let command = json!({"toolCall":{"toolCallId":"t","kind":"execute","title":"Run tests","rawInput":{"command":"npm test"}},"options":[{"optionId":"a","kind":"allow_once"},{"optionId":"A","kind":"allow_always"},{"optionId":"r","kind":"reject_once"}]});
        let (kind, allow, reject) =
            permission(&AgentProviderId::Devin, &command, "/w", false).unwrap();
        assert!(
            matches!(kind, PendingRequestKind::Command { ref command, .. } if command == "npm test")
        );
        assert_eq!((allow.as_str(), reject.as_str()), ("a", "r"));
        let edit = json!({"toolCall":{"toolCallId":"e","kind":"edit","content":[]},"options":[{"optionId":"a","kind":"allow_once"},{"optionId":"r","kind":"reject_once"}]});
        assert!(permission(&AgentProviderId::Devin, &edit, "/w", true).is_none());
        let only_always = json!({"toolCall":{"toolCallId":"t","kind":"fetch","title":"Fetch"},"options":[{"optionId":"A","kind":"allow_always"},{"optionId":"r","kind":"reject_once"}]});
        assert!(permission(&AgentProviderId::Devin, &only_always, "/w", false).is_none());
    }

    #[test]
    fn cursor_models_resolve_by_base_and_access_answers_by_mode() {
        let row = json!({"id":"model","type":"select","options":[{"value":"default[]"},{"value":"gpt-5.5[context=272k,reasoning=medium,fast=false]"},{"value":"composer-2.5[fast=true]"}]});
        assert_eq!(
            offered_model(&row, "composer-2.5[fast=true]").as_deref(),
            Some("composer-2.5[fast=true]")
        );
        assert_eq!(
            offered_model(&row, "gpt-5.5").as_deref(),
            Some("gpt-5.5[context=272k,reasoning=medium,fast=false]")
        );
        assert_eq!(
            offered_model(&row, "gpt-5.5-high-fast").as_deref(),
            Some("gpt-5.5[context=272k,reasoning=medium,fast=false]")
        );
        assert!(offered_model(&row, "nope").is_none());
        let options = |value: Value| serde_json::from_value::<ExecutionOptions>(value).unwrap();
        let edit = PendingRequestKind::FileChange {
            reason: None,
            changes: vec![],
        };
        let command = PendingRequestKind::Command {
            command: "ls".into(),
            cwd: None,
            reason: None,
        };
        let cursor = AgentProviderId::Cursor;
        assert!(auto_answer(
            &cursor,
            &options(json!({"approval":"full"})),
            &command
        ));
        assert!(auto_answer(
            &cursor,
            &options(json!({"approval":"auto"})),
            &edit
        ));
        assert!(!auto_answer(
            &cursor,
            &options(json!({"approval":"auto"})),
            &command
        ));
        assert!(!auto_answer(
            &cursor,
            &options(json!({"approval":"ask"})),
            &edit
        ));
        assert!(!auto_answer(
            &cursor,
            &options(json!({"approval":"full","planning":true})),
            &edit
        ));
        assert!(!auto_answer(
            &AgentProviderId::Devin,
            &options(json!({"approval":"full"})),
            &command
        ));
        assert_eq!(mode(&cursor, &options(json!({"planning":true}))), "plan");
        assert_eq!(mode(&cursor, &options(json!({"approval":"full"}))), "agent");
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
        assert!(validate_binding(&s, "/other").is_err());
    }

    #[tokio::test]
    async fn sets_mode_and_model_answers_a_command_once_and_streams_the_reply() {
        let root = std::env::temp_dir().join(format!("sirus-devin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let script = r#"import json,sys
read=lambda:json.loads(sys.stdin.readline())
send=lambda v:print(json.dumps(v),flush=True)
opts=lambda mode,model:[{'id':'mode','category':'mode','type':'select','currentValue':mode,'options':[{'value':'accept-edits'},{'value':'plan'},{'value':'bypass'}]},{'id':'model','category':'model','type':'select','currentValue':model,'options':[{'value':'swe-1-6'},{'value':'claude-x'}]}]
a=read();assert a['method']=='initialize';send({'id':a['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True,'promptCapabilities':{'image':True}}}})
a=read();assert a['method']=='session/new';send({'method':'session/update','params':{'sessionId':'n1','update':{'sessionUpdate':'config_option_update','configOptions':opts('accept-edits','swe-1-6')}}});send({'id':a['id'],'result':{'sessionId':'n1','configOptions':opts('accept-edits','swe-1-6')}})
a=read();assert a['method']=='session/set_config_option' and a['params']['configId']=='mode' and a['params']['value']=='bypass';send({'id':a['id'],'result':{'configOptions':opts('bypass','swe-1-6')}})
a=read();assert a['params']['configId']=='model' and a['params']['value']=='claude-x';send({'id':a['id'],'result':{'configOptions':opts('bypass','claude-x')}})
a=read();assert a['method']=='session/prompt';turn=a['id']
send({'id':9,'method':'session/request_permission','params':{'sessionId':'n1','toolCall':{'toolCallId':'c9','kind':'execute','title':'Run','rawInput':{'command':'ls'}},'options':[{'optionId':'once','kind':'allow_once'},{'optionId':'always','kind':'allow_always'},{'optionId':'no','kind':'reject_once'}]}})
a=read();assert a['id']==9 and a['result']['outcome']=={'outcome':'selected','optionId':'once'}
send({'method':'session/update','params':{'sessionId':'n1','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'done'}}}});send({'id':turn,'result':{'stopReason':'end_turn'}})
"#;
        let mut child = Command::new("python3")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut wire = Wire::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
        let mut session = session(&cwd);
        session.model = Some("claude-x".into());
        session.execution = serde_json::from_value(json!({"approval":"full"})).unwrap();
        let (sender, replies) = mpsc::channel(16);
        let mut run = Run {
            session,
            cwd,
            prompt: "hello".into(),
            attachments: Vec::new(),
            generation: "g".into(),
            replies,
        };
        let (_cancel, mut cancel) = watch::channel(false);
        let mut output = String::new();
        let mut identity = None;
        let mut acks = Vec::new();
        let mut emit = |event| {
            match event {
                Event::Pending(pending) => {
                    let (result, ack) = tokio::sync::oneshot::channel();
                    sender
                        .try_send(crate::codex::Inbound::Answer(crate::codex::Reply {
                            request: RespondAgentRequest {
                                session_id: "s".into(),
                                request_id: pending.request_id.clone(),
                                generation: "g".into(),
                                turn_id: pending.turn_id.clone(),
                                response: AgentResponse::Approval {
                                    decision: ApprovalDecision::Accept,
                                },
                            },
                            result,
                        }))
                        .unwrap();
                    acks.push(ack);
                }
                Event::Delta(text) => output.push_str(&text),
                Event::Identity(native) => identity = Some(native.thread_id),
                _ => {}
            }
            Ok(())
        };
        let interrupted = execute(&mut wire, &mut run, &mut cancel, &mut emit)
            .await
            .unwrap();
        assert!(!interrupted);
        assert_eq!(output, "done");
        assert_eq!(identity.as_deref(), Some("n1"));
        let _ = child.wait().await;
        let _ = std::fs::remove_dir_all(root);
    }
}
