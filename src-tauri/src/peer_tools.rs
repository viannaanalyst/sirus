//! Agents across computers (ADR-208). `route` is the sending side: a `sirus_*` tool call that
//! names a paired `computer` goes to that host over `peers::forward` instead of running here.
//! `peer_tool` is the receiving side, one host command a paired device may call: it runs the
//! same session tools the local bridge offers, but for a caller that lives on another computer
//! (so there is no caller session, no parent link and no wake-up message; the sender polls).
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::commands::{native_task, AppState};
use crate::error::{Error, Result};
use crate::models::{ApprovalMode, Session};
use crate::runtime::{AppHandle, Manager, State};

/// The tools that may name a `computer`.
pub const REMOTE_TOOLS: [&str; 9] = [
    "sirus_models",
    "sirus_delegate",
    "sirus_delegation_status",
    "sirus_delegation_cancel",
    "sirus_list_sessions",
    "sirus_read_session",
    "sirus_session_status",
    "sirus_create_session",
    "sirus_send_to_session",
];

/// Those that start work or change a session: refused for a read-only (planning) caller.
fn changes_something(tool: &str) -> bool {
    matches!(
        tool,
        "sirus_delegate"
            | "sirus_delegation_cancel"
            | "sirus_create_session"
            | "sirus_send_to_session"
    )
}

fn starts_session(tool: &str) -> bool {
    matches!(tool, "sirus_delegate" | "sirus_create_session")
}

/// The `computer` property added to each tool that accepts it.
pub fn computer_property() -> Value {
    json!({ "type": "string", "description": "Optional: the name or id of another paired computer (see sirus_list_computers) to run this on instead of this one. Sessions there are ordinary visible sessions on that computer; session and task ids belong to it, so pass the same computer again to read, message or cancel them. There the project is looked up by name or id (required), the work gets an isolated worktree by default, delegation is always async (no wake-up: poll sirus_delegation_status or sirus_session_status) and your approval mode is the ceiling." })
}

/// Adds `computer` to the schema of every tool that accepts it.
pub fn with_computer(tools: &mut [Value]) {
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or_default().to_owned();
        if !REMOTE_TOOLS.contains(&name.as_str()) {
            continue;
        }
        if let Some(properties) = tool["inputSchema"]["properties"].as_object_mut() {
            properties.insert("computer".into(), computer_property());
        }
    }
}

/// What the other host is asked: the tool, its arguments and who asks.
fn request_for(caller: &Session, tool: &str, args: &Value) -> Value {
    let mut args = args.clone();
    if let Some(object) = args.as_object_mut() {
        object.remove("computer");
    }
    json!({
        "tool": tool,
        "args": args,
        "approval": crate::launch::inherited_approval(caller),
        "provider": caller.agent.key(),
        "model": caller.model,
        "from": crate::launch::bounded(&caller.title, 80),
        "fromComputer": crate::commands::machine_host_name(),
    })
}

