//! Sessions started on someone's behalf (ADR-076, ADR-111): delegation and the `sirus_*`
//! tools create a normal, visible session and admit its first turn with the caller's
//! approval, stepped down to what the target provider offers. Which caller turn started
//! which target turn is remembered so Stop on the caller stops what it started.

use std::sync::Arc;

use crate::runtime::{AppHandle, Manager};
use serde_json::{json, Value};

use crate::commands::{native_task, AppState};
use crate::models::{
    AgentProviderId, AppData, ApprovalMode, CreateSessionRequest, ExecutionOptions, MessageRole,
    SendPromptRequest,
};

pub(crate) fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 200
        && !model.starts_with('-')
        && model.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'@' | b'+' | b'-')
        })
}

fn short(value: &str) -> String {
    value
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect()
}

/// One start on someone's behalf: a new session admitted through `send_prompt`.
pub struct Launch {
    pub project_id: String,
    pub title: String,
    pub agent: AgentProviderId,
    pub model: Option<String>,
    pub isolated_worktree: bool,
    pub prompt: String,
    pub approval: ApprovalMode,
    pub planning: bool,
    /// The session that delegated it (ADR-111).
    pub delegated_by: Option<crate::delegation::DelegatedBy>,
    /// Shares this folder instead of the project checkout or a new worktree.
    pub share_worktree: Option<crate::models::Worktree>,
}

/// Creates the session (announced to the renderer) and admits its first turn.
/// Returns the session ID (when created) and a short error (when anything failed).
pub async fn launch(
    app: &AppHandle,
    state: &Arc<AppState>,
    request: Launch,
) -> (Option<String>, Option<String>) {
    // The target may not offer the inherited mode: step down, never up.
    let approval = match crate::execution::approval_for(&request.agent, request.approval) {
        Ok(approval) => approval,
        Err(error) => return (None, Some(short(&error.to_string()))),
    };
    let created = {
        let state = state.clone();
        let delegated_by = request.delegated_by.clone();
        let share_worktree = request.share_worktree.clone();
        let session_request = CreateSessionRequest {
            project_id: request.project_id.clone(),
            title: Some(request.title.chars().take(200).collect()),
            agent: request.agent.clone(),
            isolated_worktree: request.isolated_worktree,
            model: request.model.clone(),
            share_worktree_of: None,
        };
        native_task(move || {
            let start = if share_worktree.is_some() {
                "HEAD".to_string()
            } else {
                crate::commands::new_worktree_start(&state, &session_request)
            };
            let mut data = state.data.lock();
            let mut session =
                crate::commands::create_session_locked(&state, &mut data, session_request, &start)?;
            session.delegated_by = delegated_by;
            if let Some(tree) = share_worktree {
                session.worktree = tree;
            }
            if let Some(stored) = data.sessions.iter_mut().find(|item| item.id == session.id) {
                stored.delegated_by = session.delegated_by.clone();
                stored.worktree = session.worktree.clone();
            }
            drop(data);
            state.persist()?;
            Ok(session)
        })
        .await
    };
    match created {
        Ok(session) => {
            crate::transcript_view::emit(app, &session);
            let send = SendPromptRequest {
                queued_after: None,
                debugging: false,
                goal: None,
                session_id: session.id.clone(),
                attachment_ids: vec![],
                attachment_owner: String::new(),
                prompt: request.prompt,
                execution: ExecutionOptions {
                    approval,
                    planning: request.planning,
                    ..Default::default()
                },
                team: false,
                client_request_id: None,
            };
            let sent =
                crate::commands::send_prompt(app.clone(), app.state::<Arc<AppState>>(), send).await;
            (
                Some(session.id),
                sent.err().map(|error| short(&error.to_string())),
            )
        }
        Err(error) => (None, Some(short(&error.to_string()))),
    }
}

