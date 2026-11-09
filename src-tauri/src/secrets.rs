//! Private secret requests (ADR-077). An agent calls `request_secret` on the session MCP
//! bridge; the person answers in a private card. The value lives only in this process's
//! memory under a one-use reference: never persisted, never in the transcript, never logged,
//! and never returned to the model. `use_secret` spends the reference by writing the value
//! to the workspace file the person saw on the card, or to a private env file for one
//! command. Everything for a session is dropped when its turn ends or the app quits.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::Duration;

use crate::runtime::{AppHandle, Emitter, Manager, State};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::commands::AppState;
use crate::error::{Error, Result as IpcResult};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_VALUE: usize = 16 * 1024;
const MAX_PENDING_PER_SESSION: usize = 2;
const REF_PREFIX: &str = "secret-ref-";
/// The activity row label the transcript renders as "Secret provided (label)".
pub const PROVIDED_LABEL: &str = "Secret provided";

/// What the card shows; never the value.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretRequest {
    pub id: String,
    pub session_id: String,
    pub label: String,
    pub description: String,
    pub env_name: Option<String>,
    /// Workspace-relative file the value will be written to, when the agent named one.
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub requests: Vec<SecretRequest>,
}

/// A value overwritten before its memory is released.
struct SecretValue(String);
impl Drop for SecretValue {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.iter_mut().for_each(|byte| *byte = 0);
        std::hint::black_box(&bytes);
    }
}

struct Pending {
    request: SecretRequest,
    reply: mpsc::Sender<Option<SecretValue>>,
}

struct Stored {
    session_id: String,
    env_name: Option<String>,
    path: Option<String>,
    value: Arc<SecretValue>,
}

#[derive(Default)]
struct Vault {
    pending: Vec<Pending>,
    refs: HashMap<String, Stored>,
    /// Values provided in the current turn, kept only to scrub echoes from output.
    values: Vec<(String, Arc<SecretValue>)>,
    env_files: Vec<(String, PathBuf)>,
}

static VAULT: Mutex<Option<Vault>> = Mutex::new(None);
static DIR: OnceLock<PathBuf> = OnceLock::new();

fn with_vault<T>(f: impl FnOnce(&mut Vault) -> T) -> T {
    f(VAULT.lock().get_or_insert_with(Vault::default))
}

/// Private folder for one-command env files; leftovers from a crash are removed.
pub fn init(data_dir: &Path) {
    let dir = data_dir.join("secrets");
    let _ = std::fs::remove_dir_all(&dir);
    if std::fs::create_dir_all(&dir).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        let _ = DIR.set(dir);
    }
}

pub fn snapshot() -> Snapshot {
    with_vault(|vault| Snapshot {
        requests: vault
            .pending
            .iter()
            .map(|pending| pending.request.clone())
            .collect(),
    })
}

fn publish(app: &AppHandle) {
    let _ = app.emit("secret-state", snapshot());
}

/// Ends a session's turn: pending cards are declined, references and env files are gone.
pub fn end_turn(session_id: &str) {
    crate::login_vault::end_turn(session_id);
    let (pending, files, changed) = with_vault(|vault| {
        let before = vault.pending.len();
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut vault.pending)
            .into_iter()
            .partition(|pending| pending.request.session_id == session_id);
        vault.pending = kept;
        vault
            .refs
            .retain(|_, stored| stored.session_id != session_id);
        vault.values.retain(|(owner, _)| owner != session_id);
        let (files, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut vault.env_files)
            .into_iter()
            .partition(|(owner, _)| owner == session_id);
        vault.env_files = kept;
        (gone, files, before != vault.pending.len())
    });
    for pending in pending {
        let _ = pending.reply.send(None);
    }
    for (_, file) in files {
        let _ = std::fs::remove_file(file);
    }
    if changed {
        if let Some(app) = crate::browser_mcp::app_handle() {
            publish(&app);
        }
    }
}

/// App quit: every pending card is declined and every value and env file is dropped.
pub fn clear_all() {
    let (pending, files) = with_vault(|vault| {
        let taken = std::mem::take(vault);
        (taken.pending, taken.env_files)
    });
    for pending in pending {
        let _ = pending.reply.send(None);
    }
    for (_, file) in files {
        let _ = std::fs::remove_file(file);
    }
}

