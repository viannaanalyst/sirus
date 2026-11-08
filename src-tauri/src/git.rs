use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use crate::error::{Error, Result};
use crate::models::{
    BranchInfo, ChangeKind, FileChange, GitCommitResult, GitIdentity, GitPushResult, GitStatus,
    GitWorktree,
};

/// The Git executable. A Finder-launched app has PATH `/usr/bin:/bin`, where
/// `git` is Apple's shim: it resolves the real tool through `xcrun` first and
/// can stall for seconds when that lookup is cold. A real binary in a fixed
/// install location is preferred; the shim remains the fallback.
pub(crate) fn git_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        ["/opt/homebrew/bin/git", "/usr/local/bin/git"]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("git"))
    })
}

/// The PATH Git runs its helpers with: the provider CLIs' one, which adds the usual
/// install locations to the app's own. Read once; it does not change while the app runs.
fn git_path() -> &'static std::ffi::OsStr {
    static PATH: OnceLock<std::ffi::OsString> = OnceLock::new();
    PATH.get_or_init(crate::detect::cli_path)
}

/// GIT_CONFIG selects an alternate source only for `git config`, so retaining
/// it would make inspectors observe different configuration from operations.
/// All native Git commands share the same cwd, environment and safe overrides.
fn git_command(cwd: &Path) -> Command {
    let mut command = Command::new(git_binary());
    command
        .env_remove("GIT_CONFIG")
        // A native command has no stdin UI; fail instead of waiting on a prompt.
        .env("GIT_TERMINAL_PROMPT", "0")
        // Sirus polls status and diffs while agents run Git in the same tree. Never
        // take the optional `.git/index.lock` a stat refresh would (ADR-100), so an
        // agent's `git add`/`commit` cannot fail on a lock Sirus holds.
        .env("GIT_OPTIONAL_LOCKS", "0")
        // Opened from Finder or the Dock, the app's PATH lacks Homebrew: Git could not
        // start helpers such as `git-lfs` in an LFS repository.
        .env("PATH", git_path())
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            // Patches Sirus applies or parses keep their `a/` `b/` headers whatever the
            // person's own diff settings say.
            "-c",
            "diff.noprefix=false",
            "-c",
            "diff.mnemonicPrefix=false",
            "--literal-pathspecs",
        ])
        .current_dir(cwd);
    // Status polling, diff stats and the automatic pull run below the person (ADR-143).
    crate::process_priority::lower_in_background(&mut command);
    command
}

/// Bounded so a hostile configuration cannot stall operations, with headroom
/// for a slow first Git start on a busy machine.
fn inspect_config(cwd: &Path, args: &[&str]) -> Result<Output> {
    capture_git(cwd, args, std::time::Duration::from_secs(4))
}

fn capture_git(cwd: &Path, args: &[&str], duration: std::time::Duration) -> Result<Output> {
    let mut command = git_command(cwd);
    command.args(args);
    let capture = move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(crate::cli_output::capture_command(
            tokio::process::Command::from(command),
            duration,
        ))
    };
    // Normal callers are blocking workers. Also support synchronous use inside
    // a runtime (including fixture tests) without attempting a nested block_on.
    let output = if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::spawn(capture)
            .join()
            .map_err(|_| Error::git("cannot inspect Git configuration safely"))?
    } else {
        capture()
    };
    output.map_err(|_| Error::git("cannot inspect Git configuration safely"))
}

/// Native Git operations never opt into executable checkout filters from project config.
fn refuse_external_filters(cwd: &Path, args: &[&str]) -> Result<()> {
    let affected = matches!(
        args.first(),
        Some(&"status" | &"diff" | &"diff-index" | &"add" | &"checkout" | &"apply" | &"stash")
    ) || (args.first() == Some(&"worktree")
        && matches!(args.get(1), Some(&"add" | &"remove")));
    if !affected {
        return Ok(());
    }
    let config = inspect_config(
        cwd,
        &[
            "config",
            "--includes",
            "--null",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process)$",
            "[^[:space:]]",
        ],
    )?;
    // Exit 1 is the documented no-match result, not a configuration failure.
    if !config.status.success() && config.status.code() != Some(1) {
        return Err(Error::git("cannot inspect Git filter configuration safely"));
    }
    if !config.stdout.is_empty() {
        return Err(Error::git("External Git filters are configured. Automatic status, diff and worktree operations are unavailable because filters can execute project commands."));
    }
    Ok(())
}

/// Git 2.54 configured hooks are independent of core.hooksPath.
/// Refuse them conservatively rather than executing or mutating user config.
fn refuse_configured_hooks(cwd: &Path) -> Result<()> {
    let config = inspect_config(
        cwd,
        &[
            "config",
            "--includes",
            "--null",
            "--name-only",
            "--get-regexp",
            r"^hook\.",
        ],
    )?;
    if !config.status.success() && config.status.code() != Some(1) {
        return Err(Error::git("cannot inspect configured Git hooks safely"));
    }
    if !config.stdout.is_empty() {
        return Err(Error::git("Configured Git hooks are present. Automatic Git operations are unavailable because these hooks can execute project commands."));
    }
    Ok(())
}

/// Bounded read-only metadata probes for native PR context. Never exposed as IPC.
pub(crate) fn bounded_probe(cwd: &Path, args: &[&str]) -> Result<Output> {
    refuse_configured_hooks(cwd)?;
    inspect_config(cwd, args)
}

/// Fixed native workspace operations, bounded to 2 MiB stdout / 64 KiB stderr.
pub(crate) fn bounded_run(cwd: &Path, args: &[&str]) -> Result<Output> {
    refuse_configured_hooks(cwd)?;
    refuse_external_filters(cwd, args)?;
    capture_git(cwd, args, std::time::Duration::from_secs(10)).map_err(|_| Error::git("Git operation exceeded its time/output limits or could not run. Refresh before retrying."))
}

pub fn run(cwd: &Path, args: &[&str]) -> Result<Output> {
    refuse_configured_hooks(cwd)?;
    refuse_external_filters(cwd, args)?;
    let output = git_command(cwd)
        .args(args)
        .output()
        .map_err(|err| Error::git(format!("git is not available: {err}")))?;
    Ok(output)
}

