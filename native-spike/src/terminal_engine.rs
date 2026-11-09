//! The terminal engines behind the Terminal panel (ADR-199): what parses the shell's output and
//! keeps the grid, scrollback, modes and selection. The panel draws whatever engine a terminal
//! was opened with from the same `Snapshot`, so the GPUI drawing code is shared.
//!
//! Two engines exist: `alacritty_terminal` (the default) and Ghostty's `libghostty-vt`
//! (`ghostty_vt.rs`, only with the `ghostty` cargo feature). Settings → Terminal → Motor do
//! terminal picks one for the terminals opened after the change.
use alacritty_terminal::{
    event::{Event as TermEvent, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Point as GridPoint, Side as AlacrittySide},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{
        Config, Term, TermMode, cell::Flags, color::Colors, point_to_viewport, viewport_to_point,
    },
    vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Processor, StdSyncHandler},
};
use std::sync::Arc;

/// Which engine a terminal runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineKind {
    Alacritty,
    Ghostty,
}

impl EngineKind {
    /// The engine a setting value names, falling back to Alacritty for an unknown value or for
    /// Ghostty in a build without it.
    pub fn from_setting(value: &str) -> Self {
        if value == "ghostty" && ghostty_built_in() {
            Self::Ghostty
        } else {
            Self::Alacritty
        }
    }
    /// The engine Settings → Terminal → Motor do terminal names now.
    pub fn chosen() -> Self {
        Self::from_setting(&crate::prefs::choice("terminalEngine", "alacritty"))
    }
}

/// Whether this build carries libghostty-vt (the `ghostty` cargo feature).
pub const fn ghostty_built_in() -> bool {
    cfg!(feature = "ghostty")
}

/// A colour as the engine knows it; the panel turns it into the theme's colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellColor {
    /// The default text colour.
    Foreground,
    DimForeground,
    /// The default background: nothing is painted.
    Background,
    Cursor,
    /// One of the 256 palette entries (0-15 follow the theme).
    Indexed(u8),
    /// A dim variant of one of the eight basic colours.
    DimIndexed(u8),
    /// 0xRRGGBB.
    Rgb(u32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellFlags {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub inverse: bool,
    pub dim: bool,
    pub hidden: bool,
    /// The glyph takes this cell and the next.
    pub wide: bool,
}

/// One drawn cell of the viewport. Spacer cells after wide glyphs are left out.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub column: usize,
    pub ch: char,
    pub fg: CellColor,
    pub bg: CellColor,
    pub flags: CellFlags,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    /// Viewport line and column, or None when the view is scrolled away from it.
    pub at: Option<(usize, usize)>,
    /// The program shows the cursor.
    pub visible: bool,
}

/// What the panel draws: the viewport's lines, top first.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub lines: Vec<Vec<Cell>>,
    pub cursor: Cursor,
}

#[cfg(test)]
impl Snapshot {
    /// A line's text, a space for every empty cell (for tests).
    pub fn line_text(&self, line: usize) -> String {
        let mut text = String::new();
        let mut column = 0;
        for cell in self.lines.get(line).into_iter().flatten() {
            while column < cell.column {
                text.push(' ');
                column += 1;
            }
            text.push(cell.ch);
            column += if cell.flags.wide { 2 } else { 1 };
        }
        text.trim_end().to_owned()
    }
}

/// The modes the panel acts on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    pub alt_screen: bool,
    /// Wheel turns into arrow keys on the alternate screen (DECSET 1007).
    pub alternate_scroll: bool,
    /// Some mouse reporting mode (X10, normal, button or any-event) is on.
    pub mouse_tracking: bool,
    pub show_cursor: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollTo {
    /// Lines; positive goes back into the history.
    Delta(i32),
    Bottom,
}

/// A cell of the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewPoint {
    pub line: usize,
    pub column: usize,
}

/// Which half of the cell the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectKind {
    Simple,
    Word,
    Line,
}

