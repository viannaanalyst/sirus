//! Tasks a reply delegated to other models (ADR-111), under the reply itself: who runs each one, its
//! state, a Stop while it works, and a click opens it. This is DelegatedTasks.tsx, shown where
//! ReplyExtras.tsx puts it, with the same rule for what counts as delegated.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;

/// The states a delegated task still works in; its Stop shows only then (DelegatedTasks.tsx).
pub(crate) fn active(status: &str) -> bool {
    matches!(status, "starting" | "running" | "waiting")
}

/// The state's word under the task's title (`delegate.state.*`). Done is also the fallback.
pub(crate) fn state_label(status: &str) -> &'static str {
    match status {
        "starting" => t("Iniciando"),
        "running" => t("Trabalhando"),
        "waiting" => t("Precisa de você"),
        "failed" => t("Falhou"),
        "stopped" => t("Parada"),
        _ => t("Concluída"),
    }
}

/// The launched sessions that notify their reply, in the order the reply launched them.
pub(crate) fn delegated_ids(launched: &[String], notifies: impl Fn(&str) -> bool) -> Vec<String> {
    launched.iter().filter(|id| notifies(id)).cloned().collect()
}

/// "Delegada para Codex · gpt-5 · Trabalhando", the line under a task's title.
pub(crate) fn subtitle(provider: &str, model: Option<&str>, status: &str) -> String {
    let mut line = tf("Delegada para {}", &[&provider]);
    if let Some(model) = model.filter(|model| !model.is_empty()) {
        line.push_str(" · ");
        line.push_str(model);
    }
    line.push_str(" · ");
    line.push_str(state_label(status));
    line
}

/// The `stop_agent` call's arguments for one task.
pub(crate) fn stop_args(session_id: &str) -> Value {
    json!({ "sessionId": session_id })
}

/// The dot that tells a task's state at a glance.
fn state_color(status: &str) -> gpui::Hsla {
    match status {
        "running" | "starting" => accent(),
        "waiting" => gpui::rgb(0xe0b25a).into(),
        "failed" => gpui::rgb(0xe08a8a).into(),
        _ => content(0.35),
    }
}

impl Shell {
    /// The cards of the tasks a turn delegated, or none when it delegated none.
    pub(super) fn delegated_tasks_view(
        &self,
        turn: &Turn,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ids = delegated_ids(&turn.launched, |id| {
            self.conversations
                .iter()
                .any(|c| c.session_id == id && c.delegated_notify)
        });
        let tasks: Vec<usize> = ids
            .iter()
            .filter_map(|id| self.conversations.iter().position(|c| c.session_id == *id))
            .collect();
        if tasks.is_empty() {
            return None;
        }
        let track = crate::hover_track::list("delegated-tasks");
        let rows = tasks.into_iter().map(|ix| {
            let conversation = &self.conversations[ix];
            let status = conversation.status.clone();
            let session = conversation.session_id.clone();
            let provider = provider_title(conversation.provider);
            let open = div()
                .id(format!("delegated-open-{ix}"))
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .gap(px(10.))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.open(ix, cx)))
                .child(provider_glyph(conversation.provider, 16.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(13.))
                                .text_color(content(0.9))
                                .child(conversation.title.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(content(0.5))
                                .child(subtitle(provider, conversation.model.as_deref(), &status)),
                        ),
                )
                .child(
                    div()
                        .size(px(7.))
                        .rounded_full()
                        .flex_shrink_0()
                        .bg(state_color(&status)),
                );
            let stop = active(&status).then(|| {
                let stop_id = session.clone();
                div()
                    .id(format!("delegated-stop-{ix}"))
                    .size(px(22.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.backend
                            .invoke("stop_agent", stop_args(&stop_id));
                        cx.notify();
                    }))
                    .child(icon("square-fill", 10.).text_color(content(0.6)))
            });
            track.row_named(
                div()
                    .w_full()
                    .h(px(46.))
                    .px(px(10.))
                    .rounded(px(crate::theme::RADIUS_LG))
                    .border_1()
                    .border_color(content(0.07))
                    .bg(content(0.03))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(open)
                    .children(stop)
                    .child(icon("chevron-right", 13.).text_color(content(0.4))),
                &session,
                false,
            )
        });
        Some(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(track.pill())
                .children(rows)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_notifying_sessions_are_delegated_and_keep_their_order() {
        let launched = vec!["b".to_owned(), "a".to_owned(), "c".to_owned()];
        let notifies = |id: &str| id != "a";
        assert_eq!(delegated_ids(&launched, notifies), ["b", "c"]);
        assert!(delegated_ids(&launched, |_| false).is_empty());
    }

    #[test]
    fn working_states_are_the_ones_with_a_stop() {
        for status in ["starting", "running", "waiting"] {
            assert!(active(status), "{status}");
        }
        for status in ["idle", "completed", "failed", "stopped"] {
            assert!(!active(status), "{status}");
        }
    }

    #[test]
    fn states_read_as_their_words_and_unknown_ones_read_as_done() {
        assert_eq!(state_label("running"), t("Trabalhando"));
        assert_eq!(state_label("waiting"), t("Precisa de você"));
        assert_eq!(state_label("failed"), t("Falhou"));
        assert_eq!(state_label("stopped"), t("Parada"));
        assert_eq!(state_label("completed"), t("Concluída"));
        assert_eq!(state_label("something-new"), t("Concluída"));
    }

    #[test]
    fn the_line_names_provider_model_and_state() {
        let with_model = subtitle("Codex", Some("gpt-5"), "running");
        assert_eq!(with_model, format!("{} · gpt-5 · {}", tf("Delegada para {}", &[&"Codex"]), t("Trabalhando")));
        let without = subtitle("Codex", Some(""), "idle");
        assert_eq!(without, format!("{} · {}", tf("Delegada para {}", &[&"Codex"]), t("Concluída")));
        assert_eq!(subtitle("Codex", None, "idle"), without);
    }

    #[test]
    fn stop_sends_only_the_session_id() {
        assert_eq!(stop_args("s-1"), json!({ "sessionId": "s-1" }));
    }
}
