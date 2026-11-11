//! Guest links (ADR-210): a read-only link to one conversation, or to a snapshot of a page open
//! in the embedded browser ("preview"), for someone without Sirus. The link is
//! `<public route>/s/<token>`; the token is 192 random bits and only its SHA-256 hash is kept
//! (`share-links.json`, `0600`, data folder). A link expires on its own (1 hour, 24 hours or
//! 7 days), counts its views and is switched off by deleting its row. The public routes
//! (`GET /s/{token}` and `GET /s/{token}/a/{id}`) are served by `remote.rs`; everything else
//! about them lives here: the store, the filtering (no absolute paths of this computer, no
//! credentials, no command output unless asked), the page, and the host command.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::activity::{ActivityItem, ItemKind};
use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::{Message, MessageAttachment, MessageRole};
use crate::runtime::State;

const FILE: &str = "share-links.json";
const PREVIEWS: &str = "share-previews";
/// Active links at most (expired ones are removed first).
pub const MAX_LINKS: usize = 50;
/// A preview's HTML: the window captures at most 15 MB; a little more is allowed for the
/// wrapper.
pub const MAX_PREVIEW: usize = 16 * 1024 * 1024;
const MAX_TITLE: usize = 120;
/// Messages a conversation page shows (the latest ones), so one page stays a sensible size.
const MAX_MESSAGES: usize = 400;
const MAX_ITEMS: usize = 200;
const MAX_OUTPUT: usize = 6000;
/// Requests an address may make in a minute, and all addresses together.
const PER_IP_PER_MINUTE: usize = 60;
const ALL_PER_MINUTE: usize = 600;

