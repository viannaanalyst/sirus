//! Settings → Conexões → "Conectar um aparelho": the popup of `PairDeviceDialog.tsx`. A hero
//! picture of what happens on the other device, a switch between the iPhone app and a browser,
//! and one view per state: a live code (the QR code with the Sirus mark, the six digits, the
//! steps, the countdown), the device that just joined (named at once) and an expired code.
//!
//! The code comes from the host's `remote_pair`; the QR code is drawn here from the pairing link
//! (the `qrcode` crate, the host's own), with round modules like `PairingQrCode.tsx`. While the
//! popup is open the host's device list is read every two seconds, so a device that used the code
//! shows up as joined.

use crate::i18n::{t, tf};
use crate::motion::{Curve, eased};
use crate::settings_pages::{Edit, Run};
use crate::theme::Sinks as _;
use crate::theme::{accent, content, floating_fill, on_accent};
use chrono::{DateTime, Utc};
use gpui::{
    Animation, AnimationExt, AnyElement, BorderStyle, Bounds, Context, Corners, Edges, FontWeight,
    Hsla, div, prelude::*, px, rgb, size, svg,
};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const DIALOG_TIME: f32 = 0.18;
/// How often the host's device list is read while the popup is open.
const POLL_EVERY: u64 = 2;
/// The text field of the device's name.
const NAME: &str = "pair-name";
/// A text edit that goes nowhere: parks typing when another control is used.
const IDLE: &str = "pair-idle";
/// Names offered for the device that joined, from `connections.suggest.*`.
const SUGGESTIONS: [&str; 3] = ["iPhone pessoal", "iPhone do trabalho", "iPad"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Way {
    App,
    Browser,
}

struct Pairing {
    code: String,
    /// What the QR code holds: the app link when there is a public route, else the first address.
    link: String,
    /// The address a browser opens.
    address: String,
    public_route: bool,
    expires: DateTime<Utc>,
    /// The QR code's modules, row by row, and its width.
    modules: Vec<bool>,
    width: usize,
}

struct Dialog {
    generation: u32,
    closing: bool,
    way: Way,
    pairing: Option<Pairing>,
    busy: bool,
    error: Option<String>,
    /// The device that used the code: its id and the name shown.
    joined: Option<(String, String)>,
    /// The name typed for it.
    name: String,
    copied: Option<Instant>,
    /// Devices known when the code was made; a new one is the device that joined.
    known: Vec<String>,
    /// Counts the code requests, so a late answer to an older one is dropped.
    request: u32,
}

static STATE: Mutex<Option<Dialog>> = Mutex::new(None);
static GENERATION: AtomicU32 = AtomicU32::new(0);

fn state() -> std::sync::MutexGuard<'static, Option<Dialog>> {
    STATE.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// The Sirus mark in the middle of the QR code (white on the dark tile).
pub fn asset(path: &str) -> Option<Cow<'static, [u8]>> {
    match path {
        "pair/sirus-glyph.png" => Some(Cow::Borrowed(include_bytes!(
            "../assets/pair-sirus-glyph.png"
        ))),
        _ => None,
    }
}

/// True while the popup is up and wants Escape.
pub fn wants_keys() -> bool {
    state().is_some()
}

/// A finished text edit of this popup. True when the key was one of its own.
pub fn commit(key: &str, text: &str) -> bool {
    match key {
        NAME => {
            if let Some(dialog) = state().as_mut() {
                dialog.name = text.trim().chars().take(60).collect();
            }
            true
        }
        IDLE => true,
        _ => false,
    }
}

/// Whether Enter in this field also finishes the popup.
pub fn enter_saves(key: &str) -> bool {
    key == NAME
}

fn message(error: &Value) -> String {
    error["message"]
        .as_str()
        .or_else(|| error.as_str())
        .unwrap_or("")
        .to_string()
}