/// Sending side: runs `tool` on the paired computer `wanted` for `caller`.
pub fn route(
    app: &AppHandle,
    caller: &Session,
    tool: &str,
    args: &Value,
    wanted: &str,
) -> std::result::Result<Value, String> {
    if !REMOTE_TOOLS.contains(&tool) {
        return Err("that tool cannot run on another computer".into());
    }
    if changes_something(tool) && (caller.execution.planning || caller.pull_ask.is_some()) {
        return Err(
            "planning is read-only: nothing can be started or changed on another computer".into(),
        );
    }
    let state = app.state::<Arc<AppState>>().inner().clone();
    let dir = state
        .data_path
        .parent()
        .ok_or("the data folder is unknown")?
        .to_path_buf();
    // Name checked before a start is counted against the turn.
    crate::peers::resolve(&crate::peers::load(&dir), wanted)?;
    if starts_session(tool) {
        crate::sirus_tools::reserve_start(&caller.id, &crate::sirus_tools::turn(caller))?;
    }
    crate::peers::forward(&dir, wanted, request_for(caller, tool, args))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerRequest {
    tool: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    approval: Option<ApprovalMode>,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    from: String,
    #[serde(default)]
    from_computer: String,
}

/// A stand-in for the caller, which lives on the other computer: it only carries the title,
/// provider and approval the shared helpers read.
fn stand_in(request: &PeerRequest) -> Result<Session> {
    let provider = if request.provider.is_empty() {
        "claude"
    } else {
        &request.provider
    };
    let mut session: Session = serde_json::from_value(json!({
        "id": "peer-caller",
        "title": request.from,
        "projectId": "",
        "agent": provider,
        "status": "idle",
        "createdAt": "",
        "lastActivityAt": "",
        "worktree": { "path": "", "branch": "", "isolated": false },
        "lastError": null,
    }))
    .map_err(|_| Error::new("invalid", "unknown provider on the calling computer"))?;
    session.model = request.model.clone();
    session.execution.approval = request.approval;
    Ok(session)
}

/// A first prompt or message that says where it came from, so the agent does not take another
/// computer's agent for the person.
fn headed(request: &PeerRequest, prompt: &str, portuguese: bool) -> String {
    let title = crate::launch::bounded(request.from.trim(), 80);
    let place = crate::launch::bounded(request.from_computer.trim(), 48);
    if portuguese {
        format!("↪ Tarefa da sessão “{title}” no computador {place}, não da pessoa:\n\n{prompt}")
    } else {
        format!("↪ Task from the session “{title}” on the computer {place}, not from the person:\n\n{prompt}")
    }
}

fn text(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned()
}

const TEXT_CHARS: usize = 32 * 1024;

/// Receiving side. Needs "Let agents manage sessions" on this computer, like any agent here.
pub async fn peer_tool(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    request: PeerRequest,
) -> Result<Value> {
    let state = state.inner().clone();
    if !state.data.lock().settings.agents_manage_sessions {
        return Err(Error::new(
            "denied",
            "the person has not allowed agents to manage sessions on this computer (Settings → MCP servers)",
        ));
    }
    if !REMOTE_TOOLS.contains(&request.tool.as_str()) {
        return Err(Error::new(
            "invalid",
            "that tool cannot run here for another computer",
        ));
    }
    let args = request.args.clone();
    let tool = request.tool.clone();
    let caller = stand_in(&request)?;
    let invalid = |message: &str| Error::new("invalid", message.to_owned());
    match tool.as_str() {
        "sirus_list_sessions" => {
            let filter = text(&args, "project");
            native_task(move || Ok(crate::sirus_tools::list(&state.data.lock(), "", &filter))).await
        }
        "sirus_models" => native_task(move || Ok(crate::delegation::models(&state, &caller))).await,
        "sirus_session_status" | "sirus_delegation_status" | "sirus_read_session" => {
            let id = text(
                &args,
                if tool == "sirus_delegation_status" {
                    "taskId"
                } else {
                    "sessionId"
                },
            );
            native_task(move || {
                let data = state.data_with_messages(&[&id]);
                let session =
                    crate::sirus_tools::find(&data, &id).map_err(Error::not_found)?;
                Ok(match tool.as_str() {
                    "sirus_session_status" => crate::sirus_tools::status(&data, session),
                    "sirus_delegation_status" => crate::delegation::task_view(session),
                    _ => crate::sirus_tools::read(
                        &data,
                        session,
                        crate::sirus_tools::ReadPage::from_args(&args),
                    ),
                })
            })
            .await
        }
        "sirus_delegation_cancel" => {
            let id = text(&args, "taskId");
            crate::sirus_tools::find(&state.data.lock(), &id).map_err(Error::not_found)?;
            crate::commands::stop_one(&app, &state, &id)?;
            Ok(json!({ "taskId": id, "stopped": true }))
        }
        "sirus_send_to_session" => {
            let id = text(&args, "sessionId");
            let (prompt, portuguese) = {
                let data = state.data.lock();
                let target =
                    crate::sirus_tools::find(&data, &id).map_err(Error::not_found)?;
                if target.status.is_active() {
                    return Err(invalid("that session is still working"));
                }
                (text(&args, "prompt"), data.settings.locale == "pt-BR")
            };
            if prompt.is_empty() || prompt.chars().count() > TEXT_CHARS {
                return Err(invalid("a prompt of up to 32,000 characters is required"));
            }
            // `send_from` heads the message with the stand-in's title and provider.
            crate::runtime::async_runtime::spawn_blocking(move || {
                let _ = portuguese;
                crate::launch::send_from(&app, &caller, &id, prompt)
            })
            .await
            .map_err(|_| Error::new("native", "the send could not complete"))?
            .map_err(|e| Error::new("invalid", e))
        }
        _ => {
            // sirus_create_session / sirus_delegate: a visible session in a project here.
            let delegating = tool == "sirus_delegate";
            let prompt = text(&args, if delegating { "task" } else { "prompt" });
            if prompt.is_empty() || prompt.chars().count() > TEXT_CHARS {
                return Err(invalid("a prompt of up to 32,000 characters is required"));
            }
            let (project_id, agent, portuguese) = {
                let data = state.data.lock();
                let project = crate::sirus_tools::resolve_project(&data, &text(&args, "project"))
                    .map_err(|_| invalid("a project name or id on this computer is required (sirus_list_sessions lists them)"))?;
                let wanted = match text(&args, "provider") {
                    provider if !provider.is_empty() => provider,
                    _ => caller.agent.key().to_owned(),
                };
                let agent: crate::models::AgentProviderId =
                    serde_json::from_value(json!(wanted))
                        .map_err(|_| invalid("unknown provider; see sirus_models"))?;
                if !data.provider_enabled(&agent) {
                    return Err(invalid(
                        "that provider is turned off here; see sirus_models",
                    ));
                }
                (project, agent, data.settings.locale == "pt-BR")
            };
            let model = match text(&args, "model") {
                model if !model.is_empty() => Some(model),
                _ if agent == caller.agent => caller.model.clone(),
                _ => None,
            };
            let title = match text(&args, "title") {
                title if !title.is_empty() => title,
                _ => prompt
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or_default()
                    .chars()
                    .take(60)
                    .collect(),
            };
            let launch = crate::launch::Launch {
                project_id,
                title,
                agent,
                model,
                effort: None,
                isolated_worktree: args
                    .get("newWorktree")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                fast: false,
                prompt: headed(&request, &prompt, portuguese),
                approval: crate::launch::inherited_approval(&caller),
                planning: false,
                delegated_by: None,
                share_worktree: None,
            };
            match crate::launch::launch(&app, &state, launch).await {
                (Some(id), error) => {
                    // One more level at most from here, so work does not bounce between computers.
                    crate::sirus_tools::record_depth(&id, 1);
                    match error {
                        None => Ok(json!({ "started": true, "sessionId": id, "taskId": id })),
                        Some(error) => Err(Error::agent(error)),
                    }
                }
                (None, error) => Err(Error::agent(
                    error.unwrap_or_else(|| "the session did not start".into()),
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(extra: Value) -> Session {
        let mut value = json!({
            "id": "c", "title": "Run the suite", "projectId": "p", "agent": "codex", "status": "running",
            "createdAt": "t", "lastActivityAt": "t", "model": "gpt-x",
            "worktree": {"path": "/f", "branch": "main", "isolated": false}, "lastError": null
        });
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn the_forwarded_request_carries_the_callers_approval_and_drops_computer() {
        let session = caller(json!({ "execution": { "approval": "full" } }));
        let request = request_for(
            &session,
            "sirus_delegate",
            &json!({ "task": "t", "computer": "vps" }),
        );
        assert_eq!(request["args"], json!({ "task": "t" }));
        assert_eq!(request["approval"], json!("full"));
        assert_eq!(request["provider"], json!("codex"));
        assert_eq!(request["from"], json!("Run the suite"));
        assert!(serde_json::from_value::<PeerRequest>(request).is_ok());
    }

    #[test]
    fn a_session_without_a_choice_asks_on_the_other_computer_too() {
        let request = request_for(&caller(json!({})), "sirus_create_session", &json!({}));
        assert_eq!(request["approval"], json!("ask"));
    }

    #[test]
    fn every_tool_that_takes_a_computer_says_so_and_others_do_not() {
        let mut tools = vec![
            json!({ "name": "sirus_delegate", "inputSchema": { "type": "object", "properties": {} } }),
            json!({ "name": "visual_show", "inputSchema": { "type": "object", "properties": {} } }),
        ];
        with_computer(&mut tools);
        assert!(tools[0]["inputSchema"]["properties"]["computer"].is_object());
        assert!(tools[1]["inputSchema"]["properties"]["computer"].is_null());
    }

    #[test]
    fn a_stand_in_caller_reads_the_senders_provider_and_ceiling() {
        let request: PeerRequest = serde_json::from_value(json!({
            "tool": "sirus_create_session", "approval": "ask", "provider": "claude", "from": "x"
        }))
        .unwrap();
        let session = stand_in(&request).unwrap();
        assert_eq!(session.agent.key(), "claude");
        assert_eq!(
            crate::launch::inherited_approval(&session),
            ApprovalMode::Ask
        );
        let bad: PeerRequest =
            serde_json::from_value(json!({ "tool": "t", "provider": "nope" })).unwrap();
        assert!(stand_in(&bad).is_err());
    }

    #[test]
    fn unknown_fields_and_tools_are_refused() {
        assert!(serde_json::from_value::<PeerRequest>(json!({ "tool": "t", "extra": 1 })).is_err());
        assert!(!REMOTE_TOOLS.contains(&"browser_open"));
        assert!(!REMOTE_TOOLS.contains(&"sirus_answer_approval"));
    }

    #[test]
    fn a_task_says_where_it_came_from() {
        let request: PeerRequest = serde_json::from_value(json!({
            "tool": "sirus_delegate", "from": "Run the suite", "fromComputer": "mac"
        }))
        .unwrap();
        assert!(headed(&request, "Do it.", false).starts_with(
            "↪ Task from the session “Run the suite” on the computer mac, not from the person:"
        ));
    }
}