/// Sent with every answer of the public routes, found or not.
pub const SECURITY_HEADERS: [(&str, &str); 6] = [
    ("cache-control", "no-store"),
    ("x-robots-tag", "noindex, nofollow"),
    ("referrer-policy", "no-referrer"),
    ("x-content-type-options", "nosniff"),
    (
        "content-security-policy",
        "default-src 'none'; style-src 'unsafe-inline'; img-src data: 'self'; font-src data:; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'; sandbox",
    ),
    ("cross-origin-resource-policy", "same-origin"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Conversation,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Validity {
    Hour,
    Day,
    Week,
}

impl Validity {
    fn duration(self) -> Duration {
        match self {
            Self::Hour => Duration::hours(1),
            Self::Day => Duration::hours(24),
            Self::Week => Duration::days(7),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    id: String,
    kind: Kind,
    token_hash: String,
    title: String,
    project: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    hide_output: bool,
    #[serde(default)]
    show_attachments: bool,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    #[serde(default)]
    views: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Links {
    #[serde(default)]
    links: Vec<Stored>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkInfo {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub project: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub views: u64,
    pub hide_output: bool,
    pub show_attachments: bool,
}

impl From<&Stored> for LinkInfo {
    fn from(link: &Stored) -> Self {
        Self {
            id: link.id.clone(),
            kind: link.kind,
            title: link.title.clone(),
            project: link.project.clone(),
            created_at: link.created_at,
            expires_at: link.expires_at,
            views: link.views,
            hide_output: link.hide_output,
            show_attachments: link.show_attachments,
        }
    }
}

/// The link just made: the only time its address exists in full.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    #[serde(flatten)]
    pub info: LinkInfo,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkList {
    pub links: Vec<LinkInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<Created>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    List,
    #[serde(rename_all = "camelCase")]
    Conversation {
        session_id: String,
        validity: Validity,
        #[serde(default = "yes")]
        hide_output: bool,
        #[serde(default)]
        show_attachments: bool,
    },
    Preview {
        title: String,
        html: String,
        validity: Validity,
    },
    Revoke {
        id: String,
    },
}

fn yes() -> bool {
    true
}

// ---------------------------------------------------------------------------------------------
// Store

/// Read-modify-write of the file happens under this.
static FILE_LOCK: Mutex<()> = Mutex::new(());

fn load(dir: &Path) -> Links {
    std::fs::read(dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(dir: &Path, links: &Links) -> Result<()> {
    let path = dir.join(FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(links)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn preview_file(dir: &Path, id: &str) -> Option<PathBuf> {
    uuid::Uuid::parse_str(id)
        .is_ok()
        .then(|| dir.join(PREVIEWS).join(format!("{id}.html")))
}

fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Equality that takes the same time wherever the strings differ.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() ^ b.len()) as u8;
    for index in 0..a.len().max(b.len()) {
        diff |= a.get(index).copied().unwrap_or(0) ^ b.get(index).copied().unwrap_or(0);
    }
    diff == 0
}

/// Drops expired links and their preview files. Whether anything changed.
fn drop_expired(dir: &Path, links: &mut Links, now: DateTime<Utc>) -> bool {
    let before = links.links.len();
    links.links.retain(|link| {
        let live = link.expires_at > now;
        if !live {
            if let Some(file) = preview_file(dir, &link.id) {
                let _ = std::fs::remove_file(file);
            }
        }
        live
    });
    links.links.len() != before
}

/// Removes the expired links (called now and then, and on every change).
pub fn prune(dir: &Path, now: DateTime<Utc>) {
    let _guard = FILE_LOCK.lock();
    let mut links = load(dir);
    if drop_expired(dir, &mut links, now) {
        let _ = save(dir, &links);
    }
}

pub fn list(dir: &Path, now: DateTime<Utc>) -> Vec<LinkInfo> {
    prune(dir, now);
    let mut links: Vec<LinkInfo> = load(dir).links.iter().map(LinkInfo::from).collect();
    links.sort_by_key(|link| std::cmp::Reverse(link.created_at));
    links
}

struct New {
    kind: Kind,
    title: String,
    project: String,
    session_id: Option<String>,
    hide_output: bool,
    show_attachments: bool,
    validity: Validity,
}

/// Makes a link. The token is in the answer and nowhere else.
fn create(
    dir: &Path,
    new: New,
    html: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(LinkInfo, String)> {
    let _guard = FILE_LOCK.lock();
    let mut links = load(dir);
    drop_expired(dir, &mut links, now);
    if links.links.len() >= MAX_LINKS {
        return Err(Error::new(
            "share",
            format!("At most {MAX_LINKS} links are active: turn one off first."),
        ));
    }
    let token = crate::remote::random_token(24);
    let id = uuid::Uuid::new_v4().to_string();
    if let Some(html) = html {
        let file = preview_file(dir, &id).ok_or_else(|| Error::new("share", "Bad id."))?;
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(&file, html)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
        }
    }
    let stored = Stored {
        id,
        kind: new.kind,
        token_hash: hash_token(&token),
        title: new.title,
        project: new.project,
        session_id: new.session_id,
        hide_output: new.hide_output,
        show_attachments: new.show_attachments,
        created_at: now,
        expires_at: now + new.validity.duration(),
        views: 0,
    };
    let info = LinkInfo::from(&stored);
    links.links.push(stored);
    save(dir, &links)?;
    tracing::info!(id = %info.id, kind = ?info.kind, "share link created");
    Ok((info, token))
}

/// Turns a link off: its next request is the "no longer active" page. Whether it was there.
pub fn revoke(dir: &Path, id: &str) -> Result<bool> {
    let _guard = FILE_LOCK.lock();
    let mut links = load(dir);
    let before = links.links.len();
    links.links.retain(|link| link.id != id);
    if links.links.len() == before {
        return Ok(false);
    }
    if let Some(file) = preview_file(dir, id) {
        let _ = std::fs::remove_file(file);
    }
    save(dir, &links)?;
    tracing::info!(id, "share link revoked");
    Ok(true)
}

/// The live link a token belongs to. Every stored hash is compared, whatever the answer, and
/// an expired link is the same as one that never existed.
fn resolve(dir: &Path, token: &str, now: DateTime<Utc>) -> Option<Stored> {
    if token.is_empty() || token.len() > 128 {
        return None;
    }
    let hashed = hash_token(token);
    let mut found = None;
    for link in load(dir).links {
        if constant_time_eq(&link.token_hash, &hashed) && link.expires_at > now {
            found = Some(link);
        }
    }
    found
}

fn count_view(dir: &Path, id: &str) {
    let _guard = FILE_LOCK.lock();
    let mut links = load(dir);
    if let Some(link) = links.links.iter_mut().find(|link| link.id == id) {
        link.views += 1;
        let _ = save(dir, &links);
    }
}

// ---------------------------------------------------------------------------------------------
// Rate limit

type Windows = (HashMap<String, VecDeque<Instant>>, VecDeque<Instant>);
static RATE: LazyLock<Mutex<Windows>> =
    LazyLock::new(|| Mutex::new((HashMap::new(), VecDeque::new())));

/// Whether a request from this address may go on: a minute's window per address and for all.
pub fn allow(ip: &str, now: Instant) -> bool {
    let mut guard = RATE.lock();
    let (by_ip, all) = &mut *guard;
    let minute = std::time::Duration::from_secs(60);
    let fresh = |window: &mut VecDeque<Instant>| {
        while window
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= minute)
        {
            window.pop_front();
        }
    };
    fresh(all);
    if by_ip.len() > 4096 {
        by_ip.retain(|_, window| {
            fresh(window);
            !window.is_empty()
        });
    }
    let window = by_ip.entry(ip.chars().take(64).collect()).or_default();
    fresh(window);
    if window.len() >= PER_IP_PER_MINUTE || all.len() >= ALL_PER_MINUTE {
        return false;
    }
    window.push_back(now);
    all.push_back(now);
    true
}

// ---------------------------------------------------------------------------------------------
// The host command

fn data_dir(state: &AppState) -> Result<PathBuf> {
    state
        .data_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| Error::new("share", "The data folder is unknown."))
}

fn clean_title(title: &str) -> String {
    let one_line: String = title
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    let words: Vec<&str> = one_line.split_whitespace().collect();
    words.join(" ").chars().take(MAX_TITLE).collect()
}

/// The host command behind the share popover and Settings → Conexões: make a link, list the
/// active ones, turn one off.
pub async fn share_link_action(
    state: State<'_, std::sync::Arc<AppState>>,
    action: Action,
) -> Result<LinkList> {
    let state = state.inner().clone();
    crate::runtime::async_runtime::spawn_blocking(move || run_action(&state, action))
        .await
        .map_err(|_| Error::new("share", "The link could not be made."))?
}

fn run_action(state: &AppState, action: Action) -> Result<LinkList> {
    let dir = data_dir(state)?;
    let now = Utc::now();
    let mut created = None;
    match action {
        Action::List => {}
        Action::Conversation {
            session_id,
            validity,
            hide_output,
            show_attachments,
        } => {
            let (title, project) = {
                let data = state.data.lock();
                let session = crate::sirus_tools::find(&data, &session_id)
                    .map_err(|_| Error::not_found("The conversation was not found."))?;
                let project = data
                    .projects
                    .iter()
                    .find(|project| project.id == session.project_id)
                    .map(|project| project.name.clone())
                    .unwrap_or_default();
                (clean_title(&session.title), project)
            };
            let (info, token) = create(
                &dir,
                New {
                    kind: Kind::Conversation,
                    title,
                    project,
                    session_id: Some(session_id),
                    hide_output,
                    show_attachments,
                    validity,
                },
                None,
                now,
            )?;
            created = Some(Created {
                url: format!("{}/s/{token}", crate::remote::share_base_url(&dir)),
                info,
            });
        }
        Action::Preview {
            title,
            html,
            validity,
        } => {
            if html.len() > MAX_PREVIEW {
                return Err(Error::new("share", "The page is too large to share."));
            }
            let html = sanitize_preview(&html);
            let title = clean_title(&title);
            let (info, token) = create(
                &dir,
                New {
                    kind: Kind::Preview,
                    title: if title.is_empty() {
                        "Preview".to_string()
                    } else {
                        title
                    },
                    project: String::new(),
                    session_id: None,
                    hide_output: true,
                    show_attachments: false,
                    validity,
                },
                Some(&html),
                now,
            )?;
            created = Some(Created {
                url: format!("{}/s/{token}", crate::remote::share_base_url(&dir)),
                info,
            });
        }
        Action::Revoke { id } => {
            if !revoke(&dir, &id)? {
                return Err(Error::not_found("No link has this id."));
            }
        }
    }
    Ok(LinkList {
        links: list(&dir, now),
        created,
    })
}

// ---------------------------------------------------------------------------------------------
// Serving

/// What a public route answers; `remote.rs` adds the security headers.
pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

fn html_reply(status: u16, html: String) -> Reply {
    Reply {
        status,
        content_type: "text/html; charset=utf-8",
        body: html.into_bytes(),
    }
}

/// The same answer for a token that never existed, one that expired and one turned off.
pub fn gone(portuguese: bool) -> Reply {
    html_reply(404, gone_page(portuguese))
}

/// `GET /s/{token}`.
pub fn serve_page(
    state: &AppState,
    token: &str,
    portuguese_visitor: bool,
    now: DateTime<Utc>,
) -> Reply {
    let Ok(dir) = data_dir(state) else {
        return gone(portuguese_visitor);
    };
    let Some(link) = resolve(&dir, token, now) else {
        return gone(portuguese_visitor);
    };
    match link.kind {
        Kind::Preview => {
            let Some(html) = preview_file(&dir, &link.id).and_then(|file| std::fs::read(file).ok())
            else {
                return gone(portuguese_visitor);
            };
            count_view(&dir, &link.id);
            Reply {
                status: 200,
                content_type: "text/html; charset=utf-8",
                body: html,
            }
        }
        Kind::Conversation => {
            let Some(id) = link.session_id.clone() else {
                return gone(portuguese_visitor);
            };
            let snapshot = {
                let data = state.data_with_messages(&[&id]);
                let Some(session) = data
                    .sessions
                    .iter()
                    .find(|session| session.id == id && !session.is_auxiliary())
                else {
                    return gone(portuguese_visitor);
                };
                let project = data
                    .projects
                    .iter()
                    .find(|project| project.id == session.project_id);
                Snapshot {
                    title: session.title.clone(),
                    project: project
                        .map(|project| project.name.clone())
                        .unwrap_or_default(),
                    model: session.model.clone().or_else(|| {
                        session
                            .messages
                            .iter()
                            .rev()
                            .find_map(|message| message.activity.as_ref()?.model.clone())
                    }),
                    created_at: session.created_at.clone(),
                    portuguese: data.settings.locale == "pt-BR",
                    strip: strip_prefixes(
                        &session.worktree.path,
                        project.map(|project| project.path.as_str()),
                    ),
                    messages: session
                        .messages
                        .iter()
                        .rev()
                        .take(MAX_MESSAGES + 1)
                        .rev()
                        .cloned()
                        .collect(),
                }
            };
            count_view(&dir, &link.id);
            html_reply(
                200,
                render_conversation(
                    &snapshot,
                    &Options {
                        hide_output: link.hide_output,
                        show_attachments: link.show_attachments,
                        token,
                        expires_in: link.expires_at - now,
                    },
                ),
            )
        }
    }
}

/// `GET /s/{token}/a/{id}`: one image of the shared conversation, when images are shown.
pub fn serve_attachment(
    state: &AppState,
    token: &str,
    attachment: &str,
    portuguese_visitor: bool,
    now: DateTime<Utc>,
) -> Reply {
    let missing = || gone(portuguese_visitor);
    let Ok(dir) = data_dir(state) else {
        return missing();
    };
    let Some(link) = resolve(&dir, token, now) else {
        return missing();
    };
    let (Kind::Conversation, true, Some(id)) =
        (link.kind, link.show_attachments, link.session_id.as_deref())
    else {
        return missing();
    };
    let owned = {
        let data = state.data_with_messages(&[id]);
        data.sessions
            .iter()
            .find(|session| session.id == id && !session.is_auxiliary())
            .is_some_and(|session| {
                session.messages.iter().any(|message| {
                    message
                        .attachments
                        .iter()
                        .any(|item| item.has_thumbnail && item.id.as_deref() == Some(attachment))
                })
            })
    };
    if !owned {
        return missing();
    }
    match crate::thumbnails::read(&state.data_path, attachment) {
        Some(bytes) => Reply {
            status: 200,
            content_type: "image/jpeg",
            body: bytes,
        },
        None => missing(),
    }
}

// ---------------------------------------------------------------------------------------------
// Filtering and rendering

/// What the page needs from the conversation, copied out of the state.
pub struct Snapshot {
    pub title: String,
    pub project: String,
    pub model: Option<String>,
    pub created_at: String,
    pub portuguese: bool,
    pub strip: Vec<(String, String)>,
    pub messages: Vec<Message>,
}

pub struct Options<'a> {
    pub hide_output: bool,
    pub show_attachments: bool,
    pub token: &'a str,
    pub expires_in: Duration,
}

/// Text to put in place of this computer's folders, longest first: the worktree and the project
/// become the project's own root, the home folder becomes `~`.
fn strip_prefixes(worktree: &str, project: Option<&str>) -> Vec<(String, String)> {
    let mut paths: Vec<(String, String)> = Vec::new();
    for path in [Some(worktree), project].into_iter().flatten() {
        let path = path.trim_end_matches('/');
        if path.len() > 1 {
            paths.push((path.to_string(), ".".to_string()));
        }
    }
    if let Some(home) = crate::new_project::home_dir() {
        let home = home.to_string_lossy().trim_end_matches('/').to_string();
        if home.len() > 1 {
            paths.push((home, "~".to_string()));
        }
    }
    paths.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
    paths.dedup_by(|a, b| a.0 == b.0);
    paths
}

/// Replaces absolute folders of this computer with relative ones, then masks credentials.
fn clean_text(text: &str, strip: &[(String, String)]) -> String {
    let mut text = text.to_string();
    for (path, replacement) in strip {
        // `/root/proj/src/a.rs` becomes `src/a.rs`; the folder alone becomes `.`.
        let with_slash = if replacement == "." { "" } else { "~/" };
        text = text.replace(&format!("{path}/"), with_slash);
        text = text.replace(path.as_str(), replacement);
    }
    // Other computers' home folders (a peer's `/Users/ana/…`) lose the user name too.
    let text = OTHER_HOME.replace_all(&text, "~");
    crate::redact::mask(&text).into_owned()
}

static OTHER_HOME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"/(?:Users|home)/[^/\s:"'`)\]]+"#).unwrap());

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            ch => out.push(ch),
        }
    }
    out
}

static CODE_SPAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`\n]+)`").unwrap());
static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*([^*\n]+)\*\*").unwrap());
static ITALIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*([^*\s][^*\n]*)\*").unwrap());
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]\n]+)\]\((https?://[^\s)]+)\)").unwrap());
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.*)$").unwrap());
static BULLET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[-*+]\s+(.*)$").unwrap());
static NUMBERED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\d+[.)]\s+(.*)$").unwrap());
static RULE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:-\s*){3,}$|^\s*(?:\*\s*){3,}$|^\s*(?:_\s*){3,}$").unwrap()
});
static TABLE_RULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$").unwrap());

