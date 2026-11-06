//! Conversations as the back end sends them, shaped for drawing: each user message and the
//! reply after it make one turn.
use crate::i18n::t;
use serde_json::Value;
use std::time::Instant;

pub struct Step {
    pub kind: String,
    pub label: String,
    pub detail: String,
    pub running: bool,
    /// The image file a read or edit names, or the picture a tool's result carried (its path
    /// from the host's `imagePath`), which the row opens.
    pub image: Option<String>,
    /// The picture is a screenshot a tool returned, so its row says "Captura da tela".
    pub capture: bool,
    /// The MCP tool's own name (`computer_click`), when the host sent it.
    pub tool: Option<String>,
    /// What the tool acted on, when the host names it: an app or a page.
    pub target: Option<String>,
}

/// A subagent the turn launched, with its own status.
pub use crate::subagent_team::Member as AgentRun;

/// A handoff to another provider (`session.handoff`): where it came from, its request, and whether
/// its recap still waits for the first send.
#[derive(Clone, Debug, PartialEq)]
pub struct Handoff {
    pub from: &'static str,
    pub request: String,
    pub pending: bool,
}

impl Handoff {
    fn of(value: &Value) -> Option<Self> {
        Some(Self {
            from: provider_id(value["from"].as_str()?),
            request: value["request"].as_str().unwrap_or("").to_owned(),
            pending: value["pending"].as_bool().unwrap_or(false),
        })
    }
}

pub struct Turn {
    pub user: String,
    /// The files sent with the message.
    pub attachments: Vec<crate::attachment_doc::Info>,
    pub steps: Vec<Step>,
    pub agents: Vec<AgentRun>,
    /// The subagent card's title: the plan's heading, else the first line of the request.
    pub headline: String,
    /// Who answered this turn, which can differ from the conversation's current provider.
    pub provider: Option<&'static str>,
    /// Visuals the agent published: (id, title, native source, or the old HTML).
    pub visuals: Vec<Visual>,
    /// Sessions the turn's replies started or delegated (`launched`, ADR-111).
    pub launched: Vec<String>,
    pub answer: String,
    /// The reply that carries the turn's changes, and the changes (message_actions.rs).
    pub message_id: Option<String>,
    /// The id of the turn's last reply, which fork, handoff and second opinion act on.
    pub reply: Option<String>,
    pub review: Option<crate::shell::message_actions::Review>,
    pub live: bool,
    pub seconds: Option<u64>,
    /// When the turn began (Unix ms) and its paused time, for the running clock.
    pub started_ms: Option<u64>,
    pub paused_ms: u64,
    pub model: Option<String>,
    pub open: bool,
    pub toggled: Option<Instant>,
    /// When it stopped being live on screen: its tree folds shut from here.
    pub finished: Option<Instant>,
    pub steps_open: bool,
    pub steps_toggled: Option<Instant>,
}

#[derive(Clone)]
pub struct Visual {
    pub id: String,
    pub title: String,
    pub source: Option<String>,
    pub html: Option<String>,
}

pub struct Conversation {
    pub session_id: String,
    pub title: String,
    pub provider: &'static str,
    pub model: Option<String>,
    pub status: String,
    pub last_activity: String,
    pub created_at: String,
    /// It was delegated by another session and its reply notifies that one (`delegatedBy.notify`).
    pub delegated_notify: bool,
    pub worktree: String,
    pub branch: String,
    /// The provider account the session uses, for the message queue's binding.
    pub account: String,
    pub approval: String,
    pub effort: Option<String>,
    pub fast: bool,
    /// The add menu's modes for the next message.
    pub planning: bool,
    pub debugging: bool,
    pub team: bool,
    pub goal: Option<String>,
    /// What the provider last reported of the context window (context_meter.rs).
    pub context: Option<crate::shell::context_meter::Reading>,
    /// The provider has a native thread to compact.
    pub native: bool,
    /// A handoff the provider switch made: the card while it is pending, the divider after.
    pub handoff: Option<Handoff>,
    pub pending: Vec<Value>,
    pub messages: Vec<Value>,
    /// The index of `messages[0]` in the whole transcript.
    pub base: usize,
    pub loaded: bool,
    pub turns: Vec<Turn>,
    /// The latest runs of the project's scripts in this session's worktree (project_scripts.rs).
    pub scripts: Vec<crate::project_scripts::Run>,
    /// The session runs in its own worktree, which is where the project's scripts run.
    pub isolated: bool,
    /// A side chat answers about another session; its scripts do not run.
    pub side_chat: bool,
}

pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub appearance: Value,
    pub open: bool,
    pub pinned: bool,
    pub conversations: Vec<usize>,
    /// Setup and On finish scripts and the preview address (project_scripts.rs).
    pub scripts: crate::project_scripts::Saved,
}

fn provider_id(agent: &str) -> &'static str {
    match agent {
        "claude" => "claude",
        "codex" => "codex",
        "cursor" => "cursor",
        "grok" => "grok",
        "antigravity" => "antigravity",
        "droid" => "droid",
        "pi" => "pi",
        "devin" => "devin",
        "hermes" => "hermes",
        _ => "opencode",
    }
}

impl Conversation {
    pub fn placeholder() -> Self {
        Self {
            session_id: String::new(),
            title: t("Conectando…").into(),
            provider: "claude",
            model: None,
            status: "idle".into(),
            last_activity: String::new(),
            created_at: String::new(),
            delegated_notify: false,
            worktree: String::new(),
            branch: String::new(),
            account: "default".into(),
            approval: "full".into(),
            effort: None,
            fast: false,
            planning: false,
            debugging: false,
            team: false,
            goal: None,
            context: None,
            native: false,
            handoff: None,
            pending: Vec::new(),
            messages: Vec::new(),
            base: 0,
            loaded: true,
            turns: Vec::new(),
            scripts: Vec::new(),
            isolated: false,
            side_chat: false,
        }
    }

    pub fn from_session(session: &Value) -> Self {
        let mut conversation = Self::placeholder();
        conversation.session_id = session["id"].as_str().unwrap_or("").to_owned();
        conversation.loaded = false;
        conversation.apply_meta(session);
        conversation
    }

