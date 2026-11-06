//! Read-only turn presentation. Never grants authority. Each row keeps a short, bounded
//! `detail` (file path, command line, search query, skill name) and the reply length when it
//! started, so the transcript can place it between the text it interleaves with. Tool output
//! is never retained, and reasoning only as the live first sentence of the latest thought
//! (`TurnActivity::thought`, ADR-101), which never reaches a transcript file.
use crate::models::{AgentProviderId, MessageRole, Session, SessionStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::Cell;

/// Longest live thought shown (characters), and the reasoning read to find its first sentence.
const THOUGHT_CHARS: usize = 140;
const THOUGHT_BUFFER: usize = 2048;

thread_local! {
    static PERSISTING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `serialize` as a transcript write: live-only fields (the latest thought) are left out.
pub fn persisting<R>(serialize: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            PERSISTING.with(|flag| flag.set(self.0));
        }
    }
    let _restore = Restore(PERSISTING.with(|flag| flag.replace(true)));
    serialize()
}

fn skip_thought(thought: &Option<String>) -> bool {
    thought.is_none() || PERSISTING.with(Cell::get)
}

/// A live-only text field (a workflow member's answer so far): sent to the renderer, never
/// written to a transcript file.
pub(crate) fn live_only(text: &Option<String>) -> bool {
    skip_thought(text)
}

/// Live reasoning signals (ADR-101). They never become rows and are never persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Thought {
    /// A new reasoning block begins.
    Start,
    /// Reasoning text: a streamed chunk, or a whole block (`fresh`) that replaces the last one.
    Text { text: String, fresh: bool },
    /// The agent started answering: the thought is no longer what it is doing.
    End,
}

/// One step of the agent's own to-do plan, normalized across providers (ADR-182).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    pub text: String,
    pub status: PlanStatus,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}
/// Longest plan kept, and the longest text of one step.
const MAX_PLAN: usize = 50;
const MAX_PLAN_TEXT: usize = 200;