/// Registers a value that went into a page (a saved login's password) so any echo of it in
/// this turn's output is masked like a provided secret (ADR-186).
pub fn note_value(session_id: &str, value: &str) {
    if value.len() >= 4 {
        let value = Arc::new(SecretValue(value.to_string()));
        with_vault(|vault| vault.values.push((session_id.to_string(), value)));
    }
}

/// Replaces any value provided in this session's turn with a mask, so an echo of it
/// never reaches the transcript or activity.
pub fn scrub(session_id: &str, text: String) -> String {
    let values: Vec<Arc<SecretValue>> = with_vault(|vault| {
        vault
            .values
            .iter()
            .filter(|(owner, value)| owner == session_id && value.0.len() >= 4)
            .map(|(_, value)| value.clone())
            .collect()
    });
    values.iter().fold(text, |text, value| {
        if text.contains(&value.0) {
            text.replace(&value.0, &crate::redact::mask_value(&value.0))
        } else {
            text
        }
    })
}

fn arg_text(args: &Value, key: &str, limit: usize) -> Result<Option<String>, String> {
    let Some(value) = args.get(key).and_then(Value::as_str) else {
        return Ok(None);
    };
    let value = value.trim();
    if value.chars().count() > limit
        || value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(format!("`{key}` is too long or has control characters"));
    }
    Ok((!value.is_empty()).then(|| value.to_string()))
}

pub fn valid_env_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A plain relative file path inside the workspace: no `..`, no absolute path, not in `.git`.
pub fn valid_relative_path(path: &str) -> bool {
    let candidate = Path::new(path);
    path.len() <= 240
        && !path.ends_with('/')
        && candidate.file_name().is_some()
        && candidate
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && candidate
            .components()
            .next()
            .is_some_and(|first| first.as_os_str() != ".git")
}

/// One `NAME=value` line in dotenv syntax; quoted when the value needs it.
pub fn dotenv_line(name: &str, value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@+=,".contains(c));
    if plain {
        return format!("{name}={value}");
    }
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('$', "\\$");
    format!("{name}=\"{escaped}\"")
}

/// Replaces the variable's line (or `export NAME=` line) in a dotenv file, or appends it.
pub fn upsert_dotenv(existing: &str, name: &str, value: &str) -> String {
    let line = dotenv_line(name, value);
    let mut replaced = false;
    let mut lines: Vec<String> = existing
        .lines()
        .map(|current| {
            let bare = current.trim_start();
            let bare = bare.strip_prefix("export ").unwrap_or(bare).trim_start();
            if !replaced
                && bare
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.trim_start().starts_with('='))
            {
                replaced = true;
                line.clone()
            } else {
                current.to_string()
            }
        })
        .collect();
    if !replaced {
        lines.push(line);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// `export NAME='value'` for a POSIX shell to source.
pub fn shell_export(name: &str, value: &str) -> String {
    format!("export {name}={}\n", shell_quote(value))
}

/// `text` as one single-quoted POSIX shell word.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn session_root(app: &AppHandle, session_id: &str) -> Result<PathBuf, String> {
    let state = app.state::<Arc<AppState>>();
    let data = state.data.lock();
    let session = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .ok_or("this session no longer exists")?;
    crate::commands::session_cwd(&data, session)
        .map_err(|_| "the session workspace is unavailable".into())
}

/// The app-owned transcript row for a provided secret: its label only.
fn record_provided(app: &AppHandle, session_id: &str, request_id: &str, label: &str) {
    let state = app.state::<Arc<AppState>>();
    let mut data = state.data.lock();
    if let Some(session) = data
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
    {
        let row = crate::activity::secret_row(request_id, label);
        if crate::activity::observe(session, row) {
            crate::transcript_view::emit(app, session);
        }
    }
    drop(data);
    crate::persist::checkpoint_soon(state.inner(), session_id);
}

