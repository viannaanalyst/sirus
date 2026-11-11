//! The window's notifications: every one of them sits in one column at the top right, under the
//! title bar (ADR-205). From the top: the "press ⌘Q again" notice, a failure or a Git result (the
//! window's `problem`), the Sirus app update, provider CLI updates (on offer, running, or just run;
//! one card listing them all when several are on offer), then the cards for other conversations
//! (activity_cards.rs). Every card is the same compact shape: 320 px wide, growing downward with its
//! words, a 12 px dithered pixel square in the card's tone on the left, a bold title over a dim
//! line, and compact buttons (the primary one filled with the accent, rightmost). Words wrap and are
//! never cut short. The buttons sit beside short words and drop to a row of their own under longer
//! ones (`buttons_beside`). A card that leaves by itself shows its clock as a pixel bar along its
//! foot; hovering pauses that clock, and ✕ (shown on hover) closes it at once. The rules are plain
//! functions so they are tested without a window. This module is a child of `shell` (`#[path]`).

use super::*;
use gpui::AnimationExt;
use std::time::Duration;

/// Long enough to read a sentence or two.
const SHOWN_FOR: Duration = Duration::from_secs(8);
/// How often the clock looks; a hovered or running card holds it still.
const TICK: Duration = Duration::from_millis(200);
/// Every card's width; it grows downward with its words.
pub(super) const WIDTH: f32 = 320.;
/// The space between two cards of the column.
const STACK_GAP: f32 = 5.;
/// The hover group that shows a card's ✕.
const GROUP: &str = "toast-frame";
/// A card comes down this far while it fades in.
const SLIDE: f32 = 14.;
const SLIDE_TIME: f32 = 0.26;
/// The error red of a failure's icon, as the old pill's text was.
const WARM: u32 = 0xe08a8a;
const GOOD: u32 = 0x5fce8c;
/// A card's padding: top, right, bottom, left.
const PAD: [f32; 4] = [7., 8., 8., 9.];
/// The pixel square on the left, and the space after it.
const ICON: f32 = 12.;
const ICON_GAP: f32 = 8.;
/// The words' width when nothing sits beside them: the card less its border, padding and icon.
const TEXT_WIDTH: f32 = WIDTH - 2. - PAD[1] - PAD[3] - ICON - ICON_GAP;
/// Rough widths, per character, of the 12 px semibold title and the 11 px detail and buttons.
const TITLE_CHAR: f32 = 6.6;
const DETAIL_CHAR: f32 = 6.2;
/// A button's padding and border across, and the space between two buttons.
const BUTTON_PAD: f32 = 14.;
const BUTTON_GAP: f32 = 4.;
/// The words and the buttons beside them are as far apart as the icon and the words.
const ACTIONS_GAP: f32 = ICON_GAP;

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
    /// The app update's phase and version put away with "Agora não", "Depois" or ✕.
    update_hidden: Option<(crate::app_update::Phase, String)>,
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
    /// The column of notifications at the top right, or nothing: the ⌘Q notice, the failure, the
    /// app update, the provider updates, then the cards for other conversations.
    pub(super) fn notice_column(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.problem_clock(cx);
        let reduced = cx.reduce_motion();
        let mut cards: Vec<AnyElement> = Vec::new();
        cards.extend(self.quit_card(cx));
        if let Some(problem) = self.problem.clone() {
            cards.push(self.problem_card(&problem, reduced, cx));
        }
        cards.extend(self.app_update_card(reduced, cx));
        let offers = self.toasts.offers.clone();
        if offers.len() > 1 {
            cards.push(self.group_card(&offers, reduced, cx));
        } else {
            for offer in &offers {
                cards.push(self.offer_card(offer, reduced, cx));
            }
        }
        cards.extend(self.activity_card_views(reduced, cx));
        if cards.is_empty() {
            return None;
        }
        Some(
            div()
                .absolute()
                .top(px(crate::theme::TITLE_BAR + 6.))
                .right(px(12.))
                .w(px(WIDTH))
                .flex()
                .flex_col()
                .gap(px(STACK_GAP))
                // The browser's page is a native view over everything: it cuts this column out of
                // itself (browser_holes.rs) rather than hiding.
                .child(
                    gpui::canvas(
                        |bounds, window, _| {
                            let zoom = f64::from(window.content_zoom());
                            let b = bounds.map(|v| f64::from(f32::from(v)) * zoom);
                            crate::browser_holes::report((
                                b.origin.x,
                                b.origin.y,
                                b.size.width,
                                b.size.height,
                            ));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
                .children(cards)
                .into_any_element(),
        )
    }

    /// A new failure gets a fresh clock and its own slide-in.
    fn problem_clock(&mut self, cx: &mut Context<Self>) {
        if self.problem == self.problem_shown {
            return;
        }
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

    /// "Aperte ⌘Q de novo", while the second press is awaited; it fades in and out, and its clock
    /// runs out with the time left for the second press.
    fn quit_card(&self, cx: &gpui::App) -> Option<AnyElement> {
        let (_, at) = self.press_again.filter(|(key, _)| *key == Press::Quit)?;
        let since = at.elapsed().as_secs_f32();
        let (shown, left) = if cx.reduce_motion() {
            (1., 1.)
        } else {
            self.send_moving.set(true);
            (press_notice_shown(since), 1. - since / PRESS_WINDOW)
        };
        let mut card = Card::new(
            "quit-notice",
            crate::theme::accent(),
            t("Aperte ⌘Q de novo para fechar o Sirus").to_owned(),
        );
        card.foot = Foot::Clock(left);
        Some(compose(card).opacity(shown).into_any_element())
    }

    fn problem_card(&self, problem: &str, reduced: bool, cx: &mut Context<Self>) -> AnyElement {
        let good = self.notice.as_deref() == Some(problem);
        let color = if good { GOOD } else { WARM };
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
            .filter(|conflict| conflict.problem == problem);
        let mut card = Card::new(
            named(format!("problem-toast-{serial}")),
            rgb(color).into(),
            title,
        );
        card.detail = detail;
        card.close = Some(close_button("problem-toast-close").on_click(cx.listener(
            |this, _, _, cx| {
                this.problem = None;
                this.toasts.conflict = None;
                cx.notify();
            },
        )));
        match conflict {
            Some(conflict) => {
                // The files in conflict, one per line, in place of the line that lists them.
                if !conflict.files.is_empty() {
                    card.detail = None;
                    card.mono = conflict_files(&conflict.files);
                }
                card.lead = Some(
                    button(
                        named(format!("problem-undo-{serial}")),
                        t("Desfazer"),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.abort_conflict(cx))),
                );
                card.buttons.push((
                    t("Abrir arquivo"),
                    button(
                        named(format!("problem-open-{serial}")),
                        t("Abrir arquivo"),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.open_conflict_file(cx))),
                ));
                card.buttons.push((
                    t("Resolver com o agente"),
                    button(
                        named(format!("problem-resolve-{serial}")),
                        t("Resolver com o agente"),
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.resolve_conflict_with_agent(cx))),
                ));
            }
            None => {
                card.foot = Foot::Clock(
                    1. - self.toasts.problem_up.as_secs_f32() / SHOWN_FOR.as_secs_f32(),
                );
            }
        }
        let card = compose(card).on_hover(cx.listener(|this, hovered: &bool, _, _| {
            this.toasts.problem_hovered = *hovered;
        }));
        slide_in(card, serial, reduced)
    }

    /// The Sirus app update, while one is on offer, downloading or waiting for a restart. "Agora
    /// não", "Depois" and ✕ put that phase of that version away for the session.
    fn app_update_card(&self, reduced: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::app_update::{Phase, status};
        let u = status();
        let version = u.version.clone().unwrap_or_default();
        let key = (u.phase, version.clone());
        if self.toasts.update_hidden.as_ref() == Some(&key) {
            return None;
        }
        let tone = crate::theme::accent();
        let hide = |key: (Phase, String)| {
            cx.listener(move |this: &mut Shell, _: &gpui::ClickEvent, _, cx| {
                this.toasts.update_hidden = Some(key.clone());
                cx.notify();
            })
        };
        let run = |kind: &'static str| {
            cx.listener(move |this: &mut Shell, _: &gpui::ClickEvent, _, cx| {
                this.run_setting("native_app_update", json!({ "type": kind }), cx)
            })
        };
        let mut card = match u.phase {
            Phase::Available => {
                let mut card = Card::new(
                    "app-update-card",
                    tone,
                    tf("{} {} disponível", &[&"Sirus", &version]),
                );
                card.detail = (!u.current.is_empty()).then(|| tf("Você tem a {}", &[&u.current]));
                card.buttons.push((
                    t("Agora não"),
                    button("app-update-later", t("Agora não"), false).on_click(hide(key.clone())),
                ));
                card.buttons.push((
                    t("Instalar"),
                    button("app-update-install", t("Instalar"), true).on_click(run("install")),
                ));
                card
            }
            Phase::Downloading => {
                let mut card = Card::new(
                    "app-update-card",
                    tone,
                    tf("Baixando o Sirus {}…", &[&version]),
                );
                card.foot = Foot::Running(reduced);
                card
            }
            Phase::Installed => {
                let mut card =
                    Card::new("app-update-card", tone, t("Atualização pronta").to_owned());
                card.detail = Some(tf("Reinicie para usar o Sirus {}", &[&version]));
                card.buttons.push((
                    t("Depois"),
                    button("app-update-later", t("Depois"), false).on_click(hide(key.clone())),
                ));
                card.buttons.push((
                    t("Reiniciar"),
                    button("app-update-restart", t("Reiniciar"), true).on_click(run("restart")),
                ));
                card
            }
            _ => return None,
        };
        if u.phase != Phase::Downloading {
            card.close = Some(close_button("app-update-close").on_click(hide(key)));
        }
        Some(slide_in(compose(card), u64::MAX, reduced))
    }

    fn offer_card(&self, offer: &Offer, reduced: bool, cx: &mut Context<Self>) -> AnyElement {
        let name = crate::shell::provider_title(&offer.provider);
        let serial = offer.serial;
        let brand: gpui::Hsla = rgb(provider_brand(&offer.provider)).into();
        let (tone, title, detail, detail_color) = match &offer.progress {
            Progress::Offered | Progress::Running => {
                let detail = if offer.progress == Progress::Running {
                    t("Atualizando…").to_owned()
                } else if offer.command {
                    format!("{} → {}", offer.installed, offer.latest)
                } else {
                    crate::i18n::from_host(&offer.notice)
                };
                (
                    brand,
                    tf("{} {} disponível", &[&name, &offer.latest]),
                    Some(detail),
                    content(0.55),
                )
            }
            Progress::Done(text) => (rgb(GOOD).into(), text.clone(), None, content(0.55)),
            Progress::Failed(message) => (
                rgb(WARM).into(),
                tf("Não foi possível atualizar {}", &[&name]),
                Some(message.clone()),
                rgb(WARM).into(),
            ),
        };
        let mut card = Card::new(named(format!("offer-toast-{serial}")), tone, title);
        card.detail = detail;
        card.detail_color = detail_color;
        if offer.progress == Progress::Offered && offer.command {
            card.buttons.push((
                t("Agora não"),
                button(
                    named(format!("offer-later-{serial}")),
                    t("Agora não"),
                    false,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.drop_offer(serial, true, cx);
                })),
            ));
            card.buttons.push((
                t("Atualizar"),
                button(
                    named(format!("offer-update-{serial}")),
                    t("Atualizar"),
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_offer(serial, cx);
                })),
            ));
        }
        card.foot = if offer.progress == Progress::Running {
            Foot::Running(reduced)
        } else {
            Foot::Clock(1. - offer.shown.as_secs_f32() / SHOWN_FOR.as_secs_f32())
        };
        card.close = Some(
            close_button(named(format!("offer-close-{serial}"))).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.drop_offer(serial, false, cx);
                },
            )),
        );
        let card = compose(card).on_hover(cx.listener(move |this, hovered: &bool, _, _| {
            if let Some(offer) = this.toasts.offers.iter_mut().find(|o| o.serial == serial) {
                offer.hovered = *hovered;
            }
        }));
        slide_in(card, serial, reduced)
    }

    /// One card for several offers: a header, a row per provider with its own "Atualizar" and
    /// progress, then "Agora não" and "Atualizar tudo".
    fn group_card(&self, offers: &[Offer], reduced: bool, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = div().mt(px(6.)).flex().flex_col().gap(px(6.));
        for offer in offers {
            let name = crate::shell::provider_title(&offer.provider);
            let serial = offer.serial;
            let versions = format!("{} → {}", offer.installed, offer.latest);
            let (tone, title, detail, detail_color) = match &offer.progress {
                Progress::Offered | Progress::Running => {
                    let detail = if offer.progress == Progress::Running {
                        t("Atualizando…").to_owned()
                    } else if offer.command || offer.notice.is_empty() {
                        versions
                    } else {
                        crate::i18n::from_host(&offer.notice)
                    };
                    (
                        rgb(provider_brand(&offer.provider)).into(),
                        name.to_owned(),
                        Some(detail),
                        content(0.55),
                    )
                }
                Progress::Done(text) => (rgb(GOOD).into(), text.clone(), None, content(0.55)),
                Progress::Failed(message) => (
                    rgb(WARM).into(),
                    tf("Não foi possível atualizar {}", &[&name]),
                    Some(message.clone()),
                    rgb(WARM).into(),
                ),
            };
            let mut row = div()
                .flex()
                .items_start()
                .gap(px(ICON_GAP))
                .child(pixel_icon(tone))
                .child(words(title, detail, detail_color, Vec::new()));
            if offer.progress == Progress::Offered && offer.command {
                row = row.child(
                    div().self_center().child(
                        button(
                            named(format!("offer-update-{serial}")),
                            t("Atualizar"),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.update_offer(serial, cx);
                        })),
                    ),
                );
            }
            rows = rows.child(row);
        }
        let waiting = offers
            .iter()
            .any(|o| o.progress == Progress::Offered && o.command);
        let running = offers.iter().any(|o| o.progress == Progress::Running);
        let header = div()
            .flex()
            .items_start()
            .gap(px(ICON_GAP))
            .child(pixel_icon(rgb(GOOD).into()))
            .child(words(
                t("Atualizações disponíveis").to_owned(),
                None,
                content(0.55),
                Vec::new(),
            ));
        let card = frame("offer-group-toast".into())
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                for offer in &mut this.toasts.offers {
                    offer.hovered = *hovered;
                }
            }))
            .child(header)
            .child(rows)
            .when(waiting, |d| {
                d.child(
                    div()
                        .mt(px(6.))
                        .flex()
                        .justify_end()
                        .gap(px(BUTTON_GAP))
                        .child(button("offer-group-later", t("Agora não"), false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.drop_all_offers(true, cx);
                            }),
                        ))
                        .child(
                            button("offer-group-all", t("Atualizar tudo"), true).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.update_all_offers(cx);
                                }),
                            ),
                        ),
                )
            })
            .when(running, |d| {
                d.child(running_stripe(
                    "offer-group-running".into(),
                    rgb(GOOD).into(),
                    reduced,
                ))
            })
            .child(
                close_button("offer-group-close").on_click(cx.listener(|this, _, _, cx| {
                    this.drop_all_offers(false, cx);
                })),
            );
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

