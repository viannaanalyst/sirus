//! Several conversations picked in the sidebar: ⌘-click adds or drops one, ⇧-click takes the run
//! of the project's list up to it, a plain click goes back to opening one. A right click on a
//! picked row acts on all of them.

use super::*;

impl Shell {
    /// Handles a click with ⌘ or ⇧; false when it is a plain click, which clears the pick.
    pub(super) fn pick_click(
        &mut self,
        index: usize,
        modifiers: gpui::Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        if modifiers.platform {
            if self.picked.is_empty() {
                let active = self.active_conversation();
                if active != index && self.project_of(active) == self.project_of(index) {
                    self.picked.push(active);
                }
            }
            if let Some(at) = self.picked.iter().position(|c| *c == index) {
                self.picked.remove(at);
            } else {
                self.picked.push(index);
            }
            cx.notify();
            return true;
        }
        if modifiers.shift {
            let anchor = self
                .picked
                .last()
                .copied()
                .unwrap_or_else(|| self.active_conversation());
            if let Some(project) = self.project_of(index) {
                let list = &self.projects[project].conversations;
                let from = list.iter().position(|c| *c == anchor);
                let to = list.iter().position(|c| *c == index);
                self.picked = match (from, to) {
                    (Some(a), Some(b)) => list[a.min(b)..=a.max(b)].to_vec(),
                    _ => vec![index],
                };
            }
            cx.notify();
            return true;
        }
        self.picked.clear();
        false
    }

    /// The menu for a right click on a picked row while more than one is picked.
    pub(super) fn picked_menu(&self, index: usize) -> bool {
        self.picked.len() > 1 && self.picked.contains(&index)
    }

    pub(super) fn pin_picked(&mut self, cx: &mut Context<Self>) {
        for index in std::mem::take(&mut self.picked) {
            self.open(index, cx);
            let tab = self
                .tabs
                .iter()
                .find(|t| t.conversation == index && !t.closing && !t.pinned)
                .map(|t| t.id);
            if let Some(tab) = tab {
                self.toggle_pin(tab, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn open_picked(&mut self, cx: &mut Context<Self>) {
        for index in std::mem::take(&mut self.picked) {
            self.open(index, cx);
        }
        cx.notify();
    }

    /// Asks once, then deletes every picked conversation that is not working.
    pub(super) fn ask_delete_picked(&mut self, cx: &mut Context<Self>) {
        let doomed: Vec<usize> = self
            .picked
            .iter()
            .copied()
            .filter(|c| !self.conversations[*c].working())
            .collect();
        if doomed.is_empty() {
            return;
        }
        let message = if doomed.len() < self.picked.len() {
            t("{n} conversas serão apagadas para sempre. As que estão trabalhando ficam.")
        } else {
            t("{n} conversas serão apagadas para sempre.")
        }
        .replace("{n}", &doomed.len().to_string());
        self.ask(
            t("Excluir conversas?").to_owned(),
            message,
            t("Excluir"),
            move |this, cx| {
                this.picked.clear();
                for index in doomed {
                    this.delete_conversation(index, cx);
                }
                cx.notify();
            },
            cx,
        );
    }
}
