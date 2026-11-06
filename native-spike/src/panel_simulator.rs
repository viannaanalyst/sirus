//! The Simulador pane, after Sirus's DevicesPane and DeviceScreen (ADR-128): the toolchain checks
//! and the consented install of the device hub, a list of iOS simulators, and the live screen of
//! the one attached, with taps, drags, typing, Home, Lock, Rotate, appearance, a screenshot and
//! Shut down. The engine is in simulator.rs. The pane renders only while it is the visible
//! pane; a heartbeat from `render` tells the pump when it is not, and then the stream stops, the
//! hub lease is released and nothing polls until the pane is shown again.
use crate::i18n::{t, tf};
use crate::panels::PanelContext;
use crate::project_files::run_blocking;
use crate::simulator::{self, Feed, Setup, SimDevice, Toolchain};
use crate::theme::{accent, content, on_accent};
use gpui::{
    AnyView, App, Bounds, Context, Div, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels, Point, Render, RenderImage,
    Stateful, Task, Window, canvas, div, prelude::*, px,
};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// A pane that has not rendered for this long is hidden.
const STALE: Duration = Duration::from_millis(1500);
/// A hidden-looking pane that is still shown gets a render at least this often.
const HEARTBEAT: Duration = Duration::from_millis(500);

pub fn new(_context: PanelContext, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let mut panel = SimulatorPanel {
            lease: simulator::new_lease(),
            toolchain: None,
            installing: false,
            devices: Vec::new(),
            hub: None,
            starting: false,
            error: None,
            attached: None,
            booting: None,
            feed: None,
            seen: 0,
            image: None,
            frame_size: (0, 0),
            last_render: Instant::now(),
            last_beat: Instant::now(),
            pump: None,
            focus: cx.focus_handle(),
            stage: Rc::new(Cell::new(Bounds::default())),
            pressed: false,
            confirm_off: false,
            dark: false,
            note: None,
        };
        panel.check(cx);
        panel
    })
    .into()
}

struct SimulatorPanel {
    lease: u64,
    /// None while the checks run.
    toolchain: Option<Rc<Toolchain>>,
    installing: bool,
    devices: Vec<SimDevice>,
    /// The hub's port while this pane holds its lease.
    hub: Option<u16>,
    starting: bool,
    error: Option<String>,
    attached: Option<SimDevice>,
    /// The name of the device being booted.
    booting: Option<String>,
    feed: Option<Arc<Feed>>,
    seen: u64,
    image: Option<Arc<RenderImage>>,
    frame_size: (u32, u32),
    last_render: Instant,
    last_beat: Instant,
    pump: Option<Task<()>>,
    focus: FocusHandle,
    /// Where the stage was last painted, to map the pointer onto the picture.
    stage: Rc<Cell<Bounds<Pixels>>>,
    pressed: bool,
    confirm_off: bool,
    dark: bool,
    note: Option<String>,
}

impl Focusable for SimulatorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn last_device_file() -> std::path::PathBuf {
    crate::backend::data_dir().join("simulator-last")
}

