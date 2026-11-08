//! Terminal side panel: a local PTY in the worktree, emulated by alacritty_terminal.
//! Ported in part from an MIT-licensed terminal (MIT).
use crate::i18n::{t, tf};
use crate::panels::PanelContext;
use crate::tab_tile::{Face, TAB_TILE, TAB_TILE_GAP, mark_tile, tab_tile_inset};
use crate::theme;
use alacritty_terminal::{
    event::{Event as TermEvent, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
    grid::{Dimensions, Scroll},
    index::{Column, Point as GridPoint, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{Config, Term, TermMode, cell::Flags, color::Colors, point_to_viewport},
    tty,
    vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Processor, Rgb, StdSyncHandler},
};
use gpui::{
    AnyElement, AnyView, App, Bounds, ClipboardItem, Context, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, FontStyle, FontWeight,
    GlobalElementId, Hsla, InspectorElementId, KeyBinding, KeyDownEvent, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollDelta, ScrollWheelEvent, ShapedLine,
    SharedString, Size, StrikethroughStyle, Style, StyleRefinement, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, actions, div, point, prelude::*, px, rgb, size,
};
use std::{borrow::Cow, cell::Cell, collections::HashMap, ops::Range, rc::Rc, sync::Arc};

actions!(
    sirus_terminal,
    [SendTab, SendBackTab, SendEscape, Copy, Paste, SelectAll]
);

const FONT_SIZE: f32 = 12.5;
/// The scrollback when Settings → Terminal → Limite do histórico has not loaded.
const SCROLLBACK: u64 = 5000;
const PAD_X: f32 = 8.;
const PAD_Y: f32 = 4.;

pub fn new(context: PanelContext, cx: &mut App) -> AnyView {
    bind_keys(cx);
    let name = match context.provider {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "cursor" => "Cursor",
        "opencode" => "OpenCode",
        "grok" => "Grok",
        "antigravity" => "Antigravity",
        "droid" => "Droid",
        "pi" => "Pi",
        "devin" => "Devin",
        _ => "Hermes",
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let folder = context
        .worktree
        .strip_prefix(&home)
        .map_or(context.worktree.clone(), |rest| format!("~{rest}"));
    let banner = crate::mascots::banner(
        context.provider,
        name,
        &context.version,
        &context.model,
        &folder,
    );
    cx.new(|cx| TerminalPanel::new(context.worktree, banner, cx))
        .into()
}

/// The panel's view: a tab strip over the terminals it holds. Every terminal
/// keeps running; only the active one is drawn.
/// A terminal tab being dragged out of the panel.
pub struct TermTabDrag {
    pub panel: gpui::WeakEntity<TerminalPanel>,
    pub ix: usize,
    pub title: String,
}

pub struct TerminalPanel {
    cwd: String,
    terminals: Vec<Entity<Terminal>>,
    active: usize,
    /// The terminal each project action last ran in, reused while it sits idle.
    action_terminals: HashMap<String, gpui::WeakEntity<Terminal>>,
}

fn folder_name(cwd: &str) -> String {
    std::path::Path::new(cwd).file_name().map_or_else(
        || cwd.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl TerminalPanel {
    fn new(cwd: String, banner: String, cx: &mut Context<Self>) -> Self {
        let first = cx.new(|cx| Terminal::new(cwd.clone(), banner, cx));
        Self {
            cwd,
            terminals: vec![first],
            active: 0,
            action_terminals: HashMap::new(),
        }
    }
    /// Gives up a terminal, shell and all, to live elsewhere. The panel is never left empty.
    fn take_terminal(
        &mut self,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<(String, Entity<Terminal>)> {
        if ix >= self.terminals.len() {
            return None;
        }
        let terminal = self.terminals.remove(ix);
        if self.terminals.is_empty() {
            let fresh = self.spawn(cx);
            self.terminals.push(fresh);
        }
        self.active = self.active.min(self.terminals.len() - 1);
        cx.notify();
        Some((self.cwd.clone(), terminal))
    }
    /// A shell in the panel's folder. Only the first terminal prints the banner.
    fn spawn(&self, cx: &mut Context<Self>) -> Entity<Terminal> {
        let cwd = self.cwd.clone();
        cx.new(|cx| Terminal::new(cwd, String::new(), cx))
    }
    fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.terminals.len() {
            self.active = ix;
            self.focus_active(window, cx);
            cx.notify();
        }
    }
    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let terminal = self.spawn(cx);
        self.terminals.push(terminal);
        self.select(self.terminals.len() - 1, window, cx);
    }
    fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.close_many(vec![ix], window, cx);
    }
    /// Closes the terminals at `ixs`. A shell with a command running asks first, in the
    /// shell's popup, naming every running one together; idle shells close at once.
    fn close_many(&mut self, ixs: Vec<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let targets: Vec<Entity<Terminal>> = ixs
            .iter()
            .filter_map(|ix| self.terminals.get(*ix).cloned())
            .collect();
        let label = folder_name(&self.cwd);
        let marked: Vec<(String, bool)> = targets
            .iter()
            .map(|t| {
                let at = self.terminals.iter().position(|o| o == t).unwrap_or(0);
                (format!("{label} {}", at + 1), t.read(cx).busy())
            })
            .collect();
        let running = running_names(&marked);
        if !needs_confirm(window, cx, &running) {
            self.close_now(&targets, window, cx);
            return;
        }
        let panel = cx.entity();
        let handle = window.window_handle();
        confirm_close(window, cx, &running, move |cx: &mut App| {
            let _ = cx.update_window(handle, |_, window, cx| {
                panel.update(cx, |this, cx| this.close_now(&targets, window, cx));
            });
        });
    }
    fn close_now(
        &mut self,
        targets: &[Entity<Terminal>],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.terminals.get(self.active).cloned();
        // Dropping the last handle to a terminal shuts its shell down.
        self.terminals.retain(|t| !targets.contains(t));
        if self.terminals.is_empty() {
            let terminal = self.spawn(cx);
            self.terminals.push(terminal);
            self.active = 0;
        } else {
            self.active = active
                .and_then(|a| self.terminals.iter().position(|t| *t == a))
                .unwrap_or(self.active)
                .min(self.terminals.len() - 1);
        }
        self.focus_active(window, cx);
        cx.notify();
    }
    fn focus_active(&self, window: &mut Window, cx: &mut App) {
        let focus = self.terminals[self.active].read(cx).focus.clone();
        window.focus(&focus, cx);
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label: SharedString = folder_name(&self.cwd).into();
        let tabs: Vec<AnyElement> = (0..self.terminals.len())
            .map(|ix| {
                let active = ix == self.active;
                let group = format!("terminal-tab-{ix}");
                let close = div()
                    .id(("terminal-close", ix))
                    .absolute()
                    .right_1()
                    .top(px(3.5))
                    .size(px(20.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(if active { 1. } else { 0. })
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .hover(|s| s.bg(theme::content(0.1)))
                    .child(theme::icon("close", 12.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close(ix, window, cx);
                    }));
                let face = Face::Fill(rgb(0x3A3A3C).into(), rgb(0x1C1C1E).into());
                let plate = div()
                    .id(("terminal-tab-button", ix))
                    .relative()
                    .h(px(theme::PLATE))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(TAB_TILE_GAP))
                    .pl(px(tab_tile_inset()))
                    .pr(px(28.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(theme::content(0.1)).text_color(theme::content(1.))
                    })
                    .when(!active, |d| {
                        d.bg(theme::content(0.04)).text_color(theme::content(0.5))
                    })
                    .child(mark_tile(
                        &TAB_TILE,
                        face,
                        gpui::svg()
                            .path("terminal-square")
                            .size(px(12.))
                            .text_color(gpui::white()),
                    ))
                    .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                    .child(close)
                    .on_click(cx.listener(move |this, _, window, cx| this.select(ix, window, cx)));
                div()
                    .id(("terminal-tab", ix))
                    .on_drag(
                        TermTabDrag {
                            panel: cx.entity().downgrade(),
                            ix,
                            title: label.to_string(),
                        },
                        |drag, _, _, cx| {
                            cx.new(|_| crate::shell::split_panes::DragChip::new(drag.title.clone()))
                        },
                    )
                    .group(group)
                    .h_full()
                    .w(px(224.))
                    .min_w_0()
                    .flex_shrink(1.)
                    .flex()
                    .items_center()
                    .child(plate)
                    .into_any_element()
            })
            .collect();
        let add = div()
            .id("new-terminal")
            .group("new-terminal")
            .cursor_pointer()
            .size(px(theme::PLATE))
            .flex_shrink_0()
            .ml(px(2.))
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(theme::content(0.04))
            .opacity(0.65)
            .flex()
            .items_center()
            .justify_center()
            .hover(|s| s.opacity(1.).bg(theme::content(0.07)))
            .child(
                theme::icon("plus", 14.)
                    .group_hover("new-terminal", |s| s.text_color(theme::content(1.))),
            )
            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx)));
        div()
            .id("sirus-terminal-panel")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(36.))
                    .flex_shrink_0()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme::content(0.08))
                    .child(
                        div()
                            .id("terminal-tabs")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .pl(px(6.))
                            .pr(px(10.))
                            .overflow_hidden()
                            .children(tabs)
                            .child(add),
                    ),
            )
            .child(
                div().flex_1().min_h_0().min_w_0().children(
                    self.terminals
                        .get(self.active)
                        // The grid shapes every line; redraw it only when the
                        // terminal itself changes.
                        .map(|t| t.clone().cached(StyleRefinement::default().size_full())),
                ),
            )
    }
}

