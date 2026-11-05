//! More browser tools for agents (ADR-150): hover, select, drag, upload, JavaScript dialogs,
//! evaluate, the network log, resize and colour scheme. Arguments are checked here before
//! anything reaches a page; the page scripts are fixed and receive their input as JSON
//! arguments (never spliced into source), except `browser_evaluate`, whose whole point is
//! to run the agent's expression (behind Agent browser access, like every browser tool).

#![cfg_attr(not(all(target_os = "macos", feature = "desktop")), allow(dead_code))]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A JavaScript `alert`, `confirm` or `prompt` waiting for an answer. The page is frozen
/// until someone answers: the person in the browser pane or an agent with `browser_dialog`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDialog {
    pub tab_id: String,
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_text: Option<String>,
    /// The page that opened it (origin and path, no query).
    pub url: String,
}

pub fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// Query values and fragments can carry tokens: the network log keeps the parameter
/// names only (`?token=…`), drops user info and fragments, and caps the length.
pub fn redact_url(url: &str) -> String {
    let url = url.trim();
    let (without_fragment, _) = url.split_once('#').unwrap_or((url, ""));
    let (base, query) = without_fragment
        .split_once('?')
        .map_or((without_fragment, None), |(base, query)| {
            (base, Some(query))
        });
    // Drop `user:password@` from the authority.
    let base = match base.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest
                .find('/')
                .map_or((rest, ""), |index| (&rest[..index], &rest[index..]));
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            format!("{scheme}://{host}{path}")
        }
        None => base.to_string(),
    };
    let mut out = truncate(&base, 400);
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        let names: Vec<String> = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .take(20)
            .map(|pair| {
                let name = pair.split_once('=').map_or(pair, |(name, _)| name);
                format!("{}=…", truncate(name, 40))
            })
            .collect();
        out.push('?');
        out.push_str(&names.join("&"));
    }
    out
}

/// One end of a drag: an element (ref or selector) or a viewport point.
#[derive(Debug, Clone, PartialEq)]
pub enum DragEnd {
    Element { reference: String, selector: String },
    Point { x: f64, y: f64 },
}

impl DragEnd {
    pub fn to_json(&self) -> Value {
        match self {
            DragEnd::Element {
                reference,
                selector,
            } => json!({ "ref": reference, "selector": selector }),
            DragEnd::Point { x, y } => json!({ "x": x, "y": y }),
        }
    }
}

fn valid_ref(value: &str) -> bool {
    value.is_empty()
        || ((2..=5).contains(&value.len())
            && value.starts_with('e')
            && value[1..].chars().all(|c| c.is_ascii_digit()))
}

fn clean(value: &Value, key: &str, limit: usize) -> Result<String, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => {
            let text = text.trim();
            if text.chars().count() > limit || text.chars().any(char::is_control) {
                Err(format!("`{key}` is too long or has control characters"))
            } else {
                Ok(text.to_string())
            }
        }
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

/// `{ref}` / `{selector}` / `{x, y}`, or a bare selector string.
pub fn parse_drag_end(value: Option<&Value>, name: &str) -> Result<DragEnd, String> {
    let value = value.ok_or_else(|| format!("`{name}` is required"))?;
    if let Some(selector) = value.as_str() {
        let selector = selector.trim();
        if selector.is_empty()
            || selector.chars().count() > 300
            || selector.chars().any(char::is_control)
        {
            return Err(format!("`{name}` is not a valid selector"));
        }
        return Ok(DragEnd::Element {
            reference: String::new(),
            selector: selector.into(),
        });
    }
    if !value.is_object() {
        return Err(format!(
            "`{name}` must be a selector, {{ref}}, {{selector}} or {{x, y}}"
        ));
    }
    let x = value.get("x").and_then(Value::as_f64);
    let y = value.get("y").and_then(Value::as_f64);
    if let (Some(x), Some(y)) = (x, y) {
        if !(0.0..=20_000.0).contains(&x) || !(0.0..=20_000.0).contains(&y) {
            return Err(format!("`{name}` is outside the page"));
        }
        return Ok(DragEnd::Point { x, y });
    }
    let reference = clean(value, "ref", 8)?;
    let selector = clean(value, "selector", 300)?;
    if !valid_ref(&reference) {
        return Err(format!("`{name}.ref` is not a ref from browser_snapshot"));
    }
    if reference.is_empty() && selector.is_empty() {
        return Err(format!("`{name}` needs a ref, a selector or x and y"));
    }
    Ok(DragEnd::Element {
        reference,
        selector,
    })
}

/// 1–20 option values or labels, each at most 300 characters.
pub fn select_values(args: &Value) -> Result<Vec<String>, String> {
    let values: Vec<String> = match args.get("values") {
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "`values` must be strings".to_string())
            })
            .collect::<Result<_, _>>()?,
        _ => return Err("`values` must list the options to select".into()),
    };
    if values.is_empty() || values.len() > 20 {
        return Err("select 1 to 20 values".into());
    }
    if values
        .iter()
        .any(|value| value.chars().count() > 300 || value.chars().any(char::is_control))
    {
        return Err("a value is too long or has control characters".into());
    }
    Ok(values)
}

pub const UPLOAD_MAX_FILES: usize = 5;
pub const UPLOAD_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Files an agent may put in a page: regular files inside the session's workspace or
/// among the session's attachments, resolved on disk (symlinks cannot point outside).
pub fn validate_upload_paths(
    paths: &[String],
    workspace: Option<&Path>,
    attachments: &[PathBuf],
) -> Result<Vec<PathBuf>, String> {
    if paths.is_empty() || paths.len() > UPLOAD_MAX_FILES {
        return Err(format!("upload 1 to {UPLOAD_MAX_FILES} files"));
    }
    let root = workspace.and_then(|root| std::fs::canonicalize(root).ok());
    let attached: Vec<PathBuf> = attachments
        .iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .collect();
    let mut total = 0u64;
    let mut out = Vec::new();
    for raw in paths {
        if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_control) {
            return Err("invalid file path".into());
        }
        let candidate = Path::new(raw);
        let candidate = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            match &root {
                Some(root) => root.join(candidate),
                None => return Err("give an absolute path".into()),
            }
        };
        let resolved =
            std::fs::canonicalize(&candidate).map_err(|_| format!("file not found: {raw}"))?;
        let inside = root.as_ref().is_some_and(|root| resolved.starts_with(root))
            || attached.contains(&resolved);
        if !inside {
            return Err(format!(
                "{raw} is outside this conversation's workspace and attachments"
            ));
        }
        let metadata = std::fs::metadata(&resolved).map_err(|_| format!("cannot read {raw}"))?;
        if !metadata.is_file() {
            return Err(format!("{raw} is not a file"));
        }
        total += metadata.len();
        if total > UPLOAD_MAX_BYTES {
            return Err("files may total at most 10 MB".into());
        }
        out.push(resolved);
    }
    Ok(out)
}

