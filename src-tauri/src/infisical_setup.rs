//! `infisical_setup_project` (ADR-198): the session tool an agent calls when the person asks to put
//! a project in Infisical. The host finds the repository's `.env*` and `.dev.vars` files, creates
//! (or reuses) the Infisical project with the machine identity, adds the person as admin, imports
//! the keys without overwriting what is there, writes `.infisical.json` and links the Sirus
//! project. Values are read here and sent straight to Infisical: the reply carries only counts, key
//! names and notes, and no value is ever formatted into a log, an error or the reply.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use reqwest::Method;
use serde_json::{json, Value};

use crate::commands::AppState;
use crate::infisical::{api, http_status_error, url_for, valid_slug, DEFAULT_ENVIRONMENT};
use crate::runtime::{AppHandle, Manager};

/// Largest env file read.
const MAX_FILE: u64 = 256 * 1024;
/// Largest value imported (Infisical's own limit is larger; the secret card takes 16 KB).
const MAX_VALUE: usize = 16 * 1024;
const MAX_FILES: usize = 200;
const MAX_DEPTH: usize = 8;
const MAX_DIRS: usize = 20_000;
/// Secrets per batch request.
const BATCH: usize = 100;

pub fn tool_definitions() -> Vec<Value> {
    vec![json!({
        "name": "infisical_setup_project",
        "description": "Use this when the person asks to put a project (its keys, its .env files) in Infisical. Sirus creates the Infisical project with the project's name, or reuses the one that has it, adds the person's Infisical account as admin, imports the repository's .env* and .dev.vars files (prod for .env.production*/.env.prod*, dev for the rest; per-app folders in a monorepo whose apps disagree on a key), never overwriting a secret that already exists unless overwrite is true, writes .infisical.json at the repository root and links the Sirus project so its terminals and scripts get the keys. The reply has only counts per environment and folder, the key names and notes: the values never pass through you, so do not read, print or copy the .env files yourself for this.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The project's folder (absolute, or relative to the session's project). Default: the session's project." },
                "projectName": { "type": "string", "description": "The Infisical project's name. Default: the folder's name." },
                "files": {
                    "type": "object",
                    "description": "Optional: the environment of specific files, by path relative to the folder, e.g. {\"apps/api/.env.staging\": \"staging\"}. \"skip\" leaves a file out.",
                    "additionalProperties": { "type": "string", "enum": ["dev", "staging", "prod", "skip"] }
                },
                "overwrite": { "type": "boolean", "description": "Replace secrets that already exist in Infisical. Default false." }
            }
        }
    })]
}

// ---- Finding and reading the files ----

/// Whether a file name is an env file the tool imports: `.env`, `.env.<anything>` or `.dev.vars`,
/// but never an example, sample or template.
pub(crate) fn is_env_file(name: &str) -> bool {
    let candidate = name == ".env" || name.starts_with(".env.") || name == ".dev.vars";
    let template = [".example", ".sample", ".template"]
        .iter()
        .any(|suffix| name.ends_with(suffix));
    candidate && !template
}

/// The environment a file's keys go to by its name, and whether the name left it unclear (those
/// go to dev, and the reply says so).
pub(crate) fn classify(name: &str) -> (&'static str, bool) {
    if name.starts_with(".env.production") || name.starts_with(".env.prod") {
        return ("prod", false);
    }
    let dev = name == ".env"
        || name == ".env.local"
        || name == ".dev.vars"
        || name.starts_with(".env.development")
        || name.starts_with(".env.dev.")
        || name == ".env.dev";
    (DEFAULT_ENVIRONMENT, !dev)
}

/// Load order inside one folder, as dotenv tools apply it: later files win.
fn precedence(name: &str) -> u8 {
    match name {
        ".env" | ".dev.vars" => 0,
        ".env.local" => 2,
        _ if name.ends_with(".local") => 3,
        _ => 1,
    }
}

