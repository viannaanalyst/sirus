//! Read-only skill discovery and explicit turn-local portable instructions.
use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::AgentProviderId;
use crate::runtime::State;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_FILES: usize = 2048;
const MAX_ENTRIES: usize = 8192;
const MAX_BYTES: usize = 64 * 1024;
const MAX_NAMES: usize = 512;
const INSTRUCTIONS_HEADER: &str = "The user explicitly invoked these skills for this turn. Apply their instructions within the existing approval policy.\n";
const ROOTS: &[(&str, &str)] = &[
    ("agents", ".agents/skills"),
    ("codex", ".codex/skills"),
    ("claude", ".claude/skills"),
    ("cursor", ".cursor/skills"),
    ("cursor", ".cursor/skills-cursor"),
    ("opencode", ".config/opencode/skills"),
    ("opencode", ".opencode/skills"),
    ("grok", ".grok/skills"),
    ("antigravity", ".gemini/antigravity/skills"),
    ("droid", ".factory/skills"),
    ("pi", ".pi/agent/skills"),
    ("devin", ".config/devin/skills"),
    ("devin", ".config/cognition/skills"),
    ("devin", ".codeium/windsurf/skills"),
];

/// Where each provider's CLI reads the person's skills, by the folder that says it's installed.
const PROVIDER_SKILLS: &[(&str, &str)] = &[
    (".claude", ".claude/skills"),
    (".codex", ".codex/skills"),
    (".agents", ".agents/skills"),
    (".cursor", ".cursor/skills"),
    (".config/opencode", ".config/opencode/skills"),
    (".grok", ".grok/skills"),
    (".factory", ".factory/skills"),
    (".pi/agent", ".pi/agent/skills"),
];
static PORTABLE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Makes the app's skills reachable from every installed provider's own folder.
pub fn publish(data_dir: &Path) {
    let portable = data_dir.join("skills");
    let _ = PORTABLE.set(portable.clone());
    let Some(home) = crate::new_project::home_dir() else {
        return;
    };
    std::thread::spawn(move || {
        for (installed, skills) in PROVIDER_SKILLS {
            if home.join(installed).is_dir() {
                link_skills(&portable, &home.join(skills));
            }
        }
    });
}

/// A named account's CLI reads skills from its own folder only, so the person's and the app's
/// are linked in each time it starts.
pub(crate) fn share_with_account(user_skills: &str, account_skills: &Path) {
    if let Some(home) = crate::new_project::home_dir() {
        link_skills(&home.join(user_skills), account_skills);
    }
    if let Some(portable) = PORTABLE.get() {
        link_skills(portable, account_skills);
    }
}

