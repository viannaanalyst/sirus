//! Cards for other conversations: when a
//! conversation the window is not showing finishes, fails, asks to run something or asks a
//! question, a card slides in at the top right, under the title bar. Newest on top, three at
//! most and then a "+N avisos" line. A permission card approves or denies right there with the
//! command the transcript uses; a question card opens the conversation (Responder also puts
//! the cursor in its composer); a result card opens it. Results stay until they are clicked,
//! closed or their conversation is opened; permission and question cards stay until answered, and
//! go when the request is gone from the conversation (answered on the phone) or when the
//! conversation is opened. Project
//! rows carry a small badge while something is waiting or unseen.
//!
//! The rules are plain functions and a plain `State` so they are tested without a window; the
//! rest of the file draws them. This module is a child of `shell` (`#[path]`), so it reaches the
//! window's private state.

use super::*;
use gpui::AnimationExt;
use std::time::Duration;

/// Cards drawn at once; the rest are counted.
pub(crate) const MAX_VISIBLE: usize = 3;
/// Cards kept in all; the oldest results go first.
const MAX_KEPT: usize = 24;
/// A request card is believed for this long before the conversation shows the request: the
/// host's alert and its session snapshot travel as two events.
pub(crate) const GRACE: Duration = Duration::from_millis(3200);
/// Event ids remembered, so a repeated event does not stack a second card.
const REMEMBERED: usize = 64;
const WIDTH: f32 = 340.;
const SLIDE: f32 = 26.;
const SLIDE_TIME: f32 = 0.26;

pub(super) const AMBER: u32 = 0xf0a35e;
pub(super) const GREEN: u32 = 0x5fce8c;
pub(super) const RED: u32 = 0xee8a96;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Permission,
    Question,
    Finished,
    Failed,
}

impl Kind {
    /// The host's `notification-activity` kinds that get a card; reminders and pull requests
    /// have their own places.
    fn of_event(name: &str) -> Option<Kind> {
        Some(match name {
            "permission" => Kind::Permission,
            "question" => Kind::Question,
            "completion" => Kind::Finished,
            "failure" => Kind::Failed,
            _ => return None,
        })
    }

    /// Stays until the request is answered.
    pub(crate) fn waits(self) -> bool {
        matches!(self, Kind::Permission | Kind::Question)
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Permission => t("Precisa de você"),
            Kind::Question => t("Tem uma pergunta"),
            Kind::Finished => t("Terminou"),
            Kind::Failed => t("Falhou"),
        }
    }

    fn color(self) -> gpui::Hsla {
        match self {
            Kind::Permission => rgb(AMBER).into(),
            Kind::Question => accent(),
            Kind::Finished => rgb(GREEN).into(),
            Kind::Failed => rgb(RED).into(),
        }
    }
}

/// One `notification-activity` payload, reduced to what a card needs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Event {
    pub id: String,
    pub session: String,
    pub kind: Kind,
    pub detail: String,
}

impl Event {
    pub(crate) fn parse(payload: &Value) -> Option<Event> {
        let kind = Kind::of_event(payload["kind"].as_str()?)?;
        let session = payload["sessionId"].as_str().filter(|s| !s.is_empty())?;
        Some(Event {
            id: payload["id"].as_str().filter(|s| !s.is_empty())?.to_owned(),
            session: session.to_owned(),
            kind,
            detail: payload["detail"].as_str().unwrap_or("").to_owned(),
        })
    }

    /// The request a permission or question names: the host's key ends with its id.
    fn request_id(&self) -> &str {
        self.id.rsplit(':').next().unwrap_or("")
    }
}

pub(crate) struct Card {
    pub event: Event,
    /// Gives the card its element ids, stable while others come and go.
    serial: u64,
    at: Instant,
    /// The conversation has listed the request at least once.
    listed: bool,
}

