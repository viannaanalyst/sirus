//! Settings pages, ported from Sirus's `components/settings/*.tsx`. Each page is a table of rows
//! bound to AppSettings keys or to backend commands (`loads` names what a page reads). The text is
//! the Portuguese of `src/i18n`. Text fields (`Ctl::Field`) start editing through `edit`; the main
//! app types into the edited text and, on Enter, calls `commit` and saves or runs what it returns.
//! The pairing popup (`settings_pair.rs`) and folder choices are run as `native_pair_*` and `native_pick_folder`. GitHub accounts,
//! opening folders and the two-click confirmations run as the `native_*` commands of
//! `native_actions`; the rest goes to the host as the backend commands the Tauri build uses
//! (`connector_action`, `worktree_cleanup_action`, `delete_session`). Destructive buttons ask first
//! through `native_confirm`, which opens the shell's confirmation popup.

use crate::i18n::{t, tf};
use crate::theme::{accent, content, on_accent};
use chrono::{DateTime, Local};
use gpui::{AnyElement, Context, FontWeight, Hsla, div, prelude::*, px, rgb};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

/// Shown in place of a page's rows while its backend data is still on its way.
const LOADING: &str = "Carregando…";
/// The row shown by a page that has no port yet.
const SOON: &str = "Esta parte entra em seguida.";

/// Runs a backend command by name with JSON args; the main app refreshes `data` afterward.
pub type Run<V> = fn(&mut V, &'static str, Value, &mut Context<V>);
/// Saves one top-level key of AppSettings; `value` is the whole key, nested objects included.
pub type Save<V> = fn(&mut V, &'static str, Value, &mut Context<V>);
/// Starts editing the text field `key`, with its current text. The main app then routes typing
/// into that text until Enter (`commit`) or Escape.
pub type Edit<V> = fn(&mut V, &'static str, String, &mut Context<V>);

/// The change a finished text edit makes: a save of a top-level key, or a backend command.
pub enum Commit {
    Save { key: &'static str, value: Value },
    Run { cmd: &'static str, args: Value },
}

/// What a page's views share: the text being edited and the callbacks each control fires.
struct Ui<'a, V> {
    /// The saved shortcuts, for a row's way back to its default.
    custom: Value,
    ids: usize,
    editing: Option<(&'a str, &'a str)>,
    save: Save<V>,
    edit: Edit<V>,
    run: Run<V>,
}

/// What a click does: save a top-level key of AppSettings, or run a backend command.
#[derive(Clone)]
enum Act {
    Save { key: &'static str, value: Value },
    Run { cmd: &'static str, args: Value },
}

/// A destructive action asks first: the shell's confirmation popup runs it.
fn ask<V: 'static>(view: &mut V, label: &'static str, act: &Act, run: Run<V>, cx: &mut Context<V>) {
    let then = match act {
        Act::Save { key, value } => json!({ "kind": "save", "key": key, "value": value }),
        Act::Run { cmd, args } => json!({ "kind": "run", "cmd": cmd, "args": args }),
    };
    run(
        view,
        "native_confirm",
        json!({ "label": label, "then": then }),
        cx,
    );
}

/// A setting: top-level (`obj` is None) or a field of an object setting such as `notifications`.
#[derive(Clone, Copy)]
struct Field {
    obj: Option<&'static str>,
    key: &'static str,
}

const fn top(key: &'static str) -> Field {
    Field { obj: None, key }
}

const fn inner(obj: &'static str, key: &'static str) -> Field {
    Field {
        obj: Some(obj),
        key,
    }
}

struct Opt {
    label: &'static str,
    chosen: bool,
    change: Act,
}

enum Ctl {
    /// A switch. `change` is None when the switch is fixed or waits on another setting.
    Switch { on: bool, change: Option<Act> },
    /// A segmented control over a short enum (two-way booleans use "true" and "false").
    Pick(Vec<Opt>),
    /// Read-only text: a value the React page edits with a richer widget, or a hint.
    Read(String),
    /// A button; dimmed and inert when `enabled` is false.
    Button {
        label: &'static str,
        act: Act,
        enabled: bool,
        danger: bool,
    },
    /// A text field of `width` px showing `text`; a click (or the `button` label) starts editing `key`.
    Field {
        key: &'static str,
        text: String,
        width: f32,
        button: Option<&'static str>,
    },
    /// A shortcut's keys; a click records new ones. `custom` adds the way back to the default.
    Shortcut {
        id: &'static str,
        combo: String,
        custom: bool,
    },
    /// A provider's folder button; a click opens its labelled path field, with what was detected.
    Path {
        key: &'static str,
        text: String,
        detected: String,
    },
}

/// The height of a page's title bar and the left inset it shares with the page's content.
const TITLE_BAR_HEIGHT: f32 = 52.;
pub const CONTENT_INSET: f32 = 28.;
pub const CONTENT_WIDTH: f32 = 1040.;

struct Row {
    title: String,
    about: String,
    controls: Vec<Ctl>,
    /// A provider's mark at the head of the row.
    provider: Option<&'static str>,
    /// A connector's logo at the head of the row, by its catalog id.
    mark: Option<&'static str>,
    /// Not available: the whole row reads as dimmed.
    faded: bool,
}

struct Group {
    /// Empty for a card without a heading.
    title: String,
    rows: Vec<Row>,
}

struct Page {
    title: String,
    about: String,
    /// Buttons at the top right of the header.
    actions: Vec<Ctl>,
    groups: Vec<Group>,
}

/// A switch row described by (title, description, setting).
type SwitchRow = (&'static str, &'static str, Field);

/// What a page reads from the backend: (key in `data`, command, args). Called when the page opens.
pub fn loads(page: &str) -> Vec<(&'static str, &'static str, Value)> {
    match page {
        "Provedores" => vec![("agents", "detect_agents", json!({}))],
        "Agent skills" => vec![("skills", "skill_action", skills_catalog())],
        "Conectores" => vec![
            (
                "connectors",
                "connector_action",
                json!({"action": {"type": "status"}}),
            ),
            ("host", "host_info", json!({})),
            ("agents", "detect_agents", json!({})),
        ],
        "Computador" => vec![(
            "computer",
            "computer_action",
            json!({"action": {"type": "status"}}),
        )],
        "Conexões" => vec![(
            "remote",
            "remote_action",
            json!({"action": {"type": "status"}}),
        )],
        "Git" => vec![
            ("host", "host_info", json!({})),
            ("github", "native_github_list", json!({})),
        ],
        "Worktrees" => vec![
            ("host", "host_info", json!({})),
            ("appState", "load_state", json!({})),
            (
                "worktreeScan",
                "worktree_cleanup_action",
                json!({"action": {"type": "scan"}}),
            ),
        ],
        "Terminal" | "Avançado" => vec![("host", "host_info", json!({}))],
        _ => Vec::new(),
    }
}

/// Renders the page `page` (its Portuguese menu name) from the backend's AppSettings JSON and the
/// data its `loads` brought. `editing` is the text field being edited (its key and its text). Each
/// control calls `save` with a top-level key, `edit` to start a text field, or `run` with a command.
pub fn page<V: 'static>(
    page: &str,
    settings: &Value,
    data: &std::collections::HashMap<&'static str, Value>,
    editing: Option<(&str, &str)>,
    save: Save<V>,
    edit: Edit<V>,
    run: Run<V>,
    cx: &mut Context<V>,
) -> (AnyElement, AnyElement) {
    let mut shown = build(page, settings, data);
    let loading = loads(page)
        .iter()
        .any(|(key, _, _)| !data.contains_key(key));
    if loading {
        shown.groups = vec![note(t(LOADING))];
    }
    let mut ui = Ui {
        ids: 0,
        custom: settings["customShortcuts"].clone(),
        editing,
        save,
        edit,
        run,
    };
    let restore = restorable_keys(page, &shown);
    let mut actions = Vec::new();
    if !restore.is_empty() && !loading {
        actions.push(control_view(restore_button(restore), &mut ui, cx));
    }
    if page == "Notificações" {
        actions.push(control_view(
            Ctl::Button {
                label: t("Testar notificação"),
                act: act_run("native_notify_test", json!({})),
                enabled: true,
                danger: false,
            },
            &mut ui,
            cx,
        ));
    }
    for ctl in shown.actions {
        actions.push(control_view(ctl, &mut ui, cx));
    }
    let mut body = Vec::new();
    if page == "Conectores" && !loading {
        // Sirus's own layout: search, connected list, catalog cards and a dialog.
        body.push(crate::settings_connectors::view(
            settings, data, editing, save, edit, run, cx,
        ));
    }
    for group in shown.groups {
        body.push(group_view(group, &mut ui, cx));
    }
    let about = (!shown.about.is_empty()).then(|| about_line(shown.about.clone()));
    (
        title_bar(shown.title.clone(), actions),
        div()
            .w_full()
            .flex()
            .flex_col()
            .children(about)
            .children(body)
            .into_any_element(),
    )
}

/// The settings a page changes, for its Restaurar padrões. Pages that list things the person added
/// have nothing to go back to.
fn restorable_keys(page: &str, shown: &Page) -> Vec<&'static str> {
    if matches!(
        page,
        "Provedores" | "Agent skills" | "Conectores" | "Conexões" | "Git"
    ) {
        return Vec::new();
    }
    let mut keys = Vec::new();
    if page == "Atalhos" {
        keys.push("customShortcuts");
    }
    for group in &shown.groups {
        for row in &group.rows {
            for ctl in &row.controls {
                let acts: Vec<&Act> = match ctl {
                    Ctl::Switch {
                        change: Some(act), ..
                    } => vec![act],
                    Ctl::Pick(options) => options.iter().map(|o| &o.change).collect(),
                    _ => Vec::new(),
                };
                for act in acts {
                    if let Act::Save { key, .. } = act {
                        if !keys.contains(key) {
                            keys.push(*key);
                        }
                    }
                }
            }
        }
    }
    keys
}

fn restore_button(keys: Vec<&'static str>) -> Ctl {
    Ctl::Button {
        label: t("Restaurar padrões"),
        act: act_run(
            "native_confirm",
            json!({
                "label": t("Restaurar"),
                "title": t("Restaurar os padrões desta página?"),
                "message": t("As opções desta página voltam ao padrão do app."),
                "then": { "kind": "run", "cmd": "native_restore", "args": { "keys": keys } },
            }),
        ),
        enabled: true,
        danger: false,
    }
}

/// Aparência keeps its look in its own file, so its restore has its own command.
pub fn restore_look_button<V: 'static>(run: Run<V>, cx: &mut Context<V>) -> AnyElement {
    div()
        .id("restore-look")
        .h(px(28.))
        .px(px(10.))
        .rounded(px(crate::theme::RADIUS_SM))
        .border_1()
        .border_color(content(0.12))
        .flex()
        .items_center()
        .text_size(px(12.5))
        .text_color(content(0.85))
        .cursor_pointer()
        .hover(|style| style.bg(content(0.08)))
        .on_click(
            cx.listener(move |this, _, _, cx| run(this, "native_restore_look", json!({}), cx)),
        )
        .child(t("Restaurar padrões"))
        .into_any_element()
}

/// The top of a settings page: its title and actions, on a rule that runs the pane's whole width.
pub fn title_bar(title: impl Into<gpui::SharedString>, actions: Vec<AnyElement>) -> AnyElement {
    div()
        .flex_shrink_0()
        .w_full()
        .h(px(TITLE_BAR_HEIGHT))
        .border_b_1()
        .border_color(content(0.08))
        .child(
            // The content's own width, so the actions end where the page's cards end.
            div()
                .w_full()
                .h_full()
                .max_w(px(CONTENT_WIDTH))
                .px(px(CONTENT_INSET))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(content(1.))
                        .child(title.into()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .flex_shrink_0()
                        .children(actions),
                ),
        )
        .into_any_element()
}

/// The page's description, first thing under the rule.
pub fn about_line(about: impl Into<gpui::SharedString>) -> AnyElement {
    div()
        .mb(px(20.))
        .text_size(px(13.))
        .text_color(content(0.5))
        .child(about.into())
        .into_any_element()
}

/// The page's title, description, header buttons and rows, from the settings and loaded data.
fn build(page: &str, s: &Value, d: &HashMap<&'static str, Value>) -> Page {
    match page {
        "Geral" => page_of(
            t("Geral"),
            t("Escolha os padrões para novas sessões, navegação e o painel Ambiente."),
            general(s),
        ),
        "Comportamento do chat" => page_of(
            t("Comportamento do chat"),
            t(
                "Controle mensagens durante a resposta, ditado, turnos terminados, revisão e confirmações de segurança.",
            ),
            chat(s),
        ),
        "Notificações" => page_of(
            t("Notificações"),
            t("Escolha como o Sirus avisa quando o trabalho termina ou precisa da sua atenção."),
            notifications(s),
        ),
        "Atalhos" => page_of(
            t("Atalhos"),
            t("Personalize os comandos de teclado que você usa no aplicativo."),
            keybindings(s),
        ),
        "Git" => page_of("Git", "", git(s, d)),
        "Worktrees" => page_of("Worktrees", "", worktrees(s, d)),
        "Terminal" => page_of("Terminal", "", terminal(s, d)),
        "Avançado" => page_of(t("Avançado"), "", advanced(s, d)),
        "Provedores" => Page {
            actions: vec![
                Ctl::Button {
                    label: t("Detectar novamente"),
                    act: Act::Run {
                        cmd: "detect_agents",
                        args: json!({}),
                    },
                    enabled: true,
                    danger: false,
                },
                Ctl::Button {
                    label: t("Verificar CLIs"),
                    act: act_run("cli_diagnostics", json!({})),
                    enabled: true,
                    danger: false,
                },
            ],
            ..page_of(
                t("Provedores"),
                t("Integrações que disponibilizam modelos para sessões."),
                providers(s, d),
            )
        },
        "Agent skills" => {
            let any_disabled = !strings(&s["disabledSkills"]).is_empty();
            Page {
                actions: vec![
                    Ctl::Button {
                        label: t("Atualizar skills"),
                        act: Act::Run {
                            cmd: "skill_action",
                            args: skills_catalog(),
                        },
                        enabled: true,
                        danger: false,
                    },
                    Ctl::Button {
                        label: t("Restaurar padrões"),
                        act: Act::Save {
                            key: "disabledSkills",
                            value: json!([]),
                        },
                        enabled: any_disabled,
                        danger: false,
                    },
                ],
                ..page_of(
                    t("Skills dos agentes"),
                    t("Fluxos reutilizáveis encontrados nos provedores e no workspace atual."),
                    skills(s, d),
                )
            }
        }
        "Conectores" => page_of(
            t("Conectores"),
            t(
                "Conecte Supabase, Linear, Notion e outros serviços uma vez; Claude, Codex, OpenCode e os outros agentes ACP passam a usá-los.",
            ),
            Vec::new(),
        ),
        "Computador" => {
            if get_data(d, "computer")["supported"].as_bool() == Some(false) {
                page_of(
                    t("Computador"),
                    t("O uso do computador só está disponível no macOS."),
                    Vec::new(),
                )
            } else {
                page_of(
                    t("Computador"),
                    t(
                        "Deixe o Codex, o Claude Code e o OpenCode verem e usarem apps do Mac que você aprovar, em segundo plano.",
                    ),
                    computer(s, d),
                )
            }
        }
        "Conexões" => page_of(
            t("Conexões"),
            t(
                "Use o Sirus pelo celular ou por outro computador. O trabalho continua rodando neste Mac; o outro aparelho só controla, pela sua rede privada da Tailscale.",
            ),
            computers_groups(d)
                .into_iter()
                .chain(connections(d))
                .collect(),
        ),
        other => page_of(other, "", soon()),
    }
}

// Building blocks.

fn page_of(title: &str, about: &str, groups: Vec<Group>) -> Page {
    Page {
        title: title.to_string(),
        about: about.to_string(),
        actions: Vec::new(),
        groups,
    }
}

fn group(title: impl Into<String>, rows: Vec<Row>) -> Group {
    Group {
        title: title.into(),
        rows,
    }
}

fn row(title: impl Into<String>, about: impl Into<String>, controls: Vec<Ctl>) -> Row {
    Row {
        title: title.into(),
        about: about.into(),
        controls,
        provider: None,
        mark: None,
        faded: false,
    }
}

/// A card without a heading whose only content is a muted line.
fn note(text: impl Into<String>) -> Group {
    group("", vec![row("", text, Vec::new())])
}

fn placeholder() -> Row {
    row(t(SOON), "", Vec::new())
}

fn soon() -> Vec<Group> {
    vec![group("", vec![placeholder()])]
}

fn get(settings: &Value, field: Field) -> Option<&Value> {
    match field.obj {
        Some(obj) => settings.get(obj)?.get(field.key),
        None => settings.get(field.key),
    }
}

fn flag(settings: &Value, field: Field) -> bool {
    get(settings, field)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The setting as text: booleans as "true"/"false", numbers and strings as they are.
fn current(settings: &Value, field: Field) -> String {
    match get(settings, field) {
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// A value from a table: "true"/"false" are booleans, whole numbers are numbers.
fn json_of(value: &str) -> Value {
    match value {
        "true" => json!(true),
        "false" => json!(false),
        _ => value
            .parse::<i64>()
            .map_or_else(|_| json!(value), |n| json!(n)),
    }
}

/// The save that sets `field` to `value`. A nested setting is saved whole, with one field changed.
fn change(settings: &Value, field: Field, value: Value) -> Act {
    match field.obj {
        None => Act::Save {
            key: field.key,
            value,
        },
        Some(obj) => {
            let mut whole = settings.get(obj).cloned().unwrap_or_else(|| json!({}));
            whole[field.key] = value;
            Act::Save {
                key: obj,
                value: whole,
            }
        }
    }
}

fn switch_of(settings: &Value, field: Field) -> Ctl {
    let on = flag(settings, field);
    Ctl::Switch {
        on,
        change: Some(change(settings, field, json!(!on))),
    }
}

/// A switch that cannot be changed here: fixed by the app, or waiting on another setting.
fn fixed(on: bool) -> Ctl {
    Ctl::Switch { on, change: None }
}

fn pick(settings: &Value, field: Field, options: &[(&'static str, &'static str)]) -> Ctl {
    let now = current(settings, field);
    Ctl::Pick(
        options
            .iter()
            .map(|&(value, label)| Opt {
                label,
                chosen: now == value,
                change: change(settings, field, json_of(value)),
            })
            .collect(),
    )
}

/// Rows of switches from a table of (title, description, setting).
fn switches(settings: &Value, table: &[SwitchRow]) -> Vec<Row> {
    table
        .iter()
        .map(|&(title, about, field)| row(title, about, vec![switch_of(settings, field)]))
        .collect()
}

/// One row with one switch, for the pages that build their rows by hand.
fn switch_row(s: &Value, title: &'static str, about: &str, field: Field) -> Row {
    row(title, about, vec![switch_of(s, field)])
}

fn yes(value: &Value) -> bool {
    value.as_bool() == Some(true)
}

/// The string items of a JSON array (`disabledSkills`, `connectors`' ids and similar).
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn has(list: &[String], item: &str) -> bool {
    list.iter().any(|entry| entry == item)
}

/// `list` without `item`.
fn without(list: &[String], item: &str) -> Vec<String> {
    list.iter()
        .filter(|entry| entry.as_str() != item)
        .cloned()
        .collect()
}

/// `list` with `item` added once.
fn with(list: &[String], item: &str) -> Vec<String> {
    let mut next = list.to_vec();
    if !has(list, item) {
        next.push(item.to_string());
    }
    next
}

/// The loaded value under `key`, or null while it is still on its way.
fn get_data<'a>(data: &'a HashMap<&'static str, Value>, key: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    data.get(key).unwrap_or(&NULL)
}

fn act_run(cmd: &'static str, args: Value) -> Act {
    Act::Run { cmd, args }
}

/// A `remote_action` call; `action` is the closed RemoteAction JSON.
fn remote_act(action: Value) -> Act {
    act_run("remote_action", json!({"action": action}))
}

/// A `computer_action` call; `action` is the closed ComputerAction JSON.
fn computer_act(action: Value) -> Act {
    act_run("computer_action", json!({"action": action}))
}

/// The skills catalog for no particular project or session.
fn skills_catalog() -> Value {
    json!({"action": {"type": "catalog", "owner": {"projectId": null, "sessionId": null}}})
}

/// A date and time as the local clock shows it, from an RFC 3339 stamp; the raw text if it does not parse.
fn local_when(stamp: &str, style: &str) -> String {
    DateTime::parse_from_rfc3339(stamp).map_or_else(
        |_| stamp.to_string(),
        |when| when.with_timezone(&Local).format(style).to_string(),
    )
}

// Views.

fn group_view<V: 'static>(group: Group, ui: &mut Ui<'_, V>, cx: &mut Context<V>) -> AnyElement {
    let mut rows = Vec::new();
    for item in group.rows {
        rows.push(row_view(item, ui, cx));
    }
    let mut block = div().mb(px(32.));
    if !group.title.is_empty() {
        block = block.child(
            div()
                .mb(px(4.))
                .text_size(px(12.5))
                .text_color(content(0.5))
                .child(group.title),
        );
    }
    block
        .child(
            div()
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.09))
                .bg(content(0.02))
                .overflow_hidden()
                .children(rows),
        )
        .into_any_element()
}

fn row_view<V: 'static>(item: Row, ui: &mut Ui<'_, V>, cx: &mut Context<V>) -> AnyElement {
    let mut controls = Vec::new();
    for ctl in item.controls {
        controls.push(control_view(ctl, ui, cx));
    }
    let mut text = div().flex_1().min_w_0().flex().flex_col().gap(px(2.));
    if !item.title.is_empty() {
        text = text.child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(content(0.95))
                .child(item.title),
        );
    }
    for line in item.about.lines() {
        text = text.child(
            div()
                .text_size(px(12.))
                .text_color(content(0.5))
                .child(line.to_owned()),
        );
    }
    div()
        .px(px(16.))
        .py(px(14.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(20.))
        .border_b_1()
        .border_color(content(0.07))
        .when(item.faded, |d| d.opacity(0.5))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(10.))
                .when_some(item.provider, |d, provider| {
                    d.child(crate::tab_tile::mark_tile(
                        &crate::tab_tile::ACTIVITY_TILE,
                        crate::tab_tile::Face::provider(provider),
                        crate::theme::provider_glyph(provider, 12.),
                    ))
                })
                .when_some(item.mark, |d, id| {
                    d.child(crate::settings_connectors::logo(Some(id), id, 26.))
                })
                .child(text),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .flex_shrink_0()
                .children(controls),
        )
        .into_any_element()
}