/// A content type for an uploaded file, from its extension.
pub fn upload_mime(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// Longest expression `browser_evaluate` takes, and the most result text it returns.
pub const EVALUATE_MAX_SOURCE: usize = 20_000;
pub const EVALUATE_MAX_RESULT: usize = 32 * 1024;

/// The page-side wrapper for `browser_evaluate`: runs `source` as an expression (or, with
/// `statements`, as an async function body that may `return`) and answers with bounded JSON.
pub fn evaluate_body(source: &str, statements: bool) -> String {
    let inner = if statements {
        source.to_string()
    } else {
        format!("return (\n{source}\n);")
    };
    format!(
        r#"const __sirusRun = async () => {{
{inner}
}};
let __sirusValue;
try {{ __sirusValue = await __sirusRun(); }} catch (error) {{
  return JSON.stringify({{ ok: true, threw: true, error: String((error && error.stack) || error).slice(0, 2000) }});
}}
let __sirusText;
try {{
  __sirusText = JSON.stringify(__sirusValue === undefined ? null : __sirusValue, (key, value) => typeof value === "bigint" ? String(value) : (typeof Node !== "undefined" && value instanceof Node) ? "<" + String(value.nodeName).toLowerCase() + ">" : typeof value === "function" ? "[function]" : value);
}} catch {{ __sirusText = JSON.stringify(String(__sirusValue)); }}
if (__sirusText === undefined) __sirusText = "null";
const __sirusLimit = {limit};
return JSON.stringify({{ ok: true, type: __sirusValue === null ? "null" : Array.isArray(__sirusValue) ? "array" : typeof __sirusValue, json: __sirusText.slice(0, __sirusLimit), truncated: __sirusText.length > __sirusLimit }});"#,
        limit = EVALUATE_MAX_RESULT
    )
}

/// `http://localhost:<port>` for `browser_open {port}`.
pub fn localhost_url(port: &Value) -> Result<String, String> {
    let port = port
        .as_u64()
        .filter(|port| (1..=65_535).contains(port))
        .ok_or_else(|| "`port` must be a number from 1 to 65535".to_string())?;
    Ok(format!("http://localhost:{port}"))
}

/// Viewport sides for `browser_resize`, as device mode takes them.
pub fn resize_size(args: &Value) -> Result<(u32, u32), String> {
    let side = |key: &str| {
        args.get(key)
            .and_then(Value::as_u64)
            .filter(|value| (240..=3840).contains(value))
            .map(|value| value as u32)
            .ok_or_else(|| format!("`{key}` must be a whole number from 240 to 3840"))
    };
    Ok((side("width")?, side("height")?))
}

pub fn color_scheme(args: &Value) -> Result<&'static str, String> {
    match args.get("scheme").and_then(Value::as_str).map(str::trim) {
        Some("light") => Ok("light"),
        Some("dark") => Ok("dark"),
        Some("system") | Some("no-preference") => Ok("system"),
        _ => Err("`scheme` must be light, dark or system".into()),
    }
}

/// Network entries from the page, bounded and with URLs redacted.
pub fn network_entries(raw: &Value, limit: usize) -> Vec<Value> {
    let Some(items) = raw.get("entries").and_then(Value::as_array) else {
        return Vec::new();
    };
    let skip = items.len().saturating_sub(limit);
    items
        .iter()
        .skip(skip)
        .filter_map(|item| {
            let url = item.get("url").and_then(Value::as_str)?;
            let mut entry = json!({
                "method": truncate(item.get("method").and_then(Value::as_str).unwrap_or("GET"), 12),
                "url": redact_url(url),
                "type": truncate(item.get("kind").and_then(Value::as_str).unwrap_or("resource"), 24),
                "status": item.get("status").and_then(Value::as_u64),
                "durationMs": item.get("durationMs").and_then(Value::as_f64).map(|value| value.round()),
            });
            if let Some(error) = item.get("error").and_then(Value::as_str) {
                entry["error"] = json!(truncate(error, 200));
            }
            if let Some(size) = item.get("size").and_then(Value::as_u64) {
                entry["bytes"] = json!(size);
            }
            Some(entry)
        })
        .collect()
}

