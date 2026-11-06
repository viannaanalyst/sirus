//! Usage page history (ADR-141): tokens per local hour, provider, model and session over the
//! past 1–90 days, read from the logs the provider CLIs already keep on this computer. Sirus
//! Code records nothing itself. Claude Code and Codex write JSON lines per session; OpenCode
//! keeps a SQLite database (older versions: one JSON file per message). Parsed files stay in
//! a process-wide cache keyed by path, length and modification time, and logs that only grew
//! are read from where the last pass stopped, so reopening the page reads only what changed.
//! Paths never leave this module: sessions are reported as short hashes.

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::AgentProviderId;
use crate::runtime::State;
use chrono::{Offset, TimeZone};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;
const MAX_DAYS: i64 = 90;
/// Files kept parsed in memory; the least recently modified go first beyond this.
const MAX_CACHED_FILES: usize = 20_000;
/// OpenCode rows larger than this are user messages carrying attachments, never usage.
const OPENCODE_MAX_ROW: i64 = 1 << 20;
const OPENCODE_MAX_FILE: u64 = 1 << 20;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistoryEntry {
    /// Start of the local-time hour bucket, Unix ms.
    pub at: i64,
    pub provider: AgentProviderId,
    pub model: String,
    /// Opaque per-provider session key (a hash, never a path).
    pub session: String,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub reported_cost_usd: Option<f64>,
    /// The service tier the responses ran in; fast (priority) tiers cost more.
    pub speed: Speed,
}

/// "fast": Codex's priority tier (and faster ones) or Claude Code's fast mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Speed {
    #[default]
    Standard,
    Fast,
}

impl Speed {
    /// Codex's `service_tier`: priority, fast and ultrafast are the paid fast tiers.
    fn codex(tier: &str) -> Self {
        match tier.to_ascii_lowercase().as_str() {
            "priority" | "fast" | "ultrafast" => Speed::Fast,
            _ => Speed::Standard,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistoryProblem {
    pub provider: AgentProviderId,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageHistory {
    pub entries: Vec<UsageHistoryEntry>,
    pub problems: Vec<UsageHistoryProblem>,
    pub generated_at: i64,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn usage_history(state: State<'_, Arc<AppState>>, days: i64) -> Result<UsageHistory> {
    state.ensure_running()?;
    read_days(&state, days.clamp(1, MAX_DAYS)).await
}

/// The history of the last `days` days (the caller keeps it within bounds).
pub(crate) async fn read_days(state: &AppState, days: i64) -> Result<UsageHistory> {
    let sources = Sources::for_app(state);
    tokio::task::spawn_blocking(move || {
        let now = now_ms();
        let mut cache = cache().lock();
        collect(&sources, now - days * DAY_MS, now, &mut cache)
    })
    .await
    .map_err(|_| Error::agent("Usage history could not be read."))
}

/// Where each provider keeps its logs: Claude config folders, Codex homes, OpenCode data folders.
#[derive(Debug, Clone, Default)]
struct Sources {
    claude: Vec<PathBuf>,
    codex: Vec<PathBuf>,
    opencode: Vec<PathBuf>,
}

impl Sources {
    fn for_app(state: &AppState) -> Self {
        let home = crate::new_project::home_dir();
        let env_dir = |key: &str| {
            std::env::var_os(key)
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
        };
        let mut sources = Sources::default();
        if let Some(dir) =
            env_dir("CLAUDE_CONFIG_DIR").or_else(|| home.as_ref().map(|h| h.join(".claude")))
        {
            sources.claude.push(dir);
        }
        if let Some(dir) = env_dir("CODEX_HOME").or_else(|| home.as_ref().map(|h| h.join(".codex")))
        {
            sources.codex.push(dir);
        }
        if let Some(dir) =
            env_dir("XDG_DATA_HOME").or_else(|| home.as_ref().map(|h| h.join(".local/share")))
        {
            sources.opencode.push(dir.join("opencode"));
        }
        // Named accounts (provider_accounts.rs) each sign in from their own private profile.
        let accounts = state.data.lock().provider_accounts.clone();
        for account in accounts {
            let Ok(scope) = crate::provider_accounts::scope(state, &account.provider, &account.id)
            else {
                continue;
            };
            let Some(dir) = scope.home else { continue };
            match account.provider {
                AgentProviderId::Claude => sources.claude.push(dir),
                AgentProviderId::Codex => sources.codex.push(dir),
                _ => {}
            }
        }
        // Sirus Native's host has no account list of its own: its launcher names Sirus Code's
        // `provider-accounts` folder, which is only read here (logs, never logins or keys).
        if let Some(root) = env_dir("SIRUS_USAGE_EXTRA_ACCOUNTS") {
            add_account_profiles(&mut sources, &root);
        }
        dedupe_dirs(&mut sources);
        sources
    }
}

/// Adds each `claude/<id>` and `codex/<id>` profile under a `provider-accounts` folder, as
/// `provider_accounts::home` lays them out. Only account-id folders count; symlinks are skipped.
fn add_account_profiles(sources: &mut Sources, root: &Path) {
    sources.claude.extend(account_profiles(root, "claude"));
    sources.codex.extend(account_profiles(root, "codex"));
}

fn account_profiles(root: &Path, provider: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root.join(provider)) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok())
        })
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    dirs
}

/// Drops a folder already listed (the same path, or a symlink to it), keeping the first.
fn dedupe_dirs(sources: &mut Sources) {
    for list in [
        &mut sources.claude,
        &mut sources.codex,
        &mut sources.opencode,
    ] {
        let mut seen = HashSet::new();
        list.retain(|dir| seen.insert(std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone())));
    }
}

/// One API response's usage, as read from a log.
#[derive(Debug, Clone)]
struct Record {
    ts: i64,
    /// Identity of the response when the log repeats it (Claude) or a fork may copy it (Codex).
    key: Option<u64>,
    model: Arc<str>,
    session: Arc<str>,
    input: u64,
    cached: u64,
    write: u64,
    output: u64,
    cost: Option<f64>,
    speed: Speed,
}

impl Record {
    fn is_empty(&self) -> bool {
        self.input == 0
            && self.cached == 0
            && self.write == 0
            && self.output == 0
            && self.cost.unwrap_or(0.0) == 0.0
    }

