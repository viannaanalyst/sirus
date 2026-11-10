//! Tarefas (ADR-200): prompts that start a conversation by themselves, once or on a repeat rule.
//! A page in its own tab, like Pull requests (shell_terminal.rs), opened from the sidebar's "Tarefas"
//! row: a header (the period at the left, Dia | Semana | Mês and Nova tarefa at the right), "Minhas tarefas" at the left
//! and a calendar (Mês by default, Semana, Dia) of the runs the host lists, a popover per run, and
//! the "Nova tarefa" modal. The host
//! (`scheduled_prompts.rs`) owns the tasks and the schedule maths; this module only draws what
//! `list_tasks` answers and sends `save_task`, `delete_task`, `set_task_enabled` and
//! `run_task_now`. Times are the host's local wall-clock time, so the "now" line is the host's.
//!
//! The date and validation logic is pure (`rule_matches`, `upcoming`, `Sheet::problems`,
//! `step_clock`, `month_cap`, `slot_default`, …) and tested below.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;
use crate::motion::eased;
use chrono::{Datelike, Duration as Span, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use gpui::{Animation, AnimationExt};
use std::cell::RefCell;

const HOUR_H: f32 = 56.;
const GUTTER: f32 = 56.;
/// A run is drawn as half an hour tall.
const BLOCK_MIN: i64 = 30;
const SWITCH_ID: usize = 910_000;
const NOW_LINE: u32 = 0xea4335;
const VIEWS: [View; 3] = [View::Day, View::Week, View::Month];
/// Height of the month's weekday row, and what a day cell spends above its chips.
const MONTH_HEAD: f32 = 31.;
const CELL_TOP: f32 = 30.;
const CHIP_STEP: f32 = 21.;
/// The window's title bar, header, weekday row and footer, taken off the window's height to
/// guess how tall a month's cells are.
const PAGE_CHROME: f32 = crate::theme::TITLE_BAR + crate::theme::FOOTER + 54.;
const MODAL_WIDTH: f32 = 820.;
const LABEL_WIDTH: f32 = 104.;
/// The space between a field and the list that opens from it.
const POPUP_GAP: f32 = 6.;
/// The least room a list leaves to the window's edge.
const POPUP_MARGIN: f32 = 12.;
/// A list's fade and slide, in and out, in seconds.
const POPUP_MOTION: f32 = 0.16;
/// How far a list slides while it fades, in px.
const POPUP_SLIDE: f32 = 6.;
/// The prompt field grows to this height, then scrolls.
const PROMPT_MAX_H: f32 = 220.;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum View {
    Day,
    Week,
    Month,
}

impl View {
    fn label(self) -> &'static str {
        match self {
            View::Day => t("Dia"),
            View::Week => t("Semana"),
            View::Month => t("Mês"),
        }
    }
}

// ---------------------------------------------------------------- what the host sends

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Unit {
    Days,
    Weeks,
}

