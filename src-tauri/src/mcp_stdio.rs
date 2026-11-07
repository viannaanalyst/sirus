//! Child mode shared by the app-hosted MCP servers (browser, computer): MCP over
//! stdio, every tool call forwarded to the app over a private Unix socket with
//! the per-session token from the environment.

use serde_json::{json, Value};

pub struct Server {
    pub name: &'static str,
    pub socket_env: &'static str,
    pub token_env: &'static str,
    pub tools: fn() -> Vec<Value>,
    pub failure: &'static str,
    /// Guidance the agent's CLI adds to its system prompt (MCP `instructions`).
    pub instructions: Option<&'static str>,
}

fn text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}

/// Tool results may carry one image as `{ "image": { "data", "mimeType" } }`;
/// it becomes an MCP image block next to the remaining structured text.
pub fn tool_content(result: &Value) -> Value {
    let image = result.get("image").and_then(|image| {
        Some((
            image.get("data")?.as_str()?.to_string(),
            image.get("mimeType")?.as_str()?.to_string(),
        ))
    });
    let Some((data, mime)) = image else {
        // MCP clients reject `structuredContent` that is not an object (a log
        // list, an evaluated string): those results travel as text only.
        if !result.is_object() {
            return json!({ "content": [ { "type": "text", "text": text(result) } ] });
        }
        return json!({ "content": [ { "type": "text", "text": text(result) } ], "structuredContent": result });
    };
    let mut rest = result.clone();
    if let Some(object) = rest.as_object_mut() {
        object.remove("image");
    }
    let mut content = vec![json!({ "type": "image", "data": data, "mimeType": mime })];
    if rest.as_object().is_some_and(|object| !object.is_empty()) {
        content.push(json!({ "type": "text", "text": text(&rest) }));
    }
    json!({ "content": content })
}

/// What a JSON-RPC message from the agent's CLI asks of a server.
pub enum Answer {
    /// The reply, if the message wants one.
    Reply(Option<Value>),
    /// A tool call, for the app.
    Call {
        id: Option<Value>,
        tool: String,
        arguments: Value,
    },
}

/// The protocol, shared by the stdio child and the host's HTTP route (`mcp_http.rs`).
pub fn answer(server: &Server, message: &Value) -> Answer {
    let id = message.get("id").cloned();
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let respond = |value: Value| {
        id.as_ref()
            .map(|id| json!({ "jsonrpc": "2.0", "id": id, "result": value }))
    };
    match method {
        "initialize" => {
            let requested = message
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-06-18")
                .to_string();
            let mut result = json!({
                "protocolVersion": requested,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": server.name, "version": "0.1.0" }
            });
            if let Some(instructions) = server.instructions {
                result["instructions"] = json!(instructions);
            }
            Answer::Reply(respond(result))
        }
        "ping" => Answer::Reply(respond(json!({}))),
        "tools/list" => Answer::Reply(respond(json!({ "tools": (server.tools)() }))),
        "tools/call" => Answer::Call {
            tool: message
                .pointer("/params/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            arguments: message
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({})),
            id,
        },
        method if method.starts_with("notifications/") => Answer::Reply(None),
        _ => Answer::Reply(id.as_ref().map(|id| {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "Method not found" } })
        })),
    }
}

/// The bridge's response to a tool call, as the JSON-RPC reply.
pub fn call_reply(server: &Server, id: Value, response: &Value) -> Value {
    let result = if response.get("ok").and_then(Value::as_bool) == Some(true) {
        tool_content(response.get("result").unwrap_or(&Value::Null))
    } else {
        let error = response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(server.failure);
        json!({ "content": [ { "type": "text", "text": error } ], "isError": true })
    };
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn run(server: &Server) -> i32 {
    #[cfg(not(unix))]
    {
        let _ = server;
        1
    }
    #[cfg(unix)]
    {
        use std::io::{BufRead, BufReader, Write};
        let (Ok(socket), Ok(token)) = (
            std::env::var(server.socket_env),
            std::env::var(server.token_env),
        ) else {
            return 1;
        };
        let Ok(stream) = std::os::unix::net::UnixStream::connect(&socket) else {
            return 1;
        };
        let Ok(mut writer) = stream.try_clone() else {
            return 1;
        };
        let mut bridge = BufReader::new(stream);
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();
        let mut counter = 0u64;
        let emit = |payload: Value, out: &mut std::io::Stdout| {
            let _ = writeln!(out, "{payload}");
            let _ = out.flush();
        };
        for line in BufReader::new(stdin.lock()).lines() {
            let Ok(line) = line else {
                break;
            };
            // Room for a 512 KiB inline visual after JSON escaping (ADR-123).
            if line.len() > 4 * 1024 * 1024 {
                break;
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match answer(server, &message) {
                Answer::Reply(Some(payload)) => emit(payload, &mut stdout),
                Answer::Reply(None) => {}
                Answer::Call {
                    id,
                    tool,
                    arguments,
                } => {
                    counter += 1;
                    let request = json!({ "id": counter, "token": token, "tool": tool, "arguments": arguments });
                    let mut response = Value::Null;
                    if writeln!(writer, "{request}").is_ok() && writer.flush().is_ok() {
                        let mut response_line = String::new();
                        if bridge.read_line(&mut response_line).is_ok() {
                            response = serde_json::from_str(&response_line).unwrap_or(Value::Null);
                        }
                    }
                    if let Some(id) = id {
                        emit(call_reply(server, id, &response), &mut stdout);
                    }
                }
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_results_become_image_blocks_with_remaining_text() {
        let plain = tool_content(&json!({ "ok": true }));
        assert_eq!(plain["content"][0]["type"], "text");
        let image = tool_content(
            &json!({ "image": { "data": "AAAA", "mimeType": "image/jpeg" }, "window": "Calculator" }),
        );
        assert_eq!(image["content"][0]["type"], "image");
        assert_eq!(image["content"][0]["mimeType"], "image/jpeg");
        assert!(image["content"][1]["text"]
            .as_str()
            .unwrap()
            .contains("Calculator"));
        let only = tool_content(&json!({ "image": { "data": "AAAA", "mimeType": "image/png" } }));
        assert_eq!(only["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn only_object_results_carry_structured_content() {
        assert!(tool_content(&json!({ "ok": true }))
            .get("structuredContent")
            .is_some());
        let list = tool_content(&json!([{ "level": "log", "text": "hi" }]));
        assert!(list.get("structuredContent").is_none());
        assert!(list["content"][0]["text"].as_str().unwrap().contains("hi"));
        assert!(tool_content(&json!("done"))
            .get("structuredContent")
            .is_none());
    }
}