    /// A response written on several lines while it streamed: keep the largest counts.
    fn merge(&mut self, other: &Record) {
        self.ts = self.ts.min(other.ts);
        self.input = self.input.max(other.input);
        self.cached = self.cached.max(other.cached);
        self.write = self.write.max(other.write);
        self.output = self.output.max(other.output);
        self.cost = match (self.cost, other.cost) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.speed = self.speed.max(other.speed);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Claude,
    Codex,
    OpenCodeJson,
}

struct CodexFile {
    model: Option<Arc<str>>,
    session: Arc<str>,
    session_raw: String,
    last_total: Option<CodexUsage>,
    /// The latest `thread_settings_applied` service tier, applied to the usage that follows.
    speed: Speed,
    responses: HashSet<u64>,
    /// Per-response `token_usage_record` lines (current CLIs).
    records: Vec<Record>,
    /// Per-turn `token_count` events (older CLIs), used only when there are no records.
    counts: Vec<Record>,
}

enum Parsed {
    Records(Vec<Record>),
    Codex(Box<CodexFile>),
}

impl Parsed {
    fn new(kind: Kind, path: &Path) -> Self {
        match kind {
            Kind::Codex => {
                let raw = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_string();
                Parsed::Codex(Box::new(CodexFile {
                    model: None,
                    session: session_key("codex", &raw),
                    session_raw: raw,
                    last_total: None,
                    speed: Speed::Standard,
                    responses: HashSet::new(),
                    records: Vec::new(),
                    counts: Vec::new(),
                }))
            }
            _ => Parsed::Records(Vec::new()),
        }
    }

    fn records(&self) -> &[Record] {
        match self {
            Parsed::Records(records) => records,
            Parsed::Codex(file) if file.records.is_empty() => &file.counts,
            Parsed::Codex(file) => &file.records,
        }
    }
}

struct CachedFile {
    len: u64,
    mtime: SystemTime,
    /// Bytes read so far (always the end of a complete line).
    offset: u64,
    /// Lines that looked like usage but did not parse.
    bad: u64,
    parsed: Parsed,
}

#[derive(Default)]
struct Cache {
    files: HashMap<PathBuf, CachedFile>,
    /// OpenCode databases: the rows read since `start`, valid while the files are unchanged.
    databases: HashMap<PathBuf, CachedDatabase>,
}

struct CachedDatabase {
    fingerprint: Vec<Option<(u64, SystemTime)>>,
    start: i64,
    records: Vec<Record>,
}

/// Size and modification time of a SQLite database and its write-ahead log.
fn db_fingerprint(path: &Path) -> Vec<Option<(u64, SystemTime)>> {
    ["", "-wal"]
        .iter()
        .map(|suffix| {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            std::fs::metadata(PathBuf::from(name))
                .ok()
                .map(|meta| (meta.len(), meta.modified().unwrap_or(UNIX_EPOCH)))
        })
        .collect()
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn session_key(provider: &str, raw: &str) -> Arc<str> {
    let digest = Sha256::digest(format!("{provider}:{raw}").as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        .into()
}

fn hash_of(parts: &[&str]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut hasher);
    hasher.finish()
}

fn parse_time(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.timestamp_millis())
}

/// Files under `dir` (not following symlinks) that `want` accepts, with their metadata.
fn walk(
    dir: &Path,
    depth: usize,
    want: &dyn Fn(&str) -> bool,
    out: &mut Vec<(PathBuf, std::fs::Metadata)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            if depth > 0 {
                walk(&path, depth - 1, want, out);
            }
        } else if kind.is_file() && path.file_name().and_then(|n| n.to_str()).is_some_and(want) {
            if let Ok(meta) = entry.metadata() {
                out.push((path, meta));
            }
        }
    }
}

/// Feeds each complete line from `offset` on; returns the offset after the last one.
fn read_lines(path: &Path, offset: u64, mut each: impl FnMut(&str)) -> std::io::Result<u64> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut line = Vec::new();
    let mut position = offset;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 || line.last() != Some(&b'\n') {
            break;
        }
        position += read as u64;
        if let Ok(text) = std::str::from_utf8(&line) {
            each(text);
        }
    }
    Ok(position)
}

#[derive(Deserialize)]
struct ClaudeLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    /// "standard", or "fast" in fast mode.
    speed: Option<String>,
}

