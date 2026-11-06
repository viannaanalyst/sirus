//! Prompt links (ADR-122): `sirus://new?prompt=…&project=…|repo=owner/name&provider=…&model=…&pr=…`
//! open the new-thread landing of a project with the composer draft filled. A link never
//! sends anything: the person reads the draft and presses Send.
//!
//! A link comes from outside the app, so this module is the trust boundary: only the known
//! parameters, each at most once and bounded; a known provider id; a model that passes the
//! same check as every other model; and a prompt without the invisible characters that can
//! hide instructions from the person reading it. The UI learns how many were removed and
//! says so.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::runtime::State;
use reqwest::Url;
use serde::Serialize;

use crate::commands::{native_task, AppState};
use crate::error::{Error, Result};
use crate::models::{AgentProviderId, Project};

pub const SCHEME: &str = "sirus";
/// Tells the UI a link is waiting; the UI then takes it with `take_prompt_link`.
pub const EVENT: &str = "prompt-link";
/// Characters of the decoded prompt, before invisible characters are removed.
pub const MAX_PROMPT: usize = 8_000;
/// The whole link. Room for a prompt of four-byte characters, percent-encoded.
const MAX_URL: usize = 128 * 1024;
const MAX_PROJECT: usize = 1_024;
const MAX_PR: u32 = 999_999_999;
const PARAMS: &[&str] = &["prompt", "project", "repo", "provider", "model", "pr"];

/// A link as read, before it is matched to a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptLink {
    pub prompt: String,
    pub hidden_characters: usize,
    pub project: Option<String>,
    /// `owner/name` on GitHub.
    pub repo: Option<String>,
    pub provider: Option<AgentProviderId>,
    pub model: Option<String>,
    pub pr: Option<u32>,
}

/// Why a link was not opened. The UI words each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkError {
    Malformed,
    TooLong,
    UnknownParameter,
    RepeatedParameter,
    NoPrompt,
    HiddenOnly,
    Project,
    Repo,
    Provider,
    Model,
    PullRequest,
}

/// What the UI does with a link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PromptLinkOutcome {
    Open {
        /// The draft: the sanitized prompt, plus the pull request when the link named one.
        prompt: String,
        hidden_characters: usize,
        /// The project to open, or none when the link named one this Mac does not have.
        project_id: Option<String>,
        /// The project or repository the link asked for and no project matched.
        missing_project: Option<String>,
        provider: Option<AgentProviderId>,
        model: Option<String>,
    },
    Invalid {
        reason: LinkError,
    },
}

/// Removes characters a reader cannot see and counts them: C0/C1 controls other than tab
/// and newline, zero-width and bidi controls, the tag and variation-selector blocks, and
/// fillers that render as nothing. A zero-width joiner stays only inside an emoji
/// sequence. Line and paragraph separators become newlines, so the draft shows the break
/// the model reads. The result is trimmed.
pub fn sanitize(raw: &str) -> (String, usize) {
    let mut hidden = 0;
    let normalized = raw
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}', '\u{2029}'], "\n");
    let visible: Vec<char> = normalized
        .chars()
        .filter(|&character| {
            let keep = !is_hidden(character);
            if !keep {
                hidden += 1;
            }
            keep
        })
        .collect();
    let mut text = String::with_capacity(normalized.len());
    for (index, &character) in visible.iter().enumerate() {
        if character == '\u{200D}' {
            let before = index.checked_sub(1).and_then(|at| visible.get(at));
            let after = visible.get(index + 1);
            if !(before.is_some_and(|c| is_emoji_part(*c))
                && after.is_some_and(|c| is_emoji_part(*c)))
            {
                hidden += 1;
                continue;
            }
        }
        text.push(character);
    }
    (text.trim().to_string(), hidden)
}

fn is_hidden(character: char) -> bool {
    matches!(character as u32,
        0x0000..=0x0008
        | 0x000B..=0x001F
        | 0x007F..=0x009F
        | 0x00AD
        | 0x034F
        | 0x061C
        | 0x115F
        | 0x1160
        | 0x17B4
        | 0x17B5
        | 0x180B..=0x180F
        | 0x200B
        | 0x200C
        | 0x200E
        | 0x200F
        | 0x202A..=0x202E
        | 0x2060..=0x206F
        | 0x3164
        | 0xFE00..=0xFE0D
        | 0xFEFF
        | 0xFFA0
        | 0xFFF0..=0xFFFB
        | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A
        | 0xE0000..=0xE0FFF)
}

