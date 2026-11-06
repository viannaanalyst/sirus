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

struct TermTab {
    id: u64,
    view: Entity<TerminalGroup>,
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
            pulls: None,
            pulls_active: false,
            pulls_tab: None,
            _pulls_events: None,
        }
    }
    fn chip_count(&self) -> usize {
        let terminals = if self.placement == Placement::TitleBar {
            self.tabs.len()
        } else {
            0
        };
        terminals
    }
    pub fn chips_width(&self) -> f32 {
        match self.chip_count() {
            0 => 0.,
            count => count as f32 * (CHIP_WIDTH + CHIP_GAP) + 6.,
        }
    }
    pub fn close_menu(&mut self) -> bool {
        self.plus_menu.take().is_some()
    }
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
        self.term.tabs.push(TermTab { id, view });
        self.term.active = Some(id);
        self.term.pulls_active = false;
        self.term.plus_menu = None;
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
    pub(super) fn title_tab_view(&self) -> Option<AnyElement> {
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
        if self.term.placement != Placement::TitleBar {
            return None;
        }
        let tab = self
            .term
            .tabs
            .iter()
            .find(|t| Some(t.id) == self.term.active)?;
        Some(
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(tab.view.clone())
                .into_any_element(),
        )
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
        let terminals = if self.term.placement == Placement::TitleBar {
            self.term.tabs.as_slice()
        } else {
            &[]
        };
        let chips: Vec<AnyElement> = terminals
            .iter()
            .map(|tab| {
                let id = tab.id;
                let active = self.term.active == Some(id) && !self.term.pulls_active;
                let title: gpui::SharedString = tab.view.read(cx).title().into();
                div()
                    .id(("term-chip", id))
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
                        this.term.active = Some(id);
                        this.term.pulls_active = false;
                        this.term.plus_menu = None;
                        if let Some(tab) = this.term.tabs.iter().find(|t| t.id == id) {
                            tab.view.update(cx, |group, cx| group.focus(window, cx));
                        }
                        cx.notify();
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
    pub(super) fn bottom_view(&self) -> Option<AnyElement> {
        if self.term.placement != Placement::Bottom || !self.term.bottom_open {
            return None;
        }
        let view = self.conversation_view(0)?;
        Some(
            div()
                .flex_shrink_0()
                .w_full()
                .h(px(BOTTOM_HEIGHT))
                .border_t_1()
                .border_color(content(0.08))
                .child(view)
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
