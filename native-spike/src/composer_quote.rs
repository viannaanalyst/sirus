//! Quote blocks that hold their own reply (ADR-198).
//!
//! "Adicionar ao chat" puts the selected passage in the composer as a block: the quoted text on
//! top and, inside the same bordered card, an editable reply. The composer draft becomes a list of
//! segments, free text and quotes, in the order they sit.
//!
//! # The model
//!
//! A `Draft` with no quotes is exactly what it was: `text`, a caret and an anchor, and `segs` is
//! empty. With quotes, `segs` holds every segment and `active` says which one the caret is in.
//! The text of the active segment (the free text, or the reply of a quote) lives in `Draft::text`,
//! so every key and every IME call that already edits `text` keeps working untouched; moving to
//! another segment swaps the buffer (`activate`). The last segment is always free text, the line
//! below the last block.
//!
//! # The format the agent receives (and the draft is saved as)
//!
//! Segments are separated by one blank line, in order. Free text goes as it is. A quote is a
//! markdown blockquote whose first line starts with the source in parentheses, followed
//! (after a blank line) by its reply on a line that starts with the arrow `↳`:
//!
//! ```text
//! > (Resposta do Opus) o checkout agora chama o Asaas
//! > com o plano escolhido
//!
//! ↳ e se o cliente não tiver CPF?
//!   ele ainda consegue pagar?
//!
//! outra coisa, deixa o botão menor
//! ```
//!
//! The reply is the `↳` line plus the lines after it that start with two spaces (blank lines of
//! the reply are kept as two spaces). A quote with an empty reply has no `↳` line. The same text
//! is parsed back (`parse`) into the draft on restore and into the cards of the sent message.
//! Only a line that starts with `> (` and holds `)` opens a quote, so a plain `> ` in free text
//! stays text. The source is "Resposta do <modelo>" for a transcript reply, "Sua mensagem" for the
//! user's own, or `path:from-to` for a quote from a file.
use super::*;

/// The arrow that opens a reply.
const ARROW: &str = "↳";

/// One part of a draft.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Seg {
    Text(String),
    Quote {
        source: String,
        quoted: String,
        reply: String,
    },
}

// ---- Serializing and parsing -----------------------------------------------------------------

/// The text the agent receives for these segments.
pub(super) fn serialize(segs: &[Seg]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for seg in segs {
        match seg {
            Seg::Text(text) => {
                let text = text.trim_matches('\n');
                if !text.trim().is_empty() {
                    parts.push(text.to_owned());
                }
            }
            Seg::Quote {
                source,
                quoted,
                reply,
            } => {
                let source = source.replace(['\n', '\r'], " ").replace(')', "]");
                let mut lines: Vec<String> = Vec::new();
                for (at, line) in quoted.trim().lines().enumerate() {
                    let line = line.trim_end();
                    lines.push(match (at, line.is_empty()) {
                        (0, true) => format!("> ({source})"),
                        (0, false) => format!("> ({source}) {line}"),
                        (_, true) => ">".to_owned(),
                        _ => format!("> {line}"),
                    });
                }
                if lines.is_empty() {
                    lines.push(format!("> ({source})"));
                }
                parts.push(lines.join("\n"));
                let reply = reply.trim_matches('\n');
                if !reply.trim().is_empty() {
                    let mut out = String::new();
                    for (at, line) in reply.lines().enumerate() {
                        if at == 0 {
                            out.push_str(&format!("{ARROW} {line}"));
                        } else {
                            out.push_str(&format!("\n  {line}"));
                        }
                    }
                    // The reply follows its quote, after one blank line.
                    let quote = parts.pop().unwrap_or_default();
                    parts.push(format!("{quote}\n\n{out}"));
                }
            }
        }
    }
    parts.join("\n\n")
}

/// The source and first quoted line of a quote's opening line, if it is one.
fn opening(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("> (")?;
    let close = rest.find(')')?;
    let source = rest[..close].to_owned();
    let first = rest[close + 1..]
        .strip_prefix(' ')
        .unwrap_or(&rest[close + 1..]);
    Some((source, first.to_owned()))
}

