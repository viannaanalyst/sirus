//! The Pull requests page, after Sirus's `src/components/pull-requests/` (ADR-050): the review
//! inbox of the GitHub repository of the project selected in the sidebar, with a list column
//! (search and four segments: Abertos, Mesclados, Fechados, Issues) beside the selected item's detail (Resumo, Atividade, Checks and Código).
//! It opens as a tab in the title bar (shell_terminal.rs). Everything runs on the host through
//! `pull_request_action`; this file only asks and draws. The parsing and the list logic at the top
//! are pure and tested. Not ported: pins and unread marks (they live in the saved settings), the
//! review guide, inline review threads, the PR watch list and "send to agent".
use crate::backend::Backend;
use crate::i18n::{t, tf};
use crate::motion::{Curve, Tween};
use crate::panel_changes::{age, diff_line, note};
use crate::panel_files::{TextField, TextFieldEvent};
use crate::theme::Sinks as _;
use crate::theme::{RADIUS_MD, RADIUS_SM, accent, content, diff_colors, icon, on_accent};
use gpui::{
    AnyElement, App, Context, Div, ElementId, Entity, EventEmitter, FontWeight, Render,
    SharedString, Stateful, Subscription, Window, div, prelude::*, px, rgb, uniform_list,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

// ---------------------------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Pull,
    Issue,
}

impl Kind {
    fn wire(self) -> &'static str {
        match self {
            Kind::Pull => "pullRequest",
            Kind::Issue => "issue",
        }
    }
    fn parse(text: &str) -> Kind {
        if text == "issue" {
            Kind::Issue
        } else {
            Kind::Pull
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum State {
    Open,
    Closed,
    Merged,
}

impl State {
    fn wire(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Closed => "closed",
            State::Merged => "merged",
        }
    }
    fn parse(text: &str) -> State {
        match text {
            "merged" => State::Merged,
            "closed" => State::Closed,
            _ => State::Open,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: Kind,
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: State,
    pub is_draft: bool,
    pub author: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub head_ref: Option<String>,
    pub base_ref: Option<String>,
    pub additions: u64,
    pub deletions: u64,
    pub mergeable: Option<String>,
    pub labels: Vec<String>,
    pub comment_count: u64,
    pub assignees: Vec<String>,
    pub review_requests: Vec<String>,
    pub checks: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inbox {
    pub viewer: Option<String>,
    /// Each repository with the ids of the saved projects that own it.
    pub repositories: Vec<(String, Vec<String>)>,
    pub items: Vec<Item>,
    /// Repositories GitHub could not be asked about, with the lookup status.
    pub failures: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub name: String,
    pub status: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    pub author: Option<String>,
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub author: Option<String>,
    pub body: String,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    pub oid: String,
    pub headline: String,
    pub committed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileStat {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    pub item: Item,
    pub body: String,
    pub head_oid: Option<String>,
    pub merge_state_status: Option<String>,
    pub changed_files: u64,
    pub merge_methods: Vec<String>,
    pub checks: Vec<Check>,
    pub reviewers: Vec<String>,
    pub reviews: Vec<Review>,
    pub comments: Vec<Comment>,
    pub commits: Vec<Commit>,
    pub files: Vec<FileStat>,
    pub files_truncated: bool,
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

fn opt(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}

fn number(value: &Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or(0)
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|entry| entry.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn list<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value[key].as_array().into_iter().flatten()
}

pub fn parse_item(value: &Value) -> Option<Item> {
    let repository = value["repository"].as_str()?.to_owned();
    Some(Item {
        kind: Kind::parse(value["kind"].as_str().unwrap_or("pullRequest")),
        number: value["number"].as_u64()?,
        title: text(value, "title"),
        url: text(value, "url"),
        state: State::parse(value["state"].as_str().unwrap_or("open")),
        is_draft: value["isDraft"].as_bool().unwrap_or(false),
        author: opt(value, "author"),
        created_at: text(value, "createdAt"),
        updated_at: text(value, "updatedAt"),
        head_ref: opt(value, "headRef"),
        base_ref: opt(value, "baseRef"),
        additions: number(value, "additions"),
        deletions: number(value, "deletions"),
        mergeable: opt(value, "mergeable"),
        labels: list(value, "labels")
            .filter_map(|label| label["name"].as_str().map(str::to_owned))
            .collect(),
        comment_count: number(value, "commentCount"),
        assignees: strings(value, "assignees"),
        review_requests: strings(value, "reviewRequests"),
        checks: opt(value, "checks"),
        repository,
    })
}

pub fn parse_inbox(value: &Value) -> Inbox {
    Inbox {
        viewer: opt(value, "viewer"),
        repositories: list(value, "repositories")
            .filter_map(|entry| {
                Some((
                    entry["repository"].as_str()?.to_owned(),
                    strings(entry, "projectIds"),
                ))
            })
            .collect(),
        items: list(value, "items").filter_map(parse_item).collect(),
        failures: list(value, "failures")
            .filter_map(|entry| {
                Some((
                    entry["repository"].as_str()?.to_owned(),
                    entry["status"].as_str().unwrap_or("unavailable").to_owned(),
                ))
            })
            .collect(),
    }
}

pub fn parse_detail(value: &Value) -> Option<Detail> {
    Some(Detail {
        item: parse_item(&value["item"])?,
        body: text(value, "body"),
        head_oid: opt(value, "headOid"),
        merge_state_status: opt(value, "mergeStateStatus"),
        changed_files: number(value, "changedFiles"),
        merge_methods: strings(value, "mergeMethods"),
        checks: list(value, "checks")
            .map(|check| Check {
                name: text(check, "name"),
                status: check["status"].as_str().unwrap_or("unknown").to_owned(),
                url: opt(check, "url"),
            })
            .collect(),
        reviewers: strings(value, "reviewers"),
        reviews: list(value, "reviews")
            .map(|review| Review {
                author: opt(review, "author"),
                state: text(review, "state"),
                body: text(review, "body"),
                submitted_at: opt(review, "submittedAt"),
            })
            .collect(),
        comments: list(value, "comments")
            .map(|comment| Comment {
                author: opt(comment, "author"),
                body: text(comment, "body"),
                created_at: text(comment, "createdAt"),
            })
            .collect(),
        commits: list(value, "commits")
            .map(|commit| Commit {
                oid: text(commit, "oid"),
                headline: text(commit, "headline"),
                committed_at: opt(commit, "committedAt"),
            })
            .collect(),
        files: list(value, "files")
            .map(|file| FileStat {
                path: text(file, "path"),
                additions: number(file, "additions"),
                deletions: number(file, "deletions"),
            })
            .collect(),
        files_truncated: value["filesTruncated"].as_bool().unwrap_or(false),
    })
}

// ---------------------------------------------------------------------------------------------
// List logic
// ---------------------------------------------------------------------------------------------

/// `owner/repo#n`, the key an item goes by.
pub fn item_key(item: &Item) -> String {
    format!("{}#{}", item.repository, item.number)
}

fn same(a: Option<&str>, b: Option<&str>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b))
}

/// The four segments of the list column, in the order they are drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Segment {
    Open,
    Merged,
    Closed,
    Issues,
}

impl Segment {
    const ALL: [Segment; 4] = [
        Segment::Open,
        Segment::Merged,
        Segment::Closed,
        Segment::Issues,
    ];
    fn label(self) -> &'static str {
        match self {
            Segment::Open => t("Abertos"),
            Segment::Merged => t("Mesclados"),
            Segment::Closed => t("Fechados"),
            Segment::Issues => t("Issues"),
        }
    }
    /// What the host is asked for: the first three list pull requests in that state, "Issues"
    /// lists the open issues.
    pub fn list(self) -> (Kind, State) {
        match self {
            Segment::Open => (Kind::Pull, State::Open),
            Segment::Merged => (Kind::Pull, State::Merged),
            Segment::Closed => (Kind::Pull, State::Closed),
            Segment::Issues => (Kind::Issue, State::Open),
        }
    }
    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
    /// The plain message of an empty list.
    fn empty(self) -> &'static str {
        match self {
            Segment::Open => t("Nenhum pull request aberto."),
            Segment::Merged => t("Nenhum pull request mesclado."),
            Segment::Closed => t("Nenhum pull request fechado."),
            Segment::Issues => t("Nenhuma issue aberta."),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub repository: String,
    pub number: u64,
    pub kind: Option<Kind>,
}

fn valid_repository(owner: &str, repo: &str) -> bool {
    let owner_ok = !owner.is_empty()
        && owner.len() <= 100
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let repo_ok = !repo.is_empty()
        && repo.len() <= 100
        && repo
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    owner_ok && repo_ok
}

/// A pasted GitHub link or `owner/repo#n`.
pub fn parse_reference(input: &str) -> Option<Reference> {
    let value = input.trim();
    if let Some(rest) = value.strip_prefix("https://github.com/") {
        let mut parts = rest.splitn(4, '/');
        let (owner, repo, what, tail) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        let kind = match what {
            "pull" => Kind::Pull,
            "issues" => Kind::Issue,
            _ => return None,
        };
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        let after = &tail[digits.len()..];
        if digits.is_empty()
            || digits.len() > 8
            || !(after.is_empty() || after.starts_with(['/', '?', '#']))
            || !valid_repository(owner, repo)
        {
            return None;
        }
        return Some(Reference {
            repository: format!("{owner}/{repo}"),
            number: digits.parse().ok()?,
            kind: Some(kind),
        });
    }
    let (repository, digits) = value.split_once('#')?;
    let (owner, repo) = repository.split_once('/')?;
    if digits.is_empty()
        || digits.len() > 8
        || !digits.chars().all(|c| c.is_ascii_digit())
        || !valid_repository(owner, repo)
    {
        return None;
    }
    Some(Reference {
        repository: repository.to_owned(),
        number: digits.parse().ok()?,
        kind: None,
    })
}

/// The GitHub repositories of one project (the inbox maps each repository to project ids).
pub fn project_repositories<'a>(inbox: &'a Inbox, project: Option<&str>) -> Vec<&'a str> {
    let Some(project) = project else {
        return Vec::new();
    };
    inbox
        .repositories
        .iter()
        .filter(|(_, ids)| ids.iter().any(|id| id == project))
        .map(|(repository, _)| repository.as_str())
        .collect()
}

/// The project's own items matching the search box (a pasted link finds that item), newest
/// updated first. The project scope comes first, so nothing of another repository is ever
/// matched, not even by a pasted link.
pub fn visible_items(inbox: &Inbox, project: Option<&str>, query: &str) -> Vec<Item> {
    let repositories = project_repositories(inbox, project);
    let reference = parse_reference(query);
    let needle = query.trim().to_lowercase();
    let mut items: Vec<Item> = inbox
        .items
        .iter()
        .filter(|item| {
            repositories
                .iter()
                .any(|repository| same(Some(repository), Some(&item.repository)))
        })
        .filter(|item| {
            if let Some(reference) = &reference {
                return same(Some(&item.repository), Some(&reference.repository))
                    && item.number == reference.number;
            }
            if needle.is_empty() {
                return true;
            }
            format!(
                "{} #{} {} {} {}",
                item.title,
                item.number,
                item.repository,
                item.author.as_deref().unwrap_or_default(),
                item.head_ref.as_deref().unwrap_or_default()
            )
            .to_lowercase()
            .contains(&needle)
        })
        .cloned()
        .collect();
    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    items
}

/// A pasted link or `owner/repo#n` that points outside the project's repositories, which the
/// page refuses to open.
pub fn foreign_reference(inbox: &Inbox, project: Option<&str>, query: &str) -> Option<Reference> {
    let reference = parse_reference(query)?;
    let repositories = project_repositories(inbox, project);
    (!repositories
        .iter()
        .any(|repository| same(Some(repository), Some(&reference.repository))))
    .then_some(reference)
}

/// Why GitHub would refuse a merge now, or None when it can be tried.
pub fn merge_blocker(detail: &Detail) -> Option<&'static str> {
    let item = &detail.item;
    if item.state != State::Open {
        return None;
    }
    if item.is_draft {
        return Some(t("É rascunho: marque como pronto antes de mesclar."));
    }
    let status = detail.merge_state_status.as_deref();
    if item.mergeable.as_deref() == Some("CONFLICTING") || status == Some("DIRTY") {
        return Some(t("Há conflitos com o branch base."));
    }
    if status == Some("BLOCKED") {
        return Some(t("As regras do repositório impedem a mesclagem."));
    }
    if status == Some("BEHIND") {
        return Some(t("O branch está desatualizado em relação ao base."));
    }
    if detail.head_oid.is_none() || detail.merge_methods.is_empty() {
        return Some(t("A mesclagem não está disponível agora."));
    }
    None
}

