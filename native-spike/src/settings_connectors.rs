//! Settings → Conectores, as Sirus Code draws it (`components/settings/ConnectorsSettings.tsx`
//! and `styles/connectors.css`, ADR-137): a search bar with "Adicionar personalizado", an optional
//! scope filter, the connected list and the catalog in two columns of brand marks, a "⋯" menu on
//! each connected row and one dialog for connecting, editing and renaming.
//!
//! The page keeps its own small state here (query, open menu, the dialog being filled): the
//! text fields go through the shell's text editing (`edit`, then `commit` on Enter), and the list
//! itself lives in AppSettings (`connectors`). Each connector can carry a `label`, a name the
//! person chose to tell accounts apart ("Conta Gabriel", "Conta Moreira"). It is only shown: the
//! MCP server name agents use comes from `name` (src-tauri `connectors::unique_server_name`), so a
//! rename never changes a tool id. The account a sign-in belongs to (an email, a login) comes from
//! the host's status when the service reports one.

use crate::connector_marks::MARKS;
use crate::i18n::{t, tf};
use crate::motion::{Curve, eased};
use crate::settings_pages::{Edit, Run, Save};
use crate::theme::{accent, content, floating_fill, on_accent};
use gpui::{
    Animation, AnimationExt, AnyElement, Context, FontWeight, Hsla, div, prelude::*, px, rgb, svg,
};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The agents that take connectors (`CONNECTOR_AGENTS` in `src/lib/connectors.ts`).
const CONNECTOR_AGENTS: &[&str] = &["claude", "codex", "opencode", "cursor", "devin", "hermes"];

/// The catalog's category sections, in order.
const CONNECTOR_GROUPS: &[(&str, &str)] = &[
    ("popular", "Populares"),
    ("developer", "Ferramentas de desenvolvimento"),
    ("productivity", "Produtividade"),
];

/// The brands whose mark is a PNG from their site (Simple Icons has none).
const IMAGES: &[&str] = &["canva", "context7", "monday"];

const SEARCH: &str = "cn-search";
const NAME: &str = "cd-name";
const URL: &str = "cd-url";
const TOKEN: &str = "cd-token";
const REF: &str = "cd-ref";
const LABEL: &str = "cd-label";
/// A text edit that goes nowhere: parks typing when a dialog control other than a field is used.
const IDLE: &str = "cd-idle";

const MENU_TIME: f32 = 0.12;
const DIALOG_TIME: f32 = 0.18;

// Assets: the brand marks and the few icons the menu needs.

const PNGS: &[(&str, &[u8])] = &[
    ("canva", include_bytes!("../assets/connectors/canva.png")),
    (
        "context7",
        include_bytes!("../assets/connectors/context7.png"),
    ),
    ("monday", include_bytes!("../assets/connectors/monday.png")),
];

/// Lucide icons (ISC) the menu uses; the app's own set has no sign-in, sign-out or power.
const ICONS: &[(&str, &str)] = &[
    (
        "log-in",
        r#"<path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4"/><path d="m10 17 5-5-5-5"/><path d="M15 12H3"/>"#,
    ),
    (
        "log-out",
        r#"<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><path d="m16 17 5-5-5-5"/><path d="M21 12H9"/>"#,
    ),
    (
        "power",
        r#"<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>"#,
    ),
    (
        "ellipsis",
        r#"<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>"#,
    ),
];

/// A bundled file of this page: `connectors/<id>.svg` (a brand mark), `connectors/<id>.png` or
/// `connector-ui/<name>` (a menu icon).
pub fn asset(path: &str) -> Option<Cow<'static, [u8]>> {
    if let Some(id) = path
        .strip_prefix("connectors/")
        .and_then(|rest| rest.strip_suffix(".svg"))
    {
        let (_, data, _) = MARKS.iter().find(|(known, _, _)| *known == id)?;
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="white" d="{data}"/></svg>"#
        );
        return Some(Cow::Owned(svg.into_bytes()));
    }
    if let Some(id) = path
        .strip_prefix("connectors/")
        .and_then(|rest| rest.strip_suffix(".png"))
    {
        let (_, bytes) = PNGS.iter().find(|(known, _)| *known == id)?;
        return Some(Cow::Borrowed(bytes));
    }
    let name = path.strip_prefix("connector-ui/")?;
    let (_, body) = ICONS.iter().find(|(known, _)| *known == name)?;
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
    );
    Some(Cow::Owned(svg.into_bytes()))
}

// State.

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Connect,
    Edit,
    Rename,
}

struct Menu {
    id: String,
    closing: bool,
}

#[derive(Clone)]
struct Dialog {
    kind: Kind,
    /// The connector being edited or renamed, or the id a new one will take.
    id: String,
    /// The catalog entry a new connector comes from (its id), if any.
    entry: Option<&'static str>,
    name: String,
    label: String,
    url: String,
    auth: String,
    token: String,
    had_token: bool,
    project_only: bool,
    project: String,
    project_ref: String,
    read_only: bool,
    agents: Vec<String>,
    enabled: bool,
    /// Whether the connector was signed in when the dialog opened.
    was_connected: bool,
    error: Option<String>,
    closing: bool,
    /// Counts openings, so each starts its animation again.
    generation: u32,
}

