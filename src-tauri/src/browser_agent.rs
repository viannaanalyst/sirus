//! More browser tools for agents (ADR-150): hover, select, drag, upload, JavaScript dialogs,
//! evaluate, the network log, resize and colour scheme. Arguments are checked here before
//! anything reaches a page; the page scripts are fixed and receive their input as JSON
//! arguments (never spliced into source), except `browser_evaluate`, whose whole point is
//! to run the agent's expression (behind Agent browser access, like every browser tool).

#![allow(dead_code)]

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