/// Inline markdown of a line: everything is escaped first, so the only tags in the output are
/// the ones made here; links must be `http` or `https`.
fn inline(text: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    let marked = |segment: &str| {
        let escaped = escape(segment);
        let escaped = LINK.replace_all(
            &escaped,
            "<a href=\"$2\" rel=\"noopener noreferrer nofollow\">$1</a>",
        );
        let escaped = BOLD.replace_all(&escaped, "<strong>$1</strong>");
        ITALIC.replace_all(&escaped, "<em>$1</em>").into_owned()
    };
    for found in CODE_SPAN.captures_iter(text) {
        let whole = found.get(0).unwrap();
        out.push_str(&marked(&text[last..whole.start()]));
        out.push_str(&format!("<code>{}</code>", escape(&found[1])));
        last = whole.end();
    }
    out.push_str(&marked(&text[last..]));
    out
}

fn table_cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(str::trim)
        .collect()
}

fn flush(paragraph: &mut Vec<&str>, out: &mut String) {
    if !paragraph.is_empty() {
        let html: Vec<String> = paragraph.iter().map(|line| inline(line)).collect();
        out.push_str(&format!("<p>{}</p>", html.join("<br>")));
        paragraph.clear();
    }
}

/// A reply's markdown as safe HTML: headings, paragraphs, lists, quotes, tables, rules,
/// fenced code blocks and inline code, bold, italic and links.
pub fn markdown(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut paragraph, &mut out);
            let fence = &trimmed[..3];
            let mut code = Vec::new();
            index += 1;
            while index < lines.len() && !lines[index].trim_start().starts_with(fence) {
                code.push(lines[index]);
                index += 1;
            }
            out.push_str(&format!(
                "<pre><code>{}</code></pre>",
                escape(&code.join("\n"))
            ));
            index += 1;
            continue;
        }
        if line.trim().is_empty() {
            flush(&mut paragraph, &mut out);
            index += 1;
            continue;
        }
        if RULE.is_match(line) {
            flush(&mut paragraph, &mut out);
            out.push_str("<hr>");
            index += 1;
            continue;
        }
        if let Some(found) = HEADING.captures(line) {
            flush(&mut paragraph, &mut out);
            let level = (found[1].len() + 2).min(6);
            out.push_str(&format!("<h{level}>{}</h{level}>", inline(&found[2])));
            index += 1;
            continue;
        }
        if trimmed.starts_with('>') {
            flush(&mut paragraph, &mut out);
            let mut quote = Vec::new();
            while index < lines.len() && lines[index].trim_start().starts_with('>') {
                quote.push(
                    lines[index]
                        .trim_start()
                        .trim_start_matches('>')
                        .trim_start(),
                );
                index += 1;
            }
            let html: Vec<String> = quote.iter().map(|line| inline(line)).collect();
            out.push_str(&format!("<blockquote>{}</blockquote>", html.join("<br>")));
            continue;
        }
        if line.contains('|')
            && lines
                .get(index + 1)
                .is_some_and(|next| next.contains('-') && TABLE_RULE.is_match(next))
        {
            flush(&mut paragraph, &mut out);
            let cells = |row: &str, tag: &str| {
                table_cells(row)
                    .iter()
                    .map(|cell| format!("<{tag}>{}</{tag}>", inline(cell)))
                    .collect::<String>()
            };
            out.push_str(&format!(
                "<div class=\"tbl\"><table><thead><tr>{}</tr></thead><tbody>",
                cells(line, "th")
            ));
            index += 2;
            while index < lines.len()
                && lines[index].contains('|')
                && !lines[index].trim().is_empty()
            {
                out.push_str(&format!("<tr>{}</tr>", cells(lines[index], "td")));
                index += 1;
            }
            out.push_str("</tbody></table></div>");
            continue;
        }
        let ordered = NUMBERED.is_match(line);
        if ordered || BULLET.is_match(line) {
            flush(&mut paragraph, &mut out);
            let pattern = if ordered { &NUMBERED } else { &BULLET };
            let tag = if ordered { "ol" } else { "ul" };
            out.push_str(&format!("<{tag}>"));
            while index < lines.len() {
                let Some(found) = pattern.captures(lines[index]) else {
                    break;
                };
                out.push_str(&format!("<li>{}</li>", inline(&found[1])));
                index += 1;
            }
            out.push_str(&format!("</{tag}>"));
            continue;
        }
        paragraph.push(line);
        index += 1;
    }
    flush(&mut paragraph, &mut out);
    out
}

