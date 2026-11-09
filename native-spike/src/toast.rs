//! The window's toasts: rounded cards that slide down and stack. A failure (the window's
//! `problem`) sits at the top centre; provider CLI updates (on offer, running, or just run) sit at
//! the top right, one card each, or one card listing them all when several are on offer. They share
//! one layout: a tinted icon square on the left, a bold title over a dim line, the actions on the right
//! (the primary one last, at the edge) and a ✕. They leave by themselves after a while; hovering
//! pauses that clock, and ✕ closes one at once. The rules are plain functions so they are tested
//! without a window. This module is a child of `shell` (`#[path]`).

use super::*;
use gpui::AnimationExt;
use std::time::Duration;

/// Long enough to read a sentence or two.
const SHOWN_FOR: Duration = Duration::from_secs(8);
/// How often the clock looks; a hovered or running card holds it still.
const TICK: Duration = Duration::from_millis(200);
const WIDTH: f32 = 480.;
/// A card comes down this far while it fades in.
const SLIDE: f32 = 14.;
const SLIDE_TIME: f32 = 0.26;
/// The error red of the failure's icon, as the old pill's text was.
const WARM: u32 = 0xe08a8a;
const GOOD: u32 = 0x5fce8c;

/// Where a provider update's card is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Progress {
    Offered,
    Running,
    /// Updated, or already on the version: the card's title.
    Done(String),
    /// What went wrong, in red.
    Failed(String),
}

/// A provider CLI's update on offer, from the host's `provider-updates` or its `provider_updates`
/// answer on connect.
#[derive(Clone, Debug)]
pub(crate) struct Offer {
    pub provider: String,
    pub installed: String,
    pub latest: String,
    /// The host's one-click update; without it the card shows `notice` and no button.
    pub command: bool,
    pub notice: String,
    pub progress: Progress,
    serial: u64,
    /// Time on screen, counted only while the pointer is off the card and nothing runs.
    shown: Duration,
    hovered: bool,
}

/// A Git conflict the failure card offers to resolve: the files, and the exact problem text it
/// was raised with, so a later problem does not inherit its actions.
#[derive(Clone, Debug)]
pub(crate) struct Conflict {
    pub problem: String,
    pub files: Vec<String>,
}

/// The toasts' own state, kept apart from the window's other fields.
#[derive(Default)]
pub(crate) struct Toasts {
    /// The failure card's clock: whether the pointer is on it, and how long it has been up.
    problem_hovered: bool,
    problem_up: Duration,
    /// Gives each failure's card its own slide-in.
    problem_serial: u64,
    /// The conflict a failure card is offering to resolve, if any.
    pub conflict: Option<Conflict>,
    pub offers: Vec<Offer>,
    /// Offers let go this session (with "Agora não" or ✕), so a later event does not bring them back.
    dismissed: Vec<(String, String)>,
    next: u64,
}

/// The offers in a list of CLI entries: the host's `updates`, or the `provider_updates` answer.
/// An entry with no newer version, or with neither a one-click update nor a notice, is no offer.
pub(crate) fn offers_from(entries: &Value) -> Vec<Offer> {
    entries
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let provider = entry["provider"].as_str().filter(|s| !s.is_empty())?;
            if entry["updateAvailable"].as_bool() == Some(false) {
                return None;
            }
            let latest = entry["latestVersion"].as_str().filter(|s| !s.is_empty())?;
            let command = entry["updateSupported"].as_bool().unwrap_or(false);
            let notice = entry["notice"].as_str().unwrap_or("").to_owned();
            if !command && notice.is_empty() {
                return None;
            }
            Some(Offer {
                provider: provider.to_owned(),
                installed: entry["installedVersion"].as_str().unwrap_or("").to_owned(),
                latest: latest.to_owned(),
                command,
                notice,
                progress: Progress::Offered,
                serial: 0,
                shown: Duration::ZERO,
                hovered: false,
            })
        })
        .collect()
}

/// A card's progress from the host's run: `updated` and `unchanged` are done, anything else failed.
pub(crate) fn progress_of(
    name: &str,
    status: &str,
    installed: &str,
    latest: &str,
    message: &str,
) -> Progress {
    let version = if installed.is_empty() {
        latest
    } else {
        installed
    };
    match status {
        "updated" => Progress::Done(tf("{} atualizado para {}", &[&name, &version])),
        "unchanged" => Progress::Done(tf("{} já está na versão {}", &[&name, &version])),
        _ => Progress::Failed(if message.is_empty() {
            t("A atualização não terminou.").to_owned()
        } else {
            crate::i18n::from_host(message)
        }),
    }
}