struct State {
    query: String,
    project_only: bool,
    menu: Option<Menu>,
    /// When the last menu closed: a click on its button then must not reopen it.
    menu_closed: Option<Instant>,
    dialog: Option<Dialog>,
    /// A field to start typing in as soon as its dialog shows.
    autofocus: Option<(&'static str, String)>,
    generation: u32,
    /// Connectors whose sign-in is waiting on the browser.
    signing: Vec<String>,
    projects: Vec<(String, String)>,
    current: Option<String>,
    width: f32,
    installed: Vec<String>,
}

static STATE: Mutex<State> = Mutex::new(State {
    autofocus: None,
    query: String::new(),
    project_only: false,
    menu: None,
    menu_closed: None,
    dialog: None,
    generation: 0,
    signing: Vec::new(),
    projects: Vec::new(),
    current: None,
    width: 716.,
    installed: Vec::new(),
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// What the shell knows that the page needs: the projects, the open one, and the page's width.
pub fn set_context(projects: Vec<(String, String)>, current: Option<String>, width: f32) {
    let mut now = state();
    now.projects = projects;
    now.current = current;
    now.width = width;
}

/// The host answered a connector command (a sign-in finished or failed): nothing waits any more.
pub fn finish() {
    state().signing.clear();
}

/// True while a menu or the dialog is up and wants Enter and Escape.
/// The field the dialog that just opened wants the caret in.
pub fn take_autofocus() -> Option<(&'static str, String)> {
    state().autofocus.take()
}

/// Whether Enter in this field also saves the dialog, which has nothing else to fill in.
pub fn enter_saves(key: &str) -> bool {
    key == LABEL
        && state()
            .dialog
            .as_ref()
            .is_some_and(|d| d.kind == Kind::Rename)
}

pub fn wants_keys() -> bool {
    let now = state();
    now.menu.is_some() || now.dialog.is_some()
}

/// A finished text edit of this page. True when the key was one of its own.
pub fn commit(key: &str, text: &str) -> bool {
    if !matches!(key, SEARCH | NAME | URL | TOKEN | REF | LABEL | IDLE) {
        return false;
    }
    set_field(key, text);
    true
}

fn set_field(key: &str, text: &str) {
    let mut now = state();
    if key == SEARCH {
        now.query = text.trim().to_string();
        return;
    }
    let Some(dialog) = now.dialog.as_mut() else {
        return;
    };
    match key {
        // The host takes names and labels up to 64 characters.
        NAME => dialog.name = text.trim().chars().take(64).collect(),
        URL => dialog.url = text.trim().to_string(),
        TOKEN => dialog.token = text.trim().to_string(),
        LABEL => dialog.label = text.trim().chars().take(64).collect(),
        REF => {
            dialog.project_ref = text
                .trim()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect()
        }
        _ => {}
    }
}

/// The text being edited, carried into click handlers so a click elsewhere keeps it.
type Pending = Option<(String, String)>;

fn flush(pending: &Pending) {
    if let Some((key, text)) = pending {
        set_field(key, text);
    }
}

// Catalog.

/// The catalog as `src/lib/connectors-catalog.ts` lists it: (id, name, category, description, url,
/// auth). Each service's official remote MCP server; GitHub signs in with a token, Context7 is open.
const CATALOG: &[(&str, &str, &str, &str, &str, &str)] = &[
    (
        "supabase",
        "Supabase",
        "popular",
        "Consultar Postgres, gerenciar projetos, funções e storage",
        "https://mcp.supabase.com/mcp",
        "oauth",
    ),
    (
        "linear",
        "Linear",
        "popular",
        "Encontrar, criar e atualizar issues e projetos",
        "https://mcp.linear.app/mcp",
        "oauth",
    ),
    (
        "notion",
        "Notion",
        "popular",
        "Buscar, ler e editar páginas e bancos",
        "https://mcp.notion.com/mcp",
        "oauth",
    ),
    (
        "github",
        "GitHub",
        "popular",
        "Issues, pull requests e repositórios (token)",
        "https://api.githubcopilot.com/mcp/",
        "token",
    ),
    (
        "stripe",
        "Stripe",
        "popular",
        "Clientes, pagamentos e assinaturas",
        "https://mcp.stripe.com",
        "oauth",
    ),
    (
        "vercel",
        "Vercel",
        "popular",
        "Projetos, deploys e logs",
        "https://mcp.vercel.com",
        "oauth",
    ),
    (
        "sentry",
        "Sentry",
        "popular",
        "Investigar erros, issues e traces",
        "https://mcp.sentry.dev/mcp",
        "oauth",
    ),
    (
        "cloudflare",
        "Cloudflare",
        "developer",
        "Workers, KV, R2 e D1",
        "https://mcp.cloudflare.com/mcp",
        "oauth",
    ),
    (
        "neon",
        "Neon",
        "developer",
        "Projetos e branches de Postgres serverless",
        "https://mcp.neon.tech/mcp",
        "oauth",
    ),
    (
        "posthog",
        "PostHog",
        "developer",
        "Analytics de produto, flags e erros",
        "https://mcp.posthog.com/mcp",
        "oauth",
    ),
    (
        "netlify",
        "Netlify",
        "developer",
        "Criar, publicar e gerenciar sites",
        "https://netlify-mcp.netlify.app/mcp",
        "oauth",
    ),
    (
        "railway",
        "Railway",
        "developer",
        "Fazer deploy e gerenciar serviços",
        "https://mcp.railway.com",
        "oauth",
    ),
    (
        "prisma",
        "Prisma",
        "developer",
        "Bancos Prisma Postgres",
        "https://mcp.prisma.io/mcp",
        "oauth",
    ),
    (
        "figma",
        "Figma",
        "developer",
        "Designs, componentes e variáveis",
        "https://mcp.figma.com/mcp",
        "oauth",
    ),
    (
        "huggingface",
        "Hugging Face",
        "developer",
        "Modelos, datasets e Spaces",
        "https://huggingface.co/mcp",
        "oauth",
    ),
    (
        "context7",
        "Context7",
        "developer",
        "Documentação atualizada de bibliotecas",
        "https://mcp.context7.com/mcp",
        "none",
    ),
    (
        "atlassian",
        "Atlassian",
        "productivity",
        "Issues do Jira e páginas do Confluence",
        "https://mcp.atlassian.com/v1/mcp",
        "oauth",
    ),
    (
        "asana",
        "Asana",
        "productivity",
        "Tarefas, projetos e metas",
        "https://mcp.asana.com/mcp",
        "oauth",
    ),
    (
        "todoist",
        "Todoist",
        "productivity",
        "Encontrar, adicionar e concluir tarefas",
        "https://ai.todoist.net/mcp",
        "oauth",
    ),
    (
        "monday",
        "monday.com",
        "productivity",
        "Quadros, itens e atualizações",
        "https://mcp.monday.com/mcp",
        "oauth",
    ),
    (
        "canva",
        "Canva",
        "productivity",
        "Designs e materiais da marca",
        "https://mcp.canva.com/mcp",
        "oauth",
    ),
    (
        "intercom",
        "Intercom",
        "productivity",
        "Conversas e contatos",
        "https://mcp.intercom.com/mcp",
        "oauth",
    ),
    (
        "webflow",
        "Webflow",
        "productivity",
        "Sites, páginas e CMS",
        "https://mcp.webflow.com/mcp",
        "oauth",
    ),
    (
        "wix",
        "Wix",
        "productivity",
        "Sites, lojas e agendamentos",
        "https://mcp.wix.com/mcp",
        "oauth",
    ),
    (
        "paypal",
        "PayPal",
        "productivity",
        "Faturas, pedidos e pagamentos",
        "https://mcp.paypal.com/mcp",
        "oauth",
    ),
    (
        "square",
        "Square",
        "productivity",
        "Pagamentos, catálogo e pedidos",
        "https://mcp.squareup.com/mcp",
        "oauth",
    ),
];

fn entry(
    id: &str,
) -> Option<&'static (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)> {
    CATALOG.iter().find(|entry| entry.0 == id)
}

// Small helpers.

fn success() -> Hsla {
    rgb(0x4cc38a).into()
}

fn danger() -> Hsla {
    rgb(0xec8a85).into()
}

/// The host of a connector's address, without its path or query (which can hold secrets).
fn host_of(url: &str) -> String {
    url.split("://")
        .last()
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// An https address with a host and no credentials, as the host's `valid_url` takes it.
fn valid_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.is_empty()
        && !host.contains('@')
        && url.len() <= 2048
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// The Supabase address for one project and, optionally, read-only (`supabaseUrl`).
fn supabase_url(base: &str, project_ref: &str, read_only: bool) -> String {
    let mut query = Vec::new();
    if !project_ref.is_empty() {
        query.push(format!("project_ref={project_ref}"));
    }
    if read_only {
        query.push("read_only=true".to_string());
    }
    let base = base.split('?').next().unwrap_or(base);
    if query.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", query.join("&"))
    }
}

/// What a Supabase address says about its project: (project ref, read-only) (`supabaseDetails`).
fn supabase_details(url: &str) -> (String, bool) {
    let query = url.split_once('?').map_or("", |(_, query)| query);
    let find = |name: &str| {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
            .unwrap_or_default()
            .to_string()
    };
    (find("project_ref"), find("read_only") == "true")
}

fn agent_name(id: &str) -> &'static str {
    match id {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        "cursor" => "Cursor",
        "devin" => "Devin",
        _ => "Hermes",
    }
}

fn agent_mark(id: &str) -> &'static str {
    match id {
        "claude" => "claude.svg",
        "codex" => "codex-glyph.svg",
        "cursor" => "cursor",
        "devin" => "devin",
        "hermes" => "hermes",
        _ => "opencode.svg",
    }
}

