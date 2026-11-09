//! Agents manage sessions (ADR-076): the `sirus_*` tools on the per-session MCP
//! bridge (ADR-030) let any session list, read, start and continue sessions.
//! They are listed only while Settings → "Let agents manage sessions" is on and
//! every call checks the setting again. Starting and messaging reuse the shared
//! delegation path (`launch.rs`): new work inherits the caller's approval mode, a
//! planning (read-only) caller cannot start or message anything, and fan-out
//! and nesting are bounded.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::runtime::{AppHandle, Manager};
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::commands::AppState;
use crate::models::{AppData, MessageRole, Session};

/// Set in the MCP child's environment while the tools are enabled.
pub const ENV: &str = "SIRUS_SESSION_TOOLS";
const LIST_LIMIT: usize = 40;
const READ_DEFAULT: usize = 6;
const READ_LIMIT: usize = 20;
const MESSAGE_CHARS: usize = 2000;
const MIN_MESSAGE_CHARS: usize = 200;
const MAX_MESSAGE_CHARS: usize = 6000;
const PROMPT_CHARS: usize = 32 * 1024;
/// Sessions one caller turn may start, and messages it may send.
const STARTS_PER_TURN: u8 = 4;
const SENDS_PER_TURN: u8 = 8;
/// Sessions started by agents may start one more level, no deeper.
const MAX_DEPTH: u8 = 2;

static ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct Ledger {
    /// Session id → how many agent hops started it (memory only).
    depth: HashMap<String, u8>,
    /// (caller, turn) → (started, sent).
    turns: HashMap<(String, String), (u8, u8)>,
}
static LEDGER: Mutex<Option<Ledger>> = Mutex::new(None);

pub fn set_enabled(value: bool) {
    ENABLED.store(value, Ordering::SeqCst);
}

/// The variable passed to the MCP child so it lists the tools.
pub fn child_env() -> Option<(&'static str, &'static str)> {
    ENABLED.load(Ordering::SeqCst).then_some((ENV, "1"))
}

pub fn tool_definitions() -> Vec<Value> {
    definitions(std::env::var(ENV).as_deref() == Ok("1"))
}

/// As the host's HTTP route lists them: from the switch itself, not a child's environment.
pub fn live_tool_definitions() -> Vec<Value> {
    definitions(ENABLED.load(Ordering::SeqCst))
}

