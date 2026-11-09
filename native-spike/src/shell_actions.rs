//! The footer's Actions button, its menu and the Add/Edit dialog (`project_actions.rs` holds what
//! they keep). A child module of the shell, so it reaches its private state.
use super::*;
use crate::panel_files::{TextField, TextFieldEvent};
use crate::project_actions::{self, Action, ICONS, Saved};
use crate::terminal_place::Placement;
use crate::theme::Sinks as _;
use gpui::{Entity, Subscription};

pub(super) struct ActionsUi {
    pub saved: Saved,
    /// The button's menu: its fade-in, and whether "Editar ação" has its list open beside it.
    menu: Option<Tween>,
    editing_list: bool,
    /// Where the button and the "Editar ação" row were last drawn.
    button: Rc<Cell<Option<Bounds<Pixels>>>>,
    edit_row: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub dialog: Option<ActionDialog>,
}

pub(super) struct ActionDialog {
    /// The action being edited, or `None` for a new one.
    editing: Option<String>,
    name: Entity<TextField>,
    command: Entity<TextField>,
    icon: &'static str,
    missing: bool,
    open: Tween,
    _events: Vec<Subscription>,
}

impl ActionsUi {
    pub fn new() -> Self {
        Self {
            saved: project_actions::load(),
            menu: None,
            editing_list: false,
            button: Default::default(),
            edit_row: Default::default(),
            dialog: None,
        }
    }
    pub fn covers_pages(&self) -> bool {
        self.menu.is_some() || self.dialog.is_some()
    }
}

impl Shell {
    fn actions_project(&self) -> Option<String> {
        self.current_project
            .and_then(|ix| self.projects.get(ix))
            .map(|project| project.id.clone())
    }

    fn close_actions_menu(&mut self, cx: &mut Context<Self>) {
        self.actions.menu = None;
        self.actions.editing_list = false;
        cx.notify();
    }

    /// Runs an action in a terminal of its own, reused while it sits idle at a prompt.
    fn run_project_action(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.this_mac_only(cx) {
            return;
        }
        let Some(project) = self.actions_project() else {
            return;
        };
        let Some(action) = self.actions.saved.find(&project, id).cloned() else {
            return;
        };
        let reveal = self.actions.saved.opens_terminal;
        let key = format!("{project}:{id}");
        if self.term.placement == Placement::TitleBar {
            self.run_in_title_tab(&key, &action.command, reveal, window, cx);
        } else {
            self.ensure_pane_view(0, cx);
            let Some(view) = self.conversation_view(0) else {
                return;
            };
            crate::panel_terminal::run_in(&view, &key, &action.command, reveal, window, cx);
            if reveal {
                self.term_reveal_panel(window, cx);
            }
        }
        cx.notify();
    }