fn plural(portuguese: bool, count: usize, pt: (&str, &str), en: (&str, &str)) -> String {
    let (one, many) = if portuguese { pt } else { en };
    format!("{count} {}", if count == 1 { one } else { many })
}

/// "Read 3 files · ran 2 commands", from one reply's activity.
fn tools_summary(items: &[&ActivityItem], portuguese: bool) -> String {
    let count = |kinds: &[ItemKind]| {
        items
            .iter()
            .filter(|item| kinds.contains(&item.kind))
            .count()
    };
    let say = |pt: &str, en: &str, text: String| {
        if portuguese {
            format!("{pt} {text}")
        } else {
            format!("{en} {text}")
        }
    };
    let files = |n: usize| plural(portuguese, n, ("arquivo", "arquivos"), ("file", "files"));
    let mut parts = Vec::new();
    let reads = count(&[ItemKind::Read]);
    if reads > 0 {
        parts.push(say("leu", "read", files(reads)));
    }
    let edits = count(&[ItemKind::Edit]);
    if edits > 0 {
        parts.push(say("editou", "edited", files(edits)));
    }
    let commands = count(&[ItemKind::Command]);
    if commands > 0 {
        let text = plural(
            portuguese,
            commands,
            ("comando", "comandos"),
            ("command", "commands"),
        );
        parts.push(say("rodou", "ran", text));
    }
    let others = count(&[ItemKind::Tool, ItemKind::Agent, ItemKind::Skill]);
    if others > 0 {
        let text = plural(
            portuguese,
            others,
            ("ferramenta", "ferramentas"),
            ("tool", "tools"),
        );
        parts.push(say("usou", "used", text));
    }
    let joined = parts.join(" · ");
    let mut chars = joined.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => joined,
    }
}

fn tail_chars(text: &str, max: usize) -> &str {
    let count = text.chars().count();
    if count <= max {
        return text;
    }
    let skip = text.char_indices().nth(count - max).map_or(0, |(at, _)| at);
    &text[skip..]
}

fn tools_html(items: &[ActivityItem], snapshot: &Snapshot, options: &Options) -> String {
    let items: Vec<&ActivityItem> = items
        .iter()
        .filter(|item| !item.finished_task)
        .take(MAX_ITEMS)
        .collect();
    let summary = tools_summary(&items, snapshot.portuguese);
    if summary.is_empty() {
        return String::new();
    }
    if options.hide_output {
        return format!("<div class=\"tool\">{}</div>", escape(&summary));
    }
    let mut rows = String::new();
    for item in items {
        let detail = clean_text(&item.detail, &snapshot.strip);
        let label = clean_text(&item.label, &snapshot.strip);
        rows.push_str(&format!(
            "<li><span class=\"k\">{}</span> <code>{}</code>",
            escape(&label),
            escape(&detail)
        ));
        if item.kind == ItemKind::Command && !item.output.is_empty() {
            let output = clean_text(tail_chars(&item.output, MAX_OUTPUT), &snapshot.strip);
            rows.push_str(&format!("<pre><code>{}</code></pre>", escape(&output)));
        }
        rows.push_str("</li>");
    }
    format!(
        "<details class=\"tool\"><summary>{}</summary><ul>{rows}</ul></details>",
        escape(&summary)
    )
}

fn attachments_html(
    attachments: &[MessageAttachment],
    snapshot: &Snapshot,
    options: &Options,
) -> String {
    let mut out = String::new();
    for attachment in attachments {
        let name = escape(&clean_text(&attachment.name, &snapshot.strip));
        match (
            &attachment.id,
            attachment.has_thumbnail,
            &attachment.thumbnail,
        ) {
            (Some(id), true, _) if crate::thumbnails::valid_id(id) => out.push_str(&format!(
                "<img class=\"att\" alt=\"{name}\" src=\"/s/{}/a/{id}\">",
                options.token
            )),
            (_, _, Some(url)) if safe_data_image(url) => {
                out.push_str(&format!("<img class=\"att\" alt=\"{name}\" src=\"{url}\">"))
            }
            _ => out.push_str(&format!("<span class=\"chip\">{name}</span>")),
        }
    }
    if out.is_empty() {
        out
    } else {
        format!("<div class=\"atts\">{out}</div>")
    }
}