/// The segments a serialized text holds. Text with no quote is one `Text`.
pub(super) fn parse(text: &str) -> Vec<Seg> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut segs: Vec<Seg> = Vec::new();
    let mut free: Vec<&str> = Vec::new();
    let flush = |free: &mut Vec<&str>, segs: &mut Vec<Seg>| {
        let joined = free.join("\n");
        free.clear();
        let joined = joined.trim_matches('\n');
        if !joined.trim().is_empty() {
            segs.push(Seg::Text(joined.to_owned()));
        }
    };
    let mut at = 0;
    while at < lines.len() {
        let Some((source, first)) = opening(lines[at]) else {
            free.push(lines[at]);
            at += 1;
            continue;
        };
        flush(&mut free, &mut segs);
        let mut quoted = vec![first];
        at += 1;
        while at < lines.len() && lines[at].starts_with('>') {
            let line = lines[at].strip_prefix('>').unwrap_or("");
            quoted.push(line.strip_prefix(' ').unwrap_or(line).to_owned());
            at += 1;
        }
        // A blank line may sit between the quote and its arrow.
        let arrow_at = if at < lines.len() && lines[at].trim().is_empty() {
            at + 1
        } else {
            at
        };
        let mut reply = String::new();
        if let Some(line) = lines.get(arrow_at)
            && (*line == ARROW || line.starts_with(&format!("{ARROW} ")))
        {
            reply.push_str(line[ARROW.len()..].strip_prefix(' ').unwrap_or(""));
            at = arrow_at + 1;
            while at < lines.len() {
                if let Some(rest) = lines[at].strip_prefix("  ") {
                    reply.push('\n');
                    reply.push_str(rest);
                } else if lines[at].trim().is_empty()
                    && lines.get(at + 1).is_some_and(|next| next.starts_with("  "))
                {
                    reply.push('\n');
                } else {
                    break;
                }
                at += 1;
            }
        }
        segs.push(Seg::Quote {
            source,
            quoted: quoted.join("\n"),
            reply,
        });
    }
    flush(&mut free, &mut segs);
    segs
}

/// Whether a sent message holds any quote block.
pub(super) fn has_quotes(text: &str) -> bool {
    text.contains("> (") && parse(text).iter().any(|s| matches!(s, Seg::Quote { .. }))
}

/// The pieces of a sent message the transcript can select, in the order the bubble draws them:
/// each free line and each non-empty reply.
pub(super) fn fragments(segs: &[Seg]) -> Vec<String> {
    let mut out = Vec::new();
    for seg in segs {
        match seg {
            Seg::Text(text) => out.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned),
            ),
            Seg::Quote { reply, .. } if !reply.trim().is_empty() => {
                out.push(reply.trim().to_owned())
            }
            Seg::Quote { .. } => {}
        }
    }
    out
}