/// One file of a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
}

/// Splits a unified diff into its files (`diff --git a/x b/y`) with their line counts.
pub fn split_diff(diff: &str) -> Vec<DiffFile> {
    let mut files: Vec<DiffFile> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            let path = rest.split_once(" b/").map_or(rest, |(_, b)| b);
            files.push(DiffFile {
                path: path.to_owned(),
                additions: 0,
                deletions: 0,
            });
        } else if let Some(file) = files.last_mut() {
            if line.starts_with('+') && !line.starts_with("+++") {
                file.additions += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                file.deletions += 1;
            }
        }
    }
    files
}

/// One line of the Atividade tab.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Commit(Commit),
    Review(Review),
    Comment(Comment),
}

impl Event {
    fn at(&self) -> &str {
        match self {
            Event::Commit(c) => c.committed_at.as_deref().unwrap_or_default(),
            Event::Review(r) => r.submitted_at.as_deref().unwrap_or_default(),
            Event::Comment(c) => &c.created_at,
        }
    }
}

/// Commits, reviews and comments in the order they happened (undated ones first).
pub fn timeline(detail: &Detail) -> Vec<Event> {
    let mut events: Vec<Event> = detail
        .commits
        .iter()
        .cloned()
        .map(Event::Commit)
        .chain(detail.reviews.iter().cloned().map(Event::Review))
        .chain(detail.comments.iter().cloned().map(Event::Comment))
        .collect();
    events.sort_by(|a, b| a.at().cmp(b.at()));
    events
}

/// The latest non-comment review state of each reviewer, in first-seen order.
pub fn latest_reviews(reviews: &[Review]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for review in reviews {
        let Some(author) = &review.author else {
            continue;
        };
        if review.state == "COMMENTED" {
            continue;
        }
        match out.iter_mut().find(|(name, _)| name == author) {
            Some(entry) => entry.1 = review.state.clone(),
            None => out.push((author.clone(), review.state.clone())),
        }
    }
    out
}

/// What `pull_ask_action` takes for an item (its state as the Ask conversation names it).
pub fn ask_action(detail: &Detail) -> Value {
    let item = &detail.item;
    let state = match (item.state, item.is_draft) {
        (State::Open, true) => "draft",
        (State::Open, false) => "open",
        (State::Closed, _) => "closed",
        (State::Merged, _) => "merged",
    };
    json!({ "action": {
        "type": "open",
        "repository": item.repository,
        "number": item.number,
        "kind": item.kind.wire(),
        "context": {
            "title": item.title,
            "state": state,
            "author": item.author,
            "headRef": item.head_ref,
            "baseRef": item.base_ref,
            "description": detail.body,
        },
    } })
}

// ---------------------------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------------------------

/// What the page asks of the window.
pub enum PullsEvent {
    /// `pull_ask_action`'s arguments; the shell sends them and opens the conversation it returns.
    Ask(Value),
}

impl EventEmitter<PullsEvent> for PullRequestsPage {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Summary,
    Activity,
    Checks,
    Code,
}

#[derive(Default)]
struct Load {
    inbox: Option<Inbox>,
    loading: bool,
    error: Option<String>,
}

enum DiffState {
    Idle,
    Loading,
    Ready {
        lines: Rc<Vec<String>>,
        files: Vec<DiffFile>,
        truncated: bool,
    },
    Failed(String),
}

struct Opened {
    key: String,
    item: Item,
    detail: Option<Detail>,
    error: Option<String>,
    tab: Tab,
    diff: DiffState,
    method: Option<String>,
    busy: bool,
    notice: Option<(String, bool)>,
}

#[derive(Clone)]
enum Pending {
    Merge(String),
    SetDraft(bool),
    SetOpen(bool),
}

