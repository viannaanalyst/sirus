use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use crate::runtime::{AppHandle, Emitter};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::Child;
#[cfg(test)]
use tokio::process::Command;
use tokio::sync::watch;

use crate::agent_output::OutputParser;
use crate::commands::AppState;
use crate::detect;
use crate::error::{Error, Result};
use crate::models::{
    AgentEvent, AgentExitEvent, AgentProviderId, Message, MessageRole, Session, SessionStatus,
};
use crate::paths::now_rfc3339;

pub struct AgentProcess {
    pub(crate) cancel: watch::Sender<bool>,
    pub(crate) replies: Option<tokio::sync::mpsc::Sender<crate::codex::Inbound>>,
    pub(crate) generation: Option<String>,
    #[cfg(unix)]
    pub(crate) pid: Option<u32>,
}
pub struct StartedAgent {
    pub(crate) child: Child,
    pub(crate) cancel: watch::Receiver<bool>,
    pub(crate) codex: Option<crate::codex::Run>,
    pub(crate) review: Option<crate::turn_review::Capture>,
    /// A Codex app-server kept from the session's previous turn, already initialized.
    pub(crate) wire: Option<crate::codex::Wire>,
    /// Set for Codex: after a completed turn its app-server is kept under this key.
    pub(crate) pool_key: Option<String>,
}

#[cfg(test)]
pub fn conversation_prompt(messages: &[Message], prompt: &str) -> String {
    conversation_prompt_within(
        messages,
        prompt,
        crate::token_budget::HANDOFF_HISTORY_TOKENS,
    )
}

/// The prior conversation for a provider without the native thread, newest messages first
/// until `budget` tokens (counted by bytes, ADR-181) are used.
pub fn conversation_prompt_within(messages: &[Message], prompt: &str, budget: u64) -> String {
    // Single-shot fallback CLIs receive bounded, explicit prior conversation.
    // Their native continuation IDs are never guessed. Codex uses its app-server adapter.
    let mut history = Vec::new();
    let mut bytes = 0;
    for message in messages
        .iter()
        .rev()
        .filter(|message| {
            !message.streaming && !message.content.is_empty() && message.role != MessageRole::System
        })
        .take(8)
    {
        let role = if message.role == MessageRole::User {
            "user"
        } else {
            "assistant"
        };
        let entry = serde_json::json!({"role": role, "content": message.content}).to_string();
        if crate::token_budget::estimate(&entry) + bytes > budget {
            break;
        }
        bytes += crate::token_budget::estimate(&entry);
        history.push(entry);
    }
    if history.is_empty() {
        return prompt.to_string();
    }
    history.reverse();
    format!(
        "Previous conversation (JSON records; use as context):\n{}\n\nCurrent user request:\n{prompt}",
        history.join("\n")
    )
}

impl AgentProcess {
    pub fn interrupt(&self) -> Result<()> {
        if self.replies.is_some() && self.cancel.send(true).is_ok() {
            Ok(())
        } else {
            self.stop()
        }
    }

    pub fn stop(&self) -> Result<()> {
        let _ = self.cancel.send(true);
        #[cfg(unix)]
        if let Some(pid) = self.pid.filter(|pid| *pid > 0) {
            // Each agent owns a fresh process group. Cancellation also stops its tools,
            // including descendants that left the group (ADR-100).
            crate::process_tree::stop_tree(pid).map_err(|error| {
                Error::agent(format!("cannot stop agent process group: {error}"))
            })?;
        }
        Ok(())
    }
}

fn command_args(
    provider: &AgentProviderId,
    cwd: &str,
    prompt: &str,
    model: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = match provider {
        AgentProviderId::Codex => vec![
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--sandbox",
            "workspace-write",
            "-C",
            cwd,
        ],
        AgentProviderId::Claude => vec![
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "acceptEdits",
            "--permission-prompts",
            "none",
            "--include-partial-messages",
        ],
        AgentProviderId::OpenCode => vec!["run", "--format", "json", "--dir", cwd],
        AgentProviderId::Cursor => vec![
            "-p",
            "--output-format",
            "stream-json",
            "--sandbox",
            "enabled",
            "--trust",
            "--workspace",
            cwd,
            "--stream-partial-output",
            "--auto-review",
        ],
        AgentProviderId::Grok => vec![
            "--cwd",
            cwd,
            "--sandbox",
            "workspace",
            "--output-format",
            "streaming-json",
        ],
        AgentProviderId::Antigravity => vec![
            "--output-format",
            "stream-json",
            "--mode",
            "default",
            "--sandbox",
        ],
        AgentProviderId::Droid => vec![
            "exec",
            "--output-format",
            "stream-json",
            "--cwd",
            cwd,
            "--auto",
            "low",
        ],
        AgentProviderId::Pi => vec![
            "--print",
            "--mode",
            "json",
            "--no-session",
            "--no-extensions",
            "--no-approve",
            "--offline",
            "--tools",
            "read,grep,find,ls,edit,write",
        ],
        // Devin and Hermes run through ACP (`acp_cli`); this print form stays for Devin's fallback.
        AgentProviderId::Hermes => vec!["acp"],
        AgentProviderId::Devin => vec![
            "--print",
            "--permission-mode",
            "accept-edits",
            "--sandbox",
            "--respect-workspace-trust",
            "true",
        ],
    }
    .into_iter()
    .map(str::to_string)
    .collect();
    if let Some(model) = model.filter(|model| !model.trim().is_empty()) {
        args.extend(["--model".into(), model.into()]);
    }
    if matches!(
        provider,
        AgentProviderId::Grok | AgentProviderId::Antigravity
    ) {
        // Grok's documented single-shot flag takes a value. The prefix keeps a
        // leading option-like prompt unambiguous without granting permissions.
        args.extend(["-p".into(), format!("Task:\n{prompt}")]);
    } else {
        args.extend(["--".into(), prompt.into()]);
    }
    args
}

