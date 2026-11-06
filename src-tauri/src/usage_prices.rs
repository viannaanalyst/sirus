//! Model prices for the Usage page (ADR-141): LiteLLM's public price table, downloaded at
//! most once a day and kept next to the state as a trimmed copy. Only the table is fetched;
//! nothing about the person's usage leaves the machine. The page asks for the models it
//! shows and gets USD per million tokens back; a failed download keeps the last copy.

use crate::commands::AppState;
use crate::error::Result;
use crate::runtime::State;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SOURCE: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const FILE_NAME: &str = "model-prices.json";
const MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;
/// After a failed download, wait this long before trying again.
const RETRY_MS: i64 = 15 * 60 * 1000;
const MAX_BODY: usize = 16 << 20;
const MAX_MODELS: usize = 500;
/// The on-disk table's format; a copy written by an older format is downloaded again.
/// 2: fast (priority) prices.
const TABLE_VERSION: u32 = 2;

/// USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrice {
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
    /// Prices in the fast (priority) service tier, when the table has them.
    #[serde(default)]
    pub fast: Option<FastPrice>,
}

/// USD per million tokens in the fast (priority) service tier.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FastPrice {
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePrices {
    /// Prices for the requested models that the table knows, by the requested name.
    pub prices: BTreeMap<String, ModelPrice>,
    /// When the table in use was downloaded (Unix ms); null without any copy.
    pub fetched_at: Option<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Table {
    /// Missing in copies written before versions existed.
    #[serde(default)]
    version: u32,
    fetched_at: i64,
    rates: HashMap<String, ModelPrice>,
}

#[derive(Default)]
struct Loaded {
    table: Option<Arc<Table>>,
    failed_at: i64,
}

fn loaded() -> &'static tokio::sync::Mutex<Loaded> {
    static LOADED: OnceLock<tokio::sync::Mutex<Loaded>> = OnceLock::new();
    LOADED.get_or_init(Default::default)
}

pub async fn usage_prices(
    state: State<'_, Arc<AppState>>,
    models: Vec<String>,
) -> Result<UsagePrices> {
    state.ensure_running()?;
    Ok(prices_for(&state, &models).await)
}

/// The table's prices for the models it knows, by the requested name.
pub(crate) async fn prices_for(state: &AppState, models: &[String]) -> UsagePrices {
    let dir = state
        .data_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| state.data_path.clone());
    let table = current_table(&dir.join(FILE_NAME)).await;
    let prices = models
        .iter()
        .take(MAX_MODELS)
        .filter_map(|model| {
            let table = table.as_ref()?;
            lookup(&table.rates, model).map(|price| (model.clone(), price))
        })
        .collect();
    UsagePrices {
        prices,
        fetched_at: table.map(|table| table.fetched_at),
    }
}

/// The copy in memory, else on disk; downloaded again when older than a day.
async fn current_table(path: &Path) -> Option<Arc<Table>> {
    let mut loaded = loaded().lock().await;
    if loaded.table.is_none() {
        let path = path.to_path_buf();
        loaded.table = tokio::task::spawn_blocking(move || read_table(&path))
            .await
            .ok()
            .flatten()
            .map(Arc::new);
    }
    let now = now_ms();
    let fresh = loaded
        .table
        .as_ref()
        .is_some_and(|table| is_fresh(table, now));
    if !fresh && now - loaded.failed_at >= RETRY_MS {
        match download().await {
            Some(rates) => {
                let table = Arc::new(Table {
                    version: TABLE_VERSION,
                    fetched_at: now,
                    rates,
                });
                write_table(path.to_path_buf(), Arc::clone(&table));
                loaded.table = Some(table);
            }
            None => loaded.failed_at = now,
        }
    }
    loaded.table.clone()
}

/// Downloaded less than a day ago in the current format.
fn is_fresh(table: &Table, now: i64) -> bool {
    table.version == TABLE_VERSION && now - table.fetched_at < MAX_AGE_MS
}

fn read_table(path: &Path) -> Option<Table> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_table(path: PathBuf, table: Arc<Table>) {
    tokio::task::spawn_blocking(move || {
        static WRITING: Mutex<()> = Mutex::new(());
        let _guard = WRITING.lock();
        let Ok(bytes) = serde_json::to_vec(&*table) else {
            return;
        };
        let temp = path.with_extension("json.tmp");
        if std::fs::write(&temp, bytes).is_ok() {
            let _ = std::fs::rename(&temp, &path);
        }
    });
}

