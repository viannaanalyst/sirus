//! Codex app-server's typed, native-owned subset. No arbitrary renderer protocol messages.
use crate::error::{Error, Result};
use crate::models::*;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn validate_binding(session: &Session, cwd: &str) -> Result<()> {
    if let Some(identity) = &session.native_thread {
        if session.agent != AgentProviderId::Codex
            || identity.session_id != session.id
            || identity.project_id != session.project_id
            || identity.cwd != cwd
            || identity.model != session.model
            || identity.provider_account_id != session.provider_account_id
            || identity.thread_id.is_empty()
        {
            return Err(Error::agent(
                "Codex thread does not match this session, project, workspace or model",
            ));
        }
    }
    Ok(())
}

pub fn answer(pending: &PendingRequest, response: &AgentResponse) -> Result<Value> {
    match (&pending.kind, response) {
        (
            PendingRequestKind::Command { .. } | PendingRequestKind::FileChange { .. },
            AgentResponse::Approval { decision },
        ) => {
            let decision = match decision {
                ApprovalDecision::Accept => "accept",
                ApprovalDecision::Decline => "decline",
                ApprovalDecision::Cancel => "cancel",
            };
            Ok(json!({"decision": decision}))
        }
        (PendingRequestKind::UserInput { questions }, AgentResponse::UserInput { answers }) => {
            if answers.len() != questions.len() {
                return Err(Error::agent("Answer every native question exactly once"));
            }
            let mut result = serde_json::Map::new();
            for question in questions {
                let values = answers
                    .get(&question.id)
                    .ok_or_else(|| Error::agent("Unknown question identity"))?;
                if question.is_secret {
                    return Err(Error::agent(
                        "Secret input is not supported; use the provider CLI",
                    ));
                }
                if values.len() != 1 || values[0].trim().is_empty() || values[0].len() > 8192 {
                    return Err(Error::agent(
                        "Question requires one non-empty answer of at most 8 KiB",
                    ));
                }
                if let Some(options) = &question.options {
                    if !question.is_other && !options.iter().any(|option| option.label == values[0])
                    {
                        return Err(Error::agent("Answer is not a native offered option"));
                    }
                }
                result.insert(question.id.clone(), json!({"answers": values}));
            }
            Ok(json!({"answers": result}))
        }
        _ => Err(Error::agent(
            "Response does not match the native pending request",
        )),
    }
}

struct PendingLedger {
    session_id: String,
    generation: String,
    turn_id: String,
    requests: HashMap<String, (Value, PendingRequest)>,
}
impl PendingLedger {
    fn resolve(&mut self, vendor_id: &Value) -> Option<String> {
        let opaque = self
            .requests
            .iter()
            .find(|(_, (id, _))| id == vendor_id)
            .map(|(opaque, _)| opaque.clone())?;
        self.requests.remove(&opaque);
        Some(opaque)
    }

    fn take(&mut self, response: &RespondAgentRequest) -> Result<(Value, Value)> {
        if response.session_id != self.session_id
            || response.generation != self.generation
            || response.turn_id != self.turn_id
        {
            return Err(Error::agent("Stale or foreign Codex response"));
        }
        let (id, pending) = self
            .requests
            .get(&response.request_id)
            .ok_or_else(|| Error::agent("Codex request already answered or no longer pending"))?;
        let result = answer(pending, &response.response)?;
        let id = id.clone();
        self.requests.remove(&response.request_id);
        Ok((id, result))
    }
}

use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
#[cfg(test)]
use tokio::process::Command;
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot, watch};

pub struct Reply {
    pub request: RespondAgentRequest,
    pub result: oneshot::Sender<Result<()>>,
}
/// What a running native turn accepts from the person: an answer to a pending
/// request, or an instruction steered into the running reply.
pub enum Inbound {
    Answer(Reply),
    Steer {
        /// The instruction with the attached files' context already in it.
        text: String,
        /// Resolved attachments: images become input items, files are named in `text`.
        attachments: Vec<std::sync::Arc<crate::attachments::PreparedAttachment>>,
        result: oneshot::Sender<Result<()>>,
    },
    /// Stop one background task of the running turn (Claude's `stop_task`, ADR-097).
    StopTask {
        task_id: String,
        result: oneshot::Sender<Result<()>>,
    },
    /// A message for one running Claude subagent, read before its next tool call (ADR-113).
    SteerSubagent {
        task_id: String,
        text: String,
        result: oneshot::Sender<Result<()>>,
    },
    /// Move one running foreground subagent to the background (Claude's Ctrl+B, ADR-111).
    BackgroundTask {
        task_id: String,
        tool_use_id: String,
        result: oneshot::Sender<Result<()>>,
    },
}
pub struct Run {
    pub session: Session,
    pub cwd: String,
    pub prompt: String,
    pub attachments: Vec<std::sync::Arc<crate::attachments::PreparedAttachment>>,
    pub generation: String,
    pub replies: mpsc::Receiver<Inbound>,
}
pub enum Event {
    Identity(NativeThread),
    Delta(String),
    Pending(PendingRequest),
    Answered(String),
    Activity(Vec<crate::activity::ActivityItem>),
    Model(String),
    /// Context-window reading reported by the provider (ADR-057).
    Context(crate::models::ContextUsage),
    /// A compaction finished: the tokens of the rebuilt conversation, without the provider's
    /// own prompt and tools (Claude's `compact_boundary`).
    Compacted(u64),
    /// The turn hit the account's usage limit; the reset time when the provider reported it.
    UsageLimit(Option<i64>),
    /// A subagent's own message, thought or tool (ADR-146): live view only, never in the session.
    Feed(crate::subagent_feed::FeedUpdate),
}