pub struct PullRequestsPage {
    backend: Backend,
    segment: Segment,
    /// The segment's highlight, as a position in `Segment::ALL`.
    glide: Tween,
    /// The saved project selected in the sidebar; the page shows only its repository.
    project: Option<String>,
    query: Entity<TextField>,
    comment: Entity<TextField>,
    lists: HashMap<(Kind, State), Load>,
    opened: Option<Opened>,
    _subscriptions: Vec<Subscription>,
}

/// Resolves a host request: the value, or the error's message.
async fn settle(rx: async_channel::Receiver<Result<Value, Value>>) -> Result<Value, String> {
    match rx.recv().await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error
            .as_str()
            .or(error["message"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| error.to_string())),
        Err(_) => Err(t("O motor do Sirus não respondeu.").into()),
    }
}

const GREEN: u32 = 0x3fb950;
const PURPLE: u32 = 0xa371f7;
const RED: u32 = 0xf85149;
const AMBER: u32 = 0xd29922;
const LIST_WIDTH: f32 = 340.;
/// Four segments share the column: its width less the padding (10 each side), the track's border
/// and padding (4 each side) the column's right border (1) and the three gaps of 2.
const SEGMENT_WIDTH: f32 = (LIST_WIDTH - 1. - 20. - 8. - 6.) / 4.;

impl PullRequestsPage {
    pub fn new(backend: Backend, project: Option<String>, cx: &mut Context<Self>) -> Self {
        let query =
            cx.new(|cx| TextField::new(t("Buscar por título, repositório, autor ou #"), cx));
        let comment = cx.new(|cx| TextField::new(t("Escreva um comentário…"), cx));
        let subscriptions = vec![
            cx.subscribe(&query, |_, _, event: &TextFieldEvent, cx| {
                if matches!(event, TextFieldEvent::Changed) {
                    cx.notify();
                }
            }),
            cx.subscribe(
                &comment,
                |this, _, event: &TextFieldEvent, cx| match event {
                    TextFieldEvent::Submit => this.send_comment(cx),
                    _ => cx.notify(),
                },
            ),
        ];
        let mut page = Self {
            backend,
            segment: Segment::Open,
            glide: Tween::still(0., Instant::now()),
            project,
            query,
            comment,
            lists: HashMap::new(),
            opened: None,
            _subscriptions: subscriptions,
        };
        page.reload(cx);
        page
    }