fn next_id(ids: &mut usize) -> usize {
    *ids += 1;
    *ids
}

/// Runs what a click does: the page's `save` for a setting, its `run` for a backend command.
fn fire<V: 'static>(view: &mut V, act: &Act, save: Save<V>, run: Run<V>, cx: &mut Context<V>) {
    match act {
        Act::Save { key, value } => save(view, key, value.clone(), cx),
        Act::Run { cmd, args } => run(view, cmd, args.clone(), cx),
    }
}

/// A text field: while `key` is being edited it shows the typed text with a caret; otherwise its
/// text (or the `button` label), and a click starts editing it.
fn field_view<V: 'static>(
    key: &'static str,
    text: String,
    width: f32,
    button: Option<&'static str>,
    ui: &mut Ui<'_, V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let edit = ui.edit;
    let typing = ui
        .editing
        .and_then(|(known, typed)| (known == key).then(|| typed.to_string()));
    let id = next_id(&mut ui.ids);
    if let (None, Some(label)) = (&typing, button) {
        return div()
            .id(("setting", id))
            .h(px(28.))
            .px(px(10.))
            .rounded(px(crate::theme::RADIUS_SM))
            .border_1()
            .border_color(content(0.12))
            .flex()
            .items_center()
            .text_size(px(12.5))
            .text_color(content(0.85))
            .cursor_pointer()
            .hover(|style| style.bg(content(0.08)))
            .on_click(cx.listener(move |this, _, _, cx| edit(this, key, text.clone(), cx)))
            .child(label)
            .into_any_element();
    }
    let field = div()
        .id(("setting", id))
        .h(px(28.))
        .w(px(width))
        .px(px(8.))
        .rounded(px(crate::theme::RADIUS_SM))
        .border_1()
        .flex()
        .items_center()
        .gap(px(1.))
        .text_size(px(12.5))
        .whitespace_nowrap()
        .overflow_hidden();
    match typing {
        Some(typed) => field
            .border_color(accent())
            .text_color(content(0.95))
            .child(typed)
            .child(div().w(px(1.5)).h(px(14.)).bg(accent()))
            .into_any_element(),
        None => {
            let empty = text.is_empty();
            let shown = if empty {
                "—".to_string()
            } else {
                text.clone()
            };
            field
                .border_color(content(0.12))
                .text_color(content(if empty { 0.4 } else { 0.85 }))
                .hover(|style| style.border_color(content(0.25)))
                .on_click(cx.listener(move |this, _, _, cx| edit(this, key, text.clone(), cx)))
                .child(shown)
                .into_any_element()
        }
    }
}

/// The caps of a shortcut, or the prompt while it records.
fn shortcut_view<V: 'static>(
    id: &'static str,
    combo: String,
    custom: bool,
    ui: &mut Ui<'_, V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let (edit, save) = (ui.edit, ui.save);
    let key = key_of(format!("shortcut:{id}"));
    let cap = |text: String| {
        div()
            .min_w(px(22.))
            .h(px(22.))
            .px(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .border_1()
            .border_b_2()
            .border_color(content(0.12))
            .bg(content(0.07))
            .text_size(px(12.))
            .text_color(content(0.9))
            .child(text)
    };
    if let Some((_, note)) = ui.editing.filter(|(field, _)| *field == key) {
        let row = div().flex().items_center().gap(px(8.));
        let row = if note.is_empty() {
            row
        } else {
            row.child(
                div()
                    .text_size(px(11.5))
                    .text_color(gpui::rgb(0xe08a8a))
                    .child(note.to_owned()),
            )
        };
        return row
            .child(
                div()
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .border_1()
                    .border_color(accent())
                    .text_size(px(12.))
                    .text_color(accent())
                    .child(t("Pressione as teclas…")),
            )
            .into_any_element();
    }
    let at = next_id(&mut ui.ids);
    let settings = ui.custom.clone();
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .when(custom, |d| {
            d.child(
                div()
                    .id(("setting-reset", at))
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(|style| style.bg(content(0.07)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mut map = settings.clone();
                        if let Some(entries) = map.as_object_mut() {
                            entries.remove(id);
                        }
                        save(this, "customShortcuts", map, cx);
                    }))
                    .child(crate::theme::icon("undo", 14.)),
            )
        })
        .child(
            div()
                .id(("setting", at))
                .flex()
                .gap(px(3.))
                .cursor_pointer()
                .hover(|style| style.opacity(0.8))
                .on_click(cx.listener(move |this, _, _, cx| edit(this, key, String::new(), cx)))
                .children(shortcut_caps(&combo).into_iter().map(cap)),
        )
        .into_any_element()
}

/// A combo's label split into one cap per key: "⌥⌘↩" is three.
fn shortcut_caps(combo: &str) -> Vec<String> {
    shortcut_label(combo).chars().map(String::from).collect()
}

/// The keys a keystroke makes, in the host's form: `meta+alt+shift+k`.
fn combo_of(keys: &gpui::Keystroke) -> String {
    let mut parts = Vec::new();
    if keys.modifiers.platform {
        parts.push("meta");
    }
    if keys.modifiers.alt {
        parts.push("alt");
    }
    if keys.modifiers.shift {
        parts.push("shift");
    }
    parts.push(keys.key.as_str());
    parts.join("+")
}

/// Whether the host takes a combo for `id` (its rule in `models.rs`, in short): ⌘ plus a letter,
/// number or a few signs; not the keys macOS and text fields keep.
fn shortcut_allowed(id: &str, combo: &str) -> bool {
    let mut parts: Vec<&str> = combo.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    if id == "cycle-effort" && key == "tab" && !parts.is_empty() {
        return true;
    }
    if !parts.contains(&"meta") || parts.iter().any(|m| !["meta", "alt", "shift"].contains(m)) {
        return false;
    }
    if key == "enter" {
        return parts.len() > 1;
    }
    let simple = key.len() == 1
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ",.;/\\[]`'".contains(c));
    simple
        && (![
            "q", "w", "h", "m", "c", "v", "x", "a", "z", "r", "l", "t", "=", "-",
        ]
        .contains(&key)
            || combo == "meta+shift+t")
}