/// A terminal engine: parses bytes, keeps the grid and answers the panel.
pub trait TerminalEngine: Send {
    #[allow(dead_code)]
    fn kind(&self) -> EngineKind;
    /// Parses bytes as if the shell printed them.
    fn feed(&mut self, bytes: &[u8]);
    fn resize(&mut self, columns: usize, lines: usize, cell_width: u32, cell_height: u32);
    fn snapshot(&mut self) -> Snapshot;
    fn scroll(&mut self, to: ScrollTo);
    fn modes(&self) -> Modes;
    /// The title a program set (OSC 0/2), when the engine keeps it. The panel labels tabs with
    /// the folder today, so nothing reads it yet.
    #[allow(dead_code)]
    fn title(&self) -> Option<String>;
    /// Answers the engine owes the shell (device status, colour queries, …) since the last call.
    /// Alacritty hands its answers to its event loop instead, so it always returns nothing.
    fn take_replies(&mut self) -> Vec<u8>;

    fn select_begin(&mut self, at: ViewPoint, side: Side, kind: SelectKind);
    fn select_extend(&mut self, at: ViewPoint, side: Side);
    fn select_all(&mut self);
    fn clear_selection(&mut self);
    fn has_selection(&self) -> bool;
    /// A selection that covers nothing (a click without a drag).
    fn selection_is_empty(&self) -> bool;
    fn selection_text(&self) -> Option<String>;
    /// The text of the line under `at`, joined with the lines it wraps into, and the byte offset
    /// of the cell under `at` in it.
    fn wrapped_line_at(&self, at: ViewPoint) -> Option<(String, usize)>;
}

/// Shared handle to a terminal's engine: the panel and, for Ghostty, the PTY reader use it.
pub type SharedEngine = Arc<std::sync::Mutex<Box<dyn TerminalEngine>>>;

// ---------------------------------------------------------------------------------------------
// Alacritty

#[derive(Clone)]
pub struct Listener(pub async_channel::Sender<TermEvent>);
impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.try_send(event);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    pub columns: usize,
    pub lines: usize,
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

pub type AlacrittyTerm = Arc<FairMutex<Term<Listener>>>;

/// `alacritty_terminal`'s `Term`. Its own event loop reads the PTY and writes into it.
pub struct AlacrittyEngine {
    term: AlacrittyTerm,
}

impl AlacrittyEngine {
    pub fn new(columns: usize, lines: usize, scrollback: usize, listener: Listener) -> Self {
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let size = GridSize { columns, lines };
        Self {
            term: Arc::new(FairMutex::new(Term::new(config, &size, listener))),
        }
    }
    /// The `Term` the alacritty event loop needs.
    pub fn term(&self) -> AlacrittyTerm {
        self.term.clone()
    }
    fn grid_point(term: &Term<Listener>, at: ViewPoint) -> GridPoint {
        viewport_to_point(
            term.grid().display_offset(),
            GridPoint::new(at.line, Column(at.column)),
        )
    }
}

fn alacritty_side(side: Side) -> AlacrittySide {
    match side {
        Side::Left => AlacrittySide::Left,
        Side::Right => AlacrittySide::Right,
    }
}

/// An alacritty colour for the panel; colours a program set (OSC 4/10/11) win.
fn alacritty_color(value: AnsiColor, colors: &Colors) -> CellColor {
    let hex = |c: alacritty_terminal::vte::ansi::Rgb| {
        ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
    };
    match value {
        AnsiColor::Spec(spec) => CellColor::Rgb(hex(spec)),
        AnsiColor::Indexed(index) => {
            colors[index as usize].map_or(CellColor::Indexed(index), |c| CellColor::Rgb(hex(c)))
        }
        AnsiColor::Named(name) => {
            if let Some(c) = colors[name] {
                return CellColor::Rgb(hex(c));
            }
            match name {
                NamedColor::Foreground | NamedColor::BrightForeground => CellColor::Foreground,
                NamedColor::DimForeground => CellColor::DimForeground,
                NamedColor::Background => CellColor::Background,
                NamedColor::Cursor => CellColor::Cursor,
                NamedColor::DimBlack => CellColor::DimIndexed(0),
                NamedColor::DimRed => CellColor::DimIndexed(1),
                NamedColor::DimGreen => CellColor::DimIndexed(2),
                NamedColor::DimYellow => CellColor::DimIndexed(3),
                NamedColor::DimBlue => CellColor::DimIndexed(4),
                NamedColor::DimMagenta => CellColor::DimIndexed(5),
                NamedColor::DimCyan => CellColor::DimIndexed(6),
                NamedColor::DimWhite => CellColor::DimIndexed(7),
                other => CellColor::Indexed((other as usize).min(15) as u8),
            }
        }
    }
}