    /// Reloads the list of the shown segment.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let (kind, state) = self.segment.list();
        self.load_list(kind, state, cx);
    }

    /// Follows the sidebar's project: another one closes the opened item and reads the list again.
    pub fn set_project(&mut self, project: Option<String>, cx: &mut Context<Self>) {
        if self.project == project {
            return;
        }
        self.project = project;
        self.opened = None;
        self.reload(cx);
        cx.notify();
    }

    fn set_segment(&mut self, segment: Segment, cx: &mut Context<Self>) {
        if self.segment == segment {
            return;
        }
        self.segment = segment;
        self.opened = None;
        self.glide.set(
            segment.index() as f32,
            Instant::now(),
            if cx.reduce_motion() { 0. } else { 0.32 },
            Curve::Expo,
        );
        self.reload(cx);
        cx.notify();
    }

    fn load_list(&mut self, kind: Kind, state: State, cx: &mut Context<Self>) {
        let load = self.lists.entry((kind, state)).or_default();
        if load.loading {
            return;
        }
        load.loading = true;
        load.error = None;
        let rx = self.backend.request(
            "pull_request_action",
            json!({ "action": { "type": "list", "kind": kind.wire(), "state": state.wire() } }),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                let load = this.lists.entry((kind, state)).or_default();
                load.loading = false;
                match reply {
                    Ok(value) => load.inbox = Some(parse_inbox(&value)),
                    Err(error) => load.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// What the shown segment's last read returned.
    fn inbox(&self) -> Option<&Inbox> {
        self.lists
            .get(&self.segment.list())
            .and_then(|load| load.inbox.as_ref())
    }

    fn loading(&self) -> bool {
        self.lists
            .get(&self.segment.list())
            .is_some_and(|load| load.loading)
    }

    fn list_error(&self) -> Option<String> {
        self.lists
            .get(&self.segment.list())
            .and_then(|load| load.error.clone())
    }

    /// Items to draw: the project's, filtered by the search, newest updated first.
    fn visible(&self, inbox: &Inbox, cx: &App) -> Vec<Item> {
        visible_items(inbox, self.project.as_deref(), self.query.read(cx).text())
    }

    fn select(&mut self, item: Item, cx: &mut Context<Self>) {
        let key = item_key(&item);
        if self.opened.as_ref().is_some_and(|o| o.key == key) {
            return;
        }
        self.comment.update(cx, |field, cx| field.set_text("", cx));
        self.opened = Some(Opened {
            key,
            item,
            detail: None,
            error: None,
            tab: Tab::Summary,
            diff: DiffState::Idle,
            method: None,
            busy: false,
            notice: None,
        });
        self.load_detail(cx);
        cx.notify();
    }

    fn load_detail(&mut self, cx: &mut Context<Self>) {
        let Some(opened) = self.opened.as_mut() else {
            return;
        };
        opened.error = None;
        let (key, item) = (opened.key.clone(), opened.item.clone());
        let rx = self.backend.request(
            "pull_request_action",
            json!({ "action": { "type": "detail", "repository": item.repository,
                "number": item.number, "kind": item.kind.wire() } }),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                let Some(opened) = this.opened.as_mut().filter(|o| o.key == key) else {
                    return;
                };
                match reply {
                    Ok(value) => match parse_detail(&value) {
                        Some(detail) => opened.detail = Some(detail),
                        None => opened.error = Some(t("Resposta inesperada do GitHub.").into()),
                    },
                    Err(error) => opened.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let Some(opened) = self.opened.as_mut() else {
            return;
        };
        opened.tab = tab;
        if tab == Tab::Code && matches!(opened.diff, DiffState::Idle) {
            opened.diff = DiffState::Loading;
            let (key, item) = (opened.key.clone(), opened.item.clone());
            let rx = self.backend.request(
                "pull_request_action",
                json!({ "action": { "type": "diff", "repository": item.repository,
                    "number": item.number } }),
            );
            cx.spawn(async move |this, cx| {
                let reply = settle(rx).await;
                let _ = this.update(cx, |this, cx| {
                    let Some(opened) = this.opened.as_mut().filter(|o| o.key == key) else {
                        return;
                    };
                    opened.diff = match reply {
                        Ok(value) => {
                            let diff = value["text"].as_str().unwrap_or_default();
                            DiffState::Ready {
                                lines: Rc::new(diff.lines().map(str::to_owned).collect()),
                                files: split_diff(diff),
                                truncated: value["truncated"].as_bool().unwrap_or(false),
                            }
                        }
                        Err(error) => DiffState::Failed(error),
                    };
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Runs a change on the opened item, then reads it and the lists again.
    fn change(&mut self, action: Value, done: &'static str, cx: &mut Context<Self>) {
        let Some(opened) = self.opened.as_mut() else {
            return;
        };
        if opened.busy {
            return;
        }
        opened.busy = true;
        opened.notice = None;
        let key = opened.key.clone();
        let rx = self
            .backend
            .request("pull_request_action", json!({ "action": action }));
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                let Some(opened) = this.opened.as_mut().filter(|o| o.key == key) else {
                    return;
                };
                opened.busy = false;
                match reply {
                    Ok(_) => {
                        opened.notice = Some((done.to_owned(), false));
                        this.comment.update(cx, |field, cx| field.set_text("", cx));
                        this.load_detail(cx);
                        this.reload(cx);
                    }
                    Err(error) => opened.notice = Some((error, true)),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn run(&mut self, pending: Pending, cx: &mut Context<Self>) {
        let Some(opened) = self.opened.as_ref() else {
            return;
        };
        let (item, head) = (
            opened.item.clone(),
            opened.detail.as_ref().and_then(|d| d.head_oid.clone()),
        );
        let base = json!({ "repository": item.repository, "number": item.number, "confirm": true });
        let mut action = base;
        match pending {
            Pending::Merge(method) => {
                action["type"] = json!("merge");
                action["method"] = json!(method);
                action["expectedHead"] = json!(head.unwrap_or_default());
                self.change(action, t("Pull request mesclado."), cx);
            }
            Pending::SetDraft(draft) => {
                action["type"] = json!("setDraft");
                action["draft"] = json!(draft);
                self.change(action, t("Estado atualizado."), cx);
            }
            Pending::SetOpen(open) => {
                action["type"] = json!("setOpen");
                action["kind"] = json!(item.kind.wire());
                action["open"] = json!(open);
                self.change(action, t("Estado atualizado."), cx);
            }
        }
    }

    fn send_comment(&mut self, cx: &mut Context<Self>) {
        let body = self.comment.read(cx).text().trim().to_owned();
        let Some(opened) = self.opened.as_ref() else {
            return;
        };
        if body.is_empty() {
            return;
        }
        let action = json!({ "type": "comment", "repository": opened.item.repository,
            "number": opened.item.number, "body": body, "confirm": true });
        self.change(action, t("Comentário enviado."), cx);
    }

    /// Asks in the window's popup before changing something on GitHub.
    fn confirm(
        &mut self,
        pending: Pending,
        title: String,
        message: String,
        label: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = cx.entity();
        if let Some(Some(shell)) = window.root::<crate::shell::Shell>() {
            shell.update(cx, |shell, cx| {
                shell.confirm(
                    title,
                    message,
                    label,
                    move |cx: &mut App| page.update(cx, |this, cx| this.run(pending, cx)),
                    cx,
                );
            });
        }
    }

    fn ask(&mut self, cx: &mut Context<Self>) {
        if let Some(detail) = self.opened.as_ref().and_then(|o| o.detail.as_ref()) {
            cx.emit(PullsEvent::Ask(ask_action(detail)));
        }
    }

    /// The saved projects that own the opened item's repository.
    fn owned(&self) -> bool {
        let (Some(opened), Some(inbox)) = (self.opened.as_ref(), self.inbox()) else {
            return false;
        };
        inbox.repositories.iter().any(|(repository, projects)| {
            repository.eq_ignore_ascii_case(&opened.item.repository) && !projects.is_empty()
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------------------------

fn state_name(item: &Item) -> &'static str {
    match (item.state, item.is_draft) {
        (State::Open, true) => t("Rascunho"),
        (State::Open, false) => t("Aberto"),
        (State::Closed, _) => t("Fechado"),
        (State::Merged, _) => t("Mesclado"),
    }
}

fn state_glyph(item: &Item, side: f32) -> AnyElement {
    let (path, color) = match (item.kind, item.state, item.is_draft) {
        (Kind::Issue, State::Open, _) => ("circle-dot", rgb(GREEN).into()),
        (Kind::Issue, _, _) => ("circle-dot", rgb(PURPLE).into()),
        (Kind::Pull, State::Merged, _) => ("git-merge", rgb(PURPLE).into()),
        (Kind::Pull, State::Closed, _) => ("git-pull-request", rgb(RED).into()),
        (Kind::Pull, State::Open, true) => ("git-pull-request", content(0.45)),
        (Kind::Pull, State::Open, false) => ("git-pull-request", rgb(GREEN).into()),
    };
    icon(path, side).text_color(color).into_any_element()
}

fn check_color(status: &str) -> gpui::Hsla {
    match status.to_lowercase().as_str() {
        "success" | "passed" => rgb(GREEN).into(),
        "failure" | "error" | "failed" => rgb(RED).into(),
        "skipped" | "unknown" => content(0.3),
        _ => rgb(AMBER).into(),
    }
}

fn method_label(method: &str) -> &'static str {
    match method {
        "merge" => t("Criar merge commit"),
        "rebase" => t("Rebase e mesclar"),
        _ => t("Squash e mesclar"),
    }
}

fn pill(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.))
        .px(px(9.))
        .rounded(px(RADIUS_SM))
        .flex()
        .items_center()
        .flex_shrink_0()
        .text_size(px(11.5))
        .cursor_pointer()
        .bg(content(if on { 0.12 } else { 0.0 }))
        .text_color(content(if on { 0.95 } else { 0.55 }))
        .hover(|s| s.bg(content(0.09)).text_color(content(0.95)))
        .child(label.into())
}

/// A button of the detail header; `primary` is the accent one, a disabled one takes no click.
fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
) -> Stateful<Div> {
    let base = div()
        .id(id)
        .h(px(28.))
        .px(px(11.))
        .rounded(px(RADIUS_MD))
        .flex()
        .items_center()
        .flex_shrink_0()
        .text_size(px(12.))
        .child(label.into());
    let base = if primary {
        base.bg(accent()).text_color(on_accent())
    } else {
        base.bg(content(0.07)).text_color(content(0.85))
    };
    if enabled {
        base.cursor_pointer().sinks().hover(|s| s.opacity(0.88))
    } else {
        base.opacity(0.4)
    }
}

fn card() -> Div {
    div()
        .rounded(px(RADIUS_MD))
        .border_1()
        .border_color(crate::theme::border())
        .px(px(12.))
        .py(px(8.))
}

fn heading(label: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(content(0.45))
        .mb(px(6.))
        .child(label.into())
}

impl PullRequestsPage {
    fn header(&self, loading: bool, cx: &mut Context<Self>) -> AnyElement {
        let icon_button = |id: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .size(px(26.))
                .rounded(px(RADIUS_SM))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
                .child(icon(glyph, 14.).text_color(content(0.6)))
        };
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(content(0.92))
                    .child(t("Pull requests")),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(content(0.4))
                    .child(if loading { t("Atualizando…") } else { "" }),
            )
            .child(
                icon_button("pulls-refresh", "refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .into_any_element()
    }

    /// The four segments in one row, with a highlight that slides to the chosen one.
    fn controls(&self, now: Instant, moving: &mut bool, cx: &mut Context<Self>) -> AnyElement {
        let glide = self.glide.at(now);
        if !self.glide.done(now) {
            *moving = true;
        }
        let segments = Segment::ALL.into_iter().enumerate().map(|(ix, segment)| {
            div()
                .id(("pulls-segment", ix))
                .w(px(SEGMENT_WIDTH))
                .h(px(24.))
                .rounded(px(RADIUS_SM))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.5))
                .cursor_pointer()
                .text_color(content(if self.segment == segment { 0.95 } else { 0.55 }))
                .hover(|s| s.text_color(content(0.95)))
                .on_click(cx.listener(move |this, _, _, cx| this.set_segment(segment, cx)))
                .child(segment.label())
        });
        div()
            .relative()
            .p(px(3.))
            .flex()
            .gap(px(2.))
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(crate::theme::border())
            .bg(content(0.03))
            .child(
                div()
                    .absolute()
                    .top(px(3.))
                    .bottom(px(3.))
                    .left(px(3. + glide * (SEGMENT_WIDTH + 2.)))
                    .w(px(SEGMENT_WIDTH))
                    .rounded(px(RADIUS_SM))
                    .bg(content(0.12)),
            )
            .children(segments)
            .into_any_element()
    }

    fn row(&self, ix: usize, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let active = self
            .opened
            .as_ref()
            .is_some_and(|o| o.key == item_key(item));
        let repo = item
            .repository
            .split('/')
            .nth(1)
            .unwrap_or(&item.repository);
        let mut meta = format!("{repo} #{}", item.number);
        if let Some(author) = &item.author {
            meta.push_str(&format!(" · {author}"));
        }
        let when = age(&item.updated_at);
        if !when.is_empty() {
            meta.push_str(&format!(" · {when}"));
        }
        let picked = item.clone();
        div()
            .id(("pulls-row", ix))
            .w_full()
            .px(px(10.))
            .py(px(7.))
            .rounded(px(RADIUS_MD))
            .flex()
            .gap(px(9.))
            .cursor_pointer()
            .bg(content(if active { 0.09 } else { 0.0 }))
            .hover(|s| s.bg(content(0.06)))
            .on_click(cx.listener(move |this, _, _, cx| this.select(picked.clone(), cx)))
            .child(div().pt(px(2.)).child(state_glyph(item, 14.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(content(0.92))
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(div().min_w_0().truncate().child(meta))
                            .children(item.checks.as_deref().map(|checks| {
                                div()
                                    .size(px(6.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(check_color(checks))
                            }))
                            .when(item.comment_count > 0, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(2.))
                                        .flex_shrink_0()
                                        .child(icon("message-square", 12.).text_color(content(0.4)))
                                        .child(item.comment_count.to_string()),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn list_column(
        &mut self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let inbox = self.inbox().cloned();
        let items = match &inbox {
            Some(inbox) => self.visible(inbox, cx),
            None => Vec::new(),
        };
        let mut body: Vec<AnyElement> = Vec::new();
        if let Some(error) = self.list_error() {
            body.push(note(&error, true));
        }
        if let Some(inbox) = &inbox {
            let repositories = project_repositories(inbox, self.project.as_deref());
            let sign_in = inbox
                .failures
                .iter()
                .find(|f| f.1 == "cliMissing" || f.1 == "authRequired");
            if let Some((_, status)) = sign_in {
                body.push(note(
                    if status == "cliMissing" {
                        t("Instale o GitHub CLI (gh) para listar pull requests.")
                    } else {
                        t("Entre no GitHub CLI em Configurações → Git.")
                    },
                    false,
                ));
            }
            let query = self.query.read(cx).text().to_owned();
            if repositories.is_empty() {
                body.push(note(
                    t("Este projeto não tem um repositório no GitHub."),
                    false,
                ));
            } else if foreign_reference(inbox, self.project.as_deref(), &query).is_some() {
                body.push(note(
                    t("Este link é de outro repositório. Abra-o no projeto dele."),
                    false,
                ));
            } else if items.is_empty() && sign_in.is_none() {
                body.push(note(
                    if query.trim().is_empty() {
                        self.segment.empty()
                    } else {
                        t("Nada por aqui.")
                    },
                    false,
                ));
            }
            for (ix, item) in items.iter().enumerate() {
                body.push(self.row(ix, item, cx));
            }
            let unavailable = inbox
                .failures
                .iter()
                .filter(|f| f.1 != "cliMissing" && f.1 != "authRequired")
                .filter(|f| repositories.iter().any(|r| same(Some(r), Some(&f.0))))
                .count();
            if unavailable > 0 {
                body.push(note(
                    &if unavailable == 1 {
                        t("1 repositório indisponível.").to_owned()
                    } else {
                        tf("{} repositórios indisponíveis.", &[&unavailable])
                    },
                    false,
                ));
            }
        } else if self.loading() {
            body.push(note(t("Carregando…"), false));
        }
        div()
            .w(px(LIST_WIDTH))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(content(0.07))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(10.))
                    .child(self.header(self.loading(), cx))
                    .child(self.query.clone())
                    .child(self.controls(now, moving, cx)),
            )
            .child(
                div()
                    .id("pulls-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(4.))
                    .pb(px(8.))
                    .children(body),
            )
            .into_any_element()
    }

    fn detail_column(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(opened) = self.opened.as_ref() else {
            return div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .child(icon("git-pull-request", 24.).text_color(content(0.3)))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(content(0.8))
                        .child(t("Escolha um item")),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.4))
                        .child(t("Selecione um pull request ou issue na lista.")),
                )
                .into_any_element();
        };
        let view = opened
            .detail
            .as_ref()
            .map_or(&opened.item, |d| &d.item)
            .clone();
        let is_pull = view.kind == Kind::Pull;
        let detail = opened.detail.clone();
        let chosen = detail
            .as_ref()
            .map(|d| {
                opened
                    .method
                    .clone()
                    .filter(|m| d.merge_methods.contains(m))
                    .or_else(|| d.merge_methods.first().cloned())
                    .unwrap_or_else(|| "squash".into())
            })
            .unwrap_or_else(|| "squash".into());
        let blocker = detail.as_ref().and_then(merge_blocker);
        let open_pull = is_pull && view.state == State::Open;
        let ready = detail.is_some() && !opened.busy;
        let ref_name = format!("{}#{}", view.repository, view.number);

        let mut meta = vec![view.repository.clone()];
        if let Some(author) = &view.author {
            meta.push(tf("por {}", &[author]));
        }
        if let (Some(head), Some(base)) = (&view.head_ref, &view.base_ref) {
            meta.push(format!("{head} → {base}"));
        }
        meta.push(state_name(&view).to_owned());
        let (added, removed) = diff_colors(0);

        let mut actions = div().flex().flex_wrap().items_center().gap(px(6.));
        if open_pull && view.is_draft {
            let ref_name = ref_name.clone();
            actions = actions.child(
                button("pulls-ready", t("Pronto para revisão"), false, ready).when(ready, |b| {
                    b.on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm(
                            Pending::SetDraft(false),
                            tf("Marcar {} como pronto para revisão?", &[&ref_name]),
                            t("Revisores poderão ser notificados.").into(),
                            t("Marcar como pronto"),
                            window,
                            cx,
                        )
                    }))
                }),
            );
        } else if open_pull {
            let enabled = ready && blocker.is_none();
            let (method, ref_name, base) = (
                chosen.clone(),
                ref_name.clone(),
                view.base_ref.clone().unwrap_or_default(),
            );
            actions = actions.child(
                button("pulls-merge", method_label(&chosen), true, enabled).when(enabled, |b| {
                    b.on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm(
                            Pending::Merge(method.clone()),
                            tf("Mesclar {}?", &[&ref_name]),
                            tf(
                                "A mesclagem usa \"{}\" e entra em {}. Isso muda o repositório no GitHub.",
                                &[&method_label(&method), &base],
                            ),
                            t("Mesclar"),
                            window,
                            cx,
                        )
                    }))
                }),
            );
            if detail.as_ref().is_some_and(|d| d.merge_methods.len() > 1) {
                let methods = detail
                    .as_ref()
                    .map(|d| d.merge_methods.clone())
                    .unwrap_or_default();
                let current = chosen.clone();
                actions = actions.child(pill("pulls-method", t("Trocar método"), false).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let at = methods.iter().position(|m| *m == current).unwrap_or(0);
                        if let Some(opened) = this.opened.as_mut() {
                            opened.method = methods.get((at + 1) % methods.len()).cloned();
                        }
                        cx.notify();
                    }),
                ));
            }
        }
        let can_ask = ready && self.owned();
        actions = actions.child(div().flex_1()).child(
            button("pulls-ask", t("Perguntar"), false, can_ask).when(can_ask, |b| {
                b.on_click(cx.listener(|this, _, _, cx| this.ask(cx)))
            }),
        );
        let url = crate::markdown::web_url(&view.url);
        actions = actions
            .child(
                button("pulls-github", t("Abrir no GitHub"), false, url.is_some())
                    .when_some(url.clone(), |b, url| {
                        b.on_click(move |_, _, cx| cx.open_url(&url))
                    }),
            )
            .child(
                button("pulls-copy", t("Copiar link"), false, true).on_click({
                    let link = view.url.clone();
                    cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(link.clone()));
                        if let Some(opened) = this.opened.as_mut() {
                            opened.notice = Some((t("Link copiado.").to_owned(), false));
                        }
                        cx.notify();
                    })
                }),
            );
        if open_pull && !view.is_draft {
            let ref_name = ref_name.clone();
            actions = actions.child(
                button("pulls-draft", t("Converter em rascunho"), false, ready).when(ready, |b| {
                    b.on_click(cx.listener(move |this, _, window, cx| {
                        this.confirm(
                            Pending::SetDraft(true),
                            tf("Converter {} em rascunho?", &[&ref_name]),
                            t("Ele deixa de poder ser mesclado até ficar pronto de novo.").into(),
                            t("Converter"),
                            window,
                            cx,
                        )
                    }))
                }),
            );
        }
        if view.state == State::Open || view.state == State::Closed {
            let opening = view.state == State::Closed;
            let ref_name = ref_name.clone();
            actions = actions.child(
                button(
                    "pulls-toggle-open",
                    if opening { t("Reabrir") } else { t("Fechar") },
                    false,
                    ready,
                )
                .when(ready, |b| {
                    b.on_click(cx.listener(move |this, _, window, cx| {
                        let (title, message, label) = if opening {
                            (
                                tf("Reabrir {}?", &[&ref_name]),
                                t("Ele volta para a lista de abertos."),
                                t("Reabrir"),
                            )
                        } else {
                            (
                                tf("Fechar {}?", &[&ref_name]),
                                t("Ele sai da lista de abertos. Dá para reabrir depois."),
                                t("Fechar"),
                            )
                        };
                        this.confirm(
                            Pending::SetOpen(opening),
                            title,
                            message.into(),
                            label,
                            window,
                            cx,
                        )
                    }))
                }),
            );
        }

        let tabs: Vec<(Tab, &'static str)> = if is_pull {
            let mut tabs = vec![(Tab::Summary, t("Resumo")), (Tab::Activity, t("Atividade"))];
            if detail.as_ref().is_some_and(|d| !d.checks.is_empty()) {
                tabs.push((Tab::Checks, t("Checks")));
            }
            tabs.push((Tab::Code, t("Código")));
            tabs
        } else {
            vec![(Tab::Summary, t("Resumo")), (Tab::Activity, t("Atividade"))]
        };
        let shown = if opened.tab == Tab::Checks && !tabs.iter().any(|(tab, _)| *tab == Tab::Checks)
        {
            Tab::Summary
        } else {
            opened.tab
        };
        let tab_row = div()
            .flex()
            .gap(px(2.))
            .children(tabs.into_iter().enumerate().map(|(ix, (tab, label))| {
                pill(("pulls-tab", ix), label, shown == tab)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
            }));
        let notice = opened.notice.clone();
        let error = opened.error.clone();
        let body = self.tab_body(shown, detail.as_ref(), window, cx);

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(14.))
                    .pb(px(8.))
                    .border_b_1()
                    .border_color(content(0.07))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(8.))
                            .child(div().pt(px(3.)).child(state_glyph(&view, 16.)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(content(0.95))
                                    .child(format!("{} #{}", view.title, view.number)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_x(px(8.))
                            .text_size(px(11.5))
                            .text_color(content(0.5))
                            .child(meta.join(" · "))
                            .when(is_pull, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .gap(px(5.))
                                        .child(
                                            div()
                                                .text_color(added)
                                                .child(format!("+{}", view.additions)),
                                        )
                                        .child(
                                            div()
                                                .text_color(removed)
                                                .child(format!("−{}", view.deletions)),
                                        ),
                                )
                            }),
                    )
                    .child(actions)
                    .children(
                        blocker
                            .filter(|_| open_pull && !view.is_draft)
                            .map(|reason| {
                                div()
                                    .text_size(px(11.5))
                                    .text_color(content(0.45))
                                    .child(reason)
                            }),
                    )
                    .children(notice.map(|(text, failed)| note(&text, failed)))
                    .child(tab_row),
            )
            .children(error.map(|e| note(&e, true)))
            .child(body)
            .into_any_element()
    }

    fn tab_body(
        &mut self,
        tab: Tab,
        detail: Option<&Detail>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scroller = || {
            div()
                .id("pulls-detail-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p(px(14.))
                .flex()
                .flex_col()
                .gap(px(14.))
        };
        let Some(detail) = detail else {
            let loading = self.opened.as_ref().is_some_and(|o| o.error.is_none());
            return div()
                .child(note(if loading { t("Carregando…") } else { "" }, false))
                .into_any_element();
        };
        match tab {
            Tab::Summary => {
                let latest = latest_reviews(&detail.reviews);
                let mut body = scroller();
                body = body.child(if detail.body.trim().is_empty() {
                    div()
                        .text_size(px(12.5))
                        .text_color(content(0.4))
                        .child(t("Sem descrição."))
                        .into_any_element()
                } else {
                    crate::markdown::render(&detail.body)
                });
                if !detail.item.labels.is_empty() {
                    body = body.child(div().flex().flex_wrap().gap(px(4.)).children(
                        detail.item.labels.iter().map(|label| {
                            div()
                                .px(px(7.))
                                .py(px(1.))
                                .rounded_full()
                                .border_1()
                                .border_color(crate::theme::border())
                                .text_size(px(11.))
                                .text_color(content(0.7))
                                .child(label.clone())
                        }),
                    ));
                }
                if !latest.is_empty() || !detail.reviewers.is_empty() {
                    let pending = detail
                        .reviewers
                        .iter()
                        .filter(|name| !latest.iter().any(|(author, _)| author == *name))
                        .map(|name| (name.clone(), "REQUESTED".to_owned()));
                    body = body.child(div().child(heading(t("Revisores"))).child(card().children(
                        latest.iter().cloned().chain(pending).map(|(name, state)| {
                            div()
                                .py(px(3.))
                                .flex()
                                .justify_between()
                                .text_size(px(12.5))
                                .text_color(content(0.85))
                                .child(name)
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(match state.as_str() {
                                            "APPROVED" => rgb(GREEN).into(),
                                            "CHANGES_REQUESTED" => rgb(RED).into(),
                                            _ => content(0.45),
                                        })
                                        .child(review_label(&state)),
                                )
                        }),
                    )));
                }
                if !detail.item.assignees.is_empty() {
                    body = body.child(
                        div().child(heading(t("Atribuídos"))).child(
                            card()
                                .text_size(px(12.5))
                                .text_color(content(0.85))
                                .child(detail.item.assignees.join(", ")),
                        ),
                    );
                }
                if !detail.files.is_empty() {
                    let count = detail.changed_files.max(detail.files.len() as u64);
                    let (added, removed) = diff_colors(0);
                    body = body.child(
                        div().child(heading(tf("Arquivos ({})", &[&count]))).child(
                            card()
                                .children(detail.files.iter().map(|file| {
                                    div()
                                        .py(px(2.))
                                        .flex()
                                        .gap(px(8.))
                                        .text_size(px(11.5))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .font_family("Menlo")
                                                .text_color(content(0.7))
                                                .child(file.path.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_color(added)
                                                .child(format!("+{}", file.additions)),
                                        )
                                        .child(
                                            div()
                                                .text_color(removed)
                                                .child(format!("−{}", file.deletions)),
                                        )
                                }))
                                .when(detail.files_truncated, |c| {
                                    c.child(
                                        div()
                                            .pt(px(4.))
                                            .text_size(px(11.))
                                            .text_color(content(0.4))
                                            .child(t("Há mais arquivos do que os listados.")),
                                    )
                                }),
                        ),
                    );
                }
                body.into_any_element()
            }
            Tab::Activity => {
                let events = timeline(detail);
                let busy = self.opened.as_ref().is_some_and(|o| o.busy);
                let typed = !self.comment.read(cx).text().trim().is_empty();
                let mut body = scroller();
                if events.is_empty() {
                    body = body.child(
                        div()
                            .text_size(px(12.5))
                            .text_color(content(0.4))
                            .child(t("Nenhuma atividade ainda.")),
                    );
                }
                for event in events {
                    body = body.child(match event {
                        Event::Commit(commit) => div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(12.5))
                            .text_color(content(0.7))
                            .child(icon("branch", 12.).text_color(content(0.4)))
                            .child(div().flex_1().min_w_0().truncate().child(commit.headline))
                            .child(
                                div()
                                    .font_family("Menlo")
                                    .text_size(px(11.))
                                    .text_color(content(0.4))
                                    .child(commit.oid.chars().take(7).collect::<String>()),
                            )
                            .into_any_element(),
                        Event::Review(review) => entry(
                            review.author.as_deref(),
                            review.submitted_at.as_deref(),
                            Some(review_label(&review.state)),
                            &review.body,
                        ),
                        Event::Comment(comment) => entry(
                            comment.author.as_deref(),
                            Some(&comment.created_at),
                            None,
                            &comment.body,
                        ),
                    });
                }
                body = body.child(
                    card()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(self.comment.clone())
                        .child(
                            div().flex().justify_end().child(
                                button(
                                    "pulls-comment",
                                    if busy {
                                        t("Enviando…")
                                    } else {
                                        t("Comentar")
                                    },
                                    false,
                                    typed && !busy,
                                )
                                .when(typed && !busy, |b| {
                                    b.on_click(cx.listener(|this, _, _, cx| this.send_comment(cx)))
                                }),
                            ),
                        ),
                );
                body.into_any_element()
            }
            Tab::Checks => {
                let mut body = scroller();
                for (ix, check) in detail.checks.iter().enumerate() {
                    let (label, color): (&str, gpui::Hsla) = match check.status.as_str() {
                        "passed" => (t("Passou"), rgb(GREEN).into()),
                        "failed" => (t("Falhou"), rgb(RED).into()),
                        "pending" => (t("Pendente"), rgb(AMBER).into()),
                        "skipped" => (t("Ignorado"), content(0.4)),
                        _ => (t("Desconhecido"), content(0.4)),
                    };
                    let url = check.url.as_deref().and_then(crate::markdown::web_url);
                    body = body.child(
                        div()
                            .id(("pulls-check", ix))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(12.5))
                            .text_color(content(0.85))
                            .when_some(url, |d, url| {
                                d.cursor_pointer()
                                    .hover(|s| s.text_color(content(1.)))
                                    .on_click(move |_, _, cx| cx.open_url(&url))
                            })
                            .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(color))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(check.name.clone()),
                            )
                            .child(div().text_size(px(11.5)).text_color(color).child(label)),
                    );
                }
                body.into_any_element()
            }
            Tab::Code => match self.opened.as_ref().map(|o| &o.diff) {
                Some(DiffState::Ready {
                    lines,
                    files,
                    truncated,
                }) => {
                    let (added, removed) = diff_colors(0);
                    let (adds, dels) = files
                        .iter()
                        .fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
                    let lines = lines.clone();
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex_shrink_0()
                                .px(px(14.))
                                .py(px(6.))
                                .flex()
                                .gap(px(8.))
                                .text_size(px(11.5))
                                .text_color(content(0.5))
                                .child(tf("{} arquivos", &[&files.len()]))
                                .child(div().text_color(added).child(format!("+{adds}")))
                                .child(div().text_color(removed).child(format!("−{dels}")))
                                .when(*truncated, |d| d.child(t("O diff foi truncado."))),
                        )
                        .child(if lines.is_empty() {
                            div()
                                .child(note(t("Sem diferenças para mostrar."), false))
                                .into_any_element()
                        } else {
                            uniform_list("pulls-diff", lines.len(), move |range, _, _| {
                                range.map(|ix| diff_line(&lines[ix])).collect::<Vec<_>>()
                            })
                            .flex_1()
                            .min_h_0()
                            .into_any_element()
                        })
                        .into_any_element()
                }
                Some(DiffState::Failed(error)) => div().child(note(error, true)).into_any_element(),
                _ => div()
                    .child(note(t("Carregando…"), false))
                    .into_any_element(),
            },
        }
    }
}

