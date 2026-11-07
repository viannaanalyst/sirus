//! Selecting the text of a reply or a user message in the transcript (after
//! `transcript/selection.rs`, MIT).
//!
//! The markdown renderer hands each text fragment (paragraph, heading, list item, code block,
//! table cell) to `wrap`, which numbers them in render order within one reply. A position is
//! `(fragment, byte)`, so a selection survives scrolling and re-layout, and extracting the text
//! never depends on what is on screen: `record` renders the reply once more into plain strings.
//! Press, drag and release live in `State`; the fragments' layouts are re-registered every frame
//! so a drag can find the one under the pointer. Nothing here animates or redraws while idle.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;
use gpui::{
    AnyElement, App, Bounds, CursorStyle, Element, ElementId, GlobalElementId, InspectorElementId,
    InteractiveText, LayoutId, Point, SharedString, StyledText, TextLayout, size,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

/// One selectable message: a user bubble or one stretch of an answer (the turn's text beat at
/// `beat`), in one turn of one conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Reply {
    pub conversation: usize,
    pub turn: usize,
    pub user: bool,
    pub beat: usize,
}

/// (fragment number, byte in the fragment's text); tuples order the way the text reads.
type Pos = (usize, usize);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Gran {
    Char,
    Word,
    Line,
}

struct Drag {
    origin: Point<Pixels>,
    anchor: (Pos, Pos),
    gran: Gran,
    moved: bool,
    clicks: usize,
    /// The web link under the press, if any: it opens when the press ends without a drag.
    link: Option<String>,
}

struct Frag {
    layout: TextLayout,
    text: SharedString,
}

#[derive(Default)]
struct Inner {
    reply: Option<Reply>,
    anchor: Option<Pos>,
    head: Option<Pos>,
    drag: Option<Drag>,
    pointer: Point<Pixels>,
    /// When the action menu opened.
    menu: Option<Instant>,
    /// A fragment took this press, so the root handler must not clear the selection.
    claimed: bool,
    /// The link a plain click ended on, for the window to open (see `take_link`).
    opened: Option<String>,
    frags: HashMap<(Reply, usize), (u64, Frag)>,
    frame: u64,
}

#[derive(Clone, Default)]
pub(crate) struct State(Rc<RefCell<Inner>>);

// ---- Range logic (no window needed) -------------------------------------------------------

fn ordered(a: Pos, b: Pos) -> (Pos, Pos) {
    if a <= b { (a, b) } else { (b, a) }
}

fn is_mark(c: char) -> bool {
    matches!(c, '\u{300}'..='\u{36f}' | '\u{200d}' | '\u{fe0f}')
}

/// The nearest char boundary at or before `byte`, not between a letter and its combining marks.
fn snap(text: &str, byte: usize) -> usize {
    let mut b = byte.min(text.len());
    while !text.is_char_boundary(b) {
        b -= 1;
    }
    while b > 0 && text[b..].chars().next().is_some_and(is_mark) {
        b = text[..b].char_indices().next_back().map_or(0, |(i, _)| i);
    }
    b
}

#[derive(PartialEq)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' || is_mark(c) {
        Class::Word
    } else if c.is_whitespace() && c != '\n' {
        Class::Space
    } else {
        Class::Other
    }
}

/// The word (or run of spaces, or single symbol) around `byte`.
fn word(text: &str, byte: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let at = if byte >= text.len() {
        text.char_indices().next_back().map_or(0, |(i, _)| i)
    } else {
        snap(text, byte)
    };
    let Some(c) = text[at..].chars().next() else {
        return at..at;
    };
    let kind = class(c);
    if kind == Class::Other {
        return at..at + c.len_utf8();
    }
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| class(*c) == kind)
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| class(*c) != kind)
        .map_or(text.len(), |(i, _)| at + i);
    start..end
}

/// The line (between newlines) around `byte`.
fn line(text: &str, byte: usize) -> Range<usize> {
    let byte = snap(text, byte);
    let start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    let end = text[byte..].find('\n').map_or(text.len(), |i| byte + i);
    start..end
}

fn unit(text: &str, byte: usize, gran: Gran) -> Range<usize> {
    let byte = snap(text, byte);
    match gran {
        Gran::Char => byte..byte,
        Gran::Word => word(text, byte),
        Gran::Line => line(text, byte),
    }
}

/// The selected bytes of fragment `seq` (of `len` bytes), whichever way the selection was made.
fn slice(a: Pos, b: Pos, seq: usize, len: usize) -> Range<usize> {
    let (a, b) = ordered(a, b);
    if seq < a.0 || seq > b.0 {
        return 0..0;
    }
    let start = if seq == a.0 { a.1.min(len) } else { 0 };
    let end = if seq == b.0 { b.1.min(len) } else { len };
    start.min(end)..end
}

