//! MCP manager (ADR-075): lists the MCP servers each provider CLI has
//! configured and adds or removes servers in their own config files. Reads
//! never return environment or header values, only their names. Writes keep a
//! backup, preserve every other field, and replace the file atomically.
use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::runtime::State;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_FILE: u64 = 16 * 1024 * 1024;
const PROVIDERS: [Provider; 4] = [
    Provider::Claude,
    Provider::Codex,
    Provider::OpenCode,
    Provider::Cursor,
];
/// One write at a time across every config file the app edits.
static WRITE: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    Claude,
    Codex,
    OpenCode,
    Cursor,
}

impl Provider {
    fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Cursor => "cursor",
        }
    }
    fn parse(value: &str) -> Result<Self> {
        PROVIDERS
            .into_iter()
            .find(|provider| provider.key() == value)
            .ok_or_else(|| {
                Error::new(
                    "invalid",
                    "This provider's MCP servers are not managed here.",
                )
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    User,
    Project,
    /// Claude Code's per-project entries inside `~/.claude.json`.
    Local,
}

impl Scope {
    fn key(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
    fn parse(value: &str) -> Result<Self> {
        match value {
            "user" => Ok(Self::User),
            "project" => Ok(Self::Project),
            "local" => Ok(Self::Local),
            _ => Err(Error::new("invalid", "unknown MCP scope")),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub name: String,
    pub provider: String,
    pub scope: String,
    pub path: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    /// Names only: values never leave the native side.
    pub env_keys: Vec<String>,
    pub header_keys: Vec<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpProblem {
    pub provider: String,
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCatalog {
    pub servers: Vec<McpServer>,
    pub problems: Vec<McpProblem>,
    pub project_path: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct McpServerInput {
    pub name: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
    /// For a URL: `http` (default) or `sse`.
    pub transport: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    #[serde(rename_all = "camelCase")]
    List { project_id: Option<String> },
    #[serde(rename_all = "camelCase")]
    Add {
        project_id: Option<String>,
        providers: Vec<String>,
        scope: String,
        server: McpServerInput,
    },
    #[serde(rename_all = "camelCase")]
    Remove {
        project_id: Option<String>,
        provider: String,
        scope: String,
        name: String,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub catalog: McpCatalog,
    /// Providers whose file could not be changed (the others were written).
    pub failures: Vec<McpProblem>,
}

struct Roots {
    home: PathBuf,
    codex_home: PathBuf,
    config_home: PathBuf,
    project: Option<PathBuf>,
}

fn roots(project: Option<PathBuf>) -> Result<Roots> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| Error::new("native", "No home folder."))?;
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".codex"));
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    Ok(Roots {
        home,
        codex_home,
        config_home,
        project,
    })
}

fn first_existing(dir: &Path, names: &[&str]) -> PathBuf {
    names
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| dir.join(names[0]))
}

/// The file a provider keeps its servers in for a scope.
fn location(roots: &Roots, provider: Provider, scope: Scope) -> Option<PathBuf> {
    let project = roots.project.as_deref();
    Some(match (provider, scope) {
        (Provider::Claude, Scope::User | Scope::Local) => roots.home.join(".claude.json"),
        (Provider::Claude, Scope::Project) => project?.join(".mcp.json"),
        (Provider::Cursor, Scope::User) => roots.home.join(".cursor/mcp.json"),
        (Provider::Cursor, Scope::Project) => project?.join(".cursor/mcp.json"),
        (Provider::Codex, Scope::User) => roots.codex_home.join("config.toml"),
        (Provider::Codex, Scope::Project) => project?.join(".codex/config.toml"),
        (Provider::OpenCode, Scope::User) => first_existing(
            &roots.config_home.join("opencode"),
            &["opencode.json", "opencode.jsonc", "config.json"],
        ),
        (Provider::OpenCode, Scope::Project) => {
            first_existing(project?, &["opencode.json", "opencode.jsonc"])
        }
        (_, Scope::Local) => return None,
    })
}

fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(meta) if !meta.is_file() => return Err(Error::new("invalid", "not a regular file")),
        Ok(meta) if meta.len() > MAX_FILE => return Err(Error::new("invalid", "file too large")),
        Ok(_) => {}
    }
    Ok(Some(std::fs::read_to_string(path)?))
}

/// Removes `//` and `/* */` comments and trailing commas (OpenCode's JSONC).
fn strip_jsonc(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let (mut index, mut string, mut escaped) = (0, false, false);
    while index < chars.len() {
        let c = chars[index];
        if string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                string = false;
            }
            index += 1;
            continue;
        }
        match (c, chars.get(index + 1)) {
            ('"', _) => {
                string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
                continue;
            }
            ('/', Some('*')) => {
                index += 2;
                while index < chars.len()
                    && !(chars[index] == '*' && chars.get(index + 1) == Some(&'/'))
                {
                    index += 1;
                }
                index += 2;
                continue;
            }
            (',', _) => {
                let next = chars[index + 1..].iter().find(|c| !c.is_whitespace());
                if !matches!(next, Some('}') | Some(']')) {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
        index += 1;
    }
    out
}

fn parse_json(path: &Path, text: &str) -> Result<Value> {
    let jsonc = path.extension().is_some_and(|ext| ext == "jsonc")
        || path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|dir| dir == "opencode")
        || path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("opencode."));
    let parsed = if jsonc {
        serde_json::from_str(&strip_jsonc(text))
    } else {
        serde_json::from_str(text)
    };
    // serde_json errors carry only a line and column, never file content.
    let value: Value =
        parsed.map_err(|error| Error::new("invalid", format!("not valid JSON ({error})")))?;
    if !value.is_object() {
        return Err(Error::new("invalid", "the file is not a JSON object"));
    }
    Ok(value)
}

fn parse_toml(text: &str) -> Result<toml_edit::DocumentMut> {
    // toml_edit errors quote the offending line, which may hold a secret.
    text.parse::<toml_edit::DocumentMut>()
        .map_err(|_| Error::new("invalid", "not valid TOML"))
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn keys(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

/// One JSON server entry (Claude, Cursor or OpenCode shape).
fn json_server(
    provider: Provider,
    scope: Scope,
    path: &Path,
    name: &str,
    entry: &Value,
) -> McpServer {
    let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
    let (command, args) = match entry.get("command") {
        // OpenCode keeps the command and its arguments in one array.
        Some(Value::Array(_)) => {
            let mut parts = strings(entry.get("command"));
            let command = (!parts.is_empty()).then(|| parts.remove(0));
            (command, parts)
        }
        _ => (text("command"), strings(entry.get("args"))),
    };
    let url = text("url").or_else(|| text("serverUrl"));
    let kind = text("type").unwrap_or_default();
    let transport = if command.is_some() {
        "stdio"
    } else if kind == "sse" {
        "sse"
    } else {
        "http"
    };
    let env = entry.get("env").or_else(|| entry.get("environment"));
    McpServer {
        name: name.to_string(),
        provider: provider.key().into(),
        scope: scope.key().into(),
        path: crate::paths::display_path(path),
        transport: transport.into(),
        command,
        args,
        url,
        env_keys: keys(env),
        header_keys: keys(entry.get("headers")),
        enabled: entry
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
            && entry.get("disabled").and_then(Value::as_bool) != Some(true),
    }
}

fn container_key(provider: Provider) -> &'static str {
    if provider == Provider::OpenCode {
        "mcp"
    } else {
        "mcpServers"
    }
}

fn json_servers(
    provider: Provider,
    scope: Scope,
    path: &Path,
    root: &Value,
    project: Option<&Path>,
) -> Vec<McpServer> {
    let container = match scope {
        Scope::Local => project.and_then(|project| {
            root.get("projects")?
                .get(project.to_string_lossy().as_ref())?
                .get("mcpServers")
        }),
        _ => root.get(container_key(provider)),
    };
    container
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter(|(_, entry)| entry.is_object())
                .map(|(name, entry)| json_server(provider, scope, path, name, entry))
                .collect()
        })
        .unwrap_or_default()
}

