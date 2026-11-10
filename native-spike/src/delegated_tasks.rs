//! Tasks a reply delegated to other models (ADR-111), under the reply itself: who runs each one, its
//! state, how long it has run, a Stop while it works, and a click opens it. A task that waits on an
//! approval or a question shows that request inside its row, with the buttons its own tab has.
//! Two or more tasks of one reply form a group that folds. The tasks also nest under their parent
//! conversation in the sidebar instead of listing as conversations of their own.
//! This is DelegatedTasks.tsx, shown where ReplyExtras.tsx puts it, with the same rule for what
//! counts as delegated.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;

/// The states a delegated task still works in; its Stop shows only then (DelegatedTasks.tsx).
pub(crate) fn active(status: &str) -> bool {
    matches!(status, "starting" | "running" | "waiting")
}

/// The state's word under the task's title (`delegate.state.*`). Done is also the fallback.
pub(crate) fn state_label(status: &str) -> &'static str {
    match status {
        "starting" => t("Iniciando"),
        "running" => t("Trabalhando"),
        "waiting" => t("Precisa de você"),
        "failed" => t("Falhou"),
        "stopped" => t("Parada"),
        _ => t("Concluída"),
    }
}

/// A delegated task's row in the sidebar: 28 px tall, one after another with nothing between.
pub(crate) const TASK_ROW: f32 = 28.;

/// Where a task row sits when its slot starts at `y`: the row itself (top, height) is one pixel
/// inside the slot so the hover and selection washes do not touch, and the rail (top, height)
/// covers the whole slot, so the guide lines of consecutive rows meet and read as one line.
pub(crate) fn task_slot(y: f32) -> ((f32, f32), (f32, f32)) {
    ((y + 1., TASK_ROW - 2.), (y, TASK_ROW))
}

/// The launched sessions that notify their reply, in the order the reply launched them.
pub(crate) fn delegated_ids(launched: &[String], notifies: impl Fn(&str) -> bool) -> Vec<String> {
    launched.iter().filter(|id| notifies(id)).cloned().collect()
}

/// "Delegada para Codex · gpt-5 · Trabalhando", the line under a task's title.
pub(crate) fn subtitle(provider: &str, model: Option<&str>, status: &str) -> String {
    let mut line = tf("Delegada para {}", &[&provider]);
    if let Some(model) = model.filter(|model| !model.is_empty()) {
        line.push_str(" · ");
        line.push_str(model);
    }
    line.push_str(" · ");
    line.push_str(state_label(status));
    line
}

/// The `stop_agent` call's arguments for one task.
pub(crate) fn stop_args(session_id: &str) -> Value {
    json!({ "sessionId": session_id })
}