/// Runs `then` after `ms`, on the interface thread. A sign-in waits a moment so the settings
/// saved just before it have reached the host.
fn after<V: 'static>(
    cx: &mut Context<V>,
    ms: u64,
    then: impl FnOnce(&mut V, &mut Context<V>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(ms))
            .await;
        let _ = this.update(cx, |this, cx| then(this, cx));
    })
    .detach();
}

fn close_menu<V: 'static>(cx: &mut Context<V>) {
    {
        let mut now = state();
        match now.menu.as_mut() {
            Some(menu) if !menu.closing => menu.closing = true,
            _ => return,
        }
        now.menu_closed = Some(Instant::now());
    }
    cx.notify();
    after(cx, (MENU_TIME * 1000.) as u64 + 10, |_, cx| {
        let mut now = state();
        if now.menu.as_ref().is_some_and(|menu| menu.closing) {
            now.menu = None;
        }
        cx.notify();
    });
}

fn close_dialog<V: 'static>(cx: &mut Context<V>) {
    {
        let mut now = state();
        match now.dialog.as_mut() {
            Some(dialog) if !dialog.closing => dialog.closing = true,
            _ => return,
        }
    }
    cx.notify();
    after(cx, (DIALOG_TIME * 1000.) as u64 + 10, |_, cx| {
        let mut now = state();
        if now.dialog.as_ref().is_some_and(|dialog| dialog.closing) {
            now.dialog = None;
        }
        cx.notify();
    });
}

/// One saved connector as the page reads it.
struct Conn {
    id: String,
    name: String,
    label: String,
    catalog: Option<String>,
    url: String,
    project: Option<String>,
    enabled: bool,
    auth: String,
    state: String,
    account: Option<String>,
    raw: Value,
}

impl Conn {
    /// What the person calls it: their label, else its name.
    fn title(&self) -> &str {
        if self.label.is_empty() {
            &self.name
        } else {
            &self.label
        }
    }
}

fn text_of(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

fn open_dialog(
    kind: Kind,
    connector: Option<&Value>,
    entry: Option<&'static str>,
    connected: bool,
) {
    let mut now = state();
    now.generation += 1;
    let installed = now.installed.clone();
    let current = now.current.clone();
    let first = now.projects.first().map(|(id, _)| id.clone());
    let catalog = entry.and_then(self::entry);
    let (reference, read_only) = connector
        .map(|c| supabase_details(c["url"].as_str().unwrap_or_default()))
        .unwrap_or_default();
    let project = connector.and_then(|c| c["projectId"].as_str().map(str::to_string));
    let dialog = Dialog {
        kind,
        id: connector.map_or_else(new_id, |c| text_of(&c["id"])),
        entry,
        name: connector
            .map(|c| text_of(&c["name"]))
            .or_else(|| catalog.map(|e| e.1.to_string()))
            .unwrap_or_default(),
        label: connector.map(|c| text_of(&c["label"])).unwrap_or_default(),
        url: connector
            .map(|c| text_of(&c["url"]))
            .or_else(|| catalog.map(|e| e.4.to_string()))
            .unwrap_or_default(),
        auth: connector
            .map(|c| c["auth"].as_str().unwrap_or("oauth").to_string())
            .or_else(|| catalog.map(|e| e.5.to_string()))
            .unwrap_or_else(|| "oauth".to_string()),
        token: String::new(),
        had_token: connector.is_some_and(|c| c["auth"] == "token"),
        project_only: match connector {
            Some(c) => c["projectId"].is_string(),
            None => current.is_some(),
        },
        project: project.or(current).or(first).unwrap_or_default(),
        project_ref: reference,
        read_only: connector.is_none() || read_only,
        agents: connector.map(|c| strings(&c["agents"])).unwrap_or_else(|| {
            if installed.is_empty() {
                vec!["claude".to_string()]
            } else {
                installed
            }
        }),
        enabled: connector.is_none_or(|c| c["enabled"].as_bool().unwrap_or(true)),
        was_connected: connected,
        error: None,
        closing: false,
        generation: now.generation,
    };
    now.menu = None;
    now.autofocus = (kind == Kind::Rename).then(|| (LABEL, dialog.label.clone()));
    now.dialog = Some(dialog);
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect()
}

/// A fresh connector id: letters, digits and dashes, as the host takes them.
fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let count = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("c-{nanos:x}-{count:x}")
}

// The page.

type Stat = HashMap<String, (String, Option<String>)>;

