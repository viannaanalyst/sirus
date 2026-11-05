//! Actions the Settings pages run in this app itself, off the UI thread: the GitHub CLI accounts
//! (as `src-tauri/src/github_accounts.rs` runs them), opening a folder, and the two-click
//! confirmations. Every `pub fn` blocks on a child process: the caller runs it on a background
//! thread, then reloads the page's data.
//!
//! The run names the pages use, to route in `shell.rs` `run_setting`:
//! `native_github_list`, `native_github_switch {login}`, `native_github_logout {login}`,
//! `native_github_login`, `native_github_cancel`, `native_open_path {path}`,
//! `native_delete_sessions {ids}`, `native_reset_layout`.

use crate::i18n::{t, tf};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

const HOST: &str = "github.com";
/// How long `gh` may take to print the one-time code.
const CODE_TIMEOUT: Duration = Duration::from_secs(20);
/// GitHub's device codes expire after 15 minutes.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// A `gh auth login --web` waiting for its device code to be confirmed in the browser.
struct Login {
    child: Arc<Mutex<Child>>,
    /// Every line `gh` printed, from both of its streams.
    seen: Arc<Mutex<Vec<String>>>,
    /// The accounts before the sign-in, as (login, active), to tell a sign-in that worked.
    before: Vec<(String, bool)>,
}

static PENDING: Mutex<Option<Login>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

// Two-click confirmations. A page arms a button's key on the first click and shows "Clicar de
// novo"; the second click runs the action. The arm lasts ARM_FOR and holds one key at a time.

// GitHub accounts, through `gh`.

/// A `gh` command with the environment `gh` needs from a Finder launch: its PATH, no prompts, and
/// no repository or host variables from the shell.
fn gh() -> Command {
    let mut command = Command::new(gh_binary());
    command
        .env("PATH", search_path())
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env_remove("GH_HOST")
        .env_remove("GH_REPO")
        .env_remove("GH_PAGER")
        .env_remove("PAGER")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null());
    command
}

fn gh_binary() -> PathBuf {
    ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"]
        .iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("gh"))
}

fn search_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    format!("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{current}")
}

/// A GitHub login: letters, digits and single hyphens, at most 39 characters.
fn valid_login(login: &str) -> bool {
    (1..=39).contains(&login.len())
        && login
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !login.starts_with('-')
}

/// The github.com accounts in `gh auth status --json hosts`, active first.
fn parse_status(stdout: &[u8]) -> Vec<Value> {
    let Ok(value) = serde_json::from_slice::<Value>(stdout) else {
        return Vec::new();
    };
    let mut accounts: Vec<(String, bool, bool)> = value["hosts"][HOST]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let login = entry["login"].as_str()?;
            valid_login(login).then(|| {
                (
                    login.to_string(),
                    entry["active"].as_bool() == Some(true),
                    entry["state"].as_str() == Some("success"),
                )
            })
        })
        .collect();
    accounts.sort_by_key(|(_, active, _)| !*active);
    accounts.dedup_by(|a, b| a.0 == b.0);
    accounts
        .into_iter()
        .map(|(login, active, valid)| json!({"login": login, "active": active, "valid": valid}))
        .collect()
}

/// The GitHub accounts for the Git page: `{accounts: [{login, active, valid}], missing}`, where
/// `missing` means the GitHub CLI is not installed. A signed-out `gh` still lists its accounts.
pub fn github_list() -> Value {
    match gh()
        .args(["auth", "status", "--hostname", HOST, "--json", "hosts"])
        .output()
    {
        Ok(output) => json!({"accounts": parse_status(&output.stdout), "missing": false}),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            json!({"accounts": [], "missing": true})
        }
        Err(_) => json!({"accounts": [], "missing": false}),
    }
}

/// The (login, active) pairs `github_list` reports.
fn account_pairs() -> Vec<(String, bool)> {
    github_list()["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|account| {
            Some((
                account["login"].as_str()?.to_string(),
                account["active"].as_bool() == Some(true),
            ))
        })
        .collect()
}

fn gh_failure(error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        t("O GitHub CLI (gh) não está instalado. Instale com `brew install gh`.").to_string()
    } else {
        error.to_string()
    }
}

/// `gh auth <verb> --user <login>` for an account that is signed in.
fn for_account(verb: &str, login: &str) -> Result<(), String> {
    if !valid_login(login) || !account_pairs().iter().any(|(known, _)| known == login) {
        return Err(t("Essa conta do GitHub não está conectada.").to_string());
    }
    let output = gh()
        .args(["auth", verb, "--hostname", HOST, "--user", login])
        .output()
        .map_err(gh_failure)?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if message.is_empty() {
        t("O GitHub CLI não conseguiu alterar a conta.").to_string()
    } else {
        message
    })
}

