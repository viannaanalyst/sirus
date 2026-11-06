//! What the running agents cost the Mac, and stopping one of their processes (ADR-144).
//!
//! `agent_resources` is read-only: it lists, per session with a running turn, the process
//! tree under its provider CLI (pid, executable basename, a coarse kind, CPU over a short
//! sample and resident memory), the totals and the Mac's memory pressure. The renderer asks
//! only while the sidebar's resource panel is open (every 2 s) and on a few events; nothing
//! samples in the background. Command lines are never read: arguments can hold secrets.
//!
//! `agent_resource_action` stops one process of a session's tree. Ownership is checked
//! again at that moment, against the kernel rather than the snapshot the panel showed: the
//! process must still have the start time the panel saw (a reused pid differs), its live
//! parent chain must reach the session's agent CLI, and that agent must descend from this
//! app. The agent itself, the app, launchd and anything outside the tree (the person's
//! terminals included) are refused. It gets SIGTERM, with its descendants, and SIGKILL after
//! three seconds if the same process is still there. A whole session stops through
//! `stop_agent`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::process_tree::{descendants, Proc};
use crate::runtime::State;

const PS: &str = "/bin/ps";
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// A previous reading younger than this is the CPU baseline; otherwise two readings are taken.
const BASELINE_MAX_AGE: Duration = Duration::from_secs(10);
/// The gap between the two readings of a fresh sample.
const SHORT_SAMPLE: Duration = Duration::from_millis(300);
/// Escalation after SIGTERM.
const KILL_AFTER: Duration = Duration::from_secs(3);
/// Processes listed per session (totals still count every one).
const LISTED_PER_SESSION: usize = 40;
const MAX_ANCESTRY: usize = 64;
const NAME_LIMIT: usize = 48;

/// One row of `ps -A -o pid=,ppid=,time=,rss=,comm=`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub pid: i32,
    pub ppid: i32,
    /// Cumulative CPU time in seconds.
    pub cpu_time: f64,
    pub rss_bytes: u64,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessKind {
    Agent,
    Shell,
    Runtime,
    Build,
    Package,
    Git,
    Browser,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceProcess {
    pub pid: i32,
    /// Kernel start time; passed back on Stop so a reused pid is never signalled.
    pub started: u64,
    pub name: String,
    pub kind: ProcessKind,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResources {
    pub session_id: String,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub process_count: usize,
    /// The agent first, then the busiest processes (at most `LISTED_PER_SESSION`).
    pub processes: Vec<ResourceProcess>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryPressure {
    Normal,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSnapshot {
    pub sessions: Vec<SessionResources>,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub cores: u32,
    pub memory_total: Option<u64>,
    pub memory_pressure: Option<MemoryPressure>,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    StopProcess {
        session_id: String,
        pid: i32,
        started: u64,
    },
}

/// `M:SS.ss`, `MMM:SS.ss` (macOS) or `[DD-]HH:MM:SS` (procps) as seconds.
pub fn parse_cpu_time(text: &str) -> Option<f64> {
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<f64>().ok()?, clock),
        None => (0.0, text),
    };
    let mut seconds = 0.0;
    for part in clock.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + seconds)
}

/// The executable's basename, without a login shell's dash, printable and short.
pub fn safe_name(comm: &str) -> String {
    let base = comm.trim().rsplit('/').next().unwrap_or("");
    let base = base.strip_prefix('-').unwrap_or(base);
    let clean: String = base
        .chars()
        .filter(|character| !character.is_control())
        .take(NAME_LIMIT)
        .collect();
    if clean.is_empty() {
        "?".into()
    } else {
        clean
    }
}

/// A coarse kind from the basename only, so the panel can label a process safely.
pub fn kind_of(name: &str) -> ProcessKind {
    let lower = name.to_ascii_lowercase();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| lower.starts_with(prefix));
    match lower.as_str() {
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "nu" => {
            ProcessKind::Shell
        }
        "git" | "gh" | "git-remote-https" | "ssh" => ProcessKind::Git,
        "npm" | "npx" | "pnpm" | "yarn" | "pip" | "pip3" | "uv" | "brew" | "bundle" | "gem"
        | "pod" => ProcessKind::Package,
        "cargo" | "rustc" | "cc" | "c++" | "clang" | "clang++" | "ld" | "swift" | "swiftc"
        | "swift-frontend" | "xcodebuild" | "go" | "make" | "cmake" | "ninja" | "tsc"
        | "esbuild" | "vite" | "webpack" | "gradle" | "javac" | "rust-analyzer"
        | "clippy-driver" => ProcessKind::Build,
        _ if starts(&[
            "python", "node", "bun", "deno", "ruby", "java", "php", "perl",
        ]) =>
        {
            ProcessKind::Runtime
        }
        _ if starts(&[
            "chrome",
            "chromium",
            "google chrome",
            "firefox",
            "headless_shell",
        ]) || lower.contains("webkit") =>
        {
            ProcessKind::Browser
        }
        _ => ProcessKind::Tool,
    }
}

