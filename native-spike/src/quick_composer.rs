//! The quick composer: ⌃⌥Space (changeable in Settings → Atalhos) from any app opens a small
//! floating card, laid out as Monocode's. The header picks the project, where the conversation
//! works (the project's folder or a new worktree) and the branch; the toolbar under the prompt
//! picks attachments, the model with its effort, and the access. Each pill opens a list below the
//! toolbar, inside the card: the card's height tweens so the prompt never moves, and the list
//! fades in. Enter starts the conversation in the background; ⌘Enter also brings the main window
//! to it. Escape closes an open list, and with none open closes the card, as does clicking
//! elsewhere. Settings → Geral → "Composer rápido" turns it off. The window's side of a launch
//! lives in quick_launch.rs.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Window,
    WindowBounds, WindowHandle, WindowKind, WindowOptions, canvas, div, point, px, size,
};
use serde_json::{Value, json};

use crate::i18n::{t, tf};
use crate::panel_files::{TextField, TextFieldEvent};
use crate::shell::Shell;
use crate::theme::{Sinks as _, accent, accent_wash, border, content};

const WIDTH: f32 = 680.;
/// The card with no list open, before the first frame measures it.
const INITIAL_HEIGHT: f32 = 128.;
const MAX_HEIGHT: f32 = 520.;
/// A list taller than this scrolls.
const LIST_HEIGHT: f32 = 300.;
/// Monocode's useQuickPickerMotion: the card's height tween, and the list's fade after a delay.
const PICKER_RESIZE: f32 = 0.24;
const PICKER_FADE_DELAY: f32 = 0.04;
const PICKER_FADE: f32 = 0.18;
const EFFORT_KEY: &str = "quick-effort";
const ACCESS_MODES: [&str; 3] = ["ask", "auto", "full"];

/// The lists the pills open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Picker {
    Project,
    Place,
    Branch,
    Attach,
    Model,
    Access,
}

/// What Escape does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EscapeAction {
    ClosePicker,
    CloseWindow,
}

/// Escape closes the open list first; with none open it closes the card.
fn on_escape(open: Option<Picker>) -> EscapeAction {
    if open.is_some() {
        EscapeAction::ClosePicker
    } else {
        EscapeAction::CloseWindow
    }
}

/// A pill's click: its own list closes, any other list gives way to it.
fn toggle_picker(open: Option<Picker>, wanted: Picker) -> Option<Picker> {
    (open != Some(wanted)).then_some(wanted)
}

/// The rows a search keeps, by position: every row whose text holds the query, ignoring case.
fn filtered<'a>(texts: impl IntoIterator<Item = &'a str>, query: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    texts
        .into_iter()
        .enumerate()
        .filter(|(_, text)| needle.is_empty() || text.to_lowercase().contains(&needle))
        .map(|(ix, _)| ix)
        .collect()
}

/// Up and Down in a list of `count` rows, wrapping at the ends.
fn step(highlight: usize, count: usize, down: bool) -> usize {
    match (count, down) {
        (0, _) => 0,
        (_, true) => (highlight + 1) % count,
        (_, false) => (highlight + count - 1) % count,
    }
}

/// A project as the quick composer offers it.
pub(crate) struct QuickProject {
    pub id: String,
    pub name: String,
    pub path: String,
    pub appearance: Value,
    /// A Git repository: it has branches and can have worktrees.
    pub git: bool,
}

pub(crate) struct QuickModel {
    pub id: String,
    pub name: String,
}

/// One provider's models in the model list.
pub(crate) struct QuickGroup {
    pub provider: &'static str,
    pub title: &'static str,
    pub models: Vec<QuickModel>,
}

/// What the project's latest conversation used.
pub(crate) struct LastUsed {
    pub provider: &'static str,
    pub model: Option<String>,
    pub approval: String,
}

/// Where a new conversation in a project starts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QuickDefaults {
    pub provider: &'static str,
    pub model: Option<String>,
    pub approval: String,
    pub worktree: bool,
}

