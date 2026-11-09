//! The message queue, as Sirus's: while the agent works, a message sent from the composer
//! waits in a card above it and starts, in order, once the reply is done. A message can also
//! steer the running reply ("Instruir agora"), and one sent while an approval waits declines the
//! approval first and steers. Ports `ComposerPromptQueue.tsx`, `lib/prompt-queue.ts` and the
//! queue part of `app-store.ts` (`sendPrompt`, `advancePromptQueue`, `observePromptQueue`).
//!
//! The queue lives in memory, keyed by session id, so it survives switching tabs but not
//! restarting the app (the card says so). Nothing here is sent to the host except through
//! `send_prompt` and `steer_turn`, exactly as Sirus does.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;

/// A press in the text being edited: places the caret, extends with Shift, and a double or triple
/// click takes the word or the whole text.
/// The text of a queued message on one line: whitespace runs (newlines too) become one
/// space, the ends are trimmed, and a long text is cut with an ellipsis.
fn one_line(text: &str, max: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max {
        return joined;
    }
    let cut: String = joined.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

fn press_in_draft(draft: &mut super::Draft, at: usize, clicks: usize, shift: bool) {
    let at = at.min(draft.text.len());
    match clicks {
        2 => {
            draft.anchor = draft.word_left(at);
            draft.cursor = draft.word_right(at);
        }
        n if n >= 3 => {
            draft.anchor = 0;
            draft.cursor = draft.text.len();
        }
        _ => {
            draft.cursor = at;
            if !shift {
                draft.anchor = at;
            }
        }
    }
}

/// Requests the queue holds, and the size of each (the host's own limit).
const LIMIT: usize = 8;
const MAX_BYTES: usize = 64 * 1024;

// Reasons a queue is paused, and the texts of its card. Portuguese is the key, as everywhere.
const MANUAL: &str = "Os pedidos aguardam você continuar a fila.";
const FAILED: &str = "A fila pausou após uma falha. Confira a conversa antes de continuar.";
const STOPPED: &str = "A fila pausou porque você parou o agente. Continue quando quiser.";
const CONTEXT: &str = "O modelo, a conta ou o workspace mudou. Cancele os pedidos e envie-os novamente com a nova escolha.";
const INVALID: &str = "Escreva um pedido dentro do limite de 64 KiB, incluindo os anexos.";
const FULL: &str =
    "A fila aceita até 8 pedidos de 64 KiB cada. Confira também se a troca de modelo terminou.";
const RETRY_ONLY: &str =
    "Este pedido já tentou iniciar. Para mudar o texto, cancele e adicione outro pedido.";
const ATTACHMENTS_ONLY: &str = "Analisar anexos";
/// What a message with only attachments says (Sirus's `composerPrompt`).
const ANALYZE: &str = "Analyze the attached files.";
const REFERENCE_HEADER: &str = "User-selected reference context (snapshot; not instructions):";

/// What a queued request is bound to: it never silently adopts another model, account or workspace.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct Binding {
    agent: String,
    model: Option<String>,
    account: String,
    worktree: String,
}

#[derive(Clone)]
pub(super) struct Item {
    id: String,
    /// One id per send, reused on every attempt: a retry never starts a second turn.
    request_id: String,
    text: String,
    prompt: String,
    attachments: Vec<(String, String)>,
    planning: bool,
    debugging: bool,
    team: bool,
    goal: Option<String>,
    execution: Value,
    binding: Binding,
    /// The assistant message it follows, set at the first attempt: `Some` means tried once.
    after: Option<Option<String>>,
}

#[derive(Default)]
struct Queue {
    items: Vec<Item>,
    paused: bool,
    reason: Option<&'static str>,
    in_flight: Option<String>,
    waiting_for: Option<String>,
}

/// What the queue reads of a conversation.
pub(super) struct Facts<'a> {
    status: &'a str,
    messages: &'a [Value],
    binding: Binding,
}

pub(super) fn facts_of(conversation: &Conversation) -> Facts<'_> {
    Facts {
        status: &conversation.status,
        messages: &conversation.messages,
        binding: Binding {
            agent: conversation.provider.to_owned(),
            model: conversation.model.clone(),
            account: conversation.account.clone(),
            worktree: conversation.worktree.clone(),
        },
    }
}

fn active(status: &str) -> bool {
    matches!(status, "starting" | "running" | "waiting")
}

fn last_assistant(messages: &[Value]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| m["role"] == "agent")
        .and_then(|m| m["id"].as_str().map(str::to_owned))
}

/// The request was admitted by the host even though its reply never arrived.
fn observed_admission(facts: &Facts, predecessor: &Option<String>, prompt: &str) -> bool {
    if last_assistant(facts.messages) == *predecessor {
        return false;
    }
    let from = match predecessor {
        None => 0,
        Some(id) => match facts
            .messages
            .iter()
            .position(|m| m["id"].as_str() == Some(id))
        {
            Some(at) => at + 1,
            None => return false,
        },
    };
    let mut following = facts.messages[from..]
        .iter()
        .filter(|m| m["role"] != "system");
    let first = following.next();
    first.is_some_and(|m| m["role"] == "user" && m["content"].as_str() == Some(prompt.trim()))
        && following.next().is_some_and(|m| m["role"] == "agent")
}

/// The `steer_turn` arguments: attachments are those of the conversation's session.
fn steer_args(session_id: &str, text: &str, attachments: &[(String, String)]) -> Value {
    let mut args = json!({ "sessionId": session_id, "text": text });
    if !attachments.is_empty() {
        args["attachmentIds"] = json!(attachments.iter().map(|(id, _)| id).collect::<Vec<_>>());
        args["attachmentOwner"] = json!(format!("session:{session_id}"));
    }
    args
}

fn valid(prompt: &str) -> bool {
    !prompt.trim().is_empty() && prompt.len() <= MAX_BYTES
}

/// Codex and Claude take instructions in a running reply (ADR-062).
fn can_steer(provider: &str, status: &str) -> bool {
    matches!(provider, "claude" | "codex") && active(status)
}

/// A running `/compact` takes no instructions: a message sent meanwhile waits in the queue.
fn compacting(messages: &[Value]) -> bool {
    messages
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .is_some_and(|m| {
            m["content"]
                .as_str()
                .is_some_and(|c| c.trim() == "/compact")
        })
}

/// Whether the conversation takes an instruction this moment. During a compaction a message simply
/// stays in the queue and goes out behind it.
fn takes_instruction(provider: &str, status: &str, messages: &[Value]) -> bool {
    can_steer(provider, status) && !compacting(messages)
}

/// A sent prompt as the person wrote it, without the reference block the agent received.
fn request_of(content: &str) -> &str {
    let marker = format!("\n\n{REFERENCE_HEADER}\n");
    content.find(&marker).map_or(content, |at| &content[..at])
}

/// A request ready to start, claimed by the queue until its reply settles.
pub(super) struct Dispatch {
    session_id: String,
    item_id: String,
    request: Value,
}