/// `data:image/(png|jpeg|gif|webp);base64,…` with nothing else in it.
fn safe_data_image(url: &str) -> bool {
    let Some(rest) = ["png", "jpeg", "jpg", "gif", "webp"]
        .iter()
        .find_map(|kind| url.strip_prefix(&format!("data:image/{kind};base64,")))
    else {
        return false;
    };
    rest.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

fn remaining(duration: Duration, portuguese: bool) -> String {
    let minutes = duration.num_minutes().max(1);
    let text = if minutes < 90 {
        format!("{minutes} min")
    } else if minutes < 48 * 60 {
        format!("{} h", (minutes + 30) / 60)
    } else {
        format!("{} d", (minutes + 12 * 60) / (24 * 60))
    };
    if portuguese {
        format!("vence em {text}")
    } else {
        format!("expires in {text}")
    }
}

const STYLE: &str = "\
:root{color-scheme:dark;--acc:#7a9cff;--t:rgba(255,255,255,.92);--d:rgba(255,255,255,.58);--f:rgba(255,255,255,.34);--line:rgba(255,255,255,.09)}\
*{box-sizing:border-box}html{-webkit-text-size-adjust:100%}\
body{margin:0;background:#050506;color:var(--t);font:15px/1.55 -apple-system,BlinkMacSystemFont,'SF Pro Text',Inter,'Segoe UI',sans-serif;min-height:100vh;position:relative}\
body::before{content:'';position:fixed;inset:0;pointer-events:none;background-image:radial-gradient(rgba(122,156,255,.22) 1px,transparent 1.3px);background-size:6px 6px;-webkit-mask-image:radial-gradient(ellipse 70% 45% at 50% 100%,#000,transparent);mask-image:radial-gradient(ellipse 70% 45% at 50% 100%,#000,transparent)}\
header,main,footer{position:relative;z-index:1}\
header{display:flex;flex-wrap:wrap;align-items:center;gap:10px 12px;padding:16px 28px;border-bottom:1px solid var(--line)}\
.logo{width:22px;height:22px;border-radius:6px;background:linear-gradient(135deg,#8fa8ff,#5b6fd6);flex:none}\
.head{min-width:0;flex:1 1 260px}.tt{font-weight:600;overflow-wrap:anywhere}.pm{color:var(--d);font-size:12.5px}\
.pill{font-size:12px;color:var(--d);padding:4px 10px;border-radius:999px;border:1px solid rgba(255,255,255,.12);white-space:nowrap}\
main{max-width:740px;margin:0 auto;padding:22px 24px 90px}\
.bub{margin:18px 0 8px auto;width:fit-content;max-width:85%;background:#1e2740;padding:9px 14px;border-radius:14px;white-space:pre-wrap;overflow-wrap:anywhere}\
.who{display:flex;align-items:center;gap:8px;color:var(--d);font-size:12.5px;margin:18px 0 6px}\
.prov{width:18px;height:18px;border-radius:5px;background:#d0805a;display:inline-block}\
.rep{color:rgba(255,255,255,.86);overflow-wrap:anywhere}.rep p{margin:0 0 10px}.rep h3,.rep h4,.rep h5,.rep h6{margin:16px 0 6px;font-size:1em}.rep h3{font-size:1.1em}\
.rep ul,.rep ol{margin:0 0 10px;padding-left:22px}.rep a{color:var(--acc)}\
.rep blockquote{margin:0 0 10px;padding:2px 12px;border-left:2px solid var(--line);color:var(--d)}\
hr{border:0;border-top:1px solid var(--line);margin:14px 0}\
code{font:12.5px ui-monospace,Menlo,Consolas,monospace;background:rgba(255,255,255,.07);padding:1px 5px;border-radius:4px}\
pre{margin:0 0 12px;padding:10px 12px;background:rgba(255,255,255,.05);border:1px solid var(--line);border-radius:8px;overflow-x:auto}pre code{background:none;padding:0}\
.tbl{overflow-x:auto;margin:0 0 12px}table{border-collapse:collapse;font-size:13.5px}th,td{border:1px solid var(--line);padding:5px 10px;text-align:left}th{background:rgba(255,255,255,.04)}\
.tool{display:block;width:fit-content;max-width:100%;color:var(--f);font-size:12.5px;padding:6px 10px;border-radius:8px;border:1px solid var(--line);margin:8px 0}\
details.tool summary{cursor:pointer;color:var(--d)}details.tool ul{margin:8px 0 0;padding-left:18px;color:var(--d)}.k{color:var(--f)}\
.atts{display:flex;flex-wrap:wrap;gap:8px;justify-content:flex-end;margin:4px 0}.att{max-width:min(320px,100%);max-height:240px;border-radius:10px;border:1px solid var(--line)}\
.chip{font-size:12px;padding:3px 9px;border-radius:999px;border:1px solid var(--line);color:var(--d)}\
.note{color:var(--f);font-size:12.5px;text-align:center;margin:10px 0}\
footer{position:fixed;left:0;right:0;bottom:0;display:flex;align-items:center;gap:10px;padding:11px 28px;border-top:1px solid var(--line);background:rgba(6,6,7,.88);color:var(--f);font-size:12.5px}\
footer .logo{width:16px;height:16px;border-radius:4px}\
.gone{min-height:100vh;display:grid;place-items:center;text-align:center;padding:24px;position:relative;z-index:1}.gone .logo{margin:0 auto}.gone h1{font-size:18px;margin:14px 0 4px}.gone p{color:var(--d);margin:0 auto;max-width:300px}\
@media(max-width:560px){header{padding:40px 16px 12px}main{padding:12px 16px 80px}footer{padding:10px 16px}.bub{max-width:92%}}";

fn shell(lang: &str, title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"{lang}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex,nofollow\"><meta name=\"referrer\" content=\"no-referrer\">\
         <title>{}</title><style>{STYLE}</style></head><body>{body}</body></html>",
        escape(title)
    )
}

/// The page for a link that is not active: nothing about any conversation.
pub fn gone_page(portuguese: bool) -> String {
    let (title, text) = if portuguese {
        (
            "Este link não está mais ativo",
            "Ele venceu ou foi desligado por quem compartilhou.",
        )
    } else {
        (
            "This link is no longer active",
            "It expired or was turned off by whoever shared it.",
        )
    };
    shell(
        if portuguese { "pt-BR" } else { "en" },
        title,
        &format!(
            "<div class=\"gone\"><div><div class=\"logo\"></div><h1>{title}</h1><p>{text}</p></div></div>"
        ),
    )
}

