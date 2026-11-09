//! A floating balloon over a maximized side panel: a pill at the bottom with the conversation's
//! latest activity, which grows into a drawer holding the real conversation (transcript,
//! approval cards and composer) so it can be followed and driven without leaving the panel.
//!
//! The rules are plain functions so they are tested without a window; the rest draws the pill
//! and the drawer. This module is a child of `shell` (`#[path]`), so it reaches the window's
//! private state. The drawer takes the transcript element the shell built for the main slot,
//! which is empty while the panel fills the card.

use super::*;
use crate::motion::Curve;

const PILL_W: f32 = 380.;
const PILL_H: f32 = 44.;
const BOTTOM: f32 = 16.;
const SIDE: f32 = 20.;
const DRAWER_W: f32 = 720.;
const DRAWER_MAX_H: f32 = 560.;
const GROW: f32 = 0.42;
/// A web view draws above GPUI, so the page leaves this strip free for the pill.
const PAGE_ROOM: f32 = BOTTOM + PILL_H + 12.;
/// How long a new request pulses on the pill before the drawer opens by itself.
const AUTO_DELAY: f32 = 0.9;

/// What the balloon says the conversation is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Phase {
    Working,
    Waiting,
    Done,
    Idle,
}

pub(super) struct State {
    pub(super) open: bool,
    /// 0 is the pill, 1 the drawer; the spring overshoots a little past 1.
    t: Tween,
    session: String,
    armed: bool,
    was_working: bool,
    finished: bool,
    /// Requests waiting when last looked at, to tell a new one from one already there.
    seen: usize,
    auto_at: Option<Instant>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            t: Tween::still(0., Instant::now()),
            session: String::new(),
            armed: false,
            was_working: false,
            finished: false,
            seen: 0,
            auto_at: None,
        }
    }
}

impl State {
    fn progress(&self, now: Instant) -> f32 {
        self.t.at(now).clamp(0., 1.08)
    }

    /// The drawer, or its growth or shrinking, is on screen.
    fn drawer_visible(&self, now: Instant) -> bool {
        self.open || self.progress(now) > 0.001
    }

    /// Runs the balloon to the drawer (`open`) or back to the pill, with the open's length and
    /// spring both ways. It starts from the progress on screen, so a reopen mid-collapse turns
    /// back without a jump.
    fn run(&mut self, open: bool, now: Instant, reduce_motion: bool) {
        self.open = open;
        self.t.set(
            if open { 1. } else { 0. },
            now,
            if reduce_motion { 0. } else { GROW },
            Curve::Spring,
        );
    }

    fn reset(&mut self, now: Instant) {
        self.open = false;
        self.t.snap(0., now);
        self.armed = false;
        self.auto_at = None;
        self.finished = false;
    }
}

/// The balloon exists while the panel is open and fills the card, over a conversation.
pub(super) fn shows(dock_open: bool, maximized: bool, has_conversation: bool) -> bool {
    dock_open && maximized && has_conversation
}

/// A request that was not there before, with the drawer shut, in a balloon that shows.
pub(super) fn auto_open_due(shows: bool, open: bool, waiting: usize, seen: usize) -> bool {
    shows && !open && waiting > seen
}

/// The delay after an arrival has run out and the request still waits.
pub(super) fn auto_open_ready(due: Option<Instant>, now: Instant, waiting: usize) -> bool {
    waiting > 0 && due.is_some_and(|at| now >= at)
}

pub(super) fn phase(working: bool, waiting: usize, finished: bool) -> Phase {
    if waiting > 0 {
        Phase::Waiting
    } else if working {
        Phase::Working
    } else if finished {
        Phase::Done
    } else {
        Phase::Idle
    }
}

/// The pill's one line: a request that waits, else the step running, else the last thing the
/// agent wrote, else a plain "working" while it works.
pub(super) fn status_line(
    pending: Option<String>,
    step: Option<String>,
    answer: &str,
    working: bool,
) -> String {
    if let Some(pending) = pending.filter(|p| !p.trim().is_empty()) {
        return pending;
    }
    if let Some(step) = step.filter(|s| !s.trim().is_empty()) {
        return format!("{}…", step.trim_end_matches('…'));
    }
    if let Some(line) = last_line(answer) {
        return line;
    }
    if working {
        return format!("{}…", t("Trabalhando"));
    }
    String::new()
}

