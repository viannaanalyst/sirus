//! The browser side of the login vault (ADR-186): the page scripts, `browser_fill_login`
//! for agents and the key icon's autofill for the person. Credentials go from
//! `login_vault` into the page as arguments of a fixed script (never spliced into source,
//! never returned): the tool result and the person's command say which fields were filled,
//! not what was put in them.

#![allow(dead_code)]

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