impl Unit {
    fn key(self) -> &'static str {
        match self {
            Unit::Days => "days",
            Unit::Weeks => "weeks",
        }
    }

    fn of(key: &str) -> Option<Self> {
        match key {
            "days" => Some(Unit::Days),
            "weeks" => Some(Unit::Weeks),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Unit::Days => t("dias"),
            Unit::Weeks => t("semanas"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Rule {
    Once(NaiveDateTime),
    Daily(NaiveTime),
    Weekdays(NaiveTime),
    /// 0 is Monday.
    Weekly(NaiveTime, Vec<u8>),
    /// The day of the month, 1 to 31; a shorter month uses its last day.
    Monthly(NaiveTime, u8),
    /// Every `every` days or weeks counted from `start`.
    Custom {
        time: NaiveTime,
        every: u32,
        unit: Unit,
        start: NaiveDate,
    },
}

impl Rule {
    fn time(&self) -> NaiveTime {
        match self {
            Rule::Once(at) => at.time(),
            Rule::Daily(time)
            | Rule::Weekdays(time)
            | Rule::Weekly(time, _)
            | Rule::Monthly(time, _)
            | Rule::Custom { time, .. } => *time,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Task {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub project_id: String,
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub approval: Option<String>,
    pub fast: bool,
    pub worktree: bool,
    pub rule: Rule,
    /// The first day a repeating rule may run.
    pub start: Option<NaiveDate>,
    pub enabled: bool,
    pub last_session: Option<String>,
    /// When the last run started and whether it worked.
    pub last_run: Option<(NaiveDateTime, bool)>,
    pub next: Option<NaiveDateTime>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Run {
    Planned,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Occ {
    task: String,
    at: NaiveDateTime,
    run: Run,
    session: Option<String>,
}

fn parse_at(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M"))
        .ok()
}

fn parse_clock(text: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(text, "%H:%M").ok()
}

fn parse_day(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

fn parse_rule(value: &Value) -> Option<Rule> {
    if let Some(at) = value["once"].as_str() {
        return parse_at(at).map(Rule::Once);
    }
    let repeat = &value["repeat"];
    let time = parse_clock(repeat["time"].as_str()?)?;
    match repeat["kind"].as_str()? {
        "daily" => Some(Rule::Daily(time)),
        "weekdays" => Some(Rule::Weekdays(time)),
        "weekly" => {
            let mut days: Vec<u8> = repeat["weekdays"]
                .as_array()?
                .iter()
                .filter_map(|day| day.as_u64().filter(|day| *day < 7).map(|day| day as u8))
                .collect();
            days.sort_unstable();
            days.dedup();
            Some(Rule::Weekly(time, days))
        }
        "monthly" => {
            let day = repeat["day"]
                .as_u64()
                .filter(|day| (1..=31).contains(day))?;
            Some(Rule::Monthly(time, day as u8))
        }
        "custom" => Some(Rule::Custom {
            time,
            every: repeat["every"]
                .as_u64()
                .filter(|every| (1..=30).contains(every))? as u32,
            unit: Unit::of(repeat["unit"].as_str()?)?,
            start: parse_day(repeat["start"].as_str()?)?,
        }),
        _ => None,
    }
}

/// The first day a repeating rule may run, when the host has one.
fn parse_start(value: &Value) -> Option<NaiveDate> {
    parse_day(value["repeat"]["start"].as_str()?)
}

/// The rule as `save_task` takes it. `start` is the first day of a repeating rule.
pub(crate) fn rule_json(rule: &Rule, start: Option<NaiveDate>) -> Value {
    let clock = |time: &NaiveTime| time.format("%H:%M").to_string();
    let mut value = match rule {
        Rule::Once(at) => {
            return json!({ "once": at.format("%Y-%m-%dT%H:%M").to_string() });
        }
        Rule::Daily(time) => json!({ "repeat": { "kind": "daily", "time": clock(time) } }),
        Rule::Weekdays(time) => json!({ "repeat": { "kind": "weekdays", "time": clock(time) } }),
        Rule::Weekly(time, days) => {
            json!({ "repeat": { "kind": "weekly", "time": clock(time), "weekdays": days } })
        }
        Rule::Monthly(time, day) => {
            json!({ "repeat": { "kind": "monthly", "time": clock(time), "day": day } })
        }
        Rule::Custom {
            time, every, unit, ..
        } => json!({ "repeat": {
            "kind": "custom", "time": clock(time), "every": every, "unit": unit.key(),
        } }),
    };
    let start = match rule {
        Rule::Custom { start, .. } => Some(*start),
        _ => start,
    };
    if let Some(start) = start {
        value["repeat"]["start"] = json!(start.format("%Y-%m-%d").to_string());
    }
    value
}

/// The host's `list_tasks` answer: tasks, the runs in the asked range and the host's "now".
pub(crate) fn parse_list(value: &Value) -> (Vec<Task>, Vec<Occ>, Option<NaiveDateTime>) {
    let tasks = value["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|task| {
            Some(Task {
                id: task["id"].as_str()?.to_owned(),
                title: task["title"].as_str().unwrap_or("").to_owned(),
                prompt: task["prompt"].as_str().unwrap_or("").to_owned(),
                project_id: task["projectId"].as_str().unwrap_or("").to_owned(),
                provider: task["provider"].as_str().unwrap_or("claude").to_owned(),
                model: task["model"].as_str().map(str::to_owned),
                effort: task["effort"].as_str().map(str::to_owned),
                approval: task["approval"].as_str().map(str::to_owned),
                fast: task["fast"].as_bool().unwrap_or(false),
                worktree: task["worktree"].as_bool().unwrap_or(false),
                rule: parse_rule(&task["schedule"])?,
                start: parse_start(&task["schedule"]),
                enabled: task["enabled"].as_bool().unwrap_or(true),
                last_session: task["lastRun"]["sessionId"].as_str().map(str::to_owned),
                last_run: task["lastRun"]["at"]
                    .as_str()
                    .and_then(parse_at)
                    .map(|at| (at, task["lastRun"]["ok"].as_bool().unwrap_or(false))),
                next: task["nextRun"].as_str().and_then(parse_at),
            })
        })
        .collect();
    let occurrences = value["occurrences"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|occ| {
            Some(Occ {
                task: occ["taskId"].as_str()?.to_owned(),
                at: parse_at(occ["at"].as_str()?)?,
                run: match occ["state"].as_str()? {
                    "done" => Run::Done,
                    "failed" => Run::Failed,
                    _ => Run::Planned,
                },
                session: occ["sessionId"].as_str().map(str::to_owned),
            })
        })
        .collect();
    (tasks, occurrences, value["now"].as_str().and_then(parse_at))
}

/// Tasks that will still run: on, with a next run.
pub(crate) fn active_count(tasks: &[Task]) -> usize {
    tasks
        .iter()
        .filter(|task| task.enabled && task.next.is_some())
        .count()
}

/// A paused task's coming runs are not drawn (it will not run them); its past ones are.
fn drawn(run: Run, task_enabled: bool) -> bool {
    run != Run::Planned || task_enabled
}

// ---------------------------------------------------------------- words

const WEEKDAYS: [&str; 7] = ["Seg", "Ter", "Qua", "Qui", "Sex", "Sáb", "Dom"];
const EVERY_WEEKDAY: [&str; 7] = [
    "Toda segunda",
    "Toda terça",
    "Toda quarta",
    "Toda quinta",
    "Toda sexta",
    "Todo sábado",
    "Todo domingo",
];
const WEEKDAY_NAMES_PT: [&str; 7] = [
    "segunda", "terça", "quarta", "quinta", "sexta", "sábado", "domingo",
];
const WEEKDAY_NAMES_EN: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const WEEKDAY_LETTERS_PT: [&str; 7] = ["S", "T", "Q", "Q", "S", "S", "D"];
const WEEKDAY_LETTERS_EN: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];
const MONTHS: [&str; 12] = [
    "jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez",
];
const MONTH_NAMES: [&str; 12] = [
    "Janeiro",
    "Fevereiro",
    "Março",
    "Abril",
    "Maio",
    "Junho",
    "Julho",
    "Agosto",
    "Setembro",
    "Outubro",
    "Novembro",
    "Dezembro",
];

/// A weekday's whole name, "segunda" or "Monday" (0 is Monday).
fn weekday_name(day: usize) -> &'static str {
    if crate::i18n::is_en() {
        WEEKDAY_NAMES_EN[day % 7]
    } else {
        WEEKDAY_NAMES_PT[day % 7]
    }
}

/// The letter on a weekday chip (0 is Monday).
fn weekday_letter(day: usize) -> &'static str {
    if crate::i18n::is_en() {
        WEEKDAY_LETTERS_EN[day % 7]
    } else {
        WEEKDAY_LETTERS_PT[day % 7]
    }
}

/// "Novembro de 2026", or "November 2026" in English.
pub(crate) fn month_title(date: NaiveDate) -> String {
    let name = MONTH_NAMES[date.month0() as usize];
    if crate::i18n::is_en() {
        format!("{} {}", t(name), date.year())
    } else {
        format!("{name} de {}", date.year())
    }
}

/// "9 de nov", or "Nov 9" in English.
pub(crate) fn short_date(date: NaiveDate) -> String {
    let month = t(MONTHS[date.month0() as usize]);
    if crate::i18n::is_en() {
        format!("{month} {}", date.day())
    } else {
        format!("{} de {month}", date.day())
    }
}

/// "segunda, 9 de nov" with the year when it is not `today`'s: the label of a day.
pub(crate) fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    let weekday = weekday_name(date.weekday().num_days_from_monday() as usize);
    let year = date.year() != today.year();
    if crate::i18n::is_en() {
        let tail = if year {
            format!(", {}", date.year())
        } else {
            String::new()
        };
        format!("{weekday}, {}{tail}", short_date(date))
    } else {
        let tail = if year {
            format!(" de {}", date.year())
        } else {
            String::new()
        };
        format!("{weekday}, {}{tail}", short_date(date))
    }
}

/// The header's label for the period on screen.
pub(crate) fn period_label(
    view: View,
    anchor: NaiveDate,
    today: NaiveDate,
    monday_first: bool,
) -> String {
    match view {
        View::Month => month_title(anchor),
        View::Day => day_label(anchor, today),
        View::Week => {
            let first = week_start(anchor, monday_first);
            let last = first + Span::days(6);
            let (a, b) = (first, last);
            if crate::i18n::is_en() {
                if a.year() != b.year() {
                    format!(
                        "{}, {} – {}, {}",
                        short_date(a),
                        a.year(),
                        short_date(b),
                        b.year()
                    )
                } else if a.month() != b.month() {
                    format!("{} – {}, {}", short_date(a), short_date(b), b.year())
                } else {
                    format!("{} – {}, {}", short_date(a), b.day(), b.year())
                }
            } else if a.year() != b.year() {
                format!(
                    "{} de {} – {} de {}",
                    short_date(a),
                    a.year(),
                    short_date(b),
                    b.year()
                )
            } else if a.month() != b.month() {
                format!("{} – {} de {}", short_date(a), short_date(b), b.year())
            } else {
                format!("{} – {} de {}", a.day(), short_date(b), b.year())
            }
        }
    }
}

/// `9:00`: the hour has no leading zero.
fn clock(time: NaiveTime) -> String {
    format!("{}:{:02}", time.hour(), time.minute())
}

/// The rule without its time: "Dias úteis", "Seg, Qua e Sex", "A cada 2 semanas, segunda".
fn rule_head(rule: &Rule) -> String {
    match rule {
        Rule::Once(at) => tf(
            "Uma vez, {} {}",
            &[&at.day(), &t(MONTHS[at.month0() as usize])],
        ),
        Rule::Daily(_) => t("Todo dia").to_owned(),
        Rule::Weekdays(_) => t("Dias úteis").to_owned(),
        Rule::Monthly(_, day) => tf("Todo mês, dia {}", &[day]),
        Rule::Weekly(_, days) => {
            let days: Vec<u8> = days.iter().copied().filter(|day| *day < 7).collect();
            if days.len() == 7 {
                return t("Todo dia").to_owned();
            }
            if days.len() == 5 && days.iter().all(|day| *day < 5) {
                return t("Dias úteis").to_owned();
            }
            let names: Vec<&str> = days
                .iter()
                .map(|day| {
                    if days.len() == 1 {
                        t(EVERY_WEEKDAY[*day as usize])
                    } else {
                        t(WEEKDAYS[*day as usize])
                    }
                })
                .collect();
            match names.split_last() {
                None => String::new(),
                Some((only, [])) => (*only).to_owned(),
                Some((last, head)) => format!("{} {} {last}", head.join(", "), t("e")),
            }
        }
        Rule::Custom {
            every, unit, start, ..
        } => {
            let step = match (unit, every) {
                (Unit::Days, 1) => t("A cada dia").to_owned(),
                (Unit::Days, n) => tf("A cada {} dias", &[n]),
                (Unit::Weeks, 1) => t("A cada semana").to_owned(),
                (Unit::Weeks, n) => tf("A cada {} semanas", &[n]),
            };
            match unit {
                Unit::Days => step,
                Unit::Weeks => format!(
                    "{step}, {}",
                    weekday_name(start.weekday().num_days_from_monday() as usize)
                ),
            }
        }
    }
}

/// The rule in words: "Dias úteis às 9:00".
pub(crate) fn rule_words(rule: &Rule) -> String {
    tf("{} às {}", &[&rule_head(rule), &clock(rule.time())])
}

/// The rule in a list row: "Dias úteis · 9:00".
pub(crate) fn rule_short(rule: &Rule) -> String {
    format!("{} · {}", rule_head(rule), clock(rule.time()))
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------- dates and layout

/// Weeks start on Monday in English and on Sunday ("Dom" to "Sáb") in Portuguese.
fn monday_first() -> bool {
    crate::i18n::is_en()
}

/// The first day of `date`'s week.
pub(crate) fn week_start(date: NaiveDate, monday_first: bool) -> NaiveDate {
    let back = if monday_first {
        date.weekday().num_days_from_monday()
    } else {
        date.weekday().num_days_from_sunday()
    };
    date - Span::days(i64::from(back))
}

/// The column headings of a week as indexes into `WEEKDAYS` (0 is Monday), in the order shown.
pub(crate) fn weekday_order(monday_first: bool) -> [usize; 7] {
    if monday_first {
        [0, 1, 2, 3, 4, 5, 6]
    } else {
        [6, 0, 1, 2, 3, 4, 5]
    }
}

/// A month's grid: whole weeks from the one holding the 1st to the one holding the last day
/// (four to six of them).
pub(crate) fn month_grid(anchor: NaiveDate, monday_first: bool) -> Vec<NaiveDate> {
    let first = anchor.with_day(1).unwrap_or(anchor);
    let start = week_start(first, monday_first);
    let last = step_end(first);
    let end = week_start(last, monday_first) + Span::days(6);
    let count = (end - start).num_days() + 1;
    (0..count).map(|day| start + Span::days(day)).collect()
}

/// The days a view shows: one, seven, or a month's weeks.
pub(crate) fn view_days(view: View, anchor: NaiveDate, monday_first: bool) -> Vec<NaiveDate> {
    match view {
        View::Day => vec![anchor],
        View::Week => {
            let first = week_start(anchor, monday_first);
            (0..7).map(|day| first + Span::days(day)).collect()
        }
        View::Month => month_grid(anchor, monday_first),
    }
}

/// The anchor one step away: a day, a week, or a month. Stepping to the month of `today` lands
/// on today, so "Hoje" and the arrows agree.
pub(crate) fn step_anchor(view: View, anchor: NaiveDate, forward: bool) -> NaiveDate {
    let sign = if forward { 1 } else { -1 };
    match view {
        View::Day => anchor + Span::days(sign),
        View::Week => anchor + Span::days(7 * sign),
        View::Month => {
            let months = anchor.year() * 12 + anchor.month0() as i32 + sign as i32;
            let (year, month) = (months.div_euclid(12), months.rem_euclid(12) as u32 + 1);
            NaiveDate::from_ymd_opt(year, month, 1)
                .map(|first| {
                    let last = step_end(first);
                    first
                        .with_day(anchor.day().min(last.day()))
                        .unwrap_or(first)
                })
                .unwrap_or(anchor)
        }
    }
}

/// The last day of the month `first` starts.
fn step_end(first: NaiveDate) -> NaiveDate {
    let (year, month) = if first.month() == 12 {
        (first.year() + 1, 1)
    } else {
        (first.year(), first.month() + 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1).map_or(first, |next| next - Span::days(1))
}

/// The first day of the month `anchor` is in.
fn month_first(anchor: NaiveDate) -> NaiveDate {
    anchor.with_day(1).unwrap_or(anchor)
}

/// The first of the month `by` months from `anchor`'s.
pub(crate) fn add_months(anchor: NaiveDate, by: i32) -> NaiveDate {
    let months = anchor.year() * 12 + anchor.month0() as i32 + by;
    NaiveDate::from_ymd_opt(months.div_euclid(12), months.rem_euclid(12) as u32 + 1, 1)
        .unwrap_or(anchor)
}

/// How many chips a month's day cell can hold: what is left under the day number, in whole chips,
/// and at least one.
pub(crate) fn month_cap(cell_height: f32) -> usize {
    (((cell_height - CELL_TOP) / CHIP_STEP).floor().max(1.)) as usize
}

/// What a day cell draws of `count` runs: how many chips, and how many are left for "+N mais".
pub(crate) fn month_split(count: usize, cap: usize) -> (usize, usize) {
    if count <= cap {
        (count, 0)
    } else {
        let shown = cap.saturating_sub(1);
        (shown, count - shown)
    }
}

/// Lane and lane count of each start (minutes into the day, sorted) so that runs closer than a
/// block's length sit side by side instead of on top of each other.
pub(crate) fn lanes(starts: &[i64]) -> Vec<(usize, usize)> {
    let mut out = vec![(0, 1); starts.len()];
    let mut cluster: Vec<usize> = Vec::new();
    let mut ends: Vec<i64> = Vec::new();
    let finish = |cluster: &mut Vec<usize>, ends: &mut Vec<i64>, out: &mut Vec<(usize, usize)>| {
        for index in cluster.drain(..) {
            out[index].1 = ends.len().max(1);
        }
        ends.clear();
    };
    for (index, start) in starts.iter().enumerate() {
        if ends.iter().all(|end| *end <= *start) {
            finish(&mut cluster, &mut ends, &mut out);
        }
        let lane = ends
            .iter()
            .position(|end| *end <= *start)
            .unwrap_or(ends.len());
        if lane == ends.len() {
            ends.push(start + BLOCK_MIN);
        } else {
            ends[lane] = start + BLOCK_MIN;
        }
        out[index].0 = lane;
        cluster.push(index);
    }
    finish(&mut cluster, &mut ends, &mut out);
    out
}

/// Whether a repeating (or one-off) rule has a run on `date`. `start` is the first day a repeat
/// may run.
pub(crate) fn rule_matches(rule: &Rule, start: Option<NaiveDate>, date: NaiveDate) -> bool {
    if start.is_some_and(|start| date < start) {
        return false;
    }
    let weekday = date.weekday().num_days_from_monday() as u8;
    match rule {
        Rule::Once(at) => at.date() == date,
        Rule::Daily(_) => true,
        Rule::Weekdays(_) => weekday < 5,
        Rule::Weekly(_, days) => days.contains(&weekday),
        Rule::Monthly(_, day) => {
            let last = step_end(month_first(date)).day();
            date.day() == u32::from(*day).min(last)
        }
        Rule::Custom {
            every, unit, start, ..
        } => {
            let step = i64::from(*every)
                * match unit {
                    Unit::Days => 1,
                    Unit::Weeks => 7,
                };
            let since = (date - *start).num_days();
            step > 0 && since >= 0 && since % step == 0
        }
    }
}

/// The next `limit` runs after `now`, from `start` on: the "Próximas" list.
pub(crate) fn upcoming(
    rule: &Rule,
    start: Option<NaiveDate>,
    now: NaiveDateTime,
    limit: usize,
) -> Vec<NaiveDateTime> {
    if let Rule::Once(at) = rule {
        return if *at > now { vec![*at] } else { Vec::new() };
    }
    let first = start.map_or(now.date(), |start| start.max(now.date()));
    (0..800)
        .map(|offset| first + Span::days(offset))
        .filter(|date| rule_matches(rule, start, *date))
        .map(|date| date.and_time(rule.time()))
        .filter(|at| *at > now)
        .take(limit)
        .collect()
}

/// Where a list of `size` opens for a field at `field`: directly below it, `POPUP_GAP` away and
/// with its left edge on the field's, or above the field when the window has no room below.
pub(crate) fn popover_origin(
    field: Bounds<Pixels>,
    size: gpui::Size<Pixels>,
    window_height: Pixels,
) -> gpui::Point<Pixels> {
    let (gap, margin) = (px(POPUP_GAP), px(POPUP_MARGIN));
    let below = field.bottom() + gap;
    let above = field.top() - gap - size.height;
    let fits_below = below + size.height <= window_height - margin;
    let top = if fits_below {
        below
    } else if above >= margin {
        above
    } else if window_height - field.bottom() >= field.top() {
        // Neither side has room: the roomier one, kept inside the window.
        below.min(window_height - margin - size.height).max(margin)
    } else {
        above.max(margin)
    };
    gpui::point(field.origin.x, top)
}

/// A time field's step, as the arrows move it: hours wrap through the day, minutes go by five and
/// carry into the hour.
pub(crate) fn step_clock(minutes: u32, hours: bool, up: bool) -> u32 {
    let (mut hour, mut minute) = (minutes / 60 % 24, minutes % 60);
    if hours {
        hour = if up {
            (hour + 1) % 24
        } else {
            (hour + 23) % 24
        };
    } else if up {
        minute += 5;
        if minute >= 60 {
            minute = 0;
            hour = (hour + 1) % 24;
        }
    } else if minute < 5 {
        minute = 55;
        hour = (hour + 23) % 24;
    } else {
        minute -= 5;
    }
    hour * 60 + minute
}

/// "9:30" for a minute count into the day.
fn clock_at(minutes: u32) -> String {
    format!("{}:{:02}", minutes / 60 % 24, minutes % 60)
}

fn minutes_of(time: NaiveTime) -> u32 {
    time.hour() * 60 + time.minute()
}

fn stamp(at: NaiveDateTime) -> String {
    at.format("%Y-%m-%dT%H:%M:%S").to_string()
}

// ---------------------------------------------------------------- the form

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Preset {
    InHour,
    TodayEvening,
    TomorrowMorning,
    MondayMorning,
}

const PRESETS: [(Preset, &str); 4] = [
    (Preset::InHour, "Daqui a 1 hora"),
    (Preset::TodayEvening, "Hoje às 18:00"),
    (Preset::TomorrowMorning, "Amanhã às 9:00"),
    (Preset::MondayMorning, "Segunda às 9:00"),
];

/// A preset's moment; `None` for "today at 18:00" once that has passed.
pub(crate) fn preset_time(preset: Preset, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let at = |date: NaiveDate, hour: u32| date.and_hms_opt(hour, 0, 0);
    match preset {
        Preset::InHour => (now + Span::hours(1)).with_second(0),
        Preset::TodayEvening => at(now.date(), 18).filter(|at| *at > now),
        Preset::TomorrowMorning => at(now.date() + Span::days(1), 9),
        Preset::MondayMorning => {
            let ahead = 7 - i64::from(now.date().weekday().num_days_from_monday());
            at(now.date() + Span::days(ahead), 9)
        }
    }
}

/// The time a new task gets when a day (or a slot of it) is clicked: that slot (`minutes` into
/// the day) when one was clicked, else 9:00, or the next hour when today's 9:00 has passed. A day
/// that is over becomes today.
pub(crate) fn slot_default(
    date: NaiveDate,
    minutes: Option<u32>,
    now: NaiveDateTime,
) -> NaiveDateTime {
    let at = |minutes: u32| {
        let minutes = minutes.min(24 * 60 - 1);
        date.and_hms_opt(minutes / 60, minutes % 60, 0)
            .unwrap_or(now)
    };
    if date < now.date() {
        return slot_default(now.date(), None, now);
    }
    if let Some(minutes) = minutes {
        return at(minutes);
    }
    let morning = at(9 * 60);
    if date == now.date() && morning <= now {
        let next = now.hour() + 1;
        return if next < 24 { at(next * 60) } else { morning };
    }
    morning
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Repeat {
    No,
    Daily,
    Weekdays,
    Weekly,
    Monthly,
    Custom,
}

const REPEATS: [(Repeat, &str); 6] = [
    (Repeat::No, "Não"),
    (Repeat::Daily, "Todo dia"),
    (Repeat::Weekdays, "Dias úteis"),
    (Repeat::Weekly, "Semanal"),
    (Repeat::Monthly, "Mensal"),
    (Repeat::Custom, "Personalizado"),
];

/// A text field of the modal that keys go to, or a time box the arrow keys step.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Field {
    Title,
    Prompt,
    Hour,
    Minute,
}

/// What floats over the modal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Popup {
    Project,
    Date,
    /// The composer's model catalog (`Shell::catalog_page`), choosing for the draft.
    Model,
}

/// What stops the task from being created, as the footer's button finds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Problems {
    pub prompt: bool,
    pub effort: bool,
    pub weekdays: bool,
    pub past: bool,
}

impl Problems {
    pub(crate) fn any(self) -> bool {
        self.prompt || self.effort || self.weekdays || self.past
    }
}

/// The modal's form.
#[derive(Clone)]
pub(super) struct Sheet {
    /// The task being edited; `None` for a new one.
    editing: Option<String>,
    enabled: bool,
    title: String,
    prompt: String,
    project: String,
    worktree: bool,
    provider: &'static str,
    model: Option<String>,
    /// A level from the model; nothing is preselected.
    effort: Option<String>,
    fast: bool,
    /// "ask", "auto" or "full".
    approval: &'static str,
    date: NaiveDate,
    /// Minutes into the day.
    minutes: u32,
    repeat: Repeat,
    weekdays: [bool; 7],
    dom: u8,
    dom_edited: bool,
    every: u32,
    unit: Unit,
    /// The month the date popup shows.
    month: NaiveDate,
    field: Option<Field>,
    popup: Option<Popup>,
    /// The list drawn, which outlives `popup` while it fades out.
    popup_drawn: Option<Popup>,
    /// 0 closed, 1 open: the list's fade and slide.
    popup_open: Tween,
    /// A save was tried: problems show from now on.
    tried: bool,
    error: Option<String>,
    saving: bool,
}

impl Sheet {
    pub(super) fn provider(&self) -> &'static str {
        self.provider
    }

    pub(super) fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// Opens, switches or closes the list under a field; it fades and slides either way.
    fn set_popup(&mut self, popup: Option<Popup>) {
        let now = Instant::now();
        if popup.is_some() {
            self.popup_drawn = popup;
        }
        self.popup_open.set(
            if popup.is_some() { 1. } else { 0. },
            now,
            POPUP_MOTION,
            Curve::Expo,
        );
        self.popup = popup;
    }

    /// The list to draw now and how far in it is (0 to 1); `None` once it has faded out.
    fn popup_frame(&self, now: Instant) -> Option<(Popup, f32)> {
        let shown = self.popup_open.at(now).clamp(0., 1.);
        if self.popup.is_none() && self.popup_open.done(now) {
            return None;
        }
        self.popup_drawn.map(|popup| (popup, shown))
    }

    /// A form for a new task on `at`.
    fn blank(at: NaiveDateTime, project: String, provider: &'static str) -> Self {
        let mut weekdays = [false; 7];
        weekdays[at.date().weekday().num_days_from_monday() as usize] = true;
        Self {
            editing: None,
            enabled: true,
            title: String::new(),
            prompt: String::new(),
            project,
            worktree: false,
            provider,
            model: None,
            effort: None,
            fast: false,
            approval: "ask",
            date: at.date(),
            minutes: minutes_of(at.time()),
            repeat: Repeat::No,
            weekdays,
            dom: at.day() as u8,
            dom_edited: false,
            every: 2,
            unit: Unit::Weeks,
            month: month_first(at.date()),
            field: Some(Field::Prompt),
            popup: None,
            popup_drawn: None,
            popup_open: Tween::still(0., Instant::now()),
            tried: false,
            error: None,
            saving: false,
        }
    }

    /// A form filled from a task. A repeating rule's date is its first day, or today when that is
    /// behind us; an "every N" rule keeps its own start, since the steps count from it.
    fn of_task(task: &Task, now: NaiveDateTime, provider: &'static str) -> Self {
        let today = now.date();
        let near = task.next.map_or(today + Span::days(1), |at| at.date());
        let first = task.start.unwrap_or(near).max(today);
        let mut sheet = Self::blank(now, task.project_id.clone(), provider);
        sheet.editing = Some(task.id.clone());
        sheet.enabled = task.enabled;
        sheet.title = task.title.clone();
        sheet.prompt = task.prompt.clone();
        sheet.worktree = task.worktree;
        sheet.model = task.model.clone();
        sheet.effort = task.effort.clone();
        sheet.fast = task.fast;
        sheet.approval = ["ask", "auto", "full"]
            .into_iter()
            .find(|mode| Some(*mode) == task.approval.as_deref())
            .unwrap_or("ask");
        sheet.field = None;
        sheet.dom_edited = true;
        let (date, time) = match &task.rule {
            Rule::Once(at) => (at.date(), at.time()),
            Rule::Custom { time, start, .. } => (*start, *time),
            rule => (first, rule.time()),
        };
        match &task.rule {
            Rule::Once(_) => sheet.repeat = Repeat::No,
            Rule::Daily(_) => sheet.repeat = Repeat::Daily,
            Rule::Weekdays(_) => sheet.repeat = Repeat::Weekdays,
            Rule::Weekly(_, days) => {
                sheet.repeat = Repeat::Weekly;
                sheet.weekdays = [false; 7];
                for day in days.iter().filter(|day| **day < 7) {
                    sheet.weekdays[*day as usize] = true;
                }
            }
            Rule::Monthly(_, day) => {
                sheet.repeat = Repeat::Monthly;
                sheet.dom = *day;
            }
            Rule::Custom { every, unit, .. } => {
                sheet.repeat = Repeat::Custom;
                sheet.every = *every;
                sheet.unit = *unit;
            }
        }
        sheet.date = date;
        sheet.minutes = minutes_of(time);
        sheet.month = month_first(date);
        sheet
    }

    /// The moment the date and time name.
    pub(crate) fn when(&self) -> NaiveDateTime {
        self.date
            .and_hms_opt(self.minutes / 60 % 24, self.minutes % 60, 0)
            .unwrap_or_default()
    }

    /// The first day of a repeating rule: the form's date. A one-off has none.
    pub(crate) fn start(&self) -> Option<NaiveDate> {
        (self.repeat != Repeat::No).then_some(self.date)
    }

    /// The rule the form makes; `None` while a weekly repeat has no day.
    pub(crate) fn rule(&self) -> Option<Rule> {
        let when = self.when();
        let time = when.time();
        match self.repeat {
            Repeat::No => Some(Rule::Once(when)),
            Repeat::Daily => Some(Rule::Daily(time)),
            Repeat::Weekdays => Some(Rule::Weekdays(time)),
            Repeat::Weekly => {
                let days: Vec<u8> = (0..7u8)
                    .filter(|day| self.weekdays[*day as usize])
                    .collect();
                (!days.is_empty()).then_some(Rule::Weekly(time, days))
            }
            Repeat::Monthly => Some(Rule::Monthly(time, self.dom.clamp(1, 31))),
            Repeat::Custom => Some(Rule::Custom {
                time,
                every: self.every.clamp(1, 30),
                unit: self.unit,
                start: self.date,
            }),
        }
    }

    /// What stops the form from being saved. `effort_required` is false for a model that
    /// offers no levels.
    pub(crate) fn problems(&self, effort_required: bool, now: NaiveDateTime) -> Problems {
        Problems {
            prompt: self.prompt.trim().is_empty(),
            effort: effort_required && self.effort.is_none(),
            weekdays: self.repeat == Repeat::Weekly && self.weekdays.iter().all(|on| !on),
            past: self.repeat == Repeat::No && self.when() <= now,
        }
    }

    /// The rule as the footer says it; `None` while a weekly repeat has no day.
    pub(crate) fn rule_text(&self) -> Option<String> {
        if self.repeat == Repeat::No {
            let when = self.when();
            return Some(tf(
                "Uma vez, {}, {} às {}",
                &[
                    &weekday_name(self.date.weekday().num_days_from_monday() as usize),
                    &short_date(self.date),
                    &clock(when.time()),
                ],
            ));
        }
        self.rule().map(|rule| rule_words(&rule))
    }

    /// Sets the day; a weekly repeat with at most one day follows it, and a monthly one follows
    /// until its day was chosen by hand.
    fn set_date(&mut self, date: NaiveDate) {
        self.date = date;
        if self.repeat == Repeat::Weekly && self.weekdays.iter().filter(|on| **on).count() <= 1 {
            self.weekdays = [false; 7];
            self.weekdays[date.weekday().num_days_from_monday() as usize] = true;
        }
        if !self.dom_edited {
            self.dom = date.day() as u8;
        }
        self.month = month_first(date);
    }

    /// Sets the day and the time of day (what a preset or a clicked slot names).
    fn set_when(&mut self, at: NaiveDateTime) {
        self.minutes = minutes_of(at.time());
        self.set_date(at.date());
    }

    fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
        if repeat == Repeat::Weekly && self.weekdays.iter().all(|on| !on) {
            self.weekdays[self.date.weekday().num_days_from_monday() as usize] = true;
        }
    }

    /// Steps the hour or the minute box.
    fn step_time(&mut self, hours: bool, up: bool) {
        self.minutes = step_clock(self.minutes, hours, up);
    }
}

/// The description under a model's name in the picker: what the catalog says about it.
pub(crate) fn model_note(levels: usize, fast: bool) -> String {
    let base = match levels {
        0 => String::new(),
        1 => t("1 nível de esforço").to_owned(),
        n => tf("{} níveis de esforço", &[&n]),
    };
    match (base.is_empty(), fast) {
        (_, false) => base,
        (true, true) => capitalized(t("modo rápido")),
        (false, true) => format!("{base} · {}", t("modo rápido")),
    }
}

/// The quick examples under the prompt: the chip's name, the title and the prompt it fills.
const EXAMPLES: [(&str, &str, &str); 5] = [
    (
        "Rodar testes",
        "Rodar testes e resumir falhas",
        "Rode a suíte de testes do projeto. Se algo falhar, explique a causa provável de cada falha e resuma tudo em uma lista curta, sem alterar código.",
    ),
    (
        "Revisar PRs abertos",
        "Revisar PRs abertos",
        "Liste os pull requests abertos, revise cada diff e deixe um resumo com riscos e sugestões.",
    ),
    (
        "Atualizar dependências",
        "Atualizar dependências",
        "Verifique dependências desatualizadas, atualize as seguras (patch e minor), rode os testes e abra um PR com o resultado.",
    ),
    (
        "Gerar changelog",
        "Gerar changelog",
        "Gere o changelog desde a última tag a partir dos commits, agrupando por tipo de mudança.",
    ),
    (
        "Caçar código morto",
        "Procurar código morto",
        "Procure exports, arquivos e dependências sem uso e proponha a remoção em um PR.",
    ),
];

// ---------------------------------------------------------------- state

/// A run's (or a task's) popover.
pub(super) struct Popover {
    task: String,
    at: gpui::Point<gpui::Pixels>,
    /// The run it opened on, when a chip was clicked.
    occ: Option<Occ>,
    /// "Excluir" was pressed once: the next press deletes.
    confirm: bool,
    /// "Executar agora" is waiting for the host.
    running: bool,
}

pub(super) struct State {
    view: View,
    anchor: Option<NaiveDate>,
    pub tasks: Vec<Task>,
    occurrences: Vec<Occ>,
    now: Option<NaiveDateTime>,
    loaded: bool,
    error: Option<String>,
    pub(super) popover: Option<Popover>,
    pub(super) sheet: Option<Sheet>,
    scroll: gpui::ScrollHandle,
    sheet_scroll: gpui::ScrollHandle,
    /// The prompt field's own scroll, once it is taller than `PROMPT_MAX_H`.
    prompt_scroll: gpui::ScrollHandle,
    /// The prompt's length when the caret was last followed, so only typing scrolls it.
    prompt_seen: Rc<Cell<usize>>,
    /// Bumped by each request so a late answer to an older range is dropped.
    generation: u64,
    /// The grid scrolls to the morning (or to now) once after it opens or changes view.
    scrolled: Cell<bool>,
    /// Bumped when the view or the range changes, so the body fades in again.
    shown: u64,
    /// Where the modal's pickers are, so their lists open under them.
    anchors: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
    /// The window's size, seen by the last frame, to fit a month's chips to its cells.
    viewport: Cell<(f32, f32)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            view: View::Month,
            anchor: None,
            tasks: Vec::new(),
            occurrences: Vec::new(),
            now: None,
            loaded: false,
            error: None,
            popover: None,
            sheet: None,
            scroll: gpui::ScrollHandle::new(),
            sheet_scroll: gpui::ScrollHandle::new(),
            prompt_scroll: gpui::ScrollHandle::new(),
            prompt_seen: Rc::new(Cell::new(usize::MAX)),
            generation: 0,
            scrolled: Cell::new(false),
            shown: 0,
            anchors: Rc::default(),
            viewport: Cell::new((1280., 800.)),
        }
    }
}

