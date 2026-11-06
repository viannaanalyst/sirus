//! The first run: a full-window welcome shown once, before anything else. Four steps over the
//! letters robot (new_tab_field.rs), which fills the window, follows the pointer and ripples on a
//! click and on every step change, in the accent's colour:
//!
//! 1. Bem-vindo ao Sirus.
//! 2. Seus agentes: what `detect_agents` finds on this Mac.
//! 3. Abra um projeto: git folders found under the usual places, the "Novo projeto" folder
//!    picker, or later.
//! 4. Deixe do seu jeito: theme and accent, which change the look at once through `set_look`.
//!
//! The end of the last step opens the app on the new tab's own greeting and composer. Whether it
//! was shown is remembered in `native-first-run.json`; an install that already has projects never
//! sees it. Settings → Geral can play it again. Reduced motion keeps the field still and the
//! steps cut instead of sliding.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;
use crate::motion::progress;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How long a step takes to arrive, and to leave.
const ENTER_S: f32 = 0.55;
const LEAVE_S: f32 = 0.22;
/// The overlay fading away at the end.
const CLOSE_S: f32 = 0.5;
/// The scan shows "Procurando…" for at least this long, so the rows do not flash.
const SCAN_MIN_S: f32 = 0.7;
/// Folders offered on the project step, and agents on the agents step.
const RECENT_MAX: usize = 3;
const AGENTS_MAX: usize = 6;
/// One segment of the theme switch.
const SEGMENT_W: f32 = 110.;

/// The places people keep their code, under the home folder. Documents and Desktop are left out:
/// reading them asks the person for permission.
const CODE_FOLDERS: [&str; 10] = [
    "Projetos",
    "Projects",
    "Developer",
    "Code",
    "dev",
    "src",
    "repos",
    "git",
    "GitHub",
    "workspace",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Step {
    Welcome,
    Agents,
    Project,
    Look,
}

impl Step {
    const ALL: [Step; 4] = [Step::Welcome, Step::Agents, Step::Project, Step::Look];

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn next(self) -> Option<Step> {
        Self::ALL.get(self.index() + 1).copied()
    }

    fn back(self) -> Option<Step> {
        self.index().checked_sub(1).map(|ix| Self::ALL[ix])
    }
}

/// Where a step change goes: another step, or out of the first run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Goal {
    Step(Step),
    Leave,
}

/// What to do when the state first arrives.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Start {
    Show,
    /// An install that already has projects: remember it, show nothing.
    MarkSeen,
    Skip,
}

pub(crate) fn decide(seen: bool, projects: usize) -> Start {
    if seen {
        Start::Skip
    } else if projects > 0 {
        Start::MarkSeen
    } else {
        Start::Show
    }
}

fn seen_file() -> PathBuf {
    crate::backend::data_dir().join("native-first-run.json")
}

fn was_seen() -> bool {
    std::fs::read(seen_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|value| value["seen"] == true)
}

fn mark_seen() {
    let _ = std::fs::create_dir_all(crate::backend::data_dir());
    let _ = std::fs::write(seen_file(), json!({ "seen": true }).to_string());
}

/// An agent `detect_agents` reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Agent {
    id: String,
    name: String,
    installed: bool,
    version: Option<String>,
}

/// The version number out of what `--version` printed: "2.1.318 (Claude Code)" gives "2.1.318".
fn short_version(version: &str) -> String {
    version
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .unwrap_or(version.trim())
        .to_owned()
}

/// The agents to list: the ones found first, in the host's order, then the ones not found.
pub(crate) fn agents_of(reply: &Value) -> Vec<Agent> {
    let mut agents: Vec<Agent> = reply
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| {
            let id = agent["id"].as_str()?;
            Some(Agent {
                id: id.to_owned(),
                name: agent["name"].as_str().unwrap_or(id).to_owned(),
                installed: agent["installed"].as_bool() == Some(true),
                version: agent["version"]
                    .as_str()
                    .map(short_version)
                    .filter(|v| !v.is_empty()),
            })
        })
        .collect();
    // The ones most people came for lead; the host's order is alphabetical at best.
    let lead = |id: &str| ["claude", "codex", "opencode"].iter().position(|known| *known == id);
    agents.sort_by_key(|agent| (!agent.installed, lead(&agent.id).unwrap_or(usize::MAX)));
    agents.truncate(AGENTS_MAX);
    agents
}

/// A folder to offer: a git project under the usual places.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Recent {
    name: String,
    path: String,
    /// The path with the home folder as "~".
    shown: String,
    /// Seconds since the repository last changed.
    ago: u64,
}