/// A project's defaults: its latest conversation's provider, model and access, while that
/// provider is still on (an empty `enabled` means the providers aren't known yet); otherwise what
/// the main composer gives a new conversation, `fallback`, with Settings' "Permissões padrão".
/// `worktree` is Settings' "Workspace padrão" for a Git project.
pub(crate) fn resolve_defaults(
    last: Option<LastUsed>,
    enabled: &[&'static str],
    fallback: (&'static str, Option<String>),
    default_approval: Option<&str>,
    worktree: bool,
) -> QuickDefaults {
    let known = |mode: &str| ACCESS_MODES.contains(&mode);
    let approval = default_approval
        .filter(|mode| known(mode))
        .unwrap_or("full")
        .to_owned();
    let Some(last) = last else {
        return QuickDefaults {
            provider: fallback.0,
            model: fallback.1,
            approval,
            worktree,
        };
    };
    let approval = if known(&last.approval) {
        last.approval
    } else {
        approval
    };
    let (provider, model) = if enabled.is_empty() || enabled.contains(&last.provider) {
        (last.provider, last.model)
    } else {
        fallback
    };
    QuickDefaults {
        provider,
        model,
        approval,
        worktree,
    }
}

/// What a launch carries to the window (quick_launch.rs).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QuickLaunch {
    pub project_id: String,
    pub text: String,
    pub provider: &'static str,
    pub model: Option<String>,
    pub approval: String,
    pub worktree: bool,
    /// The local branch a new worktree starts from, when not the usual start.
    pub base: Option<String>,
    /// The branch the project's checkout moves to first.
    pub checkout: Option<String>,
    pub attachments: Vec<PathBuf>,
    /// ⌘Enter: the main window comes forward on the new conversation.
    pub foreground: bool,
}

/// What the chosen branch means: in the project's folder a branch other than the current one is
/// checked out first; for a new worktree it is where the worktree starts. The current branch
/// changes nothing, so a new worktree keeps Settings' start (`HEAD` or origin).
fn branch_plan(
    worktree: bool,
    chosen: Option<&str>,
    current: Option<&str>,
) -> (Option<String>, Option<String>) {
    let other = chosen.filter(|b| Some(*b) != current).map(str::to_owned);
    if worktree {
        (None, other)
    } else {
        (other, None)
    }
}

/// The `create_session` request for a launch.
pub(crate) fn create_request(launch: &QuickLaunch) -> Value {
    use crate::workspace_choice::{Choice, request};
    let choice = if launch.worktree {
        Choice::NewWorktree
    } else {
        Choice::Folder
    };
    let mut out = request(
        &choice,
        &launch.project_id,
        launch.provider,
        launch.model.as_deref(),
    );
    if launch.worktree
        && let Some(base) = &launch.base
    {
        out["baseBranch"] = json!(base);
    }
    out
}

/// Cubic bezier easing, as CSS's `cubic-bezier(x1, y1, x2, y2)`.
fn bezier(t: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    if t <= 0. {
        return 0.;
    }
    if t >= 1. {
        return 1.;
    }
    let f = |s: f32, a: f32, b: f32| {
        3. * (1. - s) * (1. - s) * s * a + 3. * (1. - s) * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0., 1.);
    for _ in 0..14 {
        let middle = (low + high) * 0.5;
        if f(middle, x1, x2) < t {
            low = middle;
        } else {
            high = middle;
        }
    }
    f((low + high) * 0.5, y1, y2)
}

/// The card's height as a list opens, changes or closes: from the height it had to the height
/// its content now measures, and the new list's fade.
#[derive(Default)]
struct Motion {
    height: f32,
    picker: Option<Picker>,
    start: Option<(Instant, f32)>,
}

/// One frame of `Motion`: the card's height, the list's opacity, and whether it still moves.
#[derive(Debug, PartialEq)]
struct Frame {
    height: f32,
    fade: f32,
    moving: bool,
}

impl Motion {
    fn frame(
        &mut self,
        picker: Option<Picker>,
        natural: Option<f32>,
        now: Instant,
        reduce: bool,
    ) -> Frame {
        if self.picker != picker {
            self.picker = picker;
            self.start = (!reduce && self.height > 0.).then_some((now, self.height));
        }
        let (Some((start, from)), Some(to)) = (self.start, natural) else {
            self.start = None;
            self.height = natural.unwrap_or(self.height);
            return Frame {
                height: self.height,
                fade: 1.,
                moving: false,
            };
        };
        let elapsed = now.saturating_duration_since(start).as_secs_f32();
        if elapsed >= PICKER_RESIZE {
            self.start = None;
            self.height = to;
            return Frame {
                height: to,
                fade: 1.,
                moving: false,
            };
        }
        let grow = bezier(elapsed / PICKER_RESIZE, 0.22, 1., 0.36, 1.);
        // `ease-out`, held at zero through the delay.
        let fade = bezier(
            ((elapsed - PICKER_FADE_DELAY) / PICKER_FADE).clamp(0., 1.),
            0.,
            0.,
            0.58,
            1.,
        );
        self.height = from + (to - from) * grow;
        Frame {
            height: self.height,
            fade,
            moving: true,
        }
    }
}

struct QuickComposer {
    field: Entity<TextField>,
    /// The project and branch lists' search.
    query: Entity<TextField>,
    projects: Vec<QuickProject>,
    project: usize,
    worktree: bool,
    /// The project's local branches; None while they load or when it has none.
    branches: Option<Vec<String>>,
    current_branch: Option<String>,
    branch: Option<String>,
    provider: &'static str,
    model: Option<String>,
    model_name: String,
    /// The model's effort levels and the one chosen.
    efforts: (Vec<String>, Option<String>),
    /// The model list, read when it opens.
    groups: Vec<QuickGroup>,
    approval: String,
    attachments: Vec<PathBuf>,
    picker: Option<Picker>,
    highlight: usize,
    motion: Motion,
    /// The content's natural height, measured each frame.
    natural: Rc<Cell<Option<f32>>>,
    /// A file dialog or screenshot holds the keyboard: losing it doesn't close the card.
    away: bool,
    shell: WindowHandle<Shell>,
    _subscriptions: Vec<Subscription>,
}

thread_local! {
    static OPEN: Cell<Option<WindowHandle<QuickComposer>>> = const { Cell::new(None) };
}

/// The registered hotkey's id, read by the event handler (which runs off the main thread).
static WANTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The hotkey manager and the hotkey it holds now. Main thread only, like the manager itself.
struct Hotkeys {
    manager: global_hotkey::GlobalHotKeyManager,
    current: Option<global_hotkey::hotkey::HotKey>,
}

thread_local! {
    static HOTKEYS: std::cell::RefCell<Option<Hotkeys>> = const { std::cell::RefCell::new(None) };
}

/// A combo in the settings' form (`ctrl+alt+space`) as a hotkey.
fn hotkey_of(combo: &str) -> Option<global_hotkey::hotkey::HotKey> {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    let parsed = crate::hotkey_conflicts::Combo::parse(combo)?;
    let mut modifiers = Modifiers::empty();
    for (on, flag) in [
        (parsed.ctrl, Modifiers::CONTROL),
        (parsed.alt, Modifiers::ALT),
        (parsed.shift, Modifiers::SHIFT),
        (parsed.meta, Modifiers::SUPER),
    ] {
        if on {
            modifiers |= flag;
        }
    }
    let code = if parsed.key == "space" {
        Code::Space
    } else {
        parsed.key.parse::<Code>().ok().or_else(|| {
            let letter = parsed
                .key
                .chars()
                .next()
                .filter(|_| parsed.key.len() == 1)?;
            format!("Key{}", letter.to_ascii_uppercase())
                .parse::<Code>()
                .ok()
        })?
    };
    Some(HotKey::new(Some(modifiers), code))
}

/// The combo the settings ask for, and whether the hotkey should be registered at all.
fn wanted(settings: &Value) -> (String, bool) {
    (
        crate::settings_pages::quick_composer_combo(settings).to_owned(),
        settings["quickComposer"] != json!(false),
    )
}

/// Whether `combo` can be registered now, without keeping it: macOS refuses a hotkey another app
/// holds. The hotkey the settings keep stays registered (or comes back) either way.
pub(crate) fn can_register(combo: &str, settings: &Value) -> bool {
    let Some(hotkey) = hotkey_of(combo) else {
        return false;
    };
    let (current, enabled) = wanted(settings);
    if current == combo && enabled {
        return true;
    }
    HOTKEYS.with(|hotkeys| {
        let mut hotkeys = hotkeys.borrow_mut();
        let Some(state) = hotkeys.as_mut() else {
            return true;
        };
        let held = state.current;
        if let Some(old) = held {
            let _ = state.manager.unregister(old);
        }
        let free = state.manager.register(hotkey).is_ok();
        if free {
            let _ = state.manager.unregister(hotkey);
        }
        if let Some(old) = held {
            let _ = state.manager.register(old);
        }
        free
    })
}

/// Makes the registered hotkey match the settings: the old one is unregistered, the new one
/// registered, and with the switch off none is. When the new one cannot be registered the old one
/// stays. Returns whether the settings' hotkey is the one registered now.
pub(crate) fn rebind(settings: &Value) -> bool {
    let (combo, enabled) = wanted(settings);
    HOTKEYS.with(|hotkeys| {
        let mut hotkeys = hotkeys.borrow_mut();
        let Some(state) = hotkeys.as_mut() else {
            return false;
        };
        if !enabled {
            if let Some(old) = state.current.take() {
                let _ = state.manager.unregister(old);
            }
            WANTED.store(0, std::sync::atomic::Ordering::Relaxed);
            return true;
        }
        let Some(hotkey) = hotkey_of(&combo) else {
            return false;
        };
        if state.current == Some(hotkey) {
            return true;
        }
        let old = state.current.take();
        if let Some(old) = old {
            let _ = state.manager.unregister(old);
        }
        match state.manager.register(hotkey) {
            Ok(()) => {
                state.current = Some(hotkey);
                WANTED.store(hotkey.id(), std::sync::atomic::Ordering::Relaxed);
                true
            }
            Err(_) => {
                if let Some(old) = old
                    && state.manager.register(old).is_ok()
                {
                    state.current = Some(old);
                }
                false
            }
        }
    })
}

/// Registers the hotkey from the settings. It lives as long as the app; `rebind` moves it when
/// the settings change. The keyboard is the only way in: there is no menu bar icon.
pub(crate) fn start(shell: WindowHandle<Shell>, cx: &mut App) {
    let (tx, rx) = async_channel::unbounded::<()>();
    if let Ok(manager) = global_hotkey::GlobalHotKeyManager::new() {
        HOTKEYS.with(|hotkeys| {
            *hotkeys.borrow_mut() = Some(Hotkeys {
                manager,
                current: None,
            });
        });
        global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
            move |event: global_hotkey::GlobalHotKeyEvent| {
                if event.id == WANTED.load(std::sync::atomic::Ordering::Relaxed)
                    && event.state == global_hotkey::HotKeyState::Pressed
                {
                    let _ = tx.try_send(());
                }
            },
        ));
    }
    let settings = shell
        .update(cx, |shell, _, _| shell.settings_snapshot())
        .unwrap_or_default();
    rebind(&settings);
    cx.spawn(async move |cx| {
        while rx.recv().await.is_ok() {
            let _ = cx.update(|cx| toggle(shell, cx));
        }
    })
    .detach();
}

