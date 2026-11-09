//! Infisical (ADR-198): the project keys the owner keeps in Infisical cloud, reached with a machine
//! identity (Universal Auth) whose client id and secret live in the macOS Keychain under account
//! `sirus`, the same items the `infisical-run` wrapper reads. The window only ever sees project
//! and key *names*; a value is typed in a secret card and saved here, or read here and handed
//! straight to the waiting secret request, and never reaches a log or the window.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::runtime::State;

const API: &str = "https://app.infisical.com";
const ACCOUNT: &str = "sirus";
const ID_SERVICE: &str = "Sirus Infisical client id";
const SECRET_SERVICE: &str = "Sirus Infisical client secret";
const LINK_FILE: &str = ".infisical.json";
const MAX_VALUE: usize = 16 * 1024;
/// A token this close to expiring is renewed.
const TOKEN_MARGIN: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    /// Whether the credentials exist, whether a login works, and the organization's projects.
    Status,
    /// Keeps the machine identity's credentials in the Keychain.
    Connect {
        client_id: String,
        client_secret: String,
    },
    /// Forgets the credentials.
    Disconnect,
    /// Writes `.infisical.json` in the project's folder.
    LinkProject {
        project_path: String,
        workspace_id: String,
        environment: String,
    },
    /// Reads `.infisical.json` back.
    ProjectLink { project_path: String },
    /// The names of the secrets in an environment, never their values.
    ListKeys {
        project_path: String,
        environment: String,
    },
    /// Creates or updates one secret.
    SaveSecret {
        project_path: String,
        environment: String,
        key: String,
        value: String,
    },
    /// The keys a new terminal of this conversation starts with: every secret of the project's
    /// environment as `{name: value}`, empty when the project does not use them (ADR-198).
    TerminalEnv { session_id: String },
    /// Reads a secret here and answers the waiting secret request with it.
    Answer {
        request_id: String,
        project_path: String,
        environment: String,
        key: String,
    },
}

// ---- Keychain ----

#[cfg(target_os = "macos")]
mod keys {
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    pub fn get(service: &str) -> Option<String> {
        get_generic_password(service, super::ACCOUNT)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .filter(|text| !text.is_empty())
    }

    pub fn put(service: &str, value: &str) -> bool {
        set_generic_password(service, super::ACCOUNT, value.as_bytes()).is_ok()
            || (delete_generic_password(service, super::ACCOUNT).is_ok()
                && set_generic_password(service, super::ACCOUNT, value.as_bytes()).is_ok())
    }

    pub fn delete(service: &str) {
        let _ = delete_generic_password(service, super::ACCOUNT);
    }
}

#[cfg(not(target_os = "macos"))]
mod keys {
    //! Elsewhere the connectors' private store keeps them.
    fn id(service: &str) -> String {
        format!("infisical:{service}")
    }
    pub fn get(service: &str) -> Option<String> {
        crate::connectors::secret_get(&id(service)).filter(|text| !text.is_empty())
    }
    pub fn put(service: &str, value: &str) -> bool {
        crate::connectors::secret_put(&id(service), value)
    }
    pub fn delete(service: &str) {
        crate::connectors::secret_delete(&id(service));
    }
}

fn credentials() -> Option<(String, String)> {
    Some((keys::get(ID_SERVICE)?, keys::get(SECRET_SERVICE)?))
}

// ---- Token cache ----

struct Token {
    client_id: String,
    value: String,
    until: Instant,
}

static TOKEN: Mutex<Option<Token>> = Mutex::new(None);

fn token_cache() -> std::sync::MutexGuard<'static, Option<Token>> {
    TOKEN.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn forget_token() {
    *token_cache() = None;
}

fn cached_token(client_id: &str) -> Option<String> {
    token_cache()
        .as_ref()
        .filter(|token| token.client_id == client_id && token.until > Instant::now())
        .map(|token| token.value.clone())
}

// ---- Requests ----

pub(crate) fn login_body(client_id: &str, client_secret: &str) -> Value {
    json!({ "clientId": client_id, "clientSecret": client_secret })
}

