//! The text editor of the Arquivos pane: the smallest honest editor. A caret, typing through the
//! platform's input handler (accents and IME), selection with the arrows and Shift, copy, cut,
//! paste, undo and redo, Home and End, a click to place the caret, and ⌘S, which the pane answers
//! by writing the file through the host. Syntax colors come from syntax.rs and are parsed in the
//! background after a pause in typing.
//!
//! Also here: drag selection (a drag past the top or bottom edge scrolls one line a move), a double
//! click for a word, a triple click for a line, and Option+arrows and Option+Backspace by word.
//!
//! Not here yet: wrapping (lines run on, and the pane does not scroll sideways), find and replace,
//! and auto-indent. Click placement assumes one advance per character, so tabs and wide characters
//! put the caret a little off.
//!
//! `Buffer` is the text, its selection and its edits, with no UI, so it is unit-tested on its own.
//! Positions are byte offsets that always sit on a character boundary.

use crate::panel_files::{byte_at, group_color, utf16_len};
use crate::project_files;
use crate::syntax::{self, Group, LineSpans};
use crate::theme::{self, CARET_WIDTH, caret_color, content};
use gpui::{
    AnyElement, Bounds, ClipboardItem, Context, DispatchPhase, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Font, Hsla, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ScrollStrategy,
    ShapedLine, SharedString, TextAlign, TextRun, UTF16Selection, UniformListScrollHandle, Window,
    canvas, div, fill, point, prelude::*, px, rgb, size, uniform_list,
};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

/// Files over this size open read-only (the pane's Source view), as the syntax parser's limit.
pub const MAX_EDITABLE_BYTES: usize = 2_000_000;
/// A line of the editor, in pixels: the Source view's row height.
const ROW: f32 = 19.;
/// The gutter before the text: the Source view's line number column, 48 px wide with its 14 px
/// padding inside (the box is border-box), so the text starts here.
const GUTTER: f32 = 48.;
const FONT_SIZE: f32 = 12.5;
/// Quiet time after the last edit before the colors are parsed again.
const HIGHLIGHT_DEBOUNCE_MS: u64 = 150;
/// Lines a page key moves the caret by.
const PAGE_LINES: isize = 20;
/// What a Tab inserts (CodeMirror's default indent unit).
const INDENT: &str = "  ";

// ---- The buffer -----------------------------------------------------------------------------

/// The selection: the anchor and the caret. They are equal when nothing is selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sel {
    pub anchor: usize,
    pub cursor: usize,
}

/// One change: `removed` bytes at `start` were replaced by `inserted`. Undo puts them back.
#[derive(Clone, Debug)]
struct Edit {
    start: usize,
    removed: String,
    inserted: String,
    before: Sel,
    after: Sel,
}

/// The text, the selection, and the edits made to it. Undo and redo walk `history`; `applied` is
/// how many of its edits are in the text now, and `saved` is the `applied` count the file was last
/// written at (None once that state can no longer be reached).
pub struct Buffer {
    text: String,
    sel: Sel,
    /// The column a run of Up and Down keeps, in characters.
    goal: Option<usize>,
    history: Vec<Edit>,
    applied: usize,
    saved: Option<usize>,
    /// Counts every change to the text, so a save only marks the text it wrote.
    revision: u64,
    /// The byte offset each line starts at. Line 0 starts at 0; a line ends before the next start.
    starts: Vec<usize>,
}

