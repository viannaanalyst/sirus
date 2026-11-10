//! The active computer of the window (ADR-133, option A): the whole window shows one computer at
//! a time, this Mac or a paired Sirus host, and switching reloads it once on arrival. Projects,
//! conversations and transcripts live on each computer's host; what the window keeps per computer
//! is the layout of its tabs, saved when leaving and restored once the arrival has loaded.
//!
//! Commands and events that belong to this Mac stay local through the backend's routing
//! (`computers::runs_here`). What reads this Mac's folders or runs this Mac's `git` (Files and
//! Changes, attachments from disk, adding a folder, opening in an editor) is switched off on a
//! remote computer with a short note, instead of half-working on a path that is not there.
//! Terminals and project actions run their shells on that computer's host (ADR-204). A dropped connection shows its state and retries; it never falls back to local data.
//! A child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;
use crate::computers::{self, Computer, HOME, Status, Tokens};

/// The tabs of one computer as the person left them, by session id.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct View {
    /// Session id, pinned, project id.
    pub tabs: Vec<(String, bool, String)>,
    pub active: Option<String>,
    pub project: Option<String>,
}

pub(crate) struct State {
    pub list: Vec<Computer>,
    /// The computer the window shows: `HOME` or a paired computer's id.
    pub active: String,
    /// Where the active remote connection is; `Open` while the active computer is this Mac.
    pub status: Status,
    views: HashMap<String, View>,
    /// The tabs to restore once the next load of the active computer arrives.
    arriving: Option<View>,
    /// The "add a computer" form of Settings → Conexões, while it is open (computer_ui.rs).
    pub add: super::computer_ui::AddForm,
    /// A conversation is being moved to another computer.
    pub moving: bool,
    /// An outcome the Conexões page shows under the list of computers.
    pub note: Option<String>,
}

impl State {
    pub fn new() -> Self {
        Self {
            list: computers::load(),
            active: HOME.to_owned(),
            status: Status::Open,
            views: HashMap::new(),
            arriving: None,
            add: Default::default(),
            note: None,
            moving: false,
        }
    }

    pub fn remote(&self) -> bool {
        self.active != HOME
    }

    pub fn active_name(&self) -> String {
        self.list
            .iter()
            .find(|c| c.id == self.active)
            .map_or_else(|| t("Este Mac").to_owned(), |c| c.name.clone())
    }

    pub fn name_of(&self, id: &str) -> String {
        if id == HOME {
            return t("Este Mac").to_owned();
        }
        self.list
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| id.to_owned(), |c| c.name.clone())
    }
}

/// The saved tabs that still exist on the computer, as indexes into its session ids, in order.
pub(crate) fn surviving(view: &View, sessions: &[String]) -> Vec<(usize, bool)> {
    let mut seen = Vec::new();
    view.tabs
        .iter()
        .filter_map(|(id, pinned, _)| {
            let ix = sessions.iter().position(|s| !s.is_empty() && s == id)?;
            if seen.contains(&ix) {
                return None;
            }
            seen.push(ix);
            Some((ix, *pinned))
        })
        .collect()
}

impl Shell {
    pub(super) fn on_remote(&self) -> bool {
        self.computers.remote()
    }

    /// For what only works on this Mac's folders: true, with a short note, on a remote computer.
    pub(super) fn this_mac_only(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.on_remote() {
            return false;
        }
        self.problem =
            Some(t("Disponível só neste Mac: esta janela está em outro computador.").to_owned());
        cx.notify();
        true
    }

    fn snapshot_view(&self) -> View {
        let session = |tab: &Tab| {
            let conversation = &self.conversations[tab.conversation];
            (!conversation.session_id.is_empty() && !tab.closing).then(|| {
                let project = self
                    .project_of(tab.conversation)
                    .map(|p| self.projects[p].id.clone())
                    .unwrap_or_default();
                (conversation.session_id.clone(), tab.pinned, project)
            })
        };
        let mut tabs: Vec<_> = self.tabs.iter().filter_map(session).collect();
        let mut parked: Vec<_> = self.parked.iter().collect();
        parked.sort_by_key(|(project, _)| **project);
        for (_, (list, _)) in parked {
            tabs.extend(list.iter().filter_map(session));
        }
        let active = self
            .tabs
            .iter()
            .find(|t| t.id == self.active)
            .and_then(|t| session(t))
            .map(|(id, _, _)| id);
        View {
            tabs,
            active,
            project: self.current_project.map(|p| self.projects[p].id.clone()),
        }
    }