#[allow(clippy::too_many_arguments)]
pub fn view<V: 'static>(
    settings: &Value,
    data: &HashMap<&'static str, Value>,
    editing: Option<(&str, &str)>,
    save: Save<V>,
    edit: Edit<V>,
    run: Run<V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let list = settings["connectors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let states: Stat = data
        .get("connectors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            (
                text_of(&item["id"]),
                (
                    text_of(&item["state"]),
                    item["account"].as_str().map(str::to_string),
                ),
            )
        })
        .collect();
    let installed: Vec<String> = data
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|agent| agent["installed"].as_bool() == Some(true))
        .filter_map(|agent| agent["id"].as_str())
        .filter(|id| CONNECTOR_AGENTS.contains(id))
        .map(str::to_string)
        .collect();
    let (query, project_only, menu, current, projects, width, signing) = {
        let mut now = state();
        now.installed = installed;
        (
            now.query.clone(),
            now.project_only,
            now.menu.as_ref().map(|m| (m.id.clone(), m.closing)),
            now.current.clone(),
            now.projects.clone(),
            now.width,
            now.signing.clone(),
        )
    };
    let typing = editing
        .filter(|(key, _)| *key == SEARCH)
        .map(|(_, text)| text.to_string());
    let shown_query = typing.clone().unwrap_or_else(|| query.clone());
    let needle = shown_query.trim().to_lowercase();
    let matches = |parts: &[&str]| {
        needle.is_empty()
            || parts
                .iter()
                .any(|part| part.to_lowercase().contains(&needle))
    };
    let project_name = |id: &str| {
        projects
            .iter()
            .find(|(known, _)| known == id)
            .map_or_else(|| id.chars().take(8).collect(), |(_, name)| name.clone())
    };
    let cols = if width < 560. { 1 } else { 2 };

    // The search bar.
    let search = {
        let edit_text = shown_query.clone();
        div()
            .id(SEARCH)
            .flex_1()
            .min_w_0()
            .h(px(32.))
            .px(px(11.))
            .rounded(px(9.))
            .border_1()
            .border_color(content(if typing.is_some() { 0.18 } else { 0.09 }))
            .bg(content(0.03))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(13.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| edit(this, SEARCH, edit_text.clone(), cx)))
            .child(svg().path("search").size(px(14.)).text_color(content(0.5)))
            .child(match typing {
                Some(text) => div()
                    .flex()
                    .items_center()
                    .gap(px(1.))
                    .text_color(content(0.95))
                    .child(text)
                    .child(div().w(px(1.5)).h(px(14.)).bg(accent()))
                    .into_any_element(),
                None if query.is_empty() => div()
                    .text_color(content(0.4))
                    .child(t("Buscar conectores"))
                    .into_any_element(),
                None => div()
                    .text_color(content(0.95))
                    .child(query.clone())
                    .into_any_element(),
            })
    };
    let add_custom = div()
        .id("cn-add")
        .h(px(32.))
        .px(px(12.))
        .rounded(px(8.))
        .bg(accent())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(6.))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(on_accent())
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .on_click(cx.listener(|_, _, _, cx| {
            open_dialog(Kind::Connect, None, None, false);
            cx.notify();
        }))
        .child(svg().path("plus").size(px(14.)).text_color(on_accent()))
        .child(t("Adicionar personalizado"));
    let mut page = div().w_full().flex().flex_col().mb(px(28.)).child(
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(search)
            .child(add_custom),
    );

    // The scope filter, when a project is open.
    if let Some(name) = current.as_deref().map(project_name) {
        let pill = |id: &'static str, label: String, on: bool, only: bool, cx: &mut Context<V>| {
            div()
                .id(id)
                .h(px(26.))
                .px(px(10.))
                .rounded_full()
                .border_1()
                .border_color(content(if on { 0.14 } else { 0.09 }))
                .when(on, |d| d.bg(content(0.08)))
                .flex()
                .items_center()
                .text_size(px(12.))
                .text_color(content(if on { 1. } else { 0.6 }))
                .cursor_pointer()
                .hover(|s| s.text_color(content(1.)))
                .on_click(cx.listener(move |_, _, _, cx| {
                    state().project_only = only;
                    cx.notify();
                }))
                .child(label)
        };
        page = page.child(
            div()
                .flex()
                .gap(px(6.))
                .mt(px(10.))
                .child(pill(
                    "cn-all",
                    t("Todos").to_string(),
                    !project_only,
                    false,
                    cx,
                ))
                .child(pill(
                    "cn-project",
                    tf("Neste projeto · {}", &[&name]),
                    project_only,
                    true,
                    cx,
                )),
        );
    }

    // The connected ones.
    let connected: Vec<Conn> = list
        .iter()
        .map(|raw| {
            let id = text_of(&raw["id"]);
            let (state, account) = states.get(&id).cloned().unwrap_or_default();
            Conn {
                name: text_of(&raw["name"]),
                label: text_of(&raw["label"]),
                catalog: raw["catalogId"].as_str().map(str::to_string),
                url: text_of(&raw["url"]),
                project: raw["projectId"].as_str().map(str::to_string),
                enabled: raw["enabled"].as_bool().unwrap_or(true),
                auth: raw["auth"].as_str().unwrap_or("oauth").to_string(),
                state,
                account,
                id,
                raw: raw.clone(),
            }
        })
        .collect();
    let shown: Vec<&Conn> = connected
        .iter()
        .filter(|c| {
            if project_only
                && c.project
                    .as_ref()
                    .is_some_and(|p| Some(p) != current.as_ref())
            {
                return false;
            }
            let desc = c
                .catalog
                .as_deref()
                .and_then(entry)
                .map_or_else(|| c.url.clone(), |e| t(e.3).to_string());
            matches(&[
                &c.name,
                &c.label,
                c.account.as_deref().unwrap_or_default(),
                &desc,
            ])
        })
        .collect();
    let mut any = !shown.is_empty();
    if !shown.is_empty() {
        let items: Vec<AnyElement> = shown
            .iter()
            .map(|c| {
                connected_item(
                    c,
                    &project_name,
                    menu.as_ref(),
                    &signing,
                    &list,
                    save,
                    run,
                    cx,
                )
            })
            .collect();
        page = page.child(section(t("Conectados"), items, cols));
    }

    // The catalog.
    for (category, title) in CONNECTOR_GROUPS {
        let items: Vec<AnyElement> = CATALOG
            .iter()
            .filter(|e| e.2 == *category && matches(&[e.1, t(e.3)]))
            .map(|e| {
                let (id, name, _, desc, _, _) = *e;
                let entry_id: &'static str = id;
                let button = icon_button(format!("cn-add-{id}"), "plus", cx, move |_, cx| {
                    open_dialog(Kind::Connect, None, Some(entry_id), false);
                    cx.notify();
                });
                item(
                    format!("cat-{id}"),
                    logo(Some(id), name, 28.),
                    vec![
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(content(0.95))
                            .truncate()
                            .child(name)
                            .into_any_element(),
                    ],
                    t(desc).to_string(),
                    false,
                    vec![button],
                )
            })
            .collect();
        if !items.is_empty() {
            any = true;
            page = page.child(section(t(title), items, cols));
        }
    }
    if !needle.is_empty() && !any {
        page = page.child(foot(t("Nenhum conector encontrado.")));
    }
    page.child(foot(t(
        "Conversas novas recebem os conectores daqui. Os conectores de um projeto só chegam às conversas dele. Grok, Antigravity, Droid e Pi ainda não aceitam conectores.",
    )))
    .into_any_element()
}

fn foot(text: &str) -> AnyElement {
    div()
        .mt(px(18.))
        .text_size(px(12.))
        .text_color(content(0.5))
        .child(text.to_string())
        .into_any_element()
}

/// A heading and its items in `cols` columns.
fn section(title: &str, items: Vec<AnyElement>, cols: usize) -> AnyElement {
    let mut rows = Vec::new();
    let mut items = items.into_iter();
    loop {
        let mut row = div().flex().gap(px(24.));
        let mut filled = 0;
        for _ in 0..cols {
            match items.next() {
                Some(item) => {
                    row = row.child(div().flex_1().min_w_0().child(item));
                    filled += 1;
                }
                None => row = row.child(div().flex_1().min_w_0()),
            }
        }
        if filled == 0 {
            break;
        }
        rows.push(row);
    }
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .mt(px(22.))
                .mb(px(6.))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(0.5))
                .child(title.to_string()),
        )
        .child(div().flex().flex_col().gap(px(2.)).children(rows))
        .into_any_element()
}

/// A brand's mark in a rounded square: its Simple Icons path in its color (near-black ones follow
/// the text color), the PNG from its site, or its first letter.
fn logo(catalog_id: Option<&str>, name: &str, size: f32) -> AnyElement {
    let frame = div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(px(8.))
        .bg(content(0.05))
        .flex()
        .items_center()
        .justify_center();
    let Some(id) = catalog_id else {
        return letter(frame, name);
    };
    if IMAGES.contains(&id) {
        return frame
            .child(
                gpui::img(format!("connectors/{id}.png"))
                    .size(px(size - 8.))
                    .rounded(px(5.)),
            )
            .into_any_element();
    }
    match MARKS.iter().find(|(known, _, _)| *known == id) {
        Some((_, _, color)) => {
            let (r, g, b) = ((color >> 16) & 255, (color >> 8) & 255, color & 255);
            let light = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
            let ink = if light < 60. {
                content(0.9)
            } else {
                rgb(*color).into()
            };
            frame
                .child(
                    svg()
                        .path(format!("connectors/{id}.svg"))
                        .size(px(size - 10.))
                        .text_color(ink),
                )
                .into_any_element()
        }
        None => letter(frame, name),
    }
}

fn letter(frame: gpui::Div, name: &str) -> AnyElement {
    let first = name
        .trim()
        .chars()
        .next()
        .map_or("·".to_string(), |c| c.to_uppercase().collect::<String>());
    frame
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(content(0.6))
        .child(first)
        .into_any_element()
}

/// One row: the mark, a name line, a description line and the actions at the end.
fn item(
    id: String,
    mark: AnyElement,
    name_line: Vec<AnyElement>,
    description: String,
    off: bool,
    actions: Vec<AnyElement>,
) -> AnyElement {
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .px(px(8.))
        .py(px(8.))
        .rounded(px(10.))
        .flex()
        .items_center()
        .gap(px(11.))
        .hover(|s| s.bg(content(0.05)))
        .child(div().when(off, |d| d.opacity(0.5)).child(mark))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .when(off, |d| d.opacity(0.5))
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .children(name_line),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .truncate()
                        .child(description),
                ),
        )
        .children(actions)
        .into_any_element()
}

