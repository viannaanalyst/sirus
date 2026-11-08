//! The quick composer: ⌥Space from any app, or the Sirus icon in the menu bar, opens a small
//! floating field. Enter starts a new conversation with the text in the chosen project and the
//! field goes away; the conversation runs in Sirus behind whatever app was in front. Escape or
//! clicking elsewhere closes it. Settings → Geral → "Composer rápido (⌥Espaço)" turns it off.

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Subscription, Window, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, point, px, size,
};

use crate::i18n::t;
use crate::panel_files::{TextField, TextFieldEvent};
use crate::shell::Shell;
use crate::theme::content;

struct QuickComposer {
    field: Entity<TextField>,
    /// `(id, name)`, the open project first.
    projects: Vec<(String, String)>,
    chosen: usize,
    shell: WindowHandle<Shell>,
    _events: Subscription,
    _activation: Option<Subscription>,
}

thread_local! {
    static OPEN: std::cell::RefCell<Option<gpui::AnyWindowHandle>> = const { std::cell::RefCell::new(None) };
}

/// The menu bar icon: a composer box with a caret, drawn as a template so macOS tints it.
fn tray_image() -> tray_icon::Icon {
    const S: usize = 36;
    let mut rgba = vec![0u8; S * S * 4];
    for y in 0..S {
        for x in 0..S {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // A rounded rectangle 30×22 centred, 2.5 px stroke.
            let (cx, cy, hw, hh, r) = (18., 18., 15., 11., 6.);
            let qx = (fx - cx).abs() - (hw - r);
            let qy = (fy - cy).abs() - (hh - r);
            let outside = (qx.max(0.).powi(2) + qy.max(0.).powi(2)).sqrt() + qx.max(qy).min(0.) - r;
            let ring = (1.25 - outside.abs()).clamp(0., 1.);
            // The caret: a short vertical bar at the left.
            let caret = if (10.5..13.).contains(&fx) && (12.5..23.5).contains(&fy) {
                1.
            } else {
                0.
            };
            let alpha = ring.max(caret);
            let at = (y * S + x) * 4;
            rgba[at..at + 4].copy_from_slice(&[0, 0, 0, (alpha * 255.) as u8]);
        }
    }
    tray_icon::Icon::from_rgba(rgba, S as u32, S as u32).expect("a fixed icon")
}

/// Registers ⌥Space and puts the icon in the menu bar. Both live as long as the app.
pub(crate) fn start(shell: WindowHandle<Shell>, cx: &mut App) {
    let (tx, rx) = async_channel::unbounded::<()>();
    let hotkey = global_hotkey::hotkey::HotKey::new(
        Some(global_hotkey::hotkey::Modifiers::ALT),
        global_hotkey::hotkey::Code::Space,
    );
    if let Ok(manager) = global_hotkey::GlobalHotKeyManager::new()
        && manager.register(hotkey).is_ok()
    {
        let wanted = hotkey.id();
        let sender = tx.clone();
        global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
            move |event: global_hotkey::GlobalHotKeyEvent| {
                if event.id == wanted && event.state == global_hotkey::HotKeyState::Pressed {
                    let _ = sender.try_send(());
                }
            },
        ));
        std::mem::forget(manager);
    }
    if let Ok(tray) = tray_icon::TrayIconBuilder::new()
        .with_icon(tray_image())
        .with_icon_as_template(true)
        .with_tooltip(t("Composer rápido do Sirus (⌥Espaço)"))
        .build()
    {
        let sender = tx;
        tray_icon::TrayIconEvent::set_event_handler(Some(
            move |event: tray_icon::TrayIconEvent| {
                if let tray_icon::TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } = event
                {
                    let _ = sender.try_send(());
                }
            },
        ));
        std::mem::forget(tray);
    }
    cx.spawn(async move |cx| {
        while rx.recv().await.is_ok() {
            let _ = cx.update(|cx| toggle(shell, cx));
        }
    })
    .detach();
}

fn close(cx: &mut App) {
    if let Some(window) = OPEN.with(|open| open.borrow_mut().take()) {
        let _ = window.update(cx, |_, window, _| window.remove_window());
    }
}

