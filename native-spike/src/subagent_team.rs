//! The subagents a turn launched, as Sirus's team card reads them (`lib/subagent-team.ts`,
//! ADR-146): who they are, their state, the phases they were launched in, the card's counts and
//! the elapsed-time text. Pure data, so it is tested without a window.
use serde_json::Value;
use std::ops::Range;

/// How a subagent reads on the card.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Running,
    Completed,
    Failed,
    Stopped,
    Unknown,
}

impl Status {
    /// A background child the turn stopped (`timedOut`) reads as stopped; an unreported one stays unknown.
    pub fn of(state: &str, timed_out: bool) -> Self {
        if timed_out {
            return Self::Stopped;
        }
        match state {
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "stopped" => Self::Stopped,
            _ => Self::Unknown,
        }
    }
}

/// One subagent of a turn.
#[derive(Clone, Debug)]
pub struct Member {
    /// The CLI's task id behind the row (`agent:<task id>`), which stop and message take.
    pub task: String,
    /// Where its own conversation is found (`subagent_feed`): its launching call, else its row id.
    pub key: String,
    pub label: String,
    pub status: Status,
    /// Native observation window, in milliseconds since the epoch.
    pub started: Option<i64>,
    pub ended: Option<i64>,
    /// Tools it used: the steps it reported, kept or folded away.
    pub tools: usize,
    /// What it is doing now (the running step), else its last step.
    pub step: String,
    pub background: bool,
    /// The launching tool call, which `background_subagent` takes (empty for a workflow).
    pub source: Option<String>,
    /// A workflow's row: its members are not moved one by one.
    pub workflow: bool,
    /// The phase it was launched in, from 1.
    pub phase: usize,
    /// The model the subagent runs on, as the host reports it (empty when unknown).
    pub model: Option<String>,
}

impl Member {
    pub fn from_item(item: &Value, phase: usize) -> Self {
        let id = item["id"].as_str().unwrap_or("");
        let steps = item["steps"].as_array();
        let tools = match item["workflow"]["agents"].as_array() {
            Some(agents) => agents
                .iter()
                .map(|a| a["toolCalls"].as_u64().unwrap_or(0) as usize)
                .sum(),
            None => steps.map_or(0, Vec::len) + item["hiddenSteps"].as_u64().unwrap_or(0) as usize,
        };
        let status = Status::of(
            item["state"].as_str().unwrap_or(""),
            item["timedOut"].as_bool().unwrap_or(false),
        );
        // Progress in the agent's own words, else the running step. A finished subagent shows
        // none: "Running cargo test" would read as still happening.
        let step = (status == Status::Running)
            .then(|| {
                item["detail"]
                    .as_str()
                    .map(str::trim)
                    .filter(|detail| !detail.is_empty())
                    .map(str::to_owned)
                    .or_else(|| {
                        steps?
                            .iter()
                            .rev()
                            .find(|s| s["state"] == "running")
                            .or_else(|| steps?.last())?["label"]
                            .as_str()
                            .map(|label| crate::i18n::from_host(label.trim()))
                    })
            })
            .flatten()
            .unwrap_or_default();
        Self {
            task: id.strip_prefix("agent:").unwrap_or(id).to_owned(),
            key: item["source"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(id)
                .to_owned(),
            label: item["label"].as_str().unwrap_or("").trim().to_owned(),
            status,
            started: item["startedAt"].as_i64(),
            ended: item["endedAt"].as_i64(),
            tools,
            step,
            background: item["background"].as_bool().unwrap_or(false),
            source: item["source"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            workflow: item.get("workflow").is_some_and(|w| !w.is_null()),
            phase,
            model: item["model"]
                .as_str()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_owned),
        }
    }

    /// The tool call to send a running foreground subagent to the background with, as React's
    /// SubagentControls offers it: not already in the background, not a workflow, and known.
    pub fn backgroundable(&self) -> Option<&str> {
        if self.running() && !self.background && !self.workflow {
            self.source.as_deref()
        } else {
            None
        }
    }

    pub fn running(&self) -> bool {
        self.status == Status::Running
    }

    /// Seconds it has worked: its own window, still open while it runs.
    pub fn elapsed(&self, now_ms: i64) -> Option<u64> {
        elapsed(self.started, self.ended, now_ms)
    }
}

/// Seconds between `started` and `ended` (else `now`), never negative.
pub fn elapsed(started: Option<i64>, ended: Option<i64>, now_ms: i64) -> Option<u64> {
    let started = started?;
    Some((ended.unwrap_or(now_ms) - started).max(0) as u64 / 1000)
}

/// `12s`, `3m 05s`... as Sirus's `formatActivityDuration`: seconds, minutes and seconds, then hours and minutes.
pub fn duration(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, seconds / 60 % 60),
    }
}