fn tag(text: String, project: bool) -> AnyElement {
    div()
        .min_w_0()
        .truncate()
        .px(px(7.))
        .py(px(1.))
        .rounded_full()
        .bg(if project {
            crate::theme::accent_wash(0.16)
        } else {
            content(0.07)
        })
        .text_size(px(10.5))
        .text_color(if project { accent() } else { content(0.7) })
        .child(text)
        .into_any_element()
}

fn signed_in_badge() -> AnyElement {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(5.))
        .pl(px(6.))
        .pr(px(8.))
        .py(px(1.))
        .rounded_full()
        .bg(Hsla {
            a: 0.14,
            ..success()
        })
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(success())
        .child(div().size(px(6.)).rounded_full().bg(success()))
        .child(t("Conectado"))
        .into_any_element()
}

/// A square icon button at the end of a row.
fn icon_button<V: 'static>(
    id: String,
    icon: &'static str,
    cx: &mut Context<V>,
    on_click: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .size(px(28.))
        .flex_shrink_0()
        .rounded(px(7.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(content(0.08)))
        .on_click(cx.listener(move |this, _, _, cx| on_click(this, cx)))
        .child(svg().path(icon).size(px(15.)).text_color(content(0.55)))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn connected_item<V: 'static>(
    c: &Conn,
    project_name: &dyn Fn(&str) -> String,
    menu: Option<&(String, bool)>,
    signing: &[String],
    list: &[Value],
    save: Save<V>,
    run: Run<V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let catalog = c.catalog.as_deref().and_then(entry);
    let (reference, read_only) = if c.catalog.as_deref() == Some("supabase") {
        supabase_details(&c.url)
    } else {
        Default::default()
    };
    let detail = if !reference.is_empty() {
        if read_only {
            format!("{reference} · {}", t("Somente leitura").to_lowercase())
        } else {
            reference
        }
    } else if let Some(e) = catalog {
        t(e.3).to_string()
    } else {
        host_of(&c.url)
    };
    // The line under the name: the connector's own name when a label replaces it, the account it
    // is signed in as, then what it is.
    let mut parts = Vec::new();
    if !c.label.is_empty() {
        parts.push(c.name.clone());
    }
    if let Some(account) = c.account.as_ref().filter(|_| c.state == "connected") {
        parts.push(account.clone());
    }
    parts.push(detail);
    let description = parts.join(" · ");

    let needs_sign_in = c.auth != "none" && matches!(c.state.as_str(), "signedOut" | "expired");
    let waiting = signing.contains(&c.id);
    let mut name_line = vec![
        div()
            .flex_shrink_0()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(content(0.95))
            .child(c.title().to_string())
            .into_any_element(),
        tag(
            c.project
                .as_deref()
                .map_or_else(|| t("Todos os projetos").to_string(), project_name),
            c.project.is_some(),
        ),
    ];
    if !c.enabled {
        name_line.push(tag(t("Desligado").to_string(), false));
    }
    if c.auth != "none" && c.state == "connected" {
        name_line.push(signed_in_badge());
    }

    let mut actions = Vec::new();
    if needs_sign_in || waiting {
        let label = if waiting {
            t("Aguardando o navegador…")
        } else if c.state == "expired" {
            t("Entrar de novo")
        } else {
            t("Entrar")
        };
        let button = div()
            .id(format!("cn-signin-{}", c.id))
            .h(px(28.))
            .px(px(11.))
            .flex_shrink_0()
            .rounded(px(8.))
            .border_1()
            .border_color(content(0.14))
            .flex()
            .items_center()
            .text_size(px(12.5))
            .text_color(content(0.85))
            .child(label);
        let id = c.id.clone();
        let raw = c.raw.clone();
        let oauth = c.auth == "oauth";
        actions.push(if waiting {
            button.opacity(0.6).into_any_element()
        } else {
            button
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if oauth {
                        sign_in(this, &id, run, cx);
                    } else {
                        open_dialog(
                            Kind::Edit,
                            Some(&raw),
                            raw["catalogId"]
                                .as_str()
                                .and_then(|i| entry(i).map(|e| e.0)),
                            false,
                        );
                    }
                    cx.notify();
                }))
                .into_any_element()
        });
    }
    actions.push(more_menu(c, menu, list, save, run, cx));
    let mark = logo(c.catalog.as_deref(), &c.name, 28.);
    item(
        format!("con-{}", c.id),
        mark,
        name_line,
        description,
        !c.enabled,
        actions,
    )
}

fn sign_in<V: 'static>(view: &mut V, id: &str, run: Run<V>, cx: &mut Context<V>) {
    let _ = view;
    state().signing.push(id.to_string());
    let id = id.to_string();
    after(cx, 0, move |this, cx| {
        run(
            this,
            "connector_action",
            json!({"action": {"type": "signIn", "id": id}}),
            cx,
        )
    });
}