/// Removes the card. Deferred: the card's own handlers run inside an update of its window, where
/// removing that window fails silently (and Escape used to do nothing).
fn close(cx: &mut App) {
    if let Some(window) = OPEN.with(|open| open.take()) {
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        });
    }
}

/// The project's branches from the host (`list_branches`), for the open card; None when the host
/// could not list them.
pub(crate) fn branches_arrived(project_id: &str, reply: Option<&Value>, cx: &mut App) {
    let Some(window) = OPEN.with(Cell::get) else {
        return;
    };
    let mut current = None;
    let names: Option<Vec<String>> = reply.map(|value| {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|branch| {
                let name = branch["name"].as_str()?.to_owned();
                if branch["current"].as_bool() == Some(true) {
                    current = Some(name.clone());
                }
                Some(name)
            })
            .collect()
    });
    let _ = window.update(cx, |this, _, cx| {
        if this.projects.get(this.project).map(|p| p.id.as_str()) == Some(project_id) {
            this.branches = names.filter(|names| !names.is_empty());
            this.current_branch = current.clone();
            if this.branch.is_none() {
                this.branch = current;
            }
            cx.notify();
        }
    });
}

fn toggle(shell: WindowHandle<Shell>, cx: &mut App) {
    if OPEN.with(Cell::get).is_some() {
        close(cx);
        return;
    }
    let Ok((enabled, projects)) = shell.update(cx, |shell, _, _| {
        (shell.quick_composer_enabled(), shell.quick_projects())
    }) else {
        return;
    };
    if !enabled {
        return;
    }
    if projects.is_empty() {
        let _ = shell.update(cx, |_, window, _| window.activate_window());
        return;
    }
    let display = cx.primary_display();
    let screen = display.as_ref().map(|d| d.bounds());
    let origin = screen.map_or(point(px(400.), px(240.)), |s| {
        point(
            s.origin.x + (s.size.width - px(WIDTH)) / 2.,
            s.origin.y + s.size.height * 0.28,
        )
    });
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin,
            size: size(px(WIDTH), px(INITIAL_HEIGHT)),
        })),
        titlebar: None,
        kind: WindowKind::PopUp,
        focus: true,
        show: true,
        is_movable: false,
        window_background: gpui::WindowBackgroundAppearance::Blurred,
        display_id: display.map(|d| d.id()),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| QuickComposer::new(projects, shell, window, cx))
    });
    if let Ok(window) = opened {
        OPEN.with(|open| open.set(Some(window)));
        cx.activate(true);
        let _ = window.update(cx, |_, window, _| {
            round_glass(window);
            window.activate_window();
        });
    }
}

impl QuickComposer {
    fn new(
        projects: Vec<QuickProject>,
        shell: WindowHandle<Shell>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| {
            let mut field = TextField::new("", cx);
            field.set_bare(true);
            field.set_placeholder_focused(true);
            field.focus(cx);
            field
        });
        let query = cx.new(|cx| {
            let mut field = TextField::new("", cx);
            field.set_bare(true);
            field.set_placeholder_focused(true);
            field
        });
        let subscriptions = vec![
            cx.subscribe(&field, |this: &mut Self, _, event, cx| match event {
                TextFieldEvent::Submit => this.launch(false, cx),
                TextFieldEvent::Cancel => this.escape(cx),
                TextFieldEvent::Changed => cx.notify(),
            }),
            cx.subscribe(&query, |this: &mut Self, _, event, cx| match event {
                TextFieldEvent::Submit => this.choose(this.highlight, cx),
                TextFieldEvent::Cancel => this.escape(cx),
                TextFieldEvent::Changed => {
                    this.highlight = 0;
                    cx.notify();
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() && !this.away {
                    close(cx);
                }
            }),
        ];
        let mut this = Self {
            field,
            query,
            projects,
            project: 0,
            worktree: false,
            branches: None,
            current_branch: None,
            branch: None,
            provider: "claude",
            model: None,
            model_name: String::new(),
            efforts: (Vec::new(), None),
            groups: Vec::new(),
            approval: "full".into(),
            attachments: Vec::new(),
            picker: None,
            highlight: 0,
            motion: Motion::default(),
            natural: Rc::default(),
            away: false,
            shell,
            _subscriptions: subscriptions,
        };
        this.set_project(0, cx);
        this
    }

    fn current(&self) -> Option<&QuickProject> {
        self.projects.get(self.project)
    }

    /// Moves to a project: its defaults, its branches, its name in the prompt.
    fn set_project(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(project) = self.projects.get(ix) else {
            return;
        };
        self.project = ix;
        let (id, name, git) = (project.id.clone(), project.name.clone(), project.git);
        self.branches = None;
        self.current_branch = None;
        self.branch = None;
        let defaults = self.shell.update(cx, |shell, _, _| {
            if git {
                shell.quick_branches(&id);
            }
            shell.quick_defaults(&id)
        });
        if let Ok(defaults) = defaults {
            self.provider = defaults.provider;
            self.model = defaults.model;
            self.approval = defaults.approval;
            self.worktree = defaults.worktree && git;
        }
        self.refresh_model(cx);
        self.field.update(cx, |field, cx| {
            field.set_placeholder(tf("Peça qualquer coisa no {}…", &[&name]), cx)
        });
        cx.notify();
    }

    /// Reads the model's name and effort from the window.
    fn refresh_model(&mut self, cx: &mut Context<Self>) {
        let (provider, model) = (self.provider, self.model.clone());
        if let Ok((name, efforts)) = self.shell.update(cx, |shell, _, _| {
            (
                shell.quick_model_name(provider, model.as_deref()),
                shell.quick_effort(provider, model.as_deref()),
            )
        }) {
            self.model_name = name;
            self.efforts = efforts;
        }
    }