/// The env files under `root`, relative to it, sorted. Skips `node_modules`, `.git`, build output,
/// symlinks and nested repositories (other checkouts and worktrees).
pub(crate) fn find_env_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0;
    while let Some((dir, depth)) = stack.pop() {
        visited += 1;
        if visited > MAX_DIRS || found.len() >= MAX_FILES {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if kind.is_dir() {
                let skipped = matches!(name.as_str(), "node_modules" | ".git" | "target");
                if !skipped && depth < MAX_DEPTH && !path.join(".git").exists() {
                    stack.push((path, depth + 1));
                }
            } else if kind.is_file() && is_env_file(&name) {
                if let Ok(relative) = path.strip_prefix(root) {
                    found.push(relative.to_path_buf());
                }
            }
        }
    }
    found.sort();
    found.truncate(MAX_FILES);
    found
}

/// `KEY=value` lines of an env file: `export`, comments, single, double and backtick quotes (double
/// quotes may span lines and take `\n`), inline ` #` comments. Lines that are not an assignment of
/// a valid name are counted, never quoted.
pub(crate) fn parse_dotenv(text: &str) -> (Vec<(String, String)>, usize) {
    let mut pairs = Vec::new();
    let mut skipped = 0;
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, rest)) = line.split_once('=') else {
            skipped += 1;
            continue;
        };
        let key = key.trim();
        if !crate::secrets::valid_env_name(key) {
            skipped += 1;
            continue;
        }
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some('"') => {
                let mut body = rest[1..].to_owned();
                let mut closed = closing_quote(&body);
                while closed.is_none() {
                    let Some(next) = lines.next() else { break };
                    body.push('\n');
                    body.push_str(next);
                    closed = closing_quote(&body);
                }
                let Some(end) = closed else {
                    skipped += 1;
                    continue;
                };
                unescape(&body[..end])
            }
            Some(quote @ ('\'' | '`')) => match rest[1..].find(quote) {
                Some(end) => rest[1..1 + end].to_owned(),
                None => {
                    skipped += 1;
                    continue;
                }
            },
            _ => {
                let bare = match rest.find(" #") {
                    Some(at) => &rest[..at],
                    None => rest,
                };
                bare.trim_end().to_owned()
            }
        };
        pairs.push((key.to_owned(), value));
    }
    (pairs, skipped)
}

/// The byte index of the first `"` not escaped by a backslash.
fn closing_quote(text: &str) -> Option<usize> {
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some(at),
            _ => escaped = false,
        }
    }
    None
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

// ---- The plan ----

/// One env file read: where it is, where its keys go, and its keys. No `Debug`, so the values can
/// never be printed by accident.
pub(crate) struct EnvFile {
    /// Path relative to the folder, with `/`.
    pub path: String,
    pub environment: String,
    pub ambiguous: bool,
    pub pairs: Vec<(String, String)>,
    pub skipped_lines: usize,
}

impl EnvFile {
    /// The folder the file is in, relative to the root ("" for the root).
    fn app(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(dir, _)| dir)
    }

    fn name(&self) -> &str {
        self.path
            .rsplit_once('/')
            .map_or(&self.path, |(_, name)| name)
    }
}

/// What goes where: `(environment, secret path)` to the keys and values, plus notes.
pub(crate) struct Plan {
    pub groups: BTreeMap<(String, String), BTreeMap<String, String>>,
    pub notes: Vec<String>,
}

/// An Infisical folder name for an app's directory: its last part, or the whole path when two
/// apps end the same way. Only letters, digits, `-` and `_`.
fn folder_names(apps: &BTreeSet<String>) -> BTreeMap<String, String> {
    let clean = |text: &str| -> String {
        text.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .chars()
            .take(64)
            .collect()
    };
    let last = |app: &str| app.rsplit('/').next().unwrap_or(app).to_owned();
    apps.iter()
        .filter(|app| !app.is_empty())
        .map(|app| {
            let short = last(app);
            let shared = apps
                .iter()
                .filter(|other| !other.is_empty() && last(other) == short)
                .count()
                > 1;
            let name = clean(if shared { app.as_str() } else { short.as_str() });
            (
                app.clone(),
                if name.is_empty() { "app".into() } else { name },
            )
        })
        .collect()
}

