//! Inline HTML visuals (ADR-123): `sirus_show_visual` lets an agent publish a chart, a
//! small dashboard or a diagram into its own running reply. The HTML is kept with the
//! message (persisted like the rest of the transcript) and the transcript renders it as a
//! card in the sandboxed `sirus-preview://` frame (`html_preview.rs`, `InlineVisual.tsx`).
//! The tool only ever writes to the calling session's current reply.

use std::sync::Arc;

use crate::runtime::{AppHandle, Manager};
use serde_json::{json, Value};

use crate::commands::AppState;
use crate::models::{MessageRole, MessageVisual, Session};

pub const TOOL: &str = "sirus_show_visual";
/// The HTML of one visual, in bytes.
pub const MAX_HTML_BYTES: usize = 512 * 1024;
pub const MAX_TITLE_CHARS: usize = 120;
/// Distinct visuals one reply may hold (revisions replace in place).
pub const MAX_PER_REPLY: usize = 8;

/// One line appended to Claude's system prompt while the Sirus bridge is attached.
pub const CLAUDE_HINT: &str = "Sirus Code shows visuals in the conversation. When an answer compares options, reports numbers (benchmarks, timings, sizes, costs, before and after), lays out a plan in phases, or explains a flow or an architecture, call the mcp__sirus_browser__sirus_show_visual tool with one compact, themed HTML card instead of a long Markdown table, and keep the reply text to the finding.";

pub fn tool_definition() -> Value {
    json!({
        "name": TOOL,
        "description": "Sirus Code: show a visual inside your reply in the person's conversation: a chart, a small dashboard, a table with highlights, a diagram or a timeline. Use it whenever the answer compares options, reports numbers (benchmarks, timings, sizes, costs, before and after), lays out a plan in phases or explains a flow or an architecture: it reads faster than text and replaces long Markdown tables. Send one self-contained HTML document (inline <style> and <script>, at most 512 KiB). It renders as a card under your reply text, sized to its content (taller pages scroll inside). It runs in an isolated sandbox: no network requests (fetch, XHR and WebSockets are blocked), no forms, popups, dialogs or nested frames, and no storage that survives; https images, fonts, stylesheets and scripts (for example a chart library from a CDN) do load, but prefer inline SVG or canvas. Link clicks open http(s) URLs in the person's browser. The page follows the app theme: use the CSS variables --sirus-text, --sirus-text-secondary, --sirus-muted, --sirus-background, --sirus-surface, --sirus-border, --sirus-accent, --sirus-info, --sirus-success, --sirus-warning, --sirus-danger, --sirus-series-1 to --sirus-series-6 (chart colors), --sirus-font, --sirus-mono, --sirus-font-size and --sirus-radius; html has data-theme light or dark and a sirus-theme event fires on window when it changes (redraw canvas charts then). Leave the page background transparent, let content set the height (no 100vh) and keep it compact (the column is about 700 px wide). Calling it again with the same title in the same turn replaces that visual. Keep your reply text short: state the finding, do not describe the visual. At most 8 visuals per reply.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "A short name shown on the card (at most 120 characters); reuse it to update the visual." },
                "html": { "type": "string", "description": "The complete, self-contained HTML document." }
            },
            "required": ["title", "html"]
        }
    })
}

/// The title on one line, without control characters; the HTML within its size.
pub fn validate(title: &str, html: &str) -> Result<(String, String), String> {
    let title = title
        .split(|c: char| c.is_control() || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        return Err("a title is required".into());
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!(
            "the title must be at most {MAX_TITLE_CHARS} characters"
        ));
    }
    if html.trim().is_empty() {
        return Err("the html must be a non-empty HTML document".into());
    }
    if html.len() > MAX_HTML_BYTES {
        return Err(format!(
            "the page is {} KiB; the limit is {} KiB: remove embedded data or simplify it",
            html.len().div_ceil(1024),
            MAX_HTML_BYTES / 1024
        ));
    }
    Ok((title, html.to_owned()))
}

