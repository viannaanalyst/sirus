//! Settings → Conectores → Infisical (ADR-198): the machine identity that reaches the owner's
//! project keys, and for each Sirus project the Infisical project and environment it uses. Once
//! connected, Infisical is a card in the "Conectados" grid with the same "⋯" menu as the other
//! connectors; "Projetos e chaves…" opens one dialog with a line per Sirus project (Infisical
//! project, environment, whether its terminals get the keys). Not connected, it is a catalog card
//! whose "+" opens the connect dialog. The host does the work (`infisical_action`, src-tauri
//! `infisical.rs`, which also links projects by `.infisical.json` or by name); the window only keeps
//! what it shows. Client secrets are masked while typed and cleared once sent. The window never
//! sees a key's value, only names.

use crate::i18n::{t, tf};
use crate::motion::{Curve, eased};
use crate::settings_connectors::{
    DIALOG_TIME, MenuEntry, Pending, after, danger, icon_button, input, item, logo, row_menu,
    signed_in_badge,
};
use crate::settings_pages::{Edit, Run, switch_track};
use crate::theme::{accent, accent_wash, content, floating_fill, on_accent};
use gpui::{
    Animation, AnimationExt, AnyElement, Context, FontWeight, Hsla, div, prelude::*, px, svg,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::Duration;

pub(crate) const CLIENT_ID: &str = "if-client-id";
pub(crate) const CLIENT_SECRET: &str = "if-client-secret";
/// The environments a project can be linked to.
pub(crate) const ENVIRONMENTS: [&str; 3] = ["dev", "staging", "prod"];
/// The catalog section the card sits in while not connected.
pub(crate) const CATEGORY: &str = "developer";
/// What the settings search jumps to: the card, connected or not.
pub(crate) const KEY: &str = "Infisical";
/// The catalog card's description (also indexed by the settings search).
pub(crate) const ABOUT: &str = "Chaves dos projetos nos terminais, scripts e agentes";
const SITE: &str = "https://app.infisical.com";
const MENU_ID: &str = "infisical";

/// A Sirus project as this page needs it.
#[derive(Clone)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub path: String,
    /// The environment the owner chose for its scripts and terminals, when there is one.
    pub scripts: Option<String>,
}

/// What a Sirus project uses, as the host reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LinkRow {
    pub workspace: String,
    pub environment: String,
    /// "folder", "repo" or "name": where the link comes from.
    pub source: String,
    /// Whether its terminals and scripts get the keys (on by default, ADR-198).
    pub terminal: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DialogKind {
    Connect,
    Projects,
}

#[derive(Clone, Copy)]
struct Dialog {
    kind: DialogKind,
    closing: bool,
    generation: u32,
}

struct State {
    projects: Vec<ProjectRow>,
    /// Sirus project path to its link; `None` once the host said it has none.
    links: BTreeMap<String, Option<LinkRow>>,
    /// Paths whose link was asked for, so nothing asks twice.
    asked: BTreeSet<String>,
    status_known: bool,
    connected: bool,
    logged_in: bool,
    identity: String,
    /// Infisical projects: (id, name).
    infisical: Vec<(String, String)>,
    status_error: Option<String>,
    client_id: String,
    client_secret: String,
    busy: bool,
    error: Option<String>,
    /// The Sirus project (path) whose Infisical project list is open in the dialog.
    picker: Option<String>,
    dialog: Option<Dialog>,
    generation: u32,
}