#[cfg(all(target_os = "macos", feature = "desktop"))]
pub(crate) mod platform {
    use std::path::PathBuf;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool};
    use objc2_foundation::{
        MainThreadMarker, NSDate, NSHTTPCookie, NSHTTPCookieDomain, NSHTTPCookieExpires,
        NSHTTPCookieName, NSHTTPCookiePath, NSHTTPCookieSecure, NSHTTPCookieValue,
        NSMutableDictionary, NSString,
    };
    use objc2_web_kit::{WKUserScript, WKUserScriptInjectionTime, WKWebViewConfiguration};
    use serde_json::{json, Value};

    use super::{BrowserDialog, DragEnd};
    use crate::browser::platform as browser;
    use crate::browser::BrowserSessionState;
    use crate::browser_cookies::{CookieImportReport, DecodedCookie};
    use crate::error::{Error, Result};
    use crate::runtime::{AppHandle, Emitter};

    enum Reply {
        Alert(RcBlock<dyn Fn()>),
        Confirm(RcBlock<dyn Fn(Bool)>),
        Prompt(RcBlock<dyn Fn(*mut NSString)>, String),
    }

    /// A dialog's completion handler. WebKit requires it to be called exactly once, so
    /// dropping an unanswered dialog (tab closed, browser released) dismisses it.
    pub struct PendingDialog {
        pub info: BrowserDialog,
        reply: Option<Reply>,
    }

    impl PendingDialog {
        pub fn answer(mut self, accept: bool, text: Option<&str>) {
            if let Some(reply) = self.reply.take() {
                call(reply, accept, text);
            }
        }
    }

    impl Drop for PendingDialog {
        fn drop(&mut self) {
            // An unanswered dialog is dismissed, so the page never stays frozen.
            if let Some(reply) = self.reply.take() {
                call(reply, false, None);
            }
        }
    }

    fn call(reply: Reply, accept: bool, text: Option<&str>) {
        match reply {
            Reply::Alert(handler) => handler.call(()),
            Reply::Confirm(handler) => handler.call((Bool::new(accept),)),
            Reply::Prompt(handler, default) => {
                if accept {
                    let value = NSString::from_str(text.unwrap_or(&default));
                    handler.call((Retained::as_ptr(&value) as *mut NSString,));
                } else {
                    handler.call((std::ptr::null_mut(),));
                }
            }
        }
    }

    pub enum DialogHandler<'a> {
        Alert(&'a block2::DynBlock<dyn Fn()>),
        Confirm(&'a block2::DynBlock<dyn Fn(Bool)>),
        Prompt(&'a block2::DynBlock<dyn Fn(*mut NSString)>, String),
    }

    /// From the tab's WKUIDelegate: queue the dialog on its tab and tell the pane.
    pub fn on_dialog(
        app: &AppHandle,
        session_id: &str,
        tab_id: &str,
        handler: DialogHandler<'_>,
        message: String,
        url: String,
    ) {
        let (kind, reply, default_text) = match handler {
            DialogHandler::Alert(block) => ("alert", Reply::Alert(block.copy()), None),
            DialogHandler::Confirm(block) => ("confirm", Reply::Confirm(block.copy()), None),
            DialogHandler::Prompt(block, default) => (
                "prompt",
                Reply::Prompt(block.copy(), default.clone()),
                Some(super::truncate(&default, 500)),
            ),
        };
        let info = BrowserDialog {
            tab_id: tab_id.to_string(),
            kind: kind.to_string(),
            message: super::truncate(&message, 1000),
            default_text,
            url: super::redact_url(&url)
                .split('?')
                .next()
                .unwrap_or_default()
                .to_string(),
        };
        let pending = PendingDialog {
            info,
            reply: Some(reply),
        };
        // If the session is gone or the browser is busy, `pending` drops here and dismisses.
        if let Some(state) = browser::queue_dialog(session_id, pending) {
            let _ = app.emit("browser-state", state);
        }
    }

    /// Answers the open dialog of `tab_id` (or of any tab of the session).
    pub fn answer_dialog(
        app: &AppHandle,
        session_id: &str,
        tab_id: Option<String>,
        accept: bool,
        text: Option<String>,
    ) -> Result<(BrowserDialog, BrowserSessionState)> {
        let session = session_id.to_string();
        let result = browser::on_main(
            app,
            move || -> Result<(BrowserDialog, BrowserSessionState)> {
                let (pending, state) = browser::take_dialog(&session, tab_id.as_deref())
                    .ok_or_else(|| Error::not_found("No dialog is open in this browser."))?;
                let info = pending.info.clone();
                pending.answer(accept, text.as_deref());
                Ok((info, state))
            },
        )??;
        let _ = app.emit("browser-state", result.1.clone());
        Ok(result)
    }

    /// Records fetch/XHR calls and resource timings for `browser_network`, at document start.
    const NETWORK_SCRIPT: &str = r##"
(() => {
  if (window.__sirusNetwork) return;
  const log = window.__sirusNetwork = [];
  const push = (entry) => { log.push(entry); if (log.length > 300) log.shift(); };
  const now = () => performance.now();
  const absolute = (value) => { try { return new URL(String(value), location.href).href.slice(0, 2048); } catch { return String(value).slice(0, 2048); } };
  const originalFetch = window.fetch;
  if (typeof originalFetch === "function") {
    window.fetch = function (resource, init) {
      const started = now();
      const request = typeof Request !== "undefined" && resource instanceof Request ? resource : null;
      const entry = { kind: "fetch", method: String((init && init.method) || (request && request.method) || "GET").toUpperCase(), url: absolute(request ? request.url : resource), status: 0, at: Date.now() };
      return originalFetch.apply(this, arguments).then((response) => {
        entry.status = response.status; entry.durationMs = now() - started; push(entry); return response;
      }, (error) => {
        entry.error = String(error).slice(0, 200); entry.durationMs = now() - started; push(entry); throw error;
      });
    };
  }
  const open = XMLHttpRequest.prototype.open;
  const send = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url) {
    this.__sirusEntry = { kind: "xhr", method: String(method || "GET").toUpperCase(), url: absolute(url), status: 0 };
    return open.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function () {
    const entry = this.__sirusEntry;
    if (entry) {
      const started = now();
      entry.at = Date.now();
      this.addEventListener("loadend", () => { entry.status = this.status; entry.durationMs = now() - started; if (!this.status) entry.error = "failed"; push(entry); }, { once: true });
    }
    return send.apply(this, arguments);
  };
  try {
    new PerformanceObserver((list) => {
      for (const item of list.getEntries()) {
        if (item.initiatorType === "fetch" || item.initiatorType === "xmlhttprequest") continue;
        push({ kind: item.initiatorType || "resource", method: "GET", url: String(item.name).slice(0, 2048), status: item.responseStatus || null, at: Math.round(performance.timeOrigin + item.startTime), durationMs: item.duration, size: item.transferSize || undefined });
      }
    }).observe({ type: "resource", buffered: true });
  } catch {}
})()
"##;

    /// Adds the agent tools' document-start scripts to a new tab's configuration.
    pub fn install_scripts(config: &WKWebViewConfiguration, mtm: MainThreadMarker) {
        unsafe {
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                mtm.alloc::<WKUserScript>(),
                &NSString::from_str(NETWORK_SCRIPT),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
            );
            config.userContentController().addUserScript(&script);
        }
    }

    macro_rules! find_element {
        () => {
            r##"
const input = JSON.parse(args);
const find = (spec) => spec.ref ? document.querySelector("[data-sirus-ref='" + spec.ref + "']") : (spec.selector ? document.querySelector(spec.selector) : null);
const humanBusy = () => Date.now() - (window.__sirusHumanAt || 0) < 1500;
"##
        };
    }

    const HOVER_SCRIPT: &str = concat!(
        find_element!(),
        r##"
if (humanBusy()) return JSON.stringify({ interrupted: true });
const target = find(input);
if (!target) return JSON.stringify({ ok: false, error: "element not found" });
target.scrollIntoView({ block: "center", inline: "center" });
const rect = target.getBoundingClientRect();
const x = rect.left + rect.width / 2, y = rect.top + rect.height / 2;
const init = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, view: window };
const pointer = { ...init, pointerId: 1, pointerType: "mouse", isPrimary: true };
target.dispatchEvent(new PointerEvent("pointerover", pointer));
target.dispatchEvent(new PointerEvent("pointerenter", { ...pointer, bubbles: false }));
target.dispatchEvent(new MouseEvent("mouseover", init));
target.dispatchEvent(new MouseEvent("mouseenter", { ...init, bubbles: false }));
target.dispatchEvent(new PointerEvent("pointermove", pointer));
target.dispatchEvent(new MouseEvent("mousemove", init));
return JSON.stringify({ ok: true, x: Math.round(x), y: Math.round(y) });
"##
    );

    const SELECT_SCRIPT: &str = concat!(
        find_element!(),
        r##"
if (humanBusy()) return JSON.stringify({ interrupted: true });
const target = find(input);
if (!target) return JSON.stringify({ ok: false, error: "element not found" });
const wanted = input.values.map(String);
const text = (node) => (node.textContent || "").trim().replace(/\s+/g, " ");
if (target instanceof HTMLSelectElement) {
  const options = Array.from(target.options);
  const chosen = options.filter((option) => wanted.some((value) => value === option.value || value === (option.label || "").trim() || value === text(option)));
  const known = options.slice(0, 50).map((option) => ({ value: option.value, label: (option.label || text(option)).slice(0, 80) }));
  if (!chosen.length) return JSON.stringify({ ok: false, error: "no option matches; options: " + JSON.stringify(known) });
  if (!target.multiple && wanted.length > 1) return JSON.stringify({ ok: false, error: "this select takes one value" });
  if (target.disabled) return JSON.stringify({ ok: false, error: "the select is disabled" });
  target.focus();
  if (target.multiple) { for (const option of options) option.selected = chosen.includes(option); }
  else target.value = chosen[0].value;
  target.dispatchEvent(new Event("input", { bubbles: true }));
  target.dispatchEvent(new Event("change", { bubbles: true }));
  return JSON.stringify({ ok: true, selected: chosen.map((option) => option.value) });
}
const owned = (node) => { const id = node.getAttribute("aria-controls") || node.getAttribute("aria-owns"); return id ? document.getElementById(id) : null; };
let listbox = target.getAttribute("role") === "listbox" ? target : owned(target);
if (!listbox && (target.getAttribute("role") === "combobox" || target.getAttribute("aria-haspopup"))) {
  target.click();
  await new Promise((resolve) => setTimeout(resolve, 200));
  listbox = owned(target) || document.querySelector("[role='listbox']");
}
if (!listbox) return JSON.stringify({ ok: false, error: "the element is not a <select>, a listbox or a combobox with one" });
const options = Array.from(listbox.querySelectorAll("[role='option']"));
const chosen = options.filter((option) => wanted.some((value) => value === text(option) || value === (option.getAttribute("data-value") || "") || value === (option.getAttribute("aria-label") || "")));
if (!chosen.length) return JSON.stringify({ ok: false, error: "no option matches; options: " + JSON.stringify(options.slice(0, 50).map(text)) });
for (const option of chosen) {
  option.scrollIntoView({ block: "nearest" });
  const rect = option.getBoundingClientRect();
  const init = { bubbles: true, cancelable: true, composed: true, clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2, view: window };
  option.dispatchEvent(new PointerEvent("pointerdown", { ...init, pointerId: 1, pointerType: "mouse", isPrimary: true }));
  option.dispatchEvent(new MouseEvent("mousedown", init));
  option.dispatchEvent(new PointerEvent("pointerup", { ...init, pointerId: 1, pointerType: "mouse", isPrimary: true }));
  option.dispatchEvent(new MouseEvent("mouseup", init));
  option.click();
}
return JSON.stringify({ ok: true, selected: chosen.map((option) => text(option).slice(0, 80)) });
"##
    );

    const DRAG_SCRIPT: &str = concat!(
        find_element!(),
        r##"
if (humanBusy()) return JSON.stringify({ interrupted: true });
const isPoint = (spec) => typeof spec.x === "number";
const fromEl = isPoint(input.from) ? null : find(input.from);
const toEl = isPoint(input.to) ? null : find(input.to);
if (!isPoint(input.from) && !fromEl) return JSON.stringify({ ok: false, error: "drag source not found" });
if (!isPoint(input.to) && !toEl) return JSON.stringify({ ok: false, error: "drop target not found" });
if (fromEl) fromEl.scrollIntoView({ block: "center", inline: "center" });
const center = (el) => { const r = el.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; };
const a = fromEl ? center(fromEl) : { x: input.from.x, y: input.from.y };
const b = toEl ? center(toEl) : { x: input.to.x, y: input.to.y };
const at = (p) => document.elementFromPoint(p.x, p.y) || document.body;
const source = fromEl || at(a);
const mouse = (type, p, el, buttons) => {
  const init = { bubbles: true, cancelable: true, composed: true, clientX: p.x, clientY: p.y, button: 0, buttons, view: window };
  el.dispatchEvent(new PointerEvent(type.replace("mouse", "pointer"), { ...init, pointerId: 1, pointerType: "mouse", isPrimary: true }));
  el.dispatchEvent(new MouseEvent(type, init));
};
const draggable = source.closest ? source.closest("[draggable='true']") : null;
const data = draggable ? new DataTransfer() : null;
const drag = (type, p, el) => el.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, composed: true, clientX: p.x, clientY: p.y, dataTransfer: data }));
mouse("mousedown", a, source, 1);
if (draggable) drag("dragstart", a, draggable);
let last = source;
const steps = 10;
for (let i = 1; i <= steps; i++) {
  const p = { x: a.x + (b.x - a.x) * i / steps, y: a.y + (b.y - a.y) * i / steps };
  const over = i === steps && toEl ? toEl : at(p);
  if (draggable) { if (over !== last) drag("dragenter", p, over); drag("dragover", p, over); }
  else mouse("mousemove", p, over, 1);
  last = over;
  await new Promise((resolve) => setTimeout(resolve, 16));
}
const target = toEl || at(b);
if (draggable) { drag("drop", b, target); drag("dragend", b, draggable); }
mouse("mouseup", b, target, 0);
return JSON.stringify({ ok: true, from: { x: Math.round(a.x), y: Math.round(a.y) }, to: { x: Math.round(b.x), y: Math.round(b.y) }, html5: Boolean(draggable) });
"##
    );

    const UPLOAD_SCRIPT: &str = concat!(
        find_element!(),
        r##"
if (humanBusy()) return JSON.stringify({ interrupted: true });
const target = find(input);
if (!target) return JSON.stringify({ ok: false, error: "element not found" });
const transfer = new DataTransfer();
for (const file of input.files) {
  const binary = atob(file.data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  transfer.items.add(new File([bytes], file.name, { type: file.type }));
}
if (target instanceof HTMLInputElement && target.type === "file") {
  if (!target.multiple && input.files.length > 1) return JSON.stringify({ ok: false, error: "this file input takes one file" });
  if (target.disabled) return JSON.stringify({ ok: false, error: "the file input is disabled" });
  target.files = transfer.files;
  target.dispatchEvent(new Event("input", { bubbles: true }));
  target.dispatchEvent(new Event("change", { bubbles: true }));
  return JSON.stringify({ ok: true, files: target.files.length, via: "input" });
}
// Not a file input: drop the files on it, as a drag-and-drop upload area expects.
target.scrollIntoView({ block: "center" });
const rect = target.getBoundingClientRect();
const init = { bubbles: true, cancelable: true, composed: true, clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2, dataTransfer: transfer };
for (const type of ["dragenter", "dragover", "drop"]) target.dispatchEvent(new DragEvent(type, init));
return JSON.stringify({ ok: true, files: input.files.length, via: "drop" });
"##
    );

    const NETWORK_READ_SCRIPT: &str = r##"
const input = JSON.parse(args);
const log = window.__sirusNetwork || [];
const entries = log.slice(-Math.max(1, Math.min(200, input.limit || 50)));
if (input.clear) log.length = 0;
return JSON.stringify({ ok: true, url: location.href, entries });
"##;

    fn element_args(reference: &str, selector: &str) -> Value {
        json!({ "ref": reference, "selector": selector })
    }

    pub fn hover(
        app: &AppHandle,
        session_id: &str,
        reference: &str,
        selector: &str,
    ) -> Result<String> {
        browser::page_action(
            app,
            session_id,
            HOVER_SCRIPT,
            element_args(reference, selector),
        )
    }

    pub fn select(
        app: &AppHandle,
        session_id: &str,
        reference: &str,
        selector: &str,
        values: Vec<String>,
    ) -> Result<String> {
        let mut args = element_args(reference, selector);
        args["values"] = json!(values);
        browser::page_action(app, session_id, SELECT_SCRIPT, args)
    }

    pub fn drag(app: &AppHandle, session_id: &str, from: &DragEnd, to: &DragEnd) -> Result<String> {
        browser::page_action(
            app,
            session_id,
            DRAG_SCRIPT,
            json!({ "from": from.to_json(), "to": to.to_json() }),
        )
    }

    pub fn upload(
        app: &AppHandle,
        session_id: &str,
        reference: &str,
        selector: &str,
        files: &[PathBuf],
    ) -> Result<String> {
        use base64::Engine as _;
        let mut encoded = Vec::new();
        for path in files {
            let bytes = read_bounded(path)?;
            encoded.push(json!({
                "name": path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_else(|| "file".into()),
                "type": super::upload_mime(path),
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }));
        }
        let mut args = element_args(reference, selector);
        args["files"] = json!(encoded);
        browser::page_action(app, session_id, UPLOAD_SCRIPT, args)
    }

    /// Reads a checked file without following a link swapped in since the check.
    fn read_bounded(path: &std::path::Path) -> Result<Vec<u8>> {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| Error::new("invalid", "Cannot read the file to upload."))?;
        if !file.metadata()?.is_file() {
            return Err(Error::new("invalid", "Only regular files can be uploaded."));
        }
        let mut bytes = Vec::new();
        file.take(super::UPLOAD_MAX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > super::UPLOAD_MAX_BYTES {
            return Err(Error::new("invalid", "Files may total at most 10 MB."));
        }
        Ok(bytes)
    }

    /// Runs the agent's JavaScript in the page; a syntax error as an expression retries it
    /// as a function body (statements with `return`).
    pub fn evaluate(app: &AppHandle, session_id: &str, source: &str) -> Result<Value> {
        let tab_id = browser::opened_active_tab(app, session_id)?;
        let mut last = String::new();
        for statements in [false, true] {
            let raw = browser::call_page(
                app,
                session_id,
                &tab_id,
                super::evaluate_body(source, statements),
                json!({}),
            )?;
            let parsed: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
            if parsed.get("ok").and_then(Value::as_bool) == Some(true) {
                return Ok(parsed);
            }
            last = parsed
                .get("error")
                .and_then(Value::as_str)
                .map(|text| super::truncate(text, 600))
                .unwrap_or_else(|| "the script could not run".into());
            if !last.to_ascii_lowercase().contains("syntax") {
                break;
            }
        }
        Err(Error::new("invalid", last))
    }

    pub fn network(app: &AppHandle, session_id: &str, limit: usize, clear: bool) -> Result<String> {
        browser::page_action(
            app,
            session_id,
            NETWORK_READ_SCRIPT,
            json!({ "limit": limit, "clear": clear }),
        )
    }

    pub fn resize(
        app: &AppHandle,
        session_id: &str,
        width: u32,
        height: u32,
    ) -> Result<BrowserSessionState> {
        let device = crate::browser::device_from(None, Some(width), Some(height), width > height)?;
        browser::mcp_device(app, session_id, Some(device))
    }

    pub fn color_scheme(
        app: &AppHandle,
        session_id: &str,
        scheme: &str,
    ) -> Result<BrowserSessionState> {
        browser::opened_active_tab(app, session_id)?;
        let state = browser::set_appearance(app, session_id, scheme)?;
        let _ = app.emit("browser-state", state.clone());
        Ok(state)
    }

    /// Closes every tab of a custom profile, then deletes its website data from disk.
    pub fn delete_profile(app: &AppHandle, profile: &str) -> Result<()> {
        if !matches!(
            crate::browser_profiles::store_of(profile),
            Some(crate::browser_profiles::Store::Custom(_))
        ) {
            return Err(Error::new(
                "invalid",
                "Only custom browser profiles can be deleted.",
            ));
        }
        let closing = profile.to_string();
        let states = browser::on_main(app, move || browser::close_profile_tabs(&closing))?;
        for state in states {
            let _ = app.emit("browser-state", state);
        }
        if !crate::browser_profiles::custom_supported() {
            return Ok(());
        }
        // A later turn of the run loop, once the closed views are released.
        let (sender, receiver) = std::sync::mpsc::channel::<Option<String>>();
        let removing = profile.to_string();
        browser::on_main(app, move || {
            let Some(mtm) = MainThreadMarker::new() else {
                let _ = sender.send(Some("the browser must run on the main thread".into()));
                return;
            };
            let reply = sender.clone();
            if !crate::browser_profiles::platform::remove(
                &removing,
                move |error| {
                    let _ = reply.send(error);
                },
                mtm,
            ) {
                let _ = sender.send(None);
            }
        })?;
        match receiver.recv_timeout(std::time::Duration::from_secs(15)) {
            Ok(None) => Ok(()),
            Ok(Some(message)) => Err(Error::new(
                "native",
                format!("The profile's data could not be deleted: {message}"),
            )),
            Err(_) => Err(Error::new(
                "native",
                "Deleting the profile's data took too long.",
            )),
        }
    }

    /// Settings → Browser profiles → Import cookies (ADR-152). Reads the owner's domains from the
    /// chosen source, in memory, and stores them in the profile's WebKit cookie store.
    pub fn import_cookies(
        app: &AppHandle,
        profile: &str,
        source_id: &str,
        domains: &[String],
    ) -> Result<CookieImportReport> {
        use crate::browser_profiles::{custom_supported, from_settings, store_of, Store};
        let (profiles, _) = from_settings(app);
        let persistent = match store_of(profile) {
            Some(Store::Default) => true,
            Some(Store::Custom(_)) => {
                custom_supported() && profiles.iter().any(|item| item.id == profile)
            }
            _ => false,
        };
        if !persistent {
            return Err(Error::new(
                "invalid",
                "Cookies can be imported into a saved browser profile only.",
            ));
        }
        let domains = crate::browser_cookies::normalize_domains(domains)?;
        let home = crate::new_project::home_dir()
            .ok_or_else(|| Error::new("native", "The home folder is unavailable."))?;
        let source = crate::browser_cookies::source_from_id(&home, source_id)?;
        let cookies = crate::browser_cookies::read_source(&home, &source)?;
        // The Keychain is asked only if a selected Chrome cookie is encrypted (ADR-152).
        let (decoded, skipped) = crate::browser_cookies::collect_import(
            cookies,
            &domains,
            crate::browser_cookies::now_unix(),
            || {
                let password = crate::browser_cookies::chrome_safe_storage_password()?;
                Ok(crate::browser_cookies::chrome_key(&password))
            },
        )?;
        let decoded_count = decoded.len() as u32;
        let imported = store_cookies(app, profile, decoded)?;
        Ok(CookieImportReport {
            imported,
            skipped: skipped + (decoded_count - imported),
        })
    }

    /// Adds cookies to a profile's WebKit cookie store on the main thread, and waits until WebKit
    /// has stored them all. Returns how many were built into cookies WebKit accepted.
    fn store_cookies(app: &AppHandle, profile: &str, cookies: Vec<DecodedCookie>) -> Result<u32> {
        if cookies.is_empty() {
            return Ok(0);
        }
        let (done_sender, done_receiver) = std::sync::mpsc::channel::<()>();
        let profile = profile.to_string();
        let stored = browser::on_main(app, move || -> Result<u32> {
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| Error::new("native", "the browser must run on the main thread"))?;
            let made: Vec<Retained<NSHTTPCookie>> = cookies.iter().filter_map(new_cookie).collect();
            if made.is_empty() {
                return Ok(0);
            }
            let mut scratch = None;
            let store = crate::browser_profiles::platform::store(&profile, &mut scratch, mtm);
            let cookie_store = unsafe { store.httpCookieStore() };
            let pending = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(made.len()));
            for cookie in &made {
                let pending = pending.clone();
                let done = done_sender.clone();
                let handler = block2::RcBlock::new(move || {
                    let left = pending.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    if left == 1 {
                        let _ = done.send(());
                    }
                });
                unsafe {
                    cookie_store.setCookie_completionHandler(cookie, Some(&handler));
                }
            }
            Ok(made.len() as u32)
        })??;
        if stored > 0 {
            done_receiver
                .recv_timeout(std::time::Duration::from_secs(15))
                .map_err(|_| {
                    Error::new(
                        "native",
                        "WebKit did not confirm the imported cookies in time.",
                    )
                })?;
        }
        Ok(stored)
    }

    /// A WebKit cookie from decoded properties; `None` when Foundation refuses them.
    fn new_cookie(cookie: &DecodedCookie) -> Option<Retained<NSHTTPCookie>> {
        let properties = NSMutableDictionary::<NSString, AnyObject>::new();
        let name = NSString::from_str(&cookie.name);
        let value = NSString::from_str(&cookie.value);
        let domain = NSString::from_str(&cookie.host);
        let path = NSString::from_str(if cookie.path.is_empty() {
            "/"
        } else {
            &cookie.path
        });
        let yes = NSString::from_str("TRUE");
        // Foundation has no constant for HttpOnly; its property dictionary takes this key.
        let http_only_key = NSString::from_str("HttpOnly");
        unsafe {
            properties.insert(NSHTTPCookieName, &name);
            properties.insert(NSHTTPCookieValue, &value);
            properties.insert(NSHTTPCookieDomain, &domain);
            properties.insert(NSHTTPCookiePath, &path);
            if cookie.secure {
                properties.insert(NSHTTPCookieSecure, &yes);
            }
            if cookie.http_only {
                properties.insert(&*http_only_key, &yes);
            }
            if let Some(expires) = cookie.expires {
                let date = NSDate::dateWithTimeIntervalSince1970(expires);
                properties.insert(NSHTTPCookieExpires, &date);
            }
            NSHTTPCookie::cookieWithProperties(&properties)
        }
    }
}