/// Where a drag at `byte` of fragment `seq` leaves the selection, as (anchor, head).
fn extend(anchor: (Pos, Pos), gran: Gran, seq: usize, text: &str, byte: usize) -> (Pos, Pos) {
    let r = unit(text, byte, gran);
    let (start, end) = ((seq, r.start), (seq, r.end));
    if start < anchor.0 {
        (anchor.1, start)
    } else {
        (anchor.0, end)
    }
}

/// Ends a recorded fragment that continues the line of the next one.
const GLUE: char = '\u{e000}';

/// The text between two positions over all of a reply's fragments, one per line.
fn extract(texts: &[String], a: Pos, b: Pos) -> Option<String> {
    let (a, b) = ordered(a, b);
    if a == b || texts.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for seq in a.0..=b.0.min(texts.len() - 1) {
        let text = &texts[seq];
        let r = slice(a, b, seq, text.len());
        let (s, e) = (snap(text, r.start), snap(text, r.end));
        if s < e {
            parts.push(text[s..e].to_owned());
        }
    }
    // A piece of a flowing paragraph reads on into the next; anything else starts a new line.
    let mut joined = String::new();
    for (at, part) in parts.iter().enumerate() {
        match part.strip_suffix(GLUE) {
            Some(glued) => joined.push_str(glued),
            None => {
                joined.push_str(part);
                if at + 1 < parts.len() {
                    joined.push('\n');
                }
            }
        }
    }
    let joined = joined.trim().to_owned();
    (!joined.is_empty()).then_some(joined)
}

// ---- The selection state ------------------------------------------------------------------

impl State {
    /// Called once per shell render: forgets fragments laid out before the previous frame.
    pub(crate) fn begin_frame(&self) {
        let mut s = self.0.borrow_mut();
        s.frame += 1;
        let frame = s.frame;
        s.frags.retain(|_, (stamp, _)| *stamp + 1 >= frame);
    }

    pub(crate) fn clear(&self) {
        let mut s = self.0.borrow_mut();
        s.reply = None;
        s.anchor = None;
        s.head = None;
        s.drag = None;
        s.menu = None;
    }

    /// Whether anything is selected, dragged or offered.
    pub(crate) fn active(&self) -> bool {
        let s = self.0.borrow();
        s.reply.is_some() || s.drag.is_some() || s.menu.is_some()
    }

    #[cfg(test)]
    pub(crate) fn has_selection(&self) -> bool {
        let s = self.0.borrow();
        s.anchor.is_some() && s.anchor != s.head
    }

    pub(crate) fn reply(&self) -> Option<Reply> {
        self.0.borrow().reply
    }

    pub(crate) fn dragging(&self) -> bool {
        self.0.borrow().drag.is_some()
    }

    /// The root's press handler: clears unless a fragment took this very press.
    pub(crate) fn clear_unless_claimed(&self) -> bool {
        let claimed = std::mem::take(&mut self.0.borrow_mut().claimed);
        if claimed || !self.active() {
            return false;
        }
        self.clear();
        true
    }

    pub(crate) fn menu_since(&self) -> Option<Instant> {
        self.0.borrow().menu
    }

    fn press(
        &self,
        reply: Reply,
        seq: usize,
        text: &str,
        byte: usize,
        clicks: usize,
        shift: bool,
        at: Point<Pixels>,
    ) {
        let mut s = self.0.borrow_mut();
        s.claimed = true;
        let gran = match clicks {
            0 | 1 => Gran::Char,
            2 => Gran::Word,
            _ => Gran::Line,
        };
        let r = unit(text, byte, gran);
        let (start, end) = ((seq, r.start), (seq, r.end));
        let anchor = match s.anchor {
            Some(a) if shift && s.reply == Some(reply) => (a, a),
            _ => (start, end),
        };
        s.anchor = Some(anchor.0);
        s.head = Some(if start < anchor.0 { start } else { end });
        s.reply = Some(reply);
        s.menu = None;
        s.opened = None;
        s.pointer = at;
        s.drag = Some(Drag {
            origin: at,
            anchor,
            gran,
            moved: false,
            clicks: clicks.max(1),
            link: None,
        });
    }

    /// Says which web link the press just made is on (called right after `press`).
    fn note_link(&self, link: Option<String>) {
        if let Some(drag) = self.0.borrow_mut().drag.as_mut() {
            drag.link = link;
        }
    }