static STATE: Mutex<State> = Mutex::new(State {
    projects: Vec::new(),
    links: BTreeMap::new(),
    asked: BTreeSet::new(),
    status_known: false,
    connected: false,
    logged_in: false,
    identity: String::new(),
    infisical: Vec::new(),
    status_error: None,
    client_id: String::new(),
    client_secret: String::new(),
    busy: false,
    error: None,
    picker: None,
    dialog: None,
    generation: 0,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub fn set_projects(rows: Vec<ProjectRow>) {
    state().projects = rows;
}

/// `(Infisical project id, environment)` the Sirus project at `path` is linked to.
pub(crate) fn link(path: &str) -> Option<(String, String)> {
    state()
        .links
        .get(path)
        .cloned()
        .flatten()
        .map(|link| (link.workspace, link.environment))
}

/// True the first time it is asked about this path (or `force`): the caller then asks the host.
pub(crate) fn should_ask(path: &str, force: bool) -> bool {
    let first = state().asked.insert(path.to_owned());
    first || force
}

/// The name of an Infisical project, once the organization's list is known.
pub(crate) fn infisical_name(workspace_id: &str) -> Option<String> {
    state()
        .infisical
        .iter()
        .find(|(id, _)| id == workspace_id)
        .map(|(_, name)| name.clone())
}

pub(crate) fn status_known() -> bool {
    state().status_known
}

pub fn status_args() -> Value {
    json!({"action": {"type": "status"}})
}

pub fn link_args(path: &str) -> Value {
    json!({"action": {"type": "projectLink", "projectPath": path}})
}

fn plain_error(value: &Value) -> String {
    crate::i18n::from_host(
        value["message"]
            .as_str()
            .or(value.as_str())
            .unwrap_or("Infisical refused the request."),
    )
}

/// A `link` value of the host (alone, or one of the status's `links`), kept by its path.
fn apply_link(now: &mut State, value: &Value) {
    let Some(path) = value["projectPath"].as_str() else {
        return;
    };
    let row = (value["linked"].as_bool() == Some(true)).then(|| LinkRow {
        workspace: value["workspaceId"].as_str().unwrap_or("").to_owned(),
        environment: value["environment"].as_str().unwrap_or("dev").to_owned(),
        source: value["source"].as_str().unwrap_or("folder").to_owned(),
        terminal: value["terminal"].as_bool().unwrap_or(true),
    });
    now.links.insert(path.to_owned(), row);
}

/// A host reply for the page. True when the status should be read again.
pub fn reply(ok: bool, value: &Value) -> bool {
    let mut now = state();
    if !ok {
        now.busy = false;
        now.error = Some(plain_error(value));
        return false;
    }
    match value["kind"].as_str() {
        Some("status") => {
            now.status_known = true;
            now.connected = value["connected"].as_bool() == Some(true);
            now.logged_in = value["loggedIn"].as_bool() == Some(true);
            now.identity = value["identity"].as_str().unwrap_or("sirus").to_owned();
            now.infisical = value["projects"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    Some((p["id"].as_str()?.to_owned(), p["name"].as_str()?.to_owned()))
                })
                .collect();
            now.status_error = value["error"].as_str().map(crate::i18n::from_host);
            for link in value["links"].as_array().into_iter().flatten() {
                apply_link(&mut now, link);
            }
            false
        }
        Some("connected") => {
            now.busy = false;
            now.error = None;
            now.client_id.clear();
            now.client_secret.clear();
            if now.dialog.is_some_and(|d| d.kind == DialogKind::Connect) {
                now.dialog = None;
            }
            true
        }
        Some("disconnected") => {
            now.connected = false;
            now.logged_in = false;
            now.infisical.clear();
            now.dialog = None;
            true
        }
        Some("link") => {
            apply_link(&mut now, value);
            false
        }
        _ => false,
    }
}

/// A finished text edit of the connect dialog. True when the key was one of its own.
pub fn commit(key: &str, text: &str) -> bool {
    let mut now = state();
    match key {
        CLIENT_ID => now.client_id = text.trim().chars().take(256).collect(),
        CLIENT_SECRET => now.client_secret = text.trim().chars().take(1024).collect(),
        _ => return false,
    }
    true
}

// ---- What the card says ----

/// How many of the projects use an Infisical project, and how many the host said have none
/// (projects whose link is not known yet count in neither).
pub(crate) fn link_counts(
    projects: &[ProjectRow],
    links: &BTreeMap<String, Option<LinkRow>>,
) -> (usize, usize) {
    projects
        .iter()
        .fold((0, 0), |(linked, unlinked), p| match links.get(&p.path) {
            Some(Some(_)) => (linked + 1, unlinked),
            Some(None) => (linked, unlinked + 1),
            None => (linked, unlinked),
        })
}

/// The card's line under the name.
fn card_description(
    logged_in: bool,
    error: Option<String>,
    linked: usize,
    identity: &str,
) -> String {
    if !logged_in {
        return error
            .unwrap_or_else(|| t("As credenciais estão salvas, mas o login falhou.").to_owned());
    }
    match linked {
        1 => tf("Chaves de 1 projeto · como {}", &[&identity]),
        n => tf("Chaves de {} projetos · como {}", &[&n, &identity]),
    }
}

fn unlinked_line(n: usize) -> String {
    match n {
        1 => t("1 pasta sem projeto").to_owned(),
        n => tf("{} pastas sem projeto", &[&n]),
    }
}

fn open_dialog(kind: DialogKind) {
    let mut now = state();
    now.generation += 1;
    now.picker = None;
    if kind == DialogKind::Connect {
        now.error = None;
    }
    now.dialog = Some(Dialog {
        kind,
        closing: false,
        generation: now.generation,
    });
}

/// True while one of the page's Infisical dialogs is up (it takes Escape).
pub fn wants_keys() -> bool {
    state().dialog.is_some_and(|d| !d.closing)
}

pub fn close_dialog<V: 'static>(cx: &mut Context<V>) {
    {
        let mut now = state();
        match now.dialog.as_mut() {
            Some(dialog) if !dialog.closing => dialog.closing = true,
            _ => return,
        }
        now.picker = None;
        if now.dialog.is_some_and(|d| d.kind == DialogKind::Connect) {
            now.client_id.clear();
            now.client_secret.clear();
        }
    }
    cx.notify();
    after(cx, (DIALOG_TIME * 1000.) as u64 + 10, |_, cx| {
        let mut now = state();
        if now.dialog.is_some_and(|d| d.closing) {
            now.dialog = None;
        }
        cx.notify();
    });
}

