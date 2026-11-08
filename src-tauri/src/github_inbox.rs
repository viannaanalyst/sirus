//! Pull request and issue inbox across owned projects (ADR-050).
//!
//! Repositories come only from each saved project's `origin` remote. Every call
//! uses the person's existing `gh` login through the hardened `gh` command from
//! `pull_requests.rs`; queries and endpoints are fixed, and renderer input is
//! limited to validated repository names, numbers and closed enums. Changes on
//! GitHub (merge, draft/ready, close/reopen, comment) need `confirm: true` and a
//! repository the person owns as a project.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::runtime::State;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::{native_task, AppState};
use crate::error::{Error, Result};
use crate::pull_requests::{github_link, hardened, repository, sha, text, LookupStatus};

const MAX_REPOSITORIES: usize = 24;
const PAGE: u32 = 50;
const TITLE_LIMIT: usize = 300;
const BODY_LIMIT: usize = 65_536;
const COMMENT_LIMIT: usize = 16_384;
const DIFF_LIMIT: usize = 2 * 1024 * 1024;
const COMMENT_BODY_LIMIT: usize = 65_536;
/// Failing checks described for an agent: at most this many, each with a bounded log excerpt.
const MAX_FAILURES: usize = 10;
const LOG_EXCERPT_LIMIT: usize = 4_000;
/// All log excerpts together stay well under the 64 KiB composer draft bound.
const LOG_BUDGET: usize = 32_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    PullRequest,
    Issue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemState {
    Open,
    /// Closed without merging (for issues: every closed issue).
    Closed,
    /// Merged pull requests.
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    fn api(self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    /// The GitHub repositories of saved projects (no `gh` call): chat links to their pull
    /// requests and issues open on the Pull requests page (ADR-149).
    Repositories,
    /// Open or closed pull requests/issues of every owned GitHub repository.
    List { kind: ItemKind, state: ItemState },
    Detail {
        repository: String,
        number: u32,
        kind: ItemKind,
    },
    /// Unified diff of one pull request (bounded).
    Diff { repository: String, number: u32 },
    /// Every failing check of the pull request's latest commit, with its
    /// annotations and a bounded excerpt of the GitHub Actions job log.
    Failures { repository: String, number: u32 },
    Merge {
        repository: String,
        number: u32,
        method: MergeMethod,
        /// GitHub refuses the merge if the head moved after the person looked.
        expected_head: String,
        confirm: bool,
    },
    SetDraft {
        repository: String,
        number: u32,
        draft: bool,
        confirm: bool,
    },
    SetOpen {
        repository: String,
        number: u32,
        kind: ItemKind,
        open: bool,
        confirm: bool,
    },
    Comment {
        repository: String,
        number: u32,
        body: String,
        confirm: bool,
    },
    /// Inline review conversations of a pull request (bounded).
    ReviewThreads { repository: String, number: u32 },
    /// Replies to one review thread; the thread must belong to this pull request.
    ReplyThread {
        repository: String,
        number: u32,
        thread_id: String,
        body: String,
        confirm: bool,
    },
    /// Resolves or unresolves one review thread of this pull request.
    ResolveThread {
        repository: String,
        number: u32,
        thread_id: String,
        resolved: bool,
        confirm: bool,
    },
    /// One GitHub image attachment of this item's text, read through `gh` (so a
    /// private repository's upload works without the token reaching the
    /// renderer) and returned as a bounded `data:image/…` URL. `asset` is the
    /// attachment's ID (`<uuid>` or `<number>/<uuid>`), never a URL.
    Attachment {
        repository: String,
        number: u32,
        asset: String,
    },
    /// Pushes the session branch, then opens a pull request from it into the
    /// repository's default branch.
    Create {
        session_id: String,
        title: String,
        body: String,
        draft: bool,
        confirm: bool,
    },
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Response {
    Repositories {
        repositories: Vec<Repository>,
    },
    Inbox(Inbox),
    Detail(Box<Detail>),
    Diff {
        text: String,
        truncated: bool,
    },
    Failures {
        checks: Vec<FailedCheck>,
        /// More checks failed than were described.
        truncated: bool,
    },
    Created {
        url: String,
    },
    Threads {
        threads: Vec<ReviewThread>,
        /// More threads exist than were read.
        truncated: bool,
    },
    Attachment {
        data_url: String,
    },
    Done,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewThread {
    pub id: String,
    pub path: String,
    /// The line the thread is on in the current diff; `None` once outdated.
    pub line: Option<u32>,
    pub start_line: Option<u32>,
    /// The line it was written on, kept for outdated threads.
    pub original_line: Option<u32>,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub viewer_can_reply: bool,
    pub viewer_can_resolve: bool,
    pub viewer_can_unresolve: bool,
    /// The end of the diff hunk the first comment was written on.
    pub diff_hunk: Option<String>,
    pub comments: Vec<Comment>,
    /// Every comment of the thread, including those not read.
    pub comment_count: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedCheck {
    pub name: String,
    pub url: Option<String>,
    /// The check's own title/summary/description, when it reports one.
    pub summary: Option<String>,
    /// `path:line message` lines from the check's annotations.
    pub annotations: Vec<String>,
    /// Error-focused excerpt of the GitHub Actions job log; `None` when the
    /// check is not an Actions job or the log could not be read.
    pub log: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inbox {
    pub viewer: Option<String>,
    pub repositories: Vec<Repository>,
    pub items: Vec<Item>,
    pub failures: Vec<Failure>,
    pub checked_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub repository: String,
    pub project_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub repository: String,
    pub status: LookupStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub kind: ItemKind,
    pub repository: String,
    pub number: u32,
    pub title: String,
    pub url: String,
    /// `open`, `closed` or `merged`.
    pub state: String,
    pub is_draft: bool,
    pub author: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
    pub additions: u32,
    pub deletions: u32,
    pub review_decision: Option<String>,
    pub mergeable: Option<String>,
    pub labels: Vec<Label>,
    pub comment_count: u32,
    pub assignees: Vec<String>,
    pub review_requests: Vec<String>,
    /// Combined check state: SUCCESS, FAILURE, PENDING, ERROR, EXPECTED.
    pub checks: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    /// passed, failed, pending, skipped or unknown.
    pub status: String,
    pub url: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub author: Option<String>,
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub author: Option<String>,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub oid: String,
    pub headline: String,
    pub author: Option<String>,
    pub committed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub additions: u32,
    pub deletions: u32,
    pub change_type: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub item: Item,
    pub body: String,
    pub head_oid: Option<String>,
    pub merged_at: Option<String>,
    pub closed_at: Option<String>,
    pub merge_state_status: Option<String>,
    pub changed_files: u32,
    pub viewer_can_update: bool,
    pub merge_methods: Vec<MergeMethod>,
    pub checks: Vec<Check>,
    pub reviewers: Vec<String>,
    pub reviews: Vec<Review>,
    pub comments: Vec<Comment>,
    pub commits: Vec<Commit>,
    pub files: Vec<ChangedFile>,
    pub files_truncated: bool,
}

/// Why a pull request is worth an alert (ADR-050 amendment, 2026-10-18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AlertReason {
    Opened,
    ReviewRequested,
}

/// A pull request alert built only from what native itself listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullAlert {
    /// `owner/repo#n`, as listed.
    pub key: String,
    pub repository: String,
    pub number: u32,
    pub author: String,
    pub title: String,
    pub reason: AlertReason,
}

/// The viewer and the open pull requests of the last open pull request list. A pull request
/// alert can only name an item in it, so the renderer cannot make one up.
static OPEN_PULLS: parking_lot::Mutex<Option<(Option<String>, Vec<Item>)>> =
    parking_lot::Mutex::new(None);

fn remember_open_pulls(inbox: &Inbox) {
    let items = inbox
        .items
        .iter()
        .filter(|item| item.kind == ItemKind::PullRequest && item.state == "open")
        .cloned()
        .collect();
    *OPEN_PULLS.lock() = Some((inbox.viewer.clone(), items));
}

/// The alerts the last open pull request list confirms, in the asked order, one per item
/// (a review request wins over the opening). Only open pull requests someone other than the
/// viewer authored qualify, and a review request must name the viewer.
pub fn pull_alerts(requests: &[(String, u32, AlertReason)]) -> Vec<PullAlert> {
    let guard = OPEN_PULLS.lock();
    let Some((viewer, items)) = guard.as_ref() else {
        return vec![];
    };
    confirmed_alerts(viewer.as_deref(), items, requests)
}

fn confirmed_alerts(
    viewer: Option<&str>,
    items: &[Item],
    requests: &[(String, u32, AlertReason)],
) -> Vec<PullAlert> {
    let Some(viewer) = viewer.filter(|viewer| !viewer.is_empty()) else {
        return vec![];
    };
    let mut alerts: Vec<PullAlert> = Vec::new();
    for (repository, number, reason) in requests {
        let Some(item) = items.iter().find(|item| {
            item.kind == ItemKind::PullRequest
                && item.state == "open"
                && item.number == *number
                && item.repository.eq_ignore_ascii_case(repository)
        }) else {
            continue;
        };
        let Some(author) = item
            .author
            .as_deref()
            .filter(|author| !author.is_empty() && !author.eq_ignore_ascii_case(viewer))
        else {
            continue;
        };
        let requested = item
            .review_requests
            .iter()
            .any(|login| login.eq_ignore_ascii_case(viewer));
        let reason = match reason {
            AlertReason::ReviewRequested if !requested => continue,
            AlertReason::Opened if requested => AlertReason::ReviewRequested,
            reason => *reason,
        };
        let key = format!("{}#{}", item.repository, item.number);
        if let Some(known) = alerts.iter_mut().find(|alert| alert.key == key) {
            if reason == AlertReason::ReviewRequested {
                known.reason = reason;
            }
            continue;
        }
        alerts.push(PullAlert {
            key,
            repository: item.repository.clone(),
            number: item.number,
            author: author.to_owned(),
            title: item.title.clone(),
            reason,
        });
    }
    alerts
}

pub async fn pull_request_action(
    state: State<'_, Arc<AppState>>,
    action: Action,
) -> Result<Response> {
    let state = state.inner().clone();
    let snapshot = state.clone();
    let repositories = native_task(move || owned_repositories(&snapshot)).await?;
    let binary = OsStr::new("gh");
    match action {
        Action::Repositories => Ok(Response::Repositories { repositories }),
        Action::List { kind, state } => {
            let inbox = list(binary, repositories, kind, state).await;
            if kind == ItemKind::PullRequest && state == ItemState::Open {
                remember_open_pulls(&inbox);
            }
            Ok(Response::Inbox(inbox))
        }
        Action::Detail {
            repository,
            number,
            kind,
        } => {
            let repo = owned(&repositories, &repository, number)?;
            Ok(Response::Detail(Box::new(
                detail(binary, repo, number, kind).await?,
            )))
        }
        Action::Diff { repository, number } => {
            let repo = owned(&repositories, &repository, number)?;
            let (text, truncated) = diff(binary, repo, number).await?;
            Ok(Response::Diff { text, truncated })
        }
        Action::Failures { repository, number } => {
            let repo = owned(&repositories, &repository, number)?;
            let (checks, truncated) = failures(binary, repo, number).await?;
            Ok(Response::Failures { checks, truncated })
        }
        Action::Merge {
            repository,
            number,
            method,
            expected_head,
            confirm,
        } => {
            let repo = owned(&repositories, &repository, number)?;
            confirmed(confirm)?;
            if !sha(&expected_head) {
                return Err(Error::new("invalid", "Invalid pull request head."));
            }
            let endpoint = format!("/repos/{repo}/pulls/{number}/merge");
            rest(
                binary,
                "PUT",
                &endpoint,
                &[
                    ("merge_method", method.api()),
                    ("sha", &expected_head.to_ascii_lowercase()),
                ],
            )
            .await?;
            Ok(Response::Done)
        }
        Action::SetDraft {
            repository,
            number,
            draft,
            confirm,
        } => {
            let repo = owned(&repositories, &repository, number)?;
            confirmed(confirm)?;
            set_draft(binary, repo, number, draft).await?;
            Ok(Response::Done)
        }
        Action::SetOpen {
            repository,
            number,
            kind,
            open,
            confirm,
        } => {
            let repo = owned(&repositories, &repository, number)?;
            confirmed(confirm)?;
            let path = if kind == ItemKind::PullRequest {
                "pulls"
            } else {
                "issues"
            };
            let endpoint = format!("/repos/{repo}/{path}/{number}");
            let state = if open { "open" } else { "closed" };
            rest(binary, "PATCH", &endpoint, &[("state", state)]).await?;
            Ok(Response::Done)
        }
        Action::Comment {
            repository,
            number,
            body,
            confirm,
        } => {
            let repo = owned(&repositories, &repository, number)?;
            confirmed(confirm)?;
            let body = body.trim();
            if body.is_empty() || body.len() > COMMENT_BODY_LIMIT || body.contains('\0') {
                return Err(Error::new(
                    "invalid",
                    "Comments must have 1 to 65,536 bytes of text.",
                ));
            }
            let endpoint = format!("/repos/{repo}/issues/{number}/comments");
            rest(binary, "POST", &endpoint, &[("body", body)]).await?;
            Ok(Response::Done)
        }
        Action::ReviewThreads { repository, number } => {
            let repo = owned(&repositories, &repository, number)?;
            let (threads, truncated) = review_threads(binary, repo, number).await?;
            Ok(Response::Threads { threads, truncated })
        }
        Action::ReplyThread {
            repository,
            number,
            thread_id,
            body,
            confirm,
        } => {
            let repo = owned(&repositories, &repository, number)?;
            confirmed(confirm)?;
            let body = body.trim();
            if body.is_empty() || body.len() > COMMENT_BODY_LIMIT || body.contains('\0') {
                return Err(Error::new(
                    "invalid",
                    "Replies must have 1 to 65,536 bytes of text.",
                ));
            }
            let thread = owned_thread(binary, repo, number, &thread_id).await?;
            thread_mutation(binary, REPLY_MUTATION, &[("id", thread), ("body", body)]).await?;
            Ok(Response::Done)
        }
        Action::ResolveThread {
            repository,
            number,
            thread_id,
            resolved,
            confirm,
        } => {
            let repo = owned(&repositories, &repository, number)?;
            confirmed(confirm)?;
            let thread = owned_thread(binary, repo, number, &thread_id).await?;
            let mutation = if resolved {
                RESOLVE_MUTATION
            } else {
                UNRESOLVE_MUTATION
            };
            thread_mutation(binary, mutation, &[("id", thread)]).await?;
            Ok(Response::Done)
        }
        Action::Attachment {
            repository,
            number,
            asset,
        } => {
            let repo = owned(&repositories, &repository, number)?;
            let url = attachment_url(repo, &asset)
                .ok_or_else(|| Error::new("invalid", "Not a GitHub image attachment."))?;
            Ok(Response::Attachment {
                data_url: attachment(binary, &url).await?,
            })
        }
        Action::Create {
            session_id,
            title,
            body,
            draft,
            confirm,
        } => {
            confirmed(confirm)?;
            let title = title.trim().to_string();
            let body = body.trim().to_string();
            if title.is_empty() || title.chars().count() > 256 || title.contains('\0') {
                return Err(Error::new(
                    "invalid",
                    "The title must have 1 to 256 characters.",
                ));
            }
            if body.len() > COMMENT_BODY_LIMIT || body.contains('\0') {
                return Err(Error::new("invalid", "The description is too long."));
            }
            let probe = state.clone();
            let id = session_id.clone();
            let (cwd, context) = native_task(move || {
                let cwd = crate::commands::session_path(&probe, &id)?;
                let context = crate::pull_requests::inspect(cwd.clone())?;
                Ok((cwd, context))
            })
            .await?;
            // GitLab and Bitbucket: the same push, then a merge request or pull request there.
            if let Some(remote) = context.forge.clone() {
                let head = context
                    .branch
                    .clone()
                    .ok_or_else(|| Error::git("cannot open a pull request from a detached HEAD"))?;
                let push_cwd = cwd.clone();
                native_task(move || crate::git::push(&push_cwd).map(|_| ())).await?;
                let url = crate::forge::create(&remote, &head, &title, &body, draft).await?;
                return Ok(Response::Created { url });
            }
            let repo = context
                .repository
                .clone()
                .filter(|repo| repositories.iter().any(|item| &item.repository == repo))
                .ok_or_else(|| {
                    Error::new(
                        "invalid",
                        "This session has no GitHub origin of a saved project.",
                    )
                })?;
            let head = context
                .branch
                .clone()
                .ok_or_else(|| Error::git("cannot open a pull request from a detached HEAD"))?;
            let push_cwd = cwd.clone();
            native_task(move || crate::git::push(&push_cwd).map(|_| ())).await?;
            let base = rest_capture(
                binary,
                "GET",
                &format!("/repos/{repo}"),
                &[],
                ".default_branch",
            )
            .await?;
            if base.is_empty() || base == head {
                return Err(Error::new(
                    "invalid",
                    "This branch is the repository's default branch.",
                ));
            }
            let draft = if draft { "true" } else { "false" };
            let url = rest_capture(
                binary,
                "POST",
                &format!("/repos/{repo}/pulls"),
                &[
                    ("title", &title),
                    ("head", &head),
                    ("base", &base),
                    ("body", &body),
                    ("draft", draft),
                ],
                ".html_url",
            )
            .await?;
            if !url.starts_with("https://github.com/") {
                return Err(Error::new(
                    "github_unavailable",
                    "GitHub did not return the new pull request.",
                ));
            }
            Ok(Response::Created { url })
        }
    }
}

/// A fixed REST call whose one `--jq` field is returned (bounded, trimmed).
/// `draft` is sent as a typed boolean; every other field stays a string.
async fn rest_capture(
    binary: &OsStr,
    method: &str,
    endpoint: &str,
    fields: &[(&str, &str)],
    jq: &str,
) -> Result<String> {
    let mut command = hardened(binary);
    command.args([
        "api",
        "--hostname",
        "github.com",
        "--method",
        method,
        "-H",
        "Accept: application/vnd.github+json",
        endpoint,
        "--jq",
        jq,
    ]);
    for (key, value) in fields {
        let flag = if *key == "draft" { "-F" } else { "-f" };
        command.arg(flag).arg(format!("{key}={value}"));
    }
    let output = if method == "GET" {
        crate::gh_gate::read(command, Duration::from_secs(30)).await
    } else {
        let output = crate::cli_output::capture_command(command, Duration::from_secs(30)).await;
        crate::gh_gate::wrote(&output);
        output
    }
    .map_err(|error| {
        lookup_error(if error.kind() == std::io::ErrorKind::NotFound {
            LookupStatus::CliMissing
        } else {
            LookupStatus::Unavailable
        })
    })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout)
            .trim()
            .chars()
            .take(400)
            .collect());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(if stderr.contains("A pull request already exists") {
        Error::new(
            "github_conflict",
            "A pull request already exists for this branch.",
        )
    } else if stderr.contains("HTTP 403") || stderr.contains("HTTP 404") {
        Error::new(
            "github_forbidden",
            "Your GitHub account cannot change this repository.",
        )
    } else if stderr.contains("HTTP 422") {
        Error::new(
            "github_invalid",
            "GitHub refused this pull request (is the branch pushed and different from the base?).",
        )
    } else {
        lookup_error(status_of(&output.stderr))
    })
}

fn confirmed(confirm: bool) -> Result<()> {
    if confirm {
        Ok(())
    } else {
        Err(Error::new(
            "confirmation_required",
            "Confirm this GitHub change first.",
        ))
    }
}

/// The repository must belong to a saved project; the number must be plausible.
pub(crate) fn owned<'a>(
    repositories: &'a [Repository],
    requested: &str,
    number: u32,
) -> Result<&'a str> {
    if number == 0 || number > 10_000_000 {
        return Err(Error::new(
            "invalid",
            "Invalid pull request or issue number.",
        ));
    }
    repositories
        .iter()
        .find(|repo| repo.repository.eq_ignore_ascii_case(requested))
        .map(|repo| repo.repository.as_str())
        .ok_or_else(|| Error::not_found("This repository is not one of your projects."))
}