    /// Moves the head to the fragment nearest the pointer. Says whether the selection changed.
    pub(crate) fn drag_to(&self, at: Point<Pixels>) -> bool {
        let mut s = self.0.borrow_mut();
        s.pointer = at;
        let Some(reply) = s.reply else { return false };
        let Some(drag) = s.drag.as_mut() else {
            return false;
        };
        if !drag.moved {
            let d = at - drag.origin;
            if d.x.abs().max(d.y.abs()) < px(3.) {
                return false;
            }
            drag.moved = true;
        }
        let (anchor, gran) = (drag.anchor, drag.gran);
        let distance = |layout: &TextLayout| {
            let b = layout.bounds();
            let dx = (b.left() - at.x).max(px(0.)).max(at.x - b.right());
            let dy = (b.top() - at.y).max(px(0.)).max(at.y - b.bottom());
            f32::from(dy) * 10000. + f32::from(dx)
        };
        let hit = s
            .frags
            .iter()
            .filter(|((r, _), _)| *r == reply)
            .min_by(|a, b| distance(&a.1.1.layout).total_cmp(&distance(&b.1.1.layout)));
        let Some((&(_, seq), (_, frag))) = hit else {
            return false;
        };
        let byte = frag.layout.index_for_position(at).unwrap_or_else(|i| i);
        let (a, h) = extend(anchor, gran, seq, &frag.text, byte);
        if s.anchor == Some(a) && s.head == Some(h) {
            return false;
        }
        s.anchor = Some(a);
        s.head = Some(h);
        true
    }

    /// The mouse went up: ends the drag and opens the menu if something is selected.
    pub(crate) fn release(&self) -> bool {
        let mut s = self.0.borrow_mut();
        let Some(drag) = s.drag.take() else {
            return false;
        };
        if s.anchor.is_some() && s.anchor != s.head {
            s.menu = Some(Instant::now());
        }
        // A single click that did not drag opens the link it was pressed on.
        if !drag.moved && drag.clicks == 1 {
            s.opened = drag.link;
        }
        true
    }

    /// The web link a plain click just ended on, once: the window opens it with `open_url`.
    pub(crate) fn take_link(&self) -> Option<String> {
        self.0.borrow_mut().opened.take()
    }

    pub(crate) fn stop_drag(&self) {
        self.0.borrow_mut().drag = None;
    }

    /// Pixels to scroll this frame while the pointer drags past the list's top or bottom edge
    /// (positive scrolls down), or 0.
    pub(crate) fn autoscroll(&self, viewport: Bounds<Pixels>) -> f32 {
        let s = self.0.borrow();
        if !s.drag.as_ref().is_some_and(|d| d.moved) {
            return 0.;
        }
        let (top, bottom) = (viewport.top(), viewport.bottom());
        let over = if s.pointer.y < top {
            -f32::from(top - s.pointer.y)
        } else if s.pointer.y > bottom {
            f32::from(s.pointer.y - bottom)
        } else {
            return 0.;
        };
        (over * 0.4).clamp(-48., 48.) + over.signum() * 2.
    }

    pub(crate) fn select_all(&self, reply: Reply, texts: &[String]) {
        let Some(last) = texts.last() else { return };
        let mut s = self.0.borrow_mut();
        s.reply = Some(reply);
        s.anchor = Some((0, 0));
        s.head = Some((texts.len() - 1, last.len()));
        s.drag = None;
        s.menu = Some(Instant::now());
    }

    fn range(&self, reply: Reply, seq: usize, len: usize) -> Range<usize> {
        let s = self.0.borrow();
        match (s.reply, s.anchor, s.head) {
            (Some(r), Some(a), Some(h)) if r == reply => slice(a, h, seq, len),
            _ => 0..0,
        }
    }

    /// The selected text, given the reply's fragments in render order.
    pub(crate) fn text(&self, texts: &[String]) -> Option<String> {
        let s = self.0.borrow();
        extract(texts, s.anchor?, s.head?)
    }

    /// The top centre of the selection's first line, in window coordinates, if it is on screen.
    pub(crate) fn menu_anchor(&self) -> Option<Point<Pixels>> {
        let s = self.0.borrow();
        let reply = s.reply?;
        let (a, b) = ordered(s.anchor?, s.head?);
        // The first fragment of the selection that is laid out.
        let (seq, (_, frag)) =
            (a.0..=b.0).find_map(|seq| s.frags.get(&(reply, seq)).map(|f| (seq, f)))?;
        let from = if seq == a.0 {
            a.1.min(frag.text.len())
        } else {
            0
        };
        let top = frag.layout.position_for_index(from)?;
        let right = (seq == b.0)
            .then(|| frag.layout.position_for_index(b.1.min(frag.text.len())))
            .flatten()
            .filter(|p| p.y == top.y)
            .map_or(frag.layout.bounds().right(), |p| p.x);
        Some(gpui::point((top.x + right) / 2., top.y))
    }
}

