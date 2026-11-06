//! The window header's Pull button (ADR-148). For the selected conversation's workspace (its
//! worktree or the project checkout) the renderer asks what its branch's upstream has that the
//! branch does not: the counts both ways and the newest incoming commits. Asking may first
//! fetch that one upstream branch in the background (low priority, 15 s bound), at most once
//! per 15 s per repository and upstream, backing off from 30 s to 5 min after failures. No
//! fetch on a detached HEAD, without an upstream or when the upstream is not on a remote.
//! Pulling is the person's own fast-forward-only `git pull` (30 s bound), refused while a
//! conversation works in that workspace; blocking local files and divergence come back as
//! answers the header can explain, not as raw Git text.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::Serialize;

use crate::commands::{native_task, session_path, AppState};
use crate::error::{Error, Result};
use crate::git;
use crate::models::AppData;
use crate::runtime::State;

/// The incoming commits listed at most.
const MAX_COMMITS: usize = 20;
const MAX_SUBJECT: usize = 200;
const MAX_AUTHOR: usize = 80;
const MAX_BLOCKING_FILES: usize = 50;
/// A successful fetch is reused this long (and nothing fetches a repository more often).
const FRESH: Duration = Duration::from_secs(15);
/// Backoff after a failed fetch: 30 s, doubling, at most 5 min.
const BACKOFF_FIRST: Duration = Duration::from_secs(30);
const BACKOFF_MAX: Duration = Duration::from_secs(300);
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingCommit {
    /// Abbreviated hash.
    pub sha: String,
    pub subject: String,
    pub author: String,
    /// Committer time, Unix seconds.
    pub time: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Incoming {
    pub branch: String,
    /// Short upstream name, e.g. `origin/main`.
    pub upstream: String,
    pub ahead: u32,
    pub behind: u32,
    /// Newest first, at most 20.
    pub commits: Vec<IncomingCommit>,
    /// Unix milliseconds of the last successful fetch of this upstream; None if none yet.
    pub checked_at: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PullOutcome {
    Pulled {
        incoming: Option<Incoming>,
    },
    /// Local changes Git would overwrite; nothing was changed.
    Blocked {
        files: Vec<String>,
    },
    /// Both sides have commits; no fast-forward is possible.
    Diverged {
        incoming: Option<Incoming>,
    },
}

/// The current branch's upstream as Git config names it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Upstream {
    branch: String,
    /// `refs/remotes/<remote>/<branch>`.
    tracking: String,
    /// `origin/main`.
    short: String,
    remote: String,
    /// The branch name on the remote.
    remote_branch: String,
}

/// One fetch target's history: when it was last tried and how that went.
#[derive(Debug, Clone, Copy, Default)]
struct Mark {
    tried: Option<Instant>,
    failures: u32,
    in_flight: bool,
    succeeded_at: Option<u64>,
}

static MARKS: Mutex<Option<HashMap<String, Mark>>> = Mutex::new(None);

/// How long after the last try the next fetch may run.
fn wait_after(failures: u32) -> Duration {
    if failures == 0 {
        return FRESH;
    }
    let factor = 1u32 << failures.saturating_sub(1).min(10);
    BACKOFF_FIRST.saturating_mul(factor).min(BACKOFF_MAX)
}

fn fetch_due(mark: &Mark, now: Instant) -> bool {
    !mark.in_flight
        && mark
            .tried
            .is_none_or(|tried| now.saturating_duration_since(tried) >= wait_after(mark.failures))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

fn text(cwd: &Path, args: &[&str]) -> Option<String> {
    git::bounded_run(cwd, args)
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `for-each-ref` fields of the branch's upstream, NUL separated.
fn parse_upstream(branch: &str, raw: &str) -> Option<Upstream> {
    let mut fields = raw.trim_end_matches(['\n', '\r']).split('\0');
    let tracking = fields.next()?.to_string();
    let short = fields.next()?.to_string();
    let remote = fields.next()?.to_string();
    let remote_branch = fields.next()?.strip_prefix("refs/heads/")?.to_string();
    // A local upstream (remote ".") or an odd name has nothing to fetch.
    if !git::valid_remote_name(&remote)
        || !git::valid_branch_chars(&remote_branch)
        || !tracking
            .strip_prefix("refs/remotes/")
            .is_some_and(git::valid_branch_chars)
        || short.is_empty()
    {
        return None;
    }
    Some(Upstream {
        branch: branch.to_string(),
        tracking,
        short,
        remote,
        remote_branch,
    })
}

fn upstream(cwd: &Path) -> Option<Upstream> {
    let branch = text(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if !git::valid_branch_chars(&branch) {
        return None;
    }
    let raw = text(
        cwd,
        &[
            "for-each-ref",
            "--count=1",
            "--format=%(upstream)%00%(upstream:short)%00%(upstream:remotename)%00%(upstream:remoteref)",
            &format!("refs/heads/{branch}"),
        ],
    )?;
    parse_upstream(&branch, &raw)
}

/// `git log --format=%h%x1f%s%x1f%an%x1f%ct%x1e` → commits, bounded.
fn parse_commits(raw: &str) -> Vec<IncomingCommit> {
    raw.split('\u{1e}')
        .filter_map(|record| {
            let record = record.trim_start_matches(['\n', '\r']);
            let mut fields = record.split('\u{1f}');
            let sha = fields.next()?.trim();
            if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            let subject = fields.next()?.trim().chars().take(MAX_SUBJECT).collect();
            let author = fields.next()?.trim().chars().take(MAX_AUTHOR).collect();
            let time = fields.next()?.trim().parse().ok()?;
            Some(IncomingCommit {
                sha: sha.to_string(),
                subject,
                author,
                time,
            })
        })
        .take(MAX_COMMITS)
        .collect()
}

/// `rev-list --left-right --count HEAD...upstream` → (ahead, behind).
fn parse_counts(raw: &str) -> Option<(u32, u32)> {
    let mut parts = raw.split_whitespace();
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// What a failed `git pull --ff-only` said, in the terms the header shows.
#[derive(Debug, PartialEq, Eq)]
enum PullFailure {
    Blocked(Vec<String>),
    Diverged,
    Other(String),
}

fn classify_failure(stderr: &str) -> PullFailure {
    let mut files = Vec::new();
    let mut listing = false;
    for line in stderr.lines() {
        if line.contains("would be overwritten by") {
            listing = true;
            continue;
        }
        if listing {
            if let Some(file) = line.strip_prefix('\t') {
                let file = file.trim();
                if !file.is_empty()
                    && files.len() < MAX_BLOCKING_FILES
                    && !files.iter().any(|f| f == file)
                {
                    files.push(file.to_string());
                }
                continue;
            }
            listing = false;
        }
    }
    if !files.is_empty() {
        return PullFailure::Blocked(files);
    }
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("not possible to fast-forward") || lower.contains("diverging branches") {
        return PullFailure::Diverged;
    }
    let message: String = stderr.trim().chars().take(600).collect();
    PullFailure::Other(if message.is_empty() {
        "git pull failed".to_string()
    } else {
        message
    })
}

/// The workspace's incoming state; fetches first when `fetch` and the cache allow it.
pub(crate) fn incoming(cwd: &Path, fetch: bool) -> Option<Incoming> {
    let upstream = upstream(cwd)?;
    let common = text(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let key = format!("{common}\0{}", upstream.tracking);
    let start = {
        let mut marks = MARKS.lock();
        let mark = marks
            .get_or_insert_with(HashMap::new)
            .entry(key.clone())
            .or_default();
        let now = Instant::now();
        let due = fetch && fetch_due(mark, now);
        if due {
            mark.in_flight = true;
            mark.tried = Some(now);
        }
        due
    };
    if start {
        let fetched = git::fetch_branch(
            cwd,
            &upstream.remote,
            &upstream.remote_branch,
            &upstream.tracking,
            FETCH_TIMEOUT,
        );
        if let Err(error) = &fetched {
            tracing::debug!(%error, upstream = %upstream.short, "incoming fetch failed");
        }
        let mut marks = MARKS.lock();
        let mark = marks
            .get_or_insert_with(HashMap::new)
            .entry(key.clone())
            .or_default();
        mark.in_flight = false;
        if fetched.is_ok() {
            mark.failures = 0;
            mark.succeeded_at = Some(now_millis());
        } else {
            mark.failures = mark.failures.saturating_add(1);
        }
    }
    let checked_at = MARKS
        .lock()
        .as_ref()
        .and_then(|marks| marks.get(&key))
        .and_then(|mark| mark.succeeded_at);
    let (ahead, behind) = parse_counts(&text(
        cwd,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{}", upstream.tracking),
        ],
    )?)?;
    let commits = if behind > 0 {
        text(
            cwd,
            &[
                "log",
                "--no-show-signature",
                "-n",
                "20",
                "--format=%h%x1f%s%x1f%an%x1f%ct%x1e",
                &format!("HEAD..{}", upstream.tracking),
            ],
        )
        .map(|raw| parse_commits(&raw))
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    Some(Incoming {
        branch: upstream.branch,
        upstream: upstream.short,
        ahead,
        behind,
        commits,
        checked_at,
    })
}

/// The person's fast-forward-only pull of the workspace branch from its upstream.
pub(crate) fn pull(cwd: &Path) -> Result<PullOutcome> {
    let upstream = upstream(cwd)
        .ok_or_else(|| Error::git("This branch has no upstream on a remote to pull from."))?;
    let output = git::pull_fast_forward(cwd, &upstream.remote, &upstream.remote_branch)?;
    if output.status.success() {
        return Ok(PullOutcome::Pulled {
            incoming: incoming(cwd, false),
        });
    }
    match classify_failure(&String::from_utf8_lossy(&output.stderr)) {
        PullFailure::Blocked(files) => Ok(PullOutcome::Blocked { files }),
        PullFailure::Diverged => Ok(PullOutcome::Diverged {
            incoming: incoming(cwd, false),
        }),
        PullFailure::Other(message) => Err(Error::git(message)),
    }
}

/// Whether a conversation works in this session's workspace: itself, or, for the project
/// checkout, any conversation of that project outside its own worktree (true). Otherwise the
/// folders of the other working conversations, for the caller to compare outside the lock.
fn busy_or_folders(data: &AppData, session_id: &str) -> (bool, Vec<String>) {
    let Some(own) = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)
    else {
        return (false, Vec::new());
    };
    let mut folders = Vec::new();
    for session in data
        .sessions
        .iter()
        .filter(|session| session.status.is_active())
    {
        if session.id == own.id
            || (!own.worktree.isolated
                && !session.worktree.isolated
                && session.project_id == own.project_id)
        {
            return (true, Vec::new());
        }
        folders.push(session.worktree.path.clone());
    }
    (false, folders)
}

/// What the selected conversation's workspace branch can pull; null when there is nothing to
/// compare with (no repository, detached HEAD, no upstream on a remote).
pub async fn git_incoming(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Option<Incoming>> {
    let state = state.inner().clone();
    // Asked on focus and on a timer: the app's own background Git (ADR-143).
    native_task(move || {
        crate::process_priority::background(|| {
            let cwd = session_path(&state, &session_id)?;
            Ok(incoming(&cwd, true))
        })
    })
    .await
}

/// The header's Pull: `git pull --ff-only` from the branch's upstream.
pub async fn git_pull_ff(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<PullOutcome> {
    let state = state.inner().clone();
    native_task(move || {
        state.ensure_running()?;
        let cwd = session_path(&state, &session_id)?;
        let (busy, folders) = busy_or_folders(&state.data.lock(), &session_id);
        if busy
            || folders
                .iter()
                .any(|folder| std::fs::canonicalize(folder).is_ok_and(|folder| folder == cwd))
        {
            return Err(Error::git(
                "A conversation is working in this folder. Pull when it finishes.",
            ));
        }
        pull(&cwd)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::tests::Repo;

    #[test]
    fn fetches_wait_fifteen_seconds_and_back_off_after_failures() {
        assert_eq!(wait_after(0), Duration::from_secs(15));
        assert_eq!(wait_after(1), Duration::from_secs(30));
        assert_eq!(wait_after(2), Duration::from_secs(60));
        assert_eq!(wait_after(4), Duration::from_secs(240));
        assert_eq!(wait_after(5), Duration::from_secs(300));
        assert_eq!(wait_after(40), Duration::from_secs(300));
        let now = Instant::now();
        assert!(fetch_due(&Mark::default(), now));
        let recent = Mark {
            tried: Some(now),
            ..Mark::default()
        };
        assert!(!fetch_due(&recent, now + Duration::from_secs(14)));
        assert!(fetch_due(&recent, now + Duration::from_secs(15)));
        let failing = Mark {
            tried: Some(now),
            failures: 2,
            ..Mark::default()
        };
        assert!(!fetch_due(&failing, now + Duration::from_secs(59)));
        assert!(fetch_due(&failing, now + Duration::from_secs(60)));
        let running = Mark {
            in_flight: true,
            ..Mark::default()
        };
        assert!(!fetch_due(&running, now));
    }

    #[test]
    fn upstreams_commits_and_failures_parse() {
        let up = parse_upstream(
            "main",
            "refs/remotes/origin/main\0origin/main\0origin\0refs/heads/main\n",
        )
        .unwrap();
        assert_eq!(up.short, "origin/main");
        assert_eq!(up.remote_branch, "main");
        assert!(parse_upstream("main", "refs/heads/base\0base\0.\0refs/heads/base").is_none());
        assert!(parse_upstream("main", "\0\0\0").is_none());
        assert!(
            parse_upstream("main", "refs/remotes/-x/main\0-x/main\0-x\0refs/heads/main").is_none()
        );

        let raw = "abc1234\u{1f}Fix the thing\u{1f}Ana\u{1f}1700000000\u{1e}\nzzz\u{1f}x\u{1f}y\u{1f}1\u{1e}\ndef5678\u{1f}Sub\u{1f}Bo\u{1f}nope\u{1e}\n";
        let commits = parse_commits(raw);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].sha, "abc1234");
        assert_eq!(commits[0].author, "Ana");
        assert_eq!(commits[0].time, 1_700_000_000);
        assert_eq!(parse_counts("1\t3\n"), Some((1, 3)));

        let blocked = "error: Your local changes to the following files would be overwritten by merge:\n\tfile.txt\n\tsrc/a b.rs\nPlease commit your changes or stash them before you merge.\nerror: The following untracked working tree files would be overwritten by merge:\n\tnew.txt\nAborting\n";
        assert_eq!(
            classify_failure(blocked),
            PullFailure::Blocked(vec![
                "file.txt".into(),
                "src/a b.rs".into(),
                "new.txt".into()
            ])
        );
        assert_eq!(
            classify_failure("hint: Diverging branches can't be fast-forwarded\nfatal: Not possible to fast-forward, aborting.\n"),
            PullFailure::Diverged
        );
        assert_eq!(
            classify_failure(""),
            PullFailure::Other("git pull failed".into())
        );
    }

    fn upstream_commit(origin: &Repo, name: &str, content: &str) {
        std::fs::write(origin.cwd().join(name), content).unwrap();
        git::run_ok(&origin.cwd(), &["add", "--", name]).unwrap();
        git::run_ok(
            &origin.cwd(),
            &["commit", "-qm", &format!("upstream {name}")],
        )
        .unwrap();
    }

    #[test]
    fn a_branch_behind_its_upstream_lists_the_commits_and_fast_forwards() {
        let origin = Repo::new();
        let clone = crate::worktree::tests::clone_of(&origin);
        let current = incoming(&clone, true).unwrap();
        assert_eq!((current.ahead, current.behind), (0, 0));
        assert!(current.upstream.starts_with("origin/"));
        assert!(current.checked_at.is_some());

        upstream_commit(&origin, "file.txt", "after\n");
        upstream_commit(&origin, "two.txt", "two\n");
        // Fetched 15 s ago at most: the cached remote ref still says nothing is new.
        assert_eq!(incoming(&clone, true).unwrap().behind, 0);
        git::fetch_origin_branch(&clone, &current.branch).unwrap();
        let behind = incoming(&clone, false).unwrap();
        assert_eq!((behind.ahead, behind.behind), (0, 2));
        assert_eq!(behind.commits.len(), 2);
        assert_eq!(behind.commits[0].subject, "upstream two.txt");

        let PullOutcome::Pulled { incoming: after } = pull(&clone).unwrap() else {
            panic!("expected a fast-forward");
        };
        assert_eq!(after.unwrap().behind, 0);
        assert_eq!(
            git::run_ok(&clone, &["rev-parse", "HEAD"]).unwrap(),
            git::run_ok(&origin.cwd(), &["rev-parse", "HEAD"]).unwrap()
        );
        // Detached: nothing to compare.
        git::run_ok(&clone, &["checkout", "-q", "--detach"]).unwrap();
        assert!(incoming(&clone, true).is_none());
    }

    #[test]
    fn local_changes_in_the_way_block_and_divergence_is_refused() {
        let origin = Repo::new();
        let clone = crate::worktree::tests::clone_of(&origin);
        upstream_commit(&origin, "file.txt", "after\n");
        std::fs::write(clone.join("file.txt"), "mine\n").unwrap();
        let head = git::run_ok(&clone, &["rev-parse", "HEAD"]).unwrap();
        assert!(
            matches!(pull(&clone).unwrap(), PullOutcome::Blocked { files } if files == ["file.txt"])
        );
        assert_eq!(git::run_ok(&clone, &["rev-parse", "HEAD"]).unwrap(), head);
        assert_eq!(
            std::fs::read_to_string(clone.join("file.txt")).unwrap(),
            "mine\n"
        );

        git::run_ok(&clone, &["commit", "-qam", "local work"]).unwrap();
        let head = git::run_ok(&clone, &["rev-parse", "HEAD"]).unwrap();
        let PullOutcome::Diverged { incoming } = pull(&clone).unwrap() else {
            panic!("expected divergence");
        };
        let incoming = incoming.unwrap();
        assert_eq!((incoming.ahead, incoming.behind), (1, 1));
        assert_eq!(git::run_ok(&clone, &["rev-parse", "HEAD"]).unwrap(), head);
    }
}