/// The person answers a page's dialog from the browser pane.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_dialog_reply(
    app: tauri::AppHandle,
    session_id: String,
    tab_id: String,
    accept: bool,
    text: Option<String>,
) -> crate::error::Result<crate::browser::BrowserSessionState> {
    let text = text.map(|value| truncate(&value, 16 * 1024));
    #[cfg(target_os = "macos")]
    {
        crate::commands::native_task(move || {
            platform::answer_dialog(&app, &session_id, Some(tab_id), accept, text)
                .map(|(_, state)| state)
        })
        .await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, session_id, tab_id, accept, text);
        Err(crate::error::Error::new(
            "unsupported",
            "The embedded browser is only available on macOS.",
        ))
    }
}

/// Settings → Browser profiles: closes the profile's tabs and deletes its data. The renderer
/// then saves settings without it.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_profile_delete(
    app: tauri::AppHandle,
    profile_id: String,
) -> crate::error::Result<()> {
    #[cfg(target_os = "macos")]
    {
        crate::commands::native_task(move || platform::delete_profile(&app, &profile_id)).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, profile_id);
        Ok(())
    }
}

/// Whether custom profiles work on this Mac (macOS 14 and later).
#[cfg(feature = "desktop")]
#[tauri::command]
pub fn browser_profiles_supported() -> bool {
    crate::browser_profiles::custom_supported()
}

