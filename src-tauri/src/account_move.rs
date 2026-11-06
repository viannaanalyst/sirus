//! Continuing a conversation on another account of the same provider (ADR-063 amendment,
//! 2026-10-19). Each named account signs in from its own private profile (`provider_accounts`),
//! and Claude and Codex keep their native transcripts inside it, so a usage limit on one account
//! would otherwise strand the conversation there. Both CLIs resume from the transcript on disk:
//! copying its files into the other profile keeps the native context (tool history, thinking,
//! subagents) and the next turn resumes the same native session there. When the copy is not
//! possible, the session drops its native thread and the next turn carries a handoff recap.
//!
//! - **Claude:** `projects/<encoded cwd>/<id>.jsonl`, its sidecar folder `projects/<encoded cwd>/<id>/`
//!   (subagent transcripts, large tool results) and `file-history/<id>/` (edit checkpoints).
//! - **Codex:** the rollout `sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl` (or an archived one),
//!   the rollouts of agents it spawned (`thread_spawn_edges` in the source `state_<n>.sqlite`) and
//!   any `history_base` rollout. Codex indexes a copied rollout by itself on `thread/resume`.
//!
//! The transcript or root rollout is copied last, through a flushed temporary file, so a partial
//! copy is never mistaken for a complete one; an older copy from an earlier move is replaced.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::commands::{native_task, AppState};
use crate::error::{Error, Result};
use crate::models::AgentProviderId;
use crate::runtime::State;

/// Rollouts carried with one Codex conversation (the root, spawned agents, history bases).
const MAX_ROLLOUTS: usize = 256;
/// Files carried from one Claude sidecar or checkpoint folder.
const MAX_SIDE_FILES: usize = 4096;
/// Folder depth walked inside a Claude sidecar or checkpoint folder.
const MAX_SIDE_DEPTH: usize = 6;
/// Project folders scanned when the session's own encoded folder does not hold the transcript.
const MAX_PROJECT_DIRS: usize = 20_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveSessionAccountRequest {
    pub session_id: String,
    pub account_id: String,
}

/// How the conversation continues on the other account.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AccountMoveOutcome {
    /// The native transcript was copied; the next turn resumes the same native session.
    Carried,
    /// The copy was not possible; the next turn starts fresh with a handoff recap.
    Summary,
    /// The session had no native session yet; only its account changed.
    Switched,
}

/// Sessions with a move in progress: a second move of the same session is refused.
static MOVING: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());

struct MoveGuard(String);
impl MoveGuard {
    fn claim(id: &str) -> Result<Self> {
        let mut moving = MOVING.lock();
        if moving.iter().any(|item| item == id) {
            return Err(Error::agent(
                "This conversation is already moving to another account.",
            ));
        }
        moving.push(id.into());
        Ok(Self(id.into()))
    }
}
impl Drop for MoveGuard {
    fn drop(&mut self) {
        MOVING.lock().retain(|item| item != &self.0);
    }
}

/// Moves a stopped Claude or Codex session to another signed-in account of the same provider.
/// The caller then sends the continue turn through `send_prompt`, which clears the usage limit.
pub async fn move_session_account(
    state: State<'_, Arc<AppState>>,
    request: MoveSessionAccountRequest,
) -> Result<serde_json::Value> {
    let state = state.inner().clone();
    native_task(move || move_native(&state, request)).await
}

