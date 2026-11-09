//! The composer's context meter and Compact (ADR-057). Ports Sirus's `ContextMeter.tsx`,
//! `lib/context-meter.ts` and `lib/compact-before-send.ts`.
//!
//! A ring on the composer shelf shows how much of the model's window the conversation holds, as
//! the provider last reported it (`Session.contextUsage`, never estimated here). It opens a popup
//! with the percentage, the tokens and a Compact button. Compact sends the standalone `/compact`
//! to the host, which runs the provider's own compaction (Codex and Claude only); the same route
//! serves a typed `/compact` and "Compact and send", where a heavy Claude conversation that has
//! been idle past its prompt cache compacts first and the message goes out behind it.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;
use gpui::{Bounds, PathBuilder, canvas, fill, point, size};
use std::cell::Cell;
use std::rc::Rc;

/// The standalone command that compacts the native conversation; never text for the model.
pub(crate) const COMPACT_COMMAND: &str = "/compact";
/// The standalone command that starts a fresh provider session in the same conversation.
pub(crate) const CLEAR_COMMAND: &str = "/clear";
/// A heavy context. Below this a cold re-read is cheap, so compacting mostly loses context.
const COMPACT_TOKENS: u64 = 200_000;
/// Without a known expiry: long enough that the provider's prompt cache has expired.
const COMPACT_IDLE_MS: i64 = 70 * 60 * 1000;

/// What the provider last reported (`Session.contextUsage`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Reading {
    pub used: u64,
    pub window: Option<u64>,
    /// When Claude's prompt cache for the conversation expires (Unix ms), when it said.
    pub cache_expires_at: Option<i64>,
}

impl Reading {
    pub(crate) fn of(session: &Value) -> Option<Self> {
        let usage = &session["contextUsage"];
        Some(Self {
            used: usage["used"].as_u64()?,
            window: usage["window"].as_u64().filter(|window| *window > 0),
            cache_expires_at: usage["cacheExpiresAt"].as_i64(),
        })
    }
}

/// The window's state for the shell: the popup's animation, the ring's place and whether the
/// next send skips compacting.
#[derive(Default)]
pub(crate) struct State {
    popup: Option<Tween>,
    ring: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Shift-click on send: this message goes out without compacting first.
    pub(super) skip_compact: bool,
    /// The message being sent behind a compaction is not asked about compacting again.
    resending: bool,
    /// The conversation whose chip says "Manter": its next message goes out without compacting.
    pub(super) keep: Option<String>,
}

impl State {
    pub(crate) fn close(&mut self) -> bool {
        self.popup.take().is_some()
    }
}

/// Compact token count: 950, 9.5K, 120K, 1M, 1.2M. Values that would round up to 1000 of a
/// unit move to the next one, so 999,500 reads "1M", never "1000K".
pub(crate) fn format_tokens(value: u64) -> String {
    let trim = |text: String| text.strip_suffix(".0").map(str::to_owned).unwrap_or(text);
    if value < 1000 {
        return value.to_string();
    }
    if value < 999_500 {
        let thousands = value as f64 / 1000.;
        return if value < 9_950 {
            format!("{}K", trim(format!("{thousands:.1}")))
        } else {
            format!("{}K", thousands.round() as u64)
        };
    }
    let millions = value as f64 / 1_000_000.;
    if millions < 9.95 {
        format!("{}M", trim(format!("{millions:.1}")))
    } else {
        format!("{}M", millions.round() as u64)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Level {
    Normal,
    Warning,
    Danger,
}

pub(crate) struct Ring {
    pub ratio: f32,
    pub percent: u32,
    pub level: Level,
    /// No arc while usage rounds to 0%: a round cap on a zero-length arc would still draw a dot.
    pub arc: bool,
}

pub(crate) fn ring(used: u64, window: u64) -> Ring {
    let ratio = if window > 0 && used > 0 {
        (used as f64 / window as f64).min(1.)
    } else {
        0.
    };
    let percent = (ratio * 100.).round() as u32;
    let level = if ratio >= 0.9 {
        Level::Danger
    } else if ratio >= 0.75 {
        Level::Warning
    } else {
        Level::Normal
    };
    Ring {
        ratio: ratio as f32,
        percent,
        level,
        arc: percent > 0,
    }
}

/// Providers whose native adapter compacts on `/compact`.
fn compacts(provider: &str) -> bool {
    matches!(provider, "claude" | "codex")
}

fn busy(status: &str) -> bool {
    matches!(status, "starting" | "running" | "waiting")
}

pub(crate) fn is_compact_command(text: &str) -> bool {
    text.trim() == COMPACT_COMMAND
}

/// A typed `/clear`: the word alone, any case.
pub(crate) fn is_clear_command(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case(CLEAR_COMMAND)
}

/// Whether the provider keeps a session of its own to drop: Codex, Claude Code and OpenCode
/// resume one; the others replay the visible history each turn, so `/clear` would change nothing.
pub(crate) fn clears(provider: &str) -> bool {
    matches!(provider, "claude" | "codex" | "opencode")
}

/// Why `/clear` cannot run on this conversation now (its Portuguese is the key), or None. The
/// host refuses the same cases.
pub(crate) fn clear_refusal(provider: &str, status: &str) -> Option<&'static str> {
    if !clears(provider) {
        return Some("Só conversas do Codex, do Claude Code e do OpenCode podem ser limpas.");
    }
    if busy(status) {
        return Some("Espere o turno atual terminar");
    }
    None
}

/// Why Compact cannot run on this conversation now, as the message (its Portuguese is the key),
/// or None when it can. The host refuses the same cases; this answers before anything is sent.
pub(crate) fn refusal(provider: &str, status: &str, native: bool) -> Option<&'static str> {
    if !compacts(provider) {
        return Some("Só conversas do Codex e do Claude Code podem ser compactadas.");
    }
    if !native {
        return Some("Ainda não há o que compactar: esta conversa não começou.");
    }
    if busy(status) {
        return Some("Espere o turno atual terminar");
    }
    None
}

