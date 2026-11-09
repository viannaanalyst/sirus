//! Two Git functions of the environment card and the top bar, after Sirus: "Criar branch"
//! (EnvironmentPanel's BranchRow, with the name checked here before the host is asked) and the
//! state behind the top bar's Git split button (git_split.rs and git_button.rs, ADR-148 and
//! ADR-198): what the branch is ahead of or behind its upstream, when GitHub was last asked, what
//! the button is doing and whether its menu is open. The host does the Git work; this module
//! reads its answers and keeps the state. shell.rs asks `observe` on each frame, sends the
//! requests, and feeds the answers back, so an answer that changes nothing never asks for a
//! redraw.
use crate::git_split::Running;
use crate::i18n::t;
use crate::motion::{Curve, Tween};
use crate::theme::{accent, content, primary_action};
use gpui::{AnyElement, App, ClickEvent, Div, Window, div, prelude::*, px, rgb};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Sirus asks again every 5 minutes while the window is visible (lib/git-incoming.ts).
pub const INCOMING_REFRESH: Duration = Duration::from_secs(5 * 60);
/// Longest branch name the host takes (its MAX_BRANCH_NAME).
const MAX_BRANCH_NAME: usize = 120;

// --- Branch names -----------------------------------------------------------------------------

/// Why a typed branch name cannot be created. `Empty` shows no message: the button is just off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameProblem {
    Empty,
    TooLong,
    Invalid,
    Exists,
}

/// The trimmed name when it can be created as a new local branch. The rules are the host's
/// (`valid_branch_chars` plus `git check-ref-format --branch`), so a name passing here can still
/// be refused by the host, and its error is shown as is.
pub fn check_branch_name(raw: &str, existing: &[String]) -> Result<String, NameProblem> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(NameProblem::Empty);
    }
    if name.len() > MAX_BRANCH_NAME {
        return Err(NameProblem::TooLong);
    }
    if !valid_shape(name) {
        return Err(NameProblem::Invalid);
    }
    if existing.iter().any(|branch| branch == name) {
        return Err(NameProblem::Exists);
    }
    Ok(name.to_owned())
}

fn valid_shape(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'))
        && !name.starts_with('-')
        && name != "@"
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("@{")
        && !name.ends_with('/')
        && !name.ends_with('.')
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// The line shown under the field for a problem; `None` for `Empty`.
pub fn problem_text(problem: NameProblem) -> Option<&'static str> {
    match problem {
        NameProblem::Empty => None,
        NameProblem::TooLong => Some(t("Nome longo demais (120 caracteres no máximo).")),
        NameProblem::Invalid => Some(t(
            "Nome inválido: use letras, números, /, ., - e _, sem espaços nem .. e sem começar com - ou terminar com / ou ponto.",
        )),
        NameProblem::Exists => Some(t("Já existe um branch com esse nome.")),
    }
}

// --- Incoming state ---------------------------------------------------------------------------

/// What the selected conversation's branch has against its upstream (`git_incoming`).
#[derive(Debug, Clone, PartialEq)]
pub struct GitIncoming {
    pub branch: String,
    pub upstream: String,
    pub ahead: u64,
    pub behind: u64,
    /// Unix milliseconds of the host's last successful fetch of the upstream.
    pub checked_at: Option<u64>,
}

/// Reads `git_incoming`'s answer. `null` and anything malformed mean nothing to show.
pub fn parse_incoming(value: &Value) -> Option<GitIncoming> {
    let branch = value["branch"].as_str().filter(|b| !b.is_empty())?;
    let upstream = value["upstream"].as_str().filter(|u| !u.is_empty())?;
    let ahead = value["ahead"].as_u64()?;
    let behind = value["behind"].as_u64()?;
    Some(GitIncoming {
        branch: branch.to_owned(),
        upstream: upstream.to_owned(),
        ahead,
        behind,
        checked_at: value["checkedAt"].as_u64(),
    })
}

/// `git_sync_pull`'s answer, as the button needs it.
#[derive(Debug, Clone, PartialEq)]
pub enum Synced {
    Pulled(Option<GitIncoming>),
    UpToDate(Option<GitIncoming>),
    Diverged,
    /// The pull conflicted and was undone; these files were in the way.
    Conflict(Vec<String>),
    Failed(String),
}

