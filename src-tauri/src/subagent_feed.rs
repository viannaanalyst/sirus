//! Each subagent's own conversation, for its live view (ADR-146). The parent turn's activity
//! keeps a child as one row with its newest generic steps; this module keeps what the child
//! itself said, thought and ran, in order, with stable ids, so the renderer can stream it into a
//! side panel without touching the transcript.
//!
//! - Claude: stream-json messages carrying `parent_tool_use_id` (the `Task`/`Agent` call that
//!   launched the child) are that child's messages. Keyed by that call id.
//! - Codex: `item/started` / `item/completed` of a spawned agent's own thread. Keyed by the
//!   child row's id (`agent:<thread>`).
//!
//! Feeds live in memory only, bounded per feed, per session and in sessions. A finished Claude
//! child is reopened from the CLI's own transcript (`<config>/projects/<cwd>/<thread>/subagents/
//! agent-<id>.jsonl`, found through its `.meta.json` `toolUseId`), read inside the Claude config
//! folder only. Nothing here is written to Sirus's database. Text is masked (ADR-077).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::activity::{ItemKind, ItemState};
use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::AgentProviderId;
use crate::runtime::{AppHandle, Emitter, State};

/// Entries kept per feed; older ones only count toward `dropped`.
const MAX_ENTRIES: usize = 300;
/// Bytes of text kept per feed (texts, details and outputs together).
const MAX_FEED_BYTES: usize = 256 * 1024;
/// A message, thought or prompt is kept to its first characters.
const MAX_TEXT: usize = 4000;
/// A tool's output is kept to its last characters.
const MAX_OUTPUT: usize = 2000;
const MAX_DETAIL: usize = 240;
const MAX_FEEDS_PER_SESSION: usize = 32;
const MAX_SESSIONS: usize = 12;
const MAX_KEY: usize = 256;
/// The end of a Claude subagent transcript read to reopen it.
const TRANSCRIPT_TAIL: u64 = 2 * 1024 * 1024;
const MAX_META_FILES: usize = 1024;
const MAX_PROJECT_DIRS: usize = 4096;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    /// What the child was asked (its first user message), or a later note to it.
    Prompt,
    Text,
    Thinking,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FeedEntry {
    pub id: String,
    pub kind: EntryKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Tools: the native tool name (`Read`, `Bash`) or Sirus's generic label (Codex).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ItemKind>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ItemState>,
}

impl FeedEntry {
    fn new(id: String, kind: EntryKind) -> Self {
        Self {
            id,
            kind,
            text: String::new(),
            label: String::new(),
            tool: None,
            detail: String::new(),
            output: String::new(),
            state: None,
        }
    }
    fn size(&self) -> usize {
        self.id.len() + self.text.len() + self.label.len() + self.detail.len() + self.output.len()
    }
}

/// What one native frame adds to (or changes in) one child's feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedUpdate {
    pub key: String,
    pub entries: Vec<FeedEntry>,
}

/// `subagent-feed`: the entries one frame added or changed, already merged.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedEvent {
    pub session_id: String,
    pub key: String,
    pub entries: Vec<FeedEntry>,
    pub dropped: u32,
}

/// `subagent_feed`: one child's feed. `source` says where it came from: `live` (this run of
/// the app saw it stream), `transcript` (the provider's own file) or `none`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentFeed {
    pub key: String,
    pub entries: Vec<FeedEntry>,
    pub dropped: u32,
    pub source: &'static str,
}

#[derive(Debug, Default)]
struct Feed {
    entries: Vec<FeedEntry>,
    dropped: u32,
    bytes: usize,
    touched: u64,
}