/// "~/Projetos/site" for "/Users/me/Projetos/site".
fn tilde(path: &str, home: &str) -> String {
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => path.to_owned(),
    }
}

/// The git folders directly under `roots`, the most recently changed first. `known` are projects
/// the app already has.
fn recent_projects(
    roots: &[PathBuf],
    home: &str,
    known: &[String],
    limit: usize,
    now: SystemTime,
) -> Vec<Recent> {
    let mut found: Vec<(SystemTime, Recent)> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let git = path.join(".git");
            if name.starts_with('.') || !git.exists() || !path.is_dir() {
                continue;
            }
            let path_text = path.display().to_string();
            if known.iter().any(|k| *k == path_text) {
                continue;
            }
            let changed = std::fs::metadata(&git)
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            found.push((
                changed,
                Recent {
                    name,
                    shown: tilde(&path_text, home),
                    path: path_text,
                    ago: now.duration_since(changed).map_or(0, |d| d.as_secs()),
                },
            ));
        }
    }
    found.sort_by_key(|(changed, _)| std::cmp::Reverse(*changed));
    found
        .into_iter()
        .map(|(_, recent)| recent)
        .take(limit)
        .collect()
}

/// How long ago, as the row says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Age {
    Minutes(u64),
    Hours(u64),
    Yesterday,
    Days(u64),
}

fn age_of(seconds: u64) -> Age {
    match seconds {
        0..=3599 => Age::Minutes((seconds / 60).max(1)),
        3600..=86_399 => Age::Hours(seconds / 3600),
        86_400..=172_799 => Age::Yesterday,
        _ => Age::Days(seconds / 86_400),
    }
}

impl Age {
    fn text(self) -> String {
        match self {
            Age::Minutes(n) => tf("há {} min", &[&n]),
            Age::Hours(n) => tf("há {} h", &[&n]),
            Age::Yesterday => t("ontem").to_owned(),
            Age::Days(n) => tf("há {} dias", &[&n]),
        }
    }
}

enum Scan {
    Searching(Instant),
    Found(Vec<Agent>, Instant),
}

/// One walk through the first run.
struct Run {
    step: Step,
    /// When the step on screen began.
    since: Instant,
    /// Going to another step (or out): where, and since when.
    leaving: Option<(Goal, Instant)>,
    scan: Option<Scan>,
    recents: Vec<Recent>,
    /// A folder was chosen: the number of projects before it, and the folder, while it is added.
    adding: Option<(usize, Option<String>)>,
    /// Where the theme switch's thumb is, in segments.
    thumb: Tween,
    /// The overlay fading away.
    closing: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct State {
    run: Option<Run>,
    checked: bool,
    /// The robot field's pointer and waves.
    fx: crate::new_tab_field::State,
}

impl State {
    pub(crate) fn active(&self) -> bool {
        self.run.is_some()
    }
}

/// The theme switch's segments, as `Look::mode` values: system, light, dark.
const MODES: [usize; 3] = [0, 2, 1];

fn segment_of(mode: usize) -> usize {
    MODES.iter().position(|m| *m == mode).unwrap_or(0)
}

fn hex_of(color: gpui::Hsla) -> String {
    let rgba = color.to_rgb();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    format!(
        "#{:06x}",
        byte(rgba.r) << 16 | byte(rgba.g) << 8 | byte(rgba.b)
    )
}

impl Shell {
    /// Once, when the state first arrives: show the first run to someone who has never seen it.
    pub(super) fn first_run_check(&mut self, cx: &mut Context<Self>) {
        if self.first_run.checked {
            return;
        }
        self.first_run.checked = true;
        match decide(was_seen(), self.projects.len()) {
            Start::Show => {
                mark_seen();
                self.first_run_start(cx);
            }
            Start::MarkSeen => mark_seen(),
            Start::Skip => {}
        }
    }

    /// Plays the first run (also from Settings → Geral).
    pub(super) fn first_run_start(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let home = std::env::var("HOME").unwrap_or_default();
        let roots: Vec<PathBuf> = CODE_FOLDERS
            .iter()
            .map(|folder| Path::new(&home).join(folder))
            .collect();
        let known: Vec<String> = self.projects.iter().map(|p| p.path.clone()).collect();
        self.appearance = None;
        self.palette_ui = None;
        self.first_run.fx = Default::default();
        self.first_run.run = Some(Run {
            step: Step::Welcome,
            since: now,
            leaving: None,
            scan: None,
            recents: recent_projects(&roots, &home, &known, RECENT_MAX, SystemTime::now()),
            adding: None,
            thumb: Tween::still(segment_of(self.look.mode) as f32, now),
            closing: None,
        });
        self.first_run_pulse(cx);
        cx.notify();
    }

