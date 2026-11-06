//! Local development servers (ADR-151).
//!
//! Listing: the listening TCP sockets come from one fixed `lsof` probe, with each
//! listener's working folder and the process table, cached for a few seconds. A
//! listener is shown for a session when its process works inside the session's
//! worktree or project folder (whoever started it), or when it holds a typical
//! development port and is not a macOS service. Sirus's own listeners and the agent
//! CLIs are never listed.
//!
//! Stopping: only a process that Sirus itself started (a terminal shell's
//! child or an agent's tool) can be stopped: the listener of a localhost port is
//! found with the same probe, its parent chain must reach this app's process, and it
//! gets SIGTERM. Servers started outside the app are never touched.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::runtime::State;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::commands::AppState;
use crate::error::{Error, Result};

const LSOF: &str = "/usr/sbin/lsof";
const PS: &str = "/bin/ps";
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_ANCESTRY: usize = 64;

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    Stop { session_id: String, url: String },
}

/// A listening server shown in the browser's empty state and the Environment card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalServer {
    pub url: String,
    pub port: u16,
    pub pid: u32,
    pub process: String,
    /// Started by Sirus (descends from this app's process): it can be stopped.
    pub owned: bool,
    /// Its process works inside the session's worktree or project folder.
    pub in_workspace: bool,
}

/// One listening socket from `lsof -F pcn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub pid: u32,
    pub command: String,
    pub port: u16,
}

/// Parses `lsof -nP -iTCP -sTCP:LISTEN -F pcn`: `p` starts a process, `c` names it,
/// each `n` is `host:port`. Duplicates (IPv4 and IPv6 of one port) collapse.
pub fn parse_listeners(text: &str) -> Vec<Listener> {
    let mut found: Vec<Listener> = Vec::new();
    let (mut pid, mut command) = (None::<u32>, String::new());
    for line in text.lines() {
        let Some(tag) = line.chars().next() else {
            continue;
        };
        let value = &line[tag.len_utf8()..];
        match tag {
            'p' => {
                pid = value.parse().ok();
                command.clear();
            }
            'c' => command = value.chars().take(64).collect(),
            'n' => {
                let Some(pid) = pid else { continue };
                let Some(port) = value
                    .rsplit_once(':')
                    .and_then(|(_, port)| port.parse::<u16>().ok())
                    .filter(|port| *port > 0)
                else {
                    continue;
                };
                if !found
                    .iter()
                    .any(|item| item.pid == pid && item.port == port)
                {
                    found.push(Listener {
                        pid,
                        command: command.clone(),
                        port,
                    });
                }
            }
            _ => {}
        }
    }
    found
}

/// Parses `lsof -a -d cwd -p … -F n`: each process's working folder.
pub fn parse_cwds(text: &str) -> HashMap<u32, String> {
    let mut cwds = HashMap::new();
    let mut pid = None::<u32>;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse().ok();
        } else if let (Some(value), Some(pid)) = (line.strip_prefix('n'), pid) {
            cwds.insert(pid, value.to_string());
        }
    }
    cwds
}

/// Ports dev servers pick by default: Next/Rails/Express, Vite, Astro, Angular,
/// Storybook, Django/FastAPI, webpack, Wrangler, Expo, Hugo, Phoenix/Jekyll.
pub fn is_dev_port(port: u16) -> bool {
    matches!(
        port,
        1313 | 3000..=3010 | 4000..=4010 | 4173..=4179 | 4200 | 4321 | 5000..=5010
            | 5173..=5180 | 6006 | 8000..=8010 | 8080..=8090 | 8787 | 8888 | 9000
            | 19000..=19006
    )
}

/// macOS services and agent CLIs that listen on such ports but are never a page to open.
fn is_excluded_process(command: &str) -> bool {
    const EXCLUDED: &[&str] = &[
        "ControlCenter",
        "rapportd",
        "launchd",
        "mDNSResponder",
        "sharingd",
        "AirPlayXPCHelper",
        "remoted",
        "claude",
        "codex",
        "opencode",
        "cursor-agent",
        "grok",
    ];
    EXCLUDED
        .iter()
        .any(|name| command.eq_ignore_ascii_case(name))
}