fn toml_servers(scope: Scope, path: &Path, doc: &toml_edit::DocumentMut) -> Vec<McpServer> {
    let Some(table) = doc.get("mcp_servers").and_then(|item| item.as_table_like()) else {
        return vec![];
    };
    table
        .iter()
        .filter_map(|(name, item)| {
            let entry = item.as_table_like()?;
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(|item| item.as_str())
                    .map(str::to_string)
            };
            let list = |key: &str| -> Vec<String> {
                entry
                    .get(key)
                    .and_then(|item| item.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|v| v.as_str())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let names = |key: &str| -> Vec<String> {
                entry
                    .get(key)
                    .and_then(|item| item.as_table_like())
                    .map(|table| table.iter().map(|(key, _)| key.to_string()).collect())
                    .unwrap_or_default()
            };
            let command = text("command");
            let mut header_keys = names("http_headers");
            header_keys.extend(names("env_http_headers"));
            if entry.contains_key("bearer_token_env_var") {
                header_keys.push("Authorization".into());
            }
            Some(McpServer {
                name: name.to_string(),
                provider: Provider::Codex.key().into(),
                scope: scope.key().into(),
                path: crate::paths::display_path(path),
                transport: if command.is_some() { "stdio" } else { "http" }.into(),
                args: list("args"),
                url: text("url"),
                command,
                env_keys: names("env"),
                header_keys,
                enabled: entry
                    .get("enabled")
                    .and_then(|item| item.as_bool())
                    .unwrap_or(true),
            })
        })
        .collect()
}