impl Feed {
    /// Merges entries by id and returns those that changed. A tool result (no label) only
    /// updates the call it answers; a result for a call never seen is not new work.
    fn apply(&mut self, incoming: Vec<FeedEntry>) -> Vec<FeedEntry> {
        let mut changed: Vec<FeedEntry> = Vec::new();
        for entry in incoming {
            let entry = bound(entry);
            if let Some(old) = self.entries.iter_mut().find(|old| old.id == entry.id) {
                let mut next = old.clone();
                if entry.kind == EntryKind::Tool {
                    if !entry.label.is_empty() {
                        next.label = entry.label;
                        next.tool = entry.tool.or(next.tool);
                    }
                    if !entry.detail.is_empty() {
                        next.detail = entry.detail;
                    }
                    if !entry.output.is_empty() {
                        next.output = entry.output;
                    }
                    // A settled call never goes back to running.
                    if let Some(state) = entry.state {
                        if !(settled(next.state.as_ref()) && state == ItemState::Running) {
                            next.state = Some(state);
                        }
                    }
                } else if !entry.text.is_empty() {
                    next.text = entry.text;
                }
                if *old != next {
                    self.bytes = self.bytes.saturating_sub(old.size()) + next.size();
                    *old = next.clone();
                    upsert(&mut changed, next);
                }
                continue;
            }
            if entry.kind == EntryKind::Tool && entry.label.is_empty() {
                continue;
            }
            if entry.kind != EntryKind::Tool && entry.text.trim().is_empty() {
                continue;
            }
            self.bytes += entry.size();
            self.entries.push(entry.clone());
            upsert(&mut changed, entry);
        }
        while self.entries.len() > MAX_ENTRIES
            || (self.bytes > MAX_FEED_BYTES && self.entries.len() > 1)
        {
            let gone = self.entries.remove(0);
            self.bytes = self.bytes.saturating_sub(gone.size());
            self.dropped = self.dropped.saturating_add(1);
            changed.retain(|entry| entry.id != gone.id);
        }
        changed
    }
}

fn upsert(list: &mut Vec<FeedEntry>, entry: FeedEntry) {
    match list.iter_mut().find(|known| known.id == entry.id) {
        Some(known) => *known = entry,
        None => list.push(entry),
    }
}

fn settled(state: Option<&ItemState>) -> bool {
    matches!(
        state,
        Some(ItemState::Completed | ItemState::Failed | ItemState::Stopped)
    )
}

fn first_chars(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{}…", head.trim_end())
    } else {
        head
    }
}

fn last_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_owned();
    }
    format!(
        "…{}",
        text.chars().skip(count - limit + 1).collect::<String>()
    )
}

