//! The subagents of a turn as one card in the transcript (Sirus's `SubagentTeam`, ADR-146):
//! a header with how many there are and how far they got, the subagents in phases, a row each.
//! One subagent has no header: its row is the card. Pointing at a running Claude subagent swaps
//! its step count for Mensagem and Parar (ADR-111, ADR-113), so nothing is covered; a row opens
//! what that subagent did (`subagent_feed`, Sirus's `SubagentPane`) in place, under the row.
//!
//! The data and its rules are in `subagent_team.rs`. This module is a child of `shell`
//! (`#[path]`), so it reaches the window's private state. Nothing here draws while idle: the
//! elapsed times follow the shell's own pulse, which runs only while a conversation works.

use super::*;
use crate::subagent_team::{self as team, Member, Status};
use gpui::ScrollHandle;
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

const HEAD: f32 = 36.;
const ROW: f32 = 30.;
/// A pip per subagent up to this many; past it the count says enough.
const MAX_PIPS: usize = 8;
/// The room the three buttons take, which the step count shares.
const ACTIONS: f32 = 80.;
const PHASE: f32 = 22.;
const COMPOSE: f32 = 38.;
const FEED: f32 = 232.;
const PAGE: usize = 60;
const KEPT: usize = 300;
/// The `turn` of a `transcript_select::Reply` that names an entry of the open feed (its `beat`),
/// not a turn of the conversation.
pub(super) const FEED_TURN: usize = usize::MAX;

/// What a reply from the host is for.
#[derive(Clone)]
pub(super) enum Call {
    /// A message sent to a subagent: its session and task.
    Message(String, String),
    Stop,
    /// A running subagent sent to the background.
    Background,
    /// Its feed was asked for: session and key.
    Feed(String, String),
}

/// The one-line input under a row.
struct Compose {
    session: String,
    task: String,
    text: String,
    busy: bool,
    error: Option<String>,
    shut: bool,
    open: Tween,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Prompt,
    Text,
    Thinking,
    Tool,
}

#[derive(Clone)]
struct Entry {
    id: String,
    kind: Kind,
    text: String,
    label: String,
    detail: String,
    output: String,
    tool: String,
    state: Status,
}

impl Entry {
    fn from_json(value: &Value) -> Self {
        let text = |key: &str| value[key].as_str().unwrap_or("").to_owned();
        Self {
            id: text("id"),
            kind: match value["kind"].as_str() {
                Some("prompt") => Kind::Prompt,
                Some("text") => Kind::Text,
                Some("thinking") => Kind::Thinking,
                _ => Kind::Tool,
            },
            text: text("text"),
            label: text("label"),
            detail: text("detail"),
            output: text("output"),
            tool: text("tool"),
            state: Status::of(value["state"].as_str().unwrap_or(""), false),
        }
    }
}

/// New entries merged by id: a known one is replaced in place, new ones go at the end. At most
/// `limit` are kept, the oldest dropped first; how many were dropped is returned.
fn merge(known: &mut Vec<Entry>, incoming: Vec<Entry>, limit: usize) -> usize {
    for entry in incoming {
        match known.iter().position(|e| e.id == entry.id) {
            Some(at) => known[at] = entry,
            None => known.push(entry),
        }
    }
    let dropped = known.len().saturating_sub(limit);
    known.drain(..dropped);
    dropped
}

/// One subagent's own conversation, open under its row.
struct Feed {
    session: String,
    key: String,
    entries: Vec<Entry>,
    dropped: usize,
    loading: bool,
    shown: usize,
    opened: HashSet<String>,
    scroll: ScrollHandle,
    shut: bool,
    open: Tween,
    /// Where the feed was scrolled when it was left, put back once its entries arrive.
    restore: Option<gpui::Point<Pixels>>,
}

/// What one subagent's feed keeps while another subagent's is open.
struct Remembered {
    shown: usize,
    opened: HashSet<String>,
    offset: gpui::Point<Pixels>,
}

