//! The window: title strip, sidebar and a transcript with the live activity tree.
use crate::data::{self, Conversation, Project, THINKING, Turn, WORDS_PER_SECOND};
use crate::marks::{SPINNER_PERIOD, TILE_RAIL_INDENT, rail, shimmer, spinner_at};
use crate::motion::{Curve, DISCLOSE, MENU, REORDER, SIDEBAR, SWITCH, TAB_OPEN, Tween, eased};
use crate::tab_label::tab_label;
use crate::tab_tile::{ACTIVITY_TILE, FanCard, Face, TAB_TILE, activity_face, fan_card, fan_tile, fan_width, mark_tile, push_tile, tab_tile_inset};
use crate::theme::{ACCENTS, FOOTER, Look, PLATE, PRESETS, Palette, diff_colors, TITLE_BAR, accent, accent_wash, chrome_wash, content, field_fill, floating_fill, icon as theme_icon, mix, on_accent, palette, pane_fill, pane_opaque, provider_glyph, shell_fill};
use gpui::{
    AnyElement, Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, ScrollHandle, Stateful, Transformation, Window, WindowControlArea, div, linear_color_stop, linear_gradient, percentage, prelude::*, px, rgb, svg,
};
use gpui::transparent_black as transparent;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const NAV: f32 = 78. + 80.;
const GAP: f32 = 2.;
const PIN_WIDTH: f32 = 26.;
/// Between the pinned deck and the strip's first tab, split by a hairline.
const DOCK_GAP: f32 = 6.;
/// The most pins that sit in a plain row; more gather into a deck.
const ROW_MAX: usize = 3;
const FAN: f32 = 0.24;
const SHUFFLE: f32 = 0.34;
/// Seconds a tile's mark and its chevron take to trade places.
const MORPH: f32 = 0.3;
const DOCK_WIDTH: f32 = 470.;
/// The side panel's six options, as Sirus's RightDock: icon, name, what it does, its key.
const PANES: [(&str, &str, &str, &str); 6] = [
    ("square-terminal", "Terminal", "Rode comandos no projeto.", "t"),
    ("files", "Arquivos", "Abra os arquivos do projeto.", "a"),
    ("git-compare", "Alterações", "Revise o diff e faça commit.", "l"),
    ("globe", "Navegador", "Abra um app ou qualquer site.", "n"),
    ("messages", "Chat lateral", "Uma conversa à parte.", "c"),
    ("smartphone", "Simulador", "Rode o app num aparelho.", "s"),
];
const COMMANDS: [&str; 5] = ["Nova aba", "Abrir ou fechar a sidebar", "Abrir ou fechar o painel lateral", "Configurações", "Fixar ou desafixar a aba"];

/// One line of the palette and what choosing it does.
#[derive(Clone)]
enum PaletteHit {
    Chat(usize),
    Message(usize, String),
    Command(usize),
    File(&'static str, &'static str),
    Project(usize),
}

struct PaletteState {
    /// The top bar's lupa: this project's files, `>` for commands. Otherwise ⌘K's palette.
    files: bool,
    query: String,
    selected: usize,
    open: Tween,
}

/// A collapsed sidebar sliding out over the workspace (Monocode's sidebar_peek.rs).
struct Peek {
    open: bool,
    t: Tween,
}

const PEEK_EDGE: f32 = 6.;
const PEEK_INSET: f32 = 6.;
const PEEK_DRIFT: f32 = 12.;
const PEEK_CLOSE: Duration = Duration::from_millis(180);
const SIDEBAR_SEARCH: f32 = 42.;
/// Settings, same pages in the same order as Sirus's SettingsSidebar.
const SETTINGS_NAV: [(&str, &[(&str, &str)]); 3] = [
    ("Pessoal", &[("tuning", "Geral"), ("message-square", "Comportamento do chat"), ("monitor", "Aparência"), ("bell", "Notificações"), ("keyboard", "Atalhos")]),
    ("Programação", &[("puzzle", "Provedores"), ("box", "Agent skills"), ("plug", "Conectores"), ("pointer", "Computador"), ("branch", "Git"), ("folder-git", "Worktrees"), ("terminal-square", "Terminal")]),
    ("Sistema", &[("smartphone", "Conexões"), ("sliders", "Avançado")]),
];
const APPEARANCE: &str = "Aparência";
const TAB_MAX: f32 = 224.;
const TAB_MIN: f32 = 64.;
const SELECTED_MIN: f32 = 112.;
const SIDEBAR_MIN: f32 = 190.;
const SIDEBAR_MAX: f32 = 420.;
const PROJECT_ROW: f32 = 30.;
const CONV_ROW: f32 = 29.;
const HEADING: f32 = 30.;
const STEP_ROW: f32 = 26.;

struct Tab {
    id: u64,
    conversation: usize,
    pinned: bool,
    closing: bool,
    x: Tween,
    w: Tween,
}

struct TabDrag {
    id: u64,
    origin: f32,
    start: f32,
    x: f32,
    active: bool,
}