fn discover(roots: &Roots) -> McpCatalog {
    let mut catalog = McpCatalog {
        servers: vec![],
        problems: vec![],
        project_path: roots
            .project
            .as_ref()
            .map(|path| crate::paths::display_path(path)),
    };
    let mut scopes = vec![(Provider::Claude, Scope::Local)];
    for provider in PROVIDERS {
        scopes.push((provider, Scope::User));
        scopes.push((provider, Scope::Project));
    }
    for (provider, scope) in scopes {
        if scope == Scope::Local && roots.project.is_none() {
            continue;
        }
        let path = match (provider, scope) {
            (Provider::Claude, Scope::Local) => location(roots, provider, Scope::User),
            _ => location(roots, provider, scope),
        };
        let Some(path) = path else { continue };
        let found = read_text(&path).and_then(|text| {
            let Some(text) = text else { return Ok(vec![]) };
            if provider == Provider::Codex {
                Ok(toml_servers(scope, &path, &parse_toml(&text)?))
            } else {
                Ok(json_servers(
                    provider,
                    scope,
                    &path,
                    &parse_json(&path, &text)?,
                    roots.project.as_deref(),
                ))
            }
        });
        match found {
            Ok(servers) => catalog.servers.extend(servers),
            // A broken Local read repeats the User one; report it once.
            Err(_) if scope == Scope::Local => {}
            Err(error) => catalog.problems.push(McpProblem {
                provider: provider.key().into(),
                path: crate::paths::display_path(&path),
                message: error.to_string(),
            }),
        }
    }
    catalog
}

fn plain(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.chars().count() <= limit && !value.chars().any(char::is_control)
}

fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Checks a server before it is written anywhere.
fn validate(server: &McpServerInput) -> Result<()> {
    let invalid = |message: &str| Err(Error::new("invalid", message.to_string()));
    if !valid_name(&server.name) {
        return invalid("Use 1–64 letters, digits, hyphens or underscores for the name.");
    }
    if server.name.to_ascii_lowercase().starts_with("sirus_") {
        return invalid("Names starting with sirus_ are reserved for Sirus Code.");
    }
    match (&server.command, &server.url) {
        (Some(command), None) => {
            if !plain(command.trim(), 1024) {
                return invalid("The command is empty or too long.");
            }
            if !server.headers.is_empty() {
                return invalid("Headers apply only to URL servers.");
            }
        }
        (None, Some(url)) => {
            let url = url.trim();
            if url.len() > 2048
                || !(url.starts_with("https://") || url.starts_with("http://"))
                || url.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                return invalid("The URL must be an http(s) address.");
            }
            if !server.args.is_empty() || !server.env.is_empty() {
                return invalid("Arguments and environment apply only to command servers.");
            }
        }
        _ => return invalid("Give either a command or a URL."),
    }
    if !matches!(
        server.transport.as_deref(),
        None | Some("http") | Some("sse") | Some("stdio")
    ) {
        return invalid("The transport must be stdio, http or sse.");
    }
    if server.args.len() > 64
        || server
            .args
            .iter()
            .any(|arg| arg.chars().count() > 4096 || arg.contains('\0') || arg.contains('\n'))
    {
        return invalid("Too many arguments, or one is too long.");
    }
    let env_key = |key: &str| {
        (1..=128).contains(&key.len())
            && key
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    };
    if server.env.len() > 64
        || server
            .env
            .iter()
            .any(|(key, value)| !env_key(key) || value.len() > 8192 || value.contains('\0'))
    {
        return invalid("An environment variable has an invalid name or value.");
    }
    let header_key = |key: &str| {
        (1..=128).contains(&key.len())
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    if server.headers.len() > 32
        || server.headers.iter().any(|(key, value)| {
            !header_key(key) || value.len() > 8192 || value.chars().any(char::is_control)
        })
    {
        return invalid("A header has an invalid name or value.");
    }
    Ok(())
}