pub fn parse_table(text: &str) -> Vec<Row> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid = fields.next()?.parse().ok()?;
            let cpu_time = parse_cpu_time(fields.next()?)?;
            let rss_bytes = fields.next()?.parse::<u64>().ok()?.saturating_mul(1024);
            let comm = fields.collect::<Vec<_>>().join(" ");
            Some(Row {
                pid,
                ppid,
                cpu_time,
                rss_bytes,
                name: safe_name(&comm),
            })
        })
        .collect()
}

/// CPU % of one core between two readings; a pid absent from the baseline was born since.
pub fn cpu_percent(row: &Row, baseline: &HashMap<i32, f64>, elapsed: f64) -> f64 {
    if elapsed <= 0.0 {
        return 0.0;
    }
    let before = baseline.get(&row.pid).copied().unwrap_or(0.0);
    // A pid reused since the baseline restarts from zero.
    let used = if row.cpu_time >= before {
        row.cpu_time - before
    } else {
        row.cpu_time
    };
    (used / elapsed * 100.0).max(0.0)
}

/// Per session, its agent and every process below it. `roots` are (session id, agent pid).
pub fn aggregate(
    table: &[Row],
    baseline: &HashMap<i32, f64>,
    elapsed: f64,
    roots: &[(String, i32)],
    started: impl Fn(i32) -> Option<u64>,
) -> Vec<SessionResources> {
    let procs: Vec<Proc> = table
        .iter()
        .map(|row| Proc {
            pid: row.pid,
            ppid: row.ppid,
            pgid: 0,
        })
        .collect();
    let by_pid: HashMap<i32, &Row> = table.iter().map(|row| (row.pid, row)).collect();
    let mut sessions = Vec::new();
    for (session_id, root) in roots {
        let Some(agent) = by_pid.get(root) else {
            continue;
        };
        let mut members: Vec<(&Row, ProcessKind)> = vec![(agent, ProcessKind::Agent)];
        for child in descendants(&procs, *root) {
            if let Some(row) = by_pid.get(&child.pid) {
                members.push((row, kind_of(&row.name)));
            }
        }
        let mut listed: Vec<ResourceProcess> = members
            .iter()
            .filter_map(|(row, kind)| {
                Some(ResourceProcess {
                    pid: row.pid,
                    started: started(row.pid)?,
                    name: row.name.clone(),
                    kind: *kind,
                    cpu_percent: cpu_percent(row, baseline, elapsed),
                    memory_bytes: row.rss_bytes,
                })
            })
            .collect();
        let cpu = members
            .iter()
            .map(|(row, _)| cpu_percent(row, baseline, elapsed))
            .sum();
        let memory = members.iter().map(|(row, _)| row.rss_bytes).sum();
        let process_count = members.len();
        let skip = usize::from(listed.first().is_some_and(|p| p.kind == ProcessKind::Agent));
        listed[skip..].sort_by(|a, b| {
            b.cpu_percent
                .total_cmp(&a.cpu_percent)
                .then(b.memory_bytes.cmp(&a.memory_bytes))
        });
        listed.truncate(LISTED_PER_SESSION);
        sessions.push(SessionResources {
            session_id: session_id.clone(),
            cpu_percent: cpu,
            memory_bytes: memory,
            process_count,
            processes: listed,
        });
    }
    sessions
}

