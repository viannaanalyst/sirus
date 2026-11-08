//! GitLab and Bitbucket beside GitHub: which service a remote belongs to, the merge request or
//! pull request of the session's branch with its checks, and opening one. GitLab goes through
//! the `glab` CLI (its sign-in, its hosts); Bitbucket through its REST API with a token kept in
//! the Keychain, or the credentials Git already uses for bitbucket.org. GitHub stays in
//! `pull_requests` and `github_inbox`; nothing here runs for a GitHub remote.
use std::ffi::OsStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::pull_requests::{
    text, CheckStatus, Context, LookupStatus, PullRequest, PullRequestCheck, PullRequestSnapshot,
};

const BITBUCKET_API: &str = "https://api.bitbucket.org/2.0";
const BITBUCKET_SECRET: &str = "forge:bitbucket";
const MAX_CHECKS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Forge {
    Github,
    Gitlab,
    Bitbucket,
}

impl Forge {
    pub fn id(self) -> &'static str {
        match self {
            Forge::Github => "github",
            Forge::Gitlab => "gitlab",
            Forge::Bitbucket => "bitbucket",
        }
    }
}

/// A remote's service, its host (no port) and the repository path on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub forge: Forge,
    pub host: String,
    pub path: String,
}

impl Remote {
    fn web(&self) -> String {
        format!("https://{}/{}", self.host, self.path)
    }
}

