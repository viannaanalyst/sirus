//! Idle compaction (ADR-157, beta, off by default): an idle Claude conversation with a large
//! context sends `/compact` shortly before its prompt cache expires, so coming back does not
//! resend the whole conversation uncached. The expiry comes from the last main request
//! (`ContextUsage::cache_expires_at`, set in `claude.rs`).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use crate::commands::AppState;
use crate::models::{AgentProviderId, Session};
use crate::runtime::{AppHandle, Manager};

/// Below this a cold re-read is cheap, so compacting mostly loses context for little saving
/// (Claude Code's own idle compaction uses the same floor).
const MIN_TOKENS: u64 = 200_000;
/// Start this long before the cache expires: enough to resume and compact while it is warm.
const LEAD_MS: i64 = 2 * 60 * 1000;
const PASS: Duration = Duration::from_secs(30);
const FIRST_PASS: Duration = Duration::from_secs(60);

/// Whether this conversation should compact now: idle, large, and inside its window.
fn due(session: &Session, now_ms: i64) -> bool {
    let Some(usage) = session.context_usage else {
        return false;
    };
    let Some(expires) = usage.cache_expires_at else {
        return false;
    };
    session.agent == AgentProviderId::Claude
        && session.native_thread.is_some()
        && !session.status.is_active()
        && session.pending_requests.is_empty()
        && session.snoozed_until.is_none()
        && !session.is_auxiliary()
        && session.team_worker.is_none()
        && usage.used >= MIN_TOKENS
        && now_ms >= expires - LEAD_MS
        && now_ms < expires
}

pub fn start(app: AppHandle) {
    crate::runtime::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        tokio::time::sleep(FIRST_PASS).await;
        // One compaction per cache window: (session, expiry) pairs already sent.
        let mut sent: HashSet<(String, i64)> = HashSet::new();
        loop {
            if state.ensure_running().is_err() {
                return;
            }
            pass(&app, &state, &mut sent).await;
            tokio::time::sleep(PASS).await;
        }
    });
}

async fn pass(app: &AppHandle, state: &Arc<AppState>, sent: &mut HashSet<(String, i64)>) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let due_now: Vec<(String, i64, crate::models::ExecutionOptions)> = {
        let data = state.data.lock();
        if !data.settings.compact_idle_before_cache_expiry {
            return;
        }
        data.sessions
            .iter()
            .filter(|session| due(session, now_ms))
            .filter_map(|session| {
                let expires = session.context_usage?.cache_expires_at?;
                Some((session.id.clone(), expires, session.execution.clone()))
            })
            .filter(|(id, expires, _)| !sent.contains(&(id.clone(), *expires)))
            .collect()
    };
    sent.retain(|(_, expires)| *expires > now_ms);
    for (session_id, expires, execution) in due_now {
        sent.insert((session_id.clone(), expires));
        let request = crate::models::SendPromptRequest {
            queued_after: None,
            debugging: false,
            goal: None,
            session_id,
            attachment_ids: vec![],
            attachment_owner: String::new(),
            prompt: "/compact".into(),
            execution,
            team: false,
            client_request_id: None,
        };
        if let Err(error) =
            crate::commands::send_prompt(app.clone(), app.state::<Arc<AppState>>(), request).await
        {
            tracing::warn!(%error, "an idle conversation could not compact before its cache expired");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_large_idle_claude_conversation_inside_its_window_is_due() {
        let mut session: Session = serde_json::from_value(serde_json::json!({
            "id": "s1", "title": "Work", "projectId": "p", "agent": "claude", "status": "idle",
            "createdAt": "2026-10-21T00:00:00Z", "lastActivityAt": "2026-10-21T00:00:00Z",
            "worktree": {"path": "/trees/s1", "branch": "sirus/s1", "isolated": false},
            "lastError": null,
            "nativeThread": {"threadId": "t", "sessionId": "s1",
                "projectId": "p", "cwd": "/trees/s1", "model": null}
        }))
        .unwrap();
        let expires = 10_000_000;
        session.context_usage = Some(crate::models::ContextUsage {
            used: MIN_TOKENS,
            window: None,
            cache_expires_at: Some(expires),
            floor: None,
        });
        assert!(due(&session, expires - LEAD_MS));
        assert!(!due(&session, expires - LEAD_MS - 1), "too early");
        assert!(!due(&session, expires), "the cache already expired");
        session.context_usage = Some(crate::models::ContextUsage {
            used: MIN_TOKENS - 1,
            window: None,
            cache_expires_at: Some(expires),
            floor: None,
        });
        assert!(!due(&session, expires - 1000), "small context");
    }
}
