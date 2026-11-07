//! The app-hosted MCP servers over HTTP on loopback: one listener in the host instead of two
//! `--mcp-browser` and `--mcp-computer` children for every Claude conversation. Same protocol
//! (`mcp_stdio::answer`), same bridges and same per-session tokens; a call reaches the app over
//! the private socket the children use. Providers that launch a child keep doing so.
#![cfg(unix)]

use axum::extract::Path;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::sync::OnceLock;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

static PORT: OnceLock<u16> = OnceLock::new();
/// A tool may wait on the person for a long time; `sirus_delegate` waits on another agent.
pub const TOOL_TIMEOUT_MS: u64 = 65 * 60 * 1000;

pub fn start() {
    crate::runtime::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(%error, "mcp http: cannot bind");
                return;
            }
        };
        let Ok(address) = listener.local_addr() else {
            return;
        };
        let _ = PORT.set(address.port());
        let router = Router::new().route("/{server}", post(serve));
        if let Err(error) = axum::serve(listener, router).await {
            tracing::warn!(%error, "mcp http stopped");
        }
    });
}

/// The `mcpServers` entry for a server, once the listener is up.
pub fn claude_entry(server: &str, token: &str) -> Option<Value> {
    let port = PORT.get()?;
    Some(json!({
        "type": "http",
        "url": format!("http://127.0.0.1:{port}/{server}"),
        "headers": { "Authorization": format!("Bearer {token}") },
        "timeout": TOOL_TIMEOUT_MS,
    }))
}

struct Route {
    server: crate::mcp_stdio::Server,
    socket: std::path::PathBuf,
    known: fn(&str) -> bool,
}

fn route(server: &str) -> Option<Route> {
    match server {
        crate::browser_mcp::MCP_SERVER_NAME => Some(Route {
            server: crate::browser_mcp::http_server(),
            socket: crate::browser_mcp::socket_path()?,
            known: crate::browser_mcp::token_known,
        }),
        crate::computer_mcp::SERVER_NAME => Some(Route {
            server: crate::computer_mcp::http_server(),
            socket: crate::computer_mcp::socket_path()?,
            known: crate::computer_mcp::token_known,
        }),
        _ => None,
    }
}

/// The bearer token of a request. A browser page can reach loopback, and only it sends `Origin`.
fn token_of(headers: &HeaderMap) -> Option<&str> {
    if headers.contains_key(header::ORIGIN) {
        return None;
    }
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn serve(
    Path(server): Path<String>,
    headers: HeaderMap,
    Json(message): Json<Value>,
) -> Response {
    let Some(route) = route(&server) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(token) = token_of(&headers).filter(|token| (route.known)(token)) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match crate::mcp_stdio::answer(&route.server, &message) {
        crate::mcp_stdio::Answer::Reply(Some(reply)) => Json(reply).into_response(),
        crate::mcp_stdio::Answer::Reply(None) => StatusCode::ACCEPTED.into_response(),
        crate::mcp_stdio::Answer::Call {
            id,
            tool,
            arguments,
        } => {
            let response = forward(&route.socket, token, &tool, arguments)
                .await
                .unwrap_or(Value::Null);
            match id {
                Some(id) => {
                    Json(crate::mcp_stdio::call_reply(&route.server, id, &response)).into_response()
                }
                None => StatusCode::ACCEPTED.into_response(),
            }
        }
    }
}

/// One tool call over the bridge's socket, as a child would send it.
async fn forward(
    socket: &std::path::Path,
    token: &str,
    tool: &str,
    arguments: Value,
) -> std::io::Result<Value> {
    let stream = tokio::net::UnixStream::connect(socket).await?;
    let (read, mut write) = stream.into_split();
    let request = json!({ "id": 1, "token": token, "tool": tool, "arguments": arguments });
    write.write_all(format!("{request}\n").as_bytes()).await?;
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await?;
    Ok(serde_json::from_str(&line).unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_cannot_use_the_route() {
        let mut headers = HeaderMap::new();
        assert_eq!(token_of(&headers), None);
        headers.insert(header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        assert_eq!(token_of(&headers), Some("abc"));
        headers.insert(header::ORIGIN, "https://example.com".parse().unwrap());
        assert_eq!(token_of(&headers), None);
    }

    #[tokio::test]
    async fn a_call_goes_over_the_socket_with_the_token() {
        // `/tmp`: a socket path has a short limit.
        let dir = std::path::PathBuf::from(format!(
            "/tmp/mh-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bridge.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            BufReader::new(read).read_line(&mut line).await.unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            let reply = json!({ "ok": true, "result": { "saw": request["token"], "tool": request["tool"] } });
            write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        });
        let response = forward(&path, "secret", "browser_open", json!({}))
            .await
            .unwrap();
        assert_eq!(response["result"]["saw"], "secret");
        assert_eq!(response["result"]["tool"], "browser_open");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