impl Buffer {
    pub fn new(text: String) -> Self {
        let mut buffer = Self {
            text,
            sel: Sel::default(),
            goal: None,
            history: Vec::new(),
            applied: 0,
            saved: Some(0),
            revision: 0,
            starts: Vec::new(),
        };
        buffer.reindex();
        buffer
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn selection(&self) -> Range<usize> {
        self.sel.anchor.min(self.sel.cursor)..self.sel.anchor.max(self.sel.cursor)
    }

    pub fn sel(&self) -> Sel {
        self.sel
    }

    pub fn cursor(&self) -> usize {
        self.sel.cursor
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// True when the text differs from the last text marked as saved.
    pub fn is_modified(&self) -> bool {
        self.saved != Some(self.applied)
    }

    /// Marks the text of `revision` as saved. A later change makes the mark stale and is ignored.
    pub fn mark_saved(&mut self, revision: u64) -> bool {
        if revision != self.revision {
            return false;
        }
        self.saved = Some(self.applied);
        true
    }

    /// Replaces `range` with `with`; the caret goes to the end of the new text. An empty range and
    /// an empty `with` change nothing.
    pub fn replace(&mut self, range: Range<usize>, with: &str) {
        let (a, b) = (self.floor(range.start), self.floor(range.end));
        let range = a.min(b)..a.max(b);
        if range.is_empty() && with.is_empty() {
            return;
        }
        let removed = self.text[range.clone()].to_owned();
        let before = self.sel;
        self.text.replace_range(range.clone(), with);
        let cursor = range.start + with.len();
        let after = Sel {
            anchor: cursor,
            cursor,
        };
        self.sel = after;
        self.goal = None;
        self.record(Edit {
            start: range.start,
            removed,
            inserted: with.to_owned(),
            before,
            after,
        });
        self.reindex();
        self.revision += 1;
    }

    /// Types `text` over the selection.
    pub fn insert(&mut self, text: &str) {
        let range = self.selection();
        self.replace(range, text);
    }

    /// Backspace: deletes the selection, else the character before the caret.
    pub fn delete_backward(&mut self) {
        let range = self.selection();
        if !range.is_empty() {
            self.replace(range, "");
        } else if self.sel.cursor > 0 {
            let from = self.prev(self.sel.cursor);
            self.replace(from..self.sel.cursor, "");
        }
    }

    /// Delete: deletes the selection, else the character after the caret.
    pub fn delete_forward(&mut self) {
        let range = self.selection();
        if !range.is_empty() {
            self.replace(range, "");
        } else if self.sel.cursor < self.text.len() {
            let to = self.next(self.sel.cursor);
            self.replace(self.sel.cursor..to, "");
        }
    }

    /// Moves the caret to `byte`; `extend` keeps the anchor, which grows the selection.
    pub fn move_to(&mut self, byte: usize, extend: bool) {
        let byte = self.floor(byte);
        self.sel.cursor = byte;
        if !extend {
            self.sel.anchor = byte;
        }
        self.goal = None;
    }

    /// Left: collapses a selection to its start, else moves one character back.
    pub fn move_left(&mut self, extend: bool) {
        let range = self.selection();
        if !extend && !range.is_empty() {
            self.move_to(range.start, false);
        } else {
            self.move_to(self.prev(self.sel.cursor), extend);
        }
    }

    /// Right: collapses a selection to its end, else moves one character forward.
    pub fn move_right(&mut self, extend: bool) {
        let range = self.selection();
        if !extend && !range.is_empty() {
            self.move_to(range.end, false);
        } else {
            self.move_to(self.next(self.sel.cursor), extend);
        }
    }

    /// Up or Down by `lines`, keeping the column the run of moves started in.
    pub fn move_vertical(&mut self, lines: isize, extend: bool) {
        let line = self.line_of(self.sel.cursor) as isize;
        let column = self.goal.unwrap_or_else(|| self.column_of(self.sel.cursor));
        let last = self.starts.len() as isize - 1;
        let target = (line + lines).clamp(0, last) as usize;
        let byte = self.byte_of(target, column);
        self.move_to(byte, extend);
        self.goal = Some(column);
    }

    /// The caret to the start of its line.
    pub fn home(&mut self, extend: bool) {
        let start = self.line_range(self.line_of(self.sel.cursor)).start;
        self.move_to(start, extend);
    }

    /// The caret to the end of its line.
    pub fn end(&mut self, extend: bool) {
        let end = self.line_range(self.line_of(self.sel.cursor)).end;
        self.move_to(end, extend);
    }

    /// Option+Left: the caret to the start of the word before it.
    pub fn move_word_left(&mut self, extend: bool) {
        self.move_to(self.word_left(self.sel.cursor), extend);
    }

    /// Option+Right: the caret to the end of the word after it.
    pub fn move_word_right(&mut self, extend: bool) {
        self.move_to(self.word_right(self.sel.cursor), extend);
    }

    /// Option+Backspace: deletes the selection, else the word before the caret.
    pub fn delete_word_backward(&mut self) {
        let range = self.selection();
        if !range.is_empty() {
            self.replace(range, "");
        } else if self.sel.cursor > 0 {
            let from = self.word_left(self.sel.cursor);
            self.replace(from..self.sel.cursor, "");
        }
    }

    /// The start of the word before `byte`: the whitespace before it is passed, then the run of one
    /// kind of character (a word, or a mark or punctuation).
    pub fn word_left(&self, byte: usize) -> usize {
        let mut at = self.floor(byte);
        while let Some(c) = self.before(at).filter(|c| c.is_whitespace()) {
            at -= c.len_utf8();
        }
        if let Some(kind) = self.before(at).map(class) {
            while let Some(c) = self.before(at).filter(|c| class(*c) == kind) {
                at -= c.len_utf8();
            }
        }
        at
    }

    /// The end of the word after `byte`: the whitespace after it is passed, then the run of one
    /// kind of character.
    pub fn word_right(&self, byte: usize) -> usize {
        let mut at = self.floor(byte);
        while let Some(c) = self.after(at).filter(|c| c.is_whitespace()) {
            at += c.len_utf8();
        }
        if let Some(kind) = self.after(at).map(class) {
            while let Some(c) = self.after(at).filter(|c| class(*c) == kind) {
                at += c.len_utf8();
            }
        }
        at
    }

    /// The word a double click on `byte` picks: the run of one kind of character around it, within
    /// its line. Between two characters the word wins over a space or a mark.
    pub fn word_at(&self, byte: usize) -> Range<usize> {
        let at = self.floor(byte);
        let after = self.after(at).filter(|c| *c != '\n');
        let before = self.before(at).filter(|c| *c != '\n');
        let picked = match (after, before) {
            (Some(a), _) if class(a) == 0 => Some((at, a)),
            (_, Some(b)) if class(b) == 0 => Some((at - b.len_utf8(), b)),
            (Some(a), _) => Some((at, a)),
            (None, Some(b)) => Some((at - b.len_utf8(), b)),
            (None, None) => None,
        };
        let Some((mut start, pick)) = picked else {
            return at..at;
        };
        let kind = class(pick);
        let mut end = start + pick.len_utf8();
        while let Some(c) = self.before(start).filter(|c| *c != '\n' && class(*c) == kind) {
            start -= c.len_utf8();
        }
        while let Some(c) = self.after(end).filter(|c| *c != '\n' && class(*c) == kind) {
            end += c.len_utf8();
        }
        start..end
    }

    /// A double click: selects the word on `byte`.
    pub fn select_word_at(&mut self, byte: usize) {
        let word = self.word_at(byte);
        self.sel = Sel {
            anchor: word.start,
            cursor: word.end,
        };
        self.goal = None;
    }

    /// A triple click: selects the line on `byte`, its newline included, as the next line starts
    /// where the selection ends.
    pub fn select_line_at(&mut self, byte: usize) {
        let line = self.line_range(self.line_of(byte));
        let end = if line.end < self.text.len() {
            line.end + 1
        } else {
            line.end
        };
        self.sel = Sel {
            anchor: line.start,
            cursor: end,
        };
        self.goal = None;
    }

    /// The character before `at`, if any.
    fn before(&self, at: usize) -> Option<char> {
        self.text[..at].chars().next_back()
    }

    /// The character from `at` on, if any.
    fn after(&self, at: usize) -> Option<char> {
        self.text[at..].chars().next()
    }

    pub fn select_all(&mut self) {
        self.sel = Sel {
            anchor: 0,
            cursor: self.text.len(),
        };
        self.goal = None;
    }

    /// Undoes the last change; false when there is none.
    pub fn undo(&mut self) -> bool {
        if self.applied == 0 {
            return false;
        }
        self.applied -= 1;
        let edit = self.history[self.applied].clone();
        self.text
            .replace_range(edit.start..edit.start + edit.inserted.len(), &edit.removed);
        self.sel = edit.before;
        self.goal = None;
        self.reindex();
        self.revision += 1;
        true
    }

    /// Redoes the change last undone; false when there is none.
    pub fn redo(&mut self) -> bool {
        if self.applied == self.history.len() {
            return false;
        }
        let edit = self.history[self.applied].clone();
        self.text
            .replace_range(edit.start..edit.start + edit.removed.len(), &edit.inserted);
        self.applied += 1;
        self.sel = edit.after;
        self.goal = None;
        self.reindex();
        self.revision += 1;
        true
    }

    /// Adds an edit at the end of the history. A run of typing letters, or of Backspace or Delete
    /// presses in a word, is one edit, so one undo takes back a word rather than a letter.
    fn record(&mut self, edit: Edit) {
        self.history.truncate(self.applied);
        if self.saved.is_some_and(|saved| saved > self.applied) {
            self.saved = None;
        }
        if let Some(last) = self.history.last_mut() {
            if mergeable(last, &edit) {
                // The saved text was the one this edit now extends, so it is gone.
                if self.saved == Some(self.applied) {
                    self.saved = None;
                }
                merge(last, edit);
                return;
            }
        }
        self.history.push(edit);
        self.applied += 1;
    }

    fn reindex(&mut self) {
        self.starts.clear();
        self.starts.push(0);
        self.starts.extend(
            self.text
                .bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(at, _)| at + 1),
        );
    }

    /// The number of lines: one more than the newlines, so an empty text has one.
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The line `byte` is on (a newline belongs to the line it ends).
    pub fn line_of(&self, byte: usize) -> usize {
        let byte = self.floor(byte);
        self.starts.partition_point(|&start| start <= byte) - 1
    }

    /// The bytes of a line, without its newline. A line past the last one is the last one.
    pub fn line_range(&self, line: usize) -> Range<usize> {
        let line = line.min(self.starts.len() - 1);
        let start = self.starts[line];
        let end = self
            .starts
            .get(line + 1)
            .map_or(self.text.len(), |next| next - 1);
        start..end
    }

    /// The column of `byte` in its line, in characters.
    pub fn column_of(&self, byte: usize) -> usize {
        let byte = self.floor(byte);
        let start = self.starts[self.line_of(byte)];
        self.text[start..byte].chars().count()
    }

    /// The byte of `column` characters into `line`, or the line's end when it is shorter.
    pub fn byte_of(&self, line: usize, column: usize) -> usize {
        let range = self.line_range(line);
        let mut byte = range.start;
        for c in self.text[range].chars().take(column) {
            byte += c.len_utf8();
        }
        byte
    }

    /// `byte` moved back to a character boundary, and to the text's end at most.
    fn floor(&self, byte: usize) -> usize {
        let mut byte = byte.min(self.text.len());
        while !self.text.is_char_boundary(byte) {
            byte -= 1;
        }
        byte
    }

    fn prev(&self, byte: usize) -> usize {
        self.text[..byte]
            .chars()
            .next_back()
            .map_or(0, |c| byte - c.len_utf8())
    }

    fn next(&self, byte: usize) -> usize {
        self.text[byte..]
            .chars()
            .next()
            .map_or(byte, |c| byte + c.len_utf8())
    }
}

/// What a word is made of, for Option+arrows and double clicks: 0 for letters, digits and `_`
/// (Unicode letters included), 2 for whitespace, and 1 for everything else, such as punctuation.
fn class(c: char) -> u8 {
    if c.is_whitespace() {
        2
    } else if c.is_alphanumeric() || c == '_' {
        0
    } else {
        1
    }
}

/// Whether `next` is the same kind of change as `last` and continues it: letters typed in a row,
/// or Backspace or Delete presses in a row over text without spaces.
fn mergeable(last: &Edit, next: &Edit) -> bool {
    let plain = |text: &str| !text.chars().any(char::is_whitespace);
    if last.removed.is_empty() && next.removed.is_empty() {
        return !last.inserted.is_empty()
            && !next.inserted.is_empty()
            && plain(&next.inserted)
            && last
                .inserted
                .chars()
                .last()
                .is_some_and(|c| !c.is_whitespace())
            && next.start == last.start + last.inserted.len();
    }
    if last.inserted.is_empty() && next.inserted.is_empty() {
        let joins = next.start + next.removed.len() == last.start // Backspace
            || next.start == last.start; // Delete
        return joins && plain(&next.removed) && plain(&last.removed);
    }
    false
}

fn merge(last: &mut Edit, next: Edit) {
    if last.removed.is_empty() {
        last.inserted.push_str(&next.inserted);
    } else if next.start + next.removed.len() == last.start {
        let mut removed = next.removed;
        removed.push_str(&last.removed);
        last.removed = removed;
        last.start = next.start;
    } else {
        last.removed.push_str(&next.removed);
    }
    last.after = next.after;
}

// ---- The editor view ------------------------------------------------------------------------

/// What the editor tells its pane: a change (the tab's dot follows it) and ⌘S (the pane saves).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    Changed,
    Save,
}

