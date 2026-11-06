//! The window: title strip, sidebar and a transcript with the live activity tree.
#[path = "shell_terminal.rs"]
mod shell_terminal;
use crate::backend::{self, Backend, Incoming};
use crate::data::{self, Conversation, Project, Turn};
use crate::effort_energy;
use crate::git_sync::{self, IncomingState};
use crate::i18n::{t, tf};
use crate::marks::{SPINNER_PERIOD, TILE_RAIL_INDENT, rail, shimmer, spinner_at};
use crate::motion::{Curve, DISCLOSE, MENU, REORDER, SIDEBAR, SWITCH, TAB_OPEN, Tween, eased};
use crate::tab_label::tab_label;
use crate::tab_tile::{
    ACTIVITY_TILE, Face, FanCard, TAB_TILE, activity_face, fan_card, fan_tile, fan_width,
    mark_tile, push_tile, tab_tile_inset,
};
use crate::theme::{
    ACCENTS, FOOTER, Look, PLATE, PRESETS, Palette, TITLE_BAR, accent, accent_wash, chrome_wash,
    content, diff_colors, field_fill, floating_fill, icon as theme_icon, mix, on_accent, palette,
    pane_fill, pane_opaque, provider_glyph, shell_fill,
};
use gpui::transparent_black as transparent;
use gpui::{
    AnyElement, Context, Div, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, SharedString, Stateful, Transformation,
    Window, WindowControlArea, div, linear_color_stop, linear_gradient, percentage, prelude::*, px,
    rgb, svg,
};
use gpui::{
    Bounds, ElementInputHandler, EntityInputHandler, StyledText, TextLayout, UTF16Selection,
};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::rc::Rc;

/// The card for a turn that ended on an error, with a retry and a move to another account.
#[path = "account_card.rs"]
mod account_card;
/// What the running agents cost the Mac: the footer's gauge and its panel (agent_resources.rs).
#[path = "agent_resources.rs"]
mod agent_resources;
/// The message queue, steering and the composer keys that go with them.
#[path = "composer_queue.rs"]
mod composer_queue;
/// The composer's context ring, its popup, Compact and Compact and send (ADR-057).
#[path = "context_meter.rs"]
pub(crate) mod context_meter;
/// Saving a conversation as Markdown from its context menu (conversation_export.rs).
#[path = "conversation_export.rs"]
mod conversation_export;
/// The tasks a reply delegated to other models, under the reply (delegated_tasks.rs).
#[path = "delegated_tasks.rs"]
mod delegated_tasks;
/// The transcript's follow of the end: glides instead of jumping (glide.rs).
#[path = "glide.rs"]
mod glide;
/// The streamed reply revealed at a steady pace (Monocode's wordFade rhythm).
#[path = "paced.rs"]
mod paced;
/// The sidebar's hover card: a conversation's model, work time, title and folder (sidebar_card.rs).
#[path = "sidebar_card.rs"]
pub(crate) mod sidebar_card;
/// The subagents of a turn as one card.
#[path = "subagent_card.rs"]
mod subagent_card;
/// The Usage page: Custo, Tokens and Limites over the window (usage_page.rs).
#[path = "usage_page.rs"]
mod usage_page;

/// Cards and project badges for other conversations (activity_cards.rs).
#[path = "activity_cards.rs"]
pub(crate) mod activity_cards;
/// The first run: a full-window welcome over the letters robot (first_run.rs).
#[path = "first_run.rs"]
pub(crate) mod first_run;
/// All-conversation search: ⌘⇧F and the palette's message results (global_search.rs).
#[path = "global_search.rs"]
pub(crate) mod global_search;
/// The row of actions under a finished reply: copy, try again, undo, side chat (ADR-187).
#[path = "message_actions.rs"]
pub(crate) mod message_actions;
/// ⌘F: the find bar over the open conversation (transcript_find.rs).
#[path = "transcript_find.rs"]
pub(crate) mod transcript_find;
/// Selecting the text of replies and user messages (transcript_select.rs).
#[path = "transcript_select.rs"]
pub(crate) mod transcript_select;

#[path = "handoff.rs"]
pub(crate) mod handoff;

#[path = "computer_switch.rs"]
pub(crate) mod computer_switch;
#[path = "computer_ui.rs"]
pub(crate) mod computer_ui;
#[path = "prompt_link.rs"]
pub(crate) mod prompt_link;
/// The changed files under a finished reply, opened in place (turn_review.rs).
#[path = "turn_review.rs"]
mod turn_review;
use std::time::{Duration, Instant};

#[path = "attachment_view.rs"]
mod attachment_view;
/// Conversations side by side: the layout tree, the panes, their drop zones and slide (split_panes.rs).
#[path = "split_panes.rs"]
pub(crate) mod split_panes;

const NAV: f32 = 78. + 80.;
const GAP: f32 = 2.;
const PIN_WIDTH: f32 = 26.;
/// Between the pinned deck and the strip's first tab, split by a hairline.
const DOCK_GAP: f32 = 6.;
/// The most pins that sit in a plain row; more gather into a deck.
const ROW_MAX: usize = 3;
const FAN: f32 = 0.24;
const SHUFFLE: f32 = 0.34;
/// Seconds a tile's mark and its chevron take to trade places.
const MORPH: f32 = 0.3;
const DOCK_MIN: f32 = 280.;
const DOCK_MAX_WIDTH: f32 = 900.;
/// The side panel's six options, as Sirus's RightDock: icon, name, what it does, its key.
const PANES: [(&str, &str, &str); 6] = [
    ("square-terminal", "Terminal", "Rode comandos no projeto."),
    ("files", "Arquivos", "Abra os arquivos do projeto."),
    ("git-compare", "Alterações", "Revise o diff e faça commit."),
    ("globe", "Navegador", "Abra um app ou qualquer site."),
    ("messages", "Chat lateral", "Uma conversa à parte."),
    ("smartphone", "Simulador", "Rode o app num aparelho."),
];
/// The Simulator is built but hidden for now: the owner doesn't need it yet.
const VISIBLE_PANES: usize = 5;
const COMMANDS: [&str; 17] = [
    "Nova conversa",
    "Abrir ou fechar a sidebar",
    "Abrir ou fechar o painel lateral",
    "Configurações",
    "Fixar ou desafixar a aba",
    "Abrir Terminal",
    "Abrir Arquivos",
    "Abrir Alterações",
    "Abrir Navegador",
    "Abrir Chat lateral",
    "Abrir Simulador",
    "Trocar modelo e esforço",
    "Ver uso do provedor",
    "Trocar de projeto",
    "Abrir Uso",
    "Abrir Pull Requests",
    "Copiar link do prompt",
];

/// One line of the palette and what choosing it does.
#[derive(Clone)]
enum PaletteHit {
    Chat(usize),
    Message(usize, String),
    Command(usize),
    File(String, String),
    Project(usize),
}

/// The composer's text: a caret and, while selecting, the other end of the selection (bytes).
#[derive(Default, Clone)]
struct Draft {
    text: String,
    cursor: usize,
    anchor: usize,
}

impl Draft {
    fn selection(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }
    fn insert(&mut self, text: &str) {
        let range = self.selection();
        self.text.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        self.anchor = self.cursor;
    }
    fn previous(&self, at: usize) -> usize {
        self.text[..at]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }
    fn next(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8())
    }
    fn word_left(&self, at: usize) -> usize {
        let before = self.text[..at].trim_end();
        before.rfind(char::is_whitespace).map_or(0, |i| i + 1)
    }
    fn word_right(&self, at: usize) -> usize {
        let after = &self.text[at..];
        let skipped = after.len() - after.trim_start().len();
        after[skipped..]
            .find(char::is_whitespace)
            .map_or(self.text.len(), |i| at + skipped + i)
    }
    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |i| i + 1)
    }
    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |i| at + i)
    }
}

/// Whether a conversation's composer holds text that was not sent: its tab is dashed.
fn tab_has_draft(drafts: &HashMap<usize, Draft>, conversation: usize) -> bool {
    drafts
        .get(&conversation)
        .is_some_and(|d| !d.text.trim().is_empty())
}

/// The host's key for a conversation's composer draft, as Sirus Code's `session:<id>`.
fn draft_key(session_id: &str) -> String {
    format!("session:{session_id}")
}

/// Drafts are saved once the composer has been quiet this long.
const DRAFT_SAVE_QUIET: Duration = Duration::from_millis(400);

/// The time left before drafts edited at `edited` are saved, or `None` once the composer is quiet.
fn draft_save_delay(edited: Instant, now: Instant) -> Option<Duration> {
    let quiet = now.saturating_duration_since(edited);
    (quiet < DRAFT_SAVE_QUIET).then(|| DRAFT_SAVE_QUIET - quiet)
}

/// A draft restored from the host, its caret at the end.
fn restored_draft(text: &str) -> Draft {
    Draft {
        text: text.to_owned(),
        cursor: text.len(),
        anchor: text.len(),
    }
}

/// Fills each conversation's empty composer from the host's drafts. `sessions` lists the session
/// id of each conversation by index; text already typed in a composer is kept.
fn restore_drafts(
    drafts: &mut HashMap<usize, Draft>,
    sessions: &[String],
    saved: &HashMap<String, String>,
) {
    for (ix, session) in sessions.iter().enumerate() {
        let Some(text) = saved.get(&draft_key(session)) else {
            continue;
        };
        if drafts.get(&ix).is_none_or(|d| d.text.is_empty()) {
            drafts.insert(ix, restored_draft(text));
        }
    }
}

/// The tabs a bulk close marks: every tab but `keep` that is not pinned and that `closes` picks,
/// given each tab's index and the index of `keep`. Pinned tabs survive, as in browsers.
fn bulk_close_marks(
    tabs: &[(u64, bool)],
    keep: u64,
    closes: impl Fn(usize, usize) -> bool,
) -> Vec<u64> {
    let at = tabs.iter().position(|(id, _)| *id == keep).unwrap_or(0);
    tabs.iter()
        .enumerate()
        .filter(|(ix, (id, pinned))| *id != keep && !pinned && closes(*ix, at))
        .map(|(_, (id, _))| *id)
        .collect()
}

/// Where the selection goes when the tab at `at` of `count` closes: the one to its right, else
/// the one to its left, as in a browser. None when it was the only one.
fn neighbour_on_close(count: usize, at: usize) -> Option<usize> {
    if at + 1 < count {
        Some(at + 1)
    } else {
        at.checked_sub(1)
    }
}

/// A random version 4 UUID, one per send.
fn uuid() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut random) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = random.read_exact(&mut bytes);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// What a composer menu lists.
#[derive(Clone, Copy, PartialEq)]
enum ComposerMenu {
    Access,
    /// Effort and Fast for the current model.
    Effort,
    /// Every provider's models.
    Catalog,
}

#[derive(Clone)]
struct ModelInfo {
    id: String,
    name: String,
    efforts: Vec<String>,
    default_effort: Option<String>,
    fast: bool,
    default_fast: bool,
    available: bool,
    parameterized: bool,
}

/// The project popup under the sidebar's switcher and the sub-menu beside it.
#[derive(Clone, Copy, PartialEq)]
enum ProjectSub {
    Checkouts,
    Projects,
    Settings,
}

struct ProjectMenu {
    open: Tween,
    sub: Option<ProjectSub>,
    sub_at: Instant,
    /// The Checkouts or Projects search, or the project's name while editing it.
    query: String,
    selected: usize,
    /// Which settings row is unfolded: "move" or "editor".
    expanded: Option<&'static str>,
}

/// The usage popup's accounts page, after Sirus Code's `ProviderAccountsPanel`.
#[derive(Default)]
struct AccountsPage {
    /// The provider whose accounts these are.
    provider: &'static str,
    /// Set when the page floats over the Limites tab, which fades it in.
    shown: Option<Tween>,
    /// Who each account is signed in as, from its own usage.
    who: HashMap<String, String>,
    /// The name being typed for a new account.
    adding: Option<String>,
    signing_in: Option<String>,
    problem: Option<String>,
}

fn move_project_to_folder(settings: &mut Value, project: &str, folder: Option<&str>) {
    for entry in settings["projectFolders"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        let on = folder == entry["id"].as_str();
        if let Some(ids) = entry["projectIds"].as_array_mut() {
            ids.retain(|id| id.as_str() != Some(project));
            if on {
                ids.push(json!(project));
            }
        }
    }
}

/// The checkout a conversation runs in: its worktree, or the project's own folder.
fn checkout_of(conversation: &Conversation, project_path: &str) -> String {
    if conversation.worktree.is_empty() {
        project_path.to_owned()
    } else {
        conversation.worktree.clone()
    }
}

fn tilde_parent(path: &str) -> String {
    let parent = std::path::Path::new(path)
        .parent()
        .map_or(String::new(), |p| p.display().to_string());
    match std::env::var("HOME") {
        Ok(home) if parent.starts_with(&home) => format!("~{}", &parent[home.len()..]),
        _ => parent,
    }
}

const HEADING: f32 = 28.;
const DATE_GROUPS: [&str; 5] = [
    "Hoje",
    "Ontem",
    "Últimos 7 dias",
    "Últimos 30 dias",
    "Mais antigas",
];

/// What the sidebar's funnel shows.
#[derive(Default)]
struct SessionFilter {
    open: Option<Tween>,
    /// "working", "approval", "done": none chosen shows all.
    status: Vec<&'static str>,
    /// Days back; 0 is all time.
    days: u32,
    hidden_providers: Vec<&'static str>,
}

impl SessionFilter {
    fn active(&self) -> bool {
        !self.status.is_empty() || self.days != 0 || !self.hidden_providers.is_empty()
    }
}

/// Whole days since midnight today for a conversation's last activity; today is 0.
fn days_ago(conversation: &Conversation) -> i64 {
    let Ok(when) = chrono::DateTime::parse_from_rfc3339(&conversation.last_activity) else {
        return 9999;
    };
    let (when, today) = (
        when.with_timezone(&chrono::Local).date_naive(),
        chrono::Local::now().date_naive(),
    );
    (today - when).num_days().max(0)
}

fn date_group(days: i64) -> u8 {
    match days {
        0 => 0,
        1 => 1,
        2..=6 => 2,
        7..=29 => 3,
        _ => 4,
    }
}

/// The pages of the composer's + menu.
#[derive(Clone, Copy, PartialEq)]
enum AddPage {
    Add,
    Goal,
    Skills,
}

/// The popup every deletion passes through: what it does runs only on Confirm.
struct ConfirmDialog {
    open: Tween,
    title: String,
    message: String,
    action_label: String,
    action: Box<dyn FnOnce(&mut Shell, &mut Context<Shell>)>,
}

/// A right click's menu over a conversation or a project.
#[derive(Clone, Copy, PartialEq)]
enum MenuTarget {
    Session(usize),
    Project(usize),
}

struct ContextMenu {
    target: MenuTarget,
    x: f32,
    y: f32,
    open: Tween,
    renaming: Option<String>,
}

const EFFORT_ORDER: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];
const PROVIDER_ORDER: [&str; 10] = [
    "claude",
    "codex",
    "cursor",
    "grok",
    "opencode",
    "antigravity",
    "droid",
    "pi",
    "devin",
    "hermes",
];

/// Effort and Fast for one model, as the picker shows them.
struct Execution {
    levels: Vec<String>,
    effort: Option<String>,
    fast: bool,
    fast_ok: bool,
}

impl Execution {
    /// Where the current level sits on the bar, 0 to 1.
    fn share(&self) -> f32 {
        let at = self
            .effort
            .as_ref()
            .and_then(|e| self.levels.iter().position(|l| l == e))
            .unwrap_or(0);
        if self.levels.len() > 1 {
            at as f32 / (self.levels.len() - 1) as f32
        } else {
            1.
        }
    }
}

struct ModelRow {
    provider: &'static str,
    id: String,
    name: String,
    favorite: bool,
    selected: bool,
}

/// A model's line and version: "Opus 5.5" and "Opus 4.6" share a line, "GPT-5.4-mini" has its own.
fn model_line(name: &str) -> Option<(String, Vec<u32>)> {
    let start = name.find(|c: char| c.is_ascii_digit())?;
    let rest = &name[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    let version = rest[..end]
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect();
    Some((
        format!("{}#{}", &name[..start], &rest[end..]).to_lowercase(),
        version,
    ))
}

/// What a Cursor model id offers: its stem, effort and Fast ("composer-high-fast").
fn cursor_preset(id: &str) -> (String, Option<&'static str>, bool) {
    let (rest, fast) = id.strip_suffix("-fast").map_or((id, false), |r| (r, true));
    let rest = rest.strip_suffix("-thinking").unwrap_or(rest);
    for effort in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
        if let Some(stem) = rest.strip_suffix(&format!("-{effort}")) {
            return (stem.to_owned(), Some(effort), fast);
        }
    }
    (rest.to_owned(), None, fast)
}

/// A sidebar title being renamed: the caret follows the text, and a long text keeps its end
/// (and the caret) in view, clipped at the start with no ellipsis.
fn inline_name_field(text: &str) -> AnyElement {
    let caret = div()
        .flex_shrink_0()
        .w(px(crate::theme::CARET_WIDTH))
        .h(px(15.))
        .rounded_full()
        .bg(crate::theme::caret_color());
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .overflow_hidden()
        .text_size(px(13.))
        .when(text.is_empty(), |d| d.justify_start())
        .when(!text.is_empty(), |d| d.justify_end())
        .child(
            div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .text_color(content(0.95))
                .child(text.to_owned()),
        )
        .child(caret)
        .into_any_element()
}

/// A field's text with the caret after it, or the caret before a placeholder.
fn typed_text(text: &str, placeholder: &str, caret: Div) -> Div {
    let line = div().flex_1().min_w_0().flex().items_center().gap(px(1.));
    if text.is_empty() {
        line.child(caret).child(
            div()
                .min_w_0()
                .truncate()
                .text_color(content(0.4))
                .child(placeholder.to_owned()),
        )
    } else {
        line.child(
            div()
                .min_w_0()
                .truncate()
                .text_color(content(0.95))
                .child(text.to_owned()),
        )
        .child(caret)
    }
}

fn provider_title(provider: &str) -> &'static str {
    match provider {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "cursor" => "Cursor",
        "grok" => "Grok",
        "opencode" => "OpenCode",
        "antigravity" => "Antigravity",
        "droid" => "Droid",
        "pi" => "Pi",
        "devin" => "Devin",
        "hermes" => "Hermes",
        _ => t("Agente"),
    }
}

/// A usage window's name, as Sirus Code's `usageWindowLabel`.
fn usage_window_label(id: &str, minutes: Option<u64>) -> String {
    let known = match id {
        "five_hour" | "opencode-go/rolling" => t("Limite de 5 horas"),
        "seven_day" | "opencode-go/weekly" => t("Limite semanal"),
        "seven_day_opus" => t("Semanal · Opus"),
        "seven_day_sonnet" => t("Semanal · Sonnet"),
        "seven_day_oauth_apps" => t("Semanal · apps OAuth"),
        "opencode-go/monthly" => t("Limite mensal"),
        "cursor/models" => t("Modelos do Cursor"),
        "cursor/other-models" => t("Outros modelos"),
        "cursor/included" => t("Uso incluído"),
        "cursor/member" => t("Limite individual"),
        "cursor/team" => t("Conjunto da equipe"),
        _ => "",
    };
    if !known.is_empty() {
        return known.to_owned();
    }
    let reserve = id.starts_with("base_model_inference/");
    match (minutes, reserve) {
        (Some(300), true) => t("5 horas · modelo reserva").to_owned(),
        (_, true) => t("Semanal · modelo reserva").to_owned(),
        (Some(300), _) => t("Limite de 5 horas").to_owned(),
        (Some(10_080), _) => t("Limite semanal").to_owned(),
        _ => id.to_owned(),
    }
}

fn usage_bar_color(used: f64) -> gpui::Hsla {
    if used >= 90. {
        gpui::rgb(0xe5675f).into()
    } else if used >= 60. {
        gpui::rgb(0xe9b44c).into()
    } else {
        content(0.62)
    }
}

/// Time until a reset given in milliseconds: "2d 20h", "4h 44m", "33m".
fn reset_left(at: i64) -> String {
    let minutes = ((at - chrono::Utc::now().timestamp_millis()) as f64 / 60_000.)
        .ceil()
        .max(0.) as i64;
    if minutes >= 1440 {
        format!("{}d {}h", minutes / 1440, minutes % 1440 / 60)
    } else if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

fn provider_path(provider: &str) -> &'static str {
    match provider {
        "claude" => "claude.svg",
        "codex" => "codex-glyph.svg",
        "cursor" => "cursor",
        "grok" => "grok",
        "antigravity" => "antigravity",
        "droid" => "droid",
        "pi" => "pi",
        "devin" => "devin",
        "hermes" => "hermes",
        _ => "opencode.svg",
    }
}

/// A provider's mark in its own colours, as Monocode's `provider_icon`: Codex's is a gradient, so
/// it is an image; the rest are one colour.
fn provider_mark(provider: &str, side: f32) -> AnyElement {
    if provider == "codex" {
        return gpui::img("codex.svg")
            .size(px(side))
            .flex_shrink_0()
            .into_any_element();
    }
    svg()
        .path(provider_path(provider))
        .size(px(side))
        .flex_shrink_0()
        .text_color(gpui::Hsla::from(gpui::rgb(provider_brand(provider))))
        .into_any_element()
}

fn provider_brand(provider: &str) -> u32 {
    match provider {
        "claude" => 0xd87957,
        "cursor" => 0xb3a0f2,
        _ => 0xf2f2f4,
    }
}

/// The color the effort bar heats from, as Sirus Code's `resolveModelBrand`.
fn model_brand(model: &str, provider: &str) -> u32 {
    let id = model.to_lowercase();
    if provider == "claude"
        || id.contains("claude")
        || id.contains("opus")
        || id.contains("sonnet")
        || id.contains("haiku")
    {
        0xd87957
    } else if provider == "codex" || id.contains("gpt") {
        0x3b82f6
    } else if id.contains("gemini") {
        0x8bb8ff
    } else if provider == "cursor" {
        0xb3a0f2
    } else {
        0xa2a2ab
    }
}

/// What `/` or `@` is completing at the caret.
struct Mention {
    start: usize,
    hits: Vec<(String, String)>,
}

fn effort_label(effort: &str) -> String {
    match effort {
        "none" => t("Sem esforço"),
        "minimal" => t("Mínimo"),
        "low" => t("Baixo"),
        "medium" => t("Médio"),
        "high" => t("Alto"),
        "xhigh" => t("Extra alto"),
        "max" => t("Máximo"),
        "ultra" => "Ultra",
        other => other,
    }
    .to_owned()
}

/// Access by risk: asking is neutral, automatic review blue, full access orange, on any theme.
fn access_look(mode: &str) -> (&'static str, &'static str, &'static str, gpui::Hsla) {
    match mode {
        "ask" => (
            "hand",
            t("Pedir aprovação"),
            t("Pergunta antes de agir."),
            content(0.6),
        ),
        "auto" => (
            "bot",
            t("Revisão automática"),
            t("Aprova o que for seguro."),
            rgb(0x74a4f7).into(),
        ),
        _ => (
            "zap",
            t("Acesso total"),
            t("Age sem perguntar."),
            rgb(0xe8894a).into(),
        ),
    }
}

/// `text` with its `/` and `@` tokens in the accent color.
fn styled_tokens(text: String) -> StyledText {
    let tokens = data::token_ranges(&text);
    let found = crate::markdown::search_ranges(&text);
    let mut cuts = vec![0, text.len()];
    for range in tokens.iter().chain(found.iter().map(|(range, _)| range)) {
        cuts.push(range.start);
        cuts.push(range.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let highlights: Vec<_> = cuts
        .windows(2)
        .filter_map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let inside = |range: &std::ops::Range<usize>| range.start <= a && b <= range.end;
            let token = tokens.iter().any(inside);
            let fill = found
                .iter()
                .find(|(range, _)| inside(range))
                .map(|(_, fill)| *fill);
            (token || fill.is_some()).then(|| {
                (
                    a..b,
                    gpui::HighlightStyle {
                        color: token.then(accent),
                        background_color: fill,
                        ..Default::default()
                    },
                )
            })
        })
        .collect();
    StyledText::new(text).with_highlights(highlights)
}

/// What a reply from the back end is for.
enum Waiting {
    /// A drawn-on picture coming back from the host, and the draft it replaces.
    MarkedUp(usize, String),
    SettingsRestored,
    Older(usize),
    Models(&'static str),
    Worktrees(String),
    ProjectAdded,
    AccountUsage(String),
    AccountCreated(&'static str),
    AccountLogin(&'static str, String),
    /// `move_session_account` of a failed conversation: its session and approval, to continue after.
    AccountMoved(String, String),
    /// `send_prompt` that carries on after a failure.
    FailureContinue,
    Skills,
    Secrets,
    Agents,
    Branches,
    /// `create_branch` from the Ambiente card's name field.
    BranchCreate,
    /// `git_incoming` for a conversation generation (git_sync.rs).
    Incoming(u64),
    /// `git_pull_ff` for a conversation generation (git_sync.rs).
    IncomingPull(u64),
    Servers,
    Editors,
    Icons,
    Attached(usize),
    Dictated,
    SideChat,
    SettingsData(&'static str),
    Usage,
    CliAgents,
    CliProbe(String),
    CliLogin(String),
    ScriptsSaved(usize),
    ScriptAction,
    Transcript(usize),
    /// `transcript_action` search for the palette's text (global_search.rs).
    PaletteSearch(String),
    /// `detect_agents` for the first run's agents step (first_run.rs).
    FirstRunAgents,
    Created,
    /// `resolve_prompt_link` for a `sirus://` link (prompt_link.rs).
    PromptLink,
    /// `build_prompt_link` for the palette's "Copiar link do prompt".
    PromptLinkCopy,
    /// The conversation a prompt link asked for, and the prompt that goes in its composer.
    LinkCreated(String),
    Session(usize),
    /// A queued request sent to the host: its session and queue item.
    Queued(String, String),
    Steered(composer_queue::SteerWait),
    /// `/compact` sent for a conversation, and the message that goes out behind it.
    Compacted(usize, Option<Box<composer_queue::Outgoing>>),
    /// An approval declined before a redirect (its redirect id).
    Declined(u64),
    /// A subagent's message, stop or feed (subagent_card.rs).
    Subagent(subagent_card::Call),
}

/// A `git` result from a background thread.
enum GitResult {
    Files(String, Vec<(String, String)>),
    Diff(String, (u32, u32)),
}

struct PaletteState {
    /// The top bar's lupa: this project's files, `>` for commands. Otherwise ⌘K's palette.
    files: bool,
    /// ⌘⇧F: only message results, from every conversation (global_search.rs).
    all: bool,
    search: global_search::Search,
    query: String,
    selected: usize,
    open: Tween,
}

/// A collapsed sidebar sliding out over the workspace (Monocode's sidebar_peek.rs).
struct Peek {
    open: bool,
    t: Tween,
}

fn look_file() -> std::path::PathBuf {
    crate::backend::data_dir().join("native-look.json")
}
/// Aparência from disk; without a saved one, the default.
fn load_look() -> Look {
    std::fs::read(look_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .map_or_else(Look::default, |value| crate::theme::look_from_json(&value))
}
/// The accent the phone wears, kept by the host (`phone_look_set`).
fn phone_look_args(look: &Look) -> Value {
    json!({ "accent": crate::theme::accent_hex(look) })
}
fn save_look(look: &Look) {
    let bytes = crate::theme::look_to_json(look).to_string();
    let _ = std::fs::write(look_file(), bytes);
}

const PEEK_EDGE: f32 = 6.;
const PEEK_INSET: f32 = 6.;
const PEEK_DRIFT: f32 = 12.;
const PEEK_CLOSE: Duration = Duration::from_millis(180);
/// Where the sidebar menu's switches start numbering, clear of Settings' own.
const SIDEBAR_SWITCH_ID: usize = 800_000;
/// Settings, same pages in the same order as Sirus's SettingsSidebar.
const SETTINGS_NAV: [(&str, &[(&str, &str)]); 3] = [
    (
        "Pessoal",
        &[
            ("tuning", "Geral"),
            ("message-square", "Comportamento do chat"),
            ("monitor", "Aparência"),
            ("bell", "Notificações"),
            ("keyboard", "Atalhos"),
        ],
    ),
    (
        "Programação",
        &[
            ("puzzle", "Provedores"),
            ("box", "Agent skills"),
            ("plug", "Conectores"),
            ("pointer", "Computador"),
            ("branch", "Git"),
            ("folder-git", "Worktrees"),
            ("terminal-square", "Terminal"),
        ],
    ),
    (
        "Sistema",
        &[("smartphone", "Conexões"), ("sliders", "Avançado")],
    ),
];
const APPEARANCE: &str = "Aparência";
const TAB_MAX: f32 = 224.;
const TAB_MIN: f32 = 64.;
const SELECTED_MIN: f32 = 112.;
/// The sidebar's width when it opens, and the narrowest it can get.
const SIDEBAR_MIN: f32 = 196.;
const SIDEBAR_MAX: f32 = 420.;
const CONV_ROW: f32 = 29.;
const STEP_ROW: f32 = 28.;

struct Tab {
    id: u64,
    conversation: usize,
    pinned: bool,
    closing: bool,
    x: Tween,
    w: Tween,
}

struct TabDrag {
    id: u64,
    origin: f32,
    start: f32,
    x: f32,
    active: bool,
}

struct Menu {
    tab: u64,
    at: Point<Pixels>,
    open: Tween,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Row {
    Heading(u8),
    Conversation(usize),
}

struct RowMotion {
    y: Tween,
    alpha: Tween,
}

pub struct Shell {
    focus: FocusHandle,
    projects: Vec<Project>,
    conversations: Vec<Conversation>,
    project_order: Vec<usize>,
    tabs: Vec<Tab>,
    active: u64,
    next_id: u64,
    new_tab_x: Tween,
    tray: Tween,
    backend: Backend,
    settings: Value,
    provider_accounts: Value,
    selected_accounts: Value,
    new_tab_fx: crate::new_tab_field::State,
    /// The composer's width, as the robot walking over it last measured it.
    runner_width: Rc<Cell<f32>>,
    dock_width: f32,
    resizing_dock: bool,
    /// The add menu: main list, or its goal page.
    add_menu: Option<(AddPage, Tween)>,
    add_query: String,
    goal_input: Option<String>,
    visual_views: std::cell::RefCell<HashMap<String, gpui::AnyView>>,
    native_tx: async_channel::Sender<(&'static str, Value)>,
    agent_versions: HashMap<&'static str, String>,
    /// Which row of Ambiente is open.
    env_open: Option<&'static str>,
    env_branches: Vec<String>,
    /// The name typed under "Criar branch…", when that row is open.
    branch_draft: Option<String>,
    branch_error: Option<String>,
    branch_busy: bool,
    /// The header's Puxar button and its popover (git_sync.rs).
    incoming: IncomingState,
    env_servers: Vec<String>,
    env_editors: Vec<(String, String)>,
    env_icons: HashMap<String, std::path::PathBuf>,
    commit_message: Option<String>,
    /// The Settings text field being typed in: its key and text.
    setting_edit: Option<(&'static str, String)>,
    /// Each side panel, per conversation; they keep their state while hidden.
    pane_views: HashMap<(usize, String), gpui::AnyView>,
    settings_data: HashMap<&'static str, Value>,
    /// The project's scripts while its settings popup edits them (project_scripts.rs).
    scripts_edit: Option<crate::project_scripts::Edit>,
    /// Whether the notice above the composer shows the latest run's output.
    scripts_output: bool,
    waiting: HashMap<u64, Waiting>,
    drafts: HashMap<usize, Draft>,
    /// A `sirus://` link heard before the host was connected (prompt_link.rs).
    link_pending: Option<String>,
    /// The draft text the host holds per draft key, so only changes are saved.
    saved_drafts: HashMap<String, String>,
    /// When a draft was last edited and not yet saved; the save timer waits for the composer to go quiet.
    draft_edited: Option<Instant>,
    /// Whether the save timer is running.
    draft_timer: bool,
    /// Messages waiting for the agent to be free, per session (composer_queue.rs).
    queues: composer_queue::Queues,
    /// The subagent card's folds, message input and open feed (subagent_card.rs).
    subagents: subagent_card::State,
    /// The footer's agent gauge and its panel (agent_resources.rs).
    resources: agent_resources::State,
    /// The Usage page, opened from the footer or the palette (usage_page.rs).
    usage_page: usage_page::State,
    /// The context ring's popup and its compaction state (context_meter.rs).
    context: context_meter::State,
    /// The sidebar's hover card for a conversation row (sidebar_card.rs).
    sidebar_card: sidebar_card::State,
    /// Cards for other conversations that finished, failed or wait for the person.
    cards: activity_cards::State,
    /// The accent last sent to the host for the phone, `#rrggbb`.
    phone_accent: String,
    /// The ⌘F find bar and its hits (transcript_find.rs).
    find: transcript_find::State,
    /// The text selected in a reply, and its menu (transcript_select.rs).
    select: transcript_select::State,
    /// The copy check, undo's outcome and its request under a reply (message_actions.rs).
    message_actions: message_actions::State,
    /// The changed files under a reply, opened in place (turn_review.rs).
    turn_review: turn_review::State,
    /// Files attached to each conversation's next message: (id, name).
    attachments: HashMap<usize, Vec<(String, String)>>,
    /// The attachment chips' facts and the open attachment viewer (attachment_view.rs).
    viewer: attachment_view::Files,
    /// Ditado: since when it records.
    dictating: Option<Instant>,
    viewport_width: f32,
    dock_drawn: f32,
    ignored_text: String,
    composer_layout: std::cell::RefCell<Option<TextLayout>>,
    /// Groups of steps the person opened or closed: (conversation, turn, first step).
    phase_open: HashMap<(usize, usize, usize), bool>,
    /// When each prompt last folded or unfolded, and its bubble's width.
    prompt_moved: HashMap<(usize, usize), Instant>,
    prompt_widths: std::cell::RefCell<HashMap<(usize, usize), Rc<Cell<Pixels>>>>,
    /// When each group of steps last changed between open and closed, and those running now.
    phase_moved: std::cell::RefCell<HashMap<(usize, usize, usize), Instant>>,
    phase_live: std::cell::RefCell<HashSet<(usize, usize, usize)>>,
    /// The width of the conversation's column, for measuring what a fold reveals.
    turn_width: Rc<Cell<Pixels>>,
    /// Where the pointer met the message trail, for placing its preview.
    trail_x: f32,
    /// The composer's text scrolls under this, following the caret as it moves.
    composer_scroll: gpui::ScrollHandle,
    composer_caret_seen: Rc<Cell<(usize, usize)>>,
    selecting: bool,
    tick: u32,
    window_active: bool,
    composer_menu: Option<(ComposerMenu, Tween)>,
    models: HashMap<&'static str, Vec<ModelInfo>>,
    skills: Vec<(String, String)>,
    secrets: Vec<Value>,
    /// The secret being typed: its request and value, shown masked and sent once.
    secret_entry: Option<(String, String)>,
    mention_selected: usize,
    /// A token whose completion the person dismissed.
    mention_dismissed: Option<usize>,
    /// The provider switch animation: which logo, since when.
    switch_anim: Option<(&'static str, Instant)>,
    effort_bar: Tween,
    effort_drag: bool,
    effort_label_from: Option<(String, Instant)>,
    composer_menu_at: Instant,
    model_scope: String,
    model_query: String,
    model_selected: Option<usize>,
    model_glide: Tween,
    model_list_at: Instant,
    model_trigger: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<Pixels>>>>,
    composer_bounds: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<Pixels>>>>,
    /// Where the model menu opened: it stays there while its label changes width.
    model_menu_anchor: Option<(f32, f32)>,
    viewport_height: f32,
    usage: Option<(String, f32)>,
    usage_value: Value,
    session_filter: SessionFilter,
    filter_button_at: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<Pixels>>>>,
    /// The sidebar entries turned off (sidebar_items.rs), and the three-dots menu that turns
    /// them on or off: its fade, heading to 0 while it closes, and its button's bounds.
    sidebar_items: crate::sidebar_items::Hidden,
    sidebar_options: Option<Tween>,
    sidebar_options_at: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<Pixels>>>>,
    context_menu: Option<ContextMenu>,
    confirm: Option<ConfirmDialog>,
    /// Where the terminal lives: the strip, the title tabs (`shell_terminal.rs`).
    term: shell_terminal::TermState,
    lists: HashMap<usize, gpui::ListState>,
    /// Each conversation's follow of its end (glide.rs).
    glides: HashMap<usize, glide::Glide>,
    /// User messages opened past their four-line clamp, by (conversation, turn).
    expanded_prompts: HashSet<(usize, usize)>,
    /// Whether each user message wraps past four lines, measured in the list (prompt_lines).
    prompt_overflow: HashMap<(usize, usize), bool>,
    /// Each conversation's transcript size from the last layout, read outside the list's own
    /// layout (reading the list from inside its item processor panics: the list is borrowed).
    view_sizes: HashMap<usize, (f32, f32)>,
    /// A streaming reply's paced reveal, by conversation and turn.
    pacers: std::cell::RefCell<HashMap<(usize, usize), paced::Pacer>>,
    /// The live steps' bounded window of each turn, scrolled to follow the newest step.
    phase_scrolls:
        std::cell::RefCell<HashMap<(usize, usize), (gpui::ScrollHandle, std::cell::Cell<f32>)>>,
    /// The panes beside each other, when there are several (split_panes.rs).
    split: split_panes::State,
    jump: Tween,
    trail_hover: Option<(usize, f32)>,
    usage_accounts: Option<AccountsPage>,
    project_menu: Option<ProjectMenu>,
    project_button: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<Pixels>>>>,
    checkout_filter: HashMap<usize, String>,
    worktrees: HashMap<String, Vec<Value>>,
    usage_popup: Option<Tween>,
    handoffs: handoff::State,
    usage_refreshing: bool,
    usage_chip: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<Pixels>>>>,
    /// Each project keeps its own tabs; `tabs` are the current project's.
    parked: HashMap<usize, (Vec<Tab>, u64)>,
    current_project: Option<usize>,
    marked: Option<Range<usize>>,
    problem: Option<String>,
    connected: bool,
    files: (String, Vec<(String, String)>),
    diffs: HashMap<String, (u32, u32)>,
    git_tx: async_channel::Sender<GitResult>,
    palette_ui: Option<PaletteState>,
    environment: Option<Tween>,
    dock_open: bool,
    dock: Tween,
    dock_max: Tween,
    dock_panes: Vec<usize>,
    dock_active: Option<usize>,
    chooser_since: Instant,
    look: Look,
    /// The send button's morph (send_morph.rs) and whether it wants another frame.
    send_morph: std::cell::RefCell<crate::send_morph::Morph>,
    send_moving: std::cell::Cell<bool>,
    system_dark: bool,
    theme_menu: Option<PaletteState>,
    settings_page: &'static str,
    settings_since: Instant,
    settings_direction: f32,
    slider_drag: Option<usize>,
    slider_bounds: std::rc::Rc<std::cell::RefCell<[Option<gpui::Bounds<Pixels>>; 3]>>,
    peek: Option<Peek>,
    peek_close: Option<gpui::Task<()>>,
    palette_from: Palette,
    palette_t: Tween,
    appearance: Option<Tween>,
    fan: Tween,
    deck_hovered: bool,
    card_hovered: Option<u64>,
    deck_front: Option<u64>,
    shuffle: Option<(u64, Instant)>,
    history: Vec<u64>,
    history_at: usize,
    rolls: HashMap<(usize, usize), Tween>,
    drag: Option<TabDrag>,
    menu: Option<Menu>,
    sidebar_width: f32,
    sidebar_open: bool,
    sidebar: Tween,
    resizing: bool,
    rows: HashMap<Row, RowMotion>,
    switch: Tween,
    next_turn: usize,
    epoch: Instant,
    frames: VecDeque<Instant>,
    show_stats: bool,
    first_run: first_run::State,
    /// The computers of the window and the active one (computer_switch.rs).
    computers: computer_switch::State,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let now = Instant::now();
        let (projects, conversations) = (Vec::new(), vec![Conversation::placeholder()]);
        let (backend, incoming) = backend::start();
        crate::browser_bridge::start(backend.clone(), cx);
        // A device paired, connected or was removed: Settings → Conexões lists it without a reload.
        let remote_changed = backend.subscribe("remote-changed");
        cx.spawn(async move |this, cx| {
            while remote_changed.recv().await.is_ok() {
                let done = this.update(cx, |this, cx| {
                    if this.appearance.is_some() && this.settings_page == "Conexões" {
                        this.load_settings_page();
                        cx.notify();
                    }
                });
                if done.is_err() {
                    break;
                }
            }
        })
        .detach();
        let (git_tx, git_rx) = async_channel::unbounded();
        let (native_tx, native_rx) = async_channel::unbounded::<(&'static str, Value)>();
        cx.spawn(async move |this, cx| {
            while let Ok((key, value)) = native_rx.recv().await {
                let done = this.update(cx, |this, cx| {
                    if key == "computer_pair" {
                        this.computer_paired(&value, cx);
                        return;
                    }
                    if key == "computer_move" {
                        this.computer_moved(&value, cx);
                        return;
                    }
                    this.settings_data.insert(key, value);
                    cx.notify();
                });
                if done.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Ok(message) = incoming.recv().await {
                if this
                    .update(cx, |this, cx| this.receive(message, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Ok(result) = git_rx.recv().await {
                let done = this.update(cx, |this, cx| {
                    match result {
                        GitResult::Files(path, files) => this.files = (path, files),
                        GitResult::Diff(path, stat) => {
                            this.diffs.insert(path, stat);
                        }
                    }
                    cx.notify();
                });
                if done.is_err() {
                    break;
                }
            }
        })
        .detach();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.observe_window_activation(window, |_, _, cx| cx.notify())
            .detach();
        // Loops (spinners, shimmers, streaming text) tick together at 30 Hz, as Monocode's pulse.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    // An idle window draws nothing: the caret never blinks. Work animates at 15 Hz,
                    // slower out of focus.
                    this.tick = this.tick.wrapping_add(1);
                    let every = if this.window_active { 2 } else { 12 };
                    let pulse = this.tick % every == 0
                        && this.conversations.iter().any(Conversation::working);
                    if pulse {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        // Updates (app_update.rs): the first check once the window has settled, then every few hours.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(crate::app_update::START_DELAY)
                .await;
            loop {
                if this.update(cx, |this, _| this.app_update_auto()).is_err() {
                    break;
                }
                cx.background_executor()
                    .timer(crate::app_update::CHECK_INTERVAL)
                    .await;
            }
        })
        .detach();
        // Sirus Code's incoming check: every 5 minutes while the window is visible. Answers only
        // redraw when they change (git_sync.rs), so an idle window stays still.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(git_sync::INCOMING_REFRESH)
                    .await;
                let alive = this.update(cx, |this, _| {
                    if this.incoming.due() {
                        this.incoming_fetch();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        let project_order = (0..projects.len()).collect();
        let mut shell = Self {
            focus,
            projects,
            conversations,
            project_order,
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            new_tab_x: Tween::still(0., now),
            tray: Tween::still(0., now),
            backend,
            settings: Value::Null,
            provider_accounts: Value::Null,
            selected_accounts: json!({}),
            new_tab_fx: Default::default(),
            runner_width: Rc::new(Cell::new(600.)),
            dock_width: 470.,
            resizing_dock: false,
            add_menu: None,
            add_query: String::new(),
            goal_input: None,
            visual_views: Default::default(),
            native_tx,
            agent_versions: HashMap::new(),
            env_open: None,
            env_branches: Vec::new(),
            branch_draft: None,
            branch_error: None,
            branch_busy: false,
            incoming: IncomingState::default(),
            env_servers: Vec::new(),
            env_editors: Vec::new(),
            env_icons: HashMap::new(),
            commit_message: None,
            setting_edit: None,
            pane_views: HashMap::new(),
            settings_data: HashMap::new(),
            scripts_edit: None,
            scripts_output: false,
            waiting: HashMap::new(),
            drafts: HashMap::new(),
            link_pending: None,
            saved_drafts: HashMap::new(),
            draft_edited: None,
            draft_timer: false,
            queues: composer_queue::Queues::default(),
            subagents: subagent_card::State::default(),
            resources: agent_resources::State::default(),
            usage_page: usage_page::State::default(),
            context: Default::default(),
            sidebar_card: Default::default(),
            cards: Default::default(),
            phone_accent: String::new(),
            message_actions: Default::default(),
            turn_review: Default::default(),
            find: Default::default(),
            select: Default::default(),
            attachments: HashMap::new(),
            viewer: Default::default(),
            dictating: None,
            viewport_width: 0.,
            dock_drawn: 0.,
            ignored_text: String::new(),
            composer_layout: Default::default(),
            phase_open: HashMap::new(),
            prompt_moved: HashMap::new(),
            prompt_widths: Default::default(),
            phase_moved: Default::default(),
            phase_live: Default::default(),
            turn_width: Rc::new(Cell::new(px(0.))),
            trail_x: 0.,
            composer_scroll: gpui::ScrollHandle::new(),
            composer_caret_seen: Default::default(),
            selecting: false,
            tick: 0,
            window_active: true,
            composer_menu: None,
            models: HashMap::new(),
            skills: Vec::new(),
            secrets: Vec::new(),
            secret_entry: None,
            mention_selected: 0,
            mention_dismissed: None,
            switch_anim: None,
            effort_bar: Tween::still(0., now),
            effort_drag: false,
            effort_label_from: None,
            composer_menu_at: now,
            model_scope: String::new(),
            model_query: String::new(),
            model_selected: None,
            model_glide: Tween::still(0., now),
            model_list_at: now,
            model_trigger: Default::default(),
            composer_bounds: Default::default(),
            model_menu_anchor: None,
            viewport_height: 0.,
            usage: None,
            usage_value: Value::Null,
            session_filter: SessionFilter::default(),
            filter_button_at: Default::default(),
            sidebar_items: crate::sidebar_items::load(),
            sidebar_options: None,
            sidebar_options_at: Default::default(),
            context_menu: None,
            confirm: None,
            term: shell_terminal::TermState::new(),
            lists: HashMap::new(),
            glides: HashMap::new(),
            expanded_prompts: HashSet::new(),
            prompt_overflow: HashMap::new(),
            view_sizes: HashMap::new(),
            pacers: Default::default(),
            phase_scrolls: Default::default(),
            jump: Tween::still(0., now),
            trail_hover: None,
            usage_accounts: None,
            project_menu: None,
            project_button: Default::default(),
            checkout_filter: HashMap::new(),
            worktrees: HashMap::new(),
            usage_popup: None,
            handoffs: handoff::State::default(),
            split: Default::default(),
            usage_refreshing: false,
            usage_chip: Default::default(),
            parked: HashMap::new(),
            current_project: None,
            marked: None,
            problem: None,
            connected: false,
            files: (String::new(), Vec::new()),
            diffs: HashMap::new(),
            git_tx,
            palette_ui: None,
            environment: None,
            dock_open: false,
            dock: Tween::still(0., now),
            dock_max: Tween::still(0., now),
            dock_panes: Vec::new(),
            dock_active: None,
            chooser_since: now,
            look: load_look(),
            send_morph: Default::default(),
            send_moving: Default::default(),
            system_dark: true,
            theme_menu: None,
            settings_page: APPEARANCE,
            settings_since: now,
            settings_direction: 1.,
            slider_drag: None,
            slider_bounds: Default::default(),
            peek: None,
            peek_close: None,
            palette_from: palette(&Look::default(), true),
            palette_t: Tween::still(1., now),
            appearance: None,
            fan: Tween::still(1., now),
            deck_hovered: false,
            card_hovered: None,
            deck_front: None,
            shuffle: None,
            history: Vec::new(),
            history_at: 0,
            rolls: HashMap::new(),
            drag: None,
            menu: None,
            sidebar_width: SIDEBAR_MIN,
            sidebar_open: true,
            sidebar: Tween::still(1., now),
            resizing: false,
            rows: HashMap::new(),
            switch: Tween::still(1., now),
            next_turn: 0,
            epoch: now,
            frames: VecDeque::new(),
            show_stats: false,
            first_run: Default::default(),
            computers: computer_switch::State::new(),
        };
        shell.tabs.push(Tab {
            id: 1,
            conversation: 0,
            pinned: false,
            closing: false,
            x: Tween::still(-1., now),
            w: Tween::still(-1., now),
        });
        shell.next_id = 2;
        shell.active = 1;
        shell.history.push(1);
        // main.rs stops the host in the same quit pass, so the drafts reach it first: the replies
        // are awaited here, while this handler runs before any quit future is polled.
        cx.on_app_quit(|this, _| {
            let deadline = Instant::now() + Duration::from_millis(500);
            for reply in this.save_drafts() {
                loop {
                    match reply.try_recv() {
                        Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        _ => break,
                    }
                }
            }
            std::future::ready(())
        })
        .detach();
        shell
    }

    // The back end.

    fn receive(&mut self, message: Incoming, cx: &mut Context<Self>) {
        match message {
            Incoming::Problem(problem) => {
                if !is_not_a_repository(&problem) {
                    self.problem = Some(problem);
                }
            }
            Incoming::Ready(state) => {
                self.load_state(state, cx);
                self.computer_arrived(cx);
            }
            Incoming::Link { id, status } => self.computer_link(&id, status, cx),
            Incoming::Reply { id, ok, value } => match self.waiting.remove(&id) {
                Some(Waiting::CliAgents) if ok => self.cli_agents_arrived(value, cx),
                Some(Waiting::CliAgents) => self.problem = Some(error_text(&value)),
                Some(Waiting::CliProbe(id)) => {
                    let reply = if ok {
                        value
                    } else {
                        json!({ "error": error_text(&value) })
                    };
                    self.cli_reply("cliProbes", id, reply);
                }
                Some(Waiting::CliLogin(id)) => {
                    let reply = if ok {
                        value
                    } else {
                        json!({ "error": error_text(&value) })
                    };
                    self.cli_reply("cliLogins", id, reply);
                }
                Some(Waiting::ScriptsSaved(ix)) if ok => {
                    if let Some(project) = self.projects.get_mut(ix) {
                        project.scripts = crate::project_scripts::Saved::of(&value["scripts"]);
                    }
                }
                Some(Waiting::ScriptsSaved(_) | Waiting::ScriptAction) if !ok => {
                    self.problem = Some(error_text(&value));
                }
                Some(Waiting::ScriptsSaved(_) | Waiting::ScriptAction) => {}
                Some(Waiting::Older(ix)) if ok => {
                    let conversation = &mut self.conversations[ix];
                    let older = value["messages"].as_array().cloned().unwrap_or_default();
                    conversation.base = value["start"].as_u64().unwrap_or(0) as usize;
                    conversation.messages.splice(0..0, older);
                    let before = conversation.turns.len();
                    conversation.turns.clear();
                    conversation.rebuild();
                    let after = conversation.turns.len();
                    if let Some(state) = self.lists.get(&ix) {
                        // Older turns arrive above: what's on screen stays where it is.
                        state.splice(1..1, after.saturating_sub(before));
                    }
                }
                Some(Waiting::Models(provider)) if ok => {
                    let models = value["models"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|m| ModelInfo {
                            id: m["id"].as_str().unwrap_or("").to_owned(),
                            name: m["displayName"].as_str().unwrap_or("").to_owned(),
                            efforts: m["effortLevels"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|e| e.as_str().map(str::to_owned))
                                .collect(),
                            default_effort: m["defaultEffort"].as_str().map(str::to_owned),
                            fast: m["fastMode"].as_bool().unwrap_or(false),
                            default_fast: m["defaultFast"].as_bool().unwrap_or(false),
                            available: m["availability"].as_str().is_none_or(|a| a == "available"),
                            parameterized: m["parameterized"].as_bool().unwrap_or(false),
                        })
                        .collect();
                    let models = if provider == "claude" {
                        claude_catalog(models)
                    } else {
                        models
                    };
                    self.models.insert(provider, models);
                }
                Some(Waiting::Worktrees(project)) if ok => {
                    self.worktrees
                        .insert(project, value.as_array().cloned().unwrap_or_default());
                }
                Some(Waiting::ProjectAdded) if ok => {
                    self.projects.push(Project {
                        id: value["id"].as_str().unwrap_or("").to_owned(),
                        name: value["name"].as_str().unwrap_or("").to_owned(),
                        path: value["path"].as_str().unwrap_or("").to_owned(),
                        appearance: value["appearance"].clone(),
                        scripts: crate::project_scripts::Saved::of(&value["scripts"]),
                        open: true,
                        pinned: false,
                        conversations: Vec::new(),
                    });
                    let ix = self.projects.len() - 1;
                    self.project_order.insert(0, ix);
                    self.switch_project(ix, cx);
                }
                Some(Waiting::AccountUsage(id)) if ok => {
                    if let Some(page) = self.usage_accounts.as_mut() {
                        let who = value["account"]["email"]
                            .as_str()
                            .or(value["account"]["name"].as_str())
                            .unwrap_or("Sem login")
                            .to_owned();
                        page.who.insert(id, who);
                    }
                }
                Some(Waiting::AccountCreated(provider)) if ok => {
                    let id = value["id"].as_str().unwrap_or("").to_owned();
                    if let Some(list) = self.provider_accounts.as_array_mut() {
                        list.push(value.clone());
                    }
                    if let Some(page) = self.usage_accounts.as_mut() {
                        page.adding = None;
                    }
                    // The new account joins the page's list at once, still waiting for its sign-in.
                    if self.usage_page.open.is_some() {
                        self.usage_load_limits(false, cx);
                    }
                    self.sign_in_account(provider, id);
                }
                Some(Waiting::AccountLogin(provider, id)) => {
                    if let Some(page) = self.usage_accounts.as_mut() {
                        page.signing_in = None;
                        if !ok {
                            page.problem = Some(error_text(&value));
                        }
                    }
                    if ok {
                        self.select_account(provider, id);
                    }
                    self.load_account_usage(provider);
                    if self.usage_page.open.is_some() {
                        self.usage_load_limits(false, cx);
                    }
                }
                Some(Waiting::SettingsRestored) if ok => {
                    self.settings = value;
                    self.load_settings_page();
                }
                Some(Waiting::SettingsData(key)) if ok => {
                    if key == "connectors" {
                        crate::settings_connectors::finish();
                    }
                    if key == "remote" {
                        self.settings_data.remove("remote_error");
                    }
                    self.settings_data.insert(key, value);
                }
                Some(Waiting::SettingsData("connectors")) => {
                    crate::settings_connectors::finish();
                    self.problem = Some(error_text(&value));
                }
                Some(Waiting::SettingsData("remote")) => {
                    // Shown under the Remote access group, where the change was made.
                    self.settings_data
                        .insert("remote_error", json!(error_text(&value)));
                }
                Some(Waiting::Attached(_)) if !ok => {
                    self.problem = Some(
                        value["message"]
                            .as_str()
                            .or(value.as_str())
                            .unwrap_or(t("Não foi possível anexar."))
                            .to_owned(),
                    );
                }
                Some(Waiting::Attached(ix)) => {
                    self.problem = None;
                    self.viewer.note(&value);
                    let added = value.as_array().into_iter().flatten().map(|a| {
                        (
                            a["id"].as_str().unwrap_or("").to_owned(),
                            a["name"].as_str().unwrap_or("").to_owned(),
                        )
                    });
                    self.attachments.entry(ix).or_default().extend(added);
                }
                Some(Waiting::MarkedUp(..)) if !ok => {
                    self.viewer.note_markup_failed();
                    self.problem = Some(error_text(&value));
                }
                Some(Waiting::MarkedUp(ix, old)) => self.markup_replaced(ix, &old, &value, cx),
                Some(Waiting::AccountMoved(session, approval)) if ok => {
                    self.account_moved(&session, &approval)
                }
                Some(Waiting::AccountMoved(..) | Waiting::FailureContinue) if !ok => {
                    self.problem = Some(error_text(&value))
                }
                Some(Waiting::FailureContinue) => {}
                Some(Waiting::Dictated) if ok => {
                    if let Some(text) = value.as_str().filter(|t| !t.is_empty()) {
                        self.draft().insert(text);
                    }
                }
                Some(Waiting::SideChat) if ok && false => {
                    let ix = match self
                        .conversations
                        .iter()
                        .position(|c| Some(c.session_id.as_str()) == value["id"].as_str())
                    {
                        Some(ix) => ix,
                        None => self.add_session(&value),
                    };
                    self.ensure_loaded(ix);
                    let (project, id) = (self.current_project, self.next_id);
                    let _ = project;
                    if !self.tabs.iter().any(|t| t.conversation == ix && !t.closing) {
                        self.next_id += 1;
                        let now = Instant::now();
                        let x = self.new_tab_x.at(now).max(0.);
                        self.tabs.push(Tab {
                            id,
                            conversation: ix,
                            pinned: false,
                            closing: false,
                            x: Tween::still(x, now),
                            w: Tween::still(0., now),
                        });
                        self.select(id, cx);
                    }
                }
                Some(Waiting::Agents) if ok => {
                    for agent in value.as_array().into_iter().flatten() {
                        if let (Some(id), Some(version)) =
                            (agent["id"].as_str(), agent["version"].as_str())
                        {
                            let key = [
                                "claude",
                                "codex",
                                "cursor",
                                "opencode",
                                "grok",
                                "antigravity",
                                "droid",
                                "pi",
                                "devin",
                                "hermes",
                            ]
                            .into_iter()
                            .find(|k| *k == id);
                            if let Some(key) = key {
                                let short = version
                                    .split_whitespace()
                                    .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
                                    .unwrap_or(version);
                                self.agent_versions.insert(key, format!("v{short}"));
                                if !self.models.contains_key(key)
                                    && !["claude", "codex", "opencode"].contains(&key)
                                {
                                    let call = self
                                        .backend
                                        .invoke("list_provider_models", json!({ "id": key }));
                                    self.waiting.insert(call, Waiting::Models(key));
                                }
                            }
                        }
                    }
                    // A prompt link that started the app waited for the providers to be known.
                    self.take_prompt_link(cx);
                }
                Some(Waiting::Branches) if ok => {
                    self.env_branches = value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|b| b["name"].as_str().map(str::to_owned))
                        .collect();
                }
                Some(Waiting::Servers) if ok => {
                    self.env_servers = value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s["url"].as_str().map(str::to_owned))
                        .collect();
                }
                Some(Waiting::Editors) if ok => {
                    self.env_editors = value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|e| e["installed"].as_bool().unwrap_or(false))
                        .filter_map(|e| {
                            Some((e["id"].as_str()?.to_owned(), e["name"].as_str()?.to_owned()))
                        })
                        .collect();
                }
                Some(Waiting::Icons) if ok => {
                    for item in value.as_array().into_iter().flatten() {
                        let (Some(id), Some(png)) = (item["id"].as_str(), item["png"].as_str())
                        else {
                            continue;
                        };
                        if let Some(path) = backend::save_icon(id, png) {
                            self.env_icons.insert(id.to_owned(), path);
                        }
                    }
                }
                Some(Waiting::Skills) if ok => {
                    self.skills = value["catalog"]["skills"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|s| {
                            (
                                s["name"].as_str().unwrap_or("").to_owned(),
                                s["description"].as_str().unwrap_or("").to_owned(),
                            )
                        })
                        .collect();
                }
                Some(Waiting::Secrets) if ok => {
                    self.secrets = value["requests"].as_array().cloned().unwrap_or_default()
                }
                Some(Waiting::Usage) if !ok => self.usage_refreshing = false,
                Some(Waiting::Usage) => {
                    self.usage_warm(cx);
                    self.usage_refreshing = false;
                    self.usage_value = value.clone();
                    let parts: Vec<String> = value["windows"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|w| {
                            let used = w["usedPercent"].as_f64()?;
                            let left = w["resetsAt"].as_i64().map_or(String::new(), |at| {
                                let at = if at > 100_000_000_000 { at / 1000 } else { at };
                                let minutes = ((at - chrono::Utc::now().timestamp()) / 60).max(0);
                                if minutes >= 1440 {
                                    format!(" {}d {}h", minutes / 1440, minutes % 1440 / 60)
                                } else {
                                    format!(" {}h {}m", minutes / 60, minutes % 60)
                                }
                            });
                            Some(format!("{used:.0}%{left}"))
                        })
                        .collect();
                    let tightest = value["windows"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|w| w["usedPercent"].as_f64())
                        .fold(0., f64::max);
                    self.usage = (!parts.is_empty()).then(|| (parts.join(" · "), tightest as f32));
                }
                Some(Waiting::Transcript(ix)) if ok => {
                    let conversation = &mut self.conversations[ix];
                    conversation.messages =
                        value["messages"].as_array().cloned().unwrap_or_default();
                    conversation.base = value["start"].as_u64().unwrap_or(0) as usize;
                    conversation.loaded = true;
                    conversation.rebuild();
                    let count = 1 + conversation.turns.len();
                    let state = self.list_state(ix, cx);
                    state.reset(count);
                    state.set_follow_mode(gpui::FollowMode::Tail);
                    self.glides.remove(&ix);
                }
                Some(Waiting::FirstRunAgents) => self.first_run_agents_arrived(ok, &value, cx),
                Some(Waiting::PaletteSearch(needle)) => {
                    self.palette_search_arrived(needle, ok, &value, cx)
                }
                Some(Waiting::Created) if ok => {
                    let ix = self.add_session(&value);
                    self.conversations[ix].loaded = true;
                    self.open(ix, cx);
                }
                Some(Waiting::PromptLink) => self.prompt_link_resolved(ok, &value, cx),
                Some(Waiting::PromptLinkCopy) => self.prompt_link_copied(ok, &value, cx),
                Some(Waiting::LinkCreated(prompt)) if ok => self.link_created(prompt, &value, cx),
                Some(Waiting::Queued(session, item)) => {
                    self.queued_reply(&session, &item, ok, &value, cx)
                }
                Some(Waiting::Steered(wait)) => self.steered_reply(wait, ok, &value, cx),
                Some(Waiting::Compacted(ix, then)) => {
                    self.compacted_reply(ix, then, ok, &value, cx)
                }
                Some(Waiting::Declined(key)) => self.declined_reply(key, ok, &value, cx),
                Some(Waiting::Subagent(call)) => self.subagent_reply(call, ok, &value, cx),
                Some(Waiting::BranchCreate) => self.branch_created(ok, &value, cx),
                Some(Waiting::Incoming(generation)) => {
                    if self.incoming.answer(generation, ok, &value) {
                        cx.notify();
                    }
                }
                Some(Waiting::IncomingPull(generation)) => {
                    self.incoming_pulled(generation, ok, &value, cx)
                }
                Some(Waiting::Session(usize::MAX)) => {}
                Some(Waiting::Session(_)) if ok => self.session_updated(&value, cx),
                Some(_) | None if !ok => {
                    let message = error_text(&value);
                    if !is_not_a_repository(&message) {
                        self.problem = Some(message);
                    }
                }
                _ => {}
            },
            Incoming::Event { name, payload } => match name.as_str() {
                "session-updated" => {
                    self.session_updated(&payload, cx);
                    self.cards_settle();
                }
                "notification-activity" => self.activity_event(&payload, cx),
                "agent-output" => self.agent_output(&payload),
                "agent-exit" => self.refresh_diff(),
                // Monocode's chime for activity on something linked to a session.
                "pr-watch-activity" => {
                    if self.settings["notifications"]["sounds"].as_bool() != Some(false) {
                        crate::sound::play(crate::sound::Cue::LinkedActivity);
                    }
                }
                "notification-native" => {
                    crate::native_notify::deliver(&payload, self.window_active)
                }
                "subagent-feed" => {
                    self.subagents.feed_event(&payload);
                }
                "secret-state" => {
                    self.secrets = payload["requests"].as_array().cloned().unwrap_or_default()
                }
                _ => {}
            },
        }
        cx.notify();
    }

    fn load_state(&mut self, state: Value, cx: &mut Context<Self>) {
        let now = Instant::now();
        self.connected = true;
        self.problem = None;
        self.settings = state["settings"].clone();
        self.push_phone_look(true);
        // These two fix up this Mac's own settings; another computer's are left as they are.
        let remote = self.on_remote();
        let repaired = !remote && crate::settings_pages::repair_provider_paths(&mut self.settings);
        if !remote
            && (crate::chat_background::seed_monocode_default(&mut self.settings) || repaired)
        {
            self.backend
                .invoke("save_settings", json!({ "settings": self.settings }));
        }
        crate::panel_browser::note_settings(&self.settings);
        // The window keeps this Mac's language and look whichever computer it shows.
        if !remote {
            crate::i18n::set_locale(self.settings["locale"].as_str().unwrap_or("pt-BR"));
        }
        self.provider_accounts = state["providerAccounts"].clone();
        self.selected_accounts = if state["selectedProviderAccounts"].is_object() {
            state["selectedProviderAccounts"].clone()
        } else {
            json!({})
        };
        // Drafts are keyed by conversation index, which the rebuild renumbers: carry each by session.
        let sessions: Vec<String> = self
            .conversations
            .iter()
            .map(|c| c.session_id.clone())
            .collect();
        let mut by_session = Vec::new();
        let mut sessionless = Vec::new();
        for (ix, draft) in std::mem::take(&mut self.drafts) {
            match sessions.get(ix).filter(|s| !s.is_empty()) {
                Some(session) => by_session.push((session.clone(), draft)),
                None => sessionless.push((ix, draft)),
            }
        }
        self.conversations.clear();
        self.tabs.clear();
        self.rows.clear();
        self.lists.clear();
        self.glides.clear();
        self.pacers.borrow_mut().clear();
        self.expanded_prompts.clear();
        self.phase_scrolls.borrow_mut().clear();
        self.history.clear();
        self.history_at = 0;
        self.projects = state["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| Project {
                id: p["id"].as_str().unwrap_or("").to_owned(),
                name: p["name"].as_str().unwrap_or("").to_owned(),
                path: p["path"].as_str().unwrap_or("").to_owned(),
                appearance: p["appearance"].clone(),
                scripts: crate::project_scripts::Saved::of(&p["scripts"]),
                open: false,
                pinned: false,
                conversations: Vec::new(),
            })
            .collect();
        self.project_order = (0..self.projects.len()).collect();
        for session in state["sessions"].as_array().into_iter().flatten() {
            self.add_session(session);
        }
        // Newest first inside each project, and the projects most recently used first.
        let activity: Vec<String> = self
            .conversations
            .iter()
            .map(|c| c.last_activity.clone())
            .collect();
        for project in &mut self.projects {
            project
                .conversations
                .sort_by(|a, b| activity[*b].cmp(&activity[*a]));
        }
        let latest = |p: &Project| {
            p.conversations
                .first()
                .map_or(String::new(), |c| activity[*c].clone())
        };
        let projects = &self.projects;
        self.project_order
            .sort_by(|a, b| latest(&projects[*b]).cmp(&latest(&projects[*a])));
        // Side chats have no place in a project's list, so they never open on launch.
        let mut recent: Vec<usize> = (0..self.conversations.len())
            .filter(|ix| self.project_of(*ix).is_some())
            .collect();
        recent.sort_by(|a, b| activity[*b].cmp(&activity[*a]));
        recent.truncate(4);
        self.parked.clear();
        self.current_project = recent.first().and_then(|ix| self.project_of(*ix));
        for ix in &recent {
            let project = self.project_of(*ix);
            if let Some(project) = project {
                self.projects[project].open = true;
            }
            let id = self.next_id;
            self.next_id += 1;
            let tab = Tab {
                id,
                conversation: *ix,
                pinned: false,
                closing: false,
                x: Tween::still(-1., now),
                w: Tween::still(-1., now),
            };
            if project == self.current_project {
                self.tabs.push(tab);
            } else if let Some(project) = project {
                let parked = self.parked.entry(project).or_insert((Vec::new(), id));
                parked.0.push(tab);
            }
        }
        if self.conversations.is_empty() {
            self.conversations.push(Conversation::placeholder());
            self.conversations[0].title = t("Sem conversas").into();
            self.tabs.push(Tab {
                id: self.next_id,
                conversation: 0,
                pinned: false,
                closing: false,
                x: Tween::still(-1., now),
                w: Tween::still(-1., now),
            });
            self.next_id += 1;
        }
        for (session, draft) in by_session {
            if let Some(ix) = self
                .conversations
                .iter()
                .position(|c| c.session_id == session)
            {
                self.drafts.insert(ix, draft);
            }
        }
        for (ix, draft) in sessionless {
            self.drafts.entry(ix).or_insert(draft);
        }
        self.split_restore(cx);
        self.saved_drafts = state["composerDrafts"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
            .collect();
        let sessions: Vec<String> = self
            .conversations
            .iter()
            .map(|c| c.session_id.clone())
            .collect();
        restore_drafts(&mut self.drafts, &sessions, &self.saved_drafts);
        // Text typed while disconnected differs from the host's copy: it is saved now.
        self.save_drafts();
        self.active = self.tabs[0].id;
        self.history.push(self.active);
        for provider in ["claude", "codex", "opencode"] {
            let id = self
                .backend
                .invoke("list_provider_models", json!({ "id": provider }));
            self.waiting.insert(id, Waiting::Models(provider));
        }
        self.refresh_usage();
        for (command, waiting) in [
            ("detect_agents", Waiting::Agents),
            ("detect_editors", Waiting::Editors),
            ("editor_app_icons", Waiting::Icons),
        ] {
            let id = self.backend.invoke(command, json!({}));
            self.waiting.insert(id, waiting);
        }
        let secrets = self
            .backend
            .invoke("secret_action", json!({ "action": { "type": "status" } }));
        self.waiting.insert(secrets, Waiting::Secrets);
        self.ensure_loaded(self.tabs[0].conversation);
        self.refresh_skills();
        self.refresh_files();
        self.refresh_diff();
        self.first_run_check(cx);
        cx.notify();
    }

    /// A change from a Settings page: the back end keeps the whole settings object.
    fn save_setting(&mut self, key: &'static str, value: Value, cx: &mut Context<Self>) {
        self.settings[key] = value;
        if key == "locale" {
            crate::i18n::set_locale(self.settings["locale"].as_str().unwrap_or("pt-BR"));
        }
        self.backend
            .invoke("save_settings", json!({ "settings": self.settings }));
        cx.notify();
    }

    /// What the open Settings page shows, fresh from the back end.
    /// Settings → Provedores: the install list first; each installed CLI is then tested and its
    /// login read (cli_diagnostics.rs).
    fn refresh_cli_diagnostics(&mut self) {
        self.settings_data.remove("cliProbes");
        self.settings_data.remove("cliLogins");
        let call = self.backend.invoke("detect_agents", json!({}));
        self.waiting.insert(call, Waiting::CliAgents);
    }

    fn cli_agents_arrived(&mut self, agents: Value, cx: &mut Context<Self>) {
        let installed: Vec<String> = agents
            .as_array()
            .into_iter()
            .flatten()
            .filter(|agent| agent["installed"].as_bool() == Some(true))
            .filter_map(|agent| agent["id"].as_str().map(str::to_owned))
            .collect();
        for id in installed {
            let probe = self
                .backend
                .invoke("probe_provider", json!({ "id": id, "path": Value::Null }));
            self.waiting.insert(probe, Waiting::CliProbe(id.clone()));
            let usage = self
                .backend
                .invoke("provider_usage", json!({ "provider": id, "refresh": true }));
            self.waiting.insert(usage, Waiting::CliLogin(id));
        }
        self.settings_data.insert("agents", agents);
        cx.notify();
    }

    fn cli_reply(&mut self, key: &'static str, id: String, reply: Value) {
        let slot = self.settings_data.entry(key).or_insert_with(|| json!({}));
        slot[id] = reply;
    }

    /// Sends one `project_scripts_action`; its failure shows as the window's problem.
    fn script_action(&mut self, args: Value, cx: &mut Context<Self>) {
        let call = self.backend.invoke("project_scripts_action", args);
        self.waiting.insert(call, Waiting::ScriptAction);
        cx.notify();
    }

    fn run_script_in(
        &mut self,
        session: &str,
        kind: crate::project_scripts::Kind,
        cx: &mut Context<Self>,
    ) {
        self.scripts_output = false;
        self.script_action(crate::project_scripts::run_args(session, kind), cx);
    }

    /// Saves the settings popup's draft when it changed and is valid; `leave` ends the edit.
    /// An invalid draft is not saved, and leaving discards it.
    fn save_project_scripts(&mut self, leave: bool) {
        if let Some(edit) = self.scripts_edit.as_ref() {
            if let Some(project) = self.projects.get(edit.project) {
                if edit.draft.changed(&project.scripts) && edit.draft.valid() {
                    let args = edit.draft.save_args(&project.id);
                    let index = edit.project;
                    let call = self.backend.invoke("project_scripts_action", args);
                    self.waiting.insert(call, Waiting::ScriptsSaved(index));
                }
            }
        }
        if leave {
            self.scripts_edit = None;
        }
    }

    fn focus_script(&mut self, field: crate::project_scripts::Field, cx: &mut Context<Self>) {
        if let Some(edit) = self.scripts_edit.as_mut() {
            edit.focus = Some(field);
        }
        cx.notify();
    }

    fn toggle_script_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(edit) = self.scripts_edit.as_mut() {
            if edit.draft.open_preview_allowed() {
                edit.draft.open_preview = !edit.draft.open_preview;
            }
        }
        cx.notify();
    }

    /// The Scripts row of the settings popup: unfolding starts an edit of the saved scripts.
    fn toggle_scripts_row(&mut self, cx: &mut Context<Self>) {
        let opening = self.project_menu.as_ref().and_then(|m| m.expanded) != Some("scripts");
        self.toggle_project_row("scripts", cx);
        if opening {
            if let Some(ix) = self.current_project {
                self.scripts_edit = Some(crate::project_scripts::Edit::new(
                    ix,
                    &self.projects[ix].scripts,
                ));
            }
        }
    }

    /// The Scripts block of the settings popup, scrolling inside its own area.
    fn scripts_fields_view(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let project = &self.projects[index];
        let edit = self
            .scripts_edit
            .as_ref()
            .filter(|edit| edit.project == index);
        let draft = edit.map_or_else(
            || crate::project_scripts::Draft::of(&project.scripts),
            |edit| edit.draft.clone(),
        );
        let focus = edit.and_then(|edit| edit.focus);
        let can_save =
            edit.is_some_and(|edit| edit.draft.changed(&project.scripts) && edit.draft.valid());
        use crate::project_scripts::Field;
        let handlers = [
            crate::project_scripts_view::click(cx, |this, cx| this.focus_script(Field::Setup, cx)),
            crate::project_scripts_view::click(cx, |this, cx| this.focus_script(Field::Finish, cx)),
            crate::project_scripts_view::click(cx, |this, cx| {
                this.focus_script(Field::Preview, cx)
            }),
        ];
        let toggle =
            crate::project_scripts_view::click(cx, |this, cx| this.toggle_script_preview(cx));
        let save = crate::project_scripts_view::click(cx, |this, cx| {
            this.save_project_scripts(false);
            cx.notify();
        });
        let fields =
            crate::project_scripts_view::fields(&draft, focus, handlers, toggle, save, can_save);
        div()
            .id("project-scripts-scroll")
            .max_h(px(crate::project_scripts_view::SETTINGS_HEIGHT - 20.))
            .overflow_y_scroll()
            .child(fields)
            .into_any_element()
    }

    /// The run notice above the composer: the conversation's latest script run.
    fn script_notice(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let conversation = self.conversations.get(ix)?;
        let run = crate::project_scripts::visible(&conversation.scripts)?.clone();
        let shown = crate::project_scripts::notice(&run, conversation.working());
        let session = conversation.session_id.clone();
        let (stop, again, dismiss) = (session.clone(), session.clone(), session);
        let kind = run.kind;
        let handlers = [
            crate::project_scripts_view::click(cx, move |this, cx| {
                this.script_action(crate::project_scripts::cancel_args(&stop), cx)
            }),
            crate::project_scripts_view::click(cx, move |this, cx| {
                this.run_script_in(&again, kind, cx)
            }),
            crate::project_scripts_view::click(cx, move |this, cx| {
                this.script_action(crate::project_scripts::dismiss_args(&dismiss, kind), cx)
            }),
            crate::project_scripts_view::click(cx, |this, cx| {
                this.scripts_output = !this.scripts_output;
                cx.notify();
            }),
        ];
        Some(crate::project_scripts_view::notice(
            &run,
            shown,
            self.scripts_output,
            handlers,
        ))
    }

    /// The composer's add menu rows that run the project's saved scripts in this conversation.
    fn script_run_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        use crate::project_scripts::Kind;
        let ix = self.active_conversation();
        let conversation = &self.conversations[ix];
        let Some(project) = self.project_of(ix).and_then(|p| self.projects.get(p)) else {
            return Vec::new();
        };
        let running = conversation
            .scripts
            .iter()
            .any(|run| run.status == crate::project_scripts::Status::Running);
        let enabled = crate::project_scripts::can_run(
            conversation.isolated,
            conversation.side_chat,
            conversation.working(),
            running,
        );
        [Kind::Setup, Kind::Finish]
            .into_iter()
            .filter(|kind| project.scripts.has(*kind))
            .map(|kind| {
                let about = if !conversation.isolated {
                    t("Roda só em um worktree isolado").to_owned()
                } else if running {
                    t("Um script já está rodando").to_owned()
                } else {
                    project
                        .scripts
                        .body(kind)
                        .lines()
                        .map(str::trim)
                        .find(|line| !line.is_empty())
                        .unwrap_or("")
                        .to_owned()
                };
                let session = conversation.session_id.clone();
                let id = match kind {
                    Kind::Setup => "add-run-setup",
                    Kind::Finish => "add-run-finish",
                };
                let on_click = crate::project_scripts_view::click(cx, move |this, cx| {
                    this.add_menu = None;
                    this.run_script_in(&session, kind, cx);
                });
                crate::project_scripts_view::run_row(id, kind, about, enabled, on_click)
            })
            .collect()
    }

    fn load_settings_page(&mut self) {
        self.publish_computers();
        if self.settings_page == "Provedores" {
            self.refresh_cli_diagnostics();
        }
        for (key, command, args) in crate::settings_pages::loads(self.settings_page) {
            if command == "native_github_list" {
                self.native_task(key, crate::native_actions::github_list);
                continue;
            }
            let id = self.backend.invoke(command, args);
            self.waiting.insert(id, Waiting::SettingsData(key));
        }
    }

    /// Runs a blocking action off the interface; its result lands in the page's data.
    /// Checks the update feed, or downloads and installs the version on offer (app_update.rs), off
    /// the UI thread; every phase change redraws. `then_install` installs what a check found
    /// (the automatic download). Nothing here restarts the app.
    fn app_update_run(&mut self, install: bool, then_install: bool) {
        let tx = self.native_tx.clone();
        std::thread::spawn(move || {
            let ping = || {
                let _ = tx.send_blocking(("app_update", Value::Null));
            };
            ping();
            if install {
                crate::app_update::install(ping);
            } else {
                let found = crate::app_update::check();
                if then_install && found.phase == crate::app_update::Phase::Available {
                    ping();
                    crate::app_update::install(ping);
                }
            }
            ping();
        });
    }

    /// The timed check: only in a release build, only when Settings allows it, and it downloads
    /// only when "Baixar atualizações em segundo plano" is on.
    fn app_update_auto(&mut self) {
        let on = |key: &str| self.settings[key].as_bool().unwrap_or(false);
        if !crate::app_update::automatic_allowed()
            || !on("appUpdateAutoCheck")
            || !crate::app_update::automatic_due()
        {
            return;
        }
        let download = on("appUpdateAutoDownload");
        self.app_update_run(false, download);
    }

    /// Restarts into the installed update. With agents running and "confirm before closing" on, the
    /// running-agents confirmation comes first (ADR-090); declining keeps everything as it is.
    fn app_update_restart(&mut self, cx: &mut Context<Self>) {
        let agents = self.conversations.iter().any(|c| c.working());
        let confirm = self.settings["confirmCloseRunning"]
            .as_bool()
            .unwrap_or(true);
        if agents && confirm {
            self.ask(
                t("Reiniciar para atualizar?").to_owned(),
                t("Há agentes trabalhando. Reiniciar interrompe todos. As conversas e as mensagens ficam salvas, e a atualização também vale na próxima vez que você abrir o Sirus.").to_owned(),
                t("Reiniciar e interromper"),
                |this, cx| this.app_update_relaunch(cx),
                cx,
            );
        } else {
            self.app_update_relaunch(cx);
        }
    }

    fn app_update_relaunch(&mut self, cx: &mut Context<Self>) {
        match crate::app_update::relaunch() {
            Ok(()) => cx.quit(),
            Err(e) => {
                self.problem = Some(e);
                cx.notify();
            }
        }
    }

    /// The small notice in the footer: a version on offer, its download, or the restart it waits for.
    fn update_notice(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        use crate::app_update::{Phase, status};
        let u = status();
        let version = u.version.clone().unwrap_or_default();
        let (text, kind): (String, Option<&'static str>) = match u.phase {
            Phase::Available => (
                tf("Sirus {} disponível · Instalar", &[&version]),
                Some("install"),
            ),
            Phase::Downloading => (tf("Baixando o Sirus {}…", &[&version]), None),
            Phase::Installed => (
                t("Atualização pronta · Reiniciar").to_owned(),
                Some("restart"),
            ),
            _ => return None,
        };
        let notice = div()
            .id("app-update-notice")
            .h(px(22.))
            .px(px(6.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .text_size(px(12.))
            .text_color(accent())
            .child(text);
        Some(match kind {
            Some(kind) => notice
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.run_setting("native_app_update", json!({ "type": kind }), cx)
                })),
            None => notice,
        })
    }

    fn native_task(&self, key: &'static str, work: impl FnOnce() -> Value + Send + 'static) {
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(work());
        });
        let results = self.native_tx.clone();
        std::thread::spawn(move || {
            if let Ok(value) = rx.recv_blocking() {
                let _ = results.send_blocking((key, value));
            }
        });
    }

    fn edit_setting(&mut self, key: &'static str, text: String, cx: &mut Context<Self>) {
        self.setting_edit = Some((key, text));
        cx.notify();
    }

    /// A button on a Settings page: runs its command, then reloads the page's data. The
    /// `native_` ones are this app's own: pairing and native dialogs.
    fn run_setting(&mut self, command: &'static str, args: Value, cx: &mut Context<Self>) {
        let text = |key: &str| args[key].as_str().unwrap_or("").to_owned();
        match command {
            "native_sound" => crate::native_notify::play(&text("sound")),
            "native_notify_test" => {
                if let Some(sound) = self.settings["notifications"]["completionSound"].as_str() {
                    crate::native_notify::play(sound);
                }
                crate::native_notify::banner(
                    "Sirus",
                    crate::i18n::t(
                        "Teste de notificação. Seus avisos de atividade aparecerão aqui.",
                    ),
                );
            }
            // Keys left out come back at their defaults: the host fills them in when it reads.
            "native_restore" => {
                let keys: Vec<String> = args["keys"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|key| key.as_str().map(str::to_owned))
                    .collect();
                if let Some(settings) = self.settings.as_object_mut() {
                    for key in &keys {
                        settings.remove(key);
                    }
                }
                let call = self
                    .backend
                    .invoke("save_settings", json!({ "settings": self.settings }));
                self.waiting.insert(call, Waiting::SettingsRestored);
                cx.notify();
            }
            "native_restore_look" => {
                self.set_look(|look| *look = Look::default(), cx);
            }
            "native_confirm" => {
                let label = text("label");
                let title = args["title"].as_str().map(str::to_owned);
                let message = args["message"].as_str().map(str::to_owned);
                let then = args["then"].clone();
                let run = move |this: &mut Self, cx: &mut Context<Self>| {
                    let name = |key: &str| -> &'static str {
                        Box::leak(then[key].as_str().unwrap_or("").to_owned().into_boxed_str())
                    };
                    if then["kind"] == "save" {
                        this.save_setting(name("key"), then["value"].clone(), cx);
                    } else {
                        this.run_setting(name("cmd"), then["args"].clone(), cx);
                    }
                };
                let action_label: &'static str = Box::leak(label.clone().into_boxed_str());
                self.ask(
                    title.unwrap_or_else(|| format!("{label}?")),
                    message.unwrap_or_else(|| {
                        crate::i18n::t("Isso não pode ser desfeito.").to_owned()
                    }),
                    action_label,
                    run,
                    cx,
                );
            }
            "native_terminal_placement" => {
                if let Some(placement) =
                    crate::terminal_place::Placement::from_id(&text("placement"))
                {
                    self.set_term_placement(placement, cx);
                }
            }
            "native_connector_remove" => {
                // Forgets the sign-in, then drops the connector from the saved list.
                self.backend.invoke(
                    "connector_action",
                    json!({ "action": { "type": "signOut", "id": args["id"] } }),
                );
                self.save_setting("connectors", args["list"].clone(), cx);
            }
            "connector_action" => {
                // The reply is the sign-in states; it lands in the page when a sign-in is over.
                let id = self.backend.invoke(command, args);
                self.waiting.insert(id, Waiting::SettingsData("connectors"));
            }
            "native_github_list" => self.native_task("github", crate::native_actions::github_list),
            "native_github_switch" => {
                let login = text("login");
                self.native_task("github", move || {
                    crate::native_actions::github_switch(&login);
                    crate::native_actions::github_list()
                });
            }
            "native_github_logout" => {
                let login = text("login");
                self.native_task("github", move || {
                    crate::native_actions::github_logout(&login);
                    crate::native_actions::github_list()
                });
            }
            "native_github_login" => {
                let results = self.native_tx.clone();
                self.native_task("github_device", move || {
                    let Some((code, url)) = crate::native_actions::github_login_start() else {
                        return Value::Null;
                    };
                    let _ = results
                        .send_blocking(("github_device", json!({ "code": code, "url": url })));
                    crate::native_actions::github_login_wait();
                    let _ = results.send_blocking(("github", crate::native_actions::github_list()));
                    Value::Null
                });
            }
            "native_github_cancel" => {
                crate::native_actions::github_login_cancel();
                self.settings_data.remove("github_device");
            }
            "native_app_update" => match text("type").as_str() {
                "check" => self.app_update_run(false, false),
                "install" => self.app_update_run(true, false),
                "restart" => self.app_update_restart(cx),
                _ => {}
            },
            "native_open_path" => {
                let path = text("path");
                std::thread::spawn(move || crate::native_actions::open_path(&path));
            }
            "native_delete_sessions" => {
                for id in args["ids"].as_array().into_iter().flatten() {
                    self.backend
                        .invoke("delete_session", json!({ "sessionId": id }));
                }
                self.load_settings_page();
            }
            "native_reset_layout" => {
                self.sidebar_width = SIDEBAR_MIN;
                self.look = Look::default();
                save_look(&self.look);
                self.push_phone_look(false);
            }
            "cli_diagnostics" => self.refresh_cli_diagnostics(),
            "native_first_run" => self.first_run_start(cx),
            "native_copy" => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text("text")));
            }
            "remote_action" => {
                // The reply is the whole status, which lands in the page; reloading beside it
                // would race the change.
                let id = self.backend.invoke(command, args);
                self.waiting.insert(id, Waiting::SettingsData("remote"));
            }
            name if name.starts_with("native_computer_") => self.computer_command(name, &args, cx),
            name if name.starts_with("native_pair_") => {
                let known = self.settings_data["remote"]["devices"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|d| d["id"].as_str().map(str::to_owned))
                    .collect();
                crate::settings_pair::command(
                    self,
                    name,
                    &args,
                    &self.backend.clone(),
                    Shell::run_setting,
                    Shell::edit_setting,
                    known,
                    cx,
                );
            }
            "native_pick_folder" | "native_pick_background" => {
                let folder = command == "native_pick_folder";
                let key = args["key"].as_str().unwrap_or("").to_owned();
                let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
                    files: !folder,
                    directories: folder,
                    multiple: false,
                    prompt: None,
                });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = paths.await {
                        let Some(path) = paths.first().map(|p| p.display().to_string()) else {
                            return;
                        };
                        let _ = this.update(cx, |this, cx| {
                            if folder {
                                this.settings[key.as_str()] = json!(path);
                                this.backend
                                    .invoke("save_settings", json!({ "settings": this.settings }));
                            } else {
                                let name = format!("chat-{}.jpg", uuid());
                                if backend::save_background(&path, &name) {
                                    this.settings["chatBackground"] = json!(name);
                                    this.backend.invoke(
                                        "save_settings",
                                        json!({ "settings": this.settings }),
                                    );
                                }
                            }
                            cx.notify();
                        });
                    }
                })
                .detach();
            }
            _ => {
                self.backend.invoke(command, args);
                self.load_settings_page();
            }
        }
        cx.notify();
    }

    fn commit_setting_edit(&mut self, cx: &mut Context<Self>) {
        let Some((key, text)) = self.setting_edit.take() else {
            return;
        };
        match crate::settings_pages::commit(&self.settings, key, &text) {
            Some(crate::settings_pages::Commit::Save { key, value }) => {
                self.save_setting(key, value, cx)
            }
            Some(crate::settings_pages::Commit::Run { cmd, args }) => {
                self.run_setting(cmd, args, cx)
            }
            None => {}
        }
    }

    fn project_of(&self, conversation: usize) -> Option<usize> {
        self.projects
            .iter()
            .position(|p| p.conversations.contains(&conversation))
    }

    /// The id of the project the sidebar shows: the current one, or the first in the list.
    pub(super) fn sidebar_project_id(&self) -> Option<String> {
        self.current_project
            .or(self.project_order.first().copied())
            .map(|ix| self.projects[ix].id.clone())
    }

    /// Tells the Pull requests page which project is selected, so it shows that repository.
    pub(super) fn sync_pulls_project(&self, cx: &mut Context<Self>) {
        if let Some(view) = self.term.pulls.clone() {
            let id = self.sidebar_project_id();
            view.update(cx, |page, cx| page.set_project(id, cx));
        }
    }

    /// Tabs belong to their project, as in Monocode: going to another project brings its own.
    fn enter_project(&mut self, project: Option<usize>, cx: &mut Context<Self>) {
        if project == self.current_project {
            return;
        }
        let now = Instant::now();
        let leaving = std::mem::take(&mut self.tabs);
        if let Some(old) = self.current_project {
            let kept: Vec<Tab> = leaving.into_iter().filter(|t| !t.closing).collect();
            self.parked.insert(old, (kept, self.active));
        }
        self.current_project = project;
        self.sync_pulls_project(cx);
        self.drag = None;
        self.menu = None;
        self.card_hovered = None;
        self.deck_front = None;
        if let Some((mut tabs, active)) = project.and_then(|p| self.parked.remove(&p)) {
            for tab in &mut tabs {
                tab.x = Tween::still(-1., now);
                tab.w = Tween::still(-1., now);
            }
            self.tabs = tabs;
            self.active = active;
        }
    }

    fn refresh_skills(&mut self) {
        let session = self.conversations[self.active_conversation()]
            .session_id
            .clone();
        if session.is_empty() {
            return;
        }
        let id = self.backend.invoke(
            "skill_action",
            json!({ "action": { "type": "catalog", "owner": { "sessionId": session } } }),
        );
        self.waiting.insert(id, Waiting::Skills);
    }

    /// The `/` or `@` token under the caret and what it can complete to.
    fn mention(&self) -> Option<Mention> {
        if self.palette_ui.is_some()
            || self.appearance.is_some()
            || self.usage_page_open()
            || self.secret_entry.is_some()
            || self.grabs_typing()
        {
            return None;
        }
        let draft = self.drafts.get(&self.active_conversation())?;
        if draft.anchor != draft.cursor {
            return None;
        }
        let before = &draft.text[..draft.cursor];
        let start = before.rfind(char::is_whitespace).map_or(0, |i| {
            i + before[i..].chars().next().map_or(1, char::len_utf8)
        });
        if self.mention_dismissed == Some(start) {
            return None;
        }
        let token = &before[start..];
        let needle = token.get(1..)?.to_lowercase();
        let hits: Vec<(String, String)> = match token.chars().next()? {
            '/' => self
                .skills
                .iter()
                .filter(|(name, _)| name.to_lowercase().contains(&needle))
                .take(8)
                .cloned()
                .collect(),
            '@' => self
                .files
                .1
                .iter()
                .filter(|(name, folder)| {
                    format!("{folder}/{name}").to_lowercase().contains(&needle)
                })
                .take(8)
                .map(|(name, folder)| {
                    (
                        if folder.is_empty() {
                            name.clone()
                        } else {
                            format!("{folder}/{name}")
                        },
                        String::new(),
                    )
                })
                .collect(),
            _ => return None,
        };
        (!hits.is_empty()).then_some(Mention { start, hits })
    }

    fn accept_mention(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(mention) = self.mention() else {
            return;
        };
        let draft = self.draft();
        let sigil = draft.text[mention.start..].chars().next().unwrap_or('/');
        draft.anchor = mention.start;
        draft.insert(&format!("{sigil}{name} "));
        self.mention_selected = 0;
        cx.notify();
    }

    fn refresh_usage(&mut self) {
        self.request_usage(false);
    }

    fn accounts_of(&self, provider: &str) -> Vec<(String, String)> {
        let mut list = vec![("default".to_owned(), t("Conta padrão").to_owned())];
        for account in self.provider_accounts.as_array().into_iter().flatten() {
            if account["provider"].as_str() == Some(provider) {
                list.push((
                    account["id"].as_str().unwrap_or("").to_owned(),
                    account["label"].as_str().unwrap_or("").to_owned(),
                ));
            }
        }
        list
    }

    fn load_account_usage(&mut self, provider: &'static str) {
        for (id, _) in self.accounts_of(provider) {
            let call = self.backend.invoke(
                "provider_usage",
                json!({ "provider": provider, "refresh": false, "accountId": id }),
            );
            self.waiting.insert(call, Waiting::AccountUsage(id));
        }
    }

    fn sign_in_account(&mut self, provider: &'static str, id: String) {
        if let Some(page) = self.usage_accounts.as_mut() {
            page.signing_in = Some(id.clone());
            page.problem = None;
        }
        let call = self.backend.invoke(
            "login_provider_account",
            json!({ "provider": provider, "accountId": id }),
        );
        self.waiting
            .insert(call, Waiting::AccountLogin(provider, id));
    }

    fn select_account(&mut self, provider: &'static str, id: String) {
        self.backend.invoke(
            "select_provider_account",
            json!({ "provider": provider, "accountId": id }),
        );
        self.selected_accounts[provider] = json!(id);
    }

    fn accounts_page_view(&self, provider: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let page = self.usage_accounts.as_ref();
        let chosen = self.selected_accounts[provider]
            .as_str()
            .unwrap_or("default")
            .to_owned();
        let signing = page.and_then(|p| p.signing_in.clone());
        let rows: Vec<AnyElement> = self
            .accounts_of(provider)
            .into_iter()
            .enumerate()
            .map(|(index, (id, label))| {
                let who = page
                    .and_then(|p| p.who.get(&id).cloned())
                    .unwrap_or_else(|| "Carregando…".to_owned());
                let signed = who != "Sem login" && who != "Carregando…";
                // `who` keeps the Portuguese placeholders it is compared with; the row shows them translated.
                let who = match who.as_str() {
                    "Sem login" => t("Sem login").to_owned(),
                    "Carregando…" => t("Carregando…").to_owned(),
                    _ => who,
                };
                let current = id == chosen;
                let target = id.clone();
                div()
                    .id(("account-row", index))
                    .mt(px(8.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(crate::theme::RADIUS_LG))
                    .border_1()
                    .border_color(content(if current { 0.2 } else { 0.07 }))
                    .bg(content(if current { 0.07 } else { 0.03 }))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if signed || target == "default" {
                            this.select_account(provider, target.clone());
                        } else {
                            this.sign_in_account(provider, target.clone());
                        }
                        cx.notify();
                    }))
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
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(content(0.95))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .mt(px(2.))
                                    .text_size(px(11.5))
                                    .text_color(content(0.45))
                                    .child(if signed || id == "default" {
                                        who
                                    } else {
                                        t("Clique para entrar").to_owned()
                                    }),
                            ),
                    )
                    .when(current, |d| {
                        d.child(icon("check", 14.).text_color(accent()))
                    })
                    .into_any_element()
            })
            .collect();
        let typed = page.and_then(|p| p.adding.clone());
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .id("accounts-back")
                            .size(px(26.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.07)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.usage_accounts = None;
                                cx.notify();
                            }))
                            .child(icon("arrow-left", 15.).text_color(content(0.7))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(14.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(content(0.97))
                            .child(tf("Contas do {}", &[&provider_title(provider)])),
                    ),
            )
            .child(
                div()
                    .mt(px(8.))
                    .text_size(px(11.5))
                    .text_color(content(0.45))
                    .child(t("Cada conversa fica na conta que a iniciou.")),
            )
            .children(rows)
            .when_some(signing, |d, id| {
                d.child(
                    div()
                        .mt(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .text_color(content(0.6))
                        .child(div().flex_1().child(t("Conclua o login no navegador.")))
                        .child(
                            div()
                                .id("account-cancel")
                                .px(px(8.))
                                .h(px(24.))
                                .rounded(px(crate::theme::RADIUS_SM))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(content(0.07)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.backend.invoke(
                                        "cancel_provider_account_login",
                                        json!({ "provider": provider, "accountId": id }),
                                    );
                                    cx.notify();
                                }))
                                .child(t("Cancelar")),
                        ),
                )
            })
            .when_some(page.and_then(|p| p.problem.clone()), |d, problem| {
                d.child(
                    div()
                        .mt(px(8.))
                        .text_size(px(12.))
                        .text_color(rgb(0xe08a8a))
                        .child(problem),
                )
            })
            .child(match typed {
                Some(name) => div()
                    .mt(px(10.))
                    .h(px(30.))
                    .px(px(8.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .border_1()
                    .border_color(content(0.3))
                    .bg(field_fill())
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(13.))
                    .child(typed_text(
                        &name,
                        t("Nome da conta, Enter para entrar"),
                        div()
                            .flex_shrink_0()
                            .w(px(1.5))
                            .h(px(15.))
                            .rounded_full()
                            .bg(crate::theme::caret_color()),
                    ))
                    .into_any_element(),
                None => div()
                    .id("account-add")
                    .mt(px(10.))
                    .h(px(28.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .bg(content(0.04))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .text_size(px(11.5))
                    .text_color(content(0.72))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(page) = this.usage_accounts.as_mut() {
                            page.adding = Some(String::new());
                        }
                        cx.notify();
                    }))
                    .child(icon("plus", 13.).text_color(content(0.6)))
                    .child(t("Adicionar conta"))
                    .into_any_element(),
            })
            .into_any_element()
    }

    fn request_usage(&mut self, refresh: bool) {
        let provider = self.conversations[self.active_conversation()].provider;
        let id = self.backend.invoke(
            "provider_usage",
            json!({ "provider": provider, "refresh": refresh }),
        );
        self.waiting.insert(id, Waiting::Usage);
        self.usage_refreshing = refresh;
    }

    fn toggle_usage_popup(&mut self, cx: &mut Context<Self>) {
        self.usage_accounts = None;
        self.usage_popup = match self.usage_popup {
            Some(_) => None,
            None => {
                self.request_usage(false);
                Some(Tween::from_to(
                    0.,
                    1.,
                    Instant::now(),
                    if cx.reduce_motion() { 0. } else { 0.16 },
                    Curve::Expo,
                ))
            }
        };
        cx.notify();
    }

    fn open_settings_page(&mut self, name: &'static str, cx: &mut Context<Self>) {
        self.usage_popup = None;
        self.settings_page = name;
        self.settings_since = Instant::now();
        if self.appearance.is_none() {
            self.toggle_appearance(cx);
        } else {
            self.load_settings_page();
            cx.notify();
        }
    }

    /// What the footer's usage opens: Monocode's cards with Sirus Code's account and actions.
    fn usage_popup_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.usage_popup.as_ref()?;
        if !open.done(now) {
            *moving = true;
        }
        let t = open.at(now).clamp(0., 1.);
        let provider = self.conversations[self.active_conversation()].provider;
        let value = &self.usage_value;
        let updated = value["updatedAt"].as_i64().map(|at| {
            let minutes = (chrono::Utc::now().timestamp_millis() - at) / 60_000;
            if minutes < 1 {
                crate::i18n::t("Atualizado agora").to_owned()
            } else if minutes < 60 {
                tf("Atualizado há {} min", &[&minutes])
            } else {
                tf("Atualizado há {} h", &[&(minutes / 60)])
            }
        });
        let account = value["account"]["email"]
            .as_str()
            .or(value["account"]["name"].as_str())
            .map(str::to_owned)
            .or_else(|| {
                value["account"]["keyFingerprint"]
                    .as_str()
                    .map(|key| tf("Chave conectada · {}", &[&key]))
            });
        let plan = value["account"]["plan"]
            .as_str()
            .filter(|p| !p.is_empty())
            .map(|p| {
                let mut letters = p.chars();
                letters.next().map_or(String::new(), |first| {
                    first.to_uppercase().chain(letters).collect()
                })
            });
        let mut windows: Vec<&Value> = value["windows"].as_array().into_iter().flatten().collect();
        windows.sort_by_key(|w| {
            w["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("base_model_inference/"))
        });
        let cards: Vec<AnyElement> = windows
            .iter()
            .map(|window| {
                let used = window["usedPercent"].as_f64();
                let label = usage_window_label(
                    window["id"].as_str().unwrap_or(""),
                    window["durationMinutes"].as_u64(),
                );
                let reset = window["resetsAt"].as_i64().map(|at| {
                    if at > chrono::Utc::now().timestamp_millis() {
                        tf("Renova em {}", &[&reset_left(at)])
                    } else {
                        crate::i18n::t("Hora de renovar já passou").to_owned()
                    }
                });
                div()
                    .mt(px(8.))
                    .px(px(12.))
                    .py(px(11.))
                    .rounded(px(crate::theme::RADIUS_LG))
                    .border_1()
                    .border_color(content(0.07))
                    .bg(content(0.03))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap(px(8.))
                            .text_size(px(13.))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(content(0.72))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(content(0.95))
                                    .child(used.map_or_else(
                                        || "—".to_owned(),
                                        |u| tf("{} usado", &[&format!("{u:.0}%")]),
                                    )),
                            ),
                    )
                    .when_some(used, |d, used| {
                        d.child(
                            div()
                                .my(px(9.))
                                .h(px(6.))
                                .rounded_full()
                                .bg(content(0.08))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(used.clamp(0., 100.) as f32 / 100.))
                                        .rounded_full()
                                        .bg(usage_bar_color(used)),
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap(px(8.))
                            .text_size(px(11.5))
                            .text_color(content(0.45))
                            .children(used.map(|u| {
                                div().child(tf(
                                    "{} restante",
                                    &[&format!("{:.0}%", (100. - u).max(0.))],
                                ))
                            }))
                            .children(reset.map(|r| div().child(r))),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = cards.is_empty();
        let note = value["note"].as_str().map(str::to_owned);
        let action = |id: &'static str,
                      glyph: &'static str,
                      label: &'static str,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex_1()
                .h(px(28.))
                .rounded(px(crate::theme::RADIUS_MD))
                .bg(content(0.04))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .text_size(px(11.5))
                .text_color(content(0.72))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.09)).text_color(content(0.95)))
                .on_click(cx.listener(|this, _, _, cx| this.open_settings_page("Provedores", cx)))
                .child(icon(glyph, 13.).text_color(content(0.6)))
                .child(label)
        };
        let add = div()
            .id("usage-account")
            .flex_1()
            .h(px(28.))
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(content(0.04))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .text_size(px(11.5))
            .text_color(content(0.72))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.09)).text_color(content(0.95)))
            .on_click(cx.listener(|this, _, _, cx| {
                let provider = this.conversations[this.active_conversation()].provider;
                this.usage_accounts = Some(AccountsPage {
                    provider,
                    ..Default::default()
                });
                this.load_account_usage(provider);
                cx.notify();
            }))
            .child(icon("plus", 13.).text_color(content(0.6)))
            .child(crate::i18n::t("Conta"));
        let providers = action(
            "usage-providers",
            "settings",
            crate::i18n::t("Provedores"),
            cx,
        );
        let left = self
            .usage_chip
            .get()
            .map_or(8., |bounds| f32::from(bounds.left()));
        let spin = if self.usage_refreshing {
            *moving = true;
            now.saturating_duration_since(self.composer_menu_at)
                .as_secs_f32()
        } else {
            0.
        };
        Some(
            div()
                .id("usage-popup")
                .occlude()
                .absolute()
                .bottom(px(FOOTER + 2. + (1. - t) * -6.))
                .left(px(left.min(self.viewport_width - 320.).max(8.)))
                .w(px(310.))
                .p(px(14.))
                .flex()
                .flex_col()
                .opacity(t)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .when(self.usage_accounts.is_some(), |d| {
                    d.child(self.accounts_page_view(provider, cx))
                })
                .when(self.usage_accounts.is_none(), |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .size(px(34.))
                                    .flex_shrink_0()
                                    .rounded(px(crate::theme::RADIUS_MD))
                                    .border_1()
                                    .border_color(content(0.08))
                                    .bg(content(0.05))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(provider_mark(provider, 18.)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(14.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(content(0.97))
                                            .child(tf("Uso do {}", &[&provider_title(provider)])),
                                    )
                                    .children(updated.map(|u| {
                                        div()
                                            .mt(px(2.))
                                            .text_size(px(11.5))
                                            .text_color(content(0.45))
                                            .child(u)
                                    })),
                            )
                            .child(
                                div()
                                    .id("usage-refresh")
                                    .size(px(26.))
                                    .flex_shrink_0()
                                    .rounded(px(crate::theme::RADIUS_MD))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(content(0.07)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.composer_menu_at = Instant::now();
                                        this.request_usage(true);
                                        cx.notify();
                                    }))
                                    .child(
                                        icon("refresh", 14.)
                                            .text_color(content(0.55))
                                            .with_transformation(Transformation::rotate(
                                                gpui::radians(spin * std::f32::consts::TAU),
                                            )),
                                    ),
                            ),
                    )
                })
                .when_some(
                    account.filter(|_| self.usage_accounts.is_none()),
                    |d, account| {
                        let initial = account
                            .chars()
                            .next()
                            .map_or(String::new(), |c| c.to_uppercase().collect());
                        d.child(
                            div()
                                .mt(px(10.))
                                .px(px(8.))
                                .py(px(6.))
                                .rounded(px(crate::theme::RADIUS_LG))
                                .bg(content(0.04))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .size(px(22.))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(11.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(gpui::white())
                                        .bg(linear_gradient(
                                            135.,
                                            linear_color_stop(gpui::rgb(0x5b8cff), 0.),
                                            linear_color_stop(gpui::rgb(0xa371f7), 1.),
                                        ))
                                        .child(initial),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(12.5))
                                        .text_color(content(0.72))
                                        .child(account),
                                )
                                .children(plan.map(|p| {
                                    div()
                                        .flex_shrink_0()
                                        .px(px(8.))
                                        .py(px(1.))
                                        .rounded_full()
                                        .bg(content(0.08))
                                        .text_size(px(11.))
                                        .text_color(content(0.78))
                                        .child(p)
                                })),
                        )
                    },
                )
                .when(self.usage_accounts.is_none(), |d| d.children(cards))
                .when(empty && self.usage_accounts.is_none(), |d| {
                    d.child(
                        div()
                            .mt(px(10.))
                            .text_size(px(12.5))
                            .text_color(content(0.5))
                            .child(note.unwrap_or_else(|| {
                                crate::i18n::t("Este provedor não informa uso.").to_owned()
                            })),
                    )
                })
                .when(self.usage_accounts.is_none(), |d| {
                    d.child(
                        div()
                            .mt(px(10.))
                            .pt(px(10.))
                            .border_t_1()
                            .border_color(content(0.07))
                            .flex()
                            .gap(px(6.))
                            .child(add)
                            .child(providers),
                    )
                })
                .into_any_element(),
        )
    }

    fn load_older(&mut self, ix: usize, cx: &mut Context<Self>) {
        let conversation = &self.conversations[ix];
        let id = self.backend.invoke("transcript_action", json!({ "action": { "type": "load", "sessionId": conversation.session_id, "before": conversation.base, "limit": 120, "lite": true } }));
        self.waiting.insert(id, Waiting::Older(ix));
        cx.notify();
    }

    fn answer(&mut self, ix: usize, request: &Value, question: &str, label: &str) {
        let args = json!({ "request": { "sessionId": self.conversations[ix].session_id, "generation": request["generation"], "requestId": request["requestId"], "turnId": request["turnId"], "response": { "type": "userInput", "answers": { question: [label] } } } });
        self.backend.invoke("respond_agent_request", args);
    }

    fn add_session(&mut self, session: &Value) -> usize {
        let ix = self.conversations.len();
        self.conversations.push(Conversation::from_session(session));
        // A side chat or a pull request's question belongs to its parent, not to the project's list.
        if session["sideChat"].is_object() || session["pullAsk"].is_object() {
            return ix;
        }
        let project = session["projectId"].as_str().unwrap_or("");
        if let Some(project) = self.projects.iter_mut().find(|p| p.id == project) {
            project.conversations.insert(0, ix);
        }
        ix
    }

    fn ensure_loaded(&mut self, ix: usize) {
        let conversation = &self.conversations[ix];
        if conversation.loaded || conversation.session_id.is_empty() {
            return;
        }
        // The newest messages; older ones will page in as the conversation scrolls back.
        let id = self.backend.invoke("transcript_action", json!({ "action": { "type": "load", "sessionId": conversation.session_id, "limit": 120, "lite": true } }));
        self.waiting.insert(id, Waiting::Transcript(ix));
    }

    fn session_updated(&mut self, session: &Value, cx: &mut Context<Self>) {
        let id = session["id"].as_str().unwrap_or("");
        let ix = match self.conversations.iter().position(|c| c.session_id == id) {
            Some(ix) => ix,
            None => self.add_session(session),
        };
        let conversation = &mut self.conversations[ix];
        conversation.apply_meta(session);
        if conversation.loaded {
            if let Some(messages) = session["messages"].as_array().filter(|m| !m.is_empty()) {
                // A reply to a send carries the newest messages and no window: merge them by id.
                let Some(from) = session["transcriptWindow"]["from"]
                    .as_u64()
                    .map(|f| f as usize)
                else {
                    for message in messages {
                        match conversation
                            .messages
                            .iter_mut()
                            .find(|m| m["id"] == message["id"])
                        {
                            Some(held) => *held = message.clone(),
                            None => conversation.messages.push(message.clone()),
                        }
                    }
                    conversation.rebuild();
                    self.observe_queue(ix);
                    cx.notify();
                    return;
                };
                let kept = conversation.base + conversation.messages.len();
                if from > kept {
                    // The window starts past what's here, so messages are missing: load them.
                    conversation.loaded = false;
                    self.ensure_loaded(ix);
                    cx.notify();
                    return;
                }
                if from >= conversation.base {
                    conversation.messages.truncate(from - conversation.base);
                    conversation.messages.extend(messages.iter().cloned());
                }
                // Usually only the newest turn is in the window.
                let at = from.saturating_sub(conversation.base);
                if conversation
                    .messages
                    .iter()
                    .rposition(|m| m["role"] == "user")
                    .is_some_and(|last| last <= at)
                {
                    conversation.rebuild_last();
                } else {
                    conversation.rebuild();
                }
            } else {
                conversation.rebuild_last();
            }
        }
        // A finished reply lets the next queued message start.
        self.observe_queue(ix);
        self.resources_running_changed(cx);
        cx.notify();
    }

    /// Streamed text, applied by its offset so a repeat or a gap can't garble the reply.
    fn agent_output(&mut self, event: &Value) {
        let id = event["sessionId"].as_str().unwrap_or("");
        let Some(ix) = self.conversations.iter().position(|c| c.session_id == id) else {
            return;
        };
        let conversation = &mut self.conversations[ix];
        if !conversation.loaded {
            return;
        }
        let message_id = event["messageId"].as_str().unwrap_or("");
        let at = conversation
            .messages
            .iter()
            .rposition(|m| m["id"].as_str() == Some(message_id));
        if event["message"].is_object() {
            match at {
                Some(at) => {
                    let (old, new) = (
                        conversation.messages[at]["content"]
                            .as_str()
                            .unwrap_or("")
                            .len(),
                        event["message"]["content"].as_str().unwrap_or("").len(),
                    );
                    if new > old {
                        let activity = conversation.messages[at]["activity"].take();
                        let streaming = conversation.messages[at]["streaming"].take();
                        conversation.messages[at] = event["message"].clone();
                        conversation.messages[at]["activity"] = activity;
                        conversation.messages[at]["streaming"] = streaming;
                    }
                }
                None => conversation.messages.push(event["message"].clone()),
            }
        } else if let Some(at) = at {
            let offset = event["offset"].as_u64().unwrap_or(0) as usize;
            let mut text = conversation.messages[at]["content"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            if data::utf16_len(&text) == offset {
                text.push_str(event["chunk"].as_str().unwrap_or(""));
                conversation.messages[at]["content"] = Value::String(text);
            }
        }
        conversation.rebuild_last();
    }

    fn new_conversation(&mut self, project: Option<usize>, cx: &mut Context<Self>) {
        let active = self.active_conversation();
        let project = project
            .or_else(|| {
                self.projects
                    .iter()
                    .position(|p| p.conversations.contains(&active))
            })
            .or(self.project_order.first().copied());
        let Some(project) = project else { return };
        // An open tab that is still empty is already a new conversation: go there, no second one.
        let empty = self.projects[project].conversations.iter().find_map(|&ix| {
            let conversation = &self.conversations[ix];
            let blank = conversation.is_blank() && !conversation.working();
            blank
                .then(|| {
                    self.tabs
                        .iter()
                        .find(|t| t.conversation == ix && !t.closing)
                })
                .flatten()
                .map(|tab| tab.id)
        });
        if let Some(id) = empty {
            self.select(id, cx);
            return;
        }
        let agent = self.conversations[active].provider;
        let model = self.conversations[active].model.clone();
        let id = self.backend.invoke("create_session", json!({ "request": { "projectId": self.projects[project].id, "agent": agent, "isolatedWorktree": false, "model": model } }));
        self.waiting.insert(id, Waiting::Created);
    }

    fn respond(&mut self, ix: usize, request: &Value, decision: &str) {
        let args = json!({ "request": { "sessionId": self.conversations[ix].session_id, "generation": request["generation"], "requestId": request["requestId"], "turnId": request["turnId"], "response": { "type": "approval", "decision": decision } } });
        self.backend.invoke("respond_agent_request", args);
    }

    fn refresh_diff(&self) {
        // This Mac's `git` cannot read a path on another computer.
        if self.on_remote() {
            return;
        }
        let path = self.conversations[self.active_conversation()]
            .worktree
            .clone();
        if path.is_empty() {
            return;
        }
        let tx = self.git_tx.clone();
        std::thread::spawn(move || {
            let stat = backend::git(&path, &["diff", "--shortstat", "HEAD"]);
            let number = |word: &str| {
                stat.split(',')
                    .find(|part| part.contains(word))
                    .and_then(|part| part.trim().split(' ').next()?.parse().ok())
                    .unwrap_or(0)
            };
            let _ = tx.send_blocking(GitResult::Diff(
                path,
                (number("insertion"), number("deletion")),
            ));
        });
    }

    fn refresh_files(&self) {
        if self.on_remote() {
            return;
        }
        let path = self.conversations[self.active_conversation()]
            .worktree
            .clone();
        if path.is_empty() || self.files.0 == path {
            return;
        }
        let tx = self.git_tx.clone();
        std::thread::spawn(move || {
            let listed = backend::git(
                &path,
                &["ls-files", "--cached", "--others", "--exclude-standard"],
            );
            let files = listed
                .lines()
                .take(50_000)
                .map(|line| match line.rsplit_once('/') {
                    Some((folder, name)) => (name.to_owned(), folder.to_owned()),
                    None => (line.to_owned(), String::new()),
                })
                .collect();
            let _ = tx.send_blocking(GitResult::Files(path, files));
        });
    }

    /// A search field, while one is up; otherwise typing goes to the composer's draft.
    fn query_target(&mut self) -> Option<&mut String> {
        // The find bar takes the typing while it is up (transcript_find.rs).
        if self.find.shown() {
            return Some(&mut self.find.query);
        }
        // The settings popup's focused script field takes the typing.
        if self
            .scripts_edit
            .as_ref()
            .is_some_and(|edit| edit.focus.is_some())
        {
            let edit = self.scripts_edit.as_mut()?;
            let field = edit.focus?;
            return Some(edit.draft.field_mut(field));
        }
        if self.edit_target().is_some() {
            return self.edit_target();
        }
        if let Some((_, value)) = self.secret_entry.as_mut() {
            return Some(value);
        }
        if let Some((_, value)) = self.setting_edit.as_mut() {
            return Some(value);
        }
        if let Some(message) = self.commit_message.as_mut() {
            return Some(message);
        }
        if self.env_open == Some("branch") {
            if let Some(draft) = self.branch_draft.as_mut() {
                return Some(draft);
            }
        }
        if let Some(goal) = self.goal_input.as_mut() {
            return Some(goal);
        }
        // Field by field: this returns a borrow of `self`, so no method may borrow all of it.
        let active = self
            .tabs
            .iter()
            .find(|t| t.id == self.active)
            .map_or(0, |t| t.conversation);
        if let Some(c) = self.conversations.get(active) {
            if self.subagents.composing(&c.session_id) {
                return self.subagents.compose_text(&c.session_id);
            }
        }
        if self.usage_page.open.is_some() {
            return Some(&mut self.ignored_text);
        }
        if self.appearance.is_some() {
            return Some(
                self.theme_menu
                    .as_mut()
                    .map_or(&mut self.ignored_text, |m| &mut m.query),
            );
        }
        if matches!(self.composer_menu, Some((ComposerMenu::Catalog, _))) {
            return Some(&mut self.model_query);
        }
        if self.confirm.is_some() || self.viewer.visible() {
            return Some(&mut self.ignored_text);
        }
        if matches!(self.add_menu, Some((AddPage::Skills, _))) {
            return Some(&mut self.add_query);
        }
        if let Some(name) = self.context_menu.as_mut().and_then(|m| m.renaming.as_mut()) {
            return Some(name);
        }
        if self.project_menu.as_ref().is_some_and(|m| m.sub.is_some()) {
            return self.project_menu.as_mut().map(|m| &mut m.query);
        }
        if let Some(name) = self.usage_accounts.as_mut().and_then(|p| p.adding.as_mut()) {
            return Some(name);
        }
        self.palette_ui.as_mut().map(|m| &mut m.query)
    }

    /// Whether a popup's search or name field takes the keys instead of the composer.
    fn grabs_typing(&self) -> bool {
        self.find.shown()
            || self.queues.is_editing()
            || self.subagents.composing(&self.active_session_id())
            || matches!(self.add_menu, Some((AddPage::Skills | AddPage::Goal, _)))
            || self
                .context_menu
                .as_ref()
                .is_some_and(|m| m.renaming.is_some())
            || matches!(self.composer_menu, Some((ComposerMenu::Catalog, _)))
            || self.project_menu.as_ref().is_some_and(|m| m.sub.is_some())
            || self
                .usage_accounts
                .as_ref()
                .is_some_and(|p| p.adding.is_some())
    }

    /// A click on the composer's toolbar hands the keys back to the composer, unless a popup
    /// that takes typing is up (its field keeps the focus).
    fn keep_typing_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.grabs_typing() {
            window.focus(&self.focus, cx);
        }
    }

    fn draft(&mut self) -> &mut Draft {
        self.touch_drafts();
        let ix = self.active_conversation();
        self.drafts.entry(ix).or_default()
    }

    /// Notes that a draft changed: the save timer saves it once the composer is quiet.
    fn touch_drafts(&mut self) {
        self.draft_edited = Some(Instant::now());
    }

    /// Saves the drafts whose text the host does not hold yet, and returns the replies. Conversations
    /// without a session id keep their drafts in memory only.
    fn save_drafts(&mut self) -> Vec<async_channel::Receiver<Result<Value, Value>>> {
        self.draft_edited = None;
        // Every conversation, not only those holding a draft: sending removes the draft, and the
        // empty text is what clears it on the host.
        let changed: Vec<(String, String)> = self
            .conversations
            .iter()
            .enumerate()
            .filter(|(_, conversation)| !conversation.session_id.is_empty())
            .map(|(ix, conversation)| {
                let text = self
                    .drafts
                    .get(&ix)
                    .map_or(String::new(), |d| d.text.clone());
                (draft_key(&conversation.session_id), text)
            })
            .collect();
        let mut replies = Vec::new();
        for (key, text) in changed {
            if self.saved_drafts.get(&key).map(String::as_str) == Some(text.as_str())
                || (text.is_empty() && !self.saved_drafts.contains_key(&key))
            {
                continue;
            }
            self.saved_drafts.insert(key.clone(), text.clone());
            replies.push(
                self.backend
                    .request("save_composer_draft", json!({ "key": key, "value": text })),
            );
        }
        replies
    }

    /// Runs the save timer while a draft is unsaved: it saves once the composer has been quiet.
    fn ensure_draft_timer(&mut self, cx: &mut Context<Self>) {
        if self.draft_timer || self.draft_edited.is_none() {
            return;
        }
        self.draft_timer = true;
        cx.spawn(async move |this, cx| {
            loop {
                let wait = this.update(cx, |this, _| {
                    let delay = this
                        .draft_edited
                        .and_then(|edited| draft_save_delay(edited, Instant::now()));
                    if delay.is_none() {
                        this.save_drafts();
                        this.draft_timer = false;
                    }
                    delay
                });
                match wait {
                    Ok(Some(delay)) => cx.background_executor().timer(delay).await,
                    _ => break,
                }
            }
        })
        .detach();
    }

    /// The active conversation's session id, empty while there is none.
    fn active_session_id(&self) -> String {
        self.conversations
            .get(self.active_conversation())
            .map_or_else(String::new, |c| c.session_id.clone())
    }

    fn active_conversation(&self) -> usize {
        // A pane being built names its own conversation.
        if let Some(building) = self.split.building() {
            return building;
        }
        self.tabs
            .iter()
            .find(|t| t.id == self.active)
            .map_or(0, |t| t.conversation)
    }

    fn select(&mut self, id: u64, cx: &mut Context<Self>) {
        self.leave_title_tab();
        if self.active == id {
            return;
        }
        self.history.truncate(self.history_at + 1);
        self.history.push(id);
        self.history_at = self.history.len() - 1;
        self.show(id, cx);
    }

    fn show(&mut self, id: u64, cx: &mut Context<Self>) {
        self.active = id;
        self.marked = None;
        // A failure in the footer belongs to what was on screen when it happened.
        self.problem = None;
        self.split_sync();
        if let Some(ix) = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.conversation)
        {
            self.ensure_loaded(ix);
        }
        self.refresh_diff();
        self.refresh_usage();
        self.refresh_skills();
        self.refresh_files();
        self.switch = Tween::from_to(0., 1., Instant::now(), SWITCH, Curve::Expo);
        cx.notify();
    }

    /// Back and forward walk the tabs visited, skipping any since closed.
    fn navigate(&mut self, forward: bool, cx: &mut Context<Self>) {
        let mut at = self.history_at as isize;
        loop {
            at += if forward { 1 } else { -1 };
            let Some(id) = usize::try_from(at)
                .ok()
                .and_then(|i| self.history.get(i))
                .copied()
            else {
                return;
            };
            if self.tabs.iter().any(|t| t.id == id && !t.closing) {
                self.history_at = at as usize;
                self.show(id, cx);
                return;
            }
        }
    }

    fn can_navigate(&self, forward: bool) -> bool {
        let live = |id: &u64| self.tabs.iter().any(|t| t.id == *id && !t.closing);
        if forward {
            self.history[(self.history_at + 1).min(self.history.len())..]
                .iter()
                .any(live)
        } else {
            self.history[..self.history_at].iter().any(live)
        }
    }

    fn open(&mut self, conversation: usize, cx: &mut Context<Self>) {
        let project = self.project_of(conversation);
        self.enter_project(project, cx);
        if let Some(id) = self
            .tabs
            .iter()
            .find(|t| t.conversation == conversation && !t.closing)
            .map(|t| t.id)
        {
            self.select(id, cx);
            return;
        }
        let now = Instant::now();
        let id = self.next_id;
        self.next_id += 1;
        // Starts at the end of the strip with no width, then grows.
        let x = self.new_tab_x.at(now).max(0.);
        self.tabs.push(Tab {
            id,
            conversation,
            pinned: false,
            closing: false,
            x: Tween::still(x, now),
            w: Tween::still(0., now),
        });
        self.select(id, cx);
    }

    fn close(&mut self, id: u64, cx: &mut Context<Self>) {
        let live: Vec<u64> = self
            .tabs
            .iter()
            .filter(|t| !t.closing)
            .map(|t| t.id)
            .collect();
        if live.len() <= 1 {
            return;
        }
        if self.active == id {
            // Like a browser: the next tab to the right, then the one to the left.
            let at = live.iter().position(|t| *t == id).unwrap_or(0);
            let next = neighbour_on_close(live.len(), at).and_then(|ix| live.get(ix).copied());
            if let Some(next) = next {
                // A page tab being shown stays shown; only the conversation underneath moves on.
                let page = self.term.pulls_active;
                self.select(next, cx);
                self.term.pulls_active = page;
            }
        }
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.closing = true;
        }
        cx.notify();
    }

    /// A pinned tab leaves the strip for a card in the deck, its plate collapsing where it
    /// was; unpinned, it grows back at the strip's left end.
    fn toggle_pin(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(at) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        let now = Instant::now();
        let mut tab = self.tabs.remove(at);
        if tab.pinned {
            tab.pinned = false;
            tab.x = Tween::still(0., now);
            tab.w = Tween::still(0., now);
        } else {
            let ghost = Tab {
                id: self.next_id,
                conversation: tab.conversation,
                pinned: false,
                closing: true,
                x: tab.x,
                w: tab.w,
            };
            self.next_id += 1;
            self.tabs.insert(at, ghost);
            tab.pinned = true;
        }
        let edge = self
            .tabs
            .iter()
            .position(|t| !t.pinned)
            .unwrap_or(self.tabs.len());
        self.tabs.insert(edge, tab);
        self.drag = None;
        self.card_hovered = None;
        cx.notify();
    }

    /// Pinned tabs stay, as in browsers; the kept tab becomes the active one if it was closed.
    fn close_where(
        &mut self,
        keep: u64,
        closes: impl Fn(usize, usize) -> bool,
        cx: &mut Context<Self>,
    ) {
        let tabs: Vec<(u64, bool)> = self.tabs.iter().map(|t| (t.id, t.pinned)).collect();
        let marks = bulk_close_marks(&tabs, keep, closes);
        let mut closed_active = false;
        for tab in self.tabs.iter_mut().filter(|t| marks.contains(&t.id)) {
            tab.closing = true;
            closed_active |= tab.id == self.active;
        }
        if closed_active {
            self.select(keep, cx);
        }
        cx.notify();
    }

    /// Where each strip tab belongs, measured from the end of the pinned dock; tweens head
    /// there. Returns whether anything still moves.
    fn layout_tabs(&mut self, strip: f32, now: Instant, reduced: bool) -> bool {
        let (open, reorder, fan_time) = if reduced {
            (0., 0., 0.)
        } else {
            (TAB_OPEN, REORDER, FAN)
        };
        let pinned = self.tabs.iter().filter(|t| t.pinned && !t.closing).count();
        let pulls_live = self.term.pulls_tab.as_ref().is_some_and(|t| !t.closing);
        let pulls_shown = pulls_live && self.term.pulls_active;
        let free = (self.tabs.iter().filter(|t| !t.pinned && !t.closing).count()
            + usize::from(pulls_live)) as f32;
        let menu_on_pin = self
            .menu
            .as_ref()
            .is_some_and(|m| self.tabs.iter().any(|t| t.id == m.tab && t.pinned));
        let spread = pinned <= ROW_MAX || self.deck_hovered || menu_on_pin;
        self.fan
            .set(if spread { 1. } else { 0. }, now, fan_time, Curve::Expo);
        let dock = |fan: f32| {
            if pinned > 0 {
                fan_width(&TAB_TILE, pinned, fan) + tab_tile_inset() * 2. + DOCK_GAP * 2.
            } else {
                0.
            }
        };
        // The dock follows the fan exactly while it moves, so the tabs give way with it.
        if self.fan.done(now) {
            self.tray
                .set(dock(self.fan.at(now)), now, reorder, Curve::Expo);
        } else {
            self.tray.snap(dock(self.fan.at(now)), now);
        }
        let room = strip - dock(self.fan.to) - (free - 1.).max(0.) * GAP;
        let even = if free > 0. {
            (room / free).clamp(TAB_MIN, TAB_MAX)
        } else {
            0.
        };
        let active_free = pulls_shown || self.tabs.iter().any(|t| t.id == self.active && !t.pinned);
        let (selected, others) = if active_free && even < SELECTED_MIN && free > 1. {
            (
                SELECTED_MIN,
                ((room - SELECTED_MIN) / (free - 1.)).max(TAB_MIN),
            )
        } else {
            (even, even)
        };
        let mut x = 0.;
        let mut moving = !self.tray.done(now) || !self.fan.done(now);
        let dragging = self.drag.as_ref().filter(|d| d.active).map(|d| (d.id, d.x));
        for tab in &mut self.tabs {
            if tab.pinned {
                continue;
            }
            let width = if tab.closing {
                0.
            } else if tab.id == self.active && !pulls_shown {
                selected
            } else {
                others
            };
            if tab.x.to < 0. {
                tab.x.snap(x, now);
                tab.w.snap(width, now);
            }
            tab.w.set(width, now, open, Curve::Quad);
            match dragging {
                Some((id, at)) if id == tab.id => tab.x.snap(at, now),
                _ => tab.x.set(x, now, reorder, Curve::Expo),
            }
            moving |= !tab.x.done(now) || !tab.w.done(now);
            if !tab.closing {
                x += width + GAP;
            }
        }
        // The page tab follows the session tabs, in the same strip.
        if let Some(page) = self.term.pulls_tab.as_mut() {
            let width = match (page.closing, pulls_shown) {
                (true, _) => 0.,
                (false, true) => selected,
                (false, false) => others,
            };
            if page.x.to < 0. {
                page.x.snap(x, now);
                page.w.snap(width, now);
            }
            page.w.set(width, now, open, Curve::Quad);
            page.x.set(x, now, reorder, Curve::Expo);
            moving |= !page.x.done(now) || !page.w.done(now);
            if !page.closing {
                x += width + GAP;
            }
            if page.closing && page.w.done(now) {
                self.term.pulls_tab = None;
            }
        }
        self.new_tab_x.set(x, now, reorder, Curve::Expo);
        moving |= !self.new_tab_x.done(now);
        self.tabs
            .retain(|t| !(t.closing && (t.pinned || t.w.done(now))));
        moving
    }

    /// The pinned cards, as Monocode's pinned_tabs.rs: up to three in a row, more stacked
    /// behind the last chosen, fanning out when pointed at. Hovered cards show their title.
    fn pinned_dock(
        &mut self,
        now: Instant,
        moving: &mut bool,
        reduced: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pinned: Vec<(u64, usize)> = self
            .tabs
            .iter()
            .filter(|t| t.pinned && !t.closing)
            .map(|t| (t.id, t.conversation))
            .collect();
        if pinned.is_empty() {
            self.deck_front = None;
            return None;
        }
        let count = pinned.len();
        let stacks = count > ROW_MAX;
        let fan = self.fan.at(now);
        let front = pinned.iter().position(|(id, _)| *id == self.active);
        let top = front
            .or_else(|| {
                pinned
                    .iter()
                    .position(|(id, _)| Some(*id) == self.deck_front)
            })
            .unwrap_or(0);
        if self.deck_front != Some(pinned[top].0) {
            let was = self.deck_front.replace(pinned[top].0);
            self.shuffle = was
                .filter(|w| stacks && !reduced && pinned.iter().any(|(id, _)| id == w))
                .map(|w| (w, now));
        }
        if !stacks {
            self.shuffle = None;
        }
        let shuffle = self.shuffle.and_then(|(was, start)| {
            let t = now.duration_since(start).as_secs_f32() / SHUFFLE;
            let was = pinned.iter().position(|(id, _)| *id == was)?;
            (t < 1.).then_some((was, t))
        });
        if shuffle.is_none() {
            self.shuffle = None;
        } else {
            *moving = true;
        }
        let inset = tab_tile_inset();
        let width = fan_width(&TAB_TILE, count, fan) + inset * 2.;
        let hovered_card = self
            .card_hovered
            .filter(|c| pinned.iter().any(|(id, _)| id == c));
        let plate_top = (TITLE_BAR - PLATE) / 2.;
        let places: Vec<FanCard> = (0..count)
            .map(|ix| fan_card(&TAB_TILE, ix, top, fan, shuffle))
            .collect();
        let caption = hovered_card.and_then(|c| {
            let ix = pinned.iter().position(|(id, _)| *id == c)?;
            Some((self.conversations[pinned[ix].1].title.clone(), places[ix].x))
        });
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by(|a, b| places[*b].z.total_cmp(&places[*a].z));
        let mut cards = Vec::new();
        for ix in order {
            let (id, conversation) = pinned[ix];
            let place = &places[ix];
            let selected = id == self.active;
            let hovered = hovered_card == Some(id);
            let (face, mark) = self.tile_parts(&self.conversations[conversation], now);
            cards.push(
                div()
                    .id(("pinned-tab", id))
                    .cursor_pointer()
                    .absolute()
                    .left(px(inset + place.x))
                    .top(px(inset + place.y))
                    .w(px(TAB_TILE.w))
                    .h(px(TAB_TILE.h))
                    .when(place.opacity < 1., |d| d.opacity(place.opacity))
                    .child(fan_tile(&TAB_TILE, place, face, mark))
                    .when(selected, |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(-inset + 0.5))
                                .left(px((TAB_TILE.w - 10.) / 2.))
                                .w(px(10.))
                                .h(px(2.))
                                .rounded_full()
                                .bg(accent()),
                        )
                    })
                    .when(hovered && !selected, |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(-inset + 0.5))
                                .left(px((TAB_TILE.w - 6.) / 2.))
                                .w(px(6.))
                                .h(px(2.))
                                .rounded_full()
                                .bg(content(0.35)),
                        )
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.card_hovered = Some(id);
                        } else if this.card_hovered == Some(id) {
                            this.card_hovered = None;
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let duration = if cx.reduce_motion() { 0. } else { MENU };
                            this.menu = Some(Menu {
                                tab: id,
                                at: event.position,
                                open: Tween::from_to(0., 1., Instant::now(), duration, Curve::Expo),
                            });
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                    .into_any_element(),
            );
        }
        let any_selected = front.is_some();
        let any_hovered = hovered_card.is_some();
        Some(
            div()
                .absolute()
                .left_0()
                .top_0()
                .h(px(TITLE_BAR))
                .w(px(self.tray.at(now)))
                .child(
                    div()
                        .absolute()
                        .right(px(DOCK_GAP))
                        .top(px((TITLE_BAR - 16.) / 2.))
                        .w(px(1.))
                        .h(px(16.))
                        .bg(content(0.1)),
                )
                .child(
                    div()
                        .id("pinned-tab-deck")
                        .absolute()
                        .left_0()
                        .top(px(plate_top))
                        .w(px(width))
                        .h(px(PLATE))
                        .rounded(px(crate::theme::RADIUS_MD))
                        .child(
                            div()
                                .absolute()
                                .size_full()
                                .rounded(px(crate::theme::RADIUS_MD))
                                .bg(content(if any_selected {
                                    0.1
                                } else if any_hovered {
                                    0.07
                                } else {
                                    0.04
                                })),
                        )
                        .children(cards)
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            this.deck_hovered = *hovered;
                            if !*hovered {
                                this.card_hovered = None;
                            }
                            cx.notify();
                        })),
                )
                .children(caption.filter(|_| fan > 0.05).map(|(title, x)| {
                    gpui::deferred(
                        div()
                            .absolute()
                            .left(px(inset + x))
                            .top(px(plate_top + PLATE + 6.))
                            .max_w(px(260.))
                            .h(px(24.))
                            .px_2()
                            .flex()
                            .items_center()
                            .opacity(fan)
                            .rounded(px(crate::theme::RADIUS_SM))
                            .border_1()
                            .border_color(content(0.1))
                            .child(crate::theme::floating_surface(crate::theme::RADIUS_SM))
                            .shadow_md()
                            .text_size(px(12.))
                            .text_color(content(1.))
                            .child(div().min_w_0().truncate().child(title)),
                    )
                    .with_priority(3)
                }))
                .into_any_element(),
        )
    }

    /// A conversation's tile: the spinner on a neutral face while it works, else its model.
    fn tile_parts(&self, conversation: &Conversation, now: Instant) -> (Face, AnyElement) {
        if running(conversation, now) {
            (Face::Neutral, self.spin(now).into_any_element())
        } else {
            (
                Face::provider(conversation.provider),
                provider_glyph(conversation.provider, 14.),
            )
        }
    }

    fn start_drag(&mut self, id: u64, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let now = Instant::now();
        let start = self
            .tabs
            .iter()
            .find(|t| t.id == id)
            .map_or(0., |t| t.x.at(now));
        self.drag = Some(TabDrag {
            id,
            origin: f32::from(event.position.x),
            start,
            x: start,
            active: false,
        });
        self.menu = None;
        self.select(id, cx);
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.split_move(event, cx) {
            return;
        }
        if self.select.dragging() {
            if event.pressed_button != Some(MouseButton::Left) {
                self.select.stop_drag();
            } else if self.select.drag_to(event.position) {
                cx.notify();
            }
            return;
        }
        if self.effort_drag {
            if event.pressed_button != Some(MouseButton::Left) {
                self.effort_drag = false;
                return;
            }
            self.pick_effort_at(f32::from(event.position.x), cx);
            return;
        }
        if self.resizing_dock {
            if event.pressed_button != Some(MouseButton::Left) {
                self.resizing_dock = false;
                return;
            }
            self.dock_width = (self.viewport_width - f32::from(event.position.x) - 8.)
                .clamp(DOCK_MIN, DOCK_MAX_WIDTH);
            cx.notify();
            return;
        }
        if self.resizing {
            if event.pressed_button != Some(MouseButton::Left) {
                self.resizing = false;
                return;
            }
            self.sidebar_width = (f32::from(event.position.x) - 6.).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
            cx.notify();
            return;
        }
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            cx.notify();
            return;
        }
        let delta = f32::from(event.position.x) - drag.origin;
        if delta.abs() < 4. && !drag.active {
            return;
        }
        drag.active = true;
        let now = Instant::now();
        let id = drag.id;
        let Some(from) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        let width = self.tabs[from].w.to;
        let group: Vec<usize> = (0..self.tabs.len())
            .filter(|i| !self.tabs[*i].pinned && !self.tabs[*i].closing)
            .collect();
        let (Some(first), Some(last)) = (group.first().copied(), group.last().copied()) else {
            return;
        };
        let low = self.tabs[first].x.to.min(drag.start);
        let high = self.tabs[last].x.to.max(drag.start);
        let x = (drag.start + delta).clamp(low, high);
        drag.x = x;
        let center = x + width / 2.;
        let mut to = from;
        for ix in &group {
            let other = &self.tabs[*ix];
            if other.id != id && center > other.x.to && center < other.x.to + other.w.to {
                to = *ix;
            }
        }
        if to != from {
            let tab = self.tabs.remove(from);
            self.tabs.insert(to, tab);
        }
        let _ = now;
        cx.notify();
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.split_release(cx);
        if self.select.release() {
            if let Some(url) = self.select.take_link() {
                cx.open_url(&url);
            }
            cx.notify();
        }
        self.effort_drag = false;
        self.resizing = false;
        self.resizing_dock = false;
        if let Some(drag) = self.drag.take() {
            if drag.active {
                let now = Instant::now();
                // The tab eases from the pointer into its slot.
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == drag.id) {
                    tab.x.snap(drag.x, now);
                }
            }
            cx.notify();
        }
    }

    /// While a row of Settings → Atalhos records, the next key is its new shortcut, or a note on
    /// why it cannot be. Escape stops recording.
    fn record_shortcut(&mut self, keys: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(id) = self
            .setting_edit
            .as_ref()
            .and_then(|(field, _)| field.strip_prefix("shortcut:"))
        else {
            return false;
        };
        if keys.key == "escape" {
            self.setting_edit = None;
            cx.notify();
            return true;
        }
        match crate::settings_pages::recorded_shortcut(&self.settings, id, keys) {
            Ok(custom) => {
                self.setting_edit = None;
                self.save_setting("customShortcuts", custom, cx);
            }
            Err(problem) => {
                if let Some((_, note)) = self.setting_edit.as_mut() {
                    *note = problem;
                }
                cx.notify();
            }
        }
        true
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.first_run_keys(&event.keystroke, cx) {
            cx.stop_propagation();
            return;
        }
        if self.record_shortcut(&event.keystroke, cx) {
            cx.stop_propagation();
            return;
        }
        let remapped;
        let keys = match crate::settings_pages::shortcut_route(&self.settings, &event.keystroke) {
            crate::settings_pages::ShortcutRoute::Pass => &event.keystroke,
            crate::settings_pages::ShortcutRoute::Unbound => return,
            crate::settings_pages::ShortcutRoute::As(keystroke) => {
                remapped = keystroke;
                &remapped
            }
        };
        let key = keys.key.as_str();
        // The find bar's keys come first: its Esc closes the bar, never stops the agent.
        if self.find_keys(keys, window, cx) {
            return;
        }
        // A queued message being edited takes its keys first.
        if self.queues.is_editing() && self.queue_keys(keys, window, cx) {
            return;
        }
        if self.subagent_keys(key, cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.goal_input.is_some() && matches!(key, "enter" | "escape" | "backspace") {
            match key {
                "enter" => {
                    self.save_goal();
                }
                "escape" => {
                    self.goal_input = None;
                    self.add_menu = None;
                }
                _ => {
                    if let Some(goal) = self.goal_input.as_mut() {
                        goal.pop();
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.env_open == Some("branch")
            && self.branch_draft.is_some()
            && matches!(key, "enter" | "escape" | "backspace")
        {
            match key {
                "enter" => self.submit_branch(cx),
                "escape" => self.branch_draft = None,
                _ => {
                    if let Some(draft) = self.branch_draft.as_mut() {
                        draft.pop();
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.commit_message.is_some() && matches!(key, "enter" | "escape" | "backspace") {
            match key {
                "enter" => self.commit_now(cx),
                "escape" => self.commit_message = None,
                _ => {
                    if let Some(message) = self.commit_message.as_mut() {
                        message.pop();
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.setting_edit.is_some() && matches!(key, "enter" | "escape" | "backspace") {
            match key {
                "enter" => {
                    let saves = self
                        .setting_edit
                        .as_ref()
                        .is_some_and(|(field, _)| crate::settings_connectors::enter_saves(field));
                    let names = self
                        .setting_edit
                        .as_ref()
                        .is_some_and(|(field, _)| crate::settings_pair::enter_saves(field));
                    self.commit_setting_edit(cx);
                    if names {
                        crate::settings_pair::key(self, key, Shell::run_setting, cx);
                    }
                    if saves {
                        let list = self.settings["connectors"].clone();
                        crate::settings_connectors::key(
                            self,
                            key,
                            &list,
                            Shell::save_setting,
                            Shell::run_setting,
                            cx,
                        );
                    }
                }
                "escape" => self.setting_edit = None,
                _ => {
                    if let Some((_, value)) = self.setting_edit.as_mut() {
                        value.pop();
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.appearance.is_some()
            && self.settings_page == "Conectores"
            && matches!(key, "enter" | "escape")
            && crate::settings_connectors::wants_keys()
        {
            let list = self.settings["connectors"].clone();
            if crate::settings_connectors::key(
                self,
                key,
                &list,
                Shell::save_setting,
                Shell::run_setting,
                cx,
            ) {
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        if self.appearance.is_some()
            && self.settings_page == "Conexões"
            && matches!(key, "enter" | "escape")
            && crate::settings_pair::wants_keys()
            && crate::settings_pair::key(self, key, Shell::run_setting, cx)
        {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.secret_entry.is_some() && matches!(key, "enter" | "escape" | "backspace") {
            match key {
                "enter" => {
                    if let Some((id, value)) = self.secret_entry.take() {
                        if !value.is_empty() {
                            self.respond_secret(&id, Some(value), cx);
                        }
                    }
                }
                "escape" => self.secret_entry = None,
                _ => {
                    if let Some((_, value)) = self.secret_entry.as_mut() {
                        value.pop();
                    }
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if let Some(mention) = self.mention() {
            let handled = match key {
                "down" => {
                    self.mention_selected = (self.mention_selected + 1).min(mention.hits.len() - 1);
                    true
                }
                "up" => {
                    self.mention_selected = self.mention_selected.saturating_sub(1);
                    true
                }
                "enter" | "tab" if !keys.modifiers.shift => {
                    let name = mention.hits[self.mention_selected.min(mention.hits.len() - 1)]
                        .0
                        .clone();
                    self.accept_mention(&name, cx);
                    true
                }
                "escape" => {
                    self.mention_dismissed = Some(mention.start);
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        if matches!(self.add_menu, Some((AddPage::Skills, _))) {
            match key {
                "escape" => {
                    self.add_query.clear();
                    if let Some((page, _)) = self.add_menu.as_mut() {
                        *page = AddPage::Add;
                    }
                }
                "backspace" => {
                    self.add_query.pop();
                }
                "enter" => {
                    if let Some((name, _)) = self.skill_hits().first().cloned() {
                        self.use_skill(&name, cx);
                    }
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.confirm.is_some() {
            match key {
                "enter" => self.answer_confirm(true, cx),
                "escape" => self.answer_confirm(false, cx),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }
        if self.attachment_key(key, keys.modifiers.platform, cx) {
            cx.stop_propagation();
            return;
        }
        if self.context_menu_key(key, cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self
            .context_menu
            .as_ref()
            .is_some_and(|m| m.renaming.is_some())
        {
            return;
        }
        if key == "escape" && self.close_sidebar_options(cx) {
            return;
        }
        if key == "escape" && self.session_filter.open.take().is_some() {
            cx.notify();
            return;
        }
        if self.project_menu_key(key, cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self
            .usage_accounts
            .as_ref()
            .is_some_and(|p| p.adding.is_some())
        {
            match key {
                "escape" => self.usage_accounts.as_mut().unwrap().adding = None,
                "backspace" => {
                    self.usage_accounts
                        .as_mut()
                        .unwrap()
                        .adding
                        .as_mut()
                        .unwrap()
                        .pop();
                }
                "enter" => {
                    let name = self
                        .usage_accounts
                        .as_ref()
                        .and_then(|p| p.adding.clone())
                        .unwrap_or_default();
                    if !name.trim().is_empty() {
                        let provider = self.usage_accounts.as_ref().map_or("", |p| p.provider);
                        let call = self.backend.invoke(
                            "create_provider_account",
                            json!({ "provider": provider, "name": name.trim() }),
                        );
                        self.waiting.insert(call, Waiting::AccountCreated(provider));
                    }
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "escape" && self.usage_accounts.take().is_some() {
            cx.notify();
            return;
        }
        if key == "escape" && self.select.active() {
            self.select.clear();
            cx.notify();
            return;
        }
        if key == "escape" && self.message_actions.more.take().is_some() {
            cx.notify();
            return;
        }
        if key == "escape" && self.usage_popup.take().is_some() {
            cx.notify();
            return;
        }
        if key == "escape" && self.resources.close() {
            cx.notify();
            return;
        }
        if key == "escape" && self.context.close() {
            cx.notify();
            return;
        }
        if let Some((which, _)) = self.composer_menu.as_ref().map(|(w, t)| (*w, *t)) {
            match which {
                ComposerMenu::Catalog => {
                    let count = self.model_rows().len();
                    match key {
                        "escape" => self.composer_menu = None,
                        "backspace" => {
                            self.model_query.pop();
                            self.model_selected = None;
                        }
                        "down" if count > 0 => {
                            self.model_selected =
                                Some(self.model_selected.map_or(0, |at| (at + 1) % count))
                        }
                        "up" if count > 0 => {
                            self.model_selected = Some(
                                self.model_selected
                                    .map_or(count - 1, |at| (at + count - 1) % count),
                            )
                        }
                        "enter" => {
                            let rows = self.model_rows();
                            if let Some(row) = self
                                .model_selected
                                .or((count == 1).then_some(0))
                                .and_then(|at| rows.get(at))
                            {
                                self.pick_model(row.provider, row.id.clone(), cx);
                            }
                        }
                        _ => {
                            self.model_selected = None;
                            cx.notify();
                            return;
                        }
                    }
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                ComposerMenu::Effort if matches!(key, "left" | "right" | "escape") => {
                    if key == "escape" {
                        self.composer_menu = None;
                    } else {
                        let execution = self.current_execution();
                        let at = execution
                            .effort
                            .as_ref()
                            .and_then(|e| execution.levels.iter().position(|l| l == e))
                            .unwrap_or(0);
                        let next = if key == "left" {
                            at.saturating_sub(1)
                        } else {
                            (at + 1).min(execution.levels.len().saturating_sub(1))
                        };
                        self.step_effort(next, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                ComposerMenu::Access if key == "escape" => {
                    self.composer_menu = None;
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }
        if let Some(state) = self.palette_ui.as_mut() {
            let files = state.files;
            let count = {
                let q = state.query.clone();
                self.palette_hits(files, &q).len()
            };
            let state = self.palette_ui.as_mut().unwrap();
            match key {
                "escape" => self.palette_ui = None,
                "down" => state.selected = (state.selected + 1).min(count.saturating_sub(1)),
                "up" => state.selected = state.selected.saturating_sub(1),
                "backspace" => {
                    state.query.pop();
                    state.selected = 0;
                    self.palette_query_changed(cx);
                }
                "enter" => {
                    let (q, at) = (state.query.clone(), state.selected);
                    let hit = self.palette_hits(files, &q).get(at).cloned();
                    if let Some(hit) = hit {
                        self.run_hit(hit, cx);
                    }
                }
                _ if keys.modifiers.platform && keys.modifiers.shift && key == "f" => {
                    self.toggle_global_search(cx)
                }
                _ if keys.modifiers.platform && (key == "k" || key == "p") => {
                    self.palette_ui = None
                }
                _ => {
                    state.selected = 0;
                    cx.notify();
                    return;
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.theme_menu.is_some() {
            let state = self.theme_menu.as_mut().unwrap();
            let needle = state.query.to_lowercase();
            let matches: Vec<usize> = PRESETS
                .iter()
                .enumerate()
                .filter(|(_, p)| t(p.name).to_lowercase().contains(&needle))
                .map(|(ix, _)| ix)
                .collect();
            match key {
                "escape" => self.theme_menu = None,
                "down" => {
                    state.selected = (state.selected + 1).min(matches.len().saturating_sub(1))
                }
                "up" => state.selected = state.selected.saturating_sub(1),
                "backspace" => {
                    state.query.pop();
                    state.selected = 0;
                }
                "enter" => {
                    if let Some(ix) = matches.get(state.selected).copied() {
                        self.pick_theme(ix, cx);
                    }
                }
                _ => {
                    state.selected = 0;
                    cx.notify();
                    return;
                }
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "escape"
            && (self.environment.is_some()
                || self.appearance.is_some()
                || self.usage_page_open()
                || self.menu.is_some())
        {
            self.environment = None;
            self.appearance = None;
            self.close_usage_page(cx);
            self.menu = None;
            cx.notify();
            return;
        }
        // Escape and ⌘. stop the agent, ⌘Enter flips queue/steer, ↑ recalls the last message.
        if self.queue_keys(keys, window, cx) {
            return;
        }
        if matches!(key, "pageup" | "pagedown") {
            let ix = self.active_conversation();
            let state = self.list_state(ix, cx);
            let page = (f32::from(state.viewport_bounds().size.height) - 80.).max(120.);
            state.scroll_by(px(if key == "pageup" { -page } else { page }));
            cx.notify();
            return;
        }
        let (cmd, shift, ctrl) = (
            keys.modifiers.platform,
            keys.modifiers.shift,
            keys.modifiers.control,
        );
        let live: Vec<u64> = self
            .tabs
            .iter()
            .filter(|t| !t.closing)
            .map(|t| t.id)
            .collect();
        match (cmd, shift, ctrl, key) {
            (true, false, false, "q") => cx.quit(),
            (true, false, false, "t") | (true, false, false, "n") => {
                self.new_conversation(None, cx)
            }
            (true, false, false, "w") if self.term.pulls_active => self.close_pull_requests(cx),
            (true, false, false, "w") => self.close(self.active, cx),
            (true, true, false, "p") => self.toggle_pin(self.active, cx),
            (true, false, false, "s") => self.toggle_sidebar(cx),
            (true, false, false, ",") => self.toggle_appearance(cx),
            (true, false, false, "f") => self.open_find(window, cx),
            (true, false, false, "k") => self.toggle_palette(false, cx),
            (true, false, false, "p") => self.toggle_palette(true, cx),
            (true, false, false, "\\") => self.toggle_dock(cx),
            (true, shift, false, "j") => self.term_shortcut(shift, window, cx),
            (true, true, false, "w") => self.close_where(self.active, |_, _| true, cx),
            (true, false, false, "[") => self.navigate(false, cx),
            (true, false, false, "]") => self.navigate(true, cx),
            (true, true, false, "f") => self.toggle_global_search(cx),
            // The frame-time overlay; not listed in Settings → Atalhos.
            (false, true, true, "f") => {
                self.show_stats = !self.show_stats;
                cx.notify();
            }
            (true, false, false, digit)
                if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() && digit != "0" =>
            {
                let index = (digit.as_bytes()[0] - b'1') as usize;
                if let Some(id) = live.get(index) {
                    self.select(*id, cx);
                }
            }
            (false, shift, true, "tab") => {
                let at = live.iter().position(|t| *t == self.active).unwrap_or(0);
                let next = if shift { at + live.len() - 1 } else { at + 1 } % live.len();
                self.select(live[next], cx);
            }
            (false, false, false, "enter") => {
                self.send(cx);
                cx.stop_propagation();
            }
            (false, true, false, "enter") => {
                self.draft().insert("\n");
                cx.stop_propagation();
                cx.notify();
            }
            (
                _,
                _,
                false,
                "backspace" | "delete" | "left" | "right" | "home" | "end" | "up" | "down",
            ) if !keys.modifiers.alt
                || matches!(key, "left" | "right" | "backspace" | "delete") =>
            {
                let word = keys.modifiers.alt;
                let draft = self.draft();
                let at = draft.cursor;
                let target = match key {
                    "backspace" | "left" if cmd => draft.line_start(at),
                    "backspace" | "left" if word => draft.word_left(at),
                    "backspace" | "left" => draft.previous(at),
                    "delete" | "right" if cmd => draft.line_end(at),
                    "delete" | "right" if word => draft.word_right(at),
                    "delete" | "right" => draft.next(at),
                    "home" => draft.line_start(at),
                    "end" => draft.line_end(at),
                    "up" if cmd => 0,
                    "down" if cmd => draft.text.len(),
                    "up" => draft.line_start(at).saturating_sub(1).min(at),
                    _ => draft.line_end(at).saturating_add(1).min(draft.text.len()),
                };
                if matches!(key, "backspace" | "delete") {
                    if draft.anchor == draft.cursor {
                        draft.anchor = target;
                    }
                    draft.insert("");
                } else {
                    let collapsing = !shift && draft.anchor != draft.cursor;
                    let selection = draft.selection();
                    draft.cursor = if collapsing && key == "left" {
                        selection.start
                    } else if collapsing && key == "right" {
                        selection.end
                    } else {
                        target
                    };
                    if !shift {
                        draft.anchor = draft.cursor;
                    }
                }
                cx.stop_propagation();
                cx.notify();
            }
            (true, false, false, "c") if self.copy_selected_text(cx) => {
                cx.stop_propagation();
                cx.notify();
            }
            (true, false, false, "a") if self.select_reply_text(cx) => {
                cx.stop_propagation();
            }
            (true, false, false, "a") => {
                let draft = self.draft();
                draft.anchor = 0;
                draft.cursor = draft.text.len();
                cx.stop_propagation();
                cx.notify();
            }
            (true, false, false, "c" | "x") => {
                let cut = key == "x";
                let draft = self.draft();
                let selected = draft.text[draft.selection()].to_owned();
                if !selected.is_empty() {
                    if cut {
                        draft.insert("");
                    }
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(selected));
                }
                cx.stop_propagation();
                cx.notify();
            }
            (true, false, false, "v") => {
                let clipboard = cx.read_from_clipboard();
                let image = clipboard.as_ref().and_then(|item| {
                    item.entries().iter().find_map(|entry| match entry {
                        gpui::ClipboardEntry::Image(image) => {
                            Some((image.bytes.clone(), image.format))
                        }
                        _ => None,
                    })
                });
                // Files copied in Finder paste as attachments.
                let files: Vec<std::path::PathBuf> =
                    clipboard.as_ref().map_or_else(Vec::new, |item| {
                        item.entries()
                            .iter()
                            .filter_map(|entry| match entry {
                                gpui::ClipboardEntry::ExternalPaths(paths) => {
                                    Some(paths.paths().to_vec())
                                }
                                _ => None,
                            })
                            .flatten()
                            .collect()
                    });
                if !files.is_empty() {
                    self.attach_files(files, cx);
                } else if let Some((bytes, kind)) = image {
                    self.attach_pasted_image(bytes, kind, cx);
                } else if let Some(text) = clipboard.and_then(|item| item.text()) {
                    match self.query_target() {
                        Some(query) => query.push_str(&text),
                        None => self.draft().insert(&text),
                    }
                    self.palette_query_changed(cx);
                }
                cx.stop_propagation();
                cx.notify();
            }
            _ => {}
        }
    }

    // Sidebar.

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        self.peek = None;
        self.peek_close = None;
        let duration = if cx.reduce_motion() { 0. } else { SIDEBAR };
        self.sidebar.set(
            if self.sidebar_open { 1. } else { 0. },
            Instant::now(),
            duration,
            Curve::Expo,
        );
        cx.notify();
    }

    fn layout_rows(&mut self, now: Instant, reduced: bool) -> (Vec<(Row, f32)>, f32, bool) {
        let duration = if reduced { 0. } else { DISCLOSE };
        let mut wanted: Vec<(Row, f32, f32)> = Vec::new();
        let mut y = 4.;
        if let Some(project) = self.current_project {
            let mut listed: Vec<(usize, i64)> = self.projects[project]
                .conversations
                .iter()
                .copied()
                .filter(|c| self.in_sidebar_list(project, *c))
                .map(|c| (c, days_ago(&self.conversations[c])))
                .collect();
            listed.sort_by_key(|(_, days)| date_group(*days));
            let mut group = None;
            for (conversation, days) in listed {
                let this_group = date_group(days);
                if group != Some(this_group) {
                    group = Some(this_group);
                    wanted.push((Row::Heading(this_group), y, y));
                    y += HEADING;
                }
                wanted.push((Row::Conversation(conversation), y, y));
                y += CONV_ROW;
            }
        }
        let mut moving = false;
        let mut seen: Vec<Row> = Vec::new();
        for (row, target, origin) in wanted {
            seen.push(row);
            let motion = self.rows.entry(row).or_insert_with(|| RowMotion {
                y: Tween::still(origin, now),
                alpha: if now.duration_since(self.epoch).as_millis() < 50 {
                    Tween::still(1., now)
                } else {
                    Tween::still(0., now)
                },
            });
            if now.duration_since(self.epoch).as_millis() < 50 {
                motion.y.snap(target, now);
            }
            motion.y.set(target, now, duration, Curve::Expo);
            motion.alpha.set(1., now, duration, Curve::Quad);
        }
        self.rows.retain(|row, motion| {
            if !seen.contains(row) {
                motion.alpha.set(0., now, duration, Curve::Quad);
                if motion.alpha.done(now) {
                    return false;
                }
            }
            moving |= !motion.y.done(now) || !motion.alpha.done(now);
            true
        });
        let mut drawn: Vec<(Row, f32)> = self
            .rows
            .keys()
            .map(|row| (*row, self.rows[row].y.at(now)))
            .collect();
        // Fading rows paint first, under the rows that stay.
        drawn.sort_by(|a, b| {
            self.rows[&a.0]
                .alpha
                .to
                .total_cmp(&self.rows[&b.0].alpha.to)
                .then(a.1.total_cmp(&b.1))
        });
        (drawn, y, moving)
    }

    /// Enter and the send button: sends, queues or steers (composer_queue.rs).
    fn send(&mut self, cx: &mut Context<Self>) {
        self.problem = None;
        self.send_with(false, cx);
    }

    fn respond_secret(&mut self, id: &str, value: Option<String>, cx: &mut Context<Self>) {
        self.backend.invoke(
            "secret_action",
            json!({ "action": { "type": "respond", "requestId": id, "value": value } }),
        );
        self.secret_entry = None;
        self.secrets.retain(|r| r["id"].as_str() != Some(id));
        cx.notify();
    }

    /// Sends files to the back end, which keeps them for this conversation's next message.
    pub(crate) fn attach_paths(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.attach_files(paths, cx);
    }

    fn skill_hits(&self) -> Vec<(String, String)> {
        let off: Vec<&str> = self.settings["disabledSkills"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let needle = self.add_query.trim().to_lowercase();
        self.skills
            .iter()
            .filter(|(name, about)| {
                !off.contains(&name.as_str())
                    && (needle.is_empty()
                        || name.to_lowercase().contains(&needle)
                        || about.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect()
    }

    /// Adds `/name` at the start of the draft, once, as Sirus Code's `insertSkillInvocation`.
    fn use_skill(&mut self, name: &str, cx: &mut Context<Self>) {
        let token = format!("/{name}");
        let draft = self.draft();
        let leading = draft
            .text
            .split_whitespace()
            .take_while(|t| t.starts_with('/'))
            .any(|t| t.eq_ignore_ascii_case(&token));
        if !leading {
            draft.text = format!("{token} {}", draft.text);
            let end = draft.text.len();
            draft.cursor = end;
            draft.anchor = end;
        }
        self.add_menu = None;
        self.add_query.clear();
        cx.notify();
    }

    fn save_goal(&mut self) {
        let goal = self.goal_input.take().unwrap_or_default();
        let ix = self.active_conversation();
        self.conversations[ix].goal =
            (!goal.trim().is_empty()).then(|| goal.trim().chars().take(400).collect());
        self.add_menu = None;
    }

    fn pick_attachments(&mut self, cx: &mut Context<Self>) {
        if self.this_mac_only(cx) {
            return;
        }
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |this, cx| this.attach_paths(paths, cx));
            }
        })
        .detach();
    }

    /// The microphone: records on the back end; stopping brings the text into the composer.
    /// Screenshots a window the person picks, then attaches it (`screencapture -iw`).
    fn attach_window(&mut self, cx: &mut Context<Self>) {
        if self.this_mac_only(cx) {
            return;
        }
        let path = std::env::temp_dir().join(format!("sirus-janela-{}.png", uuid()));
        let (tx, rx) = async_channel::bounded(1);
        let shot = path.clone();
        std::thread::spawn(move || {
            let done = std::process::Command::new("screencapture")
                .args(["-iw", "-o"])
                .arg(&shot)
                .status()
                .is_ok_and(|s| s.success())
                && shot.exists();
            let _ = tx.send_blocking(done);
        });
        cx.spawn(async move |this, cx| {
            if rx.recv().await.unwrap_or(false) {
                let _ = this.update(cx, |this, cx| this.attach_paths(vec![path], cx));
            }
        })
        .detach();
    }

    /// The composer's + menu, as Sirus Code's: attach, skills, goal, planning, debug and team.
    fn add_menu_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (page, open) = self.add_menu.as_ref()?;
        let page = *page;
        if !open.done(now) {
            *moving = true;
        }
        let t = open.at(now).clamp(0., 1.);
        let conversation = &self.conversations[self.active_conversation()];
        let (planning, debugging, team) = (
            conversation.planning,
            conversation.debugging,
            conversation.team,
        );
        // As Sirus Code's `supportsPlanning` and its team check.
        let provider = conversation.provider;
        let planning_ok = !matches!(provider, "grok" | "antigravity")
            && (provider != "codex" || conversation.model.is_some());
        let team_ok = matches!(provider, "claude" | "codex" | "opencode");
        let caret = move || {
            div()
                .flex_shrink_0()
                .w(px(1.5))
                .h(px(15.))
                .rounded_full()
                .bg(crate::theme::caret_color())
        };
        let track = crate::hover_track::list("add-menu");
        let item = |id: &'static str,
                    glyph: &'static str,
                    title: &'static str,
                    about: &'static str,
                    on: bool| {
            track.row_named(
                div()
                    .id(id)
                    .h(px(32.))
                    .px(px(8.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(13.))
                    .text_color(content(0.92))
                    .cursor_pointer()
                    .when(on, |d| d.bg(accent_wash(0.14)))
                    .child(icon(glyph, 15.).text_color(if on { accent() } else { content(0.6) }))
                    .child(div().flex_shrink_0().child(title))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(content(0.45))
                            .child(about),
                    ),
                id,
                on,
            )
        };
        let back = |cx: &mut Context<Self>| {
            div()
                .id("add-back")
                .mb(px(4.))
                .h(px(26.))
                .px(px(8.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(px(12.5))
                .text_color(content(0.55))
                .cursor_pointer()
                .hover(|s| s.text_color(content(0.95)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.goal_input = None;
                    this.add_query.clear();
                    if let Some((page, _)) = this.add_menu.as_mut() {
                        *page = AddPage::Add;
                    }
                    cx.notify();
                }))
                .child(icon("chevron-left", 14.).text_color(content(0.55)))
                .child(crate::i18n::t("Adicionar"))
                .into_any_element()
        };
        let body: Vec<AnyElement> = if page == AddPage::Goal {
            let text = self.goal_input.clone().unwrap_or_default();
            vec![
                back(cx),
                div()
                    .px(px(8.))
                    .pb(px(6.))
                    .text_size(px(13.))
                    .text_color(content(0.7))
                    .child(crate::i18n::t("Defina uma meta a perseguir"))
                    .into_any_element(),
                div()
                    .min_h(px(66.))
                    .mx(px(8.))
                    .p(px(8.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .border_1()
                    .border_color(content(0.14))
                    .bg(field_fill())
                    .text_size(px(13.))
                    .flex()
                    .items_start()
                    .child(typed_text(
                        &text,
                        crate::i18n::t("Escreva a meta e aperte Enter"),
                        caret(),
                    ))
                    .into_any_element(),
                div()
                    .mx(px(8.))
                    .mt(px(6.))
                    .text_size(px(11.5))
                    .text_color(content(0.45))
                    .child(crate::i18n::t(
                        "Aplicada ao enviar e mantida nos seus próximos envios manuais.",
                    ))
                    .into_any_element(),
                div()
                    .id("goal-save")
                    .mx(px(8.))
                    .mt(px(8.))
                    .mb(px(6.))
                    .w(px(72.))
                    .h(px(28.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .bg(content(0.09))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.5))
                    .text_color(content(0.9))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.14)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.save_goal();
                        cx.notify();
                    }))
                    .child(crate::i18n::t("Salvar"))
                    .into_any_element(),
            ]
        } else if page == AddPage::Skills {
            let hits = self.skill_hits();
            // The list scrolls, so it has its own pill, clipped with it.
            let skills = crate::hover_track::list("add-skills");
            let list: Vec<AnyElement> = hits
                .iter()
                .enumerate()
                .map(|(at, (name, about))| {
                    let chosen = name.clone();
                    skills
                        .row(
                            div()
                                .id(("skill-pick", at))
                                .px(px(8.))
                                .py(px(6.))
                                .rounded(px(crate::theme::RADIUS_MD))
                                .flex()
                                .items_start()
                                .gap(px(8.))
                                .cursor_pointer()
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.use_skill(&chosen, cx)),
                                )
                                .child(
                                    div()
                                        .mt(px(2.))
                                        .child(icon("box", 13.).text_color(content(0.5))),
                                )
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
                                                .text_color(content(0.95))
                                                .child(name.clone()),
                                        )
                                        .child(
                                            div()
                                                .mt(px(1.))
                                                .text_size(px(11.5))
                                                .line_clamp(2)
                                                .text_color(content(0.45))
                                                .child(about.clone()),
                                        ),
                                ),
                            at,
                            false,
                        )
                        .into_any_element()
                })
                .collect();
            let empty = list.is_empty();
            vec![
                back(cx),
                div()
                    .px(px(8.))
                    .pb(px(6.))
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .child(crate::i18n::t(
                        "Selecione um fluxo para adicionar /nome-da-skill ao rascunho.",
                    ))
                    .into_any_element(),
                div()
                    .mx(px(8.))
                    .mb(px(6.))
                    .h(px(32.))
                    .px(px(8.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .border_1()
                    .border_color(content(0.14))
                    .bg(field_fill())
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(13.))
                    .child(icon("search", 13.).text_color(content(0.45)))
                    .child(typed_text(
                        &self.add_query,
                        crate::i18n::t("Buscar skills"),
                        caret(),
                    ))
                    .into_any_element(),
                div()
                    .id("skill-list")
                    .max_h(px(256.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(skills.pill())
                    .when(empty, |d| {
                        d.child(
                            div()
                                .p(px(8.))
                                .text_size(px(12.))
                                .text_color(content(0.45))
                                .child(crate::i18n::t("Nenhuma skill encontrada")),
                        )
                    })
                    .children(list)
                    .into_any_element(),
            ]
        } else {
            vec![
                div()
                    .px(px(8.))
                    .pt(px(3.))
                    .pb(px(3.))
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .child(crate::i18n::t("Adicionar"))
                    .into_any_element(),
                item(
                    "add-files",
                    "paperclip",
                    crate::i18n::t("Arquivos e pastas"),
                    "",
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.add_menu = None;
                    this.pick_attachments(cx);
                }))
                .into_any_element(),
                item(
                    "add-window",
                    "window",
                    crate::i18n::t("Anexar janela"),
                    crate::i18n::t("Capturar uma janela aberta"),
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.add_menu = None;
                    this.attach_window(cx);
                }))
                .into_any_element(),
                item(
                    "add-skill",
                    "box",
                    crate::i18n::t("Usar uma skill"),
                    "",
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.add_query.clear();
                    this.refresh_skills();
                    if let Some((page, _)) = this.add_menu.as_mut() {
                        *page = AddPage::Skills;
                    }
                    cx.notify();
                }))
                .into_any_element(),
                item(
                    "add-goal",
                    "goal",
                    crate::i18n::t("Meta"),
                    crate::i18n::t("Defina uma meta a perseguir"),
                    conversation.goal.is_some(),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let ix = this.active_conversation();
                    this.goal_input = Some(this.conversations[ix].goal.clone().unwrap_or_default());
                    if let Some((page, _)) = this.add_menu.as_mut() {
                        *page = AddPage::Goal;
                    }
                    cx.notify();
                }))
                .into_any_element(),
                item(
                    "add-plan",
                    "bulb",
                    crate::i18n::t("Modo planejamento"),
                    crate::i18n::t("Analisar e propor antes de editar"),
                    planning,
                )
                .when(!planning_ok, |d| d.opacity(0.4))
                .when(planning_ok, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| {
                        let ix = this.active_conversation();
                        let on = !this.conversations[ix].planning;
                        this.conversations[ix].planning = on;
                        if on {
                            // Planning never runs with full access, as in Sirus Code.
                            this.conversations[ix].debugging = false;
                            if this.conversations[ix].approval == "full" {
                                this.conversations[ix].approval =
                                    if this.conversations[ix].provider == "cursor" {
                                        "auto"
                                    } else {
                                        "ask"
                                    }
                                    .to_owned();
                            }
                        }
                        this.add_menu = None;
                        cx.notify();
                    }))
                })
                .into_any_element(),
                item(
                    "add-debug",
                    "bug",
                    crate::i18n::t("Modo debug"),
                    crate::i18n::t("Ativar modo debug"),
                    debugging,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let ix = this.active_conversation();
                    let on = !this.conversations[ix].debugging;
                    this.conversations[ix].debugging = on;
                    if on {
                        this.conversations[ix].planning = false;
                        this.conversations[ix].team = false;
                    }
                    this.add_menu = None;
                    cx.notify();
                }))
                .into_any_element(),
                item(
                    "add-team",
                    "users",
                    crate::i18n::t("Equipe"),
                    crate::i18n::t("Vários agentes trabalhando juntos"),
                    team,
                )
                .when(!team_ok, |d| d.opacity(0.4))
                .when(team_ok, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| {
                        let ix = this.active_conversation();
                        let on = !this.conversations[ix].team;
                        this.conversations[ix].team = on;
                        if on {
                            this.conversations[ix].planning = false;
                            this.conversations[ix].debugging = false;
                        }
                        this.add_menu = None;
                        cx.notify();
                    }))
                })
                .into_any_element(),
            ]
        };
        // The project's scripts run from the add menu's main page too (ADR-078).
        let body = if page == AddPage::Add {
            let mut body = body;
            body.extend(self.script_run_rows(cx));
            body
        } else {
            body
        };
        Some(
            div()
                .id("add-menu")
                .occlude()
                .absolute()
                .bottom(px(self.above_composer().2 + (1. - t) * -6.))
                .left(px(self.above_composer().0))
                .w(px(self.above_composer().1))
                .p(px(4.))
                .opacity(t)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(track.pill())
                .children(body)
                .into_any_element(),
        )
    }

    fn toggle_dictation(&mut self, cx: &mut Context<Self>) {
        if self.dictating.take().is_some() {
            let id = self
                .backend
                .invoke("stop_dictation", json!({ "cancel": false }));
            self.waiting.insert(id, Waiting::Dictated);
        } else {
            self.backend
                .invoke("start_dictation", json!({ "locale": crate::i18n::code() }));
            self.dictating = Some(Instant::now());
        }
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.stop_agent(cx);
    }

    // Drawing.

    fn tab(&self, tab: &Tab, now: Instant, cx: &mut Context<Self>) -> AnyElement {
        let id = tab.id;
        let conversation = &self.conversations[tab.conversation];
        let active = id == self.active && !tab.closing && !self.term.pulls_active;
        let group: gpui::SharedString = format!("tab{id}").into();
        let width = tab.w.at(now);
        let dragged = self.drag.as_ref().is_some_and(|d| d.active && d.id == id);
        let ink = if active { 1. } else { 0.5 };
        let (face, glyph) = self.tile_parts(conversation, now);
        let mark = mark_tile(&TAB_TILE, face, glyph);
        let closable = active || width >= 72.;
        let title = ((width - PIN_WIDTH) / 60.).clamp(0., 1.);
        let draft = tab_has_draft(&self.drafts, tab.conversation);
        div()
            .id(("tab", id))
            .group(group.clone())
            .absolute()
            .left(px(tab.x.at(now)))
            .top(px((TITLE_BAR - PLATE) / 2.))
            .w(px(width))
            .h(px(PLATE))
            .rounded(px(crate::theme::RADIUS_MD))
            .overflow_hidden()
            .flex()
            .items_center()
            .pl(px(tab_tile_inset()))
            .gap(px(8.))
            .bg(content(if active { 0.1 } else { 0.04 }))
            .when(!active, |d| d.hover(|s| s.bg(content(0.07))))
            .when(dragged, |d| d.bg(content(0.14)).shadow_md())
            .when(tab.closing, |d| {
                d.opacity((width / PIN_WIDTH).clamp(0., 1.))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.start_drag(id, event, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let duration = if cx.reduce_motion() { 0. } else { MENU };
                    this.menu = Some(Menu {
                        tab: id,
                        at: event.position,
                        open: Tween::from_to(0., 1., Instant::now(), duration, Curve::Expo),
                    });
                    cx.notify();
                }),
            )
            .child(mark)
            // A conversation with a composer draft dashes its tab, as Monocode's title tabs do.
            // Overlaid so the dashes take no room from the content.
            .when(draft, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .rounded(px(crate::theme::RADIUS_MD))
                        .border_1()
                        .border_dashed()
                        .border_color(content(if active { 0.3 } else { 0.25 })),
                )
            })
            // The title keeps its width while the plate narrows, so pinning clips it instead of rewrapping.
            .when(title > 0., |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(20.))
                        .mr(px(if closable { 24. } else { 8. }))
                        .opacity(title)
                        .text_size(px(13.))
                        .text_color(content(ink))
                        .group_hover(group.clone(), |s| s.text_color(content(1.)))
                        .child(tab_label(conversation.title.clone())),
                )
            })
            .when(closable, |d| {
                d.child(
                    div()
                        .id(("close", id))
                        .absolute()
                        .right(px(4.))
                        .top(px(4.5))
                        .size(px(18.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(!active, |d| {
                            d.opacity(0.).group_hover(group, |s| s.opacity(1.))
                        })
                        .hover(|s| s.bg(content(0.12)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| this.close(id, cx)))
                        .child(icon("close", 12.).text_color(content(0.6))),
                )
            })
            .into_any_element()
    }

    fn conversation_mark(
        &self,
        conversation: &Conversation,
        now: Instant,
        lit: bool,
    ) -> AnyElement {
        let box_ = div()
            .size(px(14.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center();
        if running(conversation, now) {
            let fraction = (now.duration_since(self.epoch).as_secs_f32() / SPINNER_PERIOD).fract();
            box_.child(spinner_at(fraction)).into_any_element()
        } else {
            mark_tile(
                &crate::tab_tile::TileSize {
                    w: 16.,
                    h: 14.,
                    radius: 4.,
                },
                Face::provider(conversation.provider),
                provider_glyph(conversation.provider, 10.),
            )
            .opacity(if lit { 1. } else { 0.7 })
            .into_any_element()
        }
    }

    fn titlebar(
        &mut self,
        width: f32,
        now: Instant,
        moving: &mut bool,
        reduced: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The tabs start where the card does: traffic lights and arrows keep to the sidebar's
        // column, and the arrows follow its edge as it slides.
        let nav_end = (self.sidebar_width * self.sidebar.at(now)).max(NAV);
        let strip_left = nav_end + 6.;
        let open_dock = self.dock.at(now);
        // With the side panel open the tabs stop at its edge, marked by a short rule; its own
        // options sit in the bar above it, as in Monocode.
        let room = width - self.sidebar_width * self.sidebar.at(now) - 12.;
        let dock_width =
            (self.dock_width + (room - self.dock_width) * self.dock_max.at(now)) * open_dock;
        // Exactly over the card's hairline: gutter, card border, then the panel.
        let dock_edge = (width - 6. - 1. - dock_width - 1.).max(nav_end + 6.);
        let controls = (80. + 28. * open_dock + 72.).max(width - dock_edge - 16. + 6.);
        let (added, removed) = self.diff_stat();
        let (add_color, remove_color) = diff_colors(self.look.diff);
        let strip =
            width - strip_left - 12. - PLATE - 6. - controls - 10. - self.term.chips_width();
        *moving |= self.layout_tabs(strip, now, reduced);
        let tabs: Vec<AnyElement> = {
            // Pinned tabs live in the dock, not the strip.
            let mut order: Vec<usize> = (0..self.tabs.len())
                .filter(|ix| !self.tabs[*ix].pinned)
                .collect();
            // The dragged tab paints over its neighbours.
            let dragged = self.drag.as_ref().filter(|d| d.active).map(|d| d.id);
            order.sort_by_key(|ix| Some(self.tabs[*ix].id) == dragged);
            let mut out = Vec::new();
            for ix in order {
                let tab = &self.tabs[ix];
                out.push(self.tab(tab, now, cx));
            }
            out.extend(self.pulls_tab_view(now, cx));
            out
        };
        let dock = self.pinned_dock(now, moving, reduced, cx);
        let tray = self.tray.at(now);
        let add_x = self.new_tab_x.at(now) + tray;
        let term_chips = self.term_chips(strip_left + strip + PLATE + 8., cx);
        div()
            .h(px(TITLE_BAR))
            .flex_shrink_0()
            .relative()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |event, window, cx| {
                        cx.stop_propagation();
                        if event.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .child(
                icon_button("toggle-sidebar", "sidebar", 16.)
                    .absolute()
                    .left(px(78.))
                    .top(px(8.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.set_peek(true, cx);
                        }
                    }))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
            )
            .child(
                div()
                    .absolute()
                    .right(px(10.))
                    .top(px(8.))
                    .flex()
                    .gap(px(4.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(git_sync::pill(
                        git_sync::shown(self.incoming.value.as_ref()),
                        self.incoming.pulling,
                        cx.listener(|this, _, _, cx| this.incoming_toggle(cx)),
                    ))
                    .child(
                        div()
                            .id("top-diff")
                            .h(px(24.))
                            .px(px(6.))
                            .rounded(px(crate::theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.07)))
                            .on_click(cx.listener(|this, _, _, cx| this.open_pane(2, cx)))
                            .child(div().text_color(add_color).child(format!("+{added}")))
                            .child(div().text_color(remove_color).child(format!("-{removed}"))),
                    )
                    .child(
                        icon_button("top-search", "search", 16.)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_palette(true, cx))),
                    )
                    .child(
                        toggled(
                            icon_button("top-environment", "window", 16.),
                            self.environment.is_some(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.appearance = None;
                            this.environment = match this.environment {
                                Some(_) => None,
                                None => Some(Tween::from_to(
                                    0.,
                                    1.,
                                    Instant::now(),
                                    if cx.reduce_motion() { 0. } else { MENU },
                                    Curve::Expo,
                                )),
                            };
                            cx.notify();
                        })),
                    )
                    .when(open_dock > 0.01, |d| {
                        d.child(
                            toggled(
                                icon_button("dock-max", "expand", 14.),
                                self.dock_max.to > 0.5,
                            )
                            .opacity(open_dock)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let to = if this.dock_max.to > 0.5 { 0. } else { 1. };
                                this.dock_max.set(
                                    to,
                                    Instant::now(),
                                    if cx.reduce_motion() { 0. } else { SIDEBAR },
                                    Curve::Expo,
                                );
                                cx.notify();
                            })),
                        )
                    })
                    .child(
                        toggled(
                            icon_button("top-panel", "sidebar-right", 16.),
                            self.dock_open,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_dock(cx))),
                    ),
            )
            .when(open_dock > 0.01, |d| {
                let active_pane = self.dock_active;
                d.child(
                    div()
                        .absolute()
                        .left(px(dock_edge))
                        .top(px((TITLE_BAR - 16.) / 2.))
                        .w(px(1.))
                        .h(px(16.))
                        .bg(content(0.14))
                        .opacity(open_dock),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(dock_edge + 8.))
                        .top(px(7.))
                        .h(px(26.))
                        .opacity(open_dock)
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .children(PANES.iter().take(VISIBLE_PANES).enumerate().map(
                            |(ix, (glyph, name, _))| {
                                let chosen = active_pane == Some(ix);
                                div()
                                    .id(("pane-tab", ix))
                                    .h(px(26.))
                                    .px(px(6.))
                                    .rounded(px(crate::theme::RADIUS_MD))
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .text_size(px(12.5))
                                    .text_color(content(0.95))
                                    .when(chosen, |d| d.bg(content(0.1)))
                                    .when(!chosen, |d| d.hover(|s| s.bg(content(0.07))))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.dock_active == Some(ix) {
                                            this.dock_active = None;
                                            this.chooser_since = Instant::now();
                                            cx.notify();
                                        } else {
                                            this.open_pane(ix, cx);
                                        }
                                    }))
                                    .child(icon(glyph, 15.).text_color(content(if chosen {
                                        0.95
                                    } else {
                                        0.5
                                    })))
                                    .when(chosen, |d| d.child(crate::i18n::t(*name)))
                            },
                        )),
                )
            })
            .child(nav_button(
                "back",
                nav_end - 54.,
                0.5,
                self.can_navigate(false),
                cx,
                false,
            ))
            .child(nav_button(
                "forward",
                nav_end - 28.,
                0.,
                self.can_navigate(true),
                cx,
                true,
            ))
            .children(term_chips)
            .child(
                div()
                    .absolute()
                    .left(px(strip_left))
                    .top_0()
                    .bottom_0()
                    .w(px((strip + PLATE + 4.).max(0.)))
                    .children(dock)
                    .child(
                        div()
                            .absolute()
                            .left(px(tray))
                            .top_0()
                            .bottom_0()
                            .right_0()
                            .overflow_hidden()
                            .children(tabs),
                    )
                    .child(
                        icon_button("new-tab", "plus", 14.)
                            .absolute()
                            .left(px(add_x))
                            .top(px((TITLE_BAR - PLATE) / 2.))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.term_plus_click(strip_left + add_x, cx) {
                                    return;
                                }
                                this.new_conversation(None, cx);
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn sidebar_view(
        &mut self,
        now: Instant,
        moving: &mut bool,
        reduced: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.peek.as_ref().is_some_and(|p| !p.open && p.t.done(now)) {
            self.peek = None;
        }
        if self
            .sidebar_options
            .is_some_and(|t| t.to < 0.5 && t.done(now))
        {
            self.sidebar_options = None;
        }
        let open = self.sidebar.at(now);
        *moving |= !self.sidebar.done(now);
        let (rows, height, rows_moving) = self.layout_rows(now, reduced);
        *moving |= rows_moving;
        let width = self.sidebar_width;
        let active = self.active_conversation();
        let mut drawn = Vec::new();
        for (row, y) in rows {
            let alpha = self.rows[&row].alpha.at(now);
            drawn.push(match row {
                Row::Heading(group) => div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(y))
                    .h(px(HEADING))
                    .opacity(alpha)
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .text_size(px(11.5))
                    .text_color(content(0.42))
                    .child(crate::i18n::t(DATE_GROUPS[group as usize]))
                    .into_any_element(),
                Row::Conversation(index) => {
                    self.conversation_row(index, y, alpha, index == active, now, cx)
                }
            });
        }
        let contents = |id: &'static str,
                        this: &Self,
                        cx: &mut Context<Self>,
                        drawn: Vec<AnyElement>| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(width))
                .flex()
                .flex_col()
                .child(this.project_header(cx))
                .child(
                    div()
                        .relative()
                        .flex_shrink_0()
                        .child(crate::hover_track::list("sidebar-nav").pill())
                        .children(this.sidebar_new(cx))
                        .children(this.sidebar_pulls(cx)),
                )
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id(id)
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .on_scroll_wheel(
                                    cx.listener(|this, _, _, cx| this.sidebar_card_scrolled(cx)),
                                )
                                .child(
                                    div()
                                        .relative()
                                        .w_full()
                                        .h(px(height + 12.))
                                        .child(crate::hover_track::list("sidebar").pill())
                                        .children(drawn),
                                ),
                        )
                        .child(this.filter_button(cx)),
                )
        };
        // A collapsed sidebar slides out over the workspace while the pointer holds it there.
        if open < 0.001 {
            let Some(peek) = self.peek.as_ref() else {
                return div().into_any_element();
            };
            let t = peek.t.at(now).clamp(0., 1.);
            *moving |= !peek.t.done(now);
            return div()
                .id("sidebar-peek")
                .occlude()
                .absolute()
                .left(px(PEEK_INSET - (1. - t) * PEEK_DRIFT))
                .top(px(TITLE_BAR))
                .bottom(px(PEEK_INSET))
                .w(px(width))
                .opacity(t)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.1))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .overflow_hidden()
                .child(contents("sidebar-list", self, cx, drawn))
                .into_any_element();
        }
        div()
            .relative()
            .flex_shrink_0()
            .h_full()
            .w(px(width * open))
            .overflow_hidden()
            // The content keeps its width while the column slides, so nothing rewraps.
            .child(contents("sidebar-list", self, cx, drawn).opacity(open.clamp(0., 1.)))
            .child(
                div()
                    .id("sidebar-resize")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(px(5.))
                    .cursor_col_resize()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.resizing = true;
                        }),
                    ),
            )
            .into_any_element()
    }

    /// Whether the project's sidebar list shows this conversation (its checkout and filters pass).
    fn in_sidebar_list(&self, project: usize, conversation: usize) -> bool {
        let conversation = &self.conversations[conversation];
        self.checkout_filter
            .get(&project)
            .is_none_or(|wanted| *wanted == checkout_of(conversation, &self.projects[project].path))
            && self.passes_filter(conversation)
    }

    /// The conversation whose title edits in place in its sidebar row, when a rename is
    /// open for a conversation the list shows. Otherwise the rename stays in the popup.
    fn inline_rename_row(&self) -> Option<usize> {
        let menu = self.context_menu.as_ref()?;
        let MenuTarget::Session(index) = menu.target else {
            return None;
        };
        menu.renaming.as_ref()?;
        let project = self.current_project?;
        (self.projects[project].conversations.contains(&index)
            && self.in_sidebar_list(project, index))
        .then_some(index)
    }

    fn passes_filter(&self, conversation: &Conversation) -> bool {
        let filter = &self.session_filter;
        if filter.hidden_providers.contains(&conversation.provider) {
            return false;
        }
        if filter.days != 0 && days_ago(conversation) >= i64::from(filter.days) {
            return false;
        }
        filter.status.is_empty()
            || filter.status.iter().any(|status| match *status {
                "working" => conversation.working() && conversation.pending.is_empty(),
                "approval" => !conversation.pending.is_empty(),
                _ => !conversation.working() && conversation.pending.is_empty(),
            })
    }

    fn filter_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.filter_button_at.clone();
        let lit = self.session_filter.open.is_some() || self.session_filter.active();
        div()
            .id("session-filter")
            .absolute()
            .top(px(6.))
            .right(px(8.))
            .size(px(24.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(lit, |d| d.bg(content(0.09)))
            .hover(|s| s.bg(content(0.07)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| {
                this.session_filter.open = match this.session_filter.open {
                    Some(_) => None,
                    None => Some(Tween::from_to(
                        0.,
                        1.,
                        Instant::now(),
                        if cx.reduce_motion() { 0. } else { 0.16 },
                        Curve::Expo,
                    )),
                };
                cx.notify();
            }))
            .child(
                gpui::canvas(move |bounds, _, _| store.set(Some(bounds)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                icon("filter", 14.).text_color(if self.session_filter.active() {
                    accent()
                } else {
                    content(0.55)
                }),
            )
            .into_any_element()
    }

    fn filter_menu_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.session_filter.open.as_ref()?;
        let button = self.filter_button_at.get()?;
        if !open.done(now) {
            *moving = true;
        }
        let shown = open.at(now).clamp(0., 1.);
        let filter = &self.session_filter;
        let track = crate::hover_track::list("filter-menu");
        let caption = |text: &'static str| {
            div()
                .px(px(8.))
                .pt(px(8.))
                .pb(px(3.))
                .text_size(px(11.5))
                .text_color(content(0.42))
                .child(crate::i18n::t(text))
        };
        let row = |id: (&'static str, usize),
                   glyph: Option<&'static str>,
                   label: String,
                   chosen: bool| {
            track.row_named(
                div()
                    .id(id)
                    .h(px(30.))
                    .px(px(8.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .text_size(px(13.))
                    .text_color(content(0.92))
                    .cursor_pointer()
                    .children(glyph.map(|g| icon(g, 14.).text_color(content(0.6))))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .when(chosen, |d| {
                        d.child(icon("check", 13.).text_color(content(0.8)))
                    }),
                &format!("{}{}", id.0, id.1),
                false,
            )
        };
        let statuses: Vec<AnyElement> = [
            ("working", "zap", "Trabalhando"),
            ("approval", "hand", "Precisa de aprovação"),
            ("done", "check", "Concluídas"),
        ]
        .into_iter()
        .enumerate()
        .map(|(at, (key, glyph, label))| {
            row(
                ("filter-status", at),
                Some(glyph),
                crate::i18n::t(label).to_owned(),
                filter.status.contains(&key),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let list = &mut this.session_filter.status;
                match list.iter().position(|s| *s == key) {
                    Some(found) => {
                        list.remove(found);
                    }
                    None => list.push(key),
                }
                cx.notify();
            }))
            .into_any_element()
        })
        .collect();
        let times: Vec<AnyElement> = [
            (0, "calendar-range", "Todo o período"),
            (1, "calendar-today", "Hoje"),
            (7, "calendar-week", "Últimos 7 dias"),
            (30, "calendar-month", "Últimos 30 dias"),
        ]
        .into_iter()
        .enumerate()
        .map(|(at, (days, glyph, label))| {
            row(
                ("filter-time", at),
                Some(glyph),
                crate::i18n::t(label).to_owned(),
                filter.days == days,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.session_filter.days = days;
                cx.notify();
            }))
            .into_any_element()
        })
        .collect();
        let mut present: Vec<&'static str> = self.current_project.map_or_else(Vec::new, |p| {
            self.projects[p]
                .conversations
                .iter()
                .map(|c| self.conversations[*c].provider)
                .collect()
        });
        present.sort_by_key(|p| PROVIDER_ORDER.iter().position(|o| o == p));
        present.dedup();
        let providers: Vec<AnyElement> = present
            .into_iter()
            .enumerate()
            .map(|(at, provider)| {
                track
                    .row(
                        div()
                            .id(("filter-provider", at))
                            .h(px(30.))
                            .px(px(8.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .text_size(px(13.))
                            .text_color(content(0.92))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let hidden = &mut this.session_filter.hidden_providers;
                                match hidden.iter().position(|p| *p == provider) {
                                    Some(found) => {
                                        hidden.remove(found);
                                    }
                                    None => hidden.push(provider),
                                }
                                cx.notify();
                            }))
                            .child(provider_mark(provider, 15.))
                            .child(div().flex_1().child(provider_title(provider)))
                            .when(!filter.hidden_providers.contains(&provider), |d| {
                                d.child(icon("check", 13.).text_color(content(0.8)))
                            }),
                        100 + at,
                        false,
                    )
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .id("filter-menu")
                .occlude()
                .absolute()
                .left(px(f32::from(button.left())))
                .top(px(f32::from(button.bottom()) + 4. + (1. - shown) * -6.))
                .w(px(230.))
                .p(px(4.))
                .flex()
                .flex_col()
                .opacity(shown)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(track.pill())
                .child(caption("Status"))
                .children(statuses)
                .child(caption("Período"))
                .children(times)
                .child(caption("Provedor"))
                .children(providers)
                .into_any_element(),
        )
    }

    fn open_context_menu(
        &mut self,
        target: MenuTarget,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some(ContextMenu {
            target,
            x: f32::from(event.position.x),
            y: f32::from(event.position.y),
            open: Tween::from_to(
                0.,
                1.,
                Instant::now(),
                if cx.reduce_motion() { 0. } else { 0.12 },
                Curve::Expo,
            ),
            renaming: None,
        });
        cx.notify();
    }

    fn rename_conversation(&mut self, index: usize, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        self.conversations[index].title = title.to_owned();
        self.backend.invoke(
            "rename_session",
            json!({ "sessionId": self.conversations[index].session_id, "title": title }),
        );
    }

    /// Takes a conversation out of the sidebar and its tab; another takes its place on screen.
    fn leave_conversation(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = self
            .tabs
            .iter()
            .find(|t| t.conversation == index && !t.closing)
            .map(|t| t.id);
        let project = self.project_of(index);
        if let Some(tab) = tab {
            let live = self.tabs.iter().filter(|t| !t.closing).count();
            if live > 1 {
                self.close(tab, cx);
            } else {
                let next = project.and_then(|p| {
                    self.projects[p]
                        .conversations
                        .iter()
                        .copied()
                        .find(|c| *c != index && self.passes_filter(&self.conversations[*c]))
                });
                match next {
                    Some(next) => {
                        self.open(next, cx);
                        self.close(tab, cx);
                    }
                    None => self.new_conversation(project, cx),
                }
            }
        }
    }

    fn delete_conversation(&mut self, index: usize, cx: &mut Context<Self>) {
        let id = self.conversations[index].session_id.clone();
        self.leave_conversation(index, cx);
        if let Some(project) = self.project_of(index) {
            self.projects[project].conversations.retain(|c| *c != index);
        }
        self.backend.invoke(
            "delete_session",
            json!({ "sessionId": id, "removeWorktree": false, "confirm": true }),
        );
    }

    fn ask(
        &mut self,
        title: String,
        message: String,
        action_label: &'static str,
        action: impl FnOnce(&mut Shell, &mut Context<Shell>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.confirm = Some(ConfirmDialog {
            open: Tween::from_to(
                0.,
                1.,
                Instant::now(),
                if cx.reduce_motion() { 0. } else { 0.18 },
                Curve::Expo,
            ),
            title,
            message,
            action_label: action_label.to_owned(),
            action: Box::new(action),
        });
        cx.notify();
    }

    /// Settings → Comportamento do chat → "Confirmar ao fechar um terminal" (on unless turned off).
    pub fn confirms_terminal_close(&self) -> bool {
        self.settings["confirmTerminalClose"] != json!(false)
    }

    /// Confirmation in the shell's popup for another view (the browser panel's deletions); `action`
    /// runs only on yes.
    pub fn confirm(
        &mut self,
        title: String,
        message: String,
        action_label: &'static str,
        action: impl FnOnce(&mut gpui::App) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.ask(title, message, action_label, move |_, cx| action(cx), cx);
    }

    /// The browser's custom profiles and the profile new tabs open in, kept with the settings.
    pub fn save_browser_profiles(
        &mut self,
        profiles: Value,
        default: &str,
        cx: &mut Context<Self>,
    ) {
        self.settings["browserProfiles"] = profiles;
        self.settings["defaultBrowserProfile"] = json!(default);
        crate::panel_browser::note_settings(&self.settings);
        self.backend
            .invoke("save_settings", json!({ "settings": self.settings }));
        cx.notify();
    }

    fn ask_delete_conversation(&mut self, index: usize, cx: &mut Context<Self>) {
        let title = self.conversations[index].title.clone();
        self.ask(
            crate::i18n::t("Excluir conversa?").to_owned(),
            tf("\"{}\" será apagada para sempre.", &[&title]),
            crate::i18n::t("Excluir"),
            move |this, cx| this.delete_conversation(index, cx),
            cx,
        );
    }

    fn ask_remove_project(&mut self, index: usize, cx: &mut Context<Self>) {
        let name = self.projects[index].name.clone();
        self.ask(
            crate::i18n::t("Remover projeto?").to_owned(),
            tf(
                "\"{}\" sai da lista. A pasta e os arquivos continuam no disco.",
                &[&name],
            ),
            crate::i18n::t("Remover"),
            move |this, cx| this.drop_project(index, cx),
            cx,
        );
    }

    fn answer_confirm(&mut self, yes: bool, cx: &mut Context<Self>) {
        if let Some(dialog) = self.confirm.take() {
            if yes {
                (dialog.action)(self, cx);
            }
        }
        cx.notify();
    }

    fn confirm_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.confirm.as_ref()?;
        if !dialog.open.done(now) {
            *moving = true;
        }
        let shown = dialog.open.at(now).clamp(0., 1.);
        let danger: gpui::Hsla = rgb(0xec8a85).into();
        Some(
            div()
                .id("confirm-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::Hsla {
                    a: 0.45 * shown,
                    ..gpui::black()
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.answer_confirm(false, cx);
                    }),
                )
                .child(
                    div()
                        .id("confirm-card")
                        .relative()
                        .top(px((1. - shown) * 6.))
                        .w(px(300.))
                        .p(px(18.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .opacity(shown)
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(content(0.12))
                        .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .size(px(38.))
                                .mb(px(10.))
                                .rounded_full()
                                .bg(gpui::Hsla { a: 0.12, ..danger })
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon("trash", 18.).text_color(danger)),
                        )
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(content(0.97))
                                .child(dialog.title.clone()),
                        )
                        .child(
                            div()
                                .mt(px(6.))
                                .text_center()
                                .text_size(px(12.5))
                                .line_height(px(18.))
                                .text_color(content(0.6))
                                .child(dialog.message.clone()),
                        )
                        .child(
                            div()
                                .mt(px(16.))
                                .w_full()
                                .flex()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .id("confirm-cancel")
                                        .flex_1()
                                        .h(px(30.))
                                        .rounded(px(crate::theme::RADIUS_MD))
                                        .bg(content(0.06))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(content(0.8))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(content(0.07)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.answer_confirm(false, cx)
                                        }))
                                        .child(crate::i18n::t("Cancelar")),
                                )
                                .child(
                                    div()
                                        .id("confirm-yes")
                                        .flex_1()
                                        .h(px(30.))
                                        .rounded(px(crate::theme::RADIUS_MD))
                                        .bg(rgb(0xc9504a))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(gpui::white())
                                        .cursor_pointer()
                                        .hover(|s| s.bg(rgb(0xd5605a)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.answer_confirm(true, cx)
                                        }))
                                        .child(dialog.action_label.clone()),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn context_menu_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.context_menu.as_mut() else {
            return false;
        };
        let target = menu.target;
        match key {
            "escape" => {
                self.context_menu = None;
                true
            }
            "enter" if menu.renaming.is_some() => {
                let title = menu.renaming.clone().unwrap_or_default();
                self.context_menu = None;
                if let MenuTarget::Session(index) = target {
                    self.rename_conversation(index, &title);
                } else if let MenuTarget::Project(index) = target {
                    if !title.trim().is_empty() {
                        self.projects[index].name = title.trim().to_owned();
                        self.backend.invoke(
                            "rename_project",
                            json!({ "projectId": self.projects[index].id, "name": title.trim() }),
                        );
                    }
                }
                true
            }
            "backspace" if menu.renaming.is_some() => {
                menu.renaming.as_mut().unwrap().pop();
                true
            }
            "backspace" => {
                self.context_menu = None;
                match target {
                    MenuTarget::Session(index) if !self.conversations[index].working() => {
                        self.ask_delete_conversation(index, cx)
                    }
                    MenuTarget::Project(index) => self.ask_remove_project(index, cx),
                    MenuTarget::Session(_) => {}
                }
                true
            }
            "f2" if menu.renaming.is_none() => {
                let name = match target {
                    MenuTarget::Session(index) => self.conversations[index].title.clone(),
                    MenuTarget::Project(index) => self.projects[index].name.clone(),
                };
                self.context_menu.as_mut().unwrap().renaming = Some(name);
                true
            }
            _ => false,
        }
    }

    fn context_menu_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.context_menu.as_ref()?;
        // A rename of a listed conversation edits in its row instead (conversation_row).
        if self.inline_rename_row().is_some() {
            return None;
        }
        if !menu.open.done(now) {
            *moving = true;
        }
        let shown = menu.open.at(now).clamp(0., 1.);
        let track = crate::hover_track::list("context-menu");
        let item = |id: &'static str,
                    glyph: &'static str,
                    label: &'static str,
                    hint: Option<&'static str>| {
            track.row_named(
                div()
                    .id(id)
                    .h(px(30.))
                    .px(px(8.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .text_size(px(13.))
                    .text_color(content(0.92))
                    .cursor_pointer()
                    .child(icon(glyph, 14.).text_color(content(0.65)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(crate::i18n::t(label)),
                    )
                    .children(
                        hint.map(|h| div().text_size(px(11.)).text_color(content(0.4)).child(h)),
                    ),
                id,
                false,
            )
        };
        let divider = || div().mx(px(6.)).my(px(4.)).h(px(1.)).bg(content(0.08));
        let danger: gpui::Hsla = rgb(0xe08a8a).into();
        let close = |this: &mut Self| this.context_menu = None;
        let body: AnyElement = if let Some(name) = &menu.renaming {
            div()
                .h(px(32.))
                .px(px(8.))
                .rounded(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_color(content(0.3))
                .bg(field_fill())
                .flex()
                .items_center()
                .text_size(px(13.))
                .child(typed_text(
                    name,
                    crate::i18n::t("Nome"),
                    div()
                        .flex_shrink_0()
                        .w(px(1.5))
                        .h(px(15.))
                        .rounded_full()
                        .bg(crate::theme::caret_color()),
                ))
                .into_any_element()
        } else {
            match menu.target {
                MenuTarget::Session(index) => {
                    let conversation = &self.conversations[index];
                    let tab = self
                        .tabs
                        .iter()
                        .find(|t| t.conversation == index && !t.closing);
                    let pinned = tab.is_some_and(|t| t.pinned);
                    let (id_text, working) =
                        (conversation.session_id.clone(), conversation.working());
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            item(
                                "menu-pin",
                                "pin",
                                if pinned { "Desafixar" } else { "Fixar" },
                                None,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    close(this);
                                    this.open(index, cx);
                                    let tab = this
                                        .tabs
                                        .iter()
                                        .find(|t| t.conversation == index && !t.closing)
                                        .map(|t| t.id);
                                    if let Some(tab) = tab {
                                        this.toggle_pin(tab, cx);
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            item("menu-split", "layout-grid", "Abrir ao lado", None).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    close(this);
                                    this.open_beside(index, split_panes::Edge::Right, cx);
                                }),
                            ),
                        )
                        .child(
                            item("menu-rename", "pencil", "Renomear", Some("F2")).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    let name = this.conversations[index].title.clone();
                                    if let Some(menu) = this.context_menu.as_mut() {
                                        menu.renaming = Some(name);
                                    }
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            item("menu-copy-id", "copy", "Copiar ID da conversa", None).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        id_text.clone(),
                                    ));
                                    close(this);
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            item("menu-export", "upload", "Exportar conversa", None).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    close(this);
                                    this.export_conversation(index, cx);
                                }),
                            ),
                        )
                        .child({
                            // Forks the newest reply; disabled while the conversation works.
                            let fork = item("menu-fork", "branch", "Bifurcar", None);
                            if self.can_fork_latest(index) {
                                fork.on_click(cx.listener(move |this, _, _, cx| {
                                    close(this);
                                    this.fork_latest(index, cx);
                                }))
                                .into_any_element()
                            } else {
                                fork.opacity(0.4).into_any_element()
                            }
                        })
                        .children(self.move_menu_items(index, cx))
                        .child(divider())
                        .child(
                            div()
                                .id("menu-delete")
                                .h(px(30.))
                                .px(px(8.))
                                .rounded(px(crate::theme::RADIUS_MD))
                                .flex()
                                .items_center()
                                .gap(px(9.))
                                .text_size(px(13.))
                                .text_color(danger)
                                .when(working, |d| d.opacity(0.4))
                                .when(!working, |d| {
                                    d.cursor_pointer()
                                        .hover(move |s| s.bg(gpui::Hsla { a: 0.12, ..danger }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            close(this);
                                            this.ask_delete_conversation(index, cx);
                                        }))
                                })
                                .child(icon("trash", 14.).text_color(danger))
                                .child(div().flex_1().child(crate::i18n::t("Excluir")))
                                .child(
                                    div().text_size(px(11.)).text_color(content(0.4)).child("⌫"),
                                ),
                        )
                        .into_any_element()
                }
                MenuTarget::Project(index) => {
                    let path = self.projects[index].path.clone();
                    let (reveal, copied) = (path.clone(), path);
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            item("menu-new", "square-pen", "Nova conversa", None).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    close(this);
                                    this.close_project_menu();
                                    this.enter_project(Some(index), cx);
                                    this.new_conversation(Some(index), cx);
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            item("menu-project-rename", "pencil", "Renomear", Some("F2")).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    let name = this.projects[index].name.clone();
                                    if let Some(menu) = this.context_menu.as_mut() {
                                        menu.renaming = Some(name);
                                    }
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            item(
                                "menu-project-settings",
                                "tuning",
                                "Ajustes do projeto",
                                None,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    close(this);
                                    this.switch_project(index, cx);
                                    this.toggle_project_menu(cx);
                                    this.open_project_sub(ProjectSub::Settings, cx);
                                },
                            )),
                        )
                        .child(divider())
                        .child(
                            item("menu-reveal", "folder-open", "Mostrar no Finder", None).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    let _ = crate::native_actions::open_path(&reveal);
                                    close(this);
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(
                            item("menu-copy-path", "copy", "Copiar caminho", None).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        copied.clone(),
                                    ));
                                    close(this);
                                    cx.notify();
                                }),
                            ),
                        )
                        .child(divider())
                        .child(
                            div()
                                .id("menu-project-remove")
                                .h(px(30.))
                                .px(px(8.))
                                .rounded(px(crate::theme::RADIUS_MD))
                                .flex()
                                .items_center()
                                .gap(px(9.))
                                .text_size(px(13.))
                                .text_color(danger)
                                .cursor_pointer()
                                .hover(move |s| s.bg(gpui::Hsla { a: 0.12, ..danger }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    close(this);
                                    this.ask_remove_project(index, cx);
                                }))
                                .child(icon("trash", 14.).text_color(danger))
                                .child(crate::i18n::t("Remover da lista")),
                        )
                        .into_any_element()
                }
            }
        };
        const WIDTH: f32 = 230.;
        Some(
            div()
                .id("context-menu")
                .occlude()
                .absolute()
                .left(px(menu.x.min(self.viewport_width - WIDTH - 8.).max(8.)))
                .top(px(
                    menu.y.min(self.viewport_height - 300.).max(8.) + (1. - shown) * -4.
                ))
                .w(px(WIDTH))
                .p(px(4.))
                .flex()
                .flex_col()
                .opacity(shown)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(crate::hover_track::list("context-menu").pill())
                .child(body)
                .into_any_element(),
        )
    }

    /// One virtual list per conversation: only what's on screen is built.
    fn list_state(&mut self, conversation: usize, cx: &mut Context<Self>) -> gpui::ListState {
        if let Some(state) = self.lists.get(&conversation) {
            return state.clone();
        }
        let state = gpui::ListState::new(
            1 + self.conversations[conversation].turns.len(),
            gpui::ListAlignment::Top,
            px(320.),
        );
        state.set_follow_mode(gpui::FollowMode::Tail);
        let shell = cx.entity().downgrade();
        state.set_scroll_handler(move |_, _, cx| {
            let _ = shell.update(cx, |_, cx| cx.notify());
        });
        self.lists.insert(conversation, state.clone());
        state
    }

    /// The rail beside the transcript: one tick per message, as Sirus Code's `MessageTrail`.
    fn message_trail(
        &self,
        conversation: usize,
        state: &gpui::ListState,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let count = self.conversations[conversation].turns.len();
        if count < 2 {
            return None;
        }
        let current = state
            .logical_scroll_top()
            .item_ix
            .saturating_sub(1)
            .min(count - 1);
        let mut last_visible = current;
        while last_visible + 1 < count
            && state.item_is_below_viewport(last_visible + 2) == Some(false)
        {
            last_visible += 1;
        }
        let hovered = self.trail_hover.map(|h| h.0);
        let ticks: Vec<AnyElement> = (0..count)
            .map(|at| {
                let on_current = at == current;
                let emphasis =
                    hovered.map_or(0., |h| (-((at as f32 - h as f32).powi(2)) / 4.5).exp());
                let base = if on_current { 12. } else { 6. };
                let width = base + emphasis * (30. - base);
                let alpha = if hovered == Some(at) {
                    1.
                } else if on_current {
                    0.9
                } else if at >= current && at <= last_visible {
                    0.5
                } else {
                    0.25
                };
                let target = state.clone();
                div()
                    .id(("trail-tick", at))
                    .h(px(12.))
                    .w_full()
                    .pl(px(10.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_start()
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovering: &bool, window, cx| {
                        if *hovering {
                            this.trail_x = f32::from(window.mouse_position().x);
                            this.trail_hover = Some((at, f32::from(window.mouse_position().y)));
                        } else if this.trail_hover.is_some_and(|h| h.0 == at) {
                            this.trail_hover = None;
                        }
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.trail_hover = None;
                        target.pause_following_tail();
                        this.glides.entry(conversation).or_default().leave();
                        target.scroll_to(gpui::ListOffset {
                            item_ix: at + 1,
                            offset_in_item: px(0.),
                        });
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(width))
                            .h(px(2.))
                            .rounded_full()
                            .bg(content(alpha)),
                    )
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(44.))
                .flex()
                .flex_col()
                .justify_center()
                .child(
                    div()
                        .id("message-trail")
                        .max_h(gpui::relative(0.8))
                        .overflow_y_scroll()
                        .py(px(8.))
                        .flex()
                        .flex_col()
                        .children(ticks),
                )
                .into_any_element(),
        )
    }

    /// The preview beside a hovered tick: the message, then the start of its reply.
    fn trail_card(&self) -> Option<AnyElement> {
        let (at, y) = self.trail_hover?;
        let turn = self.conversations[self.active_conversation()]
            .turns
            .get(at)?;
        let preview = |text: &str, limit: usize| -> String {
            let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if line.chars().count() > limit {
                format!(
                    "{}…",
                    line.chars().take(limit).collect::<String>().trim_end()
                )
            } else {
                line
            }
        };
        let mut paragraphs = turn
            .answer
            .split("\n\n")
            .map(str::trim)
            .filter(|p| !p.is_empty());
        let (first, second) = (
            paragraphs.next().map(|p| preview(p, 90)),
            paragraphs.next().map(|p| preview(p, 110)),
        );
        Some(
            div()
                .absolute()
                .left(px(self.trail_x + 44.))
                .top(px((y - 34.).clamp(TITLE_BAR, self.viewport_height - 170.)))
                .w(px(290.))
                .px(px(14.))
                .py(px(11.))
                .flex()
                .flex_col()
                .gap(px(5.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(0.97))
                        .line_clamp(2)
                        .child(preview(&turn.user, 120)),
                )
                .children(first.map(|text| {
                    div()
                        .text_size(px(12.5))
                        .text_color(content(0.55))
                        .line_clamp(2)
                        .child(text)
                }))
                .children(second.map(|text| {
                    div()
                        .mt(px(1.))
                        .pl(px(9.))
                        .border_l_2()
                        .border_color(content(0.14))
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .line_clamp(2)
                        .child(text)
                }))
                .into_any_element(),
        )
    }

    fn project_avatar_for(&self, index: usize, side: f32) -> AnyElement {
        let project = &self.projects[index];
        let among = || {
            project
                .conversations
                .iter()
                .map(|c| &self.conversations[*c])
        };
        let state = if among().any(|c| !c.pending.is_empty()) {
            crate::project_avatar::AvatarState::Waiting
        } else if among().any(Conversation::working) {
            crate::project_avatar::AvatarState::Working
        } else {
            crate::project_avatar::AvatarState::Idle
        };
        crate::project_avatar::project_avatar_state(
            &project.name,
            &project.appearance,
            side,
            state,
            Instant::now().duration_since(self.epoch).as_secs_f32(),
        )
    }

    /// One project at a time, as Monocode's: its name opens the popup that changes it.
    fn project_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.current_project.or(self.project_order.first().copied());
        let name = current.map_or(String::new(), |ix| self.projects[ix].name.clone());
        let store = self.project_button.clone();
        let open = self.project_menu.is_some();
        div()
            .flex_shrink_0()
            .h(px(38.))
            .pl(px(6.))
            .pr(px(8.))
            .pt(px(6.))
            .flex()
            .items_center()
            .gap(px(1.))
            .child(
                div()
                    .id("project-switcher")
                    .relative()
                    .min_w_0()
                    .h(px(30.))
                    .px(px(3.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .cursor_pointer()
                    .when(open, |d| d.bg(content(0.09)))
                    .hover(|s| s.bg(content(0.07)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_project_menu(cx)))
                    .child(
                        gpui::canvas(move |bounds, _, _| store.set(Some(bounds)), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .children(current.map(|ix| self.project_avatar_for(ix, 20.)))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(content(0.95))
                            .child(name),
                    )
                    .child(chevron_down().flex_shrink_0())
                    .children(self.other_projects_badge().map(|b| self.badge_view(b))),
            )
            .child(div().flex_1())
            .children(self.sidebar_search_button(cx))
            .child(self.sidebar_options_button(cx))
            .into_any_element()
    }

    fn toggle_project_menu(&mut self, cx: &mut Context<Self>) {
        if !self.close_project_menu() {
            let now = Instant::now();
            self.composer_menu = None;
            self.usage_popup = None;
            self.sidebar_options = None;
            self.project_menu = Some(ProjectMenu {
                open: Tween::from_to(
                    0.,
                    1.,
                    now,
                    if cx.reduce_motion() { 0. } else { 0.16 },
                    Curve::Expo,
                ),
                sub: None,
                sub_at: now,
                query: String::new(),
                selected: 0,
                expanded: None,
            });
        }
        cx.notify();
    }

    /// Closes the popup, saving a name being edited. Says whether it was open.
    fn close_project_menu(&mut self) -> bool {
        self.save_project_scripts(true);
        let Some(menu) = self.project_menu.take() else {
            return false;
        };
        if menu.sub == Some(ProjectSub::Settings) {
            self.commit_project_name(&menu.query);
        }
        true
    }

    fn commit_project_name(&mut self, name: &str) {
        let name = name.trim();
        let Some(ix) = self.current_project else {
            return;
        };
        if name.is_empty() || self.projects[ix].name == name {
            return;
        }
        self.projects[ix].name = name.to_owned();
        self.backend.invoke(
            "rename_project",
            json!({ "projectId": self.projects[ix].id, "name": name }),
        );
    }

    fn open_project_sub(&mut self, sub: ProjectSub, cx: &mut Context<Self>) {
        if self
            .project_menu
            .as_ref()
            .is_some_and(|m| m.sub != Some(sub))
        {
            self.save_project_scripts(true);
        }
        let Some(ix) = self.current_project else {
            return;
        };
        let (name, id) = (self.projects[ix].name.clone(), self.projects[ix].id.clone());
        let Some(menu) = self.project_menu.as_mut() else {
            return;
        };
        if menu.sub == Some(sub) {
            return;
        }
        let edited = (menu.sub == Some(ProjectSub::Settings)).then(|| menu.query.clone());
        menu.sub = Some(sub);
        menu.sub_at = Instant::now();
        menu.selected = 0;
        menu.expanded = None;
        menu.query = if sub == ProjectSub::Settings {
            name
        } else {
            String::new()
        };
        if let Some(edited) = edited {
            self.commit_project_name(&edited);
        }
        if sub == ProjectSub::Checkouts {
            let call = self
                .backend
                .invoke("list_worktrees", json!({ "projectId": id }));
            self.waiting.insert(call, Waiting::Worktrees(id));
        }
        cx.notify();
    }

    fn switch_project(&mut self, index: usize, cx: &mut Context<Self>) {
        self.close_project_menu();
        self.enter_project(Some(index), cx);
        if self.tabs.is_empty() {
            match self.projects[index].conversations.first().copied() {
                Some(first) => self.open(first, cx),
                None => self.new_conversation(Some(index), cx),
            }
        }
        cx.notify();
    }

    fn add_new_project(&mut self, cx: &mut Context<Self>) {
        self.close_project_menu();
        if self.this_mac_only(cx) {
            return;
        }
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let Some(path) = paths.first().map(|p| p.display().to_string()) else {
                    return;
                };
                let _ = this.update(cx, |this, _| {
                    let call = this.backend.invoke("add_project", json!({ "path": path }));
                    this.waiting.insert(call, Waiting::ProjectAdded);
                });
            }
        })
        .detach();
    }

    /// Removes a project from the list and goes to the next one, if it was the current one.
    fn drop_project(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.close_project_menu();
        let id = self.projects[ix].id.clone();
        self.backend
            .invoke("remove_project", json!({ "projectId": id }));
        self.project_order.retain(|p| *p != ix);
        if self.current_project == Some(ix) {
            if let Some(next) = self.project_order.first().copied() {
                self.switch_project(next, cx);
            }
        }
        cx.notify();
    }

    fn toggle_project_row(&mut self, which: &'static str, cx: &mut Context<Self>) {
        if self.project_menu.as_ref().and_then(|m| m.expanded) == Some("scripts") {
            self.save_project_scripts(true);
        }
        if let Some(menu) = self.project_menu.as_mut() {
            menu.expanded = if menu.expanded == Some(which) {
                None
            } else {
                Some(which)
            };
        }
        cx.notify();
    }

    fn set_project_look(&mut self, field: &str, value: Value, cx: &mut Context<Self>) {
        let Some(ix) = self.current_project else {
            return;
        };
        let mut look = crate::project_avatar::saved_look(
            &self.projects[ix].name,
            &self.projects[ix].appearance,
        );
        look[field] = value;
        self.projects[ix].appearance = look.clone();
        self.backend.invoke(
            "set_project_appearance",
            json!({ "projectId": self.projects[ix].id, "appearance": look }),
        );
        cx.notify();
    }

    /// The Projects rows in order, for keys and the list alike.
    fn project_hits(&self, query: &str) -> Vec<usize> {
        let needle = query.trim().to_lowercase();
        self.project_order
            .iter()
            .copied()
            .filter(|p| {
                needle.is_empty() || self.projects[*p].name.to_lowercase().contains(&needle)
            })
            .collect()
    }

    /// Checkouts as (path, name, about, conversations), project folder first.
    fn checkout_rows(&self, project: usize, query: &str) -> Vec<(String, String, String, usize)> {
        let path = self.projects[project].path.clone();
        let count = |checkout: &str| {
            self.projects[project]
                .conversations
                .iter()
                .filter(|c| checkout_of(&self.conversations[**c], &path) == checkout)
                .count()
        };
        let mut rows = Vec::new();
        for tree in self
            .worktrees
            .get(&self.projects[project].id)
            .into_iter()
            .flatten()
        {
            let tree_path = tree["path"].as_str().unwrap_or("").to_owned();
            let folder = tree_path.rsplit('/').next().unwrap_or("").to_owned();
            let name = tree["branch"].as_str().map_or(folder, |b| {
                b.strip_prefix("refs/heads/").unwrap_or(b).to_owned()
            });
            let about = if tree_path == path {
                t("Pasta do projeto")
            } else {
                "Worktree"
            }
            .to_owned();
            let conversations = count(&tree_path);
            rows.push((tree_path, name, about, conversations));
        }
        rows.sort_by_key(|row| row.0 != path);
        let needle = query.trim().to_lowercase();
        rows.retain(|row| needle.is_empty() || row.1.to_lowercase().contains(&needle));
        rows
    }

    fn project_menu_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.project_menu.as_ref() else {
            return false;
        };
        let (sub, query, selected) = (menu.sub, menu.query.clone(), menu.selected);
        let Some(project) = self.current_project else {
            return false;
        };
        // A focused script field takes Enter, Backspace, Tab and Escape before the menu does.
        if let Some(edit) = self.scripts_edit.as_mut() {
            let taken = match key {
                "enter" => edit.enter(),
                "backspace" => edit.backspace(),
                "tab" => edit.tab(),
                "escape" => edit.escape(),
                _ => false,
            };
            if taken {
                cx.notify();
                return true;
            }
        }
        match key {
            "tab" => {
                let next = match sub {
                    None | Some(ProjectSub::Settings) => ProjectSub::Checkouts,
                    Some(ProjectSub::Checkouts) => ProjectSub::Projects,
                    Some(ProjectSub::Projects) => ProjectSub::Settings,
                };
                self.open_project_sub(next, cx);
                true
            }
            "escape" => {
                if sub.is_some() {
                    self.save_project_scripts(true);
                    if sub == Some(ProjectSub::Settings) {
                        self.commit_project_name(&query);
                    }
                    let menu = self.project_menu.as_mut().unwrap();
                    menu.sub = None;
                    menu.query.clear();
                } else {
                    self.project_menu = None;
                }
                true
            }
            "backspace" if sub.is_some() => {
                let menu = self.project_menu.as_mut().unwrap();
                menu.query.pop();
                menu.selected = 0;
                true
            }
            "down" | "up" if matches!(sub, Some(ProjectSub::Projects | ProjectSub::Checkouts)) => {
                let count = if sub == Some(ProjectSub::Projects) {
                    self.project_hits(&query).len()
                } else {
                    self.checkout_rows(project, &query).len() + 1
                };
                if count > 0 {
                    self.project_menu.as_mut().unwrap().selected = if key == "down" {
                        (selected + 1) % count
                    } else {
                        (selected + count - 1) % count
                    };
                }
                true
            }
            "enter" => {
                match sub {
                    Some(ProjectSub::Projects) => {
                        if let Some(next) = self.project_hits(&query).get(selected).copied() {
                            self.switch_project(next, cx);
                        }
                    }
                    Some(ProjectSub::Checkouts) => {
                        if selected == 0 {
                            self.checkout_filter.remove(&project);
                        } else if let Some(row) =
                            self.checkout_rows(project, &query).get(selected - 1)
                        {
                            self.checkout_filter.insert(project, row.0.clone());
                        }
                        self.close_project_menu();
                    }
                    Some(ProjectSub::Settings) => {
                        self.commit_project_name(&query);
                    }
                    None => {}
                }
                true
            }
            _ if sub.is_some() => {
                self.project_menu.as_mut().unwrap().selected = 0;
                cx.notify();
                // The text itself arrives through the input handler.
                false
            }
            _ => false,
        }
    }

    fn project_menu_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.project_menu.as_ref()?;
        let index = self.current_project?;
        let button = self.project_button.get()?;
        if !menu.open.done(now) {
            *moving = true;
        }
        let t = menu.open.at(now).clamp(0., 1.);
        let left = f32::from(button.left());
        let top = f32::from(button.bottom()) + 6.;
        let sub = menu.sub;
        let project = &self.projects[index];
        let filter = self.checkout_filter.get(&index).cloned();
        let current_checkout = filter.as_ref().map_or_else(
            || crate::i18n::t("Todos os checkouts").to_owned(),
            |path| {
                self.worktrees
                    .get(&project.id)
                    .into_iter()
                    .flatten()
                    .find(|w| w["path"].as_str() == Some(path))
                    .and_then(|w| w["branch"].as_str())
                    .map_or_else(
                        || path.rsplit('/').next().unwrap_or("").to_owned(),
                        |b| b.strip_prefix("refs/heads/").unwrap_or(b).to_owned(),
                    )
            },
        );
        let track = crate::hover_track::list("project-menu");
        let row = |id: &'static str,
                   glyph: &'static str,
                   label: &'static str,
                   value: Option<String>,
                   target: ProjectSub,
                   cx: &mut Context<Self>| {
            let active = sub == Some(target);
            track.row_named(
                div()
                    .id(id)
                    .h(px(32.))
                    .px(px(8.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_size(px(13.))
                    .text_color(content(0.92))
                    .cursor_pointer()
                    .when(active, |d| d.bg(content(0.09)))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.open_project_sub(target, cx);
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_project_sub(target, cx)))
                    .child(icon(glyph, 15.).text_color(content(0.65)))
                    .child(div().flex_shrink_0().child(label))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(content(0.42))
                            .children(value),
                    )
                    .child(icon("chevron-right", 12.).text_color(content(0.45))),
                id,
                active,
            )
        };
        let main = div()
            .id("project-menu")
            .occlude()
            .absolute()
            .left(px(left))
            .top(px(top + (1. - t) * -6.))
            .w(px(240.))
            .p(px(4.))
            .flex()
            .flex_col()
            .opacity(t)
            .rounded(px(crate::theme::RADIUS_LG))
            .border_1()
            .border_color(content(0.12))
            .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(crate::hover_track::list("project-menu").pill())
            .child(row(
                "project-checkouts",
                "branch",
                crate::i18n::t("Trocar…"),
                Some(current_checkout),
                ProjectSub::Checkouts,
                cx,
            ))
            .child(div().mx(px(8.)).my(px(4.)).h(px(1.)).bg(content(0.08)))
            .child(row(
                "project-switch",
                "folder",
                crate::i18n::t("Trocar de projeto"),
                None,
                ProjectSub::Projects,
                cx,
            ))
            .child(row(
                "project-settings",
                "tuning",
                crate::i18n::t("Ajustes do projeto"),
                None,
                ProjectSub::Settings,
                cx,
            ));
        let sub_view =
            sub.map(|sub| self.project_sub_view(sub, index, left + 246., top, now, moving, cx));
        Some(
            div()
                .absolute()
                .inset_0()
                .child(main)
                .children(sub_view)
                .into_any_element(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn project_sub_view(
        &self,
        sub: ProjectSub,
        index: usize,
        left: f32,
        menu_top: f32,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let menu = self.project_menu.as_ref().expect("open");
        let since = now.saturating_duration_since(menu.sub_at).as_secs_f32();
        let t = eased(since / 0.14, Curve::Expo);
        if since < 0.14 {
            *moving = true;
        }
        let caret = move || {
            div()
                .flex_shrink_0()
                .w(px(1.5))
                .h(px(16.))
                .rounded_full()
                .bg(crate::theme::caret_color())
        };
        let query = menu.query.clone();
        let selected = menu.selected;
        let search = |placeholder: &'static str| {
            div()
                .h(px(42.))
                .flex_shrink_0()
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(9.))
                .border_b_1()
                .border_color(content(0.08))
                .text_size(px(13.))
                .child(icon("search", 15.).text_color(content(0.5)))
                .child(typed_text(&query, placeholder, caret()))
        };
        let project_path = self.projects[index].path.clone();
        let (width, top_offset, height, body): (f32, f32, f32, AnyElement) = match sub {
            ProjectSub::Checkouts => {
                let rows = self.checkout_rows(index, &query);
                let filter = self.checkout_filter.get(&index).cloned();
                let total = self.projects[index].conversations.len();
                let line = |id: usize,
                            glyph: &'static str,
                            name: String,
                            about: String,
                            count: usize,
                            chosen: bool,
                            focus: bool,
                            target: Option<String>,
                            cx: &mut Context<Self>| {
                    crate::hover_track::list("checkout-list").row(
                        div()
                            .id(("checkout-row", id))
                            .min_h(px(44.))
                            .px(px(8.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .cursor_pointer()
                            .when(focus, |d| d.bg(content(0.09)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                match target.clone() {
                                    Some(path) => {
                                        this.checkout_filter.insert(index, path);
                                    }
                                    None => {
                                        this.checkout_filter.remove(&index);
                                    }
                                }
                                this.close_project_menu();
                                cx.notify();
                            }))
                            .child(icon(glyph, 15.).text_color(content(0.6)))
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
                                            .text_color(content(0.95))
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(11.5))
                                            .text_color(content(0.45))
                                            .child(about),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(content(0.45))
                                    .child(count.to_string()),
                            )
                            .when(chosen, |d| {
                                d.child(icon("check", 13.).text_color(content(0.8)))
                            }),
                        id,
                        focus,
                    )
                };
                let mut list: Vec<AnyElement> = vec![
                    line(
                        0,
                        "layout-grid",
                        crate::i18n::t("Todos os checkouts").to_owned(),
                        crate::i18n::t("Todas as conversas").to_owned(),
                        total,
                        filter.is_none(),
                        selected == 0,
                        None,
                        cx,
                    )
                    .into_any_element(),
                ];
                for (at, (path, name, about, count)) in rows.iter().cloned().enumerate() {
                    let chosen = filter.as_deref() == Some(path.as_str());
                    list.push(
                        line(
                            at + 1,
                            "branch",
                            name,
                            about,
                            count,
                            chosen,
                            selected == at + 1,
                            Some(path),
                            cx,
                        )
                        .into_any_element(),
                    );
                }
                let title = query.trim().to_owned();
                let project_for_new = index;
                let create = div().id("checkout-create").h(px(34.)).px(px(8.)).mt(px(2.)).rounded(px(crate::theme::RADIUS_MD)).flex().items_center().gap(px(9.)).text_size(px(13.)).text_color(content(0.72)).cursor_pointer().hover(|s| s.bg(content(0.07)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let active = this.active_conversation();
                        let agent = this.conversations[active].provider;
                        let mut request = json!({ "projectId": this.projects[project_for_new].id, "agent": agent, "isolatedWorktree": true });
                        if !title.is_empty() {
                            request["title"] = json!(title);
                        }
                        let call = this.backend.invoke("create_session", json!({ "request": request }));
                        this.waiting.insert(call, Waiting::Created);
                        this.close_project_menu();
                        cx.notify();
                    }))
                    .child(icon("plus", 14.).text_color(content(0.6)))
                    .child(crate::i18n::t("Nova conversa em worktree"));
                let count = list.len() as f32;
                (
                    280.,
                    0.,
                    44. + 8. + 44. * count + 36.,
                    div()
                        .flex()
                        .flex_col()
                        .child(search(crate::i18n::t("Buscar ou criar worktree…")))
                        .child(
                            div()
                                .id("checkout-list")
                                .p(px(4.))
                                .max_h(px(330.))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .child(crate::hover_track::list("checkout-list").pill())
                                .children(list)
                                .child(create),
                        )
                        .into_any_element(),
                )
            }
            ProjectSub::Projects => {
                let hits = self.project_hits(&query);
                let current = self.current_project;
                let list: Vec<AnyElement> = hits
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(at, project)| {
                        let info = &self.projects[project];
                        crate::hover_track::list("project-list")
                            .row(
                                div()
                                    .id(("project-pick", at))
                                    .h(px(40.))
                                    .px(px(8.))
                                    .rounded(px(crate::theme::RADIUS_MD))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .cursor_pointer()
                                    .when(selected == at, |d| d.bg(content(0.09)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.switch_project(project, cx)
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.open_context_menu(
                                                MenuTarget::Project(project),
                                                event,
                                                cx,
                                            );
                                        }),
                                    )
                                    .child(
                                        div().w(px(14.)).flex_shrink_0().when(
                                            current == Some(project),
                                            |d| {
                                                d.child(icon("check", 13.).text_color(content(0.8)))
                                            },
                                        ),
                                    )
                                    .child(
                                        div()
                                            .w(px(24.))
                                            .flex_shrink_0()
                                            .child(self.project_avatar_for(project, 24.)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(content(0.95))
                                            .child(info.name.clone()),
                                    )
                                    .children(
                                        self.project_badge(project).map(|b| self.badge_view(b)),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .font_family("Menlo")
                                            .text_size(px(11.))
                                            .text_color(content(0.4))
                                            .child(tilde_parent(&info.path)),
                                    ),
                                at,
                                selected == at,
                            )
                            .into_any_element()
                    })
                    .collect();
                let count = list.len().min(9) as f32;
                let body = div()
                    .flex()
                    .flex_col()
                    .child(search(crate::i18n::t("Buscar projetos…")))
                    .child(
                        div()
                            .id("project-list")
                            .p(px(4.))
                            .max_h(px(330.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .child(crate::hover_track::list("project-list").pill())
                            .children(list),
                    )
                    .children(self.computers_menu(cx))
                    .child(
                        div()
                            .border_t_1()
                            .border_color(content(0.08))
                            .p(px(4.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .id("project-add")
                                    .h(px(32.))
                                    .px(px(8.))
                                    .rounded(px(crate::theme::RADIUS_MD))
                                    .flex()
                                    .items_center()
                                    .gap(px(9.))
                                    .text_size(px(13.))
                                    .text_color(content(0.72))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(content(0.07)))
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.add_new_project(cx)),
                                    )
                                    .child(icon("plus", 14.).text_color(content(0.6)))
                                    .child(crate::i18n::t("Novo projeto")),
                            ),
                    );
                (
                    300.,
                    40.,
                    42. + 8. + 40. * count + 42. + self.computers_menu_height(),
                    body.into_any_element(),
                )
            }
            ProjectSub::Settings => {
                let project = &self.projects[index];
                let look = crate::project_avatar::saved_look(&project.name, &project.appearance);
                let (swatches, accessories, eyes) = crate::project_avatar::appearance_choices();
                let name_for_preview = project.name.clone();
                let color_row: Vec<AnyElement> = swatches
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(at, color)| {
                        let hex = format!("#{color:06x}");
                        let on = look["color"].as_str() == Some(hex.as_str());
                        div()
                            .id(("project-color", at))
                            .size(px(16.))
                            .rounded_full()
                            .bg(rgb(color))
                            .cursor_pointer()
                            .when(on, |d| d.border_2().border_color(gpui::white()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_project_look("color", json!(hex), cx)
                            }))
                            .into_any_element()
                    })
                    .collect();
                let choices = |field: &'static str,
                               options: &'static [&'static str],
                               base: u32,
                               cx: &mut Context<Self>|
                 -> Vec<AnyElement> {
                    options
                        .iter()
                        .copied()
                        .enumerate()
                        .map(|(at, option)| {
                            let mut sample = look.clone();
                            sample[field] = json!(option);
                            let on = look[field].as_str() == Some(option);
                            div()
                                .id(("project-look", base as usize + at))
                                .size(px(26.))
                                .rounded(px(crate::theme::RADIUS_SM))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .when(on, |d| {
                                    d.bg(content(0.12)).border_1().border_color(content(0.3))
                                })
                                .hover(|s| s.bg(content(0.07)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_project_look(field, json!(option), cx)
                                }))
                                .child(crate::project_avatar::project_avatar(
                                    &name_for_preview,
                                    &sample,
                                    24.,
                                ))
                                .into_any_element()
                        })
                        .collect()
                };
                let accessory_row = choices("accessory", accessories, 100, cx);
                let eyes_row = choices("eyes", eyes, 200, cx);
                let item =
                    |id: &'static str, glyph: &'static str, label: &'static str, more: bool| {
                        div()
                            .id(id)
                            .h(px(32.))
                            .px(px(8.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .text_size(px(13.))
                            .text_color(content(0.92))
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.07)))
                            .child(icon(glyph, 15.).text_color(content(0.65)))
                            .child(div().flex_1().child(label))
                            .when(more, |d| {
                                d.child(icon("chevron-right", 12.).text_color(content(0.45)))
                            })
                    };
                let expanded = menu.expanded;
                let project_id = project.id.clone();
                let folders: Vec<(String, String)> = self.settings["projectFolders"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| {
                        (
                            f["id"].as_str().unwrap_or("").to_owned(),
                            f["name"].as_str().unwrap_or("").to_owned(),
                        )
                    })
                    .collect();
                let in_folder = self.settings["projectFolders"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|f| {
                        f["projectIds"].as_array().is_some_and(|ids| {
                            ids.iter().any(|i| i.as_str() == Some(project_id.as_str()))
                        })
                    })
                    .and_then(|f| f["id"].as_str())
                    .map(str::to_owned);
                let sub_row = |id: (&'static str, usize), label: String, chosen: bool| {
                    div()
                        .id(id)
                        .h(px(28.))
                        .ml(px(25.))
                        .px(px(8.))
                        .rounded(px(crate::theme::RADIUS_MD))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.5))
                        .text_color(content(0.8))
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.07)))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .when(chosen, |d| {
                            d.child(icon("check", 12.).text_color(content(0.8)))
                        })
                };
                let mut move_rows: Vec<AnyElement> = Vec::new();
                if expanded == Some("move") {
                    if folders.is_empty() {
                        move_rows.push(
                            div()
                                .h(px(28.))
                                .ml(px(25.))
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .text_size(px(12.))
                                .text_color(content(0.45))
                                .child(crate::i18n::t("Nenhuma pasta criada"))
                                .into_any_element(),
                        );
                    }
                    for (at, (folder, label)) in folders.iter().cloned().enumerate() {
                        let id = project_id.clone();
                        move_rows.push(
                            sub_row(
                                ("project-folder", at),
                                label,
                                in_folder.as_deref() == Some(folder.as_str()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                move_project_to_folder(&mut this.settings, &id, Some(&folder));
                                this.backend
                                    .invoke("save_settings", json!({ "settings": this.settings }));
                                cx.notify();
                            }))
                            .into_any_element(),
                        );
                    }
                    if in_folder.is_some() {
                        let id = project_id.clone();
                        move_rows.push(
                            sub_row(
                                ("project-folder-out", 0),
                                crate::i18n::t("Tirar da pasta").to_owned(),
                                false,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                move_project_to_folder(&mut this.settings, &id, None);
                                this.backend
                                    .invoke("save_settings", json!({ "settings": this.settings }));
                                cx.notify();
                            }))
                            .into_any_element(),
                        );
                    }
                }
                let session = self.conversations[self.active_conversation()]
                    .session_id
                    .clone();
                let editor_rows: Vec<AnyElement> = if expanded == Some("editor") {
                    self.env_editors.iter().cloned().enumerate().map(|(at, (editor, label))| {
                        let session = session.clone();
                        sub_row(("project-editor", at), label, false)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.this_mac_only(cx) {
                                    this.backend.invoke("open_in_editor", json!({ "sessionId": session, "editor": editor, "path": Value::Null }));
                                }
                                this.close_project_menu();
                                cx.notify();
                            })).into_any_element()
                    }).collect()
                } else {
                    Vec::new()
                };
                let scripts_open = expanded == Some("scripts");
                let scripts_rows: Vec<AnyElement> = if scripts_open {
                    vec![self.scripts_fields_view(index, cx)]
                } else {
                    Vec::new()
                };
                let extra = (move_rows.len() + editor_rows.len()) as f32 * 28.
                    + if scripts_open {
                        crate::project_scripts_view::SETTINGS_HEIGHT
                    } else {
                        0.
                    };
                let reveal_path = project_path.clone();
                let divider = || div().mx(px(4.)).my(px(5.)).h(px(1.)).bg(content(0.08));
                let body = div()
                    .p(px(8.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(32.))
                            .px(px(10.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .border_1()
                            .border_color(content(0.14))
                            .bg(field_fill())
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .text_size(px(13.))
                            .text_color(content(0.95))
                            .child(div().min_w_0().truncate().child(query.clone()))
                            .child(caret()),
                    )
                    .child(
                        div()
                            .mt(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .size(px(42.))
                                    .flex_shrink_0()
                                    .rounded(px(crate::theme::RADIUS_MD))
                                    .border_1()
                                    .border_color(content(0.1))
                                    .bg(content(0.04))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(crate::project_avatar::project_avatar(
                                        &project.name,
                                        &project.appearance,
                                        32.,
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(content(0.45))
                                            .child(crate::i18n::t("Logo do projeto")),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(12.5))
                                            .text_color(content(0.8))
                                            .child(crate::i18n::t(
                                                "Aparece nas abas e no composer",
                                            )),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .mt(px(10.))
                            .flex()
                            .justify_between()
                            .children(color_row),
                    )
                    .child(
                        div()
                            .mt(px(10.))
                            .text_size(px(11.5))
                            .text_color(content(0.45))
                            .child(crate::i18n::t("Mascote")),
                    )
                    .child(
                        div()
                            .mt(px(4.))
                            .flex()
                            .justify_between()
                            .children(accessory_row),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .text_size(px(11.5))
                            .text_color(content(0.45))
                            .child(crate::i18n::t("Olhos")),
                    )
                    .child(div().mt(px(4.)).flex().gap(px(6.)).children(eyes_row))
                    .child(divider())
                    .child(
                        item(
                            "project-theme",
                            "palette",
                            crate::i18n::t("Cores do tema"),
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_project_menu();
                            this.open_settings_page(APPEARANCE, cx);
                        })),
                    )
                    .child(
                        item(
                            "project-background",
                            "image-plus",
                            crate::i18n::t("Imagem de fundo"),
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_project_menu();
                            this.open_settings_page(APPEARANCE, cx);
                        })),
                    )
                    .child(crate::project_scripts_view::settings_row(
                        "project-scripts",
                        scripts_open,
                        crate::project_scripts_view::click(cx, |this, cx| {
                            this.toggle_scripts_row(cx)
                        }),
                    ))
                    .children(scripts_rows)
                    .child(
                        item(
                            "project-move",
                            "chevrons-up-down",
                            crate::i18n::t("Mover"),
                            true,
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.toggle_project_row("move", cx)),
                        ),
                    )
                    .children(move_rows)
                    .child(
                        item(
                            "project-reveal",
                            "folder-open",
                            crate::i18n::t("Mostrar no Finder"),
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let _ = crate::native_actions::open_path(&reveal_path);
                            this.close_project_menu();
                            cx.notify();
                        })),
                    )
                    .child(
                        item(
                            "project-editor-open",
                            "app-window",
                            crate::i18n::t("Abrir no editor"),
                            true,
                        )
                        .on_click(
                            cx.listener(|this, _, _, cx| this.toggle_project_row("editor", cx)),
                        ),
                    )
                    .children(editor_rows)
                    .child(divider())
                    .child(
                        div()
                            .id("project-remove")
                            .h(px(32.))
                            .px(px(8.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .text_size(px(13.))
                            .text_color(rgb(0xe08a8a))
                            .cursor_pointer()
                            .hover(|s| {
                                s.bg(gpui::Hsla {
                                    a: 0.12,
                                    ..rgb(0xe08a8a).into()
                                })
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.close_project_menu();
                                this.ask_remove_project(index, cx);
                            }))
                            .child(icon("trash", 15.).text_color(rgb(0xe08a8a)))
                            .child(crate::i18n::t("Remover da lista")),
                    );
                (260., 72., 500. + extra, body.into_any_element())
            }
        };
        let top = (menu_top + top_offset)
            .min(self.viewport_height - height - 10.)
            .max(8.);
        div()
            .id("project-sub")
            .occlude()
            .absolute()
            .left(px(left + (1. - t) * -4.))
            .top(px(top))
            .w(px(width))
            .opacity(t)
            .rounded(px(crate::theme::RADIUS_LG))
            .border_1()
            .border_color(content(0.12))
            .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(body)
            .into_any_element()
    }

    /// The magnifier beside the three dots: it opens the ⌘K palette (conversations, messages,
    /// projects and commands). The "Buscar" switch of the three-dots menu shows or hides it.
    fn sidebar_search_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.sidebar_items.shown("search") {
            return None;
        }
        Some(
            div()
                .id("sidebar-search")
                .flex_shrink_0()
                .size(px(24.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| this.toggle_palette(false, cx)))
                .child(icon("search", 16.).text_color(content(0.6)))
                .into_any_element(),
        )
    }

    /// A row of the sidebar's navigation, sized as Monocode's.
    fn sidebar_nav_row(id: &'static str, active: bool) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(32.))
            .px(px(8.))
            .rounded(px(crate::theme::RADIUS_MD))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(14.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(content(if active { 1. } else { 0.5 }))
            .cursor_pointer()
            .when(active, |d| d.bg(content(0.1)))
            .hover(|s| s.text_color(content(1.)))
    }

    /// "New" (⌘T), at the top of the sidebar: opens a new conversation.
    fn sidebar_new(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.sidebar_items.shown("new") {
            return None;
        }
        Some(
            div()
                .flex_shrink_0()
                .px(px(6.))
                .pt(px(4.))
                .child(
                    crate::hover_track::list("sidebar-nav")
                        .row_named(Self::sidebar_nav_row("sidebar-new", false), "new", false)
                        .on_click(cx.listener(|this, _, _, cx| this.new_conversation(None, cx)))
                        .child(icon("square-plus", 16.).text_color(content(0.35)))
                        .child(div().flex_1().min_w_0().truncate().child(t("Novo")))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(12.))
                                .font_weight(FontWeight::NORMAL)
                                .text_color(content(0.35))
                                .child("⌘T"),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The Pull requests entry below "New", marked while its tab is the one showing.
    fn sidebar_pulls(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.sidebar_items.shown("pulls") {
            return None;
        }
        let active = self.term.pulls_active;
        Some(
            div()
                .flex_shrink_0()
                .px(px(6.))
                .pb(px(6.))
                .child(
                    crate::hover_track::list("sidebar-nav")
                        .row_named(Self::sidebar_nav_row("sidebar-pulls", active), "pulls", active)
                        .on_click(cx.listener(|this, _, _, cx| this.open_pull_requests(cx)))
                        .child(icon("git-pull-request", 16.).text_color(content(if active {
                            0.7
                        } else {
                            0.35
                        })))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(t("Pull requests")),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Opens the three-dots menu, or fades it out when it is open.
    fn toggle_sidebar_options(&mut self, cx: &mut Context<Self>) {
        if self.close_sidebar_options(cx) {
            return;
        }
        self.close_project_menu();
        self.sidebar_options = Some(Tween::from_to(
            0.,
            1.,
            Instant::now(),
            if cx.reduce_motion() { 0. } else { 0.16 },
            Curve::Expo,
        ));
        cx.notify();
    }

    /// Starts the menu's fade-out; says whether it was open (and not already leaving).
    fn close_sidebar_options(&mut self, cx: &mut Context<Self>) -> bool {
        let duration = if cx.reduce_motion() { 0. } else { 0.12 };
        match self.sidebar_options.as_mut() {
            Some(open) if open.to > 0.5 => {
                open.set(0., Instant::now(), duration, Curve::Expo);
                cx.notify();
                true
            }
            _ => false,
        }
    }

    /// The three dots beside the project's name.
    fn sidebar_options_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.sidebar_options_at.clone();
        let open = self.sidebar_options.is_some_and(|t| t.to > 0.5);
        div()
            .id("sidebar-options")
            .relative()
            .flex_shrink_0()
            .size(px(24.))
            .rounded(px(crate::theme::RADIUS_MD))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(open, |d| d.bg(content(0.09)))
            .hover(|s| s.bg(content(0.07)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar_options(cx)))
            .child(
                gpui::canvas(move |bounds, _, _| store.set(Some(bounds)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(icon("more-horizontal", 16.).text_color(content(0.6)))
            .into_any_element()
    }

    /// A switch per optional sidebar entry (the table in sidebar_items.rs), under the button.
    fn sidebar_options_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.sidebar_options.as_ref()?;
        let button = self.sidebar_options_at.get()?;
        if !open.done(now) {
            *moving = true;
        }
        let shown = open.at(now).clamp(0., 1.);
        let rows = crate::sidebar_items::ITEMS
            .iter()
            .enumerate()
            .map(|(at, item)| {
                let on = self.sidebar_items.shown(item.id);
                let id = item.id;
                let row = div()
                    .id(("sidebar-option", at))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .text_color(content(0.92))
                    .cursor_pointer()
                    .child(icon(item.icon, 14.).text_color(content(0.65)))
                    .child(div().flex_1().min_w_0().truncate().child(t(item.label)))
                    // Monocode's menu switch: neutral, never the accent.
                    .child(
                        div()
                            .relative()
                            .w(px(36.))
                            .h(px(20.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(content(if on { 0.35 } else { 0.15 }))
                            .child(
                                div()
                                    .absolute()
                                    .top(px(2.))
                                    .left(px(if on { 18. } else { 2. }))
                                    .size(px(16.))
                                    .rounded_full()
                                    .bg(content(1.)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sidebar_items.toggle(id);
                        crate::sidebar_items::save(&this.sidebar_items);
                        cx.notify();
                    }));
                crate::hover_track::list("sidebar-options").row(row, at, false)
            });
        const WIDTH: f32 = 184.;
        Some(
            div()
                .id("sidebar-options-menu")
                .occlude()
                .absolute()
                .left(px(f32::from(button.left()).max(8.)))
                .top(px(f32::from(button.bottom()) + 4. + (1. - shown) * -6.))
                .w(px(WIDTH))
                .p(px(4.))
                .flex()
                .flex_col()
                .opacity(shown)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(crate::hover_track::list("sidebar-options").pill())
                .children(rows)
                .into_any_element(),
        )
    }

    /// The Usage page, right beside Configurações.
    fn usage_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("sidebar-usage-page")
            .h(px(22.))
            .px(px(6.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(content(0.55))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.08)).text_color(content(0.95)))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_usage_page(cx)))
            .child(icon("chart-column", 13.).text_color(content(0.55)))
            .child(t("Uso"))
    }

    /// Settings stay one click away at the footer's right end.
    fn settings_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("sidebar-settings")
            .h(px(22.))
            .px(px(6.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(content(0.55))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.08)).text_color(content(0.95)))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_appearance(cx)))
            .child(icon("settings", 13.).text_color(content(0.55)))
            .child(t("Configurações"))
    }

    fn set_peek(&mut self, open: bool, cx: &mut Context<Self>) {
        self.peek_close = None;
        if open && (self.sidebar_open || self.sidebar.at(Instant::now()) > 0.001) {
            return;
        }
        let now = Instant::now();
        let from = self.peek.as_ref().map_or(0., |p| p.t.at(now));
        if self.peek.as_ref().is_some_and(|p| p.open == open) || (self.peek.is_none() && !open) {
            return;
        }
        let duration = if cx.reduce_motion() {
            0.
        } else if open {
            0.24
        } else {
            0.2
        };
        self.peek = Some(Peek {
            open,
            t: Tween::from_to(from, if open { 1. } else { 0. }, now, duration, Curve::Expo),
        });
        cx.notify();
    }

    /// The window's pointer drives the peek: the left edge opens it; leaving the panel, after
    /// a short grace, puts it away.
    fn track_peek(&mut self, position: Point<Pixels>, height: f32, cx: &mut Context<Self>) {
        if self.sidebar_open {
            return;
        }
        let (x, y) = (f32::from(position.x), f32::from(position.y));
        let on_edge = x <= PEEK_EDGE && y >= TITLE_BAR;
        let on_toggle = (78.0..104.).contains(&x) && y < TITLE_BAR;
        let on_panel = self.peek.as_ref().is_some_and(|p| p.open)
            && x >= PEEK_INSET
            && x <= PEEK_INSET + self.sidebar_width
            && y >= TITLE_BAR
            && y <= height - PEEK_INSET;
        if on_edge
            || on_toggle
            || on_panel
            || self.palette_ui.is_some()
            || self.appearance.is_some()
            || self.project_menu.is_some()
            || self.context_menu.is_some()
            || self.session_filter.open.is_some()
        {
            if on_edge {
                self.set_peek(true, cx);
            } else if self.peek.as_ref().is_some_and(|p| p.open) {
                self.peek_close = None;
            }
            return;
        }
        if self.peek.as_ref().is_some_and(|p| p.open) && self.peek_close.is_none() {
            self.peek_close = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PEEK_CLOSE).await;
                let _ = this.update(cx, |this, cx| {
                    this.peek_close = None;
                    this.set_peek(false, cx);
                });
            }));
        }
    }

    fn conversation_row(
        &self,
        index: usize,
        y: f32,
        alpha: f32,
        active: bool,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let conversation = &self.conversations[index];
        // The draft title while this row is being renamed in place.
        let draft: Option<String> = if self.inline_rename_row() == Some(index) {
            self.context_menu.as_ref().and_then(|m| m.renaming.clone())
        } else {
            None
        };
        let editing = draft.is_some();
        // Where the row was painted, for its hover card (sidebar_card.rs).
        let place = sidebar_card::Place::default();
        let row = div()
            .id(("conversation", index))
            .absolute()
            .left(px(6.))
            .right(px(6.))
            .top(px(y))
            .h(px(CONV_ROW - 2.))
            .opacity(alpha)
            .rounded(px(crate::theme::RADIUS_MD))
            .pl(px(10.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .when(active, |d| d.bg(content(0.09)))
            .when(editing, |d| {
                d.bg(content(0.05))
                    .border_1()
                    .border_color(content(0.3))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            })
            .when(!editing, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.open(index, cx)))
                    .on_drag(
                        split_panes::ConvDrag {
                            session: conversation.session_id.clone(),
                            title: conversation.title.clone(),
                        },
                        |drag, _, _, cx| cx.new(|_| split_panes::DragChip::new(drag.title.clone())),
                    )
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_context_menu(MenuTarget::Session(index), event, cx);
                }),
            )
            .on_hover({
                let (view, place) = (cx.entity(), place.clone());
                move |hovered: &bool, _, cx| {
                    let hovered = *hovered;
                    view.update(cx, |this, cx| {
                        this.sidebar_card_hover(index, place.get(), hovered, cx)
                    });
                }
            })
            .child(place.probe())
            // The 2 px gap under the row belongs to it, so the pointer crosses rows unbroken.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(CONV_ROW - 2.))
                    .h(px(2.)),
            )
            .child(self.conversation_mark(conversation, now, active))
            .child(match &draft {
                Some(text) => inline_name_field(text),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(content(if active { 1. } else { 0.68 }))
                    .child(conversation.title.clone())
                    .into_any_element(),
            })
            .when(!editing, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(content(0.35))
                        .child(conversation.age()),
                )
            });
        crate::hover_track::list("sidebar")
            .row(row, index, active)
            .into_any_element()
    }

    fn transcript(
        &mut self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let index = self.active_conversation();
        let list_state = self.list_state(index, cx);
        let wanted = 1 + self.conversations[index].turns.len();
        // A drag past the edge scrolls (transcript_select.rs); it keeps the frames coming.
        let dragged = self.select_autoscroll(&list_state);
        if dragged {
            self.glides.entry(index).or_default().leave();
        }
        *moving |= dragged;
        let listed = list_state.item_count();
        if listed < wanted {
            list_state.splice(listed..listed, wanted - listed);
        } else if listed > wanted {
            list_state.reset(wanted);
        }
        // ⌘F: the hits follow the query and the turns (transcript_find.rs); a new query reveals its first hit.
        let turns = &self.conversations[index].turns;
        if let Some(turn) = self.find.sync(index, turns.len(), |i| {
            (turns[i].user.as_str(), turns[i].answer.as_str())
        }) {
            list_state.pause_following_tail();
            self.glides.entry(index).or_default().leave();
            list_state.scroll_to_reveal_item(turn + 1);
        }
        let switching = self.switch.at(now);
        *moving |= !self.switch.done(now);
        let live = running(&self.conversations[index], now);
        // Away from the end, a button brings the latest message back.
        // Following the end is a glide of ours (glide.rs): the list stays paused and is moved a
        // little each frame, so a sent message and a growing reply slide instead of jumping.
        let (current, max) = (
            -f32::from(list_state.scroll_px_offset_for_scrollbar().y),
            f32::from(list_state.max_offset_for_scrollbar().y),
        );
        let viewport = list_state.viewport_bounds().size;
        let view_height = f32::from(viewport.height);
        self.view_sizes
            .insert(index, (f32::from(viewport.width), view_height));
        let reduce_motion = cx.reduce_motion();
        let glide = self.glides.entry(index).or_default();
        let plan = glide.step(current, max, view_height, now, reduce_motion);
        if glide.stick {
            list_state.pause_following_tail();
        }
        if plan.scroll > 0. {
            list_state.scroll_by(px(plan.scroll));
        }
        *moving |= plan.moving;
        let anchored = glide.anchored;
        let away = !glide.stick;
        self.jump.set(
            if away { 1. } else { 0. },
            now,
            if cx.reduce_motion() { 0. } else { 0.16 },
            Curve::Expo,
        );
        *moving |= !self.jump.done(now);
        let jump = self.jump.at(now).clamp(0., 1.);
        let trail = self.message_trail(index, &list_state, cx);
        let items = gpui::list(
            list_state.clone(),
            cx.processor(move |this: &mut Self, item: usize, window, cx| {
                let column = |content: AnyElement, bottom: f32| {
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .w_full()
                                .max_w(px(832.))
                                .px(px(23.))
                                .pb(px(bottom))
                                .child(content),
                        )
                        .into_any_element()
                };
                if item == 0 {
                    let older = this.conversations[index].base > 0;
                    return div()
                        .w_full()
                        .pt(px(20.))
                        .pb(px(if older { 14. } else { 0. }))
                        .when(older, |d| {
                            d.child(
                                div().flex().justify_center().child(
                                    div()
                                        .id("load-older")
                                        .h(px(26.))
                                        .px(px(10.))
                                        .rounded(px(crate::theme::RADIUS_MD))
                                        .flex()
                                        .items_center()
                                        .text_size(px(12.5))
                                        .text_color(content(0.55))
                                        .cursor_pointer()
                                        .hover(|s| s.bg(content(0.07)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.load_older(index, cx)
                                        }))
                                        .child(t("Carregar mensagens anteriores")),
                                ),
                            )
                        })
                        .into_any_element();
                }
                // A user message past four wrapped lines collapses (prompt_overflows in turn_body).
                if let Some(text) = this.conversations[index]
                    .turns
                    .get(item - 1)
                    .map(|turn| turn.user.clone())
                    .filter(|text| !text.trim().is_empty())
                {
                    let view = this.view_sizes.get(&index).map_or(0., |size| size.0);
                    let column = if view > 0. { view.min(832.) } else { 832. } - 46.;
                    let over = prompt_lines_over(&text, column.min(576.) - 24., window) > 4;
                    this.prompt_overflow.insert((index, item - 1), over);
                }
                let Some(turn) = this.conversations[index].turns.get(item - 1) else {
                    return div().into_any_element();
                };
                let last = item == this.conversations[index].turns.len();
                let mut animating = false;
                let view =
                    this.turn_view(index, item - 1, turn, Instant::now(), &mut animating, cx);
                if animating {
                    window.request_animation_frame();
                }
                let view = if last && anchored {
                    div()
                        // Leaves room above the sent message, clear of the fade at the top.
                        .min_h(px((view_height - 28. - ANCHOR_TOP).max(0.)))
                        .child(view)
                        .into_any_element()
                } else {
                    view
                };
                column(view, if last { 28. } else { 14. })
            }),
        )
        .size_full();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .opacity(switching.clamp(0., 1.))
                    .child(
                        div()
                            .relative()
                            .size_full()
                            .top(px((1. - switching) * 6.))
                            .child(items),
                    )
                    .children(trail)
                    .children(self.find_bar(now, moving, cx))
                    .when(jump > 0.01, |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(10. + jump * 4.))
                                .left_0()
                                .right_0()
                                .flex()
                                .justify_center()
                                .opacity(jump)
                                .child(
                                    div()
                                        .id("jump-latest")
                                        .size(px(32.))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(content(0.14))
                                        .relative()
                                        .child(crate::theme::floating_surface(16.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(content(0.1)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.glides.entry(index).or_default().follow(false);
                                            cx.notify();
                                        }))
                                        .child(icon("chevron-down", 15.).text_color(content(0.8))),
                                ),
                        )
                    }),
            )
            // Text fades as it scrolls under the top edge and toward the composer.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(26.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(pane_fill(), 0.),
                        linear_color_stop(transparent(), 1.),
                    )),
            )
            .children({
                *moving |= self.conversations[index].working();
                self.new_tab_field(index, now, moving, cx)
            })
            .children(self.new_tab_greeting(index))
            .child(self.composer(live, cx))
            .children(self.switch_view(now, moving))
            .into_any_element()
    }

    /// The moving background behind a new tab's greeting (new_tab_field.rs).
    fn new_tab_field(
        &mut self,
        index: usize,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let conversation = &self.conversations[index];
        if !conversation.is_blank() {
            return None;
        }
        let project = self.projects.get(self.project_of(index)?)?;
        let (shape, color) = crate::project_avatar::look_of(&project.name, &project.appearance);
        *moving |= self.new_tab_fx.moving(now);
        Some(crate::new_tab_field::field(
            shape,
            color,
            &self.new_tab_fx,
            |this: &mut Shell| &mut this.new_tab_fx,
            cx.entity().downgrade(),
            cx.reduce_motion(),
        ))
    }

    /// A conversation with nothing in it yet: its project's icon in the middle, a line under it,
    /// and the composer stays at the bottom.
    fn new_tab_greeting(&self, index: usize) -> Option<AnyElement> {
        let conversation = &self.conversations[index];
        if !conversation.is_blank() {
            return None;
        }
        let project = self.projects.get(self.project_of(index)?)?;
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom(px(150.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(14.))
                .child(crate::project_avatar::project_avatar(
                    &project.name,
                    &project.appearance,
                    56.,
                ))
                .child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(0.92))
                        .child(tf("O que vamos fazer em {}?", &[&project.name])),
                )
                .into_any_element(),
        )
    }

    /// Composer B: the field is a raised card over a shelf holding the session's settings.
    /// Typing comes with the real composer in part 2; Enter simulates a reply.
    /// Changing provider: its logo forms in pixels, like the project avatars, then comes apart.
    fn switch_view(&self, now: Instant, moving: &mut bool) -> Option<AnyElement> {
        const DURATION: f32 = 1.4;
        let (provider, start) = self.switch_anim?;
        let a = now.duration_since(start).as_secs_f32() / DURATION;
        if a >= 1. {
            return None;
        }
        *moving = true;
        let fill: gpui::Hsla = match provider {
            "claude" => rgb(0xe3876a).into(),
            "codex" => rgb(0x7a8cff).into(),
            _ => content(0.75),
        };
        let mask = crate::provider_masks::mask(provider);
        Some(
            div()
                .absolute()
                .left(gpui::relative(0.5))
                .top(gpui::relative(0.4))
                .ml(px(-66.))
                .mt(px(-66.))
                .size(px(132.))
                .child(
                    gpui::canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            const CELL: f32 = 6.;
                            for (y, row) in mask.iter().enumerate() {
                                for (x, c) in row.bytes().enumerate() {
                                    if c != b'#' {
                                        continue;
                                    }
                                    // A steady random per pixel: each arrives and leaves in its own time.
                                    let r = (((x * 73 + y * 151) * 2654435761usize % 1000) as f32)
                                        / 1000.;
                                    let arrive = (a / 0.45 / (0.35 + r * 0.65)).min(1.);
                                    let leave = if a > 0.7 {
                                        (1. - (a - 0.7) / 0.3 / (0.3 + r * 0.7)).max(0.)
                                    } else {
                                        1.
                                    };
                                    if arrive <= 0. || leave <= 0. {
                                        continue;
                                    }
                                    let drop = (1. - arrive) * 14. * (r - 0.5) * CELL;
                                    let origin = bounds.origin
                                        + gpui::point(
                                            px(x as f32 * CELL),
                                            px(y as f32 * CELL + drop),
                                        );
                                    window.paint_quad(gpui::fill(
                                        Bounds::new(
                                            origin,
                                            gpui::size(px(CELL - 1.), px(CELL - 1.)),
                                        ),
                                        gpui::Hsla {
                                            a: fill.a * arrive.min(leave),
                                            ..fill
                                        },
                                    ));
                                }
                            }
                        },
                    )
                    .size_full(),
                )
                .into_any_element(),
        )
    }

    /// What the model allows and what is chosen, as Sirus Code's `modelExecutionControls`.
    fn execution(&self, provider: &str, model: Option<&str>) -> Execution {
        let info = model.and_then(|id| {
            self.models
                .get(provider)
                .and_then(|list| list.iter().find(|m| m.id == id))
        });
        // Cursor's plain models carry effort and Fast in their id: moving the bar changes the model.
        if let (true, Some(current)) = (
            provider == "cursor" && info.is_some_and(|m| !m.parameterized),
            info,
        ) {
            let (stem, effort, fast) = cursor_preset(&current.id);
            let list = self.models.get(provider).map_or(&[][..], Vec::as_slice);
            let family: Vec<_> = list
                .iter()
                .filter(|m| m.available && cursor_preset(&m.id).0 == stem)
                .map(|m| cursor_preset(&m.id))
                .collect();
            let levels = EFFORT_ORDER
                .iter()
                .filter(|level| family.iter().any(|f| f.1 == Some(**level)))
                .map(|level| (*level).to_owned())
                .collect();
            let fast_ok = family.iter().any(|f| f.1 == effort && f.2 != fast);
            return Execution {
                levels,
                effort: effort.map(str::to_owned),
                fast,
                fast_ok,
            };
        }
        let preference = model.map_or(Value::Null, |id| {
            self.settings["modelExecution"][format!("{provider}::{id}")].clone()
        });
        let levels: Vec<String> = EFFORT_ORDER
            .iter()
            .filter(|level| info.is_some_and(|m| m.efforts.iter().any(|e| e == *level)))
            .map(|level| (*level).to_owned())
            .collect();
        let chosen = preference["effort"]
            .as_str()
            .filter(|e| levels.iter().any(|l| l == e))
            .map(str::to_owned);
        let default = info
            .and_then(|m| m.default_effort.clone())
            .filter(|e| levels.contains(e));
        let effort = chosen
            .or(default)
            .or_else(|| levels.get(levels.len() / 2).cloned());
        let fast_ok = matches!(provider, "claude" | "codex") && info.is_some_and(|m| m.fast);
        let fast = fast_ok
            && preference["fast"]
                .as_bool()
                .unwrap_or_else(|| info.is_some_and(|m| m.default_fast));
        Execution {
            levels,
            effort,
            fast,
            fast_ok,
        }
    }

    /// The name people use, from the provider's catalog: "Haiku 5.5", not the alias "haiku".
    fn model_display(&self, provider: &str, model: Option<&str>) -> String {
        let named = model
            .and_then(|id| self.models.get(provider)?.iter().find(|m| m.id == id))
            .filter(|m| !m.name.is_empty());
        match named {
            Some(info) => info
                .name
                .strip_prefix("Claude ")
                .unwrap_or(&info.name)
                .to_owned(),
            None => model_label(model, provider),
        }
    }

    fn current_execution(&self) -> Execution {
        let conversation = &self.conversations[self.active_conversation()];
        self.execution(conversation.provider, conversation.model.as_deref())
    }

    /// Saves effort or Fast for the current model, shared by every conversation using it.
    fn set_execution(&mut self, field: &str, value: Value) {
        let conversation = &self.conversations[self.active_conversation()];
        let Some(model) = conversation.model.clone() else {
            return;
        };
        if conversation.provider == "cursor"
            && self
                .models
                .get("cursor")
                .and_then(|l| l.iter().find(|m| m.id == model))
                .is_some_and(|m| !m.parameterized)
        {
            let (stem, effort, fast) = cursor_preset(&model);
            let (effort, fast) = match field {
                "effort" => (value.as_str().map(str::to_owned), fast),
                _ => (effort.map(str::to_owned), value.as_bool().unwrap_or(fast)),
            };
            let next = self.models["cursor"]
                .iter()
                .find(|m| {
                    let preset = cursor_preset(&m.id);
                    m.available
                        && preset.0 == stem
                        && preset.1.map(str::to_owned) == effort
                        && preset.2 == fast
                })
                .map(|m| m.id.clone());
            if let Some(next) = next {
                let ix = self.active_conversation();
                let session = self.conversations[ix].session_id.clone();
                self.conversations[ix].model = Some(next.clone());
                let call = self.backend.invoke(
                    "set_session_model",
                    json!({ "sessionId": session, "agent": "cursor", "model": next }),
                );
                self.waiting.insert(call, Waiting::Session(ix));
            }
            return;
        }
        let key = format!("{}::{model}", conversation.provider);
        if !self.settings["modelExecution"].is_object() {
            self.settings["modelExecution"] = json!({});
        }
        self.settings["modelExecution"][key][field] = value;
        self.backend
            .invoke("save_settings", json!({ "settings": self.settings }));
    }

    fn pick_effort_at(&mut self, x: f32, cx: &mut Context<Self>) {
        let execution = self.current_execution();
        if execution.levels.len() < 2 {
            return;
        }
        let Some(bounds) = self.slider_bounds.borrow()[2] else {
            return;
        };
        let share = ((x - f32::from(bounds.left()) - 14.) / (f32::from(bounds.size.width) - 28.))
            .clamp(0., 1.);
        self.step_effort(
            (share * (execution.levels.len() - 1) as f32).round() as usize,
            cx,
        );
    }

    fn step_effort(&mut self, pick: usize, cx: &mut Context<Self>) {
        let execution = self.current_execution();
        let Some(level) = execution.levels.get(pick).cloned() else {
            return;
        };
        if execution.effort.as_ref() == Some(&level) {
            return;
        }
        let now = Instant::now();
        self.effort_label_from = Some((
            execution
                .effort
                .as_deref()
                .map_or_else(|| t("Automático").to_owned(), effort_label),
            now,
        ));
        self.set_execution("effort", json!(level));
        self.effort_bar.set(
            pick as f32 / (execution.levels.len() - 1) as f32,
            now,
            if cx.reduce_motion() { 0. } else { 0.2 },
            Curve::Expo,
        );
        cx.notify();
    }

    fn snap_effort_bar(&mut self) {
        let execution = self.current_execution();
        self.effort_bar = Tween::still(execution.share(), Instant::now());
        self.effort_label_from = None;
    }

    /// The providers the picker offers: installed and not turned off, in Sirus Code's order.
    fn enabled_providers(&self) -> Vec<&'static str> {
        let off: Vec<&str> = self.settings["disabledProviders"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        PROVIDER_ORDER
            .iter()
            .copied()
            .filter(|id| self.agent_versions.contains_key(id) && !off.contains(id))
            .collect()
    }

    fn favorite_models(&self) -> Vec<String> {
        self.settings["favoriteModels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }

    /// The rows for a scope, as Sirus Code's `pickerModelChoices`: the newest of each model line,
    /// keeping anything favorited, selected or found by a search.
    fn model_rows(&self) -> Vec<ModelRow> {
        let conversation = &self.conversations[self.active_conversation()];
        let favorites = self.favorite_models();
        let needle = self.model_query.trim().to_lowercase();
        let providers = if self.model_scope == "favorites" {
            self.enabled_providers()
        } else {
            self.enabled_providers()
                .into_iter()
                .filter(|p| *p == self.model_scope)
                .collect()
        };
        let mut rows = Vec::new();
        for provider in providers {
            let list = self.models.get(provider).map_or(&[][..], Vec::as_slice);
            let mut group: Vec<ModelRow> = list
                .iter()
                .filter(|m| m.available)
                .filter_map(|m| {
                    let favorite = favorites.contains(&format!("{provider}::{}", m.id));
                    let current = conversation.provider == provider
                        && conversation.model.as_deref() == Some(m.id.as_str());
                    if !needle.is_empty() {
                        if !format!("{} {} {}", m.id, m.name, provider_title(provider))
                            .to_lowercase()
                            .contains(&needle)
                        {
                            return None;
                        }
                    } else if !current && !favorite {
                        if m.id.eq_ignore_ascii_case("default")
                            || m.name.to_lowercase().starts_with("default")
                        {
                            return None;
                        }
                    }
                    let name = if m.name.is_empty() {
                        m.id.clone()
                    } else {
                        m.name.strip_prefix("Claude ").unwrap_or(&m.name).to_owned()
                    };
                    Some(ModelRow {
                        provider,
                        id: m.id.clone(),
                        name,
                        favorite,
                        selected: current,
                    })
                })
                .collect();
            if provider == "cursor" {
                let mut families: Vec<String> = Vec::new();
                group.retain(|row| {
                    let parameterized = list.iter().any(|m| m.id == row.id && m.parameterized);
                    let family = cursor_preset(&row.id).0;
                    if parameterized || row.selected || !families.contains(&family) {
                        families.push(family);
                        true
                    } else {
                        false
                    }
                });
            }
            group.sort_by(|a, b| {
                model_line(&b.name)
                    .map(|l| l.1)
                    .cmp(&model_line(&a.name).map(|l| l.1))
                    .then_with(|| a.name.cmp(&b.name))
            });
            rows.extend(group);
        }
        if self.model_scope == "favorites" {
            rows.retain(|row| row.favorite);
        }
        rows
    }

    fn browse_models(&mut self, scope: &str, cx: &mut Context<Self>) {
        if self.model_scope == scope {
            return;
        }
        let tabs = self.enabled_providers();
        let at = if scope == "favorites" {
            0
        } else {
            tabs.iter().position(|p| *p == scope).map_or(0, |i| i + 1)
        };
        let now = Instant::now();
        self.model_glide.set(
            at as f32,
            now,
            if cx.reduce_motion() { 0. } else { 0.32 },
            Curve::Expo,
        );
        self.model_scope = scope.to_owned();
        self.model_query.clear();
        self.model_selected = None;
        self.model_list_at = now;
        cx.notify();
    }

    fn pick_model(&mut self, provider: &'static str, id: String, cx: &mut Context<Self>) {
        let ix = self.active_conversation();
        let session = self.conversations[ix].session_id.clone();
        if self.conversations[ix].provider != provider {
            self.switch_anim = Some((provider, Instant::now()));
        }
        self.conversations[ix].provider = provider;
        self.conversations[ix].model = Some(id.clone());
        let call = self.backend.invoke(
            "set_session_model",
            json!({ "sessionId": session, "agent": provider, "model": id }),
        );
        self.waiting.insert(call, Waiting::Session(ix));
        self.model_query.clear();
        self.snap_effort_bar();
        if let Some((which, _)) = self.composer_menu.as_mut() {
            *which = ComposerMenu::Effort;
        }
        cx.notify();
    }

    fn toggle_favorite_model(&mut self, provider: &str, id: &str, cx: &mut Context<Self>) {
        let key = format!("{provider}::{id}");
        let mut favorites = self.favorite_models();
        if let Some(at) = favorites.iter().position(|f| *f == key) {
            favorites.remove(at);
        } else {
            favorites.push(key);
        }
        self.settings["favoriteModels"] = json!(favorites);
        self.backend
            .invoke("save_settings", json!({ "settings": self.settings }));
        cx.notify();
    }

    fn set_model_page(&mut self, page: ComposerMenu, cx: &mut Context<Self>) {
        if page == ComposerMenu::Catalog {
            let provider = self.conversations[self.active_conversation()].provider;
            self.model_scope = provider.to_owned();
            self.model_query.clear();
            self.model_selected = None;
            let at = self
                .enabled_providers()
                .iter()
                .position(|p| *p == provider)
                .map_or(0, |i| i + 1);
            self.model_glide = Tween::still(at as f32, Instant::now());
            self.model_list_at = Instant::now();
        } else {
            self.snap_effort_bar();
        }
        if let Some((which, _)) = self.composer_menu.as_mut() {
            *which = page;
        }
        cx.notify();
    }

    fn toggle_composer_menu(&mut self, which: ComposerMenu, cx: &mut Context<Self>) {
        let model_menu = |menu: ComposerMenu| menu != ComposerMenu::Access;
        let same = self
            .composer_menu
            .as_ref()
            .is_some_and(|(m, _)| *m == which || (model_menu(*m) && model_menu(which)));
        self.composer_menu = if same {
            None
        } else {
            Some((
                which,
                Tween::from_to(
                    0.,
                    1.,
                    Instant::now(),
                    if cx.reduce_motion() { 0. } else { 0.16 },
                    Curve::Expo,
                ),
            ))
        };
        if !same && which == ComposerMenu::Effort {
            self.model_menu_anchor = self.model_trigger.get().map(|bounds| {
                (
                    f32::from(bounds.left()) + f32::from(bounds.size.width) / 2.,
                    self.viewport_height - f32::from(bounds.top()) + 6.,
                )
            });
            self.composer_menu_at = Instant::now();
            // With no model yet there is nothing to adjust: the catalog opens first.
            let page = if self.conversations[self.active_conversation()]
                .model
                .is_some()
            {
                ComposerMenu::Effort
            } else {
                ComposerMenu::Catalog
            };
            self.set_model_page(page, cx);
        }
        cx.notify();
    }

    fn access_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let approval = self.conversations[self.active_conversation()]
            .approval
            .clone();
        let track = crate::hover_track::list("access-menu");
        let mut rows = vec![
            div()
                .px(px(8.))
                .pt(px(4.))
                .pb(px(4.))
                .text_size(px(11.5))
                .text_color(content(0.45))
                .child(t("Como as ações do agente devem ser aprovadas?"))
                .into_any_element(),
        ];
        rows.extend(
            ["ask", "auto", "full"]
                .into_iter()
                .enumerate()
                .map(|(i, mode)| {
                    let (glyph, label, about, color) = access_look(mode);
                    let chosen = approval == mode;
                    track
                        .row(
                            div()
                                .id(("access", i))
                                .px(px(8.))
                                .py(px(6.))
                                .rounded(px(crate::theme::RADIUS_MD))
                                .flex()
                                .gap(px(8.))
                                .cursor_pointer()
                                .when(chosen, |d| d.bg(content(0.1)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let ix = this.active_conversation();
                                    this.conversations[ix].approval = mode.to_owned();
                                    if mode == "full" {
                                        this.conversations[ix].planning = false;
                                    }
                                    this.composer_menu = None;
                                    cx.notify();
                                }))
                                .child(div().mt(px(2.)).child(icon(glyph, 15.).text_color(color)))
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .text_size(px(13.))
                                        .text_color(content(0.92))
                                        .child(label)
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(content(0.5))
                                                .child(about),
                                        ),
                                )
                                .when(chosen, |d| {
                                    d.child(
                                        div()
                                            .mt(px(3.))
                                            .child(icon("check", 13.).text_color(content(0.6))),
                                    )
                                }),
                            i,
                            chosen,
                        )
                        .into_any_element()
                }),
        );
        rows
    }

    /// Sirus Code's effort page: 280 wide, a 32 tall bar and a thumb that steps through levels.
    fn effort_page(
        &self,
        now: Instant,
        moving: &mut bool,
        reduce: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let conversation = &self.conversations[self.active_conversation()];
        let provider = conversation.provider;
        let model_id = conversation.model.clone().unwrap_or_default();
        let execution = self.execution(provider, conversation.model.as_deref());
        let count = execution.levels.len();
        let share = self.effort_bar.at(now);
        if !self.effort_bar.done(now) {
            *moving = true;
        }
        let heat = if count > 1 { share } else { 0. };
        let base: gpui::Hsla = gpui::rgb(model_brand(&model_id, provider)).into();
        let tone = effort_energy::blend(base, gpui::rgb(0xff4d3d).into(), heat * 0.85);
        let soft = effort_energy::blend(tone, gpui::rgb(0xe4b1f3).into(), 0.55);
        let cool = effort_energy::blend(tone, gpui::rgb(0x8bb8ff).into(), 0.72);
        let label = execution
            .effort
            .as_deref()
            .map_or_else(|| t("Automático").to_owned(), effort_label);
        // The old level leaves upward, then the new one arrives from below.
        let (shown, rise, seen) = match self.effort_label_from.as_ref() {
            Some((old, at)) => {
                let since = now.saturating_duration_since(*at).as_secs_f32();
                if since < 0.14 {
                    *moving = true;
                    (old.clone(), -8. * since / 0.14, 1. - since / 0.14)
                } else if since < 0.28 {
                    *moving = true;
                    let p = eased((since - 0.14) / 0.14, Curve::Quad);
                    (label.clone(), 8. * (1. - p), p)
                } else {
                    (label.clone(), 0., 1.)
                }
            }
            None => (label.clone(), 0., 1.),
        };
        let fill = match count {
            0 => 1.,
            1 => 0.945,
            // Ends at the thumb's center, wherever it is.
            _ => (16. + 224. * share) / 256.,
        };
        let energy = effort_energy::Energy {
            fill,
            tone,
            soft,
            cool,
            heat,
            embers: if count > 1 {
                2 + (heat * 8.).round() as usize
            } else {
                0
            },
            fast: execution.fast,
            time: if reduce {
                1.2
            } else {
                now.saturating_duration_since(self.composer_menu_at)
                    .as_secs_f32()
            },
        };
        if !reduce && (count > 0 || execution.fast_ok) {
            *moving = true;
        }
        let store = self.slider_bounds.clone();
        let shell = cx.entity().downgrade();
        let fast = execution.fast;
        let fast_ok = execution.fast_ok;
        let thumb_tint = effort_energy::blend(gpui::white(), tone, 0.13);
        let model = self.model_display(provider, conversation.model.as_deref());
        let first = execution.levels.first().map(|l| effort_label(l));
        let last = execution.levels.last().map(|l| effort_label(l));
        div()
            .p(px(12.))
            .flex()
            .flex_col()
            .child(
                div().mb(px(12.)).flex().items_center().gap(px(8.))
                    .child(
                        div().id("effort-fast").size(px(32.)).flex_shrink_0().rounded_full().flex().items_center().justify_center()
                            .bg(gpui::Hsla { a: if fast { 0.22 } else { 0.09 }, ..tone })
                            .when(!fast_ok, |d| d.opacity(0.35))
                            .when(fast_ok, |d| d.cursor_pointer().hover(|s| s.bg(gpui::Hsla { a: 0.19, ..tone })).on_click(cx.listener(move |this, _, _, cx| {
                                this.set_execution("fast", json!(!fast));
                                cx.notify();
                            })))
                            .child(theme_icon(if fast { "zap-fill" } else { "zap" }, 20.).text_color(tone)),
                    )
                    .child(
                        div().flex_1().min_w_0().flex().flex_col().items_center()
                            .child(div().h(px(24.)).overflow_hidden().child(div().relative().top(px(rise)).opacity(seen).text_size(px(15.)).line_height(px(24.)).font_weight(FontWeight::MEDIUM).text_color(tone).child(shown)))
                            .child(
                                div().id("effort-model").max_w_full().px(px(8.)).py(px(2.)).rounded_full().flex().items_center().gap(px(4.)).text_size(px(13.)).text_color(content(0.65)).cursor_pointer().hover(|s| s.text_color(content(0.95)))
                                    .on_click(cx.listener(|this, _, _, cx| this.set_model_page(ComposerMenu::Catalog, cx)))
                                    .child(div().min_w_0().truncate().child(model))
                                    .child(icon("chevron-right", 14.).text_color(content(0.55))),
                            ),
                    )
                    .child(
                        div().id("effort-reset").size(px(28.)).flex_shrink_0().rounded_full().flex().items_center().justify_center().cursor_pointer().hover(|s| s.bg(content(0.07)))
                            .when(count < 2 && !fast_ok, |d| d.opacity(0.35))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let conversation = &this.conversations[this.active_conversation()];
                                if let Some(model) = conversation.model.clone() {
                                    let key = format!("{}::{model}", conversation.provider);
                                    if let Some(all) = this.settings["modelExecution"].as_object_mut() {
                                        all.remove(&key);
                                    }
                                    this.backend.invoke("save_settings", json!({ "settings": this.settings }));
                                    let target = this.current_execution().share();
                                    this.effort_bar.set(target, Instant::now(), if cx.reduce_motion() { 0. } else { 0.2 }, Curve::Expo);
                                }
                                cx.notify();
                            }))
                            .child(icon("undo", 14.).text_color(content(0.5))),
                    ),
            )
            .child(
                div().id("effort-slider").relative().h(px(32.)).w_full()
                    .when(count > 1, |d| d.cursor_pointer().on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.effort_drag = true;
                        this.pick_effort_at(f32::from(event.position.x), cx);
                    })))
                    .child(
                        div().absolute().inset_0().rounded_full().border_1().border_color(content(0.115)).bg(content(0.09)).overflow_hidden()
                            .child(gpui::canvas(move |bounds, _, _| store.borrow_mut()[2] = Some(bounds), move |bounds, _, window, _| {
                                effort_energy::paint(bounds, &energy, window);
                                // The menu occludes the window's own listeners, so a drag listens here.
                                let moved = shell.clone();
                                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                                    if phase == gpui::DispatchPhase::Bubble {
                                        let _ = moved.update(cx, |this, cx| {
                                            if this.effort_drag {
                                                if event.pressed_button == Some(MouseButton::Left) {
                                                    this.pick_effort_at(f32::from(event.position.x), cx);
                                                } else {
                                                    this.effort_drag = false;
                                                }
                                            }
                                        });
                                    }
                                });
                                let released = shell.clone();
                                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                                    if phase == gpui::DispatchPhase::Bubble {
                                        let _ = released.update(cx, |this, _| this.effort_drag = false);
                                    }
                                });
                            }).absolute().size_full())
                            .when(count > 1, |d| d.child(div().absolute().top_0().bottom_0().left(px(14.)).right(px(14.)).flex().items_center().justify_between().children((0..count).map(|_| div().size(px(4.)).rounded_full().bg(gpui::Hsla { a: 0.4, ..gpui::white() }))))),
                    )
                    .when(count > 0, |d| d.child(
                        div().absolute().top(px(2.)).left(px(if count == 1 { 256. - 30. } else { 2. + share * (256. - 32.) })).size(px(28.)).rounded_full().border_1().border_color(gpui::Hsla { a: 0.7, ..gpui::white() }).shadow_sm()
                            .bg(linear_gradient(135., linear_color_stop(gpui::white(), 0.), linear_color_stop(thumb_tint, 1.))),
                    )),
            )
            .child(match (first, last, count) {
                (_, _, 0) => div().mt(px(8.)).text_size(px(12.)).text_color(content(0.45)).child(t("Esforço gerenciado pelo provedor; esta CLI/modelo não expõe ajuste manual.")),
                (_, Some(last), 1) => div().mt(px(8.)).flex().justify_between().text_size(px(11.5)).text_color(content(0.45)).child(t("Fixo")).child(last),
                (first, last, _) => div().mt(px(8.)).flex().justify_between().text_size(px(11.5)).text_color(content(0.45)).children(first).children(last),
            })
            .into_any_element()
    }

    /// Sirus Code's catalog page: 288 wide, provider tabs with a gliding mark, search and rows.
    fn catalog_page(&self, now: Instant, moving: &mut bool, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.enabled_providers();
        let tab_count = tabs.len() + 1;
        let rail = 288. - 2. - 12. - 2. - 6.;
        let tab_width = (rail - 2. * (tab_count as f32 - 1.)) / tab_count as f32;
        let glide = self.model_glide.at(now);
        if !self.model_glide.done(now) {
            *moving = true;
        }
        let list_since = now
            .saturating_duration_since(self.model_list_at)
            .as_secs_f32();
        let list_t = eased(list_since / 0.14, Curve::Expo);
        if list_since < 0.14 {
            *moving = true;
        }
        let scope = self.model_scope.clone();
        let favorites_scope = scope == "favorites";
        let rows = self.model_rows();
        let loading = !favorites_scope && !self.models.contains_key(scope.as_str());
        let selected_row = self.model_selected;
        let conversation = &self.conversations[self.active_conversation()];
        let has_model = conversation.model.is_some();
        let tab = |id: usize,
                   active: bool,
                   content_element: AnyElement,
                   target: String,
                   cx: &mut Context<Self>| {
            let (hover_target, click_target) = (target.clone(), target);
            div()
                .id(("model-tab", id))
                .w(px(tab_width))
                .h(px(24.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(!active, |d| d.opacity(0.75))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.browse_models(&hover_target, cx);
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| this.browse_models(&click_target, cx)))
                .child(content_element)
        };
        let mut rail_tabs: Vec<AnyElement> = vec![
            tab(
                0,
                favorites_scope,
                theme_icon(if favorites_scope { "star-fill" } else { "star" }, 14.)
                    .text_color(gpui::rgb(0xe9b44c))
                    .into_any_element(),
                "favorites".to_owned(),
                cx,
            )
            .into_any_element(),
        ];
        for (index, provider) in tabs.iter().enumerate() {
            let active = scope == *provider;
            let mark = provider_mark(provider, if active { 16. } else { 15. }).into_any_element();
            rail_tabs
                .push(tab(index + 1, active, mark, (*provider).to_owned(), cx).into_any_element());
        }
        let models = crate::hover_track::list("model-list");
        let list: Vec<AnyElement> =
            rows.iter()
                .enumerate()
                .map(|(index, row)| {
                    let (provider, id, star_id, favorite) =
                        (row.provider, row.id.clone(), row.id.clone(), row.favorite);
                    models
                        .row(
                            div()
                                .id(("model-row", index))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .rounded(px(crate::theme::RADIUS_MD))
                                .when(row.selected || selected_row == Some(index), |d| {
                                    d.bg(content(0.09))
                                })
                                .child(
                                    div()
                                        .id(("model-pick", index))
                                        .flex_1()
                                        .min_w_0()
                                        .min_h(px(28.))
                                        .px(px(8.))
                                        .py(px(4.))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.pick_model(provider, id.clone(), cx)
                                        }))
                                        .child(provider_mark(provider, 16.))
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
                                                        .text_color(content(0.95))
                                                        .child(row.name.clone()),
                                                )
                                                .when(favorites_scope, |d| {
                                                    d.child(
                                                        div()
                                                            .truncate()
                                                            .text_size(px(12.))
                                                            .text_color(content(0.45))
                                                            .child(provider_title(provider)),
                                                    )
                                                }),
                                        )
                                        .when(row.selected, |d| {
                                            d.child(icon("check", 13.).text_color(content(0.6)))
                                        }),
                                )
                                .child(
                                    div()
                                        .id(("model-star", index))
                                        .size(px(24.))
                                        .mr(px(4.))
                                        .flex_shrink_0()
                                        .rounded(px(crate::theme::RADIUS_SM))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(content(0.07)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_favorite_model(provider, &star_id, cx)
                                        }))
                                        .child(
                                            theme_icon(
                                                if favorite { "star-fill" } else { "star" },
                                                14.,
                                            )
                                            .text_color(if favorite {
                                                content(0.95)
                                            } else {
                                                content(0.4)
                                            }),
                                        ),
                                ),
                            index,
                            row.selected || selected_row == Some(index),
                        )
                        .into_any_element()
                })
                .collect();
        let empty = if loading {
            t("Carregando catálogo de modelos…")
        } else if !self.model_query.trim().is_empty() {
            t("Nenhum modelo corresponde à busca.")
        } else if favorites_scope {
            t("Nenhum favorito disponível. Marque um modelo com a estrela.")
        } else {
            t("Nenhum modelo habilitado disponível neste provedor.")
        };
        let caption = if favorites_scope {
            t("Favoritos").to_owned()
        } else {
            provider_title(&scope).to_owned()
        };
        div()
            .flex()
            .flex_col()
            .when(has_model, |d| {
                d.child(
                    div()
                        .id("model-back")
                        .w_full()
                        .px(px(12.))
                        .py(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .border_b_1()
                        .border_color(content(0.08))
                        .text_size(px(13.))
                        .text_color(content(0.65))
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.07)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_model_page(ComposerMenu::Effort, cx)
                        }))
                        .child(icon("chevron-left", 14.).text_color(content(0.65)))
                        .child(t("Esforço e velocidade")),
                )
            })
            .child(
                div()
                    .h(px(320.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .relative()
                            .mx(px(6.))
                            .mt(px(6.))
                            .mb(px(4.))
                            .p(px(3.))
                            .flex()
                            .gap(px(2.))
                            .rounded(px(crate::theme::RADIUS_LG))
                            .border_1()
                            .border_color(content(0.08))
                            .bg(content(0.03))
                            .child(
                                div()
                                    .absolute()
                                    .top(px(3.))
                                    .bottom(px(3.))
                                    .left(px(3. + glide * (tab_width + 2.)))
                                    .w(px(tab_width))
                                    .rounded(px(crate::theme::RADIUS_MD))
                                    .bg(content(0.09)),
                            )
                            .children(rail_tabs),
                    )
                    .child(
                        div()
                            .px(px(12.))
                            .text_size(px(11.5))
                            .text_color(content(0.45))
                            .child(caption),
                    )
                    .child(
                        div()
                            .mx(px(6.))
                            .mb(px(2.))
                            .mt(px(6.))
                            .h(px(28.))
                            .flex_shrink_0()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .border_1()
                            .border_color(content(0.3))
                            .bg(field_fill())
                            .text_size(px(13.))
                            .child(icon("search", 13.).text_color(content(0.45)))
                            .child(typed_text(
                                &self.model_query,
                                t("Buscar modelos..."),
                                div()
                                    .flex_shrink_0()
                                    .w(px(1.5))
                                    .h(px(15.))
                                    .rounded_full()
                                    .bg(crate::theme::caret_color()),
                            ))
                            .when(!self.model_query.is_empty(), |d| {
                                d.child(
                                    div()
                                        .id("model-search-clear")
                                        .size(px(20.))
                                        .ml_auto()
                                        .rounded(px(crate::theme::RADIUS_SM))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(content(0.07)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.model_query.clear();
                                            cx.notify();
                                        }))
                                        .child(icon("close", 12.).text_color(content(0.5))),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id("model-list")
                            .relative()
                            .left(px((1. - list_t) * 4.))
                            .opacity(list_t)
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p(px(4.))
                            .flex()
                            .flex_col()
                            .child(models.pill())
                            .when(list.is_empty(), |d| {
                                d.child(
                                    div()
                                        .px(px(8.))
                                        .py(px(16.))
                                        .text_size(px(13.))
                                        .text_color(content(0.5))
                                        .child(empty),
                                )
                            })
                            .children(list),
                    ),
            )
            .into_any_element()
    }

    /// The access and model menus, rising from the composer's shelf.
    fn composer_menu_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (which, open) = self.composer_menu.as_ref()?;
        let which = *which;
        if !open.done(now) {
            *moving = true;
        }
        let t = open.at(now).clamp(0., 1.);
        let reduce = cx.reduce_motion();
        let (body, width): (Vec<AnyElement>, f32) = match which {
            ComposerMenu::Access => (self.access_rows(cx), 320.),
            ComposerMenu::Effort => (vec![self.effort_page(now, moving, reduce, cx)], 280.),
            ComposerMenu::Catalog => (vec![self.catalog_page(now, moving, cx)], 288.),
        };
        let (bottom, left, width) = match (which, self.model_menu_anchor) {
            (ComposerMenu::Access, _) | (_, None) => {
                let (left, full, bottom) = self.above_composer();
                (
                    bottom,
                    left,
                    if which == ComposerMenu::Access {
                        full
                    } else {
                        width
                    },
                )
            }
            (_, Some((center, bottom))) => (
                bottom,
                (center - width / 2.).clamp(10., (self.viewport_width - width - 10.).max(10.)),
                width,
            ),
        };
        Some(
            div()
                .id("composer-menu")
                .occlude()
                .absolute()
                .bottom(px(bottom + (1. - t) * -6.))
                .left(px(left))
                .w(px(width))
                .when(which == ComposerMenu::Access, |d| d.p(px(4.)))
                .opacity(t)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .when(which == ComposerMenu::Access, |d| {
                    d.child(crate::hover_track::list("access-menu").pill())
                })
                .children(body)
                .into_any_element(),
        )
    }

    /// Where the composer's column starts and ends, for its menus.
    fn composer_column(&self) -> (f32, f32) {
        let now = Instant::now();
        let left = self.sidebar_width * self.sidebar.at(now);
        let width = self.viewport_width - left - 12. - self.dock_drawn;
        let column = width.min(760.);
        let start = left + 6. + (width - column) / 2. + 24.;
        (start, self.viewport_width - start - column + 48.)
    }

    /// A popup spanning the composer card, sitting just above it: (left, width, bottom).
    fn above_composer(&self) -> (f32, f32, f32) {
        match self.composer_bounds.get() {
            Some(bounds) => (
                f32::from(bounds.left()) + 24.,
                f32::from(bounds.size.width) - 48.,
                self.viewport_height - f32::from(bounds.top()) + 6.,
            ),
            None => (self.composer_menu_left(), 420., FOOTER + 100.),
        }
    }

    fn composer_menu_left(&self) -> f32 {
        self.composer_column().0
    }

    fn composer_menu_right(&self) -> f32 {
        self.composer_column().1
    }

    /// While the agent works, the project's robot walks along the composer's top edge over a line
    /// of letters in its colour. Reduce Motion leaves it out.
    fn composer_runner(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let conversation = &self.conversations[ix];
        if !conversation.working() || cx.reduce_motion() {
            return None;
        }
        let project = self.projects.get(self.project_of(ix)?)?;
        let (_, color) = crate::project_avatar::look_of(&project.name, &project.appearance);
        let clock = Instant::now().duration_since(self.epoch).as_secs_f32();
        let (x, hop) = crate::new_tab_field::walk(self.runner_width.get(), clock);
        Some(
            div()
                .absolute()
                .top(px(-34.))
                .left_0()
                .right_0()
                .h(px(34.))
                .child(crate::new_tab_field::sparks(
                    color,
                    clock,
                    self.runner_width.clone(),
                ))
                .child(div().absolute().left(px(x)).top(px(8. - hop)).child(
                    crate::project_avatar::project_avatar_state(
                        &project.name,
                        &project.appearance,
                        26.,
                        crate::project_avatar::AvatarState::Idle,
                        clock,
                    ),
                ))
                .into_any_element(),
        )
    }

    fn composer(&self, live: bool, cx: &mut Context<Self>) -> AnyElement {
        let ix = self.active_conversation();
        let conversation = &self.conversations[ix];
        let provider = conversation.provider;
        let model = self.model_display(provider, conversation.model.as_deref());
        let access = match conversation.approval.as_str() {
            "ask" => t("Pedir aprovação"),
            "auto" => t("Revisão automática"),
            _ => t("Acesso total"),
        };
        let (access_glyph, _, _, access_color) = access_look(&conversation.approval);
        let chosen = self.execution(provider, conversation.model.as_deref());
        let effort = chosen.effort.as_deref().map(effort_label);
        let trigger_store = self.model_trigger.clone();
        let mention = self.mention();
        let mention_selected = self.mention_selected;
        let dictating = self.dictating.is_some();
        let goal_text = conversation.goal.clone().unwrap_or_default();
        let mode_chips: [(&str, bool, &'static str); 4] = [
            (t("Planejamento"), conversation.planning, "bulb"),
            ("Debug", conversation.debugging, "bug"),
            (t("Equipe"), conversation.team, "users"),
            (t("Meta"), conversation.goal.is_some(), "goal"),
        ];
        let attached = self.attachments.get(&ix).cloned().unwrap_or_default();
        let attachment_chips = self.attachment_chips(&attached, cx);
        let secret = self.secrets.first().cloned();
        let secret_entry = self.secret_entry.clone();
        let draft = self.drafts.get(&ix).cloned().unwrap_or_default();
        let has_text = !draft.text.trim().is_empty() || !attached.is_empty();
        // While the agent works Stop and Send sit side by side, and what is sent joins the
        // queue. With only a queue waiting and nothing written, none.
        let approvals_pending = conversation
            .pending
            .iter()
            .any(|r| r["kind"]["type"] != "userInput");
        let queueing = live || self.queues.waiting(&conversation.session_id);
        let queue_card = self.queue_card(ix, cx);
        let typing = self.palette_ui.is_none()
            && self.appearance.is_none()
            && !self.usage_page_open()
            && !self.grabs_typing();
        let styled = styled_tokens(if draft.text.is_empty() {
            " ".to_owned()
        } else {
            draft.text.clone()
        });
        let layout = styled.layout().clone();
        *self.composer_layout.borrow_mut() = Some(layout.clone());
        let selection = draft.selection();
        let caret_at = draft.cursor;
        let caret_key = (draft.cursor, draft.text.len());
        let composer_scroll = self.composer_scroll.clone();
        let caret_seen = self.composer_caret_seen.clone();
        // The caret and the selection are painted over the laid out text, so the text never moves.
        let overlay = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let line = layout.line_height();
                // Only a caret that moved pulls the text along, so the wheel stays free.
                if caret_seen.replace(caret_key) != caret_key
                    && let Some(at) = layout.position_for_index(caret_at)
                {
                    let top = at.y - bounds.origin.y;
                    let seen = composer_scroll.bounds().size.height;
                    let scrolled = -composer_scroll.offset().y;
                    let target = follow_caret(top, line, scrolled, seen);
                    if target != scrolled {
                        composer_scroll.set_offset(gpui::point(px(0.), -target));
                        window.on_next_frame(|window, _| window.refresh());
                    }
                }
                let fill = crate::theme::text_selection();
                if !selection.is_empty() {
                    let mut at = selection.start;
                    while at < selection.end {
                        let Some(start) = layout.position_for_index(at) else {
                            break;
                        };
                        let mut stop = at;
                        let mut end = start;
                        for next in (at + 1)..=selection.end {
                            if let Some(p) = layout.position_for_index(next) {
                                if p.y != start.y {
                                    break;
                                }
                                stop = next;
                                end = p;
                            }
                        }
                        if stop == at {
                            at += 1;
                            continue;
                        }
                        window.paint_quad(gpui::fill(
                            Bounds::new(start, gpui::size(end.x - start.x, line)),
                            fill,
                        ));
                        at = stop;
                    }
                } else if typing {
                    if let Some(origin) =
                        layout.position_for_index(caret_at).or(Some(bounds.origin))
                    {
                        window.paint_quad(
                            gpui::fill(
                                Bounds::new(
                                    origin,
                                    gpui::size(px(crate::theme::CARET_WIDTH), line),
                                ),
                                crate::theme::caret_color(),
                            )
                            .corner_radii(px(1.)),
                        );
                    }
                }
            },
        )
        // Pinned to the corner: with no offsets it would sit after the text, and the caret's
        // place measured from it would never grow.
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let text = div()
            .relative()
            .w_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let index = this
                        .composer_layout
                        .borrow()
                        .as_ref()
                        .map(|l| l.index_for_position(event.position).unwrap_or_else(|i| i));
                    if let Some(index) = index {
                        let draft = this.draft();
                        let index = index.min(draft.text.len());
                        draft.cursor = index;
                        if !event.modifiers.shift {
                            draft.anchor = index;
                        }
                        this.selecting = true;
                        cx.notify();
                    }
                }),
            )
            .child(styled)
            .child(overlay)
            .when(draft.text.is_empty(), |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .text_color(content(0.4))
                        .child(if approvals_pending {
                            t("Ou diga ao agente o que fazer em vez disso (recusa a aprovação)…")
                        } else {
                            t("Peça qualquer coisa")
                        }),
                )
            })
            .into_any_element();
        let pending = conversation.pending.first().cloned();
        let round = |id: &'static str, name: &'static str| {
            div()
                .id(id)
                .size(px(30.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)))
                .child(icon(name, 15.).text_color(content(0.6)))
        };
        let shelf_button = |id: &'static str| {
            div()
                .id(id)
                .h(px(24.))
                .px(px(8.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(content(0.6))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)).text_color(content(0.95)))
        };
        let ring = self.context_ring(cx);
        let compact_send = self.compact_send_pending();
        let composer_store = self.composer_bounds.clone();
        div()
            .relative()
            .flex_shrink_0()
            .mx_auto()
            .w_full()
            .max_w(px(832.))
            .px(px(23.))
            .pb(px(14.))
            .child(
                gpui::canvas(
                    move |bounds, _, _| composer_store.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .left_0()
                .top_0()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(-24.))
                    .h(px(24.))
                    .bg(linear_gradient(
                        0.,
                        linear_color_stop(pane_fill(), 0.),
                        linear_color_stop(transparent(), 1.),
                    )),
            )
            .when_some(mention, |d, mention| {
                let selected = mention_selected.min(mention.hits.len() - 1);
                d.child(
                    div()
                        .id("mention-menu")
                        .occlude()
                        .absolute()
                        .left(px(24.))
                        .right(px(24.))
                        .bottom(gpui::relative(1.))
                        .mb(px(8.))
                        .p(px(4.))
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(content(0.12))
                        .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(crate::hover_track::list("mention-menu").pill())
                        .children(mention.hits.into_iter().enumerate().map(
                            |(i, (name, about))| {
                                let chosen = name.clone();
                                crate::hover_track::list("mention-menu").row(
                                    div()
                                        .id(("mention", i))
                                        .h(px(30.))
                                        .px(px(8.))
                                        .rounded(px(crate::theme::RADIUS_MD))
                                        .flex()
                                        .items_center()
                                        .gap(px(10.))
                                        .text_size(px(13.))
                                        .text_color(content(0.92))
                                        .cursor_pointer()
                                        .when(i == selected, |d| d.bg(content(0.1)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.accept_mention(&chosen, cx)
                                        }))
                                        .when(about.is_empty(), |d| {
                                            d.child(crate::file_icons::file_icon(
                                                name.rsplit('/').next().unwrap_or(&name),
                                                false,
                                                15.,
                                            ))
                                        })
                                        .child(div().flex_shrink_0().child(name))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(px(11.5))
                                                .text_color(content(0.45))
                                                .child(about),
                                        ),
                                    i,
                                    i == selected,
                                )
                            },
                        )),
                )
            })
            // Sign-in approvals and computer-use requests (browser_bridge.rs).
            .when_some(
                crate::browser_bridge::cards(&conversation.session_id),
                |d, cards| d.child(cards),
            )
            // A request for a secret: the value is typed here, masked, and the agent never sees it.
            .when_some(secret, |d, request| {
                let id = request["id"].as_str().unwrap_or("").to_owned();
                let label = request["envName"]
                    .as_str()
                    .or(request["label"].as_str())
                    .unwrap_or("")
                    .to_owned();
                let about = request["description"].as_str().unwrap_or("").to_owned();
                let typing_secret = secret_entry.as_ref().is_some_and(|(entry, _)| *entry == id);
                let masked = secret_entry
                    .as_ref()
                    .filter(|_| typing_secret)
                    .map_or(String::new(), |(_, value)| {
                        "•".repeat(value.chars().count())
                    });
                let (open_id, decline_id) = (id.clone(), id.clone());
                d.child(
                    div()
                        .mb(px(8.))
                        .h(px(40.))
                        .px(px(10.))
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(accent())
                        .bg(field_fill())
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(13.))
                        .child(icon("key", 15.).text_color(accent()))
                        .child(div().flex_shrink_0().text_color(content(0.95)).child(label))
                        .child(if typing_secret {
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(content(0.9))
                                .child(if masked.is_empty() {
                                    t("Digite o valor e aperte Enter").to_owned()
                                } else {
                                    masked
                                })
                        } else {
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(content(0.5))
                                .child(about)
                        })
                        .when(!typing_secret, |d| {
                            d.child(
                                div()
                                    .id("secret-decline")
                                    .h(px(26.))
                                    .px(px(10.))
                                    .rounded(px(crate::theme::RADIUS_MD))
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.5))
                                    .text_color(content(0.75))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(content(0.07)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.respond_secret(&decline_id, None, cx)
                                    }))
                                    .child(t("Recusar")),
                            )
                            .child(
                                div()
                                    .id("secret-open")
                                    .h(px(26.))
                                    .px(px(10.))
                                    .rounded(px(crate::theme::RADIUS_MD))
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.5))
                                    .text_color(on_accent())
                                    .bg(accent())
                                    .cursor_pointer()
                                    .hover(|s| s.opacity(0.88))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.secret_entry = Some((open_id.clone(), String::new()));
                                        cx.notify();
                                    }))
                                    .child(t("Informar")),
                            )
                        }),
                )
            })
            .when_some(pending, |d, request| {
                let kind = &request["kind"];
                let question = kind["type"] == "userInput";
                let asked = &kind["questions"][0];
                let question_id = asked["id"].as_str().unwrap_or("").to_owned();
                let options: Vec<String> = asked["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|o| o["label"].as_str().map(str::to_owned))
                    .collect();
                let what = match kind["type"].as_str() {
                    Some("command") => {
                        tf("Executar: {}", &[&kind["command"].as_str().unwrap_or("")])
                    }
                    Some("fileChange") => t("Alterar arquivos").to_owned(),
                    Some("tool") => tf(
                        "Usar a ferramenta {}",
                        &[&kind["name"].as_str().unwrap_or("")],
                    ),
                    _ => asked["question"]
                        .as_str()
                        .unwrap_or(t("O agente fez uma pergunta."))
                        .to_owned(),
                };
                let (accept, decline, asked_request) =
                    (request.clone(), request.clone(), request.clone());
                let button = |id: &'static str, label: &'static str, main: bool| {
                    div()
                        .id(id)
                        .h(px(26.))
                        .px(px(10.))
                        .rounded(px(crate::theme::RADIUS_MD))
                        .flex()
                        .items_center()
                        .text_size(px(12.5))
                        .cursor_pointer()
                        .text_color(if main { on_accent() } else { content(0.85) })
                        .bg(if main { accent() } else { content(0.08) })
                        .hover(|s| s.opacity(0.88))
                };
                d.child(
                    div()
                        .mb(px(8.))
                        .p(px(10.))
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(accent_wash(0.4))
                        .bg(field_fill())
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(13.))
                                .text_color(content(0.9))
                                .child(what),
                        )
                        .when(question, |d| {
                            d.children(options.into_iter().enumerate().map(|(i, label)| {
                                let (request, question_id, answer) =
                                    (asked_request.clone(), question_id.clone(), label.clone());
                                div()
                                    .id(("answer", i))
                                    .h(px(26.))
                                    .px(px(10.))
                                    .rounded(px(crate::theme::RADIUS_MD))
                                    .flex()
                                    .items_center()
                                    .text_size(px(12.5))
                                    .cursor_pointer()
                                    .text_color(content(0.9))
                                    .bg(content(0.08))
                                    .hover(|s| s.bg(content(0.14)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.answer(ix, &request, &question_id, &answer);
                                        cx.notify();
                                    }))
                                    .child(label)
                            }))
                        })
                        .when(!question, |d| {
                            d.child(button("deny", t("Recusar"), false).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.respond(ix, &decline, "decline");
                                    cx.notify();
                                },
                            )))
                            .child(
                                button("allow", t("Aprovar"), true).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.respond(ix, &accept, "accept");
                                        cx.notify();
                                    },
                                )),
                            )
                        }),
                )
            })
            .when_some(self.account_card(ix, cx), |d, card| d.child(card))
            .when_some(queue_card, |d, card| d.child(card))
            .when_some(self.script_notice(ix, cx), |d, card| d.child(card))
            .when_some(self.handoff_card(ix, cx), |d, card| d.child(card))
            .child(
                div()
                    .relative()
                    .rounded(px(crate::theme::RADIUS_LG))
                    .border_1()
                    .border_color(content(0.1))
                    .children(self.composer_runner(ix, cx))
                    .child(crate::theme::composer_surface(crate::theme::RADIUS_LG - 1.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .rounded(px(crate::theme::RADIUS_LG))
                            .border_b_1()
                            .border_color(content(0.07))
                            .pt(px(14.))
                            .px(px(8.))
                            .pb(px(8.))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .when(mode_chips.iter().any(|m| m.1), |d| {
                                d.child(div().px(px(8.)).flex().flex_wrap().gap(px(5.)).children(
                                    mode_chips.iter().enumerate().filter(|(_, m)| m.1).map(
                                        |(at, (label, _, glyph))| {
                                            div()
                                                .id(("mode-chip", at))
                                                .h(px(22.))
                                                .pl(px(7.))
                                                .pr(px(3.))
                                                .rounded(px(crate::theme::RADIUS_SM))
                                                .flex()
                                                .items_center()
                                                .gap(px(5.))
                                                .text_size(px(12.))
                                                .text_color(accent())
                                                .bg(accent_wash(0.13))
                                                .child(icon(glyph, 12.).text_color(accent()))
                                                .child(div().max_w(px(176.)).truncate().child(
                                                    if at == 3 {
                                                        goal_text.clone()
                                                    } else {
                                                        (*label).to_owned()
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .id(("mode-chip-off", at))
                                                        .size(px(16.))
                                                        .rounded(px(crate::theme::RADIUS_SM))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .cursor_pointer()
                                                        .hover(|s| s.bg(accent_wash(0.2)))
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                let ix = this.active_conversation();
                                                                match at {
                                                                    0 => {
                                                                        this.conversations[ix]
                                                                            .planning = false
                                                                    }
                                                                    1 => {
                                                                        this.conversations[ix]
                                                                            .debugging = false
                                                                    }
                                                                    2 => {
                                                                        this.conversations[ix]
                                                                            .team = false
                                                                    }
                                                                    _ => {
                                                                        this.conversations[ix]
                                                                            .goal = None
                                                                    }
                                                                }
                                                                cx.notify();
                                                            },
                                                        ))
                                                        .child(
                                                            icon("close", 10.).text_color(accent()),
                                                        ),
                                                )
                                        },
                                    ),
                                ))
                            })
                            .children(attachment_chips)
                            .child(
                                div()
                                    .id("composer-text")
                                    .px(px(8.))
                                    .max_h(px(190.))
                                    .overflow_y_scroll()
                                    .track_scroll(&self.composer_scroll)
                                    .text_size(px(14.))
                                    .line_height(px(22.))
                                    .text_color(content(0.95))
                                    .child(text),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .child(
                                        round("composer-add", "plus")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, window, cx| {
                                                    this.keep_typing_focus(window, cx);
                                                    cx.stop_propagation();
                                                }),
                                            )
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.add_menu = match this.add_menu {
                                                    Some(_) => None,
                                                    None => Some((
                                                        AddPage::Add,
                                                        Tween::from_to(
                                                            0.,
                                                            1.,
                                                            Instant::now(),
                                                            if cx.reduce_motion() {
                                                                0.
                                                            } else {
                                                                MENU
                                                            },
                                                            Curve::Expo,
                                                        ),
                                                    )),
                                                };
                                                cx.notify();
                                            })),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        round("composer-mic", "mic")
                                            .when(dictating, |d| d.bg(rgb(0xe0654d)))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.toggle_dictation(cx)
                                            })),
                                    )
                                    // A heavy conversation idle past its cache compacts before
                                    // this message; the plain arrow sends without (context_meter.rs).
                                    .when(compact_send && has_text && !live, |d| {
                                        d.child(
                                            div()
                                                .id("send-plain")
                                                .ml(px(2.))
                                                .size(px(26.))
                                                .rounded(px(crate::theme::RADIUS_MD))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .bg(content(0.09))
                                                .cursor_pointer()
                                                .hover(|s| s.bg(content(0.16)))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.context.skip_compact = true;
                                                    this.send(cx);
                                                    this.context.skip_compact = false;
                                                }))
                                                .child(
                                                    icon("arrow-up", 15.).text_color(content(0.7)),
                                                ),
                                        )
                                    })
                                    .when(live || !queueing || has_text, |d| {
                                        // One button: Stop while the agent works with nothing typed.
                                        let stopping = live && !has_text;
                                        let expanded = has_text;
                                        let face = crate::send_morph::Face {
                                            stop: stopping,
                                            label: if compact_send && !live {
                                                t("Compactar e enviar")
                                            } else if live {
                                                t("Na fila")
                                            } else {
                                                t("Enviar")
                                            },
                                            glyph: if compact_send && has_text && !live {
                                                "chevrons-down-up"
                                            } else {
                                                "chevron-up"
                                            },
                                            expanded,
                                        };
                                        let frame = self.send_morph.borrow_mut().frame(
                                            face,
                                            crate::send_morph::expanded_width(face.label),
                                            !cx.reduce_motion(),
                                        );
                                        if frame.moving {
                                            self.send_moving.set(true);
                                        }
                                        d.child(
                                            crate::send_morph::button(
                                                "send",
                                                face,
                                                &frame,
                                                has_text || stopping,
                                            )
                                            .ml(px(2.))
                                            .on_click(
                                                cx.listener(
                                                    move |this, event: &gpui::ClickEvent, _, cx| {
                                                        if stopping {
                                                            this.stop(cx);
                                                        } else if has_text {
                                                            // Shift-click sends without compacting.
                                                            this.context.skip_compact =
                                                                event.modifiers().shift;
                                                            this.send(cx);
                                                            this.context.skip_compact = false;
                                                        }
                                                    },
                                                ),
                                            ),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .h(px(34.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            // Empty space in the toolbar keeps typing in the composer.
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.keep_typing_focus(window, cx)
                                }),
                            )
                            .child(
                                div()
                                    .id("composer-access")
                                    .h(px(24.))
                                    .px(px(6.))
                                    .rounded(px(crate::theme::RADIUS_SM))
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .text_size(px(12.))
                                    .text_color(access_color)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(content(0.07)))
                                    .child(icon(access_glyph, 14.).text_color(access_color))
                                    .child(access)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.keep_typing_focus(window, cx);
                                            cx.stop_propagation();
                                        }),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_composer_menu(ComposerMenu::Access, cx)
                                    })),
                            )
                            .child(div().flex_1())
                            .children(ring)
                            .child(
                                shelf_button("composer-model")
                                    .relative()
                                    .max_w(px(300.))
                                    .child(
                                        gpui::canvas(
                                            move |bounds, _, _| trigger_store.set(Some(bounds)),
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .size_full(),
                                    )
                                    .child(provider_mark(provider, 16.))
                                    .child(div().min_w_0().truncate().child(model))
                                    .children(effort.map(|e| {
                                        div().flex_shrink_0().text_color(content(0.4)).child(e)
                                    }))
                                    .when(chosen.fast, |d| {
                                        d.child(
                                            theme_icon("zap-fill", 12.)
                                                .text_color(gpui::rgb(0xf0c43a)),
                                        )
                                    })
                                    .child(chevron_down())
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.keep_typing_focus(window, cx);
                                            cx.stop_propagation();
                                        }),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_composer_menu(ComposerMenu::Effort, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The bar under the window, like the title bar: the model's usage and the actions.
    /// Sample numbers; they come from the provider in part 2.
    fn footer(&self, left: f32, cx: &mut Context<Self>) -> AnyElement {
        let provider = self.conversations[self.active_conversation()].provider;
        div()
            .relative()
            .h(px(FOOTER))
            .flex_shrink_0()
            .pl(px(left + 8.))
            .pr(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(11.))
            .text_color(content(0.55))
            .child(
                div()
                    .id("usage")
                    .relative()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_usage_popup(cx)))
                    .child(
                        gpui::canvas(
                            {
                                let store = self.usage_chip.clone();
                                move |bounds, _, _| store.set(Some(bounds))
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .h(px(20.))
                    .px(px(6.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.1)).text_color(content(1.)))
                    .child(mark_tile(
                        &crate::tab_tile::TileSize {
                            w: 14.,
                            h: 12.,
                            radius: 3.,
                        },
                        Face::provider(provider),
                        provider_glyph(provider, 9.),
                    ))
                    .when_some(self.usage.clone(), |d, (_, tightest)| {
                        d.child(
                            div()
                                .w(px(32.))
                                .h(px(4.))
                                .rounded_full()
                                .bg(content(0.1))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .w(px(32. * tightest.clamp(0., 100.) / 100.))
                                        .rounded_full()
                                        .bg(usage_bar_color(f64::from(tightest))),
                                ),
                        )
                    })
                    .child(self.usage.as_ref().map_or_else(
                        || {
                            model_label(
                                self.conversations[self.active_conversation()]
                                    .model
                                    .as_deref(),
                                provider,
                            )
                        },
                        |(text, _)| text.clone(),
                    )),
            )
            // Refresh sits right beside the numbers, as in Monocode.
            .child(
                div()
                    .id("refresh-usage")
                    .size(px(20.))
                    .ml(px(-4.))
                    .rounded_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .opacity(if self.usage_refreshing { 0.5 } else { 1. })
                    .when(!self.usage_refreshing, |d| {
                        d.hover(|s| s.bg(content(0.1)))
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.usage_refreshing {
                            this.request_usage(true);
                            cx.notify();
                        }
                    }))
                    .child(icon("refresh", 12.)),
            )
            .when_some(self.computer_chip(cx), |d, chip| d.child(chip))
            .when_some(self.problem.clone(), |d, problem| {
                d.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(0xe08a8a))
                        .child(problem),
                )
            })
            .child(div().flex_1())
            .when_some(self.update_notice(cx), |d, notice| d.child(notice))
            .when_some(self.resources_gauge(cx), |d, gauge| d.child(gauge))
            .child(self.usage_button(cx))
            .child(self.settings_button(cx))
            .into_any_element()
    }

    /// The part of a streaming reply to draw now (paced.rs); a reply that is not streaming, and
    /// one that was already whole when it came, is drawn whole.
    fn paced_answer<'a>(
        &self,
        conversation: usize,
        turn_ix: usize,
        turn: &'a data::Turn,
        moving: &mut bool,
    ) -> &'a str {
        let key = (conversation, turn_ix);
        let mut pacers = self.pacers.borrow_mut();
        if !turn.live && !pacers.contains_key(&key) {
            return &turn.answer;
        }
        let now = Instant::now();
        let pacer = pacers.entry(key).or_insert_with(|| paced::Pacer::new(now));
        let (text, more) = pacer.view(&turn.answer, now, turn.live);
        if more || turn.live {
            *moving = true;
        } else {
            pacers.remove(&key);
        }
        text
    }

    /// A group of steps: its tile and title, and the steps on a rail when open. The running group
    /// is open by itself, inside a window that follows the newest step (Monocode's live phase);
    /// a settled one opens with a click.
    #[allow(clippy::too_many_arguments)]
    fn phase_view(
        &self,
        conversation: usize,
        index: usize,
        turn: &Turn,
        range: std::ops::Range<usize>,
        active: bool,
        width: Pixels,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let start = range.start;
        let steps = &turn.steps[range];
        let key = (conversation, index, start);
        let open = self.phase_open.get(&key).copied().unwrap_or(active);
        // A group that was running closes the same way a click closes it.
        if active {
            self.phase_live.borrow_mut().insert(key);
        } else if self.phase_live.borrow_mut().remove(&key) {
            self.phase_moved.borrow_mut().insert(key, now);
        }
        let reveal_of = |curve: fn(f32) -> f32, length: f32, moving: &mut bool| {
            let Some(at) = self.phase_moved.borrow().get(&key).copied() else {
                return if open { 1. } else { 0. };
            };
            let since = now.duration_since(at).as_secs_f32();
            if since < length {
                *moving = true;
            }
            let eased = curve(since / length);
            if open { eased } else { 1. - eased }
        };
        let height = reveal_of(crate::disclosure::settle, crate::disclosure::REVEAL, moving);
        let fade = reveal_of(crate::disclosure::ease, crate::disclosure::FADE, moving);
        let turned = reveal_of(crate::disclosure::settle, crate::disclosure::TURN, moving);
        let roll_key = 1_000_000 + index * 4096 + start;
        let roll = self.roll(conversation, roll_key, now, moving);
        let lead = steps
            .iter()
            .find(|s| s.kind == "command")
            .or(steps.first())
            .map_or("terminal", step_glyph);
        let (face, mark, ink) = if active {
            (
                Face::Neutral,
                self.spin(now).into_any_element(),
                crate::tab_tile::tile_ink(),
            )
        } else {
            activity_face(lead)
        };
        let morph = if active { roll } else { roll.max(turned) };
        let chevron = icon("chevron-right", 14.)
            .text_color(ink)
            .with_transformation(Transformation::rotate(percentage(turned * 0.25)))
            .into_any_element();
        let clock = now.duration_since(self.epoch).as_secs_f32();
        let title: AnyElement = if active {
            let phase = live_summary(steps);
            shimmer(if phase.is_empty() { summary(steps) } else { phase }, clock)
        } else {
            div().truncate().child(summary(steps)).into_any_element()
        };
        let header = div()
            .id(("phase", index * 4096 + start))
            .h(px(STEP_ROW))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(14.))
            .text_color(content(0.5))
            .cursor_pointer()
            .hover(|s| s.text_color(content(0.8)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.set_roll(conversation, roll_key, *hovered, cx)
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.phase_open.insert(key, !open);
                this.phase_moved.borrow_mut().insert(key, Instant::now());
                cx.notify();
            }))
            .child(push_tile(
                &ACTIVITY_TILE,
                face,
                mark,
                chevron,
                morph.clamp(0., 1.),
            ))
            .child(title);
        let block = div().w_full().min_w_0().flex().flex_col().child(header);
        if height <= 0.001 {
            return block.into_any_element();
        }
        // In motion the body is measured and revealed; at rest it is simply there.
        let framed = |body: AnyElement| -> AnyElement {
            if height >= 0.999 || width <= px(0.) {
                body
            } else {
                div()
                    .w_full()
                    .overflow_hidden()
                    .opacity(fade)
                    .child(crate::disclosure::Disclosure::new(body, width, height))
                    .into_any_element()
            }
        };
        let rows = steps.iter().enumerate().map(|(i, step)| {
            let inner: AnyElement = match &step.image {
                Some(path) => self
                    .image_row(path, step, format!("image-step-{index}-{}", start + i), cx)
                    .into_any_element(),
                None if step.running => {
                    let text = if step.kind == "command" && !step.caption.is_empty() {
                        step.caption.clone()
                    } else {
                        crate::step_lines::text(&step_line(step))
                    };
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(shimmer(text, clock))
                        .into_any_element()
                }
                None => step_row(step).into_any_element(),
            };
            div()
                .relative()
                .w_full()
                .h(px(STEP_ROW))
                .overflow_hidden()
                .pl(px(TILE_RAIL_INDENT))
                .flex()
                .items_center()
                .gap(px(7.))
                .text_size(px(14.))
                .text_color(content(0.5))
                .child(rail(i + 1 == steps.len()))
                .child(inner)
                .into_any_element()
        });
        let rows: Vec<AnyElement> = rows.collect();
        if !active {
            let body = div().w_full().flex().flex_col().children(rows);
            return block.child(framed(body.into_any_element())).into_any_element();
        }
        // At most 280 px or 45% of the view; it follows the newest step while the reader has not
        // scrolled up, and the steps fade at the edges with no colour laid over the glass.
        let body_height = rows.len() as f32 * STEP_ROW;
        let view = self.view_sizes.get(&conversation).map_or(600., |size| size.1);
        let max_height = 280f32.min(view * 0.45).max(96.);
        let (handle, was_at_bottom, offset, max) = {
            let mut scrolls = self.phase_scrolls.borrow_mut();
            let entry = scrolls
                .entry((conversation, index * 4096 + start))
                .or_insert_with(|| (gpui::ScrollHandle::new(), std::cell::Cell::new(0.)));
            let offset = -f32::from(entry.0.offset().y);
            let max = f32::from(entry.0.max_offset().y);
            let was_at_bottom = offset >= entry.1.get() - 1.;
            entry.1.set(max);
            (entry.0.clone(), was_at_bottom, offset, max)
        };
        if was_at_bottom {
            handle.scroll_to_bottom();
        }
        let top_fade = if offset > 1. { 24. } else { 0. };
        let bottom_fade = if max - offset > 1. { 32. } else { 0. };
        let mask = |begin: bool| {
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    if top_fade > 0. || bottom_fade > 0. {
                        let scale = window.scale_factor();
                        window.paint_layer(bounds, |window| {
                            window.paint_quad(gpui::fill(bounds, gpui::black()).border_color(
                                gpui_apple::metal_renderer::edge_fade_marker(
                                    begin,
                                    top_fade * scale,
                                    bottom_fade * scale,
                                ),
                            ));
                        });
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };
        let window_body = 
            div()
                .relative()
                .w_full()
                .h(px(body_height.min(max_height)))
                .overflow_hidden()
                .child(mask(true))
                .child(
                    div()
                        .id(("live-steps", index * 4096 + start))
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&handle)
                        .children(rows),
                )
                .child(mask(false));
        block
            .child(framed(window_body.into_any_element()))
            .into_any_element()
    }

    /// What the person sent into the running reply, as a message of theirs, where it arrived.
    fn steer_bubble(
        &self,
        index: usize,
        steer: usize,
        turn: &Turn,
        conversation: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(steer_data) = turn.steers.get(steer) else {
            return div().into_any_element();
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_end()
            .gap(px(8.))
            .py(px(4.))
            .when(!steer_data.attachments.is_empty(), |d| {
                d.child(self.sent_chips(
                    conversation,
                    1_000 + index * 64 + steer,
                    &steer_data.attachments,
                    cx,
                ))
            })
            .when(!steer_data.text.is_empty(), |d| {
                d.child(
                    div()
                        .max_w(px(576.))
                        .min_w_0()
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(12.))
                        .bg(accent_wash(0.24))
                        .text_size(px(14.))
                        .line_height(px(20.))
                        .text_color(content(1.))
                        .child(steer_data.text.clone()),
                )
            })
            .into_any_element()
    }

    /// A stretch's header: the model's tile and how long it worked. A click opens what is folded.
    #[allow(clippy::too_many_arguments)]
    fn work_header(
        &self,
        conversation: usize,
        index: usize,
        section: usize,
        turn: &Turn,
        provider: &'static str,
        (working, last, foldable): (bool, bool, bool),
        (key, open, openness): ((usize, usize, usize), bool, f32),
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let clock = now.duration_since(self.epoch).as_secs_f32();
        let name = model_label(
            turn.model
                .as_deref()
                .or(self.conversations[conversation].model.as_deref()),
            provider,
        );
        let label: AnyElement = if working {
            let running = turn.started_ms.map(|start| {
                (chrono::Utc::now().timestamp_millis().max(0) as u64)
                    .saturating_sub(start)
                    .saturating_sub(turn.paused_ms)
                    / 1000
            });
            shimmer(
                match running {
                    Some(s) if s >= 60 => {
                        tf("{} trabalhando há {}m {}s", &[&name, &(s / 60), &(s % 60)])
                    }
                    Some(s) => tf("{} trabalhando há {}s", &[&name, &s]),
                    None => tf("{} trabalhando", &[&name]),
                },
                clock,
            )
        } else {
            let worked = match turn.seconds.filter(|_| last) {
                Some(s) if s >= 60 => {
                    tf("{} trabalhou por {}m {}s", &[&name, &(s / 60), &(s % 60)])
                }
                Some(s) => tf("{} trabalhou por {}s", &[&name, &s]),
                None => name.clone(),
            };
            div().child(worked).into_any_element()
        };
        // Pointing at the tile pushes the model's mark out to the right as the chevron slides
        // in from the left; an open fold keeps the chevron (Monocode's phase_tile.rs).
        let roll_key = 2_000_000 + index * 64 + section;
        let roll = self.roll(conversation, roll_key, now, moving);
        let shown = if foldable { roll.max(openness) } else { 0. }.clamp(0., 1.);
        let chevron = icon("chevron-right", 14.)
            .text_color(gpui::white())
            .with_transformation(Transformation::rotate(percentage(openness * 0.25)))
            .into_any_element();
        div()
            .id(("activity", index * 64 + section))
            .h(px(STEP_ROW))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(14.))
            .text_color(content(0.5))
            .when(foldable, |d| {
                d.hover(|s| s.text_color(content(0.8))).cursor_pointer()
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.set_roll(conversation, roll_key, *hovered, cx)
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                if foldable {
                    this.phase_open.insert(key, !open);
                    this.phase_moved.borrow_mut().insert(key, Instant::now());
                    cx.notify();
                }
            }))
            .child(push_tile(
                &ACTIVITY_TILE,
                Face::provider(provider),
                provider_glyph(provider, 12.),
                chevron,
                shown,
            ))
            .child(label)
            .into_any_element()
    }

    fn turn_body(
        &self,
        conversation: usize,
        index: usize,
        turn: &Turn,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if context_meter::is_compact_command(&turn.user) {
            return self.compaction_row(conversation, index, turn, now, moving);
        }
        let provider = turn
            .provider
            .unwrap_or(self.conversations[conversation].provider);
        let live = turn.live;
        let shown_answer = self.paced_answer(conversation, index, turn, moving);
        let rich = crate::reply_view::Rich {
            session: self.conversations[conversation].session_id.clone(),
            cwd: self.conversations[conversation].worktree.clone(),
            streaming: turn.live,
            act: std::rc::Rc::new(cx.listener(|this, act: &crate::reply_rich::Act, _, cx| {
                this.reply_act(act.clone(), cx)
            })),
        };
        let last_steps = turn
            .beats
            .iter()
            .rposition(|beat| matches!(beat, data::Beat::Steps(_)));
        let turn_width = self.turn_width.get();
        // An instruction opens a new stretch of the turn, as a prompt does in Monocode: its bubble,
        // then a header of its own over what followed.
        let mut sections: Vec<(Option<usize>, std::ops::Range<usize>)> = Vec::new();
        let (mut start, mut before) = (0, None);
        for (at, beat) in turn.beats.iter().enumerate() {
            if let data::Beat::Steer(steer) = beat {
                if at > start || before.is_some() {
                    sections.push((before, start..at));
                }
                before = Some(*steer);
                start = at + 1;
            }
        }
        sections.push((before, start..turn.beats.len()));
        let count = sections.len();
        let has_header =
            live || !turn.steps.is_empty() || !turn.agents.is_empty() || turn.seconds.is_some();
        let mut pill = self.background_pill(turn);
        let mut blocks: Vec<AnyElement> = Vec::new();
        let mut answer_view = None;
        for (section, (steer, range)) in sections.into_iter().enumerate() {
            let last_section = section + 1 == count;
            let working = live && last_section;
            let beats = &turn.beats[range.clone()];
            if let Some(steer) = steer {
                blocks.push(self.steer_bubble(index, steer, turn, conversation, cx));
            }
            // What came before the newest words folds under the header, even while the turn works:
            // the view keeps the latest reply and the running group, as in Monocode.
            let foldable = data::folds_work(beats);
            let answer = data::final_answer(beats);
            let key = (conversation, index, FOLD_KEY + section);
            // A ⌘F hit inside the fold shows: the current turn opens what holds the word.
            let found_inside = self.find.current() == Some(index)
                && self.find.query().is_some_and(|query| {
                    answer.is_some_and(|answer| {
                        beats[..answer].iter().any(|beat| {
                            matches!(beat, data::Beat::Text(text) if turn.answer.get(text.clone())
                                .is_some_and(|words| !transcript_find::ranges(words, query).is_empty()))
                        })
                    })
                });
            let open = found_inside || self.phase_open.get(&key).copied().unwrap_or(false);
            let openness = self.phase_moved.borrow().get(&key).copied().filter(|_| !found_inside).map_or(
                if open { 1. } else { 0. },
                |at| {
                    let since = now.duration_since(at).as_secs_f32();
                    if since < DISCLOSE {
                        *moving = true;
                    }
                    let eased = crate::disclosure::settle(since / DISCLOSE);
                    if open { eased } else { 1. - eased }
                },
            );
            let labelled = if section == 0 {
                has_header
            } else {
                working || !beats.is_empty()
            };
            if labelled {
                let header = self.work_header(
                    conversation,
                    index,
                    section,
                    turn,
                    provider,
                    (working, last_section, foldable),
                    (key, open, openness),
                    now,
                    moving,
                    cx,
                );
                blocks.push(match pill.take().filter(|_| section == 0) {
                    Some(pill) => div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(header)
                        .child(pill)
                        .into_any_element(),
                    None => header,
                });
            }
            let fold_at = blocks.len();
            let mut folded: Vec<AnyElement> = Vec::new();
            for (at, beat) in beats.iter().enumerate() {
                let hidden = foldable && answer.is_some_and(|answer| at < answer);
                if hidden && openness <= 0.001 {
                    continue;
                }
                let absolute = range.start + at;
                let element = match beat {
                    data::Beat::Text(text) => {
                        let end = text.end.min(shown_answer.len());
                        if text.start >= end {
                            continue;
                        }
                        // A settled turn's last words are drawn after its cards, with selection.
                        if !working && last_section && at + 1 == beats.len() {
                            answer_view = Some(&shown_answer[text.start..end]);
                            continue;
                        }
                        let body =
                            crate::markdown::render_rich(&shown_answer[text.start..end], &rich);
                        self.wake_reply_assets(cx);
                        body
                    }
                    data::Beat::Steps(steps) => self.phase_view(
                        conversation,
                        index,
                        turn,
                        steps.clone(),
                        live && Some(absolute) == last_steps,
                        if hidden {
                            turn_width - px(TILE_RAIL_INDENT)
                        } else {
                            turn_width
                        },
                        now,
                        moving,
                        cx,
                    ),
                    data::Beat::Steer(_) => continue,
                };
                if hidden {
                    folded.push(element);
                } else {
                    blocks.push(element);
                }
            }
            if !folded.is_empty() {
                let total = folded.len();
                let rows = folded.into_iter().enumerate().map(|(at, element)| {
                    div()
                        .relative()
                        .w_full()
                        .min_w_0()
                        .pl(px(TILE_RAIL_INDENT))
                        .pb(px(6.))
                        .child(rail(at + 1 == total))
                        // Work under the fold reads a step back from the answer, as in Monocode.
                        .child(div().opacity(0.83).child(element))
                });
                let body = div().w_full().flex().flex_col().children(rows);
                let body = if openness >= 0.999 || turn_width <= px(0.) {
                    body.into_any_element()
                } else {
                    div()
                        .w_full()
                        .overflow_hidden()
                        .opacity(openness.clamp(0., 1.))
                        .child(crate::disclosure::Disclosure::new(
                            body, turn_width, openness,
                        ))
                        .into_any_element()
                };
                // The fold goes right under the header, above the answer.
                let at = fold_at;
                blocks.insert(
                    at,
                    div().w_full().mt(px(-6.)).child(body).into_any_element(),
                );
            }
        }
        // Subagents, as one card (subagent_card.rs), and the pill for the ones in the background.
        let prompt_key = (conversation, index);
        let prompt_overflows = !turn.user.is_empty()
            && self
                .prompt_overflow
                .get(&prompt_key)
                .copied()
                .unwrap_or(false);
        let prompt_collapsed = prompt_overflows && !self.expanded_prompts.contains(&prompt_key);
        let agents = self.subagent_card(conversation, index, turn, provider, now, moving, cx);
        let width_store = self.turn_width.clone();
        // Show more slides the bubble open and closed.
        let prompt_motion = self
            .prompt_moved
            .get(&prompt_key)
            .map(|at| now.duration_since(*at).as_secs_f32())
            .filter(|since| *since < crate::disclosure::REVEAL)
            .map(|since| {
                *moving = true;
                let eased = crate::disclosure::settle(since / crate::disclosure::REVEAL);
                if prompt_collapsed { 1. - eased } else { eased }
            });
        let width_cell = self
            .prompt_widths
            .borrow_mut()
            .entry(prompt_key)
            .or_insert_with(|| Rc::new(Cell::new(px(0.))))
            .clone();
        let bubble_width = width_cell.get() - px(24.);
        div()
            .group(format!("turn-actions-{index}"))
            .relative()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                gpui::canvas(
                    move |bounds, _, _| width_store.set(bounds.size.width),
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .when(!turn.attachments.is_empty(), |d| {
                d.child(self.sent_chips(conversation, index, &turn.attachments, cx))
            })
            .when(!turn.user.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(2.))
                        .child(
                            div().flex().justify_end().child(
                                div()
                                    .relative()
                                    .max_w(px(576.))
                                    .min_w_0()
                                    .px(px(12.))
                                    .py(px(8.))
                                    .text_size(px(14.))
                                    .line_height(px(20.))
                                    .text_color(content(1.))
                                    .child(
                                        gpui::canvas(
                                            |_, _, _| {},
                                            move |bounds, _, window, _| {
                                                width_cell.set(bounds.size.width);
                                                let radius = if bounds.size.height <= px(37.) {
                                                    f32::from(bounds.size.height) * 0.5
                                                } else {
                                                    12.
                                                };
                                                window.paint_quad(
                                                    gpui::fill(bounds, accent_wash(0.24))
                                                        .corner_radii(px(radius)),
                                                );
                                            },
                                        )
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .size_full(),
                                    )
// Four lines while folded; Show more stays inside the bubble.
                                    .child(prompt_frame(prompt_motion, bubble_width, prompt_collapsed, transcript_select::user_text(
                                        &self.select,
                                        transcript_select::Reply {
                                            conversation,
                                            turn: index,
                                            user: true,
                                        },
                                        turn.user.clone(),
                                        styled_tokens(turn.user.clone()),
                                    )))
                        .when(prompt_overflows, |d| {
                            d.child(div().flex().child(
                                div()
                                    .id(("prompt-more", index))
                                    .mt_1()
                                    .px_1()
                                    // Its label lines up with the text; the padding is for the hover.
                                    .ml(px(-4.))
                                    .py(px(2.))
                                    .rounded(px(4.))
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .text_color(content(0.6))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(content(0.08)).text_color(content(1.)))
                                    .child(if prompt_collapsed {
                                        "Show more"
                                    } else {
                                        "Show less"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.expanded_prompts.remove(&prompt_key) {
                                            this.expanded_prompts.insert(prompt_key);
                                        }
                                        this.prompt_moved.insert(prompt_key, Instant::now());
                                        cx.notify();
                                    }))))
                        })
                            ),
                        )
                )
            })
            .children(blocks)
            .children(agents)
            .children(
                turn.visuals
                    .iter()
                    .enumerate()
                    .map(|(i, visual)| self.visual_element(index, i, visual, cx)),
            )
            .when_some(answer_view, |d, shown| {
                d.child(transcript_select::scoped(
                    &self.select,
                    transcript_select::Reply {
                        conversation,
                        turn: index,
                        user: false,
                    },
                    || {
                        // Linked pictures and videos, diagrams, formulas and HTML open from here.
                        let body = crate::markdown::render_rich(shown, &rich);
                        self.wake_reply_assets(cx);
                        body
                    },
                ))
            })
            .when_some(self.delegated_tasks_view(turn, cx), |d, row| d.child(row))
            .when_some(
                self.turn_actions(conversation, index, turn, cx),
                |d, row| d.child(row),
            )
            .when_some(
                self.turn_review_view(conversation, index, turn, cx),
                |d, review| d.child(review),
            )
            .into_any_element()
    }

    fn toggle_dock(&mut self, cx: &mut Context<Self>) {
        self.dock_open = !self.dock_open;
        if self.dock_open && self.dock_active.is_none() {
            self.chooser_since = Instant::now();
        }
        self.dock.set(
            if self.dock_open { 1. } else { 0. },
            Instant::now(),
            if cx.reduce_motion() { 0. } else { SIDEBAR },
            Curve::Expo,
        );
        cx.notify();
    }

    /// A step that is an image file: its file glyph and label. A click opens it in the Files pane.
    fn image_row(
        &self,
        path: &str,
        step: &data::Step,
        id: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let open = path.to_owned();
        let name = crate::tool_images::file_name(path).to_owned();
        let label = crate::tool_images::row_label(path, step.capture);
        let mark = if step.capture {
            // The shot itself says which one it is, without opening it.
            gpui::img(std::path::PathBuf::from(path))
                .w(px(26.))
                .h(px(20.))
                .flex_shrink_0()
                .rounded(px(4.))
                .border_1()
                .border_color(content(0.16))
                .object_fit(gpui::ObjectFit::Cover)
                .into_any_element()
        } else {
            crate::file_icons::file_icon(&name, false, 14.).into_any_element()
        };
        div()
            .id(SharedString::from(id))
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(7.))
            .cursor_pointer()
            .hover(|s| s.text_color(content(0.8)))
            .on_click(cx.listener(move |this, _, _, cx| this.open_image_file(open.clone(), cx)))
            .child(mark)
            .child(div().flex_shrink_0().child(label))
            .when_some(step.target.clone(), |d, target| {
                d.child(target_chip(target))
            })
    }

    /// Opens an image an agent read or changed: the Files pane comes up with it in a tab.
    fn open_image_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.open_pane(1, cx);
        let session = self.conversations[self.active_conversation()]
            .session_id
            .clone();
        if let Some(view) = self.pane_views.get(&(1, session)) {
            crate::panel_files::show_file(view, path, cx);
        }
        cx.notify();
    }

    fn open_pane(&mut self, kind: usize, cx: &mut Context<Self>) {
        // Terminal, Files and Changes work on this Mac's folders.
        if kind <= 2 && self.this_mac_only(cx) {
            return;
        }
        if kind == 0 && self.term_redirect(cx) {
            return;
        }
        if !self.dock_panes.contains(&kind) {
            self.dock_panes.push(kind);
        }
        self.ensure_pane_view(kind, cx);
        self.dock_active = Some(kind);
        if !self.dock_open {
            self.toggle_dock(cx);
        }
        cx.notify();
    }

    /// Each conversation has its own panes, built the first time it shows one.
    fn ensure_pane_view(&mut self, kind: usize, cx: &mut Context<Self>) {
        let conversation = &self.conversations[self.active_conversation()];
        let key = (kind, conversation.session_id.clone());
        if !self.pane_views.contains_key(&key) {
            let context = crate::panels::PanelContext {
                backend: self.backend.clone(),
                session_id: conversation.session_id.clone(),
                worktree: conversation.worktree.clone(),
                provider: conversation.provider,
                model: model_label(conversation.model.as_deref(), conversation.provider),
                version: self
                    .agent_versions
                    .get(conversation.provider)
                    .cloned()
                    .unwrap_or_default(),
            };
            if let Some(view) = crate::panels::open(kind, context, cx) {
                self.pane_views.insert(key, view);
            }
        }
    }

    fn close_pane(&mut self, kind: usize, cx: &mut Context<Self>) {
        self.dock_panes.retain(|k| *k != kind);
        if self.dock_active == Some(kind) {
            self.dock_active = self.dock_panes.last().copied();
            if self.dock_active.is_none() {
                self.chooser_since = Instant::now();
            }
        }
        cx.notify();
    }

    /// The side panel: its open panes as chips, or the six options in two columns.
    fn dock_view(
        &self,
        width: f32,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body: AnyElement = match self.dock_active {
            None => {
                let since = now.duration_since(self.chooser_since).as_secs_f32();
                let reduced = cx.reduce_motion();
                if since < 0.4 && !reduced {
                    *moving = true;
                }
                let cards = PANES.iter().take(VISIBLE_PANES).enumerate().map(
                    |(ix, (glyph, name, about))| {
                        // Each card rises in just after the one before it.
                        let t = if reduced {
                            1.
                        } else {
                            eased((since - ix as f32 * 0.03) / 0.22, Curve::Expo)
                        };
                        div()
                            .id(("pane-option", ix))
                            .relative()
                            .top(px((1. - t) * 6.))
                            .opacity(t)
                            .w(px(205.))
                            .h(px(70.))
                            .p(px(12.))
                            .rounded(px(crate::theme::RADIUS_LG))
                            .border_1()
                            .border_color(content(0.1))
                            .bg(content(0.03))
                            .hover(|s| s.bg(content(0.07)).border_color(content(0.16)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.open_pane(ix, cx)))
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(icon(glyph, 15.).text_color(content(0.75)))
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(13.5))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(content(0.95))
                                            .child(crate::i18n::t(*name)),
                                    ),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(content(0.5))
                                    .child(crate::i18n::t(*about)),
                            )
                    },
                );
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(5.))
                    .child(
                        div()
                            .text_size(px(14.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.95))
                            .child(t("Abrir um painel")),
                    )
                    .child(
                        div()
                            .mb(px(14.))
                            .text_size(px(12.5))
                            .text_color(content(0.5))
                            .child(t("Escolha o que abrir ao lado da conversa.")),
                    )
                    .child(
                        div()
                            .w(px(420.))
                            .flex()
                            .flex_wrap()
                            .justify_center()
                            .gap(px(10.))
                            .children(cards),
                    )
                    .into_any_element()
            }
            Some(kind) => {
                let (glyph, name, _) = PANES[kind];
                match self.pane_views.get(&(
                    kind,
                    self.conversations[self.active_conversation()]
                        .session_id
                        .clone(),
                )) {
                    Some(view) => div()
                        .flex_1()
                        .min_h_0()
                        .child(view.clone())
                        .into_any_element(),
                    None => div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(8.))
                        .child(icon(glyph, 22.).text_color(content(0.35)))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(content(0.5))
                                .child(tf("{} ainda não está pronto", &[&crate::i18n::t(name)])),
                        )
                        .into_any_element(),
                }
            }
        };
        div()
            .relative()
            .flex_shrink_0()
            .h_full()
            .w(px(width))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(body)
            .into_any_element()
    }

    fn toggle_palette(&mut self, files: bool, cx: &mut Context<Self>) {
        self.environment = None;
        self.appearance = None;
        if files {
            self.refresh_files();
        }
        let same = self.palette_ui.as_ref().is_some_and(|p| p.files == files);
        self.palette_ui = if same {
            None
        } else {
            Some(PaletteState {
                files,
                all: false,
                search: Default::default(),
                query: String::new(),
                selected: 0,
                open: Tween::from_to(
                    0.,
                    1.,
                    Instant::now(),
                    if cx.reduce_motion() { 0. } else { MENU },
                    Curve::Expo,
                ),
            })
        };
        cx.notify();
    }

    fn diff_stat(&self) -> (u32, u32) {
        let path = &self.conversations[self.active_conversation()].worktree;
        self.diffs.get(path).copied().unwrap_or((0, 0))
    }

    fn active_project(&self) -> String {
        let c = self.active_conversation();
        self.projects
            .iter()
            .find(|p| p.conversations.contains(&c))
            .map_or(String::new(), |p| p.name.clone())
    }

    /// As Sirus's command palette: conversations, words inside messages, then commands.
    fn palette_hits(&self, files: bool, query: &str) -> Vec<PaletteHit> {
        let needle = query.trim().to_lowercase();
        let mut hits = Vec::new();
        if files {
            if let Some(commands) = needle.strip_prefix('>') {
                let commands = commands.trim();
                for (ix, label) in COMMANDS.iter().enumerate() {
                    if *label != "Abrir Simulador" && t(label).to_lowercase().contains(commands) {
                        hits.push(PaletteHit::Command(ix));
                    }
                }
                return hits;
            }
            for (name, folder) in &self.files.1 {
                if needle.is_empty() || format!("{folder}/{name}").to_lowercase().contains(&needle)
                {
                    hits.push(PaletteHit::File(name.clone(), folder.clone()));
                    if hits.len() >= 9 {
                        break;
                    }
                }
            }
            hits.truncate(9);
            return hits;
        }
        if self.palette_ui.as_ref().is_some_and(|p| p.all) {
            return self.palette_message_hits(query.trim(), &[]);
        }
        for (ix, c) in self.conversations.iter().enumerate() {
            if needle.is_empty() || c.title.to_lowercase().contains(&needle) {
                hits.push(PaletteHit::Chat(ix));
            }
        }
        // Words inside messages come from the host's search (global_search.rs), from 3 characters.
        let listed: Vec<usize> = hits
            .iter()
            .filter_map(|hit| match hit {
                PaletteHit::Chat(ix) => Some(*ix),
                _ => None,
            })
            .collect();
        hits.extend(self.palette_message_hits(query.trim(), &listed));
        if !needle.is_empty() {
            for (ix, project) in self.projects.iter().enumerate() {
                if project.name.contains(&needle) {
                    hits.push(PaletteHit::Project(ix));
                }
            }
        }
        for (ix, label) in COMMANDS.iter().enumerate() {
            if *label != "Abrir Simulador" && t(label).to_lowercase().contains(&needle) {
                hits.push(PaletteHit::Command(ix));
            }
        }
        hits.truncate(if needle.is_empty() { 8 } else { 12 });
        hits
    }

    fn run_hit(&mut self, hit: PaletteHit, cx: &mut Context<Self>) {
        self.palette_ui = None;
        match hit {
            PaletteHit::Chat(c) | PaletteHit::Message(c, _) => self.open(c, cx),
            PaletteHit::File(..) => self.open_pane(1, cx),
            PaletteHit::Project(ix) => {
                self.projects[ix].open = true;
                if let Some(first) = self.projects[ix].conversations.first().copied() {
                    self.open(first, cx);
                }
            }
            PaletteHit::Command(0) => self.new_conversation(None, cx),
            PaletteHit::Command(1) => self.toggle_sidebar(cx),
            PaletteHit::Command(2) => self.toggle_dock(cx),
            PaletteHit::Command(3) => self.toggle_appearance(cx),
            PaletteHit::Command(4) => self.toggle_pin(self.active, cx),
            PaletteHit::Command(11) => self.toggle_composer_menu(ComposerMenu::Effort, cx),
            PaletteHit::Command(12) => self.toggle_usage_popup(cx),
            PaletteHit::Command(13) => {
                self.toggle_project_menu(cx);
                self.open_project_sub(ProjectSub::Projects, cx);
            }
            PaletteHit::Command(14) => self.toggle_usage_page(cx),
            PaletteHit::Command(15) => self.open_pull_requests(cx),
            PaletteHit::Command(16) => self.copy_prompt_link(cx),
            PaletteHit::Command(pane) => self.open_pane(pane - 5, cx),
        }
        cx.notify();
    }

    fn palette_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.palette_ui.as_ref()?;
        if !state.open.done(now) {
            *moving = true;
        }
        let p = state.open.at(now).clamp(0., 1.);
        let hits = self.palette_hits(state.files, &state.query);
        let selected = state.selected.min(hits.len().saturating_sub(1));
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut group = "";
        for (ix, hit) in hits.iter().enumerate() {
            let (heading, title, detail, mark): (&str, String, Option<String>, AnyElement) =
                match hit {
                    PaletteHit::Chat(c) => {
                        let conv = &self.conversations[*c];
                        (
                            t("Conversas recentes"),
                            conv.title.clone(),
                            Some(conv.age()),
                            mark_tile(
                                &crate::tab_tile::TileSize {
                                    w: 18.,
                                    h: 16.,
                                    radius: 4.,
                                },
                                Face::provider(conv.provider),
                                provider_glyph(conv.provider, 11.),
                            )
                            .into_any_element(),
                        )
                    }
                    PaletteHit::Message(c, snippet) => {
                        let conv = &self.conversations[*c];
                        (
                            t("Nas mensagens"),
                            snippet.clone(),
                            Some(conv.title.clone()),
                            mark_tile(
                                &crate::tab_tile::TileSize {
                                    w: 18.,
                                    h: 16.,
                                    radius: 4.,
                                },
                                Face::provider(conv.provider),
                                provider_glyph(conv.provider, 11.),
                            )
                            .into_any_element(),
                        )
                    }
                    PaletteHit::File(name, folder) => (
                        "",
                        name.to_string(),
                        Some(if folder.is_empty() {
                            name.to_string()
                        } else {
                            format!("{folder}/{name}")
                        }),
                        crate::file_icons::file_icon(name, false, 18.),
                    ),
                    PaletteHit::Project(p) => (
                        t("Projetos"),
                        self.projects[*p].name.to_string(),
                        None,
                        div()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("folder", 14.).text_color(content(0.6)))
                            .into_any_element(),
                    ),
                    PaletteHit::Command(i) => (
                        t("Comandos"),
                        t(COMMANDS[*i]).to_string(),
                        None,
                        div()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("chevron-right", 12.).text_color(content(0.5)))
                            .into_any_element(),
                    ),
                };
            if heading != group && !heading.is_empty() {
                group = heading;
                rows.push(
                    div()
                        .px(px(14.))
                        .pt(px(8.))
                        .pb(px(3.))
                        .text_size(px(11.5))
                        .text_color(content(0.4))
                        .child(heading)
                        .into_any_element(),
                );
            }
            let hit = hit.clone();
            let two_lines = matches!(hit, PaletteHit::File(..));
            rows.push(
                crate::hover_track::list("palette")
                    .row(
                        div()
                            .id(("palette-row", ix))
                            .mx(px(6.))
                            .h(px(if two_lines { 46. } else { 34. }))
                            .px(px(8.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .text_size(px(13.))
                            .text_color(content(0.9))
                            .when(ix == selected, |d| d.bg(content(0.1)))
                            .cursor_pointer()
                            .on_click(cx.listener({
                                let hit = hit.clone();
                                move |this, _, _, cx| this.run_hit(hit.clone(), cx)
                            }))
                            .child(mark)
                            .map(|d| {
                                if two_lines {
                                    d.child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .child(div().truncate().child(title))
                                            .children(detail.clone().map(|d| {
                                                div()
                                                    .truncate()
                                                    .text_size(px(11.5))
                                                    .text_color(content(0.45))
                                                    .child(d)
                                            })),
                                    )
                                } else {
                                    d.child(div().flex_1().min_w_0().truncate().child(title))
                                        .children(detail.clone().map(|d| {
                                            div()
                                                .flex_shrink_0()
                                                .max_w(px(170.))
                                                .truncate()
                                                .text_size(px(11.5))
                                                .text_color(content(0.4))
                                                .child(d)
                                        }))
                                }
                            }),
                        ix,
                        ix == selected,
                    )
                    .into_any_element(),
            );
        }
        if hits.is_empty() {
            rows.push(
                div()
                    .px(px(16.))
                    .py(px(18.))
                    .text_size(px(13.))
                    .text_color(content(0.45))
                    .child(self.palette_note())
                    .into_any_element(),
            );
        }
        Some(
            div()
                .id("palette-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .bg(gpui::hsla(0., 0., 0., 0.38 * p))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.palette_ui = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("palette")
                        .occlude()
                        .absolute()
                        .left(gpui::relative(0.5))
                        .ml(px(-290.))
                        .top(px(92. + (1. - p) * -8.))
                        .w(px(580.))
                        .opacity(p)
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(content(0.12))
                        .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                        .shadow_lg()
                        .pb(px(6.))
                        .flex()
                        .flex_col()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h(px(50.))
                                .px(px(16.))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .border_b_1()
                                .border_color(content(0.08))
                                .text_size(px(14.))
                                .child(icon("search", 15.).text_color(content(0.5)))
                                .child(typed_text(
                                    &state.query,
                                    if state.files {
                                        t("Buscar arquivos pelo nome…  (> para comandos)")
                                    } else if state.all {
                                        t("Buscar em todas as conversas")
                                    } else {
                                        t("Buscar conversas, mensagens ou comandos")
                                    },
                                    div().flex_shrink_0().w(px(1.5)).h(px(18.)).bg(accent()),
                                )),
                        )
                        .child(crate::hover_track::list("palette").pill())
                        .children(rows)
                        .when(state.files, |d| {
                            d.child(
                                div()
                                    .mt(px(4.))
                                    .h(px(32.))
                                    .px(px(16.))
                                    .flex()
                                    .items_center()
                                    .gap(px(14.))
                                    .border_t_1()
                                    .border_color(content(0.08))
                                    .text_size(px(11.5))
                                    .text_color(content(0.4))
                                    .child(t("↑ ↓ navegar"))
                                    .child(t("↵ abrir"))
                                    .child(t("Esc fechar"))
                                    .child(t("> comandos")),
                            )
                        }),
                )
                .into_any_element(),
        )
    }

    fn toggle_env_row(&mut self, row: &'static str) {
        self.branch_draft = None;
        self.branch_error = None;
        self.env_open = if self.env_open == Some(row) {
            None
        } else {
            Some(row)
        };
        let ix = self.active_conversation();
        let session = self.conversations[ix].session_id.clone();
        match row {
            "branch" => {
                if let Some(project) = self.project_of(ix) {
                    let id = self.backend.invoke(
                        "list_branches",
                        json!({ "projectId": self.projects[project].id }),
                    );
                    self.waiting.insert(id, Waiting::Branches);
                }
            }
            "servers" => {
                let id = self
                    .backend
                    .invoke("local_servers_list", json!({ "sessionId": session }));
                self.waiting.insert(id, Waiting::Servers);
            }
            "commit" => {
                self.commit_message = if self.env_open.is_some() {
                    Some(String::new())
                } else {
                    None
                };
            }
            _ => {}
        }
        if row != "commit" {
            self.commit_message = None;
        }
    }

    fn commit_now(&mut self, cx: &mut Context<Self>) {
        let Some(message) = self.commit_message.clone().filter(|m| !m.trim().is_empty()) else {
            return;
        };
        let ix = self.active_conversation();
        let id = self.backend.invoke(
            "git_commit",
            json!({ "sessionId": self.conversations[ix].session_id, "message": message }),
        );
        self.waiting.insert(id, Waiting::Session(usize::MAX));
        self.commit_message = Some(String::new());
        self.refresh_diff();
        cx.notify();
    }

    /// The Ambiente card, compact: one 26 px row per function, each opening in place. Same
    /// functions as Sirus Code's EnvironmentPanel.
    fn environment_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.environment.as_ref()?;
        if !open.done(now) {
            *moving = true;
        }
        let p = open.at(now).clamp(0., 1.);
        let ix = self.active_conversation();
        let conversation = &self.conversations[ix];
        let session = conversation.session_id.clone();
        let (added, removed) = self.diff_stat();
        let (add_color, remove_color) = diff_colors(self.look.diff);
        let row = |id: &'static str, glyph: &'static str, label: String| {
            crate::hover_track::list("environment").row_named(
                div()
                    .id(id)
                    .relative()
                    .h(px(26.))
                    .px(px(7.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(12.5))
                    .text_color(content(0.75))
                    .hover(|s| s.text_color(content(1.)))
                    .cursor_pointer()
                    .child(icon(glyph, 14.).text_color(content(0.5)))
                    .child(div().flex_1().min_w_0().truncate().child(label)),
                id,
                false,
            )
        };
        let value = |text: String| {
            div()
                .flex_shrink_0()
                .max_w(px(96.))
                .truncate()
                .text_size(px(11.5))
                .text_color(content(0.45))
                .child(text)
        };
        let expand = |id: &'static str,
                      glyph: &'static str,
                      label: &str,
                      trailing: String,
                      cx: &mut Context<Self>| {
            row(id, glyph, label.to_owned())
                .child(value(trailing))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_env_row(id);
                    cx.notify();
                }))
        };
        let sub = |children: Vec<AnyElement>| {
            div()
                .mx(px(2.))
                .mb(px(3.))
                .p(px(3.))
                .rounded(px(crate::theme::RADIUS_MD))
                .bg(content(0.03))
                .flex()
                .flex_col()
                .children(children)
        };
        let small = |id: &'static str, label: &'static str, main: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(22.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.5))
                .cursor_pointer()
                .text_color(if main { on_accent() } else { content(0.85) })
                .bg(if main { accent() } else { content(0.08) })
                .hover(|s| s.opacity(0.88))
        };
        let open_row = self.env_open;
        let mut rows: Vec<AnyElement> = vec![
            div()
                .h(px(22.))
                .px(px(7.))
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(12.))
                .text_color(content(0.45))
                .child(t("Ambiente"))
                .into_any_element(),
            row("env-changes", "git-compare", t("Alterações").into())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.environment = None;
                    this.open_pane(2, cx);
                }))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(add_color)
                        .child(format!("+{added}")),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(remove_color)
                        .child(format!("-{removed}")),
                )
                .into_any_element(),
            expand(
                "branch",
                "branch",
                "Branch",
                conversation.branch.clone(),
                cx,
            )
            .into_any_element(),
        ];
        if open_row == Some("branch") {
            let current = conversation.branch.clone();
            let project_id = self
                .project_of(ix)
                .map(|p| self.projects[p].id.clone())
                .unwrap_or_default();
            let session_for_pull = session.clone();
            let mut list: Vec<AnyElement> = self
                .env_branches
                .iter()
                .take(8)
                .enumerate()
                .map(|(i, branch)| {
                    let (name, project_id) = (branch.clone(), project_id.clone());
                    div()
                        .id(("env-branch-item", i))
                        .h(px(24.))
                        .px(px(6.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(12.))
                        .text_color(content(0.8))
                        .cursor_pointer()
                        .hover(|s| s.bg(content(0.07)))
                        .child(div().min_w_0().truncate().child(branch.clone()))
                        .when(*branch == current, |d| {
                            d.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(11.))
                                    .text_color(content(0.4))
                                    .child(t("atual")),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.backend.invoke(
                                "checkout_branch",
                                json!({ "projectId": project_id, "branch": name, "confirm": true }),
                            );
                            this.env_open = None;
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect();
            list.push(div().id("env-pull").h(px(24.)).px(px(6.)).rounded(px(crate::theme::RADIUS_SM)).flex().items_center().text_size(px(12.)).text_color(content(0.6)).cursor_pointer().hover(|s| s.bg(content(0.07)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.backend.invoke("git_workspace_action", json!({ "action": { "type": "pull", "sessionId": session_for_pull, "confirm": true } }));
                    cx.notify();
                }))
                .child(t("Puxar da origem")).into_any_element());
            list.push(
                div()
                    .id("env-branch-new")
                    .h(px(24.))
                    .px(px(6.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(content(0.6))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.branch_draft = Some(String::new());
                        this.branch_error = None;
                        cx.notify();
                    }))
                    .child(t("Criar branch…"))
                    .into_any_element(),
            );
            // The name field (git_sync.rs): the problem with the typed name shows under it, else
            // the host's last answer.
            if let Some(draft) = self.branch_draft.clone() {
                let checked = git_sync::check_branch_name(&draft, &self.env_branches);
                let problem = checked
                    .as_ref()
                    .err()
                    .and_then(|p| git_sync::problem_text(*p));
                let error = problem
                    .map(str::to_owned)
                    .or_else(|| self.branch_error.clone());
                list.push(git_sync::branch_field(
                    &draft,
                    error.as_deref(),
                    checked.is_ok(),
                    self.branch_busy,
                    cx.listener(|this, _, _, cx| this.submit_branch(cx)),
                ));
            }
            rows.push(sub(list).into_any_element());
        }
        rows.push(
            expand(
                "commit",
                "upload",
                t("Commit, push e PR"),
                String::new(),
                cx,
            )
            .into_any_element(),
        );
        if open_row == Some("commit") {
            let session_push = session.clone();
            let message = self.commit_message.clone().unwrap_or_default();
            rows.push(
                sub(vec![
                    div()
                        .h(px(24.))
                        .mb(px(3.))
                        .px(px(6.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .border_1()
                        .border_color(content(0.12))
                        .flex()
                        .items_center()
                        .gap(px(1.))
                        .text_size(px(12.))
                        .when(message.is_empty(), |d| {
                            d.child(div().w(px(1.5)).h(px(13.)).bg(accent())).child(
                                div()
                                    .text_color(content(0.4))
                                    .child(t("Mensagem do commit")),
                            )
                        })
                        .when(!message.is_empty(), |d| {
                            d.child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(content(0.95))
                                    .child(message.clone()),
                            )
                            .child(div().w(px(1.5)).h(px(13.)).bg(accent()))
                        })
                        .into_any_element(),
                    div()
                        .flex()
                        .gap(px(3.))
                        .child(
                            small("env-commit", "Commit", true)
                                .on_click(cx.listener(|this, _, _, cx| this.commit_now(cx))),
                        )
                        .child(small("env-push", "Push", false).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.backend
                                    .invoke("git_push", json!({ "sessionId": session_push }));
                                cx.notify();
                            },
                        )))
                        .into_any_element(),
                ])
                .into_any_element(),
            );
        }
        rows.push(
            expand(
                "servers",
                "server",
                t("Servidores locais"),
                if self.env_servers.is_empty() {
                    String::new()
                } else {
                    self.env_servers.len().to_string()
                },
                cx,
            )
            .into_any_element(),
        );
        if open_row == Some("servers") {
            let list: Vec<AnyElement> = if self.env_servers.is_empty() {
                vec![
                    div()
                        .h(px(24.))
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .text_size(px(12.))
                        .text_color(content(0.4))
                        .child(t("Nenhum servidor local"))
                        .into_any_element(),
                ]
            } else {
                self.env_servers.iter().enumerate().map(|(i, url)| {
                    let (stop_url, stop_session) = (url.clone(), session.clone());
                    div().h(px(24.)).px(px(6.)).flex().items_center().gap(px(6.)).text_size(px(12.)).text_color(content(0.8))
                        .child(div().size(px(6.)).flex_shrink_0().rounded_full().bg(rgb(0x7ad69a)))
                        .child(div().flex_1().min_w_0().truncate().child(url.clone()))
                        .child(div().id(("env-stop", i)).flex_shrink_0().text_size(px(11.)).text_color(content(0.45)).cursor_pointer().hover(|s| s.text_color(content(0.9)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.backend.invoke("local_server_action", json!({ "action": { "type": "stop", "sessionId": stop_session, "url": stop_url } }));
                                this.env_servers.retain(|u| *u != stop_url);
                                cx.notify();
                            }))
                            .child(t("Parar")))
                        .into_any_element()
                }).collect()
            };
            rows.push(sub(list).into_any_element());
        }
        rows.push(
            div()
                .my(px(3.))
                .mx(px(5.))
                .h(px(1.))
                .bg(content(0.08))
                .into_any_element(),
        );
        rows.push(
            expand("repo", "folder", &self.active_project(), String::new(), cx).into_any_element(),
        );
        if open_row == Some("repo") {
            let mut list: Vec<AnyElement> = self.env_editors.iter().enumerate().map(|(i, (id, name))| {
                let (editor, editor_session) = (id.clone(), session.clone());
                div().id(("env-editor", i)).h(px(26.)).px(px(6.)).rounded(px(crate::theme::RADIUS_SM)).flex().items_center().gap(px(7.)).text_size(px(12.)).text_color(content(0.8)).cursor_pointer().hover(|s| s.bg(content(0.07)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.this_mac_only(cx) {
                            this.backend.invoke("open_in_editor", json!({ "sessionId": editor_session, "editor": editor, "path": Value::Null }));
                        }
                        this.environment = None;
                        cx.notify();
                    }))
                    .child(match self.env_icons.get(id) {
                        Some(path) => gpui::img(path.clone()).size(px(16.)).flex_shrink_0().into_any_element(),
                        None => icon("folder", 14.).text_color(content(0.5)).into_any_element(),
                    })
                    .child(name.clone())
                    .into_any_element()
            }).collect();
            let path = conversation.worktree.clone();
            list.push(
                div()
                    .id("env-copy-path")
                    .h(px(24.))
                    .px(px(6.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(content(0.6))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.clone()));
                        this.env_open = None;
                        cx.notify();
                    }))
                    .child(t("Copiar caminho"))
                    .into_any_element(),
            );
            rows.push(sub(list).into_any_element());
        }
        Some(
            div()
                .id("environment-card")
                .occlude()
                .absolute()
                .right(px(10.))
                .top(px(TITLE_BAR + 10. + (1. - p) * -6.))
                .w(px(248.))
                .p(px(4.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_MD))
                .shadow_lg()
                .opacity(p)
                .flex()
                .flex_col()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(crate::hover_track::list("environment").pill())
                .children(rows)
                .into_any_element(),
        )
    }

    /// Asks the host for the selected conversation's incoming commits (git_sync.rs).
    fn incoming_fetch(&mut self) {
        let session = self.incoming.session().to_owned();
        if session.is_empty() {
            return;
        }
        let generation = self.incoming.generation();
        let id = self
            .backend
            .invoke("git_incoming", json!({ "sessionId": session }));
        self.waiting.insert(id, Waiting::Incoming(generation));
    }

    /// The header's Puxar: `git pull --ff-only` from the branch's upstream.
    fn incoming_pull(&mut self, cx: &mut Context<Self>) {
        if !self.incoming.begin_pull() {
            return;
        }
        let session = self.incoming.session().to_owned();
        let generation = self.incoming.generation();
        let id = self
            .backend
            .invoke("git_pull_ff", json!({ "sessionId": session }));
        self.waiting.insert(id, Waiting::IncomingPull(generation));
        cx.notify();
    }

    fn incoming_pulled(
        &mut self,
        generation: u64,
        ok: bool,
        value: &Value,
        cx: &mut Context<Self>,
    ) {
        let answer = git_sync::pull_answer(ok, value);
        let done = matches!(answer, git_sync::Pulled::Done(_));
        if !self.incoming.pulled(generation, answer) {
            return;
        }
        if done {
            self.refresh_diff();
            // The green check stays a moment, then the popover closes.
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(git_sync::PULLED_FOR).await;
                let _ = this.update(cx, |this, cx| {
                    if this.incoming.faded(generation) {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn incoming_toggle(&mut self, cx: &mut Context<Self>) {
        self.environment = None;
        let duration = if cx.reduce_motion() { 0. } else { MENU };
        self.incoming.toggle(Instant::now(), duration);
        cx.notify();
    }

    /// "Criar branch…" in the Ambiente card: checks the name, then the host creates and switches.
    fn submit_branch(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.branch_draft.clone() else {
            return;
        };
        let ix = self.active_conversation();
        let Some(project) = self.project_of(ix) else {
            return;
        };
        if self.branch_busy {
            return;
        }
        if let Ok(name) = git_sync::check_branch_name(&draft, &self.env_branches) {
            self.branch_busy = true;
            self.branch_error = None;
            let id = self.backend.invoke(
                "create_branch",
                json!({ "projectId": self.projects[project].id, "branch": name, "confirm": true }),
            );
            self.waiting.insert(id, Waiting::BranchCreate);
        }
        cx.notify();
    }

    fn branch_created(&mut self, ok: bool, value: &Value, cx: &mut Context<Self>) {
        self.branch_busy = false;
        if ok {
            self.branch_draft = None;
            self.branch_error = None;
            self.env_open = None;
            self.refresh_diff();
        } else {
            self.branch_error = Some(error_text(value));
        }
        cx.notify();
    }

    fn toggle_appearance(&mut self, cx: &mut Context<Self>) {
        self.appearance = match self.appearance {
            Some(_) => None,
            None => Some(Tween::from_to(
                0.,
                1.,
                Instant::now(),
                if cx.reduce_motion() { 0. } else { MENU },
                Curve::Expo,
            )),
        };
        self.load_settings_page();
        cx.notify();
    }

    /// Tells the host which accent the phone should wear; only when it changed, unless forced
    /// (a new connection to the host starts without it).
    fn push_phone_look(&mut self, force: bool) {
        let args = phone_look_args(&self.look);
        let accent = args["accent"].as_str().unwrap_or_default().to_owned();
        if force || accent != self.phone_accent {
            self.phone_accent = accent;
            self.backend.invoke("phone_look_set", args);
        }
    }

    /// Colors ease to the new look instead of cutting to it.
    fn set_look(&mut self, change: impl FnOnce(&mut Look), cx: &mut Context<Self>) {
        let now = Instant::now();
        self.palette_from = crate::theme::current_palette();
        change(&mut self.look);
        save_look(&self.look);
        self.push_phone_look(false);
        self.palette_t = Tween::from_to(
            0.,
            1.,
            now,
            if cx.reduce_motion() { 0. } else { 0.26 },
            Curve::Expo,
        );
        let _ = now;
        cx.notify();
    }

    fn pick_theme(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.theme_menu = None;
        self.set_look(
            |l| {
                l.preset = ix;
                l.accent = None;
                l.custom = None;
                l.hue = None;
                l.saturation = None;
            },
            cx,
        );
    }

    fn set_slider(&mut self, id: usize, x: f32, cx: &mut Context<Self>) {
        let Some(bounds) = self.slider_bounds.borrow()[id] else {
            return;
        };
        let v = ((x - f32::from(bounds.left())) / f32::from(bounds.size.width)).clamp(0., 1.);
        let (hue, light) = self.look.custom.unwrap_or((260., 0.72));
        self.look.custom = Some(if id == 0 {
            (v * 360., light)
        } else {
            (hue, 0.35 + v * 0.5)
        });
        save_look(&self.look);
        self.push_phone_look(false);
        cx.notify();
    }

    fn slider(
        &self,
        id: usize,
        value: f32,
        stops: Vec<gpui::Hsla>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let store = self.slider_bounds.clone();
        div()
            .id(("custom-slider", id))
            .relative()
            .w(px(200.))
            .h(px(14.))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.slider_drag = Some(id);
                    this.set_slider(id, f32::from(event.position.x), cx);
                }),
            )
            .child(
                gpui::canvas(
                    move |bounds, _, _| store.borrow_mut()[id] = Some(bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(4.))
                    .h(px(6.))
                    .rounded_full()
                    .overflow_hidden()
                    .flex()
                    .children(stops.into_iter().map(|c| div().flex_1().h_full().bg(c))),
            )
            .child(
                div()
                    .absolute()
                    .left(px(value * 200. - 7.))
                    .top_0()
                    .size(px(14.))
                    .rounded_full()
                    .bg(gpui::white())
                    .border_1()
                    .border_color(gpui::hsla(0., 0., 0., 0.3))
                    .shadow_md(),
            )
            .into_any_element()
    }

    /// The Settings page, as Sirus's today: the whole window, a 256 px menu with Back to app,
    /// search and the same pages, and a 780 px column. Aparência works; the other pages come
    /// when real settings connect.
    fn appearance_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.appearance.as_ref()?;
        if !open.done(now) {
            *moving = true;
        }
        let p = open.at(now).clamp(0., 1.);
        if self.theme_menu.as_ref().is_some_and(|m| !m.open.done(now)) {
            *moving = true;
        }
        let since = now.duration_since(self.settings_since).as_secs_f32();
        let reduced = cx.reduce_motion();
        if since < 0.28 && !reduced {
            *moving = true;
        }
        let page_t = if reduced {
            1.
        } else {
            eased(since / 0.28, Curve::Expo)
        };
        // Back and search stay put: only the page list scrolls, so Back never slides under the
        // window's traffic lights.
        let nav_head = div()
            .flex_shrink_0()
            .pt(px(TITLE_BAR))
            .px(px(8.))
            .child(
                crate::hover_track::list("settings-nav").row_named(
                    div()
                        .id("settings-back")
                        .h(px(30.))
                        .mb(px(10.))
                        .px(px(10.))
                        .rounded(px(crate::theme::RADIUS_MD))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(px(13.))
                        .text_color(content(0.95))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.appearance = None;
                            cx.notify();
                        }))
                        .child(icon("arrow-left", 16.).text_color(content(0.65)))
                        .child(t("Voltar ao app")),
                    "settings-back",
                    false,
                ),
            )
            .child(
                div()
                    .h(px(34.))
                    .mb(px(10.))
                    .px(px(10.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .border_1()
                    .border_color(content(0.09))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .text_color(content(0.4))
                    .child(icon("search", 14.).text_color(content(0.45)))
                    .child(t("Buscar configurações")),
            );
        let nav_list = div()
            .id("settings-nav")
            .flex_1()
            .min_h_0()
            .px(px(8.))
            .overflow_y_scroll()
            .child(crate::hover_track::list("settings-nav").pill())
            .children(SETTINGS_NAV.iter().enumerate().map(|(g, (group, items))| {
                div()
                    .child(
                        div()
                            .mt(px(if g == 0 { 4. } else { 16. }))
                            .mb(px(4.))
                            .px(px(10.))
                            .text_size(px(11.5))
                            .text_color(content(0.45))
                            .child(t(*group)),
                    )
                    .children(items.iter().map(|(glyph, name)| {
                        let name: &'static str = name;
                        let chosen = name == self.settings_page;
                        crate::hover_track::list("settings-nav").row_named(
                            div()
                                .id(name)
                                .h(px(32.))
                                .px(px(10.))
                                .rounded(px(crate::theme::RADIUS_MD))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .text_size(px(13.))
                                .text_color(content(if chosen { 1. } else { 0.72 }))
                                .when(chosen, |d| d.bg(content(0.1)))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.settings_page == name {
                                        return;
                                    }
                                    let order = |n: &str| {
                                        SETTINGS_NAV
                                            .iter()
                                            .flat_map(|(_, items)| items.iter())
                                            .position(|(_, i)| *i == n)
                                            .unwrap_or(0)
                                    };
                                    this.settings_direction =
                                        if order(name) > order(this.settings_page) {
                                            1.
                                        } else {
                                            -1.
                                        };
                                    this.settings_page = name;
                                    this.settings_since = Instant::now();
                                    this.theme_menu = None;
                                    this.load_settings_page();
                                    cx.notify();
                                }))
                                .child(icon(glyph, 16.).text_color(content(if chosen {
                                    0.9
                                } else {
                                    0.55
                                })))
                                .child(t(nav_label(name))),
                            name,
                            chosen,
                        )
                    }))
            }));
        let (bar, body): (AnyElement, AnyElement) = if self.settings_page == APPEARANCE {
            (
                crate::settings_pages::title_bar(
                    t("Aparência"),
                    vec![crate::settings_pages::restore_look_button(
                        Shell::run_setting,
                        cx,
                    )],
                ),
                self.appearance_page(cx),
            )
        } else {
            crate::settings_connectors::set_context(
                self.projects
                    .iter()
                    .map(|p| (p.id.clone(), p.name.clone()))
                    .collect(),
                self.current_project
                    .and_then(|ix| self.projects.get(ix))
                    .map(|p| p.id.clone()),
                (self.viewport_width - 256. - 64.).min(716.),
            );
            crate::settings_pages::page(
                self.settings_page,
                &self.settings,
                &self.settings_data,
                self.setting_edit.as_ref().map(|(k, v)| (*k, v.as_str())),
                Shell::save_setting,
                Shell::edit_setting,
                Shell::run_setting,
                cx,
            )
        };
        Some(
            div()
                .id("settings-page")
                .occlude()
                .absolute()
                .inset_0()
                .opacity(p)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if this.theme_menu.take().is_some() {
                            cx.notify();
                        }
                    }),
                )
                // Over glass the app underneath fades out instead, so the desktop shows through.
                .bg(if pane_fill().a < 0.5 {
                    chrome_wash()
                } else {
                    pane_opaque()
                })
                .flex()
                .child(
                    div()
                        .w(px(self.sidebar_width))
                        .flex_shrink_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(nav_head)
                        .child(nav_list),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .my(px(6.))
                        .mr(px(6.))
                        .flex()
                        .flex_col()
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(content(0.08))
                        .bg(pane_fill())
                        .overflow_hidden()
                        .child(div().opacity(page_t).child(bar))
                        .child(
                            div().flex_1().min_h_0().flex().child(
                                div()
                                    .id("settings-main")
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .overflow_y_scroll()
                                    .child(
                                        div()
                                            .relative()
                                            .top(px((1. - page_t) * 22. * self.settings_direction))
                                            .opacity(page_t)
                                            .w_full()
                                            .max_w(px(crate::settings_pages::CONTENT_WIDTH))
                                            .mx_auto()
                                            .pt(px(16.))
                                            .pb(px(40.))
                                            .px(px(crate::settings_pages::CONTENT_INSET))
                                            .child(body),
                                    ),
                            ),
                        ),
                )
                .children(crate::settings_connectors::overlay(
                    self.settings_page,
                    &self.settings,
                    self.setting_edit.as_ref().map(|(k, v)| (*k, v.as_str())),
                    Shell::save_setting,
                    Shell::edit_setting,
                    Shell::run_setting,
                    cx,
                ))
                .children(crate::settings_pair::overlay(
                    self.settings_page,
                    self.setting_edit.as_ref().map(|(k, v)| (*k, v.as_str())),
                    Shell::run_setting,
                    Shell::edit_setting,
                    cx,
                ))
                .into_any_element(),
        )
    }

    fn appearance_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let look = self.look;
        let row = |title: &'static str, about: &'static str, control: AnyElement| {
            div()
                .px(px(16.))
                .py(px(14.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(20.))
                .border_b_1()
                .border_color(content(0.07))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(content(0.95))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(content(0.5))
                                .child(about),
                        ),
                )
                .child(control)
                .into_any_element()
        };
        let group = |title: &'static str, rows: Vec<AnyElement>| {
            div()
                .mb(px(32.))
                .child(
                    div()
                        .mb(px(4.))
                        .text_size(px(12.5))
                        .text_color(content(0.5))
                        .child(title),
                )
                .child(
                    div()
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(content(0.09))
                        .bg(content(0.02))
                        .overflow_hidden()
                        .children(rows),
                )
        };
        fn segmented(
            id: &'static str,
            options: &[&'static str],
            chosen: usize,
            cx: &mut Context<Shell>,
            pick: fn(&mut Look, usize),
        ) -> AnyElement {
            div()
                .flex()
                .p(px(2.))
                .rounded(px(crate::theme::RADIUS_SM))
                .border_1()
                .border_color(content(0.1))
                .children(options.iter().enumerate().map(|(ix, label)| {
                    div()
                        .id((id, ix))
                        .h(px(24.))
                        .px(px(10.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .flex()
                        .items_center()
                        .text_size(px(12.5))
                        .text_color(content(if ix == chosen { 1. } else { 0.55 }))
                        .when(ix == chosen, |d| d.bg(content(0.1)))
                        .when(ix != chosen, |d| d.hover(|s| s.text_color(content(0.9))))
                        .cursor_pointer()
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.set_look(|l| pick(l, ix), cx)),
                        )
                        .child(*label)
                }))
                .into_any_element()
        }
        // Theme, as ThemeModePicker: three small windows to choose from.
        let modes = segmented_icons(
            "theme-mode",
            &[
                ("monitor", t("Sistema")),
                ("sun", t("Claro")),
                ("moon", t("Escuro")),
            ],
            [0, 2, 1].iter().position(|m| *m == look.mode).unwrap_or(0),
            cx,
            |l, ix| l.mode = [0, 2, 1][ix],
        );
        let swatch = |ix: usize| {
            let colors = palette(
                &Look {
                    preset: ix,
                    mode: 1,
                    ..Look::default()
                },
                true,
            );
            div()
                .size(px(18.))
                .flex_shrink_0()
                .rounded(px(crate::theme::RADIUS_SM))
                .border_1()
                .border_color(content(0.16))
                .bg(colors.field)
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(7.)).rounded_full().bg(colors.accent))
        };
        // Workspace theme: a dropdown with a search field, as Monocode's picker.
        let menu = self.theme_menu.as_ref().map(|menu| {
            let t = menu.open.at(Instant::now()).clamp(0., 1.);
            let needle = menu.query.to_lowercase();
            let rows: Vec<AnyElement> = PRESETS
                .iter()
                .enumerate()
                .filter(|(_, p)| crate::i18n::t(p.name).to_lowercase().contains(&needle))
                .enumerate()
                .map(|(row, (ix, preset))| {
                    let highlighted = row == menu.selected;
                    crate::hover_track::list("theme-menu")
                        .row(
                            div()
                                .id(("theme", ix))
                                .h(px(30.))
                                .px(px(7.))
                                .rounded(px(crate::theme::RADIUS_MD))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(13.))
                                .text_color(content(0.92))
                                .when(highlighted, |d| d.bg(content(0.1)))
                                .cursor_pointer()
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.pick_theme(ix, cx)),
                                )
                                .child(swatch(ix))
                                .child(crate::i18n::t(preset.name)),
                            ix,
                            highlighted,
                        )
                        .into_any_element()
                })
                .collect();
            gpui::deferred(
                div()
                    .id("theme-menu")
                    .occlude()
                    .absolute()
                    .right_0()
                    .top(px(36. + (1. - t) * -6.))
                    .w(px(280.))
                    .max_h(px(380.))
                    .p(px(5.))
                    .opacity(t)
                    .rounded(px(crate::theme::RADIUS_LG))
                    .border_1()
                    .border_color(content(0.12))
                    .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .h(px(32.))
                            .mb(px(3.))
                            .px(px(8.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .border_1()
                            .border_color(content(0.1))
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .text_size(px(13.))
                            .child(icon("search", 14.).text_color(content(0.45)))
                            .child(typed_text(
                                &menu.query,
                                crate::i18n::t("Buscar"),
                                div().flex_shrink_0().w(px(1.5)).h(px(15.)).bg(accent()),
                            )),
                    )
                    .child(crate::hover_track::list("theme-menu").pill())
                    .children(rows),
            )
            .with_priority(4)
        });
        let themes = div()
            .relative()
            .child(
                div()
                    .id("theme-button")
                    .w(px(220.))
                    .h(px(32.))
                    .px(px(8.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .border_1()
                    .border_color(content(if self.theme_menu.is_some() { 0.22 } else { 0.1 }))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(13.))
                    .text_color(content(0.95))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.theme_menu = match this.theme_menu {
                            Some(_) => None,
                            None => Some(PaletteState {
                                files: false,
                                all: false,
                                search: Default::default(),
                                query: String::new(),
                                selected: 0,
                                open: Tween::from_to(
                                    0.,
                                    1.,
                                    Instant::now(),
                                    if cx.reduce_motion() { 0. } else { MENU },
                                    Curve::Expo,
                                ),
                            }),
                        };
                        cx.notify();
                    }))
                    .child(swatch(look.preset))
                    .child(div().flex_1().child(t(PRESETS[look.preset].name)))
                    .child(
                        icon("chevron-right", 12.)
                            .text_color(content(0.5))
                            .with_transformation(Transformation::rotate(percentage(
                                if self.theme_menu.is_some() {
                                    0.75
                                } else {
                                    0.25
                                },
                            ))),
                    ),
            )
            .children(menu);
        let shown_accent = palette(&look, true).accent.to_rgb();
        let accents = div()
            .flex()
            .gap(px(2.))
            .children(ACCENTS.iter().enumerate().map(|(ix, (_, color))| {
                let fill: gpui::Hsla = rgb(*color).into();
                let f = fill.to_rgb();
                let chosen = look.custom.is_none()
                    && (f.r - shown_accent.r).abs()
                        + (f.g - shown_accent.g).abs()
                        + (f.b - shown_accent.b).abs()
                        < 0.01;
                div()
                    .id(("accent", ix))
                    .size(px(24.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(chosen, |d| d.border_2().border_color(fill))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_look(
                            |l| {
                                l.accent = Some(ix);
                                l.custom = None;
                            },
                            cx,
                        )
                    }))
                    .child(div().size(px(14.)).rounded_full().bg(fill))
            }))
            // A color of one's own: the rainbow swatch opens its two sliders.
            .child(
                div()
                    .id("accent-custom")
                    .size(px(24.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(look.custom.is_some(), |d| {
                        d.border_2().border_color(accent())
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_look(
                            |l| {
                                if l.custom.is_none() {
                                    l.custom = Some((330., 0.62));
                                }
                            },
                            cx,
                        )
                    }))
                    .child(div().size(px(14.)).rounded_full().bg(linear_gradient(
                        135.,
                        linear_color_stop(rgb(0xf0556d), 0.),
                        linear_color_stop(rgb(0x5a7bf5), 1.),
                    ))),
            );
        let saturation = match look.saturation {
            None => 0,
            Some(s) if s < 0.01 => 1,
            Some(s) if s < 0.2 => 2,
            _ => 3,
        };
        let hues = div().flex().gap(px(2.)).children((0..10usize).map(|ix| {
            let hue = ix as f32 * 36.;
            let chosen = look.hue == Some(hue);
            let fill = gpui::hsla(hue / 360., 0.6, 0.55, 1.);
            div()
                .id(("hue", ix))
                .size(px(22.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .when(chosen, |d| d.border_2().border_color(fill))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_look(
                        |l| l.hue = if l.hue == Some(hue) { None } else { Some(hue) },
                        cx,
                    )
                }))
                .child(div().size(px(12.)).rounded_full().bg(fill))
        }));
        let mut colors = vec![
            row(
                t("Tema do workspace"),
                t("Uma predefinição de cores."),
                themes.into_any_element(),
            ),
            row(
                t("Cor de destaque"),
                t("No botão de enviar, nos seus balões e na guia ativa."),
                accents.into_any_element(),
            ),
        ];
        if let Some((hue, light)) = look.custom {
            let hue_stops: Vec<gpui::Hsla> = (0..24)
                .map(|i| gpui::hsla(i as f32 / 24., 0.75, 0.6, 1.))
                .collect();
            let light_stops: Vec<gpui::Hsla> = (0..12)
                .map(|i| gpui::hsla(hue / 360., 0.75, 0.35 + i as f32 / 11. * 0.5, 1.))
                .collect();
            colors.push(row(
                t("Matiz da cor"),
                t("Arraste para escolher a sua cor."),
                self.slider(0, hue / 360., hue_stops, cx),
            ));
            colors.push(row(
                t("Luminosidade da cor"),
                t("Mais escura ou mais clara."),
                self.slider(1, (light - 0.35) / 0.5, light_stops, cx),
            ));
        }
        colors.push(row(
            t("Cores do diff"),
            t("Daltônico e alto contraste usam azul e laranja."),
            segmented(
                "diff",
                &[t("Padrão"), t("Daltônico"), t("Alto contraste")],
                look.diff,
                cx,
                |l, ix| l.diff = ix,
            ),
        ));
        colors.push(row(
            t("Matiz do fundo"),
            t("Clique de novo para voltar ao do tema."),
            hues.into_any_element(),
        ));
        colors.push(row(
            t("Saturação do fundo"),
            t("Quanto da cor aparece no fundo."),
            segmented(
                "saturation",
                &[t("Do tema"), t("Sem"), t("Suave"), t("Forte")],
                saturation,
                cx,
                |l, ix| l.saturation = [None, Some(0.), Some(0.12), Some(0.35)][ix],
            ),
        ));
        let later = |title: &'static str, about: &'static str| {
            row(
                title,
                about,
                div()
                    .text_size(px(12.))
                    .text_color(content(0.35))
                    .child(t("Em breve"))
                    .into_any_element(),
            )
        };
        div()
            .child(crate::settings_pages::about_line(t(
                "Personalize materiais e movimento.",
            )))
            .child(
                div()
                    .mb(px(32.))
                    .child(
                        div()
                            .mb(px(8.))
                            .text_size(px(12.5))
                            .text_color(content(0.5))
                            .child(t("Tema")),
                    )
                    .child(modes),
            )
            .child(group(t("Cores"), colors))
            .child(group(
                t("Materiais"),
                vec![row(
                    t("Translucidez"),
                    t("Deixa o fundo da janela aparecer através do app."),
                    segmented(
                        "glass",
                        &[t("Ligada"), t("Desligada")],
                        if look.glass { 0 } else { 1 },
                        cx,
                        |l, ix| l.glass = ix == 0,
                    ),
                )],
            ))
            .child(group(
                t("Interface"),
                vec![row(
                    t("Tamanho da interface"),
                    t("Aumenta ou diminui tudo."),
                    segmented(
                        "scale",
                        &["90%", "100%", "110%", "125%"],
                        [0.9f32, 1., 1.1, 1.25]
                            .iter()
                            .position(|s| (*s - look.scale).abs() < 0.01)
                            .unwrap_or(1),
                        cx,
                        |l, ix| l.scale = [0.9, 1., 1.1, 1.25][ix],
                    ),
                )],
            ))
            .children(crate::settings_pages::appearance_groups(
                &self.settings,
                &self.settings_data,
                self.setting_edit.as_ref().map(|(k, v)| (*k, v.as_str())),
                Shell::save_setting,
                Shell::edit_setting,
                Shell::run_setting,
                cx,
            ))
            .into_any_element()
    }

    /// A published visual: native programs render here; older HTML ones open in the browser.
    fn visual_element(
        &self,
        turn: usize,
        position: usize,
        visual: &data::Visual,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(source) = &visual.source {
            let view = self
                .visual_views
                .borrow_mut()
                .entry(visual.id.clone())
                .or_insert_with(|| crate::visuals::view(visual.title.clone(), source.clone(), cx))
                .clone();
            return div().w_full().child(view).into_any_element();
        }
        let (id, html) = (visual.id.clone(), visual.html.clone().unwrap_or_default());
        div()
            .id(("visual-html", turn * 100 + position))
            .h(px(34.))
            .px(px(10.))
            .rounded(px(crate::theme::RADIUS_MD))
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.03))
            .hover(|s| s.bg(content(0.07)))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(13.))
            .text_color(content(0.85))
            .on_click(cx.listener(move |_, _, _, cx| {
                let path = std::env::temp_dir().join(format!("sirus-visual-{id}.html"));
                if std::fs::write(&path, &html).is_ok() {
                    cx.open_url(&format!("file://{}", path.display()));
                }
            }))
            .child(icon("globe", 14.).text_color(content(0.55)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(visual.title.clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .child(t("Abrir")),
            )
            .into_any_element()
    }

    fn roll(&self, conversation: usize, key: usize, now: Instant, moving: &mut bool) -> f32 {
        let Some(tween) = self.rolls.get(&(conversation, key)) else {
            return 0.;
        };
        if !tween.done(now) {
            *moving = true;
        }
        tween.at(now)
    }

    fn set_roll(&mut self, conversation: usize, key: usize, hovered: bool, cx: &mut Context<Self>) {
        let now = Instant::now();
        let duration = if cx.reduce_motion() { 0. } else { MORPH };
        let mut tween = self
            .rolls
            .get(&(conversation, key))
            .copied()
            .unwrap_or_else(|| Tween::still(0., now));
        tween.set(if hovered { 1. } else { 0. }, now, duration, Curve::Expo);
        self.rolls.insert((conversation, key), tween);
        cx.notify();
    }

    fn spin(&self, now: Instant) -> Div {
        div()
            .size(px(13.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(spinner_at(
                (now.duration_since(self.epoch).as_secs_f32() / SPINNER_PERIOD).fract(),
            ))
    }

    fn menu_view(
        &mut self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let tab = menu.tab;
        let pinned = self.tabs.iter().find(|t| t.id == tab)?.pinned;
        *moving |= !menu.open.done(now);
        let open = menu.open.at(now);
        let at = self.tabs.iter().position(|t| t.id == tab)?;
        let closable = |ix: usize, t: &Tab| t.id != tab && !t.pinned && !t.closing && ix != at;
        let has_others = self.tabs.iter().enumerate().any(|(ix, t)| closable(ix, t));
        let has_right = self
            .tabs
            .iter()
            .enumerate()
            .any(|(ix, t)| closable(ix, t) && ix > at);
        let has_left = self
            .tabs
            .iter()
            .enumerate()
            .any(|(ix, t)| closable(ix, t) && ix < at);
        let separator = || div().h(px(1.)).mx(px(4.)).my(px(4.)).bg(content(0.08));
        let item = |id: &'static str,
                    glyph: &'static str,
                    label: &'static str,
                    shortcut: &'static str,
                    enabled: bool,
                    danger: bool,
                    cx: &mut Context<Self>,
                    action: fn(&mut Self, u64, &mut Context<Self>)| {
            let row = div()
                .id(id)
                .h(px(26.))
                .px(px(8.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(9.))
                .text_size(px(13.))
                .text_color(if danger {
                    rgb(0xe08a8a).into()
                } else {
                    content(if enabled { 0.9 } else { 0.3 })
                })
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            action(this, tab, cx);
                            cx.notify();
                        }))
                })
                .child(icon(glyph, 14.).text_color(content(if enabled { 0.65 } else { 0.25 })))
                .child(div().flex_1().child(label))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.4))
                        .child(shortcut),
                );
            if enabled {
                crate::hover_track::list("tab-menu").row_named(row, id, false)
            } else {
                row
            }
        };
        Some(
            div()
                .id("tab-menu")
                .occlude()
                .absolute()
                .left(menu.at.x)
                .top(menu.at.y + px(2. + (1. - open) * -4.))
                .w(px(240.))
                .p(px(4.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_MD))
                .shadow_lg()
                .opacity(open.clamp(0., 1.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(crate::hover_track::list("tab-menu").pill())
                .child(item(
                    "menu-pin",
                    "pin",
                    if pinned {
                        t("Desafixar aba")
                    } else {
                        t("Fixar aba")
                    },
                    "⇧⌘P",
                    true,
                    false,
                    cx,
                    |this, id, cx| this.toggle_pin(id, cx),
                ))
                .child(separator())
                .child(item(
                    "menu-close",
                    "tab-close",
                    t("Fechar aba"),
                    "⌘W",
                    true,
                    false,
                    cx,
                    |this, id, cx| this.close(id, cx),
                ))
                .child(separator())
                .child(item(
                    "menu-others",
                    "tabs-others",
                    t("Fechar outras abas"),
                    "",
                    has_others,
                    false,
                    cx,
                    |this, id, cx| this.close_where(id, |_, _| true, cx),
                ))
                .child(item(
                    "menu-right",
                    "tabs-right",
                    t("Fechar abas à direita"),
                    "",
                    has_right,
                    false,
                    cx,
                    |this, id, cx| this.close_where(id, |ix, at| ix > at, cx),
                ))
                .child(item(
                    "menu-left",
                    "tabs-left",
                    t("Fechar abas à esquerda"),
                    "",
                    has_left,
                    false,
                    cx,
                    |this, id, cx| this.close_where(id, |ix, at| ix < at, cx),
                ))
                .child(item(
                    "menu-all",
                    "tabs-all",
                    t("Fechar todas as abas"),
                    "⇧⌘W",
                    has_others,
                    false,
                    cx,
                    |this, id, cx| this.close_where(id, |_, _| true, cx),
                ))
                .into_any_element(),
        )
    }

    fn stats(&mut self, now: Instant) -> Option<AnyElement> {
        if !self.show_stats {
            return None;
        }
        let deltas: Vec<f32> = self
            .frames
            .iter()
            .zip(self.frames.iter().skip(1))
            .map(|(a, b)| b.duration_since(*a).as_secs_f32() * 1000.)
            .filter(|d| *d < 250.)
            .collect();
        let _ = now;
        let average = deltas.iter().sum::<f32>() / deltas.len().max(1) as f32;
        let worst = deltas.iter().copied().fold(0., f32::max);
        Some(
            div()
                .absolute()
                .right(px(14.))
                .bottom(px(14.))
                .px(px(8.))
                .py(px(4.))
                .rounded(px(crate::theme::RADIUS_MD))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_SM))
                .text_size(px(11.5))
                .text_color(content(0.75))
                .child(tf(
                    "{} fps · pior quadro {} ms",
                    &[
                        &format!("{:.0}", if average > 0. { 1000. / average } else { 0. }),
                        &format!("{:.1}", worst),
                    ],
                ))
                .into_any_element(),
        )
    }
}

fn running(conversation: &Conversation, _now: Instant) -> bool {
    conversation.working()
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn error_text(value: &Value) -> String {
    value["message"]
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

/// A model id as a person reads it: `claude-opus-5-5` is Opus 5.5, `gpt-5.5` is GPT-5.5.
/// What Claude Code's aliases stand for today. Its catalog lists only the aliases, so the versions
/// come from here; a new Claude Code release can move an alias before this table follows.
const CLAUDE_ALIASES: [(&str, &str); 4] = [
    ("fable", "Fable 5.1"),
    ("opus", "Opus 5.5"),
    ("sonnet", "Sonnet 5.5"),
    ("haiku", "Haiku 5.5"),
];

/// Earlier models Claude Code still takes by full name (`--model`), each with its effort levels.
const CLAUDE_EARLIER: [(&str, &str, &[&str]); 8] = [
    (
        "claude-fable-5",
        "Fable 5",
        &["low", "medium", "high", "xhigh", "max"],
    ),
    (
        "claude-opus-5",
        "Opus 5",
        &["low", "medium", "high", "xhigh", "max"],
    ),
    (
        "claude-opus-4-8",
        "Opus 4.8",
        &["low", "medium", "high", "xhigh", "max"],
    ),
    (
        "claude-opus-4-7",
        "Opus 4.7",
        &["low", "medium", "high", "xhigh", "max"],
    ),
    (
        "claude-opus-4-6",
        "Opus 4.6",
        &["low", "medium", "high", "max"],
    ),
    (
        "claude-sonnet-5",
        "Sonnet 5",
        &["low", "medium", "high", "xhigh", "max"],
    ),
    (
        "claude-sonnet-4-6",
        "Sonnet 4.6",
        &["low", "medium", "high", "max"],
    ),
    ("claude-haiku-4-5", "Haiku 4.5", &[]),
];

/// Claude's list as the picker shows it: versions on the names, the earlier models added, newest
/// of each family first. Being listed does not prove the account can use a model.
fn claude_catalog(mut models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    for model in &mut models {
        let family = model.name.to_lowercase();
        if let Some((_, name)) = CLAUDE_ALIASES.iter().find(|(alias, _)| *alias == family) {
            model.name = (*name).to_owned();
        }
    }
    for (id, name, efforts) in CLAUDE_EARLIER {
        if models.iter().all(|m| m.id != id) {
            models.push(ModelInfo {
                id: id.to_owned(),
                name: name.to_owned(),
                efforts: efforts.iter().map(|e| (*e).to_owned()).collect(),
                default_effort: None,
                fast: false,
                default_fast: false,
                available: true,
                parameterized: false,
            });
        }
    }
    let rank = |name: &str| {
        let order = ["Fable", "Opus", "Sonnet", "Haiku"];
        let family = order
            .iter()
            .position(|f| name.starts_with(f))
            .unwrap_or(order.len());
        let version: Vec<u32> = name
            .split_whitespace()
            .nth(1)
            .unwrap_or("0")
            .split('.')
            .filter_map(|part| part.parse().ok())
            .collect();
        (family, std::cmp::Reverse(version))
    };
    models.sort_by_cached_key(|m| rank(&m.name));
    models
}

/// A settings page's name in the menu, where the full title does not fit.
fn nav_label(page: &'static str) -> &'static str {
    if page == "Comportamento do chat" {
        "Comportamento"
    } else {
        page
    }
}

fn model_label(model: Option<&str>, provider: &str) -> String {
    let Some(model) = model.filter(|m| !m.is_empty()) else {
        return model_name(provider).to_owned();
    };
    if let Some((_, name)) = CLAUDE_ALIASES.iter().find(|(alias, _)| *alias == model) {
        return (*name).to_owned();
    }
    let trimmed = model.strip_prefix("claude-").unwrap_or(model);
    if let Some(rest) = trimmed.strip_prefix("gpt") {
        return format!("GPT{rest}");
    }
    let mut parts = trimmed.split('-');
    let name = parts.next().unwrap_or(trimmed);
    let version: Vec<&str> = parts
        .filter(|p| p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit()))
        .collect();
    let mut chars = name.chars();
    let title: String = chars
        .next()
        .map_or(String::new(), |c| c.to_uppercase().chain(chars).collect());
    if version.is_empty() {
        title
    } else {
        format!("{title} {}", version.join("."))
    }
}

fn model_name(provider: &str) -> &'static str {
    match provider {
        "claude" => "Opus 5.5",
        "codex" => "GPT-5.5",
        "opencode" => "OpenCode",
        _ => t("Agente"),
    }
}

/// The live phase's header, as Monocode's: "Running 11 commands · Explored the project".
fn live_summary(steps: &[data::Step]) -> String {
    let count = |kind: &str| steps.iter().filter(|s| s.kind == kind).count();
    let (commands, reads, edits) = (count("command"), count("read"), count("edit"));
    let mut parts = Vec::new();
    if commands > 0 {
        let noun = if commands == 1 { "command" } else { "commands" };
        parts.push(format!("Running {commands} {noun}"));
    }
    if reads > 0 {
        parts.push("Explored the project".to_owned());
    }
    if edits > 0 {
        let noun = if edits == 1 { "file" } else { "files" };
        parts.push(format!("Edited {edits} {noun}"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod live_header_tests {
    use super::*;

    fn step(kind: &str) -> data::Step {
        data::Step {
            kind: kind.into(),
            label: String::new(),
            detail: String::new(),
            running: false,
            image: None,
            capture: false,
            tool: None,
            target: None,
            caption: String::new(),
        }
    }

    #[test]
    fn the_live_header_counts_commands_and_reads_as_monocode_does() {
        let steps: Vec<_> = (0..11)
            .map(|_| step("command"))
            .chain([step("read")])
            .collect();
        assert_eq!(
            live_summary(&steps),
            "Running 11 commands · Explored the project"
        );
        assert_eq!(live_summary(&[step("command")]), "Running 1 command");
        assert_eq!(live_summary(&[]), "");
    }
}

/// Wrapped lines of a user message at the bubble's text width, shaped as the bubble draws it.
fn prompt_lines_over(text: &str, wrap: f32, window: &Window) -> usize {
    let text = text.trim_end();
    let run = gpui::TextRun {
        len: text.len(),
        font: gpui::font(".SystemUIFont"),
        color: Default::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_text(
            SharedString::from(text.to_owned()),
            px(14.),
            &[run],
            Some(px(wrap)),
            None,
        )
        .unwrap_or_default()
        .iter()
        .map(|line| line.wrap_boundaries.len() + 1)
        .sum()
}

/// "Executou 2 comandos · Leu 3 arquivos", from a finished turn's steps.
fn summary(steps: &[data::Step]) -> String {
    let count = |kind: &str| steps.iter().filter(|s| s.kind == kind).count();
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let mut parts = Vec::new();
    for (kind, verb, one, many) in [
        ("command", t("Executou"), t("comando"), t("comandos")),
        ("read", t("Leu"), t("arquivo"), t("arquivos")),
        ("edit", t("Editou"), t("arquivo"), t("arquivos")),
        ("tool", t("Usou"), t("ferramenta"), t("ferramentas")),
        ("agent", t("Iniciou"), t("agente"), t("agentes")),
        ("skill", t("Usou"), "skill", "skills"),
    ] {
        let n = count(kind);
        if n > 0 {
            parts.push(format!("{verb} {}", plural(n, one, many)));
        }
    }
    parts.join(" · ")
}

fn step_line(step: &data::Step) -> crate::step_lines::Line {
    crate::step_lines::line(
        &step.label,
        &step.detail,
        step.tool.as_deref(),
        step.target.as_deref(),
    )
}

/// What a tool acted on, in the tree: a small mono chip.
/// A prompt's text: four lines when folded, whole when not, and in between while it slides.
fn prompt_frame(motion: Option<f32>, width: Pixels, collapsed: bool, text: AnyElement) -> AnyElement {
    match motion {
        Some(progress) if width > px(0.) => div()
            .overflow_hidden()
            .child(crate::disclosure::Disclosure::from_height(
                text,
                width,
                px(80.),
                progress,
            ))
            .into_any_element(),
        _ => div()
            .when(collapsed, |d| d.max_h(px(80.)).overflow_hidden())
            .child(text)
            .into_any_element(),
    }
}

/// How far below the top of the view a sent message comes to rest.
const ANCHOR_TOP: f32 = 32.;

/// The first fold key among `phase_open`'s (conversation, turn, key); below it are step indexes.
const FOLD_KEY: usize = 1 << 40;

/// A project with no Git is normal: a Git call failing on it is not a problem to report.
fn is_not_a_repository(message: &str) -> bool {
    message.contains("not a git repository")
}

/// How far the composer's text should be scrolled so the caret's line shows: unchanged when it
/// already does, else just enough to bring it in.
fn follow_caret(top: Pixels, line: Pixels, scrolled: Pixels, seen: Pixels) -> Pixels {
    if top < scrolled {
        top
    } else if top + line > scrolled + seen {
        top + line - seen
    } else {
        scrolled
    }
}

fn target_chip(target: String) -> Div {
    div()
        .min_w_0()
        .truncate()
        .px(px(5.))
        .rounded(px(4.))
        .bg(content(0.06))
        .font_family("Menlo")
        .text_size(px(11.5))
        .text_color(content(0.7))
        .child(target)
}

/// A command's path chip: the file or folder icon and its name.
fn path_chip(name: String, folder: bool) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(5.))
        .h(px(22.))
        .px(px(7.))
        .rounded(px(6.))
        .bg(content(0.06))
        .font_family("Menlo")
        .text_size(px(11.5))
        .text_color(content(0.7))
        .child(crate::file_icons::file_icon(&name, folder, 13.))
        .child(name)
}

/// Rows of a command with a caption are taller: a title and its chips (tree "A").
fn step_height(step: &data::Step) -> f32 {
    if step.capture {
        28.
    } else if step.kind == "command" {
        30.
    } else {
        22.
    }
}

/// A finished row of the opened tree: a quiet icon, the verb and what it acted on. A command
/// reads as what the model said it was for, with its program and the files it names as chips.
fn step_row(step: &data::Step) -> Div {
    if step.kind == "command" {
        let program = crate::step_lines::command_label(&step.detail);
        let title = if step.caption.is_empty() {
            None
        } else {
            Some(step.caption.clone())
        };
        return div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(8.))
            .when_some(title, |d, title| {
                d.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(14.))
                        .text_color(content(0.5))
                        .child(title),
                )
            })
            .when(!program.is_empty(), |d| d.child(target_chip(program)))
            .children(
                crate::step_lines::command_paths(&step.detail, 2)
                    .into_iter()
                    .map(|(name, folder)| path_chip(name, folder)),
            );
    }
    let line = step_line(step);
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(px(7.))
        .child(icon(line.icon, 14.).text_color(content(0.42)))
        .when(!line.verb.is_empty(), |d| {
            d.child(div().flex_shrink_0().child(crate::i18n::t(line.verb)))
        })
        .when_some(line.target, |d, target| d.child(target_chip(target)))
}

fn step_glyph(step: &data::Step) -> &'static str {
    match step.kind.as_str() {
        "command" => "terminal",
        "edit" => "pencil-line",
        "read" => "search",
        "agent" => "bot",
        _ => "bot",
    }
}

/// A top-bar button whose panel is open keeps its fill and brighter icon.
fn toggled(button: Stateful<Div>, on: bool) -> Stateful<Div> {
    button.when(on, |d| d.bg(content(0.1)))
}

/// Rounds what a card paints at its corners: GPUI clips to rectangles, so the renderer masks
/// them (Monocode's layout.rs, MIT), and the chrome's wash fills outside the curve.
fn card_corner_mask(begin: bool) -> impl IntoElement {
    const RADIUS: f32 = crate::theme::RADIUS_LG;
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let r = px(RADIUS)
                .min(bounds.size.width / 2.)
                .min(bounds.size.height / 2.);
            let corners = [
                bounds.origin,
                gpui::point(bounds.right() - r, bounds.top()),
                gpui::point(bounds.right() - r, bounds.bottom() - r),
                gpui::point(bounds.left(), bounds.bottom() - r),
            ];
            for (corner, origin) in corners.into_iter().enumerate() {
                window.paint_quad(
                    gpui::fill(Bounds::new(origin, gpui::size(r, r)), gpui::black()).border_color(
                        gpui_apple::metal_renderer::corner_clip_marker(begin, corner as u8),
                    ),
                );
            }
            if !begin {
                let vertices = [
                    (
                        bounds.origin,
                        gpui::point(bounds.left() + r, bounds.top()),
                        gpui::point(bounds.left(), bounds.top() + r),
                    ),
                    (
                        gpui::point(bounds.right(), bounds.top()),
                        gpui::point(bounds.right(), bounds.top() + r),
                        gpui::point(bounds.right() - r, bounds.top()),
                    ),
                    (
                        gpui::point(bounds.right(), bounds.bottom()),
                        gpui::point(bounds.right() - r, bounds.bottom()),
                        gpui::point(bounds.right(), bounds.bottom() - r),
                    ),
                    (
                        gpui::point(bounds.left(), bounds.bottom()),
                        gpui::point(bounds.left(), bounds.bottom() - r),
                        gpui::point(bounds.left() + r, bounds.bottom()),
                    ),
                ];
                for (corner, start, end) in vertices {
                    let mut path = gpui::PathBuilder::fill();
                    path.move_to(corner);
                    path.line_to(start);
                    path.arc_to(gpui::point(r, r), px(0.), false, false, end);
                    path.close();
                    // Over glass the renderer already restored the backdrop outside the curve; a
                    // fill here would darken the corner.
                    if pane_fill().a >= 1.
                        && let Ok(path) = path.build()
                    {
                        window.paint_path(path, shell_fill().blend(chrome_wash()));
                    }
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn chevron_down() -> gpui::Svg {
    icon("chevron-right", 11.)
        .text_color(content(0.4))
        .with_transformation(Transformation::rotate(percentage(0.25)))
}

fn nav_button(
    id: &'static str,
    left: f32,
    turn: f32,
    enabled: bool,
    cx: &mut Context<Shell>,
    forward: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .absolute()
        .left(px(left))
        .top(px(8.))
        .size(px(24.))
        .rounded(px(crate::theme::RADIUS_SM))
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |d| {
            d.hover(|s| s.bg(content(0.07)))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.navigate(forward, cx)))
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            icon("chevron-right", 15.)
                .text_color(content(if enabled { 0.6 } else { 0.25 }))
                .with_transformation(Transformation::rotate(percentage(turn))),
        )
}

fn icon(name: &'static str, side: f32) -> gpui::Svg {
    theme_icon(name, side).text_color(content(0.8))
}

fn icon_button(id: &'static str, name: &'static str, side: f32) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(PLATE - 3.))
        .rounded(px(crate::theme::RADIUS_MD))
        .flex()
        .items_center()
        .justify_center()
        .hover(|s| s.bg(content(0.07)))
        .cursor_pointer()
        .child(icon(name, side).text_color(content(0.6)))
}

/// The byte index of a UTF-16 offset.
fn byte_at(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, c) in text.char_indices() {
        if units >= utf16 {
            return byte;
        }
        units += c.len_utf16();
    }
    text.len()
}

impl EntityInputHandler for Shell {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = match self.query_target() {
            Some(query) => query.clone(),
            None => self.draft().text.clone(),
        };
        let (start, end) = (byte_at(&text, range.start), byte_at(&text, range.end));
        *adjusted = Some(range);
        Some(text[start..end].to_owned())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if let Some(query) = self.query_target() {
            let end = data::utf16_len(query);
            return Some(UTF16Selection {
                range: end..end,
                reversed: false,
            });
        }
        let draft = self.draft();
        let range = draft.selection();
        Some(UTF16Selection {
            range: data::utf16_len(&draft.text[..range.start])
                ..data::utf16_len(&draft.text[..range.end]),
            reversed: draft.cursor < draft.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone()
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.first_run.active() {
            return;
        }
        let marked = self.marked.take();
        self.mention_selected = 0;
        self.mention_dismissed = None;
        if let Some(query) = self.query_target() {
            let end = data::utf16_len(query);
            let range = range.or(marked).unwrap_or(end..end);
            let (start, stop) = (byte_at(query, range.start), byte_at(query, range.end));
            query.replace_range(start..stop, text);
        } else {
            let draft = self.draft();
            if let Some(range) = range.or(marked) {
                draft.anchor = byte_at(&draft.text, range.start);
                draft.cursor = byte_at(&draft.text, range.end);
            }
            draft.insert(text);
        }
        self.palette_query_changed(cx);
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.first_run.active() {
            return;
        }
        let marked = self.marked.take();
        let start_utf16;
        if let Some(query) = self.query_target() {
            let end = data::utf16_len(query);
            let range = range.or(marked).unwrap_or(end..end);
            start_utf16 = range.start;
            let (start, stop) = (byte_at(query, range.start), byte_at(query, range.end));
            query.replace_range(start..stop, new_text);
        } else {
            let draft = self.draft();
            if let Some(range) = range.or(marked) {
                draft.anchor = byte_at(&draft.text, range.start);
                draft.cursor = byte_at(&draft.text, range.end);
            }
            start_utf16 = data::utf16_len(&draft.text[..draft.selection().start]);
            draft.insert(new_text);
        }
        self.marked =
            (!new_text.is_empty()).then(|| start_utf16..start_utf16 + data::utf16_len(new_text));
        self.palette_query_changed(cx);
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        self.select.begin_frame();
        self.ensure_draft_timer(cx);
        let reduced = cx.reduce_motion();
        let size = window.viewport_size();
        self.window_active = window.is_window_active();
        self.cards_seen();
        // The header's incoming check follows the selected conversation, its folder and its turns.
        if !self.conversations.is_empty() {
            let conversation = &self.conversations[self.active_conversation()];
            let (session, folder, working) = (
                conversation.session_id.clone(),
                conversation.worktree.clone(),
                conversation.working(),
            );
            if self
                .incoming
                .observe(&session, &folder, working, self.window_active)
            {
                self.incoming_fetch();
            }
        }
        if let Some(field) = crate::settings_connectors::take_autofocus() {
            self.setting_edit = Some(field);
        }
        if let Some(kind) = self
            .dock_active
            .filter(|_| self.dock_open && !self.conversations.is_empty())
        {
            self.ensure_pane_view(kind, cx);
        }
        let mut moving = !self.palette_t.done(now);
        self.system_dark = matches!(
            window.appearance(),
            gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
        );
        crate::theme::apply(mix(
            self.palette_from,
            palette(&self.look, self.system_dark),
            self.palette_t.at(now),
        ));
        // The interface size applies after this frame, so layout never sees it change mid-draw.
        if (window.content_zoom() - self.look.scale).abs() > f32::EPSILON {
            let scale = self.look.scale;
            window.defer(cx, move |window, cx| window.set_content_zoom(scale, cx));
        }
        self.frames.push_back(now);
        while self.frames.len() > 120 {
            self.frames.pop_front();
        }
        let titlebar = self.titlebar(f32::from(size.width), now, &mut moving, reduced, cx);
        let docked = self.sidebar_open || self.sidebar.at(now) > 0.001;
        let sidebar = self.sidebar_view(now, &mut moving, reduced, cx);
        let peek_blur = self
            .peek
            .as_ref()
            .filter(|_| !docked)
            .map(|peek| peek.t.at(now).clamp(0., 1.));
        let (sidebar, peek_panel) = if docked {
            (Some(sidebar), None)
        } else {
            (None, self.peek.is_some().then_some(sidebar))
        };
        let transcript = self.split_view(now, &mut moving, cx);
        // Settings → Aparência → Fundo do chat: under the card, clipped by its corner masks.
        let empty_chat = self.conversations[self.active_conversation()]
            .turns
            .is_empty();
        let chat_background = crate::chat_background::layer(&self.settings, empty_chat, window, cx);
        let menu = self.menu_view(now, &mut moving, cx);
        let open = self.sidebar.at(now);
        let footer = self.footer(self.sidebar_width * open, cx);
        let panel = self.appearance_view(now, &mut moving, cx);
        self.usage_settle(now);
        let usage_page = self.usage_page_view(now, &mut moving, window, cx);
        let settings_cover = self
            .appearance
            .as_ref()
            .map_or(0., |open| open.at(now).clamp(0., 1.));
        self.viewport_width = f32::from(size.width);
        self.viewport_height = f32::from(size.height);
        self.dock_drawn = self.dock.at(now) * self.dock_width;
        crate::panel_browser::set_covered(
            self.appearance.is_some()
                || self.usage_page_open()
                || self.palette_ui.is_some()
                || self.environment.is_some()
                || self.menu.is_some()
                || self.peek.is_some()
                || self.project_menu.is_some()
                || self.confirm.is_some()
                || self.first_run.active()
                || self.viewer.visible()
                || self.cards_visible(),
        );
        let composer_menu = self.composer_menu_view(now, &mut moving, cx);
        let usage_popup = self.usage_popup_view(now, &mut moving, cx);
        let resources_popup = self.resources_popup_view(now, &mut moving, cx);
        let context_popup = self.context_popup_view(now, &mut moving, cx);
        let sidebar_card = self.sidebar_card_view(now, &mut moving, cx);
        let activity_cards = self.cards_view(cx);
        let project_menu = self.project_menu_view(now, &mut moving, cx);
        let filter_menu = self.filter_menu_view(now, &mut moving, cx);
        let sidebar_options = self.sidebar_options_view(now, &mut moving, cx);
        let context_menu = self.context_menu_view(now, &mut moving, cx);
        let select_menu = self.select_menu_view(now, &mut moving, cx);
        let handoff_popup = self.handoff_popup_view(now, &mut moving, cx);
        let more_menu = self.more_menu_view(now, &mut moving, cx);
        let confirm_dialog = self.confirm_view(now, &mut moving, cx);
        let term_plus_menu = self.term_plus_menu_view(now, &mut moving, cx);
        let title_tab = self.title_tab_view();
        let bottom_terminal = self.bottom_view();
        let attachment_modal = self.attachment_view(now, &mut moving, cx);
        let trail_card = self.trail_card();
        let add_menu = self.add_menu_view(now, &mut moving, cx);
        let environment = self.environment_view(now, &mut moving, cx);
        let incoming_card = {
            let p = match &self.incoming.tween {
                Some(tween) => {
                    moving |= !tween.done(now);
                    tween.at(now).clamp(0., 1.)
                }
                None => 1.,
            };
            git_sync::popover(
                &self.incoming,
                TITLE_BAR,
                p,
                cx.listener(|this, _, _, cx| this.incoming_pull(cx)),
            )
        };
        // Watches the pointer over the whole window, even above opaque panels.
        let view = cx.entity();
        let height = f32::from(size.height);
        // Typed text, accents and IME composition arrive through this, wherever the focus is.
        let input_view = cx.entity();
        let input_focus = self.focus.clone();
        let input_area = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                window.handle_input(
                    &input_focus,
                    ElementInputHandler::new(bounds, input_view),
                    cx,
                );
            },
        )
        .absolute()
        .size_full();
        let tracker = gpui::canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        view.update(cx, |this, cx| {
                            if let Some(id) = this.slider_drag {
                                if event.pressed_button == Some(MouseButton::Left) {
                                    this.set_slider(id, f32::from(event.position.x), cx);
                                } else {
                                    this.slider_drag = None;
                                }
                            }
                            if this.selecting {
                                if event.pressed_button == Some(MouseButton::Left) {
                                    let index = this.composer_layout.borrow().as_ref().map(|l| {
                                        l.index_for_position(event.position).unwrap_or_else(|i| i)
                                    });
                                    if let Some(index) = index {
                                        let draft = this.draft();
                                        draft.cursor = index.min(draft.text.len());
                                        cx.notify();
                                    }
                                } else {
                                    this.selecting = false;
                                }
                            }
                            this.track_peek(event.position, height, cx);
                        });
                    }
                });
            },
        )
        .absolute()
        .size_0();
        let palette_view = self.palette_view(now, &mut moving, cx);
        let first_run = self.first_run_view(now, &mut moving, cx);
        if !self.dock.done(now) || !self.dock_max.done(now) {
            moving = true;
        }
        let dock_share = self.dock.at(now);
        let room = f32::from(size.width) - self.sidebar_width * self.sidebar.at(now) - 12.;
        let dock_full = self.dock_width + (room - self.dock_width) * self.dock_max.at(now);
        let dock_width = dock_full * dock_share;
        let dock = (dock_share > 0.001).then(|| self.dock_view(dock_full, now, &mut moving, cx));
        let stats = self.stats(now);
        moving |= self.send_moving.take();
        if moving {
            window.request_animation_frame();
        }
        div()
            .id("root")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(".SystemUIFont")
            .bg(shell_fill())
            .text_color(content(0.9))
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| this.attach_paths(paths.paths().to_vec(), cx)))
            .on_key_down(cx.listener(Self::key))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.select.clear_unless_claimed() {
                    cx.notify();
                }
                let closed = [this.incoming.close(), this.menu.take().is_some(), this.environment.take().is_some(),this.composer_menu.take().is_some(), this.add_menu.take().is_some(), this.usage_popup.take().is_some(), this.resources.close(), this.context.close(), this.close_project_menu(), this.session_filter.open.take().is_some(), this.context_menu.take().is_some(), this.term.close_menu(), this.close_sidebar_options(cx)];
                this.goal_input = None;
                if closed.contains(&true) {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .bg(chrome_wash())
                    .when(pane_fill().a < 0.5, |d| d.opacity(1. - settings_cover))
                    .when(pane_fill().a < 0.5 && settings_cover >= 1., |d| d.invisible())
                    .child(titlebar)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .children(sidebar)
                            // The card runs from the title bar to the footer, with a small gutter
                            // beside it; the closed sidebar leaves the same gutter on the left.
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .pr(px(6.))
                                    .pl(px(6. * (1. - open)))
                                    // One card: the conversation and the panel, split by a hairline.
                                    .child(
                                        div()
                                            .relative()
                                            .flex_1()
                                            .min_w_0()
                                            .h_full()
                                            .flex()
                                            .overflow_hidden()
                                            .child(card_corner_mask(true))
                                            .children(chat_background)
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .h_full()
                                                    .flex()
                                                    .flex_col()
                                                    .rounded(px(crate::theme::RADIUS_LG))
                                                    .border_1()
                                                    .border_color(content(0.08))
                                                    .bg(pane_fill())
                                                    .overflow_hidden()
                                                    .child(div().flex_1().min_h_0().w_full().flex()
                                                    .child(title_tab.unwrap_or(transcript))
                                                    .when_some(dock, |d, dock| {
                                                        d.child(
                                                            div().id("dock-resize").relative().flex_shrink_0().h_full().w(px(1.)).bg(content(0.08)).opacity(dock_share)
                                                                .child(div().absolute().top_0().bottom_0().left(px(-3.)).w(px(7.)).cursor_col_resize().on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                                                    cx.stop_propagation();
                                                                    this.resizing_dock = true;
                                                                }))),
                                                        )
                                                            .child(div().flex_shrink_0().h_full().w(px(dock_width)).overflow_hidden().child(dock))
                                                    }))
                                                    .children(bottom_terminal),
                                            )
                                            .child(card_corner_mask(false)),
                                    ),
                            ),
                    )
                    .child(footer),
            )
            .child(tracker)
            .child(crate::panel_browser::frame_end())
            .child(input_area)
            // Behind a peeking sidebar the window goes out of focus, as in Monocode.
            .children(peek_blur.filter(|t| *t > 0.001).map(|t| {
                gpui::canvas(|_, _, _| {}, move |bounds, _, window, _| {
                    window.paint_quad(gpui::fill(bounds, gpui::Hsla { a: 0.14 * t, ..gpui::black() }).border_color(gpui::backdrop_blur_marker(t * 14.)));
                })
                .absolute()
                .size_full()
            }))
            .children(peek_panel)
            .children(project_menu)
            .children(filter_menu)
            .children(sidebar_options)
            .children(context_menu)
            .children(select_menu)
            .children(handoff_popup)
            .children(more_menu)
            .children(trail_card)
            .children(attachment_modal)
            .children(confirm_dialog)
            .children(term_plus_menu)
            .children(menu)
            .children(composer_menu)
            .children(usage_popup)
            .children(resources_popup)
            .children(context_popup)
            .children(sidebar_card)
            .children(activity_cards)
            .children(add_menu)
            .children(environment)
            .children(incoming_card)
            .children(panel)
            .children(usage_page)
            .children(palette_view)
            .children(first_run)
            .children(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_close_keeps_pinned_tabs_and_the_kept_one() {
        // (id, pinned): tab 3 is kept, tabs 1 and 4 are pinned.
        let tabs = [(1, true), (2, false), (3, false), (4, true), (5, false)];
        assert_eq!(bulk_close_marks(&tabs, 3, |_, _| true), vec![2, 5]);
        assert_eq!(bulk_close_marks(&tabs, 3, |ix, at| ix > at), vec![5]);
        assert_eq!(bulk_close_marks(&tabs, 3, |ix, at| ix < at), vec![2]);
    }

    #[test]
    fn closing_a_tab_selects_the_right_neighbour_then_the_left() {
        assert_eq!(neighbour_on_close(3, 0), Some(1));
        assert_eq!(neighbour_on_close(3, 1), Some(2));
        assert_eq!(neighbour_on_close(3, 2), Some(1));
        assert_eq!(neighbour_on_close(2, 1), Some(0));
        assert_eq!(neighbour_on_close(1, 0), None);
    }

    #[test]
    fn bulk_close_marks_nothing_when_only_pinned_tabs_are_left() {
        let tabs = [(1, true), (2, true)];
        assert!(bulk_close_marks(&tabs, 1, |_, _| true).is_empty());
    }

    #[test]
    fn a_tab_is_dashed_only_for_unsent_text() {
        let mut drafts = HashMap::new();
        drafts.insert(
            0,
            Draft {
                text: "hello".into(),
                ..Default::default()
            },
        );
        drafts.insert(
            1,
            Draft {
                text: " \n".into(),
                ..Default::default()
            },
        );
        assert!(tab_has_draft(&drafts, 0));
        assert!(!tab_has_draft(&drafts, 1));
        assert!(!tab_has_draft(&drafts, 2));
    }

    #[test]
    fn draft_keys_are_sirus_codes_session_keys() {
        assert_eq!(draft_key("abc"), "session:abc");
    }

    #[test]
    fn a_saved_draft_is_restored_with_the_caret_at_its_end() {
        let draft = restored_draft("olá");
        assert_eq!(
            (draft.text.as_str(), draft.cursor, draft.anchor),
            ("olá", "olá".len(), "olá".len())
        );
    }

    #[test]
    fn restoring_fills_empty_composers_and_keeps_typed_text() {
        let sessions = vec![
            "a".to_owned(),
            "b".to_owned(),
            String::new(),
            "c".to_owned(),
        ];
        let saved = HashMap::from([
            ("session:a".to_owned(), "from host".to_owned()),
            ("session:b".to_owned(), "older".to_owned()),
        ]);
        let mut drafts = HashMap::new();
        drafts.insert(
            1,
            Draft {
                text: "typed".into(),
                cursor: 5,
                anchor: 5,
            },
        );
        restore_drafts(&mut drafts, &sessions, &saved);
        assert_eq!(drafts[&0].text, "from host");
        assert_eq!(drafts[&0].cursor, "from host".len());
        assert_eq!(drafts[&1].text, "typed");
        assert!(!drafts.contains_key(&2));
        assert!(!drafts.contains_key(&3));
    }

    #[test]
    fn drafts_wait_until_the_composer_is_quiet() {
        let edited = Instant::now();
        assert_eq!(draft_save_delay(edited, edited), Some(DRAFT_SAVE_QUIET));
        assert_eq!(
            draft_save_delay(edited, edited + Duration::from_millis(150)),
            Some(Duration::from_millis(250))
        );
        assert_eq!(draft_save_delay(edited, edited + DRAFT_SAVE_QUIET), None);
        assert_eq!(
            draft_save_delay(edited, edited + Duration::from_secs(5)),
            None
        );
    }
}

/// Tema's control: a segmented row like the page's others, each option with its icon.
fn segmented_icons(
    id: &'static str,
    options: &[(&'static str, &'static str)],
    chosen: usize,
    cx: &mut Context<Shell>,
    apply: fn(&mut Look, usize),
) -> Div {
    // Its own row, so the control takes the width of its options, not of the page.
    div().flex().child(
        div()
            .flex()
            .flex_none()
            .gap(px(2.))
            .p(px(3.))
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(content(0.06))
            .children(options.iter().enumerate().map(|(ix, (glyph, label))| {
                let on = ix == chosen;
                div()
                    .id((id, ix))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(7.))
                    .h(px(28.))
                    .w(px(108.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .text_size(px(13.))
                    .text_color(content(if on { 1. } else { 0.65 }))
                    .when(on, |d| d.bg(content(0.14)))
                    .when(!on, |d| d.hover(|s| s.bg(content(0.06))))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.set_look(|look| apply(look, ix), cx)
                        }),
                    )
                    .child(icon(glyph, 16.).text_color(content(if on { 0.95 } else { 0.6 })))
                    .child(*label)
            })),
    )
}

#[cfg(test)]
mod claude_catalog_tests {
    use super::*;

    fn alias(id: &str, name: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_owned(),
            name: name.to_owned(),
            efforts: Vec::new(),
            default_effort: None,
            fast: false,
            default_fast: false,
            available: true,
            parameterized: false,
        }
    }

    #[test]
    fn aliases_get_their_version_and_earlier_models_follow_their_family() {
        let list = claude_catalog(vec![
            alias("haiku", "Haiku"),
            alias("opus", "Opus"),
            alias("claude-fable-5-1[1m]", "Fable"),
            alias("sonnet", "Sonnet"),
        ]);
        let names: Vec<&str> = list.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Fable 5.1",
                "Fable 5",
                "Opus 5.5",
                "Opus 5",
                "Opus 4.8",
                "Opus 4.7",
                "Opus 4.6",
                "Sonnet 5.5",
                "Sonnet 5",
                "Sonnet 4.6",
                "Haiku 5.5",
                "Haiku 4.5",
            ]
        );
        assert_eq!(list[2].id, "opus", "the alias stays what is sent");
    }

    #[test]
    fn a_session_on_an_alias_reads_with_its_version() {
        assert_eq!(model_label(Some("opus"), "claude"), "Opus 5.5");
        assert_eq!(model_label(Some("claude-opus-4-8"), "claude"), "Opus 4.8");
    }
}

#[cfg(test)]
mod follow_caret_tests {
    use super::*;

    #[test]
    fn the_text_moves_only_as_far_as_the_caret_needs() {
        let (line, seen) = (px(22.), px(190.));
        // The caret's line already shows.
        assert_eq!(follow_caret(px(44.), line, px(0.), seen), px(0.));
        // Typing past the bottom brings its line in, and no more.
        assert_eq!(follow_caret(px(198.), line, px(0.), seen), px(30.));
        // Going back past the top does the same the other way.
        assert_eq!(follow_caret(px(22.), line, px(100.), seen), px(22.));
    }
}

#[cfg(test)]
mod composer_follow_tests {
    use super::*;
    use gpui::{Render, TestAppContext, VisualTestContext};

    struct View {
        text: String,
        cursor: usize,
        handle: gpui::ScrollHandle,
        seen: Rc<Cell<(usize, usize)>>,
    }
    impl Render for View {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let styled = gpui::StyledText::new(self.text.clone());
            let layout = styled.layout().clone();
            let (handle, seen, cursor, key) = (
                self.handle.clone(),
                self.seen.clone(),
                self.cursor,
                (self.cursor, self.text.len()),
            );
            let overlay = gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let line = layout.line_height();
                    if seen.replace(key) != key
                        && let Some(at) = layout.position_for_index(cursor)
                    {
                        let top = at.y - bounds.origin.y;
                        let height = handle.bounds().size.height;
                        let scrolled = -handle.offset().y;
                        let target = follow_caret(top, line, scrolled, height);
                        if target != scrolled {
                            handle.set_offset(gpui::point(px(0.), -target));
                            window.on_next_frame(|window, _| window.refresh());
                        }
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            div().size_full().flex().flex_col().child(
                div()
                    .id("composer-text")
                    .max_h(px(100.))
                    .overflow_y_scroll()
                    .track_scroll(&self.handle)
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .child(div().relative().w_full().child(styled).child(overlay)),
            )
        }
    }

    #[gpui::test]
    fn the_text_scrolls_with_the_caret(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| View {
            text: "a".into(),
            cursor: 1,
            handle: gpui::ScrollHandle::new(),
            seen: Default::default(),
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        let mut offsets = Vec::new();
        for _ in 0..12 {
            window
                .update(&mut vcx, |view, _, cx| {
                    view.text.push_str("\nline");
                    view.cursor = view.text.len();
                    cx.notify();
                })
                .unwrap();
            vcx.run_until_parked();
            offsets.push(window.update(&mut vcx, |view, _, _| view.handle.offset().y).unwrap());
        }
        // 13 lines of 22 px in a 100 px window: the last line stays in view.
        assert_eq!(*offsets.last().unwrap(), px(-(13. * 22. - 100.)));
    }
}
