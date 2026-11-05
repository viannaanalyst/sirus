//! Connectors (ADR-137): remote MCP servers (Supabase, Linear, Notion, a custom URL…) the
//! person connects once in Settings and every supported agent then receives, for all projects
//! or for one project. The list lives in the settings; sign-in tokens never do. Sirus signs in
//! itself with the MCP authorization flow (protected-resource metadata, dynamic client
//! registration, PKCE, a loopback redirect), keeps the tokens in the macOS Keychain (a private
//! file elsewhere), refreshes them when a session starts, and hands each session only the
//! connectors of its project: Claude in its `--mcp-config` file, Codex through `-c` overrides
//! with the token in its environment, ACP agents in `mcpServers` when they take HTTP servers.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::{AgentProviderId, Session};
use crate::runtime::{AppHandle, State};

pub const MAX_CONNECTORS: usize = 64;
const MAX_URL: usize = 2048;
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
/// A token this close to expiring is refreshed before a session starts.
const REFRESH_MARGIN_SECS: i64 = 120;

/// Agents whose sessions can take a connector: Claude and Codex by their own MCP options,
/// the ACP agents through `mcpServers` when they advertise HTTP support.
pub const SUPPORTED: [AgentProviderId; 6] = [
    AgentProviderId::Claude,
    AgentProviderId::Codex,
    AgentProviderId::OpenCode,
    AgentProviderId::Cursor,
    AgentProviderId::Devin,
    AgentProviderId::Hermes,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ConnectorAuth {
    /// The service's own sign-in (MCP authorization).
    #[default]
    Oauth,
    /// A token the person pasted, sent as `Authorization: Bearer`.
    Token,
    /// An open server.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connector {
    pub id: String,
    /// The catalog entry it came from (`supabase`), or none for a custom URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    pub name: String,
    pub url: String,
    /// Only this project's sessions receive it; none means every project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default)]
    pub agents: Vec<AgentProviderId>,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub auth: ConnectorAuth,
}

fn enabled() -> bool {
    true
}

fn invalid(message: &str) -> Error {
    Error::new("invalid_settings", message)
}

pub fn validate(list: &[Connector]) -> Result<()> {
    if list.len() > MAX_CONNECTORS {
        return Err(invalid("Too many connectors."));
    }
    let mut ids = HashSet::new();
    for connector in list {
        let id_ok = !connector.id.is_empty()
            && connector.id.len() <= 64
            && connector
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !id_ok || !ids.insert(connector.id.as_str()) {
            return Err(invalid("Invalid connector id."));
        }
        if connector.name.trim().is_empty()
            || connector.name.chars().count() > 64
            || connector.name.chars().any(char::is_control)
        {
            return Err(invalid("A connector needs a short name."));
        }
        if !valid_url(&connector.url) {
            return Err(invalid("A connector's address must be an https URL."));
        }
        if connector.catalog_id.as_deref().is_some_and(|id| {
            id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        }) || connector
            .project_id
            .as_deref()
            .is_some_and(|id| id.len() > 128)
        {
            return Err(invalid("Invalid connector."));
        }
        let agents: HashSet<_> = connector.agents.iter().collect();
        if agents.len() != connector.agents.len()
            || connector
                .agents
                .iter()
                .any(|agent| !SUPPORTED.contains(agent))
        {
            return Err(invalid("Invalid connector agents."));
        }
    }
    Ok(())
}

fn valid_url(url: &str) -> bool {
    url.len() <= MAX_URL
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
        && reqwest::Url::parse(url).is_ok_and(|parsed| {
            parsed.scheme() == "https"
                && parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none()
        })
}

// ---- Configuration snapshot (read when sessions start) ----

static CONFIG: OnceLock<RwLock<Vec<Connector>>> = OnceLock::new();
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

fn config() -> &'static RwLock<Vec<Connector>> {
    CONFIG.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn init(data_dir: &Path, list: &[Connector]) {
    let _ = DATA_DIR.set(data_dir.to_path_buf());
    set(list);
}

pub fn set(list: &[Connector]) {
    *config().write() = list.to_vec();
}

/// Catalog services that moved: (catalog id, old address, new address). Cloudflare retired its
/// Workers bindings server for one that covers the whole API, DNS and zones included.
const MOVED: &[(&str, &str, &str)] = &[(
    "cloudflare",
    "https://bindings.mcp.cloudflare.com/mcp",
    "https://mcp.cloudflare.com/mcp",
)];

/// Points connectors at the new address of a moved service. Returns the ids that changed.
fn move_retired(list: &mut [Connector]) -> Vec<String> {
    let mut moved = Vec::new();
    for connector in list.iter_mut() {
        let target = MOVED.iter().find(|(catalog, old, _)| {
            connector.catalog_id.as_deref() == Some(*catalog) && connector.url == *old
        });
        if let Some((_, _, new)) = target {
            connector.url = (*new).to_owned();
            moved.push(connector.id.clone());
        }
    }
    moved
}

/// `init`, for startup: first moves retired connectors and saves that. Their tokens belong to the
/// old server, so they go only once the new address is saved; otherwise the next launch, still
/// seeing the old address, would erase the sign-in made in between.
pub fn start(data_dir: &Path, state_path: &Path, data: &mut crate::models::AppData) {
    let moved = move_retired(&mut data.settings.connectors);
    init(data_dir, &data.settings.connectors);
    if !moved.is_empty() && crate::persist::save(state_path, data).is_ok() {
        for id in &moved {
            vault::delete(id);
        }
    }
}

// ---- Token vault ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Tokens {
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
}

