//! GitHub accounts through the GitHub CLI (ADR-188). `gh` keeps several accounts per host
//! with one active; every GitHub feature here (pull requests, PR watches, CI auto-fix) and
//! `git push` over HTTPS use the active one. Settings → Git lists them, switches the active
//! account, signs one out, and adds one with GitHub's device flow: `gh auth login --web`
//! prints a one-time code and waits while the person confirms it on github.com.

use std::ffi::OsStr;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;

use crate::error::{Error, Result};

const HOST: &str = "github.com";
const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
/// How long `gh` may take to print the one-time code.
const CODE_TIMEOUT: Duration = Duration::from_secs(20);
/// GitHub's device codes expire after 15 minutes.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubAccount {
    pub login: String,
    pub active: bool,
    /// The stored token still works (`gh auth status` reported success).
    pub valid: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    List,
    Switch { login: String },
    Logout { login: String },
    LoginStart,
    LoginWait,
    LoginCancel,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Response {
    /// `missing`: the GitHub CLI is not installed.
    Accounts {
        accounts: Vec<GitHubAccount>,
        missing: bool,
    },
    LoginCode {
        code: String,
        url: String,
    },
    Done,
}

/// A `gh auth login` waiting for the person to confirm its code.
struct Login {
    child: Child,
    /// The rest of its output, for the error when it fails.
    output: tokio::sync::mpsc::UnboundedReceiver<String>,
    /// The accounts before it, to tell a sign-in that worked from one that did not.
    before: Vec<GitHubAccount>,
}

fn pending() -> &'static Mutex<Option<Login>> {
    static PENDING: OnceLock<Mutex<Option<Login>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}

/// Wakes a `loginWait` so Cancel stops the sign-in it is waiting on.
fn cancelled() -> &'static tokio::sync::Notify {
    static CANCELLED: OnceLock<tokio::sync::Notify> = OnceLock::new();
    CANCELLED.get_or_init(tokio::sync::Notify::new)
}

/// The sign-in added an account or made another one active: `gh` can exit with a failure
/// after storing the token (a later step such as Git setup), so the accounts decide.
pub fn signed_in(before: &[GitHubAccount], after: &[GitHubAccount]) -> bool {
    let active = |list: &[GitHubAccount]| list.iter().find(|a| a.active).map(|a| a.login.clone());
    after
        .iter()
        .any(|account| !before.iter().any(|old| old.login == account.login))
        || (active(after).is_some() && active(after) != active(before))
}

async fn account_list() -> Result<Vec<GitHubAccount>> {
    match accounts().await? {
        Response::Accounts { accounts, .. } => Ok(accounts),
        _ => Ok(Vec::new()),
    }
}

fn gh() -> tokio::process::Command {
    crate::pull_requests::hardened(OsStr::new("gh"))
}

/// A GitHub login: letters, digits and single hyphens, at most 39 characters.
pub fn valid_login(login: &str) -> bool {
    (1..=39).contains(&login.len())
        && login
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !login.starts_with('-')
}

/// The github.com accounts in `gh auth status --json hosts`, active first.
pub fn parse_status(stdout: &[u8]) -> Vec<GitHubAccount> {
    let Ok(value) = serde_json::from_slice::<Value>(stdout) else {
        return Vec::new();
    };
    let mut accounts: Vec<GitHubAccount> = value["hosts"][HOST]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let login = entry["login"].as_str()?;
            valid_login(login).then(|| GitHubAccount {
                login: login.to_string(),
                active: entry["active"].as_bool() == Some(true),
                valid: entry["state"].as_str() == Some("success"),
            })
        })
        .collect();
    accounts.sort_by_key(|account| !account.active);
    accounts.dedup_by(|a, b| a.login == b.login);
    accounts
}

/// The one-time code and the URL from `gh auth login --web`'s output, once both are in.
pub fn parse_device_prompt(lines: &[String]) -> Option<(String, String)> {
    let code = lines.iter().find_map(|line| {
        let rest = line.split("one-time code:").nth(1)?.trim();
        let valid = (8..=12).contains(&rest.len())
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        valid.then(|| rest.to_string())
    })?;
    let url = lines.iter().find_map(|line| {
        let start = line.find("https://github.com/")?;
        let url = line[start..].split_whitespace().next()?;
        url.starts_with("https://github.com/login/device")
            .then(|| url.to_string())
    })?;
    Some((code, url))
}

async fn accounts() -> Result<Response> {
    let mut command = gh();
    command.args(["auth", "status", "--hostname", HOST, "--json", "hosts"]);
    match crate::cli_output::capture_command(command, STATUS_TIMEOUT).await {
        // Signed out, or a token that no longer works, still prints the JSON with a failure code.
        Ok(output) => Ok(Response::Accounts {
            accounts: parse_status(&output.stdout),
            missing: false,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Response::Accounts {
            accounts: Vec::new(),
            missing: true,
        }),
        Err(error) => Err(error.into()),
    }
}

/// Runs `gh auth <verb> --user <login>` for a signed-in account.
async fn for_account(verb: &str, login: &str) -> Result<Response> {
    let Response::Accounts {
        accounts: known, ..
    } = accounts().await?
    else {
        unreachable!()
    };
    if !valid_login(login) || !known.iter().any(|account| account.login == login) {
        return Err(Error::not_found("That GitHub account is not signed in."));
    }
    let mut command = gh();
    command.args(["auth", verb, "--hostname", HOST, "--user", login]);
    let output = crate::cli_output::capture_command(command, COMMAND_TIMEOUT).await;
    crate::gh_gate::wrote(&output);
    let output = output?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::new(
            "github_account",
            if message.is_empty() {
                "GitHub CLI could not change the account.".into()
            } else {
                message
            },
        ));
    }
    accounts().await
}