/// The "⋯" button and, while open, its menu.
fn more_menu<V: 'static>(
    c: &Conn,
    menu: Option<&(String, bool)>,
    list: &[Value],
    save: Save<V>,
    run: Run<V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let id = c.id.clone();
    let trigger = {
        let id = id.clone();
        div()
            .id(format!("cn-more-{id}"))
            .size(px(28.))
            .flex_shrink_0()
            .rounded(px(7.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(content(0.08)))
            .on_click(cx.listener(move |_, _, _, cx| {
                let mut now = state();
                let reopened = now
                    .menu_closed
                    .is_some_and(|at| at.elapsed() < Duration::from_millis(250));
                let here = now.menu.as_ref().is_some_and(|m| m.id == id && !m.closing);
                if here {
                    drop(now);
                    close_menu(cx);
                } else if !reopened {
                    now.menu = Some(Menu {
                        id: id.clone(),
                        closing: false,
                    });
                    drop(now);
                }
                cx.notify();
            }))
            .child(
                svg()
                    .path("connector-ui/ellipsis")
                    .size(px(15.))
                    .text_color(content(0.55)),
            )
    };
    let mut holder = div().relative().flex_shrink_0().child(trigger);
    let Some((open, closing)) = menu.filter(|(open, _)| *open == c.id) else {
        return holder.into_any_element();
    };
    let _ = open;
    let closing = *closing;

    let connected = c.state == "connected";
    type Action<V> = Box<dyn Fn(&mut V, &mut Context<V>)>;
    let mut entries: Vec<(&'static str, String, Action<V>)> = Vec::new();
    if c.auth == "oauth" {
        let id = c.id.clone();
        entries.push((
            "connector-ui/log-in",
            t(if connected {
                "Entrar de novo"
            } else {
                "Entrar"
            })
            .to_string(),
            Box::new(move |this, cx| sign_in(this, &id, run, cx)),
        ));
    }
    if c.auth != "none" && connected {
        let id = c.id.clone();
        entries.push((
            "connector-ui/log-out",
            t("Sair").to_string(),
            Box::new(move |this, cx| {
                run(
                    this,
                    "connector_action",
                    json!({"action": {"type": "signOut", "id": id}}),
                    cx,
                )
            }),
        ));
    }
    let entry_id = c.catalog.as_deref().and_then(|i| entry(i).map(|e| e.0));
    let raw = c.raw.clone();
    entries.push((
        "pencil-line",
        t("Renomear conta").to_string(),
        Box::new(move |_, _| open_dialog(Kind::Rename, Some(&raw), None, connected)),
    ));
    let raw = c.raw.clone();
    entries.push((
        "pencil",
        t("Editar").to_string(),
        Box::new(move |_, _| open_dialog(Kind::Edit, Some(&raw), entry_id, connected)),
    ));
    let mut toggled = list.to_vec();
    if let Some(item) = toggled.iter_mut().find(|item| item["id"] == c.id.as_str()) {
        item["enabled"] = json!(!c.enabled);
    }
    let toggled = Value::Array(toggled);
    entries.push((
        "connector-ui/power",
        t(if c.enabled { "Desligar" } else { "Ligar" }).to_string(),
        Box::new(move |this, cx| save(this, "connectors", toggled.clone(), cx)),
    ));
    let without: Vec<Value> = list
        .iter()
        .filter(|item| item["id"] != c.id.as_str())
        .cloned()
        .collect();
    let remove_args = json!({
        "label": t("Remover"),
        "title": tf("Remover {}?", &[&c.title()]),
        "message": t("Os agentes deixam de recebê-lo e o login dele é esquecido."),
        "then": {"kind": "run", "cmd": "native_connector_remove", "args": {"id": c.id, "list": without}},
    });
    let rows: Vec<AnyElement> = entries
        .into_iter()
        .enumerate()
        .map(|(index, (icon, label, action))| {
            menu_row(
                format!("cn-menu-{}-{index}", c.id),
                icon,
                label,
                false,
                cx.listener(move |this, _, _, cx| {
                    close_menu(cx);
                    action(this, cx);
                    cx.notify();
                }),
            )
        })
        .collect();
    let remove = menu_row(
        format!("cn-menu-{}-remove", c.id),
        "trash",
        t("Remover").to_string(),
        true,
        cx.listener(move |this, _, _, cx| {
            close_menu(cx);
            run(this, "native_confirm", remove_args.clone(), cx);
            cx.notify();
        }),
    );
    let panel = div()
        .id(format!("cn-menu-{}", c.id))
        .absolute()
        .top(px(32.))
        .right(px(0.))
        .w(px(190.))
        .p(px(4.))
        .rounded(px(10.))
        .border_1()
        .border_color(content(0.12))
        .child(crate::theme::floating_surface(10.))
        .shadow_md()
        .occlude()
        .on_mouse_down_out(cx.listener(|_, _, _, cx| close_menu(cx)))
        .children(rows)
        .child(div().h(px(1.)).my(px(4.)).bg(content(0.08)))
        .child(remove)
        .with_animation(
            format!("cn-menu-{}-{closing}", c.id),
            Animation::new(Duration::from_secs_f32(MENU_TIME))
                .with_easing(|t| eased(t, Curve::Expo)),
            move |el, t| {
                let p = if closing { 1. - t } else { t };
                el.opacity(p).top(px(32. - (1. - p) * 4.))
            },
        );
    holder = holder.child(gpui::deferred(panel).with_priority(10));
    holder.into_any_element()
}

fn menu_row(
    id: String,
    icon: &'static str,
    label: String,
    destructive: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let ink = if destructive { danger() } else { content(0.9) };
    div()
        .id(id)
        .h(px(28.))
        .px(px(8.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.5))
        .text_color(ink)
        .cursor_pointer()
        .hover(|s| s.bg(content(0.07)))
        .on_click(on_click)
        .child(svg().path(icon).size(px(14.)).text_color(if destructive {
            ink
        } else {
            content(0.6)
        }))
        .child(label)
        .into_any_element()
}

// The dialog.

/// What a control of the dialog changes.
#[derive(Clone)]
enum Change {
    Auth(&'static str),
    Where(bool),
    Project(String),
    ReadOnly(bool),
    Agent(String),
}

fn apply(change: &Change) {
    let mut now = state();
    let Some(d) = now.dialog.as_mut() else {
        return;
    };
    match change {
        Change::Auth(auth) => d.auth = auth.to_string(),
        Change::Where(only) => d.project_only = *only,
        Change::Project(id) => d.project = id.clone(),
        Change::ReadOnly(on) => d.read_only = *on,
        Change::Agent(id) => {
            if d.agents.contains(id) {
                d.agents.retain(|known| known != id);
            } else {
                d.agents.push(id.clone());
            }
        }
    }
}

/// The address the dialog would save.
fn final_url(d: &Dialog) -> String {
    if d.entry == Some("supabase") {
        let base = entry("supabase").map_or("", |e| e.4);
        supabase_url(base, &d.project_ref, d.read_only)
    } else {
        d.url.clone()
    }
}

/// Why the dialog cannot be saved yet, if it cannot.
fn problem(d: &Dialog) -> Option<&'static str> {
    if d.kind == Kind::Rename {
        return None;
    }
    if d.name.trim().is_empty() {
        Some("Nome")
    } else if !valid_url(&final_url(d)) {
        Some("Use um endereço https.")
    } else if d.agents.is_empty() {
        Some("Escolha pelo menos um agente.")
    } else if d.project_only && d.project.is_empty() {
        Some("Escolha um projeto.")
    } else if d.auth == "token" && d.token.is_empty() && !(d.kind == Kind::Edit && d.had_token) {
        Some("Cole o token.")
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
pub fn overlay<V: 'static>(
    page: &str,
    settings: &Value,
    editing: Option<(&str, &str)>,
    save: Save<V>,
    edit: Edit<V>,
    run: Run<V>,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    if page != "Conectores" {
        return None;
    }
    let (d, projects) = {
        let now = state();
        (now.dialog.clone()?, now.projects.clone())
    };
    let list = settings["connectors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let pending: Pending = editing.map(|(k, v)| (k.to_string(), v.to_string()));
    let blocked = problem(&d);
    let closing = d.closing;
    let catalog = d.entry.and_then(entry);

    let title = match d.kind {
        Kind::Rename => tf("Renomear {}", &[&d.name]),
        Kind::Edit => {
            if d.label.is_empty() {
                d.name.clone()
            } else {
                d.label.clone()
            }
        }
        Kind::Connect => match catalog {
            Some(e) => tf("Conectar {}", &[&e.1]),
            None => t("Adicionar conector personalizado").to_string(),
        },
    };
    let help = if d.kind == Kind::Rename {
        t("Só aparece nas listas do Sirus. Os agentes continuam usando o mesmo identificador.")
    } else if catalog.is_none() && d.kind == Kind::Connect {
        t("O endereço de qualquer servidor MCP remoto (HTTP).")
    } else {
        match d.auth.as_str() {
            "oauth" => t("O Sirus abre o login do serviço no seu navegador; você entra uma vez."),
            "token" => t("Cole um token do serviço; ele fica no Keychain, nunca nos ajustes."),
            _ => t("Este servidor não precisa de login."),
        }
    };

    let mut form = div().flex().flex_col().gap(px(16.)).mt(px(16.));
    if d.kind == Kind::Rename {
        form = form.child(input(
            LABEL,
            t("Rótulo da conta"),
            &d.label,
            &d.name,
            false,
            Some(t("Em branco usa o nome do conector.").to_string()),
            &pending,
            edit,
            cx,
        ));
    } else {
        if let Some(e) = catalog {
            form = form.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .text_size(px(12.5))
                    .text_color(content(0.7))
                    .child(logo(Some(e.0), e.1, 36.))
                    .child(t(e.3)),
            );
        } else {
            form = form.child(input(
                NAME,
                t("Nome"),
                &d.name,
                "",
                false,
                None,
                &pending,
                edit,
                cx,
            ));
            form = form.child(input(
                URL,
                t("URL"),
                &d.url,
                "https://mcp.example.com/mcp",
                false,
                None,
                &pending,
                edit,
                cx,
            ));
            form = form.child(field_block(
                t("Login"),
                segmented(
                    "cd-auth",
                    vec![
                        (
                            t("Login do serviço"),
                            d.auth == "oauth",
                            Change::Auth("oauth"),
                        ),
                        (t("Token"), d.auth == "token", Change::Auth("token")),
                        (t("Nenhum"), d.auth == "none", Change::Auth("none")),
                    ],
                    &pending,
                    edit,
                    cx,
                ),
                None,
            ));
        }
        form = form.child(input(
            LABEL,
            t("Rótulo da conta"),
            &d.label,
            t("Opcional"),
            false,
            Some(
                t("Distingue duas contas do mesmo serviço, como “Conta Gabriel” e “Conta Moreira”. Os agentes não veem o rótulo.")
                    .to_string(),
            ),
            &pending,
            edit,
            cx,
        ));
        if d.auth == "token" {
            form = form.child(input(
                TOKEN,
                t("Token"),
                &d.token,
                "",
                true,
                (d.kind == Kind::Edit && d.had_token)
                    .then(|| t("Deixe vazio para manter o token salvo.").to_string()),
                &pending,
                edit,
                cx,
            ));
        }
        let mut place = div().flex().flex_col().gap(px(8.)).child(field_block(
            t("Onde vale"),
            segmented(
                "cd-where",
                vec![
                    (t("Só em um projeto"), d.project_only, Change::Where(true)),
                    (
                        t("Todos os projetos"),
                        !d.project_only,
                        Change::Where(false),
                    ),
                ],
                &pending,
                edit,
                cx,
            ),
            None,
        ));
        if d.project_only {
            place = place.child(field_block(
                t("Projeto"),
                segmented(
                    "cd-project",
                    projects
                        .iter()
                        .map(|(id, name)| {
                            (
                                Box::leak(name.clone().into_boxed_str()) as &'static str,
                                *id == d.project,
                                Change::Project(id.clone()),
                            )
                        })
                        .collect(),
                    &pending,
                    edit,
                    cx,
                ),
                None,
            ));
        }
        form = form.child(place);
        if d.entry == Some("supabase") {
            form = form.child(input(
                REF,
                t("Projeto do Supabase (project ref)"),
                &d.project_ref,
                "abcdefghijklmnopqrst",
                false,
                Some(t("Em Supabase → Project Settings → General. Deixe vazio para o agente ver todos os projetos da conta.").to_string()),
                &pending,
                edit,
                cx,
            ));
            let read_pending = pending.clone();
            let on = d.read_only;
            form = form.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(content(0.95))
                                    .child(t("Somente leitura")),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(content(0.5))
                                    .child(t("O agente consulta, mas não altera o banco.")),
                            ),
                    )
                    .child(
                        div()
                            .id("cd-readonly")
                            .w(px(40.))
                            .h(px(22.))
                            .p(px(2.))
                            .flex_shrink_0()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .bg(if on { accent() } else { content(0.16) })
                            .when(on, |d| d.justify_end())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                flush(&read_pending);
                                park(this, &read_pending, edit, cx);
                                apply(&Change::ReadOnly(!on));
                                cx.notify();
                            }))
                            .child(div().size(px(18.)).rounded_full().bg(if on {
                                on_accent()
                            } else {
                                content(0.75)
                            })),
                    ),
            );
        }
        let boxes: Vec<AnyElement> = CONNECTOR_AGENTS
            .iter()
            .map(|id| {
                let on = d.agents.iter().any(|known| known == id);
                let pending = pending.clone();
                let agent = id.to_string();
                div()
                    .id(format!("cd-agent-{id}"))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(13.))
                    .text_color(content(0.9))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        flush(&pending);
                        park(this, &pending, edit, cx);
                        apply(&Change::Agent(agent.clone()));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(16.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if on { accent() } else { content(0.25) })
                            .when(on, |d| d.bg(accent()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(on, |d| {
                                d.child(svg().path("check").size(px(11.)).text_color(on_accent()))
                            }),
                    )
                    .child(
                        svg()
                            .path(agent_mark(id))
                            .size(px(14.))
                            .text_color(content(0.8)),
                    )
                    .child(agent_name(id))
                    .into_any_element()
            })
            .collect();
        form = form.child(field_block(
            t("Agentes"),
            div()
                .flex()
                .flex_wrap()
                .gap_x(px(20.))
                .gap_y(px(8.))
                .children(boxes)
                .into_any_element(),
            None,
        ));
    }
    if let Some(error) = &d.error {
        form = form.child(
            div()
                .text_size(px(12.))
                .text_color(danger())
                .child(error.clone()),
        );
    }

    let cancel_pending = pending.clone();
    let submit_pending = pending.clone();
    let list_for_submit = list.clone();
    let can_save = blocked.is_none();
    let primary = div()
        .id("cd-save")
        .h(px(32.))
        .px(px(14.))
        .rounded(px(8.))
        .bg(accent())
        .flex()
        .items_center()
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(on_accent())
        .child(match d.kind {
            Kind::Connect => t("Conectar"),
            _ => t("Salvar"),
        });
    let primary = if can_save {
        primary
            .cursor_pointer()
            .hover(|s| s.opacity(0.9))
            .on_click(cx.listener(move |this, _, _, cx| {
                submit(this, &list_for_submit, save, run, &submit_pending, cx);
            }))
            .into_any_element()
    } else {
        primary.opacity(0.45).into_any_element()
    };
    let buttons = div()
        .flex()
        .justify_end()
        .gap(px(8.))
        .mt(px(20.))
        .child(
            div()
                .id("cd-cancel")
                .h(px(32.))
                .px(px(12.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .text_size(px(13.))
                .text_color(content(0.75))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    park(this, &cancel_pending, edit, cx);
                    close_dialog(cx);
                }))
                .child(t("Cancelar")),
        )
        .child(primary);

    let card = div()
        .id("cd-card")
        .relative()
        .w(px(500.))
        .max_w_full()
        .max_h(gpui::relative(0.92))
        .overflow_y_scroll()
        .p(px(20.))
        .rounded(px(14.))
        .border_1()
        .border_color(content(0.12))
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
        .child(buttons)
        .with_animation(
            format!("cd-card-{}-{closing}", d.generation),
            Animation::new(Duration::from_secs_f32(DIALOG_TIME))
                .with_easing(|t| eased(t, Curve::Expo)),
            move |el, t| {
                let p = if closing { 1. - t } else { t };
                el.opacity(p).top(px((1. - p) * 6.))
            },
        );
    Some(
        div()
            .id("cd-backdrop")
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
                format!("cd-backdrop-{}-{closing}", d.generation),
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

/// Keeps what is being typed when another control is used, and parks typing afterwards.
fn park<V: 'static>(view: &mut V, pending: &Pending, edit: Edit<V>, cx: &mut Context<V>) {
    flush(pending);
    if pending.is_some() {
        edit(view, IDLE, String::new(), cx);
    }
}