struct Menu {
    tab: u64,
    at: Point<Pixels>,
    open: Tween,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Row {
    Heading(bool),
    Project(usize),
    Conversation(usize),
}

struct RowMotion {
    y: Tween,
    alpha: Tween,
}

pub struct Shell {
    focus: FocusHandle,
    projects: Vec<Project>,
    conversations: Vec<Conversation>,
    project_order: Vec<usize>,
    tabs: Vec<Tab>,
    active: u64,
    next_id: u64,
    new_tab_x: Tween,
    tray: Tween,
    palette_ui: Option<PaletteState>,
    environment: Option<Tween>,
    dock_open: bool,
    dock: Tween,
    dock_max: Tween,
    dock_panes: Vec<usize>,
    dock_active: Option<usize>,
    chooser_since: Instant,
    look: Look,
    system_dark: bool,
    settings_page: &'static str,
    settings_since: Instant,
    settings_direction: f32,
    slider_drag: Option<usize>,
    slider_bounds: std::rc::Rc<std::cell::RefCell<[Option<gpui::Bounds<Pixels>>; 2]>>,
    peek: Option<Peek>,
    peek_close: Option<gpui::Task<()>>,
    palette_from: Palette,
    palette_t: Tween,
    appearance: Option<Tween>,
    fan: Tween,
    deck_hovered: bool,
    card_hovered: Option<u64>,
    deck_front: Option<u64>,
    shuffle: Option<(u64, Instant)>,
    history: Vec<u64>,
    history_at: usize,
    rolls: HashMap<(usize, usize), Tween>,
    drag: Option<TabDrag>,
    menu: Option<Menu>,
    sidebar_width: f32,
    sidebar_open: bool,
    sidebar: Tween,
    resizing: bool,
    rows: HashMap<Row, RowMotion>,
    switch: Tween,
    scrolls: HashMap<usize, ScrollHandle>,
    next_turn: usize,
    epoch: Instant,
    frames: VecDeque<Instant>,
    show_stats: bool,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let now = Instant::now();
        let (projects, conversations) = data::sample();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Loops (spinners, shimmers, streaming text) tick together at 30 Hz, as Monocode's pulse.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(33)).await;
                let alive = this.update(cx, |this, cx| {
                    if this.conversations.iter().any(|c| running(c, Instant::now())) {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let project_order = (0..projects.len()).collect();
        let mut shell = Self {
            focus,
            projects,
            conversations,
            project_order,
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            new_tab_x: Tween::still(0., now),
            tray: Tween::still(0., now),
            palette_ui: None,
            environment: None,
            dock_open: false,
            dock: Tween::still(0., now),
            dock_max: Tween::still(0., now),
            dock_panes: Vec::new(),
            dock_active: None,
            chooser_since: now,
            look: Look::default(),
            system_dark: true,
            settings_page: APPEARANCE,
            settings_since: now,
            settings_direction: 1.,
            slider_drag: None,
            slider_bounds: Default::default(),
            peek: None,
            peek_close: None,
            palette_from: palette(&Look::default(), true),
            palette_t: Tween::still(1., now),
            appearance: None,
            fan: Tween::still(1., now),
            deck_hovered: false,
            card_hovered: None,
            deck_front: None,
            shuffle: None,
            history: Vec::new(),
            history_at: 0,
            rolls: HashMap::new(),
            drag: None,
            menu: None,
            sidebar_width: 264.,
            sidebar_open: true,
            sidebar: Tween::still(1., now),
            resizing: false,
            rows: HashMap::new(),
            switch: Tween::still(1., now),
            scrolls: HashMap::new(),
            next_turn: 0,
            epoch: now,
            frames: VecDeque::new(),
            show_stats: false,
        };
        for (conversation, pinned) in [(6, true), (4, true), (0, false), (1, false), (2, false)] {
            let id = shell.next_id;
            shell.next_id += 1;
            shell.tabs.push(Tab { id, conversation, pinned, closing: false, x: Tween::still(-1., now), w: Tween::still(-1., now) });
        }
        shell.active = shell.tabs[2].id;
        shell.history.push(shell.active);
        // One conversation is already working in the background, so a tab's spinner shows at once.
        let started = shell.conversations[2].turns.len();
        shell.conversations[2].turns.push(data::turn(started, Some(now)));
        shell
    }

    fn scroll(&mut self, conversation: usize) -> ScrollHandle {
        self.scrolls.entry(conversation).or_default().clone()
    }

    // Tabs.

    fn active_conversation(&self) -> usize {
        self.tabs.iter().find(|t| t.id == self.active).map_or(0, |t| t.conversation)
    }

    fn select(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.active == id {
            return;
        }
        self.history.truncate(self.history_at + 1);
        self.history.push(id);
        self.history_at = self.history.len() - 1;
        self.show(id, cx);
    }

    fn show(&mut self, id: u64, cx: &mut Context<Self>) {
        self.active = id;
        self.switch = Tween::from_to(0., 1., Instant::now(), SWITCH, Curve::Expo);
        cx.notify();
    }

    /// Back and forward walk the tabs visited, skipping any since closed.
    fn navigate(&mut self, forward: bool, cx: &mut Context<Self>) {
        let mut at = self.history_at as isize;
        loop {
            at += if forward { 1 } else { -1 };
            let Some(id) = usize::try_from(at).ok().and_then(|i| self.history.get(i)).copied() else { return };
            if self.tabs.iter().any(|t| t.id == id && !t.closing) {
                self.history_at = at as usize;
                self.show(id, cx);
                return;
            }
        }
    }

    fn can_navigate(&self, forward: bool) -> bool {
        let live = |id: &u64| self.tabs.iter().any(|t| t.id == *id && !t.closing);
        if forward { self.history[(self.history_at + 1).min(self.history.len())..].iter().any(live) } else { self.history[..self.history_at].iter().any(live) }
    }

    fn open(&mut self, conversation: usize, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.iter().find(|t| t.conversation == conversation && !t.closing).map(|t| t.id) {
            self.select(id, cx);
            return;
        }
        let now = Instant::now();
        let id = self.next_id;
        self.next_id += 1;
        // Starts at the end of the strip with no width, then grows.
        let x = self.new_tab_x.at(now);
        self.tabs.push(Tab { id, conversation, pinned: false, closing: false, x: Tween::still(x, now), w: Tween::still(0., now) });
        self.select(id, cx);
    }

    fn close(&mut self, id: u64, cx: &mut Context<Self>) {
        let live: Vec<u64> = self.tabs.iter().filter(|t| !t.closing).map(|t| t.id).collect();
        if live.len() <= 1 {
            return;
        }
        if self.active == id {
            // Like a browser: the next tab to the right, then the one to the left.
            let at = live.iter().position(|t| *t == id).unwrap_or(0);
            let next = live.get(at + 1).or_else(|| at.checked_sub(1).and_then(|i| live.get(i))).copied();
            if let Some(next) = next {
                self.select(next, cx);
            }
        }
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.closing = true;
        }
        cx.notify();
    }

    /// A pinned tab leaves the strip for a card in the deck, its plate collapsing where it
    /// was; unpinned, it grows back at the strip's left end.
    fn toggle_pin(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(at) = self.tabs.iter().position(|t| t.id == id) else { return };
        let now = Instant::now();
        let mut tab = self.tabs.remove(at);
        if tab.pinned {
            tab.pinned = false;
            tab.x = Tween::still(0., now);
            tab.w = Tween::still(0., now);
        } else {
            let ghost = Tab { id: self.next_id, conversation: tab.conversation, pinned: false, closing: true, x: tab.x, w: tab.w };
            self.next_id += 1;
            self.tabs.insert(at, ghost);
            tab.pinned = true;
        }
        let edge = self.tabs.iter().position(|t| !t.pinned).unwrap_or(self.tabs.len());
        self.tabs.insert(edge, tab);
        self.drag = None;
        self.card_hovered = None;
        cx.notify();
    }

    /// Pinned tabs stay, as in browsers; the kept tab becomes the active one if it was closed.
    fn close_where(&mut self, keep: u64, closes: impl Fn(usize, usize) -> bool, cx: &mut Context<Self>) {
        let at = self.tabs.iter().position(|t| t.id == keep).unwrap_or(0);
        let mut closed_active = false;
        for (ix, tab) in self.tabs.iter_mut().enumerate() {
            if tab.id != keep && !tab.pinned && closes(ix, at) {
                tab.closing = true;
                closed_active |= tab.id == self.active;
            }
        }
        if closed_active {
            self.select(keep, cx);
        }
        cx.notify();
    }

    /// Where each strip tab belongs, measured from the end of the pinned dock; tweens head
    /// there. Returns whether anything still moves.
    fn layout_tabs(&mut self, strip: f32, now: Instant, reduced: bool) -> bool {
        let (open, reorder, fan_time) = if reduced { (0., 0., 0.) } else { (TAB_OPEN, REORDER, FAN) };
        let pinned = self.tabs.iter().filter(|t| t.pinned && !t.closing).count();
        let free = self.tabs.iter().filter(|t| !t.pinned && !t.closing).count() as f32;
        let menu_on_pin = self.menu.as_ref().is_some_and(|m| self.tabs.iter().any(|t| t.id == m.tab && t.pinned));
        let spread = pinned <= ROW_MAX || self.deck_hovered || menu_on_pin;
        self.fan.set(if spread { 1. } else { 0. }, now, fan_time, Curve::Expo);
        let dock = |fan: f32| if pinned > 0 { fan_width(&TAB_TILE, pinned, fan) + tab_tile_inset() * 2. + DOCK_GAP * 2. } else { 0. };
        // The dock follows the fan exactly while it moves, so the tabs give way with it.
        if self.fan.done(now) {
            self.tray.set(dock(self.fan.at(now)), now, reorder, Curve::Expo);
        } else {
            self.tray.snap(dock(self.fan.at(now)), now);
        }
        let room = strip - dock(self.fan.to) - (free - 1.).max(0.) * GAP;
        let even = if free > 0. { (room / free).clamp(TAB_MIN, TAB_MAX) } else { 0. };
        let active_free = self.tabs.iter().any(|t| t.id == self.active && !t.pinned);
        let (selected, others) = if active_free && even < SELECTED_MIN && free > 1. {
            (SELECTED_MIN, ((room - SELECTED_MIN) / (free - 1.)).max(TAB_MIN))
        } else {
            (even, even)
        };
        let mut x = 0.;
        let mut moving = !self.tray.done(now) || !self.fan.done(now);
        let dragging = self.drag.as_ref().filter(|d| d.active).map(|d| (d.id, d.x));
        for tab in &mut self.tabs {
            if tab.pinned {
                continue;
            }
            let width = if tab.closing { 0. } else if tab.id == self.active { selected } else { others };
            if tab.x.to < 0. {
                tab.x.snap(x, now);
                tab.w.snap(width, now);
            }
            tab.w.set(width, now, open, Curve::Quad);
            match dragging {
                Some((id, at)) if id == tab.id => tab.x.snap(at, now),
                _ => tab.x.set(x, now, reorder, Curve::Expo),
            }
            moving |= !tab.x.done(now) || !tab.w.done(now);
            if !tab.closing {
                x += width + GAP;
            }
        }
        self.new_tab_x.set(x, now, reorder, Curve::Expo);
        moving |= !self.new_tab_x.done(now);
        self.tabs.retain(|t| !(t.closing && (t.pinned || t.w.done(now))));
        moving
    }

    /// The pinned cards, as Monocode's pinned_tabs.rs: up to three in a row, more stacked
    /// behind the last chosen, fanning out when pointed at. Hovered cards show their title.
    fn pinned_dock(&mut self, now: Instant, moving: &mut bool, reduced: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pinned: Vec<(u64, usize)> = self.tabs.iter().filter(|t| t.pinned && !t.closing).map(|t| (t.id, t.conversation)).collect();
        if pinned.is_empty() {
            self.deck_front = None;
            return None;
        }
        let count = pinned.len();
        let stacks = count > ROW_MAX;
        let fan = self.fan.at(now);
        let front = pinned.iter().position(|(id, _)| *id == self.active);
        let top = front.or_else(|| pinned.iter().position(|(id, _)| Some(*id) == self.deck_front)).unwrap_or(0);
        if self.deck_front != Some(pinned[top].0) {
            let was = self.deck_front.replace(pinned[top].0);
            self.shuffle = was.filter(|w| stacks && !reduced && pinned.iter().any(|(id, _)| id == w)).map(|w| (w, now));
        }
        if !stacks {
            self.shuffle = None;
        }
        let shuffle = self.shuffle.and_then(|(was, start)| {
            let t = now.duration_since(start).as_secs_f32() / SHUFFLE;
            let was = pinned.iter().position(|(id, _)| *id == was)?;
            (t < 1.).then_some((was, t))
        });
        if shuffle.is_none() {
            self.shuffle = None;
        } else {
            *moving = true;
        }
        let inset = tab_tile_inset();
        let width = fan_width(&TAB_TILE, count, fan) + inset * 2.;
        let hovered_card = self.card_hovered.filter(|c| pinned.iter().any(|(id, _)| id == c));
        let plate_top = (TITLE_BAR - PLATE) / 2.;
        let places: Vec<FanCard> = (0..count).map(|ix| fan_card(&TAB_TILE, ix, top, fan, shuffle)).collect();
        let caption = hovered_card.and_then(|c| {
            let ix = pinned.iter().position(|(id, _)| *id == c)?;
            Some((self.conversations[pinned[ix].1].title.clone(), places[ix].x))
        });
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by(|a, b| places[*b].z.total_cmp(&places[*a].z));
        let mut cards = Vec::new();
        for ix in order {
            let (id, conversation) = pinned[ix];
            let place = &places[ix];
            let selected = id == self.active;
            let hovered = hovered_card == Some(id);
            let (face, mark) = self.tile_parts(&self.conversations[conversation], now);
            cards.push(
                div()
                    .id(("pinned-tab", id))
                    .cursor_pointer()
                    .absolute()
                    .left(px(inset + place.x))
                    .top(px(inset + place.y))
                    .w(px(TAB_TILE.w))
                    .h(px(TAB_TILE.h))
                    .when(place.opacity < 1., |d| d.opacity(place.opacity))
                    .child(fan_tile(&TAB_TILE, place, face, mark))
                    .when(selected, |d| d.child(div().absolute().bottom(px(-inset + 0.5)).left(px((TAB_TILE.w - 10.) / 2.)).w(px(10.)).h(px(2.)).rounded_full().bg(accent())))
                    .when(hovered && !selected, |d| d.child(div().absolute().bottom(px(-inset + 0.5)).left(px((TAB_TILE.w - 6.) / 2.)).w(px(6.)).h(px(2.)).rounded_full().bg(content(0.35))))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.card_hovered = Some(id);
                        } else if this.card_hovered == Some(id) {
                            this.card_hovered = None;
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        let duration = if cx.reduce_motion() { 0. } else { MENU };
                        this.menu = Some(Menu { tab: id, at: event.position, open: Tween::from_to(0., 1., Instant::now(), duration, Curve::Expo) });
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                    .into_any_element(),
            );
        }
        let any_selected = front.is_some();
        let any_hovered = hovered_card.is_some();
        Some(
            div()
                .absolute()
                .left_0()
                .top_0()
                .h(px(TITLE_BAR))
                .w(px(self.tray.at(now)))
                .child(div().absolute().right(px(DOCK_GAP)).top(px((TITLE_BAR - 16.) / 2.)).w(px(1.)).h(px(16.)).bg(content(0.1)))
                .child(
                    div()
                        .id("pinned-tab-deck")
                        .absolute()
                        .left_0()
                        .top(px(plate_top))
                        .w(px(width))
                        .h(px(PLATE))
                        .rounded_md()
                        .child(div().absolute().size_full().rounded_md().bg(content(if any_selected { 0.1 } else if any_hovered { 0.07 } else { 0.04 })))
                        .children(cards)
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            this.deck_hovered = *hovered;
                            if !*hovered {
                                this.card_hovered = None;
                            }
                            cx.notify();
                        })),
                )
                .children(caption.filter(|_| fan > 0.05).map(|(title, x)| {
                    gpui::deferred(
                        div()
                            .absolute()
                            .left(px(inset + x))
                            .top(px(plate_top + PLATE + 6.))
                            .max_w(px(260.))
                            .h(px(24.))
                            .px_2()
                            .flex()
                            .items_center()
                            .opacity(fan)
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.1))
                            .bg(floating_fill())
                            .shadow_md()
                            .text_size(px(12.))
                            .text_color(content(1.))
                            .child(div().min_w_0().truncate().child(title)),
                    )
                    .with_priority(3)
                }))
                .into_any_element(),
        )
    }

    /// A conversation's tile: the spinner on a neutral face while it works, else its model.
    fn tile_parts(&self, conversation: &Conversation, now: Instant) -> (Face, AnyElement) {
        if running(conversation, now) {
            (Face::Neutral, self.spin(now).into_any_element())
        } else {
            (Face::provider(conversation.provider), provider_glyph(conversation.provider, 14.))
        }
    }

    fn start_drag(&mut self, id: u64, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let now = Instant::now();
        let start = self.tabs.iter().find(|t| t.id == id).map_or(0., |t| t.x.at(now));
        self.drag = Some(TabDrag { id, origin: f32::from(event.position.x), start, x: start, active: false });
        self.menu = None;
        self.select(id, cx);
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.resizing {
            if event.pressed_button != Some(MouseButton::Left) {
                self.resizing = false;
                return;
            }
            self.sidebar_width = (f32::from(event.position.x) - 6.).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
            cx.notify();
            return;
        }
        let Some(drag) = self.drag.as_mut() else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            cx.notify();
            return;
        }
        let delta = f32::from(event.position.x) - drag.origin;
        if delta.abs() < 4. && !drag.active {
            return;
        }
        drag.active = true;
        let now = Instant::now();
        let id = drag.id;
        let Some(from) = self.tabs.iter().position(|t| t.id == id) else { return };
        let width = self.tabs[from].w.to;
        let group: Vec<usize> = (0..self.tabs.len()).filter(|i| !self.tabs[*i].pinned && !self.tabs[*i].closing).collect();
        let (Some(first), Some(last)) = (group.first().copied(), group.last().copied()) else { return };
        let low = self.tabs[first].x.to.min(drag.start);
        let high = self.tabs[last].x.to.max(drag.start);
        let x = (drag.start + delta).clamp(low, high);
        drag.x = x;
        let center = x + width / 2.;
        let mut to = from;
        for ix in &group {
            let other = &self.tabs[*ix];
            if other.id != id && center > other.x.to && center < other.x.to + other.w.to {
                to = *ix;
            }
        }
        if to != from {
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);
        }
        let _ = now;
        cx.notify();
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.resizing = false;
        if let Some(drag) = self.drag.take() {
            if drag.active {
                let now = Instant::now();
                // The tab eases from the pointer into its slot.
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == drag.id) {
                    tab.x.snap(drag.x, now);
                }
            }
            cx.notify();
        }
    }

    fn key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let key = keys.key.as_str();
        if let Some(state) = self.palette_ui.as_mut() {
            let files = state.files;
            let count = { let q = state.query.clone(); self.palette_hits(files, &q).len() };
            let state = self.palette_ui.as_mut().unwrap();
            match key {
                "escape" => self.palette_ui = None,
                "down" => state.selected = (state.selected + 1).min(count.saturating_sub(1)),
                "up" => state.selected = state.selected.saturating_sub(1),
                "backspace" => {
                    state.query.pop();
                    state.selected = 0;
                }
                "enter" => {
                    let (q, at) = (state.query.clone(), state.selected);
                    let hit = self.palette_hits(files, &q).get(at).cloned();
                    if let Some(hit) = hit {
                        self.run_hit(hit, cx);
                    }
                }
                _ if keys.modifiers.platform && (key == "k" || key == "p") => self.palette_ui = None,
                _ if !keys.modifiers.platform && !keys.modifiers.control => {
                    if let Some(text) = keys.key_char.as_ref() {
                        state.query.push_str(text);
                        state.selected = 0;
                    }
                }
                _ => {}
            }
            cx.notify();
            return;
        }
        if key == "escape" && (self.environment.is_some() || self.appearance.is_some() || self.menu.is_some()) {
            self.environment = None;
            self.appearance = None;
            self.menu = None;
            cx.notify();
            return;
        }
        // The side panel's options open with their letter while it shows them.
        if self.appearance.is_none() && self.dock_open && self.dock_active.is_none() && !keys.modifiers.platform && !keys.modifiers.control && !keys.modifiers.alt {
            if let Some(kind) = PANES.iter().position(|p| p.3 == key) {
                self.open_pane(kind, cx);
                return;
            }
        }
        let (cmd, shift, ctrl) = (keys.modifiers.platform, keys.modifiers.shift, keys.modifiers.control);
        let live: Vec<u64> = self.tabs.iter().filter(|t| !t.closing).map(|t| t.id).collect();
        match (cmd, shift, ctrl, key) {
            (true, false, false, "q") => cx.quit(),
            (true, false, false, "t") => {
                let free = (0..self.conversations.len()).find(|c| !self.tabs.iter().any(|t| t.conversation == *c && !t.closing));
                if let Some(free) = free {
                    self.open(free, cx);
                }
            }
            (true, false, false, "w") => self.close(self.active, cx),
            (true, true, false, "p") => self.toggle_pin(self.active, cx),
            (true, false, false, "s") => self.toggle_sidebar(cx),
            (true, false, false, ",") => self.toggle_appearance(cx),
            (true, false, false, "k") => self.toggle_palette(false, cx),
            (true, false, false, "p") => self.toggle_palette(true, cx),
            (true, false, false, "\\") => self.toggle_dock(cx),
            (true, true, false, "w") => self.close_where(self.active, |_, _| true, cx),
            (true, false, false, "[") => self.navigate(false, cx),
            (true, false, false, "]") => self.navigate(true, cx),
            (true, true, false, "f") => {
                self.show_stats = !self.show_stats;
                cx.notify();
            }
            (true, false, false, digit) if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() && digit != "0" => {
                let index = (digit.as_bytes()[0] - b'1') as usize;
                if let Some(id) = live.get(index) {
                    self.select(*id, cx);
                }
            }
            (false, shift, true, "tab") => {
                let at = live.iter().position(|t| *t == self.active).unwrap_or(0);
                let next = if shift { at + live.len() - 1 } else { at + 1 } % live.len();
                self.select(live[next], cx);
            }
            (false, false, false, "enter") => self.send(cx),
            _ => {}
        }
    }

    // Sidebar.

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        self.peek = None;
        self.peek_close = None;
        let duration = if cx.reduce_motion() { 0. } else { SIDEBAR };
        self.sidebar.set(if self.sidebar_open { 1. } else { 0. }, Instant::now(), duration, Curve::Expo);
        cx.notify();
    }

    fn toggle_project(&mut self, index: usize, cx: &mut Context<Self>) {
        self.projects[index].open = !self.projects[index].open;
        cx.notify();
    }

    /// Rows move and fade into place, so pinning a project or folding one never jumps.
    fn layout_rows(&mut self, now: Instant, reduced: bool) -> (Vec<(Row, f32)>, f32, bool) {
        let duration = if reduced { 0. } else { DISCLOSE };
        let mut wanted: Vec<(Row, f32, f32)> = Vec::new();
        let mut y = 4.;
        let order: Vec<usize> = {
            let (pinned, rest): (Vec<usize>, Vec<usize>) = self.project_order.iter().copied().partition(|p| self.projects[*p].pinned);
            for (section, list) in [(true, &pinned), (false, &rest)] {
                if list.is_empty() {
                    continue;
                }
                wanted.push((Row::Heading(section), y, y));
                y += HEADING;
                for project in list {
                    let top = y;
                    wanted.push((Row::Project(*project), y, y));
                    y += PROJECT_ROW;
                    if self.projects[*project].open {
                        for conversation in &self.projects[*project].conversations {
                            wanted.push((Row::Conversation(*conversation), y, top));
                            y += CONV_ROW;
                        }
                    }
                }
                y += 6.;
            }
            Vec::new()
        };
        let _ = order;
        let mut moving = false;
        let mut seen: Vec<Row> = Vec::new();
        for (row, target, origin) in wanted {
            seen.push(row);
            let motion = self.rows.entry(row).or_insert_with(|| RowMotion {
                y: Tween::still(origin, now),
                alpha: if now.duration_since(self.epoch).as_millis() < 50 { Tween::still(1., now) } else { Tween::still(0., now) },
            });
            if now.duration_since(self.epoch).as_millis() < 50 {
                motion.y.snap(target, now);
            }
            motion.y.set(target, now, duration, Curve::Expo);
            motion.alpha.set(1., now, duration, Curve::Quad);
        }
        // Folded rows slide up under their project and fade.
        let projects = &self.projects;
        let mut parents: HashMap<usize, f32> = HashMap::new();
        for (index, project) in projects.iter().enumerate() {
            let top = self.rows.get(&Row::Project(index)).map_or(0., |m| m.y.to);
            for conversation in &project.conversations {
                parents.insert(*conversation, top);
            }
        }
        self.rows.retain(|row, motion| {
            if !seen.contains(row) {
                let parent = if let Row::Conversation(c) = row { parents.get(c).copied() } else { None };
                motion.y.set(parent.unwrap_or(motion.y.to), now, duration, Curve::Expo);
                motion.alpha.set(0., now, duration, Curve::Quad);
                if motion.alpha.done(now) {
                    return false;
                }
            }
            moving |= !motion.y.done(now) || !motion.alpha.done(now);
            true
        });
        let mut drawn: Vec<(Row, f32)> = self.rows.keys().map(|row| (*row, self.rows[row].y.at(now))).collect();
        // Fading rows paint first, under the rows that stay.
        drawn.sort_by(|a, b| self.rows[&a.0].alpha.to.total_cmp(&self.rows[&b.0].alpha.to).then(a.1.total_cmp(&b.1)));
        (drawn, y, moving)
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let conversation = self.active_conversation();
        let now = Instant::now();
        if running(&self.conversations[conversation], now) {
            return;
        }
        self.next_turn += 1;
        let turn = data::turn(self.next_turn, Some(now));
        self.conversations[conversation].turns.push(turn);
        self.scroll(conversation).scroll_to_bottom();
        cx.notify();
    }

    // Drawing.

    fn tab(&self, tab: &Tab, now: Instant, cx: &mut Context<Self>) -> AnyElement {
        let id = tab.id;
        let conversation = &self.conversations[tab.conversation];
        let active = id == self.active && !tab.closing;
        let group: gpui::SharedString = format!("tab{id}").into();
        let width = tab.w.at(now);
        let dragged = self.drag.as_ref().is_some_and(|d| d.active && d.id == id);
        let ink = if active { 1. } else { 0.5 };
        let (face, glyph) = self.tile_parts(conversation, now);
        let mark = mark_tile(&TAB_TILE, face, glyph);
        let closable = active || width >= 72.;
        let title = ((width - PIN_WIDTH) / 60.).clamp(0., 1.);
        div()
            .id(("tab", id))
            .group(group.clone())
            .absolute()
            .left(px(tab.x.at(now)))
            .top(px((TITLE_BAR - PLATE) / 2.))
            .w(px(width))
            .h(px(PLATE))
            .rounded_md()
            .overflow_hidden()
            .flex()
            .items_center()
            .pl(px(tab_tile_inset()))
            .gap(px(8.))
            .bg(content(if active { 0.1 } else { 0.04 }))
            .when(!active, |d| d.hover(|s| s.bg(content(0.07))))
            .when(dragged, |d| d.bg(content(0.14)).shadow_md())
            .when(tab.closing, |d| d.opacity((width / PIN_WIDTH).clamp(0., 1.)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.start_drag(id, event, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let duration = if cx.reduce_motion() { 0. } else { MENU };
                this.menu = Some(Menu { tab: id, at: event.position, open: Tween::from_to(0., 1., Instant::now(), duration, Curve::Expo) });
                cx.notify();
            }))
            .child(mark)
            // The title keeps its width while the plate narrows, so pinning clips it instead of rewrapping.
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
                        .group_hover(group.clone(), |s| s.text_color(content(1.)))
                        .child(tab_label(conversation.title.clone())),
                )
            })
            .when(closable, |d| {
                d.child(
                    div()
                        .id(("close", id))
                        .absolute()
                        .right(px(4.))
                        .top(px(4.5))
                        .size(px(18.))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(!active, |d| d.opacity(0.).group_hover(group, |s| s.opacity(1.)))
                        .hover(|s| s.bg(content(0.12)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| this.close(id, cx)))
                        .child(icon("close", 12.).text_color(content(0.6))),
                )
            })
            .into_any_element()
    }

    fn conversation_mark(&self, conversation: &Conversation, now: Instant, lit: bool) -> AnyElement {
        let box_ = div().size(px(14.)).flex_shrink_0().flex().items_center().justify_center();
        if running(conversation, now) {
            let fraction = (now.duration_since(self.epoch).as_secs_f32() / SPINNER_PERIOD).fract();
            box_.child(spinner_at(fraction)).into_any_element()
        } else {
            mark_tile(&crate::tab_tile::TileSize { w: 16., h: 14., radius: 4. }, Face::provider(conversation.provider), provider_glyph(conversation.provider, 10.)).opacity(if lit { 1. } else { 0.7 }).into_any_element()
        }
    }

    fn titlebar(&mut self, width: f32, now: Instant, moving: &mut bool, reduced: bool, cx: &mut Context<Self>) -> AnyElement {
        // The tabs start where the card does: traffic lights and arrows keep to the sidebar's
        // column, and the arrows follow its edge as it slides.
        let nav_end = (self.sidebar_width * self.sidebar.at(now)).max(NAV);
        let strip_left = nav_end + 6.;
        let open_dock = self.dock.at(now);
        let controls = 80. + 56. * open_dock + 72.;
        let (added, removed) = self.diff_stat();
        let (add_color, remove_color) = diff_colors(self.look.diff);
        let strip = width - strip_left - 12. - PLATE - 6. - controls - 10.;
        *moving |= self.layout_tabs(strip, now, reduced);
        let tabs: Vec<AnyElement> = {
            // Pinned tabs live in the dock, not the strip.
            let mut order: Vec<usize> = (0..self.tabs.len()).filter(|ix| !self.tabs[*ix].pinned).collect();
            // The dragged tab paints over its neighbours.
            let dragged = self.drag.as_ref().filter(|d| d.active).map(|d| d.id);
            order.sort_by_key(|ix| Some(self.tabs[*ix].id) == dragged);
            let mut out = Vec::new();
            for ix in order {
                let tab = &self.tabs[ix];
                out.push(self.tab(tab, now, cx));
            }
            out
        };
        let dock = self.pinned_dock(now, moving, reduced, cx);
        let tray = self.tray.at(now);
        let add_x = self.new_tab_x.at(now) + tray;
        div()
            .h(px(TITLE_BAR))
            .flex_shrink_0()
            .relative()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |event, window, cx| {
                        cx.stop_propagation();
                        if event.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .child(
                icon_button("toggle-sidebar", "sidebar", 16.)
                    .absolute()
                    .left(px(78.))
                    .top(px(8.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.set_peek(true, cx);
                        }
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
            )
            .child(
                div()
                    .absolute()
                    .right(px(10.))
                    .top(px(8.))
                    .flex()
                    .gap(px(4.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .id("top-diff")
                            .h(px(24.))
                            .px(px(6.))
                            .rounded_md()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.08)))
                            .on_click(cx.listener(|this, _, _, cx| this.open_pane(2, cx)))
                            .child(div().text_color(add_color).child(format!("+{added}")))
                            .child(div().text_color(remove_color).child(format!("-{removed}"))),
                    )
                    .child(icon_button("top-search", "search", 16.).on_click(cx.listener(|this, _, _, cx| this.toggle_palette(true, cx))))
                    .child(toggled(icon_button("top-environment", "window", 16.), self.environment.is_some()).on_click(cx.listener(|this, _, _, cx| {
                        this.appearance = None;
                        this.environment = match this.environment {
                            Some(_) => None,
                            None => Some(Tween::from_to(0., 1., Instant::now(), if cx.reduce_motion() { 0. } else { MENU }, Curve::Expo)),
                        };
                        cx.notify();
                    })))
                    .when(open_dock > 0.01, |d| {
                        d.child(div().flex().gap(px(4.)).opacity(open_dock)
                            .child(icon_button("dock-add", "plus", 14.).on_click(cx.listener(|this, _, _, cx| {
                                this.dock_active = None;
                                this.chooser_since = Instant::now();
                                cx.notify();
                            })))
                            .child(toggled(icon_button("dock-max", "expand", 14.), self.dock_max.to > 0.5).on_click(cx.listener(|this, _, _, cx| {
                                let to = if this.dock_max.to > 0.5 { 0. } else { 1. };
                                this.dock_max.set(to, Instant::now(), if cx.reduce_motion() { 0. } else { SIDEBAR }, Curve::Expo);
                                cx.notify();
                            }))))
                    })
                    .child(toggled(icon_button("top-panel", "sidebar-right", 16.), self.dock_open).on_click(cx.listener(|this, _, _, cx| this.toggle_dock(cx)))),
            )
            .child(nav_button("back", nav_end - 54., 0.5, self.can_navigate(false), cx, false))
            .child(nav_button("forward", nav_end - 28., 0., self.can_navigate(true), cx, true))
            .child(div().absolute().left(px(strip_left)).top_0().bottom_0().w(px(strip + PLATE + 4.)).children(dock).child(div().absolute().left(px(tray)).top_0().bottom_0().right_0().overflow_hidden().children(tabs)).child(
                icon_button("new-tab", "plus", 14.)
                    .absolute()
                    .left(px(add_x))
                    .top(px((TITLE_BAR - PLATE) / 2.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        let free = (0..this.conversations.len()).find(|c| !this.tabs.iter().any(|t| t.conversation == *c && !t.closing));
                        if let Some(free) = free {
                            this.open(free, cx);
                        }
                    })),
            ))
            .into_any_element()
    }

    fn sidebar_view(&mut self, now: Instant, moving: &mut bool, reduced: bool, cx: &mut Context<Self>) -> AnyElement {
        if self.peek.as_ref().is_some_and(|p| !p.open && p.t.done(now)) {
            self.peek = None;
        }
        let open = self.sidebar.at(now);
        *moving |= !self.sidebar.done(now);
        let (rows, height, rows_moving) = self.layout_rows(now, reduced);
        *moving |= rows_moving;
        let width = self.sidebar_width;
        let active = self.active_conversation();
        let mut drawn = Vec::new();
        for (row, y) in rows {
            let alpha = self.rows[&row].alpha.at(now);
            drawn.push(match row {
                Row::Heading(pinned) => div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(y))
                    .h(px(HEADING))
                    .opacity(alpha)
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .text_size(px(11.5))
                    .text_color(content(0.4))
                    .child(if pinned { "Fixados" } else { "Projetos" })
                    .into_any_element(),
                Row::Project(index) => self.project_row(index, y, alpha, cx),
                Row::Conversation(index) => self.conversation_row(index, y, alpha, index == active, now, cx),
            });
        }
        let contents = |id: &'static str, this: &Self, cx: &mut Context<Self>, drawn: Vec<AnyElement>| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(width))
                .flex()
                .flex_col()
                .child(this.sidebar_search(cx))
                .child(div().id(id).flex_1().min_h_0().overflow_y_scroll().child(div().relative().w_full().h(px(height + 12.)).children(drawn)))
        };
        // A collapsed sidebar slides out over the workspace while the pointer holds it there.
        if open < 0.001 {
            let Some(peek) = self.peek.as_ref() else { return div().into_any_element() };
            let t = peek.t.at(now).clamp(0., 1.);
            *moving |= !peek.t.done(now);
            return div()
                .id("sidebar-peek")
                .occlude()
                .absolute()
                .left(px(PEEK_INSET - (1. - t) * PEEK_DRIFT))
                .top(px(TITLE_BAR))
                .bottom(px(PEEK_INSET))
                .w(px(width))
                .opacity(t)
                .rounded(px(10.))
                .border_1()
                .border_color(content(0.1))
                .bg(field_fill())
                .shadow_lg()
                .overflow_hidden()
                .child(contents("sidebar-list", self, cx, drawn))
                .into_any_element();
        }
        div()
            .relative()
            .flex_shrink_0()
            .h_full()
            .w(px(width * open))
            .overflow_hidden()
            // The content keeps its width while the column slides, so nothing rewraps.
            .child(contents("sidebar-list", self, cx, drawn).opacity(open.clamp(0., 1.)))
            .child(
                div()
                    .id("sidebar-resize")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(px(5.))
                    .cursor_col_resize()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.resizing = true;
                    })),
            )
            .into_any_element()
    }

    /// ⌘K lives in the sidebar, as Monocode's: conversations, messages, projects and commands.
    fn sidebar_search(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_shrink_0()
            .h(px(SIDEBAR_SEARCH))
            .px(px(6.))
            .pt(px(6.))
            .child(
                div()
                    .id("sidebar-search")
                    .h(px(30.))
                    .px(px(8.))
                    .rounded_lg()
                    .border_1()
                    .border_color(content(0.09))
                    .bg(content(0.02))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(13.))
                    .text_color(content(0.5))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.06)).text_color(content(0.8)))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_palette(false, cx)))
                    .child(icon("search", 14.).text_color(content(0.5)))
                    .child(div().flex_1().child("Buscar"))
                    .child(div().text_size(px(11.)).text_color(content(0.35)).child("⌘K")),
            )
            .into_any_element()
    }

    /// Settings stay one click away at the footer's right end.
    fn settings_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("sidebar-settings")
            .h(px(22.))
            .px(px(6.))
            .rounded_md()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(content(0.55))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.08)).text_color(content(0.95)))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_appearance(cx)))
            .child(icon("settings", 13.).text_color(content(0.55)))
            .child("Configurações")
    }

    fn set_peek(&mut self, open: bool, cx: &mut Context<Self>) {
        self.peek_close = None;
        if open && (self.sidebar_open || self.sidebar.at(Instant::now()) > 0.001) {
            return;
        }
        let now = Instant::now();
        let from = self.peek.as_ref().map_or(0., |p| p.t.at(now));
        if self.peek.as_ref().is_some_and(|p| p.open == open) || (self.peek.is_none() && !open) {
            return;
        }
        let duration = if cx.reduce_motion() { 0. } else if open { 0.24 } else { 0.2 };
        self.peek = Some(Peek { open, t: Tween::from_to(from, if open { 1. } else { 0. }, now, duration, Curve::Expo) });
        cx.notify();
    }

    /// The window's pointer drives the peek: the left edge opens it; leaving the panel, after
    /// a short grace, puts it away.
    fn track_peek(&mut self, position: Point<Pixels>, height: f32, cx: &mut Context<Self>) {
        if self.sidebar_open {
            return;
        }
        let (x, y) = (f32::from(position.x), f32::from(position.y));
        let on_edge = x <= PEEK_EDGE && y >= TITLE_BAR;
        let on_toggle = (78.0..104.).contains(&x) && y < TITLE_BAR;
        let on_panel = self.peek.as_ref().is_some_and(|p| p.open) && x >= PEEK_INSET && x <= PEEK_INSET + self.sidebar_width && y >= TITLE_BAR && y <= height - PEEK_INSET;
        if on_edge || on_toggle || on_panel || self.palette_ui.is_some() || self.appearance.is_some() {
            if on_edge {
                self.set_peek(true, cx);
            } else if self.peek.as_ref().is_some_and(|p| p.open) {
                self.peek_close = None;
            }
            return;
        }
        if self.peek.as_ref().is_some_and(|p| p.open) && self.peek_close.is_none() {
            self.peek_close = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PEEK_CLOSE).await;
                let _ = this.update(cx, |this, cx| {
                    this.peek_close = None;
                    this.set_peek(false, cx);
                });
            }));
        }
    }

    fn project_row(&self, index: usize, y: f32, alpha: f32, cx: &mut Context<Self>) -> AnyElement {
        let project = &self.projects[index];
        let group: gpui::SharedString = format!("project{index}").into();
        let first_free = self.projects[index].conversations.first().copied();
        div()
            .id(("project", index))
            .group(group.clone())
            .absolute()
            .left(px(6.))
            .right(px(6.))
            .top(px(y))
            .h(px(PROJECT_ROW - 2.))
            .opacity(alpha)
            .rounded_md()
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(6.))
            .hover(|s| s.bg(content(0.05)))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_project(index, cx)))
            .child(icon("chevron-right", 12.).text_color(content(0.45)).with_transformation(Transformation::rotate(percentage(if project.open { 0.25 } else { 0. }))))
            .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(content(0.85)).child(project.name))
            // One click on a project shows its conversations; its pencil starts a new one.
            .child(
                div()
                    .id(("project-new", index))
                    .size(px(20.))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(1.))
                    .hover(|s| s.bg(content(0.1)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.projects[index].open = true;
                        if let Some(first) = first_free {
                            this.open(first, cx);
                        }
                    }))
                    .child(icon("plus", 13.).text_color(content(0.6))),
            )
            .into_any_element()
    }

    fn conversation_row(&self, index: usize, y: f32, alpha: f32, active: bool, now: Instant, cx: &mut Context<Self>) -> AnyElement {
        let conversation = &self.conversations[index];
        div()
            .id(("conversation", index))
            .absolute()
            .left(px(6.))
            .right(px(6.))
            .top(px(y))
            .h(px(CONV_ROW - 2.))
            .opacity(alpha)
            .rounded_md()
            .pl(px(10.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .when(active, |d| d.bg(content(0.09)))
            .when(!active, |d| d.hover(|s| s.bg(content(0.05))))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.open(index, cx)))
            .child(self.conversation_mark(conversation, now, active))
            .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).text_color(content(if active { 1. } else { 0.68 })).child(conversation.title.clone()))
            .child(div().flex_shrink_0().text_size(px(11.)).text_color(content(0.35)).child(conversation.age))
            .into_any_element()
    }

    fn transcript(&mut self, now: Instant, moving: &mut bool, cx: &mut Context<Self>) -> AnyElement {
        let index = self.active_conversation();
        let scroll = self.scroll(index);
        let switching = self.switch.at(now);
        *moving |= !self.switch.done(now);
        let live = running(&self.conversations[index], now);
        if live {
            scroll.scroll_to_bottom();
        }
        let conversation = &self.conversations[index];
        let turns: Vec<AnyElement> = conversation.turns.iter().enumerate().map(|(i, turn)| {
            self.turn_view(index, i, turn, now, moving, cx)
        }).collect();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .rounded(px(10.))
            .border_1()
            .border_color(content(0.08))
            .bg(pane_fill())
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(("transcript", index))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .opacity(switching.clamp(0., 1.))
                    .child(
                        div()
                            .relative()
                            .top(px((1. - switching) * 6.))
                            .mx_auto()
                            .w_full()
                            .max_w(px(760.))
                            .px(px(24.))
                            .pt(px(20.))
                            .pb(px(28.))
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .children(turns),
                    ),
            )
            // Text fades as it scrolls under the top edge and toward the composer.
            .child(div().absolute().top_0().left_0().right_0().h(px(26.)).bg(linear_gradient(180., linear_color_stop(pane_opaque(), 0.), linear_color_stop(transparent(), 1.))))
            .child(self.composer(live, cx))
            .into_any_element()
    }

    /// Composer B: the field is a raised card over a shelf holding the session's settings.
    /// Typing comes with the real composer in part 2; Enter simulates a reply.
    fn composer(&self, live: bool, cx: &mut Context<Self>) -> AnyElement {
        let provider = self.conversations[self.active_conversation()].provider;
        let round = |id: &'static str, name: &'static str| {
            div()
                .id(id)
                .size(px(30.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
                .child(icon(name, 15.).text_color(content(0.6)))
        };
        let shelf_button = |id: &'static str| {
            div().id(id).h(px(24.)).px(px(8.)).rounded(px(7.)).flex().items_center().gap(px(6.)).text_size(px(12.)).text_color(content(0.6)).cursor_pointer().hover(|s| s.bg(content(0.07)).text_color(content(0.95)))
        };
        let caret = || icon("chevron-right", 11.).text_color(content(0.4)).with_transformation(Transformation::rotate(percentage(0.25)));
        div()
            .relative()
            .flex_shrink_0()
            .mx_auto()
            .w_full()
            .max_w(px(760.))
            .px(px(24.))
            .pb(px(14.))
            .child(div().absolute().left_0().right_0().top(px(-24.)).h(px(24.)).bg(linear_gradient(0., linear_color_stop(pane_opaque(), 0.), linear_color_stop(transparent(), 1.))))
            .child(
                div()
                    .rounded(px(20.))
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.03))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .rounded(px(19.))
                            .border_1()
                            .border_color(content(0.07))
                            .bg(field_fill())
                            .shadow_md()
                            .pt(px(14.))
                            .px(px(8.))
                            .pb(px(8.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(div().px(px(8.)).text_size(px(14.)).line_height(px(22.)).text_color(content(0.4)).child("Peça qualquer coisa, / para skills, @ para arquivos"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .child(round("composer-add", "plus"))
                                    .child(div().flex_1())
                                    .child(round("composer-mic", "mic"))
                                    .child(
                                        div()
                                            .id("send")
                                            .ml(px(2.))
                                            .size(px(30.))
                                            .rounded_full()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .bg(if live { content(0.09) } else { accent() })
                                            .when(!live, |d| d.cursor_pointer().hover(|s| s.opacity(0.88)))
                                            .on_click(cx.listener(|this, _, _, cx| this.send(cx)))
                                            .child(icon("arrow-up", 15.).text_color(if live { content(0.4) } else { on_accent() })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .h(px(34.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .child(shelf_button("composer-access").child(icon("zap", 13.).text_color(content(0.6))).child("Acesso total").child(caret()))
                            .child(div().flex_1())
                            .child(shelf_button("composer-context").child(div().size(px(7.)).rounded_full().bg(accent())).child("34%"))
                            .child(shelf_button("composer-model").child(mark_tile(&crate::tab_tile::TileSize { w: 16., h: 14., radius: 4. }, Face::provider(provider), provider_glyph(provider, 10.))).child(model_name(provider)).child(caret())),
                    ),
            )
            .into_any_element()
    }

    /// The bar under the window, like the title bar: the model's usage and the actions.
    /// Sample numbers; they come from the provider in part 2.
    fn footer(&self, left: f32, cx: &mut Context<Self>) -> AnyElement {
        let provider = self.conversations[self.active_conversation()].provider;
        div()
            .relative()
            .h(px(FOOTER))
            .flex_shrink_0()
            .pl(px(left + 8.))
            .pr(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(11.))
            .text_color(content(0.55))
            .child(
                div()
                    .id("usage")
                    .h(px(20.))
                    .px(px(6.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.1)).text_color(content(1.)))
                    .child(mark_tile(&crate::tab_tile::TileSize { w: 14., h: 12., radius: 3. }, Face::provider(provider), provider_glyph(provider, 9.)))
                    .child(div().w(px(32.)).h(px(4.)).rounded_full().bg(content(0.1)).overflow_hidden().child(div().h_full().w(px(32. * 0.15)).rounded_full().bg(content(0.7))))
                    .child("15% 2h 56m · 41% 2d 22h"),
            )
            .child(div().id("refresh-usage").size(px(20.)).ml(px(-4.)).rounded_full().flex().items_center().justify_center().cursor_pointer().hover(|s| s.bg(content(0.1))).child(icon("refresh", 12.).text_color(content(0.55))))
            .child(div().flex_1())
            .child(self.settings_button(cx))
            .into_any_element()
    }

    fn turn_view(&self, conversation: usize, index: usize, turn: &Turn, now: Instant, moving: &mut bool, cx: &mut Context<Self>) -> AnyElement {
        let provider = self.conversations[conversation].provider;
        let elapsed = turn.started.map(|s| now.duration_since(s).as_secs_f32());
        let work: f32 = turn.steps.iter().map(|s| s.seconds).sum();
        let answer_start = THINKING + work;
        let words = turn.answer.split_whitespace().count() as f32;
        let done_at = answer_start + words / WORDS_PER_SECOND;
        // History shows finished, folded work; a live turn plays its phases from the clock.
        let e = elapsed.unwrap_or(f32::MAX);
        let working = e < answer_start;
        let streaming = !working && e < done_at;
        let open_now = open_target(turn, e, answer_start, done_at);
        let openness = match (turn.toggled, elapsed) {
            (Some(at), _) => {
                let since = now.duration_since(at).as_secs_f32();
                if since < DISCLOSE { *moving = true; }
                let p = eased(since / DISCLOSE, Curve::Expo);
                if turn.open { p } else { 1. - p }
            }
            (None, Some(_)) if e >= done_at => {
                let since = e - done_at;
                if since < DISCLOSE { *moving = true; }
                1. - eased(since / DISCLOSE, Curve::Expo)
            }
            (None, Some(_)) => 1.,
            (None, None) => 0.,
        };
        let clock = now.duration_since(self.epoch).as_secs_f32();
        let name = model_name(provider);
        let label: AnyElement = if e < THINKING {
            shimmer(format!("{name} pensando"), clock)
        } else if working {
            shimmer(format!("{name} trabalhando"), clock)
        } else {
            div().child(format!("{name} trabalhou por {}s", (work + THINKING).round() as u32)).into_any_element()
        };
        // Pointing at the tile pushes the model's mark out to the right as the chevron slides
        // in from the left; an open fold keeps the chevron (Monocode's phase_tile.rs).
        let roll = self.roll(conversation, index * 2, now, moving);
        let shown = if working { roll } else { roll.max(openness) }.clamp(0., 1.);
        let chevron = icon("chevron-right", 14.).text_color(gpui::white()).with_transformation(Transformation::rotate(percentage(openness * 0.25))).into_any_element();
        let header = div()
            .id(("activity", index))
            .h(px(STEP_ROW))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(14.))
            .text_color(content(0.5))
            .hover(|s| s.text_color(content(0.8)))
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.set_roll(conversation, index * 2, *hovered, cx)))
            .on_click(cx.listener(move |this, _, _, cx| {
                let turn = &mut this.conversations[conversation].turns[index];
                let now = Instant::now();
                let e = turn.started.map_or(f32::MAX, |s| now.duration_since(s).as_secs_f32());
                let work: f32 = turn.steps.iter().map(|s| s.seconds).sum();
                let words = turn.answer.split_whitespace().count() as f32;
                turn.open = !open_target(turn, e, THINKING + work, THINKING + work + words / WORDS_PER_SECOND);
                turn.toggled = Some(now);
                cx.notify();
            }))
            .child(push_tile(&ACTIVITY_TILE, Face::provider(provider), provider_glyph(provider, 12.), chevron, shown))
            .child(label);

        let mut rows: Vec<(f32, AnyElement)> = Vec::new();
        let child_row = |height: f32, grow: f32, last: bool, inner: AnyElement| {
            div()
                .relative()
                .w_full()
                .h(px(height))
                .opacity(grow)
                .overflow_hidden()
                .pl(px(TILE_RAIL_INDENT))
                .flex()
                .items_center()
                .gap(px(7.))
                .text_size(px(13.))
                .text_color(content(0.55))
                .child(rail(last))
                .child(inner)
                .into_any_element()
        };
        let prose_grow = eased((e - THINKING) / 0.2, Curve::Expo);
        if e >= THINKING {
            let last = !working && turn.steps.is_empty();
            rows.push((STEP_ROW * prose_grow, child_row(STEP_ROW * prose_grow, prose_grow, last, div().flex_1().min_w_0().truncate().child(turn.prose).into_any_element())));
        }
        let step_text = |step: &data::Step| format!("{} {}", step.label, step.detail);
        if working {
            let mut at = THINKING + 0.35;
            let visible = turn.steps.iter().scan(at, |t, s| { let begin = *t; *t += s.seconds; Some(begin) }).filter(|b| e >= *b).count();
            for (i, step) in turn.steps.iter().enumerate() {
                let begin = at;
                at += step.seconds;
                if e < begin {
                    break;
                }
                let grow = eased((e - begin) / 0.2, Curve::Expo);
                let live = e < at;
                let text: AnyElement = if live { shimmer(step_text(step), clock) } else { div().truncate().child(step_text(step)).into_any_element() };
                let inner = div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(12.5))
                    .child(if live {
                        mark_tile(&ACTIVITY_TILE, Face::Neutral, self.spin(now)).into_any_element()
                    } else {
                        let (face, mark, _) = activity_face(step_glyph(step));
                        mark_tile(&ACTIVITY_TILE, face, mark).into_any_element()
                    })
                    .child(div().flex_1().min_w_0().child(text))
                    .into_any_element();
                rows.push((STEP_ROW * grow, child_row(STEP_ROW * grow, grow, i + 1 == visible, inner)));
            }
        } else {
            let nested_turn = turn.steps_toggled.map_or(if turn.steps_open { 1. } else { 0. }, |at| {
                let p = eased(now.duration_since(at).as_secs_f32() / DISCLOSE, Curve::Expo);
                if turn.steps_open { p } else { 1. - p }
            });
            let roll = self.roll(conversation, index * 2 + 1, now, moving);
            let (face, mark, ink) = activity_face(turn.steps.iter().map(step_glyph).next().unwrap_or("terminal"));
            let chevron = icon("chevron-right", 14.).text_color(ink).with_transformation(Transformation::rotate(percentage(nested_turn * 0.25))).into_any_element();
            let inner = div()
                .id(("summary", index))
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .hover(|s| s.text_color(content(0.8)))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.set_roll(conversation, index * 2 + 1, *hovered, cx)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let turn = &mut this.conversations[conversation].turns[index];
                    turn.steps_open = !turn.steps_open;
                    turn.steps_toggled = Some(Instant::now());
                    cx.notify();
                }))
                .child(push_tile(&ACTIVITY_TILE, face, mark, chevron, roll.max(nested_turn).clamp(0., 1.)))
                .child(summary(turn.steps))
                .into_any_element();
            rows.push((STEP_ROW + 2., child_row(STEP_ROW + 2., 1., true, inner)));
            let nested = match turn.steps_toggled {
                Some(at) => {
                    let since = now.duration_since(at).as_secs_f32();
                    if since < DISCLOSE { *moving = true; }
                    let p = eased(since / DISCLOSE, Curve::Expo);
                    if turn.steps_open { p } else { 1. - p }
                }
                None => 0.,
            };
            if nested > 0.001 {
                for step in turn.steps {
                    let height = 22. * nested;
                    rows.push((height, div().w_full().h(px(height)).opacity(nested).overflow_hidden().pl(px(TILE_RAIL_INDENT + ACTIVITY_TILE.w + 8.)).flex().items_center().gap(px(7.)).text_size(px(12.5)).text_color(content(0.45))
                        .child(div().flex_1().min_w_0().truncate().child(step_text(step)))
                        .into_any_element()));
                }
            }
        }
        let _ = open_now;
        let body_height: f32 = rows.iter().map(|(h, _)| *h).sum();
        let body: Vec<AnyElement> = rows.into_iter().map(|(_, row)| row).collect();
        let answer = if streaming {
            let count = ((e - answer_start) * WORDS_PER_SECOND) as usize;
            Some(turn.answer.split_whitespace().take(count).collect::<Vec<_>>().join(" "))
        } else if !working {
            Some(turn.answer.clone())
        } else {
            None
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div().flex().justify_end().child(
                    div()
                        .max_w(px(520.))
                        .rounded(px(14.))
                        .px(px(12.))
                        .py(px(8.))
                        .bg(accent_wash(0.16))
                        .text_size(px(14.))
                        .line_height(px(21.))
                        .text_color(content(0.95))
                        .child(turn.user.clone()),
                ),
            )
            .child(header)
            .child(div().w_full().mt(px(-6.)).h(px(body_height * openness)).opacity(openness.clamp(0., 1.)).overflow_hidden().children(body))
            .when_some(answer, |d, answer| d.child(div().text_size(px(14.)).line_height(px(23.)).text_color(content(0.88)).child(answer)))
            .into_any_element()
    }

    fn toggle_dock(&mut self, cx: &mut Context<Self>) {
        self.dock_open = !self.dock_open;
        if self.dock_open && self.dock_active.is_none() {
            self.chooser_since = Instant::now();
        }
        self.dock.set(if self.dock_open { 1. } else { 0. }, Instant::now(), if cx.reduce_motion() { 0. } else { SIDEBAR }, Curve::Expo);
        cx.notify();
    }

    fn open_pane(&mut self, kind: usize, cx: &mut Context<Self>) {
        if !self.dock_panes.contains(&kind) {
            self.dock_panes.push(kind);
        }
        self.dock_active = Some(kind);
        if !self.dock_open {
            self.toggle_dock(cx);
        }
        cx.notify();
    }

    fn close_pane(&mut self, kind: usize, cx: &mut Context<Self>) {
        self.dock_panes.retain(|k| *k != kind);
        if self.dock_active == Some(kind) {
            self.dock_active = self.dock_panes.last().copied();
            if self.dock_active.is_none() {
                self.chooser_since = Instant::now();
            }
        }
        cx.notify();
    }

    /// The side panel: its open panes as chips, or the six options in two columns.
    fn dock_view(&self, width: f32, now: Instant, moving: &mut bool, cx: &mut Context<Self>) -> AnyElement {
        let body: AnyElement = match self.dock_active {
            None => {
                let since = now.duration_since(self.chooser_since).as_secs_f32();
                let reduced = cx.reduce_motion();
                if since < 0.4 && !reduced {
                    *moving = true;
                }
                let cards = PANES.iter().enumerate().map(|(ix, (glyph, name, about, key))| {
                    // Each card rises in just after the one before it.
                    let t = if reduced { 1. } else { eased((since - ix as f32 * 0.03) / 0.22, Curve::Expo) };
                    div()
                        .id(("pane-option", ix))
                        .relative()
                        .top(px((1. - t) * 6.))
                        .opacity(t)
                        .w(px(205.))
                        .h(px(70.))
                        .p(px(12.))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(content(0.1))
                        .bg(content(0.03))
                        .hover(|s| s.bg(content(0.07)).border_color(content(0.16)))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.open_pane(ix, cx)))
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(icon(glyph, 15.).text_color(content(0.75)))
                                .child(div().flex_1().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).text_color(content(0.95)).child(*name))
                                .child(div().text_size(px(11.)).text_color(content(0.3)).child(key.to_uppercase())),
                        )
                        .child(div().truncate().text_size(px(12.)).text_color(content(0.5)).child(*about))
                });
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(5.))
                    .child(div().text_size(px(14.5)).font_weight(FontWeight::MEDIUM).text_color(content(0.95)).child("Abrir um painel"))
                    .child(div().mb(px(14.)).text_size(px(12.5)).text_color(content(0.5)).child("Escolha o que abrir ao lado da conversa."))
                    .child(div().w(px(420.)).flex().flex_wrap().justify_center().gap(px(10.)).children(cards))
                    .into_any_element()
            }
            Some(kind) => {
                let chips = self.dock_panes.iter().map(|k| {
                    let k = *k;
                    let (glyph, name, _, _) = PANES[k];
                    let active = self.dock_active == Some(k);
                    let group: gpui::SharedString = format!("chip{k}").into();
                    div()
                        .id(("dock-chip", k))
                        .group(group.clone())
                        .relative()
                        .w(px(112.))
                        .h(px(24.))
                        .pl(px(8.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.))
                        .text_color(content(if active { 1. } else { 0.55 }))
                        .bg(content(if active { 0.1 } else { 0.04 }))
                        .when(!active, |d| d.hover(|s| s.bg(content(0.07))))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dock_active = Some(k);
                            cx.notify();
                        }))
                        .child(icon(glyph, 13.).text_color(content(0.7)))
                        .child(div().flex_1().min_w_0().truncate().child(name))
                        .child(
                            div()
                                .id(("dock-chip-close", k))
                                .absolute()
                                .right(px(3.))
                                .top(px(3.))
                                .size(px(18.))
                                .rounded_sm()
                                .flex()
                                .items_center()
                                .justify_center()
                                .opacity(0.)
                                .group_hover(group, |s| s.opacity(1.))
                                .hover(|s| s.bg(content(0.14)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.close_pane(k, cx);
                                }))
                                .child(icon("close", 11.).text_color(content(0.7))),
                        )
                });
                let (glyph, name, _, _) = PANES[kind];
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(div().h(px(36.)).flex_shrink_0().px(px(6.)).flex().items_center().gap(px(4.)).border_b_1().border_color(content(0.07)).children(chips))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap(px(8.))
                            .child(icon(glyph, 22.).text_color(content(0.35)))
                            .child(div().text_size(px(13.)).text_color(content(0.5)).child(format!("{name}: conteúdo real nas próximas partes"))),
                    )
                    .into_any_element()
            }
        };
        div()
            .relative()
            .flex_shrink_0()
            .h_full()
            .w(px(width))
            .overflow_hidden()
            .rounded(px(10.))
            .border_1()
            .border_color(content(0.08))
            .bg(pane_fill())
            .flex()
            .flex_col()
            .child(body)
            .into_any_element()
    }

    fn toggle_palette(&mut self, files: bool, cx: &mut Context<Self>) {
        self.environment = None;
        self.appearance = None;
        let same = self.palette_ui.as_ref().is_some_and(|p| p.files == files);
        self.palette_ui = if same {
            None
        } else {
            Some(PaletteState { files, query: String::new(), selected: 0, open: Tween::from_to(0., 1., Instant::now(), if cx.reduce_motion() { 0. } else { MENU }, Curve::Expo) })
        };
        cx.notify();
    }

    /// The active conversation's lines added and removed, sample numbers.
    fn diff_stat(&self) -> (u32, u32) {
        let c = self.active_conversation() as u32;
        (((c * 37 + 12) % 240), ((c * 19 + 3) % 80))
    }

    fn active_project(&self) -> &'static str {
        let c = self.active_conversation();
        self.projects.iter().find(|p| p.conversations.contains(&c)).map_or("sirus-code", |p| p.name)
    }

    /// As Sirus's command palette: conversations, words inside messages, then commands.
    fn palette_hits(&self, files: bool, query: &str) -> Vec<PaletteHit> {
        let needle = query.trim().to_lowercase();
        let mut hits = Vec::new();
        if files {
            if let Some(commands) = needle.strip_prefix('>') {
                let commands = commands.trim();
                for (ix, label) in COMMANDS.iter().enumerate() {
                    if label.to_lowercase().contains(commands) {
                        hits.push(PaletteHit::Command(ix));
                    }
                }
                return hits;
            }
            for (name, folder) in data::files(self.active_project()) {
                if needle.is_empty() || format!("{folder}/{name}").to_lowercase().contains(&needle) {
                    hits.push(PaletteHit::File(name, folder));
                }
            }
            hits.truncate(9);
            return hits;
        }
        for (ix, c) in self.conversations.iter().enumerate() {
            if needle.is_empty() || c.title.to_lowercase().contains(&needle) {
                hits.push(PaletteHit::Chat(ix));
            }
        }
        if !needle.is_empty() {
            for (ix, c) in self.conversations.iter().enumerate() {
                let found = c.turns.iter().find_map(|t| {
                    let text = [t.user.as_str(), t.answer.as_str()].into_iter().find(|text| text.to_lowercase().contains(&needle))?;
                    Some(text.chars().take(90).collect::<String>())
                });
                if let Some(snippet) = found {
                    hits.push(PaletteHit::Message(ix, snippet));
                }
            }
        }
        if !needle.is_empty() {
            for (ix, project) in self.projects.iter().enumerate() {
                if project.name.contains(&needle) {
                    hits.push(PaletteHit::Project(ix));
                }
            }
        }
        for (ix, label) in COMMANDS.iter().enumerate() {
            if label.to_lowercase().contains(&needle) {
                hits.push(PaletteHit::Command(ix));
            }
        }
        hits.truncate(if needle.is_empty() { 8 } else { 12 });
        hits
    }

    fn run_hit(&mut self, hit: PaletteHit, cx: &mut Context<Self>) {
        self.palette_ui = None;
        match hit {
            PaletteHit::Chat(c) | PaletteHit::Message(c, _) => self.open(c, cx),
            PaletteHit::File(..) => self.open_pane(1, cx),
            PaletteHit::Project(ix) => {
                self.projects[ix].open = true;
                if let Some(first) = self.projects[ix].conversations.first().copied() {
                    self.open(first, cx);
                }
            }
            PaletteHit::Command(0) => {
                let free = (0..self.conversations.len()).find(|c| !self.tabs.iter().any(|t| t.conversation == *c && !t.closing));
                if let Some(free) = free {
                    self.open(free, cx);
                }
            }
            PaletteHit::Command(1) => self.toggle_sidebar(cx),
            PaletteHit::Command(2) => self.toggle_dock(cx),
            PaletteHit::Command(3) => self.toggle_appearance(cx),
            PaletteHit::Command(_) => self.toggle_pin(self.active, cx),
        }
        cx.notify();
    }

    fn palette_view(&self, now: Instant, moving: &mut bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.palette_ui.as_ref()?;
        if !state.open.done(now) {
            *moving = true;
        }
        let p = state.open.at(now).clamp(0., 1.);
        let hits = self.palette_hits(state.files, &state.query);
        let selected = state.selected.min(hits.len().saturating_sub(1));
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut group = "";
        for (ix, hit) in hits.iter().enumerate() {
            let (heading, title, detail, mark): (&str, String, Option<String>, AnyElement) = match hit {
                PaletteHit::Chat(c) => {
                    let conv = &self.conversations[*c];
                    ("Conversas recentes", conv.title.clone(), Some(conv.age.to_string()), mark_tile(&crate::tab_tile::TileSize { w: 18., h: 16., radius: 4. }, Face::provider(conv.provider), provider_glyph(conv.provider, 11.)).into_any_element())
                }
                PaletteHit::Message(c, snippet) => {
                    let conv = &self.conversations[*c];
                    ("Nas mensagens", snippet.clone(), Some(conv.title.clone()), mark_tile(&crate::tab_tile::TileSize { w: 18., h: 16., radius: 4. }, Face::provider(conv.provider), provider_glyph(conv.provider, 11.)).into_any_element())
                }
                PaletteHit::File(name, folder) => ("", name.to_string(), Some(if folder.is_empty() { name.to_string() } else { format!("{folder}/{name}") }), file_mark(name)),
                PaletteHit::Project(p) => ("Projetos", self.projects[*p].name.to_string(), None, div().size(px(18.)).flex().items_center().justify_center().child(icon("folder", 14.).text_color(content(0.6))).into_any_element()),
                PaletteHit::Command(i) => ("Comandos", COMMANDS[*i].to_string(), None, div().size(px(18.)).flex().items_center().justify_center().child(icon("chevron-right", 12.).text_color(content(0.5))).into_any_element()),
            };
            if heading != group && !heading.is_empty() {
                group = heading;
                rows.push(div().px(px(14.)).pt(px(8.)).pb(px(3.)).text_size(px(11.5)).text_color(content(0.4)).child(heading).into_any_element());
            }
            let hit = hit.clone();
            let two_lines = matches!(hit, PaletteHit::File(..));
            rows.push(
                div()
                    .id(("palette-row", ix))
                    .mx(px(6.))
                    .h(px(if two_lines { 46. } else { 34. }))
                    .px(px(8.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(13.))
                    .text_color(content(0.9))
                    .when(ix == selected, |d| d.bg(content(0.1)))
                    .when(ix != selected, |d| d.hover(|s| s.bg(content(0.05))))
                    .cursor_pointer()
                    .on_click(cx.listener({ let hit = hit.clone(); move |this, _, _, cx| this.run_hit(hit.clone(), cx) }))
                    .child(mark)
                    .map(|d| {
                        if two_lines {
                            d.child(div().flex_1().min_w_0().flex().flex_col().child(div().truncate().child(title)).children(detail.clone().map(|d| div().truncate().text_size(px(11.5)).text_color(content(0.45)).child(d))))
                        } else {
                            d.child(div().flex_1().min_w_0().truncate().child(title)).children(detail.clone().map(|d| div().flex_shrink_0().max_w(px(170.)).truncate().text_size(px(11.5)).text_color(content(0.4)).child(d)))
                        }
                    })
                    .into_any_element(),
            );
        }
        if hits.is_empty() {
            rows.push(div().px(px(16.)).py(px(18.)).text_size(px(13.)).text_color(content(0.45)).child("Nada encontrado.").into_any_element());
        }
        Some(
            div()
                .id("palette-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .bg(gpui::hsla(0., 0., 0., 0.38 * p))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.palette_ui = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("palette")
                        .occlude()
                        .absolute()
                        .left(gpui::relative(0.5))
                        .ml(px(-290.))
                        .top(px(92. + (1. - p) * -8.))
                        .w(px(580.))
                        .opacity(p)
                        .rounded(px(14.))
                        .border_1()
                        .border_color(content(0.12))
                        .bg(floating_fill())
                        .shadow_lg()
                        .pb(px(6.))
                        .flex()
                        .flex_col()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h(px(50.))
                                .px(px(16.))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .border_b_1()
                                .border_color(content(0.08))
                                .text_size(px(14.))
                                .child(icon("search", 15.).text_color(content(0.5)))
                                .child(div().w(px(1.5)).h(px(18.)).bg(accent()))
                                .child(if state.query.is_empty() {
                                    div().text_color(content(0.4)).child(if state.files { "Buscar arquivos pelo nome…  (> para comandos)" } else { "Buscar conversas, mensagens ou comandos" })
                                } else {
                                    div().text_color(content(0.95)).child(state.query.clone())
                                }),
                        )
                        .children(rows)
                        .when(state.files, |d| {
                            d.child(div().mt(px(4.)).h(px(32.)).px(px(16.)).flex().items_center().gap(px(14.)).border_t_1().border_color(content(0.08)).text_size(px(11.5)).text_color(content(0.4)).child("↑ ↓ navegar").child("↵ abrir").child("Esc fechar").child("> comandos"))
                        }),
                )
                .into_any_element(),
        )
    }

    /// The Ambiente card: the conversation's changes, checkout, branch and servers, in Sirus's
    /// order. Sample values; only Alterações does something here.
    fn environment_view(&self, now: Instant, moving: &mut bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.environment.as_ref()?;
        if !open.done(now) {
            *moving = true;
        }
        let p = open.at(now).clamp(0., 1.);
        let row = |id: &'static str, glyph: &'static str, label: &'static str| {
            div()
                .id(id)
                .h(px(28.))
                .px(px(8.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(13.))
                .text_color(content(0.75))
                .hover(|s| s.bg(content(0.07)).text_color(content(1.)))
                .cursor_pointer()
                .child(icon(glyph, 15.).text_color(content(0.5)))
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        let label = |text: &'static str| div().px(px(8.)).pt(px(3.)).pb(px(2.)).text_size(px(12.)).text_color(content(0.45)).child(text);
        let divider = || div().my(px(3.)).h(px(1.)).bg(content(0.08));
        Some(
            div()
                .id("environment-card")
                .occlude()
                .absolute()
                .right(px(10.))
                .top(px(TITLE_BAR + 2. + (1. - p) * -6.))
                .w(px(300.))
                .p(px(6.))
                .rounded_lg()
                .border_1()
                .border_color(content(0.12))
                .bg(floating_fill())
                .shadow_lg()
                .opacity(p)
                .flex()
                .flex_col()
                .gap(px(2.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div().h(px(24.)).px(px(8.)).flex().items_center().justify_between().text_size(px(13.)).text_color(content(0.5)).child("Ambiente").child(icon("settings", 14.).text_color(content(0.5))),
                )
                .child(
                    row("env-changes", "git-compare", "Alterações")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.environment = None;
                            this.open_pane(2, cx);
                        }))
                        .child(div().text_size(px(12.)).text_color(diff_colors(self.look.diff).0).child(format!("+{}", self.diff_stat().0)))
                        .child(div().text_size(px(12.)).text_color(diff_colors(self.look.diff).1).child(format!("-{}", self.diff_stat().1))),
                )
                .child(row("env-local", "laptop", "Local"))
                .child(row("env-branch", "branch", "main"))
                .child(row("env-push", "upload", "Fazer commit e enviar"))
                .child(row("env-servers", "server", "Servidores locais"))
                .child(divider())
                .child(label("Repositório"))
                .child(row("env-repo", "folder", "sirus-code"))
                .into_any_element(),
        )
    }

    fn toggle_appearance(&mut self, cx: &mut Context<Self>) {
        self.appearance = match self.appearance {
            Some(_) => None,
            None => Some(Tween::from_to(0., 1., Instant::now(), if cx.reduce_motion() { 0. } else { MENU }, Curve::Expo)),
        };
        cx.notify();
    }

    /// Colors ease to the new look instead of cutting to it.
    fn set_look(&mut self, change: impl FnOnce(&mut Look), cx: &mut Context<Self>) {
        let now = Instant::now();
        self.palette_from = crate::theme::current_palette();
        change(&mut self.look);
        self.palette_t = Tween::from_to(0., 1., now, if cx.reduce_motion() { 0. } else { 0.26 }, Curve::Expo);
        let _ = now;
        cx.notify();
    }

    fn set_slider(&mut self, id: usize, x: f32, cx: &mut Context<Self>) {
        let Some(bounds) = self.slider_bounds.borrow()[id] else { return };
        let v = ((x - f32::from(bounds.left())) / f32::from(bounds.size.width)).clamp(0., 1.);
        let (hue, light) = self.look.custom.unwrap_or((260., 0.72));
        self.look.custom = Some(if id == 0 { (v * 360., light) } else { (hue, 0.35 + v * 0.5) });
        cx.notify();
    }

    fn slider(&self, id: usize, value: f32, stops: Vec<gpui::Hsla>, cx: &mut Context<Self>) -> AnyElement {
        let store = self.slider_bounds.clone();
        div()
            .id(("custom-slider", id))
            .relative()
            .w(px(200.))
            .h(px(14.))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.slider_drag = Some(id);
                this.set_slider(id, f32::from(event.position.x), cx);
            }))
            .child(gpui::canvas(move |bounds, _, _| store.borrow_mut()[id] = Some(bounds), |_, _, _, _| {}).absolute().size_full())
            .child(div().absolute().left_0().right_0().top(px(4.)).h(px(6.)).rounded_full().overflow_hidden().flex().children(stops.into_iter().map(|c| div().flex_1().h_full().bg(c))))
            .child(div().absolute().left(px(value * 200. - 7.)).top_0().size(px(14.)).rounded_full().bg(gpui::white()).border_1().border_color(gpui::hsla(0., 0., 0., 0.3)).shadow_md())
            .into_any_element()
    }

    /// The Settings page, as Sirus's today: the whole window, a 256 px menu with Back to app,
    /// search and the same pages, and a 780 px column. Aparência works; the other pages come
    /// when real settings connect.
    fn appearance_view(&self, now: Instant, moving: &mut bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.appearance.as_ref()?;
        if !open.done(now) {
            *moving = true;
        }
        let p = open.at(now).clamp(0., 1.);
        let since = now.duration_since(self.settings_since).as_secs_f32();
        let reduced = cx.reduce_motion();
        if since < 0.28 && !reduced {
            *moving = true;
        }
        let page_t = if reduced { 1. } else { eased(since / 0.28, Curve::Expo) };
        let nav = div()
            .id("settings-nav")
            .w(px(256.))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(content(0.08))
            .pt(px(TITLE_BAR))
            .px(px(8.))
            .overflow_y_scroll()
            .child(
                div()
                    .id("settings-back")
                    .h(px(30.))
                    .mb(px(10.))
                    .px(px(10.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(13.))
                    .text_color(content(0.95))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.appearance = None;
                        cx.notify();
                    }))
                    .child(icon("arrow-left", 16.).text_color(content(0.65)))
                    .child("Voltar ao app"),
            )
            .child(
                div().h(px(34.)).mb(px(10.)).px(px(10.)).rounded_lg().border_1().border_color(content(0.09)).flex().items_center().gap(px(8.)).text_size(px(13.)).text_color(content(0.4))
                    .child(icon("search", 14.).text_color(content(0.45)))
                    .child("Buscar configurações"),
            )
            .children(SETTINGS_NAV.iter().enumerate().map(|(g, (group, items))| {
                div()
                    .child(div().mt(px(if g == 0 { 4. } else { 16. })).mb(px(4.)).px(px(10.)).text_size(px(11.5)).text_color(content(0.45)).child(*group))
                    .children(items.iter().map(|(glyph, name)| {
                        let name: &'static str = name;
                        let chosen = name == self.settings_page;
                        div()
                            .id(name)
                            .h(px(32.))
                            .px(px(10.))
                            .rounded_md()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .text_size(px(13.))
                            .text_color(content(if chosen { 1. } else { 0.72 }))
                            .when(chosen, |d| d.bg(content(0.1)))
                            .when(!chosen, |d| d.hover(|s| s.bg(content(0.06))))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.settings_page == name {
                                    return;
                                }
                                let order = |n: &str| SETTINGS_NAV.iter().flat_map(|(_, items)| items.iter()).position(|(_, i)| *i == n).unwrap_or(0);
                                this.settings_direction = if order(name) > order(this.settings_page) { 1. } else { -1. };
                                this.settings_page = name;
                                this.settings_since = Instant::now();
                                cx.notify();
                            }))
                            .child(icon(glyph, 16.).text_color(content(if chosen { 0.9 } else { 0.55 })))
                            .child(name)
                    }))
            }));
        let body: AnyElement = if self.settings_page == APPEARANCE {
            self.appearance_page(cx)
        } else {
            div()
                .child(div().text_size(px(19.)).font_weight(FontWeight::MEDIUM).text_color(content(1.)).child(self.settings_page))
                .child(div().mt(px(6.)).text_size(px(13.)).text_color(content(0.5)).child("Esta página é a mesma do Sirus de hoje e entra quando ligarmos as configurações reais."))
                .into_any_element()
        };
        Some(
            div()
                .id("settings-page")
                .occlude()
                .absolute()
                .inset_0()
                .opacity(p)
                .bg(pane_opaque())
                .flex()
                .child(nav)
                .child(
                    div()
                        .id("settings-main")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .bg(pane_fill())
                        .overflow_y_scroll()
                        .child(
                            div()
                                .relative()
                                .top(px((1. - page_t) * 22. * self.settings_direction))
                                .opacity(page_t)
                                .mx_auto()
                                .w_full()
                                .max_w(px(780.))
                                .pt(px(TITLE_BAR + 12.))
                                .pb(px(40.))
                                .px(px(32.))
                                .child(body),
                        ),
                )
                .into_any_element(),
        )
    }

    fn appearance_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let look = self.look;
        let row = |title: &'static str, about: &'static str, control: AnyElement| {
            div()
                .px(px(16.))
                .py(px(14.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(20.))
                .border_b_1()
                .border_color(content(0.07))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(content(0.95)).child(title)).child(div().text_size(px(12.)).text_color(content(0.5)).child(about)))
                .child(control)
                .into_any_element()
        };
        let group = |title: &'static str, rows: Vec<AnyElement>| {
            div()
                .mb(px(32.))
                .child(div().mb(px(4.)).text_size(px(12.5)).text_color(content(0.5)).child(title))
                .child(div().rounded(px(12.)).border_1().border_color(content(0.09)).bg(content(0.02)).overflow_hidden().children(rows))
        };
        fn segmented(id: &'static str, options: &[&'static str], chosen: usize, cx: &mut Context<Shell>, pick: fn(&mut Look, usize)) -> AnyElement {
            div()
                .flex()
                .p(px(2.))
                .rounded(px(8.))
                .border_1()
                .border_color(content(0.1))
                .children(options.iter().enumerate().map(|(ix, label)| {
                    div()
                        .id((id, ix))
                        .h(px(24.))
                        .px(px(10.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .text_size(px(12.5))
                        .text_color(content(if ix == chosen { 1. } else { 0.55 }))
                        .when(ix == chosen, |d| d.bg(content(0.1)))
                        .when(ix != chosen, |d| d.hover(|s| s.text_color(content(0.9))))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.set_look(|l| pick(l, ix), cx)))
                        .child(*label)
                }))
                .into_any_element()
        }
        // Theme, as ThemeModePicker: three small windows to choose from.
        let modes = div().flex().gap(px(12.)).children([(0usize, "Sistema"), (2, "Claro"), (1, "Escuro")].into_iter().map(|(mode, label)| {
            let chosen = look.mode == mode;
            let art = |dark: bool| {
                let (bg, line) = if dark { (rgb(0x1a1a1f), rgb(0x3a3a42)) } else { (rgb(0xf1f1f4), rgb(0xcfcfd6)) };
                div().flex_1().h_full().bg(bg).p(px(8.)).flex().flex_col().gap(px(5.)).child(div().w(px(38.)).h(px(5.)).rounded_full().bg(line)).child(div().w(px(24.)).h(px(5.)).rounded_full().bg(line))
            };
            div()
                .id(("mode", mode))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.set_look(|l| l.mode = mode, cx)))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .w(px(150.))
                        .h(px(88.))
                        .rounded(px(10.))
                        .border_2()
                        .border_color(if chosen { accent() } else { content(0.1) })
                        .overflow_hidden()
                        .flex()
                        .map(|d| match mode {
                            0 => d.child(art(false)).child(art(true)),
                            2 => d.child(art(false)),
                            _ => d.child(art(true)),
                        }),
                )
                .child(div().text_size(px(13.)).text_color(content(if chosen { 1. } else { 0.65 })).child(label))
        }));
        let themes = div().w(px(330.)).flex().flex_wrap().justify_end().gap(px(4.)).children(PRESETS.iter().enumerate().map(|(ix, preset)| {
            let colors = palette(&Look { preset: ix, mode: 1, ..Look::default() }, true);
            let chosen = ix == look.preset;
            div()
                .id(("theme", ix))
                .h(px(28.))
                .w(px(106.))
                .px(px(5.))
                .rounded_md()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.5))
                .text_color(content(if chosen { 1. } else { 0.7 }))
                .when(chosen, |d| d.bg(content(0.1)))
                .when(!chosen, |d| d.hover(|s| s.bg(content(0.06))))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_look(|l| {
                        l.preset = ix;
                        l.accent = None;
                        l.custom = None;
                        l.hue = None;
                        l.saturation = None;
                    }, cx)
                }))
                .child(div().size(px(18.)).rounded(px(5.)).border_1().border_color(content(0.16)).bg(colors.field).flex().items_center().justify_center().child(div().size(px(7.)).rounded_full().bg(colors.accent)))
                .child(preset.name)
        }));
        let shown_accent = palette(&look, true).accent.to_rgb();
        let accents = div()
            .flex()
            .gap(px(2.))
            .children(ACCENTS.iter().enumerate().map(|(ix, (_, color))| {
                let fill: gpui::Hsla = rgb(*color).into();
                let f = fill.to_rgb();
                let chosen = look.custom.is_none() && (f.r - shown_accent.r).abs() + (f.g - shown_accent.g).abs() + (f.b - shown_accent.b).abs() < 0.01;
                div()
                    .id(("accent", ix))
                    .size(px(24.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(chosen, |d| d.border_2().border_color(fill))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_look(|l| {
                            l.accent = Some(ix);
                            l.custom = None;
                        }, cx)
                    }))
                    .child(div().size(px(14.)).rounded_full().bg(fill))
            }))
            // A color of one's own: the rainbow swatch opens its two sliders.
            .child(
                div()
                    .id("accent-custom")
                    .size(px(24.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(look.custom.is_some(), |d| d.border_2().border_color(accent()))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_look(|l| {
                            if l.custom.is_none() {
                                l.custom = Some((330., 0.62));
                            }
                        }, cx)
                    }))
                    .child(div().size(px(14.)).rounded_full().bg(linear_gradient(135., linear_color_stop(rgb(0xf0556d), 0.), linear_color_stop(rgb(0x5a7bf5), 1.)))),
            );
        let saturation = match look.saturation { None => 0, Some(s) if s < 0.01 => 1, Some(s) if s < 0.2 => 2, _ => 3 };
        let hues = div().flex().gap(px(2.)).children((0..10usize).map(|ix| {
            let hue = ix as f32 * 36.;
            let chosen = look.hue == Some(hue);
            let fill = gpui::hsla(hue / 360., 0.6, 0.55, 1.);
            div()
                .id(("hue", ix))
                .size(px(22.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .when(chosen, |d| d.border_2().border_color(fill))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.set_look(|l| l.hue = if l.hue == Some(hue) { None } else { Some(hue) }, cx)))
                .child(div().size(px(12.)).rounded_full().bg(fill))
        }));
        let mut colors = vec![
            row("Tema do workspace", "Uma predefinição de cores.", themes.into_any_element()),
            row("Cor de destaque", "No botão de enviar, nos seus balões e na guia ativa.", accents.into_any_element()),
        ];
        if let Some((hue, light)) = look.custom {
            let hue_stops: Vec<gpui::Hsla> = (0..24).map(|i| gpui::hsla(i as f32 / 24., 0.75, 0.6, 1.)).collect();
            let light_stops: Vec<gpui::Hsla> = (0..12).map(|i| gpui::hsla(hue / 360., 0.75, 0.35 + i as f32 / 11. * 0.5, 1.)).collect();
            colors.push(row("Matiz da cor", "Arraste para escolher a sua cor.", self.slider(0, hue / 360., hue_stops, cx)));
            colors.push(row("Luminosidade da cor", "Mais escura ou mais clara.", self.slider(1, (light - 0.35) / 0.5, light_stops, cx)));
        }
        colors.push(row("Cores do diff", "Daltônico e alto contraste usam azul e laranja.", segmented("diff", &["Padrão", "Daltônico", "Alto contraste"], look.diff, cx, |l, ix| l.diff = ix)));
        colors.push(row("Matiz do fundo", "Clique de novo para voltar ao do tema.", hues.into_any_element()));
        colors.push(row("Saturação do fundo", "Quanto da cor aparece no fundo.", segmented("saturation", &["Do tema", "Sem", "Suave", "Forte"], saturation, cx, |l, ix| l.saturation = [None, Some(0.), Some(0.12), Some(0.35)][ix])));
        let later = |title: &'static str, about: &'static str| row(title, about, div().text_size(px(12.)).text_color(content(0.35)).child("Em breve").into_any_element());
        div()
            .child(div().text_size(px(19.)).font_weight(FontWeight::MEDIUM).text_color(content(1.)).child("Aparência"))
            .child(div().mt(px(6.)).mb(px(24.)).text_size(px(13.)).text_color(content(0.5)).child("Personalize materiais, tipografia e movimento."))
            .child(div().mb(px(32.)).child(div().mb(px(8.)).text_size(px(12.5)).text_color(content(0.5)).child("Tema")).child(modes))
            .child(group("Cores", colors))
            .child(group("Materiais", vec![row("Translucidez", "Deixa o fundo da janela aparecer através do app.", segmented("glass", &["Ligada", "Desligada"], if look.glass { 0 } else { 1 }, cx, |l, ix| l.glass = ix == 0))]))
            .child(group("Tipografia da interface", vec![
                row("Tamanho da interface", "Aumenta ou diminui tudo.", segmented("scale", &["90%", "100%", "110%", "125%"], [0.9f32, 1., 1.1, 1.25].iter().position(|s| (*s - look.scale).abs() < 0.01).unwrap_or(1), cx, |l, ix| l.scale = [0.9, 1., 1.1, 1.25][ix])),
                later("Fonte da interface", "Como no Sirus de hoje."),
            ]))
            .child(group("Tipografia de código", vec![later("Fonte e tamanho do código", "Como no Sirus de hoje.")]))
            .child(group("Tipografia do terminal", vec![later("Fonte e tamanho do terminal", "Como no Sirus de hoje.")]))
            .child(group("Efeitos e movimento", vec![later("Efeitos e movimento", "Como no Sirus de hoje.")]))
            .child(group("Fundo do chat", vec![later("Imagem de fundo", "Como no Sirus de hoje.")]))
            .child(group("Barra lateral", vec![later("Estilo da barra lateral", "Como no Sirus de hoje.")]))
            .child(group("Aplicativo", vec![later("Ícone do Dock", "Como no Sirus de hoje.")]))
            .into_any_element()
    }

    fn roll(&self, conversation: usize, key: usize, now: Instant, moving: &mut bool) -> f32 {
        let Some(tween) = self.rolls.get(&(conversation, key)) else { return 0. };
        if !tween.done(now) {
            *moving = true;
        }
        tween.at(now)
    }

    fn set_roll(&mut self, conversation: usize, key: usize, hovered: bool, cx: &mut Context<Self>) {
        let now = Instant::now();
        let duration = if cx.reduce_motion() { 0. } else { MORPH };
        let mut tween = self.rolls.get(&(conversation, key)).copied().unwrap_or_else(|| Tween::still(0., now));
        tween.set(if hovered { 1. } else { 0. }, now, duration, Curve::Expo);
        self.rolls.insert((conversation, key), tween);
        cx.notify();
    }

    fn spin(&self, now: Instant) -> Div {
        div().size(px(13.)).flex_shrink_0().flex().items_center().justify_center().child(spinner_at((now.duration_since(self.epoch).as_secs_f32() / SPINNER_PERIOD).fract()))
    }

    fn menu_view(&mut self, now: Instant, moving: &mut bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let tab = menu.tab;
        let pinned = self.tabs.iter().find(|t| t.id == tab)?.pinned;
        *moving |= !menu.open.done(now);
        let open = menu.open.at(now);
        let at = self.tabs.iter().position(|t| t.id == tab)?;
        let closable = |ix: usize, t: &Tab| t.id != tab && !t.pinned && !t.closing && ix != at;
        let has_others = self.tabs.iter().enumerate().any(|(ix, t)| closable(ix, t));
        let has_right = self.tabs.iter().enumerate().any(|(ix, t)| closable(ix, t) && ix > at);
        let has_left = self.tabs.iter().enumerate().any(|(ix, t)| closable(ix, t) && ix < at);
        let separator = || div().h(px(1.)).mx(px(4.)).my(px(4.)).bg(content(0.08));
        let item = |id: &'static str, label: &'static str, shortcut: &'static str, enabled: bool, danger: bool, cx: &mut Context<Self>, action: fn(&mut Self, u64, &mut Context<Self>)| {
            div()
                .id(id)
                .h(px(26.))
                .px(px(8.))
                .rounded_md()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(18.))
                .text_size(px(13.))
                .text_color(if danger { rgb(0xe08a8a).into() } else { content(if enabled { 0.9 } else { 0.3 }) })
                .when(enabled, |d| {
                    d.hover(|s| s.bg(content(0.1))).cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        action(this, tab, cx);
                        cx.notify();
                    }))
                })
                .child(label)
                .child(div().text_size(px(12.)).text_color(content(0.4)).child(shortcut))
        };
        Some(
            div()
                .id("tab-menu")
                .occlude()
                .absolute()
                .left(menu.at.x)
                .top(menu.at.y + px(2. + (1. - open) * -4.))
                .w(px(240.))
                .p(px(4.))
                .rounded_lg()
                .border_1()
                .border_color(content(0.12))
                .bg(floating_fill())
                .shadow_lg()
                .opacity(open.clamp(0., 1.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(item("menu-pin", if pinned { "Desafixar aba" } else { "Fixar aba" }, "⇧⌘P", true, false, cx, |this, id, cx| this.toggle_pin(id, cx)))
                .child(separator())
                .child(item("menu-close", "Fechar aba", "⌘W", true, false, cx, |this, id, cx| this.close(id, cx)))
                .child(separator())
                .child(item("menu-others", "Fechar outras abas", "", has_others, false, cx, |this, id, cx| this.close_where(id, |_, _| true, cx)))
                .child(item("menu-right", "Fechar abas à direita", "", has_right, false, cx, |this, id, cx| this.close_where(id, |ix, at| ix > at, cx)))
                .child(item("menu-left", "Fechar abas à esquerda", "", has_left, false, cx, |this, id, cx| this.close_where(id, |ix, at| ix < at, cx)))
                .child(item("menu-all", "Fechar todas as abas", "⇧⌘W", has_others, false, cx, |this, id, cx| this.close_where(id, |_, _| true, cx)))
                .child(separator())
                .child(item("menu-archive", "Arquivar", "", true, false, cx, |this, id, cx| this.close(id, cx)))
                .child(item("menu-delete", "Excluir", "", true, true, cx, |this, id, cx| this.close(id, cx)))
                .into_any_element(),
        )
    }

    fn stats(&mut self, now: Instant) -> Option<AnyElement> {
        if !self.show_stats {
            return None;
        }
        let deltas: Vec<f32> = self.frames.iter().zip(self.frames.iter().skip(1)).map(|(a, b)| b.duration_since(*a).as_secs_f32() * 1000.).filter(|d| *d < 250.).collect();
        let _ = now;
        let average = deltas.iter().sum::<f32>() / deltas.len().max(1) as f32;
        let worst = deltas.iter().copied().fold(0., f32::max);
        Some(
            div()
                .absolute()
                .right(px(14.))
                .bottom(px(14.))
                .px(px(8.))
                .py(px(4.))
                .rounded_md()
                .bg(floating_fill())
                .text_size(px(11.5))
                .text_color(content(0.75))
                .child(format!("{:.0} fps · pior quadro {:.1} ms", if average > 0. { 1000. / average } else { 0. }, worst))
                .into_any_element(),
        )
    }
}