fn definitions(enabled: bool) -> Vec<Value> {
    // Delegating a task to another model is always on (ADR-111).
    let mut tools = crate::delegation::tool_definitions();
    // Inline visuals only touch the caller's own reply, so they are always on too (ADR-123).
    tools.push(crate::visuals::tool_definition());
    tools.extend(crate::visuals::native_tool_definitions());
    // Guided reviews only store a guide, never change code, so they are always on too (ADR-125).
    tools.extend(crate::review_guides::tool_definitions());
    if !enabled {
        return tools;
    }
    let id = || json!({ "type": "object", "properties": { "sessionId": { "type": "string" } }, "required": ["sessionId"] });
    tools.extend([
        json!({ "name": "sirus_list_sessions", "description": "Sirus: the projects and recent sessions in the app (id, title, project, provider, model, status, branch), newest first. Your own session is marked self.", "inputSchema": { "type": "object", "properties": { "project": { "type": "string", "description": "Optional project name or id." } } } }),
        json!({ "name": "sirus_read_session", "description": "Sirus: a session's status and a page of its messages (user and assistant text only), oldest first within the page. With no paging parameters you get the newest 6, each cut at 2,000 characters. To read further back, pass the previous response's nextCursor as cursor (it is null once you reach the start). A message cut at maxChars carries a marker, truncated, nextCharOffset and totalChars: read the rest by calling again with cursor set to its index + 1, limit 1 and charOffset set to nextCharOffset. earlierMessages counts the messages before the page.", "inputSchema": { "type": "object", "properties": { "sessionId": { "type": "string" }, "limit": { "type": "integer", "description": "Messages per page, 1–20 (default 6)." }, "cursor": { "type": "integer", "description": "Page ending just before this message index; use the nextCursor of the previous page. Default: the end of the transcript (newest messages)." }, "maxChars": { "type": "integer", "description": "Characters kept per message, 200–6,000 (default 2,000)." }, "charOffset": { "type": "integer", "description": "Character to start each message from, to read the rest of a message cut at maxChars (use with limit 1)." } }, "required": ["sessionId"] } }),
        json!({ "name": "sirus_session_status", "description": "Sirus: a session's status (idle, running, waiting for the person, completed, failed, stopped), title and last error.", "inputSchema": id() }),
        json!({ "name": "sirus_create_session", "description": "Sirus: start a new visible session in a project with a first prompt. It runs on its own with your approval mode (so it may wait for the person's approval in its own tab) and returns at once; check it later with sirus_session_status. At most 4 per turn.", "inputSchema": { "type": "object", "properties": { "project": { "type": "string", "description": "Project name or id." }, "prompt": { "type": "string", "description": "The task: objective, context, constraints, what it may change and the result you expect." }, "title": { "type": "string" }, "provider": { "type": "string", "description": "Optional provider id (claude, codex, opencode, …); defaults to yours." }, "model": { "type": "string" }, "newWorktree": { "type": "boolean", "description": "Default true: an isolated worktree, so parallel sessions never mix files." } }, "required": ["project", "prompt"] } }),
        json!({ "name": "sirus_session_diff", "description": "Sirus: the uncommitted changes in a session's workspace: a per-file summary and the unified diff against HEAD (new files listed by name), cut to 60,000 characters. Optional path: one file, relative to the workspace.", "inputSchema": { "type": "object", "properties": { "sessionId": { "type": "string" }, "path": { "type": "string" } }, "required": ["sessionId"] } }),
        json!({ "name": "sirus_answer_approval", "description": "Sirus: answer an approval waiting in a session you started (its requestId is in sirus_session_status's pendingApprovals). decline is always allowed; accept only when you yourself run with full access. Questions are left to the person.", "inputSchema": { "type": "object", "properties": { "sessionId": { "type": "string" }, "requestId": { "type": "string" }, "decision": { "type": "string", "enum": ["accept", "decline"] } }, "required": ["sessionId", "requestId", "decision"] } }),
        json!({ "name": "sirus_send_to_session", "description": "Sirus: continue another idle session with a message, using your approval mode. Returns at once.", "inputSchema": { "type": "object", "properties": { "sessionId": { "type": "string" }, "prompt": { "type": "string" } }, "required": ["sessionId", "prompt"] } }),
    ]);
    tools
}

/// Sessions these tools may see: not side chats.
fn visible(session: &Session) -> bool {
    !session.is_auxiliary()
}

fn project_name(data: &AppData, id: &str) -> Option<String> {
    data.projects
        .iter()
        .find(|project| project.id == id)
        .map(|project| project.name.clone())
}

fn list(data: &AppData, caller: &str, filter: &str) -> Value {
    let project = |session: &Session| {
        filter.is_empty()
            || session.project_id == filter
            || project_name(data, &session.project_id)
                .is_some_and(|name| name.eq_ignore_ascii_case(filter))
    };
    let mut sessions: Vec<&Session> = data
        .sessions
        .iter()
        .filter(|session| visible(session) && project(session))
        .collect();
    sessions.sort_by(|a, b| b.last_activity_at.cmp(&a.last_activity_at));
    let rows: Vec<Value> = sessions
        .iter()
        .take(LIST_LIMIT)
        .map(|session| {
            let mut row = crate::launch::session_row(data, session);
            if session.id == caller {
                row["self"] = json!(true);
            }
            row
        })
        .collect();
    json!({
        "projects": data.projects.iter().map(|project| json!({ "id": project.id, "name": project.name })).collect::<Vec<_>>(),
        "sessions": rows,
        "truncated": sessions.len() > LIST_LIMIT,
    })
}