    /// A wave from the middle of the window.
    fn first_run_pulse(&mut self, cx: &mut Context<Self>) {
        if cx.reduce_motion() {
            return;
        }
        let middle = gpui::point(
            px(self.viewport_width / 2.),
            px(self.viewport_height * 0.46),
        );
        self.first_run.fx.pulse(middle, Instant::now());
    }

    fn first_run_go(&mut self, goal: Goal, cx: &mut Context<Self>) {
        let now = Instant::now();
        let Some(run) = self.first_run.run.as_mut() else {
            return;
        };
        if run.leaving.is_some() || run.closing.is_some() {
            return;
        }
        run.leaving = Some((goal, now));
        self.first_run_pulse(cx);
        cx.notify();
    }

    fn first_run_next(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.first_run.run.as_ref() else {
            return;
        };
        let ready = match run.step {
            Step::Agents => matches!(&run.scan, Some(Scan::Found(_, at)) if *at <= Instant::now()),
            Step::Project => false,
            _ => true,
        };
        if !ready {
            return;
        }
        let goal = run.step.next().map_or(Goal::Leave, Goal::Step);
        self.first_run_go(goal, cx);
    }

    fn first_run_back(&mut self, cx: &mut Context<Self>) {
        if let Some(back) = self.first_run.run.as_ref().and_then(|r| r.step.back()) {
            self.first_run_go(Goal::Step(back), cx);
        }
    }

    /// The keys while it is up: Enter and → go on, ← goes back, Esc skips. ⌘ shortcuts pass.
    pub(super) fn first_run_keys(
        &mut self,
        keys: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.first_run.run.is_none() {
            return false;
        }
        if keys.modifiers.platform {
            return false;
        }
        match keys.key.as_str() {
            "enter" | "right" => self.first_run_next(cx),
            "left" => self.first_run_back(cx),
            "escape" => self.first_run_go(Goal::Leave, cx),
            _ => {}
        }
        true
    }

    /// The host listed the agents.
    pub(super) fn first_run_agents_arrived(
        &mut self,
        ok: bool,
        reply: &Value,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let agents = if ok { agents_of(reply) } else { Vec::new() };
        if let Some(run) = self.first_run.run.as_mut() {
            let started = match run.scan {
                Some(Scan::Searching(at)) => at,
                _ => now,
            };
            // Hold the answer back until "Procurando…" has been up for a moment.
            let shown = started + Duration::from_secs_f32(SCAN_MIN_S);
            run.scan = Some(Scan::Found(agents, shown.max(now)));
        }
        cx.notify();
    }

    /// Adds a folder as a project, as "Novo projeto" does; the step moves on when it arrives.
    fn first_run_add(&mut self, path: String, cx: &mut Context<Self>) {
        let baseline = self.projects.len();
        if let Some(run) = self.first_run.run.as_mut() {
            run.adding = Some((baseline, Some(path.clone())));
        }
        let call = self.backend.invoke("add_project", json!({ "path": path }));
        self.waiting.insert(call, Waiting::ProjectAdded);
        cx.notify();
    }

    /// The folder picker the sidebar's "Novo projeto" uses.
    fn first_run_pick(&mut self, cx: &mut Context<Self>) {
        let baseline = self.projects.len();
        if let Some(run) = self.first_run.run.as_mut() {
            run.adding = Some((baseline, None));
        }
        self.add_new_project(cx);
    }

    fn first_run_set_mode(&mut self, segment: usize, cx: &mut Context<Self>) {
        self.set_look(|look| look.mode = MODES[segment], cx);
        if let Some(run) = self.first_run.run.as_mut() {
            run.thumb
                .set(segment as f32, Instant::now(), 0.32, Curve::Expo);
        }
        self.first_run_pulse(cx);
    }

    fn first_run_set_accent(&mut self, at: usize, cx: &mut Context<Self>) {
        self.set_look(
            |look| {
                look.accent = Some(at);
                look.custom = None;
            },
            cx,
        );
        self.first_run_pulse(cx);
    }

