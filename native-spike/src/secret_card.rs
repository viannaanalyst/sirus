//! The secret request card (ADR-198): an agent asks for a key and the person types it here, masked,
//! and picks where it is kept: any mix of the linked Infisical project's environments and the
//! workspace's `.env.local`, or only this conversation. Or "Usar do Infisical…" hands over a key that is
//! already there. The value is typed in a real field (caret, selection, ⌘A, paste, undo, arrows,
//! a reveal toggle), sent once to the host, and never logged or shown to the agent.

use super::*;
use gpui::{Hsla, rgb};

const AMBER: u32 = 0xf0a35e;
/// How many characters of a long value fit the field; the window follows the caret.
const VISIBLE: usize = 46;
const MAX_UNDO: usize = 100;
/// A press that closed a menu from outside must not be read as the click that reopens it.
const REOPEN_GUARD: Duration = Duration::from_millis(250);
/// The error color of the field's line, the same red the browser panel uses.
const DANGER: u32 = 0xec8a85;

fn amber(alpha: f32) -> Hsla {
    Hsla {
        a: alpha,
        ..Hsla::from(rgb(AMBER))
    }
}

/// Where the typed value is kept.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Dest {
    /// An environment of the linked Infisical project.
    Infisical(String),
    /// `.env.local` in the conversation's workspace.
    DotEnv,
    /// Only for this conversation (the agent's `use_secret`).
    Conversation,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum CardMenu {
    Where,
    From,
}

pub(crate) enum Keys {
    Idle,
    Loading,
    Names(Vec<String>),
    Failed(String),
}

pub(crate) struct SecretCard {
    pub request: String,
    pub draft: Draft,
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    pub reveal: bool,
    pub focused: bool,
    /// What the person ticked; `None` follows the default.
    pub chosen: Option<Vec<Dest>>,
    pub menu: Option<CardMenu>,
    /// The menu a press outside just closed, and when; its button's click then stays closed.
    menu_closed: Option<(CardMenu, Instant)>,
    pub keys: Keys,
    /// The save is on its way to the host; the card stays until it answers.
    pub saving: Option<String>,
    /// Why the last save failed, shown under the field in the person's language.
    pub error: Option<String>,
}

impl Drop for SecretCard {
    fn drop(&mut self) {
        for text in std::iter::once(&mut self.draft.text)
            .chain(self.undo.iter_mut().map(|(text, _)| text))
            .chain(self.redo.iter_mut().map(|(text, _)| text))
        {
            let mut bytes = std::mem::take(text).into_bytes();
            bytes.iter_mut().for_each(|byte| *byte = 0);
            std::hint::black_box(&bytes);
        }
    }
}

impl SecretCard {
    pub fn new(request: String) -> Self {
        Self {
            request,
            draft: Draft::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            reveal: false,
            focused: false,
            chosen: None,
            menu: None,
            menu_closed: None,
            keys: Keys::Idle,
            saving: None,
            error: None,
        }
    }

    /// A mouse press outside the open menu closes it.
    pub fn close_menu_outside(&mut self, now: Instant) {
        if let Some(kind) = self.menu.take() {
            self.menu_closed = Some((kind, now));
        }
    }

    /// The menu's button: opens it, or closes it when it is open. True when it opened. A click
    /// that follows the press which just closed this menu from outside keeps it closed.
    pub fn toggle_menu(&mut self, kind: CardMenu, now: Instant) -> bool {
        if self.menu == Some(kind) {
            self.menu = None;
            return false;
        }
        let just_closed = self
            .menu_closed
            .is_some_and(|(was, at)| was == kind && now.duration_since(at) < REOPEN_GUARD);
        if just_closed {
            return false;
        }
        self.menu = Some(kind);
        true
    }

    /// The host's answer to a save. A failure keeps the card and the typed value and shows the
    /// message under the field; only a success returns the line the card collapses to.
    pub fn saved(&mut self, outcome: Result<(), String>) -> Option<String> {
        let line = self.saving.take();
        match outcome {
            Ok(()) => line,
            Err(problem) => {
                self.error = Some(problem);
                None
            }
        }
    }

    fn remember(&mut self) {
        if self.undo.len() >= MAX_UNDO {
            self.undo.remove(0);
        }
        self.undo.push((self.draft.text.clone(), self.draft.cursor));
        self.redo.clear();
    }

    /// Replaces the selection (or inserts at the caret). Line breaks and NULs never get in.
    pub fn insert(&mut self, text: &str) {
        let clean: String = text
            .chars()
            .filter(|c| !matches!(c, '\n' | '\r' | '\0'))
            .collect();
        if clean.is_empty() && self.draft.selection().is_empty() {
            return;
        }
        self.remember();
        self.draft.insert(&clean);
    }

    pub fn select_all(&mut self) {
        self.draft.anchor = 0;
        self.draft.cursor = self.draft.text.len();
    }

    pub fn undo(&mut self) {
        if let Some((text, cursor)) = self.undo.pop() {
            self.redo.push((self.draft.text.clone(), self.draft.cursor));
            self.draft.text = text;
            self.draft.cursor = cursor.min(self.draft.text.len());
            self.draft.anchor = self.draft.cursor;
        }
    }

    pub fn redo(&mut self) {
        if let Some((text, cursor)) = self.redo.pop() {
            self.undo.push((self.draft.text.clone(), self.draft.cursor));
            self.draft.text = text;
            self.draft.cursor = cursor.min(self.draft.text.len());
            self.draft.anchor = self.draft.cursor;
        }
    }