/// Where each file's keys go. Inside one folder, later files win as dotenv loads them. When two
/// folders of the repository give one key different values in the same environment, every app
/// keeps its own Infisical folder (`apps/web/.env` → `/web`); otherwise everything goes to `/`.
pub(crate) fn plan_import(mut files: Vec<EnvFile>) -> Plan {
    let mut notes = Vec::new();
    files.sort_by(|a, b| {
        (a.app(), precedence(a.name()), a.name()).cmp(&(b.app(), precedence(b.name()), b.name()))
    });
    // (environment, app) → keys.
    let mut merged: BTreeMap<(String, String), BTreeMap<String, String>> = BTreeMap::new();
    for file in &files {
        if file.ambiguous {
            notes.push(format!(
                "{} went to {} because its name does not say dev or prod.",
                file.path, file.environment
            ));
        }
        if file.skipped_lines > 0 {
            notes.push(format!(
                "{}: {} line(s) that are not KEY=value were left out.",
                file.path, file.skipped_lines
            ));
        }
        let keys = merged
            .entry((file.environment.clone(), file.app().to_owned()))
            .or_default();
        let mut long = 0;
        for (key, value) in &file.pairs {
            if value.len() > MAX_VALUE || value.contains('\0') {
                long += 1;
                continue;
            }
            keys.insert(key.clone(), value.clone());
        }
        if long > 0 {
            notes.push(format!(
                "{}: {long} key(s) longer than 16 KB were left out.",
                file.path
            ));
        }
    }
    let conflict = {
        let mut seen: BTreeMap<(&str, &str), &str> = BTreeMap::new();
        let mut conflict = false;
        for ((environment, _), keys) in &merged {
            for (key, value) in keys {
                match seen.insert((environment, key), value) {
                    Some(before) if before != value => conflict = true,
                    _ => {}
                }
            }
        }
        conflict
    };
    let apps: BTreeSet<String> = merged.keys().map(|(_, app)| app.clone()).collect();
    let names = folder_names(&apps);
    let mut groups: BTreeMap<(String, String), BTreeMap<String, String>> = BTreeMap::new();
    for ((environment, app), keys) in merged {
        let folder = match names.get(&app) {
            Some(name) if conflict => format!("/{name}"),
            _ => "/".to_owned(),
        };
        groups
            .entry((environment, folder))
            .or_default()
            .extend(keys);
    }
    groups.retain(|_, keys| !keys.is_empty());
    if conflict && apps.len() > 1 {
        notes.push(
            "Apps of this repository give some keys different values, so each app has its own folder."
                .to_owned(),
        );
    }
    Plan { groups, notes }
}

// ---- The reply ----

/// What happened to one `(environment, folder)`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub created: usize,
    pub updated: usize,
    /// Already in Infisical and left as they were.
    pub kept: usize,
    pub error: Option<String>,
}

/// The tool's answer: counts and key names per environment and folder, and notes. Built from the
/// plan's keys only; values are never read here.
pub(crate) fn reply_of(
    project: &Value,
    plan: &Plan,
    outcomes: &BTreeMap<(String, String), Outcome>,
    notes: &[String],
) -> Value {
    let imported: Vec<Value> = plan
        .groups
        .iter()
        .map(|(place, keys)| {
            let outcome = outcomes.get(place).cloned().unwrap_or_default();
            let mut row = json!({
                "environment": place.0,
                "folder": place.1,
                "keys": keys.keys().collect::<Vec<_>>(),
                "created": outcome.created,
                "updated": outcome.updated,
                "keptExisting": outcome.kept,
            });
            if let Some(error) = outcome.error {
                row["error"] = json!(error);
            }
            row
        })
        .collect();
    json!({
        "project": project,
        "imported": imported,
        "notes": plan.notes.iter().chain(notes).collect::<Vec<_>>(),
    })
}

// ---- Infisical ----

