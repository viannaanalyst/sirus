//! Read-only GitHub metadata. Renderer inputs never select endpoints or CLI flags.
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LookupStatus {
    Ready,
    NoPullRequest,
    NotGit,
    NoCommit,
    UnsupportedRemote,
    Detached,
    CliMissing,
    AuthRequired,
    Unavailable,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestSnapshot {
    pub session_id: String,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub local_head: Option<String>,
    pub status: LookupStatus,
    pub checked_at: String,
    pub pull_request: Option<PullRequest>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u32,
    pub title: String,
    pub state: String,
    pub draft: bool,
    pub url: String,
    pub head_branch: String,
    pub base_branch: String,
    pub head_sha: String,
    pub local_commit_differs: bool,
    pub checks: Vec<PullRequestCheck>,
    pub checks_complete: bool,
    pub checks_truncated: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestCheck {
    pub id: String,
    pub name: String,
    pub status: CheckStatus,
    pub url: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    Passed,
    Failed,
    Pending,
    Skipped,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Context {
    pub cwd: PathBuf,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub local_head: Option<String>,
    pub status: LookupStatus,
}

pub(crate) fn repository(web_url: &str) -> Option<String> {
    let value = web_url.strip_prefix("https://github.com/")?;
    let parts: Vec<_> = value.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty() || part.len() > 100 || part.starts_with('.') || part.contains("..")
        })
    {
        return None;
    }
    if !parts[0]
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || !parts[1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return None;
    }
    Some(value.to_owned())
}
pub(crate) fn sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
pub(crate) fn text(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}
pub(crate) fn github_link(value: Option<&str>, repo: &str) -> Option<String> {
    let value = value?;
    if value.len() > 2048 {
        return None;
    }
    let mut url = reqwest::Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url
            .path()
            .to_ascii_lowercase()
            .starts_with(&format!("/{}/", repo.to_ascii_lowercase())))
    .then(|| {
        url.set_query(None);
        url.set_fragment(None);
        url.to_string()
    })
}

fn check_link(value: Option<&str>, pr: &PullRequest) -> Option<String> {
    let repo = pr
        .url
        .strip_prefix("https://github.com/")?
        .split_once("/pull/")?
        .0;
    github_link(value, repo)
}

pub(crate) fn inspect(cwd: PathBuf) -> Result<Context> {
    let mut context = Context {
        cwd,
        repository: None,
        branch: None,
        local_head: None,
        status: LookupStatus::NotGit,
    };
    let inside = crate::git::bounded_probe(&context.cwd, &["rev-parse", "--is-inside-work-tree"])?;
    if !inside.status.success() || inside.stdout != b"true\n" {
        return Ok(context);
    }
    let branch = crate::git::bounded_probe(
        &context.cwd,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )?;
    if branch.status.success() {
        let value =
            String::from_utf8(branch.stdout).map_err(|_| Error::git("Invalid Git branch"))?;
        let value = value.trim();
        if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
            return Err(Error::git("Invalid Git branch"));
        }
        context.branch = Some(value.to_owned());
    }
    let head = crate::git::bounded_probe(&context.cwd, &["rev-parse", "--verify", "HEAD"])?;
    if !head.status.success() {
        context.status = LookupStatus::NoCommit;
        return Ok(context);
    }
    let head = String::from_utf8_lossy(&head.stdout).trim().to_owned();
    if !sha(&head) {
        return Err(Error::git("Invalid Git commit"));
    }
    context.local_head = Some(head.to_ascii_lowercase());
    if context.branch.is_none() {
        context.status = LookupStatus::Detached;
        return Ok(context);
    }
    let remote = crate::git::bounded_probe(&context.cwd, &["remote", "get-url", "origin"])?;
    if remote.status.success() {
        context.repository =
            crate::git::normalize_github_web_url(String::from_utf8_lossy(&remote.stdout).trim())
                .and_then(|url| repository(&url));
    }
    context.status = if context.repository.is_some() {
        LookupStatus::Ready
    } else {
        LookupStatus::UnsupportedRemote
    };
    Ok(context)
}

