//! Fork and handoff from a finished turn, with the handoff card above the composer and the
//! divider where another provider took over. This is MessageActions.tsx, HandoffCard.tsx,
//! HandoffMarker.tsx and the store's forkSession, handoffSession and dismissHandoff, with the same
//! lists and rules. The host's refusal is shown as the host wrote it.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state. The
//! actions sit in the turn's row (message_actions.rs); the handoff popup opens above its button,
//! animates in, and closes on a click outside it or a second click on the action.

use super::message_actions::settle;
use super::*;

/// The popup open over one turn: the session and reply it acts on, and where it sits.
pub(crate) struct Popup {
    pub session: String,
    pub reply: String,
    /// Pixels from the left edge and from the bottom of the window.
    pub left: f32,
    pub bottom: f32,
    pub open: Tween,
}

/// The window's state for these actions: the open popup, the provider column's last choice, and
/// whether a handoff is on its way to the host.
#[derive(Default)]
pub(crate) struct State {
    pub popup: Option<Popup>,
    pub scope: Option<&'static str>,
    pub busy: bool,
}

/// Which of the actions a turn's row shows: they need a settled reply with an id, and the handoff
/// needs text to hand over (MessageActions.tsx).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Shown {
    pub fork: bool,
    pub handoff: bool,
}

pub(crate) fn shown(finished: bool, answered: bool, has_reply: bool) -> Shown {
    let settled = finished && has_reply;
    Shown {
        fork: settled,
        handoff: settled && answered,
    }
}

