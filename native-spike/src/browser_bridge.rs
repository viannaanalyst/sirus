//! Agent control of the browser, and the approval cards that come with it (ADR-189).
//!
//! `sirus-host` has no window, so when an agent calls a `browser_*` tool the host validates it
//! and sends it here as a `browser_tool_request` event (only to this app, which registered with
//! `register_browser_client` on connect). The request runs on the conversation's `Browser`
//! (panel_browser.rs) and the answer goes back with the `browser_tool_result` command. The host
//! shapes results, so the refs and shapes are Sirus's. A paired headless host (a Linux server)
//! sends its requests the same way while it is the active computer (ADR-201); the answer goes
//! back on the connection the request came from.
//!
//! Also here: the host's login prompts (`login-vault-state`: "save this login?" offers for the
//! panel, "let the agent sign in?" approvals) and computer-use requests (`computer-state`),
//! which show as cards above the composer, as in `AgentRequests.tsx`.
use crate::backend::Backend;
use crate::browser_tab;
use crate::i18n::{t, tf};
use crate::panel_browser::{self, Step};
use crate::theme;
use base64::Engine as _;
use gpui::{AnyElement, App, AsyncApp, FutureExt as _, div, prelude::*, px};
use serde_json::{Value, json};
use std::{cell::RefCell, time::Duration};

thread_local! {
    static BACKEND: RefCell<Option<Backend>> = const { RefCell::new(None) };
    static LOGINS: RefCell<Value> = const { RefCell::new(Value::Null) };
    static COMPUTER: RefCell<Value> = const { RefCell::new(Value::Null) };
}

/// How long a tool waits for a page that is still loading (a slow page is then read as it is).
const LOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Tools that start a navigation: they answer once it has finished, as the page then stands.
const NAVIGATING: [&str; 5] = [
    "browser_open",
    "browser_navigate",
    "browser_back",
    "browser_forward",
    "browser_reload",
];

/// Tools that read or act on the page: a page script run while a navigation is in flight is
/// destroyed with the old document (it answers an error, or nothing), so they wait for it.
const ON_PAGE: [&str; 12] = [
    "browser_snapshot",
    "browser_click",
    "browser_type",
    "browser_press",
    "browser_wait_for",
    "browser_scroll",
    "browser_logs",
    "browser_hover",
    "browser_select",
    "browser_drag",
    "browser_upload",
    "browser_network",
];

/// Tools the agent's pointer shows on the page (`browser_js/pointer.js`).
const POINTED: [&str; 4] = [
    "browser_click",
    "browser_type",
    "browser_scroll",
    "browser_drag",
];

/// How long a page may take to answer a tool (waiting scripts run up to 8 s inside it).
const PAGE_TIMEOUT: Duration = Duration::from_secs(20);