/// `-c` overrides for the integrated browser MCP server, and the environment the
/// Codex process needs for them. Every `-c` value is visible to any local user in
/// `ps`, so the session token is set in Codex's environment and forwarded to the
/// server with `env_vars` (ADR-100); the socket path is not a secret.
fn browser_overrides(
    executable: &str,
    socket: &str,
    token: &str,
    child_env: &[(&'static str, &'static str)],
) -> (Vec<String>, Vec<(&'static str, String)>) {
    const PREFIX: &str = "mcp_servers.sirus_browser";
    let mut args = vec![
        format!("{PREFIX}.command={executable:?}"),
        format!("{PREFIX}.args=[\"--mcp-browser\"]"),
        format!("{PREFIX}.env.{}={socket:?}", crate::browser_mcp::SOCKET_ENV),
        format!("{PREFIX}.env_vars=[{:?}]", crate::browser_mcp::TOKEN_ENV),
        // Integrated browser tools are fixed and bounded; no per-call
        // prompt (the app already interrupts on human page input).
        format!("{PREFIX}.default_tools_approval_mode=\"approve\""),
        // A private secret card waits up to five minutes for the person (ADR-077).
        format!("{PREFIX}.tool_timeout_sec=330"),
    ];
    for (key, value) in child_env {
        args.push(format!("{PREFIX}.env.{key}={value:?}"));
    }
    (
        args,
        vec![(crate::browser_mcp::TOKEN_ENV, token.to_string())],
    )
}

/// Each turn owns one server process. Follow-ups resume the exact persisted thread.
/// No idle process remains after a completed turn; pending callbacks are generation-local.
pub async fn start(
    session: Session,
    cwd: String,
    prompt: String,
    overrides: HashMap<AgentProviderId, String>,
    account_home: Option<std::path::PathBuf>,
) -> Result<(crate::agent::AgentProcess, crate::agent::StartedAgent)> {
    validate_binding(&session, &cwd)?;
    if (session.provider_account_id == "default") != account_home.is_none() {
        return Err(Error::agent("Account profile does not match this session."));
    }
    let install = crate::detect::resolve_with_overrides(&AgentProviderId::Codex, &overrides)
        .ok_or_else(|| Error::agent("Unknown provider"))?;
    if !install.installed {
        return Err(Error::agent("Codex is not installed on this machine"));
    }
    let binary = install.path.unwrap_or(install.binary);
    let connectors = crate::connectors::for_session(&session).await;
    // A change of connectors or their tokens needs a new process.
    let key = format!(
        "{}\u{0}{}",
        codex_pool_key(&session, &cwd, &binary, account_home.as_deref()),
        crate::connectors::fingerprint(&connectors)
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
                #[cfg(unix)]
                pid: child.id(),
                replies: Some(sender),
                generation: Some(generation.clone()),
            },
            crate::agent::StartedAgent {
                review: None,
                child,
                cancel: receiver,
                wire: Some(wire),
                pool_key: Some(key),
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
    let mut cmd = crate::detect::command(binary);
    crate::provider_accounts::apply(&mut cmd, &session.agent, account_home.as_deref());
    // Connectors (ADR-137): their tokens reach Codex through its environment, never argv.
    let (connector_args, connector_env) = crate::connectors::codex_overrides(&connectors);
    for value in connector_args {
        cmd.arg("-c").arg(value);
    }
    for (key, value) in connector_env {
        cmd.env(key, value);
    }
    if let Some(endpoint) = crate::browser_mcp::ensure_endpoint(&session.id) {
        if let Ok(exe) = std::env::current_exe() {
            let (mut args, mut env) = browser_overrides(
                &exe.to_string_lossy(),
                &endpoint.socket.to_string_lossy(),
                &endpoint.token,
                &crate::browser_mcp::child_env(),
            );
            let (computer_args, computer_env) = crate::computer_mcp::codex_overrides(&session.id);
            args.extend(computer_args);
            env.extend(computer_env);
            for value in args {
                cmd.arg("-c").arg(value);
            }
            // Tokens reach the MCP servers through Codex's environment, never argv.
            for (key, value) in env {
                cmd.env(key, value);
            }
        }
    }
    cmd.args(["app-server", "--stdio"])
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    crate::process_priority::lower_async(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|_| Error::agent("Cannot start Codex app-server"))?;
    let generation = uuid::Uuid::new_v4().to_string();
    let (cancel, receiver) = watch::channel(false);
    let (sender, replies) = mpsc::channel(16);
    Ok((
        crate::agent::AgentProcess {
            cancel,
            #[cfg(unix)]
            pid: child.id(),
            replies: Some(sender),
            generation: Some(generation.clone()),
        },
        crate::agent::StartedAgent {
            review: None,
            child,
            cancel: receiver,
            wire: None,
            pool_key: Some(key),
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

/// What a kept app-server must match to be reused.
pub(crate) fn codex_pool_key(
    session: &Session,
    cwd: &str,
    binary: &str,
    account_home: Option<&std::path::Path>,
) -> String {
    // The MCP servers are fixed when the process starts: computer use turning on or off
    // needs a new one.
    let computer = !crate::computer_mcp::codex_overrides(&session.id)
        .0
        .is_empty();
    format!(
        "{binary}\u{0}{}\u{0}{}\u{0}{cwd}\u{0}{computer}",
        session.provider_account_id,
        account_home
            .map(|home| home.to_string_lossy().into_owned())
            .unwrap_or_default()
    )
}

pub(crate) struct Wire {
    /// The process already answered `initialize` (a process kept between turns).
    pub(crate) initialized: bool,
    /// Set by a provider when its turn ended cleanly enough to keep the process (agent_pool).
    pub(crate) reusable: bool,
    /// The native session an ACP process already has loaded (kept between turns), with the
    /// process's `initialize` result and the session's setup result.
    pub(crate) loaded: Option<String>,
    pub(crate) acp_init: serde_json::Value,
    pub(crate) acp_setup: serde_json::Value,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    sequence: u64,
    deferred: std::collections::VecDeque<Value>,
    deferred_bytes: usize,
    line: Vec<u8>,
}
impl Wire {
    pub(crate) fn new(input: ChildStdin, output: ChildStdout) -> Self {
        Self {
            initialized: false,
            reusable: false,
            loaded: None,
            acp_init: Value::Null,
            acp_setup: Value::Null,
            input,
            output: BufReader::new(output),
            sequence: 0,
            deferred: Default::default(),
            deferred_bytes: 0,
            line: Vec::new(),
        }
    }
    pub(crate) async fn send(&mut self, value: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value)?;
        bytes.push(b'\n');
        tokio::time::timeout(Duration::from_secs(5), self.input.write_all(&bytes))
            .await
            .map_err(|_| Error::agent("Native protocol write timed out"))??;
        Ok(())
    }
    pub(crate) async fn read(&mut self) -> Result<Value> {
        use tokio::io::AsyncBufReadExt;
        loop {
            let buffer = self.output.fill_buf().await?;
            if buffer.is_empty() {
                return Err(Error::agent("Native protocol closed its output"));
            }
            let end = buffer.iter().position(|byte| *byte == b'\n');
            let count = end.map_or(buffer.len(), |index| index + 1);
            if self.line.len() + count > 8 * 1024 * 1024 {
                return Err(Error::agent("Native protocol line exceeds limit"));
            }
            self.line.extend_from_slice(&buffer[..count]);
            self.output.consume(count);
            if end.is_some() {
                let line = std::mem::take(&mut self.line);
                return serde_json::from_slice(&line)
                    .map_err(|_| Error::agent("Native protocol sent invalid JSON"));
            }
        }
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Value> {
        self.sequence += 1;
        let id = format!("sirus-{}", self.sequence);
        self.send(json!({"id":id,"method":method,"params":params}))
            .await?;
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let value = tokio::select! {
                    value = self.read() => value?,
                    _ = cancel.changed() => return Err(Error::agent("Codex startup cancelled")),
                };
                if value["id"] == id {
                    if value.get("error").is_some() { return Err(Error::agent(format!("Codex rejected {method}; check CLI compatibility and native thread availability"))); }
                    return value.get("result").cloned().ok_or_else(|| Error::agent("Codex response lacks result"));
                }
                if matches!(method, "turn/start" | "thread/compact/start" | "turn/steer") && value.get("method").is_some() {
                    let size=value.to_string().len();
                    if self.deferred.len() >= 256 || self.deferred_bytes + size > 8*1024*1024 {return Err(Error::agent("Too many early Codex turn events"));}
                    self.deferred_bytes+=size;
                    self.deferred.push_back(value);
                    continue;
                }
                // Startup never authorizes callbacks or displays account/configuration metadata.
                if value.get("id").is_some() && value.get("method").is_some() {
                    self.send(json!({"id":value["id"],"error":{"code":-32601,"message":"Unsupported during initialization"}})).await?;
                }
            }
        }).await.map_err(|_| Error::agent("Codex initialization timed out"))?
    }
}

fn string(value: &Value, name: &str) -> Result<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty() && v.len() < 16 * 1024)
        .map(str::to_owned)
        .ok_or_else(|| Error::agent("Codex protocol identity is invalid"))
}
fn pending_kind(method: &str, params: &Value) -> Result<Option<PendingRequestKind>> {
    Ok(match method {
        "item/commandExecution/requestApproval" => {
            // Stdin/network variants and permission amendments are not generic command approvals.
            if params
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind != "command")
                || params
                    .get("networkApprovalContext")
                    .is_some_and(|v| !v.is_null())
            {
                return Ok(None);
            }
            Some(PendingRequestKind::Command {
                command: string(params, "command")?,
                cwd: params["cwd"].as_str().map(str::to_owned),
                reason: params["reason"].as_str().map(str::to_owned),
            })
        }
        "item/fileChange/requestApproval" => {
            if params["grantRoot"].as_str().is_some() {
                return Ok(None);
            }
            let Some(changes) = params["changes"]
                .as_array()
                .filter(|changes| !changes.is_empty() && changes.len() <= 256)
            else {
                return Ok(None);
            };
            let changes = changes
                .iter()
                .map(|change| {
                    Ok(ProposedFileChange {
                        path: string(change, "path")?,
                        kind: string(&change["kind"], "type")?,
                        diff: change["diff"]
                            .as_str()
                            .ok_or_else(|| Error::agent("Invalid native file diff"))?
                            .into(),
                        move_path: change["kind"]["move_path"].as_str().map(str::to_owned),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Some(PendingRequestKind::FileChange {
                reason: params["reason"].as_str().map(str::to_owned),
                changes,
            })
        }
        "item/tool/requestUserInput" => {
            let questions: Vec<InputQuestion> = serde_json::from_value(params["questions"].clone())
                .map_err(|_| Error::agent("Invalid native questions"))?;
            let unique: std::collections::HashSet<_> =
                questions.iter().map(|question| &question.id).collect();
            if questions.is_empty()
                || questions.len() > 16
                || unique.len() != questions.len()
                || questions.iter().any(|q| q.is_secret)
            {
                return Ok(None);
            }
            Some(PendingRequestKind::UserInput { questions })
        }
        _ => None,
    })
}

async fn execute(
    wire: &mut Wire,
    run: &mut Run,
    cancel: &mut watch::Receiver<bool>,
    emit: &mut impl FnMut(Event) -> Result<()>,
) -> Result<bool> {
    // A kept app-server is initialized once; each turn still resumes and checks its thread.
    if !wire.initialized {
        wire.request("initialize", json!({"clientInfo":{"name":"sirus","title":"Sirus","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),cancel).await?;
        wire.send(json!({"method":"initialized"})).await?;
        wire.initialized = true;
    }
    let (sandbox, approval, reviewer) = crate::execution::codex_policy(&run.session.execution);
    let mut params = json!({"cwd":run.cwd,"sandbox":sandbox,"approvalPolicy":approval,"approvalsReviewer":reviewer});
    if let Some(model) = &run.session.model {
        params["model"] = json!(model);
    }
    params["developerInstructions"] = json!(crate::skills::agent_hint());
    let method = if let Some(identity) = &run.session.native_thread {
        params["threadId"] = json!(identity.thread_id);
        params["excludeTurns"] = json!(true);
        "thread/resume"
    } else {
        "thread/start"
    };
    let thread = wire.request(method, params, cancel).await?;
    let thread_id = string(&thread["thread"], "id")?;
    if thread["cwd"].as_str() != Some(&run.cwd)
        || thread["approvalPolicy"] != approval
        || thread["approvalsReviewer"] != reviewer
        || thread["sandbox"]["type"]
            != if sandbox == "danger-full-access" {
                "dangerFullAccess"
            } else {
                "workspaceWrite"
            }
    {
        return Err(Error::agent(
            "Codex did not honor the selected approval and sandbox policy",
        ));
    }
    if run
        .session
        .native_thread
        .as_ref()
        .is_some_and(|identity| identity.thread_id != thread_id)
    {
        return Err(Error::agent("Codex resumed a different native thread"));
    }
    emit(Event::Identity(NativeThread {
        provider_account_id: run.session.provider_account_id.clone(),
        thread_id: thread_id.clone(),
        session_id: run.session.id.clone(),
        project_id: run.session.project_id.clone(),
        cwd: run.cwd.clone(),
        model: run.session.model.clone(),
    }))?;
    if let Some(model) = thread["model"].as_str() {
        emit(Event::Model(model.into()))?;
    }
    // `/compact` on an existing thread compacts its context instead of starting a turn (ADR-057).
    // It is never sent to the model as a text turn.
    let compact = is_compact(&run.prompt);
    if compact && (run.session.native_thread.is_none() || !run.attachments.is_empty()) {
        return Err(Error::agent(
            "There is nothing to compact yet: this conversation has not started.",
        ));
    }
    let turn_id = if compact {
        wire.request(
            "thread/compact/start",
            json!({"threadId":thread_id}),
            cancel,
        )
        .await?;
        compaction_turn(wire, &thread_id, cancel).await?
    } else {
        let mut turn_params = json!({"threadId":thread_id,"cwd":run.cwd,"approvalPolicy":"on-request","approvalsReviewer":"user","sandboxPolicy":{"type":"workspaceWrite","writableRoots":[run.cwd],"networkAccess":false,"excludeTmpdirEnvVar":true,"excludeSlashTmp":true},"input":[{"type":"text","text":run.prompt,"text_elements":[]} ]});
        turn_params["input"] = crate::attachments::codex_input(&run.prompt, &run.attachments);
        crate::execution::codex_turn(
            &mut turn_params,
            &run.session.execution,
            run.session.model.as_deref(),
        )?;
        let turn = wire.request("turn/start", turn_params, cancel).await?;
        string(&turn["turn"], "id")?
    };
    let mut ledger = PendingLedger {
        session_id: run.session.id.clone(),
        generation: run.generation.clone(),
        turn_id: turn_id.clone(),
        requests: HashMap::new(),
    };
    let mut items: HashMap<String, String> = HashMap::new();
    let mut item_key_bytes = 0;
    // Separate consecutive assistant message items in the one transcript message.
    let mut last_text_item: Option<String> = None;
    // Usage-limit refusal and the latest exhausted window's reset (`account/rateLimits/updated`).
    let mut usage_limited = false;
    let mut context_full = false;
    let mut exhausted_reset: Option<i64> = None;
    let mut file_changes: HashMap<String, Value> = HashMap::new();
    let mut file_change_bytes = 0;
    let mut seen_requests = std::collections::HashSet::new();
    loop {
        let value = if let Some(value) = wire.deferred.pop_front() {
            wire.deferred_bytes = wire.deferred_bytes.saturating_sub(value.to_string().len());
            value
        } else {
            tokio::select! {
                value = wire.read() => value?,
                reply = run.replies.recv() => {
                    let Some(reply) = reply else { return Err(Error::agent("Codex response channel closed")); };
                    let reply = match reply {
                        Inbound::Answer(reply) => reply,
                        Inbound::Steer { text, attachments, result } => {
                            // `turn/steer` adds the person's input to this same turn; its events wait in the queue.
                            let params = json!({"threadId":thread_id,"input":crate::attachments::codex_input(&text, &attachments),"expectedTurnId":turn_id});
                            let delivered = wire.request("turn/steer", params, &mut cancel.clone()).await.map(|_| ()).map_err(|_| Error::agent("Codex did not accept the instruction for this turn"));
                            let _ = result.send(delivered);
                            continue;
                        }
                        Inbound::StopTask { result, .. } => {
                            let _ = result.send(Err(Error::agent("Codex has no background tasks to stop")));
                            continue;
                        }
                        Inbound::SteerSubagent { result, .. } => {
                            let _ = result.send(Err(Error::agent("Codex subagents cannot receive messages")));
                            continue;
                        }
                        Inbound::BackgroundTask { result, .. } => {
                            let _ = result.send(Err(Error::agent("Codex cannot move a subagent to the background")));
                            continue;
                        }
                    };
                    match ledger.take(&reply.request) {
                        Ok((id,result)) => {
                            let sent = wire.send(json!({"id":id,"result":result})).await;
                            if sent.is_ok() { emit(Event::Answered(reply.request.request_id))?; }
                            let failed = sent.is_err(); let _ = reply.result.send(sent);
                            if failed { return Err(Error::agent("Codex approval response could not be delivered")); }
                        }
                        Err(error) => { let _ = reply.result.send(Err(error)); }
                    }
                    continue;
                }
                _ = cancel.changed() => {
                    // Native interruption first, bounded fallback cleanup is performed by monitor.
                    wire.sequence += 1;
                    wire.send(json!({"id":format!("sirus-{}",wire.sequence),"method":"turn/interrupt","params":{"threadId":thread_id,"turnId":turn_id}})).await?;
                    let _ = tokio::time::timeout(Duration::from_secs(2), async {
                        while let Ok(value) = wire.read().await {
                            if value["method"] == "turn/completed" && value["params"]["turn"]["id"] == turn_id { break; }
                        }
                    }).await;
                    return Ok(true);
                }
            }
        };
        let Some(method) = value["method"].as_str() else {
            continue;
        };
        let params = &value["params"];
        if value.get("id").is_some() {
            if params["threadId"] != thread_id || params["turnId"] != turn_id {
                return Err(Error::agent(
                    "Codex callback belongs to another thread or turn",
                ));
            }
            let original = value["id"].clone();
            if !(original
                .as_str()
                .is_some_and(|id| !id.is_empty() && id.len() <= 1024)
                || original.is_i64())
                || seen_requests.len() >= 1024
                || !seen_requests.insert(original.to_string())
            {
                return Err(Error::agent("Invalid or duplicate Codex callback"));
            }
            if ledger.requests.len() >= 16 {
                return Err(Error::agent("Too many pending Codex callbacks"));
            }
            let mut details = params.clone();
            if method == "item/fileChange/requestApproval" {
                if let Some(changes) = params["itemId"]
                    .as_str()
                    .and_then(|item| file_changes.get(item))
                {
                    details["changes"] = changes.clone();
                }
            }
            if let Some(kind) = pending_kind(method, &details)? {
                let pending = PendingRequest {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    generation: run.generation.clone(),
                    turn_id: turn_id.clone(),
                    item_id: string(params, "itemId")?,
                    kind,
                };
                ledger
                    .requests
                    .insert(pending.request_id.clone(), (original, pending.clone()));
                emit(Event::Pending(pending))?;
            } else {
                let result = match method {
                    "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                        Some(json!({"decision":"decline"}))
                    }
                    "item/permissions/requestApproval" => {
                        Some(json!({"permissions":{},"scope":"turn"}))
                    }
                    "item/tool/requestUserInput" => Some(json!({"answers":{}})),
                    _ => None,
                };
                wire.send(match result {
                    Some(result) => json!({"id":original,"result":result}),
                    None => json!({"id":original,"error":{"code":-32601,"message":"Unsupported native request"}}),
                }).await?;
            }
            continue;
        }
        if params["threadId"] != thread_id {
            // This turn's server only hosts its own thread and the children it spawns. Child work is
            // display-only and attaches to an observed child row; unknown threads are discarded.
            if let Some(thread) = params["threadId"].as_str() {
                if matches!(method, "item/started" | "item/completed") {
                    if let Some(update) = crate::subagent_feed::from_codex(
                        &params["item"],
                        method == "item/started",
                        thread,
                    ) {
                        emit(Event::Feed(update))?;
                    }
                    let items = crate::activity::codex_child(
                        &params["item"],
                        method == "item/started",
                        thread,
                    );
                    if !items.is_empty() {
                        emit(Event::Activity(items))?;
                    }
                }
            }
            continue;
        }
        if params["turnId"] == turn_id && matches!(method, "item/started" | "item/completed") {
            let items = crate::activity::codex(&params["item"], method == "item/started");
            if !items.is_empty() {
                emit(Event::Activity(items))?;
            }
        }
        if params["turnId"] == turn_id && method == "turn/plan/updated" {
            if let Some(plan) = crate::activity::codex_plan(params) {
                emit(Event::Activity(vec![plan]))?;
            }
        }
        match method {
            "serverRequest/resolved" => {
                if let Some(opaque) = ledger.resolve(&params["requestId"]) {
                    emit(Event::Answered(opaque))?;
                }
            }
            "item/started"
                if params["turnId"] == turn_id && params["item"]["type"] == "fileChange" =>
            {
                if file_changes.len() >= 256 {
                    return Err(Error::agent("Too many native file change proposals"));
                }
                let item = string(&params["item"], "id")?;
                let changes = params["item"]["changes"].clone();
                if let Some(previous) = file_changes.remove(&item) {
                    file_change_bytes -= previous.to_string().len();
                }
                file_change_bytes += changes.to_string().len();
                if file_change_bytes > 8 * 1024 * 1024 {
                    return Err(Error::agent(
                        "Codex file proposals exceeded the 8 MiB limit",
                    ));
                }
                file_changes.insert(item, changes);
            }
            "item/completed"
                if params["turnId"] == turn_id && params["item"]["type"] == "fileChange" =>
            {
                if let Some(previous) = params["item"]["id"]
                    .as_str()
                    .and_then(|item| file_changes.remove(item))
                {
                    file_change_bytes -= previous.to_string().len();
                }
            }
            "item/agentMessage/delta" if params["turnId"] == turn_id => {
                let item = string(params, "itemId")?;
                let delta = params["delta"]
                    .as_str()
                    .ok_or_else(|| Error::agent("Invalid Codex text delta"))?;
                if delta.is_empty() {
                    continue;
                }
                let separator = item_separator(&mut last_text_item, &item);
                let text = bounded_item(&mut items, &mut item_key_bytes, item)?;
                if text.len() + delta.len() > 8 * 1024 * 1024 {
                    return Err(Error::agent("Codex output exceeded the 8 MiB limit"));
                }
                text.push_str(delta);
                emit(Event::Delta(format!("{separator}{delta}")))?;
            }
            "item/completed"
                if params["turnId"] == turn_id && params["item"]["type"] == "agentMessage" =>
            {
                let item = string(&params["item"], "id")?;
                let text = params["item"]["text"]
                    .as_str()
                    .ok_or_else(|| Error::agent("Invalid Codex assistant text"))?;
                let separator = if text.len() > items.get(&item).map_or(0, String::len) {
                    item_separator(&mut last_text_item, &item)
                } else {
                    ""
                };
                let previous = bounded_item(&mut items, &mut item_key_bytes, item)?;
                if let Some(rest) = text.strip_prefix(previous.as_str()) {
                    if !rest.is_empty() {
                        emit(Event::Delta(format!("{separator}{rest}")))?;
                    }
                } else {
                    return Err(Error::agent(
                        "Codex final text did not match streamed output",
                    ));
                }
                *previous = text.into();
            }
            "account/rateLimits/updated" => {
                if let Some(reset) = exhausted_window_reset(&params["rateLimits"]) {
                    exhausted_reset = Some(reset);
                }
            }
            "error" if params["turnId"] == turn_id => {
                usage_limited |= is_usage_limit(&params["error"]);
                context_full |= is_context_window_full(&params["error"]);
            }
            "thread/tokenUsage/updated" => {
                if let Some(usage) = context_usage(params) {
                    emit(Event::Context(usage))?;
                }
            }
            "turn/completed" if params["turn"]["id"] == turn_id => {
                return match params["turn"]["status"].as_str() {
                    Some("completed") => Ok(false),
                    Some("interrupted") => Ok(true),
                    // A full context window is never a usage limit: no reset will fix it.
                    _ if context_full || is_context_window_full(&params["turn"]["error"]) => {
                        Err(Error::new("context_window", CONTEXT_WINDOW_FULL))
                    }
                    _ if usage_limited || is_usage_limit(&params["turn"]["error"]) => {
                        emit(Event::UsageLimit(exhausted_reset))?;
                        Err(Error::new("usage_limit", "Usage limit reached."))
                    }
                    _ => Err(Error::agent("Codex turn failed; see CLI diagnostics")),
                };
            }
            _ => {}
        }
    }
}

/// Codex marks a spent account limit with `codexErrorInfo: "usageLimitExceeded"`.
fn is_usage_limit(error: &Value) -> bool {
    match &error["codexErrorInfo"] {
        Value::String(kind) => kind == "usageLimitExceeded",
        Value::Object(kinds) => kinds.contains_key("usageLimitExceeded"),
        _ => false,
    }
}

/// The turn error shown when the conversation no longer fits the model's context window.
pub(crate) const CONTEXT_WINDOW_FULL: &str = "The conversation no longer fits the model's context window. Run /compact or start a new conversation.";

/// Codex marks a prompt over the model's window with `codexErrorInfo: "contextWindowExceeded"`.
fn is_context_window_full(error: &Value) -> bool {
    match &error["codexErrorInfo"] {
        Value::String(kind) => kind == "contextWindowExceeded",
        Value::Object(kinds) => kinds.contains_key("contextWindowExceeded"),
        _ => false,
    }
}

/// The latest reset (UTC ms) among rate-limit windows at or over 100%.
pub(crate) fn exhausted_window_reset(snapshot: &Value) -> Option<i64> {
    ["primary", "secondary"]
        .iter()
        .filter_map(|key| {
            let window = &snapshot[*key];
            (window["usedPercent"].as_f64()? >= 100.0)
                .then(|| window["resetsAt"].as_i64())
                .flatten()
        })
        .max()
        .map(|seconds| seconds.saturating_mul(1000))
}

/// The standalone `/compact` command.
pub(crate) fn is_compact(prompt: &str) -> bool {
    prompt.trim() == "/compact"
}

/// `thread/tokenUsage/updated`: the last request's total is what the context holds now;
/// the running total keeps growing across compactions and is ignored.
fn context_usage(params: &Value) -> Option<crate::models::ContextUsage> {
    let usage = &params["tokenUsage"];
    Some(crate::models::ContextUsage {
        used: usage["last"]["totalTokens"].as_u64()?,
        window: usage["modelContextWindow"]
            .as_u64()
            .filter(|window| *window > 0),
        cache_expires_at: None,
        floor: None,
    })
}

/// A compaction runs as its own turn; its id arrives with `turn/started`.
/// Other early events stay queued for the turn loop, within the same bounds.
async fn compaction_turn(
    wire: &mut Wire,
    thread_id: &str,
    cancel: &mut watch::Receiver<bool>,
) -> Result<String> {
    let started = |value: &Value| {
        (value["method"] == "turn/started" && value["params"]["threadId"] == thread_id)
            .then(|| value["params"]["turn"]["id"].as_str().map(str::to_owned))
            .flatten()
    };
    if let Some(id) = wire.deferred.iter().find_map(started) {
        return Ok(id);
    }
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let value = tokio::select! {
                value = wire.read() => value?,
                _ = cancel.changed() => return Err(Error::agent("Codex compaction cancelled")),
            };
            let id = started(&value);
            let size = value.to_string().len();
            if wire.deferred.len() >= 256 || wire.deferred_bytes + size > 8 * 1024 * 1024 {
                return Err(Error::agent("Too many early Codex turn events"));
            }
            wire.deferred_bytes += size;
            wire.deferred.push_back(value);
            if let Some(id) = id {
                return Ok(id);
            }
        }
    })
    .await
    .map_err(|_| Error::agent("Codex did not start the compaction"))?
}

/// A blank line before the first text of a new assistant message item, so
/// consecutive Codex messages in one turn do not run together.
fn item_separator(last: &mut Option<String>, item: &str) -> &'static str {
    let separator = match last.as_deref() {
        Some(previous) if previous != item => "\n\n",
        _ => "",
    };
    if last.as_deref() != Some(item) {
        *last = Some(item.to_owned());
    }
    separator
}

fn bounded_item<'a>(
    items: &'a mut HashMap<String, String>,
    bytes: &mut usize,
    item: String,
) -> Result<&'a mut String> {
    if !items.contains_key(&item) {
        if items.len() >= 4096 || *bytes + item.len() > 1024 * 1024 {
            return Err(Error::agent("Too many Codex output item identities"));
        }
        *bytes += item.len();
    }
    Ok(items.entry(item).or_default())
}

/// Fresh Claude processes an auth-refresh retry may try (ADR-162).
const AUTH_REFRESH_RETRIES: u8 = 2;
/// Wait before each retry: the process that won the refresh writes the rotated credential
/// (keychain-backed) within a moment.
const AUTH_REFRESH_BACKOFF: Duration = Duration::from_millis(1500);

/// A fresh Claude process for a turn whose shared OAuth credential was mid-refresh (ADR-162).
/// The account scope and provider overrides are resolved exactly as at admission, and the
/// session snapshot's own `native_thread` decides `--resume`, so a continuation keeps its
/// thread while a first turn starts clean.
async fn restart_claude(
    state: &std::sync::Arc<crate::commands::AppState>,
    run: &Run,
) -> Result<(crate::agent::AgentProcess, crate::agent::StartedAgent, Wire)> {
    let overrides = state.data.lock().settings.provider_paths.clone();
    let home = crate::provider_accounts::scope(
        state,
        &run.session.agent,
        &run.session.provider_account_id,
    )?
    .home;
    let (process, mut started) = crate::claude::start(
        run.session.clone(),
        run.cwd.clone(),
        run.prompt.clone(),
        overrides,
        home,
    )
    .await?;
    let input = started
        .child
        .stdin
        .take()
        .ok_or_else(|| Error::agent("Claude retry has no input pipe"))?;
    let output = started
        .child
        .stdout
        .take()
        .ok_or_else(|| Error::agent("Claude retry has no output pipe"))?;
    Ok((process, started, Wire::new(input, output)))
}

/// The size of the native transcript a continuation turn appends to, recorded before the turn
/// so a failed auth-refresh attempt can be rolled back before the retry (ADR-162). A first
/// turn has no thread yet: its failed attempt only leaves an orphan transcript the retry never
/// resumes.
fn claude_transcript_len(
    config_dir: Option<&std::path::Path>,
    run: &Run,
) -> Option<(std::path::PathBuf, u64)> {
    let dir = config_dir?;
    let thread = run.session.native_thread.as_ref()?.thread_id.as_str();
    let path = crate::account_move::claude_transcript(dir, &run.cwd, thread)?;
    let len = std::fs::metadata(&path).ok()?.len();
    Some((path, len))
}

/// Removes a failed Claude attempt's appended lines from the native transcript, restoring the
/// pre-turn thread so the retry's `--resume` never replays the prompt twice. Called only after
/// the failed process is stopped, so nothing else writes the file.
fn rollback_claude_transcript(transcript: &Option<(std::path::PathBuf, u64)>) {
    let Some((path, len)) = transcript else {
        return;
    };
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_len(*len);
    }
}

