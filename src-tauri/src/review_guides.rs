//! Guided reviews (ADR-125): an ordered tour of a pull request
//! or of a session's changes. Each step has a title, a short Markdown explanation, an optional
//! call the reviewer should make, and anchors to file line ranges in the diff.
//!
//! Agents write and read guides with the always-on `sirus_write_review_guide` and
//! `sirus_read_review_guide` tools; they only touch guides, never code. The person reads them in
//! the pull request's Guide tab and in the Changes pane. Guides are kept one row each in
//! `state.db` (`review_guides`), one per target, at most `MAX_GUIDES` (the oldest go first).

use std::sync::Arc;

use crate::runtime::{AppHandle, Emitter, Manager, State};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::commands::{native_task, AppState};
use crate::error::{Error, Result};
use crate::models::AppData;

pub const WRITE_TOOL: &str = "sirus_write_review_guide";
pub const READ_TOOL: &str = "sirus_read_review_guide";
pub const TOOLS: [&str; 2] = [WRITE_TOOL, READ_TOOL];
/// Emitted with `{ key }` when a guide is written or deleted.
pub const CHANGED: &str = "review-guides-changed";

pub const MAX_GUIDES: usize = 40;
pub const MAX_STEPS: usize = 24;
pub const MAX_ANCHORS: usize = 8;
pub const MAX_TITLE_CHARS: usize = 120;
pub const MAX_STEP_TITLE_CHARS: usize = 160;
pub const MAX_BODY_CHARS: usize = 4000;
pub const MAX_ATTENTION_CHARS: usize = 400;
pub const MAX_CAPTION_CHARS: usize = 160;
pub const MAX_PATH_CHARS: usize = 512;
/// Lines one anchor may span: a step points at what carries its idea, not a whole file.
pub const MAX_ANCHOR_LINES: u32 = 400;

/// What a guide is about: a GitHub pull request, or a session's uncommitted changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum GuideTarget {
    PullRequest { repository: String, number: u64 },
    Session { session_id: String },
}