    /// Moves the run along: a step that has left, a project that arrived, the end.
    fn first_run_advance(&mut self, now: Instant, cx: &mut Context<Self>) {
        let projects = self.projects.len();
        let reduce = cx.reduce_motion();
        let segment = segment_of(self.look.mode) as f32;
        let Some(run) = self.first_run.run.as_mut() else {
            return;
        };
        if run
            .closing
            .is_some_and(|at| progress(at, now, 0., CLOSE_S, Curve::Quad) >= 1.)
        {
            self.first_run.run = None;
            cx.notify();
            return;
        }
        if run
            .adding
            .as_ref()
            .is_some_and(|(before, _)| projects > *before)
        {
            run.adding = None;
            if run.step == Step::Project && run.leaving.is_none() {
                run.leaving = Some((Goal::Step(Step::Look), now));
            }
        }
        let Some((goal, at)) = run.leaving else {
            return;
        };
        if !reduce && progress(at, now, 0., LEAVE_S, Curve::Quad) < 1. {
            return;
        }
        run.leaving = None;
        match goal {
            Goal::Leave => run.closing = Some(now),
            Goal::Step(step) => {
                run.step = step;
                run.since = now;
                match step {
                    Step::Agents if !matches!(run.scan, Some(Scan::Found(..))) => {
                        run.scan = Some(Scan::Searching(now));
                        let call = self.backend.invoke("detect_agents", json!({}));
                        self.waiting.insert(call, Waiting::FirstRunAgents);
                    }
                    Step::Look => run.thumb.snap(segment, now),
                    _ => {}
                }
            }
        }
        if matches!(goal, Goal::Leave) {
            self.first_run_pulse(cx);
        }
        cx.notify();
    }

