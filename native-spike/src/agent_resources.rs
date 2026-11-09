//! What the running agents cost the Mac (ADR-144): AgentResources.tsx. A gauge in the footer shows
//! while a conversation runs, turns amber or red when the agents use a lot of CPU or memory (or the
//! Mac is under memory pressure), and opens a panel to stop a conversation or one of its processes.
//! Both stops confirm in the shell's popup first. The thresholds, the sort and the labels are those
//! of lib/agent-resources.ts.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;
use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

/// While the panel is open it reads every 2 s; a change of the running set reads once it settles.
const OPEN_INTERVAL: Duration = Duration::from_secs(2);
const SETTLE: Duration = Duration::from_millis(1500);
/// Amber and red, as the React panel tints its numbers.
const AMBER: u32 = 0xe0b25a;
const RED: u32 = 0xe08a8a;

/// The total's and a row's thresholds (lib/agent-resources.ts): amber first, then red.
const TOTAL_CPU_SHARE: (f64, f64) = (0.4, 0.75);
const TOTAL_MEMORY_SHARE: (f64, f64) = (0.25, 0.5);
const ROW_CPU_PERCENT: (f64, f64) = (90., 200.);
const ROW_MEMORY_SHARE: (f64, f64) = (0.1, 0.25);

/// How hot a reading is; the order is the order of severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Level {
    Normal,
    Elevated,
    High,
}

impl Level {
    /// The colour of the figures at this level, or none for the normal one.
    fn color(self) -> Option<gpui::Hsla> {
        match self {
            Level::Normal => None,
            Level::Elevated => Some(gpui::rgb(AMBER).into()),
            Level::High => Some(gpui::rgb(RED).into()),
        }
    }
}

fn step(value: f64, (elevated, high): (f64, f64)) -> Level {
    if value >= high {
        Level::High
    } else if value >= elevated {
        Level::Elevated
    } else {
        Level::Normal
    }
}

/// The Mac's side of a reading: its cores, its memory and how the memory is doing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Machine {
    pub cores: f64,
    pub memory_total: Option<f64>,
    pub pressure: Option<String>,
}

/// The whole set of agents against the Mac; the footer's gauge reads this.
pub(crate) fn total_level(cpu_percent: f64, memory: f64, machine: &Machine) -> Level {
    let cpu_share = cpu_percent / (machine.cores.max(1.) * 100.);
    let memory_share = machine.memory_total.map_or(0., |total| memory / total);
    let pressure = match machine.pressure.as_deref() {
        Some("critical") => Level::High,
        Some("warning") => Level::Elevated,
        _ => Level::Normal,
    };
    step(cpu_share, TOTAL_CPU_SHARE)
        .max(step(memory_share, TOTAL_MEMORY_SHARE))
        .max(pressure)
}

/// One session or one process.
pub(crate) fn row_level(cpu_percent: f64, memory: f64, machine: &Machine) -> Level {
    let memory_share = machine.memory_total.map_or(0., |total| memory / total);
    step(cpu_percent, ROW_CPU_PERCENT).max(step(memory_share, ROW_MEMORY_SHARE))
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SessionUse {
    pub session_id: String,
    pub cpu: f64,
    pub memory: f64,
}

/// One reading of the agents' process trees and the Mac (`agent_resources`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Snapshot {
    pub sessions: Vec<SessionUse>,
    pub cpu: f64,
    pub memory: f64,
    pub machine: Machine,
}

impl Snapshot {
    pub(crate) fn of(value: &Value) -> Option<Self> {
        let sessions = value["sessions"]
            .as_array()?
            .iter()
            .map(|entry| SessionUse {
                session_id: entry["sessionId"].as_str().unwrap_or("").to_owned(),
                cpu: entry["cpuPercent"].as_f64().unwrap_or(0.),
                memory: entry["memoryBytes"].as_f64().unwrap_or(0.),
            })
            .collect();
        Some(Self {
            sessions,
            cpu: value["cpuPercent"].as_f64().unwrap_or(0.),
            memory: value["memoryBytes"].as_f64().unwrap_or(0.),
            machine: Machine {
                cores: value["cores"].as_f64().unwrap_or(1.),
                memory_total: value["memoryTotal"].as_f64(),
                pressure: value["memoryPressure"].as_str().map(str::to_owned),
            },
        })
    }

    /// The level of the whole reading.
    pub(crate) fn level(&self) -> Level {
        total_level(self.cpu, self.memory, &self.machine)
    }
}

