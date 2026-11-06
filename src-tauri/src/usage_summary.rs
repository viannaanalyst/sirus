//! Usage summary for the phone (and the accent it follows): the Mac's Usage page numbers in a
//! few KB. The pricing and bucketing mirror `native-spike/src/usage_page.rs` (`built_in_price`,
//! `entry_cost`, `summarize`) so both screens show the same figures; the tests below carry
//! that file's own fixtures. Buckets are local-time hours when `days` is 1, local days
//! otherwise. `phone_look_*` keep the accent the window resolved (Appearance, custom colour
//! included) so the phone can wear the same colour.

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::AgentProviderId;
use crate::runtime::State;
use crate::usage_history::UsageHistoryEntry;
use crate::usage_prices::ModelPrice;
use chrono::{Local, NaiveDate, TimeZone};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const HOUR_MS: i64 = 3_600_000;
#[cfg(test)]
const DAY_MS: i64 = 24 * HOUR_MS;
const MAX_DAYS: i64 = 90;
/// Models listed; the totals still count every one.
const MAX_MODELS: usize = 30;
const DEFAULT_ACCENT: &str = "#bdb4f2";
const LOOK_FILE: &str = "phone-look.json";

/// USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rates {
    input: f64,
    cache_read: f64,
    cache_write: f64,
    output: f64,
}