impl Queue {
    /// The next request, when nothing holds the queue back. Claims it before the call is made.
    fn advance(&mut self, session_id: &str, facts: &Facts) -> Option<Dispatch> {
        let item = self.items.first()?;
        if self.paused
            || self.in_flight.is_some()
            || self.waiting_for.is_some()
            || active(facts.status)
        {
            return None;
        }
        if item.binding != facts.binding {
            self.paused = true;
            self.reason = Some(CONTEXT);
            return None;
        }
        let predecessor = item
            .after
            .clone()
            .unwrap_or_else(|| last_assistant(facts.messages));
        if last_assistant(facts.messages) != predecessor {
            self.paused = true;
            self.reason = Some(CONTEXT);
            return None;
        }
        let item = &mut self.items[0];
        item.after = Some(predecessor.clone());
        self.in_flight = Some(item.id.clone());
        let mut request = json!({
            "sessionId": session_id,
            "prompt": item.prompt,
            "execution": item.execution,
            "clientRequestId": item.request_id,
            "debugging": item.debugging && !item.planning,
            "goal": item.goal,
            "attachmentIds": item.attachments.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            "attachmentOwner": format!("session:{session_id}"),
            "queuedAfter": {
                "agent": item.binding.agent,
                "model": item.binding.model,
                "providerAccountId": item.binding.account,
                "worktreePath": item.binding.worktree,
                "messageId": predecessor,
            },
        });
        if item.team {
            request["team"] = json!(true);
        }
        Some(Dispatch {
            session_id: session_id.to_owned(),
            item_id: item.id.clone(),
            request: json!({ "request": request }),
        })
    }
}

#[derive(Default, Clone)]
pub(super) struct Recall {
    index: Option<usize>,
    draft: String,
}

/// One ↑ (-1) or ↓ (+1) through `sent` (oldest first). Browsing starts only from an empty
/// composer; ↓ past the newest restores the draft that was there.
fn recall_step(
    sent: &[String],
    state: &Recall,
    value: &str,
    direction: i32,
) -> Option<(Recall, String)> {
    if sent.is_empty() {
        return None;
    }
    let Some(index) = state.index else {
        if direction > 0 || !value.trim().is_empty() {
            return None;
        }
        let last = sent.len() - 1;
        return Some((
            Recall {
                index: Some(last),
                draft: value.to_owned(),
            },
            sent[last].clone(),
        ));
    };
    let next = index as i32 + direction;
    if next < 0 {
        return Some((state.clone(), sent[index].clone()));
    }
    if next as usize >= sent.len() {
        return Some((Recall::default(), state.draft.clone()));
    }
    Some((
        Recall {
            index: Some(next as usize),
            draft: state.draft.clone(),
        },
        sent[next as usize].clone(),
    ))
}

/// What a send is, taken from the composer before anything is decided.
#[derive(Clone)]
pub(super) struct Outgoing {
    ix: usize,
    session_id: String,
    /// The draft exactly as typed, to know it was left alone.
    draft: String,
    text: String,
    prompt: String,
    attachments: Vec<(String, String)>,
    execution: Value,
    goal: Option<String>,
    planning: bool,
    debugging: bool,
    team: bool,
}

/// Approvals being declined before a redirect, then the message that follows.
pub(super) struct Redirect {
    pending: usize,
    failed: bool,
    invert: bool,
    out: Outgoing,
}

/// A steer waiting for the host: from the composer (its draft is cleared once accepted) or
/// from a queued request (which then leaves the queue).
pub(super) struct SteerWait {
    session_id: String,
    item: Option<String>,
    draft: Option<(usize, String, Vec<(String, String)>)>,
}

#[derive(Default)]
pub(super) struct Queues {
    map: HashMap<String, Queue>,
    /// A composer send that is waiting for the host (declining approvals, steering).
    pub(super) busy: bool,
    /// The queued request being steered now.
    steering: Option<String>,
    /// The request being edited: its id and text.
    editing: Option<(String, super::Draft)>,
    edit_error: bool,
    recall: HashMap<usize, Recall>,
    redirects: HashMap<u64, Redirect>,
    next_redirect: u64,
}

impl Queues {
    pub(super) fn has_items(&self, session_id: &str) -> bool {
        self.map
            .get(session_id)
            .is_some_and(|q| !q.items.is_empty())
    }

