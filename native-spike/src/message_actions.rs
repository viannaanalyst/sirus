//! The row of actions under a finished reply, as Sirus Code shows it (MessageActions.tsx,
//! UndoTurnButton.tsx, ADR-187): copy the answer, try the latest turn again, undo the turn's file
//! changes after a read-only check and a confirmation, and a `⋯` button. The `⋯` opens a popup
//! with the side chat, fork, handoff and second opinion (handoff.rs opens the last two's popups).
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state. It
//! draws nothing on its own: the row fades in on hover of the turn (`group_hover`), and the only
//! timer is the copy check's, started by a click.

use super::*;
use gpui::{AnyView, App, ClickEvent, ClipboardItem, Pixels, Point, Render, SharedString};
use std::time::Duration;

/// The `⋯` popup's width and the height it needs per row, for choosing its side of the button.
const MORE_WIDTH: f32 = 220.;
const MORE_ROW: f32 = 30.;

/// How long the copy button shows its check mark.
const COPIED_FOR: Duration = Duration::from_millis(1400);
/// Files named in the undo confirmation before the rest are counted.
const LISTED_FILES: usize = 5;

/// One file of a turn's review, as `activity.review.files` sends it (ADR-061, ADR-164).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReviewFile {
    pub path: String,
    pub binary: bool,
    /// A retained text diff, or one the transcript left out for the host to fetch (ADR-164).
    pub reversible: bool,
    pub undone: bool,
    /// Included in a commit made through Sirus: it leaves the review (ADR-108).
    pub committed: bool,
    /// Added and removed lines (`additions`, `deletions`); None when the host did not send them.
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    /// The retained diff text; None when it is left out (`diffOmitted`) or not kept.
    pub diff: Option<String>,
    pub omitted: bool,
}

/// A turn's changed files (`activity.review`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Review {
    pub files: Vec<ReviewFile>,
    pub expired: bool,
}

impl Review {
    pub(crate) fn of(value: &Value) -> Option<Self> {
        let files = value["files"].as_array()?;
        Some(Self {
            files: files
                .iter()
                .map(|file| ReviewFile {
                    path: file["path"].as_str().unwrap_or("").to_owned(),
                    binary: file["binary"].as_bool().unwrap_or(false),
                    reversible: file["diff"].is_string()
                        || file["diffOmitted"].as_bool().unwrap_or(false),
                    undone: file["undoneAt"].is_string(),
                    committed: file["committedAt"].is_string(),
                    additions: file["additions"].as_u64(),
                    deletions: file["deletions"].as_u64(),
                    diff: file["diff"].as_str().map(str::to_owned),
                    omitted: file["diffOmitted"].as_bool().unwrap_or(false),
                })
                .collect(),
            expired: value["expired"].as_bool().unwrap_or(false),
        })
    }

    /// The files undo can still put back (`undoableFiles` in UndoTurnButton.tsx).
    pub(crate) fn undoable(&self) -> Vec<&ReviewFile> {
        self.files
            .iter()
            .filter(|file| !file.undone && !file.binary && file.reversible)
            .collect()
    }

    fn all_undone(&self) -> bool {
        !self.files.is_empty() && self.files.iter().all(|file| file.undone)
    }
}

/// The window's state for the row: which copy is checked, the undo outcome shown under a turn,
/// the turns whose changes went back in this window, the request in flight, and the `⋯` popup.
#[derive(Default)]
pub(crate) struct State {
    copied: Option<String>,
    notice: Option<(String, String)>,
    undone: Vec<String>,
    pending: Option<String>,
    pub(crate) more: Option<More>,
}

/// The `⋯` popup open over one turn: its session and turn key, and where the button was pressed.
pub(crate) struct More {
    pub session: String,
    pub key: String,
    pub anchor: Point<Pixels>,
    pub open: Tween,
}

