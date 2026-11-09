//! The terminal's place in the window (see `terminal_place`): the strip under the conversation,
//! the tabs in the title bar and ⌘J / ⇧⌘J. A child module of the shell, so it reaches its fields.

use super::*;
use crate::panel_terminal::{TerminalGroup, confirm_close, needs_confirm, running_names};
use crate::pull_requests_page::{PullRequestsPage, PullsEvent};
use crate::terminal_place::{self, Action, Placement};
use gpui::{AnyView, App, Entity, Subscription};

pub(super) const BOTTOM_HEIGHT: f32 = 280.;
/// A terminal's key in the deck of pinned tabs, clear of the tabs' ids.
pub(super) const TERM_CARD: u64 = 1 << 62;
/// The Pull requests tab's key in the deck.
pub(super) const PULLS_CARD: u64 = 1 << 61;
/// The Tarefas tab's key in the deck.
pub(super) const TASKS_CARD: u64 = 1 << 60;
/// What the tab menu calls the Pull requests tab; terminal tabs count from 1.
const PULLS_TAB: u64 = 0;
/// The Tarefas tab's name in the same menus, clear of the terminals' counting.
pub(super) const TASKS_TAB: u64 = u64::MAX - 1;

struct TermTab {
    id: u64,
    view: Entity<TerminalGroup>,
    /// Lives in the deck of pinned tabs instead of the strip.
    pinned: bool,
    /// Its place in the strip, among the session tabs.
    place: PageTab,
}

pub(super) struct TermState {
    pub placement: Placement,
    tabs: Vec<TermTab>,
    next_id: u64,
    /// The title tab shown in place of the conversation.
    active: Option<u64>,
    /// The tab each project action last ran in, in the title bar's placement.
    action_tabs: HashMap<String, u64>,
    pub(super) bottom_open: bool,
    /// The plus button's menu in the title strip: left edge and fade-in.
    plus_menu: Option<(f32, Tween)>,
    /// The right-click menu of a terminal tab: which tab, where, and its fade-in.
    chip_menu: Option<(u64, gpui::Point<Pixels>, Tween)>,
    /// The Pull requests page, a tab among the session tabs in every placement (pull_requests_page.rs).
    pub(super) pulls: Option<Entity<PullRequestsPage>>,
    pub(super) pulls_active: bool,
    /// The Pull requests tab sits in the deck of pinned tabs.
    pulls_pinned: bool,
    /// Its tab in the strip of session tabs, drawn and laid out with them (see `layout_tabs`).
    pub(super) pulls_tab: Option<PageTab>,
    _pulls_events: Option<Subscription>,
    /// The Tarefas page (tasks_page.rs): a title tab like Pull requests, after it in the strip.
    pub(super) tasks_active: bool,
    pub(super) tasks_tab: Option<PageTab>,
    /// The Tarefas tab sits in the deck of pinned tabs.
    tasks_pinned: bool,
    /// The page tabs saved last, to write the file only when they change.
    pages_saved: Option<crate::page_tabs::Snapshot>,
    pages_restored: bool,
}

/// A page's tab among the session tabs: it is not a conversation, so it has no `Tab`.
pub(super) struct PageTab {
    pub x: Tween,
    pub w: Tween,
    pub closing: bool,
}