#[cfg(target_os = "macos")]
mod vault {
    //! The macOS Keychain through the Security framework: values of any length, never on a
    //! command line. (The `security` tool's interactive mode cut values past about 4 KB, which
    //! long OAuth tokens reach, and reported every save as failed.)
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    const KIND: &str = "connectors";

    pub fn get(id: &str) -> Option<String> {
        get_generic_password(&crate::keychain_scope::service(KIND), id)
            .ok()
            .or_else(|| crate::keychain_scope::adopt_shared(KIND, id))
            .and_then(|bytes| String::from_utf8(bytes).ok())
    }

    pub fn put(id: &str, value: &str) -> bool {
        // An item the old `security` path left belongs to that tool; replace it rather than fail.
        let service = crate::keychain_scope::service(KIND);
        set_generic_password(&service, id, value.as_bytes()).is_ok()
            || (delete_generic_password(&service, id).is_ok()
                && set_generic_password(&service, id, value.as_bytes()).is_ok())
    }

    pub fn delete(id: &str) {
        let _ = delete_generic_password(&crate::keychain_scope::service(KIND), id);
    }
}

#[cfg(not(target_os = "macos"))]
mod vault {
    //! A private file next to the state on servers (owner read and write only).
    use std::collections::BTreeMap;

    fn path() -> Option<std::path::PathBuf> {
        super::DATA_DIR
            .get()
            .map(|dir| dir.join("connector-tokens.json"))
    }
    fn read() -> BTreeMap<String, String> {
        path()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
    fn write(map: &BTreeMap<String, String>) -> bool {
        let Some(path) = path() else { return false };
        let Ok(bytes) = serde_json::to_vec(map) else {
            return false;
        };
        let temp = path.with_extension("json.tmp");
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temp)
        };
        #[cfg(not(unix))]
        let file = std::fs::File::create(&temp);
        file.and_then(|mut file| std::io::Write::write_all(&mut file, &bytes))
            .and_then(|_| std::fs::rename(&temp, &path))
            .is_ok()
    }
    pub fn get(id: &str) -> Option<String> {
        read().get(id).cloned()
    }
    pub fn put(id: &str, value: &str) -> bool {
        let mut map = read();
        map.insert(id.to_owned(), value.to_owned());
        write(&map)
    }
    pub fn delete(id: &str) {
        let mut map = read();
        if map.remove(id).is_some() {
            write(&map);
        }
    }
}

fn load_tokens(id: &str) -> Option<Tokens> {
    let encoded = vault::get(id)?;
    serde_json::from_slice(&STANDARD.decode(encoded).ok()?).ok()
}

fn save_tokens(id: &str, tokens: &Tokens) -> Result<()> {
    let encoded = STANDARD.encode(serde_json::to_vec(tokens)?);
    if vault::put(id, &encoded) {
        Ok(())
    } else {
        Err(Error::new("connector", "The sign-in could not be saved."))
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ---- HTTP ----

fn http() -> Result<&'static reqwest::Client> {
    static HTTP: OnceLock<std::result::Result<reqwest::Client, String>> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::limited(3))
            .connect_timeout(Duration::from_secs(6))
            .timeout(Duration::from_secs(15))
            .user_agent("Sirus Code")
            .build()
            .map_err(|error| error.to_string())
    })
    .as_ref()
    .map_err(|error| Error::new("connector", error.clone()))
}

fn network(error: reqwest::Error) -> Error {
    Error::new(
        "connector",
        format!("The service could not be reached ({error})."),
    )
}

async fn get_json(url: &str) -> Option<Value> {
    let response = http()
        .ok()?
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    serde_json::from_slice(&response.bytes().await.ok()?).ok()
}