/// Whether a card's buttons fit beside its words: only when the title and each line under it
/// (at most one) fit on one line in the width the buttons leave. Otherwise the buttons take a row
/// of their own under the words. GPUI cannot measure and lay out again, so this estimates from the
/// character counts.
pub(crate) fn buttons_beside(title: &str, lines: &[&str], labels: &[&str]) -> bool {
    if labels.is_empty() || lines.len() > 1 {
        return false;
    }
    let buttons: f32 = labels
        .iter()
        .map(|label| label.chars().count() as f32 * DETAIL_CHAR + BUTTON_PAD)
        .sum::<f32>()
        + BUTTON_GAP * (labels.len() - 1) as f32;
    let room = TEXT_WIDTH - ACTIONS_GAP - buttons;
    let fits =
        |text: &str, per: f32| !text.contains('\n') && text.chars().count() as f32 * per <= room;
    fits(title, TITLE_CHAR) && lines.iter().all(|line| fits(line, DETAIL_CHAR))
}

/// The cells of the 12 px icon square, five by five: ink fading from solid at the top left,
/// ordered by the Bayer thresholds.
fn icon_cells() -> Vec<(u32, u32)> {
    const SIDE: u32 = 5;
    (0..SIDE)
        .flat_map(|y| (0..SIDE).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let ink = 0.45 + 0.55 * (1. - (x + y) as f32 / (2 * SIDE) as f32);
            ink > crate::project_wallpaper::bayer8(x, y)
        })
        .collect()
}