/// "Compact and send": a heavy Claude conversation coming back after a long pause compacts
/// first, so the next request does not resend the whole uncached context. Only Claude: its
/// prompt cache lapses after an hour; Codex caches differently.
pub(crate) fn compact_before_send(conversation: &Conversation, now_ms: i64) -> bool {
    if conversation.provider != "claude" || !conversation.native || busy(&conversation.status) {
        return false;
    }
    let Some(usage) = conversation.context else {
        return false;
    };
    if usage.used < COMPACT_TOKENS {
        return false;
    }
    // The cache's own expiry when Claude reported it; else the idle-time guess.
    if let Some(expires) = usage.cache_expires_at {
        return now_ms >= expires;
    }
    chrono::DateTime::parse_from_rfc3339(&conversation.last_activity)
        .is_ok_and(|last| now_ms - last.timestamp_millis() > COMPACT_IDLE_MS)
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The level colors of Sirus's `--warning` and `--danger`, in both themes.
fn level_color(level: Level) -> Option<gpui::Hsla> {
    let light = crate::theme::is_light();
    match level {
        Level::Normal => None,
        Level::Warning => Some(rgb(if light { 0x9d6800 } else { 0xf9b13e }).into()),
        Level::Danger => Some(rgb(if light { 0xcc272b } else { 0xf45959 }).into()),
    }
}

/// The ring itself: a 16 px circle of radius 5.5 and stroke 2, the track at a quarter of the
/// color and the arc starting at twelve o'clock with round caps.
fn paint_ring(bounds: Bounds<Pixels>, ring: &Ring, color: gpui::Hsla, window: &mut Window) {
    const RADIUS: f32 = 5.5;
    const STEPS: usize = 72;
    let centre = (
        f32::from(bounds.left()) + f32::from(bounds.size.width) / 2.,
        f32::from(bounds.top()) + f32::from(bounds.size.height) / 2.,
    );
    let at = |turn: f32| {
        let angle = (turn - 0.25) * std::f32::consts::TAU;
        (
            centre.0 + RADIUS * angle.cos(),
            centre.1 + RADIUS * angle.sin(),
        )
    };
    let mut track = PathBuilder::stroke(px(2.));
    let outline: Vec<Point<Pixels>> = (0..STEPS)
        .map(|step| {
            let (x, y) = at(step as f32 / STEPS as f32);
            point(px(x), px(y))
        })
        .collect();
    track.add_polygon(&outline, true);
    if let Ok(path) = track.build() {
        window.paint_path(
            path,
            gpui::Hsla {
                a: color.a * 0.25,
                ..color
            },
        );
    }
    if !ring.arc {
        return;
    }
    let steps = ((ring.ratio * STEPS as f32).ceil() as usize).max(2);
    let mut arc = PathBuilder::stroke(px(2.));
    for step in 0..=steps {
        let (x, y) = at(ring.ratio * step as f32 / steps as f32);
        if step == 0 {
            arc.move_to(point(px(x), px(y)));
        } else {
            arc.line_to(point(px(x), px(y)));
        }
    }
    if let Ok(path) = arc.build() {
        window.paint_path(path, color);
    }
    // Round caps: a dot of the stroke's width on each end.
    for turn in [0., ring.ratio] {
        let (x, y) = at(turn);
        window.paint_quad(
            fill(
                Bounds::new(point(px(x - 1.), px(y - 1.)), size(px(2.), px(2.))),
                color,
            )
            .corner_radii(px(1.)),
        );
    }
}

impl Shell {
    /// The ring on the composer shelf; nothing until both numbers are known.
    pub(super) fn context_ring(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let conversation = &self.conversations[self.active_conversation()];
        let usage = conversation.context?;
        let window = usage.window?;
        let reading = ring(usage.used, window);
        let open = self.context.popup.is_some();
        let base = level_color(reading.level).unwrap_or_else(|| content(0.5));
        let color = if open && reading.level == Level::Normal {
            content(0.7)
        } else {
            base
        };
        let store = self.context.ring.clone();
        Some(
            div()
                .id("composer-context")
                .relative()
                .size(px(24.))
                .flex_shrink_0()
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(open, |d| d.bg(content(0.05)))
                .hover(|s| s.bg(content(0.07)))
                .child(
                    canvas(move |bounds, _, _| store.set(Some(bounds)), |_, _, _, _| {})
                        .absolute()
                        .size_full(),
                )
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let side = 16.;
                            let inner = Bounds::new(
                                point(
                                    bounds.left() + (bounds.size.width - px(side)) / 2.,
                                    bounds.top() + (bounds.size.height - px(side)) / 2.,
                                ),
                                size(px(side), px(side)),
                            );
                            paint_ring(inner, &reading, color, window);
                        },
                    )
                    .size(px(16.)),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| this.toggle_context_popup(cx)))
                .into_any_element(),
        )
    }

    pub(super) fn toggle_context_popup(&mut self, cx: &mut Context<Self>) {
        if self.context.popup.take().is_some() {
            cx.notify();
            return;
        }
        // One floating thing at a time.
        self.composer_menu = None;
        self.add_menu = None;
        self.usage_popup = None;
        self.context.popup = Some(Tween::from_to(
            0.,
            1.,
            Instant::now(),
            if cx.reduce_motion() { 0. } else { MENU },
            Curve::Expo,
        ));
        cx.notify();
    }

    /// The popup above the ring: a line with the ring, "65%" and "647K / 1M", and a full-width
    /// Compact button, dimmed with the reason in a tooltip when compacting cannot run.
    pub(super) fn context_popup_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.context.popup.as_ref()?;
        let ix = self.active_conversation();
        let conversation = &self.conversations[ix];
        let usage = conversation.context?;
        let window = usage.window?;
        let anchor = self.context.ring.get()?;
        if !open.done(now) {
            *moving = true;
        }
        let phase = open.at(now).clamp(0., 1.);
        let reading = ring(usage.used, window);
        let refused = refusal(
            conversation.provider,
            &conversation.status,
            conversation.native,
        );
        let strong = reading.level != Level::Normal;
        let right = self.viewport_width - f32::from(anchor.right());
        let bottom = self.viewport_height - f32::from(anchor.top()) + 8. + (1. - phase) * -6.;
        let percent = reading.percent;
        let ring_color = level_color(reading.level).unwrap_or_else(|| content(0.7));
        let mut button = div()
            .id("context-compact")
            .h(px(28.))
            .w_full()
            .rounded(px(crate::theme::RADIUS_SM))
            .border_1()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .when(strong, |d| {
                d.border_color(transparent())
                    .bg(accent())
                    .text_color(on_accent())
            })
            .when(!strong, |d| {
                d.border_color(crate::theme::border())
                    .bg(content(0.03))
                    .text_color(content(0.95))
            })
            .child(icon("chevrons-down-up", 12.).text_color(if strong {
                on_accent()
            } else {
                content(0.6)
            }))
            .child(t("Compactar"));
        button = match refused {
            Some(reason) => button
                .opacity(0.5)
                .tooltip(message_actions::name_tip(t(reason))),
            None => button
                .cursor_pointer()
                .hover(move |s| {
                    if strong {
                        s.opacity(0.88)
                    } else {
                        s.bg(content(0.07))
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.conversations[ix].working() {
                        return;
                    }
                    this.context.popup = None;
                    this.compact_conversation(ix, None, cx);
                })),
        };
        Some(
            div()
                .id("context-popup")
                .occlude()
                .absolute()
                .bottom(px(bottom))
                .right(px(right))
                .w(px(208.))
                .opacity(phase)
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(7.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .h(px(22.))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    paint_ring(bounds, &reading, ring_color, window);
                                },
                            )
                            .size(px(14.))
                            .flex_shrink_0(),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(content(0.95))
                                .child(format!("{percent}%")),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(content(0.5))
                                .child(format!(
                                    "{} / {}",
                                    format_tokens(usage.used),
                                    format_tokens(window)
                                )),
                        ),
                )
                .child(button)
                .into_any_element(),
        )
    }

    /// Sends `/compact` alone. With `then`, that message is sent behind it ("Compact and
    /// send"): it queues while the compaction runs and goes out when it settles. Returns whether
    /// it was sent.
    pub(super) fn compact_conversation(
        &mut self,
        ix: usize,
        then: Option<composer_queue::Outgoing>,
        cx: &mut Context<Self>,
    ) -> bool {
        let conversation = &self.conversations[ix];
        if let Some(reason) = refusal(
            conversation.provider,
            &conversation.status,
            conversation.native,
        ) {
            self.problem = Some(t(reason).to_owned());
            cx.notify();
            return false;
        }
        let session_id = conversation.session_id.clone();
        // The session's own choices: a compaction changes none of them.
        let execution = json!({ "approval": conversation.approval, "effort": conversation.effort, "fast": conversation.fast, "planning": conversation.planning });
        let call = self.backend.invoke(
            "send_prompt",
            json!({ "request": { "sessionId": session_id, "prompt": COMPACT_COMMAND, "clientRequestId": uuid(), "execution": execution, "attachmentIds": [], "attachmentOwner": format!("session:{session_id}") } }),
        );
        self.problem = None;
        // The message waiting behind the compaction can't be sent twice meanwhile.
        self.queues.busy |= then.is_some();
        self.waiting
            .insert(call, Waiting::Compacted(ix, then.map(Box::new)));
        cx.notify();
        true
    }

    /// The host's answer to `/compact`: the session as it stands, then the message that waited.
    pub(super) fn compacted_reply(
        &mut self,
        ix: usize,
        then: Option<Box<composer_queue::Outgoing>>,
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        if then.is_some() {
            self.queues.busy = false;
        }
        if !ok {
            // Nothing was sent: the draft stays where it was.
            self.problem = Some(error_text(value));
            cx.notify();
            return;
        }
        self.session_updated(value, cx);
        // The divider glides to the top of the view, as a sent message does.
        self.pin_to_top(ix, cx);
        if let Some(out) = then {
            // The compaction is running (or done): the message queues behind it, or goes now.
            self.context.resending = true;
            self.dispatch(*out, false, false, cx);
            self.context.resending = false;
        }
    }

    /// A typed `/compact` is the command, never text for the model, a steer or a queued request.
    /// Returns true when the draft was that command (handled here, with or without a problem).
    pub(super) fn typed_compact(
        &mut self,
        ix: usize,
        text: &str,
        has_attachments: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !is_compact_command(text) {
            return false;
        }
        if has_attachments {
            self.problem = Some(
                t("Envie /compact sozinho, sem anexos e sem os modos equipe ou depuração.")
                    .to_owned(),
            );
            cx.notify();
            return true;
        }
        if self.compact_conversation(ix, None, cx) {
            self.drafts.remove(&ix);
            self.touch_drafts();
            self.marked = None;
        }
        true
    }

    /// A typed `/clear` is the command, never text for the model: the host drops the provider's
    /// session and the transcript gets its divider. True when the draft was that command.
    pub(super) fn typed_clear(
        &mut self,
        ix: usize,
        text: &str,
        has_attachments: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !is_clear_command(text) {
            return false;
        }
        let conversation = &self.conversations[ix];
        let reason = if has_attachments {
            Some("Envie /clear sozinho, sem anexos.")
        } else if conversation.session_id.is_empty() || !conversation.native {
            Some("Ainda não há o que limpar: esta conversa não começou.")
        } else {
            clear_refusal(conversation.provider, &conversation.status)
        };
        if let Some(reason) = reason {
            self.problem = Some(t(reason).to_owned());
            cx.notify();
            return true;
        }
        let session_id = conversation.session_id.clone();
        let call = self
            .backend
            .invoke("clear_session_context", json!({ "sessionId": session_id }));
        self.waiting.insert(call, Waiting::Session);
        self.problem = None;
        self.drafts.remove(&ix);
        self.touch_drafts();
        self.marked = None;
        cx.notify();
        true
    }

    /// Whether this message should compact the conversation first: idle, heavy, past its
    /// prompt cache, with no queue ahead of it and no team request.
    pub(super) fn compacts_first(&self, ix: usize) -> bool {
        let conversation = &self.conversations[ix];
        !self.context.resending
            && !self.context.skip_compact
            && !conversation.team
            && !conversation.working()
            && self.context.keep.as_deref() != Some(conversation.session_id.as_str())
            && compact_before_send(conversation, now_ms())
    }

    /// The chip beside send, in a heavy conversation idle past its cache: the other choice to
    /// "Compactar e enviar". "Manter 735K" keeps the history for that message and then reads
    /// "Mantendo 735K", whose click goes back to compacting.
    pub(super) fn compact_chip(&self, live: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let conversation = &self.conversations[self.active_conversation()];
        if live || conversation.team || !compact_before_send(conversation, now_ms()) {
            return None;
        }
        let used = conversation.context?.used;
        let session = conversation.session_id.clone();
        let keeping = self.context.keep.as_deref() == Some(session.as_str());
        let tokens = format_tokens(used);
        Some(
            div()
                .id("composer-compact-chip")
                .flex_shrink_0()
                .h(px(26.))
                .px(px(8.))
                .ml(px(2.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(12.))
                .text_color(content(if keeping { 0.55 } else { 0.85 }))
                .bg(content(if keeping { 0.05 } else { 0.09 }))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.14)))
                .tooltip(crate::shell::message_actions::name_tip(if keeping {
                    t("Clique para compactar antes de enviar")
                } else {
                    t("Clique para manter o histórico nesta mensagem")
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.context.keep = if keeping { None } else { Some(session.clone()) };
                    cx.notify();
                }))
                .child(format!(
                    "{} {tokens}",
                    t(if keeping { "Mantendo" } else { "Manter" })
                ))
                .into_any_element(),
        )
    }

    /// A compaction in the transcript: one quiet line between two hairlines instead of a
    /// "/compact" bubble and an empty reply.
    pub(super) fn compaction_row(
        &self,
        conversation: usize,
        index: usize,
        turn: &Turn,
        now: Instant,
        moving: &mut bool,
    ) -> AnyElement {
        let held = &self.conversations[conversation];
        // A compaction that failed or was stopped says so in the conversation's error, not here.
        let broken = index + 1 == held.turns.len()
            && !turn.live
            && matches!(held.status.as_str(), "failed" | "stopped");
        let label: AnyElement = if turn.live {
            *moving = true;
            shimmer(
                t("Compactando o contexto…").to_owned(),
                now.duration_since(self.epoch).as_secs_f32(),
            )
        } else if broken {
            div().child(t("Contexto não compactado")).into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(icon("check", 12.).text_color(content(0.5)))
                .child(t("Contexto compactado"))
                .into_any_element()
        };
        let line = || div().h(px(1.)).flex_1().bg(content(0.08));
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(4.))
            .text_size(px(12.))
            .text_color(content(0.5))
            .child(line())
            .child(label)
            .child(line())
            .into_any_element()
    }

    /// A `/clear` in the transcript: the same quiet line as a compaction, saying a new provider
    /// session starts here.
    pub(super) fn clear_row(&self) -> AnyElement {
        let line = || div().h(px(1.)).flex_1().bg(content(0.08));
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(4.))
            .text_size(px(12.))
            .text_color(content(0.5))
            .child(line())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(icon("check", 12.).text_color(content(0.5)))
                    .child(t("Contexto limpo · nova sessão")),
            )
            .child(line())
            .into_any_element()
    }

    /// The send button's state for the composer: whether a send now would compact first.
    pub(super) fn compact_send_pending(&self) -> bool {
        self.compacts_first(self.active_conversation())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(
        provider: &'static str,
        status: &str,
        used: u64,
        expires: Option<i64>,
    ) -> Conversation {
        let mut conversation = Conversation::placeholder();
        conversation.provider = provider;
        conversation.status = status.into();
        conversation.native = true;
        conversation.context = Some(Reading {
            used,
            window: Some(1_000_000),
            cache_expires_at: expires,
        });
        conversation
    }

    #[test]
    fn clear_is_typed_alone_and_for_providers_with_a_session_of_their_own() {
        assert!(is_clear_command(" /Clear ") && !is_clear_command("/clear now"));
        for provider in ["claude", "codex", "opencode"] {
            assert_eq!(clear_refusal(provider, "idle"), None);
            assert!(clear_refusal(provider, "running").is_some());
        }
        assert!(clear_refusal("cursor", "idle").is_some());
    }

    #[test]
    fn tokens_read_compactly() {
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(9_500), "9.5K");
        assert_eq!(format_tokens(647_000), "647K");
        assert_eq!(format_tokens(999_499), "999K");
        assert_eq!(format_tokens(999_500), "1M");
        assert_eq!(format_tokens(1_000_000), "1M");
        assert_eq!(format_tokens(1_240_000), "1.2M");
    }

    #[test]
    fn the_ring_has_levels_and_no_dot_at_zero() {
        let empty = ring(0, 1_000_000);
        assert!(!empty.arc && empty.percent == 0);
        assert!(!ring(1_000, 1_000_000).arc, "rounds to 0%");
        let ring = |used| self::ring(used, 1_000_000);
        assert_eq!(ring(647_000).percent, 65);
        assert_eq!(ring(647_000).level, Level::Normal);
        assert_eq!(ring(750_000).level, Level::Warning);
        assert_eq!(ring(900_000).level, Level::Danger);
        assert_eq!(ring(2_000_000).percent, 100);
    }

    #[test]
    fn compact_has_its_refusals_in_order() {
        assert!(refusal("opencode", "idle", true).is_some_and(|r| r.contains("Codex")));
        assert!(refusal("claude", "idle", false).is_some_and(|r| r.contains("não começou")));
        assert_eq!(
            refusal("claude", "running", true),
            Some("Espere o turno atual terminar")
        );
        assert_eq!(
            refusal("codex", "waiting", true),
            Some("Espere o turno atual terminar")
        );
        assert_eq!(refusal("claude", "completed", true), None);
        assert_eq!(refusal("codex", "failed", true), None);
        assert!(is_compact_command(" /compact ") && !is_compact_command("/compact now"));
    }

    #[test]
    fn a_heavy_claude_conversation_past_its_cache_compacts_before_sending() {
        let now = 10_000_000;
        // Past the cache's own expiry.
        assert!(compact_before_send(
            &conversation("claude", "completed", 650_000, Some(now - 1)),
            now
        ));
        assert!(!compact_before_send(
            &conversation("claude", "completed", 650_000, Some(now + 1)),
            now
        ));
        // Light, running, other providers and no thread never do.
        assert!(!compact_before_send(
            &conversation("claude", "completed", 20_000, Some(now - 1)),
            now
        ));
        assert!(!compact_before_send(
            &conversation("claude", "running", 650_000, Some(now - 1)),
            now
        ));
        assert!(!compact_before_send(
            &conversation("codex", "completed", 650_000, Some(now - 1)),
            now
        ));
        let mut fresh = conversation("claude", "completed", 650_000, Some(now - 1));
        fresh.native = false;
        assert!(!compact_before_send(&fresh, now));
        // No reported expiry: the idle-time guess.
        let mut idle = conversation("claude", "completed", 650_000, None);
        idle.last_activity = chrono::DateTime::from_timestamp_millis(now - COMPACT_IDLE_MS - 1)
            .unwrap()
            .to_rfc3339();
        assert!(compact_before_send(&idle, now));
        idle.last_activity = chrono::DateTime::from_timestamp_millis(now - 1000)
            .unwrap()
            .to_rfc3339();
        assert!(!compact_before_send(&idle, now));
    }

    #[test]
    fn a_reading_needs_used_and_ignores_a_zero_window() {
        let session = json!({"contextUsage": {"used": 5, "window": 0, "cacheExpiresAt": 9}});
        let reading = Reading::of(&session).unwrap();
        assert_eq!(
            (reading.used, reading.window, reading.cache_expires_at),
            (5, None, Some(9))
        );
        assert!(Reading::of(&json!({"contextUsage": null})).is_none());
    }
}