/// Links each skill of `from` into `to`, never over what's there, and clears links left by a
/// skill that was removed.
fn link_skills(from: &Path, to: &Path) {
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    if std::fs::create_dir_all(to).is_err() {
        return;
    }
    if let Ok(existing) = std::fs::read_dir(to) {
        for link in existing.flatten() {
            let path = link.path();
            let dangling = std::fs::read_link(&path)
                .is_ok_and(|target| target.starts_with(from) && !target.exists());
            if dangling {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    for entry in entries.flatten() {
        let name = entry.file_name();
        let target = to.join(&name);
        if name.to_string_lossy().starts_with('.')
            || !entry.path().join("SKILL.md").exists()
            || std::fs::symlink_metadata(&target).is_ok()
        {
            continue;
        }
        #[cfg(unix)]
        if let Err(error) = std::os::unix::fs::symlink(entry.path(), &target) {
            tracing::warn!(%error, "cannot link a skill");
        }
    }
}

/// What an agent is told about pages and skills, in its system instructions.
pub(crate) fn agent_hint() -> String {
    let browser = "To open, preview or test any web page, always use Sirus's built-in browser (the browser_* tools of the sirus_browser MCP server; load them with tool search if deferred), never the `open` command, a system browser or a browser you start yourself. It takes http(s) URLs, so serve local files from 127.0.0.1 first. Computer use on a web page goes through this browser too. If those tools are not available to you, say so instead of opening another browser.";
    let project = "Project skills may also sit in the workspace's .agents/skills, .sirus/skills and .claude/skills.";
    match PORTABLE.get() {
        Some(portable) => format!(
            "{browser} Every skill the person has lives at {}/<name>/SKILL.md. {project} When a skill is named or clearly fits and your skill tool does not offer it, find it in these folders and read its SKILL.md instead of saying it did not load.",
            portable.display()
        ),
        None => format!("{browser} {project}"),
    }
}

/// Claude and Codex take the hint as system instructions. Every other provider, and any added
/// later, has no such channel in common, so it reads it at the top of a thread's first prompt.
pub(crate) fn with_agent_hint(session: &crate::models::Session, prompt: String) -> String {
    if hint_is_in_prompt(&session.agent, session.native_thread.is_some()) {
        format!("{}\n\nUser request:\n{prompt}", agent_hint())
    } else {
        prompt
    }
}

fn hint_is_in_prompt(provider: &AgentProviderId, resumes: bool) -> bool {
    !resumes && !matches!(provider, AgentProviderId::Claude | AgentProviderId::Codex)
}

/// The app's skills folder, for the agents' read access.
pub(crate) fn portable_skills() -> Option<&'static Path> {
    PORTABLE.get().map(PathBuf::as_path)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSource {
    pub id: String,
    pub origin: String,
    pub scope: String,
    pub path: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub sources: Vec<SkillSource>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub portable_dir: String,
    pub skills: Vec<Skill>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Owner {
    pub project_id: Option<String>,
    pub session_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    Catalog {
        owner: Owner,
    },
    Preview {
        owner: Owner,
        #[serde(rename = "sourceId")]
        source_id: String,
    },
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Response {
    Catalog { catalog: Catalog },
    Preview { document: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Context {
    pub cwd: Option<PathBuf>,
    pub provider: Option<AgentProviderId>,
    pub profile_home: Option<PathBuf>,
    pub account_id: Option<String>,
    pub disabled: Vec<String>,
}
pub(crate) fn context(state: &AppState, owner: &Owner) -> Result<Context> {
    let data = state.data.lock();
    state.ensure_running()?;
    let (cwd, provider, account) = if let Some(id) = &owner.session_id {
        let session = data
            .sessions
            .iter()
            .find(|s| &s.id == id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        if owner
            .project_id
            .as_ref()
            .is_some_and(|id| id != &session.project_id)
        {
            return Err(Error::new("invalid", "foreign skill owner"));
        }
        (
            Some(crate::commands::session_cwd(&data, session)?),
            Some(session.agent.clone()),
            Some(session.provider_account_id.clone()),
        )
    } else if let Some(id) = &owner.project_id {
        let project = data
            .projects
            .iter()
            .find(|p| &p.id == id)
            .ok_or_else(|| Error::not_found("project not found"))?;
        let provider = data.settings.default_agent.clone();
        (
            Some(crate::paths::ensure_dir(Path::new(&project.path))?),
            Some(provider.clone()),
            Some(crate::provider_accounts::selected(&data, &provider)),
        )
    } else {
        (None, None, None)
    };
    let disabled = data.settings.disabled_skills.clone();
    drop(data);
    let profile_home = match (&provider, &account) {
        (Some(provider), Some(id)) if crate::provider_accounts::supported(provider) => {
            crate::provider_accounts::scope(state, provider, id)?.home
        }
        _ => None,
    };
    Ok(Context {
        cwd,
        provider,
        profile_home,
        account_id: account,
        disabled,
    })
}
pub fn validate_disabled(names: &[String]) -> Result<()> {
    if names.len() > MAX_NAMES
        || names.iter().any(|name| !valid_name(name))
        || names.iter().collect::<HashSet<_>>().len() != names.len()
    {
        return Err(Error::new(
            "invalid_settings",
            "Invalid disabled skill names.",
        ));
    }
    Ok(())
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-_.:".contains(&c))
}
fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}
#[derive(Clone)]
struct Root {
    path: PathBuf,
    boundary: PathBuf,
    origin: String,
    scope: String,
}
#[derive(Clone)]
struct AllowedRoot {
    path: PathBuf,
    boundary: PathBuf,
}
fn roots(home: Option<&Path>, portable: &Path, context: &Context) -> Vec<Root> {
    let mut out = Vec::new();
    if let Some(cwd) = &context.cwd {
        for (origin, suffix) in ROOTS {
            let suffix = if *origin == "opencode" {
                ".opencode/skills"
            } else if *origin == "pi" {
                ".pi/skills"
            } else {
                suffix
            };
            out.push(Root {
                path: cwd.join(suffix),
                boundary: cwd.clone(),
                origin: (*origin).into(),
                scope: "project".into(),
            });
        }
        out.push(Root {
            path: cwd.join(".sirus/skills"),
            boundary: cwd.clone(),
            origin: "sirus".into(),
            scope: "project".into(),
        });
    }
    out.push(Root {
        path: portable.into(),
        boundary: portable.parent().unwrap_or(portable).into(),
        origin: "sirus".into(),
        scope: "user".into(),
    });
    if let Some(home) = home {
        for (origin, suffix) in ROOTS {
            let profile = context.profile_home.as_ref().filter(|_| {
                context
                    .provider
                    .as_ref()
                    .is_some_and(|p| p.key() == *origin)
            });
            let path = profile.map_or_else(|| home.join(suffix), |profile| profile.join("skills"));
            out.push(Root {
                path,
                boundary: profile.map_or_else(|| home.into(), Clone::clone),
                origin: (*origin).into(),
                scope: "user".into(),
            });
        }
    }
    out
}
fn portable_dir(state: &AppState) -> Result<PathBuf> {
    let path = state
        .data_path
        .parent()
        .ok_or_else(|| Error::new("native", "skill storage unavailable"))?
        .join("skills");
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink() || !m.is_dir()) {
        return Err(Error::new("invalid", "unsafe portable skill folder"));
    }
    std::fs::create_dir_all(&path)?;
    Ok(path)
}
fn bounded_read(path: &Path, allowed: &[AllowedRoot]) -> Result<String> {
    let path = std::fs::canonicalize(path)?;
    if !allowed.iter().any(|root| path.starts_with(&root.path))
        || path.file_name().is_none_or(|name| name != "SKILL.md")
    {
        return Err(Error::new(
            "invalid",
            "skill document outside catalog roots",
        ));
    }
    let root = allowed
        .iter()
        .find(|root| path.starts_with(&root.path))
        .unwrap();
    // Mutable skill subdirectories never become descriptor authority.
    let file = crate::paths::open_regular_within(&root.boundary, &path)?;
    if crate::paths::ensure_within(&root.boundary, &root.path)? != root.path {
        return Err(Error::invalid_path("skill root changed during read"));
    }
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_BYTES as u64 {
        return Err(Error::new(
            "invalid",
            "skill document exceeds the 64 KiB limit or is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(Error::new("invalid", "skill document changed size"));
    }
    String::from_utf8(bytes).map_err(|_| Error::new("invalid", "skill document is not UTF-8"))
}
fn frontmatter(document: &str, fallback: &str) -> Option<(String, String)> {
    let normalized = document
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    let mut name = fallback.to_lowercase();
    let mut description = String::new();
    if let Some(rest) = normalized.strip_prefix("---\n") {
        if let Some((header, _)) = rest.split_once("\n---") {
            let mut multiline = false;
            for line in header.lines().take(200) {
                if multiline && line.starts_with(char::is_whitespace) {
                    description.push(' ');
                    description.push_str(line.trim());
                    continue;
                }
                multiline = false;
                if line.starts_with(char::is_whitespace) {
                    continue;
                }
                if let Some((key, value)) = line.split_once(':') {
                    let value = value.trim();
                    let value = serde_json::from_str::<String>(value)
                        .unwrap_or_else(|_| value.trim_matches('\'').into());
                    match key.trim() {
                        "name" => name = value.trim().to_lowercase(),
                        "description" => {
                            multiline = matches!(value.as_str(), ">" | "|" | ">-" | "|-");
                            description = if multiline { String::new() } else { value };
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    if !valid_name(&name) {
        return None;
    }
    Some((
        name,
        description
            .chars()
            .filter(|c| !c.is_control())
            .take(1200)
            .collect::<String>()
            .trim()
            .into(),
    ))
}
fn admitted_root(root: &Root) -> Option<AllowedRoot> {
    let boundary = crate::paths::ensure_dir(&root.boundary).ok()?;
    let path = crate::paths::ensure_within(&boundary, &root.path).ok()?;
    path.is_dir().then_some(AllowedRoot { path, boundary })
}
fn scan(roots: &[Root], portable: &Path) -> Catalog {
    let allowed: Vec<AllowedRoot> = roots.iter().filter_map(admitted_root).collect();
    let mut groups: BTreeMap<String, Skill> = BTreeMap::new();
    let mut budget = MAX_ENTRIES;
    let mut files = 0;
    let mut truncated = false;
    for root in roots {
        let Some(admitted) = admitted_root(root) else {
            continue;
        };
        let mut pending = vec![(admitted.path, 0)];
        let mut visited = HashSet::new();
        while let Some((path, depth)) = pending.pop() {
            if budget == 0 || files == MAX_FILES {
                truncated = true;
                break;
            }
            let Ok(canonical) = std::fs::canonicalize(&path) else {
                continue;
            };
            if !allowed.iter().any(|r| canonical.starts_with(&r.path))
                || !visited.insert(canonical.clone())
            {
                continue;
            }
            if depth > 4 {
                truncated = true;
                continue;
            }
            let document = path.join("SKILL.md");
            if let Ok(text) = bounded_read(&document, &allowed) {
                files += 1;
                let fallback = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
                if let Some((name, description)) = frontmatter(&text, fallback) {
                    if !groups.contains_key(&name) && groups.len() == MAX_NAMES {
                        truncated = true;
                        continue;
                    }
                    let actual = std::fs::canonicalize(&document).unwrap_or(document);
                    let path = actual.to_string_lossy().into_owned();
                    let id = format!(
                        "{:x}",
                        Sha256::digest(format!("{}:{}:{path}", root.scope, root.origin).as_bytes())
                    );
                    let group = groups.entry(name.clone()).or_insert_with(|| Skill {
                        name,
                        description,
                        sources: vec![],
                    });
                    if !group.sources.iter().any(|s| s.id == id) {
                        group.sources.push(SkillSource {
                            id,
                            origin: root.origin.clone(),
                            scope: root.scope.clone(),
                            path,
                        });
                    }
                }
                continue; // A skill's scripts/assets are not additional skill roots.
            }
            let Ok(entries) = std::fs::read_dir(path) else {
                continue;
            };
            let mut paths = Vec::new();
            for entry in entries {
                if budget == 0 {
                    truncated = true;
                    break;
                }
                budget -= 1;
                let Ok(entry) = entry else { continue };
                if matches!(
                    entry.file_name().to_str(),
                    Some("node_modules" | ".git" | "target")
                ) {
                    continue;
                }
                paths.push(entry.path());
            }
            paths.sort();
            for path in paths.into_iter().rev() {
                if path.is_dir() {
                    pending.push((path, depth + 1));
                }
            }
        }
    }
    Catalog {
        portable_dir: portable.to_string_lossy().into_owned(),
        skills: groups.into_values().collect(),
        truncated,
    }
}
fn discover(state: &AppState, context: &Context) -> Result<(Catalog, Vec<AllowedRoot>)> {
    let portable = portable_dir(state)?;
    let roots = roots(home_dir().as_deref(), &portable, context);
    let allowed = roots.iter().filter_map(admitted_root).collect();
    Ok((scan(&roots, &portable), allowed))
}
pub async fn skill_action(state: State<'_, Arc<AppState>>, action: Action) -> Result<Response> {
    let state = state.inner().clone();
    crate::commands::native_task(move || {
        let owner = match &action {
            Action::Catalog { owner } | Action::Preview { owner, .. } => owner,
        };
        let snapshot = context(&state, owner)?;
        let (catalog, allowed) = discover(&state, &snapshot)?;
        let response = match &action {
            Action::Catalog { .. } => Response::Catalog { catalog },
            Action::Preview { source_id, .. } => {
                if source_id.len() != 64 {
                    return Err(Error::new("invalid", "invalid skill source"));
                }
                let source = catalog
                    .skills
                    .iter()
                    .flat_map(|s| &s.sources)
                    .find(|s| &s.id == source_id)
                    .ok_or_else(|| Error::not_found("skill source not found"))?;
                Response::Preview {
                    document: bounded_read(Path::new(&source.path), &allowed)?,
                }
            }
        };
        if context(&state, owner)? != snapshot {
            return Err(Error::new(
                "stale",
                "Skill owner changed. Refresh and try again.",
            ));
        }
        Ok(response)
    })
    .await
}
fn instructions(
    catalog: &Catalog,
    allowed: &[AllowedRoot],
    context: &Context,
    prompt: &str,
) -> Result<String> {
    let mut out = String::new();
    let mut names = HashSet::new();
    // A `/skill` token invokes wherever it stands in the prompt, so a skill chosen
    // mid-sentence stays where it was typed. Other slash words (paths, prose) are ignored.
    for token in prompt.split_whitespace() {
        let Some(name) = token.strip_prefix('/') else {
            continue;
        };
        let name = name.to_lowercase();
        let Some(skill) = catalog.skills.iter().find(|s| s.name == name) else {
            continue;
        };
        if context.disabled.contains(&name) {
            return Err(Error::new(
                "invalid",
                "This skill is disabled in Sirus Code.",
            ));
        }
        if !names.insert(name) {
            continue;
        }
        if names.len() > 4 {
            return Err(Error::new(
                "invalid",
                "At most four skills may be invoked per turn.",
            ));
        }
        let source = skill
            .sources
            .iter()
            .min_by_key(|s| {
                (
                    s.scope != "project",
                    context
                        .provider
                        .as_ref()
                        .is_none_or(|p| p.key() != s.origin),
                    s.origin != "agents",
                    s.origin != "sirus",
                    &s.path,
                )
            })
            .ok_or_else(|| Error::not_found("skill source unavailable"))?;
        let content = bounded_read(Path::new(&source.path), allowed)?;
        let dir = Path::new(&source.path).parent().unwrap().to_string_lossy();
        let block = format!(
            "\nSkill {} (relative references resolve from {}):\n{}\n",
            serde_json::to_string(&skill.name)?,
            serde_json::to_string(&dir)?,
            content
        );
        if INSTRUCTIONS_HEADER.len() + out.len() + block.len() > MAX_BYTES {
            return Err(Error::new(
                "invalid",
                "Invoked skills exceed the 64 KiB turn limit.",
            ));
        }
        out.push_str(&block);
    }
    if !out.is_empty() {
        out.insert_str(0, INSTRUCTIONS_HEADER);
    }
    Ok(out)
}
/// The admitted instructions plus the invoked skill names in prompt order (shown in the turn's activity).
pub(crate) fn prepare(
    state: &AppState,
    owner: &Owner,
    prompt: &str,
) -> Result<(Context, String, Vec<String>)> {
    let context = context(state, owner)?;
    let (catalog, allowed) = discover(state, &context)?;
    let text = instructions(&catalog, &allowed, &context, prompt)?;
    let mut names: Vec<String> = Vec::new();
    for token in prompt.split_whitespace() {
        let Some(name) = token.strip_prefix('/').map(str::to_lowercase) else {
            continue;
        };
        if !names.contains(&name) && catalog.skills.iter().any(|skill| skill.name == name) {
            names.push(name);
        }
    }
    names.truncate(4);
    Ok((context, text, names))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn allowed(path: &Path) -> AllowedRoot {
        AllowedRoot {
            path: path.into(),
            boundary: path.into(),
        }
    }
    fn fixture() -> Context {
        Context {
            cwd: None,
            provider: Some(AgentProviderId::Codex),
            profile_home: None,
            account_id: None,
            disabled: vec![],
        }
    }
    fn put(root: &Path, folder: &str, text: &str) {
        let path = root.join(folder);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("SKILL.md"), text).unwrap();
    }
    #[test]
    fn only_providers_without_system_instructions_read_the_hint_in_the_prompt() {
        assert!(!hint_is_in_prompt(&AgentProviderId::Claude, false));
        assert!(!hint_is_in_prompt(&AgentProviderId::Codex, false));
        assert!(hint_is_in_prompt(&AgentProviderId::Cursor, false));
        assert!(hint_is_in_prompt(&AgentProviderId::OpenCode, false));
        assert!(!hint_is_in_prompt(&AgentProviderId::OpenCode, true));
    }

    #[test]
    fn skills_link_in_without_touching_what_is_there_and_gone_ones_leave() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("app"), root.path().join("cli/skills"));
        for name in ["shared", "own", ".trash", "gone"] {
            std::fs::create_dir_all(from.join(name)).unwrap();
            std::fs::write(from.join(name).join("SKILL.md"), "x").unwrap();
        }
        std::fs::create_dir_all(from.join("not-a-skill")).unwrap();
        std::fs::create_dir_all(to.join("own")).unwrap();
        link_skills(&from, &to);
        let is_link = |name: &str| {
            std::fs::symlink_metadata(to.join(name))
                .unwrap()
                .file_type()
                .is_symlink()
        };
        assert!(is_link("shared") && is_link("gone"));
        assert!(!is_link("own"));
        assert!(!to.join(".trash").exists() && !to.join("not-a-skill").exists());
        std::fs::remove_dir_all(from.join("gone")).unwrap();
        link_skills(&from, &to);
        assert!(std::fs::symlink_metadata(to.join("gone")).is_err());
        assert!(is_link("shared"));
    }

    #[test]
    fn frontmatter_handles_quotes_multiline_crlf_and_invalid_names() {
        assert_eq!(frontmatter("---\r\nname: 'Example'\r\ndescription: >\r\n  First line\r\n  second line\r\n---\r\n", "fallback"), Some(("example".into(), "First line second line".into())));
        assert!(frontmatter("---\nname: ../escape\n---\n", "fallback").is_none());
        assert_eq!(
            frontmatter("plain text", "fallback"),
            Some(("fallback".into(), String::new()))
        );
    }
    #[test]
    fn groups_copies_and_native_invocation_precedence_without_mutating_files() {
        let dir = tempfile::tempdir().unwrap();
        let portable = dir.path().join("portable");
        let codex = dir.path().join("codex");
        put(
            &portable,
            "test",
            "---\nname: test\ndescription: portable\n---\nPORTABLE",
        );
        put(&codex, "test", "---\nname: test\n---\nNATIVE");
        let portable = std::fs::canonicalize(portable).unwrap();
        let codex = std::fs::canonicalize(codex).unwrap();
        let roots = vec![
            Root {
                path: portable.clone(),
                boundary: portable.clone(),
                origin: "sirus".into(),
                scope: "user".into(),
            },
            Root {
                path: codex.clone(),
                boundary: codex.clone(),
                origin: "codex".into(),
                scope: "user".into(),
            },
        ];
        let catalog = scan(&roots, &portable);
        assert_eq!(catalog.skills.len(), 1);
        assert_eq!(catalog.skills[0].sources.len(), 2);
        let text = instructions(
            &catalog,
            &[allowed(&portable), allowed(&codex)],
            &fixture(),
            "/test work",
        )
        .unwrap();
        assert!(text.contains("NATIVE"));
        assert!(!text.contains("PORTABLE"));
        // A skill token anywhere in the prompt invokes it, so one chosen mid-sentence stays put.
        assert!(instructions(
            &catalog,
            &[allowed(&portable), allowed(&codex)],
            &fixture(),
            "fix the docs with /test please"
        )
        .unwrap()
        .contains("NATIVE"));
        assert!(instructions(
            &catalog,
            &[allowed(&portable)],
            &fixture(),
            "see /usr/local/bin and /tmp"
        )
        .unwrap()
        .is_empty());
        let mut context = fixture();
        context.disabled.push("test".into());
        assert!(instructions(&catalog, &[allowed(&portable)], &context, "/test work").is_err());
    }
    #[test]
    fn bounds_and_catalog_root_authority_are_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skills");
        put(&root, "test", "hello");
        let root = std::fs::canonicalize(root).unwrap();
        assert!(bounded_read(&root.join("test/SKILL.md"), &[allowed(&root)]).is_ok());
        assert!(bounded_read(&root.join("test/SKILL.md"), &[]).is_err());
        put(&root, "large", &"x".repeat(MAX_BYTES + 1));
        assert!(bounded_read(&root.join("large/SKILL.md"), &[allowed(&root)]).is_err());
        assert!(validate_disabled(&["test".into(), "test".into()]).is_err());
        assert!(validate_disabled(&["../x".into()]).is_err());
        assert!(serde_json::from_str::<Action>(r#"{"type":"preview","owner":{"projectId":null,"sessionId":null},"sourceId":"x","path":"/etc/passwd"}"#).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn replacing_an_admitted_skill_root_cannot_retarget_its_read_boundary() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let root = project.join(".agents/skills");
        let outside = dir.path().join("outside");
        put(&root, "test", "safe");
        put(&outside, "test", "foreign");
        let admitted = admitted_root(&Root {
            path: root.clone(),
            boundary: project,
            origin: "agents".into(),
            scope: "project".into(),
        })
        .unwrap();
        std::fs::rename(&root, root.with_extension("old")).unwrap();
        symlink(&outside, &root).unwrap();
        assert!(
            bounded_read(&root.join("test/SKILL.md"), std::slice::from_ref(&admitted)).is_err()
        );
        // Simulate replacement after the initial canonical-path prefix check.
        assert!(crate::paths::open_regular_within(
            &admitted.boundary,
            &admitted.path.join("test/SKILL.md")
        )
        .is_err());
    }
    #[cfg(unix)]
    #[test]
    fn configured_project_root_and_ancestor_symlinks_cannot_authorize_foreign_trees() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&project).unwrap();
        put(&outside, "secret", "secret");
        std::fs::create_dir_all(project.join(".agents")).unwrap();
        symlink(&outside, project.join(".agents/skills")).unwrap();
        symlink(&outside, project.join(".claude")).unwrap();
        let mut context = fixture();
        context.cwd = Some(std::fs::canonicalize(&project).unwrap());
        let project_roots = roots(None, &project.join("portable"), &context);
        assert!(scan(&project_roots, &project.join("portable"))
            .skills
            .is_empty());
    }
    #[test]
    fn project_and_named_profile_roots_preserve_scope_and_provider_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let home = dir.path().join("home");
        let profile = dir.path().join("profile");
        put(&project.join(".pi/skills"), "local", "project");
        put(&profile.join("skills"), "private", "profile");
        put(&home.join(".codex/skills"), "default-only", "default");
        let mut context = fixture();
        context.cwd = Some(project.clone());
        context.profile_home = Some(profile);
        let portable = dir.path().join("portable");
        let catalog = scan(&roots(Some(&home), &portable, &context), &portable);
        assert!(catalog.skills.iter().any(|skill| skill.name == "local"
            && skill.sources[0].scope == "project"
            && skill.sources[0].origin == "pi"));
        assert!(catalog.skills.iter().any(|skill| skill.name == "private"));
        assert!(!catalog
            .skills
            .iter()
            .any(|skill| skill.name == "default-only"));
    }
    #[test]
    fn turn_limit_includes_headers_and_duplicate_invocations_resolve_once() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "test", "---\nname: test\n---\nINSTRUCTIONS");
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let catalog = scan(
            &[Root {
                path: root.clone(),
                boundary: root.clone(),
                origin: "codex".into(),
                scope: "user".into(),
            }],
            &root,
        );
        let once = instructions(&catalog, &[allowed(&root)], &fixture(), "/test request").unwrap();
        assert_eq!(
            once,
            instructions(
                &catalog,
                &[allowed(&root)],
                &fixture(),
                "/test /TEST request"
            )
            .unwrap()
        );
        assert_eq!(
            once,
            instructions(
                &catalog,
                &[allowed(&root)],
                &fixture(),
                "/unknown /test request"
            )
            .unwrap()
        );
        put(&root, "test", &"x".repeat(MAX_BYTES - 10));
        assert!(instructions(&catalog, &[allowed(&root)], &fixture(), "/test request").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_cycles_and_foreign_documents_cannot_escape_discovery() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skills");
        let outside = dir.path().join("outside");
        put(&root, "test", "safe");
        put(&outside, "secret", "secret");
        symlink(&root, root.join("cycle")).unwrap();
        symlink(&outside, root.join("foreign")).unwrap();
        let catalog = scan(
            &[Root {
                path: root.clone(),
                boundary: dir.path().into(),
                origin: "agents".into(),
                scope: "user".into(),
            }],
            &root,
        );
        assert_eq!(catalog.skills.len(), 1);
        assert_eq!(catalog.skills[0].name, "test");
    }
}