    /// Everything but the transcript, from a session the back end sent.
    pub fn apply_meta(&mut self, session: &Value) {
        if let Some(title) = session["title"].as_str() {
            self.title = title.to_owned();
        }
        self.provider = provider_id(session["agent"].as_str().unwrap_or("claude"));
        self.model = session["model"].as_str().map(str::to_owned);
        self.status = session["status"].as_str().unwrap_or("idle").to_owned();
        self.last_activity = session["lastActivityAt"].as_str().unwrap_or("").to_owned();
        self.created_at = session["createdAt"].as_str().unwrap_or("").to_owned();
        self.delegated_notify = session["delegatedBy"]["notify"].as_bool() == Some(true);
        self.worktree = session["worktree"]["path"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        self.branch = session["worktree"]["branch"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        self.account = session["providerAccountId"]
            .as_str()
            .unwrap_or("default")
            .to_owned();
        if let Some(approval) = session["execution"]["approval"].as_str() {
            self.approval = approval.to_owned();
        }
        self.effort = session["execution"]["effort"].as_str().map(str::to_owned);
        self.fast = session["execution"]["fast"].as_bool().unwrap_or(false);
        self.planning = session["execution"]["planning"].as_bool().unwrap_or(false);
        self.goal = session["goal"].as_str().map(str::to_owned);
        self.context = crate::shell::context_meter::Reading::of(session);
        self.scripts = crate::project_scripts::runs_of(&session["scripts"]);
        self.isolated = session["worktree"]["isolated"].as_bool() == Some(true);
        self.side_chat = session["sideChat"].is_object();
        self.native = session["nativeThread"].is_object();
        self.handoff = Handoff::of(&session["handoff"]);
        self.pending = session["pendingRequests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
    }

    /// Known to hold nothing yet. One still loading may turn out to have a whole transcript.
    pub fn is_blank(&self) -> bool {
        self.loaded && self.turns.is_empty() && self.handoff.is_none()
    }

    pub fn working(&self) -> bool {
        matches!(self.status.as_str(), "starting" | "running" | "waiting")
    }

    /// How long since the last activity, for the sidebar.
    pub fn age(&self) -> String {
        let Ok(when) = chrono::DateTime::parse_from_rfc3339(&self.last_activity) else {
            return String::new();
        };
        let minutes = (chrono::Utc::now() - when.to_utc()).num_minutes().max(0);
        match minutes {
            0 => t("agora").into(),
            1..=59 => format!("{minutes} min"),
            60..=1439 => format!("{} h", minutes / 60),
            _ => format!("{} d", minutes / 1440),
        }
    }

    /// Rebuilds the turns from the messages, keeping what the person opened or folded.
    pub fn rebuild(&mut self) {
        let turns = turns_of(&self.messages);
        let old = std::mem::take(&mut self.turns);
        self.turns = self.carried_over(turns, old, 0);
    }

    /// Only the newest turn changed, as while a reply streams: the older ones stay as they are,
    /// so a long conversation costs the same as a short one.
    pub fn rebuild_last(&mut self) {
        let Some(last) = self.messages.iter().rposition(|m| m["role"] == "user") else {
            return self.rebuild();
        };
        let before = self.messages[..last]
            .iter()
            .filter(|m| m["role"] == "user")
            .count()
            + usize::from(last > 0 && self.messages[0]["role"] != "user");
        if self.turns.len() < before {
            return self.rebuild();
        }
        let old = self.turns.split_off(before);
        let tail = self.carried_over(turns_of(&self.messages[last..]), old, before);
        self.turns.extend(tail);
    }

    /// New turns keep what the person opened or folded in the old ones; `offset` is where they start.
    fn carried_over(&self, mut turns: Vec<Turn>, old: Vec<Turn>, offset: usize) -> Vec<Turn> {
        let working = self.working();
        let mut old = old.into_iter();
        let count = offset + turns.len();
        for (ix, turn) in turns.iter_mut().enumerate() {
            if offset + ix + 1 == count && working && turn.answer.is_empty() {
                turn.live = true;
            }
            if let Some(before) = old.next() {
                turn.open = before.open;
                turn.toggled = before.toggled;
                turn.steps_open = before.steps_open;
                turn.steps_toggled = before.steps_toggled;
                turn.finished = before
                    .finished
                    .or((before.live && !turn.live).then(Instant::now));
            }
        }
        turns
    }
}

fn blank(user: &str) -> Turn {
    Turn {
        user: user.to_owned(),
        attachments: Vec::new(),
        steps: Vec::new(),
        agents: Vec::new(),
        headline: String::new(),
        provider: None,
        visuals: Vec::new(),
        launched: Vec::new(),
        answer: String::new(),
        message_id: None,
        reply: None,
        review: None,
        live: false,
        seconds: None,
        started_ms: None,
        paused_ms: 0,
        model: None,
        open: false,
        toggled: None,
        finished: None,
        steps_open: false,
        steps_toggled: None,
    }
}

/// The UTF-16 length, which is what the back end's offsets count.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Where `/skill` and `@file` tokens sit in a text: at its start or after a space.
pub fn token_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = None;
    let mut before_space = true;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                if i > s + 1 {
                    ranges.push(s..i);
                }
            }
            before_space = true;
        } else {
            if before_space && (c == '/' || c == '@') {
                start = Some(i);
            }
            before_space = false;
        }
    }
    if let Some(s) = start {
        if text.len() > s + 1 {
            ranges.push(s..text.len());
        }
    }
    ranges
}

