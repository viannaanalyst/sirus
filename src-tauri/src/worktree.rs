use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::git;
use crate::models::Worktree;
use crate::paths::{display_path, slug};

pub fn create_isolated(
    project_root: &Path,
    worktree_parent: &Path,
    session_id: &str,
    title: &str,
    branch_pattern: &str,
) -> Result<Worktree> {
    create_isolated_at(
        project_root,
        worktree_parent,
        session_id,
        title,
        branch_pattern,
        "HEAD",
    )
}

/// Where a new worktree starts (ADR-130): `origin/<default branch>`'s commit when the setting
/// is on and the fetch gave one, otherwise the checkout's `HEAD` as before.
pub fn start_point(from_origin: bool, origin_commit: Option<String>) -> String {
    match origin_commit {
        Some(commit) if from_origin && !commit.is_empty() => commit,
        _ => "HEAD".into(),
    }
}

/// Fetches origin's default branch and returns its commit, or None (no origin, a refused or
/// failed fetch, or an unknown default branch); the caller then starts from `HEAD`.
pub fn origin_commit(project_root: &Path) -> Option<String> {
    let branch = git::origin_default_branch(project_root)?;
    if let Err(error) = git::fetch_origin_branch(project_root, &branch) {
        tracing::info!(%error, "new worktree starts from HEAD: origin was not fetched");
        return None;
    }
    git::run_ok(
        project_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/origin/{branch}^{{commit}}"),
        ],
    )
    .ok()
    .filter(|commit| commit.len() >= 40 && commit.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The commit a local branch points at, for a new worktree started from it; None when the name
/// is not a valid branch or no such local branch exists.
pub fn branch_commit(project_root: &Path, branch: &str) -> Option<String> {
    git::run_ok(project_root, &["check-ref-format", "--branch", branch]).ok()?;
    git::run_ok(
        project_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}^{{commit}}"),
        ],
    )
    .ok()
    .filter(|commit| commit.len() >= 40 && commit.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Same as `create_isolated`, starting the new branch at `start` (a verified commit or `HEAD`).
pub fn create_isolated_at(
    project_root: &Path,
    worktree_parent: &Path,
    session_id: &str,
    title: &str,
    branch_pattern: &str,
    start: &str,
) -> Result<Worktree> {
    let identity = git::identity(project_root)?;
    if !identity.is_repo {
        return Err(Error::git("isolated worktrees require a Git repository"));
    }
    let slug_title = slug(title);
    let short_id = session_id.chars().take(8).collect::<String>();
    let branch = branch_pattern
        .replace("{session-name}", &slug_title)
        .replace("{id}", &short_id);
    let mut branch = if branch.trim().is_empty() {
        format!("sirus/{slug_title}-{short_id}")
    } else {
        branch
    };
    if !branch_pattern.contains("{id}") && !branch.ends_with(&short_id) {
        branch = format!("{branch}-{short_id}");
    }
    git::run_ok(project_root, &["check-ref-format", "--branch", &branch])?;
    fs::create_dir_all(worktree_parent)?;
    let dest = unique_dir(worktree_parent, &slug_title)?;
    git::run_ok(
        project_root,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &display_path(&dest),
            start,
        ],
    )?;
    Ok(Worktree {
        path: display_path(&dest),
        branch,
        isolated: true,
    })
}

/// An isolated worktree on a branch fetched from origin (a conversation moved here, ADR-133):
/// `git worktree add -b <branch> <dest> origin/<branch>`, so the branch tracks origin and
/// later pushes go to the same branch. Refused when that branch already exists here.
pub fn create_from_origin_branch(
    project_root: &Path,
    worktree_parent: &Path,
    title: &str,
    branch: &str,
) -> Result<Worktree> {
    create_from_origin(project_root, worktree_parent, title, branch, None)
}

/// An isolated worktree on a new `branch` started from `origin/<start>` without tracking it
/// (a conversation continued here while its computer is off, ADR-133 addendum).
pub fn create_from_origin_start(
    project_root: &Path,
    worktree_parent: &Path,
    title: &str,
    branch: &str,
    start: &str,
) -> Result<Worktree> {
    git::run_ok(project_root, &["check-ref-format", "--branch", start])?;
    create_from_origin(project_root, worktree_parent, title, branch, Some(start))
}

fn create_from_origin(
    project_root: &Path,
    worktree_parent: &Path,
    title: &str,
    branch: &str,
    start: Option<&str>,
) -> Result<Worktree> {
    git::run_ok(project_root, &["check-ref-format", "--branch", branch])?;
    let local = git::run(
        project_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )?;
    if local.status.success() {
        return Err(Error::git(format!(
            "The branch {branch} already exists on this computer."
        )));
    }
    fs::create_dir_all(worktree_parent)?;
    let dest = unique_dir(worktree_parent, &slug(title))?;
    git::run_ok(
        project_root,
        &[
            "worktree",
            "add",
            if start.is_some() {
                "--no-track"
            } else {
                "--track"
            },
            "-b",
            branch,
            &display_path(&dest),
            &format!("origin/{}", start.unwrap_or(branch)),
        ],
    )?;
    Ok(Worktree {
        path: display_path(&dest),
        branch: branch.to_string(),
        isolated: true,
    })
}