/// How a card's foot reads.
pub(super) enum Foot {
    None,
    /// The share of the card's time still left, as a pixel bar that shrinks.
    Clock(f32),
    /// Something runs: a stripe of pixels that travels along the foot (still with reduced motion).
    Running(bool),
}

/// What a card shows; `compose` lays it out.
pub(super) struct Card {
    pub id: gpui::ElementId,
    pub tone: gpui::Hsla,
    pub title: String,
    /// What stands in the pixel square's place, and its side: a project's avatar.
    pub icon: Option<(AnyElement, f32)>,
    pub detail: Option<String>,
    pub detail_color: gpui::Hsla,
    /// Lines in a monospaced face under the detail: file paths, a command.
    pub mono: Vec<String>,
    /// A tertiary action at the far left of the button row; with one, the buttons get their row.
    pub lead: Option<gpui::Stateful<gpui::Div>>,
    /// The buttons and their labels, the primary one last.
    pub buttons: Vec<(&'static str, gpui::Stateful<gpui::Div>)>,
    pub foot: Foot,
    pub close: Option<gpui::Stateful<gpui::Div>>,
    /// A click on the words.
    pub words_click: Option<Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static>>,
}

impl Card {
    pub(super) fn new(id: impl Into<gpui::ElementId>, tone: gpui::Hsla, title: String) -> Card {
        Card {
            id: id.into(),
            tone,
            title,
            icon: None,
            detail: None,
            detail_color: content(0.55),
            mono: Vec::new(),
            lead: None,
            buttons: Vec::new(),
            foot: Foot::None,
            close: None,
            words_click: None,
        }
    }
}

/// An element id from a formatted name.
pub(super) fn named(name: String) -> gpui::ElementId {
    gpui::SharedString::from(name).into()
}

/// Lays a card out: the pixel square, the words, and the buttons beside them or on a row under.
pub(super) fn compose(card: Card) -> gpui::Stateful<gpui::Div> {
    let Card {
        id,
        tone,
        title,
        icon,
        detail,
        detail_color,
        mono,
        lead,
        buttons,
        foot,
        close,
        words_click,
    } = card;
    let mut lines: Vec<&str> = detail.iter().map(String::as_str).collect();
    lines.extend(mono.iter().map(String::as_str));
    let labels: Vec<&str> = buttons.iter().map(|(label, _)| *label).collect();
    let beside = lead.is_none() && buttons_beside(&title, &lines, &labels);
    let stacked = !beside && (lead.is_some() || !buttons.is_empty());
    let running_id = named(format!("{id:?}-running"));
    let words = words(title, detail, detail_color, mono);
    let words = match words_click {
        Some(click) => words
            .id(named(format!("{id:?}-words")))
            .cursor_pointer()
            .on_click(click)
            .into_any_element(),
        None => words.into_any_element(),
    };
    let actions = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(BUTTON_GAP))
        .children(buttons.into_iter().map(|(_, button)| button));
    let (icon, side) = match icon {
        Some((element, side)) => (div().flex_none().child(element).into_any_element(), side),
        None => (pixel_icon(tone).into_any_element(), ICON),
    };
    let mut main = div()
        .flex()
        .items_start()
        .gap(px(ICON_GAP))
        .child(icon)
        .child(words);
    let mut row = None;
    if beside {
        // The row's gap is the space between the words and the buttons too.
        main = main.child(div().self_center().child(actions));
    } else if stacked {
        row = Some(
            div()
                .mt(px(6.))
                .pl(px(side + ICON_GAP))
                .flex()
                .items_center()
                .gap(px(BUTTON_GAP))
                .children(lead)
                .child(div().flex_1())
                .child(actions),
        );
    }
    frame(id)
        .child(main)
        .children(row)
        .map(|d| match foot {
            Foot::None => d,
            Foot::Clock(share) => d.child(clock(share, tone)),
            Foot::Running(reduced) => d.child(running_stripe(running_id, tone, reduced)),
        })
        .children(close)
}