// ---- Hooking the markdown renderer ---------------------------------------------------------

enum Mode {
    Draw,
    Record(Vec<String>),
}

struct Scope {
    state: State,
    reply: Reply,
    next: usize,
    mode: Mode,
}

thread_local! {
    static SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

fn within<R>(scope: Scope, f: impl FnOnce() -> R) -> (R, Scope) {
    let previous = SCOPE.with_borrow_mut(|s| s.replace(scope));
    let out = f();
    let scope = SCOPE.with_borrow_mut(|s| std::mem::replace(s, previous));
    (out, scope.expect("scope set above"))
}

/// Runs `f` (which renders markdown) so its text fragments belong to `reply`.
pub(crate) fn scoped<R>(state: &State, reply: Reply, f: impl FnOnce() -> R) -> R {
    within(
        Scope {
            state: state.clone(),
            reply,
            next: 0,
            mode: Mode::Draw,
        },
        f,
    )
    .0
}

/// Runs `f` and returns the text of every fragment it rendered, in order.
pub(crate) fn record(f: impl FnOnce()) -> Vec<String> {
    let scope = Scope {
        state: State::default(),
        reply: Reply {
            conversation: 0,
            turn: 0,
            user: false,
            beat: 0,
        },
        next: 0,
        mode: Mode::Record(Vec::new()),
    };
    match within(scope, f).1.mode {
        Mode::Record(texts) => texts,
        Mode::Draw => Vec::new(),
    }
}

/// A user message made selectable.
pub(crate) fn user_text(
    state: &State,
    reply: Reply,
    text: String,
    styled: StyledText,
) -> AnyElement {
    scoped(state, reply, || wrap(styled, text.into()))
}

/// Makes one text fragment selectable, inside a `scoped` render; outside one, it is left alone.
pub(crate) fn wrap(styled: StyledText, text: SharedString) -> AnyElement {
    wrap_links(styled, text, Vec::new())
}

/// `wrap` for a fragment with web links: `links` are the byte ranges and the addresses they open.
/// A link shows a pointing hand; a plain click on it opens it (after the root's release), and a
/// press that drags selects text instead.
pub(crate) fn wrap_links(
    styled: StyledText,
    text: SharedString,
    links: Vec<(Range<usize>, String)>,
) -> AnyElement {
    wrap_piece(styled, text, links, false)
}

/// A fragment that may be one piece of a flowing paragraph: with `glue`, copying carries on into
/// the next fragment on the same line.
pub(crate) fn wrap_piece(
    styled: StyledText,
    text: SharedString,
    links: Vec<(Range<usize>, String)>,
    glue: bool,
) -> AnyElement {
    let claimed = SCOPE.with_borrow_mut(|scope| {
        let scope = scope.as_mut()?;
        let seq = scope.next;
        scope.next += 1;
        match &mut scope.mode {
            Mode::Record(texts) => {
                let mut recorded = text.to_string();
                if glue {
                    recorded.push(GLUE);
                }
                texts.push(recorded);
                None
            }
            Mode::Draw => Some((scope.state.clone(), scope.reply, seq)),
        }
    });
    let layout = styled.layout().clone();
    let Some((state, reply, seq)) = claimed else {
        // Outside a selectable reply: a link still opens on click.
        if links.is_empty() {
            return styled.into_any_element();
        }
        let urls: Vec<String> = links.iter().map(|(_, url)| url.clone()).collect();
        let ranges = links.into_iter().map(|(range, _)| range).collect();
        let id = format!("link-{:x}", fingerprint(&text));
        return InteractiveText::new(SharedString::from(id), styled)
            .on_click(ranges, move |ix, _, cx| cx.open_url(&urls[ix]))
            .into_any_element();
    };
    let press = (layout.clone(), text.clone(), state.clone(), links.clone());
    let child = if links.is_empty() {
        styled.into_any_element()
    } else {
        // The pointing hand comes from InteractiveText; the click itself is the root's (see `take_link`).
        let ranges = links.iter().map(|(range, _)| range.clone()).collect();
        let id = format!(
            "sel-link-{}-{}-{}-{seq}",
            reply.conversation, reply.turn, reply.user
        );
        InteractiveText::new(SharedString::from(id), styled)
            .on_click(ranges, |_, _, _| {})
            .into_any_element()
    };
    div()
        .relative()
        .cursor(CursorStyle::IBeam)
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            let (layout, text, state, links) = &press;
            let byte = layout
                .index_for_position(event.position)
                .unwrap_or_else(|i| i);
            state.press(
                reply,
                seq,
                text,
                byte,
                event.click_count,
                event.modifiers.shift,
                event.position,
            );
            state.note_link(
                links
                    .iter()
                    .find(|(range, _)| range.contains(&byte))
                    .map(|(_, url)| url.clone()),
            );
            window.refresh();
        })
        .child(Selected {
            state,
            reply,
            seq,
            text,
            layout,
            child,
        })
        .into_any_element()
}