fn move_native(state: &AppState, request: MoveSessionAccountRequest) -> Result<serde_json::Value> {
    state.ensure_running()?;
    let _guard = MoveGuard::claim(&request.session_id)?;
    let (agent, from_account, thread, cwd) = {
        let data = state.data.lock();
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == request.session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        if !crate::provider_accounts::supported(&session.agent) {
            return Err(Error::agent(
                "Only Claude and Codex conversations can continue on another account.",
            ));
        }
        if state.agents.lock().contains_key(&session.id) || session.status.is_active() {
            return Err(Error::agent(
                "Stop or finish this conversation before moving it to another account.",
            ));
        }
        if session.provider_account_id == request.account_id {
            return Err(Error::agent("This conversation already uses that account."));
        }
        let cwd = match &session.native_thread {
            Some(thread) => thread.cwd.clone(),
            None => crate::commands::session_cwd(&data, session)?
                .to_string_lossy()
                .into_owned(),
        };
        (
            session.agent.clone(),
            session.provider_account_id.clone(),
            session.native_thread.clone(),
            cwd,
        )
    };
    // The account must belong to this provider, be registered and not be signing in.
    let target = crate::provider_accounts::scope(state, &agent, &request.account_id)?;
    if state.usage.signed_in(&agent, &target.id) != Some(true) {
        return Err(Error::agent(
            "That account is not signed in. Sign in to it in Settings → Providers first.",
        ));
    }
    // A kept process still runs on the old account; it must not write to the transcript.
    crate::agent_pool::discard(&request.session_id);
    let outcome = match &thread {
        None => AccountMoveOutcome::Switched,
        Some(thread) => {
            let source = crate::provider_accounts::scope(state, &agent, &from_account)
                .ok()
                .and_then(|scope| provider_home(state, &agent, scope.home, &cwd));
            let destination = provider_home(state, &agent, target.home.clone(), &cwd);
            let copied = match (source, destination) {
                (Some(from), Some(to)) => carry(&agent, &from, &to, &thread.cwd, &thread.thread_id),
                _ => Err("the account folders could not be found".into()),
            };
            match copied {
                Ok(()) => AccountMoveOutcome::Carried,
                Err(reason) => {
                    tracing::warn!(%reason, "native transcript not carried; continuing from a recap");
                    AccountMoveOutcome::Summary
                }
            }
        }
    };
    let mut data = state.data_with_messages(&[&request.session_id]);
    state.ensure_running()?;
    let running = state.agents.lock().contains_key(&request.session_id);
    let session = data
        .sessions
        .iter_mut()
        .find(|session| session.id == request.session_id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    if running
        || session.status.is_active()
        || session.agent != agent
        || session.provider_account_id != from_account
        || session.native_thread != thread
    {
        return Err(Error::agent(
            "The conversation changed while it was moving; try again.",
        ));
    }
    let previous = session.clone();
    apply_move(session, &target.id, outcome);
    let meta = crate::transcript_view::session_meta(session)?;
    if let Err(error) = crate::persist::save(&state.data_path, &data) {
        if let Some(session) = data
            .sessions
            .iter_mut()
            .find(|session| session.id == request.session_id)
        {
            *session = previous;
        }
        return Err(error);
    }
    Ok(serde_json::json!({ "outcome": outcome, "session": meta }))
}

/// Pins the session to its new account. A carried transcript keeps the native session (now
/// bound to that account); otherwise the next turn starts a new one with a handoff recap.
fn apply_move(session: &mut crate::models::Session, account: &str, outcome: AccountMoveOutcome) {
    session.provider_account_id = account.into();
    session
        .account_bindings
        .insert(session.agent.clone(), account.into());
    match outcome {
        AccountMoveOutcome::Carried => {
            if let Some(thread) = session.native_thread.as_mut() {
                thread.provider_account_id = account.into();
            }
        }
        AccountMoveOutcome::Summary => {
            session.native_thread = None;
            if let Some(origin) = &mut session.fork_origin {
                origin.seeded_native_thread_id = None;
            }
            let agent = session.agent.clone();
            crate::transcript::arm_in_session_handoff(session, agent);
        }
        AccountMoveOutcome::Switched => {}
    }
    session.last_activity_at = crate::paths::now_rfc3339();
}

/// The folder the provider CLI reads for this account: the named account's private profile,
/// else the folder the default login uses (`CLAUDE_CONFIG_DIR` / `CODEX_HOME`, else `~/.claude`
/// / `~/.codex`).
fn provider_home(
    state: &AppState,
    agent: &AgentProviderId,
    home: Option<PathBuf>,
    cwd: &str,
) -> Option<PathBuf> {
    if home.is_some() {
        return home;
    }
    match agent {
        AgentProviderId::Claude => {
            crate::provider_accounts::claude_config_dir(state, "default", cwd)
        }
        AgentProviderId::Codex => {
            match std::env::var_os("CODEX_HOME").filter(|dir| !dir.is_empty()) {
                Some(dir) => Some(Path::new(cwd).join(dir)).filter(|dir| dir.is_absolute()),
                None => crate::new_project::home_dir().map(|home| home.join(".codex")),
            }
        }
        _ => None,
    }
}

fn carry(
    agent: &AgentProviderId,
    from: &Path,
    to: &Path,
    cwd: &str,
    thread_id: &str,
) -> std::result::Result<(), String> {
    valid_thread_id(thread_id)?;
    if same_path(from, to) {
        return Err("both accounts use the same folder".into());
    }
    match agent {
        AgentProviderId::Claude => carry_claude(from, to, cwd, thread_id),
        AgentProviderId::Codex => carry_codex(from, to, thread_id),
        _ => Err("this provider has no accounts".into()),
    }
}

fn valid_thread_id(id: &str) -> std::result::Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("invalid native session id".into());
    }
    Ok(())
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Claude names a project folder after the working directory, every character other than
/// an ASCII letter or digit becoming `-`.
pub(crate) fn claude_project_name(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

fn is_real_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

pub(crate) fn carry_claude(
    from: &Path,
    to: &Path,
    cwd: &str,
    id: &str,
) -> std::result::Result<(), String> {
    let name = format!("{id}.jsonl");
    let projects = from.join("projects");
    let expected = projects.join(claude_project_name(cwd));
    let project = if is_real_file(&expected.join(&name)) {
        expected
    } else {
        // Long paths are shortened by the CLI: look for the transcript in any project folder.
        std::fs::read_dir(&projects)
            .map_err(|_| "the Claude projects folder is missing")?
            .take(MAX_PROJECT_DIRS)
            .flatten()
            .map(|entry| entry.path())
            .find(|dir| is_real_dir(dir) && is_real_file(&dir.join(&name)))
            .ok_or("the Claude transcript was not found")?
    };
    let folder = project
        .file_name()
        .ok_or("invalid Claude project folder")?
        .to_owned();
    let dest_project = to.join("projects").join(folder);
    private_dirs(&dest_project)?;
    let mut budget = MAX_SIDE_FILES;
    copy_tree(&project.join(id), &dest_project.join(id), 0, &mut budget)?;
    copy_tree(
        &from.join("file-history").join(id),
        &to.join("file-history").join(id),
        0,
        &mut budget,
    )?;
    copy_durable(&project.join(&name), &dest_project.join(&name))
        .map_err(|error| format!("cannot copy the Claude transcript: {error}"))
}

/// The CLI's own transcript file for a session's native thread inside `config_dir`
/// (`projects/<encoded cwd>/<id>.jsonl`), looking in every project folder when the folder
/// name is shortened. `None` when no such transcript exists.
pub(crate) fn claude_transcript(config_dir: &Path, cwd: &str, thread_id: &str) -> Option<PathBuf> {
    valid_thread_id(thread_id).ok()?;
    let name = format!("{thread_id}.jsonl");
    let projects = config_dir.join("projects");
    let expected = projects.join(claude_project_name(cwd)).join(&name);
    if is_real_file(&expected) {
        return Some(expected);
    }
    std::fs::read_dir(&projects)
        .ok()?
        .take(MAX_PROJECT_DIRS)
        .flatten()
        .map(|entry| entry.path())
        .find(|dir| is_real_dir(dir) && is_real_file(&dir.join(&name)))
        .map(|dir| dir.join(&name))
}

pub(crate) fn carry_codex(from: &Path, to: &Path, id: &str) -> std::result::Result<(), String> {
    let roots: Vec<PathBuf> = [from.join("sessions"), from.join("archived_sessions")]
        .into_iter()
        .filter(|root| is_real_dir(root))
        .collect();
    let find = |thread: &str| roots.iter().find_map(|root| find_rollout(root, thread, 0));
    let root = find(id).ok_or("the Codex rollout was not found")?;
    let mut files = vec![root];
    let mut pending = spawned_threads(from, id);
    let mut index = 0;
    loop {
        while index < files.len() {
            if let Some(base) = history_base(&files[index]) {
                if valid_thread_id(&base).is_ok() {
                    pending.push(base);
                }
            }
            index += 1;
        }
        let Some(thread) = pending.pop() else { break };
        if files.len() >= MAX_ROLLOUTS {
            return Err("the Codex conversation has too many rollouts".into());
        }
        if files.iter().any(|path| rollout_matches(path, &thread)) {
            continue;
        }
        // A spawned agent that never wrote a rollout has nothing to resume.
        if let Some(path) = find(&thread) {
            files.push(path);
        }
    }
    // Descendants first, the root last.
    for path in files.iter().rev() {
        let dest = rollout_dest(to, path)?;
        copy_durable(path, &dest)
            .map_err(|error| format!("cannot copy a Codex rollout: {error}"))?;
    }
    Ok(())
}

fn rollout_matches(path: &Path, id: &str) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.starts_with("rollout-")
            && (name.ends_with(&format!("-{id}.jsonl"))
                || name.ends_with(&format!("-{id}.jsonl.zst")))
    })
}

