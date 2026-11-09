//! Session-owned, bounded index preparation and local history. Never accepts Git argv.
use crate::error::{Error, Result};
use std::path::Path;

use crate::models::{ChangeKind, GitIdentity};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Component, PathBuf};

pub(crate) static INDEX_WRITE: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
const FILE_LIMIT: usize = 200;
const HISTORY_LIMIT: usize = 50;
/// Older pages stop here; the view never walks unbounded ancestry.
const HISTORY_MAX_SKIP: u32 = 5_000;
#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    Snapshot {
        session_id: String,
    },
    Stage {
        session_id: String,
        paths: Vec<String>,
        expected_index: String,
    },
    Unstage {
        session_id: String,
        paths: Vec<String>,
        expected_index: String,
    },
    /// Discards every change to the listed paths (staged and unstaged). Deleting files Git does
    /// not hold needs `confirm_delete`; Git cannot restore them.
    Discard {
        session_id: String,
        paths: Vec<String>,
        expected_index: String,
        confirm_delete: bool,
    },
    Diff {
        session_id: String,
        path: String,
        staged: bool,
        expected_index: String,
    },
    /// The next page of ancestry from the exact commit the view was loaded at, so new commits
    /// cannot shift or repeat rows.
    History {
        session_id: String,
        from: String,
        skip: u32,
    },
    /// Explicit fast-forward-only pull of the session branch from origin.
    Pull {
        session_id: String,
        confirm: bool,
    },
}
impl Action {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Snapshot { session_id }
            | Self::Stage { session_id, .. }
            | Self::Unstage { session_id, .. }
            | Self::Discard { session_id, .. }
            | Self::Diff { session_id, .. }
            | Self::History { session_id, .. }
            | Self::Pull { session_id, .. } => session_id,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Response {
    Snapshot {
        snapshot: Snapshot,
    },
    Diff {
        diff: String,
    },
    History {
        entries: Vec<HistoryEntry>,
        truncated: bool,
    },
    Pulled {
        branch: String,
        summary: String,
    },
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub path: String,
    pub kind: ChangeKind,
    pub conflicted: bool,
    pub actionable: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub hash: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    pub authored_at: String,
    pub refs: Vec<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub identity: GitIdentity,
    pub head: Option<String>,
    pub index_token: String,
    pub staged: Vec<Entry>,
    pub unstaged: Vec<Entry>,
    pub truncated: bool,
    pub conflicts: bool,
    pub outside_scope: bool,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub history: Vec<HistoryEntry>,
    pub history_truncated: bool,
}
fn output(cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let result = crate::git::bounded_run(cwd, args)?;
    if !result.status.success() {
        return Err(Error::git(
            String::from_utf8_lossy(&result.stderr).trim().to_string(),
        ));
    }
    Ok(result.stdout)
}
fn text(cwd: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(output(cwd, args)?)
        .map(|s| s.trim_end_matches('\n').to_owned())
        .map_err(|_| Error::git("Git metadata is not UTF-8"))
}
fn head(cwd: &Path) -> Result<Option<String>> {
    let result = crate::git::bounded_run(cwd, &["rev-parse", "--verify", "--quiet", "HEAD"])?;
    if result.status.success() {
        Ok(Some(String::from_utf8_lossy(&result.stdout).trim().into()))
    } else if result.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(Error::git("Cannot read HEAD"))
    }
}
fn index_token(cwd: &Path, head: &Option<String>) -> Result<String> {
    let root = std::fs::canonicalize(text(cwd, &["rev-parse", "--show-toplevel"])?)?;
    let index = output(&root, &["ls-files", "--stage", "-z", "--full-name"])?;
    let mut digest = Sha256::new();
    digest.update(cwd.as_os_str().as_encoded_bytes());
    digest.update([0]);
    digest.update(text(cwd, &["rev-parse", "--absolute-git-dir"])?);
    digest.update([0]);
    digest.update(head.as_deref().unwrap_or("unborn"));
    digest.update([0]);
    digest.update(crate::git::bounded_run(cwd, &["symbolic-ref", "--quiet", "HEAD"])?.stdout);
    digest.update([0]);
    digest.update(index);
    Ok(format!("{:x}", digest.finalize()))
}
fn verify_token(cwd: &Path, expected: &str) -> Result<()> {
    if expected.len() != 64 || index_token(cwd, &head(cwd)?)? != expected {
        return Err(Error::git(
            "Git preparation changed. Refresh before continuing.",
        ));
    }
    Ok(())
}
fn valid_name(path: &str) -> bool {
    !path.is_empty() && path.len() <= 4096 && !path.contains('\\') && !path.chars().any(char::is_control)
        && Path::new(path).components().all(|c| matches!(c, Component::Normal(v) if !v.to_string_lossy().eq_ignore_ascii_case(".git")))
        && path.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}
/// No linked components, directories or submodules; deleted leaves remain eligible.
fn validate_path(cwd: &Path, path: &str) -> Result<()> {
    if !valid_name(path) {
        return Err(Error::invalid_path("Invalid workspace Git path"));
    }
    let mut current = cwd.to_path_buf();
    let components = Path::new(path).components().collect::<Vec<_>>();
    for (i, part) in components.iter().enumerate() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink()
                    || (i + 1 == components.len() && !meta.is_file())
                    || (i + 1 < components.len() && !meta.is_dir())
                {
                    return Err(Error::invalid_path(
                        "Linked paths, directories and submodules cannot be prepared",
                    ));
                }
                if !std::fs::canonicalize(&current)?.starts_with(cwd) {
                    return Err(Error::invalid_path("Workspace path changed"));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => break,
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}
fn kind(code: u8) -> ChangeKind {
    match code {
        b'A' => ChangeKind::Added,
        b'D' => ChangeKind::Deleted,
        b'?' => ChangeKind::Untracked,
        _ => ChangeKind::Modified,
    }
}
fn history(cwd: &Path, exists: bool) -> Result<(Vec<HistoryEntry>, bool)> {
    if !exists {
        return Ok((vec![], false));
    }
    history_page(cwd, "HEAD", 0)
}

/// One bounded page of ancestry starting `skip` commits below `from` (a full commit hash).
pub fn history_from(cwd: &Path, from: &str, skip: u32) -> Result<(Vec<HistoryEntry>, bool)> {
    if !matches!(from.len(), 40 | 64) || !from.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::git("Invalid Git history start"));
    }
    if skip > HISTORY_MAX_SKIP {
        return Ok((vec![], false));
    }
    history_page(cwd, from, skip)
}

