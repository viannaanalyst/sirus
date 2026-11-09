//! Scheduled tasks, "Tarefas" in the window (ADR-200): a prompt that starts a new conversation by
//! itself, once or on a repeat rule. A background loop wakes every minute, claims the tasks that
//! are due and runs each through `launch::launch`, the path delegation uses, so a run is a normal,
//! visible conversation. Tasks are one row each in `state.db` (`scheduled_prompts`, ADR-114).
//!
//! Times are the host's local time as naive ISO strings (`2026-11-10T09:00:00`): a rule such as
//! "weekdays at 9:00" means 9:00 on the wall clock. The schedule maths are pure functions
//! ([`next_run`], [`occurrences`], [`claim_due`]) so they are tested without a clock.
//!
//! A run missed while Sirus was closed runs once at the next launch: the first pass claims every
//! due task and sets its next run after now, so nothing repeats to catch up.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Duration as Span, NaiveDate, NaiveDateTime, NaiveTime};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::{AgentProviderId, ApprovalMode};
use crate::runtime::{AppHandle, Emitter, Manager, State};

/// Emitted with `{}` when a task is saved, deleted, switched or has run.
pub const UPDATED: &str = "tasks-updated";

const WAKE: Duration = Duration::from_secs(60);
const MAX_TASKS: usize = 200;
const MAX_PROMPT_CHARS: usize = 20_000;
const MAX_TITLE_CHARS: usize = 80;
/// Runs kept per task, for the calendar's ✓ and ✗.
const MAX_HISTORY: usize = 60;
/// Planned runs listed per task in one calendar range.
const MAX_PLANNED: usize = 400;
const STAMP: &str = "%Y-%m-%dT%H:%M:%S";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RepeatKind {
    Daily,
    Weekdays,
    Weekly,
    /// On one day of the month; a shorter month uses its last day.
    Monthly,
    /// Every `every` days or weeks, counted from `start`.
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RepeatUnit {
    Days,
    Weeks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repeat {
    pub kind: RepeatKind,
    /// `HH:MM`, local.
    pub time: String,
    /// For `weekly`: 0 is Monday … 6 is Sunday.
    #[serde(default)]
    pub weekdays: Vec<u8>,
    /// For `monthly`: the day of the month, 1 to 31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<u8>,
    /// For `custom`: the step, 1 to 30 of `unit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<RepeatUnit>,
    /// `YYYY-MM-DD`: the first day a repeat may run, and the anchor of `custom`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Schedule {
    /// One local date and time, `YYYY-MM-DDTHH:MM`.
    Once(String),
    Repeat(Repeat),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    pub at: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// Empty when a window creates it; the host gives it one.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    pub prompt: String,
    pub project_id: String,
    pub provider: AgentProviderId,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// How the run's actions are approved; a missing value is `ask`, since nobody is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalMode>,
    /// Fast mode, for the models that have one.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fast: bool,
    /// Each run works in a new worktree instead of the project checkout.
    #[serde(default, skip_serializing_if = "is_false")]
    pub worktree: bool,
    pub schedule: Schedule,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Owned by the host: whatever a window sends here is replaced.
    #[serde(default)]
    pub last_run: Option<LastRun>,
    #[serde(default)]
    pub history: Vec<LastRun>,
    #[serde(default)]
    pub next_run: Option<String>,
}

fn yes() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Occurrence {
    pub task_id: String,
    pub at: String,
    /// `planned`, `done` or `failed`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskList {
    /// The host's local "now", so the calendar's line agrees with the schedule.
    pub now: String,
    pub tasks: Vec<Task>,
    pub occurrences: Vec<Occurrence>,
}

// ------------------------------------------------------------------ pure schedule maths

pub fn parse_stamp(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text, STAMP)
        .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M"))
        .ok()
}

pub fn stamp(at: NaiveDateTime) -> String {
    at.format(STAMP).to_string()
}

pub fn parse_time(text: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(text, "%H:%M").ok()
}

fn matches_day(repeat: &Repeat, date: NaiveDate) -> bool {
    let day = date.weekday().num_days_from_monday() as u8;
    match repeat.kind {
        RepeatKind::Daily => true,
        RepeatKind::Weekdays => day < 5,
        RepeatKind::Weekly => repeat.weekdays.contains(&day),
        // Handled by `monthly_next` and `custom_next`: not weekday patterns.
        RepeatKind::Monthly | RepeatKind::Custom => false,
    }
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|first| first.pred_opt())
        .map_or(28, |last| last.day())
}