    /// The window as `load_state` leaves it when the computer has nothing, until the real load
    /// arrives: nothing of the previous computer stays on screen.
    fn blank_window(&mut self, title: String) {
        let now = Instant::now();
        self.conversations.clear();
        self.tabs.clear();
        self.parked.clear();
        self.dock_places.clear();
        self.remembered.clear();
        self.rows.clear();
        self.lists.clear();
        self.glides.clear();
        self.history.clear();
        self.history_at = 0;
        self.projects.clear();
        self.project_order.clear();
        self.current_project = None;
        self.drafts.clear();
        self.saved_drafts.clear();
        self.attachments.clear();
        self.pane_views.clear();
        self.dock_panes.clear();
        self.dock_active = None;
        self.files = (String::new(), Vec::new());
        self.diffs.clear();
        self.worktrees.clear();
        self.remote_repos.clear();
        self.secrets.clear();
        self.skills.clear();
        self.conversations.push(Conversation::placeholder());
        self.conversations[0].title = title;
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab {
            id,
            conversation: 0,
            pinned: false,
            closing: false,
            x: Tween::still(-1., now),
            w: Tween::still(-1., now),
        });
        self.active = id;
        self.history.push(id);
    }

    /// Makes `id` the computer of the whole window. The old computer's tabs are remembered, its
    /// screen is cleared, and the new one is loaded once it answers.
    pub(super) fn switch_computer(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.computers.active == id {
            return;
        }
        let target = if id == HOME {
            None
        } else {
            let Some(computer) = self.computers.list.iter().find(|c| c.id == id).cloned() else {
                return;
            };
            let Ok(address) = computer.address() else {
                self.problem = Some(t("O endereço desse computador é inválido.").to_owned());
                cx.notify();
                return;
            };
            let Some(token) = computers::Keychain.get(id) else {
                self.problem =
                    Some(t("Esse computador perdeu o pareamento: pareie de novo.").to_owned());
                cx.notify();
                return;
            };
            Some((address, token))
        };
        // What is typed goes to the computer it was typed for.
        self.save_drafts();
        let view = self.snapshot_view();
        let leaving = std::mem::replace(&mut self.computers.active, id.to_owned());
        self.computers.views.insert(leaving, view);
        self.computers.arriving = self.computers.views.get(id).cloned();
        self.computers.status = if target.is_some() {
            Status::Connecting
        } else {
            Status::Open
        };
        self.computers.note = None;
        self.project_menu = None;
        self.context_menu = None;
        self.confirm = None;
        self.palette_ui = None;
        if self.term.pulls.is_some() {
            self.close_pull_requests(cx);
        }
        self.drop_remote_title_tabs(cx);
        let title = if target.is_some() {
            tf("Conectando a {}…", &[&self.computers.name_of(id)])
        } else {
            t("Carregando…").to_owned()
        };
        self.blank_window(title);
        // What the pages loaded belongs to the computer that was left; this Mac's own stays.
        self.settings_data.retain(|key, _| {
            matches!(
                *key,
                "remote" | "computers" | "github" | "github_device" | "app_update"
            )
        });
        self.publish_computers();
        self.problem = None;
        self.connected = false;
        match target {
            Some((address, token)) => {
                self.backend.activate(Some(id));
                self.backend.connect_remote(id, address, token);
            }
            None => {
                self.backend.disconnect_remote();
                self.backend.activate(None);
                self.backend.reload();
            }
        }
        cx.notify();
    }

    /// The connection to a computer changed. A drop keeps the tabs to restore when it returns.
    pub(super) fn computer_link(&mut self, id: &str, status: Status, cx: &mut Context<Self>) {
        if self.computers.active != id {
            return;
        }
        if status == Status::Lost && self.computers.status == Status::Open {
            self.computers.arriving = Some(self.snapshot_view());
        }
        self.computers.status = status;
        self.publish_computers();
        cx.notify();
    }

    /// A load of the active computer finished: its saved tabs come back.
    pub(super) fn computer_arrived(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.computers.arriving.take() else {
            return;
        };
        let sessions: Vec<String> = self
            .conversations
            .iter()
            .map(|c| c.session_id.clone())
            .collect();
        let tabs = surviving(&view, &sessions);
        if tabs.is_empty() {
            return;
        }
        let now = Instant::now();
        let project = view
            .project
            .as_ref()
            .and_then(|id| self.projects.iter().position(|p| &p.id == id))
            .or_else(|| self.project_of(tabs[0].0));
        self.tabs.clear();
        self.parked.clear();
        self.dock_places.clear();
        self.remembered.clear();
        self.current_project = project;
        for (ix, pinned) in &tabs {
            let owner = self.project_of(*ix);
            if let Some(owner) = owner {
                self.projects[owner].open = true;
            }
            let id = self.next_id;
            self.next_id += 1;
            let tab = Tab {
                id,
                conversation: *ix,
                pinned: *pinned,
                closing: false,
                x: Tween::still(-1., now),
                w: Tween::still(-1., now),
            };
            if owner == self.current_project {
                self.tabs.push(tab);
            } else if let Some(owner) = owner {
                self.parked
                    .entry(owner)
                    .or_insert((Vec::new(), id))
                    .0
                    .push(tab);
            }
        }
        if self.tabs.is_empty() {
            // The saved project is gone: show the first project that has a tab.
            if let Some(&owner) = self.parked.keys().min() {
                if let Some((list, _)) = self.parked.remove(&owner) {
                    self.current_project = Some(owner);
                    self.tabs = list;
                }
            }
        }
        let chosen = view
            .active
            .as_ref()
            .and_then(|id| {
                self.tabs
                    .iter()
                    .find(|t| &self.conversations[t.conversation].session_id == id)
            })
            .or(self.tabs.first())
            .map(|t| t.id);
        if let Some(id) = chosen {
            self.active = id;
            self.history.clear();
            self.history.push(id);
            let conversation = self
                .tabs
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.conversation);
            if let Some(conversation) = conversation {
                self.ensure_loaded(conversation);
            }
        }
        cx.notify();
    }

    /// Makes the next arrival at `computer` open `session` as the active tab.
    pub(super) fn open_on_arrival(&mut self, computer: &str, session: String, project: String) {
        let view = self.computers.views.entry(computer.to_owned()).or_default();
        view.tabs.retain(|(id, _, _)| *id != session);
        view.tabs.push((session.clone(), false, project.clone()));
        view.active = Some(session);
        view.project = Some(project);
    }

    /// The footer's chip for another computer: its name and where the connection is, and a way
    /// back to this Mac that works even when the computer has no project to switch from.
    /// The bottom bar's switch beside Ações: the computer the window shows now, as an icon and
    /// "Mac" or "VPS", with a status dot while it is a remote one; a click goes to the other.
    /// Shown once another computer is paired.
    pub(super) fn computer_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let other = self.computers.list.first()?.id.clone();
        let remote = self.on_remote();
        let name = self.computers.active_name();
        let (dot, tip) = match self.computers.status {
            Status::Open => (0x7fd19b, name.clone()),
            Status::Connecting => (0xd8b36a, tf("Conectando a {}…", &[&name])),
            Status::Lost => (
                0xe08a8a,
                tf("Sem conexão com {}. Tentando de novo…", &[&name]),
            ),
            Status::Unauthorized => (
                0xe08a8a,
                tf(
                    "{} não reconhece mais este Mac. Pareie de novo em Configurações → Conexões.",
                    &[&name],
                ),
            ),
        };
        let tip = if remote {
            format!("{tip} · {}", t("clique para usar este Mac"))
        } else {
            t("Este Mac · clique para usar a VPS").to_owned()
        };
        Some(
            div()
                .id("computer-switch")
                .h(px(24.))
                .px(px(6.))
                .flex_shrink_0()
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .gap(px(6.))
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(content(0.75))
                .hover(|s| s.bg(content(0.08)).text_color(content(0.95)))
                .tooltip(crate::shell::message_actions::text_tip(tip))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.on_remote() {
                        this.switch_computer(HOME, cx)
                    } else {
                        this.switch_computer(&other, cx)
                    }
                }))
                .when(remote, |d| {
                    d.child(div().size(px(6.)).rounded_full().bg(rgb(dot)))
                })
                .child(icon(if remote { "server" } else { "laptop" }, 14.).text_color(content(0.7)))
                .child(if remote { "VPS" } else { "Mac" })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(tabs: &[(&str, bool)]) -> View {
        View {
            tabs: tabs
                .iter()
                .map(|(id, pinned)| ((*id).into(), *pinned, "p".into()))
                .collect(),
            ..View::default()
        }
    }

    #[test]
    fn saved_tabs_keep_their_order_and_drop_what_is_gone() {
        let sessions: Vec<String> = ["a", "b", "", "c"].iter().map(|s| (*s).into()).collect();
        let kept = surviving(
            &view(&[
                ("c", false),
                ("zzz", true),
                ("a", true),
                ("c", false),
                ("", false),
            ]),
            &sessions,
        );
        assert_eq!(kept, vec![(3, false), (0, true)]);
        assert!(surviving(&view(&[("x", false)]), &sessions).is_empty());
    }
}
