//! The Usage page (UsagePage.tsx): Custo, Tokens and Limites over the whole window, opened from the
//! footer's "Uso" or the command palette. Cost and Tokens read the provider CLIs' local logs through
//! `usage_history` (90 days, cut to the range here) with a stacked cumulative area per provider;
//! Limites shows each provider's and account's quota windows, with a daily pace for the weekly
//! one. The pace needs the weekly percentage at the start of the local day, which the host does
//! not keep, so the first reading of each day is saved in `usage-day-readings.json`.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;
use crate::visuals::dither::{Align, Label, Scale, crosshair, hoverable, pointer, readout};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) const RANGES: [u32; 4] = [1, 7, 30, 90];
const HOUR: i64 = 3_600_000;
#[cfg(test)]
const DAY: i64 = 24 * HOUR;
const WEEK_MINUTES: i64 = 7 * 24 * 60;
/// Days of readings kept in the file.
const KEEP_DAYS: usize = 21;
/// The providers whose quota the host reads (`provider_usage::reports_usage`).
const LIMIT_PROVIDERS: [(&str, &str); 4] =
    [("codex", "Codex"), ("claude", "Claude"), ("cursor", "Cursor"), ("opencode", "OpenCode")];
const AMBER: u32 = 0xe9b44c;

// ---------------------------------------------------------------- providers and history

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Prov {
    Claude,
    Codex,
    OpenCode,
    Other,
}

const PROVS: [Prov; 4] = [Prov::Claude, Prov::Codex, Prov::OpenCode, Prov::Other];

impl Prov {
    pub(crate) fn of(id: &str) -> Prov {
        match id {
            "claude" => Prov::Claude,
            "codex" => Prov::Codex,
            "opencode" => Prov::OpenCode,
            _ => Prov::Other,
        }
    }
    fn index(self) -> usize {
        PROVS.iter().position(|p| *p == self).unwrap_or(3)
    }
    fn name(self) -> &'static str {
        match self {
            Prov::Claude => "Claude",
            Prov::Codex => "Codex",
            Prov::OpenCode => "OpenCode",
            Prov::Other => t("Outros"),
        }
    }
    /// Validated for dark; on a light theme the same hues a little deeper stay readable.
    fn color(self) -> gpui::Hsla {
        let base = match self {
            Prov::Claude => 0xc9774a,
            Prov::Codex => 0x1ea3b8,
            Prov::OpenCode => 0x9180ee,
            Prov::Other => return gpui::hsla(0., 0., if crate::theme::is_light() { 0.5 } else { 0.55 }, 1.),
        };
        let mut color: gpui::Hsla = rgb(base).into();
        if crate::theme::is_light() {
            color.l *= 0.8;
        }
        color
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub at: i64,
    pub provider: String,
    pub model: String,
    pub session: String,
    pub input: f64,
    pub cached: f64,
    pub cache_write: f64,
    pub output: f64,
    pub reported: Option<f64>,
    pub fast: bool,
}

impl Entry {
    fn tokens(&self) -> f64 {
        self.input + self.cached + self.cache_write + self.output
    }
}

pub(crate) fn parse_entries(value: &Value) -> Vec<Entry> {
    let number = |v: &Value, key: &str| v[key].as_f64().unwrap_or(0.);
    value["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some(Entry {
                at: e["at"].as_i64()?,
                provider: e["provider"].as_str()?.to_owned(),
                model: e["model"].as_str().unwrap_or("").to_owned(),
                session: e["session"].as_str().unwrap_or("").to_owned(),
                input: number(e, "inputTokens"),
                cached: number(e, "cachedInputTokens"),
                cache_write: number(e, "cacheWriteTokens"),
                output: number(e, "outputTokens"),
                reported: e["reportedCostUsd"].as_f64(),
                fast: e["speed"].as_str() == Some("fast"),
            })
        })
        .collect()
}

/// USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rates {
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Price {
    pub rates: Rates,
    pub fast: Option<Rates>,
}

fn rates_of(v: &Value) -> Option<Rates> {
    Some(Rates {
        input: v["input"].as_f64()?,
        cache_read: v["cacheRead"].as_f64()?,
        cache_write: v["cacheWrite"].as_f64()?,
        output: v["output"].as_f64()?,
    })
}

pub(crate) fn parse_prices(value: &Value) -> HashMap<String, Price> {
    value["prices"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, p)| Some((name.clone(), Price { rates: rates_of(p)?, fast: rates_of(&p["fast"]) })))
        .collect()
}

fn list_price(input: f64, output: f64, cache_read: Option<f64>) -> Rates {
    Rates { input, cache_read: cache_read.unwrap_or(input / 10.), cache_write: input * 1.25, output }
}

/// Anthropic list prices, used when the downloaded table has no entry; the longest prefix wins.
fn built_in_price(model: &str) -> Option<Rates> {
    let table: [(&str, Rates); 11] = [
        ("claude-fable-5-1", list_price(10., 50., Some(0.25))),
        ("claude-fable-5", list_price(10., 50., None)),
        ("claude-mythos", list_price(10., 50., Some(0.25))),
        ("claude-opus-5-5", list_price(4., 20., Some(0.2))),
        ("claude-opus-5", list_price(5., 25., None)),
        ("claude-opus-4", list_price(5., 25., None)),
        ("claude-sonnet-5-5", list_price(2., 10., Some(0.2))),
        ("claude-sonnet-5", list_price(2., 10., None)),
        ("claude-sonnet-4", list_price(3., 15., None)),
        ("claude-haiku-5-5", list_price(0.1, 0.5, None)),
        ("claude-haiku-4", list_price(1., 5., None)),
    ];
    table
        .iter()
        .filter(|(prefix, _)| model.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, rates)| *rates)
}

/// The rates an entry was billed at: fast prices for fast entries when the table has them.
fn entry_rates(entry: &Entry, prices: &HashMap<String, Price>) -> Option<Rates> {
    match prices.get(&entry.model) {
        Some(price) => Some(if entry.fast { price.fast.unwrap_or(price.rates) } else { price.rates }),
        None => built_in_price(&entry.model),
    }
}