/// A stable number for a fragment's text, so an unselectable link keeps its element state.
fn fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Paints the highlight under its text and registers the layout for the drag.
struct Selected {
    state: State,
    reply: Reply,
    seq: usize,
    text: SharedString,
    layout: TextLayout,
    child: AnyElement,
}

impl IntoElement for Selected {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Selected {
    type RequestLayoutState = ();
    type PrepaintState = ();
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
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
        let mut s = self.state.0.borrow_mut();
        let frame = s.frame;
        s.frags.insert(
            (self.reply, self.seq),
            (
                frame,
                Frag {
                    layout: self.layout.clone(),
                    text: self.text.clone(),
                },
            ),
        );
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let selected = self.state.range(self.reply, self.seq, self.text.len());
        if !selected.is_empty() {
            let bounds = self.layout.bounds();
            let height = self.layout.line_height();
            if let (Some(a), Some(b)) = (
                self.layout.position_for_index(selected.start),
                self.layout.position_for_index(selected.end),
            ) {
                let mut y = a.y;
                while y <= b.y {
                    let left = if y == a.y { a.x } else { bounds.left() };
                    let right = if y == b.y { b.x } else { bounds.right() };
                    if right > left {
                        window.paint_quad(gpui::fill(
                            Bounds::new(gpui::point(left, y), size(right - left, height)),
                            crate::theme::text_selection(),
                        ));
                    }
                    y += height;
                }
            }
        }
        self.child.paint(window, cx);
    }
}

// ---- The window's side: keys, copy, menu ---------------------------------------------------

const MENU_OPEN: f32 = 0.12;
const MENU_WIDTH: f32 = 164.;

impl Shell {
    /// The fragments of a reply in render order, from its current source.
    fn reply_texts(&self, reply: Reply) -> Vec<String> {
        let Some(turn) = self
            .conversations
            .get(reply.conversation)
            .and_then(|c| c.turns.get(reply.turn))
        else {
            return Vec::new();
        };
        if reply.user {
            vec![turn.user.clone()]
        } else {
            // The same rendering as on screen: drawn diagrams and formulas change which pieces of
            // text exist, and a selection is addressed by piece.
            let conversation = &self.conversations[reply.conversation];
            let rich = crate::reply_view::Rich {
                session: conversation.session_id.clone(),
                cwd: conversation.worktree.clone(),
                streaming: turn.live,
                act: std::rc::Rc::new(|_, _, _| {}),
            };
            let answer = match turn.beats.get(reply.beat) {
                Some(data::Beat::Text(range)) => turn
                    .answer
                    .get(range.start..range.end.min(turn.answer.len()))
                    .unwrap_or("")
                    .to_owned(),
                _ => String::new(),
            };
            record(|| {
                crate::markdown::render_rich(&answer, &rich);
            })
        }
    }

    /// The selected text of the open conversation's reply.
    pub(super) fn selected_text(&self) -> Option<String> {
        let reply = self.select.reply()?;
        if reply.conversation != self.active_conversation() {
            return None;
        }
        self.select.text(&self.reply_texts(reply))
    }