impl Feed {
    fn remember(&self) -> Remembered {
        Remembered {
            shown: self.shown,
            opened: self.opened.clone(),
            offset: self.scroll.offset(),
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    /// Each subagent's feed view, by (session, key), for when it is opened again.
    remembered: HashMap<(String, String), Remembered>,
    /// A card folded shut: (conversation, turn) to its openness; absent is open.
    folds: HashMap<(usize, usize), Tween>,
    compose: Option<Compose>,
    feed: Option<Feed>,
    /// The subagent a message was just queued for.
    sent: Option<String>,
}

impl State {
    /// Whether the input under a row takes the keys for `session`.
    pub(super) fn composing(&self, session: &str) -> bool {
        self.compose
            .as_ref()
            .is_some_and(|c| !c.shut && c.session == session)
    }

    pub(super) fn compose_text(&mut self, session: &str) -> Option<&mut String> {
        self.compose
            .as_mut()
            .filter(|c| !c.shut && c.session == session)
            .map(|c| &mut c.text)
    }

    /// Merges what a `subagent-feed` event carries into the open feed.
    pub(super) fn feed_event(&mut self, event: &Value) -> bool {
        let Some(feed) = self.feed.as_mut().filter(|f| {
            f.session == event["sessionId"].as_str().unwrap_or("")
                && f.key == event["key"].as_str().unwrap_or("")
        }) else {
            return false;
        };
        let incoming = event["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .map(Entry::from_json)
            .collect();
        feed.take(incoming, event["dropped"].as_u64().unwrap_or(0) as usize);
        true
    }
}

impl Feed {
    fn take(&mut self, incoming: Vec<Entry>, host_dropped: usize) {
        let at_bottom = self.scroll.max_offset().y + self.scroll.offset().y < px(48.);
        let first = self.entries.is_empty();
        let restore = self
            .restore
            .take()
            .filter(|_| first && !incoming.is_empty());
        self.dropped = host_dropped.max(self.dropped) + merge(&mut self.entries, incoming, KEPT);
        self.loading = false;
        if let Some(offset) = restore {
            self.scroll.set_offset(offset);
        } else if at_bottom || first {
            self.scroll.scroll_to_bottom();
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// "1 de 3", with any failures after it; the pips show the rest.
fn summary(p: &team::Progress) -> String {
    let count = tf("{} de {}", &[&p.done, &p.total]);
    match p.failed {
        0 => count,
        1 => format!("{count} · {}", t("1 falhou")),
        n => format!("{count} · {}", tf("{} falharam", &[&n])),
    }
}

fn steps_text(tools: usize) -> String {
    if tools == 1 {
        t("1 passo").to_owned()
    } else {
        tf("{} passos", &[&tools])
    }
}

/// The provider's mark, in the header or in a lone subagent's row.
fn provider_tile(provider: &str) -> impl IntoElement {
    mark_tile(
        &crate::tab_tile::TileSize {
            w: 16.,
            h: 14.,
            radius: 4.,
        },
        Face::provider(provider),
        provider_glyph(provider, 10.),
    )
}

impl Shell {
    /// The card for a turn's subagents, or none when it launched none.
    pub(super) fn subagent_card(
        &self,
        conversation: usize,
        index: usize,
        turn: &Turn,
        provider: &str,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let members = &turn.agents;
        if members.is_empty() {
            return None;
        }
        let session = self.conversations[conversation].session_id.clone();
        let progress = team::progress(members);
        let state = &self.subagents;
        // One subagent has no header to fold it with.
        let open = match state.folds.get(&(conversation, index)) {
            Some(tween) if members.len() > 1 => {
                *moving |= !tween.done(now);
                tween.at(now).clamp(0., 1.)
            }
            _ => 1.,
        };
        let lone = members.len() == 1;
        let (add, remove) = diff_colors(self.look.diff);
        let pips = (members.len() <= MAX_PIPS).then(|| {
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(3.))
                .children(members.iter().map(|member| {
                    div()
                        .w(px(14.))
                        .h(px(3.))
                        .rounded(px(2.))
                        .bg(match member.status {
                            Status::Running => accent(),
                            Status::Completed => add.into(),
                            Status::Failed | Status::Stopped => remove.into(),
                            Status::Unknown => content(0.14),
                        })
                }))
        });
        let head = (!lone).then(|| {
            div()
                .id(("sub-head", index))
                .h(px(HEAD))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .on_click(
                    cx.listener(move |this, _, _, cx| this.toggle_fold(conversation, index, cx)),
                )
                .child(provider_tile(provider))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(0.88))
                        .child(tf("{} subagentes", &[&members.len()])),
                )
                .children(pips)
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child(summary(&progress)),
                )
                .child(
                    theme_icon("chevron-right", 12.)
                        .text_color(content(0.5))
                        .with_transformation(Transformation::rotate(percentage(open * 0.25))),
                )
        });

        // The body is laid out only while it shows; its height is exact (every row is the same).
        let rows = crate::hover_track::list_barred(&format!("sub-rows-{conversation}-{index}"));
        let buttons = crate::hover_track::list(&format!("sub-actions-{conversation}-{index}"));
        let mut body: Vec<AnyElement> = vec![rows.pill().into_any_element()];
        let mut natural = 0.;
        if open > 0.001 {
            let runs = team::phases(members);
            let headed = runs.len() > 1;
            natural += 6.;
            let wall = now_ms();
            let controls = turn.live && provider == "claude";
            for run in runs {
                if headed {
                    natural += PHASE;
                    body.push(
                        div()
                            .h(px(PHASE))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .text_size(px(11.5))
                            .text_color(content(0.4))
                            .child(tf("Fase {}", &[&members[run.start].phase]))
                            .into_any_element(),
                    );
                }
                for at in run {
                    let member = &members[at];
                    natural += ROW;
                    body.push(
                        rows.row(
                            self.subagent_row(
                                &session, index, at, member, lone, provider, controls, wall, now,
                                &buttons, cx,
                            ),
                            at,
                            false,
                        )
                        .into_any_element(),
                    );
                    if let Some(extra) = self.under_row(&session, member, now, moving, cx) {
                        natural += extra.0;
                        body.push(extra.1);
                    }
                }
            }
            natural += 4.;
        }
        Some(
            div()
                .w_full()
                .rounded(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_color(crate::theme::border())
                .bg(content(0.03))
                .overflow_hidden()
                .flex()
                .flex_col()
                .children(head)
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(natural * open))
                        .opacity(open)
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .px(px(4.))
                        .pt(px(2.))
                        .children(body),
                )
                .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn subagent_row(
        &self,
        session: &str,
        turn: usize,
        at: usize,
        member: &Member,
        lone: bool,
        provider: &str,
        controls: bool,
        wall: i64,
        now: Instant,
        buttons: &crate::hover_track::HoverTrack,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (add, remove) = diff_colors(self.look.diff);
        let uid = turn * 4096 + at;
        let group = format!("sub-row-{uid}");
        let group_steps = group.clone();
        let lead: AnyElement = match member.status {
            Status::Running => self.spin(now).into_any_element(),
            Status::Completed => theme_icon("check", 13.).text_color(add).into_any_element(),
            Status::Failed | Status::Stopped => theme_icon("close", 13.)
                .text_color(remove)
                .into_any_element(),
            Status::Unknown => div()
                .size(px(13.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(5.)).rounded_full().bg(content(0.3)))
                .into_any_element(),
        };
        let name = if member.label.is_empty() {
            tf("Subagente {}", &[&(at + 1)])
        } else {
            member.label.clone()
        };
        let elapsed = member.elapsed(wall).map(team::duration);
        let key = member.key.clone();
        let open_session = session.to_owned();
        let actions = (controls && member.running()).then(|| {
            let (message_session, message_task) = (session.to_owned(), member.task.clone());
            let (stop_session, stop_task) = (session.to_owned(), member.task.clone());
            let background = member
                .backgroundable()
                .map(|source| (session.to_owned(), member.task.clone(), source.to_owned()));
            let action =
                |id: &'static str, offset: usize, name: &'static str, glyph: AnyElement| {
                    buttons.row(
                        div()
                            .id((id, uid + offset))
                            .relative()
                            .w(px(26.))
                            .h(px(24.))
                            .rounded(px(crate::theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .tooltip(crate::shell::message_actions::name_tip(t(name)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(glyph),
                        uid * 3 + offset,
                        false,
                    )
                };
            div()
                .absolute()
                .right_0()
                .top(px((ROW - 24.) / 2.))
                .flex()
                .items_center()
                .gap(px(1.))
                .opacity(0.)
                .group_hover(group.clone(), |s| s.opacity(1.))
                .child(
                    action(
                        "sub-message",
                        0,
                        "Enviar mensagem",
                        theme_icon("message-square", 14.)
                            .text_color(content(0.7))
                            .into_any_element(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.compose_subagent(&message_session, &message_task, cx);
                    })),
                )
                .children(background.map(|(bg_session, bg_task, bg_source)| {
                    action(
                        "sub-background",
                        1,
                        "Mandar para segundo plano",
                        theme_icon("layers", 14.)
                            .text_color(content(0.7))
                            .into_any_element(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.background_subagent(&bg_session, &bg_task, &bg_source, cx);
                    }))
                }))
                .child(
                    action(
                        "sub-stop",
                        2,
                        "Parar subagente",
                        theme_icon("square-fill", 12.)
                            .text_color(content(0.7))
                            .into_any_element(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.stop_subagent(&stop_session, &stop_task, cx);
                    })),
                )
        });
        let swaps = actions.is_some();
        div()
            .id(("sub-row", uid))
            .group(group)
            .relative()
            .w_full()
            .h(px(ROW))
            .flex_shrink_0()
            .px(px(8.))
            .rounded(px(crate::theme::RADIUS_MD))
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_feed(&open_session, &key, cx)))
            .child(lead)
            .when(lone, |d| d.child(provider_tile(provider)))
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px(240.))
                    .truncate()
                    .line_height(px(ROW))
                    .text_size(px(13.))
                    .text_color(content(0.9))
                    .child(name),
            )
            // Which model runs it, next to its name, in the provider's own colour.
            .children(member.model.as_deref().map(|model| {
                div()
                    .flex_shrink_0()
                    .max_w(px(160.))
                    .truncate()
                    .line_height(px(ROW))
                    .text_size(px(11.5))
                    .text_color(content(0.4))
                    .child(crate::shell::model_label(Some(model), provider))
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .line_height(px(ROW))
                    .text_size(px(12.))
                    .text_color(content(0.4))
                    .child(member.step.clone()),
            )
            // The step count and the buttons share one place, and neither covers the time.
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .h_full()
                    .when(swaps, |d| d.w(px(ACTIONS)))
                    .flex()
                    .items_center()
                    .justify_end()
                    .when(member.tools > 0, |d| {
                        d.child(
                            div()
                                .line_height(px(ROW))
                                .text_size(px(12.))
                                .text_color(content(0.4))
                                .when(swaps, |d| d.group_hover(group_steps, |s| s.opacity(0.)))
                                .child(steps_text(member.tools)),
                        )
                    })
                    .children(actions),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(52.))
                    .flex()
                    .justify_end()
                    .line_height(px(ROW))
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .children(elapsed),
            )
    }