fn workspaces_of(reply: &Value) -> Vec<(String, String, String)> {
    reply["workspaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|w| {
            Some((
                w["id"].as_str()?.to_owned(),
                w["name"].as_str().unwrap_or_default().to_owned(),
                w["orgId"]
                    .as_str()
                    .or(w["organization"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
            ))
        })
        .collect()
}

/// The person to add as admin, from the organization's members: the only person with an email,
/// else the only admin or owner. `Err` is the note saying why nobody was picked.
pub(crate) fn pick_person(members: &Value) -> std::result::Result<String, &'static str> {
    let people: Vec<(&str, &str)> = members["users"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["status"].as_str().is_none_or(|s| s == "accepted"))
        .filter_map(|m| {
            let email = m["user"]["email"]
                .as_str()
                .or(m["user"]["username"].as_str())
                .filter(|e| e.contains('@'))?;
            Some((email, m["role"].as_str().unwrap_or_default()))
        })
        .collect();
    match people.as_slice() {
        [] => {
            Err("No person was found in the Infisical organization, so nobody was added as admin.")
        }
        [(email, _)] => Ok((*email).to_owned()),
        _ => {
            let admins: Vec<&str> = people
                .iter()
                .filter(|(_, role)| matches!(*role, "admin" | "owner"))
                .map(|(email, _)| *email)
                .collect();
            match admins.as_slice() {
                [email] => Ok((*email).to_owned()),
                _ => Err("The organization has several people, so nobody was added as admin; add yourself to the project in Infisical."),
            }
        }
    }
}

fn ok(status: u16) -> bool {
    (200..300).contains(&status)
}

/// The project with this name (case-insensitive), or a new one. `(id, orgId, created)`.
async fn ensure_project(name: &str) -> crate::error::Result<(String, String, bool)> {
    let (status, reply) = api(
        Method::GET,
        url_for(&["api", "v1", "workspace"], &[])?,
        None,
    )
    .await?;
    if !ok(status) {
        return Err(http_status_error(status));
    }
    let wanted = name.trim().to_lowercase();
    let workspaces = workspaces_of(&reply);
    if let Some((id, _, org)) = workspaces
        .iter()
        .find(|(_, other, _)| other.trim().to_lowercase() == wanted)
    {
        return Ok((id.clone(), org.clone(), false));
    }
    let (status, reply) = api(
        Method::POST,
        url_for(&["api", "v2", "workspace"], &[])?,
        Some(json!({ "projectName": name })),
    )
    .await?;
    if !ok(status) {
        return Err(http_status_error(status));
    }
    let project = &reply["project"];
    let id = project["id"]
        .as_str()
        .or(project["_id"].as_str())
        .ok_or_else(crate::infisical::unreadable)?
        .to_owned();
    let org = project["orgId"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| workspaces.first().map(|(_, _, org)| org.clone()))
        .unwrap_or_default();
    Ok((id, org, true))
}

/// Adds the person as the project's admin; the note says what happened.
async fn add_person(project: &str, org: &str) -> String {
    if !valid_slug(org) {
        return "The Infisical organization is unknown, so nobody was added as admin.".into();
    }
    let members = match api(
        Method::GET,
        match url_for(&["api", "v2", "organizations", org, "memberships"], &[]) {
            Ok(url) => url,
            Err(_) => return "The organization's members could not be read.".into(),
        },
        None,
    )
    .await
    {
        Ok((status, reply)) if ok(status) => reply,
        _ => {
            return "The organization's members could not be read (the sirus identity may lack that permission), so nobody was added as admin.".into()
        }
    };
    let email = match pick_person(&members) {
        Ok(email) => email,
        Err(note) => return note.into(),
    };
    let Ok(url) = url_for(&["api", "v2", "workspace", project, "memberships"], &[]) else {
        return "The person could not be added as admin.".into();
    };
    match api(
        Method::POST,
        url,
        Some(json!({ "emails": [email], "roleSlugs": ["admin"] })),
    )
    .await
    {
        Ok((status, _)) if ok(status) => format!("{email} was added to the project as admin."),
        Ok((400 | 409, _)) => format!("{email} is already in the project."),
        _ => format!("{email} could not be added as admin; add them in Infisical."),
    }
}

