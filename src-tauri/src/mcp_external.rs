//! MCP for agents outside Sirus (ADR-209): Claude Code in a terminal, Claude Desktop, a remote
//! MCP connector. The host's HTTP server answers `POST /mcp` (MCP's Streamable HTTP, JSON replies
//! only) for a client that sends `Authorization: Bearer <token>`. A token is made by the person
//! (Settings → Conexões, or `sirus-host mcp-token create`), has a name and a scope (`read` or
//! `write`), is kept only as a SHA-256 hash in `mcp-tokens.json` (`0600`, the data folder) and is
//! revoked by deleting its row. The tools are the session tools agents inside Sirus have
//! (ADR-076, ADR-208), run through `peer_tools::external`: sessions an outside client starts
//! are ordinary visible ones, headed "Via MCP: <token name>", on the person's default approval
//! mode (never full access on its own), and approvals are never answered from outside.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::runtime::{AppHandle, State};

const FILE: &str = "mcp-tokens.json";
/// What every token starts with, so a leaked one is recognizable and a stray string is refused
/// before any hashing.
pub const PREFIX: &str = "sirus_mcp_";
const MAX_TOKENS: usize = 32;
const MAX_NAME: usize = 60;
/// A request body: prompts are at most 32,000 characters.
pub const MAX_BODY: usize = 256 * 1024;
/// Requests a token may make in a minute, and how many of them may change something.
const READS_PER_MINUTE: usize = 120;
const WRITES_PER_MINUTE: usize = 12;
/// `last_used` is written at most this often, so reads do not rewrite the file.
const SEEN_EVERY_SECS: i64 = 60;

/// The tools a `read` token may call, and those added for `write`.
pub const READ_TOOLS: [&str; 6] = [
    "sirus_list_sessions",
    "sirus_read_session",
    "sirus_session_status",
    "sirus_models",
    "sirus_list_computers",
    "sirus_delegation_status",
];
pub const WRITE_TOOLS: [&str; 4] = [
    "sirus_create_session",
    "sirus_send_to_session",
    "sirus_delegate",
    "sirus_delegation_cancel",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read,
    Write,
}

impl Scope {
    pub fn allows(self, tool: &str) -> bool {
        READ_TOOLS.contains(&tool) || (self == Scope::Write && WRITE_TOOLS.contains(&tool))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    id: String,
    name: String,
    scope: Scope,
    token_hash: String,
    created_at: DateTime<Utc>,
    #[serde(default)]
    last_used: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Tokens {
    #[serde(default)]
    tokens: Vec<Stored>,
}

/// What the person sees of a token: never the secret, never the hash.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenInfo {
    id: String,
    name: String,
    scope: Scope,
    created_at: DateTime<Utc>,
    last_used: Option<DateTime<Utc>>,
}

impl TokenInfo {
    /// One line for `sirus-host mcp-token list`.
    pub fn line(&self) -> String {
        format!(
            "{}  {}  {}  created {}  last used {}",
            self.id,
            if self.scope == Scope::Write {
                "write"
            } else {
                "read "
            },
            self.name,
            self.created_at.format("%Y-%m-%d"),
            self.last_used.map_or("never".to_owned(), |at| at
                .format("%Y-%m-%d %H:%M")
                .to_string()),
        )
    }
}

impl From<&Stored> for TokenInfo {
    fn from(stored: &Stored) -> Self {
        Self {
            id: stored.id.clone(),
            name: stored.name.clone(),
            scope: stored.scope,
            created_at: stored.created_at,
            last_used: stored.last_used,
        }
    }
}

/// The answer to every token command: the list, and the new secret once, on creation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenList {
    tokens: Vec<TokenInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created: Option<Created>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    pub id: String,
    pub name: String,
    scope: Scope,
    /// Shown once: only its hash is kept.
    pub token: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    List,
    Create { name: String, scope: Scope },
    Revoke { id: String },
}

fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Read-modify-write of the file happens under this, so two calls in one process do not lose
/// each other's change. (The CLI is another process: it writes whole files atomically.)
static FILE_LOCK: Mutex<()> = Mutex::new(());