/// Splits subagents into phases as they are met: launched back to back (nothing else between
/// them) at the same place in the reply is one phase; other work in between, or another place,
/// starts the next. Finished markers and background shells neither join nor split a phase.
#[derive(Default)]
pub struct Phaser {
    phase: usize,
    open: Option<Option<u32>>,
}

impl Phaser {
    /// Something that is not a subagent happened (a tool call, a command).
    pub fn other(&mut self) {
        self.open = None;
    }

    /// A subagent launched where the reply stood at `offset`; its phase, from 1.
    pub fn member(&mut self, offset: Option<u32>) -> usize {
        let split = match self.open {
            None => true,
            Some(first) => matches!((offset, first), (Some(a), Some(b)) if a != b),
        };
        if split {
            self.phase += 1;
            self.open = Some(offset);
        }
        self.phase
    }
}

/// The runs of members that share a phase, in order.
pub fn phases(members: &[Member]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for (at, member) in members.iter().enumerate() {
        match out.last_mut() {
            Some(run) if members[run.start].phase == member.phase => run.end = at + 1,
            _ => out.push(at..at + 1),
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Progress {
    /// Settled: no longer running.
    pub done: usize,
    pub total: usize,
    pub running: usize,
    pub failed: usize,
    /// Running ones launched with `run_in_background`.
    pub background: usize,
}

pub fn progress(members: &[Member]) -> Progress {
    let mut out = Progress {
        total: members.len(),
        ..Progress::default()
    };
    for member in members {
        match member.status {
            Status::Running => {
                out.running += 1;
                out.background += member.background as usize;
            }
            Status::Failed => {
                out.done += 1;
                out.failed += 1;
            }
            _ => out.done += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn member(phase: usize, status: Status) -> Member {
        Member::from_item(&json!({ "id": "agent:a", "label": "x" }), phase).with(status)
    }

    #[test]
    fn the_model_comes_from_the_host_item_when_it_reports_one() {
        let known = Member::from_item(
            &json!({ "id": "agent:a", "label": "x", "model": "claude-haiku-5-5" }),
            1,
        );
        assert_eq!(known.model.as_deref(), Some("claude-haiku-5-5"));
        let blank = Member::from_item(&json!({ "id": "agent:b", "model": "  " }), 1);
        assert_eq!(blank.model, None);
    }

    impl Member {
        fn with(mut self, status: Status) -> Self {
            self.status = status;
            self
        }
    }

    #[test]
    fn back_to_back_launches_are_one_phase() {
        let mut p = Phaser::default();
        assert_eq!(
            [p.member(Some(10)), p.member(Some(10)), p.member(None)],
            [1, 1, 1]
        );
    }

    #[test]
    fn other_work_or_another_place_starts_the_next_phase() {
        let mut p = Phaser::default();
        assert_eq!(p.member(Some(10)), 1);
        p.other();
        assert_eq!(p.member(Some(10)), 2);
        assert_eq!(p.member(Some(40)), 3);
        assert_eq!(p.member(Some(40)), 3);
        // A launch without a place cannot tell it moved.
        assert_eq!(p.member(None), 3);
    }

    #[test]
    fn runs_of_a_phase_are_ranges() {
        let members: Vec<Member> = [1, 1, 2, 3, 3]
            .iter()
            .map(|n| member(*n, Status::Running))
            .collect();
        assert_eq!(phases(&members), vec![0..2, 2..3, 3..5]);
        assert!(phases(&[]).is_empty());
    }

    #[test]
    fn counts_settled_running_failed_and_background() {
        let mut bg = member(1, Status::Running);
        bg.background = true;
        let members = vec![
            member(1, Status::Completed),
            member(1, Status::Failed),
            member(1, Status::Stopped),
            member(1, Status::Running),
            member(2, Status::Unknown),
            bg,
        ];
        assert_eq!(
            progress(&members),
            Progress {
                done: 4,
                total: 6,
                running: 2,
                failed: 1,
                background: 1
            }
        );
    }

    #[test]
    fn a_timed_out_child_reads_as_stopped() {
        assert_eq!(Status::of("running", true), Status::Stopped);
        assert_eq!(Status::of("running", false), Status::Running);
        assert_eq!(Status::of("queued", false), Status::Unknown);
    }

    #[test]
    fn elapsed_runs_until_it_ends() {
        assert_eq!(elapsed(Some(1_000), Some(13_999), 99_000), Some(12));
        assert_eq!(elapsed(Some(1_000), None, 4_500), Some(3));
        assert_eq!(elapsed(Some(5_000), None, 1_000), Some(0));
        assert_eq!(elapsed(None, None, 1_000), None);
    }

    #[test]
    fn durations_read_like_sirus_code() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(59), "59s");
        assert_eq!(duration(60), "1m 0s");
        assert_eq!(duration(185), "3m 5s");
        assert_eq!(duration(3599), "59m 59s");
        assert_eq!(duration(3600), "1h 0m");
        assert_eq!(duration(7384), "2h 3m");
    }

    #[test]
    fn a_member_reads_its_row() {
        let item = json!({
            "id": "agent:t1", "source": "toolu_9", "label": " Review ", "state": "running",
            "startedAt": 1000, "background": true, "hiddenSteps": 2,
            "steps": [
                { "id": "s1", "kind": "command", "label": "Read file", "state": "completed" },
                { "id": "s2", "kind": "command", "label": "Running cargo test", "state": "running" },
                { "id": "s3", "kind": "read", "label": "Read other", "state": "completed" }
            ]
        });
        let m = Member::from_item(&item, 2);
        assert_eq!(
            (m.task.as_str(), m.key.as_str(), m.label.as_str()),
            ("t1", "toolu_9", "Review")
        );
        assert_eq!(
            (m.tools, m.step.as_str(), m.phase),
            (5, "Running cargo test", 2)
        );
        assert!(m.running() && m.background);
        // Its progress wins over the step's generic name, and a finished one shows neither.
        let told = json!({ "id": "agent:t3", "state": "running", "detail": "Running npm test",
            "steps": [{ "label": "Command", "state": "running" }] });
        assert_eq!(Member::from_item(&told, 1).step, "Running npm test");
        let done = json!({ "id": "agent:t4", "state": "completed", "detail": "Running npm test",
            "steps": [{ "label": "Command", "state": "completed" }] });
        assert_eq!(Member::from_item(&done, 1).step, "");
        // Without steps it has no current step; the key falls back to the row id.
        let bare = Member::from_item(&json!({ "id": "agent:t2", "state": "completed" }), 1);
        assert_eq!(bare.backgroundable(), None);
        assert_eq!(
            (bare.step.as_str(), bare.key.as_str(), bare.tools),
            ("", "agent:t2", 0)
        );
    }

    #[test]
    fn only_a_running_foreground_subagent_with_a_source_can_go_to_the_background() {
        let item = |extra: Value| {
            let mut base = json!({ "id": "agent:a", "source": "toolu_1", "state": "running" });
            base.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            Member::from_item(&base, 1)
        };
        assert_eq!(item(json!({})).backgroundable(), Some("toolu_1"));
        assert_eq!(item(json!({ "background": true })).backgroundable(), None);
        assert_eq!(
            item(json!({ "workflow": { "agents": [] } })).backgroundable(),
            None
        );
        assert_eq!(item(json!({ "state": "completed" })).backgroundable(), None);
        assert_eq!(item(json!({ "source": "" })).backgroundable(), None);
    }

    #[test]
    fn a_workflow_counts_its_members_tools() {
        let item = json!({ "id": "agent:w", "workflow": { "agents": [{ "toolCalls": 3 }, { "toolCalls": 4 }, {}] } });
        assert_eq!(Member::from_item(&item, 1).tools, 7);
    }
}
