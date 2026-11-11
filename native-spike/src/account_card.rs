//! The card above the composer when a turn ended on an error (`lastError` of the session): what
//! went wrong, in the person's words, and a way on. Until now the window showed an empty reply
//! ("trabalhou por 13s") and hid the host's message. A sign-in failure on a shared credential
//! ("OAuth session expired") is the common case: Retry sends "continue" again (the other session
//! that refreshed the token may have finished), and each other account of the provider
//! continues the conversation there through `move_session_account`, which carries the native
//! transcript along (account_move.rs). A usage limit also offers the next provider of the
//! fallback order (Settings → Provedores, ADR-198): `set_session_model` switches the conversation
//! in place, which arms the handoff recap, and "continue" goes out there; with the automatic
//! switch on, that happens as soon as the turn stops. This module is a child of `shell` (`#[path]`).

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

/// A provider's name in a sentence: "Claude", not "Claude Code".
fn short_name(provider: &str) -> &'static str {
    match provider {
        "claude" => "Claude",
        other => provider_title(other),
    }
}

/// A usage reply with a window spent until a reset still ahead (T3 Code's `exhaustedUsageWindow`).
pub(crate) fn exhausted(usage: &Value, now: i64) -> bool {
    usage["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|window| {
            window["usedPercent"]
                .as_f64()
                .is_some_and(|used| used >= 100.)
                && window["resetsAt"].as_i64().is_none_or(|at| {
                    let at = if at > 100_000_000_000 { at / 1000 } else { at };
                    at > now
                })
        })
}

