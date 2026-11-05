//! The browser side of the login vault (ADR-186): the page scripts, `browser_fill_login`
//! for agents and the key icon's autofill for the person. Credentials go from
//! `login_vault` into the page as arguments of a fixed script (never spliced into source,
//! never returned): the tool result and the person's command say which fields were filled,
//! not what was put in them.

#![cfg_attr(not(all(target_os = "macos", feature = "desktop")), allow(dead_code))]

use serde_json::{json, Value};

use crate::login_vault::Credentials;

/// Reports submitted password forms to the app. Wrapped here so the script file stays a
/// plain function that can be tested without a browser.
pub const CAPTURE_SCRIPT: &str = concat!(
    "(",
    include_str!("browser_login_capture.js"),
    ")(window, document, function (message) { window.webkit.messageHandlers.sirusLogin.postMessage(message); });"
);

/// The body run by `callAsyncJavaScript`; `args` is its JSON argument.
pub const FILL_SCRIPT: &str = include_str!("browser_login_fill.js");

pub const TOOL_NAME: &str = "browser_fill_login";

pub fn tool_definition() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Sign in on the current page with a login the person saved in Sirus Code's vault. Fills the username and password (part \"login\", the default; on a two-step form call it once per step) or the authenticator code (part \"totp\") straight into the page; you never see the values and the result only says which fields were filled. It works only when the page's origin exactly matches a saved login that the person marked as usable by agents (no subdomain matching), and the first time per site in a session the person is asked to confirm. It does not submit the form: click the sign-in button or press Enter afterwards. If it fails because no login is saved, use request_secret instead of asking in chat.",
        "inputSchema": { "type": "object", "properties": {
            "username": { "type": "string", "description": "Only needed when several logins are saved for the site." },
            "part": { "type": "string", "enum": ["login", "totp"] }
        } }
    })
}

/// What the fill script reported, as a tool result or an error that names no value.
pub fn parse_fill_result(raw: &str, origin: &str) -> Result<Value, String> {
    let parsed: Value =
        serde_json::from_str(raw).map_err(|_| "the page did not answer".to_string())?;
    if parsed.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(parsed
            .get("error")
            .and_then(Value::as_str)
            .map(|error| error.chars().take(200).collect::<String>())
            .unwrap_or_else(|| "the page could not be filled".into()));
    }
    let fields: Vec<&str> = parsed
        .get("fields")
        .and_then(Value::as_array)
        .map(|fields| fields.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    Ok(json!({
        "filled": true,
        "origin": origin,
        "fields": fields,
        "result": format!("filled for {origin}"),
        "note": "The values were put into the page and are not shown to you. Submit the form yourself (click the button or press Enter).",
    }))
}

/// The JSON argument of the fill script. This is the only place the secrets are serialised
/// for the page; `Credentials` itself has no `Serialize`.
pub(crate) fn fill_arguments(
    origin: &str,
    part: &str,
    credentials: &Credentials,
    code: Option<&str>,
) -> Value {
    json!({
        "origin": origin,
        "part": part,
        "username": credentials.username,
        "password": credentials.password,
        "code": code,
    })
}

#[cfg(all(target_os = "macos", feature = "desktop"))]
mod platform {
    use super::*;
    use crate::browser::platform as browser;
    use crate::runtime::AppHandle;
    use objc2_foundation::{MainThreadMarker, NSString};
    use objc2_web_kit::{
        WKContentWorld, WKUserScript, WKUserScriptInjectionTime, WKWebViewConfiguration,
    };