fn history_page(cwd: &Path, from: &str, skip: u32) -> Result<(Vec<HistoryEntry>, bool)> {
    let skip_arg = format!("--skip={skip}");
    // NUL fields cannot occur in valid commit metadata. Never interpret decoration as argv.
    let raw = output(
        cwd,
        &[
            "log",
            "--topo-order",
            "-51",
            &skip_arg,
            "--no-show-signature",
            "--format=%H%x00%P%x00%s%x00%an%x00%aI%x00%D%x00",
            from,
            "--",
        ],
    )?;
    let mut fields = raw.split(|c| *c == 0);
    let mut entries = vec![];
    while let Some(hash) = fields.next() {
        let hash = String::from_utf8_lossy(hash).trim().to_owned();
        if hash.is_empty() {
            break;
        }
        let mut next = || {
            fields
                .next()
                .ok_or_else(|| Error::git("Incomplete Git history"))
        };
        let parents: Vec<String> = String::from_utf8_lossy(next()?)
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let valid_hash = |value: &str| {
            matches!(value.len(), 40 | 64) && value.bytes().all(|c| c.is_ascii_hexdigit())
        };
        if !valid_hash(&hash) || parents.len() > 128 || parents.iter().any(|p| !valid_hash(p)) {
            return Err(Error::git("Invalid or excessive Git history metadata"));
        }
        let bounded = |bytes: &[u8], limit| {
            String::from_utf8_lossy(bytes)
                .chars()
                .filter(|c| !c.is_control())
                .take(limit)
                .collect::<String>()
        };
        let subject = bounded(next()?, 300);
        let author = bounded(next()?, 120);
        let authored_at = bounded(next()?, 40);
        let refs = bounded(next()?, 1024)
            .split(", ")
            .filter(|s| !s.is_empty())
            .take(12)
            .map(str::to_owned)
            .collect();
        entries.push(HistoryEntry {
            hash,
            parents,
            subject,
            author,
            authored_at,
            refs,
        });
    }
    // Commit messages are untrusted bytes. Independently bind parsed rows to
    // Git's hash-only ancestry so malformed NUL metadata cannot invent rows.
    let ancestry = text(
        cwd,
        &[
            "rev-list",
            "--topo-order",
            "--max-count=51",
            &skip_arg,
            "--parents",
            from,
            "--",
        ],
    )?;
    let expected = ancestry
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .collect::<Vec<_>>();
    if expected.len() != entries.len()
        || expected.iter().zip(&entries).any(|(ids, entry)| {
            ids.first().copied() != Some(entry.hash.as_str())
                || ids.get(1..).unwrap_or_default()
                    != entry.parents.iter().map(String::as_str).collect::<Vec<_>>()
        })
    {
        return Err(Error::git("Invalid or excessive Git history metadata"));
    }
    let truncated = entries.len() > HISTORY_LIMIT;
    entries.truncate(HISTORY_LIMIT);
    Ok((entries, truncated))
}
pub fn snapshot(cwd: &Path) -> Result<Snapshot> {
    let cwd = std::fs::canonicalize(cwd)?;
    let root = PathBuf::from(text(&cwd, &["rev-parse", "--show-toplevel"])?);
    let root = std::fs::canonicalize(root)?;
    let prefix = cwd
        .strip_prefix(&root)
        .map_err(|_| Error::invalid_path("Workspace is outside repository"))?;
    let initial_head = head(&cwd)?;
    let token = index_token(&cwd, &initial_head)?;
    let branch_result =
        crate::git::bounded_run(&cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let detached = !branch_result.status.success();
    let branch = if detached {
        initial_head.as_ref().map(|s| s.chars().take(10).collect())
    } else {
        Some(
            String::from_utf8_lossy(&branch_result.stdout)
                .trim()
                .to_owned(),
        )
    };
    let index = output(&root, &["ls-files", "--stage", "-z", "--full-name"])?;
    // A staged gitlink deletion has already disappeared from the index. Keep
    // HEAD identities too, so unstage/commit cannot admit that hidden submodule.
    let head_tree = if initial_head.is_some() {
        output(&root, &["ls-tree", "-r", "-z", "--full-tree", "HEAD", "--"])?
    } else {
        vec![]
    };
    let submodules: HashSet<&[u8]> = index
        .split(|c| *c == 0)
        .chain(head_tree.split(|c| *c == 0))
        .filter(|record| record.starts_with(b"160000 "))
        .filter_map(|record| {
            record
                .iter()
                .position(|c| *c == b'\t')
                .map(|position| &record[position + 1..])
        })
        .collect();
    // Disable rename collapsing: old deletion and new addition are separately explicit paths.
    let raw = output(
        &root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--ignore-submodules=none",
        ],
    )?;
    let mut staged = vec![];
    let mut unstaged = vec![];
    let mut conflicts = false;
    let mut outside_scope = false;
    for record in raw.split(|c| *c == 0).filter(|r| !r.is_empty()) {
        if record.len() < 4 || record[2] != b' ' {
            return Err(Error::git("Invalid Git status"));
        }
        let path =
            std::str::from_utf8(&record[3..]).map_err(|_| Error::git("Git path is not UTF-8"))?;
        let x = record[0];
        let y = record[1];
        if !b" MADRCU?!T".contains(&x) || !b" MADRCU?!T".contains(&y) {
            return Err(Error::git("Invalid Git status"));
        }
        let conflict =
            x == b'U' || y == b'U' || (x == b'A' && y == b'A') || (x == b'D' && y == b'D');
        conflicts |= conflict;
        let Ok(relative) = Path::new(path).strip_prefix(prefix) else {
            outside_scope |= x != b' ' && x != b'?';
            continue;
        };
        let relative = relative.to_string_lossy().into_owned();
        let actionable = !conflict
            && !submodules.contains(path.as_bytes())
            && validate_path(&cwd, &relative).is_ok();
        if x != b' ' && x != b'?' {
            staged.push(Entry {
                path: relative.clone(),
                kind: kind(x),
                conflicted: conflict,
                actionable,
            });
        }
        if y != b' ' {
            unstaged.push(Entry {
                path: relative,
                kind: kind(y),
                conflicted: conflict,
                actionable,
            });
        }
    }
    let truncated = staged.len() + unstaged.len() > FILE_LIMIT;
    staged.truncate(FILE_LIMIT);
    unstaged.truncate(FILE_LIMIT.saturating_sub(staged.len()));
    let (history, history_truncated) = history(&cwd, initial_head.is_some())?;
    let counts = crate::git::bounded_run(
        &cwd,
        &[
            "rev-list",
            "--left-right",
            "--count",
            "HEAD...@{upstream}",
            "--",
        ],
    )?;
    let values = String::from_utf8_lossy(&counts.stdout)
        .split_whitespace()
        .filter_map(|v| v.parse::<u32>().ok())
        .collect::<Vec<_>>();
    let (ahead, behind) = if counts.status.success() && values.len() == 2 {
        (Some(values[0]), Some(values[1]))
    } else {
        (None, None)
    };
    verify_token(&cwd, &token)?;
    Ok(Snapshot {
        identity: GitIdentity {
            is_repo: true,
            root: Some(root.to_string_lossy().into_owned()),
            branch,
            detached,
        },
        head: initial_head,
        index_token: token,
        staged,
        unstaged,
        truncated,
        conflicts,
        outside_scope,
        ahead,
        behind,
        history,
        history_truncated,
    })
}
#[cfg(test)]
fn prepare(cwd: &Path, paths: &[String], stage: bool, expected: &str) -> Result<Snapshot> {
    prepare_admitted(cwd, paths, stage, expected, || Ok(()))
}