    /// What opens under a row: the message input, then the subagent's own conversation.
    fn under_row(
        &self,
        session: &str,
        member: &Member,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<(f32, AnyElement)> {
        let state = &self.subagents;
        let compose = state
            .compose
            .as_ref()
            .filter(|c| c.session == session && c.task == member.task);
        let feed = state
            .feed
            .as_ref()
            .filter(|f| f.session == session && f.key == member.key);
        let sent = state.sent.as_deref() == Some(member.task.as_str());
        let mut total = 0.;
        let mut parts: Vec<AnyElement> = Vec::new();
        if let Some(compose) = compose {
            *moving |= !compose.open.done(now);
            let open = compose.open.at(now).clamp(0., 1.);
            if open > 0.001 {
                total += COMPOSE * open;
                parts.push(self.compose_view(compose, open, cx));
            }
        } else if sent {
            total += 22.;
            parts.push(
                div()
                    .h(px(22.))
                    .px(px(40.))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .child(t("Enviada: lida no próximo passo"))
                    .into_any_element(),
            );
        }
        if let Some(feed) = feed {
            *moving |= !feed.open.done(now);
            let open = feed.open.at(now).clamp(0., 1.);
            if open > 0.001 {
                total += (FEED + 6.) * open;
                parts.push(self.feed_view(feed, member, open, cx));
            }
        }
        (!parts.is_empty()).then(|| {
            (
                total,
                div()
                    .w_full()
                    .flex_shrink_0()
                    .h(px(total))
                    .overflow_hidden()
                    .children(parts)
                    .into_any_element(),
            )
        })
    }

    fn compose_view(&self, compose: &Compose, open: f32, cx: &mut Context<Self>) -> AnyElement {
        let (_, remove) = diff_colors(self.look.diff);
        div()
            .w_full()
            .h(px(COMPOSE * open))
            .overflow_hidden()
            .opacity(open)
            .child(
                div()
                    .h(px(28.))
                    .mt(px(5.))
                    .mx(px(8.))
                    .px(px(10.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .border_1()
                    .border_color(crate::theme::border())
                    .bg(field_fill())
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .opacity(if compose.busy { 0.6 } else { 1. })
                    // A click anywhere else, such as the composer, closes it and gives the keys back.
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if this
                            .subagents
                            .compose
                            .as_ref()
                            .is_some_and(|c| !c.busy && !c.shut)
                        {
                            this.close_compose(cx);
                        }
                    }))
                    .child(typed_text(
                        &compose.text,
                        t("Mensagem para este subagente"),
                        self.popup_caret(),
                    ))
                    .children(compose.error.clone().map(|error| {
                        div()
                            .max_w(px(260.))
                            .flex_shrink_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(remove)
                            .child(error)
                    }))
                    .child(
                        div()
                            .id("sub-compose-close")
                            .ml_auto()
                            .flex_shrink_0()
                            .size(px(18.))
                            .rounded(px(crate::theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.08)))
                            .tooltip(crate::shell::message_actions::name_tip(t("Fechar")))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| this.close_compose(cx)))
                            .child(theme_icon("close", 11.).text_color(content(0.5))),
                    ),
            )
            .into_any_element()
    }

    fn feed_view(
        &self,
        feed: &Feed,
        member: &Member,
        open: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (add, remove) = diff_colors(self.look.diff);
        let start = feed.entries.len().saturating_sub(feed.shown);
        let hidden = start;
        let mut rows: Vec<AnyElement> = Vec::new();
        if feed.dropped > 0 {
            rows.push(note(tf(
                "{} entradas anteriores não são mais guardadas",
                &[&feed.dropped],
            )));
        }
        if hidden > 0 {
            let (session, key) = (feed.session.clone(), feed.key.clone());
            rows.push(
                div()
                    .id("sub-earlier")
                    .h(px(24.))
                    .flex_shrink_0()
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .cursor_pointer()
                    .hover(|s| s.text_color(content(0.9)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(f) = this
                            .subagents
                            .feed
                            .as_mut()
                            .filter(|f| f.session == session && f.key == key)
                        {
                            f.shown += PAGE;
                        }
                        cx.notify();
                    }))
                    .child(tf("Mostrar {} anteriores", &[&hidden.min(PAGE)]))
                    .into_any_element(),
            );
        }
        for (at, entry) in feed.entries.iter().enumerate().skip(start) {
            rows.push(self.feed_entry(feed, at, entry, (add, remove), cx));
        }
        if feed.loading {
            rows.push(note(t("Lendo o que ele fez…").to_owned()));
        } else if feed.entries.is_empty() {
            rows.push(note(
                if member.running() {
                    t("Esperando o primeiro passo…")
                } else {
                    t("Nada foi registrado para este subagente.")
                }
                .to_owned(),
            ));
        }
        div()
            .w_full()
            .h(px((FEED + 6.) * open))
            .overflow_hidden()
            .opacity(open)
            .child(
                div()
                    .id("sub-feed")
                    .h(px(FEED))
                    .mt(px(3.))
                    .mb(px(3.))
                    .mx(px(8.))
                    .p(px(6.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .bg(content(0.03))
                    .border_1()
                    .border_color(crate::theme::border())
                    .overflow_y_scroll()
                    .track_scroll(&feed.scroll)
                    // The wheel scrolls the feed alone, never the conversation behind it, while
                    // the feed has more than it shows.
                    .on_scroll_wheel({
                        let scroll = feed.scroll.clone();
                        move |_: &gpui::ScrollWheelEvent, _, cx| {
                            if scroll.max_offset().y > px(0.) {
                                cx.stop_propagation();
                            }
                        }
                    })
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(rows),
            )
            .into_any_element()
    }

    /// The selection address of the open feed's entry `at`.
    fn feed_reply(&self, at: usize) -> transcript_select::Reply {
        transcript_select::Reply {
            conversation: self.active_conversation(),
            turn: FEED_TURN,
            user: false,
            beat: at,
        }
    }

    /// The selectable fragments of the open feed's entry `at`, as drawn.
    pub(super) fn feed_texts(&self, at: usize) -> Vec<String> {
        let Some(entry) = self.subagents.feed.as_ref().and_then(|f| f.entries.get(at)) else {
            return Vec::new();
        };
        match entry.kind {
            Kind::Prompt => vec![entry.text.clone()],
            Kind::Text => transcript_select::record(|| {
                crate::markdown::render(&entry.text);
            }),
            Kind::Thinking | Kind::Tool => Vec::new(),
        }
    }

    fn feed_entry(
        &self,
        feed: &Feed,
        at: usize,
        entry: &Entry,
        (add, remove): (gpui::Hsla, gpui::Hsla),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match entry.kind {
            Kind::Prompt => div()
                .px(px(6.))
                .py(px(4.))
                .rounded(px(crate::theme::RADIUS_SM))
                .bg(content(0.04))
                .flex_shrink_0()
                .text_size(px(12.))
                .text_color(content(0.6))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child(t("Tarefa")),
                )
                .child(div().line_clamp(4).child(transcript_select::scoped(
                    &self.select,
                    self.feed_reply(at),
                    || {
                        transcript_select::wrap(
                            gpui::StyledText::new(entry.text.clone()),
                            entry.text.clone().into(),
                        )
                    },
                )))
                .into_any_element(),
            Kind::Text => div()
                .px(px(6.))
                .flex_shrink_0()
                .text_color(content(0.75))
                .child(transcript_select::scoped(
                    &self.select,
                    self.feed_reply(at),
                    || crate::markdown::render(&entry.text),
                ))
                .into_any_element(),
            Kind::Thinking => {
                let first = entry
                    .text
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or_else(|| t("Raciocínio"))
                    .to_owned();
                div()
                    .h(px(22.))
                    .flex_shrink_0()
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(12.))
                    .text_color(content(0.4))
                    .child(theme_icon("bulb", 12.).text_color(content(0.4)))
                    .child(div().flex_1().min_w_0().truncate().child(first))
                    .into_any_element()
            }
            Kind::Tool => {
                let glyph = match entry.tool.as_str() {
                    "read" => "search",
                    "edit" => "pencil",
                    "command" => "terminal-square",
                    "agent" => "bot",
                    "skill" => "box",
                    _ => "tool-branch",
                };
                let mark: AnyElement = match entry.state {
                    Status::Running => self.spin(Instant::now()).into_any_element(),
                    Status::Failed | Status::Stopped => theme_icon("close", 12.)
                        .text_color(remove)
                        .into_any_element(),
                    Status::Completed => {
                        theme_icon("check", 12.).text_color(add).into_any_element()
                    }
                    Status::Unknown => div().size(px(12.)).into_any_element(),
                };
                let has_output = !entry.output.trim().is_empty();
                let expanded = has_output && feed.opened.contains(&entry.id);
                let (session, key, id) = (feed.session.clone(), feed.key.clone(), entry.id.clone());
                let line = div()
                    .id(("sub-entry", at))
                    .h(px(22.))
                    .flex_shrink_0()
                    .px(px(6.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(12.))
                    .text_color(content(0.6))
                    .when(has_output, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(content(0.07)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(f) = this
                                    .subagents
                                    .feed
                                    .as_mut()
                                    .filter(|f| f.session == session && f.key == key)
                                {
                                    if !f.opened.remove(&id) {
                                        f.opened.insert(id.clone());
                                    }
                                }
                                cx.notify();
                            }))
                    })
                    .child(theme_icon(glyph, 12.).text_color(content(0.45)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .gap(px(6.))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .max_w(px(160.))
                                    .truncate()
                                    .child(entry.label.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(content(0.4))
                                    .child(entry.detail.clone()),
                            ),
                    )
                    .child(mark);
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .child(line)
                    .when(expanded, |d| {
                        d.child(
                            div()
                                .mx(px(6.))
                                .my(px(2.))
                                .p(px(6.))
                                .rounded(px(crate::theme::RADIUS_SM))
                                .bg(content(0.05))
                                .text_size(px(11.5))
                                .text_color(content(0.55))
                                .children(
                                    entry
                                        .output
                                        .trim_end()
                                        .lines()
                                        .rev()
                                        .take(14)
                                        .collect::<Vec<_>>()
                                        .into_iter()
                                        .rev()
                                        .map(|l| div().min_h(px(15.)).child(l.to_owned())),
                                ),
                        )
                    })
                    .into_any_element()
            }
        }
    }

    // Actions.

    fn toggle_fold(&mut self, conversation: usize, index: usize, cx: &mut Context<Self>) {
        let now = Instant::now();
        let tween = self
            .subagents
            .folds
            .entry((conversation, index))
            .or_insert_with(|| Tween::still(1., now));
        let to = if tween.at(now) > 0.5 { 0. } else { 1. };
        let duration = if cx.reduce_motion() { 0. } else { DISCLOSE };
        tween.set(to, now, duration, Curve::Expo);
        cx.notify();
    }

    /// Opens the one-line input under a running subagent's row.
    fn compose_subagent(&mut self, session: &str, task: &str, cx: &mut Context<Self>) {
        let now = Instant::now();
        let duration = if cx.reduce_motion() { 0. } else { DISCLOSE };
        self.subagents.sent = None;
        self.subagents.compose = Some(Compose {
            session: session.to_owned(),
            task: task.to_owned(),
            text: String::new(),
            busy: false,
            error: None,
            shut: false,
            open: Tween::from_to(0., 1., now, duration, Curve::Expo),
        });
        cx.notify();
    }

    fn close_compose(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let duration = if cx.reduce_motion() { 0. } else { DISCLOSE };
        if let Some(compose) = self.subagents.compose.as_mut() {
            compose.shut = true;
            compose.open.set(0., now, duration, Curve::Expo);
        }
        cx.notify();
    }

    /// Keys for the input under a row: Enter sends, Esc closes, Backspace erases.
    /// Returns whether it took the key.
    pub(super) fn subagent_keys(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let session = self.active_session_id();
        if !self.subagents.composing(&session) || !matches!(key, "enter" | "escape" | "backspace") {
            return false;
        }
        match key {
            "escape" => self.close_compose(cx),
            "enter" => {
                let Some(compose) = self.subagents.compose.as_mut().filter(|c| !c.busy) else {
                    return true;
                };
                let text = compose.text.trim().to_owned();
                if text.is_empty() {
                    return true;
                }
                compose.busy = true;
                compose.error = None;
                let (session, task) = (compose.session.clone(), compose.task.clone());
                let call = self.backend.invoke(
                    "steer_subagent",
                    json!({ "sessionId": session, "taskId": task, "text": text }),
                );
                self.waiting
                    .insert(call, Waiting::Subagent(Call::Message(session, task)));
            }
            _ => {
                if let Some(compose) = self.subagents.compose.as_mut().filter(|c| !c.busy) {
                    compose.text.pop();
                    compose.error = None;
                }
            }
        }
        true
    }

    /// Stops this subagent only; the host settles its row.
    fn stop_subagent(&mut self, session: &str, task: &str, cx: &mut Context<Self>) {
        let call = self.backend.invoke(
            "stop_background_task",
            json!({ "sessionId": session, "taskId": task }),
        );
        self.waiting.insert(call, Waiting::Subagent(Call::Stop));
        cx.notify();
    }

    /// Moves this running foreground subagent to the background: the reply goes on without
    /// waiting for it. The row then reads as background through the next activity update.
    fn background_subagent(
        &mut self,
        session: &str,
        task: &str,
        source: &str,
        cx: &mut Context<Self>,
    ) {
        let call = self.backend.invoke(
            "background_subagent",
            background_args(session, task, source),
        );
        self.waiting
            .insert(call, Waiting::Subagent(Call::Background));
        cx.notify();
    }

    /// Opens (or shuts) a subagent's own conversation under its row.
    fn toggle_feed(&mut self, session: &str, key: &str, cx: &mut Context<Self>) {
        let now = Instant::now();
        let duration = if cx.reduce_motion() { 0. } else { DISCLOSE };
        if let Some(feed) = self
            .subagents
            .feed
            .as_mut()
            .filter(|f| f.session == session && f.key == key)
        {
            feed.shut = !feed.shut;
            feed.open
                .set(if feed.shut { 0. } else { 1. }, now, duration, Curve::Expo);
            cx.notify();
            return;
        }
        // The feed being left keeps its page, opened steps and scroll for its next opening.
        if let Some(left) = self.subagents.feed.take() {
            let kept = left.remember();
            self.subagents
                .remembered
                .insert((left.session, left.key), kept);
        }
        let back = self
            .subagents
            .remembered
            .remove(&(session.to_owned(), key.to_owned()));
        self.subagents.feed = Some(Feed {
            session: session.to_owned(),
            key: key.to_owned(),
            entries: Vec::new(),
            dropped: 0,
            loading: true,
            shown: back.as_ref().map_or(PAGE, |b| b.shown),
            restore: back.as_ref().map(|b| b.offset),
            opened: back
                .as_ref()
                .map_or_else(HashSet::new, |b| b.opened.clone()),
            scroll: ScrollHandle::new(),
            shut: false,
            open: Tween::from_to(0., 1., now, duration, Curve::Expo),
        });
        let call = self
            .backend
            .invoke("subagent_feed", json!({ "sessionId": session, "key": key }));
        self.waiting.insert(
            call,
            Waiting::Subagent(Call::Feed(session.to_owned(), key.to_owned())),
        );
        cx.notify();
    }

    pub(super) fn subagent_reply(
        &mut self,
        call: Call,
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        match call {
            Call::Stop | Call::Background => {
                if !ok {
                    self.problem = Some(error_text(value));
                }
            }
            Call::Message(session, task) => {
                let current = self
                    .subagents
                    .compose
                    .as_mut()
                    .filter(|c| c.session == session && c.task == task);
                if let Some(compose) = current {
                    compose.busy = false;
                    if ok {
                        self.subagents.sent = Some(task);
                        self.close_compose(cx);
                    } else {
                        compose.error = Some(error_text(value));
                    }
                } else if !ok {
                    self.problem = Some(error_text(value));
                }
            }
            Call::Feed(session, key) => {
                if let Some(feed) = self
                    .subagents
                    .feed
                    .as_mut()
                    .filter(|f| f.session == session && f.key == key)
                {
                    if ok {
                        // What streamed in while the call ran is newer than what it read.
                        let mut read: Vec<Entry> = value["entries"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(Entry::from_json)
                            .collect();
                        let streamed = std::mem::take(&mut feed.entries);
                        merge(&mut read, streamed, KEPT);
                        feed.entries = Vec::new();
                        feed.take(read, value["dropped"].as_u64().unwrap_or(0) as usize);
                    } else {
                        feed.loading = false;
                    }
                }
                if !ok {
                    self.problem = Some(error_text(value));
                }
            }
        }
        cx.notify();
    }

    /// "N em segundo plano", beside the turn's working line while background subagents run.
    pub(super) fn background_pill(&self, turn: &Turn) -> Option<AnyElement> {
        let count = team::progress(&turn.agents).background;
        (count > 0).then(|| {
            div()
                .h(px(22.))
                .px(px(9.))
                .flex_shrink_0()
                .rounded(px(crate::theme::RADIUS_SM))
                .bg(accent_wash(0.16))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(accent())
                .child(div().size(px(6.)).rounded_full().bg(accent()))
                .child(tf("{} em segundo plano", &[&count]))
                .into_any_element()
        })
    }
}