fn parse(value: &Value) -> Option<Pairing> {
    let code = value["code"].as_str()?.to_string();
    let urls: Vec<&str> = value["urls"]
        .as_array()
        .map(|urls| urls.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let app = value["appUrl"].as_str();
    let link = app.or(urls.first().copied())?.to_string();
    let expires = value["expiresAt"]
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|at| at.with_timezone(&Utc))?;
    let qr =
        qrcode::QrCode::with_error_correction_level(link.as_bytes(), qrcode::EcLevel::M).ok()?;
    let width = qr.width();
    let modules = qr
        .to_colors()
        .into_iter()
        .map(|color| color == qrcode::Color::Dark)
        .collect();
    Some(Pairing {
        code,
        address: urls.first().map(|url| url.to_string()).unwrap_or_default(),
        public_route: app.is_some(),
        link,
        expires,
        modules,
        width,
    })
}

/// Seconds left on the code, if any.
fn seconds_left(pairing: &Pairing) -> Option<i64> {
    let left = (pairing.expires - Utc::now()).num_seconds();
    (left > 0).then_some(left)
}

fn countdown(seconds: i64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Opens the popup and asks the host for a code. `known` are the ids of the devices listed now.
pub fn open<V: 'static>(
    backend: &crate::backend::Backend,
    run: Run<V>,
    known: Vec<String>,
    cx: &mut Context<V>,
) {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    *state() = Some(Dialog {
        generation,
        closing: false,
        way: Way::App,
        pairing: None,
        busy: true,
        error: None,
        joined: None,
        name: String::new(),
        copied: None,
        known,
        request: 0,
    });
    cx.notify();
    request(backend, cx);
    watch(backend.clone(), run, generation, cx);
}

/// Asks for a fresh code; the old one leaves the screen first, so a failure never shows a dead one.
fn request<V: 'static>(backend: &crate::backend::Backend, cx: &mut Context<V>) {
    let number = {
        let mut now = state();
        let Some(dialog) = now.as_mut() else { return };
        dialog.pairing = None;
        dialog.busy = true;
        dialog.error = None;
        dialog.request += 1;
        dialog.request
    };
    cx.notify();
    let reply = backend.request("remote_pair", json!({}));
    cx.spawn(async move |this, cx| {
        let reply = reply.recv().await;
        let _ = this.update(cx, |_, cx| {
            if let Some(dialog) = state().as_mut().filter(|d| d.request == number) {
                dialog.busy = false;
                match reply {
                    Ok(Ok(value)) => match parse(&value) {
                        Some(pairing) => dialog.pairing = Some(pairing),
                        None => dialog.error = Some(t("O motor não gerou um código.").to_string()),
                    },
                    Ok(Err(error)) => dialog.error = Some(message(&error)),
                    Err(_) => dialog.error = Some(t("O pareamento não respondeu.").to_string()),
                }
            }
            cx.notify();
        });
    })
    .detach();
}

/// Redraws every second for the countdown and reads the device list to see who joined.
fn watch<V: 'static>(
    backend: crate::backend::Backend,
    run: Run<V>,
    generation: u32,
    cx: &mut Context<V>,
) {
    cx.spawn(async move |this, cx| {
        let mut tick = 0u64;
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            tick += 1;
            let poll = tick % POLL_EVERY == 0;
            let reply = poll
                .then(|| backend.request("remote_action", json!({"action": {"type": "status"}})));
            let alive = this.update(cx, |_, cx| {
                let open = state().as_ref().is_some_and(|d| d.generation == generation);
                if open {
                    cx.notify();
                }
                open
            });
            if !alive.unwrap_or(false) {
                return;
            }
            let Some(reply) = reply else { continue };
            let Ok(Ok(status)) = reply.recv().await else {
                continue;
            };
            let _ = this.update(cx, |this, cx| {
                let mut now = state();
                let Some(dialog) = now.as_mut().filter(|d| d.generation == generation) else {
                    return;
                };
                if dialog.joined.is_some() || dialog.pairing.is_none() {
                    return;
                }
                let found = status["devices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|d| {
                        d["id"]
                            .as_str()
                            .is_some_and(|id| !dialog.known.iter().any(|known| known == id))
                    });
                if let Some(device) = found {
                    let id = device["id"].as_str().unwrap_or_default().to_string();
                    let name = device["name"].as_str().unwrap_or_default().to_string();
                    dialog.known.push(id.clone());
                    dialog.name = name.clone();
                    dialog.joined = Some((id, name));
                    drop(now);
                    // The page's list shows it too.
                    run(
                        this,
                        "remote_action",
                        json!({"action": {"type": "status"}}),
                        cx,
                    );
                    cx.notify();
                }
            });
        }
    })
    .detach();
}