/// Masks credentials and bounds every field before anything is kept.
fn bound(mut entry: FeedEntry) -> FeedEntry {
    entry.id = entry
        .id
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_KEY)
        .collect();
    entry.text = first_chars(crate::redact::mask(entry.text.trim()).as_ref(), MAX_TEXT);
    entry.label = entry
        .label
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect();
    entry.detail = first_chars(
        crate::redact::mask(
            entry
                .detail
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim(),
        )
        .as_ref(),
        MAX_DETAIL,
    );
    entry.output = last_chars(
        &crate::activity::output(&crate::redact::mask(&entry.output)),
        MAX_OUTPUT,
    );
    entry
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY && !key.chars().any(char::is_control)
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part["type"] == "text" || part["type"].is_null())
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The entries of one Claude message line, from the stream or from a subagent transcript:
/// text, non-empty thinking and tool calls of an `assistant` line; the prompt, notes and tool
/// results of a `user` line. Harness lines (`isMeta`, attachments, summaries) are skipped.
pub fn claude_entries(line: &Value) -> Vec<FeedEntry> {
    let kind = line["type"].as_str();
    if !matches!(kind, Some("assistant" | "user"))
        || line["isMeta"] == true
        || line["isCompactSummary"] == true
    {
        return vec![];
    }
    let message = &line["message"];
    // One id per line, so its blocks keep their place; the API message id when a line has none.
    let Some(base) = line["uuid"]
        .as_str()
        .or_else(|| message["id"].as_str())
        .filter(|id| valid_key(id))
    else {
        return vec![];
    };
    if kind == Some("user") {
        if let Some(text) = message["content"].as_str() {
            let mut entry = FeedEntry::new(format!("{base}:0"), EntryKind::Prompt);
            entry.text = text.to_owned();
            return vec![entry];
        }
    }
    let Some(blocks) = message["content"].as_array() else {
        return vec![];
    };
    blocks
        .iter()
        .take(64)
        .enumerate()
        .filter_map(|(index, block)| {
            let id = format!("{base}:{index}");
            match (kind, block["type"].as_str()) {
                (Some("assistant"), Some("text")) => {
                    let mut entry = FeedEntry::new(id, EntryKind::Text);
                    entry.text = block["text"].as_str()?.to_owned();
                    Some(entry)
                }
                (Some("assistant"), Some("thinking")) => {
                    let mut entry = FeedEntry::new(id, EntryKind::Thinking);
                    entry.text = block["thinking"].as_str()?.to_owned();
                    Some(entry)
                }
                (Some("assistant"), Some("tool_use")) => {
                    let name = block["name"].as_str()?;
                    let mut entry = FeedEntry::new(
                        block["id"].as_str().filter(|id| valid_key(id))?.to_owned(),
                        EntryKind::Tool,
                    );
                    let (tool, _) = crate::activity::tool_kind(name);
                    entry.tool = Some(if matches!(name, "Agent" | "Task") {
                        ItemKind::Agent
                    } else {
                        tool
                    });
                    entry.label = name.to_owned();
                    entry.detail = crate::activity::tool_input_detail(&block["input"]).to_owned();
                    if entry.detail.is_empty() {
                        entry.detail = block["input"]["description"]
                            .as_str()
                            .unwrap_or("")
                            .to_owned();
                    }
                    entry.state = Some(ItemState::Running);
                    Some(entry)
                }
                (Some("user"), Some("tool_result")) => {
                    let mut entry = FeedEntry::new(
                        block["tool_use_id"]
                            .as_str()
                            .filter(|id| valid_key(id))?
                            .to_owned(),
                        EntryKind::Tool,
                    );
                    entry.state = Some(if block["is_error"] == true {
                        ItemState::Failed
                    } else {
                        ItemState::Completed
                    });
                    entry.output = text_of(&block["content"]);
                    Some(entry)
                }
                (Some("user"), Some("text")) => {
                    let mut entry = FeedEntry::new(id, EntryKind::Prompt);
                    entry.text = block["text"].as_str()?.to_owned();
                    Some(entry)
                }
                _ => None,
            }
        })
        .collect()
}

/// A Claude stream frame that belongs to a subagent: its entries under the call that launched it.
pub fn from_claude(frame: &Value) -> Option<FeedUpdate> {
    let key = frame["parent_tool_use_id"]
        .as_str()
        .filter(|key| valid_key(key))?;
    let entries = claude_entries(frame);
    (!entries.is_empty()).then(|| FeedUpdate {
        key: key.to_owned(),
        entries,
    })
}

/// A spawned Codex agent's own item: its answer, its reasoning summary, what it was asked,
/// or one of its tools (read like the parent's rows).
pub fn codex_entries(item: &Value, started: bool) -> Vec<FeedEntry> {
    let Some(id) = item["id"].as_str().filter(|id| valid_key(id)) else {
        return vec![];
    };
    match item["type"].as_str() {
        Some("agentMessage") => {
            if started {
                return vec![];
            }
            let mut entry = FeedEntry::new(id.to_owned(), EntryKind::Text);
            entry.text = item["text"].as_str().unwrap_or("").to_owned();
            vec![entry]
        }
        Some("reasoning") => {
            if started {
                return vec![];
            }
            let join = |key: &str| {
                item[key]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(Value::as_str)
                            .filter(|text| !text.trim().is_empty())
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    })
                    .unwrap_or_default()
            };
            let mut entry = FeedEntry::new(id.to_owned(), EntryKind::Thinking);
            entry.text = Some(join("summary"))
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| join("content"));
            vec![entry]
        }
        Some("userMessage") => {
            let mut entry = FeedEntry::new(id.to_owned(), EntryKind::Prompt);
            entry.text = text_of(&item["content"]);
            vec![entry]
        }
        _ => crate::activity::codex(item, started)
            .into_iter()
            .filter(|row| row.kind != ItemKind::Agent && row.thought.is_none())
            .map(|row| {
                let mut entry = FeedEntry::new(row.id, EntryKind::Tool);
                entry.label = row.label;
                entry.tool = Some(row.kind);
                entry.detail = row.detail;
                entry.output = row.output;
                entry.state = Some(row.state).filter(|state| *state != ItemState::Unknown);
                entry
            })
            .collect(),
    }
}