pub fn start(backend: Backend, cx: &mut App) {
    BACKEND.with(|b| *b.borrow_mut() = Some(backend.clone()));
    let requests = backend.subscribe("browser_tool_request");
    let logins = backend.subscribe("login-vault-state");
    let computer = backend.subscribe("computer-state");
    crate::pointer_overlay::start(&backend, cx);
    cx.spawn(async move |cx| {
        while let Ok(request) = requests.recv().await {
            let backend = backend.clone();
            cx.spawn(async move |cx| answer(request, backend, cx).await)
                .detach();
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Ok(state) = logins.recv().await {
            LOGINS.with(|s| *s.borrow_mut() = state);
            let _ = cx.update(|cx| cx.refresh_windows());
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        while let Ok(state) = computer.recv().await {
            COMPUTER.with(|s| *s.borrow_mut() = state);
            let _ = cx.update(|cx| cx.refresh_windows());
        }
    })
    .detach();
}

/// Runs one request from the host and sends the result.
async fn answer(request: Value, backend: Backend, cx: &mut AsyncApp) {
    let id = request["id"].as_str().unwrap_or_default().to_owned();
    if id.is_empty() {
        return;
    }
    let session = request["sessionId"].as_str().unwrap_or_default().to_owned();
    let tool = request["tool"].as_str().unwrap_or_default().to_owned();
    // A page an agent drives must not walk off with the keys of someone typing in the app.
    let keys_home = on_window(cx, panel_browser::keyboard_home).unwrap_or(false);
    // Sign-ins and computer use run here only for another computer's host: this Mac's own
    // host runs them itself.
    panel_browser::agent_acting(true);
    let result = if local_command(&tool).is_some() && !from_remote(&request) {
        Err("this Mac runs that itself".to_owned())
    } else {
        run(&session, &tool, &request["args"], &backend, cx).await
    };
    // The keys go back first, then the frames judge them again; GPUI's focus was never touched.
    if keys_home {
        on_window(cx, panel_browser::reclaim_keyboard);
    }
    panel_browser::agent_acting(false);
    let _ = cx.update(|cx| cx.refresh_windows());
    if from_remote(&request) {
        backend.invoke_remote("browser_tool_result", result_args(&id, result));
    } else {
        backend.invoke("browser_tool_result", result_args(&id, result));
    }
}

/// Whether a request came from the active computer's host rather than this Mac's (backend.rs
/// notes it as `via`).
pub fn from_remote(request: &Value) -> bool {
    request["via"] == "remote"
}

/// Reads or acts on the app's window, when it has one.
fn on_window<T>(cx: &mut AsyncApp, work: impl FnOnce(&gpui::Window) -> T) -> Option<T> {
    cx.update(|app| {
        let window = app.windows().first().copied()?;
        window.update(app, |_, window, _| work(window)).ok()
    })
}

/// The arguments of `browser_tool_result`.
pub fn result_args(id: &str, result: Result<Value, String>) -> Value {
    match result {
        Ok(value) => json!({ "id": id, "ok": true, "value": value }),
        Err(error) => json!({ "id": id, "ok": false, "error": error }),
    }
}

async fn run(
    session: &str,
    tool: &str,
    args: &Value,
    backend: &Backend,
    cx: &mut AsyncApp,
) -> Result<Value, String> {
    if session.is_empty() {
        return Err("missing session".into());
    }
    // A remote host hands this Mac the sign-in and the computer use whole: the saved logins
    // and the screen are this Mac's, and the approval cards show in this window (ADR-201 addendum).
    if let Some(command) = local_command(tool) {
        return local_run(command, session, tool, args, backend, cx).await;
    }
    let mut provider = None;
    let entity = cx.update(|app| {
        let entity = panel_browser::entity_for(session, backend, app);
        // The web views hang off the window's content view, which a panel that was never
        // shown hasn't measured yet.
        if let Some(window) = app.windows().first().copied() {
            let _ = window.update(app, |_, window, app| {
                entity
                    .read(app)
                    .set_parent(panel_browser::parent_of(window));
            });
            if let Some(shell) = window.downcast::<crate::shell::Shell>() {
                provider = shell
                    .read(app)
                    .ok()
                    .and_then(|shell| shell.provider_of(session));
                if matches!(tool, "browser_open" | "browser_navigate") {
                    let _ = shell.update(app, |shell, _, cx| shell.reveal_browser(session, cx));
                }
            }
        }
        entity
    });
    // Clicks, typing, scrolls and drags show the agent's pointer on the page (ADR-198).
    let mut args = args.clone();
    if POINTED.contains(&tool) && args.is_object() {
        let config = entity.update(cx, |browser, _| {
            browser.pointer_config(tool, provider.unwrap_or_default())
        });
        args["sirusPointer"] = config;
    }
    let args = &args;
    if ON_PAGE.contains(&tool) || tool == "browser_screenshot" {
        settle(&entity, cx).await;
    }
    let step = entity.update(cx, |browser, cx| {
        let step = browser.tool(tool, args, cx);
        cx.notify();
        step
    });
    match step {
        Step::Done(result) if NAVIGATING.contains(&tool) && result.is_ok() => {
            settle(&entity, cx).await;
            Ok(entity.update(cx, |browser, _| browser.fresh_state()))
        }
        Step::Done(result) => result,
        Step::Script(answer) => {
            let raw = wait(answer, cx).await?;
            Ok(browser_tab::decode(&raw))
        }
        Step::Later(answer) => wait(answer, cx).await,
        Step::Shot(answer) => {
            let png = wait(answer, cx).await?;
            Ok(json!({ "data": base64::engine::general_purpose::STANDARD.encode(png) }))
        }
    }
}

/// How long a request waits for the person to answer an approval card, and then to act.
const APPROVED_TIMEOUT: Duration = Duration::from_secs(170);

/// The command of this Mac's own host that runs a tool a remote host forwarded, if it is one:
/// the agent's sign-in (this Mac's vault) or a computer-use call (this Mac's screen).
fn local_command(tool: &str) -> Option<&'static str> {
    match tool {
        "browser_fill_login" => Some("login_vault_agent_fill"),
        _ if tool.starts_with("computer_") => Some("computer_remote_execute"),
        _ => None,
    }
}

/// The arguments of that command: the sign-in takes what the model named; a computer call
/// takes its tool, arguments and the conversation's context, which came in `args`.
fn local_args(command: &str, session: &str, tool: &str, args: &Value) -> Value {
    if command == "login_vault_agent_fill" {
        let mut input = json!({ "sessionId": session });
        for key in ["part", "username"] {
            input[key] = args[key].clone();
        }
        input
    } else {
        json!({
            "sessionId": session,
            "tool": tool,
            "arguments": args["arguments"],
            "context": args["context"],
        })
    }
}

/// Runs the tool on this Mac's own host and waits for the person's card and the action.
async fn local_run(
    command: &str,
    session: &str,
    tool: &str,
    args: &Value,
    backend: &Backend,
    cx: &mut AsyncApp,
) -> Result<Value, String> {
    let reply = backend.request_local(command, local_args(command, session, tool, args));
    match reply
        .recv()
        .with_timeout(APPROVED_TIMEOUT, cx.background_executor())
        .await
    {
        Ok(Ok(Ok(value))) => Ok(value),
        Ok(Ok(Err(error))) => Err(error["message"]
            .as_str()
            .unwrap_or("this Mac could not do that")
            .to_owned()),
        Ok(Err(_)) => Err("this Mac did not answer".into()),
        Err(_) => Err("this Mac did not answer in time".into()),
    }
}

/// Waits (up to `LOAD_TIMEOUT`) until the active page has finished loading.
async fn settle(entity: &gpui::Entity<panel_browser::Browser>, cx: &mut AsyncApp) {
    let started = std::time::Instant::now();
    while entity.update(cx, |browser, _| browser.page_loading()) && started.elapsed() < LOAD_TIMEOUT
    {
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
}

/// Waits for a completion handler (it runs on the main thread and wakes this task; no polling).
async fn wait<T>(answer: browser_tab::Answer<T>, cx: &mut AsyncApp) -> Result<T, String> {
    match answer
        .recv()
        .with_timeout(PAGE_TIMEOUT, cx.background_executor())
        .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("the page did not answer".into()),
        Err(_) => Err("the page did not answer in time".into()),
    }
}

// ---- Login prompts ----

pub fn offer_count() -> usize {
    LOGINS.with(|s| s.borrow()["offers"].as_array().map_or(0, Vec::len))
}

/// "Save this login?" offers for a conversation's browser.
pub fn offers_for(session: &str) -> Vec<Value> {
    LOGINS.with(|s| for_session(&s.borrow()["offers"], session))
}

fn for_session(list: &Value, session: &str) -> Vec<Value> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["sessionId"] == session)
        .cloned()
        .collect()
}