/// `https://app.infisical.com/<segments>?<query>`; each segment is percent-encoded.
pub(crate) fn url_for(segments: &[&str], query: &[(&str, &str)]) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(API).map_err(|_| unreadable())?;
    url.path_segments_mut()
        .map_err(|_| unreadable())?
        .extend(segments);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    Ok(url)
}

pub(crate) fn secrets_url(workspace_id: &str, environment: &str) -> Result<reqwest::Url> {
    url_for(
        &["api", "v3", "secrets", "raw"],
        &[
            ("workspaceId", workspace_id),
            ("environment", environment),
            ("secretPath", "/"),
        ],
    )
}

pub(crate) fn secret_url(workspace_id: &str, environment: &str, key: &str) -> Result<reqwest::Url> {
    url_for(
        &["api", "v3", "secrets", "raw", key],
        &[
            ("workspaceId", workspace_id),
            ("environment", environment),
            ("secretPath", "/"),
        ],
    )
}

pub(crate) fn save_body(workspace_id: &str, environment: &str, value: &str) -> Value {
    json!({
        "workspaceId": workspace_id,
        "environment": environment,
        "secretPath": "/",
        "secretValue": value,
    })
}

fn unreadable() -> Error {
    Error::agent("Infisical answered something unreadable.")
}

fn unreachable_error() -> Error {
    Error::agent("Infisical could not be reached.")
}

fn not_connected() -> Error {
    Error::agent("Infisical is not connected.")
}

fn http_status_error(status: u16) -> Error {
    Error::agent(match status {
        401 | 403 => "Infisical refused the credentials or this project is out of their reach.",
        404 => "That Infisical project, environment or secret was not found.",
        _ => "Infisical refused the request.",
    })
}

async fn send(
    method: Method,
    url: reqwest::Url,
    bearer: Option<&str>,
    body: Option<&Value>,
) -> Result<(u16, Value)> {
    let client = crate::connectors::http()?;
    let mut request = client
        .request(method, url)
        .header("Accept", "application/json");
    if let Some(bearer) = bearer {
        request = request.bearer_auth(bearer);
    }
    if let Some(body) = body {
        request = request
            .header("Content-Type", "application/json")
            .body(serde_json::to_vec(body)?);
    }
    let response = request.send().await.map_err(|_| unreachable_error())?;
    let status = response.status().as_u16();
    let bytes = response.bytes().await.map_err(|_| unreachable_error())?;
    Ok((
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    ))
}

/// Logs in with the Keychain's credentials, or reuses the token until it expires.
async fn token() -> Result<String> {
    let (client_id, client_secret) = credentials().ok_or_else(not_connected)?;
    if let Some(token) = cached_token(&client_id) {
        return Ok(token);
    }
    let url = url_for(&["api", "v1", "auth", "universal-auth", "login"], &[])?;
    let (status, reply) = send(
        Method::POST,
        url,
        None,
        Some(&login_body(&client_id, &client_secret)),
    )
    .await?;
    if !(200..300).contains(&status) {
        return Err(Error::agent(
            "Infisical refused the sign-in. Check the client ID and the secret.",
        ));
    }
    let value = reply["accessToken"]
        .as_str()
        .filter(|token| !token.is_empty())
        .ok_or_else(unreadable)?
        .to_owned();
    let lifetime = Duration::from_secs(reply["expiresIn"].as_u64().unwrap_or(600));
    *token_cache() = Some(Token {
        client_id,
        value: value.clone(),
        until: Instant::now() + lifetime.saturating_sub(TOKEN_MARGIN),
    });
    Ok(value)
}

/// One authorized call; a rejected token is renewed once.
async fn api(method: Method, url: reqwest::Url, body: Option<Value>) -> Result<(u16, Value)> {
    for attempt in 0..2 {
        let bearer = token().await?;
        let (status, reply) =
            send(method.clone(), url.clone(), Some(&bearer), body.as_ref()).await?;
        if status == 401 && attempt == 0 {
            forget_token();
            continue;
        }
        return Ok((status, reply));
    }
    Err(unreachable_error())
}

// ---- Replies ----