impl TermState {
    pub fn new() -> Self {
        Self {
            placement: terminal_place::current(),
            tabs: Vec::new(),
            next_id: 1,
            active: None,
            action_tabs: HashMap::new(),
            bottom_open: false,
            plus_menu: None,
            chip_menu: None,
            pulls: None,
            pulls_active: false,
            pulls_pinned: false,
            pulls_tab: None,
            _pulls_events: None,
            tasks_active: false,
            tasks_tab: None,
            tasks_pinned: false,
            pages_saved: None,
            pages_restored: false,
        }
    }
    pub fn tasks_in_strip(&self) -> bool {
        !self.tasks_pinned && self.tasks_tab.as_ref().is_some_and(|tab| !tab.closing)
    }
    pub fn tasks_in_deck(&self) -> bool {
        self.tasks_pinned && self.tasks_tab.as_ref().is_some_and(|tab| !tab.closing)
    }
    /// Pinned, closing or not: its plate is not in the strip to lay out.
    pub fn tasks_in_deck_now(&self) -> bool {
        self.tasks_pinned
    }
    /// Terminal tabs in the strip: one pulled from the panel stays a tab, in any placement.
    pub fn strip_count(&self) -> usize {
        self.tabs
            .iter()
            .filter(|tab| !tab.pinned && !tab.place.closing)
            .count()
    }
    pub fn pinned_ids(&self) -> Vec<u64> {
        self.tabs
            .iter()
            .filter(|tab| tab.pinned && !tab.place.closing)
            .map(|tab| tab.id)
            .collect()
    }
    /// Pinned, closing or not: its plate is not in the strip to lay out.
    pub fn pulls_in_deck_now(&self) -> bool {
        self.pulls_pinned
    }
    pub fn pulls_in_deck(&self) -> bool {
        self.pulls_pinned && self.pulls_tab.as_ref().is_some_and(|tab| !tab.closing)
    }
    pub fn pulls_in_strip(&self) -> bool {
        !self.pulls_pinned && self.pulls_tab.as_ref().is_some_and(|tab| !tab.closing)
    }
    /// A terminal tab is on screen in the conversation's place.
    pub fn terminal_showing(&self) -> bool {
        self.active.is_some() && !self.pulls_active
    }
    /// That terminal tab sits in the strip, so it takes the selected tab's width.
    pub fn terminal_showing_in_strip(&self) -> bool {
        !self.pulls_active
            && self
                .active
                .is_some_and(|id| self.tabs.iter().any(|t| t.id == id && !t.pinned))
    }
    /// The deck's key of the page tab showing, if any.
    pub fn active_card(&self) -> Option<u64> {
        if self.pulls_active {
            return Some(PULLS_CARD);
        }
        if self.tasks_active {
            return Some(TASKS_CARD);
        }
        self.active.map(|id| TERM_CARD + id)
    }
    /// Lays the terminal tabs after the session tabs; true while one moves.
    pub fn lay_out(
        &mut self,
        x: &mut f32,
        (selected, others): (f32, f32),
        (open, reorder): (f32, f32),
        now: Instant,
    ) -> bool {
        let mut moving = false;
        let showing = self.active.filter(|_| !self.pulls_active);
        for tab in self.tabs.iter_mut().filter(|tab| !tab.pinned) {
            let width = if tab.place.closing {
                0.
            } else if showing == Some(tab.id) {
                selected
            } else {
                others
            };
            tab.place.w.set(width, now, open, Curve::Quad);
            tab.place.x.set(*x, now, reorder, Curve::Expo);
            moving |= !tab.place.x.done(now) || !tab.place.w.done(now);
            if !tab.place.closing {
                *x += width + GAP;
            }
        }
        // Dropping the last handle to a tab ends its shells.
        self.tabs
            .retain(|tab| !(tab.place.closing && tab.place.w.done(now)));
        moving
    }
    pub fn close_menu(&mut self) -> bool {
        if self.chip_menu.take().is_some() {
            return true;
        }
        self.plus_menu.take().is_some()
    }
}

/// Which tabs a bulk close takes, from a page tab's menu.
#[derive(Clone, Copy)]
enum Around {
    Others,
    Left,
    Right,
}

/// A tab beside a page tab in the strip.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Beside {
    Session(u64),
    Page(u64),
}

/// The Pull requests tab's tile, in the strip and the deck.
pub(super) fn pulls_tile() -> (Face, AnyElement) {
    (
        Face::Neutral,
        crate::theme::icon("git-pull-request", 14.)
            .text_color(content(0.9))
            .into_any_element(),
    )
}

/// The Tarefas tab's tile, in the strip.
pub(super) fn tasks_tile() -> (Face, AnyElement) {
    (
        Face::Neutral,
        crate::theme::icon("task-clock", 14.)
            .text_color(content(0.9))
            .into_any_element(),
    )
}

impl Shell {
    pub(super) fn conversation_view(&self, kind: usize) -> Option<AnyView> {
        let session = self.conversations[self.active_conversation()]
            .session_id
            .clone();
        self.pane_views.get(&(kind, session)).cloned()
    }

    /// The side panel's Terminal option follows the placement: the strip or a title tab instead.
    /// True when it was handled here.
    pub(super) fn term_redirect(&mut self, cx: &mut Context<Self>) -> bool {
        match self.term.placement {
            Placement::Side => false,
            Placement::Bottom => {
                self.show_bottom(cx);
                true
            }
            Placement::TitleBar => {
                self.new_title_terminal(None, cx);
                true
            }
        }
    }

    fn show_bottom(&mut self, cx: &mut Context<Self>) {
        if self.this_mac_only(cx) {
            return;
        }
        self.ensure_pane_view(0, cx);
        self.term.bottom_open = true;
        cx.notify();
    }

    /// ⌘J and ⇧⌘J (Split Terminal Right).
    pub(super) fn term_shortcut(
        &mut self,
        split: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.this_mac_only(cx) {
            return;
        }
        let focused = self.term.active.is_some();
        match terminal_place::shortcut(self.term.placement, split, focused) {
            Action::OpenSide => self.open_pane(0, cx),
            Action::ToggleBottom => {
                if self.term.bottom_open {
                    self.term.bottom_open = false;
                    cx.notify();
                } else {
                    self.show_bottom(cx);
                    self.focus_bottom(window, cx);
                }
            }
            Action::NewTitleTab => self.new_title_terminal(Some(window), cx),
            Action::SplitTitleTab => {
                let view = self
                    .term
                    .tabs
                    .iter()
                    .find(|t| Some(t.id) == self.term.active)
                    .map(|t| t.view.clone());
                if let Some(view) = view {
                    view.update(cx, |group, cx| group.split(window, cx));
                }
            }
            Action::AddInPanel => {
                if self.term.placement == Placement::Bottom {
                    self.show_bottom(cx);
                    self.term.bottom_open = true;
                } else {
                    self.open_pane(0, cx);
                }
                if let Some(view) = self.conversation_view(0) {
                    crate::panel_terminal::add_in(&view, window, cx);
                }
            }
        }
        cx.stop_propagation();
    }