/// `https://app.example.com` as the person reads it: the host without the scheme.
pub fn site_label(origin: &str) -> String {
    origin
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned()
}

pub fn answer_offer(id: &str, save: bool, agent_use: bool, cx: &mut App) {
    send(
        "login_vault_answer_offer",
        json!({ "id": id, "save": save, "agentUse": agent_use }),
    );
    LOGINS.with(|s| remove(&mut s.borrow_mut()["offers"], id));
    cx.refresh_windows();
}

fn remove(list: &mut Value, id: &str) {
    if let Some(items) = list.as_array_mut() {
        items.retain(|item| item["id"] != id);
    }
}

fn send(cmd: &str, args: Value) {
    BACKEND.with(|b| {
        if let Some(backend) = b.borrow().as_ref() {
            backend.invoke(cmd, args);
        }
    });
}

// ---- Approval cards ----

/// Agent sign-in approvals and computer-use requests of one conversation, for above the
/// composer. `None` when nothing waits.
pub fn cards(session: &str) -> Option<AnyElement> {
    let logins = LOGINS.with(|s| for_session(&s.borrow()["approvals"], session));
    let computer = COMPUTER.with(|s| for_session(&s.borrow()["requests"], session));
    if logins.is_empty() && computer.is_empty() {
        return None;
    }
    let mut cards: Vec<AnyElement> = Vec::new();
    for (i, approval) in logins.iter().enumerate() {
        let id = approval["id"].as_str().unwrap_or("").to_owned();
        let title = tf(
            "O agente quer entrar em {} como {}",
            &[
                &site_label(approval["origin"].as_str().unwrap_or("")),
                &approval["username"].as_str().unwrap_or(""),
            ],
        );
        let (allow, deny) = (id.clone(), id);
        cards.push(card(
            ("login-approval", i),
            "key",
            title,
            t("O Sirus preenche seu login salvo na página. O agente nunca vê a senha, e você não é perguntado de novo sobre este site nesta conversa."),
            t("Permitir"),
            t("Negar"),
            move |cx| respond_login(&allow, true, cx),
            move |cx| respond_login(&deny, false, cx),
        ));
    }
    for (i, request) in computer.iter().enumerate() {
        let id = request["id"].as_str().unwrap_or("").to_owned();
        let title = tf(
            "{} quer usar {}",
            &[
                &request["provider"].as_str().unwrap_or(""),
                &request["app"].as_str().unwrap_or(""),
            ],
        );
        let (allow, deny) = (id.clone(), id);
        cards.push(card(
            ("computer-request", i),
            "pointer",
            title,
            t("Ele poderá ver a janela deste app e clicar e digitar nela durante esta sessão."),
            t("Permitir nesta sessão"),
            t("Não permitir"),
            move |cx| respond_computer(&allow, true, cx),
            move |cx| respond_computer(&deny, false, cx),
        ));
    }
    Some(div().flex().flex_col().children(cards).into_any_element())
}

