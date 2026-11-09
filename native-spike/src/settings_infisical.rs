//! Settings → Conectores → Infisical (ADR-198): the machine identity that reaches the owner's
//! project keys, and for each Sirus project the Infisical project and environment it uses. The
//! host does the work (`infisical_action`, src-tauri `infisical.rs`); the window only keeps what it
//! shows: whether it is connected, the organization's project names, each Sirus project's link and
//! the form being typed. Client secrets are masked while typed and cleared once sent. The window
//! never sees a key's value, only names.

use crate::i18n::{t, tf};
use crate::settings_connectors::{Pending, foot, input, item, logo, section, signed_in_badge, tag};
use crate::settings_pages::{Edit, Run, switch_track};
use crate::theme::{accent, accent_wash, content, on_accent};
use gpui::{AnyElement, Context, FontWeight, div, prelude::*, px};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

pub(crate) const CLIENT_ID: &str = "if-client-id";
pub(crate) const CLIENT_SECRET: &str = "if-client-secret";
/// The environments a project can be linked to.
pub(crate) const ENVIRONMENTS: [&str; 3] = ["dev", "staging", "prod"];

/// A Sirus project as this page needs it.
#[derive(Clone)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub path: String,
    /// The environment its scripts run under with the Infisical keys, when that is on.
    pub scripts: Option<String>,
}

struct State {
    projects: Vec<ProjectRow>,
    /// Sirus project path to (Infisical project id, environment).
    links: BTreeMap<String, (String, String)>,
    /// Paths whose link was asked for, so nothing asks twice.
    asked: BTreeSet<String>,
    status_known: bool,
    connected: bool,
    logged_in: bool,
    identity: String,
    /// Infisical projects: (id, name).
    infisical: Vec<(String, String)>,
    status_error: Option<String>,
    form: bool,
    client_id: String,
    client_secret: String,
    busy: bool,
    error: Option<String>,
    /// The Sirus project (path) whose Infisical project list is open.
    open: Option<String>,
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
    form: false,
    client_id: String::new(),
    client_secret: String::new(),
    busy: false,
    error: None,
    open: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub fn set_projects(rows: Vec<ProjectRow>) {
    state().projects = rows;
}

/// `(Infisical project id, environment)` the Sirus project at `path` is linked to.
pub(crate) fn link(path: &str) -> Option<(String, String)> {
    state().links.get(path).cloned()
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
            false
        }
        Some("connected") => {
            now.busy = false;
            now.form = false;
            now.error = None;
            now.client_id.clear();
            now.client_secret.clear();
            true
        }
        Some("disconnected") => {
            now.connected = false;
            now.logged_in = false;
            now.infisical.clear();
            true
        }
        Some("link") => {
            if let Some(path) = value["projectPath"].as_str() {
                if value["linked"].as_bool() == Some(true) {
                    now.links.insert(
                        path.to_owned(),
                        (
                            value["workspaceId"].as_str().unwrap_or("").to_owned(),
                            value["environment"].as_str().unwrap_or("dev").to_owned(),
                        ),
                    );
                } else {
                    now.links.remove(path);
                }
            }
            false
        }
        _ => false,
    }
}

/// A finished text edit of the form. True when the key was one of its own.
pub fn commit(key: &str, text: &str) -> bool {
    let mut now = state();
    match key {
        CLIENT_ID => now.client_id = text.trim().chars().take(256).collect(),
        CLIENT_SECRET => now.client_secret = text.trim().chars().take(1024).collect(),
        _ => return false,
    }
    true
}

fn pill<V: 'static>(
    id: impl Into<gpui::ElementId>,
    label: String,
    main: bool,
    enabled: bool,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    let base = div()
        .id(id)
        .h(px(26.))
        .px(px(10.))
        .flex_shrink_0()
        .rounded(px(crate::theme::RADIUS_SM))
        .flex()
        .items_center()
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if main { on_accent() } else { content(0.85) })
        .bg(if main { accent() } else { content(0.08) })
        .child(label);
    if enabled {
        base.cursor_pointer()
            .hover(|s| s.opacity(0.88))
            .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
            .into_any_element()
    } else {
        base.opacity(0.45).into_any_element()
    }
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
        .h(px(24.))
        .px(px(9.))
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