pub fn prepare_admitted(
    cwd: &Path,
    paths: &[String],
    stage: bool,
    expected: &str,
    admit: impl FnOnce() -> Result<()>,
) -> Result<Snapshot> {
    let _write = INDEX_WRITE.lock();
    let cwd = std::fs::canonicalize(cwd)?;
    let current = snapshot(&cwd)?;
    verify_token(&cwd, expected)?;
    if current.truncated || current.conflicts || paths.is_empty() || paths.len() > FILE_LIMIT {
        return Err(Error::git(
            "Refresh and resolve incomplete or conflicted Git changes before preparing files",
        ));
    }
    let entries = if stage {
        &current.unstaged
    } else {
        &current.staged
    };
    let mut unique = HashSet::new();
    for path in paths {
        if !unique.insert(path) || !entries.iter().any(|e| e.path == *path && e.actionable) {
            return Err(Error::invalid_path(
                "Git file is not an eligible workspace change",
            ));
        }
        validate_path(&cwd, path)?;
    }
    let mut args = if stage {
        vec!["add", "--"]
    } else {
        vec!["reset", "-q", "--"]
    };
    args.extend(paths.iter().map(String::as_str));
    verify_token(&cwd, expected)?;
    admit()?;
    output(&cwd, &args)?;
    snapshot(&cwd)
}
/// One Git or filesystem step that discards part of a path. The argv always carries `--` before
/// the path, and literal pathspecs are already on for every Git command (`git::git_command`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiscardStep {
    /// `git restore --staged`: unstage, keeping the working file.
    RestoreStaged,
    /// `git restore --worktree`: the index's content back into the working file.
    RestoreWorktree,
    /// `git rm --cached -f`: a new file leaves the index (it is then an untracked file).
    RemoveFromIndex,
    /// Removes a file Git does not hold any more. Not recoverable from Git.
    DeleteFile,
}

impl DiscardStep {
    /// The Git argv for this step, or None for the filesystem step.
    pub(crate) fn argv(self, path: &str) -> Option<Vec<&str>> {
        match self {
            Self::RestoreStaged => Some(vec!["restore", "--staged", "--", path]),
            Self::RestoreWorktree => Some(vec!["restore", "--worktree", "--", path]),
            Self::RemoveFromIndex => Some(vec!["rm", "--cached", "-f", "-q", "--", path]),
            Self::DeleteFile => None,
        }
    }
}

/// Steps for one path from its staged and unstaged kinds in the snapshot. Renames never reach
/// here: the snapshot lists them as an old-path deletion and a new-path addition.
pub(crate) fn discard_steps(
    staged: Option<&ChangeKind>,
    unstaged: Option<&ChangeKind>,
) -> Result<Vec<DiscardStep>> {
    use ChangeKind::{Added, Deleted, Modified, Untracked};
    let tracked = |kind: Option<&ChangeKind>| matches!(kind, None | Some(Modified | Deleted));
    match (staged, unstaged) {
        (Some(Added), u) if tracked(u) => {
            Ok(vec![DiscardStep::RemoveFromIndex, DiscardStep::DeleteFile])
        }
        (Some(Modified | Deleted), u) if tracked(u) => Ok(vec![
            DiscardStep::RestoreStaged,
            DiscardStep::RestoreWorktree,
        ]),
        (None, Some(Untracked)) => Ok(vec![DiscardStep::DeleteFile]),
        (None, Some(Modified | Deleted)) => Ok(vec![DiscardStep::RestoreWorktree]),
        // Both a staged deletion and a new untracked file at one path: restoring would overwrite it.
        _ => Err(Error::invalid_path(
            "Git file is not an eligible workspace change",
        )),
    }
}

/// Whether discarding this path deletes a file Git cannot restore.
pub(crate) fn discard_deletes(steps: &[DiscardStep]) -> bool {
    steps.contains(&DiscardStep::DeleteFile)
}