fn bind_keys(cx: &mut App) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static BOUND: AtomicBool = AtomicBool::new(false);
    if BOUND.swap(true, Ordering::SeqCst) {
        return;
    }
    let c = Some("SirusTerminal");
    cx.bind_keys([
        KeyBinding::new("tab", SendTab, c),
        KeyBinding::new("shift-tab", SendBackTab, c),
        KeyBinding::new("escape", SendEscape, c),
        KeyBinding::new("cmd-c", Copy, c),
        KeyBinding::new("cmd-v", Paste, c),
        KeyBinding::new("cmd-a", SelectAll, c),
    ]);
}

#[derive(Clone)]
struct Listener(async_channel::Sender<TermEvent>);
impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.try_send(event);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct GridSize {
    columns: usize,
    lines: usize,
}
impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

#[derive(Clone, Copy)]
struct Geometry {
    origin: gpui::Point<gpui::Pixels>,
    cell: Size<gpui::Pixels>,
    cursor: Bounds<gpui::Pixels>,
}

struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: Option<EventLoopSender>,
    focus: FocusHandle,
    size: GridSize,
    geometry: Rc<Cell<Option<Geometry>>>,
    shell_pid: u32,
    /// A duplicate of the PTY master, kept to ask which process group is in front.
    master_fd: i32,
    exited: bool,
    selecting: bool,
    /// The pointer rests on a web address, which a click opens.
    over_link: bool,
    scroll_remainder: f32,
    marked: Option<String>,
    _events: gpui::Task<()>,
}
impl Focusable for Terminal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn write_note(term: &Arc<FairMutex<Term<Listener>>>, text: &str) {
    let mut parser = Processor::<StdSyncHandler>::new();
    parser.advance(&mut *term.lock(), text.as_bytes());
}

fn working_dir(cwd: &str) -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let path = match cwd.strip_prefix("~") {
        Some(rest) => home
            .clone()
            .map_or_else(|| cwd.into(), |h| h.join(rest.trim_start_matches('/'))),
        None => std::path::PathBuf::from(cwd),
    };
    if path.is_dir() {
        path
    } else {
        home.unwrap_or(path)
    }
}

