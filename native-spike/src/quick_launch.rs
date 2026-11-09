//! The window's side of the quick composer (quick_composer.rs): what it offers (projects, models,
//! effort, the defaults for a project, branches) and its launch. A launch moves the checkout to
//! the chosen branch when asked, creates the conversation where it was asked to work, attaches
//! the files, sends the text, and puts the window back on the tab it showed unless ⌘↵ asked to
//! open the new conversation.

use super::*;
use crate::quick_composer::{
    LastUsed, QuickDefaults, QuickGroup, QuickLaunch, QuickModel, QuickProject,
};

impl Shell {
    /// The projects the quick composer offers, the open one first.
    pub(crate) fn quick_projects(&self) -> Vec<QuickProject> {
        let mut order: Vec<usize> = self.current_project.into_iter().collect();
        order.extend(
            self.project_order
                .iter()
                .copied()
                .filter(|p| Some(*p) != self.current_project),
        );
        order
            .into_iter()
            .filter_map(|p| self.projects.get(p))
            .map(|p| QuickProject {
                id: p.id.clone(),
                name: p.name.clone(),
                path: p.path.clone(),
                appearance: p.appearance.clone(),
                git: std::path::Path::new(&p.path).join(".git").exists(),
            })
            .collect()
    }

    /// Where a new conversation in this project starts: what its latest conversation used, else
    /// what the main composer gives a new conversation.
    pub(crate) fn quick_defaults(&self, project_id: &str) -> QuickDefaults {
        let project = self.projects.iter().find(|p| p.id == project_id);
        let last = project.and_then(|p| {
            p.conversations
                .iter()
                .filter_map(|ix| self.conversations.get(*ix))
                .filter(|c| !c.session_id.is_empty() && !c.side_chat)
                .max_by(|a, b| a.last_activity.cmp(&b.last_activity))
                .map(|c| LastUsed {
                    provider: c.provider,
                    model: c.model.clone(),
                    approval: c.approval.clone(),
                })
        });
        let active = self.active_conversation();
        let provider = self.new_session_agent(active);
        let model = (provider == self.conversations[active].provider)
            .then(|| self.conversations[active].model.clone())
            .flatten();
        let git = project.is_some_and(|p| std::path::Path::new(&p.path).join(".git").exists());
        crate::quick_composer::resolve_defaults(
            last,
            &self.enabled_providers(),
            (provider, model),
            self.settings["defaultApproval"].as_str(),
            git && self.settings["defaultSessionWorkspace"].as_str() == Some("worktree"),
        )
    }

    /// The model list: every provider that is installed and on, with the rows the main
    /// composer's catalog shows for it.
    pub(crate) fn quick_models(&self, current: (&str, Option<&str>)) -> Vec<QuickGroup> {
        let favorites = self.favorite_models();
        self.enabled_providers()
            .into_iter()
            .map(|provider| QuickGroup {
                provider,
                title: provider_title(provider),
                models: self
                    .provider_model_rows(provider, current, "", &favorites)
                    .into_iter()
                    .map(|row| QuickModel {
                        id: row.id,
                        name: row.name,
                    })
                    .collect(),
            })
            .filter(|group| !group.models.is_empty())
            .collect()
    }

    /// The name the pill shows for a model: "Opus 5.5", not the alias.
    pub(crate) fn quick_model_name(&self, provider: &str, model: Option<&str>) -> String {
        self.model_display(provider, model)
    }

    /// The effort levels a model offers and the one chosen, as the main composer's bar.
    pub(crate) fn quick_effort(
        &self,
        provider: &str,
        model: Option<&str>,
    ) -> (Vec<String>, Option<String>) {
        let execution = self.execution(provider, model);
        (execution.levels, execution.effort)
    }

    /// Picks an effort for a model, shared with every conversation using it. For Cursor's
    /// plain models the level is part of the id: the model to use instead comes back.
    pub(crate) fn quick_set_effort(
        &mut self,
        provider: &str,
        model: &str,
        level: &str,
    ) -> Option<String> {
        self.save_execution(provider, model, "effort", json!(level))
    }