/// Same guards as `run`, with extra environment (team snapshots use a private temporary index).
pub fn run_env(cwd: &Path, args: &[&str], env: &[(&str, &std::ffi::OsStr)]) -> Result<Output> {
    refuse_configured_hooks(cwd)?;
    refuse_external_filters(cwd, args)?;
    let mut command = git_command(cwd);
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    command
        .output()
        .map_err(|err| Error::git(format!("git is not available: {err}")))
}

/// Same guards as `run`, with bounded input on stdin (team merge patches).
pub fn run_input(cwd: &Path, args: &[&str], input: &[u8]) -> Result<Output> {
    use std::io::Write;
    refuse_configured_hooks(cwd)?;
    refuse_external_filters(cwd, args)?;
    let mut child = git_command(cwd)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| Error::git(format!("git is not available: {err}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::git("cannot write to git"))?;
    let data = input.to_vec();
    // Write on a separate thread so a full stdout/stderr pipe cannot deadlock the writer.
    let writer = std::thread::spawn(move || stdin.write_all(&data));
    let output = child
        .wait_with_output()
        .map_err(|err| Error::git(format!("git failed: {err}")))?;
    writer
        .join()
        .map_err(|_| Error::git("cannot write to git"))?
        .map_err(|err| Error::git(format!("cannot write to git: {err}")))?;
    Ok(output)
}

pub fn run_ok(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = run(cwd, args)?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(Error::git(if stderr.is_empty() {
        format!("git {} failed", args.join(" "))
    } else {
        stderr
    }))
}

const MAX_COMMIT_MESSAGE_BYTES: usize = 8 * 1024;

/// Explicit local commit. Hooks stay disabled/refused like every other native Git
/// operation, and the message travels as a single argv entry (no shell string).
pub fn commit_checked(
    cwd: &Path,
    message: &str,
    expected: Option<&str>,
    admit: impl FnOnce() -> Result<()>,
) -> Result<GitCommitResult> {
    commit_or_amend(cwd, message, expected, false, admit)
}

/// Replaces the last commit with what is staged. An empty message keeps the commit's own. It
/// refuses a commit already on a remote, whose replacement could not be pushed without force.
pub fn amend_checked(
    cwd: &Path,
    message: &str,
    expected: Option<&str>,
    admit: impl FnOnce() -> Result<()>,
) -> Result<GitCommitResult> {
    commit_or_amend(cwd, message, expected, true, admit)
}

fn commit_or_amend(
    cwd: &Path,
    message: &str,
    expected: Option<&str>,
    amend: bool,
    admit: impl FnOnce() -> Result<()>,
) -> Result<GitCommitResult> {
    let _write = crate::git_workspace::INDEX_WRITE.lock();
    let message = message.trim();
    if message.is_empty() && !amend {
        return Err(Error::new("invalid", "commit message is empty"));
    }
    if message.len() > MAX_COMMIT_MESSAGE_BYTES {
        return Err(Error::new(
            "invalid",
            "commit message exceeds the 8 KiB limit",
        ));
    }
    if amend {
        crate::git_workspace::amend_guard(cwd, expected)?;
        let on_remote = bounded_run(cwd, &["branch", "-r", "--contains", "HEAD"])?;
        if !on_remote.status.success() {
            return Err(Error::git("There is no commit to amend."));
        }
        if !String::from_utf8_lossy(&on_remote.stdout).trim().is_empty() {
            return Err(Error::git(
                "The last commit is already on a remote, so it can't be amended.",
            ));
        }
    } else {
        crate::git_workspace::commit_guard(cwd, expected)?;
    }
    admit()?;
    let mut args = vec!["commit"];
    if amend {
        args.push("--amend");
    }
    if message.is_empty() {
        args.push("--no-edit");
    } else {
        args.extend(["-m", message]);
    }
    let output = bounded_run(cwd, &args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::git(if stderr.is_empty() {
            "git commit failed".to_string()
        } else {
            stderr
        }));
    }
    let hash = bounded_run(cwd, &["rev-parse", "HEAD"])?;
    let summary = bounded_run(cwd, &["log", "-1", "--no-show-signature", "--pretty=%s"])?;
    if !hash.status.success() || !summary.status.success() {
        return Err(Error::git(
            "Commit completed, but its metadata is unavailable. Refresh before continuing.",
        ));
    }
    let hash = String::from_utf8_lossy(&hash.stdout).trim().to_owned();
    let summary = String::from_utf8_lossy(&summary.stdout).trim().to_owned();
    Ok(GitCommitResult { hash, summary })
}

/// Project-local Git config can redirect a push (credential helpers, ssh
/// commands, url rewrites, proxies). Native Git never executes project-provided
/// commands implicitly, so a configured override makes push unavailable.
fn refuse_project_network_helpers(cwd: &Path) -> Result<()> {
    let config = inspect_config(
        cwd,
        &[
            "config",
            "--local",
            "--null",
            "--name-only",
            "--get-regexp",
            r"^(credential\.|core\.(sshcommand|gitproxy|askpass)$|remote\..*\.(uploadpack|receivepack|proxy)$|url\..*\.insteadof$|http.*\.proxy$)",
            "[^[:space:]]",
        ],
    )?;
    if !config.status.success() && config.status.code() != Some(1) {
        return Err(Error::git(
            "cannot inspect Git network configuration safely",
        ));
    }
    if !config.stdout.is_empty() {
        return Err(Error::git("This repository configures Git network helpers. Push is unavailable because project configuration could redirect credentials or connections."));
    }
    Ok(())
}

/// Push of the current branch to origin. Never forced and refused on a
/// detached HEAD. Only explicit actions call it, plus the owner-enabled CI
/// auto-fix after its own fix commit (ADR-064).
pub fn push(cwd: &Path) -> Result<GitPushResult> {
    refuse_project_network_helpers(cwd)?;
    let branch = {
        let output = run(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        if !output.status.success() {
            return Err(Error::git("cannot push a detached HEAD"));
        }
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    if branch.is_empty() {
        return Err(Error::git("cannot push a detached HEAD"));
    }
    let remote = run_ok(cwd, &["remote", "get-url", "origin"])
        .map_err(|_| Error::git("no origin remote is configured"))?;
    if remote.trim().is_empty() {
        return Err(Error::git("no origin remote is configured"));
    }
    let output = run(cwd, &["push", "origin", "HEAD"])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::git(if stderr.is_empty() {
            "git push failed".to_string()
        } else {
            stderr
        }));
    }
    Ok(GitPushResult { branch })
}

/// Publishes one commit as a new branch on origin (moving a conversation to another computer,
/// ADR-133): `git push origin <commit>:refs/heads/<branch>`, never forced, under the same
/// network-helper refusal as push. The checkout, its branch and its index are not touched.
pub fn push_commit_to_branch(cwd: &Path, commit: &str, branch: &str) -> Result<()> {
    if !valid_branch_chars(branch) || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::git("invalid branch or commit"));
    }
    refuse_project_network_helpers(cwd)?;
    let remote = run_ok(cwd, &["remote", "get-url", "origin"])
        .map_err(|_| Error::git("no origin remote is configured"))?;
    if remote.trim().is_empty() {
        return Err(Error::git("no origin remote is configured"));
    }
    let refspec = format!("{commit}:refs/heads/{branch}");
    let output = run(cwd, &["push", "origin", &refspec])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::git(if stderr.is_empty() {
            "git push failed".to_string()
        } else {
            stderr
        }));
    }
    Ok(())
}