/// Starts closing: the popup fades, then goes away.
pub fn close<V: 'static>(cx: &mut Context<V>) {
    {
        let mut now = state();
        match now.as_mut() {
            Some(dialog) if !dialog.closing => dialog.closing = true,
            _ => return,
        }
    }
    cx.notify();
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis((DIALOG_TIME * 1000.) as u64 + 10))
            .await;
        let _ = this.update(cx, |_, cx| {
            let mut now = state();
            if now.as_ref().is_some_and(|d| d.closing) {
                *now = None;
            }
            cx.notify();
        });
    })
    .detach();
}

/// Saves the name typed for the device that joined, when it changed.
fn save_name<V: 'static>(view: &mut V, run: Run<V>, cx: &mut Context<V>) {
    let rename = {
        let now = state();
        now.as_ref().and_then(|d| {
            let (id, shown) = d.joined.as_ref()?;
            (!d.name.is_empty() && d.name != *shown).then(|| (id.clone(), d.name.clone()))
        })
    };
    if let Some((id, name)) = rename {
        run(
            view,
            "remote_action",
            json!({"action": {"type": "rename", "deviceId": id, "name": name}}),
            cx,
        );
    }
}

/// The popup's own commands, from its buttons.
#[allow(clippy::too_many_arguments)]
pub fn command<V: 'static>(
    view: &mut V,
    command: &str,
    args: &Value,
    backend: &crate::backend::Backend,
    run: Run<V>,
    edit: Edit<V>,
    known: Vec<String>,
    cx: &mut Context<V>,
) {
    match command {
        "native_pair_open" => open(backend, run, known, cx),
        "native_pair_new" => request(backend, cx),
        "native_pair_close" => close(cx),
        "native_pair_way" => {
            if let Some(dialog) = state().as_mut() {
                dialog.way = if args["way"] == "browser" {
                    Way::Browser
                } else {
                    Way::App
                };
            }
            cx.notify();
        }
        "native_pair_copy" => {
            let text = {
                let mut now = state();
                now.as_mut().and_then(|dialog| {
                    let text = {
                        let pairing = dialog.pairing.as_ref()?;
                        match args["what"].as_str() {
                            Some("address") => pairing.address.clone(),
                            _ => pairing.link.clone(),
                        }
                    };
                    dialog.copied = Some(Instant::now());
                    Some(text)
                })
            };
            if let Some(text) = text {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            }
            cx.notify();
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(1600))
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
        "native_pair_suggest" => {
            if let Some(dialog) = state().as_mut() {
                dialog.name = args["name"].as_str().unwrap_or_default().to_string();
            }
            edit(view, IDLE, String::new(), cx);
            cx.notify();
        }
        "native_pair_another" => {
            save_name(view, run, cx);
            if let Some(dialog) = state().as_mut() {
                dialog.joined = None;
                dialog.name.clear();
            }
            request(backend, cx);
            edit(view, IDLE, String::new(), cx);
        }
        "native_pair_done" => {
            save_name(view, run, cx);
            edit(view, IDLE, String::new(), cx);
            close(cx);
        }
        _ => {}
    }
}

