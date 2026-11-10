//! New project on a remote computer. There is no Mac folder picker for another computer's disk, so
//! the window offers what the phone does (ADR-131): clone `owner/repo` or an address, with an
//! optional folder name, or type the path of a folder that is already on the host. A child module
//! of the shell, so it reaches its private state.
use super::*;
use crate::panel_files::{TextField, TextFieldEvent};
use crate::theme::Sinks as _;
use gpui::{App, Entity, Subscription};

pub(super) struct RemoteProject {
    /// Clone from GitHub, or add a folder that is already there.
    clone: bool,
    source: Entity<TextField>,
    folder: Entity<TextField>,
    path: Entity<TextField>,
    /// Where a clone goes on the host.
    parent: String,
    error: Option<String>,
    busy: bool,
    open: Tween,
    /// The field to focus at the next frame (the dialog has no window when it opens).
    focus: bool,
    _events: Vec<Subscription>,
}

impl RemoteProject {
    pub fn focus_handle(&mut self, cx: &App) -> Option<FocusHandle> {
        if !std::mem::take(&mut self.focus) {
            return None;
        }
        let field = if self.clone { &self.source } else { &self.path };
        Some(field.read(cx).focus_handle().clone())
    }
}

impl Shell {
    pub(super) fn open_remote_project(&mut self, cx: &mut Context<Self>) {
        let source = cx.new(|cx| TextField::new(t("dono/repositório ou endereço"), cx));
        let folder = cx.new(|cx| TextField::new(t("Nome da pasta (opcional)"), cx));
        let path = cx.new(|cx| TextField::new("/home/usuario/projeto", cx));
        let answer = |this: &mut Shell, event: &TextFieldEvent, cx: &mut Context<Shell>| match event
        {
            TextFieldEvent::Submit => this.submit_remote_project(cx),
            TextFieldEvent::Cancel => this.close_remote_project(cx),
            TextFieldEvent::Changed => {
                if let Some(dialog) = this.remote_project.as_mut() {
                    dialog.error = None;
                }
                cx.notify();
            }
        };
        let events = vec![
            cx.subscribe(&source, move |this, _, event, cx| answer(this, event, cx)),
            cx.subscribe(&folder, move |this, _, event, cx| answer(this, event, cx)),
            cx.subscribe(&path, move |this, _, event, cx| answer(this, event, cx)),
        ];
        let recent = self
            .project_order
            .first()
            .and_then(|ix| self.projects.get(*ix))
            .map(|project| project.path.as_str());
        let length = if cx.reduce_motion() { 0. } else { 0.18 };
        self.remote_project = Some(RemoteProject {
            clone: true,
            source,
            folder,
            path,
            parent: crate::remote_host::clone_parent(recent),
            error: None,
            busy: false,
            open: Tween::from_to(0., 1., Instant::now(), length, Curve::Expo),
            focus: true,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_remote_project(&mut self, cx: &mut Context<Self>) {
        self.remote_project = None;
        self.refocus = true;
        cx.notify();
    }

    fn submit_remote_project(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.remote_project.as_mut() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let request = if dialog.clone {
            crate::remote_host::clone_args(
                dialog.source.read(cx).text(),
                dialog.folder.read(cx).text(),
                &dialog.parent,
            )
            .map(|args| ("clone_project", args))
        } else {
            crate::remote_host::folder_args(dialog.path.read(cx).text())
                .map(|args| ("add_project", args))
        };
        match request {
            Ok((command, args)) => {
                dialog.busy = true;
                dialog.error = None;
                let call = self.backend.invoke(command, args);
                self.waiting.insert(call, Waiting::RemoteProject);
            }
            Err(message) => dialog.error = Some(t(message).to_owned()),
        }
        cx.notify();
    }

    /// The host's answer to a clone or an added folder: the project joins the list, or the dialog
    /// says why not.
    pub(super) fn remote_project_answered(
        &mut self,
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        if ok {
            self.remote_project = None;
            self.adopt_project(value, cx);
        } else if let Some(dialog) = self.remote_project.as_mut() {
            dialog.busy = false;
            dialog.error = Some(error_text(value));
        }
        cx.notify();
    }

    /// Lists a project the host just added and opens it; one already listed is only opened.
    pub(super) fn adopt_project(&mut self, value: &Value, cx: &mut Context<Self>) {
        let id = value["id"].as_str().unwrap_or("");
        if let Some(ix) = self
            .projects
            .iter()
            .position(|project| !id.is_empty() && project.id == id)
        {
            if !self.project_order.contains(&ix) {
                self.project_order.insert(0, ix);
            }
            self.switch_project(ix, cx);
            return;
        }
        self.projects.push(Project {
            id: id.to_owned(),
            name: value["name"].as_str().unwrap_or("").to_owned(),
            path: value["path"].as_str().unwrap_or("").to_owned(),
            appearance: value["appearance"].clone(),
            scripts: crate::project_scripts::Saved::of(&value["scripts"]),
            open: true,
            conversations: Vec::new(),
        });
        let ix = self.projects.len() - 1;
        self.project_order.insert(0, ix);
        self.switch_project(ix, cx);
    }

    /// The dialog, centered over the window.
    pub(super) fn remote_project_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.remote_project.as_ref()?;
        *moving |= !dialog.open.done(now);
        let shown = dialog.open.at(now).clamp(0., 1.);
        let label = |text: &'static str| {
            div()
                .mt(px(14.))
                .mb(px(5.))
                .text_size(px(12.))
                .text_color(content(0.75))
                .child(text)
        };
        let field = |input: Entity<TextField>| {
            div()
                .h(px(34.))
                .px(px(10.))
                .rounded(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_color(crate::theme::border())
                .bg(field_fill())
                .flex()
                .items_center()
                .text_size(px(13.))
                .text_color(content(1.))
                .child(input)
        };
        let button = |id: &'static str, text: &'static str| {
            div()
                .id(id)
                .h(px(30.))
                .px(px(12.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .text_size(px(13.))
                .cursor_pointer()
                .sinks()
                .child(text)
        };
        let mode = |id: &'static str, text: &'static str, on: bool, clone: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px(px(10.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .text_size(px(12.))
                .cursor_pointer()
                .sinks()
                .text_color(content(if on { 1. } else { 0.6 }))
                .when(on, |d| d.bg(content(0.1)))
                .when(!on, |d| d.hover(|s| s.bg(content(0.06))))
                .child(text)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(dialog) = this.remote_project.as_mut() {
                        dialog.clone = clone;
                        dialog.error = None;
                        dialog.focus = true;
                    }
                    cx.notify();
                }))
        };
        let cloning = dialog.clone;
        Some(
            div()
                .id("remote-project-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::Hsla {
                    a: 0.45 * shown,
                    ..gpui::black()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_remote_project(cx);
                    }),
                )
                .child(
                    div()
                        .id("remote-project")
                        .relative()
                        .top(px((1. - shown) * 6.))
                        .w(px(420.))
                        .max_w(px((self.viewport_width - 24.).max(200.)))
                        .p(px(16.))
                        .flex()
                        .flex_col()
                        .opacity(shown)
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(crate::theme::border())
                        .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .relative()
                                .text_size(px(14.))
                                .text_color(content(1.))
                                .child(t("Novo projeto")),
                        )
                        .child(
                            div()
                                .relative()
                                .mt(px(4.))
                                .text_size(px(12.))
                                .text_color(content(0.6))
                                .child(t(
                                    "Neste computador não há seletor de pastas: clone um repositório ou diga onde a pasta está.",
                                )),
                        )
                        .child(
                            div()
                                .relative()
                                .mt(px(12.))
                                .flex()
                                .gap(px(4.))
                                .child(mode(
                                    "remote-mode-clone",
                                    t("Clonar do GitHub"),
                                    cloning,
                                    true,
                                ))
                                .child(mode(
                                    "remote-mode-folder",
                                    t("Pasta no computador"),
                                    !cloning,
                                    false,
                                )),
                        )
                        .when(cloning, |d| {
                            d.child(div().relative().child(label(t("Repositório"))))
                                .child(div().relative().child(field(dialog.source.clone())))
                                .child(div().relative().child(label(t("Nome da pasta"))))
                                .child(div().relative().child(field(dialog.folder.clone())))
                                .child(
                                    div()
                                        .relative()
                                        .mt(px(8.))
                                        .text_size(px(11.))
                                        .text_color(content(0.5))
                                        .child(crate::i18n::tf("Em {}.", &[&dialog.parent])),
                                )
                        })
                        .when(!cloning, |d| {
                            d.child(div().relative().child(label(t("Caminho da pasta"))))
                                .child(div().relative().child(field(dialog.path.clone())))
                        })
                        .when_some(dialog.error.clone(), |d, error| {
                            d.child(
                                div()
                                    .relative()
                                    .mt(px(10.))
                                    .text_size(px(12.))
                                    .text_color(rgb(0xe08a8a))
                                    .child(error),
                            )
                        })
                        .child(
                            div()
                                .relative()
                                .mt(px(16.))
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(div().flex_1())
                                .child(
                                    button("remote-project-cancel", t("Cancelar"))
                                        .text_color(content(0.85))
                                        .bg(content(0.08))
                                        .hover(|s| s.bg(content(0.12)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_remote_project(cx)
                                        })),
                                )
                                .child(
                                    button(
                                        "remote-project-add",
                                        t(if dialog.busy {
                                            "Adicionando…"
                                        } else if cloning {
                                            "Clonar"
                                        } else {
                                            "Adicionar"
                                        }),
                                    )
                                    .text_color(on_accent())
                                    .bg(accent())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.submit_remote_project(cx)
                                    })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