/// The card every notification shares: a rounded surface that blocks clicks from reaching the
/// window beneath it.
fn frame(id: gpui::ElementId) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .group(GROUP)
        .relative()
        .overflow_hidden()
        .w(px(WIDTH))
        .rounded(px(crate::theme::RADIUS_SM))
        .border_1()
        .border_color(crate::theme::border())
        .shadow_lg()
        .pt(px(PAD[0]))
        .pr(px(PAD[1]))
        .pb(px(PAD[2]))
        .pl(px(PAD[3]))
        .flex()
        .flex_col()
        .child(crate::theme::floating_surface(crate::theme::RADIUS_SM))
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

/// The 12 px dithered square on the left of a card, in its tone.
pub(super) fn pixel_icon(tone: gpui::Hsla) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            const CELL: f32 = 2.;
            const PITCH: f32 = 2.4;
            for (x, y) in icon_cells() {
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        bounds.origin + gpui::point(px(x as f32 * PITCH), px(y as f32 * PITCH)),
                        gpui::size(px(CELL), px(CELL)),
                    ),
                    tone,
                ));
            }
        },
    )
    .flex_none()
    .mt(px(2.))
    .size(px(ICON))
}

/// The time left, as a 2 px pixel bar along the card's foot that shrinks to the left.
fn clock(share: f32, tone: gpui::Hsla) -> impl IntoElement {
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h(px(2.))
        .child(crate::pixel_bar::pixel_fill(
            share,
            gpui::Hsla { a: 0.75, ..tone },
            2.,
        ))
}