    /// Adds a request behind the others. The error is the text to show.
    fn enqueue(&mut self, session_id: &str, item: Item, facts: &Facts) -> Result<(), &'static str> {
        let queue = self.map.get(session_id);
        if !valid(&item.prompt) || queue.map_or(0, |q| q.items.len()) >= LIMIT {
            return Err(FULL);
        }
        let intended = active(facts.status) || queue.is_some_and(|q| !q.items.is_empty());
        let stopped = matches!(facts.status, "failed" | "stopped");
        let queue = self.map.entry(session_id.to_owned()).or_insert_with(|| {
            let paused = intended && stopped;
            Queue {
                paused,
                reason: paused.then_some(FAILED),
                waiting_for: active(facts.status)
                    .then(|| last_assistant(facts.messages))
                    .flatten(),
                ..Queue::default()
            }
        });
        queue.paused |= intended && stopped;
        queue.items.push(item);
        Ok(())
    }

    /// Sends the next request once the reply it waited for is done (`observePromptQueue`).
    fn observe(&mut self, session_id: &str, facts: &Facts) -> Option<Dispatch> {
        let queue = self.map.get_mut(session_id)?;
        let message_id = last_assistant(facts.messages);
        // A send whose reply was lost may have been admitted all the same: never replay it.
        if queue.in_flight.is_none() {
            if let Some(head) = queue.items.first() {
                if head.binding == facts.binding
                    && head
                        .after
                        .as_ref()
                        .is_some_and(|after| observed_admission(facts, after, &head.prompt))
                {
                    queue.items.remove(0);
                    queue.waiting_for = message_id.clone();
                }
            }
        }
        if active(facts.status) {
            if message_id.is_some() && message_id != queue.waiting_for {
                queue.waiting_for = message_id;
            }
            return None;
        }
        let streaming = facts
            .messages
            .iter()
            .find(|m| m["id"].as_str() == message_id.as_deref())
            .is_some_and(|m| m["streaming"].as_bool().unwrap_or(false));
        // Nothing to wait for means the reply arrived before anyone saw it: it is the one to follow.
        if (queue.waiting_for.is_some() && message_id != queue.waiting_for) || streaming {
            return None;
        }
        // A stop or a failure pauses the queue: the next request never starts by itself after
        // the person stopped the agent or the turn went wrong.
        let failed = facts.status != "completed";
        queue.waiting_for = None;
        if failed {
            queue.paused = true;
            queue.reason = Some(if facts.status == "stopped" {
                STOPPED
            } else {
                FAILED
            });
        }
        queue.advance(session_id, facts)
    }

    /// The reply of a queued `send_prompt`.
    fn settle(
        &mut self,
        session_id: &str,
        item_id: &str,
        succeeded: bool,
        response: &Value,
        facts: &Facts,
    ) -> Option<Dispatch> {
        let queue = self.map.get_mut(session_id)?;
        if queue.in_flight.as_deref() != Some(item_id) {
            return None;
        }
        let item = queue.items.iter().find(|i| i.id == item_id)?;
        let predecessor = item.after.clone().flatten();
        let live_last = last_assistant(facts.messages);
        let answered = succeeded && live_last == predecessor;
        let response_messages: &[Value] =
            response["messages"].as_array().map_or(&[], Vec::as_slice);
        // Startup or persistence may fail after admission: an admitted turn is never replayed.
        let admitted =
            succeeded || observed_admission(facts, &item.after.clone().flatten(), &item.prompt);
        let waiting_for = if active(facts.status) {
            live_last.clone()
        } else if answered {
            last_assistant(response_messages)
        } else {
            None
        };
        let status = if answered {
            response["status"].as_str().unwrap_or(facts.status)
        } else {
            facts.status
        };
        let failed = !succeeded || matches!(status, "failed" | "stopped");
        if admitted {
            queue.items.retain(|i| i.id != item_id);
        }
        queue.in_flight = None;
        queue.waiting_for = waiting_for.clone();
        queue.paused |= failed;
        if failed {
            queue.reason = Some(FAILED);
        }
        // A very fast turn may have settled while the call was still returning.
        if waiting_for.is_none() && facts.status == "completed" {
            return queue.advance(session_id, facts);
        }
        None
    }

    fn pause(&mut self, session_id: &str, facts: &Facts) {
        let queue = self
            .map
            .entry(session_id.to_owned())
            .or_insert_with(|| Queue {
                waiting_for: last_assistant(facts.messages),
                ..Queue::default()
            });
        queue.paused = true;
        queue.reason = Some(MANUAL);
    }

    fn resume(&mut self, session_id: &str, facts: &Facts) -> Option<Dispatch> {
        let queue = self.map.get_mut(session_id)?;
        if queue.in_flight.is_some() || queue.items.is_empty() {
            return None;
        }
        if queue.items.iter().any(|i| i.binding != facts.binding) {
            queue.paused = true;
            queue.reason = Some(CONTEXT);
            return None;
        }
        let last = last_assistant(facts.messages);
        queue.paused = false;
        queue.reason = None;
        queue.waiting_for = if active(facts.status) {
            last
        } else if queue.waiting_for != last {
            queue.waiting_for.take()
        } else {
            None
        };
        queue.advance(session_id, facts)
    }

    /// Replaces the text of a request that has not started. False when it can't be.
    fn edit(&mut self, session_id: &str, id: &str, text: &str) -> bool {
        let Some(item) = self
            .map
            .get_mut(session_id)
            .filter(|q| q.in_flight.as_deref() != Some(id))
            .and_then(|q| q.items.iter_mut().find(|i| i.id == id))
            .filter(|i| i.after.is_none())
        else {
            return false;
        };
        let prompt = if text.trim().is_empty() && !item.attachments.is_empty() {
            ANALYZE
        } else {
            text.trim()
        };
        if !valid(prompt) {
            return false;
        }
        item.prompt = prompt.to_owned();
        item.text = text.to_owned();
        true
    }

    /// Removes a request; its attachments come back so unused ones can be released.
    fn cancel(&mut self, session_id: &str, id: &str) -> Vec<(String, String)> {
        let Some(queue) = self
            .map
            .get_mut(session_id)
            .filter(|q| q.in_flight.as_deref() != Some(id))
        else {
            return Vec::new();
        };
        let at = queue.items.iter().position(|i| i.id == id);
        at.map(|at| queue.items.remove(at).attachments)
            .unwrap_or_default()
    }

    /// One place earlier or later; a request that is sending or was tried stays at the head.
    fn shift(&mut self, session_id: &str, id: &str, delta: i32) -> bool {
        let Some(queue) = self.map.get_mut(session_id) else {
            return false;
        };
        match Self::target(queue, id, delta) {
            Some((from, to)) => {
                queue.items.swap(from, to);
                true
            }
            None => false,
        }
    }

    fn target(queue: &Queue, id: &str, delta: i32) -> Option<(usize, usize)> {
        let from = queue.items.iter().position(|i| i.id == id)?;
        let to = from as i32 + delta;
        if to < 0 || to as usize >= queue.items.len() {
            return None;
        }
        let fixed =
            |i: &Item| queue.in_flight.as_deref() == Some(i.id.as_str()) || i.after.is_some();
        (!fixed(&queue.items[from]) && !fixed(&queue.items[to as usize]))
            .then_some((from, to as usize))
    }

    /// An emptied queue that nothing holds leaves no trace.
    fn tidy(&mut self, session_id: &str) {
        if self
            .map
            .get(session_id)
            .is_some_and(|q| q.items.is_empty() && q.in_flight.is_none())
        {
            self.map.remove(session_id);
        }
    }
}

impl Shell {
    /// Enter, the send button, ⌘Enter (`invert`: the other of queue and steer, for this message).
    pub(super) fn send_with(&mut self, invert: bool, cx: &mut Context<Self>) {
        let ix = self.active_conversation();
        if self.queues.busy {
            return;
        }
        // A typed `/compact` is the command, never text for the model (context_meter.rs).
        let typed = self
            .drafts
            .get(&ix)
            .map_or(String::new(), |d| d.full_text());
        let files = self.attachments.get(&ix).is_some_and(|a| !a.is_empty());
        if self.typed_compact(ix, &typed, files, cx) {
            return;
        }
        if self.typed_git(ix, &typed, files, cx) {
            return;
        }
        let conversation = &self.conversations[ix];
        if conversation.session_id.is_empty() {
            return;
        }
        let session_id = conversation.session_id.clone();
        let chosen = self.execution(conversation.provider, conversation.model.as_deref());
        let plain_cursor = conversation.provider == "cursor"
            && conversation
                .model
                .as_deref()
                .and_then(|id| self.models.get("cursor")?.iter().find(|m| m.id == id))
                .is_some_and(|m| !m.parameterized);
        let execution = json!({ "approval": conversation.approval, "effort": if plain_cursor { None } else { chosen.effort }, "fast": chosen.fast && !plain_cursor, "planning": conversation.planning });
        let draft = self
            .drafts
            .get(&ix)
            .map_or(String::new(), |d| d.full_text());
        let text = draft.trim().to_owned();
        let attachments = self.attachments.get(&ix).cloned().unwrap_or_default();
        // Attachments alone are enough, as in Sirus.
        if text.is_empty() && attachments.is_empty() {
            return;
        }
        let out = Outgoing {
            ix,
            session_id,
            prompt: if text.is_empty() {
                ANALYZE.to_owned()
            } else {
                text.clone()
            },
            text,
            draft,
            attachments,
            execution,
            goal: conversation.goal.clone(),
            planning: conversation.planning,
            debugging: conversation.debugging,
            team: conversation.team,
        };
        // A message sent while an approval waits redirects the agent: the approvals are declined
        // first, since a reply still waiting on one would never read the message.
        let approvals: Vec<Value> = conversation
            .pending
            .iter()
            .filter(|r| r["kind"]["type"] != "userInput")
            .cloned()
            .collect();
        if approvals.is_empty() || out.text.is_empty() {
            self.dispatch(out, false, invert, cx);
            return;
        }
        self.queues.busy = true;
        self.queues.next_redirect += 1;
        let key = self.queues.next_redirect;
        for request in &approvals {
            let args = json!({ "request": { "sessionId": out.session_id, "generation": request["generation"], "requestId": request["requestId"], "turnId": request["turnId"], "response": { "type": "approval", "decision": "decline" } } });
            let call = self.backend.invoke("respond_agent_request", args);
            self.waiting.insert(call, Waiting::Declined(key));
        }
        self.queues.redirects.insert(
            key,
            Redirect {
                pending: approvals.len(),
                failed: false,
                invert,
                out,
            },
        );
        cx.notify();
    }