fn request(app: &AppHandle, session_id: &str, args: &Value) -> Result<Value, String> {
    let label =
        arg_text(args, "label", 80)?.ok_or("`label` is required (e.g. \"GitHub token\")")?;
    let description = arg_text(args, "description", 600)?.unwrap_or_default();
    let env_name = arg_text(args, "envName", 64)?;
    if env_name
        .as_deref()
        .is_some_and(|name| !valid_env_name(name))
    {
        return Err("`envName` must look like an environment variable (A-Z, 0-9, _)".into());
    }
    let path = arg_text(args, "path", 240)?;
    if path
        .as_deref()
        .is_some_and(|path| !valid_relative_path(path))
    {
        return Err(
            "`path` must be a relative file path inside the workspace, outside .git".into(),
        );
    }
    if path.is_some() {
        session_root(app, session_id)?;
    }
    let (reply, wait) = mpsc::channel();
    let request = SecretRequest {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.into(),
        label: label.clone(),
        description,
        env_name: env_name.clone(),
        path: path.clone(),
    };
    let id = request.id.clone();
    let admitted = with_vault(|vault| {
        let open = vault
            .pending
            .iter()
            .filter(|pending| pending.request.session_id == session_id)
            .count();
        if open >= MAX_PENDING_PER_SESSION {
            return false;
        }
        vault.pending.push(Pending { request, reply });
        true
    });
    if !admitted {
        return Err("another secret request is already waiting for the person".into());
    }
    publish(app);
    let answer = wait.recv_timeout(REQUEST_TIMEOUT).ok().flatten();
    with_vault(|vault| vault.pending.retain(|pending| pending.request.id != id));
    publish(app);
    let Some(value) = answer else {
        return Err(format!(
            "The person declined to provide \"{label}\". Do not ask for it in chat; continue without it or explain what is needed."
        ));
    };
    let reference = format!("{REF_PREFIX}{}", uuid::Uuid::new_v4().simple());
    let value = Arc::new(value);
    with_vault(|vault| {
        vault.values.push((session_id.into(), value.clone()));
        vault.refs.insert(
            reference.clone(),
            Stored {
                session_id: session_id.into(),
                env_name: env_name.clone(),
                path: path.clone(),
                value,
            },
        );
    });
    record_provided(app, session_id, &id, &label);
    let mut targets = vec![];
    if path.is_some() {
        targets.push("\"file\" writes it to the approved path");
    }
    if env_name.is_some() {
        targets.push("\"env\" returns a private env file to source for one command");
    }
    Ok(json!({
        "provided": true,
        "secretRef": reference,
        "label": label,
        "envName": env_name,
        "path": path,
        "instructions": if targets.is_empty() {
            "The value was provided but no path or envName was requested, so it cannot be used; request it again with one.".to_string()
        } else {
            format!(
                "Call use_secret with this secretRef once, target {}. The ref works once and expires when this turn ends. You never see the value: never print, log, commit or repeat it.",
                targets.join(" or ")
            )
        },
    }))
}

fn write_file(
    root: &Path,
    relative: &str,
    name: Option<&str>,
    value: &str,
) -> Result<Value, String> {
    use std::io::{Read, Seek, Write};
    let target = root.join(relative);
    let parent = target.parent().ok_or("invalid path")?;
    let parent = crate::paths::ensure_within(root, parent)
        .map_err(|_| "the folder for that path does not exist inside the workspace".to_string())?;
    let file_name = target.file_name().ok_or("invalid path")?;
    let target = parent.join(file_name);
    if let Ok(output) = crate::git::run(root, &["ls-files", "--error-unmatch", "--", relative]) {
        if output.status.success() {
            return Err(format!(
                "{relative} is tracked by Git; secrets are only written to untracked files"
            ));
        }
    }
    let content = match std::fs::symlink_metadata(&target) {
        Ok(meta) if !meta.is_file() => return Err("the path is not a regular file".into()),
        Ok(meta) if meta.len() > 1024 * 1024 => {
            return Err("the file is too large to update".into())
        }
        Ok(_) => {
            let mut file = crate::paths::open_regular_for_write_within(root, &target)
                .map_err(|_| "the file cannot be opened safely".to_string())?;
            let mut existing = String::new();
            let mut reader = crate::paths::open_regular_within(root, &target)
                .map_err(|_| "the file cannot be read safely".to_string())?;
            reader
                .read_to_string(&mut existing)
                .map_err(|_| "the file is not UTF-8 text".to_string())?;
            let content = match name {
                Some(name) => upsert_dotenv(&existing, name, value),
                None => value.to_string(),
            };
            file.set_len(0)
                .map_err(|_| "the file cannot be written".to_string())?;
            file.rewind()
                .map_err(|_| "the file cannot be written".to_string())?;
            file.write_all(content.as_bytes())
                .map_err(|_| "the file cannot be written".to_string())?;
            None
        }
        Err(_) => Some(match name {
            Some(name) => format!("{}\n", dotenv_line(name, value)),
            None => value.to_string(),
        }),
    };
    if let Some(content) = content {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options
            .open(&target)
            .map_err(|_| "the file cannot be created".to_string())?;
        crate::paths::verify_open_handle(root, &file)
            .map_err(|_| "the path escapes the workspace".to_string())?;
        file.write_all(content.as_bytes())
            .map_err(|_| "the file cannot be written".to_string())?;
    }
    let ignored = crate::git::run(root, &["check-ignore", "-q", "--", relative])
        .is_ok_and(|output| output.status.success());
    Ok(json!({
        "written": relative,
        "variable": name,
        "ignoredByGit": ignored,
        "note": if ignored { "Written. Do not print the file." } else { "Written, but Git does not ignore this file: add it to .gitignore before committing. Do not print the file." },
    }))
}