impl GuideTarget {
    /// One guide per key; repositories compare without case, as GitHub does.
    pub fn key(&self) -> String {
        match self {
            Self::PullRequest { repository, number } => {
                format!("pr:{}#{number}", repository.to_lowercase())
            }
            Self::Session { session_id } => format!("session:{session_id}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnchorSide {
    /// Added or kept lines, numbered as in the changed file.
    #[default]
    New,
    /// Removed lines, numbered as in the file before the change.
    Old,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideAnchor {
    pub path: String,
    #[serde(default)]
    pub side: AnchorSide,
    pub start: u32,
    pub end: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub caption: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideStep {
    pub title: String,
    /// Markdown.
    #[serde(default)]
    pub body: String,
    /// The one call the reviewer should make at this step, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<String>,
    #[serde(default)]
    pub anchors: Vec<GuideAnchor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewGuide {
    pub id: String,
    pub target: GuideTarget,
    pub title: String,
    /// Markdown.
    #[serde(default)]
    pub summary: String,
    pub steps: Vec<GuideStep>,
    /// The head commit the guide was written against; another head means it may be stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_session_id: Option<String>,
    #[serde(default)]
    pub revision: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// What an agent sends; unknown fields are ignored so a near miss still lands.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuideInput {
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    head_sha: Option<String>,
    steps: Vec<StepInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepInput {
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    attention: Option<String>,
    #[serde(default)]
    anchors: Vec<AnchorInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnchorInput {
    path: String,
    #[serde(default)]
    side: Option<String>,
    start: i64,
    #[serde(default)]
    end: Option<i64>,
    #[serde(default)]
    caption: String,
}

/// Every whitespace run (including line breaks and control characters) becomes one space.
fn one_line(text: &str) -> String {
    text.split(|c: char| c.is_control() || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn limited(what: &str, text: String, max: usize) -> std::result::Result<String, String> {
    if text.chars().count() > max {
        return Err(format!("{what} must be at most {max} characters"));
    }
    Ok(text)
}

/// Markdown keeps its line breaks; other control characters and NUL go.
fn markdown(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A repository-relative path, as `git diff` names it.
fn clean_path(path: &str) -> std::result::Result<String, String> {
    let mut path = path.trim();
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    if path.is_empty() {
        return Err("each anchor needs a path".into());
    }
    if path.chars().count() > MAX_PATH_CHARS {
        return Err(format!(
            "anchor paths must be at most {MAX_PATH_CHARS} characters"
        ));
    }
    if path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| part.is_empty() || part == "..")
    {
        return Err(format!(
            "{path:?} is not a repository-relative path like src/app.ts"
        ));
    }
    Ok(path.to_owned())
}

fn clean_anchor(input: AnchorInput) -> std::result::Result<GuideAnchor, String> {
    let path = clean_path(&input.path)?;
    let side = match input.side.as_deref().map(str::trim) {
        None | Some("") | Some("new") => AnchorSide::New,
        Some("old") => AnchorSide::Old,
        Some(_) => return Err("an anchor's side is \"new\" or \"old\"".into()),
    };
    let end = input.end.unwrap_or(input.start);
    let (start, end) = (input.start.min(end), input.start.max(end));
    if start < 1 || end > i64::from(u32::MAX) {
        return Err(format!("{path}: line numbers start at 1"));
    }
    let (start, end) = (start as u32, end as u32);
    if end - start + 1 > MAX_ANCHOR_LINES {
        return Err(format!(
            "{path}: an anchor spans at most {MAX_ANCHOR_LINES} lines; point at the lines that carry the idea"
        ));
    }
    Ok(GuideAnchor {
        path,
        side,
        start,
        end,
        caption: limited(
            "an anchor caption",
            one_line(&input.caption),
            MAX_CAPTION_CHARS,
        )?,
    })
}

fn clean_head(head: Option<String>) -> std::result::Result<Option<String>, String> {
    let Some(head) = head.map(|head| head.trim().to_lowercase()) else {
        return Ok(None);
    };
    if head.is_empty() {
        return Ok(None);
    }
    if !(7..=64).contains(&head.len()) || !head.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("headSha must be a commit hash (7 to 64 hex characters)".into());
    }
    Ok(Some(head))
}

/// The guide's content, checked and trimmed: everything an agent sent, or why it was refused.
#[derive(Debug)]
pub struct Content {
    pub title: String,
    pub summary: String,
    pub head_sha: Option<String>,
    pub steps: Vec<GuideStep>,
}

pub fn validate(args: &Value) -> std::result::Result<Content, String> {
    let input: GuideInput = serde_json::from_value(args.clone()).map_err(|error| {
        format!("the guide needs a title and a steps array of {{title, body, anchors}}: {error}")
    })?;
    let title = limited("the title", one_line(&input.title), MAX_TITLE_CHARS)?;
    if title.is_empty() {
        return Err("a title is required".into());
    }
    let summary = limited("the summary", markdown(&input.summary), MAX_BODY_CHARS)?;
    if input.steps.is_empty() {
        return Err("a guide needs at least one step".into());
    }
    if input.steps.len() > MAX_STEPS {
        return Err(format!(
            "a guide has at most {MAX_STEPS} steps; group the mechanical rest into one closing step"
        ));
    }
    let mut steps = Vec::with_capacity(input.steps.len());
    for (index, step) in input.steps.into_iter().enumerate() {
        let number = index + 1;
        let title = limited(
            &format!("step {number}'s title"),
            one_line(&step.title),
            MAX_STEP_TITLE_CHARS,
        )?;
        if title.is_empty() {
            return Err(format!("step {number} needs a title"));
        }
        if step.anchors.len() > MAX_ANCHORS {
            return Err(format!("step {number} has more than {MAX_ANCHORS} anchors"));
        }
        let attention = limited(
            &format!("step {number}'s attention"),
            one_line(step.attention.as_deref().unwrap_or_default()),
            MAX_ATTENTION_CHARS,
        )?;
        let anchors = step
            .anchors
            .into_iter()
            .map(clean_anchor)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| format!("step {number}: {error}"))?;
        steps.push(GuideStep {
            title,
            body: limited(
                &format!("step {number}'s body"),
                markdown(&step.body),
                MAX_BODY_CHARS,
            )?,
            attention: (!attention.is_empty()).then_some(attention),
            anchors,
        });
    }
    Ok(Content {
        title,
        summary,
        head_sha: clean_head(input.head_sha)?,
        steps,
    })
}

/// `owner/repo#12` or a GitHub pull request link.
pub fn parse_pull_request(text: &str) -> Option<GuideTarget> {
    let text = text.trim();
    let (repository, number) = if let Some(rest) = text
        .strip_prefix("https://github.com/")
        .or_else(|| text.strip_prefix("http://github.com/"))
    {
        let mut parts = rest.split('/');
        let (owner, name, kind, number) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        if kind != "pull" {
            return None;
        }
        let number = number.split(['?', '#']).next()?;
        (format!("{owner}/{name}"), number)
    } else {
        let (repository, number) = text.rsplit_once('#')?;
        (repository.to_owned(), number)
    };
    let (owner, name) = repository.split_once('/')?;
    let valid = |part: &str, extra: &[char]| {
        !part.is_empty()
            && part.len() <= 100
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || extra.contains(&c))
    };
    if !valid(owner, &[]) || !valid(name, &['.', '_']) || name.contains('/') {
        return None;
    }
    let number: u64 = number.parse().ok().filter(|n| *n > 0 && *n < 100_000_000)?;
    Some(GuideTarget::PullRequest { repository, number })
}

pub fn find<'a>(data: &'a AppData, target: &GuideTarget) -> Option<&'a ReviewGuide> {
    let key = target.key();
    data.review_guides
        .iter()
        .find(|guide| guide.target.key() == key)
}

/// Writes the guide for its target, replacing an earlier one in place. Returns it and
/// whether it replaced one.
pub fn write(
    data: &mut AppData,
    target: GuideTarget,
    content: Content,
    author: Option<String>,
    now: &str,
) -> (ReviewGuide, bool) {
    let key = target.key();
    if let Some(guide) = data
        .review_guides
        .iter_mut()
        .find(|guide| guide.target.key() == key)
    {
        guide.title = content.title;
        guide.summary = content.summary;
        guide.steps = content.steps;
        guide.head_sha = content.head_sha;
        guide.author_session_id = author;
        guide.revision = guide.revision.saturating_add(1);
        guide.updated_at = now.to_owned();
        return (guide.clone(), true);
    }
    let guide = ReviewGuide {
        id: uuid::Uuid::new_v4().to_string(),
        target,
        title: content.title,
        summary: content.summary,
        steps: content.steps,
        head_sha: content.head_sha,
        author_session_id: author,
        revision: 0,
        created_at: now.to_owned(),
        updated_at: now.to_owned(),
    };
    data.review_guides.push(guide.clone());
    prune(data);
    (guide, false)
}

/// Guides of sessions that are gone leave; past the bound, the least recently written go.
pub fn prune(data: &mut AppData) {
    let sessions: std::collections::HashSet<&str> = data
        .sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect();
    data.review_guides.retain(|guide| match &guide.target {
        GuideTarget::Session { session_id } => sessions.contains(session_id.as_str()),
        GuideTarget::PullRequest { .. } => true,
    });
    while data.review_guides.len() > MAX_GUIDES {
        let Some(oldest) = data
            .review_guides
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.updated_at.cmp(&b.updated_at))
            .map(|(index, _)| index)
        else {
            break;
        };
        data.review_guides.remove(oldest);
    }
}

pub fn tool_definitions() -> Vec<Value> {
    let anchor = json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Repository-relative path as git diff names it, e.g. src/app.ts." },
            "start": { "type": "integer", "description": "First line (1-based) on the given side." },
            "end": { "type": "integer", "description": "Last line; defaults to start. At most 400 lines per anchor." },
            "side": { "type": "string", "enum": ["new", "old"], "description": "new (default): added or kept lines, numbered as in the changed file. old: removed lines, numbered as before the change." },
            "caption": { "type": "string", "description": "Optional one-line label." }
        },
        "required": ["path", "start"]
    });
    vec![
        json!({
            "name": WRITE_TOOL,
            "description": "Sirus Code: write a guided review, a step-by-step tour the person follows through a change. Give pullRequest (owner/repo#123 or its GitHub link) for a pull request; without it the guide covers your own session's uncommitted changes (git diff HEAD in your worktree). Writing again replaces the guide for the same target. Read the diff first. Order steps so each builds on the ones before (data shape or contract first, then the code implementing it, then its callers and UI); 1 step for a trivial change, usually 2 to 6, at most 24; group mechanical changes (renames, snapshots, lockfiles) into a closing step. Each step: a title saying what it accomplishes (not a file name), a body of 2 to 4 sentences of Markdown explaining before vs after and why, an optional attention naming the one call the reviewer should judge there (a trade-off, a compatibility promise, a risk accepted; not a bug or nit; most steps have none), and anchors pointing at the lines worth reading. Anchor lines must be inside the diff's hunks: new-side numbers for added code (the + side of the hunk headers), side old for removed code. It only stores the guide; it never changes code. Tell the person in one line that the guide is ready; do not repeat it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pullRequest": { "type": "string", "description": "owner/repo#123 or https://github.com/owner/repo/pull/123. Omit for your session's changes." },
                    "title": { "type": "string", "description": "What the change does, at most 120 characters." },
                    "summary": { "type": "string", "description": "Optional Markdown overview, at most 4,000 characters." },
                    "headSha": { "type": "string", "description": "Optional head commit the guide was written against (gh pr view --json headRefOid), so the app can tell when it is stale." },
                    "steps": {
                        "type": "array",
                        "description": "The tour, in reading order.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string" },
                                "body": { "type": "string", "description": "Markdown, at most 4,000 characters." },
                                "attention": { "type": "string", "description": "Optional: the one call to make here." },
                                "anchors": { "type": "array", "items": anchor, "description": "0 to 8 line ranges." }
                            },
                            "required": ["title", "body"]
                        }
                    }
                },
                "required": ["title", "steps"]
            }
        }),
        json!({
            "name": READ_TOOL,
            "description": "Sirus Code: read a guided review: the one for pullRequest (owner/repo#123 or its link), or for a session's changes (sessionId, default your own). Returns the guide's steps and anchors, or guide null when none was written.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pullRequest": { "type": "string" },
                    "sessionId": { "type": "string" }
                }
            }
        }),
    ]
}