/// GitHub repositories of saved projects, from each project's `origin` remote.
pub(crate) fn owned_repositories(state: &AppState) -> Result<Vec<Repository>> {
    let projects: Vec<(String, PathBuf)> = state
        .data
        .lock()
        .projects
        .iter()
        .map(|project| (project.id.clone(), PathBuf::from(&project.path)))
        .collect();
    let mut found: BTreeMap<String, Repository> = BTreeMap::new();
    for (id, path) in projects {
        let Ok(output) = crate::git::bounded_probe(&path, &["remote", "get-url", "origin"]) else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let Some(repo) =
            crate::git::normalize_github_web_url(String::from_utf8_lossy(&output.stdout).trim())
                .and_then(|url| repository(&url))
        else {
            continue;
        };
        let entry = found
            .entry(repo.to_ascii_lowercase())
            .or_insert_with(|| Repository {
                repository: repo.clone(),
                project_ids: vec![],
            });
        entry.project_ids.push(id);
        if found.len() >= MAX_REPOSITORIES {
            break;
        }
    }
    Ok(found.into_values().collect())
}

fn status_of(stderr: &[u8]) -> LookupStatus {
    let stderr = String::from_utf8_lossy(stderr);
    if stderr.contains("gh auth login") || stderr.contains("HTTP 401") {
        LookupStatus::AuthRequired
    } else {
        LookupStatus::Unavailable
    }
}