impl State {
    fn today(&self) -> NaiveDate {
        self.now
            .map_or_else(|| Local::now().date_naive(), |now| now.date())
    }

    fn anchor(&self) -> NaiveDate {
        self.anchor.unwrap_or_else(|| self.today())
    }

    fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|task| task.id == id)
    }

    /// The model list is open over the modal.
    pub(super) fn model_open(&self) -> bool {
        self.sheet
            .as_ref()
            .is_some_and(|sheet| sheet.popup == Some(Popup::Model))
    }

    /// The text the modal's focused field edits; keys go to `fallback` when none has focus.
    pub(super) fn text_target<'a>(&'a mut self, fallback: &'a mut String) -> &'a mut String {
        match self.sheet.as_mut() {
            Some(sheet) => match sheet.field {
                Some(Field::Title) => &mut sheet.title,
                Some(Field::Prompt) => &mut sheet.prompt,
                _ => fallback,
            },
            None => fallback,
        }
    }

    /// Where a picker of the modal was last drawn.
    fn anchor_of(&self, key: &'static str) -> Option<Bounds<Pixels>> {
        self.anchors.borrow().get(key).copied()
    }
}

impl Shell {
    /// The Tarefas tab is the page on screen.
    pub(super) fn tasks_page_open(&self) -> bool {
        self.term.tasks_active
    }

    /// How many tasks will still run, for the sidebar's badge.
    pub(super) fn tasks_active(&self) -> usize {
        active_count(&self.tasks_page.tasks)
    }

    /// Esc takes the layers apart one at a time: the open list, the modal, the popover.
    fn tasks_close_layer(&mut self, cx: &mut Context<Self>) -> bool {
        let page = &mut self.tasks_page;
        if let Some(sheet) = page.sheet.as_mut() {
            if sheet.popup.is_none() {
                page.sheet = None;
            } else {
                sheet.set_popup(None);
            }
            cx.notify();
            return true;
        }
        if page.popover.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// Asks the host for the tasks and the runs of what the page shows.
    pub(super) fn tasks_load(&mut self, cx: &mut Context<Self>) {
        let page = &mut self.tasks_page;
        let days = view_days(page.view, page.anchor(), monday_first());
        let (Some(first), Some(last)) = (days.first().copied(), days.last().copied()) else {
            return;
        };
        let from = first.and_hms_opt(0, 0, 0).unwrap_or_default();
        let to = (last + Span::days(1))
            .and_hms_opt(0, 0, 0)
            .unwrap_or_default();
        page.generation += 1;
        let generation = page.generation;
        let rx = self.backend.request(
            "list_tasks",
            json!({ "from": stamp(from), "to": stamp(to) }),
        );
        cx.spawn(async move |this, cx| {
            let answer = message_actions::settle(rx).await;
            let _ =
                this.update(cx, |this, cx| {
                    let page = &mut this.tasks_page;
                    if page.generation != generation {
                        return;
                    }
                    match answer {
                        Ok(value) => {
                            let (tasks, occurrences, now) = parse_list(&value);
                            page.tasks = tasks;
                            page.occurrences = occurrences;
                            page.now = now;
                            page.error = None;
                            page.loaded = true;
                            // A popover for a task that is gone has nothing to show.
                            if page.popover.as_ref().is_some_and(|pop| {
                                page.tasks.iter().all(|task| task.id != pop.task)
                            }) {
                                page.popover = None;
                            }
                        }
                        Err(problem) => {
                            page.error = Some(crate::i18n::from_host(&problem));
                        }
                    }
                    cx.notify();
                });
        })
        .detach();
    }

    fn tasks_set_view(&mut self, view: View, cx: &mut Context<Self>) {
        if self.tasks_page.view == view {
            return;
        }
        self.tasks_page.view = view;
        self.tasks_page.popover = None;
        self.tasks_page.scrolled.set(false);
        self.tasks_page.shown += 1;
        self.tasks_load(cx);
        cx.notify();
    }

    fn tasks_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let page = &mut self.tasks_page;
        page.anchor = Some(step_anchor(page.view, page.anchor(), forward));
        page.popover = None;
        page.shown += 1;
        self.tasks_load(cx);
        cx.notify();
    }

    fn tasks_today(&mut self, cx: &mut Context<Self>) {
        let page = &mut self.tasks_page;
        page.anchor = None;
        page.popover = None;
        page.scrolled.set(false);
        page.shown += 1;
        self.tasks_load(cx);
        cx.notify();
    }

