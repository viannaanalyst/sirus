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
}

/// A subagent the turn launched, with its own status.
pub struct AgentRun {
    pub label: String,
    pub running: bool,
    pub failed: bool,
    pub seconds: Option<u64>,
    pub tools: usize,
}

pub struct Turn {
    pub user: String,
    pub steps: Vec<Step>,
    pub agents: Vec<AgentRun>,
    /// Who answered this turn, which can differ from the conversation's current provider.
    pub provider: Option<&'static str>,
    /// Visuals the agent published: (id, title, native source, or the old HTML).
    pub visuals: Vec<Visual>,
    pub answer: String,
    pub live: bool,
    pub seconds: Option<u64>,
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
    pub worktree: String,
    pub branch: String,
    pub approval: String,
    pub effort: Option<String>,
    pub fast: bool,
    /// The add menu's modes for the next message.
    pub planning: bool,
    pub debugging: bool,
    pub team: bool,
    pub goal: Option<String>,
    pub pending: Vec<Value>,
    pub messages: Vec<Value>,
    /// The index of `messages[0]` in the whole transcript.
    pub base: usize,
    pub loaded: bool,
    pub turns: Vec<Turn>,
}

pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub appearance: Value,
    pub open: bool,
    pub pinned: bool,
    pub conversations: Vec<usize>,
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
            worktree: String::new(),
            branch: String::new(),
            approval: "full".into(),
            effort: None,
            fast: false,
            planning: false,
            debugging: false,
            team: false,
            goal: None,
            pending: Vec::new(),
            messages: Vec::new(),
            base: 0,
            loaded: true,
            turns: Vec::new(),
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
        self.worktree = session["worktree"]["path"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        self.branch = session["worktree"]["branch"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        if let Some(approval) = session["execution"]["approval"].as_str() {
            self.approval = approval.to_owned();
        }
        self.effort = session["execution"]["effort"].as_str().map(str::to_owned);
        self.fast = session["execution"]["fast"].as_bool().unwrap_or(false);
        self.planning = session["execution"]["planning"].as_bool().unwrap_or(false);
        self.goal = session["goal"].as_str().map(str::to_owned);
        self.pending = session["pendingRequests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
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
        let working = self.working();
        let mut old = std::mem::take(&mut self.turns).into_iter();
        let mut turns: Vec<Turn> = Vec::new();
        for message in &self.messages {
            let role = message["role"].as_str().unwrap_or("");
            if role == "user" {
                turns.push(blank(message["content"].as_str().unwrap_or("")));
            } else if role == "agent" {
                if turns.is_empty() {
                    turns.push(blank(""));
                }
                let turn = turns.last_mut().unwrap();
                if let Some(text) = message["content"].as_str() {
                    turn.answer.push_str(text);
                }
                let activity = &message["activity"];
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
                if let (Some(start), Some(end)) =
                    (activity["startedAt"].as_u64(), activity["endedAt"].as_u64())
                {
                    let paused = activity["pausedMs"].as_u64().unwrap_or(0);
                    turn.seconds = Some(end.saturating_sub(start).saturating_sub(paused) / 1000);
                }
                turn.live = message["streaming"].as_bool().unwrap_or(false);
                for item in activity["items"].as_array().into_iter().flatten() {
                    if item["kind"] == "agent" {
                        let state = item["state"].as_str().unwrap_or("");
                        turn.agents.push(AgentRun {
                            label: item["label"].as_str().unwrap_or("").to_owned(),
                            running: state == "running",
                            failed: matches!(state, "failed" | "stopped"),
                            seconds: item["startedAt"]
                                .as_u64()
                                .zip(item["endedAt"].as_u64())
                                .map(|(a, b)| b.saturating_sub(a) / 1000),
                            tools: item["steps"].as_array().map_or(0, Vec::len),
                        });
                        continue;
                    }
                    turn.steps.push(Step {
                        kind: item["kind"].as_str().unwrap_or("tool").to_owned(),
                        label: item["label"].as_str().unwrap_or("").to_owned(),
                        detail: item["detail"].as_str().unwrap_or("").to_owned(),
                        running: item["state"].as_str() == Some("running"),
                    });
                }
            }
        }
        let count = turns.len();
        for (ix, turn) in turns.iter_mut().enumerate() {
            if ix + 1 == count && working && turn.answer.is_empty() {
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
        self.turns = turns;
    }
}

fn blank(user: &str) -> Turn {
    Turn {
        user: user.to_owned(),
        steps: Vec::new(),
        agents: Vec::new(),
        provider: None,
        visuals: Vec::new(),
        answer: String::new(),
        live: false,
        seconds: None,
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