fn target_from(args: &Value, caller: &str) -> std::result::Result<GuideTarget, String> {
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    if let Some(reference) = text("pullRequest") {
        return parse_pull_request(reference).ok_or_else(|| {
            format!("{reference:?} is not a pull request; use owner/repo#123 or its GitHub link")
        });
    }
    Ok(GuideTarget::Session {
        session_id: text("sessionId").unwrap_or(caller).to_owned(),
    })
}

pub fn execute(
    app: &AppHandle,
    session_id: &str,
    tool: &str,
    args: &Value,
) -> std::result::Result<Value, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if tool == READ_TOOL {
        let target = target_from(args, session_id)?;
        let data = state.data.lock();
        if let GuideTarget::Session { session_id } = &target {
            if !data
                .sessions
                .iter()
                .any(|session| session.id == *session_id)
            {
                return Err("session not found".into());
            }
        }
        return Ok(match find(&data, &target) {
            Some(guide) => json!({ "guide": guide }),
            None => {
                json!({ "guide": null, "hint": format!("No guide yet; write one with {WRITE_TOOL}.") })
            }
        });
    }
    // Writing is for a pull request or the caller's own changes, never another session's.
    let target = match target_from(args, session_id)? {
        GuideTarget::Session { .. } => GuideTarget::Session {
            session_id: session_id.to_owned(),
        },
        pull => pull,
    };
    let content = validate(args)?;
    let now = chrono::Local::now().to_rfc3339();
    let (guide, replaced) = {
        let mut data = state.data.lock();
        if !data.sessions.iter().any(|session| session.id == session_id) {
            return Err("session not found".into());
        }
        write(
            &mut data,
            target.clone(),
            content,
            Some(session_id.to_owned()),
            &now,
        )
    };
    state.persist().map_err(|error| error.to_string())?;
    let _ = app.emit(CHANGED, json!({ "key": target.key() }));
    let place = match target {
        GuideTarget::PullRequest { .. } => "the pull request's Guide tab on the Pull requests page",
        GuideTarget::Session { .. } => "Guided review in this session's Changes panel",
    };
    Ok(json!({
        "saved": true,
        "guideId": guide.id,
        "steps": guide.steps.len(),
        "updated": replaced,
        "hint": format!("The person opens it from {place}. Tell them it is ready in one line."),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    Get { target: GuideTarget },
    Delete { target: GuideTarget },
}

pub async fn review_guide_action(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    action: Action,
) -> Result<Option<ReviewGuide>> {
    let state = state.inner().clone();
    native_task(move || match action {
        Action::Get { target } => Ok(find(&state.data.lock(), &target).cloned()),
        Action::Delete { target } => {
            let key = target.key();
            {
                let mut data = state.data.lock();
                state.ensure_running()?;
                let before = data.review_guides.len();
                data.review_guides.retain(|guide| guide.target.key() != key);
                if data.review_guides.len() == before {
                    return Err(Error::not_found("guide not found"));
                }
            }
            state.persist()?;
            let _ = app.emit(CHANGED, json!({ "key": key }));
            Ok(None)
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guide(steps: Value) -> Value {
        json!({ "title": "Move the picker", "summary": "  Why  ", "steps": steps })
    }

    fn step() -> Value {
        json!({ "title": "The shape", "body": "Line one\r\nLine two\u{7}", "anchors": [{ "path": "./src/a.ts", "start": 12, "end": 4, "caption": "the\ntype" }] })
    }

    #[test]
    fn content_is_trimmed_and_anchors_normalized() {
        let content = validate(&guide(json!([step()]))).unwrap();
        assert_eq!(content.title, "Move the picker");
        assert_eq!(content.summary, "Why");
        let step = &content.steps[0];
        assert_eq!(step.body, "Line one\nLine two");
        assert_eq!(step.attention, None);
        assert_eq!(
            step.anchors[0],
            GuideAnchor {
                path: "src/a.ts".into(),
                side: AnchorSide::New,
                start: 4,
                end: 12,
                caption: "the type".into()
            }
        );
        let old = validate(&guide(json!([{ "title": "t", "body": "", "attention": " judge\nthis ", "anchors": [{ "path": "a", "start": 3, "side": "old" }] }]))).unwrap();
        assert_eq!(old.steps[0].anchors[0].side, AnchorSide::Old);
        assert_eq!(old.steps[0].anchors[0].end, 3);
        assert_eq!(old.steps[0].attention.as_deref(), Some("judge this"));
    }

    #[test]
    fn bad_guides_are_refused_with_a_reason() {
        let refused = |value: Value| validate(&value).unwrap_err();
        assert!(refused(json!({ "title": "x" })).contains("steps"));
        assert!(refused(guide(json!([]))).contains("at least one"));
        assert!(refused(json!({ "title": " ", "steps": [step()] })).contains("title"));
        assert!(refused(guide(json!(vec![step(); MAX_STEPS + 1]))).contains(&MAX_STEPS.to_string()));
        assert!(refused(guide(json!([{ "title": "", "body": "" }]))).contains("step 1"));
        let long = "x".repeat(MAX_BODY_CHARS + 1);
        assert!(refused(guide(json!([{ "title": "t", "body": long }]))).contains("body"));
        for path in ["/etc/passwd", "../x", "a//b", "a\\b", "", "a/../b"] {
            let error = refused(guide(
                json!([{ "title": "t", "body": "", "anchors": [{ "path": path, "start": 1 }] }]),
            ));
            assert!(error.starts_with("step 1:"), "{path}: {error}");
        }
        assert!(refused(guide(
            json!([{ "title": "t", "body": "", "anchors": [{ "path": "a", "start": 0 }] }])
        ))
        .contains("start at 1"));
        assert!(refused(guide(
            json!([{ "title": "t", "body": "", "anchors": [{ "path": "a", "start": 1, "end": 401 }] }])
        ))
        .contains("400"));
        assert!(refused(guide(
            json!([{ "title": "t", "body": "", "anchors": [{ "path": "a", "start": 1, "side": "left" }] }])
        ))
        .contains("side"));
        let anchors = vec![json!({ "path": "a", "start": 1 }); MAX_ANCHORS + 1];
        assert!(refused(guide(
            json!([{ "title": "t", "body": "", "anchors": anchors }])
        ))
        .contains("anchors"));
        let mut with_head = guide(json!([step()]));
        with_head["headSha"] = json!("not-a-sha");
        assert!(refused(with_head.clone()).contains("headSha"));
        with_head["headSha"] = json!(" ABCDEF1 ");
        assert_eq!(
            validate(&with_head).unwrap().head_sha.as_deref(),
            Some("abcdef1")
        );
    }

    #[test]
    fn pull_request_references_parse() {
        let pr = |repository: &str, number| GuideTarget::PullRequest {
            repository: repository.into(),
            number,
        };
        assert_eq!(
            parse_pull_request("acme/site#12"),
            Some(pr("acme/site", 12))
        );
        assert_eq!(
            parse_pull_request("https://github.com/acme/my.site/pull/7/files?x=1"),
            Some(pr("acme/my.site", 7))
        );
        for bad in [
            "acme#12",
            "acme/site#0",
            "acme/site#x",
            "https://github.com/acme/site/issues/3",
            "a/b/c#1",
            "acme/si te#1",
        ] {
            assert_eq!(parse_pull_request(bad), None, "{bad}");
        }
        assert_eq!(pr("Acme/Site", 3).key(), "pr:acme/site#3");
    }

    fn data_with_session(id: &str) -> AppData {
        let mut data = AppData::default();
        data.sessions.push(serde_json::from_value(json!({
            "id": id, "title": id, "projectId": "p", "agent": "claude", "status": "completed",
            "createdAt": "t", "lastActivityAt": "t", "worktree": { "path": "/tmp", "branch": "main", "isolated": false },
            "messages": []
        })).unwrap());
        data
    }

    #[test]
    fn one_guide_per_target_bounded_and_pruned() {
        let mut data = data_with_session("s");
        let target = GuideTarget::PullRequest {
            repository: "Acme/Site".into(),
            number: 1,
        };
        let content = || validate(&guide(json!([step()]))).unwrap();
        let (first, replaced) = write(&mut data, target.clone(), content(), None, "1");
        assert!(!replaced);
        let lower = GuideTarget::PullRequest {
            repository: "acme/site".into(),
            number: 1,
        };
        let (second, replaced) = write(&mut data, lower.clone(), content(), Some("s".into()), "2");
        assert!(replaced);
        assert_eq!(
            (second.id.as_str(), second.revision),
            (first.id.as_str(), 1)
        );
        assert_eq!(find(&data, &lower).unwrap().created_at, "1");
        write(
            &mut data,
            GuideTarget::Session {
                session_id: "s".into(),
            },
            content(),
            None,
            "3",
        );
        write(
            &mut data,
            GuideTarget::Session {
                session_id: "gone".into(),
            },
            content(),
            None,
            "4",
        );
        assert_eq!(
            data.review_guides.len(),
            2,
            "a deleted session's guide goes"
        );
        for index in 0..MAX_GUIDES {
            let target = GuideTarget::PullRequest {
                repository: "acme/other".into(),
                number: index as u64 + 1,
            };
            write(&mut data, target, content(), None, &format!("5-{index:03}"));
        }
        assert_eq!(data.review_guides.len(), MAX_GUIDES);
        assert!(find(&data, &target).is_none(), "the oldest went first");
    }

    #[test]
    fn guides_round_trip_and_old_state_loads_without_them() {
        let mut data = data_with_session("s");
        write(
            &mut data,
            GuideTarget::Session {
                session_id: "s".into(),
            },
            validate(&guide(json!([step()]))).unwrap(),
            None,
            "t",
        );
        let encoded = serde_json::to_value(&data.review_guides[0]).unwrap();
        assert_eq!(
            encoded["target"],
            json!({ "kind": "session", "sessionId": "s" })
        );
        assert_eq!(encoded["steps"][0]["anchors"][0]["side"], "new");
        assert!(encoded.get("headSha").is_none());
        let decoded: ReviewGuide = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, data.review_guides[0]);
        let old: AppData = serde_json::from_value(json!({ "projects": [], "sessions": [], "settings": serde_json::to_value(&data.settings).unwrap() })).unwrap();
        assert!(old.review_guides.is_empty());
        let action: Action = serde_json::from_value(json!({ "type": "get", "target": { "kind": "pullRequest", "repository": "a/b", "number": 2 } })).unwrap();
        assert!(matches!(
            action,
            Action::Get {
                target: GuideTarget::PullRequest { number: 2, .. }
            }
        ));
    }

    #[test]
    fn tools_describe_their_inputs() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], WRITE_TOOL);
        assert_eq!(
            tools[0]["inputSchema"]["required"],
            json!(["title", "steps"])
        );
        assert_eq!(tools[1]["name"], READ_TOOL);
    }
}