/// Why a stop is refused before any signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// launchd, the kernel, this app or the agent CLI itself.
    Protected,
    /// Exited, or the pid now belongs to another process.
    Gone,
    /// Not below the session's agent (the person's terminals, other apps).
    NotOwned,
}

/// The ownership rule at stop time, over live kernel lookups: `info(pid)` gives the parent
/// and start time. `pid` must still have `started`, its parent chain must reach `root`
/// before launchd, and `root` must descend from `me` (this app).
pub fn check_stop(
    pid: i32,
    started: u64,
    root: i32,
    me: i32,
    info: impl Fn(i32) -> Option<(i32, u64)>,
) -> std::result::Result<(), Refusal> {
    if pid <= 1 || pid == me || pid == root || root <= 1 {
        return Err(Refusal::Protected);
    }
    match info(pid) {
        Some((_, now)) if now == started => {}
        _ => return Err(Refusal::Gone),
    }
    if !reaches(pid, root, &info) || !reaches(root, me, &info) {
        return Err(Refusal::NotOwned);
    }
    Ok(())
}

/// True when `ancestor` is a strict ancestor of `pid` through live parent links.
fn reaches(pid: i32, ancestor: i32, info: &impl Fn(i32) -> Option<(i32, u64)>) -> bool {
    let mut current = pid;
    for _ in 0..MAX_ANCESTRY {
        let Some((parent, _)) = info(current) else {
            return false;
        };
        if parent == ancestor {
            return true;
        }
        if parent <= 1 || parent == current {
            return false;
        }
        current = parent;
    }
    false
}

/// Parent pid and start time from the kernel (not from a snapshot).
#[cfg(target_os = "macos")]
fn kernel_info(pid: i32) -> Option<(i32, u64)> {
    // SAFETY: proc_pidinfo writes at most `size` bytes into a zeroed proc_bsdinfo.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        if libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        ) != size
        {
            return None;
        }
        Some((
            info.pbi_ppid as i32,
            info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        ))
    }
}