fn load(dir: &Path) -> Tokens {
    std::fs::read(dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(dir: &Path, tokens: &Tokens) -> Result<()> {
    let path = dir.join(FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(tokens)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// A name as the person typed it: trimmed, one line, at most 60 characters.
fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::new("mcp", "Give the token a name."));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::new("mcp", "The name must be one line of text."));
    }
    if name.chars().count() > MAX_NAME {
        return Err(Error::new(
            "mcp",
            format!("The name is at most {MAX_NAME} characters."),
        ));
    }
    Ok(name.to_owned())
}

pub fn list(dir: &Path) -> Vec<TokenInfo> {
    load(dir).tokens.iter().map(TokenInfo::from).collect()
}

/// Makes a token. The secret is in the answer and nowhere else.
pub fn create(dir: &Path, name: &str, scope: Scope) -> Result<Created> {
    let name = clean_name(name)?;
    let _guard = FILE_LOCK.lock();
    let mut tokens = load(dir);
    if tokens.tokens.len() >= MAX_TOKENS {
        return Err(Error::new(
            "mcp",
            format!("At most {MAX_TOKENS} tokens: revoke one you no longer use."),
        ));
    }
    let secret = format!("{PREFIX}{}", crate::remote::random_token(32));
    let stored = Stored {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        scope,
        token_hash: hash_token(&secret),
        created_at: Utc::now(),
        last_used: None,
    };
    let created = Created {
        id: stored.id.clone(),
        name: stored.name.clone(),
        scope,
        token: secret,
    };
    tokens.tokens.push(stored);
    save(dir, &tokens)?;
    tracing::info!(token = %created.name, ?scope, "mcp token created");
    Ok(created)
}

/// Removes a token; its next request is refused. Whether it was there.
pub fn revoke(dir: &Path, id: &str) -> Result<bool> {
    let _guard = FILE_LOCK.lock();
    let mut tokens = load(dir);
    let before = tokens.tokens.len();
    tokens.tokens.retain(|token| token.id != id);
    if tokens.tokens.len() == before {
        return Ok(false);
    }
    save(dir, &tokens)?;
    forget_rate(id);
    tracing::info!(id, "mcp token revoked");
    Ok(true)
}

/// The token a bearer secret belongs to, noting when it was last used. The file is the one
/// source of truth, so a revocation (from the window or the CLI) takes effect at once.
fn authenticate(dir: &Path, bearer: &str) -> Option<Stored> {
    if !bearer.starts_with(PREFIX) || bearer.len() > 256 {
        return None;
    }
    let hashed = hash_token(bearer);
    let found = load(dir)
        .tokens
        .into_iter()
        .find(|token| token.token_hash == hashed)?;
    let now = Utc::now();
    let stale = found
        .last_used
        .is_none_or(|seen| (now - seen).num_seconds() >= SEEN_EVERY_SECS);
    if stale {
        let _guard = FILE_LOCK.lock();
        let mut tokens = load(dir);
        if let Some(token) = tokens.tokens.iter_mut().find(|t| t.id == found.id) {
            token.last_used = Some(now);
            let _ = save(dir, &tokens);
        }
    }
    Some(found)
}

// ---------------------------------------------------------------------------------------------
// Rate limit

#[derive(Default)]
struct Rate {
    all: VecDeque<Instant>,
    writes: VecDeque<Instant>,
}

static RATES: Mutex<Option<HashMap<String, Rate>>> = Mutex::new(None);

fn forget_rate(id: &str) {
    if let Some(rates) = RATES.lock().as_mut() {
        rates.remove(id);
    }
}

fn within(window: &mut VecDeque<Instant>, now: Instant, limit: usize) -> bool {
    while window
        .front()
        .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(60))
    {
        window.pop_front();
    }
    if window.len() >= limit {
        return false;
    }
    window.push_back(now);
    true
}

/// Counts one request of token `id` (and a change, when `write`); false when it is over the
/// limit of a sliding minute.
fn allow(id: &str, write: bool, now: Instant) -> bool {
    let mut guard = RATES.lock();
    let rate = guard
        .get_or_insert_with(HashMap::new)
        .entry(id.to_owned())
        .or_default();
    within(&mut rate.all, now, READS_PER_MINUTE)
        && (!write || within(&mut rate.writes, now, WRITES_PER_MINUTE))
}