/// "3m 05s", "1h 02m" or "42s": how long a task has run.
pub(crate) fn elapsed_text(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// Seconds a task has run: up to now while it works, up to its last activity once it stopped.
pub(crate) fn elapsed_secs(
    created: &str,
    last_activity: &str,
    active: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<u64> {
    let start = chrono::DateTime::parse_from_rfc3339(created).ok()?;
    let end = if active {
        now
    } else {
        chrono::DateTime::parse_from_rfc3339(last_activity)
            .ok()?
            .with_timezone(&chrono::Utc)
    };
    Some(
        (end - start.with_timezone(&chrono::Utc))
            .num_seconds()
            .max(0) as u64,
    )
}

/// "3 tarefas delegadas · 2 concluídas", the header of a group of tasks.
pub(crate) fn group_summary(total: usize, done: usize) -> String {
    let finished = if done == 1 {
        tf("{} concluída", &[&done])
    } else {
        tf("{} concluídas", &[&done])
    };
    format!("{} · {finished}", tf("{} tarefas delegadas", &[&total]))
}

/// The provider stack a parent conversation shows for its delegated tasks: each provider once, the
/// most recent task first, three tiles at most, with the total and whether any waits on the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskStack {
    pub providers: Vec<String>,
    pub count: usize,
    pub waiting: bool,
}

/// Composes a parent row's stack from its children, given oldest first as `nest` returns them.
pub(crate) fn task_stack(children: &[(String, bool)]) -> TaskStack {
    let mut providers = Vec::new();
    for (provider, _) in children.iter().rev() {
        if providers.len() >= 3 {
            break;
        }
        if !providers.iter().any(|seen| seen == provider) {
            providers.push(provider.clone());
        }
    }
    TaskStack {
        providers,
        count: children.len(),
        waiting: children.iter().any(|(_, waiting)| *waiting),
    }
}

/// Splits a project's conversations, given as (index, parent index), into those the list shows and
/// the tasks nested under each of them, oldest first (the list is newest first). A task whose
/// parent is not in the list stays in the list; a task of a task nests under the top conversation.
pub(crate) fn nest(
    list: &[(usize, Option<usize>)],
) -> (Vec<usize>, std::collections::HashMap<usize, Vec<usize>>) {
    let listed = |ix: usize| list.iter().any(|(c, _)| *c == ix);
    let parent_of = |ix: usize| {
        list.iter()
            .find(|(c, _)| *c == ix)
            .and_then(|(_, parent)| *parent)
            .filter(|parent| *parent != ix && listed(*parent))
    };
    let mut top = Vec::new();
    let mut nested: std::collections::HashMap<usize, Vec<usize>> = Default::default();
    for (ix, _) in list {
        // Climb to the conversation the list shows; a loop of parents ends the climb.
        let mut root = *ix;
        let mut steps = 0;
        while let Some(parent) = parent_of(root) {
            root = parent;
            steps += 1;
            if steps > list.len() {
                root = *ix;
                break;
            }
        }
        if root == *ix {
            top.push(*ix);
        } else {
            nested.entry(root).or_default().push(*ix);
        }
    }
    for tasks in nested.values_mut() {
        tasks.reverse();
    }
    (top, nested)
}

/// The dot that tells a task's state at a glance.
fn state_color(status: &str) -> gpui::Hsla {
    match status {
        "running" | "starting" => accent(),
        "waiting" => gpui::rgb(0xe0b25a).into(),
        "failed" => gpui::rgb(0xe08a8a).into(),
        _ => content(0.35),
    }
}

impl Shell {
    /// The conversation this task nests under in the project's list, when that one is there too.
    pub(super) fn nested_parent(&self, project: usize, conversation: usize) -> Option<usize> {
        let parent_id = self.conversations[conversation].parent_session.as_deref()?;
        let parent = self
            .conversations
            .iter()
            .position(|c| c.session_id == parent_id)?;
        (parent != conversation && self.projects[project].conversations.contains(&parent))
            .then_some(parent)
    }

    /// The project's list as the sidebar nests it: top conversations and the tasks under each.
    pub(super) fn project_tree(
        &self,
        project: usize,
    ) -> (Vec<usize>, std::collections::HashMap<usize, Vec<usize>>) {
        let list: Vec<(usize, Option<usize>)> = self.projects[project]
            .conversations
            .iter()
            .map(|&c| (c, self.nested_parent(project, c)))
            .collect();
        nest(&list)
    }

    /// The tasks nested under a conversation, oldest first.
    pub(super) fn tasks_of(&self, parent: usize) -> Vec<usize> {
        self.project_of(parent)
            .map(|project| self.project_tree(project).1)
            .and_then(|mut kids| kids.remove(&parent))
            .unwrap_or_default()
    }

    /// The tasks under a parent that the person opened: they follow its row while open, and slide
    /// away when the stack closes.
    pub(super) fn push_tasks(
        &self,
        conversation: usize,
        kids: &std::collections::HashMap<usize, Vec<usize>>,
        wanted: &mut Vec<(Row, f32, f32)>,
        y: &mut f32,
    ) {
        if !self.tasks_open.contains(&conversation) {
            return;
        }
        let Some(tasks) = kids.get(&conversation) else {
            return;
        };
        for task in tasks {
            wanted.push((Row::Conversation(*task), *y, *y));
            *y += TASK_ROW;
        }
    }

    /// The stack of provider tiles on a parent row: a click opens or closes its tasks, and an amber
    /// dot shows while one of them waits on the person, so a closed list never hides a request.
    pub(super) fn task_stack_view(
        &self,
        parent: usize,
        tasks: &[usize],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let children: Vec<(String, bool)> = tasks
            .iter()
            .map(|ix| {
                let conversation = &self.conversations[*ix];
                (
                    conversation.provider.to_owned(),
                    !conversation.pending.is_empty(),
                )
            })
            .collect();
        let stack = task_stack(&children);
        let open = self.tasks_open.contains(&parent);
        div()
            .id(("tasks-stack", parent))
            .h(px(20.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .pl(px(2.))
            .pr(px(3.))
            .rounded(px(crate::theme::RADIUS_SM))
            .cursor_pointer()
            .when(open, |d| d.bg(content(0.08)))
            .hover(|s| s.bg(content(0.1)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.tasks_open.remove(&parent) {
                    this.tasks_open.insert(parent);
                }
                cx.notify();
            }))
            .children(stack.providers.iter().enumerate().map(|(at, provider)| {
                div()
                    .when(at > 0, |d| d.ml(px(-5.)))
                    .size(px(16.))
                    .rounded(px(5.))
                    .bg(crate::theme::black(0.65))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(mark_tile(
                        &crate::tab_tile::TileSize {
                            w: 14.,
                            h: 14.,
                            radius: 4.,
                        },
                        Face::provider(provider),
                        provider_glyph(provider, 9.),
                    ))
            }))
            .child(
                div()
                    .ml(px(5.))
                    .text_size(px(11.))
                    .text_color(content(0.55))
                    .child(stack.count.to_string()),
            )
            .when(stack.waiting, |d| {
                d.child(
                    div()
                        .ml(px(5.))
                        .size(px(6.))
                        .rounded_full()
                        .bg(state_color("waiting")),
                )
            })
            .into_any_element()
    }

    /// A delegated task under its parent, drawn like the transcript's activity tree: the same rail
    /// and elbow, then its state, its provider, its title and its age.
    pub(super) fn task_row(
        &self,
        index: usize,
        y: f32,
        alpha: f32,
        selected: bool,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let conversation = &self.conversations[index];
        let last = self
            .project_of(index)
            .and_then(|project| self.nested_parent(project, index))
            .is_none_or(|parent| self.tasks_of(parent).last() == Some(&index));
        let waiting = !conversation.pending.is_empty();
        let status = conversation.status.as_str();
        // A finished task shows no mark: only running, failed and waiting ones say something.
        let mark: Option<AnyElement> = if waiting {
            Some(
                div()
                    .size(px(7.))
                    .rounded_full()
                    .bg(state_color("waiting"))
                    .into_any_element(),
            )
        } else if status == "failed" {
            Some(
                icon("close", 10.)
                    .text_color(gpui::rgb(0xff7a7a))
                    .into_any_element(),
            )
        } else if active(status) {
            Some(
                div()
                    .size(px(12.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(spinner_at(
                        (now.duration_since(self.epoch).as_secs_f32() / SPINNER_PERIOD).fract(),
                    ))
                    .into_any_element(),
            )
        } else {
            None
        };
        let ((row_top, row_h), (rail_top, rail_h)) = task_slot(y);
        let row = div()
            .id(("task", index))
            .absolute()
            .left(px(8.))
            .right(px(6.))
            .top(px(row_top))
            .h(px(row_h))
            .opacity(alpha)
            .rounded(px(crate::theme::RADIUS_MD))
            .pl(px(TILE_RAIL_INDENT))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(7.))
            .when(selected, |d| {
                d.bg(content(crate::segment_pill::ROW_FILL)).child(
                    div()
                        .absolute()
                        .left(px(0.))
                        .top(px(4.5))
                        .h(px(18.))
                        .w(px(2.5))
                        .rounded(px(1.25))
                        .bg(accent()),
                )
            })
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.open(index, cx)))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(rail_top - row_top))
                    .h(px(rail_h))
                    .child(rail(last)),
            )
            .children(mark.map(|mark| {
                div()
                    .size(px(14.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(mark)
            }))
            .child(crate::tab_tile::provider_mark(conversation.provider, 14.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(content(if selected { 1. } else { 0.68 }))
                    .child(conversation.title.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(content(0.35))
                    .child(conversation.age()),
            );
        crate::hover_track::list("sidebar")
            .row(row, index, selected)
            .into_any_element()
    }

    /// The cards of the tasks a turn delegated, or none when it delegated none. Two or more fold
    /// into one group with a summary; each task shows how long it has run, and a request it waits
    /// on, answered here as it would be in the task's own tab.
    pub(super) fn delegated_tasks_view(
        &self,
        turn: &Turn,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ids = delegated_ids(&turn.launched, |id| {
            self.conversations
                .iter()
                .any(|c| c.session_id == id && c.delegated_notify)
        });
        let tasks: Vec<usize> = ids
            .iter()
            .filter_map(|id| self.conversations.iter().position(|c| c.session_id == *id))
            .collect();
        if tasks.is_empty() {
            return None;
        }
        let now = chrono::Utc::now();
        let group = tasks.len() >= 2;
        let key = ids.first().cloned().unwrap_or_default();
        let folded = group && self.delegated_folded.contains(&key);
        let done = tasks
            .iter()
            .filter(|ix| !active(&self.conversations[**ix].status))
            .count();
        let asking = tasks
            .iter()
            .any(|ix| !self.conversations[*ix].pending.is_empty());
        let track = crate::hover_track::list("delegated-tasks");
        let rows = tasks.into_iter().map(|ix| {
            let conversation = &self.conversations[ix];
            let status = conversation.status.clone();
            let session = conversation.session_id.clone();
            let provider = provider_title(conversation.provider);
            let elapsed = elapsed_secs(
                &conversation.created_at,
                &conversation.last_activity,
                active(&status),
                now,
            )
            .map(elapsed_text);
            let open = div()
                .id(format!("delegated-open-{ix}"))
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .gap(px(10.))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.open(ix, cx)))
                .child(crate::tab_tile::provider_mark(conversation.provider, 16.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(13.))
                                .text_color(content(0.9))
                                .child(conversation.title.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.5))
                                .text_color(content(0.5))
                                .child(subtitle(provider, conversation.model.as_deref(), &status)),
                        ),
                )
                .children(elapsed.map(|text| {
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.5))
                        .text_color(content(0.5))
                        .child(text)
                }))
                .child(
                    div()
                        .size(px(7.))
                        .rounded_full()
                        .flex_shrink_0()
                        .bg(state_color(&status)),
                );
            let stop = active(&status).then(|| {
                let stop_id = session.clone();
                div()
                    .id(format!("delegated-stop-{ix}"))
                    .size(px(22.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.backend.invoke("stop_agent", stop_args(&stop_id));
                        cx.notify();
                    }))
                    .child(icon("square-fill", 10.).text_color(content(0.6)))
            });
            let row = track.row_named(
                div()
                    .w_full()
                    .h(px(46.))
                    .px(px(10.))
                    .rounded(px(crate::theme::RADIUS_LG))
                    .border_1()
                    .border_color(crate::theme::border())
                    .bg(content(0.03))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(open)
                    .children(stop)
                    .child(icon("chevron-right", 13.).text_color(content(0.4))),
                &session,
                false,
            );
            // The task's own request, answered here (it resolves in the task, not in this reply).
            let request = conversation
                .pending
                .first()
                .map(|request| self.pending_request_card(ix, request, cx));
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(row)
                .children(request)
        });
        let rows: Vec<_> = rows.collect();
        let header = group.then(|| {
            let toggle = key.clone();
            div()
                .id(format!("delegated-group-{key}"))
                .h(px(26.))
                .px(px(4.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(content(0.6))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.delegated_folded.remove(&toggle) {
                        this.delegated_folded.insert(toggle.clone());
                    }
                    cx.notify();
                }))
                .child(
                    icon(
                        if folded {
                            "chevron-right"
                        } else {
                            "chevron-down"
                        },
                        12.,
                    )
                    .text_color(content(0.45)),
                )
                .child(group_summary(ids.len(), done))
                .when(asking && folded, |d| {
                    d.child(div().size(px(6.)).rounded_full().bg(state_color("waiting")))
                })
        });
        Some(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(track.pill())
                .children(header)
                .when(!folded, |d| d.children(rows))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_slots_abut_so_the_guide_is_one_line() {
        let ((top, height), (rail_top, rail_h)) = task_slot(100.);
        assert_eq!((top, height), (101., 26.));
        assert_eq!((rail_top, rail_h), (100., TASK_ROW));
        let (_, (next_top, _)) = task_slot(100. + TASK_ROW);
        assert_eq!(rail_top + rail_h, next_top);
        assert_eq!(TASK_ROW, 28.);
    }

    #[test]
    fn only_notifying_sessions_are_delegated_and_keep_their_order() {
        let launched = vec!["b".to_owned(), "a".to_owned(), "c".to_owned()];
        let notifies = |id: &str| id != "a";
        assert_eq!(delegated_ids(&launched, notifies), ["b", "c"]);
        assert!(delegated_ids(&launched, |_| false).is_empty());
    }

    #[test]
    fn working_states_are_the_ones_with_a_stop() {
        for status in ["starting", "running", "waiting"] {
            assert!(active(status), "{status}");
        }
        for status in ["idle", "completed", "failed", "stopped"] {
            assert!(!active(status), "{status}");
        }
    }

    #[test]
    fn states_read_as_their_words_and_unknown_ones_read_as_done() {
        assert_eq!(state_label("running"), t("Trabalhando"));
        assert_eq!(state_label("waiting"), t("Precisa de você"));
        assert_eq!(state_label("failed"), t("Falhou"));
        assert_eq!(state_label("stopped"), t("Parada"));
        assert_eq!(state_label("completed"), t("Concluída"));
        assert_eq!(state_label("something-new"), t("Concluída"));
    }

    #[test]
    fn the_line_names_provider_model_and_state() {
        let with_model = subtitle("Codex", Some("gpt-5"), "running");
        assert_eq!(
            with_model,
            format!(
                "{} · gpt-5 · {}",
                tf("Delegada para {}", &[&"Codex"]),
                t("Trabalhando")
            )
        );
        let without = subtitle("Codex", Some(""), "idle");
        assert_eq!(
            without,
            format!(
                "{} · {}",
                tf("Delegada para {}", &[&"Codex"]),
                t("Concluída")
            )
        );
        assert_eq!(subtitle("Codex", None, "idle"), without);
    }

    #[test]
    fn stop_sends_only_the_session_id() {
        assert_eq!(stop_args("s-1"), json!({ "sessionId": "s-1" }));
    }

    #[test]
    fn elapsed_reads_as_seconds_minutes_or_hours() {
        assert_eq!(elapsed_text(0), "0s");
        assert_eq!(elapsed_text(59), "59s");
        assert_eq!(elapsed_text(60), "1m 00s");
        assert_eq!(elapsed_text(185), "3m 05s");
        assert_eq!(elapsed_text(3600), "1h 00m");
        assert_eq!(elapsed_text(3725), "1h 02m");
    }

    #[test]
    fn a_working_task_runs_until_now_and_a_stopped_one_until_its_last_activity() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-01-01T10:10:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let (start, last) = ("2026-01-01T10:00:00Z", "2026-01-01T10:04:30Z");
        assert_eq!(elapsed_secs(start, last, true, now), Some(600));
        assert_eq!(elapsed_secs(start, last, false, now), Some(270));
        assert_eq!(elapsed_secs("", last, true, now), None);
        assert_eq!(elapsed_secs(start, "", false, now), None);
        // A clock that reads before the start never goes negative.
        assert_eq!(elapsed_secs(last, start, false, now), Some(0));
    }

    #[test]
    fn the_group_header_counts_tasks_and_finished_ones() {
        assert_eq!(
            group_summary(3, 2),
            format!(
                "{} · {}",
                tf("{} tarefas delegadas", &[&3]),
                tf("{} concluídas", &[&2])
            )
        );
        assert!(group_summary(2, 1).ends_with(&tf("{} concluída", &[&1])));
        assert!(group_summary(2, 0).ends_with(&tf("{} concluídas", &[&0])));
    }

    #[test]
    fn the_stack_keeps_unique_providers_newest_first_and_counts_every_task() {
        let children = vec![
            ("claude".to_owned(), false),
            ("codex".to_owned(), true),
            ("codex".to_owned(), false),
            ("deepseek".to_owned(), false),
            ("claude".to_owned(), false),
        ];
        let stack = task_stack(&children);
        assert_eq!(stack.providers, ["claude", "deepseek", "codex"]);
        assert_eq!(stack.count, 5);
        assert!(stack.waiting);
    }

    #[test]
    fn the_stack_caps_at_three_tiles_and_reports_no_waiting() {
        let children: Vec<(String, bool)> = ["a", "b", "c", "d"]
            .iter()
            .map(|provider| ((*provider).to_owned(), false))
            .collect();
        let stack = task_stack(&children);
        assert_eq!(stack.providers, ["d", "c", "b"]);
        assert_eq!(stack.count, 4);
        assert!(!stack.waiting);
        assert!(task_stack(&[]).providers.is_empty());
        assert_eq!(task_stack(&[]).count, 0);
    }

    #[test]
    fn tasks_nest_under_their_parent_and_orphans_stay_in_the_list() {
        // Newest first, as the list is: 5 is a task of 2, 4 of 2, 3 of a parent that is gone.
        let list = [
            (5, Some(2)),
            (4, Some(2)),
            (3, Some(99)),
            (2, None),
            (1, None),
        ];
        let (top, nested) = nest(&list);
        assert_eq!(top, [3, 2, 1]);
        assert_eq!(nested.get(&2), Some(&vec![4, 5]), "oldest first");
        assert!(!nested.contains_key(&1) && !nested.contains_key(&3));
    }

    #[test]
    fn a_task_of_a_task_nests_under_the_top_conversation_and_loops_do_not_hide_rows() {
        let list = [(3, Some(2)), (2, Some(1)), (1, None)];
        let (top, nested) = nest(&list);
        assert_eq!(top, [1]);
        assert_eq!(nested.get(&1), Some(&vec![2, 3]));
        // Two sessions that name each other would vanish: they stay listed instead.
        let (top, nested) = nest(&[(1, Some(2)), (2, Some(1))]);
        assert_eq!(top, [1, 2]);
        assert!(nested.is_empty());
        assert_eq!(nest(&[(7, Some(7))]).0, [7]);
    }
}