fn name_line(name: &str, badge: bool) -> Vec<AnyElement> {
    let mut line = vec![
        div()
            .flex_shrink_0()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(content(0.95))
            .child(name.to_owned())
            .into_any_element(),
    ];
    if badge {
        line.push(signed_in_badge());
    }
    line
}

/// The card in the "Conectados" grid, once credentials are saved; with a muted line under it when
/// some project folders have no Infisical project.
pub fn connected_card<V: 'static>(
    matches: &dyn Fn(&[&str]) -> bool,
    cx: &mut Context<V>,
    run: Run<V>,
) -> Option<AnyElement> {
    let (known, connected, logged_in, error, identity, projects, links) = {
        let now = state();
        (
            now.status_known,
            now.connected,
            now.logged_in,
            now.status_error.clone(),
            now.identity.clone(),
            now.projects.clone(),
            now.links.clone(),
        )
    };
    if !known || !connected {
        return None;
    }
    let (linked, unlinked) = link_counts(&projects, &links);
    let description = card_description(logged_in, error, linked, &identity);
    if !matches(&["Infisical", &description, t(ABOUT)]) {
        return None;
    }
    let entries: Vec<MenuEntry<V>> = vec![
        (
            "connector-ui/key-round",
            t("Projetos e chaves…").to_owned(),
            false,
            Box::new(|_, _| open_dialog(DialogKind::Projects)),
        ),
        (
            "connector-ui/external-link",
            t("Abrir no Infisical").to_owned(),
            false,
            Box::new(move |this, cx| run(this, "native_open_path", json!({"path": SITE}), cx)),
        ),
        (
            "connector-ui/log-out",
            t("Desconectar").to_owned(),
            true,
            Box::new(move |this, cx| {
                run(
                    this,
                    "infisical_action",
                    json!({"action": {"type": "disconnect"}}),
                    cx,
                )
            }),
        ),
    ];
    let card = item(
        "if-card".to_owned(),
        logo(Some("infisical"), "Infisical", 28.),
        name_line("Infisical", logged_in),
        description,
        false,
        vec![row_menu(MENU_ID, entries, cx)],
        KEY,
    );
    if !logged_in || unlinked == 0 {
        return Some(card);
    }
    let line = div()
        .ml(px(47.))
        .mt(px(-4.))
        .mb(px(4.))
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(px(11.5))
        .text_color(content(0.45))
        .child(unlinked_line(unlinked))
        .child("·")
        .child(
            div()
                .id("if-choose")
                .text_color(content(0.65))
                .cursor_pointer()
                .hover(|s| s.text_color(content(0.95)))
                .on_click(cx.listener(|_, _, _, cx| {
                    open_dialog(DialogKind::Projects);
                    cx.notify();
                }))
                .child(t("Escolher")),
        );
    Some(
        div()
            .flex()
            .flex_col()
            .child(card)
            .child(line)
            .into_any_element(),
    )
}