/// The providers a handoff can go to: the enabled ones but the session's own.
pub(crate) fn handoff_targets(enabled: &[&'static str], current: &str) -> Vec<&'static str> {
    enabled
        .iter()
        .copied()
        .filter(|id| *id != current)
        .collect()
}

/// Which provider the popup opens on: the one kept from last time while it is still offered,
/// else the first target.
pub(crate) fn first_scope(
    targets: &[&'static str],
    kept: Option<&'static str>,
) -> Option<&'static str> {
    kept.filter(|kept| targets.contains(kept))
        .or_else(|| targets.first().copied())
}

/// The divider between two providers, when the reply's provider differs from the one before.
pub(crate) fn switched(
    previous: Option<&'static str>,
    provider: Option<&'static str>,
) -> Option<(&'static str, &'static str)> {
    match (previous, provider) {
        (Some(from), Some(to)) if from != to => Some((from, to)),
        _ => None,
    }
}

/// The name of a provider as the menus show it.
fn name_of(provider: &str) -> &'static str {
    provider_title(provider)
}

impl Shell {
    /// Which of the fork and handoff actions may run now in a conversation: it is idle, no
    /// handoff is under way, and the popup has a provider to offer.
    pub(super) fn handoff_can(&self, conversation: usize) -> super::message_actions::Can {
        let conv = &self.conversations[conversation];
        let enabled = self.enabled_providers();
        let free = !conv.working() && !self.handoffs.busy;
        super::message_actions::Can {
            fork: !conv.working(),
            handoff: free && !handoff_targets(&enabled, conv.provider).is_empty(),
        }
    }

    /// The fork of the newest reply of a conversation, from the sidebar's menu.
    pub(super) fn can_fork_latest(&self, ix: usize) -> bool {
        let conv = &self.conversations[ix];
        !conv.working() && conv.turns.iter().any(|turn| turn.reply.is_some())
    }

    pub(super) fn fork_latest(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.can_fork_latest(ix) {
            return;
        }
        let conv = &self.conversations[ix];
        let Some(reply) = conv.turns.iter().rev().find_map(|turn| turn.reply.clone()) else {
            return;
        };
        let session = conv.session_id.clone();
        self.fork_turn(&session, &reply, cx);
    }

    /// Creates the fork of a reply and opens it in a new tab (store: forkSession).
    pub(super) fn fork_turn(&mut self, session: &str, reply: &str, cx: &mut Context<Self>) {
        let rx = self.backend.request(
            "fork_session",
            json!({ "sessionId": session, "messageId": reply }),
        );
        self.adopt(rx, true, cx);
    }

    /// Dismisses the pending handoff card (store: dismissHandoff).
    pub(super) fn dismiss_handoff(&mut self, session: String, cx: &mut Context<Self>) {
        let rx = self
            .backend
            .request("dismiss_handoff", json!({ "sessionId": session }));
        self.adopt(rx, false, cx);
    }

    /// Takes the Session a host call returns into the window, and opens it in a tab when asked.
    /// A refusal is shown as the host wrote it.
    fn adopt(
        &mut self,
        rx: async_channel::Receiver<Result<Value, Value>>,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.handoffs.busy = false;
                match result {
                    Ok(session) => {
                        let id = session["id"].as_str().unwrap_or("").to_owned();
                        this.session_updated(&session, cx);
                        if open {
                            if let Some(ix) =
                                this.conversations.iter().position(|c| c.session_id == id)
                            {
                                this.open_beside(ix, super::split_panes::Edge::Right, cx);
                            }
                        }
                    }
                    Err(problem) => this.problem = Some(problem),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens or closes the popup of one button. The popup starts on the provider it last showed.
    pub(super) fn open_popup(
        &mut self,
        session: &str,
        reply: &str,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self
            .handoffs
            .popup
            .as_ref()
            .is_some_and(|open| open.reply == reply)
        {
            self.handoffs.popup = None;
            cx.notify();
            return;
        }
        let Some(ix) = self
            .conversations
            .iter()
            .position(|c| c.session_id == session)
        else {
            return;
        };
        let provider = self.conversations[ix].provider;
        let enabled = self.enabled_providers();
        let targets = handoff_targets(&enabled, provider);
        let scope = first_scope(&targets, self.handoffs.scope);
        self.handoffs.scope = scope;
        if let Some(scope) = scope {
            self.load_models(scope);
        }
        let x = f32::from(at.x);
        let y = f32::from(at.y);
        self.handoffs.popup = Some(Popup {
            session: session.to_owned(),
            reply: reply.to_owned(),
            left: (x - 12.).max(8.),
            bottom: (self.viewport_height - y + 6.).max(0.),
            open: Tween::from_to(
                0.,
                1.,
                Instant::now(),
                if cx.reduce_motion() { 0. } else { 0.16 },
                Curve::Expo,
            ),
        });
        cx.notify();
    }

    /// Asks the host for a provider's catalog unless it is already here (store: loadProviderModels).
    fn load_models(&mut self, provider: &'static str) {
        if self.models.contains_key(provider) {
            return;
        }
        let id = self
            .backend
            .invoke("list_provider_models", json!({ "id": provider }));
        self.waiting.insert(id, Waiting::Models(provider));
    }

    fn browse(&mut self, provider: &'static str, cx: &mut Context<Self>) {
        self.handoffs.scope = Some(provider);
        self.load_models(provider);
        cx.notify();
    }

    /// The model the popup chose: the handoff goes out and the popup closes.
    fn pick_target(
        &mut self,
        provider: &'static str,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.handoffs.busy {
            return;
        }
        let Some(popup) = self.handoffs.popup.take() else {
            return;
        };
        self.handoffs.scope = Some(provider);
        self.handoffs.busy = true;
        self.start_handoff(&popup.session, &popup.reply, provider, model, cx);
        cx.notify();
    }

    /// Hands the conversation to another provider in the same folder (store: handoffSession).
    fn start_handoff(
        &mut self,
        session: &str,
        reply: &str,
        provider: &'static str,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let rx = self.backend.request(
            "handoff_session",
            json!({
                "sessionId": session,
                "messageId": reply,
                "agent": provider,
                "model": model,
                "newWorktree": false,
            }),
        );
        self.adopt(rx, true, cx);
    }

    /// The popup over the turn's row: a provider column and the models of the chosen provider.
    pub(super) fn handoff_popup_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let popup = self.handoffs.popup.as_ref()?;
        if !popup.open.done(now) {
            *moving = true;
        }
        let shown = popup.open.at(now).clamp(0., 1.);
        let ix = self
            .conversations
            .iter()
            .position(|c| c.session_id == popup.session)?;
        let conv = &self.conversations[ix];
        let enabled = self.enabled_providers();
        let targets = handoff_targets(&enabled, conv.provider);
        let scope = self.handoffs.scope.filter(|scope| targets.contains(scope));
        let hint = t(
            "Passa a conversa a outro provedor nesta pasta. A recapitulação vai com a primeira mensagem.",
        );
        let providers: Vec<AnyElement> = targets
            .iter()
            .enumerate()
            .map(|(i, &provider)| {
                let active = scope == Some(provider);
                div()
                    .id(("handoff-provider", i))
                    .size(px(28.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(active, |d| d.bg(content(0.08)))
                    .hover(|s| s.bg(content(0.07)))
                    .on_click(cx.listener(move |this, _, _, cx| this.browse(provider, cx)))
                    .child(crate::tab_tile::provider_mark(provider, 18.))
                    .into_any_element()
            })
            .collect();
        let row = |id: (&'static str, usize)| {
            div()
                .id(id)
                .min_h(px(36.))
                .w_full()
                .px(px(8.))
                .py(px(8.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)))
        };
        let mut models: Vec<AnyElement> = Vec::new();
        let mut loading = false;
        if let Some(scope) = scope {
            loading = !self.models.contains_key(scope);
            if let Some(list) = self.models.get(scope) {
                for (i, model) in list.iter().enumerate() {
                    let picked =
                        conv.provider == scope && conv.model.as_deref() == Some(model.id.as_str());
                    let id = model.id.clone();
                    let name = self.model_display(scope, Some(&model.id));
                    models.push(
                        row(("handoff-model", i))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(content(0.92))
                                    .child(name),
                            )
                            .when(picked, |d| {
                                d.child(icon("check", 13.).text_color(content(0.6)))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pick_target(scope, Some(id.clone()), cx)
                            }))
                            .into_any_element(),
                    );
                }
            }
            if models.is_empty() && !loading {
                models.push(
                    row(("handoff-default", 0))
                        .child(crate::tab_tile::provider_mark(scope, 16.))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_size(px(13.))
                                .text_color(content(0.92))
                                .child(t("Usar o padrão do provedor")),
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.pick_target(scope, None, cx)),
                        )
                        .into_any_element(),
                );
            }
        }
        if loading {
            models.clear();
            models.push(
                div()
                    .px(px(8.))
                    .py(px(12.))
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child(t("Carregando…"))
                    .into_any_element(),
            );
        }
        let width = 300.;
        let left = popup.left.min(self.viewport_width - width - 8.).max(8.);
        let bottom = popup.bottom + (1. - shown) * -4.;
        let panel = div()
            .id("handoff-popup")
            .occlude()
            .absolute()
            .left(px(left))
            .bottom(px(bottom))
            .w(px(width))
            .flex()
            .flex_col()
            .opacity(shown)
            .rounded(px(crate::theme::RADIUS_LG))
            .border_1()
            .border_color(content(0.12))
            .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px(px(12.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(content(0.08))
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child(hint),
            )
            .child(
                div()
                    .h(px(260.))
                    .flex()
                    .child(
                        div()
                            .w(px(40.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(8.))
                            .py(px(8.))
                            .border_r_1()
                            .border_color(content(0.08))
                            .children(providers),
                    )
                    .child(
                        div()
                            .id("handoff-models")
                            .min_w_0()
                            .flex_1()
                            .p(px(6.))
                            .overflow_y_scroll()
                            .children(models),
                    ),
            );
        // A click outside the panel closes it; the panel itself keeps its clicks.
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.handoffs.popup = None;
                        cx.notify();
                    }),
                )
                .child(panel)
                .into_any_element(),
        )
    }

    /// The pending handoff card above the composer (HandoffCard.tsx).
    pub(super) fn handoff_card(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let conv = &self.conversations[ix];
        let handoff = conv.handoff.as_ref().filter(|handoff| handoff.pending)?;
        let session = conv.session_id.clone();
        let (from, to) = (handoff.from, conv.provider);
        // A shelf on the composer's top edge.
        Some(
            div()
                .mx(px(14.))
                .pl(px(12.))
                .pr(px(6.))
                .py(px(4.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded_t(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_b_0()
                .border_color(content(0.08))
                .bg(content(0.03))
                .child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.5))
                        .text_color(content(0.85))
                        .child(crate::tab_tile::provider_mark(from, 14.))
                        .child(name_of(from))
                        .child(icon("chevron-right", 11.).text_color(content(0.45)))
                        .child(crate::tab_tile::provider_mark(to, 14.))
                        .child(name_of(to)),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child(t("Leva o contexto ao enviar")),
                )
                .child(
                    div()
                        .id("handoff-dismiss")
                        .size(px(22.))
                        .flex_shrink_0()
                        .rounded(px(crate::theme::RADIUS_SM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(content(0.5))
                        .hover(|s| s.bg(content(0.07)).text_color(content(0.9)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dismiss_handoff(session.clone(), cx)
                        }))
                        .child(icon("close", 13.)),
                )
                .into_any_element(),
        )
    }

    /// One turn in the transcript, with the divider above it where another provider took over.
    pub(super) fn turn_view(
        &self,
        conversation: usize,
        index: usize,
        turn: &Turn,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // ⌘F marks the words while this turn is drawn.
        crate::markdown::set_search(
            self.find
                .query()
                .map(|query| (query, self.find.current() == Some(index))),
        );
        let body = self.turn_body(conversation, index, turn, now, moving, cx);
        crate::markdown::set_search(None);
        let conv = &self.conversations[conversation];
        let previous = match index {
            0 => conv.handoff.as_ref().map(|handoff| handoff.from),
            _ => conv.turns.get(index - 1).and_then(|before| before.provider),
        };
        match switched(previous, turn.provider) {
            Some((from, to)) => div()
                .flex()
                .flex_col()
                .child(handoff_divider(from, to))
                .child(body)
                .into_any_element(),
            None => body,
        }
    }
}