fn running(conversation: &Conversation, now: Instant) -> bool {
    conversation.turns.last().is_some_and(|turn| {
        turn.started.is_some_and(|started| {
            let total = THINKING + turn.steps.iter().map(|s| s.seconds).sum::<f32>() + turn.answer.split_whitespace().count() as f32 / WORDS_PER_SECOND;
            now.duration_since(started).as_secs_f32() < total
        })
    })
}

/// Whether a turn's tree should be open: while it works, then shut, unless a click decided.
fn open_target(turn: &Turn, e: f32, _answer_start: f32, done_at: f32) -> bool {
    turn.toggled.map_or(turn.started.is_some() && e < done_at, |_| turn.open)
}

fn model_name(provider: &str) -> &'static str {
    match provider {
        "claude" => "Opus 5.5",
        "codex" => "GPT-5.5",
        "opencode" => "OpenCode",
        _ => "Agente",
    }
}

/// "Executou 2 comandos · Leu 3 arquivos", from the steps of a finished turn.
fn summary(steps: &[data::Step]) -> String {
    let count = |label: &str| steps.iter().filter(|s| s.label == label).count();
    let plural = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    let mut parts = Vec::new();
    for (label, verb, one, many) in [("Executando", "Executou", "comando", "comandos"), ("Lendo", "Leu", "arquivo", "arquivos"), ("Editando", "Editou", "arquivo", "arquivos"), ("Buscando", "Buscou", "vez", "vezes")] {
        let n = count(label);
        if n > 0 {
            parts.push(format!("{verb} {}", plural(n, one, many)));
        }
    }
    parts.join(" · ")
}