    /// Runs a command on a task, then reloads. A refusal shows as the page's message.
    fn tasks_command(&mut self, command: &'static str, args: Value, cx: &mut Context<Self>) {
        let rx = self.backend.request(command, args);
        cx.spawn(async move |this, cx| {
            let answer = message_actions::settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                if let Err(problem) = answer {
                    this.tasks_page.error = Some(crate::i18n::from_host(&problem));
                }
                this.tasks_load(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn tasks_toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(task) = self.tasks_page.tasks.iter_mut().find(|task| task.id == id) else {
            return;
        };
        task.enabled = !task.enabled;
        let enabled = task.enabled;
        self.tasks_command(
            "set_task_enabled",
            json!({ "id": id, "enabled": enabled }),
            cx,
        );
        cx.notify();
    }

    /// "Excluir" asks once more; the second press deletes. The question lapses after a moment.
    fn tasks_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(pop) = self.tasks_page.popover.as_mut() else {
            return;
        };
        if !pop.confirm {
            pop.confirm = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(2600))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(pop) = this.tasks_page.popover.as_mut() {
                        pop.confirm = false;
                    }
                    cx.notify();
                });
            })
            .detach();
            cx.notify();
            return;
        }
        self.tasks_page.popover = None;
        self.tasks_command("delete_task", json!({ "id": id }), cx);
        cx.notify();
    }

    /// Starts the task now; the popover shows "Executando…" until the host answers.
    fn tasks_run_now(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(pop) = self.tasks_page.popover.as_mut() else {
            return;
        };
        if pop.running {
            return;
        }
        pop.running = true;
        let rx = self.backend.request("run_task_now", json!({ "id": id }));
        cx.spawn(async move |this, cx| {
            let answer = message_actions::settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.tasks_page.popover = None;
                if let Err(problem) = answer {
                    this.tasks_page.error = Some(crate::i18n::from_host(&problem));
                }
                this.tasks_load(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Opens the conversation of a run, when it still exists.
    fn tasks_open_chat(&mut self, session: &str, cx: &mut Context<Self>) {
        let Some(ix) = self
            .conversations
            .iter()
            .position(|conversation| conversation.session_id == session)
        else {
            self.tasks_page.error = Some(t("Essa conversa não existe mais.").to_owned());
            cx.notify();
            return;
        };
        self.tasks_page.popover = None;
        self.open(ix, cx);
    }

    /// Opens a task's popover: on the run clicked, or (from the list) on its next or last one.
    fn tasks_pop(
        &mut self,
        task: &str,
        occ: Option<Occ>,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.tasks_page.popover = Some(Popover {
            task: task.to_owned(),
            at,
            occ,
            confirm: false,
            running: false,
        });
        cx.notify();
    }

    // ------------------------------------------------------------ the modal

    fn tasks_now(&self) -> NaiveDateTime {
        self.tasks_page
            .now
            .unwrap_or_else(|| Local::now().naive_local())
    }

    /// The first model the provider's picker lists: what a new task starts on.
    fn tasks_first_model(&self, provider: &'static str) -> Option<String> {
        self.provider_model_rows(provider, (provider, None), "", &self.favorite_models())
            .first()
            .map(|row| row.id.clone())
    }

    /// Makes the form agree with its model: a level the model lacks is dropped, and Fast is off
    /// where the model has none.
    fn tasks_settle(&mut self) {
        let Some(sheet) = self.tasks_page.sheet.as_ref() else {
            return;
        };
        let exec = self.execution(sheet.provider, sheet.model.as_deref());
        let Some(sheet) = self.tasks_page.sheet.as_mut() else {
            return;
        };
        if sheet
            .effort
            .as_ref()
            .is_some_and(|level| !exec.levels.contains(level))
        {
            sheet.effort = None;
        }
        if !exec.fast_ok {
            sheet.fast = false;
        }
        if sheet.effort.is_none() {
            crate::segment_pill::forget("tasks-effort");
        }
    }

    /// Opens the modal for a new task at `at` (the next free hour of today when none).
    fn tasks_new(&mut self, at: Option<NaiveDateTime>, cx: &mut Context<Self>) {
        let now = self.tasks_now();
        let at = at.unwrap_or_else(|| slot_default(now.date(), None, now));
        let provider = self
            .enabled_providers()
            .first()
            .copied()
            .unwrap_or("claude");
        let project = self
            .sidebar_project_id()
            .or_else(|| self.projects.first().map(|project| project.id.clone()))
            .unwrap_or_default();
        let mut sheet = Sheet::blank(at, project, provider);
        sheet.model = self.tasks_first_model(provider);
        self.tasks_page.popover = None;
        self.tasks_page.sheet = Some(sheet);
        self.tasks_settle();
        self.tasks_forget_pills();
        self.field_caret_moved.set(Instant::now());
        cx.notify();
    }

    /// The modal's segmented controls start in place each time it opens, and so does the prompt.
    fn tasks_forget_pills(&self) {
        self.tasks_page.prompt_seen.set(usize::MAX);
        self.tasks_page
            .prompt_scroll
            .set_offset(gpui::point(px(0.), px(0.)));
        for key in [
            "tasks-effort",
            "tasks-worktree",
            "tasks-access",
            "tasks-repeat",
            "tasks-unit",
        ] {
            crate::segment_pill::forget(key);
        }
    }

    /// Opens the modal on a task, to edit it: saving sends the same `save_task`, with its id.
    fn tasks_edit(&mut self, id: &str, cx: &mut Context<Self>) {
        let now = self.tasks_now();
        let Some(task) = self.tasks_page.task(id).cloned() else {
            return;
        };
        let provider = PROVIDER_ORDER
            .iter()
            .copied()
            .find(|provider| *provider == task.provider)
            .unwrap_or("claude");
        let mut sheet = Sheet::of_task(&task, now, provider);
        if sheet.model.is_none() {
            sheet.model = self.tasks_first_model(provider);
        }
        self.tasks_page.popover = None;
        self.tasks_page.sheet = Some(sheet);
        self.tasks_settle();
        self.tasks_forget_pills();
        cx.notify();
    }

    fn tasks_save(&mut self, cx: &mut Context<Self>) {
        let now = self.tasks_now();
        let Some(sheet) = self.tasks_page.sheet.as_ref() else {
            return;
        };
        let exec = self.execution(sheet.provider, sheet.model.as_deref());
        let Some(sheet) = self.tasks_page.sheet.as_mut() else {
            return;
        };
        if sheet.saving {
            return;
        }
        sheet.tried = true;
        if sheet.problems(!exec.levels.is_empty(), now).any() {
            cx.notify();
            return;
        }
        let Some(rule) = sheet.rule() else {
            cx.notify();
            return;
        };
        if sheet.project.is_empty() {
            sheet.error = Some(t("Escolha um projeto.").to_owned());
            cx.notify();
            return;
        }
        sheet.error = None;
        sheet.saving = true;
        let mut task = json!({
            "title": sheet.title.trim(),
            "prompt": sheet.prompt.trim(),
            "projectId": sheet.project,
            "provider": sheet.provider,
            "model": sheet.model,
            "effort": sheet.effort,
            "approval": sheet.approval,
            "fast": sheet.fast && exec.fast_ok,
            "worktree": sheet.worktree,
            "schedule": rule_json(&rule, sheet.start()),
            "enabled": sheet.enabled,
        });
        if let Some(id) = &sheet.editing {
            task["id"] = json!(id);
        }
        let rx = self.backend.request("save_task", json!({ "task": task }));
        cx.spawn(async move |this, cx| {
            let answer = message_actions::settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                match answer {
                    Ok(_) => {
                        // The calendar goes to the new task's day when it is not on screen.
                        if let Some(date) = this.tasks_page.sheet.as_ref().map(|s| s.date) {
                            let page = &mut this.tasks_page;
                            let shown = view_days(page.view, page.anchor(), monday_first());
                            if !shown.contains(&date) {
                                page.anchor = Some(date);
                                page.shown += 1;
                            }
                        }
                        this.tasks_page.sheet = None;
                        this.tasks_load(cx);
                    }
                    Err(problem) => {
                        if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                            sheet.saving = false;
                            sheet.error = Some(crate::i18n::from_host(&problem));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The page's keys while its tab shows. Without the modal: ← and → step, T is today, D, S and
    /// M pick the view, N opens a new task. With it: Enter, Tab, Backspace, the arrows on a time
    /// box and Esc edit the focused field. Returns whether the key was taken.
    pub(super) fn tasks_keys(&mut self, keys: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        if !self.tasks_page_open() {
            return false;
        }
        let key = keys.key.as_str();
        let held = keys.modifiers.platform || keys.modifiers.control || keys.modifiers.alt;
        if key == "escape" {
            return self.tasks_close_layer(cx);
        }
        if self.tasks_page.sheet.is_none() {
            if !held {
                match key {
                    "left" | "right" => self.tasks_step(key == "right", cx),
                    "t" => self.tasks_today(cx),
                    "d" => self.tasks_set_view(View::Day, cx),
                    "s" => self.tasks_set_view(View::Week, cx),
                    "m" => self.tasks_set_view(View::Month, cx),
                    "n" => self.tasks_new(None, cx),
                    "enter" | "tab" | "backspace" => {}
                    _ => return false,
                }
                return true;
            }
            return false;
        }
        if key == "enter" && keys.modifiers.platform {
            self.tasks_save(cx);
            return true;
        }
        if self.tasks_page.model_open() && matches!(key, "backspace" | "up" | "down" | "enter") {
            let rows = self.model_rows_for(ModelTarget::Task);
            let count = rows.len();
            match key {
                "backspace" => {
                    self.model_query.pop();
                    self.model_selected = None;
                }
                "down" if count > 0 => {
                    self.model_selected =
                        Some(self.model_selected.map_or(0, |at| (at + 1) % count));
                }
                "up" if count > 0 => {
                    self.model_selected = Some(
                        self.model_selected
                            .map_or(count - 1, |at| (at + count - 1) % count),
                    );
                }
                "enter" => {
                    if let Some(row) = self
                        .model_selected
                        .or((count == 1).then_some(0))
                        .and_then(|at| rows.get(at))
                    {
                        self.tasks_choose_model(row.provider, row.id.clone(), cx);
                    }
                }
                _ => {}
            }
            self.field_caret_moved.set(Instant::now());
            cx.notify();
            return true;
        }
        let Some(sheet) = self.tasks_page.sheet.as_mut() else {
            return false;
        };
        let taken = match key {
            "enter" => {
                match sheet.field {
                    Some(Field::Title) => sheet.field = Some(Field::Prompt),
                    Some(Field::Prompt) => sheet.prompt.push('\n'),
                    Some(_) => sheet.field = None,
                    None => {}
                }
                true
            }
            "tab" => {
                sheet.field = match sheet.field {
                    Some(Field::Title) => Some(Field::Prompt),
                    Some(Field::Prompt) => Some(Field::Hour),
                    Some(Field::Hour) => Some(Field::Minute),
                    Some(Field::Minute) => None,
                    None => Some(Field::Title),
                };
                sheet.set_popup(None);
                true
            }
            "backspace" => {
                match sheet.field {
                    Some(Field::Title) => {
                        sheet.title.pop();
                    }
                    Some(Field::Prompt) => {
                        sheet.prompt.pop();
                    }
                    _ => {}
                }
                true
            }
            "up" | "down" => match sheet.field {
                Some(field @ (Field::Hour | Field::Minute)) => {
                    sheet.step_time(field == Field::Hour, key == "up");
                    true
                }
                _ => false,
            },
            _ => false,
        };
        if taken {
            self.field_caret_moved.set(Instant::now());
            cx.notify();
        }
        taken
    }

    // ------------------------------------------------------------ drawing

    fn tasks_color(provider: &str) -> gpui::Hsla {
        usage_page::Prov::of(provider).color()
    }

    /// Reports where an element of the modal is, for the list that opens under it.
    fn tasks_anchor(&self, key: &'static str) -> impl IntoElement {
        let anchors = self.tasks_page.anchors.clone();
        gpui::canvas(
            move |bounds, _: &mut Window, _: &mut gpui::App| {
                anchors.borrow_mut().insert(key, bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    fn tasks_project_name(&self, id: &str) -> String {
        self.projects
            .iter()
            .find(|project| project.id == id)
            .map_or_else(|| t("Projeto removido").to_owned(), |p| p.name.clone())
    }

    /// A project's robot avatar, as the sidebar draws it; an empty square for a removed project.
    fn tasks_avatar(&self, project: &str, side: f32) -> AnyElement {
        match self.projects.iter().position(|p| p.id == project) {
            Some(ix) => self.project_avatar_for(ix, side),
            None => div().size(px(side)).into_any_element(),
        }
    }

    fn tasks_project_path(&self, id: &str) -> String {
        self.projects
            .iter()
            .find(|project| project.id == id)
            .map(|project| tidy_path(&project.path))
            .unwrap_or_default()
    }

    /// The page in the Tarefas tab: header, the task list and the calendar, and a message when
    /// the host refused something. A change of view or range fades the calendar in again.
    pub(super) fn tasks_page_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.tasks_page;
        let calendar: AnyElement = match page.view {
            View::Month => self.tasks_month(cx),
            View::Week => self.tasks_grid(view_days(View::Week, page.anchor(), monday_first()), cx),
            View::Day => self.tasks_grid(vec![page.anchor()], cx),
        };
        let calendar = div()
            .id("tasks-calendar")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(calendar);
        let calendar = if crate::prefs::still() {
            calendar.into_any_element()
        } else {
            calendar
                .with_animation(
                    SharedString::from(format!("tasks-calendar-{}", page.shown)),
                    Animation::new(std::time::Duration::from_millis(180))
                        .with_easing(|t| eased(t, Curve::Expo)),
                    |el, t| el.opacity(t),
                )
                .into_any_element()
        };
        // The calendar always shows, with or without tasks (ADR-198).
        let work: AnyElement = div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(self.tasks_list(cx))
            .child(calendar)
            .into_any_element();
        div()
            .id("tasks-page")
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(self.tasks_header(cx))
            .when_some(page.error.clone(), |d, error| {
                d.child(
                    div()
                        .px(px(24.))
                        .py(px(6.))
                        .text_size(px(12.))
                        .text_color(gpui::rgb(0xe5484d))
                        .child(error),
                )
            })
            .child(work)
            .into_any_element()
    }

    /// What floats over the window while the tab shows: the run's popover, the modal and the
    /// lists that open from it.
    pub(super) fn tasks_overlays(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if !self.tasks_page_open() {
            return Vec::new();
        }
        let size = window.viewport_size();
        let (vw, vh) = (f32::from(size.width), f32::from(size.height));
        self.tasks_page.viewport.set((vw, vh));
        if let Some(sheet) = self.tasks_page.sheet.as_ref()
            && !sheet.popup_open.done(Instant::now())
        {
            window.request_animation_frame();
        }
        self.tasks_popover(vw, vh, cx)
            .into_iter()
            .chain(self.tasks_sheet_view(vw, vh, cx))
            .collect()
    }

    fn tasks_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.tasks_page;
        let range = period_label(page.view, page.anchor(), page.today(), monday_first());
        let chosen = VIEWS.iter().position(|v| *v == page.view).unwrap_or(2);
        let segments = VIEWS
            .iter()
            .enumerate()
            .map(|(ix, view)| {
                let view = *view;
                div()
                    .id(("tasks-view", ix))
                    .relative()
                    .h(px(26.))
                    .px(px(14.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(13.))
                    .when(view == page.view, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .cursor_pointer()
                    .text_color(if view == page.view {
                        crate::theme::on_accent()
                    } else {
                        content(0.5)
                    })
                    .when(view != page.view, |d| {
                        d.hover(|s| s.text_color(content(0.92)))
                    })
                    .child(crate::segment_pill::probe("tasks-view", ix))
                    .on_click(cx.listener(move |this, _, _, cx| this.tasks_set_view(view, cx)))
                    .child(view.label())
            })
            .collect::<Vec<_>>();
        let arrow =
            |id: &'static str, glyph: &'static str, forward: bool, cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .size(px(28.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(20.))
                    .text_color(content(0.75))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.09)))
                    .on_click(cx.listener(move |this, _, _, cx| this.tasks_step(forward, cx)))
                    .child(glyph)
            };
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(18.))
            .h(px(54.))
            .border_b_1()
            .border_color(content(0.07))
            .child(arrow("tasks-prev", "‹", false, cx))
            .child(
                div()
                    .min_w(px(210.))
                    .flex()
                    .justify_center()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(content(0.92))
                    .child(range),
            )
            .child(arrow("tasks-next", "›", true, cx))
            .child(div().flex_1())
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_shrink_0()
                    .p(px(2.))
                    .rounded(px(9.))
                    .border_1()
                    .border_color(content(0.07))
                    .bg(content(0.05))
                    .child(crate::segment_pill::pill(
                        "tasks-view",
                        chosen,
                        crate::theme::accent(),
                        crate::theme::RADIUS_SM,
                    ))
                    .children(segments),
            )
            // Far right, after the view switch.
            .child(self.tasks_new_button(cx))
            .into_any_element()
    }

    fn tasks_new_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("tasks-new")
            .h(px(30.))
            .px(px(13.))
            .rounded(px(8.))
            .bg(crate::theme::accent())
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(crate::theme::on_accent())
            .cursor_pointer()
            .hover(|s| s.opacity(0.9))
            .active(|s| s.opacity(0.75))
            .on_click(cx.listener(|this, _, _, cx| this.tasks_new(None, cx)))
            .child("+")
            .child(t("Nova tarefa"))
    }

    /// "Minhas tarefas": the tasks with their switches, at the left of the calendar.
    fn tasks_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.tasks_page;
        let rows = page.tasks.iter().enumerate().map(|(ix, task)| {
            let color = Self::tasks_color(&task.provider);
            let id = task.id.clone();
            let toggle = task.id.clone();
            let off = !task.enabled;
            div()
                .id(("tasks-row", ix))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(6.))
                .py(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.05)))
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                    let occ = this.tasks_page.task(&id).and_then(|task| {
                        task.next.map(|at| Occ {
                            task: task.id.clone(),
                            at,
                            run: Run::Planned,
                            session: None,
                        })
                    });
                    this.tasks_pop(&id, occ, event.position(), cx);
                }))
                .child(
                    div()
                        .flex_shrink_0()
                        .size(px(10.))
                        .rounded(px(3.))
                        .bg(color)
                        .when(off, |d| d.opacity(0.35)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(content(if off { 0.45 } else { 0.92 }))
                                .when(off, |d| d.line_through())
                                .child(if task.title.is_empty() {
                                    t("Sem título").to_owned()
                                } else {
                                    task.title.clone()
                                }),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(content(0.48))
                                .child(rule_short(&task.rule)),
                        ),
                )
                .child(
                    div()
                        .id(("tasks-row-toggle", ix))
                        .flex_shrink_0()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.tasks_toggle(&toggle, cx)
                        }))
                        .child(crate::settings_pages::switch_track(
                            SWITCH_ID + 1 + ix,
                            task.enabled,
                            true,
                        )),
                )
        });
        div()
            .id("tasks-list")
            .w(px(216.))
            .flex_shrink_0()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(content(0.07))
            .px(px(12.))
            .py(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .mb(px(8.))
                    .ml(px(6.))
                    .text_size(px(11.))
                    .text_color(content(0.4))
                    .child(div().flex_1().child(t("Minhas tarefas")))
                    .child(tf("{} ativas", &[&self.tasks_active()])),
            )
            .children(rows)
            .when(page.loaded && page.tasks.is_empty(), |d| {
                d.child(
                    div()
                        .ml(px(6.))
                        .mt(px(4.))
                        .text_size(px(12.))
                        .line_height(px(17.))
                        .text_color(content(0.4))
                        .child(t(
                            "Nenhuma tarefa ainda. Clique num dia ou em + Nova tarefa.",
                        )),
                )
            })
            .into_any_element()
    }

    /// A run as a flat chip: a soft tint of the provider's colour, the time a little brighter,
    /// the title in normal text and a quiet ✓ or ✗ at the right.
    fn tasks_chip(&self, occ: &Occ, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let task = self.tasks_page.task(&occ.task)?;
        let color = Self::tasks_color(&task.provider);
        let id = task.id.clone();
        let run = occ.clone();
        let mark = match occ.run {
            Run::Done => Some(("✓", 0x3fb97d_u32)),
            Run::Failed => Some(("✗", 0xe5484d_u32)),
            Run::Planned => None,
        };
        let mut wash = color;
        wash.a = 0.2;
        let mut hover = color;
        hover.a = 0.32;
        Some(
            div()
                .id(SharedString::from(format!(
                    "tasks-run-{}-{}",
                    task.id,
                    stamp(occ.at)
                )))
                .h(px(19.))
                .mt(px(2.))
                .flex()
                .items_center()
                .gap(px(5.))
                .px(px(6.))
                .rounded(px(5.))
                .bg(wash)
                .text_size(px(11.5))
                .overflow_hidden()
                .cursor_pointer()
                .when(occ.run != Run::Planned, |d| d.opacity(0.66))
                .hover(move |s| s.bg(hover))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                    this.tasks_pop(&id, Some(run.clone()), event.position(), cx);
                }))
                .child(
                    div()
                        .flex_shrink_0()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(bright(color))
                        .child(clock(occ.at.time())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(content(0.9))
                        .child(if task.title.is_empty() {
                            t("Sem título").to_owned()
                        } else {
                            task.title.clone()
                        }),
                )
                .when_some(mark, |d, (glyph, hex)| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.5))
                            .text_color(gpui::rgb(hex).opacity(0.75))
                            .child(glyph),
                    )
                }),
        )
    }

    /// The runs of a day, in time order, leaving out a paused task's coming ones.
    fn tasks_day_runs(&self, day: NaiveDate) -> Vec<&Occ> {
        let page = &self.tasks_page;
        let mut occs: Vec<&Occ> = page
            .occurrences
            .iter()
            .filter(|occ| occ.at.date() == day)
            .filter(|occ| {
                page.task(&occ.task)
                    .is_some_and(|task| drawn(occ.run, task.enabled))
            })
            .collect();
        occs.sort_by_key(|occ| occ.at);
        occs
    }

    /// Week and day: hour rows, one column per day, runs placed by their time. Hovering an empty
    /// half hour shows a ghost; clicking it opens the modal there.
    fn tasks_grid(&self, days: Vec<NaiveDate>, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.tasks_page;
        let today = page.today();
        let now = page.now;
        let hairline = content(0.07);
        // Opened on the morning, or around now when today is showing.
        if !page.scrolled.get() {
            let hour = match now {
                Some(now) if days.contains(&now.date()) => f32::from(now.hour() as u8) - 2.,
                _ => 7.,
            };
            page.scroll
                .set_offset(gpui::point(px(0.), px(-(hour.max(0.) * HOUR_H))));
        }
        page.scrolled.set(true);
        let heads = days.iter().map(|day| {
            let day = *day;
            let is_today = day == today;
            let one = days.len() == 1;
            div()
                .id(SharedString::from(format!("tasks-head-{day}")))
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .py(px(7.))
                .border_l_1()
                .border_color(hairline)
                .when(!one, |d| d.cursor_pointer())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.tasks_page.view == View::Week {
                        this.tasks_page.anchor = Some(day);
                        this.tasks_set_view(View::Day, cx);
                    }
                }))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(if is_today {
                            crate::theme::accent()
                        } else {
                            content(0.48)
                        })
                        .child(t(WEEKDAYS[day.weekday().num_days_from_monday() as usize])),
                )
                .child(
                    div()
                        .min_w(px(34.))
                        .h(px(34.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(19.))
                        .when(is_today, |d| {
                            d.bg(crate::theme::accent()).font_weight(FontWeight::BOLD)
                        })
                        .text_color(if is_today {
                            crate::theme::on_accent()
                        } else {
                            content(0.9)
                        })
                        .child(day.day().to_string()),
                )
        });
        let labels = (1..24).map(|hour| {
            div()
                .absolute()
                .top(px(hour as f32 * HOUR_H - 7.))
                .right(px(8.))
                .text_size(px(10.5))
                .text_color(content(0.48))
                .child(format!("{hour:02}:00"))
        });
        let mut columns = Vec::new();
        for day in &days {
            let occs = self.tasks_day_runs(*day);
            let starts: Vec<i64> = occs
                .iter()
                .map(|occ| i64::from(occ.at.hour()) * 60 + i64::from(occ.at.minute()))
                .collect();
            let placed = lanes(&starts);
            let is_today = *day == today;
            let mut column = div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(24. * HOUR_H))
                .border_l_1()
                .border_color(hairline)
                .when(is_today, |d| d.bg(crate::theme::accent_wash(0.03)));
            // Hour lines.
            for hour in 1..24 {
                column = column.child(
                    div()
                        .absolute()
                        .top(px(hour as f32 * HOUR_H))
                        .left_0()
                        .right_0()
                        .h(px(1.))
                        .bg(hairline),
                );
            }
            // Half-hour slots: an empty one starts a task at its time, and shows a ghost.
            for slot in 0..48u32 {
                let day = *day;
                let minutes = slot * 30;
                let group = SharedString::from(format!("tasks-slot-{day}-{slot}"));
                column = column.child(
                    div()
                        .id(group.clone())
                        .group(group.clone())
                        .absolute()
                        .top(px(slot as f32 * HOUR_H / 2.))
                        .left_0()
                        .right_0()
                        .h(px(HOUR_H / 2.))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let now = this.tasks_now();
                            this.tasks_new(Some(slot_default(day, Some(minutes), now)), cx);
                        }))
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(px(3.))
                                .right(px(3.))
                                .rounded(px(6.))
                                .border_1()
                                .border_dashed()
                                .border_color(crate::theme::accent().opacity(0.6))
                                .bg(crate::theme::accent_wash(0.1))
                                .px(px(6.))
                                .py(px(2.))
                                .text_size(px(11.))
                                .text_color(crate::theme::accent())
                                .opacity(0.)
                                .group_hover(group, |s| s.opacity(1.))
                                .child(format!("+ {}", clock_at(minutes))),
                        ),
                );
            }
            for (occ, (lane, count)) in occs.iter().zip(placed) {
                let Some(task) = page.task(&occ.task) else {
                    continue;
                };
                let color = Self::tasks_color(&task.provider);
                let minutes = (occ.at.hour() * 60 + occ.at.minute()) as f32;
                let (width, left) = (1. / count as f32, lane as f32 / count as f32);
                let id = task.id.clone();
                let run = (*occ).clone();
                let mut wash = color;
                wash.a = 0.22;
                let mut hover = color;
                hover.a = 0.36;
                let mark = match occ.run {
                    Run::Done => Some(("✓", 0x3fb97d_u32)),
                    Run::Failed => Some(("✗", 0xe5484d_u32)),
                    Run::Planned => None,
                };
                column = column.child(
                    div()
                        .absolute()
                        .top(px(minutes / 60. * HOUR_H + 1.))
                        .h(px(BLOCK_MIN as f32 / 60. * HOUR_H - 2.))
                        .left(gpui::relative(left))
                        .w(gpui::relative(width))
                        .px(px(2.))
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "tasks-block-{}-{}",
                                    task.id,
                                    stamp(occ.at)
                                )))
                                .occlude()
                                .size_full()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .px(px(6.))
                                .rounded(px(7.))
                                .bg(wash)
                                .overflow_hidden()
                                .cursor_pointer()
                                .when(occ.run != Run::Planned, |d| d.opacity(0.7))
                                .hover(move |s| s.bg(hover))
                                .on_click(cx.listener(
                                    move |this, event: &gpui::ClickEvent, _, cx| {
                                        this.tasks_pop(
                                            &id,
                                            Some(run.clone()),
                                            event.position(),
                                            cx,
                                        );
                                    },
                                ))
                                .child(provider_mark(&task.provider, 12.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(content(0.92))
                                        .child(task.title.clone()),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(px(11.))
                                        .text_color(bright(color))
                                        .child(clock(occ.at.time())),
                                )
                                .when_some(mark, |d, (glyph, hex)| {
                                    d.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(px(10.5))
                                            .text_color(gpui::rgb(hex))
                                            .child(glyph),
                                    )
                                }),
                        ),
                );
            }
            if let Some(now) = now.filter(|now| now.date() == *day) {
                let at = (now.hour() * 60 + now.minute()) as f32 / 60. * HOUR_H;
                let red: gpui::Hsla = gpui::rgb(NOW_LINE).into();
                column = column
                    .child(
                        div()
                            .absolute()
                            .top(px(at - 1.))
                            .left_0()
                            .right_0()
                            .h(px(2.))
                            .bg(red),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(at - 5.))
                            .left(px(-5.))
                            .size(px(10.))
                            .rounded_full()
                            .bg(red),
                    );
            }
            columns.push(column);
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .border_b_1()
                    .border_color(hairline)
                    .pr(px(8.))
                    .child(div().w(px(GUTTER)).flex_shrink_0())
                    .children(heads),
            )
            .child(
                div()
                    .id("tasks-grid")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&page.scroll)
                    .child(
                        div()
                            .flex()
                            .child(
                                div()
                                    .relative()
                                    .w(px(GUTTER))
                                    .h(px(24. * HOUR_H))
                                    .flex_shrink_0()
                                    .children(labels),
                            )
                            .children(columns),
                    ),
            )
            .into_any_element()
    }

    /// Month: a full-height grid of days, each with its runs as flat chips. What does not fit
    /// shows as "+N mais", which opens the day.
    fn tasks_month(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.tasks_page;
        let anchor = page.anchor();
        let today = page.today();
        let hairline = content(0.07);
        let days = month_grid(anchor, monday_first());
        let rows = (days.len() / 7).max(1);
        let (_, window_h) = page.viewport.get();
        let cell_h = ((window_h - PAGE_CHROME - MONTH_HEAD) / rows as f32).max(60.);
        let cap = month_cap(cell_h);
        let order = weekday_order(monday_first());
        let heads = order.into_iter().enumerate().map(|(ix, day)| {
            div()
                .flex_1()
                .min_w_0()
                .px(px(10.))
                .py(px(8.))
                .text_size(px(11.))
                .text_color(content(0.48))
                .when(ix < 6, |d| d.border_r_1().border_color(hairline))
                .child(t(WEEKDAYS[day]))
        });
        let weeks = days.chunks(7).map(|week| {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .children(week.iter().enumerate().map(|(column, day)| {
                    let day = *day;
                    let in_month = day.month() == anchor.month();
                    let is_today = day == today;
                    let occs = self.tasks_day_runs(day);
                    let (shown, hidden) = month_split(occs.len(), cap);
                    let chips: Vec<AnyElement> = occs
                        .iter()
                        .take(shown)
                        .filter_map(|occ| self.tasks_chip(occ, cx))
                        .map(IntoElement::into_any_element)
                        .collect();
                    div()
                        .id(SharedString::from(format!("tasks-day-{day}")))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .px(px(5.))
                        .py(px(4.))
                        .border_b_1()
                        .border_color(hairline)
                        .when(column < 6, |d| d.border_r_1())
                        .overflow_hidden()
                        .cursor_pointer()
                        .when(!in_month, |d| d.bg(gpui::black().opacity(0.18)))
                        .hover(|s| s.bg(content(0.022)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let now = this.tasks_now();
                            this.tasks_new(Some(slot_default(day, None, now)), cx);
                        }))
                        .child(
                            div()
                                .h(px(24.))
                                .mb(px(1.))
                                .flex()
                                .justify_center()
                                .items_center()
                                .child(
                                    div()
                                        .min_w(px(22.))
                                        .h(px(22.))
                                        .px(px(5.))
                                        .rounded(px(11.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(12.))
                                        .when(is_today, |d| {
                                            d.bg(crate::theme::accent())
                                                .font_weight(FontWeight::BOLD)
                                        })
                                        .text_color(if is_today {
                                            crate::theme::on_accent()
                                        } else {
                                            content(if in_month { 0.85 } else { 0.32 })
                                        })
                                        .child(if day.day() == 1 {
                                            short_date(day)
                                        } else {
                                            day.day().to_string()
                                        }),
                                ),
                        )
                        .children(chips)
                        .when(hidden > 0, |d| {
                            d.child(
                                div()
                                    .id(SharedString::from(format!("tasks-more-{day}")))
                                    .h(px(18.))
                                    .mt(px(2.))
                                    .px(px(6.))
                                    .rounded(px(5.))
                                    .text_size(px(11.5))
                                    .text_color(content(0.48))
                                    .hover(|s| s.bg(content(0.08)).text_color(content(0.92)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    // The rest of the day's runs are in its own view.
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.tasks_page.anchor = Some(day);
                                        this.tasks_set_view(View::Day, cx);
                                    }))
                                    .child(tf("+{} mais", &[&hidden])),
                            )
                        })
                }))
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .border_b_1()
                    .border_color(hairline)
                    .children(heads),
            )
            .children(weeks)
            .into_any_element()
    }

    /// The run's popover: what the task is, its last run and what to do with it.
    fn tasks_popover(&self, vw: f32, vh: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pop = self.tasks_page.popover.as_ref()?;
        let task = self.tasks_page.task(&pop.task)?;
        let id = task.id.clone();
        const WIDTH: f32 = 340.;
        let (x, y) = (f32::from(pop.at.x), f32::from(pop.at.y));
        let left = if x + 12. + WIDTH > vw - 12. {
            x - WIDTH - 12.
        } else {
            x + 12.
        }
        .max(12.);
        let top = (y - 8.).min(vh - 380.).max(12.);
        let now = self.tasks_now();
        let today = now.date();
        let running = pop.running;
        let confirm = pop.confirm;
        let row = |label: &'static str, body: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .mt(px(9.))
                .child(
                    div()
                        .w(px(62.))
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(content(0.48))
                        .child(t(label)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_size(px(13.))
                        .text_color(content(0.9))
                        .child(body),
                )
        };
        let plain = |text: String| div().min_w_0().truncate().child(text).into_any_element();
        let tag = |text: String| {
            div()
                .px(px(7.))
                .py(px(1.))
                .rounded(px(9.))
                .bg(content(0.08))
                .text_size(px(11.))
                .text_color(content(0.7))
                .child(text)
        };
        // When: the run clicked, or the next one.
        let when = match (&pop.occ, task.next) {
            (Some(occ), _) => Some((occ.at, occ.run != Run::Planned && occ.at <= now)),
            (None, Some(at)) => Some((at, false)),
            (None, None) => None,
        };
        let when_text = match when {
            Some((at, true)) => tf(
                "Executou {} às {}",
                &[&day_label(at.date(), today), &clock(at.time())],
            ),
            Some((at, false)) => tf(
                "Próxima execução {} às {}",
                &[&day_label(at.date(), today), &clock(at.time())],
            ),
            None => t("Sem próxima execução").to_owned(),
        };
        // The run box: this run's result, or the last one's.
        let this_run = pop
            .occ
            .as_ref()
            .filter(|occ| occ.run != Run::Planned)
            .map(|occ| (occ.at, occ.run == Run::Done, occ.session.clone(), true));
        let last_run = this_run.or_else(|| {
            task.last_run
                .map(|(at, ok)| (at, ok, task.last_session.clone(), false))
        });
        let run_box = match last_run {
            None => div()
                .mt(px(12.))
                .px(px(10.))
                .py(px(9.))
                .rounded(px(9.))
                .border_1()
                .border_color(content(0.07))
                .bg(content(0.04))
                .flex()
                .items_center()
                .gap(px(9.))
                .child(
                    div()
                        .text_size(px(15.))
                        .text_color(content(0.48))
                        .child("○"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .child(t("Ainda não executada"))
                        .when_some(task.next, |d, at| {
                            d.child(div().text_color(content(0.48)).child(tf(
                                "A primeira execução será {}.",
                                &[&day_label(at.date(), today)],
                            )))
                        }),
                ),
            Some((at, ok, session, this)) => {
                let heading = match (this, ok) {
                    (true, true) => t("Esta execução: concluída"),
                    (true, false) => t("Esta execução: falhou"),
                    (false, true) => t("Última execução: concluída"),
                    (false, false) => t("Última execução: falhou"),
                };
                let tint = if ok { 0x4cc38a_u32 } else { 0xf0616d_u32 };
                let chat = session.map(|session| {
                    div()
                        .id("tasks-open-chat")
                        .h(px(26.))
                        .px(px(10.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(content(0.13))
                        .bg(content(0.04))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .text_size(px(12.))
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.09)))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.tasks_open_chat(&session, cx)),
                        )
                        .child(t("Abrir conversa"))
                });
                div()
                    .mt(px(12.))
                    .px(px(10.))
                    .py(px(9.))
                    .rounded(px(9.))
                    .border_1()
                    .border_color(gpui::rgb(tint).opacity(0.22))
                    .bg(gpui::rgb(tint).opacity(0.08))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(gpui::rgb(tint))
                            .child(if ok { "✓" } else { "✗" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(heading))
                            .child(div().text_color(content(0.48)).child(format!(
                                "{}, {}",
                                day_label(at.date(), today),
                                clock(at.time())
                            ))),
                    )
                    .children(chat)
            }
        };
        let effort = task
            .effort
            .as_deref()
            .map(|level| format!(" · {}", effort_label(level)))
            .unwrap_or_default();
        let model_row = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .child(provider_mark(&task.provider, 14.))
            .child(plain(format!(
                "{}{effort}",
                self.model_display(data::provider_id(&task.provider), task.model.as_deref())
            )))
            .when(task.fast, |d| d.child(tag(format!("⚡ {}", t("Rápido")))))
            .into_any_element();
        let project_row = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .min_w_0()
            .child(self.tasks_avatar(&task.project_id, 20.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tasks_project_name(&task.project_id)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(content(0.48))
                    .child(self.tasks_project_path(&task.project_id)),
            )
            .into_any_element();
        let access = access_look(task.approval.as_deref().unwrap_or("ask")).1;
        let access_row =
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(access)
                .child(div().text_size(px(11.5)).text_color(content(0.48)).child(
                    if task.worktree {
                        t("nova worktree")
                    } else {
                        t("local")
                    },
                ))
                .into_any_element();
        let red: gpui::Hsla = gpui::rgb(0xf0616d).into();
        let button = |key: &'static str, label: String, hot: bool| {
            div()
                .id(key)
                .h(px(26.))
                .px(px(10.))
                .rounded(px(8.))
                .border_1()
                .border_color(if hot { red.opacity(0.4) } else { content(0.13) })
                .bg(if hot { red.opacity(0.2) } else { content(0.04) })
                .text_color(if hot || key == "tasks-delete" {
                    red
                } else {
                    content(0.92)
                })
                .flex()
                .items_center()
                .text_size(px(12.))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.09)))
                .child(label)
        };
        let (edit_id, run_id, delete_id, toggle_id) =
            (id.clone(), id.clone(), id.clone(), id.clone());
        Some(
            div()
                .id("tasks-popover")
                .occlude()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(WIDTH))
                .p(px(14.))
                .rounded(px(12.))
                .border_1()
                .border_color(content(0.13))
                .child(crate::theme::floating_surface(12.))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(10.))
                        .child(div().pt(px(2.)).child(provider_mark(&task.provider, 16.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(15.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(content(0.95))
                                        .child(task.title.clone()),
                                )
                                .child(
                                    div()
                                        .mt(px(2.))
                                        .text_size(px(13.))
                                        .text_color(content(0.48))
                                        .child(rule_words(&task.rule)),
                                ),
                        )
                        .child(
                            div()
                                .id("tasks-popover-close")
                                .size(px(24.))
                                .rounded(px(6.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(18.))
                                .text_color(content(0.48))
                                .cursor_pointer()
                                .hover(|s| s.bg(content(0.08)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.tasks_page.popover = None;
                                    cx.notify();
                                }))
                                .child("×"),
                        ),
                )
                .child(row("Quando", plain(when_text)))
                .child(row("Projeto", project_row))
                .child(row("Modelo", model_row))
                .child(row("Acesso", access_row))
                .child(run_box)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .mt(px(12.))
                        .pt(px(12.))
                        .border_t_1()
                        .border_color(content(0.07))
                        .child(
                            button("tasks-edit", t("Editar").to_owned(), false).on_click(
                                cx.listener(move |this, _, _, cx| this.tasks_edit(&edit_id, cx)),
                            ),
                        )
                        .child(
                            button(
                                "tasks-run-now",
                                if running {
                                    t("Executando…").to_owned()
                                } else {
                                    format!("▶ {}", t("Executar agora"))
                                },
                                false,
                            )
                            .when(running, |d| d.opacity(0.6))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.tasks_run_now(&run_id, cx)),
                            ),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(7.))
                                .text_size(px(12.))
                                .text_color(content(0.48))
                                .child(t("Ativa"))
                                .child(
                                    div()
                                        .id("tasks-toggle")
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.tasks_toggle(&toggle_id, cx)
                                        }))
                                        .child(crate::settings_pages::switch_track(
                                            SWITCH_ID,
                                            task.enabled,
                                            true,
                                        )),
                                ),
                        )
                        .child(
                            button(
                                "tasks-delete",
                                if confirm {
                                    t("Confirmar exclusão").to_owned()
                                } else {
                                    t("Excluir").to_owned()
                                },
                                confirm,
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.tasks_delete(&delete_id, cx)
                                }),
                            ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// A colour mixed toward another: `amount` 0 is `from`, 1 is `to`.