/// Something runs: a short pixel stripe travelling along the card's foot; with reduced motion it
/// stays at the start.
fn running_stripe(id: gpui::ElementId, tone: gpui::Hsla, reduced: bool) -> impl IntoElement {
    let stripe = div()
        .absolute()
        .top_0()
        .h(px(2.))
        .w(gpui::relative(0.3))
        .child(crate::pixel_bar::pixel_fill(
            1.,
            gpui::Hsla { a: 0.75, ..tone },
            2.,
        ));
    let stripe = if reduced {
        stripe.left_0().into_any_element()
    } else {
        stripe
            .with_animation(
                id,
                gpui::Animation::new(Duration::from_millis(1400)).repeat(),
                |el, t| el.left(gpui::relative(t * 1.3 - 0.3)),
            )
            .into_any_element()
    };
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h(px(2.))
        .child(stripe)
}

/// A conflict's files, one per line: the first four, then how many more.
fn conflict_files(files: &[String]) -> Vec<String> {
    const SHOWN: usize = 4;
    let mut lines: Vec<String> = files.iter().take(SHOWN).cloned().collect();
    if files.len() > SHOWN {
        lines.push(format!("+{}", files.len() - SHOWN));
    }
    lines
}

/// A bold title over a dim line, and monospaced lines under it. All of them wrap.
fn words(
    title: String,
    detail: Option<String>,
    detail_color: gpui::Hsla,
    mono: Vec<String>,
) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(1.))
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(0.92))
                .child(title),
        )
        .when_some(detail, |d, detail| {
            d.child(
                div()
                    .text_size(px(11.))
                    .line_height(px(14.))
                    .text_color(detail_color)
                    .child(detail),
            )
        })
        .when(!mono.is_empty(), |d| {
            d.child(
                div()
                    .mt(px(1.))
                    .flex()
                    .flex_col()
                    .children(mono.into_iter().map(|line| {
                        div()
                            .font_family("Menlo")
                            .text_size(px(10.5))
                            .line_height(px(14.))
                            .text_color(content(0.88))
                            .child(line)
                    })),
            )
        })
}