/// Explicit pull of the current branch from origin: fast-forward only, never a
/// merge or rebase, with the same network-helper refusal as push. Git itself
/// refuses when local changes would be overwritten.
pub fn pull(cwd: &Path) -> Result<(String, String)> {
    refuse_project_network_helpers(cwd)?;
    let output = run(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || branch.is_empty() {
        return Err(Error::git("cannot pull into a detached HEAD"));
    }
    let remote = run_ok(cwd, &["remote", "get-url", "origin"])
        .map_err(|_| Error::git("no origin remote is configured"))?;
    if remote.trim().is_empty() {
        return Err(Error::git("no origin remote is configured"));
    }
    let output = run(
        cwd,
        &[
            "pull",
            "--ff-only",
            "--no-rebase",
            "--no-edit",
            "origin",
            &branch,
        ],
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::git(if stderr.is_empty() {
            "git pull failed".to_string()
        } else {
            stderr.chars().take(600).collect()
        }));
    }
    let summary = stdout
        .lines()
        .last()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect();
    Ok((branch, summary))
}

/// The default branch on `origin` (`origin/HEAD`, else `main` or `master` when origin has
/// one), as a branch name without the remote. None without an origin or a valid name.
pub fn origin_default_branch(cwd: &Path) -> Option<String> {
    let head = run(
        cwd,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
    .ok()
    .filter(|output| output.status.success())
    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    .and_then(|name| name.strip_prefix("origin/").map(str::to_string));
    let branch = head.or_else(|| {
        ["main", "master"]
            .into_iter()
            .map(str::to_string)
            .find(|name| {
                run(
                    cwd,
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        &format!("refs/remotes/origin/{name}"),
                    ],
                )
                .is_ok_and(|output| output.status.success())
            })
    })?;
    (valid_branch_chars(&branch) && run_ok(cwd, &["check-ref-format", "--branch", &branch]).is_ok())
        .then_some(branch)
}

/// Fetches one branch from `origin` into `refs/remotes/origin/<branch>`, bounded to 30 s and
/// refused under the same project network-helper rule as push and pull (ADR-130).
pub fn fetch_origin_branch(cwd: &Path, branch: &str) -> Result<()> {
    fetch_branch(
        cwd,
        "origin",
        branch,
        &format!("refs/remotes/origin/{branch}"),
        std::time::Duration::from_secs(30),
    )
}

/// Fetches `refs/heads/<branch>` of `remote` into the remote-tracking ref `tracking` and
/// nothing else (no tags, no FETCH_HEAD), bounded by `timeout`, under the same hook and
/// project network-helper refusals as push and pull (ADR-130, ADR-148).
pub(crate) fn fetch_branch(
    cwd: &Path,
    remote: &str,
    branch: &str,
    tracking: &str,
    timeout: std::time::Duration,
) -> Result<()> {
    if !valid_remote_name(remote)
        || !valid_branch_chars(branch)
        || !tracking
            .strip_prefix("refs/remotes/")
            .is_some_and(valid_branch_chars)
    {
        return Err(Error::git("invalid branch name"));
    }
    refuse_configured_hooks(cwd)?;
    refuse_project_network_helpers(cwd)?;
    let refspec = format!("+refs/heads/{branch}:{tracking}");
    let output = capture_git(
        cwd,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            remote,
            &refspec,
        ],
        timeout,
    )
    .map_err(|_| Error::git("git fetch did not finish in time"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::git("git fetch failed"))
    }
}

/// `git pull --ff-only <remote> <branch>` for the header's Pull (ADR-148): never a merge or
/// rebase, bounded to 30 s, under the same hook and network-helper refusals as push. The raw
/// output is returned so the caller can tell blocking files and divergence apart.
pub(crate) fn pull_fast_forward(cwd: &Path, remote: &str, branch: &str) -> Result<Output> {
    if !valid_remote_name(remote) || !valid_branch_chars(branch) {
        return Err(Error::git("invalid branch name"));
    }
    refuse_configured_hooks(cwd)?;
    refuse_project_network_helpers(cwd)?;
    capture_git(
        cwd,
        &[
            "pull",
            "--ff-only",
            "--no-rebase",
            "--no-edit",
            "--no-stat",
            remote,
            branch,
        ],
        std::time::Duration::from_secs(30),
    )
    .map_err(|_| Error::git("git pull did not finish in 30 seconds."))
}

/// A remote's name as Git config keeps it; no option-like or path-like names.
pub(crate) fn valid_remote_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with(['-', '.'])
        && !name.contains("..")
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
}

/// Normalize an `origin` remote to a web URL. Only GitHub is exposed, and the
/// renderer receives a fixed https URL, never the raw remote.
pub fn remote_web_url(path: &Path) -> Result<Option<String>> {
    let output = run(path, &["remote", "get-url", "origin"])?;
    if !output.status.success() {
        return Ok(None);
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(normalize_github_web_url(&raw))
}

pub(crate) fn normalize_github_web_url(raw: &str) -> Option<String> {
    let path = raw
        .strip_prefix("https://github.com/")
        .or_else(|| raw.strip_prefix("http://github.com/"))
        .or_else(|| raw.strip_prefix("git@github.com:"))
        .or_else(|| raw.strip_prefix("ssh://git@github.com/"))?
        .trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty()
        || path.len() > 512
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment.starts_with('.') || segment.contains(".."))
        || path.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~'))
        })
    {
        return None;
    }
    Some(format!("https://github.com/{path}"))
}

