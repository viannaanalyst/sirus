//! O painel Chat lateral, depois do SideChat do Sirus (ADR-049): uma conversa pequena, ao lado da
//! principal, que fala sobre ela sem alterá-la. Ao abrir, o back end cria (ou reaproveita) a
//! sessão lateral; o transcrito vem de `transcript_action` e se atualiza por `session-updated` e
//! `agent-output`. O composer é próprio: o texto chega pelo input handler do GPUI.
use crate::data::{self, Conversation};
use crate::i18n::t;
use crate::marks::shimmer;
use crate::panels::PanelContext;
use crate::theme::{accent, accent_wash, content, field_fill, icon, on_accent};
use gpui::{
    AnyView, App, Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle,
    KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point, Render, ScrollHandle, StyledText,
    TextLayout, UTF16Selection, Window, div, prelude::*, px,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub fn new(context: PanelContext, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let mut panel = SideChat {
            backend: context.backend,
            parent: context.session_id,
            side: Conversation::placeholder(),
            ready: false,
            error: None,
            text: String::new(),
            cursor: 0,
            anchor: 0,
            marked: None,
            focus: cx.focus_handle(),
            focused: false,
            scroll: ScrollHandle::new(),
            epoch: Instant::now(),
            layout: Rc::new(RefCell::new(None)),
        };
        panel.open(cx);
        panel.listen(cx);
        // Os laços (shimmer, texto chegando) andam a 30 Hz; o caret é fixo.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    if this.side.working() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        panel
    })
    .into()
}

struct SideChat {
    backend: crate::backend::Backend,
    parent: String,
    /// A sessão lateral; o `session_id` fica vazio até o back end responder.
    side: Conversation,
    ready: bool,
    error: Option<String>,
    text: String,
    cursor: usize,
    anchor: usize,
    marked: Option<Range<usize>>,
    focus: FocusHandle,
    focused: bool,
    scroll: ScrollHandle,
    epoch: Instant,
    layout: Rc<RefCell<Option<TextLayout>>>,
}

/// Puts a passage from the main chat in the composer as a quote and focuses it (the selection
/// menu's "Perguntar no chat lateral").
pub fn quote(view: &AnyView, passage: &str, window: &mut Window, cx: &mut App) {
    let Ok(panel) = view.clone().downcast::<SideChat>() else {
        return;
    };
    let quoted = passage
        .lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    panel.update(cx, |panel, cx| {
        if !panel.text.trim().is_empty() {
            panel.text.push_str("\n\n");
        } else {
            panel.text.clear();
        }
        panel.text.push_str(&quoted);
        panel.text.push_str("\n\n");
        panel.cursor = panel.text.len();
        panel.anchor = panel.cursor;
        window.focus(&panel.focus, cx);
        cx.notify();
    });
}

async fn settle(rx: async_channel::Receiver<Result<Value, Value>>) -> Result<Value, String> {
    match rx.recv().await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error
            .as_str()
            .or(error["message"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| error.to_string())),
        Err(_) => Err(t("O motor do Sirus não respondeu.").into()),
    }
}

/// Um UUID v4 aleatório, um por envio.
fn uuid() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut random) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = random.read_exact(&mut bytes);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// O índice em bytes de um offset UTF-16.
fn byte_at(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, c) in text.char_indices() {
        if units >= utf16 {
            return byte;
        }
        units += c.len_utf16();
    }
    text.len()
}