/// The turns of a run of messages: a user message and what answered it.
fn turns_of(messages: &[Value]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut phaser = crate::subagent_team::Phaser::default();
    for message in messages {
        let role = message["role"].as_str().unwrap_or("");
        if role == "user" {
            let mut turn = blank(message["content"].as_str().unwrap_or(""));
            turn.attachments = message["attachments"]
                .as_array()
                .into_iter()
                .flatten()
                .map(crate::attachment_doc::Info::from_message)
                .collect();
            turns.push(turn);
            phaser = crate::subagent_team::Phaser::default();
        } else if role == "agent" {
            if turns.is_empty() {
                turns.push(blank(""));
            }
            let turn = turns.last_mut().unwrap();
            if let Some(text) = message["content"].as_str() {
                turn.answer.push_str(text);
            }
            turn.reply = message["id"].as_str().map(str::to_owned);
            turn.launched.extend(
                message["launched"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|id| id.as_str().map(str::to_owned)),
            );
            let activity = &message["activity"];
            if let Some(review) = crate::shell::message_actions::Review::of(&activity["review"]) {
                turn.message_id = message["id"].as_str().map(str::to_owned);
                turn.review = Some(review);
            }
            turn.model = activity["model"]
                .as_str()
                .map(str::to_owned)
                .or(turn.model.take());
            if let Some(provider) = activity["provider"].as_str() {
                turn.provider = Some(provider_id(provider));
            }
            for visual in message["visuals"].as_array().into_iter().flatten() {
                turn.visuals.push(Visual {
                    id: visual["id"].as_str().unwrap_or("").to_owned(),
                    title: visual["title"].as_str().unwrap_or("Visual").to_owned(),
                    source: visual["source"].as_str().map(str::to_owned),
                    html: visual["html"].as_str().map(str::to_owned),
                });
            }
            turn.started_ms = activity["startedAt"].as_u64();
            turn.paused_ms = activity["pausedMs"].as_u64().unwrap_or(0);
            if let (Some(start), Some(end)) =
                (activity["startedAt"].as_u64(), activity["endedAt"].as_u64())
            {
                let paused = activity["pausedMs"].as_u64().unwrap_or(0);
                turn.seconds = Some(end.saturating_sub(start).saturating_sub(paused) / 1000);
            }
            turn.live = message["streaming"].as_bool().unwrap_or(false);
            let content = message["content"].as_str().unwrap_or("");
            for item in activity["items"].as_array().into_iter().flatten() {
                if item["kind"] == "agent" {
                    // Background shells and "finished" markers are neither subagents nor a break.
                    let marker = item["shell"].as_bool().unwrap_or(false)
                        || item["finishedTask"].as_bool().unwrap_or(false);
                    if marker {
                        continue;
                    }
                    let offset = item["offset"].as_u64().map(|o| o as u32);
                    if turn.agents.is_empty() {
                        let head = crate::subagent_team::reply_head(content, offset);
                        turn.headline = crate::subagent_team::headline(&turn.user, head);
                    }
                    let phase = phaser.member(offset);
                    turn.agents.push(AgentRun::from_item(item, phase));
                    continue;
                }
                phaser.other();
                let kind = item["kind"].as_str().unwrap_or("tool").to_owned();
                let detail = item["detail"].as_str().unwrap_or("").to_owned();
                let named = crate::tool_images::step_image(&kind, &detail);
                let saved = item["imagePath"].as_str().map(str::to_owned);
                turn.steps.push(Step {
                    capture: named.is_none() && saved.is_some(),
                    image: named.or(saved),
                    kind,
                    label: item["label"].as_str().unwrap_or("").to_owned(),
                    detail,
                    tool: item["tool"].as_str().map(str::to_owned),
                    target: item["target"].as_str().map(str::to_owned),
                    running: item["state"].as_str() == Some("running"),
                });
            }
        }
    }
    turns
}