/// Where the editor drew its box and caret on the last frame: the caret's rectangle is the input
/// method's candidate position, and the box is where a click's x is measured from.
#[derive(Default)]
struct Geometry {
    bounds: Option<Bounds<Pixels>>,
    caret: Option<Bounds<Pixels>>,
    /// The lines' box, below the error line; a drag past its edge scrolls.
    list: Option<Bounds<Pixels>>,
}

pub struct Editor {
    buffer: Buffer,
    /// The file's name, which picks the grammar.
    name: String,
    /// Colors per line of the text as it was when parsed. They stay until the next parse lands.
    spans: Option<Rc<Vec<LineSpans>>>,
    highlight_seq: u64,
    /// The text the input method is composing, as bytes.
    marked: Option<Range<usize>>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// A failed save, shown above the text until the next change or save.
    error: Option<String>,
    /// The width of one character in the editor's font.
    advance: f32,
    geometry: Rc<RefCell<Geometry>>,
    /// The left button went down on the text and the selection follows the pointer.
    dragging: bool,
}

impl EventEmitter<EditorEvent> for Editor {}

impl Editor {
    pub fn new(name: &str, text: String, cx: &mut Context<Self>) -> Self {
        let mut editor = Self {
            buffer: Buffer::new(text),
            name: name.to_owned(),
            spans: None,
            highlight_seq: 0,
            marked: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            error: None,
            advance: FONT_SIZE * 0.6,
            geometry: Rc::default(),
            dragging: false,
        };
        editor.schedule_highlight(cx);
        editor
    }

    pub fn is_modified(&self) -> bool {
        self.buffer.is_modified()
    }

    /// The text to save, and its revision, for `mark_saved` once the write succeeds.
    pub fn snapshot(&self) -> (String, u64) {
        (self.buffer.text().to_owned(), self.buffer.revision())
    }

    pub fn mark_saved(&mut self, revision: u64, cx: &mut Context<Self>) {
        if self.buffer.mark_saved(revision) {
            self.error = None;
            cx.emit(EditorEvent::Changed);
            cx.notify();
        }
    }