    pub(super) fn declined_reply(
        &mut self,
        key: u64,
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        if !ok {
            self.problem = Some(error_text(value));
        }
        let Some(redirect) = self.queues.redirects.get_mut(&key) else {
            return;
        };
        redirect.pending = redirect.pending.saturating_sub(1);
        redirect.failed |= !ok;
        if redirect.pending > 0 {
            return;
        }
        let redirect = self.queues.redirects.remove(&key).unwrap();
        self.queues.busy = false;
        if !redirect.failed {
            self.dispatch(redirect.out, true, redirect.invert, cx);
        }
        cx.notify();
    }

    /// Steers, queues or sends, as Sirus's `send` decides after any redirect.
    pub(super) fn dispatch(
        &mut self,
        out: Outgoing,
        redirect: bool,
        invert: bool,
        cx: &mut Context<Self>,
    ) {
        // Compact and send: a heavy conversation idle past its prompt cache compacts first and
        // the message follows (context_meter.rs).
        if !self.queues.has_items(&out.session_id) && self.compacts_first(out.ix) {
            let ix = out.ix;
            self.compact_conversation(ix, Some(out), cx);
            return;
        }
        let conversation = &self.conversations[out.ix];
        let steer_by_default = self.settings["steerWhileRunning"]
            .as_bool()
            .unwrap_or(false);
        let steer = (redirect || steer_by_default != invert)
            && takes_instruction(
                conversation.provider,
                &conversation.status,
                &conversation.messages,
            )
            && !out.team
            && !out.prompt.is_empty();
        if steer {
            self.queues.busy = true;
            // Attachments alone steer with the same line a first message gets.
            let attachments = out.attachments.clone();
            self.steer(
                &out.session_id,
                &out.prompt,
                &attachments,
                SteerWait {
                    session_id: out.session_id.clone(),
                    item: None,
                    draft: Some((out.ix, out.draft.clone(), attachments.clone())),
                },
            );
            cx.notify();
            return;
        }
        if conversation.working() || self.queues.has_items(&out.session_id) {
            self.enqueue(out, cx);
        } else {
            self.send_now(out, cx);
        }
    }

    fn steer(
        &mut self,
        session_id: &str,
        text: &str,
        attachments: &[(String, String)],
        wait: SteerWait,
    ) {
        let call = self
            .backend
            .invoke("steer_turn", steer_args(session_id, text, attachments));
        self.waiting.insert(call, Waiting::Steered(wait));
    }

    pub(super) fn steered_reply(
        &mut self,
        wait: SteerWait,
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        self.queues.busy = false;
        self.queues.steering = None;
        if !ok {
            self.problem = Some(error_text(value));
        } else {
            // The instruction glides to the top of the view, as a sent message does.
            if let Some(ix) = self
                .conversations
                .iter()
                .position(|conversation| conversation.session_id == wait.session_id)
            {
                self.pin_to_top(ix, cx);
            }
            if let Some(item) = &wait.item {
                let files = self.queues.cancel(&wait.session_id, item);
                self.release_unused(files, &wait.session_id);
                self.queues.tidy(&wait.session_id);
            }
            // The draft goes only if it was left as it was sent.
            if let Some((ix, draft, files)) = wait.draft {
                if self.drafts.get(&ix).is_some_and(|d| d.full_text() == draft)
                    && self.attachments.get(&ix).cloned().unwrap_or_default() == files
                {
                    self.drafts.remove(&ix);
                    self.touch_drafts();
                    // The host took the attachments (they count as sent): nothing to release.
                    self.attachments.remove(&ix);
                    self.marked = None;
                    self.queues.recall.remove(&ix);
                }
            }
        }
        cx.notify();
    }

    fn enqueue(&mut self, out: Outgoing, cx: &mut Context<Self>) {
        let ix = out.ix;
        let conversation = &self.conversations[ix];
        let facts = facts_of(conversation);
        let item = Item {
            id: uuid(),
            request_id: uuid(),
            text: out.text.clone(),
            prompt: out.prompt.clone(),
            attachments: out.attachments.clone(),
            planning: out.planning,
            debugging: out.debugging,
            team: out.team,
            goal: out.goal.clone(),
            execution: out.execution.clone(),
            binding: facts.binding.clone(),
            after: None,
        };
        if let Err(reason) = self.queues.enqueue(&out.session_id, item, &facts) {
            self.problem = Some(t(reason).to_owned());
            return;
        }
        self.problem = None;
        self.sent(ix);
        let facts = facts_of(&self.conversations[ix]);
        let next = self.queues.advance_head(&out.session_id, &facts);
        self.run_queue(next);
        cx.notify();
    }

    /// The message shows at once; the host's window replaces it when it arrives.
    fn show_sent(&mut self, ix: usize, prompt: &str, attachments: &[(String, String)]) {
        let conversation = &mut self.conversations[ix];
        if !conversation.loaded {
            return;
        }
        let files: Vec<Value> = attachments
            .iter()
            .map(|(id, name)| json!({ "id": id, "name": name }))
            .collect();
        conversation.messages.push(json!({ "id": uuid(), "role": "user", "content": prompt, "attachments": files, "streaming": false }));
        conversation.rebuild();
    }

    fn send_now(&mut self, out: Outgoing, cx: &mut Context<Self>) {
        let ix = out.ix;
        self.show_sent(ix, &out.prompt, &out.attachments);
        self.queues.tidy(&out.session_id);
        self.sent(ix);
        let attachment_ids: Vec<&String> = out.attachments.iter().map(|(id, _)| id).collect();
        // One id per send, so a repeated delivery can't start a second turn.
        let id = self.backend.invoke("send_prompt", json!({ "request": { "sessionId": out.session_id, "prompt": out.prompt, "clientRequestId": uuid(), "execution": out.execution, "attachmentIds": attachment_ids, "attachmentOwner": format!("session:{}", out.session_id), "goal": out.goal, "debugging": out.debugging, "team": out.team } }));
        self.waiting.insert(id, Waiting::Session);
        // The sent message glides up to the top of the view (glide.rs).
        self.pin_to_top(ix, cx);
        cx.notify();
    }

    /// Brings the conversation's newest turn to the top of the view, with its room below: what a
    /// sent message, an instruction and a compaction divider all do (glide.rs).
    pub(super) fn pin_to_top(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.list_state(ix, cx).pause_following_tail();
        self.glides.entry(ix).or_default().follow(true);
    }

    /// The composer after a message left it: empty, with a team request spent (it is one-shot).
    fn sent(&mut self, ix: usize) {
        self.drafts.remove(&ix);
        // The empty draft goes to the host now, so its key is removed without waiting for the timer.
        self.save_drafts();
        self.marked = None;
        self.attachments.remove(&ix);
        self.queues.recall.remove(&ix);
        self.conversations[ix].team = false;
    }