/// `host` and `path` of a Git remote URL: `git@host:path`, `ssh://user@host[:port]/path`,
/// `https://[user@]host[:port]/path`. Credentials, queries and odd characters give `None`.
fn split_remote(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim();
    if raw.len() > 1024 || raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let (host, path) = if let Some(rest) = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
        .or_else(|| raw.strip_prefix("ssh://"))
        .or_else(|| raw.strip_prefix("git+ssh://"))
    {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        if authority.matches('@').count() > 1 || authority.split('@').next()?.contains(':') {
            // `user:password@host` never becomes a target.
            return None;
        }
        (host.split(':').next()?.to_owned(), path.to_owned())
    } else {
        // scp-like `user@host:path`.
        let (authority, path) = raw.split_once(':')?;
        let host = authority.rsplit('@').next()?;
        if path.starts_with("//") {
            return None;
        }
        (host.to_owned(), path.trim_start_matches('/').to_owned())
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty()
        || host.len() > 253
        || host.starts_with('.')
        || host.starts_with('-')
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
    {
        return None;
    }
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty()
        || path.len() > 512
        || path.contains('?')
        || path.contains('#')
        || path.split('/').any(|segment| {
            segment.is_empty()
                || segment.starts_with('.')
                || segment.starts_with('-')
                || segment.contains("..")
                || !segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
    {
        return None;
    }
    Some((host, path.to_owned()))
}

/// The service a remote belongs to. GitLab is gitlab.com, any host named like GitLab, or a host
/// `glab` is configured for; Bitbucket is bitbucket.org (Bitbucket Server has another API).
pub fn parse_remote(raw: &str, gitlab_hosts: &[String]) -> Option<Remote> {
    let (host, path) = split_remote(raw)?;
    let segments = path.split('/').count();
    let forge = if host == "github.com" {
        Forge::Github
    } else if host == "bitbucket.org" {
        Forge::Bitbucket
    } else if host == "gitlab.com"
        || host.split('.').any(|label| label == "gitlab")
        || gitlab_hosts
            .iter()
            .any(|known| known.eq_ignore_ascii_case(&host))
    {
        Forge::Gitlab
    } else {
        return None;
    };
    let fits = match forge {
        Forge::Gitlab => (2..=20).contains(&segments),
        _ => segments == 2,
    };
    fits.then_some(Remote { forge, host, path })
}

/// The hosts under `hosts:` of glab's `config.yml` (two-space indented keys).
pub fn glab_hosts_from(config: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    let mut inside = false;
    let mut level = None;
    for line in config.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            inside = line.trim_end() == "hosts:";
            continue;
        }
        if inside && *level.get_or_insert(indent) == indent {
            if let Some(key) = line.trim().strip_suffix(':') {
                let key = key.trim_matches(|c| c == '"' || c == '\'');
                if !key.is_empty()
                    && key
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
                    && !hosts.iter().any(|h: &String| h.eq_ignore_ascii_case(key))
                {
                    hosts.push(key.to_ascii_lowercase());
                }
            }
        }
    }
    hosts.truncate(32);
    hosts
}

/// Hosts the person signed `glab` in to, from its config file.
fn glab_hosts() -> Vec<String> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("GLAB_CONFIG_DIR") {
        dirs.push(std::path::PathBuf::from(dir));
    }
    if let Some(home) = dirs_home() {
        dirs.push(home.join(".config/glab-cli"));
        dirs.push(home.join("Library/Application Support/glab-cli"));
    }
    for dir in dirs {
        if let Ok(text) = std::fs::read_to_string(dir.join("config.yml")) {
            let hosts = glab_hosts_from(&text);
            if !hosts.is_empty() {
                return hosts;
            }
        }
    }
    Vec::new()
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// The non-GitHub remote of `origin`, for `pull_requests::inspect`.
pub fn remote_of(raw: &str) -> Option<Remote> {
    parse_remote(raw, &glab_hosts()).filter(|remote| remote.forge != Forge::Github)
}

// ---------------------------------------------------------------------------------------------
// GitLab through glab
// ---------------------------------------------------------------------------------------------

/// `glab` with no prompts, pager, browser, proxy or host override, outside any repository.
fn glab(binary: &OsStr) -> tokio::process::Command {
    let mut command = crate::detect::command(binary);
    command
        .current_dir(std::env::temp_dir())
        .env("NO_PROMPT", "1")
        .env("GLAB_NO_PROMPT", "1")
        .env("GLAB_CHECK_UPDATE", "false")
        .env("NO_COLOR", "1");
    for key in [
        "GITLAB_HOST",
        "GL_HOST",
        "GITLAB_URI",
        "GLAB_PAGER",
        "PAGER",
        "BROWSER",
        "GLAB_DEBUG",
        "DEBUG",
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

fn encoded(path: &str) -> String {
    path.replace('/', "%2F")
}

fn glab_failure(stderr: &str) -> LookupStatus {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("401")
        || lower.contains("glab auth login")
        || lower.contains("unauthorized")
        || lower.contains("not authenticated")
        || lower.contains("no token")
    {
        LookupStatus::AuthRequired
    } else {
        LookupStatus::Unavailable
    }
}

/// One `glab api` call; `fields` become `-f key=value` (bodies, never flags).
async fn glab_api(
    binary: &OsStr,
    host: &str,
    method: &str,
    endpoint: &str,
    fields: &[(&str, &str)],
) -> std::result::Result<Value, (LookupStatus, String)> {
    let mut command = glab(binary);
    command.args(["api", "--hostname", host, "--method", method, endpoint]);
    for (key, value) in fields {
        command.arg("-f").arg(format!("{key}={value}"));
    }
    let output = crate::cli_output::capture_command(command, Duration::from_secs(20))
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                (LookupStatus::CliMissing, String::new())
            } else {
                (LookupStatus::Unavailable, String::new())
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let said = format!("{stderr}\n{stdout}");
        return Err((glab_failure(&said), said));
    }
    serde_json::from_slice(&output.stdout).map_err(|_| (LookupStatus::Unavailable, String::new()))
}

fn same_host(url: &str, host: &str) -> Option<String> {
    if url.len() > 2048 {
        return None;
    }
    let mut parsed = reqwest::Url::parse(url).ok()?;
    (parsed.scheme() == "https"
        && parsed.host_str() == Some(host)
        && parsed.username().is_empty()
        && parsed.password().is_none())
    .then(|| {
        parsed.set_query(None);
        parsed.set_fragment(None);
        parsed.to_string()
    })
}

fn hex(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The merge request of a GitLab list reply, checked against the session's branch.
fn parse_gitlab_mr(value: &Value, remote: &Remote, context: &Context) -> Option<PullRequest> {
    let mr = value.as_array()?.first()?;
    let number = u32::try_from(mr["iid"].as_u64()?).ok()?;
    let head_branch = mr["source_branch"].as_str()?;
    let head_sha = mr["sha"].as_str().unwrap_or_default();
    if number == 0 || Some(head_branch) != context.branch.as_deref() || !hex(head_sha) {
        return None;
    }
    let state = match mr["state"].as_str()? {
        "opened" | "locked" => "open",
        "closed" => "closed",
        "merged" => "merged",
        _ => return None,
    };
    let url = same_host(mr["web_url"].as_str()?, &remote.host)?;
    Some(PullRequest {
        number,
        title: text(mr["title"].as_str().unwrap_or_default(), 240),
        state: state.into(),
        draft: mr["draft"].as_bool().unwrap_or(false)
            || mr["work_in_progress"].as_bool().unwrap_or(false),
        url,
        head_branch: head_branch.to_owned(),
        base_branch: text(mr["target_branch"].as_str().unwrap_or_default(), 512),
        local_commit_differs: context
            .local_head
            .as_ref()
            .is_none_or(|local| !local.eq_ignore_ascii_case(head_sha)),
        head_sha: head_sha.to_ascii_lowercase(),
        checks: vec![],
        checks_complete: true,
        checks_truncated: false,
    })
}

fn gitlab_job_status(status: &str, allow_failure: bool) -> CheckStatus {
    match status {
        "success" => CheckStatus::Passed,
        "failed" if allow_failure => CheckStatus::Skipped,
        "failed" | "canceled" => CheckStatus::Failed,
        "skipped" | "manual" => CheckStatus::Skipped,
        "created"
        | "pending"
        | "running"
        | "preparing"
        | "scheduled"
        | "waiting_for_resource"
        | "waiting_for_callback" => CheckStatus::Pending,
        _ => CheckStatus::Unknown,
    }
}

/// A pipeline's jobs as checks; a retried job keeps only its latest run (the list is newest first).
fn apply_gitlab_jobs(pr: &mut PullRequest, jobs: &Value, host: &str) {
    let Some(jobs) = jobs.as_array() else {
        pr.checks_complete = false;
        return;
    };
    pr.checks_truncated |= jobs.len() >= MAX_CHECKS;
    let mut names = std::collections::HashSet::new();
    for job in jobs.iter().take(MAX_CHECKS) {
        let (Some(id), Some(name)) = (job["id"].as_u64(), job["name"].as_str()) else {
            pr.checks_complete = false;
            continue;
        };
        if !names.insert(name.to_owned()) {
            continue;
        }
        pr.checks.push(PullRequestCheck {
            id: format!("job:{id}"),
            name: text(name, 160),
            status: gitlab_job_status(
                job["status"].as_str().unwrap_or_default(),
                job["allow_failure"].as_bool().unwrap_or(false),
            ),
            url: job["web_url"].as_str().and_then(|url| same_host(url, host)),
        });
    }
}

async fn load_gitlab(
    binary: &OsStr,
    remote: &Remote,
    context: &Context,
) -> std::result::Result<PullRequest, LookupStatus> {
    let branch = context.branch.as_deref().ok_or(LookupStatus::Unavailable)?;
    let project = encoded(&remote.path);
    let query = |state: &str| {
        let mut url = reqwest::Url::parse("https://x/").expect("fixed URL");
        url.set_path(&format!("/projects/{project}/merge_requests"));
        url.query_pairs_mut()
            .append_pair("source_branch", branch)
            .append_pair("state", state)
            .append_pair("order_by", "updated_at")
            .append_pair("sort", "desc")
            .append_pair("per_page", "1");
        // `set_path` would decode the `%2F` of the project; put it back as GitLab wants.
        format!(
            "projects/{project}/merge_requests?{}",
            url.query().unwrap_or_default()
        )
    };
    for state in ["opened", "all"] {
        let list = glab_api(binary, &remote.host, "GET", &query(state), &[])
            .await
            .map_err(|(status, _)| status)?;
        let Some(mut pr) = parse_gitlab_mr(&list, remote, context) else {
            continue;
        };
        let detail = glab_api(
            binary,
            &remote.host,
            "GET",
            &format!("projects/{project}/merge_requests/{}", pr.number),
            &[],
        )
        .await;
        let pipeline = detail
            .ok()
            .and_then(|detail| detail["head_pipeline"]["id"].as_u64());
        if let Some(pipeline) = pipeline {
            match glab_api(
                binary,
                &remote.host,
                "GET",
                &format!("projects/{project}/pipelines/{pipeline}/jobs?per_page=100"),
                &[],
            )
            .await
            {
                Ok(jobs) => apply_gitlab_jobs(&mut pr, &jobs, &remote.host),
                Err(_) => pr.checks_complete = false,
            }
        }
        pr.checks_complete &= !pr.checks_truncated;
        return Ok(pr);
    }
    Err(LookupStatus::NoPullRequest)
}

// ---------------------------------------------------------------------------------------------
// Bitbucket through its REST API
// ---------------------------------------------------------------------------------------------

/// How a Bitbucket request signs in: `email:token` (an Atlassian API token) as Basic, a bare
/// access token as Bearer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Auth {
    Basic(String, String),
    Bearer(String),
}

fn auth_from(saved: &str) -> Option<Auth> {
    let saved = saved.trim();
    if saved.is_empty() || saved.len() > 4096 || saved.chars().any(char::is_control) {
        return None;
    }
    match saved.split_once(':') {
        Some((user, secret)) if !user.is_empty() && !secret.is_empty() => {
            Some(Auth::Basic(user.to_owned(), secret.to_owned()))
        }
        Some(_) => None,
        None => Some(Auth::Bearer(saved.to_owned())),
    }
}

/// The username and password `git credential fill` knows for bitbucket.org, never prompting.
async fn git_credentials() -> Option<Auth> {
    use tokio::io::AsyncWriteExt;
    let mut command = tokio::process::Command::new("git");
    command
        .args(["-c", "credential.interactive=never", "credential", "fill"])
        .current_dir(std::env::temp_dir())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    stdin
        .write_all(b"protocol=https\nhost=bitbucket.org\n\n")
        .await
        .ok()?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let reply = String::from_utf8_lossy(&output.stdout);
    let field = |key: &str| {
        reply
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(str::to_owned)
    };
    Some(Auth::Basic(field("username")?, field("password")?))
}

async fn bitbucket_auth() -> Option<Auth> {
    if let Some(auth) = crate::connectors::secret_get(BITBUCKET_SECRET)
        .as_deref()
        .and_then(auth_from)
    {
        return Some(auth);
    }
    git_credentials().await
}

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(15))
        .user_agent("Sirus")
        .build()
        .map_err(|_| {
            Error::new(
                "bitbucket_unavailable",
                "Bitbucket did not answer. Try again.",
            )
        })
}