pub fn view<V: 'static>(
    editing: Option<(&str, &str)>,
    edit: Edit<V>,
    run: Run<V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let (snapshot_projects, links, form, id, secret, busy, error, open) = {
        let now = state();
        (
            now.projects.clone(),
            now.links.clone(),
            now.form,
            now.client_id.clone(),
            now.client_secret.clone(),
            now.busy,
            now.error.clone(),
            now.open.clone(),
        )
    };
    let (connected, logged_in, identity, infisical, status_error, status_known) = {
        let now = state();
        (
            now.connected,
            now.logged_in,
            now.identity.clone(),
            now.infisical.clone(),
            now.status_error.clone(),
            now.status_known,
        )
    };
    let pending: Pending = editing.map(|(k, v)| (k.to_string(), v.to_string()));

    let description = if !status_known {
        t("Verificando…").to_owned()
    } else if connected && logged_in {
        match infisical.len() {
            1 => tf("Conectado como {} · 1 projeto", &[&identity]),
            n => tf("Conectado como {} · {} projetos", &[&identity, &n]),
        }
    } else if connected {
        status_error
            .unwrap_or_else(|| t("As credenciais estão salvas, mas o login falhou.").to_owned())
    } else {
        t("Não conectado").to_owned()
    };
    let action = if connected {
        pill(
            "if-disconnect",
            t("Desconectar").to_owned(),
            false,
            true,
            cx,
            move |this, cx| {
                run(
                    this,
                    "infisical_action",
                    json!({"action": {"type": "disconnect"}}),
                    cx,
                );
                cx.notify();
            },
        )
    } else {
        pill(
            "if-connect",
            t("Conectar").to_owned(),
            true,
            status_known,
            cx,
            move |_, cx| {
                let mut now = state();
                now.form = !now.form;
                now.error = None;
                drop(now);
                cx.notify();
            },
        )
    };
    let mut name_line = vec![
        div()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(content(0.95))
            .child("Infisical")
            .into_any_element(),
    ];
    if connected && logged_in {
        name_line.push(signed_in_badge());
    }
    let mut rows: Vec<AnyElement> = vec![item(
        "if-row".to_owned(),
        logo(None, "Infisical", 28.),
        name_line,
        description,
        false,
        vec![action],
        "",
    )];

    if form && !connected {
        let send_pending = pending.clone();
        let can_send = !busy;
        rows.push(
            div()
                .mx(px(8.))
                .mb(px(6.))
                .p(px(14.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(input(
                    CLIENT_ID,
                    t("ID do cliente"),
                    &id,
                    "",
                    false,
                    None,
                    &pending,
                    edit,
                    cx,
                ))
                .child(input(
                    CLIENT_SECRET,
                    t("Segredo do cliente"),
                    &secret,
                    "",
                    true,
                    Some(
                        t("A identidade de máquina “sirus” (Universal Auth). Fica no Keychain, nunca nos ajustes.")
                            .to_owned(),
                    ),
                    &pending,
                    edit,
                    cx,
                ))
                .when_some(error.clone(), |d, error| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.75))
                            .child(error),
                    )
                })
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(pill(
                            "if-save",
                            t("Conectar").to_owned(),
                            true,
                            can_send,
                            cx,
                            move |this, cx| {
                                if let Some((key, text)) = &send_pending {
                                    commit(key, text);
                                }
                                let (id, secret) = {
                                    let mut now = state();
                                    if now.client_id.is_empty() || now.client_secret.is_empty() {
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
                        ))
                        .child(pill(
                            "if-cancel",
                            t("Cancelar").to_owned(),
                            false,
                            true,
                            cx,
                            move |this, cx| {
                                let mut now = state();
                                now.form = false;
                                now.client_id.clear();
                                now.client_secret.clear();
                                now.error = None;
                                drop(now);
                                edit(this, "cd-idle", String::new(), cx);
                                cx.notify();
                            },
                        )),
                )
                .into_any_element(),
        );
    }

    let mut page = div()
        .w_full()
        .flex()
        .flex_col()
        .child(section("Infisical", rows, 1));

    // The Sirus projects and what each one uses.
    if !snapshot_projects.is_empty() {
        let usable = connected && logged_in;
        let mut items: Vec<AnyElement> = Vec::new();
        for (at, project) in snapshot_projects.iter().enumerate() {
            let linked = links.get(&project.path).cloned();
            let linked_name = linked.as_ref().map(|(workspace, _)| {
                infisical
                    .iter()
                    .find(|(id, _)| id == workspace)
                    .map_or_else(|| workspace.chars().take(8).collect(), |(_, n)| n.clone())
            });
            let env = linked.as_ref().map(|(_, e)| e.clone());
            let path = project.path.clone();
            let list_open = open.as_deref() == Some(path.as_str());
            let picker_label = linked_name
                .clone()
                .unwrap_or_else(|| t("Escolher projeto").to_owned());
            let toggle_path = path.clone();
            let picker = pill(
                ("if-pick", at),
                picker_label,
                false,
                usable,
                cx,
                move |_, cx| {
                    let mut now = state();
                    now.open = if now.open.as_deref() == Some(toggle_path.as_str()) {
                        None
                    } else {
                        Some(toggle_path.clone())
                    };
                    drop(now);
                    cx.notify();
                },
            );
            let mut actions = vec![picker];
            let envs: Vec<AnyElement> = ENVIRONMENTS
                .iter()
                .enumerate()
                .map(|(e_at, slug)| {
                    let (path, scripts_on, project_id) =
                        (path.clone(), project.scripts.is_some(), project.id.clone());
                    let workspace = linked.as_ref().map(|(w, _)| w.clone());
                    env_pill(
                        ("if-env", at * 8 + e_at),
                        slug,
                        env.as_deref() == Some(*slug),
                        usable && workspace.is_some(),
                        cx,
                        move |this, cx| {
                            let Some(workspace) = workspace.clone() else {
                                return;
                            };
                            run(
                                this,
                                "infisical_action",
                                json!({"action": {"type": "linkProject", "projectPath": path, "workspaceId": workspace, "environment": slug}}),
                                cx,
                            );
                            if scripts_on {
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
            actions.push(
                div()
                    .flex_shrink_0()
                    .flex()
                    .p(px(2.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .border_1()
                    .border_color(crate::theme::border())
                    .children(envs)
                    .into_any_element(),
            );
            let mut name_line = vec![
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(0.95))
                    .child(project.name.clone())
                    .into_any_element(),
            ];
            if let Some(env) = env.clone() {
                name_line.push(tag(env, true));
            }
            let mut block = div().flex().flex_col().child(item(
                format!("if-project-{at}"),
                logo(None, &project.name, 28.),
                name_line,
                match linked_name {
                    Some(name) => tf("Usa as chaves de {}", &[&name]),
                    None => t("Sem vínculo com o Infisical").to_owned(),
                },
                false,
                actions,
                "",
            ));
            if list_open {
                let choices: Vec<AnyElement> = infisical
                    .iter()
                    .enumerate()
                    .map(|(c_at, (workspace, name))| {
                        let (path, workspace, env) = (
                            path.clone(),
                            workspace.clone(),
                            env.clone().unwrap_or_else(|| "dev".to_owned()),
                        );
                        div()
                            .id(("if-choice", at * 1000 + c_at))
                            .px(px(8.))
                            .py(px(5.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .text_size(px(13.))
                            .text_color(content(0.92))
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.08)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                state().open = None;
                                run(
                                    this,
                                    "infisical_action",
                                    json!({"action": {"type": "linkProject", "projectPath": path, "workspaceId": workspace, "environment": env}}),
                                    cx,
                                );
                                cx.notify();
                            }))
                            .child(name.clone())
                            .into_any_element()
                    })
                    .collect();
                block = block.child(
                    div()
                        .mx(px(8.))
                        .mb(px(4.))
                        .p(px(4.))
                        .max_h(px(200.))
                        .overflow_hidden()
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(crate::theme::border())
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
                        .children(choices),
                );
            }
            if let (true, Some(env)) = (usable, env.clone()) {
                let on = project.scripts.is_some();
                let project_id = project.id.clone();
                block = block.child(
                    div()
                        .id(("if-scripts", at))
                        .ml(px(47.))
                        .mr(px(8.))
                        .mb(px(6.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            run(
                                this,
                                "native_infisical_scripts",
                                json!({"projectId": project_id, "environment": if on { Value::Null } else { json!(env) }}),
                                cx,
                            );
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(12.5))
                                .text_color(content(0.7))
                                .child(t("Rodar scripts com as chaves do Infisical")),
                        )
                        .child(switch_track(9000 + at, on, true)),
                );
            }
            items.push(block.into_any_element());
        }
        page = page.child(section(t("Projetos do Sirus"), items, 1));
    }
    page.child(foot(t(
        "As chaves ficam no Infisical. O Sirus só mostra os nomes delas e entrega o valor ao agente sem que ele o veja. Os scripts do projeto rodam com as chaves; o ambiente dos agentes não muda.",
    )))
    .mb(px(28.))
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_status_reply_counts_projects_and_names_the_identity() {
        let refresh = reply(
            true,
            &json!({"kind": "status", "connected": true, "loggedIn": true, "identity": "sirus",
                    "projects": [{"id": "w1", "name": "App", "environments": []}]}),
        );
        assert!(!refresh);
        assert_eq!(infisical_name("w1").as_deref(), Some("App"));
        assert!(state().connected);
        assert!(reply(true, &json!({"kind": "connected"})));
    }

    #[test]
    fn client_fields_are_trimmed_and_keep_to_their_keys() {
        assert!(commit(CLIENT_ID, "  abc "));
        assert!(!commit("other", "x"));
        assert_eq!(state().client_id, "abc");
        state().client_id.clear();
    }
}