fn opencode_private_config(inline: Option<&str>) -> Result<String> {
    let mut value: serde_json::Value = match inline.filter(|value| !value.trim().is_empty()) {
        Some(raw) => serde_json::from_str(raw)
            .map_err(|_| Error::agent("OpenCode inline configuration is invalid JSON"))?,
        None => serde_json::json!({}),
    };
    let object = value
        .as_object_mut()
        .ok_or_else(|| Error::agent("OpenCode inline configuration must be an object"))?;
    object.insert("share".into(), serde_json::json!("disabled"));
    Ok(value.to_string())
}

fn execution_args(
    provider: &AgentProviderId,
    cwd: &str,
    prompt: &str,
    model: Option<&str>,
    execution: &crate::models::ExecutionOptions,
) -> Vec<String> {
    let mut args = command_args(provider, cwd, prompt, model);
    if execution.planning {
        if *provider == AgentProviderId::Pi {
            if let Some(index) = args.iter().position(|arg| arg == "--tools") {
                args[index + 1] = "read,grep,find,ls".into();
            }
        } else if *provider == AgentProviderId::Droid {
            if let Some(index) = args.iter().position(|arg| arg == "--auto") {
                args.drain(index..index + 2);
            }
        }
    }
    if *provider == AgentProviderId::Cursor
        && execution.approval == Some(crate::models::ApprovalMode::Full)
    {
        // Fixed native mapping only; no renderer argv/config passthrough.
        if let Some(index) = args.iter().position(|arg| arg == "--auto-review") {
            args.remove(index);
        }
        if let Some(index) = args.iter().position(|arg| arg == "--sandbox") {
            args[index + 1] = "disabled".into();
        }
        args.insert(0, "--force".into());
    }
    args
}

pub async fn start(
    provider: AgentProviderId,
    cwd: String,
    prompt: String,
    model: Option<String>,
    overrides: HashMap<AgentProviderId, String>,
    execution: crate::models::ExecutionOptions,
) -> Result<(AgentProcess, StartedAgent)> {
    // Resolve only this executable. Detection/version probes never belong in the send path.
    let install = detect::resolve_with_overrides(&provider, &overrides)
        .ok_or_else(|| Error::agent("unknown provider"))?;
    if !install.installed {
        return Err(Error::agent(format!(
            "{} is not installed on this machine",
            install.name
        )));
    }
    let mut cmd = detect::command(install.path.unwrap_or(install.binary));
    if provider == AgentProviderId::OpenCode {
        // Prevent inherited auto-sharing without changing the user's vendor config or login.
        let inline = std::env::var("OPENCODE_CONFIG_CONTENT").ok();
        cmd.env(
            "OPENCODE_CONFIG_CONTENT",
            opencode_private_config(inline.as_deref())?,
        )
        .env("OPENCODE_AUTO_SHARE", "false");
    }
    if execution.planning && provider == AgentProviderId::Cursor {
        cmd.args(["--mode", "plan"]);
    }
    if provider == AgentProviderId::Grok {
        if let Some(effort) = execution.effort.as_deref() {
            cmd.args(["--effort", effort]);
        }
    }
    cmd.args(execution_args(
        &provider,
        &cwd,
        &prompt,
        model.as_deref(),
        &execution,
    ))
    .current_dir(cwd)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    crate::process_priority::lower_async(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|err| Error::agent(format!("failed to start agent: {err}")))?;
    let (cancel, receiver) = watch::channel(false);
    Ok((
        AgentProcess {
            cancel,
            replies: None,
            generation: None,
            #[cfg(unix)]
            pid: child.id(),
        },
        StartedAgent {
            child,
            cancel: receiver,
            codex: None,
            review: None,
            wire: None,
            pool_key: None,
        },
    ))
}