impl Terminal {
    fn new(cwd: String, banner: String, cx: &mut Context<Self>) -> Self {
        let size = GridSize {
            columns: 80,
            lines: 24,
        };
        let (tx, rx) = async_channel::unbounded();
        let listener = Listener(tx);
        let config = Config {
            scrolling_history: crate::prefs::number("terminalScrollback", SCROLLBACK)
                .clamp(1000, 100_000) as usize,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let workdir = working_dir(&cwd);
        let program = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/zsh".into());
        let mut env = HashMap::new();
        for (key, value) in [
            ("TERM", "xterm-256color".to_owned()),
            ("COLORTERM", "truecolor".to_owned()),
            ("TERM_PROGRAM", "SirusNative".to_owned()),
            ("PWD", workdir.to_string_lossy().into_owned()),
        ] {
            env.insert(key.to_owned(), value);
        }
        let options = tty::Options {
            shell: Some(tty::Shell::new(program.clone(), vec!["-l".into()])),
            working_directory: Some(workdir),
            drain_on_exit: false,
            env,
        };
        let window = WindowSize {
            num_lines: size.lines as u16,
            num_cols: size.columns as u16,
            cell_width: 8,
            cell_height: 16,
        };
        write_note(&term, &banner);
        let mut shell_pid = 0;
        let mut master_fd = -1;
        let sender = match tty::new(&options, window, cx.entity_id().as_u64()) {
            Ok(pty) => {
                shell_pid = pty.child().id();
                {
                    use std::os::fd::AsRawFd;
                    master_fd = unsafe { libc::dup(pty.file().as_raw_fd()) };
                }
                match EventLoop::new(term.clone(), listener, pty, false, false) {
                    Ok(event_loop) => {
                        let sender = event_loop.channel();
                        event_loop.spawn();
                        Some(sender)
                    }
                    Err(error) => {
                        write_note(&term, &format!("\x1b[31m{error}\x1b[0m"));
                        None
                    }
                }
            }
            Err(error) => {
                write_note(
                    &term,
                    &format!("\x1b[31mCould not start the shell ({program}): {error}\x1b[0m"),
                );
                None
            }
        };
        let events = cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let mut batch = vec![event];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                if this
                    .update(cx, |this, cx| {
                        for event in batch {
                            this.on_event(event, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            exited: sender.is_none(),
            term,
            sender,
            focus: cx.focus_handle(),
            size,
            geometry: Rc::default(),
            shell_pid,
            master_fd,
            selecting: false,
            over_link: false,
            scroll_remainder: 0.,
            marked: None,
            _events: events,
        }
    }

    fn on_event(&mut self, event: TermEvent, cx: &mut Context<Self>) {
        match event {
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {
                cx.notify()
            }
            TermEvent::PtyWrite(text) => self.write(text.into_bytes()),
            TermEvent::ColorRequest(index, format) => {
                let colors = *self.term.lock().colors();
                self.write(format(color_rgb(index, &colors)).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let cell = self
                    .geometry
                    .get()
                    .map_or(size(px(8.), px(16.)), |g| g.cell);
                let reply = format(WindowSize {
                    num_lines: self.size.lines as u16,
                    num_cols: self.size.columns as u16,
                    cell_width: cell.width.as_f32() as u16,
                    cell_height: cell.height.as_f32() as u16,
                });
                self.write(reply.into_bytes());
            }
            TermEvent::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            TermEvent::ClipboardLoad(_, format) => {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                self.write(format(&text).into_bytes());
            }
            TermEvent::ChildExit(status) => {
                let code = status.code().map(|c| format!(" ({c})")).unwrap_or_default();
                write_note(&self.term, &format!("\r\n[process exited{code}]\r\n"));
                self.finish(cx);
            }
            TermEvent::Exit => self.finish(cx),
            _ => {}
        }
    }
    fn finish(&mut self, cx: &mut Context<Self>) {
        self.exited = true;
        self.sender = None;
        cx.notify();
    }
    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Input(bytes.into()));
        }
    }
    /// Types a command at the prompt and presses return.
    fn run(&mut self, command: &str, cx: &mut Context<Self>) {
        self.input(format!("{command}\n").into_bytes(), cx);
    }
    /// Typing returns a scrolled-back view to the prompt.
    fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        self.selecting = false;
        {
            let mut term = self.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.write(bytes);
        cx.notify();
    }
    fn resize(&mut self, columns: usize, lines: usize, cell: Size<gpui::Pixels>) {
        let next = GridSize { columns, lines };
        if next == self.size {
            return;
        }
        self.size = next;
        self.term.lock().resize(next);
        if let Some(sender) = &self.sender {
            let _ = sender.send(Msg::Resize(WindowSize {
                num_lines: lines as u16,
                num_cols: columns as u16,
                cell_width: cell.width.as_f32() as u16,
                cell_height: cell.height.as_f32() as u16,
            }));
        }
    }
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let mode = *self.term.lock().mode();
        if let Some(bytes) = key_bytes(&event.keystroke, mode) {
            cx.stop_propagation();
            self.input(bytes, cx);
        }
    }
    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self
            .term
            .lock()
            .selection_to_string()
            .filter(|t| !t.is_empty())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let bytes = if bracketed {
            let text = text.replace("\x1b[201~", "");
            format!("\x1b[200~{text}\x1b[201~").into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        self.input(bytes, cx);
    }
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let mut term = self.term.lock();
        let mut selection = Selection::new(
            SelectionType::Simple,
            GridPoint::new(term.topmost_line(), Column(0)),
            Side::Left,
        );
        selection.update(
            GridPoint::new(term.bottommost_line(), term.last_column()),
            Side::Right,
        );
        term.selection = Some(selection);
        drop(term);
        cx.notify();
    }
    fn grid_point(&self, position: gpui::Point<gpui::Pixels>) -> Option<(GridPoint, Side)> {
        let g = self.geometry.get()?;
        let x = ((position.x - g.origin.x) / g.cell.width).max(0.);
        let y = ((position.y - g.origin.y) / g.cell.height).max(0.);
        let column = (x as usize).min(self.size.columns.saturating_sub(1));
        let line = (y as usize).min(self.size.lines.saturating_sub(1));
        let side = if x.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        let term = self.term.lock();
        let at = alacritty_terminal::term::viewport_to_point(
            term.grid().display_offset(),
            GridPoint::new(line, Column(column)),
        );
        Some((at, side))
    }
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let Some((at, side)) = self.grid_point(event.position) else {
            return;
        };
        let mut term = self.term.lock();
        match term.selection.as_mut() {
            Some(selection) if event.modifiers.shift => selection.update(at, side),
            _ => {
                let ty = match event.click_count {
                    2 => SelectionType::Semantic,
                    3.. => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                term.selection = Some(Selection::new(ty, at, side));
            }
        }
        drop(term);
        self.selecting = true;
        cx.notify();
    }
    /// The web address under `position`, across a line the terminal wrapped.
    fn link_at(&self, position: gpui::Point<gpui::Pixels>) -> Option<String> {
        let (at, _) = self.grid_point(position)?;
        let term = self.term.lock();
        let grid = term.grid();
        let columns = grid.columns();
        let wrapped = |line: alacritty_terminal::index::Line| {
            grid[line][Column(columns - 1)]
                .flags
                .contains(alacritty_terminal::term::cell::Flags::WRAPLINE)
        };
        let (top, bottom) = (grid.topmost_line(), grid.bottommost_line());
        let mut first = at.line;
        while first > top && wrapped(first - 1) {
            first = first - 1;
        }
        let mut last = at.line;
        while last < bottom && wrapped(last) {
            last = last + 1;
        }
        let mut text = String::new();
        let mut under = None;
        let mut line = first;
        loop {
            for column in 0..columns {
                if line == at.line && column == at.column.0 {
                    under = Some(text.len());
                }
                text.push(grid[line][Column(column)].c);
            }
            if line == last {
                break;
            }
            line = line + 1;
        }
        url_at(&text, under?).map(str::to_owned)
    }
    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.selecting {
            let over = self.link_at(event.position).is_some();
            if over != self.over_link {
                self.over_link = over;
                cx.notify();
            }
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.selecting = false;
            return;
        }
        if let Some((at, side)) = self.grid_point(event.position) {
            if let Some(selection) = self.term.lock().selection.as_mut() {
                selection.update(at, side);
            }
            cx.notify();
        }
    }
    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.selecting {
            self.selecting = false;
            // A click without a drag leaves no selection behind.
            let mut term = self.term.lock();
            let clicked = term.selection.as_ref().is_some_and(|s| s.is_empty());
            if clicked {
                term.selection = None;
            }
            drop(term);
            // That same click on a web address opens it; a drag across one still selects it.
            if clicked
                && event.click_count == 1
                && let Some(url) = self.link_at(event.position)
            {
                let held = event.modifiers;
                let system = held.platform || held.control || held.alt;
                crate::shell::open_link_from(&url, system, cx);
            }
            cx.notify();
        }
    }
    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let line = self.geometry.get().map_or(16., |g| g.cell.height.as_f32());
        let delta = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => pixels.y.as_f32() / line,
        } + self.scroll_remainder;
        let lines = delta.trunc() as i32;
        self.scroll_remainder = delta.fract();
        if lines == 0 {
            return;
        }
        let mode = *self.term.lock().mode();
        // A full-screen program scrolls itself; the shell scrolls history.
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let key: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.write(key.repeat(lines.unsigned_abs() as usize));
        } else {
            self.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }
}