async fn download() -> Option<HashMap<String, ModelPrice>> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(20))
        .user_agent("Sirus Code")
        .build()
        .ok()?;
    let response = client
        .get(SOURCE)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    if response
        .content_length()
        .is_some_and(|length| length as usize > MAX_BODY)
    {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    if bytes.len() > MAX_BODY {
        return None;
    }
    let rates = parse(&serde_json::from_slice(&bytes).ok()?);
    (!rates.is_empty()).then_some(rates)
}

/// Every entry with both an input and an output price, by lower-case name; a name after a
/// provider prefix (`openai/gpt-6-sol`) is also kept bare when it is unambiguous. A missing
/// cache price falls back to the input price, so cached tokens are never counted as free.
/// Fast prices come from the `_priority` fields, else from a `provider_specific_entry.fast`
/// multiplier of the standard prices.
pub(crate) fn parse(document: &Value) -> HashMap<String, ModelPrice> {
    let Some(entries) = document.as_object() else {
        return HashMap::new();
    };
    let number = |entry: &Value, key: &str| {
        entry
            .get(key)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
    };
    let mut rates = HashMap::new();
    for (name, entry) in entries {
        let (Some(input), Some(output)) = (
            number(entry, "input_cost_per_token"),
            number(entry, "output_cost_per_token"),
        ) else {
            continue;
        };
        let key = name.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        let million = 1_000_000.0;
        let cache_read = number(entry, "cache_read_input_token_cost").unwrap_or(input);
        let cache_write = number(entry, "cache_creation_input_token_cost").unwrap_or(input);
        let priority = match (
            number(entry, "input_cost_per_token_priority"),
            number(entry, "output_cost_per_token_priority"),
        ) {
            (Some(fast_input), Some(fast_output)) => {
                // A missing cache price scales with the input price.
                let ratio = if input > 0.0 {
                    fast_input / input
                } else if output > 0.0 {
                    fast_output / output
                } else {
                    1.0
                };
                Some(FastPrice {
                    input: fast_input * million,
                    cache_read: number(entry, "cache_read_input_token_cost_priority")
                        .unwrap_or(cache_read * ratio)
                        * million,
                    cache_write: number(entry, "cache_creation_input_token_cost_priority")
                        .unwrap_or(cache_write * ratio)
                        * million,
                    output: fast_output * million,
                })
            }
            _ => None,
        };
        let fast = priority.or_else(|| {
            let factor = number(&entry["provider_specific_entry"], "fast").filter(|f| *f > 0.0)?;
            Some(FastPrice {
                input: input * factor * million,
                cache_read: cache_read * factor * million,
                cache_write: cache_write * factor * million,
                output: output * factor * million,
            })
        });
        rates.insert(
            key,
            ModelPrice {
                input: input * million,
                cache_read: cache_read * million,
                cache_write: cache_write * million,
                output: output * million,
                fast,
            },
        );
    }
    let mut aliases: HashMap<String, Option<ModelPrice>> = HashMap::new();
    for (key, price) in &rates {
        let Some((_, bare)) = key.rsplit_once('/') else {
            continue;
        };
        if bare.is_empty() || rates.contains_key(bare) {
            continue;
        }
        aliases
            .entry(bare.to_owned())
            .and_modify(|held| {
                if held.is_some_and(|held| held != *price) {
                    *held = None;
                }
            })
            .or_insert(Some(*price));
    }
    rates.extend(
        aliases
            .into_iter()
            .filter_map(|(bare, price)| price.map(|price| (bare, price))),
    );
    rates
}

