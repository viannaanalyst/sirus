//! The Alterações pane, after Sirus's ChangesPane.tsx and GitHistory.tsx: the session's Git status
//! in two groups, Preparados (staged) and Alterações (unstaged), with added and removed line counts.
//! A click shows the file's diff, colored; Preparar and Despreparar move one file between the groups.
//! Below the lists, the Histórico: the branch's commits, a graph line and "Carregar mais" for older
//! pages. A click on a commit lists the files it changed with their added and removed lines, and a
//! click on one of those shows its diff; the arrow goes back a step. The
//! host back end has no such request, so those two are read with `git` in the worktree, on a
//! background thread (local_git.rs). The footer commits the staged files with the title typed in
//! its field.
use crate::backend::Backend;
use crate::i18n::{t, tf};
use crate::local_git::{self, CommitFiles};
use crate::panel_files::{TextField, TextFieldEvent};
use crate::panels::PanelContext;
use crate::project_files::run_blocking;
use crate::theme::{accent, accent_wash, content, diff_colors, on_accent};
use gpui::{
    AnyElement, AnyView, App, Context, Div, Entity, Hsla, Render, SharedString, Stateful,
    Subscription, Window, div, prelude::*, px, rgb, svg, uniform_list,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::rc::Rc;

/// The Alterações tab: the branch and its history.
pub fn new(context: PanelContext, cx: &mut App) -> AnyView {
    build(context, Mode::History, cx)
}

/// The changes view the top bar's +N −N opens: message, Commit and the changed files.
pub fn new_review(context: PanelContext, cx: &mut App) -> AnyView {
    build(context, Mode::Review, cx)
}

fn build(context: PanelContext, mode: Mode, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let commit = cx.new(|cx| {
            let mut field = TextField::new(t("Mensagem (⌘↩ para commit)"), cx);
            field.set_bare(true);
            field.set_command_submit(true);
            field
        });
        let commit_events = cx.subscribe(
            &commit,
            |this: &mut ChangesPanel, _, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Submit => this.commit(cx),
                TextFieldEvent::Changed => cx.notify(),
                TextFieldEvent::Cancel => {}
            },
        );
        let follow = crate::workspace_follow::Follow::start(
            context.backend.clone(),
            context.session_id.clone(),
            ChangesPanel::disk_changed,
            cx,
        );
        let mut panel = ChangesPanel {
            mode,
            menu: None,
            discard: None,
            staged_open: true,
            changes_open: true,
            _follow: follow,
            backend: context.backend,
            session_id: context.session_id,
            root: context.worktree,
            snapshot: None,
            counts: HashMap::new(),
            loading: false,
            busy: false,
            error: None,
            opened: None,
            commit_view: None,
            amending: false,
            older: None,
            loading_older: false,
            commit,
            committing: false,
            title_job: None,
            title_note: None,
            push: PushState::default(),
            _commit_events: commit_events,
            pr: PullRequestForm::new(cx),
        };
        panel.refresh(cx);
        panel
    })
    .into()
}

/// One commit of the Histórico.
#[derive(Clone)]
struct Commit {
    hash: String,
    parents: Vec<String>,
    /// Its branches and tags, as `git log` decorates it.
    refs: Vec<String>,
    subject: String,
    author: String,
    /// The author's date as the back end sends it (RFC 3339).
    when: String,
}

/// Pages loaded below the snapshot's own history, for the head they were loaded from.
struct Older {
    from: String,
    entries: Vec<Commit>,
    truncated: bool,
}

#[derive(Clone)]
struct Entry {
    path: String,
    kind: String,
    actionable: bool,
}

struct Snapshot {
    /// The index this view was read from; every change sends it back, so a stale view fails.
    token: String,
    repo: bool,
    branch: Option<String>,
    detached: bool,
    ahead: Option<u64>,
    behind: Option<u64>,
    /// The commit HEAD points to; older pages of the Histórico continue from it.
    head: Option<String>,
    history: Vec<Commit>,
    history_truncated: bool,
    staged: Vec<Entry>,
    unstaged: Vec<Entry>,
    conflicts: bool,
    truncated: bool,
}

enum Lines {
    Loading,
    Ready(Rc<Vec<String>>),
    Failed(String),
}

struct Opened {
    path: String,
    staged: bool,
    lines: Lines,
}

/// A commit of the Histórico opened to see what it changed.
struct CommitView {
    commit: Commit,
    files: CommitFilesState,
    /// The file whose diff is on screen; None while the file list is.
    diff: Option<CommitDiff>,
}

enum CommitFilesState {
    Loading,
    Ready(Rc<CommitFiles>),
    Failed(String),
}

struct CommitDiff {
    path: String,
    lines: Lines,
}

/// Which half of the old Alterações tab a panel shows.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// The branch row and the Histórico.
    History,
    /// The message, the Commit button and the changed files.
    Review,
}

/// The menu open over the changes view.
#[derive(Clone, Copy, PartialEq)]
enum Menu {
    /// The "⋯" of the branch row.
    Branch,
    /// The "▾" of the Commit button.
    Commit,
}

/// The header bar's height, and where the Commit button's menu starts under it.
const TITLE: f32 = 34.;
const COMMIT_BAR: f32 = 8. + 34. + 8. + 32. + 4.;

struct ChangesPanel {
    mode: Mode,
    menu: Option<Menu>,
    /// A discard waiting for the confirmation card.
    discard: Option<DiscardAsk>,
    staged_open: bool,
    changes_open: bool,
    /// Reads the status again when files change on disk; stops with the panel.
    _follow: crate::workspace_follow::Follow,
    backend: Backend,
    session_id: String,
    /// The session's folder, where `git` runs for the history views.
    root: String,
    snapshot: Option<Snapshot>,
    /// Added and removed line counts by path, from git_status.
    counts: HashMap<String, (u64, u64)>,
    loading: bool,
    busy: bool,
    error: Option<String>,
    /// The diff on screen; None while the list is showing.
    opened: Option<Opened>,
    /// The commit opened from the Histórico; shown instead of the list while it is Some.
    commit_view: Option<CommitView>,
    /// The next commit replaces the last one.
    amending: bool,
    older: Option<Older>,
    loading_older: bool,
    commit: Entity<TextField>,
    committing: bool,
    /// The generated title being written, if any (commit_title_action).
    title_job: Option<TitleJob>,
    /// Who wrote the last generated title, or why it was dropped.
    title_note: Option<String>,
    /// Pushing: the confirmation after "Commit e Push" and what the last push said.
    push: PushState,
    _commit_events: Subscription,
    /// The Criar PR form of the footer, as the Environment panel's.
    pr: PullRequestForm,
}

/// What the discard confirmation will throw away: the card's title and the files.
struct DiscardAsk {
    title: String,
    paths: Vec<String>,
}

/// A commit title being generated: its request id (the host takes one job at a time) and whether
/// Cancel was already sent.
struct TitleJob {
    id: String,
    cancelling: bool,
}

/// Pushing is never implicit: Commit e Push commits, then asks to confirm, as in React's pane.
#[derive(Default)]
struct PushState {
    confirm: bool,
    pushing: bool,
    /// The last push: a message and whether it failed.
    result: Option<(String, bool)>,
}

/// Criar PR: the title, the description and the draft toggle, and what the last try gave.
struct PullRequestForm {
    open: bool,
    title: Entity<TextField>,
    body: Entity<TextField>,
    draft: bool,
    creating: bool,
    /// The last try: a message, whether it failed, and the new pull request's address.
    result: Option<(String, bool, Option<String>)>,
    /// What `session_pull_request` last said about this branch, and when it was asked.
    found: Option<Found>,
    asked: Option<std::time::Instant>,
    _events: Vec<Subscription>,
}