/// The last non-empty line of a reply, with its markdown marks taken off.
fn last_line(answer: &str) -> Option<String> {
    answer.lines().rev().find_map(|line| {
        let line = line
            .trim()
            .trim_start_matches(['#', '>', '-', '*', ' '])
            .replace(['`', '*'], "");
        let line = line.trim();
        (!line.is_empty() && line != "---").then(|| line.to_owned())
    })
}

/// What a waiting request asks, as the composer's card words it.
fn pending_text(kind: &Value) -> String {
    match kind["type"].as_str() {
        Some("command") => tf("Executar: {}", &[&kind["command"].as_str().unwrap_or("")]),
        Some("fileChange") => t("Alterar arquivos").to_owned(),
        Some("tool") => tf(
            "Usar a ferramenta {}",
            &[&kind["name"].as_str().unwrap_or("")],
        ),
        _ => kind["questions"][0]["question"]
            .as_str()
            .unwrap_or(t("O agente fez uma pergunta."))
            .to_owned(),
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Shell {
    /// The drawer is up, or still shrinking, over a panel that fills the card: its web view has
    /// to step aside until the collapse ends.
    pub(super) fn balloon_covers_page(&self, now: Instant) -> bool {
        self.balloon.drawer_visible(now) && self.dock_open && self.dock_max.to > 0.5
    }

    pub(super) fn balloon_shows(&self) -> bool {
        shows(
            self.dock_open,
            self.dock_max.to > 0.5,
            !self.conversations.is_empty(),
        )
    }

    pub(super) fn balloon_open(&mut self, cx: &mut Context<Self>) {
        if !self.balloon_shows() || self.balloon.open {
            return;
        }
        let now = Instant::now();
        self.balloon.auto_at = None;
        self.balloon.run(true, now, cx.reduce_motion());
        let ix = self.active_conversation();
        self.glides.entry(ix).or_default().follow_fast();
        self.refocus = true;
        cx.notify();
    }

    pub(super) fn balloon_close(&mut self, cx: &mut Context<Self>) {
        if !self.balloon.open {
            return;
        }
        self.balloon.run(false, Instant::now(), cx.reduce_motion());
        self.refocus = true;
        cx.notify();
    }

    pub(super) fn balloon_toggle(&mut self, cx: &mut Context<Self>) {
        if self.balloon.open {
            self.balloon_close(cx);
        } else {
            self.balloon_open(cx);
        }
    }

    /// Keeps the state in step with the conversation: finished marks, new requests and the
    /// auto-open. Returns the number of requests waiting.
    fn balloon_tick(&mut self, now: Instant, moving: &mut bool, cx: &mut Context<Self>) -> usize {
        let ix = self.active_conversation();
        let conversation = &self.conversations[ix];
        let (permissions, questions) = activity_cards::pending_counts(&conversation.pending);
        let waiting = permissions + questions;
        let working = conversation.working();
        let session = conversation.session_id.clone();
        let state = &mut self.balloon;
        if state.session != session || !state.armed {
            // Another conversation, or the balloon just appeared: what waits already is not news.
            state.session = session;
            state.armed = true;
            state.seen = waiting;
            state.was_working = working;
            state.finished = false;
            state.auto_at = None;
        }
        if working {
            state.finished = false;
        } else if state.was_working {
            state.finished = true;
        }
        state.was_working = working;
        if auto_open_due(true, state.open, waiting, state.seen) && state.auto_at.is_none() {
            state.auto_at = Some(now + Duration::from_secs_f32(AUTO_DELAY));
        }
        state.seen = waiting;
        if waiting == 0 || state.open {
            state.auto_at = None;
        }
        if auto_open_ready(state.auto_at, now, waiting) {
            self.balloon_open(cx);
        }
        *moving |= !self.balloon.t.done(now)
            || self.balloon.auto_at.is_some()
            || (waiting > 0 && !self.balloon.open)
            || (working && !self.balloon.open);
        waiting
    }

    /// Puts the balloon over the card. The drawer takes `chat` (the conversation with its
    /// composer) and leaves an empty slot behind; the pill leaves it where it was.
    pub(super) fn balloon_view(
        &mut self,
        chat: AnyElement,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> (AnyElement, Option<AnyElement>) {
        if !self.balloon_shows() {
            self.balloon.reset(now);
            crate::panel_browser::set_bottom_room(0.);
            return (chat, None);
        }
        let waiting = self.balloon_tick(now, moving, cx);
        let ix = self.active_conversation();
        let p = self.balloon.progress(now);
        let open = self.balloon.open;
        crate::panel_browser::set_bottom_room(if self.balloon.drawer_visible(now) {
            0.
        } else {
            PAGE_ROOM
        });
        // Fades in with the panel filling the card.
        let presence = self.dock_max.at(now).clamp(0., 1.);
        let conversation = &self.conversations[ix];
        let phase = phase(conversation.working(), waiting, self.balloon.finished);
        let project = self
            .project_of(ix)
            .map(|p| self.projects[p].name.clone())
            .unwrap_or_default();
        let line = status_line(
            conversation
                .pending
                .first()
                .map(|request| pending_text(&request["kind"])),
            conversation
                .turns
                .last()
                .and_then(|turn| turn.steps.iter().rev().find(|step| step.running))
                .map(|step| {
                    if step.kind == "command" && !step.caption.is_empty() {
                        step.caption.clone()
                    } else {
                        crate::step_lines::text(&step_line(step))
                    }
                }),
            conversation
                .turns
                .last()
                .map_or("", |turn| turn.answer.as_str()),
            conversation.working(),
        );
        let (permissions, _) = activity_cards::pending_counts(&conversation.pending);
        let sub = match phase {
            Phase::Working => format!("{}…", t("Trabalhando")),
            Phase::Waiting if permissions > 0 => t("Esperando sua aprovação").to_owned(),
            Phase::Waiting => t("Esperando sua resposta").to_owned(),
            Phase::Done => t("Terminou agora").to_owned(),
            Phase::Idle => conversation.age(),
        };
        let title = conversation.title.clone();
        let provider = conversation.provider;
        let clock = (now.duration_since(self.epoch).as_secs_f32() / SPINNER_PERIOD).fract();
        let amber: gpui::Hsla = rgb(activity_cards::AMBER).into();
        let green: gpui::Hsla = rgb(activity_cards::GREEN).into();
        let mark = move || -> AnyElement {
            let slot = div()
                .size(px(16.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center();
            match phase {
                Phase::Working => slot.child(spinner_at(clock)).into_any_element(),
                Phase::Waiting => slot
                    .child(div().size(px(9.)).rounded_full().bg(amber))
                    .into_any_element(),
                Phase::Done => slot
                    .rounded_full()
                    .bg(green)
                    .child(icon("check", 12.).text_color(rgb(0x06210f)))
                    .into_any_element(),
                Phase::Idle => slot
                    .child(crate::tab_tile::provider_mark(provider, 14.))
                    .into_any_element(),
            }
        };
        let panel_w =
            (self.dock_full(self.dock_room(self.viewport_width, now), now) - 2.).max(200.);
        let panel_h = (self.viewport_height - TITLE_BAR - FOOTER - 8.).max(200.);
        let drawer_w = (panel_w - SIDE * 2.).min(DRAWER_W);
        let drawer_h = (panel_h * 0.62).min(DRAWER_MAX_H);
        let pill_w = (panel_w - SIDE * 2.).min(PILL_W);
        let width = lerp(pill_w, drawer_w, p);
        let height = lerp(PILL_H, drawer_h, p);
        let pulse = if waiting > 0 && !open {
            0.5 + 0.5 * (now.duration_since(self.epoch).as_secs_f32() * 3.9).sin()
        } else {
            0.
        };
        let border = if waiting > 0 && !open {
            gpui::Hsla {
                a: 0.3 + 0.5 * pulse,
                ..amber
            }
        } else {
            crate::theme::border()
        };

        let pill = (p < 0.5).then(|| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .pl(px(12.))
                .pr(px(8.))
                .flex()
                .items_center()
                .gap(px(10.))
                .opacity((1. - p * 2.2).clamp(0., 1.))
                .child(mark())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(13.))
                        .child(
                            div()
                                .max_w(px(96.))
                                .flex_shrink_0()
                                .truncate()
                                .text_color(content(0.5))
                                .child(project.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(content(0.85))
                                .child(line.clone()),
                        ),
                )
                .when(waiting > 0, |d| {
                    d.child(
                        div()
                            .min_w(px(20.))
                            .h(px(20.))
                            .px(px(6.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(amber)
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0x2a1606))
                            .child(waiting.to_string()),
                    )
                })
                .child(
                    div()
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .border_1()
                        .border_color(crate::theme::border())
                        .text_size(px(11.))
                        .text_color(content(0.45))
                        .child("⌘L"),
                )
        });

        let mut slot = chat;
        let mut drawer = None;
        if p > 0.3 {
            let header_button = |id: &'static str, glyph: &'static str| {
                div()
                    .id(id)
                    .size(px(26.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.1)))
                    .child(icon(glyph, 14.).text_color(content(0.65)))
            };
            let header = div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(9.))
                .px(px(12.))
                .py(px(10.))
                .border_b_1()
                .border_color(content(0.07))
                .child(mark())
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
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(content(0.9))
                                .child(title),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(content(0.5))
                                .child(format!("{project} · {sub}")),
                        ),
                )
                .child(
                    header_button("balloon-side", "sidebar-right").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.balloon_close(cx);
                            this.dock_max.set(
                                0.,
                                Instant::now(),
                                if cx.reduce_motion() { 0. } else { SIDEBAR },
                                Curve::Expo,
                            );
                            cx.notify();
                        },
                    )),
                )
                .child(
                    header_button("balloon-close", "chevron-down")
                        .on_click(cx.listener(|this, _, _, cx| this.balloon_close(cx))),
                );
            drawer = Some(
                div()
                    .absolute()
                    .bottom_0()
                    .left(gpui::relative(0.5))
                    .ml(px(-drawer_w / 2.))
                    .w(px(drawer_w))
                    .h(px(drawer_h))
                    .flex()
                    .flex_col()
                    .opacity(((p - 0.3) / 0.5).clamp(0., 1.))
                    .child(header)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(std::mem::replace(&mut slot, div().into_any_element())),
                    ),
            );
        }

        let balloon = div()
            .id("conversation-balloon")
            .relative()
            .w_full()
            .max_w(px(width))
            .h(px(height))
            .rounded(px(crate::theme::RADIUS_LG))
            .border_1()
            .border_color(border)
            .bg(crate::theme::opaque_floating())
            .shadow_lg()
            .overflow_hidden()
            .occlude()
            .when(!open && p < 0.5, |d| {
                d.cursor_pointer()
                    .hover(|s| s.border_color(accent_wash(0.6)))
                    .on_click(cx.listener(|this, _, _, cx| this.balloon_open(cx)))
            })
            .children(pill)
            .children(drawer);
        let scrim = (p > 0.001).then(|| {
            div()
                .id("balloon-scrim")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .bg(crate::theme::black(0.5 * p.min(1.)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.balloon_close(cx)),
                )
        });
        let overlay = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .opacity(presence)
            .children(scrim)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(BOTTOM))
                    .px(px(SIDE))
                    .flex()
                    .justify_center()
                    .child(balloon),
            )
            .into_any_element();
        (slot, Some(overlay))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_waiting_request_comes_first_in_the_line() {
        let line = status_line(
            Some("Executar: npm install".into()),
            Some("Leu shell.rs".into()),
            "Pronto.",
            true,
        );
        assert_eq!(line, "Executar: npm install");
    }

    #[test]
    fn the_running_step_beats_the_last_reply() {
        let line = status_line(None, Some("Editou app/page.tsx".into()), "Pronto.", true);
        assert_eq!(line, "Editou app/page.tsx…");
        let again = status_line(None, Some("Lendo…".into()), "", true);
        assert_eq!(again, "Lendo…");
    }

    #[test]
    fn with_no_step_the_last_written_line_shows_without_markdown() {
        let answer = "Criei a página.\n\n## Resumo\n- **Hero** com `botão`\n";
        let line = status_line(None, None, answer, false);
        assert_eq!(line, "Hero com botão");
        assert_eq!(status_line(None, None, "", false), "");
        assert_eq!(status_line(None, None, "  \n", true), "Trabalhando…");
    }

    #[test]
    fn a_request_reads_as_the_composer_words_it() {
        let kind = json!({ "type": "command", "command": "ls" });
        assert_eq!(pending_text(&kind), "Executar: ls");
        let ask = json!({ "type": "userInput", "questions": [{ "question": "Qual?" }] });
        assert_eq!(pending_text(&ask), "Qual?");
    }

    #[test]
    fn the_phase_follows_requests_then_work_then_the_finish() {
        assert_eq!(phase(true, 1, false), Phase::Waiting);
        assert_eq!(phase(true, 0, true), Phase::Working);
        assert_eq!(phase(false, 0, true), Phase::Done);
        assert_eq!(phase(false, 0, false), Phase::Idle);
    }

    #[test]
    fn the_balloon_shows_only_for_a_maximized_open_panel_over_a_conversation() {
        assert!(shows(true, true, true));
        assert!(!shows(false, true, true));
        assert!(!shows(true, false, true));
        assert!(!shows(true, true, false));
    }

    #[test]
    fn only_a_new_request_with_the_drawer_shut_opens_it() {
        assert!(auto_open_due(true, false, 1, 0));
        assert!(auto_open_due(true, false, 2, 1));
        assert!(!auto_open_due(true, false, 1, 1));
        assert!(!auto_open_due(true, true, 1, 0));
        assert!(!auto_open_due(false, false, 1, 0));
        assert!(!auto_open_due(true, false, 0, 0));
    }

    #[test]
    fn collapsing_runs_the_open_length_and_easing_back_into_the_pill() {
        let base = Instant::now();
        let mut state = State::default();
        state.run(true, base, false);
        let opened = state.progress(base + Duration::from_secs_f32(GROW));
        assert!((opened - 1.).abs() < 1e-3);

        // Closing from the middle of the open starts at the progress on screen, not at 1.
        let at = base + Duration::from_millis(120);
        let before = state.progress(at);
        state.run(false, at, false);
        assert!(!state.open);
        assert!(
            (state.progress(at) - before).abs() < 1e-4,
            "no jump on close"
        );
        let mut last = before;
        for ms in (0..=(GROW * 1000.) as u64).step_by(20) {
            let p = state.progress(at + Duration::from_millis(ms));
            assert!(p <= last + 1e-4, "the collapse only goes down");
            last = p;
        }
        // The drawer stays on screen while it shrinks, and the pill is back when it ends.
        assert!(state.drawer_visible(at + Duration::from_millis(200)));
        let end = at + Duration::from_secs_f32(GROW);
        assert!(state.progress(end) < 1e-3);
        assert!(!state.drawer_visible(end));
    }

    #[test]
    fn reopening_mid_collapse_turns_back_from_the_current_progress() {
        let base = Instant::now();
        let mut state = State::default();
        state.run(true, base, false);
        state.run(false, base + Duration::from_secs_f32(GROW), false);
        let at = base + Duration::from_secs_f32(GROW) + Duration::from_millis(150);
        let shrinking = state.progress(at);
        assert!(shrinking > 0.05 && shrinking < 0.95);

        state.run(true, at, false);
        assert!(state.open);
        assert!(
            (state.progress(at) - shrinking).abs() < 1e-4,
            "no jump on reopen"
        );
        assert!(state.progress(at + Duration::from_millis(60)) > shrinking);
        let end = at + Duration::from_secs_f32(GROW);
        assert!((state.progress(end) - 1.).abs() < 1e-3);
    }

    #[test]
    fn with_reduced_motion_the_balloon_changes_at_once() {
        let base = Instant::now();
        let mut state = State::default();
        state.run(true, base, true);
        assert!((state.progress(base) - 1.).abs() < 1e-6);
        state.run(false, base, true);
        assert!(state.progress(base) < 1e-6);
        assert!(!state.drawer_visible(base));
    }

    #[test]
    fn the_drawer_opens_after_the_delay_and_only_while_a_request_waits() {
        let base = Instant::now();
        let due = Some(base + Duration::from_millis(900));
        assert!(!auto_open_ready(due, base, 1));
        assert!(auto_open_ready(due, base + Duration::from_secs(1), 1));
        assert!(!auto_open_ready(due, base + Duration::from_secs(1), 0));
        assert!(!auto_open_ready(None, base + Duration::from_secs(1), 1));
    }
}