/// A session started on behalf of another one (`sirus_*` tools, delegation).
pub(crate) struct StartRequest {
    pub project_id: String,
    pub title: String,
    pub prompt: String,
    /// Empty: the caller's provider.
    pub provider: String,
    /// Empty: the caller's model when the provider is the same.
    pub model: String,
    pub new_worktree: bool,
    pub delegated_by: Option<crate::delegation::DelegatedBy>,
    /// Work in this existing folder instead of a new one (`sirus_delegate`, ADR-111).
    pub share_worktree: Option<crate::models::Worktree>,
}

/// The caller's own permission choice carries over to the work it delegates.
pub(crate) fn inherited_approval(caller: &crate::models::Session) -> crate::models::ApprovalMode {
    caller
        .execution
        .approval
        .unwrap_or(crate::models::ApprovalMode::Ask)
}

/// Starts a normal, visible session whose first turn inherits the caller's approval.
pub(crate) fn start_from(
    app: &AppHandle,
    caller: &crate::models::Session,
    request: StartRequest,
) -> std::result::Result<String, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let agent = if request.provider.is_empty() {
        caller.agent.clone()
    } else {
        serde_json::from_value::<crate::models::AgentProviderId>(json!(request.provider))
            .map_err(|_| "unknown provider")?
    };
    let model = match request.model {
        model if !model.is_empty() => Some(model),
        _ if agent == caller.agent => caller.model.clone(),
        _ => None,
    };
    let start = Launch {
        project_id: request.project_id,
        title: request.title,
        agent,
        model,
        isolated_worktree: request.new_worktree && request.share_worktree.is_none(),
        prompt: request.prompt,
        approval: inherited_approval(caller),
        planning: false,
        delegated_by: request.delegated_by,
        share_worktree: request.share_worktree,
    };
    match crate::runtime::async_runtime::block_on(launch(app, &state, start)) {
        (Some(id), error) => {
            record_launch(app, &state, &caller.id, &id);
            match error {
                None => Ok(id),
                Some(error) => Err(error),
            }
        }
        (None, Some(error)) => Err(error),
        _ => Err("the session did not start".into()),
    }
}

/// A turn another session runs because a caller's turn started or messaged it (ADR-101).
struct LaunchRecord {
    caller: String,
    caller_turn: String,
    target: String,
    target_turn: String,
}
/// Memory only: processes do not outlive the app. Bounded, oldest dropped.
static LAUNCHES: parking_lot::Mutex<Vec<LaunchRecord>> = parking_lot::Mutex::new(Vec::new());
const MAX_LAUNCHES: usize = 512;

/// A session's current turn: its latest user message.
fn current_turn(session: &crate::models::Session) -> Option<&str> {
    session
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .map(|message| message.id.as_str())
}

/// Remembers that the caller's current turn started the target's current one, so a Stop on the
/// caller stops it too (delegation and the `sirus_*` tools).
pub(crate) fn remember_launch(data: &AppData, caller_id: &str, target_id: &str) {
    let turn = |id: &str| {
        data.sessions
            .iter()
            .find(|session| session.id == id)
            .and_then(current_turn)
            .map(str::to_owned)
    };
    let (Some(caller_turn), Some(target_turn)) = (turn(caller_id), turn(target_id)) else {
        return;
    };
    let mut launches = LAUNCHES.lock();
    launches.retain(|launch| launch.target != target_id);
    if launches.len() >= MAX_LAUNCHES {
        launches.remove(0);
    }
    launches.push(LaunchRecord {
        caller: caller_id.to_owned(),
        caller_turn,
        target: target_id.to_owned(),
        target_turn,
    });
}

/// Sessions the caller ever launched (to load their transcripts before `launched_running`).
pub(crate) fn launched_by_session(caller_id: &str) -> Vec<String> {
    LAUNCHES
        .lock()
        .iter()
        .filter(|launch| launch.caller == caller_id)
        .map(|launch| launch.target.clone())
        .collect()
}

/// Sessions still running the turn the caller's current turn launched (ADR-101).
pub(crate) fn launched_running(data: &AppData, caller_id: &str) -> Vec<String> {
    let Some(caller_turn) = data
        .sessions
        .iter()
        .find(|session| session.id == caller_id)
        .and_then(current_turn)
    else {
        return vec![];
    };
    LAUNCHES
        .lock()
        .iter()
        .filter(|launch| launch.caller == caller_id && launch.caller_turn == caller_turn)
        .filter(|launch| {
            data.sessions.iter().any(|session| {
                session.id == launch.target
                    && session.status.is_active()
                    && current_turn(session) == Some(launch.target_turn.as_str())
            })
        })
        .map(|launch| launch.target.clone())
        .collect()
}