fn use_secret(app: &AppHandle, session_id: &str, args: &Value) -> Result<Value, String> {
    let reference = arg_text(args, "secretRef", 64)?.ok_or("`secretRef` is required")?;
    let target = arg_text(args, "target", 8)?.ok_or("`target` must be \"file\" or \"env\"")?;
    let stored = with_vault(|vault| {
        let owned = vault
            .refs
            .get(&reference)
            .is_some_and(|stored| stored.session_id == session_id);
        owned.then(|| vault.refs.remove(&reference)).flatten()
    })
    .ok_or("unknown, used or expired secretRef; request the secret again")?;
    match target.as_str() {
        "file" => {
            let path = stored
                .path
                .as_deref()
                .ok_or("this secret was not requested with a path")?;
            let root = session_root(app, session_id)?;
            write_file(&root, path, stored.env_name.as_deref(), &stored.value.0)
        }
        "env" => {
            let name = stored
                .env_name
                .as_deref()
                .ok_or("this secret was not requested with an envName")?;
            let dir = DIR.get().ok_or("private storage is unavailable")?;
            let file = dir.join(format!("{}.env", uuid::Uuid::new_v4().simple()));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            use std::io::Write;
            options
                .open(&file)
                .and_then(|mut handle| {
                    handle.write_all(shell_export(name, &stored.value.0).as_bytes())
                })
                .map_err(|_| "the env file cannot be written".to_string())?;
            with_vault(|vault| vault.env_files.push((session_id.into(), file.clone())));
            let shown = file.to_string_lossy();
            Ok(json!({
                "envFile": shown,
                "variable": name,
                "usage": format!("( . '{shown}' && your-command )"),
                "note": "Source it in the same shell command that needs it. It is deleted when this turn ends. Never print it.",
            }))
        }
        _ => Err("`target` must be \"file\" or \"env\"".into()),
    }
}

/// Sent as the session server's MCP `instructions`, which the agent CLIs put in their
/// system prompt: credentials are asked for through the private card, never in chat.
pub const INSTRUCTIONS: &str = "Sirus session tools. Credentials: whenever a task needs an API key, token, password or other secret you do not have, call `request_secret` (label, why, and the `envName` and/or workspace `path` it belongs in) and then `use_secret` with the returned secretRef. The person types it in a private card in the app; you never see the value. Do this instead of asking for the secret in chat, telling the person to paste it, or ending your reply with \"credentials are missing\". Only skip it when the person said they will configure it themselves. Sign-ins: on a login page in the session browser, try `browser_fill_login` first; it uses a login the person saved and you never see the password.";