fn field_block(label: &str, control: AnyElement, help: Option<String>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(content(0.9))
                .child(label.to_string()),
        )
        .child(control)
        .when_some(help, |d, help| {
            d.child(
                div()
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(content(0.5))
                    .child(help),
            )
        })
        .into_any_element()
}

/// A labeled text field. Clicking it starts the shell's text editing for `key`; the text is kept
/// in the dialog on Enter, or when another control is used.
#[allow(clippy::too_many_arguments)]
fn input<V: 'static>(
    key: &'static str,
    label: &str,
    value: &str,
    placeholder: &str,
    secret: bool,
    help: Option<String>,
    pending: &Pending,
    edit: Edit<V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let typed = pending
        .as_ref()
        .filter(|(known, _)| known == key)
        .map(|(_, text)| text.clone());
    let editing = typed.is_some();
    let start = typed.clone().unwrap_or_else(|| value.to_string());
    let others = pending.clone().filter(|(known, _)| known != key);
    let show = |text: &str| {
        if secret {
            "•".repeat(text.chars().count())
        } else {
            text.to_string()
        }
    };
    let body = match typed {
        Some(text) => div()
            .flex()
            .items_center()
            .gap(px(1.))
            .text_color(content(0.95))
            .child(show(&text))
            .child(div().w(px(1.5)).h(px(14.)).bg(accent()))
            .into_any_element(),
        None if value.is_empty() => div()
            .text_color(content(0.35))
            .child(placeholder.to_string())
            .into_any_element(),
        None => div()
            .text_color(content(0.95))
            .child(show(value))
            .into_any_element(),
    };
    field_block(
        label,
        div()
            .id(key)
            .h(px(32.))
            .px(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(if editing { accent() } else { content(0.14) })
            .flex()
            .items_center()
            .text_size(px(13.))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                flush(&others);
                edit(this, key, start.clone(), cx);
            }))
            .child(body)
            .into_any_element(),
        help,
    )
}

