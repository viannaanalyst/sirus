//! The top bar's Git split button (ADR-198), after Synara: a ghost button before the +N -N
//! numbers whose left half does the one thing the branch needs (pull, push, commit and push,
//! sync) and whose chevron opens "Ações do Git". The choices are in git_split.rs and the state in
//! git_sync.rs; this draws the button and its menu and runs the work through the host:
//! `git_incoming` (a real, throttled `git fetch` of the upstream), `git_sync_pull` (pull with the
//! local changes kept, a conflict undone), `git_push`, and for a commit the review view's own
//! path: stage, `commit_title_action`, `git_commit`.
use super::*;
use crate::git_split::{self, GitFacts, Item, Quick, Running};
use crate::git_sync::{self, Synced};
use message_actions::{settle, text_tip};

const MENU_WIDTH: f32 = 224.;
const BUTTON_HEIGHT: f32 = 30.;
const AMBER: u32 = 0xf0b26a;
const GROUP: &str = "git-split";

/// What the success toast says once a push lands.
#[derive(Clone)]
enum PushToast {
    Sent,
    /// A sync: the branch and upstream it was brought up to.
    Synced(Option<(String, String)>),
}

/// How a commit (and its push) ended.
enum Committed {
    Commit(String),
    Pushed { branch: String },
    PushFailed(String),
}

impl Shell {
    /// The branch's state, or None when the button has nothing to say: no upstream, a clean tree
    /// and nothing running.
    fn git_facts(&self) -> Option<GitFacts> {
        let (added, removed) = self.diff_stat();
        let dirty = added + removed > 0;
        let value = self.incoming.value.as_ref();
        if value.is_none() && !dirty && self.incoming.running.is_none() {
            return None;
        }
        Some(GitFacts {
            ahead: value.map_or(0, |v| v.ahead),
            behind: value.map_or(0, |v| v.behind),
            dirty,
        })
    }

    /// The room the button takes in the title bar, for the tab strip to leave.
    pub(super) fn git_button_width(&self, shown: bool) -> f32 {
        let Some(facts) = self.git_facts().filter(|_| shown) else {
            return 0.;
        };
        let label = self
            .incoming
            .running
            .map(Running::label)
            .or_else(|| git_split::quick_action(facts).label());
        git_split::width_hint(label) + 4.
    }