/// The reply of the session's current turn: the agent message after its latest user message.
fn current_reply(session: &Session) -> Option<usize> {
    let messages = &session.messages;
    let user = messages
        .iter()
        .rposition(|message| message.role == MessageRole::User)?;
    messages
        .iter()
        .enumerate()
        .skip(user + 1)
        .rev()
        .find(|(_, message)| message.role == MessageRole::Agent)
        .map(|(index, _)| index)
}

/// Adds the visual to the current reply, or replaces the one with the same title.
/// Returns the reply's index and the visual's id and revision.
#[cfg(test)]
pub fn publish(
    session: &mut Session,
    title: String,
    html: String,
    now: &str,
) -> Result<(usize, String, u32), String> {
    publish_with(session, title, html, None, now)
}

/// `publish`, with the JavaScript program of a native visual (`visual_show`).
pub fn publish_with(
    session: &mut Session,
    title: String,
    html: String,
    source: Option<String>,
    now: &str,
) -> Result<(usize, String, u32), String> {
    let index =
        current_reply(session).ok_or("there is no reply in progress to show the visual in")?;
    let reply = &mut session.messages[index];
    if let Some(visual) = reply
        .visuals
        .iter_mut()
        .find(|visual| visual.title == title)
    {
        visual.html = html;
        visual.source = source;
        visual.revision = visual.revision.saturating_add(1);
        visual.created_at = now.to_owned();
        return Ok((index, visual.id.clone(), visual.revision));
    }
    if reply.visuals.len() >= MAX_PER_REPLY {
        return Err(format!(
            "at most {MAX_PER_REPLY} visuals per reply; reuse a title to update one"
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    reply.visuals.push(MessageVisual {
        id: id.clone(),
        title,
        html,
        source,
        created_at: now.to_owned(),
        revision: 0,
    });
    Ok((index, id, 0))
}

pub fn execute(app: &AppHandle, session_id: &str, args: &Value) -> Result<Value, String> {
    let text = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or_default();
    let (title, html) = validate(text("title"), text("html"))?;
    store(app, session_id, title, html, None)
}

/// Saves the visual in the session's current reply and tells the transcript.
fn store(
    app: &AppHandle,
    session_id: &str,
    title: String,
    html: String,
    source: Option<String>,
) -> Result<Value, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let now = chrono::Local::now().to_rfc3339();
    let (snapshot, index, id, revision) = {
        let mut data = state.data_with_messages(&[session_id]);
        let session = data
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            .ok_or("session not found")?;
        let (index, id, revision) = publish_with(session, title.clone(), html, source, &now)?;
        (session.clone(), index, id, revision)
    };
    state.persist().map_err(|error| error.to_string())?;
    crate::transcript_view::emit_from(app, &snapshot, index);
    Ok(json!({
        "shown": true,
        "visualId": id,
        "title": title,
        "updated": revision > 0,
        "hint": "The person sees the visual under your reply; do not describe it again."
    }))
}

// ---- Native visuals (Sirus Native): JavaScript programs drawn by GPUI, ported from Monocode (MIT).

/// The JavaScript engine for `visual_check`; only the headless host links it, so the desktop
/// build stays untouched. The program files are shared with the native app.
#[cfg(feature = "host")]
#[path = "../../native-spike/src/visuals/program/mod.rs"]
mod program;

pub const NATIVE_TOOLS: [&str; 3] = ["visual_docs", "visual_check", "visual_show"];
const MAX_SOURCE_BYTES: usize = 64 * 1024;

/// Shown by the web app in place of a native visual, which only Sirus Native draws.
const NATIVE_PLACEHOLDER: &str = "<p style=\"font:13px system-ui;opacity:.6;margin:0\">This visual is interactive and is drawn by Sirus Native.</p>";

const DOCUMENTATION: &str = r#"Native session visuals
Use visual_check to validate a visual, then visual_show to publish it into this
conversation (under your reply). Both accept {source}; visual_show also takes {title}.
The source is a JavaScript function body with a `visual` SDK. Always return a render function. Use closure state for interactions. Example:
const state = visual.state({count: 0});
return () => visual.column([
  visual.text('Counter', {size: 'title'}),
  visual.stat('Count', String(state.count)),
  visual.button('Add', () => { state.count += 1; })
], {gap: 12});

Publish one small visual per idea (a single chart, a stat row, a gauge) rather than one
dashboard holding everything. Calling visual_show again with the same title replaces it.

Native components:
visual.column(children,{gap?}), row(children,{gap?}), grid(children,{columns?,gap?})
visual.card(children,{gap?}) groups a chart or stats on a raised panel.
visual.text(text,{size?,color?})
visual.stat(label,value,{detail?,delta?,trend?,better?,color?}): delta is a signed
change such as '+12.4%'; trend is an array of numbers drawn as a sparkline;
better:'lower' colors a falling delta as good (latency, errors, cost).
visual.table(columns,rows)
visual.chart('bar'|'line'|'area',[{label,values,color?}],{labels?,height?})
A single-series bar chart labels its latest bar; give each series its own color.
visual.gauge(label,value,{min?,max?,unit?,color?}) for a reading against a range.
visual.donut([{label,value,color?}],{label?,value?}) for parts of a whole;
label and value are written in the middle (default: the total).
visual.heatmap(rows,{rows?,columns?,color?}): rows is an array of equal-length
number arrays, such as weekdays by weeks; labels are optional.
visual.progress(label,value,{max?,detail?,color?})
visual.button(label,onClick,{disabled?}), toggle(label,value,onChange)
visual.slider(label,value,min,max,onChange,{step?}), tabs(labels,selected,onChange)
visual.input(label,value,onChange,{placeholder?}), select(label,options,selectedIndex,onChange)
Input handlers receive a string (up to 4096 bytes); select handlers receive an index.
Select and input labels are not shown: make options and placeholders self-explanatory.
visual.canvas(shapes,{width?,height?})
Canvas shapes: visual.rect(x,y,w,h,{color?}), circle(x,y,r,{color?}),
line(x1,y1,x2,y2,{color?,width?}), polyline([[x,y],...],{color?,width?}),
label(x,y,text,{color?}).
visual.scene(objects,{width?,height?,camera?})
Scene objects: visual.object({kind:'box'|'sphere'|'cylinder'|'cone'|'plane'},
{position:[x,y,z],rotation:[x,y,z],scale:[x,y,z],color:'accent'}).
Custom geometry: {kind:'mesh',vertices:[[x,y,z],...],indices:[[a,b,c],...]}
or {kind:'line',points:[[x,y,z],...],width:1}. Rotations are radians.
Camera: {eye:[5,4,6],target:[0,0,0],fov:45}. Use controls and state to change
geometry or camera; every handler rerenders. State is local to this visual.

Style is inherited from the app. Use semantic colors only: ink, muted, faint,
accent, positive, negative, warning, series1 through series6. Text sizes are
caption, small, body, subtitle, title, hero. Choose clear labels and restrained
headings, use row/column/grid layouts, and allow the host width to determine
layout. Use gaps around 8-16 and chart/canvas/scene heights around 220-360.
Do not draw custom buttons, simulate a whole webpage, add a page background, or
hardcode font families or CSS. Native controls and the app theme own those styles.
Include units, chart legends, readable labels, and useful empty states. Keep
supporting prose in the conversation.
The sandbox has no DOM, network, files, timers, imports, or external libraries.
No HTML, SVG markup, or shader code. Source is limited to 64 KiB; keep scripts compact.
Each call may run for 60 ms and the interpreter has 16 MiB. visual_check validates execution,
the initial component tree and representative controls; it does not capture a screenshot.
Fix errors before publishing. Set a concise title for the published visual.
"#;

pub fn native_tool_definitions() -> Vec<Value> {
    let source = json!({ "type": "string", "description": "JavaScript function body using the native visual SDK" });
    vec![
        json!({ "name": "visual_docs", "description": "Sirus Code: read the native visual SDK reference, examples and styling rules before building an inline interactive chart, table, calculator, diagram or 3D explainer. Does not publish. Native visuals are JavaScript programs drawn by Sirus Native (no browser); prefer them to sirus_show_visual when the person uses Sirus Native.", "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false } }),
        json!({ "name": "visual_check", "description": "Sirus Code: validate a native visual program and its representative controls without publishing. Read visual_docs for the SDK reference.", "inputSchema": { "type": "object", "properties": { "source": source }, "required": ["source"], "additionalProperties": false } }),
        json!({ "name": "visual_show", "description": "Sirus Code: publish a native visual (chart, stat row, table, calculator, diagram, interactive explainer) under your reply in the conversation, drawn natively and interactive after the turn ends. Use it for data with a trend, comparison or breakdown, and whenever the person asks for a visual. Read visual_docs first and validate with visual_check. Calling it again with the same title in the same turn replaces that visual. At most 8 visuals per reply; keep your reply text short and do not describe the visual.", "inputSchema": { "type": "object", "properties": { "source": source, "title": { "type": "string", "maxLength": 120, "description": "A short name shown on the card; reuse it to update the visual." } }, "required": ["source", "title"], "additionalProperties": false } }),
    ]
}

/// Runs the program and its sample controls with the real interpreter in the host; the desktop
/// build has no JavaScript engine, so it only checks the source's size and shape there.
fn check_source(source: &str) -> Result<Value, String> {
    if source.trim().is_empty() {
        return Err("the source must be a non-empty JavaScript function body".into());
    }
    if source.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "the source is {} KiB; the limit is {} KiB",
            source.len().div_ceil(1024),
            MAX_SOURCE_BYTES / 1024
        ));
    }
    #[cfg(feature = "host")]
    {
        let tree = program::check(source)?;
        Ok(json!({ "ok": true, "engine": true, "tree": tree }))
    }
    #[cfg(not(feature = "host"))]
    {
        if !source.contains("return") || !source.contains("visual.") {
            return Err(
                "the program must use the `visual` SDK and return a render function".into(),
            );
        }
        Ok(
            json!({ "ok": true, "engine": false, "note": "structural check only: this build has no JavaScript engine, so the program was not executed" }),
        )
    }
}

pub fn execute_native(
    app: &AppHandle,
    session_id: &str,
    tool: &str,
    args: &Value,
) -> Result<Value, String> {
    let text = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or_default();
    match tool {
        "visual_docs" => Ok(json!({ "documentation": DOCUMENTATION })),
        "visual_check" => check_source(text("source")),
        _ => {
            let source = text("source");
            check_source(source)?;
            let (title, _) = validate(text("title"), NATIVE_PLACEHOLDER)?;
            let mut shown = store(
                app,
                session_id,
                title,
                NATIVE_PLACEHOLDER.to_owned(),
                Some(source.to_owned()),
            )?;
            shown["hint"] =
                json!("The person sees the visual under your reply; do not describe it again.");
            Ok(shown)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(roles: &[&str]) -> Session {
        let messages: Vec<Value> = roles
            .iter()
            .enumerate()
            .map(|(index, role)| json!({ "id": format!("m{index}"), "sessionId": "s", "role": role, "content": "", "createdAt": "t", "streaming": false }))
            .collect();
        serde_json::from_value(json!({
            "id": "s", "title": "s", "projectId": "p", "agent": "claude", "status": "running",
            "createdAt": "t", "lastActivityAt": "t", "worktree": { "path": "/tmp", "branch": "main", "isolated": false },
            "messages": messages
        }))
        .unwrap()
    }

    #[cfg(feature = "host")]
    #[test]
    fn native_programs_are_run_by_the_check() {
        let ok = "const s = visual.state({n: 1}); return () => visual.column([visual.stat('N', String(s.n)), visual.slider('Q', s.n, 0, 9, v => { s.n = v; })]);";
        assert_eq!(check_source(ok).unwrap()["engine"], true);
        assert!(check_source("return () => visual.nope()").is_err());
        assert!(check_source(" ").is_err());
    }

    #[test]
    fn titles_and_sizes_are_validated() {
        assert_eq!(
            validate("  Build\ntimes \u{7}  ", "<p>x</p>").unwrap().0,
            "Build times"
        );
        assert!(validate(" \n ", "<p>x</p>").is_err());
        assert!(validate(&"t".repeat(MAX_TITLE_CHARS + 1), "<p>x</p>").is_err());
        assert!(validate("Chart", "   ").is_err());
        assert!(validate("Chart", &"x".repeat(MAX_HTML_BYTES)).is_ok());
        let error = validate("Chart", &"x".repeat(MAX_HTML_BYTES + 1)).unwrap_err();
        assert!(error.contains("513 KiB"), "{error}");
    }

    #[test]
    fn visuals_go_to_the_current_reply_and_a_same_title_replaces() {
        let mut session = session(&["user", "agent", "user", "agent"]);
        let (index, id, revision) =
            publish(&mut session, "Chart".into(), "<p>1</p>".into(), "a").unwrap();
        assert_eq!((index, revision), (3, 0));
        let (_, same, revision) =
            publish(&mut session, "Chart".into(), "<p>2</p>".into(), "b").unwrap();
        assert_eq!((same.as_str(), revision), (id.as_str(), 1));
        let reply = &session.messages[3];
        assert_eq!(reply.visuals.len(), 1);
        assert_eq!(reply.visuals[0].html, "<p>2</p>");
        assert!(session.messages[1].visuals.is_empty());
        for index in 1..MAX_PER_REPLY {
            publish(&mut session, format!("Other {index}"), "x".into(), "c").unwrap();
        }
        assert!(publish(&mut session, "One more".into(), "x".into(), "d").is_err());
        // Revisions of an existing visual still go through at the bound.
        assert!(publish(&mut session, "Chart".into(), "<p>3</p>".into(), "e").is_ok());
    }

    #[test]
    fn a_turn_without_its_reply_refuses() {
        assert!(publish(
            &mut session(&["user", "agent", "user"]),
            "C".into(),
            "x".into(),
            "t"
        )
        .is_err());
        assert!(publish(&mut session(&[]), "C".into(), "x".into(), "t").is_err());
    }

    #[test]
    fn messages_round_trip_their_visuals_and_old_ones_load_without() {
        let mut session = session(&["user", "agent"]);
        publish(&mut session, "Chart".into(), "<svg/>".into(), "t").unwrap();
        let encoded = serde_json::to_value(&session.messages[1]).unwrap();
        assert_eq!(encoded["visuals"][0]["title"], "Chart");
        assert_eq!(encoded["visuals"][0]["createdAt"], "t");
        let decoded: crate::models::Message = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.visuals, session.messages[1].visuals);
        // Rows written before visuals existed, and replies without any, carry no field.
        assert!(serde_json::to_value(&session.messages[0])
            .unwrap()
            .get("visuals")
            .is_none());
        let old: crate::models::Message = serde_json::from_value(json!({ "id": "m", "sessionId": "s", "role": "agent", "content": "hi", "createdAt": "t", "streaming": false })).unwrap();
        assert!(old.visuals.is_empty());
        let no_revision: MessageVisual = serde_json::from_value(
            json!({ "id": "v", "title": "T", "html": "<p/>", "createdAt": "t" }),
        )
        .unwrap();
        assert_eq!(no_revision.revision, 0);
    }
}