async fn bitbucket(
    auth: &Auth,
    method: reqwest::Method,
    url: &str,
    body: Option<Value>,
) -> std::result::Result<Value, (LookupStatus, u16, String)> {
    let client = http().map_err(|_| (LookupStatus::Unavailable, 0, String::new()))?;
    let mut request = client
        .request(method, url)
        .header("Accept", "application/json");
    request = match auth {
        Auth::Basic(user, secret) => request.basic_auth(user, Some(secret)),
        Auth::Bearer(token) => request.bearer_auth(token),
    };
    if let Some(body) = body {
        request = request
            .header("Content-Type", "application/json")
            .body(body.to_string());
    }
    let response = request
        .send()
        .await
        .map_err(|_| (LookupStatus::Unavailable, 0, String::new()))?;
    let status = response.status().as_u16();
    let bytes = response
        .bytes()
        .await
        .map_err(|_| (LookupStatus::Unavailable, status, String::new()))?;
    if !(200..300).contains(&status) {
        let said = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or_default();
        let kind = if status == 401 {
            LookupStatus::AuthRequired
        } else {
            LookupStatus::Unavailable
        };
        return Err((kind, status, said));
    }
    serde_json::from_slice(&bytes).map_err(|_| (LookupStatus::Unavailable, status, String::new()))
}