// Observe without reaping: retaining the leader prevents PID/group identity reuse
// until every signal to the owned group has been issued.
#[cfg(unix)]
pub(crate) async fn observe_owned_exit(child: &Child) -> std::io::Result<()> {
    let pid = child
        .id()
        .ok_or_else(|| std::io::Error::other("agent already reaped"))?;
    tokio::task::spawn_blocking(move || {
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        loop {
            let result =
                unsafe { libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) };
            if result == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

/// How long an idle CLI gets to exit by itself once its input is closed. A Claude process
/// that just refreshed its OAuth token persists the rotated credential through a `security`
/// child in its own group; killing the group first loses that write and leaves the stored
/// login unusable (ADR-165).
pub(crate) const GRACEFUL_EXIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Ends a process whose input the caller has already closed (stdin or `Wire` dropped): it
/// gets `GRACEFUL_EXIT` to finish and exit, then whatever is left of its group is killed and
/// the leader reaped.
pub(crate) async fn retire_owned(mut child: Child) {
    #[cfg(unix)]
    {
        let observed = tokio::time::timeout(GRACEFUL_EXIT, observe_owned_exit(&child)).await;
        // Never signal a reaped/reused identity if observation failed.
        if !matches!(observed, Ok(Err(_))) {
            kill_owned_group(&mut child);
        }
    }
    #[cfg(not(unix))]
    if tokio::time::timeout(GRACEFUL_EXIT, child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
    }
    let _ = child.wait().await;
}

pub(crate) fn kill_owned_group(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // Only called before child.wait()/try_wait(): leader is still PID-owned.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

#[cfg(unix)]
async fn stop_after_owned_exit(
    child: &mut Child,
    cancel: &mut watch::Receiver<bool>,
) -> std::io::Result<()> {
    if !*cancel.borrow() {
        tokio::select! {
            result = observe_owned_exit(child) => result?,
            _ = cancel.changed() => {},
        }
    }
    // Stopped: `stop_tree` already sent the leader SIGTERM. It gets time to finish what it is
    // writing (a rotated OAuth token, ADR-165); only then does the group go.
    if let Ok(result) =
        tokio::time::timeout(crate::process_tree::LEADER_GRACE, observe_owned_exit(child)).await
    {
        result?;
    }
    kill_owned_group(child);
    observe_owned_exit(child).await
}

#[cfg(test)]
async fn wait_cancellable(
    child: &mut Child,
    cancel: &mut watch::Receiver<bool>,
) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(unix)]
    {
        stop_after_owned_exit(child, cancel).await?;
        child.wait().await
    }
    #[cfg(not(unix))]
    tokio::select! {
        status = child.wait() => status,
        _ = cancel.changed() => {
            child.start_kill()?;
            child.wait().await
        }
    }
}

pub(crate) async fn drain_output_readers(
    readers: impl IntoIterator<Item = tokio::task::JoinHandle<bool>>,
) -> bool {
    let mut failed = false;
    for mut reader in readers {
        match tokio::time::timeout(std::time::Duration::from_secs(2), &mut reader).await {
            Ok(Ok(reader_failed)) => failed |= reader_failed,
            Ok(Err(_)) => failed = true,
            Err(_) => {
                reader.abort();
                let _ = reader.await;
                failed = true;
            }
        }
    }
    failed
}

impl StartedAgent {
    pub async fn reap(mut self) {
        let _ = self.child.wait().await;
    }
    pub fn monitor(mut self, app: AppHandle, state: Arc<AppState>, session_id: String) {
        // Every provider's turn passes here: the Mac stays awake until the turn's
        // monitor ends (ADR-100). Agents already started lowered (ADR-143).
        let wanted = state.data.lock().settings.keep_awake_while_working;
        let awake = crate::keep_awake::hold_if(wanted);
        if let Some(run) = self.codex.take() {
            crate::codex::monitor(self, run, app, state, session_id, awake);
            return;
        }
        let stdout = self.child.stdout.take();
        let stderr = self.child.stderr.take();
        tokio::spawn(async move {
            let _awake = awake;
            let out = stdout.map(|reader| {
                spawn_reader(
                    app.clone(),
                    state.clone(),
                    session_id.clone(),
                    "stdout",
                    reader,
                )
            });
            let err = stderr.map(|reader| {
                spawn_reader(
                    app.clone(),
                    state.clone(),
                    session_id.clone(),
                    "stderr",
                    reader,
                )
            });
            #[cfg(unix)]
            let observed = stop_after_owned_exit(&mut self.child, &mut self.cancel).await;
            #[cfg(not(unix))]
            let status = tokio::select! {
                status = self.child.wait() => status,
                _ = self.cancel.changed() => {
                    let _ = self.child.start_kill();
                    self.child.wait().await
                }
            };
            // Drain both pipes before publishing completion, including the final output line.
            let protocol_failed = drain_output_readers([out, err].into_iter().flatten()).await;
            let review =
                crate::turn_review::finish(self.review.take(), state.clone(), session_id.clone())
                    .await;
            let code;
            {
                let mut data = state.data.lock();
                // Admission and unregistering stay under the session lock. Remove
                // the signal handle before reaping the still-owned leader.
                state.agents.lock().remove(&session_id);
                #[cfg(unix)]
                let status = observed.and_then(|()| {
                    self.child
                        .try_wait()?
                        .ok_or_else(|| std::io::Error::other("agent exit was not observed"))
                });
                code = status.as_ref().ok().and_then(|status| status.code());
                let session = data.sessions.iter_mut().find(|s| s.id == session_id);
                let session = session.map(|session| {
                    let wait_error = status.as_ref().err().map(ToString::to_string);
                    finalize_session(
                        session,
                        code,
                        *self.cancel.borrow(),
                        protocol_failed,
                        state.closing.load(std::sync::atomic::Ordering::Acquire),
                        wait_error.as_deref(),
                    );
                    crate::turn_review::attach(session, review);
                    session.clone()
                });
                if let Err(error) = crate::persist::save(&state.data_path, &data) {
                    tracing::error!(%error, "cannot persist agent completion");
                }
                // Keep lifecycle publication ordered with admission of the next run.
                if let Some(session) = session {
                    crate::notifications::publish(&app, &state, &session);
                    crate::transcript_view::emit(&app, &session);
                }
            }
            crate::team::settled(&app, &state, &session_id);
            crate::delegation::settled(&app, &state, &session_id);
            crate::ci_autofix::settled(&state, &session_id);
            crate::project_scripts::settled(&app, &state, &session_id);
            crate::output_batch::finish(&app, &session_id);
            let _ = app.emit("agent-exit", AgentExitEvent { session_id, code });
        });
    }
}

pub(crate) fn finalize_session(
    session: &mut Session,
    code: Option<i32>,
    cancelled: bool,
    protocol_failed: bool,
    closing: bool,
    wait_error: Option<&str>,
) {
    session.status = if session.status == SessionStatus::Failed && (protocol_failed || closing) {
        SessionStatus::Failed
    } else if cancelled || closing {
        SessionStatus::Stopped
    } else if code == Some(0) && !protocol_failed {
        SessionStatus::Completed
    } else {
        SessionStatus::Failed
    };
    crate::activity::sync(session);
    // Secret references, env files and open secret cards never outlive the turn (ADR-077).
    crate::secrets::end_turn(&session.id);
    session.last_activity_at = now_rfc3339();
    if session.status == SessionStatus::Completed {
        // A turn that ends normally clears the automatic-continuation chain (ADR-118).
        session.auto_continuations = 0;
        if let (Some(origin), Some(thread)) = (&mut session.fork_origin, &session.native_thread) {
            origin.seeded_native_thread_id = Some(thread.thread_id.clone());
        }
    }
    for message in &mut session.messages {
        message.streaming = false;
    }
    // Nothing answers an approval once the turn is over; one left listed would hold the next
    // message back in the composer.
    session.pending_requests.clear();
    session.last_error = if session.status == SessionStatus::Failed {
        session.last_error.clone().or_else(|| {
            Some(wait_error.map(str::to_owned).unwrap_or_else(|| {
                if protocol_failed {
                    return "Agent reported an error; see output.".into();
                }
                code.map(|code| format!("Agent exited with status {code}"))
                    .unwrap_or_else(|| "Agent process terminated without an exit code".into())
            }))
        })
    } else if closing {
        session
            .last_error
            .clone()
            .or_else(|| Some("Execution was interrupted when Sirus closed.".into()))
    } else {
        None
    };
}

const MAX_OUTPUT_LINE: usize = 1024 * 1024;
const MAX_MESSAGE_OUTPUT: usize = 8 * 1024 * 1024;

pub(crate) async fn bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |index| index + 1);
        if bytes.len() + count > MAX_OUTPUT_LINE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "agent output line exceeds 1 MiB",
            ));
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            break;
        }
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes).map(Some).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "agent output is not UTF-8")
    })
}

