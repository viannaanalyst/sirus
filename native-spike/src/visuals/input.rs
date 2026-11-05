//! A small single-line text field for `visual.input`: typing, backspace, paste.
use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, Hsla, IntoElement, Render, SharedString, Window,
    div, prelude::*, px,
};

pub(crate) struct Changed;

pub(crate) struct Input {
    pub content: SharedString,
    placeholder: SharedString,
    placeholder_color: Hsla,
    max_chars: usize,
    focus: FocusHandle,
}
impl EventEmitter<Changed> for Input {}
impl Focusable for Input {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Input {
    pub fn field(
        _label: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        placeholder_color: Hsla,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            content: SharedString::default(),
            placeholder: placeholder.into(),
            placeholder_color,
            max_chars: 4096,
            focus: cx.focus_handle(),
        }
    }
    pub fn set_max_chars(&mut self, max: usize) {
        self.max_chars = max;
    }
    /// Replaces the text from outside; this is not an edit by the person.
    pub fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.content = text.chars().take(self.max_chars).collect::<String>().into();
        cx.notify();
    }
    fn edit(&mut self, next: String, cx: &mut Context<Self>) {
        let next: String = next.chars().take(self.max_chars).collect();
        if next != self.content.as_ref() {
            self.content = next.into();
            cx.emit(Changed);
            cx.notify();
        }
    }
}

impl Render for Input {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let empty = self.content.is_empty();
        div()
            .id("visual-input-field")
            .track_focus(&self.focus)
            .key_context("VisualInput")
            .w_full()
            .min_h(px(20.))
            .flex()
            .items_center()
            .text_size(px(13.))
            .cursor_text()
            .on_click(cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                let key = &event.keystroke;
                let mut text = this.content.to_string();
                if key.modifiers.platform || key.modifiers.control {
                    if key.key == "v" {
                        if let Some(clip) = cx.read_from_clipboard().and_then(|c| c.text()) {
                            text.push_str(&clip.replace(['\n', '\r'], " "));
                            this.edit(text, cx);
                            cx.stop_propagation();
                        }
                    }
                    return;
                }
                match key.key.as_str() {
                    "backspace" => {
                        text.pop();
                    }
                    "escape" | "tab" | "enter" => return,
                    _ => match key.key_char.as_deref() {
                        Some(typed) if !typed.chars().any(char::is_control) => text.push_str(typed),
                        _ => return,
                    },
                }
                this.edit(text, cx);
                cx.stop_propagation();
            }))
            .child(if empty {
                div()
                    .text_color(self.placeholder_color)
                    .child(self.placeholder.clone())
            } else {
                div().child(self.content.clone())
            })
            .when(focused, |d| {
                d.child(
                    div()
                        .w(px(1.))
                        .h(px(15.))
                        .ml(px(1.))
                        .bg(crate::theme::accent()),
                )
            })
    }
}