/// Makes `login` the active github.com account.
pub fn github_switch(login: &str) -> Result<(), String> {
    for_account("switch", login)
}

/// Signs `login` out of github.com. The page arms it first (`arm`) so one click cannot sign out.
pub fn github_logout(login: &str) -> Result<(), String> {
    for_account("logout", login)
}

/// True when the accounts show a sign-in that worked: a new account, or another one made active.
/// `gh` can exit with a failure after it stored the token, so the accounts decide.
fn signed_in(before: &[(String, bool)], after: &[(String, bool)]) -> bool {
    let active = |list: &[(String, bool)]| {
        list.iter()
            .find(|(_, active)| *active)
            .map(|(login, _)| login.clone())
    };
    after
        .iter()
        .any(|(login, _)| !before.iter().any(|(old, _)| old == login))
        || (active(after).is_some() && active(after) != active(before))
}

/// The one-time code and the URL from `gh auth login --web`'s output, once both are in.
fn parse_device_prompt(lines: &[String]) -> Option<(String, String)> {
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

/// Reads one of `gh`'s streams line by line into `seen`, on its own thread.
fn collect_lines<R: Read + Send + 'static>(stream: R, seen: Arc<Mutex<Vec<String>>>) {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            lock(&seen).push(line);
        }
    });
}

/// Starts `gh auth login --web` for github.com and waits for its device code (up to 20 s). Returns
/// (code, url) to show, and opens the URL in the browser. The sign-in then waits in `PENDING`:
/// call `github_login_wait` on a thread to finish it. None when `gh` is missing or gave no code.
pub fn github_login_start() -> Option<(String, String)> {
    github_login_cancel();
    let before = account_pairs();
    let mut child = gh()
        .args([
            "auth",
            "login",
            "--hostname",
            HOST,
            "--web",
            "--git-protocol",
            "https",
        ])
        // The page opens here, next to the code, so `gh` must not open its own.
        .env("GH_BROWSER", "true")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let seen = Arc::new(Mutex::new(Vec::new()));
    if let Some(stream) = child.stdout.take() {
        collect_lines(stream, seen.clone());
    }
    if let Some(stream) = child.stderr.take() {
        collect_lines(stream, seen.clone());
    }
    let started = Instant::now();
    let found = loop {
        if let Some(found) = parse_device_prompt(&lock(&seen)) {
            break Some(found);
        }
        if started.elapsed() > CODE_TIMEOUT || matches!(child.try_wait(), Ok(Some(_))) {
            break None;
        }
        thread::sleep(Duration::from_millis(100));
    };
    let Some((code, url)) = found else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let _ = Command::new("open").arg(&url).spawn();
    *lock(&PENDING) = Some(Login {
        child: Arc::new(Mutex::new(child)),
        seen,
        before,
    });
    Some((code, url))
}

/// Waits (up to 15 minutes) for the sign-in `github_login_start` began. Ok when it worked; Err with
/// the message to show otherwise, including when `github_login_cancel` stopped it.
pub fn github_login_wait() -> Result<(), String> {
    let Some((child, seen, before)) = lock(&PENDING).as_ref().map(|login| {
        (
            login.child.clone(),
            login.seen.clone(),
            login.before.clone(),
        )
    }) else {
        return Err(t("Nenhum login do GitHub está esperando.").to_string());
    };
    let started = Instant::now();
    let status = loop {
        let polled = lock(&child).try_wait();
        match polled {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < LOGIN_TIMEOUT => {
                thread::sleep(Duration::from_millis(300))
            }
            _ => {
                let _ = lock(&child).kill();
                break None;
            }
        }
    };
    *lock(&PENDING) = None;
    let Some(status) = status else {
        return Err(t("O código de login expirou. Tente de novo.").to_string());
    };
    if status.success() || signed_in(&before, &account_pairs()) {
        return Ok(());
    }
    let last = lock(&seen)
        .iter()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string());
    Err(last.unwrap_or_else(|| t("O login do GitHub não terminou.").to_string()))
}

/// Stops the sign-in `github_login_start` began, if one waits.
pub fn github_login_cancel() {
    if let Some(login) = lock(&PENDING).take() {
        let _ = lock(&login.child).kill();
    }
}

// Folders.

/// Opens `path` in Finder (the data folder or its logs).
pub fn open_path(path: &str) -> Result<(), String> {
    let status = Command::new("open")
        .arg(path)
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(tf("Não foi possível abrir {}.", &[&path]))
    }
}