fn fail_stream(app: &AppHandle, state: &AppState, session_id: &str, message: &str) {
    let mut data = state.data.lock();
    if let Some(session) = data
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
    {
        // A user cancellation stays a cancellation; stream limits are failures.
        if session.status != SessionStatus::Stopped {
            session.status = SessionStatus::Failed;
            crate::activity::sync(session);
            session.last_error = Some(message.into());
            crate::transcript_view::emit(app, session);
        }
    }
    if let Some(process) = state.agents.lock().get(session_id) {
        if let Err(error) = process.stop() {
            tracing::warn!(%error, "cannot stop failed output stream");
        }
    }
    if let Err(error) = crate::persist::save(&state.data_path, &data) {
        tracing::error!(%error, "cannot persist output failure");
    }
}

/// Per-stream position: the message it appends to and its known length, so each
/// chunk's UTF-16 offset is not recounted over the whole message.
#[derive(Default)]
pub(crate) struct OutputCursor {
    message_id: Option<String>,
    /// (UTF-8 bytes, UTF-16 units) after the last append by this cursor.
    known: Option<(usize, usize)>,
}

pub(crate) fn record_output(
    session: &mut Session,
    cursor: &mut OutputCursor,
    stream: &str,
    chunk: String,
) -> Result<Option<AgentEvent>> {
    // An echoed secret value never reaches the transcript.
    let chunk = crate::secrets::scrub(&session.id, chunk);
    let message_id = &mut cursor.message_id;
    if matches!(
        session.status,
        SessionStatus::Stopped | SessionStatus::Failed
    ) {
        return Ok(None);
    }
    let first = message_id.is_none();
    if first {
        if stream == "stderr" {
            let id = uuid::Uuid::new_v4().to_string();
            session.messages.append(Message {
                id: id.clone(),
                session_id: session.id.clone(),
                role: MessageRole::System,
                content: String::new(),
                created_at: now_rfc3339(),
                streaming: true,
                activity: None,
                steers: Vec::new(),
                attachments: Vec::new(),
                launched: vec![],
                visuals: Vec::new(),
                client_request_id: None,
                rotation: None,
            });
            *message_id = Some(id);
        } else {
            *message_id = session
                .messages
                .iter()
                .rev()
                .find(|message| message.role == MessageRole::Agent)
                .map(|message| message.id.clone());
        }
    }
    // The streamed message is the newest or close to it; only it and later rows
    // count as changed for the next checkpoint.
    let Some(message) = session
        .messages
        .rev_find_mut(|message| Some(&message.id) == message_id.as_ref())
    else {
        return Ok(None);
    };
    if message.content.len() + chunk.len() > MAX_MESSAGE_OUTPUT {
        return Err(Error::agent(
            "Agent output exceeded the 8 MiB limit. Execution was stopped to preserve responsiveness.",
        ));
    }
    let offset = match cursor.known {
        // Streaming only appends; a length mismatch means another writer changed it.
        Some((bytes, units)) if bytes == message.content.len() => units,
        _ => message.content.encode_utf16().count(),
    };
    message.content.push_str(&chunk);
    cursor.known = Some((message.content.len(), offset + chunk.encode_utf16().count()));
    session.last_activity_at = now_rfc3339();
    Ok(Some(AgentEvent {
        session_id: session.id.clone(),
        message_id: message.id.clone(),
        offset,
        message: first.then(|| message.clone()),
        stream: stream.into(),
        chunk,
    }))
}