/// Bridge entry for `request_secret` and `use_secret`; blocks while the card waits.
pub fn execute(
    app: &AppHandle,
    session_id: &str,
    tool: &str,
    args: &Value,
) -> Result<Value, String> {
    match tool {
        "request_secret" => request(app, session_id, args),
        "use_secret" => use_secret(app, session_id, args),
        _ => Err("unknown secret tool".into()),
    }
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({ "name": "request_secret", "description": "Ask the person privately for a credential (API key, token, password) whenever a task needs one you do not have. They type it in a private card; you never see the value and get a one-use secretRef instead. Name `path` (a workspace file such as .env.local) and/or `envName`. Never ask for secrets in chat or stop at \"credentials are missing\"; call this instead.", "inputSchema": { "type": "object", "properties": { "label": { "type": "string", "description": "Short name, e.g. \"Stripe test key\"." }, "description": { "type": "string", "description": "Why it is needed and where to find it." }, "envName": { "type": "string", "description": "Variable name, e.g. STRIPE_SECRET_KEY." }, "path": { "type": "string", "description": "Workspace-relative file to write it to (shown to the person)." } }, "required": ["label"] } }),
        json!({ "name": "use_secret", "description": "Spend a secretRef once. target \"file\": write it to the path the person approved (as envName=value when envName was given, otherwise the whole file). target \"env\": get a private env file exporting envName to source for one command; it is deleted when the turn ends.", "inputSchema": { "type": "object", "properties": { "secretRef": { "type": "string" }, "target": { "type": "string", "enum": ["file", "env"] } }, "required": ["secretRef", "target"] } }),
    ]
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    Status,
    #[serde(rename_all = "camelCase")]
    Respond {
        request_id: String,
        /// `None` declines.
        value: Option<String>,
        /// Also save the value in the linked Infisical project, in this environment (ADR-198).
        #[serde(default)]
        infisical_env: Option<String>,
        /// Also write it to `.env.local` in the conversation's workspace, under the requested name.
        #[serde(default)]
        dotenv: bool,
    },
}

/// Closed secret-card surface: status, provide or decline. Values are never echoed back.
pub async fn secret_action(state: State<'_, Arc<AppState>>, action: Action) -> IpcResult<Snapshot> {
    state.ensure_running()?;
    if let Action::Respond {
        request_id,
        value,
        infisical_env,
        dotenv,
    } = action
    {
        // Saved first, from the value still in hand; the agent is answered only once the save
        // worked, so a failed save leaves the request waiting for a retry.
        let mut saved = Ok(());
        if let (Some(environment), Some(typed)) = (infisical_env.as_deref(), value.as_deref()) {
            let asked = snapshot()
                .requests
                .into_iter()
                .find(|request| request.id == request_id);
            if let Some(request) = asked {
                let key = request
                    .env_name
                    .clone()
                    .unwrap_or_else(|| request.label.clone());
                saved = crate::infisical::save_for_session(
                    state.inner(),
                    &request.session_id,
                    environment,
                    &key,
                    typed,
                )
                .await;
            }
        }
        if let (true, Some(typed)) = (dotenv, value.as_deref()) {
            saved = save_dotenv(state.inner(), &request_id, typed);
        }
        respond_if_saved(saved, || respond(&request_id, value))?;
    }
    Ok(snapshot())
}

/// The agent hears back only after the save: a failed one is returned and nothing is answered.
fn respond_if_saved(
    saved: IpcResult<()>,
    respond: impl FnOnce() -> IpcResult<()>,
) -> IpcResult<()> {
    saved?;
    respond()
}

/// The card's ".env.local do projeto": the value is written to that untracked file in the
/// conversation's workspace, as `NAME=value`.
fn save_dotenv(state: &AppState, request_id: &str, value: &str) -> IpcResult<()> {
    let request = snapshot()
        .requests
        .into_iter()
        .find(|request| request.id == request_id)
        .ok_or_else(|| Error::agent("That request is no longer waiting."))?;
    let name = request
        .env_name
        .ok_or_else(|| Error::agent("This request has no variable name to save under."))?;
    let root = {
        let data = state.data.lock();
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == request.session_id)
            .ok_or_else(|| Error::agent("This conversation no longer exists."))?;
        crate::commands::session_cwd(&data, session)
            .map_err(|_| Error::agent("The conversation's workspace is unavailable."))?
    };
    write_file(&root, ".env.local", Some(&name), value)
        .map(|_| ())
        .map_err(|_| Error::agent("The .env.local file could not be written."))
}