/// One fixed GraphQL document with typed variables. String values go through
/// `-f` (raw, never `@file`); integers through `-F`.
pub(crate) async fn graphql(
    binary: &OsStr,
    query: &str,
    strings: &[(&str, &str)],
    ints: &[(&str, u32)],
) -> std::result::Result<Value, LookupStatus> {
    let mut command = hardened(binary);
    command.args(["api", "graphql", "--hostname", "github.com", "-f"]);
    command.arg(format!("query={query}"));
    for (key, value) in strings {
        command.arg("-f").arg(format!("{key}={value}"));
    }
    for (key, value) in ints {
        command.arg("-F").arg(format!("{key}={value}"));
    }
    // Queries share the GitHub read gate (ADR-100); mutations run directly.
    let output = if query.trim_start().starts_with("mutation") {
        let output = crate::cli_output::capture_command(command, Duration::from_secs(20)).await;
        crate::gh_gate::wrote(&output);
        output
    } else {
        crate::gh_gate::read(command, Duration::from_secs(20)).await
    }
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            LookupStatus::CliMissing
        } else {
            LookupStatus::Unavailable
        }
    })?;
    if !output.status.success() {
        return Err(status_of(&output.stderr));
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).map_err(|_| LookupStatus::Unavailable)?;
    if value.get("data").is_none_or(Value::is_null) {
        return Err(LookupStatus::Unavailable);
    }
    Ok(value["data"].clone())
}

pub(crate) fn lookup_error(status: LookupStatus) -> Error {
    match status {
        LookupStatus::CliMissing => Error::new(
            "gh_missing",
            "GitHub CLI (gh) is not installed. Install it and run `gh auth login`.",
        ),
        LookupStatus::AuthRequired => Error::new(
            "gh_auth",
            "GitHub CLI is not signed in. Run `gh auth login` in a terminal.",
        ),
        _ => Error::new("github_unavailable", "GitHub did not answer. Try again."),
    }
}

/// A fixed REST change. Never publishes raw output; maps refusals to short reasons.
async fn rest(binary: &OsStr, method: &str, endpoint: &str, fields: &[(&str, &str)]) -> Result<()> {
    let mut command = hardened(binary);
    command.args([
        "api",
        "--hostname",
        "github.com",
        "--method",
        method,
        "-H",
        "Accept: application/vnd.github+json",
        endpoint,
    ]);
    for (key, value) in fields {
        command.arg("-f").arg(format!("{key}={value}"));
    }
    let output = crate::cli_output::capture_command(command, Duration::from_secs(30)).await;
    crate::gh_gate::wrote(&output);
    let output = output.map_err(|error| {
        lookup_error(if error.kind() == std::io::ErrorKind::NotFound {
            LookupStatus::CliMissing
        } else {
            LookupStatus::Unavailable
        })
    })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(if stderr.contains("HTTP 409") {
        Error::new(
            "github_conflict",
            "The pull request changed on GitHub. Refresh and review it again.",
        )
    } else if stderr.contains("HTTP 405") {
        Error::new(
            "github_refused",
            "GitHub cannot merge this pull request now (checks, reviews or conflicts).",
        )
    } else if stderr.contains("HTTP 403") || stderr.contains("HTTP 404") {
        Error::new(
            "github_forbidden",
            "Your GitHub account cannot change this repository.",
        )
    } else if stderr.contains("HTTP 422") {
        Error::new("github_invalid", "GitHub refused this change.")
    } else {
        lookup_error(status_of(&output.stderr))
    })
}

fn string(value: &Value, limit: usize) -> Option<String> {
    value
        .as_str()
        .map(|item| text(item, limit))
        .filter(|item| !item.is_empty())
}

/// Multi-line text (bodies, comments) keeps newlines and tabs only.
fn body(value: &Value, limit: usize) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(limit)
        .collect()
}

fn login(value: &Value) -> Option<String> {
    string(&value["login"], 100)
}

fn count(value: &Value) -> u32 {
    value
        .as_u64()
        .map(|n| n.min(u32::MAX as u64) as u32)
        .unwrap_or(0)
}