/// The arguments of `background_subagent`: the CLI's task id and its launching tool call.
fn background_args(session: &str, task: &str, source: &str) -> Value {
    json!({ "sessionId": session, "taskId": task, "toolUseId": source })
}

fn note(text: String) -> AnyElement {
    div()
        .px(px(6.))
        .py(px(2.))
        .flex_shrink_0()
        .text_size(px(12.))
        .text_color(content(0.4))
        .child(text)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, text: &str) -> Entry {
        Entry::from_json(&json!({ "id": id, "kind": "text", "text": text }))
    }

    #[test]
    fn merging_replaces_known_entries_and_appends_new_ones() {
        let mut known = vec![entry("a", "1"), entry("b", "2")];
        let dropped = merge(&mut known, vec![entry("b", "two"), entry("c", "3")], 10);
        assert_eq!(dropped, 0);
        let texts: Vec<_> = known.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["1", "two", "3"]);
    }

    #[test]
    fn merging_keeps_the_newest_and_counts_what_it_dropped() {
        let mut known = vec![entry("a", ""), entry("b", "")];
        let dropped = merge(&mut known, vec![entry("c", ""), entry("d", "")], 3);
        assert_eq!(dropped, 1);
        let ids: Vec<_> = known.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["b", "c", "d"]);
    }

    #[test]
    fn the_summary_reads_done_running_and_failed() {
        crate::i18n::set_locale("pt-BR");
        let p = team::Progress {
            done: 2,
            total: 5,
            running: 3,
            failed: 0,
            background: 0,
        };
        assert_eq!(summary(&p), "2 de 5");
        let p = team::Progress {
            done: 5,
            total: 5,
            running: 0,
            failed: 1,
            background: 0,
        };
        assert_eq!(summary(&p), "5 de 5 · 1 falhou");
        let p = team::Progress {
            done: 4,
            total: 4,
            running: 0,
            failed: 2,
            background: 0,
        };
        assert_eq!(summary(&p), "4 de 4 · 2 falharam");
    }

    #[test]
    fn background_args_name_the_task_and_its_launching_call() {
        let args = background_args("s1", "t1", "toolu_9");
        assert_eq!(args["sessionId"], "s1");
        assert_eq!(args["taskId"], "t1");
        assert_eq!(args["toolUseId"], "toolu_9");
    }

    #[test]
    fn steps_are_counted_in_the_singular_too() {
        crate::i18n::set_locale("pt-BR");
        assert_eq!(steps_text(1), "1 passo");
        assert_eq!(steps_text(12), "12 passos");
    }

    #[test]
    fn typing_goes_to_the_input_of_its_session_only() {
        let mut state = State::default();
        assert!(!state.composing("s1"));
        state.compose = Some(Compose {
            session: "s1".into(),
            task: "t".into(),
            text: String::new(),
            busy: false,
            error: None,
            shut: false,
            open: Tween::still(1., Instant::now()),
        });
        assert!(state.composing("s1") && !state.composing("s2"));
        state.compose_text("s1").unwrap().push('x');
        assert_eq!(state.compose.as_ref().unwrap().text, "x");
        state.compose.as_mut().unwrap().shut = true;
        assert!(!state.composing("s1") && state.compose_text("s1").is_none());
    }
}