/// Whether closing needs the popup: something is running and Settings still asks.
pub fn needs_confirm(window: &mut Window, cx: &App, running: &[String]) -> bool {
    if running.is_empty() {
        return false;
    }
    match window.root::<crate::shell::Shell>() {
        Some(Some(shell)) => shell.read(cx).confirms_terminal_close(),
        _ => true,
    }
}

/// Asks in the shell's popup, naming every running terminal; `close` runs only on yes.
pub fn confirm_close(
    window: &mut Window,
    cx: &mut App,
    running: &[String],
    close: impl FnOnce(&mut App) + 'static,
) {
    let (title, message) = if running.len() == 1 {
        (
            t("Fechar o terminal?").to_owned(),
            tf(
                "Há um comando rodando em \"{}\". Fechar encerra o shell e o que estiver rodando nele.",
                &[&running[0]],
            ),
        )
    } else {
        (
            t("Fechar os terminais?").to_owned(),
            tf(
                "Há comandos rodando em: {}. Fechar encerra esses shells e o que estiver rodando neles.",
                &[&running.join(", ")],
            ),
        )
    };
    if let Some(Some(shell)) = window.root::<crate::shell::Shell>() {
        shell.update(cx, |shell, cx| {
            shell.confirm(title, message, t("Fechar"), close, cx)
        });
    }
}

/// One more terminal in the Terminal pane (the side panel or the bottom strip).
pub fn add_in(view: &AnyView, window: &mut Window, cx: &mut App) -> bool {
    match view.clone().downcast::<TerminalPanel>() {
        Ok(panel) => {
            panel.update(cx, |panel, cx| panel.add(window, cx));
            true
        }
        Err(_) => false,
    }
}

/// Gives the keyboard to the Terminal pane's active shell.
pub fn focus_in(view: &AnyView, window: &mut Window, cx: &mut App) {
    if let Ok(panel) = view.clone().downcast::<TerminalPanel>() {
        panel.update(cx, |panel, cx| panel.focus_active(window, cx));
    }
}

/// Runs a project action in its own terminal of the pane, opening one if it has none idle.
pub fn run_in(
    view: &AnyView,
    key: &str,
    command: &str,
    reveal: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Ok(panel) = view.clone().downcast::<TerminalPanel>() else {
        return;
    };
    panel.update(cx, |panel, cx| {
        let idle = panel
            .action_terminals
            .get(key)
            .and_then(|weak| weak.upgrade())
            .filter(|terminal| {
                panel.terminals.contains(terminal) && {
                    let terminal = terminal.read(cx);
                    !terminal.busy() && !terminal.exited
                }
            });
        let terminal = idle.unwrap_or_else(|| {
            let fresh = panel.spawn(cx);
            panel.terminals.push(fresh.clone());
            panel
                .action_terminals
                .insert(key.to_owned(), fresh.downgrade());
            fresh
        });
        terminal.update(cx, |terminal, cx| terminal.run(command, cx));
        if reveal {
            if let Some(at) = panel.terminals.iter().position(|t| *t == terminal) {
                panel.active = at;
            }
            panel.focus_active(window, cx);
        }
        cx.notify();
    });
}

/// A title-bar tab: one shell, or several side by side after Split Terminal Right.
pub struct TerminalGroup {
    cwd: String,
    panes: Vec<Entity<Terminal>>,
}

/// Takes the dragged terminal out of its panel, into a group of its own.
pub fn group_from_drag(drag: &TermTabDrag, cx: &mut App) -> Option<Entity<TerminalGroup>> {
    let panel = drag.panel.upgrade()?;
    let (cwd, terminal) = panel.update(cx, |panel, cx| panel.take_terminal(drag.ix, cx))?;
    Some(cx.new(|_| TerminalGroup {
        cwd,
        panes: vec![terminal],
    }))
}

/// Puts a title tab's shells back in the Terminal pane, the last one active.
pub fn adopt_group(
    view: &AnyView,
    group: &Entity<TerminalGroup>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Ok(panel) = view.clone().downcast::<TerminalPanel>() else {
        return false;
    };
    let panes = group.update(cx, |group, _| std::mem::take(&mut group.panes));
    panel.update(cx, |panel, cx| {
        panel.terminals.extend(panes);
        panel.active = panel.terminals.len() - 1;
        panel.focus_active(window, cx);
        cx.notify();
    });
    true
}