/// `application/x-www-form-urlencoded` body of some pairs.
fn form(pairs: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse("http://form.invalid/").expect("static url");
    url.query_pairs_mut().extend_pairs(pairs);
    url.query().unwrap_or_default().to_owned()
}

async fn post_form(url: &str, pairs: &[(&str, &str)]) -> Result<Value> {
    let response = http()?
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(form(pairs))
        .send()
        .await
        .map_err(network)?;
    let status = response.status();
    let body: Value =
        serde_json::from_slice(&response.bytes().await.map_err(network)?).unwrap_or(Value::Null);
    if !status.is_success() {
        let reason = body["error_description"]
            .as_str()
            .or(body["error"].as_str())
            .unwrap_or("rejected");
        return Err(Error::new(
            "connector",
            format!("The service refused the sign-in: {reason}."),
        ));
    }
    Ok(body)
}

// ---- Discovery (MCP authorization, RFC 9728 and RFC 8414) ----

#[derive(Debug, Clone, PartialEq, Eq)]
struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    scopes: Vec<String>,
    /// The MCP server's canonical URI, the token's audience (RFC 8707).
    resource: String,
}

/// The server URL without its query and fragment.
fn canonical(url: &reqwest::Url) -> String {
    let mut url = url.clone();
    url.set_query(None);
    url.set_fragment(None);
    url.to_string().trim_end_matches('/').to_owned()
}

/// `resource_metadata="…"` from a `WWW-Authenticate` challenge.
fn metadata_from_challenge(header: &str) -> Option<String> {
    let start = header.find("resource_metadata=\"")? + "resource_metadata=\"".len();
    let end = header[start..].find('"')?;
    Some(header[start..start + end].to_owned())
}

/// Well-known URLs for an issuer or resource: the suffix goes after the host and before
/// the path (RFC 8414 §3.1), then the bare forms.
fn well_known(base: &reqwest::Url, suffix: &str) -> Vec<String> {
    let origin = format!(
        "{}://{}",
        base.scheme(),
        base.host_str().unwrap_or_default()
    ) + &base
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let path = base.path().trim_end_matches('/');
    let mut urls = Vec::new();
    if !path.is_empty() {
        urls.push(format!("{origin}/.well-known/{suffix}{path}"));
    }
    urls.push(format!("{origin}/.well-known/{suffix}"));
    if !path.is_empty() {
        urls.push(format!("{origin}{path}/.well-known/{suffix}"));
    }
    urls
}