    /// Asks the host for the project's branches; they reach the quick composer by
    /// `quick_composer::branches_arrived`.
    pub(crate) fn quick_branches(&mut self, project_id: &str) {
        let call = self
            .backend
            .invoke("list_branches", json!({ "projectId": project_id }));
        self.waiting
            .insert(call, Waiting::QuickBranches(project_id.to_owned()));
    }

    /// The quick composer's Enter (or ⌘↵): the checkout moves to the chosen branch first when
    /// asked, then the conversation is created and the text goes out once it exists.
    pub(crate) fn quick_launch(&mut self, launch: QuickLaunch, cx: &mut Context<Self>) {
        if let Some(branch) = launch.checkout.clone() {
            let call = self.backend.invoke(
                "checkout_branch",
                json!({ "projectId": launch.project_id, "branch": branch, "confirm": true }),
            );
            self.waiting
                .insert(call, Waiting::QuickCheckout(Box::new(launch)));
        } else {
            self.quick_create(launch);
        }
        cx.notify();
    }

    fn quick_create(&mut self, launch: QuickLaunch) {
        let call = self.backend.invoke(
            "create_session",
            json!({ "request": crate::quick_composer::create_request(&launch) }),
        );
        self.waiting
            .insert(call, Waiting::QuickCreated(Box::new(launch)));
    }

    pub(super) fn quick_checked_out(
        &mut self,
        launch: QuickLaunch,
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        if ok {
            self.quick_create(launch);
        } else {
            self.problem = Some(error_text(value));
        }
        cx.notify();
    }

    pub(super) fn quick_created(
        &mut self,
        launch: QuickLaunch,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        let before = (self.current_project, self.active);
        let ix = self.add_session(value);
        self.conversations[ix].loaded = true;
        self.conversations[ix].approval = launch.approval.clone();
        self.open(ix, cx);
        self.drafts
            .insert(ix, Draft::with_text(launch.text.clone()));
        self.touch_drafts();
        if launch.attachments.is_empty() {
            self.quick_send(ix, launch.foreground, before, cx);
            return;
        }
        // The files go to the host first; the message follows once it has them. A file that
        // can't be read leaves the text in the conversation's composer, unsent.
        let backend = self.backend.clone();
        let owner = format!("session:{}", self.conversations[ix].session_id);
        let paths = launch.attachments;
        let foreground = launch.foreground;
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(attachment_view::prepare_files(&paths, 0));
        });
        cx.spawn(async move |this, cx| {
            let Ok(prepared) = rx.recv().await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                match prepared {
                    Ok(files) if !files.is_empty() => {
                        let call = backend.invoke(
                            "paste_prompt_attachments",
                            json!({ "owner": owner, "files": files }),
                        );
                        this.waiting
                            .insert(call, Waiting::QuickAttached(ix, foreground, before));
                    }
                    Ok(_) => this.quick_send(ix, foreground, before, cx),
                    Err(message) => this.problem = Some(message),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn quick_attached(
        &mut self,
        ix: usize,
        foreground: bool,
        before: (Option<usize>, u64),
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        if !ok {
            self.problem = Some(error_text(value));
            cx.notify();
            return;
        }
        self.viewer.note(value);
        let added = value.as_array().into_iter().flatten().map(|a| {
            (
                a["id"].as_str().unwrap_or("").to_owned(),
                a["name"].as_str().unwrap_or("").to_owned(),
            )
        });
        self.attachments.entry(ix).or_default().extend(added);
        self.quick_send(ix, foreground, before, cx);
    }

    /// Sends the new conversation's draft; without ⌘↵ the window goes back to the project and
    /// tab it showed, and the conversation carries on in its own tab.
    fn quick_send(
        &mut self,
        ix: usize,
        foreground: bool,
        before: (Option<usize>, u64),
        cx: &mut Context<Self>,
    ) {
        if self.active_conversation() != ix {
            self.open(ix, cx);
        }
        self.send(cx);
        if !foreground {
            let (project, tab) = before;
            if project != self.current_project {
                self.enter_project(project, cx);
            }
            if self.tabs.iter().any(|t| t.id == tab && !t.closing) {
                self.select(tab, cx);
            }
        }
        cx.notify();
    }
}