/// The combo a binding answers to now.
fn effective_combo<'a>(settings: &'a Value, id: &str, fallback: &'a str) -> &'a str {
    settings["customShortcuts"][id].as_str().unwrap_or(fallback)
}

/// What a recorded keystroke does to the saved shortcuts: the new map, or why it cannot be.
pub fn recorded_shortcut(
    settings: &Value,
    id: &str,
    keys: &gpui::Keystroke,
) -> Result<Value, String> {
    let combo = combo_of(keys);
    if !shortcut_allowed(id, &combo) {
        return Err(t("Use ⌘ com uma letra ou número livre.").to_owned());
    }
    // Keys the window answers to that the list above does not show.
    const UNLISTED: [(&str, &str); 5] = [
        ("meta+j", "Abrir terminal"),
        ("meta+shift+j", "Abrir terminal"),
        ("meta+p", "Buscar arquivos"),
        ("meta+shift+p", "Paleta de comandos"),
        ("meta+shift+w", "Fechar outras abas"),
    ];
    if let Some((_, label)) = UNLISTED.iter().find(|(known, _)| *known == combo) {
        return Err(tf("Já usado por {}", &[&t(label)]));
    }
    if let Some((_, label, _, _)) = KEYBINDINGS.iter().find(|(known, _, _, fallback)| {
        *known != id && effective_combo(settings, known, fallback) == combo
    }) {
        return Err(tf("Já usado por {}", &[&t(label)]));
    }
    let mut map = settings["customShortcuts"].clone();
    if !map.is_object() {
        map = json!({});
    }
    if KEYBINDINGS
        .iter()
        .any(|(known, _, _, fallback)| *known == id && *fallback == combo)
    {
        if let Some(entries) = map.as_object_mut() {
            entries.remove(id);
        }
    } else {
        map[id] = json!(combo);
    }
    Ok(map)
}

/// What a pressed key means once shortcuts can move.
pub enum ShortcutRoute {
    /// Nothing moved it.
    Pass,
    /// Its binding moved away: it does nothing now.
    Unbound,
    /// It is a binding's new keys: handle it as the keys the app knows that binding by.
    As(gpui::Keystroke),
}

pub fn shortcut_route(settings: &Value, keys: &gpui::Keystroke) -> ShortcutRoute {
    let Some(custom) = settings["customShortcuts"]
        .as_object()
        .filter(|m| !m.is_empty())
    else {
        return ShortcutRoute::Pass;
    };
    let pressed = combo_of(keys);
    for (id, _, _, fallback) in KEYBINDINGS {
        match custom.get(*id).and_then(Value::as_str) {
            Some(combo) if combo == pressed => {
                return gpui::Keystroke::parse(&fallback.replace("meta", "cmd").replace('+', "-"))
                    .map_or(ShortcutRoute::Pass, ShortcutRoute::As);
            }
            Some(_) if *fallback == pressed => return ShortcutRoute::Unbound,
            _ => {}
        }
    }
    ShortcutRoute::Pass
}

/// The folder button of a provider row. While its path is edited, the field with its label: the
/// field alone once read as the account, and an email saved there hid the CLI.
fn path_view<V: 'static>(
    key: &'static str,
    text: String,
    detected: String,
    ui: &mut Ui<'_, V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let edit = ui.edit;
    let editing = ui.editing.is_some_and(|(known, _)| known == key);
    if !editing {
        let id = next_id(&mut ui.ids);
        return div()
            .id(("setting", id))
            .size(px(26.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|style| style.bg(content(0.07)))
            .on_click(cx.listener(move |this, _, _, cx| edit(this, key, text.clone(), cx)))
            .child(crate::theme::icon("folder", 14.))
            .into_any_element();
    }
    div()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(11.5))
                .text_color(content(0.5))
                .child(t("Caminho do executável. Vazio usa o detectado.")),
        )
        .child(field_view(key, text, 300., None, ui, cx))
        .when(!detected.is_empty(), |d| {
            d.child(
                div()
                    .max_w(px(300.))
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(content(0.4))
                    .child(tf("Detectado: {}", &[&detected])),
            )
        })
        .into_any_element()
}

const SWITCH_OFF: f32 = 2.;
const SWITCH_ON: f32 = 20.;

thread_local! {
    static SWITCHES: std::cell::RefCell<HashMap<usize, (bool, usize)>> =
        std::cell::RefCell::new(HashMap::new());
}

/// A switch that just changed gets a new animation id, so its knob slides once; one seen for the
/// first time, as a page opens, stays put. Its generation stays while the state does, so the slide
/// keeps its clock across frames.
fn switch_motion(id: usize, on: bool) -> Option<usize> {
    SWITCHES.with(|known| {
        let mut known = known.borrow_mut();
        match known.get_mut(&id) {
            None => {
                known.insert(id, (on, 0));
                None
            }
            Some((state, generation)) => {
                if *state != on {
                    *state = on;
                    *generation += 1;
                }
                (*generation > 0).then_some(id * 10_000 + *generation)
            }
        }
    })
}

/// The switch of Settings: a track and a knob that slides when `on` changes and stretches while
/// pressed. `id` keys its animation, so it must stay the same for the same switch across frames;
/// `live` false dims it. The caller adds the click.
pub(crate) fn switch_track(id: usize, on: bool, live: bool) -> gpui::Stateful<gpui::Div> {
    let group: gpui::SharedString = format!("switch-{id}").into();
    let knob = div()
        .id(("switch-knob", id))
        .absolute()
        .top(px(2.))
        .left(px(if on { SWITCH_ON } else { SWITCH_OFF }))
        .h(px(18.))
        .w(px(18.))
        .rounded_full()
        .bg(gpui::white())
        .shadow_sm()
        // While pressed the knob stretches toward where it is going, as on iOS.
        .when(live, |d| {
            d.group_active(group.clone(), move |style| {
                let style = style.w(px(23.));
                if on {
                    style.left(px(SWITCH_ON - 5.))
                } else {
                    style
                }
            })
        });
    use gpui::AnimationExt as _;
    let knob = match switch_motion(id, on) {
        Some(generation) => knob
            .with_animation(
                ("switch-slide", generation),
                gpui::Animation::new(std::time::Duration::from_millis(240))
                    .with_easing(gpui::ease_out_quint()),
                move |knob, t| {
                    let (from, to) = if on {
                        (SWITCH_OFF, SWITCH_ON)
                    } else {
                        (SWITCH_ON, SWITCH_OFF)
                    };
                    knob.left(px(from + (to - from) * t))
                },
            )
            .into_any_element(),
        None => knob.into_any_element(),
    };
    div()
        .id(("setting", id))
        .group(group)
        .relative()
        .w(px(40.))
        .h(px(22.))
        .rounded_full()
        .bg(if on { accent() } else { content(0.16) })
        .when(!live, |d| d.opacity(0.45))
        .child(knob)
}