/// The catalog card while Infisical is not connected; "+" opens the connect dialog.
pub fn catalog_card<V: 'static>(
    matches: &dyn Fn(&[&str]) -> bool,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    if state().connected || !matches(&["Infisical", t(ABOUT)]) {
        return None;
    }
    let button = icon_button("cn-add-infisical".to_owned(), "plus", cx, |_, cx| {
        open_dialog(DialogKind::Connect);
        cx.notify();
    });
    Some(item(
        "cat-infisical".to_owned(),
        logo(Some("infisical"), "Infisical", 28.),
        name_line("Infisical", false),
        t(ABOUT).to_owned(),
        false,
        vec![button],
        KEY,
    ))
}

// ---- The dialogs ----

fn button<V: 'static>(
    id: &'static str,
    label: &str,
    main: bool,
    enabled: bool,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let base = div()
        .id(id)
        .h(px(32.))
        .px(px(if main { 14. } else { 12. }))
        .rounded(px(crate::theme::RADIUS_MD))
        .flex()
        .items_center()
        .text_size(px(13.))
        .when(main, |d| {
            d.bg(accent())
                .font_weight(FontWeight::MEDIUM)
                .text_color(on_accent())
        })
        .when(!main, |d| d.text_color(content(0.75)))
        .child(label.to_owned());
    if !enabled {
        return base.opacity(0.45).into_any_element();
    }
    base.cursor_pointer()
        .when(main, |d| d.hover(|s| s.opacity(0.9)))
        .when(!main, |d| d.hover(|s| s.bg(content(0.07))))
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .into_any_element()
}

fn env_pill<V: 'static>(
    id: (&'static str, usize),
    env: &'static str,
    on: bool,
    enabled: bool,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let base = div()
        .id(id)
        .h(px(22.))
        .px(px(8.))
        .rounded(px(crate::theme::RADIUS_SM))
        .flex()
        .items_center()
        .text_size(px(12.))
        .text_color(if on { accent() } else { content(0.55) })
        .when(on, |d| d.bg(accent_wash(0.18)))
        .child(env);
    if enabled {
        base.cursor_pointer()
            .when(!on, |d| d.hover(|s| s.text_color(content(0.9))))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .into_any_element()
    } else {
        base.opacity(0.45).into_any_element()
    }
}

fn link_project<V: 'static>(
    this: &mut V,
    run: Run<V>,
    path: &str,
    workspace: &str,
    environment: &str,
    cx: &mut Context<V>,
) {
    run(
        this,
        "infisical_action",
        json!({"action": {"type": "linkProject", "projectPath": path, "workspaceId": workspace, "environment": environment}}),
        cx,
    );
}