fn list_price(input: f64, output: f64, cache_read: Option<f64>) -> Rates {
    Rates {
        input,
        cache_read: cache_read.unwrap_or(input / 10.),
        cache_write: input * 1.25,
        output,
    }
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

fn rates_of(price: &ModelPrice, fast: bool) -> Rates {
    match (fast, price.fast) {
        (true, Some(f)) => Rates {
            input: f.input,
            cache_read: f.cache_read,
            cache_write: f.cache_write,
            output: f.output,
        },
        _ => Rates {
            input: price.input,
            cache_read: price.cache_read,
            cache_write: price.cache_write,
            output: price.output,
        },
    }
}

/// Estimated cost of one entry; none when the model has no price and the provider logged none.
fn entry_cost(entry: &UsageHistoryEntry, prices: &HashMap<String, ModelPrice>) -> Option<f64> {
    if let Some(cost) = entry.reported_cost_usd {
        return Some(cost);
    }
    let fast = entry.speed == crate::usage_history::Speed::Fast;
    let r = match prices.get(&entry.model) {
        Some(price) => rates_of(price, fast),
        None => built_in_price(&entry.model)?,
    };
    Some(
        (entry.input_tokens as f64 * r.input
            + entry.cached_input_tokens as f64 * r.cache_read
            + entry.cache_write_tokens as f64 * r.cache_write
            + entry.output_tokens as f64 * r.output)
            / 1_000_000.,
    )
}

fn tokens_of(entry: &UsageHistoryEntry) -> f64 {
    (entry.input_tokens
        + entry.cached_input_tokens
        + entry.cache_write_tokens
        + entry.output_tokens) as f64
}

/// 0 Claude, 1 Codex, 2 OpenCode, 3 everything else (the Usage page's order).
fn slot(provider: &AgentProviderId) -> usize {
    match provider {
        AgentProviderId::Claude => 0,
        AgentProviderId::Codex => 1,
        AgentProviderId::OpenCode => 2,
        _ => 3,
    }
}

const SLOT_NAMES: [&str; 4] = ["claude", "codex", "opencode", "other"];

fn date_start(date: NaiveDate) -> i64 {
    let midnight = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    Local.from_local_datetime(&midnight).earliest().map_or_else(
        || midnight.and_utc().timestamp_millis(),
        |d| d.timestamp_millis(),
    )
}

fn local_date(ms: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map_or_else(|| chrono::Utc::now().date_naive(), |d| d.date_naive())
}

/// Bucket starts covering the range, oldest first: 24 hours, or `days` local days ending today.
fn bucket_starts(days: i64, now: i64) -> Vec<i64> {
    if days <= 1 {
        let last = now.div_euclid(HOUR_MS) * HOUR_MS;
        return (0..24).map(|i| last - (23 - i) * HOUR_MS).collect();
    }
    let today = local_date(now);
    (0..days)
        .map(|i| date_start(today - chrono::Duration::days(days - 1 - i)))
        .collect()
}

/// Where the period just before `starts` begins: the same number of hours or local days.
fn previous_start(starts: &[i64], days: i64, now: i64) -> i64 {
    if days <= 1 {
        return starts[0] - 24 * HOUR_MS;
    }
    date_start(local_date(now) - chrono::Duration::days(2 * days - 1))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Split {
    pub claude: f64,
    pub codex: f64,
    pub opencode: f64,
    pub other: f64,
}

impl Split {
    fn of(values: [f64; 4], round: bool) -> Self {
        let v = |i: usize| if round { cents(values[i]) } else { values[i] };
        Split {
            claude: v(0),
            codex: v(1),
            opencode: v(2),
            other: v(3),
        }
    }
}

/// Costs keep six decimals: enough for any price, and the reply stays small.
fn cents(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SummaryBucket {
    pub at: i64,
    pub costs: Split,
    pub tokens: Split,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SummaryModel {
    pub model: String,
    pub provider: &'static str,
    pub cost: f64,
    pub tokens: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub buckets: Vec<SummaryBucket>,
    pub models: Vec<SummaryModel>,
    pub total_cost: f64,
    pub total_tokens: f64,
    pub previous_cost: f64,
}

fn summarize(
    entries: &[UsageHistoryEntry],
    days: i64,
    now: i64,
    prices: &HashMap<String, ModelPrice>,
) -> UsageSummary {
    let days = days.clamp(1, MAX_DAYS);
    let hourly = days <= 1;
    let starts = bucket_starts(days, now);
    let from = starts[0];
    let before = previous_start(&starts, days, now);
    let index: HashMap<i64, usize> = starts.iter().enumerate().map(|(i, s)| (*s, i)).collect();
    let mut cost = vec![[0.; 4]; starts.len()];
    let mut tokens = vec![[0.; 4]; starts.len()];
    let mut total_cost = 0.;
    let mut total_tokens = 0.;
    let mut previous_cost = 0.;
    let mut models: HashMap<(usize, &str), (Option<f64>, f64)> = HashMap::new();
    for entry in entries {
        if entry.at >= before && entry.at < from {
            previous_cost += entry_cost(entry, prices).unwrap_or(0.);
            continue;
        }
        if entry.at < from || entry.at > now {
            continue;
        }
        let p = slot(&entry.provider);
        let amount = tokens_of(entry);
        let priced = entry_cost(entry, prices);
        let value = priced.unwrap_or(0.);
        total_cost += value;
        total_tokens += amount;
        let row = models.entry((p, entry.model.as_str())).or_default();
        row.1 += amount;
        if let Some(c) = priced {
            row.0 = Some(row.0.unwrap_or(0.) + c);
        }
        let start = if hourly {
            entry.at.div_euclid(HOUR_MS) * HOUR_MS
        } else {
            date_start(local_date(entry.at))
        };
        if let Some(at) = index.get(&start) {
            cost[*at][p] += value;
            tokens[*at][p] += amount;
        }
    }
    let mut rows: Vec<(usize, &str, Option<f64>, f64)> = models
        .into_iter()
        .filter(|(_, (_, amount))| *amount > 0.)
        .map(|((p, model), (c, amount))| (p, model, c, amount))
        .collect();
    // The Usage page's order: by cost (unpriced last, by tokens).
    rows.sort_by(|a, b| {
        b.2.unwrap_or(-1.)
            .total_cmp(&a.2.unwrap_or(-1.))
            .then(b.3.total_cmp(&a.3))
            .then(a.1.cmp(b.1))
    });
    UsageSummary {
        buckets: starts
            .iter()
            .enumerate()
            .map(|(i, at)| SummaryBucket {
                at: *at,
                costs: Split::of(cost[i], true),
                tokens: Split::of(tokens[i], false),
            })
            .collect(),
        models: rows
            .into_iter()
            .take(MAX_MODELS)
            .map(|(p, model, c, amount)| SummaryModel {
                model: model.to_owned(),
                provider: SLOT_NAMES[p],
                cost: cents(c.unwrap_or(0.)),
                tokens: amount,
            })
            .collect(),
        total_cost: cents(total_cost),
        total_tokens,
        previous_cost: cents(previous_cost),
    }
}

pub async fn usage_summary(state: State<'_, Arc<AppState>>, days: i64) -> Result<UsageSummary> {
    state.ensure_running()?;
    let days = days.clamp(1, MAX_DAYS);
    // The period before needs as much history again.
    let history = crate::usage_history::read_days(&state, days * 2 + 1).await?;
    let mut names: Vec<String> = history.entries.iter().map(|e| e.model.clone()).collect();
    names.sort_unstable();
    names.dedup();
    let prices = crate::usage_prices::prices_for(&state, &names).await.prices;
    let prices: HashMap<String, ModelPrice> = prices.into_iter().collect();
    let now = history.generated_at;
    Ok(summarize(&history.entries, days, now, &prices))
}

fn look_path(state: &AppState) -> PathBuf {
    state
        .data_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| state.data_path.clone())
        .join(LOOK_FILE)
}

/// `#rrggbb`, lower-cased; none for anything else.
fn clean_accent(text: &str) -> Option<String> {
    let text = text.trim();
    let hex = text.strip_prefix('#')?;
    (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_lowercase()))
}

fn stored_accent(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| clean_accent(value["accent"].as_str()?))
        .unwrap_or_else(|| DEFAULT_ACCENT.to_owned())
}

pub fn phone_look_get(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value> {
    Ok(serde_json::json!({ "accent": stored_accent(&look_path(&state)) }))
}

pub fn phone_look_set(
    state: State<'_, Arc<AppState>>,
    accent: String,
) -> Result<serde_json::Value> {
    let accent = clean_accent(&accent)
        .ok_or_else(|| Error::new("invalid", "The accent must look like #rrggbb."))?;
    let path = look_path(&state);
    std::fs::write(&path, serde_json::json!({ "accent": accent }).to_string())
        .map_err(|e| Error::new("io", e.to_string()))?;
    Ok(serde_json::json!({ "accent": accent }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_history::Speed;
    use crate::usage_prices::FastPrice;

    fn entry(
        at: i64,
        provider: AgentProviderId,
        model: &str,
        input: u64,
        output: u64,
    ) -> UsageHistoryEntry {
        UsageHistoryEntry {
            at,
            session: format!("{provider:?}-s"),
            provider,
            model: model.to_owned(),
            input_tokens: input,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: output,
            reported_cost_usd: None,
            speed: Speed::Standard,
        }
    }

    // The Mac's own test (`summary_prices_tokens_and_buckets_the_last_24_hours`), same fixture
    // and same expected figures.
    #[test]
    fn it_matches_the_usage_page_on_its_fixture() {
        use AgentProviderId::*;
        let now: i64 = 1_700_000_000_000;
        let hour = now.div_euclid(HOUR_MS) * HOUR_MS;
        let entries = vec![
            entry(hour, Claude, "claude-sonnet-4-5", 1_000_000, 100_000),
            entry(hour - HOUR_MS, Codex, "gpt-x", 500, 500),
            entry(now - 3 * DAY_MS, Claude, "claude-sonnet-4-5", 9, 9),
            UsageHistoryEntry {
                reported_cost_usd: Some(0.25),
                ..entry(hour, OpenCode, "oc", 10, 10)
            },
            entry(hour, Cursor, "claude-haiku-4", 1_000_000, 0),
        ];
        let s = summarize(&entries, 1, now, &HashMap::new());
        assert_eq!(s.buckets.len(), 24);
        assert!((s.total_cost - 5.75).abs() < 1e-9, "{}", s.total_cost);
        assert_eq!(s.buckets[23].costs.claude, 4.5);
        assert_eq!(s.buckets[23].costs.other, 1.);
        assert_eq!(s.buckets[22].tokens.codex, 1000.);
        assert_eq!(s.buckets[23].at, hour);
        // The Mac's rows: by cost, the unpriced Codex model last.
        let order: Vec<&str> = s.models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(
            order,
            ["claude-sonnet-4-5", "claude-haiku-4", "oc", "gpt-x"]
        );
        assert_eq!(s.models[3].cost, 0.);
        assert_eq!(s.models[1].provider, "other");
        // The entry three days back is before this 24 hours and before the previous 24 too.
        assert_eq!(s.previous_cost, 0.);
        assert_eq!(s.total_tokens, 1_100_000. + 1_000. + 20. + 1_000_000.);
    }

    // The Mac's `a_downloaded_price_beats_the_built_in_one_and_fast_costs_more`.
    #[test]
    fn prices_follow_the_usage_page_rules() {
        let mut prices = HashMap::new();
        prices.insert(
            "claude-sonnet-4-5".to_owned(),
            ModelPrice {
                input: 1.,
                cache_read: 0.1,
                cache_write: 1.25,
                output: 2.,
                fast: Some(FastPrice {
                    input: 2.,
                    cache_read: 0.2,
                    cache_write: 2.5,
                    output: 4.,
                }),
            },
        );
        let mut e = entry(
            0,
            AgentProviderId::Claude,
            "claude-sonnet-4-5",
            1_000_000,
            0,
        );
        assert_eq!(entry_cost(&e, &prices), Some(1.));
        e.speed = Speed::Fast;
        assert_eq!(entry_cost(&e, &prices), Some(2.));
        let unknown = entry(0, AgentProviderId::Codex, "unknown", 5, 5);
        assert_eq!(entry_cost(&unknown, &prices), None);
        assert_eq!(built_in_price("claude-opus-5-5-x").unwrap().input, 4.);
        assert_eq!(built_in_price("claude-opus-5-1").unwrap().input, 5.);
        assert_eq!(built_in_price("claude-fable-5-1").unwrap().cache_read, 0.25);
        assert_eq!(
            built_in_price("claude-sonnet-4-x").unwrap().cache_write,
            3.75
        );
    }

    #[test]
    fn the_previous_period_is_the_same_length_just_before() {
        use AgentProviderId::Claude;
        let now: i64 = 1_700_000_000_000;
        let hour = now.div_euclid(HOUR_MS) * HOUR_MS;
        // $1 now; $2 in the 24 hours before the 24 hours; $4 older than that.
        let entries = vec![
            entry(hour, Claude, "claude-sonnet-4-5", 0, 66_667),
            entry(hour - 30 * HOUR_MS, Claude, "claude-sonnet-4-5", 0, 133_334),
            entry(hour - 60 * HOUR_MS, Claude, "claude-sonnet-4-5", 0, 266_667),
        ];
        let day = summarize(&entries, 1, now, &HashMap::new());
        assert!((day.total_cost - 1.).abs() < 1e-3);
        assert!(
            (day.previous_cost - 2.).abs() < 1e-3,
            "{}",
            day.previous_cost
        );
        // Seven local days: the entries 30 h and 60 h back fall in this week, nothing before it.
        let week = summarize(&entries, 7, now, &HashMap::new());
        assert_eq!(week.buckets.len(), 7);
        assert!((week.total_cost - 7.).abs() < 1e-2);
        assert_eq!(week.previous_cost, 0.);
        // Old history lands in the period before.
        let old = vec![entry(
            now - 10 * DAY_MS,
            Claude,
            "claude-sonnet-4-5",
            0,
            66_667,
        )];
        let week = summarize(&old, 7, now, &HashMap::new());
        assert_eq!(week.total_cost, 0.);
        assert!(
            (week.previous_cost - 1.).abs() < 1e-3,
            "{}",
            week.previous_cost
        );
    }

    #[test]
    fn daily_buckets_start_at_local_midnight_and_the_reply_stays_small() {
        use AgentProviderId::Claude;
        let now: i64 = 1_700_000_000_000;
        let entries = vec![entry(now - HOUR_MS, Claude, "claude-sonnet-4-5", 10, 10)];
        let s = summarize(&entries, 90, now, &HashMap::new());
        assert_eq!(s.buckets.len(), 90);
        for b in &s.buckets {
            let local = Local.timestamp_millis_opt(b.at).unwrap();
            assert_eq!(local.format("%H:%M:%S").to_string(), "00:00:00");
        }
        assert!(s.buckets.windows(2).all(|w| w[0].at < w[1].at));
        assert!(serde_json::to_string(&s).unwrap().len() < 20_000);
        let value = serde_json::to_value(&s).unwrap();
        assert!(value["buckets"][0]["costs"]["claude"].is_number());
        assert!(value["totalCost"].is_number() && value["previousCost"].is_number());
    }

    #[test]
    fn the_model_list_is_capped_and_sorted_by_cost() {
        use AgentProviderId::Claude;
        let now: i64 = 1_700_000_000_000;
        let hour = now.div_euclid(HOUR_MS) * HOUR_MS;
        let entries: Vec<_> = (0..40)
            .map(|i| {
                entry(
                    hour,
                    Claude,
                    &format!("claude-sonnet-4-{i}"),
                    1000 * (i + 1),
                    0,
                )
            })
            .collect();
        let s = summarize(&entries, 1, now, &HashMap::new());
        assert_eq!(s.models.len(), MAX_MODELS);
        assert!(s.models.windows(2).all(|w| w[0].cost >= w[1].cost));
        // The total still counts every model.
        assert!(s.total_cost > s.models.iter().map(|m| m.cost).sum::<f64>());
    }

    #[test]
    fn the_accent_is_kept_in_the_data_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOOK_FILE);
        assert_eq!(stored_accent(&path), "#bdb4f2");
        std::fs::write(&path, r##"{"accent":"#112233"}"##).unwrap();
        assert_eq!(stored_accent(&path), "#112233");
        std::fs::write(&path, r#"{"accent":"red"}"#).unwrap();
        assert_eq!(stored_accent(&path), "#bdb4f2");
        assert_eq!(clean_accent(" #ABCDEF "), Some("#abcdef".to_owned()));
        for bad in ["abcdef", "#abc", "#12345g", "#1234567", ""] {
            assert_eq!(clean_accent(bad), None, "{bad}");
        }
    }
}