fn nodes(value: &Value) -> &[Value] {
    value["nodes"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn labels(value: &Value) -> Vec<Label> {
    nodes(value)
        .iter()
        .filter_map(|label| {
            Some(Label {
                name: string(&label["name"], 80)?,
                color: label["color"]
                    .as_str()
                    .filter(|c| c.len() == 6 && c.bytes().all(|b| b.is_ascii_hexdigit()))
                    .map(str::to_owned),
            })
        })
        .collect()
}

fn item(node: &Value, repo: &str) -> Option<Item> {
    let kind = match node["__typename"].as_str() {
        Some("PullRequest") => ItemKind::PullRequest,
        Some("Issue") => ItemKind::Issue,
        _ => return None,
    };
    let number = count(&node["number"]);
    if number == 0 {
        return None;
    }
    let url = github_link(node["url"].as_str(), repo)?;
    let state = match node["state"].as_str() {
        Some("MERGED") => "merged",
        Some("CLOSED") => "closed",
        _ => "open",
    };
    let review_requests = nodes(&node["reviewRequests"])
        .iter()
        .filter_map(|request| login(&request["requestedReviewer"]))
        .collect();
    let checks = node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"]
        .as_str()
        .map(|state| text(state, 20));
    Some(Item {
        kind,
        repository: repo.to_owned(),
        number,
        title: string(&node["title"], TITLE_LIMIT).unwrap_or_default(),
        url,
        state: state.into(),
        is_draft: node["isDraft"].as_bool().unwrap_or(false),
        author: login(&node["author"]),
        created_at: string(&node["createdAt"], 40).unwrap_or_default(),
        updated_at: string(&node["updatedAt"], 40).unwrap_or_default(),
        head_ref: string(&node["headRefName"], 255),
        base_ref: string(&node["baseRefName"], 255),
        additions: count(&node["additions"]),
        deletions: count(&node["deletions"]),
        review_decision: string(&node["reviewDecision"], 40),
        mergeable: string(&node["mergeable"], 40),
        labels: labels(&node["labels"]),
        comment_count: count(&node["comments"]["totalCount"]),
        assignees: nodes(&node["assignees"]).iter().filter_map(login).collect(),
        review_requests,
        checks,
    })
}

const LIST_QUERY: &str = "query($q:String!,$first:Int!){viewer{login} search(query:$q,type:ISSUE,first:$first){nodes{__typename \
... on PullRequest{number title url state isDraft createdAt updatedAt headRefName baseRefName additions deletions reviewDecision mergeable author{login} labels(first:10){nodes{name color}} comments{totalCount} assignees(first:10){nodes{login}} reviewRequests(first:10){nodes{requestedReviewer{... on User{login}}}} commits(last:1){nodes{commit{statusCheckRollup{state}}}}} \
... on Issue{number title url state createdAt updatedAt author{login} labels(first:10){nodes{name color}} comments{totalCount} assignees(first:10){nodes{login}}}}}}";

fn search_query(repo: &str, kind: ItemKind, state: ItemState) -> String {
    let filter = match (kind, state) {
        (_, ItemState::Open) => "is:open",
        (ItemKind::PullRequest, ItemState::Closed) => "is:closed is:unmerged",
        (ItemKind::PullRequest, ItemState::Merged) => "is:merged",
        (ItemKind::Issue, _) => "is:closed",
    };
    format!(
        "repo:{repo} {} {filter} sort:updated-desc",
        if kind == ItemKind::PullRequest {
            "is:pr"
        } else {
            "is:issue"
        }
    )
}

async fn list(
    binary: &OsStr,
    repositories: Vec<Repository>,
    kind: ItemKind,
    state: ItemState,
) -> Inbox {
    let mut tasks = tokio::task::JoinSet::new();
    for (index, repo) in repositories.iter().enumerate() {
        let repo = repo.repository.clone();
        let binary = binary.to_owned();
        tasks.spawn(async move {
            let query = search_query(&repo, kind, state);
            let result = graphql(&binary, LIST_QUERY, &[("q", &query)], &[("first", PAGE)]).await;
            (index, repo, result)
        });
    }
    let mut results = Vec::new();
    while let Some(Ok(result)) = tasks.join_next().await {
        results.push(result);
    }
    results.sort_by_key(|(index, _, _)| *index);
    let mut inbox = Inbox {
        viewer: None,
        repositories,
        items: vec![],
        failures: vec![],
        checked_at: chrono::Utc::now().to_rfc3339(),
    };
    for (_, repo, result) in results {
        match result {
            Ok(data) => {
                inbox.viewer = inbox.viewer.or_else(|| login(&data["viewer"]));
                inbox.items.extend(
                    nodes(&data["search"])
                        .iter()
                        .filter_map(|node| item(node, &repo)),
                );
            }
            Err(status) => inbox.failures.push(Failure {
                repository: repo,
                status,
            }),
        }
    }
    inbox.items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    inbox
}

const PR_DETAIL_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed pullRequest(number:$number){__typename \
number title url state isDraft body createdAt updatedAt mergedAt closedAt headRefName baseRefName headRefOid additions deletions changedFiles reviewDecision mergeable mergeStateStatus viewerCanUpdate author{login} \
labels(first:20){nodes{name color}} comments{totalCount} assignees(first:10){nodes{login}} reviewRequests(first:20){nodes{requestedReviewer{... on User{login} ... on Team{name}}}} \
reviews(last:30){nodes{author{login} state body submittedAt}} commentsList:comments(last:50){nodes{author{login} body createdAt}} \
commits(last:50){nodes{commit{oid messageHeadline committedDate author{name} statusCheckRollup{state contexts(first:80){nodes{__typename ... on CheckRun{name status conclusion detailsUrl startedAt completedAt} ... on StatusContext{context state targetUrl}}}}}}} \
files(first:100){totalCount nodes{path additions deletions changeType}}}}}";

const ISSUE_DETAIL_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){issue(number:$number){__typename \
number title url state body createdAt updatedAt closedAt viewerCanUpdate author{login} labels(first:20){nodes{name color}} comments{totalCount} assignees(first:10){nodes{login}} \
commentsList:comments(last:50){nodes{author{login} body createdAt}}}}}";

pub(crate) fn check_status(node: &Value) -> String {
    let value = match node["__typename"].as_str() {
        Some("CheckRun") => match (node["status"].as_str(), node["conclusion"].as_str()) {
            (Some("COMPLETED"), Some("SUCCESS")) => "passed",
            (Some("COMPLETED"), Some("NEUTRAL" | "SKIPPED")) => "skipped",
            (Some("COMPLETED"), Some(_)) => "failed",
            (Some(_), _) => "pending",
            _ => "unknown",
        },
        Some("StatusContext") => match node["state"].as_str() {
            Some("SUCCESS") => "passed",
            Some("FAILURE" | "ERROR") => "failed",
            Some("PENDING" | "EXPECTED") => "pending",
            _ => "unknown",
        },
        _ => "unknown",
    };
    value.into()
}

async fn detail(binary: &OsStr, repo: &str, number: u32, kind: ItemKind) -> Result<Detail> {
    let (owner, name) = repo.split_once('/').expect("validated repository");
    let query = if kind == ItemKind::PullRequest {
        PR_DETAIL_QUERY
    } else {
        ISSUE_DETAIL_QUERY
    };
    let data = graphql(
        binary,
        query,
        &[("owner", owner), ("name", name)],
        &[("number", number)],
    )
    .await
    .map_err(lookup_error)?;
    let repository = &data["repository"];
    let node = if kind == ItemKind::PullRequest {
        &repository["pullRequest"]
    } else {
        &repository["issue"]
    };
    let item = item(node, repo).ok_or_else(|| Error::not_found("Not found on GitHub."))?;
    let latest = node["commits"]["nodes"]
        .as_array()
        .and_then(|commits| commits.last())
        .cloned()
        .unwrap_or(Value::Null);
    let checks = nodes(&latest["commit"]["statusCheckRollup"]["contexts"])
        .iter()
        .filter_map(|check| {
            let repo_link = |field: &str| github_link(check[field].as_str(), repo);
            Some(Check {
                name: string(&check["name"], 200).or_else(|| string(&check["context"], 200))?,
                status: check_status(check),
                url: repo_link("detailsUrl").or_else(|| repo_link("targetUrl")),
                started_at: string(&check["startedAt"], 40),
                completed_at: string(&check["completedAt"], 40),
            })
        })
        .collect();
    let mut merge_methods = vec![];
    for (field, method) in [
        ("squashMergeAllowed", MergeMethod::Squash),
        ("mergeCommitAllowed", MergeMethod::Merge),
        ("rebaseMergeAllowed", MergeMethod::Rebase),
    ] {
        if repository[field].as_bool().unwrap_or(false) {
            merge_methods.push(method);
        }
    }
    let files_total = count(&node["files"]["totalCount"]);
    let files: Vec<ChangedFile> = nodes(&node["files"])
        .iter()
        .filter_map(|file| {
            Some(ChangedFile {
                path: string(&file["path"], 1024)?,
                additions: count(&file["additions"]),
                deletions: count(&file["deletions"]),
                change_type: string(&file["changeType"], 20).unwrap_or_default(),
            })
        })
        .collect();
    Ok(Detail {
        body: body(&node["body"], BODY_LIMIT),
        head_oid: node["headRefOid"]
            .as_str()
            .filter(|oid| sha(oid))
            .map(str::to_ascii_lowercase),
        merged_at: string(&node["mergedAt"], 40),
        closed_at: string(&node["closedAt"], 40),
        merge_state_status: string(&node["mergeStateStatus"], 40),
        changed_files: count(&node["changedFiles"]),
        viewer_can_update: node["viewerCanUpdate"].as_bool().unwrap_or(false),
        merge_methods,
        checks,
        reviewers: nodes(&node["reviewRequests"])
            .iter()
            .filter_map(|request| {
                login(&request["requestedReviewer"])
                    .or_else(|| string(&request["requestedReviewer"]["name"], 100))
            })
            .collect(),
        reviews: nodes(&node["reviews"])
            .iter()
            .map(|review| Review {
                author: login(&review["author"]),
                state: string(&review["state"], 40).unwrap_or_default(),
                body: body(&review["body"], COMMENT_LIMIT),
                submitted_at: string(&review["submittedAt"], 40),
            })
            .collect(),
        comments: nodes(&node["commentsList"])
            .iter()
            .map(|comment| Comment {
                author: login(&comment["author"]),
                body: body(&comment["body"], COMMENT_LIMIT),
                created_at: string(&comment["createdAt"], 40).unwrap_or_default(),
            })
            .collect(),
        commits: nodes(&node["commits"])
            .iter()
            .filter_map(|commit| {
                let commit = &commit["commit"];
                Some(Commit {
                    oid: commit["oid"].as_str().filter(|oid| sha(oid))?.to_owned(),
                    headline: string(&commit["messageHeadline"], 300).unwrap_or_default(),
                    author: string(&commit["author"]["name"], 100),
                    committed_at: string(&commit["committedDate"], 40),
                })
            })
            .collect(),
        files_truncated: (files.len() as u32) < files_total,
        files,
        item,
    })
}