/// Escape closes the popup; Enter finishes naming. True when the key was used.
pub fn key<V: 'static>(view: &mut V, key: &str, run: Run<V>, cx: &mut Context<V>) -> bool {
    let (open, joined) = {
        let now = state();
        match now.as_ref() {
            Some(dialog) => (!dialog.closing, dialog.joined.is_some()),
            None => (false, false),
        }
    };
    match key {
        "escape" if open => close(cx),
        "enter" if open && joined => {
            save_name(view, run, cx);
            close(cx);
        }
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------------------------------------
// Drawing

const INK: u32 = 0x111111;
const FINDER: usize = 7;
const DOT_RADIUS: f32 = 0.48;

/// The QR code: round modules, rounded finder squares and the Sirus mark in a cleared square at
/// its centre, on a white tile whatever the theme (not every camera reads an inverted code).
fn qr(pairing: &Pairing, side: f32) -> AnyElement {
    let width = pairing.width;
    let modules = pairing.modules.clone();
    let tile = 12.;
    let inner = side - tile * 2.;
    let unit = inner / width as f32;
    // An odd span keeps the cleared square centred on a module.
    let span = ((width as f32 * 0.2).round() as usize) | 1;
    let start = (width - span) as f32 / 2.;
    let mark = (span as f32 - 1.5) * unit;
    let canvas = gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let ink: Hsla = rgb(INK).into();
            let at = |x: f32, y: f32, w: f32, h: f32| {
                Bounds::new(
                    bounds.origin + gpui::point(px(x * unit), px(y * unit)),
                    size(px(w * unit), px(h * unit)),
                )
            };
            for y in 0..width {
                for x in 0..width {
                    let in_finder = |fx: usize, fy: usize| {
                        x >= fx && x < fx + FINDER && y >= fy && y < fy + FINDER
                    };
                    let far = width - FINDER;
                    if !modules[y * width + x]
                        || in_finder(0, 0)
                        || in_finder(far, 0)
                        || in_finder(0, far)
                    {
                        continue;
                    }
                    let (fx, fy) = (x as f32, y as f32);
                    if fx >= start
                        && fx < start + span as f32
                        && fy >= start
                        && fy < start + span as f32
                    {
                        continue;
                    }
                    window.paint_quad(gpui::quad(
                        at(
                            fx + 0.5 - DOT_RADIUS,
                            fy + 0.5 - DOT_RADIUS,
                            DOT_RADIUS * 2.,
                            DOT_RADIUS * 2.,
                        ),
                        Corners::all(px(DOT_RADIUS * unit)),
                        ink,
                        Edges::all(px(0.)),
                        ink,
                        BorderStyle::Solid,
                    ));
                }
            }
            let far = (width - FINDER) as f32;
            for (fx, fy) in [(0., 0.), (far, 0.), (0., far)] {
                window.paint_quad(gpui::quad(
                    at(fx + 0.5, fy + 0.5, 6., 6.),
                    Corners::all(px(1.9 * unit)),
                    gpui::transparent_black(),
                    Edges::all(px(unit)),
                    ink,
                    BorderStyle::Solid,
                ));
                window.paint_quad(gpui::quad(
                    at(fx + 2., fy + 2., 3., 3.),
                    Corners::all(px(0.9 * unit)),
                    ink,
                    Edges::all(px(0.)),
                    ink,
                    BorderStyle::Solid,
                ));
            }
        },
    )
    .size(px(inner));
    div()
        .relative()
        .size(px(side))
        .flex_shrink_0()
        .p(px(tile))
        .rounded(px(22.))
        .bg(rgb(0xffffff))
        .border_1()
        .border_color(Hsla {
            a: 0.08,
            ..gpui::black()
        })
        .child(canvas)
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(px(mark))
                        .rounded(px(mark * 0.28))
                        .bg(rgb(INK))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(gpui::img("pair/sirus-glyph.png").size(px(mark * 0.62))),
                ),
        )
        .into_any_element()
}

fn tile(path: &'static str) -> AnyElement {
    div()
        .size(px(52.))
        .rounded(px(crate::theme::RADIUS_LG))
        .bg(content(0.08))
        .border_1()
        .border_color(content(0.1))
        .flex()
        .items_center()
        .justify_center()
        .child(svg().path(path).size(px(26.)).text_color(content(0.8)))
        .into_any_element()
}

