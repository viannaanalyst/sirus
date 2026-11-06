//! Continuing conversations an unexpected shutdown cut mid-turn (ADR-118). A normal quit stops every running turn and saves it as stopped
//! (`AppState::shutdown`), so a session still saved as running at the next launch was cut by a
//! crash, a forced quit or a power loss. At startup those are sent one follow-up asking the agent
//! to check what already happened before repeating anything. Delegated tasks keep their own
//! path (`delegation::resume_interrupted`) and share the checks here.

use std::collections::HashSet;
use std::sync::Arc;

use crate::runtime::{AppHandle, Manager};
use chrono::{DateTime, Utc};

use crate::commands::AppState;
use crate::models::{AppData, MessageRole, Session, SessionStatus};

/// Automatic continuations in a row before a person has to step in.
pub const MAX_CONTINUATIONS: u32 = 3;

/// Sessions the last load found cut mid-turn (memory only).
static INTERRUPTED: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());
/// Sessions whose prompt is being sent by this module: that send does not reset the count.
static AUTOMATIC: parking_lot::Mutex<Option<HashSet<String>>> = parking_lot::Mutex::new(None);

/// Called by the state load with the sessions a shutdown cut mid-turn.
pub fn note_interrupted(ids: Vec<String>) {
    *INTERRUPTED.lock() = ids;
}

/// Whether a session saved as active was cut while its agent was working: not waiting on an
/// approval or a question, not asking for a secret, and its turn had not already ended.
pub fn cut_mid_turn(session: &Session) -> bool {
    if !matches!(
        session.status,
        SessionStatus::Starting | SessionStatus::Running
    ) {
        return false;
    }
    let Some(activity) = session.messages.loaded().and_then(|messages| {
        messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::Agent)
            .and_then(|message| message.activity.as_ref())
    }) else {
        return true;
    };
    if activity.ended_at.is_some() || activity.waiting_since.is_some() {
        return false;
    }
    // A secret card lives in memory only; its open tool call is what the transcript keeps.
    !activity.items.iter().any(|item| {
        item.kind == crate::activity::ItemKind::Tool
            && item.state == crate::activity::ItemState::Running
            && (item.label.contains("request_secret") || item.detail.contains("request_secret"))
    })
}

/// The checks every automatic continuation shares: the person did not stop it, it is not
/// snoozed, and it has not been continued too many times in a row.
pub fn may_continue(session: &Session, now: DateTime<Utc>) -> bool {
    session.stopped_by_user_at.is_none()
        && !session
            .snoozed_until
            .as_deref()
            .is_some_and(|until| crate::snooze::parse(until).is_none_or(|until| until > now))
        && session.auto_continuations < MAX_CONTINUATIONS
}

/// Ordinary conversations to continue: cut mid-turn, not a side chat, a delegated task or a
/// team worker (they have their own owners), and the setting is on.
fn resumable(data: &AppData, interrupted: &[String], now: DateTime<Utc>) -> Vec<String> {
    if !data.settings.continue_interrupted_sessions {
        return Vec::new();
    }
    data.sessions
        .iter()
        .filter(|session| interrupted.contains(&session.id))
        .filter(|session| {
            !session.is_auxiliary()
                && session.delegated_by.is_none()
                && session.team_worker.is_none()
        })
        .filter(|session| may_continue(session, now))
        .map(|session| session.id.clone())
        .collect()
}

/// Whether this module is sending the session's current prompt (the count is kept).
pub fn automatic(session_id: &str) -> bool {
    AUTOMATIC
        .lock()
        .as_ref()
        .is_some_and(|ids| ids.contains(session_id))
}

pub fn prompt(portuguese: bool) -> &'static str {
    if portuguese {
        "O Sirus fechou inesperadamente enquanto você trabalhava neste turno, então seus últimos passos podem não ter terminado. Antes de repetir qualquer coisa, confira o que já aconteceu (arquivos, estado do git, processos) e então continue de onde parou."
    } else {
        "Sirus closed unexpectedly while you were working on this turn, so your last steps may not have finished. Before repeating anything, check what already happened (files, git state, processes), then continue where you left off."
    }
}