fn bitbucket_repo_url(remote: &Remote) -> String {
    let mut url = reqwest::Url::parse(BITBUCKET_API).expect("fixed URL");
    {
        let mut segments = url.path_segments_mut().expect("base URL");
        segments.push("repositories");
        for part in remote.path.split('/') {
            segments.push(part);
        }
    }
    url.to_string()
}

/// The pull request of a Bitbucket list reply, checked against the session's branch.
fn parse_bitbucket_pr(value: &Value, context: &Context) -> Option<PullRequest> {
    let pr = value["values"].as_array()?.first()?;
    let number = u32::try_from(pr["id"].as_u64()?).ok()?;
    let head_branch = pr["source"]["branch"]["name"].as_str()?;
    let head_sha = pr["source"]["commit"]["hash"].as_str().unwrap_or_default();
    if number == 0 || Some(head_branch) != context.branch.as_deref() || !hex(head_sha) {
        return None;
    }
    let state = match pr["state"].as_str()? {
        "OPEN" => "open",
        "MERGED" => "merged",
        "DECLINED" | "SUPERSEDED" => "closed",
        _ => return None,
    };
    let url = same_host(pr["links"]["html"]["href"].as_str()?, "bitbucket.org")?;
    // Bitbucket gives a short hash; the local commit is the same when it starts with it.
    let local = context.local_head.as_deref();
    let matches = local.is_some_and(|local| {
        local
            .to_ascii_lowercase()
            .starts_with(&head_sha.to_ascii_lowercase())
    });
    Some(PullRequest {
        number,
        title: text(pr["title"].as_str().unwrap_or_default(), 240),
        state: state.into(),
        draft: pr["draft"].as_bool().unwrap_or(false),
        url,
        head_branch: head_branch.to_owned(),
        base_branch: text(
            pr["destination"]["branch"]["name"]
                .as_str()
                .unwrap_or_default(),
            512,
        ),
        local_commit_differs: !matches,
        head_sha: if matches {
            local.unwrap_or(head_sha).to_ascii_lowercase()
        } else {
            head_sha.to_ascii_lowercase()
        },
        checks: vec![],
        checks_complete: true,
        checks_truncated: false,
    })
}