/// True when `path` is `root` or inside it (on a path boundary).
fn inside(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    !root.is_empty()
        && (path == root
            || path
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('/')))
}

/// The servers to show for a session whose folders are `roots`, workspace ones first.
pub fn select_servers(
    listeners: &[Listener],
    cwds: &HashMap<u32, String>,
    table: &[crate::process_tree::Proc],
    me: u32,
    roots: &[String],
) -> Vec<LocalServer> {
    let owned: std::collections::HashSet<i32> = crate::process_tree::descendants(table, me as i32)
        .into_iter()
        .map(|process| process.pid)
        .collect();
    let mut servers: Vec<LocalServer> = listeners
        .iter()
        .filter(|listener| listener.pid != me && !is_excluded_process(&listener.command))
        .filter_map(|listener| {
            let in_workspace = cwds
                .get(&listener.pid)
                .is_some_and(|cwd| roots.iter().any(|root| inside(cwd, root)));
            if !in_workspace && !is_dev_port(listener.port) {
                return None;
            }
            Some(LocalServer {
                url: format!("http://localhost:{}", listener.port),
                port: listener.port,
                pid: listener.pid,
                process: listener.command.clone(),
                owned: owned.contains(&(listener.pid as i32)),
                in_workspace,
            })
        })
        .collect();
    // One row per port: a server's workers share it.
    servers.sort_by_key(|server| (!server.in_workspace, !server.owned, server.port));
    let mut seen = std::collections::HashSet::new();
    servers.retain(|server| seen.insert(server.port));
    servers.truncate(8);
    servers
}

/// A preview URL a project script may open: http(s) on a loopback host with a
/// port, no credentials, at most 512 characters. Returned trimmed.
pub fn loopback_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.len() > 512 || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') || authority.contains("0.0.0.0") {
        return None;
    }
    localhost_port(url).map(|_| url.to_string())
}

/// Whether something accepts connections on the URL's loopback port.
pub async fn responds(url: &str) -> bool {
    let Some(port) = localhost_port(url) else {
        return false;
    };
    for address in [
        std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)),
    ] {
        if let Ok(Ok(_)) = tokio::time::timeout(
            Duration::from_millis(600),
            tokio::net::TcpStream::connect(address),
        )
        .await
        {
            return true;
        }
    }
    false
}

/// The port of an http(s) localhost URL, or `None` for anything else.
pub fn localhost_port(url: &str) -> Option<u16> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, tail) = bracketed.split_once(']')?;
        (host, tail.strip_prefix(':')?)
    } else {
        authority.rsplit_once(':')?
    };
    if !matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") {
        return None;
    }
    port.parse::<u16>().ok().filter(|port| *port > 0)
}

async fn output(program: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    let result = crate::cli_output::capture_command(command, PROBE_TIMEOUT)
        .await
        .ok()?;
    Some(String::from_utf8_lossy(&result.stdout).into_owned())
}

async fn parent_of(pid: u32) -> Option<u32> {
    output(PS, &["-o", "ppid=", "-p", &pid.to_string()])
        .await?
        .trim()
        .parse()
        .ok()
}

/// True when `pid` descends from this app's process.
async fn owned(pid: u32) -> bool {
    let me = std::process::id();
    let mut current = pid;
    for _ in 0..MAX_ANCESTRY {
        if current == me {
            return current != pid;
        }
        match parent_of(current).await {
            Some(parent) if parent > 1 && parent != current => current = parent,
            _ => return false,
        }
    }
    false
}