    fn focus_bottom(&self, window: &mut Window, cx: &mut App) {
        if let Some(view) = self.conversation_view(0) {
            crate::panel_terminal::focus_in(&view, window, cx);
        }
    }

    /// Settings → Geral → Terminal.
    pub(super) fn set_term_placement(&mut self, placement: Placement, cx: &mut Context<Self>) {
        self.term.placement = placement;
        self.term.plus_menu = None;
        if placement != Placement::TitleBar {
            self.term.active = None;
        }
        terminal_place::set(placement);
        cx.notify();
    }

    fn new_title_terminal(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if self.this_mac_only(cx) {
            return;
        }
        let conversation = &self.conversations[self.active_conversation()];
        let cwd = conversation.worktree.clone();
        let keys = crate::panel_terminal::KeySource {
            backend: self.backend.clone(),
            session_id: conversation.session_id.clone(),
        };
        let view = cx.new(|cx| TerminalGroup::new(cwd, keys, cx));
        let id = self.term.next_id;
        self.term.next_id += 1;
        if let Some(window) = window {
            view.update(cx, |group, cx| group.focus(window, cx));
        }
        let place = self.fresh_page_tab();
        self.term.tabs.push(TermTab {
            id,
            view,
            pinned: false,
            place,
        });
        self.term.active = Some(id);
        self.term.pulls_active = false;
        self.term.tasks_active = false;
        self.term.plus_menu = None;
        cx.notify();
    }

    /// A terminal dragged out of the panel becomes a title tab; its shell keeps running.
    pub(super) fn adopt_terminal(
        &mut self,
        drag: &crate::panel_terminal::TermTabDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let view = crate::panel_terminal::group_from_drag(drag, cx)?;
        view.update(cx, |group, cx| group.focus(window, cx));
        let id = self.term.next_id;
        self.term.next_id += 1;
        let place = self.fresh_page_tab();
        self.term.tabs.push(TermTab {
            id,
            view,
            pinned: false,
            place,
        });
        self.term.active = Some(id);
        self.term.pulls_active = false;
        self.term.tasks_active = false;
        cx.notify();
        Some(id)
    }

    /// Starts at the end of the strip with no width, then grows, like a new session.
    fn fresh_page_tab(&self) -> PageTab {
        let now = Instant::now();
        PageTab {
            x: Tween::still(self.new_tab_x.at(now).max(0.), now),
            w: Tween::still(0., now),
            closing: false,
        }
    }

    /// Shows a terminal tab in the conversation's place.
    pub(super) fn show_term_tab(&mut self, id: Option<u64>) {
        self.term.active = id;
    }

    /// The group of a terminal tab, for the pane that shows it beside a conversation.
    pub(super) fn term_view(&self, id: u64) -> Option<Entity<TerminalGroup>> {
        let tab = self
            .term
            .tabs
            .iter()
            .find(|tab| tab.id == id && !tab.place.closing)?;
        Some(tab.view.clone())
    }

    pub(super) fn term_title(&self, id: u64, cx: &App) -> String {
        self.term_view(id)
            .map_or_else(String::new, |view| view.read(cx).title())
    }