/// A text with only letters and digits, lowercase: how a quote finds its source despite the
/// markdown the reply was written in.
fn flat(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The turn before `before` whose text holds `quoted`, the latest first.
pub(super) fn source_turn(turns: &[(&str, &str)], before: usize, quoted: &str) -> Option<usize> {
    let first = quoted.lines().map(str::trim).find(|l| !l.is_empty())?;
    let needle: String = flat(first).chars().take(60).collect();
    if needle.is_empty() {
        return None;
    }
    (0..before.min(turns.len()))
        .rev()
        .find(|&at| flat(turns[at].1).contains(&needle) || flat(turns[at].0).contains(&needle))
}

// ---- The draft's segments --------------------------------------------------------------------

impl Draft {
    /// Whether nothing at all is written.
    pub(super) fn is_empty(&self) -> bool {
        self.segs.is_empty() && self.text.is_empty()
    }

    /// Whether there is anything to send.
    pub(super) fn has_content(&self) -> bool {
        !self.segs.is_empty() || !self.text.trim().is_empty()
    }

    pub(super) fn quoted(&self) -> bool {
        !self.segs.is_empty()
    }

    /// Every segment, the active one with the live text.
    pub(super) fn segments(&self) -> Vec<Seg> {
        if self.segs.is_empty() {
            return vec![Seg::Text(self.text.clone())];
        }
        let mut segs = self.segs.clone();
        match segs.get_mut(self.active) {
            Some(Seg::Text(text)) => text.clone_from(&self.text),
            Some(Seg::Quote { reply, .. }) => reply.clone_from(&self.text),
            None => {}
        }
        segs
    }

    /// What is sent and saved: the plain text, or the serialized segments.
    pub(super) fn full_text(&self) -> String {
        if self.segs.is_empty() {
            self.text.clone()
        } else {
            serialize(&self.segments())
        }
    }

    /// A saved draft: quote blocks come back as blocks, anything else as plain text.
    pub(super) fn from_saved(text: &str) -> Draft {
        if !has_quotes(text) {
            return Draft::with_text(text.to_owned());
        }
        let mut draft = Draft::default();
        let mut segs = parse(text);
        if !matches!(segs.last(), Some(Seg::Text(_))) {
            segs.push(Seg::Text(String::new()));
        }
        let last = segs.len() - 1;
        draft.install(segs, last, None);
        draft
    }

    /// Makes `segs` the draft with the caret in `active`, at byte `at` of its text (the end by
    /// default). Keeps the rules: a free line last, and no segments at all without a quote.
    fn install(&mut self, mut segs: Vec<Seg>, mut active: usize, at: Option<usize>) {
        if !matches!(segs.last(), Some(Seg::Text(_))) {
            segs.push(Seg::Text(String::new()));
        }
        if !segs.iter().any(|s| matches!(s, Seg::Quote { .. })) {
            let text = match segs.into_iter().next() {
                Some(Seg::Text(text)) => text,
                _ => String::new(),
            };
            self.segs.clear();
            self.active = 0;
            self.text = text;
            self.cursor = at.unwrap_or(self.text.len()).min(self.text.len());
            self.anchor = self.cursor;
            return;
        }
        active = active.min(segs.len() - 1);
        self.text = match &mut segs[active] {
            Seg::Text(text) => std::mem::take(text),
            Seg::Quote { reply, .. } => std::mem::take(reply),
        };
        self.segs = segs;
        self.active = active;
        self.cursor = at.unwrap_or(self.text.len()).min(self.text.len());
        self.anchor = self.cursor;
    }

    /// Moves the caret to another segment, at byte `at` of its text (the end by default).
    pub(super) fn activate(&mut self, to: usize, at: Option<usize>) {
        if self.segs.is_empty() || to >= self.segs.len() {
            return;
        }
        let segs = self.segments();
        self.install(segs, to, at);
    }

    /// Whether the caret is in a quote's reply.
    pub(super) fn in_reply(&self) -> bool {
        matches!(self.segs.get(self.active), Some(Seg::Quote { .. }))
    }

    /// Puts a quote block at the caret: right after the block whose reply the caret is in, or
    /// else after the current line. The caret goes to the new block's reply.
    pub(super) fn insert_quote(&mut self, source: String, quoted: String) {
        let quote = Seg::Quote {
            source,
            quoted,
            reply: String::new(),
        };
        let current = self.active;
        let line_end = self.line_end(self.cursor);
        let mut segs = self.segments();
        let at = match segs.get(current) {
            Some(Seg::Quote { .. }) => {
                segs.insert(current + 1, quote);
                current + 1
            }
            _ => {
                let current = current.min(segs.len() - 1);
                let Seg::Text(text) = &segs[current] else {
                    unreachable!("a text segment");
                };
                let before = text[..line_end].to_owned();
                let after = text[line_end..]
                    .strip_prefix('\n')
                    .unwrap_or(&text[line_end..])
                    .to_owned();
                // An empty first line is not kept: the block opens the message.
                let keep = !(current == 0 && before.trim().is_empty());
                let mut put = Vec::new();
                if keep {
                    put.push(Seg::Text(before));
                }
                let at = current + put.len();
                put.push(quote);
                put.push(Seg::Text(after));
                segs.splice(current..=current, put);
                at
            }
        };
        self.install(segs, at, Some(0));
    }

    /// Enter on an empty last line of a reply leaves the block, without adding the empty line.
    /// False, and nothing changed, when the caret is not at the end of such a reply.
    pub(super) fn leave_block(&mut self) -> bool {
        if !self.in_reply() || self.cursor != self.text.len() || self.anchor != self.cursor {
            return false;
        }
        if !(self.text.is_empty() || self.text.ends_with('\n')) {
            return false;
        }
        let kept = self.text.trim_end_matches('\n').len();
        self.text.truncate(kept);
        let mut segs = self.segments();
        let next = self.active + 1;
        if !matches!(segs.get(next), Some(Seg::Text(_))) {
            segs.insert(next, Seg::Text(String::new()));
        }
        self.install(segs, next, None);
        true
    }

    /// Removes the quote block `at`. The caret goes to the line before it (Backspace) or after it
    /// (the ✕), and two free lines left side by side become one.
    pub(super) fn remove_quote(&mut self, at: usize, back: bool) {
        let mut segs = self.segments();
        if !matches!(segs.get(at), Some(Seg::Quote { .. })) {
            return;
        }
        segs.remove(at);
        let before = at.checked_sub(1);
        let merged = match (before, segs.get(at).cloned()) {
            (Some(b), Some(Seg::Text(next))) => match &mut segs[b] {
                Seg::Text(prev) => {
                    let join = prev.len();
                    if !prev.is_empty() && !next.is_empty() {
                        prev.push('\n');
                    }
                    prev.push_str(&next);
                    Some((b, join))
                }
                _ => None,
            },
            _ => None,
        };
        if let Some((b, join)) = merged {
            segs.remove(at);
            self.install(segs, b, Some(join));
            return;
        }
        match (back, before) {
            (true, Some(b)) => self.install(segs, b, None),
            _ => {
                let to = at.min(segs.len() - 1);
                self.install(segs, to, if at == 0 { Some(0) } else { None })
            }
        }
    }

    /// Backspace in an empty reply removes its block. False when it does not apply.
    pub(super) fn backspace_in_empty_reply(&mut self) -> bool {
        if !self.in_reply() || !self.text.is_empty() {
            return false;
        }
        self.remove_quote(self.active, true);
        true
    }

    /// An arrow at the end of a segment's text moves the caret into the neighbour. False when the
    /// caret is not at that end or there is no neighbour.
    pub(super) fn step_segment(&mut self, forward: bool) -> bool {
        if self.segs.is_empty() || self.anchor != self.cursor {
            return false;
        }
        if forward && self.cursor == self.text.len() && self.active + 1 < self.segs.len() {
            self.activate(self.active + 1, Some(0));
            return true;
        }
        if !forward && self.cursor == 0 && self.active > 0 {
            self.activate(self.active - 1, None);
            return true;
        }
        false
    }
}

// ---- The window's side -----------------------------------------------------------------------

/// Accent-tinted border of a block whose reply has the caret.
fn focused_edge() -> gpui::Hsla {
    crate::theme::accent_wash(0.36)
}

impl Shell {
    /// "Adicionar ao chat": the passage becomes a quote block in the composer, with the caret in
    /// its reply.
    pub(super) fn add_to_chat(&mut self, text: &str, reply: Option<transcript_select::Reply>) {
        let source = self.quote_source(reply);
        let quoted = text.trim().to_owned();
        if quoted.is_empty() {
            return;
        }
        self.touch_drafts();
        let ix = self.active_conversation();
        self.drafts
            .entry(ix)
            .or_default()
            .insert_quote(source, quoted);
    }

    /// Where a selected passage came from, as the block says it.
    fn quote_source(&self, reply: Option<transcript_select::Reply>) -> String {
        match reply {
            Some(reply) if reply.user => t("Sua mensagem").to_owned(),
            _ => {
                let conversation = &self.conversations[self.active_conversation()];
                let model =
                    self.model_display(conversation.provider, conversation.model.as_deref());
                format!("{} {model}", t("Resposta do"))
            }
        }
    }

    /// ⌘⇧L over a selection in the transcript, as the menu's "Adicionar ao chat".
    pub(super) fn add_selection_to_chat(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(text) = self.selected_text() else {
            return false;
        };
        let reply = self.select.reply();
        self.select.clear();
        self.add_to_chat(&text, reply);
        self.keep_typing_focus(window, cx);
        cx.notify();
        true
    }

    /// The composer draft, when it holds quote blocks and nothing else has the keys.
    fn quoted_draft(&mut self) -> Option<&mut Draft> {
        if self.edit_target().is_some() || self.query_target().is_some() {
            return None;
        }
        let ix = self.active_conversation();
        self.drafts.get_mut(&ix).filter(|d| d.quoted())
    }

    /// Enter in a reply: leaves the block from an empty last line, else adds a line (the message
    /// goes with ⌘Enter, as in the prototype). False outside a reply.
    pub(super) fn quote_enter(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(draft) = self.quoted_draft() else {
            return false;
        };
        if !draft.in_reply() {
            return false;
        }
        if !draft.leave_block() {
            draft.insert("\n");
        }
        self.touch_drafts();
        cx.notify();
        true
    }

    /// Backspace in an empty reply, and the arrows at the ends of a segment.
    pub(super) fn quote_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(draft) = self.quoted_draft() else {
            return false;
        };
        let done = match key {
            "backspace" => draft.backspace_in_empty_reply(),
            "down" | "right" => draft.step_segment(true),
            "up" | "left" => draft.step_segment(false),
            _ => false,
        };
        if done {
            self.touch_drafts();
            cx.notify();
        }
        done
    }

    /// A click in a segment other than the active one puts the caret there.
    fn activate_segment(&mut self, seg: usize, at: usize, cx: &mut Context<Self>) {
        let Some(draft) = self.quoted_draft() else {
            return;
        };
        draft.activate(seg, Some(at));
        self.selecting = true;
        cx.notify();
    }

    /// Clicking empty composer space focuses the last free line.
    pub(super) fn focus_last_line(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.quoted_draft() else {
            return;
        };
        let last = draft.segs.len() - 1;
        draft.activate(last, None);
        cx.notify();
    }

    fn remove_block(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some(draft) = self.quoted_draft() else {
            return;
        };
        draft.remove_quote(at, false);
        self.touch_drafts();
        cx.notify();
    }

    /// A free line that is not the active one: static text, a click puts the caret in it.
    fn quiet_line(
        &self,
        at: usize,
        text: &str,
        placeholder: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shown = crate::inline_badges::shown_for(
            if text.is_empty() { " " } else { text },
            false,
            14.,
            cx,
        );
        let styled = crate::inline_badges::styled(&shown);
        let layout = styled.layout().clone();
        let badges =
            crate::inline_badges::overlay(layout.clone(), shown.clone(), 0x1000 + at as u64);
        let len = text.len();
        div()
            .relative()
            .w_full()
            .min_h(px(22.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    let index = layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|i| i);
                    let index = shown.to_plain(index).min(len);
                    this.activate_segment(at, index, cx);
                    cx.stop_propagation();
                }),
            )
            .child(styled)
            .child(badges)
            .when_some(placeholder.filter(|_| text.is_empty()), |d, text| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(3.))
                        .text_color(content(0.4))
                        .child(t(text)),
                )
            })
            .into_any_element()
    }

    /// The composer's text when it holds quote blocks: the free lines and the blocks in order.
    /// `editor` is the live editor of the active segment, put where the caret is.
    pub(super) fn quote_stack(
        &self,
        draft: &Draft,
        editor: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut editor = Some(editor);
        let last = draft.segs.len() - 1;
        let mut column = div().relative().w_full().flex().flex_col().gap(px(6.));
        for (at, seg) in draft.segs.iter().enumerate() {
            let live = at == draft.active;
            match seg {
                Seg::Text(text) => {
                    let placeholder =
                        (at == last).then_some("Escreva outra coisa, fora da citação…");
                    let line = if live {
                        editor.take().unwrap_or_else(|| div().into_any_element())
                    } else {
                        self.quiet_line(at, text, placeholder, cx)
                    };
                    column = column.child(div().w_full().px(px(2.)).child(line));
                }
                Seg::Quote {
                    source,
                    quoted,
                    reply,
                } => {
                    let line = if live {
                        editor.take().unwrap_or_else(|| div().into_any_element())
                    } else {
                        self.quiet_line(at, reply, Some("Sua resposta a este trecho…"), cx)
                    };
                    let edge = if live {
                        focused_edge()
                    } else {
                        crate::theme::border()
                    };
                    column =
                        column.child(
                            div()
                                .id(("quote-block", at))
                                .relative()
                                .w_full()
                                .rounded(px(crate::theme::RADIUS_MD))
                                .border_1()
                                .border_color(edge)
                                .bg(content(0.02))
                                .overflow_hidden()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(quote_head(source, quoted, false))
                                .child(
                                    div()
                                        .id(("quote-x", at))
                                        .absolute()
                                        .top(px(5.))
                                        .right(px(7.))
                                        .size(px(16.))
                                        .rounded(px(crate::theme::RADIUS_SM))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .text_size(px(11.))
                                        .text_color(content(0.4))
                                        .hover(|s| s.text_color(content(0.9)))
                                        .child("✕")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_block(at, cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .relative()
                                        .w_full()
                                        .pl(px(24.))
                                        .pr(px(12.))
                                        .py(px(8.))
                                        .child(reply_mark(8.))
                                        .child(line),
                                ),
                        );
                }
            }
        }
        column.into_any_element()
    }

    /// The sent message when it holds quotes: each block a card with the quote on top and its
    /// reply below, free text as paragraphs. Clicking a quote scrolls to where it came from.
    pub(super) fn quoted_bubble(
        &self,
        conversation: usize,
        turn: usize,
        segs: &[Seg],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        crate::inline_badges::sync(&self.skills, &self.files);
        let reply = transcript_select::Reply {
            conversation,
            turn,
            user: true,
            beat: 0,
        };
        let key = 1 + turn as u64 + ((conversation as u64) << 24);
        let mut piece = 0u64;
        let mut text_of = |text: &str, cx: &mut Context<Self>| -> AnyElement {
            piece += 1;
            let shown = crate::inline_badges::shown_for(text, true, 14., cx);
            crate::inline_badges::remember(&shown);
            let styled = crate::inline_badges::styled(&shown);
            let badges = crate::inline_badges::overlay(
                styled.layout().clone(),
                shown.clone(),
                key + (piece << 40),
            );
            div()
                .relative()
                .child(transcript_select::wrap(styled, shown.text.clone().into()))
                .child(badges)
                .into_any_element()
        };
        let rows = transcript_select::scoped(&self.select, reply, || {
            let mut rows: Vec<AnyElement> = Vec::new();
            for (at, seg) in segs.iter().enumerate() {
                match seg {
                    Seg::Text(text) => {
                        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                            rows.push(div().w_full().child(text_of(line, cx)).into_any_element());
                        }
                    }
                    Seg::Quote {
                        source,
                        quoted,
                        reply,
                    } => {
                        let reply = reply.trim();
                        let target = quoted.clone();
                        let head = quote_head(source, quoted, true);
                        let head = div()
                            .id(("quote-ref", at + (turn << 8)))
                            .cursor_pointer()
                            .child(head)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.reveal_quote_source(conversation, turn, &target, cx)
                            }));
                        rows.push(
                            div()
                                .w_full()
                                .rounded(px(crate::theme::RADIUS_MD))
                                .border_1()
                                .border_color(content(0.11))
                                .bg(gpui::black().opacity(0.15))
                                .overflow_hidden()
                                .child(head)
                                .when(!reply.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .relative()
                                            .pl(px(24.))
                                            .pr(px(12.))
                                            .py(px(7.))
                                            .child(reply_mark(7.))
                                            .child(text_of(reply, cx)),
                                    )
                                })
                                .into_any_element(),
                        );
                    }
                }
            }
            rows
        });
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .min_w(px(0.))
            .children(rows)
            .into_any_element()
    }

    /// Scrolls to the answer a quote was taken from, found by its text.
    fn reveal_quote_source(
        &mut self,
        conversation: usize,
        from: usize,
        quoted: &str,
        cx: &mut Context<Self>,
    ) {
        let texts: Vec<(&str, &str)> = self.conversations[conversation]
            .turns
            .iter()
            .map(|t| (t.user.as_str(), t.answer.as_str()))
            .collect();
        let Some(turn) = source_turn(&texts, from, quoted) else {
            return;
        };
        self.list_state(conversation, cx)
            .scroll_to_reveal_item(turn + 1);
        cx.notify();
    }
}

