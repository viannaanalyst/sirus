//! Agent processes kept between turns. Codex, Claude and the ACP
//! agents (OpenCode, Devin, Hermes, Cursor) keep their process after a turn that ended cleanly,
//! so the session's next turn reuses it instead of starting the CLI, its MCP servers and the
//! native session again. A process is reused only while its key (binary, account, workspace and
//! the options fixed at start) is unchanged; at most POOL_LIMIT sessions are kept, each for
//! POOL_IDLE without a turn. Stop, a failure, deleting the session or project, and
//! quitting stop it. A kept process is never killed outright: its input is closed first so it
//! can finish what it was writing (a rotated OAuth credential, ADR-165) and exit by itself.
use crate::codex::Wire;
use std::time::Duration;
use tokio::process::Child;

/// A key plus the native conversation the process holds: a kept process continues only the
/// conversation it ran (its first turn creates it, so the key is completed when kept).
pub(crate) fn with_thread(key: &str, session: &crate::models::Session) -> String {
    let thread = session
        .native_thread
        .as_ref()
        .map(|native| native.thread_id.as_str())
        .unwrap_or("");
    format!("{key}\u{0}{thread}")
}

/// Kept processes at most: every finished conversation by default; the person
/// can lower it (Providers → Advanced, `keep_idle_agents`).
pub(crate) const POOL_LIMIT: usize = 16;
static LIMIT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(POOL_LIMIT);

/// Applies the setting at once: lowering it stops the longest idle processes first.
pub(crate) fn set_limit(limit: usize) {
    let limit = limit.min(POOL_LIMIT);
    LIMIT.store(limit, std::sync::atomic::Ordering::Relaxed);
    let evicted: Vec<Pooled> = {
        let mut pool = POOL.lock();
        let excess = pool.len().saturating_sub(limit);
        pool.drain(..excess).collect()
    };
    retire(evicted);
}

pub(crate) const POOL_IDLE: Duration = Duration::from_secs(30 * 60);

struct Pooled {
    session_id: String,
    key: String,
    child: Child,
    wire: Wire,
    serial: u64,
}

static POOL: parking_lot::Mutex<Vec<Pooled>> = parking_lot::Mutex::new(Vec::new());
static POOL_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The session's kept process when it still runs with the same key; otherwise none (and a
/// stale one is stopped).
pub(crate) fn take(session_id: &str, key: &str) -> Option<(Child, Wire)> {
    let mut pool = POOL.lock();
    let index = pool
        .iter()
        .position(|entry| entry.session_id == session_id)?;
    let mut entry = pool.remove(index);
    drop(pool);
    let alive = matches!(entry.child.try_wait(), Ok(None));
    if alive && entry.key == key {
        Some((entry.child, entry.wire))
    } else {
        retire(vec![entry]);
        None
    }
}

/// Keeps a cleanly ended turn's process for the session's next turn.
pub(crate) fn keep(session_id: String, key: String, child: Child, wire: Wire) {
    let serial = POOL_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut evicted = Vec::new();
    {
        let mut pool = POOL.lock();
        if let Some(index) = pool.iter().position(|entry| entry.session_id == session_id) {
            evicted.push(pool.remove(index));
        }
        pool.push(Pooled {
            session_id,
            key,
            child,
            wire,
            serial,
        });
        while pool.len() > LIMIT.load(std::sync::atomic::Ordering::Relaxed) {
            evicted.push(pool.remove(0));
        }
    }
    retire(evicted);
    tokio::spawn(async move {
        tokio::time::sleep(POOL_IDLE).await;
        let idle = {
            let mut pool = POOL.lock();
            pool.iter()
                .position(|entry| entry.serial == serial)
                .map(|index| pool.remove(index))
        };
        if let Some(entry) = idle {
            retire(vec![entry]);
        }
    });
}

/// Stops a session's kept process (session removed, project removed).
pub(crate) fn discard(session_id: &str) {
    let entry = {
        let mut pool = POOL.lock();
        pool.iter()
            .position(|entry| entry.session_id == session_id)
            .map(|index| pool.remove(index))
    };
    if let Some(entry) = entry {
        retire(vec![entry]);
    }
}