/// Settings → Browser profiles → Import cookies (ADR-152): the browsers and Chrome profiles
/// the owner can import from. Names only; nothing is read beyond the folder listing.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_cookie_sources(
) -> crate::error::Result<Vec<crate::browser_cookies::CookieSource>> {
    #[cfg(target_os = "macos")]
    {
        crate::commands::native_task(move || {
            let home = crate::new_project::home_dir().ok_or_else(|| {
                crate::error::Error::new("native", "The home folder is unavailable.")
            })?;
            Ok(crate::browser_cookies::list_sources(&home))
        })
        .await
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(crate::error::Error::new(
            "unsupported",
            "Importing cookies is only available on macOS.",
        ))
    }
}

/// The domains that have cookies in one source, with counts. Values never leave the back end.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_cookie_domains(
    source_id: String,
) -> crate::error::Result<Vec<crate::browser_cookies::CookieDomain>> {
    #[cfg(target_os = "macos")]
    {
        crate::commands::native_task(move || {
            let home = crate::new_project::home_dir().ok_or_else(|| {
                crate::error::Error::new("native", "The home folder is unavailable.")
            })?;
            let source = crate::browser_cookies::source_from_id(&home, &source_id)?;
            let cookies = crate::browser_cookies::read_source(&home, &source)?;
            Ok(crate::browser_cookies::domain_summary(
                &cookies,
                crate::browser_cookies::now_unix(),
            ))
        })
        .await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = source_id;
        Err(crate::error::Error::new(
            "unsupported",
            "Importing cookies is only available on macOS.",
        ))
    }
}