/// Rollouts live at most three folders deep (`YYYY/MM/DD`).
fn find_rollout(dir: &Path, id: &str, depth: usize) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut folders = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_file() && rollout_matches(&path, id) {
            return Some(path);
        }
        if kind.is_dir() && depth < 3 {
            folders.push(path);
        }
    }
    // Recent days first: the conversation that just hit its limit is usually there.
    folders.sort_unstable_by(|a, b| b.cmp(a));
    folders
        .into_iter()
        .find_map(|folder| find_rollout(&folder, id, depth + 1))
}

/// Where a rollout goes in another home: `sessions/YYYY/MM/DD/<same name>`, the date taken
/// from the name (`rollout-YYYY-MM-DDT…`).
fn rollout_dest(home: &Path, path: &Path) -> std::result::Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or("missing Codex rollout name")?
        .to_string_lossy()
        .into_owned();
    if !name.starts_with("rollout-") || !(name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
    {
        return Err("not a Codex rollout".into());
    }
    let date = name
        .get(8..18)
        .filter(|date| {
            date.bytes().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            })
        })
        .ok_or("missing Codex rollout date")?;
    let dir = home
        .join("sessions")
        .join(&date[..4])
        .join(&date[5..7])
        .join(&date[8..10]);
    private_dirs(&dir)?;
    Ok(dir.join(name))
}