pub(crate) fn spawn_reader<R>(
    app: AppHandle,
    state: Arc<AppState>,
    session_id: String,
    stream: &'static str,
    reader: R,
) -> tokio::task::JoinHandle<bool>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut parser = state
            .data
            .lock()
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .map(|session| OutputParser::for_provider(&session.agent))
            .unwrap_or_default();
        let mut cursor = OutputCursor::default();
        let mut lines = BufReader::new(reader);
        loop {
            let chunk = match bounded_line(&mut lines).await {
                Ok(Some(line)) if stream == "stdout" => parser.parse(&line),
                Ok(Some(line)) => format!("{line}\n"),
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(%error, "agent output stream failed");
                    fail_stream(
                        &app,
                        &state,
                        &session_id,
                        "Agent output could not be read safely. Check the CLI diagnostics.",
                    );
                    return true;
                }
            };
            if chunk.is_empty() {
                continue;
            }
            {
                let mut data = state.data.lock();
                let event =
                    if let Some(session) = data.sessions.iter_mut().find(|s| s.id == session_id) {
                        match record_output(session, &mut cursor, stream, chunk) {
                            Ok(Some(event)) => event,
                            Ok(None) => continue,
                            Err(error) => {
                                drop(data);
                                fail_stream(&app, &state, &session_id, &error.to_string());
                                return true;
                            }
                        }
                    } else {
                        continue;
                    };
                // Event-driven, coalesced checkpoint written outside the lock. Final
                // output is always saved by the monitor; crashes retain recent text.
                crate::persist::checkpoint_soon(&state, &session_id);
                crate::output_batch::emit(&app, event);
            }
        }
        parser.failed()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_cli_adapters_pin_permissions_and_isolate_prompt_arguments() {
        use crate::models::ExecutionOptions;
        for provider in [
            AgentProviderId::Antigravity,
            AgentProviderId::Droid,
            AgentProviderId::Pi,
            AgentProviderId::Devin,
        ] {
            let args = execution_args(
                &provider,
                "/workspace",
                "--unsafe-prompt",
                Some("vendor/model"),
                &ExecutionOptions::default(),
            );
            assert!(!args.iter().any(|arg| arg.contains("dangerously")
                || arg.contains("skip-permissions")
                || arg == "--cloud"));
            assert_eq!(
                args[args.iter().position(|arg| arg == "--model").unwrap() + 1],
                "vendor/model"
            );
            if provider == AgentProviderId::Antigravity {
                assert_eq!(args.last().unwrap(), "Task:\n--unsafe-prompt");
                assert!(args.contains(&"--sandbox".into()));
            } else {
                assert_eq!(&args[args.len() - 2..], &["--", "--unsafe-prompt"]);
            }
        }
        let plan = ExecutionOptions {
            planning: true,
            ..Default::default()
        };
        let droid = execution_args(&AgentProviderId::Droid, "/workspace", "plan", None, &plan);
        assert!(!droid.contains(&"--auto".into()));
        let pi = execution_args(&AgentProviderId::Pi, "/workspace", "plan", None, &plan);
        assert_eq!(
            pi[pi.iter().position(|arg| arg == "--tools").unwrap() + 1],
            "read,grep,find,ls"
        );
        assert!(pi.contains(&"--no-extensions".into()) && pi.contains(&"--no-approve".into()));
    }
    #[test]
    fn cursor_full_access_does_not_change_prompt_or_model_and_default_restores_sandbox() {
        let options = serde_json::from_value(serde_json::json!({"approval":"full"})).unwrap();
        let args = execution_args(
            &AgentProviderId::Cursor,
            "/fixture",
            "--auto-review",
            Some("model"),
            &options,
        );
        assert_eq!(args.first().map(String::as_str), Some("--force"));
        let sandbox = args.iter().position(|arg| arg == "--sandbox").unwrap();
        assert_eq!(args[sandbox + 1], "disabled");
        assert_eq!(
            &args[args.len() - 4..],
            &["--model", "model", "--", "--auto-review"]
        );
        let default = execution_args(
            &AgentProviderId::Cursor,
            "/fixture",
            "hello",
            None,
            &Default::default(),
        );
        assert!(!default.iter().any(|arg| arg == "--force"));
        assert!(default.iter().any(|arg| arg == "--auto-review"));
        assert_eq!(
            default[default.iter().position(|arg| arg == "--sandbox").unwrap() + 1],
            "enabled"
        );
    }
    #[test]
    fn diagnostics_are_native_system_messages_and_never_conversation_context() {
        let mut session: Session = serde_json::from_value(serde_json::json!({"id":"s","title":"Test","projectId":"p","agent":"codex","status":"running","createdAt":"time","lastActivityAt":"time","worktree":{"path":"/unused","branch":"main","isolated":false},"messages":[{"id":"a","sessionId":"s","role":"agent","content":"🚂","createdAt":"time","streaming":true}]})).unwrap();
        let mut out_id = OutputCursor::default();
        let mut err_id = OutputCursor::default();
        let err = record_output(&mut session, &mut err_id, "stderr", "CLI warning".into())
            .unwrap()
            .unwrap();
        assert_eq!(err.message.unwrap().role, MessageRole::System);
        let out = record_output(&mut session, &mut out_id, "stdout", " reply".into())
            .unwrap()
            .unwrap();
        assert_eq!(out.message_id, "a");
        assert_eq!(out.offset, 2);
        session.messages[0].streaming = false;
        let prompt = conversation_prompt(&session.messages, "next");
        assert!(!prompt.contains("CLI warning"));
        assert!(prompt.contains("reply"));
        let next = record_output(&mut session, &mut err_id, "stderr", " more".into())
            .unwrap()
            .unwrap();
        assert!(next.message.is_none());
        assert_eq!(session.messages.len(), 2);
    }
    #[test]
    fn streamed_offsets_stay_exact_without_recounting_and_after_outside_edits() {
        let mut session: Session = serde_json::from_value(serde_json::json!({"id":"s","title":"Test","projectId":"p","agent":"codex","status":"running","createdAt":"time","lastActivityAt":"time","worktree":{"path":"/unused","branch":"main","isolated":false},"messages":[{"id":"a","sessionId":"s","role":"agent","content":"","createdAt":"time","streaming":true}]})).unwrap();
        let mut cursor = OutputCursor::default();
        let mut expected = 0;
        for chunk in ["ação ", "🚂🚂", " fim"] {
            let event = record_output(&mut session, &mut cursor, "stdout", chunk.into())
                .unwrap()
                .unwrap();
            assert_eq!(event.offset, expected);
            expected += chunk.encode_utf16().count();
        }
        session.messages[0].content.push('✓');
        let event = record_output(&mut session, &mut cursor, "stdout", "!".into())
            .unwrap()
            .unwrap();
        assert_eq!(event.offset, expected + 1, "an outside append is recounted");
    }
    #[test]
    fn a_failed_turn_leaves_no_pending_request_behind() {
        let mut session: Session = serde_json::from_value(serde_json::json!({"id":"s","title":"Test","projectId":"p","agent":"claude","status":"waiting","createdAt":"time","lastActivityAt":"time","worktree":{"path":"/unused","branch":"main","isolated":false},"messages":[]})).unwrap();
        session
            .pending_requests
            .push(crate::models::PendingRequest {
                request_id: "r".into(),
                generation: "g".into(),
                turn_id: "t".into(),
                item_id: "i".into(),
                kind: crate::models::PendingRequestKind::UserInput { questions: vec![] },
            });
        finalize_session(&mut session, Some(1), false, false, false, None);
        assert_eq!(session.status, SessionStatus::Failed);
        assert!(session.pending_requests.is_empty());
    }
    #[test]
    fn a_late_process_exit_preserves_shutdown_interruption() {
        let mut session: Session = serde_json::from_value(serde_json::json!({"id":"s","title":"Test","projectId":"p","agent":"codex","status":"stopped","createdAt":"time","lastActivityAt":"time","worktree":{"path":"/unused","branch":"main","isolated":false},"lastError":"Execution was interrupted when Sirus closed.","messages":[]})).unwrap();
        finalize_session(&mut session, Some(0), true, false, true, None);
        assert_eq!(session.status, SessionStatus::Stopped);
        assert_eq!(
            session.last_error.as_deref(),
            Some("Execution was interrupted when Sirus closed.")
        );
        session.status = SessionStatus::Failed;
        session.last_error = Some("Output limit".into());
        finalize_session(&mut session, Some(0), true, true, true, None);
        assert_eq!(session.status, SessionStatus::Failed);
        assert_eq!(session.last_error.as_deref(), Some("Output limit"));
        finalize_session(&mut session, Some(0), true, false, false, None);
        assert_eq!(session.status, SessionStatus::Stopped);
        assert!(session.last_error.is_none());
    }

    #[tokio::test]
    async fn stream_lines_are_bounded_and_preserve_a_final_partial_line() {
        let mut input = BufReader::new(&b"one\r\nlast"[..]);
        assert_eq!(
            bounded_line(&mut input).await.unwrap().as_deref(),
            Some("one")
        );
        assert_eq!(
            bounded_line(&mut input).await.unwrap().as_deref(),
            Some("last")
        );
        assert_eq!(bounded_line(&mut input).await.unwrap(), None);
        let huge = vec![b'x'; MAX_OUTPUT_LINE + 1];
        let mut input = BufReader::new(huge.as_slice());
        assert!(bounded_line(&mut input).await.is_err());
        let mut invalid = BufReader::new(&b"\xff\n"[..]);
        assert!(bounded_line(&mut invalid).await.is_err());
    }
    #[tokio::test]
    async fn failed_diagnostics_preserve_failure_after_successful_native_result() {
        let mut session: Session = serde_json::from_value(serde_json::json!({
            "id":"s", "projectId":"p", "title":"Task", "agent":"codex", "status":"failed",
            "createdAt":"time", "lastActivityAt":"time", "lastError":"invalid UTF-8 diagnostic",
            "worktree":{"path":"/fixture", "branch":"main", "isolated":false}, "messages":[]
        }))
        .unwrap();
        let failed = drain_output_readers([tokio::spawn(async { true })]).await;
        finalize_session(&mut session, Some(0), true, failed, false, None);
        assert_eq!(session.status, SessionStatus::Failed);
        assert_eq!(
            session.last_error.as_deref(),
            Some("invalid UTF-8 diagnostic")
        );
        assert!(!drain_output_readers([tokio::spawn(async { false })]).await);
        assert!(drain_output_readers([tokio::spawn(async { panic!("reader failure") })]).await);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_closes_pipes_owned_by_descendant_tools() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt};
        let mut child = Command::new("/bin/sh")
            .args(["-c", "sleep 30 & echo ready; wait"])
            .process_group(0)
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            output.read_line(&mut ready),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(ready.trim(), "ready");
        let (cancel, mut receiver) = watch::channel(false);
        let process = AgentProcess {
            cancel,
            replies: None,
            generation: None,
            pid: child.id(),
        };
        process.stop().unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            wait_cancellable(&mut child, &mut receiver),
        )
        .await
        .unwrap()
        .unwrap();
        let mut rest = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            output.read_to_string(&mut rest),
        )
        .await
        .expect("descendant must not keep the output pipe open")
        .unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn normal_exit_cleans_pipe_owning_descendant_and_preserves_unrelated_process() {
        use tokio::io::AsyncReadExt;
        let mut unrelated = Command::new("/bin/sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "sleep 30 & printf 'final output\\n'; exit 0"])
            .process_group(0)
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut output = child.stdout.take().unwrap();
        let (_sender, mut receiver) = watch::channel(false);
        let status = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            wait_cancellable(&mut child, &mut receiver),
        )
        .await
        .unwrap()
        .unwrap();
        let mut bytes = Vec::new();
        let drained = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            output.read_to_end(&mut bytes),
        )
        .await;
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().await.unwrap();
        assert!(status.success());
        assert!(
            drained.is_ok(),
            "normal exit must clean descendants retaining output pipes"
        );
        assert_eq!(bytes, b"final output\n");
    }
    #[tokio::test]
    async fn stop_interrupts_waiting_child() {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (sender, mut receiver) = watch::channel(false);
        sender.send(true).unwrap();
        // `stop_tree` sends this signal; the wait only watches for the exit it causes.
        unsafe {
            libc::kill(child.id().unwrap() as i32, libc::SIGTERM);
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_cancellable(&mut child, &mut receiver),
        )
        .await;
        assert!(
            result.is_ok(),
            "cancellation must not wait for natural process exit"
        );
        assert!(!result.unwrap().unwrap().success());
    }
    #[test]
    fn argv_keeps_model_workspace_and_prompt_separate() {
        let args = command_args(
            &AgentProviderId::Codex,
            "/tmp/a b",
            "--dangerously-bypass-approvals-and-sandbox",
            Some("gpt-model"),
        );
        assert_eq!(
            args.last().unwrap(),
            "--dangerously-bypass-approvals-and-sandbox"
        );
        assert_eq!(args[args.len() - 2], "--");
        assert!(args.windows(2).any(|a| a == ["--model", "gpt-model"]));
        assert!(args
            .windows(2)
            .any(|a| a == ["--sandbox", "workspace-write"]));
    }
    #[test]
    fn opencode_child_sharing_policy_preserves_inline_settings() {
        let policy = opencode_private_config(Some(
            r#"{"model":"local/model","share":"auto","permission":{"read":"allow"}}"#,
        ))
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&policy).unwrap();
        assert_eq!(parsed["share"], "disabled");
        assert_eq!(parsed["model"], "local/model");
        assert_eq!(parsed["permission"]["read"], "allow");
        assert!(opencode_private_config(Some("not JSON")).is_err());
    }
    #[test]
    fn codex_supports_validated_projects_without_git() {
        assert!(
            command_args(&AgentProviderId::Codex, "/tmp/plain", "hello", None)
                .contains(&"--skip-git-repo-check".to_string())
        );
    }

    #[test]
    fn cursor_uses_classifier_and_sandbox_without_force_approval() {
        let args = command_args(&AgentProviderId::Cursor, "/tmp/workspace", "hello", None);
        assert!(args.contains(&"--auto-review".to_string()));
        assert!(args.windows(2).any(|pair| pair == ["--sandbox", "enabled"]));
        assert!(!args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--force" | "--yolo" | "--approve-mcps")));
    }

    // Explicit opt-in: these exercise real authenticated CLIs, never the user's repositories.
    enum SmokeMode {
        Edit,
        Analysis,
    }
    async fn live_workspace_smoke(provider: AgentProviderId, mode: SmokeMode) {
        let repo = crate::git::tests::Repo::new();
        let tree = crate::worktree::create_isolated(
            &repo.cwd(),
            &repo.0.join("workspaces"),
            "smoke001",
            "Live provider smoke",
            "sirus/{session-name}-{id}",
        )
        .unwrap();
        let edit_prompt = "This is a disposable Sirus integration test. Create exactly one file named sirus-smoke.txt in the current workspace containing SIRUS_OK followed by a newline. Do not modify any other file, use network, install dependencies, or read authentication/configuration files. Then respond with SIRUS_OK.";
        let (prompt, expected, seconds) = match mode {
            SmokeMode::Edit => (edit_prompt, "SIRUS_OK", 120),
            SmokeMode::Analysis => (
                "This is a disposable Sirus read-only integration test. Reply only SIRUS_ANALYSIS_OK. Do not read or write files, execute tools, use network tools, or inspect authentication/configuration.",
                "SIRUS_ANALYSIS_OK",
                45,
            ),
        };
        let model = std::env::var("SIRUS_SMOKE_MODEL").ok();
        let (process, mut started) = start(
            provider,
            tree.path.clone(),
            prompt.into(),
            model,
            HashMap::new(),
            Default::default(),
        )
        .await
        .unwrap();
        let out = started.child.stdout.take().unwrap();
        let err = started.child.stderr.take().unwrap();
        let output = tokio::spawn(async move {
            let mut parser = OutputParser::default();
            let mut lines = BufReader::new(out).lines();
            let mut visible = String::new();
            while let Some(line) = lines.next_line().await.unwrap() {
                visible.push_str(&parser.parse(&line));
                assert!(
                    visible.len() < 1024 * 1024,
                    "unexpectedly large smoke output"
                );
            }
            visible
        });
        let errors = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut bytes = Vec::new();
            err.take(64 * 1024).read_to_end(&mut bytes).await.unwrap();
            bytes.len()
        });
        let status = match tokio::time::timeout(
            std::time::Duration::from_secs(seconds),
            wait_cancellable(&mut started.child, &mut started.cancel),
        )
        .await
        {
            Ok(status) => status.unwrap(),
            Err(_) => {
                process.stop().unwrap();
                started.child.wait().await.unwrap();
                output.abort();
                errors.abort();
                panic!("live provider timed out; owned process group stopped");
            }
        };
        let visible = tokio::time::timeout(std::time::Duration::from_secs(3), output)
            .await
            .unwrap()
            .unwrap();
        let stderr_bytes = tokio::time::timeout(std::time::Duration::from_secs(3), errors)
            .await
            .unwrap()
            .unwrap();
        assert!(
            status.success(),
            "provider failed with code {:?}, stderr bytes {stderr_bytes}; raw vendor diagnostics deliberately omitted",
            status.code()
        );
        assert!(
            visible.contains(expected),
            "normalized assistant output did not contain expected response"
        );
        let status = crate::git::status(std::path::Path::new(&tree.path)).unwrap();
        match mode {
            SmokeMode::Edit => {
                assert_eq!(
                    std::fs::read_to_string(
                        std::path::Path::new(&tree.path).join("sirus-smoke.txt")
                    )
                    .unwrap(),
                    "SIRUS_OK\n"
                );
                assert!(status
                    .changes
                    .iter()
                    .any(|file| file.path == "sirus-smoke.txt"));
            }
            SmokeMode::Analysis => assert!(
                !status.dirty,
                "read-only analysis must preserve the workspace"
            ),
        }
        assert!(
            !repo.cwd().join("sirus-smoke.txt").exists(),
            "isolated workspace must not edit checkout"
        );
    }

    #[tokio::test]
    #[ignore = "real provider; explicitly run with existing authentication"]
    async fn live_codex_workspace() {
        live_workspace_smoke(AgentProviderId::Codex, SmokeMode::Edit).await;
    }

    #[tokio::test]
    #[ignore = "real provider; explicitly run with existing authentication"]
    async fn live_cursor_analysis() {
        live_workspace_smoke(AgentProviderId::Cursor, SmokeMode::Analysis).await;
    }

    #[tokio::test]
    #[ignore = "real provider; explicitly run with existing authentication"]
    async fn live_cursor_workspace() {
        live_workspace_smoke(AgentProviderId::Cursor, SmokeMode::Edit).await;
    }

    #[tokio::test]
    #[ignore = "real provider; explicitly run with existing authentication"]
    async fn live_opencode_workspace() {
        live_workspace_smoke(AgentProviderId::OpenCode, SmokeMode::Edit).await;
    }

    #[tokio::test]
    #[ignore = "real provider; explicitly run with existing authentication"]
    async fn live_claude_workspace() {
        live_workspace_smoke(AgentProviderId::Claude, SmokeMode::Edit).await;
    }

    #[test]
    fn opencode_does_not_auto_approve_permissions() {
        assert!(
            !command_args(&AgentProviderId::OpenCode, "/tmp", "hello", None)
                .contains(&"--auto".to_string())
        );
    }
    #[test]
    fn followup_includes_bounded_history_in_chronological_order() {
        let message = |role, content: &str| Message {
            id: "message".into(),
            session_id: "session".into(),
            role,
            content: content.into(),
            created_at: String::new(),
            streaming: false,
            activity: None,
            steers: Vec::new(),
            attachments: Vec::new(),
            launched: vec![],
            visuals: Vec::new(),
            client_request_id: None,
            rotation: None,
        };
        let messages = vec![
            message(MessageRole::User, "First request"),
            message(MessageRole::Agent, "First answer"),
        ];
        let text = conversation_prompt(&messages, "Follow up");
        assert!(text.find("First request").unwrap() < text.find("First answer").unwrap());
        assert!(text.ends_with("Current user request:\nFollow up"));
        assert_eq!(conversation_prompt(&[], "hello"), "hello");
        assert_eq!(
            conversation_prompt(
                &[message(MessageRole::Agent, &"x".repeat(40 * 1024))],
                "hello"
            ),
            "hello"
        );
        // The history is capped at 16k tokens (bytes), newest first, and shrinks with the budget.
        let many: Vec<Message> = (0..8)
            .map(|index| message(MessageRole::User, &format!("{index}{}", "y".repeat(3_000))))
            .collect();
        let capped = conversation_prompt(&many, "go");
        assert!(
            capped.len() < 16_000 + 200 && capped.contains("7yyy") && !capped.contains("\"0yyy")
        );
        assert_eq!(conversation_prompt_within(&many, "go", 0), "go");
        assert!(
            conversation_prompt_within(&many, "go", 7_000)
                .matches("yyy")
                .count()
                < capped.matches("yyy").count()
        );
    }
}
