//! The right-click menu of a text field: Recortar, Copiar, Colar, Selecionar tudo. Each item
//! runs the field's own shortcut, so the menu does exactly what ⌘X, ⌘C, ⌘V and ⌘A do there.

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Point,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::i18n::t;
use crate::theme::content;

/// Where the menu is open and which of its items can act.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Open {
    pub at: Point<Pixels>,
    pub selection: bool,
    pub editable: bool,
}

/// The shortcut each item stands for.
pub(crate) const ITEMS: [(&str, &str, &str); 4] = [
    ("edit-cut", "Recortar", "cmd-x"),
    ("edit-copy", "Copiar", "cmd-c"),
    ("edit-paste", "Colar", "cmd-v"),
    ("edit-select-all", "Selecionar tudo", "cmd-a"),
];

/// The key event a shortcut would have sent.
pub(crate) fn keystroke(shortcut: &str) -> gpui::KeyDownEvent {
    gpui::KeyDownEvent {
        keystroke: gpui::Keystroke::parse(shortcut).expect("a fixed shortcut"),
        is_held: false,
        prefer_character_input: false,
    }
}

fn hint(shortcut: &str) -> &'static str {
    match shortcut {
        "cmd-x" => "⌘X",
        "cmd-c" => "⌘C",
        "cmd-v" => "⌘V",
        _ => "⌘A",
    }
}

/// The menu, floated over everything; `pick` runs with the chosen shortcut, `dismiss` on a click
/// outside.
pub(crate) fn view(
    open: Open,
    pick: impl Fn(&'static str, &mut Window, &mut App) + Clone + 'static,
    dismiss: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let rows = ITEMS.iter().map(|(id, label, shortcut)| {
        let enabled = match *shortcut {
            "cmd-x" => open.selection && open.editable,
            "cmd-c" => open.selection,
            "cmd-v" => open.editable,
            _ => true,
        };
        let pick = pick.clone();
        let shortcut: &'static str = shortcut;
        div()
            .id(*id)
            .h(px(28.))
            .px(px(8.))
            .rounded(px(crate::theme::RADIUS_MD))
            .flex()
            .items_center()
            .gap(px(16.))
            .text_size(px(13.))
            .text_color(content(if enabled { 0.92 } else { 0.35 }))
            .when(enabled, move |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(content(0.08)))
                    .on_click(move |_, window, cx| pick(shortcut, window, cx))
            })
            .child(div().flex_1().child(t(label)))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(content(0.4))
                    .child(hint(shortcut)),
            )
    });
    gpui::deferred(
        gpui::anchored().position(open.at).snap_to_window().child(
            div()
                .occlude()
                .on_mouse_down_out(move |_, window, cx| dismiss(window, cx))
                .w(px(200.))
                .p(px(4.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.1))
                .shadow_lg()
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .flex()
                .flex_col()
                .children(rows),
        ),
    )
    .with_priority(6)
    .into_any_element()
}