/// A file's type, as a small colored label.
fn file_mark(name: &str) -> AnyElement {
    let extension = name.rsplit('.').next().unwrap_or("");
    let (label, color): (&str, u32) = match extension {
        "tsx" | "ts" => ("TS", 0x5fa8f2),
        "rs" => ("RS", 0xe8956a),
        "css" => ("CS", 0x8f8bf0),
        "md" => ("MD", 0x7fa8f5),
        "json" => ("{}", 0xdcb84f),
        "gitignore" => ("GI", 0xd9673f),
        "example" => ("EN", 0xd9c04f),
        "html" => ("HT", 0xe07a5f),
        _ => ("··", 0x9a9a9a),
    };
    div().w(px(20.)).flex_shrink_0().flex().justify_center().text_size(px(10.5)).font_weight(FontWeight::BOLD).text_color(rgb(color)).child(label).into_any_element()
}

fn step_glyph(step: &data::Step) -> &'static str {
    match step.label {
        "Executando" => "terminal",
        "Editando" => "pencil-line",
        _ => "search",
    }
}

/// A top-bar button whose panel is open keeps its fill and brighter icon.
fn toggled(button: Stateful<Div>, on: bool) -> Stateful<Div> {
    button.when(on, |d| d.bg(content(0.1)))
}