const MAX_BRANCHES: usize = 500;
const MAX_BRANCH_NAME: usize = 120;

pub(crate) fn valid_branch_chars(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_BRANCH_NAME
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.ends_with('/')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '-' | '_')
        })
}

pub fn list_branches(cwd: &Path) -> Result<Vec<BranchInfo>> {
    let output = run(cwd, &["branch", "--format=%(refname:short)%09%(HEAD)"])?;
    if !output.status.success() {
        return Err(Error::git("git branch failed"));
    }
    let mut branches = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        let name = parts.next().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let current = parts.next().map(str::trim) == Some("*");
        branches.push(BranchInfo {
            name: name.to_string(),
            current,
        });
        if branches.len() >= MAX_BRANCHES {
            break;
        }
    }
    Ok(branches)
}

/// Explicit branch switch in the project checkout. The branch must already
/// exist locally; hooks/filters guards apply like every native Git operation.
pub fn checkout_branch(cwd: &Path, branch: &str) -> Result<()> {
    if !valid_branch_chars(branch) {
        return Err(Error::new("invalid", "Invalid branch name"));
    }
    if !list_branches(cwd)?.iter().any(|entry| entry.name == branch) {
        return Err(Error::new("invalid", "Branch not found"));
    }
    let output = run(cwd, &["checkout", branch])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::git(if stderr.is_empty() {
            "git checkout failed".to_string()
        } else {
            stderr
        }));
    }
    Ok(())
}

/// Explicit new-branch creation in the project checkout.
pub fn create_branch(cwd: &Path, branch: &str) -> Result<()> {
    if !valid_branch_chars(branch) {
        return Err(Error::new("invalid", "Invalid branch name"));
    }
    let check = run(cwd, &["check-ref-format", "--branch", branch])?;
    if !check.status.success() {
        return Err(Error::new("invalid", "Invalid branch name"));
    }
    let output = run(cwd, &["checkout", "-b", branch])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::git(if stderr.is_empty() {
            "git checkout -b failed".to_string()
        } else {
            stderr
        }));
    }
    Ok(())
}