/// The read-only page of a conversation.
pub fn render_conversation(snapshot: &Snapshot, options: &Options) -> String {
    let pt = snapshot.portuguese;
    let strip = &snapshot.strip;
    let title = clean_text(&snapshot.title, strip);
    let date = DateTime::parse_from_rfc3339(&snapshot.created_at)
        .map(|at| {
            at.format(if pt { "%d/%m/%Y" } else { "%Y-%m-%d" })
                .to_string()
        })
        .unwrap_or_default();
    let meta: Vec<String> = [
        clean_text(&snapshot.project, strip),
        snapshot
            .model
            .as_deref()
            .map(|model| clean_text(model, strip))
            .unwrap_or_default(),
        date,
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .map(|part| escape(&part))
    .collect();
    let mut body = format!(
        "<header><div class=\"logo\"></div><div class=\"head\"><div class=\"tt\">{}</div>\
         <div class=\"pm\">{}</div></div><div class=\"pill\">{} · {}</div></header><main>",
        escape(&title),
        meta.join(" · "),
        if pt { "Só leitura" } else { "Read only" },
        remaining(options.expires_in, pt),
    );
    let shown: &[Message] = if snapshot.messages.len() > MAX_MESSAGES {
        body.push_str(&format!(
            "<div class=\"note\">{}</div>",
            if pt {
                "Mensagens anteriores não aparecem aqui."
            } else {
                "Earlier messages are not shown here."
            }
        ));
        &snapshot.messages[snapshot.messages.len() - MAX_MESSAGES..]
    } else {
        &snapshot.messages
    };
    for message in shown {
        match message.role {
            MessageRole::System => {}
            MessageRole::User => {
                if options.show_attachments && !message.attachments.is_empty() {
                    body.push_str(&attachments_html(&message.attachments, snapshot, options));
                }
                if !message.content.trim().is_empty() {
                    body.push_str(&format!(
                        "<div class=\"bub\">{}</div>",
                        escape(&clean_text(&message.content, strip))
                    ));
                }
            }
            MessageRole::Agent => {
                let tools = message
                    .activity
                    .as_ref()
                    .map(|activity| tools_html(&activity.items, snapshot, options))
                    .unwrap_or_default();
                if tools.is_empty() && message.content.trim().is_empty() {
                    continue;
                }
                let name = message
                    .activity
                    .as_ref()
                    .and_then(|activity| activity.model.clone())
                    .or_else(|| snapshot.model.clone())
                    .unwrap_or_else(|| "Agent".to_string());
                body.push_str(&format!(
                    "<div class=\"who\"><span class=\"prov\"></span>{}</div>{tools}",
                    escape(&clean_text(&name, strip))
                ));
                if !message.content.trim().is_empty() {
                    body.push_str(&format!(
                        "<div class=\"rep\">{}</div>",
                        markdown(&clean_text(&message.content, strip))
                    ));
                }
            }
        }
    }
    body.push_str(&format!(
        "</main><footer><div class=\"logo\"></div>{}</footer>",
        if pt {
            "Compartilhado pelo Sirus · só leitura"
        } else {
            "Shared with Sirus · read only"
        }
    ));
    shell(if pt { "pt-BR" } else { "en" }, &title, &body)
}

static SCRIPT_BLOCK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<script\b.*?</script\s*>").unwrap());
static ACTIVE_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<iframe\b.*?</iframe\s*>|<object\b.*?</object\s*>|<embed\b.*?</embed\s*>|<applet\b.*?</applet\s*>|<noscript\b.*?</noscript\s*>").unwrap()
});
static ACTIVE_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?is)</?(?:script|iframe|object|embed|applet|frame|frameset|base|link)\b[^>]*>|<meta\s[^>]*http-equiv[^>]*>",
    )
    .unwrap()
});
static EVENT_ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)[\s/]on[a-z]+\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+)"#).unwrap()
});
static JAVASCRIPT_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)javascript\s*:").unwrap());