fn list_endpoint(repo: &str, branch: &str, state: &str) -> String {
    let mut url = reqwest::Url::parse("https://api.github.com").expect("fixed URL");
    url.set_path(&format!("/repos/{repo}/pulls"));
    url.query_pairs_mut()
        .append_pair("state", state)
        .append_pair(
            "head",
            &format!(
                "{}:{branch}",
                repo.split('/').next().expect("validated owner")
            ),
        )
        .append_pair("sort", "updated")
        .append_pair("direction", "desc")
        .append_pair("per_page", "1");
    url.as_str()
        .strip_prefix("https://api.github.com")
        .expect("fixed host")
        .to_owned()
}
pub(crate) fn command(binary: &OsStr, endpoint: &str) -> tokio::process::Command {
    let mut command = hardened(binary);
    command.args([
        "api",
        "--hostname",
        "github.com",
        "--method",
        "GET",
        "-H",
        "Accept: application/vnd.github+json",
        endpoint,
    ]);
    command
}

/// `gh` with no prompts, pager, browser, proxy, debug or host/repo overrides,
/// run outside any repository so local configuration cannot select a target.
pub(crate) fn hardened(binary: &OsStr) -> tokio::process::Command {
    let mut command = crate::detect::command(binary);
    command
        .current_dir(std::env::temp_dir())
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1");
    for key in [
        "GH_HOST",
        "GH_REPO",
        "GH_DEBUG",
        "DEBUG",
        "GH_PAGER",
        "PAGER",
        "GH_BROWSER",
        "BROWSER",
        "GH_FORCE_TTY",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "https_proxy",
        "http_proxy",
        "all_proxy",
    ] {
        command.env_remove(key);
    }
    command
}
pub(crate) async fn api(
    binary: &OsStr,
    endpoint: &str,
) -> std::result::Result<serde_json::Value, LookupStatus> {
    // Shared GitHub read gate: concurrency, rate-limit pause, reuse (ADR-100).
    let result = crate::gh_gate::read(command(binary, endpoint), Duration::from_secs(8)).await;
    match result {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(LookupStatus::CliMissing),
        Err(_) => Err(LookupStatus::Unavailable),
        Ok(output) if !output.status.success() => {
            // Never publish raw gh output, account details, request headers or API errors.
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(
                if stderr.contains("gh auth login") || stderr.contains("HTTP 401") {
                    LookupStatus::AuthRequired
                } else {
                    LookupStatus::Unavailable
                },
            )
        }
        Ok(output) => serde_json::from_slice(&output.stdout).map_err(|_| LookupStatus::Unavailable),
    }
}

#[derive(Deserialize)]
struct WireRepo {
    full_name: String,
}
#[derive(Deserialize)]
struct WireRef {
    #[serde(rename = "ref")]
    branch: String,
    sha: String,
    repo: Option<WireRepo>,
}
#[derive(Deserialize)]
struct WirePr {
    number: u32,
    title: String,
    state: String,
    #[serde(default)]
    draft: bool,
    merged_at: Option<String>,
    head: WireRef,
    base: WireRef,
}
fn parse_pr(
    value: serde_json::Value,
    context: &Context,
) -> std::result::Result<Option<PullRequest>, LookupStatus> {
    let mut entries: Vec<WirePr> =
        serde_json::from_value(value).map_err(|_| LookupStatus::Unavailable)?;
    if entries.len() > 1 {
        return Err(LookupStatus::Unavailable);
    }
    let Some(pr) = entries.pop() else {
        return Ok(None);
    };
    let repo = context
        .repository
        .as_deref()
        .ok_or(LookupStatus::Unavailable)?;
    if pr.number == 0
        || !sha(&pr.head.sha)
        || Some(&pr.head.branch) != context.branch.as_ref()
        || !pr
            .head
            .repo
            .as_ref()
            .is_some_and(|r| r.full_name.eq_ignore_ascii_case(repo))
        || !pr
            .base
            .repo
            .as_ref()
            .is_some_and(|r| r.full_name.eq_ignore_ascii_case(repo))
        || !matches!(pr.state.as_str(), "open" | "closed")
    {
        return Err(LookupStatus::Unavailable);
    }
    Ok(Some(PullRequest {
        number: pr.number,
        title: text(&pr.title, 240),
        state: if pr.merged_at.is_some() {
            "merged".into()
        } else {
            pr.state
        },
        draft: pr.draft,
        url: format!("https://github.com/{repo}/pull/{}", pr.number),
        head_branch: pr.head.branch,
        base_branch: text(&pr.base.branch, 512),
        local_commit_differs: context
            .local_head
            .as_ref()
            .is_none_or(|local| !local.eq_ignore_ascii_case(&pr.head.sha)),
        head_sha: pr.head.sha.to_ascii_lowercase(),
        checks: vec![],
        checks_complete: true,
        checks_truncated: false,
    }))
}