pub fn identity(path: &Path) -> Result<GitIdentity> {
    let inside = run(path, &["rev-parse", "--is-inside-work-tree"])?;
    if !inside.status.success() || String::from_utf8_lossy(&inside.stdout).trim() != "true" {
        return Ok(GitIdentity {
            is_repo: false,
            root: None,
            branch: None,
            detached: false,
        });
    }
    let root = run_ok(path, &["rev-parse", "--show-toplevel"]).ok();
    let detached = run_ok(path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .as_deref()
        == Some("HEAD");
    let branch = if detached {
        run_ok(path, &["rev-parse", "--short", "HEAD"]).ok()
    } else {
        run_ok(path, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()
    };
    Ok(GitIdentity {
        is_repo: true,
        root,
        branch,
        detached,
    })
}

/// Lines added and removed in the checkout, for the project row's `+N -M`: tracked
/// changes against HEAD (staged and unstaged) plus the lines of new, untracked text
/// files. Bounded: up to 500 new files of at most 1 MiB each; binaries count as nothing.
pub fn diff_totals(path: &Path) -> Result<(u32, u32)> {
    if !identity(path)?.is_repo {
        return Ok((0, 0));
    }
    let numstat = run(
        path,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--numstat",
            "HEAD",
        ],
    )?;
    // No commit yet: every file is new, counted below.
    let (mut added, removed) = if numstat.status.success() {
        sum_numstat(&String::from_utf8_lossy(&numstat.stdout))
    } else {
        (0, 0)
    };
    let untracked = run(path, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    if untracked.status.success() {
        for name in String::from_utf8_lossy(&untracked.stdout)
            .split('\0')
            .filter(|name| !name.is_empty())
            .take(500)
        {
            added = added.saturating_add(text_lines(&path.join(name)));
        }
    }
    Ok((added, removed))
}

/// Lines of a small text file; binaries, links and large files count as nothing.
fn text_lines(file: &Path) -> u32 {
    const LIMIT: u64 = 1024 * 1024;
    let Ok(metadata) = std::fs::symlink_metadata(file) else {
        return 0;
    };
    if !metadata.is_file() || metadata.len() > LIMIT {
        return 0;
    }
    let Ok(bytes) = std::fs::read(file) else {
        return 0;
    };
    if bytes.is_empty() || bytes.contains(&0) {
        return 0;
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count() as u32;
    newlines + u32::from(!bytes.ends_with(b"\n"))
}

/// Sums `git diff --numstat` rows; binary files (`-`) count as nothing.
pub fn sum_numstat(text: &str) -> (u32, u32) {
    text.lines().fold((0, 0), |(added, removed), row| {
        let mut columns = row.split('\t');
        let plus = columns
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let minus = columns
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        (added.saturating_add(plus), removed.saturating_add(minus))
    })
}

/// Untracked files listed in one status; the rest are counted out (`untracked_truncated`).
pub const MAX_UNTRACKED: usize = 2000;

pub fn status(path: &Path) -> Result<GitStatus> {
    let identity = identity(path)?;
    if !identity.is_repo {
        return Ok(GitStatus {
            identity,
            dirty: false,
            ahead: 0,
            behind: 0,
            changes: vec![],
            untracked_truncated: false,
        });
    }

    let mut ahead = 0u32;
    let mut behind = 0u32;
    if let Ok(counts) = run_ok(
        path,
        &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    ) {
        let parts: Vec<&str> = counts.split_whitespace().collect();
        if parts.len() == 2 {
            behind = parts[0].parse().unwrap_or(0);
            ahead = parts[1].parse().unwrap_or(0);
        }
    }

    let porcelain = run(
        path,
        &["status", "--porcelain=v1", "-z", "-uall", "--", "."],
    )?;
    if !porcelain.status.success() {
        return Err(Error::git(String::from_utf8_lossy(&porcelain.stderr)));
    }
    // Plumbing: porcelain `git diff` still rewrites the index to refresh stat
    // data even with GIT_OPTIONAL_LOCKS=0. `-M` keeps porcelain's rename detection.
    let numstat = run(
        path,
        &[
            "diff-index",
            "-M",
            "--no-ext-diff",
            "--no-textconv",
            "--numstat",
            "--relative",
            "-z",
            "HEAD",
        ],
    )?;
    let mut stats = std::collections::HashMap::<String, (u32, u32)>::new();
    let text = String::from_utf8_lossy(&numstat.stdout);
    let mut fields = text.split('\0');
    while let Some(row) = fields.next() {
        let cols: Vec<_> = row.splitn(3, '\t').collect();
        if cols.len() != 3 {
            continue;
        }
        let name = if cols[2].is_empty() {
            let _old = fields.next();
            fields.next().unwrap_or_default()
        } else {
            cols[2]
        };
        stats.insert(
            name.to_string(),
            (cols[0].parse().unwrap_or(0), cols[1].parse().unwrap_or(0)),
        );
    }
    let root = identity
        .root
        .as_ref()
        .ok_or_else(|| Error::git("Git root is unavailable"))?;
    let canonical = path.canonicalize()?;
    let root = Path::new(root).canonicalize()?;
    let scope = canonical
        .strip_prefix(&root)
        .map_err(|_| Error::git("workspace is outside Git root"))?;
    let text = String::from_utf8_lossy(&porcelain.stdout);
    let mut rows = text.split('\0');
    let mut changes = Vec::new();
    let mut untracked = 0usize;
    let mut untracked_truncated = false;
    while let Some(row) = rows.next() {
        if row.len() < 4 {
            continue;
        }
        let code = &row.as_bytes()[..2];
        let path_name = row[3..].to_string();
        let kind = if code.contains(&b'R') || code.contains(&b'C') {
            let _original = rows.next();
            ChangeKind::Renamed
        } else if code.contains(&b'?') {
            ChangeKind::Untracked
        } else if code.contains(&b'A') {
            ChangeKind::Added
        } else if code.contains(&b'D') {
            ChangeKind::Deleted
        } else {
            ChangeKind::Modified
        };
        let Ok(relative) = Path::new(&path_name).strip_prefix(scope) else {
            continue;
        };
        if kind == ChangeKind::Untracked {
            // A build output or vendored tree left untracked can list hundreds of
            // thousands of files; the UI gets a bounded list and a flag (ADR-100).
            untracked += 1;
            if untracked > MAX_UNTRACKED {
                untracked_truncated = true;
                continue;
            }
        }
        let path_name = relative.to_string_lossy().into_owned();
        #[cfg(windows)]
        let path_name = path_name.replace('\\', "/");
        let (additions, deletions) = stats.get(&path_name).copied().unwrap_or((0, 0));
        changes.push(FileChange {
            path: path_name,
            kind,
            additions,
            deletions,
        });
    }

    Ok(GitStatus {
        dirty: !changes.is_empty(),
        identity,
        ahead,
        behind,
        changes,
        untracked_truncated,
    })
}

pub fn diff_file(cwd: &Path, rel_path: &str) -> Result<String> {
    let relative = Path::new(rel_path);
    if rel_path.is_empty()
        || rel_path.contains('\0')
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(Error::invalid_path(
            "refusing absolute or traversing diff path",
        ));
    }
    let target = cwd.join(relative);
    // Deleted files have no canonical target; their parent still must remain jailed.
    if target.exists() || target.symlink_metadata().is_ok() {
        crate::paths::ensure_within(cwd, &target)?;
    } else {
        let parent = target
            .ancestors()
            .skip(1)
            .find(|parent| parent.exists() || parent.symlink_metadata().is_ok())
            .unwrap_or(cwd);
        crate::paths::ensure_within(cwd, parent)?;
    }
    const PREVIEW_LIMIT: u64 = 2 * 1024 * 1024;
    if let Ok(metadata) = std::fs::metadata(&target) {
        if !metadata.is_file() {
            return Err(Error::invalid_path("preview supports regular files only"));
        }
        if metadata.len() > PREVIEW_LIMIT {
            return Err(Error::new(
                "diff",
                "file is too large to preview (2 MiB limit)",
            ));
        }
    }
    let tracked = run(cwd, &["ls-files", "--error-unmatch", "--", rel_path])?;
    let has_head = run(cwd, &["rev-parse", "--verify", "HEAD"])?
        .status
        .success();
    // Git revision paths prefixed with ./ resolve against cwd, even for a nested
    // project. Check both base and index so an unborn or deleted blob is bounded.
    for object in [format!("HEAD:./{rel_path}"), format!(":./{rel_path}")] {
        if run_ok(cwd, &["cat-file", "-t", &object]).ok().as_deref() == Some("tree") {
            return Err(Error::invalid_path("diff preview requires a file"));
        }
        if run_ok(cwd, &["cat-file", "-s", &object])
            .ok()
            .and_then(|size| size.parse::<u64>().ok())
            .is_some_and(|size| size > PREVIEW_LIMIT)
        {
            return Err(Error::new(
                "diff",
                "file is too large to preview (2 MiB limit)",
            ));
        }
    }
    if tracked.status.success() || has_head {
        let args = if has_head {
            vec![
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "HEAD",
                "--",
                rel_path,
            ]
        } else {
            vec![
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--cached",
                "--",
                rel_path,
            ]
        };
        let output = run(cwd, &args)?;
        if !output.status.success() {
            return Err(Error::git(String::from_utf8_lossy(&output.stderr)));
        }
        if tracked.status.success() || !output.stdout.is_empty() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
    }
    use std::io::Read;
    let file = crate::paths::open_regular_within(cwd, &target)?;
    let mut bytes = Vec::new();
    file.take(PREVIEW_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > PREVIEW_LIMIT {
        return Err(Error::new(
            "diff",
            "file is too large to preview (2 MiB limit)",
        ));
    }
    let contents = String::from_utf8(bytes)
        .map_err(|_| Error::new("diff", "binary file cannot be previewed as text"))?;
    Ok(format!("+++ untracked\n{contents}"))
}

pub fn list_worktrees(cwd: &Path) -> Result<Vec<GitWorktree>> {
    let output = run(cwd, &["worktree", "list", "--porcelain", "-z"])?;
    if !output.status.success() {
        return Err(Error::git(String::from_utf8_lossy(&output.stderr)));
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| Error::git("worktree paths are not UTF-8"))?;
    parse_worktree_inventory(text)
}

fn parse_worktree_inventory(text: &str) -> Result<Vec<GitWorktree>> {
    let mut entries = Vec::new();
    let mut current: Option<GitWorktree> = None;
    for field in text.split('\0') {
        if field.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }
        if let Some(path) = field.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            if path.is_empty() {
                return Err(Error::git("worktree path is empty"));
            }
            current = Some(GitWorktree {
                path: path.into(),
                ..GitWorktree::default()
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            return Err(Error::git("invalid worktree listing"));
        };
        if let Some(head) = field.strip_prefix("HEAD ") {
            entry.head = Some(head.into());
        } else if let Some(branch) = field.strip_prefix("branch ") {
            entry.branch = Some(branch.strip_prefix("refs/heads/").unwrap_or(branch).into());
        } else if field == "detached" {
            entry.detached = true;
        } else if field == "bare" {
            entry.bare = true;
        } else if field == "locked" || field.starts_with("locked ") {
            entry.locked = Some(field.strip_prefix("locked ").unwrap_or("").into());
        } else if field == "prunable" || field.starts_with("prunable ") {
            entry.prunable = Some(field.strip_prefix("prunable ").unwrap_or("").into());
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn numstat_rows_add_up_and_binaries_count_as_nothing() {
        assert_eq!(
            sum_numstat("10\t2\tsrc/a.ts\n44\t11\tsrc/b.ts\n-\t-\tlogo.png\n"),
            (54, 13)
        );
        assert_eq!(sum_numstat(""), (0, 0));
    }

    #[test]
    fn project_totals_count_uncommitted_lines() {
        let repo = Repo::new();
        fs::write(repo.cwd().join("a.txt"), "one\ntwo\n").unwrap();
        run_ok(&repo.cwd(), &["add", "."]).unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "init"]).unwrap();
        assert_eq!(diff_totals(&repo.cwd()).unwrap(), (0, 0));
        fs::write(repo.cwd().join("a.txt"), "one\nTWO\nthree\n").unwrap();
        assert_eq!(diff_totals(&repo.cwd()).unwrap(), (2, 1));
        // New files count their lines; binaries count as nothing.
        fs::write(repo.cwd().join("new.md"), "a\nb\nc").unwrap();
        fs::write(repo.cwd().join("logo.bin"), [0u8, 1, 2]).unwrap();
        assert_eq!(diff_totals(&repo.cwd()).unwrap(), (5, 1));
    }
    #[test]
    fn patches_keep_their_prefixes_and_helpers_find_homebrew() {
        let repo = Repo::new();
        run_ok(&repo.cwd(), &["config", "diff.noprefix", "true"]).unwrap();
        run_ok(&repo.cwd(), &["config", "diff.mnemonicPrefix", "true"]).unwrap();
        fs::write(repo.cwd().join("file.txt"), "after\n").unwrap();
        let patch = run_ok(&repo.cwd(), &["diff"]).unwrap();
        assert!(patch.contains("--- a/file.txt") && patch.contains("+++ b/file.txt"));
        #[cfg(unix)]
        assert!(std::env::split_paths(git_path()).any(|dir| dir == Path::new("/opt/homebrew/bin")));
    }
    pub(crate) struct Repo(pub(crate) std::path::PathBuf);
    impl Repo {
        pub(crate) fn new() -> Self {
            let path = std::env::temp_dir().join(format!("sirus-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(path.join("repo")).unwrap();
            let repo = Self(path);
            run_ok(&repo.cwd(), &["init", "-q"]).unwrap();
            run_ok(&repo.cwd(), &["config", "user.name", "Sirus Test"]).unwrap();
            run_ok(
                &repo.cwd(),
                &["config", "user.email", "test@example.invalid"],
            )
            .unwrap();
            fs::write(repo.cwd().join("file.txt"), "before\n").unwrap();
            run_ok(&repo.cwd(), &["add", "--", "file.txt"]).unwrap();
            run_ok(&repo.cwd(), &["commit", "-qm", "fixture"]).unwrap();
            repo
        }
        pub(crate) fn cwd(&self) -> std::path::PathBuf {
            self.0.join("repo")
        }
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn worktree_inventory_preserves_delimiters_and_status() {
        let raw = "worktree /tmp/tree with\nnewline\0HEAD abcd\0branch refs/heads/feature\0locked reason\0\0worktree /tmp/detached\0HEAD efgh\0detached\0prunable stale\0\0";
        let entries = parse_worktree_inventory(raw).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "/tmp/tree with\nnewline");
        assert_eq!(entries[0].branch.as_deref(), Some("feature"));
        assert_eq!(entries[0].locked.as_deref(), Some("reason"));
        assert!(entries[1].detached);
        assert_eq!(entries[1].prunable.as_deref(), Some("stale"));
    }
    #[test]
    fn nested_project_status_and_diff_use_workspace_relative_paths() {
        let repo = Repo::new();
        fs::create_dir(repo.cwd().join("nested")).unwrap();
        fs::write(repo.cwd().join("nested/in.txt"), "before\n").unwrap();
        run_ok(&repo.cwd(), &["add", "nested/in.txt"]).unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "nested fixture"]).unwrap();
        fs::write(repo.cwd().join("nested/in.txt"), "after\n").unwrap();
        fs::write(repo.cwd().join("file.txt"), "outside scope\n").unwrap();
        let cwd = repo.cwd().join("nested");
        let status = status(&cwd).unwrap();
        assert_eq!(status.changes.len(), 1);
        assert_eq!(status.changes[0].path, "in.txt");
        assert_eq!(status.changes[0].additions, 1);
        assert!(diff_file(&cwd, "in.txt").unwrap().contains("+after"));
    }
    #[test]
    fn configured_hooks_are_refused_before_worktree_or_branch_creation() {
        let repo = Repo::new();
        for (key, value) in [
            (
                "hook.sentinel.command",
                "printf unsafe > configured-hook-ran.txt",
            ),
            ("hook.sentinel.event", "post-checkout"),
        ] {
            assert!(std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(repo.cwd())
                .status()
                .unwrap()
                .success());
        }
        let parent = repo.0.join("guarded-trees");
        let result = crate::worktree::create_isolated(
            &repo.cwd(),
            &parent,
            "guarded",
            "Guarded",
            "sirus/{session-name}",
        );
        assert!(
            result.is_err(),
            "configured hooks must not execute during native worktree creation"
        );
        assert!(!parent.exists());
        assert!(!repo.cwd().join("configured-hook-ran.txt").exists());
        assert!(!format!("{}", result.unwrap_err()).contains("printf"));
        let output = std::process::Command::new("git")
            .args(["branch", "--list", "sirus/guarded-guarded"])
            .current_dir(repo.cwd())
            .output()
            .unwrap();
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn config_override_cannot_hide_hook_or_filter_guards() {
        const FIXTURE: &str = "SIRUS_GIT_CONFIG_FIXTURE";
        if let Some(path) = std::env::var_os(FIXTURE) {
            let cwd = std::path::PathBuf::from(path);
            let destination = cwd.parent().unwrap().join("override-tree");
            let result = run(
                &cwd,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    destination.to_str().unwrap(),
                    "HEAD",
                ],
            );
            assert!(
                result.is_err(),
                "alternate config must not conceal executable project configuration"
            );
            assert!(!destination.exists());
            assert!(!cwd.parent().unwrap().join("override-sentinel").exists());
            return;
        }
        for hook in [true, false] {
            let repo = Repo::new();
            let sentinel = repo.0.join("override-sentinel");
            let command = format!("touch '{}'; cat", sentinel.display());
            let entries = if hook {
                vec![
                    ("hook.override.command", command.as_str()),
                    ("hook.override.event", "post-checkout"),
                ]
            } else {
                fs::write(
                    repo.cwd().join(".gitattributes"),
                    "file.txt filter=override\n",
                )
                .unwrap();
                run_ok(&repo.cwd(), &["add", ".gitattributes"]).unwrap();
                run_ok(&repo.cwd(), &["commit", "-qm", "attributes"]).unwrap();
                vec![("filter.override.smudge", command.as_str())]
            };
            for (key, value) in entries {
                assert!(Command::new("git")
                    .args(["config", key, value])
                    .current_dir(repo.cwd())
                    .status()
                    .unwrap()
                    .success());
            }
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "git::tests::config_override_cannot_hide_hook_or_filter_guards",
                    "--nocapture",
                ])
                .env(FIXTURE, repo.cwd())
                .env("GIT_CONFIG", "/dev/null")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                )
            );
            assert!(!sentinel.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn included_fifo_configuration_is_refused_within_probe_deadline() {
        let repo = Repo::new();
        let fifo = repo.0.join("included-config");
        let path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(Command::new("git")
            .args(["config", "include.path", fifo.to_str().unwrap()])
            .current_dir(repo.cwd())
            .status()
            .unwrap()
            .success());
        let started = std::time::Instant::now();
        let error = run(&repo.cwd(), &["status", "--porcelain"]).unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(7));
        assert_eq!(error.to_string(), "cannot inspect Git configuration safely");
    }

    #[test]
    fn oversized_configured_hook_keys_are_refused_without_returning_values() {
        let repo = Repo::new();
        use std::io::Write;
        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(repo.cwd().join(".git/config"))
            .unwrap();
        // Arbitrary included config can produce output far larger than a normal
        // catalog. Names-only inspection must still enforce the capture ceiling.
        let name = "x".repeat(1024);
        for index in 0..2200 {
            writeln!(
                config,
                "[hook \"{name}{index}\"]\n command = private-command-value"
            )
            .unwrap();
        }
        let error = run(&repo.cwd(), &["status", "--porcelain"]).unwrap_err();
        assert_eq!(error.to_string(), "cannot inspect Git configuration safely");
    }

    #[cfg(unix)]
    #[test]
    fn app_git_commands_do_not_execute_checkout_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let repo = Repo::new();
        let hook = repo.cwd().join(".git/hooks/post-checkout");
        fs::write(&hook, "#!/bin/sh\nprintf unsafe > hook-ran.txt\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        let tree = crate::worktree::create_isolated(
            &repo.cwd(),
            &repo.0.join("trees"),
            "hooktest",
            "Hooks",
            "sirus/{id}",
        )
        .unwrap();
        assert!(
            !Path::new(&tree.path).join("hook-ran.txt").exists(),
            "untrusted repository hook executed"
        );
    }
    #[cfg(unix)]
    #[test]
    fn external_checkout_filters_are_refused_without_running_them() {
        use std::os::unix::fs::PermissionsExt;
        let repo = Repo::new();
        let filter = repo.0.join("unsafe-filter.sh");
        let sentinel = repo.0.join("filter-ran");
        fs::write(
            &filter,
            format!("#!/bin/sh\nprintf unsafe > '{}'\ncat\n", sentinel.display()),
        )
        .unwrap();
        fs::set_permissions(&filter, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            repo.cwd().join(".gitattributes"),
            "file.txt filter=unsafe\n",
        )
        .unwrap();
        run_ok(&repo.cwd(), &["add", ".gitattributes"]).unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "filter fixture"]).unwrap();
        run_ok(
            &repo.cwd(),
            &["config", "filter.unsafe.smudge", filter.to_str().unwrap()],
        )
        .unwrap();
        let result = crate::worktree::create_isolated(
            &repo.cwd(),
            &repo.0.join("trees"),
            "filter01",
            "Filters",
            "sirus/{id}",
        );
        assert!(result.is_err(), "external filter must be refused");
        assert!(!sentinel.exists(), "external filter ran");
        assert!(
            identity(&repo.cwd()).unwrap().is_repo,
            "safe identity lookup remains usable"
        );
    }
    #[test]
    fn preview_bounds_tracked_and_deleted_blobs_and_refuses_directories() {
        let repo = Repo::new();
        fs::write(
            repo.cwd().join("large.txt"),
            "x".repeat(2 * 1024 * 1024 + 1),
        )
        .unwrap();
        run_ok(&repo.cwd(), &["add", "large.txt"]).unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "large fixture"]).unwrap();
        assert!(diff_file(&repo.cwd(), "large.txt")
            .unwrap_err()
            .to_string()
            .contains("2 MiB"));
        fs::remove_file(repo.cwd().join("large.txt")).unwrap();
        assert!(diff_file(&repo.cwd(), "large.txt")
            .unwrap_err()
            .to_string()
            .contains("2 MiB"));
        fs::create_dir(repo.cwd().join("folder")).unwrap();
        fs::write(repo.cwd().join("folder/small.txt"), "small").unwrap();
        assert!(diff_file(&repo.cwd(), "folder").is_err());
        fs::write(
            repo.cwd().join("folder/large.txt"),
            "x".repeat(2 * 1024 * 1024 + 1),
        )
        .unwrap();
        run_ok(&repo.cwd(), &["add", "folder/large.txt"]).unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "nested large fixture"]).unwrap();
        fs::remove_file(repo.cwd().join("folder/large.txt")).unwrap();
        assert!(diff_file(&repo.cwd().join("folder"), "large.txt")
            .unwrap_err()
            .to_string()
            .contains("2 MiB"));
        let unborn = repo.0.join("unborn");
        fs::create_dir(&unborn).unwrap();
        run_ok(&unborn, &["init", "-q"]).unwrap();
        fs::write(unborn.join("large.txt"), "x".repeat(2 * 1024 * 1024 + 1)).unwrap();
        run_ok(&unborn, &["add", "large.txt"]).unwrap();
        fs::write(unborn.join("large.txt"), "small").unwrap();
        assert!(diff_file(&unborn, "large.txt")
            .unwrap_err()
            .to_string()
            .contains("2 MiB"));
    }

    #[cfg(unix)]
    #[test]
    fn diff_refuses_fifo_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let repo = Repo::new();
        let fifo = repo.cwd().join("pipe");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let start = std::time::Instant::now();
        assert!(diff_file(&repo.cwd(), "pipe").is_err());
        assert!(crate::paths::open_regular_within(&repo.cwd(), &fifo).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn diff_refuses_parent_traversal() {
        let repo = Repo::new();
        fs::write(repo.0.join("private.txt"), "private").unwrap();
        assert!(diff_file(&repo.cwd(), "../private.txt").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn diff_refuses_symlink_escape() {
        let repo = Repo::new();
        fs::write(repo.0.join("private.txt"), "private").unwrap();
        std::os::unix::fs::symlink(repo.0.join("private.txt"), repo.cwd().join("link.txt"))
            .unwrap();
        assert!(diff_file(&repo.cwd(), "link.txt").is_err());
    }
    #[test]
    fn diff_includes_staged_changes() {
        let repo = Repo::new();
        fs::write(repo.cwd().join("file.txt"), "after\n").unwrap();
        run_ok(&repo.cwd(), &["add", "--", "file.txt"]).unwrap();
        let diff = diff_file(&repo.cwd(), "file.txt").unwrap();
        assert!(
            diff.contains("-before") && diff.contains("+after"),
            "{diff}"
        );
    }
    #[test]
    fn diff_includes_staged_deletion_of_a_removed_directory() {
        let repo = Repo::new();
        fs::create_dir(repo.cwd().join("nested")).unwrap();
        fs::write(repo.cwd().join("nested/file.txt"), "removed text\n").unwrap();
        run_ok(&repo.cwd(), &["add", "--", "nested/file.txt"]).unwrap();
        run_ok(&repo.cwd(), &["commit", "-qm", "nested fixture"]).unwrap();
        run_ok(&repo.cwd(), &["rm", "-r", "--", "nested"]).unwrap();
        let diff = diff_file(&repo.cwd(), "nested/file.txt").unwrap();
        assert!(diff.contains("-removed text"));
    }
    #[test]
    fn status_reads_never_rewrite_the_index() {
        let command = git_command(Path::new("."));
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "GIT_OPTIONAL_LOCKS"
                && value == Some(std::ffi::OsStr::new("0"))));
        let repo = Repo::new();
        let index = repo.cwd().join(".git/index");
        // Same content, new mtime: porcelain status would refresh and rewrite the index.
        let file = fs::OpenOptions::new()
            .write(true)
            .open(repo.cwd().join("file.txt"))
            .unwrap();
        file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        drop(file);
        fs::write(repo.cwd().join("other.txt"), "changed\n").unwrap();
        let before = fs::read(&index).unwrap();
        let status = status(&repo.cwd()).unwrap();
        assert_eq!(fs::read(&index).unwrap(), before);
        assert!(!repo.cwd().join(".git/index.lock").exists());
        assert!(status.changes.iter().any(|c| c.path == "other.txt"));
        assert!(
            !status.changes.iter().any(|c| c.path == "file.txt"),
            "a stat-only change is not a change"
        );
    }
    #[test]
    fn status_counts_lines_through_diff_index() {
        let repo = Repo::new();
        fs::write(repo.cwd().join("file.txt"), "after\nmore\n").unwrap();
        let status = status(&repo.cwd()).unwrap();
        let change = status
            .changes
            .iter()
            .find(|c| c.path == "file.txt")
            .unwrap();
        assert_eq!((change.additions, change.deletions), (2, 1));
        assert!(!status.untracked_truncated);
    }
    #[test]
    fn status_caps_the_untracked_listing() {
        let repo = Repo::new();
        fs::create_dir(repo.cwd().join("build")).unwrap();
        for index in 0..(MAX_UNTRACKED + 25) {
            fs::write(repo.cwd().join(format!("build/{index}.o")), "").unwrap();
        }
        fs::write(repo.cwd().join("file.txt"), "after\n").unwrap();
        let status = status(&repo.cwd()).unwrap();
        let untracked = status
            .changes
            .iter()
            .filter(|c| c.kind == ChangeKind::Untracked)
            .count();
        assert_eq!(untracked, MAX_UNTRACKED);
        assert!(status.untracked_truncated);
        assert!(status.dirty);
        assert!(status.changes.iter().any(|c| c.path == "file.txt"));
    }
    #[test]
    fn status_preserves_spaces_quotes_unicode_and_newlines() {
        let repo = Repo::new();
        let names = [
            " leading.txt",
            "a \"quote\".txt",
            "ação.txt",
            "line\nbreak.txt",
            #[cfg(unix)]
            "literal\\backslash.txt",
        ];
        for name in names {
            fs::write(repo.cwd().join(name), "new\n").unwrap();
        }
        let status = status(&repo.cwd()).unwrap();
        for name in names {
            assert!(
                status.changes.iter().any(|c| c.path == name),
                "missing {name:?}"
            );
        }
    }
}