// ---------------------------------------------------------------------------------------------
// The protocol

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
    })
}

fn computer() -> Value {
    json!({ "type": "string", "description": "Optional: the name of another computer paired with this one (see sirus_list_computers) to run this on instead. Session and task ids belong to the computer they were made on, so pass the same computer again to read or continue them." })
}

/// The tools this scope may call, as `tools/list` shows them.
pub fn definitions(scope: Scope) -> Vec<Value> {
    let session =
        json!({ "type": "string", "description": "A session id from sirus_list_sessions." });
    let task = json!({ "type": "string", "description": "The sessionId (taskId) of a task started with sirus_delegate or sirus_create_session." });
    let provider = json!({ "type": "string", "description": "Optional provider id from sirus_models (claude, codex, opencode, …); defaults to the person's default agent." });
    let model = json!({ "type": "string", "description": "Optional model id from sirus_models." });
    let mut tools = vec![
        tool(
            "sirus_list_sessions",
            "Sirus: the projects and recent sessions in the app (id, title, project, provider, model, status, branch), newest first.",
            json!({ "project": { "type": "string", "description": "Optional project name or id." }, "computer": computer() }),
            &[],
        ),
        tool(
            "sirus_read_session",
            "Sirus: a session's status and a page of its messages (user and assistant text only), oldest first within the page. With no paging parameters you get the newest 6, each cut at 2,000 characters. To read further back, pass the previous response's nextCursor as cursor (null once you reach the start). A cut message carries truncated, nextCharOffset and totalChars: read the rest with cursor set to its index + 1, limit 1 and charOffset set to nextCharOffset.",
            json!({
                "sessionId": session,
                "limit": { "type": "integer", "description": "Messages per page, 1–20 (default 6)." },
                "cursor": { "type": "integer", "description": "Page ending just before this message index; use the previous nextCursor." },
                "maxChars": { "type": "integer", "description": "Characters kept per message, 200–6,000 (default 2,000)." },
                "charOffset": { "type": "integer", "description": "Character to start each message from (use with limit 1)." },
                "computer": computer(),
            }),
            &["sessionId"],
        ),
        tool(
            "sirus_session_status",
            "Sirus: a session's status (idle, running, waiting for the person, completed, failed, stopped), title and last error. Approvals a session waits for are answered by the person in the app, never from outside.",
            json!({ "sessionId": session, "computer": computer() }),
            &["sessionId"],
        ),
        tool(
            "sirus_models",
            "Sirus: the providers and models that can be used for a new session (installed and enabled in the app).",
            json!({ "computer": computer() }),
            &[],
        ),
        tool(
            "sirus_list_computers",
            "Sirus: the other computers paired with this one, by name. Pass a name as `computer` to the other tools to work there.",
            json!({}),
            &[],
        ),
        tool(
            "sirus_delegation_status",
            "Sirus: a task started with sirus_delegate or sirus_create_session: its status, its final answer once finished (finalReply) and any error.",
            json!({ "taskId": task, "computer": computer() }),
            &["taskId"],
        ),
    ];
    if scope == Scope::Write {
        tools.extend([
            tool(
                "sirus_create_session",
                "Sirus: start a new visible session in a project with a first prompt. It runs on its own in an isolated worktree by default, on the person's default approval mode (never full access unless they chose another default), so it may wait for the person's approval in its own tab. Returns at once; follow it with sirus_session_status.",
                json!({
                    "project": { "type": "string", "description": "Project name or id (see sirus_list_sessions)." },
                    "prompt": { "type": "string", "description": "The task: objective, context, constraints and the result you expect." },
                    "title": { "type": "string" },
                    "provider": provider,
                    "model": model,
                    "newWorktree": { "type": "boolean", "description": "Default true: an isolated worktree, so parallel sessions never mix files." },
                    "computer": computer(),
                }),
                &["project", "prompt"],
            ),
            tool(
                "sirus_send_to_session",
                "Sirus: continue an idle session with a message (default approval mode of the person's settings or stricter). Returns at once.",
                json!({ "sessionId": session, "prompt": { "type": "string" }, "computer": computer() }),
                &["sessionId", "prompt"],
            ),
            tool(
                "sirus_delegate",
                "Sirus: hand one self-contained task to a new agent in a project (any provider and model). The agent only sees the task text, so include the objective, context, constraints and the result you expect. It is a visible session, async: it returns a taskId at once and sirus_delegation_status reads the result.",
                json!({
                    "project": { "type": "string", "description": "Project name or id (see sirus_list_sessions)." },
                    "task": { "type": "string", "description": "The self-contained task." },
                    "title": { "type": "string" },
                    "provider": provider,
                    "model": model,
                    "newWorktree": { "type": "boolean", "description": "Default true: an isolated worktree." },
                    "computer": computer(),
                }),
                &["project", "task"],
            ),
            tool(
                "sirus_delegation_cancel",
                "Sirus: stop a running session, like the person pressing Stop in it. Its session and results so far stay.",
                json!({ "taskId": task, "computer": computer() }),
                &["taskId"],
            ),
        ]);
    }
    tools
}

