//! Tarefas (ADR-200): prompts that start a conversation by themselves, once or on a repeat rule.
//! A page in its own tab, like Pull requests (shell_terminal.rs), opened from the sidebar's "Tarefas"
//! row: a calendar (Mês by default, Semana, Dia) of the runs the host lists, a popover per run, and
//! the "Nova tarefa" modal. The host
//! (`scheduled_prompts.rs`) owns the tasks and the schedule maths; this module only draws what
//! `list_tasks` answers and sends `save_task`, `delete_task`, `set_task_enabled` and
//! `run_task_now`. Times are the host's local wall-clock time, so the "now" line is the host's.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;
use crate::motion::eased;
use chrono::{Datelike, Duration as Span, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use gpui::{Animation, AnimationExt};

const HOUR_H: f32 = 48.;
const GUTTER: f32 = 56.;
/// A run is drawn as half an hour tall.
const BLOCK_MIN: i64 = 30;
const SWITCH_ID: usize = 910_000;
const NOW_LINE: u32 = 0xff4f8b;
const VIEWS: [View; 3] = [View::Day, View::Week, View::Month];
/// Runs a month's day lists before it says "+N".
const DAY_CHIPS: usize = 3;

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

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Rule {
    Once(NaiveDateTime),
    Daily(NaiveTime),
    Weekdays(NaiveTime),
    /// 0 is Monday.
    Weekly(NaiveTime, Vec<u8>),
    /// The day of the month, 1 to 31; a shorter month uses its last day.
    Monthly(NaiveTime, u8),
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
    pub rule: Rule,
    pub enabled: bool,
    pub last_session: Option<String>,
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
}

fn parse_at(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M"))
        .ok()
}

fn parse_clock(text: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(text, "%H:%M").ok()
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
        _ => None,
    }
}