    /// The agent CLI holding terminal tab `id`, if any: its tab shows that provider's mark.
    pub(super) fn term_agent(&self, id: u64, cx: &App) -> Option<&'static str> {
        self.term_view(id).and_then(|view| view.read(cx).agent(cx))
    }

    /// What a terminal tab carries when it is dragged: the same drag as a conversation's pane.
    pub(super) fn term_drag(&self, id: u64, cx: &App) -> split_panes::ConvDrag {
        split_panes::ConvDrag {
            session: split_panes::term_session(id),
            title: self.term_title(id, cx),
        }
    }

    /// A title tab dropped on the side panel or the bottom strip goes back into the Terminal
    /// pane, shells and all, and its tab goes away.
    pub(super) fn terminal_to_panel(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.term_view(id) else {
            return;
        };
        if self.term.placement != Placement::Bottom {
            if !self.dock_panes.contains(&0) {
                self.dock_panes.push(0);
            }
            self.dock_active = Some(0);
        }
        self.ensure_pane_view(0, cx);
        let Some(view) = self.conversation_view(0) else {
            return;
        };
        if !crate::panel_terminal::adopt_group(&view, &group, window, cx) {
            return;
        }
        self.term.tabs.retain(|tab| tab.id != id);
        if self.term.active == Some(id) {
            self.term.active = None;
        }
        self.split_clear_aim();
        cx.notify();
    }

    /// Over the side panel or the strip while a terminal tab is dragged: the wash that says it
    /// can land there. It lets everything else through, and takes the drop.
    pub(super) fn term_drop_zone(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        cx.has_active_drag().then(|| {
            div()
                .id("term-drop-zone")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .drag_over::<split_panes::ConvDrag>(|style, drag, _, _| {
                    if split_panes::term_id(&drag.session).is_some() {
                        style.bg(crate::theme::accent_wash(0.16))
                    } else {
                        style
                    }
                })
                .on_drop(
                    cx.listener(|this, drag: &split_panes::ConvDrag, window, cx| {
                        if let Some(id) = split_panes::term_id(&drag.session) {
                            this.terminal_to_panel(id, window, cx);
                        }
                    }),
                )
                .into_any_element()
        })
    }

    pub(super) fn open_title_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        // Already beside a conversation: it stays there and takes the keys.
        self.term.active = (!self.split_shows_term(id)).then_some(id);
        self.term.pulls_active = false;
        self.term.tasks_active = false;
        self.term.plus_menu = None;
        if let Some(tab) = self.term.tabs.iter().find(|t| t.id == id) {
            tab.view.update(cx, |group, cx| group.focus(window, cx));
        }
        cx.notify();
    }

    pub(super) fn ask_chip_menu(
        &mut self,
        id: u64,
        at: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let length = if cx.reduce_motion() { 0. } else { 0.12 };
        self.term.chip_menu = Some((
            id,
            at,
            Tween::from_to(0., 1., Instant::now(), length, Curve::Expo),
        ));
        cx.notify();
    }

    /// A project action in the title bar's placement: a terminal tab of its own.
    pub(super) fn run_in_title_tab(
        &mut self,
        key: &str,
        command: &str,
        reveal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let idle = self.term.action_tabs.get(key).copied().filter(|id| {
            self.term_view(*id)
                .is_some_and(|view| view.read(cx).idle(cx))
        });
        let shown = self.term.active;
        let id = match idle {
            Some(id) => id,
            None => {
                self.new_title_terminal(None, cx);
                let id = self.term.next_id - 1;
                self.term.action_tabs.insert(key.to_owned(), id);
                id
            }
        };
        if let Some(view) = self.term_view(id) {
            view.update(cx, |group, cx| group.run(command, cx));
        }
        if reveal {
            self.open_title_tab(id, window, cx);
        } else {
            self.term.active = shown;
        }
    }

    /// A project action's terminal comes up where the placement keeps the pane.
    pub(super) fn term_reveal_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.term.placement {
            Placement::Bottom => {
                self.show_bottom(cx);
                self.focus_bottom(window, cx);
            }
            _ => self.open_pane(0, cx),
        }
    }

    /// The right-click menu of a terminal or Pull requests tab, with the session tabs' rows.
    fn term_chip_menu_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (id, at, open) = self.term.chip_menu.as_ref()?;
        let (id, at) = (*id, *at);
        *moving |= !open.done(now);
        let shown = open.at(now).clamp(0., 1.);
        let pinned = if id == PULLS_TAB {
            self.term.pulls_pinned
        } else if id == TASKS_TAB {
            self.term.tasks_pinned
        } else {
            self.term.tabs.iter().find(|tab| tab.id == id)?.pinned
        };
        let (left, right) = self.around_page(id);
        let others = !left.is_empty() || !right.is_empty();
        let track = crate::hover_track::list("term-chip-menu");
        let row = |key: &'static str, glyph: &'static str, label: &'static str| {
            div()
                .id(key)
                .relative()
                .h(px(26.))
                .px(px(8.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(9.))
                .text_size(px(13.))
                .text_color(content(0.9))
                .child(icon(glyph, 14.).text_color(content(0.65)))
                .child(label)
        };
        let bulk = |key: &'static str,
                    glyph: &'static str,
                    label: &'static str,
                    enabled: bool,
                    side: Around,
                    cx: &mut Context<Self>|
         -> AnyElement {
            if !enabled {
                return row(key, glyph, label).opacity(0.4).into_any_element();
            }
            track
                .row_named(row(key, glyph, label).cursor_pointer(), key, false)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.term.chip_menu = None;
                    this.close_around_page(id, side, window, cx);
                }))
                .into_any_element()
        };
        let line = || div().mx(px(8.)).my(px(4.)).h(px(1.)).bg(content(0.08));
        Some(
            div()
                .id("term-chip-menu")
                .occlude()
                .absolute()
                .left(at.x)
                .top(at.y + px(2.))
                .w(px(220.))
                .p(px(4.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .child(crate::theme::floating_surface(crate::theme::RADIUS_MD))
                .shadow_lg()
                .opacity(shown)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(track.pill())
                .child(
                    track
                        .row_named(
                            row(
                                "term-menu-pin",
                                "pin",
                                t(if pinned { "Desafixar aba" } else { "Fixar aba" }),
                            )
                            .cursor_pointer(),
                            "term-menu-pin",
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.term.chip_menu = None;
                            this.toggle_page_pin(id);
                            cx.notify();
                        })),
                )
                .child(
                    track
                        .row_named(
                            row("term-menu-close", "tab-close", t("Fechar aba")).cursor_pointer(),
                            "term-menu-close",
                            false,
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.term.chip_menu = None;
                            this.close_page(id, window, cx);
                        })),
                )
                .child(line())
                .child(bulk(
                    "term-menu-others",
                    "tabs-others",
                    t("Fechar outras abas"),
                    others,
                    Around::Others,
                    cx,
                ))
                .child(bulk(
                    "term-menu-right",
                    "tabs-right",
                    t("Fechar abas à direita"),
                    !right.is_empty(),
                    Around::Right,
                    cx,
                ))
                .child(bulk(
                    "term-menu-left",
                    "tabs-left",
                    t("Fechar abas à esquerda"),
                    !left.is_empty(),
                    Around::Left,
                    cx,
                ))
                .into_any_element(),
        )
    }

    fn toggle_page_pin(&mut self, id: u64) {
        if id == PULLS_TAB {
            self.term.pulls_pinned = !self.term.pulls_pinned;
        } else if id == TASKS_TAB {
            self.term.tasks_pinned = !self.term.tasks_pinned;
        } else if let Some(tab) = self.term.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.pinned = !tab.pinned;
        }
    }

    fn close_page(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if id == PULLS_TAB {
            self.close_pull_requests(cx);
        } else if id == TASKS_TAB {
            self.close_tasks_tab(cx);
        } else {
            self.close_title_tab(id, window, cx);
        }
    }

    /// The strip's tabs on each side of a page tab: session tabs come first, then Pull requests,
    /// then the terminals. A pinned page has no side; everything in the strip is on its left.
    fn around_page(&self, id: u64) -> (Vec<Beside>, Vec<Beside>) {
        let mut strip: Vec<Beside> = self
            .tabs
            .iter()
            .filter(|t| !t.pinned && !t.closing)
            .map(|t| Beside::Session(t.id))
            .collect();
        if self.term.pulls_in_strip() {
            strip.push(Beside::Page(PULLS_TAB));
        }
        if self.term.tasks_in_strip() {
            strip.push(Beside::Page(TASKS_TAB));
        }
        strip.extend(
            self.term
                .tabs
                .iter()
                .filter(|tab| !tab.pinned && !tab.place.closing)
                .map(|tab| Beside::Page(tab.id)),
        );
        match strip.iter().position(|b| *b == Beside::Page(id)) {
            Some(at) => {
                let right = strip.split_off(at + 1);
                strip.pop();
                (strip, right)
            }
            None => (strip, Vec::new()),
        }
    }

    fn close_around_page(
        &mut self,
        id: u64,
        side: Around,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (left, right) = self.around_page(id);
        let doomed: Vec<Beside> = match side {
            Around::Others => left.into_iter().chain(right).collect(),
            Around::Left => left,
            Around::Right => right,
        };
        let mut terminals = Vec::new();
        for tab in doomed {
            match tab {
                Beside::Session(session) => self.close(session, cx),
                Beside::Page(PULLS_TAB) => self.close_pull_requests(cx),
                Beside::Page(TASKS_TAB) => self.close_tasks_tab(cx),
                Beside::Page(terminal) => terminals.push(terminal),
            }
        }
        if !terminals.is_empty() {
            self.close_title_tabs(terminals, window, cx);
        }
        cx.notify();
    }

    /// Closes title tab `id`, asking first when a command runs in any of its shells.
    fn close_title_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.close_title_tabs(vec![id], window, cx);
    }

    /// Bulk close: one popup lists every running terminal; Cancel keeps all the tabs.
    fn close_title_tabs(&mut self, ids: Vec<u64>, window: &mut Window, cx: &mut Context<Self>) {
        let mut running = Vec::new();
        for tab in self.term.tabs.iter().filter(|t| ids.contains(&t.id)) {
            let state = tab.view.read(cx).panes_state(cx);
            running.extend(running_names(&state));
        }
        if !needs_confirm(window, cx, &running) {
            self.drop_title_tabs(&ids, cx);
            return;
        }
        let shell = cx.entity();
        confirm_close(window, cx, &running, move |cx: &mut App| {
            shell.update(cx, |this, cx| this.drop_title_tabs(&ids, cx));
        });
    }

    fn drop_title_tabs(&mut self, ids: &[u64], cx: &mut Context<Self>) {
        // A pinned one has no plate to shrink; the others go when theirs has.
        self.term
            .tabs
            .retain(|t| !(ids.contains(&t.id) && t.pinned));
        for tab in &mut self.term.tabs {
            if ids.contains(&tab.id) {
                tab.place.closing = true;
            }
        }
        if self.term.active.is_some_and(|a| ids.contains(&a)) {
            self.term.active = self
                .term
                .tabs
                .iter()
                .rev()
                .find(|t| !t.place.closing)
                .map(|t| t.id);
        }
        cx.notify();
    }

    /// Drops the title tabs whose shells belong to `session`, for a settled conversation (ADR-198).
    /// The caller has checked that the conversation is idle and not on screen.
    pub(super) fn drop_session_title_tabs(&mut self, session: &str, cx: &mut Context<Self>) {
        let ids: Vec<u64> = self
            .term
            .tabs
            .iter()
            .filter(|tab| tab.view.read(cx).session() == session)
            .map(|tab| tab.id)
            .collect();
        if !ids.is_empty() {
            self.drop_title_tabs(&ids, cx);
        }
    }

    pub(super) fn leave_title_tab(&mut self) {
        self.term.active = None;
        self.term.pulls_active = false;
        self.term.tasks_active = false;
        // The modal and the popover belong to the page while it shows.
        self.tasks_page.sheet = None;
        self.tasks_page.popover = None;
    }

    /// Opens the Pull requests page as a title tab, or goes back to it when it is already one.
    pub(super) fn open_pull_requests(&mut self, cx: &mut Context<Self>) {
        if self.term.pulls.is_none() {
            let backend = self.backend.clone();
            let project = self.sidebar_project_id();
            let view = cx.new(|cx| PullRequestsPage::new(backend, project, cx));
            self.term._pulls_events = Some(cx.subscribe(
                &view,
                |this: &mut Shell, _, event: &PullsEvent, _| match event {
                    // The reply is a session: it opens as a conversation, like a new one.
                    PullsEvent::Ask(args) => {
                        let id = this.backend.invoke("pull_ask_action", args.clone());
                        this.waiting.insert(id, Waiting::Created);
                    }
                },
            ));
            self.term.pulls = Some(view);
        } else if let Some(view) = self.term.pulls.clone() {
            self.sync_pulls_project(cx);
            view.update(cx, |page, cx| page.reload(cx));
        }
        match self.term.pulls_tab.as_mut() {
            Some(tab) => tab.closing = false,
            None => {
                self.term.pulls_tab = Some(self.fresh_page_tab());
            }
        }
        self.term.active = None;
        self.term.pulls_active = true;
        self.term.tasks_active = false;
        self.term.plus_menu = None;
        self.palette_ui = None;
        cx.notify();
    }

    /// Closes the page's tab. If it was the one showing, the neighbour in the strip takes over:
    /// the tab to its left (it is the last one), or the conversation it came from.
    pub(super) fn close_pull_requests(&mut self, cx: &mut Context<Self>) {
        let was_active = self.term.pulls_active;
        match self.term.pulls_tab.as_mut() {
            Some(tab) if !tab.closing => tab.closing = true,
            _ => return,
        }
        self.term.pulls = None;
        self.term._pulls_events = None;
        self.term.pulls_active = false;
        self.term.pulls_pinned = false;
        if was_active {
            let strip: Vec<u64> = self
                .tabs
                .iter()
                .filter(|t| !t.pinned && !t.closing)
                .map(|t| t.id)
                .collect();
            let next = neighbour_on_close(strip.len() + 1, strip.len())
                .and_then(|at| strip.get(at).copied())
                .unwrap_or(self.active);
            self.select(next, cx);
        }
        cx.notify();
    }

    /// Opens the Tarefas page as a title tab, or goes back to it when it is already one.
    pub(super) fn open_tasks_page(&mut self, cx: &mut Context<Self>) {
        match self.term.tasks_tab.as_mut() {
            Some(tab) => tab.closing = false,
            None => {
                self.tasks_page = tasks_page::State::default();
                self.term.tasks_tab = Some(self.fresh_page_tab());
                crate::segment_pill::forget("tasks-view");
            }
        }
        self.tasks_load(cx);
        self.term.active = None;
        self.term.pulls_active = false;
        self.term.tasks_active = true;
        self.term.plus_menu = None;
        self.palette_ui = None;
        cx.notify();
    }

    /// The page tabs as they stand: open ones, their pins, the one showing.
    fn page_snapshot(&self) -> crate::page_tabs::Snapshot {
        use crate::page_tabs::{Page, Snapshot};
        let live = |tab: &Option<PageTab>| tab.as_ref().is_some_and(|tab| !tab.closing);
        let mut pages = Vec::new();
        if live(&self.term.pulls_tab) {
            pages.push((Page::Pulls, self.term.pulls_pinned));
        }
        if live(&self.term.tasks_tab) {
            pages.push((Page::Tasks, self.term.tasks_pinned));
        }
        let active = if self.term.pulls_active {
            Some(Page::Pulls)
        } else if self.term.tasks_active {
            Some(Page::Tasks)
        } else {
            None
        };
        Snapshot { pages, active }
    }

    /// Writes the page tabs when they changed since the last frame. Until they are restored the
    /// file is left alone, so a launch that has not reopened them yet does not wipe it.
    pub(super) fn save_page_tabs(&mut self) {
        if !self.term.pages_restored {
            return;
        }
        let now = self.page_snapshot();
        if self.term.pages_saved.as_ref() != Some(&now) {
            crate::page_tabs::write(&crate::page_tabs::file(), &now);
            self.term.pages_saved = Some(now);
        }
    }

    /// Reopens the page tabs the last run left, in their order and pins, once the state is in.
    pub(super) fn restore_page_tabs(&mut self, cx: &mut Context<Self>) {
        use crate::page_tabs::Page;
        if std::mem::replace(&mut self.term.pages_restored, true) {
            return;
        }
        let Some(saved) = crate::page_tabs::read(&crate::page_tabs::file()) else {
            return;
        };
        let saved = saved.restored();
        for (page, pinned) in &saved.pages {
            match page {
                Page::Pulls => {
                    self.open_pull_requests(cx);
                    self.term.pulls_pinned = *pinned;
                }
                Page::Tasks => {
                    self.open_tasks_page(cx);
                    self.term.tasks_pinned = *pinned;
                }
            }
        }
        // Opening a page shows it; only the one that was showing stays on screen.
        self.term.pulls_active = saved.active == Some(Page::Pulls);
        self.term.tasks_active = saved.active == Some(Page::Tasks);
        self.term.pages_saved = Some(self.page_snapshot());
        cx.notify();
    }

    /// Closes the Tarefas tab; if it was showing, its left neighbour takes over, as for Pull requests.
    pub(super) fn close_tasks_tab(&mut self, cx: &mut Context<Self>) {
        let was_active = self.term.tasks_active;
        match self.term.tasks_tab.as_mut() {
            Some(tab) if !tab.closing => tab.closing = true,
            _ => return,
        }
        self.term.tasks_active = false;
        self.term.tasks_pinned = false;
        self.tasks_page.sheet = None;
        self.tasks_page.popover = None;
        if was_active {
            let strip: Vec<u64> = self
                .tabs
                .iter()
                .filter(|t| !t.pinned && !t.closing)
                .map(|t| t.id)
                .collect();
            let next = neighbour_on_close(strip.len() + 1, strip.len())
                .and_then(|at| strip.get(at).copied())
                .unwrap_or(self.active);
            self.select(next, cx);
        }
        cx.notify();
    }

    /// A page's plate, built like a session tab's: its tile, its title clipped as the plate
    /// narrows, and the close button that shows under the pointer.
    fn page_plate(
        &self,
        id: gpui::ElementId,
        group: SharedString,
        place: &PageTab,
        active: bool,
        mark: AnyElement,
        title: SharedString,
        key: u64,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let glow = self.tab_glow.level(key, now);
        let width = place.w.at(now);
        let closable = active || width >= 72.;
        let shown = ((width - PIN_WIDTH) / 60.).clamp(0., 1.);
        div()
            .id(id)
            .group(group)
            .absolute()
            .left(px(place.x.at(now)))
            .top(px((TITLE_BAR - PLATE) / 2.))
            .w(px(width))
            .h(px(PLATE))
            .rounded(px(crate::theme::RADIUS_MD))
            .overflow_hidden()
            .flex()
            .items_center()
            .pl(px(tab_tile_inset()))
            .gap(px(8.))
            .cursor_pointer()
            .bg(content(if active {
                0.1
            } else {
                crate::tab_hover::plate_ink(glow)
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.tab_glow.hover(key, *hovered);
                cx.notify();
            }))
            .when(place.closing, |d| {
                d.opacity((width / PIN_WIDTH).clamp(0., 1.))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(mark)
            .when(shown > 0., |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(20.))
                        .mr(px(if closable { 24. } else { 8. }))
                        .opacity(shown)
                        .text_size(px(13.))
                        .text_color(content(if active {
                            1.
                        } else {
                            crate::tab_hover::title_ink(glow)
                        }))
                        .child(crate::tab_label::tab_label(title)),
                )
            })
    }

    fn page_close(
        id: gpui::ElementId,
        group: SharedString,
        width: f32,
        active: bool,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        (active || width >= 72.).then(|| {
            div()
                .id(id)
                .absolute()
                .right(px(3.5))
                .top(px(3.5))
                .size(px(20.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .opacity(0.)
                .group_hover(group, |s| s.opacity(1.))
                .hover(|s| s.bg(content(0.1)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(icon("close", 12.).text_color(content(0.6)))
        })
    }

    pub(super) fn pulls_tab_view(
        &self,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tab = self
            .term
            .pulls_tab
            .as_ref()
            .filter(|_| !self.term.pulls_pinned)?;
        let active = self.term.pulls_active && !tab.closing;
        let group: SharedString = "pulls-tab".into();
        let (face, glyph) = pulls_tile();
        let close = Self::page_close(
            "pulls-tab-close".into(),
            group.clone(),
            tab.w.at(now),
            active,
        )
        .map(|close| {
            close.on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.close_pull_requests(cx);
            }))
        });
        Some(
            self.page_plate(
                "pulls-tab".into(),
                group,
                tab,
                active,
                mark_tile(&TAB_TILE, face, glyph).into_any_element(),
                t("Pull requests").into(),
                PULLS_CARD,
                now,
                cx,
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.ask_chip_menu(PULLS_TAB, event.position, cx);
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| this.open_pull_requests(cx)))
            .children(close)
            .into_any_element(),
        )
    }

    /// The Tarefas tab, after Pull requests in the strip.
    pub(super) fn tasks_tab_view(
        &self,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tab = self
            .term
            .tasks_tab
            .as_ref()
            .filter(|_| !self.term.tasks_pinned)?;
        let active = self.term.tasks_active && !tab.closing;
        let group: SharedString = "tasks-tab".into();
        let (face, glyph) = tasks_tile();
        let close = Self::page_close(
            "tasks-tab-close".into(),
            group.clone(),
            tab.w.at(now),
            active,
        )
        .map(|close| {
            close.on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.close_tasks_tab(cx);
            }))
        });
        Some(
            self.page_plate(
                "tasks-tab".into(),
                group,
                tab,
                active,
                mark_tile(&TAB_TILE, face, glyph).into_any_element(),
                t("Tarefas").into(),
                TASKS_CARD,
                now,
                cx,
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.ask_chip_menu(TASKS_TAB, event.position, cx);
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| this.open_tasks_page(cx)))
            .children(close)
            .into_any_element(),
        )
    }

    /// The terminal tabs in the strip, drawn as the session tabs are.
    pub(super) fn term_tabs_view(&self, now: Instant, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.term
            .tabs
            .iter()
            .filter(|tab| !tab.pinned)
            .map(|tab| {
                let id = tab.id;
                let active =
                    self.term.active == Some(id) && !self.term.pulls_active && !tab.place.closing;
                let group: SharedString = format!("term-tab-{id}").into();
                let (face, glyph) = terminal_tile(tab.view.read(cx).agent(cx));
                let close = Self::page_close(
                    ("term-tab-close", id).into(),
                    group.clone(),
                    tab.place.w.at(now),
                    active,
                )
                .map(|close| {
                    close.on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_title_tab(id, window, cx);
                    }))
                });
                self.page_plate(
                    ("term-tab", id).into(),
                    group,
                    &tab.place,
                    active,
                    mark_tile(&TAB_TILE, face, glyph).into_any_element(),
                    tab.view.read(cx).title().into(),
                    TERM_CARD + id,
                    now,
                    cx,
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_title_tab(id, window, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.ask_chip_menu(id, event.position, cx);
                    }),
                )
                .on_drag(self.term_drag(id, cx), |drag, _, _, cx| {
                    cx.new(|_| split_panes::DragChip::new(drag.title.clone()))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_title_tab(id, window, cx);
                }))
                .children(close)
                .into_any_element()
            })
            .collect()
    }

    /// The tab shown in place of the conversation.
    pub(super) fn title_tab_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.term.tasks_active {
            return Some(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.tasks_page_body(cx))
                    .into_any_element(),
            );
        }
        if let Some(pulls) = self.term.pulls.as_ref().filter(|_| self.term.pulls_active) {
            return Some(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(pulls.clone())
                    .into_any_element(),
            );
        }
        let tab = self
            .term
            .tabs
            .iter()
            .find(|t| Some(t.id) == self.term.active)?;
        let page = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(tab.view.clone())
            .into_any_element();
        Some(self.split_over_page(page, cx))
    }

    /// The plus button of the title strip: in this placement it opens a two-entry menu.
    pub(super) fn term_plus_click(&mut self, x: f32, cx: &mut Context<Self>) -> bool {
        if self.term.placement != Placement::TitleBar {
            return false;
        }
        self.term.plus_menu = Some((
            x,
            Tween::from_to(
                0.,
                1.,
                Instant::now(),
                if cx.reduce_motion() { 0. } else { 0.12 },
                Curve::Expo,
            ),
        ));
        cx.notify();
        true
    }

    /// The strip under the conversation: the active conversation's Terminal pane.
    pub(super) fn bottom_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.term.placement != Placement::Bottom || !self.term.bottom_open {
            return None;
        }
        let view = self.conversation_view(0)?;
        Some(
            div()
                .relative()
                .flex_shrink_0()
                .w_full()
                .h(px(BOTTOM_HEIGHT))
                .border_t_1()
                .border_color(content(0.08))
                .child(view)
                .children(self.term_drop_zone(cx))
                .into_any_element(),
        )
    }

    /// Nova sessão / Novo terminal, under the title strip's plus button.
    pub(super) fn term_plus_menu_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(menu) = self.term_chip_menu_view(now, moving, cx) {
            return Some(menu);
        }
        let (x, open) = self.term.plus_menu.as_ref()?;
        if !open.done(now) {
            *moving = true;
        }
        let shown = open.at(now).clamp(0., 1.);
        let item = |id: &'static str, glyph: &'static str, label: &'static str| {
            div()
                .id(id)
                .h(px(30.))
                .px(px(8.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(9.))
                .text_size(px(13.))
                .text_color(content(0.92))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)))
                .child(icon(glyph, 14.).text_color(content(0.65)))
                .child(div().flex_1().truncate().child(t(label)))
        };
        Some(
            div()
                .id("term-plus-menu")
                .occlude()
                .absolute()
                .left(px(x.min(self.viewport_width - 190.).max(8.)))
                .top(px(TITLE_BAR - 4. + (1. - shown) * -4.))
                .w(px(180.))
                .p(px(4.))
                .flex()
                .flex_col()
                .opacity(shown)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    item("term-plus-session", "plus", "Nova sessão").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.term.plus_menu = None;
                            this.term.active = None;
                            this.term.pulls_active = false;
                            this.term.tasks_active = false;
                            this.new_conversation(None, cx);
                            cx.notify();
                        },
                    )),
                )
                .child(
                    item("term-plus-terminal", "terminal-square", "Novo terminal").on_click(
                        cx.listener(|this, _, window, cx| {
                            this.new_title_terminal(Some(window), cx);
                        }),
                    ),
                )
                .into_any_element(),
        )
    }
}