/// Closes the kept processes' input and lets each exit by itself in the background before
/// its group is killed (`agent::retire_owned`).
fn retire(entries: Vec<Pooled>) {
    for entry in entries {
        let Pooled { child, wire, .. } = entry;
        drop(wire);
        crate::runtime::async_runtime::spawn(crate::agent::retire_owned(child));
    }
}

/// How long quitting waits for kept processes to exit once their input is closed. Idle CLIs
/// exit at once on end of input; the wait only covers one still persisting its login.
const QUIT_GRACE: Duration = Duration::from_secs(2);

/// Stops every kept process (Sirus closing). Quitting cannot wait on the runtime, so the
/// input of every process is closed at once and their exits are polled, within QUIT_GRACE,
/// before the groups are killed.
pub(crate) fn clear() {
    let mut children: Vec<Child> = std::mem::take(&mut *POOL.lock())
        .into_iter()
        .map(|Pooled { child, wire, .. }| {
            drop(wire);
            child
        })
        .collect();
    let deadline = std::time::Instant::now() + QUIT_GRACE;
    while !children.iter().all(exited) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    for child in &mut children {
        crate::agent::kill_owned_group(child);
    }
}

/// Whether the leader has exited, observed without reaping so its group can still be
/// signalled safely.
fn exited(child: &Child) -> bool {
    #[cfg(unix)]
    {
        let Some(pid) = child.id() else {
            return true;
        };
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &mut info,
                libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
            )
        };
        // WNOHANG leaves the pid zero while the process still runs.
        result != 0 || unsafe { info.si_pid() } != 0
    }
    #[cfg(not(unix))]
    {
        let _ = child;
        false
    }
}

// The tests drive `/bin/cat` in its own process group.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn idle_server() -> (Child, Wire) {
        let mut child = tokio::process::Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0)
            .spawn()
            .unwrap();
        let wire = Wire::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
        (child, wire)
    }

    #[tokio::test]
    async fn kept_app_servers_are_reused_only_with_the_same_key_and_bounded() {
        clear();
        let (child, mut wire) = idle_server();
        wire.initialized = true;
        keep("pool-a".into(), "key".into(), child, wire);
        // Another key (account, workspace or binary changed) stops the kept process.
        assert!(take("pool-a", "other").is_none());
        assert!(take("pool-a", "key").is_none());
        let (child, mut wire) = idle_server();
        wire.initialized = true;
        keep("pool-a".into(), "key".into(), child, wire);
        let (mut child, wire) = take("pool-a", "key").unwrap();
        assert!(wire.initialized);
        assert!(matches!(child.try_wait(), Ok(None)));
        crate::agent::kill_owned_group(&mut child);
        // At most POOL_LIMIT sessions: the oldest is stopped first.
        for index in 0..=POOL_LIMIT {
            let (child, wire) = idle_server();
            keep(format!("pool-{index}"), "key".into(), child, wire);
        }
        assert!(take("pool-0", "key").is_none());
        assert!(take(&format!("pool-{POOL_LIMIT}"), "key").is_some());
        // Lowering the setting stops the longest idle at once; None keeps nothing.
        for index in 0..3 {
            let (child, wire) = idle_server();
            keep(format!("limit-{index}"), "key".into(), child, wire);
        }
        set_limit(1);
        assert!(take("limit-0", "key").is_none());
        assert!(take("limit-1", "key").is_none());
        assert!(take("limit-2", "key").is_some());
        set_limit(0);
        let (child, wire) = idle_server();
        keep("none".into(), "key".into(), child, wire);
        assert!(take("none", "key").is_none());
        set_limit(POOL_LIMIT);
        clear();
        assert!(POOL.lock().is_empty());
    }

    #[tokio::test]
    async fn a_discarded_process_finishes_writing_before_its_group_is_killed() {
        // Like Claude persisting a refreshed login: once its input ends, the leader has a
        // child in its group write a file, waits for it and exits.
        let dir = tempfile::tempdir().unwrap();
        let written = dir.path().join("credential");
        let script = format!(
            "cat >/dev/null; (sleep 0.3; echo rotated > '{}') & wait",
            written.display()
        );
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0)
            .spawn()
            .unwrap();
        let wire = Wire::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
        retire(vec![Pooled {
            session_id: "persisting".into(),
            key: "key".into(),
            child,
            wire,
            serial: 0,
        }]);
        for _ in 0..100 {
            if written.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            std::fs::read_to_string(&written).unwrap_or_default(),
            "rotated\n"
        );
    }
}