/// What a JSON-RPC message from a client asks of the host.
#[derive(Debug, PartialEq)]
pub enum Routed {
    /// The reply, if the message wants one (`None`: a notification, answered with 202).
    Reply(Option<Value>),
    Call {
        id: Value,
        tool: String,
        arguments: Value,
    },
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The protocol: `initialize`, `ping`, `tools/list` (what the scope may call) and `tools/call`
/// (refused here for a tool the scope does not allow, before anything runs).
pub fn route(scope: Scope, message: &Value) -> Routed {
    let Some(id) = message.get("id").cloned().filter(|id| !id.is_null()) else {
        // A notification (`notifications/initialized`, …) or a response: nothing to answer.
        return Routed::Reply(None);
    };
    let respond = |result: Value| {
        Routed::Reply(Some(
            json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        ))
    };
    match message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "initialize" => {
            let version = message
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .filter(|version| version.len() <= 32)
                .unwrap_or("2025-06-18");
            respond(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "sirus", "version": env!("CARGO_PKG_VERSION") },
                "instructions": if scope == Scope::Write {
                    "Sirus: read the person's coding sessions and start or continue them. Sessions you start are visible to the person, run on their approval settings and may wait for them to approve actions in the app."
                } else {
                    "Sirus: read the person's coding sessions (read-only token)."
                },
            }))
        }
        "ping" => respond(json!({})),
        "tools/list" => respond(json!({ "tools": definitions(scope) })),
        "tools/call" => {
            let name = message
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !READ_TOOLS.contains(&name) && !WRITE_TOOLS.contains(&name) {
                return Routed::Reply(Some(rpc_error(&id, -32602, "Unknown tool")));
            }
            if !scope.allows(name) {
                return Routed::Reply(Some(rpc_error(
                    &id,
                    -32001,
                    "This token is read-only: make a token with write access to use this tool.",
                )));
            }
            Routed::Call {
                id,
                tool: name.to_owned(),
                arguments: message
                    .pointer("/params/arguments")
                    .cloned()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({})),
            }
        }
        _ => Routed::Reply(Some(rpc_error(&id, -32601, "Method not found"))),
    }
}