/// The thread a rollout inherits its history from, named in its first line
/// (`session_meta.payload.history_base.thread_id`). Compressed rollouts are not read.
fn history_base(path: &Path) -> Option<String> {
    use std::io::{BufRead, Read};
    if path.extension().is_some_and(|ext| ext == "zst") {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(file.take(1024 * 1024))
        .read_line(&mut line)
        .ok()?;
    let value: serde_json::Value = serde_json::from_str(&line).ok()?;
    if value["type"] != "session_meta" {
        return None;
    }
    value["payload"]["history_base"]["thread_id"]
        .as_str()
        .map(str::to_owned)
}

/// Agents the conversation spawned, from the source home's newest `state_<n>.sqlite`
/// (read-only). Without the index the root alone still resumes.
fn spawned_threads(home: &Path, id: &str) -> Vec<String> {
    let Some(path) = state_db(home) else {
        return Vec::new();
    };
    let read = || -> rusqlite::Result<Vec<String>> {
        let db = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.busy_timeout(std::time::Duration::from_secs(2))?;
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='thread_spawn_edges')",
            [],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(Vec::new());
        }
        let mut statement = db.prepare(
            "WITH RECURSIVE subtree(child) AS (SELECT child_thread_id FROM thread_spawn_edges WHERE parent_thread_id = ?1 UNION SELECT edge.child_thread_id FROM thread_spawn_edges edge JOIN subtree ON edge.parent_thread_id = subtree.child) SELECT child FROM subtree LIMIT ?2",
        )?;
        let rows = statement.query_map(rusqlite::params![id, MAX_ROLLOUTS as i64], |row| {
            row.get::<_, String>(0)
        })?;
        Ok(rows
            .flatten()
            .filter(|thread| valid_thread_id(thread).is_ok())
            .collect())
    };
    read().unwrap_or_default()
}

fn state_db(home: &Path) -> Option<PathBuf> {
    std::fs::read_dir(home)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version: u32 = name
                .strip_prefix("state_")?
                .strip_suffix(".sqlite")?
                .parse()
                .ok()?;
            Some((version, entry.path()))
        })
        .filter(|(_, path)| is_real_file(path))
        .max_by_key(|(version, _)| *version)
        .map(|(_, path)| path)
}

/// Creates missing folders private to the person (0700), like the account profiles.
fn private_dirs(path: &Path) -> std::result::Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    if !is_real_dir(path) {
        return Err("a destination folder is not a folder".into());
    }
    Ok(())
}