/// The provider-specific JSON entry for a server.
fn json_entry(provider: Provider, server: &McpServerInput) -> Value {
    let sse = server.transport.as_deref() == Some("sse");
    let mut entry = Map::new();
    match (provider, &server.command, &server.url) {
        (Provider::OpenCode, Some(command), _) => {
            let mut parts = vec![json!(command.trim())];
            parts.extend(server.args.iter().map(|arg| json!(arg)));
            entry.insert("type".into(), json!("local"));
            entry.insert("command".into(), Value::Array(parts));
            if !server.env.is_empty() {
                entry.insert("environment".into(), json!(server.env));
            }
            entry.insert("enabled".into(), json!(true));
        }
        (Provider::OpenCode, None, Some(url)) => {
            entry.insert("type".into(), json!("remote"));
            entry.insert("url".into(), json!(url.trim()));
            if !server.headers.is_empty() {
                entry.insert("headers".into(), json!(server.headers));
            }
            entry.insert("enabled".into(), json!(true));
        }
        (_, Some(command), _) => {
            if provider == Provider::Claude {
                entry.insert("type".into(), json!("stdio"));
            }
            entry.insert("command".into(), json!(command.trim()));
            entry.insert("args".into(), json!(server.args));
            if !server.env.is_empty() {
                entry.insert("env".into(), json!(server.env));
            }
        }
        (_, None, Some(url)) => {
            if provider == Provider::Claude {
                entry.insert("type".into(), json!(if sse { "sse" } else { "http" }));
            }
            entry.insert("url".into(), json!(url.trim()));
            if !server.headers.is_empty() {
                entry.insert("headers".into(), json!(server.headers));
            }
        }
        _ => {}
    }
    Value::Object(entry)
}

fn toml_entry(server: &McpServerInput) -> toml_edit::Table {
    use toml_edit::{value, Array, InlineTable};
    let mut table = toml_edit::Table::new();
    let inline = |map: &BTreeMap<String, String>| {
        let mut inline = InlineTable::new();
        for (key, item) in map {
            inline.insert(key, item.as_str().into());
        }
        value(inline)
    };
    if let Some(command) = &server.command {
        table["command"] = value(command.trim());
        table["args"] = value(server.args.iter().map(String::as_str).collect::<Array>());
        if !server.env.is_empty() {
            table["env"] = inline(&server.env);
        }
    } else if let Some(url) = &server.url {
        table["url"] = value(url.trim());
        if !server.headers.is_empty() {
            table["http_headers"] = inline(&server.headers);
        }
    }
    table
}

/// Adds (`Some`) or removes (`None`) one server in a JSON config value.
fn edit_json(
    root: &mut Value,
    provider: Provider,
    scope: Scope,
    project: Option<&Path>,
    name: &str,
    entry: Option<Value>,
) -> Result<()> {
    let object = root
        .as_object_mut()
        .ok_or_else(|| Error::new("invalid", "the file is not a JSON object"))?;
    let container = match scope {
        Scope::Local => {
            let project = project.ok_or_else(|| Error::new("invalid", "no project"))?;
            object
                .get_mut("projects")
                .and_then(|projects| projects.get_mut(project.to_string_lossy().as_ref()))
                .and_then(Value::as_object_mut)
                .ok_or_else(|| Error::not_found("no server with that name"))?
                .entry("mcpServers")
                .or_insert_with(|| json!({}))
        }
        _ => object
            .entry(container_key(provider))
            .or_insert_with(|| json!({})),
    };
    let servers = container
        .as_object_mut()
        .ok_or_else(|| Error::new("invalid", "the servers entry is not an object"))?;
    match entry {
        Some(_) if servers.contains_key(name) => Err(Error::new(
            "exists",
            "A server with that name already exists there.",
        )),
        Some(entry) => {
            servers.insert(name.to_string(), entry);
            Ok(())
        }
        None => servers
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| Error::not_found("no server with that name")),
    }
}