pub async fn local_server_action(
    state: State<'_, std::sync::Arc<AppState>>,
    action: Action,
) -> Result<u32> {
    let Action::Stop { session_id, url } = action;
    state.ensure_running()?;
    if !state
        .data
        .lock()
        .sessions
        .iter()
        .any(|session| session.id == session_id)
    {
        return Err(Error::not_found("session not found"));
    }
    let port = localhost_port(&url)
        .ok_or_else(|| Error::new("invalid", "Only localhost servers can be stopped."))?;
    let listing = output(
        LSOF,
        &["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"],
    )
    .await
    .unwrap_or_default();
    let pids: Vec<u32> = listing
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .take(8)
        .collect();
    if pids.is_empty() {
        return Err(Error::new("not_found", "That server is no longer running."));
    }
    let mut stopped = 0;
    for pid in pids {
        if !owned(pid).await {
            continue;
        }
        #[cfg(unix)]
        // SAFETY: plain signal to a verified descendant process id.
        if unsafe { libc::kill(pid as i32, libc::SIGTERM) } == 0 {
            stopped += 1;
        }
    }
    if stopped == 0 {
        return Err(Error::new(
            "forbidden",
            "This server was not started by Sirus, so it is left running.",
        ));
    }
    Ok(stopped)
}

/// Listening sockets, their processes' folders and the process table.
#[derive(Clone, Default)]
struct Snapshot {
    listeners: Vec<Listener>,
    cwds: HashMap<u32, String>,
    table: Vec<crate::process_tree::Proc>,
}

/// Probes run at most this often; every caller in between reads the same snapshot.
const SNAPSHOT_TTL: Duration = Duration::from_secs(3);

fn snapshot_cache() -> &'static Mutex<Option<(Instant, Snapshot)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Snapshot)>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

async fn snapshot() -> Snapshot {
    if let Some((at, snapshot)) = snapshot_cache().lock().as_ref() {
        if at.elapsed() < SNAPSHOT_TTL {
            return snapshot.clone();
        }
    }
    let listeners = parse_listeners(
        &output(LSOF, &["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pcn"])
            .await
            .unwrap_or_default(),
    );
    let mut pids: Vec<String> = listeners
        .iter()
        .map(|listener| listener.pid.to_string())
        .collect();
    pids.dedup();
    pids.truncate(64);
    let cwds = if pids.is_empty() {
        HashMap::new()
    } else {
        parse_cwds(
            &output(LSOF, &["-a", "-d", "cwd", "-p", &pids.join(","), "-F", "n"])
                .await
                .unwrap_or_default(),
        )
    };
    let table = crate::process_tree::parse_table(
        &output(PS, &["-A", "-o", "pid=,ppid=,pgid="])
            .await
            .unwrap_or_default(),
    );
    let snapshot = Snapshot {
        listeners,
        cwds,
        table,
    };
    *snapshot_cache().lock() = Some((Instant::now(), snapshot.clone()));
    snapshot
}

