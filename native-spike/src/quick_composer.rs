//! The quick composer: ⌃⌥Space (changeable in Settings → Atalhos) from any app, or the Sirus icon in the menu bar, opens a small
//! floating field. Enter starts a new conversation with the text in the chosen project and the
//! field goes away; the conversation runs in Sirus behind whatever app was in front. Escape or
//! clicking elsewhere closes it. Settings → Geral → "Composer rápido" turns it off.

use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Subscription, Window, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, point, px, size,
};

use crate::i18n::{t, tf};
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

/// The registered hotkey's id, read by the event handler (which runs off the main thread).
static WANTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The hotkey manager, the hotkey it holds now and the menu bar icon. Main thread only, like the
/// manager itself.
struct Hotkeys {
    manager: global_hotkey::GlobalHotKeyManager,
    current: Option<global_hotkey::hotkey::HotKey>,
}

thread_local! {
    static HOTKEYS: std::cell::RefCell<Option<Hotkeys>> = const { std::cell::RefCell::new(None) };
    static TRAY: std::cell::RefCell<Option<tray_icon::TrayIcon>> = const { std::cell::RefCell::new(None) };
}

/// A combo in the settings' form (`ctrl+alt+space`) as a hotkey.
fn hotkey_of(combo: &str) -> Option<global_hotkey::hotkey::HotKey> {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    let parsed = crate::hotkey_conflicts::Combo::parse(combo)?;
    let mut modifiers = Modifiers::empty();
    for (on, flag) in [
        (parsed.ctrl, Modifiers::CONTROL),
        (parsed.alt, Modifiers::ALT),
        (parsed.shift, Modifiers::SHIFT),
        (parsed.meta, Modifiers::SUPER),
    ] {
        if on {
            modifiers |= flag;
        }
    }
    let code = if parsed.key == "space" {
        Code::Space
    } else {
        parsed.key.parse::<Code>().ok().or_else(|| {
            let letter = parsed
                .key
                .chars()
                .next()
                .filter(|_| parsed.key.len() == 1)?;
            format!("Key{}", letter.to_ascii_uppercase())
                .parse::<Code>()
                .ok()
        })?
    };
    Some(HotKey::new(Some(modifiers), code))
}

/// The combo the settings ask for, and whether the hotkey should be registered at all.
fn wanted(settings: &serde_json::Value) -> (String, bool) {
    (
        crate::settings_pages::quick_composer_combo(settings).to_owned(),
        settings["quickComposer"] != serde_json::json!(false),
    )
}

/// Whether `combo` can be registered now, without keeping it: macOS refuses a hotkey another app
/// holds. The hotkey the settings keep stays registered (or comes back) either way.
pub(crate) fn can_register(combo: &str, settings: &serde_json::Value) -> bool {
    let Some(hotkey) = hotkey_of(combo) else {
        return false;
    };
    let (current, enabled) = wanted(settings);
    if current == combo && enabled {
        return true;
    }
    HOTKEYS.with(|hotkeys| {
        let mut hotkeys = hotkeys.borrow_mut();
        let Some(state) = hotkeys.as_mut() else {
            return true;
        };
        let held = state.current;
        if let Some(old) = held {
            let _ = state.manager.unregister(old);
        }
        let free = state.manager.register(hotkey).is_ok();
        if free {
            let _ = state.manager.unregister(hotkey);
        }
        if let Some(old) = held {
            let _ = state.manager.register(old);
        }
        free
    })
}

/// Makes the registered hotkey match the settings: the old one is unregistered, the new one
/// registered, and with the switch off none is. When the new one cannot be registered the old one
/// stays. Returns whether the settings' hotkey is the one registered now.
pub(crate) fn rebind(settings: &serde_json::Value) -> bool {
    let (combo, enabled) = wanted(settings);
    let tooltip = tf(
        "Composer rápido do Sirus ({})",
        &[&crate::settings_pages::quick_composer_label(settings)],
    );
    TRAY.with(|tray| {
        if let Some(tray) = tray.borrow().as_ref() {
            let _ = tray.set_tooltip(Some(tooltip));
        }
    });
    HOTKEYS.with(|hotkeys| {
        let mut hotkeys = hotkeys.borrow_mut();
        let Some(state) = hotkeys.as_mut() else {
            return false;
        };
        if !enabled {
            if let Some(old) = state.current.take() {
                let _ = state.manager.unregister(old);
            }
            WANTED.store(0, std::sync::atomic::Ordering::Relaxed);
            return true;
        }
        let Some(hotkey) = hotkey_of(&combo) else {
            return false;
        };
        if state.current == Some(hotkey) {
            return true;
        }
        let old = state.current.take();
        if let Some(old) = old {
            let _ = state.manager.unregister(old);
        }
        match state.manager.register(hotkey) {
            Ok(()) => {
                state.current = Some(hotkey);
                WANTED.store(hotkey.id(), std::sync::atomic::Ordering::Relaxed);
                true
            }
            Err(_) => {
                if let Some(old) = old
                    && state.manager.register(old).is_ok()
                {
                    state.current = Some(old);
                }
                false
            }
        }
    })
}

/// Registers the hotkey from the settings and puts the icon in the menu bar. Both live as long as
/// the app; `rebind` moves the hotkey when the settings change.
pub(crate) fn start(shell: WindowHandle<Shell>, cx: &mut App) {
    let (tx, rx) = async_channel::unbounded::<()>();
    if let Ok(manager) = global_hotkey::GlobalHotKeyManager::new() {
        HOTKEYS.with(|hotkeys| {
            *hotkeys.borrow_mut() = Some(Hotkeys {
                manager,
                current: None,
            });
        });
        let sender = tx.clone();
        global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
            move |event: global_hotkey::GlobalHotKeyEvent| {
                if event.id == WANTED.load(std::sync::atomic::Ordering::Relaxed)
                    && event.state == global_hotkey::HotKeyState::Pressed
                {
                    let _ = sender.try_send(());
                }
            },
        ));
    }
    let settings = shell
        .update(cx, |shell, _, _| shell.settings_snapshot())
        .unwrap_or_default();
    if let Ok(tray) = tray_icon::TrayIconBuilder::new()
        .with_icon(tray_image())
        .with_icon_as_template(true)
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
        TRAY.with(|slot| *slot.borrow_mut() = Some(tray));
    }
    rebind(&settings);
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
                    .border_color(crate::theme::border())
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