/// What the row knows about one turn.
pub(crate) struct Facts<'a> {
    /// The turn has finished (its reply is no longer live).
    pub finished: bool,
    /// It has answer text to copy or to ask about.
    pub answered: bool,
    /// It is the conversation's newest turn.
    pub latest: bool,
    /// The conversation is working; nothing may be retried or undone meanwhile.
    pub working: bool,
    pub review: Option<&'a Review>,
    /// Its changes went back from this window already.
    pub undone_here: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Undo {
    Hidden,
    /// Every file is back as it was: the row says so instead of the button.
    Done,
    /// The button; disabled while the conversation works.
    Offered { enabled: bool },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Shown {
    pub copy: bool,
    pub retry: bool,
    pub undo: Undo,
    pub side_chat: bool,
}

/// Which actions a turn's row shows (the rules of MessageActions.tsx and TurnChangeSummary.tsx).
pub(crate) fn visible(facts: &Facts) -> Shown {
    let undo = match facts.review {
        Some(review) if facts.finished && !review.expired && !review.files.is_empty() => {
            if facts.undone_here || review.all_undone() {
                Undo::Done
            } else if review.undoable().is_empty() {
                Undo::Hidden
            } else {
                Undo::Offered {
                    enabled: !facts.working,
                }
            }
        }
        _ => Undo::Hidden,
    };
    Shown {
        copy: facts.finished && facts.answered,
        retry: facts.finished && facts.latest && !facts.working,
        undo,
        side_chat: facts.finished && facts.answered,
    }
}

/// The rows of the `⋯` popup, in their order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    SideChat,
    Fork,
    Handoff,
    Opinion,
}

/// One row of the popup: hidden when its rule does not apply, dimmed while its action cannot run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MoreRow {
    pub row: Row,
    pub enabled: bool,
}

/// Which of the fork, handoff and second opinion actions may run now: the conversation is idle
/// and, for the two popups, another provider is on offer and no handoff is under way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Can {
    pub fork: bool,
    pub handoff: bool,
    pub opinion: bool,
}

/// The `⋯` popup's rows for a turn: the side chat when the answer is there, then the three
/// reply actions, each only when `handoff::shown` says the turn has a settled reply for it.
pub(crate) fn more_rows(shown: &Shown, reply: &handoff::Shown, can: Can) -> Vec<MoreRow> {
    let mut rows = Vec::new();
    if shown.side_chat {
        rows.push(MoreRow {
            row: Row::SideChat,
            enabled: true,
        });
    }
    if reply.fork {
        rows.push(MoreRow {
            row: Row::Fork,
            enabled: can.fork,
        });
    }
    if reply.handoff {
        rows.push(MoreRow {
            row: Row::Handoff,
            enabled: can.handoff,
        });
    }
    if reply.opinion {
        rows.push(MoreRow {
            row: Row::Opinion,
            enabled: can.opinion,
        });
    }
    rows
}

/// A small label shown on hover of a row button, through gpui's tooltip.
struct NameTip(&'static str);

impl Render for NameTip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(crate::theme::RADIUS_MD))
            .border_1()
            .border_color(content(0.12))
            .bg(floating_fill())
            .text_size(px(12.))
            .text_color(content(0.92))
            .child(self.0)
    }
}

/// The tooltip builder for a button's name: the label, shown on hover.
pub(crate) fn name_tip(name: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| cx.new(|_| NameTip(name)).into()
}

/// One click's work, carried into the handlers.
#[derive(Clone)]
pub(crate) enum Act {
    Copy { key: String, text: String },
    Retry { session: String },
    Undo {
        key: String,
        session: String,
        message: String,
        files: Vec<String>,
    },
}

/// Settles a request's reply: the value, or the error's text.
pub(crate) async fn settle(rx: async_channel::Receiver<Result<Value, Value>>) -> Result<Value, String> {
    match rx.recv().await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error["message"]
            .as_str()
            .or(error.as_str())
            .map_or_else(|| error.to_string(), str::to_owned)),
        Err(_) => Err(t("O motor do Sirus não respondeu.").to_owned()),
    }
}

/// The files of a block or a confirmation: the first, then how many more.
fn files_text(files: &[String]) -> String {
    match files {
        [] => t("estes arquivos").to_owned(),
        [only] => only.clone(),
        [first, rest @ ..] => tf("{} e mais {} arquivos", &[first, &rest.len()]),
    }
}