/// The arrow that marks a reply, at the left of its text.
fn reply_mark(top: f32) -> Div {
    div()
        .absolute()
        .left(px(9.))
        .top(px(top))
        .text_size(px(12.))
        .text_color(accent())
        .child(ARROW)
}

/// The top of a block: a 3 px accent bar, the source and the quote clamped to two lines.
fn quote_head(source: &str, quoted: &str, sent: bool) -> Div {
    div()
        .flex()
        .gap(px(9.))
        .px(px(10.))
        .pt(px(8.))
        .pb(px(7.))
        .pr(px(if sent { 10. } else { 28. }))
        .border_b_1()
        .border_color(content(0.05))
        .bg(content(if sent { 0. } else { 0.025 }))
        .child(div().w(px(3.)).flex_shrink_0().rounded(px(3.)).bg(accent()))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.5))
                        .line_height(px(15.))
                        .text_color(content(0.55))
                        .child(source.to_owned()),
                )
                .child(
                    div()
                        .line_clamp(2)
                        .text_size(px(12.5))
                        .line_height(px(17.))
                        .text_color(content(if sent { 0.9 } else { 0.85 }))
                        .child(quoted.to_owned()),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(source: &str, quoted: &str, reply: &str) -> Seg {
        Seg::Quote {
            source: source.into(),
            quoted: quoted.into(),
            reply: reply.into(),
        }
    }

    fn text(t: &str) -> Seg {
        Seg::Text(t.into())
    }

    #[test]
    fn a_quote_is_a_blockquote_followed_by_its_reply() {
        let segs = vec![
            quote(
                "Resposta do Opus",
                "o checkout agora chama o Asaas",
                "e se o cliente não tiver CPF?",
            ),
            text("outra coisa, deixa o botão menor"),
        ];
        assert_eq!(
            serialize(&segs),
            "> (Resposta do Opus) o checkout agora chama o Asaas\n\n\
             ↳ e se o cliente não tiver CPF?\n\n\
             outra coisa, deixa o botão menor"
        );
    }

    #[test]
    fn serializing_and_parsing_round_trip() {
        let segs = vec![
            text("antes\n\ndo bloco"),
            quote(
                "Resposta do Opus",
                "linha um\n\nlinha três",
                "resposta\n\nem dois\n  recuada",
            ),
            quote("src/main.rs:12-20", "fn main() {}", ""),
            text("depois"),
        ];
        let flat = serialize(&segs);
        assert_eq!(parse(&flat), segs);
        // The saved draft comes back with its blocks, and sends the same text.
        let draft = Draft::from_saved(&flat);
        assert!(draft.quoted());
        assert_eq!(draft.full_text(), flat);
        assert_eq!(draft.segments(), segs);
    }

    #[test]
    fn plain_text_is_not_a_quote() {
        for plain in ["", "olá", "> uma citação comum\n\nsegue", "↳ solta"] {
            assert!(!has_quotes(plain), "{plain}");
            let draft = Draft::from_saved(plain);
            assert!(!draft.quoted());
            assert_eq!((draft.text.as_str(), draft.cursor), (plain, plain.len()));
            assert_eq!(draft.full_text(), plain);
        }
    }

    #[test]
    fn a_quote_goes_after_the_current_line() {
        let mut draft = Draft::with_text("primeira\nsegunda\nterceira".into());
        draft.cursor = 3;
        draft.anchor = 3;
        draft.insert_quote("Resposta do Opus".into(), "trecho".into());
        assert_eq!(
            draft.segments(),
            vec![
                text("primeira"),
                quote("Resposta do Opus", "trecho", ""),
                text("segunda\nterceira"),
            ]
        );
        // The caret is in the new reply.
        assert!(draft.in_reply());
        assert_eq!(draft.active, 1);
        assert_eq!(draft.text, "");
    }

    #[test]
    fn a_quote_in_an_empty_composer_opens_it_with_a_free_line_below() {
        let mut draft = Draft::default();
        draft.insert_quote("s".into(), "t".into());
        assert_eq!(draft.segments(), vec![quote("s", "t", ""), text("")]);
        assert!(draft.in_reply());
    }

    #[test]
    fn a_quote_from_inside_a_reply_goes_right_after_that_block() {
        let mut draft = Draft::default();
        draft.insert_quote("s".into(), "um".into());
        draft.insert("minha resposta");
        draft.insert_quote("s".into(), "dois".into());
        assert_eq!(
            draft.segments(),
            vec![
                quote("s", "um", "minha resposta"),
                quote("s", "dois", ""),
                text(""),
            ]
        );
        assert_eq!(draft.active, 1);
    }

    #[test]
    fn enter_on_an_empty_last_line_leaves_the_block_without_the_empty_line() {
        let mut draft = Draft::default();
        draft.insert_quote("s".into(), "um".into());
        draft.insert("resposta\n");
        assert!(draft.leave_block());
        assert_eq!(draft.segments()[0], quote("s", "um", "resposta"));
        assert_eq!(draft.active, 1);
        assert!(!draft.in_reply());
        // An empty reply leaves too; a line with text does not.
        let mut empty = Draft::default();
        empty.insert_quote("s".into(), "um".into());
        assert!(empty.leave_block());
        let mut typing = Draft::default();
        typing.insert_quote("s".into(), "um".into());
        typing.insert("ainda escrevendo");
        assert!(!typing.leave_block());
        assert_eq!(typing.text, "ainda escrevendo");
    }

    #[test]
    fn leaving_a_block_followed_by_another_adds_a_free_line_between() {
        let mut draft = Draft::default();
        draft.insert_quote("s".into(), "um".into());
        draft.insert_quote("s".into(), "dois".into());
        draft.activate(0, None);
        assert!(draft.leave_block());
        assert_eq!(
            draft.segments(),
            vec![
                quote("s", "um", ""),
                text(""),
                quote("s", "dois", ""),
                text(""),
            ]
        );
        assert_eq!(draft.active, 1);
    }

    #[test]
    fn backspace_in_an_empty_reply_removes_the_block_and_goes_to_the_line_before() {
        let mut draft = Draft::with_text("antes".into());
        draft.insert_quote("s".into(), "um".into());
        assert_eq!(draft.segments().len(), 3);
        assert!(draft.backspace_in_empty_reply());
        // The two free lines are one again, with the caret where they met; no quote is left, so
        // the draft is plain text.
        assert!(!draft.quoted());
        assert_eq!((draft.text.as_str(), draft.cursor), ("antes", 5));
        // Not on a reply with text.
        let mut full = Draft::default();
        full.insert_quote("s".into(), "um".into());
        full.insert("x");
        assert!(!full.backspace_in_empty_reply());
        assert!(full.quoted());
    }

    #[test]
    fn removing_a_block_keeps_the_others_and_the_caret_before_it() {
        let mut draft = Draft::default();
        draft.insert_quote("s".into(), "um".into());
        draft.insert("a");
        draft.insert_quote("s".into(), "dois".into());
        assert!(draft.backspace_in_empty_reply());
        assert_eq!(draft.segments(), vec![quote("s", "um", "a"), text("")]);
        assert_eq!(draft.active, 0);
        assert_eq!(draft.cursor, 1);
        draft.remove_quote(0, false);
        assert!(!draft.quoted());
        assert!(draft.is_empty());
    }

    #[test]
    fn arrows_cross_segments_only_at_their_ends() {
        let mut draft = Draft::default();
        draft.insert_quote("s".into(), "um".into());
        draft.insert("ab");
        draft.cursor = 1;
        draft.anchor = 1;
        assert!(!draft.step_segment(true));
        draft.cursor = 2;
        draft.anchor = 2;
        assert!(draft.step_segment(true));
        assert_eq!(draft.active, 1);
        assert_eq!(draft.cursor, 0);
        assert!(draft.step_segment(false));
        assert_eq!((draft.active, draft.cursor), (0, 2));
    }

    #[test]
    fn a_quote_finds_the_turn_it_came_from() {
        let turns = vec![
            ("", "O **checkout** agora chama o Asaas com o plano."),
            ("", "Outra coisa."),
        ];
        assert_eq!(
            source_turn(&turns, 2, "o checkout agora chama o Asaas"),
            Some(0)
        );
        assert_eq!(source_turn(&turns, 0, "o checkout agora"), None);
        assert_eq!(source_turn(&turns, 2, "nada a ver"), None);
    }
}
