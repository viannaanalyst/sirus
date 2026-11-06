//! Fork, handoff and second opinion from a finished turn, with the handoff card above the composer
//! and the divider where another provider took over. This is MessageActions.tsx, HandoffCard.tsx,
//! HandoffMarker.tsx and the store's forkSession, handoffSession, secondOpinion and dismissHandoff,
//! with the same lists and rules. The host's refusal is shown as the host wrote it.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state. The
//! actions sit in the turn's `⋯` popup (message_actions.rs); the handoff and second opinion
//! popups open above that `⋯` button, animate in, and close on a click outside them or a second
//! click on the same action.

use super::message_actions::settle;
use super::*;

/// Files a second opinion's prompt names, at most (second-opinion.ts).
const FILE_LIMIT: usize = 40;
/// The longest request and answer the prompt quotes (second-opinion.ts).
const REQUEST_LIMIT: usize = 600;
const ANSWER_LIMIT: usize = 1600;
/// The longest title a second opinion's session takes (the store).
const TITLE_LIMIT: usize = 200;

/// The two popups: a handoff moves the conversation, a second opinion starts a reviewing one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Handoff,
    Opinion,
}

/// The popup open over one turn: its kind, the session and reply it acts on, and where it sits.
pub(crate) struct Popup {
    pub kind: Kind,
    pub session: String,
    pub reply: String,
    /// Pixels from the left edge and from the bottom of the window.
    pub left: f32,
    pub bottom: f32,
    pub open: Tween,
}

/// The window's state for these actions: the open popup, the provider column's last choice, and
/// whether a handoff or second opinion is on its way to the host.
#[derive(Default)]
pub(crate) struct State {
    pub popup: Option<Popup>,
    pub scope: Option<&'static str>,
    pub busy: bool,
}

/// Which of the three actions a turn's row shows: they need a settled reply with an id, and the
/// two popups need text to hand over (MessageActions.tsx).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Shown {
    pub fork: bool,
    pub handoff: bool,
    pub opinion: bool,
}

