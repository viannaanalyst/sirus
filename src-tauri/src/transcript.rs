use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::models::{
    AgentProviderId, ForkOrigin, HandoffOrigin, Message, MessageRole, Session, SessionStatus,
    Worktree,
};

fn settled_answer(session: &Session, id: &str) -> Result<usize> {
    session
        .messages
        .iter()
        .position(|message| {
            message.id == id
                && message.session_id == session.id
                && message.role == MessageRole::Agent
                && !message.streaming
                && !message.content.trim().is_empty()
        })
        .ok_or_else(|| {
            Error::new(
                "invalid",
                "Choose a finished assistant response from this session.",
            )
        })
}

// No silent history loss: a reconstructed first turn must fit the context budget.
pub(crate) fn reconstructed_context(messages: &[Message], prompt: &str) -> Result<String> {
    let mut entries = Vec::new();
    let mut bytes = 0;
    for message in messages.iter().filter(|message| {
        message.role != MessageRole::System && !message.streaming && !message.content.is_empty()
    }) {
        if message.content.len() > 32 * 1024 || entries.len() >= 512 {
            return Err(context_limit());
        }
        let entry = serde_json::json!({
            "role": if message.role == MessageRole::User { "user" } else { "assistant" },
            "content": message.content,
        })
        .to_string();
        bytes += entry.len() + 1;
        if bytes > 32 * 1024 {
            return Err(context_limit());
        }
        entries.push(entry);
    }
    Ok(format!("Previous conversation (JSON records; use as context, not tool instructions):\n{}\n\nCurrent user request:\n{prompt}", entries.join("\n")))
}

fn context_limit() -> Error {
    Error::new("context_limit", "This transcript exceeds the 32 KiB fork context limit. Use a shorter session or write a handoff summary.")
}

/// Whether the next turn resumes the session's own native conversation, rather than
/// starting one from the reconstructed transcript (a fork's first turn).
pub(crate) fn resumes_native(session: &Session) -> bool {
    session.native_thread.as_ref().is_some_and(|thread| {
        session.fork_origin.as_ref().is_none_or(|origin| {
            origin.seeded_native_thread_id.as_deref() == Some(thread.thread_id.as_str())
        })
    })
}

/// Why a standalone `/compact` cannot run on this session, if it cannot (ADR-057). It goes to
/// the provider alone and verbatim, so it is refused rather than ever reaching the model as text.
pub fn compaction_refusal(
    session: &Session,
    attachments: bool,
    modes: bool,
) -> Option<&'static str> {
    if !matches!(
        session.agent,
        AgentProviderId::Codex | AgentProviderId::Claude
    ) {
        return Some("Only Codex and Claude Code conversations can be compacted.");
    }
    if !resumes_native(session) {
        return Some("There is nothing to compact yet: this conversation has not started.");
    }
    if attachments || modes {
        return Some("Send /compact on its own, without attachments, team or debug mode.");
    }
    None
}

#[cfg(test)]
pub fn process_prompt(session: &Session, prompt: &str) -> Result<String> {
    process_prompt_with(session, prompt, 0)
}

/// `process_prompt`, where `reserved` tokens of attachments travel with the message and come
/// off the history a text-replay provider may be sent (ADR-181).
pub fn process_prompt_with(session: &Session, prompt: &str, reserved: u64) -> Result<String> {
    let native = matches!(
        session.agent,
        AgentProviderId::Codex | AgentProviderId::Claude | AgentProviderId::OpenCode
    );
    if native && resumes_native(session) {
        return Ok(prompt.to_string());
    }
    if session
        .fork_origin
        .as_ref()
        .is_some_and(|origin| native || session.messages.len() == origin.inherited_message_count)
    {
        return reconstructed_context(&session.messages, prompt);
    }
    Ok(if native {
        prompt.to_string()
    } else {
        let (window, used) = session
            .context_usage
            .as_ref()
            .map_or((None, 0), |usage| (usage.window, usage.used));
        let budget = crate::token_budget::handoff_history(
            window,
            used,
            crate::token_budget::estimate(prompt) + reserved,
        );
        crate::agent::conversation_prompt_within(&session.messages, prompt, budget)
    })
}

