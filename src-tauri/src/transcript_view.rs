//! The renderer's view of sessions without full transcripts (ADR-048):
//! metadata-only state, turn-windowed session events and the closed
//! read-only `transcript_action` (load one transcript, search candidates).

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use parking_lot::Mutex;

use crate::runtime::{AppHandle, Emitter, Manager};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::models::{AppData, Message, MessageRole, Session};

/// Search candidates are bounded in count and bytes; the renderer applies its
/// exact matching and its own 200-hit limit to them.
const MAX_SEARCH_QUERY: usize = 256;
const MAX_CANDIDATES: usize = 400;
const MAX_CANDIDATE_BYTES: usize = 8 * 1024 * 1024;
/// Most recently active sessions a search reads; older ones are
/// skipped and the result says it is truncated.
const MAX_SEARCHED_SESSIONS: usize = 400;

/// User/assistant messages: what views treat as a started conversation. Known
/// without reading an unloaded transcript (ADR-099).
fn conversation_length(session: &Session) -> usize {
    session.messages.conversation_len()
}

/// One session without its transcript. `messages` stays present and empty so the
/// shared Session shape holds; `transcriptLength` says whether it has content and
/// `lastReplyPreview` what it said last (ADR-139).
pub fn session_meta(session: &Session) -> Result<Value> {
    let mut value = crate::persist::without_transcripts(|| serde_json::to_value(session))?;
    value["messages"] = json!([]);
    value["transcriptLength"] = json!(conversation_length(session));
    if let Some(preview) = session.messages.reply_preview() {
        value[crate::persist::LAST_REPLY_PREVIEW] = json!(preview);
    }
    Ok(value)
}

/// A session as a command reply: metadata plus its messages from the last agent reply on.
/// Callers read the new turn's state and that reply; a whole transcript would make every
/// send from a paired phone carry megabytes it never uses.
pub fn session_tail(session: &Session) -> Result<Value> {
    let from = session
        .messages
        .iter()
        .rposition(|message| message.role == crate::models::MessageRole::Agent)
        .unwrap_or(session.messages.len());
    let mut value = session_meta(session)?;
    value["messages"] = serde_json::to_value(&session.messages[from..])?;
    Ok(value)
}

/// `load_state` payload: every session as metadata only.
pub fn state_meta(data: &AppData) -> Result<Value> {
    let mut value = crate::persist::without_transcripts(|| serde_json::to_value(data))?;
    if let Some(entries) = value.get_mut("sessions").and_then(Value::as_array_mut) {
        for (entry, session) in entries.iter_mut().zip(&data.sessions) {
            entry["messages"] = json!([]);
            entry["transcriptLength"] = json!(conversation_length(session));
            if let Some(preview) = session.messages.reply_preview() {
                entry[crate::persist::LAST_REPLY_PREVIEW] = json!(preview);
            }
        }
    }
    // Review guides load one at a time through `review_guide_action` (ADR-125).
    if let Some(fields) = value.as_object_mut() {
        fields.remove("reviewGuides");
        // Tasks load through `list_tasks`.
        fields.remove("scheduledPrompts");
    }
    Ok(value)
}

/// Index of the current turn: the last user message, or the whole transcript.
pub fn current_turn(session: &Session) -> usize {
    session
        .messages
        .iter()
        .rposition(|message| message.role == MessageRole::User)
        .unwrap_or(0)
}

/// A session event carries only `messages[from..]` plus the window, so a long
/// transcript is not re-sent for every activity change. A renderer that holds
/// fewer than `from` messages reloads the transcript instead of splicing.
pub fn session_event(session: &Session, from: usize) -> Result<Value> {
    let mut value = session_meta(session)?;
    let Some(messages) = session.messages.loaded() else {
        // An unloaded (or unreadable) transcript did not change: the window is empty
        // at its end, so a renderer keeps what it holds, or reloads when it holds less.
        let total = session.messages.counts().messages;
        value["transcriptWindow"] = json!({ "from": total, "total": total });
        return Ok(value);
    };
    let from = from.min(messages.len());
    value["messages"] = serde_json::to_value(&messages[from..])?;
    if let Some(sent) = value["messages"].as_array_mut() {
        sent.iter_mut()
            .for_each(crate::turn_review::omit_diffs_value);
    }
    value["transcriptWindow"] = json!({ "from": from, "total": messages.len() });
    Ok(value)
}