    /// ⌘C over a selected reply; false if there is none, so the composer's copy goes on.
    pub(super) fn copy_selected_text(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.selected_text() else {
            return false;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        true
    }

    /// ⌘A after a click in a reply selects that reply; false if the focus is elsewhere.
    pub(super) fn select_reply_text(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(reply) = self.select.reply() else {
            return false;
        };
        if reply.conversation != self.active_conversation() || self.grabs_typing() {
            return false;
        }
        let texts = self.reply_texts(reply);
        if texts.is_empty() {
            return false;
        }
        self.select.select_all(reply, &texts);
        cx.notify();
        true
    }

    /// Per frame, before the list is built: a drag past the edge scrolls and keeps the head
    /// under the pointer. Says whether it needs another frame.
    pub(super) fn select_autoscroll(&mut self, list: &gpui::ListState) -> bool {
        let delta = self.select.autoscroll(list.viewport_bounds());
        if delta == 0. {
            return false;
        }
        let at = self.select.0.borrow().pointer;
        self.select.drag_to(at);
        list.pause_following_tail();
        list.scroll_by(px(delta));
        true
    }

    /// Puts a selected passage at the end of the composer's draft, on its own paragraph, with the
    /// caret on a new one below it.
    fn add_to_chat(&mut self, text: &str) {
        let draft = self.draft();
        let separator = if draft.text.is_empty() || draft.text.ends_with("\n\n") {
            ""
        } else if draft.text.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        draft.text = format!("{}{separator}{}\n\n", draft.text, text.trim());
        draft.cursor = draft.text.len();
        draft.anchor = draft.cursor;
    }

    pub(super) fn select_menu_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let since = self.select.menu_since()?;
        let text = self.selected_text()?;
        let anchor = self.select.menu_anchor()?;
        let shown = if cx.reduce_motion() {
            1.
        } else {
            let t = now.duration_since(since).as_secs_f32() / MENU_OPEN;
            if t < 1. {
                *moving = true;
            }
            eased(t.min(1.), Curve::Expo)
        };
        let track = crate::hover_track::list("selection-menu");
        let item = |id: &'static str, glyph: &'static str, label: &'static str| {
            div()
                .id(id)
                .h(px(26.))
                .px(px(7.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(7.))
                .text_size(px(12.5))
                .text_color(content(0.92))
                .cursor_pointer()
                .child(icon(glyph, 13.).text_color(content(0.65)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(crate::i18n::t(label)),
                )
        };
        let copy = text.clone();
        let ask = text;
        const HEIGHT: f32 = 60.;
        let left = (f32::from(anchor.x) - MENU_WIDTH / 2.)
            .min(self.viewport_width - MENU_WIDTH - 8.)
            .max(8.);
        let above = f32::from(anchor.y) - HEIGHT - 8.;
        let top = if above < 8. {
            f32::from(anchor.y) + 28.
        } else {
            above
        };
        Some(
            div()
                .id("transcript-selection-menu")
                .occlude()
                .absolute()
                .left(px(left))
                .top(px(top + (1. - shown) * 4.))
                .w(px(MENU_WIDTH))
                .p(px(3.))
                .flex()
                .flex_col()
                .opacity(shown)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(track.pill())
                .child(track.row(
                    item("selection-copy", "copy", "Copiar").on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()));
                            this.select.clear();
                            cx.notify();
                        },
                    )),
                    0,
                    false,
                ))
                .child(track.row(
                    item("selection-add-chat", "comment-add", "Adicionar ao chat").on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.select.clear();
                            this.add_to_chat(&ask);
                            this.keep_typing_focus(window, cx);
                            cx.notify();
                        }),
                    ),
                    1,
                    false,
                ))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: Reply = Reply {
        conversation: 0,
        turn: 1,
        user: false,
        beat: 0,
    };

    fn at(x: f32, y: f32) -> Point<Pixels> {
        gpui::point(px(x), px(y))
    }

    #[test]
    fn slice_is_direction_independent() {
        let forward = slice((0, 3), (2, 4), 0, 10);
        assert_eq!(forward, 3..10);
        assert_eq!(slice((2, 4), (0, 3), 0, 10), forward);
        assert_eq!(slice((0, 3), (2, 4), 1, 20), 0..20);
        assert_eq!(slice((2, 4), (0, 3), 2, 20), 0..4);
        assert_eq!(slice((0, 3), (2, 4), 3, 20), 0..0);
        // A selection inside one fragment, and offsets past its end.
        assert_eq!(slice((1, 2), (1, 5), 1, 10), 2..5);
        assert_eq!(slice((1, 9), (1, 2), 1, 5), 2..5);
    }

    #[test]
    fn words_spaces_and_symbols_expand() {
        let text = "olá, mundo  _x1 (ok)";
        assert_eq!(&text[word(text, 1)], "olá");
        assert_eq!(&text[word(text, 0)], "olá");
        assert_eq!(&text[word(text, 4)], ",");
        assert_eq!(&text[word(text, 6)], "mundo");
        assert_eq!(&text[word(text, 12)], "  ");
        assert_eq!(&text[word(text, 15)], "_x1");
        assert_eq!(&text[word(text, 20)], ")");
        // Past the end it takes the last word.
        assert_eq!(&"fim"[word("fim", 99)], "fim");
        assert_eq!(word("", 0), 0..0);
    }

    #[test]
    fn lines_expand_between_newlines() {
        let text = "um\ndois três\n\nquatro";
        assert_eq!(&text[line(text, 5)], "dois três");
        assert_eq!(&text[line(text, 0)], "um");
        assert_eq!(line(text, 14), 14..14);
        assert_eq!(&text[line(text, 99)], "quatro");
        assert_eq!(unit(text, 5, Gran::Line), line(text, 5));
        assert_eq!(unit(text, 5, Gran::Char), 5..5);
    }

    #[test]
    fn offsets_never_split_utf8_or_combining_marks() {
        let text = "café 🙂 e\u{301}x";
        // 'é' is two bytes (3..5): a byte in the middle snaps back to its start.
        assert_eq!(snap(text, 4), 3);
        assert_eq!(snap("🙂", 2), 0);
        // "e" + combining acute: the mark stays with its letter.
        let e = text.find("e\u{301}").unwrap();
        assert_eq!(snap(text, e + 1), e);
        assert_eq!(snap(text, 999), text.len());
        assert_eq!(&text[word(text, 4)], "café");
        for byte in 0..=text.len() + 2 {
            let r = unit(text, byte, Gran::Word);
            assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
        }
        // Extraction with offsets in the middle of characters does not panic.
        let texts = vec![text.to_owned()];
        assert_eq!(extract(&texts, (0, 0), (0, 5)).as_deref(), Some("café"));
    }

    #[test]
    fn extraction_crosses_blocks_in_either_direction() {
        let texts: Vec<String> = ["Título", "Olá mundo", "item um", "fn main() {}", "célula"]
            .map(String::from)
            .to_vec();
        let forward = extract(&texts, (1, 4), (3, 2));
        assert_eq!(forward.as_deref(), Some("mundo\nitem um\nfn"));
        assert_eq!(extract(&texts, (3, 2), (1, 4)), forward);
        assert_eq!(
            extract(&texts, (0, 0), (4, 100)).unwrap().lines().count(),
            5
        );
        assert_eq!(extract(&texts, (2, 3), (2, 3)), None);
        assert_eq!(extract(&[], (0, 0), (1, 1)), None);
    }

    #[test]
    fn markdown_fragments_are_recorded_in_reading_order() {
        let source = "# Título\n\nOlá **mundo**\n\n- um\n- dois\n\n```\nlet x = 1;\n```\n\n| a | b |\n|---|---|\n| c | d |\n";
        let texts = record(|| {
            crate::markdown::render(source);
        });
        assert_eq!(
            texts,
            [
                "Título",
                "Olá mundo",
                "um",
                "dois",
                "let x = 1;",
                "a",
                "b",
                "c",
                "d"
            ]
        );
        let all = extract(&texts, (0, 0), (texts.len() - 1, 1)).unwrap();
        assert!(all.starts_with("Título\nOlá mundo\num\ndois\nlet x = 1;"));
        // Outside a scope, rendering records nothing and wraps nothing.
        assert!(record(|| {}).is_empty());
    }

    #[test]
    fn press_expands_by_click_count_and_shift_extends() {
        let state = State::default();
        let text = "um dois três";
        state.press(REPLY, 2, text, 5, 1, false, at(0., 0.));
        assert!(!state.has_selection());
        state.press(REPLY, 2, text, 5, 2, false, at(0., 0.));
        assert_eq!(state.range(REPLY, 2, text.len()), 3..7);
        state.press(REPLY, 2, text, 5, 3, false, at(0., 0.));
        assert_eq!(state.range(REPLY, 2, text.len()), 0..text.len());
        // Another reply is untouched.
        let other = Reply { turn: 9, ..REPLY };
        assert_eq!(state.range(other, 2, text.len()), 0..0);
        // Shift extends from the old anchor.
        state.press(REPLY, 2, text, 0, 1, false, at(0., 0.));
        state.press(REPLY, 2, text, 6, 1, true, at(0., 0.));
        assert_eq!(state.range(REPLY, 2, text.len()), 0..6);
        // A click elsewhere clears unless a fragment took it.
        assert!(!state.clear_unless_claimed());
        assert!(state.has_selection());
        assert!(state.clear_unless_claimed());
        assert!(!state.has_selection() && !state.active());
    }

    #[test]
    fn dragging_backwards_by_word_keeps_the_first_word() {
        let anchor = ((0, 4), (0, 8));
        // Dragging to the left of the anchor word selects from the head to the anchor's end.
        assert_eq!(
            extend(anchor, Gran::Word, 0, "um dois três", 0),
            ((0, 8), (0, 0))
        );
        assert_eq!(
            extend(anchor, Gran::Word, 0, "um dois três", 10),
            ((0, 4), (0, 13))
        );
        // Into an earlier or later fragment.
        assert_eq!(extend(anchor, Gran::Char, 1, "x", 1), ((0, 4), (1, 1)));
    }

    #[test]
    fn select_all_and_text_follow_the_fragments() {
        let state = State::default();
        let texts = vec!["abc".to_owned(), "dé".to_owned()];
        state.select_all(REPLY, &texts);
        assert_eq!(state.text(&texts).as_deref(), Some("abc\ndé"));
        assert!(state.has_selection() && state.menu_since().is_some());
        state.clear();
        assert_eq!(state.text(&texts), None);
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use gpui::{Modifiers, MouseMoveEvent, Render, TestAppContext, VisualTestContext, Window, div};
    use std::cell::RefCell;
    use std::rc::Rc;

    const REPLY: Reply = Reply {
        conversation: 0,
        turn: 1,
        user: false,
        beat: 0,
    };

    /// A reply rendered the way the transcript renders it, with the root's handlers as the shell has them.
    struct Probe {
        state: State,
        source: &'static str,
        opened: Rc<RefCell<Vec<String>>>,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.state.begin_frame();
            let body = scoped(&self.state, REPLY, || crate::markdown::render(self.source));
            div()
                .size_full()
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                    this.state.drag_to(event.position);
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if this.state.release() {
                            if let Some(url) = this.state.take_link() {
                                this.opened.borrow_mut().push(url);
                            }
                            cx.notify();
                        }
                    }),
                )
                .child(body)
        }
    }

    fn at(x: f32, y: f32) -> Point<Pixels> {
        gpui::point(px(x), px(y))
    }

    /// Draws one frame, as the platform would.
    fn frame(vcx: &mut VisualTestContext, window: gpui::AnyWindowHandle) {
        vcx.update_window(window, |_, window, cx| {
            let _ = window.draw(cx);
        })
        .unwrap();
    }

    fn open_probe(
        cx: &mut TestAppContext,
        source: &'static str,
    ) -> (State, Rc<RefCell<Vec<String>>>, gpui::AnyWindowHandle) {
        let state = State::default();
        let opened = Rc::new(RefCell::new(Vec::new()));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| Probe {
                    state: state.clone(),
                    source,
                    opened: opened.clone(),
                })
            })
            .unwrap()
        });
        (state, opened, window.into())
    }

    #[gpui::test]
    fn release_after_a_drag_offers_the_menu(cx: &mut TestAppContext) {
        let (state, _, window) = open_probe(
            cx,
            "Primeiro parágrafo com texto\n\nSegundo parágrafo com mais texto",
        );
        let mut vcx = VisualTestContext::from_window(window, cx);
        frame(&mut vcx, window);
        vcx.simulate_mouse_down(at(4., 8.), MouseButton::Left, Modifiers::default());
        frame(&mut vcx, window);
        vcx.simulate_mouse_move(at(120., 70.), MouseButton::Left, Modifiers::default());
        frame(&mut vcx, window);
        assert!(state.has_selection(), "the drag selects text");
        vcx.simulate_mouse_up(at(120., 70.), MouseButton::Left, Modifiers::default());
        frame(&mut vcx, window);
        assert!(!state.dragging());
        assert!(state.menu_since().is_some(), "release opens the menu");
        assert!(state.menu_anchor().is_some(), "the menu has an anchor");
    }

    /// Where the text of fragment `seq` of the reply starts at byte `byte`, once drawn.
    fn spot(state: &State, seq: usize, byte: usize) -> Point<Pixels> {
        let s = state.0.borrow();
        let (_, frag) = s.frags.get(&(REPLY, seq)).expect("drawn");
        frag.layout.position_for_index(byte).expect("on the line")
    }

    #[gpui::test]
    fn a_click_on_a_link_opens_it_and_a_drag_across_it_does_not(cx: &mut TestAppContext) {
        let (state, opened, window) = open_probe(cx, "Veja [o site](https://example.com) agora");
        let mut vcx = VisualTestContext::from_window(window, cx);
        frame(&mut vcx, window);
        let start = spot(&state, 0, 5);
        let on_link = at(f32::from(start.x) + 4., f32::from(start.y) + 8.);
        vcx.simulate_mouse_down(on_link, MouseButton::Left, Modifiers::default());
        frame(&mut vcx, window);
        vcx.simulate_mouse_up(on_link, MouseButton::Left, Modifiers::default());
        frame(&mut vcx, window);
        assert_eq!(*opened.borrow(), ["https://example.com"]);

        // Pressing on the link and dragging selects text: no link opens.
        opened.borrow_mut().clear();
        vcx.simulate_mouse_down(on_link, MouseButton::Left, Modifiers::default());
        frame(&mut vcx, window);
        let far = at(f32::from(on_link.x) + 120., f32::from(on_link.y));
        vcx.simulate_mouse_move(far, MouseButton::Left, Modifiers::default());
        frame(&mut vcx, window);
        vcx.simulate_mouse_up(far, MouseButton::Left, Modifiers::default());
        frame(&mut vcx, window);
        assert!(opened.borrow().is_empty(), "a drag does not open the link");
        assert!(state.menu_since().is_some());
    }
}
