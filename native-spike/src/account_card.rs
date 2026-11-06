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

/// The error in one short line for the card, or None when the session did not fail.
pub(crate) fn headline(status: &str, error: &str) -> Option<String> {
    if status != "failed" || error.trim().is_empty() {
        return None;
    }
    Some(if is_auth_error(error) {
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
        let others: Vec<(String, String)> = if is_auth_error(&conversation.last_error) {
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
                .h(px(28.))
                .px(px(10.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .text_size(px(12.))
                .text_color(content(0.8))
                .bg(content(0.08))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.14)))
                .child(label)
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
        Some(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(12.))
                .py(px(10.))
                .rounded(px(crate::theme::RADIUS_LG))
                .bg(content(0.05))
                .border_1()
                .border_color(content(0.08))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(0xee8a96))
                        .child(t("Esta conversa parou por um erro")),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(18.))
                        .text_color(content(0.65))
                        .child(headline),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .child(retry)
                        .children(moves),
                )
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
    fn a_sign_in_failure_is_recognised() {
        assert!(is_auth_error(
            "Failed to authenticate: OAuth session expired and could not be refreshed"
        ));
        assert!(!is_auth_error("The model is overloaded"));
    }
}