pub fn sync_answer(ok: bool, value: &Value) -> Synced {
    if !ok {
        let message = value["message"]
            .as_str()
            .or_else(|| value.as_str())
            .map_or_else(|| t("O git falhou.").to_owned(), crate::i18n::from_host);
        return Synced::Failed(message);
    }
    match value["status"].as_str() {
        Some("pulled") => Synced::Pulled(parse_incoming(&value["incoming"])),
        Some("upToDate") => Synced::UpToDate(parse_incoming(&value["incoming"])),
        Some("diverged") => Synced::Diverged,
        Some("conflict") => Synced::Conflict(
            value["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|file| file.as_str().filter(|f| !f.is_empty()).map(str::to_owned))
                .take(50)
                .collect(),
        ),
        _ => Synced::Failed(t("O git falhou.").to_owned()),
    }
}

/// The Git button's state for the selected conversation.
#[derive(Default)]
pub struct IncomingState {
    session: String,
    folder: String,
    /// Bumped when the conversation or its folder changes; older answers are dropped.
    generation: u64,
    working: bool,
    window_active: bool,
    pub value: Option<GitIncoming>,
    /// What the button is doing; it ignores clicks meanwhile.
    pub running: Option<Running>,
    pub open: bool,
    pub tween: Option<Tween>,
}

impl IncomingState {
    /// Called on each frame with the selected conversation. True when the host should be asked
    /// now: the conversation or folder changed, a turn settled, or the window came back.
    pub fn observe(
        &mut self,
        session: &str,
        folder: &str,
        working: bool,
        window_active: bool,
    ) -> bool {
        let moved = session != self.session || folder != self.folder;
        let settled = self.working && !working;
        let back = window_active && !self.window_active;
        self.working = working;
        self.window_active = window_active;
        if moved {
            self.session = session.to_owned();
            self.folder = folder.to_owned();
            self.generation = self.generation.wrapping_add(1);
            self.value = None;
            self.running = None;
            self.open = false;
            self.tween = None;
        }
        !self.session.is_empty() && (moved || settled || back)
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The window is visible and a conversation is selected: the timer may ask.
    pub fn due(&self) -> bool {
        self.window_active && !self.session.is_empty()
    }

    /// A `git_incoming` answer. True when the button or menu must redraw.
    pub fn answer(&mut self, generation: u64, ok: bool, value: &Value) -> bool {
        if generation != self.generation {
            return false;
        }
        let next = if ok { parse_incoming(value) } else { None };
        if next == self.value {
            return false;
        }
        self.value = next;
        true
    }

    /// Starts `work` unless something is already running.
    pub fn begin(&mut self, work: Running) -> bool {
        if self.running.is_some() {
            return false;
        }
        self.running = Some(work);
        true
    }

    /// The work for `generation` ended, with `next` as the branch's new state when it brought
    /// one. False when the conversation changed meanwhile and nothing here applies.
    pub fn finish(&mut self, generation: u64, next: Option<Option<GitIncoming>>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.running = None;
        if let Some(next) = next {
            self.value = next;
        }
        true
    }

    /// Opens or closes the menu; `duration` is the fade (0 with reduced motion).
    pub fn toggle(&mut self, now: Instant, duration: f32) {
        if self.open {
            self.close_at(now, duration);
        } else {
            self.open = true;
            self.tween = Some(Tween::from_to(0., 1., now, duration, Curve::Expo));
        }
    }

    /// Closes the menu with its fade; true when it was open.
    pub fn close_at(&mut self, now: Instant, duration: f32) -> bool {
        let was = self.open;
        if was {
            self.open = false;
            self.tween = Some(Tween::from_to(1., 0., now, duration, Curve::Expo));
        }
        was
    }

    /// The menu is drawn while open and while it fades out.
    pub fn visible(&self, now: Instant) -> bool {
        self.open || self.tween.is_some_and(|tween| !tween.done(now))
    }
}

/// The name field's frame (after the commit field), with the error line under it.
pub fn branch_field(
    draft: &str,
    error: Option<&str>,
    can_create: bool,
    busy: bool,
    on_create: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let mut field: Div = div()
        .h(px(24.))
        .px(px(6.))
        .rounded(px(crate::theme::RADIUS_SM))
        .border_1()
        .border_color(crate::theme::border())
        .flex()
        .items_center()
        .gap(px(1.))
        .text_size(px(12.));
    field = if draft.is_empty() {
        field.child(div().w(px(1.5)).h(px(13.)).bg(accent())).child(
            div()
                .text_color(content(0.4))
                .child(t("Nome do novo branch")),
        )
    } else {
        field
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(content(0.95))
                    .child(draft.to_owned()),
            )
            .child(div().w(px(1.5)).h(px(13.)).bg(accent()))
    };
    let (fill, ink) = primary_action(can_create && !busy);
    let mut create = div()
        .id("env-branch-create")
        .h(px(22.))
        .px(px(8.))
        .rounded(px(crate::theme::RADIUS_SM))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.5))
        .bg(fill)
        .text_color(ink)
        .child(t("Criar"));
    if can_create && !busy {
        create = create
            .cursor_pointer()
            .hover(|s| s.opacity(0.88))
            .on_click(on_create);
    }
    let mut column = div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(field)
        .child(create);
    if let Some(error) = error {
        column = column.child(
            div()
                .px(px(6.))
                .text_size(px(11.5))
                .text_color(rgb(0xe08a8a))
                .child(error.to_owned()),
        );
    }
    column.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn existing() -> Vec<String> {
        vec!["main".into(), "feature/login".into()]
    }

    #[test]
    fn branch_names_follow_the_host_rules() {
        let ok = |name: &str| check_branch_name(name, &existing());
        assert_eq!(
            ok("  feature/new-ui_2.0  "),
            Ok("feature/new-ui_2.0".into())
        );
        assert_eq!(ok(""), Err(NameProblem::Empty));
        assert_eq!(ok("   "), Err(NameProblem::Empty));
        assert_eq!(ok("main"), Err(NameProblem::Exists));
        assert_eq!(ok("-rf"), Err(NameProblem::Invalid));
        assert_eq!(ok("a..b"), Err(NameProblem::Invalid));
        assert_eq!(ok("a b"), Err(NameProblem::Invalid));
        assert_eq!(ok("ação"), Err(NameProblem::Invalid));
        assert_eq!(ok("a~b"), Err(NameProblem::Invalid));
        assert_eq!(ok("a:b"), Err(NameProblem::Invalid));
        assert_eq!(ok("a@{b"), Err(NameProblem::Invalid));
        assert_eq!(ok("@"), Err(NameProblem::Invalid));
        assert_eq!(ok("trail/"), Err(NameProblem::Invalid));
        assert_eq!(ok("trail."), Err(NameProblem::Invalid));
        assert_eq!(ok("a//b"), Err(NameProblem::Invalid));
        assert_eq!(ok("/lead"), Err(NameProblem::Invalid));
        assert_eq!(ok(".hidden"), Err(NameProblem::Invalid));
        assert_eq!(ok("x/.hidden"), Err(NameProblem::Invalid));
        assert_eq!(ok("name.lock"), Err(NameProblem::Invalid));
        assert_eq!(ok(&"a".repeat(121)), Err(NameProblem::TooLong));
        assert!(ok(&"a".repeat(120)).is_ok());
    }

    #[test]
    fn problems_have_text_except_empty() {
        assert_eq!(problem_text(NameProblem::Empty), None);
        assert!(problem_text(NameProblem::Exists).is_some());
        assert!(problem_text(NameProblem::Invalid).is_some());
    }

    #[test]
    fn incoming_answers_are_read_like_the_react_client() {
        let value = json!({
            "branch": "main", "upstream": "origin/main", "ahead": 0, "behind": 2,
            "commits": [{"sha": "abcdef1", "subject": "Fix", "author": "Ana", "time": 1}],
            "checkedAt": 1_700_000_000_000u64
        });
        let parsed = parse_incoming(&value).expect("valid answer");
        assert_eq!(parsed.behind, 2);
        assert_eq!(parsed.checked_at, Some(1_700_000_000_000));
        assert_eq!(parse_incoming(&Value::Null), None);
        assert_eq!(
            parse_incoming(
                &json!({"branch": "", "upstream": "o/m", "ahead": 0, "behind": 1, "commits": []})
            ),
            None
        );
        assert_eq!(
            parse_incoming(
                &json!({"branch": "m", "upstream": "o/m", "ahead": -1, "behind": 1, "commits": []})
            ),
            None
        );
        let unchecked =
            json!({"branch": "m", "upstream": "o/m", "ahead": 0, "behind": 0, "checkedAt": null});
        assert_eq!(parse_incoming(&unchecked).unwrap().checked_at, None);
    }

    #[test]
    fn sync_answers_map_to_what_the_button_says() {
        assert_eq!(
            sync_answer(false, &json!({"message": "busy"})),
            Synced::Failed("busy".into())
        );
        assert_eq!(
            sync_answer(true, &json!({"status": "pulled", "incoming": null})),
            Synced::Pulled(None)
        );
        assert_eq!(
            sync_answer(true, &json!({"status": "upToDate"})),
            Synced::UpToDate(None)
        );
        assert_eq!(
            sync_answer(true, &json!({"status": "diverged", "incoming": null})),
            Synced::Diverged
        );
        assert_eq!(
            sync_answer(
                true,
                &json!({"status": "conflict", "files": ["a.rs", "", 3, "b.rs"]})
            ),
            Synced::Conflict(vec!["a.rs".into(), "b.rs".into()])
        );
        assert!(matches!(
            sync_answer(true, &json!({"status": "other"})),
            Synced::Failed(_)
        ));
    }

    #[test]
    fn refresh_is_asked_on_change_settle_and_return_only() {
        let mut state = IncomingState::default();
        assert!(
            !state.observe("", "", false, true),
            "nothing selected, nothing to ask"
        );
        assert!(
            state.observe("s1", "/p", false, true),
            "a new conversation asks"
        );
        assert!(
            !state.observe("s1", "/p", false, true),
            "the same frame does not ask again"
        );
        assert!(
            !state.observe("s1", "/p", true, true),
            "a turn starting does not ask"
        );
        assert!(
            state.observe("s1", "/p", false, true),
            "a turn settling asks"
        );
        assert!(!state.observe("s1", "/p", false, false));
        assert!(
            state.observe("s1", "/p", false, true),
            "the window coming back asks"
        );
        assert!(
            state.observe("s2", "/p", false, true),
            "switching conversation asks"
        );
        assert!(
            state.observe("s2", "/other", false, true),
            "a new folder asks"
        );
    }

    #[test]
    fn stale_answers_and_idle_repeats_change_nothing() {
        let mut state = IncomingState::default();
        state.observe("s1", "/p", false, true);
        let generation = state.generation();
        let value =
            json!({"branch": "m", "upstream": "o/m", "ahead": 0, "behind": 1, "commits": []});
        assert!(
            state.answer(generation, true, &value),
            "first answer redraws"
        );
        assert!(
            !state.answer(generation, true, &value),
            "the same answer does not redraw"
        );
        assert!(
            !state.answer(generation.wrapping_add(1), true, &Value::Null),
            "an old conversation is dropped"
        );
        assert!(state.value.is_some());
        assert!(
            state.answer(generation, false, &Value::Null),
            "a failed read hides the state"
        );
        assert!(state.value.is_none());
    }

    #[test]
    fn one_job_runs_at_a_time_and_a_late_finish_is_dropped() {
        let mut state = IncomingState::default();
        state.observe("s1", "/p", false, true);
        let generation = state.generation();
        assert!(state.begin(Running::Pulling));
        assert!(!state.begin(Running::Pushing), "one job at a time");
        let next = parse_incoming(
            &json!({"branch": "m", "upstream": "o/m", "ahead": 0, "behind": 0, "commits": []}),
        );
        assert!(state.finish(generation, Some(next.clone())));
        assert_eq!(state.running, None);
        assert_eq!(state.value, next);
        assert!(state.begin(Running::Syncing));
        state.observe("s2", "/p", false, true);
        assert_eq!(state.running, None, "a new conversation starts idle");
        assert!(!state.finish(generation, Some(None)), "late answer");
    }

    #[test]
    fn the_menu_fades_out_before_it_goes() {
        let mut state = IncomingState::default();
        let now = Instant::now();
        assert!(!state.visible(now));
        state.toggle(now, 0.2);
        assert!(state.open && state.visible(now));
        assert!(state.close_at(now, 0.2));
        assert!(!state.open);
        assert!(state.visible(now), "still fading");
        assert!(!state.visible(now + Duration::from_secs(1)));
        assert!(!state.close_at(now, 0.2), "already closed");
    }
}