    fn before(&self, at: usize) -> usize {
        self.draft.text[..at]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn after(&self, at: usize) -> usize {
        self.draft.text[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8())
    }

    /// Moves or deletes for an editing key. True when the key was one of those.
    pub fn edit_key(&mut self, key: &str, cmd: bool, alt: bool, shift: bool) -> bool {
        let d = &self.draft;
        let at = d.cursor;
        let target = match key {
            "backspace" | "left" if cmd => 0,
            "backspace" | "left" if alt => d.word_left(at),
            "backspace" | "left" => self.before(at),
            "delete" | "right" if cmd => d.text.len(),
            "delete" | "right" if alt => d.word_right(at),
            "delete" | "right" => self.after(at),
            "home" | "up" => 0,
            "end" | "down" => d.text.len(),
            _ => return false,
        };
        if matches!(key, "backspace" | "delete") {
            if self.draft.anchor == self.draft.cursor {
                self.draft.anchor = target;
            }
            if self.draft.selection().is_empty() {
                return true;
            }
            self.remember();
            self.draft.insert("");
        } else {
            let selection = self.draft.selection();
            let collapsing = !shift && !selection.is_empty();
            self.draft.cursor = if collapsing && key == "left" {
                selection.start
            } else if collapsing && key == "right" {
                selection.end
            } else {
                target
            };
            if !shift {
                self.draft.anchor = self.draft.cursor;
            }
        }
        true
    }

    /// The value as the host takes it, or `None` while empty.
    pub fn value(&self) -> Option<String> {
        (!self.draft.text.is_empty()).then(|| self.draft.text.clone())
    }
}

/// The field's text as it is drawn: three runs (before the selection, the selection, after it) and
/// whether the caret sits among them, cropped to what fits around the caret.
pub(crate) fn runs(card: &SecretCard) -> (String, String, String) {
    let shown = |text: &str| -> Vec<char> {
        if card.reveal {
            text.chars().collect()
        } else {
            text.chars().map(|_| '•').collect()
        }
    };
    let text = &card.draft.text;
    let selection = card.draft.selection();
    let before = shown(&text[..selection.start]);
    let middle = shown(&text[selection.clone()]);
    let after = shown(&text[selection.end..]);
    let total = before.len() + middle.len() + after.len();
    let caret = if card.draft.cursor == selection.start {
        before.len()
    } else {
        before.len() + middle.len()
    };
    let (lo, hi) = if total <= VISIBLE {
        (0, total)
    } else {
        let lo = caret
            .saturating_sub(VISIBLE - 6)
            .min(total.saturating_sub(VISIBLE));
        (lo, lo + VISIBLE)
    };
    let cut = |from: usize, chars: &[char]| -> String {
        chars
            .iter()
            .enumerate()
            .filter(|(i, _)| (lo..hi).contains(&(from + i)))
            .map(|(_, c)| *c)
            .collect()
    };
    (
        cut(0, &before),
        cut(before.len(), &middle),
        cut(before.len() + middle.len(), &after),
    )
}

/// "N caracteres" or "Vazio".
pub(crate) fn count_text(characters: usize) -> String {
    match characters {
        0 => t("Vazio").to_owned(),
        1 => t("1 caractere").to_owned(),
        n => tf("{} caracteres", &[&n]),
    }
}

fn destination_label(dest: &Dest, project: &str) -> String {
    match dest {
        Dest::Infisical(env) => format!("Infisical · {project} · {env}"),
        Dest::DotEnv => t(".env.local do projeto").to_owned(),
        Dest::Conversation => t("Só nesta conversa").to_owned(),
    }
}

/// The ticks after the person toggles `option`. "Só nesta conversa" is exclusive: ticking it
/// unticks the rest, and ticking anything else unticks it. One option always stays ticked, and
/// the result follows the order of `options`.
pub(crate) fn toggle_dest(current: &[Dest], option: &Dest, options: &[Dest]) -> Vec<Dest> {
    let mut next: Vec<Dest> = if *option == Dest::Conversation {
        vec![Dest::Conversation]
    } else {
        let mut others: Vec<Dest> = current
            .iter()
            .filter(|d| **d != Dest::Conversation)
            .cloned()
            .collect();
        match others.iter().position(|d| d == option) {
            Some(at) => {
                others.remove(at);
            }
            None => others.push(option.clone()),
        }
        others
    };
    if next.is_empty() {
        // The last tick cannot be removed.
        return current.to_vec();
    }
    next.sort_by_key(|d| options.iter().position(|o| o == d).unwrap_or(usize::MAX));
    next
}

/// What is ticked for a request: the person's picks that still exist (no Infisical without a
/// link), or the default: Infisical dev when linked, otherwise only this conversation.
pub(crate) fn effective_dests(chosen: Option<&[Dest]>, linked: bool) -> Vec<Dest> {
    let kept: Vec<Dest> = chosen
        .unwrap_or_default()
        .iter()
        .filter(|d| linked || !matches!(d, Dest::Infisical(_)))
        .cloned()
        .collect();
    if !kept.is_empty() {
        kept
    } else if linked {
        vec![Dest::Infisical("dev".to_owned())]
    } else {
        vec![Dest::Conversation]
    }
}

/// The collapsed "Salvar em" text: "Infisical · appmoradah · dev + prod" for Infisical alone,
/// "dev, prod e .env.local" when `.env.local` joins it.
pub(crate) fn dest_summary(dests: &[Dest], project: &str) -> String {
    let envs: Vec<&str> = dests
        .iter()
        .filter_map(|d| match d {
            Dest::Infisical(env) => Some(env.as_str()),
            _ => None,
        })
        .collect();
    let dotenv = dests.contains(&Dest::DotEnv);
    match (envs.is_empty(), dotenv) {
        (true, false) => destination_label(&Dest::Conversation, ""),
        (true, true) => destination_label(&Dest::DotEnv, ""),
        (false, false) => format!("Infisical · {project} · {}", envs.join(" + ")),
        (false, true) => {
            let head = envs.join(", ");
            tf("{} e {}", &[&head, &".env.local"])
        }
    }
}

/// The targets of an answer, as the host reads them. `Conversation` writes nowhere.
pub(crate) fn targets_json(dests: &[Dest]) -> Vec<Value> {
    dests
        .iter()
        .filter_map(|d| match d {
            Dest::Infisical(env) => Some(json!({"kind": "infisical", "env": env})),
            Dest::DotEnv => Some(json!({"kind": "dotenv"})),
            Dest::Conversation => None,
        })
        .collect()
}

/// One failure line from the host, "Could not save to {target}: {reason}", split in two.
fn split_failure(line: &str) -> Option<(&str, &str)> {
    line.strip_prefix("Could not save to ")?.split_once(": ")
}

/// The host's save error in the person's language, one line per target that failed.
fn save_error_text(message: &str) -> String {
    message
        .lines()
        .map(|line| match split_failure(line) {
            Some((target, reason)) => tf(
                "Não salvou em {}: {}",
                &[&target, &crate::i18n::from_host(reason)],
            ),
            None => crate::i18n::from_host(line),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Shell {
    /// The path of the project a secret request belongs to.
    fn secret_project(&self, request: &Value) -> Option<String> {
        let session = request["sessionId"].as_str()?;
        let conversation = self
            .conversations
            .iter()
            .position(|c| !c.session_id.is_empty() && c.session_id == session)?;
        let project = self.project_of(conversation)?;
        Some(self.projects[project].path.clone())
    }

    /// Keeps the card in step with the host's waiting requests: one card for the first of them.
    pub(super) fn sync_secret_card(&mut self) {
        let Some(request) = self.secrets.first().cloned() else {
            self.secret_card = None;
            return;
        };
        let id = request["id"].as_str().unwrap_or("").to_owned();
        if self.secret_card.as_ref().is_some_and(|c| c.request == id) {
            return;
        }
        self.secret_card = Some(SecretCard::new(id));
        self.secret_done = None;
        if let Some(path) = self.secret_project(&request)
            && crate::settings_infisical::should_ask(&path, true)
        {
            let call = self.backend.invoke(
                "infisical_action",
                crate::settings_infisical::link_args(&path),
            );
            self.waiting.insert(call, Waiting::Infisical);
        }
    }

    /// The project link and its Infisical project's name, for the request on show.
    fn secret_link(&self, request: &Value) -> Option<(String, String, String)> {
        let path = self.secret_project(request)?;
        let (workspace, env) = crate::settings_infisical::link(&path)?;
        let name = crate::settings_infisical::infisical_name(&workspace)
            .unwrap_or_else(|| workspace.chars().take(8).collect());
        Some((path, name, env))
    }

    fn card_dests(&self, request: &Value) -> Vec<Dest> {
        let linked = self.secret_link(request).is_some();
        effective_dests(
            self.secret_card.as_ref().and_then(|c| c.chosen.as_deref()),
            linked,
        )
    }

    /// Typing and pasting go to the card's field while it has the focus.
    pub(super) fn card_typing(&self) -> bool {
        self.secret_card.as_ref().is_some_and(|c| c.focused)
    }

    /// Text from the input method, over `range` (UTF-16) when it names one.
    pub(super) fn card_replace(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        mark: bool,
    ) -> bool {
        let marked = self.marked.take();
        let Some(card) = self.secret_card.as_mut().filter(|c| c.focused) else {
            self.marked = marked;
            return false;
        };
        if let Some(range) = range.or(marked) {
            card.draft.anchor = byte_at(&card.draft.text, range.start);
            card.draft.cursor = byte_at(&card.draft.text, range.end);
        }
        let start = data::utf16_len(&card.draft.text[..card.draft.selection().start]);
        card.insert(text);
        if mark {
            self.marked = Some(start..start + data::utf16_len(text));
        }
        self.caret_moved.set(Instant::now());
        true
    }

    /// The field's text and selection for the input method.
    pub(super) fn card_text(&self) -> Option<(String, Range<usize>, bool)> {
        let card = self.secret_card.as_ref().filter(|c| c.focused)?;
        let text = &card.draft.text;
        let selection = card.draft.selection();
        Some((
            text.clone(),
            data::utf16_len(&text[..selection.start])..data::utf16_len(&text[..selection.end]),
            card.draft.cursor < card.draft.anchor,
        ))
    }

    /// The keys of the card: editing, ⌘A, ⌘V, undo, Enter to save, Esc to cancel. True when used.
    pub(super) fn secret_card_key(
        &mut self,
        keys: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(request) = self.secrets.first().cloned() else {
            return false;
        };
        let Some(card) = self.secret_card.as_mut() else {
            return false;
        };
        let key = keys.key.as_str();
        let (cmd, alt, shift) = (
            keys.modifiers.platform,
            keys.modifiers.alt,
            keys.modifiers.shift,
        );
        if key == "escape" && card.menu.take().is_some() {
            return true;
        }
        if !card.focused {
            // Unfocused, the keys belong to whatever has the typing (the composer): Enter there
            // sends the message and Esc does its own thing, never the card's.
            return false;
        }
        self.caret_moved.set(Instant::now());
        match (cmd, key) {
            (_, "enter") => self.card_save(&request, cx),
            (_, "escape") => {
                let id = card.request.clone();
                self.respond_secret(&id, None, cx);
            }
            (true, "a") => card.select_all(),
            (true, "v") => {
                if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    card.insert(pasted.trim());
                }
            }
            (true, "z") if shift => card.redo(),
            (true, "z") => card.undo(),
            // A masked value is not copied out; the rest of ⌘ keys belong to the app.
            (true, "c" | "x") => {}
            (true, "left" | "right" | "backspace" | "delete" | "up" | "down") => {
                card.edit_key(key, true, false, shift);
            }
            (true, _) => return false,
            _ => {
                if !card.edit_key(key, false, alt, shift) {
                    return false;
                }
            }
        }
        true
    }

    /// Sends the typed value to where it goes; the card collapses to its done line.
    pub(super) fn card_save(&mut self, request: &Value, cx: &mut Context<Self>) {
        let Some(card) = self.secret_card.as_ref() else {
            return;
        };
        let Some(value) = card.value() else {
            return;
        };
        if card.saving.is_some() {
            return;
        }
        let id = card.request.clone();
        let dests = self.card_dests(request);
        let link = self.secret_link(request);
        let mut action = json!({"type": "respond", "requestId": id, "value": value});
        let targets = targets_json(&dests);
        let conversation_only = targets.is_empty();
        if !conversation_only {
            action["targets"] = json!(targets);
        }
        let project = link.map(|(_, name, _)| name).unwrap_or_default();
        let line = if conversation_only {
            t("Chave guardada só nesta conversa. O agente a recebeu sem ver o valor.").to_owned()
        } else {
            tf(
                "Salvo em {}. O agente recebeu a chave sem ver o valor.",
                &[&dest_summary(&dests, &project)],
            )
        };
        let call = self
            .backend
            .invoke("secret_action", json!({ "action": action }));
        if conversation_only {
            // Nothing is written anywhere, so the agent is answered and the card collapses.
            self.waiting.insert(call, Waiting::ScriptAction);
            self.finish_secret(&id, line, cx);
            return;
        }
        // Infisical and .env.local: the host answers the agent only once the save worked, so
        // the card waits for its reply and stays open, value and all, when it fails.
        self.waiting.insert(call, Waiting::SecretSave(id));
        if let Some(card) = self.secret_card.as_mut() {
            card.saving = Some(line);
            card.error = None;
        }
        cx.notify();
    }

    /// The host's reply to a save from the card.
    pub(super) fn secret_save_reply(
        &mut self,
        id: &str,
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        let Some(card) = self.secret_card.as_mut().filter(|c| c.request == id) else {
            return;
        };
        let outcome = if ok {
            Ok(())
        } else {
            Err(save_error_text(
                value["message"]
                    .as_str()
                    .or(value.as_str())
                    .unwrap_or("Infisical refused the request."),
            ))
        };
        if let Some(line) = card.saved(outcome) {
            self.finish_secret(id, line, cx);
        } else {
            cx.notify();
        }
    }

    /// Closes the card and shows `line` in its place for a few seconds.
    fn finish_secret(&mut self, id: &str, line: String, cx: &mut Context<Self>) {
        self.secret_card = None;
        self.secrets.retain(|r| r["id"].as_str() != Some(id));
        self.secret_done = Some((line.clone(), Instant::now()));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f32(DONE_SECS))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .secret_done
                    .as_ref()
                    .is_some_and(|(shown, _)| *shown == line)
                {
                    this.secret_done = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// "Usar do Infisical…": the names of the keys in the linked environment.
    fn card_open_keys(&mut self, request: &Value, cx: &mut Context<Self>) {
        let Some((path, _, env)) = self.secret_link(request) else {
            return;
        };
        let Some(card) = self.secret_card.as_mut() else {
            return;
        };
        if !card.toggle_menu(CardMenu::From, Instant::now()) {
            cx.notify();
            return;
        }
        card.keys = Keys::Loading;
        let call = self.backend.invoke(
            "infisical_action",
            json!({"action": {"type": "listKeys", "projectPath": path, "environment": env}}),
        );
        self.waiting.insert(call, Waiting::SecretCard);
        cx.notify();
    }

    fn card_use_key(&mut self, request: &Value, key: String, cx: &mut Context<Self>) {
        let Some((path, _, env)) = self.secret_link(request) else {
            return;
        };
        let Some(card) = self.secret_card.as_mut() else {
            return;
        };
        card.keys = Keys::Loading;
        let call = self.backend.invoke(
            "infisical_action",
            json!({"action": {"type": "answer", "requestId": card.request, "projectPath": path, "environment": env, "key": key}}),
        );
        self.waiting.insert(call, Waiting::SecretCard);
        self.secret_picked = Some(key);
        cx.notify();
    }

    /// The host's reply to a key list or to a key being handed over.
    pub(super) fn secret_card_reply(&mut self, ok: bool, value: &Value, cx: &mut Context<Self>) {
        if !ok {
            if let Some(card) = self.secret_card.as_mut() {
                card.keys = Keys::Failed(crate::i18n::from_host(
                    value["message"]
                        .as_str()
                        .or(value.as_str())
                        .unwrap_or("Infisical refused the request."),
                ));
            }
            self.secret_picked = None;
            cx.notify();
            return;
        }
        match value["kind"].as_str() {
            Some("keys") => {
                if let Some(card) = self.secret_card.as_mut() {
                    card.keys = Keys::Names(
                        value["keys"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|k| k.as_str().map(str::to_owned))
                            .collect(),
                    );
                }
            }
            Some("answered") => {
                let key = self.secret_picked.take().unwrap_or_default();
                if let Some(id) = self.secret_card.as_ref().map(|c| c.request.clone()) {
                    self.finish_secret(
                        &id,
                        tf(
                            "Usada a chave {} do Infisical. O agente a recebeu sem ver o valor.",
                            &[&key],
                        ),
                        cx,
                    );
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn card_button(
        id: &'static str,
        label: &str,
        hint: Option<&'static str>,
        primary: bool,
        ghost: bool,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(30.))
            .px(px(12.))
            .flex_shrink_0()
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.5))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .text_color(if primary {
                on_accent()
            } else if ghost {
                content(0.55)
            } else {
                content(0.85)
            })
            .when(!ghost, |d| {
                d.border_1()
                    .border_color(if primary {
                        transparent()
                    } else {
                        crate::theme::border()
                    })
                    .bg(if primary { accent() } else { content(0.03) })
            })
            .hover(|s| {
                if primary {
                    s.opacity(0.9)
                } else {
                    s.bg(content(0.08))
                }
            })
            // The main button wears the same pixel weave as the question card's Enviar.
            .when(primary, |d| {
                d.relative()
                    .overflow_hidden()
                    .child(super::pixel_stripes(Hsla {
                        a: 0.18,
                        ..gpui::white()
                    }))
            })
            .child(label.to_owned())
            .children(hint.map(|hint| {
                div()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::NORMAL)
                    .opacity(0.6)
                    .child(hint)
            }))
    }

    fn card_icon_button(id: &'static str, name: &'static str) -> Stateful<Div> {
        div()
            .id(id)
            .size(px(30.))
            .flex_shrink_0()
            .rounded(px(7.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(content(0.08)))
            .child(icon(name, 16.).text_color(content(0.55)))
    }

    /// A menu's surface; a press anywhere outside it closes the menu.
    fn card_menu_surface(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("secret-menu")
            .absolute()
            .w(px(250.))
            .p(px(5.))
            .rounded(px(crate::theme::RADIUS_MD))
            .border_1()
            .border_color(crate::theme::border())
            .bg(crate::theme::opaque_floating())
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if let Some(card) = this.secret_card.as_mut() {
                    card.close_menu_outside(Instant::now());
                }
                cx.notify();
            }))
            .flex()
            .flex_col()
    }

    fn card_menu_row(id: (&'static str, usize), text: String, on: bool) -> Stateful<Div> {
        div()
            .id(id)
            .px(px(8.))
            .py(px(7.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.5))
            .text_color(content(0.9))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.08)))
            .child(div().flex_1().min_w_0().truncate().child(text))
            .when(on, |d| d.child(icon("check", 13.).text_color(content(0.6))))
    }