impl SimulatorPanel {
    fn check(&mut self, cx: &mut Context<Self>) {
        self.toolchain = None;
        self.error = None;
        let rx = run_blocking(simulator::check);
        cx.spawn(async move |this, cx| {
            if let Ok(toolchain) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.toolchain = Some(Rc::new(toolchain));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        let Some(node) = self.toolchain.as_ref().and_then(|t| t.node.clone()) else {
            return;
        };
        self.installing = true;
        self.error = None;
        let rx = run_blocking(move || simulator::install(&node));
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.installing = false;
                    if let Err(error) = result {
                        this.error = Some(tf("A instalação falhou: {}", &[&error]));
                        cx.notify();
                    } else {
                        this.check(cx);
                    }
                });
            }
        })
        .detach();
        cx.notify();
    }

    /// Takes the hub lease, lists the simulators and brings back the last one if it is booted.
    fn begin(&mut self, cx: &mut Context<Self>) {
        let Some(toolchain) = self.toolchain.clone() else {
            return;
        };
        let (Some(node), Some(entry)) = (toolchain.node.clone(), toolchain.entry.clone()) else {
            return;
        };
        self.starting = true;
        self.error = None;
        let lease = self.lease;
        let rx = run_blocking(move || {
            simulator::acquire(lease, &node, &entry).map(|port| (port, simulator::list_devices()))
        });
        self.start_pump(cx);
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                let _ = this.update(cx, |this, cx| this.hub_ready(result, cx));
            }
        })
        .detach();
    }

    fn hub_ready(&mut self, result: Result<(u16, Vec<SimDevice>), String>, cx: &mut Context<Self>) {
        self.starting = false;
        match result {
            Err(error) => {
                let detail = if error == "hub" {
                    t("o motor de aparelhos não respondeu.").to_owned()
                } else {
                    error
                };
                self.error = Some(tf("O Simulador não iniciou: {}", &[&detail]));
            }
            Ok((port, devices)) => {
                self.hub = Some(port);
                self.devices = devices;
                if self.pump.is_none() || self.stale() {
                    self.suspend();
                } else {
                    let last = self.attached.take().map(|d| d.udid).or_else(|| {
                        std::fs::read_to_string(last_device_file())
                            .ok()
                            .map(|s| s.trim().to_owned())
                    });
                    if let Some(device) = last.and_then(|udid| {
                        self.devices
                            .iter()
                            .find(|d| d.udid == udid && d.booted)
                            .cloned()
                    }) {
                        self.attach(device, cx);
                    }
                }
            }
        }
        cx.notify();
    }

    fn stale(&self) -> bool {
        self.last_render.elapsed() > STALE
    }

    fn start_pump(&mut self, cx: &mut Context<Self>) {
        if self.pump.is_some() {
            return;
        }
        self.pump = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(simulator::FRAME_INTERVAL)
                    .await;
                match this.update(cx, |this, cx| this.tick(cx)) {
                    Ok(true) => {}
                    _ => break,
                }
            }
        }));
    }

    /// One pump step: false ends the pump, because the pane is no longer shown.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        if self.stale() {
            self.suspend();
            self.pump = None;
            return false;
        }
        let frames = self
            .feed
            .as_ref()
            .map(|f| f.seq.load(Ordering::Relaxed))
            .unwrap_or(0);
        if frames != self.seen || self.last_beat.elapsed() > HEARTBEAT {
            self.seen = frames;
            self.last_beat = Instant::now();
            cx.notify();
        }
        true
    }

    /// Hidden: end the stream and give the hub back. The attached device comes back on show.
    fn suspend(&mut self) {
        if let Some(feed) = self.feed.take() {
            feed.stop();
        }
        if self.hub.take().is_some() || self.starting {
            let lease = self.lease;
            std::thread::spawn(move || simulator::release(lease));
        }
        self.pressed = false;
        self.confirm_off = false;
    }

    fn attach(&mut self, device: SimDevice, cx: &mut Context<Self>) {
        let Some(port) = self.hub else { return };
        self.booting = Some(device.name.clone());
        self.note = None;
        let target = device.clone();
        let rx = run_blocking(move || {
            simulator::attach(port, &target).map(|()| simulator::list_devices())
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.booting = None;
                    match result {
                        Ok(devices) => {
                            this.devices = devices;
                            if let Some(port) = this.hub {
                                let _ = std::fs::write(last_device_file(), &device.udid);
                                this.feed = Some(Feed::start(port, &device.udid));
                                this.attached = Some(SimDevice {
                                    booted: true,
                                    ..device
                                });
                            }
                        }
                        Err(error) => {
                            this.note = Some(tf("Não consegui ligar o aparelho: {}", &[&error]))
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn detach(&mut self, cx: &mut Context<Self>) {
        if let Some(feed) = self.feed.take() {
            feed.stop();
        }
        self.attached = None;
        self.image = None;
        self.confirm_off = false;
        cx.notify();
    }

    fn shut_down(&mut self, cx: &mut Context<Self>) {
        let Some(device) = self.attached.clone() else {
            return;
        };
        let port = self.hub;
        self.detach(cx);
        let _ = std::fs::remove_file(last_device_file());
        let rx = run_blocking(move || {
            simulator::shutdown(port, &device.udid).map(|()| simulator::list_devices())
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(devices) => this.devices = devices,
                        Err(error) => this.note = Some(error),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn appearance(&mut self, cx: &mut Context<Self>) {
        let Some(device) = self.attached.clone() else {
            return;
        };
        self.dark = !self.dark;
        let dark = self.dark;
        let rx = run_blocking(move || simulator::set_appearance(&device.udid, dark));
        cx.spawn(async move |this, cx| {
            if let Ok(Err(error)) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.note = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn screenshot(&mut self, cx: &mut Context<Self>) {
        let Some(device) = self.attached.clone() else {
            return;
        };
        let rx = run_blocking(move || simulator::screenshot(&device.udid));
        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.note = Some(match result {
                        Ok(path) => tf("Captura salva em {}", &[&path.display()]),
                        Err(error) => error,
                    });
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn rotate(&mut self) {
        if let Some(feed) = &self.feed {
            feed.send(simulator::orientation_packet(simulator::next_orientation(
                feed.screen().as_ref().map(|s| s.orientation.as_str()),
            )));
        }
    }

    fn press(&mut self, button: &str) {
        if let Some(feed) = &self.feed {
            feed.send(simulator::button_packet(button));
        }
    }

    /// The pointer as a 0..1 point of the picture, which is the largest box at the frame's
    /// aspect centered in the stage.
    fn picture_point(&self, position: Point<Pixels>) -> Option<(f64, f64)> {
        let stage = self.stage.get();
        let (fw, fh) = self.picture(stage.size.width.as_f32(), stage.size.height.as_f32())?;
        let left = stage.origin.x.as_f32() + (stage.size.width.as_f32() - fw) / 2.;
        let top = stage.origin.y.as_f32() + (stage.size.height.as_f32() - fh) / 2.;
        Some((
            ((position.x.as_f32() - left) / fw).clamp(0., 1.) as f64,
            ((position.y.as_f32() - top) / fh).clamp(0., 1.) as f64,
        ))
    }

    fn picture(&self, width: f32, height: f32) -> Option<(f32, f32)> {
        let (w, h) = self.frame_size;
        (w > 0 && h > 0)
            .then(|| simulator::fit_frame(w as f32 / h as f32, width, height))
            .filter(|(w, _)| *w > 0.)
    }

    fn touch(&mut self, phase: &str, position: Point<Pixels>) {
        let Some((x, y)) = self.picture_point(position) else {
            return;
        };
        if let Some(feed) = &self.feed {
            feed.send(simulator::touch_packet(feed.screen().as_ref(), phase, x, y));
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
            return;
        }
        let Some(usage) = simulator::hid_usage(&k.key) else {
            return;
        };
        let Some(feed) = &self.feed else { return };
        let shift = k.modifiers.shift
            || k.key
                .chars()
                .next()
                .is_some_and(|c| k.key.len() == 1 && c.is_ascii_uppercase());
        for packet in simulator::key_packets(usage, shift) {
            feed.send(packet);
        }
        cx.stop_propagation();
    }

    fn tool(
        &self,
        id: &'static str,
        label: String,
        enabled: bool,
        cx: &mut Context<Self>,
        act: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let button = div()
            .id(id)
            .h(px(24.))
            .px(px(8.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .flex_shrink_0()
            .text_size(px(12.))
            .text_color(content(if enabled { 0.85 } else { 0.35 }))
            .child(label);
        if enabled {
            button
                .cursor_pointer()
                .hover(|style| style.bg(content(0.08)))
                .on_click(cx.listener(move |this, _, _, cx| act(this, cx)))
        } else {
            button
        }
    }

    fn card(&self, title: String, body: String, action: Option<AnyElementPair>) -> Div {
        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(16.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(content(0.9))
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.6))
                    .child(body),
            );
        if let Some((id, label, busy, handler)) = action {
            let mut button = div()
                .id(id)
                .h(px(28.))
                .px(px(12.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .justify_center()
                .w_auto()
                .text_size(px(12.))
                .bg(accent())
                .text_color(on_accent())
                .child(label);
            if !busy {
                button = button.cursor_pointer().on_click(handler);
            } else {
                button = button.opacity(0.6);
            }
            card = card.child(div().flex().child(button));
        }
        card
    }

    fn note_line(&self) -> Option<Div> {
        self.note.clone().map(|note| {
            div()
                .px(px(10.))
                .py(px(6.))
                .text_size(px(11.))
                .text_color(content(0.55))
                .child(note)
        })
    }
}

type AnyElementPair = (
    &'static str,
    String,
    bool,
    Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static>,
);

impl Drop for SimulatorPanel {
    fn drop(&mut self) {
        if let Some(feed) = &self.feed {
            feed.stop();
        }
        let lease = self.lease;
        std::thread::spawn(move || simulator::release(lease));
    }
}

impl Render for SimulatorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.last_render = Instant::now();
        if let Some(feed) = &self.feed {
            if let Some(frame) = feed.take_frame() {
                if let Some(old) = self.image.replace(frame.image) {
                    cx.drop_image(old, Some(window));
                }
                self.frame_size = (frame.width, frame.height);
            }
        }
        let root = div().size_full().flex().flex_col();
        let Some(toolchain) = self.toolchain.clone() else {
            return root
                .p(px(16.))
                .text_size(px(12.))
                .text_color(content(0.55))
                .child(t("Verificando o Simulador…"));
        };
        match simulator::setup(&toolchain) {
            Setup::NeedsXcode => {
                return root.child(self.card(
                    t("O Simulador precisa do Xcode").to_owned(),
                    t("Instale o Xcode com um runtime de iOS e verifique de novo.").to_owned(),
                    Some((
                        "sim-check",
                        t("Verificar de novo").to_owned(),
                        false,
                        Box::new(cx.listener(|this, _, _, cx| this.check(cx))),
                    )),
                ));
            }
            Setup::NeedsNode(found) => {
                let mut body = tf(
                    "Instale o Node.js {} ou mais novo, com npm, e verifique de novo.",
                    &[&simulator::NODE_MAJOR],
                );
                if let Some(found) = found {
                    body = format!("{body} {}", tf("Encontrei {}.", &[&found]));
                }
                return root.child(self.card(
                    t("O Simulador precisa do Node.js").to_owned(),
                    body,
                    Some((
                        "sim-check",
                        t("Verificar de novo").to_owned(),
                        false,
                        Box::new(cx.listener(|this, _, _, cx| this.check(cx))),
                    )),
                ));
            }
            Setup::NeedsInstall => {
                let body = tf(
                    "Instala {} {} (MIT) com o seu npm, cerca de 40 MB. Nada é baixado antes de você pedir.",
                    &[&simulator::HUB_PACKAGE, &simulator::HUB_VERSION],
                );
                let body = match &self.error {
                    Some(error) => format!("{body}\n{error}"),
                    None => body,
                };
                let installing = self.installing;
                return root.child(
                    self.card(
                        t("Instalar as ferramentas do aparelho").to_owned(),
                        body,
                        Some((
                            "sim-install",
                            t(if installing {
                                "Instalando…"
                            } else {
                                "Instalar"
                            })
                            .to_owned(),
                            installing,
                            Box::new(cx.listener(|this, _, _, cx| this.install(cx))),
                        )),
                    ),
                );
            }
            Setup::Ready => {}
        }
        if self.hub.is_none() && !self.starting && self.error.is_none() {
            self.begin(cx);
        }
        self.start_pump(cx);
        if let Some(error) = self.error.clone() {
            return root.child(self.card(
                t("O Simulador não está disponível").to_owned(),
                error,
                Some((
                    "sim-retry",
                    t("Verificar de novo").to_owned(),
                    false,
                    Box::new(cx.listener(|this, _, _, cx| this.check(cx))),
                )),
            ));
        }
        if self.hub.is_none() {
            return root
                .p(px(16.))
                .text_size(px(12.))
                .text_color(content(0.55))
                .child(t("Iniciando o Simulador…"));
        }
        if let Some(name) = self.booting.clone() {
            return root
                .p(px(16.))
                .text_size(px(12.))
                .text_color(content(0.55))
                .child(tf("Ligando {}…", &[&name]));
        }
        let Some(device) = self.attached.clone() else {
            return root.child(self.device_list(cx));
        };

        let ready = self.feed.as_ref().is_some_and(|f| f.input_ready());
        let confirm = self.confirm_off;
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(8.))
            .h(px(36.))
            .flex_shrink_0()
            .child(self.tool(
                "sim-switch",
                t("Trocar").to_owned(),
                true,
                cx,
                |this, cx| this.detach(cx),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .px(px(4.))
                    .child(device.name.clone()),
            )
            .child(
                self.tool("sim-home", t("Início").to_owned(), ready, cx, |this, _| {
                    this.press("home")
                }),
            )
            .child(self.tool(
                "sim-lock",
                t("Bloquear").to_owned(),
                ready,
                cx,
                |this, _| this.press("lock"),
            ))
            .child(
                self.tool("sim-rotate", t("Girar").to_owned(), ready, cx, |this, _| {
                    this.rotate()
                }),
            )
            .child(self.tool(
                "sim-look",
                t(if self.dark { "Claro" } else { "Escuro" }).to_owned(),
                true,
                cx,
                |this, cx| this.appearance(cx),
            ))
            .child(
                self.tool("sim-shot", t("Captura").to_owned(), true, cx, |this, cx| {
                    this.screenshot(cx)
                }),
            )
            .child(self.tool(
                "sim-off",
                t(if confirm { "Confirmar" } else { "Desligar" }).to_owned(),
                true,
                cx,
                |this, cx| {
                    if this.confirm_off {
                        this.shut_down(cx);
                    } else {
                        this.confirm_off = true;
                        cx.notify();
                    }
                },
            ));

        let stage_bounds = self.stage.clone();
        let size = self.stage.get().size;
        let mut stage = div()
            .id("sim-stage")
            .relative()
            .flex_1()
            .min_h_0()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.key_down(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.confirm_off = false;
                    this.pressed = true;
                    this.touch("begin", event.position);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                if this.pressed && event.pressed_button == Some(MouseButton::Left) {
                    this.touch("move", event.position);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, _| {
                    if std::mem::take(&mut this.pressed) {
                        this.touch("end", event.position);
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, _| {
                    if std::mem::take(&mut this.pressed) {
                        this.touch("end", event.position);
                    }
                }),
            )
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |_, bounds, _, _| stage_bounds.set(bounds),
                )
                .absolute()
                .size_full(),
            );
        match (
            &self.image,
            self.picture(size.width.as_f32(), size.height.as_f32()),
        ) {
            (Some(image), Some((fw, fh))) => {
                stage = stage.child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            gpui::img(image.clone())
                                .w(px(fw))
                                .h(px(fh))
                                .rounded(px(crate::theme::RADIUS_XL))
                                .object_fit(ObjectFit::Fill),
                        ),
                );
            }
            _ => {
                let message = match self.feed.as_ref().and_then(|f| f.problem()) {
                    Some(problem) => tf("A tela não conectou: {}", &[&problem]),
                    None => t("Conectando à tela…").to_owned(),
                };
                stage = stage.child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .child(message),
                );
            }
        }
        let mut column = root.child(toolbar).child(stage);
        if self.feed.is_some() && !ready && self.image.is_some() {
            column = column.child(
                div()
                    .px(px(10.))
                    .py(px(4.))
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .child(t("Entrada indisponível por enquanto.")),
            );
        }
        if let Some(note) = self.note_line() {
            column = column.child(note);
        }
        column
    }
}

impl SimulatorPanel {
    fn device_list(&self, cx: &mut Context<Self>) -> Div {
        let track = crate::hover_track::list("sim-devices");
        let mut list = div()
            .relative()
            .flex()
            .flex_col()
            .p(px(8.))
            .child(track.pill());
        if self.devices.is_empty() {
            return div()
                .p(px(16.))
                .text_size(px(12.))
                .text_color(content(0.55))
                .child(t("Nenhum simulador de iOS encontrado."));
        }
        list = list.child(
            div()
                .px(px(8.))
                .py(px(6.))
                .text_size(px(11.))
                .text_color(content(0.45))
                .child(t("Escolha um aparelho")),
        );
        for (ix, device) in self.devices.iter().enumerate().take(24) {
            let target = device.clone();
            let state = t(if device.booted { "Ligado" } else { "Desligado" });
            let row = div()
                .id(("sim-device", ix))
                .h(px(30.))
                .px(px(8.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .cursor_pointer()
                .text_size(px(12.))
                .on_click(cx.listener(move |this, _, _, cx| this.attach(target.clone(), cx)))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(content(0.9))
                        .child(device.name.clone()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(content(0.45))
                        .child(format!("{} · {state}", device.runtime)),
                );
            list = list.child(track.row_named(row, &device.udid, false));
        }
        let mut column = div().size_full().flex().flex_col().child(list);
        if let Some(note) = self.note_line() {
            column = column.child(note);
        }
        column
    }
}