/// The reply to a finished tool call.
fn call_reply(id: Value, result: std::result::Result<Value, String>) -> Value {
    let result = match result {
        Ok(value) => crate::mcp_stdio::tool_content(&value),
        Err(error) => json!({ "content": [ { "type": "text", "text": error } ], "isError": true }),
    };
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// What the HTTP route answers.
#[derive(Debug, PartialEq)]
pub enum Reply {
    Status(u16),
    Json(Value),
}

/// One `POST /mcp`: authenticates the bearer secret, applies the limits, routes the message
/// and runs a tool call. The secret is never logged.
pub async fn handle(app: &AppHandle, dir: &Path, bearer: Option<&str>, body: &[u8]) -> Reply {
    let Some(token) = bearer.and_then(|bearer| authenticate(dir, bearer)) else {
        return Reply::Status(401);
    };
    if body.len() > MAX_BODY {
        return Reply::Status(413);
    }
    let Ok(message) = serde_json::from_slice::<Value>(body) else {
        return Reply::Status(400);
    };
    if !message.is_object() {
        // Batches are not part of the 2025-06-18 transport.
        return Reply::Status(400);
    }
    let write = WRITE_TOOLS.contains(
        &message
            .pointer("/params/name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    if !allow(&token.id, write, Instant::now()) {
        tracing::warn!(token = %token.name, "mcp rate limit");
        return Reply::Status(429);
    }
    match route(token.scope, &message) {
        Routed::Reply(Some(reply)) => Reply::Json(reply),
        Routed::Reply(None) => Reply::Status(202),
        Routed::Call {
            id,
            tool,
            arguments,
        } => {
            tracing::info!(token = %token.name, tool = %tool, "mcp call");
            let result = if tool == "sirus_list_computers" {
                Ok(crate::peers::public_list(&crate::peers::load(dir)))
            } else {
                crate::peer_tools::external(app, &tool, arguments, &token.name).await
            };
            Reply::Json(call_reply(id, result))
        }
    }
}

/// The host command behind Settings → Conexões: the person's list of tokens, made, revoked.
pub async fn mcp_token_action(
    state: State<'_, std::sync::Arc<AppState>>,
    action: Action,
) -> Result<TokenList> {
    let dir = state
        .inner()
        .data_path
        .parent()
        .ok_or_else(|| Error::new("mcp", "The data folder is unknown."))?
        .to_path_buf();
    run_action(&dir, action)
}

/// The same, for `sirus-host mcp-token` and the command.
pub fn run_action(dir: &Path, action: Action) -> Result<TokenList> {
    let created = match action {
        Action::List => None,
        Action::Create { name, scope } => Some(create(dir, &name, scope)?),
        Action::Revoke { id } => {
            if !revoke(dir, &id)? {
                return Err(Error::not_found("No token has this id."));
            }
            None
        }
    };
    Ok(TokenList {
        tokens: list(dir),
        created,
    })
}

// ---------------------------------------------------------------------------------------------
// `sirus-host mcp`: the same endpoint for a client that starts a command (`claude mcp add`)

/// File in the data folder where a running host writes the loopback address of its endpoint.
pub const ENDPOINT_FILE: &str = "mcp-endpoint";

/// MCP over stdio for a local client: every JSON-RPC line on stdin is posted to the host's
/// `/mcp` with the token, and the JSON reply is written to stdout. Nothing else is printed there.
pub fn run_stdio(endpoint: &str, token: &str) -> i32 {
    use std::io::{BufRead, Write};
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return 1;
    };
    let client = reqwest::Client::new();
    let mut stdout = std::io::stdout();
    for line in std::io::BufReader::new(std::io::stdin().lock()).lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let id = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|message| message.get("id").cloned())
            .filter(|id| !id.is_null());
        let sent = runtime.block_on(async {
            let response = client
                .post(endpoint)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .header("accept", "application/json")
                .timeout(Duration::from_secs(120))
                .body(line.clone())
                .send()
                .await?;
            let status = response.status();
            Ok::<_, reqwest::Error>((status, response.text().await?))
        });
        let reply = match sent {
            Ok((status, text)) if status.is_success() => (!text.trim().is_empty()).then_some(text),
            Ok((status, _)) => id.as_ref().map(|id| {
                let message = match status.as_u16() {
                    401 => "Sirus refused the token: it is wrong or was revoked.".to_owned(),
                    429 => "Too many requests: wait a moment.".to_owned(),
                    code => format!("Sirus answered HTTP {code}."),
                };
                rpc_error(id, -32000, &message).to_string()
            }),
            Err(_) => id.as_ref().map(|id| {
                rpc_error(id, -32000, "Cannot reach the Sirus host; is it running?").to_string()
            }),
        };
        if let Some(reply) = reply {
            // A reply is one line.
            let _ = writeln!(stdout, "{}", reply.replace(['\n', '\r'], " "));
            let _ = stdout.flush();
        }
    }
    0
}