    /// A tick box in the app's style (the same as the pull request's draft switch).
    fn card_checkbox(on: bool) -> Div {
        div()
            .size(px(15.))
            .flex_shrink_0()
            .rounded(px(3.))
            .border_1()
            .border_color(if on { accent() } else { content(0.3) })
            .flex()
            .items_center()
            .justify_center()
            .when(on, |d| {
                d.bg(accent())
                    .child(svg().path("check").size(px(11.)).text_color(on_accent()))
            })
    }

    /// The card that replaces the old one-line request.
    pub(super) fn secret_card_view(&self, request: &Value, cx: &mut Context<Self>) -> AnyElement {
        let id = request["id"].as_str().unwrap_or("").to_owned();
        let label = request["label"].as_str().unwrap_or("").to_owned();
        let env_name = request["envName"].as_str().map(str::to_owned);
        let about = request["description"].as_str().unwrap_or("").to_owned();
        let Some(card) = self.secret_card.as_ref().filter(|c| c.request == id) else {
            return div().into_any_element();
        };
        let link = self.secret_link(request);
        let dests = self.card_dests(request);
        let project = link.as_ref().map(|l| l.1.clone()).unwrap_or_default();
        let chars = card.draft.text.chars().count();
        let focused = card.focused;
        let reveal = card.reveal;
        let menu = card.menu;

        // The caret blinks with the composer's clock; it rests lit while the window is idle.
        let lit = !focused || caret_lit(self.caret_moved.get().elapsed().as_secs_f32());
        if focused {
            self.caret_drawn.set(lit);
            self.caret_blinks.set(card.draft.selection().is_empty());
        }
        let (before, middle, after) = runs(card);
        let caret = |on: bool| {
            div().w(px(1.5)).h(px(16.)).flex_shrink_0().bg(if on {
                content(0.95)
            } else {
                transparent()
            })
        };
        let text = |s: String| div().flex_shrink_0().whitespace_nowrap().child(s);
        let content_row = if chars == 0 {
            div()
                .flex()
                .items_center()
                .when(focused, |d| d.child(caret(lit)))
                .child(
                    div()
                        .ml(px(if focused { 2. } else { 0. }))
                        .font_family(".SystemUIFont")
                        .text_color(content(0.35))
                        .child(t("Digite ou cole o valor")),
                )
        } else {
            let selecting = !middle.is_empty();
            div()
                .flex()
                .items_center()
                .text_color(content(0.92))
                .child(text(before))
                .when(!selecting, |d| d.child(caret(focused && lit)))
                .when(selecting, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .bg(accent_wash(0.4))
                            .child(middle),
                    )
                })
                .child(text(after))
        };

        let focus_id = id.clone();
        let field = div()
            .id("secret-field")
            .flex()
            .items_center()
            .h(px(40.))
            .pl(px(12.))
            .pr(px(4.))
            .gap(px(2.))
            .relative()
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(crate::theme::black(0.35))
            .child(super::dashed_frame(
                crate::theme::RADIUS_MD,
                if focused {
                    accent_wash(0.8)
                } else {
                    content(0.2)
                },
            ))
            .font_family("Menlo")
            .text_size(px(13.5))
            .cursor_text()
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(card) = this.secret_card.as_mut().filter(|c| c.request == focus_id) {
                    card.focused = true;
                    card.menu = None;
                }
                this.caret_moved.set(Instant::now());
                window.focus(&this.focus, cx);
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(content_row),
            )
            .child(
                Self::card_icon_button("secret-eye", if reveal { "eye-off" } else { "eye" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(card) = this.secret_card.as_mut() {
                            card.reveal = !card.reveal;
                            card.focused = true;
                        }
                        this.caret_moved.set(Instant::now());
                        cx.notify();
                    })),
            )
            .child(
                Self::card_icon_button("secret-paste", "clipboard").on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text())
                            && let Some(card) = this.secret_card.as_mut()
                        {
                            card.insert(pasted.trim());
                            card.focused = true;
                        }
                        this.caret_moved.set(Instant::now());
                        cx.notify();
                    },
                )),
            );
        // The focus ring sits around the field without moving it.
        let field = div()
            .mx(px(-3.))
            .p(px(3.))
            .rounded(px(crate::theme::RADIUS_MD + 3.))
            .bg(if focused {
                accent_wash(0.12)
            } else {
                transparent()
            })
            .child(field);

        // "Salvar em" with its menu.
        let options: Vec<(Dest, String)> = link
            .iter()
            .flat_map(|(_, name, _)| {
                ["dev", "prod"].map(|env| {
                    let dest = Dest::Infisical(env.to_owned());
                    let label = destination_label(&dest, name);
                    (dest, label)
                })
            })
            .chain([
                (Dest::DotEnv, destination_label(&Dest::DotEnv, "")),
                (
                    Dest::Conversation,
                    destination_label(&Dest::Conversation, ""),
                ),
            ])
            .collect();
        let where_label = dest_summary(&dests, &project);
        let where_button = div()
            .id("secret-where")
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(6.))
            .py(px(3.))
            .mr(px(-6.))
            .rounded(px(7.))
            .text_color(content(0.75))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.06)))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(card) = this.secret_card.as_mut() {
                    card.toggle_menu(CardMenu::Where, Instant::now());
                }
                cx.notify();
            }))
            .child(super::question_marker(true, accent()))
            .child(where_label)
            .child(icon("chevron-down", 12.).text_color(content(0.5)));
        let where_menu = (menu == Some(CardMenu::Where)).then(|| {
            let all: Vec<Dest> = options.iter().map(|(option, _)| option.clone()).collect();
            let rows: Vec<AnyElement> = options
                .iter()
                .enumerate()
                .map(|(at, (option, text))| {
                    let ticked = dests.contains(option);
                    let option = option.clone();
                    let (all, request) = (all.clone(), request.clone());
                    Self::card_menu_row(("secret-dest", at), text.clone(), false)
                        .child(Self::card_checkbox(ticked))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // The menu stays open so several can be ticked in a row.
                            let current = this.card_dests(&request);
                            if let Some(card) = this.secret_card.as_mut() {
                                card.chosen = Some(toggle_dest(&current, &option, &all));
                            }
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect();
            gpui::deferred(
                self.card_menu_surface(cx)
                    .top(px(24.))
                    .right(px(-6.))
                    .child(
                        div()
                            .px(px(8.))
                            .pt(px(5.))
                            .pb(px(3.))
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(t("Salvar em")),
                    )
                    .children(rows),
            )
            .with_priority(6)
        });

        // "Usar do Infisical…" with its list of names.
        let from_menu = (menu == Some(CardMenu::From)).then(|| {
            let header = link
                .as_ref()
                .map(|(_, name, env)| format!("{name} · {env}"))
                .unwrap_or_default();
            let body: Vec<AnyElement> = match &card.keys {
                Keys::Idle | Keys::Loading => vec![
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_size(px(12.5))
                        .text_color(content(0.6))
                        .child(t("Buscando as chaves…"))
                        .into_any_element(),
                ],
                Keys::Failed(problem) => vec![
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_size(px(12.5))
                        .text_color(content(0.75))
                        .child(problem.clone())
                        .into_any_element(),
                ],
                Keys::Names(names) if names.is_empty() => vec![
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_size(px(12.5))
                        .text_color(content(0.6))
                        .child(t("Nenhuma chave neste ambiente."))
                        .into_any_element(),
                ],
                Keys::Names(names) => names
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(at, name)| {
                        let (request, picked) = (request.clone(), name.clone());
                        Self::card_menu_row(("secret-key", at), name, false)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.card_use_key(&request, picked.clone(), cx)
                            }))
                            .into_any_element()
                    })
                    .collect(),
            };
            gpui::deferred(
                self.card_menu_surface(cx)
                    .bottom(px(36.))
                    .left(px(0.))
                    .max_h(px(240.))
                    .child(
                        div()
                            .px(px(8.))
                            .pt(px(5.))
                            .pb(px(3.))
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(header),
                    )
                    .child(
                        div()
                            .id("secret-keys")
                            .max_h(px(200.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .children(body),
                    ),
            )
            .with_priority(6)
        });

        let (save_request, from_request) = (request.clone(), request.clone());
        let decline_id = id.clone();
        let ready = chars > 0;
        let bar = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .relative()
            .px(px(12.))
            .py(px(10.))
            .rounded_b(px(crate::theme::RADIUS_LG))
            .child(dotted_rule())
            .children(link.is_some().then(|| {
                div()
                    .relative()
                    .child(
                        Self::card_button(
                            "secret-from",
                            t("Usar do Infisical…"),
                            None,
                            false,
                            false,
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.card_open_keys(&from_request, cx)
                            }),
                        ),
                    )
                    .children(from_menu)
            }))
            .child(div().flex_1())
            .child(
                Self::card_button("secret-decline", t("Cancelar"), Some("esc"), false, true)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.respond_secret(&decline_id, None, cx)
                    })),
            )
            .child(
                Self::card_button("secret-save", t("Salvar"), Some("↩"), true, false)
                    .when(!ready, |d| d.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| this.card_save(&save_request, cx))),
            );

        div()
            .mb(px(8.))
            .relative()
            .overflow_hidden()
            .rounded(px(crate::theme::RADIUS_LG))
            .border_1()
            .border_color(crate::theme::border())
            .bg(field_fill())
            .flex()
            .flex_col()
            // The approved "A+" look, as the question card: a pixel glow at the card's foot.
            .child(super::question_dither(accent(), 0.22, false))
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .px(px(16.))
                    .pt(px(14.))
                    .pb(px(12.))
                    .child(
                        div()
                            .size(px(34.))
                            .flex_shrink_0()
                            .relative()
                            .overflow_hidden()
                            .rounded(px(9.))
                            .border_1()
                            .border_color(amber(0.35))
                            .bg(amber(0.1))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(super::question_dither(amber(1.), 0.3, true))
                            .child(icon("lock", 17.).text_color(amber(1.))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(content(0.95))
                                            .child(label),
                                    )
                                    .children(env_name.map(|name| {
                                        div()
                                            .flex_shrink_0()
                                            .px(px(5.))
                                            .py(px(1.))
                                            .rounded(px(5.))
                                            .bg(amber(0.1))
                                            .font_family("Menlo")
                                            .text_size(px(12.5))
                                            .text_color(Hsla::from(rgb(0xffd9b0)))
                                            .child(name)
                                    })),
                            )
                            .when(!about.is_empty(), |d| {
                                d.child(
                                    div()
                                        .mt(px(3.))
                                        .text_size(px(12.5))
                                        .line_height(px(18.))
                                        .text_color(content(0.55))
                                        .child(about),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .relative()
                    .px(px(16.))
                    .pb(px(14.))
                    .child(field)
                    .children(card.error.clone().map(|problem| {
                        div()
                            .id("secret-error")
                            .mt(px(6.))
                            .mx(px(2.))
                            .text_size(px(12.))
                            .line_height(px(17.))
                            .text_color(Hsla::from(rgb(DANGER)))
                            .child(problem)
                    }))
                    .child(
                        div()
                            .relative()
                            .flex()
                            .justify_between()
                            .items_center()
                            .mt(px(7.))
                            .mx(px(2.))
                            .text_size(px(11.5))
                            .text_color(content(0.42))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(pixel_meter(chars, accent()))
                                    .child(count_text(chars)),
                            )
                            .child(div().relative().child(where_button).children(where_menu)),
                    ),
            )
            .child(bar)
            .into_any_element()
    }

    /// The card's collapsed line once the key has gone: a pixel padlock closes, the line fades in,
    /// a row of pixels counts the seconds down, and the box comes apart in pixels at the end.
    pub(super) fn secret_done_view(&self, line: &str, since: Instant) -> AnyElement {
        let t = since.elapsed().as_secs_f32();
        let u = (t / DONE_SECS).min(1.);
        // The last tenth: the box erodes from the right.
        let k = ((u - 0.9) / 0.1).clamp(0., 1.);
        let tone: Hsla = rgb(0x7be0a2).into();
        let text = ((t - 0.45) / 0.35).clamp(0., 1.) * (1. - k);
        let fill = field_fill();
        let body = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                const CELL: f32 = 4.;
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let (cols, rows) = ((w / CELL).ceil() as u32, (h / CELL).ceil() as u32);
                for y in 0..rows {
                    for x in 0..cols {
                        let across = x as f32 / cols.max(1) as f32;
                        if k > 0.
                            && across > 1. - k * 1.2 + crate::project_wallpaper::bayer8(x, y) * 0.2
                        {
                            continue;
                        }
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(
                                bounds.origin
                                    + gpui::point(px(x as f32 * CELL), px(y as f32 * CELL)),
                                gpui::size(
                                    px(CELL.min(w - x as f32 * CELL)),
                                    px(CELL.min(h - y as f32 * CELL)),
                                ),
                            ),
                            fill,
                        ));
                    }
                }
                if u < 1. {
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .relative()
            .mb(px(8.))
            .h(px(46.))
            .rounded(px(crate::theme::RADIUS_LG))
            .overflow_hidden()
            .child(body)
            .child(super::question_dither(tone, 0.28 * (1. - k), false))
            .child(super::dashed_frame(
                crate::theme::RADIUS_LG,
                Hsla {
                    a: 0.5 * (1. - k),
                    ..tone
                },
            ))
            .child(
                div()
                    .relative()
                    .size_full()
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(pixel_lock(t > 0.38, tone))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.5))
                            .text_color(content(0.9))
                            .opacity(text)
                            .child(line.to_owned()),
                    )
                    .child(pixel_countdown(1. - u, tone, 1. - k)),
            )
            .into_any_element()
    }
}

