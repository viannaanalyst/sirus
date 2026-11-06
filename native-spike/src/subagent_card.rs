//! The subagents of a turn as one card in the transcript (Sirus Code's `SubagentTeam`, ADR-146):
//! a header with the team's headline and counts, the subagents in phases, a row each. Pointing
//! at a running Claude subagent shows Mensagem and Parar (ADR-111, ADR-113); a row opens what
//! that subagent did (`subagent_feed`, Sirus Code's `SubagentPane`) in place, under the row.
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
const ROW: f32 = 28.;
const PHASE: f32 = 22.;
const COMPOSE: f32 = 38.;
const FEED: f32 = 232.;
const PAGE: usize = 60;
const KEPT: usize = 300;

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
}

#[derive(Default)]
pub(super) struct State {
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
        self.dropped = host_dropped.max(self.dropped) + merge(&mut self.entries, incoming, KEPT);
        self.loading = false;
        if at_bottom || first {
            self.scroll.scroll_to_bottom();
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// "N de M concluídos · K em execução · J falharam", the card's counts.
fn summary(p: &team::Progress) -> String {
    let mut parts = vec![tf("{} de {} concluídos", &[&p.done, &p.total])];
    if p.running > 0 {
        parts.push(tf("{} em execução", &[&p.running]));
    }
    match p.failed {
        0 => {}
        1 => parts.push(t("1 falhou").to_owned()),
        n => parts.push(tf("{} falharam", &[&n])),
    }
    parts.join(" · ")
}

fn steps_text(tools: usize) -> String {
    if tools == 1 {
        t("1 passo").to_owned()
    } else {
        tf("{} passos", &[&tools])
    }
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
        let open = match state.folds.get(&(conversation, index)) {
            Some(tween) => {
                *moving |= !tween.done(now);
                tween.at(now).clamp(0., 1.)
            }
            None => 1.,
        };
        let title = if turn.headline.is_empty() {
            if members.len() == 1 {
                t("1 subagente").to_owned()
            } else {
                tf("{} subagentes", &[&members.len()])
            }
        } else {
            turn.headline.clone()
        };
        let head = div()
            .id(("sub-head", index))
            .h(px(HEAD))
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_fold(conversation, index, cx)
            }))
            .child(theme_icon("users", 14.).text_color(content(0.7)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(content(0.85))
                    .child(title),
            )
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
            );

        // The body is laid out only while it shows; its height is exact (every row is the same).
        let mut body: Vec<AnyElement> = Vec::new();
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
                    body.push(self.subagent_row(
                        &session, index, at, member, provider, controls, wall, now, cx,
                    ));
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
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.1))
                .bg(content(0.03))
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(head)
                .child(
                    div()
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
        provider: &str,
        controls: bool,
        wall: i64,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (add, remove) = diff_colors(self.look.diff);
        let uid = turn * 4096 + at;
        let group = format!("sub-row-{uid}");
        let lead: AnyElement = match member.status {
            Status::Running => self.spin(now).into_any_element(),
            Status::Completed => theme_icon("check", 13.).text_color(add).into_any_element(),
            Status::Failed | Status::Stopped => {
                theme_icon("close", 13.).text_color(remove).into_any_element()
            }
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
            let background = member.backgroundable().map(|source| {
                (session.to_owned(), member.task.clone(), source.to_owned())
            });
            div()
                .absolute()
                // Clear of the elapsed time, which keeps ticking.
                .right(px(46.))
                .top(px((ROW - 26.) / 2.))
                .h(px(26.))
                .px(px(1.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(2.))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_MD))
                .border_1()
                .border_color(content(0.1))
                .opacity(0.)
                .group_hover(group.clone(), |s| s.opacity(1.))
                .child(
                    div()
                        .id(("sub-message", uid))
                        .size(px(24.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.07)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.compose_subagent(&message_session, &message_task, cx);
                        }))
                        .child(theme_icon("message-square", 13.).text_color(content(0.7))),
                )
                .children(background.map(|(bg_session, bg_task, bg_source)| {
                    div()
                        .id(("sub-background", uid))
                        .size(px(24.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.07)))
                        .tooltip(crate::shell::message_actions::name_tip(t(
                            "Enviar para segundo plano",
                        )))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.background_subagent(&bg_session, &bg_task, &bg_source, cx);
                        }))
                        .child(theme_icon("zap", 13.).text_color(content(0.7)))
                }))
                .child(
                    div()
                        .id(("sub-stop", uid))
                        .size(px(24.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.07)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.stop_subagent(&stop_session, &stop_task, cx);
                        }))
                        .child(theme_icon("square-fill", 10.).text_color(content(0.7))),
                )
        });
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
            .hover(|s| s.bg(content(0.07)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_feed(&open_session, &key, cx)
            }))
            .child(lead)
            .child(mark_tile(
                &crate::tab_tile::TileSize {
                    w: 16.,
                    h: 14.,
                    radius: 4.,
                },
                Face::provider(provider),
                provider_glyph(provider, 10.),
            ))
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px(220.))
                    .truncate()
                    .text_size(px(13.))
                    .text_color(content(0.9))
                    .child(name),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(content(0.4))
                    .child(member.step.clone()),
            )
            .when(member.tools > 0, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(content(0.4))
                        .child(steps_text(member.tools)),
                )
            })
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(52.))
                    .flex()
                    .justify_end()
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .children(elapsed),
            )
            .children(actions)
            .into_any_element()
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
                parts.push(self.compose_view(compose, open));
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

    fn compose_view(&self, compose: &Compose, open: f32) -> AnyElement {
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
                    .border_color(content(0.16))
                    .bg(field_fill())
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .opacity(if compose.busy { 0.6 } else { 1. })
                    .child(typed_text(
                        &compose.text,
                        t("Mensagem para este subagente"),
                        div()
                            .flex_shrink_0()
                            .w(px(1.5))
                            .h(px(15.))
                            .rounded_full()
                            .bg(crate::theme::caret_color()),
                    ))
                    .children(compose.error.clone().map(|error| {
                        div()
                            .max_w(px(260.))
                            .flex_shrink_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(remove)
                            .child(error)
                    })),
            )
            .into_any_element()
    }

    fn feed_view(&self, feed: &Feed, member: &Member, open: f32, cx: &mut Context<Self>) -> AnyElement {
        let (add, remove) = diff_colors(self.look.diff);
        let start = feed.entries.len().saturating_sub(feed.shown);
        let hidden = start;
        let mut rows: Vec<AnyElement> = Vec::new();
        if feed.dropped > 0 {
            rows.push(note(tf("{} entradas anteriores não são mais guardadas", &[&feed.dropped])));
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
                        if let Some(f) = this.subagents.feed.as_mut().filter(|f| f.session == session && f.key == key) {
                            f.shown += PAGE;
                        }
                        cx.notify();
                    }))
                    .child(tf("Mostrar {} anteriores", &[&hidden.min(PAGE)]))
                    .into_any_element(),
            );
        }
        for (at, entry) in feed.entries[start..].iter().enumerate() {
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
                    .border_color(content(0.08))
                    .overflow_y_scroll()
                    .track_scroll(&feed.scroll)
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(rows),
            )
            .into_any_element()
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
                .child(div().text_size(px(11.)).text_color(content(0.4)).child(t("Tarefa")))
                .child(div().line_clamp(4).child(entry.text.clone()))
                .into_any_element(),
            Kind::Text => div()
                .px(px(6.))
                .flex_shrink_0()
                .text_color(content(0.75))
                .child(crate::markdown::render(&entry.text))
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
                    Status::Failed | Status::Stopped => {
                        theme_icon("close", 12.).text_color(remove).into_any_element()
                    }
                    Status::Completed => theme_icon("check", 12.).text_color(add).into_any_element(),
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
                                if let Some(f) = this.subagents.feed.as_mut().filter(|f| f.session == session && f.key == key) {
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
                            .child(div().flex_shrink_0().max_w(px(160.)).truncate().child(entry.label.clone()))
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
    fn background_subagent(&mut self, session: &str, task: &str, source: &str, cx: &mut Context<Self>) {
        let call = self.backend.invoke(
            "background_subagent",
            background_args(session, task, source),
        );
        self.waiting.insert(call, Waiting::Subagent(Call::Background));
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
        self.subagents.feed = Some(Feed {
            session: session.to_owned(),
            key: key.to_owned(),
            entries: Vec::new(),
            dropped: 0,
            loading: true,
            shown: PAGE,
            opened: HashSet::new(),
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

    pub(super) fn subagent_reply(&mut self, call: Call, ok: bool, value: &Value, cx: &mut Context<Self>) {
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
                .rounded_full()
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
        let p = team::Progress { done: 2, total: 5, running: 3, failed: 0, background: 0 };
        assert_eq!(summary(&p), "2 de 5 concluídos · 3 em execução");
        let p = team::Progress { done: 5, total: 5, running: 0, failed: 1, background: 0 };
        assert_eq!(summary(&p), "5 de 5 concluídos · 1 falhou");
        let p = team::Progress { done: 4, total: 4, running: 0, failed: 2, background: 0 };
        assert_eq!(summary(&p), "4 de 4 concluídos · 2 falharam");
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