/// The picture above the title: what happens on the other device, or a link and its state.
fn hero(view: &str, way: Way, code: &str, address: &str) -> AnyElement {
    let frame = div()
        .h(px(132.))
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(content(0.035))
        .border_b_1()
        .border_color(content(0.08))
        .overflow_hidden();
    match (view, way) {
        ("live", Way::App) => frame.child(
            div()
                .mt(px(34.))
                .w(px(170.))
                .h(px(120.))
                .rounded_t(px(22.))
                .border_1()
                .border_color(content(0.16))
                .bg(floating_fill())
                .flex()
                .flex_col()
                .items_center()
                .pt(px(10.))
                .gap(px(6.))
                .child(div().w(px(46.)).h(px(10.)).rounded_full().bg(content(0.16)))
                .child(
                    div()
                        .mt(px(6.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(content(0.92))
                        .child(t("Conectar a este Mac?")),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(content(0.55))
                        .child(t("O Sirus deste iPhone vai controlá-lo.")),
                )
                .child(
                    div()
                        .mt(px(2.))
                        .h(px(22.))
                        .px(px(16.))
                        .rounded_full()
                        .bg(accent())
                        .flex()
                        .items_center()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(on_accent())
                        .child(t("Conectar")),
                ),
        ),
        ("live", Way::Browser) => frame.child(
            div()
                .w(px(300.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.16))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .overflow_hidden()
                .child(
                    div()
                        .h(px(24.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .border_b_1()
                        .border_color(content(0.08))
                        .children(
                            (0..3).map(|_| div().size(px(7.)).rounded_full().bg(content(0.2))),
                        )
                        .child(
                            div()
                                .ml(px(8.))
                                .text_size(px(10.5))
                                .text_color(content(0.5))
                                .child(
                                    address
                                        .trim_start_matches("https://")
                                        .trim_start_matches("http://")
                                        .trim_end_matches('/')
                                        .to_string(),
                                ),
                        ),
                )
                .child(
                    div()
                        .py(px(12.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(content(0.9))
                                .child(t("Digite o código")),
                        )
                        .child(div().flex().gap(px(5.)).children(code.chars().map(|digit| {
                            div()
                                .size_with(px(24.), px(30.))
                                .rounded(px(crate::theme::RADIUS_SM))
                                .bg(content(0.07))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(content(0.9))
                                .child(digit.to_string())
                        }))),
                ),
        ),
        _ => frame.child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(tile("smartphone"))
                .child(
                    div()
                        .size(px(28.))
                        .rounded_full()
                        .bg(if view == "expired" {
                            content(0.16)
                        } else {
                            accent()
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(if view == "expired" {
                                    "refresh"
                                } else {
                                    "check"
                                })
                                .size(px(14.))
                                .text_color(if view == "expired" {
                                    content(0.8)
                                } else {
                                    on_accent()
                                }),
                        ),
                )
                .child(tile("laptop")),
        ),
    }
    .into_any_element()
}

trait SizeWith {
    fn size_with(self, w: gpui::Pixels, h: gpui::Pixels) -> Self;
}

impl SizeWith for gpui::Div {
    fn size_with(self, w: gpui::Pixels, h: gpui::Pixels) -> Self {
        self.w(w).h(h)
    }
}

fn button<V: 'static>(
    id: &'static str,
    label: Option<&str>,
    icon: Option<&'static str>,
    primary: bool,
    enabled: bool,
    command: &'static str,
    args: Value,
    typing: Option<String>,
    run: Run<V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let base = div()
        .id(id)
        .h(px(32.))
        .px(px(12.))
        .rounded(px(crate::theme::RADIUS_MD))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .when(primary, |d| d.bg(accent()).text_color(on_accent()))
        .when(!primary, |d| d.bg(content(0.07)).text_color(content(0.88)))
        .when_some(icon, |d, path| {
            d.child(svg().path(path).size(px(14.)).text_color(if primary {
                on_accent()
            } else {
                content(0.8)
            }))
        })
        .when_some(label, |d, text| d.child(text.to_string()));
    if !enabled {
        return base.opacity(0.45).into_any_element();
    }
    base.cursor_pointer()
        .sinks()
        .hover(|s| s.opacity(0.88))
        .on_click(cx.listener(move |this, _, _, cx| {
            // The shell routes commands to `command` above.
            if let Some(text) = &typing {
                commit(NAME, text);
            }
            run(this, command, args.clone(), cx);
        }))
        .into_any_element()
}

/// The popup, over Settings → Conexões.
pub fn overlay<V: 'static>(
    page: &str,
    editing: Option<(&str, &str)>,
    run: Run<V>,
    edit: Edit<V>,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    if page != "Conexões" {
        return None;
    }
    let now = state();
    let dialog = now.as_ref()?;
    let closing = dialog.closing;
    let generation = dialog.generation;
    let pairing = dialog.pairing.as_ref();
    let left = pairing.and_then(seconds_left);
    let view = if dialog.joined.is_some() {
        "linked"
    } else if pairing.is_some() && left.is_none() {
        "expired"
    } else {
        "live"
    };
    let way = dialog.way;
    let code = pairing.map_or("······".to_string(), |p| p.code.clone());
    let address = pairing.map(|p| p.address.clone()).unwrap_or_default();
    let typing: Option<String> = editing
        .filter(|(key, _)| *key == NAME)
        .map(|(_, text)| text.to_string());
    let copied = dialog
        .copied
        .is_some_and(|at| at.elapsed() < Duration::from_millis(1500));

    let (title, about) = match view {
        "linked" => (
            tf(
                "{} conectado",
                &[&dialog.joined.as_ref().map_or("", |(_, name)| name.as_str())],
            ),
            t("Dê um nome para reconhecer este aparelho na lista."),
        ),
        "expired" => (
            t("Este código expirou").to_string(),
            t(
                "Por segurança cada código vale 5 minutos e serve uma vez. Gere outro para continuar.",
            ),
        ),
        _ => (
            t("Conectar um aparelho").to_string(),
            match way {
                Way::App => t("Leia com a câmera do iPhone ou digite o código no app do Sirus."),
                Way::Browser => t(
                    "Abra o endereço no navegador de outro aparelho da sua Tailscale e digite o código.",
                ),
            },
        ),
    };

    let spaced = if code.len() == 6 {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.clone()
    };

    let mut body = div().flex().flex_col().gap(px(14.)).px(px(20.)).pb(px(20.));
    match view {
        "live" => {
            let segment =
                |id: &'static str, label: &'static str, icon: &'static str, value: Way| {
                    let on = way == value;
                    div()
                        .id(id)
                        .h(px(28.))
                        .px(px(12.))
                        .rounded(px(crate::theme::RADIUS_MD))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(12.5))
                        .text_color(content(if on { 0.95 } else { 0.55 }))
                        .when(on, |d| d.bg(content(0.1)))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            run(
                                this,
                                "native_pair_way",
                                json!({"way": if value == Way::App { "app" } else { "browser" }}),
                                cx,
                            );
                        }))
                        .child(svg().path(icon).size(px(13.)).text_color(content(if on {
                            0.9
                        } else {
                            0.5
                        })))
                        .child(label)
                };
            body = body.child(
                div()
                    .self_center()
                    .p(px(2.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .bg(content(0.05))
                    .flex()
                    .gap(px(2.))
                    .child(segment(
                        "pair-way-app",
                        t("App do iPhone"),
                        "smartphone",
                        Way::App,
                    ))
                    .child(segment(
                        "pair-way-browser",
                        t("Navegador"),
                        "globe",
                        Way::Browser,
                    )),
            );
            let card = div()
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.1))
                .bg(content(0.03))
                .p(px(16.));
            body = body.child(match way {
                Way::App => card
                    .flex()
                    .items_center()
                    .gap(px(20.))
                    .child(match pairing {
                        Some(p) => qr(p, 176.),
                        None => div()
                            .size(px(176.))
                            .flex_shrink_0()
                            .rounded(px(22.))
                            .bg(content(0.06))
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(content(0.5))
                                            .child(t("Ou digite no app")),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(30.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(content(0.96))
                                            .child(spaced.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(5.))
                                    .text_size(px(12.))
                                    .line_height(px(17.))
                                    .text_color(content(0.62))
                                    .children(
                                        [
                                            if pairing.is_some_and(|p| p.public_route) {
                                                t("Funciona de qualquer lugar pelo endereço público deste Mac, sem Tailscale")
                                            } else {
                                                t("Tailscale no iPhone, com a mesma conta deste Mac")
                                            },
                                            t("Leia o QR ou digite o código"),
                                            t("Dê um nome ao aparelho"),
                                        ]
                                        .into_iter()
                                        .enumerate()
                                        .map(|(i, step)| {
                                            div()
                                                .flex()
                                                .gap(px(8.))
                                                .child(
                                                    div()
                                                        .w(px(14.))
                                                        .text_color(content(0.4))
                                                        .child(format!("{}.", i + 1)),
                                                )
                                                .child(div().flex_1().child(step))
                                        }),
                                    ),
                            ),
                    )
                    .into_any_element(),
                Way::Browser => card
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(content(0.5))
                                    .child(t("Endereço")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.5))
                                    .text_color(content(0.9))
                                    .child(address.clone()),
                            )
                            .child(button(
                                "pair-copy-address",
                                None,
                                Some(if copied { "check" } else { "copy" }),
                                false,
                                pairing.is_some(),
                                "native_pair_copy",
                                json!({"what": "address"}),
                                None,
                                run,
                                cx,
                            )),
                    )
                    .child(
                        div().flex().justify_center().gap(px(6.)).children(code.chars().enumerate().map(
                            |(i, digit)| {
                                div()
                                    .ml(px(if i == 3 { 10. } else { 0. }))
                                    .w(px(40.))
                                    .h(px(52.))
                                    .rounded(px(crate::theme::RADIUS_LG))
                                    .bg(content(0.07))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(24.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(content(0.95))
                                    .child(digit.to_string())
                            },
                        )),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child(t("Só aparelhos da sua Tailscale chegam a este endereço.")),
                    )
                    .into_any_element(),
            });
            if let Some(error) = &dialog.error {
                body = body.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(0xec8a85))
                        .child(error.clone()),
                );
            }
            let status = match left {
                Some(seconds) => tf("Vale por {} · uso único", &[&countdown(seconds)]),
                None if dialog.busy => t("Gerando um código…").to_string(),
                None => String::new(),
            };
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.55))
                            .child(status),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .when(way == Way::App && pairing.is_some(), |d| {
                                d.child(button(
                                    "pair-copy-link",
                                    Some(if copied {
                                        t("Copiado")
                                    } else {
                                        t("Copiar link")
                                    }),
                                    Some(if copied { "check" } else { "copy" }),
                                    false,
                                    true,
                                    "native_pair_copy",
                                    json!({"what": "link"}),
                                    None,
                                    run,
                                    cx,
                                ))
                            })
                            .child(button(
                                "pair-new",
                                Some(t("Novo código")),
                                Some("refresh"),
                                false,
                                !dialog.busy,
                                "native_pair_new",
                                json!({}),
                                None,
                                run,
                                cx,
                            )),
                    ),
            );
        }
        "linked" => {
            let typed = typing.clone();
            let shown = typed.clone().unwrap_or_else(|| dialog.name.clone());
            let editing_now = typed.is_some();
            let start = shown.clone();
            body = body
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(content(0.9))
                                .child(t("Nome")),
                        )
                        .child(
                            div()
                                .id("pair-name")
                                .h(px(32.))
                                .px(px(10.))
                                .rounded(px(crate::theme::RADIUS_MD))
                                .border_1()
                                .border_color(if editing_now { accent() } else { content(0.14) })
                                .flex()
                                .items_center()
                                .gap(px(1.))
                                .text_size(px(13.))
                                .text_color(content(0.95))
                                .cursor_text()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    edit(this, NAME, start.clone(), cx);
                                }))
                                .child(shown)
                                .when(editing_now, |d| {
                                    d.child(div().w(px(1.5)).h(px(14.)).bg(accent()))
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .children(SUGGESTIONS.iter().map(|name| {
                            let label = t(name);
                            div()
                                .id(*name)
                                .h(px(26.))
                                .px(px(10.))
                                .rounded_full()
                                .bg(content(0.07))
                                .flex()
                                .items_center()
                                .text_size(px(12.))
                                .text_color(content(0.75))
                                .cursor_pointer()
                                .hover(|s| s.bg(content(0.12)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    run(this, "native_pair_suggest", json!({"name": label}), cx);
                                }))
                                .child(label)
                        })),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(button(
                            "pair-another",
                            Some(t("Conectar outro")),
                            None,
                            false,
                            true,
                            "native_pair_another",
                            json!({}),
                            typing.clone(),
                            run,
                            cx,
                        ))
                        .child(button(
                            "pair-done",
                            Some(t("Concluir")),
                            None,
                            true,
                            !dialog.name.is_empty()
                                || typing.as_ref().is_some_and(|t| !t.trim().is_empty()),
                            "native_pair_done",
                            json!({}),
                            typing.clone(),
                            run,
                            cx,
                        )),
                );
        }
        _ => {
            body = body.child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(button(
                        "pair-close",
                        Some(t("Fechar")),
                        None,
                        false,
                        true,
                        "native_pair_close",
                        json!({}),
                        None,
                        run,
                        cx,
                    ))
                    .child(button(
                        "pair-new",
                        Some(t("Novo código")),
                        Some("refresh"),
                        true,
                        !dialog.busy,
                        "native_pair_new",
                        json!({}),
                        None,
                        run,
                        cx,
                    )),
            );
        }
    }

    let header = div()
        .px(px(20.))
        .pt(px(16.))
        .pb(px(14.))
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(0.97))
                .child(title),
        )
        .child(
            div()
                .mt(px(4.))
                .text_size(px(12.5))
                .line_height(px(18.))
                .text_color(content(0.55))
                .child(about),
        );
    let card = div()
        .id("pair-card")
        .relative()
        .w(px(520.))
        .max_w_full()
        .max_h(gpui::relative(0.94))
        .overflow_y_scroll()
        .rounded(px(crate::theme::RADIUS_LG))
        .border_1()
        .border_color(content(0.12))
        .bg(floating_fill())
        .shadow_lg()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(hero(view, way, &code, &address))
        .child(header)
        .child(body)
        .with_animation(
            format!("pair-card-{generation}-{closing}"),
            Animation::new(Duration::from_secs_f32(DIALOG_TIME))
                .with_easing(|t| eased(t, Curve::Expo)),
            move |el, t| {
                let p = if closing { 1. - t } else { t };
                el.opacity(p).top(px((1. - p) * 6.))
            },
        );
    Some(
        div()
            .id("pair-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    run(this, "native_pair_close", json!({}), cx);
                    cx.stop_propagation();
                }),
            )
            .child(card)
            .with_animation(
                format!("pair-backdrop-{generation}-{closing}"),
                Animation::new(Duration::from_secs_f32(DIALOG_TIME))
                    .with_easing(|t| eased(t, Curve::Expo)),
                move |el, t| {
                    let p = if closing { 1. - t } else { t };
                    el.bg(Hsla {
                        a: 0.45 * p,
                        ..gpui::black()
                    })
                },
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(expires: &str) -> Value {
        json!({
            "code": "123456",
            "urls": ["http://100.97.103.5:7711/?pair=123456"],
            "expiresAt": expires,
            "qrSvg": "<svg/>",
        })
    }

    #[test]
    fn a_pairing_becomes_a_qr_of_its_link() {
        let soon = (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        let pairing = parse(&sample(&soon)).unwrap();
        assert_eq!(pairing.code, "123456");
        assert_eq!(pairing.address, "http://100.97.103.5:7711/?pair=123456");
        assert_eq!(pairing.link, pairing.address);
        assert!(!pairing.public_route);
        assert_eq!(pairing.modules.len(), pairing.width * pairing.width);
        // Finder squares: the corner module is dark and the quiet cell beside the first is light.
        assert!(pairing.modules[0] && !pairing.modules[7]);
        assert!(seconds_left(&pairing).is_some_and(|left| left > 290));
        assert_eq!(countdown(299), "4:59");
        assert_eq!(countdown(60), "1:00");
    }

    #[test]
    fn an_app_link_is_what_the_qr_holds_and_old_codes_have_no_time_left() {
        let mut value = sample("2020-01-01T00:00:00Z");
        value["appUrl"] = json!("sirus://pair?address=https%3A%2F%2Fmac.ts.net&code=123456");
        let pairing = parse(&value).unwrap();
        assert!(pairing.public_route);
        assert!(pairing.link.starts_with("sirus://pair"));
        assert!(seconds_left(&pairing).is_none());
        // No address and no link: nothing to show.
        assert!(
            parse(&json!({"code": "1", "urls": [], "expiresAt": "2030-01-01T00:00:00Z"})).is_none()
        );
    }
}