/// The branch's pull request (GitHub, Bitbucket) or merge request (GitLab) as the footer shows it.
#[derive(Debug, Clone, PartialEq)]
struct Found {
    /// `github`, `gitlab` or `bitbucket`.
    forge: String,
    /// The request, when the branch has one: number, state, draft, address and its checks.
    request: Option<FoundRequest>,
    /// Why there is none to show: the CLI is missing or signed out (GitLab, Bitbucket only).
    hint: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
struct FoundRequest {
    number: u64,
    state: String,
    draft: bool,
    url: String,
    passed: usize,
    failed: usize,
    pending: usize,
    total: usize,
}

impl Found {
    fn merge_request(&self) -> bool {
        self.forge == "gitlab"
    }
}

/// A `session_pull_request` reply, read for the footer.
fn found_of(value: &Value) -> Option<Found> {
    let forge = value["forge"].as_str().unwrap_or("github").to_owned();
    let pr = &value["pullRequest"];
    let request = pr["number"].as_u64().and_then(|number| {
        let checks = pr["checks"].as_array().cloned().unwrap_or_default();
        let count = |status: &str| checks.iter().filter(|c| c["status"] == status).count();
        Some(FoundRequest {
            number,
            state: pr["state"].as_str()?.to_owned(),
            draft: pr["draft"].as_bool().unwrap_or(false),
            url: pr["url"].as_str()?.to_owned(),
            passed: count("passed"),
            failed: count("failed"),
            pending: count("pending"),
            total: checks.len(),
        })
    });
    let other = forge != "github";
    let hint = match (value["status"].as_str().unwrap_or_default(), forge.as_str()) {
        ("cliMissing", "gitlab") => Some(t(
            "Instale o GitLab CLI (`brew install glab`) e rode `glab auth login` para ver e criar merge requests.",
        )),
        ("authRequired", "gitlab") => Some(t(
            "Rode `glab auth login` no terminal para ver e criar merge requests.",
        )),
        ("authRequired", "bitbucket") => Some(t(
            "Adicione um token do Bitbucket em Configurações → Git para ver e criar pull requests.",
        )),
        _ => None,
    };
    (request.is_some() || hint.is_some() || other).then_some(Found {
        forge,
        request,
        hint,
    })
}

/// "PR #12 · Aberto · 2 de 3 checks passaram", or "MR !12" on GitLab.
fn found_line(found: &Found, request: &FoundRequest) -> String {
    let name = if found.merge_request() {
        format!("MR !{}", request.number)
    } else {
        format!("PR #{}", request.number)
    };
    let state = match (request.state.as_str(), request.draft) {
        ("merged", _) => t("Mesclado"),
        ("closed", _) => t("Fechado"),
        (_, true) => t("Rascunho"),
        _ => t("Aberto"),
    };
    let mut parts = vec![name, state.to_owned()];
    if request.total > 0 {
        parts.push(if request.failed > 0 {
            tf(
                "{} de {} checks falharam",
                &[&request.failed, &request.total],
            )
        } else if request.pending > 0 {
            tf(
                "{} de {} checks rodando",
                &[&request.pending, &request.total],
            )
        } else {
            tf(
                "{} de {} checks passaram",
                &[&request.passed, &request.total],
            )
        });
    }
    parts.join(" · ")
}

impl PullRequestForm {
    fn new(cx: &mut Context<ChangesPanel>) -> Self {
        let title = cx.new(|cx| TextField::new(t("Título do pull request"), cx));
        let body = cx.new(|cx| TextField::new(t("Descrição (opcional)"), cx));
        let events = vec![
            cx.subscribe(
                &title,
                |this: &mut ChangesPanel, _, event: &TextFieldEvent, cx| match event {
                    TextFieldEvent::Submit => this.create_pull_request(cx),
                    TextFieldEvent::Changed => cx.notify(),
                    TextFieldEvent::Cancel => {}
                },
            ),
            cx.subscribe(
                &body,
                |this: &mut ChangesPanel, _, event: &TextFieldEvent, cx| match event {
                    TextFieldEvent::Submit => this.create_pull_request(cx),
                    TextFieldEvent::Changed => cx.notify(),
                    TextFieldEvent::Cancel => {}
                },
            ),
        ];
        Self {
            open: false,
            title,
            body,
            draft: false,
            creating: false,
            result: None,
            found: None,
            asked: None,
            _events: events,
        }
    }
}

/// The address `pull_request_action` returns for a created pull request.
fn created_url(reply: &Value) -> Option<String> {
    (reply["type"] == "created")
        .then(|| reply["url"].as_str().map(str::to_owned))
        .flatten()
}

/// What `pull_request_action` takes to push the session's branch and open a pull request.
fn create_action(session_id: &str, title: &str, body: &str, draft: bool) -> Value {
    json!({ "action": { "type": "create", "sessionId": session_id, "title": title.trim(),
        "body": body.trim(), "draft": draft, "confirm": true } })
}

impl ChangesPanel {
    /// The next page of the Histórico, from HEAD as the snapshot read it, after the rows shown so far.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let Some(from) = snapshot.head.clone() else {
            return;
        };
        if self.loading_older {
            return;
        }
        let loaded = self
            .older
            .as_ref()
            .filter(|older| older.from == from)
            .map_or(0, |older| older.entries.len());
        let skip = snapshot.history.len() + loaded;
        self.loading_older = true;
        let rx = self.backend.request(
            "git_workspace_action",
            json!({ "action": { "type": "history", "sessionId": self.session_id, "from": from.clone(), "skip": skip } }),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_older = false;
                match reply {
                    Ok(value) => {
                        let mut older = this
                            .older
                            .take()
                            .filter(|older| older.from == from)
                            .unwrap_or(Older {
                                from: from.clone(),
                                entries: Vec::new(),
                                truncated: false,
                            });
                        older.entries.extend(commits_of(&value["entries"]));
                        older.truncated = value["truncated"].as_bool().unwrap_or(false);
                        this.older = Some(older);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Opens a commit of the Histórico: its changed files are read with `git` on a background thread.
    fn open_commit(&mut self, commit: Commit, cx: &mut Context<Self>) {
        let (root, sha) = (self.root.clone(), commit.hash.clone());
        self.commit_view = Some(CommitView {
            commit,
            files: CommitFilesState::Loading,
            diff: None,
        });
        let rx = run_blocking({
            let sha = sha.clone();
            move || local_git::commit_files(&root, &sha)
        });
        cx.spawn(async move |this, cx| {
            let files = match rx.recv().await {
                Ok(Ok(files)) => CommitFilesState::Ready(Rc::new(files)),
                Ok(Err(error)) => CommitFilesState::Failed(error),
                Err(_) => CommitFilesState::Failed(t("Não foi possível ler o commit.").into()),
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(view) = this
                    .commit_view
                    .as_mut()
                    .filter(|view| view.commit.hash == sha)
                {
                    view.files = files;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Shows one file's diff of the opened commit.
    fn open_commit_file(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(view) = self.commit_view.as_mut() else {
            return;
        };
        let sha = view.commit.hash.clone();
        view.diff = Some(CommitDiff {
            path: path.clone(),
            lines: Lines::Loading,
        });
        let root = self.root.clone();
        let rx = run_blocking({
            let (sha, path) = (sha.clone(), path.clone());
            move || local_git::commit_file_diff(&root, &sha, &path)
        });
        cx.spawn(async move |this, cx| {
            let lines = match rx.recv().await {
                Ok(Ok(diff)) => Lines::Ready(Rc::new(diff.lines().map(str::to_owned).collect())),
                Ok(Err(error)) => Lines::Failed(error),
                Err(_) => Lines::Failed(t("Não foi possível ler o diff.").into()),
            };
            let _ = this.update(cx, |this, cx| {
                let shown = this
                    .commit_view
                    .as_mut()
                    .filter(|view| view.commit.hash == sha)
                    .and_then(|view| view.diff.as_mut())
                    .filter(|diff| diff.path == path);
                if let Some(diff) = shown {
                    diff.lines = lines;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// One step back from a commit: its diff to its file list, its file list to the Histórico.
    fn commit_back(&mut self, cx: &mut Context<Self>) {
        match self.commit_view.as_mut() {
            Some(view) if view.diff.is_some() => view.diff = None,
            _ => self.commit_view = None,
        }
        cx.notify();
    }

    /// Commits the staged files with the typed title. The index token makes a stale view fail, so
    /// the pane reads the status again afterwards either way.
    fn commit(&mut self, cx: &mut Context<Self>) {
        self.commit_with(false, cx);
    }

    /// Commits, and with `then_push` asks to confirm a push once the commit is made.
    fn commit_with(&mut self, then_push: bool, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let message = self.commit.read(cx).text().trim().to_owned();
        let amend = self.amending;
        if self.committing || (!amend && (message.is_empty() || snapshot.staged.is_empty())) {
            return;
        }
        let token = snapshot.token.clone();
        self.committing = true;
        self.error = None;
        self.title_note = None;
        self.push.result = None;
        let rx = self.backend.request(
            "git_commit",
            json!({ "sessionId": self.session_id, "message": message, "expectedIndex": token, "amend": amend }),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.committing = false;
                match reply {
                    Ok(_) => {
                        this.commit.update(cx, |field, cx| field.set_text("", cx));
                        this.amending = false;
                        this.push.confirm = then_push;
                    }
                    Err(error) => this.error = Some(error),
                }
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Asks the host to write a title for the staged changes. A title typed meanwhile, or a
    /// changed index, keeps the person's text.
    fn generate_title(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        if self.title_job.is_some() || self.committing || snapshot.staged.is_empty() {
            return;
        }
        let token = snapshot.token.clone();
        let started_with = self.commit.read(cx).text().to_owned();
        let id = crate::panel_sidechat::uuid();
        self.title_job = Some(TitleJob {
            id: id.clone(),
            cancelling: false,
        });
        self.title_note = None;
        self.error = None;
        let rx = self.backend.request(
            "commit_title_action",
            title_action(&self.session_id, &token, &id),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                if this.title_job.as_ref().map(|job| job.id.as_str()) != Some(id.as_str()) {
                    return;
                }
                this.title_job = None;
                match reply.map(|value| title_reply(&value)) {
                    Ok(TitleReply::Title {
                        title,
                        provider,
                        partial,
                    }) => {
                        let now = this.commit.read(cx).text().to_owned();
                        let current = this.snapshot.as_ref().map(|s| s.token.as_str());
                        if keeps_typed(&started_with, &now, &token, current.unwrap_or("")) {
                            this.title_note = Some(t("Seu texto foi mantido.").into());
                        } else {
                            this.commit
                                .update(cx, |field, cx| field.set_text(&title, cx));
                            this.title_note = Some(attribution(&provider, partial));
                        }
                    }
                    Ok(TitleReply::Cancelled) => {
                        this.title_note = Some(t("Geração do título cancelada.").into())
                    }
                    Ok(TitleReply::Invalid) => {
                        this.error = Some(t("Resposta inesperada do título do commit.").into())
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn cancel_title(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.title_job.as_mut() else {
            return;
        };
        if job.cancelling {
            return;
        }
        job.cancelling = true;
        // The pending generate returns Cancelled; this answer only matters when it fails.
        let rx = self.backend.request(
            "commit_title_action",
            cancel_title_action(&self.session_id, &job.id),
        );
        cx.spawn(async move |this, cx| {
            if let Err(error) = settle(rx).await {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    /// Pushes the session's branch. Only reached from the confirmation.
    fn push_now(&mut self, cx: &mut Context<Self>) {
        let blocked = self
            .snapshot
            .as_ref()
            .is_none_or(|s| !can_push(s.repo, s.detached, &s.head));
        if self.push.pushing || blocked {
            return;
        }
        self.push.pushing = true;
        self.push.result = None;
        let rx = self
            .backend
            .request("git_push", json!({ "sessionId": self.session_id }));
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.push.pushing = false;
                this.push.confirm = false;
                this.push.result = Some(match reply {
                    Ok(value) => (pushed_text(&value), false),
                    Err(error) => (error, true),
                });
                this.pr.asked = None;
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Pushes the branch and opens the pull request; the host does both in one command.
    fn create_pull_request(&mut self, cx: &mut Context<Self>) {
        let title = self.pr.title.read(cx).text().trim().to_owned();
        if title.is_empty() || self.pr.creating {
            return;
        }
        let body = self.pr.body.read(cx).text().to_owned();
        self.pr.creating = true;
        self.pr.result = None;
        let rx = self.backend.request(
            "pull_request_action",
            create_action(&self.session_id, &title, &body, self.pr.draft),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.pr.creating = false;
                match reply.map(|value| created_url(&value)) {
                    Ok(Some(url)) => {
                        let made = if this.pr.found.as_ref().is_some_and(Found::merge_request) {
                            t("Merge request criado")
                        } else {
                            t("Pull request criado")
                        };
                        this.pr.result = Some((made.into(), false, Some(url)));
                        this.check_pull_request(true, cx);
                        this.pr.open = false;
                        this.pr.draft = false;
                        this.pr.title.update(cx, |field, cx| field.set_text("", cx));
                        this.pr.body.update(cx, |field, cx| field.set_text("", cx));
                    }
                    Ok(None) => {
                        this.pr.result =
                            Some((t("Resposta inesperada do serviço.").into(), true, None))
                    }
                    Err(error) => this.pr.result = Some((error, true, None)),
                }
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Asks which pull request (or merge request) the branch has, at most once a minute unless
    /// `now` (after a push or a new request).
    fn check_pull_request(&mut self, now: bool, cx: &mut Context<Self>) {
        if !now
            && self
                .pr
                .asked
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(60))
        {
            return;
        }
        self.pr.asked = Some(std::time::Instant::now());
        let rx = self.backend.request(
            "session_pull_request",
            json!({ "sessionId": self.session_id }),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(value) = reply {
                    let found = found_of(&value);
                    if found != this.pr.found {
                        this.pr.found = found;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// The Criar PR row under the commit field: a button, then its form.
    fn pull_request_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let merge_request = self.pr.found.as_ref().is_some_and(Found::merge_request);
        let create_label = if merge_request {
            t("Criar MR")
        } else {
            t("Criar PR")
        };
        let small = |id: &'static str, label: &'static str, main: bool, enabled: bool| {
            let base = div()
                .id(id)
                .h(px(26.))
                .px(px(10.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .flex_shrink_0()
                .text_size(px(12.))
                .child(label);
            let base = if main {
                base.bg(accent()).text_color(on_accent())
            } else {
                base.bg(content(0.07)).text_color(content(0.85))
            };
            if enabled {
                base.cursor_pointer().hover(|style| style.opacity(0.9))
            } else {
                base.opacity(0.4)
            }
        };
        let mut row = div().flex().flex_col().gap(px(6.)).px(px(8.)).pb(px(8.));
        if let Some(found) = &self.pr.found {
            if let Some(request) = &found.request {
                let mut line = div().flex().items_center().gap(px(8.)).child(
                    div()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(if request.failed > 0 {
                            rgb(0xe08a8a).into()
                        } else {
                            content(0.6)
                        })
                        .child(found_line(found, request)),
                );
                if let Some(url) = crate::markdown::web_url(&request.url) {
                    line = line.child(
                        div()
                            .id("pr-current-open")
                            .text_size(px(12.))
                            .text_color(accent())
                            .cursor_pointer()
                            .on_click(move |_, _, cx| cx.open_url(&url))
                            .child(t("Abrir")),
                    );
                }
                row = row.child(line);
            } else if let Some(hint) = found.hint {
                row = row.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(content(0.5))
                        .child(hint),
                );
            }
        }
        if let Some((text, failed, url)) = &self.pr.result {
            let mut line = div().flex().items_center().gap(px(8.)).child(
                div()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(if *failed {
                        rgb(0xe08a8a).into()
                    } else {
                        content(0.6)
                    })
                    .child(text.clone()),
            );
            if let Some(url) = url.clone().and_then(|url| crate::markdown::web_url(&url)) {
                line = line.child(
                    div()
                        .id("pr-open")
                        .text_size(px(12.))
                        .text_color(accent())
                        .cursor_pointer()
                        .on_click(move |_, _, cx| cx.open_url(&url))
                        .child(if merge_request {
                            t("Abrir MR")
                        } else {
                            t("Abrir PR")
                        }),
                );
            }
            row = row.child(line);
        }
        if !self.pr.open {
            return row.into_any_element();
        }
        let typed = !self.pr.title.read(cx).text().trim().is_empty();
        let enabled = typed && !self.pr.creating;
        let toggle = div()
            .id("pr-draft")
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(content(0.7))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.pr.draft = !this.pr.draft;
                cx.notify();
            }))
            .child(
                div()
                    .size(px(14.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(content(0.3))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(self.pr.draft, |d| {
                        d.bg(accent())
                            .border_color(accent())
                            .child(svg().path("check").size(px(12.)).text_color(on_accent()))
                    }),
            )
            .child(t("Abrir como rascunho"));
        row.child(self.pr.title.clone())
            .child(self.pr.body.clone())
            .child(toggle)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(content(0.4))
                    .child(if merge_request {
                        t("Envia este branch e abre o merge request para o branch padrão.")
                    } else {
                        t("Envia este branch e abre o pull request para o branch padrão.")
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        small("pr-cancel", t("Cancelar"), false, !self.pr.creating).on_click(
                            cx.listener(|this, _, _, cx| {
                                if !this.pr.creating {
                                    this.pr.open = false;
                                    cx.notify();
                                }
                            }),
                        ),
                    )
                    .child(
                        small(
                            "pr-submit",
                            if self.pr.creating {
                                t("Criando…")
                            } else {
                                create_label
                            },
                            true,
                            enabled,
                        )
                        .when(enabled, |b| {
                            b.on_click(cx.listener(|this, _, _, cx| this.create_pull_request(cx)))
                        }),
                    ),
            )
            .into_any_element()
    }

    /// Files changed on disk (an agent, a terminal, an editor): read the status again, unless a
    /// read or a commit is already on its way, which will see them.
    fn disk_changed(&mut self, cx: &mut Context<Self>) {
        if !self.loading && !self.committing {
            self.refresh(cx);
        }
    }

    /// Reads the workspace's status and the line counts; both are fresh when this returns.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let status = self.backend.request(
            "git_workspace_action",
            json!({ "action": { "type": "snapshot", "sessionId": self.session_id } }),
        );
        let counts = self
            .backend
            .request("git_status", json!({ "sessionId": self.session_id }));
        cx.spawn(async move |this, cx| {
            let snapshot = settle(status).await;
            let counts = settle(counts).await.ok();
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match snapshot {
                    Ok(value) => {
                        if let Some(snapshot) = snapshot_of(&value) {
                            this.snapshot = Some(snapshot);
                        }
                    }
                    Err(error) => this.error = Some(error),
                }
                if let Some(counts) = counts {
                    this.counts = counts_of(&counts);
                }
                if this.snapshot.as_ref().is_some_and(|s| s.branch.is_some()) {
                    this.check_pull_request(false, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Moves one file into or out of the index. A stale index is refused by the back end, so
    /// every attempt ends with a fresh read.
    fn move_path(&mut self, path: String, staged: bool, cx: &mut Context<Self>) {
        let Some(token) = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.token.clone())
        else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let action = if staged { "unstage" } else { "stage" };
        let rx = self.backend.request(
            "git_workspace_action",
            json!({ "action": { "type": action, "sessionId": self.session_id, "paths": [path], "expectedIndex": token } }),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if let Err(error) = reply {
                    this.error = Some(error);
                }
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn open_diff(&mut self, path: String, staged: bool, cx: &mut Context<Self>) {
        let Some(token) = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.token.clone())
        else {
            return;
        };
        self.opened = Some(Opened {
            path: path.clone(),
            staged,
            lines: Lines::Loading,
        });
        let rx = self.backend.request(
            "git_workspace_action",
            json!({ "action": { "type": "diff", "sessionId": self.session_id, "path": path.clone(), "staged": staged, "expectedIndex": token } }),
        );
        cx.spawn(async move |this, cx| {
            let lines = match settle(rx).await {
                Ok(value) => Lines::Ready(Rc::new(
                    value["diff"]
                        .as_str()
                        .unwrap_or("")
                        .lines()
                        .map(str::to_owned)
                        .collect(),
                )),
                Err(error) => Lines::Failed(error),
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(opened) = this
                    .opened
                    .as_mut()
                    .filter(|opened| opened.path == path && opened.staged == staged)
                {
                    opened.lines = lines;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// One changed file, as Monocode draws it: its icon, name, the folder dimmed and cut to fit,
    /// the line counts and the status letter. A click opens the diff; the end button moves the
    /// file between Preparadas and Alterações.
    fn change_row(&self, entry: &Entry, staged: bool, cx: &mut Context<Self>) -> AnyElement {
        let (added, removed) = diff_colors(0);
        let (add, remove) = self.counts.get(&entry.path).copied().unwrap_or((0, 0));
        let (dir, name) = split_path(&entry.path);
        let (letter, letter_color) = kind_mark(&entry.kind, added, removed);
        let (open_path, stage_path) = (entry.path.clone(), entry.path.clone());
        let enabled = entry.actionable && !self.busy;
        let icon = crate::file_icons::file_icon(&name, false, 14.);
        let mut mover = div()
            .id(format!("move:{staged}:{}", entry.path))
            .size(px(20.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .tooltip(crate::shell::message_actions::name_tip(if staged {
                t("Despreparar")
            } else {
                t("Preparar")
            }))
            .child(
                svg()
                    .path(if staged { "minus" } else { "plus" })
                    .size(px(12.))
                    .text_color(content(0.6))
                    .flex_shrink_0(),
            );
        let discard_path = entry.path.clone();
        let discard_title = tf("Descartar alterações em {}?", &[&name]);
        let discarder = (!staged).then(|| {
            let button = div()
                .id(format!("discard:{}", entry.path))
                .size(px(20.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .opacity(0.)
                .group_hover(SharedString::from("change-row"), |style| style.opacity(1.))
                .tooltip(crate::shell::message_actions::name_tip(t("Descartar")))
                .child(
                    svg()
                        .path("undo")
                        .size(px(12.))
                        .text_color(content(0.6))
                        .flex_shrink_0(),
                );
            if enabled {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(content(0.1)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.ask_discard(discard_title.clone(), vec![discard_path.clone()], cx)
                    }))
            } else {
                button
            }
        });
        mover =
            if enabled {
                mover
                    .cursor_pointer()
                    .hover(|style| style.bg(content(0.1)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.move_path(stage_path.clone(), staged, cx)
                    }))
            } else {
                mover.opacity(0.35)
            };
        div()
            .id(format!("open:{staged}:{}", entry.path))
            .group("change-row")
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(14.))
            .py(px(4.))
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|style| style.bg(content(0.05)))
            .on_click(
                cx.listener(move |this, _, _, cx| this.open_diff(open_path.clone(), staged, cx)),
            )
            .child(icon)
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.5))
                    .text_color(content(0.9))
                    .child(name),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(content(0.38))
                    .child(dir),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_family("Menlo")
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .child(counts_text(add, remove)),
            )
            .child(
                div()
                    .w(px(12.))
                    .flex_shrink_0()
                    .font_family("Menlo")
                    .text_size(px(11.))
                    .text_color(letter_color)
                    .child(letter),
            )
            .children(discarder)
            .child(mover)
            .into_any_element()
    }

    /// A section header of the changes view: a fold arrow, the name, a count pill and, on the
    /// right, the section's buttons.
    fn section_header(
        &self,
        id: &'static str,
        title: &str,
        count: usize,
        open: bool,
        tools: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(14.))
            .py(px(6.))
            .child(
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path(if open {
                                "chevron-down"
                            } else {
                                "chevron-right"
                            })
                            .size(px(12.))
                            .text_color(content(0.5))
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(content(0.6))
                            .child(title.to_owned()),
                    )
                    .child(
                        div()
                            .px(px(7.))
                            .rounded(px(99.))
                            .bg(accent_wash(0.25))
                            .text_size(px(10.5))
                            .text_color(content(0.85))
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match id {
                            "fold-staged" => this.staged_open = !this.staged_open,
                            _ => this.changes_open = !this.changes_open,
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .children(tools),
            )
            .into_any_element()
    }

    /// The history tab's body: only the Histórico.
    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut blocks = Vec::new();
        match &self.snapshot {
            None => blocks.push(note(t("Carregando…"), false)),
            Some(snapshot) if !snapshot.repo => {
                blocks.push(note(t("Esta pasta não é um repositório Git."), false))
            }
            Some(snapshot) => blocks.push(self.history(snapshot, cx)),
        }
        div()
            .id("changes-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(px(4.))
            .flex()
            .flex_col()
            .children(blocks)
            .into_any_element()
    }

    /// The changes view's body, under the message field and the Commit button: Preparadas and
    /// Alterações, each foldable, or "Nenhuma alteração".
    fn review_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut blocks = Vec::new();
        match &self.snapshot {
            None => blocks.push(note(t("Carregando…"), false)),
            Some(snapshot) if !snapshot.repo => {
                blocks.push(note(t("Esta pasta não é um repositório Git."), false))
            }
            Some(snapshot) => {
                if snapshot.staged.is_empty() && snapshot.unstaged.is_empty() {
                    blocks.push(note(t("Nenhuma alteração"), false));
                }
                if !snapshot.staged.is_empty() {
                    let unstage_all = self.tool_button(
                        "unstage-all",
                        "minus",
                        t("Despreparar tudo"),
                        !self.busy && !actionable_paths(&snapshot.staged).is_empty(),
                        cx.listener(|this, _, _, cx| this.move_all(true, cx)),
                    );
                    blocks.push(self.section_header(
                        "fold-staged",
                        t("PREPARADAS"),
                        snapshot.staged.len(),
                        self.staged_open,
                        vec![unstage_all],
                        cx,
                    ));
                    if self.staged_open {
                        blocks.extend(
                            snapshot
                                .staged
                                .iter()
                                .map(|entry| self.change_row(entry, true, cx)),
                        );
                    }
                }
                if !snapshot.unstaged.is_empty() {
                    let stage_all = self.tool_button(
                        "stage-all",
                        "square-plus",
                        t("Preparar tudo"),
                        !self.busy && !actionable_paths(&snapshot.unstaged).is_empty(),
                        cx.listener(|this, _, _, cx| this.move_all(false, cx)),
                    );
                    let discard_all = self.tool_button(
                        "discard-all",
                        "undo",
                        t("Descartar tudo"),
                        !self.busy && !actionable_paths(&snapshot.unstaged).is_empty(),
                        cx.listener(|this, _, _, cx| {
                            let paths = this
                                .snapshot
                                .as_ref()
                                .map(|snapshot| actionable_paths(&snapshot.unstaged))
                                .unwrap_or_default();
                            this.ask_discard(
                                t("Descartar todas as alterações?").to_owned(),
                                paths,
                                cx,
                            );
                        }),
                    );
                    blocks.push(self.section_header(
                        "fold-changes",
                        t("ALTERAÇÕES"),
                        snapshot.unstaged.len(),
                        self.changes_open,
                        vec![discard_all, stage_all],
                        cx,
                    ));
                    if self.changes_open {
                        blocks.extend(
                            snapshot
                                .unstaged
                                .iter()
                                .map(|entry| self.change_row(entry, false, cx)),
                        );
                    }
                }
                if snapshot.conflicts {
                    blocks.push(note(t("Há conflitos: resolva-os antes de preparar."), true));
                }
                if snapshot.truncated {
                    blocks.push(note(
                        t("A lista está incompleta: há mais arquivos do que o limite."),
                        false,
                    ));
                }
            }
        }
        div()
            .id("review-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pb(px(8.))
            .flex()
            .flex_col()
            .children(blocks)
            .into_any_element()
    }

    /// A small icon button of a section header.
    fn tool_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        tip: &'static str,
        enabled: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let button = div()
            .id(id)
            .size(px(22.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .tooltip(crate::shell::message_actions::name_tip(tip))
            .child(
                svg()
                    .path(glyph)
                    .size(px(14.))
                    .text_color(content(0.55))
                    .flex_shrink_0(),
            );
        if enabled {
            button
                .cursor_pointer()
                .hover(|style| style.bg(content(0.08)))
                .on_click(on_click)
                .into_any_element()
        } else {
            button.opacity(0.35).into_any_element()
        }
    }

    /// Opens the confirmation for discarding `paths`; nothing is lost until it is accepted.
    fn ask_discard(&mut self, title: String, paths: Vec<String>, cx: &mut Context<Self>) {
        if paths.is_empty() || self.busy {
            return;
        }
        self.discard = Some(DiscardAsk { title, paths });
        cx.notify();
    }

    /// Discards the confirmed files through the host's `discard` action (staged and unstaged
    /// changes of those paths; files Git does not hold are deleted, which the card announced).
    fn discard_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(ask) = self.discard.take() else {
            return;
        };
        let Some(token) = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.token.clone())
        else {
            cx.notify();
            return;
        };
        if self.busy {
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        let rx = self.backend.request(
            "git_workspace_action",
            json!({ "action": discard_action(&self.session_id, &ask.paths, &token) }),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if let Err(error) = reply {
                    this.error = Some(error);
                }
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// The confirmation card, over the whole panel: Cancelar or the danger-colored Descartar.
    fn discard_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ask = self.discard.as_ref()?;
        let danger: Hsla = rgb(0xec8a85).into();
        Some(
            div()
                .id("discard-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::Hsla {
                    a: 0.45,
                    ..gpui::black()
                })
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.discard = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("discard-card")
                        .relative()
                        .w(px(280.))
                        .p(px(18.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(content(0.12))
                        .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                        .shadow_lg()
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .size(px(38.))
                                .mb(px(10.))
                                .rounded_full()
                                .bg(gpui::Hsla { a: 0.12, ..danger })
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(crate::theme::icon("undo", 18.).text_color(danger)),
                        )
                        .child(
                            div()
                                .text_center()
                                .text_size(px(15.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(content(0.97))
                                .child(ask.title.clone()),
                        )
                        .child(
                            div()
                                .mt(px(6.))
                                .text_center()
                                .text_size(px(12.5))
                                .line_height(px(18.))
                                .text_color(content(0.6))
                                .child(t(
                                    "As alterações que ainda não viraram commit serão perdidas. Não dá para desfazer.",
                                )),
                        )
                        .child(
                            div()
                                .mt(px(16.))
                                .w_full()
                                .flex()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .id("discard-cancel")
                                        .flex_1()
                                        .h(px(30.))
                                        .rounded(px(crate::theme::RADIUS_MD))
                                        .bg(content(0.06))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(12.5))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(content(0.8))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(content(0.1)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.discard = None;
                                            cx.notify();
                                        }))
                                        .child(t("Cancelar")),
                                )
                                .child(
                                    div()
                                        .id("discard-yes")
                                        .flex_1()
                                        .h(px(30.))
                                        .rounded(px(crate::theme::RADIUS_MD))
                                        .bg(rgb(0xc9504a))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(12.5))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(gpui::white())
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0xd5605a)))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.discard_confirmed(cx)),
                                        )
                                        .child(t("Descartar")),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Stages every actionable unstaged file, or unstages every staged one, in one request.
    fn move_all(&mut self, staged: bool, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        let paths = actionable_paths(if staged {
            &snapshot.staged
        } else {
            &snapshot.unstaged
        });
        if paths.is_empty() || self.busy {
            return;
        }
        let token = snapshot.token.clone();
        self.busy = true;
        self.error = None;
        let action = if staged { "unstage" } else { "stage" };
        let rx = self.backend.request(
            "git_workspace_action",
            json!({ "action": { "type": action, "sessionId": self.session_id, "paths": paths, "expectedIndex": token } }),
        );
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if let Err(error) = reply {
                    this.error = Some(error);
                }
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// The Histórico: the snapshot's commits, then the pages loaded since, one graph row each.
    fn history(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> AnyElement {
        let older: &[Commit] = match &self.older {
            Some(older) if snapshot.head.as_deref() == Some(older.from.as_str()) => &older.entries,
            _ => &[],
        };
        let truncated = match &self.older {
            Some(older) if snapshot.head.as_deref() == Some(older.from.as_str()) => older.truncated,
            _ => snapshot.history_truncated,
        };
        let commits: Vec<&Commit> = snapshot.history.iter().chain(older.iter()).collect();
        let count = commits.len();
        let mut section = div().flex().flex_col().pt(px(8.)).pb(px(6.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(14.))
                .pb(px(6.))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(content(0.6))
                        .child(t("HISTÓRICO")),
                )
                .child(
                    div()
                        .px(px(7.))
                        .rounded(px(99.))
                        .bg(accent_wash(0.25))
                        .text_size(px(10.5))
                        .text_color(content(0.85))
                        .child(count.to_string()),
                ),
        );
        if commits.is_empty() {
            section = section.child(note(t("Nenhum commit ainda."), false));
        }
        let graph = crate::git_graph::rows(
            commits
                .iter()
                .map(|commit| (commit.hash.as_str(), commit.parents.as_slice())),
        );
        let lanes = graph
            .iter()
            .flat_map(|row| {
                row.above
                    .iter()
                    .chain(&row.below)
                    .flat_map(|(from, to)| [*from, *to])
                    .chain([row.lane])
            })
            .max()
            .map_or(1, |lane| lane + 1)
            .min(MAX_LANES);
        for (index, commit) in commits.iter().enumerate() {
            let picked = (*commit).clone();
            section = section.child(
                div()
                    .id(format!("commit:{}", commit.hash))
                    .cursor_pointer()
                    .hover(|style| style.bg(content(0.05)))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_commit(picked.clone(), cx)),
                    )
                    .child(commit_row(
                        commit,
                        snapshot.head.as_deref() == Some(commit.hash.as_str()),
                        &graph[index],
                        lanes,
                    )),
            );
        }
        if truncated {
            let loading = self.loading_older;
            section = section.child(
                div()
                    .id("history-more")
                    .mx(px(12.))
                    .mt(px(4.))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(content(if loading { 0.35 } else { 0.7 }))
                    .when(!loading, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(content(0.07)))
                            .on_click(cx.listener(|this, _, _, cx| this.load_more(cx)))
                    })
                    .child(if loading {
                        t("Carregando…")
                    } else {
                        t("Carregar mais")
                    }),
            );
        }
        section.into_any_element()
    }

    /// A row of a floating menu.
    fn menu_item(
        &self,
        id: &'static str,
        label: &str,
        checked: bool,
        enabled: bool,
    ) -> Stateful<Div> {
        let row = div()
            .id(id)
            .px(px(9.))
            .py(px(6.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.5))
            .text_color(content(0.85))
            .child(div().flex_1().child(label.to_owned()))
            .when(checked, |d| {
                d.child(
                    svg()
                        .path("check")
                        .size(px(12.))
                        .text_color(accent())
                        .flex_shrink_0(),
                )
            });
        if enabled {
            row.cursor_pointer().hover(|style| style.bg(content(0.08)))
        } else {
            row.opacity(0.4)
        }
    }

    /// The branch's "⋯" menu or the Commit button's "▾" menu, floating over the view, with a
    /// backdrop that closes it.
    fn menu_overlay(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu?;
        let typed = !self.commit.read(cx).text().trim().is_empty();
        let can_amend =
            snapshot.ahead.is_none_or(|ahead| ahead > 0) && !snapshot.history.is_empty();
        let amending = self.amending && can_amend;
        let ready = commit_ready(
            typed,
            amending,
            self.committing,
            snapshot.staged.is_empty(),
            snapshot.token.is_empty(),
        );
        let can_push = can_push(snapshot.repo, snapshot.detached, &snapshot.head);
        let has_open = self
            .pr
            .found
            .as_ref()
            .and_then(|found| found.request.as_ref())
            .is_some_and(|request| request.state == "open");
        let merge_request = self.pr.found.as_ref().is_some_and(Found::merge_request);
        let mut items: Vec<AnyElement> = Vec::new();
        let top;
        match menu {
            Menu::Branch => {
                top = TITLE + 4.;
                items.push(
                    self.menu_item("menu-refresh", t("Atualizar"), false, true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            this.refresh(cx);
                        }))
                        .into_any_element(),
                );
            }
            Menu::Commit => {
                top = TITLE + COMMIT_BAR;
                let both_on = ready && can_push;
                items.push(
                    self.menu_item("menu-commit-push", t("Commit e Push"), false, both_on)
                        .when(both_on, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                this.commit_with(true, cx);
                            }))
                        })
                        .into_any_element(),
                );
                let push_on = can_push && !self.push.pushing;
                items.push(
                    self.menu_item("menu-push", t("Push"), false, push_on)
                        .when(push_on, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                this.push.confirm = true;
                                this.push.result = None;
                                cx.notify();
                            }))
                        })
                        .into_any_element(),
                );
                let pr_on = snapshot.branch.is_some() && !has_open && !self.pr.open;
                items.push(
                    self.menu_item(
                        "menu-pr",
                        if merge_request {
                            t("Criar MR")
                        } else {
                            t("Criar PR")
                        },
                        false,
                        pr_on,
                    )
                    .when(pr_on, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            this.pr.open = true;
                            this.pr.result = None;
                            this.pr.title.update(cx, |field, cx| field.focus(cx));
                            cx.notify();
                        }))
                    })
                    .into_any_element(),
                );
                if can_amend {
                    items.push(
                        self.menu_item("menu-amend", t("Corrigir o último"), amending, true)
                            .tooltip(crate::shell::message_actions::name_tip(t(
                                "O próximo commit substitui o último, que ainda não foi enviado. Sem título, mantém a mensagem dele.",
                            )))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.menu = None;
                                this.amending = !this.amending;
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
            }
        }
        Some(
            div()
                .id("changes-menu-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .absolute()
                        .top(px(top))
                        .right(px(12.))
                        .w(px(200.))
                        .p(px(5.))
                        .rounded(px(crate::theme::RADIUS_MD))
                        .bg(crate::theme::opaque_floating())
                        .border_1()
                        .border_color(content(0.1))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .children(items),
                )
                .into_any_element(),
        )
    }

    /// The changes view's top: the message field with its ✦, the split Commit button, and what
    /// the last commit, push or pull request said.
    fn commit_bar(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> AnyElement {
        let typed = !self.commit.read(cx).text().trim().is_empty();
        let can_amend =
            snapshot.ahead.is_none_or(|ahead| ahead > 0) && !snapshot.history.is_empty();
        let amending = self.amending && can_amend;
        let ready = commit_ready(
            typed,
            amending,
            self.committing,
            snapshot.staged.is_empty(),
            snapshot.token.is_empty(),
        );
        let busy_title = self.title_job.is_some();
        let sparkle_on = !busy_title && !self.committing && !snapshot.staged.is_empty();
        let sparkle = div()
            .id("commit-generate")
            .size(px(24.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .child(
                svg()
                    .path(if busy_title { "square" } else { "sparkles" })
                    .size(px(14.))
                    .text_color(content(0.6))
                    .flex_shrink_0(),
            );
        let sparkle = if busy_title {
            let cancelling = self.title_job.as_ref().is_some_and(|job| job.cancelling);
            sparkle
                .opacity(if cancelling { 0.4 } else { 1.0 })
                .tooltip(crate::shell::message_actions::name_tip(t(
                    "Cancelar geração do título",
                )))
                .when(!cancelling, |d| {
                    d.cursor_pointer()
                        .hover(|style| style.bg(content(0.08)))
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_title(cx)))
                })
        } else if sparkle_on {
            sparkle
                .tooltip(crate::shell::message_actions::name_tip(t(
                    "Gerar título do commit",
                )))
                .cursor_pointer()
                .hover(|style| style.bg(content(0.08)))
                .on_click(cx.listener(|this, _, _, cx| this.generate_title(cx)))
        } else {
            sparkle.opacity(0.4)
        };
        let field = div()
            .mx(px(12.))
            .mt(px(8.))
            .h(px(34.))
            .px(px(10.))
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(content(0.05))
            .border_1()
            .border_color(content(0.08))
            .flex()
            .items_center()
            .gap(px(6.))
            .child(div().flex_1().min_w_0().child(self.commit.clone()))
            .child(sparkle);
        let main = div()
            .id("commit-button")
            .flex_1()
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .text_size(px(13.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .rounded_l(px(crate::theme::RADIUS_MD))
            .text_color(content(if ready { 0.92 } else { 0.4 }))
            .child(
                svg()
                    .path("check")
                    .size(px(14.))
                    .text_color(content(if ready { 0.92 } else { 0.4 }))
                    .flex_shrink_0(),
            )
            .child(if self.committing {
                t("Enviando…")
            } else if amending {
                t("Corrigir")
            } else {
                "Commit"
            });
        let main = if ready {
            main.cursor_pointer()
                .hover(|style| style.bg(content(0.06)))
                .on_click(cx.listener(|this, _, _, cx| this.commit(cx)))
        } else {
            main
        };
        let dropdown = div()
            .id("commit-menu")
            .w(px(34.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded_r(px(crate::theme::RADIUS_MD))
            .border_l_1()
            .border_color(content(0.12))
            .cursor_pointer()
            .hover(|style| style.bg(content(0.06)))
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu = if this.menu == Some(Menu::Commit) {
                    None
                } else {
                    Some(Menu::Commit)
                };
                cx.notify();
            }))
            .child(
                svg()
                    .path("chevron-down")
                    .size(px(14.))
                    .text_color(content(0.7))
                    .flex_shrink_0(),
            );
        let split = div()
            .mx(px(12.))
            .mt(px(8.))
            .mb(px(8.))
            .h(px(32.))
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(content(0.05))
            .border_1()
            .border_color(content(0.12))
            .flex()
            .child(main)
            .child(dropdown);
        let mut bar = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(field)
            .child(split);
        if let Some(error) = &self.error {
            bar = bar.child(note(error, true));
        }
        if let Some(text) = &self.title_note {
            bar = bar.child(note(text, false));
        }
        if self.push.confirm {
            let branch = snapshot.branch.clone().unwrap_or_default();
            let pushing = self.push.pushing;
            let button = |id: &'static str, label: &'static str, enabled: bool| {
                let base = div()
                    .id(id)
                    .h(px(26.))
                    .px(px(10.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .bg(content(0.07))
                    .text_color(content(0.85))
                    .child(label);
                if enabled {
                    base.cursor_pointer().hover(|style| style.opacity(0.9))
                } else {
                    base.opacity(0.4)
                }
            };
            bar = bar.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(12.))
                    .pb(px(8.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.7))
                            .child(tf("Enviar {} para o remoto?", &[&branch])),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(6.))
                            .child(button("push-cancel", t("Cancelar"), !pushing).when(
                                !pushing,
                                |b| {
                                    b.on_click(cx.listener(|this, _, _, cx| {
                                        this.push.confirm = false;
                                        cx.notify();
                                    }))
                                },
                            ))
                            .child(
                                button(
                                    "push-confirm",
                                    if pushing {
                                        t("Enviando…")
                                    } else {
                                        t("Confirmar push")
                                    },
                                    !pushing,
                                )
                                .when(!pushing, |b| {
                                    b.on_click(cx.listener(|this, _, _, cx| this.push_now(cx)))
                                }),
                            ),
                    ),
            );
        }
        if let Some((text, failed)) = &self.push.result {
            bar = bar.child(note(text, *failed));
        }
        bar.when(snapshot.branch.is_some(), |bar| {
            bar.child(self.pull_request_row(cx))
        })
        .into_any_element()
    }

    /// An opened commit: who and when, a summary of its lines, and its files, each opening its diff.
    fn commit_files(&self, view: &CommitView, cx: &mut Context<Self>) -> AnyElement {
        let files = match &view.files {
            CommitFilesState::Loading => {
                return div()
                    .flex_1()
                    .child(note(t("Carregando…"), false))
                    .into_any_element();
            }
            CommitFilesState::Failed(error) => {
                return div().flex_1().child(note(error, true)).into_any_element();
            }
            CommitFilesState::Ready(files) => files,
        };
        let (added, removed) = diff_colors(0);
        let total = |pick: fn(&local_git::CommitFile) -> Option<u64>| {
            files.files.iter().filter_map(pick).sum::<u64>()
        };
        let (adds, removes) = (total(|file| file.added), total(|file| file.removed));
        let by = match (view.commit.author.is_empty(), age(&view.commit.when)) {
            (true, age) => age,
            (false, age) if age.is_empty() => view.commit.author.clone(),
            (false, age) => format!("{} · {age}", view.commit.author),
        };
        let count = files.files.len();
        let summary = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(11.5))
            .text_color(content(0.5))
            .child(div().child(if count == 1 {
                t("1 arquivo").to_owned()
            } else {
                tf("{} arquivos", &[&count])
            }))
            .child(
                div()
                    .font_family("Menlo")
                    .text_color(added)
                    .child(format!("+{adds}")),
            )
            .child(
                div()
                    .font_family("Menlo")
                    .text_color(removed)
                    .child(format!("−{removes}")),
            );
        let mut head = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(content(0.07))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(content(0.92))
                    .child(view.commit.subject.clone()),
            );
        if !by.is_empty() {
            head = head.child(div().text_size(px(11.5)).text_color(content(0.4)).child(by));
        }
        head = head.child(summary);
        if files.merge {
            head = head.child(div().text_size(px(11.5)).text_color(content(0.4)).child(t(
                "Mesclagem: arquivos alterados em relação ao primeiro pai.",
            )));
        }
        let mut rows: Vec<AnyElement> = files
            .files
            .iter()
            .map(|file| commit_file_row(file, added, removed, cx))
            .collect();
        if files.files.is_empty() {
            rows.push(note(t("Nenhum arquivo alterado."), false));
        }
        if files.truncated {
            rows.push(note(
                t("A lista está incompleta: há mais arquivos do que o limite."),
                false,
            ));
        }
        div()
            .id("commit-files")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(head)
            .child(div().flex().flex_col().py(px(4.)).children(rows))
            .into_any_element()
    }

    fn diff(&self, lines: &Lines) -> AnyElement {
        match lines {
            Lines::Loading => div()
                .flex_1()
                .child(note(t("Carregando…"), false))
                .into_any_element(),
            Lines::Failed(error) => div().flex_1().child(note(error, true)).into_any_element(),
            Lines::Ready(lines) if lines.is_empty() => div()
                .flex_1()
                .child(note(t("Sem diferenças para mostrar."), false))
                .into_any_element(),
            Lines::Ready(lines) => {
                let lines = lines.clone();
                uniform_list("diff-lines", lines.len(), move |range, _window, _cx| {
                    range
                        .map(|index| diff_line(&lines[index]))
                        .collect::<Vec<_>>()
                })
                .flex_1()
                .min_h_0()
                .into_any_element()
            }
        }
    }
}

impl Render for ChangesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (header, body) = match (&self.commit_view, &self.opened) {
            (Some(view), _) => {
                let back = div()
                    .id("commit-back")
                    .size(px(24.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .hover(|style| style.bg(content(0.08)))
                    .on_click(cx.listener(|this, _, _, cx| this.commit_back(cx)))
                    .child(
                        svg()
                            .path("arrow-left")
                            .size(px(14.))
                            .text_color(content(0.6))
                            .flex_shrink_0(),
                    );
                let short: String = view.commit.hash.chars().take(7).collect();
                let (title, tag) = match &view.diff {
                    Some(diff) => (diff.path.clone(), short),
                    None => (view.commit.subject.clone(), short),
                };
                let header = div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .child(back)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(content(0.9))
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family("Menlo")
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child(tag),
                    );
                let body = match &view.diff {
                    Some(diff) => self.diff(&diff.lines),
                    None => self.commit_files(view, cx),
                };
                (header.into_any_element(), body)
            }
            (None, Some(opened)) => {
                let back = div()
                    .id("changes-back")
                    .size(px(24.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .hover(|style| style.bg(content(0.08)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.opened = None;
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path("arrow-left")
                            .size(px(14.))
                            .text_color(content(0.6))
                            .flex_shrink_0(),
                    );
                let label = if opened.staged {
                    t("Preparado")
                } else {
                    t("Alterações")
                };
                let header = div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .child(back)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(content(0.9))
                            .child(opened.path.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child(label),
                    );
                (header.into_any_element(), self.diff(&opened.lines))
            }
            (None, None) if self.mode == Mode::Review => {
                let name = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.branch.clone())
                    .unwrap_or_else(|| t("Alterações").into());
                let more = div()
                    .id("branch-menu")
                    .size(px(24.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .hover(|style| style.bg(content(0.08)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu = if this.menu == Some(Menu::Branch) {
                            None
                        } else {
                            Some(Menu::Branch)
                        };
                        cx.notify();
                    }))
                    .child(
                        svg()
                            .path("more-horizontal")
                            .size(px(14.))
                            .text_color(content(0.6))
                            .flex_shrink_0(),
                    );
                let header = div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .child(
                        svg()
                            .path("branch")
                            .size(px(14.))
                            .text_color(content(0.6))
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(content(0.85))
                            .child(name),
                    )
                    .child(div().ml_auto().child(more));
                let bar = match &self.snapshot {
                    Some(snapshot) if snapshot.repo => self.commit_bar(snapshot, cx),
                    _ => div().into_any_element(),
                };
                let body = div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(bar)
                    .child(self.review_list(cx));
                (header.into_any_element(), body.into_any_element())
            }
            (None, None) => {
                let branch = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| {
                        let name = snapshot.branch.clone()?;
                        Some(match (snapshot.ahead, snapshot.behind) {
                            (Some(ahead), Some(behind)) => format!("{name}  ↑{ahead} ↓{behind}"),
                            _ => name,
                        })
                    })
                    .unwrap_or_else(|| t("Alterações").into());
                let status = if self.loading {
                    t("Atualizando…")
                } else {
                    ""
                };
                let refresh = div()
                    .id("changes-refresh")
                    .size(px(24.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .hover(|style| style.bg(content(0.08)))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                    .child(
                        svg()
                            .path("refresh")
                            .size(px(14.))
                            .text_color(content(0.6))
                            .flex_shrink_0(),
                    );
                let header = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(content(0.7))
                            .child(branch),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(content(0.4))
                                    .child(status),
                            )
                            .child(refresh),
                    );
                let body = div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(self.list(cx));
                (header.into_any_element(), body.into_any_element())
            }
        };
        let overlay = match (&self.snapshot, self.mode, self.menu) {
            (Some(snapshot), Mode::Review, Some(_)) => self.menu_overlay(snapshot, cx),
            _ => None,
        };
        let confirm = if self.mode == Mode::Review {
            self.discard_overlay(cx)
        } else {
            None
        };
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(TITLE))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(content(0.07))
                    .child(header),
            )
            .child(body)
            .children(overlay)
            .children(confirm)
    }
}

/// Resolves a back end request: the value, or the error's message.
async fn settle(rx: async_channel::Receiver<Result<Value, Value>>) -> Result<Value, String> {
    match rx.recv().await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(message(&error)),
        Err(_) => Err(t("O motor do Sirus não respondeu.").into()),
    }
}

fn message(error: &Value) -> String {
    error
        .as_str()
        .or(error["message"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string())
}

/// The status snapshot from a `git_workspace_action` snapshot reply.
fn snapshot_of(value: &Value) -> Option<Snapshot> {
    let snapshot = value.get("snapshot")?;
    Some(Snapshot {
        token: snapshot["indexToken"].as_str().unwrap_or("").to_owned(),
        repo: snapshot["identity"]["isRepo"].as_bool().unwrap_or(false),
        branch: snapshot["identity"]["branch"].as_str().map(str::to_owned),
        detached: snapshot["identity"]["detached"].as_bool().unwrap_or(false),
        ahead: snapshot["ahead"].as_u64(),
        behind: snapshot["behind"].as_u64(),
        head: snapshot["head"].as_str().map(str::to_owned),
        history: commits_of(&snapshot["history"]),
        history_truncated: snapshot["historyTruncated"].as_bool().unwrap_or(false),
        staged: entries(&snapshot["staged"]),
        unstaged: {
            let mut unstaged = entries(&snapshot["unstaged"]);
            // Settings → Git → Mostrar arquivos não rastreados.
            if !crate::prefs::flag("gitShowUntracked", true) {
                unstaged.retain(|entry| entry.kind != "untracked");
            }
            unstaged
        },
        conflicts: snapshot["conflicts"].as_bool().unwrap_or(false),
        truncated: snapshot["truncated"].as_bool().unwrap_or(false),
    })
}

/// `commit_title_action` generate request for the staged index the pane read.
/// The host's `discard` action: every change to `paths`, staged or not. The confirmation card
/// came first, so deleting files Git does not hold is allowed.
fn discard_action(session_id: &str, paths: &[String], token: &str) -> Value {
    json!({
        "type": "discard",
        "sessionId": session_id,
        "paths": paths,
        "expectedIndex": token,
        "confirmDelete": true,
    })
}

fn title_action(session_id: &str, token: &str, request_id: &str) -> Value {
    json!({ "action": {
        "type": "generate", "sessionId": session_id, "expectedIndex": token, "requestId": request_id
    } })
}

fn cancel_title_action(session_id: &str, request_id: &str) -> Value {
    json!({ "action": { "type": "cancel", "sessionId": session_id, "requestId": request_id } })
}

/// What `commit_title_action` answered.
#[derive(Debug, PartialEq)]
enum TitleReply {
    Title {
        title: String,
        provider: String,
        partial: bool,
    },
    Cancelled,
    Invalid,
}

/// Reads the reply with the checks of the React client: a trimmed one-line title of at most 72
/// characters from codex or claude.
fn title_reply(value: &Value) -> TitleReply {
    match value["type"].as_str() {
        Some("cancelled") => TitleReply::Cancelled,
        Some("title") => {
            let title = value["title"].as_str().unwrap_or("");
            let provider = value["provider"].as_str().unwrap_or("");
            let sane = !title.is_empty()
                && title.trim() == title
                && title.chars().count() <= 72
                && !title.chars().any(char::is_control)
                && !title.contains(['\u{2028}', '\u{2029}'])
                && matches!(provider, "codex" | "claude");
            match (sane, value["partial"].as_bool()) {
                (true, Some(partial)) => TitleReply::Title {
                    title: title.to_owned(),
                    provider: provider.to_owned(),
                    partial,
                },
                _ => TitleReply::Invalid,
            }
        }
        _ => TitleReply::Invalid,
    }
}

/// A generated title yields to what the person typed meanwhile, or to a changed index.
fn keeps_typed(started_with: &str, now: &str, token_then: &str, token_now: &str) -> bool {
    started_with != now || token_then != token_now
}

fn attribution(provider: &str, partial: bool) -> String {
    let who = if provider == "codex" {
        "Codex"
    } else {
        "Claude"
    };
    let text = tf("Título escrito por {}", &[&who]);
    if partial {
        format!("{text} · {}", t("parcial"))
    } else {
        text
    }
}

/// A push needs a repository, a commit and a branch (React's `pushBlocked`).
fn can_push(repo: bool, detached: bool, head: &Option<String>) -> bool {
    repo && !detached && head.is_some()
}

fn pushed_text(value: &Value) -> String {
    match value["branch"].as_str() {
        Some(branch) => tf("Enviado {}", &[&branch]),
        None => t("Enviado.").into(),
    }
}

fn entries(list: &Value) -> Vec<Entry> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(Entry {
                path: entry["path"].as_str()?.to_owned(),
                kind: entry["kind"].as_str().unwrap_or("modified").to_owned(),
                actionable: entry["actionable"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// Added and removed counts by path, from a git_status reply's `changes`.
fn counts_of(status: &Value) -> HashMap<String, (u64, u64)> {
    status["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|change| {
            Some((
                change["path"].as_str()?.to_owned(),
                (
                    change["additions"].as_u64().unwrap_or(0),
                    change["deletions"].as_u64().unwrap_or(0),
                ),
            ))
        })
        .collect()
}

/// The one-letter mark of a change kind.
fn kind_letter(kind: &str) -> &'static str {
    match kind {
        "added" => "A",
        "untracked" => "U",
        "deleted" => "D",
        "renamed" => "R",
        _ => "M",
    }
}

/// The one-letter mark of a change kind and its color: M yellow, A green, D red, R blue.
fn kind_mark(kind: &str, added: Hsla, removed: Hsla) -> (&'static str, Hsla) {
    let color = match kind_letter(kind) {
        "A" | "U" => added,
        "D" => removed,
        "R" => rgb(0x58a6ff).into(),
        _ => rgb(0xe9c46a).into(),
    };
    (kind_letter(kind), color)
}

/// A path as its folder and its file name.
fn split_path(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (dir.to_owned(), name.to_owned()),
        None => (String::new(), path.to_owned()),
    }
}

/// The small "+a −d" of a row: a side with nothing on it is left out.
fn counts_text(added: u64, removed: u64) -> String {
    match (added, removed) {
        (0, 0) => String::new(),
        (a, 0) => format!("+{a}"),
        (0, d) => format!("−{d}"),
        (a, d) => format!("+{a} −{d}"),
    }
}

/// The paths Preparar tudo or Despreparar tudo moves: the ones the host lets be moved.
fn actionable_paths(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| entry.actionable)
        .map(|entry| entry.path.clone())
        .collect()
}

/// Whether Commit can run: something typed (or amending, which keeps the old message) and
/// staged files (amending needs none), nothing already sending, and an index to commit from.
fn commit_ready(
    typed: bool,
    amending: bool,
    committing: bool,
    nothing_staged: bool,
    no_token: bool,
) -> bool {
    (amending || typed) && !committing && (amending || !nothing_staged) && !no_token
}

/// One line of a diff: additions green, removals red on a faint wash, hunk headers muted.
pub(crate) fn diff_line(text: &str) -> Div {
    let (added, removed) = diff_colors(0);
    let (color, wash) = if text.starts_with("+++") || text.starts_with("---") {
        (content(0.5), None)
    } else if text.starts_with('+') {
        (added, Some(added))
    } else if text.starts_with('-') {
        (removed, Some(removed))
    } else if text.starts_with("@@") {
        (content(0.45), None)
    } else {
        (content(0.8), None)
    };
    let mut line = div()
        .h(px(18.))
        .px(px(10.))
        .flex()
        .items_center()
        .font_family("Menlo")
        .text_size(px(12.))
        .whitespace_nowrap()
        .text_color(color);
    if let Some(mut wash) = wash {
        wash.a = 0.12;
        line = line.bg(wash);
    }
    line.child(text.to_owned())
}

/// One file of an opened commit: its change letter, name and folder, and its added and removed lines.
fn commit_file_row(
    file: &local_git::CommitFile,
    added: Hsla,
    removed: Hsla,
    cx: &mut Context<ChangesPanel>,
) -> AnyElement {
    let (dir, name) = match file.path.rsplit_once('/') {
        Some((dir, name)) => (dir.to_owned(), name.to_owned()),
        None => (String::new(), file.path.clone()),
    };
    let kind = match file.status {
        'A' => "added",
        'D' => "deleted",
        'R' => "renamed",
        _ => "modified",
    };
    let (letter, letter_color) = kind_mark(kind, added, removed);
    let counts = match (file.added, file.removed) {
        (Some(add), Some(remove)) => div()
            .flex()
            .gap(px(5.))
            .child(div().text_color(added).child(format!("+{add}")))
            .child(div().text_color(removed).child(format!("−{remove}"))),
        _ => div().text_color(content(0.4)).child(t("binário")),
    };
    let path = file.path.clone();
    div()
        .id(format!("commit-file:{}", file.path))
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .py(px(4.))
        .cursor_pointer()
        .hover(|style| style.bg(content(0.05)))
        .on_click(cx.listener(move |this, _, _, cx| this.open_commit_file(path.clone(), cx)))
        .child(
            div()
                .w(px(12.))
                .flex_shrink_0()
                .font_family("Menlo")
                .text_size(px(11.))
                .text_color(letter_color)
                .child(letter),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(content(0.9))
                        .child(name),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child(dir),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .font_family("Menlo")
                .text_size(px(11.))
                .child(counts),
        )
        .into_any_element()
}

pub(crate) fn note(text: &str, failed: bool) -> AnyElement {
    let color: Hsla = if failed {
        rgb(0xe08a8a).into()
    } else {
        content(0.5)
    };
    div()
        .px(px(12.))
        .py(px(6.))
        .text_size(px(12.))
        .text_color(color)
        .child(text.to_owned())
        .into_any_element()
}

#[cfg(test)]
mod found_tests {
    use super::*;

    #[test]
    fn gitlab_merge_request_reads_as_mr_with_its_checks() {
        let reply = json!({"forge": "gitlab", "status": "ready", "pullRequest": {
            "number": 12, "state": "open", "draft": false, "url": "https://gitlab.com/g/p/-/merge_requests/12",
            "checks": [{"status": "passed"}, {"status": "failed"}, {"status": "pending"}]}});
        let found = found_of(&reply).unwrap();
        assert!(found.merge_request());
        let request = found.request.clone().unwrap();
        assert_eq!((request.passed, request.failed, request.pending), (1, 1, 1));
        assert!(found_line(&found, &request).starts_with("MR !12 · "));
    }

    #[test]
    fn hints_only_for_the_other_services() {
        let gitlab = found_of(&json!({"forge": "gitlab", "status": "cliMissing"})).unwrap();
        assert!(gitlab.hint.is_some() && gitlab.request.is_none());
        let bitbucket = found_of(&json!({"forge": "bitbucket", "status": "authRequired"})).unwrap();
        assert!(bitbucket.hint.is_some());
        assert!(found_of(&json!({"forge": "github", "status": "cliMissing"})).is_none());
        let github = found_of(&json!({"forge": "github", "status": "ready",
            "pullRequest": {"number": 3, "state": "merged", "url": "https://github.com/o/r/pull/3", "checks": []}}))
        .unwrap();
        assert!(!github.merge_request());
        assert!(found_line(&github, github.request.as_ref().unwrap()).starts_with("PR #3 · "));
    }
}

fn strings_of(list: &Value) -> Vec<String> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect()
}

/// Commits from a history list (the snapshot's, or a `history` page's).
fn commits_of(list: &Value) -> Vec<Commit> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(Commit {
                hash: entry["hash"].as_str()?.to_owned(),
                parents: strings_of(&entry["parents"]),
                refs: strings_of(&entry["refs"]),
                subject: entry["subject"].as_str().unwrap_or("").to_owned(),
                author: entry["author"].as_str().unwrap_or("").to_owned(),
                when: entry["authoredAt"].as_str().unwrap_or("").to_owned(),
            })
        })
        .collect()
}

/// How long ago a commit was made, in the sidebar's short units ("agora", "12 min", "3 h", "2 d").
pub(crate) fn age(when: &str) -> String {
    let Ok(when) = chrono::DateTime::parse_from_rfc3339(when) else {
        return String::new();
    };
    let minutes = (chrono::Utc::now() - when.to_utc()).num_minutes().max(0);
    match minutes {
        0 => t("agora").into(),
        1..=59 => format!("{minutes} min"),
        60..=1439 => format!("{} h", minutes / 60),
        _ => format!("{} d", minutes / 1440),
    }
}

/// Height of one Histórico row; the graph's dot sits 14.5 px down, on the first line's middle.
const COMMIT_ROW: f32 = 44.;
/// The lanes drawn side by side; further ones share the last, so the subject keeps its room.
const MAX_LANES: usize = 6;
/// The distance between two lanes.
const LANE: f32 = 16.;
const DOT_CENTER: f32 = 14.5;
/// Room left of the first lane: the head's halo is the widest thing drawn.
const LANE_PAD: f32 = 9.;
/// How far a side branch takes to leave or rejoin a lane: a short, tight bend.
const BEND: f32 = 14.;
const SIDE_BLUE: u32 = 0x7aa2ff;

/// A lane's colour: the first, the main line, wears the accent; every side lane is one blue.
fn lane_color(lane: usize) -> gpui::Hsla {
    match lane {
        0 => accent(),
        _ => rgb(SIDE_BLUE).into(),
    }
}

/// How one line of a row's half is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stroke {
    /// Straight down one lane.
    Straight,
    /// Leaves the commit's lane with a tight bend right below its dot, then runs straight.
    Leave,
    /// Runs straight, then bends tight into another lane at the end of the half: a side branch
    /// arriving at its parent's dot (above) or folding into the main line (below).
    Join,
}

/// The way the line (`from`, `to`) of a row's upper (`above`) or lower half is drawn.
fn stroke_of(from: usize, to: usize, commit_lane: usize, above: bool) -> Stroke {
    match (from == to, above, from == commit_lane) {
        (true, _, _) => Stroke::Straight,
        (false, true, _) => Stroke::Join,
        (false, false, true) => Stroke::Leave,
        (false, false, false) => Stroke::Join,
    }
}

/// The graph beside a commit: its lanes' lines through the row, and its node. A commit is a ring
/// in its lane's colour, a merge a small filled dot, and the head a filled accent dot in a halo.
fn graph_cell(
    row: crate::git_graph::Row,
    head: bool,
    merge: bool,
    lanes: usize,
) -> impl IntoElement {
    let x = |lane: usize| LANE_PAD + lane.min(MAX_LANES - 1) as f32 * LANE;
    let width = x(lanes - 1) + LANE_PAD;
    let (node_x, color) = (x(row.lane), lane_color(row.lane));
    let node = |size: f32| {
        div()
            .absolute()
            .left(px(node_x - size / 2.))
            .top(px(DOT_CENTER - size / 2.))
            .size(px(size))
            .rounded_full()
    };
    let commit_lane = row.lane;
    let marker = if head {
        div()
            .child(node(18.).bg(Hsla { a: 0.2, ..accent() }))
            .child(node(10.).bg(accent()))
            .into_any_element()
    } else if merge {
        node(6.).bg(color).into_any_element()
    } else {
        node(11.)
            .border_2()
            .border_color(color)
            .bg(crate::theme::pane_opaque())
            .into_any_element()
    };
    div()
        .relative()
        .w(px(width))
        .h_full()
        .flex_shrink_0()
        .child(
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let at = |lane_x: f32, y: f32| bounds.origin + gpui::point(px(lane_x), px(y));
                    let mut draw = |from: usize, to: usize, top: f32, bottom: f32, above: bool| {
                        let color = lane_color(from.max(to));
                        let (xa, xb) = (x(from), x(to));
                        let mut path = gpui::PathBuilder::stroke(px(2.));
                        // A bend between y0 and y1: out of the first lane and into the second
                        // along two quadratic arcs, vertical at both ends.
                        let bend = |path: &mut gpui::PathBuilder, y0: f32, y1: f32| {
                            let middle = (y0 + y1) / 2.;
                            path.curve_to(at((xa + xb) / 2., middle), at(xa, (y0 + middle) / 2.));
                            path.curve_to(at(xb, y1), at(xb, (middle + y1) / 2.));
                        };
                        match stroke_of(from, to, commit_lane, above) {
                            Stroke::Straight => {
                                path.move_to(at(xa, top));
                                path.line_to(at(xb, bottom));
                            }
                            Stroke::Leave => {
                                path.move_to(at(xa, top));
                                bend(&mut path, top, top + BEND);
                                path.line_to(at(xb, bottom));
                            }
                            Stroke::Join => {
                                path.move_to(at(xa, top));
                                path.line_to(at(xa, bottom - BEND));
                                bend(&mut path, bottom - BEND, bottom);
                            }
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                    };
                    for (from, to) in &row.above {
                        draw(*from, *to, 0., DOT_CENTER, true);
                    }
                    for (from, to) in &row.below {
                        draw(*from, *to, DOT_CENTER, COMMIT_ROW, false);
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(marker)
}

fn commit_row(
    commit: &Commit,
    head: bool,
    row: &crate::git_graph::Row,
    lanes: usize,
) -> AnyElement {
    let graph = graph_cell(row.clone(), head, commit.parents.len() > 1, lanes);
    let short: String = commit.hash.chars().take(7).collect();
    let by = match (commit.author.is_empty(), age(&commit.when)) {
        (true, age) => age,
        (false, age) if age.is_empty() => commit.author.clone(),
        (false, age) => format!("{} · {age}", commit.author),
    };
    div()
        .h(px(COMMIT_ROW))
        .w_full()
        .flex()
        .gap(px(10.))
        .px(px(12.))
        .child(graph)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.5))
                                .text_color(content(0.9))
                                .child(commit.subject.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .font_family("Menlo")
                                .text_size(px(11.))
                                .text_color(content(0.4))
                                .child(short),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_size(px(11.))
                        .children(commit.refs.iter().take(2).map(|name| {
                            div()
                                .flex_shrink_0()
                                .max_w(px(110.))
                                .truncate()
                                .h(px(14.))
                                .px(px(4.))
                                .rounded(px(crate::theme::RADIUS_SM))
                                .bg(content(0.08))
                                .text_color(content(0.7))
                                .child(name.clone())
                        }))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(content(0.4))
                                .child(by),
                        ),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_branches_bend_tight_and_straight_lines_stay_straight() {
        assert_eq!(stroke_of(0, 0, 0, false), Stroke::Straight);
        assert_eq!(stroke_of(0, 1, 0, false), Stroke::Leave);
        assert_eq!(stroke_of(1, 0, 0, false), Stroke::Join);
        assert_eq!(stroke_of(1, 0, 0, true), Stroke::Join);
    }

    #[test]
    fn discard_names_the_paths_and_confirms_deletes() {
        let action = discard_action("s1", &["a.rs".to_owned()], "tok");
        assert_eq!(action["type"], "discard");
        assert_eq!(action["paths"][0], "a.rs");
        assert_eq!(action["expectedIndex"], "tok");
        assert_eq!(action["confirmDelete"], true);
    }

    #[test]
    fn create_action_trims_and_confirms() {
        let action = create_action("s1", "  Fix login ", "\nbody\n", true);
        assert_eq!(action["action"]["type"], "create");
        assert_eq!(action["action"]["sessionId"], "s1");
        assert_eq!(action["action"]["title"], "Fix login");
        assert_eq!(action["action"]["body"], "body");
        assert_eq!(action["action"]["draft"], true);
        assert_eq!(action["action"]["confirm"], true);
    }

    #[test]
    fn title_actions_carry_the_request_and_index() {
        let generate = title_action("s1", "tok", "r1");
        assert_eq!(generate["action"]["type"], "generate");
        assert_eq!(generate["action"]["expectedIndex"], "tok");
        assert_eq!(generate["action"]["requestId"], "r1");
        let cancel = cancel_title_action("s1", "r1");
        assert_eq!(cancel["action"]["type"], "cancel");
        assert!(cancel["action"].get("expectedIndex").is_none());
    }

    #[test]
    fn title_reply_follows_the_react_checks() {
        let ok = json!({ "type": "title", "title": "Fix login", "provider": "claude", "partial": false });
        assert_eq!(
            title_reply(&ok),
            TitleReply::Title {
                title: "Fix login".into(),
                provider: "claude".into(),
                partial: false
            }
        );
        assert_eq!(
            title_reply(&json!({ "type": "cancelled" })),
            TitleReply::Cancelled
        );
        for bad in [
            json!({ "type": "title", "title": " padded", "provider": "claude", "partial": false }),
            json!({ "type": "title", "title": "two\nlines", "provider": "claude", "partial": false }),
            json!({ "type": "title", "title": "", "provider": "claude", "partial": false }),
            json!({ "type": "title", "title": "x", "provider": "other", "partial": false }),
            json!({ "type": "title", "title": "x", "provider": "codex" }),
            json!({ "type": "title", "title": "x".repeat(73), "provider": "codex", "partial": true }),
            json!({ "type": "unknown" }),
        ] {
            assert_eq!(title_reply(&bad), TitleReply::Invalid, "{bad}");
        }
    }

    #[test]
    fn a_generated_title_yields_to_typing_or_a_moved_index() {
        assert!(!keeps_typed("", "", "a", "a"));
        assert!(keeps_typed("", "my own", "a", "a"));
        assert!(keeps_typed("", "", "a", "b"));
    }

    #[test]
    fn pushing_needs_a_commit_on_a_branch() {
        let head = Some("abc".to_owned());
        assert!(can_push(true, false, &head));
        assert!(!can_push(true, true, &head));
        assert!(!can_push(true, false, &None));
        assert!(!can_push(false, false, &head));
        assert_eq!(pushed_text(&json!({ "branch": "main" })), "Enviado main");
    }

    #[test]
    fn created_url_needs_the_created_reply() {
        let created = json!({ "type": "created", "url": "https://github.com/o/r/pull/3" });
        assert_eq!(
            created_url(&created).as_deref(),
            Some("https://github.com/o/r/pull/3")
        );
        assert_eq!(created_url(&json!({ "type": "done" })), None);
        assert_eq!(created_url(&json!({ "type": "created" })), None);
    }

    #[test]
    fn status_letters_and_colors_follow_the_kind() {
        let (green, red) = diff_colors(0);
        assert_eq!(kind_letter("modified"), "M");
        assert_eq!(kind_letter("added"), "A");
        assert_eq!(kind_letter("untracked"), "U");
        assert_eq!(kind_letter("deleted"), "D");
        assert_eq!(kind_letter("renamed"), "R");
        assert_eq!(kind_mark("added", green, red).1, green);
        assert_eq!(kind_mark("deleted", green, red).1, red);
        assert_eq!(
            kind_mark("renamed", green, red).1,
            Hsla::from(rgb(0x58a6ff))
        );
        assert_eq!(
            kind_mark("modified", green, red).1,
            Hsla::from(rgb(0xe9c46a))
        );
    }

    #[test]
    fn a_row_shows_folder_name_and_only_the_counts_that_exist() {
        assert_eq!(
            split_path("native-spike/src/shell.rs"),
            ("native-spike/src".to_owned(), "shell.rs".to_owned())
        );
        assert_eq!(
            split_path("README.md"),
            (String::new(), "README.md".to_owned())
        );
        assert_eq!(counts_text(92, 11), "+92 \u{2212}11");
        assert_eq!(counts_text(120, 0), "+120");
        assert_eq!(counts_text(0, 3), "\u{2212}3");
        assert_eq!(counts_text(0, 0), "");
    }

    #[test]
    fn stage_all_takes_only_what_the_host_lets_move() {
        let entry = |path: &str, actionable| Entry {
            path: path.into(),
            kind: "modified".into(),
            actionable,
        };
        let list = [entry("a", true), entry("b", false), entry("c", true)];
        assert_eq!(actionable_paths(&list), ["a", "c"]);
        assert!(actionable_paths(&[]).is_empty());
    }

    #[test]
    fn commit_needs_a_message_and_staged_files_unless_amending() {
        assert!(commit_ready(true, false, false, false, false));
        assert!(!commit_ready(false, false, false, false, false));
        assert!(!commit_ready(true, false, false, true, false));
        assert!(!commit_ready(true, false, true, false, false));
        assert!(!commit_ready(true, false, false, false, true));
        assert!(commit_ready(false, true, false, true, false));
        assert!(!commit_ready(false, true, true, true, false));
    }
}