/// Where `sirus-host mcp` finds the host: the flag, `SIRUS_MCP_URL`, the address a running host
/// wrote in the data folder, else the default port.
pub fn endpoint(flag: Option<&str>, env: Option<&str>, dir: &Path, port: u16) -> String {
    let base = flag
        .map(str::to_owned)
        .or_else(|| env.filter(|value| !value.is_empty()).map(str::to_owned))
        .or_else(|| {
            std::fs::read_to_string(dir.join(ENDPOINT_FILE))
                .ok()
                .map(|text| text.trim().to_owned())
                .filter(|text| text.starts_with("http://127.0.0.1:"))
        })
        .unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
    let base = base.trim_end_matches('/');
    if base.ends_with("/mcp") {
        base.to_owned()
    } else {
        format!("{base}/mcp")
    }
}

/// A running host's endpoint for `sirus-host mcp`: the window's own listener when there is one.
pub fn write_endpoint(dir: &Path, port: u16) {
    let _ = std::fs::write(
        dir.join(ENDPOINT_FILE),
        format!("http://127.0.0.1:{port}/mcp"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn a_token_is_shown_once_and_kept_only_as_a_hash() {
        let dir = dir();
        let created = create(dir.path(), "  Claude Code  ", Scope::Read).unwrap();
        assert!(created.token.starts_with(PREFIX));
        assert_eq!(created.name, "Claude Code");
        let saved = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(!saved.contains(&created.token));
        assert!(saved.contains(&hash_token(&created.token)));
        let shown = serde_json::to_string(&run_action(dir.path(), Action::List).unwrap()).unwrap();
        assert!(!shown.contains(&created.token));
        assert!(!shown.contains("tokenHash"));
        assert!(shown.contains("Claude Code"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn only_the_right_secret_authenticates_and_use_is_noted() {
        let dir = dir();
        let created = create(dir.path(), "a", Scope::Write).unwrap();
        create(dir.path(), "b", Scope::Read).unwrap();
        assert!(authenticate(dir.path(), "").is_none());
        assert!(authenticate(dir.path(), "sirus_mcp_nope").is_none());
        assert!(authenticate(dir.path(), &created.token[..created.token.len() - 1]).is_none());
        // The hash itself is not a credential, and neither is a token without the prefix.
        assert!(authenticate(dir.path(), &hash_token(&created.token)).is_none());
        let found = authenticate(dir.path(), &created.token).unwrap();
        assert_eq!((found.name.as_str(), found.scope), ("a", Scope::Write));
        assert!(
            list(dir.path())
                .iter()
                .find(|token| token.id == created.id)
                .unwrap()
                .last_used
                .is_some()
        );
    }

    #[test]
    fn a_revoked_token_stops_working_at_once() {
        let dir = dir();
        let created = create(dir.path(), "a", Scope::Read).unwrap();
        assert!(authenticate(dir.path(), &created.token).is_some());
        assert!(revoke(dir.path(), &created.id).unwrap());
        assert!(authenticate(dir.path(), &created.token).is_none());
        assert!(!revoke(dir.path(), &created.id).unwrap());
        assert!(run_action(dir.path(), Action::Revoke { id: created.id }).is_err());
    }

    #[test]
    fn names_and_the_number_of_tokens_are_bounded() {
        let dir = dir();
        assert!(create(dir.path(), "   ", Scope::Read).is_err());
        assert!(create(dir.path(), "two\nlines", Scope::Read).is_err());
        assert!(create(dir.path(), &"x".repeat(61), Scope::Read).is_err());
        for index in 0..MAX_TOKENS {
            create(dir.path(), &format!("t{index}"), Scope::Read).unwrap();
        }
        assert!(create(dir.path(), "one more", Scope::Read).is_err());
    }

    #[test]
    fn scopes_decide_the_tools() {
        for tool in READ_TOOLS {
            assert!(
                Scope::Read.allows(tool) && Scope::Write.allows(tool),
                "{tool}"
            );
        }
        for tool in WRITE_TOOLS {
            assert!(!Scope::Read.allows(tool), "{tool}");
            assert!(Scope::Write.allows(tool), "{tool}");
        }
        for tool in [
            "sirus_answer_approval",
            "sirus_session_diff",
            "request_secret",
            "use_secret",
            "browser_open",
            "computer_click",
            "",
        ] {
            assert!(!Scope::Write.allows(tool), "{tool}");
        }
    }

    fn names(scope: Scope) -> Vec<String> {
        definitions(scope)
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn the_listed_tools_match_the_scope() {
        let read = names(Scope::Read);
        assert_eq!(read, READ_TOOLS);
        let write = names(Scope::Write);
        assert!(
            READ_TOOLS
                .iter()
                .chain(&WRITE_TOOLS)
                .all(|tool| write.iter().any(|name| name == tool))
        );
        assert_eq!(write.len(), READ_TOOLS.len() + WRITE_TOOLS.len());
        assert!(write.iter().all(|name| Scope::Write.allows(name)));
    }

    #[test]
    fn initialize_lists_tools_and_pings() {
        let init = route(
            Scope::Read,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } }),
        );
        let Routed::Reply(Some(init)) = init else {
            panic!("initialize answers");
        };
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "sirus");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        let Routed::Reply(Some(list)) = route(
            Scope::Read,
            &json!({ "jsonrpc": "2.0", "id": "a", "method": "tools/list" }),
        ) else {
            panic!("tools/list answers");
        };
        assert_eq!(list["id"], "a");
        assert_eq!(
            list["result"]["tools"].as_array().unwrap().len(),
            READ_TOOLS.len()
        );
        let Routed::Reply(Some(write)) = route(
            Scope::Write,
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        ) else {
            panic!("tools/list answers");
        };
        assert_eq!(write["result"]["tools"].as_array().unwrap().len(), 10);
        assert!(matches!(
            route(
                Scope::Read,
                &json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" })
            ),
            Routed::Reply(Some(_))
        ));
        assert_eq!(
            route(
                Scope::Read,
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
            ),
            Routed::Reply(None)
        );
        let Routed::Reply(Some(unknown)) = route(
            Scope::Read,
            &json!({ "jsonrpc": "2.0", "id": 4, "method": "resources/list" }),
        ) else {
            panic!("an unknown method is an error");
        };
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn a_read_token_cannot_call_a_write_tool() {
        let call = |scope, name: &str| {
            route(
                scope,
                &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": name, "arguments": { "project": "p", "prompt": "x" } } }),
            )
        };
        let Routed::Reply(Some(refused)) = call(Scope::Read, "sirus_create_session") else {
            panic!("refused before anything runs");
        };
        assert_eq!(refused["error"]["code"], -32001);
        assert_eq!(refused["id"], 7);
        let Routed::Reply(Some(unknown)) = call(Scope::Write, "sirus_answer_approval") else {
            panic!("an unlisted tool is unknown");
        };
        assert_eq!(unknown["error"]["code"], -32602);
        assert!(matches!(
            call(Scope::Read, "sirus_list_sessions"),
            Routed::Call { .. }
        ));
        let Routed::Call {
            tool, arguments, ..
        } = call(Scope::Write, "sirus_create_session")
        else {
            panic!("a write token may start sessions");
        };
        assert_eq!(tool, "sirus_create_session");
        assert_eq!(arguments["project"], "p");
        // Arguments that are not an object are dropped, not passed on.
        let Routed::Call { arguments, .. } = route(
            Scope::Read,
            &json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/call", "params": { "name": "sirus_models", "arguments": "x" } }),
        ) else {
            panic!("a call");
        };
        assert_eq!(arguments, json!({}));
    }

    #[test]
    fn a_call_result_is_content_and_an_error_is_flagged() {
        let ok = call_reply(json!(1), Ok(json!({ "started": true })));
        assert_eq!(ok["result"]["structuredContent"]["started"], true);
        assert!(ok["result"].get("isError").is_none());
        let failed = call_reply(json!(1), Err("no session with that id".into()));
        assert_eq!(failed["result"]["isError"], true);
        assert_eq!(
            failed["result"]["content"][0]["text"],
            "no session with that id"
        );
    }

    #[test]
    fn requests_per_minute_are_limited_per_token_and_writes_more_tightly() {
        let now = Instant::now();
        let id = format!("limit-{}", uuid::Uuid::new_v4());
        for _ in 0..WRITES_PER_MINUTE {
            assert!(allow(&id, true, now));
        }
        assert!(!allow(&id, true, now), "writes are capped");
        assert!(allow(&id, false, now), "reads still pass");
        assert!(allow(&format!("{id}-other"), true, now), "per token");
        let later = now + Duration::from_secs(61);
        assert!(allow(&id, true, later), "the minute slides");
        let reads = format!("reads-{}", uuid::Uuid::new_v4());
        for _ in 0..READS_PER_MINUTE {
            assert!(allow(&reads, false, now));
        }
        assert!(!allow(&reads, false, now));
    }

    #[test]
    fn the_stdio_client_finds_the_host() {
        let dir = dir();
        assert_eq!(
            endpoint(None, None, dir.path(), 7710),
            "http://127.0.0.1:7710/mcp"
        );
        write_endpoint(dir.path(), 7711);
        assert_eq!(
            endpoint(None, None, dir.path(), 7710),
            "http://127.0.0.1:7711/mcp"
        );
        assert_eq!(
            endpoint(None, Some("http://127.0.0.1:9/"), dir.path(), 7710),
            "http://127.0.0.1:9/mcp"
        );
        assert_eq!(
            endpoint(
                Some("https://sirus.example.com/mcp"),
                Some("x"),
                dir.path(),
                7710
            ),
            "https://sirus.example.com/mcp"
        );
    }

    #[test]
    fn external_sessions_never_start_on_full_access_by_themselves() {
        use crate::models::ApprovalMode::*;
        assert_eq!(crate::peer_tools::external_approval(Full), Ask);
        assert_eq!(crate::peer_tools::external_approval(Auto), Auto);
        assert_eq!(crate::peer_tools::external_approval(Ask), Ask);
    }

    #[tokio::test]
    async fn the_route_refuses_without_a_valid_token_and_answers_with_one() {
        let dir = dir();
        let app = AppHandle::new(dir.path().to_path_buf());
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        assert_eq!(
            handle(&app, dir.path(), None, body).await,
            Reply::Status(401)
        );
        assert_eq!(
            handle(&app, dir.path(), Some("sirus_mcp_x"), body).await,
            Reply::Status(401)
        );
        let read = create(dir.path(), "r", Scope::Read).unwrap();
        let Reply::Json(list) = handle(&app, dir.path(), Some(&read.token), body).await else {
            panic!("a valid token gets the list");
        };
        assert_eq!(
            list["result"]["tools"].as_array().unwrap().len(),
            READ_TOOLS.len()
        );
        assert_eq!(
            handle(&app, dir.path(), Some(&read.token), b"nope").await,
            Reply::Status(400)
        );
        assert_eq!(
            handle(&app, dir.path(), Some(&read.token), b"[]").await,
            Reply::Status(400)
        );
        let big = vec![b' '; MAX_BODY + 1];
        assert_eq!(
            handle(&app, dir.path(), Some(&read.token), &big).await,
            Reply::Status(413)
        );
        let note = br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert_eq!(
            handle(&app, dir.path(), Some(&read.token), note).await,
            Reply::Status(202)
        );
        let Reply::Json(computers) = handle(
            &app,
            dir.path(),
            Some(&read.token),
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"sirus_list_computers"}}"#,
        )
        .await
        else {
            panic!("list_computers answers without the app state");
        };
        assert!(computers["result"]["structuredContent"]["computers"].is_array());
        let Reply::Json(refused) = handle(
            &app,
            dir.path(),
            Some(&read.token),
            br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"sirus_send_to_session","arguments":{"sessionId":"s","prompt":"p"}}}"#,
        )
        .await
        else {
            panic!("a refusal is a reply");
        };
        assert_eq!(refused["error"]["code"], -32001);
        revoke(dir.path(), &read.id).unwrap();
        assert_eq!(
            handle(&app, dir.path(), Some(&read.token), body).await,
            Reply::Status(401)
        );
    }
}