    /// The first run over the whole window, or nothing when it is not up.
    pub(super) fn first_run_view(
        &mut self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.first_run.run.as_ref()?;
        self.first_run_advance(now, cx);
        let run = self.first_run.run.as_ref()?;
        let reduce = cx.reduce_motion();
        let duration = |seconds: f32| if reduce { 0. } else { seconds };
        let arrived = progress(run.since, now, 0., duration(ENTER_S), Curve::Expo);
        let leaving = run.leaving.map_or(0., |(_, at)| {
            progress(at, now, 0., duration(LEAVE_S), Curve::Quad)
        });
        let fade = run.closing.map_or(1., |at| {
            1. - progress(at, now, 0., duration(CLOSE_S), Curve::Quad)
        });
        let age = now.saturating_duration_since(run.since).as_secs_f32();
        let searching = matches!(run.scan, Some(Scan::Searching(_)))
            || matches!(run.scan, Some(Scan::Found(_, at)) if at > now);
        *moving |= !reduce
            && (age < 1.3
                || run.leaving.is_some()
                || run.closing.is_some()
                || (run.step == Step::Welcome)
                || !run.thumb.done(now))
            || searching
            || self.first_run.fx.moving(now);
        let step = run.step;
        let accent_hsla = accent();
        let robot = {
            let rgb = accent_hsla.to_rgb();
            let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
            byte(rgb.r) << 16 | byte(rgb.g) << 8 | byte(rgb.b)
        };
        let body = match step {
            Step::Welcome => self.first_run_welcome(now, cx),
            Step::Agents => self.first_run_agents(now, searching, cx),
            Step::Project => self.first_run_project(now, cx),
            Step::Look => self.first_run_look(now, cx),
        };
        let shift = (1. - arrived) * 16. - leaving * 10.;
        let plate = |inset_x: f32, inset_y: f32, radius: f32, alpha: f32| {
            let mut fill = pane_opaque();
            fill.a = alpha;
            div()
                .absolute()
                .top(px(-inset_y))
                .bottom(px(-inset_y))
                .left(px(-inset_x))
                .right(px(-inset_x))
                .rounded(px(radius))
                .bg(fill)
        };
        let dots = Step::ALL.iter().map(|s| {
            let on = *s == step;
            div()
                .h(px(6.))
                .w(px(if on { 6. + 16. * arrived } else { 6. }))
                .rounded(px(3.))
                .bg(if on { accent() } else { content(0.2) })
        });
        let mut backdrop = pane_opaque();
        backdrop.a = fade;
        Some(
            div()
                .id("first-run")
                .occlude()
                .absolute()
                .inset_0()
                .bg(backdrop)
                .opacity(fade.max(0.001))
                .child(crate::new_tab_field::field_with(
                    "tv",
                    robot,
                    &self.first_run.fx,
                    |this: &mut Shell| &mut this.first_run.fx,
                    cx.entity().downgrade(),
                    reduce,
                    true,
                ))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px(px(40.))
                        .child(
                            div()
                                .relative()
                                .top(px(shift))
                                .opacity((arrived * (1. - leaving)).max(0.001))
                                .w(px(560.))
                                .max_w_full()
                                .flex()
                                .flex_col()
                                .items_center()
                                .text_center()
                                .gap(px(12.))
                                // A soft darkening behind the text keeps it readable over the letters.
                                .child(plate(120., 80., 140., 0.2))
                                .child(plate(80., 50., 110., 0.3))
                                .child(plate(40., 24., 80., 0.4))
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .gap(px(12.))
                                        .children(body),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("first-run-skip")
                        .absolute()
                        .top(px(14.))
                        .right(px(22.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(7.))
                        .text_size(px(13.))
                        .text_color(content(0.5))
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.07)).text_color(content(0.95)))
                        .on_click(cx.listener(|this, _, _, cx| this.first_run_go(Goal::Leave, cx)))
                        .child(t("Pular")),
                )
                .child(
                    div()
                        .absolute()
                        .bottom(px(26.))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .gap(px(8.))
                        .children(dots),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(22.))
                        .bottom(px(24.))
                        .text_size(px(12.))
                        .text_color(content(0.35))
                        .child(t("Mexa o mouse e clique no fundo")),
                )
                .into_any_element(),
        )
    }

    fn first_run_title(text: &'static str, size: f32) -> AnyElement {
        div()
            .text_size(px(size))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(content(0.95))
            .child(t(text))
            .into_any_element()
    }

    fn first_run_text(text: String) -> AnyElement {
        div()
            .max_w(px(440.))
            .text_size(px(15.))
            .text_color(content(0.62))
            .child(text)
            .into_any_element()
    }

    /// A button: the accent one is the way on, the plain one is an option.
    fn first_run_button(
        id: &'static str,
        label: String,
        primary: bool,
        enabled: bool,
    ) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .h(px(40.))
            .px(px(20.))
            .rounded(px(12.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(14.))
            .font_weight(if primary {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .when(primary, |d| d.bg(accent()).text_color(on_accent()))
            .when(!primary, |d| {
                d.border_1()
                    .border_color(content(0.12))
                    .text_color(content(0.9))
                    .hover(|s| s.bg(content(0.07)))
            })
            .when(!enabled, |d| d.opacity(0.45))
            .when(enabled, |d| d.cursor_pointer())
            .child(label)
    }

    fn first_run_ghost(id: &'static str, label: String) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .px(px(10.))
            .py(px(8.))
            .text_size(px(13.5))
            .text_color(content(0.55))
            .cursor_pointer()
            .hover(|s| s.text_color(content(0.95)))
            .child(label)
    }

    fn first_run_nav(children: Vec<AnyElement>) -> AnyElement {
        div()
            .mt(px(16.))
            .flex()
            .items_center()
            .gap(px(8.))
            .children(children)
            .into_any_element()
    }

    /// The glass card the rows sit in.
    fn first_run_card(rows: Vec<AnyElement>) -> AnyElement {
        div()
            .w_full()
            .mt(px(10.))
            .rounded(px(16.))
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.045))
            .overflow_hidden()
            .text_left()
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element()
    }

    /// A row of a card, rising in after `delay` seconds.
    fn first_run_row(ix: usize, now: Instant, since: Instant, reduce: bool) -> Div {
        let rise = progress(
            since,
            now,
            0.09 * ix as f32,
            if reduce { 0. } else { 0.45 },
            Curve::Expo,
        );
        div()
            .relative()
            .top(px((1. - rise) * 8.))
            .opacity(rise)
            .px(px(16.))
            .py(px(12.))
            .flex()
            .items_center()
            .gap(px(12.))
            .when(ix > 0, |d| d.border_t_1().border_color(content(0.1)))
    }

    fn first_run_welcome(&mut self, now: Instant, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let since = self.first_run.run.as_ref().map_or(now, |r| r.since);
        let clock = if cx.reduce_motion() {
            0.
        } else {
            now.saturating_duration_since(since).as_secs_f32()
        };
        let appearance = json!({ "color": hex_of(accent()), "accessory": "tv", "eyes": "bars" });
        vec![
            crate::project_avatar::project_avatar_state(
                "Sirus",
                &appearance,
                92.,
                crate::project_avatar::AvatarState::Idle,
                clock,
            ),
            div().mt(px(8.)).child(Self::first_run_title("Bem-vindo ao Sirus", 34.)).into_any_element(),
            Self::first_run_text(
                t("Seus agentes de código lado a lado, com as conversas e as mudanças num lugar só.").to_owned(),
            ),
            Self::first_run_nav(vec![
                Self::first_run_button("first-run-start", t("Começar").to_owned(), true, true)
                    .on_click(cx.listener(|this, _, _, cx| this.first_run_next(cx)))
                    .into_any_element(),
            ]),
        ]
    }

    fn first_run_agents(
        &mut self,
        now: Instant,
        searching: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let reduce = cx.reduce_motion();
        let Some(run) = self.first_run.run.as_ref() else {
            return Vec::new();
        };
        let agents: &[Agent] = match &run.scan {
            Some(Scan::Found(agents, _)) if !searching => agents,
            _ => &[],
        };
        let found = agents.iter().filter(|a| a.installed).count();
        let lead = if searching {
            t("Procurando agentes instalados no seu Mac…")
        } else if found > 0 {
            t("Encontramos estes no seu Mac. Um já basta para começar.")
        } else {
            t(
                "Nenhum agente encontrado ainda. Instale um e ele aparece em Configurações → Provedores.",
            )
        };
        let clock = (now.saturating_duration_since(run.since).as_secs_f32()
            % crate::marks::SPINNER_PERIOD)
            / crate::marks::SPINNER_PERIOD;
        let rows: Vec<AnyElement> = if searching {
            ["claude", "codex", "opencode", "cursor"]
                .iter()
                .enumerate()
                .map(|(ix, id)| {
                    Self::first_run_row(ix, now, run.since, reduce)
                        .child(Self::first_run_mark(id))
                        .child(
                            div().flex_1().child(
                                div()
                                    .text_size(px(14.))
                                    .text_color(content(0.4))
                                    .child(t("Verificando…")),
                            ),
                        )
                        .child(crate::marks::spinner_at(clock))
                        .into_any_element()
                })
                .collect()
        } else {
            agents
                .iter()
                .enumerate()
                .map(|(ix, agent)| {
                    let sub = match (&agent.version, agent.installed) {
                        (Some(version), true) => tf("Versão {}", &[version]),
                        (None, true) => t("Instalado").to_owned(),
                        _ => t("Não encontrado").to_owned(),
                    };
                    Self::first_run_row(ix, now, run.since, reduce)
                        .child(Self::first_run_mark(&agent.id))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .text_color(content(if agent.installed {
                                            0.92
                                        } else {
                                            0.55
                                        }))
                                        .child(agent.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .text_color(content(0.5))
                                        .child(sub),
                                ),
                        )
                        .when(agent.installed, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .text_size(px(13.))
                                    .text_color(gpui::rgb(0x5fcf80))
                                    .child(theme_icon("check", 13.).text_color(gpui::rgb(0x5fcf80)))
                                    .child(t("Pronto")),
                            )
                        })
                        .into_any_element()
                })
                .collect()
        };
        let ready = !searching && matches!(run.scan, Some(Scan::Found(..)));
        vec![
            Self::first_run_title("Seus agentes", 26.),
            Self::first_run_text(lead.to_owned()),
            Self::first_run_card(rows),
            Self::first_run_nav(vec![
                Self::first_run_ghost("first-run-back", t("Voltar").to_owned())
                    .on_click(cx.listener(|this, _, _, cx| this.first_run_back(cx)))
                    .into_any_element(),
                Self::first_run_button(
                    "first-run-continue",
                    t("Continuar").to_owned(),
                    true,
                    ready,
                )
                .when(ready, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.first_run_next(cx)))
                })
                .into_any_element(),
            ]),
        ]
    }

    /// A provider's mark on its brand tile, as the tabs draw it.
    fn first_run_mark(id: &str) -> AnyElement {
        mark_tile(
            &crate::tab_tile::TileSize {
                w: 32.,
                h: 28.,
                radius: 8.,
            },
            Face::provider(id),
            provider_glyph(id, 17.),
        )
        .into_any_element()
    }

    fn first_run_project(&mut self, now: Instant, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let reduce = cx.reduce_motion();
        let Some(run) = self.first_run.run.as_ref() else {
            return Vec::new();
        };
        let adding = run.adding.as_ref().and_then(|(_, path)| path.clone());
        let clock = (now.saturating_duration_since(run.since).as_secs_f32()
            % crate::marks::SPINNER_PERIOD)
            / crate::marks::SPINNER_PERIOD;
        let rows: Vec<AnyElement> = run
            .recents
            .iter()
            .enumerate()
            .map(|(ix, recent)| {
                let path = recent.path.clone();
                let busy = adding.as_deref() == Some(recent.path.as_str());
                Self::first_run_row(ix, now, run.since, reduce)
                    .id(("first-run-recent", ix))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.05)))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.first_run_add(path.clone(), cx)),
                    )
                    .child(
                        div()
                            .w(px(32.))
                            .h(px(28.))
                            .rounded(px(8.))
                            .bg(content(0.12))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(theme_icon("folder", 16.).text_color(content(0.85))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(14.))
                                    .text_color(content(0.92))
                                    .child(recent.name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .text_color(content(0.5))
                                    .child(recent.shown.clone()),
                            ),
                    )
                    .child(if busy {
                        crate::marks::spinner_at(clock).into_any_element()
                    } else {
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.5))
                            .text_color(content(0.5))
                            .child(age_of(recent.ago).text())
                            .into_any_element()
                    })
                    .into_any_element()
            })
            .collect();
        let has_recents = !rows.is_empty();
        let mut children = vec![
            Self::first_run_title("Abra um projeto", 26.),
            Self::first_run_text(
                t("O agente trabalha dentro da pasta que você escolher.").to_owned(),
            ),
        ];
        if has_recents {
            children.push(Self::first_run_card(rows));
        }
        children.push(Self::first_run_nav(vec![
            Self::first_run_ghost("first-run-back", t("Voltar").to_owned())
                .on_click(cx.listener(|this, _, _, cx| this.first_run_back(cx)))
                .into_any_element(),
            Self::first_run_button(
                "first-run-pick",
                if has_recents {
                    t("Escolher outra pasta…")
                } else {
                    t("Escolher uma pasta…")
                }
                .to_owned(),
                !has_recents,
                true,
            )
            .on_click(cx.listener(|this, _, _, cx| this.first_run_pick(cx)))
            .into_any_element(),
            Self::first_run_ghost("first-run-later", t("Depois").to_owned())
                .on_click(
                    cx.listener(|this, _, _, cx| this.first_run_go(Goal::Step(Step::Look), cx)),
                )
                .into_any_element(),
        ]));
        children
    }

    fn first_run_look(&mut self, now: Instant, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let thumb = self.first_run.run.as_ref().map_or(0., |r| r.thumb.at(now));
        let chosen = segment_of(self.look.mode);
        let accent_at = if self.look.custom.is_some() {
            None
        } else {
            self.look.accent
        };
        let segments: Vec<AnyElement> = [t("Sistema"), t("Claro"), t("Escuro")]
            .into_iter()
            .enumerate()
            .map(|(ix, label)| {
                div()
                    .id(("first-run-mode", ix))
                    .relative()
                    .w(px(SEGMENT_W))
                    .py(px(8.))
                    .text_size(px(14.))
                    .text_center()
                    .cursor_pointer()
                    .text_color(content(if ix == chosen { 0.95 } else { 0.65 }))
                    .on_click(cx.listener(move |this, _, _, cx| this.first_run_set_mode(ix, cx)))
                    .child(label)
                    .into_any_element()
            })
            .collect();
        let swatches: Vec<AnyElement> = ACCENTS
            .iter()
            .enumerate()
            .map(|(ix, (_, color))| {
                let on = accent_at == Some(ix);
                div()
                    .id(("first-run-accent", ix))
                    .size(px(32.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_2()
                    .border_color(if on { content(0.9) } else { transparent() })
                    .hover(|s| s.size(px(34.)))
                    .on_click(cx.listener(move |this, _, _, cx| this.first_run_set_accent(ix, cx)))
                    .child(div().size(px(24.)).rounded_full().bg(rgb(*color)))
                    .into_any_element()
            })
            .collect();
        vec![
            Self::first_run_title("Deixe do seu jeito", 26.),
            Self::first_run_text(
                t("O fundo já muda enquanto você escolhe. Dá para trocar depois nas Configurações.").to_owned(),
            ),
            div()
                .mt(px(10.))
                .relative()
                .p(px(3.))
                .rounded(px(12.))
                .bg(content(0.07))
                .flex()
                .child(
                    div()
                        .absolute()
                        .top(px(3.))
                        .bottom(px(3.))
                        .left(px(3. + thumb * SEGMENT_W))
                        .w(px(SEGMENT_W))
                        .rounded(px(9.))
                        .bg(content(0.13)),
                )
                .children(segments)
                .into_any_element(),
            div().mt(px(14.)).flex().gap(px(6.)).children(swatches).into_any_element(),
            Self::first_run_nav(vec![
                Self::first_run_ghost("first-run-back", t("Voltar").to_owned())
                    .on_click(cx.listener(|this, _, _, cx| this.first_run_back(cx)))
                    .into_any_element(),
                Self::first_run_button("first-run-done", t("Tudo pronto").to_owned(), true, true)
                    .on_click(cx.listener(|this, _, _, cx| this.first_run_next(cx)))
                    .into_any_element(),
            ]),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_fresh_install_without_projects_sees_it() {
        assert_eq!(decide(false, 0), Start::Show);
        assert_eq!(decide(false, 3), Start::MarkSeen);
        assert_eq!(decide(true, 0), Start::Skip);
        assert_eq!(decide(true, 3), Start::Skip);
    }

    #[test]
    fn steps_walk_forward_and_back_and_stop_at_the_ends() {
        assert_eq!(Step::Welcome.back(), None);
        assert_eq!(Step::Welcome.next(), Some(Step::Agents));
        assert_eq!(Step::Project.back(), Some(Step::Agents));
        assert_eq!(Step::Look.next(), None);
        assert_eq!(
            Step::ALL.iter().map(|s| s.index()).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn versions_are_the_number_in_the_cli_output() {
        assert_eq!(short_version("2.1.318 (Claude Code)"), "2.1.318");
        assert_eq!(short_version("codex-cli 0.166.0"), "0.166.0");
        assert_eq!(short_version("dev"), "dev");
    }

    #[test]
    fn agents_found_come_first_and_the_list_is_bounded() {
        let reply = json!([
            { "id": "cursor", "name": "Cursor", "installed": false },
            { "id": "claude", "name": "Claude Code", "installed": true, "version": "2.1.318 (Claude Code)" },
            { "id": "codex", "installed": true },
            { "id": "a", "installed": false }, { "id": "b", "installed": false },
            { "id": "c", "installed": false }, { "id": "d", "installed": false },
            { "name": "no id" },
        ]);
        let agents = agents_of(&reply);
        assert_eq!(agents.len(), AGENTS_MAX);
        assert_eq!(agents[0].id, "claude");
        assert_eq!(agents[0].version.as_deref(), Some("2.1.318"));
        assert_eq!(
            (agents[1].id.as_str(), agents[1].name.as_str()),
            ("codex", "codex")
        );
        assert!(agents[1].version.is_none());
        assert!(!agents[2].installed);
        assert!(agents_of(&json!({})).is_empty());
    }

    #[test]
    fn paths_show_the_home_folder_as_a_tilde() {
        assert_eq!(
            tilde("/Users/me/Projetos/site", "/Users/me"),
            "~/Projetos/site"
        );
        assert_eq!(tilde("/Users/me", "/Users/me"), "~");
        assert_eq!(tilde("/Users/meow/x", "/Users/me"), "/Users/meow/x");
        assert_eq!(tilde("/opt/x", ""), "/opt/x");
    }

    #[test]
    fn ages_pick_the_coarsest_unit_that_fits() {
        assert_eq!(age_of(5), Age::Minutes(1));
        assert_eq!(age_of(600), Age::Minutes(10));
        assert_eq!(age_of(7200), Age::Hours(2));
        assert_eq!(age_of(100_000), Age::Yesterday);
        assert_eq!(age_of(86_400 * 3 + 10), Age::Days(3));
    }

    #[test]
    fn the_theme_switch_maps_to_the_looks_modes() {
        // Sistema, Claro, Escuro are modes 0, 2, 1.
        assert_eq!(MODES.map(segment_of), [0, 1, 2]);
        assert_eq!(segment_of(9), 0);
    }

    #[test]
    fn git_folders_are_listed_newest_first_without_known_projects() {
        let base = std::env::temp_dir().join(format!("sirus-first-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for name in ["old", "new", "owned", ".hidden"] {
            std::fs::create_dir_all(base.join(name).join(".git")).unwrap();
        }
        std::fs::create_dir_all(base.join("plain")).unwrap();
        std::fs::write(base.join("file.txt"), "x").unwrap();
        let old = base.join("old/.git");
        let file = std::fs::File::open(&old).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(86_400 * 5))
            .unwrap();
        let owned = base.join("owned").display().to_string();
        let home = base.display().to_string();
        let found = recent_projects(
            &[base.clone(), base.join("missing")],
            &home,
            &[owned],
            5,
            SystemTime::now(),
        );
        let names: Vec<&str> = found.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["new", "old"]);
        assert_eq!(found[0].shown, "~/new");
        assert!(found[1].ago >= 86_400 * 5);
        assert_eq!(
            recent_projects(&[base.clone()], &home, &[], 1, SystemTime::now()).len(),
            1
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