/// "Avisos no app" on? Missing means on, as the host's default.
pub(crate) fn toasts_on(settings: &Value) -> bool {
    settings["notifications"]["toasts"].as_bool() != Some(false)
}

/// Whether an event gets a card: not when the person is looking at that conversation right now
/// (the window in front and the conversation on screen), and not with the setting off.
pub(crate) fn shows(toasts: bool, window_active: bool, session_visible: bool) -> bool {
    toasts && !(window_active && session_visible)
}

/// The sound the card plays, if any. With the window in front the host's own alert is silent
/// (`native_notify::deliver`) unless "foreground" is on, in which case it plays itself; with the
/// window behind, that alert plays and the card stays quiet.
pub(crate) fn card_sound(settings: &Value, kind: Kind, window_active: bool) -> Option<String> {
    let prefs = &settings["notifications"];
    if !window_active
        || prefs["sounds"].as_bool() == Some(false)
        || prefs["foreground"].as_bool() == Some(true)
    {
        return None;
    }
    let (key, default) = match kind {
        Kind::Permission => ("permissionSound", "ping"),
        Kind::Question => ("questionSound", "pop"),
        Kind::Finished | Kind::Failed => ("completionSound", "glass"),
    };
    Some(prefs[key].as_str().unwrap_or(default).to_owned())
}

#[derive(Default)]
pub(crate) struct State {
    /// Oldest first.
    cards: Vec<Card>,
    remembered: VecDeque<String>,
    /// Sessions with a result nobody has looked at, for the project badge.
    results: Vec<(String, Kind)>,
    serial: u64,
}

impl State {
    /// Takes an event the rules let through. False for a repeat or a hidden event.
    pub(crate) fn admit(&mut self, event: Event, show: bool, now: Instant) -> bool {
        if self.remembered.contains(&event.id) {
            return false;
        }
        if self.remembered.len() == REMEMBERED {
            self.remembered.pop_front();
        }
        self.remembered.push_back(event.id.clone());
        if !show {
            return false;
        }
        if !event.kind.waits() {
            // A newer result replaces an older one of the same conversation.
            self.cards
                .retain(|c| c.event.kind.waits() || c.event.session != event.session);
            self.results
                .retain(|(session, _)| *session != event.session);
            self.results.push((event.session.clone(), event.kind));
        }
        self.serial += 1;
        self.cards.push(Card {
            event,
            serial: self.serial,
            at: now,
            listed: false,
        });
        while self.cards.len() > MAX_KEPT {
            let drop = self
                .cards
                .iter()
                .position(|c| !c.event.kind.waits())
                .unwrap_or(0);
            self.cards.remove(drop);
        }
        true
    }

    /// Request cards go once the conversation no longer lists their request (answered
    /// elsewhere), or when it never listed it within the grace time. `pending` says whether
    /// a conversation lists a request.
    pub(crate) fn settle(&mut self, now: Instant, pending: impl Fn(&str, &str) -> bool) -> bool {
        let before = self.cards.len();
        self.cards.retain_mut(|card| {
            if !card.event.kind.waits() {
                return true;
            }
            if pending(&card.event.session, card.event.request_id()) {
                card.listed = true;
                return true;
            }
            !(card.listed || now.saturating_duration_since(card.at) >= GRACE)
        });
        before != self.cards.len()
    }

    /// The person is looking at these conversations: their cards and unseen results go.
    pub(crate) fn seen(&mut self, sessions: &[String]) -> bool {
        let before = (self.cards.len(), self.results.len());
        self.cards.retain(|c| !sessions.contains(&c.event.session));
        self.results
            .retain(|(session, _)| !sessions.contains(session));
        before != (self.cards.len(), self.results.len())
    }

