//! "Editar" on the last message: the message and everything after it leave the transcript and
//! its text goes back to the composer, so sending again answers the edited message instead. A
//! provider with its own thread starts a new one primed with the kept history, as a fork does,
//! since none of them can drop a turn from a thread it already holds.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::commands::{native_task, AppState};
use crate::error::{Error, Result};
use crate::models::{AgentProviderId, ForkOrigin, MessageRole, SessionStatus};
use crate::runtime::State;

pub async fn rewind_last_turn(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Value> {
    let state = state.inner().clone();
    native_task(move || {
        let mut data = state.data_with_messages(&[&session_id]);
        state.ensure_running()?;
        if state.agents.lock().contains_key(&session_id) {
            return Err(Error::new("busy", "The conversation is still working."));
        }
        let session = data
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        if session.status.is_active()
            || session.status == SessionStatus::Waiting
            || session.messages.iter().any(|message| message.streaming)
        {
            return Err(Error::new("busy", "The conversation is still working."));
        }
        if session.pull_ask.is_some() || session.team.is_some() {
            return Err(Error::new(
                "invalid",
                "This conversation's messages cannot be edited.",
            ));
        }
        let at = session
            .messages
            .iter()
            .rposition(|message| message.role == MessageRole::User)
            .ok_or_else(|| Error::new("invalid", "There is no message to edit."))?;
        let prompt = session.messages[at].content.clone();
        let native = matches!(
            session.agent,
            AgentProviderId::Codex | AgentProviderId::Claude | AgentProviderId::OpenCode
        );
        let kept = &session.messages[..at];
        if native && !kept.is_empty() {
            crate::transcript::reconstructed_context(kept, "").map_err(|_| {
                Error::new(
                    "context_limit",
                    "This conversation is too long to edit its last message. Send a new message instead.",
                )
            })?;
        }
        let origin = (native && !kept.is_empty()).then(|| ForkOrigin {
            seeded_native_thread_id: None,
            source_session_id: session.id.clone(),
            source_message_id: kept.last().map(|m| m.id.clone()).unwrap_or_default(),
            source_title: session.title.clone(),
            inherited_message_count: kept.len(),
        });
        session.messages.truncate(at);
        session.native_thread = None;
        session.fork_origin = origin;
        session.handoff = None;
        session.context_usage = None;
        session.last_error = None;
        session.status = SessionStatus::Idle;
        let tail = crate::transcript_view::session_tail(session)?;
        crate::persist::save(&state.data_path, &data)?;
        Ok(json!({ "prompt": prompt, "tail": tail }))
    })
    .await
}