/// Lower-case, without a bracketed tier (`claude-fable-5-1[1m]`); bare family names and
/// locally generated messages have no price.
fn lookup(rates: &HashMap<String, ModelPrice>, model: &str) -> Option<ModelPrice> {
    let key = model.trim().to_lowercase();
    let key = key.split('[').next().unwrap_or_default();
    let bare = key.rsplit('/').next().unwrap_or_default();
    if matches!(
        bare,
        "" | "<synthetic>" | "synthetic" | "opus" | "sonnet" | "haiku" | "fable" | "unknown"
    ) {
        return None;
    }
    rates.get(key).or_else(|| rates.get(bare)).copied()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_keeps_priced_entries_bare_aliases_and_cache_fallbacks() {
        let rates = parse(&json!({
            "sample_spec": { "input_cost_per_token": "x" },
            "GPT-6-Sol": { "input_cost_per_token": 2e-6, "output_cost_per_token": 1e-5, "cache_read_input_token_cost": 2e-7 },
            "openai/gpt-6-luna": { "input_cost_per_token": 1e-7, "output_cost_per_token": 5e-7 },
            "a/twin": { "input_cost_per_token": 1e-6, "output_cost_per_token": 1e-6 },
            "b/twin": { "input_cost_per_token": 2e-6, "output_cost_per_token": 1e-6 },
            "no-output": { "input_cost_per_token": 1e-6 }
        }));
        let sol = rates["gpt-6-sol"];
        assert!((sol.input - 2.0).abs() < 1e-9 && (sol.cache_read - 0.2).abs() < 1e-9);
        assert!(
            (sol.cache_write - 2.0).abs() < 1e-9,
            "no cache write price: input price"
        );
        assert!(rates.contains_key("gpt-6-luna"), "unambiguous bare alias");
        assert!(!rates.contains_key("twin"), "ambiguous bare alias dropped");
        assert!(!rates.contains_key("no-output") && !rates.contains_key("sample_spec"));
        assert!(sol.fast.is_none(), "no fast prices without priority fields");
    }

    #[test]
    fn parse_reads_priority_prices_and_fast_multipliers() {
        let rates = parse(&json!({
            "gpt-6.1-sol": {
                "input_cost_per_token": 2e-6, "output_cost_per_token": 1e-5,
                "cache_read_input_token_cost": 1e-7, "cache_creation_input_token_cost": 2.5e-6,
                "input_cost_per_token_priority": 4e-6, "output_cost_per_token_priority": 2e-5,
                "cache_read_input_token_cost_priority": 2e-7
            },
            "claude-opus-5-5": {
                "input_cost_per_token": 4e-6, "output_cost_per_token": 2e-5,
                "cache_read_input_token_cost": 2e-7, "cache_creation_input_token_cost": 5e-6,
                "provider_specific_entry": { "us": 1.1, "fast": 2 }
            },
            "half-priority": {
                "input_cost_per_token": 1e-6, "output_cost_per_token": 1e-6,
                "input_cost_per_token_priority": 2e-6
            }
        }));
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let sol = rates["gpt-6.1-sol"].fast.unwrap();
        assert!(close(sol.input, 4.0) && close(sol.output, 20.0) && close(sol.cache_read, 0.2));
        assert!(
            close(sol.cache_write, 5.0),
            "missing cache price scales by the input ratio"
        );
        let opus = rates["claude-opus-5-5"].fast.unwrap();
        assert!(close(opus.input, 8.0) && close(opus.cache_read, 0.4));
        assert!(close(opus.cache_write, 10.0) && close(opus.output, 40.0));
        assert!(
            rates["half-priority"].fast.is_none(),
            "no output priority price"
        );
    }

    #[test]
    fn lookup_strips_tiers_and_refuses_bare_families() {
        let rates = parse(&json!({
            "claude-fable-5-1": { "input_cost_per_token": 1e-5, "output_cost_per_token": 5e-5 }
        }));
        assert!(lookup(&rates, "Claude-Fable-5-1[1m]").is_some());
        assert!(lookup(&rates, "anthropic/claude-fable-5-1").is_some());
        assert!(lookup(&rates, "fable").is_none());
        assert!(lookup(&rates, "<synthetic>").is_none());
    }

    #[test]
    fn table_round_trips_on_disk() {
        let dir = std::env::temp_dir().join(format!("sirus-prices-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        let mut rates = HashMap::new();
        rates.insert(
            "m".to_owned(),
            ModelPrice {
                input: 1.0,
                cache_read: 0.1,
                cache_write: 1.25,
                output: 5.0,
                fast: Some(FastPrice {
                    input: 2.0,
                    cache_read: 0.2,
                    cache_write: 2.5,
                    output: 10.0,
                }),
            },
        );
        let table = Arc::new(Table {
            version: TABLE_VERSION,
            fetched_at: 42,
            rates,
        });
        std::fs::write(&path, serde_json::to_vec(&*table).unwrap()).unwrap();
        let read = read_table(&path).unwrap();
        assert_eq!(read.fetched_at, 42);
        assert_eq!(read.version, TABLE_VERSION);
        assert_eq!(read.rates["m"], table.rates["m"]);
        // A copy from before versions still reads (as a fallback) but is stale.
        std::fs::write(
            &path,
            r#"{"fetchedAt":42,"rates":{"m":{"input":1,"cacheRead":0.1,"cacheWrite":1.25,"output":5}}}"#,
        )
        .unwrap();
        let old = read_table(&path).unwrap();
        assert_eq!(old.version, 0);
        assert!(old.rates["m"].fast.is_none());
        assert!(!is_fresh(&old, 43), "older format: downloaded again");
        assert!(is_fresh(&read, 43) && !is_fresh(&read, 42 + MAX_AGE_MS));
        let _ = std::fs::remove_dir_all(dir);
    }
}