/// A conversation's title and provider, for the rows.
pub(crate) struct Meta {
    pub session_id: String,
    pub title: String,
    pub provider: &'static str,
}

/// A reading joined with the conversations' titles, busiest first.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub session_id: String,
    pub title: String,
    pub provider: Option<&'static str>,
    pub cpu: f64,
    pub memory: f64,
}

pub(crate) fn rows(snapshot: &Snapshot, sessions: &[Meta]) -> Vec<Row> {
    let mut rows: Vec<Row> = snapshot
        .sessions
        .iter()
        .map(|entry| {
            let meta = sessions.iter().find(|s| s.session_id == entry.session_id);
            Row {
                session_id: entry.session_id.clone(),
                title: meta.map(|s| s.title.clone()).unwrap_or_default(),
                provider: meta.map(|s| s.provider),
                cpu: entry.cpu,
                memory: entry.memory,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.cpu
            .total_cmp(&a.cpu)
            .then(b.memory.total_cmp(&a.memory))
            .then(a.session_id.cmp(&b.session_id))
    });
    rows
}

/// CPU is a share of one core, as in Activity Monitor: 100% is one core.
pub(crate) fn format_cpu(percent: f64) -> String {
    if !percent.is_finite() || percent <= 0. {
        return "0%".to_owned();
    }
    if percent < 1. {
        return "<1%".to_owned();
    }
    format!("{}%", percent.round() as i64)
}

pub(crate) fn format_memory(bytes: f64) -> String {
    if !bytes.is_finite() || bytes <= 0. {
        return "0 MB".to_owned();
    }
    let mb = bytes / (1024. * 1024.);
    if mb < 1. {
        return "<1 MB".to_owned();
    }
    if mb < 1024. {
        return format!("{} MB", mb.round() as i64);
    }
    let gb = mb / 1024.;
    if gb < 10. {
        format!("{gb:.1} GB")
    } else {
        format!("{} GB", gb.round() as i64)
    }
}

const POPUP_WIDTH: f32 = 300.;
const ROW_GROUP: &str = "agent-resource-row";

/// Memory pressure's word, when the Mac reports one.
fn pressure_label(pressure: &str) -> Option<&'static str> {
    match pressure {
        "normal" => Some(t("Pressão de memória normal")),
        "warning" => Some(t("Pressão de memória alta")),
        "critical" => Some(t("Pressão de memória crítica")),
        _ => None,
    }
}

/// "Conversation", for a session whose title is blank.
fn title_or_untitled(title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        t("Conversa").to_owned()
    } else {
        title.to_owned()
    }
}

/// Ids of the running conversations, sorted and joined: the footer reads again when this changes.
pub(crate) fn running_key(conversations: &[Conversation]) -> String {
    let mut ids: Vec<&str> = conversations
        .iter()
        .filter(|c| c.working())
        .map(|c| c.session_id.as_str())
        .collect();
    ids.sort_unstable();
    ids.join(",")
}