    pub(crate) fn dismiss(&mut self, id: &str) {
        self.cards.retain(|c| c.event.id != id);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// The cards to draw, newest first, and how many more there are. `known` keeps out cards
    /// whose conversation the window does not have (yet).
    pub(crate) fn stack(&self, known: impl Fn(&str) -> bool) -> (Vec<&Card>, usize) {
        let mut all: Vec<&Card> = self
            .cards
            .iter()
            .rev()
            .filter(|c| known(&c.event.session))
            .collect();
        let more = all.len().saturating_sub(MAX_VISIBLE);
        all.truncate(MAX_VISIBLE);
        (all, more)
    }

    /// The unseen result of a conversation.
    pub(super) fn result_of(&self, session: &str) -> Option<Kind> {
        self.results
            .iter()
            .find(|(s, _)| s == session)
            .map(|(_, k)| *k)
    }
}

/// What a project row shows, the most urgent first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Badge {
    Permission(usize),
    Question(usize),
    Failed,
    Finished,
}

/// Requests waiting for an approval and for an answer.
pub(crate) fn pending_counts(pending: &[Value]) -> (usize, usize) {
    let questions = pending
        .iter()
        .filter(|r| r["kind"]["type"] == "userInput")
        .count();
    (pending.len() - questions, questions)
}

/// The badge for a project's counts; none when nothing waits.
pub(crate) fn badge(
    permissions: usize,
    questions: usize,
    failed: bool,
    finished: bool,
) -> Option<Badge> {
    if permissions > 0 {
        Some(Badge::Permission(permissions))
    } else if questions > 0 {
        Some(Badge::Question(questions))
    } else if failed {
        Some(Badge::Failed)
    } else if finished {
        Some(Badge::Finished)
    } else {
        None
    }
}

impl Shell {
    /// Every conversation on screen: the active one and the panes beside it.
    pub(super) fn visible_sessions(&self) -> Vec<String> {
        let mut sessions = self.split.sessions();
        let active = self.active_session_id();
        if !active.is_empty() && !sessions.contains(&active) {
            sessions.push(active);
        }
        sessions
    }

    /// Whether a project is silenced now: no cards, no sounds, no badge. What it left unread stays.
    pub(super) fn project_muted(&self, project: usize) -> bool {
        self.projects
            .get(project)
            .and_then(|project| self.settings["mutedProjects"][project.id.as_str()].as_i64())
            .is_some_and(|until| until > chrono::Utc::now().timestamp_millis())
    }

    /// Silences a project for `hours`, or until it is turned back on with `None`; `Some(0)` lifts it.
    pub(super) fn mute_project(
        &mut self,
        project: usize,
        hours: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.projects.get(project).map(|project| project.id.clone()) else {
            return;
        };
        let now = chrono::Utc::now().timestamp_millis();
        let mut list = self.settings["mutedProjects"].clone();
        if !list.is_object() {
            list = json!({});
        }
        let kept = list.as_object_mut().expect("just made an object");
        kept.retain(|_, until| until.as_i64().is_some_and(|until| until > now));
        match hours {
            Some(0) => {
                kept.remove(&id);
            }
            Some(hours) => {
                kept.insert(id, json!(now + hours * 3_600_000));
            }
            None => {
                kept.insert(id, json!(i64::MAX));
            }
        }
        self.save_setting("mutedProjects", list, cx);
    }