/// `Ok(None)` for lines without usage, `Err` for usage lines in an unknown shape.
fn claude_line(line: &str, fallback_session: &str) -> std::result::Result<Option<Record>, ()> {
    if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
        return Ok(None);
    }
    let parsed: ClaudeLine = serde_json::from_str(line).map_err(|_| ())?;
    if parsed.kind.as_deref() != Some("assistant") {
        return Ok(None);
    }
    let Some(message) = parsed.message else {
        return Ok(None);
    };
    let Some(usage) = message.usage else {
        return Ok(None);
    };
    let model = message.model.unwrap_or_else(|| "unknown".into());
    if model == "<synthetic>" {
        return Ok(None);
    }
    let ts = parsed.timestamp.as_deref().and_then(parse_time).ok_or(())?;
    let key = message
        .id
        .as_deref()
        .map(|id| hash_of(&[id, parsed.request_id.as_deref().unwrap_or_default()]));
    let session = parsed.session_id.as_deref().unwrap_or(fallback_session);
    Ok(Some(Record {
        ts,
        key,
        model: model.into(),
        session: session_key("claude", session),
        input: usage.input_tokens.unwrap_or(0),
        cached: usage.cache_read_input_tokens.unwrap_or(0),
        write: usage.cache_creation_input_tokens.unwrap_or(0),
        output: usage.output_tokens.unwrap_or(0),
        cost: None,
        speed: if usage.speed.as_deref() == Some("fast") {
            Speed::Fast
        } else {
            Speed::Standard
        },
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct CodexUsage {
    input: u64,
    cached: u64,
    write: u64,
    output: u64,
}

impl CodexUsage {
    fn read(value: &Value) -> Option<Self> {
        let field = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
        value.is_object().then(|| CodexUsage {
            input: field("input_tokens"),
            cached: field("cached_input_tokens"),
            write: field("cache_write_input_tokens"),
            output: field("output_tokens"),
        })
    }

    fn minus(self, before: Option<Self>) -> Self {
        let before = before.unwrap_or_default();
        CodexUsage {
            input: self.input.saturating_sub(before.input),
            cached: self.cached.saturating_sub(before.cached),
            write: self.write.saturating_sub(before.write),
            output: self.output.saturating_sub(before.output),
        }
    }
}

/// Codex's `input_tokens` includes the cached (and cache-written) part.
fn codex_record(file: &CodexFile, ts: i64, key: Option<u64>, usage: CodexUsage) -> Record {
    Record {
        ts,
        key,
        model: file.model.clone().unwrap_or_else(|| "unknown".into()),
        session: file.session.clone(),
        input: usage
            .input
            .saturating_sub(usage.cached)
            .saturating_sub(usage.write),
        cached: usage.cached,
        write: usage.write,
        output: usage.output,
        cost: None,
        speed: file.speed,
    }
}

fn set_codex_session(file: &mut CodexFile, raw: Option<&str>) {
    if let Some(raw) = raw.filter(|raw| !raw.is_empty() && *raw != file.session_raw) {
        file.session = session_key("codex", raw);
        file.session_raw = raw.to_string();
    }
}

fn codex_line(file: &mut CodexFile, line: &str) -> std::result::Result<(), ()> {
    if ![
        "token_count",
        "token_usage_record",
        "turn_context",
        "session_meta",
        "thread_settings_applied",
    ]
    .iter()
    .any(|marker| line.contains(marker))
    {
        return Ok(());
    }
    let value: Value = serde_json::from_str(line).map_err(|_| ())?;
    let payload = &value["payload"];
    let ts = || value["timestamp"].as_str().and_then(parse_time);
    match value["type"].as_str() {
        Some("session_meta") => {
            set_codex_session(
                file,
                payload["id"].as_str().or(payload["session_id"].as_str()),
            );
            if let Some(model) = payload["model"].as_str() {
                file.model.get_or_insert_with(|| model.into());
            }
        }
        Some("turn_context") => {
            if let Some(model) = payload["model"].as_str() {
                if file.model.as_deref() != Some(model) {
                    file.model = Some(model.into());
                }
            }
        }
        Some("token_usage_record") => {
            let usage = CodexUsage::read(&payload["usage"]).ok_or(())?;
            let ts = ts().ok_or(())?;
            set_codex_session(file, payload["session_id"].as_str());
            let key = payload["response_id"]
                .as_str()
                .map(|id| hash_of(&["codex", id]));
            if key.is_some_and(|key| !file.responses.insert(key)) {
                return Ok(());
            }
            let mut record = codex_record(file, ts, key, usage);
            // Not written by current CLIs, but preferred over the thread setting if it appears.
            if let Some(tier) = payload["service_tier"].as_str() {
                record.speed = Speed::codex(tier);
            }
            if !record.is_empty() {
                file.records.push(record);
            }
        }
        Some("event_msg") if payload["type"].as_str() == Some("thread_settings_applied") => {
            if let Some(tier) = payload["thread_settings"]["service_tier"].as_str() {
                file.speed = Speed::codex(tier);
            }
        }
        Some("event_msg") if payload["type"].as_str() == Some("token_count") => {
            let info = &payload["info"];
            if info.is_null() {
                return Ok(());
            }
            let total = CodexUsage::read(&info["total_token_usage"]);
            // The same totals are re-sent with rate-limit updates: count a turn once.
            if total.is_some() && total == file.last_total {
                return Ok(());
            }
            let usage = match CodexUsage::read(&info["last_token_usage"]) {
                Some(last) => last,
                None => total.ok_or(())?.minus(file.last_total),
            };
            if total.is_some() {
                file.last_total = total;
            }
            let ts = ts().ok_or(())?;
            let record = codex_record(file, ts, None, usage);
            if !record.is_empty() {
                file.counts.push(record);
            }
        }
        _ => {}
    }
    Ok(())
}

/// One OpenCode message (`storage/message/<session>/<id>.json` or a `message.data` row).
fn opencode_message(data: &Value, session: Option<&str>, ts: Option<i64>) -> Option<Record> {
    if data["role"].as_str() != Some("assistant") {
        return None;
    }
    let tokens = &data["tokens"];
    let count = |value: &Value| value.as_u64().unwrap_or(0);
    let session = session.or(data["sessionID"].as_str())?;
    let record = Record {
        ts: ts.or(data["time"]["created"].as_i64())?,
        key: None,
        model: data["modelID"].as_str().unwrap_or("unknown").into(),
        session: session_key("opencode", session),
        input: count(&tokens["input"]),
        cached: count(&tokens["cache"]["read"]),
        write: count(&tokens["cache"]["write"]),
        output: count(&tokens["output"]) + count(&tokens["reasoning"]),
        cost: data["cost"].as_f64(),
        speed: Speed::Standard,
    };
    (!record.is_empty()).then_some(record)
}

fn parse_file(
    path: &Path,
    offset: u64,
    parsed: &mut Parsed,
    bad: &mut u64,
) -> std::io::Result<u64> {
    match parsed {
        Parsed::Codex(file) => read_lines(path, offset, |line| {
            if codex_line(file, line).is_err() {
                *bad += 1;
            }
        }),
        Parsed::Records(records) if path.extension().is_some_and(|ext| ext == "jsonl") => {
            let fallback = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string();
            read_lines(path, offset, |line| match claude_line(line, &fallback) {
                Ok(Some(record)) => records.push(record),
                Ok(None) => {}
                Err(()) => *bad += 1,
            })
        }
        Parsed::Records(records) => {
            let mut text = String::new();
            std::fs::File::open(path)?
                .take(OPENCODE_MAX_FILE)
                .read_to_string(&mut text)?;
            match serde_json::from_str::<Value>(&text) {
                Ok(value) => records.extend(opencode_message(&value, None, None)),
                Err(_) => *bad += 1,
            }
            Ok(text.len() as u64)
        }
    }
}

fn mtime_ms(meta: &std::fs::Metadata) -> (SystemTime, i64) {
    let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
    let ms = mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    (mtime, ms)
}

impl Cache {
    /// The file's records, reparsing only what changed since the last pass.
    fn refresh(
        &mut self,
        path: &Path,
        meta: &std::fs::Metadata,
        kind: Kind,
    ) -> std::io::Result<&CachedFile> {
        let len = meta.len();
        let (mtime, _) = mtime_ms(meta);
        let append = match self.files.get(path) {
            Some(file) if file.len == len && file.mtime == mtime => None,
            // JSON-lines logs only grow: read the new lines.
            Some(file) if kind != Kind::OpenCodeJson && len > file.len => Some(true),
            _ => Some(false),
        };
        match append {
            None => {}
            Some(true) => {
                let file = self.files.get_mut(path).expect("cached");
                let mut bad = file.bad;
                match parse_file(path, file.offset, &mut file.parsed, &mut bad) {
                    Ok(offset) => {
                        file.offset = offset;
                        file.bad = bad;
                        file.len = len;
                        file.mtime = mtime;
                    }
                    Err(error) => {
                        self.files.remove(path);
                        return Err(error);
                    }
                }
            }
            Some(false) => {
                self.files.remove(path);
                let mut parsed = Parsed::new(kind, path);
                let mut bad = 0;
                let offset = parse_file(path, 0, &mut parsed, &mut bad)?;
                self.files.insert(
                    path.to_path_buf(),
                    CachedFile {
                        len,
                        mtime,
                        offset,
                        bad,
                        parsed,
                    },
                );
            }
        }
        Ok(self.files.get(path).expect("cached"))
    }

    fn prune(&mut self, seen: &HashSet<PathBuf>) {
        self.files.retain(|path, _| seen.contains(path));
        self.databases.retain(|path, _| seen.contains(path));
        if self.files.len() > MAX_CACHED_FILES {
            let mut by_age: Vec<(SystemTime, PathBuf)> = self
                .files
                .iter()
                .map(|(path, file)| (file.mtime, path.clone()))
                .collect();
            by_age.sort();
            let extra = self.files.len() - MAX_CACHED_FILES;
            for (_, path) in by_age.into_iter().take(extra) {
                self.files.remove(&path);
            }
        }
    }
}

#[derive(Default)]
struct Totals {
    input: u64,
    cached: u64,
    write: u64,
    output: u64,
    cost: Option<f64>,
}

type BucketKey = (i64, AgentProviderId, Arc<str>, Arc<str>, Speed);

#[derive(Default)]
struct Accumulator {
    start: i64,
    buckets: HashMap<BucketKey, Totals>,
    unique: HashMap<(AgentProviderId, u64), Record>,
    offsets: HashMap<i64, i64>,
}

impl Accumulator {
    fn add(&mut self, provider: &AgentProviderId, record: &Record) {
        if record.ts < self.start {
            return;
        }
        match record.key {
            Some(key) => {
                self.unique
                    .entry((provider.clone(), key))
                    .and_modify(|seen| seen.merge(record))
                    .or_insert_with(|| record.clone());
            }
            None => self.bucket(provider.clone(), record),
        }
    }

    /// Start of the local-time hour holding `ms` (any UTC offset, including half hours).
    fn hour(&mut self, ms: i64) -> i64 {
        let offset = *self
            .offsets
            .entry(ms.div_euclid(HOUR_MS))
            .or_insert_with(|| {
                chrono::Local
                    .timestamp_millis_opt(ms)
                    .single()
                    .map(|time| time.offset().fix().local_minus_utc() as i64 * 1000)
                    .unwrap_or(0)
            });
        (ms + offset).div_euclid(HOUR_MS) * HOUR_MS - offset
    }

    fn bucket(&mut self, provider: AgentProviderId, record: &Record) {
        let at = self.hour(record.ts);
        let totals = self
            .buckets
            .entry((
                at,
                provider,
                record.model.clone(),
                record.session.clone(),
                record.speed,
            ))
            .or_default();
        totals.input += record.input;
        totals.cached += record.cached;
        totals.write += record.write;
        totals.output += record.output;
        if let Some(cost) = record.cost {
            *totals.cost.get_or_insert(0.0) += cost;
        }
    }

    fn finish(mut self) -> Vec<UsageHistoryEntry> {
        for ((provider, _), record) in std::mem::take(&mut self.unique) {
            self.bucket(provider, &record);
        }
        let mut entries: Vec<UsageHistoryEntry> = self
            .buckets
            .into_iter()
            .map(
                |((at, provider, model, session, speed), totals)| UsageHistoryEntry {
                    at,
                    provider,
                    model: model.to_string(),
                    session: session.to_string(),
                    input_tokens: totals.input,
                    cached_input_tokens: totals.cached,
                    cache_write_tokens: totals.write,
                    output_tokens: totals.output,
                    reported_cost_usd: totals.cost,
                    speed,
                },
            )
            .collect();
        entries.sort_by(|a, b| {
            (a.at, a.provider.key(), &a.model, &a.session, a.speed).cmp(&(
                b.at,
                b.provider.key(),
                &b.model,
                &b.session,
                b.speed,
            ))
        });
        entries
    }
}

/// How a provider's files fared, to tell "unused" from "unreadable".
#[derive(Default)]
struct Health {
    root_exists: bool,
    files: usize,
    failed: usize,
    bad_lines: u64,
    records: usize,
}

impl Health {
    fn problem(&self, name: &str) -> Option<String> {
        if !self.root_exists || self.files == 0 {
            return None;
        }
        if self.failed == self.files {
            return Some(format!("Could not read the {name} logs."));
        }
        (self.records == 0 && self.bad_lines > 0)
            .then(|| format!("The {name} logs are in a format this version does not recognize."))
    }
}

fn scan_files(
    cache: &mut Cache,
    acc: &mut Accumulator,
    seen: &mut HashSet<PathBuf>,
    health: &mut Health,
    provider: &AgentProviderId,
    kind: Kind,
    files: Vec<(PathBuf, std::fs::Metadata)>,
) {
    for (path, meta) in files {
        seen.insert(path.clone());
        // Logs untouched since the range began hold nothing in it: never opened.
        if mtime_ms(&meta).1 < acc.start {
            continue;
        }
        health.files += 1;
        match cache.refresh(&path, &meta, kind) {
            Ok(file) => {
                health.bad_lines += file.bad;
                let records = file.parsed.records();
                health.records += records.len();
                for record in records {
                    acc.add(provider, record);
                }
            }
            Err(_) => health.failed += 1,
        }
    }
}

/// OpenCode's assistant messages since `start`, from the cache while the database is unchanged.
fn opencode_db<'a>(
    cache: &'a mut Cache,
    path: &Path,
    start: i64,
) -> std::result::Result<&'a [Record], rusqlite::Error> {
    let fingerprint = db_fingerprint(path);
    let fresh = cache
        .databases
        .get(path)
        .is_some_and(|db| db.fingerprint == fingerprint && db.start <= start);
    if !fresh {
        let records = query_opencode(path, start)?;
        cache.databases.insert(
            path.to_path_buf(),
            CachedDatabase {
                fingerprint,
                start,
                records,
            },
        );
    }
    Ok(&cache.databases[path].records)
}

fn query_opencode(path: &Path, start: i64) -> std::result::Result<Vec<Record>, rusqlite::Error> {
    use rusqlite::OpenFlags;
    let conn = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(3))?;
    // `octet_length` reads a row's size without loading it, so attachment-laden user
    // messages (gigabytes in a busy database) are never read; the fields are picked in SQL.
    let mut statement = conn.prepare(
        "SELECT session_id, time_created, json_extract(data, '$.modelID'), \
         json_extract(data, '$.tokens.input'), json_extract(data, '$.tokens.output'), \
         json_extract(data, '$.tokens.reasoning'), json_extract(data, '$.tokens.cache.read'), \
         json_extract(data, '$.tokens.cache.write'), json_extract(data, '$.cost') \
         FROM message WHERE time_created >= ?1 AND octet_length(data) < ?2 \
         AND json_extract(data, '$.role') = 'assistant'",
    )?;
    let mut rows = statement.query(rusqlite::params![start, OPENCODE_MAX_ROW])?;
    let mut sessions: HashMap<String, Arc<str>> = HashMap::new();
    let mut models: HashMap<String, Arc<str>> = HashMap::new();
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        let tokens = |index: usize| -> rusqlite::Result<u64> {
            Ok(row.get::<_, Option<i64>>(index)?.unwrap_or(0).max(0) as u64)
        };
        let raw_session: String = row.get(0)?;
        let model = row
            .get::<_, Option<String>>(2)?
            .unwrap_or_else(|| "unknown".into());
        let record = Record {
            ts: row.get(1)?,
            key: None,
            model: models
                .entry(model)
                .or_insert_with_key(|model| model.as_str().into())
                .clone(),
            session: sessions
                .entry(raw_session)
                .or_insert_with_key(|raw| session_key("opencode", raw))
                .clone(),
            input: tokens(3)?,
            cached: tokens(6)?,
            write: tokens(7)?,
            output: tokens(4)? + tokens(5)?,
            cost: row.get::<_, Option<f64>>(8)?,
            speed: Speed::Standard,
        };
        if !record.is_empty() {
            records.push(record);
        }
    }
    Ok(records)
}