    pub fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.error = error;
        cx.notify();
    }

    /// Parses the colors in the background once typing pauses. A newer edit drops the older answer.
    fn schedule_highlight(&mut self, cx: &mut Context<Self>) {
        if !syntax::supports(&self.name) {
            return;
        }
        self.highlight_seq += 1;
        let seq = self.highlight_seq;
        let name = self.name.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(HIGHLIGHT_DEBOUNCE_MS))
                .await;
            let Ok(Some(text)) = this.update(cx, |this, _| {
                (this.highlight_seq == seq).then(|| this.buffer.text().to_owned())
            }) else {
                return;
            };
            let spans = project_files::run_blocking(move || syntax::highlight(&name, &text))
                .recv()
                .await
                .ok()
                .flatten();
            let _ = this.update(cx, |this, cx| {
                if this.highlight_seq == seq {
                    this.spans = spans.map(Rc::new);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Runs after a key or the input method changed the text or the caret: keeps the caret in view,
    /// parses again when the text changed, and tells the pane.
    fn edited(&mut self, from: u64, cx: &mut Context<Self>) {
        self.marked = None;
        if self.buffer.revision() != from {
            self.error = None;
            self.schedule_highlight(cx);
            cx.emit(EditorEvent::Changed);
        }
        self.reveal_caret();
        cx.notify();
    }

    fn reveal_caret(&self) {
        let line = self.buffer.line_of(self.buffer.cursor());
        self.scroll.scroll_to_item(line, ScrollStrategy::Nearest);
    }

    /// The column under `x`, in characters, counted from the text's left edge.
    fn column_at(&self, x: f32) -> usize {
        let left = self
            .geometry
            .borrow()
            .bounds
            .map_or(0., |bounds| f32::from(bounds.origin.x));
        ((x - left - GUTTER) / self.advance).round().max(0.) as usize
    }

    /// A press on `line`: a click places the caret, a double click selects the word and a triple
    /// click the line. Only a single click starts a drag.
    fn press(&mut self, line: usize, event: &MouseDownEvent) {
        let byte = self
            .buffer
            .byte_of(line, self.column_at(f32::from(event.position.x)));
        match event.click_count {
            2 => self.buffer.select_word_at(byte),
            3.. => self.buffer.select_line_at(byte),
            _ => self.buffer.move_to(byte, event.modifiers.shift),
        }
        self.dragging = event.click_count == 1;
    }

    /// The pointer moved with the button held: the selection's end follows it. Past the top or the
    /// bottom edge of the lines, the list scrolls one line toward the pointer.
    fn drag_to(&mut self, position: Point<Pixels>) {
        let list = self.geometry.borrow().list;
        let Some(list) = list else {
            return;
        };
        let count = self.buffer.line_count();
        let (first, inner) = self.scroll.0.borrow().base_handle.logical_scroll_top();
        let (top, height) = (f32::from(list.origin.y), f32::from(list.size.height));
        let y = f32::from(position.y);
        let line = if y < top {
            let line = first.saturating_sub(1);
            self.scroll.scroll_to_item(line, ScrollStrategy::Nearest);
            line
        } else if y >= top + height {
            let visible = (height / ROW).ceil().max(1.) as usize;
            let line = (first + visible).min(count - 1);
            self.scroll.scroll_to_item(line, ScrollStrategy::Nearest);
            line
        } else {
            let rows = (y - top + f32::from(inner)) / ROW;
            (first + rows.max(0.) as usize).min(count - 1)
        };
        let byte = self
            .buffer
            .byte_of(line, self.column_at(f32::from(position.x)));
        self.buffer.move_to(byte, true);
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let (shift, command) = (keys.modifiers.shift, keys.modifiers.platform);
        let from = self.buffer.revision();
        let handled = match keys.key.as_str() {
            "s" if command => {
                cx.emit(EditorEvent::Save);
                true
            }
            "z" if command && shift => self.buffer.redo(),
            "z" if command => self.buffer.undo(),
            "y" if command => self.buffer.redo(),
            "a" if command => {
                self.buffer.select_all();
                true
            }
            "c" if command => {
                let text = self.buffer.selected_text().to_owned();
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                true
            }
            "x" if command => {
                let text = self.buffer.selected_text().to_owned();
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.buffer.insert("");
                }
                true
            }
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.buffer.insert(&normalize(&text));
                }
                true
            }
            "enter" => {
                self.buffer.insert("\n");
                true
            }
            "tab" if !shift => {
                self.buffer.insert(INDENT);
                true
            }
            "backspace" if keys.modifiers.alt => {
                self.buffer.delete_word_backward();
                true
            }
            "backspace" => {
                self.buffer.delete_backward();
                true
            }
            "delete" => {
                self.buffer.delete_forward();
                true
            }
            "left" if command => {
                self.buffer.home(shift);
                true
            }
            "right" if command => {
                self.buffer.end(shift);
                true
            }
            "up" if command => {
                self.buffer.move_to(0, shift);
                true
            }
            "down" if command => {
                self.buffer.move_to(self.buffer.text().len(), shift);
                true
            }
            "left" if keys.modifiers.alt => {
                self.buffer.move_word_left(shift);
                true
            }
            "right" if keys.modifiers.alt => {
                self.buffer.move_word_right(shift);
                true
            }
            "left" => {
                self.buffer.move_left(shift);
                true
            }
            "right" => {
                self.buffer.move_right(shift);
                true
            }
            "up" => {
                self.buffer.move_vertical(-1, shift);
                true
            }
            "down" => {
                self.buffer.move_vertical(1, shift);
                true
            }
            "home" => {
                self.buffer.home(shift);
                true
            }
            "end" => {
                self.buffer.end(shift);
                true
            }
            "pageup" => {
                self.buffer.move_vertical(-PAGE_LINES, shift);
                true
            }
            "pagedown" => {
                self.buffer.move_vertical(PAGE_LINES, shift);
                true
            }
            // Typed characters arrive through the input handler, not here.
            _ => false,
        };
        if !handled {
            return;
        }
        cx.stop_propagation();
        self.edited(from, cx);
    }

    /// One row of the text: its number in the gutter, the text in its colors, the selection behind
    /// it, and the caret when it is on this line.
    fn row(&self, line: usize, focused: bool, entity: &Entity<Editor>) -> AnyElement {
        let range = self.buffer.line_range(line);
        let text = self.buffer.text()[range.clone()].to_owned();
        let spans = self
            .spans
            .as_ref()
            .and_then(|spans| spans.get(line))
            .cloned()
            .unwrap_or_default();
        let selection = self.buffer.selection();
        let picked = {
            let from = selection.start.max(range.start);
            let to = selection.end.min(range.end);
            (from < to).then(|| from - range.start..to - range.start)
        };
        // A selection that runs past the line's end also colors the line break, one character wide.
        let newline_picked =
            !selection.is_empty() && selection.start <= range.end && selection.end > range.end;
        let on_line = self.buffer.cursor() >= range.start && self.buffer.cursor() <= range.end;
        let caret = (focused && on_line).then(|| self.buffer.cursor() - range.start);
        let shown = if text.is_empty() {
            " ".to_owned()
        } else {
            text.clone()
        };
        let runs = line_runs(&shown, &spans);
        let advance = self.advance;
        let geometry = self.geometry.clone();
        let click = entity.clone();
        let mut row = div()
            .h(px(ROW))
            .w_full()
            .flex()
            .items_start()
            .font_family("Menlo")
            .text_size(px(FONT_SIZE))
            .line_height(px(ROW));
        if focused && on_line {
            row = row.bg(content(0.035));
        }
        row.on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, cx| {
                click.update(cx, |this, cx| {
                    window.focus(&this.focus, cx);
                    this.press(line, event);
                    cx.notify();
                });
            },
        )
        .child(
            div()
                .w(px(48.))
                .flex_shrink_0()
                .pr(px(14.))
                .text_right()
                .text_color(content(0.3))
                .child((line + 1).to_string()),
        )
        .child(
            canvas(
                move |_, window, _| {
                    window.text_system().shape_line(
                        SharedString::from(shown),
                        px(FONT_SIZE),
                        &runs,
                        None,
                    )
                },
                move |bounds, shaped: ShapedLine, window, cx| {
                    let origin = bounds.origin;
                    if let Some(range) = picked {
                        let x0 = shaped.x_for_index(range.start);
                        let mut x1 = shaped.x_for_index(range.end);
                        if newline_picked {
                            x1 += px(advance);
                        }
                        window.paint_quad(fill(
                            Bounds::new(point(origin.x + x0, origin.y), size(x1 - x0, px(ROW))),
                            theme::text_selection(),
                        ));
                    } else if newline_picked {
                        window.paint_quad(fill(
                            Bounds::new(point(origin.x, origin.y), size(px(advance), px(ROW))),
                            theme::text_selection(),
                        ));
                    }
                    let _ = shaped.paint(origin, px(ROW), TextAlign::Left, None, window, cx);
                    if let Some(at) = caret {
                        let x = shaped.x_for_index(at);
                        let rect = Bounds::new(
                            point(origin.x + x, origin.y),
                            size(px(CARET_WIDTH), px(ROW)),
                        );
                        window.paint_quad(fill(rect, caret_color()));
                        geometry.borrow_mut().caret = Some(rect);
                    }
                },
            )
            .flex_1()
            .min_w_0()
            .h(px(ROW)),
        )
        .into_any_element()
    }
}