/// The first `day` of a month at `time` strictly after `after`; a short month uses its last day.
fn monthly_next(day: u8, time: NaiveTime, after: NaiveDateTime) -> Option<NaiveDateTime> {
    if !(1..=31).contains(&day) {
        return None;
    }
    let (mut year, mut month) = (after.year(), after.month());
    for _ in 0..14 {
        let date = NaiveDate::from_ymd_opt(
            year,
            month,
            u32::from(day).min(last_day_of_month(year, month)),
        )?;
        let at = date.and_time(time);
        if at > after {
            return Some(at);
        }
        if month == 12 {
            (year, month) = (year + 1, 1);
        } else {
            month += 1;
        }
    }
    None
}

pub fn parse_day(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

/// The first run of an "every N days or weeks" rule strictly after `after`: the days are those
/// that are a whole number of steps from `start`.
fn custom_next(repeat: &Repeat, time: NaiveTime, after: NaiveDateTime) -> Option<NaiveDateTime> {
    let start = parse_day(repeat.start.as_deref()?)?;
    let every = i64::from(repeat.every.filter(|every| (1..=30).contains(every))?);
    let step = match repeat.unit? {
        RepeatUnit::Days => every,
        RepeatUnit::Weeks => 7 * every,
    };
    (0..=step).find_map(|offset| {
        let date = after.date().checked_add_signed(Span::days(offset))?;
        let at = date.and_time(time);
        let since = (date - start).num_days();
        (since >= 0 && since % step == 0 && at > after).then_some(at)
    })
}

/// The first run strictly after `after`; `None` when a one-time task is past or a rule is broken.
pub fn next_run(schedule: &Schedule, after: NaiveDateTime) -> Option<NaiveDateTime> {
    match schedule {
        Schedule::Once(at) => parse_stamp(at).filter(|at| *at > after),
        Schedule::Repeat(repeat) => {
            let time = parse_time(&repeat.time)?;
            // A repeat may not start before its first day.
            let after = match repeat.start.as_deref().and_then(parse_day) {
                Some(start) => after.max(start.and_time(NaiveTime::MIN) - Span::seconds(1)),
                None => after,
            };
            if repeat.kind == RepeatKind::Monthly {
                return monthly_next(repeat.day?, time, after);
            }
            if repeat.kind == RepeatKind::Custom {
                return custom_next(repeat, time, after);
            }
            // A week and a day cover every weekday pattern, month ends included.
            (0..=7).find_map(|offset| {
                let date = after.date().checked_add_signed(Span::days(offset))?;
                let at = date.and_time(time);
                (matches_day(repeat, date) && at > after).then_some(at)
            })
        }
    }
}

/// Runs with `from <= at < to`, at most `limit`.
pub fn occurrences(
    schedule: &Schedule,
    from: NaiveDateTime,
    to: NaiveDateTime,
    limit: usize,
) -> Vec<NaiveDateTime> {
    let mut found = Vec::new();
    let mut cursor = from - Span::seconds(1);
    while found.len() < limit {
        match next_run(schedule, cursor) {
            Some(at) if at < to => {
                found.push(at);
                cursor = at;
            }
            _ => break,
        }
    }
    found
}

/// Takes the enabled tasks whose next run is due: each one's next run moves past `now` at once,
/// so a run missed for any length of time happens a single time.
pub fn claim_due(tasks: &mut [Task], now: NaiveDateTime) -> Vec<Task> {
    let mut due = Vec::new();
    for task in tasks.iter_mut() {
        let is_due = task.enabled
            && task
                .next_run
                .as_deref()
                .and_then(parse_stamp)
                .is_some_and(|at| at <= now);
        if is_due {
            task.next_run = next_run(&task.schedule, now).map(stamp);
            due.push(task.clone());
        }
    }
    due
}

/// A title from the first line of the prompt.
pub fn title_of(prompt: &str) -> String {
    let line = prompt.lines().find(|line| !line.trim().is_empty());
    line.unwrap_or("")
        .trim()
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect()
}

fn check(task: &Task, now: NaiveDateTime) -> Result<()> {
    let invalid = |message: &str| Error::new("invalid", message);
    if task.prompt.trim().is_empty() {
        return Err(invalid("the prompt is empty"));
    }
    if task.prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(invalid("the prompt is too long"));
    }
    if let Some(model) = &task.model {
        if !crate::launch::valid_model(model) {
            return Err(invalid("the model is not valid"));
        }
    }
    match &task.schedule {
        Schedule::Once(at) => {
            let at = parse_stamp(at).ok_or_else(|| invalid("the date is not valid"))?;
            if at <= now {
                return Err(invalid("the date has already passed"));
            }
        }
        Schedule::Repeat(repeat) => {
            parse_time(&repeat.time).ok_or_else(|| invalid("the time is not valid"))?;
            if repeat.kind == RepeatKind::Weekly
                && (repeat.weekdays.is_empty() || repeat.weekdays.iter().any(|day| *day > 6))
            {
                return Err(invalid("pick at least one weekday"));
            }
            if repeat.kind == RepeatKind::Monthly
                && !repeat.day.is_some_and(|d| (1..=31).contains(&d))
            {
                return Err(invalid("pick a day of the month"));
            }
            if repeat
                .start
                .as_deref()
                .is_some_and(|day| parse_day(day).is_none())
            {
                return Err(invalid("the start date is not valid"));
            }
            if repeat.kind == RepeatKind::Custom
                && !(repeat.start.is_some()
                    && repeat.unit.is_some()
                    && repeat.every.is_some_and(|every| (1..=30).contains(&every)))
            {
                return Err(invalid("pick how often it repeats"));
            }
        }
    }
    Ok(())
}