fn is_emoji_part(character: char) -> bool {
    matches!(character as u32, 0x1F000..=0x1FAFF | 0x2190..=0x2BFF | 0xFE0F)
}

/// Reads a `sirus://new?…` link.
pub fn parse(raw: &str) -> std::result::Result<PromptLink, LinkError> {
    if raw.len() > MAX_URL {
        return Err(LinkError::TooLong);
    }
    let url = Url::parse(raw).map_err(|_| LinkError::Malformed)?;
    if url.scheme() != SCHEME
        || url.host_str() != Some("new")
        || !matches!(url.path(), "" | "/")
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(LinkError::Malformed);
    }
    let mut values: [Option<String>; PARAMS.len()] = Default::default();
    for (key, value) in url.query_pairs() {
        let index = PARAMS
            .iter()
            .position(|known| *known == key)
            .ok_or(LinkError::UnknownParameter)?;
        if values[index].replace(value.into_owned()).is_some() {
            return Err(LinkError::RepeatedParameter);
        }
    }
    let [prompt, project, repo, provider, model, pr] = values;
    let optional = |value: Option<String>| {
        value
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };

    let raw_prompt = prompt.ok_or(LinkError::NoPrompt)?;
    if raw_prompt.chars().count() > MAX_PROMPT {
        return Err(LinkError::TooLong);
    }
    let (prompt, hidden_characters) = sanitize(&raw_prompt);
    if prompt.is_empty() {
        return Err(if hidden_characters > 0 {
            LinkError::HiddenOnly
        } else {
            LinkError::NoPrompt
        });
    }

    let project = optional(project);
    if project.as_ref().is_some_and(|value| {
        value.chars().count() > MAX_PROJECT || value.chars().any(|c| c.is_control() || is_hidden(c))
    }) {
        return Err(LinkError::Project);
    }
    let repo = match optional(repo) {
        Some(value) => Some(parse_repo(&value).ok_or(LinkError::Repo)?),
        None => None,
    };
    let provider = match optional(provider) {
        Some(value) => Some(provider_id(&value).ok_or(LinkError::Provider)?),
        None => None,
    };
    let model = optional(model);
    if let Some(model) = &model {
        if provider.is_none() || !crate::launch::valid_model(model) {
            return Err(LinkError::Model);
        }
    }
    let pr = match optional(pr) {
        Some(value) => Some(parse_pr(&value).ok_or(LinkError::PullRequest)?),
        None => None,
    };
    Ok(PromptLink {
        prompt,
        hidden_characters,
        project,
        repo,
        provider,
        model,
        pr,
    })
}

fn provider_id(value: &str) -> Option<AgentProviderId> {
    serde_json::from_value(serde_json::Value::String(value.to_ascii_lowercase())).ok()
}

fn parse_pr(value: &str) -> Option<u32> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|n| (1..=MAX_PR).contains(n))
}

