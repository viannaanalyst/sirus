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
use crate::settings_pages::Run;
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

struct Pairing {
    code: String,
    /// What the QR code holds: the app link when there is a public route, else the first address.
    link: String,
    expires: DateTime<Utc>,
    /// The QR code's modules, row by row, and its width.
    modules: Vec<bool>,
    width: usize,
}

struct Dialog {
    generation: u32,
    closing: bool,
    pairing: Option<Pairing>,
    busy: bool,
    error: Option<String>,
    /// The device that used the code: its id and the name shown.
    joined: bool,
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

/// How long a code lasts, for the ring around its time.
const CODE_SECONDS: f32 = 300.;
const CARD_WIDTH: f32 = 380.;

/// The QR's side in a window of `room`: as large as the dialog holds, and never so small it
/// stops being readable.
fn qr_side(room: (f32, f32)) -> f32 {
    let wide = room.0.min(CARD_WIDTH) - 88.;
    let tall = room.1 * 0.94 - 350.;
    wide.min(tall).clamp(132., 224.)
}

/// A ring that empties as the code's time runs out.
fn time_ring(share: f32) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let center = bounds.center();
            let radius = f32::from(bounds.size.width) / 2. - 1.;
            let arc = |share: f32| {
                let mut path = gpui::PathBuilder::stroke(px(1.5));
                let steps = (share.clamp(0., 1.) * 48.).ceil().max(1.) as usize;
                for step in 0..=steps {
                    let turn = share.clamp(0., 1.) * step as f32 / steps as f32;
                    let angle = turn * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    let at =
                        center + gpui::point(px(angle.cos() * radius), px(angle.sin() * radius));
                    if step == 0 {
                        path.move_to(at);
                    } else {
                        path.line_to(at);
                    }
                }
                path.build()
            };
            if let Ok(track) = arc(1.) {
                window.paint_path(track, content(0.12));
            }
            if let Ok(left) = arc(share) {
                window.paint_path(left, accent());
            }
        },
    )
    .size(px(14.))
    .flex_shrink_0()
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
        pairing: None,
        busy: true,
        error: None,
        joined: false,
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
                if dialog.joined || dialog.pairing.is_none() {
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
                    dialog.known.push(id.clone());
                    dialog.joined = true;
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

/// The popup's own commands, from its buttons.
#[allow(clippy::too_many_arguments)]
pub fn command<V: 'static>(
    command: &str,
    backend: &crate::backend::Backend,
    run: Run<V>,
    known: Vec<String>,
    cx: &mut Context<V>,
) {
    match command {
        "native_pair_open" => open(backend, run, known, cx),
        "native_pair_new" => request(backend, cx),
        "native_pair_close" => close(cx),
        "native_pair_copy" => {
            let text = {
                let mut now = state();
                now.as_mut().and_then(|dialog| {
                    let text = {
                        let pairing = dialog.pairing.as_ref()?;
                        pairing.link.clone()
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
        "native_pair_done" => {
            close(cx);
        }
        _ => {}
    }
}

/// Escape closes the popup; Enter finishes naming. True when the key was used.
pub fn key<V: 'static>(key: &str, cx: &mut Context<V>) -> bool {
    let (open, joined) = {
        let now = state();
        match now.as_ref() {
            Some(dialog) => (!dialog.closing, dialog.joined),
            None => (false, false),
        }
    };
    match key {
        "escape" if open => close(cx),
        "enter" if open && joined => close(cx),
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

fn button<V: 'static>(
    id: &'static str,
    label: Option<&str>,
    icon: Option<&'static str>,
    primary: bool,
    enabled: bool,
    command: &'static str,
    args: Value,
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
            run(this, command, args.clone(), cx);
        }))
        .into_any_element()
}

/// The popup, over Settings → Conexões.
pub fn overlay<V: 'static>(
    page: &str,
    room: (f32, f32),
    run: Run<V>,
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
    let view = if dialog.joined {
        "linked"
    } else if pairing.is_some() && left.is_none() {
        "expired"
    } else {
        "live"
    };
    let code = pairing.map_or("······".to_string(), |p| p.code.clone());
    let copied = dialog
        .copied
        .is_some_and(|at| at.elapsed() < Duration::from_millis(1500));

    let (title, about) = match view {
        "linked" => (
            t("iPhone conectado").to_string(),
            t("Já pode usar o Sirus pelo telefone."),
        ),
        "expired" => (
            t("Este código expirou").to_string(),
            t(
                "Por segurança cada código vale 5 minutos e serve uma vez. Gere outro para continuar.",
            ),
        ),
        _ => (
            t("Conectar um aparelho").to_string(),
            t("Leia com a câmera do iPhone ou escaneie pelo app do Sirus."),
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
            body = body.child({
                // The code fits the window, so the whole dialog stays on screen.
                let side = qr_side(room);
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(14.))
                    .child(match pairing {
                        Some(p) => qr(p, side),
                        None => div()
                            .size(px(side))
                            .flex_shrink_0()
                            .rounded(px(22.))
                            .bg(content(0.06))
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(content(0.5))
                                    .child(t("Ou digite no app")),
                            )
                            .child(
                                div()
                                    .text_size(px(28.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(content(0.96))
                                    .child(spaced.clone()),
                            ),
                    )
                    .into_any_element()
            });
            if let Some(error) = &dialog.error {
                body = body.child(
                    div()
                        .w_full()
                        .text_center()
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
                    .justify_center()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .children(left.map(|seconds| time_ring(seconds as f32 / CODE_SECONDS)))
                    .child(status),
            );
            body = body.child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap(px(8.))
                    .when(pairing.is_some(), |d| {
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
                        run,
                        cx,
                    )),
            );
        }
        "linked" => {
            body = body.child(
                div()
                    .id("pair-done")
                    .h(px(32.))
                    .w_full()
                    .rounded(px(crate::theme::RADIUS_MD))
                    .bg(accent())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(on_accent())
                    .cursor_pointer()
                    .sinks()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        run(this, "native_pair_done", json!({}), cx);
                    }))
                    .child(t("Concluir")),
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
                        run,
                        cx,
                    )),
            );
        }
    }

    let header = div()
        .w_full()
        .text_center()
        .px(px(20.))
        .pt(px(18.))
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
        .w(px(CARD_WIDTH))
        .max_w_full()
        .max_h(gpui::relative(0.94))
        .overflow_y_scroll()
        .rounded(px(crate::theme::RADIUS_LG))
        .border_1()
        .border_color(content(0.12))
        .bg(floating_fill())
        .shadow_lg()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(view == "linked", |d| {
            d.child(
                div().w_full().pt(px(22.)).flex().justify_center().child(
                    div()
                        .size(px(44.))
                        .rounded_full()
                        .bg(Hsla {
                            a: 0.14,
                            ..rgb(0x34d399).into()
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(svg().path("check").size(px(22.)).text_color(rgb(0x34d399))),
                ),
            )
        })
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
            "urls": ["https://host.example.com/?pair=123456"],
            "expiresAt": expires,
            "qrSvg": "<svg/>",
        })
    }

    #[test]
    fn a_pairing_becomes_a_qr_of_its_link() {
        let soon = (Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        let pairing = parse(&sample(&soon)).unwrap();
        assert_eq!(pairing.code, "123456");
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
        value["appUrl"] = json!("sirus://pair?address=https%3A%2F%2Fhost.example.com&code=123456");
        let pairing = parse(&value).unwrap();
        assert!(pairing.link.starts_with("sirus://pair"));
        assert!(seconds_left(&pairing).is_none());
        // No address and no link: nothing to show.
        assert!(
            parse(&json!({"code": "1", "urls": [], "expiresAt": "2030-01-01T00:00:00Z"})).is_none()
        );
    }
}