/// One provider's `status`, installed version and message in `update_providers`' answer, an array
/// of the host's per-provider results.
fn answer_entry(value: &Value, provider: &str) -> Option<(String, String, String)> {
    let entry = value
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["provider"].as_str() == Some(provider))?;
    let text = |key: &str| entry[key].as_str().unwrap_or("").to_owned();
    Some((text("status"), text("installedVersion"), text("message")))
}

impl Shell {
    /// The toasts: the failure on the top centre: the failure first, then the offers in the order they came.
    pub(super) fn toast_stack(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.problem != self.problem_shown {
            self.problem_shown = self.problem.clone();
            self.toasts.problem_up = Duration::ZERO;
            self.toasts.problem_hovered = false;
            self.toasts.problem_serial += 1;
            if let Some(shown) = self.problem.clone() {
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(TICK).await;
                        let alive = this
                            .update(cx, |this, cx| this.problem_tick(&shown, cx))
                            .unwrap_or(false);
                        if !alive {
                            break;
                        }
                    }
                })
                .detach();
            }
        }
        if self.problem.is_none() && self.toasts.offers.is_empty() {
            return None;
        }
        let reduced = cx.reduce_motion();
        let top = px(crate::theme::TITLE_BAR + 6.);
        let mut layer = div().absolute().top_0().left_0().size_full();
        if let Some(problem) = self.problem.clone() {
            layer = layer.child(
                div()
                    .absolute()
                    .top(top)
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(self.problem_card(&problem, reduced, cx)),
            );
        }
        if !self.toasts.offers.is_empty() {
            let offers = self.toasts.offers.clone();
            let mut column = div().w(px(WIDTH)).flex().flex_col().gap(px(8.));
            if offers.len() > 1 {
                column = column.child(self.group_card(&offers, reduced, cx));
            } else {
                for offer in &offers {
                    column = column.child(self.offer_card(offer, reduced, cx));
                }
            }
            layer = layer.child(div().absolute().top(top).right(px(12.)).child(column));
        }
        Some(layer.into_any_element())
    }

    /// One tick of the failure's clock; false once the failure is gone and the clock can stop.
    fn problem_tick(&mut self, shown: &str, cx: &mut Context<Self>) -> bool {
        if self.problem.as_deref() != Some(shown) {
            return false;
        }
        // A conflict waits for the person: it leaves by its ✕ or by one of its actions.
        if self
            .toasts
            .conflict
            .as_ref()
            .is_some_and(|conflict| conflict.problem == shown)
        {
            return true;
        }
        if self.toasts.problem_hovered {
            return true;
        }
        self.toasts.problem_up += TICK;
        if self.toasts.problem_up >= SHOWN_FOR {
            self.problem = None;
            cx.notify();
            return false;
        }
        true
    }

    fn problem_card(&self, problem: &str, reduced: bool, cx: &mut Context<Self>) -> AnyElement {
        let good = self.notice.as_deref() == Some(problem);
        let (mark, color) = if good { ("✓", GOOD) } else { ("!", WARM) };
        let (title, detail) = match problem.split_once('\n') {
            // A title with a line under it (the Git button's results).
            Some((title, detail)) => (title.to_owned(), Some(detail.to_owned())),
            None => (problem.to_owned(), None),
        };
        let serial = self.toasts.problem_serial;
        let conflict = self
            .toasts
            .conflict
            .as_ref()
            .is_some_and(|conflict| conflict.problem == problem);
        let mut actions = div().flex_shrink_0().flex().items_center().gap(px(6.));
        if conflict {
            actions = actions
                .child(
                    action(format!("problem-undo-{serial}"), t("Desfazer"), false)
                        .on_click(cx.listener(|this, _, _, cx| this.abort_conflict(cx))),
                )
                .child(
                    action(format!("problem-open-{serial}"), t("Abrir arquivo"), false)
                        .on_click(cx.listener(|this, _, _, cx| this.open_conflict_file(cx))),
                )
                .child(
                    action(
                        format!("problem-resolve-{serial}"),
                        t("Resolver com o agente"),
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.resolve_conflict_with_agent(cx))),
                );
        }
        let card = frame(format!("problem-toast-{serial}"))
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                this.toasts.problem_hovered = *hovered;
            }))
            .child(badge(mark, color))
            .child(words(title, detail, content(0.55)))
            .child(actions)
            .child(
                close_button("problem-toast-close").on_click(cx.listener(|this, _, _, cx| {
                    this.problem = None;
                    this.toasts.conflict = None;
                    cx.notify();
                })),
            );
        slide_in(card, serial, reduced)
    }

    fn offer_card(&self, offer: &Offer, reduced: bool, cx: &mut Context<Self>) -> AnyElement {
        let name = crate::shell::provider_title(&offer.provider);
        let serial = offer.serial;
        let (mark, title, detail, detail_color) = match &offer.progress {
            Progress::Offered | Progress::Running => {
                let detail = if offer.progress == Progress::Running {
                    t("Atualizando…").to_owned()
                } else if offer.command {
                    tf(
                        "Você está na {}. Atualizar não interrompe o que está rodando.",
                        &[&offer.installed],
                    )
                } else {
                    crate::i18n::from_host(&offer.notice)
                };
                (
                    provider_mark_badge(&offer.provider),
                    tf("{} {} disponível", &[&name, &offer.latest]),
                    Some(detail),
                    content(0.55),
                )
            }
            Progress::Done(text) => (badge("✓", GOOD), text.clone(), None, content(0.55)),
            Progress::Failed(message) => (
                badge("!", WARM),
                tf("Não foi possível atualizar {}", &[&name]),
                Some(message.clone()),
                rgb(WARM).into(),
            ),
        };
        let mut actions = div().flex_shrink_0().flex().items_center().gap(px(6.));
        if offer.progress == Progress::Offered && offer.command {
            actions = actions
                .child(
                    action(format!("offer-later-{serial}"), t("Agora não"), false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.drop_offer(serial, true, cx);
                        }),
                    ),
                )
                .child(
                    action(format!("offer-update-{serial}"), t("Atualizar"), true).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.update_offer(serial, cx);
                        }),
                    ),
                );
        }
        let card = frame(format!("offer-toast-{serial}"))
            .on_hover(cx.listener(move |this, hovered: &bool, _, _| {
                if let Some(offer) = this.toasts.offers.iter_mut().find(|o| o.serial == serial) {
                    offer.hovered = *hovered;
                }
            }))
            .child(mark)
            .child(words(title, detail, detail_color))
            .child(actions)
            .child(
                close_button(format!("offer-close-{serial}")).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.drop_offer(serial, false, cx);
                    },
                )),
            );
        slide_in(card, serial, reduced)
    }

    /// One card for several offers: a header, a row per provider with its own "Atualizar" and
    /// progress, then "Agora não" and "Atualizar tudo".
    fn group_card(&self, offers: &[Offer], reduced: bool, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = div().flex().flex_col().gap(px(8.));
        for offer in offers {
            let name = crate::shell::provider_title(&offer.provider);
            let serial = offer.serial;
            let versions = format!("{} → {}", offer.installed, offer.latest);
            let (mark, title, detail, detail_color) = match &offer.progress {
                Progress::Offered | Progress::Running => {
                    let detail = if offer.progress == Progress::Running {
                        t("Atualizando…").to_owned()
                    } else if offer.command || offer.notice.is_empty() {
                        versions
                    } else {
                        crate::i18n::from_host(&offer.notice)
                    };
                    (
                        provider_mark_badge(&offer.provider),
                        name.to_owned(),
                        Some(detail),
                        content(0.55),
                    )
                }
                Progress::Done(text) => (badge("✓", GOOD), text.clone(), None, content(0.55)),
                Progress::Failed(message) => (
                    badge("!", WARM),
                    tf("Não foi possível atualizar {}", &[&name]),
                    Some(message.clone()),
                    rgb(WARM).into(),
                ),
            };
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(mark)
                .child(words(title, detail, detail_color));
            if offer.progress == Progress::Offered && offer.command {
                row = row.child(
                    action(format!("offer-update-{serial}"), t("Atualizar"), false).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.update_offer(serial, cx);
                        }),
                    ),
                );
            }
            rows = rows.child(row);
        }
        let waiting = offers
            .iter()
            .any(|o| o.progress == Progress::Offered && o.command);
        let header = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(badge("↑", GOOD))
            .child(words(
                t("Atualizações disponíveis").to_owned(),
                None,
                content(0.55),
            ))
            .child(
                close_button("offer-group-close").on_click(cx.listener(|this, _, _, cx| {
                    this.drop_all_offers(false, cx);
                })),
            );
        let mut card = frame("offer-group-toast".to_owned())
            .flex_col()
            .items_stretch()
            .gap(px(10.))
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                for offer in &mut this.toasts.offers {
                    offer.hovered = *hovered;
                }
            }))
            .child(header)
            .child(rows);
        if waiting {
            card = card.child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        action("offer-group-later".to_owned(), t("Agora não"), false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.drop_all_offers(true, cx);
                            }),
                        ),
                    )
                    .child(
                        action("offer-group-all".to_owned(), t("Atualizar tudo"), true).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.update_all_offers(cx);
                            }),
                        ),
                    ),
            );
        }
        slide_in(card, 0, reduced)
    }

    /// Starts an offer's clock: it ends when the offer is gone or has been up long enough.
    fn offer_clock(&self, serial: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this
                    .update(cx, |this, cx| this.offer_tick(serial, cx))
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    /// One tick of an offer's clock; false once the offer is gone and the clock can stop.
    fn offer_tick(&mut self, serial: u64, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.toasts.offers.iter().position(|o| o.serial == serial) else {
            return false;
        };
        let offer = &mut self.toasts.offers[ix];
        if offer.hovered || offer.progress == Progress::Running {
            return true;
        }
        offer.shown += TICK;
        if offer.shown < SHOWN_FOR {
            return true;
        }
        self.toasts.offers.remove(ix);
        cx.notify();
        false
    }

    fn next_serial(&mut self) -> u64 {
        self.toasts.next += 1;
        self.toasts.next
    }

    /// The host's offers replace the ones still waiting; a card that runs or has a result stays.
    pub(super) fn offers_arrived(&mut self, entries: &Value, cx: &mut Context<Self>) {
        let fresh = offers_from(entries);
        let listed = |offer: &Offer| {
            fresh
                .iter()
                .any(|f| f.provider == offer.provider && f.latest == offer.latest)
        };
        self.toasts
            .offers
            .retain(|offer| offer.progress != Progress::Offered || listed(offer));
        for mut offer in fresh {
            let known = self
                .toasts
                .offers
                .iter()
                .any(|o| o.provider == offer.provider && o.latest == offer.latest);
            let let_go =
                self.toasts.dismissed.iter().any(|(provider, version)| {
                    *provider == offer.provider && *version == offer.latest
                });
            if known || let_go {
                continue;
            }
            offer.serial = self.next_serial();
            self.offer_clock(offer.serial, cx);
            self.toasts.offers.push(offer);
        }
        cx.notify();
    }

    /// A `provider-updates` event: `available` offers, or a `result` of an update that ran.
    pub(super) fn provider_event(&mut self, payload: &Value, cx: &mut Context<Self>) {
        match payload["kind"].as_str() {
            Some("available") => self.offers_arrived(&payload["updates"], cx),
            Some("result") => {
                let run = &payload["run"];
                self.finish_update(
                    run["provider"].as_str().unwrap_or(""),
                    run["status"].as_str().unwrap_or(""),
                    run["installedVersion"].as_str().unwrap_or(""),
                    run["message"].as_str().unwrap_or(""),
                    cx,
                );
            }
            _ => {}
        }
    }

    /// `update_providers`' answer: the fallback for when the `result` event does not come. A
    /// card that already has its result keeps it.
    pub(super) fn update_answered(
        &mut self,
        provider: &str,
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        let running = self
            .toasts
            .offers
            .iter()
            .find(|o| o.provider == provider)
            .is_none_or(|o| o.progress == Progress::Running);
        if !running {
            return;
        }
        if !ok {
            let message = error_text(value);
            return self.finish_update(provider, "failed", "", &message, cx);
        }
        if let Some((status, installed, message)) = answer_entry(value, provider) {
            self.finish_update(provider, &status, &installed, &message, cx);
        }
    }

    fn finish_update(
        &mut self,
        provider: &str,
        status: &str,
        installed: &str,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        let name = crate::shell::provider_title(provider);
        let latest = self
            .toasts
            .offers
            .iter()
            .find(|o| o.provider == provider)
            .map(|o| o.latest.clone())
            .unwrap_or_default();
        let progress = progress_of(name, status, installed, &latest, message);
        match self
            .toasts
            .offers
            .iter_mut()
            .find(|o| o.provider == provider)
        {
            Some(offer) => {
                offer.progress = progress;
                offer.shown = Duration::ZERO;
            }
            // The card was closed while it ran: a failure still reaches the person.
            None => {
                if let Progress::Failed(text) = progress {
                    let title = tf("Não foi possível atualizar {}", &[&name]);
                    self.problem = Some(format!("{title}\n{text}"));
                }
            }
        }
        cx.notify();
    }

    /// "Atualizar": runs the update for one provider, with the host's confirmation.
    fn update_offer(&mut self, serial: u64, cx: &mut Context<Self>) {
        let Some(offer) = self.toasts.offers.iter_mut().find(|o| o.serial == serial) else {
            return;
        };
        offer.progress = Progress::Running;
        offer.shown = Duration::ZERO;
        let provider = offer.provider.clone();
        let call = self.backend.invoke(
            "update_providers",
            json!({ "providers": [provider.clone()], "confirm": true }),
        );
        self.waiting.insert(call, Waiting::ProviderUpdate(provider));
        cx.notify();
    }

    /// "Atualizar tudo": every offer still waiting with a one-click update.
    fn update_all_offers(&mut self, cx: &mut Context<Self>) {
        let serials: Vec<u64> = self
            .toasts
            .offers
            .iter()
            .filter(|o| o.progress == Progress::Offered && o.command)
            .map(|o| o.serial)
            .collect();
        for serial in serials {
            self.update_offer(serial, cx);
        }
    }

    /// "Agora não" lets go of the offers still waiting (and tells the host); ✕ on the list
    /// closes every row.
    fn drop_all_offers(&mut self, not_now: bool, cx: &mut Context<Self>) {
        let serials: Vec<u64> = self
            .toasts
            .offers
            .iter()
            .filter(|o| !not_now || o.progress == Progress::Offered)
            .map(|o| o.serial)
            .collect();
        for serial in serials {
            self.drop_offer(serial, not_now, cx);
        }
    }

    /// Takes an offer off the stack. "Agora não" also tells the host, and either way the offer is
    /// not shown again this session.
    fn drop_offer(&mut self, serial: u64, tell_host: bool, cx: &mut Context<Self>) {
        let Some(ix) = self.toasts.offers.iter().position(|o| o.serial == serial) else {
            return;
        };
        let offer = self.toasts.offers.remove(ix);
        self.toasts
            .dismissed
            .push((offer.provider.clone(), offer.latest.clone()));
        if tell_host {
            self.backend.invoke(
                "dismiss_provider_update",
                json!({ "provider": offer.provider, "version": offer.latest }),
            );
        }
        cx.notify();
    }
}