/// Answers a waiting request: the value goes to the agent's side and the card closes.
pub(crate) fn respond(request_id: &str, value: Option<String>) -> IpcResult<()> {
    {
        let value = match value {
            Some(value) if value.is_empty() || value.len() > MAX_VALUE || value.contains('\0') => {
                return Err(Error::agent("Enter a value up to 16 KB."));
            }
            other => other.map(SecretValue),
        };
        // Taken out of the list as it is answered, so the snapshot this returns no longer shows
        // it; otherwise that snapshot could land after the waiting side's update and reopen the card.
        let reply = with_vault(|vault| {
            let at = vault
                .pending
                .iter()
                .position(|pending| pending.request.id == request_id)?;
            Some(vault.pending.remove(at).reply)
        })
        .ok_or_else(|| Error::agent("That request is no longer waiting."))?;
        reply
            .send(value)
            .map_err(|_| Error::agent("That request is no longer waiting."))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_save_does_not_answer_the_agent() {
        let mut answered = false;
        let failed = respond_if_saved(Err(Error::agent("Infisical could not be reached.")), || {
            answered = true;
            Ok(())
        });
        assert!(failed.is_err() && !answered);
        respond_if_saved(Ok(()), || {
            answered = true;
            Ok(())
        })
        .unwrap();
        assert!(answered);
    }

    #[test]
    fn names_and_paths_are_strict() {
        assert!(valid_env_name("GITHUB_TOKEN") && valid_env_name("_x1"));
        assert!(!valid_env_name("1X") && !valid_env_name("A-B") && !valid_env_name(""));
        assert!(valid_relative_path(".env.local") && valid_relative_path("config/secret.json"));
        for bad in [
            "/etc/passwd",
            "../x",
            "a/../../b",
            ".git/config",
            "dir/",
            "",
        ] {
            assert!(!valid_relative_path(bad), "{bad}");
        }
    }

    #[test]
    fn dotenv_and_shell_lines_quote_safely() {
        assert_eq!(dotenv_line("A", "abc123"), "A=abc123");
        assert_eq!(dotenv_line("A", "a b\"$c"), "A=\"a b\\\"\\$c\"");
        assert_eq!(
            upsert_dotenv("X=1\nexport A=old\n# c\n", "A", "new"),
            "X=1\nA=new\n# c\n"
        );
        assert_eq!(upsert_dotenv("", "A", "v"), "A=v\n");
        assert_eq!(upsert_dotenv("AB=1", "A", "v"), "AB=1\nA=v\n");
        assert_eq!(shell_export("A", "it's"), "export A='it'\\''s'\n");
    }

    #[test]
    fn references_are_one_use_scoped_and_end_with_the_turn() {
        let value = Arc::new(SecretValue("hunter2-value".into()));
        with_vault(|vault| {
            vault.values.push(("s1".into(), value.clone()));
            vault.refs.insert(
                "secret-ref-a".into(),
                Stored {
                    session_id: "s1".into(),
                    env_name: None,
                    path: None,
                    value,
                },
            );
        });
        assert_eq!(scrub("s1", "echo hunter2-value".into()), "echo ••••");
        assert_eq!(
            scrub("s2", "echo hunter2-value".into()),
            "echo hunter2-value"
        );
        let (reply, wait) = mpsc::channel();
        with_vault(|vault| {
            vault.pending.push(Pending {
                request: SecretRequest {
                    id: "r".into(),
                    session_id: "s1".into(),
                    label: "Key".into(),
                    description: String::new(),
                    env_name: None,
                    path: None,
                },
                reply,
            })
        });
        end_turn("s1");
        assert!(
            matches!(wait.recv(), Ok(None)),
            "the turn's end declines the card"
        );
        assert!(with_vault(|vault| !vault.refs.contains_key("secret-ref-a")));
        assert_eq!(scrub("s1", "hunter2-value".into()), "hunter2-value");
    }

    #[test]
    fn files_are_written_inside_the_workspace_only() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        write_file(&root, ".env.local", Some("API_KEY"), "abc").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(".env.local")).unwrap(),
            "API_KEY=abc\n"
        );
        write_file(&root, ".env.local", Some("API_KEY"), "def").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(".env.local")).unwrap(),
            "API_KEY=def\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join(".env.local"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(write_file(&root, "missing/dir.txt", None, "x").is_err());
        let tools = tool_definitions();
        assert!(tools
            .iter()
            .all(|tool| tool["inputSchema"]["type"] == "object"));
    }
}