/// Per session, a fingerprint of the current turn's user message as last published.
static SENT_TURNS: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(Default::default);

/// Everything a renderer shows of a user message. Older embedded thumbnails could make
/// it megabytes, so it is hashed field by field instead of serialized.
fn fingerprint(message: &Message) -> u64 {
    let mut hasher = DefaultHasher::new();
    if let Ok(bytes) = serde_json::to_vec(&(
        &message.id,
        &message.role,
        &message.content,
        &message.created_at,
        message.streaming,
        &message.activity,
        &message.steers,
        &message.launched,
    )) {
        bytes.hash(&mut hasher);
    }
    for attachment in &message.attachments {
        attachment.id.hash(&mut hasher);
        attachment.name.hash(&mut hasher);
        attachment.kind.hash(&mut hasher);
        attachment.mime_type.hash(&mut hasher);
        attachment.size.hash(&mut hasher);
        attachment.thumbnail.hash(&mut hasher);
        attachment.has_thumbnail.hash(&mut hasher);
    }
    message.attachments.len().hash(&mut hasher);
    hasher.finish()
}

/// Where a live event's window starts. The turn's user message (with its image
/// thumbnails) is sent once; while it stays unchanged, later events of the same
/// turn start at the reply after it. A renderer that missed that first event holds
/// fewer than `from` messages and reloads the transcript instead of splicing.
fn live_from(sent: &mut HashMap<String, u64>, session: &Session, from: usize) -> usize {
    let turn = current_turn(session);
    let Some(user) = session
        .messages
        .get(turn)
        .filter(|message| message.role == MessageRole::User)
    else {
        return from;
    };
    if from > turn {
        return from;
    }
    let print = fingerprint(user);
    if from == turn && sent.get(&session.id) == Some(&print) {
        return turn + 1;
    }
    sent.insert(session.id.clone(), print);
    from
}

/// Per session, the current turn's command outputs as last published.
static SENT_OUTPUTS: LazyLock<Mutex<HashMap<String, SentOutputs>>> =
    LazyLock::new(Default::default);

#[derive(Default)]
struct SentOutputs {
    /// The turn's user message id; another turn starts over.
    turn: String,
    /// Activity item id → fingerprint of the output last sent for it.
    outputs: HashMap<String, u64>,
}

/// A command row's output (up to 16 KiB each, most of a busy turn's bytes) is
/// sent when it changes, not with every later event of the turn. The renderer
/// keeps the output it holds for a command row whose event omits it; natively a
/// command's output never goes from text back to empty. Transcript loads are whole.
fn omit_sent_outputs(
    sent: &mut HashMap<String, SentOutputs>,
    session: &Session,
    value: &mut Value,
) {
    let turn = session
        .messages
        .get(current_turn(session))
        .filter(|message| message.role == MessageRole::User)
        .map(|message| message.id.as_str())
        .unwrap_or_default();
    let entry = sent.entry(session.id.clone()).or_default();
    if entry.turn != turn {
        entry.turn = turn.to_owned();
        entry.outputs.clear();
    }
    let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    let items = messages
        .iter_mut()
        .filter_map(|message| message.pointer_mut("/activity/items")?.as_array_mut())
        .flatten();
    for item in items {
        let (Some(id), Some(output)) = (
            item.get("id").and_then(Value::as_str).map(str::to_owned),
            item.get("output").and_then(Value::as_str),
        ) else {
            continue;
        };
        let mut hasher = DefaultHasher::new();
        output.hash(&mut hasher);
        let print = hasher.finish();
        if entry.outputs.get(&id) == Some(&print) {
            if let Some(item) = item.as_object_mut() {
                item.remove("output");
            }
        } else {
            entry.outputs.insert(id, print);
        }
    }
}