fn mixed(from: gpui::Hsla, to: gpui::Hsla, amount: f32) -> gpui::Hsla {
    let (a, b) = (gpui::Rgba::from(from), gpui::Rgba::from(to));
    gpui::Rgba {
        r: a.r + (b.r - a.r) * amount,
        g: a.g + (b.g - a.g) * amount,
        b: a.b + (b.b - a.b) * amount,
        a: a.a + (b.a - a.a) * amount,
    }
    .into()
}

/// A provider's colour a little brighter (deeper on a light theme): the time on a chip.
fn bright(color: gpui::Hsla) -> gpui::Hsla {
    if crate::theme::is_light() {
        mixed(color, gpui::black(), 0.25)
    } else {
        mixed(color, gpui::white(), 0.45)
    }
}

/// A path with the home folder as `~`.
fn tidy_path(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_owned(),
    }
}

/// A provider's name on its tab: "Claude" rather than "Claude Code".
fn provider_short(provider: &str) -> &'static str {
    match provider {
        "claude" => "Claude",
        other => provider_title(other),
    }
}

impl Shell {
    // ------------------------------------------------------------ the modal's view

    /// Changes the form, then redraws.
    fn tasks_edit_sheet(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Sheet)) {
        if let Some(sheet) = self.tasks_page.sheet.as_mut() {
            change(sheet);
        }
        cx.notify();
    }

    /// A pick in the catalog popover: the draft's provider and model. The effort follows the
    /// model: a level it lacks is dropped (`tasks_settle`), and a new provider starts unchosen.
    pub(super) fn tasks_choose_model(
        &mut self,
        provider: &'static str,
        id: String,
        cx: &mut Context<Self>,
    ) {
        self.model_query.clear();
        self.tasks_edit_sheet(cx, |sheet| {
            if sheet.provider != provider {
                sheet.effort = None;
            }
            sheet.provider = provider;
            sheet.model = Some(id);
            sheet.set_popup(None);
        });
        self.tasks_settle();
    }

    /// The popover's "‹ Esforço e velocidade" header: back to the modal, whose own controls
    /// hold the effort.
    pub(super) fn tasks_close_model(&mut self, cx: &mut Context<Self>) {
        self.tasks_edit_sheet(cx, |sheet| sheet.set_popup(None));
    }

    /// The model button opens the catalog on the draft's provider, or closes it.
    fn tasks_toggle_model(&mut self, cx: &mut Context<Self>) {
        let open = self.tasks_page.model_open();
        if !open && let Some(provider) = self.tasks_page.sheet.as_ref().map(|s| s.provider) {
            self.reset_catalog(provider);
        }
        self.tasks_edit_sheet(cx, |sheet| {
            sheet.field = None;
            sheet.set_popup(if open { None } else { Some(Popup::Model) });
        });
    }

    /// A text field drawn as a box; the focused one shows the caret.
    fn tasks_field(
        &self,
        id: &'static str,
        field: Field,
        text: &str,
        placeholder: &'static str,
        lines: bool,
        invalid: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let focused = self
            .tasks_page
            .sheet
            .as_ref()
            .is_some_and(|s| s.field == Some(field));
        let caret = || self.popup_caret();
        let content_el: AnyElement = if text.is_empty() && !focused {
            div()
                .text_color(content(0.35))
                .child(t(placeholder))
                .into_any_element()
        } else if lines {
            self.tasks_wrapped(text, focused)
        } else {
            div()
                .flex()
                .child(text.to_owned())
                .when(focused, |d| d.child(caret()))
                .into_any_element()
        };
        div()
            .id(id)
            .w_full()
            .px(px(12.))
            .when(lines, |d| {
                d.py(px(10.))
                    .min_h(px(112.))
                    .max_h(px(PROMPT_MAX_H))
                    .overflow_y_scroll()
                    .track_scroll(&self.tasks_page.prompt_scroll)
            })
            .when(!lines, |d| d.h(px(36.)).flex().items_center())
            .rounded(px(8.))
            .border_1()
            .border_color(if invalid {
                gpui::rgb(0xf0616d).into()
            } else if focused {
                crate::theme::accent()
            } else {
                content(0.13)
            })
            .bg(crate::theme::field_fill())
            .text_size(px(13.))
            .text_color(content(0.92))
            .cursor_text()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.tasks_edit_sheet(cx, |sheet| {
                    sheet.field = Some(field);
                    sheet.set_popup(None);
                });
                this.field_caret_moved.set(Instant::now());
            }))
            .child(content_el)
    }

    /// The prompt's text wrapped to the field, as the composer lays its own out, with the caret
    /// painted over the laid out text at its end. The caret is followed when the text changes.
    fn tasks_wrapped(&self, text: &str, focused: bool) -> AnyElement {
        let shown = if text.is_empty() { " " } else { text };
        let styled = gpui::StyledText::new(SharedString::from(shown.to_owned()));
        let layout = styled.layout().clone();
        let len = text.len();
        let lit = focused && self.field_caret_lit();
        let scroll = self.tasks_page.prompt_scroll.clone();
        let seen = self.tasks_page.prompt_seen.clone();
        let overlay = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let line = layout.line_height();
                let at = if len == 0 {
                    Some(bounds.origin)
                } else {
                    layout.position_for_index(len)
                };
                let Some(at) = at else {
                    return;
                };
                if seen.replace(len) != len {
                    let top = at.y - bounds.origin.y;
                    let scrolled = -scroll.offset().y;
                    let target = follow_caret(top, line, scrolled, scroll.bounds().size.height);
                    if target != scrolled {
                        scroll.set_offset(gpui::point(px(0.), -target));
                        window.on_next_frame(|window, _| window.refresh());
                    }
                }
                if lit {
                    let height = px(FIELD_CARET_HEIGHT);
                    window.paint_quad(
                        gpui::fill(
                            Bounds::new(
                                gpui::point(at.x, at.y + (line - height) / 2.),
                                gpui::size(px(crate::theme::CARET_WIDTH), height),
                            ),
                            crate::theme::caret_color(),
                        )
                        .corner_radii(px(1.)),
                    );
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .relative()
            .w_full()
            .min_w_0()
            .child(styled)
            .child(overlay)
            .into_any_element()
    }

    /// A rounded chip: a preset, an example, a repeat's weekday.
    fn tasks_chip_button(
        id: SharedString,
        label: &str,
        on: bool,
        enabled: bool,
        suggestion: bool,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(28.))
            .px(px(12.))
            .rounded(px(14.))
            .flex()
            .items_center()
            .text_size(px(12.))
            .border_1()
            .border_color(if on {
                crate::theme::accent()
            } else if suggestion {
                transparent()
            } else {
                content(0.13)
            })
            .bg(if on {
                crate::theme::accent_wash(0.16)
            } else if suggestion {
                content(0.05)
            } else {
                content(0.03)
            })
            .text_color(if !enabled {
                content(0.3)
            } else if on {
                content(1.)
            } else if suggestion {
                content(0.5)
            } else {
                content(0.75)
            })
            .when(enabled, |d| {
                d.cursor_pointer().hover(|s| {
                    s.border_color(crate::theme::accent())
                        .bg(crate::theme::accent_wash(0.1))
                        .text_color(content(1.))
                })
            })
            .child(label.to_owned())
    }

    /// A segmented control with the accent pill sliding under the chosen segment; nothing is
    /// highlighted while `chosen` is `None`.
    fn tasks_seg(
        key: &'static str,
        labels: Vec<String>,
        chosen: Option<usize>,
        cx: &mut Context<Self>,
        pick: impl Fn(&mut Shell, usize, &mut Context<Shell>) + 'static,
    ) -> Div {
        let pick: Rc<dyn Fn(&mut Shell, usize, &mut Context<Shell>)> = Rc::new(pick);
        let segments = labels
            .into_iter()
            .enumerate()
            .map(|(ix, label)| {
                let on = chosen == Some(ix);
                let pick = pick.clone();
                div()
                    .id((key, ix))
                    .relative()
                    .flex_1()
                    .h(px(30.))
                    .px(px(4.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.5))
                    .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .text_color(if on {
                        crate::theme::on_accent()
                    } else {
                        content(0.5)
                    })
                    .cursor_pointer()
                    .when(!on, |d| d.hover(|s| s.text_color(content(0.92))))
                    .child(crate::segment_pill::probe(key, ix))
                    .on_click(cx.listener(move |this, _, _, cx| pick(this, ix, cx)))
                    .child(div().truncate().child(label))
            })
            .collect::<Vec<_>>();
        div()
            .relative()
            .w_full()
            .flex()
            .p(px(2.))
            .rounded(px(9.))
            .border_1()
            .border_color(content(0.07))
            .bg(content(0.05))
            .when_some(chosen, |d, ix| {
                d.child(crate::segment_pill::pill(
                    key,
                    ix,
                    crate::theme::accent(),
                    crate::theme::RADIUS_SM,
                ))
            })
            .children(segments)
    }

    /// A numbered step of the modal, divided from the one above.
    fn tasks_step_block(number: u32, title: &'static str, first: bool, rows: Vec<Div>) -> Div {
        div()
            .py(px(24.))
            .when(!first, |d| d.border_t_1().border_color(content(0.07)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mb(px(16.))
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(content(0.92))
                    .child(
                        div()
                            .size(px(20.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(crate::theme::accent_wash(0.16))
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(crate::theme::accent())
                            .child(number.to_string()),
                    )
                    .child(t(title)),
            )
            .children(rows)
    }

    /// A row of a step: its label in a column of fixed width, then the content.
    fn tasks_form_row(label: &'static str, top: bool, first: bool, body: impl IntoElement) -> Div {
        div()
            .flex()
            .gap(px(16.))
            .when(top, |d| d.items_start())
            .when(!top, |d| d.items_center())
            .when(!first, |d| d.mt(px(16.)))
            .child(
                div()
                    .w(px(LABEL_WIDTH))
                    .flex_shrink_0()
                    .when(top, |d| d.pt(px(9.)))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.48))
                    .child(t(label)),
            )
            .child(div().flex_1().min_w_0().child(body))
    }

    fn tasks_hint(text: impl Into<SharedString>, color: gpui::Hsla) -> Div {
        div()
            .mt(px(8.))
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(color)
            .child(text.into())
    }

    /// "− 2 +": a small stepper for a count.
    fn tasks_stepper(
        id: &'static str,
        value: u32,
        cx: &mut Context<Self>,
        step: impl Fn(&mut Sheet, bool) + 'static,
    ) -> Div {
        let step: Rc<dyn Fn(&mut Sheet, bool)> = Rc::new(step);
        let buttons = [("−", false), ("+", true)].map(|(sign, up)| {
            let step = step.clone();
            div()
                .id((id, usize::from(up)))
                .size(px(28.))
                .rounded(px(7.))
                .border_1()
                .border_color(content(0.13))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(15.))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tasks_edit_sheet(cx, |sheet| step(sheet, up));
                }))
                .child(sign)
        });
        let [minus, plus] = buttons;
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(minus)
            .child(
                div()
                    .min_w(px(28.))
                    .flex()
                    .justify_center()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(value.to_string()),
            )
            .child(plus)
    }

    /// The hour or the minute: a box with its two arrows; the arrow keys step it while focused.
    fn tasks_time_box(
        &self,
        id: &'static str,
        field: Field,
        text: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let focused = self
            .tasks_page
            .sheet
            .as_ref()
            .is_some_and(|s| s.field == Some(field));
        let hours = field == Field::Hour;
        let arrow = |up: bool, cx: &mut Context<Self>| {
            div()
                .id((id, usize::from(up)))
                .w(px(18.))
                .h(px(14.))
                .rounded(px(3.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.1)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tasks_edit_sheet(cx, |sheet| sheet.step_time(hours, up));
                }))
                .child(
                    icon(if up { "chevron-up" } else { "chevron-down" }, 9.)
                        .text_color(content(0.48)),
                )
        };
        div()
            .id(id)
            .h(px(36.))
            .pl(px(12.))
            .pr(px(4.))
            .flex()
            .items_center()
            .gap(px(4.))
            .rounded(px(8.))
            .border_1()
            .border_color(if focused {
                crate::theme::accent()
            } else {
                content(0.13)
            })
            .bg(crate::theme::field_fill())
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.tasks_edit_sheet(cx, |sheet| {
                    sheet.field = Some(field);
                    sheet.set_popup(None);
                });
            }))
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(text),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(arrow(true, cx))
                    .child(arrow(false, cx)),
            )
    }

    /// What each access mode does, under the control.
    fn tasks_access_note(mode: &str) -> &'static str {
        match mode {
            "ask" => t("Pausa e pergunta antes de editar arquivos ou rodar comandos."),
            "auto" => t("Um revisor aprova o que é seguro e só pergunta nas ações arriscadas."),
            _ => t("Roda tudo sem perguntar. Use só em projetos de confiança."),
        }
    }

    fn tasks_sheet_view(&self, vw: f32, vh: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sheet = self.tasks_page.sheet.as_ref()?;
        let now = self.tasks_now();
        let today = now.date();
        let exec = self.execution(sheet.provider, sheet.model.as_deref());
        let problems = sheet.problems(!exec.levels.is_empty(), now);
        let show = sheet.tried;
        let red: gpui::Hsla = gpui::rgb(0xf0616d).into();
        let amber: gpui::Hsla = gpui::rgb(0xe9b44c).into();
        let accent = crate::theme::accent();
        let dim = content(0.48);

        // -------- 1. O que fazer
        let examples = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .mt(px(12.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.34))
                    .child(t("Sugestões")),
            )
            .children(
                EXAMPLES
                    .iter()
                    .enumerate()
                    .map(|(ix, (label, title, prompt))| {
                        let (title, prompt) = (t(title), t(prompt));
                        Self::tasks_chip_button(
                            SharedString::from(format!("tasks-example-{ix}")),
                            t(label),
                            false,
                            true,
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tasks_edit_sheet(cx, |sheet| {
                                sheet.title = title.to_owned();
                                sheet.prompt = prompt.to_owned();
                            });
                        }))
                    }),
            );
        let step_one = Self::tasks_step_block(
            1,
            "O que fazer",
            true,
            vec![
                div()
                    .child(self.tasks_field(
                        "tasks-title",
                        Field::Title,
                        &sheet.title,
                        "Título (ex.: Rodar testes e resumir falhas)",
                        false,
                        false,
                        cx,
                    ))
                    .child(div().mt(px(8.)).child(self.tasks_field(
                        "tasks-prompt",
                        Field::Prompt,
                        &sheet.prompt,
                        "Descreva o que o agente deve fazer, com o resultado que você espera…",
                        true,
                        show && problems.prompt,
                        cx,
                    )))
                    .when(show && problems.prompt, |d| {
                        d.child(Self::tasks_hint(
                            t("Escreva o que o agente deve fazer. Sem prompt não há tarefa."),
                            red,
                        ))
                    })
                    .child(examples),
            ],
        );

        // -------- 2. Quem e onde
        let project_open = sheet.popup == Some(Popup::Project);
        let project_button = div()
            .id("tasks-project")
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(36.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(if project_open { accent } else { content(0.13) })
            .bg(crate::theme::field_fill())
            .cursor_pointer()
            .hover(|s| s.border_color(content(0.25)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| {
                this.tasks_edit_sheet(cx, |sheet| {
                    sheet.field = None;
                    sheet.set_popup(if sheet.popup == Some(Popup::Project) {
                        None
                    } else {
                        Some(Popup::Project)
                    });
                });
            }))
            .child(self.tasks_anchor("tasks-project"))
            .child(self.tasks_avatar(&sheet.project, 24.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(8.))
                    .overflow_hidden()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(13.))
                            .child(self.tasks_project_name(&sheet.project)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(dim)
                            .child(self.tasks_project_path(&sheet.project)),
                    ),
            )
            .child(icon("chevron-down", 12.).text_color(dim));
        let worktree = Self::tasks_seg(
            "tasks-worktree",
            vec![t("Local").to_owned(), t("Nova worktree").to_owned()],
            Some(usize::from(sheet.worktree)),
            cx,
            |this, ix, cx| this.tasks_edit_sheet(cx, |sheet| sheet.worktree = ix == 1),
        );
        let project_row = Self::tasks_form_row(
            "Projeto",
            false,
            true,
            div()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(project_button)
                        .child(div().w(px(224.)).flex_shrink_0().child(worktree)),
                )
                .child(Self::tasks_hint(
                    if sheet.worktree {
                        t("Cada execução cria a própria worktree e branch, sem mexer no que está aberto.")
                    } else {
                        t("Roda direto na pasta do projeto, com o que estiver aberto agora.")
                    },
                    dim,
                )),
        );
        // The model: the composer's pill, which opens its catalog popover under it.
        let model_open = sheet.popup == Some(Popup::Model);
        let info = self.execution(sheet.provider, sheet.model.as_deref());
        let model_button = div()
            .id("tasks-model")
            .relative()
            .h(px(34.))
            .px(px(10.))
            .max_w(px(360.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(if model_open { accent } else { content(0.13) })
            .bg(crate::theme::field_fill())
            .text_size(px(13.))
            .text_color(content(0.92))
            .cursor_pointer()
            .hover(|s| s.border_color(content(0.25)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| this.tasks_toggle_model(cx)))
            .child(self.tasks_anchor("tasks-model"))
            .child(provider_mark(sheet.provider, 16.))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.model_display(sheet.provider, sheet.model.as_deref())),
            )
            .child(icon("chevron-down", 12.).text_color(dim));
        let model_row = Self::tasks_form_row(
            "Modelo",
            false,
            false,
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(model_button)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(dim)
                        .child(model_note(info.levels.len(), info.fast_ok)),
                ),
        );
        // The effort: required, nothing preselected.
        let effort_chosen = sheet
            .effort
            .as_ref()
            .and_then(|level| exec.levels.iter().position(|l| l == level));
        let level_ids = exec.levels.clone();
        let effort_body = if exec.levels.is_empty() {
            Self::tasks_hint(t("Escolha um modelo para ajustar o esforço."), dim)
                .mt(px(0.))
                .into_any_element()
        } else {
            let hint = match (&sheet.effort, show && problems.effort) {
                (None, true) => Self::tasks_hint(t("Escolha o esforço para criar a tarefa."), red),
                (None, false) => Self::tasks_hint(
                    t("Escolha um nível. O Sirus nunca decide o esforço por você."),
                    accent,
                ),
                (Some(level), _) => Self::tasks_hint(
                    tf(
                        "{} · este modelo tem {} níveis.",
                        &[&effort_label(level), &exec.levels.len()],
                    ),
                    dim,
                ),
            };
            div()
                .child(Self::tasks_seg(
                    "tasks-effort",
                    exec.levels
                        .iter()
                        .map(|level| effort_label(level))
                        .collect(),
                    effort_chosen,
                    cx,
                    move |this, ix, cx| {
                        let level = level_ids.get(ix).cloned();
                        this.tasks_edit_sheet(cx, |sheet| sheet.effort = level);
                    },
                ))
                .child(hint)
                .into_any_element()
        };
        let effort_row = Self::tasks_form_row("Esforço", true, false, effort_body);
        // Fast and access, side by side.
        let fast = div()
            .id("tasks-fast")
            .h(px(36.))
            .pl(px(12.))
            .pr(px(10.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(12.))
            .rounded(px(8.))
            .border_1()
            .when(exec.fast_ok, |d| {
                d.border_color(amber.opacity(0.2)).bg(amber.opacity(0.07))
            })
            .when(!exec.fast_ok, |d| {
                d.border_color(content(0.07))
                    .bg(content(0.03))
                    .text_color(dim)
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(icon("zap", 13.).text_color(if exec.fast_ok { amber } else { dim }))
                    .child(t("Rápido")),
            )
            .child(if exec.fast_ok {
                div()
                    .id("tasks-fast-switch")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tasks_edit_sheet(cx, |sheet| sheet.fast = !sheet.fast);
                    }))
                    .child(crate::settings_pages::switch_track(
                        SWITCH_ID + 500,
                        sheet.fast,
                        true,
                    ))
                    .into_any_element()
            } else {
                div()
                    .text_size(px(11.5))
                    .child(t("indisponível neste modelo"))
                    .into_any_element()
            });
        let modes = ["ask", "auto", "full"];
        let access = Self::tasks_seg(
            "tasks-access",
            modes
                .iter()
                .map(|mode| access_look(mode).1.to_owned())
                .collect(),
            modes.iter().position(|mode| *mode == sheet.approval),
            cx,
            move |this, ix, cx| {
                let mode = modes[ix.min(2)];
                this.tasks_edit_sheet(cx, |sheet| sheet.approval = mode);
            },
        );
        let run_row = Self::tasks_form_row(
            "Execução",
            true,
            false,
            div()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(fast)
                        .child(div().flex_1().min_w_0().child(access)),
                )
                .child(Self::tasks_hint(
                    Self::tasks_access_note(sheet.approval),
                    if sheet.approval == "full" { amber } else { dim },
                )),
        );
        let step_two = Self::tasks_step_block(
            2,
            "Quem e onde",
            false,
            vec![project_row, model_row, effort_row, run_row],
        );

        // -------- 3. Quando
        let date_open = sheet.popup == Some(Popup::Date);
        let date_text = {
            let label = capitalized(&day_label(sheet.date, today));
            if sheet.date == today {
                format!("{label} · {}", t("hoje"))
            } else {
                label
            }
        };
        let date_button = div()
            .id("tasks-date")
            .relative()
            .w(px(248.))
            .flex_shrink_0()
            .h(px(36.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(if date_open { accent } else { content(0.13) })
            .bg(crate::theme::field_fill())
            .cursor_pointer()
            .hover(|s| s.border_color(content(0.25)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| {
                this.tasks_edit_sheet(cx, |sheet| {
                    sheet.field = None;
                    let open = if sheet.popup == Some(Popup::Date) {
                        None
                    } else {
                        sheet.month = month_first(sheet.date);
                        Some(Popup::Date)
                    };
                    sheet.set_popup(open);
                });
            }))
            .child(self.tasks_anchor("tasks-date"))
            .child(icon("calendar", 15.).text_color(dim))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .child(date_text),
            )
            .child(icon("chevron-down", 12.).text_color(dim));
        let time = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(18.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(self.tasks_time_box(
                "tasks-hour",
                Field::Hour,
                format!("{:02}", sheet.minutes / 60 % 24),
                cx,
            ))
            .child(":")
            .child(self.tasks_time_box(
                "tasks-minute",
                Field::Minute,
                format!("{:02}", sheet.minutes % 60),
                cx,
            ));
        let when_row = Self::tasks_form_row(
            "Data e hora",
            false,
            true,
            div()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(date_button)
                        .child(time),
                )
                .when(show && problems.past, |d| {
                    d.child(Self::tasks_hint(
                        t("Esse horário já passou. Escolha um horário futuro ou repita a tarefa."),
                        red,
                    ))
                }),
        );
        let when_now = sheet.when();
        let presets = Self::tasks_form_row(
            "Atalhos",
            false,
            false,
            div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .children(PRESETS.iter().enumerate().map(|(ix, (preset, label))| {
                    let moment = preset_time(*preset, now);
                    let chip = Self::tasks_chip_button(
                        SharedString::from(format!("tasks-preset-{ix}")),
                        t(label),
                        moment == Some(when_now),
                        moment.is_some(),
                        false,
                    );
                    match moment {
                        Some(moment) => chip.on_click(cx.listener(move |this, _, _, cx| {
                            this.tasks_edit_sheet(cx, |sheet| {
                                sheet.set_when(moment);
                                sheet.set_popup(None);
                            });
                        })),
                        None => chip,
                    }
                })),
        );
        // Repeat.
        let repeat_seg = Self::tasks_seg(
            "tasks-repeat",
            REPEATS
                .iter()
                .map(|(_, label)| t(label).to_owned())
                .collect(),
            REPEATS
                .iter()
                .position(|(repeat, _)| *repeat == sheet.repeat),
            cx,
            |this, ix, cx| {
                let repeat = REPEATS[ix.min(REPEATS.len() - 1)].0;
                this.tasks_edit_sheet(cx, |sheet| sheet.set_repeat(repeat));
            },
        );
        let repeat_detail: Option<Div> = match sheet.repeat {
            Repeat::Weekly => Some(div().flex().gap(px(8.)).children(
                weekday_order(monday_first()).into_iter().map(|day| {
                    let on = sheet.weekdays[day];
                    div()
                        .id(("tasks-weekday", day))
                        .size(px(32.))
                        .rounded_full()
                        .border_1()
                        .border_color(if on { accent } else { content(0.13) })
                        .when(on, |d| d.bg(accent))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if on {
                            crate::theme::on_accent()
                        } else {
                            content(0.48)
                        })
                        .cursor_pointer()
                        .when(!on, |d| {
                            d.hover(|s| s.border_color(content(0.3)).text_color(content(1.)))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tasks_edit_sheet(cx, |sheet| {
                                sheet.weekdays[day] = !sheet.weekdays[day];
                            });
                        }))
                        .child(weekday_letter(day))
                }),
            )),
            Repeat::Monthly => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(t("Todo dia"))
                    .child(Self::tasks_stepper(
                        "tasks-dom",
                        u32::from(sheet.dom),
                        cx,
                        |sheet, up| {
                            let next = i32::from(sheet.dom) + if up { 1 } else { -1 };
                            sheet.dom = next.clamp(1, 31) as u8;
                            sheet.dom_edited = true;
                        },
                    ))
                    .when(sheet.dom > 28, |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(dim)
                                .child(t("Em meses curtos roda no último dia.")),
                        )
                    }),
            ),
            Repeat::Custom => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(t("A cada"))
                    .child(Self::tasks_stepper(
                        "tasks-every",
                        sheet.every,
                        cx,
                        |sheet, up| {
                            let next = sheet.every as i32 + if up { 1 } else { -1 };
                            sheet.every = next.clamp(1, 30) as u32;
                        },
                    ))
                    .child(div().w(px(170.)).child(Self::tasks_seg(
                        "tasks-unit",
                        vec![
                            Unit::Days.label().to_owned(),
                            Unit::Weeks.label().to_owned(),
                        ],
                        Some(usize::from(sheet.unit == Unit::Weeks)),
                        cx,
                        |this, ix, cx| {
                            this.tasks_edit_sheet(cx, |sheet| {
                                sheet.unit = if ix == 0 { Unit::Days } else { Unit::Weeks };
                            });
                        },
                    ))),
            ),
            _ => None,
        };
        let repeat_row = Self::tasks_form_row(
            "Repetir",
            true,
            false,
            div()
                .child(repeat_seg)
                .when_some(repeat_detail, |d, detail| {
                    d.child(div().pt(px(12.)).child(detail))
                })
                .when(show && problems.weekdays, |d| {
                    d.child(Self::tasks_hint(
                        t("Escolha ao menos um dia da semana."),
                        red,
                    ))
                }),
        );
        let next_runs = sheet
            .rule()
            .map(|rule| upcoming(&rule, sheet.start(), now, 4))
            .unwrap_or_default();
        let next_list = if next_runs.is_empty() {
            div()
                .text_size(px(12.))
                .text_color(dim)
                .child(t("Nenhuma execução futura nesse horário."))
        } else {
            div().children(next_runs.iter().enumerate().map(|(ix, at)| {
                div()
                    .flex()
                    .gap(px(8.))
                    .py(px(2.))
                    .text_size(px(12.))
                    .text_color(dim)
                    .child(
                        div()
                            .font_weight(if ix == 0 {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .text_color(content(0.68))
                            .child(capitalized(&day_label(at.date(), today))),
                    )
                    .child(format!(
                        "· {}{}",
                        clock(at.time()),
                        if ix == 0 {
                            format!(" · {}", t("primeira"))
                        } else {
                            String::new()
                        }
                    ))
            }))
        };
        let step_three = Self::tasks_step_block(
            3,
            "Quando",
            false,
            vec![
                when_row,
                presets,
                repeat_row,
                Self::tasks_form_row("Próximas", true, false, next_list),
            ],
        );

        // -------- header and footer
        let editing = sheet.editing.is_some();
        let header = div()
            .flex_shrink_0()
            .flex()
            .items_start()
            .px(px(32.))
            .pt(px(24.))
            .pb(px(16.))
            .border_b_1()
            .border_color(content(0.07))
            .child(
                div()
                    .flex_1()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(px(24.))
                            .text_color(content(0.95))
                            .child(if editing {
                                t("Editar tarefa")
                            } else {
                                t("Nova tarefa")
                            }),
                    )
                    .child(
                        div()
                            .mt(px(4.))
                            .text_size(px(13.))
                            .text_color(dim)
                            .child(t(
                                "O Sirus roda o prompt sozinho, no horário escolhido, e guarda a conversa de cada execução.",
                            )),
                    ),
            )
            .child(
                div()
                    .id("tasks-sheet-close")
                    .size(px(32.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.08)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tasks_page.sheet = None;
                        cx.notify();
                    }))
                    .child(icon("close", 14.).text_color(dim)),
            );
        // The live sentence.
        let sep = || div().text_color(content(0.3)).child("·");
        let summary = div()
            .flex_1()
            .min_w_0()
            .pr(px(16.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(6.))
            .text_size(px(13.))
            .line_height(px(20.))
            .text_color(content(0.85))
            .child(match sheet.rule_text() {
                Some(text) => div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(content(1.))
                    .child(text),
                None => div().text_color(accent).child(t("Escolha os dias")),
            })
            .child(sep())
            .child(div().child(format!(
                "{} {}",
                provider_short(sheet.provider),
                self.model_display(sheet.provider, sheet.model.as_deref())
            )))
            .child(sep())
            .child(match &sheet.effort {
                Some(level) => div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(content(1.))
                    .child(effort_label(level)),
                None => div().text_color(accent).child(t("escolha o esforço")),
            })
            .when(sheet.fast && exec.fast_ok, |d| {
                d.child(sep()).child(format!("⚡ {}", t("Rápido")))
            })
            .child(sep())
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .child(t("no projeto"))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(content(1.))
                            .child(self.tasks_project_name(&sheet.project)),
                    )
                    .when(sheet.worktree, |d| {
                        d.child(div().text_color(content(0.3)).child(t("(nova worktree)")))
                    }),
            );
        let footer = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .px(px(32.))
            .py(px(16.))
            .border_t_1()
            .border_color(content(0.07))
            .bg(gpui::black().opacity(0.12))
            .when_some(sheet.error.clone(), |d, error| {
                d.child(
                    div()
                        .mb(px(8.))
                        .text_size(px(12.))
                        .text_color(red)
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(summary)
                    .child(
                        div()
                            .id("tasks-cancel")
                            .h(px(36.))
                            .px(px(20.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(content(0.13))
                            .bg(content(0.04))
                            .flex()
                            .items_center()
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.09)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tasks_page.sheet = None;
                                cx.notify();
                            }))
                            .child(t("Cancelar")),
                    )
                    .child(
                        div()
                            .id("tasks-save")
                            .h(px(36.))
                            .px(px(20.))
                            .rounded(px(8.))
                            .bg(accent)
                            .flex()
                            .items_center()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(crate::theme::on_accent())
                            .when(sheet.saving, |d| d.opacity(0.6))
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.9))
                            .on_click(cx.listener(|this, _, _, cx| this.tasks_save(cx)))
                            .child(if editing {
                                t("Salvar alterações")
                            } else {
                                t("Criar tarefa")
                            }),
                    ),
            );
        let body = div()
            .id("tasks-sheet-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.tasks_page.sheet_scroll)
            .px(px(32.))
            .child(step_one)
            .child(step_two)
            .child(step_three);
        let modal = div()
            .id("tasks-sheet")
            .relative()
            .w(px(MODAL_WIDTH.min(vw - 56.)))
            .h(px((vh * 0.9).min(880.)))
            .flex()
            .flex_col()
            .rounded(px(16.))
            .border_1()
            .border_color(content(0.13))
            .overflow_hidden()
            .child(crate::theme::floating_surface(16.))
            .shadow_lg()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.tasks_edit_sheet(cx, |sheet| {
                        sheet.field = None;
                        sheet.set_popup(None);
                    });
                    cx.stop_propagation();
                }),
            )
            .child(header)
            .child(body)
            .child(footer);
        let popups = self.tasks_popups(sheet, today, vh, cx);
        Some(
            div()
                .id("tasks-sheet-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.6))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.tasks_page.sheet = None;
                        cx.notify();
                        cx.stop_propagation();
                    }),
                )
                .child(modal)
                .children(popups)
                .into_any_element(),
        )
    }

    /// The project list and the date calendar, opened under their buttons.
    fn tasks_popups(
        &self,
        sheet: &Sheet,
        today: NaiveDate,
        vh: f32,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let accent = crate::theme::accent();
        let dim = content(0.48);
        let frame = sheet.popup_frame(Instant::now());
        let progress = |which: Popup| frame.filter(|(popup, _)| *popup == which).map(|(_, t)| t);
        if let Some(shown) = progress(Popup::Project)
            && let Some(at) = self.tasks_page.anchor_of("tasks-project")
        {
            let height = (self.projects.len() as f32 * 40. + 10.).min(282.);
            let origin = popover_origin(at, gpui::size(at.size.width, px(height)), px(vh));
            let rows = self.projects.iter().enumerate().map(|(ix, project)| {
                let id = project.id.clone();
                let on = project.id == sheet.project;
                div()
                    .id(("tasks-project-row", ix))
                    .h(px(40.))
                    .px(px(10.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tasks_edit_sheet(cx, |sheet| {
                            sheet.project = id.clone();
                            sheet.set_popup(None);
                        });
                    }))
                    .child(self.project_avatar_for(ix, 24.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(project.name.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(11.5))
                                    .text_color(dim)
                                    .child(tidy_path(&project.path)),
                            ),
                    )
                    .when(on, |d| d.child(icon("check", 12.).text_color(accent)))
            });
            out.push(
                div()
                    .id("tasks-project-menu")
                    .occlude()
                    .absolute()
                    .left(origin.x)
                    .top(origin.y + px((1. - shown) * -POPUP_SLIDE))
                    .opacity(shown)
                    .w(at.size.width)
                    .max_h(px(280.))
                    .overflow_y_scroll()
                    .p(px(4.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(content(0.13))
                    .child(crate::theme::floating_surface(10.))
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(rows)
                    .into_any_element(),
            );
        }
        if let Some(shown) = progress(Popup::Model)
            && let Some(at) = self.tasks_page.anchor_of("tasks-model")
        {
            // The catalog's back row, 320 px of list, and the border.
            let origin = popover_origin(at, gpui::size(px(288.), px(360.)), px(vh));
            let mut moving = false;
            let page = self.catalog_page(ModelTarget::Task, Instant::now(), &mut moving, cx);
            if moving {
                self.send_moving.set(true);
            }
            out.push(
                div()
                    .id("tasks-model-menu")
                    .occlude()
                    .absolute()
                    .left(origin.x)
                    .top(origin.y + px((1. - shown) * -POPUP_SLIDE))
                    .opacity(shown)
                    .w(px(288.))
                    .rounded(px(crate::theme::RADIUS_LG))
                    .border_1()
                    .border_color(crate::theme::border())
                    .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                    .shadow_lg()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(page)
                    .into_any_element(),
            );
        }
        if let Some(shown) = progress(Popup::Date)
            && let Some(at) = self.tasks_page.anchor_of("tasks-date")
        {
            let grid = month_grid(sheet.month, monday_first());
            // Padding and border 26, month row 30, weekday row 20, then 33 a week.
            let height = 76. + (grid.len() / 7) as f32 * 33.;
            let origin = popover_origin(at, gpui::size(px(272.), px(height)), px(vh));
            let heads = weekday_order(monday_first()).into_iter().map(|day| {
                div()
                    .flex_1()
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.5))
                    .text_color(dim)
                    .child(weekday_letter(day))
            });
            let weeks = grid
                .chunks(7)
                .map(|week| {
                    div().flex().mt(px(1.)).children(week.iter().map(|day| {
                        let day = *day;
                        let past = day < today;
                        let is_today = day == today;
                        let chosen = day == sheet.date;
                        let other = day.month() != sheet.month.month();
                        div().flex_1().flex().justify_center().child(
                            div()
                                .id(SharedString::from(format!("tasks-cal-{day}")))
                                .size(px(32.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.5))
                                .when(is_today, |d| d.bg(accent).font_weight(FontWeight::BOLD))
                                .when(chosen && !is_today, |d| d.border_1().border_color(accent))
                                .text_color(if is_today {
                                    crate::theme::on_accent()
                                } else if past {
                                    content(0.2)
                                } else if other {
                                    content(0.28)
                                } else {
                                    content(0.82)
                                })
                                .when(!past, |d| {
                                    d.cursor_pointer()
                                        .when(!is_today, |d| d.hover(|s| s.bg(content(0.09))))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.tasks_edit_sheet(cx, |sheet| {
                                                sheet.set_date(day);
                                                sheet.set_popup(None);
                                            });
                                        }))
                                })
                                .child(day.day().to_string()),
                        )
                    }))
                })
                .collect::<Vec<_>>();
            let step = |id: &'static str, glyph: &'static str, by: i32, cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .size(px(22.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(16.))
                    .text_color(content(0.75))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.09)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tasks_edit_sheet(cx, |sheet| {
                            sheet.month = add_months(sheet.month, by);
                        });
                    }))
                    .child(glyph)
            };
            out.push(
                div()
                    .id("tasks-calendar-popup")
                    .occlude()
                    .absolute()
                    .left(origin.x)
                    .top(origin.y + px((1. - shown) * -POPUP_SLIDE))
                    .opacity(shown)
                    .w(px(272.))
                    .p(px(12.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(content(0.13))
                    .child(crate::theme::floating_surface(10.))
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .mb(px(8.))
                            .pl(px(4.))
                            .child(
                                div()
                                    .flex_1()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(month_title(sheet.month)),
                            )
                            .child(step("tasks-cal-prev", "‹", -1, cx))
                            .child(step("tasks-cal-next", "›", 1, cx)),
                    )
                    .child(div().flex().children(heads))
                    .children(weeks)
                    .into_any_element(),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        parse_at(text).unwrap()
    }

    fn field(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(gpui::point(px(x), px(y)), gpui::size(px(w), px(h)))
    }

    fn origin(field: Bounds<Pixels>, height: f32, window: f32) -> (f32, f32) {
        let at = popover_origin(field, gpui::size(px(272.), px(height)), px(window));
        (f32::from(at.x), f32::from(at.y))
    }

    #[test]
    fn a_popover_opens_below_its_field_six_px_away_and_flush_left() {
        let at = origin(field(100., 200., 248., 36.), 274., 800.);
        assert_eq!(at, (100., 242.));
    }

    #[test]
    fn a_popover_flips_above_only_when_there_is_no_room_below() {
        // 560 + 36 + 6 + 274 = 876 > 788: above, its bottom 6 px over the field.
        let at = origin(field(100., 560., 248., 36.), 274., 800.);
        assert_eq!(at, (100., 280.));
        // Exactly the room that is left (window height minus the 12 px margin) still fits below.
        let at = origin(field(100., 400., 248., 36.), 330., 794.);
        assert_eq!(at.1, 442.);
    }

    #[test]
    fn a_popover_with_no_room_either_way_stays_inside_the_window() {
        let tall = origin(field(10., 150., 200., 36.), 400., 420.);
        // More room below than above: below, pushed up to the margin.
        assert_eq!(tall.1, 12.);
        let low = origin(field(10., 300., 200., 36.), 400., 420.);
        assert_eq!(low.1, 12.);
    }

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn time(hour: u32, minute: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
    }

    fn task(rule: Rule, start: Option<NaiveDate>, next: Option<NaiveDateTime>) -> Task {
        Task {
            id: "a".into(),
            title: "Morning".into(),
            prompt: "Do it".into(),
            project_id: "p1".into(),
            provider: "claude".into(),
            model: Some("opus".into()),
            effort: Some("high".into()),
            approval: Some("auto".into()),
            fast: true,
            worktree: true,
            rule,
            start,
            enabled: false,
            last_session: None,
            last_run: None,
            next,
        }
    }

    /// A form for 2026-11-10 at 9:00 with a prompt, as a new task.
    fn sheet() -> Sheet {
        let mut sheet = Sheet::blank(at("2026-11-10T09:00:00"), "p".into(), "claude");
        sheet.prompt = "Summarise".into();
        sheet
    }

    #[test]
    fn rules_read_in_words_in_both_languages() {
        crate::i18n::with_locale(false, || {
            assert_eq!(
                rule_words(&Rule::Weekdays(time(9, 0))),
                "Dias úteis às 9:00"
            );
            assert_eq!(rule_words(&Rule::Daily(time(18, 5))), "Todo dia às 18:05");
            assert_eq!(
                rule_words(&Rule::Once(at("2026-11-12T14:30:00"))),
                "Uma vez, 12 nov às 14:30"
            );
            assert_eq!(
                rule_words(&Rule::Weekly(time(9, 0), vec![0])),
                "Toda segunda às 9:00"
            );
            assert_eq!(
                rule_words(&Rule::Weekly(time(9, 0), vec![0, 2, 4])),
                "Seg, Qua e Sex às 9:00"
            );
            assert_eq!(
                rule_words(&Rule::Weekly(time(9, 0), vec![1, 5])),
                "Ter e Sáb às 9:00"
            );
            // Every day, or the five weekdays, read as the simpler rule.
            assert_eq!(
                rule_words(&Rule::Weekly(time(9, 0), vec![0, 1, 2, 3, 4, 5, 6])),
                "Todo dia às 9:00"
            );
            assert_eq!(
                rule_words(&Rule::Weekly(time(9, 0), vec![0, 1, 2, 3, 4])),
                "Dias úteis às 9:00"
            );
            let custom = |every, unit| Rule::Custom {
                time: time(14, 0),
                every,
                unit,
                start: day("2026-11-02"),
            };
            assert_eq!(
                rule_words(&custom(2, Unit::Weeks)),
                "A cada 2 semanas, segunda às 14:00"
            );
            assert_eq!(
                rule_words(&custom(1, Unit::Weeks)),
                "A cada semana, segunda às 14:00"
            );
            assert_eq!(rule_words(&custom(3, Unit::Days)), "A cada 3 dias às 14:00");
            assert_eq!(rule_short(&custom(1, Unit::Days)), "A cada dia · 14:00");
        });
        crate::i18n::with_locale(true, || {
            assert_eq!(rule_words(&Rule::Weekdays(time(9, 0))), "Weekdays at 9:00");
            assert_eq!(rule_words(&Rule::Daily(time(9, 0))), "Every day at 9:00");
            assert_eq!(
                rule_words(&Rule::Weekly(time(9, 0), vec![6])),
                "Every Sunday at 9:00"
            );
            assert_eq!(
                rule_words(&Rule::Weekly(time(9, 0), vec![0, 2, 4])),
                "Mon, Wed and Fri at 9:00"
            );
            assert_eq!(
                rule_words(&Rule::Once(at("2026-11-12T14:30:00"))),
                "Once, 12 Nov at 14:30"
            );
            assert_eq!(
                rule_words(&Rule::Custom {
                    time: time(9, 0),
                    every: 2,
                    unit: Unit::Weeks,
                    start: day("2026-11-02"),
                }),
                "Every 2 weeks, Monday at 9:00"
            );
            assert_eq!(rule_short(&Rule::Weekdays(time(9, 0))), "Weekdays · 9:00");
        });
    }

    #[test]
    fn the_host_answer_parses_into_tasks_and_runs() {
        let value = json!({
            "now": "2026-11-09T12:00:00",
            "tasks": [
                { "id": "a", "title": "Morning", "prompt": "p", "projectId": "p1", "provider": "claude",
                  "model": "opus", "enabled": false, "nextRun": "2026-11-10T09:00:00",
                  "fast": true, "worktree": true,
                  "lastRun": { "at": "2026-11-09T09:00:00", "ok": true, "sessionId": "s1" },
                  "schedule": { "repeat": { "kind": "weekly", "time": "09:00", "weekdays": [4, 0, 0, 9],
                                            "start": "2026-11-01" } } },
                { "id": "b", "title": "Once", "prompt": "p", "projectId": "p1", "provider": "codex",
                  "schedule": { "once": "2026-11-12T14:30" } },
                { "id": "c", "title": "Custom", "prompt": "p", "projectId": "p1", "provider": "codex",
                  "schedule": { "repeat": { "kind": "custom", "time": "14:00", "every": 2,
                                            "unit": "weeks", "start": "2026-11-02" } } },
                { "id": "broken", "schedule": { "repeat": { "kind": "yearly", "time": "09:00" } } },
                { "id": "no-step", "schedule": { "repeat": { "kind": "custom", "time": "09:00",
                                                              "unit": "days", "start": "2026-11-02" } } }
            ],
            "occurrences": [
                { "taskId": "a", "at": "2026-11-09T09:00:00", "state": "done", "sessionId": "s1" },
                { "taskId": "a", "at": "2026-11-10T09:00:00", "state": "planned" },
                { "taskId": "a", "at": "bad", "state": "failed" }
            ]
        });
        let (tasks, occurrences, now) = parse_list(&value);
        assert_eq!(now, Some(at("2026-11-09T12:00:00")));
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].rule, Rule::Weekly(time(9, 0), vec![0, 4]));
        assert_eq!(tasks[0].start, Some(day("2026-11-01")));
        assert!(tasks[0].fast && tasks[0].worktree);
        assert!(!tasks[0].enabled && tasks[1].enabled);
        assert!(!tasks[1].fast && !tasks[1].worktree);
        assert_eq!(tasks[0].last_session.as_deref(), Some("s1"));
        assert_eq!(tasks[0].last_run, Some((at("2026-11-09T09:00:00"), true)));
        assert_eq!(tasks[1].rule, Rule::Once(at("2026-11-12T14:30:00")));
        assert_eq!(
            tasks[2].rule,
            Rule::Custom {
                time: time(14, 0),
                every: 2,
                unit: Unit::Weeks,
                start: day("2026-11-02"),
            }
        );
        assert_eq!(occurrences.len(), 2);
        assert_eq!(occurrences[0].run, Run::Done);
        assert_eq!(occurrences[0].session.as_deref(), Some("s1"));
        // One task is paused, one never runs again: only a paused-or-finished list counts none.
        assert_eq!(active_count(&tasks), 0);
    }

    #[test]
    fn a_rule_goes_back_to_the_host_in_its_own_shape() {
        for rule in [
            Rule::Once(at("2026-11-12T14:30:00")),
            Rule::Daily(time(9, 0)),
            Rule::Weekdays(time(18, 30)),
            Rule::Weekly(time(7, 5), vec![1, 3]),
            Rule::Monthly(time(9, 0), 31),
            Rule::Custom {
                time: time(14, 0),
                every: 3,
                unit: Unit::Days,
                start: day("2026-11-02"),
            },
        ] {
            assert_eq!(parse_rule(&rule_json(&rule, None)), Some(rule));
        }
        // A repeat's first day goes along; a one-off has none.
        let weekly = rule_json(&Rule::Weekly(time(9, 0), vec![2]), Some(day("2026-11-10")));
        assert_eq!(weekly["repeat"]["start"], "2026-11-10");
        assert_eq!(parse_start(&weekly), Some(day("2026-11-10")));
        let once = rule_json(
            &Rule::Once(at("2026-11-12T14:30:00")),
            Some(day("2026-11-10")),
        );
        assert_eq!(once, json!({ "once": "2026-11-12T14:30" }));
        let custom = rule_json(
            &Rule::Custom {
                time: time(14, 0),
                every: 2,
                unit: Unit::Weeks,
                start: day("2026-11-02"),
            },
            None,
        );
        assert_eq!(custom["repeat"]["unit"], "weeks");
        assert_eq!(custom["repeat"]["every"], 2);
        assert_eq!(custom["repeat"]["start"], "2026-11-02");
    }

    #[test]
    fn views_show_one_day_a_week_or_a_months_weeks() {
        // 2026-11-11 is a Wednesday.
        let wed = day("2026-11-11");
        assert_eq!(view_days(View::Day, wed, true), vec![wed]);
        let week = view_days(View::Week, wed, true);
        assert_eq!(week.len(), 7);
        assert_eq!(week[0], day("2026-11-09"));
        assert_eq!(week[6], day("2026-11-15"));
        // Sunday first: the week of the 11th starts on the 8th.
        let sunday_week = view_days(View::Week, wed, false);
        assert_eq!(sunday_week[0], day("2026-11-08"));
        assert_eq!(sunday_week[6], day("2026-11-14"));
        // November 2026 starts on a Sunday: Monday first needs the 26th of October, six weeks.
        let month = view_days(View::Month, wed, true);
        assert_eq!(month.len(), 42);
        assert_eq!(month[0], day("2026-10-26"));
        assert_eq!(month[41], day("2026-12-06"));
        // Sunday first: the 1st opens the grid and five weeks reach December 5.
        let month = view_days(View::Month, wed, false);
        assert_eq!(month.len(), 35);
        assert_eq!(month[0], day("2026-11-01"));
        assert_eq!(month[34], day("2026-12-05"));
    }

    #[test]
    fn a_month_grid_is_whole_weeks_holding_every_day() {
        // February 2027 starts on a Monday and has 28 days: four Monday-first weeks.
        let grid = month_grid(day("2027-02-14"), true);
        assert_eq!(grid.len(), 28);
        assert_eq!(grid[0], day("2027-02-01"));
        assert_eq!(grid[27], day("2027-02-28"));
        // Sunday first it needs a fifth week on each side of the month.
        let grid = month_grid(day("2027-02-14"), false);
        assert_eq!(grid.len(), 35);
        assert_eq!(grid[0], day("2027-01-31"));
        for month in 1..=12 {
            for monday in [true, false] {
                let grid = month_grid(NaiveDate::from_ymd_opt(2026, month, 5).unwrap(), monday);
                assert!(matches!(grid.len(), 28 | 35 | 42));
                assert_eq!(
                    grid[0].weekday().num_days_from_monday(),
                    if monday { 0 } else { 6 }
                );
                assert!(
                    grid.windows(2)
                        .all(|pair| pair[1] - pair[0] == Span::days(1))
                );
            }
        }
    }

    #[test]
    fn the_weekday_headings_follow_the_first_day() {
        assert_eq!(weekday_order(true), [0, 1, 2, 3, 4, 5, 6]);
        // Dom, Seg, ..., Sáb.
        assert_eq!(weekday_order(false), [6, 0, 1, 2, 3, 4, 5]);
        assert_eq!(WEEKDAYS[weekday_order(false)[0]], "Dom");
        assert_eq!(WEEKDAYS[weekday_order(false)[6]], "Sáb");
        crate::i18n::with_locale(false, || {
            assert_eq!(weekday_letter(0), "S");
            assert_eq!(weekday_letter(6), "D");
        });
        crate::i18n::with_locale(true, || assert_eq!(weekday_letter(2), "W"));
    }

    #[test]
    fn the_period_label_names_the_month_the_week_or_the_day() {
        let today = day("2026-11-09");
        crate::i18n::with_locale(false, || {
            assert_eq!(month_title(today), "Novembro de 2026");
            assert_eq!(
                period_label(View::Month, today, today, false),
                "Novembro de 2026"
            );
            assert_eq!(
                period_label(View::Day, today, today, false),
                "segunda, 9 de nov"
            );
            assert_eq!(
                period_label(View::Day, day("2027-11-09"), today, false),
                "terça, 9 de nov de 2027"
            );
            // Sunday first: the 8th to the 14th.
            assert_eq!(
                period_label(View::Week, day("2026-11-11"), today, false),
                "8 – 14 de nov de 2026"
            );
            assert_eq!(
                period_label(View::Week, day("2026-11-30"), today, false),
                "29 de nov – 5 de dez de 2026"
            );
            assert_eq!(
                period_label(View::Week, day("2026-12-31"), today, false),
                "27 de dez de 2026 – 2 de jan de 2027"
            );
        });
        crate::i18n::with_locale(true, || {
            assert_eq!(month_title(today), "November 2026");
            assert_eq!(period_label(View::Day, today, today, true), "Monday, Nov 9");
            assert_eq!(
                period_label(View::Day, day("2027-11-09"), today, true),
                "Tuesday, Nov 9, 2027"
            );
            assert_eq!(
                period_label(View::Week, day("2026-11-11"), today, true),
                "Nov 9 – 15, 2026"
            );
            assert_eq!(
                period_label(View::Week, day("2026-11-30"), today, true),
                "Nov 30 – Dec 6, 2026"
            );
        });
    }

    #[test]
    fn a_clicked_day_or_slot_makes_the_new_tasks_time() {
        let now = at("2026-11-09T12:34:00");
        // A day ahead: 9:00. A slot of it: that time, half hours included.
        assert_eq!(
            slot_default(day("2026-11-20"), None, now),
            at("2026-11-20T09:00:00")
        );
        assert_eq!(
            slot_default(day("2026-11-20"), Some(15 * 60), now),
            at("2026-11-20T15:00:00")
        );
        assert_eq!(
            slot_default(day("2026-11-20"), Some(15 * 60 + 30), now),
            at("2026-11-20T15:30:00")
        );
        // Today after 9:00: the next hour. Late at night: not past midnight.
        assert_eq!(
            slot_default(day("2026-11-09"), None, now),
            at("2026-11-09T13:00:00")
        );
        let night = at("2026-11-09T23:40:00");
        assert_eq!(
            slot_default(day("2026-11-09"), None, night),
            at("2026-11-09T09:00:00")
        );
        // Today before 9:00 keeps 9:00.
        let early = at("2026-11-09T07:00:00");
        assert_eq!(
            slot_default(day("2026-11-09"), None, early),
            at("2026-11-09T09:00:00")
        );
        // A day that is over becomes today.
        assert_eq!(
            slot_default(day("2026-11-02"), Some(10 * 60), now),
            at("2026-11-09T13:00:00")
        );
    }

    #[test]
    fn stepping_moves_a_day_a_week_or_a_month_without_overflowing() {
        let wed = day("2026-11-11");
        assert_eq!(step_anchor(View::Day, wed, true), day("2026-11-12"));
        assert_eq!(step_anchor(View::Week, wed, false), day("2026-11-04"));
        assert_eq!(step_anchor(View::Month, wed, true), day("2026-12-11"));
        // From the 31st into a shorter month, and across years.
        assert_eq!(
            step_anchor(View::Month, day("2026-10-31"), true),
            day("2026-11-30")
        );
        assert_eq!(
            step_anchor(View::Month, day("2026-01-31"), true),
            day("2026-02-28")
        );
        assert_eq!(
            step_anchor(View::Month, day("2026-12-15"), true),
            day("2027-01-15")
        );
        assert_eq!(
            step_anchor(View::Month, day("2026-01-15"), false),
            day("2025-12-15")
        );
        assert_eq!(add_months(day("2026-11-20"), 1), day("2026-12-01"));
        assert_eq!(add_months(day("2026-01-20"), -1), day("2025-12-01"));
    }

    #[test]
    fn runs_close_together_share_the_width() {
        // Alone, and far apart: full width each.
        assert_eq!(lanes(&[540, 720]), vec![(0, 1), (0, 1)]);
        // Two at 9:00 and one at 9:15 overlap in a block of 30 minutes: three lanes.
        assert_eq!(lanes(&[540, 540, 555]), vec![(0, 3), (1, 3), (2, 3)]);
        // 9:00 and 9:30 touch but do not overlap.
        assert_eq!(lanes(&[540, 570]), vec![(0, 1), (0, 1)]);
        // A later pair is its own cluster.
        assert_eq!(
            lanes(&[540, 540, 600, 610]),
            vec![(0, 2), (1, 2), (0, 2), (1, 2)]
        );
        assert!(lanes(&[]).is_empty());
    }

    #[test]
    fn a_month_cell_fits_its_chips_and_counts_the_rest() {
        // 30 px for the day number, then 21 px a chip, never fewer than one.
        assert_eq!(month_cap(100.), 3);
        assert_eq!(month_cap(51.), 1);
        assert_eq!(month_cap(20.), 1);
        assert_eq!(month_cap(160.), 6);
        // Everything fits: no "+N mais". Otherwise the last slot is the "+N mais" line.
        assert_eq!(month_split(3, 3), (3, 0));
        assert_eq!(month_split(0, 3), (0, 0));
        assert_eq!(month_split(5, 3), (2, 3));
        assert_eq!(month_split(2, 1), (0, 2));
        assert_eq!(month_split(1, 1), (1, 0));
    }

    #[test]
    fn a_paused_tasks_coming_runs_are_not_drawn_but_its_past_ones_are() {
        assert!(drawn(Run::Planned, true));
        assert!(!drawn(Run::Planned, false));
        assert!(drawn(Run::Done, false));
        assert!(drawn(Run::Failed, false));
    }

    #[test]
    fn presets_name_moments_after_now() {
        // Monday 2026-11-09, 12:34:56.
        let now = at("2026-11-09T12:34:56");
        assert_eq!(
            preset_time(Preset::InHour, now),
            Some(at("2026-11-09T13:34:00"))
        );
        assert_eq!(
            preset_time(Preset::TodayEvening, now),
            Some(at("2026-11-09T18:00:00"))
        );
        assert_eq!(
            preset_time(Preset::TomorrowMorning, now),
            Some(at("2026-11-10T09:00:00"))
        );
        // On a Monday, "Monday" is next week's.
        assert_eq!(
            preset_time(Preset::MondayMorning, now),
            Some(at("2026-11-16T09:00:00"))
        );
        // Past 18:00 the evening preset is gone.
        let late = at("2026-11-09T19:00:00");
        assert_eq!(preset_time(Preset::TodayEvening, late), None);
        // On a Sunday the next Monday is tomorrow.
        let sunday = at("2026-11-15T10:00:00");
        assert_eq!(
            preset_time(Preset::MondayMorning, sunday),
            Some(at("2026-11-16T09:00:00"))
        );
        // The year turns over.
        let new_year = at("2026-12-31T23:30:00");
        assert_eq!(
            preset_time(Preset::TomorrowMorning, new_year),
            Some(at("2027-01-01T09:00:00"))
        );
        assert_eq!(
            preset_time(Preset::InHour, new_year),
            Some(at("2027-01-01T00:30:00"))
        );
    }

    #[test]
    fn the_time_boxes_step_hours_through_the_day_and_minutes_by_five() {
        let nine = 9 * 60;
        assert_eq!(step_clock(nine, true, true), 10 * 60);
        assert_eq!(step_clock(nine + 30, true, false), 8 * 60 + 30);
        assert_eq!(step_clock(23 * 60 + 10, true, true), 10);
        assert_eq!(step_clock(10, true, false), 23 * 60 + 10);
        assert_eq!(step_clock(nine, false, true), nine + 5);
        // Minutes carry into the hour, up and down.
        assert_eq!(step_clock(nine + 55, false, true), 10 * 60);
        assert_eq!(step_clock(nine, false, false), 8 * 60 + 55);
        assert_eq!(step_clock(0, false, false), 23 * 60 + 55);
        assert_eq!(step_clock(23 * 60 + 55, false, true), 0);
        assert_eq!(step_clock(nine + 30, false, false), nine + 25);
        // An odd minute (from "in one hour") goes to the hour before, as the preview does.
        assert_eq!(step_clock(nine + 3, false, false), 8 * 60 + 55);
        assert_eq!(clock_at(9 * 60 + 5), "9:05");
    }

    #[test]
    fn a_rule_finds_its_days() {
        let monthly = Rule::Monthly(time(10, 0), 31);
        assert!(rule_matches(&monthly, None, day("2027-02-28")));
        assert!(!rule_matches(&monthly, None, day("2027-02-27")));
        assert!(rule_matches(&monthly, None, day("2026-12-31")));
        let weekdays = Rule::Weekdays(time(9, 0));
        // 2026-11-13 is a Friday.
        assert!(rule_matches(&weekdays, None, day("2026-11-13")));
        assert!(!rule_matches(&weekdays, None, day("2026-11-14")));
        // Not before the first day.
        assert!(!rule_matches(
            &Rule::Daily(time(9, 0)),
            Some(day("2026-11-20")),
            day("2026-11-19")
        ));
        let biweekly = Rule::Custom {
            time: time(14, 0),
            every: 2,
            unit: Unit::Weeks,
            start: day("2026-11-02"),
        };
        assert!(rule_matches(&biweekly, None, day("2026-11-02")));
        assert!(!rule_matches(&biweekly, None, day("2026-11-09")));
        assert!(rule_matches(&biweekly, None, day("2026-11-16")));
        assert!(!rule_matches(&biweekly, None, day("2026-10-19")));
        assert!(rule_matches(
            &Rule::Once(at("2026-11-12T08:00:00")),
            None,
            day("2026-11-12")
        ));
    }

    #[test]
    fn the_next_runs_list_starts_after_now() {
        let now = at("2026-11-09T11:24:00");
        let list = |rule: &Rule, start| -> Vec<String> {
            upcoming(rule, start, now, 4)
                .iter()
                .map(|at| stamp(*at))
                .collect()
        };
        // 9:00 has passed today: the weekdays start tomorrow.
        assert_eq!(
            list(&Rule::Weekdays(time(9, 0)), None),
            [
                "2026-11-10T09:00:00",
                "2026-11-11T09:00:00",
                "2026-11-12T09:00:00",
                "2026-11-13T09:00:00"
            ]
        );
        assert_eq!(
            list(&Rule::Daily(time(18, 0)), Some(day("2026-11-20")))[0],
            "2026-11-20T18:00:00"
        );
        assert_eq!(
            list(&Rule::Daily(time(18, 0)), None)[0],
            "2026-11-09T18:00:00"
        );
        let biweekly = Rule::Custom {
            time: time(14, 0),
            every: 2,
            unit: Unit::Weeks,
            start: day("2026-11-02"),
        };
        assert_eq!(
            list(&biweekly, Some(day("2026-11-02"))),
            [
                "2026-11-16T14:00:00",
                "2026-11-30T14:00:00",
                "2026-12-14T14:00:00",
                "2026-12-28T14:00:00"
            ]
        );
        assert_eq!(
            list(&Rule::Monthly(time(10, 0), 31), None),
            [
                "2026-11-30T10:00:00",
                "2026-12-31T10:00:00",
                "2027-01-31T10:00:00",
                "2027-02-28T10:00:00"
            ]
        );
        assert_eq!(
            list(&Rule::Once(at("2026-11-12T08:00:00")), None),
            ["2026-11-12T08:00:00"]
        );
        assert!(list(&Rule::Once(at("2026-11-09T08:00:00")), None).is_empty());
    }

    #[test]
    fn the_form_makes_a_rule_from_when_and_repeat() {
        let mut form = sheet();
        assert_eq!(form.rule(), Some(Rule::Once(at("2026-11-10T09:00:00"))));
        assert_eq!(form.start(), None);
        form.repeat = Repeat::Weekdays;
        assert_eq!(form.rule(), Some(Rule::Weekdays(time(9, 0))));
        assert_eq!(form.start(), Some(day("2026-11-10")));
        // Weekly starts on the date's weekday, and needs one.
        form.set_repeat(Repeat::Weekly);
        assert_eq!(form.rule(), Some(Rule::Weekly(time(9, 0), vec![1])));
        form.weekdays = [false; 7];
        assert_eq!(form.rule(), None, "weekly needs a day");
        form.weekdays[2] = true;
        assert_eq!(form.rule(), Some(Rule::Weekly(time(9, 0), vec![2])));
        // Monthly takes the day it follows, then the one chosen by hand.
        form.repeat = Repeat::Monthly;
        assert_eq!(form.rule(), Some(Rule::Monthly(time(9, 0), 10)));
        form.dom = 31;
        assert_eq!(form.rule(), Some(Rule::Monthly(time(9, 0), 31)));
        // Custom counts from the date.
        form.repeat = Repeat::Custom;
        form.every = 3;
        form.unit = Unit::Days;
        assert_eq!(
            form.rule(),
            Some(Rule::Custom {
                time: time(9, 0),
                every: 3,
                unit: Unit::Days,
                start: day("2026-11-10"),
            })
        );
    }

    #[test]
    fn a_prompt_an_effort_a_weekday_and_a_future_time_are_required() {
        let now = at("2026-11-09T11:24:00");
        let mut form = sheet();
        form.prompt = "   ".into();
        let found = form.problems(true, now);
        assert!(found.prompt && found.effort && !found.weekdays && !found.past);
        assert!(found.any());
        // Nothing is chosen for the user: effort stays a problem until picked.
        form.prompt = "Do it".into();
        assert!(form.problems(true, now).effort);
        assert!(
            !form.problems(false, now).effort,
            "a model without levels needs none"
        );
        form.effort = Some("high".into());
        assert!(!form.problems(true, now).any());
        // A weekly repeat with no day.
        form.repeat = Repeat::Weekly;
        form.weekdays = [false; 7];
        assert!(form.problems(true, now).weekdays);
        form.weekdays[0] = true;
        assert!(!form.problems(true, now).any());
        // A one-off in the past, even by a minute, but not a repeat.
        form.repeat = Repeat::No;
        form.date = day("2026-11-09");
        form.minutes = 11 * 60 + 24;
        assert!(form.problems(true, now).past);
        form.minutes = 11 * 60 + 25;
        assert!(!form.problems(true, now).past);
        form.minutes = 8 * 60;
        form.repeat = Repeat::Daily;
        assert!(!form.problems(true, now).past);
    }

    #[test]
    fn a_new_form_starts_with_nothing_chosen_for_the_user() {
        let form = Sheet::blank(at("2026-11-10T09:30:00"), "p".into(), "claude");
        assert_eq!(form.effort, None);
        assert!(form.prompt.is_empty() && form.title.is_empty());
        assert_eq!((form.date, form.minutes), (day("2026-11-10"), 9 * 60 + 30));
        assert_eq!(form.repeat, Repeat::No);
        assert!(!form.worktree && !form.fast && form.approval == "ask");
        // The weekday chip of the date is already on, for when Semanal is chosen.
        assert_eq!(
            form.weekdays,
            [false, true, false, false, false, false, false]
        );
        assert_eq!(form.dom, 10);
    }

    #[test]
    fn choosing_a_date_moves_a_weekly_or_monthly_repeat_with_it() {
        let mut form = sheet();
        form.set_repeat(Repeat::Weekly);
        form.set_date(day("2026-11-12"));
        assert_eq!(
            form.weekdays,
            [false, false, false, true, false, false, false]
        );
        // Two chosen days stay as they are.
        form.weekdays[0] = true;
        form.set_date(day("2026-11-13"));
        assert_eq!(form.weekdays[0], true);
        assert_eq!(form.weekdays[4], false);
        assert_eq!(form.dom, 13);
        form.dom_edited = true;
        form.set_date(day("2026-11-20"));
        assert_eq!(form.dom, 13, "a chosen day of the month stays");
        assert_eq!(form.month, day("2026-11-01"));
        form.set_when(at("2026-12-01T18:45:00"));
        assert_eq!((form.date, form.minutes), (day("2026-12-01"), 18 * 60 + 45));
        assert_eq!(form.month, day("2026-12-01"));
        form.step_time(true, true);
        form.step_time(false, true);
        assert_eq!(form.minutes, 19 * 60 + 50);
    }

    #[test]
    fn the_footer_says_the_rule_in_words() {
        let mut form = sheet();
        crate::i18n::with_locale(false, || {
            assert_eq!(
                form.rule_text().as_deref(),
                Some("Uma vez, terça, 10 de nov às 9:00")
            );
            form.repeat = Repeat::Weekdays;
            assert_eq!(form.rule_text().as_deref(), Some("Dias úteis às 9:00"));
            form.repeat = Repeat::Weekly;
            form.weekdays = [false; 7];
            assert_eq!(form.rule_text(), None);
        });
        crate::i18n::with_locale(true, || {
            form.repeat = Repeat::No;
            assert_eq!(
                form.rule_text().as_deref(),
                Some("Once, Tuesday, Nov 10 at 9:00")
            );
        });
    }

    #[test]
    fn editing_fills_the_form_from_the_task() {
        let now = at("2026-11-09T12:00:00");
        // A weekly task keeps its days, its settings and its switch; a start behind us is today.
        let weekly = task(
            Rule::Weekly(time(9, 0), vec![1, 3]),
            Some(day("2026-10-01")),
            Some(at("2026-11-10T09:00:00")),
        );
        let form = Sheet::of_task(&weekly, now, "claude");
        assert_eq!(form.editing.as_deref(), Some("a"));
        assert_eq!(form.repeat, Repeat::Weekly);
        assert_eq!(
            form.weekdays,
            [false, true, false, true, false, false, false]
        );
        assert_eq!((form.date, form.minutes), (day("2026-11-09"), 9 * 60));
        assert_eq!(form.title, "Morning");
        assert_eq!(form.effort.as_deref(), Some("high"));
        assert_eq!(form.approval, "auto");
        assert!(form.fast && form.worktree && !form.enabled);
        assert_eq!(form.field, None);
        // A start ahead stays.
        let ahead = task(Rule::Daily(time(9, 0)), Some(day("2026-11-20")), None);
        assert_eq!(
            Sheet::of_task(&ahead, now, "claude").date,
            day("2026-11-20")
        );
        // Without one, the next run's day.
        let next = task(
            Rule::Daily(time(9, 0)),
            None,
            Some(at("2026-11-10T09:00:00")),
        );
        assert_eq!(Sheet::of_task(&next, now, "claude").date, day("2026-11-10"));
        // A one-off keeps its moment.
        let once = task(Rule::Once(at("2026-11-12T07:45:00")), None, None);
        let form = Sheet::of_task(&once, now, "claude");
        assert_eq!(
            (form.repeat, form.date, form.minutes),
            (Repeat::No, day("2026-11-12"), 7 * 60 + 45)
        );
        // Monthly keeps its day; "every N" keeps its count and its anchor, even in the past.
        let monthly = task(Rule::Monthly(time(9, 0), 31), None, None);
        let form = Sheet::of_task(&monthly, now, "claude");
        assert_eq!(
            (form.repeat, form.dom, form.dom_edited),
            (Repeat::Monthly, 31, true)
        );
        let custom = task(
            Rule::Custom {
                time: time(14, 0),
                every: 2,
                unit: Unit::Weeks,
                start: day("2026-10-05"),
            },
            None,
            None,
        );
        let form = Sheet::of_task(&custom, now, "claude");
        assert_eq!(
            (form.repeat, form.every, form.unit, form.date),
            (Repeat::Custom, 2, Unit::Weeks, day("2026-10-05"))
        );
        assert_eq!(form.rule(), Some(custom.rule.clone()));
    }

    #[test]
    fn a_model_is_described_by_its_levels_and_fast_mode() {
        crate::i18n::with_locale(false, || {
            assert_eq!(model_note(5, false), "5 níveis de esforço");
            assert_eq!(model_note(1, false), "1 nível de esforço");
            assert_eq!(model_note(4, true), "4 níveis de esforço · modo rápido");
            assert_eq!(model_note(0, true), "Modo rápido");
            assert_eq!(model_note(0, false), "");
        });
        crate::i18n::with_locale(true, || {
            assert_eq!(model_note(3, false), "3 effort levels");
        });
    }

    #[test]
    fn a_monthly_rule_reads_and_goes_back_to_the_host() {
        let host = json!({ "repeat": { "kind": "monthly", "time": "09:00", "day": 31 } });
        let rule = parse_rule(&host).unwrap();
        assert_eq!(rule, Rule::Monthly(time(9, 0), 31));
        assert_eq!(rule_json(&rule, None), host);
        assert_eq!(
            parse_rule(&json!({ "repeat": { "kind": "monthly", "time": "09:00" } })),
            None
        );
        crate::i18n::with_locale(false, || {
            assert_eq!(rule_words(&rule), "Todo mês, dia 31 às 9:00");
        });
    }
}