impl SideChat {
    fn open(&mut self, cx: &mut Context<Self>) {
        let rx = self.backend.request(
            "side_chat_action",
            json!({ "action": { "type": "open", "parentSessionId": self.parent } }),
        );
        cx.spawn(async move |this, cx| {
            let opened = settle(rx).await;
            let _ = this.update(cx, |this, cx| match opened {
                Ok(session) => {
                    this.side = Conversation::from_session(&session);
                    this.load(cx);
                    cx.notify();
                }
                Err(error) => {
                    this.error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let rx = self.backend.request("transcript_action", json!({ "action": { "type": "load", "sessionId": self.side.session_id, "limit": 120, "lite": true } }));
        cx.spawn(async move |this, cx| {
            let loaded = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                match loaded {
                    Ok(value) => {
                        this.side.messages =
                            value["messages"].as_array().cloned().unwrap_or_default();
                        this.side.base = value["start"].as_u64().unwrap_or(0) as usize;
                        this.side.loaded = true;
                        this.side.rebuild();
                        this.ready = true;
                        this.scroll.scroll_to_bottom();
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn listen(&mut self, cx: &mut Context<Self>) {
        let updates = self.backend.subscribe("session-updated");
        cx.spawn(async move |this, cx| {
            while let Ok(session) = updates.recv().await {
                if this
                    .update(cx, |this, cx| this.session_updated(&session, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let output = self.backend.subscribe("agent-output");
        cx.spawn(async move |this, cx| {
            while let Ok(event) = output.recv().await {
                if this
                    .update(cx, |this, cx| this.agent_output(&event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn session_updated(&mut self, session: &Value, cx: &mut Context<Self>) {
        if self.side.session_id.is_empty()
            || session["id"].as_str() != Some(self.side.session_id.as_str())
        {
            return;
        }
        let conversation = &mut self.side;
        conversation.apply_meta(session);
        if conversation.loaded {
            if let Some(messages) = session["messages"].as_array().filter(|m| !m.is_empty()) {
                let from = session["transcriptWindow"]["from"]
                    .as_u64()
                    .map_or(conversation.base, |f| f as usize);
                if from >= conversation.base {
                    conversation.messages.truncate(from - conversation.base);
                    conversation.messages.extend(messages.iter().cloned());
                }
            }
            conversation.rebuild();
        }
        self.follow();
        cx.notify();
    }

    /// Texto em fluxo, aplicado pelo offset (UTF-16) para que repetição ou lacuna não o embaralhem.
    fn agent_output(&mut self, event: &Value, cx: &mut Context<Self>) {
        let conversation = &mut self.side;
        if conversation.session_id.is_empty()
            || event["sessionId"].as_str() != Some(conversation.session_id.as_str())
            || !conversation.loaded
        {
            return;
        }
        let message_id = event["messageId"].as_str().unwrap_or("");
        let at = conversation
            .messages
            .iter()
            .rposition(|m| m["id"].as_str() == Some(message_id));
        if event["message"].is_object() {
            match at {
                Some(at) => {
                    let (old, new) = (
                        conversation.messages[at]["content"]
                            .as_str()
                            .unwrap_or("")
                            .len(),
                        event["message"]["content"].as_str().unwrap_or("").len(),
                    );
                    if new > old {
                        let activity = conversation.messages[at]["activity"].take();
                        let streaming = conversation.messages[at]["streaming"].take();
                        conversation.messages[at] = event["message"].clone();
                        conversation.messages[at]["activity"] = activity;
                        conversation.messages[at]["streaming"] = streaming;
                    }
                }
                None => conversation.messages.push(event["message"].clone()),
            }
        } else if let Some(at) = at {
            let offset = event["offset"].as_u64().unwrap_or(0) as usize;
            let mut text = conversation.messages[at]["content"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            if data::utf16_len(&text) == offset {
                text.push_str(event["chunk"].as_str().unwrap_or(""));
                conversation.messages[at]["content"] = Value::String(text);
            }
        }
        conversation.rebuild();
        self.follow();
        cx.notify();
    }

    /// Mantém a conversa no fim enquanto trabalha, a não ser que a pessoa tenha rolado para ler.
    fn follow(&mut self) {
        let (offset, max) = (
            f32::from(self.scroll.offset().y),
            f32::from(self.scroll.max_offset().y),
        );
        if max + offset < 90. {
            self.scroll.scroll_to_bottom();
        }
    }


    fn selection(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    fn insert(&mut self, text: &str) {
        let range = self.selection();
        self.text.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        self.anchor = self.cursor;
    }

    fn previous(&self, at: usize) -> usize {
        self.text[..at]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8())
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        let prompt = self.text.trim().to_owned();
        if prompt.is_empty() || !self.ready || self.side.working() {
            return;
        }
        let rx = self.backend.request("send_prompt", json!({ "request": { "sessionId": self.side.session_id, "prompt": prompt, "clientRequestId": uuid() } }));
        self.text.clear();
        self.cursor = 0;
        self.anchor = 0;
        self.marked = None;
        self.error = None;
        self.scroll.scroll_to_bottom();
        cx.spawn(async move |this, cx| {
            if let Err(error) = settle(rx).await {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if !self.side.session_id.is_empty() {
            self.backend
                .invoke("stop_agent", json!({ "sessionId": self.side.session_id }));
        }
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let (cmd, shift) = (keys.modifiers.platform, keys.modifiers.shift);
        match (cmd, shift, keys.key.as_str()) {
            (false, false, "enter") => self.send(cx),
            (false, true, "enter") => {
                self.insert("\n");
            }
            (_, _, "backspace") => {
                if self.anchor == self.cursor {
                    self.anchor = if cmd {
                        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
                    } else {
                        self.previous(self.cursor)
                    };
                }
                self.insert("");
            }
            (false, _, "left" | "right") => {
                let selection = self.selection();
                let collapsing = !shift && !selection.is_empty();
                let left = keys.key == "left";
                self.cursor = match (collapsing, left) {
                    (true, true) => selection.start,
                    (true, false) => selection.end,
                    (false, true) => self.previous(self.cursor),
                    (false, false) => self.next(self.cursor),
                };
                if !shift {
                    self.anchor = self.cursor;
                }
            }
            (true, false, "a") => {
                self.anchor = 0;
                self.cursor = self.text.len();
            }
            (true, false, "c" | "x") => {
                let selected = self.text[self.selection()].to_owned();
                if !selected.is_empty() {
                    if keys.key == "x" {
                        self.insert("");
                    }
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                }
            }
            (true, false, "v") => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.insert(&text);
                }
            }
            // Letras e acentos chegam pelo input handler; atalhos do app seguem para o shell.
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn turns(&self) -> Vec<gpui::AnyElement> {
        let clock = self.epoch.elapsed().as_secs_f32();
        let mut rows = Vec::new();
        for turn in &self.side.turns {
            if !turn.user.is_empty() {
                rows.push(
                    div()
                        .w_full()
                        .flex()
                        .justify_end()
                        .child(
                            div()
                                .max_w(gpui::relative(0.88))
                                .px(px(10.))
                                .py(px(6.))
                                .rounded(px(crate::theme::RADIUS_LG))
                                .bg(accent_wash(0.16))
                                .text_size(px(13.))
                                .text_color(content(0.95))
                                .child(turn.user.clone()),
                        )
                        .into_any_element(),
                );
            }
            if !turn.answer.is_empty() {
                rows.push(
                    div()
                        .w_full()
                        .text_size(px(13.))
                        .text_color(content(0.9))
                        .child(crate::markdown::render(&turn.answer))
                        .into_any_element(),
                );
            }
            if turn.live {
                rows.push(
                    div()
                        .text_size(px(12.))
                        .child(shimmer(t("trabalhando").to_owned(), clock))
                        .into_any_element(),
                );
            }
        }
        rows
    }
}

impl EntityInputHandler for SideChat {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let (start, end) = (
            byte_at(&self.text, range.start),
            byte_at(&self.text, range.end),
        );
        *adjusted = Some(range);
        Some(self.text[start..end].to_owned())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let range = self.selection();
        Some(UTF16Selection {
            range: data::utf16_len(&self.text[..range.start])
                ..data::utf16_len(&self.text[..range.end]),
            reversed: self.cursor < self.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone()
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
        if let Some(range) = range.or(marked) {
            self.anchor = byte_at(&self.text, range.start);
            self.cursor = byte_at(&self.text, range.end);
        }
        self.insert(text);
        cx.notify();
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
        if let Some(range) = range.or(marked) {
            self.anchor = byte_at(&self.text, range.start);
            self.cursor = byte_at(&self.text, range.end);
        }
        let start = data::utf16_len(&self.text[..self.selection().start]);
        self.insert(new_text);
        self.marked = (!new_text.is_empty()).then(|| start..start + data::utf16_len(new_text));
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
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

impl Render for SideChat {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.focused = self.focus.is_focused(window);
        let working = self.side.working();
        let has_text = !self.text.trim().is_empty();
        let turns = self.turns();
        let empty = self.ready && turns.is_empty();

        let styled = StyledText::new(if self.text.is_empty() {
            " ".to_owned()
        } else {
            self.text.clone()
        });
        let layout = styled.layout().clone();
        *self.layout.borrow_mut() = Some(layout.clone());
        let (selection, caret_at, focused) = (self.selection(), self.cursor, self.focused);
        let (input_focus, input_view) = (self.focus.clone(), cx.entity());
        let overlay = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                window.handle_input(
                    &input_focus,
                    ElementInputHandler::new(bounds, input_view),
                    cx,
                );
                let line = layout.line_height();
                if !selection.is_empty() {
                    for at in selection.clone() {
                        if let (Some(start), Some(end)) = (
                            layout.position_for_index(at),
                            layout.position_for_index(at + 1),
                        ) {
                            if start.y == end.y && end.x > start.x {
                                window.paint_quad(gpui::fill(
                                    Bounds::new(start, gpui::size(end.x - start.x, line)),
                                    crate::theme::text_selection(),
                                ));
                            }
                        }
                    }
                } else if focused {
                    let origin = layout.position_for_index(caret_at).unwrap_or(bounds.origin);
                    window.paint_quad(
                        gpui::fill(
                            Bounds::new(origin, gpui::size(px(2.), line)),
                            crate::theme::caret_color(),
                        )
                        .corner_radii(px(1.)),
                    );
                }
            },
        )
        .absolute()
        .size_full();
        let shared_layout = self.layout.clone();
        let input = div()
            .relative()
            .w_full()
            .max_h(px(120.))
            .overflow_hidden()
            .text_size(px(13.))
            .text_color(content(0.95))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    let index = shared_layout
                        .borrow()
                        .as_ref()
                        .map(|l| l.index_for_position(event.position).unwrap_or_else(|i| i));
                    if let Some(index) = index {
                        this.cursor = index.min(this.text.len());
                        if !event.modifiers.shift {
                            this.anchor = this.cursor;
                        }
                        cx.notify();
                    }
                }),
            )
            .child(styled)
            .child(overlay)
            .when(self.text.is_empty(), |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .text_color(content(0.4))
                        .child(t("Pergunte sobre a conversa")),
                )
            });

        let send = div()
            .id("sidechat-send")
            .flex_shrink_0()
            .size(px(28.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .when(working, |d| {
                d.bg(accent())
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.88))
                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
                    .child(icon("square", 12.).text_color(on_accent()))
            })
            .when(!working && has_text && self.ready, |d| {
                d.bg(accent())
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.88))
                    .on_click(cx.listener(|this, _, _, cx| this.send(cx)))
                    .child(icon("arrow-up", 14.).text_color(on_accent()))
            })
            .when(!working && !(has_text && self.ready), |d| {
                d.bg(content(0.1))
                    .child(icon("arrow-up", 14.).text_color(content(0.35)))
            });

        div()
            .id("sirus-sidechat")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.key(event, cx)))
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("sidechat-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(px(12.))
                    .py(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .children(turns)
                    .when(empty, |d| {
                        d.child(
                            div().flex().flex_col().items_center().gap(px(4.)).pt(px(24.)).px(px(10.))
                                .child(div().text_size(px(13.)).text_color(content(0.8)).child(t("Pergunte sem interromper")))
                                .child(div().text_size(px(12.)).text_color(content(0.5)).text_center().child(t("Cada pergunta leva o que a sessão principal está fazendo agora. Começa em só leitura e usa o mesmo workspace."))),
                        )
                    })
                    .when(!self.ready && self.error.is_none(), |d| d.child(div().text_size(px(12.)).text_color(content(0.45)).child(t("Abrindo o chat lateral…"))))
                    .when_some(self.error.clone(), |d, error| d.child(div().text_size(px(12.)).text_color(content(0.6)).child(error))),
            )
            .child(
                div().flex_shrink_0().px(px(10.)).pb(px(10.)).child(
                    div()
                        .w_full()
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(if self.focused { accent_wash(0.5) } else { content(0.12) })
                        .bg(field_fill())
                        .pl(px(12.))
                        .pr(px(6.))
                        .py(px(6.))
                        .flex()
                        .items_end()
                        .gap(px(8.))
                        .child(div().flex_1().min_w_0().py(px(5.)).child(input))
                        .child(send),
                ),
            )
    }
}