/// Makes `/name` exist in an environment; a folder that already exists is fine.
async fn ensure_folder(project: &str, environment: &str, folder: &str) -> crate::error::Result<()> {
    let name = folder.trim_start_matches('/');
    if name.is_empty() {
        return Ok(());
    }
    let (status, _) = api(
        Method::POST,
        url_for(&["api", "v1", "folders"], &[])?,
        Some(json!({ "workspaceId": project, "environment": environment, "name": name, "path": "/" })),
    )
    .await?;
    if ok(status) || status == 400 || status == 409 {
        Ok(())
    } else {
        Err(http_status_error(status))
    }
}

async fn existing_keys(
    project: &str,
    environment: &str,
    folder: &str,
) -> crate::error::Result<BTreeSet<String>> {
    let url = url_for(
        &["api", "v3", "secrets", "raw"],
        &[
            ("workspaceId", project),
            ("environment", environment),
            ("secretPath", folder),
        ],
    )?;
    let (status, reply) = api(Method::GET, url, None).await?;
    if status == 404 {
        return Ok(BTreeSet::new());
    }
    if !ok(status) {
        return Err(http_status_error(status));
    }
    Ok(crate::infisical::keys_of(&reply).into_iter().collect())
}

/// The batch request body; the values move from the plan into the request and nowhere else.
pub(crate) fn batch_body(
    project: &str,
    environment: &str,
    folder: &str,
    secrets: &[(&String, &String)],
) -> Value {
    json!({
        "workspaceId": project,
        "environment": environment,
        "secretPath": folder,
        "secrets": secrets
            .iter()
            .map(|(key, value)| json!({ "secretKey": key, "secretValue": value }))
            .collect::<Vec<_>>(),
    })
}

async fn send_batch(
    method: Method,
    project: &str,
    environment: &str,
    folder: &str,
    secrets: &[(&String, &String)],
) -> crate::error::Result<()> {
    for chunk in secrets.chunks(BATCH) {
        let (status, _) = api(
            method.clone(),
            url_for(&["api", "v3", "secrets", "batch", "raw"], &[])?,
            Some(batch_body(project, environment, folder, chunk)),
        )
        .await?;
        if !ok(status) {
            return Err(http_status_error(status));
        }
    }
    Ok(())
}

async fn import_group(
    project: &str,
    environment: &str,
    folder: &str,
    keys: &BTreeMap<String, String>,
    overwrite: bool,
) -> Outcome {
    let result: crate::error::Result<Outcome> = async {
        ensure_folder(project, environment, folder).await?;
        let existing = existing_keys(project, environment, folder).await?;
        let (old, new): (Vec<_>, Vec<_>) =
            keys.iter().partition(|(key, _)| existing.contains(*key));
        send_batch(Method::POST, project, environment, folder, &new).await?;
        let mut outcome = Outcome {
            created: new.len(),
            ..Outcome::default()
        };
        if overwrite {
            send_batch(Method::PATCH, project, environment, folder, &old).await?;
            outcome.updated = old.len();
        } else {
            outcome.kept = old.len();
        }
        Ok(outcome)
    }
    .await;
    result.unwrap_or_else(|error| Outcome {
        error: Some(error.to_string()),
        ..Outcome::default()
    })
}

// ---- The tool ----

fn arg<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args[name].as_str().map(str::trim).filter(|s| !s.is_empty())
}

/// The folder to set up and the Sirus project it belongs to (`(id, path)`), from the arguments.
fn target(
    app: &AppHandle,
    session_id: &str,
    args: &Value,
) -> Result<(PathBuf, Option<(String, PathBuf)>), String> {
    let state = app.state::<Arc<AppState>>();
    let data = state.data.lock();
    let session = data
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .ok_or("this session no longer exists")?;
    let own = data
        .projects
        .iter()
        .find(|p| p.id == session.project_id)
        .ok_or("the session's project is not in Sirus")?;
    let cwd = crate::commands::session_cwd(&data, session).ok();
    let wanted = match arg(args, "path") {
        Some(path) if Path::new(path).is_absolute() => PathBuf::from(path),
        Some(path) => Path::new(&own.path).join(path),
        None => PathBuf::from(&own.path),
    };
    let folder = wanted
        .canonicalize()
        .map_err(|_| "that folder does not exist".to_string())?;
    if !folder.is_dir() {
        return Err("`path` must be a folder".into());
    }
    let canonical = |path: &str| Path::new(path).canonicalize().ok();
    let project = data
        .projects
        .iter()
        .filter_map(|p| canonical(&p.path).map(|root| (p, root)))
        .filter(|(_, root)| folder.starts_with(root))
        .max_by_key(|(_, root)| root.as_os_str().len())
        .map(|(p, root)| (p.id.clone(), root));
    let in_session = cwd
        .and_then(|cwd| cwd.canonicalize().ok())
        .is_some_and(|cwd| folder.starts_with(cwd));
    if project.is_none() && !in_session {
        return Err("`path` must be inside a project in Sirus".into());
    }
    Ok((folder, project))
}