/// A Codex child thread's item: its entries under the child row (`agent:<thread>`).
pub fn from_codex(item: &Value, started: bool, thread: &str) -> Option<FeedUpdate> {
    if thread.is_empty() || thread.len() > 192 || thread.chars().any(char::is_control) {
        return None;
    }
    let entries = codex_entries(item, started);
    (!entries.is_empty()).then(|| FeedUpdate {
        key: format!("agent:{thread}"),
        entries,
    })
}

#[derive(Default)]
struct Registry {
    /// Most recent session last.
    sessions: Vec<(String, HashMap<String, Feed>)>,
    clock: u64,
    /// Claude: call id → its subagent transcript, once found.
    transcripts: HashMap<String, PathBuf>,
}

impl Registry {
    fn record(&mut self, session_id: &str, update: FeedUpdate) -> Option<FeedEvent> {
        self.clock += 1;
        let clock = self.clock;
        let index = match self.sessions.iter().position(|(id, _)| id == session_id) {
            Some(index) => index,
            None => {
                if self.sessions.len() >= MAX_SESSIONS {
                    self.sessions.remove(0);
                }
                self.sessions.push((session_id.to_owned(), HashMap::new()));
                self.sessions.len() - 1
            }
        };
        let (id, mut feeds) = self.sessions.remove(index);
        if !feeds.contains_key(&update.key) && feeds.len() >= MAX_FEEDS_PER_SESSION {
            if let Some(oldest) = feeds
                .iter()
                .min_by_key(|(_, feed)| feed.touched)
                .map(|(key, _)| key.clone())
            {
                feeds.remove(&oldest);
            }
        }
        let feed = feeds.entry(update.key.clone()).or_default();
        feed.touched = clock;
        let entries = feed.apply(update.entries);
        let dropped = feed.dropped;
        self.sessions.push((id, feeds));
        (!entries.is_empty()).then(|| FeedEvent {
            session_id: session_id.to_owned(),
            key: update.key,
            entries,
            dropped,
        })
    }

    fn read(&self, session_id: &str, key: &str) -> Option<SubagentFeed> {
        let (_, feeds) = self.sessions.iter().find(|(id, _)| id == session_id)?;
        let feed = feeds.get(key).filter(|feed| !feed.entries.is_empty())?;
        Some(SubagentFeed {
            key: key.to_owned(),
            entries: feed.entries.clone(),
            dropped: feed.dropped,
            source: "live",
        })
    }
}

static FEEDS: LazyLock<Mutex<Registry>> = LazyLock::new(Mutex::default);

/// Keeps a frame's entries and tells the renderer what changed (`subagent-feed`).
pub fn publish(app: &AppHandle, session_id: &str, mut update: FeedUpdate) {
    scrub(session_id, &mut update.entries);
    let event = FEEDS.lock().record(session_id, update);
    if let Some(event) = event {
        let _ = app.emit("subagent-feed", event);
    }
}

/// A value the person provided privately (ADR-077) never reaches a feed.
fn scrub(session_id: &str, entries: &mut [FeedEntry]) {
    for entry in entries {
        for field in [&mut entry.text, &mut entry.detail, &mut entry.output] {
            if !field.is_empty() {
                *field = crate::secrets::scrub(session_id, std::mem::take(field));
            }
        }
    }
}

/// A removed conversation's feeds go with it.
pub fn forget(session_id: &str) {
    FEEDS.lock().sessions.retain(|(id, _)| id != session_id);
}