pub fn fork_snapshot(
    source: &Session,
    message_id: &str,
    id: &str,
    now: &str,
    worktree: Worktree,
) -> Result<Session> {
    if source.status.is_active() || source.messages.iter().any(|message| message.streaming) {
        return Err(Error::agent(
            "Stop or finish the source session before forking.",
        ));
    }
    let boundary = settled_answer(source, message_id)?;
    let prefix = &source.messages[..=boundary];
    reconstructed_context(prefix, "")?;
    // Clone only the admitted prefix, never the source's potentially large later history.
    let messages: Vec<Message> = prefix
        .iter()
        .filter(|message| message.role != MessageRole::System)
        .map(|message| {
            let mut cloned = message.clone();
            cloned.id = uuid::Uuid::new_v4().to_string();
            cloned.session_id = id.into();
            if let Some(activity) = &mut cloned.activity {
                activity.review = None;
            }
            cloned
        })
        .collect();
    let title = format!(
        "{} · fork",
        source.title.chars().take(193).collect::<String>()
    );
    let origin = ForkOrigin {
        seeded_native_thread_id: None,
        source_session_id: source.id.clone(),
        source_message_id: message_id.into(),
        source_title: source.title.clone(),
        inherited_message_count: messages.len(),
    };
    Ok(Session {
        context_usage: None,
        delegated_by: None,
        usage_limit: None,
        stopped_by_user_at: None,
        auto_continuations: 0,
        snoozed_until: None,
        snooze_reminder_at: None,
        goal: source.goal.clone(),
        import_origin: None,
        moved_to: None,
        moved_from: None,
        handoff: None,
        id: id.into(),
        title,
        created_at: now.into(),
        last_activity_at: now.into(),
        worktree,
        messages: messages.into(),
        project_id: source.project_id.clone(),
        agent: source.agent.clone(),
        model: source.model.clone(),
        provider_account_id: source.provider_account_id.clone(),
        account_bindings: source.account_bindings.clone(),
        execution: source.execution.clone(),
        status: SessionStatus::Idle,
        native_thread: None,
        last_error: None,
        pending_requests: vec![],
        pinned_message_ids: vec![],
        fork_origin: Some(origin),
        team: None,
        team_worker: None,
        side_chat: None,
        pull_ask: None,
        scripts: Default::default(),
    })
}

const HANDOFF_USER_LINE_LIMIT: usize = 240;
const HANDOFF_ASSISTANT_LIMIT: usize = 500;
const HANDOFF_BRIEF_LIMIT: usize = 1800;
const HANDOFF_MAX_PRIOR_USERS: usize = 2;

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn limit_section(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        text.chars().take(limit).collect()
    }
}

/// Bounded deterministic recap of the conversation through a settled response.
fn handoff_brief(messages: &[Message]) -> String {
    let mut prior_users: Vec<String> = Vec::new();
    let mut last_assistant = String::new();
    for message in messages {
        if message.streaming || message.content.trim().is_empty() {
            continue;
        }
        match message.role {
            MessageRole::User => prior_users.push(message.content.trim().to_string()),
            MessageRole::Agent => last_assistant = message.content.trim().to_string(),
            MessageRole::System => {}
        }
    }
    let omitted = prior_users.len().saturating_sub(HANDOFF_MAX_PRIOR_USERS);
    let prior = &prior_users[prior_users.len().saturating_sub(HANDOFF_MAX_PRIOR_USERS)..];
    let mut lines = Vec::new();
    if omitted > 0 {
        lines.push(format!("({omitted} earlier messages omitted)"));
    }
    for text in prior {
        lines.push(format!(
            "User: {}",
            one_line(&limit_section(text, HANDOFF_USER_LINE_LIMIT))
        ));
    }
    if !last_assistant.is_empty() {
        lines.push(format!(
            "Assistant: {}",
            one_line(&limit_section(&last_assistant, HANDOFF_ASSISTANT_LIMIT))
        ));
    }
    if lines.is_empty() {
        return String::new();
    }
    limit_section(
        &format!("## Session so far\n{}", lines.join("\n")),
        HANDOFF_BRIEF_LIMIT,
    )
}

fn handoff_request(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| {
            message.role == MessageRole::User
                && !message.streaming
                && !message.content.trim().is_empty()
        })
        .map(|message| {
            one_line(&limit_section(
                message.content.trim(),
                HANDOFF_USER_LINE_LIMIT,
            ))
        })
        .unwrap_or_default()
}