fn nav_button(id: &'static str, left: f32, turn: f32, enabled: bool, cx: &mut Context<Shell>, forward: bool) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .left(px(left))
        .top(px(8.))
        .size(px(24.))
        .rounded_md()
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |d| {
            d.hover(|s| s.bg(content(0.08))).cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.navigate(forward, cx)))
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(icon("chevron-right", 15.).text_color(content(if enabled { 0.6 } else { 0.25 })).with_transformation(Transformation::rotate(percentage(turn))))
}

fn icon(name: &'static str, side: f32) -> gpui::Svg {
    theme_icon(name, side).text_color(content(0.8))
}

fn icon_button(id: &'static str, name: &'static str, side: f32) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(PLATE - 3.))
        .rounded_md()
        .flex()
        .items_center()
        .justify_center()
        .hover(|s| s.bg(content(0.08)))
        .cursor_pointer()
        .child(icon(name, side).text_color(content(0.6)))
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let reduced = cx.reduce_motion();
        let size = window.viewport_size();
        let mut moving = !self.palette_t.done(now);
        self.system_dark = matches!(window.appearance(), gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark);
        crate::theme::apply(mix(self.palette_from, palette(&self.look, self.system_dark), self.palette_t.at(now)));
        // The interface size applies after this frame, so layout never sees it change mid-draw.
        if (window.content_zoom() - self.look.scale).abs() > f32::EPSILON {
            let scale = self.look.scale;
            window.defer(cx, move |window, cx| window.set_content_zoom(scale, cx));
        }
        self.frames.push_back(now);
        while self.frames.len() > 120 {
            self.frames.pop_front();
        }
        let titlebar = self.titlebar(f32::from(size.width), now, &mut moving, reduced, cx);
        let docked = self.sidebar_open || self.sidebar.at(now) > 0.001;
        let sidebar = self.sidebar_view(now, &mut moving, reduced, cx);
        let (sidebar, peek_panel) = if docked { (Some(sidebar), None) } else { (None, self.peek.is_some().then_some(sidebar)) };
        let transcript = self.transcript(now, &mut moving, cx);
        let menu = self.menu_view(now, &mut moving, cx);
        let open = self.sidebar.at(now);
        let footer = self.footer(self.sidebar_width * open, cx);
        let panel = self.appearance_view(now, &mut moving, cx);
        let environment = self.environment_view(now, &mut moving, cx);
        // Watches the pointer over the whole window, even above opaque panels.
        let view = cx.entity();
        let height = f32::from(size.height);
        let tracker = gpui::canvas(|_, _, _| {}, move |_, _, window, _| {
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Capture {
                    view.update(cx, |this, cx| {
                        if let Some(id) = this.slider_drag {
                            if event.pressed_button == Some(MouseButton::Left) {
                                this.set_slider(id, f32::from(event.position.x), cx);
                            } else {
                                this.slider_drag = None;
                            }
                        }
                        this.track_peek(event.position, height, cx);
                    });
                }
            });
        })
        .absolute()
        .size_0();
        let palette_view = self.palette_view(now, &mut moving, cx);
        if !self.dock.done(now) || !self.dock_max.done(now) {
            moving = true;
        }
        let dock_share = self.dock.at(now);
        let room = f32::from(size.width) - self.sidebar_width * self.sidebar.at(now) - 12.;
        let dock_full = DOCK_WIDTH + (room - DOCK_WIDTH) * self.dock_max.at(now);
        let dock_width = dock_full * dock_share;
        let dock = (dock_share > 0.001).then(|| self.dock_view(dock_full, now, &mut moving, cx));
        let stats = self.stats(now);
        if moving {
            window.request_animation_frame();
        }
                div()
            .id("root")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(".SystemUIFont")
            .bg(shell_fill())
            .text_color(content(0.9))
            .on_key_down(cx.listener(Self::key))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                let closed = [this.menu.take().is_some(), this.appearance.take().is_some(), this.environment.take().is_some()];
                if closed.contains(&true) {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .bg(chrome_wash())
                    .child(titlebar)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .children(sidebar)
                            // The card runs from the title bar to the footer, with a small gutter
                            // beside it; the closed sidebar leaves the same gutter on the left.
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .pr(px(6.))
                                    .pl(px(6. * (1. - open)))
                                    .child(div().flex_1().min_w_0().h_full().flex().child(transcript))
                                    .when_some(dock, |d, dock| {
                                        // The panel slides in beside the card, its content keeping its width.
                                        d.child(div().flex_shrink_0().h_full().ml(px(6. * dock_share)).w(px(dock_width)).overflow_hidden().child(dock))
                                    }),
                            ),
                    )
                    .child(footer),
            )
            .child(tracker)
            .children(peek_panel)
            .children(menu)
            .children(environment)
            .children(panel)
            .children(palette_view)
            .children(stats)
    }
}
