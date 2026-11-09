//! Drawing of the project scripts (ADR-078): the fields in the project's settings popup, the run
//! notice above the composer and the run rows of the composer's add menu. What each one means is
//! decided in `project_scripts.rs`; the handlers come from the shell, which owns the state.

use crate::i18n::{t, tf};
use crate::project_scripts::{Kind, Notice, Run, Status, Tone};
use crate::theme::{accent, content, icon};
use gpui::{AnyElement, App, ClickEvent, Context, Window, div, prelude::*, px};

/// A click handler the shell builds with `cx.listener`.
pub type Handler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A click handler that runs `f` on the view `V`.
pub fn click<V: 'static>(
    cx: &mut Context<V>,
    f: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> Handler {
    let listener = cx.listener(move |this, _: &ClickEvent, _, cx| f(this, cx));
    Box::new(listener)
}

/// The Portuguese source of a run's title (`scripts.<status>.<kind>` in the React app).
pub fn title_of(kind: Kind, status: Status) -> &'static str {
    match (kind, status) {
        (Kind::Setup, Status::Running) => "Rodando o script de preparação…",
        (Kind::Finish, Status::Running) => "Rodando o script de término…",
        (Kind::Setup, Status::Succeeded) => "Script de preparação concluído",
        (Kind::Finish, Status::Succeeded) => "Script de término concluído",
        (Kind::Setup, Status::Failed) => "Script de preparação falhou",
        (Kind::Finish, Status::Failed) => "Script de término falhou",
        (Kind::Setup, Status::TimedOut) => "Script de preparação passou do tempo",
        (Kind::Finish, Status::TimedOut) => "Script de término passou do tempo",
        (Kind::Setup, Status::Cancelled) => "Script de preparação interrompido",
        (Kind::Finish, Status::Cancelled) => "Script de término interrompido",
    }
}

/// The notice above the composer for one run. `handlers` are stop, run again, dismiss and show
/// output. `output_open` shows the run's output when the run has ended.
pub fn notice(run: &Run, shown: Notice, output_open: bool, handlers: [Handler; 4]) -> AnyElement {
    let [stop, run_again, dismiss, toggle] = handlers;
    let title = title_of(run.kind, run.status);
    let mut details: Vec<String> = Vec::new();
    if let Some(code) = shown.exit_code {
        details.push(tf("saída {}", &[&code]));
    }
    if shown.continues {
        details.push(t("A conversa continua.").to_owned());
    }
    let (glyph, tint) = match shown.tone {
        Tone::Working => ("refresh", accent()),
        Tone::Done => ("check", content(0.8)),
        Tone::Stopped => ("square", content(0.5)),
        Tone::Failed => ("close", gpui::rgb(0xe08a8a).into()),
    };
    let button = |label: &'static str, glyph: &'static str| {
        div()
            .h(px(24.))
            .px(px(8.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .gap(px(5.))
            .text_size(px(11.5))
            .text_color(content(0.8))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.07)))
            .child(icon(glyph, 12.).text_color(content(0.6)))
            .child(t(label))
    };
    let mut actions = div().flex().items_center().gap(px(4.));
    if shown.output {
        actions = actions.child(
            button(
                if output_open {
                    "Ocultar saída"
                } else {
                    "Ver saída"
                },
                if output_open {
                    "chevron-up"
                } else {
                    "chevron-down"
                },
            )
            .id("script-output-toggle")
            .on_click(toggle),
        );
    }
    if shown.stop {
        actions = actions.child(
            button("Parar script", "square-fill")
                .id("script-stop")
                .on_click(stop),
        );
    }
    if shown.run_again {
        let enabled = shown.run_again_enabled;
        actions = actions.child(
            button("Rodar de novo", "play")
                .id("script-run-again")
                .when(!enabled, |d| d.opacity(0.4))
                .when(enabled, |d| d.on_click(run_again)),
        );
    }
    if shown.dismiss {
        actions = actions.child(
            div()
                .id("script-dismiss")
                .size(px(24.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)))
                .on_click(dismiss)
                .child(icon("close", 12.).text_color(content(0.6))),
        );
    }
    let mut card = div()
        .w_full()
        .mb(px(8.))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(crate::theme::RADIUS_LG))
        .border_1()
        .border_color(crate::theme::border())
        .bg(content(0.03))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(icon(glyph, 15.).text_color(tint))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(content(0.95))
                        .child(t(title)),
                )
                .when(!details.is_empty(), |d| {
                    d.child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(content(0.45))
                            .child(details.join(" · ")),
                    )
                })
                .when(details.is_empty(), |d| d.child(div().flex_1()))
                .child(actions),
        );
    if shown.output && output_open {
        let body = if run.output.trim().is_empty() {
            t("Sem saída.").to_owned()
        } else {
            run.output.clone()
        };
        card = card.child(
            div()
                .mt(px(8.))
                .max_h(px(224.))
                .overflow_hidden()
                .p(px(8.))
                .rounded(px(crate::theme::RADIUS_MD))
                .bg(content(0.05))
                .text_size(px(11.5))
                .text_color(content(0.7))
                .child(body),
        );
    }
    card.into_any_element()
}

/// A run row of the composer's add menu, styled as the menu's other rows.
pub fn run_row(
    id: &'static str,
    kind: Kind,
    about: impl Into<String>,
    enabled: bool,
    on_click: Handler,
) -> AnyElement {
    let title = match kind {
        Kind::Setup => "Rodar preparação",
        Kind::Finish => "Rodar ao terminar",
    };
    let row = div()
        .id(id)
        .h(px(32.))
        .px(px(8.))
        .rounded(px(crate::theme::RADIUS_MD))
        .flex()
        .items_center()
        .gap(px(10.))
        .text_size(px(13.))
        .text_color(content(0.92))
        .when(enabled, |d| d.cursor_pointer().on_click(on_click))
        .when(!enabled, |d| d.opacity(0.4))
        .child(icon("terminal-square", 15.).text_color(content(0.6)))
        .child(div().flex_shrink_0().child(t(title)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .text_color(content(0.45))
                .child(about.into()),
        );
    // Part of the add menu's sliding pill, when it can be clicked.
    if enabled {
        crate::hover_track::list("add-menu")
            .row_named(row, id, false)
            .into_any_element()
    } else {
        row.into_any_element()
    }
}