fn respond_login(id: &str, allow: bool, cx: &mut App) {
    send(
        "login_vault_answer_approval",
        json!({ "id": id, "allow": allow }),
    );
    LOGINS.with(|s| remove(&mut s.borrow_mut()["approvals"], id));
    cx.refresh_windows();
}

fn respond_computer(id: &str, allow: bool, cx: &mut App) {
    send(
        "computer_action",
        json!({ "action": { "type": "respond", "requestId": id, "allow": allow } }),
    );
    COMPUTER.with(|s| remove(&mut s.borrow_mut()["requests"], id));
    cx.refresh_windows();
}

#[allow(clippy::too_many_arguments)]
fn card(
    id: (&'static str, usize),
    glyph: &'static str,
    title: String,
    body: &'static str,
    allow: &'static str,
    deny: &'static str,
    on_allow: impl Fn(&mut App) + 'static,
    on_deny: impl Fn(&mut App) + 'static,
) -> AnyElement {
    let button = |key: &'static str, label: &'static str, main: bool| {
        let base = div()
            .id((key, id.1))
            .relative()
            .overflow_hidden()
            .h(px(28.))
            .px(px(13.))
            .rounded(px(crate::theme::RADIUS_MD))
            .flex()
            .items_center()
            .cursor_pointer()
            .text_size(px(12.5))
            .hover(|s| s.opacity(0.9));
        if main {
            base.font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::on_accent())
                .bg(theme::accent())
                .child(crate::shell::pixel_stripes(gpui::Hsla {
                    a: 0.18,
                    ..gpui::white()
                }))
                .child(div().relative().child(label))
        } else {
            base.text_color(theme::content(0.85))
                .border_1()
                .border_color(theme::content(0.1))
                .bg(theme::content(0.06))
                .child(label)
        }
    };
    let amber: gpui::Hsla = gpui::rgb(0xe8b05c).into();
    div()
        .id(id)
        .mb(px(8.))
        .p(px(14.))
        .rounded(px(crate::theme::RADIUS_LG))
        .border_1()
        .border_color(theme::accent_wash(0.4))
        .relative()
        .overflow_hidden()
        // The composer's own glass, so the card reads as part of it, not a dark plate above it.
        .child(theme::composer_surface(crate::theme::RADIUS_LG - 1.))
        // The approved look: a pixel glow at the foot and a textured icon, as the secret card.
        .child(crate::shell::question_dither(theme::accent(), 0.22, false))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(
                    div()
                        .relative()
                        .overflow_hidden()
                        .size(px(30.))
                        .flex_shrink_0()
                        .rounded(px(9.))
                        .border_1()
                        .border_color(gpui::Hsla { a: 0.4, ..amber })
                        .bg(gpui::Hsla { a: 0.1, ..amber })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::shell::question_dither(amber, 0.3, true))
                        .child(theme::icon(glyph, 15.).text_color(amber)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme::content(0.95))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .line_height(px(18.))
                                .text_color(theme::content(0.55))
                                .child(body),
                        ),
                ),
        )
        // The main action at the far right, a dotted rule above the row.
        .child(
            div()
                .relative()
                .mt(px(12.))
                .pt(px(12.))
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.))
                .child(crate::shell::dotted_rule())
                .child(button("deny", deny, false).on_click(move |_, _, cx| on_deny(cx)))
                .child(button("allow", allow, true).on_click(move |_, _, cx| on_allow(cx))),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_carry_the_request_id_and_either_a_value_or_an_error() {
        let ok = result_args("r1", Ok(json!({ "tabs": [] })));
        assert_eq!(
            (ok["id"].as_str(), ok["ok"].as_bool()),
            (Some("r1"), Some(true))
        );
        assert!(ok["value"]["tabs"].is_array());
        let err = result_args("r2", Err("No browser tab is open.".into()));
        assert_eq!(err["ok"], false);
        assert_eq!(err["error"], "No browser tab is open.");
        assert!(err.get("value").is_none());
    }

    #[test]
    fn answers_go_back_where_the_request_came_from() {
        assert!(from_remote(&json!({ "id": "a", "via": "remote" })));
        assert!(!from_remote(&json!({ "id": "a", "via": "local" })));
        assert!(!from_remote(&json!({ "id": "a" })));
    }

    #[test]
    fn navigations_and_page_tools_wait_for_the_load_but_status_does_not() {
        assert!(NAVIGATING.contains(&"browser_open"));
        assert!(ON_PAGE.contains(&"browser_snapshot") && ON_PAGE.contains(&"browser_wait_for"));
        for tool in ["browser_status", "browser_tabs", "browser_close"] {
            assert!(!NAVIGATING.contains(&tool) && !ON_PAGE.contains(&tool));
        }
    }

    #[test]
    fn prompts_are_filtered_by_conversation() {
        let list = json!([
            {"id": "a", "sessionId": "s1"},
            {"id": "b", "sessionId": "s2"},
            {"id": "c", "sessionId": "s1"},
        ]);
        let mine = for_session(&list, "s1");
        assert_eq!(mine.len(), 2);
        assert!(for_session(&Value::Null, "s1").is_empty());
        let mut list = list;
        remove(&mut list, "a");
        assert_eq!(list.as_array().unwrap().len(), 2);
    }

    #[test]
    fn sign_ins_and_computer_use_run_on_this_macs_host_with_no_value_from_the_remote() {
        assert_eq!(
            local_command("browser_fill_login"),
            Some("login_vault_agent_fill")
        );
        for tool in ["computer_apps", "computer_click", "computer_launch"] {
            assert_eq!(
                local_command(tool),
                Some("computer_remote_execute"),
                "{tool}"
            );
        }
        for tool in ["browser_open", "browser_snapshot", "fill_login_page"] {
            assert_eq!(local_command(tool), None, "{tool}");
        }
        // Only the part and the username go on, whatever else the request carried.
        let fill = local_args(
            "login_vault_agent_fill",
            "s1",
            "browser_fill_login",
            &json!({ "part": "totp", "username": "me", "password": "nope" }),
        );
        assert_eq!(
            fill,
            json!({ "sessionId": "s1", "part": "totp", "username": "me" })
        );
        let computer = local_args(
            "computer_remote_execute",
            "s1",
            "computer_click",
            &json!({ "arguments": { "app": "Notes" }, "context": { "provider": "Codex" } }),
        );
        assert_eq!(computer["tool"], "computer_click");
        assert_eq!(computer["arguments"]["app"], "Notes");
        assert_eq!(computer["context"]["provider"], "Codex");
    }

    #[test]
    fn sites_read_without_their_scheme() {
        assert_eq!(site_label("https://app.example.com"), "app.example.com");
        assert_eq!(site_label("http://localhost:3000/"), "localhost:3000");
    }
}