    fn set_effort(&mut self, level: String, cx: &mut Context<Self>) {
        let Some(model) = self.model.clone() else {
            return;
        };
        let provider = self.provider;
        if let Ok(next) = self.shell.update(cx, |shell, _, _| {
            shell.quick_set_effort(provider, &model, &level)
        }) && next.is_some()
        {
            self.model = next;
        }
        self.refresh_model(cx);
        cx.notify();
    }

    fn focus_prompt(&mut self, cx: &mut Context<Self>) {
        self.field.update(cx, |field, cx| field.focus(cx));
    }

    fn escape(&mut self, cx: &mut Context<Self>) {
        match on_escape(self.picker) {
            EscapeAction::ClosePicker => self.close_picker(cx),
            EscapeAction::CloseWindow => close(cx),
        }
    }

    fn close_picker(&mut self, cx: &mut Context<Self>) {
        self.picker = None;
        self.focus_prompt(cx);
        cx.notify();
    }

    fn open(&mut self, wanted: Picker, cx: &mut Context<Self>) {
        self.picker = toggle_picker(self.picker, wanted);
        self.query.update(cx, |query, cx| query.set_text("", cx));
        self.highlight = 0;
        match self.picker {
            Some(Picker::Project) => {
                self.highlight = self.project;
                self.query.update(cx, |query, cx| {
                    query.set_placeholder(t("Buscar projeto"), cx);
                    query.focus(cx);
                });
            }
            Some(Picker::Branch) => {
                self.highlight = self
                    .branch_rows()
                    .iter()
                    .position(|b| Some(b) == self.branch.as_ref())
                    .unwrap_or(0);
                self.query.update(cx, |query, cx| {
                    query.set_placeholder(t("Buscar branch"), cx);
                    query.focus(cx);
                });
            }
            Some(Picker::Model) => {
                let (provider, model) = (self.provider, self.model.clone());
                if let Ok(groups) = self.shell.update(cx, |shell, _, _| {
                    shell.quick_models((provider, model.as_deref()))
                }) {
                    self.groups = groups;
                }
                self.refresh_model(cx);
                self.highlight = self
                    .model_rows()
                    .iter()
                    .position(|(p, id, _)| *p == provider && Some(id) == model.as_ref())
                    .unwrap_or(0);
                crate::segment_pill::forget(EFFORT_KEY);
                self.focus_prompt(cx);
            }
            Some(Picker::Place) => {
                self.highlight = usize::from(self.worktree);
                self.focus_prompt(cx);
            }
            Some(Picker::Access) => {
                self.highlight = ACCESS_MODES
                    .iter()
                    .position(|m| *m == self.approval)
                    .unwrap_or(0);
                self.focus_prompt(cx);
            }
            Some(Picker::Attach) | None => self.focus_prompt(cx),
        }
        cx.notify();
    }

    fn query_text(&self, cx: &App) -> String {
        self.query.read(cx).text().to_owned()
    }

    /// The project list's rows after the search, by index into `projects`.
    fn project_rows(&self, cx: &App) -> Vec<usize> {
        filtered(
            self.projects.iter().map(|p| p.name.as_str()),
            &self.query_text(cx),
        )
    }

    fn branch_rows(&self) -> Vec<String> {
        self.branches.clone().unwrap_or_default()
    }

    fn filtered_branches(&self, cx: &App) -> Vec<String> {
        let all = self.branch_rows();
        filtered(all.iter().map(String::as_str), &self.query_text(cx))
            .into_iter()
            .map(|ix| all[ix].clone())
            .collect()
    }

    /// The model list flattened: provider, model id, name.
    fn model_rows(&self) -> Vec<(&'static str, String, String)> {
        self.groups
            .iter()
            .flat_map(|g| {
                g.models
                    .iter()
                    .map(|m| (g.provider, m.id.clone(), m.name.clone()))
            })
            .collect()
    }

    fn row_count(&self, cx: &App) -> usize {
        match self.picker {
            Some(Picker::Project) => self.project_rows(cx).len(),
            Some(Picker::Branch) => self.filtered_branches(cx).len(),
            Some(Picker::Place) => 2,
            Some(Picker::Access) => ACCESS_MODES.len(),
            Some(Picker::Attach) => 3,
            Some(Picker::Model) => self.model_rows().len(),
            None => 0,
        }
    }

    /// Picks row `ix` of the open list, which then closes.
    fn choose(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.picker {
            Some(Picker::Project) => {
                if let Some(project) = self.project_rows(cx).get(ix).copied() {
                    self.set_project(project, cx);
                }
            }
            Some(Picker::Branch) => {
                if let Some(branch) = self.filtered_branches(cx).get(ix).cloned() {
                    self.branch = Some(branch);
                }
            }
            Some(Picker::Place) => {
                let git = self.current().is_some_and(|p| p.git);
                self.worktree = ix == 1 && git;
            }
            Some(Picker::Access) => {
                if let Some(mode) = ACCESS_MODES.get(ix) {
                    self.approval = (*mode).to_owned();
                }
            }
            Some(Picker::Model) => {
                if let Some((provider, id, _)) = self.model_rows().get(ix).cloned() {
                    self.provider = provider;
                    self.model = Some(id);
                    self.refresh_model(cx);
                }
            }
            Some(Picker::Attach) => {
                self.picker = None;
                match ix {
                    0 => self.pick_files(cx),
                    1 => self.screenshot(cx),
                    _ => self.paste(cx),
                }
                cx.notify();
                return;
            }
            None => return,
        }
        self.close_picker(cx);
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        self.away = true;
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |_, cx| {
            let picked = paths.await;
            let _ = cx.update(|cx| {
                back(cx, move |this| {
                    if let Ok(Ok(Some(paths))) = picked {
                        this.attachments.extend(paths);
                    }
                })
            });
        })
        .detach();
    }

    /// `screencapture -i`: the person drags the area; nothing happens when they press Escape.
    fn screenshot(&mut self, cx: &mut Context<Self>) {
        self.away = true;
        let path = std::env::temp_dir().join(format!("sirus-captura-{}.png", stamp()));
        let (tx, rx) = async_channel::bounded(1);
        let shot = path.clone();
        std::thread::spawn(move || {
            let done = std::process::Command::new("screencapture")
                .args(["-i", "-o"])
                .arg(&shot)
                .status()
                .is_ok_and(|s| s.success())
                && shot.exists();
            let _ = tx.send_blocking(done);
        });
        cx.spawn(async move |_, cx| {
            let done = rx.recv().await.unwrap_or(false);
            let _ = cx.update(|cx| {
                back(cx, move |this| {
                    if done {
                        this.attachments.push(path);
                    }
                })
            });
        })
        .detach();
    }

    /// Files copied in Finder, or an image on the clipboard saved as a file.
    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        for entry in item.entries() {
            match entry {
                gpui::ClipboardEntry::ExternalPaths(paths) => {
                    self.attachments.extend(paths.paths().iter().cloned());
                }
                gpui::ClipboardEntry::Image(image) => {
                    let extension = match image.format {
                        gpui::ImageFormat::Jpeg => "jpg",
                        gpui::ImageFormat::Gif => "gif",
                        gpui::ImageFormat::Webp => "webp",
                        _ => "png",
                    };
                    let path =
                        std::env::temp_dir().join(format!("sirus-colado-{}.{extension}", stamp()));
                    if std::fs::write(&path, &image.bytes).is_ok() {
                        self.attachments.push(path);
                    }
                }
                _ => {}
            }
        }
        self.focus_prompt(cx);
    }

    fn can_send(&self, cx: &App) -> bool {
        !self.field.read(cx).text().trim().is_empty() || !self.attachments.is_empty()
    }

    /// Enter starts the conversation behind whatever app is in front; ⌘Enter (`foreground`)
    /// also brings the main window to it.
    fn launch(&mut self, foreground: bool, cx: &mut Context<Self>) {
        if !self.can_send(cx) {
            return;
        }
        let Some(project_id) = self.current().map(|p| p.id.clone()) else {
            return;
        };
        let (checkout, base) = branch_plan(
            self.worktree,
            self.branch.as_deref(),
            self.current_branch.as_deref(),
        );
        let launch = QuickLaunch {
            project_id,
            text: self.field.read(cx).text().trim().to_owned(),
            provider: self.provider,
            model: self.model.clone(),
            approval: self.approval.clone(),
            worktree: self.worktree,
            base,
            checkout,
            attachments: std::mem::take(&mut self.attachments),
            foreground,
        };
        close(cx);
        let _ = self.shell.update(cx, |shell, window, cx| {
            if foreground {
                window.activate_window();
            }
            shell.quick_launch(launch, cx);
        });
        if foreground {
            cx.activate(true);
        } else {
            cx.hide();
        }
    }
}