fn edit_toml(
    doc: &mut toml_edit::DocumentMut,
    name: &str,
    entry: Option<toml_edit::Table>,
) -> Result<()> {
    if entry.is_some() && doc.get("mcp_servers").is_none() {
        let mut table = toml_edit::Table::new();
        table.set_implicit(true);
        doc.insert("mcp_servers", toml_edit::Item::Table(table));
    }
    let servers = doc
        .get_mut("mcp_servers")
        .and_then(|item| item.as_table_like_mut())
        .ok_or_else(|| match entry {
            Some(_) => Error::new("invalid", "mcp_servers is not a table"),
            None => Error::not_found("no server with that name"),
        })?;
    match entry {
        Some(_) if servers.contains_key(name) => Err(Error::new(
            "exists",
            "A server with that name already exists there.",
        )),
        Some(entry) => {
            servers.insert(name, toml_edit::Item::Table(entry));
            Ok(())
        }
        None => servers
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| Error::not_found("no server with that name")),
    }
}

/// Backs up the current file, then replaces it through a synced temporary
/// sibling with the same permissions (0600 for new files).
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    // Edit the real file behind a symlink instead of replacing the link.
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let parent = path
        .parent()
        .ok_or_else(|| Error::invalid_path("no parent folder"))?;
    std::fs::create_dir_all(parent)?;
    let existing = std::fs::metadata(&path).ok();
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    if existing.is_some() {
        std::fs::copy(&path, parent.join(format!("{name}.sirus-backup")))?;
    }
    let temporary = parent.join(format!(".{name}.sirus-{}", uuid::Uuid::new_v4().simple()));
    let result = (|| -> Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = existing
                .as_ref()
                .map(|meta| meta.permissions().mode() & 0o777)
                .unwrap_or(0o600);
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Reads, edits and writes one provider file under the write lock.
fn change(
    roots: &Roots,
    provider: Provider,
    scope: Scope,
    name: &str,
    server: Option<&McpServerInput>,
) -> Result<()> {
    let _guard = WRITE.lock();
    let path = location(roots, provider, scope)
        .ok_or_else(|| Error::new("invalid", "Choose a project for the project scope."))?;
    let text = read_text(&path)?;
    if text.is_none() && server.is_none() {
        return Err(Error::not_found("no server with that name"));
    }
    let bytes = if provider == Provider::Codex {
        let mut doc = parse_toml(text.as_deref().unwrap_or_default())?;
        edit_toml(&mut doc, name, server.map(toml_entry))?;
        doc.to_string().into_bytes()
    } else {
        let mut root = match &text {
            Some(text) if !text.trim().is_empty() => parse_json(&path, text)?,
            _ => json!({}),
        };
        edit_json(
            &mut root,
            provider,
            scope,
            roots.project.as_deref(),
            name,
            server.map(|server| json_entry(provider, server)),
        )?;
        let mut bytes = serde_json::to_vec_pretty(&root)?;
        bytes.push(b'\n');
        bytes
    };
    write_atomic(&path, &bytes)?;
    tracing::info!(
        provider = provider.key(),
        scope = scope.key(),
        server = name,
        "mcp config changed"
    );
    Ok(())
}

fn project_path(state: &AppState, project_id: Option<&str>) -> Result<Option<PathBuf>> {
    let Some(id) = project_id else {
        return Ok(None);
    };
    let data = state.data.lock();
    let project = data
        .projects
        .iter()
        .find(|project| project.id == id)
        .ok_or_else(|| Error::not_found("project not found"))?;
    Ok(Some(PathBuf::from(&project.path)))
}

fn run(state: &AppState, action: Action) -> Result<Response> {
    let mut failures = vec![];
    let project_id = match &action {
        Action::List { project_id }
        | Action::Add { project_id, .. }
        | Action::Remove { project_id, .. } => project_id.clone(),
    };
    let roots = roots(project_path(state, project_id.as_deref())?)?;
    match action {
        Action::List { .. } => {}
        Action::Add {
            providers,
            scope,
            server,
            ..
        } => {
            validate(&server)?;
            let scope = Scope::parse(&scope)?;
            if scope == Scope::Local {
                return Err(Error::new("invalid", "Choose the user or project scope."));
            }
            if providers.is_empty() || providers.len() > PROVIDERS.len() {
                return Err(Error::new("invalid", "Choose at least one provider."));
            }
            for key in providers {
                let provider = Provider::parse(&key)?;
                if let Err(error) = change(&roots, provider, scope, &server.name, Some(&server)) {
                    failures.push(McpProblem {
                        provider: key,
                        path: location(&roots, provider, scope)
                            .map(|path| crate::paths::display_path(&path))
                            .unwrap_or_default(),
                        message: error.to_string(),
                    });
                }
            }
        }
        Action::Remove {
            provider,
            scope,
            name,
            ..
        } => {
            if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
                return Err(Error::new("invalid", "invalid server name"));
            }
            change(
                &roots,
                Provider::parse(&provider)?,
                Scope::parse(&scope)?,
                &name,
                None,
            )?;
        }
    }
    Ok(Response {
        catalog: discover(&roots),
        failures,
    })
}