    fn open_action_dialog(
        &mut self,
        editing: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.actions_project() else {
            return;
        };
        self.actions.menu = None;
        self.actions.editing_list = false;
        let existing = editing
            .as_ref()
            .and_then(|id| self.actions.saved.find(&project, id))
            .cloned();
        let name = cx.new(|cx| TextField::new(t("Nome da ação"), cx));
        let command = cx.new(|cx| TextField::new("npm test", cx));
        if let Some(action) = &existing {
            name.update(cx, |field, cx| field.set_text(&action.name, cx));
            command.update(cx, |field, cx| field.set_text(&action.command, cx));
        }
        let answer = |this: &mut Shell, event: &TextFieldEvent, cx: &mut Context<Shell>| match event
        {
            TextFieldEvent::Submit => this.save_action(cx),
            TextFieldEvent::Cancel => this.close_action_dialog(cx),
            TextFieldEvent::Changed => {
                if let Some(dialog) = this.actions.dialog.as_mut() {
                    dialog.missing = false;
                }
                cx.notify();
            }
        };
        let events = vec![
            cx.subscribe(&name, move |this, _, event, cx| answer(this, event, cx)),
            cx.subscribe(&command, move |this, _, event, cx| answer(this, event, cx)),
        ];
        let focus = name.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
        let length = if cx.reduce_motion() { 0. } else { 0.18 };
        self.actions.dialog = Some(ActionDialog {
            icon: existing.as_ref().map_or(ICONS[0], Action::icon),
            editing: existing.map(|action| action.id),
            name,
            command,
            missing: false,
            open: Tween::from_to(0., 1., Instant::now(), length, Curve::Expo),
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_action_dialog(&mut self, cx: &mut Context<Self>) {
        self.actions.dialog = None;
        self.refocus = true;
        cx.notify();
    }

    fn save_action(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.actions_project() else {
            return;
        };
        let Some(dialog) = self.actions.dialog.as_mut() else {
            return;
        };
        let name = dialog.name.read(cx).text().trim().to_owned();
        let command = dialog.command.read(cx).text().trim().to_owned();
        if name.is_empty() || command.is_empty() {
            dialog.missing = true;
            cx.notify();
            return;
        }
        let action = Action {
            id: dialog.editing.clone().unwrap_or_else(uuid),
            name,
            command,
            icon: dialog.icon.to_owned(),
        };
        self.actions.saved.put(&project, action);
        project_actions::save(&self.actions.saved);
        self.close_action_dialog(cx);
    }

    fn delete_action(&mut self, cx: &mut Context<Self>) {
        let id = self
            .actions
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.editing.clone());
        if let (Some(id), Some(project)) = (id, self.actions_project()) {
            self.actions.saved.remove(&project, &id);
            project_actions::save(&self.actions.saved);
        }
        self.close_action_dialog(cx);
    }

    /// The footer's button, the size of its neighbours: everything is in the menu it opens.
    pub(super) fn actions_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.actions_project()?;
        let open = self.actions.menu.is_some();
        let store = self.actions.button.clone();
        Some(
            div()
                .id("footer-actions")
                .relative()
                .size(px(24.))
                .flex_shrink_0()
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .sinks()
                .when(open, |d| d.bg(content(0.08)))
                .hover(|s| s.bg(content(0.08)))
                .tooltip(message_actions::name_tip(t("Ações")))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.actions.editing_list = false;
                    this.actions.menu = match this.actions.menu {
                        Some(_) => None,
                        None => {
                            let length = if cx.reduce_motion() { 0. } else { MENU };
                            Some(Tween::from_to(0., 1., Instant::now(), length, Curve::Expo))
                        }
                    };
                    cx.notify();
                }))
                .child(
                    gpui::canvas(move |bounds, _, _| store.set(Some(bounds)), |_, _, _, _| {})
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                )
                .child(icon("play", 16.).text_color(content(0.6)))
                .into_any_element(),
        )
    }

    /// The button's menu, above it, with "Editar ação" listing the actions beside it.
    pub(super) fn actions_menu_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.actions.menu.as_ref()?;
        let anchor = self.actions.button.get()?;
        let project = self.actions_project()?;
        *moving |= !open.done(now);
        let shown = open.at(now).clamp(0., 1.);
        let actions = self.actions.saved.of(&project).to_vec();
        const WIDTH: f32 = 250.;
        let left = (f32::from(anchor.right()) - WIDTH)
            .min(self.viewport_width - WIDTH - 8.)
            .max(8.);
        let bottom = (self.viewport_height - f32::from(anchor.top()) + 6.).max(0.);
        let row = |glyph: &'static str, label: String| {
            div()
                .relative()
                .h(px(28.))
                .px(px(8.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(9.))
                .text_size(px(13.))
                .text_color(content(0.92))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)))
                .child(icon(glyph, 14.).text_color(content(0.65)))
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        let shut = |this: &mut Self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>| {
            if *hovered && std::mem::take(&mut this.actions.editing_list) {
                cx.notify();
            }
        };
        let runs: Vec<AnyElement> = actions
            .iter()
            .enumerate()
            .map(|(at, action)| {
                let id = action.id.clone();
                row(action.icon(), action.name.clone())
                    .id(("action-run", at))
                    .on_hover(cx.listener(shut))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_actions_menu(cx);
                        this.run_project_action(&id, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        let line = || div().mx(px(4.)).my(px(4.)).h(px(1.)).bg(content(0.08));
        let store = self.actions.edit_row.clone();
        let edit = (!actions.is_empty()).then(|| {
            row("pencil", t("Editar ação").to_owned())
                .id("action-edit")
                .when(self.actions.editing_list, |d| d.bg(content(0.07)))
                .child(
                    gpui::canvas(move |bounds, _, _| store.set(Some(bounds)), |_, _, _, _| {})
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                )
                .child(icon("chevron-right", 12.).text_color(content(0.5)))
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if *hovered && !this.actions.editing_list {
                        this.actions.editing_list = true;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.actions.editing_list = true;
                    cx.notify();
                }))
        });
        let opens = self.actions.saved.opens_terminal;
        let card = |id: &'static str| {
            div()
                .id(id)
                .occlude()
                .absolute()
                .p(px(4.))
                .opacity(shown)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        let flyout = self
            .actions
            .editing_list
            .then(|| self.actions.edit_row.get())
            .flatten()
            .map(|at| {
                const LIST: f32 = 190.;
                let beside = if left + WIDTH + 6. + LIST <= self.viewport_width - 8. {
                    left + WIDTH + 6.
                } else {
                    (left - LIST - 6.).max(8.)
                };
                let tall = actions.len() as f32 * 28. + 10.;
                let top = (f32::from(at.top()) - 5.)
                    .min(self.viewport_height - tall - 8.)
                    .max(8.);
                card("action-edit-list")
                    .left(px(beside))
                    .top(px(top))
                    .w(px(LIST))
                    .children(actions.iter().enumerate().map(|(at, action)| {
                        let id = action.id.clone();
                        row(action.icon(), action.name.clone())
                            .id(("action-edit-row", at))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_action_dialog(Some(id.clone()), window, cx)
                            }))
                    }))
            });
        Some(
            div()
                .id("actions-scrim")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.close_actions_menu(cx)),
                )
                .child(
                    card("actions-menu")
                        .left(px(left))
                        .bottom(px(bottom))
                        .w(px(WIDTH))
                        .children(runs)
                        .when(!actions.is_empty(), |d| d.child(line()))
                        .children(edit)
                        .child(
                            row("plus", t("Adicionar ação").to_owned())
                                .id("action-add")
                                .on_hover(cx.listener(shut))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_action_dialog(None, window, cx)
                                })),
                        )
                        .child(line())
                        .child(
                            row("square-terminal", t("Mostrar terminal ao rodar").to_owned())
                                .id("action-opens")
                                .on_hover(cx.listener(shut))
                                .when(opens, |d| {
                                    d.child(icon("check", 12.).text_color(content(0.7)))
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let saved = &mut this.actions.saved;
                                    saved.opens_terminal = !saved.opens_terminal;
                                    project_actions::save(saved);
                                    cx.notify();
                                })),
                        ),
                )
                .children(flyout)
                .into_any_element(),
        )
    }

    /// The Add/Edit dialog, centered over the window.
    pub(super) fn action_dialog_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.actions.dialog.as_ref()?;
        *moving |= !dialog.open.done(now);
        let shown = dialog.open.at(now).clamp(0., 1.);
        let editing = dialog.editing.is_some();
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
        let icons = ICONS.iter().copied().enumerate().map(|(at, glyph)| {
            let on = glyph == dialog.icon;
            div()
                .id(("action-icon", at))
                .size(px(34.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_color(if on { accent() } else { content(0.1) })
                .when(on, |d| d.bg(content(0.08)))
                .when(!on, |d| d.hover(|s| s.bg(content(0.06))))
                .cursor_pointer()
                .child(icon(glyph, 16.).text_color(content(if on { 1. } else { 0.6 })))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(dialog) = this.actions.dialog.as_mut() {
                        dialog.icon = glyph;
                    }
                    cx.notify();
                }))
        });
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
        Some(
            div()
                .id("action-backdrop")
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
                        this.close_action_dialog(cx);
                    }),
                )
                .child(
                    div()
                        .id("action-dialog")
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
                                .child(t(if editing {
                                    "Editar ação"
                                } else {
                                    "Adicionar ação"
                                })),
                        )
                        .when(!editing, |d| {
                            d.child(
                                div()
                                    .relative()
                                    .mt(px(4.))
                                    .text_size(px(12.))
                                    .text_color(content(0.6))
                                    .child(t(
                                        "Salve um comando do projeto para rodar com um clique.",
                                    )),
                            )
                        })
                        .child(div().relative().child(label(t("Nome"))))
                        .child(div().relative().child(field(dialog.name.clone())))
                        .child(div().relative().child(label(t("Comando"))))
                        .child(div().relative().child(field(dialog.command.clone())))
                        .child(div().relative().child(label(t("Ícone"))))
                        .child(div().relative().flex().gap(px(6.)).children(icons))
                        .when(dialog.missing, |d| {
                            d.child(
                                div()
                                    .relative()
                                    .mt(px(10.))
                                    .text_size(px(12.))
                                    .text_color(rgb(0xe08a8a))
                                    .child(t("Dê um nome e um comando à ação.")),
                            )
                        })
                        .child(
                            div()
                                .relative()
                                .mt(px(16.))
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .when(editing, |d| {
                                    d.child(
                                        button("action-delete", t("Excluir"))
                                            .text_color(rgb(0xe08a8a))
                                            .hover(|s| s.bg(content(0.07)))
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.delete_action(cx)
                                                }),
                                            ),
                                    )
                                })
                                .child(div().flex_1())
                                .child(
                                    button("action-cancel", t("Cancelar"))
                                        .text_color(content(0.85))
                                        .bg(content(0.08))
                                        .hover(|s| s.bg(content(0.12)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_action_dialog(cx)
                                        })),
                                )
                                .child(
                                    button("action-save", t("Salvar"))
                                        .text_color(on_accent())
                                        .bg(accent())
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.save_action(cx)),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