pub fn monitor(
    mut started: crate::agent::StartedAgent,
    mut run: Run,
    app: crate::runtime::AppHandle,
    state: std::sync::Arc<crate::commands::AppState>,
    session_id: String,
    awake: crate::keep_awake::Hold,
) {
    use crate::runtime::Emitter;
    let reused = started.wire.take();
    let mut pool_key = started.pool_key.take();
    let fresh = match reused {
        Some(wire) => Ok(wire),
        None => {
            let Some(input) = started.child.stdin.take() else {
                return;
            };
            let Some(output) = started.child.stdout.take() else {
                return;
            };
            Err((input, output))
        }
    };
    let (provider_error_sender, mut provider_errors) = tokio::sync::mpsc::channel(4);
    // OpenCode and Devin write routine logs to stderr: drained, never shown as messages.
    let quiet_logs = matches!(
        run.session.agent,
        AgentProviderId::OpenCode | AgentProviderId::Devin | AgentProviderId::Hermes
    );
    // The log reader of a kept process outlives its first turn: it reports to whichever turn
    // runs now.
    crate::opencode::route_errors(&session_id, provider_error_sender);
    let diagnostics = started.child.stderr.take().map(|reader| {
        if quiet_logs {
            crate::opencode::watch_logs(reader, session_id.clone())
        } else {
            crate::agent::spawn_reader(
                app.clone(),
                state.clone(),
                session_id.clone(),
                "stderr",
                reader,
            )
        }
    });
    tokio::spawn(async move {
        // Held until the turn's monitor ends: the Mac stays awake (ADR-100).
        let _awake = awake;
        let mut wire = match fresh {
            Ok(wire) => wire,
            Err((input, output)) => Wire::new(input, output),
        };
        // A process that may be kept between turns has a stderr reader that lives as long as
        // it does; the turn does not wait for it.
        let diagnostics = if pool_key.is_some() {
            None
        } else {
            diagnostics
        };
        let mut cursor = crate::agent::OutputCursor::default();
        let mut emit = |event| -> Result<()> {
            // A subagent's own work goes to its live view only, outside the session (ADR-146).
            let event = match event {
                Event::Feed(update) => {
                    crate::subagent_feed::publish(&app, &session_id, update);
                    return Ok(());
                }
                event => event,
            };
            let mut data = state.data.lock();
            let session = data
                .sessions
                .iter_mut()
                .find(|session| session.id == session_id)
                .ok_or_else(|| Error::agent("Session removed during native turn"))?;
            if session.status == SessionStatus::Stopped {
                return Ok(());
            }
            if session.status == SessionStatus::Failed {
                return Err(Error::agent("Native turn cancelled"));
            }
            let mut publish = true;
            let mut persist_immediately = true;
            // Tool rows, subagent steps and context readings change many times a
            // second in a busy turn; they are published together (ADR-048 events).
            let mut coalesce = false;
            match event {
                Event::Identity(identity) => session.native_thread = Some(identity),
                Event::Activity(items) => {
                    publish = false;
                    persist_immediately = false;
                    coalesce = true;
                    for item in items {
                        publish |= crate::activity::observe(session, item);
                    }
                }
                Event::Model(model) => crate::activity::model(session, &model),
                // Handled before the lock.
                Event::Feed(_) => publish = false,
                Event::UsageLimit(resets_at) => {
                    session.usage_limit = Some(crate::models::UsageLimit { resets_at })
                }
                Event::Context(usage) => {
                    // A reading without a window keeps the last window known for this model.
                    let window = usage
                        .window
                        .or(session.context_usage.and_then(|known| known.window));
                    // Likewise the last known cache expiry (a compaction or a result reports none).
                    let cache_expires_at = usage.cache_expires_at.or(session
                        .context_usage
                        .and_then(|known| known.cache_expires_at));
                    // The smallest reading is the provider's own prompt and tools.
                    let floor = session
                        .context_usage
                        .and_then(|known| known.floor)
                        .map_or(usage.used, |known| known.min(usage.used));
                    let next = crate::models::ContextUsage {
                        used: usage.used,
                        window,
                        cache_expires_at,
                        floor: Some(floor).filter(|floor| *floor > 0),
                    };
                    publish = session.context_usage != Some(next);
                    persist_immediately = false;
                    coalesce = true;
                    session.context_usage = Some(next);
                }
                Event::Compacted(rebuilt) => {
                    // The boundary leaves out the provider's prompt and tools: add back what the
                    // conversation was seen to cost at its lightest, until the next reply
                    // reports the real figure.
                    let known = session.context_usage;
                    let floor = known.and_then(|known| known.floor);
                    let next = crate::models::ContextUsage {
                        used: rebuilt + floor.unwrap_or(0),
                        window: known.and_then(|known| known.window),
                        cache_expires_at: known.and_then(|known| known.cache_expires_at),
                        floor,
                    };
                    publish = session.context_usage != Some(next);
                    persist_immediately = false;
                    coalesce = true;
                    session.context_usage = Some(next);
                }
                Event::Delta(chunk) => {
                    if let Some(event) =
                        crate::agent::record_output(session, &mut cursor, "stdout", chunk)?
                    {
                        crate::output_batch::emit(&app, event);
                    }
                    publish = false;
                    persist_immediately = false;
                }
                Event::Pending(pending) => {
                    session.pending_requests.push(pending);
                    session.status = SessionStatus::Waiting;
                    crate::notifications::publish(&app, &state, session);
                }
                Event::Answered(id) => {
                    session
                        .pending_requests
                        .retain(|request| request.request_id != id);
                    if session.pending_requests.is_empty() {
                        session.status = SessionStatus::Running;
                    }
                }
            }
            crate::activity::sync(session);
            // Identity, requests and answers are saved now; streamed text and activity
            // use the coalesced checkpoint written outside the lock.
            if persist_immediately {
                crate::persist::save(&state.data_path, &data)?;
            } else {
                crate::persist::checkpoint_soon(&state, &session_id);
            }
            if publish && coalesce {
                crate::transcript_view::emit_soon(&app, &session_id);
            } else if publish {
                if let Some(session) = data
                    .sessions
                    .iter()
                    .find(|session| session.id == session_id)
                {
                    crate::transcript_view::emit(&app, session);
                }
            }
            Ok(())
        };
        let outcome = if run.session.agent == AgentProviderId::Claude {
            let config_dir = crate::provider_accounts::claude_config_dir(
                &state,
                &run.session.provider_account_id,
                &run.cwd,
            );
            let transcript = claude_transcript_len(config_dir.as_deref(), &run);
            let mut result = crate::claude::execute_in(
                &mut wire,
                &mut run,
                config_dir.clone(),
                &mut started.cancel,
                &mut emit,
            )
            .await;
            // Several `claude` processes share one login (another Sirus turn, the usage probe,
            // a terminal). The OAuth token is single-use, so the loser of a refresh race gives
            // up with "could not be refreshed" even though the login is fine. A fresh process
            // reads the rotated credential, so the turn is retried before it is reported failed.
            let mut retries = 0;
            while retries < AUTH_REFRESH_RETRIES
                && !*started.cancel.borrow()
                && result
                    .as_ref()
                    .err()
                    .is_some_and(crate::claude::auth_refresh_error)
            {
                retries += 1;
                // Its input is still open, so it will not exit by itself: a short wait only.
                stop_owned(&mut started.child, Duration::from_secs(2)).await;
                // A continuation's failed attempt wrote the prompt into the native thread;
                // drop it so the retry's resume does not replay it twice.
                rollback_claude_transcript(&transcript);
                tokio::time::sleep(AUTH_REFRESH_BACKOFF).await;
                if *started.cancel.borrow() {
                    break;
                }
                let attachments = std::mem::take(&mut run.attachments);
                match restart_claude(&state, &run).await {
                    Ok((process, mut fresh, fresh_wire)) => {
                        if let Some(reader) = fresh.child.stderr.take() {
                            crate::agent::spawn_reader(
                                app.clone(),
                                state.clone(),
                                session_id.clone(),
                                "stderr",
                                reader,
                            );
                        }
                        wire = fresh_wire;
                        started.child = fresh.child;
                        started.cancel = fresh.cancel;
                        let Some(mut fresh_run) = fresh.codex.take() else {
                            run.attachments = attachments;
                            result = Err(Error::agent("Claude retry produced no turn"));
                            break;
                        };
                        fresh_run.attachments = attachments;
                        run = fresh_run;
                        pool_key = fresh.pool_key.take();
                        state.agents.lock().insert(session_id.clone(), process);
                        result = crate::claude::execute_in(
                            &mut wire,
                            &mut run,
                            config_dir.clone(),
                            &mut started.cancel,
                            &mut emit,
                        )
                        .await;
                    }
                    Err(error) => {
                        run.attachments = attachments;
                        result = Err(error);
                    }
                }
            }
            // Still refused after the retries: the shared credential is not merely mid-refresh
            // but broken (another process overwrote it). Say what to do instead of the raw text.
            match result {
                Err(error) if crate::claude::auth_refresh_error(&error) => Err(Error::new(
                    "claude_auth_refresh",
                    format!(
                        "{error}. Sign in to Claude Code again with `claude auth login --claudeai`: concurrent sessions can overwrite the shared credential."
                    ),
                )),
                other => other,
            }
        } else if run.session.agent == AgentProviderId::OpenCode {
            crate::opencode::execute(
                &mut wire,
                &mut run,
                &mut started.cancel,
                &mut provider_errors,
                &mut emit,
            )
            .await
        } else if crate::acp_cli::handles(&run.session.agent) {
            crate::acp_cli::execute(&mut wire, &mut run, &mut started.cancel, &mut emit).await
        } else {
            let outcome = execute(&mut wire, &mut run, &mut started.cancel, &mut emit).await;
            // A completed Codex turn leaves its app-server idle and ready for the next.
            wire.reusable = matches!(outcome, Ok(false));
            outcome
        };
        // Completion invalidates callbacks before the server's bounded cleanup window.
        // Closing the reply receiver also rejects IPC queued after the turn has settled.
        drop(run);
        {
            let mut data = state.data.lock();
            if let Some(session) = data
                .sessions
                .iter_mut()
                .find(|session| session.id == session_id)
            {
                if !session.pending_requests.is_empty() {
                    session.pending_requests.clear();
                    crate::transcript_view::emit(&app, session);
                }
            }
        }
        // A completed Codex turn keeps its app-server for the session's next turn; any other
        // ending closes it (EOF requests app-server shutdown after turn completion).
        let keep = pool_key.is_some()
            && wire.reusable
            && matches!(outcome, Ok(false))
            && !state.closing.load(std::sync::atomic::Ordering::Acquire)
            && matches!(started.child.try_wait(), Ok(None));
        let kept_wire = if keep {
            Some(wire)
        } else {
            drop(wire);
            // Long enough for a CLI to persist a refreshed login before its group is killed.
            stop_owned(&mut started.child, crate::agent::GRACEFUL_EXIT).await;
            None
        };
        let protocol_failed =
            crate::agent::drain_output_readers(diagnostics).await || outcome.is_err();
        let review =
            crate::turn_review::finish(started.review.take(), state.clone(), session_id.clone())
                .await;
        let mut data = state.data.lock();
        // Admission and unregistering stay ordered; no stale signal handle survives reap.
        state.agents.lock().remove(&session_id);
        let _ = started.child.try_wait();
        if let Some(session) = data
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            let cancelled = *started.cancel.borrow()
                || outcome.as_ref().is_ok_and(|interrupted| *interrupted)
                || session.status == SessionStatus::Stopped;
            if let Err(error) = &outcome {
                if !cancelled {
                    session.last_error = Some(error.to_string());
                }
            }
            crate::agent::finalize_session(
                session,
                Some(if outcome.is_ok() { 0 } else { 1 }),
                cancelled,
                protocol_failed,
                state.closing.load(std::sync::atomic::Ordering::Acquire),
                None,
            );
            session.pending_requests.clear();
            crate::turn_review::attach(session, review);
            crate::transcript_view::emit(&app, session);
            crate::notifications::publish(&app, &state, session);
        }
        if let Err(error) = crate::persist::save(&state.data_path, &data) {
            tracing::error!(%error,"cannot persist native completion");
        }
        crate::team::settled(&app, &state, &session_id);
        crate::delegation::settled(&app, &state, &session_id);
        crate::ci_autofix::settled(&state, &session_id);
        crate::project_scripts::settled(&app, &state, &session_id);
        let _ = app.emit(
            "agent-exit",
            AgentExitEvent {
                session_id: session_id.clone(),
                code: outcome.is_ok().then_some(0),
            },
        );
        if let (Some(wire), Some(key)) = (kept_wire, pool_key) {
            // Kept under the conversation it now holds, which the next turn asks for.
            // `data` is still held here: never lock it again.
            let thread = data
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .map(|session| crate::agent_pool::with_thread(&key, session));
            if let Some(key) = thread {
                crate::agent_pool::keep(session_id, key, started.child, wire);
            }
        }
    });
}