/// Estimated cost of one entry; none when the model has no price and the provider logged none.
pub(crate) fn entry_cost(entry: &Entry, prices: &HashMap<String, Price>) -> Option<f64> {
    if let Some(cost) = entry.reported {
        return Some(cost);
    }
    let r = entry_rates(entry, prices)?;
    Some(
        (entry.input * r.input + entry.cached * r.cache_read + entry.cache_write * r.cache_write + entry.output * r.output)
            / 1_000_000.,
    )
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Bucket {
    pub at: i64,
    pub cost: [f64; 4],
    pub tokens: [f64; 4],
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ProviderTotal {
    pub cost: f64,
    pub tokens: f64,
    pub sessions: usize,
    pub unpriced: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelRow {
    pub provider: Prov,
    pub model: String,
    pub cost: Option<f64>,
    pub tokens: f64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Summary {
    pub cost: f64,
    pub tokens: f64,
    pub sessions: usize,
    pub input: f64,
    pub cached: f64,
    pub cache_write: f64,
    pub output: f64,
    /// Cost by token type: input, cache read, cache write, output, reported by the provider.
    pub by_type: [f64; 5],
    pub savings: f64,
    pub partial: bool,
    pub providers: Vec<(Prov, ProviderTotal)>,
    pub models: Vec<ModelRow>,
    pub buckets: Vec<Bucket>,
    pub hourly: bool,
}

pub(crate) fn date_start(date: NaiveDate) -> i64 {
    let midnight = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map_or_else(|| midnight.and_utc().timestamp_millis(), |d| d.timestamp_millis())
}

pub(crate) fn local_date(ms: i64) -> NaiveDate {
    Local.timestamp_millis_opt(ms).single().map_or_else(|| chrono::Utc::now().date_naive(), |d| d.date_naive())
}

/// Bucket starts covering the range, oldest first: 24 hours, or `range` local days ending today.
pub(crate) fn bucket_starts(range: u32, now: i64) -> Vec<i64> {
    if range <= 1 {
        let last = now.div_euclid(HOUR) * HOUR;
        return (0..24).map(|i| last - (23 - i) * HOUR).collect();
    }
    let today = local_date(now);
    (0..range as i64).map(|i| date_start(today - chrono::Duration::days(range as i64 - 1 - i))).collect()
}

pub(crate) fn summarize(entries: &[Entry], range: u32, now: i64, prices: &HashMap<String, Price>) -> Summary {
    let starts = bucket_starts(range, now);
    let hourly = range <= 1;
    let from = starts[0];
    let index: HashMap<i64, usize> = starts.iter().enumerate().map(|(i, s)| (*s, i)).collect();
    let mut out = Summary {
        hourly,
        buckets: starts.iter().map(|at| Bucket { at: *at, ..Default::default() }).collect(),
        ..Default::default()
    };
    let mut totals: [ProviderTotal; 4] = Default::default();
    let mut sessions: [std::collections::HashSet<&str>; 4] = Default::default();
    let mut models: HashMap<(usize, &str), ModelRow> = HashMap::new();
    for entry in entries {
        if entry.at < from || entry.at > now {
            continue;
        }
        let prov = Prov::of(&entry.provider);
        let p = prov.index();
        let amount = entry.tokens();
        let cost = entry_cost(entry, prices);
        if cost.is_none() && amount > 0. {
            out.partial = true;
            totals[p].unpriced = true;
        }
        out.cost += cost.unwrap_or(0.);
        out.tokens += amount;
        out.input += entry.input;
        out.cached += entry.cached;
        out.cache_write += entry.cache_write;
        out.output += entry.output;
        match (entry.reported, entry_rates(entry, prices)) {
            (Some(reported), _) => out.by_type[4] += reported,
            (None, Some(r)) => {
                out.by_type[0] += entry.input * r.input / 1e6;
                out.by_type[1] += entry.cached * r.cache_read / 1e6;
                out.by_type[2] += entry.cache_write * r.cache_write / 1e6;
                out.by_type[3] += entry.output * r.output / 1e6;
                out.savings += entry.cached * (r.input - r.cache_read) / 1e6;
            }
            _ => {}
        }
        totals[p].cost += cost.unwrap_or(0.);
        totals[p].tokens += amount;
        sessions[p].insert(entry.session.as_str());
        let row = models.entry((p, entry.model.as_str())).or_insert_with(|| ModelRow {
            provider: prov,
            model: entry.model.clone(),
            cost: None,
            tokens: 0.,
        });
        row.tokens += amount;
        if let Some(cost) = cost {
            row.cost = Some(row.cost.unwrap_or(0.) + cost);
        }
        let start = if hourly { entry.at.div_euclid(HOUR) * HOUR } else { date_start(local_date(entry.at)) };
        if let Some(slot) = index.get(&start) {
            out.buckets[*slot].cost[p] += cost.unwrap_or(0.);
            out.buckets[*slot].tokens[p] += amount;
        }
    }
    for (i, prov) in PROVS.iter().enumerate() {
        totals[i].sessions = sessions[i].len();
        if totals[i].tokens > 0. || totals[i].cost > 0. {
            out.sessions += totals[i].sessions;
            out.providers.push((*prov, totals[i].clone()));
        }
    }
    out.models = models.into_values().filter(|r| r.tokens > 0.).collect();
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Metric {
    Cost,
    Tokens,
}

impl Bucket {
    fn value(&self, metric: Metric, p: usize) -> f64 {
        match metric {
            Metric::Cost => self.cost[p],
            Metric::Tokens => self.tokens[p],
        }
    }
}

/// Rows ordered by cost (unpriced last, by tokens) or by tokens.
pub(crate) fn sort_models(rows: &mut [ModelRow], metric: Metric) {
    rows.sort_by(|a, b| match metric {
        Metric::Tokens => b.tokens.total_cmp(&a.tokens),
        Metric::Cost => {
            b.cost.unwrap_or(-1.).total_cmp(&a.cost.unwrap_or(-1.)).then(b.tokens.total_cmp(&a.tokens))
        }
    });
}

/// One provider's band in the stacked cumulative chart.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layer {
    pub prov: Prov,
    pub bottom: Vec<f64>,
    pub top: Vec<f64>,
}

/// The cumulative total of each provider over the buckets, stacked in provider order. A hidden
/// provider leaves the stack; one that never used anything has no band.
pub(crate) fn stack(buckets: &[Bucket], metric: Metric, hidden: [bool; 4]) -> (Vec<Layer>, f64) {
    let mut below = vec![0.; buckets.len()];
    let mut layers = Vec::new();
    for (p, prov) in PROVS.iter().enumerate() {
        if hidden[p] || buckets.iter().all(|b| b.value(metric, p) <= 0.) {
            continue;
        }
        let mut running = 0.;
        let top: Vec<f64> = buckets
            .iter()
            .enumerate()
            .map(|(i, b)| {
                running += b.value(metric, p);
                below[i] + running
            })
            .collect();
        layers.push(Layer { prov: *prov, bottom: below.clone(), top: top.clone() });
        below = top;
    }
    let max = below.iter().copied().fold(0., f64::max);
    (layers, max)
}

fn frac(part: f64, whole: f64) -> f32 {
    if whole > 0. { (part / whole).clamp(0., 1.) as f32 } else { 0. }
}

// ---------------------------------------------------------------- number and date text

fn separators(en: bool) -> (char, char) {
    if en { (',', '.') } else { ('.', ',') }
}

fn trim_zeros(text: String, decimal: char) -> String {
    if text.contains(decimal) {
        text.trim_end_matches('0').trim_end_matches(decimal).to_owned()
    } else {
        text
    }
}

pub(crate) fn usd(value: f64, en: bool) -> String {
    let (group, decimal) = separators(en);
    let cents = (value.abs() * 100.).round() as u64;
    let whole = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(group);
        }
        grouped.push(ch);
    }
    let sign = if value < 0. { "-" } else { "" };
    let prefix = if en { "$" } else { "US$ " };
    format!("{sign}{prefix}{grouped}{decimal}{:02}", cents % 100)
}

/// 1.2 mil / 3.4 mi (pt) or 1.2K / 3.4M (en), up to two decimals from a thousand.
pub(crate) fn count(value: f64, en: bool) -> String {
    let (_, decimal) = separators(en);
    let (scaled, suffix) = if value >= 1e9 {
        (value / 1e9, if en { "B" } else { " bi" })
    } else if value >= 1e6 {
        (value / 1e6, if en { "M" } else { " mi" })
    } else if value >= 1e3 {
        (value / 1e3, if en { "K" } else { " mil" })
    } else {
        return format!("{}", value.round() as i64);
    };
    let text = format!("{scaled:.2}").replace('.', &decimal.to_string());
    format!("{}{suffix}", trim_zeros(text, decimal))
}

/// A chart axis tick in the same words as `count` ("2 mil", "1,5 mi", "2K"), with only the
/// decimals the tick step needs. Cost ticks take the currency's prefix: "US$ " or "$".
pub(crate) fn axis_label(value: f64, step: f64, cost: bool, en: bool) -> String {
    let (_, decimal) = separators(en);
    let (divisor, suffix) = if value.abs() >= 1e9 {
        (1e9, if en { "B" } else { " bi" })
    } else if value.abs() >= 1e6 {
        (1e6, if en { "M" } else { " mi" })
    } else if value.abs() >= 1e3 {
        (1e3, if en { "K" } else { " mil" })
    } else {
        (1., "")
    };
    let scaled_step = step / divisor;
    let decimals = (0..=3)
        .find(|d| {
            let s = scaled_step * 10f64.powi(*d);
            (s - s.round()).abs() < 1e-6
        })
        .unwrap_or(3) as usize;
    let text = format!("{:.decimals$}", value / divisor).replace('.', &decimal.to_string());
    let number = format!("{}{suffix}", trim_zeros(text, decimal));
    match (cost, en) {
        (false, _) => number,
        (true, true) => format!("${number}"),
        (true, false) => format!("US$ {number}"),
    }
}

pub(crate) fn share(part: f64, whole: f64, en: bool) -> String {
    if whole <= 0. {
        return "—".to_owned();
    }
    let (_, decimal) = separators(en);
    let percent = part / whole * 100.;
    if percent > 0. && percent < 0.1 {
        return format!("<0{decimal}1%");
    }
    format!("{}%", trim_zeros(format!("{percent:.1}").replace('.', &decimal.to_string()), decimal))
}

/// A percentage with at most one decimal: "14,3", "8".
pub(crate) fn percent(value: f64, en: bool) -> String {
    let (_, decimal) = separators(en);
    trim_zeros(format!("{value:.1}").replace('.', &decimal.to_string()), decimal)
}

const MONTHS_PT: [&str; 12] = ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez"];
const MONTHS_EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS_PT: [&str; 7] = ["seg", "ter", "qua", "qui", "sex", "sáb", "dom"];
const DAYS_EN: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn local_time(ms: i64) -> NaiveDateTime {
    Local.timestamp_millis_opt(ms).single().map_or_else(|| chrono::Utc::now().naive_utc(), |d| d.naive_local())
}

/// An axis label: "14:00" for hours, "5 nov" / "Nov 5" for days.
pub(crate) fn bucket_label(at: i64, hourly: bool, en: bool) -> String {
    let when = local_time(at);
    if hourly {
        return format!("{:02}:00", when.hour());
    }
    let month = if en { MONTHS_EN } else { MONTHS_PT }[when.month0() as usize];
    if en { format!("{month} {}", when.day()) } else { format!("{} {month}", when.day()) }
}

/// The tooltip's title: the weekday and date, with the hour's span for hourly buckets.
pub(crate) fn bucket_title(at: i64, hourly: bool, en: bool) -> String {
    let when = local_time(at);
    let weekday = if en { DAYS_EN } else { DAYS_PT }[when.weekday().num_days_from_monday() as usize];
    let date = bucket_label(at, false, en);
    if hourly {
        format!("{weekday}, {date} · {:02}:00–{:02}:00", when.hour(), (when.hour() + 1) % 24)
    } else {
        format!("{weekday}, {date}")
    }
}

// ---------------------------------------------------------------- limits and daily pace

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Win {
    pub id: String,
    pub used: Option<f64>,
    pub resets_at: Option<i64>,
    pub minutes: Option<i64>,
}

pub(crate) fn parse_windows(value: &Value) -> Vec<Win> {
    value["windows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| Win {
            id: w["id"].as_str().unwrap_or("").to_owned(),
            used: w["usedPercent"].as_f64(),
            // Seconds or milliseconds, as the shell's own reading accepts both.
            resets_at: w["resetsAt"].as_i64().map(|at| if at > 100_000_000_000 { at } else { at * 1000 }),
            minutes: w["durationMinutes"].as_i64(),
        })
        .collect()
}

/// The provider's main quota first, the reserve model's after.
fn ordered(windows: &[Win]) -> Vec<&Win> {
    let mut list: Vec<&Win> = windows.iter().collect();
    list.sort_by_key(|w| w.id.starts_with("base_model_inference/"));
    list
}

/// The account's weekly window, with a percentage and a reset time; none means no pace.
pub(crate) fn weekly(windows: &[Win]) -> Option<&Win> {
    windows.iter().find(|w| {
        !w.id.starts_with("base_model_inference/")
            && !["_opus", "_sonnet", "_oauth_apps"].iter().any(|s| w.id.ends_with(s))
            && (w.minutes == Some(WEEK_MINUTES) || matches!(w.id.as_str(), "seven_day" | "opencode-go/weekly"))
            && w.used.is_some()
            && w.resets_at.is_some()
    })
}

fn window_label(w: &Win) -> &'static str {
    match w.id.as_str() {
        "five_hour" | "opencode-go/rolling" => t("Uso em 5 horas"),
        "seven_day" | "opencode-go/weekly" => t("Uso semanal"),
        "seven_day_opus" => t("Semanal · Opus"),
        "seven_day_sonnet" => t("Semanal · Sonnet"),
        "seven_day_oauth_apps" => t("Semanal · apps OAuth"),
        "opencode-go/monthly" => t("Uso mensal"),
        "cursor/models" => t("Modelos do Cursor"),
        "cursor/other-models" => t("Outros modelos"),
        "cursor/included" => t("Uso incluído"),
        "cursor/member" => t("Limite individual"),
        "cursor/team" => t("Pool da equipe"),
        id if id.starts_with("base_model_inference/") => {
            if w.minutes == Some(300) { t("5 horas · modelo reserva") } else { t("Semanal · modelo reserva") }
        }
        _ if w.minutes == Some(300) => t("Uso em 5 horas"),
        _ if w.minutes == Some(WEEK_MINUTES) => t("Uso semanal"),
        _ => t("Outro limite"),
    }
}