impl TerminalEngine for AlacrittyEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::Alacritty
    }
    fn feed(&mut self, bytes: &[u8]) {
        let mut parser = Processor::<StdSyncHandler>::new();
        parser.advance(&mut *self.term.lock(), bytes);
    }
    fn resize(&mut self, columns: usize, lines: usize, _: u32, _: u32) {
        self.term.lock().resize(GridSize { columns, lines });
    }
    fn snapshot(&mut self) -> Snapshot {
        let term = self.term.lock();
        let lines = term.screen_lines();
        let content = term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset;
        let selection = content.selection;
        let mut rows: Vec<Vec<Cell>> = vec![Vec::new(); lines];
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
            // The default background stays unpainted even when a program recoloured it.
            let bg = match c.bg {
                AnsiColor::Named(NamedColor::Background) => CellColor::Background,
                other => alacritty_color(other, colors),
            };
            rows[at.line].push(Cell {
                column: at.column.0,
                ch: c.c,
                fg: alacritty_color(c.fg, colors),
                bg,
                flags: CellFlags {
                    bold: c.flags.contains(Flags::BOLD),
                    italic: c.flags.contains(Flags::ITALIC),
                    underline: c.flags.intersects(Flags::ALL_UNDERLINES),
                    strike: c.flags.contains(Flags::STRIKEOUT),
                    inverse: c.flags.contains(Flags::INVERSE),
                    dim: c.flags.contains(Flags::DIM),
                    hidden: c.flags.contains(Flags::HIDDEN),
                    wide: c.flags.contains(Flags::WIDE_CHAR),
                },
                selected: selection.is_some_and(|s| s.contains(indexed.point)),
            });
        }
        let visible = content.mode.contains(TermMode::SHOW_CURSOR)
            && content.cursor.shape != CursorShape::Hidden;
        let at = point_to_viewport(offset, content.cursor.point)
            .filter(|at| at.line < lines)
            .map(|at| (at.line, at.column.0));
        Snapshot {
            lines: rows,
            cursor: Cursor { at, visible },
        }
    }
    fn scroll(&mut self, to: ScrollTo) {
        self.term.lock().scroll_display(match to {
            ScrollTo::Delta(lines) => Scroll::Delta(lines),
            ScrollTo::Bottom => Scroll::Bottom,
        });
    }
    fn modes(&self) -> Modes {
        let mode = *self.term.lock().mode();
        Modes {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            alt_screen: mode.contains(TermMode::ALT_SCREEN),
            alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
            mouse_tracking: mode.intersects(TermMode::MOUSE_MODE),
            show_cursor: mode.contains(TermMode::SHOW_CURSOR),
        }
    }
    fn title(&self) -> Option<String> {
        // Alacritty sends titles as events (TermEvent::Title); the panel keeps the last one.
        None
    }
    fn take_replies(&mut self) -> Vec<u8> {
        Vec::new()
    }
    fn select_begin(&mut self, at: ViewPoint, side: Side, kind: SelectKind) {
        let mut term = self.term.lock();
        let point = Self::grid_point(&term, at);
        let ty = match kind {
            SelectKind::Simple => SelectionType::Simple,
            SelectKind::Word => SelectionType::Semantic,
            SelectKind::Line => SelectionType::Lines,
        };
        term.selection = Some(Selection::new(ty, point, alacritty_side(side)));
    }
    fn select_extend(&mut self, at: ViewPoint, side: Side) {
        let mut term = self.term.lock();
        let point = Self::grid_point(&term, at);
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, alacritty_side(side));
        }
    }
    fn select_all(&mut self) {
        let mut term = self.term.lock();
        let mut selection = Selection::new(
            SelectionType::Simple,
            GridPoint::new(term.topmost_line(), Column(0)),
            AlacrittySide::Left,
        );
        selection.update(
            GridPoint::new(term.bottommost_line(), term.last_column()),
            AlacrittySide::Right,
        );
        term.selection = Some(selection);
    }
    fn clear_selection(&mut self) {
        self.term.lock().selection = None;
    }
    fn has_selection(&self) -> bool {
        self.term.lock().selection.is_some()
    }
    fn selection_is_empty(&self) -> bool {
        self.term
            .lock()
            .selection
            .as_ref()
            .is_some_and(|s| s.is_empty())
    }
    fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }
    fn wrapped_line_at(&self, at: ViewPoint) -> Option<(String, usize)> {
        let term = self.term.lock();
        let at = Self::grid_point(&term, at);
        let grid = term.grid();
        let columns = grid.columns();
        let wrapped = |line: alacritty_terminal::index::Line| {
            grid[line][Column(columns - 1)]
                .flags
                .contains(Flags::WRAPLINE)
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
        Some((text, under?))
    }
}

