//! Context rotation without a model call (ADR-181).
//!
//! Every turn of a native Claude or Codex conversation resends its whole context. With the
//! setting on, once the reported context reaches 250,000 tokens or 80% of the window (the lower), the next send starts a
//! fresh provider session instead. The new session is seeded with a brief that code builds
//! from the transcript (nothing is summarized by a model): the last two exchanges word for
//! word, then one short line per earlier exchange. It rides the handoff plumbing (ADR-028,
//! ADR-101), so a turn that fails before the provider takes the prompt keeps the brief.

use crate::models::{
    AgentProviderId, ContextRotation, HandoffOrigin, Message, MessageRole, Session,
};

/// Rotate at this share of the reported window, leaving room for the next turn.
pub const ROTATE_RATIO: f64 = 0.8;
/// A fixed ceiling: a 1M window still rotates here, since every step rereads the whole context.
pub const ROTATE_AT_TOKENS: u64 = 250_000;

/// The context size that triggers a rotation for a window: the lower of 250,000 tokens and 80%.
pub fn threshold(window: u64) -> u64 {
    ROTATE_AT_TOKENS.min((window as f64 * ROTATE_RATIO) as u64)
}
/// Recent exchanges carried word for word.
const VERBATIM_EXCHANGES: usize = 2;
const VERBATIM_USER_CHARS: usize = 2_000;
const VERBATIM_REPLY_CHARS: usize = 3_000;
/// Earlier exchanges: one line each, and this many characters for all of them (newest kept).
const LINE_CHARS: usize = 280;
const EARLIER_CHARS: usize = 3_000;

struct Exchange {
    user: String,
    reply: String,
}

/// The user's messages and the last thing the agent said after each.
fn exchanges(messages: &[Message]) -> Vec<Exchange> {
    let mut out: Vec<Exchange> = Vec::new();
    for message in messages {
        let text = message.content.trim();
        if message.streaming || text.is_empty() {
            continue;
        }
        match message.role {
            MessageRole::User => out.push(Exchange {
                user: text.to_string(),
                reply: String::new(),
            }),
            MessageRole::Agent => {
                if let Some(current) = out.last_mut() {
                    current.reply = text.to_string();
                }
            }
            MessageRole::System => {}
        }
    }
    out
}

/// Keeps the head and the tail of a long text within `max` characters.
fn clip(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let half = (max - 5) / 2;
    let head: String = text.chars().take(half).collect();
    let tail: String = text.chars().skip(count - half).collect();
    format!("{} […] {}", head.trim_end(), tail.trim_start())
}

fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

fn line(exchange: &Exchange) -> String {
    // The whole line stays within LINE_CHARS: the request gets up to half, the reply the rest.
    let user = one_line(&exchange.user, LINE_CHARS / 2);
    if exchange.reply.is_empty() {
        return format!("- User: {user}");
    }
    let used = "- User:  → Assistant: ".chars().count() + user.chars().count();
    let reply = one_line(&exchange.reply, LINE_CHARS.saturating_sub(used).max(2));
    format!("- User: {user} → Assistant: {reply}")
}

/// The newest lines that fit `budget` characters, oldest first.
fn newest_within(lines: Vec<String>, budget: usize) -> Vec<String> {
    let mut used = 0;
    let mut kept: Vec<String> = Vec::new();
    for line in lines.into_iter().rev() {
        used += line.chars().count() + 1;
        if used > budget {
            break;
        }
        kept.push(line);
    }
    kept.reverse();
    kept
}

/// The deterministic brief a rotated session starts with. Empty when there is nothing to carry.
pub fn brief(messages: &[Message]) -> String {
    let messages = crate::transcript::since_clear(messages);
    let all = exchanges(messages);
    if all.is_empty() {
        return String::new();
    }
    let split = all.len().saturating_sub(VERBATIM_EXCHANGES);
    let earlier = newest_within(all[..split].iter().map(line).collect(), EARLIER_CHARS);
    let mut parts = vec![
        "This is the same conversation. It continues in a fresh session so the context stays small; nothing was lost. Pick up where it left off and do not mention the switch unless asked."
            .to_string(),
    ];
    if !earlier.is_empty() {
        parts.push(format!("Earlier, oldest first:\n{}", earlier.join("\n")));
    }
    let recent: Vec<String> = all[split..]
        .iter()
        .map(|exchange| {
            let user = clip(&exchange.user, VERBATIM_USER_CHARS);
            if exchange.reply.is_empty() {
                format!("User:\n{user}")
            } else {
                format!(
                    "User:\n{user}\n\nAssistant:\n{}",
                    clip(&exchange.reply, VERBATIM_REPLY_CHARS)
                )
            }
        })
        .collect();
    parts.push(format!(
        "Most recent, word for word:\n\n{}",
        recent.join("\n\n---\n\n")
    ));
    format!(
        "<previous_conversation>\n{}\n</previous_conversation>",
        parts.join("\n\n")
    )
}