fn control_view<V: 'static>(ctl: Ctl, ui: &mut Ui<'_, V>, cx: &mut Context<V>) -> AnyElement {
    let (save, run) = (ui.save, ui.run);
    match ctl {
        Ctl::Read(value) => div()
            .text_size(px(12.5))
            .text_color(content(0.6))
            .child(value)
            .into_any_element(),
        Ctl::Path {
            key,
            text,
            detected,
        } => path_view(key, text, detected, ui, cx),
        Ctl::Shortcut { id, combo, custom } => shortcut_view(id, combo, custom, ui, cx),
        Ctl::Field {
            key,
            text,
            width,
            button,
        } => field_view(key, text, width, button, ui, cx),
        Ctl::Button {
            label,
            act,
            enabled,
            danger,
        } => {
            let button = div()
                .id(("setting", next_id(&mut ui.ids)))
                .h(px(28.))
                .px(px(10.))
                .rounded(px(crate::theme::RADIUS_SM))
                .border_1()
                .border_color(content(0.12))
                .flex()
                .items_center()
                .text_size(px(12.5))
                .text_color(if danger {
                    Hsla::from(rgb(0xe08a8a))
                } else {
                    content(0.85)
                })
                .child(label);
            if enabled {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(content(0.08)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if danger {
                            ask(this, label, &act, run, cx);
                        } else {
                            fire(this, &act, save, run, cx);
                        }
                    }))
                    .into_any_element()
            } else {
                button.opacity(0.45).into_any_element()
            }
        }
        Ctl::Switch { on, change } => {
            let live = change.is_some();
            let id = next_id(&mut ui.ids);
            let track = switch_track(id, on, live);
            match change {
                Some(act) => track
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| fire(this, &act, save, run, cx)))
                    .into_any_element(),
                None => track.into_any_element(),
            }
        }
        Ctl::Pick(options) => {
            let mut items = Vec::new();
            for opt in options {
                let Opt {
                    label,
                    chosen,
                    change: act,
                } = opt;
                items.push(
                    div()
                        .id(("setting", next_id(&mut ui.ids)))
                        .h(px(24.))
                        .px(px(10.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .flex()
                        .items_center()
                        .text_size(px(12.5))
                        .text_color(content(if chosen { 1. } else { 0.55 }))
                        .when(chosen, |d| d.bg(content(0.1)))
                        .when(!chosen, |d| d.hover(|s| s.text_color(content(0.9))))
                        .cursor_pointer()
                        .on_click(
                            cx.listener(move |this, _, _, cx| fire(this, &act, save, run, cx)),
                        )
                        .child(label)
                        .into_any_element(),
                );
            }
            // Wraps onto more lines when the options are many (the font lists).
            div()
                .flex()
                .flex_wrap()
                .max_w(px(440.))
                .p(px(2.))
                .rounded(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_color(content(0.1))
                .children(items)
                .into_any_element()
        }
    }
}

// Pages. Each is a table of rows over AppSettings or over loaded backend data.

/// The providers in the order of `src/lib/provider-registry.ts`: (id, name).
const PROVIDER_LIST: &[(&str, &str)] = &[
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("cursor", "Cursor"),
    ("grok", "Grok"),
    ("opencode", "OpenCode"),
    ("antigravity", "Antigravity"),
    ("droid", "Droid"),
    ("pi", "Pi"),
    ("devin", "Devin"),
    ("hermes", "Hermes"),
];

fn provider_name(id: &str) -> String {
    PROVIDER_LIST
        .iter()
        .find(|(known, _)| *known == id)
        .map_or_else(|| id.to_string(), |(_, name)| name.to_string())
}

/// The name a skill's source origin shows as: the shared folder, Sirus Code or a provider.
fn origin_label(origin: &str) -> String {
    match origin {
        "agents" => t("Compartilhadas (.agents)").to_string(),
        "sirus" => "Sirus".to_string(),
        other => provider_name(other),
    }
}

fn general(s: &Value) -> Vec<Group> {
    let provider = provider_name(&current(s, top("defaultAgent")));
    let sidebar_rows: &[SwitchRow] = &[(
        t("Trabalhando e não lidas primeiro"),
        t(
            "Sessões trabalhando, esperando você ou concluídas sem serem vistas ficam acima da ordem das sessões. As listas não se mexem enquanto o ponteiro está sobre elas.",
        ),
        top("sidebarAttentionFirst"),
    )];
    let startup_rows: &[SwitchRow] = &[
        (
            t("Reabrir último projeto"),
            t("Reabre o último projeto ativo ao iniciar o Sirus."),
            top("openLastProject"),
        ),
        (
            t("Reabrir sessão mais nova"),
            t(
                "Reabre a sessão criada mais recentemente no último projeto. As outras sessões continuam na barra lateral; os agentes não iniciam automaticamente.",
            ),
            top("restorePreviousSessions"),
        ),
    ];
    let application_rows: &[SwitchRow] = &[
        (
            t("Confirmar fechamento de sessões em execução"),
            t("Pede confirmação antes de fechar ou sair enquanto os agentes estão em execução."),
            top("confirmCloseRunning"),
        ),
        (
            t("Retomar sozinho quando o limite de uso liberar"),
            t(
                r#"Uma conversa parada pelo limite da IA envia "Continue from where you left off." quando o limite libera. Dá para cancelar em cada aviso."#,
            ),
            top("usageAutoResume"),
        ),
    ];
    let environment_rows: &[SwitchRow] = &[
        (
            t("Abrir por padrão"),
            t(
                "Abre o painel Ambiente automaticamente. Quando desativado, ele fica fechado até você abrir. A última ação de abrir/fechar também atualiza esta preferência.",
            ),
            top("environmentPanelDefaultOpen"),
        ),
        (
            t("Repositório"),
            t("Exiba o link do repositório no painel Ambiente."),
            top("showEnvironmentRepository"),
        ),
        (
            "Editor",
            t("Exiba as ações do editor no painel Ambiente."),
            top("showEnvironmentEditor"),
        ),
    ];
    let core = vec![
        row(
            t("Provedor padrão"),
            t("Escolha o provedor das novas sessões. As sessões atuais mantêm seu provedor."),
            vec![Ctl::Read(provider)],
        ),
        row(
            t("Novas sessões"),
            t("Escolha o workspace padrão das novas sessões."),
            vec![pick(
                s,
                top("defaultSessionWorkspace"),
                &[
                    ("ask", t("Perguntar sempre")),
                    ("checkout", "Local"),
                    ("worktree", t("Novo worktree")),
                ],
            )],
        ),
        row(
            t("Permissões padrão"),
            t(
                "O modo de aprovação com que novas conversas começam. Um provedor sem ele usa o próximo modo mais seguro que oferece; uma conversa mantém o último modo usado.",
            ),
            vec![pick(
                s,
                top("defaultApproval"),
                &[
                    ("ask", t("Solicitar aprovação")),
                    ("auto", t("Aprovar por mim")),
                    ("full", t("Acesso completo")),
                ],
            )],
        ),
        row(
            t("Idioma"),
            "",
            vec![pick(
                s,
                top("locale"),
                &[("pt-BR", "Português (Brasil)"), ("en", "English")],
            )],
        ),
    ];
    let mut sidebar = vec![
        row(
            t("Ordem dos projetos"),
            t("Projetos fixados ficam primeiro. Arrastar uma pasta ativa a ordem manual."),
            vec![pick(
                s,
                top("sidebarProjectSortOrder"),
                &[("manual", "Manual"), ("created_at", t("Criação"))],
            )],
        ),
        row(
            t("Ordem das sessões"),
            t("Organiza as sessões dentro de cada projeto. As fixadas mantêm sua própria área."),
            vec![pick(
                s,
                top("sidebarThreadSortOrder"),
                &[
                    ("updated_at", t("Atividade recente")),
                    ("created_at", t("Criação")),
                ],
            )],
        ),
    ];
    sidebar.extend(switches(s, sidebar_rows));
    vec![
        group(t("Preferências padrão"), core),
        group(t("Organização da sidebar"), sidebar),
        group(t("Inicialização"), switches(s, startup_rows)),
        group(t("Aplicativo"), {
            let mut rows = switches(s, application_rows);
            rows.push(first_run_row());
            rows
        }),
        group(t("Terminal"), vec![terminal_placement_row()]),
        group(t("Painel Ambiente"), switches(s, environment_rows)),
    ]
}

/// Plays the first run again (first_run.rs).
fn first_run_row() -> Row {
    row(
        t("Boas-vindas"),
        t("Mostra de novo a primeira entrada: seus agentes, um projeto e o visual do Sirus."),
        vec![Ctl::Button {
            label: t("Ver boas-vindas de novo"),
            act: act_run("native_first_run", json!({})),
            enabled: true,
            danger: false,
        }],
    )
}

/// Where the terminal opens. Kept in its own file, not in the host's settings, so it is a
/// `native_terminal_placement` command rather than a saved key.
fn terminal_placement_row() -> Row {
    use crate::terminal_place::{Placement, current};
    let now = current();
    row(
        t("Posição do terminal"),
        t(
            "Embaixo, no painel lateral ou como abas na barra de título. ⌘J abre um terminal e ⇧⌘J divide à direita.",
        ),
        vec![Ctl::Pick(
            Placement::ALL
                .iter()
                .map(|p| Opt {
                    label: t(p.label()),
                    chosen: *p == now,
                    change: Act::Run {
                        cmd: "native_terminal_placement",
                        args: json!({ "placement": p.id() }),
                    },
                })
                .collect(),
        )],
    )
}

fn chat(s: &Value) -> Vec<Group> {
    let conversation_rows: &[SwitchRow] = &[
        (
            t("Recolher turnos terminados"),
            t(
                r#"Quando uma resposta termina, os passos dela se recolhem na linha "Trabalhou por". Desligue para manter todos os passos visíveis."#,
            ),
            top("foldFinishedTurns"),
        ),
        (
            t("Mostrar sessões trabalhando"),
            t(
                "Lista as sessões em que um agente está trabalhando ao lado do campo de mensagem, quando duas ou mais rodam ao mesmo tempo.",
            ),
            top("showWorkingPanel"),
        ),
        (
            t("Autocorreção"),
            t(
                "Sublinhado de ortografia e correções automáticas do macOS enquanto você escreve uma mensagem, em todas as conversas.",
            ),
            top("composerAutocorrect"),
        ),
        (
            t("Abas de conversa"),
            t(
                "Mostra as conversas do projeto como abas no topo da janela. Desligado, a barra mostra só a conversa atual.",
            ),
            top("showSessionTabs"),
        ),
        (
            t("Continuar conversas interrompidas após uma falha"),
            t(
                "Se o Sirus fechar inesperadamente enquanto um agente trabalha, a conversa continua na próxima abertura, pedindo que o agente confira primeiro o que já aconteceu. No máximo três vezes seguidas; conversas paradas, adiadas ou aguardando você ficam como estão.",
            ),
            top("continueInterruptedSessions"),
        ),
        (
            t("Compactar conversas paradas do Claude antes do cache expirar (beta)"),
            t(
                "Uma conversa do Claude com mais de 200 mil tokens parada compacta uns dois minutos antes do cache do prompt expirar (1 hora na assinatura, 5 minutos com chave de API), assim ao voltar a conversa inteira não é reenviada. Aparece como um turno /compact; conversas adiadas ficam como estão.",
            ),
            top("compactIdleBeforeCacheExpiry"),
        ),
        (
            t("Iniciar uma sessão nova quando o contexto estiver quase cheio"),
            t(
                "Quando uma conversa do Claude ou do Codex chega a 80% da janela de contexto, a próxima mensagem inicia uma sessão nova do provedor com um resumo curto montado pelo próprio app: as duas últimas trocas palavra por palavra e uma linha para cada anterior. Nenhuma chamada ao modelo é gasta com isso, e a conversa completa continua aqui.",
            ),
            top("rotateAtContextLimit"),
        ),
    ];
    let diffs_rows: &[SwitchRow] = &[(
        t("Quebrar linhas nos diffs"),
        t("Linhas longas quebram em vez de rolar para o lado."),
        top("diffWordWrap"),
    )];
    let recording_rows: &[SwitchRow] = &[
        (
            t("Mostrar teclas"),
            t(
                "Teclas digitadas na página aparecem embaixo do vídeo. Campos de senha nunca aparecem.",
            ),
            top("browserRecordingKeys"),
        ),
        (
            t("Mostrar cliques"),
            t("Um círculo marca cada clique no vídeo."),
            top("browserRecordingClicks"),
        ),
    ];
    let mut conversation = vec![
        row(
            t("Mensagem durante a resposta"),
            t(
                "Escolha se uma mensagem enviada enquanto o agente trabalha espera na fila ou instrui a resposta atual (Codex e Claude). ⌘Enter faz o contrário só naquela mensagem.",
            ),
            vec![pick(
                s,
                top("steerWhileRunning"),
                &[("false", t("Fila")), ("true", t("Instruir"))],
            )],
        ),
        row(
            t("Enter durante o ditado"),
            t(
                "Parar e transcrever no composer, ou parar e enviar a mensagem assim que for transcrita.",
            ),
            vec![pick(
                s,
                top("dictationEnterSends"),
                &[("false", t("Parar")), ("true", t("Parar e enviar"))],
            )],
        ),
    ];
    conversation.extend(switches(s, conversation_rows));
    let mut agent = vec![row(
        t("Exibição da resposta"),
        t("Como o texto da resposta aparece enquanto o agente escreve."),
        vec![pick(
            s,
            top("responseStreaming"),
            &[
                ("turn", t("Depois do turno")),
                ("paragraph", t("Parágrafos prontos")),
                ("token", t("Palavra por palavra")),
            ],
        )],
    )];
    agent.extend(switches(
        s,
        &[(
            t("Acesso dos agentes ao navegador"),
            t("Permite que agentes usem o navegador do app. Desligado, novos agentes não recebem as ferramentas dele e os que estão rodando são recusados; seu painel de navegador não muda."),
            top("agentBrowserAccess"),
        )],
    ));
    let mut review = vec![
        row(
            t("Abrir links no"),
            t(
                "Onde abrem os links de respostas, das suas mensagens e dos terminais. ⌘, Ctrl, Option ou clique do meio sempre abrem o navegador do sistema.",
            ),
            vec![pick(
                s,
                top("linkOpenTarget"),
                &[
                    ("app", t("Navegador do Sirus")),
                    ("system", t("Navegador do sistema")),
                ],
            )],
        ),
        row(
            t("Abrir pull requests e issues"),
            t(
                "Onde abre um link de pull request ou issue de um dos seus projetos. Outros repositórios seguem Abrir links no.",
            ),
            vec![pick(
                s,
                top("githubLinksInApp"),
                &[("true", t("No Sirus")), ("false", t("Navegador"))],
            )],
        ),
    ];
    review.extend(switches(s, diffs_rows));
    let mut recording = vec![row(
        t("Quadros por segundo"),
        t(
            "60 fps fica mais suave e gera arquivos maiores. A página é capturada o mais rápido que ela permitir.",
        ),
        vec![pick(
            s,
            top("browserRecordingFps"),
            &[("30", "30 fps"), ("60", "60 fps")],
        )],
    )];
    recording.extend(switches(s, recording_rows));
    let mut safety = switches(
        s,
        &[(
            t("Confirmar ao fechar um terminal"),
            t("Pergunta antes de fechar uma aba do terminal; o shell e o histórico dela terminam."),
            top("confirmTerminalClose"),
        )],
    );
    safety.push(row(
        t("Confirmar ao excluir"),
        t("Sempre ligado: excluir uma conversa apaga o histórico dela."),
        vec![fixed(true)],
    ));
    vec![
        group(t("Conversa"), conversation),
        group(t("Comportamento do agente"), agent),
        group(t("Revisão"), review),
        group(
            t("Navegador"),
            switches(
                s,
                &[(
                    t("Mostrar páginas que o agente abre"),
                    t(
                        "Uma página que o agente abre ou navega aparece num player pequeno sobre a conversa. Ela fica oculta enquanto o painel do navegador mostra a sessão.",
                    ),
                    top("browserShowAgentPages"),
                )],
            ),
        ),
        group(t("Gravações do navegador"), recording),
        group(t("Confirmações de segurança"), safety),
    ]
}

fn notifications(s: &Value) -> Vec<Group> {
    const SOUNDS: &[(&str, &str)] = &[
        ("glass", "Glass"),
        ("ping", "Ping"),
        ("pop", "Pop"),
        ("submarine", "Submarine"),
        ("tink", "Tink"),
        ("hero", "Hero"),
    ];
    let activity_rows: &[SwitchRow] = &[
        (
            t("Avisos no app"),
            t("Mostre um aviso para sessões que você não está visualizando."),
            inner("notifications", "toasts"),
        ),
        (
            t("Notificações do sistema"),
            "",
            inner("notifications", "system"),
        ),
        (
            t("Sons de notificação"),
            t("Toque o som escolhido. Usa o volume de saída do sistema."),
            inner("notifications", "sounds"),
        ),
        (
            t("Avisar com o app em foco"),
            t("Mostre notificações e toque sons também enquanto o Sirus estiver ativo."),
            inner("notifications", "foreground"),
        ),
        (
            t("Sons"),
            t(
                "Um toque suave quando um agente termina um turno ou precisa de você. Não toca para a conversa que você está vendo.",
            ),
            top("agentSounds"),
        ),
    ];
    // Each event: its sound, and its switch.
    let event = |s: &Value,
                 title: &'static str,
                 about: &'static str,
                 on: &'static str,
                 sound: &'static str| {
        row(
            title,
            about,
            vec![
                pick(s, inner("notifications", sound), SOUNDS),
                Ctl::Button {
                    label: t("Ouvir"),
                    act: act_run(
                        "native_sound",
                        json!({ "sound": current(s, inner("notifications", sound)) }),
                    ),
                    enabled: true,
                    danger: false,
                },
                switch_of(s, inner("notifications", on)),
            ],
        )
    };
    let mut events = vec![
        event(
            s,
            t("Pedidos de permissão"),
            t("Quando um agente precisa de aprovação para continuar."),
            "permissions",
            "permissionSound",
        ),
        event(
            s,
            t("Perguntas do agente"),
            t("Quando um agente pede uma escolha ou resposta."),
            "questions",
            "questionSound",
        ),
        event(
            s,
            t("Tarefa concluída"),
            t("Quando uma tarefa termina com sucesso. Cancelamentos e falhas não geram avisos."),
            "completion",
            "completionSound",
        ),
    ];
    events.extend(switches(
        s,
        &[(
            "Pull requests",
            t("Quando outra pessoa abre um pull request ou pede sua revisão no repositório de um projeto salvo. Verificado ao abrir o Sirus e quando você volta para ele."),
            inner("notifications", "pullRequests"),
        )],
    ));
    vec![
        group(t("Avisos de atividade"), switches(s, activity_rows)),
        group(t("Eventos e sons"), events),
    ]
}

/// Atalhos: (id, Portuguese command, Portuguese group, default shortcut as the app stores it).
const KEYBINDINGS: &[(&str, &str, &str, &str)] = &[
    ("new-session", "Nova sessão", "Sessão", "meta+n"),
    ("palette", "Paleta de comandos", "Aplicativo", "meta+k"),
    ("open-project", "Abrir projeto", "Projeto", "meta+o"),
    ("settings", "Configurações", "Aplicativo", "meta+,"),
    (
        "toggle-sidebar",
        "Mostrar / ocultar barra lateral",
        "Visualização",
        "meta+s",
    ),
    (
        "toggle-context",
        "Alternar painel direito",
        "Visualização",
        "meta+\\",
    ),
    ("open-terminal", "Abrir terminal", "Visualização", "meta+`"),
    ("stop-agent", "Parar agente", "Sessão", "meta+."),
    ("back", "Voltar", "Aplicativo", "meta+["),
    ("forward", "Avançar", "Aplicativo", "meta+]"),
    ("open-files", "Abrir Arquivos", "Visualização", "meta+alt+o"),
    (
        "open-browser",
        "Abrir Navegador",
        "Visualização",
        "meta+alt+b",
    ),
    (
        "toggle-environment",
        "Mostrar/ocultar Ambiente",
        "Visualização",
        "meta+alt+e",
    ),
    (
        "find-in-conversation",
        "Buscar nesta conversa",
        "Sessão",
        "meta+f",
    ),
    (
        "search-conversations",
        "Buscar em todas as conversas",
        "Sessão",
        "meta+shift+f",
    ),
    (
        "toggle-side-chat",
        "Abrir ou fechar o chat lateral",
        "Sessão",
        "meta+alt+s",
    ),
    (
        "send-new-thread",
        "Enviar e começar nova conversa",
        "Sessão",
        "meta+alt+enter",
    ),
    (
        "cycle-effort",
        "Alternar nível de raciocínio",
        "Sessão",
        "shift+tab",
    ),
    (
        "reopen-closed",
        "Reabrir guia fechada",
        "Visualização",
        "meta+shift+t",
    ),
];

const SHORTCUT_GROUPS: &[&str] = &["Sessão", "Aplicativo", "Projeto", "Visualização"];