/// A compact button: the primary one filled with the accent, the others a thin outline.
pub(super) fn button(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    let base = div()
        .id(id)
        .flex_none()
        .px(px(6.))
        .py(px(3.))
        .rounded(px(5.))
        .border_1()
        .text_size(px(11.))
        .line_height(px(14.))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.opacity(0.85));
    if primary {
        base.bg(crate::theme::accent())
            .border_color(crate::theme::accent())
            .text_color(crate::theme::on_accent())
            .font_weight(FontWeight::SEMIBOLD)
            .child(label)
    } else {
        base.border_color(content(0.12))
            .text_color(content(0.92))
            .child(label)
    }
}

/// The ✕, out of the way until the pointer is over the card, in its top right corner.
pub(super) fn close_button(id: impl Into<gpui::ElementId>) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .absolute()
        // Top left, over the icon's corner as macOS puts it, so it never sits on a button.
        .top(px(2.))
        .left(px(2.))
        .size(px(16.))
        .opacity(0.)
        .group_hover(GROUP, |s| s.opacity(1.))
        .rounded_full()
        .bg(content(0.16))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(content(0.26)))
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
    fn short_words_keep_the_buttons_beside_them() {
        assert!(buttons_beside(
            "Push feito",
            &["main → origin · 3 commits"],
            &["Desfazer"]
        ));
        assert!(buttons_beside(
            "Precisa de você",
            &["Sirus · npm test"],
            &["Negar", "Aprovar"]
        ));
        assert!(buttons_beside("Tem uma pergunta", &[], &["Responder"]));
    }

    #[test]
    fn longer_words_send_the_buttons_to_their_own_row() {
        // A detail that takes more than one line.
        assert!(!buttons_beside(
            "Não foi possível enviar a mensagem",
            &["O host não respondeu em 30 segundos. Verifique se o computador remoto está ligado."],
            &["Fechar"]
        ));
        // A title too long for the room the buttons leave.
        assert!(!buttons_beside(
            "Conflito ao mesclar worktree-agent-ad613f68 em main",
            &[],
            &["Abrir arquivo", "Resolver com o agente"]
        ));
        // Two lines under the title, or a line break in the detail.
        assert!(!buttons_beside(
            "Falhou",
            &["app", "src/main.rs"],
            &["Abrir"]
        ));
        assert!(!buttons_beside("Falhou", &["um\ndois"], &["Abrir"]));
        // More buttons leave less room for the same words.
        let detail = ["Claude Code · você tem a 2.2.9"];
        assert!(buttons_beside("Atualização", &detail, &["Atualizar"]));
        assert!(!buttons_beside(
            "Atualização",
            &detail,
            &["Agora não", "Atualizar"]
        ));
    }

    #[test]
    fn without_buttons_the_words_take_the_width() {
        assert!(!buttons_beside("Push feito", &[], &[]));
    }

    #[test]
    fn the_icon_is_solid_at_the_top_left_and_thins_out() {
        let cells = icon_cells();
        assert!(cells.contains(&(0, 0)));
        assert!(cells.contains(&(1, 0)) && cells.contains(&(0, 1)));
        let first_row = cells.iter().filter(|(_, y)| *y == 0).count();
        let last_row = cells.iter().filter(|(_, y)| *y == 4).count();
        assert!(first_row > last_row, "{first_row} vs {last_row}");
        assert!(cells.len() < 25);
    }

    #[test]
    fn a_conflict_lists_four_files_then_counts_the_rest() {
        let files: Vec<String> = (1..=6).map(|n| format!("f{n}.rs")).collect();
        assert_eq!(
            conflict_files(&files),
            ["f1.rs", "f2.rs", "f3.rs", "f4.rs", "+2"]
        );
        assert_eq!(conflict_files(&files[..2]), ["f1.rs", "f2.rs"]);
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