    /// Starts the request the queue let go of.
    pub(super) fn run_queue(&mut self, next: Option<Dispatch>) {
        if let Some(d) = next {
            if let Some(ix) = self
                .conversations
                .iter()
                .position(|c| c.session_id == d.session_id)
            {
                let prompt = d.request["request"]["prompt"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                self.show_sent(ix, &prompt, &[]);
            }
            let call = self.backend.invoke("send_prompt", d.request);
            self.waiting
                .insert(call, Waiting::Queued(d.session_id, d.item_id));
        }
    }

    pub(super) fn queued_reply(
        &mut self,
        session_id: &str,
        item_id: &str,
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        if ok {
            self.session_updated(value, cx);
        } else {
            self.problem = Some(error_text(value));
        }
        let Some(ix) = self
            .conversations
            .iter()
            .position(|c| c.session_id == session_id)
        else {
            return;
        };
        let facts = facts_of(&self.conversations[ix]);
        let next = self.queues.settle(session_id, item_id, ok, value, &facts);
        self.run_queue(next);
        cx.notify();
    }

    /// Called with every session snapshot: lets the queue start its next request.
    pub(super) fn observe_queue(&mut self, ix: usize) {
        let facts = facts_of(&self.conversations[ix]);
        let id = self.conversations[ix].session_id.clone();
        let next = self.queues.observe(&id, &facts);
        self.run_queue(next);
    }

    /// Releases attachments no queued request or draft still holds.
    fn release_unused(&mut self, files: Vec<(String, String)>, session_id: &str) {
        let owner = format!("session:{session_id}");
        let ids: Vec<String> = files
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| {
                !self
                    .attachments
                    .values()
                    .flatten()
                    .any(|(held, _)| held == id)
                    && !self
                        .queues
                        .map
                        .values()
                        .flat_map(|q| &q.items)
                        .flat_map(|i| &i.attachments)
                        .any(|(held, _)| held == id)
            })
            .collect();
        if !ids.is_empty() {
            self.backend.invoke(
                "release_prompt_attachments",
                json!({ "owner": owner, "ids": ids }),
            );
        }
    }

    /// The Stop button, Escape and ⌘.: stops the agent and pauses the queue behind it.
    pub(super) fn stop_agent(&mut self, cx: &mut Context<Self>) {
        let ix = self.active_conversation();
        let session_id = self.conversations[ix].session_id.clone();
        if session_id.is_empty() {
            return;
        }
        let facts = facts_of(&self.conversations[ix]);
        self.queues.pause(&session_id, &facts);
        self.backend
            .invoke("stop_agent", json!({ "sessionId": session_id }));
        cx.notify();
    }

    fn toggle_queue(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let Some(ix) = self
            .conversations
            .iter()
            .position(|c| c.session_id == session_id)
        else {
            return;
        };
        let facts = facts_of(&self.conversations[ix]);
        let paused = self.queues.map.get(session_id).is_some_and(|q| q.paused);
        if paused {
            let next = self.queues.resume(session_id, &facts);
            self.run_queue(next);
        } else {
            self.queues.pause(session_id, &facts);
        }
        cx.notify();
    }

    /// Keys that belong to the composer while the agent works. True when one was taken.
    pub(super) fn queue_keys(
        &mut self,
        keys: &gpui::Keystroke,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = keys.key.as_str();
        let m = &keys.modifiers;
        let ix = self.active_conversation();
        // The queued request being edited takes its own keys.
        if self.edit_target().is_some() {
            return self.edit_keys(key, m.shift, cx);
        }
        let plain = !m.platform && !m.shift && !m.alt && !m.control;
        let typing_elsewhere = self.query_target().is_some();
        if key == "." && m.platform && !m.shift && !m.alt && !m.control {
            self.stop_agent(cx);
            cx.stop_propagation();
            return true;
        }
        if (m.platform || m.control) && !m.shift && !m.alt && key == "enter" && !typing_elsewhere {
            self.send_with(true, cx);
            cx.stop_propagation();
            return true;
        }
        let covered = self.add_menu.is_some()
            || self.composer_menu.is_some()
            || self.context_menu.is_some()
            || self.confirm.is_some()
            || self.project_menu.is_some()
            || self.usage_popup.is_some()
            || self.usage_accounts.is_some()
            || self.palette_ui.is_some()
            || self.theme_menu.is_some()
            || self.menu.is_some()
            || self.session_filter.open.is_some();
        // Escape twice stops the agent, unless a menu, dialog or pane holds the keys. Clicking the
        // transcript can leave nothing focused, which still counts as the conversation.
        if key == "escape"
            && plain
            && !covered
            && !typing_elsewhere
            && self.conversations[ix].working()
            && (self.focus.is_focused(window) || window.focused(cx).is_none())
        {
            if self.pressed_twice(Press::StopAgent, cx) {
                self.stop_agent(cx);
            }
            cx.stop_propagation();
            return true;
        }
        if matches!(key, "up" | "down")
            && plain
            && !covered
            && !typing_elsewhere
            && self.recall_prompt(ix, key == "up")
        {
            cx.stop_propagation();
            cx.notify();
            return true;
        }
        false
    }

    /// ↑/↓ on an empty composer walk this conversation's sent prompts.
    fn recall_prompt(&mut self, ix: usize, up: bool) -> bool {
        let draft = self
            .drafts
            .get(&ix)
            .map_or(String::new(), |d| d.full_text());
        let state = self.queues.recall.get(&ix).cloned().unwrap_or_default();
        if state.index.is_none() && (!up || !draft.is_empty()) {
            return false;
        }
        let mut sent: Vec<String> = Vec::new();
        for message in &self.conversations[ix].messages {
            if message["role"] != "user" {
                continue;
            }
            let request = request_of(message["content"].as_str().unwrap_or("")).trim();
            if !request.is_empty() && sent.last().map(String::as_str) != Some(request) {
                sent.push(request.to_owned());
            }
        }
        let Some((next, value)) = recall_step(&sent, &state, &draft, if up { -1 } else { 1 })
        else {
            return false;
        };
        self.queues.recall.insert(ix, next);
        self.touch_drafts();
        let entry = self.drafts.entry(ix).or_default();
        entry.text = value;
        entry.cursor = entry.text.len();
        entry.anchor = entry.cursor;
        true
    }

    /// The text being edited in the queue card, while the card is the one on screen.
    pub(super) fn edit_target(&mut self) -> Option<&mut super::Draft> {
        let session = self.conversations[self.active_conversation()]
            .session_id
            .clone();
        let (id, _) = self.queues.editing.as_ref()?;
        let shown = self
            .queues
            .map
            .get(&session)
            .is_some_and(|q| q.items.iter().any(|i| i.id == *id));
        if !shown {
            return None;
        }
        self.queues.editing.as_mut().map(|(_, text)| text)
    }

    fn edit_keys(&mut self, key: &str, shift: bool, cx: &mut Context<Self>) -> bool {
        let session = self.conversations[self.active_conversation()]
            .session_id
            .clone();
        match key {
            "enter" if shift => {
                if let Some(draft) = self.edit_target() {
                    draft.insert("\n");
                }
            }
            "enter" => self.save_edit(&session),
            "escape" => self.queues.editing = None,
            // Everything else, ⌘A and the arrows included, is the composer's keys on this text.
            _ => {
                self.queues.edit_error = false;
                return false;
            }
        }
        cx.stop_propagation();
        cx.notify();
        true
    }