/// The shortcut as the app shows it (lilac ⌘ ⌥ ⇧ glyphs), as `shortcutLabel` in keybindings.ts.
fn shortcut_label(combo: &str) -> String {
    let mut s = combo.to_string();
    if let Some(rest) = s.strip_suffix("enter") {
        s = format!("{rest}↩");
    }
    if let Some(rest) = s.strip_suffix("tab") {
        s = format!("{rest}⇥");
    }
    s = s
        .replacen("meta+", "⌘", 1)
        .replacen("alt+", "⌥", 1)
        .replacen("shift+", "⇧", 1);
    if let Some(rest) = s.strip_prefix('⌘') {
        let (alt, rest) = rest.strip_prefix('⌥').map_or(("", rest), |r| ("⌥", r));
        let (shift, rest) = rest.strip_prefix('⇧').map_or(("", rest), |r| ("⇧", r));
        s = format!("{alt}{shift}⌘{rest}");
    }
    s.to_uppercase()
}

fn keybindings(s: &Value) -> Vec<Group> {
    let custom = s.get("customShortcuts");
    SHORTCUT_GROUPS
        .iter()
        .map(|&name| {
            let rows = KEYBINDINGS
                .iter()
                .filter(|entry| entry.2 == name)
                .map(|&(id, label, _, combo)| {
                    let saved = custom.and_then(|c| c.get(id)).and_then(Value::as_str);
                    row(
                        t(label),
                        "",
                        vec![Ctl::Shortcut {
                            id,
                            combo: saved.unwrap_or(combo).to_owned(),
                            custom: saved.is_some(),
                        }],
                    )
                })
                .collect();
            group(t(name), rows)
        })
        .filter(|item| !item.rows.is_empty())
        .collect()
}

fn git(s: &Value, d: &HashMap<&'static str, Value>) -> Vec<Group> {
    let host = get_data(d, "host");
    vec![
        group(
            t("Instalação"),
            vec![
                row(
                    t("Git detectado"),
                    "",
                    vec![Ctl::Read(if yes(&host["gitDetected"]) {
                        t("Sim").to_string()
                    } else {
                        t("Não").to_string()
                    })],
                ),
                row(
                    t("Versão"),
                    "",
                    vec![Ctl::Read(
                        host["gitVersion"].as_str().unwrap_or("—").to_string(),
                    )],
                ),
                row(
                    t("Executável"),
                    "",
                    vec![Ctl::Read(
                        host["gitPath"].as_str().unwrap_or("—").to_string(),
                    )],
                ),
            ],
        ),
        github_accounts(d),
        group(
            t("Comportamento"),
            vec![
                row(
                    t("Atualizar referências ao abrir projeto"),
                    t(
                        "O fetch automático está indisponível enquanto a configuração Git do projeto puder executar helpers ou divulgar credenciais. Faça o fetch manualmente pelo terminal.",
                    ),
                    vec![Ctl::Read(t("Em breve").to_string()), fixed(false)],
                ),
                switch_row(
                    s,
                    t("Corrigir CI automaticamente"),
                    t(
                        "Quando os checks do PR aberto de uma sessão falham, o agente dela recebe os erros e corrige; depois o Sirus faz commit e push do branch da sessão (nunca forçado). Até 3 tentativas por falha; senão pausa e avisa.",
                    ),
                    top("ciAutoFix"),
                ),
                switch_row(
                    s,
                    t("Atualizar o branch padrão automaticamente"),
                    t(
                        "A cada 5 minutos, avança o checkout do projeto quando ele está no branch padrão do origin, sem mudanças locais e sem conversa rodando nele. Nunca faz merge, rebase nem mexe numa árvore com mudanças.",
                    ),
                    top("gitAutoPull"),
                ),
                switch_row(
                    s,
                    t("Permitir que sessões acompanhem o pull request"),
                    t(
                        "Uma sessão que acompanha o PR (pelo cartão Ambiente ou quando você pede ao agente) recebe um turno automático, quando está ociosa, se um check falhar, alguém revisar ou comentar o código ou o branch tiver conflitos com a base. O GitHub é lido no máximo uma vez por minuto por PR; o acompanhamento para após 5 atualizações sem um novo commit.",
                    ),
                    top("prWatch"),
                ),
                switch_row(
                    s,
                    t("Mostrar arquivos não rastreados"),
                    "",
                    top("gitShowUntracked"),
                ),
                row(
                    t("Confirmar ações destrutivas do Git"),
                    t(
                        "Proteção permanente. O Sirus não executa operações destrutivas do Git automaticamente.",
                    ),
                    vec![fixed(true)],
                ),
            ],
        ),
    ]
}

/// Settings → Git → GitHub accounts (ADR-188), through the GitHub CLI: listing, switching, signing
/// out (a second click) and adding an account with the device code shown here. The shell keeps
/// `github` (the accounts), `github_device` ({code, url} while a sign-in waits) and
/// `github_error` (the text of a failed sign-in) in its data.
fn github_accounts(d: &HashMap<&'static str, Value>) -> Group {
    let github = get_data(d, "github");
    let device = get_data(d, "github_device");
    let mut rows = vec![row(
        "",
        t(
            "Pull requests, acompanhamento de PRs, correção de CI e pushes por HTTPS usam a conta ativa. As contas ficam guardadas no GitHub CLI (gh).",
        ),
        Vec::new(),
    )];
    if yes(&github["missing"]) {
        rows.push(row(
            "",
            t("O GitHub CLI (gh) não está instalado. Instale com `brew install gh` e depois adicione uma conta aqui."),
            Vec::new(),
        ));
    } else {
        let accounts = github["accounts"].as_array().cloned().unwrap_or_default();
        if accounts.is_empty() {
            rows.push(row("", t("Nenhuma conta do GitHub conectada."), Vec::new()));
        }
        for account in &accounts {
            let login = account["login"].as_str().unwrap_or_default();
            let about = if yes(&account["valid"]) {
                ""
            } else {
                t("Login expirado")
            };
            let mut controls = Vec::new();
            if yes(&account["active"]) {
                controls.push(Ctl::Read(t("Ativa").to_string()));
            } else {
                controls.push(Ctl::Button {
                    label: t("Usar esta conta"),
                    act: act_run("native_github_switch", json!({"login": login})),
                    enabled: true,
                    danger: false,
                });
            }
            controls.push(confirm_button(
                t("Sair"),
                act_run("native_github_logout", json!({"login": login})),
            ));
            let mut item = row(login, about, controls);
            item.mark = Some("github");
            rows.push(item);
        }
        if let Some(code) = device["code"].as_str() {
            rows.push(row(
                t("Código de confirmação"),
                tf(
                    "Abra {} e digite o código. Esta tela termina quando o GitHub confirmar.",
                    &[&device["url"].as_str().unwrap_or("github.com/login/device")],
                ),
                vec![
                    Ctl::Read(code.to_string()),
                    Ctl::Button {
                        label: t("Cancelar"),
                        act: act_run("native_github_cancel", json!({})),
                        enabled: true,
                        danger: false,
                    },
                ],
            ));
        } else {
            rows.push(row(
                t("Adicionar conta"),
                "",
                vec![Ctl::Button {
                    label: t("Adicionar conta"),
                    act: act_run("native_github_login", json!({})),
                    enabled: true,
                    danger: false,
                }],
            ));
            if let Some(error) = get_data(d, "github_error").as_str() {
                rows.push(row("", error, Vec::new()));
            }
        }
    }
    group(t("Contas do GitHub"), rows)
}

/// A button that asks in the confirmation popup before running `act`.
fn confirm_button(label: &'static str, act: Act) -> Ctl {
    Ctl::Button {
        label,
        act,
        enabled: true,
        danger: true,
    }
}

/// Bytes as the Portuguese of the app shows them: "512 MB", "1,2 GB".
fn size_of(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    const MB: f64 = 1_000_000.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB).replace('.', ",")
    } else {
        format!("{:.0} MB", (bytes / MB).max(1.0))
    }
}

fn worktrees(s: &Value, d: &HashMap<&'static str, Value>) -> Vec<Group> {
    let host = get_data(d, "host");
    let custom = current(s, top("worktreeLocation")) == "custom";
    let mut defaults = vec![
        row(
            t("Workspace padrão da sessão"),
            "",
            vec![pick(
                s,
                top("defaultSessionWorkspace"),
                &[
                    ("ask", t("Perguntar sempre")),
                    ("checkout", t("Checkout atual")),
                    ("worktree", t("Novo worktree")),
                ],
            )],
        ),
        switch_row(
            s,
            t("Iniciar novos worktrees a partir do origin"),
            t(
                "Busca o branch padrão do origin e baseia novos worktrees nele, e não no branch local. Sem origin ou sem rede, eles começam do branch local.",
            ),
            top("worktreesFromOrigin"),
        ),
        row(
            t("Local dos worktrees"),
            "",
            vec![pick(
                s,
                top("worktreeLocation"),
                &[
                    ("automatic", t("Automático")),
                    ("custom", t("Personalizado")),
                ],
            )],
        ),
    ];
    if custom {
        defaults.push(row(
            t("Diretório personalizado"),
            t("Pasta onde os worktrees são criados. Escolha uma pasta ou digite o caminho."),
            vec![
                Ctl::Field {
                    key: "worktreeBasePath",
                    text: current(s, top("worktreeBasePath")),
                    width: 220.,
                    button: None,
                },
                Ctl::Button {
                    label: t("Escolher pasta"),
                    act: act_run("native_pick_folder", json!({"key": "worktreeBasePath"})),
                    enabled: true,
                    danger: false,
                },
            ],
        ));
    } else {
        defaults.push(row(
            t("Diretório automático"),
            host["worktreeRoot"]
                .as_str()
                .unwrap_or("Application Support / worktrees"),
            Vec::new(),
        ));
    }
    defaults.push(row(
        t("Padrão do nome da branch"),
        t("Variáveis: {session-name} {id}. O identificador mantém branches únicas."),
        vec![Ctl::Field {
            key: "worktreeBranchPattern",
            text: current(s, top("worktreeBranchPattern")),
            width: 220.,
            button: None,
        }],
    ));
    // Sessions that ended with their own isolated worktree (the `delete_session` of the Tauri build).
    let finished: Vec<String> = get_data(d, "appState")["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|session| {
            session["worktree"]["isolated"].as_bool() == Some(true)
                && matches!(
                    session["status"].as_str(),
                    Some("completed" | "failed" | "stopped")
                )
        })
        .filter_map(|session| session["id"].as_str().map(str::to_string))
        .collect();
    let finished_control = if finished.is_empty() {
        Ctl::Button {
            label: t("Nenhum para limpar"),
            act: act_run("", json!({})),
            enabled: false,
            danger: false,
        }
    } else {
        confirm_button(
            t("Limpar worktrees"),
            act_run("native_delete_sessions", json!({"ids": finished})),
        )
    };
    // Leftover folders (`worktree_cleanup_action`'s scan, loaded with the page): the clean removes
    // only those with no uncommitted changes, and never a branch.
    let scan = get_data(d, "worktreeScan");
    let removable = scan["leftovers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["kept"].is_null())
        .count();
    let kept = scan["leftovers"].as_array().map_or(0, Vec::len) - removable;
    let freed = size_of(scan["reclaimableBytes"].as_u64().unwrap_or(0));
    let mut leftover_controls = Vec::new();
    if kept > 0 {
        leftover_controls.push(Ctl::Read(tf("{} mantidas", &[&kept])));
    }
    leftover_controls.push(if removable == 0 {
        Ctl::Button {
            label: t("Nada sobrando"),
            act: act_run("", json!({})),
            enabled: false,
            danger: false,
        }
    } else {
        confirm_button(
            t("Liberar espaço"),
            act_run(
                "worktree_cleanup_action",
                json!({"action": {"type": "clean", "confirm": true}}),
            ),
        )
    });
    vec![
        group(t("Padrões"), defaults),
        group(
            t("Limpeza"),
            vec![
                row(
                    t("Limpar worktrees concluídos"),
                    t(
                        "Remove sessões encerradas e seus worktrees isolados. Worktrees com alterações são preservados.",
                    ),
                    vec![finished_control],
                ),
                row(
                    t("Limpar worktrees que sobraram"),
                    tf(
                        "Pastas no local dos worktrees que nenhuma sessão usa. Pastas com alterações não commitadas ficam; branches nunca são apagadas.{}",
                        &[&if removable > 0 {
                            tf(" Liberam {}.", &[&freed])
                        } else {
                            String::new()
                        }],
                    ),
                    leftover_controls,
                ),
            ],
        ),
    ]
}

fn terminal(s: &Value, d: &HashMap<&'static str, Value>) -> Vec<Group> {
    let host = get_data(d, "host");
    vec![
        group(
            "Shell",
            vec![
                row(
                    t("Shell padrão"),
                    "",
                    vec![Ctl::Read(host["shell"].as_str().unwrap_or("—").to_string())],
                ),
                row(
                    t("Usar shell padrão do sistema"),
                    t("O terminal usa sempre o shell configurado no sistema."),
                    vec![fixed(true)],
                ),
                row(
                    t("Diretório de trabalho"),
                    "",
                    vec![Ctl::Read(t("Workspace da sessão").to_string())],
                ),
            ],
        ),
        group(
            t("Exibição"),
            vec![
                row(
                    t("Estilo do cursor"),
                    "",
                    vec![pick(
                        s,
                        top("terminalCursorStyle"),
                        &[
                            ("block", t("Bloco")),
                            ("bar", t("Barra")),
                            ("underline", t("Sublinhado")),
                        ],
                    )],
                ),
                row(
                    t("Limite do histórico"),
                    "",
                    vec![pick(
                        s,
                        top("terminalScrollback"),
                        &[("2000", "2000"), ("5000", "5000"), ("10000", "10000")],
                    )],
                ),
            ],
        ),
    ]
}

/// A folder button: `Abrir` runs `native_open_path`; dimmed when there is no folder to open.
fn open_button(path: String, disabled: bool) -> Ctl {
    Ctl::Button {
        label: t("Abrir"),
        act: act_run("native_open_path", json!({"path": path})),
        enabled: !disabled,
        danger: false,
    }
}