fn find<'a>(data: &'a AppData, id: &str) -> Result<&'a Session, String> {
    data.sessions
        .iter()
        .find(|session| session.id == id && visible(session))
        .ok_or_else(|| "no session with that id".to_string())
}

fn status(data: &AppData, session: &Session) -> Value {
    let mut row = crate::launch::session_row(data, session);
    row["lastError"] = json!(session.last_error);
    row["waitingForPerson"] = json!(session.status == crate::models::SessionStatus::Waiting);
    row["pendingApprovals"] = json!(session
        .pending_requests
        .iter()
        .filter_map(|request| {
            use crate::models::PendingRequestKind as Kind;
            let (kind, detail, reason) = match &request.kind {
                Kind::Command {
                    command, reason, ..
                } => ("command", command.clone(), reason),
                Kind::Tool { name, reason, .. } => ("tool", name.clone(), reason),
                Kind::FileChange { changes, reason } => (
                    "fileChange",
                    changes
                        .iter()
                        .map(|change| change.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    reason,
                ),
                Kind::UserInput { .. } => return None,
            };
            Some(json!({
                "requestId": request.request_id,
                "type": kind,
                "detail": crate::launch::bounded(&detail, 500),
                "reason": reason.as_deref().map(|reason| crate::launch::bounded(reason, 500)),
            }))
        })
        .collect::<Vec<_>>());
    row
}

/// An approval the caller may answer: in a session it started itself, never a question,
/// and accepting only from a caller that already runs with full access.
fn answerable(
    caller: &Session,
    target: &Session,
    request_id: &str,
    decision: &crate::models::ApprovalDecision,
) -> Result<crate::models::RespondAgentRequest, String> {
    if target
        .delegated_by
        .as_ref()
        .is_none_or(|parent| parent.session_id != caller.id)
    {
        return Err("only approvals in sessions you started can be answered".into());
    }
    if matches!(decision, crate::models::ApprovalDecision::Accept)
        && caller.execution.approval != Some(crate::models::ApprovalMode::Full)
    {
        return Err(
            "accepting needs full access in your own session; decline, or leave it to the person"
                .into(),
        );
    }
    let request = target
        .pending_requests
        .iter()
        .find(|request| request.request_id == request_id)
        .ok_or("no such pending approval")?;
    if matches!(
        request.kind,
        crate::models::PendingRequestKind::UserInput { .. }
    ) {
        return Err("questions are answered by the person".into());
    }
    Ok(crate::models::RespondAgentRequest {
        session_id: target.id.clone(),
        generation: request.generation.clone(),
        request_id: request.request_id.clone(),
        turn_id: request.turn_id.clone(),
        response: crate::models::AgentResponse::Approval {
            decision: decision.clone(),
        },
    })
}

const DIFF_CHARS: usize = 60_000;

/// A workspace's uncommitted changes against HEAD, read-only and bounded.
fn session_diff(cwd: &std::path::Path, path: &str) -> Result<Value, String> {
    if !path.is_empty()
        && (std::path::Path::new(path).is_absolute()
            || path.split(['/', '\\']).any(|part| part == ".."))
    {
        return Err("path must be relative to the workspace".into());
    }
    let run = |args: &[&str]| -> Result<String, String> {
        let output = crate::git::bounded_run(cwd, args).map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let mut diff_args = vec!["diff", "--no-ext-diff", "--no-textconv", "HEAD"];
    let mut stat_args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--stat=200",
        "HEAD",
    ];
    let mut new_args = vec!["ls-files", "--others", "--exclude-standard"];
    if !path.is_empty() {
        for args in [&mut diff_args, &mut stat_args, &mut new_args] {
            args.extend(["--", path]);
        }
    }
    let diff = run(&diff_args)?;
    let truncated = diff.chars().count() > DIFF_CHARS;
    Ok(json!({
        "summary": run(&stat_args)?.trim(),
        "newFiles": run(&new_args)?.lines().take(200).collect::<Vec<_>>(),
        "diff": diff.chars().take(DIFF_CHARS).collect::<String>(),
        "truncated": truncated,
    }))
}

/// What `sirus_read_session` was asked for; `None` fields take their defaults.
#[derive(Default, Clone, Copy)]
struct ReadPage {
    limit: Option<usize>,
    /// Index just past the page's newest message; `None` is the end of the transcript.
    cursor: Option<usize>,
    max_chars: Option<usize>,
    char_offset: usize,
}

impl ReadPage {
    fn from_args(args: &Value) -> Self {
        let number = |key: &str| {
            args.get(key)
                .and_then(Value::as_u64)
                .map(|value| value as usize)
        };
        Self {
            limit: number("limit"),
            cursor: number("cursor"),
            max_chars: number("maxChars"),
            char_offset: number("charOffset").unwrap_or(0),
        }
    }
}

/// One message's slice of `max` characters from `offset`, with a marker when more remains.
/// Returns the text and, while more remains, the next offset and the message's length.
fn message_slice(
    text: &str,
    index: usize,
    max: usize,
    offset: usize,
) -> (String, Option<(usize, usize)>) {
    let total = text.chars().count();
    let slice: String = text.chars().skip(offset).take(max).collect();
    let next = offset.saturating_add(max);
    if total <= next {
        return (slice, None);
    }
    let marker = format!(
        "[{} more characters; read on with sirus_read_session cursor {} and limit 1, charOffset {next}]",
        total - next,
        index + 1
    );
    (format!("{slice}\n\n{marker}"), Some((next, total)))
}

fn read(data: &AppData, session: &Session, page: ReadPage) -> Value {
    let limit = page.limit.unwrap_or(READ_DEFAULT).clamp(1, READ_LIMIT);
    let max_chars = page
        .max_chars
        .unwrap_or(MESSAGE_CHARS)
        .clamp(MIN_MESSAGE_CHARS, MAX_MESSAGE_CHARS);
    let messages: Vec<&crate::models::Message> = session
        .messages
        .iter()
        .filter(|message| {
            matches!(message.role, MessageRole::User | MessageRole::Agent)
                && !message.content.trim().is_empty()
        })
        .collect();
    let end = page.cursor.unwrap_or(messages.len()).min(messages.len());
    let start = end.saturating_sub(limit);
    let mut row = status(data, session);
    row["messages"] = json!(messages[start..end]
        .iter()
        .enumerate()
        .map(|(offset, message)| {
            let index = start + offset;
            let (text, more) =
                message_slice(message.content.trim(), index, max_chars, page.char_offset);
            let mut item = json!({
                "index": index,
                "role": if message.role == MessageRole::User { "user" } else { "assistant" },
                "text": text,
                "truncated": more.is_some(),
                "streaming": message.streaming,
                "at": message.created_at,
            });
            if let Some((next, total)) = more {
                item["nextCharOffset"] = json!(next);
                item["totalChars"] = json!(total);
            }
            item
        })
        .collect::<Vec<_>>());
    row["earlierMessages"] = json!(start);
    row["nextCursor"] = if start > 0 { json!(start) } else { Value::Null };
    row
}

fn resolve_project(data: &AppData, wanted: &str) -> Result<String, String> {
    data.projects
        .iter()
        .find(|project| project.id == wanted || project.name.eq_ignore_ascii_case(wanted))
        .map(|project| project.id.clone())
        .ok_or_else(|| "no project with that name or id".to_string())
}

/// One start for the caller's turn (`sirus_delegate`); returns the new session's depth.
pub(crate) fn reserve_start(caller: &str, turn: &str) -> Result<u8, String> {
    let mut guard = LEDGER.lock();
    reserve(
        guard.get_or_insert_with(Ledger::default),
        caller,
        turn,
        true,
    )
}

pub(crate) fn record_depth(session_id: &str, depth: u8) {
    LEDGER
        .lock()
        .get_or_insert_with(Ledger::default)
        .depth
        .insert(session_id.to_owned(), depth);
}

/// The caller's current turn: its latest user message.
pub(crate) fn turn(caller: &Session) -> String {
    caller
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .map(|message| message.id.clone())
        .unwrap_or_default()
}

/// Reserves one start (or send) for the caller's turn; refuses past the bounds.
fn reserve(ledger: &mut Ledger, caller: &str, turn: &str, start: bool) -> Result<u8, String> {
    let depth = ledger.depth.get(caller).copied().unwrap_or(0);
    if start && depth >= MAX_DEPTH {
        return Err("sessions started this deep by agents cannot start more".into());
    }
    let used = ledger
        .turns
        .entry((caller.to_string(), turn.to_string()))
        .or_default();
    if start {
        if used.0 >= STARTS_PER_TURN {
            return Err(format!(
                "at most {STARTS_PER_TURN} sessions can be started per turn"
            ));
        }
        used.0 += 1;
    } else {
        if used.1 >= SENDS_PER_TURN {
            return Err(format!(
                "at most {SENDS_PER_TURN} messages can be sent per turn"
            ));
        }
        used.1 += 1;
    }
    Ok(depth + 1)
}

pub fn execute(
    app: &AppHandle,
    session_id: &str,
    tool: &str,
    args: &Value,
) -> Result<Value, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let arg = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    if crate::delegation::TOOLS.contains(&tool) {
        return crate::delegation::execute(app, session_id, tool, args);
    }
    if tool == crate::visuals::TOOL {
        return crate::visuals::execute(app, session_id, args);
    }
    if crate::visuals::NATIVE_TOOLS.contains(&tool) {
        return crate::visuals::execute_native(app, session_id, tool, args);
    }
    if crate::review_guides::TOOLS.contains(&tool) {
        return crate::review_guides::execute(app, session_id, tool, args);
    }
    let caller = {
        let data = state.data.lock();
        if !data.settings.agents_manage_sessions {
            return Err(
                "the person has not allowed agents to manage sessions (Settings → MCP servers)"
                    .into(),
            );
        }
        data.sessions
            .iter()
            .find(|session| session.id == session_id)
            .cloned()
            .ok_or("session not found")?
    };
    let mutating = matches!(
        tool,
        "sirus_create_session" | "sirus_send_to_session" | "sirus_answer_approval"
    );
    // Planning, and an Ask conversation about a pull request (ADR-050), are read-only.
    if mutating && (caller.execution.planning || caller.pull_ask.is_some()) {
        return Err("planning is read-only: sessions cannot be started or messaged".into());
    }
    match tool {
        "sirus_list_sessions" => Ok(list(&state.data.lock(), session_id, &arg("project"))),
        "sirus_session_status" => {
            let data = state.data.lock();
            Ok(status(&data, find(&data, &arg("sessionId"))?))
        }
        "sirus_read_session" => {
            let page = ReadPage::from_args(args);
            let target = arg("sessionId");
            let data = state.data_with_messages(&[&target]);
            Ok(read(&data, find(&data, &target)?, page))
        }
        "sirus_create_session" => {
            let prompt = arg("prompt");
            if prompt.is_empty() || prompt.chars().count() > PROMPT_CHARS {
                return Err("a prompt of up to 32,000 characters is required".into());
            }
            let project_id = resolve_project(&state.data.lock(), &arg("project"))?;
            let title = match arg("title") {
                title if !title.is_empty() => title,
                _ => prompt
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(60)
                    .collect(),
            };
            let depth = {
                let mut guard = LEDGER.lock();
                reserve(
                    guard.get_or_insert_with(Ledger::default),
                    session_id,
                    &turn(&caller),
                    true,
                )?
            };
            let id = crate::launch::start_from(
                app,
                &caller,
                crate::launch::StartRequest {
                    project_id,
                    title,
                    prompt,
                    provider: arg("provider"),
                    model: arg("model"),
                    new_worktree: args
                        .get("newWorktree")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    // Linked for the breadcrumb only: these sessions do not report back.
                    delegated_by: Some(crate::delegation::DelegatedBy {
                        session_id: caller.id.clone(),
                        turn: turn(&caller),
                        notify: false,
                        settled: false,
                        delivered: false,
                    }),
                    share_worktree: None,
                },
            )?;
            LEDGER
                .lock()
                .get_or_insert_with(Ledger::default)
                .depth
                .insert(id.clone(), depth);
            Ok(json!({ "started": true, "sessionId": id }))
        }
        "sirus_session_diff" => {
            let path = {
                let data = state.data.lock();
                find(&data, &arg("sessionId"))?.worktree.path.clone()
            };
            session_diff(std::path::Path::new(&path), &arg("path"))
        }
        "sirus_answer_approval" => {
            let decision = match arg("decision").as_str() {
                "accept" => crate::models::ApprovalDecision::Accept,
                "decline" => crate::models::ApprovalDecision::Decline,
                _ => return Err("decision must be accept or decline".into()),
            };
            let request = {
                let data = state.data.lock();
                let target = find(&data, &arg("sessionId"))?;
                answerable(&caller, target, &arg("requestId"), &decision)?
            };
            crate::runtime::async_runtime::block_on(crate::commands::respond_agent_request(
                app.clone(),
                app.state::<Arc<AppState>>(),
                request,
            ))
            .map_err(|error| error.to_string())?;
            Ok(json!({ "answered": true }))
        }
        "sirus_send_to_session" => {
            let prompt = arg("prompt");
            if prompt.is_empty() || prompt.chars().count() > PROMPT_CHARS {
                return Err("a prompt of up to 32,000 characters is required".into());
            }
            let target = {
                let data = state.data.lock();
                let target = find(&data, &arg("sessionId"))?;
                if target.id == session_id {
                    return Err("a session cannot message itself".into());
                }
                if target.status.is_active() {
                    return Err("that session is still working".into());
                }
                target.id.clone()
            };
            {
                let mut guard = LEDGER.lock();
                reserve(
                    guard.get_or_insert_with(Ledger::default),
                    session_id,
                    &turn(&caller),
                    false,
                )?;
            }
            crate::launch::send_from(app, &caller, &target, prompt)
        }
        _ => Err("unknown Sirus tool".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting(id: &str, extra: Value) -> Session {
        let mut value = json!({
            "id": id, "title": "Work", "projectId": "p", "agent": "claude", "status": "waiting",
            "createdAt": "t", "lastActivityAt": "t",
            "worktree": {"path": "/trees/x", "branch": "b", "isolated": false}, "lastError": null
        });
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn approvals_are_answered_only_in_started_sessions_and_accepted_only_on_full_access() {
        use crate::models::ApprovalDecision::{Accept, Decline};
        let mut target = waiting("child", json!({"delegatedBy": {"sessionId": "parent"}}));
        // Pending callbacks are never deserialized: they belong to a live process.
        target.pending_requests = serde_json::from_value(json!([
            {"requestId": "r1", "generation": "g", "turnId": "u", "itemId": "i",
             "kind": {"type": "command", "command": "rm -rf build", "cwd": null, "reason": null}},
            {"requestId": "q1", "generation": "g", "turnId": "u", "itemId": "i",
             "kind": {"type": "userInput", "questions": []}}
        ]))
        .unwrap();
        let parent = waiting("parent", json!({}));
        let full = waiting("parent", json!({"execution": {"approval": "full"}}));
        let stranger = waiting("other", json!({"execution": {"approval": "full"}}));
        answerable(&parent, &target, "r1", &Decline).unwrap();
        assert!(
            answerable(&parent, &target, "r1", &Accept).is_err(),
            "accept needs full access"
        );
        assert!(answerable(&full, &target, "r1", &Accept).is_ok());
        assert!(
            answerable(&stranger, &target, "r1", &Decline).is_err(),
            "not its session"
        );
        assert!(
            answerable(&full, &target, "q1", &Decline).is_err(),
            "questions stay with the person"
        );
        assert!(answerable(&full, &target, "missing", &Decline).is_err());
        let listed = &status(&AppData::default(), &target)["pendingApprovals"];
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["detail"], "rm -rf build");
    }

    #[test]
    fn a_diff_path_stays_inside_the_workspace() {
        let cwd = std::path::Path::new("/nonexistent");
        assert!(session_diff(cwd, "/etc/passwd")
            .unwrap_err()
            .contains("relative"));
        assert!(session_diff(cwd, "src/../../x")
            .unwrap_err()
            .contains("relative"));
    }

    fn session(id: &str, project: &str, activity: &str) -> Session {
        serde_json::from_value(json!({
            "id": id, "title": id, "projectId": project, "agent": "claude", "status": "completed",
            "createdAt": "t", "lastActivityAt": activity, "worktree": { "path": "/tmp", "branch": "main", "isolated": false },
            "messages": []
        }))
        .unwrap()
    }

    fn data() -> AppData {
        let mut data = AppData::default();
        data.projects.push(
            serde_json::from_value(
                json!({ "id": "p1", "name": "Site", "path": "/tmp/site", "addedAt": "t", "lastOpenedAt": "t" }),
            )
            .unwrap(),
        );
        data.sessions.push(session("a", "p1", "2026-01-01"));
        data.sessions.push(session("b", "p1", "2026-02-01"));
        let mut side = session("side", "p1", "2026-03-01");
        side.side_chat = serde_json::from_value(json!({ "parentSessionId": "a" })).unwrap();
        data.sessions.push(side);
        data
    }

    #[test]
    fn tools_are_listed_only_with_the_child_flag() {
        std::env::remove_var(ENV);
        // Delegation (4), inline visuals (1), native visuals (3) and review guides (2) are always on.
        assert_eq!(tool_definitions().len(), 10);
        std::env::set_var(ENV, "1");
        let tools = tool_definitions();
        std::env::remove_var(ENV);
        // Delegation (4), inline visuals (1), native visuals (3), review guides (2) and sessions (7).
        assert_eq!(tools.len(), 17);
        assert!(tools.iter().all(|tool| {
            let name = tool["name"].as_str().unwrap();
            name.starts_with("sirus_") || name.starts_with("visual_")
        }));
    }

    #[test]
    fn listing_hides_side_chats_sorts_and_marks_self() {
        let data = data();
        let listed = list(&data, "a", "");
        let ids: Vec<&str> = listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["b", "a"]);
        assert_eq!(listed["sessions"][1]["self"], true);
        assert_eq!(listed["projects"][0]["name"], "Site");
        assert_eq!(
            list(&data, "a", "site")["sessions"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(list(&data, "a", "other")["sessions"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(find(&data, "side").is_err());
        assert_eq!(resolve_project(&data, "SITE").unwrap(), "p1");
        assert!(resolve_project(&data, "nope").is_err());
    }

    #[test]
    fn reading_is_bounded() {
        let mut data = data();
        for index in 0..30 {
            data.sessions[0].messages.push(serde_json::from_value(json!({
                "id": format!("m{index}"), "sessionId": "a", "role": if index % 2 == 0 { "user" } else { "agent" },
                "content": "x".repeat(3000), "createdAt": "t", "streaming": false
            })).unwrap());
        }
        let session = data.sessions[0].clone();
        let page = |limit| ReadPage {
            limit: Some(limit),
            ..ReadPage::default()
        };
        let row = read(&data, &session, page(100));
        let messages = row["messages"].as_array().unwrap();
        assert_eq!(messages.len(), READ_LIMIT);
        assert_eq!(row["earlierMessages"], 10);
        assert_eq!(row["nextCursor"], 10);
        assert!(messages[0]["text"].as_str().unwrap().chars().count() < MESSAGE_CHARS + 200);
        assert_eq!(
            read(&data, &session, page(0))["messages"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        // No paging parameters: the newest READ_DEFAULT messages, as before.
        let row = read(&data, &session, ReadPage::default());
        let messages = row["messages"].as_array().unwrap();
        assert_eq!(messages.len(), READ_DEFAULT);
        assert_eq!(messages[READ_DEFAULT - 1]["index"], 29);
    }

    #[test]
    fn reading_pages_back_to_the_start_with_cursors() {
        let mut data = data();
        for index in 0..7 {
            data.sessions[0].messages.push(serde_json::from_value(json!({
                "id": format!("m{index}"), "sessionId": "a", "role": if index % 2 == 0 { "user" } else { "agent" },
                "content": format!("message {index}"), "createdAt": "t", "streaming": false
            })).unwrap());
        }
        let session = data.sessions[0].clone();
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let row = read(
                &data,
                &session,
                ReadPage {
                    limit: Some(3),
                    cursor,
                    ..ReadPage::default()
                },
            );
            let mut page: Vec<u64> = row["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|message| message["index"].as_u64().unwrap())
                .collect();
            assert!(page.windows(2).all(|pair| pair[0] + 1 == pair[1]));
            page.extend(seen);
            seen = page;
            match row["nextCursor"].as_u64() {
                Some(next) => cursor = Some(next as usize),
                None => break,
            }
        }
        assert_eq!(seen, (0..7).collect::<Vec<u64>>());
        // A cursor past the end is the end.
        let row = read(
            &data,
            &session,
            ReadPage {
                cursor: Some(500),
                limit: Some(1),
                ..ReadPage::default()
            },
        );
        assert_eq!(row["messages"][0]["index"], 6);
    }

    #[test]
    fn a_huge_message_is_cut_with_a_marker_and_read_on_by_offset() {
        let mut data = data();
        let long = format!("{}{}", "a".repeat(6000), "🙂".repeat(500));
        data.sessions[0].messages.push(
            serde_json::from_value(json!({
                "id": "m0", "sessionId": "a", "role": "agent",
                "content": long, "createdAt": "t", "streaming": false
            }))
            .unwrap(),
        );
        let session = data.sessions[0].clone();
        let first = read(
            &data,
            &session,
            ReadPage {
                max_chars: Some(6000),
                ..ReadPage::default()
            },
        );
        let first = &first["messages"][0];
        assert_eq!(first["truncated"], true);
        assert_eq!(first["nextCharOffset"], 6000);
        assert_eq!(first["totalChars"], 6500);
        let text = first["text"].as_str().unwrap();
        assert!(text.starts_with(&"a".repeat(6000)));
        assert!(text.contains("500 more characters") && text.contains("charOffset 6000"));
        let rest = read(
            &data,
            &session,
            ReadPage {
                limit: Some(1),
                cursor: Some(1),
                max_chars: Some(6000),
                char_offset: 6000,
            },
        );
        let rest = &rest["messages"][0];
        assert_eq!(rest["text"], "🙂".repeat(500));
        assert_eq!(rest["truncated"], false);
        assert!(rest.get("nextCharOffset").is_none());
        // Out-of-range maxChars is clamped.
        let tiny = read(
            &data,
            &session,
            ReadPage {
                max_chars: Some(1),
                ..ReadPage::default()
            },
        );
        assert_eq!(tiny["messages"][0]["nextCharOffset"], MIN_MESSAGE_CHARS);
    }

    #[test]
    fn fan_out_and_depth_are_bounded() {
        let mut ledger = Ledger::default();
        for _ in 0..STARTS_PER_TURN {
            assert_eq!(reserve(&mut ledger, "a", "t1", true).unwrap(), 1);
        }
        assert!(reserve(&mut ledger, "a", "t1", true).is_err());
        assert!(reserve(&mut ledger, "a", "t2", true).is_ok());
        for _ in 0..SENDS_PER_TURN {
            reserve(&mut ledger, "a", "t1", false).unwrap();
        }
        assert!(reserve(&mut ledger, "a", "t1", false).is_err());
        ledger.depth.insert("child".into(), 1);
        assert_eq!(reserve(&mut ledger, "child", "t", true).unwrap(), 2);
        ledger.depth.insert("grandchild".into(), MAX_DEPTH);
        assert!(reserve(&mut ledger, "grandchild", "t", true).is_err());
        // Deep sessions may still message others.
        assert!(reserve(&mut ledger, "grandchild", "t", false).is_ok());
    }
}