/// Publishes a lifecycle change whose message edits are within the current turn.
pub fn emit(app: &AppHandle, session: &Session) {
    emit_from(app, session, current_turn(session));
}

/// How long frequent activity changes (tool rows, subagent steps, context
/// readings) wait to be published together. Each event carries the whole current
/// turn, so a busy turn used to send tens of turn-sized snapshots per second.
const ACTIVITY_PUBLISH_DELAY: Duration = Duration::from_millis(250);

/// Sessions with a coalesced publication scheduled.
static SCHEDULED: LazyLock<Mutex<Coalescer>> = LazyLock::new(Default::default);

/// Sessions waiting for a deferred publication. An immediate publication sends the
/// latest snapshot, so it cancels the deferred one.
#[derive(Default)]
struct Coalescer(HashSet<String>);

impl Coalescer {
    /// Marks a session; true when no publication was already scheduled for it.
    fn request(&mut self, session_id: &str) -> bool {
        self.0.insert(session_id.to_owned())
    }
    /// True when the deferred publication is still due (not superseded).
    fn take(&mut self, session_id: &str) -> bool {
        self.0.remove(session_id)
    }
}

/// Publishes a frequent activity change at most once per [`ACTIVITY_PUBLISH_DELAY`]
/// per session. The snapshot is taken when it is sent, under the state lock, so it
/// includes every change and streamed delta recorded before it, and deltas recorded
/// after it are emitted after it: the renderer's ordering is unchanged.
pub fn emit_soon(app: &AppHandle, session_id: &str) {
    if !SCHEDULED.lock().request(session_id) {
        return;
    }
    let (app, session_id) = (app.clone(), session_id.to_owned());
    std::thread::spawn(move || {
        std::thread::sleep(ACTIVITY_PUBLISH_DELAY);
        let Some(state) = app.try_state::<std::sync::Arc<crate::commands::AppState>>() else {
            return;
        };
        let data = state.data.lock();
        if !SCHEDULED.lock().take(&session_id) {
            return;
        }
        if let Some(session) = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
        {
            emit(&app, session);
        }
    });
}