/// The local servers worth opening for a session (ADR-151). Read on demand only: the
/// browser's empty state, the Environment card's row and a starting script or terminal.
pub async fn local_servers_list(
    state: State<'_, std::sync::Arc<AppState>>,
    session_id: String,
) -> Result<Vec<LocalServer>> {
    let roots = {
        let data = state.data.lock();
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        let mut roots = vec![session.worktree.path.clone()];
        if let Some(project) = data
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
        {
            roots.push(project.path.clone());
        }
        roots
    };
    if !cfg!(unix) {
        return Ok(Vec::new());
    }
    // lsof reports resolved folders (`/private/var/…` for `/var/…`).
    let resolved: Vec<String> = roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let mut roots = roots;
    roots.extend(resolved);
    let snapshot = snapshot().await;
    Ok(select_servers(
        &snapshot.listeners,
        &snapshot.cwds,
        &snapshot.table,
        std::process::id(),
        &roots,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_tree::Proc;

    const LSOF_SAMPLE: &str = "p727\ncControlCenter\nf9\nn*:7000\nf11\nn*:5000\np900\ncnode\nf20\nn127.0.0.1:5173\nf21\nn[::1]:5173\np901\ncpython3.12\nf3\nn*:8899\np902\ncnode\nf4\nn*:3000\np903\ncsirus-code\nf5\nn127.0.0.1:3001\np904\ncopencode\nf6\nn127.0.0.1:4096\n";

    #[test]
    fn lsof_listeners_parse_and_collapse_address_families() {
        let listeners = parse_listeners(LSOF_SAMPLE);
        assert_eq!(listeners.len(), 7);
        assert_eq!(
            listeners[2],
            Listener {
                pid: 900,
                command: "node".into(),
                port: 5173
            }
        );
        assert!(parse_listeners("garbage\nnno-port\n").is_empty());
        let cwds = parse_cwds("p900\nfcwd\nn/work/app\np901\nfcwd\nn/\n");
        assert_eq!(cwds.get(&900).map(String::as_str), Some("/work/app"));
        assert_eq!(cwds.get(&901).map(String::as_str), Some("/"));
    }

    #[test]
    fn servers_are_workspace_ones_or_dev_ports_never_services_or_sirus() {
        let listeners = parse_listeners(LSOF_SAMPLE);
        let cwds = parse_cwds(
            "p900\nn/elsewhere\np901\nn/work/app/api\np902\nn/Users/me\np904\nn/work/app\n",
        );
        let table = vec![
            Proc {
                pid: 903,
                ppid: 1,
                pgid: 903,
            },
            Proc {
                pid: 950,
                ppid: 903,
                pgid: 950,
            },
            Proc {
                pid: 902,
                ppid: 950,
                pgid: 950,
            },
        ];
        let servers = select_servers(&listeners, &cwds, &table, 903, &["/work/app".into()]);
        let ports: Vec<u16> = servers.iter().map(|server| server.port).collect();
        // 8899 is in the workspace; 3000 is Sirus's grandchild; 5173 a dev port elsewhere.
        // ControlCenter's 5000/7000, Sirus's own 3001 and OpenCode's 4096 never show.
        assert_eq!(ports, vec![8899, 3000, 5173]);
        assert!(servers[0].in_workspace && !servers[0].owned);
        assert!(servers[1].owned && !servers[1].in_workspace);
        assert_eq!(servers[2].url, "http://localhost:5173");
        assert_eq!(servers[2].process, "node");
        assert!(!inside("/work/application", "/work/app"));
        assert!(inside("/work/app", "/work/app/"));
        assert!(is_dev_port(5173) && is_dev_port(3000) && !is_dev_port(22) && !is_dev_port(5432));
    }

    #[test]
    fn preview_urls_are_loopback_http_with_a_port() {
        assert_eq!(
            loopback_url(" http://localhost:5173/app "),
            Some("http://localhost:5173/app".into())
        );
        assert!(loopback_url("https://127.0.0.1:8443").is_some());
        assert!(loopback_url("http://[::1]:3000/").is_some());
        assert!(loopback_url("http://localhost").is_none());
        assert!(loopback_url("http://example.com:3000").is_none());
        assert!(loopback_url("http://user@localhost:3000").is_none());
        assert!(loopback_url("http://0.0.0.0:3000").is_none());
        assert!(loopback_url("file:///etc/passwd").is_none());
        assert!(loopback_url("javascript:alert(1)").is_none());
        assert!(loopback_url("http://localhost:3000/a b").is_none());
    }

    #[test]
    fn only_localhost_ports_are_accepted() {
        assert_eq!(localhost_port("http://localhost:5173/"), Some(5173));
        assert_eq!(localhost_port("http://127.0.0.1:3000/app?x=1"), Some(3000));
        assert_eq!(localhost_port("https://[::1]:8443"), Some(8443));
        assert_eq!(localhost_port("http://example.com:80"), None);
        assert_eq!(localhost_port("http://localhost"), None);
        assert_eq!(localhost_port("file:///tmp:80"), None);
        assert_eq!(localhost_port("http://localhost:0"), None);
    }
}
