//! Two Git functions of the environment card and the header, after Sirus Code: "Criar branch"
//! (EnvironmentPanel's BranchRow, with the name checked here before the host is asked) and the
//! incoming commits behind the header's Puxar button (GitIncomingButton, lib/git-incoming.ts,
//! ADR-148). The host does the Git work; this module reads its answers and keeps the state.
//! shell.rs asks `observe` on each frame, sends the requests, and feeds the answers back, so an
//! answer that changes nothing never asks for a redraw.
use crate::i18n::t;
use crate::motion::{Curve, Tween};
use crate::panel_changes::age;
use crate::theme::{accent, content, on_accent, primary_action};
use gpui::{AnyElement, App, ClickEvent, Div, Hsla, Window, div, prelude::*, px, rgb};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Sirus Code asks again every 5 minutes while the window is visible (lib/git-incoming.ts).
pub const INCOMING_REFRESH: Duration = Duration::from_secs(5 * 60);
/// The green check after a pull stays this long, then the popover closes.
pub const PULLED_FOR: Duration = Duration::from_secs(3);
/// Longest branch name the host takes (its MAX_BRANCH_NAME).
const MAX_BRANCH_NAME: usize = 120;
/// Commits listed in the popover; the rest count as "N more".
const SHOWN_COMMITS: usize = 6;
/// Blocking files listed after a refused pull.
const SHOWN_FILES: usize = 8;

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

// --- Incoming commits -------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
    pub author: String,
    /// Unix seconds.
    pub time: i64,
}

/// What the selected conversation's branch can pull from its upstream (`git_incoming`).
#[derive(Debug, Clone, PartialEq)]
pub struct GitIncoming {
    pub branch: String,
    pub upstream: String,
    pub ahead: u64,
    pub behind: u64,
    pub commits: Vec<Commit>,
}

fn commit_of(value: &Value) -> Option<Commit> {
    let sha = value["sha"].as_str()?;
    let subject = value["subject"].as_str()?;
    let author = value["author"].as_str()?;
    let time = value["time"].as_f64().filter(|time| time.is_finite())?;
    let hex = sha.len() >= 4 && sha.len() <= 64 && sha.chars().all(|c| c.is_ascii_hexdigit());
    hex.then(|| Commit {
        sha: sha.to_owned(),
        subject: subject.chars().take(200).collect(),
        author: author.chars().take(80).collect(),
        time: time as i64,
    })
}

/// Reads `git_incoming`'s answer. `null` and anything malformed mean nothing to show.
pub fn parse_incoming(value: &Value) -> Option<GitIncoming> {
    let branch = value["branch"].as_str().filter(|b| !b.is_empty())?;
    let upstream = value["upstream"].as_str().filter(|u| !u.is_empty())?;
    let ahead = value["ahead"].as_u64()?;
    let behind = value["behind"].as_u64()?;
    let commits = value["commits"].as_array()?;
    Some(GitIncoming {
        branch: branch.to_owned(),
        upstream: upstream.to_owned(),
        ahead,
        behind,
        commits: commits.iter().take(20).filter_map(commit_of).collect(),
    })
}

/// What the header shows for an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    Hidden,
    /// Fast-forward possible: nothing local, something new.
    Pull {
        count: u64,
    },
    /// Both sides have commits: shown muted, no pull.
    Diverged {
        ahead: u64,
        behind: u64,
    },
}

pub fn shown(incoming: Option<&GitIncoming>) -> Shown {
    match incoming {
        Some(g) if g.behind > 0 && g.ahead > 0 => Shown::Diverged {
            ahead: g.ahead,
            behind: g.behind,
        },
        Some(g) if g.behind > 0 => Shown::Pull { count: g.behind },
        _ => Shown::Hidden,
    }
}

/// `git_pull_ff`'s answer, as the popover needs it.
#[derive(Debug, Clone, PartialEq)]
pub enum Pulled {
    Done(Option<GitIncoming>),
    Blocked(Vec<String>),
    Diverged { ahead: u64, behind: u64 },
    Failed(String),
}