async fn discover(server: &str) -> Result<Discovery> {
    let url = reqwest::Url::parse(server).map_err(|_| invalid("Invalid connector address."))?;
    // An unauthorized request names the resource metadata in its challenge.
    let probe = http()?
        .post(url.clone())
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"Sirus Code","version":"1"}}}).to_string())
        .send()
        .await
        .map_err(network)?;
    let challenged = probe
        .headers()
        .get_all(reqwest::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(metadata_from_challenge);
    let mut candidates: Vec<String> = challenged.into_iter().collect();
    candidates.extend(well_known(&url, "oauth-protected-resource"));
    let mut resource_metadata = None;
    for candidate in candidates {
        if let Some(value) = get_json(&candidate)
            .await
            .filter(|value| value["authorization_servers"].is_array())
        {
            resource_metadata = Some(value);
            break;
        }
    }
    let issuer = resource_metadata
        .as_ref()
        .and_then(|value| value["authorization_servers"][0].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default()));
    let scopes = resource_metadata
        .as_ref()
        .and_then(|value| value["scopes_supported"].as_array())
        .map(|scopes| {
            scopes
                .iter()
                .filter_map(|scope| scope.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let issuer_url = reqwest::Url::parse(&issuer)
        .map_err(|_| Error::new("connector", "The service's sign-in address is invalid."))?;
    let mut candidates = well_known(&issuer_url, "oauth-authorization-server");
    candidates.extend(well_known(&issuer_url, "openid-configuration"));
    for candidate in candidates {
        let Some(meta) = get_json(&candidate).await else {
            continue;
        };
        let (Some(authorization), Some(token)) = (
            meta["authorization_endpoint"].as_str(),
            meta["token_endpoint"].as_str(),
        ) else {
            continue;
        };
        return Ok(Discovery {
            authorization_endpoint: authorization.to_owned(),
            token_endpoint: token.to_owned(),
            registration_endpoint: meta["registration_endpoint"].as_str().map(str::to_owned),
            scopes,
            resource: resource_metadata
                .as_ref()
                .and_then(|value| value["resource"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| canonical(&url)),
        });
    }
    Err(Error::new(
        "connector",
        "This service does not offer a sign-in Sirus can use. Add it with a token instead.",
    ))
}

// ---- Sign-in ----

fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

async fn register(endpoint: &str, redirect: &str) -> Result<(String, Option<String>)> {
    let response = http()?
        .post(endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(
            json!({
                "client_name": "Sirus Code",
                "redirect_uris": [redirect],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            })
            .to_string(),
        )
        .send()
        .await
        .map_err(network)?;
    let status = response.status();
    let body: Value =
        serde_json::from_slice(&response.bytes().await.map_err(network)?).unwrap_or(Value::Null);
    let client_id = body["client_id"]
        .as_str()
        .filter(|_| status.is_success())
        .ok_or_else(|| {
            Error::new(
                "connector",
                "The service did not register Sirus Code for sign-in.",
            )
        })?;
    Ok((
        client_id.to_owned(),
        body["client_secret"].as_str().map(str::to_owned),
    ))
}

fn tokens_from(
    body: &Value,
    previous: Option<&Tokens>,
    discovery_token: &str,
    client_id: &str,
    client_secret: Option<&str>,
    resource: &str,
) -> Result<Tokens> {
    let access = body["access_token"]
        .as_str()
        .filter(|token| !token.is_empty() && token.len() < 16 * 1024)
        .ok_or_else(|| Error::new("connector", "The service sent no access token."))?;
    Ok(Tokens {
        access_token: access.to_owned(),
        refresh_token: body["refresh_token"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| previous.and_then(|tokens| tokens.refresh_token.clone())),
        expires_at: body["expires_in"].as_i64().map(|seconds| now() + seconds),
        token_endpoint: Some(discovery_token.to_owned()),
        client_id: Some(client_id.to_owned()),
        client_secret: client_secret.map(str::to_owned),
        resource: Some(resource.to_owned()),
    })
}

/// The one request a loopback redirect brings: `code` and `state` from `/callback`.
async fn wait_for_code(listener: tokio::net::TcpListener, state: &str) -> Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut buffer = vec![0u8; 8192];
        let mut read = 0;
        while read < buffer.len() {
            let count = stream.read(&mut buffer[read..]).await?;
            if count == 0 {
                break;
            }
            read += count;
            if buffer[..read]
                .windows(4)
                .any(|window| window == b"\r\n\r\n")
            {
                break;
            }
        }
        let head = String::from_utf8_lossy(&buffer[..read]).into_owned();
        let target = head.split_whitespace().nth(1).unwrap_or("/");
        let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
            continue;
        };
        if url.path() != "/callback" {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            continue;
        }
        let query = |key: &str| {
            url.query_pairs()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
        };
        let ok = query("state").as_deref() == Some(state) && query("code").is_some();
        let page = if ok {
            "<!doctype html><meta charset=utf-8><title>Sirus Code</title><body style=\"font:16px -apple-system,sans-serif;background:#111;color:#eee;display:grid;place-items:center;height:100vh;margin:0\"><p>Pronto. Pode voltar ao Sirus Code.<br><small style=\"color:#888\">Done. You can return to Sirus Code.</small></p>"
        } else {
            "<!doctype html><meta charset=utf-8><title>Sirus Code</title><body style=\"font:16px -apple-system,sans-serif;background:#111;color:#eee;display:grid;place-items:center;height:100vh;margin:0\"><p>O login não foi concluído. Tente de novo no Sirus Code.<br><small style=\"color:#888\">The sign-in did not finish. Try again in Sirus Code.</small></p>"
        };
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len());
        let _ = stream.write_all(response.as_bytes()).await;
        if query("state").as_deref() != Some(state) {
            continue;
        }
        if let Some(error) = query("error") {
            return Err(Error::new(
                "connector",
                format!(
                    "The service refused the sign-in: {}.",
                    query("error_description").unwrap_or(error)
                ),
            ));
        }
        if let Some(code) = query("code") {
            return Ok(code);
        }
    }
}

async fn sign_in(connector: &Connector, open: impl FnOnce(&str) -> Result<()>) -> Result<()> {
    let discovery = discover(&connector.url).await?;
    let registration = discovery.registration_endpoint.clone().ok_or_else(|| {
        Error::new(
            "connector",
            "This service does not let apps register for sign-in. Add it with a token instead.",
        )
    })?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let redirect = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr()?.port()
    );
    let (client_id, client_secret) = register(&registration, &redirect).await?;
    let verifier = random_token();
    let state = random_token();
    let mut authorize = reqwest::Url::parse(&discovery.authorization_endpoint)
        .map_err(|_| Error::new("connector", "The service's sign-in address is invalid."))?;
    {
        let mut query = authorize.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("code_challenge", &challenge(&verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", &discovery.resource);
        if !discovery.scopes.is_empty() {
            query.append_pair("scope", &discovery.scopes.join(" "));
        }
    }
    if authorize.scheme() != "https" {
        return Err(Error::new(
            "connector",
            "The service's sign-in page is not secure.",
        ));
    }
    open(authorize.as_str())?;
    let code = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_code(listener, &state))
        .await
        .map_err(|_| Error::new("connector", "The sign-in took too long. Try again."))??;
    let mut pairs = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("client_id", client_id.as_str()),
        ("code_verifier", verifier.as_str()),
        ("resource", discovery.resource.as_str()),
    ];
    if let Some(secret) = client_secret.as_deref() {
        pairs.push(("client_secret", secret));
    }
    let body = post_form(&discovery.token_endpoint, &pairs).await?;
    let tokens = tokens_from(
        &body,
        None,
        &discovery.token_endpoint,
        &client_id,
        client_secret.as_deref(),
        &discovery.resource,
    )?;
    save_tokens(&connector.id, &tokens)
}