impl TerminalGroup {
    pub fn new(cwd: String, cx: &mut Context<Self>) -> Self {
        let first = cx.new(|cx| Terminal::new(cwd.clone(), String::new(), cx));
        Self {
            cwd,
            panes: vec![first],
        }
    }
    /// A shell beside the last one, in the same folder, with the keyboard focus.
    pub fn split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.cwd.clone();
        let pane = cx.new(|cx| Terminal::new(cwd, String::new(), cx));
        self.panes.push(pane);
        self.focus(window, cx);
        cx.notify();
    }
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(pane) = self.panes.last() {
            let focus = pane.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
    }
    /// Whether its last shell is free to take a command.
    pub fn idle(&self, cx: &App) -> bool {
        self.panes.last().is_some_and(|pane| {
            let pane = pane.read(cx);
            !pane.busy() && !pane.exited
        })
    }
    pub fn run(&self, command: &str, cx: &mut App) {
        if let Some(pane) = self.panes.last() {
            pane.update(cx, |pane, cx| pane.run(command, cx));
        }
    }
    /// The folder's name, the label of the tab.
    pub fn title(&self) -> String {
        folder_name(&self.cwd)
    }
    /// Every pane with a name and whether a command runs in it.
    pub fn panes_state(&self, cx: &App) -> Vec<(String, bool)> {
        let label = folder_name(&self.cwd);
        self.panes
            .iter()
            .enumerate()
            .map(|(ix, pane)| (format!("{label} {}", ix + 1), pane.read(cx).busy()))
            .collect()
    }
}

impl Render for TerminalGroup {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let last = self.panes.len().saturating_sub(1);
        div()
            .id("sirus-terminal-group")
            .size_full()
            .flex()
            .children(self.panes.iter().enumerate().map(|(ix, pane)| {
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .when(ix < last, |d| {
                        d.border_r_1().border_color(theme::content(0.08))
                    })
                    .child(pane.clone().cached(StyleRefinement::default().size_full()))
            }))
    }
}

impl Terminal {
    /// Whether a command other than the shell itself holds the terminal.
    fn busy(&self) -> bool {
        if self.exited || self.master_fd < 0 {
            return false;
        }
        let front = unsafe { libc::tcgetpgrp(self.master_fd) };
        foreground_busy(front, self.shell_pid as i32)
    }
}

/// A shell is a process group leader, so while its group is in front the terminal is idle; any
/// other group in front is a running command. An unreadable group (<= 0) counts as idle.
pub fn foreground_busy(front: i32, shell: i32) -> bool {
    front > 0 && shell > 0 && front != shell
}

/// The names of the terminals that have a command running, for the confirmation's message.
pub fn running_names(terminals: &[(String, bool)]) -> Vec<String> {
    terminals
        .iter()
        .filter(|(_, busy)| *busy)
        .map(|(name, _)| name.clone())
        .collect()
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if self.master_fd >= 0 {
            unsafe { libc::close(self.master_fd) };
        }
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(Msg::Shutdown);
        }
        if self.shell_pid > 1 && !self.exited {
            let pid = self.shell_pid as i32;
            unsafe {
                libc::kill(pid, libc::SIGHUP);
                libc::kill(-pid, libc::SIGHUP);
            }
        }
    }
}

impl EntityInputHandler for Terminal {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        self.input(text.as_bytes().to_vec(), cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| new_text.to_owned());
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        element_bounds: Bounds<gpui::Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<gpui::Pixels>> {
        Some(self.geometry.get().map_or(element_bounds, |g| g.cursor))
    }
    fn character_index_for_point(
        &mut self,
        _: gpui::Point<gpui::Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for Terminal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        div()
            .id("sirus-terminal")
            .key_context("SirusTerminal")
            .track_focus(&self.focus)
            .size_full()
            .cursor(if self.over_link {
                gpui::CursorStyle::PointingHand
            } else {
                gpui::CursorStyle::IBeam
            })
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .on_action(cx.listener(|this, _: &SendTab, _, cx| this.input(b"\t".to_vec(), cx)))
            .on_action(
                cx.listener(|this, _: &SendBackTab, _, cx| this.input(b"\x1b[Z".to_vec(), cx)),
            )
            .on_action(cx.listener(|this, _: &SendEscape, _, cx| this.input(b"\x1b".to_vec(), cx)))
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.mouse_down(event, window, cx)
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.scroll(event, cx)))
            .child(Grid {
                terminal: cx.entity(),
                focused,
            })
    }
}

/// The http(s) address around byte `at` of `text`: it ends at a space, a quote or a bracket that
/// the address didn't open, and sentence punctuation after it is left out.
fn url_at(text: &str, at: usize) -> Option<&str> {
    for scheme in ["https://", "http://"] {
        let mut from = 0;
        while let Some(found) = text[from..].find(scheme) {
            let start = from + found;
            let rest = &text[start..];
            let mut end = rest
                .find(|c: char| {
                    c.is_whitespace()
                        || c.is_control()
                        || matches!(c, '"' | '\'' | '<' | '>' | '`' | '│')
                })
                .unwrap_or(rest.len());
            let mut url = &rest[..end];
            loop {
                let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?']);
                let closes = |open: char, close: char| {
                    trimmed.ends_with(close)
                        && trimmed.matches(close).count() > trimmed.matches(open).count()
                };
                let cut = if closes('(', ')') || closes('[', ']') || closes('{', '}') {
                    &trimmed[..trimmed.len() - 1]
                } else {
                    trimmed
                };
                if cut.len() == url.len() {
                    break;
                }
                url = cut;
            }
            end = url.len();
            if url.len() > scheme.len() && (start..start + end).contains(&at) {
                return Some(url);
            }
            from = start + scheme.len();
        }
    }
    None
}

struct Painted {
    lines: Vec<(gpui::Point<gpui::Pixels>, ShapedLine)>,
    rects: Vec<(Bounds<gpui::Pixels>, Hsla)>,
    cursor: Option<(Bounds<gpui::Pixels>, Hsla, bool)>,
    line_height: gpui::Pixels,
}