fn toggle(shell: WindowHandle<Shell>, cx: &mut App) {
    if OPEN.with(|open| open.borrow().is_some()) {
        close(cx);
        return;
    }
    let Ok((enabled, projects)) = shell.update(cx, |shell, _, _| {
        (shell.quick_composer_enabled(), shell.quick_projects())
    }) else {
        return;
    };
    if !enabled {
        return;
    }
    if projects.is_empty() {
        let _ = shell.update(cx, |_, window, _| window.activate_window());
        return;
    }
    let width = 620.;
    let height = 112.;
    let display = cx.primary_display();
    let screen = display.as_ref().map(|d| d.bounds());
    let origin = screen.map_or(point(px(400.), px(240.)), |s| {
        point(
            s.origin.x + (s.size.width - px(width)) / 2.,
            s.origin.y + s.size.height * 0.28,
        )
    });
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin,
            size: size(px(width), px(height)),
        })),
        titlebar: None,
        kind: WindowKind::PopUp,
        focus: true,
        show: true,
        is_movable: false,
        window_background: gpui::WindowBackgroundAppearance::Blurred,
        display_id: display.map(|d| d.id()),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let field = cx.new(|cx| {
                let mut field = TextField::new(t("Peça algo ao agente…"), cx);
                field.focus(cx);
                field
            });
            let events = cx.subscribe(
                &field,
                |this: &mut QuickComposer, _, event, cx| match event {
                    TextFieldEvent::Submit => this.send(cx),
                    TextFieldEvent::Cancel => close(cx),
                    TextFieldEvent::Changed => {}
                },
            );
            let activation = Some(cx.observe_window_activation(window, |_, window, cx| {
                if !window.is_window_active() {
                    close(cx);
                }
            }));
            QuickComposer {
                field,
                projects,
                chosen: 0,
                shell,
                _events: events,
                _activation: activation,
            }
        })
    });
    if let Ok(window) = opened {
        let any: gpui::AnyWindowHandle = window.into();
        OPEN.with(|open| *open.borrow_mut() = Some(any));
        cx.activate(true);
        let _ = window.update(cx, |_, window, _| window.activate_window());
    }
}

impl QuickComposer {
    fn send(&mut self, cx: &mut Context<Self>) {
        let text = self.field.read(cx).text().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let Some((project, _)) = self.projects.get(self.chosen).cloned() else {
            return;
        };
        let _ = self
            .shell
            .update(cx, |shell, _, cx| shell.quick_send(&project, text, cx));
        close(cx);
        cx.hide();
    }
}

impl Render for QuickComposer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self
            .projects
            .get(self.chosen)
            .map_or(String::new(), |(_, name)| name.clone());
        let several = self.projects.len() > 1;
        div()
            .size_full()
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .text_color(content(0.95))
            .child(
                div()
                    .h(px(40.))
                    .px(px(12.))
                    .rounded(px(crate::theme::RADIUS_LG))
                    .border_1()
                    .border_color(content(0.14))
                    .bg(content(0.05))
                    .flex()
                    .items_center()
                    .text_size(px(15.))
                    .child(div().flex_1().min_w_0().child(self.field.clone())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child(
                        div()
                            .id("quick-project")
                            .h(px(24.))
                            .px(px(8.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_color(content(0.85))
                            .bg(content(0.07))
                            .when(several, |d| {
                                d.cursor_pointer().hover(|s| s.bg(content(0.12))).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.chosen = (this.chosen + 1) % this.projects.len();
                                        this.field.update(cx, |field, cx| field.focus(cx));
                                        cx.notify();
                                    }),
                                )
                            })
                            .child(crate::shell::icon("folder", 12.).text_color(content(0.6)))
                            .child(name)
                            .when(several, |d| {
                                d.child(
                                    crate::shell::icon("chevron-down", 10.)
                                        .text_color(content(0.5)),
                                )
                            }),
                    )
                    .child(div().flex_1())
                    .child(t("Enter envia · Esc fecha")),
            )
    }
}