/// Publishes a change that also touched an earlier message (`from` covers it).
pub fn emit_from(app: &AppHandle, session: &Session, from: usize) {
    // This snapshot supersedes a deferred activity publication.
    SCHEDULED.lock().take(&session.id);
    // Output recorded before this snapshot reaches the renderer before it (ADR-180).
    crate::output_batch::flush(app, &session.id);
    if session.messages.loaded().is_none() {
        // A metadata change of a transcript that is not in memory (ADR-099).
        match session_event(session, 0) {
            Ok(value) => {
                let _ = app.emit("session-updated", value);
            }
            Err(error) => tracing::error!(%error, "cannot publish a session update"),
        }
        return;
    }
    let from = live_from(&mut SENT_TURNS.lock(), session, from);
    match session_event(session, from) {
        Ok(mut value) => {
            omit_sent_outputs(&mut SENT_OUTPUTS.lock(), session, &mut value);
            let _ = app.emit("session-updated", value);
        }
        Err(error) => tracing::error!(%error, "cannot publish a session update"),
    }
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    /// One owned session's transcript, when a view opens it: whole by default; a phone asks
    /// for its newest `limit` messages before index `before` and pages back as it scrolls,
    /// and `lite` leaves out what it never shows (command output, turn review diffs).
    Load {
        session_id: String,
        #[serde(default)]
        before: Option<usize>,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        lite: bool,
    },
    /// Messages that may contain `query` (case-insensitive literal), for the
    /// renderer's exact all-conversations search.
    Search { query: String },
    /// The full output of one command row whose output is stored apart (ADR-147), when the
    /// person opens it. `stepId` is the activity row's id.
    ToolOutput {
        session_id: String,
        message_id: String,
        step_id: String,
    },
    /// One agent reply's turn review with its diff texts, which transcripts and session
    /// events leave out (ADR-164), when the review pane opens.
    TurnReview {
        session_id: String,
        message_id: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Response {
    Transcript {
        session_id: String,
        messages: Vec<Message>,
        /// Index of the first message sent and the transcript's length, so a page knows
        /// whether older messages remain.
        start: usize,
        total: usize,
    },
    Candidates {
        sessions: Vec<CandidateSession>,
        truncated: bool,
    },
    /// One command row's full output (at most 1 MiB, its end).
    ToolOutput { text: String },
    /// A turn review with its diffs; `null` when the reply has none.
    TurnReview {
        review: Option<crate::turn_review::TurnReview>,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateSession {
    pub session_id: String,
    pub messages: Vec<Message>,
}

/// Sessions whose project still exists, in state order.
fn owned(data: &AppData) -> impl Iterator<Item = &Session> {
    let projects = data
        .projects
        .iter()
        .map(|project| project.id.as_str())
        .collect::<HashSet<_>>();
    data.sessions
        .iter()
        .filter(move |session| projects.contains(session.project_id.as_str()))
}

/// Messages a page may hold, and the default page.
const MAX_PAGE: usize = 500;

pub fn load(
    data: &AppData,
    session_id: &str,
    before: Option<usize>,
    limit: Option<usize>,
    lite: bool,
) -> Result<Response> {
    let session = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    let total = session.messages.len();
    let end = before.unwrap_or(total).min(total);
    let start = limit.map_or(0, |limit| end.saturating_sub(limit.clamp(1, MAX_PAGE)));
    let mut messages = session.messages[start..end].to_vec();
    messages.iter_mut().for_each(crate::turn_review::omit_diffs);
    if lite {
        for message in &mut messages {
            if let Some(activity) = message.activity.as_mut() {
                activity.review = None;
                for item in &mut activity.items {
                    item.output.clear();
                    item.output_bytes = None;
                }
            }
        }
    }
    if session.messages.failed() {
        // Never answer with an empty transcript the renderer would show as the conversation.
        return Err(Error::new(
            "persist",
            "This conversation could not be read.",
        ));
    }
    Ok(Response::Transcript {
        session_id: session.id.clone(),
        messages,
        start,
        total,
    })
}

/// One reply's whole turn review (ADR-164), from the loaded transcript.
pub fn turn_review(data: &AppData, session_id: &str, message_id: &str) -> Result<Response> {
    let session = owned(data)
        .find(|session| session.id == session_id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    let review = session
        .messages
        .iter()
        .find(|message| message.id == message_id)
        .and_then(|message| message.activity.as_ref()?.review.clone());
    Ok(Response::TurnReview { review })
}

/// One command row's full output (ADR-147): from the loaded transcript when it still holds
/// it, otherwise from the store. The state lock is held only to find the row.
pub fn tool_output(
    state: &Mutex<AppData>,
    state_path: &Path,
    session_id: &str,
    message_id: &str,
    step_id: &str,
) -> Result<Response> {
    use crate::tool_output::{bounded, locate, valid_id, Located};
    if ![session_id, message_id, step_id].into_iter().all(valid_id) {
        return Err(Error::new("invalid", "Invalid output request."));
    }
    let gone = || Error::not_found("This output is no longer available.");
    let located = {
        let data = state.lock();
        let session = owned(&data)
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        locate(session.messages.loaded(), message_id, step_id).ok_or_else(gone)?
    };
    let text = match located {
        Located::Here(text) => text,
        Located::Stored if crate::persist::uses_database(state_path) => {
            crate::store::read_tool_output(state_path, session_id, message_id, step_id)?
                .ok_or_else(gone)?
        }
        Located::Stored => return Err(gone()),
    };
    Ok(Response::ToolOutput {
        text: bounded(text),
    })
}

/// One session as a search reads it, taken under the lock: loaded transcripts are
/// copied, unloaded ones are read from their files during the scan (ADR-099).
pub struct Searched {
    session_id: String,
    project_id: String,
    transcript: crate::transcript_store::Snapshot,
}

/// What [`search_snapshot`] takes under the lock: sessions, owned project ids and
/// whether older sessions were left out.
pub type SearchSnapshot = (Vec<Searched>, HashSet<String>, bool);

/// Runs on a snapshot so a long scan never holds the state lock. Unloaded
/// transcripts are read one at a time and dropped after their matches are copied.
pub fn search((sessions, projects, skipped): SearchSnapshot, query: &str) -> Result<Response> {
    let needle = query.trim();
    if needle.is_empty() || needle.chars().count() > MAX_SEARCH_QUERY {
        return Err(Error::new(
            "invalid",
            "Search text must be 1 to 256 characters.",
        ));
    }
    let needle = needle.to_lowercase();
    let (mut found, mut bytes, mut truncated) = (Vec::<CandidateSession>::new(), 0usize, skipped);
    let mut count = 0usize;
    'sessions: for session in sessions
        .into_iter()
        .filter(|session| projects.contains(&session.project_id))
    {
        let Some(transcript) = session.transcript.read() else {
            continue;
        };
        let mut messages = Vec::new();
        for message in transcript {
            if message.role == MessageRole::System
                || message.session_id != session.session_id
                || !message.content.to_lowercase().contains(&needle)
            {
                continue;
            }
            if count >= MAX_CANDIDATES || bytes + message.content.len() > MAX_CANDIDATE_BYTES {
                truncated = true;
                if !messages.is_empty() {
                    found.push(CandidateSession {
                        session_id: session.session_id,
                        messages,
                    });
                }
                break 'sessions;
            }
            bytes += message.content.len();
            count += 1;
            let mut message = message;
            crate::turn_review::omit_diffs(&mut message);
            messages.push(message);
        }
        if !messages.is_empty() {
            found.push(CandidateSession {
                session_id: session.session_id,
                messages,
            });
        }
    }
    Ok(Response::Candidates {
        sessions: found,
        truncated,
    })
}

/// Takes what a search needs under the lock (no file reads); the scan runs after
/// release. At most [`MAX_SEARCHED_SESSIONS`], the most recently active, in state order.
pub fn search_snapshot(data: &AppData) -> SearchSnapshot {
    // Side chats are hidden from session lists, so search does not open them.
    let mut owned = owned(data)
        .filter(|session| !session.is_auxiliary())
        .enumerate()
        .collect::<Vec<_>>();
    let skipped = owned.len() > MAX_SEARCHED_SESSIONS;
    if skipped {
        owned.sort_by(|a, b| b.1.last_activity_at.cmp(&a.1.last_activity_at));
        owned.truncate(MAX_SEARCHED_SESSIONS);
        owned.sort_by_key(|(index, _)| *index);
    }
    (
        owned
            .into_iter()
            .map(|(_, session)| Searched {
                session_id: session.id.clone(),
                project_id: session.project_id.clone(),
                transcript: session.messages.snapshot(),
            })
            .collect(),
        data.projects
            .iter()
            .map(|project| project.id.clone())
            .collect(),
        skipped,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequent_publications_coalesce_and_an_immediate_one_supersedes_them() {
        let mut scheduled = Coalescer::default();
        assert!(scheduled.request("a"), "the first change schedules");
        assert!(!scheduled.request("a"), "later changes join it");
        assert!(scheduled.request("b"), "sessions are independent");
        assert!(scheduled.take("a"), "the deferred publication is due");
        assert!(!scheduled.take("a"), "and sent once");
        assert!(scheduled.request("a"), "a new change schedules again");
        // An immediate publication takes the slot, so the deferred one is skipped.
        assert!(scheduled.take("b"));
        assert!(!scheduled.take("b"));
    }

    #[test]
    fn command_replies_carry_only_the_latest_reply_on() {
        let data = data();
        let session = &data.sessions[0];
        let tail = session_tail(session).unwrap();
        let ids: Vec<_> = tail["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, ["a2", "a3"], "from the last agent reply on");
        assert_eq!(tail["title"], "A");
        assert_eq!(session_meta(session).unwrap()["messages"], json!([]));
    }

    fn data() -> AppData {
        let message = |id: &str, session: &str, role: &str, content: &str| json!({"id":id,"sessionId":session,"role":role,"content":content,"createdAt":format!("2026-10-0{}T10:00:00Z", id.len() % 9 + 1),"streaming":false});
        let mut value = serde_json::to_value(AppData::default()).unwrap();
        value["projects"] =
            json!([{"id":"p","name":"p","path":"/fixture","addedAt":"t","lastOpenedAt":"t"}]);
        value["sessions"] = json!([
            {"id":"a","title":"A","projectId":"p","agent":"codex","status":"completed","createdAt":"t","lastActivityAt":"t","worktree":{"path":"/fixture","branch":"main","isolated":false},"lastError":null,
             "forkOrigin":{"sourceSessionId":"z","sourceMessageId":"m","sourceTitle":"Z","inheritedMessageCount":1},
             "messages":[message("a0","a","user","inherited Rate limit"), message("a1","a","user","Add a RATE limit"), message("a2","a","agent","Done: rate limited"), message("a3","a","system","rate warning")]},
            {"id":"orphan","title":"O","projectId":"gone","agent":"codex","status":"completed","createdAt":"t","lastActivityAt":"t","worktree":{"path":"/fixture","branch":"main","isolated":false},"lastError":null,
             "messages":[message("o1","orphan","user","rate")]}
        ]);
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn metadata_state_and_events_never_carry_whole_transcripts() {
        let data = data();
        let state = state_meta(&data).unwrap();
        assert_eq!(state["sessions"][0]["messages"], json!([]));
        assert_eq!(state["sessions"][0]["transcriptLength"], json!(3));
        let event = session_event(&data.sessions[0], current_turn(&data.sessions[0])).unwrap();
        assert_eq!(event["transcriptWindow"], json!({"from": 1, "total": 4}));
        assert_eq!(event["messages"].as_array().unwrap().len(), 3);
        let earlier = session_event(&data.sessions[0], 0).unwrap();
        assert_eq!(earlier["messages"].as_array().unwrap().len(), 4);
        assert_eq!(
            session_event(&data.sessions[0], 99).unwrap()["transcriptWindow"]["from"],
            json!(4)
        );
        // Memory and persistence keep the inline transcript unaffected.
        assert_eq!(
            serde_json::to_value(&data.sessions[0]).unwrap()["messages"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn live_events_send_the_user_message_once_per_turn() {
        let mut data = data();
        let session = &mut data.sessions[0];
        let mut sent = HashMap::new();
        let turn = current_turn(session);
        assert_eq!(turn, 1);
        assert_eq!(
            live_from(&mut sent, session, turn),
            1,
            "first event carries it"
        );
        assert_eq!(
            live_from(&mut sent, session, turn),
            2,
            "later events skip it"
        );
        assert_eq!(
            live_from(&mut sent, session, 0),
            0,
            "explicit earlier windows stay"
        );
        assert_eq!(live_from(&mut sent, session, turn), 2);
        let event = session_event(session, live_from(&mut sent, session, turn)).unwrap();
        assert_eq!(event["transcriptWindow"], json!({"from": 2, "total": 4}));
        // An edited user message is sent again.
        session.messages[1].content.push_str(" now");
        assert_eq!(live_from(&mut sent, session, turn), 1);
        assert_eq!(live_from(&mut sent, session, turn), 2);
        // A new turn starts at its own user message.
        let mut next = session.messages[1].clone();
        next.id = "a4".into();
        session.messages.push(next);
        assert_eq!(live_from(&mut sent, session, current_turn(session)), 4);
        assert_eq!(live_from(&mut sent, session, current_turn(session)), 5);
        // Without a user message the window is unchanged.
        session
            .messages
            .retain(|message| message.role != MessageRole::User);
        assert_eq!(live_from(&mut sent, session, current_turn(session)), 0);
    }

    #[test]
    fn live_events_send_each_command_output_once_per_turn() {
        let mut data = data();
        let session = &mut data.sessions[0];
        let activity = |output: &str| {
            serde_json::from_value::<crate::activity::TurnActivity>(json!({
                "provider": "codex", "model": null, "startedAt": 1, "endedAt": null,
                "waitingSince": null, "pausedMs": 0, "status": "running", "truncated": false,
                "items": [
                    {"id": "c1", "kind": "command", "label": "Command", "state": "completed", "output": "built"},
                    {"id": "c2", "kind": "command", "label": "Command", "state": "running", "output": output}
                ]
            }))
            .unwrap()
        };
        session.messages[2].activity = Some(activity("line 1"));
        let mut sent = HashMap::new();
        let outputs = |value: &Value| {
            value["messages"][1]["activity"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| {
                    item.get("output")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect::<Vec<_>>()
        };
        let mut first = session_event(session, 1).unwrap();
        omit_sent_outputs(&mut sent, session, &mut first);
        assert_eq!(
            outputs(&first),
            [Some("built".into()), Some("line 1".into())]
        );
        let mut again = session_event(session, 1).unwrap();
        omit_sent_outputs(&mut sent, session, &mut again);
        assert_eq!(
            outputs(&again),
            [None, None],
            "unchanged outputs are not resent"
        );
        session.messages[2].activity = Some(activity("line 1\nline 2"));
        let mut grown = session_event(session, 1).unwrap();
        omit_sent_outputs(&mut sent, session, &mut grown);
        assert_eq!(outputs(&grown), [None, Some("line 1\nline 2".into())]);
        // A new turn sends everything again.
        session.messages[1].id = "next-turn".into();
        let mut next = session_event(session, 1).unwrap();
        omit_sent_outputs(&mut sent, session, &mut next);
        assert_eq!(
            outputs(&next),
            [Some("built".into()), Some("line 1\nline 2".into())]
        );
        // Persistence and loads keep every output.
        assert_eq!(
            serde_json::to_value(&session.messages[2]).unwrap()["activity"]["items"][0]["output"],
            "built"
        );
    }

    #[test]
    fn search_returns_bounded_owned_literal_candidates() {
        let data = data();
        assert!(!search_snapshot(&data).2);
        let Response::Candidates {
            sessions: found,
            truncated,
        } = search(search_snapshot(&data), "  RATE ").unwrap()
        else {
            panic!()
        };
        assert!(!truncated);
        assert_eq!(found.len(), 1, "orphaned projects are excluded");
        assert_eq!(
            found[0]
                .messages
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["a0", "a1", "a2"],
            "system text is excluded"
        );
        let again = || search_snapshot(&data);
        assert!(search(again(), "   ").is_err());
        assert!(search(again(), &"x".repeat(257)).is_err());
        assert!(
            matches!(search(again(), ".*"), Ok(Response::Candidates { sessions, .. }) if sessions.is_empty())
        );
    }

    #[test]
    fn search_reads_unloaded_transcripts_from_disk_without_keeping_them() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        crate::persist::save(&path, &data()).unwrap();
        let loaded = crate::persist::load_or_create(&path).unwrap();
        assert!(!loaded.sessions[0].messages.is_loaded());
        let Response::Candidates { sessions, .. } =
            search(search_snapshot(&loaded), "rate").unwrap()
        else {
            panic!()
        };
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].messages.len(), 3);
        assert!(
            !loaded.sessions[0].messages.is_loaded(),
            "the scan does not keep it"
        );
        // Unloaded sessions publish metadata with an empty window at their end.
        let event = session_event(&loaded.sessions[0], 0).unwrap();
        assert_eq!(event["transcriptWindow"], json!({"from": 4, "total": 4}));
        assert_eq!(event["transcriptLength"], json!(3));
        assert!(!loaded.sessions[0].messages.is_loaded());
    }

    #[test]
    fn search_reads_at_most_the_most_recent_sessions() {
        let mut data = data();
        let template = data.sessions[0].clone();
        data.sessions = (0..=MAX_SEARCHED_SESSIONS)
            .map(|index| {
                let mut session = template.clone();
                session.id = format!("s{index}");
                session.last_activity_at = format!("2026-01-01T00:00:{:06}Z", index);
                for message in &mut session.messages {
                    message.session_id = session.id.clone();
                }
                session
            })
            .collect();
        let (sessions, _, skipped) = search_snapshot(&data);
        assert!(skipped);
        assert_eq!(sessions.len(), MAX_SEARCHED_SESSIONS);
        assert_eq!(sessions[0].session_id, "s1", "the oldest is left out");
        let Response::Candidates { truncated, .. } = search(search_snapshot(&data), "zzz").unwrap()
        else {
            panic!()
        };
        assert!(truncated);
    }

    #[test]
    fn load_returns_only_owned_transcripts() {
        let data = data();
        assert!(load(&data, "missing", None, None, false).is_err());
        assert!(
            matches!(load(&data, "a", None, None, false), Ok(Response::Transcript { messages, start: 0, total: 4, .. }) if messages.len() == 4)
        );
    }

    #[test]
    fn load_pages_back_from_the_end() {
        let data = data();
        let page = |before, limit| match load(&data, "a", before, limit, false).unwrap() {
            Response::Transcript {
                messages,
                start,
                total,
                ..
            } => (messages.len(), start, total),
            _ => panic!(),
        };
        assert_eq!(page(None, Some(3)), (3, 1, 4));
        assert_eq!(page(Some(1), Some(3)), (1, 0, 4));
        assert_eq!(page(Some(99), Some(2)), (2, 2, 4));
        assert_eq!(page(Some(0), Some(2)), (0, 0, 4));
    }

    #[test]
    fn diffs_leave_loads_and_events_and_come_back_on_request() {
        let mut data = data();
        let session = &mut data.sessions[0];
        let index = session.messages.len() - 1;
        let reply = &mut session.messages[index];
        let id = reply.id.clone();
        let mut activity =
            crate::activity::TurnActivity::new(crate::models::AgentProviderId::Codex, None);
        activity.review = Some(
            serde_json::from_value(json!({"files": [
                {"path": "a.rs", "kind": "modified", "additions": 1, "deletions": 0, "binary": false, "diff": "+x"},
                {"path": "b.png", "kind": "added", "additions": 0, "deletions": 0, "binary": true, "diff": null}
            ], "partial": false, "sharedWorkspace": false, "keptAt": null}))
            .unwrap(),
        );
        reply.activity = Some(activity);
        let Response::Transcript { messages, .. } = load(&data, "a", None, None, false).unwrap()
        else {
            panic!();
        };
        let files = &messages
            .last()
            .unwrap()
            .activity
            .as_ref()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .files;
        assert_eq!(
            (files[0].diff.as_deref(), files[0].diff_omitted),
            (None, true)
        );
        assert_eq!(
            (files[1].diff.as_deref(), files[1].diff_omitted),
            (None, false),
            "nothing to fetch for a binary file"
        );
        let event = session_event(&data.sessions[0], 0).unwrap();
        let sent =
            &event["messages"].as_array().unwrap().last().unwrap()["activity"]["review"]["files"];
        assert_eq!(
            (&sent[0]["diff"], &sent[0]["diffOmitted"]),
            (&Value::Null, &json!(true))
        );
        assert!(sent[1].get("diffOmitted").is_none());
        let Response::TurnReview {
            review: Some(review),
        } = turn_review(&data, "a", &id).unwrap()
        else {
            panic!();
        };
        assert_eq!(review.files[0].diff.as_deref(), Some("+x"));
        assert!(
            serde_json::to_value(&review).unwrap()["files"][0]
                .get("diffOmitted")
                .is_none(),
            "never stored"
        );
    }
}