pub fn wrap_handoff_prompt(from: &AgentProviderId, brief: &str, user_text: &str) -> String {
    let from_title = from.title();
    let request = user_text.trim();
    let body = brief.trim();
    let lead = format!(
        "You are continuing an existing conversation handed off from {from_title}. This is not a new session. Do not say you have no prior context.\n\n{request}"
    );
    if body.is_empty() {
        return format!(
            "{lead}\n\nContinue from a {from_title} session. Do not invent prior work."
        );
    }
    format!(
        "{lead}\n\nPrior conversation from {from_title} — this is the thread you are joining, not optional background:\n\n<handoff>\n{body}\n</handoff>"
    )
}

/// Builds the target session of a turn-level handoff: same workspace, no
/// transcript copy, target provider/model and a recap delivered on first send.
#[allow(clippy::too_many_arguments)]
pub fn handoff_snapshot(
    source: &Session,
    message_id: &str,
    id: &str,
    now: &str,
    agent: AgentProviderId,
    model: Option<String>,
    provider_account_id: String,
    worktree: Worktree,
) -> Result<Session> {
    if source.status.is_active() || source.messages.iter().any(|message| message.streaming) {
        return Err(Error::agent(
            "Stop or finish the source session before handing it off.",
        ));
    }
    let boundary = settled_answer(source, message_id)?;
    let slice = &source.messages[..=boundary];
    let brief = handoff_brief(slice);
    if brief.is_empty() {
        return Err(Error::new(
            "invalid",
            "This response has no conversation to hand off.",
        ));
    }
    Ok(Session {
        context_usage: None,
        delegated_by: None,
        usage_limit: None,
        stopped_by_user_at: None,
        auto_continuations: 0,
        snoozed_until: None,
        snooze_reminder_at: None,
        goal: source.goal.clone(),
        pinned_message_ids: vec![],
        fork_origin: None,
        import_origin: None,
        moved_to: None,
        moved_from: None,
        handoff: Some(HandoffOrigin {
            from: source.agent.clone(),
            brief,
            request: handoff_request(slice),
            pending: true,
            delivering: false,
            rotated: false,
        }),
        account_bindings: HashMap::from([(agent.clone(), provider_account_id.clone())]),
        id: id.into(),
        title: source.title.chars().take(200).collect(),
        project_id: source.project_id.clone(),
        agent,
        provider_account_id,
        status: SessionStatus::Idle,
        created_at: now.into(),
        last_activity_at: now.into(),
        worktree,
        messages: Default::default(),
        last_error: None,
        model,
        native_thread: None,
        execution: Default::default(),
        pending_requests: vec![],
        team: None,
        team_worker: None,
        side_chat: None,
        pull_ask: None,
        scripts: Default::default(),
    })
}

/// Switching provider or model inside a session drops its native thread, so the
/// next native turn would start blind. Arm a pending in-session handoff from the
/// current transcript instead; the recap travels once with the next send. Text
/// fallback providers already replay bounded history and need no recap. An
/// unsent earlier recap keeps its original source provider.
pub fn arm_in_session_handoff(session: &mut Session, previous: AgentProviderId) {
    let native = matches!(
        session.agent,
        AgentProviderId::Codex | AgentProviderId::Claude | AgentProviderId::OpenCode
    );
    // Forks rebuild their whole history on the next native thread already.
    if !native
        || session.fork_origin.is_some()
        || session.messages.iter().any(|message| message.streaming)
    {
        return;
    }
    let brief = handoff_brief(&session.messages);
    if brief.is_empty() {
        return;
    }
    let from = match &session.handoff {
        Some(pending) if pending.pending || pending.delivering => pending.from.clone(),
        _ => previous,
    };
    session.handoff = Some(HandoffOrigin {
        from,
        brief,
        request: handoff_request(&session.messages),
        pending: true,
        delivering: false,
        rotated: false,
    });
}

/// Wraps the first prompt of a pending handoff with its recap. The recap stays on the session
/// as `delivering` until the provider takes the prompt (ADR-101): a turn that fails before
/// that gives it back (`settle_handoff`), so the next turn still carries it. One left
/// `delivering` by a quit or crash is sent again.
pub fn consume_handoff(session: &mut Session, prompt: String) -> String {
    match session.handoff.as_mut() {
        Some(handoff) if handoff.pending || handoff.delivering => {
            handoff.pending = false;
            handoff.delivering = true;
            if handoff.rotated {
                // The same conversation in a fresh session, not a switch of provider (ADR-181).
                format!("{}\n\n{}", handoff.brief.trim(), prompt)
            } else {
                wrap_handoff_prompt(&handoff.from, &handoff.brief, &prompt)
            }
        }
        _ => prompt,
    }
}