/// A usable access token: refreshed first when it is about to expire. None when signed out,
/// or when the refresh failed and the old token has run out (the person signs in again).
async fn access_token(id: &str) -> Option<String> {
    let tokens = load_tokens(id)?;
    if !expiring(&tokens) {
        return Some(tokens.access_token);
    }
    // Sessions starting together must not each spend the same refresh token: providers
    // that rotate it would refuse every refresh but the first (and may revoke them all).
    let gate = refresh_gate(id);
    let _turn = gate.lock().await;
    // Another session may have refreshed while this one waited.
    let tokens = load_tokens(id)?;
    if !expiring(&tokens) {
        return Some(tokens.access_token);
    }
    match refresh(&tokens).await {
        Some(fresh) => {
            // A token that could not be stored still works for this session.
            let _ = save_tokens(id, &fresh);
            Some(fresh.access_token)
        }
        // A failed refresh keeps the old token for the time it has left.
        None => still_valid(&tokens).then_some(tokens.access_token),
    }
}

fn expiring(tokens: &Tokens) -> bool {
    tokens
        .expires_at
        .is_some_and(|at| at - now() < REFRESH_MARGIN_SECS)
}

fn still_valid(tokens: &Tokens) -> bool {
    tokens.expires_at.is_none_or(|at| at > now())
}

/// One refresh at a time per connector.
fn refresh_gate(id: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static GATES: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let mut gates = GATES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    gates.entry(id.to_owned()).or_default().clone()
}

async fn refresh(tokens: &Tokens) -> Option<Tokens> {
    let (Some(refresh), Some(endpoint), Some(client_id)) = (
        tokens.refresh_token.as_deref(),
        tokens.token_endpoint.as_deref(),
        tokens.client_id.as_deref(),
    ) else {
        return None;
    };
    let resource = tokens.resource.clone().unwrap_or_default();
    let mut pairs = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", client_id),
    ];
    if !resource.is_empty() {
        pairs.push(("resource", resource.as_str()));
    }
    if let Some(secret) = tokens.client_secret.as_deref() {
        pairs.push(("client_secret", secret));
    }
    let body = post_form(endpoint, &pairs).await.ok()?;
    tokens_from(
        &body,
        Some(tokens),
        endpoint,
        client_id,
        tokens.client_secret.as_deref(),
        &resource,
    )
    .ok()
}

// ---- Status and the command ----

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorStatus {
    pub id: String,
    /// `ready` (no sign-in needed), `connected`, `signedOut` or `expired`.
    pub state: &'static str,
}