fn apply_bitbucket_statuses(pr: &mut PullRequest, value: &Value) {
    let Some(statuses) = value["values"].as_array() else {
        pr.checks_complete = false;
        return;
    };
    pr.checks_truncated |= value["next"].is_string() || statuses.len() > MAX_CHECKS;
    let mut keys = std::collections::HashSet::new();
    for status in statuses.iter().take(MAX_CHECKS) {
        let key = status["key"].as_str().unwrap_or_default();
        if key.is_empty() || !keys.insert(key.to_owned()) {
            continue;
        }
        let name = status["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .unwrap_or(key);
        pr.checks.push(PullRequestCheck {
            id: format!("status:{}", text(key, 120)),
            name: text(name, 160),
            status: match status["state"].as_str().unwrap_or_default() {
                "SUCCESSFUL" => CheckStatus::Passed,
                "FAILED" | "STOPPED" => CheckStatus::Failed,
                "INPROGRESS" => CheckStatus::Pending,
                _ => CheckStatus::Unknown,
            },
            url: status["url"].as_str().and_then(|url| {
                same_host(url, "bitbucket.org").or_else(|| {
                    reqwest::Url::parse(url)
                        .ok()
                        .filter(|parsed| parsed.scheme() == "https" && parsed.password().is_none())
                        .map(|parsed| parsed.to_string())
                })
            }),
        });
    }
}

async fn load_bitbucket(
    remote: &Remote,
    context: &Context,
) -> std::result::Result<PullRequest, LookupStatus> {
    let branch = context.branch.as_deref().ok_or(LookupStatus::Unavailable)?;
    if branch.contains('"') || branch.contains('\\') {
        return Err(LookupStatus::Unavailable);
    }
    let auth = bitbucket_auth().await.ok_or(LookupStatus::AuthRequired)?;
    let base = bitbucket_repo_url(remote);
    for state in ["OPEN", "ALL"] {
        let mut url = reqwest::Url::parse(&format!("{base}/pullrequests")).expect("built URL");
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("q", &format!("source.branch.name=\"{branch}\""))
                .append_pair("sort", "-updated_on")
                .append_pair("pagelen", "1");
            if state == "ALL" {
                for each in ["OPEN", "MERGED", "DECLINED", "SUPERSEDED"] {
                    query.append_pair("state", each);
                }
            } else {
                query.append_pair("state", state);
            }
        }
        let list = bitbucket(&auth, reqwest::Method::GET, url.as_str(), None)
            .await
            .map_err(|(status, _, _)| status)?;
        let Some(mut pr) = parse_bitbucket_pr(&list, context) else {
            continue;
        };
        let statuses = bitbucket(
            &auth,
            reqwest::Method::GET,
            &format!("{base}/commit/{}/statuses?pagelen=100", pr.head_sha),
            None,
        )
        .await;
        match statuses {
            Ok(value) => apply_bitbucket_statuses(&mut pr, &value),
            Err(_) => pr.checks_complete = false,
        }
        pr.checks_complete &= !pr.checks_truncated;
        return Ok(pr);
    }
    Err(LookupStatus::NoPullRequest)
}

// ---------------------------------------------------------------------------------------------
// The session's request, both services
// ---------------------------------------------------------------------------------------------

/// The snapshot of a GitLab or Bitbucket session, as `pull_requests::load` gives GitHub's.
pub async fn load(session_id: String, context: &Context, remote: &Remote) -> PullRequestSnapshot {
    let mut snapshot = PullRequestSnapshot {
        session_id,
        forge: remote.forge.id(),
        repository: Some(remote.path.clone()),
        branch: context.branch.clone(),
        local_head: context.local_head.clone(),
        status: LookupStatus::Ready,
        checked_at: chrono::Utc::now().to_rfc3339(),
        pull_request: None,
    };
    let lookup = match remote.forge {
        Forge::Gitlab => load_gitlab(OsStr::new("glab"), remote, context).await,
        Forge::Bitbucket => load_bitbucket(remote, context).await,
        Forge::Github => Err(LookupStatus::UnsupportedRemote),
    };
    match lookup {
        Ok(pr) => snapshot.pull_request = Some(pr),
        Err(status) => snapshot.status = status,
    }
    snapshot.checked_at = chrono::Utc::now().to_rfc3339();
    snapshot
}