fn connect_form<V: 'static>(
    pending: &Pending,
    edit: Edit<V>,
    run: Run<V>,
    cx: &mut Context<V>,
) -> (String, String, AnyElement, Vec<AnyElement>) {
    let (id, secret, busy, error) = {
        let now = state();
        (
            now.client_id.clone(),
            now.client_secret.clone(),
            now.busy,
            now.error.clone(),
        )
    };
    let form = div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .mt(px(16.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .text_size(px(12.5))
                .text_color(content(0.7))
                .child(logo(Some("infisical"), "Infisical", 36.))
                .child(t(ABOUT)),
        )
        .child(input(
            CLIENT_ID,
            t("ID do cliente"),
            &id,
            "",
            false,
            None,
            pending,
            edit,
            cx,
        ))
        .child(input(
            CLIENT_SECRET,
            t("Segredo do cliente"),
            &secret,
            "",
            true,
            None,
            pending,
            edit,
            cx,
        ))
        .when_some(error, |d, error| {
            d.child(div().text_size(px(12.)).text_color(danger()).child(error))
        })
        .into_any_element();
    let send_pending = pending.clone();
    let cancel_pending = pending.clone();
    let buttons = vec![
        button(
            "if-cancel",
            t("Cancelar"),
            false,
            true,
            cx,
            move |this, cx| {
                if cancel_pending.is_some() {
                    edit(this, "cd-idle", String::new(), cx);
                }
                close_dialog(cx);
            },
        ),
        button(
            "if-save",
            t("Conectar"),
            true,
            !busy,
            cx,
            move |this, cx| {
                if let Some((key, text)) = &send_pending {
                    commit(key, text);
                }
                let (id, secret) = {
                    let mut now = state();
                    if now.client_id.is_empty() || now.client_secret.is_empty() {
                        now.error = Some(t("Preencha o ID e o segredo do cliente.").to_owned());
                        cx.notify();
                        return;
                    }
                    now.busy = true;
                    now.error = None;
                    (now.client_id.clone(), now.client_secret.clone())
                };
                if send_pending.is_some() {
                    edit(this, "cd-idle", String::new(), cx);
                }
                run(
                    this,
                    "infisical_action",
                    json!({"action": {"type": "connect", "clientId": id, "clientSecret": secret}}),
                    cx,
                );
                cx.notify();
            },
        ),
    ];
    (
        t("Conectar Infisical").to_owned(),
        t("A identidade de máquina “sirus” (Universal Auth). Fica no Keychain, nunca nos ajustes.")
            .to_owned(),
        form,
        buttons,
    )
}

/// One line of the projects dialog: mark, name, Infisical project ▾, environment, Terminal.
#[allow(clippy::too_many_arguments)]
fn project_line<V: 'static>(
    at: usize,
    project: &ProjectRow,
    link: Option<&LinkRow>,
    infisical: &[(String, String)],
    picker_open: bool,
    usable: bool,
    run: Run<V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let path = project.path.clone();
    let linked_name = link.map(|link| {
        infisical
            .iter()
            .find(|(id, _)| *id == link.workspace)
            .map_or_else(
                || link.workspace.chars().take(8).collect(),
                |(_, n)| n.clone(),
            )
    });
    let toggle_path = path.clone();
    let picker_button = div()
        .id(("if-pick", at))
        .h(px(26.))
        .w(px(150.))
        .px(px(9.))
        .flex_shrink_0()
        .rounded(px(crate::theme::RADIUS_MD))
        .border_1()
        .border_color(if picker_open {
            accent()
        } else {
            crate::theme::border()
        })
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(12.5))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(content(if linked_name.is_some() { 0.9 } else { 0.45 }))
                .child(
                    linked_name
                        .clone()
                        .unwrap_or_else(|| t("Escolher projeto").to_owned()),
                ),
        )
        .child(
            svg()
                .path("chevron-down")
                .size(px(12.))
                .text_color(content(0.5)),
        );
    let picker_button = if usable {
        picker_button
            .cursor_pointer()
            .hover(|s| s.bg(content(0.05)))
            .on_click(cx.listener(move |_, _, _, cx| {
                let mut now = state();
                now.picker = if now.picker.as_deref() == Some(toggle_path.as_str()) {
                    None
                } else {
                    Some(toggle_path.clone())
                };
                drop(now);
                cx.notify();
            }))
            .into_any_element()
    } else {
        picker_button.opacity(0.45).into_any_element()
    };
    let mut picker = div().relative().flex_shrink_0().child(picker_button);
    if picker_open {
        let environment = link.map_or("dev", |l| l.environment.as_str()).to_owned();
        let choices: Vec<AnyElement> = infisical
            .iter()
            .enumerate()
            .map(|(c_at, (workspace, name))| {
                let (path, workspace, environment) =
                    (path.clone(), workspace.clone(), environment.clone());
                let chosen = link.is_some_and(|l| l.workspace == workspace);
                div()
                    .id(("if-choice", at * 1000 + c_at))
                    .h(px(28.))
                    .px(px(8.))
                    .flex_shrink_0()
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .text_color(content(0.9))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.07)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        {
                            let mut now = state();
                            now.picker = None;
                            let terminal = now
                                .links
                                .get(&path)
                                .cloned()
                                .flatten()
                                .is_none_or(|l| l.terminal);
                            now.links.insert(
                                path.clone(),
                                Some(LinkRow {
                                    workspace: workspace.clone(),
                                    environment: environment.clone(),
                                    source: "folder".to_owned(),
                                    terminal,
                                }),
                            );
                        }
                        link_project(this, run, &path, &workspace, &environment, cx);
                        cx.notify();
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                    .when(chosen, |d| {
                        d.child(svg().path("check").size(px(12.)).text_color(accent()))
                    })
                    .into_any_element()
            })
            .collect();
        let list = div()
            .id(("if-choices", at))
            .absolute()
            .top(px(30.))
            .left(px(0.))
            .w(px(220.))
            .max_h(px(240.))
            .overflow_y_scroll()
            .p(px(4.))
            .rounded(px(crate::theme::RADIUS_LG))
            .border_1()
            .border_color(crate::theme::border())
            .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
            .shadow_md()
            .occlude()
            .on_mouse_down_out(cx.listener(|_, _, _, cx| {
                state().picker = None;
                cx.notify();
            }))
            .flex()
            .flex_col()
            .when(infisical.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(8.))
                        .py(px(5.))
                        .text_size(px(12.5))
                        .text_color(content(0.6))
                        .child(t("Nenhum projeto no Infisical.")),
                )
            })
            .children(choices);
        picker = picker.child(gpui::deferred(list).with_priority(20));
    }

    let envs: Vec<AnyElement> = ENVIRONMENTS
        .iter()
        .enumerate()
        .map(|(e_at, slug)| {
            let path = path.clone();
            let project_id = project.id.clone();
            let chosen_env = project.scripts.is_some();
            let workspace = link.map(|l| l.workspace.clone());
            let terminal = link.is_some_and(|l| l.terminal);
            env_pill(
                ("if-env", at * 8 + e_at),
                slug,
                link.is_some_and(|l| l.environment == *slug),
                usable && workspace.is_some(),
                cx,
                move |this, cx| {
                    let Some(workspace) = workspace.clone() else {
                        return;
                    };
                    if let Some(Some(row)) = state().links.get_mut(&path) {
                        row.environment = (*slug).to_owned();
                    }
                    link_project(this, run, &path, &workspace, slug, cx);
                    // An environment chosen for the terminal follows the link's.
                    if chosen_env && terminal {
                        run(
                            this,
                            "native_infisical_scripts",
                            json!({"projectId": project_id, "environment": slug}),
                            cx,
                        );
                    }
                    cx.notify();
                },
            )
        })
        .collect();
    let segmented = div()
        .flex_shrink_0()
        .flex()
        .p(px(2.))
        .rounded(px(crate::theme::RADIUS_MD))
        .border_1()
        .border_color(crate::theme::border())
        .children(envs);

    let on = link.is_some_and(|l| l.terminal);
    let switchable = usable && link.is_some();
    let terminal_env = link.map(|l| l.environment.clone());
    let project_id = project.id.clone();
    let switch_path = path.clone();
    let terminal = div()
        .id(("if-terminal", at))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(7.))
        .text_size(px(12.))
        .text_color(content(0.6))
        .child(t("Terminal"))
        .child(switch_track(9000 + at, on, switchable));
    let terminal = if switchable {
        terminal
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(Some(row)) = state().links.get_mut(&switch_path) {
                    row.terminal = !on;
                }
                run(
                    this,
                    "native_infisical_scripts",
                    json!({"projectId": project_id, "environment": if on { Value::Null } else { json!(terminal_env) }}),
                    cx,
                );
                cx.notify();
            }))
            .into_any_element()
    } else {
        terminal.opacity(0.45).into_any_element()
    };

    div()
        .id(("if-line", at))
        .h(px(44.))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(10.))
        .when(at > 0, |d| {
            d.border_t_1().border_color(crate::theme::border())
        })
        .child(logo(None, &project.name, 24.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(content(0.95))
                .child(project.name.clone()),
        )
        .child(picker)
        .child(segmented)
        .child(terminal)
        .into_any_element()
}