/// Copies a folder's regular files (links are skipped), bounded in depth and count.
fn copy_tree(
    source: &Path,
    dest: &Path,
    depth: usize,
    budget: &mut usize,
) -> std::result::Result<(), String> {
    if !is_real_dir(source) {
        return Ok(());
    }
    if depth > MAX_SIDE_DEPTH {
        return Err("a Claude session folder is too deep".into());
    }
    private_dirs(dest)?;
    let entries = std::fs::read_dir(source).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let target = dest.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target, depth + 1, budget)?;
        } else if kind.is_file() {
            if *budget == 0 {
                return Err("a Claude session folder holds too many files".into());
            }
            *budget -= 1;
            copy_durable(&entry.path(), &target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Copies through a flushed temporary file beside the destination, so a reader never sees
/// a partial copy; an existing destination is replaced.
fn copy_durable(path: &Path, dest: &Path) -> std::io::Result<()> {
    let mut temp = dest.as_os_str().to_os_string();
    temp.push(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let temp = PathBuf::from(temp);
    let result = std::fs::copy(path, &temp)
        .and_then(|_| {
            std::fs::File::options()
                .write(true)
                .open(&temp)
                .and_then(|file| file.sync_all())
        })
        .and_then(|()| std::fs::rename(&temp, dest));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn claude_transcript_moves_with_its_sidecar_and_checkpoints() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("from"), root.path().join("to"));
        let cwd = "/Users/me/.cache/app";
        let project = from.join("projects").join("-Users-me--cache-app");
        write(&project.join("abc.jsonl"), "{\"turn\":2}\n");
        write(&project.join("abc/subagents/agent-1.jsonl"), "agent");
        write(&project.join("other.jsonl"), "unrelated");
        write(&from.join("file-history/abc/v1"), "checkpoint");
        // An older copy from an earlier move is replaced.
        let dest = to.join("projects").join("-Users-me--cache-app");
        write(&dest.join("abc.jsonl"), "{\"turn\":1}\n");

        carry(&AgentProviderId::Claude, &from, &to, cwd, "abc").unwrap();

        assert_eq!(
            std::fs::read_to_string(dest.join("abc.jsonl")).unwrap(),
            "{\"turn\":2}\n"
        );
        assert!(dest.join("abc/subagents/agent-1.jsonl").is_file());
        assert!(to.join("file-history/abc/v1").is_file());
        assert!(!dest.join("other.jsonl").exists());
        // No temporary file is left behind.
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 2);
    }

    #[test]
    fn claude_transcript_is_found_in_a_shortened_project_folder() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("from"), root.path().join("to"));
        write(&from.join("projects/-very-long-abc123/abc.jsonl"), "x");
        carry_claude(&from, &to, "/very/long/path", "abc").unwrap();
        assert!(to.join("projects/-very-long-abc123/abc.jsonl").is_file());
    }

    #[test]
    fn a_missing_transcript_or_unsafe_id_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("from"), root.path().join("to"));
        std::fs::create_dir_all(from.join("projects/-w")).unwrap();
        std::fs::create_dir_all(from.join("sessions")).unwrap();
        assert!(carry(&AgentProviderId::Claude, &from, &to, "/w", "missing").is_err());
        assert!(carry(&AgentProviderId::Codex, &from, &to, "/w", "missing").is_err());
        assert!(carry(&AgentProviderId::Claude, &from, &to, "/w", "../x").is_err());
        assert!(carry(&AgentProviderId::Claude, &from, &from, "/w", "abc").is_err());
        assert!(!to.join("projects").exists());
    }

    #[test]
    fn codex_rollout_moves_with_spawned_agents_and_history_base() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("from"), root.path().join("to"));
        let rollout = |dir: &str, day: &str, id: &str, base: Option<&str>| {
            let meta = serde_json::json!({"type":"session_meta","payload":{"id":id,
                "history_base": base.map(|id| serde_json::json!({"thread_id": id}))}});
            write(
                &from.join(format!(
                    "{dir}/2026/10/{day}/rollout-2026-10-{day}T10-00-00-{id}.jsonl"
                )),
                &format!("{meta}\n"),
            );
        };
        rollout("sessions", "18", "root-1", Some("base-1"));
        rollout("archived_sessions", "08", "base-1", None);
        rollout("sessions", "18", "child-1", None);
        rollout("sessions", "18", "grandchild-1", None);
        rollout("sessions", "17", "unrelated-1", None);
        let db = rusqlite::Connection::open(from.join("state_5.sqlite")).unwrap();
        db.execute_batch(
            "CREATE TABLE thread_spawn_edges (parent_thread_id TEXT NOT NULL, child_thread_id TEXT NOT NULL PRIMARY KEY, status TEXT NOT NULL);
             INSERT INTO thread_spawn_edges VALUES ('root-1','child-1','closed'),('child-1','grandchild-1','closed'),('other','unrelated-1','open');",
        )
        .unwrap();
        drop(db);

        carry(&AgentProviderId::Codex, &from, &to, "/w", "root-1").unwrap();

        let day = |d: &str| to.join(format!("sessions/2026/10/{d}"));
        for (d, id) in [
            ("18", "root-1"),
            ("08", "base-1"),
            ("18", "child-1"),
            ("18", "grandchild-1"),
        ] {
            assert!(
                day(d)
                    .join(format!("rollout-2026-10-{d}T10-00-00-{id}.jsonl"))
                    .is_file(),
                "{id}"
            );
        }
        assert!(!day("17").exists());
        // The index itself is not copied: Codex rebuilds it on resume.
        assert!(!to.join("state_5.sqlite").exists());
    }

    fn session(thread: bool) -> crate::models::Session {
        let mut session: crate::models::Session = serde_json::from_value(serde_json::json!({
            "id":"s","title":"Task","projectId":"p","agent":"claude","providerAccountId":"default",
            "status":"failed","createdAt":"time","lastActivityAt":"time",
            "worktree":{"path":"/w","branch":"main","isolated":false},
            "usageLimit":{"resetsAt":null},
            "messages":[
                {"id":"u","sessionId":"s","role":"user","content":"Build the page","createdAt":"time","streaming":false},
                {"id":"a","sessionId":"s","role":"agent","content":"Half of it is done","createdAt":"time","streaming":false}
            ]
        }))
        .unwrap();
        if thread {
            session.native_thread = Some(crate::models::NativeThread {
                provider_account_id: "default".into(),
                thread_id: "abc".into(),
                session_id: "s".into(),
                project_id: "p".into(),
                cwd: "/w".into(),
                model: None,
            });
        }
        session
    }

    #[test]
    fn a_carried_transcript_keeps_the_native_session_on_the_new_account() {
        let mut moved = session(true);
        apply_move(&mut moved, "work", AccountMoveOutcome::Carried);
        assert_eq!(moved.provider_account_id, "work");
        assert_eq!(moved.account_bindings[&AgentProviderId::Claude], "work");
        let thread = moved.native_thread.as_ref().unwrap();
        assert_eq!(
            (
                thread.thread_id.as_str(),
                thread.provider_account_id.as_str()
            ),
            ("abc", "work")
        );
        assert!(crate::claude::validate_binding(&moved, "/w").is_ok());
        assert!(moved.handoff.is_none());
    }

    #[test]
    fn a_failed_copy_continues_from_a_handoff_recap() {
        let mut moved = session(true);
        apply_move(&mut moved, "work", AccountMoveOutcome::Summary);
        assert_eq!(moved.provider_account_id, "work");
        assert!(moved.native_thread.is_none());
        let handoff = moved.handoff.as_ref().unwrap();
        assert!(handoff.pending && handoff.brief.contains("Half of it is done"));
        assert_eq!(handoff.from, AgentProviderId::Claude);
        let prompt = crate::transcript::consume_handoff(
            &mut moved,
            "Continue from where you left off.".into(),
        );
        assert!(
            prompt.contains("<handoff>") && prompt.contains("Continue from where you left off.")
        );
    }

    #[test]
    fn a_session_without_a_native_session_only_changes_account() {
        let mut moved = session(false);
        apply_move(&mut moved, "work", AccountMoveOutcome::Switched);
        assert_eq!(moved.provider_account_id, "work");
        assert!(moved.native_thread.is_none() && moved.handoff.is_none());
    }

    #[test]
    fn the_request_is_closed() {
        assert!(serde_json::from_value::<MoveSessionAccountRequest>(
            serde_json::json!({"sessionId":"s","accountId":"a","home":"/tmp"})
        )
        .is_err());
        assert_eq!(
            serde_json::to_value(AccountMoveOutcome::Carried).unwrap(),
            "carried"
        );
    }

    #[test]
    fn concurrent_moves_of_one_session_are_refused() {
        let first = MoveGuard::claim("guarded").unwrap();
        assert!(MoveGuard::claim("guarded").is_err());
        drop(first);
        assert!(MoveGuard::claim("guarded").is_ok());
    }

    fn private(path: &Path) {
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        if !path.exists() {
            builder.create(path).unwrap();
        }
    }

    /// A stopped Claude session on account `from`, with both accounts' profiles on disk.
    fn fixture(root: &Path, transcript: bool) -> (Arc<AppState>, PathBuf, String) {
        let (from, to) = (
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
        );
        let homes = root.join("provider-accounts");
        for dir in [homes.clone(), homes.join("claude")] {
            private(&dir);
        }
        for id in [&from, &to] {
            private(&homes.join("claude").join(id));
        }
        let workspace = root.join("work");
        std::fs::create_dir_all(&workspace).unwrap();
        let cwd = crate::paths::display_path(&workspace);
        if transcript {
            write(
                &homes
                    .join("claude")
                    .join(&from)
                    .join("projects")
                    .join(claude_project_name(&cwd))
                    .join("abc.jsonl"),
                "{}\n",
            );
        }
        let mut moved = session(true);
        moved.provider_account_id = from.clone();
        moved.worktree.path = cwd.clone();
        let thread = moved.native_thread.as_mut().unwrap();
        thread.provider_account_id = from.clone();
        thread.cwd = cwd.clone();
        let mut data = crate::models::AppData {
            projects: vec![crate::models::Project {
                id: "p".into(),
                name: "Fixture".into(),
                path: cwd,
                added_at: "t".into(),
                last_opened_at: "t".into(),
                look: Default::default(),
                appearance: None,
                scripts: Default::default(),
            }],
            sessions: vec![moved],
            ..Default::default()
        };
        for (id, label) in [(&from, "Personal"), (&to, "Work")] {
            data.provider_accounts.push(crate::models::ProviderAccount {
                id: id.clone(),
                provider: AgentProviderId::Claude,
                label: label.into(),
            });
        }
        let state = Arc::new(AppState {
            data_path: root.join("state.json"),
            worktree_root: root.join("trees"),
            data: parking_lot::Mutex::new(data),
            agents: Default::default(),
            ptys: Default::default(),
            closing: std::sync::atomic::AtomicBool::new(false),
            attachment_picker: std::sync::atomic::AtomicBool::new(false),
            attachments: Default::default(),
            usage: Default::default(),
            accounts: Default::default(),
            close_guard: Default::default(),
            draft_checkpoint: Default::default(),
            checkpoint_pending: Default::default(),
            catalogs: Default::default(),
        });
        (state, homes.join("claude").join(&to), to)
    }

    fn request(account: &str) -> MoveSessionAccountRequest {
        MoveSessionAccountRequest {
            session_id: "s".into(),
            account_id: account.into(),
        }
    }

    #[test]
    fn a_move_copies_the_transcript_or_falls_back_to_a_recap() {
        for transcript in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let (state, target_home, target) = fixture(&root, transcript);
            // Unknown, foreign and not-signed-in accounts are refused before anything moves.
            assert!(move_native(&state, request("default")).is_err());
            assert!(move_native(&state, request(&uuid::Uuid::new_v4().to_string())).is_err());
            state
                .usage
                .remember_account(AgentProviderId::Claude, &target, None);
            assert!(move_native(&state, request(&target))
                .unwrap_err()
                .to_string()
                .contains("not signed in"));
            state.usage.remember_account(
                AgentProviderId::Claude,
                &target,
                Some("work@example.com"),
            );
            state.data.lock().sessions[0].status = crate::models::SessionStatus::Running;
            assert!(move_native(&state, request(&target)).is_err());
            state.data.lock().sessions[0].status = crate::models::SessionStatus::Failed;

            let reply = move_native(&state, request(&target)).unwrap();
            let data = state.data.lock();
            let moved = &data.sessions[0];
            assert_eq!(moved.provider_account_id, target);
            assert_eq!(reply["session"]["providerAccountId"], target.as_str());
            if transcript {
                assert_eq!(reply["outcome"], "carried");
                assert_eq!(
                    moved.native_thread.as_ref().unwrap().provider_account_id,
                    target
                );
                let copied = target_home
                    .join("projects")
                    .join(claude_project_name(
                        &moved.native_thread.as_ref().unwrap().cwd,
                    ))
                    .join("abc.jsonl");
                assert!(copied.is_file());
            } else {
                assert_eq!(reply["outcome"], "summary");
                assert!(moved.native_thread.is_none());
                assert!(moved
                    .handoff
                    .as_ref()
                    .is_some_and(|handoff| handoff.pending));
            }
            // The usage limit stays until the continue turn is sent.
            assert!(moved.usage_limit.is_some());
        }
    }
}