fn gitlab_error(status: LookupStatus, said: &str) -> Error {
    match status {
        LookupStatus::CliMissing => Error::new(
            "glab_missing",
            "The GitLab CLI (glab) is not installed. Install it with `brew install glab` and run `glab auth login`.",
        ),
        LookupStatus::AuthRequired => Error::new(
            "glab_auth",
            "GitLab CLI is not signed in. Run `glab auth login` in a terminal.",
        ),
        _ if said.contains("already exists") => Error::new(
            "gitlab_conflict",
            "A merge request already exists for this branch.",
        ),
        _ if said.contains("403") || said.contains("404") => Error::new(
            "gitlab_forbidden",
            "GitLab refused: this account cannot open merge requests in this project.",
        ),
        _ => Error::new("gitlab_unavailable", "GitLab did not answer. Try again."),
    }
}

fn bitbucket_error(status: LookupStatus, code: u16, said: &str) -> Error {
    match status {
        LookupStatus::AuthRequired => Error::new(
            "bitbucket_auth",
            "Bitbucket needs a sign-in. Add a token in Settings → Git.",
        ),
        _ if said.to_ascii_lowercase().contains("already") => Error::new(
            "bitbucket_conflict",
            "A pull request already exists for this branch.",
        ),
        _ if code == 403 || code == 404 => Error::new(
            "bitbucket_forbidden",
            "Bitbucket refused: this account cannot open pull requests in this repository.",
        ),
        _ => Error::new(
            "bitbucket_unavailable",
            "Bitbucket did not answer. Try again.",
        ),
    }
}

/// The address of a created request, checked to be on the service's own host.
fn created(url: Option<&str>, host: &str, service: &str) -> Result<String> {
    url.and_then(|url| same_host(url, host)).ok_or_else(|| {
        Error::new(
            "forge_unavailable",
            format!("{service} did not return the new request."),
        )
    })
}

/// Opens a merge request (GitLab) or pull request (Bitbucket) from `head` into the default branch.
/// The branch is already pushed.
pub async fn create(
    remote: &Remote,
    head: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<String> {
    match remote.forge {
        Forge::Gitlab => {
            let binary = OsStr::new("glab");
            let project = encoded(&remote.path);
            let info = glab_api(
                binary,
                &remote.host,
                "GET",
                &format!("projects/{project}"),
                &[],
            )
            .await
            .map_err(|(status, said)| gitlab_error(status, &said))?;
            let base = info["default_branch"].as_str().unwrap_or_default();
            if base.is_empty() || base == head {
                return Err(Error::new(
                    "invalid",
                    "This branch is the repository's default branch.",
                ));
            }
            let title = if draft && !title.starts_with("Draft:") {
                format!("Draft: {title}")
            } else {
                title.to_owned()
            };
            let made = glab_api(
                binary,
                &remote.host,
                "POST",
                &format!("projects/{project}/merge_requests"),
                &[
                    ("source_branch", head),
                    ("target_branch", base),
                    ("title", &title),
                    ("description", body),
                ],
            )
            .await
            .map_err(|(status, said)| gitlab_error(status, &said))?;
            created(made["web_url"].as_str(), &remote.host, "GitLab")
        }
        Forge::Bitbucket => {
            let auth = bitbucket_auth()
                .await
                .ok_or_else(|| bitbucket_error(LookupStatus::AuthRequired, 401, ""))?;
            let base = bitbucket_repo_url(remote);
            let info = bitbucket(&auth, reqwest::Method::GET, &base, None)
                .await
                .map_err(|(status, code, said)| bitbucket_error(status, code, &said))?;
            let main = info["mainbranch"]["name"].as_str().unwrap_or_default();
            if main.is_empty() || main == head {
                return Err(Error::new(
                    "invalid",
                    "This branch is the repository's default branch.",
                ));
            }
            let made = bitbucket(
                &auth,
                reqwest::Method::POST,
                &format!("{base}/pullrequests"),
                Some(json!({
                    "title": title,
                    "description": body,
                    "source": {"branch": {"name": head}},
                    "destination": {"branch": {"name": main}},
                    "draft": draft,
                })),
            )
            .await
            .map_err(|(status, code, said)| bitbucket_error(status, code, &said))?;
            created(
                made["links"]["html"]["href"].as_str(),
                "bitbucket.org",
                "Bitbucket",
            )
        }
        Forge::Github => Err(Error::new("invalid", "GitHub uses its own path.")),
    }
}

// ---------------------------------------------------------------------------------------------
// Settings → Git
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    /// Whether glab is installed and signed in, and whether Bitbucket has a token.
    Status,
    /// Keeps a Bitbucket token (`email:api-token`, or an access token) in the Keychain.
    SetBitbucketToken { token: String },
    /// Forgets the Bitbucket token.
    ClearBitbucketToken,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// `missing`, `signedOut` or `signedIn`.
    pub gitlab: &'static str,
    /// The hosts glab is configured for.
    pub gitlab_hosts: Vec<String>,
    pub bitbucket_token: bool,
}