async fn diff(binary: &OsStr, repo: &str, number: u32) -> Result<(String, bool)> {
    let mut command = hardened(binary);
    command.args([
        "api",
        "--hostname",
        "github.com",
        "--method",
        "GET",
        "-H",
        "Accept: application/vnd.github.diff",
        &format!("/repos/{repo}/pulls/{number}"),
    ]);
    let output = crate::gh_gate::read(command, Duration::from_secs(30)).await;
    match output {
        Ok(output) if output.status.success() => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            let truncated = text.len() > DIFF_LIMIT;
            if truncated {
                let mut end = DIFF_LIMIT;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
            Ok((text, truncated))
        }
        // The capture refuses outputs over its own 2 MiB ceiling.
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => Err(Error::new(
            "diff_too_large",
            "This diff is too large to show here. Open it on GitHub.",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(lookup_error(LookupStatus::CliMissing))
        }
        Err(_) => Err(lookup_error(LookupStatus::Unavailable)),
        Ok(output) => Err(lookup_error(status_of(&output.stderr))),
    }
}

const FAILURES_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){commits(last:1){nodes{commit{statusCheckRollup{contexts(first:80){nodes{__typename \
... on CheckRun{databaseId name status conclusion detailsUrl title summary checkSuite{app{slug}} annotations(first:10){nodes{path message location{start{line}}}}} \
... on StatusContext{context state targetUrl description}}}}}}}}}}";

pub(crate) async fn failures(
    binary: &OsStr,
    repo: &str,
    number: u32,
) -> Result<(Vec<FailedCheck>, bool)> {
    let (owner, name) = repo.split_once('/').expect("validated repository");
    let data = graphql(
        binary,
        FAILURES_QUERY,
        &[("owner", owner), ("name", name)],
        &[("number", number)],
    )
    .await
    .map_err(lookup_error)?;
    let node = &data["repository"]["pullRequest"];
    if node.is_null() {
        return Err(Error::not_found("Not found on GitHub."));
    }
    let failed: Vec<&Value> =
        nodes(&node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"])
            .iter()
            .filter(|check| check_status(check) == "failed")
            .collect();
    let truncated = failed.len() > MAX_FAILURES;
    let mut budget = LOG_BUDGET;
    let mut checks = vec![];
    for check in failed.into_iter().take(MAX_FAILURES) {
        let Some(name) = string(&check["name"], 200).or_else(|| string(&check["context"], 200))
        else {
            continue;
        };
        let summary = ["title", "summary", "description"]
            .iter()
            .filter_map(|field| {
                Some(body(&check[*field], 1_500).trim().to_owned()).filter(|v| !v.is_empty())
            })
            .collect::<Vec<_>>()
            .join("\n");
        let annotations = nodes(&check["annotations"])
            .iter()
            .filter_map(|note| {
                let message = string(&note["message"], 300)?;
                let path = string(&note["path"], 300).unwrap_or_default();
                Some(match note["location"]["start"]["line"].as_u64() {
                    Some(line) => format!("{path}:{line} {message}"),
                    None if path.is_empty() => message,
                    None => format!("{path} {message}"),
                })
            })
            .collect();
        // A GitHub Actions check run's database ID is its job ID.
        let job = (check["checkSuite"]["app"]["slug"].as_str() == Some("github-actions"))
            .then(|| check["databaseId"].as_u64())
            .flatten();
        let log = match job {
            Some(job) if budget > 0 => job_log(binary, repo, job)
                .await
                .map(|log| log_excerpt(&log, LOG_EXCERPT_LIMIT.min(budget)))
                .filter(|log| !log.is_empty()),
            _ => None,
        };
        budget = budget.saturating_sub(log.as_ref().map_or(0, String::len));
        checks.push(FailedCheck {
            url: github_link(check["detailsUrl"].as_str(), repo)
                .or_else(|| github_link(check["targetUrl"].as_str(), repo)),
            summary: Some(summary).filter(|s| !s.is_empty()),
            annotations,
            log,
            name,
        });
    }
    Ok((checks, truncated))
}

/// One Actions job log through a fixed GET (bounded by the 2 MiB capture ceiling).
async fn job_log(binary: &OsStr, repo: &str, job: u64) -> Option<String> {
    let mut command = hardened(binary);
    command.args([
        "api",
        "--hostname",
        "github.com",
        "--method",
        "GET",
        &format!("/repos/{repo}/actions/jobs/{job}/logs"),
    ]);
    let output = crate::gh_gate::read(command, Duration::from_secs(20))
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Removes the timestamp prefix, ANSI colour codes and control characters of one log line.
fn clean_log_line(line: &str) -> String {
    let line = line.trim_start_matches('\u{feff}');
    let line = match line.split_once(' ') {
        Some((stamp, rest))
            if stamp.len() >= 20
                && stamp.ends_with('Z')
                && stamp.as_bytes()[4] == b'-'
                && stamp.as_bytes()[10] == b'T' =>
        {
            rest
        }
        _ => line,
    };
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
        } else if !ch.is_control() || ch == '\t' {
            out.push(ch);
        }
    }
    out.trim_end().to_owned()
}

fn is_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    line.contains("##[error]")
        || lower.starts_with("error")
        || lower.contains("error:")
        || lower.contains("error[")
        || line.contains("FAIL")
        || lower.contains("panicked at")
        || lower.contains("assertionerror")
        || line.contains('✕')
        || line.contains('✗')
}

/// The lines around each error marker plus the end of the log, at most `limit`
/// bytes (keeping the end, where the failure summary usually is).
fn log_excerpt(log: &str, limit: usize) -> String {
    let lines: Vec<String> = log
        .lines()
        .map(clean_log_line)
        .take_while(|line| {
            !line.starts_with("Post job cleanup") && line != "Cleaning up orphan processes"
        })
        .filter(|line| !line.is_empty() && line != "##[endgroup]")
        .collect();
    let mut keep = vec![false; lines.len()];
    for (index, line) in lines.iter().enumerate() {
        if is_error_line(line) {
            for slot in &mut keep[index.saturating_sub(15)..(index + 3).min(lines.len())] {
                *slot = true;
            }
        }
    }
    for slot in &mut keep[lines.len().saturating_sub(30)..] {
        *slot = true;
    }
    let mut out = String::new();
    let mut skipped = false;
    for (line, kept) in lines.iter().zip(keep) {
        if !kept {
            skipped = true;
            continue;
        }
        if skipped && !out.is_empty() {
            out.push_str("…\n");
        }
        skipped = false;
        out.push_str(line);
        out.push('\n');
    }
    if out.len() > limit {
        let mut start = out.len() - limit + "…\n".len();
        while !out.is_char_boundary(start) {
            start += 1;
        }
        out = format!("…\n{}", &out[start..]);
    }
    out.trim_end().to_owned()
}

const PR_ID_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){id isDraft}}}";
const READY_MUTATION: &str = "mutation($id:ID!){markPullRequestReadyForReview(input:{pullRequestId:$id}){pullRequest{isDraft}}}";
const DRAFT_MUTATION: &str =
    "mutation($id:ID!){convertPullRequestToDraft(input:{pullRequestId:$id}){pullRequest{isDraft}}}";

async fn set_draft(binary: &OsStr, repo: &str, number: u32, draft: bool) -> Result<()> {
    let (owner, name) = repo.split_once('/').expect("validated repository");
    let data = graphql(
        binary,
        PR_ID_QUERY,
        &[("owner", owner), ("name", name)],
        &[("number", number)],
    )
    .await
    .map_err(lookup_error)?;
    let node = &data["repository"]["pullRequest"];
    if node["isDraft"].as_bool() == Some(draft) {
        return Ok(());
    }
    let id = node["id"]
        .as_str()
        .filter(|id| node_id(id))
        .ok_or_else(|| Error::not_found("Pull request not found on GitHub."))?;
    graphql(
        binary,
        if draft {
            DRAFT_MUTATION
        } else {
            READY_MUTATION
        },
        &[("id", id)],
        &[],
    )
    .await
    .map_err(|status| match status {
        LookupStatus::Unavailable => Error::new(
            "github_refused",
            "GitHub refused to change the draft state.",
        ),
        other => lookup_error(other),
    })?;
    Ok(())
}

/// A GitHub GraphQL node ID (`PR_…`, `PRRT_…`): short, URL-safe base64 characters only.
fn node_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'='))
}

/// Review threads read for the Timeline, and comments read per thread.
const MAX_THREADS: u32 = 40;
const MAX_THREAD_COMMENTS: u32 = 20;
/// The diff hunk shown above a thread keeps its last lines only.
const HUNK_LINES: usize = 12;
const HUNK_LIMIT: usize = 4_000;