/// Lists a started session on the caller's running reply (its Sessions control, ADR-088).
fn record_launch(app: &AppHandle, state: &AppState, caller_id: &str, launched: &str) {
    let snapshot = {
        let mut data = state.data.lock();
        remember_launch(&data, caller_id, launched);
        let Some(caller) = data.sessions.iter_mut().find(|item| item.id == caller_id) else {
            return;
        };
        let Some(index) = caller
            .messages
            .iter()
            .rposition(|message| message.role == MessageRole::Agent)
        else {
            return;
        };
        let reply = &mut caller.messages[index];
        if reply.launched.iter().any(|id| id == launched) || reply.launched.len() >= 50 {
            return;
        }
        reply.launched.push(launched.to_string());
        (caller.clone(), index)
    };
    let _ = state.persist();
    crate::transcript_view::emit_from(app, &snapshot.0, snapshot.1);
}

/// A message one session sends another, headed with where it came from so the target does
/// not take another agent's request for the person's (provenance labels).
fn relayed(caller: &crate::models::Session, prompt: &str, portuguese: bool) -> String {
    let title = short(&caller.title);
    let agent = caller.agent.key();
    if portuguese {
        format!("↪ Mensagem da sessão “{title}” ({agent}), não da pessoa:\n\n{prompt}")
    } else {
        format!("↪ Message from the session “{title}” ({agent}), not from the person:\n\n{prompt}")
    }
}

/// Sends a follow-up to an idle session with the caller's approval.
pub(crate) fn send_from(
    app: &AppHandle,
    caller: &crate::models::Session,
    target: &str,
    prompt: String,
) -> std::result::Result<Value, String> {
    if prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    let state = app.state::<Arc<AppState>>();
    let portuguese = state.data.lock().settings.locale == "pt-BR";
    let prompt = relayed(caller, &prompt, portuguese);
    let request = crate::models::SendPromptRequest {
        queued_after: None,
        debugging: false,
        goal: None,
        session_id: target.to_string(),
        attachment_ids: vec![],
        attachment_owner: String::new(),
        prompt,
        execution: crate::models::ExecutionOptions {
            approval: Some(inherited_approval(caller)),
            ..Default::default()
        },
        team: false,
        client_request_id: None,
    };
    crate::runtime::async_runtime::block_on(crate::commands::send_prompt(
        app.clone(),
        state.clone(),
        request,
    ))
    .map(|_| {
        remember_launch(&state.data.lock(), &caller.id, target);
        json!({ "sent": true, "sessionId": target })
    })
    .map_err(|error| error.to_string())
}

pub(crate) fn session_row(data: &AppData, session: &crate::models::Session) -> Value {
    json!({
        "id": session.id,
        "title": session.title,
        "project": data.projects.iter().find(|project| project.id == session.project_id).map(|project| project.name.clone()),
        "provider": session.agent.key(),
        "model": session.model,
        "status": session.status,
        "branch": session.worktree.branch,
        "lastActivityAt": session.last_activity_at,
    })
}

pub(crate) fn bounded(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relayed_message_names_the_session_it_came_from() {
        let caller: crate::models::Session = serde_json::from_value(json!({
            "id":"a","title":"Fix login","projectId":"p","agent":"codex","status":"idle","createdAt":"t","lastActivityAt":"t",
            "worktree":{"path":"/f","branch":"main","isolated":false},"lastError":null
        }))
        .unwrap();
        assert_eq!(
            relayed(&caller, "Run the tests.", false),
            "↪ Message from the session “Fix login” (codex), not from the person:\n\nRun the tests."
        );
        assert!(relayed(&caller, "Rode os testes.", true)
            .starts_with("↪ Mensagem da sessão “Fix login” (codex)"));
    }
}