/// Opens Privacy & Security at Full Disk Access, the setting Safari's cookie file needs (ADR-152).
/// The address is fixed here, so the renderer cannot point it anywhere else.
#[cfg(feature = "desktop")]
#[tauri::command]
pub fn browser_open_full_disk_access(app: tauri::AppHandle) -> crate::error::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_opener::OpenerExt;
        const FULL_DISK_ACCESS: &str =
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
        app.opener()
            .open_url(FULL_DISK_ACCESS, None::<&str>)
            .map_err(|err| crate::error::Error::new("opener", err.to_string()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}

/// Imports the owner's ticked domains from one source into a saved profile. The renderer only
/// calls this after the consent step; the back end checks the profile and the domain names.
#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn browser_cookie_import(
    app: tauri::AppHandle,
    profile_id: String,
    source_id: String,
    domains: Vec<String>,
) -> crate::error::Result<crate::browser_cookies::CookieImportReport> {
    #[cfg(target_os = "macos")]
    {
        crate::commands::native_task(move || {
            platform::import_cookies(&app, &profile_id, &source_id, &domains)
        })
        .await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, profile_id, source_id, domains);
        Err(crate::error::Error::new(
            "unsupported",
            "Importing cookies is only available on macOS.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_urls_keep_parameter_names_only() {
        assert_eq!(
            redact_url("https://user:pw@api.example.com/v1/items?token=abc&page=2#frag"),
            "https://api.example.com/v1/items?token=…&page=…"
        );
        assert_eq!(redact_url("https://example.com/"), "https://example.com/");
        assert_eq!(
            redact_url("https://example.com/a?"),
            "https://example.com/a"
        );
        assert_eq!(
            redact_url("data:image/png;base64,xyz"),
            "data:image/png;base64,xyz"
        );
        assert!(
            redact_url(&format!("https://example.com/{}", "a".repeat(1000)))
                .chars()
                .count()
                <= 400
        );
        let raw = json!({ "entries": [
            { "kind": "fetch", "method": "POST", "url": "https://x.dev/login?password=hunter2", "status": 401, "durationMs": 12.6 },
            { "kind": "script", "url": "https://x.dev/app.js", "status": null, "durationMs": 3.0, "size": 120 },
            { "kind": "xhr" }
        ] });
        let entries = network_entries(&raw, 50);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["url"], "https://x.dev/login?password=…");
        assert_eq!(entries[0]["status"], 401);
        assert_eq!(entries[0]["durationMs"], 13.0);
        assert_eq!(entries[1]["bytes"], 120);
        assert_eq!(network_entries(&raw, 1).len(), 0);
    }

    #[test]
    fn drag_ends_take_refs_selectors_or_points() {
        assert_eq!(
            parse_drag_end(Some(&json!("#card")), "from").unwrap(),
            DragEnd::Element {
                reference: String::new(),
                selector: "#card".into()
            }
        );
        assert_eq!(
            parse_drag_end(Some(&json!({ "ref": "e12" })), "from").unwrap(),
            DragEnd::Element {
                reference: "e12".into(),
                selector: String::new()
            }
        );
        assert_eq!(
            parse_drag_end(Some(&json!({ "x": 10, "y": 20.5 })), "to").unwrap(),
            DragEnd::Point { x: 10.0, y: 20.5 }
        );
        assert!(parse_drag_end(None, "to").is_err());
        assert!(parse_drag_end(Some(&json!({ "x": -1, "y": 2 })), "to").is_err());
        assert!(parse_drag_end(Some(&json!({ "ref": "x1" })), "to").is_err());
        assert!(parse_drag_end(Some(&json!({})), "to").is_err());
        assert!(parse_drag_end(Some(&json!(42)), "to").is_err());
        assert!(parse_drag_end(Some(&json!("a\nb")), "to").is_err());
    }

    #[test]
    fn select_values_resize_scheme_and_port_are_bounded() {
        assert_eq!(
            select_values(&json!({ "values": "us" })).unwrap(),
            vec!["us"]
        );
        assert_eq!(
            select_values(&json!({ "values": ["a", "b"] }))
                .unwrap()
                .len(),
            2
        );
        assert!(select_values(&json!({ "values": [] })).is_err());
        assert!(select_values(&json!({ "values": [1] })).is_err());
        assert!(select_values(&json!({})).is_err());
        assert!(select_values(&json!({ "values": vec!["a"; 21] })).is_err());
        assert_eq!(
            resize_size(&json!({ "width": 390, "height": 844 })).unwrap(),
            (390, 844)
        );
        assert!(resize_size(&json!({ "width": 100, "height": 844 })).is_err());
        assert!(resize_size(&json!({ "width": 390 })).is_err());
        assert_eq!(color_scheme(&json!({ "scheme": "dark" })).unwrap(), "dark");
        assert_eq!(
            color_scheme(&json!({ "scheme": "system" })).unwrap(),
            "system"
        );
        assert!(color_scheme(&json!({ "scheme": "sepia" })).is_err());
        assert_eq!(
            localhost_url(&json!(5173)).unwrap(),
            "http://localhost:5173"
        );
        assert!(localhost_url(&json!(0)).is_err());
        assert!(localhost_url(&json!(70000)).is_err());
        assert!(localhost_url(&json!("5173")).is_err());
    }

    #[test]
    fn evaluate_wraps_expressions_and_bounds_results() {
        let body = evaluate_body("document.title", false);
        assert!(body.contains("return (\ndocument.title\n);"));
        assert!(body.contains(&EVALUATE_MAX_RESULT.to_string()));
        let statements = evaluate_body("const a = 1; return a;", true);
        assert!(statements.contains("const a = 1; return a;"));
        assert!(!statements.contains("return (\nconst"));
    }

    #[test]
    fn uploads_stay_inside_the_workspace_or_attachments() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("a.png"), b"png").unwrap();
        std::fs::create_dir(workspace.path().join("dir")).unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"no").unwrap();
        std::fs::write(outside.path().join("attached.pdf"), b"pdf").unwrap();
        let attached = [outside.path().join("attached.pdf")];
        let ok =
            validate_upload_paths(&["a.png".into()], Some(workspace.path()), &attached).unwrap();
        assert_eq!(ok.len(), 1);
        let absolute = outside
            .path()
            .join("attached.pdf")
            .to_string_lossy()
            .to_string();
        assert!(validate_upload_paths(&[absolute], Some(workspace.path()), &attached).is_ok());
        let secret = outside
            .path()
            .join("secret.txt")
            .to_string_lossy()
            .to_string();
        assert!(validate_upload_paths(&[secret], Some(workspace.path()), &attached).is_err());
        assert!(validate_upload_paths(&["../x".into()], Some(workspace.path()), &[]).is_err());
        assert!(validate_upload_paths(&["dir".into()], Some(workspace.path()), &[]).is_err());
        assert!(validate_upload_paths(&[], Some(workspace.path()), &[]).is_err());
        assert!(
            validate_upload_paths(&vec!["a.png".into(); 6], Some(workspace.path()), &[]).is_err()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                outside.path().join("secret.txt"),
                workspace.path().join("link.txt"),
            )
            .unwrap();
            assert!(
                validate_upload_paths(&["link.txt".into()], Some(workspace.path()), &[]).is_err()
            );
        }
        assert_eq!(upload_mime(Path::new("a.PNG")), "image/png");
        assert_eq!(upload_mime(Path::new("a.bin")), "application/octet-stream");
    }

    #[test]
    fn dialog_state_serialises_without_empty_fields() {
        let dialog = BrowserDialog {
            tab_id: "tab-1".into(),
            kind: "confirm".into(),
            message: "Delete?".into(),
            default_text: None,
            url: "https://example.com/".into(),
        };
        let value = serde_json::to_value(&dialog).unwrap();
        assert_eq!(value["tabId"], "tab-1");
        assert!(value.get("defaultText").is_none());
    }
}