fn review_label(state: &str) -> &'static str {
    match state {
        "APPROVED" => t("Aprovou"),
        "CHANGES_REQUESTED" => t("Pediu alterações"),
        "COMMENTED" => t("Comentou"),
        "REQUESTED" => t("Revisão solicitada"),
        "DISMISSED" => t("Descartada"),
        _ => t("Revisou"),
    }
}

/// A review or a comment: who, when, and the Markdown they wrote.
fn entry(
    author: Option<&str>,
    at: Option<&str>,
    badge: Option<&'static str>,
    body: &str,
) -> AnyElement {
    card()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(11.5))
                .text_color(content(0.45))
                .child(
                    div()
                        .text_color(content(0.75))
                        .child(author.map_or_else(|| t("Alguém").to_owned(), str::to_owned)),
                )
                .children(badge)
                .child(div().flex_1())
                .child(at.map(age).unwrap_or_default()),
        )
        .when(!body.trim().is_empty(), |d| {
            d.child(crate::markdown::render(body))
        })
        .into_any_element()
}

impl Render for PullRequestsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let mut moving = false;
        let list = self.list_column(now, &mut moving, cx);
        if moving {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .flex()
            .child(list)
            .child(self.detail_column(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(repository: &str, number: u64, title: &str) -> Item {
        Item {
            kind: Kind::Pull,
            repository: repository.into(),
            number,
            title: title.into(),
            url: format!("https://github.com/{repository}/pull/{number}"),
            state: State::Open,
            is_draft: false,
            author: Some("ana".into()),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-02T00:00:00Z".into(),
            head_ref: Some("feature".into()),
            base_ref: Some("main".into()),
            additions: 1,
            deletions: 2,
            mergeable: None,
            labels: vec![],
            comment_count: 0,
            assignees: vec![],
            review_requests: vec![],
            checks: None,
        }
    }

    fn detail(item: Item) -> Detail {
        Detail {
            item,
            body: String::new(),
            head_oid: Some("abc".into()),
            merge_state_status: Some("CLEAN".into()),
            changed_files: 0,
            merge_methods: vec!["squash".into()],
            checks: vec![],
            reviewers: vec![],
            reviews: vec![],
            comments: vec![],
            commits: vec![],
            files: vec![],
            files_truncated: false,
        }
    }

    #[test]
    fn parses_an_item_and_skips_one_without_a_number() {
        let value = json!({
            "kind": "issue", "repository": "o/r", "number": 7, "title": "Bug",
            "state": "closed", "isDraft": false, "author": "bia",
            "labels": [{"name": "bug", "color": "ff0000"}], "commentCount": 3,
            "reviewRequests": ["ana"], "checks": "FAILURE"
        });
        let parsed = parse_item(&value).unwrap();
        assert_eq!(parsed.kind, Kind::Issue);
        assert_eq!(parsed.state, State::Closed);
        assert_eq!(parsed.labels, vec!["bug"]);
        assert_eq!(parsed.comment_count, 3);
        assert_eq!(parsed.review_requests, vec!["ana"]);
        assert_eq!(parsed.checks.as_deref(), Some("FAILURE"));
        assert!(parse_item(&json!({ "repository": "o/r", "title": "x" })).is_none());
    }

    #[test]
    fn parses_an_inbox_with_failures() {
        let inbox = parse_inbox(&json!({
            "viewer": "ana",
            "repositories": [{"repository": "o/r", "projectIds": ["p1"]}],
            "items": [{"repository": "o/r", "number": 1, "title": "a"}, {"title": "broken"}],
            "failures": [{"repository": "o/x", "status": "authRequired"}]
        }));
        assert_eq!(inbox.viewer.as_deref(), Some("ana"));
        assert_eq!(
            inbox.repositories,
            vec![("o/r".to_owned(), vec!["p1".to_owned()])]
        );
        assert_eq!(inbox.items.len(), 1);
        assert_eq!(
            inbox.failures,
            vec![("o/x".to_owned(), "authRequired".to_owned())]
        );
    }

    #[test]
    fn parses_a_detail() {
        let value = json!({
            "item": {"repository": "o/r", "number": 4, "title": "t"},
            "body": "hello", "headOid": "deadbeef", "mergeStateStatus": "CLEAN",
            "changedFiles": 2, "mergeMethods": ["squash", "merge"],
            "checks": [{"name": "ci", "status": "failed", "url": "https://x"}],
            "reviews": [{"author": "bia", "state": "APPROVED", "body": "", "submittedAt": "2026-01-01T00:00:00Z"}],
            "comments": [{"author": null, "body": "hi", "createdAt": "2026-01-02T00:00:00Z"}],
            "commits": [{"oid": "1234567890", "headline": "fix", "committedAt": null}],
            "files": [{"path": "a.rs", "additions": 3, "deletions": 1}],
            "filesTruncated": true
        });
        let parsed = parse_detail(&value).unwrap();
        assert_eq!(parsed.merge_methods, vec!["squash", "merge"]);
        assert_eq!(parsed.checks[0].status, "failed");
        assert_eq!(parsed.comments[0].author, None);
        assert_eq!(parsed.files[0].additions, 3);
        assert!(parsed.files_truncated);
        assert!(parse_detail(&json!({ "body": "x" })).is_none());
    }

    #[test]
    fn parses_links_and_short_references() {
        let link = parse_reference("https://github.com/o/r/pull/12/files").unwrap();
        assert_eq!(
            (link.repository.as_str(), link.number, link.kind),
            ("o/r", 12, Some(Kind::Pull))
        );
        let issue = parse_reference(" https://github.com/o/r.js/issues/3?x=1 ").unwrap();
        assert_eq!((issue.number, issue.kind), (3, Some(Kind::Issue)));
        let short = parse_reference("o/r#9").unwrap();
        assert_eq!(
            (short.repository.as_str(), short.number, short.kind),
            ("o/r", 9, None)
        );
        for bad in [
            "https://github.com/o/r/pull/x",
            "https://github.com/o/r/pull/12abc",
            "https://github.com/o/r/commit/12",
            "o/r#",
            "o/r#1x",
            "or#1",
            "o/r r#1",
            "fix login",
        ] {
            assert!(parse_reference(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn filters_by_search_text_and_pasted_link() {
        let all = vec![
            item("o/app", 10, "Fix login"),
            item("o/app", 11, "Add logout"),
        ];
        let found = |query: &str| {
            visible_items(&inbox(all.clone()), Some("p1"), query)
                .iter()
                .map(|i| i.number)
                .collect::<Vec<_>>()
        };
        assert_eq!(found(""), vec![10, 11]);
        assert_eq!(found("LOGIN"), vec![10]);
        assert_eq!(found("#11"), vec![11]);
        assert_eq!(found("ana"), vec![10, 11]);
        assert_eq!(found("feature"), vec![10, 11]);
        assert_eq!(found("https://github.com/O/app/pull/11"), vec![11]);
        assert_eq!(found("o/app#99"), Vec::<u64>::new());
    }

    #[test]
    fn merge_blockers_follow_the_github_state() {
        let ok = detail(item("o/r", 1, "x"));
        assert_eq!(merge_blocker(&ok), None);
        let mut draft = ok.clone();
        draft.item.is_draft = true;
        assert!(merge_blocker(&draft).is_some());
        let mut conflicting = ok.clone();
        conflicting.item.mergeable = Some("CONFLICTING".into());
        assert!(merge_blocker(&conflicting).is_some());
        for status in ["DIRTY", "BLOCKED", "BEHIND"] {
            let mut blocked = ok.clone();
            blocked.merge_state_status = Some(status.into());
            assert!(merge_blocker(&blocked).is_some(), "{status}");
        }
        let mut no_head = ok.clone();
        no_head.head_oid = None;
        assert!(merge_blocker(&no_head).is_some());
        let mut no_methods = ok.clone();
        no_methods.merge_methods.clear();
        assert!(merge_blocker(&no_methods).is_some());
        let mut closed = ok;
        closed.item.state = State::Closed;
        closed.item.is_draft = true;
        assert_eq!(merge_blocker(&closed), None);
    }

    #[test]
    fn splits_a_diff_into_files_with_counts() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1,2 @@\n-old\n+new\n+more\n\
diff --git a/b.txt b/b.txt\nnew file mode 100644\n--- /dev/null\n+++ b/b.txt\n+x\n";
        let files = split_diff(diff);
        assert_eq!(
            files,
            vec![
                DiffFile {
                    path: "src/a.rs".into(),
                    additions: 2,
                    deletions: 1
                },
                DiffFile {
                    path: "b.txt".into(),
                    additions: 1,
                    deletions: 0
                },
            ]
        );
        assert!(split_diff("").is_empty());
        assert!(split_diff("no diff here").is_empty());
    }

    #[test]
    fn the_timeline_is_in_order_and_reviews_keep_the_latest_state() {
        let mut d = detail(item("o/r", 1, "x"));
        d.commits = vec![Commit {
            oid: "1".into(),
            headline: "c".into(),
            committed_at: Some("2026-01-02T00:00:00Z".into()),
        }];
        d.comments = vec![Comment {
            author: None,
            body: "late".into(),
            created_at: "2026-01-04T00:00:00Z".into(),
        }];
        d.reviews = vec![
            Review {
                author: Some("bia".into()),
                state: "CHANGES_REQUESTED".into(),
                body: String::new(),
                submitted_at: Some("2026-01-03T00:00:00Z".into()),
            },
            Review {
                author: Some("bia".into()),
                state: "COMMENTED".into(),
                body: String::new(),
                submitted_at: None,
            },
            Review {
                author: Some("bia".into()),
                state: "APPROVED".into(),
                body: String::new(),
                submitted_at: Some("2026-01-05T00:00:00Z".into()),
            },
            Review {
                author: Some("caio".into()),
                state: "COMMENTED".into(),
                body: String::new(),
                submitted_at: None,
            },
        ];
        let kinds: Vec<&str> = timeline(&d)
            .iter()
            .map(|e| match e {
                Event::Commit(_) => "commit",
                Event::Review(_) => "review",
                Event::Comment(_) => "comment",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["review", "review", "commit", "review", "comment", "review"]
        );
        assert_eq!(
            latest_reviews(&d.reviews),
            vec![("bia".to_owned(), "APPROVED".to_owned())]
        );
    }

    #[test]
    fn the_ask_action_names_draft_pulls_and_carries_the_description() {
        let mut d = detail(item("o/r", 8, "Title"));
        d.body = "why".into();
        d.item.is_draft = true;
        let action = ask_action(&d);
        assert_eq!(action["action"]["type"], "open");
        assert_eq!(action["action"]["kind"], "pullRequest");
        assert_eq!(action["action"]["number"], 8);
        assert_eq!(action["action"]["context"]["state"], "draft");
        assert_eq!(action["action"]["context"]["description"], "why");
        d.item.is_draft = false;
        d.item.state = State::Merged;
        assert_eq!(ask_action(&d)["action"]["context"]["state"], "merged");
    }

    /// An inbox of two repositories: `o/app` belongs to project p1, `o/lib` to project p2.
    fn inbox(items: Vec<Item>) -> Inbox {
        Inbox {
            viewer: None,
            repositories: vec![
                ("o/app".into(), vec!["p1".into()]),
                ("o/lib".into(), vec!["p2".into()]),
            ],
            items,
            failures: Vec::new(),
        }
    }

    #[test]
    fn segments_map_to_the_list_the_host_is_asked_for() {
        let lists: Vec<(Kind, State)> = Segment::ALL.iter().map(|s| s.list()).collect();
        assert_eq!(
            lists,
            vec![
                (Kind::Pull, State::Open),
                (Kind::Pull, State::Merged),
                (Kind::Pull, State::Closed),
                (Kind::Issue, State::Open),
            ]
        );
        let labels: Vec<&str> = Segment::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(labels, vec!["Abertos", "Mesclados", "Fechados", "Issues"]);
        assert!(Segment::ALL.iter().enumerate().all(|(i, s)| s.index() == i));
        assert_eq!(Segment::ALL[0], Segment::Open);
    }

    #[test]
    fn only_the_selected_projects_repository_is_listed_newest_updated_first() {
        let mut old = item("o/app", 1, "Old");
        old.updated_at = "2026-01-01T00:00:00Z".into();
        let mut new = item("o/APP", 2, "New");
        new.updated_at = "2026-03-01T00:00:00Z".into();
        let other = item("o/lib", 3, "Elsewhere");
        let inbox = inbox(vec![old, other, new]);
        let numbers = |project: Option<&str>| {
            visible_items(&inbox, project, "")
                .iter()
                .map(|i| i.number)
                .collect::<Vec<_>>()
        };
        assert_eq!(numbers(Some("p1")), vec![2, 1]);
        assert_eq!(numbers(Some("p2")), vec![3]);
        assert!(numbers(Some("p3")).is_empty());
        assert!(numbers(None).is_empty());
        assert_eq!(project_repositories(&inbox, Some("p1")), vec!["o/app"]);
    }

    #[test]
    fn a_link_from_another_repository_is_refused_not_matched() {
        let inbox = inbox(vec![item("o/app", 1, "A"), item("o/lib", 3, "B")]);
        let link = "https://github.com/o/lib/pull/3";
        assert!(visible_items(&inbox, Some("p1"), link).is_empty());
        assert_eq!(
            foreign_reference(&inbox, Some("p1"), link).map(|r| r.number),
            Some(3)
        );
        assert!(foreign_reference(&inbox, Some("p2"), link).is_none());
        let own = "https://github.com/o/app/pull/1";
        assert_eq!(visible_items(&inbox, Some("p1"), own).len(), 1);
        assert!(foreign_reference(&inbox, Some("p1"), own).is_none());
        assert!(foreign_reference(&inbox, Some("p1"), "plain text").is_none());
    }
}