/// One subagent's feed: what this run of the app saw stream, else (Claude) the end of the
/// CLI's own transcript for it, else nothing.
pub async fn subagent_feed(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    key: String,
) -> Result<SubagentFeed> {
    if !valid_key(&key) {
        return Err(Error::new("invalid", "Unknown subagent."));
    }
    if let Some(feed) = FEEDS.lock().read(&session_id, &key) {
        return Ok(feed);
    }
    let native = {
        let data = state.data.lock();
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        (session.agent == AgentProviderId::Claude)
            .then(|| session.native_thread.clone())
            .flatten()
    };
    let empty = SubagentFeed {
        key: key.clone(),
        entries: vec![],
        dropped: 0,
        source: "none",
    };
    let Some(native) = native else {
        return Ok(empty);
    };
    let Some(config) = crate::provider_accounts::claude_config_dir(
        &state,
        &native.provider_account_id,
        &native.cwd,
    ) else {
        return Ok(empty);
    };
    let known = FEEDS.lock().transcripts.get(&key).cloned();
    let lookup_key = key.clone();
    let read = crate::runtime::async_runtime::spawn_blocking(move || {
        let root = config.join("projects");
        let path = known
            .or_else(|| find_transcript(&root, &native.cwd, &native.thread_id, &lookup_key))?;
        let contents = read_tail(&root, &path)?;
        Some((path, contents))
    })
    .await
    .ok()
    .flatten();
    let Some((path, contents)) = read else {
        return Ok(empty);
    };
    FEEDS.lock().transcripts.insert(key.clone(), path);
    let mut feed = Feed::default();
    for line in contents.lines() {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            let mut entries = claude_entries(&value);
            scrub(&session_id, &mut entries);
            feed.apply(entries);
        }
    }
    Ok(SubagentFeed {
        key,
        dropped: feed.dropped,
        entries: feed.entries,
        source: "transcript",
    })
}

fn plain_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// The subagent transcript whose `.meta.json` names `call` as its `toolUseId`, in the native
/// conversation's folder (`<projects>/<cwd as a folder name>/<thread>/subagents`). Long paths
/// are shortened by the CLI, so the conversation folder is looked for in every project.
fn find_transcript(projects: &Path, cwd: &str, thread: &str, call: &str) -> Option<PathBuf> {
    if !plain_id(thread) {
        return None;
    }
    let expected = projects
        .join(crate::account_move::claude_project_name(cwd))
        .join(thread);
    let conversation = if expected.is_dir() {
        expected
    } else {
        std::fs::read_dir(projects)
            .ok()?
            .take(MAX_PROJECT_DIRS)
            .flatten()
            .map(|entry| entry.path().join(thread))
            .find(|dir| dir.is_dir())?
    };
    let folder = conversation.join("subagents");
    for entry in std::fs::read_dir(&folder)
        .ok()?
        .take(MAX_META_FILES)
        .flatten()
    {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(agent) = name
            .strip_prefix("agent-")
            .and_then(|rest| rest.strip_suffix(".meta.json"))
            .filter(|agent| plain_id(agent))
        else {
            continue;
        };
        if !std::fs::symlink_metadata(&path)
            .is_ok_and(|meta| meta.is_file() && meta.len() <= 64 * 1024)
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if meta["toolUseId"].as_str() == Some(call) {
            return Some(folder.join(format!("agent-{agent}.jsonl")));
        }
    }
    None
}

