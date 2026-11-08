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
    AnyElement, AnyView, App, Context, Div, Entity, Hsla, Render, Subscription, Window, div,
    prelude::*, px, rgb, svg, uniform_list,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::rc::Rc;

pub fn new(context: PanelContext, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let commit = cx.new(|cx| TextField::new(t("Título do commit"), cx));
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

struct ChangesPanel {
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
    _events: Vec<Subscription>,
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
                        this.pr.result = Some((t("Pull request criado").into(), false, Some(url)));
                        this.pr.open = false;
                        this.pr.draft = false;
                        this.pr.title.update(cx, |field, cx| field.set_text("", cx));
                        this.pr.body.update(cx, |field, cx| field.set_text("", cx));
                    }
                    Ok(None) => {
                        this.pr.result =
                            Some((t("Resposta inesperada do GitHub.").into(), true, None))
                    }
                    Err(error) => this.pr.result = Some((error, true, None)),
                }
                this.refresh(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// The Criar PR row under the commit field: a button, then its form.
    fn pull_request_row(&self, cx: &mut Context<Self>) -> AnyElement {
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
                        .child(t("Abrir PR")),
                );
            }
            row = row.child(line);
        }
        if !self.pr.open {
            return row
                .child(
                    div()
                        .flex()
                        .child(small("pr-create", t("Criar PR"), false, true).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.pr.open = true;
                                this.pr.result = None;
                                this.pr.title.update(cx, |field, cx| field.focus(cx));
                                cx.notify();
                            }),
                        )),
                )
                .into_any_element();
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
            .child(div().text_size(px(11.)).text_color(content(0.4)).child(t(
                "Envia este branch e abre o pull request para o branch padrão.",
            )))
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
                                t("Criar PR")
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

    fn change_row(&self, entry: &Entry, staged: bool, cx: &mut Context<Self>) -> AnyElement {
        let (added, removed) = diff_colors(0);
        let (add, remove) = self.counts.get(&entry.path).copied().unwrap_or((0, 0));
        let (dir, name) = match entry.path.rsplit_once('/') {
            Some((dir, name)) => (dir.to_owned(), name.to_owned()),
            None => (String::new(), entry.path.clone()),
        };
        let (letter, letter_color) = kind_mark(&entry.kind, added, removed);
        let (open_path, stage_path) = (entry.path.clone(), entry.path.clone());
        let info = div()
            .id(format!("open:{staged}:{}", entry.path))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(4.))
            .cursor_pointer()
            .hover(|style| style.bg(content(0.05)))
            .on_click(
                cx.listener(move |this, _, _, cx| this.open_diff(open_path.clone(), staged, cx)),
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
            .child(
                div()
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
                    .ml_auto()
                    .flex_shrink_0()
                    .flex()
                    .gap(px(5.))
                    .font_family("Menlo")
                    .text_size(px(11.))
                    .child(div().text_color(added).child(format!("+{add}")))
                    .child(div().text_color(removed).child(format!("−{remove}"))),
            );
        let enabled = entry.actionable && !self.busy;
        let mut button = div()
            .id(format!("move:{staged}:{}", entry.path))
            .h(px(24.))
            .px(px(8.))
            .ml(px(6.))
            .rounded(px(crate::theme::RADIUS_SM))
            .border_1()
            .border_color(content(0.12))
            .flex()
            .items_center()
            .flex_shrink_0()
            .text_size(px(12.))
            .text_color(content(0.85))
            .child(if staged {
                t("Despreparar")
            } else {
                t("Preparar")
            });
        button =
            if enabled {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(content(0.08)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.move_path(stage_path.clone(), staged, cx)
                    }))
            } else {
                button.opacity(0.45)
            };
        div()
            .flex()
            .items_center()
            .pr(px(8.))
            .child(info)
            .child(button)
            .into_any_element()
    }

    fn group(
        &self,
        title: &str,
        entries: &[Entry],
        staged: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .pb(px(6.))
            .child(
                div()
                    .px(px(12.))
                    .pt(px(8.))
                    .pb(px(4.))
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child(format!("{title} · {}", entries.len())),
            )
            .children(
                entries
                    .iter()
                    .map(|entry| self.change_row(entry, staged, cx)),
            )
            .into_any_element()
    }

    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut blocks = Vec::new();
        match &self.snapshot {
            None => blocks.push(note(t("Carregando…"), false)),
            Some(snapshot) if !snapshot.repo => {
                blocks.push(note(t("Esta pasta não é um repositório Git."), false))
            }
            Some(snapshot) => {
                if snapshot.staged.is_empty() && snapshot.unstaged.is_empty() {
                    blocks.push(note(t("Nenhuma alteração."), false));
                }
                if !snapshot.staged.is_empty() {
                    blocks.push(self.group(t("Preparados"), &snapshot.staged, true, cx));
                }
                if !snapshot.unstaged.is_empty() {
                    blocks.push(self.group(t("Alterações"), &snapshot.unstaged, false, cx));
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
                blocks.push(self.history(snapshot, cx));
            }
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
                .px(px(12.))
                .pb(px(4.))
                .text_size(px(12.))
                .text_color(content(0.5))
                .child(tf("Histórico · {}", &[&count])),
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

    /// The commit title and its button, under the lists; shown when there are staged files.
    fn footer(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> AnyElement {
        let typed = !self.commit.read(cx).text().trim().is_empty();
        let can_amend =
            snapshot.ahead.is_none_or(|ahead| ahead > 0) && !snapshot.history.is_empty();
        let amending = self.amending && can_amend;
        let enabled = (amending || typed)
            && !self.committing
            && (amending || !snapshot.staged.is_empty())
            && !snapshot.token.is_empty();
        let button = div()
            .id("commit-button")
            .h(px(28.))
            .px(px(12.))
            .rounded(px(crate::theme::RADIUS_MD))
            .flex()
            .items_center()
            .flex_shrink_0()
            .text_size(px(12.))
            .bg(accent())
            .text_color(on_accent())
            .child(if self.committing {
                t("Enviando…")
            } else if amending {
                t("Corrigir")
            } else {
                "Commit"
            });
        let button = if enabled {
            button
                .cursor_pointer()
                .hover(|style| style.opacity(0.9))
                .on_click(cx.listener(|this, _, _, cx| this.commit(cx)))
                .into_any_element()
        } else {
            button.opacity(0.4).into_any_element()
        };
        let mut footer = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(content(0.07));
        if let Some(error) = &self.error {
            footer = footer.child(note(error, true));
        }
        if let Some(text) = &self.title_note {
            footer = footer.child(note(text, false));
        }
        let can_push = can_push(snapshot.repo, snapshot.detached, &snapshot.head);
        let small = |id: &'static str, label: &'static str, on: bool| {
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
            if on {
                base.cursor_pointer().hover(|style| style.opacity(0.9))
            } else {
                base.opacity(0.4)
            }
        };
        let busy_title = self.title_job.is_some();
        let sparkle_on = !busy_title && !self.committing && !snapshot.staged.is_empty();
        let sparkle = div()
            .id("commit-generate")
            .size(px(26.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .child(
                svg()
                    .path(if busy_title { "square" } else { "zap" })
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
        let both_on = enabled && can_push;
        let amend_toggle = can_amend.then(|| {
            small("commit-amend", t("Corrigir o último"), true)
                .when(amending, |b| b.bg(accent_wash(0.18)).text_color(accent()))
                .tooltip(crate::shell::message_actions::name_tip(t(
                    "O próximo commit substitui o último, que ainda não foi enviado. Sem título, mantém a mensagem dele.",
                )))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.amending = !this.amending;
                    cx.notify();
                }))
        });
        let push_row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .pb(px(8.))
            .children(amend_toggle)
            .child(
                small("commit-push", t("Commit e Push"), both_on).when(both_on, |b| {
                    b.on_click(cx.listener(|this, _, _, cx| this.commit_with(true, cx)))
                }),
            )
            .child(
                small("push-only", t("Push"), can_push && !self.push.pushing).when(
                    can_push && !self.push.pushing,
                    |b| {
                        b.on_click(cx.listener(|this, _, _, cx| {
                            this.push.confirm = true;
                            this.push.result = None;
                            cx.notify();
                        }))
                    },
                ),
            );
        footer = footer
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .p(px(8.))
                    .child(div().flex_1().min_w_0().child(self.commit.clone()))
                    .child(sparkle)
                    .child(button),
            )
            .child(push_row);
        if self.push.confirm {
            let branch = snapshot.branch.clone().unwrap_or_default();
            let pushing = self.push.pushing;
            footer = footer.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(8.))
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
                            .child(small("push-cancel", t("Cancelar"), !pushing).when(
                                !pushing,
                                |b| {
                                    b.on_click(cx.listener(|this, _, _, cx| {
                                        this.push.confirm = false;
                                        cx.notify();
                                    }))
                                },
                            ))
                            .child(
                                small(
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
            footer = footer.child(note(text, *failed));
        }
        footer
            .when(snapshot.branch.is_some(), |footer| {
                footer.child(self.pull_request_row(cx))
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
                let footer = match &self.snapshot {
                    Some(snapshot) if snapshot.repo => self.footer(snapshot, cx),
                    _ => div().into_any_element(),
                };
                let body = div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(self.list(cx))
                    .child(footer);
                (header.into_any_element(), body.into_any_element())
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(34.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(content(0.07))
                    .child(header),
            )
            .child(body)
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

/// The one-letter mark of a change kind and its color.
fn kind_mark(kind: &str, added: Hsla, removed: Hsla) -> (&'static str, Hsla) {
    match kind {
        "added" => ("A", added),
        "untracked" => ("U", added),
        "deleted" => ("D", removed),
        "renamed" => ("R", accent()),
        _ => ("M", content(0.6)),
    }
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
const LANE: f32 = 11.;
const DOT_CENTER: f32 = 14.5;

/// One commit: a dot on a line that joins it to the next row, the title, the short hash, and the
/// author with the age. The head's dot is filled in the accent color.
/// A lane's colour. The first keeps the quiet rail; a side branch is told apart by its own.
fn lane_color(lane: usize) -> gpui::Hsla {
    const SIDE: [u32; 5] = [0x5b8def, 0x4ade80, 0xfb923c, 0xc084fc, 0x2dd4bf];
    match lane {
        0 => content(0.3),
        _ => gpui::rgb(SIDE[(lane - 1) % SIDE.len()]).into(),
    }
}

/// The graph beside a commit: its lanes' lines through the row, and its dot.
fn graph_cell(row: crate::git_graph::Row, head: bool, lanes: usize) -> impl IntoElement {
    let x = |lane: usize| 4.5 + lane.min(MAX_LANES - 1) as f32 * LANE;
    let (dot_x, dot_lane) = (x(row.lane), row.lane);
    let width = x(lanes - 1) + 5.;
    let dot = if head { accent() } else { lane_color(dot_lane) };
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
                    let mut draw = |from: usize, to: usize, top: f32, bottom: f32, color| {
                        let mut path = gpui::PathBuilder::stroke(px(1.25));
                        path.move_to(at(x(from), top));
                        if from == to {
                            path.line_to(at(x(to), bottom));
                        } else {
                            // Bends in the middle, leaving and arriving straight.
                            let middle = (top + bottom) / 2.;
                            path.curve_to(at((x(from) + x(to)) / 2., middle), at(x(from), middle));
                            path.curve_to(at(x(to), bottom), at(x(to), middle));
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                    };
                    for (from, to) in &row.above {
                        draw(*from, *to, 0., DOT_CENTER, lane_color(*from));
                    }
                    for (from, to) in &row.below {
                        draw(
                            *from,
                            *to,
                            DOT_CENTER,
                            COMMIT_ROW,
                            lane_color((*from).max(*to)),
                        );
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(
            div()
                .absolute()
                .left(px(dot_x - 4.5))
                .top(px(DOT_CENTER - 4.5))
                .size(px(9.))
                .rounded_full()
                .bg(dot),
        )
}

fn commit_row(
    commit: &Commit,
    head: bool,
    row: &crate::git_graph::Row,
    lanes: usize,
) -> AnyElement {
    let graph = graph_cell(row.clone(), head, lanes);
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
}