/// Recreates a missing isolated worktree from its branch: only an app-made folder
/// (`…/{project id}/{name}`), only while that branch exists and nothing else has it out.
pub fn restore(project_root: &Path, project_id: &str, worktree: &Worktree) -> Result<()> {
    let path = Path::new(&worktree.path);
    let owned = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some(project_id);
    if !worktree.isolated || path.exists() || !owned || worktree.branch.is_empty() {
        return Err(Error::invalid_path(
            "This session's folder is missing and cannot be restored.",
        ));
    }
    // Forget the registration of the deleted folder before adding it again.
    git::run_ok(project_root, &["worktree", "prune"])?;
    git::run_ok(
        project_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", worktree.branch),
        ],
    )
    .map_err(|_| {
        Error::git(format!(
            "The session's folder is missing and its branch {} no longer exists.",
            worktree.branch
        ))
    })?;
    fs::create_dir_all(path.parent().unwrap_or(path))?;
    git::run_ok(
        project_root,
        &["worktree", "add", &display_path(path), &worktree.branch],
    )?;
    Ok(())
}

pub fn remove(project_root: &Path, worktree: &Worktree, confirm: bool) -> Result<()> {
    if !worktree.isolated {
        return Ok(());
    }
    if !confirm {
        return Err(Error::confirmation_required(
            "removing a worktree deletes that working copy. confirm explicitly.",
        ));
    }
    if git::status(Path::new(&worktree.path))?.dirty {
        return Err(Error::git(
            "worktree contains uncommitted or untracked changes; preserve them before removal",
        ));
    }
    let output = git::run(project_root, &["worktree", "remove", "--", &worktree.path])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::git(format!(
            "worktree remove refused (no force flags are used automatically): {stderr}"
        )));
    }
    tidy(project_root, Path::new(&worktree.path));
    Ok(())
}

/// After a removal: drops stale Git worktree metadata and the project folder under
/// the worktree base once it is empty (ADR-078). Never deletes anything else.
pub fn tidy(project_root: &Path, removed: &Path) {
    let _ = git::run(project_root, &["worktree", "prune"]);
    if let Some(parent) = removed.parent() {
        // `remove_dir` only succeeds on an empty folder.
        let _ = fs::remove_dir(parent);
    }
}

/// Why an isolated worktree cannot be released without losing work, if anything:
/// uncommitted or untracked changes, or a HEAD no other branch or remote contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Unsafe {
    Uncommitted,
    Unmerged,
}