fn now_local() -> NaiveDateTime {
    chrono::Local::now().naive_local()
}

fn list_at(tasks: &[Task], now: NaiveDateTime, from: NaiveDateTime, to: NaiveDateTime) -> TaskList {
    let mut found = Vec::new();
    for task in tasks {
        for run in &task.history {
            if parse_stamp(&run.at).is_some_and(|at| at >= from && at < to) {
                found.push(Occurrence {
                    task_id: task.id.clone(),
                    at: run.at.clone(),
                    state: if run.ok { "done" } else { "failed" },
                    session_id: run.session_id.clone(),
                });
            }
        }
        for at in occurrences(&task.schedule, from.max(now), to, MAX_PLANNED) {
            found.push(Occurrence {
                task_id: task.id.clone(),
                at: stamp(at),
                state: "planned",
                session_id: None,
            });
        }
    }
    found.sort_by(|a, b| a.at.cmp(&b.at));
    TaskList {
        now: stamp(now),
        tasks: tasks.to_vec(),
        occurrences: found,
    }
}

// ------------------------------------------------------------------ commands

fn changed(app: &AppHandle) {
    let _ = app.emit(UPDATED, json!({}));
}

/// The tasks and the runs inside `[from, to)` (default: the past week and the next five).
pub fn list_tasks(
    state: State<'_, Arc<AppState>>,
    from: Option<String>,
    to: Option<String>,
) -> Result<TaskList> {
    let now = now_local();
    let from = from
        .as_deref()
        .and_then(parse_stamp)
        .unwrap_or(now - Span::days(7));
    let to = to
        .as_deref()
        .and_then(parse_stamp)
        .unwrap_or(now + Span::days(35));
    let data = state.data.lock();
    Ok(list_at(
        &data.scheduled_prompts,
        now,
        from,
        to.min(from + Span::days(120)),
    ))
}

/// Creates a task, or replaces the one with the same id (its runs stay).
pub fn save_task(app: AppHandle, state: State<'_, Arc<AppState>>, task: Task) -> Result<Task> {
    let now = now_local();
    let mut task = task;
    task.prompt = task.prompt.trim().to_string();
    check(&task, now)?;
    if task.title.trim().is_empty() {
        task.title = title_of(&task.prompt);
    }
    task.title = task.title.trim().chars().take(MAX_TITLE_CHARS).collect();
    let saved = {
        let mut data = state.data.lock();
        state.ensure_running()?;
        if !data
            .projects
            .iter()
            .any(|project| project.id == task.project_id)
        {
            return Err(Error::not_found("project not found"));
        }
        let existing = data
            .scheduled_prompts
            .iter()
            .position(|item| item.id == task.id);
        match existing {
            Some(at) => {
                task.last_run = data.scheduled_prompts[at].last_run.clone();
                task.history = data.scheduled_prompts[at].history.clone();
            }
            None => {
                if data.scheduled_prompts.len() >= MAX_TASKS {
                    return Err(Error::new("invalid", "too many tasks"));
                }
                task.id = uuid::Uuid::new_v4().to_string();
                task.last_run = None;
                task.history = Vec::new();
            }
        }
        task.next_run = next_run(&task.schedule, now).map(stamp);
        match existing {
            Some(at) => data.scheduled_prompts[at] = task.clone(),
            None => data.scheduled_prompts.push(task.clone()),
        }
        task
    };
    state.persist()?;
    changed(&app);
    Ok(saved)
}

