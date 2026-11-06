//! Worktree cleanup (ADR-078): leftover folders under the worktree base that no
//! session references.
//! Nothing with uncommitted or untracked changes is ever removed, removal never
//! uses `--force`, and branches are always kept.
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::runtime::State;
use serde::{Deserialize, Serialize};

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::git;
use crate::paths::display_path;

/// Entries counted when measuring one folder; beyond it the size is a lower bound.
const MAX_SIZE_ENTRIES: usize = 400_000;
const MAX_LEFTOVERS: usize = 200;

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    /// Lists leftover folders and how much space removing them frees.
    Scan,
    /// Removes the leftovers that are safe to remove; the caller has confirmed.
    Clean { confirm: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeptReason {
    Uncommitted,
    /// Git cannot tell whether the folder holds work (not a worktree, or its repository is gone).
    Unverified,
    InUse,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Leftover {
    pub path: String,
    pub bytes: u64,
    /// `None` when removing it is safe.
    pub kept: Option<KeptReason>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scan {
    pub leftovers: Vec<Leftover>,
    pub reclaimable_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cleaned {
    pub removed: usize,
    pub freed_bytes: u64,
    pub kept: Vec<Leftover>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Response {
    Scan(Scan),
    Cleaned(Cleaned),
}

pub async fn worktree_cleanup_action(
    state: State<'_, Arc<AppState>>,
    action: Action,
) -> Result<Response> {
    let state = state.inner().clone();
    crate::commands::native_task(move || match action {
        Action::Scan => {
            let leftovers = scan(&state)?;
            let reclaimable_bytes = leftovers
                .iter()
                .filter(|item| item.kept.is_none())
                .map(|item| item.bytes)
                .sum();
            Ok(Response::Scan(Scan {
                leftovers,
                reclaimable_bytes,
            }))
        }
        Action::Clean { confirm } => {
            if !confirm {
                return Err(Error::confirmation_required(
                    "removing leftover worktrees deletes those folders. confirm explicitly.",
                ));
            }
            clean(&state).map(Response::Cleaned)
        }
    })
    .await
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Bases holding `{projectId}/{worktree}` folders: the automatic root and the custom one.
fn bases(state: &AppState) -> Vec<PathBuf> {
    let data = state.data.lock();
    let mut bases = vec![state.worktree_root.clone()];
    if let Some(path) = &data.settings.worktree_base_path {
        bases.push(PathBuf::from(path));
    }
    let mut seen = HashSet::new();
    bases
        .into_iter()
        .filter(|base| base.is_dir())
        .map(|base| canonical(&base))
        .filter(|base| seen.insert(base.clone()))
        .collect()
}

fn referenced(state: &AppState) -> HashSet<PathBuf> {
    state
        .data
        .lock()
        .sessions
        .iter()
        .map(|session| canonical(Path::new(&session.worktree.path)))
        .collect()
}

/// Only folders shaped like the app's own project folders are looked at, so a
/// custom base shared with other files is never walked beyond them.
fn is_project_id(name: &str) -> bool {
    uuid::Uuid::parse_str(name).is_ok() && name.len() == 36
}

fn real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

fn children(dir: &Path) -> Vec<PathBuf> {
    let mut items = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| real_dir(path))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    items.sort();
    items
}

/// Unreferenced worktree folders under every base.
fn candidates(state: &AppState) -> Vec<(PathBuf, PathBuf)> {
    let used = referenced(state);
    let mut found = Vec::new();
    for base in bases(state) {
        for project in children(&base) {
            let name = project.file_name().and_then(|name| name.to_str());
            if !name.is_some_and(is_project_id) {
                continue;
            }
            for tree in children(&project) {
                if !used.contains(&canonical(&tree)) && found.len() < MAX_LEFTOVERS {
                    found.push((project.clone(), tree));
                }
            }
        }
    }
    found
}

fn is_empty_dir(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

/// The main checkout that owns a linked worktree, when Git can still tell.
fn main_checkout(tree: &Path) -> Option<PathBuf> {
    let top = git::run_ok(tree, &["rev-parse", "--show-toplevel"]).ok()?;
    if canonical(Path::new(&top)) != canonical(tree) {
        return None;
    }
    let first = git::list_worktrees(tree).ok()?.into_iter().next()?;
    let main = PathBuf::from(first.path);
    (canonical(&main) != canonical(tree)).then_some(main)
}

fn classify(tree: &Path) -> Option<KeptReason> {
    if is_empty_dir(tree) {
        return None;
    }
    if main_checkout(tree).is_none() {
        return Some(KeptReason::Unverified);
    }
    match git::status(tree) {
        Ok(status) if !status.dirty => None,
        Ok(_) => Some(KeptReason::Uncommitted),
        Err(_) => Some(KeptReason::Unverified),
    }
}

/// Bytes on disk below `path`, without following symbolic links.
pub(crate) fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_SIZE_ENTRIES {
                return total;
            }
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

fn scan(state: &AppState) -> Result<Vec<Leftover>> {
    Ok(candidates(state)
        .into_iter()
        .map(|(_, tree)| Leftover {
            bytes: dir_size(&tree),
            kept: classify(&tree),
            path: display_path(&tree),
        })
        .collect())
}

fn clean(state: &AppState) -> Result<Cleaned> {
    state.ensure_running()?;
    let mut cleaned = Cleaned {
        removed: 0,
        freed_bytes: 0,
        kept: Vec::new(),
    };
    let mut mains = HashSet::new();
    let mut projects = HashSet::new();
    for (project, tree) in candidates(state) {
        projects.insert(project);
        let bytes = dir_size(&tree);
        let mut kept = classify(&tree);
        if kept.is_none() {
            // Worktrees are created and referenced under the data lock, so an
            // unreferenced folder seen under it cannot belong to a new session.
            let unreferenced = !referenced(state).contains(&canonical(&tree));
            kept = if !unreferenced {
                Some(KeptReason::InUse)
            } else if is_empty_dir(&tree) {
                fs::remove_dir(&tree).err().map(|_| KeptReason::Failed)
            } else {
                match main_checkout(&tree) {
                    Some(main) => {
                        let path = display_path(&tree);
                        let removed = git::run(&main, &["worktree", "remove", "--", &path])
                            .is_ok_and(|output| output.status.success());
                        mains.insert(main);
                        (!removed).then_some(KeptReason::Failed)
                    }
                    None => Some(KeptReason::Unverified),
                }
            };
        }
        match kept {
            None => {
                cleaned.removed += 1;
                cleaned.freed_bytes += bytes;
            }
            Some(reason) => cleaned.kept.push(Leftover {
                path: display_path(&tree),
                bytes,
                kept: Some(reason),
            }),
        }
    }
    let roots = state
        .data
        .lock()
        .projects
        .iter()
        .map(|project| PathBuf::from(&project.path))
        .collect::<Vec<_>>();
    for root in roots.iter().chain(mains.iter()) {
        let _ = git::run(root, &["worktree", "prune"]);
    }
    for project in projects {
        let _ = fs::remove_dir(project);
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::Repo;
    use crate::models::AppData;
    use crate::worktree;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;

    fn state(repo: &Repo, sessions: serde_json::Value) -> AppState {
        let data: AppData = serde_json::from_value(serde_json::json!({
            "projects":[{"id":"p","name":"Fixture","path":display_path(&repo.cwd()),"addedAt":"t","lastOpenedAt":"t"}],
            "settings":{},
                        "sessions":sessions,
        }))
        .unwrap();
        AppState {
            data_path: repo.0.join("state.json"),
            worktree_root: repo.0.join("trees"),
            data: Mutex::new(data),
            agents: Mutex::new(HashMap::new()),
            ptys: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            attachment_picker: AtomicBool::new(false),
            attachments: Default::default(),
            usage: Default::default(),
            accounts: Default::default(),
            close_guard: Default::default(),
            draft_checkpoint: Mutex::new(None),
            checkpoint_pending: AtomicBool::new(false),
            catalogs: Mutex::new(HashMap::new()),
        }
    }

    fn session(id: &str, tree: &crate::models::Worktree) -> serde_json::Value {
        serde_json::json!({"id":id,"projectId":"p","title":"T","agent":"codex","status":"completed",
            "createdAt":"t","lastActivityAt":"t","worktree":tree,"lastError":null})
    }

    #[test]
    fn leftovers_skip_referenced_dirty_and_foreign_folders() {
        let repo = Repo::new();
        let project = repo
            .0
            .join("trees")
            .join("6f1c2a52-1d1e-4a44-9d3f-0d6c4d1a9b10");
        let make = |id: &str, title: &str| {
            worktree::create_isolated(
                &repo.cwd(),
                &project,
                id,
                title,
                "sirus/{session-name}-{id}",
            )
            .unwrap()
        };
        let used = make("11111111-a", "Used");
        let clean = make("22222222-b", "Clean");
        let dirty = make("33333333-c", "Dirty");
        fs::write(Path::new(&dirty.path).join("work.txt"), "keep me").unwrap();
        fs::write(Path::new(&clean.path).join("file.txt"), "before\n").unwrap();
        let empty = project.join("empty");
        fs::create_dir_all(&empty).unwrap();
        let plain = project.join("plain");
        fs::create_dir_all(&plain).unwrap();
        fs::write(plain.join("notes.txt"), "not git").unwrap();
        let foreign = repo.0.join("trees").join("not-a-project").join("tree");
        fs::create_dir_all(&foreign).unwrap();
        let state = state(&repo, serde_json::json!([session("s", &used)]));

        let found = scan(&state).unwrap();
        let reason = |path: &Path| {
            found
                .iter()
                .find(|item| {
                    item.path == display_path(&canonical(path)) || item.path == display_path(path)
                })
                .map(|item| item.kept)
        };
        assert_eq!(
            reason(Path::new(&used.path)),
            None,
            "referenced is not a leftover"
        );
        assert_eq!(reason(Path::new(&clean.path)), Some(None));
        assert_eq!(
            reason(Path::new(&dirty.path)),
            Some(Some(KeptReason::Uncommitted))
        );
        assert_eq!(reason(&empty), Some(None));
        assert_eq!(reason(&plain), Some(Some(KeptReason::Unverified)));
        assert_eq!(reason(&foreign), None);

        let cleaned = super::clean(&state).unwrap();
        assert_eq!(cleaned.removed, 2);
        assert!(!Path::new(&clean.path).exists());
        assert!(!empty.exists());
        assert!(Path::new(&used.path).exists());
        assert_eq!(
            fs::read_to_string(Path::new(&dirty.path).join("work.txt")).unwrap(),
            "keep me"
        );
        assert!(plain.join("notes.txt").exists());
        assert!(foreign.exists());
        assert!(git::run_ok(&repo.cwd(), &["rev-parse", "--verify", &clean.branch]).is_ok());
    }
}
