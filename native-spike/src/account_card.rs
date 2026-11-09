//! The card above the composer when a turn ended on an error (`lastError` of the session): what
//! went wrong, in the person's words, and a way on. Until now the window showed an empty reply
//! ("trabalhou por 13s") and hid the host's message. A sign-in failure on a shared credential
//! ("OAuth session expired") is the common case: Retry sends "continue" again (the other session
//! that refreshed the token may have finished), and each other account of the provider
//! continues the conversation there through `move_session_account`, which carries the native
//! transcript along (account_move.rs). This module is a child of `shell` (`#[path]`).

use super::*;

/// The text sent to carry on after a failure.
const CONTINUE: &str = "Continue de onde parou.";

/// A sign-in or credential failure, as the providers word it.
pub(crate) fn is_auth_error(error: &str) -> bool {
    let error = error.to_lowercase();
    [
        "authenticate",
        "oauth",
        "not logged in",
        "invalid api key",
        "please run /login",
        "401",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

/// The account's usage limit, as the providers word it: another account can carry on.
pub(crate) fn is_limit_error(error: &str) -> bool {
    let error = error.to_lowercase();
    [
        "usage limit",
        "limit reached",
        "hit your",
        "rate limit",
        "quota",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

/// When a limit comes back, from the provider's words ("resets 2:40pm"), as 14:40.
pub(crate) fn reset_time(text: &str) -> Option<String> {
    let after = text
        .rsplit("resets ")
        .next()
        .filter(|_| text.contains("resets "))?;
    let clock: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':')
        .collect();
    let rest = after[clock.len()..].trim_start().to_lowercase();
    let (hour, minute) = match clock.split_once(':') {
        Some((hour, minute)) => (hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?),
        None => (clock.parse::<u32>().ok()?, 0),
    };
    let hour = match (rest.starts_with("pm"), rest.starts_with("am"), hour) {
        (true, _, 12) => 12,
        (true, _, h) => h + 12,
        (_, true, 12) => 0,
        (_, _, h) => h,
    };
    Some(format!("{hour}:{minute:02}"))
}

/// The error in one short line for the card, or None when the session did not fail.
pub(crate) fn headline(status: &str, error: &str) -> Option<String> {
    if status != "failed" || error.trim().is_empty() {
        return None;
    }
    Some(if is_limit_error(error) {
        t("Esta conta atingiu o limite de uso.").to_owned()
    } else if is_auth_error(error) {
        t("O login desta conta expirou ou foi sobrescrito por outra sessão.").to_owned()
    } else {
        let line = error.lines().next().unwrap_or("").trim();
        let mut short: String = line.chars().take(180).collect();
        if line.chars().count() > 180 {
            short.push('…');
        }
        short
    })
}

impl Shell {
    pub(super) fn account_card(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let conversation = self.conversations.get(ix)?;
        if conversation.working() {
            return None;
        }
        let headline = headline(&conversation.status, &conversation.last_error)?;
        let session = conversation.session_id.clone();
        let approval = conversation.approval.clone();
        let current = conversation.account.clone();
        let others: Vec<(String, String)> = if is_auth_error(&conversation.last_error)
            || is_limit_error(&conversation.last_error)
        {
            self.accounts_of(conversation.provider)
                .into_iter()
                .filter(|(id, _)| *id != current)
                .collect()
        } else {
            Vec::new()
        };
        let button = |id: String, label: String| {
            div()
                .id(gpui::SharedString::from(id))
                .h(px(24.))
                .max_w(px(210.))
                .flex_shrink_0()
                .px(px(9.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .text_size(px(11.5))
                .text_color(content(0.8))
                .bg(content(0.1))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.16)))
                .child(div().min_w_0().truncate().child(label))
        };
        let retry = {
            let (session, approval) = (session.clone(), approval.clone());
            button("failure-retry".into(), t("Tentar de novo").to_owned()).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.continue_after_failure(&session, &approval);
                    cx.notify();
                },
            ))
        };
        let moves = others.into_iter().map(|(account, label)| {
            let (session, approval, target) = (session.clone(), approval.clone(), account.clone());
            button(
                format!("failure-move-{account}"),
                tf("Continuar em {}", &[&label]),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.move_after_failure(&session, &target, &approval);
                cx.notify();
            }))
        });
        // A ledge on the composer's top edge, like the queue: one line, the ways on at its end.
        let answer = conversation
            .turns
            .last()
            .map_or("", |turn| turn.answer.as_str());
        let headline = match reset_time(answer).filter(|_| is_limit_error(&conversation.last_error))
        {
            Some(at) => tf("Esta conta atingiu o limite de uso. Volta às {}.", &[&at]),
            None => headline,
        };
        let warm: gpui::Hsla = rgb(0xee8a96).into();
        Some(
            div()
                .mx(px(8.))
                .min_h(px(36.))
                .pl(px(10.))
                .pr(px(6.))
                .py(px(4.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded_t(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_b_0()
                .border_color(gpui::Hsla { a: 0.22, ..warm })
                .bg(gpui::Hsla { a: 0.07, ..warm })
                .child(icon("alert-circle", 14.).text_color(warm))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(content(0.75))
                        .child(headline),
                )
                .children(moves)
                .child(retry)
                .into_any_element(),
        )
    }

    fn continue_after_failure(&mut self, session: &str, approval: &str) {
        let call = self.backend.invoke(
            "send_prompt",
            json!({ "request": {
                "sessionId": session,
                "prompt": t(CONTINUE),
                "clientRequestId": crate::panel_sidechat::uuid(),
                "execution": { "approval": approval },
            } }),
        );
        self.waiting.insert(call, Waiting::FailureContinue);
    }

    fn move_after_failure(&mut self, session: &str, account: &str, approval: &str) {
        let call = self.backend.invoke(
            "move_session_account",
            json!({ "request": { "sessionId": session, "accountId": account } }),
        );
        self.waiting.insert(
            call,
            Waiting::AccountMoved(session.to_owned(), approval.to_owned()),
        );
    }

    /// The move finished: the conversation is on the other account, so carry on there.
    pub(super) fn account_moved(&mut self, session: &str, approval: &str) {
        self.continue_after_failure(session, approval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_failed_session_with_an_error_gets_a_card() {
        assert!(headline("idle", "boom").is_none());
        assert!(headline("failed", "  ").is_none());
        assert_eq!(
            headline("failed", "disk full\nmore").as_deref(),
            Some("disk full")
        );
    }

    #[test]
    fn the_reset_time_reads_as_a_clock() {
        assert_eq!(
            reset_time("You've hit your 5-hour limit · resets 2:40pm (America/Sao_Paulo)")
                .as_deref(),
            Some("14:40")
        );
        assert_eq!(reset_time("resets 12am").as_deref(), Some("0:00"));
        assert_eq!(reset_time("nothing here"), None);
    }

    #[test]
    fn a_usage_limit_is_recognised_and_said_plainly() {
        assert!(is_limit_error("Usage limit reached."));
        assert!(is_limit_error("You've hit your 5-hour limit"));
        assert!(!is_limit_error("disk full"));
        assert_eq!(
            headline("failed", "Usage limit reached.").as_deref(),
            Some("Esta conta atingiu o limite de uso.")
        );
    }

    #[test]
    fn a_sign_in_failure_is_recognised() {
        assert!(is_auth_error(
            "Failed to authenticate: OAuth session expired and could not be refreshed"
        ));
        assert!(!is_auth_error("The model is overloaded"));
    }
}