    pub(super) fn git_button(
        &mut self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let facts = self.git_facts()?;
        let quick = git_split::quick_action(facts);
        let running = self.incoming.running;
        let busy = running.is_some();
        let open = self.incoming.open;
        let label = running.map(Running::label).or_else(|| quick.label());
        let grey = content(if busy { 0.4 } else { 0.6 });
        let glyph: AnyElement = if busy {
            *moving = true;
            self.spin(now).into_any_element()
        } else {
            crate::theme::icon(quick.icon(), 16.)
                .text_color(grey)
                .into_any_element()
        };
        let bounds = self.git_bounds.clone();
        let quick_half = div()
            .id("git-quick")
            .h(px(BUTTON_HEIGHT))
            .pl(px(7.))
            .pr(px(if label.is_some() { 6. } else { 7. }))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded_l(px(crate::theme::RADIUS_SM))
            .text_size(px(12.5))
            .text_color(grey)
            .whitespace_nowrap()
            .group_hover(GROUP, |s| s.bg(content(0.04)))
            .child(glyph)
            .children(label.map(|label| div().child(label)))
            .when(!busy, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(content(0.08)))
                    .on_click(cx.listener(|this, _, _, cx| this.git_quick(cx)))
            });
        let chevron = div()
            .id("git-chevron")
            .h(px(BUTTON_HEIGHT))
            .w(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_r(px(crate::theme::RADIUS_SM))
            .cursor_pointer()
            .group_hover(GROUP, |s| s.bg(content(0.04)))
            .when(open, |d| d.bg(content(0.08)))
            .hover(|s| s.bg(content(0.08)))
            .on_click(cx.listener(|this, _, _, cx| this.git_menu_toggle(cx)))
            .child(crate::theme::icon("chevron-down", 12.).text_color(content(0.5)));
        Some(
            div()
                .id("git-split")
                .group(GROUP)
                .relative()
                .h(px(BUTTON_HEIGHT))
                .flex()
                .items_center()
                .flex_shrink_0()
                .rounded(px(crate::theme::RADIUS_SM))
                .child(
                    gpui::canvas(|_, _, _| {}, move |frame, _, _, _| bounds.set(Some(frame)))
                        .absolute()
                        .size_full(),
                )
                .child(quick_half)
                .child(chevron)
                .into_any_element(),
        )
    }

    /// The menu "Ações do Git", under the button's right edge. Drawn while it fades out too.
    pub(super) fn git_menu(
        &mut self,
        now: Instant,
        moving: &mut bool,
        size: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.incoming.visible(now) {
            return None;
        }
        let frame = self.git_bounds.get()?;
        let facts = self.git_facts().unwrap_or_default();
        let p = match &self.incoming.tween {
            Some(tween) => {
                *moving |= !tween.done(now);
                tween.at(now).clamp(0., 1.)
            }
            None => 1.,
        };
        let right = f32::from(size.width) - f32::from(frame.right());
        let top = f32::from(frame.bottom()) + 4. + (1. - p) * -6.;
        let mut rows: Vec<AnyElement> = vec![
            div()
                .px(px(8.))
                .pt(px(5.))
                .pb(px(4.))
                .text_size(px(11.5))
                .text_color(content(0.45))
                .child(t("Ações do Git"))
                .into_any_element(),
        ];
        for (ix, item) in git_split::ITEMS.into_iter().enumerate() {
            let reason = git_split::disabled_reason(item, facts);
            let row =
                div()
                    .id(("git-item", ix))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .child(
                        crate::theme::icon(item.icon(), 14.)
                            .text_color(content(if reason.is_some() { 0.25 } else { 0.6 })),
                    )
                    .child(item.label());
            rows.push(match reason {
                Some(reason) => row
                    .text_color(content(0.3))
                    .tooltip(text_tip(reason.to_owned()))
                    .into_any_element(),
                None => row
                    .text_color(content(0.9))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let fade = if cx.reduce_motion() { 0. } else { MENU };
                        this.incoming.close_at(Instant::now(), fade);
                        this.git_do(item, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
            });
        }
        let checked_at = self.incoming.value.as_ref().and_then(|v| v.checked_at);
        let footer = div()
            .mt(px(4.))
            .pt(px(6.))
            .pb(px(3.))
            .px(px(8.))
            .border_t_1()
            .border_color(content(0.07))
            .text_size(px(11.5))
            .line_height(px(16.));
        rows.push(match git_split::footer(facts, checked_at, now_millis()) {
            git_split::Footer::Behind => footer
                .text_color(rgb(AMBER))
                .child(t("Sua branch está atrás do GitHub. Puxe antes de enviar."))
                .into_any_element(),
            git_split::Footer::Checked(text) => footer
                .text_size(px(11.))
                .text_color(content(0.4))
                .child(text)
                .into_any_element(),
        });
        Some(
            div()
                .id("git-menu")
                .occlude()
                .absolute()
                .right(px(right))
                .top(px(top))
                .w(px(MENU_WIDTH))
                .p(px(5.))
                .rounded(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_color(crate::theme::border())
                .child(crate::theme::floating_surface(crate::theme::RADIUS_MD))
                .shadow_lg()
                .opacity(p)
                .flex()
                .flex_col()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(rows)
                .into_any_element(),
        )
    }

    pub(super) fn git_menu_toggle(&mut self, cx: &mut Context<Self>) {
        self.environment = None;
        let duration = if cx.reduce_motion() { 0. } else { MENU };
        let opening = !self.incoming.open;
        self.incoming.toggle(Instant::now(), duration);
        // Opening it asks GitHub (the host fetches at most once per 30 s per checkout).
        if opening {
            self.incoming_fetch();
        }
        cx.notify();
    }

    /// The left half: what the branch needs; the up-to-date cloud opens the menu instead.
    fn git_quick(&mut self, cx: &mut Context<Self>) {
        if self.incoming.running.is_some() {
            return;
        }
        let Some(facts) = self.git_facts() else {
            return;
        };
        match git_split::quick_action(facts) {
            Quick::Pull => self.git_do(Item::Pull, cx),
            Quick::Push => self.git_do(Item::Push, cx),
            Quick::CommitPush => self.git_do(Item::CommitPush, cx),
            Quick::Sync => self.git_pull(true, true, cx),
            Quick::UpToDate => self.git_menu_toggle(cx),
        }
    }

    fn git_do(&mut self, item: Item, cx: &mut Context<Self>) {
        match item {
            Item::Commit => self.git_commit(false, cx),
            Item::CommitPush => self.git_commit(true, cx),
            Item::Pull => self.git_pull(false, false, cx),
            Item::Push => self.git_push(PushToast::Sent, Some(Running::Pushing), cx),
            Item::PullRequest => self.git_open_pull_request(cx),
        }
    }

    /// Pull (`rebase` false: fast-forward) or sync (rebase, then `then_push`), local changes kept.
    fn git_pull(&mut self, rebase: bool, then_push: bool, cx: &mut Context<Self>) {
        let work = if rebase {
            Running::Syncing
        } else {
            Running::Pulling
        };
        if !self.incoming.begin(work) {
            return;
        }
        let session = self.incoming.session().to_owned();
        let generation = self.incoming.generation();
        let id = self.backend.invoke(
            "git_sync_pull",
            json!({ "sessionId": session, "rebase": rebase }),
        );
        self.waiting
            .insert(id, Waiting::IncomingPull(generation, then_push));
        cx.notify();
    }

    /// `git_sync_pull`'s answer: a toast, the new state, and for a sync the push that follows.
    pub(super) fn git_pulled(
        &mut self,
        generation: u64,
        then_push: bool,
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        let known = self.incoming.value.clone();
        let answer = git_sync::sync_answer(ok, value);
        let names = |next: &Option<git_sync::GitIncoming>| {
            next.as_ref()
                .or(known.as_ref())
                .map(|g| (g.branch.clone(), g.upstream.clone()))
        };
        match answer {
            Synced::Pulled(next) | Synced::UpToDate(next) if then_push => {
                let names = names(&next);
                if generation == self.incoming.generation() {
                    self.incoming.value = next;
                    // Still the same job: the push is its second half.
                    self.git_push(PushToast::Synced(names), None, cx);
                }
                return;
            }
            Synced::Pulled(next) => {
                let toast = names(&next).map_or(String::new(), |(branch, upstream)| {
                    tf("{} atualizada a partir de {}", &[&branch, &upstream])
                });
                self.incoming.finish(generation, Some(next));
                self.git_toast(t("Sincronizado"), toast, true);
            }
            Synced::UpToDate(next) => {
                let toast = names(&next).map_or(String::new(), |(branch, upstream)| {
                    tf("{} já está igual a {}", &[&branch, &upstream])
                });
                self.incoming.finish(generation, Some(next));
                self.git_toast(t("Já está em dia"), toast, true);
            }
            Synced::Diverged => {
                self.incoming.finish(generation, None);
                self.git_toast(
                    t("Não deu para puxar"),
                    t("Seu branch divergiu do GitHub. Use Sincronizar.").to_owned(),
                    false,
                );
            }
            Synced::Conflict(files) => {
                self.incoming.finish(generation, None);
                self.git_conflict_toast(
                    t("Não deu para puxar"),
                    git_split::conflict_line(&files),
                    files,
                );
            }
            Synced::Failed(message) => {
                self.incoming.finish(generation, None);
                self.git_toast(t("Não deu para puxar"), message, false);
            }
        }
        self.git_after(cx);
        cx.notify();
    }

    /// Pushes the branch. `begin` is the job to start; None continues one already running.
    fn git_push(&mut self, toast: PushToast, begin: Option<Running>, cx: &mut Context<Self>) {
        if let Some(work) = begin
            && !self.incoming.begin(work)
        {
            return;
        }
        let session = self.incoming.session().to_owned();
        let generation = self.incoming.generation();
        let rx = self
            .backend
            .request("git_push", json!({ "sessionId": session }));
        cx.spawn(async move |this, cx| {
            let reply = settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.incoming.finish(generation, None);
                match reply {
                    Ok(value) => {
                        let branch = value["branch"].as_str().unwrap_or("").to_owned();
                        match toast {
                            PushToast::Synced(Some((branch, upstream))) => this.git_toast(
                                t("Sincronizado"),
                                tf("{} atualizada a partir de {}", &[&branch, &upstream]),
                                true,
                            ),
                            PushToast::Synced(None) | PushToast::Sent => this.git_toast(
                                t("Enviado"),
                                tf("{} está no GitHub", &[&branch]),
                                true,
                            ),
                        }
                    }
                    Err(error) => {
                        this.problem = Some(crate::i18n::from_host(&error));
                        this.notice = None;
                    }
                }
                this.git_after(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Commits everything with a generated message, and with `push` sends it. The review view's
    /// own path: stage what the host lets move, ask for the title, commit against that index.
    fn git_commit(&mut self, push: bool, cx: &mut Context<Self>) {
        let work = if push {
            Running::Pushing
        } else {
            Running::Committing
        };
        if !self.incoming.begin(work) {
            return;
        }
        let backend = self.backend.clone();
        let session = self.incoming.session().to_owned();
        let generation = self.incoming.generation();
        cx.spawn(async move |this, cx| {
            let made = match commit_all(&backend, &session).await {
                Ok(title) if push => {
                    let sent =
                        settle(backend.request("git_push", json!({ "sessionId": session }))).await;
                    let done = match sent {
                        Ok(value) => Committed::Pushed {
                            branch: value["branch"].as_str().unwrap_or("").to_owned(),
                        },
                        Err(error) => Committed::PushFailed(error),
                    };
                    Ok((title, done))
                }
                Ok(title) => Ok((title.clone(), Committed::Commit(title))),
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |this, cx| {
                this.incoming.finish(generation, None);
                match made {
                    Ok((_, Committed::Commit(title))) => {
                        this.git_toast(t("Commit feito"), title, true)
                    }
                    Ok((_, Committed::Pushed { branch })) => {
                        this.git_toast(t("Enviado"), tf("{} está no GitHub", &[&branch]), true)
                    }
                    Ok((title, Committed::PushFailed(error))) => {
                        this.git_toast(
                            t("Commit feito"),
                            format!("{title}\n{}", crate::i18n::from_host(&error)),
                            false,
                        );
                    }
                    Err(error) => {
                        this.problem = Some(crate::i18n::from_host(&error));
                        this.notice = None;
                    }
                }
                this.git_after(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// "Criar PR": the review view opens with its form, the same as its branch menu's item.
    fn git_open_pull_request(&mut self, cx: &mut Context<Self>) {
        self.open_pane(CHANGES_PANE, cx);
        let key = (CHANGES_PANE, self.incoming.session().to_owned());
        if let Some(view) = self.pane_views.get(&key).cloned() {
            crate::panel_changes::open_pull_request_view(&view, cx);
        }
    }

    /// After a pull, commit or push: the diff numbers, the history and changes views, and what
    /// the branch now has against its upstream (asked from the cache when fetched lately).
    fn git_after(&mut self, cx: &mut Context<Self>) {
        self.refresh_diff();
        self.git_refresh_views(cx);
        self.incoming_fetch();
    }

    /// Reads the selected conversation's history graph and changes view again.
    pub(super) fn git_refresh_views(&mut self, cx: &mut Context<Self>) {
        let session = self.incoming.session().to_owned();
        let views: Vec<gpui::AnyView> = self
            .pane_views
            .iter()
            .filter(|((_, owner), _)| *owner == session)
            .map(|(_, view)| view.clone())
            .collect();
        for view in views {
            crate::panel_changes::refresh_view(&view, cx);
        }
    }

    /// The window's own toast: a title and a line under it. `ok` reports; otherwise it warns.
    fn git_toast(&mut self, title: &str, detail: String, ok: bool) {
        let text = if detail.is_empty() {
            title.to_owned()
        } else {
            format!("{title}\n{detail}")
        };
        self.notice = ok.then(|| text.clone());
        self.problem = Some(text);
        self.toasts.conflict = None;
    }

    /// A conflict toast: a warning that keeps its card and offers the ways out.
    fn git_conflict_toast(&mut self, title: &str, detail: String, files: Vec<String>) {
        let text = if detail.is_empty() {
            title.to_owned()
        } else {
            format!("{title}\n{detail}")
        };
        self.notice = None;
        self.problem = Some(text.clone());
        self.toasts.conflict = Some(crate::shell::toast::Conflict {
            problem: text,
            files,
        });
    }

    /// "Resolver com o agente": asks the conversation's agent to settle the conflict and finish
    /// the merge or rebase.
    pub(super) fn resolve_conflict_with_agent(&mut self, cx: &mut Context<Self>) {
        let Some(conflict) = self.toasts.conflict.take() else {
            return;
        };
        self.problem = None;
        let list = conflict
            .files
            .iter()
            .map(|file| format!("- {file}"))
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = tf(
            "Resolva o conflito de merge ou rebase nos arquivos abaixo, mantendo a intenção dos dois lados, e conclua o merge ou rebase:\n\n{}",
            &[&list],
        );
        let conversation = &self.conversations[self.active_conversation()];
        let session = conversation.session_id.clone();
        if session.is_empty() {
            cx.notify();
            return;
        }
        let execution = json!({
            "approval": conversation.approval,
            "effort": conversation.effort,
            "fast": conversation.fast,
            "planning": conversation.planning,
        });
        self.backend.invoke(
            "send_prompt",
            json!({ "request": {
                "sessionId": session.clone(),
                "prompt": prompt,
                "clientRequestId": crate::panel_sidechat::uuid(),
                "execution": execution,
                "attachmentIds": [],
                "attachmentOwner": format!("session:{session}"),
            } }),
        );
        cx.notify();
    }

    /// "Abrir arquivo": shows the first conflicted file in the Files pane.
    pub(super) fn open_conflict_file(&mut self, cx: &mut Context<Self>) {
        let Some(conflict) = self.toasts.conflict.take() else {
            return;
        };
        self.problem = None;
        let Some(first) = conflict.files.first().cloned() else {
            cx.notify();
            return;
        };
        let session = self.incoming.session().to_owned();
        let root = self
            .conversations
            .iter()
            .find(|conversation| conversation.session_id == session)
            .or_else(|| self.conversations.get(self.active_conversation()))
            .map(|conversation| conversation.worktree.clone())
            .unwrap_or_default();
        let path = if first.starts_with('/') || root.is_empty() {
            first
        } else {
            format!("{}/{}", root.trim_end_matches('/'), first)
        };
        self.open_chip(
            crate::file_chips::Target {
                path,
                folder: false,
                outside: false,
            },
            cx,
        );
        cx.notify();
    }

    /// "Desfazer": ends the merge or rebase left in progress in the workspace.
    pub(super) fn abort_conflict(&mut self, cx: &mut Context<Self>) {
        if self.toasts.conflict.take().is_none() {
            return;
        }
        self.problem = None;
        let session = self.incoming.session().to_owned();
        let session = if session.is_empty() {
            self.active_session_id()
        } else {
            session
        };
        if !session.is_empty() {
            self.backend
                .invoke("git_abort_conflict", json!({ "sessionId": session }));
        }
        self.git_after(cx);
        cx.notify();
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

/// The unstaged paths the host lets move, from a `git_workspace_action` snapshot.
fn movable_paths(snapshot: &Value) -> Vec<String> {
    snapshot["unstaged"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["actionable"].as_bool().unwrap_or(false))
        .filter_map(|entry| entry["path"].as_str().map(str::to_owned))
        .collect()
}

/// Stages what can be staged, asks for a title and commits against that index. The title.
async fn commit_all(backend: &Backend, session: &str) -> Result<String, String> {
    let read = || {
        settle(backend.request(
            "git_workspace_action",
            json!({ "action": { "type": "snapshot", "sessionId": session } }),
        ))
    };
    let mut snapshot = read().await?["snapshot"].clone();
    let paths = movable_paths(&snapshot);
    if !paths.is_empty() {
        let token = snapshot["indexToken"].as_str().unwrap_or("").to_owned();
        settle(backend.request(
            "git_workspace_action",
            json!({ "action": { "type": "stage", "sessionId": session, "paths": paths, "expectedIndex": token } }),
        ))
        .await?;
        snapshot = read().await?["snapshot"].clone();
    }
    let staged = snapshot["staged"].as_array().map_or(0, Vec::len);
    if staged == 0 {
        return Err(t("Nada para commitar.").to_owned());
    }
    let token = snapshot["indexToken"].as_str().unwrap_or("").to_owned();
    let reply = settle(backend.request(
        "commit_title_action",
        crate::panel_changes::title_action(session, &token, &crate::panel_sidechat::uuid()),
    ))
    .await?;
    let title = match crate::panel_changes::title_reply(&reply) {
        crate::panel_changes::TitleReply::Title { title, .. } => title,
        _ => return Err(t("Não deu para gerar a mensagem do commit.").to_owned()),
    };
    settle(backend.request(
        "git_commit",
        json!({ "sessionId": session, "message": title, "expectedIndex": token, "amend": false }),
    ))
    .await?;
    Ok(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_entries_the_host_lets_move_are_staged() {
        let snapshot = json!({ "unstaged": [
            { "path": "a.rs", "kind": "modified", "actionable": true },
            { "path": "b.rs", "kind": "conflicted", "actionable": false },
            { "path": "c.rs", "kind": "untracked", "actionable": true },
            { "kind": "modified", "actionable": true },
        ] });
        assert_eq!(movable_paths(&snapshot), ["a.rs", "c.rs"]);
        assert!(movable_paths(&json!({})).is_empty());
    }
}