/// Sends one automatic continuation, counting it against the session's limit.
pub async fn continue_session(app: &AppHandle, session_id: &str, prompt: &str) -> bool {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let execution = {
        let mut data = state.data.lock();
        let Some(session) = data
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        else {
            return false;
        };
        session.auto_continuations += 1;
        session.execution.clone()
    };
    AUTOMATIC
        .lock()
        .get_or_insert_with(HashSet::new)
        .insert(session_id.to_owned());
    let request = crate::models::SendPromptRequest {
        queued_after: None,
        debugging: false,
        goal: None,
        session_id: session_id.to_owned(),
        attachment_ids: vec![],
        attachment_owner: String::new(),
        prompt: prompt.into(),
        execution,
        team: false,
        client_request_id: None,
    };
    let result =
        crate::commands::send_prompt(app.clone(), app.state::<Arc<AppState>>(), request).await;
    if let Some(ids) = AUTOMATIC.lock().as_mut() {
        ids.remove(session_id);
    }
    if let Err(error) = &result {
        tracing::warn!(%error, "an interrupted session could not continue after a restart");
    }
    result.is_ok()
}

/// At startup: continues what the last shutdown cut (ordinary conversations here, delegated
/// tasks through their own path).
pub fn resume_interrupted(app: &AppHandle) {
    let interrupted = std::mem::take(&mut *INTERRUPTED.lock());
    crate::delegation::resume_interrupted(app, interrupted.clone());
    let app = app.clone();
    crate::runtime::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        let (ids, portuguese) = {
            let data = state.data.lock();
            (
                resumable(&data, &interrupted, Utc::now()),
                data.settings.locale == "pt-BR",
            )
        };
        for id in ids {
            continue_session(&app, &id, prompt(portuguese)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{ItemKind, ItemState};
    use serde_json::json;

    fn session(id: &str, status: SessionStatus) -> Session {
        let mut session: Session = serde_json::from_value(json!({
            "id": id, "title": "Work", "projectId": "project", "agent": "codex", "status": "idle",
            "createdAt": "2026-10-09T00:00:00Z", "lastActivityAt": "2026-10-09T00:00:00Z",
            "worktree": {"path": "/workspace", "branch": "main", "isolated": false}, "lastError": null
        }))
        .unwrap();
        session.status = status;
        session
    }

    fn with_turn(mut session: Session, activity: serde_json::Value) -> Session {
        session.messages = serde_json::from_value(json!([
            { "id": "u", "sessionId": session.id, "role": "user", "content": "Do it", "createdAt": "t", "streaming": false },
            { "id": "a", "sessionId": session.id, "role": "agent", "content": "", "createdAt": "t", "streaming": true, "activity": activity }
        ]))
        .unwrap();
        session
    }

    fn turn(ended: bool, items: serde_json::Value) -> serde_json::Value {
        json!({ "provider": "codex", "model": null, "startedAt": 1, "endedAt": ended.then_some(2),
            "waitingSince": null, "pausedMs": 0, "status": "running", "items": items, "truncated": false })
    }

    fn now() -> DateTime<Utc> {
        crate::snooze::parse("2026-10-12T12:00:00Z").unwrap()
    }

    #[test]
    fn only_turns_cut_while_the_agent_worked_count() {
        assert!(cut_mid_turn(&session("a", SessionStatus::Running)));
        assert!(cut_mid_turn(&session("a", SessionStatus::Starting)));
        assert!(
            !cut_mid_turn(&session("a", SessionStatus::Waiting)),
            "an approval or a question waits for the person"
        );
        for status in [
            SessionStatus::Stopped,
            SessionStatus::Completed,
            SessionStatus::Failed,
            SessionStatus::Idle,
        ] {
            assert!(!cut_mid_turn(&session("a", status)));
        }
        let running = with_turn(session("a", SessionStatus::Running), turn(false, json!([])));
        assert!(cut_mid_turn(&running));
        let ended = with_turn(session("a", SessionStatus::Running), turn(true, json!([])));
        assert!(!cut_mid_turn(&ended), "the turn had already ended");
        let secret = json!([{ "id": "t", "kind": "tool", "label": "Tool call", "state": "running",
            "model": null, "detail": "request_secret", "startedAt": 1, "endedAt": null }]);
        let asking = with_turn(session("a", SessionStatus::Running), turn(false, secret));
        let item = &asking.messages.loaded().unwrap()[1]
            .activity
            .as_ref()
            .unwrap()
            .items[0];
        assert_eq!(
            (&item.kind, &item.state),
            (&ItemKind::Tool, &ItemState::Running)
        );
        assert!(!cut_mid_turn(&asking), "waiting on a secret");
    }

    #[test]
    fn ordinary_sessions_continue_within_the_limits() {
        let mut data = AppData {
            sessions: vec![
                session("a", SessionStatus::Stopped),
                session("side", SessionStatus::Stopped),
                session("child", SessionStatus::Stopped),
                session("other", SessionStatus::Stopped),
            ],
            ..AppData::default()
        };
        data.sessions[1].side_chat = Some(crate::side_chat::SideChatOrigin {
            parent_session_id: "a".into(),
        });
        data.sessions[2].delegated_by = Some(crate::delegation::DelegatedBy {
            session_id: "a".into(),
            turn: "t".into(),
            notify: true,
            settled: false,
            delivered: false,
        });
        let interrupted: Vec<String> = ["a", "side", "child"].map(String::from).to_vec();
        let a = vec!["a".to_string()];
        assert_eq!(
            resumable(&data, &interrupted, now()),
            a,
            "side chats and delegated tasks have their own paths; `other` was not cut"
        );

        data.settings.continue_interrupted_sessions = false;
        assert!(resumable(&data, &a, now()).is_empty(), "setting off");
        data.settings.continue_interrupted_sessions = true;

        data.sessions[0].stopped_by_user_at = Some("t".into());
        assert!(
            resumable(&data, &a, now()).is_empty(),
            "stopped by the person"
        );
        data.sessions[0].stopped_by_user_at = None;

        data.sessions[0].snoozed_until = Some("2026-10-13T00:00:00Z".into());
        assert!(resumable(&data, &a, now()).is_empty(), "snoozed");
        data.sessions[0].snoozed_until = Some("2026-10-12T11:00:00Z".into());
        assert_eq!(resumable(&data, &a, now()), a, "the snooze elapsed");
        data.sessions[0].snoozed_until = None;

        data.sessions[0].auto_continuations = MAX_CONTINUATIONS - 1;
        assert_eq!(resumable(&data, &a, now()), a);
        data.sessions[0].auto_continuations = MAX_CONTINUATIONS;
        assert!(
            resumable(&data, &a, now()).is_empty(),
            "three in a row need a person"
        );
    }

    #[test]
    fn a_normal_end_resets_the_count() {
        let mut ended = session("a", SessionStatus::Running);
        ended.auto_continuations = 2;
        crate::agent::finalize_session(&mut ended, Some(0), false, false, false, None);
        assert_eq!(ended.status, SessionStatus::Completed);
        assert_eq!(ended.auto_continuations, 0);
        let mut failed = session("b", SessionStatus::Running);
        failed.auto_continuations = 2;
        crate::agent::finalize_session(&mut failed, Some(1), false, false, false, None);
        assert_eq!(failed.auto_continuations, 2);
    }

    #[test]
    fn the_prompt_asks_to_check_before_repeating() {
        assert!(prompt(false).contains("Before repeating anything"));
        assert!(prompt(true).contains("Antes de repetir"));
    }
}