/// The organization's projects: id, name and environments.
pub(crate) fn projects_of(reply: &Value) -> Vec<Value> {
    reply["workspaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|workspace| {
            let id = workspace["id"].as_str()?;
            let environments: Vec<Value> = workspace["environments"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|environment| {
                    Some(json!({
                        "slug": environment["slug"].as_str()?,
                        "name": environment["name"].as_str().unwrap_or_default(),
                    }))
                })
                .collect();
            Some(json!({
                "id": id,
                "name": workspace["name"].as_str().unwrap_or(id),
                "environments": environments,
            }))
        })
        .collect()
}

/// Secret names, sorted. Only `secretKey` is read, so a value can never leak through here.
pub(crate) fn keys_of(reply: &Value) -> Vec<String> {
    let mut names: Vec<String> = reply["secrets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|secret| secret["secretKey"].as_str().map(str::to_owned))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Every secret of a listing as `(name, value)`, skipping names a shell cannot export. The values
/// are only ever moved into the terminal's environment, never formatted into text.
pub(crate) fn env_pairs_of(reply: &Value) -> Vec<(String, String)> {
    reply["secrets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|secret| {
            let key = secret["secretKey"].as_str().filter(|key| valid_key(key))?;
            let value = secret["secretValue"].as_str()?;
            (!value.contains('\0')).then(|| (key.to_owned(), value.to_owned()))
        })
        .collect()
}

/// The project and environment a conversation's terminals use keys from, when it is linked and
/// the switch is on.
fn terminal_source(state: &AppState, session_id: &str) -> Result<Option<(String, String)>> {
    terminal_source_in(&state.data.lock(), session_id)
}

/// `terminal_source` for a caller that already holds the data lock.
pub(crate) fn terminal_source_in(
    data: &crate::models::AppData,
    session_id: &str,
) -> Result<Option<(String, String)>> {
    let session = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| Error::not_found("This conversation no longer exists."))?;
    let project = data
        .projects
        .iter()
        .find(|project| project.id == session.project_id)
        .ok_or_else(|| Error::not_found("That project is not in Sirus."))?;
    let Some(environment) = project.scripts.infisical_run.as_deref() else {
        return Ok(None);
    };
    if !valid_slug(environment) {
        return Ok(None);
    }
    Ok(read_link(Path::new(&project.path))
        .map(|(workspace, _)| (workspace, environment.to_owned())))
}

/// The keys a terminal starts with, for the `(workspace, environment)` of `terminal_source`
/// (nothing when there is no source). Shared by the `terminalEnv` action and the host's own
/// terminals (the phone's).
pub(crate) async fn terminal_env_pairs(
    source: Option<(String, String)>,
) -> Result<Vec<(String, String)>> {
    let Some((workspace, environment)) = source else {
        return Ok(Vec::new());
    };
    let (status, reply) = api(Method::GET, secrets_url(&workspace, &environment)?, None).await?;
    if !(200..300).contains(&status) {
        return Err(http_status_error(status));
    }
    Ok(env_pairs_of(&reply))
}

// ---- The link file ----