fn collect(sources: &Sources, start: i64, now: i64, cache: &mut Cache) -> UsageHistory {
    let mut acc = Accumulator {
        start,
        ..Default::default()
    };
    let mut seen = HashSet::new();
    let mut problems = Vec::new();
    let mut report = |provider: AgentProviderId, message: Option<String>| {
        if let Some(message) = message {
            problems.push(UsageHistoryProblem { provider, message });
        }
    };

    let mut health = Health::default();
    for dir in &sources.claude {
        let projects = dir.join("projects");
        health.root_exists |= projects.is_dir();
        let mut files = Vec::new();
        walk(&projects, 4, &|name| name.ends_with(".jsonl"), &mut files);
        let provider = AgentProviderId::Claude;
        scan_files(
            cache,
            &mut acc,
            &mut seen,
            &mut health,
            &provider,
            Kind::Claude,
            files,
        );
    }
    report(AgentProviderId::Claude, health.problem("Claude Code"));

    let mut health = Health::default();
    for dir in &sources.codex {
        let mut files = Vec::new();
        let rollout = |name: &str| name.starts_with("rollout-") && name.ends_with(".jsonl");
        // Day folders (YYYY/MM/DD) are only listed: a resumed session keeps appending to
        // the file of the day it began, so the folder date alone cannot rule a file out.
        for (sub, depth) in [("sessions", 3), ("archived_sessions", 0)] {
            health.root_exists |= dir.join(sub).is_dir();
            walk(&dir.join(sub), depth, &rollout, &mut files);
        }
        let provider = AgentProviderId::Codex;
        scan_files(
            cache,
            &mut acc,
            &mut seen,
            &mut health,
            &provider,
            Kind::Codex,
            files,
        );
    }
    report(AgentProviderId::Codex, health.problem("Codex"));

    let mut health = Health::default();
    let mut db_error = false;
    for dir in &sources.opencode {
        let db = dir.join("opencode.db");
        if db.is_file() {
            health.root_exists = true;
            seen.insert(db.clone());
            match opencode_db(cache, &db, start) {
                Ok(records) => {
                    health.records += records.len();
                    for record in records {
                        acc.add(&AgentProviderId::OpenCode, record);
                    }
                }
                Err(_) => db_error = true,
            }
        }
        let storage = dir.join("storage").join("message");
        if storage.is_dir() {
            health.root_exists = true;
            let mut files = Vec::new();
            walk(&storage, 1, &|name| name.ends_with(".json"), &mut files);
            let provider = AgentProviderId::OpenCode;
            scan_files(
                cache,
                &mut acc,
                &mut seen,
                &mut health,
                &provider,
                Kind::OpenCodeJson,
                files,
            );
        }
    }
    let opencode_problem = if db_error && health.records == 0 {
        Some("Could not read the OpenCode database.".to_string())
    } else {
        health.problem("OpenCode")
    };
    report(AgentProviderId::OpenCode, opencode_problem);

    cache.prune(&seen);
    UsageHistory {
        entries: acc.finish(),
        problems,
        generated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const NOW: i64 = 1_791_000_000_000;

    fn iso(ms: i64) -> String {
        chrono::DateTime::from_timestamp_millis(ms)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn write(path: &Path, lines: &[String]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = std::fs::File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn claude(ms: i64, id: &str, request: &str, output: u64) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": iso(ms),
            "sessionId": "s-1",
            "requestId": request,
            "message": {
                "id": id,
                "model": "claude-opus-5-5",
                "content": [{"type": "text", "text": "hi"}],
                "usage": {
                    "input_tokens": 2,
                    "cache_creation_input_tokens": 100,
                    "cache_read_input_tokens": 1000,
                    "output_tokens": output
                }
            }
        })
        .to_string()
    }

    fn run(sources: &Sources, start: i64) -> UsageHistory {
        collect(sources, start, NOW, &mut Cache::default())
    }

    fn sum(history: &UsageHistory, field: fn(&UsageHistoryEntry) -> u64) -> u64 {
        history.entries.iter().map(field).sum()
    }

    #[test]
    fn claude_counts_a_repeated_response_once() {
        let dir = tempfile::tempdir().unwrap();
        let ms = NOW - HOUR_MS;
        write(
            &dir.path().join("projects/p/s-1.jsonl"),
            &[
                claude(ms, "msg_1", "req_1", 4),
                claude(ms + 10, "msg_1", "req_1", 250),
                claude(ms + 20, "msg_1", "req_1", 250),
                claude(ms + 30, "msg_2", "req_2", 10),
                serde_json::json!({"type": "assistant", "timestamp": iso(ms), "message": {
                    "id": "x", "model": "<synthetic>", "usage": {"input_tokens": 9, "output_tokens": 9}}})
                .to_string(),
                r#"{"type":"user","message":{"content":"the word "usage" and "assistant""}}"#.into(),
            ],
        );
        // A forked session's copy of the same response, in another file.
        write(
            &dir.path().join("projects/p/s-1/subagents/a.jsonl"),
            &[claude(ms, "msg_2", "req_2", 10)],
        );
        let sources = Sources {
            claude: vec![dir.path().into()],
            ..Default::default()
        };
        let history = run(&sources, NOW - DAY_MS);
        assert_eq!(sum(&history, |e| e.output_tokens), 260);
        assert_eq!(sum(&history, |e| e.input_tokens), 4);
        assert_eq!(sum(&history, |e| e.cached_input_tokens), 2000);
        assert_eq!(sum(&history, |e| e.cache_write_tokens), 200);
        assert!(history.entries.iter().all(|e| e.model == "claude-opus-5-5"
            && e.provider == AgentProviderId::Claude
            && e.reported_cost_usd.is_none()
            && e.session.len() == 16
            && !e.session.contains('/')));
        assert!(history.problems.is_empty());
    }

    #[test]
    fn account_profiles_are_listed_by_account_id_only() {
        let root = tempfile::tempdir().unwrap();
        let id = "6f1c2b7e-0d8a-4c35-9a4e-2b7d1f0e8c11";
        std::fs::create_dir_all(root.path().join("claude").join(id)).unwrap();
        std::fs::create_dir_all(root.path().join("claude/default")).unwrap();
        std::fs::create_dir_all(root.path().join("codex").join(id)).unwrap();
        std::fs::write(root.path().join("claude/not-a-profile"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.path().join("codex").join(id),
            root.path()
                .join("claude")
                .join("9b2e4f10-0000-4000-8000-000000000000"),
        )
        .unwrap();
        let mut sources = Sources::default();
        add_account_profiles(&mut sources, root.path());
        assert_eq!(sources.claude, vec![root.path().join("claude").join(id)]);
        assert_eq!(sources.codex, vec![root.path().join("codex").join(id)]);

        let mut missing = Sources::default();
        add_account_profiles(&mut missing, &root.path().join("missing"));
        assert!(missing.claude.is_empty() && missing.codex.is_empty());
    }

    #[test]
    fn a_response_in_two_profiles_counts_once_and_a_shared_session_id_does_not_merge() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let ms = NOW - HOUR_MS;
        // Both profiles hold session s-1; msg_1 is the same response in each (a copied log).
        write(
            &first.path().join("projects/p/s-1.jsonl"),
            &[
                claude(ms, "msg_1", "req_1", 10),
                claude(ms + 10, "msg_2", "req_2", 20),
            ],
        );
        write(
            &second.path().join("projects/p/s-1.jsonl"),
            &[
                claude(ms, "msg_1", "req_1", 10),
                claude(ms + 20, "msg_3", "req_3", 30),
            ],
        );
        let mut sources = Sources {
            claude: vec![
                first.path().into(),
                second.path().into(),
                first.path().into(),
            ],
            ..Default::default()
        };
        dedupe_dirs(&mut sources);
        assert_eq!(sources.claude.len(), 2);
        let history = run(&sources, NOW - DAY_MS);
        assert_eq!(sum(&history, |e| e.output_tokens), 60);
        assert_eq!(history.entries.len(), 1);
        assert!(history.problems.is_empty());
    }

    fn codex_line_json(kind: &str, ms: i64, payload: Value) -> String {
        serde_json::json!({"timestamp": iso(ms), "type": kind, "payload": payload}).to_string()
    }

    fn usage(input: u64, cached: u64, output: u64) -> Value {
        serde_json::json!({"input_tokens": input, "cached_input_tokens": cached,
            "output_tokens": output, "reasoning_output_tokens": 1, "total_tokens": input + output})
    }

    #[test]
    fn codex_splits_cached_input_and_follows_the_turn_model() {
        let dir = tempfile::tempdir().unwrap();
        let ms = NOW - 2 * HOUR_MS;
        let count = |ms, last: Value, total: Value| {
            codex_line_json(
                "event_msg",
                ms,
                serde_json::json!({"type": "token_count",
                "info": {"last_token_usage": last, "total_token_usage": total}}),
            )
        };
        write(
            &dir.path().join("sessions/2026/10/01/rollout-a.jsonl"),
            &[
                codex_line_json("session_meta", ms, serde_json::json!({"id": "thread-1"})),
                codex_line_json(
                    "turn_context",
                    ms,
                    serde_json::json!({"model": "gpt-6.1-sol"}),
                ),
                count(ms, usage(100, 60, 10), usage(100, 60, 10)),
                // Re-sent with a rate-limit update: same totals, not a new turn.
                count(ms + 1, usage(100, 60, 10), usage(100, 60, 10)),
                codex_line_json(
                    "turn_context",
                    ms + 2,
                    serde_json::json!({"model": "gpt-6.1-mini"}),
                ),
                count(ms + 3, usage(50, 50, 5), usage(150, 110, 15)),
            ],
        );
        let sources = Sources {
            codex: vec![dir.path().into()],
            ..Default::default()
        };
        let history = run(&sources, NOW - DAY_MS);
        let by_model = |model: &str| {
            history
                .entries
                .iter()
                .find(|e| e.model == model)
                .map(|e| (e.input_tokens, e.cached_input_tokens, e.output_tokens))
        };
        assert_eq!(by_model("gpt-6.1-sol"), Some((40, 60, 10)));
        assert_eq!(by_model("gpt-6.1-mini"), Some((0, 50, 5)));
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].session, history.entries[1].session);
    }

    #[test]
    fn codex_prefers_per_response_records_and_reads_appended_lines() {
        let dir = tempfile::tempdir().unwrap();
        let ms = NOW - HOUR_MS;
        let path = dir.path().join("sessions/2026/10/01/rollout-b.jsonl");
        let record = |id: &str, input| {
            codex_line_json(
                "token_usage_record",
                ms,
                serde_json::json!({
                "session_id": "thread-2", "response_id": id, "usage": usage(input, 10, 3)}),
            )
        };
        let mut lines = vec![
            codex_line_json(
                "turn_context",
                ms,
                serde_json::json!({"model": "gpt-6.1-sol"}),
            ),
            record("resp_1", 30),
            codex_line_json(
                "event_msg",
                ms,
                serde_json::json!({"type": "token_count",
                "info": {"last_token_usage": usage(30, 10, 3), "total_token_usage": usage(30, 10, 3)}}),
            ),
        ];
        write(&path, &lines);
        let sources = Sources {
            codex: vec![dir.path().into()],
            ..Default::default()
        };
        let mut cache = Cache::default();
        let first = collect(&sources, NOW - DAY_MS, NOW, &mut cache);
        assert_eq!(sum(&first, |e| e.input_tokens), 20);
        lines.push(record("resp_2", 40));
        lines.push(record("resp_2", 40));
        write(&path, &lines);
        let second = collect(&sources, NOW - DAY_MS, NOW, &mut cache);
        assert_eq!(sum(&second, |e| e.input_tokens), 50);
        assert_eq!(sum(&second, |e| e.output_tokens), 6);
    }

    #[test]
    fn codex_applies_the_latest_service_tier_to_the_usage_that_follows() {
        let dir = tempfile::tempdir().unwrap();
        let ms = NOW - HOUR_MS;
        let tier = |ms, tier: &str| {
            codex_line_json(
                "event_msg",
                ms,
                serde_json::json!({"type": "thread_settings_applied",
                    "thread_settings": {"model": "gpt-6.1-sol", "service_tier": tier}}),
            )
        };
        let record = |ms, id: &str, input| {
            codex_line_json(
                "token_usage_record",
                ms,
                serde_json::json!({"session_id": "t", "response_id": id, "usage": usage(input, 0, 1)}),
            )
        };
        let count = |ms, total: u64| {
            codex_line_json(
                "event_msg",
                ms,
                serde_json::json!({"type": "token_count",
                    "info": {"total_token_usage": usage(total, 0, 0)}}),
            )
        };
        let model = codex_line_json(
            "turn_context",
            ms,
            serde_json::json!({"model": "gpt-6.1-sol"}),
        );
        // Current CLIs: per-response records.
        write(
            &dir.path().join("sessions/2026/10/01/rollout-c.jsonl"),
            &[
                model.clone(),
                record(ms, "r0", 5),
                tier(ms + 1, "priority"),
                record(ms + 2, "r1", 10),
                record(ms + 3, "r2", 20),
                tier(ms + 4, "default"),
                record(ms + 5, "r3", 40),
                tier(ms + 6, "ultrafast"),
                record(ms + 7, "r4", 80),
            ],
        );
        // Older CLIs: only token_count events.
        write(
            &dir.path().join("sessions/2026/10/01/rollout-d.jsonl"),
            &[
                model,
                tier(ms, "flex"),
                count(ms + 1, 100),
                tier(ms + 2, "priority"),
                count(ms + 3, 300),
            ],
        );
        let sources = Sources {
            codex: vec![dir.path().into()],
            ..Default::default()
        };
        let history = run(&sources, NOW - DAY_MS);
        let input = |speed| -> u64 {
            history
                .entries
                .iter()
                .filter(|e| e.speed == speed)
                .map(|e| e.input_tokens)
                .sum()
        };
        assert_eq!(input(Speed::Fast), 10 + 20 + 80 + 200);
        assert_eq!(input(Speed::Standard), 5 + 40 + 100);
        assert_eq!(history.entries.len(), 4, "one entry per session and speed");
        assert_eq!(
            serde_json::to_value(&history.entries[0]).unwrap()["speed"]
                .as_str()
                .map(|s| s == "fast" || s == "standard"),
            Some(true)
        );
    }

    #[test]
    fn claude_fast_mode_is_its_own_entry() {
        let dir = tempfile::tempdir().unwrap();
        let ms = NOW - HOUR_MS;
        let mut fast: Value = serde_json::from_str(&claude(ms + 1, "msg_f", "req_f", 7)).unwrap();
        fast["message"]["usage"]["speed"] = "fast".into();
        let mut standard: Value = serde_json::from_str(&claude(ms, "msg_s", "req_s", 3)).unwrap();
        standard["message"]["usage"]["speed"] = "standard".into();
        write(
            &dir.path().join("projects/p/s-1.jsonl"),
            &[standard.to_string(), fast.to_string()],
        );
        let sources = Sources {
            claude: vec![dir.path().into()],
            ..Default::default()
        };
        let history = run(&sources, NOW - DAY_MS);
        let output = |speed| {
            history
                .entries
                .iter()
                .find(|e| e.speed == speed)
                .map(|e| e.output_tokens)
        };
        assert_eq!(output(Speed::Fast), Some(7));
        assert_eq!(output(Speed::Standard), Some(3));
    }

    #[test]
    fn opencode_passes_its_own_cost_through() {
        let dir = tempfile::tempdir().unwrap();
        let ms = NOW - HOUR_MS;
        let message = |session: &str, cost: f64| {
            serde_json::json!({"role": "assistant", "modelID": "deepseek-v4", "providerID": "opencode",
                "cost": cost, "tokens": {"input": 10, "output": 4, "reasoning": 2,
                "cache": {"read": 7, "write": 3}}, "time": {"created": ms}, "sessionID": session})
        };
        // Current versions: a SQLite database.
        let data = dir.path().join("opencode");
        std::fs::create_dir_all(&data).unwrap();
        let conn = rusqlite::Connection::open(data.join("opencode.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
             time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);",
        )
        .unwrap();
        let insert = |id: &str, ts: i64, data: Value| {
            conn.execute(
                "INSERT INTO message VALUES (?1, 'ses_1', ?2, ?2, ?3)",
                rusqlite::params![id, ts, data.to_string()],
            )
            .unwrap();
        };
        insert("m1", ms, message("ses_1", 0.25));
        insert(
            "m2",
            ms,
            serde_json::json!({"role": "user", "time": {"created": ms}}),
        );
        insert("m3", NOW - 3 * DAY_MS, message("ses_1", 9.0));
        drop(conn);
        // Older versions: one JSON file per message.
        let legacy = data.join("storage/message/ses_2/msg_1.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, message("ses_2", 0.5).to_string()).unwrap();

        let sources = Sources {
            opencode: vec![data],
            ..Default::default()
        };
        let history = run(&sources, NOW - DAY_MS);
        assert_eq!(history.entries.len(), 2);
        let cost: f64 = history
            .entries
            .iter()
            .filter_map(|e| e.reported_cost_usd)
            .sum();
        assert!((cost - 0.75).abs() < 1e-9);
        assert_eq!(sum(&history, |e| e.output_tokens), 12);
        assert_eq!(sum(&history, |e| e.input_tokens), 20);
        assert_eq!(sum(&history, |e| e.cached_input_tokens), 14);
        assert_eq!(sum(&history, |e| e.cache_write_tokens), 6);
        assert!(history.problems.is_empty());
        assert!(history.entries.iter().all(|e| e.speed == Speed::Standard));
    }

    #[test]
    fn only_the_range_counts_and_hours_are_local_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let recent = NOW - 90 * 60_000;
        write(
            &dir.path().join("projects/p/s.jsonl"),
            &[
                claude(recent, "a", "r", 1),
                claude(recent + 60_000, "b", "r", 2),
                claude(NOW - 2 * DAY_MS, "c", "r", 100),
            ],
        );
        let sources = Sources {
            claude: vec![dir.path().into()],
            ..Default::default()
        };
        let day = run(&sources, NOW - DAY_MS);
        assert_eq!(sum(&day, |e| e.output_tokens), 3);
        assert_eq!(day.entries.len(), 1);
        let at = day.entries[0].at;
        assert!(at <= recent && recent < at + HOUR_MS);
        let local = chrono::Local.timestamp_millis_opt(at).unwrap();
        assert_eq!(chrono::Timelike::minute(&local), 0);
        assert_eq!(chrono::Timelike::second(&local), 0);
        assert_eq!(
            sum(&run(&sources, NOW - 7 * DAY_MS), |e| e.output_tokens),
            103
        );
    }

    #[test]
    fn unreadable_logs_are_reported_and_symlinks_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(
            &outside.path().join("s.jsonl"),
            &[claude(NOW - HOUR_MS, "a", "r", 5)],
        );
        std::fs::create_dir_all(dir.path().join("projects")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), dir.path().join("projects/link")).unwrap();
        write(
            &dir.path().join("projects/p/bad.jsonl"),
            &[
                r#"{"type":"assistant","usage":"#.into(),
                r#"{"type":"assistant","usage":{}}"#.into(),
            ],
        );
        let sources = Sources {
            claude: vec![dir.path().into()],
            ..Default::default()
        };
        let history = run(&sources, NOW - DAY_MS);
        assert!(history.entries.is_empty());
        assert_eq!(history.problems.len(), 1);
        assert_eq!(history.problems[0].provider, AgentProviderId::Claude);
        // A provider that was never used is not a problem.
        let empty = tempfile::tempdir().unwrap();
        let unused = Sources {
            claude: vec![empty.path().into()],
            codex: vec![empty.path().into()],
            opencode: vec![empty.path().into()],
        };
        assert!(run(&unused, NOW - DAY_MS).problems.is_empty());
    }

    /// Times a cold and a warm 90-day read of this computer's real logs (counts only).
    /// `cargo test usage_history::tests::real_logs -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_logs() {
        let home = crate::new_project::home_dir().unwrap();
        let sources = Sources {
            claude: vec![home.join(".claude")],
            codex: vec![home.join(".codex")],
            opencode: vec![home.join(".local/share/opencode")],
        };
        let now = now_ms();
        let mut cache = Cache::default();
        for (label, days) in [
            ("cold 90", 90),
            ("warm 90", 90),
            ("warm 30", 30),
            ("warm 1", 1),
        ] {
            let started = std::time::Instant::now();
            let history = collect(&sources, now - days * DAY_MS, now, &mut cache);
            let mut per: HashMap<&str, usize> = HashMap::new();
            for entry in &history.entries {
                *per.entry(entry.provider.key()).or_default() += entry.output_tokens as usize;
            }
            println!(
                "{label}: {:?}, {} entries, output tokens {per:?}, {} problems, {} cached files",
                started.elapsed(),
                history.entries.len(),
                history.problems.len(),
                cache.files.len()
            );
            if days != 30 {
                continue;
            }
            let codex = |speed| {
                history
                    .entries
                    .iter()
                    .filter(|e| e.provider == AgentProviderId::Codex && e.speed == speed)
                    .count()
            };
            println!(
                "30-day Codex entries: {} fast, {} standard",
                codex(Speed::Fast),
                codex(Speed::Standard)
            );
            // Cost before and after fast prices, with a downloaded LiteLLM table.
            let Some(table) = std::env::var_os("SIRUS_PRICE_TABLE") else {
                continue;
            };
            let document: Value = serde_json::from_slice(&std::fs::read(table).unwrap()).unwrap();
            let rates = crate::usage_prices::parse(&document);
            let (mut before, mut after, mut unpriced) = (0.0, 0.0, 0usize);
            for entry in history
                .entries
                .iter()
                .filter(|e| e.provider == AgentProviderId::Codex)
            {
                let Some(price) = rates.get(&entry.model.to_lowercase()) else {
                    unpriced += 1;
                    continue;
                };
                let cost = |input: f64, cache_read: f64, cache_write: f64, output: f64| {
                    (entry.input_tokens as f64 * input
                        + entry.cached_input_tokens as f64 * cache_read
                        + entry.cache_write_tokens as f64 * cache_write
                        + entry.output_tokens as f64 * output)
                        / 1e6
                };
                let standard = cost(
                    price.input,
                    price.cache_read,
                    price.cache_write,
                    price.output,
                );
                before += standard;
                after += match (entry.speed, price.fast) {
                    (Speed::Fast, Some(f)) => cost(f.input, f.cache_read, f.cache_write, f.output),
                    _ => standard,
                };
            }
            println!(
                "30-day Codex cost: ${before:.2} before, ${after:.2} after ({unpriced} unpriced entries)"
            );
        }
    }
}