/// `owner/name` with GitHub's character rules; a trailing `.git` is dropped.
pub fn parse_repo(value: &str) -> Option<String> {
    let (owner, name) = value.split_once('/')?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    let owner_ok = (1..=39).contains(&owner.len())
        && !owner.starts_with('-')
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let name_ok = (1..=100).contains(&name.len())
        && !name.bytes().all(|b| b == b'.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    (owner_ok && name_ok).then(|| format!("{owner}/{name}"))
}

/// `owner/name` of a GitHub web URL such as `git::remote_web_url` returns.
fn repo_of_web_url(url: &str) -> Option<String> {
    parse_repo(url.strip_prefix("https://github.com/")?)
}

/// Matches a link to a project: `project` by id, then name (case-insensitive), then path;
/// otherwise `repo` against each project's GitHub remote. Among several matches the most
/// recently opened project wins. `remote_of` returns a project's GitHub web URL.
pub fn resolve(
    link: PromptLink,
    projects: &[Project],
    mut remote_of: impl FnMut(&Project) -> Option<String>,
) -> PromptLinkOutcome {
    let mut ordered: Vec<&Project> = projects.iter().collect();
    ordered.sort_by(|a, b| b.last_opened_at.cmp(&a.last_opened_at));
    let by_project = link.project.as_deref().and_then(|wanted| {
        let lower = wanted.to_lowercase();
        let trimmed = trim_path(wanted);
        ordered
            .iter()
            .find(|project| project.id == wanted)
            .or_else(|| {
                ordered
                    .iter()
                    .find(|project| project.name.to_lowercase() == lower)
            })
            .or_else(|| {
                ordered
                    .iter()
                    .find(|project| trim_path(&project.path) == trimmed)
            })
            .copied()
    });
    let mut matched_remote = None;
    let by_repo = || {
        let wanted = link.repo.as_deref()?.to_ascii_lowercase();
        ordered.iter().copied().find(|project| {
            let repo = remote_of(project).and_then(|url| repo_of_web_url(&url));
            let hit = repo.as_deref().map(str::to_ascii_lowercase) == Some(wanted.clone());
            if hit {
                matched_remote = repo;
            }
            hit
        })
    };
    let project = by_project.or_else(by_repo);
    let missing_project = match project {
        Some(_) => None,
        None => link.project.clone().or_else(|| link.repo.clone()),
    };

    let mut prompt = link.prompt;
    if let Some(number) = link.pr {
        let repo = link.repo.clone().or(matched_remote).or_else(|| {
            project
                .and_then(&mut remote_of)
                .and_then(|url| repo_of_web_url(&url))
        });
        let reference = match repo {
            Some(repo) => format!("https://github.com/{repo}/pull/{number}"),
            None => format!("#{number}"),
        };
        prompt.push_str(&format!("\n\nPull request: {reference}"));
    }
    PromptLinkOutcome::Open {
        prompt,
        hidden_characters: link.hidden_characters,
        project_id: project.map(|project| project.id.clone()),
        missing_project,
        provider: link.provider,
        model: link.model,
    }
}

fn trim_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        path
    } else {
        trimmed
    }
}

/// What a link carries when Sirus builds one.
pub struct LinkDraft<'a> {
    pub prompt: &'a str,
    pub project: Option<&'a str>,
    pub repo: Option<&'a str>,
    pub provider: Option<&'a AgentProviderId>,
    pub model: Option<&'a str>,
}

/// A link that `parse` reads back to the same draft. `repo` wins over `project`.
pub fn build(draft: &LinkDraft) -> std::result::Result<String, LinkError> {
    let prompt = draft.prompt.trim();
    if prompt.is_empty() {
        return Err(LinkError::NoPrompt);
    }
    if prompt.chars().count() > MAX_PROMPT {
        return Err(LinkError::TooLong);
    }
    let mut url = Url::parse(&format!("{SCHEME}://new")).map_err(|_| LinkError::Malformed)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("prompt", prompt);
        if let Some(repo) = draft.repo {
            query.append_pair("repo", &parse_repo(repo).ok_or(LinkError::Repo)?);
        } else if let Some(project) = draft.project.map(str::trim).filter(|p| !p.is_empty()) {
            query.append_pair("project", project);
        }
        if let Some(provider) = draft.provider {
            query.append_pair("provider", provider.key());
            if let Some(model) = draft.model {
                if !crate::launch::valid_model(model) {
                    return Err(LinkError::Model);
                }
                query.append_pair("model", model);
            }
        }
    }
    Ok(url.into())
}

// ---- Delivery: the OS hands a link over; the UI takes it when it is ready. ----

/// The latest link not yet taken. A newer link replaces an older one.
static PENDING: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);
/// The last link received, so the same launch URL reported twice (at start and by the
/// open event) opens once.
static LAST: parking_lot::Mutex<Option<(String, Instant)>> = parking_lot::Mutex::new(None);
const DUPLICATE_WINDOW: Duration = Duration::from_secs(2);

/// A link read and matched to the projects of `state`.
fn outcome_of(state: &AppState, raw: &str) -> PromptLinkOutcome {
    let link = match parse(raw) {
        Ok(link) => link,
        Err(reason) => return PromptLinkOutcome::Invalid { reason },
    };
    let projects = state.data.lock().projects.clone();
    let remote = |project: &Project| {
        crate::git::remote_web_url(Path::new(&project.path))
            .ok()
            .flatten()
    };
    resolve(link, &projects, remote)
}