/// Reads the files found under `folder` with their environment and keys.
fn read_files(
    folder: &Path,
    mapping: &serde_json::Map<String, Value>,
) -> (Vec<EnvFile>, Vec<String>) {
    let mut notes = Vec::new();
    let mut files = Vec::new();
    for relative in find_env_files(folder) {
        let path = relative.to_string_lossy().replace('\\', "/");
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let (environment, mut ambiguous) = classify(&name);
        let mut chosen = None;
        if let Some(given) = mapping.get(&path).and_then(Value::as_str) {
            if given == "skip" {
                continue;
            }
            if valid_slug(given) {
                chosen = Some(given.to_owned());
                ambiguous = false;
            }
        }
        let full = folder.join(&relative);
        let readable = std::fs::metadata(&full).is_ok_and(|m| m.len() <= MAX_FILE);
        let Some(text) = readable
            .then(|| std::fs::read_to_string(&full).ok())
            .flatten()
        else {
            notes.push(format!("{path} could not be read and was left out."));
            continue;
        };
        let (pairs, skipped_lines) = parse_dotenv(&text);
        if pairs.is_empty() {
            continue;
        }
        let environment = chosen.unwrap_or_else(|| environment.to_owned());
        files.push(EnvFile {
            path,
            environment,
            ambiguous,
            pairs,
            skipped_lines,
        });
    }
    for given in mapping.keys() {
        if !files.iter().any(|f| &f.path == given) && mapping[given] != "skip" {
            notes.push(format!(
                "{given} was named but is not an env file found in the folder."
            ));
        }
    }
    (files, notes)
}