#[cfg(target_os = "linux")]
fn kernel_info(pid: i32) -> Option<(i32, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields after the command name, which is in parentheses and may hold spaces.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn kernel_info(_pid: i32) -> Option<(i32, u64)> {
    None
}

async fn read_table() -> Result<Vec<Row>> {
    let mut command = Command::new(PS);
    command
        .args(["-A", "-o", "pid=,ppid=,time=,rss=,comm="])
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LC_ALL", "C");
    let output = crate::cli_output::capture_command(command, PROBE_TIMEOUT)
        .await
        .map_err(|error| Error::new("unavailable", format!("cannot read processes: {error}")))?;
    if !output.status.success() {
        return Err(Error::new("unavailable", "cannot read processes"));
    }
    Ok(parse_table(&String::from_utf8_lossy(&output.stdout)))
}

fn cpu_times(table: &[Row]) -> HashMap<i32, f64> {
    table.iter().map(|row| (row.pid, row.cpu_time)).collect()
}

/// The last reading, the next sample's CPU baseline.
static LAST: Mutex<Option<(Instant, HashMap<i32, f64>)>> = Mutex::new(None);

/// Sessions with a running turn and their agent CLI's pid.
fn roots(state: &AppState) -> Vec<(String, i32)> {
    #[cfg(unix)]
    {
        let agents = state.agents.lock();
        agents
            .iter()
            .filter_map(|(id, process)| {
                let pid = process.pid.filter(|pid| *pid > 1)?;
                Some((id.clone(), pid as i32))
            })
            .collect()
    }
    #[cfg(not(unix))]
    {
        let _ = state;
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn sysctl_number<T: Default + Copy>(name: &std::ffi::CStr) -> Option<T> {
    let mut value = T::default();
    let mut length = std::mem::size_of::<T>();
    // SAFETY: the buffer is a `T` of `length` bytes; sysctlbyname writes at most that.
    let ok = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut value as *mut T as *mut libc::c_void,
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    (ok && length == std::mem::size_of::<T>()).then_some(value)
}

/// macOS pressure levels: 1 normal, 2 warning, 4 critical.
pub fn pressure_from_level(level: i32) -> Option<MemoryPressure> {
    match level {
        1 => Some(MemoryPressure::Normal),
        2 => Some(MemoryPressure::Warning),
        4 => Some(MemoryPressure::Critical),
        _ => None,
    }
}

fn memory_info() -> (Option<u64>, Option<MemoryPressure>) {
    #[cfg(target_os = "macos")]
    {
        let total = sysctl_number::<u64>(c"hw.memsize");
        let pressure = sysctl_number::<i32>(c"kern.memorystatus_vm_pressure_level")
            .and_then(pressure_from_level);
        (total, pressure)
    }
    #[cfg(target_os = "linux")]
    {
        let total = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| {
                let line = text.lines().find(|line| line.starts_with("MemTotal:"))?;
                line.split_whitespace().nth(1)?.parse::<u64>().ok()
            })
            .map(|kb| kb * 1024);
        (total, None)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        (None, None)
    }
}

pub async fn agent_resources(
    state: State<'_, std::sync::Arc<AppState>>,
) -> Result<ResourceSnapshot> {
    let roots = roots(&state);
    let cores = std::thread::available_parallelism()
        .map(|count| count.get() as u32)
        .unwrap_or(1);
    let (memory_total, memory_pressure) = memory_info();
    let sessions = if roots.is_empty() {
        Vec::new()
    } else {
        let first = read_table().await?;
        let read_at = Instant::now();
        let baseline = LAST
            .lock()
            .take()
            .filter(|(at, _)| read_at.duration_since(*at) <= BASELINE_MAX_AGE);
        let (baseline, elapsed, table, at) = match baseline {
            Some((at, times)) => (times, read_at.duration_since(at), first, read_at),
            None => {
                tokio::time::sleep(SHORT_SAMPLE).await;
                let second = read_table().await?;
                let at = Instant::now();
                (cpu_times(&first), at.duration_since(read_at), second, at)
            }
        };
        *LAST.lock() = Some((at, cpu_times(&table)));
        aggregate(&table, &baseline, elapsed.as_secs_f64(), &roots, |pid| {
            kernel_info(pid).map(|(_, started)| started)
        })
    };
    Ok(ResourceSnapshot {
        cpu_percent: sessions.iter().map(|session| session.cpu_percent).sum(),
        memory_bytes: sessions.iter().map(|session| session.memory_bytes).sum(),
        sessions,
        cores,
        memory_total,
        memory_pressure,
    })
}

pub async fn agent_resource_action(
    state: State<'_, std::sync::Arc<AppState>>,
    action: Action,
) -> Result<()> {
    let Action::StopProcess {
        session_id,
        pid,
        started,
    } = action;
    let root = roots(&state)
        .into_iter()
        .find(|(id, _)| *id == session_id)
        .map(|(_, root)| root)
        .ok_or_else(|| Error::new("not_found", "This conversation is no longer running."))?;
    let me = std::process::id() as i32;
    check_stop(pid, started, root, me, kernel_info).map_err(|refusal| match refusal {
        Refusal::Protected => Error::new(
            "forbidden",
            "The agent itself stops with the conversation's Stop.",
        ),
        Refusal::Gone => Error::new("not_found", "That process has already ended."),
        Refusal::NotOwned => Error::new(
            "forbidden",
            "That process does not belong to this conversation, so it is left running.",
        ),
    })?;
    // The process and what it started, each with its start time so a reused pid is skipped.
    let table = read_table().await?;
    let procs: Vec<Proc> = table
        .iter()
        .map(|row| Proc {
            pid: row.pid,
            ppid: row.ppid,
            pgid: 0,
        })
        .collect();
    let mut targets = vec![(pid, started)];
    for child in descendants(&procs, pid) {
        if child.pid == me || child.pid == root {
            continue;
        }
        if let Some((_, born)) = kernel_info(child.pid) {
            targets.push((child.pid, born));
        }
    }
    signal(&targets, Signal::Term);
    std::thread::Builder::new()
        .name("sirus-stop-process".into())
        .spawn(move || {
            std::thread::sleep(KILL_AFTER);
            signal(&targets, Signal::Kill);
        })
        .map_err(|error| Error::agent(format!("cannot schedule the stop: {error}")))?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Signal {
    Term,
    Kill,
}

/// Signals each target still carrying its recorded start time.
fn signal(targets: &[(i32, u64)], which: Signal) {
    #[cfg(unix)]
    for (pid, started) in targets {
        if kernel_info(*pid).map(|(_, now)| now) == Some(*started) {
            let number = match which {
                Signal::Term => libc::SIGTERM,
                Signal::Kill => libc::SIGKILL,
            };
            // SAFETY: a plain signal to a pid just re-verified by its start time.
            unsafe {
                libc::kill(*pid, number);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (targets, which);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: i32, ppid: i32, cpu_time: f64, kb: u64, name: &str) -> Row {
        Row {
            pid,
            ppid,
            cpu_time,
            rss_bytes: kb * 1024,
            name: name.into(),
        }
    }

    #[test]
    fn parses_cpu_times_from_both_ps_flavours() {
        assert_eq!(parse_cpu_time("0:01.50"), Some(1.5));
        assert_eq!(parse_cpu_time("125:03.00"), Some(7503.0));
        assert_eq!(parse_cpu_time("01:02:03"), Some(3723.0));
        assert_eq!(parse_cpu_time("2-00:00:01"), Some(172_801.0));
        assert_eq!(parse_cpu_time("bad"), None);
    }

    #[test]
    fn parses_rows_and_keeps_only_basenames() {
        let text = "  10     1   0:01.00  2048 /opt/homebrew/bin/claude\n  11 10 0:00.10 100 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome Helper\n 12 11 0:00.00 10 -zsh\nnot a row\n";
        let rows = parse_table(text);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "claude");
        assert_eq!(rows[0].rss_bytes, 2048 * 1024);
        assert_eq!(rows[1].name, "Google Chrome Helper");
        assert_eq!(rows[2].name, "zsh");
        assert_eq!(safe_name(""), "?");
        assert_eq!(safe_name(&"x".repeat(200)).len(), NAME_LIMIT);
    }

    #[test]
    fn kinds_come_from_the_name_only() {
        assert_eq!(kind_of("zsh"), ProcessKind::Shell);
        assert_eq!(kind_of("cargo"), ProcessKind::Build);
        assert_eq!(kind_of("node"), ProcessKind::Runtime);
        assert_eq!(kind_of("python3.12"), ProcessKind::Runtime);
        assert_eq!(kind_of("pnpm"), ProcessKind::Package);
        assert_eq!(kind_of("git"), ProcessKind::Git);
        assert_eq!(kind_of("Google Chrome Helper"), ProcessKind::Browser);
        assert_eq!(kind_of("mystery"), ProcessKind::Tool);
    }

    #[test]
    fn cpu_is_the_share_of_a_core_between_readings() {
        let baseline = HashMap::from([(10, 1.0), (11, 5.0)]);
        assert_eq!(cpu_percent(&row(10, 1, 1.5, 0, "a"), &baseline, 0.5), 100.0);
        // Born since the baseline: all its time is new.
        assert_eq!(cpu_percent(&row(12, 1, 0.1, 0, "a"), &baseline, 1.0), 10.0);
        // Reused pid with less time than the baseline restarts from zero.
        assert_eq!(cpu_percent(&row(11, 1, 0.2, 0, "a"), &baseline, 1.0), 20.0);
        assert_eq!(cpu_percent(&row(10, 1, 9.0, 0, "a"), &baseline, 0.0), 0.0);
    }

    #[test]
    fn aggregates_each_session_tree_and_nothing_else() {
        let table = [
            row(1, 0, 0.0, 10, "launchd"),
            row(100, 1, 0.0, 500, "sirus"),
            row(10, 100, 1.0, 1000, "claude"),
            row(11, 10, 2.0, 2000, "zsh"),
            row(12, 11, 3.0, 3000, "cargo"),
            row(20, 100, 0.0, 400, "codex"),
            row(30, 1, 50.0, 9000, "zsh"), // the person's terminal elsewhere
        ];
        let baseline = HashMap::from([(10, 0.5), (11, 2.0), (12, 1.0)]);
        let roots = vec![("a".to_string(), 10), ("b".to_string(), 20)];
        let sessions = aggregate(&table, &baseline, 1.0, &roots, |pid| Some(pid as u64));
        assert_eq!(sessions.len(), 2);
        let a = &sessions[0];
        assert_eq!(a.session_id, "a");
        assert_eq!(a.process_count, 3);
        assert_eq!(a.memory_bytes, 6000 * 1024);
        assert!((a.cpu_percent - 250.0).abs() < 1e-9);
        assert_eq!(a.processes[0].kind, ProcessKind::Agent);
        assert_eq!(a.processes[1].name, "cargo");
        assert!(a.processes.iter().all(|p| p.pid != 30 && p.pid != 100));
        assert_eq!(sessions[1].process_count, 1);
        // A root missing from the table is skipped.
        let gone = aggregate(&table, &baseline, 1.0, &[("c".into(), 99)], |_| Some(1));
        assert!(gone.is_empty());
    }

    #[test]
    fn stop_requires_a_live_descendant_with_the_same_start_time() {
        // pid -> (parent, started)
        let live = HashMap::from([
            (100, (1, 1)),  // the app
            (10, (100, 2)), // agent
            (11, (10, 3)),  // tool
            (12, (11, 4)),  // its child
            (30, (1, 5)),   // the person's terminal
            (31, (30, 6)),
            (40, (1, 7)), // an agent pid not started by the app
            (41, (40, 8)),
        ]);
        let info = |pid: i32| live.get(&pid).copied();
        assert_eq!(check_stop(12, 4, 10, 100, info), Ok(()));
        assert_eq!(check_stop(11, 3, 10, 100, info), Ok(()));
        assert_eq!(check_stop(12, 999, 10, 100, info), Err(Refusal::Gone));
        assert_eq!(check_stop(77, 1, 10, 100, info), Err(Refusal::Gone));
        assert_eq!(check_stop(31, 6, 10, 100, info), Err(Refusal::NotOwned));
        assert_eq!(check_stop(41, 8, 40, 100, info), Err(Refusal::NotOwned));
        for protected in [10, 100, 1, 0] {
            assert_eq!(
                check_stop(protected, 2, 10, 100, info),
                Err(Refusal::Protected)
            );
        }
    }

    #[test]
    fn pressure_levels_map_from_the_kernel() {
        assert_eq!(pressure_from_level(1), Some(MemoryPressure::Normal));
        assert_eq!(pressure_from_level(2), Some(MemoryPressure::Warning));
        assert_eq!(pressure_from_level(4), Some(MemoryPressure::Critical));
        assert_eq!(pressure_from_level(0), None);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn stopping_a_tool_spares_its_parent_and_a_sibling() {
        // This test process stands in for the agent; its parent (the test runner) for the app.
        let root = std::process::id() as i32;
        let me = kernel_info(root).unwrap().0;
        let mut tool = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut sibling = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = tool.id() as i32;
        let started = kernel_info(pid).unwrap().1;
        assert_eq!(
            check_stop(pid, started + 1, root, me, kernel_info),
            Err(Refusal::Gone)
        );
        assert_eq!(check_stop(pid, started, root, me, kernel_info), Ok(()));
        signal(&[(pid, started + 1)], Signal::Term);
        assert!(
            tool.try_wait().unwrap().is_none(),
            "a stale start time was signalled"
        );
        signal(&[(pid, started)], Signal::Term);
        let deadline = Instant::now() + Duration::from_secs(3);
        while tool.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(tool.try_wait().unwrap().is_some());
        assert!(sibling.try_wait().unwrap().is_none());
        let _ = sibling.kill();
        let _ = sibling.wait();
    }
}