pub(crate) fn shown(finished: bool, answered: bool, has_reply: bool) -> Shown {
    let settled = finished && has_reply;
    Shown {
        fork: settled,
        handoff: settled && answered,
        opinion: settled && answered,
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

/// The providers a second opinion can go to: every enabled one, the session's own included,
/// because the same provider is allowed with another model.
pub(crate) fn opinion_targets(enabled: &[&'static str]) -> Vec<&'static str> {
    enabled.to_vec()
}

/// Whether a provider's model is listed. A second opinion never repeats the session's own model.
pub(crate) fn offers_model(
    kind: Kind,
    scope: &str,
    model: &str,
    current: Option<(&str, &str)>,
) -> bool {
    kind == Kind::Handoff || current != Some((scope, model))
}

/// Which provider the popup opens on: the one kept from last time while it is still offered;
/// else the handoff's first target, or a second opinion's first provider other than the session's.
pub(crate) fn first_scope(
    kind: Kind,
    targets: &[&'static str],
    current: &str,
    kept: Option<&'static str>,
) -> Option<&'static str> {
    if let Some(kept) = kept.filter(|kept| targets.contains(kept)) {
        return Some(kept);
    }
    match kind {
        Kind::Handoff => targets.first().copied(),
        Kind::Opinion => targets
            .iter()
            .copied()
            .find(|id| *id != current)
            .or_else(|| targets.first().copied()),
    }
}

/// Whether a second opinion's send turns planning on (store: supportsPlanning).
pub(crate) fn plans(provider: &str, model: Option<&str>) -> bool {
    matches!(
        provider,
        "claude" | "cursor" | "opencode" | "pi" | "droid" | "devin" | "hermes"
    ) || (provider == "codex" && model.is_some())
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

/// A text cut at `limit` characters, with a marker when it was cut (second-opinion.ts `clip`).
fn clip(text: &str, limit: usize) -> String {
    let clean = text.trim();
    if clean.chars().count() <= limit {
        return clean.to_owned();
    }
    let kept: String = clean.chars().take(limit).collect();
    format!("{}\n[…]", kept.trim_end())
}

/// The review prompt for the provider that takes the second opinion, in the window's language. It
/// reviews the files already on disk and reports; changes wait for the person's next request.
pub(crate) fn opinion_prompt(
    from: &str,
    request: &str,
    answer: &str,
    files: &[String],
    portuguese: bool,
) -> String {
    let listed = if files.is_empty() {
        if portuguese {
            "(nenhum arquivo registrado neste turno)".to_owned()
        } else {
            "(no files recorded on this turn)".to_owned()
        }
    } else {
        files
            .iter()
            .take(FILE_LIMIT)
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let request = clip(request, REQUEST_LIMIT);
    let answer = clip(answer, ANSWER_LIMIT);
    if portuguese {
        format!(
            "Dê uma segunda opinião sobre o trabalho que {from} acabou de fazer nesta mesma pasta. Os arquivos já estão no disco.\n\n\
             Revise o que está errado, o que faltou e o que você faria diferente. Leia os arquivos listados antes de opinar. Não altere nada agora: liste os problemas do mais grave ao menos grave, com arquivo e linha quando possível, e diga como corrigir. Se estiver tudo certo, diga isso.\n\n\
             ## Pedido do usuário\n{}\n\n\
             ## O que {from} respondeu\n{}\n\n\
             ## Arquivos alterados\n{listed}",
            if request.is_empty() {
                "(sem pedido neste turno)".to_owned()
            } else {
                request
            },
            if answer.is_empty() {
                "(sem resumo escrito — inspecione os arquivos)".to_owned()
            } else {
                answer
            },
        )
    } else {
        format!(
            "Give a second opinion on the work {from} just finished in this same working copy. The files are already on disk.\n\n\
             Review what is wrong, what is missing and what you would have done differently. Read the listed files before judging. Do not change anything yet: list the problems from most to least severe, with file and line where possible, and say how to fix them. If it is all fine, say so.\n\n\
             ## User request\n{}\n\n\
             ## What {from} answered\n{}\n\n\
             ## Files changed\n{listed}",
            if request.is_empty() {
                "(no request on this turn)".to_owned()
            } else {
                request
            },
            if answer.is_empty() {
                "(no written summary — inspect the files)".to_owned()
            } else {
                answer
            },
        )
    }
}

/// The name of a provider as the menus show it.
fn name_of(provider: &str) -> &'static str {
    provider_title(provider)
}

impl Shell {
    /// Which of the fork, handoff and second opinion actions may run now in a conversation: it
    /// is idle, no handoff is under way, and the two popups have a provider to offer.
    pub(super) fn handoff_can(&self, conversation: usize) -> super::message_actions::Can {
        let conv = &self.conversations[conversation];
        let enabled = self.enabled_providers();
        let free = !conv.working() && !self.handoffs.busy;
        super::message_actions::Can {
            fork: !conv.working(),
            handoff: free && !handoff_targets(&enabled, conv.provider).is_empty(),
            opinion: free && !enabled.is_empty(),
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
        kind: Kind,
        session: &str,
        reply: &str,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self
            .handoffs
            .popup
            .as_ref()
            .is_some_and(|open| open.kind == kind && open.reply == reply)
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
        let targets = match kind {
            Kind::Handoff => handoff_targets(&enabled, provider),
            Kind::Opinion => opinion_targets(&enabled),
        };
        let scope = first_scope(kind, &targets, provider, self.handoffs.scope);
        self.handoffs.scope = scope;
        if let Some(scope) = scope {
            self.load_models(scope);
        }
        let x = f32::from(at.x);
        let y = f32::from(at.y);
        self.handoffs.popup = Some(Popup {
            kind,
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

    /// The model the popup chose: a handoff or a second opinion goes out and the popup closes.
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
        match popup.kind {
            Kind::Handoff => self.start_handoff(&popup.session, &popup.reply, provider, model, cx),
            Kind::Opinion => self.start_opinion(&popup.session, &popup.reply, provider, model, cx),
        }
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

    /// Starts a reviewing session beside this one (store: secondOpinion): a handoff without its
    /// recap, titled as a second opinion, then the review prompt sent into it.
    fn start_opinion(
        &mut self,
        session: &str,
        reply: &str,
        provider: &'static str,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self
            .conversations
            .iter()
            .position(|c| c.session_id == session)
        else {
            self.handoffs.busy = false;
            return;
        };
        let conv = &self.conversations[ix];
        let Some(turn) = conv
            .turns
            .iter()
            .find(|turn| turn.reply.as_deref() == Some(reply))
        else {
            self.handoffs.busy = false;
            return;
        };
        let portuguese = !crate::i18n::is_en();
        let files: Vec<String> = turn
            .review
            .as_ref()
            .map(|review| review.files.iter().map(|file| file.path.clone()).collect())
            .unwrap_or_default();
        let prompt = opinion_prompt(
            name_of(conv.provider),
            &turn.user,
            &turn.answer,
            &files,
            portuguese,
        );
        let title: String = format!("{} · {}", t("Segunda opinião"), conv.title)
            .chars()
            .take(TITLE_LIMIT)
            .collect();
        let planning = plans(provider, model.as_deref());
        let created = self.backend.request(
            "handoff_session",
            json!({
                "sessionId": session,
                "messageId": reply,
                "agent": provider,
                "model": model,
                "newWorktree": false,
            }),
        );
        cx.spawn(async move |this, cx| {
            let outcome: Result<(Value, Value, Value), String> = async {
                let made = settle(created).await?;
                let id = made["id"].as_str().unwrap_or("").to_owned();
                let Ok(dismiss) = this.update(cx, |this, _| {
                    this.backend
                        .request("dismiss_handoff", json!({ "sessionId": id.clone() }))
                }) else {
                    return Err(String::new());
                };
                let dismissed = settle(dismiss).await?;
                let Ok(rename) = this.update(cx, |this, _| {
                    this.backend.request(
                        "rename_session",
                        json!({ "sessionId": id.clone(), "title": title.clone() }),
                    )
                }) else {
                    return Err(String::new());
                };
                let titled = settle(rename).await?;
                Ok((made, dismissed, titled))
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.handoffs.busy = false;
                match outcome {
                    Ok((made, dismissed, titled)) => {
                        let mut session = made.clone();
                        session["handoff"] = dismissed["handoff"].clone();
                        session["title"] = titled["title"].clone();
                        session["lastActivityAt"] = titled["lastActivityAt"].clone();
                        let id = session["id"].as_str().unwrap_or("").to_owned();
                        this.session_updated(&session, cx);
                        if let Some(ix) = this.conversations.iter().position(|c| c.session_id == id) {
                            this.open_beside(ix, super::split_panes::Edge::Right, cx);
                            let mut request = json!({
                                "sessionId": id,
                                "prompt": prompt,
                                "clientRequestId": uuid(),
                                "attachmentIds": [],
                                "attachmentOwner": format!("session:{}", made["id"].as_str().unwrap_or("")),
                            });
                            if planning {
                                request["execution"] = json!({ "planning": true });
                            }
                            let call = this
                                .backend
                                .invoke("send_prompt", json!({ "request": request }));
                            this.waiting.insert(call, Waiting::Session(ix));
                        }
                    }
                    Err(problem) => {
                        if !problem.is_empty() {
                            this.problem = Some(problem);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
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
        let kind = popup.kind;
        let enabled = self.enabled_providers();
        let targets = match kind {
            Kind::Handoff => handoff_targets(&enabled, conv.provider),
            Kind::Opinion => opinion_targets(&enabled),
        };
        let scope = self.handoffs.scope.filter(|scope| targets.contains(scope));
        let hint = match kind {
            Kind::Handoff => t(
                "Passa a conversa a outro provedor nesta pasta. A recapitulação vai com a primeira mensagem.",
            ),
            Kind::Opinion => t(
                "Envie este turno a outro agente para revisar o trabalho. Abre em outra aba e só aponta problemas.",
            ),
        };
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
                    .child(provider_glyph(provider, 18.))
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
            let current = conv.model.as_deref().map(|model| (conv.provider, model));
            if let Some(list) = self.models.get(scope) {
                for (i, model) in list.iter().enumerate() {
                    if !offers_model(kind, scope, &model.id, current) {
                        continue;
                    }
                    let picked = kind == Kind::Handoff
                        && conv.provider == scope
                        && conv.model.as_deref() == Some(model.id.as_str());
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
                if kind == Kind::Opinion && Some(scope) == Some(conv.provider) {
                    models.push(
                        div()
                            .px(px(8.))
                            .py(px(12.))
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child(t("Nenhum outro modelo deste provedor."))
                            .into_any_element(),
                    );
                } else {
                    models.push(
                        row(("handoff-default", 0))
                            .child(provider_glyph(scope, 16.))
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
                                cx.listener(move |this, _, _, cx| {
                                    this.pick_target(scope, None, cx)
                                }),
                            )
                            .into_any_element(),
                    );
                }
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
        // A shelf on the composer's top edge, like the approvals' in Monocode (composer_shelf.rs).
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
                        .child(provider_glyph(from, 14.))
                        .child(name_of(from))
                        .child(icon("chevron-right", 11.).text_color(content(0.45)))
                        .child(provider_glyph(to, 14.))
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
        let body = self.turn_body(conversation, index, turn, now, moving, cx);
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
                .child(provider_glyph(from, 13.))
                .child(icon("chevron-right", 11.).text_color(content(0.5)))
                .child(provider_glyph(to, 13.))
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
                opinion: false
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
                opinion: false
            }
        );
    }

    #[test]
    fn a_settled_answer_offers_all_three() {
        assert_eq!(
            shown(true, true, true),
            Shown {
                fork: true,
                handoff: true,
                opinion: true
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
                opinion: false
            }
        );
    }

    #[test]
    fn handoff_leaves_out_the_sessions_own_provider() {
        assert_eq!(handoff_targets(&ENABLED, "codex"), ["claude", "cursor"]);
        assert!(handoff_targets(&["claude"], "claude").is_empty());
    }

    #[test]
    fn second_opinion_keeps_the_sessions_own_provider() {
        assert_eq!(opinion_targets(&ENABLED), ENABLED);
    }

    #[test]
    fn handoff_lists_every_model_but_a_second_opinion_drops_the_turns_own() {
        let current = Some(("claude", "opus"));
        assert!(offers_model(Kind::Handoff, "claude", "opus", current));
        assert!(!offers_model(Kind::Opinion, "claude", "opus", current));
        assert!(offers_model(Kind::Opinion, "claude", "sonnet", current));
        assert!(offers_model(Kind::Opinion, "codex", "opus", current));
    }

    #[test]
    fn the_popup_keeps_its_provider_while_offered() {
        let targets = ["claude", "codex"];
        assert_eq!(
            first_scope(Kind::Handoff, &targets, "cursor", Some("codex")),
            Some("codex")
        );
        assert_eq!(
            first_scope(Kind::Handoff, &targets, "cursor", Some("grok")),
            Some("claude")
        );
    }

    #[test]
    fn a_second_opinion_opens_on_another_provider_than_the_session() {
        assert_eq!(
            first_scope(Kind::Opinion, &ENABLED, "claude", None),
            Some("codex")
        );
        assert_eq!(
            first_scope(Kind::Opinion, &["claude"], "claude", None),
            Some("claude")
        );
        assert_eq!(first_scope(Kind::Handoff, &[], "claude", None), None);
    }

    #[test]
    fn planning_follows_the_provider_rule() {
        assert!(plans("claude", None));
        assert!(plans("codex", Some("gpt-5")));
        assert!(!plans("codex", None));
        assert!(!plans("grok", Some("grok-4")));
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

    #[test]
    fn the_prompt_quotes_the_turn_in_the_window_language() {
        let files = vec!["src/a.rs".to_owned(), "src/b.rs".to_owned()];
        let pt = opinion_prompt("Claude Code", "corrige o parser", "feito", &files, true);
        assert!(pt.starts_with("Dê uma segunda opinião sobre o trabalho que Claude Code"));
        assert!(pt.contains("## Pedido do usuário\ncorrige o parser"));
        assert!(pt.contains("## O que Claude Code respondeu\nfeito"));
        assert!(pt.ends_with("## Arquivos alterados\n- src/a.rs\n- src/b.rs"));
        let en = opinion_prompt("Codex", "", "", &[], false);
        assert!(en.starts_with("Give a second opinion on the work Codex"));
        assert!(en.contains("## User request\n(no request on this turn)"));
        assert!(en.contains("## What Codex answered\n(no written summary — inspect the files)"));
        assert!(en.ends_with("## Files changed\n(no files recorded on this turn)"));
    }

    #[test]
    fn a_long_request_is_cut_with_a_marker() {
        assert_eq!(clip("  abc  ", 10), "abc");
        assert_eq!(clip("abcdef", 3), "abc\n[…]");
        assert_eq!(clip("ab cdef", 3), "ab\n[…]");
    }
}