/// Session tool entry; blocks on the Infisical calls (it runs on a blocking thread).
pub fn execute(app: &AppHandle, session_id: &str, args: &Value) -> Result<Value, String> {
    let (folder, sirus_project) = target(app, session_id, args)?;
    let name = arg(args, "projectName")
        .map(str::to_owned)
        .or_else(|| folder.file_name().map(|n| n.to_string_lossy().into_owned()))
        .filter(|n| n.chars().count() <= 64 && !n.contains(char::is_control))
        .ok_or("give a projectName of up to 64 characters")?;
    let overwrite = args["overwrite"].as_bool() == Some(true);
    let empty = serde_json::Map::new();
    let mapping = args["files"].as_object().unwrap_or(&empty);
    let (files, mut notes) = read_files(&folder, mapping);
    let plan = plan_import(files);
    if plan.groups.is_empty() {
        notes.push(
            "No .env or .dev.vars file with keys was found; the project was still set up.".into(),
        );
    }

    let (project_id, created, outcomes, person) = crate::runtime::async_runtime::block_on(async {
        let (id, org, created) = ensure_project(&name).await?;
        let person = add_person(&id, &org).await;
        let mut outcomes = BTreeMap::new();
        for ((environment, path), keys) in &plan.groups {
            let outcome = import_group(&id, environment, path, keys, overwrite).await;
            outcomes.insert((environment.clone(), path.clone()), outcome);
        }
        crate::error::Result::Ok((id, created, outcomes, person))
    })
    .map_err(|error| error.to_string())?;
    notes.push(person);

    // Link the repository and the Sirus project.
    let root = crate::infisical::repo_root(&folder).unwrap_or_else(|| folder.clone());
    let environment = crate::infisical::read_link(&root)
        .map(|(_, env)| env)
        .unwrap_or_else(|| DEFAULT_ENVIRONMENT.to_owned());
    crate::infisical::write_link(&root, &project_id, &environment).map_err(|e| e.to_string())?;
    let mut linked = None;
    if let Some((id, project_root)) = sirus_project {
        // A link file in the project's own folder would win over the root's, so it follows.
        if project_root != root && crate::infisical::read_link(&project_root).is_some() {
            let _ = crate::infisical::write_link(&project_root, &project_id, &environment);
        }
        let state = app.state::<Arc<AppState>>();
        let mut data = state.data.lock();
        if let Some(project) = data.projects.iter_mut().find(|p| p.id == id) {
            project.scripts.infisical_auto = None;
            linked = Some(project.name.clone());
            if project.scripts.infisical_off {
                notes.push("The keys are off for this project in Sirus; turn them on in Settings → Conectores → Infisical → Projetos e chaves.".into());
            }
        }
        let _ = crate::persist::save(&state.data_path, &data);
    }
    let project = json!({
        "name": name,
        "id": project_id,
        "created": created,
        "linkFile": root.join(crate::infisical::LINK_FILE).to_string_lossy(),
        "sirusProject": linked,
    });
    Ok(reply_of(&project, &plan, &outcomes, &notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_files_are_recognised_and_templates_left_out() {
        for name in [
            ".env",
            ".env.local",
            ".env.production",
            ".dev.vars",
            ".env.staging",
        ] {
            assert!(is_env_file(name), "{name}");
        }
        for name in [
            ".env.example",
            ".env.local.sample",
            ".env.template",
            ".envrc",
            "env",
            ".dev.vars.example",
        ] {
            assert!(!is_env_file(name), "{name}");
        }
    }

    #[test]
    fn files_are_classified_by_name_and_unclear_ones_go_to_dev() {
        assert_eq!(classify(".env.production"), ("prod", false));
        assert_eq!(classify(".env.production.local"), ("prod", false));
        assert_eq!(classify(".env.prod"), ("prod", false));
        assert_eq!(classify(".env"), ("dev", false));
        assert_eq!(classify(".env.local"), ("dev", false));
        assert_eq!(classify(".env.development"), ("dev", false));
        assert_eq!(classify(".env.development.local"), ("dev", false));
        assert_eq!(classify(".dev.vars"), ("dev", false));
        assert_eq!(classify(".env.staging"), ("dev", true));
        assert_eq!(classify(".env.test"), ("dev", true));
    }

    #[test]
    fn dotenv_lines_are_parsed_and_bad_ones_counted() {
        let text = "# comment\nexport A=1\nB = \"two\\nlines\"\nC='lit $x'\nD=plain # note\nE=\"multi\nline\"\nnot a line\n1BAD=x\nF=\n";
        let (pairs, skipped) = parse_dotenv(text);
        let map: BTreeMap<_, _> = pairs.into_iter().collect();
        assert_eq!(map["A"], "1");
        assert_eq!(map["B"], "two\nlines");
        assert_eq!(map["C"], "lit $x");
        assert_eq!(map["D"], "plain");
        assert_eq!(map["E"], "multi\nline");
        assert_eq!(map["F"], "");
        assert_eq!(skipped, 2);
    }

    #[test]
    fn the_walk_skips_dependencies_git_and_nested_checkouts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            ".env",
            ".env.example",
            "apps/web/.env.local",
            "node_modules/x/.env",
            ".git/.env",
            "nested/.git/HEAD",
            "nested/.env",
            "apps/api/.dev.vars",
        ] {
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, "A=1").unwrap();
        }
        let found: Vec<String> = find_env_files(root)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            found,
            vec![".env", "apps/api/.dev.vars", "apps/web/.env.local"]
        );
    }

    fn file(path: &str, pairs: &[(&str, &str)]) -> EnvFile {
        let name = path.rsplit('/').next().unwrap();
        let (environment, ambiguous) = classify(name);
        EnvFile {
            path: path.into(),
            environment: environment.into(),
            ambiguous,
            pairs: pairs
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
            skipped_lines: 0,
        }
    }

    #[test]
    fn a_repository_without_conflicts_goes_to_the_root_and_later_files_win() {
        let plan = plan_import(vec![
            file(".env.local", &[("A", "local")]),
            file(".env", &[("A", "base"), ("B", "b")]),
            file("apps/web/.env", &[("C", "c")]),
            file(".env.production", &[("A", "prod")]),
            file(".env.staging", &[("S", "s")]),
        ]);
        let dev = &plan.groups[&("dev".to_owned(), "/".to_owned())];
        assert_eq!(dev["A"], "local");
        assert_eq!(dev.len(), 4);
        assert_eq!(
            plan.groups[&("prod".to_owned(), "/".to_owned())]["A"],
            "prod"
        );
        assert_eq!(plan.groups.len(), 2);
        assert!(plan
            .notes
            .iter()
            .any(|n| n.contains(".env.staging") && n.contains("dev")));
    }

    #[test]
    fn apps_that_disagree_keep_their_own_folders() {
        let plan = plan_import(vec![
            file("apps/web/.env", &[("PORT", "3000"), ("WEB", "w")]),
            file("apps/api/.env", &[("PORT", "4000")]),
            file(".env", &[("SHARED", "s")]),
            file("packages/web/.env", &[("X", "x")]),
        ]);
        let keys: Vec<(String, String)> = plan.groups.keys().cloned().collect();
        assert!(keys.contains(&("dev".into(), "/".into())));
        assert!(keys.contains(&("dev".into(), "/api".into())));
        // Two apps called "web": the whole path names each.
        assert!(keys.contains(&("dev".into(), "/apps-web".into())));
        assert!(keys.contains(&("dev".into(), "/packages-web".into())));
    }

    #[test]
    fn the_reply_names_keys_and_never_carries_a_value() {
        let plan = plan_import(vec![
            file(
                ".env",
                &[
                    ("STRIPE_KEY", "sk_live_hunter2"),
                    ("DB_URL", "postgres://u:pw@h/db"),
                ],
            ),
            file(".env.production", &[("STRIPE_KEY", "sk_live_prodvalue")]),
        ]);
        let mut outcomes = BTreeMap::new();
        outcomes.insert(
            ("dev".to_owned(), "/".to_owned()),
            Outcome {
                created: 1,
                kept: 1,
                ..Outcome::default()
            },
        );
        let reply = reply_of(&json!({"name": "app"}), &plan, &outcomes, &["note".into()]);
        let text = reply.to_string();
        for value in ["hunter2", "pw@h", "prodvalue"] {
            assert!(!text.contains(value), "{value} leaked");
        }
        assert!(text.contains("STRIPE_KEY") && text.contains("DB_URL"));
        assert_eq!(reply["imported"][0]["created"], 1);
        assert_eq!(reply["imported"][0]["keptExisting"], 1);
    }

    #[test]
    fn the_batch_body_has_the_documented_shape() {
        let (k, v) = ("A".to_owned(), "1".to_owned());
        let body = batch_body("ws", "dev", "/web", &[(&k, &v)]);
        assert_eq!(
            body,
            json!({"workspaceId": "ws", "environment": "dev", "secretPath": "/web",
                   "secrets": [{"secretKey": "A", "secretValue": "1"}]})
        );
    }

    #[test]
    fn the_admin_is_the_only_person_or_the_only_admin() {
        let one = json!({"users": [{"role": "member", "status": "accepted", "user": {"email": "a@x.com"}}]});
        assert_eq!(pick_person(&one).unwrap(), "a@x.com");
        let two = json!({"users": [
            {"role": "member", "user": {"email": "a@x.com"}},
            {"role": "admin", "user": {"email": "b@x.com"}},
            {"role": "admin", "status": "invited", "user": {"email": "c@x.com"}}
        ]});
        assert_eq!(pick_person(&two).unwrap(), "b@x.com");
        let many = json!({"users": [
            {"role": "admin", "user": {"email": "a@x.com"}},
            {"role": "admin", "user": {"email": "b@x.com"}}
        ]});
        assert!(pick_person(&many).is_err());
        assert!(pick_person(&json!({})).is_err());
    }
}