fn status_of(connector: &Connector) -> ConnectorStatus {
    let state = match connector.auth {
        ConnectorAuth::None => "ready",
        _ => match load_tokens(&connector.id) {
            None => "signedOut",
            Some(tokens)
                if tokens.refresh_token.is_none()
                    && tokens.expires_at.is_some_and(|at| at <= now()) =>
            {
                "expired"
            }
            Some(_) => "connected",
        },
    };
    ConnectorStatus {
        id: connector.id.clone(),
        state,
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
    /// Each saved connector's sign-in state.
    Status,
    /// Signs in through the service's page in the browser (the Mac only).
    SignIn { id: String },
    /// Keeps a pasted token for a connector that uses one.
    SetToken { id: String, token: String },
    /// Forgets the connector's sign-in.
    SignOut { id: String },
}

fn find(state: &AppState, id: &str) -> Result<Connector> {
    state
        .data
        .lock()
        .settings
        .connectors
        .iter()
        .find(|connector| connector.id == id)
        .cloned()
        .ok_or_else(|| Error::not_found("Connector not found."))
}

fn statuses(state: &AppState) -> Vec<ConnectorStatus> {
    let list = state.data.lock().settings.connectors.clone();
    list.iter().map(status_of).collect()
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn connector_action(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    action: Action,
) -> Result<Vec<ConnectorStatus>> {
    let state = state.inner().clone();
    state.ensure_running()?;
    match action {
        Action::Status => {}
        Action::SignIn { id } => {
            let connector = find(&state, &id)?;
            sign_in(&connector, |url| open_browser(&app, url)).await?;
        }
        Action::SetToken { id, token } => {
            let connector = find(&state, &id)?;
            let token = token.trim().trim_start_matches("Bearer ").trim().to_owned();
            if token.is_empty()
                || token.len() > 16 * 1024
                || token.chars().any(|c| c.is_control() || c.is_whitespace())
            {
                return Err(invalid("Paste the token without spaces."));
            }
            save_tokens(
                &connector.id,
                &Tokens {
                    access_token: token,
                    refresh_token: None,
                    expires_at: None,
                    token_endpoint: None,
                    client_id: None,
                    client_secret: None,
                    resource: None,
                },
            )?;
        }
        Action::SignOut { id } => {
            if !id.is_empty()
                && id.len() <= 64
                && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                vault::delete(&id);
            }
        }
    }
    let state = state.clone();
    crate::commands::native_task(move || Ok(statuses(&state))).await
}

#[cfg(feature = "desktop")]
fn open_browser(app: &AppHandle, url: &str) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| Error::new("connector", error.to_string()))
}

#[cfg(not(feature = "desktop"))]
fn open_browser(_app: &AppHandle, _url: &str) -> Result<()> {
    // The redirect comes back to this machine's loopback, which a phone cannot reach.
    Err(Error::new(
        "connector",
        "Sign in to connectors from the Mac app; a token works here.",
    ))
}

// ---- What a session receives ----

/// A connector as one session receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Active {
    pub name: String,
    pub url: String,
    pub bearer: Option<String>,
}

/// An MCP server name agents accept: `[a-z0-9_]`, at most 40 characters.
pub fn server_name(name: &str) -> String {
    let mut out: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_').chars().take(40).collect::<String>();
    if out.is_empty() || out.starts_with("sirus") {
        format!("connector_{out}").trim_end_matches('_').to_owned()
    } else {
        out
    }
}

/// The connectors a session of this provider and project receives, with fresh tokens.
/// Signed-out ones are left out; the session runs without them.
pub async fn for_session(session: &Session) -> Vec<Active> {
    let list: Vec<Connector> = config()
        .read()
        .iter()
        .filter(|connector| {
            connector.enabled
                && connector.agents.contains(&session.agent)
                && connector
                    .project_id
                    .as_deref()
                    .is_none_or(|project| project == session.project_id)
        })
        .cloned()
        .collect();
    let mut used = HashSet::new();
    let mut active = Vec::new();
    for connector in list {
        let bearer = match connector.auth {
            ConnectorAuth::None => None,
            _ => match access_token(&connector.id).await {
                Some(token) => Some(token),
                None => continue,
            },
        };
        let base = server_name(&connector.name);
        let mut name = base.clone();
        let mut index = 2;
        while !used.insert(name.clone()) {
            name = format!("{base}_{index}");
            index += 1;
        }
        active.push(Active {
            name,
            url: connector.url,
            bearer,
        });
    }
    active
}

/// Part of a kept agent process's key: a change of connectors or tokens starts a new one.
pub fn fingerprint(active: &[Active]) -> String {
    let mut hash = Sha256::new();
    for connector in active {
        hash.update(connector.name.as_bytes());
        hash.update([0]);
        hash.update(connector.url.as_bytes());
        hash.update([0]);
        hash.update(connector.bearer.as_deref().unwrap_or_default().as_bytes());
        hash.update([1]);
    }
    URL_SAFE_NO_PAD.encode(&hash.finalize()[..12])
}

/// Claude `mcpServers` entries (`--mcp-config`, a private file).
pub fn claude_servers(active: &[Active]) -> serde_json::Map<String, Value> {
    active
        .iter()
        .map(|connector| {
            let mut server = json!({ "type": "http", "url": connector.url });
            if let Some(token) = &connector.bearer {
                server["headers"] = json!({ "Authorization": format!("Bearer {token}") });
            }
            (connector.name.clone(), server)
        })
        .collect()
}

/// Codex `-c` overrides and the environment holding their tokens (never argv, ADR-100).
pub fn codex_overrides(active: &[Active]) -> (Vec<String>, Vec<(String, String)>) {
    let mut args = Vec::new();
    let mut env = Vec::new();
    for (index, connector) in active.iter().enumerate() {
        let prefix = format!("mcp_servers.{}", connector.name);
        args.push(format!("{prefix}.url={:?}", connector.url));
        if let Some(token) = &connector.bearer {
            let variable = format!("SIRUS_CONNECTOR_TOKEN_{index}");
            args.push(format!("{prefix}.bearer_token_env_var={variable:?}"));
            env.push((variable, token.clone()));
        }
    }
    (args, env)
}