/// The row that says where updates stand and offers Buscar atualização, then Baixar e instalar or
/// Reiniciar agora (app_update.rs; the restart asks first when agents are running).
fn update_row() -> Row {
    use crate::app_update::{Phase, status};
    let u = status();
    let version = u.version.clone().unwrap_or_default();
    let about = match u.phase {
        Phase::Idle => tf("Versão {}.", &[&u.current]),
        Phase::Checking => t("Buscando atualização…").to_string(),
        Phase::UpToDate => tf("Você está na versão mais recente ({}).", &[&u.current]),
        Phase::Available => tf("A versão {} está disponível.", &[&version]),
        Phase::Downloading => tf("Baixando a versão {}…", &[&version]),
        Phase::Installed => tf(
            "A versão {} está instalada. Ela começa quando você reiniciar ou abrir o app de novo.",
            &[&version],
        ),
        Phase::Failed => tf(
            "Não foi possível atualizar: {}",
            &[&u.error.as_deref().unwrap_or_default()],
        ),
    };
    let idle = !matches!(u.phase, Phase::Checking | Phase::Downloading);
    let button = |label: &'static str, kind: &str, enabled: bool| Ctl::Button {
        label,
        act: act_run("native_app_update", json!({ "type": kind })),
        enabled,
        danger: false,
    };
    let mut controls = vec![button(t("Buscar atualização"), "check", idle)];
    match u.phase {
        Phase::Available => controls.push(button(t("Baixar e instalar"), "install", true)),
        Phase::Installed => controls.push(button(t("Reiniciar agora"), "restart", true)),
        _ => {}
    }
    row(t("Atualizações"), about, controls)
}

fn advanced(s: &Value, d: &HashMap<&'static str, Value>) -> Vec<Group> {
    let host = get_data(d, "host");
    let data_dir = host["dataDir"].as_str().unwrap_or_default();
    let auto_check = flag(s, top("appUpdateAutoCheck"));
    let download_field = top("appUpdateAutoDownload");
    // Like the React page, the download switch waits on the automatic check.
    let download = row(
        t("Baixar atualizações em segundo plano"),
        t(
            "A versão nova é baixada e instalada e espera você reiniciar. Se você não reiniciar, ela entra na próxima vez que abrir o app.",
        ),
        vec![Ctl::Switch {
            on: flag(s, download_field),
            change: auto_check.then(|| change(s, download_field, json!(!flag(s, download_field)))),
        }],
    );
    vec![
        group(
            t("Sobre"),
            vec![
                switch_row(
                    s,
                    t("Procurar atualizações automaticamente"),
                    t("Ao abrir o Sirus e a cada poucas horas."),
                    top("appUpdateAutoCheck"),
                ),
                download,
                update_row(),
                switch_row(
                    s,
                    t("Mostrar o que há de novo depois de atualizar"),
                    t("Uma vez, na primeira abertura de cada versão."),
                    top("appUpdateWhatsNew"),
                ),
            ],
        ),
        group(
            t("Diagnóstico"),
            vec![
                switch_row(
                    s,
                    t("Logs de desenvolvimento"),
                    t("Salva contagens limitadas do ciclo de execução localmente. Prompts, output, paths e credenciais são excluídos."),
                    top("developerLogs"),
                ),
                row(
                    t("Abrir diretório de dados do Sirus"),
                    host["dataDir"].as_str().unwrap_or_default(),
                    vec![open_button(data_dir.to_string(), data_dir.is_empty())],
                ),
                row(
                    t("Abrir pasta de logs"),
                    "",
                    vec![open_button(
                        format!("{data_dir}/logs"),
                        data_dir.is_empty() || !flag(s, top("developerLogs")),
                    )],
                ),
                row(
                    t("Restaurar layout da janela"),
                    t("Restaura a barra lateral e as larguras dos painéis."),
                    vec![Ctl::Button {
                        label: t("Restaurar"),
                        act: act_run("native_reset_layout", json!({})),
                        enabled: true,
                        danger: false,
                    }],
                ),
                row(
                    t("Componentes UI Arc"),
                    t("Explore todos os 100 componentes gratuitos com exemplos locais."),
                    vec![Ctl::Read(
                        t("A galeria é feita de componentes React da janela web; esta janela nativa não a renderiza.")
                            .to_string(),
                    )],
                ),
                row(
                    t("Funcionalidades experimentais"),
                    "",
                    vec![Ctl::Read(t("Em breve").to_string()), fixed(false)],
                ),
            ],
        ),
        group(
            t("Restaurar preferências"),
            // `save_settings` with only the default agent: the host fills every other field with its
            // default (AppSettings is `serde(default)`), as the Tauri page does.
            vec![row(
                t("Restaurar configurações do Sirus"),
                t("Restaurar preferências não apaga pastas de projetos. Requer confirmação."),
                vec![confirm_button(
t("Restaurar"),
                act_run(
                        "save_settings",
                        json!({"settings": {"defaultAgent": s["defaultAgent"]}}),
                    ),
                )],
            )],
        ),
    ]
}

/// The providers in the order of the registry, with their installs and the enable switch.
/// Each installed CLI's path, version, test and login, with a button that copies them
/// (cli_diagnostics.rs). The checks follow Verificar CLIs and the page's opening.
fn diagnostics_rows(d: &HashMap<&'static str, Value>) -> Vec<Row> {
    use crate::cli_diagnostics::{self, Probe};
    let entries = cli_diagnostics::entries(
        get_data(d, "agents"),
        get_data(d, "cliProbes"),
        get_data(d, "cliLogins"),
    );
    let rows: Vec<Row> = entries
        .into_iter()
        .filter(|(_, entry)| !matches!(entry.probe, Probe::Absent))
        .map(|(_, entry)| {
            let copy = cli_diagnostics::copy_text(&entry);
            // Text in the row's left column: as controls, long lines squeezed it to one letter wide.
            let path = entry
                .path
                .clone()
                .unwrap_or_else(|| t("Nenhum executável").to_owned());
            let version = entry.version.clone().unwrap_or_else(|| "—".to_owned());
            let mut about = format!("{version} · {path}");
            if matches!(entry.probe, Probe::Failed(_)) {
                about.push('\n');
                about.push_str(&cli_diagnostics::probe_text(&entry.probe));
            }
            let login = cli_diagnostics::login_text(&entry.login);
            if login != "—" {
                about.push('\n');
                about.push_str(&login);
            }
            row(
                entry.name.clone(),
                about,
                vec![Ctl::Button {
                    label: t("Copiar"),
                    act: act_run("native_copy", json!({ "text": copy })),
                    enabled: true,
                    danger: false,
                }],
            )
        })
        .collect();
    if rows.is_empty() {
        vec![row("", t("Nenhuma CLI detectada ainda."), Vec::new())]
    } else {
        rows
    }
}

fn providers(s: &Value, d: &HashMap<&'static str, Value>) -> Vec<Group> {
    let agents = get_data(d, "agents");
    let install = |id: &str| {
        agents
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"].as_str() == Some(id)))
    };
    let disabled = strings(&s["disabledProviders"]);
    let (mut active, mut off, mut missing) = (Vec::new(), Vec::new(), Vec::new());
    for &(id, name) in PROVIDER_LIST {
        let found = install(id);
        let installed = found.and_then(|item| item["installed"].as_bool()) == Some(true);
        let mut item = if installed {
            let on = !has(&disabled, id);
            let flipped = if on {
                with(&disabled, id)
            } else {
                without(&disabled, id)
            };
            row(
                name,
                found
                    .and_then(|item| item["version"].as_str())
                    .unwrap_or(t("Instalado")),
                vec![
                    Ctl::Path {
                        key: key_of(format!("providerPath:{id}")),
                        text: s["providerPaths"][id]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        detected: found
                            .and_then(|item| item["path"].as_str())
                            .unwrap_or_default()
                            .to_string(),
                    },
                    Ctl::Switch {
                        on,
                        change: Some(Act::Save {
                            key: "disabledProviders",
                            value: json!(flipped),
                        }),
                    },
                ],
            )
        } else {
            let mut item = row(
                name,
                "",
                vec![
                    Ctl::Path {
                        key: key_of(format!("providerPath:{id}")),
                        text: s["providerPaths"][id]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        detected: String::new(),
                    },
                    Ctl::Read(t("Não instalado").to_string()),
                ],
            );
            item.faded = true;
            item
        };
        item.provider = Some(id);
        if !installed {
            missing.push(item);
        } else if has(&disabled, id) {
            off.push(item);
        } else {
            active.push(item);
        }
    }
    let (active_title, off_title, missing_title) = (
        tf("Ativos {}", &[&active.len()]),
        tf("Instalados, desligados {}", &[&off.len()]),
        tf("Não encontrados {}", &[&missing.len()]),
    );
    let mut clis: Vec<Group> = Vec::new();
    for (title, rows) in [
        (active_title, active),
        (off_title, off),
        (missing_title, missing),
    ] {
        if !rows.is_empty() {
            clis.push(group(title, rows));
        }
    }

    // Up to two providers in the sidebar's usage rings (`USAGE_PROVIDER_IDS`, registry order).
    const USAGE: &[&str] = &["codex", "claude", "cursor", "opencode"];
    const SIDEBAR_USAGE_LIMIT: usize = 2;
    let chosen = strings(&s["sidebarUsageProviders"]);
    let full = chosen.len() >= SIDEBAR_USAGE_LIMIT;
    let mut usage = Vec::new();
    for &(id, name) in PROVIDER_LIST.iter().filter(|(id, _)| USAGE.contains(id)) {
        let on = has(&chosen, id);
        let installed = install(id).and_then(|item| item["installed"].as_bool()) == Some(true);
        let blocked = !on && (full || !installed);
        let about = if !installed {
            t("Não instalado")
        } else if !on && full {
            t("No máximo dois provedores. Desligue um para escolher outro.")
        } else {
            ""
        };
        let next = if on {
            without(&chosen, id)
        } else {
            let mut picked = with(&chosen, id);
            picked.truncate(SIDEBAR_USAGE_LIMIT);
            picked
        };
        usage.push(row(
            name,
            about,
            vec![Ctl::Switch {
                on,
                change: (!blocked).then(|| Act::Save {
                    key: "sidebarUsageProviders",
                    value: json!(next),
                }),
            }],
        ));
    }
    usage.push(row(
        "",
        t("Mostra um anel de consumo de até dois provedores no rodapé da barra lateral."),
        Vec::new(),
    ));

    clis.extend([
        group(t("Diagnóstico das CLIs"), diagnostics_rows(d)),
        group(t("Consumo na barra lateral"), usage),
        group(
            t("Avançado"),
            vec![row(
                t("Manter agentes prontos"),
                t(
                    "Quantas conversas terminadas mantêm o agente aberto para responder mais rápido na próxima mensagem. Ficam até 30 minutos paradas; as paradas há mais tempo fecham primeiro.",
                ),
                vec![pick(
                    s,
                    top("keepIdleAgents"),
                    &[
                        ("0", t("Nenhuma")),
                        ("1", "1"),
                        ("2", "2"),
                        ("3", "3"),
                        ("5", "5"),
                        ("16", t("Todas")),
                    ],
                )],
            )],
        ),
        group(
            t("Atualizações"),
            vec![switch_row(
                s,
                t("Verificar atualizações das CLIs"),
                t(
                    "Verifica em segundo plano se Codex, Claude e outras CLIs têm versões mais novas.",
                ),
                top("enableProviderUpdateChecks"),
            )],
        ),
    ]);
    clis
}

/// The catalog's name for a skill's origin, as `skillSections` groups it.
fn section_title(key: &str) -> String {
    match key {
        "shared" => t("Skills compartilhadas").to_string(),
        "project" => t("Skills do projeto").to_string(),
        origin => origin_label(origin),
    }
}

/// The section a skill belongs to: project first, then shared, then its one origin.
fn section_of(skill: &Value) -> String {
    let sources = skill["sources"].as_array().cloned().unwrap_or_default();
    if sources
        .iter()
        .any(|source| source["scope"].as_str() == Some("project"))
    {
        "project".to_string()
    } else if sources.len() > 1 {
        "shared".to_string()
    } else {
        sources
            .first()
            .and_then(|source| source["origin"].as_str())
            .unwrap_or("sirus")
            .to_string()
    }
}

fn skill_row(skill: &Value, disabled: &[String]) -> Row {
    let name = skill["name"].as_str().unwrap_or_default();
    let description = skill["description"]
        .as_str()
        .filter(|text| !text.is_empty())
        .unwrap_or(t("Sem descrição informada."));
    let mut origins: Vec<String> = Vec::new();
    for source in skill["sources"].as_array().into_iter().flatten() {
        let label = origin_label(source["origin"].as_str().unwrap_or_default());
        if !origins.contains(&label) {
            origins.push(label);
        }
    }
    let on = !has(disabled, name);
    let mut flipped = if on {
        with(disabled, name)
    } else {
        without(disabled, name)
    };
    flipped.sort();
    row(
        name,
        tf(
            "{} · Encontrada em: {}",
            &[&description, &origins.join(", ")],
        ),
        vec![Ctl::Switch {
            on,
            change: Some(Act::Save {
                key: "disabledSkills",
                value: json!(flipped),
            }),
        }],
    )
}