/// The text's line endings as the editor keeps them: `\n`.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.advance = measure(window);
        let focused = self.focus.is_focused(window);
        let entity = cx.entity();
        let rows = uniform_list(
            "file-editor-lines",
            self.buffer.line_count(),
            move |range, _window, cx| {
                let editor = entity.read(cx);
                range
                    .map(|line| editor.row(line, focused, &entity))
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0();
        let mut column = div()
            .id("file-editor")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key));
        if let Some(error) = &self.error {
            column = column.child(
                div()
                    .flex_shrink_0()
                    .px(px(12.))
                    .py(px(4.))
                    .text_size(px(12.))
                    .text_color(rgb(0xe08a8a))
                    .child(error.clone()),
            );
        }
        let entity = cx.entity();
        let focus = self.focus.clone();
        let geometry = self.geometry.clone();
        // The lines' box records where the drag scrolls from; while a drag is on, the window
        // listens for the pointer, which may leave the box.
        let (list_geometry, dragging, drag_view) =
            (self.geometry.clone(), self.dragging, cx.entity());
        let lines = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .relative()
            .child(rows)
            .child(
                canvas(
                    move |bounds, _, _| {
                        list_geometry.borrow_mut().list = Some(bounds);
                    },
                    move |_, _, window, _| {
                        if !dragging {
                            return;
                        }
                        let (moved, released) = (drag_view.clone(), drag_view);
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            moved.update(cx, |this, cx| {
                                if event.pressed_button == Some(MouseButton::Left) {
                                    this.drag_to(event.position);
                                } else {
                                    this.dragging = false;
                                }
                                cx.notify();
                            });
                        });
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                released.update(cx, |this, cx| {
                                    this.dragging = false;
                                    cx.notify();
                                });
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            );
        column.child(lines).child(
            canvas(
                move |bounds, _, _| {
                    geometry.borrow_mut().bounds = Some(bounds);
                    bounds
                },
                move |bounds, _, window, cx| {
                    window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                },
            )
            .absolute()
            .size_full(),
        )
    }
}

/// One character's advance in the editor's font: the width of ten zeros, divided by ten.
fn measure(window: &Window) -> f32 {
    let shaped = window.text_system().shape_line(
        SharedString::from("0000000000"),
        px(FONT_SIZE),
        &[run(10, caret_color())],
        None,
    );
    f32::from(shaped.width) / 10.
}