/// The card again after a file dialog or screenshot: keyboard back, then `apply`.
fn back(cx: &mut App, apply: impl FnOnce(&mut QuickComposer) + 'static) {
    let Some(window) = OPEN.with(Cell::get) else {
        return;
    };
    let _ = window.update(cx, |this, window, cx| {
        this.away = false;
        apply(this);
        this.focus_prompt(cx);
        window.activate_window();
        cx.notify();
    });
}

/// A name for a temporary file, unique enough within a second.
fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

/// Keeps the card's top edge where it is while the window takes `height`, so the prompt never
/// moves as a list opens below it. Deferred to after the frame: AppKit's resize updates the
/// window, which fails mid-render.
#[cfg(not(test))]
fn fit_height(window: &Window, cx: &App, height: f32) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: GPUI's AppKit handle points at its live content view.
    let view = unsafe { &*appkit.ns_view.as_ptr().cast::<objc2_app_kit::NSView>() };
    let Some(ns_window) = view.window() else {
        return;
    };
    let height = f64::from(height);
    if (ns_window.frame().size.height - height).abs() < 0.5 {
        return;
    }
    cx.foreground_executor()
        .spawn(async move {
            let mut frame = ns_window.frame();
            if (frame.size.height - height).abs() < 0.5 {
                return;
            }
            // AppKit's origin is the bottom left.
            frame.origin.y += frame.size.height - height;
            frame.size.height = height;
            ns_window.setFrame_display(frame, true);
            ns_window.invalidateShadow();
        })
        .detach();
}

#[cfg(test)]
fn fit_height(_: &Window, _: &App, _: f32) {}

/// The window's glass takes the card's rounded corners, so no square of blur shows past the
/// border.
#[cfg(not(test))]
fn round_glass(window: &Window) {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: GPUI's AppKit handle points at its live content view.
    let view = unsafe { &*appkit.ns_view.as_ptr().cast::<objc2_app_kit::NSView>() };
    let Some(ns_window) = view.window() else {
        return;
    };
    let Some(content) = ns_window.contentView() else {
        return;
    };
    let mut views = vec![content.clone()];
    views.extend(content.subviews().iter());
    for view in views {
        view.setWantsLayer(true);
        // SAFETY: a layer-backed view's `layer` is a CALayer, which has both setters.
        unsafe {
            let layer: Option<Retained<AnyObject>> = msg_send![&*view, layer];
            if let Some(layer) = layer {
                let _: () = msg_send![&*layer, setCornerRadius: f64::from(crate::theme::RADIUS_LG)];
                let _: () = msg_send![&*layer, setMasksToBounds: true];
            }
        }
    }
    ns_window.invalidateShadow();
}

#[cfg(test)]
fn round_glass(_: &Window) {}

fn kbd(text: &'static str) -> impl IntoElement {
    div()
        .px(px(4.))
        .rounded(px(4.))
        .border_1()
        .border_color(border())
        .text_size(px(10.5))
        .text_color(content(0.6))
        .child(text)
}

/// The header's and toolbar's ghost pills: no fill at rest, a soft one while their list is open.
fn pill(id: &'static str, open: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .min_w_0()
        .h(px(26.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(crate::theme::RADIUS_SM))
        .border_1()
        .text_size(px(12.5))
        .cursor_pointer()
        .when(open, |d| {
            d.bg(content(0.08))
                .border_color(border())
                .text_color(content(0.95))
        })
        .when(!open, |d| {
            d.border_color(gpui::transparent_black())
                .text_color(content(0.75))
                .hover(|s| s.bg(content(0.06)).text_color(content(0.95)))
        })
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
}

fn chevron() -> impl IntoElement {
    crate::shell::icon("chevron-down", 10.).text_color(content(0.5))
}

fn check() -> impl IntoElement {
    div()
        .ml(px(8.))
        .child(crate::shell::icon("check", 12.).text_color(accent()))
}

/// A path as people write it, `~` for the home folder.
fn short_path(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_owned(),
    }
}