/// What the panel and the footer know. The panel reads while it is open, and once a little after
/// the running set changes.
#[derive(Default)]
pub(crate) struct State {
    /// The panel's open animation; None while it is closed.
    pub open: Option<Tween>,
    /// Bumped on each open and close, so a finished poll loop does not read again.
    generation: u64,
    pub snapshot: Option<Snapshot>,
    pub error: Option<String>,
    reading: bool,
    pub stopping_sessions: HashSet<String>,
    pub row_errors: HashMap<String, String>,
    /// The running set at the last look.
    running: String,
    /// Where the footer's gauge sits, for the panel's place.
    pub chip: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl State {
    /// Closes the panel; true when it was open.
    pub(crate) fn close(&mut self) -> bool {
        let was_open = self.open.take().is_some();
        if was_open {
            self.generation += 1;
        }
        was_open
    }
}

impl Shell {
    /// Opens or closes the panel. Opening reads now and every 2 s until it closes.
    pub(super) fn toggle_resources(&mut self, cx: &mut Context<Self>) {
        if self.resources.close() {
            cx.notify();
            return;
        }
        self.resources.open = Some(Tween::from_to(
            0.,
            1.,
            Instant::now(),
            if cx.reduce_motion() { 0. } else { 0.16 },
            Curve::Expo,
        ));
        self.resources.generation += 1;
        let generation = self.resources.generation;
        self.read_resources(cx);
        cx.spawn(async move |this, cx| {
            loop {
                let alive = this
                    .update(cx, |this, cx| {
                        if this.resources.generation != generation || this.resources.open.is_none()
                        {
                            return false;
                        }
                        this.read_resources(cx);
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
                cx.background_executor().timer(OPEN_INTERVAL).await;
            }
        })
        .detach();
        cx.notify();
    }

    /// Asks the host for a reading, unless one is already on its way.
    pub(super) fn read_resources(&mut self, cx: &mut Context<Self>) {
        if self.resources.reading {
            return;
        }
        self.resources.reading = true;
        let rx = self.backend.request("agent_resources", json!({}));
        cx.spawn(async move |this, cx| {
            let reading = super::message_actions::settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.resources.reading = false;
                match reading.as_ref().ok().and_then(Snapshot::of) {
                    Some(snapshot) => {
                        this.resources.snapshot = Some(snapshot);
                        this.resources.error = None;
                    }
                    None => {
                        if let Err(problem) = reading {
                            this.resources.error = Some(problem);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Called when a session changes: a new running set reads once the new process has settled.
    pub(super) fn resources_running_changed(&mut self, cx: &mut Context<Self>) {
        let key = running_key(&self.conversations);
        if key == self.resources.running {
            return;
        }
        self.resources.running = key.clone();
        if key.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let _ = this.update(cx, |this, cx| this.read_resources(cx));
        })
        .detach();
    }

    /// Stop a conversation: confirmed in the popup, then the host stops its agent.
    fn ask_stop_resource_session(
        &mut self,
        session_id: String,
        title: String,
        cx: &mut Context<Self>,
    ) {
        self.ask(
            t("Parar conversa?").to_owned(),
            tf(
                "\"{}\" para de trabalhar agora. Os processos dela são encerrados.",
                &[&title],
            ),
            t("Parar"),
            move |this, cx| this.run_stop_session(session_id, cx),
            cx,
        );
    }

    fn run_stop_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        self.resources.stopping_sessions.insert(session_id.clone());
        self.resources.row_errors.remove(&session_id);
        let rx = self
            .backend
            .request("stop_agent", super::delegated_tasks::stop_args(&session_id));
        cx.spawn(async move |this, cx| {
            let stopped = super::message_actions::settle(rx).await;
            let _ = this.update(cx, |this, cx| {
                this.resources.stopping_sessions.remove(&session_id);
                if let Err(problem) = stopped {
                    this.resources.row_errors.insert(session_id, problem);
                }
                this.read_resources(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The footer's gauge: shown only while a conversation runs, coloured by the level.
    pub(super) fn resources_gauge(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if running_key(&self.conversations).is_empty() {
            return None;
        }
        let level = self
            .resources
            .snapshot
            .as_ref()
            .map_or(Level::Normal, Snapshot::level);
        // Only when there's something to act on; its panel stays reachable.
        if level == Level::Normal && self.resources.open.is_none() {
            return None;
        }
        let store = self.resources.chip.clone();
        Some(
            div()
                .id("agent-resources")
                .relative()
                .size(px(24.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .justify_center()
                .flex_shrink_0()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| this.toggle_resources(cx)))
                .child(
                    gpui::canvas(move |bounds, _, _| store.set(Some(bounds)), |_, _, _, _| {})
                        .absolute()
                        .size_full(),
                )
                .child(icon("zap", 16.).text_color(level.color().unwrap_or_else(|| content(0.6))))
                .into_any_element(),
        )
    }

    /// The panel above the footer's gauge: the totals, then each conversation with its processes.
    pub(super) fn resources_popup_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.resources.open.as_ref()?;
        if !open.done(now) {
            *moving = true;
        }
        let shown = open.at(now).clamp(0., 1.);
        let state = &self.resources;
        let left = state
            .chip
            .get()
            .map_or(8., |bounds| f32::from(bounds.left()));
        let meta: Vec<Meta> = self
            .conversations
            .iter()
            .map(|c| Meta {
                session_id: c.session_id.clone(),
                title: c.title.clone(),
                provider: c.provider,
            })
            .collect();
        let track = crate::hover_track::list("agent-resources");
        let tint = state.snapshot.as_ref().and_then(|s| s.level().color());
        let totals = state.snapshot.as_ref().map_or_else(
            || t("Medindo…").to_string(),
            |s| format!("{} · {}", format_cpu(s.cpu), format_memory(s.memory)),
        );
        let mut body = div().flex().flex_col().child(
            div()
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(8.))
                .pt(px(4.))
                .pb(px(7.))
                .child(icon("zap", 13.).text_color(tint.unwrap_or_else(|| content(0.6))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(content(0.9))
                        .child(t("Agentes")),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(tint.unwrap_or_else(|| content(0.55)))
                        .child(totals),
                ),
        );
        let pressure = state
            .snapshot
            .as_ref()
            .and_then(|s| s.machine.pressure.as_deref())
            .filter(|pressure| *pressure != "normal")
            .and_then(pressure_label);
        for warning in pressure
            .map(str::to_owned)
            .into_iter()
            .chain(state.error.clone())
        {
            body = body.child(
                div()
                    .px(px(8.))
                    .pb(px(6.))
                    .text_size(px(11.))
                    .text_color(tint.unwrap_or_else(|| gpui::rgb(RED).into()))
                    .child(warning),
            );
        }
        if let Some(snapshot) = &state.snapshot {
            let machine = snapshot.machine.clone();
            let shown_rows = rows(snapshot, &meta);
            if shown_rows.is_empty() {
                body = body.child(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_size(px(11.5))
                        .text_color(content(0.5))
                        .child(t("Nenhum agente em execução.")),
                );
            }
            for row in shown_rows {
                let busy = state.stopping_sessions.contains(&row.session_id);
                let title = title_or_untitled(&row.title);
                let level = row_level(row.cpu, row.memory, &machine);
                let sid = row.session_id.clone();
                let stop_title = title.clone();
                // Parar takes the figures' place while the pointer is on the row.
                let figures = div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(11.))
                    .text_color(level.color().unwrap_or_else(|| content(0.55)))
                    .when(busy, |d| d.opacity(0.))
                    .when(!busy, |d| d.group_hover(ROW_GROUP, |s| s.opacity(0.)))
                    .child(format_cpu(row.cpu))
                    .child(format_memory(row.memory));
                let stop = div()
                    .id(format!("resource-stop-{sid}"))
                    .absolute()
                    .right(px(4.))
                    .top(px(3.))
                    .h(px(22.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .rounded(px(crate::theme::RADIUS_SM))
                    .text_size(px(11.))
                    .text_color(content(0.7))
                    .when(busy, |d| d.opacity(0.6))
                    .when(!busy, |d| {
                        d.opacity(0.)
                            .group_hover(ROW_GROUP, |s| s.opacity(1.))
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.1)).text_color(content(0.95)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ask_stop_resource_session(sid.clone(), stop_title.clone(), cx)
                            }))
                    })
                    .child(if busy { t("Parando…") } else { t("Parar") });
                body = body.child(
                    track.row_named(
                        div()
                            .relative()
                            .group(ROW_GROUP)
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(8.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .children(
                                row.provider
                                    .map(|provider| crate::tab_tile::provider_mark(provider, 14.)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .text_color(content(0.9))
                                    .child(title),
                            )
                            .child(figures)
                            .child(stop),
                        &row.session_id,
                        false,
                    ),
                );
                if let Some(problem) = state.row_errors.get(&row.session_id) {
                    body = body.child(
                        div()
                            .px(px(8.))
                            .pb(px(4.))
                            .text_size(px(11.))
                            .text_color(gpui::rgb(RED))
                            .child(problem.clone()),
                    );
                }
            }
        }
        Some(
            div()
                .id("agent-resources-popup")
                .occlude()
                .absolute()
                .bottom(px(FOOTER + 2. + (1. - shown) * -6.))
                .left(px(left.min(self.viewport_width - POPUP_WIDTH - 8.).max(8.)))
                .w(px(POPUP_WIDTH))
                .max_h(px(360.))
                .overflow_y_scroll()
                .p(px(6.))
                .opacity(shown)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(body)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(cores: f64, memory_total: Option<f64>, pressure: Option<&str>) -> Machine {
        Machine {
            cores,
            memory_total,
            pressure: pressure.map(str::to_owned),
        }
    }

    fn snapshot_json() -> Value {
        json!({
            "cpuPercent": 150.0,
            "memoryBytes": 2147483648u64,
            "cores": 8,
            "memoryTotal": 17179869184u64,
            "memoryPressure": "warning",
            "sessions": [
                { "sessionId": "a", "cpuPercent": 20.0, "memoryBytes": 1048576u64, "processCount": 3,
                  "processes": [ { "pid": 41, "started": 7, "name": "node", "kind": "tool", "cpuPercent": 20.0, "memoryBytes": 1048576u64 } ] },
                { "sessionId": "b", "cpuPercent": 130.0, "memoryBytes": 1073741824u64, "processCount": 1, "processes": [] }
            ]
        })
    }

    #[test]
    fn totals_turn_amber_and_red_with_cores_memory_and_pressure() {
        let calm = machine(8., Some(16e9), None);
        assert_eq!(total_level(100., 1e9, &calm), Level::Normal);
        // 40% of eight cores' 800 points is 320: amber from there, red from 75%.
        assert_eq!(total_level(320., 1e9, &calm), Level::Elevated);
        assert_eq!(total_level(600., 1e9, &calm), Level::High);
        // A quarter of memory is amber, half is red.
        assert_eq!(total_level(0., 4e9, &calm), Level::Elevated);
        assert_eq!(total_level(0., 8.5e9, &calm), Level::High);
        // The Mac's pressure counts whoever causes it.
        assert_eq!(
            total_level(0., 0., &machine(8., Some(16e9), Some("warning"))),
            Level::Elevated
        );
        assert_eq!(
            total_level(0., 0., &machine(8., Some(16e9), Some("critical"))),
            Level::High
        );
        // No memory total: memory cannot raise the level.
        assert_eq!(
            total_level(0., 1e12, &machine(8., None, None)),
            Level::Normal
        );
    }

    #[test]
    fn rows_have_their_own_thresholds() {
        let m = machine(8., Some(16e9), None);
        assert_eq!(row_level(89., 0., &m), Level::Normal);
        assert_eq!(row_level(90., 0., &m), Level::Elevated);
        assert_eq!(row_level(200., 0., &m), Level::High);
        assert_eq!(row_level(0., 1.6e9, &m), Level::Elevated);
        assert_eq!(row_level(0., 4e9, &m), Level::High);
    }

    #[test]
    fn a_reading_parses_and_reports_its_level() {
        let snapshot = Snapshot::of(&snapshot_json()).expect("parses");
        assert_eq!(snapshot.sessions.len(), 2);
        assert_eq!(snapshot.machine.memory_total, Some(17179869184.0));
        // 150 of 800 points is calm, but the warning pressure makes the total amber.
        assert_eq!(snapshot.level(), Level::Elevated);
        assert!(
            Snapshot::of(&json!({ "cpuPercent": 1 })).is_none(),
            "needs the sessions"
        );
    }

    #[test]
    fn rows_sort_busiest_first_then_by_memory_then_id() {
        let snapshot = Snapshot::of(&snapshot_json()).unwrap();
        let meta = [Meta {
            session_id: "a".into(),
            title: "Alpha".into(),
            provider: "codex",
        }];
        let ordered = rows(&snapshot, &meta);
        assert_eq!(ordered[0].session_id, "b");
        assert_eq!(
            ordered[0].title, "",
            "no title until a conversation matches"
        );
        assert_eq!(ordered[1].title, "Alpha");
        assert_eq!(ordered[1].provider, Some("codex"));
    }

    #[test]
    fn figures_read_compactly() {
        assert_eq!(format_cpu(0.), "0%");
        assert_eq!(format_cpu(0.4), "<1%");
        assert_eq!(format_cpu(99.6), "100%");
        assert_eq!(format_memory(0.), "0 MB");
        assert_eq!(format_memory(500. * 1024. * 1024.), "500 MB");
        assert_eq!(format_memory(1.5 * 1024. * 1024. * 1024.), "1.5 GB");
        assert_eq!(format_memory(12. * 1024. * 1024. * 1024.), "12 GB");
    }

    #[test]
    fn the_running_set_is_one_sorted_key() {
        let mut a = Conversation::placeholder();
        a.session_id = "b".into();
        a.status = "running".into();
        let mut b = Conversation::placeholder();
        b.session_id = "a".into();
        b.status = "waiting".into();
        let mut idle = Conversation::placeholder();
        idle.session_id = "c".into();
        idle.status = "idle".into();
        assert_eq!(running_key(&[a, b, idle]), "a,b");
        assert_eq!(running_key(&[Conversation::placeholder()]), "");
    }

    #[test]
    fn stop_arguments_have_the_host_shapes() {
        assert_eq!(
            super::super::delegated_tasks::stop_args("s"),
            json!({ "sessionId": "s" })
        );
    }

    #[test]
    fn the_close_reports_once() {
        let mut state = State::default();
        assert!(!state.close());
        state.open = Some(Tween::still(1., Instant::now()));
        let before = state.generation;
        assert!(state.close());
        assert_eq!(state.generation, before + 1, "an old poll loop ends");
        assert!(!state.close());
    }

    #[test]
    fn the_labels_follow_the_pressure() {
        assert_eq!(
            pressure_label("critical"),
            Some(t("Pressão de memória crítica"))
        );
        assert_eq!(pressure_label("unknown"), None);
        assert_eq!(title_or_untitled("  "), t("Conversa"));
    }
}