pub fn release_blocker(tree: &Path, branch: &str) -> Result<Option<Unsafe>> {
    if git::status(tree)?.dirty {
        return Ok(Some(Unsafe::Uncommitted));
    }
    let head = git::run_ok(tree, &["rev-parse", "--verify", "HEAD"])?;
    let refs = git::run_ok(
        tree,
        &[
            "for-each-ref",
            "--contains",
            &head,
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let own = format!("refs/heads/{branch}");
    let elsewhere = refs
        .lines()
        .any(|name| name != own && !name.ends_with("/HEAD"));
    Ok((!elsewhere).then_some(Unsafe::Unmerged))
}

fn unique_dir(parent: &Path, base: &str) -> Result<PathBuf> {
    let mut index = 0u32;
    loop {
        let name = if index == 0 {
            base.to_string()
        } else {
            format!("{base}-{index}")
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
        index += 1;
        if index > 1000 {
            return Err(Error::new(
                "worktree",
                "could not allocate a unique worktree path",
            ));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::git::tests::Repo;

    /// A clone of `origin` beside it, the way a project checkout tracks its remote.
    pub(crate) fn clone_of(origin: &Repo) -> PathBuf {
        let clone = origin.0.join("clone");
        git::run_ok(
            &origin.0,
            &[
                "clone",
                "-q",
                &display_path(&origin.cwd()),
                &display_path(&clone),
            ],
        )
        .unwrap();
        git::run_ok(&clone, &["config", "user.name", "Sirus Test"]).unwrap();
        git::run_ok(&clone, &["config", "user.email", "test@example.invalid"]).unwrap();
        clone
    }

    #[test]
    fn new_worktrees_start_from_origin_only_when_asked_and_fetched() {
        assert_eq!(start_point(false, Some("a".repeat(40))), "HEAD");
        assert_eq!(start_point(true, None), "HEAD");
        assert_eq!(start_point(true, Some(String::new())), "HEAD");
        assert_eq!(start_point(true, Some("b".repeat(40))), "b".repeat(40));

        let origin = Repo::new();
        let clone = clone_of(&origin);
        fs::write(origin.cwd().join("file.txt"), "after\n").unwrap();
        git::run_ok(&origin.cwd(), &["commit", "-qam", "upstream work"]).unwrap();
        let upstream = git::run_ok(&origin.cwd(), &["rev-parse", "HEAD"]).unwrap();
        let local = git::run_ok(&clone, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(upstream, local);
        // The fetch brings origin's newer commit; the local branch stays where it was.
        assert_eq!(origin_commit(&clone).as_deref(), Some(upstream.as_str()));
        assert_eq!(git::run_ok(&clone, &["rev-parse", "HEAD"]).unwrap(), local);
        // A repository without origin falls back to HEAD.
        assert_eq!(origin_commit(&origin.cwd()), None);
        // A named local branch gives its own commit; an unknown or invalid one gives none.
        assert_eq!(branch_commit(&clone, "nope"), None);
        assert_eq!(branch_commit(&clone, "-x"), None);
        let current = git::run_ok(&clone, &["branch", "--show-current"]).unwrap();
        assert_eq!(
            branch_commit(&clone, &current).as_deref(),
            Some(local.as_str())
        );
    }

    #[test]
    fn a_deleted_worktree_folder_is_restored_from_its_branch() {
        let repo = Repo::new();
        let parent = repo.0.join("worktrees").join("project-1");
        let tree = create_isolated(
            &repo.cwd(),
            &parent,
            "22222222-b",
            "Lost folder",
            "sirus/{session-name}",
        )
        .unwrap();
        fs::write(Path::new(&tree.path).join("note.txt"), "kept").unwrap();
        git::run_ok(Path::new(&tree.path), &["add", "."]).unwrap();
        git::run_ok(Path::new(&tree.path), &["commit", "-qm", "work"]).unwrap();
        fs::remove_dir_all(&tree.path).unwrap();
        assert!(
            restore(&repo.cwd(), "wrong-project", &tree).is_err(),
            "only the app's own project folder"
        );
        restore(&repo.cwd(), "project-1", &tree).unwrap();
        assert_eq!(
            fs::read_to_string(Path::new(&tree.path).join("note.txt")).unwrap(),
            "kept"
        );
        let gone = Worktree {
            branch: "sirus/never-existed".into(),
            ..tree.clone()
        };
        fs::remove_dir_all(&gone.path).unwrap();
        assert!(restore(&repo.cwd(), "project-1", &gone).is_err());
    }
    #[test]
    fn same_title_creates_distinct_branches_and_paths() {
        let repo = Repo::new();
        let parent = repo.0.join("worktrees");
        let first = create_isolated(
            &repo.cwd(),
            &parent,
            "11111111-a",
            "Fix tabs",
            "sirus/{session-name}",
        )
        .unwrap();
        let second = create_isolated(
            &repo.cwd(),
            &parent,
            "22222222-b",
            "Fix tabs",
            "sirus/{session-name}",
        )
        .unwrap();
        assert_ne!(first.branch, second.branch);
        assert_ne!(first.path, second.path);
    }
    #[test]
    fn removal_requires_confirmation_and_preserves_dirty_tree() {
        let repo = Repo::new();
        let tree = create_isolated(
            &repo.cwd(),
            &repo.0.join("worktrees"),
            "11111111-a",
            "Fix",
            "sirus/{session-name}-{id}",
        )
        .unwrap();
        assert!(remove(&repo.cwd(), &tree, false).is_err());
        fs::write(Path::new(&tree.path).join("important.txt"), "user work").unwrap();
        assert!(remove(&repo.cwd(), &tree, true).is_err());
        assert_eq!(
            fs::read_to_string(Path::new(&tree.path).join("important.txt")).unwrap(),
            "user work"
        );
    }
    #[test]
    fn release_needs_a_clean_tree_whose_commits_live_elsewhere() {
        let repo = Repo::new();
        let parent = repo.0.join("worktrees").join("project");
        let tree = create_isolated(
            &repo.cwd(),
            &parent,
            "11111111-a",
            "Fix",
            "sirus/{session-name}-{id}",
        )
        .unwrap();
        let path = PathBuf::from(&tree.path);
        // Fresh branch: its HEAD is still on the project's branch.
        assert_eq!(release_blocker(&path, &tree.branch).unwrap(), None);
        fs::write(path.join("new.txt"), "work").unwrap();
        assert_eq!(
            release_blocker(&path, &tree.branch).unwrap(),
            Some(Unsafe::Uncommitted)
        );
        git::run_ok(&path, &["add", "--", "new.txt"]).unwrap();
        git::run_ok(&path, &["commit", "-qm", "work"]).unwrap();
        assert_eq!(
            release_blocker(&path, &tree.branch).unwrap(),
            Some(Unsafe::Unmerged)
        );
        git::run_ok(&repo.cwd(), &["merge", "-q", "--ff-only", &tree.branch]).unwrap();
        assert_eq!(release_blocker(&path, &tree.branch).unwrap(), None);
        // Removal prunes metadata and the emptied project folder, keeping the branch.
        remove(&repo.cwd(), &tree, true).unwrap();
        assert!(!path.exists());
        assert!(!parent.exists());
        assert!(git::run_ok(&repo.cwd(), &["rev-parse", "--verify", &tree.branch]).is_ok());
    }
    #[test]
    fn invalid_branch_is_rejected_before_directory_creation() {
        let repo = Repo::new();
        let parent = repo.0.join("worktrees");
        assert!(create_isolated(&repo.cwd(), &parent, "11111111-a", "Fix", "../bad").is_err());
        assert!(!parent.exists());
    }
}