    fn save_edit(&mut self, session_id: &str) {
        let Some((id, draft)) = self.queues.editing.clone() else {
            return;
        };
        if self.queues.edit(session_id, &id, &draft.text) {
            self.queues.editing = None;
            self.queues.edit_error = false;
        } else {
            self.queues.edit_error = true;
        }
    }

    /// The card above the composer, with the waiting requests and what can be done to each.
    pub(super) fn queue_card(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let conversation = &self.conversations[ix];
        let session_id = conversation.session_id.clone();
        let queue = self
            .queues
            .map
            .get(&session_id)
            .filter(|q| !q.items.is_empty())?;
        let count = queue.items.len();
        let steerable = takes_instruction(
            conversation.provider,
            &conversation.status,
            &conversation.messages,
        );
        let editing = self.queues.editing.clone();
        let edit_error = self.queues.edit_error;
        let steering = self.queues.steering.is_some();
        let (paused, in_flight) = (queue.paused, queue.in_flight.clone());

        let button = |id: gpui::ElementId, enabled: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px(px(8.))
                .rounded(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_color(transparent())
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(content(0.62))
                .when(enabled, |d| {
                    d.cursor_pointer().hover(|s| {
                        s.bg(content(0.07))
                            .border_color(crate::theme::border())
                            .text_color(content(0.95))
                    })
                })
                .when(!enabled, |d| d.opacity(0.4))
        };

        // Only a paused queue says so, with the way out: the rows speak for themselves.
        let header = paused.then(|| {
            let enabled = in_flight.is_none() && editing.is_none();
            let session_id = session_id.clone();
            div()
                .h(px(28.))
                .flex()
                .items_center()
                .gap(px(8.))
                .border_b_1()
                .border_color(content(0.05))
                .child(icon("queue-pause", 14.).text_color(content(0.5)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(t(queue.reason.unwrap_or("Pausada"))),
                )
                .child(
                    button("queue-toggle".into(), enabled)
                        .h(px(24.))
                        .child(icon("queue-play", 14.).text_color(content(0.62)))
                        .child(t("Continuar fila"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.toggle_queue(&session_id, cx);
                            }
                        })),
                )
        });
        let rows = queue.items.iter().enumerate().map(|(i, item)| {
            let is_editing = editing.as_ref().is_some_and(|(id, _)| *id == item.id);
            let pictures = self.viewer.queue_tiles(&item.attachments, i);
            let body = if is_editing {
                let draft = editing
                    .as_ref()
                    .map(|(_, draft)| draft.clone())
                    .unwrap_or_default();
                let styled = gpui::StyledText::new(if draft.text.is_empty() {
                    " ".to_owned()
                } else {
                    draft.text.clone()
                });
                let layout = styled.layout().clone();
                let (paint, press, drag) = (layout.clone(), layout.clone(), layout);
                let (selection, caret) = (draft.selection(), draft.cursor);
                // The composer's caret and blink, as every field's (`field_caret`).
                let lit = selection.is_empty() && self.field_caret_lit();
                let overlay = gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        if !selection.is_empty() {
                            super::paint_selection(&paint, selection.clone(), window);
                            return;
                        }
                        if !lit {
                            return;
                        }
                        let at = paint.position_for_index(caret).unwrap_or(bounds.origin);
                        window.paint_quad(
                            gpui::fill(
                                Bounds::new(
                                    at,
                                    gpui::size(px(crate::theme::CARET_WIDTH), paint.line_height()),
                                ),
                                crate::theme::caret_color(),
                            )
                            .corner_radii(px(1.)),
                        );
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full();
                let (save_session, close_session) = (session_id.clone(), session_id.clone());
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .min_h(px(80.))
                            .w_full()
                            .p(px(8.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .border_1()
                            .border_color(crate::theme::border())
                            .bg(content(0.04))
                            .text_size(px(13.))
                            .line_height(px(20.8))
                            .text_color(content(0.95))
                            .cursor_text()
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                window.focus(&this.focus, cx);
                                                let at = press
                                                    .index_for_position(event.position)
                                                    .unwrap_or_else(|nearest| nearest);
                                                if let Some(draft) = this.edit_target() {
                                                    press_in_draft(
                                                        draft,
                                                        at,
                                                        event.click_count,
                                                        event.modifiers.shift,
                                                    );
                                                }
                                                cx.stop_propagation();
                                                cx.notify();
                                            },
                                        ),
                                    )
                                    .on_mouse_move(cx.listener(
                                        move |this, event: &MouseMoveEvent, _, cx| {
                                            if !event.dragging() {
                                                return;
                                            }
                                            let at = drag
                                                .index_for_position(event.position)
                                                .unwrap_or_else(|nearest| nearest);
                                            if let Some(draft) = this.edit_target() {
                                                draft.cursor = at;
                                            }
                                            cx.notify();
                                        },
                                    ))
                                    .child(styled)
                                    .child(overlay),
                            ),
                    )
                    .when(edit_error, |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(0xe0654d))
                                .child(t(INVALID)),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(
                                button(("queue-save", i).into(), true)
                                    .child(t("Salvar"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.save_edit(&save_session);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                button(("queue-discard", i).into(), true)
                                    .child(t("Fechar edição"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let _ = &close_session;
                                        this.queues.editing = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .into_any_element()
            } else {
                let full = item.text.clone();
                let shown = if full.trim().is_empty() {
                    t(ATTACHMENTS_ONLY).to_owned()
                } else {
                    one_line(&full, 200)
                };
                let mut line = div()
                    .id(("queue-text", i))
                    .truncate()
                    .whitespace_nowrap()
                    .text_color(content(0.75))
                    .child(shown);
                if !full.trim().is_empty() {
                    let tip: String = full.trim().chars().take(600).collect();
                    line = line.tooltip(message_actions::text_tip(tip));
                }
                line.into_any_element()
            };

            let actions = (!is_editing).then(|| {
                let mut row = div().flex().flex_shrink_0().gap(px(4.));
                if count > 1 {
                    for (delta, glyph, tag) in [
                        (-1, "chevron-up", "queue-up"),
                        (1, "chevron-down", "queue-down"),
                    ] {
                        let enabled = Queues::target(queue, &item.id, delta).is_some();
                        let (id, session_id) = (item.id.clone(), session_id.clone());
                        row = row.child(
                            button((tag, i).into(), enabled)
                                .size(px(24.))
                                .px(px(0.))
                                .child(icon(glyph, 12.).text_color(content(0.62)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if enabled {
                                        this.queues.shift(&session_id, &id, delta);
                                        cx.notify();
                                    }
                                })),
                        );
                    }
                }
                // Not for a team plan, which starts a turn of its own.
                if steerable
                    && !item.team
                    && !item.prompt.trim().is_empty()
                    && in_flight.as_deref() != Some(item.id.as_str())
                    && item.after.is_none()
                {
                    let enabled = !steering;
                    let (id, session_id, text, files) = (
                        item.id.clone(),
                        session_id.clone(),
                        item.prompt.trim().to_owned(),
                        item.attachments.clone(),
                    );
                    row = row.child(
                        button(("queue-steer", i).into(), enabled)
                            .size(px(24.))
                            .px(px(0.))
                            .tooltip(message_actions::name_tip(t("Instruir agora")))
                            .child(icon("queue-steer", 14.).text_color(content(0.62)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if enabled {
                                    this.queues.steering = Some(id.clone());
                                    this.steer(
                                        &session_id,
                                        &text,
                                        &files,
                                        SteerWait {
                                            session_id: session_id.clone(),
                                            item: Some(id.clone()),
                                            draft: None,
                                        },
                                    );
                                    cx.notify();
                                }
                            })),
                    );
                }
                {
                    let enabled =
                        in_flight.as_deref() != Some(item.id.as_str()) && item.after.is_none();
                    let (id, session_id, text) =
                        (item.id.clone(), session_id.clone(), item.text.clone());
                    row = row.child(
                        button(("queue-edit", i).into(), enabled)
                            .size(px(24.))
                            .px(px(0.))
                            .tooltip(message_actions::name_tip(t("Editar")))
                            .child(icon("pencil", 14.).text_color(content(0.62)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if enabled {
                                    let Some(at) = this
                                        .conversations
                                        .iter()
                                        .position(|c| c.session_id == session_id)
                                    else {
                                        return;
                                    };
                                    let facts = facts_of(&this.conversations[at]);
                                    this.queues.pause(&session_id, &facts);
                                    this.queues.editing =
                                        Some((id.clone(), super::Draft::with_text(text.clone())));
                                    this.queues.edit_error = false;
                                    cx.notify();
                                }
                            })),
                    );
                }
                {
                    let enabled = in_flight.as_deref() != Some(item.id.as_str());
                    let (id, session_id) = (item.id.clone(), session_id.clone());
                    row = row.child(
                        button(("queue-cancel", i).into(), enabled)
                            .size(px(24.))
                            .px(px(0.))
                            .tooltip(message_actions::name_tip(t("Cancelar")))
                            .child(icon("close", 14.).text_color(content(0.62)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if enabled {
                                    let files = this.queues.cancel(&session_id, &id);
                                    this.release_unused(files, &session_id);
                                    this.queues.tidy(&session_id);
                                    cx.notify();
                                }
                            })),
                    );
                }
                // With nothing saying the queue runs, its pause sits on the first row.
                if i == 0 && !paused {
                    let enabled = in_flight.is_none() && editing.is_none();
                    let session_id = session_id.clone();
                    row = row.child(
                        button("queue-toggle".into(), enabled)
                            .size(px(24.))
                            .px(px(0.))
                            .tooltip(message_actions::name_tip(t("Pausar fila")))
                            .child(icon("queue-pause", 14.).text_color(content(0.62)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if enabled {
                                    this.toggle_queue(&session_id, cx);
                                }
                            })),
                    );
                }
                row
            });

            div()
                .id(("queue-item", i))
                .when(is_editing, |d| d.min_h(px(32.)))
                .when(!is_editing, |d| d.h(px(42.)).overflow_hidden())
                .when(i > 0, |d| d.border_t_1().border_color(content(0.05)))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(icon("queue-list", 14.).text_color(content(0.35)))
                .when(!pictures.is_empty(), |d| {
                    d.child(div().flex().flex_shrink_0().gap(px(4.)).children(pictures))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(is_editing, |d| d.py(px(4.)))
                        .flex()
                        .flex_col()
                        .child(body)
                        .when(
                            item.after.is_some() && in_flight.as_deref() != Some(item.id.as_str()),
                            |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(content(0.45))
                                        .child(t(RETRY_ONLY)),
                                )
                            },
                        ),
                )
                .children(actions)
        });

        // A ledge on the composer's top edge: inset, open at the bottom.
        Some(
            div()
                .mx(px(8.))
                .relative()
                .rounded_t(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_b_0()
                .border_color(crate::theme::border())
                .px(px(10.))
                .py(px(2.))
                .text_size(px(12.))
                .text_color(content(0.55))
                .child(
                    gpui::canvas(
                        |_, _, _| {},
                        |bounds, _, window, _| {
                            let corners = gpui::Corners {
                                top_left: px(10.),
                                top_right: px(10.),
                                bottom_left: px(0.),
                                bottom_right: px(0.),
                            };
                            window.paint_quad(
                                gpui::fill(bounds, content(0.03)).corner_radii(corners),
                            );
                        },
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                )
                .children(header)
                .child(
                    div()
                        .id("queue-list")
                        .max_h(px(224.))
                        .overflow_y_scroll()
                        .children(rows),
                )
                .into_any_element(),
        )
    }
}