/// The weekly percentage at the first reading of a local day.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Reading {
    pub used: f64,
    #[serde(rename = "resetsAt")]
    pub resets_at: i64,
    pub at: i64,
}

/// `usage-day-readings.json`: for each provider/account, the first reading of each local day.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Readings {
    #[serde(default)]
    pub accounts: BTreeMap<String, BTreeMap<String, Reading>>,
}

impl Readings {
    pub(crate) fn path() -> std::path::PathBuf {
        crate::backend::data_dir().join("usage-day-readings.json")
    }

    pub(crate) fn load() -> Self {
        std::fs::read_to_string(Self::path()).ok().and_then(|text| Self::from_json(&text)).unwrap_or_default()
    }

    pub(crate) fn from_json(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    pub(crate) fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub(crate) fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let temp = path.with_extension("json.tmp");
        if std::fs::write(&temp, self.to_json()).is_ok() {
            let _ = std::fs::rename(&temp, &path);
        }
    }

    /// Keeps the first reading of `day`; true when this one was it (the file needs writing).
    pub(crate) fn record(&mut self, key: &str, day: &str, reading: Reading) -> bool {
        let days = self.accounts.entry(key.to_owned()).or_default();
        if days.contains_key(day) {
            return false;
        }
        days.insert(day.to_owned(), reading);
        while days.len() > KEEP_DAYS {
            let Some(oldest) = days.keys().next().cloned() else { break };
            days.remove(&oldest);
        }
        true
    }

    pub(crate) fn baseline(&self, key: &str, day: &str) -> Option<&Reading> {
        self.accounts.get(key)?.get(day)
    }

    /// Whether the account has a reading from before `day`; without one it is the first day.
    pub(crate) fn has_earlier(&self, key: &str, day: &str) -> bool {
        self.accounts.get(key).is_some_and(|days| days.keys().any(|d| d.as_str() < day))
    }
}

/// Today's share of a weekly limit, and where an even pace would be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pace {
    /// Percentage used at the start of the local day.
    pub start: f64,
    /// X: used since then.
    pub today: f64,
    /// Y: what today may use, the rest of the week spread over the days left.
    pub allowance: f64,
    /// X / Y (above 1 is over budget).
    pub fill: f64,
    pub over: bool,
    pub days_left: u32,
    /// Share of the week gone, 0 to 1.
    pub elapsed: f64,
    pub ahead: bool,
}

/// Days from today to the reset, counting today (at least 1). A reset at midnight sharp belongs
/// to the day before.
pub(crate) fn days_left(today: NaiveDate, reset: NaiveDateTime) -> u32 {
    let reset_day = (reset - chrono::Duration::milliseconds(1)).date();
    ((reset_day - today).num_days() + 1).max(1) as u32
}

/// A baseline taken in an earlier week is no use once the reset has passed: the week began at 0.
const RESET_SLACK: i64 = HOUR;

pub(crate) fn pace(
    used: f64,
    baseline: Option<&Reading>,
    resets_at: i64,
    duration_minutes: Option<i64>,
    now: i64,
    days_left: u32,
) -> Pace {
    let start = match baseline {
        Some(b) if resets_at - b.resets_at > RESET_SLACK => 0.,
        Some(b) => b.used.min(used),
        None => used,
    };
    let today = (used - start).max(0.);
    let allowance = (100. - start).max(0.) / f64::from(days_left.max(1));
    let fill = if allowance > 0. { today / allowance } else if today > 0. { f64::INFINITY } else { 0. };
    let span = duration_minutes.filter(|m| *m > 0).unwrap_or(WEEK_MINUTES) * 60_000;
    let elapsed = (1. - (resets_at - now) as f64 / span as f64).clamp(0., 1.);
    Pace { start, today, allowance, fill, over: today > allowance + 1e-9, days_left, elapsed, ahead: used > elapsed * 100. }
}

// ---------------------------------------------------------------- the window's state

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Cost,
    Tokens,
    Limits,
}

pub(super) struct Card {
    provider: &'static str,
    name: &'static str,
    account: String,
    label: String,
    /// More than one account for the provider: the account's name is shown.
    several: bool,
    email: Option<String>,
    windows: Vec<Win>,
    note: Option<String>,
    loaded: bool,
}

pub(super) struct State {
    pub open: Option<Tween>,
    tab: Tab,
    range: u32,
    entries: Option<Vec<Entry>>,
    prices: HashMap<String, Price>,
    problems: Vec<String>,
    error: Option<String>,
    loading: bool,
    hidden: [bool; 4],
    cards: Vec<Card>,
    generation: u64,
    readings: Readings,
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: None,
            tab: Tab::Cost,
            range: 30,
            entries: None,
            prices: HashMap::new(),
            problems: Vec::new(),
            error: None,
            loading: false,
            hidden: [false; 4],
            cards: Vec::new(),
            generation: 0,
            readings: Readings::load(),
        }
    }
}

impl State {
    /// How much the page covers the window, for the glass fade.
    pub(super) fn cover(&self, now: Instant) -> f32 {
        self.open.as_ref().map_or(0., |open| open.at(now).clamp(0., 1.))
    }
}

fn is_en() -> bool {
    crate::i18n::is_en()
}

impl Shell {
    pub(super) fn usage_page_open(&self) -> bool {
        self.usage_page.open.is_some()
    }

    pub(super) fn close_usage_page(&mut self) -> bool {
        self.usage_accounts = None;
        self.usage_page.open.take().is_some()
    }

    pub(super) fn toggle_usage_page(&mut self, cx: &mut Context<Self>) {
        if self.usage_page.open.is_some() {
            self.usage_page.open = None;
            self.usage_accounts = None;
            cx.notify();
            return;
        }
        self.usage_popup = None;
        self.usage_page.open = Some(Tween::from_to(
            0.,
            1.,
            Instant::now(),
            if cx.reduce_motion() { 0. } else { MENU },
            Curve::Expo,
        ));
        self.usage_load_history(cx);
        if self.usage_page.tab == Tab::Limits {
            self.usage_load_limits(false, cx);
        }
        cx.notify();
    }