async fn stop_owned(child: &mut Child, grace: Duration) {
    #[cfg(unix)]
    {
        let observed = tokio::time::timeout(grace, crate::agent::observe_owned_exit(child)).await;
        // Never signal a reaped/reused identity if observation failed.
        if !matches!(observed, Ok(Err(_))) {
            crate::agent::kill_owned_group(child);
            let _ = crate::agent::observe_owned_exit(child).await;
        }
    }
    #[cfg(not(unix))]
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

#[cfg(test)]
async fn cleanup(child: &mut Child) {
    stop_owned(child, Duration::from_secs(2)).await;
    let _ = child.wait().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_continuation_rolls_its_native_transcript_back() {
        let root = std::env::temp_dir().join(format!("sirus-auth-{}", std::process::id()));
        let project = root
            .join("projects")
            .join(crate::account_move::claude_project_name("/w"));
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join("native.jsonl");
        std::fs::write(&file, "{\"turn\":1}\n").unwrap();
        let located = crate::account_move::claude_transcript(&root, "/w", "native").unwrap();
        assert_eq!(located, file);
        let transcript = Some((located, std::fs::metadata(&file).unwrap().len()));
        // The failed attempt's prompt and error lines are dropped before the retry.
        std::fs::write(&file, "{\"turn\":1}\n{\"turn\":2}\n{\"turn\":2}\n").unwrap();
        rollback_claude_transcript(&transcript);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"turn\":1}\n");
        // A first turn (no thread) and an unknown thread roll nothing back.
        rollback_claude_transcript(&None);
        assert!(crate::account_move::claude_transcript(&root, "/w", "foreign").is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn browser_mcp_token_stays_out_of_argv() {
        let (args, env) = browser_overrides(
            "/Applications/Sirus Code.app/Contents/MacOS/sirus-code",
            "/tmp/sirus-browser.sock",
            "browser-secret-token",
            &[("SIRUS_TOOLS", "1"), ("SIRUS_BROWSER_TOOLS", "0")],
        );
        assert!(args.iter().all(|arg| !arg.contains("browser-secret-token")));
        assert!(
            args.contains(&"mcp_servers.sirus_browser.env.SIRUS_BROWSER_TOOLS=\"0\"".to_string())
        );
        assert!(args
            .contains(&"mcp_servers.sirus_browser.env_vars=[\"SIRUS_BROWSER_TOKEN\"]".to_string()));
        assert!(args.iter().any(|arg| arg
            == "mcp_servers.sirus_browser.env.SIRUS_BROWSER_SOCKET=\"/tmp/sirus-browser.sock\""));
        assert_eq!(
            env,
            vec![("SIRUS_BROWSER_TOKEN", "browser-secret-token".to_string())]
        );
    }

    #[test]
    fn usage_limits_and_their_reset_come_from_codex_reports() {
        assert!(is_usage_limit(
            &json!({"message":"limit","codexErrorInfo":"usageLimitExceeded"})
        ));
        assert!(is_usage_limit(
            &json!({"codexErrorInfo":{"usageLimitExceeded":{}}})
        ));
        assert!(
            !is_usage_limit(&json!({"codexErrorInfo":"other"})) && !is_usage_limit(&Value::Null)
        );
        // A full context window is its own failure, never a usage limit.
        let overflow =
            json!({"message":"context window exceeded","codexErrorInfo":"contextWindowExceeded"});
        assert!(is_context_window_full(&overflow) && !is_usage_limit(&overflow));
        assert!(is_context_window_full(
            &json!({"codexErrorInfo":{"contextWindowExceeded":null}})
        ));
        assert!(!is_context_window_full(
            &json!({"codexErrorInfo":"usageLimitExceeded"})
        ));
        let snapshot = json!({"primary":{"usedPercent":100,"resetsAt":1790000000},"secondary":{"usedPercent":40,"resetsAt":1790500000}});
        assert_eq!(exhausted_window_reset(&snapshot), Some(1_790_000_000_000));
        assert_eq!(
            exhausted_window_reset(&json!({"primary":{"usedPercent":99,"resetsAt":1}})),
            None
        );
    }

    #[test]
    fn context_usage_reads_the_last_request_and_window() {
        let params = json!({"threadId":"t","turnId":"u","tokenUsage":{"last":{"totalTokens":176000,"inputTokens":1,"cachedInputTokens":0,"outputTokens":0,"reasoningOutputTokens":0},"total":{"totalTokens":900000,"inputTokens":1,"cachedInputTokens":0,"outputTokens":0,"reasoningOutputTokens":0},"modelContextWindow":256000}});
        assert_eq!(
            context_usage(&params),
            Some(crate::models::ContextUsage {
                used: 176000,
                window: Some(256000),
                cache_expires_at: None,
                floor: None
            })
        );
        assert_eq!(context_usage(&json!({"tokenUsage":{"last":{}}})), None);
        assert!(is_compact(" /compact ") && !is_compact("/compact now"));
    }
    #[test]
    fn consecutive_assistant_items_are_separated_once() {
        let mut last = None;
        assert_eq!(item_separator(&mut last, "a"), "");
        assert_eq!(item_separator(&mut last, "a"), "");
        assert_eq!(item_separator(&mut last, "b"), "\n\n");
        assert_eq!(item_separator(&mut last, "b"), "");
    }
    #[test]
    fn empty_output_items_have_count_and_retained_key_bounds() {
        let mut items = HashMap::new();
        let mut bytes = 0;
        for index in 0..4096 {
            bounded_item(&mut items, &mut bytes, index.to_string()).unwrap();
        }
        assert!(bounded_item(&mut items, &mut bytes, "overflow".into()).is_err());
        let mut items = HashMap::new();
        let mut bytes = 0;
        for index in 0..64 {
            bounded_item(
                &mut items,
                &mut bytes,
                format!("{index:04}{}", "x".repeat(16 * 1024 - 4)),
            )
            .unwrap();
        }
        assert!(bounded_item(&mut items, &mut bytes, "overflow".into()).is_err());
        let existing = items.keys().next().unwrap().clone();
        assert!(bounded_item(&mut items, &mut bytes, existing).is_ok());
    }
    #[tokio::test]
    async fn protocol_read_preserves_split_frame_across_cancelled_select() {
        let mut child = Command::new("sh")
            .args([
                "-c",
                "printf '{\"type\":'; read -r release; printf '\"complete\"}\\n'",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut wire = Wire::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
        tokio::io::AsyncBufReadExt::fill_buf(&mut wire.output)
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), wire.read())
                .await
                .is_err()
        );
        assert!(!wire.line.is_empty());
        wire.input.write_all(b"release\n").await.unwrap();
        assert_eq!(wire.read().await.unwrap(), json!({"type":"complete"}));
        child.wait().await.unwrap();
    }
    fn session() -> Session {
        serde_json::from_value(json!({"id":"s","projectId":"p","title":"Task","agent":"codex","status":"running","createdAt":"time","lastActivityAt":"time","worktree":{"path":"/fixture","branch":"main","isolated":false},"messages":[],"nativeThread":{"threadId":"native","sessionId":"s","projectId":"p","cwd":"/fixture","model":null}})).unwrap()
    }
    #[tokio::test]
    async fn exact_resume_confirms_approval_profiles_before_sending_input() {
        use crate::models::ApprovalMode;
        let script = r#"
import sys,json
def read(): return json.loads(sys.stdin.readline())
def send(v): print(json.dumps(v),flush=True)
approval,reviewer,sandbox,kind,accepted=sys.argv[1:]
r=read();assert r['method']=='initialize';send({'id':r['id'],'result':{}})
assert read()['method']=='initialized'
r=read();assert r['method']=='thread/resume'
p=r['params'];assert p['threadId']=='native' and p['excludeTurns'] is True
assert (p['approvalPolicy'],p['approvalsReviewer'],p['sandbox'])==(approval,reviewer,sandbox)
send({'id':r['id'],'result':{'thread':{'id':'native'},'cwd':'/fixture','approvalPolicy':approval,'approvalsReviewer':reviewer,'sandbox':{'type':kind if accepted=='true' else 'wrong'}}})
if accepted=='true':
 r=read();assert r['method']=='turn/start';p=r['params']
 assert (p['approvalPolicy'],p['approvalsReviewer'],p['sandboxPolicy']['type'])==(approval,reviewer,kind)
 if kind=='workspaceWrite':
  assert p['sandboxPolicy']['writableRoots']==['/fixture'] and p['sandboxPolicy']['networkAccess'] is False
 assert p['input'][0]['text']=='fixture input'
 send({'id':r['id'],'result':{'turn':{'id':'turn'}}})
 for thread,turn in [('foreign','turn'),('native','stale')]:
  send({'method':'item/started','params':{'threadId':thread,'turnId':turn,'item':{'id':'foreign','type':'commandExecution','status':'inProgress'}}})
 for method,status in [('item/started','inProgress'),('item/completed','completed')]:
  send({'method':method,'params':{'threadId':'native','turnId':'turn','item':{'id':'owned','type':'commandExecution','status':status,'exitCode':0,'command':'private argument'}}})
 send({'method':'turn/completed','params':{'threadId':'native','turn':{'id':'turn','status':'completed'}}})
else:
 assert sys.stdin.readline()==''
"#;
        for mode in [ApprovalMode::Full, ApprovalMode::Ask, ApprovalMode::Auto] {
            for accepted in [true, false] {
                let mut session = session();
                session.execution.approval = Some(mode);
                let (sandbox, approval, reviewer) =
                    crate::execution::codex_policy(&session.execution);
                let kind = if mode == ApprovalMode::Full {
                    "dangerFullAccess"
                } else {
                    "workspaceWrite"
                };
                let mut child = Command::new("python3")
                    .args([
                        "-c",
                        script,
                        approval,
                        reviewer,
                        sandbox,
                        kind,
                        if accepted { "true" } else { "false" },
                    ])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    .unwrap();
                let mut wire = Wire::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
                let (_sender, replies) = mpsc::channel(16);
                let (_cancel, mut cancel) = watch::channel(false);
                let mut run = Run {
                    session,
                    cwd: "/fixture".into(),
                    prompt: "fixture input".into(),
                    attachments: Vec::new(),
                    generation: "generation".into(),
                    replies,
                };
                let mut observations = vec![];
                let result = tokio::time::timeout(
                    Duration::from_secs(3),
                    execute(&mut wire, &mut run, &mut cancel, &mut |event| {
                        if let Event::Activity(items) = event {
                            observations.extend(items);
                        }
                        Ok(())
                    }),
                )
                .await
                .unwrap();
                if accepted {
                    assert!(matches!(result, Ok(false)), "{result:?}");
                    // Child observations are display-only; without an owned child row they
                    // must never enter the retained turn. Stale parent turns stay excluded.
                    let mut activity = crate::activity::TurnActivity::new(
                        crate::models::AgentProviderId::Codex,
                        None,
                    );
                    for observed in observations {
                        activity.observe(observed);
                    }
                    assert_eq!(activity.items.len(), 1);
                    assert_eq!(activity.items[0].id, "owned");
                    assert_eq!(
                        activity.items[0].state,
                        crate::activity::ItemState::Completed
                    );
                    // The command's first line is the row's detail (ADR-070); output never is.
                    assert_eq!(activity.items[0].detail, "private argument");
                } else {
                    assert!(result.is_err());
                }
                drop(wire);
                assert!(tokio::time::timeout(Duration::from_secs(3), child.wait())
                    .await
                    .unwrap()
                    .unwrap()
                    .success());
            }
        }
    }
    #[tokio::test]
    #[ignore = "real Codex app-server; existing CLI login, disposable isolated worktree only"]
    async fn live_codex_native_write_and_exact_followup() {
        let repo = crate::git::tests::Repo::new();
        let tree = crate::worktree::create_isolated(
            &repo.cwd(),
            &repo.0.join("workspaces"),
            "codexnative",
            "Native Codex smoke",
            "sirus/{session-name}-{id}",
        )
        .unwrap();
        let mut session = session();
        session.native_thread = None;
        session.worktree = tree.clone();
        session.model = std::env::var("SIRUS_SMOKE_MODEL").ok();
        let prompts = [
            "This is a disposable Sirus integration test. Remember the exact phrase SIRUS_NATIVE_REMEMBER_741. Create exactly one file sirus-native.txt in the current workspace containing SIRUS_NATIVE_OK followed by a newline. Do not change other files, use network tools, install dependencies, or read authentication/configuration files. Reply SIRUS_NATIVE_OK.",
            "Without any tools or filesystem access, repeat the exact phrase I asked you to remember in the previous turn. Reply with that phrase only.",
        ];
        let mut first_thread = None;
        for (index, prompt) in prompts.into_iter().enumerate() {
            let (process, mut started) = start(
                session.clone(),
                tree.path.clone(),
                prompt.into(),
                HashMap::new(),
                None,
            )
            .await
            .unwrap();
            let mut run = started.codex.take().unwrap();
            let err = started.child.stderr.take().unwrap();
            let diagnostics = tokio::spawn(async move {
                let mut reader = BufReader::new(err);
                while crate::agent::bounded_line(&mut reader)
                    .await
                    .is_ok_and(|line| line.is_some())
                {}
            });
            let mut wire = Wire {
                initialized: false,
                reusable: false,
                loaded: None,
                acp_init: Value::Null,
                acp_setup: Value::Null,
                input: started.child.stdin.take().unwrap(),
                output: BufReader::new(started.child.stdout.take().unwrap()),
                sequence: 0,
                deferred: Default::default(),
                deferred_bytes: 0,
                line: Vec::new(),
            };
            let mut output = String::new();
            let mut native = None;
            let mut callback = |event| {
                match event {
                    Event::Identity(identity) => native = Some(identity),
                    Event::Delta(text) => output.push_str(&text),
                    Event::Pending(_) => {
                        return Err(Error::agent("Unexpected approval in harmless native smoke"));
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
                execute(&mut wire, &mut run, &mut started.cancel, &mut callback),
            )
            .await;
            drop(wire);
            process.stop().unwrap();
            cleanup(&mut started.child).await;
            let _ = tokio::time::timeout(Duration::from_secs(2), diagnostics).await;
            assert!(
                matches!(outcome, Ok(Ok(false))),
                "Native Codex turn failed: {outcome:?}; vendor initialization/account data omitted"
            );
            let identity = native.expect("native thread identity");
            if index == 0 {
                first_thread = Some(identity.thread_id.clone());
                assert!(output.contains("SIRUS_NATIVE_OK"));
                assert_eq!(
                    std::fs::read_to_string(
                        std::path::Path::new(&tree.path).join("sirus-native.txt")
                    )
                    .unwrap(),
                    "SIRUS_NATIVE_OK\n"
                );
                assert!(!repo.cwd().join("sirus-native.txt").exists());
            } else {
                assert_eq!(Some(&identity.thread_id), first_thread.as_ref());
                assert!(
                    output.contains("SIRUS_NATIVE_REMEMBER_741"),
                    "Native follow-up did not remember prior turn"
                );
            }
            session.native_thread = Some(identity);
        }
    }

    #[tokio::test]
    #[ignore = "real Codex host approval callback; harmless command is declined"]
    async fn live_codex_declines_native_command_approval() {
        let repo = crate::git::tests::Repo::new();
        let tree = crate::worktree::create_isolated(
            &repo.cwd(),
            &repo.0.join("workspaces"),
            "codexapproval",
            "Codex approval smoke",
            "sirus/{session-name}-{id}",
        )
        .unwrap();
        let mut session = session();
        session.native_thread = None;
        session.worktree = tree.clone();
        let prompt = "This is a disposable host-approval integration test. Request one-time host approval using sandbox_permissions=require_escalated to execute exactly /usr/bin/true, with justification: harmless host approval routing test. Do not execute without requesting approval. The host will decline; do not retry or choose alternatives. After denial reply SIRUS_DENIED_OK. Do not read or write files, use network tools, inspect authentication/configuration, or change permissions.";
        let (process, mut started) = start(
            session.clone(),
            tree.path.clone(),
            prompt.into(),
            HashMap::new(),
            None,
        )
        .await
        .unwrap();
        let sender = process.replies.as_ref().unwrap().clone();
        let mut run = started.codex.take().unwrap();
        let mut wire = Wire {
            initialized: false,
            reusable: false,
            loaded: None,
            acp_init: Value::Null,
            acp_setup: Value::Null,
            input: started.child.stdin.take().unwrap(),
            output: BufReader::new(started.child.stdout.take().unwrap()),
            sequence: 0,
            deferred: Default::default(),
            deferred_bytes: 0,
            line: Vec::new(),
        };
        let err = started.child.stderr.take().unwrap();
        let diagnostics = tokio::spawn(async move {
            let mut reader = BufReader::new(err);
            while crate::agent::bounded_line(&mut reader)
                .await
                .is_ok_and(|line| line.is_some())
            {}
        });
        let mut acknowledgements = Vec::new();
        let mut output = String::new();
        let mut callback = |event| {
            match event {
                Event::Pending(pending) => {
                    if !matches!(&pending.kind,PendingRequestKind::Command {command,..} if command.contains("/usr/bin/true"))
                    {
                        return Err(Error::agent("Unexpected native callback in approval test"));
                    }
                    let (result, receiver) = oneshot::channel();
                    sender
                        .try_send(crate::codex::Inbound::Answer(Reply {
                            request: RespondAgentRequest {
                                session_id: session.id.clone(),
                                generation: pending.generation,
                                request_id: pending.request_id,
                                turn_id: pending.turn_id,
                                response: AgentResponse::Approval {
                                    decision: ApprovalDecision::Decline,
                                },
                            },
                            result,
                        }))
                        .map_err(|_| Error::agent("Approval test queue unavailable"))?;
                    acknowledgements.push(receiver);
                }
                Event::Delta(text) => output.push_str(&text),
                _ => {}
            }
            Ok(())
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(120),
            execute(&mut wire, &mut run, &mut started.cancel, &mut callback),
        )
        .await;
        drop(wire);
        process.stop().unwrap();
        cleanup(&mut started.child).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), diagnostics).await;
        assert!(
            matches!(outcome, Ok(Ok(false))),
            "Native declined turn did not complete: {outcome:?}"
        );
        assert!(
            !acknowledgements.is_empty(),
            "Provider did not request native command approval"
        );
        for receiver in acknowledgements {
            assert!(receiver.await.unwrap().is_ok());
        }
        assert!(output.contains("SIRUS_DENIED_OK"));
        assert!(
            !crate::git::status(std::path::Path::new(&tree.path))
                .unwrap()
                .dirty,
            "Declined approval test must not edit workspace"
        );
    }

    #[test]
    fn vendor_resolution_invalidates_pending_callback_before_user_response() {
        let pending = PendingRequest {
            request_id: "opaque".into(),
            generation: "g".into(),
            turn_id: "t".into(),
            item_id: "i".into(),
            kind: PendingRequestKind::FileChange {
                reason: None,
                changes: vec![],
            },
        };
        let mut ledger = PendingLedger {
            session_id: "s".into(),
            generation: "g".into(),
            turn_id: "t".into(),
            requests: [("opaque".into(), (json!(17), pending))].into(),
        };
        assert_eq!(ledger.resolve(&json!(17)).as_deref(), Some("opaque"));
        let request = RespondAgentRequest {
            session_id: "s".into(),
            generation: "g".into(),
            turn_id: "t".into(),
            request_id: "opaque".into(),
            response: AgentResponse::Approval {
                decision: ApprovalDecision::Accept,
            },
        };
        assert!(ledger.take(&request).is_err());
    }
    #[test]
    fn file_approval_requires_reviewable_native_change_details() {
        assert!(pending_kind(
            "item/fileChange/requestApproval",
            &json!({"itemId":"file","reason":"Apply edits"})
        )
        .unwrap()
        .is_none());
    }
    #[test]
    fn response_is_consumed_once_and_bound_to_session_generation_and_turn() {
        let pending = PendingRequest {
            request_id: "opaque".into(),
            generation: "generation".into(),
            turn_id: "turn".into(),
            item_id: "item".into(),
            kind: PendingRequestKind::FileChange {
                reason: None,
                changes: vec![],
            },
        };
        let mut ledger = PendingLedger {
            session_id: "session".into(),
            generation: "generation".into(),
            turn_id: "turn".into(),
            requests: [("opaque".into(), (json!(42), pending))].into(),
        };
        let response = RespondAgentRequest {
            session_id: "session".into(),
            generation: "generation".into(),
            request_id: "opaque".into(),
            turn_id: "turn".into(),
            response: AgentResponse::Approval {
                decision: ApprovalDecision::Decline,
            },
        };
        for field in ["sessionId", "generation", "turnId", "requestId"] {
            let mut raw = serde_json::to_value(&response).unwrap();
            raw[field] = json!("foreign");
            assert!(ledger.take(&serde_json::from_value(raw).unwrap()).is_err());
            assert_eq!(ledger.requests.len(), 1);
        }
        assert_eq!(
            ledger.take(&response).unwrap(),
            (json!(42), json!({"decision":"decline"}))
        );
        assert!(ledger.take(&response).is_err());
    }
    #[test]
    fn resume_refuses_foreign_workspace_session_project_or_model() {
        let original = session();
        assert!(validate_binding(&original, "/fixture").is_ok());
        assert!(validate_binding(&original, "/different").is_err());
        for field in ["sessionId", "projectId", "model", "providerAccountId"] {
            let mut raw = serde_json::to_value(&original).unwrap();
            raw["nativeThread"][field] = json!("foreign");
            let changed: Session = serde_json::from_value(raw).unwrap();
            assert!(validate_binding(&changed, "/fixture").is_err());
        }
    }
    #[test]
    fn approval_is_one_request_only_and_question_answers_match_native_options() {
        let mut pending = PendingRequest {
            request_id: "r".into(),
            generation: "g".into(),
            turn_id: "t".into(),
            item_id: "i".into(),
            kind: PendingRequestKind::Command {
                command: "echo harmless".into(),
                cwd: None,
                reason: None,
            },
        };
        assert_eq!(
            answer(
                &pending,
                &AgentResponse::Approval {
                    decision: ApprovalDecision::Accept
                }
            )
            .unwrap(),
            json!({"decision":"accept"})
        );
        pending.kind = PendingRequestKind::UserInput {
            questions: vec![InputQuestion {
                id: "q".into(),
                header: "Pick".into(),
                question: "Pick one".into(),
                is_other: false,
                is_secret: false,
                options: Some(vec![InputOption {
                    label: "A".into(),
                    description: "Option A".into(),
                }]),
            }],
        };
        let response = |id: &str, value: &str| AgentResponse::UserInput {
            answers: [(id.into(), vec![value.into()])].into(),
        };
        assert_eq!(
            answer(&pending, &response("q", "A")).unwrap(),
            json!({"answers":{"q":{"answers":["A"]}}})
        );
        assert!(answer(&pending, &response("foreign", "A")).is_err());
        assert!(answer(&pending, &response("q", "unoffered")).is_err());
        assert!(answer(
            &pending,
            &AgentResponse::Approval {
                decision: ApprovalDecision::Accept
            }
        )
        .is_err());
    }
}