struct Grid {
    terminal: Entity<Terminal>,
    focused: bool,
}
impl IntoElement for Grid {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[derive(Clone, PartialEq)]
struct Pen {
    fg: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}
/// A cell to paint: column, glyph, pen, fill and whether it is wide.
type Painting = (usize, char, Pen, Option<Hsla>, bool);

impl Element for Grid {
    type RequestLayoutState = ();
    type PrepaintState = Painted;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = gpui::relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<gpui::Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Painted {
        let font_size = px(FONT_SIZE);
        let base = crate::fonts::terminal(window.text_system());
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&base);
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map_or(px(7.5), |size| size.width);
        let line_height = (text_system.ascent(font_id, font_size)
            + text_system.descent(font_id, font_size).abs())
        .ceil()
        .max(px(14.));
        let origin = point(bounds.left() + px(PAD_X), bounds.top() + px(PAD_Y));
        let columns = (((bounds.size.width - px(PAD_X * 2.)) / cell_width).floor() as usize).max(2);
        let lines = (((bounds.size.height - px(PAD_Y * 2.)) / line_height).floor() as usize).max(1);
        let cell = size(cell_width, line_height);
        let term = self.terminal.update(cx, |t, _| {
            t.resize(columns, lines, cell);
            t.term.clone()
        });

        let term = term.lock();
        let content = term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset;
        let selection = content.selection;
        let mut rows: Vec<Vec<Painting>> = vec![Vec::new(); lines];
        for indexed in content.display_iter {
            let c = indexed.cell;
            if c.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let Some(at) = point_to_viewport(offset, indexed.point) else {
                continue;
            };
            if at.line >= lines {
                continue;
            }
            let mut fg = color(c.fg, colors);
            let mut bg = match c.bg {
                AnsiColor::Named(NamedColor::Background) => None,
                other => Some(color(other, colors)),
            };
            if c.flags.contains(Flags::INVERSE) {
                let back = bg.unwrap_or_else(theme::pane_opaque);
                bg = Some(fg);
                fg = back;
            }
            if c.flags.contains(Flags::DIM) {
                fg.a *= 0.66;
            }
            if selection.is_some_and(|s| s.contains(indexed.point)) {
                bg = Some(theme::text_selection());
            }
            let ch = if c.flags.contains(Flags::HIDDEN) {
                ' '
            } else {
                c.c
            };
            rows[at.line].push((
                at.column.0,
                ch,
                Pen {
                    fg,
                    bold: c.flags.contains(Flags::BOLD),
                    italic: c.flags.contains(Flags::ITALIC),
                    underline: c.flags.intersects(Flags::ALL_UNDERLINES),
                    strike: c.flags.contains(Flags::STRIKEOUT),
                },
                bg,
                c.flags.contains(Flags::WIDE_CHAR),
            ));
        }
        let cursor_point = content.cursor.point;
        let cursor_shape = content.cursor.shape;
        let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR);
        drop(term);

        let scale = window.scale_factor();
        let mut painted = Painted {
            lines: Vec::new(),
            rects: Vec::new(),
            cursor: None,
            line_height,
        };
        for (line, cells) in rows.iter().enumerate() {
            let y = origin.y + line_height * line as f32;
            let mut span: Option<(usize, usize, Hsla)> = None;
            for (column, _, _, bg, wide) in cells {
                let width = if *wide { 2 } else { 1 };
                match (span, bg) {
                    (Some((start, end, fill)), Some(bg)) if end == *column && fill == *bg => {
                        span = Some((start, end + width, fill));
                    }
                    (_, bg) => {
                        if let Some((start, end, fill)) = span.take() {
                            painted
                                .rects
                                .push((cell_rect(origin, cell, line, start, end), fill));
                        }
                        span = bg.map(|bg| (*column, column + width, bg));
                    }
                }
            }
            if let Some((start, end, fill)) = span {
                painted
                    .rects
                    .push((cell_rect(origin, cell, line, start, end), fill));
            }
            // Block elements are drawn as rects on the cell grid, not as glyphs,
            // so neighbouring cells and lines meet without seams.
            for (column, ch, pen, _, _) in cells {
                if let Some((shapes, alpha)) = block_shape(*ch) {
                    for [x0, y0, x1, y1] in shapes {
                        let x = |f: u8| {
                            snap(
                                origin.x + cell_width * (*column as f32 + f as f32 / 8.),
                                scale,
                            )
                        };
                        let y = |f: u8| {
                            snap(
                                origin.y + line_height * (line as f32 + f as f32 / 8.),
                                scale,
                            )
                        };
                        let (left, right, top, bottom) = (x(x0), x(x1), y(y0), y(y1));
                        let mut fill = pen.fg;
                        fill.a *= alpha;
                        painted.rects.push((
                            Bounds::new(point(left, top), size(right - left, bottom - top)),
                            fill,
                        ));
                    }
                }
            }
            let mut ix = 0;
            while ix < cells.len() {
                let (column, ch, pen, _, wide) = &cells[ix];
                if is_block(*ch) {
                    ix += 1;
                    continue;
                }
                let mut text = String::new();
                let mut end = ix;
                while end < cells.len()
                    && cells[end].2 == *pen
                    && !cells[end].4
                    && !is_block(cells[end].1)
                    && (end == ix || !wide)
                    && cells[end].0 == column + (end - ix)
                {
                    text.push(cells[end].1);
                    end += 1;
                }
                if end == ix {
                    text.push(cells[ix].1);
                    end = ix + 1;
                }
                if !text.trim().is_empty() {
                    let mut font = base.clone();
                    if pen.bold {
                        font.weight = FontWeight::BOLD;
                    }
                    if pen.italic {
                        font.style = FontStyle::Italic;
                    }
                    let run = TextRun {
                        len: text.len(),
                        font,
                        color: pen.fg,
                        background_color: None,
                        underline: pen.underline.then_some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(pen.fg),
                            wavy: false,
                        }),
                        strikethrough: pen.strike.then_some(StrikethroughStyle {
                            thickness: px(1.),
                            color: Some(pen.fg),
                        }),
                    };
                    let width = if *wide { cell_width * 2. } else { cell_width };
                    let shaped = text_system.shape_line(
                        SharedString::from(text),
                        font_size,
                        &[run],
                        Some(width),
                    );
                    painted
                        .lines
                        .push((point(origin.x + cell_width * *column as f32, y), shaped));
                }
                ix = end;
            }
        }
        let mut cursor_bounds = Bounds::new(origin, cell);
        if let Some(at) = point_to_viewport(offset, cursor_point)
            && at.line < lines
        {
            let x = origin.x + cell_width * at.column.0 as f32;
            let y = origin.y + line_height * at.line as f32;
            cursor_bounds = Bounds::new(point(x, y), cell);
            if show_cursor && cursor_shape != CursorShape::Hidden {
                // Settings → Terminal → Estilo do cursor while the terminal has focus (the block
                // is see-through, so the letter under it still reads); a hollow box otherwise.
                let mut color = theme::accent();
                let (rect, hollow) = if !self.focused {
                    (cursor_bounds, true)
                } else {
                    match crate::prefs::choice("terminalCursorStyle", "bar").as_str() {
                        "block" => {
                            color.a = 0.45;
                            (cursor_bounds, false)
                        }
                        "underline" => (
                            Bounds::new(
                                point(x, y + line_height - px(2.)),
                                size(cell_width, px(2.)),
                            ),
                            false,
                        ),
                        _ => (Bounds::new(point(x, y), size(px(2.), line_height)), false),
                    }
                };
                painted.cursor = Some((rect, color, hollow));
            }
        }
        self.terminal.update(cx, |t, _| {
            t.geometry.set(Some(Geometry {
                origin,
                cell,
                cursor: cursor_bounds,
            }));
        });
        painted
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<gpui::Pixels>,
        _: &mut (),
        painted: &mut Painted,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.terminal.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.terminal.clone()),
            cx,
        );
        // Drags keep tracking movement and release anywhere in the window.
        let terminal = self.terminal.downgrade();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == gpui::DispatchPhase::Capture {
                let _ = terminal.update(cx, |terminal, cx| terminal.mouse_move(event, cx));
            }
        });
        let terminal = self.terminal.downgrade();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == gpui::DispatchPhase::Capture && event.button == MouseButton::Left {
                let _ = terminal.update(cx, |terminal, cx| terminal.mouse_up(event, cx));
            }
        });
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for (rect, fill) in &painted.rects {
                window.paint_quad(gpui::fill(*rect, *fill));
            }
            for (origin, line) in &painted.lines {
                let _ = line.paint(
                    *origin,
                    painted.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            if let Some((rect, fill, hollow)) = painted.cursor {
                if hollow {
                    window.paint_quad(gpui::outline(rect, fill, gpui::BorderStyle::Solid));
                } else {
                    window.paint_quad(gpui::fill(rect, fill));
                }
            }
        });
    }
}