impl Queues {
    /// Lets the head of the queue start if nothing holds it back (after adding to it).
    fn advance_head(&mut self, session_id: &str, facts: &Facts) -> Option<Dispatch> {
        self.map.get_mut(session_id)?.advance(session_id, facts)
    }

    /// The draft being edited; the caller has checked there is one.
    pub(super) fn edit_draft_mut(&mut self) -> &mut super::Draft {
        &mut self.editing.as_mut().expect("being edited").1
    }

    pub(super) fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// Whether the composer shows a queue for this session (a waiting request).
    pub(super) fn waiting(&self, session_id: &str) -> bool {
        self.has_items(session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_collapses_whitespace_and_truncates() {
        assert_eq!(one_line("a = b\n2 ok\r\n3 = c", 100), "a = b 2 ok 3 = c");
        assert_eq!(one_line("a\n\n\n\nb", 100), "a b");
        assert_eq!(one_line("  \n  oi  \t mundo \n ", 100), "oi mundo");
        assert_eq!(one_line("   \n ", 100), "");
        assert_eq!(one_line("abcdefghij", 5), "abcde…");
        assert_eq!(one_line("abcd efgh", 5), "abcd…");
    }

    #[test]
    fn a_press_places_the_caret_and_clicks_take_more() {
        let mut draft = super::super::Draft::with_text("ola mundo".into());
        assert_eq!((draft.cursor, draft.anchor), (9, 9));
        press_in_draft(&mut draft, 2, 1, false);
        assert_eq!((draft.cursor, draft.anchor), (2, 2));
        press_in_draft(&mut draft, 5, 1, true);
        assert_eq!(draft.selection(), 2..5);
        press_in_draft(&mut draft, 5, 3, false);
        assert_eq!(draft.selection(), 0..9);
        press_in_draft(&mut draft, 5, 2, false);
        assert_eq!(&draft.text[draft.selection()], "mundo");
    }

    fn message(id: &str, role: &str, content: &str) -> Value {
        json!({ "id": id, "role": role, "content": content })
    }

    fn binding() -> Binding {
        Binding {
            agent: "claude".into(),
            model: Some("haiku".into()),
            account: "default".into(),
            worktree: "/w".into(),
        }
    }

    fn item(id: &str, prompt: &str) -> Item {
        Item {
            id: id.into(),
            request_id: format!("req-{id}"),
            text: prompt.into(),
            prompt: prompt.into(),
            attachments: Vec::new(),
            planning: false,
            debugging: false,
            team: false,
            goal: None,
            execution: json!({ "approval": "full" }),
            binding: binding(),
            after: None,
        }
    }

    fn facts<'a>(status: &'a str, messages: &'a [Value]) -> Facts<'a> {
        Facts {
            status,
            messages,
            binding: binding(),
        }
    }

    #[test]
    fn queued_while_working_starts_after_the_reply_completes() {
        let mut queues = Queues::default();
        let working = [message("u1", "user", "hi"), message("a1", "agent", "…")];
        queues
            .enqueue("s", item("one", "next"), &facts("running", &working))
            .unwrap();
        assert!(
            queues
                .advance_head("s", &facts("running", &working))
                .is_none(),
            "never while active"
        );
        assert!(queues.observe("s", &facts("running", &working)).is_none());
        let done = facts("completed", &working);
        let dispatch = queues
            .observe("s", &done)
            .expect("starts when the reply is done");
        assert_eq!(dispatch.request["request"]["prompt"], "next");
        assert_eq!(
            dispatch.request["request"]["queuedAfter"]["messageId"],
            "a1"
        );
        assert_eq!(dispatch.request["request"]["attachmentOwner"], "session:s");
        // Claimed: a repeated snapshot can't send it twice.
        assert!(queues.observe("s", &done).is_none());
    }

    #[test]
    fn stop_or_failure_pauses_instead_of_sending() {
        let mut queues = Queues::default();
        let working = [message("a1", "agent", "…")];
        queues
            .enqueue("s", item("one", "next"), &facts("running", &working))
            .unwrap();
        assert!(queues.observe("s", &facts("stopped", &working)).is_none());
        let queue = &queues.map["s"];
        assert!(queue.paused);
        assert_eq!(queue.reason, Some(STOPPED));
    }

    #[test]
    fn a_settled_send_leaves_the_queue_and_waits_for_the_next_reply() {
        let mut queues = Queues::default();
        let working = [message("a1", "agent", "…")];
        queues
            .enqueue("s", item("one", "first"), &facts("running", &working))
            .unwrap();
        queues
            .enqueue("s", item("two", "second"), &facts("running", &working))
            .unwrap();
        queues.observe("s", &facts("completed", &working)).unwrap();
        let after = [
            message("a1", "agent", "…"),
            message("u2", "user", "first"),
            message("a2", "agent", ""),
        ];
        let response = json!({ "status": "running", "messages": after });
        let next = queues.settle("s", "one", true, &response, &facts("running", &after));
        assert!(next.is_none());
        assert_eq!(queues.map["s"].items.len(), 1);
        assert_eq!(queues.map["s"].waiting_for.as_deref(), Some("a2"));
        let finished = [
            message("a1", "agent", "…"),
            message("u2", "user", "first"),
            message("a2", "agent", "done"),
        ];
        assert!(
            queues
                .observe("s", &facts("completed", &finished))
                .is_some(),
            "the second starts after the first"
        );
    }

    #[test]
    fn a_changed_model_pauses_the_queue() {
        let mut queues = Queues::default();
        let working = [message("a1", "agent", "…")];
        queues
            .enqueue("s", item("one", "next"), &facts("running", &working))
            .unwrap();
        let mut other = facts("completed", &working);
        other.binding.model = Some("opus".into());
        assert!(queues.observe("s", &other).is_none());
        assert_eq!(queues.map["s"].reason, Some(CONTEXT));
    }

    #[test]
    fn limit_and_empty_prompts_are_refused() {
        let mut queues = Queues::default();
        let working = [message("a1", "agent", "…")];
        for n in 0..LIMIT {
            queues
                .enqueue("s", item(&n.to_string(), "x"), &facts("running", &working))
                .unwrap();
        }
        assert!(
            queues
                .enqueue("s", item("over", "x"), &facts("running", &working))
                .is_err()
        );
        assert!(
            Queues::default()
                .enqueue("t", item("blank", "  "), &facts("running", &working))
                .is_err()
        );
    }

    #[test]
    fn items_move_but_never_past_one_that_was_tried() {
        let mut queues = Queues::default();
        let working = [message("a1", "agent", "…")];
        for id in ["a", "b", "c"] {
            queues
                .enqueue("s", item(id, id), &facts("running", &working))
                .unwrap();
        }
        assert!(queues.shift("s", "c", -1));
        let order: Vec<_> = queues.map["s"]
            .items
            .iter()
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(order, ["a", "c", "b"]);
        queues.map.get_mut("s").unwrap().items[0].after = Some(None);
        assert!(
            !queues.shift("s", "c", -1),
            "the head that was tried stays first"
        );
        assert!(!queues.edit("s", "a", "changed"), "and can't be edited");
        assert!(queues.edit("s", "b", "changed"));
    }

    #[test]
    fn attachments_are_kept_for_the_send_and_release() {
        let mut queues = Queues::default();
        let working = [message("a1", "agent", "…")];
        let mut with_file = item("f", ANALYZE);
        with_file.text = String::new();
        with_file.attachments = vec![("att-1".into(), "shot.png".into())];
        queues
            .enqueue("s", with_file, &facts("running", &working))
            .unwrap();
        let dispatch = queues.observe("s", &facts("completed", &working)).unwrap();
        assert_eq!(
            dispatch.request["request"]["attachmentIds"],
            json!(["att-1"])
        );
        assert_eq!(dispatch.request["request"]["prompt"], ANALYZE);
    }

    #[test]
    fn recall_walks_back_and_restores_the_draft() {
        let sent = vec!["one".to_owned(), "two".to_owned()];
        let start = Recall::default();
        assert!(
            recall_step(&sent, &start, "typing", -1).is_none(),
            "only from an empty composer"
        );
        let (state, value) = recall_step(&sent, &start, "", -1).unwrap();
        assert_eq!(value, "two");
        let (state, value) = recall_step(&sent, &state, &value, -1).unwrap();
        assert_eq!(value, "one");
        let (state, value) = recall_step(&sent, &state, &value, -1).unwrap();
        assert_eq!(value, "one", "stays on the oldest");
        let (state, _) = recall_step(&sent, &state, &value, 1).unwrap();
        let (state, value) = recall_step(&sent, &state, "two", 1).unwrap();
        assert_eq!(value, "");
        assert!(state.index.is_none());
    }

    #[test]
    fn a_steer_carries_its_attachments_of_the_session() {
        let plain = steer_args("s", "go", &[]);
        assert!(plain.get("attachmentIds").is_none());
        let args = steer_args("s", "go", &[("a1".into(), "shot.png".into())]);
        assert_eq!(args["attachmentIds"], json!(["a1"]));
        assert_eq!(args["attachmentOwner"], "session:s");
    }

    #[test]
    fn a_compaction_takes_no_instruction() {
        let compacting = [json!({ "role": "user", "content": "/compact" })];
        let replying = [json!({ "role": "user", "content": "fix it" })];
        assert!(!takes_instruction("claude", "running", &compacting));
        assert!(takes_instruction("claude", "running", &replying));
    }

    #[test]
    fn steering_is_for_claude_and_codex_while_active() {
        assert!(can_steer("claude", "running"));
        assert!(can_steer("codex", "waiting"));
        assert!(!can_steer("cursor", "running"));
        assert!(!can_steer("claude", "completed"));
        assert!(compacting(&[message("u", "user", " /compact ")]));
        assert!(!compacting(&[message("u", "user", "hi")]));
    }

    #[test]
    fn reference_context_is_not_part_of_a_recalled_prompt() {
        let content = format!("fix it\n\n{REFERENCE_HEADER}\nSelected file: \"a.rs\"");
        assert_eq!(request_of(&content), "fix it");
        assert_eq!(request_of("plain"), "plain");
    }
}