/// The end of a transcript, only when it resolves to a regular `.jsonl` inside `root`.
fn read_tail(root: &Path, path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let resolved = crate::paths::ensure_within(root, path).ok()?;
    if resolved.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return None;
    }
    let mut file = crate::paths::open_regular_within(root, &resolved).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(TRANSCRIPT_TAIL);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(TRANSCRIPT_TAIL).read_to_end(&mut bytes).ok()?;
    // A tail starts mid-line: drop that first line.
    let whole = if start > 0 {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(&[][..], |at| &bytes[at + 1..])
    } else {
        &bytes[..]
    };
    Some(String::from_utf8_lossy(whole).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant(parent: &str, uuid: &str, content: Value) -> Value {
        json!({"type":"assistant","parent_tool_use_id":parent,"uuid":uuid,"message":{"id":"msg","content":content}})
    }

    #[test]
    fn claude_child_frames_route_to_the_call_that_launched_them() {
        let frame = assistant(
            "toolu_1",
            "u1",
            json!([{"type":"thinking","thinking":"Look at the parser first."},{"type":"text","text":"Reading it."},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/repo/src/a.rs"}}]),
        );
        let update = from_claude(&frame).unwrap();
        assert_eq!(update.key, "toolu_1");
        let kinds: Vec<_> = update.entries.iter().map(|entry| entry.kind).collect();
        assert_eq!(
            kinds,
            [EntryKind::Thinking, EntryKind::Text, EntryKind::Tool]
        );
        assert_eq!(update.entries[2].id, "t1");
        assert_eq!(update.entries[2].label, "Read");
        assert_eq!(update.entries[2].tool, Some(ItemKind::Read));
        assert_eq!(update.entries[2].detail, "/repo/src/a.rs");
        // The parent's own frames are not a child's.
        let parent = json!({"type":"assistant","parent_tool_use_id":null,"uuid":"p","message":{"content":[{"type":"text","text":"Hi"}]}});
        assert!(from_claude(&parent).is_none());
        // Empty (redacted) thinking says nothing.
        let empty = assistant(
            "toolu_1",
            "u2",
            json!([{"type":"thinking","thinking":"","signature":"x"}]),
        );
        let mut feed = Feed::default();
        assert!(feed.apply(from_claude(&empty).unwrap().entries).is_empty());
    }

    #[test]
    fn a_tool_result_settles_its_call_and_never_reopens_it() {
        let mut feed = Feed::default();
        feed.apply(claude_entries(&assistant(
            "c",
            "u1",
            json!([{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"npm test\n--watch"}}]),
        )));
        let result = json!({"type":"user","parent_tool_use_id":"c","uuid":"u2","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":[{"type":"text","text":"\u{1b}[31mFAIL\u{1b}[0m 2 tests"}]}]}});
        let changed = feed.apply(claude_entries(&result));
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].state, Some(ItemState::Failed));
        assert_eq!(changed[0].output, "FAIL 2 tests");
        assert_eq!(changed[0].detail, "npm test");
        // A late running frame for the same call does not move it back.
        let again = feed.apply(vec![FeedEntry {
            state: Some(ItemState::Running),
            ..FeedEntry::new("t1".into(), EntryKind::Tool)
        }]);
        assert!(again.is_empty());
        // A result for a call never seen is not new work.
        let orphan = json!({"type":"user","uuid":"u3","message":{"content":[{"type":"tool_result","tool_use_id":"zz","content":"x"}]}});
        assert!(feed.apply(claude_entries(&orphan)).is_empty());
        assert_eq!(feed.entries.len(), 1);
    }

    #[test]
    fn transcript_lines_read_like_stream_frames_and_skip_harness_lines() {
        let lines = [
            json!({"type":"user","uuid":"a","isSidechain":true,"message":{"role":"user","content":"Audit the parser"}}),
            json!({"type":"attachment","uuid":"b","attachment":{"type":"hook"}}),
            json!({"type":"user","uuid":"c","isMeta":true,"message":{"content":"<reminder>"}}),
            json!({"type":"assistant","uuid":"d","message":{"content":[{"type":"text","text":"Done: token sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789 found"}]}}),
        ];
        let mut feed = Feed::default();
        for line in &lines {
            feed.apply(claude_entries(line));
        }
        assert_eq!(feed.entries.len(), 2);
        assert_eq!(feed.entries[0].kind, EntryKind::Prompt);
        assert_eq!(feed.entries[0].text, "Audit the parser");
        assert!(!feed.entries[1]
            .text
            .contains("abcdefghijklmnopqrstuvwxyz0123456789"));
    }

    #[test]
    fn codex_child_items_become_entries_under_the_child_row() {
        let message = json!({"type":"agentMessage","id":"m1","text":"All green."});
        let update = from_codex(&message, false, "thread-1").unwrap();
        assert_eq!(update.key, "agent:thread-1");
        assert_eq!(update.entries[0].kind, EntryKind::Text);
        assert!(from_codex(&message, true, "thread-1").is_none());
        let reasoning =
            json!({"type":"reasoning","id":"r1","summary":["Check the lockfile."],"content":[]});
        assert_eq!(
            codex_entries(&reasoning, false)[0].text,
            "Check the lockfile."
        );
        let command = json!({"type":"commandExecution","id":"x1","command":"cargo test","status":"inProgress","commandActions":[]});
        let started = codex_entries(&command, true);
        assert_eq!(started[0].kind, EntryKind::Tool);
        assert_eq!(started[0].tool, Some(ItemKind::Command));
        assert_eq!(started[0].state, Some(ItemState::Running));
        let spawn = json!({"type":"collabAgentToolCall","id":"s1","tool":"spawnAgent","status":"completed","receiverThreadIds":["t2"]});
        assert!(codex_entries(&spawn, false).is_empty());
    }

    #[test]
    fn feeds_are_bounded_in_entries_bytes_feeds_and_sessions() {
        let mut feed = Feed::default();
        let many: Vec<FeedEntry> = (0..MAX_ENTRIES + 5)
            .map(|index| FeedEntry {
                text: format!("line {index}"),
                ..FeedEntry::new(format!("e{index}"), EntryKind::Text)
            })
            .collect();
        feed.apply(many);
        assert_eq!(feed.entries.len(), MAX_ENTRIES);
        assert_eq!(feed.dropped, 5);
        assert_eq!(feed.entries[0].id, "e5");
        let long = FeedEntry {
            text: "x".repeat(MAX_TEXT * 3),
            ..FeedEntry::new("long".into(), EntryKind::Text)
        };
        let kept = feed.apply(vec![long]);
        assert_eq!(kept[0].text.chars().count(), MAX_TEXT + 1);
        let big: Vec<FeedEntry> = (0..200)
            .map(|index| FeedEntry {
                text: "y".repeat(MAX_TEXT),
                ..FeedEntry::new(format!("b{index}"), EntryKind::Text)
            })
            .collect();
        feed.apply(big);
        assert!(feed.bytes <= MAX_FEED_BYTES);

        let mut registry = Registry::default();
        let update = |key: &str| FeedUpdate {
            key: key.into(),
            entries: vec![FeedEntry {
                text: "hi".into(),
                ..FeedEntry::new("e".into(), EntryKind::Text)
            }],
        };
        for index in 0..MAX_FEEDS_PER_SESSION + 3 {
            assert!(registry.record("s", update(&format!("k{index}"))).is_some());
        }
        assert_eq!(registry.sessions[0].1.len(), MAX_FEEDS_PER_SESSION);
        assert!(
            registry.read("s", "k0").is_none(),
            "the least recent feed left"
        );
        assert_eq!(registry.read("s", "k5").unwrap().source, "live");
        // The same entry again changes nothing and emits nothing.
        assert!(registry.record("s", update("k5")).is_none());
        for index in 0..MAX_SESSIONS + 2 {
            registry.record(&format!("s{index}"), update("k"));
        }
        assert_eq!(registry.sessions.len(), MAX_SESSIONS);
    }

    #[test]
    fn a_finished_subagent_is_found_by_its_call_in_the_conversation_folder() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        let folder = projects.join("-repo").join("thread-1").join("subagents");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("agent-a1.meta.json"),
            r#"{"toolUseId":"toolu_other"}"#,
        )
        .unwrap();
        std::fs::write(
            folder.join("agent-a2.meta.json"),
            r#"{"toolUseId":"toolu_mine","description":"Audit"}"#,
        )
        .unwrap();
        std::fs::write(
            folder.join("agent-a2.jsonl"),
            "{\"type\":\"user\",\"uuid\":\"a\",\"message\":{\"content\":\"Audit\"}}\n",
        )
        .unwrap();
        let found = find_transcript(&projects, "/repo", "thread-1", "toolu_mine").unwrap();
        assert_eq!(found, folder.join("agent-a2.jsonl"));
        assert!(read_tail(&projects, &found).unwrap().contains("Audit"));
        assert!(find_transcript(&projects, "/repo", "../x", "toolu_mine").is_none());
        assert!(find_transcript(&projects, "/repo", "thread-1", "toolu_none").is_none());
        // Outside the projects folder nothing is read.
        let outside = dir.path().join("agent-x.jsonl");
        std::fs::write(&outside, "{}\n").unwrap();
        assert!(read_tail(&projects, &outside).is_none());
    }
}