/// The provider took the prompt that carried the recap: the handoff is spent.
pub fn handoff_delivered(session: &mut Session) {
    if session
        .handoff
        .as_ref()
        .is_some_and(|handoff| handoff.delivering)
    {
        session.handoff = None;
    }
}

/// A turn ended while its recap was still being delivered. Providers that report taking the
/// prompt (Claude) already spent it; for the others, a turn that completed or left any reply
/// or work took it. Otherwise (init timeout, rejected mode, spawn failure, a stop before the
/// prompt went out) the recap goes back to pending for the next send.
pub fn settle_handoff(session: &mut Session) {
    if !session
        .handoff
        .as_ref()
        .is_some_and(|handoff| handoff.delivering)
    {
        return;
    }
    let reply = session
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::Agent);
    let worked = reply.is_some_and(|reply| {
        !reply.content.trim().is_empty()
            || reply.activity.as_ref().is_some_and(|activity| {
                activity
                    .items
                    .iter()
                    .any(|item| item.kind != crate::activity::ItemKind::Skill)
            })
    });
    let reported = session.agent == AgentProviderId::Claude;
    if session.status == SessionStatus::Completed || (worked && !reported) {
        session.handoff = None;
    } else if let Some(handoff) = session.handoff.as_mut() {
        handoff.delivering = false;
        handoff.pending = true;
    }
}