/// The card every toast shares: a rounded surface that blocks clicks from reaching the window
/// beneath it.
fn frame(id: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::SharedString::from(id))
        .relative()
        .w(px(WIDTH))
        .rounded(px(crate::theme::RADIUS_LG))
        .border_1()
        .border_color(crate::theme::border())
        .shadow_lg()
        .p(px(12.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// The card comes down into place as it fades in; reduced motion shows it at once.
fn slide_in(card: gpui::Stateful<gpui::Div>, serial: u64, reduced: bool) -> AnyElement {
    if reduced {
        return card.into_any_element();
    }
    card.with_animation(
        format!("toast-in-{serial}"),
        gpui::Animation::new(Duration::from_secs_f32(SLIDE_TIME))
            .with_easing(|t| eased(t, Curve::Expo)),
        |el, t| el.opacity(t).top(px((t - 1.) * SLIDE)),
    )
    .into_any_element()
}

/// The tinted icon square on the left of a card, with its glyph or mark in `color`.
fn badge(glyph: &'static str, color: u32) -> gpui::Div {
    let base: gpui::Hsla = rgb(color).into();
    square(base).child(
        div()
            .text_size(px(14.))
            .font_weight(FontWeight::BOLD)
            .text_color(base)
            .child(glyph),
    )
}

/// The provider's own mark on a tinted square of its brand colour.
fn provider_mark_badge(provider: &str) -> gpui::Div {
    square(rgb(provider_brand(provider)).into()).child(crate::shell::provider_mark(provider, 16.))
}

fn square(base: gpui::Hsla) -> gpui::Div {
    div()
        .flex_shrink_0()
        .size(px(28.))
        .rounded(px(crate::theme::RADIUS_SM))
        .bg(gpui::Hsla { a: 0.16, ..base })
        .flex()
        .items_center()
        .justify_center()
}

/// A bold title over a dim line under it.
fn words(title: String, detail: Option<String>, detail_color: gpui::Hsla) -> gpui::Div {
    let block = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_size(px(13.))
                .line_height(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(0.92))
                .line_clamp(3)
                .child(title),
        );
    match detail {
        Some(detail) => block.child(
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .line_clamp(2)
                .text_color(detail_color)
                .child(detail),
        ),
        None => block,
    }
}

/// An action on the right of a card: the accent for the primary one, a soft fill for the rest.
fn action(id: String, label: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
    let (fill, ink) = if primary {
        (crate::theme::accent(), crate::theme::on_accent())
    } else {
        (content(0.1), content(0.85))
    };
    div()
        .id(gpui::SharedString::from(id))
        .h(px(26.))
        .px(px(10.))
        .rounded(px(crate::theme::RADIUS_SM))
        .flex()
        .items_center()
        .flex_shrink_0()
        .text_size(px(12.))
        .font_weight(if primary {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(ink)
        .bg(fill)
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .child(label)
}

fn close_button(id: impl Into<gpui::ElementId>) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .size(px(20.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(content(0.1)))
        .tooltip(message_actions::name_tip(t("Fechar")))
        .child(icon("close", 10.).text_color(content(0.5)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_entry_with_a_newer_version_is_an_offer() {
        let offers = offers_from(&json!([
            {
                "provider": "claude",
                "installedVersion": "2.1.319",
                "latestVersion": "2.1.322",
                "updateAvailable": true,
                "updateSupported": true,
                "updateCommand": "claude update",
            },
            {
                "provider": "codex",
                "installedVersion": "0.9.0",
                "latestVersion": "0.9.0",
                "updateAvailable": false,
                "updateSupported": true,
            },
            {
                "provider": "opencode",
                "installedVersion": "1.0.0",
                "latestVersion": "1.1.0",
                "updateSupported": false,
                "notice": "Update OpenCode by hand.",
            },
            {
                "provider": "grok",
                "installedVersion": "1.0.0",
                "latestVersion": "1.1.0",
                "updateSupported": false,
            },
        ]));
        let names: Vec<(&str, bool)> = offers
            .iter()
            .map(|offer| (offer.provider.as_str(), offer.command))
            .collect();
        assert_eq!(names, vec![("claude", true), ("opencode", false)]);
        assert_eq!(offers[0].latest, "2.1.322");
        assert_eq!(offers[0].installed, "2.1.319");
        assert_eq!(offers[1].notice, "Update OpenCode by hand.");
    }

    #[test]
    fn the_answer_is_read_by_status_not_by_ok() {
        // The host's `update_providers` answer: one result per provider.
        let answer = json!([
            { "provider": "claude", "status": "updated", "message": "", "installedVersion": "2.1.322" },
            { "provider": "pi", "status": "failed", "message": "EACCES", "installedVersion": null },
        ]);
        assert_eq!(
            answer_entry(&answer, "claude"),
            Some(("updated".into(), "2.1.322".into(), String::new()))
        );
        assert_eq!(
            answer_entry(&answer, "pi"),
            Some(("failed".into(), String::new(), "EACCES".into()))
        );
        assert_eq!(answer_entry(&answer, "codex"), None);
    }

    #[test]
    fn an_event_without_entries_offers_nothing() {
        assert!(offers_from(&json!(null)).is_empty());
        assert!(offers_from(&json!([])).is_empty());
    }

    #[test]
    fn a_run_reads_as_done_or_failed() {
        crate::i18n::with_locale(false, || {
            assert_eq!(
                progress_of("Claude Code", "updated", "2.1.322", "2.1.322", ""),
                Progress::Done("Claude Code atualizado para 2.1.322".to_owned())
            );
            assert_eq!(
                progress_of("Claude Code", "unchanged", "", "2.1.322", ""),
                Progress::Done("Claude Code já está na versão 2.1.322".to_owned())
            );
            assert_eq!(
                progress_of("Claude Code", "failed", "", "", "npm exited 1"),
                Progress::Failed("npm exited 1".to_owned())
            );
            assert_eq!(
                progress_of("Claude Code", "failed", "", "", ""),
                Progress::Failed("A atualização não terminou.".to_owned())
            );
        });
    }
}