fn projects_form<V: 'static>(
    run: Run<V>,
    cx: &mut Context<V>,
) -> (String, String, AnyElement, Vec<AnyElement>) {
    let (projects, links, infisical, picker, logged_in, error) = {
        let now = state();
        (
            now.projects.clone(),
            now.links.clone(),
            now.infisical.clone(),
            now.picker.clone(),
            now.logged_in,
            now.status_error.clone(),
        )
    };
    let lines: Vec<AnyElement> = projects
        .iter()
        .enumerate()
        .map(|(at, project)| {
            let link = links.get(&project.path).cloned().flatten();
            let open = picker.as_deref() == Some(project.path.as_str());
            project_line(
                at,
                project,
                link.as_ref(),
                &infisical,
                open,
                logged_in,
                run,
                cx,
            )
        })
        .collect();
    let block = div()
        .rounded(px(crate::theme::RADIUS_LG))
        .border_1()
        .border_color(crate::theme::border())
        .flex()
        .flex_col()
        .when(lines.is_empty(), |d| {
            d.child(
                div()
                    .p(px(12.))
                    .text_size(px(12.5))
                    .text_color(content(0.55))
                    .child(t("Nenhum projeto no Sirus ainda.")),
            )
        })
        .children(lines);
    let form = div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .mt(px(16.))
        .when(!logged_in, |d| {
            d.child(
                div().text_size(px(12.)).text_color(danger()).child(
                    error.unwrap_or_else(|| {
                        t("As credenciais estão salvas, mas o login falhou.").to_owned()
                    }),
                ),
            )
        })
        .child(block)
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(17.))
                .text_color(content(0.5))
                .child(t(
                    "As chaves ficam no Infisical. O Sirus só mostra os nomes delas e entrega o valor ao agente sem que ele o veja. Os scripts do projeto rodam com as chaves; o ambiente dos agentes não muda.",
                )),
        )
        .into_any_element();
    let buttons = vec![button(
        "if-done",
        t("Concluído"),
        true,
        true,
        cx,
        |_, cx| close_dialog(cx),
    )];
    (
        t("Projetos e chaves").to_owned(),
        t("Cada projeto do Sirus usa as chaves de um projeto do Infisical. Um projeto com .infisical.json, ou com o mesmo nome no Infisical, já vem escolhido. Terminais novos e scripts recebem as chaves.")
            .to_owned(),
        form,
        buttons,
    )
}