fn delete_file(cwd: &Path, path: &str) -> Result<()> {
    // Checked again right before deleting, so a link swapped in since the plan is never followed.
    validate_path(cwd, path)?;
    match std::fs::symlink_metadata(cwd.join(path)) {
        Ok(meta) if meta.is_file() => Ok(std::fs::remove_file(cwd.join(path))?),
        Ok(_) => Err(Error::invalid_path("Only files can be discarded")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Discards every change to `paths` under the index lock, after the same token, conflict and
/// eligibility checks as preparation. Plans come from the snapshot taken here, not from the caller.
pub fn discard_admitted(
    cwd: &Path,
    paths: &[String],
    expected: &str,
    confirm_delete: bool,
    admit: impl FnOnce() -> Result<()>,
) -> Result<Snapshot> {
    let _write = INDEX_WRITE.lock();
    let cwd = std::fs::canonicalize(cwd)?;
    let current = snapshot(&cwd)?;
    verify_token(&cwd, expected)?;
    if current.truncated || current.conflicts || paths.is_empty() || paths.len() > FILE_LIMIT {
        return Err(Error::git(
            "Refresh and resolve incomplete or conflicted Git changes before discarding files",
        ));
    }
    let mut unique = HashSet::new();
    let mut plans = Vec::with_capacity(paths.len());
    for path in paths {
        if !unique.insert(path) {
            return Err(Error::invalid_path(
                "Git file is not an eligible workspace change",
            ));
        }
        validate_path(&cwd, path)?;
        let staged = current.staged.iter().find(|entry| entry.path == *path);
        let unstaged = current.unstaged.iter().find(|entry| entry.path == *path);
        if [staged, unstaged]
            .into_iter()
            .flatten()
            .any(|entry| !entry.actionable)
        {
            return Err(Error::invalid_path(
                "Git file is not an eligible workspace change",
            ));
        }
        let steps = discard_steps(
            staged.map(|entry| &entry.kind),
            unstaged.map(|entry| &entry.kind),
        )?;
        if discard_deletes(&steps) && !confirm_delete {
            return Err(Error::new(
                "confirmation_required",
                "Discarding new or untracked files deletes them permanently. Confirm the deletion to continue.",
            ));
        }
        plans.push((path, steps));
    }
    verify_token(&cwd, expected)?;
    admit()?;
    for (path, steps) in plans {
        for step in steps {
            match step.argv(path) {
                Some(args) => {
                    output(&cwd, &args)?;
                }
                None => delete_file(&cwd, path)?,
            }
        }
    }
    snapshot(&cwd)
}

pub fn commit_guard(cwd: &Path, expected: Option<&str>) -> Result<()> {
    let current = snapshot(cwd)?;
    if current.truncated
        || current.conflicts
        || current.outside_scope
        || current.staged.is_empty()
        || current.staged.iter().any(|f| !f.actionable)
    {
        return Err(Error::git(
            "Commit requires a complete, conflict-free staged set inside this workspace",
        ));
    }
    if let Some(expected) = expected {
        verify_token(&std::fs::canonicalize(cwd)?, expected)?;
    }
    Ok(())
}
/// As `commit_guard`, but nothing staged is fine: an amend may change only the message.
pub fn amend_guard(cwd: &Path, expected: Option<&str>) -> Result<()> {
    let current = snapshot(cwd)?;
    if current.truncated
        || current.conflicts
        || current.outside_scope
        || current.staged.iter().any(|f| !f.actionable)
    {
        return Err(Error::git(
            "Amend requires a complete, conflict-free staged set inside this workspace",
        ));
    }
    if let Some(expected) = expected {
        verify_token(&std::fs::canonicalize(cwd)?, expected)?;
    }
    Ok(())
}
pub fn diff(cwd: &Path, path: &str, staged: bool, expected: &str) -> Result<String> {
    let cwd = std::fs::canonicalize(cwd)?;
    let current = snapshot(&cwd)?;
    verify_token(&cwd, expected)?;
    let entries = if staged {
        &current.staged
    } else {
        &current.unstaged
    };
    let entry = entries
        .iter()
        .find(|e| e.path == path)
        .ok_or_else(|| Error::invalid_path("Git change no longer exists"))?;
    validate_path(&cwd, path)?;
    let result = if !staged && entry.kind == ChangeKind::Untracked {
        // Existing bounded descriptor-verified editor read; no filters or external diff.
        use std::io::Read;
        let file = crate::paths::open_regular_within(&cwd, &cwd.join(path))?;
        let mut bytes = Vec::new();
        file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(Error::git("Untracked file exceeds the 2 MiB preview limit"));
        }
        if bytes.contains(&0) {
            "Binary untracked file (not committed)".into()
        } else if let Ok(content) = String::from_utf8(bytes) {
            format!("+++ untracked\n{content}")
        } else {
            "Binary untracked file (not committed)".into()
        }
    } else {
        let mut args = vec![
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--no-color",
        ];
        if staged {
            args.push("--cached");
        }
        args.extend(["--", path]);
        text(&cwd, &args)?
    };
    verify_token(&cwd, expected)?;
    Ok(result)
}

/// Capture only the prepared index, never later worktree edits or external diff tools.
pub(crate) fn title_context(cwd: &Path, expected: &str) -> Result<(String, bool)> {
    let _write = INDEX_WRITE.lock();
    commit_guard(cwd, Some(expected))?;
    let names = text(
        cwd,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--name-only",
            "--",
        ],
    )?;
    let patch = text(
        cwd,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--no-color",
            "--",
        ],
    )?;
    fn excerpt(value: &str, bound: usize) -> &str {
        let mut end = value.len().min(bound);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        &value[..end]
    }
    let partial = names.len() > 8192 || patch.len() > 40960;
    let context = serde_json::json!({"filenames": excerpt(&names, 8192), "preparedPatch": excerpt(&patch, 40960), "partial": partial}).to_string();
    commit_guard(cwd, Some(expected))?;
    Ok((context, partial))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{run_ok, tests::Repo};
    use std::fs;

    #[test]
    fn amend_replaces_an_unpushed_commit_and_refuses_a_pushed_one() {
        let repo = Repo::new();
        std::fs::write(repo.cwd().join("a.txt"), "one").unwrap();
        run_ok(&repo.cwd(), &["add", "a.txt"]).unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "first title\n\nfirst body"]).unwrap();
        // Nothing staged and no message: the commit stays as it was, body included.
        crate::git::amend_checked(&repo.cwd(), "", None, || Ok(())).unwrap();
        assert_eq!(
            run_ok(&repo.cwd(), &["log", "-1", "--format=%B"])
                .unwrap()
                .trim(),
            "first title\n\nfirst body"
        );
        let result = crate::git::amend_checked(&repo.cwd(), "new title", None, || Ok(())).unwrap();
        assert_eq!(result.summary, "new title");
        assert_eq!(
            run_ok(&repo.cwd(), &["rev-list", "--count", "HEAD"])
                .unwrap()
                .trim(),
            "2"
        );
        run_ok(
            &repo.cwd(),
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        )
        .unwrap();
        assert!(crate::git::amend_checked(&repo.cwd(), "later", None, || Ok(())).is_err());
    }

    #[test]
    fn title_context_supports_unborn_head_and_refuses_conflicts() {
        let repo = Repo::new();
        run_ok(&repo.cwd(), &["checkout", "--orphan", "title-unborn"]).unwrap();
        let snapshot = snapshot(&repo.cwd()).unwrap();
        assert!(snapshot.head.is_none());
        assert!(title_context(&repo.cwd(), &snapshot.index_token).is_ok());
        let hash = run_ok(&repo.cwd(), &["rev-parse", ":file.txt"]).unwrap();
        let entries = format!("0 0000000000000000000000000000000000000000\tfile.txt\n100644 {hash} 2\tfile.txt\n100644 {hash} 3\tfile.txt\n");
        let result = crate::git::run_input(
            &repo.cwd(),
            &["update-index", "--index-info"],
            entries.as_bytes(),
        )
        .unwrap();
        assert!(result.status.success());
        let conflicted = super::snapshot(&repo.cwd()).unwrap();
        assert!(conflicted.conflicts);
        assert!(title_context(&repo.cwd(), &conflicted.index_token).is_err());
    }
    #[test]
    fn title_context_uses_index_only_and_refuses_stale_empty_and_foreign() {
        let repo = Repo::new();
        assert!(title_context(&repo.cwd(), &snapshot(&repo.cwd()).unwrap().index_token).is_err());
        fs::write(repo.cwd().join("file.txt"), "prepared marker").unwrap();
        run_ok(&repo.cwd(), &["add", "file.txt"]).unwrap();
        let token = snapshot(&repo.cwd()).unwrap().index_token;
        fs::write(repo.cwd().join("file.txt"), "unstaged secret").unwrap();
        let (context, partial) = title_context(&repo.cwd(), &token).unwrap();
        assert!(context.contains("prepared marker"));
        assert!(!context.contains("unstaged secret"));
        assert!(!partial);
        run_ok(&repo.cwd(), &["add", "file.txt"]).unwrap();
        assert!(title_context(&repo.cwd(), &token).is_err());
        fs::create_dir(repo.cwd().join("nested")).unwrap();
        let nested = repo.cwd().join("nested");
        assert!(title_context(&nested, &snapshot(&nested).unwrap().index_token).is_err());
    }
    #[test]
    fn title_context_excerpt_preserves_utf8() {
        let repo = Repo::new();
        fs::write(repo.cwd().join("file.txt"), "é".repeat(30_000)).unwrap();
        run_ok(&repo.cwd(), &["add", "file.txt"]).unwrap();
        let (context, partial) =
            title_context(&repo.cwd(), &snapshot(&repo.cwd()).unwrap().index_token).unwrap();
        assert!(partial);
        let parsed: serde_json::Value = serde_json::from_str(&context).unwrap();
        assert!(parsed["preparedPatch"].as_str().unwrap().len() <= 40960);
    }
    #[test]
    fn selected_stage_and_unstage_preserve_other_files_and_working_bytes() {
        let repo = Repo::new();
        fs::write(repo.cwd().join("chosen file"), "chosen\n").unwrap();
        fs::write(repo.cwd().join(":(glob)*"), "literal\n").unwrap();
        let before = snapshot(&repo.cwd()).unwrap();
        let after = prepare(
            &repo.cwd(),
            &["chosen file".into()],
            true,
            &before.index_token,
        )
        .unwrap();
        assert_eq!(after.staged.len(), 1);
        assert_eq!(after.staged[0].path, "chosen file");
        assert!(after.unstaged.iter().any(|f| f.path == ":(glob)*"));
        prepare(
            &repo.cwd(),
            &["chosen file".into()],
            false,
            &after.index_token,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(repo.cwd().join("chosen file")).unwrap(),
            "chosen\n"
        );
    }

    #[test]
    fn staged_and_working_diff_are_distinct_and_stale_token_is_rejected() {
        let repo = Repo::new();
        fs::write(repo.cwd().join("file.txt"), "index version\n").unwrap();
        let before = snapshot(&repo.cwd()).unwrap();
        let after = prepare(&repo.cwd(), &["file.txt".into()], true, &before.index_token).unwrap();
        fs::write(repo.cwd().join("file.txt"), "working version\n").unwrap();
        let now = snapshot(&repo.cwd()).unwrap();
        assert_eq!(now.staged.len(), 1);
        assert_eq!(now.unstaged.len(), 1);
        assert!(diff(&repo.cwd(), "file.txt", true, &after.index_token)
            .unwrap()
            .contains("+index version"));
        assert!(diff(&repo.cwd(), "file.txt", false, &after.index_token)
            .unwrap()
            .contains("+working version"));
        assert!(prepare(
            &repo.cwd(),
            &["file.txt".into()],
            false,
            &before.index_token
        )
        .is_err());
    }

    #[test]
    fn nested_commit_refuses_foreign_staged_files() {
        let repo = Repo::new();
        fs::create_dir(repo.cwd().join("nested")).unwrap();
        fs::write(repo.cwd().join("nested/inside"), "inside").unwrap();
        fs::write(repo.cwd().join("outside"), "outside").unwrap();
        run_ok(&repo.cwd(), &["add", "--", "outside", "nested/inside"]).unwrap();
        let cwd = repo.cwd().join("nested");
        assert!(snapshot(&cwd).unwrap().outside_scope);
        assert!(commit_guard(&cwd, None).is_err());
    }
    #[test]
    fn unborn_stage_unstage_and_first_commit() {
        let repo = Repo::new();
        let cwd = repo.0.join("unborn");
        fs::create_dir(&cwd).unwrap();
        run_ok(&cwd, &["init", "-q"]).unwrap();
        run_ok(&cwd, &["config", "user.name", "Fixture"]).unwrap();
        run_ok(&cwd, &["config", "user.email", "fixture@example.invalid"]).unwrap();
        fs::write(cwd.join("first"), "first\n").unwrap();
        let before = snapshot(&cwd).unwrap();
        assert!(before.head.is_none());
        assert!(before.history.is_empty());
        let prepared = prepare(&cwd, &["first".into()], true, &before.index_token).unwrap();
        let removed = prepare(&cwd, &["first".into()], false, &prepared.index_token).unwrap();
        assert!(removed.staged.is_empty());
        assert_eq!(fs::read_to_string(cwd.join("first")).unwrap(), "first\n");
        let prepared = prepare(&cwd, &["first".into()], true, &removed.index_token).unwrap();
        assert!(diff(&cwd, "first", true, &prepared.index_token)
            .unwrap()
            .contains("+first"));
        crate::git::commit_checked(&cwd, "First", Some(&prepared.index_token), || Ok(())).unwrap();
        assert_eq!(snapshot(&cwd).unwrap().history.len(), 1);
    }

    #[test]
    fn literal_names_deletion_and_rename_are_exact_paths() {
        let repo = Repo::new();
        fs::write(repo.cwd().join(":(glob)*"), "literal\n").unwrap();
        fs::write(repo.cwd().join("unrelated"), "untouched\n").unwrap();
        let state = snapshot(&repo.cwd()).unwrap();
        let state = prepare(&repo.cwd(), &[":(glob)*".into()], true, &state.index_token).unwrap();
        assert_eq!(state.staged.len(), 1);
        assert_eq!(state.staged[0].path, ":(glob)*");
        run_ok(&repo.cwd(), &["commit", "-qm", "literal"]).unwrap();
        fs::rename(repo.cwd().join("file.txt"), repo.cwd().join("renamed.txt")).unwrap();
        let state = snapshot(&repo.cwd()).unwrap();
        assert!(state
            .unstaged
            .iter()
            .any(|e| e.path == "file.txt" && e.kind == ChangeKind::Deleted));
        assert!(state
            .unstaged
            .iter()
            .any(|e| e.path == "renamed.txt" && e.kind == ChangeKind::Untracked));
        let state = prepare(
            &repo.cwd(),
            &["file.txt".into(), "renamed.txt".into()],
            true,
            &state.index_token,
        )
        .unwrap();
        assert_eq!(state.staged.len(), 2);
        assert!(diff(&repo.cwd(), "file.txt", true, &state.index_token)
            .unwrap()
            .contains("-before"));
        assert!(state.unstaged.iter().any(|e| e.path == "unrelated"));
    }

    #[test]
    fn linked_directories_traversal_and_unlisted_paths_are_rejected() {
        let repo = Repo::new();
        fs::create_dir(repo.cwd().join("dir")).unwrap();
        fs::write(repo.cwd().join("dir/file"), "file").unwrap();
        let state = snapshot(&repo.cwd()).unwrap();
        for path in [
            "dir",
            "../escape",
            ".git/config",
            "/tmp/foreign",
            "dir/../file",
            "unknown",
            "dir//file",
        ] {
            assert!(
                prepare(&repo.cwd(), &[path.into()], true, &state.index_token).is_err(),
                "{path}"
            );
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(repo.0.join("repo/dir"), repo.cwd().join("linked")).unwrap();
            let state = snapshot(&repo.cwd()).unwrap();
            assert!(state
                .unstaged
                .iter()
                .any(|f| f.path == "linked" && !f.actionable));
            assert!(prepare(
                &repo.cwd(),
                &["linked/file".into()],
                true,
                &state.index_token
            )
            .is_err());
        }
    }

    #[test]
    fn incomplete_snapshot_and_changed_owner_never_authorize_mutation() {
        let repo = Repo::new();
        for i in 0..201 {
            fs::write(repo.cwd().join(format!("file-{i}")), "file").unwrap();
        }
        let state = snapshot(&repo.cwd()).unwrap();
        assert!(state.truncated);
        assert_eq!(state.unstaged.len(), 200);
        assert!(prepare(&repo.cwd(), &["file-0".into()], true, &state.index_token).is_err());
        assert!(commit_guard(&repo.cwd(), None).is_err());
        let other = Repo::new();
        fs::write(other.cwd().join("new"), "file").unwrap();
        let state = snapshot(&other.cwd()).unwrap();
        assert!(prepare_admitted(
            &other.cwd(),
            &["new".into()],
            true,
            &state.index_token,
            || Err(Error::not_found("owner removed"))
        )
        .is_err());
        assert!(snapshot(&other.cwd()).unwrap().staged.is_empty());
    }

    #[test]
    fn merges_and_conflicts_use_actual_parents_and_conflict_state() {
        let repo = Repo::new();
        run_ok(&repo.cwd(), &["checkout", "-qb", "side"]).unwrap();
        fs::write(repo.cwd().join("side"), "side").unwrap();
        run_ok(&repo.cwd(), &["add", "side"]).unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "side"]).unwrap();
        let side = head(&repo.cwd()).unwrap().unwrap();
        run_ok(&repo.cwd(), &["checkout", "-qb", "main-test", "HEAD~1"]).unwrap();
        fs::write(repo.cwd().join("main"), "main").unwrap();
        run_ok(&repo.cwd(), &["add", "main"]).unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "main"]).unwrap();
        let main = head(&repo.cwd()).unwrap().unwrap();
        run_ok(&repo.cwd(), &["merge", "--no-ff", "-m", "merge", "side"]).unwrap();
        let state = snapshot(&repo.cwd()).unwrap();
        assert_eq!(state.history[0].parents, vec![main, side]);
        run_ok(&repo.cwd(), &["checkout", "-qb", "conflict"]).unwrap();
        fs::write(repo.cwd().join("file.txt"), "side edit\n").unwrap();
        run_ok(&repo.cwd(), &["add", "file.txt"]).unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "side edit"]).unwrap();
        run_ok(&repo.cwd(), &["checkout", "main-test"]).unwrap();
        fs::write(repo.cwd().join("file.txt"), "main edit\n").unwrap();
        run_ok(&repo.cwd(), &["add", "file.txt"]).unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "main edit"]).unwrap();
        assert!(run_ok(&repo.cwd(), &["merge", "conflict"]).is_err());
        let state = snapshot(&repo.cwd()).unwrap();
        assert!(state.conflicts);
        assert!(state.staged.iter().all(|e| !e.actionable));
        assert!(commit_guard(&repo.cwd(), None).is_err());
    }

    #[test]
    fn older_history_pages_continue_from_the_loaded_commit() {
        let repo = Repo::new();
        for _ in 0..59 {
            run_ok(
                &repo.cwd(),
                &["commit", "--allow-empty", "-qm", "history fixture"],
            )
            .unwrap();
        }
        let first = snapshot(&repo.cwd()).unwrap();
        let head = first.head.clone().unwrap();
        // A commit made after loading must not shift the next page.
        run_ok(&repo.cwd(), &["commit", "--allow-empty", "-qm", "later"]).unwrap();
        let (page, truncated) = history_from(&repo.cwd(), &head, 50).unwrap();
        assert!(!truncated);
        assert_eq!(page.len(), 10);
        assert_eq!(first.history[49].parents[0], page[0].hash);
        assert!(page.last().unwrap().parents.is_empty());
        assert!(history_from(&repo.cwd(), "HEAD", 0).is_err());
        assert!(history_from(&repo.cwd(), "--all", 0).is_err());
        assert!(history_from(&repo.cwd(), &head, 9_999)
            .unwrap()
            .0
            .is_empty());
    }
    #[test]
    fn history_is_bounded_and_binary_preview_is_truthful() {
        let repo = Repo::new();
        for _ in 0..51 {
            run_ok(
                &repo.cwd(),
                &["commit", "--allow-empty", "-qm", "history fixture"],
            )
            .unwrap();
        }
        fs::write(repo.cwd().join("binary"), [0, 255, 0]).unwrap();
        let state = snapshot(&repo.cwd()).unwrap();
        assert!(state.history_truncated);
        assert_eq!(state.history.len(), 50);
        assert_eq!(state.history[0].parents[0], state.history[1].hash);
        assert_eq!(
            diff(&repo.cwd(), "binary", false, &state.index_token).unwrap(),
            "Binary untracked file (not committed)"
        );
    }

    #[test]
    fn stale_commit_and_excessive_diff_fail_closed() {
        let repo = Repo::new();
        fs::write(repo.cwd().join("file.txt"), "first\n").unwrap();
        let state = snapshot(&repo.cwd()).unwrap();
        let state = prepare(&repo.cwd(), &["file.txt".into()], true, &state.index_token).unwrap();
        fs::write(repo.cwd().join("file.txt"), "second\n").unwrap();
        run_ok(&repo.cwd(), &["add", "file.txt"]).unwrap();
        assert!(crate::git::commit_checked(
            &repo.cwd(),
            "Wrong revision",
            Some(&state.index_token),
            || Ok(())
        )
        .is_err());
        fs::write(repo.cwd().join("file.txt"), "large diff\n".repeat(250_000)).unwrap();
        let state = snapshot(&repo.cwd()).unwrap();
        assert!(diff(&repo.cwd(), "file.txt", false, &state.index_token).is_err());
    }
    #[test]
    fn nested_token_covers_foreign_index_changes() {
        let repo = Repo::new();
        fs::create_dir(repo.cwd().join("nested")).unwrap();
        fs::write(repo.cwd().join("nested/inside"), "inside").unwrap();
        let cwd = repo.cwd().join("nested");
        let state = snapshot(&cwd).unwrap();
        fs::write(repo.cwd().join("file.txt"), "foreign staged").unwrap();
        run_ok(&repo.cwd(), &["add", "file.txt"]).unwrap();
        assert_ne!(state.index_token, snapshot(&cwd).unwrap().index_token);
        assert!(prepare(&cwd, &["inside".into()], true, &state.index_token).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_and_executable_config_fail_closed() {
        let repo = Repo::new();
        // APFS rejects invalid UTF-8 filesystem names. Git's index still permits
        // their byte identity, so use a deleted indexed entry to exercise parsing.
        let blob = run_ok(&repo.cwd(), &["rev-parse", "HEAD:file.txt"]).unwrap();
        let mut indexed = format!("100644 {blob}\t").into_bytes();
        indexed.extend([255, 0]);
        let result = crate::git::run_input(
            &repo.cwd(),
            &["update-index", "-z", "--index-info"],
            &indexed,
        )
        .unwrap();
        assert!(result.status.success());
        assert!(snapshot(&repo.cwd()).is_err());
        let other = Repo::new();
        run_ok(&other.cwd(), &["config", "filter.hostile.clean", "false"]).unwrap();
        assert!(snapshot(&other.cwd()).is_err());
        run_ok(&other.cwd(), &["config", "--unset", "filter.hostile.clean"]).unwrap();
        run_ok(&other.cwd(), &["config", "hook.hostile.command", "false"]).unwrap();
        assert!(snapshot(&other.cwd()).is_err());
    }

    #[test]
    fn action_schema_is_closed() {
        assert!(serde_json::from_value::<Action>(
            serde_json::json!({"type":"snapshot", "sessionId":"owned", "cwd":"/foreign"})
        )
        .is_err());
        assert!(serde_json::from_value::<Action>(
            serde_json::json!({"type":"execute", "sessionId":"owned", "args":["reset", "--hard"]})
        )
        .is_err());
    }
    #[test]
    fn deleted_submodule_cannot_be_bulk_prepared() {
        let repo = Repo::new();
        let hash = head(&repo.cwd()).unwrap().unwrap();
        run_ok(
            &repo.cwd(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{hash},module"),
            ],
        )
        .unwrap();
        let state = snapshot(&repo.cwd()).unwrap();
        assert!(state
            .staged
            .iter()
            .any(|f| f.path == "module" && !f.actionable));
        assert!(prepare(&repo.cwd(), &["module".into()], true, &state.index_token).is_err());
        assert!(commit_guard(&repo.cwd(), None).is_err());
    }
    #[test]
    fn malformed_commit_metadata_cannot_invent_history_rows() {
        let repo = Repo::new();
        let parent = head(&repo.cwd()).unwrap().unwrap();
        let tree = run_ok(&repo.cwd(), &["rev-parse", "HEAD^{tree}"]).unwrap();
        let bytes = format!("tree {tree}\nparent {parent}\nauthor Fixture <fixture@example.invalid> 1700000000 +0000\ncommitter Fixture <fixture@example.invalid> 1700000000 +0000\n\nsubject\0author\0date\0refs\0{}\0\n", "f".repeat(40));
        let result = crate::git::run_input(
            &repo.cwd(),
            &[
                "hash-object",
                "-t",
                "commit",
                "--literally",
                "-w",
                "--stdin",
            ],
            bytes.as_bytes(),
        )
        .unwrap();
        assert!(result.status.success());
        let hash = String::from_utf8(result.stdout).unwrap().trim().to_owned();
        run_ok(&repo.cwd(), &["update-ref", "HEAD", &hash]).unwrap();
        if let Ok(state) = snapshot(&repo.cwd()) {
            assert_eq!(state.history.len(), 2);
            assert_eq!(state.history[0].hash, hash);
            assert_eq!(state.history[0].parents, vec![parent.clone()]);
            assert_eq!(state.history[1].hash, parent);
        }
    }
    #[test]
    fn staged_deletion_of_head_gitlink_cannot_be_unstaged_or_committed() {
        let repo = Repo::new();
        let hash = head(&repo.cwd()).unwrap().unwrap();
        run_ok(
            &repo.cwd(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{hash},module"),
            ],
        )
        .unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "fixture gitlink"]).unwrap();
        run_ok(&repo.cwd(), &["update-index", "--force-remove", "module"]).unwrap();
        assert!(!repo.cwd().join("module").exists());
        let state = snapshot(&repo.cwd()).unwrap();
        assert!(state.staged.iter().any(|entry| entry.path == "module"
            && entry.kind == ChangeKind::Deleted
            && !entry.actionable));
        assert!(prepare(&repo.cwd(), &["module".into()], false, &state.index_token).is_err());
        assert!(crate::git::commit_checked(
            &repo.cwd(),
            "Delete module",
            Some(&state.index_token),
            || Ok(())
        )
        .is_err());
        assert!(run_ok(&repo.cwd(), &["ls-files", "--", "module"])
            .unwrap()
            .is_empty());
        assert_eq!(
            run_ok(&repo.cwd(), &["log", "-1", "--format=%s"]).unwrap(),
            "fixture gitlink"
        );
    }
}