pub async fn mcp_action(state: State<'_, Arc<AppState>>, action: Action) -> Result<Response> {
    let state = state.inner().clone();
    crate::commands::native_task(move || {
        state.ensure_running()?;
        run(&state, action)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots_in(dir: &Path) -> Roots {
        Roots {
            home: dir.join("home"),
            codex_home: dir.join("home/.codex"),
            config_home: dir.join("home/.config"),
            project: Some(dir.join("project")),
        }
    }

    fn stdio(name: &str) -> McpServerInput {
        McpServerInput {
            name: name.into(),
            command: Some("npx".into()),
            args: vec!["-y".into(), "docs-server".into()],
            env: BTreeMap::from([("DOCS_TOKEN".into(), "secret-value".into())]),
            ..Default::default()
        }
    }

    #[test]
    fn validation_rejects_bad_names_mixed_kinds_and_reserved_prefix() {
        assert!(validate(&stdio("docs")).is_ok());
        assert!(validate(&stdio("bad name")).is_err());
        assert!(validate(&stdio("sirus_browser")).is_err());
        let mut both = stdio("docs");
        both.url = Some("https://example.com/mcp".into());
        assert!(validate(&both).is_err());
        let remote = McpServerInput {
            name: "remote".into(),
            url: Some("file:///etc/passwd".into()),
            ..Default::default()
        };
        assert!(validate(&remote).is_err());
        let mut env = stdio("docs");
        env.env.insert("1BAD".into(), "x".into());
        assert!(validate(&env).is_err());
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_are_stripped_outside_strings() {
        let text = "{\n // note\n \"url\": \"http://x//y\", /* block */ \"a\": [1,2,],\n}";
        let value: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(value["url"], "http://x//y");
        assert_eq!(value["a"], json!([1, 2]));
    }

    #[test]
    fn adds_and_removes_in_every_provider_preserving_other_fields() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots_in(dir.path());
        std::fs::create_dir_all(roots.home.join(".codex")).unwrap();
        std::fs::create_dir_all(roots.project.as_ref().unwrap()).unwrap();
        std::fs::write(
            roots.home.join(".claude.json"),
            r#"{"theme":"dark","mcpServers":{"old":{"command":"node"}}}"#,
        )
        .unwrap();
        std::fs::write(
            roots.codex_home.join("config.toml"),
            "# my model\nmodel = \"gpt-5\"\n\n[profiles.fast]\nmodel = \"mini\"\n",
        )
        .unwrap();
        for provider in PROVIDERS {
            change(&roots, provider, Scope::User, "docs", Some(&stdio("docs"))).unwrap();
            // A second add with the same name is refused, not overwritten.
            assert!(change(&roots, provider, Scope::User, "docs", Some(&stdio("docs"))).is_err());
        }
        let catalog = discover(&roots);
        assert!(catalog.problems.is_empty(), "{:?}", catalog.problems);
        let docs: Vec<&McpServer> = catalog
            .servers
            .iter()
            .filter(|server| server.name == "docs")
            .collect();
        assert_eq!(docs.len(), 4);
        for server in &docs {
            assert_eq!(server.command.as_deref(), Some("npx"));
            assert_eq!(server.args, vec!["-y", "docs-server"]);
            assert_eq!(server.env_keys, vec!["DOCS_TOKEN"]);
        }
        // Values never reach the catalog.
        assert!(!serde_json::to_string(&catalog)
            .unwrap()
            .contains("secret-value"));
        let claude: Value = serde_json::from_str(
            &std::fs::read_to_string(roots.home.join(".claude.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(claude["theme"], "dark");
        assert_eq!(claude["mcpServers"]["old"]["command"], "node");
        assert!(roots.home.join(".claude.json.sirus-backup").exists());
        let toml = std::fs::read_to_string(roots.codex_home.join("config.toml")).unwrap();
        assert!(toml.starts_with("# my model\nmodel = \"gpt-5\""));
        assert!(toml.contains("[profiles.fast]") && toml.contains("[mcp_servers.docs]"));
        let opencode: Value = serde_json::from_str(
            &std::fs::read_to_string(roots.config_home.join("opencode/opencode.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            opencode["mcp"]["docs"]["command"],
            json!(["npx", "-y", "docs-server"])
        );
        assert_eq!(opencode["mcp"]["docs"]["type"], "local");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(roots.home.join(".cursor/mcp.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        for provider in PROVIDERS {
            change(&roots, provider, Scope::User, "docs", None).unwrap();
            assert!(change(&roots, provider, Scope::User, "docs", None).is_err());
        }
        let toml = std::fs::read_to_string(roots.codex_home.join("config.toml")).unwrap();
        assert!(!toml.contains("docs") && toml.contains("[profiles.fast]"));
        assert_eq!(
            discover(&roots)
                .servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["old"]
        );
    }

    #[test]
    fn project_and_local_scopes_and_url_servers() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots_in(dir.path());
        let project = roots.project.clone().unwrap();
        std::fs::create_dir_all(&roots.home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let local = json!({ "projects": { project.to_string_lossy(): { "mcpServers": { "mine": { "type": "sse", "url": "https://x.dev/sse", "headers": { "Authorization": "Bearer t" } } } } } });
        std::fs::write(roots.home.join(".claude.json"), local.to_string()).unwrap();
        let remote = McpServerInput {
            name: "remote".into(),
            url: Some("https://example.com/mcp".into()),
            headers: BTreeMap::from([("X-Key".into(), "k".into())]),
            ..Default::default()
        };
        validate(&remote).unwrap();
        change(
            &roots,
            Provider::Claude,
            Scope::Project,
            "remote",
            Some(&remote),
        )
        .unwrap();
        change(
            &roots,
            Provider::Codex,
            Scope::Project,
            "remote",
            Some(&remote),
        )
        .unwrap();
        let catalog = discover(&roots);
        let find = |provider: &str, scope: &str| {
            catalog
                .servers
                .iter()
                .find(|server| server.provider == provider && server.scope == scope)
                .cloned()
                .unwrap()
        };
        assert_eq!(find("claude", "local").transport, "sse");
        assert_eq!(find("claude", "local").header_keys, vec!["Authorization"]);
        assert_eq!(
            find("claude", "project").url.as_deref(),
            Some("https://example.com/mcp")
        );
        assert_eq!(find("codex", "project").header_keys, vec!["X-Key"]);
        change(&roots, Provider::Claude, Scope::Local, "mine", None).unwrap();
        assert!(discover(&roots)
            .servers
            .iter()
            .all(|server| server.scope != "local"));
    }

    #[test]
    fn broken_files_are_reported_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let roots = roots_in(dir.path());
        std::fs::create_dir_all(&roots.codex_home).unwrap();
        std::fs::write(
            roots.codex_home.join("config.toml"),
            "token = \"leak\" = broken",
        )
        .unwrap();
        let catalog = discover(&roots);
        assert_eq!(catalog.problems.len(), 1);
        assert!(!catalog.problems[0].message.contains("leak"));
        assert!(change(
            &roots,
            Provider::Codex,
            Scope::User,
            "docs",
            Some(&stdio("docs"))
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(roots.codex_home.join("config.toml")).unwrap(),
            "token = \"leak\" = broken"
        );
    }
}