#[derive(Deserialize)]
struct Runs {
    total_count: u64,
    check_runs: Vec<Run>,
}
#[derive(Deserialize)]
struct Run {
    id: u64,
    name: String,
    status: String,
    conclusion: Option<String>,
    head_sha: String,
    html_url: Option<String>,
    details_url: Option<String>,
}
#[derive(Deserialize)]
struct Statuses {
    total_count: u64,
    sha: String,
    statuses: Vec<CommitStatus>,
}
#[derive(Deserialize)]
struct CommitStatus {
    context: String,
    state: String,
    target_url: Option<String>,
}
fn run_status(status: &str, conclusion: Option<&str>) -> CheckStatus {
    match (status, conclusion) {
        ("queued" | "in_progress" | "waiting" | "pending" | "requested", _) => CheckStatus::Pending,
        ("completed", Some("success")) => CheckStatus::Passed,
        ("completed", Some("neutral" | "skipped")) => CheckStatus::Skipped,
        (
            "completed",
            Some(
                "failure" | "cancelled" | "timed_out" | "action_required" | "stale"
                | "startup_failure",
            ),
        ) => CheckStatus::Failed,
        _ => CheckStatus::Unknown,
    }
}
fn apply_runs(pr: &mut PullRequest, value: serde_json::Value) {
    let Ok(runs) = serde_json::from_value::<Runs>(value) else {
        pr.checks_complete = false;
        return;
    };
    pr.checks_truncated |=
        runs.total_count > runs.check_runs.len() as u64 || runs.check_runs.len() > 100;
    let mut ids = std::collections::HashSet::new();
    for run in runs.check_runs.into_iter().take(100) {
        if run.id == 0 || !run.head_sha.eq_ignore_ascii_case(&pr.head_sha) || !ids.insert(run.id) {
            pr.checks_complete = false;
            continue;
        }
        pr.checks.push(PullRequestCheck {
            id: format!("run:{}", run.id),
            name: text(&run.name, 160),
            status: run_status(&run.status, run.conclusion.as_deref()),
            url: check_link(run.html_url.as_deref(), pr)
                .or_else(|| check_link(run.details_url.as_deref(), pr)),
        });
    }
}
fn apply_statuses(pr: &mut PullRequest, value: serde_json::Value) {
    let Ok(statuses) = serde_json::from_value::<Statuses>(value) else {
        pr.checks_complete = false;
        return;
    };
    if !statuses.sha.eq_ignore_ascii_case(&pr.head_sha) {
        pr.checks_complete = false;
        return;
    }
    pr.checks_truncated |=
        statuses.total_count > statuses.statuses.len() as u64 || statuses.statuses.len() > 100;
    let mut contexts = std::collections::HashSet::new();
    for status in statuses.statuses.into_iter().take(100) {
        // Combined status lists the latest context first; never revive an older result.
        if !contexts.insert(status.context.clone()) {
            continue;
        }
        pr.checks.push(PullRequestCheck {
            id: format!("status:{:x}", Sha256::digest(status.context.as_bytes())),
            name: text(&status.context, 160),
            status: match status.state.as_str() {
                "success" => CheckStatus::Passed,
                "failure" | "error" => CheckStatus::Failed,
                "pending" => CheckStatus::Pending,
                _ => CheckStatus::Unknown,
            },
            url: check_link(status.target_url.as_deref(), pr),
        });
    }
}