const THREADS_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!,$first:Int!,$comments:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){\
reviewThreads(last:$first){totalCount nodes{id isResolved isOutdated path line startLine originalLine viewerCanReply viewerCanResolve viewerCanUnresolve \
hunk:comments(first:1){nodes{diffHunk}} comments(first:$comments){totalCount nodes{author{login} body createdAt}}}}}}}";
const THREAD_OWNER_QUERY: &str = "query($id:ID!){node(id:$id){__typename ... on PullRequestReviewThread{pullRequest{number repository{nameWithOwner}}}}}";
const REPLY_MUTATION: &str = "mutation($id:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$id,body:$body}){comment{id}}}";
const RESOLVE_MUTATION: &str =
    "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}";
const UNRESOLVE_MUTATION: &str =
    "mutation($id:ID!){unresolveReviewThread(input:{threadId:$id}){thread{isResolved}}}";

async fn review_threads(
    binary: &OsStr,
    repo: &str,
    number: u32,
) -> Result<(Vec<ReviewThread>, bool)> {
    let (owner, name) = repo.split_once('/').expect("validated repository");
    let data = graphql(
        binary,
        THREADS_QUERY,
        &[("owner", owner), ("name", name)],
        &[
            ("number", number),
            ("first", MAX_THREADS),
            ("comments", MAX_THREAD_COMMENTS),
        ],
    )
    .await
    .map_err(lookup_error)?;
    let node = &data["repository"]["pullRequest"];
    if node.is_null() {
        return Err(Error::not_found("Not found on GitHub."));
    }
    Ok(parse_threads(&node["reviewThreads"]))
}

fn line(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .filter(|line| (1..=10_000_000).contains(line))
        .map(|line| line as u32)
}

/// The last `HUNK_LINES` lines of a diff hunk, control-stripped and bounded.
fn hunk_tail(value: &Value) -> Option<String> {
    let hunk = body(value, 200_000);
    let lines: Vec<&str> = hunk.lines().collect();
    let tail = lines[lines.len().saturating_sub(HUNK_LINES)..].join("\n");
    let tail: String = tail.chars().take(HUNK_LIMIT).collect();
    Some(tail).filter(|tail| !tail.trim().is_empty())
}

fn parse_threads(threads: &Value) -> (Vec<ReviewThread>, bool) {
    let parsed: Vec<ReviewThread> = nodes(threads)
        .iter()
        .filter_map(|thread| {
            let id = thread["id"].as_str().filter(|id| node_id(id))?.to_owned();
            let comments: Vec<Comment> = nodes(&thread["comments"])
                .iter()
                .map(|comment| Comment {
                    author: login(&comment["author"]),
                    body: body(&comment["body"], COMMENT_LIMIT),
                    created_at: string(&comment["createdAt"], 40).unwrap_or_default(),
                })
                .collect();
            if comments.is_empty() {
                return None;
            }
            Some(ReviewThread {
                id,
                path: string(&thread["path"], 1024).unwrap_or_default(),
                line: line(&thread["line"]),
                start_line: line(&thread["startLine"]),
                original_line: line(&thread["originalLine"]),
                is_resolved: thread["isResolved"].as_bool().unwrap_or(false),
                is_outdated: thread["isOutdated"].as_bool().unwrap_or(false),
                viewer_can_reply: thread["viewerCanReply"].as_bool().unwrap_or(false),
                viewer_can_resolve: thread["viewerCanResolve"].as_bool().unwrap_or(false),
                viewer_can_unresolve: thread["viewerCanUnresolve"].as_bool().unwrap_or(false),
                diff_hunk: hunk_tail(&thread["hunk"]["nodes"][0]["diffHunk"]),
                comment_count: count(&thread["comments"]["totalCount"]).max(comments.len() as u32),
                comments,
            })
        })
        .collect();
    let truncated = count(&threads["totalCount"]) > parsed.len() as u32;
    (parsed, truncated)
}

/// Whether a `node(id:)` answer is a review thread of this repository's pull request.
fn thread_belongs(data: &Value, repo: &str, number: u32) -> bool {
    let node = &data["node"];
    node["__typename"].as_str() == Some("PullRequestReviewThread")
        && node["pullRequest"]["number"].as_u64() == Some(u64::from(number))
        && node["pullRequest"]["repository"]["nameWithOwner"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(repo))
}

/// A thread ID the renderer sent, checked natively to be a review thread of
/// this owned pull request before anything changes.
async fn owned_thread<'a>(binary: &OsStr, repo: &str, number: u32, id: &'a str) -> Result<&'a str> {
    let refused = || Error::not_found("This conversation is not part of this pull request.");
    if !node_id(id) {
        return Err(refused());
    }
    let data = graphql(binary, THREAD_OWNER_QUERY, &[("id", id)], &[])
        .await
        .map_err(lookup_error)?;
    if thread_belongs(&data, repo, number) {
        Ok(id)
    } else {
        Err(refused())
    }
}

async fn thread_mutation(binary: &OsStr, mutation: &str, strings: &[(&str, &str)]) -> Result<()> {
    graphql(binary, mutation, strings, &[])
        .await
        .map_err(|status| match status {
            LookupStatus::Unavailable => {
                Error::new("github_refused", "GitHub refused this change.")
            }
            other => lookup_error(other),
        })?;
    Ok(())
}

/// The fixed GitHub URL of an image attachment ID: `<uuid>` is a
/// `user-attachments` upload, `<number>/<uuid>` an older upload of this repository.
fn attachment_url(repo: &str, asset: &str) -> Option<String> {
    let uuid = |value: &str| {
        value.len() == 36
            && value.bytes().enumerate().all(|(index, b)| match index {
                8 | 13 | 18 | 23 => b == b'-',
                _ => b.is_ascii_hexdigit(),
            })
    };
    match asset.split_once('/') {
        None if uuid(asset) => Some(format!(
            "https://github.com/user-attachments/assets/{}",
            asset.to_ascii_lowercase()
        )),
        Some((id, rest))
            if !id.is_empty()
                && id.len() <= 12
                && id.bytes().all(|b| b.is_ascii_digit())
                && uuid(rest) =>
        {
            Some(format!(
                "https://github.com/{repo}/assets/{id}/{}",
                rest.to_ascii_lowercase()
            ))
        }
        _ => None,
    }
}