    fn usage_load_history(&mut self, cx: &mut Context<Self>) {
        if self.usage_page.loading {
            return;
        }
        self.usage_page.loading = true;
        self.usage_page.error = None;
        let rx = self.backend.request("usage_history", json!({ "days": 90 }));
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let history = message_actions::settle(rx).await;
            let (entries, problems, prices) = match &history {
                Ok(value) => {
                    let entries = parse_entries(value);
                    let problems: Vec<String> = value["problems"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|p| format!("{}: {}", p["provider"].as_str().unwrap_or(""), p["message"].as_str().unwrap_or("")))
                        .collect();
                    let mut models: Vec<&str> = entries.iter().map(|e| e.model.as_str()).collect();
                    models.sort_unstable();
                    models.dedup();
                    // Without prices the built-in list applies.
                    let prices = message_actions::settle(backend.request("usage_prices", json!({ "models": models })))
                        .await
                        .ok()
                        .map(|value| parse_prices(&value));
                    (Some(entries), problems, prices)
                }
                Err(_) => (None, Vec::new(), None),
            };
            let _ = this.update(cx, |this, cx| {
                let page = &mut this.usage_page;
                page.loading = false;
                match (entries, history) {
                    (Some(entries), _) => {
                        page.entries = Some(entries);
                        page.problems = problems;
                        if let Some(prices) = prices {
                            page.prices = prices;
                        }
                    }
                    (None, Err(problem)) => page.error = Some(problem),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Opens the accounts popover of a provider on the Limites tab, over its first card.
    fn usage_open_accounts(&mut self, provider: &'static str, cx: &mut Context<Self>) {
        self.usage_accounts = Some(AccountsPage {
            provider,
            shown: Some(Tween::from_to(
                0.,
                1.,
                Instant::now(),
                if cx.reduce_motion() { 0. } else { MENU },
                Curve::Expo,
            )),
            ..Default::default()
        });
        self.load_account_usage(provider);
        cx.notify();
    }

    pub(super) fn usage_load_limits(&mut self, refresh: bool, cx: &mut Context<Self>) {
        let disabled: Vec<String> = self.settings["disabledProviders"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        self.usage_page.generation += 1;
        let generation = self.usage_page.generation;
        let mut cards = Vec::new();
        for (provider, name) in LIMIT_PROVIDERS {
            if disabled.iter().any(|d| d == provider) {
                continue;
            }
            let accounts = self.accounts_of(provider);
            let several = accounts.len() > 1;
            for (account, label) in accounts {
                cards.push(Card {
                    provider,
                    name,
                    account,
                    label,
                    several,
                    email: None,
                    windows: Vec::new(),
                    note: None,
                    loaded: false,
                });
            }
        }
        for (ix, card) in cards.iter().enumerate() {
            let rx = self.backend.request(
                "provider_usage",
                json!({ "provider": card.provider, "refresh": refresh, "accountId": card.account }),
            );
            cx.spawn(async move |this, cx| {
                let reply = message_actions::settle(rx).await;
                let _ = this.update(cx, |this, cx| {
                    this.usage_limits_arrived(generation, ix, reply, cx);
                });
            })
            .detach();
        }
        self.usage_page.cards = cards;
        cx.notify();
    }

    fn usage_limits_arrived(&mut self, generation: u64, ix: usize, reply: Result<Value, String>, cx: &mut Context<Self>) {
        if generation != self.usage_page.generation {
            return;
        }
        let page = &mut self.usage_page;
        let Some(card) = page.cards.get_mut(ix) else { return };
        card.loaded = true;
        match reply {
            Ok(value) => {
                card.windows = parse_windows(&value);
                card.email = value["account"]["email"].as_str().or(value["account"]["name"].as_str()).map(str::to_owned);
                card.note = value["note"].as_str().map(str::to_owned);
                // Only a reading that arrives is written, and only the day's first.
                if let Some(w) = weekly(&card.windows) {
                    let key = format!("{}/{}", card.provider, card.account);
                    let day = Local::now().format("%Y-%m-%d").to_string();
                    let reading = Reading {
                        used: w.used.unwrap_or(0.),
                        resets_at: w.resets_at.unwrap_or(0),
                        at: chrono::Utc::now().timestamp_millis(),
                    };
                    if page.readings.record(&key, &day, reading) {
                        page.readings.save();
                    }
                }
            }
            Err(problem) => card.note = Some(problem),
        }
        cx.notify();
    }

    fn usage_refresh(&mut self, cx: &mut Context<Self>) {
        if self.usage_page.tab == Tab::Limits {
            self.usage_load_limits(true, cx);
        } else {
            self.usage_load_history(cx);
        }
    }

    fn usage_set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.usage_page.tab == tab {
            return;
        }
        self.usage_page.tab = tab;
        if tab == Tab::Limits && self.usage_page.cards.is_empty() {
            self.usage_load_limits(false, cx);
        }
        cx.notify();
    }

    // ------------------------------------------------------------ drawing

    pub(super) fn usage_page_view(&self, now: Instant, moving: &mut bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.usage_page.open.as_ref()?;
        if !open.done(now) {
            *moving = true;
        }
        let p = open.at(now).clamp(0., 1.);
        let page = &self.usage_page;
        let tabs = [
            (Tab::Cost, t("Custo").to_owned()),
            (Tab::Tokens, t("Tokens").to_owned()),
            (Tab::Limits, t("Limites").to_owned()),
        ];
        let ranges: Vec<(u32, String)> = RANGES
            .iter()
            .map(|r| (*r, if *r == 1 { t("24 h").to_owned() } else { tf("{} dias", &[r]) }))
            .collect();
        let header = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(content(0.95))
                    .child(t("Uso")),
            )
            .child(segmented("usage-tab", &tabs, page.tab, |this, tab, cx| this.usage_set_tab(tab, cx), cx))
            .when(page.tab != Tab::Limits, |d| {
                d.child(segmented(
                    "usage-range",
                    &ranges,
                    page.range,
                    |this, range, cx| {
                        this.usage_page.range = range;
                        cx.notify();
                    },
                    cx,
                ))
            })
            .child(
                div()
                    .id("usage-refresh")
                    .size(px(28.))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.08)))
                    .on_click(cx.listener(|this, _, _, cx| this.usage_refresh(cx)))
                    .child(icon("refresh", 14.).text_color(content(if page.loading { 0.3 } else { 0.6 }))),
            );
        let back = crate::hover_track::list("usage-back").row_named(
            div()
                .id("usage-back")
                .h(px(30.))
                .px(px(10.))
                .w(px(150.))
                .rounded_md()
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(13.))
                .text_color(content(0.95))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.usage_page.open = None;
                    this.usage_accounts = None;
                    cx.notify();
                }))
                .child(icon("arrow-left", 16.).text_color(content(0.65)))
                .child(t("Voltar ao app")),
            "usage-back",
            false,
        );
        let body: AnyElement = if page.tab == Tab::Limits {
            self.limits_view(now, moving, cx)
        } else {
            self.history_view(if page.tab == Tab::Cost { Metric::Cost } else { Metric::Tokens }, cx)
        };
        Some(
            div()
                .id("usage-page")
                .occlude()
                .absolute()
                .inset_0()
                .opacity(p)
                .bg(if pane_fill().a < 0.5 { chrome_wash() } else { pane_opaque() })
                .flex()
                .child(
                    div()
                        .id("usage-main")
                        // A click outside the accounts popover closes it; the popover stops its own clicks.
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                if this.usage_accounts.take().is_some() {
                                    cx.notify();
                                }
                            }),
                        )
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .bg(pane_fill())
                        .overflow_y_scroll()
                        .child(
                            div()
                                .mx_auto()
                                .w_full()
                                .max_w(px(780.))
                                .pt(px(TITLE_BAR + 12.))
                                .pb(px(40.))
                                .px(px(32.))
                                .flex()
                                .flex_col()
                                .gap(px(18.))
                                .child(div().ml(px(-10.)).child(crate::hover_track::list("usage-back").pill()).child(back))
                                .child(header)
                                .child(body),
                        ),
                )
                .into_any_element(),
        )
    }

    fn history_view(&self, metric: Metric, cx: &mut Context<Self>) -> AnyElement {
        let page = &self.usage_page;
        let en = is_en();
        let mut column = div().flex().flex_col().gap(px(18.));
        if let Some(error) = &page.error {
            column = column.child(notice(error.clone(), true));
        }
        for problem in &page.problems {
            column = column.child(notice(problem.clone(), false));
        }
        let Some(entries) = &page.entries else {
            if page.error.is_none() {
                column = column.child(muted(t("Lendo o histórico…")));
            }
            return column.into_any_element();
        };
        let now = chrono::Utc::now().timestamp_millis();
        let summary = summarize(entries, page.range, now, &page.prices);
        if summary.tokens <= 0. {
            return column.child(muted(t("Nenhum uso nesse período."))).into_any_element();
        }
        let money = |v: f64| usd(v, en);
        let amount = |v: f64| if metric == Metric::Cost { money(v) } else { count(v, en) };
        let whole = if metric == Metric::Cost { summary.cost } else { summary.tokens };
        let span = if page.range == 1 { t("últimas 24 horas").to_owned() } else { tf("últimos {} dias", &[&page.range]) };
        let mut caption = format!("{} · {span}", tf("{} sessões", &[&summary.sessions]));
        if metric == Metric::Cost {
            caption.push_str(" · ");
            caption.push_str(if summary.partial {
                t("estimativa parcial, há modelos sem preço")
            } else {
                t("estimativa a preços de tabela")
            });
        }
        column = column.child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(px(34.)).font_weight(FontWeight::SEMIBOLD).text_color(content(1.)).child(amount(whole)))
                .child(div().text_size(px(12.)).text_color(content(0.5)).child(caption)),
        );
        // Provider cards. Clicking one hides it from the chart; its colour never changes.
        column = column.child(div().flex().gap(px(10.)).children(summary.providers.iter().map(|(prov, total)| {
            let prov = *prov;
            let value = if metric == Metric::Cost { total.cost } else { total.tokens };
            let other = if metric == Metric::Cost {
                tf("{} tokens", &[&count(total.tokens, en)])
            } else {
                money(total.cost)
            };
            let mut sub = format!("{} · {other}", tf("{} sessões", &[&total.sessions]));
            if total.unpriced && metric == Metric::Cost {
                sub.push_str(" · ");
                sub.push_str(t("sem preço"));
            }
            let hidden = page.hidden[prov.index()];
            div()
                .id(("usage-card", prov.index()))
                .flex_1()
                .min_w_0()
                .p(px(12.))
                .rounded_xl()
                .border_1()
                .border_color(content(0.08))
                .bg(content(0.03))
                .flex()
                .flex_col()
                .gap(px(6.))
                .cursor_pointer()
                .opacity(if hidden { 0.45 } else { 1. })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.usage_page.hidden[prov.index()] = !this.usage_page.hidden[prov.index()];
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_size(px(13.))
                        .text_color(content(0.9))
                        .child(div().size(px(9.)).rounded_full().bg(prov.color()))
                        .child(div().flex_1().truncate().child(prov.name()))
                        .child(value_text(amount(value))),
                )
                .child(div().text_size(px(11.)).text_color(content(0.5)).child(sub))
                .child(
                    div().h(px(4.)).rounded_full().bg(content(0.08)).overflow_hidden().child(
                        div().h_full().rounded_full().bg(prov.color()).w(gpui::relative(frac(value, whole))),
                    ),
                )
        })));
        // The chart: stacked cumulative areas.
        let title = if metric == Metric::Cost { t("Custo acumulado") } else { t("Tokens acumulados") };
        column = column.child(heading(title)).child(self.area_chart(&summary, metric, en));
        column = column.child(legend(page, &summary, cx));
        // Stat tiles.
        let tiles = [
            (t("Processados"), count(summary.tokens, en)),
            (t("Entrada em cache"), count(summary.cached, en)),
            (t("Entrada sem cache"), count(summary.input, en)),
            (t("Saída"), count(summary.output, en)),
            (t("Economia do cache"), money(summary.savings)),
        ];
        column = column.child(div().flex().gap(px(8.)).children(tiles.into_iter().map(|(label, value)| {
            div()
                .flex_1()
                .min_w_0()
                .p(px(10.))
                .rounded_lg()
                .bg(content(0.04))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(div().text_size(px(11.)).text_color(content(0.5)).truncate().child(label))
                .child(value_text(value))
        })));
        // By type.
        let parts: Vec<(&'static str, f64)> = if metric == Metric::Cost {
            vec![
                (t("Entrada"), summary.by_type[0]),
                (t("Leitura de cache"), summary.by_type[1]),
                (t("Escrita de cache"), summary.by_type[2]),
                (t("Saída"), summary.by_type[3]),
                (t("Informado pelo provedor"), summary.by_type[4]),
            ]
        } else {
            vec![
                (t("Entrada"), summary.input),
                (t("Leitura de cache"), summary.cached),
                (t("Escrita de cache"), summary.cache_write),
                (t("Saída"), summary.output),
            ]
        };
        let parts: Vec<(&'static str, f64)> = parts.into_iter().filter(|(_, v)| *v > 0.).collect();
        let sum: f64 = parts.iter().map(|(_, v)| v).sum();
        if sum > 0. {
            let tone = |i: usize| match i {
                0 => accent(),
                1 => content(0.6),
                2 => content(0.4),
                3 => content(0.26),
                _ => content(0.16),
            };
            column = column
                .child(heading(if metric == Metric::Cost { t("Custo por tipo") } else { t("Tokens por tipo") }))
                .child(div().flex().h(px(8.)).gap(px(2.)).rounded_full().overflow_hidden().children(
                    parts.iter().enumerate().map(|(i, (_, v))| div().h_full().bg(tone(i)).w(gpui::relative(frac(*v, sum))).flex_shrink_0()),
                ))
                .child(div().flex().flex_wrap().gap_x(px(16.)).gap_y(px(4.)).children(parts.iter().enumerate().map(
                    |(i, (label, v))| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .text_color(content(0.6))
                            .child(div().size(px(8.)).rounded_full().bg(tone(i)))
                            .child(*label)
                            .child(div().text_color(content(0.9)).child(amount(*v)))
                    },
                )));
        }
        // By model.
        let mut rows = summary.models.clone();
        sort_models(&mut rows, metric);
        let list = crate::hover_track::list("usage-models");
        let columns = |a: AnyElement, b: AnyElement, c: AnyElement, d: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().min_w_0().child(a))
                .child(div().w(px(84.)).flex_shrink_0().flex().justify_end().child(b))
                .child(div().w(px(76.)).flex_shrink_0().flex().justify_end().child(c))
                .child(div().w(px(72.)).flex_shrink_0().flex().justify_end().child(d))
        };
        let head = |text: &'static str| div().text_size(px(11.)).text_color(content(0.45)).child(text).into_any_element();
        column = column
            .child(heading(t("Por modelo")))
            .child(div().px(px(10.)).child(columns(head(t("Modelo")), head(t("Custo")), head(t("Participação")), head(t("Tokens")))))
            .child(div().relative().child(list.pill()).children(rows.iter().enumerate().map(|(ix, row)| {
                let part = if metric == Metric::Cost { row.cost.unwrap_or(0.) } else { row.tokens };
                let cost_text = row.cost.map_or_else(|| t("sem preço").to_owned(), |c| usd(c, en));
                let cell = |text: String, dim: bool| {
                    div().text_size(px(12.)).text_color(content(if dim { 0.4 } else { 0.8 })).child(text).into_any_element()
                };
                let name = div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(row.provider.color()))
                            .child(div().truncate().text_size(px(12.)).text_color(content(0.9)).child(row.model.clone())),
                    )
                    .child(
                        div()
                            .h(px(2.))
                            .rounded_full()
                            .bg(row.provider.color())
                            .w(gpui::relative(frac(part, whole).max(0.015))),
                    )
                    .into_any_element();
                list.row(
                    div().px(px(10.)).py(px(7.)).rounded_md().child(columns(
                        name,
                        cell(cost_text, row.cost.is_none()),
                        cell(share(part, whole, en), false),
                        cell(count(row.tokens, en), false),
                    )),
                    ix,
                    false,
                )
            })));
        column.into_any_element()
    }

    /// The stacked cumulative area chart, drawn by hand: a 2 px gap between bands, a recessive
    /// grid, a crosshair and a card with the day's values per provider and the total.
    fn area_chart(&self, summary: &Summary, metric: Metric, en: bool) -> AnyElement {
        let (layers, max) = stack(&summary.buckets, metric, self.usage_page.hidden);
        let n = summary.buckets.len();
        let scale = Scale::nice([max], true);
        let y_ticks: Vec<(f64, String)> = scale
            .ticks()
            .into_iter()
            .map(|v| (v, axis_label(v, scale.step, metric == Metric::Cost, en)))
            .collect();
        let hourly = summary.hourly;
        let x_ticks: Vec<(usize, String)> = [0, (n - 1) / 2, n - 1]
            .into_iter()
            .map(|i| (i, bucket_label(summary.buckets[i].at, hourly, en)))
            .collect();
        let fmt = |v: f64| if metric == Metric::Cost { usd(v, en) } else { count(v, en) };
        let mut running = 0.;
        let tips: Vec<(String, Vec<(gpui::Hsla, String, String)>)> = summary
            .buckets
            .iter()
            .map(|b| {
                let mut rows = Vec::new();
                let mut day = 0.;
                for (p, prov) in PROVS.iter().enumerate() {
                    if !layers.iter().any(|l| l.prov == *prov) {
                        continue;
                    }
                    let v = b.value(metric, p);
                    day += v;
                    rows.push((prov.color(), prov.name().to_owned(), fmt(v)));
                }
                running += day;
                rows.push((content(0.9), t("Total").to_owned(), fmt(day)));
                rows.push((content(0.45), t("Acumulado").to_owned(), fmt(running)));
                (bucket_title(b.at, hourly, en), rows)
            })
            .collect();
        let bands: Vec<(gpui::Hsla, Vec<f64>, Vec<f64>)> =
            layers.iter().map(|l| (l.prov.color(), l.bottom.clone(), l.top.clone())).collect();
        hoverable("usage-chart".into())
            .w_full()
            .h(px(230.))
            .child(
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                            paint_area(bounds, &scale, &y_ticks, &x_ticks, &bands, &tips, n, window, cx)
                        })
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

    fn limits_view(&self, at: Instant, moving: &mut bool, cx: &mut Context<Shell>) -> AnyElement {
        let page = &self.usage_page;
        let en = is_en();
        let mut column = div().flex().flex_col().gap(px(14.));
        if page.cards.is_empty() {
            return column.child(muted(t("Lendo os limites…"))).into_any_element();
        }
        let now = chrono::Utc::now().timestamp_millis();
        let today = local_date(now);
        let day = today.format("%Y-%m-%d").to_string();
        let amber: gpui::Hsla = rgb(AMBER).into();
        for (ix, card) in page.cards.iter().enumerate() {
            // The first card of a provider with accounts carries its Add account button.
            let first = ix == 0 || page.cards[ix - 1].provider != card.provider;
            let provider = card.provider;
            let key = format!("{}/{}", card.provider, card.account);
            let color = Prov::of(card.provider).color();
            let mut head = div().flex().items_center().gap(px(8.)).child(div().size(px(9.)).rounded_full().bg(color)).child(
                div().text_size(px(14.)).font_weight(FontWeight::MEDIUM).text_color(content(0.95)).child(card.name),
            );
            if card.several {
                head = head.child(
                    div()
                        .px(px(7.))
                        .py(px(1.))
                        .rounded_full()
                        .bg(content(0.08))
                        .text_size(px(11.))
                        .text_color(content(0.7))
                        .child(card.label.clone()),
                );
            }
            if let Some(email) = &card.email {
                head = head.child(div().text_size(px(11.)).text_color(content(0.45)).truncate().child(email.clone()));
            }
            if first && matches!(provider, "claude" | "codex") {
                head = head.child(div().flex_1()).child(
                    div()
                        .id(("usage-add-account", ix))
                        .h(px(26.))
                        .px(px(9.))
                        .rounded(px(7.))
                        .bg(content(0.04))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(11.5))
                        .text_color(content(0.72))
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.09)).text_color(content(0.95)))
                        .on_click(cx.listener(move |this, _, _, cx| this.usage_open_accounts(provider, cx)))
                        .child(icon("plus", 13.).text_color(content(0.6)))
                        .child(t("Adicionar conta")),
                );
            }
            let mut block = div()
                .relative()
                .p(px(14.))
                .rounded_xl()
                .border_1()
                .border_color(content(0.08))
                .bg(content(0.03))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(head);
            let main = weekly(&card.windows).map(|w| w.id.clone());
            for w in ordered(&card.windows) {
                let used = w.used;
                let left = used.map(|u| (100. - u.round()).max(0.));
                let mut bar = div().relative().h(px(6.)).rounded_full().bg(content(0.08));
                if let Some(u) = used {
                    bar = bar.child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(if u >= 90. { rgb(0xe5675f).into() } else { color })
                            .w(gpui::relative(frac(u, 100.))),
                    );
                }
                let mut pace_lines: Vec<AnyElement> = Vec::new();
                if main.as_deref() == Some(w.id.as_str())
                    && let (Some(u), Some(reset)) = (w.used, w.resets_at)
                {
                    let left_days = days_left(today, local_time(reset));
                    let pace = pace(u, page.readings.baseline(&key, &day), reset, w.minutes, now, left_days);
                    // The tick where an even pace would be, on the weekly bar.
                    bar = bar.child(
                        div()
                            .absolute()
                            .top(px(-3.))
                            .h(px(12.))
                            .w(px(2.))
                            .rounded_full()
                            .bg(content(0.85))
                            .left(gpui::relative(pace.elapsed as f32)),
                    );
                    let verdict = if pace.ahead { t("acima do ritmo") } else { t("abaixo do ritmo") };
                    pace_lines.push(
                        div()
                            .text_size(px(12.))
                            .text_color(if pace.ahead { amber } else { content(0.6) })
                            .child(format!(
                                "{verdict} · {}",
                                tf("ritmo uniforme em {}%", &[&percent(pace.elapsed * 100., en)])
                            ))
                            .into_any_element(),
                    );
                    let mut today_line = div()
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(12.))
                                .text_color(content(0.8))
                                .child(tf("Hoje: {}% de {}% disponíveis", &[&percent(pace.today, en), &percent(pace.allowance, en)]))
                                .when(pace.over, |d| {
                                    d.child(
                                        div().text_color(amber).font_weight(FontWeight::MEDIUM).child(t("acima do orçamento de hoje")),
                                    )
                                }),
                        )
                        .child(
                            div().h(px(6.)).rounded_full().bg(content(0.08)).overflow_hidden().child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(if pace.over { amber } else { accent() })
                                    .w(gpui::relative(pace.fill.min(1.) as f32)),
                            ),
                        );
                    if !page.readings.has_earlier(&key, &day) {
                        today_line = today_line.child(
                            div()
                                .text_size(px(11.))
                                .text_color(content(0.4))
                                .child(t("Primeiro dia com leitura: o ponto de partida é a primeira leitura de hoje.")),
                        );
                    }
                    pace_lines.push(today_line.into_any_element());
                }
                let reset = w.resets_at.filter(|r| *r > now).map(|r| tf("renova em {}", &[&reset_left(r)]));
                block = block.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(7.))
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(px(8.))
                                .child(div().flex_1().text_size(px(12.)).text_color(content(0.7)).child(window_label(w)))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(content(0.95))
                                        .child(left.map_or("—".to_owned(), |l| tf("{}% restante", &[&l]))),
                                )
                                .children(reset.map(|r| div().text_size(px(11.)).text_color(content(0.45)).child(r))),
                        )
                        .child(bar)
                        .children(pace_lines),
                );
            }
            if card.windows.is_empty() {
                let note = card.note.clone().unwrap_or_else(|| {
                    if card.loaded { t("Sem limites informados.").to_owned() } else { t("Lendo os limites…").to_owned() }
                });
                block = block.child(div().text_size(px(12.)).text_color(content(0.5)).child(note));
            }
            // The accounts popover hangs under the Add account button, fading in. It is deferred so
            // the scrolling column does not clip it, and a click outside closes it (usage-main).
            if let Some(accounts) = self.usage_accounts.as_ref().filter(|p| first && p.provider == provider) {
                let shown = accounts.shown.as_ref().map_or(1., |tween| tween.at(at).clamp(0., 1.));
                if accounts.shown.as_ref().is_some_and(|tween| !tween.done(at)) {
                    *moving = true;
                }
                block = block.child(
                    gpui::deferred(
                        div()
                            .id("usage-accounts")
                            .occlude()
                            .absolute()
                            .top(px(40. + (1. - shown) * -6.))
                            .right(px(14.))
                            .w(px(310.))
                            .p(px(14.))
                            .flex()
                            .flex_col()
                            .opacity(shown)
                            .rounded(px(14.))
                            .border_1()
                            .border_color(content(0.12))
                            .child(crate::theme::floating_surface(14.))
                            .shadow_lg()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(self.accounts_page_view(provider, cx)),
                    )
                    .with_priority(2),
                );
            }
            column = column.child(block);
        }
        column.into_any_element()
    }
}