/// The thin divider where another provider answered (HandoffMarker.tsx).
fn handoff_divider(from: &'static str, to: &'static str) -> AnyElement {
    let rule = || div().h(px(1.)).flex_1().bg(content(0.1));
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .my(px(8.))
        .text_size(px(12.))
        .text_color(content(0.5))
        .child(rule())
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .py(px(2.))
                .rounded_full()
                .border_1()
                .border_color(content(0.12))
                .bg(content(0.03))
                .child(crate::tab_tile::provider_mark(from, 13.))
                .child(icon("chevron-right", 11.).text_color(content(0.5)))
                .child(crate::tab_tile::provider_mark(to, 13.))
                .child(tf("{} assumiu", &[&name_of(to)])),
        )
        .child(rule())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENABLED: [&str; 3] = ["claude", "codex", "cursor"];

    #[test]
    fn a_live_turn_shows_no_action() {
        assert_eq!(
            shown(false, true, true),
            Shown {
                fork: false,
                handoff: false,
            }
        );
    }

    #[test]
    fn a_turn_without_a_reply_id_shows_no_action() {
        assert_eq!(
            shown(true, true, false),
            Shown {
                fork: false,
                handoff: false,
            }
        );
    }

    #[test]
    fn a_settled_answer_offers_both() {
        assert_eq!(
            shown(true, true, true),
            Shown {
                fork: true,
                handoff: true,
            }
        );
    }

    #[test]
    fn a_settled_turn_without_text_can_only_fork() {
        assert_eq!(
            shown(true, false, true),
            Shown {
                fork: true,
                handoff: false,
            }
        );
    }

    #[test]
    fn handoff_leaves_out_the_sessions_own_provider() {
        assert_eq!(handoff_targets(&ENABLED, "codex"), ["claude", "cursor"]);
        assert!(handoff_targets(&["claude"], "claude").is_empty());
    }

    #[test]
    fn the_popup_keeps_its_provider_while_offered() {
        let targets = ["claude", "codex"];
        assert_eq!(first_scope(&targets, Some("codex")), Some("codex"));
        assert_eq!(first_scope(&targets, Some("grok")), Some("claude"));
        assert_eq!(first_scope(&[], None), None);
    }

    #[test]
    fn a_divider_shows_only_when_the_provider_changes() {
        assert_eq!(
            switched(Some("claude"), Some("codex")),
            Some(("claude", "codex"))
        );
        assert_eq!(switched(Some("claude"), Some("claude")), None);
        assert_eq!(switched(None, Some("codex")), None);
        assert_eq!(switched(Some("claude"), None), None);
    }
}