/// The open Infisical dialog, in the style of the connector dialog.
pub fn overlay<V: 'static>(
    editing: Option<(&str, &str)>,
    edit: Edit<V>,
    run: Run<V>,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    let d = state().dialog?;
    let pending: Pending = editing.map(|(k, v)| (k.to_string(), v.to_string()));
    let closing = d.closing;
    let (title, help, form, buttons) = match d.kind {
        DialogKind::Connect => connect_form(&pending, edit, run, cx),
        DialogKind::Projects => projects_form(run, cx),
    };
    let width = match d.kind {
        DialogKind::Connect => 500.,
        DialogKind::Projects => 680.,
    };
    let card = div()
        .id("if-card-dialog")
        .relative()
        .w(px(width))
        .max_w_full()
        .max_h(gpui::relative(0.92))
        .overflow_y_scroll()
        .p(px(20.))
        .rounded(px(crate::theme::RADIUS_LG))
        .border_1()
        .border_color(crate::theme::border())
        .bg(floating_fill())
        .shadow_lg()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(0.97))
                .child(title),
        )
        .child(
            div()
                .mt(px(4.))
                .text_size(px(12.5))
                .line_height(px(18.))
                .text_color(content(0.55))
                .child(help),
        )
        .child(form)
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .mt(px(20.))
                .children(buttons),
        )
        .with_animation(
            format!("if-dialog-{}-{closing}", d.generation),
            Animation::new(Duration::from_secs_f32(DIALOG_TIME))
                .with_easing(|t| eased(t, Curve::Expo)),
            move |el, t| {
                let p = if closing { 1. - t } else { t };
                el.opacity(p).top(px((1. - p) * 6.))
            },
        );
    Some(
        div()
            .id("if-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    close_dialog(cx);
                    cx.stop_propagation();
                }),
            )
            .child(card)
            .with_animation(
                format!("if-backdrop-{}-{closing}", d.generation),
                Animation::new(Duration::from_secs_f32(DIALOG_TIME))
                    .with_easing(|t| eased(t, Curve::Expo)),
                move |el, t| {
                    let p = if closing { 1. - t } else { t };
                    el.bg(Hsla {
                        a: 0.45 * p,
                        ..gpui::black()
                    })
                },
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str) -> ProjectRow {
        ProjectRow {
            id: path.into(),
            name: path.into(),
            path: path.into(),
            scripts: None,
        }
    }

    #[test]
    fn a_link_reply_is_kept_by_path_and_an_unlinked_one_clears_it() {
        reply(
            true,
            &json!({"kind": "link", "projectPath": "/tmp/a", "linked": true, "workspaceId": "w1", "environment": "prod"}),
        );
        assert_eq!(link("/tmp/a"), Some(("w1".into(), "prod".into())));
        reply(
            true,
            &json!({"kind": "link", "projectPath": "/tmp/a", "linked": false}),
        );
        assert_eq!(link("/tmp/a"), None);
    }

    #[test]
    fn the_status_reply_counts_projects_and_carries_every_link() {
        let refresh = reply(
            true,
            &json!({"kind": "status", "connected": true, "loggedIn": true, "identity": "sirus",
            "projects": [{"id": "w1", "name": "App", "environments": []}],
            "links": [
                {"kind": "link", "projectPath": "/tmp/s1", "linked": true, "workspaceId": "w1",
                 "environment": "dev", "source": "name", "terminal": true},
                {"kind": "link", "projectPath": "/tmp/s2", "linked": false, "terminal": false}
            ]}),
        );
        assert!(!refresh);
        assert_eq!(infisical_name("w1").as_deref(), Some("App"));
        assert!(state().connected);
        let links = state().links.clone();
        assert_eq!(
            links["/tmp/s1"],
            Some(LinkRow {
                workspace: "w1".into(),
                environment: "dev".into(),
                source: "name".into(),
                terminal: true,
            })
        );
        assert_eq!(links["/tmp/s2"], None);
        assert!(reply(true, &json!({"kind": "connected"})));
    }

    #[test]
    fn the_card_counts_linked_and_unlinked_folders_only_once_known() {
        let mut links = BTreeMap::new();
        links.insert(
            "/a".to_owned(),
            Some(LinkRow {
                workspace: "w".into(),
                environment: "dev".into(),
                source: "repo".into(),
                terminal: true,
            }),
        );
        links.insert("/b".to_owned(), None);
        links.insert("/c".to_owned(), None);
        let projects = [row("/a"), row("/b"), row("/c"), row("/unknown")];
        assert_eq!(link_counts(&projects, &links), (1, 2));
        crate::i18n::with_locale(false, || {
            assert_eq!(
                card_description(true, None, 3, "sirus"),
                "Chaves de 3 projetos · como sirus"
            );
            assert_eq!(
                card_description(true, None, 1, "sirus"),
                "Chaves de 1 projeto · como sirus"
            );
            assert_eq!(unlinked_line(2), "2 pastas sem projeto");
            assert_eq!(unlinked_line(1), "1 pasta sem projeto");
        });
    }

    #[test]
    fn client_fields_are_trimmed_and_keep_to_their_keys() {
        assert!(commit(CLIENT_ID, "  abc "));
        assert!(!commit("other", "x"));
        assert_eq!(state().client_id, "abc");
        state().client_id.clear();
    }
}