    /// `notification-activity` from the host.
    pub(super) fn activity_event(&mut self, payload: &Value, cx: &mut Context<Self>) {
        let Some(event) = Event::parse(payload) else {
            return;
        };
        let visible = self.visible_sessions().contains(&event.session);
        let show = shows(toasts_on(&self.settings), self.window_active, visible);
        let kind = event.kind;
        if !self.cards.admit(event, show, Instant::now()) {
            return;
        }
        // The host's alert is silent while the window is in front; a card is not.
        if let Some(sound) = card_sound(&self.settings, kind, self.window_active) {
            crate::native_notify::play(&sound);
        }
        // A request is checked against the conversation once; a result stays until it's seen.
        if !kind.waits() {
            cx.notify();
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(GRACE + Duration::from_millis(50))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cards_settle();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Drops what has run its course: results past their time and requests no longer listed.
    pub(super) fn cards_settle(&mut self) {
        let now = Instant::now();
        let conversations = &self.conversations;
        self.cards.settle(now, |session, request| {
            conversations
                .iter()
                .find(|c| c.session_id == session)
                .is_some_and(|c| c.pending.iter().any(|r| r["requestId"] == request))
        });
    }

    /// Run on each frame: what the person is looking at needs no card or badge.
    pub(super) fn cards_seen(&mut self) {
        if self.window_active {
            let visible = self.visible_sessions();
            self.cards.seen(&visible);
        }
    }

    pub(super) fn cards_visible(&self) -> bool {
        !self.cards.is_empty()
    }

    fn card_conversation(&self, session: &str) -> Option<usize> {
        self.conversations
            .iter()
            .position(|c| !session.is_empty() && c.session_id == session)
    }

    /// The badge of a project row: waiting requests and unseen results of conversations that are
    /// not on screen. Nothing with "Avisos no app" off.
    pub(super) fn project_badge(&self, project: usize) -> Option<Badge> {
        if !toasts_on(&self.settings) || self.project_muted(project) {
            return None;
        }
        let visible = if self.window_active {
            self.visible_sessions()
        } else {
            Vec::new()
        };
        let (mut permissions, mut questions) = (0, 0);
        let (mut failed, mut finished) = (false, false);
        for ix in &self.projects.get(project)?.conversations {
            let conversation = &self.conversations[*ix];
            if visible.contains(&conversation.session_id) {
                continue;
            }
            let (p, q) = pending_counts(&conversation.pending);
            permissions += p;
            questions += q;
            match self.cards.result_of(&conversation.session_id) {
                Some(Kind::Failed) => failed = true,
                Some(_) => finished = true,
                None => {}
            }
        }
        badge(permissions, questions, failed, finished)
    }

    /// The badge for every project but the current one, on the sidebar's project switcher.
    pub(super) fn other_projects_badge(&self) -> Option<Badge> {
        let rank = |b: &Badge| match b {
            Badge::Permission(_) => 0,
            Badge::Question(_) => 1,
            Badge::Failed => 2,
            Badge::Finished => 3,
        };
        (0..self.projects.len())
            .filter(|p| Some(*p) != self.current_project)
            .filter_map(|p| self.project_badge(p))
            .min_by_key(rank)
    }

    pub(super) fn badge_view(&self, badge: Badge) -> AnyElement {
        let (fill, ink) = match badge {
            Badge::Permission(_) => (rgb(AMBER).into(), rgb(0x1c1306).into()),
            Badge::Question(_) => (accent(), on_accent()),
            Badge::Failed => (rgb(RED).into(), rgb(0x1c0a0d).into()),
            Badge::Finished => (rgb(GREEN).into(), rgb(0x0b2415).into()),
        };
        div()
            .flex_shrink_0()
            .min_w(px(15.))
            .h(px(15.))
            .px(px(4.))
            .rounded_full()
            .bg(fill)
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(10.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ink)
            .child(match badge {
                Badge::Permission(n) | Badge::Question(n) => {
                    div().child(n.min(99).to_string()).into_any_element()
                }
                Badge::Finished => icon("check", 9.).text_color(ink).into_any_element(),
                Badge::Failed => icon("close", 8.).text_color(ink).into_any_element(),
            })
            .into_any_element()
    }

    /// The request a card answers, as the conversation lists it now.
    fn card_request(&self, ix: usize, request_id: &str) -> Option<Value> {
        let pending = &self.conversations[ix].pending;
        pending
            .iter()
            .find(|r| r["requestId"] == request_id)
            .or_else(|| pending.iter().find(|r| r["kind"]["type"] != "userInput"))
            .cloned()
    }

    /// Approve or deny from the card, with the command the transcript's buttons use.
    fn card_decide(&mut self, id: &str, decision: &str, cx: &mut Context<Self>) {
        let Some((session, request_id)) = self
            .cards
            .cards
            .iter()
            .find(|c| c.event.id == id)
            .map(|c| (c.event.session.clone(), c.event.request_id().to_owned()))
        else {
            return;
        };
        if let Some(ix) = self.card_conversation(&session) {
            if let Some(request) = self.card_request(ix, &request_id) {
                self.respond(ix, &request, decision);
            }
        }
        self.cards.dismiss(id);
        cx.notify();
    }

    /// Opens the card's conversation; `answer` also hands the keys to its composer.
    fn card_open(&mut self, id: &str, answer: bool, window: &mut Window, cx: &mut Context<Self>) {
        let session = self
            .cards
            .cards
            .iter()
            .find(|c| c.event.id == id)
            .map(|c| c.event.session.clone());
        self.cards.dismiss(id);
        if let Some(ix) = session.and_then(|s| self.card_conversation(&s)) {
            self.open(ix, cx);
            if answer {
                window.focus(&self.focus, cx);
            }
        }
        cx.notify();
    }

    /// The stack under the title bar, or nothing.
    pub(super) fn cards_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (shown, more) = self.cards.stack(|session| {
            self.card_conversation(session)
                .is_some_and(|ix| self.project_of(ix).is_some())
        });
        if shown.is_empty() {
            return None;
        }
        let reduced = cx.reduce_motion();
        let mut stack = div()
            .absolute()
            .top(px(crate::theme::TITLE_BAR + 6.))
            .right(px(14.))
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .gap(px(8.));
        for card in shown {
            if let Some(view) = self.card_view(card, reduced, cx) {
                stack = stack.child(view);
            }
        }
        if more > 0 {
            stack = stack.child(
                div()
                    .text_center()
                    .text_size(px(12.5))
                    .text_color(content(0.6))
                    .child(if more == 1 {
                        tf("+{} aviso", &[&more])
                    } else {
                        tf("+{} avisos", &[&more])
                    }),
            );
        }
        Some(stack.into_any_element())
    }

    fn card_view(&self, card: &Card, reduced: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let event = &card.event;
        let ix = self.card_conversation(&event.session)?;
        let conversation = &self.conversations[ix];
        let project = &self.projects[self.project_of(ix)?];
        let kind = event.kind;
        let key = card.serial as usize * 4;
        let button = |n: usize, label: &'static str, main: bool| {
            div()
                .id(("activity-card-button", key + n))
                .flex_1()
                .h(px(30.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .text_color(if main { on_accent() } else { content(0.9) })
                .bg(if main { accent() } else { content(0.09) })
                .hover(|s| s.opacity(0.88))
                .child(label)
        };
        let id = event.id.clone();
        let (a, b, d) = (id.clone(), id.clone(), id.clone());
        let actions = match kind {
            Kind::Permission => div()
                .flex()
                .gap(px(7.))
                .child(button(0, t("Negar"), false).on_click(
                    cx.listener(move |this, _, _, cx| this.card_decide(&a, "decline", cx)),
                ))
                .child(button(1, t("Aprovar"), true).on_click(
                    cx.listener(move |this, _, _, cx| this.card_decide(&b, "accept", cx)),
                )),
            Kind::Question => div()
                .flex()
                .gap(px(7.))
                .child(button(0, t("Abrir"), false).on_click(
                    cx.listener(move |this, _, window, cx| this.card_open(&a, false, window, cx)),
                ))
                .child(button(1, t("Responder"), true).on_click(
                    cx.listener(move |this, _, window, cx| this.card_open(&b, true, window, cx)),
                )),
            Kind::Finished | Kind::Failed => {
                div()
                    .flex()
                    .gap(px(7.))
                    .child(button(0, t("Abrir"), false).on_click(cx.listener(
                        move |this, _, window, cx| this.card_open(&a, false, window, cx),
                    )))
            }
        };
        let border = match kind {
            Kind::Permission => gpui::Hsla {
                a: 0.4,
                ..rgb(AMBER).into()
            },
            Kind::Question => accent_wash(0.45),
            _ => content(0.12),
        };
        let card_el = div()
            .id(("activity-card", key))
            .relative()
            .w_full()
            .p(px(12.))
            .rounded(px(crate::theme::RADIUS_LG))
            .border_1()
            .border_color(border)
            .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id(("activity-card-close", key))
                    .absolute()
                    .top(px(9.))
                    .right(px(9.))
                    .size(px(20.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.1)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.cards.dismiss(&d);
                        cx.notify();
                    }))
                    .child(icon("close", 10.).text_color(content(0.5))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_shrink_0()
                            .child(crate::project_avatar::project_avatar(
                                &project.name,
                                &project.appearance,
                                30.,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pr(px(18.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(content(0.97))
                                    .child(conversation.title.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(content(0.5))
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(4.))
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_color(kind.color())
                                                    .child(kind.label()),
                                            )
                                            .child(div().child("·"))
                                            .child(div().truncate().child(project.name.clone())),
                                    ),
                            ),
                    ),
            )
            .when(!event.detail.is_empty(), |d| {
                d.child(
                    div()
                        .mt(px(8.))
                        .text_size(px(13.))
                        .line_height(px(18.))
                        .line_clamp(2)
                        .text_color(content(0.82))
                        .when(kind == Kind::Permission, |d| {
                            d.font_family("Menlo").text_size(px(12.))
                        })
                        .child(event.detail.clone()),
                )
            })
            .child(div().mt(px(10.)).child(actions));
        Some(if reduced {
            card_el.into_any_element()
        } else {
            card_el
                .with_animation(
                    format!("activity-card-in-{}", card.serial),
                    gpui::Animation::new(Duration::from_secs_f32(SLIDE_TIME))
                        .with_easing(|t| eased(t, Curve::Expo)),
                    |el, t| el.opacity(t).left(px((1. - t) * SLIDE)),
                )
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(id: &str, session: &str, kind: Kind) -> Event {
        Event {
            id: id.to_owned(),
            session: session.to_owned(),
            kind,
            detail: String::new(),
        }
    }

    fn secs(base: Instant, s: u64) -> Instant {
        base + Duration::from_secs(s)
    }

    #[test]
    fn the_host_payload_becomes_an_event() {
        let payload = json!({
            "id": "s1:1:t1:r9", "sessionId": "s1", "kind": "permission",
            "title": "Permissão necessária", "body": "app · Publicar", "detail": "$ npm publish"
        });
        let parsed = Event::parse(&payload).unwrap();
        assert_eq!(parsed.kind, Kind::Permission);
        assert_eq!(parsed.request_id(), "r9");
        assert_eq!(parsed.detail, "$ npm publish");
        for (name, kind) in [
            ("question", Kind::Question),
            ("completion", Kind::Finished),
            ("failure", Kind::Failed),
        ] {
            let p = json!({ "id": "x", "sessionId": "s", "kind": name });
            assert_eq!(Event::parse(&p).unwrap().kind, kind);
        }
        // Reminders, pull requests and anything without a conversation get no card.
        for p in [
            json!({ "id": "x", "sessionId": "s", "kind": "reminder" }),
            json!({ "id": "x", "sessionId": "", "kind": "completion" }),
            json!({ "id": "x", "kind": "pullRequest" }),
            json!({ "sessionId": "s", "kind": "completion" }),
        ] {
            assert!(Event::parse(&p).is_none(), "{p}");
        }
    }

    #[test]
    fn a_conversation_on_screen_in_a_window_in_front_gets_no_card() {
        assert!(
            shows(true, true, false),
            "another conversation, window in front"
        );
        assert!(
            shows(true, false, true),
            "the window is behind: the person is away"
        );
        assert!(shows(true, false, false));
        assert!(!shows(true, true, true), "looking at it right now");
        assert!(!shows(false, false, false), "Avisos no app off");
    }

    #[test]
    fn a_repeated_event_stacks_once_and_a_hidden_one_is_still_remembered() {
        let now = Instant::now();
        let mut state = State::default();
        assert!(state.admit(event("a", "s1", Kind::Permission), true, now));
        assert!(!state.admit(event("a", "s1", Kind::Permission), true, now));
        assert!(!state.admit(event("b", "s2", Kind::Question), false, now));
        // Showing it later does not bring it back.
        assert!(!state.admit(event("b", "s2", Kind::Question), true, now));
        assert_eq!(state.stack(|_| true).0.len(), 1);
    }

    #[test]
    fn at_most_three_cards_show_newest_first_and_the_rest_are_counted() {
        let now = Instant::now();
        let mut state = State::default();
        for n in 0..5 {
            state.admit(
                event(&format!("e{n}"), &format!("s{n}"), Kind::Permission),
                true,
                now,
            );
        }
        let (shown, more) = state.stack(|_| true);
        let ids: Vec<&str> = shown.iter().map(|c| c.event.id.as_str()).collect();
        assert_eq!(ids, ["e4", "e3", "e2"]);
        assert_eq!(more, 2);
        // A conversation the window does not know is not drawn, nor counted.
        let (shown, more) = state.stack(|s| s != "s4");
        assert_eq!(shown[0].event.id, "e3");
        assert_eq!(more, 1);
    }

    #[test]
    fn the_cap_drops_old_results_before_requests() {
        let now = Instant::now();
        let mut state = State::default();
        state.admit(event("req", "s0", Kind::Permission), true, now);
        for n in 1..=MAX_KEPT + 3 {
            state.admit(
                event(&format!("r{n}"), &format!("s{n}"), Kind::Finished),
                true,
                now,
            );
        }
        assert_eq!(state.cards.len(), MAX_KEPT);
        assert!(state.cards.iter().any(|c| c.event.id == "req"));
        assert!(!state.cards.iter().any(|c| c.event.id == "r1"));
    }

    #[test]
    fn results_leave_after_six_seconds_and_requests_do_not() {
        let now = Instant::now();
        let mut state = State::default();
        state.admit(event("done", "s1", Kind::Finished), true, now);
        state.admit(event("bad", "s2", Kind::Failed), true, now);
        state.admit(event("ask", "s3", Kind::Question), true, now);
        state.admit(event("run", "s4", Kind::Permission), true, now);
        // Nothing leaves by time: results wait to be seen, requests to be answered.
        let left: Vec<&str> = state.cards.iter().map(|c| c.event.id.as_str()).collect();
        assert_eq!(left, ["done", "bad", "ask", "run"]);
        assert!(state.seen(&["s1".to_owned()]));
        assert_eq!(state.cards.len(), 3);
    }

    #[test]
    fn a_newer_result_replaces_the_older_one_of_its_conversation() {
        let now = Instant::now();
        let mut state = State::default();
        state.admit(event("one", "s1", Kind::Finished), true, now);
        state.admit(event("ask", "s1", Kind::Question), true, now);
        state.admit(event("two", "s1", Kind::Failed), true, now);
        let ids: Vec<&str> = state.cards.iter().map(|c| c.event.id.as_str()).collect();
        assert_eq!(
            ids,
            ["ask", "two"],
            "the question of that conversation stays"
        );
        assert_eq!(state.result_of("s1"), Some(Kind::Failed));
    }

    #[test]
    fn a_request_goes_when_it_is_answered_elsewhere() {
        let now = Instant::now();
        let mut state = State::default();
        state.admit(event("s1:1:t:r1", "s1", Kind::Permission), true, now);
        // Listed: the card stays, and now remembers that it was listed.
        assert!(!state.settle(secs(now, 1), |s, r| s == "s1" && r == "r1"));
        // The phone answered: the conversation no longer lists it, at once.
        assert!(state.settle(secs(now, 2), |_, _| false));
        assert!(state.is_empty());
    }

    #[test]
    fn a_request_not_yet_listed_is_believed_for_the_grace_time_only() {
        let now = Instant::now();
        let mut state = State::default();
        state.admit(event("s1:1:t:r1", "s1", Kind::Question), true, now);
        assert!(
            !state.settle(secs(now, 2), |_, _| false),
            "the snapshot may be on its way"
        );
        assert!(state.settle(secs(now, 4), |_, _| false), "it never came");
    }

    #[test]
    fn opening_a_conversation_clears_its_cards_and_unseen_results() {
        let now = Instant::now();
        let mut state = State::default();
        state.admit(event("a", "s1", Kind::Permission), true, now);
        state.admit(event("b", "s1", Kind::Finished), true, now);
        state.admit(event("c", "s2", Kind::Question), true, now);
        assert!(state.seen(&["s1".to_owned()]));
        let ids: Vec<&str> = state.cards.iter().map(|c| c.event.id.as_str()).collect();
        assert_eq!(ids, ["c"]);
        assert_eq!(state.result_of("s1"), None);
        assert!(!state.seen(&["s1".to_owned()]));
    }

    #[test]
    fn closing_a_card_keeps_the_unseen_result_for_the_badge() {
        let now = Instant::now();
        let mut state = State::default();
        state.admit(event("done", "s1", Kind::Finished), true, now);
        state.dismiss("done");
        assert!(state.is_empty());
        assert_eq!(state.result_of("s1"), Some(Kind::Finished));
    }

    #[test]
    fn the_badge_shows_the_most_urgent_thing() {
        assert_eq!(badge(2, 1, true, true), Some(Badge::Permission(2)));
        assert_eq!(badge(0, 3, true, true), Some(Badge::Question(3)));
        assert_eq!(badge(0, 0, true, true), Some(Badge::Failed));
        assert_eq!(badge(0, 0, false, true), Some(Badge::Finished));
        assert_eq!(badge(0, 0, false, false), None);
        let pending = [
            json!({ "kind": { "type": "command" } }),
            json!({ "kind": { "type": "userInput" } }),
            json!({ "kind": { "type": "fileChange" } }),
        ];
        assert_eq!(pending_counts(&pending), (2, 1));
    }

    #[test]
    fn the_setting_turns_everything_off() {
        assert!(toasts_on(&json!({})));
        assert!(toasts_on(&json!({ "notifications": { "toasts": true } })));
        assert!(!toasts_on(&json!({ "notifications": { "toasts": false } })));
    }

    #[test]
    fn the_card_plays_the_sound_the_host_alert_leaves_out() {
        let on = json!({ "notifications": { "sounds": true, "permissionSound": "tink" } });
        // Window in front: the host alert is silent, so the card plays the kind's sound.
        assert_eq!(
            card_sound(&on, Kind::Permission, true).as_deref(),
            Some("tink")
        );
        assert_eq!(
            card_sound(&on, Kind::Question, true).as_deref(),
            Some("pop")
        );
        assert_eq!(
            card_sound(&on, Kind::Finished, true).as_deref(),
            Some("glass")
        );
        assert_eq!(
            card_sound(&on, Kind::Failed, true).as_deref(),
            Some("glass")
        );
        // Window behind: the host alert plays it already.
        assert_eq!(card_sound(&on, Kind::Permission, false), None);
        // Sounds off, or "foreground" on (the host alert plays in front too).
        let off = json!({ "notifications": { "sounds": false } });
        assert_eq!(card_sound(&off, Kind::Permission, true), None);
        let both = json!({ "notifications": { "foreground": true } });
        assert_eq!(card_sound(&both, Kind::Permission, true), None);
        // Nothing saved: the host's defaults.
        assert_eq!(
            card_sound(&json!({}), Kind::Permission, true).as_deref(),
            Some("ping")
        );
    }
}