pub(crate) async fn load(session_id: String, context: &Context) -> PullRequestSnapshot {
    load_with_binary(session_id, context, OsStr::new("gh")).await
}
async fn load_with_binary(
    session_id: String,
    context: &Context,
    binary: &OsStr,
) -> PullRequestSnapshot {
    let mut snapshot = PullRequestSnapshot {
        session_id,
        repository: context.repository.clone(),
        branch: context.branch.clone(),
        local_head: context.local_head.clone(),
        status: context.status.clone(),
        checked_at: chrono::Utc::now().to_rfc3339(),
        pull_request: None,
    };
    if context.status != LookupStatus::Ready {
        return snapshot;
    }
    let (Some(repo), Some(branch)) = (&context.repository, &context.branch) else {
        snapshot.status = LookupStatus::Unavailable;
        return snapshot;
    };
    let lookup = async {
        for state in ["open", "closed"] {
            if let Some(mut pr) = parse_pr(
                api(binary, &list_endpoint(repo, branch, state)).await?,
                context,
            )? {
                let runs = api(
                    binary,
                    &format!(
                        "/repos/{repo}/commits/{}/check-runs?filter=latest&per_page=100",
                        pr.head_sha
                    ),
                )
                .await;
                let statuses = api(
                    binary,
                    &format!("/repos/{repo}/commits/{}/status?per_page=100", pr.head_sha),
                )
                .await;
                match runs {
                    Ok(value) => apply_runs(&mut pr, value),
                    Err(_) => pr.checks_complete = false,
                }
                match statuses {
                    Ok(value) => apply_statuses(&mut pr, value),
                    Err(_) => pr.checks_complete = false,
                }
                pr.checks_complete &= !pr.checks_truncated;
                return Ok(pr);
            }
        }
        Err(LookupStatus::NoPullRequest)
    }
    .await;
    match lookup {
        Ok(pr) => snapshot.pull_request = Some(pr),
        Err(status) => snapshot.status = status,
    }
    snapshot.checked_at = chrono::Utc::now().to_rfc3339();
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn context() -> Context {
        Context {
            cwd: std::env::temp_dir(),
            repository: Some("owner/repo".into()),
            branch: Some("feature/topic".into()),
            local_head: Some("a".repeat(40)),
            status: LookupStatus::Ready,
        }
    }
    fn wire_pr() -> serde_json::Value {
        json!([{ "number":7, "title":"<script>PR</script>", "state":"open", "draft":true, "merged_at":null,
            "head":{"ref":"feature/topic", "sha":"b".repeat(40), "repo":{"full_name":"owner/repo"}},
            "base":{"ref":"main", "sha":"a".repeat(40), "repo":{"full_name":"owner/repo"}} }])
    }
    #[test]
    fn strict_repository_and_encoded_branch_never_become_an_endpoint_or_flag() {
        assert_eq!(
            repository("https://github.com/owner/repo"),
            Some("owner/repo".into())
        );
        for raw in [
            "https://github.com/o/r/extra",
            "https://github.com/o/../secret",
            "https://github.com/o/r?token=secret",
            "https://evil.example/o/r",
            "https://github.com/o/r%2fsecret",
        ] {
            assert!(repository(raw).is_none());
        }
        let endpoint = list_endpoint("owner/repo", "feature/a&state=closed", "open");
        let url = reqwest::Url::parse(&format!("https://api.github.com{endpoint}")).unwrap();
        assert_eq!(url.host_str(), Some("api.github.com"));
        assert_eq!(
            url.query_pairs().find(|(key, _)| key == "head").unwrap().1,
            "owner:feature/a&state=closed"
        );
        assert_eq!(
            url.query_pairs().filter(|(key, _)| key == "state").count(),
            1
        );
        let command = command(OsStr::new("gh"), &endpoint);
        let argv: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            &argv[..5],
            &["api", "--hostname", "github.com", "--method", "GET"]
        );
        assert_eq!(argv.last().unwrap(), &endpoint);
        assert_eq!(
            command.as_std().get_current_dir(),
            Some(std::env::temp_dir().as_path())
        );
        assert!(command
            .as_std()
            .get_envs()
            .any(|(key, value)| key == "GH_DEBUG" && value.is_none()));
    }
    #[test]
    fn pr_identity_and_check_commit_ownership_are_validated() {
        let mut pr = parse_pr(wire_pr(), &context()).unwrap().unwrap();
        assert!(pr.local_commit_differs);
        assert_eq!(pr.url, "https://github.com/owner/repo/pull/7");
        for (field, value) in [
            ("ref", json!("foreign")),
            ("sha", json!("../../secret")),
            ("repo", json!({"full_name":"foreign/repo"})),
        ] {
            let mut forged = wire_pr();
            forged[0]["head"][field] = value;
            assert!(parse_pr(forged, &context()).is_err());
        }
        apply_runs(
            &mut pr,
            json!({"total_count":1,"check_runs":[{"id":1,"name":"foreign","status":"completed","conclusion":"success","head_sha":"c".repeat(40),"html_url":null,"details_url":null}]}),
        );
        assert!(!pr.checks_complete);
        assert!(pr.checks.is_empty());
        assert!(github_link(Some("https://github.com@evil.example/run"), "owner/repo").is_none());
        assert!(github_link(Some("https://user:secret@github.com/o/r"), "owner/repo").is_none());
        assert!(github_link(Some("javascript:alert(1)"), "owner/repo").is_none());
        assert!(github_link(
            Some("https://github.com/login/oauth/authorize"),
            "owner/repo"
        )
        .is_none());
        assert_eq!(
            github_link(
                Some("https://github.com/owner/repo/runs/4?token=secret#step-1"),
                "owner/repo"
            ),
            Some("https://github.com/owner/repo/runs/4".into())
        );
        assert_eq!(run_status("completed", None), CheckStatus::Unknown);
        assert_eq!(
            run_status("completed", Some("cancelled")),
            CheckStatus::Failed
        );
        assert_eq!(run_status("queued", None), CheckStatus::Pending);
    }
    #[test]
    fn partial_pages_unknown_states_and_latest_contexts_remain_explicit() {
        let mut pr = parse_pr(wire_pr(), &context()).unwrap().unwrap();
        apply_runs(&mut pr, json!({"total_count":101,"check_runs":[]}));
        assert!(pr.checks_truncated);
        apply_statuses(
            &mut pr,
            json!({"total_count":2,"sha":"b".repeat(40),"statuses":[
            {"context":"ci","state":"pending","target_url":"https://ci.example/run"},
            {"context":"ci","state":"success","target_url":null}]}),
        );
        assert_eq!(pr.checks.len(), 1);
        assert_eq!(pr.checks[0].status, CheckStatus::Pending);
        assert!(pr.checks[0].url.is_none());
        apply_statuses(
            &mut pr,
            json!({"total_count":0,"sha":"c".repeat(40),"statuses":[]}),
        );
        assert!(!pr.checks_complete);
    }
    #[test]
    fn git_context_uses_actual_branch_head_and_safe_origin_without_network() {
        let repo = crate::git::tests::Repo::new();
        crate::git::run_ok(
            &repo.cwd(),
            &["remote", "add", "origin", "git@github.com:owner/repo.git"],
        )
        .unwrap();
        let initial = inspect(repo.cwd()).unwrap();
        assert_eq!(initial.repository.as_deref(), Some("owner/repo"));
        crate::git::run_ok(&repo.cwd(), &["checkout", "-b", "feature/topic"]).unwrap();
        let changed = inspect(repo.cwd()).unwrap();
        assert_eq!(changed.branch.as_deref(), Some("feature/topic"));
        assert_ne!(initial, changed);
        crate::git::run_ok(&repo.cwd(), &["checkout", "--detach"]).unwrap();
        assert_eq!(inspect(repo.cwd()).unwrap().status, LookupStatus::Detached);
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            inspect(empty.path().to_path_buf()).unwrap().status,
            LookupStatus::NotGit
        );
    }
    #[cfg(unix)]
    fn fake_gh(script: &str) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("gh");
        std::fs::write(&binary, format!("#!/bin/sh\n{script}")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir, binary)
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn read_only_cli_fixture_loads_pr_runs_statuses_and_sanitizes_failures() {
        let (dir, binary) = fake_gh("case \"$8\" in *'/pulls?'*) /bin/cat \"${0%/*}/pr.json\";; *'/check-runs?'*) /bin/cat \"${0%/*}/runs.json\";; *'/status?'*) /bin/cat \"${0%/*}/status.json\";; *) exit 1;; esac\n");
        std::fs::write(dir.path().join("pr.json"), wire_pr().to_string()).unwrap();
        std::fs::write(dir.path().join("runs.json"), json!({"total_count":1,"check_runs":[{"id":4,"name":"build","status":"completed","conclusion":"success","head_sha":"b".repeat(40),"html_url":"https://github.com/owner/repo/runs/4","details_url":null}]}).to_string()).unwrap();
        std::fs::write(dir.path().join("status.json"), json!({"total_count":1,"sha":"b".repeat(40),"statuses":[{"context":"lint","state":"failure","target_url":null}]}).to_string()).unwrap();
        let snapshot = load_with_binary("s".into(), &context(), binary.as_os_str()).await;
        assert_eq!(snapshot.status, LookupStatus::Ready);
        let pr = snapshot.pull_request.unwrap();
        assert!(pr.checks_complete);
        assert_eq!(pr.checks.len(), 2);
        assert_eq!(pr.checks[1].status, CheckStatus::Failed);
        std::fs::write(dir.path().join("runs.json"), "bad JSON").unwrap();
        assert!(
            !load_with_binary("s".into(), &context(), binary.as_os_str())
                .await
                .pull_request
                .unwrap()
                .checks_complete
        );
        let (_auth_dir, auth) = fake_gh("printf '%s' 'secret-token: gh auth login' >&2; exit 1\n");
        let denied = load_with_binary("s".into(), &context(), auth.as_os_str()).await;
        assert_eq!(denied.status, LookupStatus::AuthRequired);
        assert!(!serde_json::to_string(&denied)
            .unwrap()
            .contains("secret-token"));
        let missing = load_with_binary(
            "s".into(),
            &context(),
            OsStr::new("/not-an-installed-gh-fixture"),
        )
        .await;
        assert_eq!(missing.status, LookupStatus::CliMissing);
        std::fs::write(dir.path().join("pr.json"), "[]").unwrap();
        assert_eq!(
            load_with_binary("s".into(), &context(), binary.as_os_str())
                .await
                .status,
            LookupStatus::NoPullRequest
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closed_fallback_keeps_merged_state_and_partial_checks_explicit() {
        let (dir, binary) = fake_gh("case \"$8\" in *'state=open'*) printf '[]';; *'state=closed'*) /bin/cat \"${0%/*}/merged.json\";; *) printf '%s' 'private-error-secret' >&2; exit 1;; esac\n");
        let mut merged = wire_pr();
        merged[0]["state"] = json!("closed");
        merged[0]["merged_at"] = json!("2026-10-02T12:00:00Z");
        std::fs::write(dir.path().join("merged.json"), merged.to_string()).unwrap();
        let result = load_with_binary("s".into(), &context(), binary.as_os_str()).await;
        assert_eq!(result.status, LookupStatus::Ready);
        let pr = result.pull_request.unwrap();
        assert_eq!(pr.state, "merged");
        assert!(!pr.checks_complete);
        assert!(pr.checks.is_empty());
        assert!(!serde_json::to_string(&pr)
            .unwrap()
            .contains("private-error-secret"));
        let mut not_git = context();
        not_git.status = LookupStatus::NotGit;
        assert_eq!(
            load_with_binary("s".into(), &not_git, OsStr::new("/missing-gh-fixture"))
                .await
                .status,
            LookupStatus::NotGit
        );
    }
}