fn is_block(ch: char) -> bool {
    ('\u{2580}'..='\u{259f}').contains(&ch)
}

/// The rects that draw a block element (U+2580..=U+259F) over its cell, as
/// `[x0, y0, x1, y1]` in eighths of the cell, and the opacity of the shades.
fn block_shape(ch: char) -> Option<(Vec<[u8; 4]>, f32)> {
    let full = || vec![[0, 0, 8, 8]];
    let shape = match ch as u32 {
        0x2580 => (vec![[0, 0, 8, 4]], 1.),
        // Lower n/8 blocks.
        0x2581..=0x2587 => (vec![[0, (0x2588 - ch as u32) as u8, 8, 8]], 1.),
        0x2588 => (full(), 1.),
        // Left n/8 blocks.
        0x2589..=0x258f => (vec![[0, 0, (0x2590 - ch as u32) as u8, 8]], 1.),
        0x2590 => (vec![[4, 0, 8, 8]], 1.),
        0x2591 => (full(), 0.25),
        0x2592 => (full(), 0.5),
        0x2593 => (full(), 0.75),
        0x2594 => (vec![[0, 0, 8, 1]], 1.),
        0x2595 => (vec![[7, 0, 8, 8]], 1.),
        0x2596 => (vec![[0, 4, 4, 8]], 1.),
        0x2597 => (vec![[4, 4, 8, 8]], 1.),
        0x2598 => (vec![[0, 0, 4, 4]], 1.),
        0x2599 => (vec![[0, 0, 4, 4], [0, 4, 4, 8], [4, 4, 8, 8]], 1.),
        0x259a => (vec![[0, 0, 4, 4], [4, 4, 8, 8]], 1.),
        0x259b => (vec![[0, 0, 4, 4], [4, 0, 8, 4], [0, 4, 4, 8]], 1.),
        0x259c => (vec![[0, 0, 4, 4], [4, 0, 8, 4], [4, 4, 8, 8]], 1.),
        0x259d => (vec![[4, 0, 8, 4]], 1.),
        0x259e => (vec![[4, 0, 8, 4], [0, 4, 4, 8]], 1.),
        0x259f => (vec![[4, 0, 8, 4], [0, 4, 4, 8], [4, 4, 8, 8]], 1.),
        _ => return None,
    };
    Some(shape)
}

/// Rounds a logical position to a device pixel, so shared edges land on the
/// same pixel from both sides.
fn snap(value: gpui::Pixels, scale: f32) -> gpui::Pixels {
    px((value.as_f32() * scale).round() / scale)
}

fn cell_rect(
    origin: gpui::Point<gpui::Pixels>,
    cell: Size<gpui::Pixels>,
    line: usize,
    start: usize,
    end: usize,
) -> Bounds<gpui::Pixels> {
    Bounds::new(
        point(
            origin.x + cell.width * start as f32,
            origin.y + cell.height * line as f32,
        ),
        size(cell.width * (end - start) as f32, cell.height),
    )
}

const ANSI_DARK: [u32; 16] = [
    0x1d2428, 0xf87171, 0x4ade80, 0xfbbf24, 0x60a5fa, 0xc084fc, 0x22d3ee, 0xe8eef2, 0x64748b,
    0xfca5a5, 0x86efac, 0xfde68a, 0x93c5fd, 0xd8b4fe, 0x67e8f9, 0xf8fafc,
];
const ANSI_LIGHT: [u32; 16] = [
    0x1d2428, 0xb91c1c, 0x15803d, 0xa16207, 0x1d4ed8, 0x7e22ce, 0x0e7490, 0x64748b, 0x475569,
    0xdc2626, 0x16a34a, 0xca8a04, 0x2563eb, 0x9333ea, 0x0891b2, 0x1e293b,
];

fn ansi() -> &'static [u32; 16] {
    if theme::is_light() {
        &ANSI_LIGHT
    } else {
        &ANSI_DARK
    }
}
fn indexed_rgb(index: u8) -> u32 {
    match index {
        0..=15 => ansi()[index as usize],
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
            (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
        }
        _ => {
            let v = 8 + (index as u32 - 232) * 10;
            (v << 16) | (v << 8) | v
        }
    }
}
fn hex(rgb: Rgb) -> u32 {
    ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32
}
fn to_rgb(value: u32) -> Rgb {
    Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}