#[cfg(test)]
mod discard_tests {
    use super::*;
    use crate::git::{run_ok, tests::Repo};
    use std::fs;

    fn token(cwd: &Path) -> String {
        snapshot(cwd).unwrap().index_token
    }

    #[test]
    fn each_kind_gets_its_own_plan() {
        use ChangeKind::{Added, Deleted, Modified, Untracked};
        use DiscardStep::*;
        assert_eq!(
            discard_steps(None, Some(&Modified)).unwrap(),
            vec![RestoreWorktree]
        );
        assert_eq!(
            discard_steps(None, Some(&Deleted)).unwrap(),
            vec![RestoreWorktree]
        );
        assert_eq!(
            discard_steps(None, Some(&Untracked)).unwrap(),
            vec![DeleteFile]
        );
        assert_eq!(
            discard_steps(Some(&Modified), None).unwrap(),
            vec![RestoreStaged, RestoreWorktree]
        );
        assert_eq!(
            discard_steps(Some(&Deleted), Some(&Modified)).unwrap(),
            vec![RestoreStaged, RestoreWorktree]
        );
        assert_eq!(
            discard_steps(Some(&Added), None).unwrap(),
            vec![RemoveFromIndex, DeleteFile]
        );
        assert_eq!(
            discard_steps(Some(&Added), Some(&Modified)).unwrap(),
            vec![RemoveFromIndex, DeleteFile]
        );
        assert_eq!(
            discard_steps(Some(&Added), Some(&Deleted)).unwrap(),
            vec![RemoveFromIndex, DeleteFile]
        );
        // A rename never reaches the planner (status runs with --no-renames), and an untracked
        // file under a staged deletion would be overwritten by a restore: both refuse.
        assert!(discard_steps(Some(&ChangeKind::Renamed), None).is_err());
        assert!(discard_steps(Some(&Deleted), Some(&Untracked)).is_err());
        assert!(discard_steps(None, None).is_err());
        assert!(!discard_deletes(&[RestoreStaged, RestoreWorktree]));
        assert!(discard_deletes(&[RemoveFromIndex, DeleteFile]));
    }