/// Why undo is refused, from the host's `UndoBlock` (ADR-187), in the window's language.
pub(crate) fn block_text(block: &Value) -> String {
    let files = || -> Vec<String> {
        block["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|file| file.as_str().map(str::to_owned))
            .collect()
    };
    match block["kind"].as_str() {
        Some("headMoved") => tf(
            "O HEAD do Git mudou desde esta resposta ({} para {}): houve commit, checkout, rebase ou pull. Nada será desfeito. Abra o diff e reverta à mão, ou use git revert no novo commit.",
            &[
                &short(block["was"].as_str().unwrap_or("")),
                &short(block["now"].as_str().unwrap_or("")),
            ],
        ),
        Some("otherSession") if block["running"].as_bool() == Some(true) => tf(
            "\"{}\" está trabalhando nesta pasta agora. Espere terminar para não sobrescrever as edições dela.",
            &[&block["title"].as_str().unwrap_or("")],
        ),
        Some("otherSession") => tf(
            "\"{}\" tem mudanças não mantidas em {} nesta pasta. Mantenha ou desfaça nessa sessão primeiro.",
            &[&block["title"].as_str().unwrap_or(""), &files_text(&files())],
        ),
        Some("changedAgain") if block["laterTurn"].as_bool() == Some(true) => tf(
            "{} mudou de novo em uma resposta posterior desta sessão. Desfaça essa resposta primeiro, ou reverta à mão pelo diff.",
            &[&files_text(&files())],
        ),
        Some("changedAgain") => tf(
            "{} foi editado depois desta resposta, nos mesmos trechos. Nada será desfeito: reverta à mão pelo diff.",
            &[&files_text(&files())],
        ),
        _ => t("Nada foi desfeito.").to_owned(),
    }
}

/// A commit id as a person reads it.
fn short(commit: &str) -> String {
    commit.chars().take(8).collect()
}

/// A turn's key in this state: its reply's id, or its place when the reply has no review.
fn key_of(turn: &Turn, index: usize) -> String {
    turn.message_id
        .clone()
        .unwrap_or_else(|| format!("turn {index}"))
}

impl Shell {
    /// The row under a turn, or None when no action applies to it.
    pub(super) fn turn_actions(
        &self,
        conversation: usize,
        index: usize,
        turn: &Turn,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = &self.message_actions;
        let conv = &self.conversations[conversation];
        let key = key_of(turn, index);
        let shown = self.shown_of(conversation, index, turn);
        let mut items: Vec<AnyElement> = Vec::new();
        if shown.copy {
            let copied = state.copied.as_deref() == Some(key.as_str());
            items.push(self.action_button(
                ("turn-copy", index),
                if copied { "check" } else { "copy" },
                t("Copiar"),
                true,
                Act::Copy {
                    key: key.clone(),
                    text: turn.answer.clone(),
                },
                cx,
            ));
        }
        if shown.retry {
            items.push(self.action_button(
                ("turn-retry", index),
                "refresh",
                t("Tentar de novo"),
                true,
                Act::Retry {
                    session: conv.session_id.clone(),
                },
                cx,
            ));
        }
        let mut status: Option<String> = None;
        match shown.undo {
            Undo::Offered { enabled } => {
                let files = turn
                    .review
                    .as_ref()
                    .map(|review| {
                        review
                            .undoable()
                            .iter()
                            .map(|file| file.path.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                items.push(self.action_button(
                    ("turn-undo", index),
                    "undo",
                    t("Desfazer alterações"),
                    enabled && state.pending.is_none(),
                    Act::Undo {
                        key: key.clone(),
                        session: conv.session_id.clone(),
                        message: turn.message_id.clone().unwrap_or_default(),
                        files,
                    },
                    cx,
                ));
            }
            Undo::Done => status = Some(t("Alterações desfeitas").to_owned()),
            Undo::Hidden => {}
        }
        if !self.more_rows_of(conversation, index, turn).is_empty() {
            items.push(self.more_button(index, conv.session_id.clone(), key.clone(), cx));
        }
        let notice = state
            .notice
            .as_ref()
            .filter(|(for_key, _)| *for_key == key)
            .map(|(_, text)| text.clone());
        if items.is_empty() && status.is_none() && notice.is_none() {
            return None;
        }
        let group: SharedString = format!("turn-actions-{index}").into();
        let row = (!items.is_empty()).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .mt(px(4.))
                .opacity(0.)
                .group_hover(group, |s| s.opacity(1.))
                .children(items)
                .into_any_element()
        });
        Some(
            div()
                .flex()
                .flex_col()
                .children(row)
                .when_some(status.or(notice.clone()), |d, text| {
                    d.child(
                        div()
                            .mt(px(4.))
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(content(0.6))
                            .child(text),
                    )
                })
                .into_any_element(),
        )
    }

    /// What the row's rules say about a turn: the same facts for the buttons and the `⋯` popup.
    fn shown_of(&self, conversation: usize, index: usize, turn: &Turn) -> Shown {
        let conv = &self.conversations[conversation];
        visible(&Facts {
            finished: !turn.live,
            answered: !turn.answer.trim().is_empty(),
            latest: index + 1 == conv.turns.len(),
            working: conv.working(),
            review: turn.review.as_ref(),
            undone_here: self.message_actions.undone.contains(&key_of(turn, index)),
        })
    }

    /// The `⋯` popup's rows for a turn, with the fork, handoff and second opinion rules of handoff.rs.
    fn more_rows_of(&self, conversation: usize, index: usize, turn: &Turn) -> Vec<MoreRow> {
        let shown = self.shown_of(conversation, index, turn);
        let settled = handoff::shown(
            !turn.live,
            !turn.answer.trim().is_empty(),
            turn.reply.is_some(),
        );
        more_rows(&shown, &settled, self.handoff_can(conversation))
    }

    fn action_button(
        &self,
        id: (&'static str, usize),
        glyph: &'static str,
        name: &'static str,
        enabled: bool,
        act: Act,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = div()
            .id(id)
            .size(px(24.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .text_color(content(0.5))
            .tooltip(name_tip(name))
            .child(icon(glyph, 13.));
        if !enabled {
            return button.opacity(0.4).into_any_element();
        }
        button
            .cursor_pointer()
            .hover(|s| s.bg(content(0.07)).text_color(content(0.9)))
            .on_click(cx.listener(move |this, _, _, cx| this.run_action(act.clone(), cx)))
            .into_any_element()
    }

    /// The `⋯` button of a turn's row; a press opens or closes its popup where it was pressed.
    fn more_button(
        &self,
        index: usize,
        session: String,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dot = || div().size(px(3.)).rounded_full().bg(content(0.5));
        div()
            .id(("turn-more", index))
            .size(px(24.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(2.))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.07)))
            .tooltip(name_tip(t("Mais ações")))
            .children([dot(), dot(), dot()])
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                this.open_more(session.clone(), key.clone(), event.position(), cx)
            }))
            .into_any_element()
    }

    /// The `⋯` popup over a turn's row, above or below its button, animated in and kept on screen.
    /// A click outside it or Esc closes it. Its rows go through `more_row`, the one place to swap.
    pub(super) fn more_menu_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let popup = self.message_actions.more.as_ref()?;
        if !popup.open.done(now) {
            *moving = true;
        }
        let shown = popup.open.at(now).clamp(0., 1.);
        let conversation = self
            .conversations
            .iter()
            .position(|c| c.session_id == popup.session)?;
        let conv = &self.conversations[conversation];
        let (index, turn) = conv
            .turns
            .iter()
            .enumerate()
            .find(|(i, turn)| key_of(turn, *i) == popup.key)?;
        let rows = self.more_rows_of(conversation, index, turn);
        if rows.is_empty() {
            return None;
        }
        let reply = turn.reply.clone().unwrap_or_default();
        let at = popup.anchor;
        let (x, y) = (f32::from(at.x), f32::from(at.y));
        let height = rows.len() as f32 * MORE_ROW + 10.;
        let room_above = y - height - 14. >= 0.;
        let room_below = self.viewport_height - y - height - 6. >= 0.;
        let above = room_above || !room_below;
        let left = (x - 12.).min(self.viewport_width - MORE_WIDTH - 8.).max(8.);
        let slide = (1. - shown) * -4.;
        let items: Vec<AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(i, item)| self.more_row(i, item, &popup.session, &reply, at, cx))
            .collect();
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.message_actions.more = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("turn-more-menu")
                        .occlude()
                        .absolute()
                        .left(px(left))
                        .when(above, |d| {
                            d.bottom(px((self.viewport_height - y + 6. + slide).max(8.)))
                        })
                        .when(!above, |d| d.top(px((y + 6. + slide).max(8.))))
                        .w(px(MORE_WIDTH))
                        .p(px(4.))
                        .flex()
                        .flex_col()
                        .opacity(shown)
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(content(0.12))
                        .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(crate::hover_track::list("turn-more-menu").pill())
                        .children(items),
                )
                .into_any_element(),
        )
    }

    /// One row of the `⋯` popup. The only place the rows are drawn: swap this for the shared
    /// sliding hover when it lands. A row that cannot run now is dimmed and takes no press.
    fn more_row(
        &self,
        i: usize,
        item: MoreRow,
        session: &str,
        reply: &str,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (glyph, label) = match item.row {
            Row::SideChat => ("message-square", t("Chat lateral")),
            Row::Fork => ("branch", t("Fork")),
            Row::Handoff => ("handoff", t("Handoff")),
            Row::Opinion => ("messages", t("Segunda opinião")),
        };
        let row = div()
            .id(("turn-more-row", i))
            .h(px(MORE_ROW))
            .px(px(8.))
            .rounded(px(crate::theme::RADIUS_MD))
            .flex()
            .items_center()
            .gap(px(9.))
            .text_size(px(13.))
            .text_color(content(0.92))
            .child(icon(glyph, 14.).text_color(content(0.65)))
            .child(div().flex_1().min_w_0().truncate().child(label));
        if !item.enabled {
            return row.opacity(0.4).into_any_element();
        }
        let (session, reply) = (session.to_owned(), reply.to_owned());
        let row = row.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
            this.run_more(item.row, &session, &reply, at, cx)
        }));
        crate::hover_track::list("turn-more-menu")
            .row(row, i, false)
            .into_any_element()
    }

    /// Opens the `⋯` popup of a turn, or closes it when it is already open for that turn.
    fn open_more(
        &mut self,
        session: String,
        key: String,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let same = self
            .message_actions
            .more
            .as_ref()
            .is_some_and(|open| open.session == session && open.key == key);
        self.message_actions.more = if same {
            None
        } else {
            Some(More {
                session,
                key,
                anchor: at,
                open: Tween::from_to(
                    0.,
                    1.,
                    Instant::now(),
                    if cx.reduce_motion() { 0. } else { MENU },
                    Curve::Expo,
                ),
            })
        };
        cx.notify();
    }

    /// A row's action: the popup closes, then the side chat, a fork, or the handoff and second
    /// opinion popups (handoff.rs), which open where the `⋯` button was pressed.
    fn run_more(
        &mut self,
        row: Row,
        session: &str,
        reply: &str,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.message_actions.more = None;
        match row {
            Row::SideChat => self.open_pane(4, cx),
            Row::Fork => self.fork_turn(session, reply, cx),
            Row::Handoff => self.open_popup(handoff::Kind::Handoff, session, reply, at, cx),
            Row::Opinion => self.open_popup(handoff::Kind::Opinion, session, reply, at, cx),
        }
        cx.notify();
    }

    fn run_action(&mut self, act: Act, cx: &mut Context<Self>) {
        match act {
            Act::Copy { key, text } => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.message_actions.copied = Some(key.clone());
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(COPIED_FOR).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.message_actions.copied.as_deref() == Some(key.as_str()) {
                            this.message_actions.copied = None;
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            Act::Retry { session } => {
                let rx = self
                    .backend
                    .request("retry_last_turn", json!({ "sessionId": session }));
                cx.spawn(async move |this, cx| {
                    let result = settle(rx).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Err(problem) = result {
                            this.problem = Some(problem);
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            Act::Undo {
                key,
                session,
                message,
                files,
            } => self.check_undo(key, session, message, files, cx),
        }
        cx.notify();
    }

    /// Read-only check first (ADR-187): a refusal is shown under the turn, else the confirmation
    /// lists the files. A failed check still asks, as UndoTurnButton does.
    fn check_undo(
        &mut self,
        key: String,
        session: String,
        message: String,
        files: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        self.message_actions.pending = Some(key.clone());
        self.message_actions.notice = None;
        let rx = self.backend.request(
            "check_turn_undo",
            json!({ "request": { "sessionId": session, "messageId": message } }),
        );
        cx.spawn(async move |this, cx| {
            let found = settle(rx).await.ok().filter(|block| !block.is_null());
            let _ = this.update(cx, |this, cx| {
                this.message_actions.pending = None;
                match found {
                    Some(block) => this.message_actions.notice = Some((key, block_text(&block))),
                    None => this.ask_undo(key, session, message, files, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ask_undo(
        &mut self,
        key: String,
        session: String,
        message: String,
        files: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let listed: Vec<&str> = files.iter().take(LISTED_FILES).map(String::as_str).collect();
        let mut text = tf("{} arquivos podem ser desfeitos.", &[&files.len()]);
        text.push(' ');
        text.push_str(&listed.join(", "));
        if files.len() > LISTED_FILES {
            text.push(' ');
            text.push_str(&tf(
                "e mais {} arquivos",
                &[&(files.len() - LISTED_FILES)],
            ));
        }
        text.push_str(". ");
        text.push_str(t("Os arquivos voltam a como estavam antes desta resposta. Um arquivo mudado de novo depois, nos mesmos trechos, fica como está. Nada é commitado nem preparado."));
        self.ask(
            t("Desfazer as mudanças deste turno?").to_owned(),
            text,
            t("Desfazer"),
            move |this: &mut Shell, cx: &mut Context<Shell>| this.run_undo(key, session, message, cx),
            cx,
        );
    }

    fn run_undo(&mut self, key: String, session: String, message: String, cx: &mut Context<Self>) {
        self.message_actions.pending = Some(key.clone());
        let rx = self.backend.request(
            "undo_turn_changes",
            json!({ "request": { "sessionId": session, "messageId": message, "confirm": true } }),
        );
        cx.spawn(async move |this, cx| {
            let result = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.message_actions;
                state.pending = None;
                match result {
                    Ok(outcome) => {
                        let refused = outcome["refused"].as_array().map_or(0, Vec::len);
                        if refused == 0 {
                            state.undone.push(key.clone());
                            state.notice = None;
                        } else {
                            let text = tf(
                                "{} mudaram de novo depois e ficaram como estão",
                                &[&refused],
                            );
                            state.notice = Some((key, text));
                        }
                    }
                    Err(problem) => state.notice = Some((key, problem)),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(files: &[(&str, bool, bool, bool)], expired: bool) -> Review {
        Review {
            files: files
                .iter()
                .map(|&(path, binary, reversible, undone)| ReviewFile {
                    path: path.to_owned(),
                    binary,
                    reversible,
                    undone,
                    committed: false,
                    additions: None,
                    deletions: None,
                    diff: None,
                    omitted: false,
                })
                .collect(),
            expired,
        }
    }

    fn facts<'a>(
        finished: bool,
        answered: bool,
        latest: bool,
        working: bool,
        review: Option<&'a Review>,
    ) -> Facts<'a> {
        Facts {
            finished,
            answered,
            latest,
            working,
            review,
            undone_here: false,
        }
    }

    #[test]
    fn a_live_turn_shows_nothing() {
        let changes = review(&[("a.rs", false, true, false)], false);
        let shown = visible(&facts(false, true, true, false, Some(&changes)));
        assert_eq!(
            shown,
            Shown {
                copy: false,
                retry: false,
                undo: Undo::Hidden,
                side_chat: false,
            }
        );
    }

    #[test]
    fn an_older_answer_offers_copy_and_side_chat_only() {
        let shown = visible(&facts(true, true, false, false, None));
        assert!(shown.copy && shown.side_chat);
        assert!(!shown.retry);
        assert_eq!(shown.undo, Undo::Hidden);
    }

    #[test]
    fn retry_only_on_the_latest_finished_turn_when_idle() {
        assert!(visible(&facts(true, true, true, false, None)).retry);
        assert!(!visible(&facts(true, true, true, true, None)).retry);
        assert!(!visible(&facts(true, true, false, false, None)).retry);
    }

    #[test]
    fn a_turn_without_text_still_offers_retry_and_undo() {
        let changes = review(&[("a.rs", false, true, false)], false);
        let shown = visible(&facts(true, false, true, false, Some(&changes)));
        assert!(!shown.copy && !shown.side_chat && shown.retry);
        assert_eq!(shown.undo, Undo::Offered { enabled: true });
    }

    #[test]
    fn undo_is_disabled_while_the_conversation_works() {
        let changes = review(&[("a.rs", false, true, false)], false);
        let shown = visible(&facts(true, true, false, true, Some(&changes)));
        assert_eq!(shown.undo, Undo::Offered { enabled: false });
    }

    #[test]
    fn undo_needs_a_reversible_file_and_no_expiry() {
        let binary_only = review(&[("logo.png", true, true, false)], false);
        assert_eq!(
            visible(&facts(true, true, false, false, Some(&binary_only))).undo,
            Undo::Hidden
        );
        let expired = review(&[("a.rs", false, true, false)], true);
        assert_eq!(
            visible(&facts(true, true, false, false, Some(&expired))).undo,
            Undo::Hidden
        );
        let omitted = review(&[("a.rs", false, true, false)], false);
        assert!(matches!(
            visible(&facts(true, true, false, false, Some(&omitted))).undo,
            Undo::Offered { .. }
        ));
    }

    #[test]
    fn every_file_back_shows_done() {
        let all = review(&[("a.rs", false, true, true), ("b.rs", false, true, true)], false);
        assert_eq!(
            visible(&facts(true, true, false, false, Some(&all))).undo,
            Undo::Done
        );
        let changes = review(&[("a.rs", false, true, false)], false);
        let mut done_here = facts(true, true, false, false, Some(&changes));
        done_here.undone_here = true;
        assert_eq!(visible(&done_here).undo, Undo::Done);
    }

    #[test]
    fn a_turn_without_changes_has_no_undo() {
        let none = review(&[], false);
        assert_eq!(
            visible(&facts(true, true, false, false, Some(&none))).undo,
            Undo::Hidden
        );
    }

    const ALL_CAN: Can = Can {
        fork: true,
        handoff: true,
        opinion: true,
    };

    /// The `⋯` popup's rows for a settled-or-live turn, as `turn_actions` builds them.
    fn more(finished: bool, answered: bool, has_reply: bool, can: Can) -> Vec<MoreRow> {
        let shown = visible(&facts(finished, answered, true, false, None));
        let settled = handoff::shown(finished, answered, has_reply);
        more_rows(&shown, &settled, can)
    }

    fn enabled_rows(rows: &[MoreRow]) -> Vec<(Row, bool)> {
        rows.iter().map(|row| (row.row, row.enabled)).collect()
    }

    #[test]
    fn a_live_turn_has_no_popup_rows() {
        assert!(more(false, true, true, ALL_CAN).is_empty());
    }

    #[test]
    fn a_settled_answer_lists_all_four_rows_in_order() {
        assert_eq!(
            enabled_rows(&more(true, true, true, ALL_CAN)),
            [
                (Row::SideChat, true),
                (Row::Fork, true),
                (Row::Handoff, true),
                (Row::Opinion, true),
            ]
        );
    }

    #[test]
    fn a_turn_without_text_offers_only_the_fork() {
        assert_eq!(
            enabled_rows(&more(true, false, true, ALL_CAN)),
            [(Row::Fork, true)]
        );
    }

    #[test]
    fn a_turn_without_a_reply_id_offers_only_the_side_chat() {
        assert_eq!(
            enabled_rows(&more(true, true, false, ALL_CAN)),
            [(Row::SideChat, true)]
        );
    }

    #[test]
    fn a_working_conversation_keeps_the_reply_rows_dimmed() {
        let busy = Can::default();
        assert_eq!(
            enabled_rows(&more(true, true, true, busy)),
            [
                (Row::SideChat, true),
                (Row::Fork, false),
                (Row::Handoff, false),
                (Row::Opinion, false),
            ]
        );
    }

    #[test]
    fn a_handoff_without_another_provider_is_dimmed_not_hidden() {
        let alone = Can {
            handoff: false,
            ..ALL_CAN
        };
        assert_eq!(
            enabled_rows(&more(true, true, true, alone)),
            [
                (Row::SideChat, true),
                (Row::Fork, true),
                (Row::Handoff, false),
                (Row::Opinion, true),
            ]
        );
    }

    #[test]
    fn review_reads_the_host_shape() {
        let value = json!({
            "files": [
                { "path": "a.rs", "diff": "@@", "binary": false },
                { "path": "b.rs", "diff": null, "diffOmitted": true, "binary": false },
                { "path": "c.rs", "diff": "@@", "binary": false, "undoneAt": "2026-11-05T10:00:00Z" },
                { "path": "d.png", "diff": null, "binary": true }
            ],
            "expired": false
        });
        let parsed = Review::of(&value).expect("files present");
        let undoable: Vec<&str> = parsed.undoable().iter().map(|f| f.path.as_str()).collect();
        assert_eq!(undoable, ["a.rs", "b.rs"]);
        assert!(Review::of(&json!({})).is_none());
    }
}