/// A `data:` URL for PNG, JPEG, GIF or WebP bytes recognized by their magic
/// bytes (never the server's content type); anything else is refused.
fn image_data_url(bytes: &[u8]) -> Option<String> {
    use base64::Engine as _;
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Reads an attachment through `gh`, which adds the person's token for
/// github.com only (Go drops it on the redirect to the storage host). The
/// token never leaves `gh`; output is bounded by the 2 MiB capture ceiling.
async fn attachment(binary: &OsStr, url: &str) -> Result<String> {
    let mut command = hardened(binary);
    command.args([
        "api",
        "--hostname",
        "github.com",
        "--method",
        "GET",
        "-H",
        "Accept: image/*",
        url,
    ]);
    match crate::gh_gate::read(command, Duration::from_secs(30)).await {
        Ok(output) if output.status.success() => image_data_url(&output.stdout)
            .ok_or_else(|| Error::new("invalid", "This attachment is not an image.")),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => Err(Error::new(
            "attachment_too_large",
            "This image is too large to show here.",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(lookup_error(LookupStatus::CliMissing))
        }
        Err(_) => Err(lookup_error(LookupStatus::Unavailable)),
        Ok(output) => Err(lookup_error(status_of(&output.stderr))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repos() -> Vec<Repository> {
        vec![Repository {
            repository: "owner/repo".into(),
            project_ids: vec!["p".into()],
        }]
    }

    #[test]
    fn only_owned_repositories_and_plausible_numbers_are_accepted() {
        let repos = repos();
        assert_eq!(owned(&repos, "Owner/Repo", 7).unwrap(), "owner/repo");
        assert!(owned(&repos, "other/repo", 7).is_err());
        assert!(owned(&repos, "owner/repo", 0).is_err());
        assert!(owned(&repos, "owner/repo", 20_000_000).is_err());
        assert!(confirmed(false).is_err() && confirmed(true).is_ok());
    }

    #[test]
    fn actions_are_closed_and_mutations_carry_confirmation() {
        assert!(serde_json::from_str::<Action>(
            r#"{"type":"list","kind":"pullRequest","state":"open"}"#
        )
        .is_ok());
        assert!(serde_json::from_str::<Action>(
            r#"{"type":"list","kind":"pullRequest","state":"open","query":"repo:x/y"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Action>(
            r#"{"type":"merge","repository":"owner/repo","number":1,"method":"squash","expectedHead":"x"}"#
        )
        .is_err(), "confirm is required");
        assert!(serde_json::from_str::<Action>(
            r#"{"type":"merge","repository":"owner/repo","number":1,"method":"octopus","expectedHead":"x","confirm":true}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Action>(
            r#"{"type":"delete","repository":"owner/repo","number":1}"#
        )
        .is_err());
    }

    #[test]
    fn search_text_is_built_natively_from_validated_parts() {
        assert_eq!(
            search_query("owner/repo", ItemKind::PullRequest, ItemState::Open),
            "repo:owner/repo is:pr is:open sort:updated-desc"
        );
        assert_eq!(
            search_query("owner/repo", ItemKind::Issue, ItemState::Closed),
            "repo:owner/repo is:issue is:closed sort:updated-desc"
        );
        assert_eq!(
            search_query("owner/repo", ItemKind::PullRequest, ItemState::Closed),
            "repo:owner/repo is:pr is:closed is:unmerged sort:updated-desc"
        );
        assert_eq!(
            search_query("owner/repo", ItemKind::PullRequest, ItemState::Merged),
            "repo:owner/repo is:pr is:merged sort:updated-desc"
        );
    }

    #[test]
    fn items_keep_only_this_repository_links_and_bounded_text() {
        let node = json!({
            "__typename": "PullRequest", "number": 12, "title": "Fix\u{0007} it",
            "url": "https://github.com/owner/repo/pull/12", "state": "MERGED", "isDraft": false,
            "author": {"login": "ana"}, "createdAt": "t", "updatedAt": "u",
            "labels": {"nodes": [{"name": "bug", "color": "ff0000"}, {"name": "x", "color": "url(evil)"}]},
            "comments": {"totalCount": 3}, "assignees": {"nodes": [{"login": "bob"}]},
            "reviewRequests": {"nodes": [{"requestedReviewer": {"login": "me"}}]},
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "FAILURE"}}}]}
        });
        let parsed = item(&node, "owner/repo").unwrap();
        assert_eq!(parsed.title, "Fix it");
        assert_eq!(parsed.state, "merged");
        assert_eq!(parsed.labels[0].color.as_deref(), Some("ff0000"));
        assert_eq!(parsed.labels[1].color, None);
        assert_eq!(parsed.review_requests, vec!["me".to_string()]);
        assert_eq!(parsed.checks.as_deref(), Some("FAILURE"));
        let mut foreign = node.clone();
        foreign["url"] = json!("https://github.com/other/repo/pull/12");
        assert!(item(&foreign, "owner/repo").is_none());
        let mut evil = node;
        evil["url"] = json!("https://evil.example/owner/repo/pull/12");
        assert!(item(&evil, "owner/repo").is_none());
    }

    #[test]
    fn log_excerpts_keep_errors_and_the_end_without_noise() {
        let mut log = String::from("\u{feff}2026-09-30T15:08:56.0820715Z ##[group]Run npm test\n");
        for index in 0..200 {
            log.push_str(&format!(
                "2026-09-30T15:08:56.0820715Z \u{1b}[36;1mline {index}\u{1b}[0m\n"
            ));
        }
        log.push_str(
            "2026-09-30T15:09:02.7756340Z ##[error]checkout.spec.ts: expected 200, got 500\n",
        );
        log.push_str("2026-09-30T15:09:02.7921553Z Cleaning up orphan processes\n2026-09-30T15:09:02.7921553Z secret tail\n");
        let excerpt = log_excerpt(&log, LOG_EXCERPT_LIMIT);
        assert!(excerpt.contains("##[error]checkout.spec.ts: expected 200, got 500"));
        assert!(excerpt.contains("line 199") && !excerpt.contains("line 20\n"));
        assert!(
            !excerpt.contains('\u{1b}')
                && !excerpt.contains("2026-09-30T")
                && !excerpt.contains("secret tail")
        );
        let short = log_excerpt(&log, 200);
        assert!(short.len() <= 200 && short.starts_with('…') && short.ends_with("got 500"));
    }

    #[test]
    fn failures_action_takes_only_repository_and_number() {
        assert!(serde_json::from_value::<Action>(
            json!({"type":"failures","repository":"o/r","number":4})
        )
        .is_ok());
        assert!(serde_json::from_value::<Action>(
            json!({"type":"failures","repository":"o/r","number":4,"job":1})
        )
        .is_err());
    }

    #[test]
    fn check_states_are_closed_values() {
        assert_eq!(
            check_status(
                &json!({"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"})
            ),
            "passed"
        );
        assert_eq!(
            check_status(
                &json!({"__typename":"CheckRun","status":"COMPLETED","conclusion":"TIMED_OUT"})
            ),
            "failed"
        );
        assert_eq!(
            check_status(&json!({"__typename":"StatusContext","state":"PENDING"})),
            "pending"
        );
        assert_eq!(check_status(&json!({"__typename":"Other"})), "unknown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn graphql_passes_values_as_raw_fields_and_reports_missing_cli() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("gh");
        let log = dir.path().join("argv");
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho '{{\"data\":{{\"viewer\":{{\"login\":\"me\"}}}}}}'\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let data = graphql(
            binary.as_os_str(),
            "query{viewer{login}}",
            &[("q", "@/etc/passwd")],
            &[("n", 3)],
        )
        .await
        .unwrap();
        assert_eq!(data["viewer"]["login"], "me");
        let argv = std::fs::read_to_string(&log).unwrap();
        assert!(
            argv.contains("-f\nq=@/etc/passwd\n"),
            "strings never use -F file expansion"
        );
        assert!(argv.contains("-F\nn=3\n"));
        assert!(argv.contains("--hostname\ngithub.com\n"));
        let missing = graphql(OsStr::new("/nonexistent/gh"), "query{x}", &[], &[]).await;
        assert_eq!(missing.unwrap_err(), LookupStatus::CliMissing);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rest_changes_use_fixed_argv_and_map_refusals() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("gh");
        let log = dir.path().join("argv");
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncase \"$*\" in *merge*) echo 'HTTP 409: Head branch was modified' >&2; exit 1;; esac\necho '{{}}'\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        rest(
            binary.as_os_str(),
            "POST",
            "/repos/owner/repo/issues/7/comments",
            &[("body", "@/etc/passwd\nline two")],
        )
        .await
        .unwrap();
        let argv = std::fs::read_to_string(&log).unwrap();
        assert!(argv.starts_with("api\n--hostname\ngithub.com\n--method\nPOST\n"));
        assert!(argv.contains("-f\nbody=@/etc/passwd\nline two\n"), "{argv}");
        let refused = rest(
            binary.as_os_str(),
            "PUT",
            "/repos/owner/repo/pulls/7/merge",
            &[("merge_method", "squash")],
        )
        .await
        .unwrap_err();
        assert!(format!("{refused:?}").contains("github_conflict"));
    }

    #[test]
    fn thread_actions_are_closed_and_changes_carry_confirmation() {
        assert!(serde_json::from_value::<Action>(json!({
            "type": "replyThread", "repository": "o/r", "number": 4, "threadId": "PRRT_x", "body": "ok", "confirm": true
        }))
        .is_ok());
        assert!(serde_json::from_value::<Action>(json!({
            "type": "replyThread", "repository": "o/r", "number": 4, "threadId": "PRRT_x", "body": "ok"
        }))
        .is_err(), "confirm is required");
        assert!(serde_json::from_value::<Action>(json!({
            "type": "resolveThread", "repository": "o/r", "number": 4, "threadId": "PRRT_x", "resolved": true
        }))
        .is_err(), "confirm is required");
        assert!(serde_json::from_value::<Action>(json!({
            "type": "attachment", "repository": "o/r", "number": 4, "asset": "x", "url": "https://evil.example"
        }))
        .is_err(), "no URL field");
    }

    #[test]
    fn review_threads_parse_bounded_with_hunk_tail_and_state() {
        let hunk: String = (0..40).map(|i| format!("+line {i}\n")).collect();
        let threads = json!({"totalCount": 3, "nodes": [
            {"id": "PRRT_kwDOabc", "path": "src/a.ts", "line": 12, "startLine": null, "originalLine": 10,
             "isResolved": true, "isOutdated": false, "viewerCanReply": true, "viewerCanResolve": false, "viewerCanUnresolve": true,
             "hunk": {"nodes": [{"diffHunk": hunk}]},
             "comments": {"totalCount": 25, "nodes": [{"author": {"login": "ana"}, "body": "Why\u{0007}?", "createdAt": "2026-10-01T00:00:00Z"}]}},
            {"id": "bad id; x", "path": "b", "comments": {"nodes": [{"body": "x"}]}},
            {"id": "PRRT_empty", "path": "c", "comments": {"nodes": []}}
        ]});
        let (parsed, truncated) = parse_threads(&threads);
        assert!(truncated);
        assert_eq!(parsed.len(), 1);
        let thread = &parsed[0];
        assert_eq!(
            (thread.line, thread.start_line, thread.original_line),
            (Some(12), None, Some(10))
        );
        assert!(thread.is_resolved && thread.viewer_can_unresolve && !thread.viewer_can_resolve);
        assert_eq!(thread.comment_count, 25);
        assert_eq!(thread.comments[0].body, "Why?");
        let hunk = thread.diff_hunk.as_deref().unwrap();
        assert_eq!(hunk.lines().count(), HUNK_LINES);
        assert!(hunk.ends_with("+line 39") && !hunk.contains("+line 27\n"));
    }

    #[test]
    fn only_threads_of_this_pull_request_can_change() {
        let answer = |name: &str, number: u64, kind: &str| json!({"node": {"__typename": kind, "pullRequest": {"number": number, "repository": {"nameWithOwner": name}}}});
        assert!(thread_belongs(
            &answer("Owner/Repo", 7, "PullRequestReviewThread"),
            "owner/repo",
            7
        ));
        assert!(!thread_belongs(
            &answer("owner/other", 7, "PullRequestReviewThread"),
            "owner/repo",
            7
        ));
        assert!(!thread_belongs(
            &answer("owner/repo", 8, "PullRequestReviewThread"),
            "owner/repo",
            7
        ));
        assert!(!thread_belongs(
            &answer("owner/repo", 7, "Issue"),
            "owner/repo",
            7
        ));
        assert!(!thread_belongs(&json!({"node": null}), "owner/repo", 7));
        assert!(
            node_id("PRRT_kwDOA-b_c=")
                && !node_id("")
                && !node_id("a b")
                && !node_id(&"a".repeat(129))
        );
    }

    #[test]
    fn attachments_are_fixed_github_urls_and_images_by_magic_bytes() {
        let uuid = "E7D69315-2274-4873-a343-7c087c9f8543";
        assert_eq!(
            attachment_url("owner/repo", uuid).as_deref(),
            Some("https://github.com/user-attachments/assets/e7d69315-2274-4873-a343-7c087c9f8543")
        );
        assert_eq!(
            attachment_url("owner/repo", &format!("123/{uuid}")).as_deref(),
            Some("https://github.com/owner/repo/assets/123/e7d69315-2274-4873-a343-7c087c9f8543")
        );
        for bad in [
            "",
            "x",
            "../e7d69315-2274-4873-a343-7c087c9f8543",
            "12/34/e7d69315-2274-4873-a343-7c087c9f8543",
            "e7d69315x2274-4873-a343-7c087c9f8543",
            "a1/e7d69315-2274-4873-a343-7c087c9f8543",
            "https://evil.example/x",
        ] {
            assert_eq!(attachment_url("owner/repo", bad), None, "{bad}");
        }
        assert!(image_data_url(b"\x89PNG\r\n\x1a\n rest")
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert!(image_data_url(b"\xff\xd8\xff\xe0")
            .unwrap()
            .starts_with("data:image/jpeg;base64,"));
        assert!(image_data_url(b"GIF89a..")
            .unwrap()
            .starts_with("data:image/gif;base64,"));
        assert!(image_data_url(b"RIFF\0\0\0\0WEBPVP8 ")
            .unwrap()
            .starts_with("data:image/webp;base64,"));
        assert_eq!(
            image_data_url(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            None
        );
        assert_eq!(image_data_url(b"<!DOCTYPE html>"), None);
        assert_eq!(image_data_url(b""), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn thread_changes_check_ownership_first_and_attachments_read_through_gh() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("gh");
        let log = dir.path().join("argv");
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\ncase \"$*\" in\n *user-attachments*) printf 'GIF89a';;\n *node\\(id*) echo '{{\"data\":{{\"node\":{{\"__typename\":\"PullRequestReviewThread\",\"pullRequest\":{{\"number\":7,\"repository\":{{\"nameWithOwner\":\"owner/repo\"}}}}}}}}}}';;\n *) echo '{{\"data\":{{}}}}';;\nesac\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let gh = binary.as_os_str();
        assert_eq!(
            owned_thread(gh, "owner/repo", 7, "PRRT_a").await.unwrap(),
            "PRRT_a"
        );
        assert!(owned_thread(gh, "owner/repo", 8, "PRRT_a").await.is_err());
        assert!(owned_thread(gh, "other/repo", 7, "PRRT_a").await.is_err());
        assert!(owned_thread(gh, "owner/repo", 7, "bad id").await.is_err());
        thread_mutation(
            gh,
            REPLY_MUTATION,
            &[("id", "PRRT_a"), ("body", "@/etc/passwd")],
        )
        .await
        .unwrap();
        let url = attachment_url("owner/repo", "e7d69315-2274-4873-a343-7c087c9f8543").unwrap();
        assert_eq!(
            attachment(gh, &url).await.unwrap(),
            "data:image/gif;base64,R0lGODlh"
        );
        let argv = std::fs::read_to_string(&log).unwrap();
        assert!(
            argv.contains("-f\nbody=@/etc/passwd\n"),
            "reply bodies are raw fields"
        );
        assert!(argv.contains(
            "--method\nGET\n-H\nAccept: image/*\nhttps://github.com/user-attachments/assets/"
        ));
    }

    /// Read-only queries against real GitHub through the local `gh` login.
    /// `LIVE_GH_REPO=owner/repo cargo test --lib live_ -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_read_only_queries_parse() {
        let repo = std::env::var("LIVE_GH_REPO").unwrap_or_else(|_| "cli/cli".into());
        let repos = vec![Repository {
            repository: repo.clone(),
            project_ids: vec!["p".into()],
        }];
        let binary = OsStr::new("gh");
        let mut first_pr = None;
        for kind in [ItemKind::PullRequest, ItemKind::Issue] {
            for state in [ItemState::Open, ItemState::Closed, ItemState::Merged] {
                let inbox = list(binary, repos.clone(), kind, state).await;
                assert!(
                    inbox.failures.is_empty(),
                    "{kind:?} {state:?}: {:?}",
                    inbox.failures
                );
                assert!(inbox.viewer.is_some());
                eprintln!("{kind:?} {state:?}: {} items", inbox.items.len());
                if kind == ItemKind::PullRequest && first_pr.is_none() {
                    first_pr = inbox.items.first().map(|item| item.number);
                }
                if let Some(item) = inbox.items.first() {
                    let detail = detail(binary, &repo, item.number, kind).await.unwrap();
                    eprintln!(
                        "  #{} {:?} checks={} reviews={} comments={} commits={} files={} methods={:?}",
                        detail.item.number,
                        detail.item.state,
                        detail.checks.len(),
                        detail.reviews.len(),
                        detail.comments.len(),
                        detail.commits.len(),
                        detail.files.len(),
                        detail.merge_methods
                    );
                }
            }
        }
        if let Some(number) = first_pr {
            match diff(binary, &repo, number).await {
                Ok((text, truncated)) => eprintln!(
                    "diff #{number}: {} bytes, truncated={truncated}",
                    text.len()
                ),
                Err(error) => eprintln!("diff #{number}: {error:?}"),
            }
        }
    }

    fn pull(number: u32, author: Option<&str>, state: &str, reviewers: &[&str]) -> Item {
        Item {
            kind: ItemKind::PullRequest,
            repository: "Owner/Repo".into(),
            number,
            title: format!("Change {number}"),
            url: format!("https://github.com/Owner/Repo/pull/{number}"),
            state: state.into(),
            is_draft: false,
            author: author.map(str::to_owned),
            created_at: "2026-10-18T10:00:00Z".into(),
            updated_at: "2026-10-18T10:00:00Z".into(),
            head_ref: None,
            base_ref: None,
            additions: 0,
            deletions: 0,
            review_decision: None,
            mergeable: None,
            labels: vec![],
            comment_count: 0,
            assignees: vec![],
            review_requests: reviewers.iter().map(|login| login.to_string()).collect(),
            checks: None,
        }
    }

    #[test]
    fn pull_alerts_only_name_listed_open_pulls_by_someone_else() {
        let items = vec![
            pull(1, Some("ana"), "open", &[]),
            pull(2, Some("Me"), "open", &["me"]),
            pull(3, Some("bia"), "open", &["ME"]),
            pull(4, Some("caio"), "merged", &[]),
            pull(5, None, "open", &["me"]),
            pull(6, Some("dani"), "open", &["other"]),
        ];
        let ask = |requests: &[(&str, u32, AlertReason)]| {
            let requests: Vec<_> = requests
                .iter()
                .map(|(repo, n, reason)| (repo.to_string(), *n, *reason))
                .collect();
            confirmed_alerts(Some("me"), &items, &requests)
                .into_iter()
                .map(|alert| (alert.key, alert.reason, alert.author))
                .collect::<Vec<_>>()
        };
        use AlertReason::{Opened, ReviewRequested};
        assert_eq!(
            ask(&[
                ("owner/repo", 1, Opened),
                ("owner/repo", 2, Opened),
                ("owner/repo", 4, Opened),
                ("owner/repo", 5, ReviewRequested),
                ("owner/repo", 6, ReviewRequested),
                ("owner/repo", 99, Opened),
                ("other/repo", 1, Opened),
            ]),
            vec![("Owner/Repo#1".into(), Opened, "ana".into())]
        );
        // An opening that also asks for the viewer's review is one review alert.
        assert_eq!(
            ask(&[
                ("owner/repo", 3, Opened),
                ("owner/repo", 3, ReviewRequested)
            ]),
            vec![("Owner/Repo#3".into(), ReviewRequested, "bia".into())]
        );
        // Without a known viewer nothing qualifies.
        assert!(confirmed_alerts(None, &items, &[("owner/repo".into(), 1, Opened)]).is_empty());
    }
}