    #[test]
    fn every_git_step_separates_the_path_with_double_dash() {
        for step in [
            DiscardStep::RestoreStaged,
            DiscardStep::RestoreWorktree,
            DiscardStep::RemoveFromIndex,
        ] {
            let argv = step.argv("-dash name").unwrap();
            assert_eq!(argv[argv.len() - 2], "--", "{step:?}");
            assert_eq!(argv[argv.len() - 1], "-dash name", "{step:?}");
            assert!(!argv
                .iter()
                .any(|arg| arg.contains("reset") || arg.contains("checkout")));
        }
        assert!(DiscardStep::DeleteFile.argv("x").is_none());
    }

    #[test]
    fn paths_outside_the_repository_are_refused_before_any_change() {
        let repo = Repo::new();
        let outside = repo.0.join("outside.txt");
        fs::write(&outside, "keep\n").unwrap();
        let expected = token(&repo.cwd());
        for bad in [
            "/tmp/outside.txt",
            "../outside.txt",
            "a/../../outside.txt",
            "file.txt/",
            "a//b",
            ".git/config",
            "bad\\path",
            ".",
            "",
        ] {
            let result =
                discard_admitted(&repo.cwd(), &[bad.to_string()], &expected, true, || Ok(()));
            assert!(result.is_err(), "accepted {bad:?}");
        }
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep\n");
    }