/// Where a conversation stopped by a limit goes: the first provider of the order that is not its
/// own, not one it already left on a limit, and not known to be spent too.
pub(crate) fn fallback_provider(
    order: &[&'static str],
    current: &str,
    left: &[&'static str],
    spent: impl Fn(&str) -> bool,
) -> Option<&'static str> {
    order
        .iter()
        .copied()
        .find(|id| *id != current && !left.contains(id) && !spent(id))
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
        let fallback = is_limit_error(&conversation.last_error)
            .then(|| self.limit_fallback(ix))
            .flatten();
        let headline = match (&fallback, reset_time(answer)) {
            (Some((provider, model)), _) => {
                let target = match model {
                    Some(model) => format!(
                        "{} ({})",
                        short_name(provider),
                        self.model_display(provider, Some(model))
                    ),
                    None => short_name(provider).to_owned(),
                };
                tf(
                    "Limite do {} atingido — continuar com {}?",
                    &[&short_name(conversation.provider), &target],
                )
            }
            (None, Some(at)) if is_limit_error(&conversation.last_error) => {
                tf("Esta conta atingiu o limite de uso. Volta às {}.", &[&at])
            }
            _ => headline,
        };
        let fallback = fallback.map(|(provider, model)| {
            button(
                "failure-fallback".into(),
                tf("Continuar com {}", &[&short_name(provider)]),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.switch_after_limit(ix, provider, model.clone());
                cx.notify();
            }))
        });
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
                .relative()
                .border_color(gpui::Hsla { a: 0.22, ..warm })
                // The composer's glass with the warning's warm wash over it.
                .child(crate::theme::composer_ledge_surface(
                    crate::theme::RADIUS_MD,
                ))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .rounded_t(px(crate::theme::RADIUS_MD))
                        .bg(gpui::Hsla { a: 0.07, ..warm }),
                )
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
                .children(fallback)
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

    /// Picking an account in the usage popup also moves the open conversation onto it, so a
    /// conversation stopped by one account's limit carries on with the one just picked, and a
    /// fresh one in a project starts on the project's account. Only Claude and Codex have
    /// per-account conversations, and a working one is left as it is.
    pub(super) fn switch_conversation_account(&mut self, provider: &'static str, account: String) {
        let ix = self.active_conversation();
        let Some(conversation) = self.conversations.get(ix) else {
            return;
        };
        if conversation.provider != provider
            || !matches!(provider, "claude" | "codex")
            || conversation.working()
            || conversation.account == account
            || conversation.session_id.is_empty()
        {
            return;
        }
        let session = conversation.session_id.clone();
        let call = self.backend.invoke(
            "move_session_account",
            json!({ "request": { "sessionId": session, "accountId": account } }),
        );
        self.waiting
            .insert(call, Waiting::AccountSwitched(session, account));
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

    /// The provider and model a conversation stopped by its usage limit would carry on with.
    pub(super) fn limit_fallback(&self, ix: usize) -> Option<(&'static str, Option<String>)> {
        let conversation = self.conversations.get(ix)?;
        let order =
            crate::settings_pages::limit_fallback_order(&self.settings, &self.enabled_providers());
        let left = self
            .limit_left
            .get(&conversation.session_id)
            .map_or(&[][..], Vec::as_slice);
        let now = chrono::Utc::now().timestamp();
        let provider = fallback_provider(&order, conversation.provider, left, |id| {
            self.bar_values
                .get(id)
                .is_some_and(|usage| exhausted(usage, now))
        })?;
        Some((provider, self.fallback_model(provider)))
    }

    /// The model a fallback uses: the provider's first favorite, else the first model of its
    /// catalog, leaving out the ones turned off; none leaves the provider's own default.
    fn fallback_model(&self, provider: &str) -> Option<String> {
        let list = self.models.get(provider)?;
        let off = self.settings["disabledModels"].as_array();
        let usable = |id: &str| {
            let key = format!("{provider}::{id}");
            list.iter().any(|m| m.id == id && m.available)
                && !off.is_some_and(|off| off.iter().any(|item| item.as_str() == Some(&key)))
        };
        let favorite = self
            .favorite_models()
            .into_iter()
            .filter_map(|key| Some(key.strip_prefix(provider)?.strip_prefix("::")?.to_owned()))
            .find(|id| usable(id));
        favorite.or_else(|| list.iter().find(|m| usable(&m.id)).map(|m| m.id.clone()))
    }

    /// Moves a conversation stopped by its usage limit to `provider` and carries on there: the
    /// switch arms the handoff recap, so the new provider gets the conversation's context.
    fn switch_after_limit(&mut self, ix: usize, provider: &'static str, model: Option<String>) {
        let Some(conversation) = self.conversations.get_mut(ix) else {
            return;
        };
        let (session, approval) = (
            conversation.session_id.clone(),
            conversation.approval.clone(),
        );
        self.limit_left
            .entry(session.clone())
            .or_default()
            .push(conversation.provider);
        conversation.provider = provider;
        conversation.model = model.clone();
        let call = self.backend.invoke(
            "set_session_model",
            json!({ "sessionId": session, "agent": provider, "model": model }),
        );
        self.waiting
            .insert(call, Waiting::LimitFallback(session, approval));
    }

    /// A turn of conversation `ix` just ended: when it stopped on the usage limit and the
    /// automatic switch is on, it carries on with the next provider of the order.
    pub(super) fn limit_fallback_auto(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(conversation) = self.conversations.get(ix) else {
            return;
        };
        if conversation.status != "failed"
            || !is_limit_error(&conversation.last_error)
            || self.settings["limitFallbackAuto"].as_bool() != Some(true)
        {
            return;
        }
        if let Some((provider, model)) = self.limit_fallback(ix) {
            self.switch_after_limit(ix, provider, model);
            cx.notify();
        }
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
    fn a_fallback_skips_the_own_provider_the_ones_left_and_the_spent_ones() {
        let order = ["claude", "codex", "cursor", "opencode"];
        assert_eq!(
            fallback_provider(&order, "claude", &[], |_| false),
            Some("codex")
        );
        assert_eq!(
            fallback_provider(&order, "codex", &["claude"], |id| id == "cursor"),
            Some("opencode")
        );
        assert_eq!(
            fallback_provider(&order, "codex", &["claude", "cursor", "opencode"], |_| {
                false
            }),
            None
        );
    }

    #[test]
    fn a_window_counts_as_spent_only_until_it_resets() {
        let now = 1_790_000_000;
        assert!(exhausted(
            &json!({"windows":[{"usedPercent":100.0,"resetsAt":now + 60}]}),
            now
        ));
        assert!(exhausted(&json!({"windows":[{"usedPercent":100.0}]}), now));
        assert!(!exhausted(
            &json!({"windows":[{"usedPercent":100.0,"resetsAt":(now - 60) * 1000}]}),
            now
        ));
        assert!(!exhausted(&json!({"windows":[{"usedPercent":99.0}]}), now));
    }

    #[test]
    fn a_sign_in_failure_is_recognised() {
        assert!(is_auth_error(
            "Failed to authenticate: OAuth session expired and could not be refreshed"
        ));
        assert!(!is_auth_error("The model is overloaded"));
    }
}
