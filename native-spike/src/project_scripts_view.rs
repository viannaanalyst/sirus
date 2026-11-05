//! Drawing of the project scripts (ADR-078): the fields in the project's settings popup, the run
//! notice above the composer and the run rows of the composer's add menu. What each one means is
//! decided in `project_scripts.rs`; the handlers come from the shell, which owns the state.

use crate::i18n::{t, tf};
use crate::project_scripts::{self as scripts, Draft, Field, Kind, Notice, Run, Status, Tone};
use crate::theme::{accent, accent_wash, content, field_fill, icon};
use gpui::{AnyElement, App, ClickEvent, Context, Window, div, prelude::*, px};

/// A click handler the shell builds with `cx.listener`.
pub type Handler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The height the Scripts block adds to the settings popup (its own area scrolls inside).
pub const SETTINGS_HEIGHT: f32 = 380.;

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

/// The Portuguese source of a script's name in the popup and the add menu.
pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Setup => "Preparação",
        Kind::Finish => "Ao terminar",
    }
}

fn kind_help(kind: Kind) -> &'static str {
    match kind {
        Kind::Setup => "Roda em um worktree novo antes do primeiro turno (até 10 minutos).",
        Kind::Finish => "Roda no worktree sempre que um turno termina (até 2 minutos).",
    }
}

fn kind_placeholder(kind: Kind) -> &'static str {
    match kind {
        Kind::Setup => "npm install",
        Kind::Finish => "npm run lint -- --fix",
    }
}

/// A text field of the popup. Multi-line fields show every line of their text, up to a height.
fn text_box(
    id: (&'static str, usize),
    value: &str,
    placeholder: &'static str,
    focused: bool,
    multiline: bool,
    on_click: Handler,
) -> AnyElement {
    let shown = if value.is_empty() {
        div()
            .text_color(content(0.35))
            .child(t(placeholder))
            .into_any_element()
    } else {
        let lines = value.split('\n').map(|line| div().child(line.to_owned()));
        div().flex().flex_col().children(lines).into_any_element()
    };
    div()
        .id(id)
        .w_full()
        .min_h(px(if multiline { 58. } else { 30. }))
        .max_h(px(if multiline { 96. } else { 30. }))
        .overflow_hidden()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(8.))
        .border_1()
        .border_color(if focused { accent() } else { content(0.14) })
        .bg(field_fill())
        .text_size(px(12.))
        .text_color(content(0.9))
        .cursor_pointer()
        .on_click(on_click)
        .child(shown)
        .when(focused, |d| {
            d.child(
                div()
                    .w(px(1.5))
                    .h(px(13.))
                    .rounded_full()
                    .bg(crate::theme::caret_color()),
            )
        })
        .into_any_element()
}

fn caption(text: impl Into<String>, danger: bool) -> AnyElement {
    div()
        .text_size(px(11.))
        .text_color(if danger {
            gpui::rgb(0xe08a8a).into()
        } else {
            content(0.45)
        })
        .child(text.into())
        .into_any_element()
}