pub fn delete_task(app: AppHandle, state: State<'_, Arc<AppState>>, id: String) -> Result<()> {
    {
        let mut data = state.data.lock();
        state.ensure_running()?;
        let before = data.scheduled_prompts.len();
        data.scheduled_prompts.retain(|task| task.id != id);
        if data.scheduled_prompts.len() == before {
            return Err(Error::not_found("task not found"));
        }
    }
    state.persist()?;
    changed(&app);
    Ok(())
}

/// Switching a task on recomputes its next run from now, so it never runs for a time it was off.
pub fn set_task_enabled(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
    enabled: bool,
) -> Result<()> {
    {
        let mut data = state.data.lock();
        state.ensure_running()?;
        let task = data
            .scheduled_prompts
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| Error::not_found("task not found"))?;
        task.enabled = enabled;
        task.next_run = next_run(&task.schedule, now_local()).map(stamp);
    }
    state.persist()?;
    changed(&app);
    Ok(())
}

/// Starts the task's conversation now; its schedule is untouched. Returns the new session's id.
pub async fn run_task_now(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<Option<String>> {
    let state = state.inner().clone();
    let task = state
        .data
        .lock()
        .scheduled_prompts
        .iter()
        .find(|task| task.id == id)
        .cloned()
        .ok_or_else(|| Error::not_found("task not found"))?;
    if !begin(&task.id) {
        return Err(Error::new("busy", "the task is already starting"));
    }
    let (session, failure) = run(&app, &state, &task).await;
    match failure {
        Some(message) => Err(Error::agent(message)),
        None => Ok(session),
    }
}

// ------------------------------------------------------------------ the loop

static RUNNING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Marks a task as starting; false when it already is.
fn begin(id: &str) -> bool {
    RUNNING
        .lock()
        .get_or_insert_with(HashSet::new)
        .insert(id.to_string())
}

/// Starts the conversation and records the run. The caller has called `begin`.
async fn run(
    app: &AppHandle,
    state: &Arc<AppState>,
    task: &Task,
) -> (Option<String>, Option<String>) {
    let (session, failure) = crate::launch::launch(
        app,
        state,
        crate::launch::Launch {
            project_id: task.project_id.clone(),
            title: task.title.clone(),
            agent: task.provider.clone(),
            model: task.model.clone(),
            effort: task.effort.clone(),
            isolated_worktree: task.worktree,
            fast: task.fast,
            prompt: task.prompt.clone(),
            // Nobody is there to answer: unless the owner chose otherwise, anything that needs
            // approval waits in the conversation.
            approval: task.approval.unwrap_or(ApprovalMode::Ask),
            planning: false,
            delegated_by: None,
            share_worktree: None,
        },
    )
    .await;
    let finished = LastRun {
        at: stamp(now_local()),
        ok: failure.is_none() && session.is_some(),
        session_id: session.clone(),
    };
    {
        let mut data = state.data.lock();
        if let Some(item) = data
            .scheduled_prompts
            .iter_mut()
            .find(|item| item.id == task.id)
        {
            item.last_run = Some(finished.clone());
            item.history.push(finished);
            let extra = item.history.len().saturating_sub(MAX_HISTORY);
            item.history.drain(..extra);
        }
    }
    if let Err(error) = state.persist() {
        tracing::warn!(%error, "cannot save a task's run");
    }
    if let Some(running) = RUNNING.lock().as_mut() {
        running.remove(&task.id);
    }
    changed(app);
    (session, failure)
}

/// Claims and starts what is due. The first pass after launch is the missed-run catch-up.
async fn tick(app: &AppHandle, state: &Arc<AppState>) {
    let due = {
        let mut data = state.data.lock();
        claim_due(&mut data.scheduled_prompts, now_local())
    };
    if due.is_empty() {
        return;
    }
    if let Err(error) = state.persist() {
        tracing::warn!(%error, "cannot save the tasks' next runs");
    }
    changed(app);
    for task in due {
        if state.ensure_running().is_err() {
            return;
        }
        if begin(&task.id) {
            run(app, state, &task).await;
        }
    }
}

/// Starts the loop: a pass at launch for the runs missed while closed, then one a minute.
pub fn start(app: AppHandle) {
    crate::runtime::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        // Let the window connect and the projects load before the first conversation starts.
        tokio::time::sleep(Duration::from_secs(5)).await;
        loop {
            if state.ensure_running().is_err() {
                return;
            }
            tick(&app, &state).await;
            tokio::time::sleep(WAKE).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        parse_stamp(text).unwrap()
    }

    fn repeat(kind: RepeatKind, time: &str, weekdays: &[u8]) -> Schedule {
        Schedule::Repeat(Repeat {
            kind,
            time: time.into(),
            weekdays: weekdays.to_vec(),
            day: None,
            every: None,
            unit: None,
            start: None,
        })
    }

    fn next(schedule: &Schedule, after: &str) -> Option<String> {
        next_run(schedule, at(after)).map(stamp)
    }

    #[test]
    fn a_one_time_task_runs_once_and_never_after() {
        let once = Schedule::Once("2026-11-10T09:00".into());
        assert_eq!(
            next(&once, "2026-11-09T12:00:00").unwrap(),
            "2026-11-10T09:00:00"
        );
        assert_eq!(next(&once, "2026-11-10T09:00:00"), None);
        assert_eq!(next(&once, "2026-11-11T00:00:00"), None);
    }

    #[test]
    fn daily_runs_today_if_the_time_is_ahead_else_tomorrow() {
        let daily = repeat(RepeatKind::Daily, "09:00", &[]);
        assert_eq!(
            next(&daily, "2026-11-09T08:59:59").unwrap(),
            "2026-11-09T09:00:00"
        );
        assert_eq!(
            next(&daily, "2026-11-09T09:00:00").unwrap(),
            "2026-11-10T09:00:00"
        );
    }

    #[test]
    fn monthly_runs_on_its_day_and_a_short_month_uses_its_last() {
        let monthly = |day: u8| {
            Schedule::Repeat(Repeat {
                kind: RepeatKind::Monthly,
                time: "09:00".into(),
                weekdays: vec![],
                day: Some(day),
                every: None,
                unit: None,
                start: None,
            })
        };
        assert_eq!(
            next(&monthly(15), "2026-11-09T10:00:00").unwrap(),
            "2026-11-15T09:00:00"
        );
        assert_eq!(
            next(&monthly(15), "2026-11-15T09:00:00").unwrap(),
            "2026-12-15T09:00:00"
        );
        assert_eq!(
            next(&monthly(31), "2026-01-31T10:00:00").unwrap(),
            "2026-02-28T09:00:00"
        );
        assert_eq!(
            next(&monthly(31), "2026-12-31T10:00:00").unwrap(),
            "2027-01-31T09:00:00"
        );
        assert_eq!(next(&monthly(0), "2026-11-09T10:00:00"), None);
        let broken = Schedule::Repeat(Repeat {
            kind: RepeatKind::Monthly,
            time: "09:00".into(),
            weekdays: vec![],
            day: None,
            every: None,
            unit: None,
            start: None,
        });
        assert_eq!(next(&broken, "2026-11-09T10:00:00"), None);
    }

    #[test]
    fn weekdays_skip_the_weekend() {
        let rule = repeat(RepeatKind::Weekdays, "09:00", &[]);
        // 2026-11-13 is a Friday.
        assert_eq!(
            next(&rule, "2026-11-13T10:00:00").unwrap(),
            "2026-11-16T09:00:00"
        );
        assert_eq!(
            next(&rule, "2026-11-14T08:00:00").unwrap(),
            "2026-11-16T09:00:00"
        );
        assert_eq!(
            next(&rule, "2026-11-15T23:59:00").unwrap(),
            "2026-11-16T09:00:00"
        );
        assert_eq!(
            next(&rule, "2026-11-12T10:00:00").unwrap(),
            "2026-11-13T09:00:00"
        );
    }

    fn custom(time: &str, every: u32, unit: RepeatUnit, start: &str) -> Schedule {
        Schedule::Repeat(Repeat {
            kind: RepeatKind::Custom,
            time: time.into(),
            weekdays: vec![],
            day: None,
            every: Some(every),
            unit: Some(unit),
            start: Some(start.into()),
        })
    }

    #[test]
    fn custom_repeats_count_whole_steps_from_the_start() {
        // 2026-11-02 is a Monday; every 2 weeks: the 2nd, the 16th, the 30th.
        let biweekly = custom("14:00", 2, RepeatUnit::Weeks, "2026-11-02");
        assert_eq!(
            next(&biweekly, "2026-11-02T10:00:00").as_deref(),
            Some("2026-11-02T14:00:00")
        );
        assert_eq!(
            next(&biweekly, "2026-11-02T14:00:00").as_deref(),
            Some("2026-11-16T14:00:00")
        );
        assert_eq!(
            next(&biweekly, "2026-11-17T00:00:00").as_deref(),
            Some("2026-11-30T14:00:00")
        );
        // Before the start, the first run is the start day itself.
        assert_eq!(
            next(&biweekly, "2026-10-01T00:00:00").as_deref(),
            Some("2026-11-02T14:00:00")
        );
        // Every 3 days.
        let days = custom("08:30", 3, RepeatUnit::Days, "2026-11-09");
        assert_eq!(
            next(&days, "2026-11-09T09:00:00").as_deref(),
            Some("2026-11-12T08:30:00")
        );
        // The widest step still finds its day.
        let wide = custom("08:30", 30, RepeatUnit::Weeks, "2026-11-09");
        assert_eq!(
            next(&wide, "2026-11-09T09:00:00").as_deref(),
            Some("2027-06-07T08:30:00")
        );
    }

    #[test]
    fn a_repeat_waits_for_its_first_day() {
        let mut daily = Repeat {
            kind: RepeatKind::Daily,
            time: "09:00".into(),
            weekdays: vec![],
            day: None,
            every: None,
            unit: None,
            start: Some("2026-11-20".into()),
        };
        assert_eq!(
            next(&Schedule::Repeat(daily.clone()), "2026-11-09T12:00:00").as_deref(),
            Some("2026-11-20T09:00:00")
        );
        daily.start = Some("2026-11-01".into());
        assert_eq!(
            next(&Schedule::Repeat(daily), "2026-11-09T12:00:00").as_deref(),
            Some("2026-11-10T09:00:00")
        );
    }

    #[test]
    fn a_custom_repeat_needs_its_step_unit_and_start() {
        let now = at("2026-11-09T12:00:00");
        let good = task(
            "a",
            custom("09:00", 2, RepeatUnit::Weeks, "2026-11-10"),
            None,
            true,
        );
        assert!(check(&good, now).is_ok());
        for broken in [
            custom("09:00", 0, RepeatUnit::Days, "2026-11-10"),
            custom("09:00", 31, RepeatUnit::Days, "2026-11-10"),
            custom("09:00", 2, RepeatUnit::Days, "nope"),
        ] {
            assert!(check(&task("a", broken, None, true), now).is_err());
        }
        let value = serde_json::to_value(&good).unwrap();
        assert_eq!(value["schedule"]["repeat"]["kind"], "custom");
        assert_eq!(value["schedule"]["repeat"]["unit"], "weeks");
        assert_eq!(value["schedule"]["repeat"]["start"], "2026-11-10");
        assert!(value.get("worktree").is_none() && value.get("fast").is_none());
        let back: Task = serde_json::from_value(value).unwrap();
        assert_eq!(back, good);
    }

    #[test]
    fn worktree_and_fast_default_to_off_and_round_trip() {
        let on: Task = serde_json::from_value(json!({
            "prompt": "x", "projectId": "p", "provider": "claude",
            "worktree": true, "fast": true,
            "schedule": { "once": "2026-11-10T09:00" }
        }))
        .unwrap();
        assert!(on.worktree && on.fast);
        let value = serde_json::to_value(&on).unwrap();
        assert_eq!(value["worktree"], true);
        assert_eq!(value["fast"], true);
    }

    #[test]
    fn weekly_follows_the_chosen_days() {
        // Tuesday and Saturday.
        let rule = repeat(RepeatKind::Weekly, "18:30", &[1, 5]);
        assert_eq!(
            next(&rule, "2026-11-09T08:00:00").unwrap(),
            "2026-11-10T18:30:00"
        );
        assert_eq!(
            next(&rule, "2026-11-10T18:30:00").unwrap(),
            "2026-11-14T18:30:00"
        );
        assert_eq!(
            next(&rule, "2026-11-14T18:30:00").unwrap(),
            "2026-11-17T18:30:00"
        );
        // The chosen day with its time passed: the same day next week.
        let monday = repeat(RepeatKind::Weekly, "09:00", &[0]);
        assert_eq!(
            next(&monday, "2026-11-09T09:00:00").unwrap(),
            "2026-11-16T09:00:00"
        );
    }

    #[test]
    fn rules_cross_month_and_year_ends_and_leap_days() {
        let daily = repeat(RepeatKind::Daily, "09:00", &[]);
        assert_eq!(
            next(&daily, "2026-11-30T10:00:00").unwrap(),
            "2026-12-01T09:00:00"
        );
        assert_eq!(
            next(&daily, "2026-12-31T10:00:00").unwrap(),
            "2027-01-01T09:00:00"
        );
        assert_eq!(
            next(&daily, "2028-02-28T10:00:00").unwrap(),
            "2028-02-29T09:00:00"
        );
        assert_eq!(
            next(&daily, "2027-02-28T10:00:00").unwrap(),
            "2027-03-01T09:00:00"
        );
        // 2026-10-30 is a Friday; the next weekday run is Monday the 2nd.
        let weekdays = repeat(RepeatKind::Weekdays, "09:00", &[]);
        assert_eq!(
            next(&weekdays, "2026-10-30T10:00:00").unwrap(),
            "2026-11-02T09:00:00"
        );
        // 2026-12-31 is a Thursday, so the next day, a Friday, is a run.
        assert_eq!(
            next(&weekdays, "2026-12-31T10:00:00").unwrap(),
            "2027-01-01T09:00:00"
        );
    }

    #[test]
    fn a_broken_rule_never_runs() {
        let start = "2026-11-09T00:00:00";
        assert_eq!(next(&repeat(RepeatKind::Daily, "25:00", &[]), start), None);
        assert_eq!(next(&repeat(RepeatKind::Weekly, "09:00", &[]), start), None);
        assert_eq!(next(&Schedule::Once("soon".into()), start), None);
    }

    #[test]
    fn occurrences_fill_a_range_and_respect_its_ends() {
        let rule = repeat(RepeatKind::Weekdays, "09:00", &[]);
        let found = occurrences(
            &rule,
            at("2026-11-09T00:00:00"),
            at("2026-11-16T00:00:00"),
            100,
        );
        let days: Vec<String> = found.into_iter().map(stamp).collect();
        assert_eq!(
            days,
            [
                "2026-11-09T09:00:00",
                "2026-11-10T09:00:00",
                "2026-11-11T09:00:00",
                "2026-11-12T09:00:00",
                "2026-11-13T09:00:00"
            ]
        );
        // `from` is included, `to` is not.
        let edge = occurrences(
            &rule,
            at("2026-11-09T09:00:00"),
            at("2026-11-10T09:00:00"),
            100,
        );
        assert_eq!(edge.len(), 1);
        let capped = occurrences(
            &rule,
            at("2026-11-09T00:00:00"),
            at("2026-12-09T00:00:00"),
            3,
        );
        assert_eq!(capped.len(), 3);
    }

    fn task(id: &str, schedule: Schedule, next: Option<&str>, enabled: bool) -> Task {
        Task {
            id: id.into(),
            title: id.into(),
            prompt: "do it".into(),
            project_id: "p".into(),
            provider: AgentProviderId::Claude,
            model: None,
            effort: None,
            approval: None,
            fast: false,
            worktree: false,
            schedule,
            enabled,
            last_run: None,
            history: vec![],
            next_run: next.map(str::to_string),
        }
    }

    #[test]
    fn a_missed_run_is_claimed_once_and_moves_past_now() {
        let daily = repeat(RepeatKind::Daily, "09:00", &[]);
        // Closed for three days: the next run was Nov 6 at 9:00.
        let mut tasks = vec![task("a", daily, Some("2026-11-06T09:00:00"), true)];
        let now = at("2026-11-09T12:00:00");
        let due = claim_due(&mut tasks, now);
        assert_eq!(due.len(), 1);
        assert_eq!(tasks[0].next_run.as_deref(), Some("2026-11-10T09:00:00"));
        // The same moment again claims nothing.
        assert!(claim_due(&mut tasks, now).is_empty());
    }

    #[test]
    fn paused_and_not_yet_due_tasks_are_left_alone() {
        let daily = repeat(RepeatKind::Daily, "09:00", &[]);
        let mut tasks = vec![
            task("paused", daily.clone(), Some("2026-11-06T09:00:00"), false),
            task("later", daily, Some("2026-11-10T09:00:00"), true),
        ];
        assert!(claim_due(&mut tasks, at("2026-11-09T12:00:00")).is_empty());
        assert_eq!(tasks[0].next_run.as_deref(), Some("2026-11-06T09:00:00"));
    }

    #[test]
    fn a_one_time_task_has_no_next_run_once_claimed() {
        let once = Schedule::Once("2026-11-09T09:00".into());
        let mut tasks = vec![task("once", once, Some("2026-11-09T09:00:00"), true)];
        assert_eq!(claim_due(&mut tasks, at("2026-11-09T09:00:30")).len(), 1);
        assert_eq!(tasks[0].next_run, None);
    }

    #[test]
    fn the_list_has_past_runs_from_history_and_planned_runs_from_now() {
        let daily = repeat(RepeatKind::Daily, "09:00", &[]);
        let mut item = task("a", daily, None, true);
        item.history = vec![
            LastRun {
                at: "2026-11-08T09:00:00".into(),
                ok: true,
                session_id: Some("s1".into()),
            },
            LastRun {
                at: "2026-11-09T09:00:00".into(),
                ok: false,
                session_id: None,
            },
        ];
        let now = at("2026-11-09T12:00:00");
        let list = list_at(
            &[item],
            now,
            at("2026-11-08T00:00:00"),
            at("2026-11-11T00:00:00"),
        );
        let seen: Vec<(&str, &str)> = list
            .occurrences
            .iter()
            .map(|o| (o.at.as_str(), o.state))
            .collect();
        assert_eq!(
            seen,
            [
                ("2026-11-08T09:00:00", "done"),
                ("2026-11-09T09:00:00", "failed"),
                ("2026-11-10T09:00:00", "planned"),
            ]
        );
        assert_eq!(list.now, "2026-11-09T12:00:00");
    }

    #[test]
    fn titles_come_from_the_first_line() {
        assert_eq!(
            title_of("\n  Summarise the news  \nmore"),
            "Summarise the news"
        );
        assert_eq!(title_of(&"x".repeat(200)).chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn a_task_checks_its_prompt_schedule_and_weekdays() {
        let now = at("2026-11-09T12:00:00");
        let good = task("a", repeat(RepeatKind::Weekly, "09:00", &[0]), None, true);
        assert!(check(&good, now).is_ok());
        let mut empty = good.clone();
        empty.prompt = "  ".into();
        assert!(check(&empty, now).is_err());
        let no_days = task("a", repeat(RepeatKind::Weekly, "09:00", &[]), None, true);
        assert!(check(&no_days, now).is_err());
        let past = task("a", Schedule::Once("2026-11-09T11:00".into()), None, true);
        assert!(check(&past, now).is_err());
        let future = task("a", Schedule::Once("2026-11-09T13:00".into()), None, true);
        assert!(check(&future, now).is_ok());
    }

    #[test]
    fn a_task_round_trips_through_json_in_the_documented_shape() {
        let item = task(
            "a",
            repeat(RepeatKind::Weekdays, "09:00", &[]),
            Some("2026-11-10T09:00:00"),
            true,
        );
        let value = serde_json::to_value(&item).unwrap();
        assert_eq!(value["schedule"]["repeat"]["kind"], "weekdays");
        assert_eq!(value["projectId"], "p");
        assert_eq!(value["nextRun"], "2026-11-10T09:00:00");
        let back: Task = serde_json::from_value(value).unwrap();
        assert_eq!(back, item);
        let once: Task = serde_json::from_value(json!({
            "prompt": "x", "projectId": "p", "provider": "claude",
            "schedule": { "once": "2026-11-10T09:00" }
        }))
        .unwrap();
        assert!(once.enabled && once.id.is_empty());
    }
}