/// The waiting link, read and matched to a project; none when nothing is waiting.
pub async fn take_prompt_link(
    state: State<'_, Arc<AppState>>,
) -> Result<Option<PromptLinkOutcome>> {
    let Some(raw) = PENDING.lock().take() else {
        return Ok(None);
    };
    let state = state.inner().clone();
    native_task(move || Ok(Some(outcome_of(&state, &raw)))).await
}

/// A link the caller received itself, read and matched to a project exactly as a waiting one is.
/// Sirus Native hears `sirus://` links from macOS in its own process and has no pending slot to
/// take from. Nothing is stored and nothing is opened: the answer is the checked outcome.
pub async fn resolve_prompt_link(
    state: State<'_, Arc<AppState>>,
    url: String,
) -> Result<PromptLinkOutcome> {
    let state = state.inner().clone();
    native_task(move || Ok(outcome_of(&state, &url))).await
}

/// A link to the given draft: the project's GitHub repository when it has one, otherwise
/// the project's name.
pub async fn build_prompt_link(
    state: State<'_, Arc<AppState>>,
    project_id: Option<String>,
    prompt: String,
    provider: Option<AgentProviderId>,
    model: Option<String>,
) -> Result<String> {
    let state = state.inner().clone();
    native_task(move || {
        let project = match project_id {
            Some(id) => Some(
                state
                    .data
                    .lock()
                    .projects
                    .iter()
                    .find(|project| project.id == id)
                    .cloned()
                    .ok_or_else(|| Error::not_found("project not found"))?,
            ),
            None => None,
        };
        let repo = project
            .as_ref()
            .and_then(|project| {
                crate::git::remote_web_url(Path::new(&project.path))
                    .ok()
                    .flatten()
            })
            .and_then(|url| repo_of_web_url(&url));
        build(&LinkDraft {
            prompt: &prompt,
            project: project.as_ref().map(|project| project.name.as_str()),
            repo: repo.as_deref(),
            provider: provider.as_ref(),
            model: model.as_deref(),
        })
        .map_err(|reason| {
            Error::new(
                "promptLink",
                match reason {
                    LinkError::NoPrompt => "The draft is empty.",
                    LinkError::TooLong => "The draft is too long for a prompt link.",
                    _ => "This draft cannot be shared as a prompt link.",
                },
            )
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: &str, name: &str, path: &str, opened: &str) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            path: path.into(),
            added_at: opened.into(),
            last_opened_at: opened.into(),
            look: Default::default(),
            appearance: None,
            scripts: Default::default(),
        }
    }

    fn link(query: &str) -> std::result::Result<PromptLink, LinkError> {
        parse(&format!("sirus://new?{query}"))
    }

    #[test]
    fn reads_every_parameter() {
        let parsed = link(
            "prompt=Fix%20the%20build&repo=acme/app.git&provider=Codex&model=gpt-5.5&pr=%2342",
        )
        .unwrap();
        assert_eq!(parsed.prompt, "Fix the build");
        assert_eq!(parsed.hidden_characters, 0);
        assert_eq!(parsed.repo.as_deref(), Some("acme/app"));
        assert_eq!(parsed.provider, Some(AgentProviderId::Codex));
        assert_eq!(parsed.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(parsed.pr, Some(42));
        assert_eq!(link("prompt=a+b").unwrap().prompt, "a b");
        assert_eq!(parse("sirus://new/?prompt=x").unwrap().prompt, "x");
    }

    #[test]
    fn rejects_anything_unexpected() {
        assert_eq!(parse("https://new?prompt=x"), Err(LinkError::Malformed));
        assert_eq!(
            parse("sirus://settings?prompt=x"),
            Err(LinkError::Malformed)
        );
        assert_eq!(
            parse("sirus://new/other?prompt=x"),
            Err(LinkError::Malformed)
        );
        assert_eq!(
            parse("sirus://new?prompt=x#frag"),
            Err(LinkError::Malformed)
        );
        assert_eq!(
            parse("sirus://user@new?prompt=x"),
            Err(LinkError::Malformed)
        );
        assert_eq!(link("prompt=x&mode=yolo"), Err(LinkError::UnknownParameter));
        assert_eq!(link("prompt=x&prompt=y"), Err(LinkError::RepeatedParameter));
        assert_eq!(link("repo=a/b"), Err(LinkError::NoPrompt));
        assert_eq!(link("prompt=%20%20"), Err(LinkError::NoPrompt));
        assert_eq!(
            link("prompt=%E2%80%8B%E2%80%8B"),
            Err(LinkError::HiddenOnly)
        );
        assert_eq!(link("prompt=x&provider=gpt"), Err(LinkError::Provider));
        assert_eq!(link("prompt=x&model=gpt-5"), Err(LinkError::Model));
        assert_eq!(
            link("prompt=x&provider=codex&model=-rf"),
            Err(LinkError::Model)
        );
        assert_eq!(
            link("prompt=x&provider=codex&model=a%20b"),
            Err(LinkError::Model)
        );
        assert_eq!(link("prompt=x&repo=a"), Err(LinkError::Repo));
        assert_eq!(link("prompt=x&repo=-a/b"), Err(LinkError::Repo));
        assert_eq!(link("prompt=x&repo=a/.."), Err(LinkError::Repo));
        assert_eq!(link("prompt=x&repo=a/b/c"), Err(LinkError::Repo));
        assert_eq!(link("prompt=x&pr=0"), Err(LinkError::PullRequest));
        assert_eq!(link("prompt=x&pr=12a"), Err(LinkError::PullRequest));
        assert_eq!(link("prompt=x&pr=1234567890"), Err(LinkError::PullRequest));
        assert_eq!(link("prompt=x&project=a%0Ab"), Err(LinkError::Project));
        assert_eq!(
            link("prompt=x&project=a%E2%80%AEb"),
            Err(LinkError::Project)
        );
    }

    #[test]
    fn bounds_lengths() {
        let long = "a".repeat(MAX_PROMPT + 1);
        assert_eq!(link(&format!("prompt={long}")), Err(LinkError::TooLong));
        assert!(link(&format!("prompt={}", "a".repeat(MAX_PROMPT))).is_ok());
        let huge = format!("sirus://new?prompt={}", "a".repeat(MAX_URL));
        assert_eq!(parse(&huge), Err(LinkError::TooLong));
        let project = "p".repeat(MAX_PROJECT + 1);
        assert_eq!(
            link(&format!("prompt=x&project={project}")),
            Err(LinkError::Project)
        );
    }

    #[test]
    fn removes_hidden_characters_and_counts_them() {
        assert_eq!(sanitize("he\u{200B}llo\u{202E}"), ("hello".into(), 2));
        assert_eq!(sanitize("a\u{E0041}\u{E0042}b"), ("ab".into(), 2));
        assert_eq!(sanitize("\u{FEFF}x\u{00AD}y\u{2066}"), ("xy".into(), 3));
        assert_eq!(sanitize("a\u{0007}b\tc"), ("ab\tc".into(), 1));
        assert_eq!(
            sanitize("one\r\ntwo\u{2028}three"),
            ("one\ntwo\nthree".into(), 0)
        );
        // A joiner inside an emoji sequence is visible work; elsewhere it hides.
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(sanitize(family), (family.into(), 0));
        assert_eq!(sanitize("a\u{200D}b"), ("ab".into(), 1));
        // Emoji presentation selectors stay.
        assert_eq!(sanitize("\u{2764}\u{FE0F}"), ("\u{2764}\u{FE0F}".into(), 0));
        assert_eq!(
            sanitize("caf\u{00E9} \u{65E5}\u{672C}"),
            ("caf\u{00E9} \u{65E5}\u{672C}".into(), 0)
        );
        let parsed = link("prompt=Ignore%E2%80%8Bme").unwrap();
        assert_eq!(
            (parsed.prompt.as_str(), parsed.hidden_characters),
            ("Ignoreme", 1)
        );
    }

    #[test]
    fn matches_projects_by_id_name_path_and_repo() {
        let projects = vec![
            project("p1", "App", "/code/app", "2026-01-01"),
            project("p2", "app", "/code/app-fork", "2026-02-01"),
            project("p3", "Site", "/code/site/", "2026-01-15"),
        ];
        let remote = |project: &Project| match project.id.as_str() {
            "p1" => Some("https://github.com/Acme/App".to_string()),
            "p3" => Some("https://github.com/acme/site".to_string()),
            _ => None,
        };
        let open = |query: &str| match resolve(link(query).unwrap(), &projects, remote) {
            PromptLinkOutcome::Open {
                project_id,
                missing_project,
                prompt,
                ..
            } => (project_id, missing_project, prompt),
            other => panic!("{other:?}"),
        };
        assert_eq!(open("prompt=x&project=p1").0.as_deref(), Some("p1"));
        // Same name twice: the most recently opened.
        assert_eq!(open("prompt=x&project=APP").0.as_deref(), Some("p2"));
        assert_eq!(open("prompt=x&project=/code/site").0.as_deref(), Some("p3"));
        assert_eq!(open("prompt=x&repo=acme/app").0.as_deref(), Some("p1"));
        let (id, missing, _) = open("prompt=x&repo=acme/other");
        assert_eq!((id, missing.as_deref()), (None, Some("acme/other")));
        let (id, missing, _) = open("prompt=x&project=Nope");
        assert_eq!((id, missing.as_deref()), (None, Some("Nope")));
        // An unknown project falls back to the repository.
        assert_eq!(
            open("prompt=x&project=Nope&repo=acme/site").0.as_deref(),
            Some("p3")
        );
        let (id, missing, _) = open("prompt=x");
        assert_eq!((id, missing), (None, None));
        // The pull request is named in the draft, with its repository when known.
        assert_eq!(
            open("prompt=x&repo=acme/app&pr=7").2,
            "x\n\nPull request: https://github.com/acme/app/pull/7"
        );
        assert_eq!(
            open("prompt=x&project=p3&pr=7").2,
            "x\n\nPull request: https://github.com/acme/site/pull/7"
        );
        assert_eq!(open("prompt=x&project=p2&pr=7").2, "x\n\nPull request: #7");
    }

    #[test]
    fn built_links_read_back() {
        let tricky = "Fix the build & run tests?\n\n100% + = #1 /ok — ünïcode \u{1F600}";
        for (project, repo, provider, model) in [
            (None, None, None, None),
            (
                Some("My Project"),
                None,
                Some(AgentProviderId::Claude),
                Some("claude-opus-4-5"),
            ),
            (
                Some("ignored"),
                Some("acme/app"),
                Some(AgentProviderId::OpenCode),
                None,
            ),
            (
                None,
                Some("acme/app"),
                Some(AgentProviderId::Codex),
                Some("gpt-5.5"),
            ),
        ] {
            let url = build(&LinkDraft {
                prompt: tricky,
                project,
                repo,
                provider: provider.as_ref(),
                model,
            })
            .unwrap();
            assert!(url.starts_with("sirus://new?prompt="), "{url}");
            let parsed = parse(&url).unwrap();
            assert_eq!(parsed.prompt, tricky);
            assert_eq!(parsed.hidden_characters, 0);
            assert_eq!(parsed.repo.as_deref(), repo);
            assert_eq!(
                parsed.project.as_deref(),
                if repo.is_some() { None } else { project }
            );
            assert_eq!(parsed.provider, provider);
            assert_eq!(parsed.model.as_deref(), model);
        }
        let empty = LinkDraft {
            prompt: "  ",
            project: None,
            repo: None,
            provider: None,
            model: None,
        };
        assert_eq!(build(&empty), Err(LinkError::NoPrompt));
        let long = "a".repeat(MAX_PROMPT + 1);
        let too_long = LinkDraft {
            prompt: &long,
            project: None,
            repo: None,
            provider: None,
            model: None,
        };
        assert_eq!(build(&too_long), Err(LinkError::TooLong));
    }

    #[test]
    fn provider_ids_follow_the_model_keys() {
        for id in [
            AgentProviderId::Codex,
            AgentProviderId::Claude,
            AgentProviderId::OpenCode,
            AgentProviderId::Cursor,
            AgentProviderId::Grok,
            AgentProviderId::Antigravity,
            AgentProviderId::Droid,
            AgentProviderId::Pi,
            AgentProviderId::Devin,
            AgentProviderId::Hermes,
        ] {
            assert_eq!(provider_id(id.key()), Some(id));
        }
    }
}
