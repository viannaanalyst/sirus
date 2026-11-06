//! The terminal's place in the window (see `terminal_place`): the strip under the conversation,
//! the tabs in the title bar and ⌘J / ⇧⌘J. A child module of the shell, so it reaches its fields.

use super::*;
use crate::panel_terminal::{TerminalGroup, confirm_close, needs_confirm, running_names};
use crate::pull_requests_page::{PullRequestsPage, PullsEvent};
use crate::terminal_place::{self, Action, Placement};
use gpui::{AnyView, App, Entity, Subscription};

const CHIP_WIDTH: f32 = 124.;
const CHIP_GAP: f32 = 4.;
pub(super) const BOTTOM_HEIGHT: f32 = 280.;
/// A terminal's key in the deck of pinned tabs, clear of the tabs' ids.
pub(super) const TERM_CARD: u64 = 1 << 62;

struct TermTab {
    id: u64,
    view: Entity<TerminalGroup>,
    /// Lives in the deck of pinned tabs instead of the strip.
    pinned: bool,
}

pub(super) struct TermState {
    pub placement: Placement,
    tabs: Vec<TermTab>,
    next_id: u64,
    /// The title tab shown in place of the conversation.
    active: Option<u64>,
    bottom_open: bool,
    /// The plus button's menu in the title strip: left edge and fade-in.
    plus_menu: Option<(f32, Tween)>,
    /// The right-click menu of a terminal tab: which tab, where, and its fade-in.
    chip_menu: Option<(u64, gpui::Point<Pixels>, Tween)>,
    /// The Pull requests page, a tab among the session tabs in every placement (pull_requests_page.rs).
    pub(super) pulls: Option<Entity<PullRequestsPage>>,
    pub(super) pulls_active: bool,
    /// Its tab in the strip of session tabs, drawn and laid out with them (see `layout_tabs`).
    pub(super) pulls_tab: Option<PageTab>,
    _pulls_events: Option<Subscription>,
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
            bottom_open: false,
            plus_menu: None,
            chip_menu: None,
            pulls: None,
            pulls_active: false,
            pulls_tab: None,
            _pulls_events: None,
        }
    }
    /// Terminal tabs show in any placement: one pulled from the panel stays a tab.
    fn chip_count(&self) -> usize {
        self.tabs.iter().filter(|tab| !tab.pinned).count()
    }
    pub fn pinned_ids(&self) -> Vec<u64> {
        self.tabs
            .iter()
            .filter(|tab| tab.pinned)
            .map(|tab| tab.id)
            .collect()
    }
    /// The deck's key of the terminal tab showing, if any.
    pub fn active_card(&self) -> Option<u64> {
        self.active.map(|id| TERM_CARD + id)
    }
    pub fn chips_width(&self) -> f32 {
        match self.chip_count() {
            0 => 0.,
            count => count as f32 * (CHIP_WIDTH + CHIP_GAP) + 6.,
        }
    }
    pub fn close_menu(&mut self) -> bool {
        if self.chip_menu.take().is_some() {
            return true;
        }
        self.plus_menu.take().is_some()
    }
}

/// A pinned terminal's card in the deck, the same tile the panel's tabs carry.
pub(super) fn terminal_tile() -> (Face, AnyElement) {
    (
        Face::Fill(rgb(0x3A3A3C).into(), rgb(0x1C1C1E).into()),
        gpui::svg()
            .path("terminal-square")
            .size(px(12.))
            .text_color(gpui::white())
            .into_any_element(),
    )
}