const MAX_ITEMS: usize = 128;
/// Newest steps kept per child; older ones only count toward `hidden_steps`.
const MAX_CHILD_STEPS: usize = 20;
/// Retained child steps across one turn.
const MAX_TURN_STEPS: usize = 400;
/// Child work that arrived before its child row (bounded, oldest dropped).
const MAX_PENDING: usize = 32;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    Read,
    Edit,
    Command,
    Tool,
    Agent,
    Skill,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ItemState {
    Running,
    Completed,
    Failed,
    Stopped,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityItem {
    pub id: String,
    pub kind: ItemKind,
    pub label: String,
    pub state: ItemState,
    pub model: Option<String>,
    /// What the step touched: a path, the first command line, a query or a skill name (bounded).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Reply length (UTF-16 units) when the step first appeared, so the transcript interleaves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    /// Commands only: the end of their output (bounded, ANSI stripped), shown when the row opens.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    /// Set when the full output is stored apart (ADR-147): `output` then holds only its last
    /// lines, and this is the full output's size in bytes, fetched when the row opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<u32>,
    /// A picture the step's result carried (a screenshot), saved by tool_images.rs. Only its
    /// path is kept: the image bytes never reach the activity or a transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_path: Option<String>,
    /// An MCP tool's own name (`computer_click`), whatever the row's detail holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// What a browser or computer tool acted on, from `tool_target`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Child rows only: native observation window, never inferred from the parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    /// Child rows only: the child's own observed work, with generic labels like the parent's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<ActivityStep>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hidden_steps: u32,
    /// Child rows only: a Claude subagent launched with `run_in_background` (ADR-097). The turn
    /// keeps its process while it runs; one still running when the turn ends is `stopped`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
    /// Background child stopped because it outlived the turn's background limit (ADR-097).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
    /// A background child (or its finished marker) that is a shell command the main agent ran
    /// with `run_in_background`, not a subagent; its `Bash` call is already a row of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shell: bool,
    /// A marker where a background subagent finished (ADR-101): `label` names it, `state` is
    /// its outcome, `offset` the reply length then; the reply after it is its own answer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub finished_task: bool,
    /// Live reasoning signal only (ADR-101); never kept as a row.
    #[serde(skip)]
    pub thought: Option<Thought>,
    /// Signal only: the provider took the turn's prompt, so a handoff recap it carried is spent.
    #[serde(skip)]
    pub delivered: bool,
    /// Signal only: the agent's whole to-do plan as of now (ADR-182). An empty list clears it.
    #[serde(skip)]
    pub plan: Option<Vec<PlanEntry>>,
    /// The native delegation call that owns this child (Claude tool use): routes live updates
    /// and lets the person send that subagent to the background (ADR-111).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Observation routing only: this work belongs to the child identified by `parent`.
    #[serde(skip)]
    pub parent: Option<String>,
    /// Child rows only: a Claude workflow's phases and members (ADR-115).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<crate::workflow::Workflow>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityStep {
    pub id: String,
    pub kind: ItemKind,
    pub label: String,
    pub state: ItemState,
}
fn is_zero(v: &u32) -> bool {
    *v == 0
}
fn settled(state: &ItemState) -> bool {
    matches!(
        state,
        ItemState::Completed | ItemState::Failed | ItemState::Stopped
    )
}
/// A finished child or turn does not establish success for work still shown as running.
fn close_steps(steps: &mut [ActivityStep], stopped: bool) {
    for step in steps {
        if step.state == ItemState::Running {
            step.state = if stopped {
                ItemState::Stopped
            } else {
                ItemState::Unknown
            };
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnActivity {
    pub provider: AgentProviderId,
    pub model: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub waiting_since: Option<i64>,
    pub paused_ms: i64,
    pub status: SessionStatus,
    pub items: Vec<ActivityItem>,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<crate::turn_review::TurnReview>,
    /// First sentence (at most 140 characters) of the agent's latest reasoning while the turn
    /// runs (ADR-101). Sent to the renderer, left out of transcript files, cleared at the end.
    #[serde(default, skip_deserializing, skip_serializing_if = "skip_thought")]
    pub thought: Option<String>,
    /// The agent's latest to-do plan this turn (ADR-182): the whole list on every update.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan: Vec<PlanEntry>,
    #[serde(skip)]
    pending: Vec<ActivityItem>,
    /// The open reasoning block: what was read so far, and whether its first sentence is known.
    #[serde(skip)]
    thinking: Option<(String, bool)>,
}
impl TurnActivity {
    pub fn new(provider: AgentProviderId, model: Option<String>) -> Self {
        Self {
            provider,
            model,
            started_at: now(),
            ended_at: None,
            waiting_since: None,
            paused_ms: 0,
            status: SessionStatus::Starting,
            items: vec![],
            truncated: false,
            review: None,
            thought: None,
            plan: vec![],
            pending: vec![],
            thinking: None,
        }
    }
    pub(crate) fn sync_at(&mut self, status: SessionStatus, at: i64) {
        let at = at.max(self.started_at);
        if self.ended_at.is_some() {
            return;
        }
        if status == SessionStatus::Waiting {
            self.waiting_since.get_or_insert(at);
        } else if let Some(start) = self.waiting_since.take() {
            self.paused_ms = self
                .paused_ms
                .saturating_add(at.saturating_sub(start).max(0));
        }
        if !status.is_active() {
            self.ended_at = Some(at);
            // A parent finishing does NOT establish success for an unfinished child/tool.
            let stopped = status == SessionStatus::Stopped;
            for item in &mut self.items {
                if item.state == ItemState::Running {
                    // A background child dies with the turn's process: it was interrupted (ADR-097).
                    item.state = if stopped || item.background {
                        ItemState::Stopped
                    } else {
                        ItemState::Unknown
                    };
                }
                if item.kind == ItemKind::Agent {
                    item.ended_at.get_or_insert(at);
                    close_steps(&mut item.steps, stopped || item.background);
                }
                // A workflow member's answer so far is live only (ADR-115).
                if let Some(workflow) = item.workflow.as_mut() {
                    for agent in &mut workflow.agents {
                        agent.partial = None;
                    }
                }
            }
            self.pending.clear();
            // Reasoning is live only: nothing of it outlives the turn.
            self.thought = None;
            self.thinking = None;
        }
        self.status = status;
    }
    /// Reads a reasoning signal; true when the shown thought changed.
    fn think(&mut self, signal: Thought) -> bool {
        let text = match signal {
            Thought::Start => {
                self.thinking = Some((String::new(), false));
                return false;
            }
            Thought::End => {
                self.thinking = None;
                return self.thought.take().is_some();
            }
            Thought::Text { text, fresh } => {
                if fresh {
                    self.thinking = None;
                }
                let (buffer, known) = self.thinking.get_or_insert_with(Default::default);
                if *known {
                    return false;
                }
                buffer.push_str(head(&text, THOUGHT_BUFFER.saturating_sub(buffer.len())));
                let (line, complete) = thought_line(buffer);
                *known = complete || fresh || buffer.len() >= THOUGHT_BUFFER;
                line
            }
        };
        let text = crate::redact::mask(&text).into_owned();
        if text.is_empty() || self.thought.as_deref() == Some(text.as_str()) {
            return false;
        }
        self.thought = Some(text);
        true
    }
    /// Replaces the plan (bounded, credentials masked); true when it changed.
    fn set_plan(&mut self, plan: Vec<PlanEntry>) -> bool {
        let plan: Vec<PlanEntry> = plan
            .into_iter()
            .take(MAX_PLAN)
            .map(|entry| PlanEntry {
                text: short(&crate::redact::mask(&entry.text), MAX_PLAN_TEXT),
                status: entry.status,
            })
            .collect();
        if plan == self.plan {
            return false;
        }
        self.plan = plan;
        true
    }
    pub fn observe(&mut self, mut item: ActivityItem) -> bool {
        if self.ended_at.is_some() {
            return false;
        }
        if let Some(signal) = item.thought.take() {
            return self.think(signal);
        }
        if let Some(plan) = item.plan.take() {
            return self.set_plan(plan);
        }
        if item.delivered || item.id.is_empty() || item.id.len() > 256 {
            return false;
        }
        // Work closes the open reasoning block; the next reasoning is a new thought.
        if item.parent.is_none() {
            self.thinking = None;
        }
        item.label = short(&item.label, 100);
        // Credentials are masked before anything is kept (ADR-077).
        item.detail = detail(&crate::redact::mask(head(&item.detail, 4096)));
        item.output = output(&crate::redact::mask(tail(&item.output, 16_384)));
        item.model = item
            .model
            .as_deref()
            .map(|v| short(v, 100))
            .filter(|v| !v.is_empty());
        if let Some(parent) = item.parent.take() {
            return self.observe_step(parent, item);
        }
        // The delegation call that owns a child row is that child, not a separate tool row.
        if item.kind != ItemKind::Agent {
            if let Some(child) = self.items.iter_mut().find(|child| {
                child.kind == ItemKind::Agent && child.source.as_deref() == Some(item.id.as_str())
            }) {
                if child.model.is_none() && item.model.is_some() {
                    child.model = item.model;
                    return true;
                }
                return false;
            }
        }
        if let Some(index) = self.items.iter().position(|old| old.id == item.id) {
            let old = &self.items[index];
            // Update envelopes can omit metadata; retain known nickname/model.
            if item.label.is_empty() {
                item.label.clone_from(&old.label);
                item.kind = old.kind.clone();
            }
            if item.kind != ItemKind::Agent
                && old.state != ItemState::Running
                && old.state != ItemState::Unknown
                && item.state == ItemState::Running
            {
                return false;
            }
            if item.state == ItemState::Unknown {
                item.state = old.state.clone();
            }
            if item.model.is_none() {
                item.model.clone_from(&old.model);
            }
            if item.detail.is_empty() {
                item.detail.clone_from(&old.detail);
            }
            if item.image_path.is_none() {
                item.image_path.clone_from(&old.image_path);
            }
            if item.tool.is_none() {
                item.tool.clone_from(&old.tool);
            }
            if item.target.is_none() {
                item.target.clone_from(&old.target);
            }
            if item.output.is_empty() {
                item.output.clone_from(&old.output);
            }
            // Only command rows keep output; a result frame learns its kind from the row it updates.
            if item.kind != ItemKind::Command {
                item.output.clear();
            }
            // A row keeps the place it first appeared in the reply.
            item.offset = old.offset.or(item.offset);
            if item.source.is_none() {
                item.source.clone_from(&old.source);
            }
            item.steps.clone_from(&old.steps);
            item.hidden_steps = old.hidden_steps;
            item.workflow = match (old.workflow.as_ref(), item.workflow.take()) {
                (Some(known), Some(next)) => Some(crate::workflow::merge(known, next)),
                (known, None) => known.cloned(),
                (None, next) => next,
            };
            item.background |= old.background;
            item.timed_out |= old.timed_out;
            item.shell |= old.shell;
            item.finished_task |= old.finished_task;
            item.started_at = old.started_at;
            if item.kind == ItemKind::Agent {
                // Children can work again; only a settled native state closes the window.
                item.ended_at = settled(&item.state).then(|| old.ended_at.unwrap_or_else(now));
                if settled(&item.state) {
                    close_steps(&mut item.steps, item.state == ItemState::Stopped);
                }
            }
            if *old == item {
                return false;
            }
            self.items[index] = item;
        } else if self.items.len() < MAX_ITEMS {
            if item.kind == ItemKind::Agent {
                let at = now();
                item.started_at = Some(at);
                item.ended_at = settled(&item.state).then_some(at);
                if let Some(source) = item.source.clone() {
                    if let Some(index) = self.items.iter().position(|old| old.id == source) {
                        let call = self.items.remove(index);
                        if item.model.is_none() {
                            item.model = call.model;
                        }
                    }
                }
                self.items.push(item);
                // Replay child work that arrived before this child row.
                for early in std::mem::take(&mut self.pending) {
                    self.observe(early);
                }
            } else {
                if item.kind != ItemKind::Command {
                    item.output.clear();
                }
                self.items.push(item);
            }
        } else {
            let changed = !self.truncated;
            self.truncated = true;
            return changed;
        }
        true
    }
    fn observe_step(&mut self, parent: String, mut item: ActivityItem) -> bool {
        let retained: usize = self.items.iter().map(|child| child.steps.len()).sum();
        let Some(child) = self.items.iter_mut().find(|child| {
            child.kind == ItemKind::Agent
                && (child.id == parent || child.source.as_deref() == Some(parent.as_str()))
        }) else {
            if self.pending.len() >= MAX_PENDING {
                self.pending.remove(0);
            }
            item.parent = Some(parent);
            self.pending.push(item);
            return false;
        };
        // Nested delegation inside a child is shown as one of its steps, never as a new row.
        let (kind, label) = if item.kind == ItemKind::Agent {
            (ItemKind::Tool, "Delegate task".to_owned())
        } else {
            (item.kind, item.label)
        };
        if let Some(step) = child.steps.iter_mut().find(|step| step.id == item.id) {
            let mut next = step.clone();
            if !label.is_empty() {
                next.label = label;
                next.kind = kind;
            }
            if item.state != ItemState::Unknown
                && !(settled(&step.state) && item.state == ItemState::Running)
            {
                next.state = item.state;
            }
            if *step == next {
                return false;
            }
            *step = next;
            return true;
        }
        // Completions for steps never seen (or already folded away) are not new work.
        if label.is_empty() || (item.state != ItemState::Running && child.hidden_steps > 0) {
            return false;
        }
        if settled(&child.state) {
            return false;
        }
        if retained >= MAX_TURN_STEPS {
            child.hidden_steps = child.hidden_steps.saturating_add(1);
            return true;
        }
        if child.steps.len() >= MAX_CHILD_STEPS {
            child.steps.remove(0);
            child.hidden_steps = child.hidden_steps.saturating_add(1);
        }
        child.steps.push(ActivityStep {
            id: item.id,
            kind,
            label,
            state: item.state,
        });
        true
    }
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
/// First line of a native detail, without control characters, at most 160 characters.
fn detail(v: &str) -> String {
    short(
        v.lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim(),
        160,
    )
}
/// The end of a command's output: ANSI codes and other control characters removed, at most
/// the last 60 lines and 3,000 characters.
pub(crate) fn output(v: &str) -> String {
    let mut clean = String::with_capacity(v.len().min(16_384));
    let mut chars = v.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI sequences end at a letter; others are one character long.
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if c == '\n' || c == '\t' || !c.is_control() {
            clean.push(c);
        }
    }
    let trimmed = clean.trim_end();
    let lines: Vec<&str> = trimmed.lines().collect();
    let tail = lines[lines.len().saturating_sub(60)..].join("\n");
    let count = tail.chars().count();
    if count > 3000 {
        tail.chars().skip(count - 3000).collect()
    } else {
        tail
    }
}
/// The first `limit` bytes of `v` on a character boundary.
fn head(v: &str, limit: usize) -> &str {
    let mut end = v.len().min(limit);
    while !v.is_char_boundary(end) {
        end -= 1;
    }
    &v[..end]
}
/// The last `limit` bytes of `v` on a character boundary, so masking scans bounded text.
fn tail(v: &str, limit: usize) -> &str {
    let mut start = v.len().saturating_sub(limit);
    while !v.is_char_boundary(start) {
        start += 1;
    }
    &v[start..]
}
/// A secret the person provided privately: a settled row naming its label only (ADR-077).
pub fn secret_row(request_id: &str, label: &str) -> ActivityItem {
    let mut row = item(
        format!("secret:{}", short(request_id, 64)),
        ItemKind::Tool,
        crate::secrets::PROVIDED_LABEL,
        ItemState::Completed,
        None,
    );
    row.detail = short(label, 80);
    row
}
/// A skill the prompt invoked: a settled row at the start of the turn.
pub fn skill(name: &str) -> ActivityItem {
    let mut row = item(
        format!("skill:{}", short(name, 64)),
        ItemKind::Skill,
        "Skill",
        ItemState::Completed,
        None,
    );
    row.detail = short(name, 64);
    row.offset = Some(0);
    row
}
fn short(v: &str, n: usize) -> String {
    v.chars().filter(|c| !c.is_control()).take(n).collect()
}
fn identifier(v: &Value) -> Option<String> {
    v.as_str()
        .filter(|v| !v.is_empty() && v.len() <= 192)
        .map(str::to_owned)
}
fn item(
    id: String,
    kind: ItemKind,
    label: &str,
    state: ItemState,
    model: Option<&str>,
) -> ActivityItem {
    ActivityItem {
        id,
        kind,
        label: short(label, 100),
        state,
        model: model.map(|v| short(v, 100)),
        detail: String::new(),
        offset: None,
        output: String::new(),
        output_bytes: None,
        image_path: None,
        tool: None,
        target: None,
        started_at: None,
        ended_at: None,
        steps: vec![],
        hidden_steps: 0,
        background: false,
        timed_out: false,
        shell: false,
        finished_task: false,
        thought: None,
        delivered: false,
        plan: None,
        source: None,
        parent: None,
        workflow: None,
    }
}
/// The provider took the turn's prompt (ADR-101): a handoff recap it carried is spent.
pub fn delivered() -> ActivityItem {
    let mut row = item(
        "delivered".into(),
        ItemKind::Tool,
        "",
        ItemState::Unknown,
        None,
    );
    row.delivered = true;
    row
}
/// Where a background subagent or shell finished (ADR-101): its name and outcome at the reply's length.
pub fn background_finished(
    task_id: &str,
    label: &str,
    status: &Value,
    shell: bool,
) -> ActivityItem {
    let mut row = item(
        format!("done:{}", short(task_id, 192)),
        ItemKind::Tool,
        match (label.trim().is_empty(), shell) {
            (false, _) => label,
            (true, false) => "Subagent",
            (true, true) => "Command",
        },
        native_state(status),
        None,
    );
    row.finished_task = true;
    row.shell = shell;
    row
}
/// A live reasoning signal carried on the activity channel (ADR-101).
fn thought(signal: Thought) -> ActivityItem {
    let mut row = item(
        "thought".into(),
        ItemKind::Tool,
        "",
        ItemState::Unknown,
        None,
    );
    row.thought = Some(signal);
    row
}
/// The first sentence of reasoning, as the live row shows it: a bold opening line (a
/// Codex summary heading) wins; markdown marks are dropped; at most 140 characters. The flag
/// says whether that sentence is complete, so later text cannot change it.
pub fn thought_line(markdown: &str) -> (String, bool) {
    static HEADING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^\s*\*\*([^*\r\n]+)\*\*[ \t]*\r?(\n|$)").unwrap()
    });
    static LINK: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").unwrap());
    static MARKER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?m)^[ \t]*(?:#{1,6}|[-*+]|\d+\.)[ \t]+").unwrap()
    });
    static MARKS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"`+|\*\*|~~|__").unwrap());
    static END: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"[.?!]["'”’)]?\s"#).unwrap());
    let heading = HEADING.captures(markdown);
    let source = heading
        .as_ref()
        .and_then(|captures| captures.get(1))
        .map_or(markdown, |found| found.as_str());
    let text = LINK.replace_all(source, "$1");
    let text = MARKER.replace_all(&text, "");
    let text = MARKS.replace_all(&text, "");
    let text = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('*', "");
    let (sentence, complete) = match (heading.is_some(), END.find(&text)) {
        (true, _) => (text.clone(), true),
        // A sentence end needs the space after it, so a streamed "3." is not cut early.
        (false, Some(end)) => (text[..end.end()].trim().to_owned(), true),
        _ => (text.clone(), false),
    };
    let count = sentence.chars().count();
    if count > THOUGHT_CHARS {
        let cut: String = sentence.chars().take(THOUGHT_CHARS - 1).collect();
        return (format!("{}…", cut.trim_end()), true);
    }
    (short(&sentence, THOUGHT_CHARS), complete)
}
fn native_state(v: &Value) -> ItemState {
    match v.as_str() {
        Some("running" | "inProgress" | "in_progress" | "pending") => ItemState::Running,
        Some("completed" | "succeeded") => ItemState::Completed,
        Some("failed" | "errored" | "error") => ItemState::Failed,
        Some("interrupted" | "cancelled" | "killed" | "stopped" | "shutdown") => ItemState::Stopped,
        _ => ItemState::Unknown,
    }
}
pub fn sync(session: &mut Session) {
    let status = session.status.clone();
    session.last_activity_at = chrono::Utc::now().to_rfc3339();
    if let Some(activity) = current(session) {
        activity.sync_at(status.clone(), now());
    }
    // A turn that ended keeps or spends the handoff recap it carried (ADR-101).
    if !status.is_active() {
        crate::transcript::settle_handoff(session);
    }
}
fn current(session: &mut Session) -> Option<&mut TurnActivity> {
    // Only the newest agent message and later rows count as changed (checkpoints).
    session
        .messages
        .rev_find_mut(|m| m.role == MessageRole::Agent)?
        .activity
        .as_mut()
}
/// An interrupted restart closes at the last observed native timestamp, never at reopen time.
pub fn recover(session: &mut Session) {
    let status = session.status.clone();
    let at = chrono::DateTime::parse_from_rfc3339(&session.last_activity_at)
        .ok()
        .map(|v| v.timestamp_millis());
    if let Some(activity) = current(session) {
        activity.sync_at(status, at.unwrap_or(activity.started_at));
    }
}
pub fn observe(session: &mut Session, observation: ActivityItem) -> bool {
    if observation.delivered {
        crate::transcript::handoff_delivered(session);
        return false;
    }
    // Claude completion frames can describe shell tasks; only an observed local_agent start owns a child row.
    if session.agent == AgentProviderId::Claude
        && observation.kind == ItemKind::Agent
        && observation.label.is_empty()
        && !current(session).is_some_and(|a| a.items.iter().any(|i| i.id == observation.id))
    {
        return false;
    }
    let mut observation = observation;
    if let Some(Thought::Text { text, .. }) = observation.thought.as_mut() {
        *text = crate::secrets::scrub(&session.id, std::mem::take(text));
    }
    if let Some(plan) = observation.plan.as_mut() {
        for entry in plan {
            entry.text = crate::secrets::scrub(&session.id, std::mem::take(&mut entry.text));
        }
    }
    // A value the person provided privately this turn never lands in a row.
    observation.detail =
        crate::secrets::scrub(&session.id, std::mem::take(&mut observation.detail));
    observation.output =
        crate::secrets::scrub(&session.id, std::mem::take(&mut observation.output));
    // A screenshot's picture, saved where the tool produced it (Codex truncates the result text,
    // so the row's own result cannot carry it). The tool name is the row's detail.
    if observation.kind == ItemKind::Tool
        && observation.image_path.is_none()
        && observation.state == ItemState::Completed
    {
        let known = current(session)
            .and_then(|a| a.items.iter().find(|i| i.id == observation.id))
            .map(|i| (i.detail.clone(), i.image_path.is_some()));
        let tool = if observation.detail.is_empty() {
            known.clone().map(|(detail, _)| detail).unwrap_or_default()
        } else {
            observation.detail.clone()
        };
        if crate::tool_images::is_image_tool(&tool) && !known.is_some_and(|(_, has)| has) {
            observation.image_path = crate::tool_images::take(&session.id, &tool);
        }
    }
    // The reply so far, in the UTF-16 units the transcript slices by.
    let reply = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::Agent)
        .map(|m| m.content.encode_utf16().count() as u32);
    observation.offset = observation.offset.or(reply);
    current(session).is_some_and(|activity| activity.observe(observation))
}
pub fn model(session: &mut Session, model: &str) {
    if let Some(activity) = current(session).filter(|a| a.ended_at.is_none()) {
        if !model.is_empty() && model.len() <= 256 {
            activity.model = Some(short(model, 100));
        }
    }
}
/// A provider's status word as a plan status: `in_progress`, `inProgress` and `active` are the
/// current step, `completed` and `done` are finished; anything else (including a cancelled
/// OpenCode todo) is neither done nor being worked on.
fn plan_status(entry: &Value) -> PlanStatus {
    let word = entry["status"]
        .as_str()
        .map(|v| {
            v.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    match word.as_str() {
        "inprogress" | "active" | "running" => PlanStatus::InProgress,
        "completed" | "complete" | "done" | "finished" => PlanStatus::Completed,
        "" if entry["completed"] == true || entry["done"] == true => PlanStatus::Completed,
        _ => PlanStatus::Pending,
    }
}
/// The shared shape of every provider's plan: a list of `{text-ish, status}` entries (Claude
/// `TodoWrite` `todos`, Codex `plan`, ACP `entries`, OpenCode `todowrite`, Cursor todos).
fn plan_entries(list: &Value) -> Option<Vec<PlanEntry>> {
    let list = list.as_array()?;
    Some(
        list.iter()
            .take(MAX_PLAN)
            .filter_map(|entry| {
                let text = [
                    "content",
                    "step",
                    "text",
                    "title",
                    "description",
                    "activeForm",
                ]
                .iter()
                .find_map(|key| entry[*key].as_str().filter(|t| !t.trim().is_empty()))
                .or_else(|| entry.as_str())?;
                Some(PlanEntry {
                    text: text.trim().to_owned(),
                    status: plan_status(entry),
                })
            })
            .collect(),
    )
}
fn plan_signal(list: &Value) -> Option<ActivityItem> {
    let mut row = item("plan".into(), ItemKind::Tool, "", ItemState::Unknown, None);
    row.plan = Some(plan_entries(list)?);
    Some(row)
}
/// Codex `turn/plan/updated` (`update_plan`): the whole plan for the turn.
pub fn codex_plan(params: &Value) -> Option<ActivityItem> {
    plan_signal(&params["plan"])
}
/// Caller already checked parent thread AND active turn. Ignore all raw args and output; a
/// finished reasoning item only feeds the live thought (ADR-101).
pub fn codex(v: &Value, started: bool) -> Vec<ActivityItem> {
    let Some(id) = identifier(&v["id"]) else {
        return vec![];
    };
    match v["type"].as_str() {
        Some("reasoning") if !started => {
            // The latest summary paragraph, else the raw reasoning, when the provider shares it.
            let last = |key: &str| {
                v[key]
                    .as_array()?
                    .iter()
                    .rev()
                    .filter_map(Value::as_str)
                    .find(|text| !text.trim().is_empty())
                    .map(str::to_owned)
            };
            return last("summary")
                .or_else(|| last("content"))
                .map(|text| vec![thought(Thought::Text { text, fresh: true })])
                .unwrap_or_default();
        }
        Some("agentMessage") if started => return vec![thought(Thought::End)],
        Some("todoList") => return plan_signal(&v["items"]).into_iter().collect(),
        _ => {}
    }
    let state = if started {
        ItemState::Running
    } else {
        native_state(&v["status"])
    };
    match v["type"].as_str() {
        Some("commandExecution") => {
            let read = v["commandActions"].as_array().is_some_and(|a| {
                !a.is_empty()
                    && a.iter().all(|x| {
                        matches!(x["type"].as_str(), Some("read" | "listFiles" | "search"))
                    })
            });
            let mut row = item(
                id,
                if read {
                    ItemKind::Read
                } else {
                    ItemKind::Command
                },
                if read { "Read / search" } else { "Command" },
                if !started && v["exitCode"].as_i64().is_some_and(|c| c != 0) {
                    ItemState::Failed
                } else {
                    state
                },
                None,
            );
            // A read names its file or query; a command keeps its own first line.
            row.detail = v["commandActions"][0]["path"]
                .as_str()
                .or_else(|| v["commandActions"][0]["query"].as_str())
                .filter(|_| read)
                .or_else(|| v["command"].as_str())
                .unwrap_or("")
                .to_owned();
            row.output = v["aggregatedOutput"].as_str().unwrap_or("").to_owned();
            vec![row]
        }
        Some("fileChange") => {
            let mut row = item(id, ItemKind::Edit, "File changes", state, None);
            row.detail = v["changes"][0]["path"].as_str().unwrap_or("").to_owned();
            vec![row]
        }
        Some("mcpToolCall" | "dynamicToolCall") => {
            let mut row = item(id, ItemKind::Tool, "Tool call", state, None);
            row.detail = v["tool"].as_str().unwrap_or("").to_owned();
            if !row.detail.is_empty() {
                row.tool = Some(row.detail.clone());
                row.target = tool_target(&row.detail, &v["arguments"]);
            }
            if !started {
                row.image_path = result_image(&v["result"]["content"]);
            }
            vec![row]
        }
        Some("webSearch") => {
            let mut row = item(
                id,
                ItemKind::Read,
                "Web search",
                if started { state } else { ItemState::Completed },
                None,
            );
            row.detail = v["query"].as_str().unwrap_or("").to_owned();
            vec![row]
        }
        Some("subAgentActivity") => {
            let Some(child) = identifier(&v["agentThreadId"]) else {
                return vec![];
            };
            let state = match v["kind"].as_str() {
                Some("started") => ItemState::Running,
                Some("completed") => ItemState::Completed,
                Some("interrupted") => ItemState::Stopped,
                _ => return vec![],
            };
            vec![item(
                format!("agent:{child}"),
                ItemKind::Agent,
                v["agentPath"].as_str().unwrap_or(""),
                state,
                None,
            )]
        }
        Some("collabAgentToolCall") => {
            let mut result = vec![];
            // An actual successful spawn may offer an identity before child lifecycle arrives.
            // The collaboration call finishing does not prove the child is running/completed.
            if !started && v["tool"] == "spawnAgent" && v["status"] == "completed" {
                if let Some(receivers) = v["receiverThreadIds"].as_array().filter(|a| a.len() == 1)
                {
                    if let Some(child) = identifier(&receivers[0]) {
                        result.push(item(
                            format!("agent:{child}"),
                            ItemKind::Agent,
                            "",
                            ItemState::Unknown,
                            v["model"].as_str(),
                        ));
                    }
                }
            }
            // Wait/send bookkeeping does not create a new child. Only explicit child identities.
            if let Some(states) = v["agentsStates"].as_object() {
                for (child, data) in states.iter().take(MAX_ITEMS + 1) {
                    if child.is_empty() || child.len() > 192 {
                        continue;
                    }
                    let name = data["agentNickname"].as_str().unwrap_or("");
                    result.push(item(
                        format!("agent:{child}"),
                        ItemKind::Agent,
                        name,
                        native_state(&data["status"]),
                        data["model"].as_str().or_else(|| {
                            (v["tool"] == "spawnAgent" && states.len() == 1)
                                .then(|| v["model"].as_str())
                                .flatten()
                        }),
                    ));
                }
            }
            result
        }
        _ => vec![],
    }
}
/// What a Claude tool call touches, read from its input: a path, command, query or skill.
pub(crate) fn tool_input_detail(input: &Value) -> &str {
    [
        "file_path",
        "notebook_path",
        "command",
        "pattern",
        "url",
        "query",
        "skill",
        "path",
    ]
    .iter()
    .find_map(|key| input[*key].as_str())
    .unwrap_or("")
}
/// The tool's own name inside an MCP server's `mcp__<server>__<tool>` name (`browser_screenshot`).
fn mcp_tool(name: &str) -> Option<&str> {
    name.strip_prefix("mcp__")?
        .split_once("__")
        .map(|(_, tool)| tool)
        .filter(|tool| !tool.is_empty())
}
/// What a browser or computer tool acted on: the app, or the page without its query, which can
/// hold a token. No other argument reaches a row: typed text and the like stay out of it.
fn tool_target(tool: &str, arguments: &Value) -> Option<String> {
    let parsed;
    let arguments = match arguments.as_str() {
        Some(text) => {
            parsed = serde_json::from_str::<Value>(text).ok()?;
            &parsed
        }
        None => arguments,
    };
    let text = if tool.starts_with("computer_") {
        arguments["app"]
            .as_str()
            .or(arguments["bundleId"].as_str())?
    } else if matches!(tool, "browser_open" | "browser_navigate") {
        arguments["url"].as_str()?.split(['?', '#']).next()?
    } else {
        return None;
    };
    (!text.is_empty()).then(|| short(text, 100))
}
/// The first picture a tool result carries, as an MCP image block or Claude's base64 image
/// block, saved by tool_images.rs. Only its path comes back: the bytes are not kept.
fn result_image(blocks: &Value) -> Option<String> {
    blocks
        .as_array()?
        .iter()
        .filter(|block| block["type"] == "image")
        .find_map(|block| {
            let data = if block["source"]["type"] == "base64" {
                block["source"]["data"].as_str()
            } else {
                block["data"].as_str()
            };
            crate::tool_images::save(data?)
        })
}
pub(crate) fn tool_kind(name: &str) -> (ItemKind, &'static str) {
    match name {
        "Read" | "Glob" | "Grep" => (ItemKind::Read, "Read / search"),
        "Write" | "Edit" | "MultiEdit" => (ItemKind::Edit, "File changes"),
        "Bash" => (ItemKind::Command, "Command"),
        "WebFetch" | "WebSearch" => (ItemKind::Read, "Web search"),
        "Skill" => (ItemKind::Skill, "Skill"),
        "Agent" | "Task" => (ItemKind::Tool, "Delegate task"),
        _ => (ItemKind::Tool, "Tool call"),
    }
}
/// Parent tools, local agent tasks and their children's tool steps. Text and raw tool arguments
/// are excluded; only a delegation call's model choice is read. The main agent's streamed
/// thinking feeds the live thought only (ADR-101).
pub fn claude(v: &Value) -> Vec<ActivityItem> {
    // Child messages carry the delegation call that spawned them; their tools become that child's steps.
    let parent = v["parent_tool_use_id"].as_str();
    if parent.is_some() && identifier(&v["parent_tool_use_id"]).is_none() {
        return vec![];
    }
    if parent.is_none() && v["type"] == "stream_event" {
        let event = &v["event"];
        match (
            event["type"].as_str(),
            event["content_block"]["type"].as_str(),
        ) {
            (Some("content_block_start"), Some("thinking")) => {
                return vec![thought(Thought::Start)]
            }
            (Some("content_block_start"), Some("text")) => return vec![thought(Thought::End)],
            (Some("content_block_delta"), _) if event["delta"]["type"] == "thinking_delta" => {
                return event["delta"]["thinking"]
                    .as_str()
                    .filter(|text| !text.is_empty())
                    .map(|text| {
                        vec![thought(Thought::Text {
                            text: text.to_owned(),
                            fresh: false,
                        })]
                    })
                    .unwrap_or_default();
            }
            _ => {}
        }
    }
    if parent.is_none()
        && v["type"] == "system"
        && matches!(
            v["subtype"].as_str(),
            Some("task_started" | "task_notification" | "task_updated" | "task_progress")
        )
    {
        let Some(id) = identifier(&v["task_id"]) else {
            return vec![];
        };
        let workflow_start = v["subtype"] == "task_started" && v["task_type"] == "local_workflow";
        let shell_start = is_background_shell(v);
        if v["subtype"] == "task_started"
            && v["task_type"] != "local_agent"
            && !workflow_start
            && !shell_start
        {
            return vec![];
        }
        let state = if v["subtype"] == "task_started" {
            ItemState::Running
        } else if v["subtype"] == "task_progress" {
            ItemState::Unknown
        } else {
            native_state(v.get("status").unwrap_or(&v["patch"]["status"]))
        };
        let mut child = item(
            format!("agent:{id}"),
            ItemKind::Agent,
            if workflow_start {
                v["workflow_name"]
                    .as_str()
                    .or(v["description"].as_str())
                    .unwrap_or("Workflow")
            } else if shell_start {
                v["description"].as_str().unwrap_or("Command")
            } else if v["subtype"] == "task_started" {
                v["description"].as_str().unwrap_or("Subagent")
            } else {
                ""
            },
            state,
            None,
        );
        child.source = identifier(&v["tool_use_id"]);
        // A subagent launched with `run_in_background` by the main agent (ADR-097); a task owned
        // by another subagent ends with its owner.
        child.background = v["subtype"] == "task_started" && (is_background_task(v) || shell_start);
        child.shell = shell_start;
        // Progress names what the child is doing now ("Running npm test").
        if v["subtype"] == "task_progress" {
            child.detail = v["description"].as_str().unwrap_or("").to_owned();
        }
        // A workflow's phases and members (ADR-115); merged with what is known in `observe`.
        if workflow_start {
            child.workflow = Some(crate::workflow::Workflow {
                name: v["workflow_name"].as_str().map(str::to_owned),
                ..Default::default()
            });
        } else if v["subtype"] == "task_progress" {
            child.workflow = crate::workflow::snapshot(v);
        }
        return vec![child];
    }
    let content = if parent.is_none()
        && v["type"] == "stream_event"
        && v["event"]["type"] == "content_block_start"
    {
        vec![&v["event"]["content_block"]]
    } else if matches!(v["type"].as_str(), Some("assistant" | "user")) {
        v["message"]["content"]
            .as_array()
            .map(|a| a.iter().collect())
            .unwrap_or_default()
    } else {
        vec![]
    };
    // The main agent's whole to-do list (ADR-182); a subagent's stays its own. The streamed
    // block start carries no input yet, so only the complete message reads as a plan.
    let plans: Vec<ActivityItem> = if parent.is_none() {
        content
            .iter()
            .filter(|block| block["type"] == "tool_use" && block["name"] == "TodoWrite")
            .filter_map(|block| plan_signal(&block["input"]["todos"]))
            .collect()
    } else {
        vec![]
    };
    let mut rows: Vec<ActivityItem> = content
        .into_iter()
        .take(MAX_ITEMS)
        .filter_map(|block| {
            let mut observed = if block["type"] == "tool_use" {
                let name = block["name"].as_str()?;
                let (kind, label) = tool_kind(name);
                // The delegation's model choice names its child; of other inputs, only the row's
                // detail and `tool_target`'s app or page are read.
                let model = matches!(name, "Agent" | "Task")
                    .then(|| block["input"]["model"].as_str())
                    .flatten();
                let mut row = item(
                    identifier(&block["id"])?,
                    kind,
                    label,
                    ItemState::Running,
                    model,
                );
                row.detail = tool_input_detail(&block["input"]).to_owned();
                // An MCP tool's own name stands in when its input names nothing, as Codex's does.
                if row.detail.is_empty() {
                    row.detail = mcp_tool(name).unwrap_or("").to_owned();
                }
                if let Some(tool) = mcp_tool(name) {
                    row.tool = Some(tool.to_owned());
                    row.target = tool_target(tool, &block["input"]);
                }
                row
            } else if block["type"] == "tool_result" {
                let mut row = item(
                    identifier(&block["tool_use_id"])?,
                    ItemKind::Tool,
                    "",
                    if block["is_error"] == true {
                        ItemState::Failed
                    } else {
                        ItemState::Completed
                    },
                    None,
                );
                // Text content only; kept later only if the row is a command.
                row.output = match &block["content"] {
                    Value::String(text) => text.clone(),
                    Value::Array(parts) => parts
                        .iter()
                        .filter_map(|part| part["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                row.image_path = result_image(&block["content"]);
                row
            } else {
                return None;
            };
            observed.parent = parent.map(str::to_owned);
            Some(observed)
        })
        .collect();
    rows.extend(plans);
    rows
}
/// A step on a subagent's card: the person's message reached it (ADR-113).
pub fn subagent_message_delivered(task_id: &str) -> ActivityItem {
    let mut step = item(
        format!("steer:{}:{}", short(task_id, 120), now()),
        ItemKind::Tool,
        "Message delivered",
        ItemState::Completed,
        None,
    );
    step.parent = Some(format!("agent:{}", short(task_id, 192)));
    step
}
/// A `task_started` frame for a Claude workflow run (ADR-115): it runs on after the call
/// returns, so the turn keeps its process while it does, like a background subagent.
pub(crate) fn is_workflow_task(v: &Value) -> bool {
    v["subtype"] == "task_started"
        && v["task_type"] == "local_workflow"
        && v["parent_tool_use_id"].is_null()
        && identifier(&v["task_id"]).is_some()
}
/// A `task_started` frame for a subagent the main agent launched with `run_in_background`.
pub(crate) fn is_background_task(v: &Value) -> bool {
    v["subtype"] == "task_started"
        && v["task_type"] == "local_agent"
        && v["is_backgrounded"] == true
        && v["parent_task_id"].is_null()
        && v["parent_tool_use_id"].is_null()
        && identifier(&v["task_id"]).is_some()
}
/// A `task_started` frame for a shell command the main agent ran with `run_in_background`.
/// The CLI reports it like a background subagent and runs a turn of its own when it ends.
pub(crate) fn is_background_shell(v: &Value) -> bool {
    v["subtype"] == "task_started"
        && v["task_type"] == "local_bash"
        && v["is_backgrounded"] == true
        && v["parent_task_id"].is_null()
        && v["parent_tool_use_id"].is_null()
        && identifier(&v["task_id"]).is_some()
}
/// A background child the turn stopped at its limit: interrupted, and marked as timed out.
pub fn background_timed_out(task_id: &str) -> ActivityItem {
    let mut row = item(
        format!("agent:{}", short(task_id, 192)),
        ItemKind::Agent,
        "",
        ItemState::Stopped,
        None,
    );
    row.timed_out = true;
    row
}
/// Items from a child thread the turn spawned become that child's steps; nested children stay inside it.
pub fn codex_child(v: &Value, started: bool, thread: &str) -> Vec<ActivityItem> {
    let Some(thread) = identifier(&Value::from(thread)) else {
        return vec![];
    };
    codex(v, started)
        .into_iter()
        .filter(|observed| observed.kind != ItemKind::Agent && observed.thought.is_none())
        .map(|mut observed| {
            observed.parent = Some(format!("agent:{thread}"));
            observed
        })
        .collect()
}
pub fn opencode(v: &Value) -> Vec<ActivityItem> {
    // Thought chunks feed the live thought only (ADR-101); the answer starting ends it.
    match v["sessionUpdate"].as_str() {
        Some("agent_thought_chunk") => {
            return v["content"]["text"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(|text| {
                    vec![thought(Thought::Text {
                        text: text.to_owned(),
                        fresh: false,
                    })]
                })
                .unwrap_or_default();
        }
        Some("agent_message_chunk") => return vec![thought(Thought::End)],
        // ACP plan updates carry the whole list as `entries` (Gemini, Cursor, Devin, ...).
        Some("plan") => return plan_signal(&v["entries"]).into_iter().collect(),
        _ => {}
    }
    if !matches!(
        v["sessionUpdate"].as_str(),
        Some("tool_call" | "tool_call_update")
    ) {
        return vec![];
    }
    let Some(id) = identifier(&v["toolCallId"]) else {
        return vec![];
    };
    let (kind, label) = match v["kind"].as_str() {
        Some("read" | "search" | "fetch") => (ItemKind::Read, "Read / search"),
        Some("edit" | "delete" | "move") => (ItemKind::Edit, "File changes"),
        Some("execute") => (ItemKind::Command, "Command"),
        // OpenCode reports its `task` delegation as `think`. Only the short description names the
        // child; its prompt is never read, and ACP does not forward the child's own tools.
        Some("think") => (
            ItemKind::Agent,
            v["rawInput"]["description"]
                .as_str()
                .filter(|d| !d.trim().is_empty())
                .unwrap_or("Subagent"),
        ),
        _ => (ItemKind::Tool, "Tool call"),
    };
    let mut row = item(
        id,
        kind,
        if v["sessionUpdate"] == "tool_call_update" && v["kind"].is_null() {
            ""
        } else {
            label
        },
        native_state(&v["status"]),
        None,
    );
    // ACP titles name the target ("Read src/app.ts"); the first location is the file itself.
    row.output = v["rawOutput"]["output"]
        .as_str()
        .or_else(|| v["content"][0]["content"]["text"].as_str())
        .unwrap_or("")
        .to_owned();
    row.detail = v["locations"][0]["path"]
        .as_str()
        .or_else(|| v["title"].as_str())
        .unwrap_or("")
        .to_owned();
    // OpenCode's `todowrite` and Cursor's todo updates carry the list in the call's input.
    let plan = plan_signal(&v["rawInput"]["todos"]);
    std::iter::once(row).chain(plan).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_target_is_only_the_app_or_the_page_without_its_query() {
        assert_eq!(
            tool_target("computer_click", &json!({"app":"Calculator","ref":"e1"})).as_deref(),
            Some("Calculator")
        );
        assert_eq!(
            tool_target(
                "computer_type",
                &json!("{\"app\":\"Notes\",\"text\":\"secret\"}")
            )
            .as_deref(),
            Some("Notes")
        );
        assert_eq!(
            tool_target("browser_open", &json!({"url":"https://a.com/x?token=1#y"})).as_deref(),
            Some("https://a.com/x")
        );
        assert_eq!(
            tool_target(
                "computer_launch",
                &json!({"bundleId":"com.apple.calculator"})
            )
            .as_deref(),
            Some("com.apple.calculator")
        );
        assert_eq!(tool_target("browser_type", &json!({"text":"secret"})), None);
        assert_eq!(
            tool_target("computer_type", &json!({"text":"secret"})),
            None
        );
    }
    use serde_json::json;
    #[test]
    fn clock_pauses_and_finalization_does_not_invent_success() {
        let mut a = TurnActivity::new(AgentProviderId::Codex, None);
        a.started_at = 100;
        a.observe(item(
            "x".into(),
            ItemKind::Agent,
            "Worker",
            ItemState::Running,
            None,
        ));
        a.sync_at(SessionStatus::Waiting, 200);
        a.sync_at(SessionStatus::Waiting, 250);
        a.sync_at(SessionStatus::Running, 400);
        a.sync_at(SessionStatus::Completed, 500);
        assert_eq!(a.paused_ms, 200);
        assert_eq!(a.ended_at, Some(500));
        assert_eq!(a.items[0].state, ItemState::Unknown);
        a.sync_at(SessionStatus::Stopped, 900);
        assert_eq!(a.ended_at, Some(500));
    }
    #[test]
    fn updates_dedupe_keep_metadata_and_bound_retention() {
        let mut a = TurnActivity::new(AgentProviderId::Claude, None);
        a.observe(item(
            "x".into(),
            ItemKind::Read,
            "Read",
            ItemState::Running,
            Some("model"),
        ));
        a.observe(item(
            "x".into(),
            ItemKind::Tool,
            "",
            ItemState::Completed,
            None,
        ));
        assert_eq!(a.items.len(), 1);
        assert_eq!(a.items[0].label, "Read");
        for n in 0..200 {
            a.observe(item(
                n.to_string(),
                ItemKind::Tool,
                "Call",
                ItemState::Running,
                None,
            ));
        }
        assert_eq!(a.items.len(), MAX_ITEMS);
        assert!(a.truncated);
    }
    #[test]
    fn parses_observed_child_states_without_wait_phantom_or_raw_arguments() {
        assert!(codex(
            &json!({"id":"wait","type":"collabAgentToolCall","receiverThreadIds":["x"]}),
            false
        )
        .is_empty());
        let entries = codex(
            &json!({"id":"wait","type":"collabAgentToolCall","model":"m","prompt":"secret","agentsStates":{"x":{"status":"completed","agentNickname":"Review","message":"private"},"y":{"status":"errored"}}}),
            false,
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].state, ItemState::Completed);
        let encoded = serde_json::to_string(&entries).unwrap();
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("private"));
        assert!(codex(&json!({"id":"r","type":"reasoning","text":"hidden"}), true).is_empty());
    }
    #[test]
    fn claude_top_level_tools_and_local_tasks_only() {
        let v = json!({"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"secret"}}}});
        assert_eq!(claude(&v)[0].kind, ItemKind::Read);
        let mut child = v;
        child["parent_tool_use_id"] = json!("parent");
        // Streaming deltas from a child are not observed; complete child messages are.
        assert!(claude(&child).is_empty());
        assert_eq!(claude(&json!({"type":"system","subtype":"task_started","task_id":"a","task_type":"local_agent"}))[0].state, ItemState::Running);
        assert!(claude(
            &json!({"type":"system","subtype":"task_started","task_id":"a","task_type":"shell"})
        )
        .is_empty());
    }

    #[test]
    fn rows_keep_a_bounded_detail_their_reply_offset_and_skills() {
        let read = json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"src/app.ts"}}]}});
        assert_eq!(claude(&read)[0].detail, "src/app.ts");
        let bash = json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"b","name":"Bash","input":{"command":"npm test\nrm -rf x"}}]}});
        assert_eq!(detail(&claude(&bash)[0].detail), "npm test");
        let skill_call = json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"s","name":"Skill","input":{"skill":"graphify"}}]}});
        assert_eq!(claude(&skill_call)[0].kind, ItemKind::Skill);
        let command = codex(
            &json!({"id":"c","type":"commandExecution","command":"cargo test","status":"completed","exitCode":1}),
            false,
        );
        assert_eq!(
            (command[0].detail.as_str(), &command[0].state),
            ("cargo test", &ItemState::Failed)
        );
        // The first observation fixes the row's place in the reply; later updates keep it.
        let mut a = TurnActivity::new(AgentProviderId::Claude, None);
        let mut first = claude(&read).remove(0);
        first.offset = Some(12);
        a.observe(first);
        let mut done = item("t".into(), ItemKind::Tool, "", ItemState::Completed, None);
        done.offset = Some(90);
        a.observe(done);
        assert_eq!(
            (a.items[0].offset, a.items[0].detail.as_str()),
            (Some(12), "src/app.ts")
        );
        // A command's result keeps the end of its output, without terminal colour codes.
        let mut b = TurnActivity::new(AgentProviderId::Claude, None);
        b.observe(claude(&bash).remove(0));
        let result = json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b","is_error":true,"content":[{"type":"text","text":"\u{1b}[31mFAIL\u{1b}[0m 2 tests"}]}]}});
        b.observe(claude(&result).remove(0));
        assert_eq!(
            (b.items[0].output.as_str(), &b.items[0].state),
            ("FAIL 2 tests", &ItemState::Failed)
        );
        // Reads never keep output.
        let mut c = TurnActivity::new(AgentProviderId::Claude, None);
        c.observe(claude(&read).remove(0));
        let read_result = json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"file body"}]}});
        c.observe(claude(&read_result).remove(0));
        assert!(c.items[0].output.is_empty());
        assert_eq!(output(&"x\n".repeat(100)).lines().count(), 60);
        let row = skill("graphify");
        assert_eq!(
            (row.kind, row.detail.as_str(), row.offset),
            (ItemKind::Skill, "graphify", Some(0))
        );
    }

    #[test]
    fn v2_child_activity_and_spawn_identity_are_observed_without_inventing_work() {
        let result = codex(
            &json!({"id":"i","type":"subAgentActivity","agentThreadId":"child","agentPath":"root/review","kind":"started"}),
            true,
        );
        assert_eq!(result[0].state, ItemState::Running);
        assert_eq!(result[0].label, "root/review");
        assert!(codex(&json!({"id":"i","type":"subAgentActivity","agentThreadId":"child","kind":"interacted"}), false).is_empty());
        let result = codex(
            &json!({"id":"i","type":"collabAgentToolCall","tool":"spawnAgent","status":"completed","receiverThreadIds":["child"],"model":"selected"}),
            false,
        );
        assert_eq!(result[0].state, ItemState::Unknown);
        assert_eq!(result[0].model.as_deref(), Some("selected"));
        let mut a = TurnActivity::new(AgentProviderId::Codex, None);
        for observation in result {
            a.observe(observation);
        }
        for kind in ["started", "completed"] {
            let v = serde_json::json!({"id":"activity","type":"subAgentActivity","agentThreadId":"child","agentPath":"Review","kind":kind});
            for started in [true, false] {
                for observation in codex(&v, started) {
                    a.observe(observation);
                }
            }
        }
        assert_eq!(a.items.len(), 1);
        assert_eq!(a.items[0].model.as_deref(), Some("selected"));
        assert_eq!(a.items[0].state, ItemState::Completed);
    }

    #[test]
    fn children_can_work_again_but_wait_models_do_not_overwrite_spawn_metadata() {
        let mut a = TurnActivity::new(AgentProviderId::Codex, None);
        for state in [
            ItemState::Completed,
            ItemState::Running,
            ItemState::Completed,
        ] {
            a.observe(item(
                "agent:x".into(),
                ItemKind::Agent,
                "",
                state.clone(),
                None,
            ));
            assert_eq!(a.items[0].state, state);
        }
        let result = codex(
            &json!({"id":"wait","type":"collabAgentToolCall","tool":"wait","model":"unrelated","agentsStates":{"x":{"status":"running"}}}),
            false,
        );
        assert_eq!(result[0].model, None);
    }

    #[test]
    fn metadata_updates_keep_status_and_finished_tools_ignore_replay() {
        let mut a = TurnActivity::new(AgentProviderId::OpenCode, None);
        a.observe(item(
            "x".into(),
            ItemKind::Read,
            "Read",
            ItemState::Completed,
            None,
        ));
        a.observe(item(
            "x".into(),
            ItemKind::Tool,
            "",
            ItemState::Unknown,
            None,
        ));
        assert_eq!(a.items[0].state, ItemState::Completed);
        assert_eq!(a.items[0].kind, ItemKind::Read);
        a.observe(item(
            "x".into(),
            ItemKind::Read,
            "Read",
            ItemState::Running,
            None,
        ));
        assert_eq!(a.items[0].state, ItemState::Completed);
    }
    #[test]
    fn reload_caps_work_at_last_native_observation_and_ignores_shell_task_completion() {
        let mut s: Session = serde_json::from_value(json!({"id":"s","projectId":"p","title":"Task","agent":"claude","status":"stopped","createdAt":"2020-01-01T00:00:00Z","lastActivityAt":"2020-01-01T00:00:08Z","lastError":null,"worktree":{"path":"/fixture","branch":"main","isolated":false},"messages":[{"id":"m","sessionId":"s","role":"agent","content":"","createdAt":"2020-01-01T00:00:00Z","streaming":false}]})).unwrap();
        let mut a = TurnActivity::new(AgentProviderId::Claude, None);
        a.started_at = 1_577_836_800_000;
        s.messages[0].activity = Some(a);
        recover(&mut s);
        assert_eq!(
            s.messages[0].activity.as_ref().unwrap().ended_at,
            Some(1_577_836_808_000)
        );
        let mut a = TurnActivity::new(AgentProviderId::Claude, None);
        a.status = SessionStatus::Running;
        s.messages[0].activity = Some(a);
        s.status = SessionStatus::Running;
        for i in claude(
            &json!({"type":"system","subtype":"task_notification","task_id":"shell","status":"completed","description":"Shell"}),
        ) {
            observe(&mut s, i);
        }
        assert!(s.messages[0].activity.as_ref().unwrap().items.is_empty());
    }

    #[test]
    fn the_live_thought_is_the_first_sentence_of_the_latest_reasoning_and_never_saved() {
        assert_eq!(
            thought_line("I'll check the **config** first. Then run tests."),
            ("I'll check the config first.".to_string(), true)
        );
        assert_eq!(
            thought_line("**Inspecting the parser**\n\nLong body."),
            ("Inspecting the parser".to_string(), true)
        );
        // A streamed "3." with nothing after it is not a sentence end yet.
        assert_eq!(
            thought_line("Version 3."),
            ("Version 3.".to_string(), false)
        );
        let (long, done) = thought_line(&"word ".repeat(80));
        assert!(done && long.chars().count() <= THOUGHT_CHARS && long.ends_with('…'));
        assert_eq!(thought_line("keep snake_case").0, "keep snake_case");

        let mut a = TurnActivity::new(AgentProviderId::Claude, None);
        let frames = [
            json!({"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"thinking"}}}),
            json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"Let me read"}}}),
            json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":" the README. Then I"}}}),
            json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":" will edit."}}}),
        ];
        for frame in &frames {
            for observed in claude(frame) {
                a.observe(observed);
            }
        }
        assert_eq!(a.thought.as_deref(), Some("Let me read the README."));
        assert!(a.items.is_empty(), "reasoning never becomes a row");
        // Child thinking is not the main agent's thought.
        let mut child = frames[1].clone();
        child["parent_tool_use_id"] = json!("call");
        assert!(claude(&child).is_empty());
        // Sent to the renderer, left out of transcript files.
        assert!(serde_json::to_string(&a).unwrap().contains("Let me read"));
        assert!(!persisting(|| serde_json::to_string(&a).unwrap()).contains("Let me read"));
        let loaded: TurnActivity =
            serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert!(loaded.thought.is_none());
        // The answer starting ends it; a new block replaces it.
        a.observe(claude(&json!({"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"text"}}})).remove(0));
        assert!(a.thought.is_none());
        // Codex: a finished reasoning item's latest summary; OpenCode: thought chunks.
        for observed in codex(
            &json!({"id":"r","type":"reasoning","summary":["**Old**","**Planning the fix**\n\nbody"]}),
            false,
        ) {
            a.observe(observed);
        }
        assert_eq!(a.thought.as_deref(), Some("Planning the fix"));
        assert!(codex_child(
            &json!({"id":"r","type":"reasoning","summary":["x."]}),
            false,
            "c"
        )
        .is_empty());
        let mut o = TurnActivity::new(AgentProviderId::OpenCode, None);
        for text in ["Checking", " the tests. More"] {
            for observed in opencode(
                &json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":text}}),
            ) {
                o.observe(observed);
            }
        }
        assert_eq!(o.thought.as_deref(), Some("Checking the tests."));
        // Cleared when the turn ends.
        a.sync_at(SessionStatus::Completed, a.started_at + 1);
        assert!(a.thought.is_none());
    }

    #[test]
    fn a_finished_background_task_is_a_marker_with_its_outcome() {
        let mut a = TurnActivity::new(AgentProviderId::Claude, None);
        let mut marker = background_finished("bg1", "Audit queries", &json!("failed"), false);
        marker.offset = Some(9);
        assert!(a.observe(marker));
        let row = &a.items[0];
        assert_eq!(
            (
                row.id.as_str(),
                row.label.as_str(),
                &row.state,
                row.offset,
                row.finished_task
            ),
            (
                "done:bg1",
                "Audit queries",
                &ItemState::Failed,
                Some(9),
                true
            )
        );
        assert!(serde_json::to_string(row)
            .unwrap()
            .contains("\"finishedTask\":true"));
        assert_eq!(
            background_finished("x", "", &Value::Null, false).label,
            "Subagent"
        );
        let shell = background_finished("x", "", &json!("completed"), true);
        assert_eq!((shell.label.as_str(), shell.shell), ("Command", true));
        // The delivery signal never becomes a row.
        assert!(!a.observe(delivered()));
        assert_eq!(a.items.len(), 1);
    }

    #[test]
    fn acp_closed_tools_ignore_thoughts_and_output() {
        assert!(opencode(&json!({"sessionUpdate":"agent_thought_chunk"})).is_empty());
        let items = opencode(
            &json!({"sessionUpdate":"tool_call","toolCallId":"a","kind":"edit","status":"failed","rawInput":{"secret":true}}),
        );
        assert_eq!(items[0].kind, ItemKind::Edit);
        assert_eq!(items[0].state, ItemState::Failed);
    }

    #[test]
    fn claude_children_own_their_steps_and_absorb_the_delegation_call() {
        let mut a = TurnActivity::new(AgentProviderId::Claude, None);
        let call = json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"call","name":"Agent","input":{"description":"Write tests","prompt":"secret brief","model":"haiku"}}]}});
        for observed in claude(&call) {
            a.observe(observed);
        }
        for observed in claude(
            &json!({"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"call","description":"Write tests","task_type":"local_agent"}),
        ) {
            a.observe(observed);
        }
        // Child tools arrive as complete messages tagged with the delegation call.
        let read = json!({"type":"assistant","parent_tool_use_id":"call","message":{"content":[{"type":"tool_use","id":"r","name":"Read","input":{"file_path":"/private/key"}}]}});
        let done = json!({"type":"user","parent_tool_use_id":"call","message":{"content":[{"type":"tool_result","tool_use_id":"r","content":"private output"}]}});
        for observed in claude(&read).into_iter().chain(claude(&done)) {
            assert!(a.observe(observed));
        }
        // The call's own result is the child's, not a new tool row.
        for observed in claude(
            &json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call"}]}}),
        ) {
            a.observe(observed);
        }
        assert_eq!(a.items.len(), 1);
        let child = &a.items[0];
        assert_eq!(child.kind, ItemKind::Agent);
        assert_eq!(child.label, "Write tests");
        assert_eq!(child.model.as_deref(), Some("haiku"));
        assert!(child.started_at.is_some());
        assert_eq!(child.steps.len(), 1);
        assert_eq!(child.steps[0].kind, ItemKind::Read);
        assert_eq!(child.steps[0].state, ItemState::Completed);
        let encoded = serde_json::to_string(&a).unwrap();
        for secret in [
            "secret brief",
            "/private/key",
            "private output",
            "\"parent\"",
        ] {
            assert!(!encoded.contains(secret), "{secret}");
        }
        for observed in claude(
            &json!({"type":"system","subtype":"task_notification","task_id":"t1","status":"completed"}),
        ) {
            a.observe(observed);
        }
        assert_eq!(a.items[0].state, ItemState::Completed);
        assert!(a.items[0].ended_at.is_some());
    }

    #[test]
    fn opencode_task_delegation_is_a_child_row_without_its_prompt() {
        let mut a = TurnActivity::new(AgentProviderId::OpenCode, None);
        let start = json!({"sessionUpdate":"tool_call","toolCallId":"t","kind":"think","status":"pending","title":"task","rawInput":{"description":"List txt files","prompt":"secret brief","subagent_type":"explore"}});
        let done = json!({"sessionUpdate":"tool_call_update","toolCallId":"t","status":"completed","rawOutput":{"output":"private"}});
        for observed in opencode(&start).into_iter().chain(opencode(&done)) {
            a.observe(observed);
        }
        assert_eq!(a.items.len(), 1);
        assert_eq!(a.items[0].kind, ItemKind::Agent);
        assert_eq!(a.items[0].label, "List txt files");
        assert_eq!(a.items[0].state, ItemState::Completed);
        assert!(a.items[0].ended_at.is_some());
        let encoded = serde_json::to_string(&a).unwrap();
        assert!(!encoded.contains("secret brief") && !encoded.contains("private"));
    }
    #[test]
    fn child_steps_are_bounded_and_settle_with_the_turn() {
        let mut a = TurnActivity::new(AgentProviderId::Codex, None);
        a.observe(item(
            "agent:c".into(),
            ItemKind::Agent,
            "Worker",
            ItemState::Running,
            None,
        ));
        for n in 0..(MAX_CHILD_STEPS + 10) {
            let v = json!({"id":format!("s{n}"),"type":"fileChange","status":"inProgress"});
            for observed in codex_child(&v, true, "c") {
                a.observe(observed);
            }
        }
        assert_eq!(a.items[0].steps.len(), MAX_CHILD_STEPS);
        assert_eq!(a.items[0].hidden_steps, 10);
        assert_eq!(a.items[0].steps[0].id, "s10");
        // A completion for a folded-away step does not come back as new work.
        let old = json!({"id":"s0","type":"fileChange","status":"completed"});
        for observed in codex_child(&old, false, "c") {
            assert!(!a.observe(observed));
        }
        a.sync_at(SessionStatus::Stopped, a.started_at + 10);
        let child = &a.items[0];
        assert_eq!(child.state, ItemState::Stopped);
        assert!(child.ended_at.is_some());
        assert!(child
            .steps
            .iter()
            .all(|step| step.state == ItemState::Stopped));
    }

    #[test]
    fn codex_child_work_waits_for_its_row_and_nested_children_stay_inside() {
        let mut a = TurnActivity::new(AgentProviderId::Codex, None);
        let early = json!({"id":"cmd","type":"commandExecution","status":"inProgress","command":"cat secret"});
        for observed in codex_child(&early, true, "child") {
            assert!(!a.observe(observed));
        }
        assert!(a.items.is_empty());
        let nested = json!({"id":"spawn","type":"collabAgentToolCall","tool":"spawnAgent","status":"completed","receiverThreadIds":["grandchild"]});
        assert!(codex_child(&nested, false, "child").is_empty());
        for observed in codex(
            &json!({"id":"i","type":"subAgentActivity","agentThreadId":"child","agentPath":"Review","kind":"started"}),
            true,
        ) {
            a.observe(observed);
        }
        assert_eq!(a.items.len(), 1);
        assert_eq!(a.items[0].steps.len(), 1);
        assert_eq!(a.items[0].steps[0].kind, ItemKind::Command);
        assert!(!serde_json::to_string(&a).unwrap().contains("cat secret"));
        // Work from a thread that never became a child row is never shown.
        for observed in codex_child(&early, true, "stranger") {
            assert!(!a.observe(observed));
        }
        assert_eq!(a.items.len(), 1);
    }

    fn plan_of(items: Vec<ActivityItem>) -> Vec<(String, PlanStatus)> {
        let mut a = TurnActivity::new(AgentProviderId::Claude, None);
        for observed in items {
            a.observe(observed);
        }
        a.plan.into_iter().map(|e| (e.text, e.status)).collect()
    }
    #[test]
    fn every_provider_plan_shape_becomes_one_list() {
        use PlanStatus::*;
        let want = vec![
            ("Read".to_owned(), Completed),
            ("Fix".to_owned(), InProgress),
            ("Test".to_owned(), Pending),
        ];
        // Claude TodoWrite, from the complete message (the streamed start has no input).
        let claude_todos = json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"TodoWrite","input":{"todos":[
            {"content":"Read","status":"completed","activeForm":"Reading"},
            {"content":"Fix","status":"in_progress","activeForm":"Fixing"},
            {"content":"Test","status":"pending","activeForm":"Testing"}]}}]}});
        assert_eq!(plan_of(claude(&claude_todos)), want);
        let start = json!({"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"tool_use","id":"t","name":"TodoWrite","input":{}}}});
        assert!(plan_of(claude(&start)).is_empty());
        let mut child = claude_todos.clone();
        child["parent_tool_use_id"] = json!("p");
        assert!(plan_of(claude(&child)).is_empty());
        // Codex update_plan and the legacy todoList item.
        let codex_plan_update = codex_plan(&json!({"plan":[
            {"step":"Read","status":"completed"},{"step":"Fix","status":"inProgress"},{"step":"Test","status":"pending"}]}));
        assert_eq!(plan_of(codex_plan_update.into_iter().collect()), want);
        let legacy = codex(
            &json!({"id":"l","type":"todoList","items":[
                {"text":"Read","completed":true},{"text":"Fix","completed":false},{"text":"Test","completed":false}]}),
            false,
        );
        assert_eq!(plan_of(legacy)[0], ("Read".to_owned(), Completed));
        // ACP plan entries.
        let acp = opencode(&json!({"sessionUpdate":"plan","entries":[
            {"content":"Read","priority":"high","status":"completed"},
            {"content":"Fix","priority":"high","status":"in_progress"},
            {"content":"Test","priority":"low","status":"pending"}]}));
        assert_eq!(plan_of(acp), want);
        // OpenCode todowrite (and a cancelled todo reads as pending) and Cursor todos.
        let todowrite = opencode(
            &json!({"sessionUpdate":"tool_call","toolCallId":"w","kind":"other","status":"completed","rawInput":{"todos":[
            {"content":"Read","status":"completed","priority":"high"},
            {"content":"Fix","status":"in_progress"},
            {"content":"Test","status":"cancelled"}]}}),
        );
        assert_eq!(todowrite.len(), 2);
        assert_eq!(plan_of(todowrite), want);
    }
    #[test]
    fn plans_replace_wholesale_clear_and_mask_credentials() {
        let mut a = TurnActivity::new(AgentProviderId::Codex, None);
        let update =
            |step: &str| codex_plan(&json!({"plan":[{"step":step,"status":"pending"}]})).unwrap();
        assert!(a.observe(update("One")));
        assert!(!a.observe(update("One")));
        assert!(a.observe(update("Two")));
        assert_eq!(a.plan.len(), 1);
        assert!(a.observe(codex_plan(&json!({"plan":[]})).unwrap()));
        assert!(a.plan.is_empty());
        let long = "x".repeat(500);
        a.observe(update(&long));
        assert_eq!(a.plan[0].text.chars().count(), MAX_PLAN_TEXT);
        assert!(serde_json::to_string(&a).unwrap().contains("\"plan\""));
    }

    /// The picture folder is process-wide: the two picture tests take turns.
    static PICTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_tool_result_picture_is_saved_once_and_only_its_path_is_kept() {
        use base64::Engine as _;
        let _turn = PICTURES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        crate::tool_images::set_dir_for_tests(Some(dir.path().join("tool-images")));
        let bytes = [
            b"\x89PNG\r\n\x1a\n".as_slice(),
            b"not-a-real-picture-but-bytes",
        ]
        .concat();
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mut turn = TurnActivity::new(AgentProviderId::Claude, None);
        let frame = |id: &str| {
            claude(&json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":id,"name":"mcp__sirus_browser__browser_screenshot","input":{}}]}}))
        };
        let result = |id: &str| {
            claude(&json!({"type":"user","message":{"content":[
                {"type":"tool_result","tool_use_id":id,"content":[
                    {"type":"text","text":"Captured"},
                    {"type":"image","source":{"type":"base64","media_type":"image/png","data":data}}]}]}}))
        };
        for item in frame("a").into_iter().chain(result("a")) {
            turn.observe(item);
        }
        for item in frame("b").into_iter().chain(result("b")) {
            turn.observe(item);
        }
        let step = turn.items.iter().find(|item| item.id == "a").unwrap();
        assert_eq!(
            step.detail, "browser_screenshot",
            "the MCP tool keeps its name"
        );
        assert_eq!(step.label, "Tool call");
        assert!(step.output.is_empty(), "only command rows keep output text");
        let path = step
            .image_path
            .clone()
            .expect("the picture path is on the item");
        assert!(path.ends_with(".png"));
        assert_eq!(
            turn.items[1].image_path.as_deref(),
            Some(path.as_str()),
            "dedup by hash"
        );
        let saved: Vec<_> = std::fs::read_dir(dir.path().join("tool-images"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(saved.len(), 1, "one file for two identical results");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let json = serde_json::to_string(&turn).unwrap();
        assert!(json.contains(&path), "the path is saved");
        assert!(!json.contains(&data), "the base64 is never saved");
        // Codex: an MCP result's image block is saved the same way.
        let codex_row = codex(
            &json!({"type":"mcpToolCall","id":"c","tool":"browser_screenshot","status":"completed",
                "result":{"content":[{"type":"image","data":data,"mimeType":"image/png"}]}}),
            false,
        );
        assert_eq!(codex_row[0].image_path.as_deref(), Some(path.as_str()));
        assert_eq!(codex_row[0].detail, "browser_screenshot");
        let json = serde_json::to_string(&codex_row).unwrap();
        assert!(!json.contains(&data));
        crate::tool_images::set_dir_for_tests(None);
    }

    #[test]
    fn codex_truncated_result_takes_the_picture_saved_at_the_source() {
        let _turn = PICTURES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        crate::tool_images::set_dir_for_tests(Some(dir.path().join("tool-images")));
        let bytes = [b"\x89PNG\r\n\x1a\n".as_slice(), b"source-saved"].concat();
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        // The tool saves the picture where it is produced.
        crate::tool_images::remember("s1", "browser_screenshot", &json!({"image":{"data":data}}));
        crate::tool_images::remember("s1", "browser_click", &json!({"image":{"data":data}}));
        // Codex's item holds the result as truncated text, with no image block.
        let row = codex(
            &json!({"type":"mcpToolCall","id":"exec-1","tool":"browser_screenshot","status":"completed",
                "result":{"content":[{"type":"text","text":"{\"content\":[{\"type\":\"image\",\"data\":\"iVBORw0KGgoAAA"}]}}),
            false,
        );
        assert_eq!(row[0].image_path, None);
        assert_eq!(crate::tool_images::take("s2", "browser_screenshot"), None);
        assert_eq!(crate::tool_images::take("s1", "browser_click"), None);
        let path = crate::tool_images::take("s1", "browser_screenshot").expect("claimed");
        assert!(path.ends_with(".png"));
        assert_eq!(crate::tool_images::take("s1", "browser_screenshot"), None);
        crate::tool_images::set_dir_for_tests(None);
    }

    #[test]
    fn no_picture_path_without_a_folder_or_a_picture() {
        let _turn = PICTURES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::tool_images::set_dir_for_tests(None);
        let rows = claude(&json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"x","content":[
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}}]}]}}));
        assert_eq!(rows[0].image_path, None);
        let mcp = claude(&json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"m","name":"mcp__sirus_computer__computer_click","input":{"x":1}}]}}));
        assert_eq!(mcp[0].detail, "computer_click");
        let plain = claude(&json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","id":"p","name":"Bash","input":{"command":"ls"}}]}}));
        assert_eq!(plain[0].detail, "ls");
    }
}