/// A captured page without anything that runs or loads: no scripts, frames, plugins, `<base>`,
/// `<link>`, refresh redirects, event handlers or `javascript:` addresses. (The answer's
/// content-security policy and `sandbox` stop all of that anyway; this keeps the stored file
/// clean too.)
pub fn sanitize_preview(html: &str) -> String {
    // Removing one tag can join its neighbours into a new one (`<scr<script></script>ipt>`),
    // so go on until nothing changes.
    let mut current = sanitize_once(html);
    for _ in 0..8 {
        let next = sanitize_once(&current);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?(?:-->|\z)").unwrap());

fn sanitize_once(html: &str) -> String {
    let html = COMMENT.replace_all(html, "");
    let html = SCRIPT_BLOCK.replace_all(&html, "");
    let html = ACTIVE_BLOCK.replace_all(&html, "");
    let html = ACTIVE_TAG.replace_all(&html, "");
    let html = EVENT_ATTRIBUTE.replace_all(&html, "");
    JAVASCRIPT_URL.replace_all(&html, "blocked:").into_owned()
}

/// Whether the visitor's language is Portuguese (for the "no longer active" page).
pub fn visitor_portuguese(accept_language: Option<&str>) -> bool {
    accept_language.is_some_and(|value| value.trim().to_ascii_lowercase().starts_with("pt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn new(kind: Kind, validity: Validity) -> New {
        New {
            kind,
            title: "A title".into(),
            project: "proj".into(),
            session_id: Some("s".into()),
            hide_output: true,
            show_attachments: false,
            validity,
        }
    }

    fn state(dir: &Path) -> AppState {
        AppState::new(
            dir.join("state.db"),
            dir.join("worktrees"),
            crate::models::AppData::default(),
        )
    }

    #[test]
    fn tokens_are_long_url_safe_and_only_their_hash_is_kept() {
        let dir = dir();
        let now = Utc::now();
        let (_, token) = create(
            dir.path(),
            new(Kind::Conversation, Validity::Day),
            None,
            now,
        )
        .unwrap();
        assert!(token.len() >= 32, "192 bits is 32 base64 characters");
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        );
        let stored = std::fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(!stored.contains(&token), "the secret is never written");
        assert!(stored.contains(&hash_token(&token)));
        let (_, other) = create(
            dir.path(),
            new(Kind::Conversation, Validity::Day),
            None,
            now,
        )
        .unwrap();
        assert_ne!(token, other);
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }

    #[test]
    fn a_link_resolves_until_it_expires_or_is_turned_off() {
        let dir = dir();
        let now = Utc::now();
        let (info, token) = create(
            dir.path(),
            new(Kind::Conversation, Validity::Hour),
            None,
            now,
        )
        .unwrap();
        assert!(resolve(dir.path(), &token, now).is_some());
        assert!(resolve(dir.path(), &token, now + Duration::minutes(59)).is_some());
        assert!(resolve(dir.path(), &token, now + Duration::minutes(61)).is_none());
        assert!(resolve(dir.path(), "nope", now).is_none());
        assert!(resolve(dir.path(), "", now).is_none());
        assert!(resolve(dir.path(), &"x".repeat(500), now).is_none());
        assert!(revoke(dir.path(), &info.id).unwrap());
        assert!(!revoke(dir.path(), &info.id).unwrap());
        assert!(resolve(dir.path(), &token, now).is_none());
    }

    #[test]
    fn validities_are_one_hour_a_day_and_a_week() {
        let dir = dir();
        let now = Utc::now();
        for (validity, hours) in [
            (Validity::Hour, 1),
            (Validity::Day, 24),
            (Validity::Week, 168),
        ] {
            let (info, _) =
                create(dir.path(), new(Kind::Conversation, validity), None, now).unwrap();
            assert_eq!(info.expires_at - now, Duration::hours(hours));
        }
    }

    #[test]
    fn expired_links_are_pruned_with_their_preview_files_and_the_count_is_capped() {
        let dir = dir();
        let now = Utc::now();
        let (info, _) = create(
            dir.path(),
            new(Kind::Preview, Validity::Hour),
            Some("<p>x</p>"),
            now,
        )
        .unwrap();
        let file = preview_file(dir.path(), &info.id).unwrap();
        assert!(file.exists());
        assert_eq!(list(dir.path(), now).len(), 1);
        assert!(list(dir.path(), now + Duration::hours(2)).is_empty());
        assert!(!file.exists());
        for _ in 0..MAX_LINKS {
            create(
                dir.path(),
                new(Kind::Conversation, Validity::Day),
                None,
                now,
            )
            .unwrap();
        }
        assert!(
            create(
                dir.path(),
                new(Kind::Conversation, Validity::Day),
                None,
                now
            )
            .is_err()
        );
        // Room again once they expire.
        assert!(
            create(
                dir.path(),
                new(Kind::Conversation, Validity::Day),
                None,
                now + Duration::days(2)
            )
            .is_ok()
        );
    }

    #[test]
    fn views_are_counted() {
        let dir = dir();
        let now = Utc::now();
        let (info, _) = create(
            dir.path(),
            new(Kind::Conversation, Validity::Day),
            None,
            now,
        )
        .unwrap();
        count_view(dir.path(), &info.id);
        count_view(dir.path(), &info.id);
        assert_eq!(list(dir.path(), now)[0].views, 2);
    }

    #[test]
    fn a_missing_expired_and_revoked_link_get_the_same_answer() {
        let dir = dir();
        let state = state(dir.path());
        let now = Utc::now();
        let (info, token) = create(
            dir.path(),
            new(Kind::Conversation, Validity::Hour),
            None,
            now,
        )
        .unwrap();
        let missing = serve_page(&state, "never-made", false, now);
        let expired = serve_page(&state, &token, false, now + Duration::hours(3));
        revoke(dir.path(), &info.id).unwrap();
        let revoked = serve_page(&state, &token, false, now);
        for reply in [&missing, &expired, &revoked] {
            assert_eq!(reply.status, 404);
            assert_eq!(reply.body, missing.body);
            let page = String::from_utf8(reply.body.clone()).unwrap();
            assert!(!page.contains("A title") && !page.contains("proj"));
        }
        assert!(
            String::from_utf8(missing.body)
                .unwrap()
                .contains("no longer active")
        );
        let portuguese = String::from_utf8(gone(true).body).unwrap();
        assert!(portuguese.contains("Este link não está mais ativo"));
        assert!(portuguese.contains("Ele venceu ou foi desligado por quem compartilhou."));
    }

    #[test]
    fn a_preview_is_served_and_counted_until_it_expires() {
        let dir = dir();
        let state = state(dir.path());
        let now = Utc::now();
        let (_, token) = create(
            dir.path(),
            new(Kind::Preview, Validity::Hour),
            Some("<h1>Hello</h1>"),
            now,
        )
        .unwrap();
        let reply = serve_page(&state, &token, false, now);
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, b"<h1>Hello</h1>");
        assert_eq!(list(dir.path(), now)[0].views, 1);
        assert_eq!(
            serve_page(&state, &token, false, now + Duration::hours(2)).status,
            404
        );
    }

    #[test]
    fn security_headers_are_restrictive() {
        let get = |name: &str| {
            SECURITY_HEADERS
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| *value)
                .unwrap()
        };
        assert_eq!(get("cache-control"), "no-store");
        assert!(get("x-robots-tag").contains("noindex"));
        assert_eq!(get("referrer-policy"), "no-referrer");
        assert_eq!(get("x-content-type-options"), "nosniff");
        let csp = get("content-security-policy");
        assert!(
            csp.starts_with("default-src 'none'; style-src 'unsafe-inline'; img-src data: 'self'")
        );
        assert!(!csp.contains("script-src") && !csp.contains("'unsafe-eval'"));
        assert!(csp.contains("sandbox"));
    }

    #[test]
    fn the_rate_limit_is_per_address() {
        let now = Instant::now();
        let ip = format!("test-{}", uuid::Uuid::new_v4());
        for _ in 0..PER_IP_PER_MINUTE {
            assert!(allow(&ip, now));
        }
        assert!(!allow(&ip, now));
        assert!(allow(&format!("other-{}", uuid::Uuid::new_v4()), now));
        assert!(allow(&ip, now + std::time::Duration::from_secs(61)));
    }

    fn message(value: serde_json::Value) -> Message {
        let mut base = serde_json::json!({
            "id": "m", "sessionId": "s", "role": "user", "content": "hi",
            "createdAt": "2026-11-11T10:00:00Z", "streaming": false
        });
        for (key, value) in value.as_object().unwrap() {
            base[key] = value.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    fn activity(items: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "provider": "claude", "model": "Opus 5.5", "startedAt": 1, "endedAt": 2,
            "waitingSince": null, "pausedMs": 0, "status": "completed",
            "items": items, "truncated": false
        })
    }

    fn item(kind: &str, detail: &str, output: &str) -> serde_json::Value {
        serde_json::json!({
            "id": uuid::Uuid::new_v4().to_string(), "kind": kind, "label": kind,
            "state": "completed", "model": null, "detail": detail, "output": output
        })
    }

    fn snapshot(messages: Vec<Message>) -> Snapshot {
        Snapshot {
            title: "Fix the dock in /srv/work/app/web".into(),
            project: "app".into(),
            model: Some("Opus 5.5".into()),
            created_at: "2026-11-11T10:00:00Z".into(),
            portuguese: true,
            strip: strip_prefixes("/srv/work/wt/abc", Some("/srv/work/app")),
            messages,
        }
    }

    fn options(hide_output: bool, show_attachments: bool) -> Options<'static> {
        Options {
            hide_output,
            show_attachments,
            token: "tok",
            expires_in: Duration::hours(23),
        }
    }

    #[test]
    fn the_page_hides_command_output_and_shortens_paths_and_masks_secrets() {
        let reply = message(serde_json::json!({
            "role": "agent",
            "content": "Edited /srv/work/wt/abc/src/a.rs and /srv/work/app/b.md with token sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789",
            "activity": activity(serde_json::json!([
                item("read", "/srv/work/wt/abc/src/a.rs", ""),
                item("read", "/srv/work/app/b.md", ""),
                item("command", "cat /etc/secret", "TOP-SECRET-OUTPUT /srv/work/app/x"),
                item("command", "ls", "more output"),
            ]))
        }));
        let hidden = render_conversation(&snapshot(vec![reply.clone()]), &options(true, false));
        assert!(!hidden.contains("TOP-SECRET-OUTPUT") && !hidden.contains("cat /etc/secret"));
        assert!(hidden.contains("Leu 2 arquivos · rodou 2 comandos"));
        assert!(
            !hidden.contains("/srv/work"),
            "absolute folders are gone: {hidden}"
        );
        assert!(hidden.contains("src/a.rs") && hidden.contains("b.md"));
        assert!(!hidden.contains("abcdefghijklmnopqrstuvwxyz0123456789"));
        assert!(hidden.contains("Só leitura · vence em 23 h"));
        assert!(hidden.contains("Compartilhado pelo Sirus · só leitura"));
        assert!(!hidden.contains("<script") && !hidden.contains("<textarea"));
        let shown = render_conversation(&snapshot(vec![reply]), &options(false, false));
        assert!(shown.contains("TOP-SECRET-OUTPUT x"));
        assert!(!shown.contains("/srv/work"));
    }

    #[test]
    fn the_page_escapes_html_and_markdown_from_messages() {
        let user = message(serde_json::json!({"content": "<script>alert(1)</script> & \"q\""}));
        let agent = message(serde_json::json!({
            "role": "agent",
            "content": "# Title <b>x</b>\n\nSome **bold** and `a<b` and [bad](javascript:alert(1)) and [ok](https://example.com/?a=1&b=\"2).\n\n```rust\nfn main() { <img src=x onerror=alert(1)> }\n```\n\n- one\n- two\n\n1. first\n\n| a | b |\n|---|---|\n| 1 | <i>2</i> |\n"
        }));
        let page = render_conversation(&snapshot(vec![user, agent]), &options(true, false));
        assert!(!page.contains("<script>alert") && !page.contains("<img src=x"));
        assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;q&quot;"));
        assert!(page.contains("<strong>bold</strong>"));
        assert!(page.contains("<code>a&lt;b</code>"));
        assert!(
            page.contains(
                "<pre><code>fn main() { &lt;img src=x onerror=alert(1)&gt; }</code></pre>"
            )
        );
        assert!(!page.contains("href=\"javascript"));
        assert!(page.contains("href=\"https://example.com/?a=1&amp;b=&quot;2\""));
        assert!(page.contains("<ul><li>one</li><li>two</li></ul>"));
        assert!(page.contains("<ol><li>first</li></ol>"));
        assert!(page.contains("<th>a</th>") && page.contains("<td>&lt;i&gt;2&lt;/i&gt;</td>"));
        assert!(page.contains("&lt;b&gt;x&lt;/b&gt;"));
    }

    #[test]
    fn attachments_appear_only_when_asked_and_only_as_images_of_the_link() {
        let id = uuid::Uuid::new_v4().to_string();
        let user = message(serde_json::json!({
            "content": "look",
            "attachments": [
                {"id": id, "name": "shot.png", "kind": "file", "hasThumbnail": true},
                {"name": "/srv/work/app/notes.txt", "kind": "file"},
                {"name": "evil", "kind": "file", "thumbnail": "data:image/png;base64,AAAA\"onerror=\"x"}
            ]
        }));
        let off = render_conversation(&snapshot(vec![user.clone()]), &options(true, false));
        assert!(!off.contains("shot.png") && !off.contains("/a/") && !off.contains("notes.txt"));
        let on = render_conversation(&snapshot(vec![user]), &options(true, true));
        assert!(on.contains(&format!("src=\"/s/tok/a/{id}\"")));
        assert!(on.contains("notes.txt") && !on.contains("/srv/work"));
        assert!(
            !on.contains("onerror=\"x\""),
            "an unsafe data address is only a name"
        );
    }

    #[test]
    fn the_page_offers_no_way_to_write_or_act() {
        let page = render_conversation(
            &snapshot(vec![message(serde_json::json!({"content": "hi"}))]),
            &options(true, false),
        );
        for forbidden in [
            "<form",
            "<input",
            "<button",
            "<textarea",
            "<script",
            "<iframe",
        ] {
            assert!(!page.contains(forbidden), "{forbidden}");
        }
        assert!(page.contains("viewport"));
        assert!(page.contains("noindex"));
    }

    #[test]
    fn a_preview_loses_scripts_frames_and_handlers() {
        let clean = sanitize_preview(
            "<html><head><meta charset=utf-8><meta http-equiv=\"refresh\" content=\"0;url=x\"><base href=\"/\"><link rel=stylesheet href=\"x.css\"><style>p{color:red}</style><script>evil()</script></head>\
             <body onload=\"evil()\"><p onclick='x()'>ok</p><a href=\"javascript:alert(1)\">a</a><iframe src=x></iframe><script src=y.js></script><img src=\"data:image/png;base64,AAAA\"></body></html>",
        );
        for gone in [
            "evil()",
            "refresh",
            "<base",
            "<link",
            "<iframe",
            "onclick",
            "onload",
            "javascript:",
            "<script",
        ] {
            assert!(!clean.contains(gone), "{gone} in {clean}");
        }
        assert!(clean.contains("<style>p{color:red}</style>") && clean.contains("<p"));
        assert!(clean.contains("data:image/png;base64,AAAA") && clean.contains("charset=utf-8"));
    }

    #[test]
    fn the_command_makes_lists_and_turns_off_links() {
        let dir = dir();
        let state = state(dir.path());
        let made = run_action(
            &state,
            Action::Preview {
                title: "  My\npage ".into(),
                html: "<p>x</p><script>y()</script>".into(),
                validity: Validity::Day,
            },
        )
        .unwrap();
        let created = made.created.unwrap();
        assert!(created.url.contains("/s/"));
        assert_eq!(created.info.title, "My page");
        assert_eq!(made.links.len(), 1);
        let file = preview_file(dir.path(), &created.info.id).unwrap();
        assert_eq!(std::fs::read_to_string(file).unwrap(), "<p>x</p>");
        let listed = run_action(&state, Action::List).unwrap();
        assert!(listed.created.is_none());
        let json = serde_json::to_string(&listed).unwrap();
        assert!(!json.contains("/s/"));
        assert!(run_action(&state, Action::Revoke { id: "nope".into() }).is_err());
        let after = run_action(
            &state,
            Action::Revoke {
                id: created.info.id,
            },
        )
        .unwrap();
        assert!(after.links.is_empty());
        assert!(
            run_action(
                &state,
                Action::Conversation {
                    session_id: "missing".into(),
                    validity: Validity::Hour,
                    hide_output: true,
                    show_attachments: false
                }
            )
            .is_err()
        );
        let action: Action = serde_json::from_value(serde_json::json!({
            "type": "conversation", "sessionId": "x", "validity": "week"
        }))
        .unwrap();
        assert!(matches!(
            action,
            Action::Conversation {
                hide_output: true,
                show_attachments: false,
                ..
            }
        ));
    }

    #[test]
    fn remaining_time_reads_naturally() {
        assert_eq!(remaining(Duration::minutes(20), true), "vence em 20 min");
        assert_eq!(remaining(Duration::hours(23), false), "expires in 23 h");
        assert_eq!(remaining(Duration::days(7), true), "vence em 7 d");
    }

    #[test]
    fn sanitizer_survives_tags_that_rejoin_and_drops_comments() {
        let clean = sanitize_preview(
            "<p>a</p><scr<script></script>ipt>alert(1)</scr<script></script>ipt><img on<b onx=1>error=x><!-- key sk-abc --><scr<!-- -->ipt>x</script>",
        );
        let lower = clean.to_ascii_lowercase();
        assert!(!lower.contains("<script"), "{clean}");
        assert!(!lower.contains("onerror"), "{clean}");
        assert!(
            !clean.contains("sk-abc") && !clean.contains("<!--"),
            "{clean}"
        );
        assert!(clean.contains("<p>a</p>"));
    }

    #[test]
    fn other_computers_home_folders_lose_the_user_name() {
        let text = clean_text("see /Users/ana/proj/a.rs and (/home/bob/.ssh/id)", &[]);
        assert_eq!(text, "see ~/proj/a.rs and (~/.ssh/id)");
    }
}