/// ACP `mcpServers` entries, for agents that advertise HTTP MCP support.
pub fn acp_servers(active: &[Active], init: &Value) -> Vec<Value> {
    if init["agentCapabilities"]["mcpCapabilities"]["http"] != true {
        return Vec::new();
    }
    active
        .iter()
        .map(|connector| {
            let headers: Vec<Value> = connector
                .bearer
                .iter()
                .map(|token| json!({ "name": "Authorization", "value": format!("Bearer {token}") }))
                .collect();
            json!({ "type": "http", "name": connector.name, "url": connector.url, "headers": headers })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_refresh_keeps_the_old_token_while_it_lasts_and_refreshes_take_turns() {
        let tokens = |expires_in: i64| -> Tokens {
            serde_json::from_value(json!({"accessToken":"old","expiresAt": now() + expires_in}))
                .unwrap()
        };
        assert!(expiring(&tokens(60)) && still_valid(&tokens(60)));
        assert!(!expiring(&tokens(3600)));
        assert!(!still_valid(&tokens(-5)));
        assert!(std::sync::Arc::ptr_eq(
            &refresh_gate("a"),
            &refresh_gate("a")
        ));
        assert!(!std::sync::Arc::ptr_eq(
            &refresh_gate("a"),
            &refresh_gate("b")
        ));
    }

    /// Writes to the real Keychain, so it runs only when asked (`--ignored`).
    #[test]
    #[ignore]
    fn keychain_keeps_long_sign_ins_whole() {
        let id = "sirus-test-long-token";
        let value = "a".repeat(6000);
        assert!(vault::put(id, &value));
        assert_eq!(vault::get(id).as_deref(), Some(value.as_str()));
        assert!(vault::put(id, "short"));
        assert_eq!(vault::get(id).as_deref(), Some("short"));
        vault::delete(id);
        assert_eq!(vault::get(id), None);
    }

    fn connector(id: &str, name: &str) -> Connector {
        Connector {
            id: id.into(),
            catalog_id: Some("supabase".into()),
            name: name.into(),
            url: "https://mcp.supabase.com/mcp?project_ref=abc&read_only=true".into(),
            project_id: None,
            agents: vec![AgentProviderId::Claude, AgentProviderId::Codex],
            enabled: true,
            auth: ConnectorAuth::Oauth,
        }
    }

    #[test]
    fn validation_bounds_urls_ids_and_agents() {
        assert!(validate(&[connector("a-1", "Supabase")]).is_ok());
        assert!(validate(&[connector("a-1", "Supabase"), connector("a-1", "Other")]).is_err());
        let mut plain = connector("a-2", "x");
        plain.url = "http://example.com/mcp".into();
        assert!(validate(&[plain]).is_err());
        let mut user = connector("a-3", "x");
        user.url = "https://me:secret@example.com/mcp".into();
        assert!(validate(&[user]).is_err());
        let mut grok = connector("a-4", "x");
        grok.agents = vec![AgentProviderId::Grok];
        assert!(validate(&[grok]).is_err());
        let mut bad = connector("a 5", "x");
        bad.id = "a 5".into();
        assert!(validate(&[bad]).is_err());
    }

    #[test]
    fn names_are_safe_and_never_sirus() {
        assert_eq!(server_name("Supabase · dp-prod"), "supabase_dp_prod");
        assert_eq!(server_name("sirus_browser"), "connector_sirus_browser");
        assert_eq!(server_name("***"), "connector");
    }

    #[test]
    fn well_known_urls_follow_rfc_8414() {
        let issuer = reqwest::Url::parse("https://access.stripe.com/mcp").unwrap();
        assert_eq!(
            well_known(&issuer, "oauth-authorization-server")[0],
            "https://access.stripe.com/.well-known/oauth-authorization-server/mcp"
        );
        let bare = reqwest::Url::parse("https://mcp.linear.app").unwrap();
        assert_eq!(
            well_known(&bare, "oauth-authorization-server"),
            vec!["https://mcp.linear.app/.well-known/oauth-authorization-server".to_string()]
        );
        assert_eq!(metadata_from_challenge("Bearer realm=\"OAuth\", resource_metadata=\"https://x.dev/.well-known/oauth-protected-resource/mcp\""), Some("https://x.dev/.well-known/oauth-protected-resource/mcp".into()));
        assert_eq!(
            canonical(&reqwest::Url::parse("https://mcp.supabase.com/mcp?project_ref=a").unwrap()),
            "https://mcp.supabase.com/mcp"
        );
    }

    #[test]
    fn sessions_get_each_agent_its_own_form() {
        let active = vec![
            Active {
                name: "supabase".into(),
                url: "https://mcp.supabase.com/mcp".into(),
                bearer: Some("t1".into()),
            },
            Active {
                name: "context7".into(),
                url: "https://mcp.context7.com/mcp".into(),
                bearer: None,
            },
        ];
        let claude = claude_servers(&active);
        assert_eq!(claude["supabase"]["headers"]["Authorization"], "Bearer t1");
        assert!(claude["context7"].get("headers").is_none());
        let (args, env) = codex_overrides(&active);
        assert!(
            args.contains(&"mcp_servers.supabase.url=\"https://mcp.supabase.com/mcp\"".to_string())
        );
        assert!(args.contains(
            &"mcp_servers.supabase.bearer_token_env_var=\"SIRUS_CONNECTOR_TOKEN_0\"".to_string()
        ));
        assert!(
            !args.iter().any(|arg| arg.contains("t1")),
            "tokens stay out of argv"
        );
        assert_eq!(
            env,
            vec![("SIRUS_CONNECTOR_TOKEN_0".to_string(), "t1".to_string())]
        );
        assert!(acp_servers(&active, &json!({})).is_empty());
        let acp = acp_servers(
            &active,
            &json!({"agentCapabilities":{"mcpCapabilities":{"http":true}}}),
        );
        assert_eq!(acp[0]["headers"][0]["value"], "Bearer t1");
        assert_ne!(fingerprint(&active), fingerprint(&active[..1]));
    }

    #[test]
    fn retired_catalog_addresses_move_and_others_stay() {
        let connector = |id: &str, catalog: Option<&str>, url: &str| Connector {
            id: id.to_owned(),
            catalog_id: catalog.map(str::to_owned),
            name: id.to_owned(),
            url: url.to_owned(),
            project_id: None,
            agents: Vec::new(),
            enabled: true,
            auth: ConnectorAuth::default(),
        };
        const OLD: &str = "https://bindings.mcp.cloudflare.com/mcp";
        let mut list = vec![
            connector("a", Some("cloudflare"), OLD),
            connector("b", Some("cloudflare"), "https://mcp.cloudflare.com/mcp"),
            connector("c", None, OLD),
            connector("d", Some("supabase"), "https://mcp.supabase.com/mcp"),
        ];
        assert_eq!(move_retired(&mut list), vec!["a".to_string()]);
        assert_eq!(list[0].url, "https://mcp.cloudflare.com/mcp");
        assert_eq!(
            list[2].url, OLD,
            "a custom connector keeps the address it was given"
        );
        assert!(
            move_retired(&mut list).is_empty(),
            "running it again changes nothing"
        );
    }

    #[test]
    fn form_bodies_are_encoded() {
        assert_eq!(
            form(&[("a", "b c"), ("r", "https://x/y?z=1")]),
            "a=b+c&r=https%3A%2F%2Fx%2Fy%3Fz%3D1"
        );
    }

    /// Live discovery against the catalog's services (network; run by hand).
    #[tokio::test]
    #[ignore]
    async fn catalog_services_offer_sign_in() {
        for url in [
            "https://mcp.supabase.com/mcp",
            "https://mcp.linear.app/mcp",
            "https://mcp.notion.com/mcp",
            "https://mcp.sentry.dev/mcp",
            "https://mcp.vercel.com",
            "https://mcp.stripe.com",
            "https://mcp.neon.tech/mcp",
            "https://mcp.posthog.com/mcp",
            "https://mcp.atlassian.com/v1/mcp",
            "https://mcp.figma.com/mcp",
            "https://mcp.prisma.io/mcp",
            "https://netlify-mcp.netlify.app/mcp",
            "https://mcp.cloudflare.com/mcp",
            "https://ai.todoist.net/mcp",
            "https://mcp.asana.com/mcp",
            "https://mcp.asana.com/sse",
            "https://mcp.canva.com/mcp",
            "https://mcp.paypal.com/mcp",
            "https://mcp.monday.com/mcp",
            "https://mcp.intercom.com/mcp",
            "https://mcp.webflow.com/mcp",
            "https://mcp.squareup.com/mcp",
            "https://mcp.wix.com/mcp",
            "https://mcp.railway.com",
            "https://huggingface.co/mcp",
        ] {
            match discover(url).await {
                Ok(found) => println!(
                    "ok   {url} -> {} (register: {})",
                    found.token_endpoint,
                    found.registration_endpoint.is_some()
                ),
                Err(error) => println!("FAIL {url}: {error:?}"),
            }
        }
    }
}