fn skills(s: &Value, d: &HashMap<&'static str, Value>) -> Vec<Group> {
    let catalog = &get_data(d, "skills")["catalog"];
    let disabled = strings(&s["disabledSkills"]);
    let all = catalog["skills"].as_array().cloned().unwrap_or_default();
    let enabled = all
        .iter()
        .filter(|skill| !has(&disabled, skill["name"].as_str().unwrap_or_default()))
        .count();
    let mut out = vec![
        group(
            t("Skills portáteis"),
            vec![
                row(
                    t("Pasta de skills do Sirus"),
                    t(
                        "Adicione pastas com SKILL.md aqui. Skills invocadas funcionam entre provedores; a cópia do próprio provedor tem prioridade.",
                    ),
                    vec![Ctl::Read(
                        catalog["portableDir"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )],
                ),
                row(
                    "",
                    tf(
                        "{} de {} skills ativas · Em todas as origens",
                        &[&enabled, &all.len()],
                    ),
                    Vec::new(),
                ),
            ],
        ),
        note(t(
            "Os nomes e ícones dos provedores indicam onde cada cópia foi encontrada. Skills invocadas ficam disponíveis entre provedores. Os switches controlam o catálogo do Sirus; não alteram a descoberta nas CLIs externas.",
        )),
    ];
    if catalog["truncated"].as_bool() == Some(true) {
        out.push(note(t(
            "A busca atingiu seu limite. Algumas skills podem não aparecer.",
        )));
    }
    if all.is_empty() {
        out.push(group(
            "",
            vec![row(
                t("Nenhuma skill encontrada"),
                t("Adicione uma pasta com SKILL.md no diretório acima ou na pasta de skills de um provedor e atualize."),
                Vec::new(),
            )],
        ));
        return out;
    }
    // Sections: project first, then shared, then each origin in name order (`skillSections`).
    let rank = |key: &str| match key {
        "project" => 0,
        "shared" => 1,
        _ => 2,
    };
    let mut tagged: Vec<(String, &Value)> =
        all.iter().map(|skill| (section_of(skill), skill)).collect();
    tagged.sort_by_key(|(key, _)| (rank(key.as_str()), key.clone()));
    let mut keys: Vec<String> = tagged.iter().map(|(key, _)| key.clone()).collect();
    keys.dedup();
    for key in keys {
        let rows: Vec<Row> = tagged
            .iter()
            .filter(|(section, _)| *section == key)
            .map(|(_, skill)| skill_row(skill, &disabled))
            .collect();
        out.push(group(section_title(&key), rows));
    }
    out
}

fn computer(s: &Value, d: &HashMap<&'static str, Value>) -> Vec<Group> {
    let c = get_data(d, "computer");
    let shortcut = current(s, top("windowSnapShortcut"));
    let shortcut_now = match shortcut.as_str() {
        "optionShiftS" => "⌥⇧S",
        "controlShiftS" => "⌃⇧S",
        _ => "⌃⌥⌘S",
    };
    let snap_on = flag(s, top("windowSnapEnabled"));
    let mut window = vec![
        switch_row(
            s,
            t("Atalho global de captura"),
            t(
                "Em qualquer app, o atalho captura a janela da frente e anexa à conversa aberta no Sirus. Nada é enviado.",
            ),
            top("windowSnapEnabled"),
        ),
        row(
            t("Atalho"),
            t("Se outro app já usa a combinação, escolha outra."),
            vec![if snap_on {
                pick(
                    s,
                    top("windowSnapShortcut"),
                    &[
                        ("controlOptionCommandS", "⌃⌥⌘S"),
                        ("optionShiftS", "⌥⇧S"),
                        ("controlShiftS", "⌃⇧S"),
                    ],
                )
            } else {
                Ctl::Read(shortcut_now.to_string())
            }],
        ),
    ];
    if snap_on && !yes(&c["screenRecording"]) {
        window.push(permission(
            t("Gravação de Tela"),
            t("Necessária para capturar a janela de outro app."),
            false,
            "requestScreenRecording",
            "openScreenRecordingSettings",
        ));
    }

    let accessibility = yes(&c["accessibility"]);
    let screen = yes(&c["screenRecording"]);
    let mut out = vec![
        group(t("Captura de janela"), window),
        group(
            t("Computador"),
            vec![switch_row(
                s,
                t("Permitir que agentes usem o computador"),
                t(
                    "Adiciona as ferramentas de computador aos próximos turnos. Cada app ainda precisa da sua aprovação uma vez por sessão.",
                ),
                top("computerUseEnabled"),
            )],
        ),
        group(
            t("Permissões do macOS"),
            vec![
                permission(
                    t("Acessibilidade"),
                    t("Ler os controles dos apps e enviar cliques e teclas."),
                    accessibility,
                    "requestAccessibility",
                    "openAccessibilitySettings",
                ),
                permission(
                    t("Gravação de Tela"),
                    t("Capturas da janela do app aprovado."),
                    screen,
                    "requestScreenRecording",
                    "openScreenRecordingSettings",
                ),
            ],
        ),
    ];
    if !(accessibility && screen) {
        out.push(note(
            t("Depois de permitir nos Ajustes do Sistema, feche e reabra o Sirus se o status não atualizar."),
        ));
    }
    out.push(group(
        t("Segurança"),
        vec![
            row(
                t("Aprovação por app"),
                t("O agente só vê e usa os apps que você liberar, naquela sessão. Nada fica guardado depois que você fecha o app."),
                Vec::new(),
            ),
            row(
                t("Pare quando quiser"),
                t("Aperte Esc ou Parar no indicador flutuante para revogar todos os apps de uma vez."),
                Vec::new(),
            ),
            row(
                t("Planejamento é só leitura"),
                t("No modo planejamento o agente pode olhar, mas não clicar nem digitar."),
                Vec::new(),
            ),
        ],
    ));

    let grants = c["grants"].as_array().cloned().unwrap_or_default();
    let mut granted: Vec<Row> = grants
        .iter()
        .map(|grant| {
            row(
                grant["app"].as_str().unwrap_or_default(),
                "",
                vec![Ctl::Button {
                    label: t("Revogar"),
                    act: computer_act(json!({
                        "type": "revoke",
                        "sessionId": grant["sessionId"],
                        "bundleId": grant["bundleId"],
                    })),
                    enabled: true,
                    danger: false,
                }],
            )
        })
        .collect();
    if granted.is_empty() {
        granted.push(row("", t("Nenhum app liberado no momento."), Vec::new()));
    } else {
        granted.push(row(
            "",
            "",
            vec![Ctl::Button {
                label: t("Parar e revogar tudo"),
                act: computer_act(json!({"type": "stop"})),
                enabled: true,
                danger: true,
            }],
        ));
    }
    out.push(group(t("Liberados agora"), granted));

    let mut blocked = vec![row(
        "",
        t(
            "Gerenciadores de senha, segurança do sistema, terminais e ferramentas de automação nunca podem ser controlados.",
        ),
        Vec::new(),
    )];
    blocked.extend(
        strings(&c["blocked"])
            .iter()
            .map(|name| row(name.as_str(), "", Vec::new())),
    );
    out.push(group(t("Sempre bloqueados"), blocked));

    let history: Vec<&Value> = c["history"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["app"].as_str().is_some_and(|app| !app.is_empty()))
        .take(30)
        .collect();
    let history_rows: Vec<Row> = if history.is_empty() {
        vec![row("", t("Nenhuma ação ainda."), Vec::new())]
    } else {
        history
            .into_iter()
            .map(|entry| {
                let action = entry["action"].as_str().unwrap_or_default();
                row(
                    format!("{} · {action}", entry["app"].as_str().unwrap_or_default()),
                    local_when(entry["at"].as_str().unwrap_or_default(), "%H:%M:%S"),
                    vec![Ctl::Read(
                        match entry["outcome"].as_str().unwrap_or_default() {
                            "done" => t("feito"),
                            "failed" => t("falhou"),
                            "allowed" => t("liberado"),
                            "declined" => t("recusado"),
                            "blocked" => t("bloqueado"),
                            other => other,
                        }
                        .to_string(),
                    )],
                )
            })
            .collect()
    };
    out.push(group(t("Ações recentes"), history_rows));
    out
}

/// A permission row: its state, or the two buttons that ask for it or open its System Settings pane.
fn permission(
    title: &'static str,
    about: &'static str,
    granted: bool,
    request: &'static str,
    settings_pane: &'static str,
) -> Row {
    let controls = if granted {
        vec![Ctl::Read(t("Permitido").to_string())]
    } else {
        vec![
            Ctl::Button {
                label: t("Permitir"),
                act: computer_act(json!({"type": request})),
                enabled: true,
                danger: false,
            },
            Ctl::Button {
                label: t("Abrir Ajustes do Sistema"),
                act: computer_act(json!({"type": settings_pane})),
                enabled: true,
                danger: false,
            },
        ]
    };
    row(title, about, controls)
}

/// Computadores: this Mac and the Sirus hosts paired with this window (computers.rs). The whole
/// window shows the one in use.
fn computers_groups(d: &HashMap<&'static str, Value>) -> Vec<Group> {
    let c = get_data(d, "computers");
    let active = c["active"].as_str().unwrap_or("home");
    let use_button = |id: &str| Ctl::Button {
        label: t("Usar"),
        act: act_run("native_computer_use", json!({"id": id})),
        enabled: true,
        danger: false,
    };
    let mut rows = vec![row(
        t("Este Mac"),
        t("O motor desta janela, com seus projetos e arquivos."),
        vec![if active == "home" {
            Ctl::Read(t("Em uso").to_string())
        } else {
            use_button("home")
        }],
    )];
    for computer in c["list"].as_array().into_iter().flatten() {
        let id = computer["id"].as_str().unwrap_or("");
        let status = if id == active {
            c["status"].as_str().unwrap_or("")
        } else {
            ""
        };
        let about = [computer["address"].as_str().unwrap_or(""), status]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" · ");
        rows.push(row(
            computer["name"].as_str().unwrap_or(id).to_string(),
            about,
            vec![
                if id == active {
                    Ctl::Read(t("Em uso").to_string())
                } else {
                    use_button(id)
                },
                Ctl::Button {
                    label: t("Remover"),
                    act: act_run("native_computer_remove", json!({"id": id})),
                    enabled: true,
                    danger: true,
                },
            ],
        ));
    }
    let busy = yes(&c["busy"]);
    let field = |key: &'static str, width: f32| Ctl::Field {
        key,
        text: c[if key == crate::shell::computer_ui::ADDRESS {
            "address"
        } else {
            "code"
        }]
        .as_str()
        .unwrap_or("")
        .to_string(),
        width,
        button: None,
    };
    rows.push(row(
        t("Adicionar um computador"),
        t("O endereço HTTPS dele na Tailscale, como https://nome.rede.ts.net"),
        vec![field(crate::shell::computer_ui::ADDRESS, 280.)],
    ));
    rows.push(row(
        t("Código de pareamento"),
        t("Os seis dígitos de `sirus-host pair`, ou do QR code da página Conexões do outro Mac."),
        vec![
            field(crate::shell::computer_ui::CODE, 96.),
            Ctl::Button {
                label: t("Parear"),
                act: act_run("native_computer_pair", json!({})),
                enabled: !busy,
                danger: false,
            },
        ],
    ));
    if let Some(note) = c["note"].as_str().filter(|n| !n.is_empty()) {
        rows.push(row("", note.to_string(), Vec::new()));
    }
    vec![group(t("Computadores"), rows)]
}

fn connections(d: &HashMap<&'static str, Value>) -> Vec<Group> {
    let r = get_data(d, "remote");
    let enabled = yes(&r["enabled"]);
    let address = r["urls"]
        .as_array()
        .and_then(|urls| urls.first())
        .and_then(Value::as_str);
    let https = &r["https"];
    let https_url = https["url"].as_str();
    let https_setup = https["setupUrl"].as_str();

    let mut access = vec![row(
        t("Permitir outros aparelhos"),
        t(
            "Só aparelhos na sua rede Tailscale alcançam este Mac, e cada um precisa de um QR code desta página.",
        ),
        vec![Ctl::Switch {
            on: enabled,
            change: Some(remote_act(
                json!({"type": "setEnabled", "enabled": !enabled}),
            )),
        }],
    )];
    if enabled {
        let keep_awake = yes(&r["keepAwake"]);
        access.push(row(
            t("Manter o Mac acordado para o celular"),
            t("Na tomada, o Mac não entra em repouso e seus aparelhos sempre o alcançam. A tela continua apagando e bloqueando; na bateria, o repouso funciona normalmente. Fechar a tampa ainda põe o Mac em repouso."),
            vec![Ctl::Switch {
                on: keep_awake,
                change: Some(remote_act(
                    json!({"type": "setKeepAwake", "enabled": !keep_awake}),
                )),
            }],
        ));
        match address {
            Some(url) => access.push(row(
                t("Endereço"),
                t("Onde seus aparelhos encontram este Mac na Tailscale."),
                vec![
                    Ctl::Read(url.to_string()),
                    Ctl::Button {
                        label: t("Copiar endereço"),
                        act: act_run("native_copy", json!({"text": url})),
                        enabled: true,
                        danger: false,
                    },
                ],
            )),
            None => access.push(row(
                t("A Tailscale não está conectada"),
                t("Instale a Tailscale neste Mac e no celular e entre com a mesma conta. É grátis para uso pessoal."),
                vec![Ctl::Button {
                    label: t("Verificar de novo"),
                    act: remote_act(json!({"type": "status"})),
                    enabled: true,
                    danger: false,
                }],
            )),
        }
        if yes(&https["available"]) {
            access.push(if https_url.is_some() {
                row(
                    t("Endereço seguro (HTTPS)"),
                    t("Ligado. Os novos QR codes abrem o endereço seguro; aparelhos pareados antes precisam de um novo código por lá."),
                    vec![
                        Ctl::Read(https_url.unwrap_or_default().to_string()),
                        Ctl::Button {
                            label: t("Desativar"),
                            act: remote_act(json!({"type": "disableHttps"})),
                            enabled: true,
                            danger: true,
                        },
                    ],
                )
            } else {
                row(
                    t("Endereço seguro (HTTPS)"),
                    t("Necessário para instalar o Sirus como app no celular e receber alertas. A Tailscale fornece o certificado."),
                    vec![Ctl::Button {
                        label: t("Ativar HTTPS"),
                        act: remote_act(json!({"type": "enableHttps"})),
                        enabled: true,
                        danger: false,
                    }],
                )
            });
        }
        if https_setup.is_some() && https_url.is_none() {
            access.push(row(
                t("Permita HTTPS na sua tailnet"),
                t("Uma permissão única na sua conta da Tailscale. Abra a página, confirme e volte para verificar de novo."),
                vec![
                    Ctl::Button {
                        label: t("Abrir página"),
                        act: remote_act(json!({"type": "openHttpsSetup"})),
                        enabled: true,
                        danger: false,
                    },
                    Ctl::Button {
                        label: t("Verificar de novo"),
                        act: remote_act(json!({"type": "enableHttps"})),
                        enabled: true,
                        danger: false,
                    },
                ],
            ));
        }
    }
    let mut out = vec![group(t("Acesso remoto"), access)];
    let error = get_data(d, "remote_error")
        .as_str()
        .or_else(|| r["error"].as_str())
        .or_else(|| https["error"].as_str().filter(|_| https_setup.is_none()));
    if let Some(text) = error {
        out.push(note(text));
    }

    if enabled && address.is_some() {
        out.push(group(
            t("Conectar um aparelho"),
            vec![row(
                t("Celular ou outro computador"),
                t(
                    "Mostra um código de uso único para conectar o app do Sirus no iPhone ou um navegador da sua Tailscale.",
                ),
                vec![Ctl::Button {
                    label: t("Conectar um aparelho"),
                    act: act_run("native_pair_open", json!({})),
                    enabled: true,
                    danger: false,
                }],
            )],
        ));
    }

    let devices = r["devices"].as_array().cloned().unwrap_or_default();
    let mut device_rows: Vec<Row> = Vec::new();
    for device in &devices {
        let id = device["id"].as_str().unwrap_or_default();
        let created = device["createdAt"].as_str().unwrap_or_default();
        let mut about = match device["lastSeen"].as_str() {
            Some(seen) => tf(
                "Adicionado em {} · último uso {}",
                &[
                    &local_when(created, t("%d/%m/%Y")),
                    &local_when(seen, t("%d/%m/%Y %H:%M")),
                ],
            ),
            None => tf("Adicionado em {}", &[&local_when(created, t("%d/%m/%Y"))]),
        };
        let push = yes(&device["push"]);
        if push {
            about.push_str(t(" · alertas ligados"));
        }
        let mut controls = vec![Ctl::Field {
            key: key_of(format!("device-rename:{id}")),
            text: device["name"].as_str().unwrap_or_default().to_string(),
            width: 160.,
            button: Some(t("Renomear")),
        }];
        if push {
            controls.push(Ctl::Button {
                label: t("Enviar alerta de teste"),
                act: remote_act(json!({"type": "testPush", "deviceId": id})),
                enabled: true,
                danger: false,
            });
        }
        controls.push(Ctl::Button {
            label: t("Remover"),
            act: remote_act(json!({"type": "revoke", "deviceId": id})),
            enabled: true,
            danger: true,
        });
        device_rows.push(row(
            device["name"].as_str().unwrap_or_default(),
            about,
            controls,
        ));
    }
    if device_rows.is_empty() {
        device_rows.push(row("", t("Nenhum aparelho conectado ainda."), Vec::new()));
    }
    out.push(group(t("Aparelhos"), device_rows));
    out.push(group(
        t("O que um aparelho pode fazer"),
        vec![row(
            "",
            t("Um aparelho conectado pode fazer o que esta janela faz: abrir sessões, enviar mensagens, aprovar ações, fazer commit e push. Seletores de arquivo, ditado e controle do computador ficam só no Mac. Remova o aparelho que não usar mais."),
            Vec::new(),
        )],
    ));
    out
}