impl QuickComposer {
    fn row(&self, id: (&'static str, usize), cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let ix = id.1;
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(9.))
            .px(px(9.))
            .py(px(6.))
            .rounded(px(crate::theme::RADIUS_SM))
            .text_size(px(13.))
            .text_color(content(0.9))
            .cursor_pointer()
            .when(ix == self.highlight, |d| d.bg(content(0.07)))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.highlight != ix {
                    this.highlight = ix;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.choose(ix, cx)))
    }

    fn sub(text: impl Into<gpui::SharedString>) -> impl IntoElement {
        div()
            .ml_auto()
            .pl(px(12.))
            .min_w_0()
            .truncate()
            .text_size(px(11.5))
            .text_color(content(0.5))
            .child(text.into())
    }

    fn search(&self) -> impl IntoElement {
        div()
            .mx(px(4.))
            .mt(px(2.))
            .mb(px(6.))
            .h(px(30.))
            .px(px(9.))
            .flex()
            .items_center()
            .gap(px(7.))
            .rounded(px(crate::theme::RADIUS_SM))
            .bg(content(0.05))
            .child(crate::shell::icon("search", 12.).text_color(content(0.5)))
            .child(div().flex_1().min_w_0().child(self.query.clone()))
    }

    fn nothing() -> AnyElement {
        div()
            .px(px(9.))
            .py(px(8.))
            .text_size(px(12.))
            .text_color(content(0.45))
            .child(t("Nada encontrado"))
            .into_any_element()
    }

    fn header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let project = self.current();
        let name = project.map_or(String::new(), |p| p.name.clone());
        let appearance = project.map_or(Value::Null, |p| p.appearance.clone());
        let git = project.is_some_and(|p| p.git);
        let place = if self.worktree {
            t("Novo worktree")
        } else {
            "Local"
        };
        let branch = self.branch.clone();
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(12.))
            .pt(px(10.))
            .child(
                pill("quick-project", self.picker == Some(Picker::Project))
                    .max_w(px(260.))
                    .child(crate::project_avatar::project_avatar(
                        &name,
                        &appearance,
                        16.,
                    ))
                    .child(div().min_w_0().truncate().child(name))
                    .child(chevron())
                    .on_click(cx.listener(|this, _, _, cx| this.open(Picker::Project, cx))),
            )
            .child(
                pill("quick-place", self.picker == Some(Picker::Place))
                    .child(
                        crate::shell::icon(
                            if self.worktree {
                                "folder-git"
                            } else {
                                "folder"
                            },
                            12.,
                        )
                        .text_color(content(0.6)),
                    )
                    .child(place)
                    .child(chevron())
                    .on_click(cx.listener(|this, _, _, cx| this.open(Picker::Place, cx))),
            )
            .when(git, |d| {
                d.child(
                    pill("quick-branch", self.picker == Some(Picker::Branch))
                        .max_w(px(220.))
                        .child(crate::shell::icon("branch", 12.).text_color(content(0.6)))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(branch.unwrap_or_else(|| "…".to_owned())),
                        )
                        .child(chevron())
                        .on_click(cx.listener(|this, _, _, cx| this.open(Picker::Branch, cx))),
                )
            })
            .into_any_element()
    }

    fn attachment_chips(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.attachments.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .px(px(14.))
                .pb(px(8.))
                .children(self.attachments.iter().enumerate().map(|(ix, path)| {
                    let name = path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                    div()
                        .id(("quick-attachment", ix))
                        .h(px(22.))
                        .pl(px(7.))
                        .pr(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .max_w(px(220.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .bg(content(0.06))
                        .text_size(px(11.5))
                        .text_color(content(0.8))
                        .child(crate::shell::icon("paperclip", 12.).text_color(content(0.55)))
                        .child(div().min_w_0().truncate().child(name))
                        .child(
                            div()
                                .id(("quick-attachment-remove", ix))
                                .size(px(16.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(content(0.1)))
                                .child(crate::shell::icon("close", 10.).text_color(content(0.55)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if ix < this.attachments.len() {
                                        this.attachments.remove(ix);
                                    }
                                    cx.notify();
                                })),
                        )
                }))
                .into_any_element(),
        )
    }

    fn toolbar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let effort = self.efforts.1.as_deref().map(crate::shell::effort_label);
        let (glyph, access, _, tone) = crate::shell::access_look(&self.approval);
        let ready = self.can_send(cx);
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .border_t_1()
            .border_color(content(0.08))
            .px(px(12.))
            .py(px(8.))
            .child(
                pill("quick-attach", self.picker == Some(Picker::Attach))
                    .w(px(26.))
                    .px_0()
                    .justify_center()
                    .child(crate::shell::icon("plus", 14.).text_color(content(0.7)))
                    .on_click(cx.listener(|this, _, _, cx| this.open(Picker::Attach, cx))),
            )
            .child(
                pill("quick-model", self.picker == Some(Picker::Model))
                    .max_w(px(260.))
                    .child(crate::shell::provider_mark(self.provider, 13.))
                    .child(div().min_w_0().truncate().child(self.model_name.clone()))
                    .children(effort.map(|e| div().text_color(content(0.5)).child(e)))
                    .child(chevron())
                    .on_click(cx.listener(|this, _, _, cx| this.open(Picker::Model, cx))),
            )
            .child(
                pill("quick-access", self.picker == Some(Picker::Access))
                    .child(crate::shell::icon(glyph, 13.).text_color(tone))
                    .child(div().text_color(tone).child(access))
                    .child(chevron())
                    .on_click(cx.listener(|this, _, _, cx| this.open(Picker::Access, cx))),
            )
            .child(
                div()
                    .ml_auto()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(kbd("↵"))
                            .child(t("enviar")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(kbd("⌘↵"))
                            .child(t("enviar e abrir")),
                    )
                    .child(
                        div()
                            .id("quick-send")
                            .h(px(26.))
                            .px(px(11.))
                            .flex()
                            .items_center()
                            .rounded(px(crate::theme::RADIUS_SM))
                            .bg(accent())
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(crate::theme::on_accent())
                            .when(!ready, |d| d.opacity(0.45))
                            .when(ready, |d| d.cursor_pointer().sinks())
                            .child(t("Enviar"))
                            .on_click(cx.listener(|this, _, _, cx| this.launch(false, cx))),
                    ),
            )
            .into_any_element()
    }

    fn list(&mut self, picker: Picker, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match picker {
            Picker::Project => {
                let rows = self.project_rows(cx);
                let mut out = vec![self.search().into_any_element()];
                if rows.is_empty() {
                    out.push(Self::nothing());
                }
                for (ix, at) in rows.into_iter().enumerate() {
                    let project = &self.projects[at];
                    let (name, path) = (project.name.clone(), short_path(&project.path));
                    let avatar =
                        crate::project_avatar::project_avatar(&name, &project.appearance, 20.);
                    let chosen = at == self.project;
                    out.push(
                        self.row(("quick-project-row", ix), cx)
                            .child(avatar)
                            .child(div().min_w_0().truncate().child(name))
                            .child(Self::sub(path))
                            .when(chosen, |d| d.child(check()))
                            .into_any_element(),
                    );
                }
                out
            }
            Picker::Branch => {
                let rows = self.filtered_branches(cx);
                let mut out = vec![self.search().into_any_element()];
                if rows.is_empty() {
                    out.push(if self.branches.is_none() {
                        div()
                            .px(px(9.))
                            .py(px(8.))
                            .text_size(px(12.))
                            .text_color(content(0.45))
                            .child(t("Carregando…"))
                            .into_any_element()
                    } else {
                        Self::nothing()
                    });
                }
                for (ix, branch) in rows.into_iter().enumerate() {
                    let chosen = Some(&branch) == self.branch.as_ref();
                    out.push(
                        self.row(("quick-branch-row", ix), cx)
                            .child(crate::shell::icon("branch", 12.).text_color(content(0.55)))
                            .child(div().min_w_0().truncate().child(branch))
                            .when(chosen, |d| d.child(div().ml_auto().child(check())))
                            .into_any_element(),
                    );
                }
                out
            }
            Picker::Place => {
                let git = self.current().is_some_and(|p| p.git);
                [
                    (
                        "folder",
                        "Local",
                        t("Na pasta do projeto"),
                        !self.worktree,
                        true,
                    ),
                    (
                        "folder-git",
                        t("Novo worktree"),
                        if git {
                            t("Uma cópia isolada, numa branch nova")
                        } else {
                            t("Precisa de um repositório Git")
                        },
                        self.worktree,
                        git,
                    ),
                ]
                .into_iter()
                .enumerate()
                .map(|(ix, (glyph, label, about, chosen, usable))| {
                    self.row(("quick-place-row", ix), cx)
                        .when(!usable, |d| d.opacity(0.4))
                        .child(crate::shell::icon(glyph, 13.).text_color(content(0.6)))
                        .child(label)
                        .child(Self::sub(about))
                        .when(chosen, |d| d.child(check()))
                        .into_any_element()
                })
                .collect()
            }
            Picker::Access => ACCESS_MODES
                .into_iter()
                .enumerate()
                .map(|(ix, mode)| {
                    let (glyph, label, about, tone) = crate::shell::access_look(mode);
                    let chosen = mode == self.approval;
                    self.row(("quick-access-row", ix), cx)
                        .child(crate::shell::icon(glyph, 13.).text_color(tone))
                        .child(
                            div()
                                .text_color(if mode == "full" { tone } else { content(0.9) })
                                .child(label),
                        )
                        .child(Self::sub(about))
                        .when(chosen, |d| d.child(check()))
                        .into_any_element()
                })
                .collect(),
            Picker::Attach => [
                ("paperclip", t("Arquivo ou imagem…")),
                ("image", t("Captura de tela")),
                ("clipboard", t("Colar da área de transferência")),
            ]
            .into_iter()
            .enumerate()
            .map(|(ix, (glyph, label))| {
                self.row(("quick-attach-row", ix), cx)
                    .child(crate::shell::icon(glyph, 13.).text_color(content(0.6)))
                    .child(label)
                    .into_any_element()
            })
            .collect(),
            Picker::Model => {
                let mut out = Vec::new();
                let mut ix = 0;
                let groups: Vec<_> = self
                    .groups
                    .iter()
                    .map(|g| {
                        (
                            g.provider,
                            g.title,
                            g.models
                                .iter()
                                .map(|m| (m.id.clone(), m.name.clone()))
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect();
                for (provider, title, models) in groups {
                    out.push(
                        div()
                            .px(px(9.))
                            .pt(px(8.))
                            .pb(px(3.))
                            .text_size(px(11.))
                            .text_color(content(0.5))
                            .child(title)
                            .into_any_element(),
                    );
                    for (id, name) in models {
                        let chosen = provider == self.provider && Some(&id) == self.model.as_ref();
                        out.push(
                            self.row(("quick-model-row", ix), cx)
                                .child(crate::shell::provider_mark(provider, 13.))
                                .child(div().min_w_0().truncate().child(name))
                                .when(chosen, |d| d.child(div().ml_auto().child(check())))
                                .into_any_element(),
                        );
                        ix += 1;
                    }
                }
                if ix == 0 {
                    out.push(Self::nothing());
                }
                out
            }
        }
    }

    /// The model list's footer: the model's effort levels as a segmented control whose accent
    /// pill slides to the chosen one.
    fn effort(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (levels, chosen) = self.efforts.clone();
        if levels.is_empty() {
            return None;
        }
        let at = chosen
            .as_ref()
            .and_then(|c| levels.iter().position(|l| l == c))
            .unwrap_or(0);
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .mt(px(6.))
                .px(px(9.))
                .pt(px(10.))
                .pb(px(4.))
                .border_t_1()
                .border_color(content(0.08))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .child(t("Esforço")),
                )
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .flex()
                        .p(px(2.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .border_1()
                        .border_color(border())
                        .child(crate::segment_pill::pill(
                            EFFORT_KEY,
                            at,
                            accent_wash(0.18),
                            crate::theme::RADIUS_SM,
                        ))
                        .children(levels.into_iter().enumerate().map(|(ix, level)| {
                            let on = ix == at;
                            div()
                                .id(("quick-effort", ix))
                                .relative()
                                .flex_1()
                                .child(crate::segment_pill::probe(EFFORT_KEY, ix))
                                .sinks()
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(crate::theme::RADIUS_SM))
                                .text_size(px(12.))
                                .text_color(if on { accent() } else { content(0.55) })
                                .when(!on, |d| d.hover(|s| s.text_color(content(0.9))))
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .child(crate::shell::effort_label(&level))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_effort(level.clone(), cx)
                                }))
                        })),
                )
                .into_any_element(),
        )
    }

    fn keys(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let command = event.keystroke.modifiers.platform;
        match key {
            "escape" => self.escape(cx),
            "enter" if command => self.launch(true, cx),
            "enter" if self.picker.is_some() => self.choose(self.highlight, cx),
            "up" | "down" if self.picker.is_some() => {
                self.highlight = step(self.highlight, self.row_count(cx), key == "down");
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for QuickComposer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.motion.frame(
            self.picker,
            self.natural.get(),
            Instant::now(),
            cx.reduce_motion(),
        );
        if frame.moving {
            window.request_animation_frame();
        }
        // Nothing is measured before the first frame: the window keeps its first height.
        if frame.height > 0. {
            fit_height(window, cx, frame.height.min(MAX_HEIGHT));
        }
        let natural = self.natural.clone();
        let header = self.header(cx);
        let chips = self.attachment_chips(cx);
        let toolbar = self.toolbar(cx);
        let list = self.picker.map(|picker| {
            let rows = self.list(picker, cx);
            let effort = (picker == Picker::Model).then(|| self.effort(cx)).flatten();
            div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(content(0.08))
                .p(px(6.))
                .opacity(frame.fade)
                .child(
                    div()
                        .id("quick-list")
                        .flex()
                        .flex_col()
                        .max_h(px(LIST_HEIGHT))
                        .overflow_y_scroll()
                        .children(rows),
                )
                .children(effort)
        });
        div()
            .id("quick-composer")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(crate::theme::RADIUS_LG))
            .border_1()
            .border_color(border())
            .text_color(content(0.95))
            .capture_key_down(
                cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| this.keys(event, cx)),
            )
            .child(
                // The content at its own height, measured each frame: mid-tween the card clips
                // it, so a list is revealed rather than squeezed.
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .w_full()
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                // The border's pixel above and below. A new height draws
                                // again, so the window follows it.
                                let height = Some(f32::from(bounds.size.height) + 2.);
                                if natural.get() != height {
                                    natural.set(height);
                                    window.request_animation_frame();
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .child(header)
                    .child(
                        div()
                            .px(px(16.))
                            .pt(px(10.))
                            .pb(px(10.))
                            .text_size(px(15.))
                            .child(self.field.clone()),
                    )
                    .children(chips)
                    .child(toolbar)
                    .children(list),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn escape_closes_the_list_before_the_window() {
        assert_eq!(on_escape(Some(Picker::Model)), EscapeAction::ClosePicker);
        assert_eq!(on_escape(Some(Picker::Branch)), EscapeAction::ClosePicker);
        assert_eq!(on_escape(None), EscapeAction::CloseWindow);
    }

    #[test]
    fn a_pill_opens_its_list_closes_it_and_replaces_another() {
        assert_eq!(toggle_picker(None, Picker::Project), Some(Picker::Project));
        assert_eq!(toggle_picker(Some(Picker::Project), Picker::Project), None);
        assert_eq!(
            toggle_picker(Some(Picker::Project), Picker::Access),
            Some(Picker::Access)
        );
    }

    #[test]
    fn search_keeps_matching_rows_ignoring_case() {
        let names = ["sirus", "moradah", "Agendese", "newlex"];
        assert_eq!(filtered(names, ""), vec![0, 1, 2, 3]);
        assert_eq!(filtered(names, " AGEN "), vec![2]);
        assert_eq!(filtered(names, "e"), vec![2, 3]);
        assert!(filtered(names, "zzz").is_empty());
    }

    #[test]
    fn arrows_wrap_around_the_list() {
        assert_eq!(step(0, 3, true), 1);
        assert_eq!(step(2, 3, true), 0);
        assert_eq!(step(0, 3, false), 2);
        assert_eq!(step(0, 0, true), 0);
    }

    #[test]
    fn defaults_follow_the_projects_last_conversation() {
        let last = LastUsed {
            provider: "codex",
            model: Some("gpt-6.1".into()),
            approval: "ask".into(),
        };
        let got = resolve_defaults(
            Some(last),
            &["claude", "codex"],
            ("claude", Some("opus".into())),
            Some("auto"),
            true,
        );
        assert_eq!(
            got,
            QuickDefaults {
                provider: "codex",
                model: Some("gpt-6.1".into()),
                approval: "ask".into(),
                worktree: true,
            }
        );
    }

    #[test]
    fn defaults_fall_back_to_the_main_composers() {
        // No conversation yet: the main composer's provider and model, Settings' access.
        let got = resolve_defaults(None, &["claude"], ("claude", None), Some("auto"), false);
        assert_eq!(
            (got.provider, got.model, got.approval.as_str()),
            ("claude", None, "auto")
        );
        // An unknown or missing default access is full access, as a new conversation's.
        let got = resolve_defaults(None, &[], ("claude", None), Some("bogus"), false);
        assert_eq!(got.approval, "full");
        // A provider since turned off gives way, but the access it used stays.
        let last = LastUsed {
            provider: "cursor",
            model: Some("auto".into()),
            approval: "auto".into(),
        };
        let got = resolve_defaults(
            Some(last),
            &["claude"],
            ("claude", Some("opus".into())),
            None,
            false,
        );
        assert_eq!(
            (got.provider, got.model.as_deref(), got.approval.as_str()),
            ("claude", Some("opus"), "auto")
        );
        // Providers not detected yet: the last one is kept.
        let last = LastUsed {
            provider: "codex",
            model: None,
            approval: "full".into(),
        };
        assert_eq!(
            resolve_defaults(Some(last), &[], ("claude", None), None, false).provider,
            "codex"
        );
    }

    #[test]
    fn the_branch_moves_the_checkout_or_starts_the_worktree() {
        // Local: another branch is checked out first; the current one changes nothing.
        assert_eq!(
            branch_plan(false, Some("dev"), Some("main")),
            (Some("dev".into()), None)
        );
        assert_eq!(branch_plan(false, Some("main"), Some("main")), (None, None));
        // New worktree: another branch is where it starts.
        assert_eq!(
            branch_plan(true, Some("dev"), Some("main")),
            (None, Some("dev".into()))
        );
        assert_eq!(branch_plan(true, Some("main"), Some("main")), (None, None));
        assert_eq!(branch_plan(true, None, None), (None, None));
    }

    fn launch() -> QuickLaunch {
        QuickLaunch {
            project_id: "p1".into(),
            text: "oi".into(),
            provider: "claude",
            model: Some("opus".into()),
            approval: "ask".into(),
            worktree: true,
            base: Some("dev".into()),
            checkout: None,
            attachments: Vec::new(),
            foreground: false,
        }
    }

    #[test]
    fn the_request_names_the_worktree_and_its_start() {
        let request = create_request(&launch());
        assert_eq!(request["isolatedWorktree"], true);
        assert_eq!(request["baseBranch"], "dev");
        assert_eq!(request["model"], "opus");
        assert_eq!(request["agent"], "claude");
        let local = create_request(&QuickLaunch {
            worktree: false,
            ..launch()
        });
        assert_eq!(local["isolatedWorktree"], false);
        assert!(local.get("baseBranch").is_none());
    }

    #[test]
    fn the_card_tweens_to_the_new_height_and_the_list_fades_in() {
        let now = Instant::now();
        let mut motion = Motion::default();
        // The first frame takes the measured height as it is.
        let first = motion.frame(None, Some(120.), now, false);
        assert_eq!(
            first,
            Frame {
                height: 120.,
                fade: 1.,
                moving: false
            }
        );
        // A list opens: the card starts where it was and the list is still clear.
        let open = motion.frame(Some(Picker::Model), Some(120.), now, false);
        assert_eq!((open.height, open.fade, open.moving), (120., 0., true));
        let mid = motion.frame(
            Some(Picker::Model),
            Some(400.),
            now + Duration::from_millis(120),
            false,
        );
        assert!(mid.moving && mid.height > 120. && mid.height < 400.);
        assert!(mid.fade > 0. && mid.fade < 1.);
        let done = motion.frame(
            Some(Picker::Model),
            Some(400.),
            now + Duration::from_millis(300),
            false,
        );
        assert_eq!(
            done,
            Frame {
                height: 400.,
                fade: 1.,
                moving: false
            }
        );
        // Closing tweens back down.
        let later = now + Duration::from_secs(1);
        let closing = motion.frame(None, Some(120.), later, false);
        assert!(closing.moving && closing.height == 400.);
        let closed = motion.frame(None, Some(120.), later + Duration::from_millis(250), false);
        assert_eq!(closed.height, 120.);
    }

    #[test]
    fn reduced_motion_jumps_to_the_new_height() {
        let now = Instant::now();
        let mut motion = Motion::default();
        motion.frame(None, Some(120.), now, true);
        let open = motion.frame(Some(Picker::Access), Some(260.), now, true);
        assert_eq!(
            open,
            Frame {
                height: 260.,
                fade: 1.,
                moving: false
            }
        );
    }

    #[test]
    fn the_easing_runs_from_zero_to_one() {
        assert_eq!(bezier(0., 0.22, 1., 0.36, 1.), 0.);
        assert_eq!(bezier(1., 0.22, 1., 0.36, 1.), 1.);
        let half = bezier(0.5, 0.22, 1., 0.36, 1.);
        assert!(half > 0.8 && half < 1.);
    }

    #[test]
    fn home_reads_as_a_tilde() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            assert_eq!(
                short_path(&format!("{home}/Projetos/sirus")),
                "~/Projetos/sirus"
            );
        }
        assert_eq!(short_path("/opt/x"), "/opt/x");
    }
}