async fn status() -> Status {
    let output = {
        let mut command = glab(OsStr::new("glab"));
        command.args(["auth", "status"]);
        crate::cli_output::capture_command(command, Duration::from_secs(10)).await
    };
    let gitlab = match output {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing",
        Ok(output) if output.status.success() => "signedIn",
        _ => "signedOut",
    };
    Status {
        gitlab,
        gitlab_hosts: glab_hosts(),
        bitbucket_token: crate::connectors::secret_get(BITBUCKET_SECRET).is_some(),
    }
}

pub async fn forge_action(action: Action) -> Result<Status> {
    match action {
        Action::Status => {}
        Action::SetBitbucketToken { token } => {
            let token = token.trim().trim_start_matches("Bearer ").trim().to_owned();
            if auth_from(&token).is_none() || token.chars().any(char::is_whitespace) {
                return Err(Error::new(
                    "invalid",
                    "Paste the token without spaces, as email:token or an access token.",
                ));
            }
            if !crate::connectors::secret_put(BITBUCKET_SECRET, &token) {
                return Err(Error::new(
                    "bitbucket_auth",
                    "The Bitbucket token could not be saved.",
                ));
            }
        }
        Action::ClearBitbucketToken => crate::connectors::secret_delete(BITBUCKET_SECRET),
    }
    Ok(status().await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context {
            cwd: std::env::temp_dir(),
            repository: None,
            branch: Some("feature/topic".into()),
            local_head: Some(format!("abcdef123456{}", "0".repeat(28))),
            status: LookupStatus::UnsupportedRemote,
            forge: None,
        }
    }

    #[test]
    fn remotes_name_their_service() {
        let none: Vec<String> = vec![];
        let gl = parse_remote("git@gitlab.com:group/sub/project.git", &none).unwrap();
        assert_eq!(gl.forge, Forge::Gitlab);
        assert_eq!(gl.host, "gitlab.com");
        assert_eq!(gl.path, "group/sub/project");
        let bb = parse_remote("https://me@bitbucket.org/team/repo.git", &none).unwrap();
        assert_eq!(bb.forge, Forge::Bitbucket);
        assert_eq!(bb.path, "team/repo");
        assert_eq!(
            parse_remote("ssh://git@github.com/o/r.git", &none)
                .unwrap()
                .forge,
            Forge::Github
        );
        let own = parse_remote("ssh://git@code.example.com:2222/a/b.git", &none);
        assert!(own.is_none());
        let known = vec!["code.example.com".to_owned()];
        let own = parse_remote("ssh://git@code.example.com:2222/a/b.git", &known).unwrap();
        assert_eq!(
            (own.forge, own.host.as_str()),
            (Forge::Gitlab, "code.example.com")
        );
        assert_eq!(
            parse_remote("https://gitlab.corp.io/x/y", &none)
                .unwrap()
                .forge,
            Forge::Gitlab
        );
        assert_eq!(own.web(), "https://code.example.com/a/b");
    }

    #[test]
    fn odd_remotes_never_become_a_target() {
        let none: Vec<String> = vec![];
        for raw in [
            "https://user:secret@gitlab.com/a/b.git",
            "https://bitbucket.org/a/b/c",
            "https://gitlab.com/a",
            "https://gitlab.com/a/../b",
            "https://gitlab.com/a/b?x=1",
            "https://gitlab.com/-a/b",
            "git@gitlab.com:a/b c",
            "file:///tmp/repo",
            "/tmp/repo",
        ] {
            assert!(parse_remote(raw, &none).is_none(), "{raw}");
        }
    }

    #[test]
    fn glab_config_lists_its_hosts() {
        let config = "git_protocol: ssh\nhosts:\n    gitlab.com:\n        token: x\n    code.example.com:\n        api_host: code.example.com\n        token: y\nother:\n    not.a.host:\n";
        assert_eq!(
            glab_hosts_from(config),
            vec!["gitlab.com".to_owned(), "code.example.com".to_owned()]
        );
        assert!(glab_hosts_from("hosts:\n").is_empty());
    }

    #[test]
    fn gitlab_merge_request_and_jobs_read_strictly() {
        let remote = parse_remote("git@gitlab.com:g/p.git", &[]).unwrap();
        let sha = format!("abcdef123456{}", "0".repeat(28));
        let list = json!([{ "iid": 12, "title": "Fix\u{7}login", "state": "opened", "draft": true,
            "web_url": "https://gitlab.com/g/p/-/merge_requests/12?x=1", "source_branch": "feature/topic",
            "target_branch": "main", "sha": sha }]);
        let mut pr = parse_gitlab_mr(&list, &remote, &context()).unwrap();
        assert_eq!(pr.number, 12);
        assert_eq!(pr.title, "Fixlogin");
        assert_eq!(pr.state, "open");
        assert!(pr.draft && !pr.local_commit_differs);
        assert_eq!(pr.url, "https://gitlab.com/g/p/-/merge_requests/12");
        let jobs = json!([
            {"id": 3, "name": "test", "status": "failed", "allow_failure": false, "web_url": "https://gitlab.com/g/p/-/jobs/3"},
            {"id": 2, "name": "test", "status": "success", "web_url": "https://gitlab.com/g/p/-/jobs/2"},
            {"id": 1, "name": "lint", "status": "running", "web_url": "https://evil.example/jobs/1"},
            {"id": 4, "name": "flaky", "status": "failed", "allow_failure": true}
        ]);
        apply_gitlab_jobs(&mut pr, &jobs, "gitlab.com");
        let states: Vec<_> = pr.checks.iter().map(|c| c.status.clone()).collect();
        assert_eq!(
            states,
            vec![
                CheckStatus::Failed,
                CheckStatus::Pending,
                CheckStatus::Skipped
            ]
        );
        assert!(pr.checks[1].url.is_none());

        let other = json!([{ "iid": 12, "state": "opened", "web_url": "https://gitlab.com/g/p/-/merge_requests/12",
            "source_branch": "other", "sha": "a".repeat(40) }]);
        assert!(parse_gitlab_mr(&other, &remote, &context()).is_none());
        let foreign = json!([{ "iid": 12, "state": "merged", "web_url": "https://evil.example/x",
            "source_branch": "feature/topic", "sha": "a".repeat(40) }]);
        assert!(parse_gitlab_mr(&foreign, &remote, &context()).is_none());
        assert!(parse_gitlab_mr(&json!([]), &remote, &context()).is_none());
    }

    #[test]
    fn bitbucket_pull_request_and_statuses_read_strictly() {
        let reply = json!({"values": [{ "id": 5, "title": "Add", "state": "MERGED", "draft": false,
            "links": {"html": {"href": "https://bitbucket.org/team/repo/pull-requests/5"}},
            "source": {"branch": {"name": "feature/topic"}, "commit": {"hash": "abcdef123456"}},
            "destination": {"branch": {"name": "main"}} }]});
        let mut pr = parse_bitbucket_pr(&reply, &context()).unwrap();
        assert_eq!((pr.number, pr.state.as_str()), (5, "merged"));
        assert!(!pr.local_commit_differs);
        assert_eq!(pr.head_sha.len(), 40);
        assert_eq!(pr.base_branch, "main");
        let statuses = json!({"values": [
            {"key": "build", "name": "Pipeline", "state": "SUCCESSFUL", "url": "https://bitbucket.org/team/repo/pipelines/1"},
            {"key": "build", "name": "old", "state": "FAILED"},
            {"key": "ci", "state": "INPROGRESS", "url": "https://ci.example.com/run/2"}
        ]});
        apply_bitbucket_statuses(&mut pr, &statuses);
        assert_eq!(pr.checks.len(), 2);
        assert_eq!(pr.checks[0].status, CheckStatus::Passed);
        assert_eq!(pr.checks[1].name, "ci");
        assert_eq!(pr.checks[1].status, CheckStatus::Pending);

        let moved = json!({"values": [{ "id": 5, "state": "OPEN",
            "links": {"html": {"href": "https://bitbucket.org/team/repo/pull-requests/5"}},
            "source": {"branch": {"name": "feature/topic"}, "commit": {"hash": "999999999999"}} }]});
        assert!(
            parse_bitbucket_pr(&moved, &context())
                .unwrap()
                .local_commit_differs
        );
        assert!(parse_bitbucket_pr(&json!({"values": []}), &context()).is_none());
    }

    #[test]
    fn bitbucket_tokens_choose_their_scheme() {
        assert_eq!(
            auth_from("me@x.com:ATATT123"),
            Some(Auth::Basic("me@x.com".into(), "ATATT123".into()))
        );
        assert_eq!(auth_from("tok"), Some(Auth::Bearer("tok".into())));
        assert_eq!(auth_from(":x"), None);
        assert_eq!(auth_from(""), None);
        let remote = parse_remote("git@bitbucket.org:team/repo.git", &[]).unwrap();
        assert_eq!(
            bitbucket_repo_url(&remote),
            "https://api.bitbucket.org/2.0/repositories/team/repo"
        );
    }
}