fn valid_slug(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_workspace(text: &str) -> bool {
    valid_slug(text)
}

/// `.infisical.json` in `root`: the project id and the default environment. Other keys already in
/// the file stay.
pub(crate) fn write_link(root: &Path, workspace_id: &str, environment: &str) -> Result<()> {
    if !valid_workspace(workspace_id) || !valid_slug(environment) {
        return Err(Error::agent(
            "The Infisical project or environment is not valid.",
        ));
    }
    let path = root.join(LINK_FILE);
    let mut file = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    file["workspaceId"] = json!(workspace_id);
    file["defaultEnvironment"] = json!(environment);
    let mut text = serde_json::to_string_pretty(&file)?;
    text.push('\n');
    let temp = root.join(format!(".infisical.json.{}.tmp", std::process::id()));
    std::fs::write(&temp, text)
        .and_then(|_| std::fs::rename(&temp, &path))
        .map_err(|_| {
            let _ = std::fs::remove_file(&temp);
            Error::agent("The .infisical.json file could not be saved.")
        })
}

/// `(workspaceId, environment)` from `.infisical.json`, when the folder has a usable one.
pub(crate) fn read_link(root: &Path) -> Option<(String, String)> {
    let file: Value = serde_json::from_slice(&std::fs::read(root.join(LINK_FILE)).ok()?).ok()?;
    let workspace = file["workspaceId"]
        .as_str()
        .filter(|w| valid_workspace(w))?;
    let environment = file["defaultEnvironment"]
        .as_str()
        .filter(|e| valid_slug(e))
        .unwrap_or("dev");
    Some((workspace.to_owned(), environment.to_owned()))
}

fn link_reply(root: &Path) -> Value {
    match read_link(root) {
        Some((workspace, environment)) => {
            json!({ "kind": "link", "linked": true, "workspaceId": workspace, "environment": environment })
        }
        None => json!({ "kind": "link", "linked": false }),
    }
}

// ---- Scripts ----

/// `script` run under `infisical-run`, so it gets the project's keys. A worktree may not have the
/// untracked `.infisical.json`, so the project id is passed along when known.
pub fn wrap_script(script: &str, environment: &str, workspace_id: Option<&str>) -> String {
    let project = workspace_id
        .filter(|id| valid_workspace(id))
        .map(|id| format!(" --projectId={id}"))
        .unwrap_or_default();
    format!(
        "PATH=\"$HOME/.local/bin:$PATH\" infisical-run --env={environment}{project} -- \"${{SHELL:-/bin/sh}}\" -c {}",
        crate::secrets::shell_quote(script)
    )
}

/// The environment name a project's scripts run under, when valid.
pub fn valid_environment(text: &str) -> bool {
    valid_slug(text)
}

// ---- Actions ----

fn project_root(state: &AppState, project_path: &str) -> Result<PathBuf> {
    let wanted = project_path.trim_end_matches('/');
    let data = state.data.lock();
    data.projects
        .iter()
        .find(|project| project.path.trim_end_matches('/') == wanted)
        .map(|project| PathBuf::from(&project.path))
        .ok_or_else(|| Error::not_found("That project is not in Sirus."))
}

fn linked(state: &AppState, project_path: &str, environment: &str) -> Result<String> {
    if !valid_slug(environment) {
        return Err(Error::agent(
            "The Infisical project or environment is not valid.",
        ));
    }
    let root = project_root(state, project_path)?;
    read_link(&root)
        .map(|(workspace, _)| workspace)
        .ok_or_else(|| Error::agent("This project is not linked to Infisical."))
}

fn valid_key(key: &str) -> bool {
    crate::secrets::valid_env_name(key)
}

async fn save_secret(workspace_id: &str, environment: &str, key: &str, value: &str) -> Result<()> {
    if !valid_key(key) {
        return Err(Error::agent("That secret name is not valid."));
    }
    if value.is_empty() || value.len() > MAX_VALUE || value.contains('\0') {
        return Err(Error::agent("Enter a value up to 16 KB."));
    }
    let body = save_body(workspace_id, environment, value);
    let url = secret_url(workspace_id, environment, key)?;
    let (status, _) = api(Method::POST, url.clone(), Some(body.clone())).await?;
    if (200..300).contains(&status) {
        return Ok(());
    }
    // A secret that already exists is updated instead.
    if status == 400 || status == 409 {
        let (status, _) = api(Method::PATCH, url, Some(body)).await?;
        if (200..300).contains(&status) {
            return Ok(());
        }
        return Err(http_status_error(status));
    }
    Err(http_status_error(status))
}

async fn read_secret(workspace_id: &str, environment: &str, key: &str) -> Result<String> {
    if !valid_key(key) {
        return Err(Error::agent("That secret name is not valid."));
    }
    let url = secret_url(workspace_id, environment, key)?;
    let (status, reply) = api(Method::GET, url, None).await?;
    if !(200..300).contains(&status) {
        return Err(http_status_error(status));
    }
    reply["secret"]["secretValue"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(unreadable)
}

/// The secret card's "Save in Infisical": the typed value goes to the linked project under the
/// name the agent asked for. Fails without touching the request, which the caller still answers.
pub(crate) async fn save_for_session(
    state: &AppState,
    session_id: &str,
    environment: &str,
    key: &str,
    value: &str,
) -> Result<()> {
    let root = {
        let data = state.data.lock();
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("This conversation no longer exists."))?;
        data.projects
            .iter()
            .find(|project| project.id == session.project_id)
            .map(|project| PathBuf::from(&project.path))
            .ok_or_else(|| Error::not_found("That project is not in Sirus."))?
    };
    if !valid_slug(environment) {
        return Err(Error::agent(
            "The Infisical project or environment is not valid.",
        ));
    }
    let (workspace, _) =
        read_link(&root).ok_or_else(|| Error::agent("This project is not linked to Infisical."))?;
    save_secret(&workspace, environment, key, value).await
}

pub async fn infisical_action(state: State<'_, Arc<AppState>>, action: Action) -> Result<Value> {
    state.ensure_running()?;
    let state: &AppState = state.inner();
    match action {
        Action::Status => {
            if credentials().is_none() {
                return Ok(
                    json!({ "kind": "status", "connected": false, "loggedIn": false, "projects": [] }),
                );
            }
            let listed = api(
                Method::GET,
                url_for(&["api", "v1", "workspace"], &[])?,
                None,
            )
            .await;
            Ok(match listed {
                Ok((status, reply)) if (200..300).contains(&status) => json!({
                    "kind": "status",
                    "connected": true,
                    "loggedIn": true,
                    "identity": ACCOUNT,
                    "projects": projects_of(&reply),
                }),
                Ok((status, _)) => json!({
                    "kind": "status", "connected": true, "loggedIn": false, "identity": ACCOUNT,
                    "projects": [], "error": http_status_error(status).to_string(),
                }),
                Err(error) => json!({
                    "kind": "status", "connected": true, "loggedIn": false, "identity": ACCOUNT,
                    "projects": [], "error": error.to_string(),
                }),
            })
        }
        Action::Connect {
            client_id,
            client_secret,
        } => {
            let (client_id, client_secret) = (client_id.trim(), client_secret.trim());
            if client_id.is_empty()
                || client_secret.is_empty()
                || client_id.len() > 256
                || client_secret.len() > 1024
                || client_id.contains('\0')
                || client_secret.contains('\0')
            {
                return Err(Error::agent("Enter the client ID and the client secret."));
            }
            if !(keys::put(ID_SERVICE, client_id) && keys::put(SECRET_SERVICE, client_secret)) {
                return Err(Error::agent(
                    "The Keychain could not save the Infisical credentials.",
                ));
            }
            forget_token();
            Ok(json!({ "kind": "connected" }))
        }
        Action::Disconnect => {
            keys::delete(ID_SERVICE);
            keys::delete(SECRET_SERVICE);
            forget_token();
            Ok(json!({ "kind": "disconnected" }))
        }
        Action::LinkProject {
            project_path,
            workspace_id,
            environment,
        } => {
            let root = project_root(state, &project_path)?;
            write_link(&root, &workspace_id, &environment)?;
            let mut reply = link_reply(&root);
            reply["projectPath"] = json!(project_path);
            Ok(reply)
        }
        Action::ProjectLink { project_path } => {
            let root = project_root(state, &project_path)?;
            let mut reply = link_reply(&root);
            reply["projectPath"] = json!(project_path);
            Ok(reply)
        }
        Action::ListKeys {
            project_path,
            environment,
        } => {
            let workspace = linked(state, &project_path, &environment)?;
            let (status, reply) =
                api(Method::GET, secrets_url(&workspace, &environment)?, None).await?;
            if !(200..300).contains(&status) {
                return Err(http_status_error(status));
            }
            Ok(json!({ "kind": "keys", "environment": environment, "keys": keys_of(&reply) }))
        }
        Action::SaveSecret {
            project_path,
            environment,
            key,
            value,
        } => {
            let workspace = linked(state, &project_path, &environment)?;
            save_secret(&workspace, &environment, &key, &value).await?;
            Ok(json!({ "kind": "saved" }))
        }
        Action::TerminalEnv { session_id } => {
            let source = terminal_source(state, &session_id)?;
            let mut env = serde_json::Map::new();
            for (key, value) in terminal_env_pairs(source).await? {
                env.insert(key, Value::String(value));
            }
            Ok(json!({ "kind": "terminalEnv", "env": env }))
        }
        Action::Answer {
            request_id,
            project_path,
            environment,
            key,
        } => {
            let workspace = linked(state, &project_path, &environment)?;
            let value = read_secret(&workspace, &environment, &key).await?;
            crate::secrets::respond(&request_id, Some(value))?;
            Ok(json!({ "kind": "answered" }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_built_for_the_documented_endpoints() {
        assert_eq!(
            login_body("id", "secret"),
            json!({"clientId": "id", "clientSecret": "secret"})
        );
        let list = secrets_url("ws-1", "dev").unwrap();
        assert_eq!(list.path(), "/api/v3/secrets/raw");
        assert_eq!(
            list.query(),
            Some("workspaceId=ws-1&environment=dev&secretPath=%2F")
        );
        let one = secret_url("ws-1", "dev", "API_KEY").unwrap();
        assert_eq!(one.path(), "/api/v3/secrets/raw/API_KEY");
        assert_eq!(one.host_str(), Some("app.infisical.com"));
        assert_eq!(
            save_body("ws-1", "prod", "v"),
            json!({"workspaceId": "ws-1", "environment": "prod", "secretPath": "/", "secretValue": "v"})
        );
        // A name can never escape the path.
        let odd = secret_url("ws", "dev", "a/../b").unwrap();
        assert!(odd.path().ends_with("a%2F..%2Fb"));
    }

    #[test]
    fn the_link_file_round_trips_and_keeps_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_link(dir.path()), None);
        write_link(dir.path(), "abc-123", "dev").unwrap();
        assert_eq!(
            read_link(dir.path()),
            Some(("abc-123".into(), "dev".into()))
        );
        std::fs::write(
            dir.path().join(LINK_FILE),
            r#"{"workspaceId":"old","defaultEnvironment":"dev","gitBranchToEnvironmentMapping":{"main":"prod"}}"#,
        )
        .unwrap();
        write_link(dir.path(), "new-id", "staging").unwrap();
        let file: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(LINK_FILE)).unwrap()).unwrap();
        assert_eq!(file["workspaceId"], "new-id");
        assert_eq!(file["defaultEnvironment"], "staging");
        assert_eq!(file["gitBranchToEnvironmentMapping"]["main"], "prod");
        assert!(write_link(dir.path(), "../x", "dev").is_err());
        assert!(write_link(dir.path(), "ok", "d e v").is_err());
    }

    #[test]
    fn key_lists_carry_names_only() {
        let reply = json!({"secrets": [
            {"secretKey": "STRIPE_KEY", "secretValue": "sk_live_hunter2", "version": 2},
            {"secretKey": "A_KEY", "secretValue": "another-value"}
        ]});
        let names = keys_of(&reply);
        assert_eq!(names, vec!["A_KEY", "STRIPE_KEY"]);
        let shown = serde_json::to_string(&names).unwrap();
        assert!(!shown.contains("hunter2") && !shown.contains("another-value"));
    }

    #[test]
    fn terminal_keys_keep_values_and_skip_unusable_names() {
        let reply = json!({"secrets": [
            {"secretKey": "STRIPE_KEY", "secretValue": "sk_live_hunter2"},
            {"secretKey": "bad-name", "secretValue": "x"},
            {"secretKey": "NO_VALUE"},
            {"secretKey": "NUL", "secretValue": "a\u{0}b"}
        ]});
        assert_eq!(
            env_pairs_of(&reply),
            vec![("STRIPE_KEY".to_owned(), "sk_live_hunter2".to_owned())]
        );
    }

    #[test]
    fn projects_come_with_their_environments() {
        let reply = json!({"workspaces": [{"id": "w1", "name": "App", "environments": [
            {"slug": "dev", "name": "Development"}, {"slug": "prod", "name": "Production"}
        ]}, {"name": "no id"}]});
        let projects = projects_of(&reply);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["environments"][1]["slug"], "prod");
    }

    #[test]
    fn a_script_is_wrapped_with_the_runner_and_quoted() {
        let wrapped = wrap_script("echo 'hi'\nnpm run dev", "dev", Some("ws-1"));
        assert!(wrapped.contains("infisical-run --env=dev --projectId=ws-1 -- "));
        assert!(wrapped.ends_with("-c 'echo '\\''hi'\\''\nnpm run dev'"));
        assert!(!wrap_script("ls", "dev", Some("a b; rm")).contains("--projectId"));
    }
}
