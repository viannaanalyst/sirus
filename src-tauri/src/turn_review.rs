//! Native, bounded observations of workspace contents over one owned turn.
//! No checkout/index writes, attribution claim, raw paths from IPC or rollback authority.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::runtime::{AppHandle, State};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::commands::{session_cwd, AppState};
use crate::error::{Error, Result};
use crate::models::{ChangeKind, MessageRole, Session};

const MAX_NAMES: usize = 4096;
const MAX_FILE: usize = 512 * 1024;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_FILES: usize = 128;
const MAX_DIFF: usize = 64 * 1024;
const MAX_REVIEW: usize = 512 * 1024;
const RETAIN_DIFFS: usize = 16;
static CAPTURES: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFile {
    pub path: String,
    pub kind: ChangeKind,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
    pub diff: Option<String>,
    /// Only on what transcripts and session events carry (ADR-164): `diff` was left out and is
    /// fetched with `transcript_action { turnReview }`. Never stored.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub diff_omitted: bool,
    /// Set once the person undid this file's turn changes (`turn_undo.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undone_at: Option<String>,
    /// Set when a commit made through Sirus included this file (ADR-108): it leaves the turn's
    /// review and stays out even when the file changes again later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnReview {
    pub files: Vec<ReviewFile>,
    pub partial: bool,
    pub shared_workspace: bool,
    pub kept_at: Option<String>,
    #[serde(default)]
    pub expired: bool,
    /// Commit id of the workspace's `HEAD` when the turn settled; undo refuses once it moved (ADR-187).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
}
/// The workspace's current `HEAD` commit; `None` outside a repository or before the first commit.
pub(crate) fn head_of(cwd: &Path) -> Option<String> {
    let output =
        crate::git::bounded_probe(cwd, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok()?;
    let head = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (output.status.success() && !head.is_empty()).then_some(head)
}
/// Transcripts and session events leave diff texts out (ADR-164): they are most of a
/// settled turn's bytes and only the review pane reads them. Each file keeps its path, kind
/// and counts; the pane fetches the texts with `transcript_action { turnReview }`.
pub fn omit_diffs(message: &mut crate::models::Message) {
    let Some(review) = message
        .activity
        .as_mut()
        .and_then(|activity| activity.review.as_mut())
    else {
        return;
    };
    for file in &mut review.files {
        if file.diff.take().is_some() {
            file.diff_omitted = true;
        }
    }
}

/// [`omit_diffs`] on a message as JSON (session events are built as values).
pub fn omit_diffs_value(message: &mut serde_json::Value) {
    let Some(files) = message
        .pointer_mut("/activity/review/files")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for file in files
        .iter_mut()
        .filter_map(serde_json::Value::as_object_mut)
    {
        if file.get("diff").is_some_and(|diff| !diff.is_null()) {
            file.insert("diff".into(), serde_json::Value::Null);
            file.insert("diffOmitted".into(), serde_json::Value::Bool(true));
        }
    }
}

/// Paths a commit included, relative to the repository root.
pub fn committed_paths(cwd: &Path, hash: &str) -> std::collections::HashSet<String> {
    crate::git::run_ok(
        cwd,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            "--root",
            hash,
        ],
    )
    .map(|text| {
        text.split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

/// Marks the committed files in the turn reviews of every session working in `cwd`; returns
/// the sessions whose reviews changed.
pub fn mark_committed(
    data: &mut crate::models::AppData,
    cwd: &str,
    paths: &std::collections::HashSet<String>,
) -> Vec<String> {
    if paths.is_empty() {
        return Vec::new();
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut changed = Vec::new();
    for session in data
        .sessions
        .iter_mut()
        .filter(|session| session.worktree.path == cwd)
    {
        let mut touched = false;
        for message in session.messages.iter_mut() {
            let Some(review) = message
                .activity
                .as_mut()
                .and_then(|activity| activity.review.as_mut())
            else {
                continue;
            };
            for file in review
                .files
                .iter_mut()
                .filter(|file| file.committed_at.is_none() && paths.contains(&file.path))
            {
                file.committed_at = Some(now.clone());
                touched = true;
            }
        }
        if touched {
            changed.push(session.id.clone());
        }
    }
    changed
}

struct Slot;
impl Slot {
    fn acquire() -> Option<Self> {
        CAPTURES
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 4).then_some(n + 1)
            })
            .ok()
            .map(|_| Self)
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        CAPTURES.fetch_sub(1, Ordering::AcqRel);
    }
}
#[derive(Clone)]
enum Content {
    Missing,
    Unavailable,
    File {
        hash: [u8; 32],
        text: Option<String>,
        executable: bool,
    },
}
struct Snapshot {
    excluded: BTreeSet<String>,
    files: BTreeMap<String, Content>,
    complete: bool,
    partial: bool,
}
pub struct Capture {
    session_id: String,
    root: PathBuf,
    project_id: String,
    message_id: String,
    baseline: Option<Snapshot>,
    shared: bool,
    _slot: Option<Slot>,
}

pub(crate) fn allowed_name(name: &str) -> bool {
    name.len() <= 1024
        && !name.is_empty()
        && !name.chars().any(char::is_control)
        && Path::new(name)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && !name.split('/').any(|p| {
            crate::paths::is_ignored_dir(p)
                || p == ".env"
                || p.starts_with(".env.")
                || p == ".DS_Store"
        })
}
fn walk(root: &Path, dir: &Path, names: &mut BTreeSet<String>, deadline: Instant) -> bool {
    if Instant::now() >= deadline {
        return false;
    }
    let Ok(dir) = crate::paths::ensure_within(root, dir) else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut complete = true;
    let mut examined = 0;
    for entry in entries {
        examined += 1;
        if names.len() >= MAX_NAMES || examined > MAX_NAMES || Instant::now() >= deadline {
            return false;
        }
        let Ok(entry) = entry else {
            complete = false;
            continue;
        };
        let path = entry.path();
        let Some(name) = path.strip_prefix(root).ok().and_then(Path::to_str) else {
            complete = false;
            continue;
        };
        if !allowed_name(name) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            complete = false;
            continue;
        };
        if kind.is_dir() {
            complete &= walk(root, &path, names, deadline);
        } else {
            // Retain unavailable names (including links) to avoid invented deletions.
            names.insert(name.to_string());
        }
    }
    complete
}
fn inventory(root: &Path, deadline: Instant) -> (BTreeSet<String>, BTreeSet<String>, bool) {
    let mut names = BTreeSet::new();
    let mut excluded = BTreeSet::new();
    match crate::git::bounded_probe(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    ) {
        Ok(output) if output.status.success() => {
            let mut complete = true;
            for raw in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
                if Instant::now() >= deadline || names.len() >= MAX_NAMES {
                    complete = false;
                    break;
                }
                let Ok(name) = std::str::from_utf8(raw) else {
                    complete = false;
                    continue;
                };
                if allowed_name(name) {
                    names.insert(name.to_string());
                }
            }
            // Names/prefixes only: an ignored existing file becoming visible is not new.
            if Instant::now() >= deadline {
                return (names, excluded, false);
            }
            match crate::git::bounded_probe(
                root,
                &[
                    "ls-files",
                    "-z",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "--directory",
                ],
            ) {
                Ok(output) if output.status.success() => {
                    for raw in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
                        if Instant::now() >= deadline || excluded.len() >= MAX_NAMES {
                            complete = false;
                            break;
                        }
                        let Ok(name) = std::str::from_utf8(raw) else {
                            complete = false;
                            continue;
                        };
                        let name = name.trim_end_matches('/');
                        if allowed_name(name) {
                            excluded.insert(name.to_string());
                        }
                    }
                }
                _ => complete = false,
            }
            (names, excluded, complete)
        }
        Ok(output)
            if output.status.code() == Some(128)
                && String::from_utf8_lossy(&output.stderr).contains("not a git repository") =>
        {
            let complete = walk(root, root, &mut names, deadline);
            (names, excluded, complete)
        }
        _ => (names, excluded, false),
    }
}
fn read_content(root: &Path, name: &str, remaining: &mut usize) -> Content {
    let path = root.join(name);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Content::Missing,
        Err(_) => return Content::Unavailable,
    };
    if !metadata.is_file() || metadata.len() > MAX_FILE as u64 || metadata.len() > *remaining as u64
    {
        return Content::Unavailable;
    }
    let Ok(mut file) = crate::paths::open_regular_within(root, &path) else {
        return Content::Unavailable;
    };
    let Ok(before) = file.metadata() else {
        return Content::Unavailable;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != metadata.dev() || before.ino() != metadata.ino() {
            return Content::Unavailable;
        }
    }

    let mut bytes = Vec::new();
    if Read::by_ref(&mut file)
        .take((MAX_FILE + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_FILE
        || bytes.len() > *remaining
    {
        return Content::Unavailable;
    }
    let Ok(after) = file.metadata() else {
        return Content::Unavailable;
    };
    if before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || after.len() != bytes.len() as u64
        || crate::paths::verify_open_handle(root, &file).is_err()
    {
        return Content::Unavailable;
    }
    *remaining -= bytes.len();
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        after.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = false;
    let hash = Sha256::digest(&bytes).into();
    let text = if bytes.contains(&0) {
        None
    } else {
        String::from_utf8(bytes).ok()
    };
    Content::File {
        hash,
        text,
        executable,
    }
}
fn snapshot(root: &Path) -> Snapshot {
    snapshot_with_baseline(root, None)
}
fn snapshot_with_baseline(root: &Path, baseline: Option<&Snapshot>) -> Snapshot {
    let Ok(root) = crate::paths::ensure_dir(root) else {
        return Snapshot {
            files: BTreeMap::new(),
            excluded: BTreeSet::new(),
            complete: false,
            partial: true,
        };
    };
    let root = root.as_path();
    let deadline = Instant::now() + Duration::from_secs(3);
    let (mut names, excluded, mut complete) = inventory(root, deadline);
    if let Some(baseline) = baseline {
        // Git membership is not filesystem existence. Re-read admitted baseline names.
        names.extend(baseline.files.keys().cloned());
    }
    let mut files = BTreeMap::new();
    let mut remaining = MAX_BYTES;
    let mut partial = !complete;
    for name in names {
        if files.len() >= MAX_NAMES || Instant::now() >= deadline {
            complete = false;
            partial = true;
            break;
        }
        let content = read_content(root, &name, &mut remaining);
        partial |= matches!(content, Content::Unavailable);
        files.insert(name, content);
    }
    Snapshot {
        excluded,
        files,
        complete,
        partial,
    }
}
struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len() + bytes.len() > self.limit {
            return Err(std::io::Error::other("diff limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn compare(before: Snapshot, after: Snapshot, shared: bool) -> TurnReview {
    let mut review = TurnReview {
        files: vec![],
        partial: before.partial || after.partial,
        shared_workspace: shared,
        kept_at: None,
        expired: false,
        head: None,
    };
    let names: BTreeSet<_> = before.files.keys().chain(after.files.keys()).collect();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut remaining = MAX_REVIEW;
    for name in names {
        if review.files.len() >= MAX_FILES || Instant::now() >= deadline {
            review.partial = true;
            break;
        }
        if !before.files.contains_key(name)
            && before.excluded.iter().any(|prefix| {
                name == prefix
                    || name
                        .strip_prefix(prefix)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
        {
            review.partial = true;
            continue;
        }
        let old = before.files.get(name).or(if before.complete {
            Some(&Content::Missing)
        } else {
            None
        });
        let new = after.files.get(name).or(if after.complete {
            Some(&Content::Missing)
        } else {
            None
        });
        let (Some(old), Some(new)) = (old, new) else {
            review.partial = true;
            continue;
        };
        let (old_text, new_text, kind, binary) = match (old, new) {
            (
                Content::File {
                    hash: a,
                    executable: x,
                    ..
                },
                Content::File {
                    hash: b,
                    executable: y,
                    ..
                },
            ) if a == b && x == y => continue,
            (Content::Missing, Content::Missing) => continue,
            (Content::Unavailable, _) | (_, Content::Unavailable) => {
                review.partial = true;
                continue;
            }
            (Content::Missing, Content::File { text, .. }) => (
                "",
                text.as_deref().unwrap_or(""),
                ChangeKind::Added,
                text.is_none(),
            ),
            (Content::File { text, .. }, Content::Missing) => (
                text.as_deref().unwrap_or(""),
                "",
                ChangeKind::Deleted,
                text.is_none(),
            ),
            (Content::File { text: a, .. }, Content::File { text: b, .. }) => (
                a.as_deref().unwrap_or(""),
                b.as_deref().unwrap_or(""),
                ChangeKind::Modified,
                a.is_none() || b.is_none(),
            ),
        };
        let mut file = ReviewFile {
            diff_omitted: false,
            path: name.clone(),
            kind,
            additions: 0,
            deletions: 0,
            binary,
            diff: None,
            undone_at: None,
            committed_at: None,
        };
        if !binary {
            let diff = similar::TextDiff::configure()
                .deadline((Instant::now() + Duration::from_millis(100)).min(deadline))
                .diff_lines(old_text, new_text);
            for change in diff.iter_all_changes() {
                match change.tag() {
                    similar::ChangeTag::Insert => file.additions += 1,
                    similar::ChangeTag::Delete => file.deletions += 1,
                    _ => {}
                }
            }
            let mut writer = LimitedWriter {
                bytes: vec![],
                limit: remaining.min(MAX_DIFF),
            };
            if diff
                .unified_diff()
                .context_radius(3)
                .header(&format!("a/{name}"), &format!("b/{name}"))
                .to_writer(&mut writer)
                .is_ok()
            {
                remaining -= writer.bytes.len();
                file.diff = String::from_utf8(writer.bytes).ok();
            }
        }
        review.files.push(file);
    }
    review
}
impl Capture {
    pub fn begin(root: PathBuf, session: &Session) -> Self {
        let slot = Slot::acquire();
        let baseline = slot.as_ref().map(|_| snapshot(&root));
        Self {
            session_id: session.id.clone(),
            root,
            project_id: session.project_id.clone(),
            message_id: session
                .messages
                .iter()
                .rev()
                .find(|m| m.role == MessageRole::Agent)
                .map(|m| m.id.clone())
                .unwrap_or_default(),
            baseline,
            shared: !session.worktree.isolated,
            _slot: slot,
        }
    }
    fn owned(&self, state: &AppState, session_id: &str) -> bool {
        if self.session_id != session_id {
            return false;
        }
        let data = state.data.lock();
        data.sessions
            .iter()
            .find(|s| s.id == session_id)
            .is_some_and(|s| {
                s.project_id == self.project_id
                    && s.messages
                        .iter()
                        .rev()
                        .find(|m| m.role == MessageRole::Agent)
                        .is_some_and(|m| m.id == self.message_id)
                    && session_cwd(&data, s).is_ok_and(|root| root == self.root)
            })
    }
}
/// Runs before unregister/finality, so a follow-up cannot race the terminal snapshot.
pub async fn finish(
    capture: Option<Capture>,
    state: std::sync::Arc<AppState>,
    session_id: String,
) -> Option<(String, TurnReview)> {
    let capture = capture?;
    if state.closing.load(Ordering::Acquire) {
        return None;
    }
    tokio::task::spawn_blocking(move || {
        if !capture.owned(&state, &session_id) {
            return None;
        }
        let after = capture
            .baseline
            .as_ref()
            .map(|baseline| snapshot_with_baseline(&capture.root, Some(baseline)));
        if !capture.owned(&state, &session_id) {
            return None;
        }
        let review = match (capture.baseline, after) {
            (Some(before), Some(after)) => compare(before, after, capture.shared),
            _ => TurnReview {
                files: vec![],
                partial: true,
                shared_workspace: capture.shared,
                kept_at: None,
                expired: false,
                head: None,
            },
        };
        let mut review = review;
        review.head = head_of(&capture.root);
        Some((capture.message_id, review))
    })
    .await
    .ok()
    .flatten()
}
pub fn attach(session: &mut Session, result: Option<(String, TurnReview)>) {
    if let Some((id, review)) = result {
        if let Some(activity) = session
            .messages
            .iter_mut()
            .find(|m| m.id == id && m.session_id == session.id && m.role == MessageRole::Agent)
            .and_then(|m| m.activity.as_mut())
        {
            activity.review = Some(review);
        }
    }
    retain(session);
}
fn retain(session: &mut Session) {
    let mut count = 0;
    for review in session
        .messages
        .iter_mut()
        .rev()
        .filter_map(|m| m.activity.as_mut()?.review.as_mut())
    {
        if review.files.is_empty() {
            continue;
        }
        count += 1;
        if count > RETAIN_DIFFS {
            for file in &mut review.files {
                file.diff = None;
            }
            review.expired = true;
        }
    }
}
fn keep(session: &mut Session, message_id: &str) -> Result<TurnReview> {
    let review = session
        .messages
        .iter_mut()
        .find(|m| {
            m.id == message_id
                && m.session_id == session.id
                && m.role == MessageRole::Agent
                && !m.streaming
        })
        .and_then(|m| m.activity.as_mut())
        .filter(|a| a.ended_at.is_some())
        .and_then(|a| a.review.as_mut())
        .filter(|r| !r.files.is_empty())
        .ok_or_else(|| Error::not_found("turn review not available"))?;
    review.kept_at.get_or_insert_with(crate::paths::now_rfc3339);
    Ok(review.clone())
}
pub async fn keep_turn_changes(
    app: AppHandle,
    state: State<'_, std::sync::Arc<AppState>>,
    session_id: String,
    message_id: String,
) -> Result<TurnReview> {
    let state = state.inner().clone();
    let worker_state = state.clone();
    let owner = session_id.clone();
    let kept = message_id.clone();
    let review =
        crate::commands::native_task(move || keep_native(&worker_state, &owner, &kept)).await?;
    let data = state.data_with_messages(&[&session_id]);
    if let Some(session) = data.sessions.iter().find(|s| s.id == session_id) {
        // The acknowledged message may be older than the current turn.
        let from = session
            .messages
            .iter()
            .position(|message| message.id == message_id)
            .unwrap_or_else(|| crate::transcript_view::current_turn(session));
        crate::transcript_view::emit_from(&app, session, from);
    }
    Ok(review)
}
fn keep_native(state: &AppState, session_id: &str, message_id: &str) -> Result<TurnReview> {
    let mut data = state.data_with_messages(&[session_id]);
    state.ensure_running()?;
    let session = data
        .sessions
        .iter_mut()
        .find(|s| s.id == session_id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    let previous = session
        .messages
        .iter()
        .find(|m| m.id == message_id)
        .and_then(|m| m.activity.as_ref()?.review.as_ref()?.kept_at.clone());
    let review = keep(session, message_id)?;
    if let Err(error) = crate::persist::save(&state.data_path, &data) {
        if let Some(session) = data.sessions.iter_mut().find(|s| s.id == session_id) {
            if let Some(review) = session
                .messages
                .iter_mut()
                .find(|m| m.id == message_id)
                .and_then(|m| m.activity.as_mut()?.review.as_mut())
            {
                review.kept_at = previous;
            }
        }
        return Err(error);
    }
    Ok(review)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `SIRUS_BENCH_ROOT=<repo> cargo test --lib bench_snapshot -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_snapshot() {
        let root = std::env::var("SIRUS_BENCH_ROOT").unwrap();
        for _ in 0..3 {
            let started = Instant::now();
            let snapshot = snapshot(Path::new(&root));
            eprintln!(
                "snapshot {} files in {:?}",
                snapshot.files.len(),
                started.elapsed()
            );
        }
    }

    #[test]
    fn committed_files_leave_reviews_of_sessions_in_that_workspace_only() {
        let review = serde_json::json!({"files":[{"path":"a.ts","kind":"modified","additions":1,"deletions":0,"binary":false,"diff":null},{"path":"b.ts","kind":"added","additions":2,"deletions":0,"binary":false,"diff":null}],"partial":false,"sharedWorkspace":false,"keptAt":null});
        let session = |id: &str, path: &str| {
            serde_json::json!({"id":id,"projectId":"p","title":"t","agent":"codex","status":"completed","createdAt":"t","lastActivityAt":"t","worktree":{"path":path,"branch":"main","isolated":false},"lastError":null,
            "messages":[{"id":"m","sessionId":id,"role":"agent","content":"done","createdAt":"t","streaming":false,"activity":{"provider":"codex","model":null,"startedAt":0,"endedAt":1,"waitingSince":null,"pausedMs":0,"status":"completed","items":[],"truncated":false,"review":review}}]})
        };
        let mut data: crate::models::AppData = serde_json::from_value(serde_json::json!({"settings":{},"projects":[],"sessions":[session("here","/w"),session("other","/x")]})).unwrap();
        let paths = ["a.ts".to_string()].into_iter().collect();
        assert_eq!(mark_committed(&mut data, "/w", &paths), ["here"]);
        let files = |index: usize| {
            data.sessions[index].messages[0]
                .activity
                .as_ref()
                .unwrap()
                .review
                .as_ref()
                .unwrap()
                .files
                .clone()
        };
        assert!(files(0)[0].committed_at.is_some());
        assert!(files(0)[1].committed_at.is_none());
        assert!(files(1)[0].committed_at.is_none());
        // Already committed files are not marked again.
        assert!(mark_committed(&mut data, "/w", &paths).is_empty());
    }
    fn fixture_session(root: &Path) -> Session {
        let mut session: Session = serde_json::from_value(serde_json::json!({
            "id":"s","projectId":"p","title":"Task","agent":"codex","status":"completed","createdAt":"time","lastActivityAt":"time",
            "worktree":{"path":crate::paths::display_path(root),"branch":"main","isolated":false},
            "messages":[{"id":"answer","sessionId":"s","role":"agent","content":"Done","createdAt":"time","streaming":false}]
        })).unwrap();
        session.messages[0].activity = Some(crate::activity::TurnActivity::new(
            crate::models::AgentProviderId::Codex,
            None,
        ));
        crate::activity::sync(&mut session);
        session
    }
    fn fixture_review() -> TurnReview {
        TurnReview {
            files: vec![ReviewFile {
                diff_omitted: false,
                path: "a.txt".into(),
                kind: ChangeKind::Modified,
                additions: 1,
                deletions: 1,
                binary: false,
                diff: Some("-before\n+after\n".into()),
                undone_at: None,
                committed_at: None,
            }],
            partial: false,
            shared_workspace: true,
            kept_at: None,
            expired: false,
            head: None,
        }
    }
    fn fixture_state(root: &Path) -> std::sync::Arc<AppState> {
        let project = crate::models::Project {
            id: "p".into(),
            name: "Fixture".into(),
            path: crate::paths::display_path(root),
            added_at: "time".into(),
            last_opened_at: "time".into(),
            look: Default::default(),
            appearance: None,
            scripts: Default::default(),
        };
        let data = crate::models::AppData {
            projects: vec![project],
            sessions: vec![fixture_session(root)],
            ..Default::default()
        };
        std::sync::Arc::new(AppState {
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
        })
    }
    #[tokio::test]
    async fn endpoint_publication_revalidates_session_response_and_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let state = fixture_state(&root);
        std::fs::write(root.join("file.txt"), "before\n").unwrap();
        let session = state.data.lock().sessions[0].clone();
        let capture = Capture::begin(root.clone(), &session);
        assert!(!capture.owned(&state, "foreign"));
        std::fs::write(root.join("file.txt"), "after\n").unwrap();
        let result = finish(Some(capture), state.clone(), "s".into())
            .await
            .unwrap();
        assert!(result
            .1
            .files
            .iter()
            .any(|f| f.path == "file.txt"
                && f.diff.as_ref().is_some_and(|diff| diff.contains("+after"))));
        let capture = Capture::begin(root.clone(), &session);
        state.data.lock().sessions[0].messages[0].id = "replacement".into();
        assert!(finish(Some(capture), state.clone(), "s".into())
            .await
            .is_none());
        state.data.lock().sessions[0] = session.clone();
        let capture = Capture::begin(root.clone(), &session);
        std::fs::create_dir(root.join("nested")).unwrap();
        state.data.lock().sessions[0].worktree.path =
            crate::paths::display_path(&root.join("nested"));
        assert!(finish(Some(capture), state.clone(), "s".into())
            .await
            .is_none());
        state.data.lock().sessions[0] = session.clone();
        let capture = Capture::begin(root.clone(), &session);
        state.data.lock().projects.clear();
        assert!(finish(Some(capture), state, "s".into()).await.is_none());
    }
    #[test]
    fn keep_is_idempotent_owner_bound_and_survives_persistence() {
        let root = tempfile::tempdir().unwrap();
        let mut session = fixture_session(root.path());
        attach(&mut session, Some(("foreign".into(), fixture_review())));
        assert!(session.messages[0]
            .activity
            .as_ref()
            .unwrap()
            .review
            .is_none());
        attach(&mut session, Some(("answer".into(), fixture_review())));
        assert!(keep(&mut session, "foreign").is_err());
        session.messages[0].session_id = "foreign".into();
        assert!(keep(&mut session, "answer").is_err());
        session.messages[0].session_id = "s".into();
        session.messages[0].streaming = true;
        assert!(keep(&mut session, "answer").is_err());
        session.messages[0].streaming = false;
        let first = keep(&mut session, "answer").unwrap().kept_at;
        assert!(first.is_some());
        assert_eq!(first, keep(&mut session, "answer").unwrap().kept_at);
        let path = root.path().join("state.json");
        let data = crate::models::AppData {
            sessions: vec![session.clone()],
            ..Default::default()
        };
        crate::persist::save(&path, &data).unwrap();
        let loaded = crate::persist::load_or_create(&path).unwrap();
        assert_eq!(
            first,
            loaded.sessions[0].messages[0]
                .activity
                .as_ref()
                .unwrap()
                .review
                .as_ref()
                .unwrap()
                .kept_at
        );
        let fork = crate::transcript::fork_snapshot(
            &session,
            "answer",
            "fork",
            "time",
            session.worktree.clone(),
        )
        .unwrap();
        assert!(fork.messages[0].activity.as_ref().unwrap().review.is_none());
    }
    #[test]
    fn failed_keep_save_rolls_back_acknowledgment_for_retry() {
        let root = tempfile::tempdir().unwrap();
        let mut state = fixture_state(root.path());
        let blocked = root.path().join("blocked");
        std::fs::write(&blocked, "regular file, not directory").unwrap();
        std::sync::Arc::get_mut(&mut state).unwrap().data_path = blocked.join("state.json");
        attach(
            &mut state.data.lock().sessions[0],
            Some(("answer".into(), fixture_review())),
        );
        assert!(keep_native(&state, "s", "answer").is_err());
        assert!(state.data.lock().sessions[0].messages[0]
            .activity
            .as_ref()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .kept_at
            .is_none());
        std::sync::Arc::get_mut(&mut state).unwrap().data_path = root.path().join("state.json");
        assert!(keep_native(&state, "s", "answer")
            .unwrap()
            .kept_at
            .is_some());
        assert!(keep_native(&state, "foreign", "answer").is_err());
    }
    #[test]
    fn old_diffs_expire_without_losing_counts_or_acceptance() {
        let root = tempfile::tempdir().unwrap();
        let mut session = fixture_session(root.path());
        attach(&mut session, Some(("answer".into(), fixture_review())));
        keep(&mut session, "answer").unwrap();
        for i in 0..RETAIN_DIFFS {
            let mut message = session.messages[0].clone();
            message.id = format!("next-{i}");
            message.activity.as_mut().unwrap().review = Some(fixture_review());
            session.messages.push(message);
        }
        retain(&mut session);
        let old = session.messages[0]
            .activity
            .as_ref()
            .unwrap()
            .review
            .as_ref()
            .unwrap();
        assert!(old.expired);
        assert!(old.files[0].diff.is_none());
        assert_eq!((old.files[0].additions, old.files[0].deletions), (1, 1));
        assert!(old.kept_at.is_some());
        assert!(session.messages[1]
            .activity
            .as_ref()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .files[0]
            .diff
            .is_some());
    }
    #[test]
    fn diff_and_file_limits_preserve_honest_available_counts() {
        let mut before = Snapshot {
            files: BTreeMap::new(),
            excluded: BTreeSet::new(),
            complete: true,
            partial: false,
        };
        let mut after = Snapshot {
            files: BTreeMap::new(),
            excluded: BTreeSet::new(),
            complete: true,
            partial: false,
        };
        let huge = "a".repeat(MAX_DIFF);
        before.files.insert(
            "a.txt".into(),
            Content::File {
                hash: [1; 32],
                text: Some(huge.clone()),
                executable: false,
            },
        );
        after.files.insert(
            "a.txt".into(),
            Content::File {
                hash: [2; 32],
                text: Some(huge.replace('a', "b")),
                executable: false,
            },
        );
        for i in 0..MAX_FILES {
            after.files.insert(
                format!("new-{i}.txt"),
                Content::File {
                    hash: [1; 32],
                    text: Some("new\n".into()),
                    executable: false,
                },
            );
        }
        let review = compare(before, after, false);
        assert!(review.partial);
        assert_eq!(review.files.len(), MAX_FILES);
        assert_eq!(
            (review.files[0].additions, review.files[0].deletions),
            (1, 1)
        );
        assert!(review.files[0].diff.is_none());
        assert!(
            review
                .files
                .iter()
                .filter_map(|f| f.diff.as_ref())
                .map(String::len)
                .sum::<usize>()
                <= MAX_REVIEW
        );
    }
    #[test]
    fn dirty_baseline_is_excluded_and_history_is_immutable() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("dirty.txt"), "existing dirty\n").unwrap();
        std::fs::write(root.path().join("edit.txt"), "user edit\nunchanged\n").unwrap();
        std::fs::write(root.path().join("delete.txt"), "gone\n").unwrap();
        let before = snapshot(root.path());
        std::fs::write(root.path().join("edit.txt"), "turn edit\nunchanged\n").unwrap();
        std::fs::remove_file(root.path().join("delete.txt")).unwrap();
        std::fs::write(root.path().join("new.txt"), "new\n").unwrap();
        std::fs::write(root.path().join("binary.bin"), [0, 1, 2]).unwrap();
        let review = compare(before, snapshot(root.path()), true);
        assert!(!review.partial);
        assert_eq!(review.files.len(), 4);
        let edit = review.files.iter().find(|f| f.path == "edit.txt").unwrap();
        assert_eq!((edit.additions, edit.deletions), (1, 1));
        assert!(edit.diff.as_ref().unwrap().contains("-user edit"));
        assert!(!review.files.iter().any(|f| f.path == "dirty.txt"));
        assert!(
            review
                .files
                .iter()
                .find(|f| f.path == "binary.bin")
                .unwrap()
                .binary
        );
        let encoded = serde_json::to_string(&review).unwrap();
        std::fs::write(root.path().join("edit.txt"), "later turn\n").unwrap();
        assert_eq!(encoded, serde_json::to_string(&review).unwrap());
    }
    #[test]
    fn incomplete_inventory_never_invents_added_or_deleted_files() {
        let before = Snapshot {
            files: BTreeMap::new(),
            excluded: BTreeSet::new(),
            complete: false,
            partial: true,
        };
        let mut after = Snapshot {
            files: BTreeMap::new(),
            excluded: BTreeSet::new(),
            complete: true,
            partial: false,
        };
        after.files.insert(
            "unknown.txt".into(),
            Content::File {
                hash: [1; 32],
                text: Some("x\n".into()),
                executable: false,
            },
        );
        let review = compare(before, after, false);
        assert!(review.partial);
        assert!(review.files.is_empty());
    }
    #[test]
    fn limits_links_and_private_names_do_not_leak_contents() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join("link.txt"))
            .unwrap();
        std::fs::write(root.path().join("huge.txt"), vec![b'x'; MAX_FILE + 1]).unwrap();
        std::fs::write(root.path().join(".env"), "credential").unwrap();
        let snap = snapshot(root.path());
        assert!(snap.partial);
        assert!(matches!(snap.files["huge.txt"], Content::Unavailable));
        assert!(!snap.files.contains_key(".env"));
        #[cfg(unix)]
        assert!(matches!(snap.files["link.txt"], Content::Unavailable));
        assert!(!allowed_name("../outside"));
        assert!(!allowed_name(".git/config"));
        assert!(!allowed_name("/absolute"));
    }
    #[test]
    fn ignore_membership_changes_never_invent_file_creation_or_deletion() {
        let repo = crate::git::tests::Repo::new();
        let root = repo.cwd();
        std::fs::write(root.join("existing.txt"), "user content\n").unwrap();
        std::fs::create_dir(root.join("hidden")).unwrap();
        std::fs::write(root.join("hidden/existing.txt"), "private existing\n").unwrap();
        let before = snapshot(&root);
        std::fs::write(root.join(".gitignore"), "existing.txt\nhidden/\n").unwrap();
        let after = snapshot_with_baseline(&root, Some(&before));
        let review = compare(before, after, true);
        assert!(!review
            .files
            .iter()
            .any(|f| f.path == "existing.txt" || f.path == "hidden/existing.txt"));
        let before = snapshot(&root);
        assert!(before.excluded.contains("existing.txt"));
        assert!(before.excluded.contains("hidden"));
        std::fs::write(root.join(".gitignore"), "").unwrap();
        std::fs::write(root.join("new.txt"), "actually new\n").unwrap();
        std::fs::write(root.join("hidden-similar.txt"), "actually new\n").unwrap();
        let after = snapshot_with_baseline(&root, Some(&before));
        let review = compare(before, after, true);
        assert!(review.partial);
        assert!(!review
            .files
            .iter()
            .any(|f| f.path == "existing.txt" || f.path == "hidden/existing.txt"));
        assert!(review
            .files
            .iter()
            .any(|f| f.path == "new.txt" && matches!(f.kind, ChangeKind::Added)));
        assert!(review
            .files
            .iter()
            .any(|f| f.path == "hidden-similar.txt" && matches!(f.kind, ChangeKind::Added)));
        // An ignored tracked path remains on disk after index removal.
        crate::git::run_ok(&root, &["add", "--", "existing.txt"]).unwrap();
        std::fs::write(root.join(".gitignore"), "existing.txt\n").unwrap();
        let before = snapshot(&root);
        crate::git::run_ok(&root, &["rm", "--cached", "--", "existing.txt"]).unwrap();
        let after = snapshot_with_baseline(&root, Some(&before));
        let review = compare(before, after, false);
        assert!(!review.files.iter().any(|f| f.path == "existing.txt"));
    }
    #[test]
    fn git_inventory_respects_ignored_untracked_files_without_touching_index() {
        let repo = crate::git::tests::Repo::new();
        std::fs::write(repo.cwd().join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(repo.cwd().join("ignored.txt"), "secret").unwrap();
        std::fs::write(repo.cwd().join("visible.txt"), "visible").unwrap();
        let before = std::fs::read(repo.cwd().join(".git/index")).ok();
        let snap = snapshot(&repo.cwd());
        assert!(snap.files.contains_key("visible.txt"));
        assert!(!snap.files.contains_key("ignored.txt"));
        assert_eq!(before, std::fs::read(repo.cwd().join(".git/index")).ok());
    }
}