// ---------------------------------------------------------------------------------------------
// A PTY read on its own thread, for engines without an event loop of their own (Ghostty).

/// Reads the shell's output into the engine on a thread, writes the engine's replies and the
/// keyboard's bytes back on another, and reports wakeups and the shell's exit as alacritty
/// events so the panel handles both engines alike.
pub struct DirectPty {
    input: std::sync::mpsc::Sender<Vec<u8>>,
    fd: i32,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl DirectPty {
    pub fn spawn(
        pty: alacritty_terminal::tty::Pty,
        engine: SharedEngine,
        events: async_channel::Sender<TermEvent>,
    ) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        use std::sync::atomic::{AtomicBool, Ordering};
        let read_fd = unsafe { libc::dup(pty.file().as_raw_fd()) };
        let write_fd = unsafe { libc::dup(pty.file().as_raw_fd()) };
        let resize_fd = unsafe { libc::dup(pty.file().as_raw_fd()) };
        if read_fd < 0 || write_fd < 0 || resize_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (input, queue) = std::sync::mpsc::channel::<Vec<u8>>();
        // Writer: keyboard bytes and the engine's replies, in order.
        std::thread::Builder::new()
            .name("sirus-pty-write".into())
            .spawn(move || {
                while let Ok(bytes) = queue.recv() {
                    if !write_all(write_fd, &bytes) {
                        break;
                    }
                }
                unsafe { libc::close(write_fd) };
            })?;
        let replies = input.clone();
        let halt = stop.clone();
        std::thread::Builder::new()
            .name("sirus-pty-read".into())
            .spawn(move || {
                let pid = pty.child().id() as i32;
                let mut buffer = vec![0u8; 64 * 1024];
                while !halt.load(Ordering::Relaxed) {
                    let mut poll = libc::pollfd {
                        fd: read_fd,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    let ready = unsafe { libc::poll(&mut poll, 1, 200) };
                    if ready == 0 {
                        continue;
                    }
                    if ready < 0 {
                        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                        {
                            continue;
                        }
                        break;
                    }
                    let n =
                        unsafe { libc::read(read_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
                    if n == 0 {
                        break;
                    }
                    if n < 0 {
                        let error = std::io::Error::last_os_error();
                        match error.kind() {
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => {
                                continue;
                            }
                            // EIO: the shell and everything holding the PTY are gone.
                            _ => break,
                        }
                    }
                    let reply = {
                        let mut engine = lock(&engine);
                        engine.feed(&buffer[..n as usize]);
                        engine.take_replies()
                    };
                    if !reply.is_empty() {
                        let _ = replies.send(reply);
                    }
                    let _ = events.try_send(TermEvent::Wakeup);
                }
                unsafe { libc::close(read_fd) };
                if halt.load(Ordering::Relaxed) {
                    return;
                }
                let mut status = 0;
                let waited = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
                if waited == pid {
                    use std::os::unix::process::ExitStatusExt;
                    let status = std::process::ExitStatus::from_raw(status);
                    let _ = events.try_send(TermEvent::ChildExit(status));
                } else {
                    let _ = events.try_send(TermEvent::Exit);
                }
                // Dropping the PTY hangs up whatever still holds it.
                drop(pty);
            })?;
        Ok(Self {
            input,
            fd: resize_fd,
            stop,
        })
    }
    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.input.send(bytes);
    }
    pub fn resize(&self, columns: u16, lines: u16, cell_width: u16, cell_height: u16) {
        let size = libc::winsize {
            ws_row: lines,
            ws_col: columns,
            ws_xpixel: columns.saturating_mul(cell_width),
            ws_ypixel: lines.saturating_mul(cell_height),
        };
        if self.fd >= 0 {
            unsafe { libc::ioctl(self.fd, libc::TIOCSWINSZ, &size) };
        }
    }
}

impl Drop for DirectPty {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if self.fd >= 0 {
            unsafe { libc::close(self.fd) };
        }
    }
}

/// Writes every byte to a non-blocking fd, waiting for room when it is full.
fn write_all(fd: i32, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n > 0 {
            bytes = &bytes[n as usize..];
            continue;
        }
        let error = std::io::Error::last_os_error();
        match error.kind() {
            std::io::ErrorKind::Interrupted => {}
            std::io::ErrorKind::WouldBlock => {
                let mut poll = libc::pollfd {
                    fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                unsafe { libc::poll(&mut poll, 1, 1000) };
            }
            _ => return false,
        }
    }
    true
}

/// A new engine of `kind` for a `columns` × `lines` terminal, and for alacritty the `Term` its
/// event loop needs. A Ghostty engine that cannot start falls back to alacritty.
pub fn new_engine(
    kind: EngineKind,
    columns: usize,
    lines: usize,
    scrollback: usize,
    listener: Listener,
) -> (Box<dyn TerminalEngine>, Option<AlacrittyTerm>) {
    #[cfg(feature = "ghostty")]
    if kind == EngineKind::Ghostty
        && let Ok(engine) = crate::ghostty_vt::GhosttyEngine::new(
            columns,
            lines,
            scrollback,
            crate::ghostty_vt::ThemeColors::current(),
        )
    {
        return (Box::new(engine), None);
    }
    let _ = kind;
    let engine = AlacrittyEngine::new(columns, lines, scrollback, listener);
    let term = engine.term();
    (Box::new(engine), Some(term))
}

/// Locks a shared engine, even after a panic elsewhere left the lock poisoned.
pub fn lock(engine: &SharedEngine) -> std::sync::MutexGuard<'_, Box<dyn TerminalEngine>> {
    engine
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alacritty(columns: usize, lines: usize) -> AlacrittyEngine {
        let (tx, _rx) = async_channel::unbounded();
        AlacrittyEngine::new(columns, lines, 1000, Listener(tx))
    }