// Text edits and the typography, effects, background, sidebar and Dock groups.

/// A key made at run time (a provider, a connector's row, a device), interned so it is `'static`
/// as `edit` takes it. The set is small: one entry per provider, connector row and device.
fn key_of(text: String) -> &'static str {
    static KEYS: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut keys = KEYS.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(known) = keys.get(text.as_str()) {
        return *known;
    }
    let leaked: &'static str = Box::leak(text.into_boxed_str());
    keys.insert(leaked);
    leaked
}

/// Whether a provider path field's text can name an executable: empty, a path, or a bare program
/// name. An account's email or a sentence cannot, and saved it would make the CLI read as missing.
pub fn provider_path_like(text: &str) -> bool {
    text.contains('/')
        || text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// Drops saved provider paths that cannot name an executable. True when something changed.
pub fn repair_provider_paths(settings: &mut Value) -> bool {
    let Some(paths) = settings["providerPaths"].as_object_mut() else {
        return false;
    };
    let before = paths.len();
    paths.retain(|_, path| path.as_str().is_some_and(provider_path_like));
    paths.len() != before
}

/// The change a finished text edit makes, from the key `edit` started and the text typed. None
/// when the text is not a value that key takes (the main app keeps the field open then).
pub fn commit(settings: &Value, key: &str, text: &str) -> Option<Commit> {
    let text = text.trim();
    if let Some(id) = key.strip_prefix("providerPath:") {
        if !provider_path_like(text) {
            return None;
        }
        // An empty path returns the provider to automatic detection.
        let mut paths = settings["providerPaths"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        if text.is_empty() {
            paths.remove(id);
        } else {
            paths.insert(id.to_string(), json!(text));
        }
        return Some(Commit::Save {
            key: "providerPaths",
            value: Value::Object(paths),
        });
    }
    // The Connectors page keeps its own search, dialog fields and rename in its module.
    if crate::settings_connectors::commit(key, text) {
        return None;
    }
    if crate::settings_pair::commit(key, text) {
        return None;
    }
    if crate::shell::computer_ui::is_field(key) {
        return Some(Commit::Run {
            cmd: "native_computer_field",
            args: json!({"key": key, "text": text}),
        });
    }
    if let Some(device) = key.strip_prefix("device-rename:") {
        if text.is_empty() {
            return None;
        }
        return Some(Commit::Run {
            cmd: "remote_action",
            args: json!({"action": {"type": "rename", "deviceId": device, "name": text}}),
        });
    }
    match key {
        "worktreeBasePath" => Some(Commit::Save {
            key: "worktreeBasePath",
            value: if text.is_empty() {
                Value::Null
            } else {
                json!(text)
            },
        }),
        "worktreeBranchPattern" => (!text.is_empty()).then(|| Commit::Save {
            key: "worktreeBranchPattern",
            value: json!(text),
        }),
        _ => {
            // Numeric fields, with the ranges of the React controls.
            const NUMBERS: &[(&str, i64, i64)] = &[
                ("chatBackgroundEmptyVisibility", 0, 100),
                ("chatBackgroundSessionVisibility", 0, 100),
            ];
            let &(name, low, high) = NUMBERS.iter().find(|(name, _, _)| *name == key)?;
            let number: i64 = text.parse().ok()?;
            (low..=high).contains(&number).then(|| Commit::Save {
                key: name,
                value: json!(number),
            })
        }
    }
}

/// Settings → Aparência: the groups the native appearance page shares with the others, from
/// `AppearanceSettings.tsx` (typography, effects and motion, chat background, sidebar, Dock).
/// `data` may hold `host` (its `appearanceSupport` says what this Mac can do).
pub fn appearance_groups<V: 'static>(
    settings: &Value,
    data: &HashMap<&'static str, Value>,
    editing: Option<(&str, &str)>,
    save: Save<V>,
    edit: Edit<V>,
    run: Run<V>,
    cx: &mut Context<V>,
) -> Vec<AnyElement> {
    let mut ui = Ui {
        ids: 0,
        custom: settings["customShortcuts"].clone(),
        editing,
        save,
        edit,
        run,
    };
    let support = &get_data(data, "host")["appearanceSupport"];
    let s = settings;

    let effects = group(
        t("Efeitos e movimento"),
        vec![
            switch_row(s, t("Animações da interface"), "", top("animations")),
            switch_row(
                s,
                t("Efeito de luz nos botões"),
                t("Os botões reagem ao ponteiro do mouse."),
                top("pointerGlow"),
            ),
            row(
                t("Velocidade do corredor do composer"),
                t(
                    "O modelo de trabalho que percorre a borda do composer enquanto o agente trabalha.",
                ),
                vec![pick(
                    s,
                    top("composerLineSpeed"),
                    &[
                        ("slow", t("Lenta")),
                        ("smooth", t("Suave")),
                        ("fast", t("Rápida")),
                    ],
                )],
            ),
            switch_row(
                s,
                t("Reduzir movimento"),
                t("Também respeita prefers-reduced-motion do sistema."),
                top("reduceMotion"),
            ),
        ],
    );

    let background_rows = match s["chatBackground"].as_str() {
        None => vec![row(
            t("Escolher uma imagem"),
            t("Uma imagem atrás dos painéis de conversa. Ela fica neste Mac."),
            vec![Ctl::Button {
                label: t("Escolher imagem"),
                act: act_run("native_pick_background", json!({})),
                enabled: true,
                danger: false,
            }],
        )],
        Some(name) => vec![
            row(
                t("Imagem atual"),
                name,
                vec![
                    Ctl::Button {
                        label: t("Trocar"),
                        act: act_run("native_pick_background", json!({})),
                        enabled: true,
                        danger: false,
                    },
                    Ctl::Button {
                        label: t("Remover"),
                        act: Act::Save {
                            key: "chatBackground",
                            value: Value::Null,
                        },
                        enabled: true,
                        danger: true,
                    },
                ],
            ),
            row(
                t("Efeito do fundo"),
                t("Como a imagem é desenhada atrás das conversas."),
                vec![pick(
                    s,
                    top("chatBackgroundEffect"),
                    &[
                        ("none", t("Nenhum")),
                        ("dither", "Dither"),
                        ("ascii", "ASCII"),
                        ("halftone", "Halftone"),
                        ("scanlines", "Scanlines"),
                        ("haze", "Haze"),
                    ],
                )],
            ),
            row(
                t("Mostrar em"),
                t("Só nas sessões vazias, ou em todas as conversas."),
                vec![pick(
                    s,
                    top("chatBackgroundShowOn"),
                    &[("empty", t("Só vazias")), ("all", t("Todas as sessões"))],
                )],
            ),
            row(
                t("Visibilidade no chat vazio"),
                t("Força do fundo antes de a conversa ter mensagens."),
                vec![
                    Ctl::Field {
                        key: "chatBackgroundEmptyVisibility",
                        text: current(s, top("chatBackgroundEmptyVisibility")),
                        width: 72.,
                        button: None,
                    },
                    Ctl::Read("%".to_string()),
                ],
            ),
            row(
                t("Visibilidade na sessão"),
                t("Força do fundo quando a conversa já tem mensagens. De 0 a 100%."),
                vec![
                    Ctl::Field {
                        key: "chatBackgroundSessionVisibility",
                        text: current(s, top("chatBackgroundSessionVisibility")),
                        width: 72.,
                        button: None,
                    },
                    Ctl::Read("%".to_string()),
                ],
            ),
        ],
    };
    let background = group(t("Fundo do chat"), background_rows);

    let mut sidebar_rows = vec![row(
        t("Estilo da barra lateral"),
        t("Barra de ícones, com as conversas nas abas, ou uma lista com todas as conversas."),
        vec![pick(
            s,
            top("sidebarMode"),
            &[
                ("rail", t("Barra de ícones")),
                ("list", t("Lista de conversas")),
            ],
        )],
    )];
    if current(s, top("sidebarMode")) == "list" {
        sidebar_rows.push(row(
            t("Ordem da lista"),
            "",
            vec![pick(
                s,
                top("sidebarListOrder"),
                &[("recent", t("Recentes")), ("project", t("Por projeto"))],
            )],
        ));
    }
    let sidebar = group(t("Barra lateral"), sidebar_rows);

    let dock_supported = yes(&support["dockIcon"]);
    let application = group(
        t("Aplicativo"),
        vec![row(
            t("Ícone do Dock"),
            if dock_supported {
                t("Escolha o ícone exibido no Dock.")
            } else {
                t("A personalização do Dock não está disponível neste host.")
            },
            vec![if dock_supported {
                pick(
                    s,
                    top("dockIcon"),
                    &[
                        ("default", t("Padrão")),
                        ("smokedGlass", t("Vidro fumê")),
                        ("white", t("Branco")),
                    ],
                )
            } else {
                Ctl::Read(t("Indisponível").to_string())
            }],
        )],
    );

    [effects, background, sidebar, application]
        .into_iter()
        .map(|item| group_view(item, &mut ui, cx))
        .collect()
}

#[cfg(test)]
mod provider_path_tests {
    use super::*;

    #[test]
    fn an_email_is_not_a_provider_path() {
        for good in [
            "",
            "claude",
            "cursor-agent",
            "/Users/me/.local/bin/claude",
            "./bin/x",
        ] {
            assert!(provider_path_like(good), "{good}");
        }
        for bad in ["me@gmail.com", "my claude"] {
            assert!(!provider_path_like(bad), "{bad}");
        }
    }

    #[test]
    fn a_finished_edit_refuses_an_email_and_keeps_the_path() {
        let settings = json!({ "providerPaths": { "codex": "/bin/codex" } });
        assert!(commit(&settings, "providerPath:claude", "me@gmail.com").is_none());
        assert!(commit(&settings, "providerPath:claude", "/bin/claude").is_some());
    }

    #[test]
    fn repair_removes_only_what_cannot_be_a_path() {
        let mut settings =
            json!({ "providerPaths": { "claude": "me@gmail.com", "codex": "/bin/codex" } });
        assert!(repair_provider_paths(&mut settings));
        assert_eq!(settings["providerPaths"], json!({ "codex": "/bin/codex" }));
        assert!(!repair_provider_paths(&mut settings), "already clean");
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    fn keys(text: &str) -> gpui::Keystroke {
        gpui::Keystroke::parse(text).unwrap()
    }

    #[test]
    fn a_free_combo_is_saved_and_the_default_clears_it() {
        let settings = json!({});
        let map = recorded_shortcut(&settings, "palette", &keys("cmd-g")).unwrap();
        assert_eq!(map["palette"], "meta+g");
        let settings = json!({ "customShortcuts": map });
        let map = recorded_shortcut(&settings, "palette", &keys("cmd-k")).unwrap();
        assert!(map.get("palette").is_none());
    }

    #[test]
    fn a_taken_or_reserved_combo_is_refused() {
        let settings = json!({});
        assert!(recorded_shortcut(&settings, "palette", &keys("cmd-n")).is_err());
        assert!(
            recorded_shortcut(&settings, "palette", &keys("cmd-j")).is_err(),
            "opens the terminal"
        );
        assert!(recorded_shortcut(&settings, "palette", &keys("cmd-q")).is_err());
        assert!(recorded_shortcut(&settings, "palette", &keys("j")).is_err());
    }

    #[test]
    fn a_moved_shortcut_answers_on_its_new_keys_only() {
        let settings = json!({ "customShortcuts": { "palette": "meta+j" } });
        match shortcut_route(&settings, &keys("cmd-j")) {
            ShortcutRoute::As(keystroke) => {
                assert_eq!(keystroke.key, "k");
                assert!(keystroke.modifiers.platform);
            }
            _ => panic!("should route to the palette"),
        }
        assert!(matches!(
            shortcut_route(&settings, &keys("cmd-k")),
            ShortcutRoute::Unbound
        ));
        assert!(matches!(
            shortcut_route(&settings, &keys("cmd-p")),
            ShortcutRoute::Pass
        ));
    }

    #[test]
    fn caps_are_one_per_key() {
        assert_eq!(shortcut_caps("meta+alt+enter").len(), 3);
    }
}