/// Options in a row (wrapping), one chosen.
fn segmented<V: 'static>(
    id: &'static str,
    options: Vec<(&'static str, bool, Change)>,
    pending: &Pending,
    edit: Edit<V>,
    cx: &mut Context<V>,
) -> AnyElement {
    let items: Vec<AnyElement> = options
        .into_iter()
        .enumerate()
        .map(|(index, (label, chosen, change))| {
            let pending = pending.clone();
            div()
                .id((id, index))
                .h(px(26.))
                .px(px(11.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .text_size(px(12.5))
                .text_color(content(if chosen { 1. } else { 0.55 }))
                .when(chosen, |d| d.bg(content(0.1)))
                .when(!chosen, |d| d.hover(|s| s.text_color(content(0.9))))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    park(this, &pending, edit, cx);
                    apply(&change);
                    cx.notify();
                }))
                .child(label)
                .into_any_element()
        })
        .collect();
    div()
        .flex()
        .flex_wrap()
        .self_start()
        .p(px(2.))
        .rounded(px(8.))
        .border_1()
        .border_color(content(0.1))
        .children(items)
        .into_any_element()
}

/// Saves the dialog: the list in the settings, then the sign-in or token it needs.
fn submit<V: 'static>(
    view: &mut V,
    list: &[Value],
    save: Save<V>,
    run: Run<V>,
    pending: &Pending,
    cx: &mut Context<V>,
) {
    flush(pending);
    let Some(d) = state().dialog.clone().filter(|d| !d.closing) else {
        return;
    };
    if problem(&d).is_some() {
        return;
    }
    let mut next = list.to_vec();
    let position = next.iter().position(|item| item["id"] == d.id.as_str());
    let mut connector = position.map_or_else(|| json!({}), |index| next[index].clone());
    let label = d.label.trim();
    if d.kind == Kind::Rename {
        set_label(&mut connector, label);
    } else {
        connector["id"] = json!(d.id);
        if let Some(entry) = d.entry {
            connector["catalogId"] = json!(entry);
        }
        connector["name"] = json!(d.name.trim());
        set_label(&mut connector, label);
        connector["url"] = json!(final_url(&d));
        connector["auth"] = json!(d.auth);
        connector["agents"] = json!(d.agents);
        connector["enabled"] = json!(d.enabled);
        if d.project_only && !d.project.is_empty() {
            connector["projectId"] = json!(d.project);
        } else if let Some(object) = connector.as_object_mut() {
            object.remove("projectId");
        }
    }
    match position {
        Some(index) => next[index] = connector,
        None => next.push(connector),
    }
    save(view, "connectors", Value::Array(next), cx);
    close_dialog(cx);
    let id = d.id.clone();
    if d.kind == Kind::Rename {
        return;
    }
    // The settings are saved first: the sign-in reads the connector from them.
    if d.auth == "token" && !d.token.is_empty() {
        let token = d.token.clone();
        after(cx, 400, move |this, cx| {
            run(
                this,
                "connector_action",
                json!({"action": {"type": "setToken", "id": id, "token": token}}),
                cx,
            )
        });
    } else if d.auth == "oauth" && (position.is_none() || !d.was_connected) {
        state().signing.push(id.clone());
        after(cx, 400, move |this, cx| {
            run(
                this,
                "connector_action",
                json!({"action": {"type": "signIn", "id": id}}),
                cx,
            )
        });
    } else {
        after(cx, 400, move |this, cx| {
            run(
                this,
                "connector_action",
                json!({"action": {"type": "status"}}),
                cx,
            )
        });
    }
}

fn set_label(connector: &mut Value, label: &str) {
    if label.is_empty() {
        if let Some(object) = connector.as_object_mut() {
            object.remove("label");
        }
    } else {
        connector["label"] = json!(label);
    }
}

/// Escape closes the menu or dialog; Enter saves the dialog. True when the key was used.
pub fn key<V: 'static>(
    view: &mut V,
    key: &str,
    list: &Value,
    save: Save<V>,
    run: Run<V>,
    cx: &mut Context<V>,
) -> bool {
    let (dialog, menu) = {
        let now = state();
        (
            now.dialog.as_ref().is_some_and(|d| !d.closing),
            now.menu.as_ref().is_some_and(|m| !m.closing),
        )
    };
    match key {
        "escape" if dialog => close_dialog(cx),
        "escape" if menu => close_menu(cx),
        "enter" if dialog => {
            let list = list.as_array().cloned().unwrap_or_default();
            submit(view, &list, save, run, &None, cx);
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supabase_addresses_round_trip() {
        let base = "https://mcp.supabase.com/mcp";
        let url = supabase_url(base, "abcdefghij", true);
        assert_eq!(
            url,
            "https://mcp.supabase.com/mcp?project_ref=abcdefghij&read_only=true"
        );
        assert_eq!(supabase_details(&url), ("abcdefghij".to_string(), true));
        assert_eq!(supabase_url(&url, "", false), base);
        assert_eq!(supabase_details(base), (String::new(), false));
    }

    #[test]
    fn only_plain_https_addresses_are_valid() {
        assert!(valid_url("https://mcp.example.com/mcp"));
        assert!(!valid_url("http://mcp.example.com/mcp"));
        assert!(!valid_url("https://me:secret@example.com/mcp"));
        assert!(!valid_url("https:///mcp"));
    }

    #[test]
    fn every_catalog_entry_has_a_mark_and_the_assets_load() {
        for (id, name, ..) in CATALOG {
            let own = MARKS.iter().any(|(known, _, _)| known == id);
            assert!(own || IMAGES.contains(id), "{name} has no mark");
            let path = if own {
                format!("connectors/{id}.svg")
            } else {
                format!("connectors/{id}.png")
            };
            assert!(asset(&path).is_some(), "{path} does not load");
        }
        assert!(asset("connector-ui/ellipsis").is_some());
        assert!(asset("connector-ui/nope").is_none());
    }

    #[test]
    fn new_ids_are_valid_for_the_host() {
        let (a, b) = (new_id(), new_id());
        assert_ne!(a, b);
        assert!(a.len() <= 64 && a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn a_label_is_set_and_cleared_on_the_saved_connector() {
        let mut connector = json!({"id": "a", "name": "Supabase"});
        set_label(&mut connector, "Conta Gabriel");
        assert_eq!(connector["label"], "Conta Gabriel");
        assert_eq!(connector["name"], "Supabase");
        set_label(&mut connector, "");
        assert!(connector.get("label").is_none());
    }
}