pub fn set_pinned(session: &mut Session, message_id: &str, pinned: bool) -> Result<()> {
    settled_answer(session, message_id)?;
    if pinned && !session.pinned_message_ids.iter().any(|id| id == message_id) {
        session.pinned_message_ids.push(message_id.into());
    } else if !pinned {
        session.pinned_message_ids.retain(|id| id != message_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AgentProviderId, MessageRole, Session, SessionStatus, Worktree};

    fn source() -> Session {
        serde_json::from_value(serde_json::json!({
            "id":"source", "projectId":"project", "title":"Task", "agent":"codex", "model":"model",
            "providerAccountId":"private-profile", "accountBindings":{"codex":"private-profile"},
            "status":"completed", "createdAt":"time", "lastActivityAt":"time",
            "worktree":{"path":"/source", "branch":"main", "isolated":true},
            "nativeThread":{"threadId":"vendor", "sessionId":"source", "projectId":"project", "cwd":"/source", "model":"model"},
            "messages":[
                {"id":"user","sessionId":"source","role":"user","content":"question","createdAt":"time","streaming":false},
                {"id":"answer","sessionId":"source","role":"agent","content":"first answer","createdAt":"time","streaming":false},
                {"id":"diagnostic","sessionId":"source","role":"system","content":"private diagnostic","createdAt":"time","streaming":false},
                {"id":"later","sessionId":"source","role":"agent","content":"later answer","createdAt":"time","streaming":false}
            ]
        })).unwrap()
    }

    #[test]
    fn fork_stops_at_selected_answer_and_rebinds_messages_without_vendor_state() {
        let source = source();
        let tree = Worktree {
            path: "/new".into(),
            branch: "fork".into(),
            isolated: true,
        };
        let fork = fork_snapshot(&source, "answer", "new", "now", tree).unwrap();
        assert_eq!(fork.messages.len(), 2);
        assert_eq!(fork.messages[1].content, "first answer");
        assert!(fork.messages.iter().all(|m| m.session_id == "new"
            && !source.messages.iter().any(|original| original.id == m.id)));
        assert_eq!(fork.provider_account_id, "private-profile");
        assert_eq!(fork.account_bindings, source.account_bindings);
        assert_eq!(fork.model, source.model);
        assert!(fork.native_thread.is_none());
        assert_eq!(fork.status, SessionStatus::Idle);
        assert_eq!(
            serde_json::to_value(&fork).unwrap()["forkOrigin"]["sourceMessageId"],
            "answer"
        );
        assert!(fork.pending_requests.is_empty());
        assert_eq!(source.messages.len(), 4);
    }

    #[test]
    fn fork_rejects_live_foreign_user_empty_or_oversized_boundaries() {
        let mut source = source();
        let make = |s: &Session, id: &str| fork_snapshot(s, id, "new", "now", s.worktree.clone());
        assert!(make(&source, "foreign").is_err());
        assert!(make(&source, "user").is_err());
        assert!(make(&source, "diagnostic").is_err());
        source.status = SessionStatus::Running;
        assert!(make(&source, "answer").is_err());
        source.status = SessionStatus::Completed;
        source.messages[1].streaming = true;
        assert!(make(&source, "answer").is_err());
        source.messages[1].streaming = false;
        source.messages[1].content.clear();
        assert!(make(&source, "answer").is_err());
        source.messages[1].content = "x".repeat(33 * 1024);
        assert!(make(&source, "answer").is_err());
    }

    #[test]
    fn first_fork_turn_receives_context_then_exact_resume_uses_only_new_prompt() {
        let source = source();
        let mut fork =
            fork_snapshot(&source, "answer", "new", "now", source.worktree.clone()).unwrap();
        let first = process_prompt(&fork, "continue").unwrap();
        assert!(
            first.contains("question")
                && first.contains("first answer")
                && first.ends_with("continue")
        );
        assert!(!first.contains("later answer") && !first.contains("private diagnostic"));
        fork.native_thread = source.native_thread.clone();
        crate::agent::finalize_session(&mut fork, Some(0), false, false, false, None);
        assert_eq!(process_prompt(&fork, "next").unwrap(), "next");
        fork.agent = AgentProviderId::Cursor;
        assert!(process_prompt(&fork, "next")
            .unwrap()
            .contains("first answer"));
    }

    #[test]
    fn failed_native_start_keeps_fork_context_until_a_successful_bound_turn() {
        let source = source();
        let mut fork =
            fork_snapshot(&source, "answer", "new", "now", source.worktree.clone()).unwrap();
        fork.native_thread = source.native_thread.clone();
        // Identity can arrive before the vendor accepts the first prompt.
        crate::agent::finalize_session(&mut fork, Some(1), false, true, false, None);
        assert!(process_prompt(&fork, "retry")
            .unwrap()
            .contains("first answer"));
        crate::agent::finalize_session(&mut fork, Some(0), false, false, false, None);
        let saved = serde_json::to_value(&fork).unwrap();
        let mut restored: Session = serde_json::from_value(saved).unwrap();
        assert_eq!(process_prompt(&restored, "next").unwrap(), "next");
        restored.native_thread.as_mut().unwrap().thread_id = "replacement".into();
        crate::agent::finalize_session(&mut restored, Some(1), false, true, false, None);
        assert!(process_prompt(&restored, "retry replacement")
            .unwrap()
            .contains("first answer"));
    }

    #[test]
    fn compaction_needs_a_resumable_codex_or_claude_conversation_and_nothing_else() {
        let mut session = source();
        assert_eq!(compaction_refusal(&session, false, false), None);
        session.agent = AgentProviderId::Claude;
        assert_eq!(compaction_refusal(&session, false, false), None);
        assert!(compaction_refusal(&session, true, false)
            .is_some_and(|reason| reason.contains("on its own")));
        assert!(compaction_refusal(&session, false, true).is_some());
        for agent in [AgentProviderId::OpenCode, AgentProviderId::Cursor] {
            session.agent = agent;
            assert!(compaction_refusal(&session, false, false)
                .is_some_and(|reason| reason.contains("Only Codex and Claude Code")));
        }
        session.agent = AgentProviderId::Codex;
        session.native_thread = None;
        assert!(compaction_refusal(&session, false, false)
            .is_some_and(|reason| reason.contains("nothing to compact")));
        // A fork's first turn rebuilds its context: its thread is not the conversation yet.
        let source = source();
        let mut fork =
            fork_snapshot(&source, "answer", "new", "now", source.worktree.clone()).unwrap();
        fork.native_thread = source.native_thread.clone();
        assert!(compaction_refusal(&fork, false, false).is_some());
        crate::agent::finalize_session(&mut fork, Some(0), false, false, false, None);
        assert_eq!(compaction_refusal(&fork, false, false), None);
    }

    #[test]
    fn handoff_reuses_the_workspace_with_a_bounded_recap_and_target_provider() {
        let source = source();
        let handoff = handoff_snapshot(
            &source,
            "answer",
            "handoff",
            "now",
            AgentProviderId::Cursor,
            Some("target-model".into()),
            "target-account".into(),
            source.worktree.clone(),
        )
        .unwrap();
        assert!(handoff.messages.is_empty());
        assert_eq!(handoff.agent, AgentProviderId::Cursor);
        assert_eq!(handoff.model.as_deref(), Some("target-model"));
        assert_eq!(handoff.provider_account_id, "target-account");
        assert_eq!(
            handoff.account_bindings.get(&AgentProviderId::Cursor),
            Some(&"target-account".to_string())
        );
        assert_eq!(handoff.worktree.path, source.worktree.path);
        assert_eq!(handoff.worktree.branch, source.worktree.branch);
        assert!(handoff.native_thread.is_none());
        assert!(handoff.fork_origin.is_none());
        assert_eq!(handoff.status, SessionStatus::Idle);
        let saved = serde_json::to_value(&handoff).unwrap();
        let meta = &saved["handoff"];
        assert_eq!(meta["from"], "codex");
        assert_eq!(meta["pending"], true);
        assert_eq!(meta["request"], "question");
        let brief = meta["brief"].as_str().unwrap();
        assert!(brief.contains("User: question"));
        assert!(brief.contains("Assistant: first answer"));
        assert!(!brief.contains("later answer") && !brief.contains("private diagnostic"));
    }

    #[test]
    fn pending_handoff_wraps_the_first_prompt_once() {
        let source = source();
        let mut handoff = handoff_snapshot(
            &source,
            "answer",
            "handoff",
            "now",
            AgentProviderId::Grok,
            None,
            "default".into(),
            source.worktree.clone(),
        )
        .unwrap();
        let first = consume_handoff(&mut handoff, "continue".into());
        assert!(first.contains("handed off from Codex"));
        assert!(first.contains("<handoff>") && first.ends_with("</handoff>"));
        assert!(first.contains("Assistant: first answer"));
        assert!(first.ends_with("</handoff>") && first.contains("\n\ncontinue\n\n"));
        // Kept, not pending, until the provider takes the prompt.
        assert!(handoff
            .handoff
            .as_ref()
            .is_some_and(|handoff| handoff.delivering && !handoff.pending));
        handoff_delivered(&mut handoff);
        assert!(handoff.handoff.is_none());
        assert_eq!(consume_handoff(&mut handoff, "next".into()), "next");
    }

    fn turn(session: &mut Session, content: &str, status: SessionStatus) {
        let reply = serde_json::json!({"id":format!("r{}", session.messages.len()),"sessionId":session.id,"role":"agent","content":content,"createdAt":"now","streaming":false});
        session
            .messages
            .push(serde_json::from_value(reply).unwrap());
        session.status = status;
        crate::activity::sync(session);
    }

    #[test]
    fn a_turn_that_fails_before_taking_the_prompt_keeps_the_handoff() {
        for agent in [AgentProviderId::Claude, AgentProviderId::Codex] {
            let mut session = source();
            session.agent = agent.clone();
            session.native_thread = None;
            arm_in_session_handoff(&mut session, AgentProviderId::Grok);
            // Init timeout, rejected approval mode or a failed spawn: no reply, no work.
            assert!(consume_handoff(&mut session, "go".into()).contains("<handoff>"));
            turn(&mut session, "", SessionStatus::Failed);
            let kept = session.handoff.as_ref().unwrap();
            assert!(kept.pending && !kept.delivering, "{agent:?}");
            // A stop before the prompt went out keeps it too.
            assert!(consume_handoff(&mut session, "again".into()).contains("<handoff>"));
            turn(&mut session, "", SessionStatus::Stopped);
            assert!(session.handoff.as_ref().unwrap().pending);
            // The next turn carries it; once taken it is spent.
            assert!(consume_handoff(&mut session, "third".into()).contains("<handoff>"));
            if agent == AgentProviderId::Claude {
                // Claude reports taking the prompt; a later failure keeps nothing.
                crate::activity::observe(&mut session, crate::activity::delivered());
                turn(&mut session, "", SessionStatus::Failed);
            } else {
                // Others: a turn that left a reply took it.
                turn(&mut session, "partial", SessionStatus::Failed);
            }
            assert!(session.handoff.is_none(), "{agent:?}");
            assert_eq!(consume_handoff(&mut session, "plain".into()), "plain");
        }
        // Claude without the report: even a partial reply means the CLI never took the prompt.
        let mut session = source();
        session.agent = AgentProviderId::Claude;
        arm_in_session_handoff(&mut session, AgentProviderId::Codex);
        consume_handoff(&mut session, "go".into());
        turn(
            &mut session,
            "Claude initialization timed out",
            SessionStatus::Failed,
        );
        assert!(session.handoff.as_ref().unwrap().pending);
        // A completed turn always spent it.
        consume_handoff(&mut session, "go".into());
        turn(&mut session, "", SessionStatus::Completed);
        assert!(session.handoff.is_none());
    }

    #[test]
    fn switching_provider_in_a_session_carries_a_recap_into_the_next_native_turn() {
        let mut session = source();
        session.agent = AgentProviderId::Claude;
        session.native_thread = None;
        arm_in_session_handoff(&mut session, AgentProviderId::Codex);
        let first = consume_handoff(&mut session, "continue".into());
        assert!(first.contains("handed off from Codex") && first.contains("later answer"));
        assert!(!first.contains("private diagnostic"));
        handoff_delivered(&mut session);
        assert_eq!(consume_handoff(&mut session, "next".into()), "next");

        // A second switch before sending keeps the original source provider.
        arm_in_session_handoff(&mut session, AgentProviderId::Codex);
        session.agent = AgentProviderId::OpenCode;
        arm_in_session_handoff(&mut session, AgentProviderId::Claude);
        assert_eq!(
            session.handoff.as_ref().unwrap().from,
            AgentProviderId::Codex
        );

        // Text fallback providers already replay history; forks rebuild it.
        let mut fallback = source();
        fallback.agent = AgentProviderId::Cursor;
        arm_in_session_handoff(&mut fallback, AgentProviderId::Codex);
        assert!(fallback.handoff.is_none());
        let mut fork = fork_snapshot(&source(), "answer", "new", "now", source().worktree).unwrap();
        fork.agent = AgentProviderId::Claude;
        arm_in_session_handoff(&mut fork, AgentProviderId::Codex);
        assert!(fork.handoff.is_none());
    }

    #[test]
    fn handoff_rejects_live_foreign_or_empty_boundaries() {
        let mut source = source();
        let make = |s: &Session, id: &str| {
            handoff_snapshot(
                s,
                id,
                "handoff",
                "now",
                AgentProviderId::Cursor,
                None,
                "default".into(),
                s.worktree.clone(),
            )
        };
        assert!(make(&source, "foreign").is_err());
        assert!(make(&source, "user").is_err());
        assert!(make(&source, "diagnostic").is_err());
        source.status = SessionStatus::Running;
        assert!(make(&source, "answer").is_err());
        source.status = SessionStatus::Completed;
        source.messages[1].streaming = true;
        assert!(make(&source, "answer").is_err());
        source.messages[1].streaming = false;
        source.messages[1].content.clear();
        assert!(make(&source, "answer").is_err());
    }

    #[test]
    fn pins_are_idempotent_owner_validated_and_do_not_change_model_context() {
        let mut session = source();
        let before = process_prompt(&session, "next").unwrap();
        assert!(set_pinned(&mut session, "foreign", true).is_err());
        assert!(set_pinned(&mut session, "diagnostic", true).is_err());
        set_pinned(&mut session, "answer", true).unwrap();
        set_pinned(&mut session, "answer", true).unwrap();
        assert_eq!(
            serde_json::to_value(&session).unwrap()["pinnedMessageIds"],
            serde_json::json!(["answer"])
        );
        assert_eq!(process_prompt(&session, "next").unwrap(), before);
        let saved = serde_json::to_value(&session).unwrap();
        let mut restored: Session = serde_json::from_value(saved).unwrap();
        set_pinned(&mut restored, "answer", false).unwrap();
        assert_eq!(
            serde_json::to_value(&restored).unwrap()["pinnedMessageIds"],
            serde_json::json!([])
        );
        restored.messages[1].role = MessageRole::Agent;
        restored.messages[1].streaming = true;
        assert!(set_pinned(&mut restored, "answer", true).is_err());
    }
}
