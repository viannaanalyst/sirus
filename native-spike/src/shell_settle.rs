//! Idle conversations settle (ADR-198): the docked "N conversas paradas" row at the end of the
//! sidebar's list, the fade and bar of each row, reviving a conversation the owner opens or sends
//! to, and stopping what a settled conversation left running. A child module of the shell, so it
//! reaches its fields. The decisions are in `conversation_settle`.

use super::*;
use crate::conversation_settle::{self as settle, MAX_DAYS};
use crate::project_scripts::cancel_args;
use chrono::{DateTime, Utc};

/// How often the window checks which conversations have settled.
pub(super) const SETTLE_CHECK: std::time::Duration = std::time::Duration::from_secs(60);

impl Shell {
    /// The idle limit from Settings → Geral, or None when settling is off.
    pub(super) fn settle_limit(&self) -> Option<chrono::Duration> {
        let days = self.settings["settleIdleDays"].as_u64().unwrap_or(7);
        settle::limit_of(
            self.settings["settleIdleEnabled"].as_bool().unwrap_or(true),
            u32::try_from(days).unwrap_or(MAX_DAYS),
        )
    }

    /// When the owner last opened or sent to the session, if it was revived (`settleRevived`).
    fn opened_at(&self, session: &str) -> Option<DateTime<Utc>> {
        self.settings["settleRevived"][session]
            .as_str()
            .and_then(settle::parse)
    }

    /// The conversation's last activity, or the owner's opening of it when that is later.
    fn activity_of(&self, index: usize) -> Option<DateTime<Utc>> {
        let conversation = &self.conversations[index];
        settle::effective_activity(
            settle::parse(&conversation.last_activity),
            self.opened_at(&conversation.session_id),
        )
    }

    /// Whether the conversation has settled at `now`: past the limit and not working.
    pub(super) fn settled_now(&self, index: usize, now: DateTime<Utc>) -> bool {
        let conversation = &self.conversations[index];
        settle::is_settled(
            self.activity_of(index),
            now,
            self.settle_limit(),
            conversation.working() || !conversation.pending.is_empty(),
        )
    }

    /// The settled conversations of the project's list, newest first. A conversation the list
    /// filters out stays out of the docked group too.
    pub(super) fn settled_in_list(&self, project: usize, now: DateTime<Utc>) -> Vec<usize> {
        let mut settled: Vec<usize> = self.projects[project]
            .conversations
            .iter()
            .copied()
            .filter(|c| self.in_sidebar_list(project, *c) && self.settled_now(*c, now))
            .collect();
        settled.sort_by_key(|c| std::cmp::Reverse(self.activity_of(*c)));
        settled
    }

    /// A row's opacity and accent-bar fill at `now`. With settling off, a row is opaque, no bar.
    pub(super) fn fade_of(&self, index: usize, now: DateTime<Utc>) -> (f32, f32) {
        let (Some(limit), Some(activity)) = (self.settle_limit(), self.activity_of(index)) else {
            return (1., 0.);
        };
        let age = now - activity;
        (settle::opacity(age, limit), settle::fill(age, limit))
    }

    /// The conversations on screen: the active tab's, while a tab is active, and those in the
    /// split panes. A conversation on screen is revived rather than settled.
    fn on_screen(&self) -> Vec<usize> {
        let mut shown: Vec<usize> = self
            .tabs
            .iter()
            .any(|t| t.id == self.active)
            .then(|| self.active_conversation())
            .into_iter()
            .collect();
        let panes = self.split.sessions();
        shown.extend(
            self.conversations
                .iter()
                .enumerate()
                .filter(|(_, c)| panes.contains(&c.session_id))
                .map(|(index, _)| index),
        );
        shown
    }

    /// Makes a settled conversation fresh again, because the owner opened it or sent to it. The
    /// time is kept in the settings, so a restart keeps it.
    pub(super) fn revive(&mut self, index: usize, cx: &mut Context<Self>) {
        let now = Utc::now();
        let session = self.conversations[index].session_id.clone();
        let mut opened = self.settings["settleRevived"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        // An opening older than the limit holds nothing back any more; forget it.
        if let Some(limit) = self.settle_limit() {
            opened.retain(|_, at| {
                at.as_str()
                    .and_then(settle::parse)
                    .is_some_and(|at| now - at < limit)
            });
        }
        opened.insert(session.clone(), json!(now.to_rfc3339()));
        self.settings["settleRevived"] = Value::Object(opened);
        self.settled_stopped.remove(&session);
        self.backend
            .invoke("save_settings", json!({ "settings": self.settings }));
        cx.notify();
    }

    /// Stops what a settled conversation left running: its title tabs' terminals, its panel's
    /// shells and a script still running in it. Returns false, to try again later, while the
    /// conversation is on screen.
    fn stop_leftovers(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let session = self.conversations[index].session_id.clone();
        if self.on_screen().contains(&index) {
            return false;
        }
        self.drop_session_title_tabs(&session, cx);
        // The panel's shells stop when its last handle goes; it is built again if the owner opens
        // the conversation.
        self.pane_views.remove(&(0, session.clone()));
        let running = self.conversations[index]
            .scripts
            .iter()
            .any(|run| run.status == crate::project_scripts::Status::Running);
        if running {
            self.script_action(cancel_args(&session), cx);
        }
        true
    }

    /// Once a minute: revives the conversation in view, stops the leftovers of each newly settled
    /// conversation, and redraws when the settled set changes.
    pub(super) fn settle_check(&mut self, cx: &mut Context<Self>) {
        let now = Utc::now();
        for index in self.on_screen() {
            if self.settled_now(index, now) {
                self.revive(index, cx);
            }
        }
        let settled: Vec<usize> = (0..self.conversations.len())
            .filter(|index| self.settled_now(*index, now))
            .collect();
        let sessions: HashSet<String> = settled
            .iter()
            .map(|index| self.conversations[*index].session_id.clone())
            .collect();
        let changed = sessions != self.settled_known;
        self.settled_stopped
            .retain(|session| sessions.contains(session));
        for index in settled {
            let session = self.conversations[index].session_id.clone();
            if !self.settled_stopped.contains(&session) && self.stop_leftovers(index, cx) {
                self.settled_stopped.insert(session);
            }
        }
        self.settled_known = sessions;
        if changed {
            cx.notify();
        }
    }

    /// The docked row at the end of the list: "N conversas paradas" with a chevron. A click opens
    /// or closes the settled conversations under it, drawn like the date headings.
    pub(super) fn settled_row(&self, y: f32, alpha: f32, cx: &mut Context<Self>) -> AnyElement {
        let count = self.settled_listed;
        let label = if count == 1 {
            t("1 conversa parada").to_owned()
        } else {
            tf("{} conversas paradas", &[&count])
        };
        let open = self.settled_open;
        div()
            .id("settled-group")
            .absolute()
            .left_0()
            .right_0()
            .top(px(y))
            .h(px(HEADING))
            .opacity(alpha)
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(11.5))
            .text_color(content(0.42))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.settled_open = !this.settled_open;
                cx.notify();
            }))
            .child(
                icon(
                    if open {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                    12.,
                )
                .text_color(content(0.45)),
            )
            .child(label)
            .into_any_element()
    }
}