    #[test]
    fn an_unknown_or_missing_engine_falls_back_to_alacritty() {
        assert_eq!(EngineKind::from_setting(""), EngineKind::Alacritty);
        assert_eq!(EngineKind::from_setting("kitty"), EngineKind::Alacritty);
        assert_eq!(
            EngineKind::from_setting("ghostty") == EngineKind::Ghostty,
            ghostty_built_in()
        );
    }

    #[test]
    fn alacritty_draws_text_colours_and_the_cursor() {
        let mut engine = alacritty(20, 4);
        engine.feed(b"hi \x1b[31mred\x1b[0m\r\n\x1b[1;38;2;1;2;3mx");
        let snap = engine.snapshot();
        assert_eq!(snap.line_text(0), "hi red");
        let r = snap.lines[0].iter().find(|c| c.ch == 'r').unwrap();
        assert_eq!(r.fg, CellColor::Indexed(1));
        let x = &snap.lines[1][0];
        assert_eq!(x.fg, CellColor::Rgb(0x010203));
        assert!(x.flags.bold);
        assert_eq!(snap.cursor.at, Some((1, 1)));
        assert!(snap.cursor.visible);
    }

    #[test]
    fn alacritty_selects_and_copies() {
        let mut engine = alacritty(20, 4);
        engine.feed(b"hello world");
        let at = |column| ViewPoint { line: 0, column };
        engine.select_begin(at(0), Side::Left, SelectKind::Simple);
        assert!(engine.selection_is_empty());
        engine.select_extend(at(4), Side::Right);
        assert_eq!(engine.selection_text().as_deref(), Some("hello"));
        assert!(engine.snapshot().lines[0][0].selected);
        engine.clear_selection();
        assert!(!engine.has_selection());
    }
}