/// How long the saved-key line stays.
pub(super) const DONE_SECS: f32 = 7.;

/// A padlock of 2 px cells, its shackle up (open) or down (closed).
fn pixel_lock(closed: bool, tone: Hsla) -> impl IntoElement {
    const SHUT: [&str; 10] = [
        "..XXXX..", ".XX..XX.", ".X....X.", ".X....X.", "XXXXXXXX", "XXXXXXXX", "XXX..XXX",
        "XXX..XXX", "XXXXXXXX", "XXXXXXXX",
    ];
    const OPEN: [&str; 10] = [
        "..XXXX..", ".XX..XX.", ".X....X.", ".X......", "XXXXXXXX", "XXXXXXXX", "XXX..XXX",
        "XXX..XXX", "XXXXXXXX", "XXXXXXXX",
    ];
    let map = if closed { SHUT } else { OPEN };
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            for (y, row) in map.iter().enumerate() {
                for (x, c) in row.bytes().enumerate() {
                    if c == b'X' {
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(
                                bounds.origin + gpui::point(px(x as f32 * 2.), px(y as f32 * 2.)),
                                gpui::size(px(1.6), px(1.6)),
                            ),
                            tone,
                        ));
                    }
                }
            }
        },
    )
    .flex_shrink_0()
    .w(px(16.))
    .h(px(20.))
}