// ---------------------------------------------------------------- small views

fn heading(text: &'static str) -> gpui::Div {
    div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(content(0.9)).child(text)
}

fn muted(text: &'static str) -> gpui::Div {
    div().py(px(24.)).text_size(px(13.)).text_color(content(0.5)).child(text)
}

fn value_text(text: String) -> gpui::Div {
    div().text_size(px(14.)).font_weight(FontWeight::MEDIUM).text_color(content(0.95)).child(text)
}

fn notice(text: String, error: bool) -> gpui::Div {
    div()
        .px(px(10.))
        .py(px(7.))
        .rounded_md()
        .bg(content(0.05))
        .text_size(px(12.))
        .text_color(if error { rgb(0xe08a8a).into() } else { content(0.6) })
        .child(text)
}

/// The chart's legend: always shown; a click hides or shows that provider, colours stay put.
fn legend(page: &State, summary: &Summary, cx: &mut Context<Shell>) -> gpui::Div {
    div().flex().flex_wrap().gap_x(px(14.)).gap_y(px(4.)).children(summary.providers.iter().map(|(prov, _)| {
        let prov = *prov;
        let hidden = page.hidden[prov.index()];
        div()
            .id(("usage-legend", prov.index()))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(content(if hidden { 0.35 } else { 0.7 }))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.usage_page.hidden[prov.index()] = !this.usage_page.hidden[prov.index()];
                cx.notify();
            }))
            .child(div().size(px(9.)).rounded_sm().bg(prov.color()).opacity(if hidden { 0.35 } else { 1. }))
            .child(prov.name())
    }))
}

fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(T, String)],
    current: T,
    on: fn(&mut Shell, T, &mut Context<Shell>),
    cx: &mut Context<Shell>,
) -> gpui::Div {
    div().flex().p(px(2.)).gap(px(2.)).rounded_lg().bg(content(0.06)).children(options.iter().enumerate().map(
        |(ix, (value, label))| {
            let value = *value;
            let chosen = value == current;
            div()
                .id((id, ix))
                .h(px(24.))
                .px(px(10.))
                .rounded_md()
                .flex()
                .items_center()
                .text_size(px(12.))
                .text_color(content(if chosen { 1. } else { 0.6 }))
                .when(chosen, |d| d.bg(content(0.12)))
                .when(!chosen, |d| d.hover(|s| s.text_color(content(0.9))))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| on(this, value, cx)))
                .child(label.clone())
        },
    ))
}

fn faded(mut color: gpui::Hsla, alpha: f32) -> gpui::Hsla {
    color.a = alpha;
    color
}

#[allow(clippy::too_many_arguments)]
fn paint_area(
    bounds: Bounds<Pixels>,
    scale: &Scale,
    y_ticks: &[(f64, String)],
    x_ticks: &[(usize, String)],
    bands: &[(gpui::Hsla, Vec<f64>, Vec<f64>)],
    tips: &[(String, Vec<(gpui::Hsla, String, String)>)],
    n: usize,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    let axis = Label::mono(10., content(0.45));
    let (left, right, top, bottom) = (72., 8., 8., 22.);
    let (ox, oy) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
    let (w, h) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
    let (plot_w, plot_h) = ((w - left - right).max(1.), (h - top - bottom).max(1.));
    let x_of = |i: usize| ox + left + if n > 1 { plot_w * i as f32 / (n - 1) as f32 } else { plot_w / 2. };
    let y_of = |v: f64| oy + top + plot_h * scale.depth(v).clamp(0., 1.);
    let base = oy + top + plot_h;
    // A recessive grid and its labels.
    for (v, label) in y_ticks {
        let y = y_of(*v).round();
        window.paint_quad(gpui::fill(
            Bounds::new(gpui::point(px(ox + left), px(y)), gpui::size(px(plot_w), px(1.))),
            content(0.06),
        ));
        axis.paint(label, gpui::point(px(ox + left - 8.), px(y)), Align::Right, window, cx);
    }
    for (k, (i, label)) in x_ticks.iter().enumerate() {
        let align = match k {
            0 => Align::Left,
            2 => Align::Right,
            _ => Align::Center,
        };
        axis.paint(label, gpui::point(px(x_of(*i)), px(base + 12.)), align, window, cx);
    }
    // Each band is inset by 1 px on both edges, so neighbours show a 2 px gap of the surface.
    for (k, (color, low, high)) in bands.iter().enumerate() {
        let mut path = gpui::PathBuilder::fill();
        let upper = |i: usize| (y_of(high[i]) + 1.).min(if k == 0 { base } else { y_of(low[i]) - 1. });
        let lower = |i: usize| if k == 0 { base } else { (y_of(low[i]) - 1.).max(upper(i)) };
        path.move_to(gpui::point(px(x_of(0)), px(upper(0))));
        for i in 1..n {
            path.line_to(gpui::point(px(x_of(i)), px(upper(i))));
        }
        for i in (0..n).rev() {
            path.line_to(gpui::point(px(x_of(i)), px(lower(i))));
        }
        path.close();
        if let Ok(path) = path.build() {
            window.paint_path(path, faded(*color, 0.92));
        }
    }
    // The crosshair and its card, while the pointer is over the plot.
    let plot = Bounds::new(gpui::point(px(ox + left), px(oy + top)), gpui::size(px(plot_w), px(plot_h)));
    if let Some((x, _)) = pointer(plot, window)
        && n > 0
    {
        let i = if n > 1 { ((x / plot_w) * (n - 1) as f32).round() as usize } else { 0 }.min(n - 1);
        crosshair(gpui::point(px(0.), px(oy + top)), x_of(i), plot_h, window);
        if let Some((title, rows)) = tips.get(i) {
            let anchor = gpui::point(px(x_of(i)), px(oy + top + plot_h / 2.));
            readout(title, rows, anchor, bounds, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn reading(used: f64, resets_at: i64) -> Reading {
        Reading { used, resets_at, at: 0 }
    }

    const WEEK: i64 = 7 * DAY;

    #[test]
    fn days_left_counts_today_through_the_reset_day() {
        assert_eq!(days_left(day("2026-11-05"), at("2026-11-08 15:00:00")), 4);
        assert_eq!(days_left(day("2026-11-05"), at("2026-11-05 22:00:00")), 1);
        assert_eq!(days_left(day("2026-11-05"), at("2026-11-12 09:30:00")), 8);
    }

    #[test]
    fn a_reset_at_midnight_sharp_belongs_to_the_day_before() {
        assert_eq!(days_left(day("2026-11-05"), at("2026-11-06 00:00:00")), 1);
        assert_eq!(days_left(day("2026-11-05"), at("2026-11-06 00:00:01")), 2);
    }

    #[test]
    fn a_reset_already_past_leaves_one_day() {
        assert_eq!(days_left(day("2026-11-05"), at("2026-11-04 10:00:00")), 1);
    }

    #[test]
    fn pace_spends_the_rest_of_the_week_over_the_days_left() {
        // 30% used at the start of the day, 4 days left (today included): 70 / 4 = 17.5% a day.
        let now = 10 * DAY;
        let resets = now + 3 * DAY;
        let p = pace(38., Some(&reading(30., resets)), resets, None, now, 4);
        assert_eq!(p.start, 30.);
        assert_eq!(p.today, 8.);
        assert!((p.allowance - 17.5).abs() < 1e-9);
        assert!((p.fill - 8. / 17.5).abs() < 1e-9);
        assert!(!p.over);
    }

    #[test]
    fn a_full_week_with_nothing_used_allows_a_seventh() {
        let resets = WEEK;
        let p = pace(0., Some(&reading(0., resets)), resets, Some(10_080), 0, 7);
        assert!((p.allowance - 100. / 7.).abs() < 1e-9);
        assert_eq!(p.today, 0.);
        assert_eq!(p.fill, 0.);
        assert_eq!(p.elapsed, 0.);
        assert!(!p.ahead);
    }

    #[test]
    fn over_budget_turns_on_past_the_allowance() {
        let now = 5 * DAY;
        let resets = now + DAY;
        // 2 days left, 40% at the start: 30% allowed, 35% used today.
        let p = pace(75., Some(&reading(40., resets)), resets, None, now, 2);
        assert!((p.allowance - 30.).abs() < 1e-9);
        assert!(p.over);
        assert!(p.fill > 1.);
        // Exactly the allowance is not over.
        let p = pace(70., Some(&reading(40., resets)), resets, None, now, 2);
        assert!(!p.over);
    }

    #[test]
    fn a_week_that_rolled_over_today_starts_from_zero() {
        // The baseline came from last week's window (reset a week earlier): the new week began at 0.
        let now = 100 * DAY;
        let resets = now + 6 * DAY;
        let old = reading(92., resets - WEEK);
        let p = pace(5., Some(&old), resets, None, now, 7);
        assert_eq!(p.start, 0.);
        assert_eq!(p.today, 5.);
        assert!((p.allowance - 100. / 7.).abs() < 1e-9);
        // A reset time that only jittered by seconds is the same week.
        let same = reading(40., resets + 5_000);
        assert_eq!(pace(45., Some(&same), resets, None, now, 7).start, 40.);
    }

    #[test]
    fn without_a_stored_reading_the_current_one_is_the_start() {
        let p = pace(33., None, WEEK, None, 0, 7);
        assert_eq!(p.start, 33.);
        assert_eq!(p.today, 0.);
        assert_eq!(p.fill, 0.);
        assert!(!p.over);
    }

    #[test]
    fn a_limit_already_spent_at_the_start_of_the_day_has_no_allowance() {
        let resets = 2 * DAY;
        let p = pace(100., Some(&reading(100., resets)), resets, None, 0, 2);
        assert_eq!(p.allowance, 0.);
        assert_eq!(p.fill, 0.);
        assert!(!p.over);
    }

    #[test]
    fn the_even_pace_tick_follows_the_share_of_the_week_gone() {
        let resets = WEEK;
        let p = pace(60., None, resets, None, WEEK / 2, 4);
        assert!((p.elapsed - 0.5).abs() < 1e-9);
        assert!(p.ahead);
        let p = pace(40., None, resets, None, WEEK / 2, 4);
        assert!(!p.ahead);
        // After the reset time the week is whole, before its start empty.
        assert_eq!(pace(10., None, resets, None, WEEK + DAY, 1).elapsed, 1.);
        assert_eq!(pace(10., None, resets, None, -DAY, 7).elapsed, 0.);
    }

    #[test]
    fn readings_keep_only_the_first_of_each_day() {
        let mut readings = Readings::default();
        assert!(readings.record("claude/default", "2026-11-05", reading(10., 1)));
        assert!(!readings.record("claude/default", "2026-11-05", reading(20., 1)));
        assert_eq!(readings.baseline("claude/default", "2026-11-05").unwrap().used, 10.);
        assert!(readings.baseline("claude/default", "2026-11-06").is_none());
        assert!(readings.baseline("codex/default", "2026-11-05").is_none());
    }

    #[test]
    fn the_first_day_is_the_one_with_no_earlier_reading() {
        let mut readings = Readings::default();
        readings.record("claude/default", "2026-11-05", reading(10., 1));
        assert!(!readings.has_earlier("claude/default", "2026-11-05"));
        readings.record("claude/default", "2026-11-06", reading(14., 1));
        assert!(readings.has_earlier("claude/default", "2026-11-06"));
        assert!(!readings.has_earlier("codex/default", "2026-11-06"));
    }

    #[test]
    fn old_days_are_dropped_past_the_limit() {
        let mut readings = Readings::default();
        for d in 1..=KEEP_DAYS + 5 {
            readings.record("a/b", &format!("2026-10-{d:02}"), reading(1., 1));
        }
        let days = &readings.accounts["a/b"];
        assert_eq!(days.len(), KEEP_DAYS);
        assert!(!days.contains_key("2026-10-01"));
        assert!(days.contains_key(&format!("2026-10-{:02}", KEEP_DAYS + 5)));
    }

    #[test]
    fn readings_round_trip_through_json() {
        let mut readings = Readings::default();
        readings.record(
            "claude/default",
            "2026-11-05",
            Reading { used: 41.5, resets_at: 1_700_000_000_000, at: 1_699_999_000_000 },
        );
        readings.record("codex/acc-2", "2026-11-05", reading(7., 5));
        let text = readings.to_json();
        assert!(text.contains("resetsAt"));
        assert_eq!(Readings::from_json(&text), Some(readings));
        assert_eq!(Readings::from_json("not json"), None);
        assert_eq!(Readings::from_json("{}"), Some(Readings::default()));
    }

    #[test]
    fn readings_survive_a_write_and_a_read_from_disk() {
        let dir = std::env::temp_dir().join(format!("sirus-usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("r.json");
        let mut readings = Readings::default();
        readings.record("claude/default", "2026-11-05", reading(12., 99));
        std::fs::write(&file, readings.to_json()).unwrap();
        let back = Readings::from_json(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(back, readings);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn win(id: &str, used: Option<f64>, resets: Option<i64>, minutes: Option<i64>) -> Win {
        Win { id: id.to_owned(), used, resets_at: resets, minutes }
    }

    #[test]
    fn the_weekly_window_skips_buckets_and_reserve_models() {
        let list = vec![
            win("five_hour", Some(10.), Some(1), Some(300)),
            win("seven_day_opus", Some(50.), Some(1), Some(10_080)),
            win("base_model_inference/x", Some(60.), Some(1), Some(10_080)),
            win("seven_day", Some(30.), Some(1), None),
        ];
        assert_eq!(weekly(&list).unwrap().id, "seven_day");
        // Codex names its windows by duration alone.
        assert_eq!(weekly(&[win("codex/secondary", Some(5.), Some(1), Some(10_080))]).unwrap().id, "codex/secondary");
        // No weekly window, or one without a reading: no pace.
        assert!(weekly(&[win("five_hour", Some(5.), Some(1), Some(300))]).is_none());
        assert!(weekly(&[win("seven_day", None, Some(1), None)]).is_none());
        assert!(weekly(&[win("seven_day", Some(5.), None, None)]).is_none());
    }

    #[test]
    fn windows_accept_seconds_and_milliseconds() {
        let value = json!({ "windows": [
            { "id": "a", "usedPercent": 1.5, "resetsAt": 1_700_000_000, "durationMinutes": 300 },
            { "id": "b", "usedPercent": null, "resetsAt": 1_700_000_000_000i64, "durationMinutes": null },
        ] });
        let list = parse_windows(&value);
        assert_eq!(list[0].resets_at, Some(1_700_000_000_000));
        assert_eq!(list[1].resets_at, Some(1_700_000_000_000));
        assert_eq!(list[1].used, None);
    }

    fn bucket(at: i64, cost: [f64; 4]) -> Bucket {
        Bucket { at, cost, tokens: cost.map(|c| c * 1000.) }
    }

    #[test]
    fn the_series_is_cumulative_and_stacked_in_provider_order() {
        let buckets = vec![
            bucket(0, [1., 2., 0., 0.]),
            bucket(1, [0., 1., 0., 0.]),
            bucket(2, [3., 0., 0., 0.5]),
        ];
        let (layers, max) = stack(&buckets, Metric::Cost, [false; 4]);
        // OpenCode never used anything: no band.
        assert_eq!(layers.iter().map(|l| l.prov).collect::<Vec<_>>(), vec![Prov::Claude, Prov::Codex, Prov::Other]);
        assert_eq!(layers[0].bottom, vec![0., 0., 0.]);
        assert_eq!(layers[0].top, vec![1., 1., 4.]);
        assert_eq!(layers[1].bottom, vec![1., 1., 4.]);
        assert_eq!(layers[1].top, vec![3., 4., 7.]);
        assert_eq!(layers[2].bottom, vec![3., 4., 7.]);
        assert_eq!(layers[2].top, vec![3., 4., 7.5]);
        assert_eq!(max, 7.5);
    }

    #[test]
    fn a_hidden_provider_leaves_the_stack_and_the_others_keep_their_place() {
        let buckets = vec![bucket(0, [1., 2., 0., 0.]), bucket(1, [1., 2., 0., 0.])];
        let (layers, max) = stack(&buckets, Metric::Cost, [true, false, false, false]);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].prov, Prov::Codex);
        assert_eq!(layers[0].bottom, vec![0., 0.]);
        assert_eq!(layers[0].top, vec![2., 4.]);
        assert_eq!(max, 4.);
        assert_eq!(Prov::Codex.index(), 1);
    }

    #[test]
    fn tokens_stack_the_same_way_and_an_empty_period_has_no_bands() {
        let buckets = vec![bucket(0, [0.; 4]), bucket(1, [0.; 4])];
        let (layers, max) = stack(&buckets, Metric::Tokens, [false; 4]);
        assert!(layers.is_empty());
        assert_eq!(max, 0.);
        let (layers, max) =
            stack(&[bucket(0, [1., 0., 0., 0.]), bucket(1, [2., 0., 0., 0.])], Metric::Tokens, [false; 4]);
        assert_eq!(layers[0].top, vec![1000., 3000.]);
        assert_eq!(max, 3000.);
    }

    #[test]
    fn other_providers_fold_into_outros() {
        assert_eq!(Prov::of("claude"), Prov::Claude);
        assert_eq!(Prov::of("opencode"), Prov::OpenCode);
        assert_eq!(Prov::of("cursor"), Prov::Other);
        assert_eq!(Prov::of("grok"), Prov::Other);
    }

    fn entry(at: i64, provider: &str, model: &str, input: f64, output: f64) -> Entry {
        Entry {
            at,
            provider: provider.to_owned(),
            model: model.to_owned(),
            session: format!("{provider}-s"),
            input,
            cached: 0.,
            cache_write: 0.,
            output,
            reported: None,
            fast: false,
        }
    }

    #[test]
    fn summary_prices_tokens_and_buckets_the_last_24_hours() {
        let now: i64 = 1_700_000_000_000;
        let hour = now.div_euclid(HOUR) * HOUR;
        let entries = vec![
            entry(hour, "claude", "claude-sonnet-4-5", 1_000_000., 100_000.),
            entry(hour - HOUR, "codex", "gpt-x", 500., 500.),
            entry(now - 3 * DAY, "claude", "claude-sonnet-4-5", 9., 9.),
            Entry { reported: Some(0.25), ..entry(hour, "opencode", "oc", 10., 10.) },
            entry(hour, "cursor", "claude-haiku-4", 1_000_000., 0.),
        ];
        let s = summarize(&entries, 1, now, &HashMap::new());
        assert!(s.hourly && s.buckets.len() == 24);
        // Sonnet 4: 1M in at $3 plus 100k out at $15 = 4.5; OpenCode reported 0.25; Haiku 4: $1.
        assert!((s.cost - 5.75).abs() < 1e-9, "{}", s.cost);
        assert!(s.partial, "the codex model has no price");
        assert_eq!(s.buckets[23].cost[0], 4.5);
        assert_eq!(s.buckets[23].cost[3], 1.);
        assert_eq!(s.buckets[22].tokens[1], 1000.);
        assert_eq!(s.sessions, 4);
        assert_eq!(s.providers.len(), 4);
    }

    #[test]
    fn a_downloaded_price_beats_the_built_in_one_and_fast_costs_more() {
        let mut prices = HashMap::new();
        let base = Rates { input: 1., cache_read: 0.1, cache_write: 1.25, output: 2. };
        let fast = Rates { input: 2., cache_read: 0.2, cache_write: 2.5, output: 4. };
        prices.insert("claude-sonnet-4-5".to_owned(), Price { rates: base, fast: Some(fast) });
        let mut e = entry(0, "claude", "claude-sonnet-4-5", 1_000_000., 0.);
        assert_eq!(entry_cost(&e, &prices), Some(1.));
        e.fast = true;
        assert_eq!(entry_cost(&e, &prices), Some(2.));
        assert_eq!(entry_cost(&entry(0, "x", "unknown", 5., 5.), &prices), None);
        // The longest built-in prefix wins.
        assert_eq!(built_in_price("claude-opus-5-5-x").unwrap().input, 4.);
        assert_eq!(built_in_price("claude-opus-5-1").unwrap().input, 5.);
    }

    #[test]
    fn models_sort_by_cost_with_unpriced_last() {
        let row = |m: &str, cost: Option<f64>, tokens: f64| ModelRow {
            provider: Prov::Claude,
            model: m.to_owned(),
            cost,
            tokens,
        };
        let mut rows = vec![row("a", None, 900.), row("b", Some(1.), 10.), row("c", Some(5.), 5.)];
        sort_models(&mut rows, Metric::Cost);
        assert_eq!(rows.iter().map(|r| r.model.as_str()).collect::<String>(), "cba");
        sort_models(&mut rows, Metric::Tokens);
        assert_eq!(rows.iter().map(|r| r.model.as_str()).collect::<String>(), "abc");
    }

    #[test]
    fn numbers_read_in_both_languages() {
        assert_eq!(usd(1234.5, true), "$1,234.50");
        assert_eq!(usd(1234.5, false), "US$ 1.234,50");
        assert_eq!(usd(0.006, true), "$0.01");
        assert_eq!(count(999., true), "999");
        assert_eq!(count(1_234., true), "1.23K");
        assert_eq!(count(1_234., false), "1,23 mil");
        assert_eq!(count(2_000_000., false), "2 mi");
        assert_eq!(count(3_500_000_000., true), "3.5B");
        assert_eq!(share(1., 4., true), "25%");
        assert_eq!(share(1., 3., false), "33,3%");
        assert_eq!(share(1., 10_000., true), "<0.1%");
        assert_eq!(share(1., 0., true), "—");
        assert_eq!(percent(14.2857, false), "14,3");
        assert_eq!(percent(8., true), "8");
    }

    #[test]
    fn axis_ticks_read_in_both_languages() {
        // Tokens: the same words as the tooltip, with the step's decimals only.
        assert_eq!(axis_label(2_000., 1_000., false, false), "2 mil");
        assert_eq!(axis_label(1_500., 500., false, false), "1,5 mil");
        assert_eq!(axis_label(1_500_000., 500_000., false, false), "1,5 mi");
        assert_eq!(axis_label(3_000_000_000., 1e9, false, false), "3 bi");
        assert_eq!(axis_label(0., 0.5, false, false), "0");
        assert_eq!(axis_label(0.5, 0.5, false, false), "0,5");
        assert_eq!(axis_label(2_000., 1_000., false, true), "2K");
        assert_eq!(axis_label(1_500., 500., false, true), "1.5K");
        assert_eq!(axis_label(2_000_000., 1_000_000., false, true), "2M");
        assert_eq!(axis_label(3_000_000_000., 1e9, false, true), "3B");
        // Cost: the currency prefix of the tooltip, "US$ " in Portuguese and "$" in English.
        assert_eq!(axis_label(2_000., 1_000., true, false), "US$ 2 mil");
        assert_eq!(axis_label(1_500., 500., true, false), "US$ 1,5 mil");
        assert_eq!(axis_label(0.5, 0.5, true, false), "US$ 0,5");
        assert_eq!(axis_label(2_000., 1_000., true, true), "$2K");
        assert_eq!(axis_label(0.25, 0.25, true, true), "$0.25");
    }
}