    /// Adds the capture script to a new tab, in the client content world (invisible to the page).
    pub fn install_capture(config: &WKWebViewConfiguration, mtm: MainThreadMarker) {
        unsafe {
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
                mtm.alloc::<WKUserScript>(),
                &NSString::from_str(CAPTURE_SCRIPT),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
                &WKContentWorld::defaultClientWorld(mtm),
            );
            config.userContentController().addUserScript(&script);
        }
    }

    fn active_page(app: &AppHandle, session_id: &str) -> Result<(String, String), String> {
        let state = browser::state(app, session_id).map_err(|error| error.to_string())?;
        let tab = state
            .active_tab_id
            .and_then(|id| state.tabs.into_iter().find(|tab| tab.id == id))
            .ok_or("No browser tab is open.")?;
        Ok((tab.id, tab.url))
    }

    fn fill(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
        origin: &str,
        part: &str,
        credentials: &Credentials,
    ) -> Result<Value, String> {
        let code = match part {
            "totp" => Some(
                credentials
                    .totp
                    .as_deref()
                    .and_then(|seed| {
                        crate::login_vault::totp_code(seed, crate::login_vault::unix_now())
                    })
                    .ok_or("This login has no authenticator secret saved.")?,
            ),
            _ => None,
        };
        // Whatever the page might echo back is masked from the transcript for this turn.
        crate::secrets::note_value(session_id, &credentials.password);
        if let Some(code) = &code {
            crate::secrets::note_value(session_id, code);
        }
        let raw = browser::call_page(
            app,
            session_id,
            tab_id,
            FILL_SCRIPT.to_string(),
            fill_arguments(origin, part, credentials, code.as_deref()),
        )
        .map_err(|error| error.to_string())?;
        parse_fill_result(&raw, origin)
    }

    /// `browser_fill_login`: the model names at most a username; the values never pass through it.
    pub fn agent_fill(app: &AppHandle, session_id: &str, args: &Value) -> Result<Value, String> {
        let part = match args.get("part").and_then(Value::as_str) {
            None | Some("login") => "login",
            Some("totp") => "totp",
            Some(_) => return Err("`part` must be \"login\" or \"totp\"".into()),
        };
        let username = args
            .get("username")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty() && name.len() <= 256);
        let (tab_id, url) = active_page(app, session_id)?;
        let (summary, _) = crate::login_vault::agent_credentials(&url, username)?;
        crate::login_vault::require_approval(app, session_id, &summary.origin, &summary.username)?;
        // The person may have changed the login, or the page, while the card was open.
        let (tab_after, url_after) = active_page(app, session_id)?;
        let (again, credentials) = crate::login_vault::agent_credentials(&url_after, username)?;
        if tab_after != tab_id || again.id != summary.id {
            return Err("The page or the login changed while waiting; call the tool again.".into());
        }
        fill(app, session_id, &tab_id, &again.origin, part, &credentials)
    }

    /// The person's key icon: fills the chosen login into the active tab.
    pub fn person_fill(
        app: &AppHandle,
        session_id: &str,
        login_id: &str,
        part: &str,
    ) -> Result<Value, String> {
        let (tab_id, url) = active_page(app, session_id)?;
        let (summary, credentials) = crate::login_vault::person_credentials(&url, login_id)?;
        fill(
            app,
            session_id,
            &tab_id,
            &summary.origin,
            part,
            &credentials,
        )
    }
}

#[cfg(all(target_os = "macos", feature = "desktop"))]
pub use platform::{agent_fill, install_capture, person_fill};

/// Key icon in the address bar: fill the saved login into the session's active tab.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn login_vault_autofill(
    app: tauri::AppHandle,
    session_id: String,
    login_id: String,
    part: Option<String>,
) -> crate::error::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let part = match part.as_deref() {
            Some("totp") => "totp",
            _ => "login",
        };
        crate::commands::native_task(move || {
            person_fill(&app, &session_id, &login_id, part)
                .map(|_| ())
                .map_err(|error| crate::error::Error::new("login-vault", error))
        })
        .await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, session_id, login_id, part);
        Err(crate::error::Error::new(
            "unsupported",
            "The embedded browser is only available on macOS.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_is_named_for_the_browser_and_takes_no_secret() {
        let tool = tool_definition();
        assert!(tool["name"].as_str().unwrap().starts_with("browser_"));
        let properties = tool["inputSchema"]["properties"].as_object().unwrap();
        assert!(properties
            .keys()
            .all(|key| key == "username" || key == "part"));
    }

    #[test]
    fn the_agent_never_sees_the_secret_in_a_result() {
        // What the page script returns names fields only; the result built from it is all
        // the agent (and so the transcript) ever receives.
        let result = parse_fill_result(
            r#"{"ok":true,"fields":["username","password"]}"#,
            "https://example.com",
        )
        .unwrap();
        let text = result.to_string();
        assert!(text.contains("filled for https://example.com"));
        assert!(text.contains("password") && !text.contains("hunter2"));

        // Even if a hostile page tried to smuggle a value into the report, only the
        // known shape (field names, a bounded error) is relayed.
        let hostile = parse_fill_result(
            r#"{"ok":true,"fields":["password"],"value":"hunter2","echo":"hunter2"}"#,
            "https://example.com",
        )
        .unwrap();
        assert!(!hostile.to_string().contains("hunter2"));
        let error = parse_fill_result(
            &format!(r#"{{"ok":false,"error":"{}"}}"#, "x".repeat(500)),
            "https://example.com",
        )
        .unwrap_err();
        assert_eq!(error.len(), 200);
        assert!(parse_fill_result("not json", "https://example.com").is_err());
    }

    #[test]
    fn credentials_travel_only_as_page_arguments() {
        let credentials = Credentials {
            username: "ana".into(),
            password: "hunter2".into(),
            totp: None,
        };
        let args = fill_arguments("https://example.com", "login", &credentials, None);
        assert_eq!(args["password"], "hunter2");
        // The script source is fixed text: no value can be spliced into it.
        assert!(!FILL_SCRIPT.contains("hunter2"));
        assert!(FILL_SCRIPT.contains("JSON.parse(args)"));
        assert!(!format!("{credentials:?}").contains("hunter2"));
    }
}