/// Reply colour for an OSC 4/10/11/12 query: an app-set colour wins.
fn color_rgb(index: usize, colors: &Colors) -> Rgb {
    if let Some(rgb) = colors[index] {
        return rgb;
    }
    let light = theme::is_light();
    to_rgb(match index {
        0..=255 => indexed_rgb(index as u8),
        257 => {
            if light {
                0xf5f5f5
            } else {
                0x171717
            }
        }
        _ => {
            if light {
                0x1d2428
            } else {
                0xe8eef2
            }
        }
    })
}
fn color(value: AnsiColor, colors: &Colors) -> Hsla {
    let dim = |c: u32| {
        let mut h: Hsla = rgb(c).into();
        h.a *= 0.66;
        h
    };
    match value {
        AnsiColor::Spec(spec) => rgb(hex(spec)).into(),
        AnsiColor::Indexed(index) => colors[index as usize]
            .map_or_else(|| rgb(indexed_rgb(index)).into(), |c| rgb(hex(c)).into()),
        AnsiColor::Named(name) => {
            if let Some(c) = colors[name] {
                return rgb(hex(c)).into();
            }
            let a = ansi();
            match name {
                NamedColor::Foreground | NamedColor::BrightForeground => theme::content(1.),
                NamedColor::DimForeground => theme::content(0.66),
                NamedColor::Background => gpui::transparent_black(),
                NamedColor::Cursor => theme::accent(),
                NamedColor::DimBlack => dim(a[0]),
                NamedColor::DimRed => dim(a[1]),
                NamedColor::DimGreen => dim(a[2]),
                NamedColor::DimYellow => dim(a[3]),
                NamedColor::DimBlue => dim(a[4]),
                NamedColor::DimMagenta => dim(a[5]),
                NamedColor::DimCyan => dim(a[6]),
                NamedColor::DimWhite => dim(a[7]),
                other => rgb(a[(other as usize).min(15)]).into(),
            }
        }
    }
}

/// Bytes a non-text keystroke sends. Plain characters (and Option combos) return None
/// so they reach the input handler, where accents and dead keys compose.
fn key_bytes(keystroke: &gpui::Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    let key = keystroke.key.as_str();
    if m.platform {
        if m.control || m.shift || m.alt {
            return None;
        }
        return match key {
            "left" => Some(b"\x01".to_vec()),
            "right" => Some(b"\x05".to_vec()),
            "backspace" => Some(b"\x15".to_vec()),
            _ => None,
        };
    }
    if m.alt && !m.control && !m.shift {
        match key {
            "left" => return Some(b"\x1bb".to_vec()),
            "right" => return Some(b"\x1bf".to_vec()),
            _ => {}
        }
    }
    let modifier = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let cursor = |final_byte: char| {
        Some(if modifier > 1 {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        })
    };
    let tilde = |code: u8| {
        Some(if modifier > 1 {
            format!("\x1b[{code};{modifier}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        })
    };
    let function = |final_byte: char| {
        Some(if modifier > 1 {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        } else {
            format!("\x1bO{final_byte}").into_bytes()
        })
    };
    let meta = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if m.alt {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        Some(out)
    };
    match key {
        "up" => return cursor('A'),
        "down" => return cursor('B'),
        "right" => return cursor('C'),
        "left" => return cursor('D'),
        "home" => return cursor('H'),
        "end" => return cursor('F'),
        "insert" => return tilde(2),
        "delete" => return tilde(3),
        "pageup" => return tilde(5),
        "pagedown" => return tilde(6),
        "f1" => return function('P'),
        "f2" => return function('Q'),
        "f3" => return function('R'),
        "f4" => return function('S'),
        "f5" => return tilde(15),
        "f6" => return tilde(17),
        "f7" => return tilde(18),
        "f8" => return tilde(19),
        "f9" => return tilde(20),
        "f10" => return tilde(21),
        "f11" => return tilde(23),
        "f12" => return tilde(24),
        "enter" => return meta(b"\r"),
        "escape" => return meta(b"\x1b"),
        "tab" if m.shift => return Some(b"\x1b[Z".to_vec()),
        "tab" => return meta(b"\t"),
        "backspace" if m.control => return meta(b"\x08"),
        "backspace" => return meta(b"\x7f"),
        _ => {}
    }
    if m.control {
        let ch = if key == "space" {
            ' '
        } else {
            let mut chars = key.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            ch
        };
        let code = match ch.to_ascii_lowercase() {
            c @ 'a'..='z' => c as u8 - b'a' + 1,
            ' ' | '@' | '2' => 0,
            '[' | '3' => 27,
            '\\' | '4' => 28,
            ']' | '5' => 29,
            '^' | '6' => 30,
            '_' | '-' | '/' | '7' => 31,
            '8' => 127,
            _ => return None,
        };
        return meta(&[code]);
    }
    None
}

#[cfg(test)]
mod close_tests {
    use super::*;

    #[test]
    fn a_shell_in_front_is_idle_and_another_group_is_a_running_command() {
        assert!(!foreground_busy(500, 500));
        assert!(foreground_busy(731, 500));
        // The group could not be read, or there is no shell.
        assert!(!foreground_busy(-1, 500));
        assert!(!foreground_busy(0, 500));
        assert!(!foreground_busy(731, 0));
    }

    #[test]
    fn only_busy_terminals_are_listed_together() {
        let all = [
            ("app 1".to_owned(), true),
            ("app 2".to_owned(), false),
            ("app 3".to_owned(), true),
        ];
        assert_eq!(running_names(&all), vec!["app 1", "app 3"]);
        assert!(running_names(&all[1..2]).is_empty());
        assert!(running_names(&[]).is_empty());
    }
}

#[cfg(test)]
mod url_tests {
    use super::url_at;

    #[test]
    fn a_web_address_is_found_around_the_click() {
        let line = "Local: (http://127.0.0.1:3000/a_(b)), then https://x.dev.  end";
        let local = line.find("127").unwrap();
        assert_eq!(url_at(line, local), Some("http://127.0.0.1:3000/a_(b)"));
        assert_eq!(
            url_at(line, line.find("x.dev").unwrap()),
            Some("https://x.dev")
        );
        assert_eq!(url_at(line, line.find("then").unwrap()), None);
        assert_eq!(url_at(line, line.find("Local").unwrap()), None);
        assert_eq!(url_at("https://", 2), None);
    }
}