/// Whether the next send should start a fresh provider session: the setting is on, the
/// conversation has a native Claude or Codex thread of its own and its reported context is
/// at 80% of a known window. Idle time and restarts alone never count.
pub fn due(session: &Session, enabled: bool) -> Option<ContextRotation> {
    if !enabled
        || !matches!(
            session.agent,
            AgentProviderId::Claude | AgentProviderId::Codex
        )
        || session.status.is_active()
        || session.handoff.is_some()
        || session.side_chat.is_some()
        || session.team_worker.is_some()
        || !crate::transcript::resumes_native(session)
    {
        return None;
    }
    let usage = session.context_usage.as_ref()?;
    let window = usage.window.filter(|window| *window > 0)?;
    (usage.used >= threshold(window)).then_some(ContextRotation {
        used: usage.used,
        window,
    })
}

/// Drops the native thread and arms the brief as a pending handoff to the same provider, so
/// the prompt about to be built carries it. Returns the marker for the user message.
pub fn apply(session: &mut Session, enabled: bool) -> Option<ContextRotation> {
    let rotation = due(session, enabled)?;
    let brief = brief(&session.messages);
    if brief.is_empty() {
        return None;
    }
    session.native_thread = None;
    session.context_usage = None;
    session.handoff = Some(HandoffOrigin {
        from: session.agent.clone(),
        brief,
        request: String::new(),
        pending: true,
        delivering: false,
        rotated: true,
    });
    Some(rotation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ContextUsage;

    fn message(role: MessageRole, content: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": "m", "sessionId": "s",
            "role": match role {
                MessageRole::User => "user",
                MessageRole::Agent => "agent",
                MessageRole::System => "system",
            },
            "content": content, "createdAt": "t", "streaming": false
        }))
        .unwrap()
    }

    fn chat(turns: usize) -> Vec<Message> {
        (0..turns)
            .flat_map(|index| {
                [
                    message(MessageRole::User, &format!("request {index}")),
                    message(MessageRole::Agent, &format!("answer {index}")),
                ]
            })
            .collect()
    }

    fn session(used: u64, window: Option<u64>) -> Session {
        let mut session: Session = serde_json::from_value(serde_json::json!({
            "id":"s","projectId":"p","title":"T","agent":"claude","model":null,
            "status":"idle","createdAt":"t","lastActivityAt":"t",
            "worktree":{"path":"/w","branch":"main","isolated":false},"lastError":null,
            "messages":[]
        }))
        .unwrap();
        session.native_thread = Some(crate::models::NativeThread {
            provider_account_id: "default".into(),
            thread_id: "native".into(),
            session_id: "s".into(),
            project_id: "p".into(),
            cwd: "/w".into(),
            model: None,
        });
        session.messages = chat(5).into();
        session.context_usage = Some(ContextUsage {
            used,
            window,
            cache_expires_at: None,
            floor: None,
        });
        session
    }

    #[test]
    fn the_last_two_exchanges_are_verbatim_and_earlier_ones_are_lines_in_order() {
        let text = brief(&chat(5));
        let earlier = text.find("Earlier, oldest first:").unwrap();
        let recent = text.find("Most recent, word for word:").unwrap();
        assert!(earlier < recent);
        let (before, after) = text.split_at(recent);
        assert!(before.contains("- User: request 0 → Assistant: answer 0"));
        assert!(before.find("request 0").unwrap() < before.find("request 2").unwrap());
        assert!(!before.contains("request 3") && !before.contains("request 4"));
        assert!(after.contains("User:\nrequest 3\n\nAssistant:\nanswer 3"));
        assert!(after.contains("User:\nrequest 4\n\nAssistant:\nanswer 4"));
        assert!(after.find("request 3").unwrap() < after.find("request 4").unwrap());
        assert!(brief(&[]).is_empty());
    }

    #[test]
    fn caps_hold_for_verbatim_exchanges_lines_and_the_earlier_section() {
        let long = "word ".repeat(2_000);
        let mut messages = Vec::new();
        for _ in 0..40 {
            messages.push(message(MessageRole::User, &long));
            messages.push(message(MessageRole::Agent, &long));
        }
        let text = brief(&messages);
        let earlier: Vec<&str> = text
            .split("Earlier, oldest first:\n")
            .nth(1)
            .unwrap()
            .split("\n\nMost recent")
            .next()
            .unwrap()
            .lines()
            .collect();
        assert!(earlier
            .iter()
            .all(|line| line.chars().count() <= LINE_CHARS));
        assert!(
            earlier
                .iter()
                .map(|line| line.chars().count() + 1)
                .sum::<usize>()
                <= EARLIER_CHARS
        );
        assert!(
            earlier.len() > 5 && earlier.len() < 38,
            "the newest lines win the budget"
        );
        let recent = text.split("Most recent, word for word:").nth(1).unwrap();
        let recent = recent.split("\n</previous_conversation>").next().unwrap();
        let exchanges: Vec<&str> = recent.split("\n\n---\n\n").collect();
        assert_eq!(exchanges.len(), 2);
        for exchange in exchanges {
            let (user, reply) = exchange.split_once("\n\nAssistant:\n").unwrap();
            assert!(user.trim_start().chars().count() <= VERBATIM_USER_CHARS + "User:\n".len());
            assert!(reply.chars().count() <= VERBATIM_REPLY_CHARS);
            assert!(reply.contains("[…]"));
        }
    }

    #[test]
    fn streaming_empty_and_system_messages_are_left_out() {
        let mut messages = chat(1);
        messages.push(message(MessageRole::System, "stderr noise"));
        messages.push(message(MessageRole::User, "   "));
        let mut live = message(MessageRole::Agent, "half an answ");
        live.streaming = true;
        messages.push(live);
        let text = brief(&messages);
        assert!(!text.contains("noise") && !text.contains("half an answ"));
        assert!(text.contains("answer 0"));
    }

    #[test]
    fn a_big_window_rotates_at_250k() {
        assert_eq!(threshold(200_000), 160_000);
        assert_eq!(threshold(1_000_000), 250_000);
        assert!(due(&session(250_000, Some(1_000_000)), true).is_some());
        assert!(due(&session(249_999, Some(1_000_000)), true).is_none());
    }

    #[test]
    fn it_is_due_only_at_eighty_percent_of_a_known_window_with_the_setting_on() {
        assert!(due(&session(160_000, Some(200_000)), true).is_some());
        assert!(due(&session(159_999, Some(200_000)), true).is_none());
        assert!(due(&session(160_000, Some(200_000)), false).is_none());
        assert!(
            due(&session(900_000, None), true).is_none(),
            "no window, no rotation"
        );
        let mut other = session(190_000, Some(200_000));
        other.agent = AgentProviderId::OpenCode;
        assert!(due(&other, true).is_none());
        let mut fresh = session(190_000, Some(200_000));
        fresh.native_thread = None;
        assert!(due(&fresh, true).is_none(), "nothing to rotate yet");
        let mut working = session(190_000, Some(200_000));
        working.status = crate::models::SessionStatus::Running;
        assert!(due(&working, true).is_none());
    }

    #[test]
    fn applying_it_starts_fresh_and_arms_the_brief_once() {
        let mut session = session(170_000, Some(200_000));
        let marker = apply(&mut session, true).unwrap();
        assert_eq!((marker.used, marker.window), (170_000, 200_000));
        assert!(session.native_thread.is_none() && session.context_usage.is_none());
        let handoff = session.handoff.as_ref().unwrap();
        assert!(handoff.pending && handoff.rotated);
        let prompt = crate::transcript::consume_handoff(&mut session, "next".into());
        assert!(prompt.contains("next") && prompt.contains("<previous_conversation>"));
        assert!(!prompt.contains("handed off"));
        // The fresh session is below the limit: no second rotation, and the brief is spent.
        assert!(apply(&mut session, true).is_none());
    }
}