impl Shell {
    fn conversation_view(&self, kind: usize) -> Option<AnyView> {
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
        let cwd = self.conversations[self.active_conversation()]
            .worktree
            .clone();
        let view = cx.new(|cx| TerminalGroup::new(cwd, cx));
        let id = self.term.next_id;
        self.term.next_id += 1;
        if let Some(window) = window {
            view.update(cx, |group, cx| group.focus(window, cx));
        }
        self.term.tabs.push(TermTab {
            id,
            view,
            pinned: false,
        });
        self.term.active = Some(id);
        self.term.pulls_active = false;
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
        self.term.tabs.push(TermTab {
            id,
            view,
            pinned: false,
        });
        self.term.active = Some(id);
        self.term.pulls_active = false;
        cx.notify();
        Some(id)
    }

    /// Shows a terminal tab in the conversation's place.
    pub(super) fn show_term_tab(&mut self, id: Option<u64>) {
        self.term.active = id;
    }

    /// The group of a terminal tab, for the pane that shows it beside a conversation.
    pub(super) fn term_view(&self, id: u64) -> Option<Entity<TerminalGroup>> {
        let tab = self.term.tabs.iter().find(|tab| tab.id == id)?;
        Some(tab.view.clone())
    }

    pub(super) fn term_title(&self, id: u64, cx: &App) -> String {
        self.term_view(id)
            .map_or_else(String::new, |view| view.read(cx).title())
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

    /// A terminal tab's right-click menu.
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
        let pinned = self.term.tabs.iter().find(|tab| tab.id == id)?.pinned;
        let track = crate::hover_track::list("term-chip-menu");
        let row = |key: &'static str, glyph: &'static str, label: &'static str| {
            track.row_named(
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
                    .cursor_pointer()
                    .child(icon(glyph, 14.).text_color(content(0.65)))
                    .child(label),
                key,
                false,
            )
        };
        Some(
            div()
                .id("term-chip-menu")
                .occlude()
                .absolute()
                .left(at.x)
                .top(at.y + px(2.))
                .w(px(190.))
                .p(px(4.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_MD))
                .shadow_lg()
                .opacity(shown)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(track.pill())
                .child(
                    row(
                        "term-menu-pin",
                        "pin",
                        t(if pinned { "Desafixar aba" } else { "Fixar aba" }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.term.chip_menu = None;
                        if let Some(tab) = this.term.tabs.iter_mut().find(|tab| tab.id == id) {
                            tab.pinned = !tab.pinned;
                        }
                        cx.notify();
                    })),
                )
                .child(
                    row("term-menu-close", "tab-close", t("Fechar aba")).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.term.chip_menu = None;
                            this.close_title_tab(id, window, cx);
                        },
                    )),
                )
                .into_any_element(),
        )
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
        // Dropping the last handle to a tab ends its shells.
        self.term.tabs.retain(|t| !ids.contains(&t.id));
        if self.term.active.is_some_and(|a| ids.contains(&a)) {
            self.term.active = self.term.tabs.last().map(|t| t.id);
        }
        cx.notify();
    }

    pub(super) fn leave_title_tab(&mut self) {
        self.term.active = None;
        self.term.pulls_active = false;
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
                // Starts at the end of the strip with no width, then grows, like a new session.
                let now = Instant::now();
                self.term.pulls_tab = Some(PageTab {
                    x: Tween::still(self.new_tab_x.at(now).max(0.), now),
                    w: Tween::still(0., now),
                    closing: false,
                });
            }
        }
        self.term.active = None;
        self.term.pulls_active = true;
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

    /// The Pull requests tab, drawn like a session tab: a neutral tile with the pull request
    /// glyph, the title and a close that shows on hover.
    pub(super) fn pulls_tab_view(
        &self,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tab = self.term.pulls_tab.as_ref()?;
        let active = self.term.pulls_active && !tab.closing;
        let width = tab.w.at(now);
        let ink = if active { 1. } else { 0.5 };
        let closable = active || width >= 72.;
        let title = ((width - PIN_WIDTH) / 60.).clamp(0., 1.);
        let mark = mark_tile(
            &TAB_TILE,
            Face::Neutral,
            crate::theme::icon("git-pull-request", 14.).text_color(content(0.9)),
        );
        Some(
            div()
                .id("pulls-tab")
                .group("pulls-tab")
                .absolute()
                .left(px(tab.x.at(now)))
                .top(px((TITLE_BAR - PLATE) / 2.))
                .w(px(width))
                .h(px(PLATE))
                .rounded(px(crate::theme::RADIUS_MD))
                .overflow_hidden()
                .flex()
                .items_center()
                .pl(px(tab_tile_inset()))
                .gap(px(8.))
                .bg(content(if active { 0.1 } else { 0.04 }))
                .when(!active, |d| d.hover(|s| s.bg(content(0.07))))
                .when(tab.closing, |d| {
                    d.opacity((width / PIN_WIDTH).clamp(0., 1.))
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.open_pull_requests(cx);
                }))
                .child(mark)
                // The title keeps its width while the plate narrows, so it clips instead of rewrapping.
                .when(title > 0., |d| {
                    d.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(20.))
                            .mr(px(if closable { 24. } else { 8. }))
                            .opacity(title)
                            .text_size(px(13.))
                            .text_color(content(ink))
                            .group_hover("pulls-tab", |s| s.text_color(content(1.)))
                            .child(crate::tab_label::tab_label(t("Pull requests"))),
                    )
                })
                .when(closable, |d| {
                    d.child(
                        div()
                            .id("pulls-tab-close")
                            .absolute()
                            .right(px(4.))
                            .top(px(4.5))
                            .size(px(18.))
                            .rounded(px(crate::theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(!active, |d| {
                                d.opacity(0.).group_hover("pulls-tab", |s| s.opacity(1.))
                            })
                            .hover(|s| s.bg(content(0.12)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_pull_requests(cx);
                            }))
                            .child(icon("close", 12.).text_color(content(0.6))),
                    )
                })
                .into_any_element(),
        )
    }

    /// The tab shown in place of the conversation.
    pub(super) fn title_tab_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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

    /// The terminal tabs, drawn after the conversation tabs.
    pub(super) fn term_chips(&self, left: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.term.chips_width() == 0. {
            return None;
        }
        let chips: Vec<AnyElement> = self
            .term
            .tabs
            .iter()
            .filter(|tab| !tab.pinned)
            .map(|tab| {
                let id = tab.id;
                let active = self.term.active == Some(id) && !self.term.pulls_active;
                let title: gpui::SharedString = tab.view.read(cx).title().into();
                div()
                    .id(("term-chip", id))
                    .on_drag(self.term_drag(id, cx), |drag, _, _, cx| {
                        cx.new(|_| split_panes::DragChip::new(drag.title.clone()))
                    })
                    .group(format!("term-chip-{id}"))
                    .relative()
                    .w(px(CHIP_WIDTH))
                    .flex_shrink_0()
                    .h(px(PLATE))
                    .pl(px(8.))
                    .pr(px(26.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .bg(content(if active { 0.1 } else { 0.04 }))
                    .text_color(content(if active { 1. } else { 0.55 }))
                    .hover(|s| s.bg(content(0.07)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.ask_chip_menu(id, event.position, cx);
                        }),
                    )
                    .child(icon("terminal-square", 12.).text_color(content(0.7)))
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(
                        div()
                            .id(("term-chip-close", id))
                            .absolute()
                            .right(px(3.))
                            .top(px(3.5))
                            .size(px(20.))
                            .rounded(px(crate::theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|s| s.bg(content(0.07)))
                            .child(icon("close", 12.))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_title_tab(id, window, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_title_tab(id, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .absolute()
                .left(px(left))
                .top(px((TITLE_BAR - PLATE) / 2.))
                .flex()
                .gap(px(CHIP_GAP))
                .children(chips)
                .into_any_element(),
        )
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
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    item("term-plus-session", "plus", "Nova sessão").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.term.plus_menu = None;
                            this.term.active = None;
                            this.term.pulls_active = false;
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