async fn login_start() -> Result<Response> {
    login_cancel();
    let before = account_list().await.unwrap_or_default();
    let mut command = gh();
    command
        .args([
            "auth",
            "login",
            "--hostname",
            HOST,
            "--web",
            "--git-protocol",
            "https",
        ])
        // Sirus opens the page itself, next to the code.
        .env("GH_BROWSER", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::new("gh_missing", "GitHub CLI (gh) is not installed.")
        } else {
            error.into()
        }
    })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::new("github_login", "GitHub CLI did not start."))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::new("github_login", "GitHub CLI did not start."))?;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
    for stream in [
        Box::new(stderr) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
        Box::new(stdout),
    ] {
        let sender = sender.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stream).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
    }
    drop(sender);
    let mut seen = Vec::new();
    let prompt = tokio::time::timeout(CODE_TIMEOUT, async {
        while let Some(line) = receiver.recv().await {
            seen.push(line);
            if let Some(found) = parse_device_prompt(&seen) {
                return Some(found);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let Some((code, url)) = prompt else {
        let _ = child.start_kill();
        return Err(Error::new(
            "github_login",
            "GitHub CLI did not give a sign-in code. Try again.",
        ));
    };
    *pending().lock() = Some(Login {
        child,
        output: receiver,
        before,
    });
    Ok(Response::LoginCode { code, url })
}

async fn login_wait() -> Result<Response> {
    let Some(Login {
        mut child,
        mut output,
        before,
    }) = pending().lock().take()
    else {
        return Err(Error::new("github_login", "No GitHub sign-in is waiting."));
    };
    let status = tokio::select! {
        status = child.wait() => Some(status?),
        _ = cancelled().notified() => None,
        _ = tokio::time::sleep(LOGIN_TIMEOUT) => {
            let _ = child.start_kill();
            return Err(Error::new("github_login", "The sign-in code expired. Try again."));
        }
    };
    let Some(status) = status else {
        let _ = child.start_kill();
        return accounts().await;
    };
    crate::gh_gate::wrote(&Ok(std::process::Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    }));
    let after = account_list().await?;
    if status.success() || signed_in(&before, &after) {
        return accounts().await;
    }
    let mut last = None;
    while let Ok(line) = output.try_recv() {
        if !line.trim().is_empty() {
            last = Some(line.trim().to_string());
        }
    }
    Err(Error::new(
        "github_login",
        last.unwrap_or_else(|| "GitHub sign-in did not finish.".into()),
    ))
}

fn login_cancel() {
    if let Some(mut login) = pending().lock().take() {
        let _ = login.child.start_kill();
    }
    cancelled().notify_waiters();
}

pub async fn github_account_action(action: Action) -> Result<Response> {
    match action {
        Action::List => accounts().await,
        Action::Switch { login } => for_account("switch", &login).await,
        Action::Logout { login } => for_account("logout", &login).await,
        Action::LoginStart => login_start().await,
        Action::LoginWait => login_wait().await,
        Action::LoginCancel => {
            login_cancel();
            Ok(Response::Done)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_accounts_active_first() {
        let json = br#"{"hosts":{"github.com":[
            {"state":"success","active":false,"host":"github.com","login":"work-account"},
            {"state":"success","active":true,"host":"github.com","login":"viannaanalyst"},
            {"state":"error","active":false,"host":"github.com","login":"old"},
            {"state":"success","active":false,"host":"github.com","login":"bad login"}
        ],"example.com":[{"state":"success","active":true,"login":"elsewhere"}]}}"#;
        let accounts = parse_status(json);
        assert_eq!(
            accounts
                .iter()
                .map(|a| (a.login.as_str(), a.active, a.valid))
                .collect::<Vec<_>>(),
            vec![
                ("viannaanalyst", true, true),
                ("work-account", false, true),
                ("old", false, false)
            ]
        );
        assert!(parse_status(b"not json").is_empty());
        assert!(parse_status(br#"{"hosts":{}}"#).is_empty());
    }

    #[test]
    fn a_new_or_newly_active_account_means_signed_in() {
        let account = |login: &str, active: bool| GitHubAccount {
            login: login.into(),
            active,
            valid: true,
        };
        let before = vec![account("a", true)];
        assert!(signed_in(
            &before,
            &[account("b", true), account("a", false)]
        ));
        assert!(signed_in(
            &[account("a", true), account("b", false)],
            &[account("b", true), account("a", false)]
        ));
        assert!(!signed_in(&before, &before));
        assert!(signed_in(&[], &[account("a", true)]));
        assert!(!signed_in(&[], &[]));
    }

    #[test]
    fn logins_are_checked() {
        assert!(valid_login("viannaanalyst"));
        assert!(valid_login("a-b-1"));
        assert!(!valid_login(""));
        assert!(!valid_login("-flag"));
        assert!(!valid_login("a b"));
        assert!(!valid_login(&"a".repeat(40)));
    }

    #[test]
    fn finds_the_device_code_and_url() {
        let lines = vec![
            "! First copy your one-time code: E65D-959D".to_string(),
            "Open this URL to continue in your web browser: https://github.com/login/device"
                .to_string(),
        ];
        assert_eq!(
            parse_device_prompt(&lines),
            Some(("E65D-959D".into(), "https://github.com/login/device".into()))
        );
        assert_eq!(parse_device_prompt(&lines[..1]), None);
        let other = vec![
            "! First copy your one-time code: E65D-959D".to_string(),
            "Open https://github.com/evil".to_string(),
        ];
        assert_eq!(parse_device_prompt(&other), None);
    }
}