pub fn pull_answer(ok: bool, value: &Value) -> Pulled {
    if !ok {
        let message = value["message"]
            .as_str()
            .map_or_else(|| t("O git falhou.").to_owned(), str::to_owned);
        return Pulled::Failed(message);
    }
    match value["status"].as_str() {
        Some("pulled") => Pulled::Done(parse_incoming(&value["incoming"])),
        Some("blocked") => Pulled::Blocked(
            value["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|file| file.as_str().filter(|f| !f.is_empty()).map(str::to_owned))
                .take(50)
                .collect(),
        ),
        Some("diverged") => {
            let (ahead, behind) =
                parse_incoming(&value["incoming"]).map_or((0, 0), |g| (g.ahead, g.behind));
            Pulled::Diverged { ahead, behind }
        }
        _ => Pulled::Failed(t("O git falhou.").to_owned()),
    }
}

/// What the popover shows after a pull; the value itself lives in `IncomingState::value`.
#[derive(Debug, Clone, PartialEq)]
pub enum Feedback {
    Done { branch: String, upstream: String },
    Blocked(Vec<String>),
    Diverged { ahead: u64, behind: u64 },
    Failed(String),
}

/// The Puxar button's state for the selected conversation.
#[derive(Default)]
pub struct IncomingState {
    session: String,
    folder: String,
    /// Bumped when the conversation or its folder changes; older answers are dropped.
    generation: u64,
    working: bool,
    window_active: bool,
    pub value: Option<GitIncoming>,
    pub pulling: bool,
    pub feedback: Option<Feedback>,
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
            self.pulling = false;
            self.feedback = None;
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

    /// A `git_incoming` answer. True when the header or popover must redraw.
    pub fn answer(&mut self, generation: u64, ok: bool, value: &Value) -> bool {
        if generation != self.generation {
            return false;
        }
        let next = if ok { parse_incoming(value) } else { None };
        if next == self.value {
            return false;
        }
        self.value = next;
        if self.value.is_none() {
            self.open = false;
        }
        true
    }

    /// Starts a pull when the header offers one and no conversation is working in the folder.
    pub fn begin_pull(&mut self) -> bool {
        let Some(value) = &self.value else {
            return false;
        };
        if self.pulling || self.working || value.behind == 0 || value.ahead > 0 {
            return false;
        }
        self.pulling = true;
        self.feedback = None;
        true
    }

    /// A `git_pull_ff` answer for `generation`. True when the view changed.
    pub fn pulled(&mut self, generation: u64, answer: Pulled) -> bool {
        if generation != self.generation {
            return false;
        }
        self.pulling = false;
        match answer {
            Pulled::Done(next) => {
                let feedback = self.value.as_ref().map(|g| Feedback::Done {
                    branch: g.branch.clone(),
                    upstream: g.upstream.clone(),
                });
                self.value = next;
                self.feedback = feedback;
            }
            Pulled::Blocked(files) => self.feedback = Some(Feedback::Blocked(files)),
            Pulled::Diverged { ahead, behind } => {
                self.feedback = Some(Feedback::Diverged { ahead, behind })
            }
            Pulled::Failed(message) => self.feedback = Some(Feedback::Failed(message)),
        }
        true
    }

    /// Opens or closes the popover; `duration` is the fade (0 with reduced motion).
    pub fn toggle(&mut self, now: Instant, duration: f32) {
        self.open = !self.open;
        self.tween = Some(if self.open {
            Tween::from_to(0., 1., now, duration, Curve::Expo)
        } else {
            Tween::from_to(1., 0., now, duration, Curve::Expo)
        });
    }

    /// The green check's time is up: the popover closes and the check goes. True when it changed.
    pub fn faded(&mut self, generation: u64) -> bool {
        if generation != self.generation || !matches!(self.feedback, Some(Feedback::Done { .. })) {
            return false;
        }
        self.feedback = None;
        self.open = false;
        self.tween = None;
        true
    }

    /// Closes the popover at once; true when it was open.
    pub fn close(&mut self) -> bool {
        let was = self.open;
        self.open = false;
        self.tween = None;
        was
    }
}

// --- Drawing ----------------------------------------------------------------------------------

/// The header button: "Puxar" with the count, or muted when both sides have commits. None when
/// there is nothing to pull.
pub fn pill(
    shown: Shown,
    pulling: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Option<AnyElement> {
    let (count, muted) = match shown {
        Shown::Hidden => return None,
        Shown::Pull { count } => (Some(count), false),
        Shown::Diverged { .. } => (None, true),
    };
    let glyph = if pulling { "refresh" } else { "chevron-down" };
    let mut button = div()
        .id("git-incoming")
        .h(px(24.))
        .px(px(6.))
        .rounded(px(crate::theme::RADIUS_SM))
        .flex()
        .items_center()
        .gap(px(5.))
        .text_size(px(12.))
        .text_color(content(if muted { 0.35 } else { 0.85 }))
        .child(crate::theme::icon(glyph, 13.).text_color(content(if muted { 0.3 } else { 0.7 })))
        .child(t("Puxar"));
    if let Some(count) = count {
        button = button.child(
            div()
                .h(px(16.))
                .px(px(5.))
                .rounded_full()
                .flex()
                .items_center()
                .bg(accent())
                .text_color(on_accent())
                .text_size(px(10.5))
                .child(count.to_string()),
        );
        button = button
            .cursor_pointer()
            .hover(|s| s.bg(content(0.07)))
            .on_click(on_click);
    }
    Some(button.into_any_element())
}

/// The popover under the header: the incoming commits, what the last pull said, and "Puxar agora".
/// `top` is the y of the title bar's bottom edge and `p` the open fade (0 to 1).
pub fn popover(
    state: &IncomingState,
    top: f32,
    p: f32,
    on_pull: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Option<AnyElement> {
    if !state.open {
        return None;
    }
    let mut body: Vec<AnyElement> = Vec::new();
    match (&state.feedback, &state.value) {
        (Some(Feedback::Done { branch, upstream }), _) => {
            body.push(note_row(
                &tf_pair("Atualizado a partir de {}", upstream),
                false,
            ));
            body.push(small(&format!("{} → {}", branch, upstream), content(0.4)));
        }
        (_, Some(value)) => {
            let title = if value.behind == 1 {
                tf_pair2(
                    "{} commit novo em {}",
                    &value.behind.to_string(),
                    &value.upstream,
                )
            } else {
                tf_pair2(
                    "{} commits novos em {}",
                    &value.behind.to_string(),
                    &value.upstream,
                )
            };
            body.push(
                div()
                    .px(px(8.))
                    .pt(px(6.))
                    .text_size(px(13.))
                    .text_color(content(0.95))
                    .child(title)
                    .into_any_element(),
            );
            body.push(small(&tf_pair("Puxa para {}", &value.branch), content(0.5)));
            for commit in value.commits.iter().take(SHOWN_COMMITS) {
                body.push(commit_row(commit));
            }
            let more = value
                .behind
                .saturating_sub(SHOWN_COMMITS as u64)
                .min(value.commits.len() as u64);
            if more > 0 {
                body.push(small(&tf_pair("{} mais", &more.to_string()), content(0.45)));
            }
            match &state.feedback {
                Some(Feedback::Blocked(files)) => {
                    body.push(small(
                        t("Arquivos que impedem o pull:"),
                        rgb(0xe08a8a).into(),
                    ));
                    for file in files.iter().take(SHOWN_FILES) {
                        body.push(small(file, content(0.6)));
                    }
                }
                Some(Feedback::Diverged { ahead, behind }) => body.push(note_row(
                    &tf_pair2(
                        "Divergiu: {} local e {} do remoto. Faça merge ou rebase à mão.",
                        &ahead.to_string(),
                        &behind.to_string(),
                    ),
                    true,
                )),
                Some(Feedback::Failed(message)) => body.push(note_row(message, true)),
                _ => {}
            }
            if state.working && !state.pulling {
                body.push(small(
                    t("Uma conversa está trabalhando nesta pasta. Puxe quando ela terminar."),
                    content(0.5),
                ));
            }
            let enabled = !state.pulling && !state.working;
            let (fill, ink) = primary_action(enabled);
            let label = if state.pulling {
                t("Puxando…")
            } else {
                t("Puxar agora")
            };
            let mut action = div()
                .id("git-incoming-pull")
                .mx(px(4.))
                .mt(px(4.))
                .h(px(28.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .justify_center()
                .bg(fill)
                .text_color(ink)
                .text_size(px(12.5))
                .child(label);
            if enabled {
                action = action
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.9))
                    .on_click(on_pull);
            }
            body.push(action.into_any_element());
        }
        _ => return None,
    }
    Some(
        div()
            .id("git-incoming-card")
            .occlude()
            .absolute()
            .right(px(10.))
            .top(px(top + 2. + (1. - p) * -6.))
            .w(px(340.))
            .p(px(4.))
            .rounded(px(crate::theme::RADIUS_LG))
            .border_1()
            .border_color(content(0.12))
            .child(crate::theme::floating_surface(crate::theme::RADIUS_MD))
            .shadow_lg()
            .opacity(p)
            .flex()
            .flex_col()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(body)
            .into_any_element(),
    )
}

fn commit_row(commit: &Commit) -> AnyElement {
    let when = chrono::DateTime::from_timestamp(commit.time, 0)
        .map(|at| at.to_rfc3339())
        .unwrap_or_default();
    let short: String = commit.sha.chars().take(7).collect();
    div()
        .px(px(8.))
        .py(px(4.))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(content(0.4))
                .child(short),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_color(content(0.9))
                        .child(commit.subject.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child(format!("{} · {}", commit.author, age(&when))),
                ),
        )
        .into_any_element()
}

fn small(text: &str, color: Hsla) -> AnyElement {
    div()
        .px(px(8.))
        .py(px(2.))
        .text_size(px(11.5))
        .text_color(color)
        .child(text.to_owned())
        .into_any_element()
}

fn note_row(text: &str, failed: bool) -> AnyElement {
    let color: Hsla = if failed {
        rgb(0xe08a8a).into()
    } else {
        content(0.8)
    };
    div()
        .px(px(8.))
        .py(px(4.))
        .text_size(px(12.))
        .text_color(color)
        .child(text.to_owned())
        .into_any_element()
}

/// Fills a translated one-slot template ("Puxa para {}").
fn tf_pair(template: &'static str, value: &str) -> String {
    crate::i18n::tf(template, &[&value])
}

/// Fills a translated two-slot template.
fn tf_pair2(template: &'static str, first: &str, second: &str) -> String {
    crate::i18n::tf(template, &[&first, &second])
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
        .border_color(content(0.12))
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
            "commits": [
                {"sha": "abcdef1", "subject": "Fix", "author": "Ana", "time": 1_700_000_000.0},
                {"sha": "zz", "subject": "bad sha", "author": "x", "time": 1.0},
                {"sha": "1234abcd", "subject": "Second", "author": "Bo", "time": 1_700_000_100.5}
            ],
            "checkedAt": 1
        });
        let parsed = parse_incoming(&value).expect("valid answer");
        assert_eq!(parsed.behind, 2);
        assert_eq!(parsed.commits.len(), 2);
        assert_eq!(parsed.commits[1].time, 1_700_000_100);
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
    }

    #[test]
    fn header_shows_pull_only_when_fast_forward_is_possible() {
        let mut g = GitIncoming {
            branch: "m".into(),
            upstream: "o/m".into(),
            ahead: 0,
            behind: 3,
            commits: vec![],
        };
        assert_eq!(shown(Some(&g)), Shown::Pull { count: 3 });
        g.ahead = 1;
        assert_eq!(
            shown(Some(&g)),
            Shown::Diverged {
                ahead: 1,
                behind: 3
            }
        );
        g.ahead = 0;
        g.behind = 0;
        assert_eq!(shown(Some(&g)), Shown::Hidden);
        assert_eq!(shown(None), Shown::Hidden);
    }

    #[test]
    fn pull_answers_map_to_feedback() {
        assert_eq!(
            pull_answer(false, &json!({"message": "busy"})),
            Pulled::Failed("busy".into())
        );
        assert_eq!(
            pull_answer(true, &json!({"status": "pulled", "incoming": null})),
            Pulled::Done(None)
        );
        assert_eq!(
            pull_answer(
                true,
                &json!({"status": "blocked", "files": ["a.rs", "", 3]})
            ),
            Pulled::Blocked(vec!["a.rs".into()])
        );
        let diverged = json!({"status": "diverged", "incoming": {"branch": "m", "upstream": "o/m", "ahead": 2, "behind": 1, "commits": []}});
        assert_eq!(
            pull_answer(true, &diverged),
            Pulled::Diverged {
                ahead: 2,
                behind: 1
            }
        );
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
            "a failed fetch hides the button"
        );
        assert!(state.value.is_none());
    }

    #[test]
    fn pull_is_refused_while_working_or_with_local_commits() {
        let mut state = IncomingState::default();
        state.observe("s1", "/p", true, true);
        let generation = state.generation();
        let behind =
            json!({"branch": "m", "upstream": "o/m", "ahead": 0, "behind": 1, "commits": []});
        state.answer(generation, true, &behind);
        assert!(
            !state.begin_pull(),
            "a working conversation blocks the pull"
        );
        state.observe("s1", "/p", false, true);
        assert!(state.begin_pull());
        assert!(!state.begin_pull(), "one pull at a time");
        assert!(state.pulled(generation, Pulled::Done(None)));
        assert!(!state.pulling);
        assert_eq!(
            state.feedback,
            Some(Feedback::Done {
                branch: "m".into(),
                upstream: "o/m".into()
            })
        );
        assert!(state.value.is_none());

        let diverged =
            json!({"branch": "m", "upstream": "o/m", "ahead": 1, "behind": 1, "commits": []});
        state.answer(generation, true, &diverged);
        assert!(!state.begin_pull(), "local commits block the fast-forward");
    }

    #[test]
    fn a_pull_answer_for_another_conversation_is_dropped() {
        let mut state = IncomingState::default();
        state.observe("s1", "/p", false, true);
        let old = state.generation();
        state.observe("s2", "/p", false, true);
        state.pulling = true;
        assert!(!state.pulled(old, Pulled::Failed("late".into())));
        assert!(state.feedback.is_none());
    }
}