/// A row of 14 pixels that go out one by one as `left` (1 to 0) runs down.
fn pixel_countdown(left: f32, tone: Hsla, alpha: f32) -> impl IntoElement {
    const CELLS: u32 = 14;
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let lit = (CELLS as f32 * left).ceil() as u32;
            for i in 0..CELLS {
                let a = if i < lit { 0.9 } else { 0.18 } * alpha;
                window.paint_quad(gpui::fill(
                    gpui::Bounds::new(
                        bounds.origin + gpui::point(px(i as f32 * 5.), px(0.)),
                        gpui::size(px(3.), px(3.)),
                    ),
                    Hsla { a, ..tone },
                ));
            }
        },
    )
    .flex_shrink_0()
    .w(px(CELLS as f32 * 5.))
    .h(px(3.))
}

/// How long the value is, as a short pixel bar: dithered cells fill a share of it (a typical
/// key is about `FULL` characters) and a dotted track shows the rest.
fn pixel_meter(chars: usize, tone: Hsla) -> impl IntoElement {
    const FULL: f32 = 92.;
    let share = (chars as f32 / FULL).min(1.);
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            const CELL: f32 = 2.;
            let w = f32::from(bounds.size.width);
            let cols = (w / CELL) as u32;
            for x in 0..cols {
                let u = x as f32 / cols.max(1) as f32;
                for y in 0..2u32 {
                    let on = if u < share {
                        0.55 + 0.45 * u > crate::project_wallpaper::bayer8(x, y)
                    } else {
                        x % 3 == 0 && y == 0
                    };
                    if on {
                        let color = if u < share { tone } else { content(0.2) };
                        window.paint_quad(gpui::fill(
                            gpui::Bounds::new(
                                bounds.origin
                                    + gpui::point(px(x as f32 * CELL), px(y as f32 * CELL)),
                                gpui::size(px(CELL - 0.4), px(CELL - 0.4)),
                            ),
                            color,
                        ));
                    }
                }
            }
        },
    )
    .w(px(60.))
    .h(px(4.))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> SecretCard {
        let mut card = SecretCard::new("r".into());
        card.insert(text);
        card
    }

    fn all_options() -> Vec<Dest> {
        vec![
            Dest::Infisical("dev".into()),
            Dest::Infisical("prod".into()),
            Dest::DotEnv,
            Dest::Conversation,
        ]
    }

    #[test]
    fn ticking_adds_and_unticking_removes_but_one_stays() {
        let options = all_options();
        let dev = Dest::Infisical("dev".into());
        let prod = Dest::Infisical("prod".into());
        let both = toggle_dest(&[dev.clone()], &prod, &options);
        assert_eq!(both, vec![dev.clone(), prod.clone()]);
        // The order follows the list, not the order of the clicks.
        assert_eq!(
            toggle_dest(&[prod.clone()], &dev, &options),
            vec![dev.clone(), prod.clone()]
        );
        assert_eq!(toggle_dest(&both, &dev, &options), vec![prod.clone()]);
        // The last one cannot be unticked.
        assert_eq!(toggle_dest(&[prod.clone()], &prod, &options), vec![prod]);
    }

    #[test]
    fn the_conversation_option_is_exclusive() {
        let options = all_options();
        let dev = Dest::Infisical("dev".into());
        assert_eq!(
            toggle_dest(&[dev.clone(), Dest::DotEnv], &Dest::Conversation, &options),
            vec![Dest::Conversation]
        );
        // Ticking another one unticks it.
        assert_eq!(
            toggle_dest(&[Dest::Conversation], &Dest::DotEnv, &options),
            vec![Dest::DotEnv]
        );
        // And it cannot be unticked while it is the only one.
        assert_eq!(
            toggle_dest(&[Dest::Conversation], &Dest::Conversation, &options),
            vec![Dest::Conversation]
        );
    }

    #[test]
    fn the_default_follows_the_link_and_drops_what_is_gone() {
        assert_eq!(
            effective_dests(None, true),
            vec![Dest::Infisical("dev".into())]
        );
        assert_eq!(effective_dests(None, false), vec![Dest::Conversation]);
        let picked = [Dest::Infisical("prod".into()), Dest::DotEnv];
        assert_eq!(effective_dests(Some(&picked), true), picked.to_vec());
        assert_eq!(effective_dests(Some(&picked), false), vec![Dest::DotEnv]);
    }

    #[test]
    fn the_summary_lists_the_choices() {
        let dev = Dest::Infisical("dev".into());
        let prod = Dest::Infisical("prod".into());
        assert_eq!(
            dest_summary(&[dev.clone()], "appmoradah"),
            "Infisical · appmoradah · dev"
        );
        assert_eq!(
            dest_summary(&[dev.clone(), prod.clone()], "appmoradah"),
            "Infisical · appmoradah · dev + prod"
        );
        assert_eq!(
            dest_summary(&[dev, prod, Dest::DotEnv], "appmoradah"),
            "dev, prod e .env.local"
        );
        assert_eq!(dest_summary(&[Dest::DotEnv], ""), ".env.local do projeto");
        assert_eq!(dest_summary(&[Dest::Conversation], ""), "Só nesta conversa");
    }

    #[test]
    fn the_answer_lists_its_targets_and_conversation_writes_nowhere() {
        let targets = targets_json(&[Dest::Infisical("prod".into()), Dest::DotEnv]);
        assert_eq!(
            targets,
            vec![
                json!({"kind": "infisical", "env": "prod"}),
                json!({"kind": "dotenv"})
            ]
        );
        assert!(targets_json(&[Dest::Conversation]).is_empty());
    }

    #[test]
    fn a_failure_line_splits_into_target_and_reason() {
        assert_eq!(
            split_failure("Could not save to Infisical (prod): Infisical refused: no."),
            Some(("Infisical (prod)", "Infisical refused: no."))
        );
        assert_eq!(split_failure("Something else"), None);
    }

    #[test]
    fn a_failed_save_keeps_the_card_the_value_and_the_error() {
        let mut card = typed("sk_live_1");
        card.saving = Some("Salvo no Infisical.".into());
        assert_eq!(
            card.saved(Err("O Infisical recusou o pedido.".into())),
            None
        );
        assert_eq!(card.draft.text, "sk_live_1");
        assert_eq!(card.error.as_deref(), Some("O Infisical recusou o pedido."));
        assert!(card.saving.is_none());
        // Retrying clears the error, and only a success collapses the card.
        card.error = None;
        card.saving = Some("Salvo no Infisical.".into());
        assert_eq!(card.saved(Ok(())).as_deref(), Some("Salvo no Infisical."));
        assert!(card.error.is_none());
    }

    #[test]
    fn a_press_outside_closes_the_menu_and_its_button_does_not_reopen_it() {
        let mut card = typed("x");
        let now = Instant::now();
        assert!(card.toggle_menu(CardMenu::Where, now));
        card.close_menu_outside(now);
        assert!(card.menu.is_none());
        // The click of the same press, on the same button: stays closed.
        assert!(!card.toggle_menu(CardMenu::Where, now));
        assert!(card.menu.is_none());
        // Another menu's button still opens at once, and a later click opens this one again.
        assert!(card.toggle_menu(CardMenu::From, now));
        card.menu = None;
        assert!(card.toggle_menu(CardMenu::Where, now + REOPEN_GUARD));
        // The button closes its own open menu.
        assert!(!card.toggle_menu(CardMenu::Where, now + REOPEN_GUARD));
        assert!(card.menu.is_none());
    }

    #[test]
    fn typing_inserts_at_the_caret_and_select_all_replaces() {
        let mut card = typed("sk_test");
        card.edit_key("left", false, false, false);
        card.insert("X");
        assert_eq!(card.draft.text, "sk_tesXt");
        card.select_all();
        card.insert("new");
        assert_eq!(card.draft.text, "new");
    }

    #[test]
    fn backspace_delete_and_undo_work_on_the_value() {
        let mut card = typed("abc");
        card.edit_key("backspace", false, false, false);
        assert_eq!(card.draft.text, "ab");
        card.edit_key("home", false, false, false);
        card.edit_key("delete", false, false, false);
        assert_eq!(card.draft.text, "b");
        card.undo();
        assert_eq!(card.draft.text, "ab");
        card.undo();
        assert_eq!(card.draft.text, "abc");
        card.redo();
        assert_eq!(card.draft.text, "ab");
    }

    #[test]
    fn line_breaks_never_enter_and_the_value_is_hidden_until_revealed() {
        let mut card = typed("a\nb\r\0c");
        assert_eq!(card.draft.text, "abc");
        let (before, _, after) = runs(&card);
        assert_eq!(format!("{before}{after}"), "•••");
        card.reveal = true;
        let (before, _, after) = runs(&card);
        assert_eq!(format!("{before}{after}"), "abc");
    }

    #[test]
    fn a_long_value_is_cropped_around_the_caret() {
        let card = typed(&"k".repeat(120));
        let (before, middle, after) = runs(&card);
        assert!(before.chars().count() + middle.chars().count() + after.chars().count() <= VISIBLE);
    }

    #[test]
    fn the_count_reads_in_portuguese() {
        assert_eq!(count_text(0), "Vazio");
        assert_eq!(count_text(1), "1 caractere");
    }
}