/// The rule as `save_task` takes it.
pub(crate) fn rule_json(rule: &Rule) -> Value {
    let clock = |time: &NaiveTime| time.format("%H:%M").to_string();
    match rule {
        Rule::Once(at) => json!({ "once": at.format("%Y-%m-%dT%H:%M").to_string() }),
        Rule::Daily(time) => json!({ "repeat": { "kind": "daily", "time": clock(time) } }),
        Rule::Weekdays(time) => json!({ "repeat": { "kind": "weekdays", "time": clock(time) } }),
        Rule::Weekly(time, days) => {
            json!({ "repeat": { "kind": "weekly", "time": clock(time), "weekdays": days } })
        }
        Rule::Monthly(time, day) => {
            json!({ "repeat": { "kind": "monthly", "time": clock(time), "day": day } })
        }
    }
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
                rule: parse_rule(&task["schedule"])?,
                enabled: task["enabled"].as_bool().unwrap_or(true),
                last_session: task["lastRun"]["sessionId"].as_str().map(str::to_owned),
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

/// "Novembro de 2026", or "November 2026" in English.
pub(crate) fn month_title(date: NaiveDate) -> String {
    let name = MONTH_NAMES[date.month0() as usize];
    if crate::i18n::is_en() {
        format!("{} {}", t(name), date.year())
    } else {
        format!("{name} de {}", date.year())
    }
}

/// `9:00`: the hour has no leading zero.
fn clock(time: NaiveTime) -> String {
    format!("{}:{:02}", time.hour(), time.minute())
}

/// The rule in words: "Dias úteis às 9:00".
pub(crate) fn rule_words(rule: &Rule) -> String {
    match rule {
        Rule::Once(at) => tf(
            "Uma vez, {} {} às {}",
            &[
                &at.day(),
                &t(MONTHS[at.month0() as usize]),
                &clock(at.time()),
            ],
        ),
        Rule::Daily(time) => tf("Todo dia às {}", &[&clock(*time)]),
        Rule::Weekdays(time) => tf("Dias úteis às {}", &[&clock(*time)]),
        Rule::Monthly(time, day) => tf("Todo mês, dia {} às {}", &[day, &clock(*time)]),
        Rule::Weekly(time, days) => {
            let names: Vec<&str> = days
                .iter()
                .filter(|day| **day < 7)
                .map(|day| {
                    if days.len() == 1 {
                        t(EVERY_WEEKDAY[*day as usize])
                    } else {
                        t(WEEKDAYS[*day as usize])
                    }
                })
                .collect();
            let list = match names.split_last() {
                None => String::new(),
                Some((only, [])) => (*only).to_owned(),
                Some((last, head)) => format!("{} {} {last}", head.join(", "), t("e")),
            };
            tf("{} às {}", &[&list, &clock(*time)])
        }
    }
}

fn lowered(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
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

/// The anchor one step away: a day, a week, or a month.
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

fn parse_date(text: &str) -> Option<NaiveDate> {
    let text = text.trim();
    NaiveDate::parse_from_str(text, "%d/%m/%Y")
        .or_else(|_| NaiveDate::parse_from_str(text, "%Y-%m-%d"))
        .or_else(|_| {
            // Two-digit years: 12/11/26.
            NaiveDate::parse_from_str(text, "%d/%m/%y")
        })
        .ok()
}

fn parse_hour(text: &str) -> Option<NaiveTime> {
    let text = text.trim();
    NaiveTime::parse_from_str(text, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(&format!("{text}:00"), "%H:%M"))
        .ok()
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

/// The time a new task gets when a day (or an hour of it) is clicked: that hour when one was
/// clicked, else 9:00, or the next hour when today's 9:00 has passed.
pub(crate) fn slot_default(
    date: NaiveDate,
    hour: Option<u32>,
    now: NaiveDateTime,
) -> NaiveDateTime {
    let at = |hour: u32| date.and_hms_opt(hour.min(23), 0, 0).unwrap_or(now);
    if let Some(hour) = hour {
        return at(hour);
    }
    let morning = at(9);
    if date == now.date() && morning <= now {
        let next = now.hour() + 1;
        return if next < 24 { at(next) } else { morning };
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
}

const REPEATS: [(Repeat, &str); 5] = [
    (Repeat::No, "Não"),
    (Repeat::Daily, "Todo dia"),
    (Repeat::Weekdays, "Dias úteis"),
    (Repeat::Weekly, "Semanal"),
    (Repeat::Monthly, "Mensal"),
];

/// What the form shows for a task's rule: date, time, repeat and weekday chips. A repeating rule's
/// date is its next run's (or tomorrow).
pub(crate) fn form_parts(
    rule: &Rule,
    next: Option<NaiveDateTime>,
    now: NaiveDateTime,
) -> (NaiveDate, NaiveTime, Repeat, [bool; 7]) {
    let mut weekdays = [false; 7];
    let near = next.map_or(now.date() + Span::days(1), |at| at.date());
    match rule {
        Rule::Once(at) => (at.date(), at.time(), Repeat::No, weekdays),
        Rule::Daily(time) => (near, *time, Repeat::Daily, weekdays),
        Rule::Weekdays(time) => (near, *time, Repeat::Weekdays, weekdays),
        Rule::Weekly(time, days) => {
            for day in days.iter().filter(|day| **day < 7) {
                weekdays[*day as usize] = true;
            }
            (near, *time, Repeat::Weekly, weekdays)
        }
        Rule::Monthly(time, _) => (near, *time, Repeat::Monthly, weekdays),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Field {
    Prompt,
    Date,
    Time,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pick {
    Project,
    Model,
}

pub(super) struct Sheet {
    /// The task being edited; `None` for a new one.
    editing: Option<String>,
    enabled: bool,
    prompt: String,
    project: String,
    provider: &'static str,
    model: Option<String>,
    effort: Option<String>,
    /// "ask", "auto" or "full".
    approval: &'static str,
    date: String,
    time: String,
    repeat: Repeat,
    weekdays: [bool; 7],
    field: Option<Field>,
    pick: Option<Pick>,
    error: Option<String>,
    saving: bool,
}

impl Sheet {
    /// The moment the date and time fields name.
    pub(crate) fn when(&self) -> Option<NaiveDateTime> {
        Some(parse_date(&self.date)?.and_time(parse_hour(&self.time)?))
    }

    /// The rule the form makes, once its date and weekdays are right.
    pub(crate) fn rule(&self) -> Option<Rule> {
        let when = self.when()?;
        let time = when.time().with_second(0)?;
        match self.repeat {
            Repeat::No => Some(Rule::Once(when.with_second(0)?)),
            Repeat::Daily => Some(Rule::Daily(time)),
            Repeat::Weekdays => Some(Rule::Weekdays(time)),
            Repeat::Weekly => {
                let days: Vec<u8> = (0..7u8)
                    .filter(|day| self.weekdays[*day as usize])
                    .collect();
                (!days.is_empty()).then_some(Rule::Weekly(time, days))
            }
            Repeat::Monthly => Some(Rule::Monthly(time, when.day() as u8)),
        }
    }

    /// Sets the date and time fields to a moment (what a preset or a clicked slot names).
    fn set_when(&mut self, at: NaiveDateTime) {
        self.date = at.format("%d/%m/%Y").to_string();
        self.time = clock(at.time());
    }

    /// Moves the date by days, or the time by minutes, within the day.
    fn nudge(&mut self, field: Field, by: i64) {
        let Some(at) = self.when() else {
            return;
        };
        let moved = match field {
            Field::Date => at + Span::days(by),
            _ => {
                let minutes =
                    (i64::from(at.hour()) * 60 + i64::from(at.minute()) + by).clamp(0, 24 * 60 - 1);
                at.date()
                    .and_hms_opt((minutes / 60) as u32, (minutes % 60) as u32, 0)
                    .unwrap_or(at)
            }
        };
        self.set_when(moved);
    }

    /// "Roda dias úteis às 9:00, em Sirus, com Claude Code."
    pub(crate) fn summary(&self, project: &str, model: &str) -> String {
        match self.rule() {
            Some(rule) => tf(
                "Roda {}, em {}, com {}.",
                &[&lowered(&rule_words(&rule)), &project, &model],
            ),
            None => t("Escolha uma data e hora válidas.").to_owned(),
        }
    }

    fn text(&mut self, field: Field) -> &mut String {
        match field {
            Field::Prompt => &mut self.prompt,
            Field::Date => &mut self.date,
            Field::Time => &mut self.time,
        }
    }
}

// ---------------------------------------------------------------- state

pub(super) struct Popover {
    task: String,
    at: gpui::Point<gpui::Pixels>,
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
    /// Bumped by each request so a late answer to an older range is dropped.
    generation: u64,
    /// The grid scrolls to the morning (or to now) once after it opens or changes view.
    scrolled: Cell<bool>,
    /// Bumped when the view or the range changes, so the body fades in again.
    shown: u64,
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
            generation: 0,
            scrolled: Cell::new(false),
            shown: 0,
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

    /// The text the modal's focused field edits; keys go to `fallback` when none has focus.
    pub(super) fn text_target<'a>(&'a mut self, fallback: &'a mut String) -> &'a mut String {
        match self
            .sheet
            .as_mut()
            .and_then(|sheet| sheet.field.map(|field| (sheet, field)))
        {
            Some((sheet, field)) => sheet.text(field),
            None => fallback,
        }
    }
}

fn stamp(at: NaiveDateTime) -> String {
    at.format("%Y-%m-%dT%H:%M:%S").to_string()
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
            if sheet.pick.take().is_none() {
                page.sheet = None;
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

    fn tasks_delete(&mut self, id: &str, cx: &mut Context<Self>) {
        self.tasks_page.popover = None;
        self.tasks_command("delete_task", json!({ "id": id }), cx);
        cx.notify();
    }

    fn tasks_run_now(&mut self, id: &str, cx: &mut Context<Self>) {
        self.tasks_page.popover = None;
        self.tasks_command("run_task_now", json!({ "id": id }), cx);
        cx.notify();
    }

    /// Opens the conversation of the task's last run, when it still exists.
    fn tasks_open_last(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(session) = self
            .tasks_page
            .task(id)
            .and_then(|task| task.last_session.clone())
        else {
            return;
        };
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

    // ------------------------------------------------------------ the modal

    fn tasks_now(&self) -> NaiveDateTime {
        self.tasks_page
            .now
            .unwrap_or_else(|| Local::now().naive_local())
    }

    /// Opens the modal for a new task at `at` (tomorrow at 9:00 when none).
    fn tasks_new(&mut self, at: Option<NaiveDateTime>, cx: &mut Context<Self>) {
        let now = self.tasks_now();
        let at = at.unwrap_or_else(|| {
            preset_time(Preset::TomorrowMorning, now).unwrap_or(now + Span::days(1))
        });
        let provider = self
            .enabled_providers()
            .first()
            .copied()
            .unwrap_or("claude");
        let project = self.sidebar_project_id().unwrap_or_default();
        let mut weekdays = [false; 7];
        weekdays[at.date().weekday().num_days_from_monday() as usize] = true;
        let mut sheet = Sheet {
            editing: None,
            enabled: true,
            prompt: String::new(),
            project,
            provider,
            model: None,
            effort: None,
            approval: "ask",
            date: String::new(),
            time: String::new(),
            repeat: Repeat::No,
            weekdays,
            field: Some(Field::Prompt),
            pick: None,
            error: None,
            saving: false,
        };
        sheet.set_when(at);
        self.tasks_page.popover = None;
        self.tasks_page.sheet = Some(sheet);
        self.field_caret_moved.set(Instant::now());
        cx.notify();
    }

    /// Opens the modal on a task, to edit it: saving sends the same `save_task`, with its id.
    fn tasks_edit(&mut self, id: &str, cx: &mut Context<Self>) {
        let now = self.tasks_now();
        let Some(task) = self.tasks_page.task(id).cloned() else {
            return;
        };
        let (date, time, repeat, weekdays) = form_parts(&task.rule, task.next, now);
        let provider = PROVIDER_ORDER
            .iter()
            .copied()
            .find(|provider| *provider == task.provider)
            .unwrap_or("claude");
        let approval = ["ask", "auto", "full"]
            .into_iter()
            .find(|mode| Some(*mode) == task.approval.as_deref())
            .unwrap_or("ask");
        let mut sheet = Sheet {
            editing: Some(task.id.clone()),
            enabled: task.enabled,
            prompt: task.prompt.clone(),
            project: task.project_id.clone(),
            provider,
            model: task.model.clone(),
            effort: task.effort.clone(),
            approval,
            date: String::new(),
            time: String::new(),
            repeat,
            weekdays,
            field: None,
            pick: None,
            error: None,
            saving: false,
        };
        sheet.set_when(date.and_time(time));
        self.tasks_page.popover = None;
        self.tasks_page.sheet = Some(sheet);
        cx.notify();
    }

    fn tasks_save(&mut self, cx: &mut Context<Self>) {
        let now = self.tasks_now();
        let Some(sheet) = self.tasks_page.sheet.as_mut() else {
            return;
        };
        if sheet.saving {
            return;
        }
        if sheet.prompt.trim().is_empty() {
            sheet.error = Some(t("Escreva o prompt da tarefa.").to_owned());
            cx.notify();
            return;
        }
        let Some(rule) = sheet.rule() else {
            sheet.error = Some(
                if sheet.repeat == Repeat::Weekly && sheet.when().is_some() {
                    t("Escolha ao menos um dia da semana.")
                } else {
                    t("Escolha uma data e hora válidas.")
                }
                .to_owned(),
            );
            cx.notify();
            return;
        };
        if let Rule::Once(at) = &rule
            && *at <= now
        {
            sheet.error = Some(t("Essa data e hora já passaram.").to_owned());
            cx.notify();
            return;
        }
        if sheet.project.is_empty() {
            sheet.error = Some(t("Escolha um projeto.").to_owned());
            cx.notify();
            return;
        }
        sheet.error = None;
        sheet.saving = true;
        let mut task = json!({
            "prompt": sheet.prompt.trim(),
            "projectId": sheet.project,
            "provider": sheet.provider,
            "model": sheet.model,
            "effort": sheet.effort,
            "approval": sheet.approval,
            "schedule": rule_json(&rule),
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
    /// M pick the view. With it: Enter, Tab, Backspace and Esc edit the focused field. Returns
    /// whether the key was taken.
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
                    "enter" | "tab" | "backspace" => {}
                    _ => return false,
                }
                return true;
            }
            return false;
        }
        let Some(sheet) = self.tasks_page.sheet.as_mut() else {
            return false;
        };
        match key {
            "enter" => {
                match sheet.field {
                    Some(Field::Prompt) => sheet.prompt.push('\n'),
                    Some(_) => sheet.field = None,
                    None => {}
                }
                true
            }
            "tab" => {
                sheet.field = match sheet.field {
                    Some(Field::Prompt) => Some(Field::Date),
                    Some(Field::Date) => Some(Field::Time),
                    Some(Field::Time) => None,
                    None => Some(Field::Prompt),
                };
                self.field_caret_moved.set(Instant::now());
                true
            }
            "backspace" => {
                if let Some(field) = sheet.field {
                    sheet.text(field).pop();
                }
                true
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------ drawing

    fn tasks_color(provider: &str) -> gpui::Hsla {
        usage_page::Prov::of(provider).color()
    }

    fn tasks_project_name(&self, id: &str) -> String {
        self.projects
            .iter()
            .find(|project| project.id == id)
            .map_or_else(|| t("Projeto removido").to_owned(), |p| p.name.clone())
    }

    /// "Claude Code · Opus 5.5": the provider and, when set, its model.
    fn tasks_model_label(&self, provider: &str, model: Option<&str>) -> String {
        let title = provider_title(provider);
        let name = model.map(|id| {
            self.models
                .get(data::provider_id(provider))
                .and_then(|list| list.iter().find(|m| m.id == id))
                .map_or_else(|| id.to_owned(), |m| m.name.clone())
        });
        match name {
            Some(name) if !name.is_empty() => format!("{title} · {name}"),
            _ => title.to_owned(),
        }
    }

    /// The page in the Tarefas tab: header, the calendar and a message when the host refused
    /// something. A change of view or range fades the calendar in again.
    pub(super) fn tasks_page_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.tasks_page;
        let calendar: AnyElement = if page.loaded && page.tasks.is_empty() {
            self.tasks_empty(cx)
        } else {
            match page.view {
                View::Month => self.tasks_month(cx),
                View::Week => {
                    self.tasks_grid(view_days(View::Week, page.anchor(), monday_first()), cx)
                }
                View::Day => self.tasks_grid(vec![page.anchor()], cx),
            }
        };
        let calendar = div()
            .id("tasks-calendar")
            .flex_1()
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
            .child(calendar)
            .into_any_element()
    }

    /// What floats over the window while the tab shows: the run's popover and the modal.
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
        self.tasks_popover(vw, vh, cx)
            .into_iter()
            .chain(self.tasks_sheet_view(vh, cx))
            .collect()
    }

    fn tasks_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.tasks_page;
        let anchor = page.anchor();
        let range = match page.view {
            View::Day => format!(
                "{} {} {}",
                anchor.day(),
                t(MONTHS[anchor.month0() as usize]),
                anchor.year()
            ),
            View::Week => {
                let first = week_start(anchor, monday_first());
                let last = first + Span::days(6);
                format!(
                    "{} {} – {} {} {}",
                    first.day(),
                    t(MONTHS[first.month0() as usize]),
                    last.day(),
                    t(MONTHS[last.month0() as usize]),
                    last.year()
                )
            }
            View::Month => month_title(anchor),
        };
        let chosen = VIEWS.iter().position(|v| *v == page.view).unwrap_or(2);
        let segments = VIEWS
            .iter()
            .enumerate()
            .map(|(ix, view)| {
                let view = *view;
                div()
                    .id(("tasks-view", ix))
                    .relative()
                    .w(px(62.))
                    .h(px(26.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .text_color(if view == page.view {
                        crate::theme::on_accent()
                    } else {
                        content(0.6)
                    })
                    .when(view != page.view, |d| {
                        d.hover(|s| s.text_color(content(0.9)))
                    })
                    .child(crate::segment_pill::probe("tasks-view", ix))
                    .on_click(cx.listener(move |this, _, _, cx| this.tasks_set_view(view, cx)))
                    .child(view.label())
            })
            .collect::<Vec<_>>();
        let step = |id: &'static str, name: &'static str, forward: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .size(px(28.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)))
                .on_click(cx.listener(move |this, _, _, cx| this.tasks_step(forward, cx)))
                .child(icon(name, 14.).text_color(content(0.6)))
        };
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(24.))
            .h(px(64.))
            .border_b_1()
            .border_color(content(0.08))
            .child(
                div()
                    .min_w(px(190.))
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(content(0.95))
                    .child(range),
            )
            .child(step("tasks-prev", "chevron-left", false, cx))
            .child(step("tasks-next", "chevron-right", true, cx))
            .child(
                div()
                    .id("tasks-today")
                    .h(px(28.))
                    .px(px(10.))
                    .ml(px(4.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .border_1()
                    .border_color(crate::theme::border())
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(content(0.75))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.06)))
                    .on_click(cx.listener(|this, _, _, cx| this.tasks_today(cx)))
                    .child(t("Hoje")),
            )
            .child(div().flex_1())
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_shrink_0()
                    .p(px(2.))
                    .mr(px(8.))
                    .rounded(px(crate::theme::RADIUS_LG))
                    .bg(content(0.06))
                    .child(crate::segment_pill::pill(
                        "tasks-view",
                        chosen,
                        crate::theme::accent(),
                        crate::theme::RADIUS_SM,
                    ))
                    .children(segments),
            )
            .child(self.tasks_new_button(cx))
            .into_any_element()
    }

    fn tasks_new_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("tasks-new")
            .h(px(30.))
            .px(px(12.))
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(crate::theme::accent())
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(crate::theme::on_accent())
            .cursor_pointer()
            .hover(|s| s.opacity(0.9))
            .active(|s| s.opacity(0.75))
            .on_click(cx.listener(|this, _, _, cx| this.tasks_new(None, cx)))
            .child(icon("plus", 13.).text_color(crate::theme::on_accent()))
            .child(t("Nova tarefa"))
    }

    fn tasks_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .child(icon("task-clock", 32.).text_color(content(0.3)))
            .child(
                div()
                    .max_w(px(360.))
                    .text_center()
                    .text_size(px(14.))
                    .text_color(content(0.6))
                    .child(t(
                        "Agende um prompt para rodar sozinho, uma vez ou em repetição.",
                    )),
            )
            .child(self.tasks_new_button(cx))
            .into_any_element()
    }

    /// A run as a small block: the provider's colour, the title, ✓ or ✗ when it has run.
    fn tasks_chip(
        &self,
        occ: &Occ,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let task = self.tasks_page.task(&occ.task)?;
        let color = Self::tasks_color(&task.provider);
        let paused = !task.enabled;
        let id = task.id.clone();
        let mark = match occ.run {
            Run::Done => Some(("check", 0x3fb97d_u32)),
            Run::Failed => Some(("close", 0xe5484d_u32)),
            Run::Planned => None,
        };
        let mut wash = color;
        wash.a = if compact { 0.0 } else { 0.2 };
        let chip = div()
            .id(SharedString::from(format!(
                "tasks-run-{}-{}",
                task.id,
                stamp(occ.at)
            )))
            .size_full()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(6.))
            .rounded(px(5.))
            .bg(wash)
            .when(!compact, |d| d.border_l_2().border_color(color))
            .overflow_hidden()
            .cursor_pointer()
            .when(paused, |d| d.opacity(0.45))
            .hover(|s| s.opacity(0.85))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                this.tasks_page.popover = Some(Popover {
                    task: id.clone(),
                    at: event.position(),
                });
                cx.notify();
            }))
            .when(compact, |d| {
                d.child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(color))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(if compact { 11. } else { 12. }))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(0.9))
                    .when(paused, |d| d.line_through())
                    .child(if task.title.is_empty() {
                        t("Sem título").to_owned()
                    } else {
                        task.title.clone()
                    }),
            )
            .when_some(mark, |d, (name, hex)| {
                d.child(icon(name, 10.).text_color(gpui::rgb(hex)))
            });
        Some(chip)
    }

    /// Week and day: hour rows, one column per day, runs placed by their time.
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
            let is_today = *day == today;
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(2.))
                .border_l_1()
                .border_color(hairline)
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(content(if is_today { 0.9 } else { 0.5 }))
                        .child(t(WEEKDAYS[day.weekday().num_days_from_monday() as usize])),
                )
                .child(
                    div()
                        .size(px(26.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .when(is_today, |d| d.bg(crate::theme::accent()))
                        .text_color(if is_today {
                            crate::theme::on_accent()
                        } else {
                            content(0.85)
                        })
                        .child(day.day().to_string()),
                )
        });
        let labels = (0..24).map(|hour| {
            div()
                .absolute()
                .top(px(hour as f32 * HOUR_H - 7.))
                .right(px(8.))
                .text_size(px(10.5))
                .text_color(content(0.4))
                .when(hour == 0, |d| d.invisible())
                .child(format!("{hour}:00"))
        });
        let mut columns = Vec::new();
        for day in &days {
            let mut occs: Vec<&Occ> = page
                .occurrences
                .iter()
                .filter(|occ| occ.at.date() == *day)
                .collect();
            occs.sort_by_key(|occ| occ.at);
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
                .when(is_today, |d| d.bg(crate::theme::accent_wash(0.045)));
            // An empty slot starts a task at its hour.
            for hour in 0..24u32 {
                let day = *day;
                column = column.child(
                    div()
                        .id(SharedString::from(format!("tasks-slot-{day}-{hour}")))
                        .absolute()
                        .top(px(hour as f32 * HOUR_H))
                        .left_0()
                        .right_0()
                        .h(px(HOUR_H))
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.035)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let now = this.tasks_now();
                            this.tasks_new(Some(slot_default(day, Some(hour), now)), cx);
                        })),
                );
            }
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
            for (occ, (lane, count)) in occs.iter().zip(placed) {
                let Some(chip) = self.tasks_chip(occ, false, cx) else {
                    continue;
                };
                let minutes = (occ.at.hour() * 60 + occ.at.minute()) as f32;
                let (width, left) = (1. / count as f32, lane as f32 / count as f32);
                column = column.child(
                    div()
                        .absolute()
                        .top(px(minutes / 60. * HOUR_H + 1.))
                        .h(px(BLOCK_MIN as f32 / 60. * HOUR_H - 2.))
                        .left(gpui::relative(left))
                        .w(gpui::relative(width))
                        .px(px(2.))
                        .child(chip),
                );
            }
            if let Some(now) = now.filter(|now| now.date() == *day) {
                let at = (now.hour() * 60 + now.minute()) as f32 / 60. * HOUR_H;
                let pink: gpui::Hsla = gpui::rgb(NOW_LINE).into();
                column = column
                    .child(
                        div()
                            .absolute()
                            .top(px(at - 1.))
                            .left_0()
                            .right_0()
                            .h(px(2.))
                            .bg(pink),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(at - 4.))
                            .left(px(-4.))
                            .size(px(8.))
                            .rounded_full()
                            .bg(pink),
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
                    .h(px(58.))
                    .flex()
                    .border_b_1()
                    .border_color(hairline)
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

    /// Month: six weeks of days, each with its runs as small chips.
    fn tasks_month(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.tasks_page;
        let anchor = page.anchor();
        let today = page.today();
        let hairline = content(0.07);
        let days = month_grid(anchor, monday_first());
        let heads = weekday_order(monday_first()).into_iter().map(|ix| {
            div()
                .flex_1()
                .flex()
                .justify_center()
                .items_center()
                .text_size(px(11.))
                .text_color(content(0.5))
                .child(t(WEEKDAYS[ix]))
        });
        let weeks = days.chunks(7).map(|week| {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .border_t_1()
                .border_color(hairline)
                .children(week.iter().map(|day| {
                    let day = *day;
                    let in_month = day.month() == anchor.month();
                    let is_today = day == today;
                    let mut occs: Vec<&Occ> = page
                        .occurrences
                        .iter()
                        .filter(|occ| occ.at.date() == day)
                        .collect();
                    occs.sort_by_key(|occ| occ.at);
                    let more = occs.len().saturating_sub(DAY_CHIPS);
                    let chips: Vec<AnyElement> = occs
                        .iter()
                        .take(DAY_CHIPS)
                        .filter_map(|occ| self.tasks_chip(occ, true, cx))
                        .map(|chip| div().h(px(18.)).child(chip).into_any_element())
                        .collect();
                    div()
                        .id(SharedString::from(format!("tasks-day-{day}")))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .p(px(5.))
                        .border_l_1()
                        .border_color(hairline)
                        .overflow_hidden()
                        .cursor_pointer()
                        .when(is_today, |d| d.bg(crate::theme::accent_wash(0.05)))
                        .hover(|s| s.bg(content(0.03)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let now = this.tasks_now();
                            this.tasks_new(Some(slot_default(day, None, now)), cx);
                        }))
                        .child(
                            div()
                                .size(px(22.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .when(is_today, |d| d.bg(crate::theme::accent()))
                                .text_color(if is_today {
                                    crate::theme::on_accent()
                                } else {
                                    content(if in_month { 0.85 } else { 0.3 })
                                })
                                .child(day.day().to_string()),
                        )
                        .children(chips)
                        .when(more > 0, |d| {
                            d.child(
                                div()
                                    .id(SharedString::from(format!("tasks-more-{day}")))
                                    .px(px(6.))
                                    .rounded(px(4.))
                                    .text_size(px(10.5))
                                    .text_color(content(0.5))
                                    .hover(|s| s.bg(content(0.06)).text_color(content(0.8)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    // The rest of the day's runs are in its own view.
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.tasks_page.anchor = Some(day);
                                        this.tasks_set_view(View::Day, cx);
                                    }))
                                    .child(format!("+{more}")),
                            )
                        })
                }))
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().flex_shrink_0().h(px(32.)).flex().children(heads))
            .children(weeks)
            .into_any_element()
    }

    /// The run's popover: what the task is, its switch, and the three things to do with it.
    fn tasks_popover(&self, vw: f32, vh: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pop = self.tasks_page.popover.as_ref()?;
        let task = self.tasks_page.task(&pop.task)?;
        let id = task.id.clone();
        const WIDTH: f32 = 300.;
        let left = f32::from(pop.at.x).min(vw - WIDTH - 12.).max(12.);
        let top = (f32::from(pop.at.y) + 8.).min(vh - 330.).max(12.);
        let color = Self::tasks_color(&task.provider);
        let line = |label: String, value: String| {
            div()
                .flex()
                .gap(px(8.))
                .text_size(px(12.))
                .child(
                    div()
                        .w(px(56.))
                        .flex_shrink_0()
                        .text_color(content(0.45))
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(content(0.85))
                        .child(value),
                )
        };
        let action = |name: &'static str, label: &'static str, enabled: bool| {
            div()
                .id(name)
                .h(px(28.))
                .px(px(8.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .text_size(px(12.5))
                .text_color(content(if enabled { 0.85 } else { 0.3 }))
                .when(enabled, |d| {
                    d.cursor_pointer().hover(|s| s.bg(content(0.07)))
                })
                .child(t(label))
        };
        let toggle_id = id.clone();
        let (edit_id, run_id, last_id, delete_id) =
            (id.clone(), id.clone(), id.clone(), id.clone());
        let has_last = task.last_session.is_some();
        Some(
            div()
                .id("tasks-popover")
                .occlude()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(WIDTH))
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_shrink_0().size(px(8.)).rounded_full().bg(color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(content(0.95))
                                .child(task.title.clone()),
                        )
                        .child(
                            div()
                                .id("tasks-toggle")
                                .flex_shrink_0()
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
                .child(line(t("Regra").to_owned(), rule_words(&task.rule)))
                .child(line(
                    t("Projeto").to_owned(),
                    self.tasks_project_name(&task.project_id),
                ))
                .child(line(
                    t("Modelo").to_owned(),
                    self.tasks_model_label(&task.provider, task.model.as_deref()),
                ))
                .child(div().h(px(1.)).bg(content(0.07)))
                .child(
                    action("tasks-edit", "Editar", true)
                        .on_click(cx.listener(move |this, _, _, cx| this.tasks_edit(&edit_id, cx))),
                )
                .child(
                    action("tasks-run-now", "Executar agora", true).on_click(
                        cx.listener(move |this, _, _, cx| this.tasks_run_now(&run_id, cx)),
                    ),
                )
                .child(
                    action("tasks-open-last", "Abrir última conversa", has_last).when(
                        has_last,
                        |d| {
                            d.on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.tasks_open_last(&last_id, cx)
                                }),
                            )
                        },
                    ),
                )
                .child(
                    action("tasks-delete", "Excluir", true).on_click(
                        cx.listener(move |this, _, _, cx| this.tasks_delete(&delete_id, cx)),
                    ),
                )
                .into_any_element(),
        )
    }

    // ------------------------------------------------------------ the sheet's view

    /// A text field drawn as a box; the focused one shows the caret.
    fn tasks_field(
        &self,
        id: &'static str,
        field: Field,
        text: &str,
        placeholder: &'static str,
        lines: bool,
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
            let rows: Vec<&str> = text.split('\n').collect();
            let last = rows.len() - 1;
            div()
                .flex()
                .flex_col()
                .children(rows.iter().enumerate().map(|(ix, row)| {
                    div()
                        .flex()
                        .min_h(px(18.))
                        .child(row.to_string())
                        .when(focused && ix == last, |d| d.child(caret()))
                }))
                .into_any_element()
        } else {
            div()
                .flex()
                .child(text.to_owned())
                .when(focused, |d| d.child(caret()))
                .into_any_element()
        };
        div()
            .id(id)
            .px(px(10.))
            .py(px(7.))
            .rounded(px(crate::theme::RADIUS_MD))
            .border_1()
            .border_color(if focused {
                crate::theme::accent()
            } else {
                crate::theme::border()
            })
            .bg(content(0.03))
            .text_size(px(13.))
            .text_color(content(0.92))
            .cursor_text()
            .when(lines, |d| d.min_h(px(92.)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                    sheet.field = Some(field);
                    sheet.pick = None;
                }
                this.field_caret_moved.set(Instant::now());
                cx.notify();
            }))
            .child(content_el)
    }

    /// A small arrow beside a date or time field: a day, or half an hour, back or forward.
    fn tasks_nudge(
        &self,
        id: SharedString,
        name: &'static str,
        field: Field,
        by: i64,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .size(px(24.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(content(0.07)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                    sheet.nudge(field, by);
                    sheet.field = None;
                }
                cx.notify();
            }))
            .child(icon(name, 12.).text_color(content(0.55)))
    }

    fn tasks_chip_button(id: SharedString, label: &str, on: bool, enabled: bool) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(26.))
            .px(px(10.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .text_size(px(12.))
            .border_1()
            .border_color(if on {
                crate::theme::accent()
            } else {
                crate::theme::border()
            })
            .bg(if on {
                crate::theme::accent_wash(0.16)
            } else {
                transparent()
            })
            .text_color(content(if !enabled {
                0.25
            } else if on {
                0.95
            } else {
                0.7
            }))
            .when(enabled, |d| {
                d.cursor_pointer().hover(|s| s.bg(content(0.06)))
            })
            .child(label.to_owned())
    }

    fn tasks_label(text: &'static str) -> Div {
        div()
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(content(0.5))
            .child(t(text))
    }

    fn tasks_sheet_view(&self, vh: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sheet = self.tasks_page.sheet.as_ref()?;
        let now = self
            .tasks_page
            .now
            .unwrap_or_else(|| Local::now().naive_local());
        let project_name = self.tasks_project_name(&sheet.project);
        let model_label = self.tasks_model_label(sheet.provider, sheet.model.as_deref());
        // Projects.
        let project_picker = {
            let open = sheet.pick == Some(Pick::Project);
            let button =
                self.tasks_picker_button("tasks-project", &project_name, open, cx, Pick::Project);
            let list = open.then(|| {
                div()
                    .id("tasks-project-list")
                    .max_h(px(150.))
                    .overflow_y_scroll()
                    .p(px(4.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .border_1()
                    .border_color(crate::theme::border())
                    .children(self.projects.iter().enumerate().map(|(ix, project)| {
                        let id = project.id.clone();
                        div()
                            .id(("tasks-project-row", ix))
                            .h(px(26.))
                            .px(px(8.))
                            .rounded(px(crate::theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .text_size(px(12.5))
                            .text_color(content(if id == sheet.project { 1. } else { 0.75 }))
                            .when(id == sheet.project, |d| d.bg(content(0.08)))
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.07)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                                    sheet.project = id.clone();
                                    sheet.pick = None;
                                }
                                cx.notify();
                            }))
                            .child(project.name.clone())
                    }))
            });
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(button)
                .children(list)
        };
        // Provider and model.
        let providers = self.enabled_providers();
        let provider_chips = providers
            .iter()
            .map(|provider| {
                let provider = *provider;
                let on = sheet.provider == provider;
                Self::tasks_chip_button(
                    SharedString::from(format!("tasks-provider-{provider}")),
                    provider_title(provider),
                    on,
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(sheet) = this.tasks_page.sheet.as_mut()
                        && sheet.provider != provider
                    {
                        sheet.provider = provider;
                        sheet.model = None;
                        sheet.effort = None;
                        sheet.pick = None;
                    }
                    cx.notify();
                }))
            })
            .collect::<Vec<_>>();
        let model_picker = {
            let open = sheet.pick == Some(Pick::Model);
            let shown = match sheet.model.as_deref() {
                None => t("Padrão do provedor").to_owned(),
                Some(id) => self
                    .models
                    .get(sheet.provider)
                    .and_then(|list| list.iter().find(|m| m.id == id))
                    .map_or_else(|| id.to_owned(), |m| m.name.clone()),
            };
            let button = self.tasks_picker_button("tasks-model", &shown, open, cx, Pick::Model);
            let list = open.then(|| {
                let rows = self.provider_model_rows(
                    sheet.provider,
                    (sheet.provider, sheet.model.as_deref()),
                    "",
                    &self.favorite_models(),
                );
                let entries: Vec<(Option<String>, String)> =
                    std::iter::once((None, t("Padrão do provedor").to_owned()))
                        .chain(rows.into_iter().map(|row| (Some(row.id), row.name)))
                        .collect();
                div()
                    .id("tasks-model-list")
                    .max_h(px(170.))
                    .overflow_y_scroll()
                    .p(px(4.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .border_1()
                    .border_color(crate::theme::border())
                    .children(entries.into_iter().enumerate().map(|(ix, (id, name))| {
                        let on = id == sheet.model;
                        div()
                            .id(("tasks-model-row", ix))
                            .h(px(26.))
                            .px(px(8.))
                            .rounded(px(crate::theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .text_size(px(12.5))
                            .text_color(content(if on { 1. } else { 0.75 }))
                            .when(on, |d| d.bg(content(0.08)))
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.07)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                                    sheet.model = id.clone();
                                    sheet.effort = None;
                                    sheet.pick = None;
                                }
                                cx.notify();
                            }))
                            .child(name)
                    }))
            });
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(button)
                .children(list)
        };
        // When: presets fill the date and time, which stay editable.
        let at_now = sheet.when().map(|at| at.with_second(0).unwrap_or(at));
        let preset_chips = PRESETS
            .iter()
            .enumerate()
            .map(|(ix, (preset, label))| {
                let preset = *preset;
                let moment = preset_time(preset, now);
                let usable = moment.is_some();
                let chip = Self::tasks_chip_button(
                    SharedString::from(format!("tasks-preset-{ix}")),
                    t(label),
                    usable && moment == at_now,
                    usable,
                );
                let Some(moment) = moment else {
                    return chip;
                };
                chip.on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                        sheet.set_when(moment);
                        sheet.field = None;
                        sheet.pick = None;
                        if sheet.repeat == Repeat::Weekly {
                            sheet.weekdays = [false; 7];
                            sheet.weekdays[moment.weekday().num_days_from_monday() as usize] = true;
                        }
                    }
                    cx.notify();
                }))
            })
            .collect::<Vec<_>>();
        let mut picker = |id: &'static str,
                          field: Field,
                          text: &str,
                          hint: &'static str,
                          wide: f32,
                          (back, forward): (i64, i64)| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(self.tasks_nudge(
                    SharedString::from(format!("{id}-back")),
                    "chevron-left",
                    field,
                    back,
                    cx,
                ))
                .child(
                    self.tasks_field(id, field, text, hint, false, cx)
                        .w(px(wide)),
                )
                .child(self.tasks_nudge(
                    SharedString::from(format!("{id}-forward")),
                    "chevron-right",
                    field,
                    forward,
                    cx,
                ))
        };
        let custom = div()
            .flex()
            .items_center()
            .gap(px(14.))
            .child(picker(
                "tasks-date",
                Field::Date,
                &sheet.date,
                "dd/mm/aaaa",
                112.,
                (-1, 1),
            ))
            .child(picker(
                "tasks-time",
                Field::Time,
                &sheet.time,
                "hh:mm",
                70.,
                (-30, 30),
            ));
        // Repeat.
        let repeat_chips = REPEATS
            .iter()
            .enumerate()
            .map(|(ix, (repeat, label))| {
                let repeat = *repeat;
                Self::tasks_chip_button(
                    SharedString::from(format!("tasks-repeat-{ix}")),
                    t(label),
                    sheet.repeat == repeat,
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                        sheet.repeat = repeat;
                        if repeat == Repeat::Weekly
                            && sheet.weekdays.iter().all(|on| !on)
                            && let Some(at) = sheet.when()
                        {
                            sheet.weekdays[at.weekday().num_days_from_monday() as usize] = true;
                        }
                    }
                    cx.notify();
                }))
            })
            .collect::<Vec<_>>();
        let weekday_chips = (sheet.repeat == Repeat::Weekly).then(|| {
            div().flex().gap(px(6.)).children(
                weekday_order(monday_first())
                    .into_iter()
                    .map(|day| {
                        Self::tasks_chip_button(
                            SharedString::from(format!("tasks-weekday-{day}")),
                            t(WEEKDAYS[day]),
                            sheet.weekdays[day],
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                                sheet.weekdays[day] = !sheet.weekdays[day];
                            }
                            cx.notify();
                        }))
                    })
                    .collect::<Vec<_>>(),
            )
        });
        let summary = sheet.summary(&project_name, &model_label);
        // Effort: the composer's levels for the chosen model; a model has to be chosen first.
        let levels = self
            .execution(sheet.provider, sheet.model.as_deref())
            .levels;
        let effort_chips = std::iter::once((None, t("Automático").to_owned()))
            .chain(
                levels
                    .iter()
                    .map(|level| (Some(level.clone()), effort_label(level))),
            )
            .enumerate()
            .map(|(ix, (level, label))| {
                let on = sheet.effort == level;
                Self::tasks_chip_button(
                    SharedString::from(format!("tasks-effort-{ix}")),
                    &label,
                    on,
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                        sheet.effort = level.clone();
                    }
                    cx.notify();
                }))
            })
            .collect::<Vec<_>>();
        // Access: how the run's actions are approved, as the composer's menu names them.
        let access_chips = ["ask", "auto", "full"]
            .into_iter()
            .map(|mode| {
                let (_, label, _, _) = access_look(mode);
                Self::tasks_chip_button(
                    SharedString::from(format!("tasks-access-{mode}")),
                    label,
                    sheet.approval == mode,
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                        sheet.approval = mode;
                    }
                    cx.notify();
                }))
            })
            .collect::<Vec<_>>();
        let access_note = access_look(sheet.approval).2;
        let prompt = self.tasks_field(
            "tasks-prompt",
            Field::Prompt,
            &sheet.prompt,
            "O que o agente deve fazer?",
            true,
            cx,
        );
        let form = div()
            .id("tasks-sheet-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.tasks_page.sheet_scroll)
            .px(px(20.))
            .py(px(4.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(Self::tasks_label("Prompt"))
                    .child(prompt),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(Self::tasks_label("Projeto"))
                    .child(project_picker),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(Self::tasks_label("Provedor e modelo"))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .children(provider_chips),
                    )
                    .child(model_picker),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(Self::tasks_label("Esforço"))
                    .child(div().flex().flex_wrap().gap(px(6.)).children(effort_chips))
                    .when(levels.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(content(0.4))
                                .child(t("Escolha um modelo para ajustar o esforço.")),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(Self::tasks_label("Acesso"))
                    .child(div().flex().flex_wrap().gap(px(6.)).children(access_chips))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(content(0.4))
                            .child(access_note),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(Self::tasks_label("Quando"))
                    .child(div().flex().flex_wrap().gap(px(6.)).children(preset_chips))
                    .child(custom),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(Self::tasks_label("Repetir"))
                    .child(div().flex().flex_wrap().gap(px(6.)).children(repeat_chips))
                    .children(weekday_chips),
            );
        let footer = div()
            .flex_shrink_0()
            .px(px(20.))
            .pt(px(10.))
            .pb(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .border_t_1()
            .border_color(content(0.07))
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(content(0.7))
                    .child(summary),
            )
            .when_some(sheet.error.clone(), |d, error| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(gpui::rgb(0xe5484d))
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        div()
                            .id("tasks-cancel")
                            .h(px(30.))
                            .px(px(14.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .flex()
                            .items_center()
                            .text_size(px(12.5))
                            .text_color(content(0.7))
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.07)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.tasks_page.sheet = None;
                                cx.notify();
                            }))
                            .child(t("Cancelar")),
                    )
                    .child(
                        div()
                            .id("tasks-save")
                            .h(px(30.))
                            .px(px(16.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .bg(crate::theme::accent())
                            .flex()
                            .items_center()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(crate::theme::on_accent())
                            .when(sheet.saving, |d| d.opacity(0.6))
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.9))
                            .on_click(cx.listener(|this, _, _, cx| this.tasks_save(cx)))
                            .child(t("Salvar")),
                    ),
            );
        Some(
            div()
                .id("tasks-sheet-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.3))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.tasks_page.sheet = None;
                        cx.notify();
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .id("tasks-sheet")
                        .w(px(560.))
                        .max_h(px(vh - 72.))
                        .flex()
                        .flex_col()
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(crate::theme::border())
                        .overflow_hidden()
                        .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                        .shadow_lg()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                                    sheet.field = None;
                                    sheet.pick = None;
                                }
                                cx.notify();
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .px(px(20.))
                                .pt(px(16.))
                                .pb(px(12.))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(16.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(content(0.95))
                                        .child(if sheet.editing.is_some() {
                                            t("Editar tarefa")
                                        } else {
                                            t("Nova tarefa")
                                        }),
                                )
                                .child(
                                    div()
                                        .id("tasks-sheet-close")
                                        .size(px(26.))
                                        .rounded(px(crate::theme::RADIUS_MD))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(content(0.07)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.tasks_page.sheet = None;
                                            cx.notify();
                                        }))
                                        .child(icon("close", 14.).text_color(content(0.6))),
                                ),
                        )
                        .child(form)
                        .child(footer),
                )
                .into_any_element(),
        )
    }

    /// A closed picker: its value and a chevron. A click opens or closes its list.
    fn tasks_picker_button(
        &self,
        id: &'static str,
        value: &str,
        open: bool,
        cx: &mut Context<Self>,
        pick: Pick,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(32.))
            .px(px(10.))
            .rounded(px(crate::theme::RADIUS_MD))
            .border_1()
            .border_color(if open {
                crate::theme::accent()
            } else {
                crate::theme::border()
            })
            .bg(content(0.03))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(13.))
            .text_color(content(0.92))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(sheet) = this.tasks_page.sheet.as_mut() {
                    sheet.field = None;
                    sheet.pick = if sheet.pick == Some(pick) {
                        None
                    } else {
                        Some(pick)
                    };
                }
                cx.notify();
            }))
            .child(div().flex_1().min_w_0().truncate().child(value.to_owned()))
            .child(
                icon(if open { "chevron-up" } else { "chevron-down" }, 12.)
                    .text_color(content(0.5)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        parse_at(text).unwrap()
    }

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn time(hour: u32, minute: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
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
        });
    }

    #[test]
    fn the_host_answer_parses_into_tasks_and_runs() {
        let value = json!({
            "now": "2026-11-09T12:00:00",
            "tasks": [
                { "id": "a", "title": "Morning", "prompt": "p", "projectId": "p1", "provider": "claude",
                  "model": "opus", "enabled": false, "nextRun": "2026-11-10T09:00:00",
                  "lastRun": { "at": "2026-11-09T09:00:00", "ok": true, "sessionId": "s1" },
                  "schedule": { "repeat": { "kind": "weekly", "time": "09:00", "weekdays": [4, 0, 0, 9] } } },
                { "id": "b", "title": "Once", "prompt": "p", "projectId": "p1", "provider": "codex",
                  "schedule": { "once": "2026-11-12T14:30" } },
                { "id": "broken", "schedule": { "repeat": { "kind": "yearly", "time": "09:00" } } }
            ],
            "occurrences": [
                { "taskId": "a", "at": "2026-11-09T09:00:00", "state": "done", "sessionId": "s1" },
                { "taskId": "a", "at": "2026-11-10T09:00:00", "state": "planned" },
                { "taskId": "a", "at": "bad", "state": "failed" }
            ]
        });
        let (tasks, occurrences, now) = parse_list(&value);
        assert_eq!(now, Some(at("2026-11-09T12:00:00")));
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].rule, Rule::Weekly(time(9, 0), vec![0, 4]));
        assert!(!tasks[0].enabled && tasks[1].enabled);
        assert_eq!(tasks[0].last_session.as_deref(), Some("s1"));
        assert_eq!(tasks[1].rule, Rule::Once(at("2026-11-12T14:30:00")));
        assert_eq!(occurrences.len(), 2);
        assert_eq!(occurrences[0].run, Run::Done);
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
        ] {
            assert_eq!(parse_rule(&rule_json(&rule)), Some(rule));
        }
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
    }

    #[test]
    fn the_month_title_names_month_and_year() {
        crate::i18n::with_locale(false, || {
            assert_eq!(month_title(day("2026-11-09")), "Novembro de 2026");
        });
        crate::i18n::with_locale(true, || {
            assert_eq!(month_title(day("2026-11-09")), "November 2026");
        });
    }

    #[test]
    fn a_clicked_day_or_hour_makes_the_new_tasks_time() {
        let now = at("2026-11-09T12:34:00");
        // A day ahead: 9:00. An hour of it: that hour.
        assert_eq!(
            slot_default(day("2026-11-20"), None, now),
            at("2026-11-20T09:00:00")
        );
        assert_eq!(
            slot_default(day("2026-11-20"), Some(15), now),
            at("2026-11-20T15:00:00")
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
    }

    #[test]
    fn editing_fills_the_form_from_the_rule() {
        let now = at("2026-11-09T12:00:00");
        let (date, clock_time, repeat, weekdays) =
            form_parts(&Rule::Once(at("2026-11-12T07:45:00")), None, now);
        assert_eq!(
            (date, clock_time, repeat),
            (day("2026-11-12"), time(7, 45), Repeat::No)
        );
        assert_eq!(weekdays, [false; 7]);
        let next = Some(at("2026-11-10T09:00:00"));
        let (date, _, repeat, weekdays) =
            form_parts(&Rule::Weekly(time(9, 0), vec![1, 3]), next, now);
        assert_eq!((date, repeat), (day("2026-11-10"), Repeat::Weekly));
        assert_eq!(weekdays, [false, true, false, true, false, false, false]);
        let (date, _, repeat, _) = form_parts(&Rule::Monthly(time(9, 0), 15), None, now);
        assert_eq!((date, repeat), (day("2026-11-10"), Repeat::Monthly));
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

    fn sheet() -> Sheet {
        Sheet {
            editing: None,
            enabled: true,
            prompt: "Summarise".into(),
            project: "p".into(),
            provider: "claude",
            model: None,
            effort: None,
            approval: "ask",
            date: "10/11/2026".into(),
            time: "9:00".into(),
            repeat: Repeat::No,
            weekdays: [false; 7],
            field: None,
            pick: None,
            error: None,
            saving: false,
        }
    }

    #[test]
    fn the_form_makes_a_rule_from_when_and_repeat() {
        let mut form = sheet();
        assert_eq!(form.rule(), Some(Rule::Once(at("2026-11-10T09:00:00"))));
        form.repeat = Repeat::Weekdays;
        assert_eq!(form.rule(), Some(Rule::Weekdays(time(9, 0))));
        form.repeat = Repeat::Weekly;
        assert_eq!(form.rule(), None, "weekly needs a day");
        form.weekdays[2] = true;
        assert_eq!(form.rule(), Some(Rule::Weekly(time(9, 0), vec![2])));
        // Monthly takes the date's day of the month.
        form.repeat = Repeat::Monthly;
        assert_eq!(form.rule(), Some(Rule::Monthly(time(9, 0), 10)));
    }

    #[test]
    fn a_custom_date_and_time_are_read_as_typed() {
        let mut form = sheet();
        form.date = "12/11/2026".into();
        form.time = "7:45".into();
        assert_eq!(form.when(), Some(at("2026-11-12T07:45:00")));
        form.time = "18".into();
        assert_eq!(form.when(), Some(at("2026-11-12T18:00:00")));
        form.date = "2026-12-01".into();
        assert_eq!(form.when(), Some(at("2026-12-01T18:00:00")));
        form.date = "31/02/2026".into();
        assert_eq!(form.when(), None);
        form.date = "12/11/2026".into();
        form.time = "25:00".into();
        assert_eq!(form.when(), None);
    }

    #[test]
    fn the_arrows_move_the_date_by_days_and_the_time_within_the_day() {
        let mut form = sheet();
        form.nudge(Field::Date, 1);
        assert_eq!(form.date, "11/11/2026");
        form.nudge(Field::Date, -30);
        assert_eq!(form.date, "12/10/2026");
        form.nudge(Field::Time, 30);
        assert_eq!(form.time, "9:30");
        form.time = "23:45".into();
        form.nudge(Field::Time, 30);
        assert_eq!(form.time, "23:59", "the time stays inside its day");
        form.time = "0:10".into();
        form.nudge(Field::Time, -30);
        assert_eq!(form.time, "0:00");
        form.date = "nope".into();
        form.nudge(Field::Date, 1);
        assert_eq!(form.date, "nope");
    }

    #[test]
    fn a_monthly_rule_reads_and_goes_back_to_the_host() {
        let host = json!({ "repeat": { "kind": "monthly", "time": "09:00", "day": 31 } });
        let rule = parse_rule(&host).unwrap();
        assert_eq!(rule, Rule::Monthly(time(9, 0), 31));
        assert_eq!(rule_json(&rule), host);
        assert_eq!(
            parse_rule(&json!({ "repeat": { "kind": "monthly", "time": "09:00" } })),
            None
        );
        crate::i18n::with_locale(false, || {
            assert_eq!(rule_words(&rule), "Todo mês, dia 31 às 9:00");
        });
    }

    #[test]
    fn the_summary_is_a_sentence_in_the_current_language() {
        let mut form = sheet();
        form.repeat = Repeat::Weekdays;
        crate::i18n::with_locale(false, || {
            assert_eq!(
                form.summary("Sirus", "Claude Code"),
                "Roda dias úteis às 9:00, em Sirus, com Claude Code."
            );
        });
        crate::i18n::with_locale(true, || {
            assert_eq!(
                form.summary("Sirus", "Claude Code"),
                "Runs weekdays at 9:00, in Sirus, with Claude Code."
            );
        });
        form.date = String::new();
        crate::i18n::with_locale(false, || {
            assert_eq!(
                form.summary("Sirus", "Claude"),
                "Escolha uma data e hora válidas."
            );
        });
    }
}