/// The Scripts block of the project's settings popup. `handlers` are the field focus handlers in
/// `Field::ALL` order; `toggle` flips open-preview and `save` writes the draft, which only
/// `can_save` allows (changed and valid).
pub fn fields(
    draft: &Draft,
    focus: Option<Field>,
    handlers: [Handler; 3],
    toggle: Handler,
    save: Handler,
    can_save: bool,
) -> AnyElement {
    let [setup_click, finish_click, preview_click] = handlers;
    let over_limit = draft.setup.len() > scripts::MAX_SCRIPT_BYTES
        || draft.finish.len() > scripts::MAX_SCRIPT_BYTES;
    let preview_ok = scripts::preview_valid(&draft.preview);
    let script_block = |kind: Kind, field: Field, click: Handler| {
        let value = draft.text(field);
        div()
            .mt(px(8.))
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.8))
                    .child(t(kind_name(kind))),
            )
            .child(text_box(
                ("project-script-field", field as usize),
                value,
                kind_placeholder(kind),
                focus == Some(field),
                true,
                click,
            ))
            .child(caption(t(kind_help(kind)), false))
    };
    let toggle_row = div()
        .id("project-open-preview")
        .mt(px(8.))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .text_color(content(0.8))
        .when(draft.open_preview_allowed(), |d| {
            d.cursor_pointer().on_click(toggle)
        })
        .when(!draft.open_preview_allowed(), |d| d.opacity(0.4))
        .child(
            div()
                .size(px(13.))
                .rounded(px(3.))
                .border_1()
                .border_color(content(0.3))
                .flex()
                .items_center()
                .justify_center()
                .when(draft.open_preview && draft.open_preview_allowed(), |d| {
                    d.bg(accent_wash(0.8))
                }),
        )
        .child(t("Abrir a prévia quando responder"));
    let save_row = div()
        .id("project-scripts-save")
        .mt(px(10.))
        .h(px(30.))
        .px(px(10.))
        .rounded(px(8.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.5))
        .when(can_save, |d| {
            d.bg(accent_wash(0.16))
                .text_color(content(0.95))
                .cursor_pointer()
                .on_click(save)
        })
        .when(!can_save, |d| d.opacity(0.4).text_color(content(0.6)))
        .child(t("Salvar scripts"));
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(12.5))
                .text_color(content(0.9))
                .child(t("Scripts")),
        )
        .child(caption(
            t("Rodam no worktree próprio de cada sessão, como você, igual ao terminal. $SIRUS_PROJECT_ROOT é a pasta deste projeto."),
            false,
        ))
        .child(script_block(Kind::Setup, Field::Setup, setup_click))
        .child(script_block(Kind::Finish, Field::Finish, finish_click))
        .child(
            div()
                .mt(px(10.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.8))
                        .child(t("Endereço da prévia")),
                )
                .child(text_box(
                    ("project-script-field", Field::Preview as usize),
                    &draft.preview,
                    "http://localhost:5173",
                    focus == Some(Field::Preview),
                    false,
                    preview_click,
                ))
                .child(caption(
                    if preview_ok {
                        t("Um endereço localhost que o script serve, como http://localhost:5173.")
                            .to_owned()
                    } else {
                        t("Use um endereço http(s) localhost com porta.").to_owned()
                    },
                    !preview_ok,
                )),
        )
        .child(toggle_row)
        .child(caption(
            t("Quando um script inicia e o endereço responde, ele abre no navegador da sessão."),
            false,
        ))
        .when(over_limit, |d| {
            d.child(caption(
                t("Cada script tem até 8 KiB."),
                true,
            ))
        })
        .child(save_row)
        .into_any_element()
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
            .rounded(px(7.))
            .flex()
            .items_center()
            .gap(px(5.))
            .text_size(px(11.5))
            .text_color(content(0.8))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.08)))
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
                .rounded(px(7.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
                .on_click(dismiss)
                .child(icon("close", 12.).text_color(content(0.6))),
        );
    }
    let mut card = div()
        .w_full()
        .mb(px(8.))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(16.))
        .border_1()
        .border_color(content(0.1))
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
                .rounded(px(8.))
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
        .rounded_md()
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

/// The row that unfolds the Scripts block in the settings popup.
pub fn settings_row(id: &'static str, expanded: bool, on_click: Handler) -> AnyElement {
    div()
        .id(id)
        .h(px(32.))
        .px(px(8.))
        .rounded(px(8.))
        .flex()
        .items_center()
        .gap(px(10.))
        .text_size(px(13.))
        .text_color(content(0.92))
        .cursor_pointer()
        .hover(|s| s.bg(content(0.09)))
        .on_click(on_click)
        .child(icon("terminal-square", 15.).text_color(content(0.65)))
        .child(div().flex_1().child(t("Scripts")))
        .child(
            icon(
                if expanded {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                12.,
            )
            .text_color(content(0.45)),
        )
        .into_any_element()
}