fn run(len: usize, color: Hsla) -> TextRun {
    TextRun {
        len,
        font: editor_font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

fn editor_font() -> Font {
    gpui::font("Menlo")
}

/// The runs that color one line: each syntax span in its group's color, the rest in the base ink.
fn line_runs(text: &str, spans: &[(Range<usize>, Group)]) -> Vec<TextRun> {
    let base = content(0.88);
    let mut runs = Vec::new();
    let mut at = 0;
    for (range, group) in spans {
        let start = range.start.max(at);
        let end = range.end.min(text.len());
        if start >= end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        if start > at {
            runs.push(run(start - at, base));
        }
        runs.push(run(end - start, group_color(*group)));
        at = end;
    }
    if at < text.len() {
        runs.push(run(text.len() - at, base));
    }
    runs
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.buffer.text();
        let (start, end) = (byte_at(text, range.start), byte_at(text, range.end));
        *adjusted = Some(range);
        Some(text[start..end].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let text = self.buffer.text();
        let range = self.buffer.selection();
        let sel = self.buffer.sel();
        Some(UTF16Selection {
            range: utf16_len(&text[..range.start])..utf16_len(&text[..range.end]),
            reversed: sel.cursor < sel.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let text = self.buffer.text();
        self.marked
            .clone()
            .map(|range| utf16_len(&text[..range.start])..utf16_len(&text[..range.end]))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let marked = self.marked.take();
        let from = self
            .utf16_range(range)
            .or(marked)
            .unwrap_or_else(|| self.buffer.selection());
        let from_revision = self.buffer.revision();
        self.buffer.replace(from, &normalize(text));
        self.edited(from_revision, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let marked = self.marked.take();
        let from = self
            .utf16_range(range)
            .or(marked)
            .unwrap_or_else(|| self.buffer.selection());
        let from_revision = self.buffer.revision();
        let start = from.start.min(from.end);
        let new_text = normalize(new_text);
        self.buffer.replace(from, &new_text);
        self.edited(from_revision, cx);
        if !new_text.is_empty() {
            self.marked = Some(start..start + new_text.len());
        }
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(self.geometry.borrow().caret.unwrap_or(bounds))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Editor {
    /// An input method's UTF-16 range, as bytes of the text.
    fn utf16_range(&self, range: Option<Range<usize>>) -> Option<Range<usize>> {
        let text = self.buffer.text();
        range.map(|range| byte_at(text, range.start)..byte_at(text, range.end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> Buffer {
        Buffer::new(text.to_owned())
    }

    #[test]
    fn typing_inserts_at_the_caret() {
        let mut b = buffer("ac");
        b.move_to(1, false);
        b.insert("b");
        assert_eq!(b.text(), "abc");
        assert_eq!(b.cursor(), 2);
        assert!(b.selection().is_empty());
    }

    #[test]
    fn typing_over_a_selection_replaces_it() {
        let mut b = buffer("hello world");
        b.move_to(0, false);
        b.move_to(5, true);
        b.insert("bye");
        assert_eq!(b.text(), "bye world");
        assert_eq!(b.cursor(), 3);
    }

    #[test]
    fn backspace_and_delete_respect_the_caret() {
        let mut b = buffer("abcd");
        b.move_to(2, false);
        b.delete_backward();
        assert_eq!(b.text(), "acd");
        b.delete_forward();
        assert_eq!(b.text(), "ad");
        b.move_to(0, false);
        b.delete_backward();
        assert_eq!(b.text(), "ad", "nothing before the first byte");
        b.move_to(2, false);
        b.delete_forward();
        assert_eq!(b.text(), "ad", "nothing after the last byte");
    }

    #[test]
    fn backspace_removes_a_whole_accented_character() {
        let mut b = buffer("café");
        b.move_to(b.text().len(), false);
        b.delete_backward();
        assert_eq!(b.text(), "caf");
        let mut b = buffer("日本");
        b.move_to(3, false);
        b.delete_backward();
        assert_eq!(b.text(), "本");
        b.delete_forward();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn arrows_step_over_multibyte_characters() {
        let mut b = buffer("é😀x");
        b.move_to(0, false);
        b.move_right(false);
        assert_eq!(b.cursor(), 'é'.len_utf8());
        b.move_right(false);
        assert_eq!(b.cursor(), 'é'.len_utf8() + '😀'.len_utf8());
        b.move_left(false);
        assert_eq!(b.cursor(), 'é'.len_utf8());
    }

    #[test]
    fn move_to_stays_on_a_character_boundary_and_inside_the_text() {
        let mut b = buffer("é");
        b.move_to(1, false);
        assert_eq!(b.cursor(), 0);
        b.move_to(99, false);
        assert_eq!(b.cursor(), 2);
    }

    #[test]
    fn shift_arrows_grow_and_plain_arrows_collapse_the_selection() {
        let mut b = buffer("abcdef");
        b.move_to(1, false);
        b.move_right(true);
        b.move_right(true);
        assert_eq!(b.selection(), 1..3);
        assert_eq!(b.selected_text(), "bc");
        b.move_left(false);
        assert_eq!(b.cursor(), 1, "a plain Left collapses to the start");
        b.move_to(1, false);
        b.move_right(true);
        b.move_right(false);
        assert_eq!(b.cursor(), 2, "a plain Right collapses to the end");
    }

    #[test]
    fn select_all_then_typing_replaces_everything() {
        let mut b = buffer("one\ntwo");
        b.select_all();
        assert_eq!(b.selected_text(), "one\ntwo");
        b.insert("x");
        assert_eq!(b.text(), "x");
    }

    #[test]
    fn home_and_end_stay_on_the_line() {
        let mut b = buffer("first\nsecond line\nthird");
        b.move_to(9, false);
        b.home(false);
        assert_eq!(b.cursor(), 6);
        b.end(false);
        assert_eq!(b.cursor(), 17);
        b.home(true);
        assert_eq!(b.selected_text(), "second line");
    }

    #[test]
    fn undo_and_redo_walk_the_edits() {
        let mut b = buffer("");
        b.insert("a");
        b.insert("b");
        b.insert(" ");
        b.insert("c");
        assert_eq!(b.text(), "ab c");
        assert!(b.undo());
        assert_eq!(b.text(), "ab ", "the last letter alone");
        assert!(b.undo());
        assert_eq!(b.text(), "ab", "the space");
        assert!(b.undo());
        assert_eq!(b.text(), "", "the word typed before it, in one step");
        assert!(!b.undo());
        assert!(b.redo());
        assert_eq!(b.text(), "ab");
        assert!(b.redo());
        assert_eq!(b.text(), "ab ");
        assert!(b.redo());
        assert_eq!(b.text(), "ab c");
        assert!(!b.redo());
    }

    #[test]
    fn undo_restores_the_selection_and_redo_restores_the_caret() {
        let mut b = buffer("hello");
        b.move_to(0, false);
        b.move_to(5, true);
        b.insert("");
        assert_eq!(b.text(), "");
        assert!(b.undo());
        assert_eq!(b.text(), "hello");
        assert_eq!(b.selection(), 0..5);
        assert!(b.redo());
        assert_eq!(b.text(), "");
        assert_eq!(b.cursor(), 0);
    }

    #[test]
    fn a_new_edit_after_undo_drops_the_redo_branch() {
        let mut b = buffer("");
        b.insert("one");
        b.insert(" ");
        b.undo();
        b.insert("2");
        assert!(!b.redo());
        assert_eq!(b.text(), "one2");
    }

    #[test]
    fn consecutive_backspaces_undo_together() {
        let mut b = buffer("abcdef");
        b.move_to(6, false);
        b.delete_backward();
        b.delete_backward();
        b.delete_backward();
        assert_eq!(b.text(), "abc");
        assert!(b.undo());
        assert_eq!(b.text(), "abcdef");
        assert!(!b.undo());
    }

    #[test]
    fn consecutive_forward_deletes_undo_together() {
        let mut b = buffer("abcdef");
        b.move_to(1, false);
        b.delete_forward();
        b.delete_forward();
        assert_eq!(b.text(), "adef");
        assert!(b.undo());
        assert_eq!(b.text(), "abcdef");
    }

    #[test]
    fn a_space_starts_a_new_undo_step() {
        let mut b = buffer("");
        b.insert("hi");
        b.insert(" ");
        b.insert("there");
        assert!(b.undo());
        assert_eq!(b.text(), "hi ");
        assert!(b.undo());
        assert_eq!(b.text(), "hi");
    }

    #[test]
    fn modified_follows_the_saved_point() {
        let mut b = buffer("x");
        assert!(!b.is_modified());
        b.insert("y");
        assert!(b.is_modified());
        let revision = b.revision();
        assert!(b.mark_saved(revision));
        assert!(!b.is_modified());
        b.undo();
        assert!(b.is_modified(), "before the save point is a change");
        b.redo();
        assert!(!b.is_modified(), "and forward again is the saved text");
    }

    #[test]
    fn a_save_is_ignored_once_the_text_has_moved_on() {
        let mut b = buffer("");
        b.insert("a");
        let saved = b.revision();
        // Typing continues while the write is in flight; the letters merge into one edit.
        b.insert("b");
        assert!(!b.mark_saved(saved), "the text moved on since the snapshot");
        assert!(b.is_modified());
        let now = b.revision();
        assert!(b.mark_saved(now));
        assert!(!b.is_modified());
    }

    #[test]
    fn typing_into_the_saved_edit_makes_the_text_modified_again() {
        let mut b = buffer("");
        b.insert("a");
        let revision = b.revision();
        assert!(b.mark_saved(revision));
        // The next letter extends the saved edit, so the saved state can no longer be restored.
        b.insert("b");
        assert!(b.is_modified());
        b.undo();
        assert_eq!(b.text(), "");
        assert!(b.is_modified(), "empty is not what was saved");
    }

    #[test]
    fn line_lookup_without_wrapping() {
        let b = buffer("ab\n\ncdef\n");
        assert_eq!(b.line_count(), 4);
        assert_eq!(b.line_range(0), 0..2);
        assert_eq!(b.line_range(1), 3..3, "an empty line");
        assert_eq!(b.line_range(2), 4..8);
        assert_eq!(
            b.line_range(3),
            9..9,
            "the empty line after the last newline"
        );
        assert_eq!(&b.text()[b.line_range(2)], "cdef");
        assert_eq!(b.line_of(0), 0);
        assert_eq!(b.line_of(2), 0, "the newline belongs to its line");
        assert_eq!(b.line_of(3), 1);
        assert_eq!(b.line_of(8), 2);
        assert_eq!(b.line_of(9), 3);
        assert_eq!(b.line_of(99), 3, "past the end is the last line");
    }

    #[test]
    fn a_line_past_the_last_one_is_the_last_one() {
        let b = buffer("one");
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.line_range(5), 0..3);
        assert_eq!(buffer("").line_count(), 1);
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        // é (2 bytes), 😀 (4 bytes), x, newline, then "abc" on the next line.
        let b = buffer("é😀x\nabc");
        assert_eq!(b.column_of(6), 2, "the x starts after two characters");
        assert_eq!(b.column_of(8), 0, "the next line starts at column 0");
        assert_eq!(b.byte_of(0, 2), 6);
        assert_eq!(b.byte_of(0, 99), 7, "clamped to the end of its line");
        assert_eq!(
            b.byte_of(1, 99),
            b.text().len(),
            "the last line ends the text"
        );
    }

    #[test]
    fn up_and_down_keep_the_column_across_short_lines() {
        let mut b = buffer("long line here\nab\nlonger again");
        b.move_to(10, false);
        b.move_vertical(1, false);
        assert_eq!(b.cursor(), 17, "the short line ends first");
        b.move_vertical(1, false);
        assert_eq!(b.cursor(), 28, "back to column 10 on the next line");
        b.move_vertical(-5, false);
        assert_eq!(b.line_of(b.cursor()), 0);
    }

    #[test]
    fn vertical_moves_count_characters() {
        let mut b = buffer("é😀ab\nxyzw");
        b.move_to(0, false);
        b.move_right(false);
        b.move_right(false);
        assert_eq!(b.cursor(), 6);
        b.move_vertical(1, false);
        assert_eq!(b.cursor(), 9 + 2, "column two of the next line");
    }

    #[test]
    fn a_range_inside_a_character_is_widened_to_it() {
        let mut b = buffer("héllo");
        b.replace(2..3, "e");
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn empty_edits_change_nothing() {
        let mut b = buffer("keep");
        let revision = b.revision();
        b.replace(1..1, "");
        assert_eq!(b.revision(), revision);
        assert_eq!(b.text(), "keep");
    }

    #[test]
    fn enter_splits_the_line_at_the_caret() {
        let mut b = buffer("ab");
        b.move_to(1, false);
        b.insert("\n");
        assert_eq!(b.line_count(), 2);
        assert_eq!(&b.text()[b.line_range(0)], "a");
        assert_eq!(&b.text()[b.line_range(1)], "b");
        assert_eq!(b.cursor(), 2);
    }

    #[test]
    fn pasted_line_endings_become_newlines() {
        assert_eq!(normalize("a\r\nb\rc\n"), "a\nb\nc\n");
    }

    #[test]
    fn option_arrows_move_by_word() {
        let mut b = buffer("one two  three");
        b.move_word_right(false);
        assert_eq!(b.cursor(), 3, "the end of the first word");
        b.move_word_right(false);
        assert_eq!(b.cursor(), 7);
        b.move_word_right(false);
        assert_eq!(b.cursor(), 14, "the end of the last word");
        b.move_word_left(false);
        assert_eq!(b.cursor(), 9, "the start of three, past the two spaces");
        b.move_word_left(false);
        assert_eq!(b.cursor(), 4);
        b.move_word_left(false);
        assert_eq!(b.cursor(), 0);
        b.move_word_left(false);
        assert_eq!(b.cursor(), 0, "nothing before the first word");
    }

    #[test]
    fn option_arrows_with_shift_extend_the_selection() {
        let mut b = buffer("alpha beta");
        b.move_to(0, false);
        b.move_word_right(true);
        assert_eq!(b.selected_text(), "alpha");
        b.move_word_right(true);
        assert_eq!(b.selected_text(), "alpha beta");
        b.move_word_left(true);
        assert_eq!(b.selection(), 0..6);
    }

    #[test]
    fn word_jumps_treat_marks_and_punctuation_as_their_own_runs() {
        let b = buffer("foo.bar(x)");
        assert_eq!(b.word_right(0), 3, "foo stops before the dot");
        assert_eq!(b.word_right(3), 4, "the dot is a run of its own");
        assert_eq!(b.word_right(4), 7);
        assert_eq!(b.word_right(7), 8, "the parenthesis");
        assert_eq!(b.word_left(10), 9, "the closing parenthesis is a run");
        assert_eq!(b.word_left(4), 3);
        assert_eq!(b.word_left(3), 0);
    }

    #[test]
    fn word_jumps_follow_unicode_letters_and_skip_emoji_as_punctuation() {
        // café (5 bytes), a space, olé (4 bytes), then an emoji and x.
        let b = buffer("café olé😀x");
        assert_eq!(b.word_right(0), 5, "é is a letter");
        assert_eq!(b.word_right(5), 10, "olé ends before the emoji");
        assert_eq!(b.word_right(10), 14, "the emoji is a run of its own");
        assert_eq!(b.word_left(14), 10);
        assert_eq!(b.word_left(10), 6, "back over olé");
        let cjk = buffer("日本語 テスト");
        assert_eq!(cjk.word_right(0), 9, "a run of letters, however written");
        assert_eq!(cjk.word_left(cjk.text().len()), 10);
    }

    #[test]
    fn word_jumps_never_stop_inside_a_character() {
        let b = buffer("é😀");
        assert_eq!(b.word_right(1), 2, "a byte inside é is floored to its start, then the word is é");
        // Byte 3 is inside the emoji, so it floors to 2, which is the start of the emoji.
        assert_eq!(b.word_left(3), 0, "from the emoji's start, back over the é before it");
        assert_eq!(b.word_right(99), b.text().len());
        assert_eq!(b.word_left(99), 2, "past the end starts from the end");
    }

    #[test]
    fn option_backspace_deletes_a_word_back() {
        let mut b = buffer("hello big world");
        b.move_to(15, false);
        b.delete_word_backward();
        assert_eq!(b.text(), "hello big ");
        b.delete_word_backward();
        assert_eq!(b.text(), "hello ");
        b.delete_word_backward();
        assert_eq!(b.text(), "");
        b.delete_word_backward();
        assert_eq!(b.text(), "", "nothing before the caret");
    }

    #[test]
    fn option_backspace_over_a_selection_deletes_the_selection_only() {
        let mut b = buffer("keep this word");
        b.move_to(5, false);
        b.move_to(9, true);
        b.delete_word_backward();
        assert_eq!(b.text(), "keep  word", "the selected 'this' goes, the spaces around stay");
        assert!(b.undo());
        assert_eq!(b.text(), "keep this word");
        assert_eq!(b.selection(), 5..9);
    }

    #[test]
    fn option_backspace_is_one_undo_step() {
        let mut b = buffer("one two");
        b.move_to(7, false);
        b.delete_word_backward();
        assert_eq!(b.text(), "one ");
        assert!(b.undo());
        assert_eq!(b.text(), "one two");
        assert!(!b.undo());
    }

    #[test]
    fn a_double_click_selects_the_word_under_it() {
        let b = buffer("alpha beta_2 gamma");
        assert_eq!(b.word_at(7), 6..12, "inside beta_2, underscore and digits too");
        assert_eq!(b.word_at(6), 6..12, "at its first byte");
        assert_eq!(b.word_at(12), 6..12, "at its end, the word before wins over the space");
        assert_eq!(b.word_at(0), 0..5);
        assert_eq!(b.word_at(13), 13..18, "the next word starts at its first byte");
    }

    #[test]
    fn a_double_click_on_a_space_selects_the_run_of_spaces() {
        let b = buffer("a   b");
        assert_eq!(b.word_at(2), 1..4);
    }

    #[test]
    fn a_double_click_at_the_end_of_a_line_takes_the_word_before_it() {
        let b = buffer("ab\ncd");
        assert_eq!(b.word_at(2), 0..2, "the newline is never part of a word");
        assert_eq!(b.word_at(5), 3..5, "the end of the text");
        let empty = buffer("");
        assert_eq!(empty.word_at(0), 0..0);
    }

    #[test]
    fn a_double_click_selects_a_multibyte_word_in_whole() {
        let mut b = buffer("naïve café");
        b.select_word_at(9);
        assert_eq!(b.selected_text(), "café");
        b.select_word_at(4);
        assert_eq!(b.selected_text(), "naïve", "ï is two bytes, the boundary holds");
    }

    #[test]
    fn a_triple_click_selects_the_whole_line_with_its_newline() {
        let mut b = buffer("one\ntwo\nthree");
        b.select_line_at(5);
        assert_eq!(b.selected_text(), "two\n");
        assert_eq!(b.selection(), 4..8);
        b.select_line_at(9);
        assert_eq!(b.selected_text(), "three", "the last line has no newline to take");
    }

    #[test]
    fn a_selected_line_is_replaced_whole_by_typing() {
        let mut b = buffer("one\ntwo\nthree");
        b.select_line_at(5);
        b.insert("");
        assert_eq!(b.text(), "one\nthree");
    }

    #[test]
    fn line_runs_cover_the_whole_line_once() {
        let spans = vec![(0..2, Group::Keyword), (3..5, Group::String)];
        let runs = line_runs("fn x\"s\"", &spans);
        let total: usize = runs.iter().map(|run| run.len).sum();
        assert_eq!(total, "fn x\"s\"".len());
        assert_eq!(runs.len(), 4, "keyword, gap, string, rest");
    }
}