    #[test]
    #[cfg(unix)]
    fn symlinks_cannot_lead_discard_outside_the_repository() {
        let repo = Repo::new();
        let outside = repo.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "keep\n").unwrap();
        std::os::unix::fs::symlink(&outside, repo.cwd().join("link")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), repo.cwd().join("linkfile"))
            .unwrap();
        let expected = token(&repo.cwd());
        for bad in ["link/secret.txt", "linkfile"] {
            assert!(
                discard_admitted(&repo.cwd(), &[bad.to_string()], &expected, true, || Ok(()))
                    .is_err(),
                "{bad}"
            );
        }
        assert_eq!(
            fs::read_to_string(outside.join("secret.txt")).unwrap(),
            "keep\n"
        );
    }

    #[test]
    fn discard_restores_tracked_files_and_removes_new_ones_only_when_confirmed() {
        let repo = Repo::new();
        let cwd = repo.cwd();
        fs::write(cwd.join("file.txt"), "changed in index\n").unwrap();
        run_ok(&cwd, &["add", "--", "file.txt"]).unwrap();
        fs::write(cwd.join("file.txt"), "changed again\n").unwrap();
        fs::write(cwd.join("added.txt"), "new\n").unwrap();
        run_ok(&cwd, &["add", "--", "added.txt"]).unwrap();
        fs::write(cwd.join("untracked.txt"), "loose\n").unwrap();
        let before = snapshot(&cwd).unwrap();
        assert_eq!(before.staged.len(), 2);

        // Deleting new files is refused until the person confirms it; nothing changes.
        let all = vec![
            "file.txt".to_string(),
            "added.txt".to_string(),
            "untracked.txt".to_string(),
        ];
        let refused = discard_admitted(&cwd, &all, &before.index_token, false, || Ok(()));
        assert!(refused.is_err());
        assert!(cwd.join("added.txt").exists() && cwd.join("untracked.txt").exists());
        assert_eq!(snapshot(&cwd).unwrap().staged.len(), 2);

        let after = discard_admitted(&cwd, &all, &before.index_token, true, || Ok(())).unwrap();
        assert!(after.staged.is_empty() && after.unstaged.is_empty());
        assert_eq!(
            fs::read_to_string(cwd.join("file.txt")).unwrap(),
            "before\n"
        );
        assert!(!cwd.join("added.txt").exists());
        assert!(!cwd.join("untracked.txt").exists());
        assert!(run_ok(&cwd, &["ls-files", "added.txt"])
            .unwrap()
            .trim()
            .is_empty());
    }

    #[test]
    fn staged_deletion_comes_back_and_stale_tokens_change_nothing() {
        let repo = Repo::new();
        let cwd = repo.cwd();
        run_ok(&cwd, &["rm", "-q", "--", "file.txt"]).unwrap();
        let stale = token(&cwd);
        // Staging another file changes the index, so the earlier token must be refused.
        fs::write(cwd.join("other.txt"), "x\n").unwrap();
        run_ok(&cwd, &["add", "--", "other.txt"]).unwrap();
        assert!(
            discard_admitted(&cwd, &["file.txt".to_string()], &stale, true, || Ok(())).is_err()
        );
        assert!(!cwd.join("file.txt").exists());
        let current = token(&cwd);
        let after =
            discard_admitted(&cwd, &["file.txt".to_string()], &current, true, || Ok(())).unwrap();
        assert_eq!(
            fs::read_to_string(cwd.join("file.txt")).unwrap(),
            "before\n"
        );
        assert_eq!(after.staged.len(), 1, "other.txt stays staged");
        assert!(after.unstaged.is_empty());
    }

    #[test]
    fn action_schema_accepts_discard_and_rejects_unknown_fields() {
        let ok = r#"{"type":"discard","sessionId":"s","paths":["a"],"expectedIndex":"x","confirmDelete":false}"#;
        assert!(serde_json::from_str::<Action>(ok).is_ok());
        let extra = r#"{"type":"discard","sessionId":"s","paths":["a"],"expectedIndex":"x","confirmDelete":false,"argv":["reset"]}"#;
        assert!(serde_json::from_str::<Action>(extra).is_err());
    }
}
