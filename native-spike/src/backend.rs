//! The link to Sirus's back end: this app runs `sirus-host` (the same engine without a window,
//! ADR-131) on its own data folder and speaks the socket protocol the phone uses (ADR-080).
//! The folder starts as a copy of the Tauri app's state with automatic work turned off, so two
//! engines never share one database and the copy never resumes or acts on anything by itself.
//! Old Claude conversations start a fresh CLI thread here; their history still shows.
use crate::i18n::{t, tf};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::time::Duration;
use tungstenite::Message;

/// The window's own connection: loopback only, always on.
const PORT: u16 = 7811;
/// The listener for other devices, on only while Settings → Conexões allows it. Sirus Code uses
/// 7710, so the two never clash on one Mac.
const REMOTE_PORT: u16 = 7711;
/// `tailscale serve` takes one handler per HTTPS port and Sirus Code holds 443, so this app
/// serves its secure address on 8443 and turning either off leaves the other alone.
const HTTPS_PORT: u16 = 8443;
static HOST_PID: AtomicI32 = AtomicI32::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub enum Incoming {
    /// Connected, with `load_state`'s metadata.
    Ready(Value),
    Reply {
        id: u64,
        ok: bool,
        value: Value,
    },
    Event {
        name: String,
        payload: Value,
    },
    Problem(String),
}

type Waiters = std::sync::Arc<Mutex<HashMap<u64, async_channel::Sender<Result<Value, Value>>>>>;
type Listeners = std::sync::Arc<Mutex<Vec<(String, async_channel::Sender<Value>)>>>;

#[derive(Clone)]
pub struct Backend {
    outgoing: std::sync::Arc<Mutex<VecDeque<String>>>,
    waiters: Waiters,
    listeners: Listeners,
}

impl Backend {
    pub fn invoke(&self, cmd: &str, args: Value) -> u64 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let frame = json!({ "type": "invoke", "id": id, "cmd": cmd, "args": args }).to_string();
        self.outgoing.lock().unwrap().push_back(frame);
        id
    }

    /// Runs a command; the reply arrives on the returned channel.
    pub fn request(&self, cmd: &str, args: Value) -> async_channel::Receiver<Result<Value, Value>> {
        let (tx, rx) = async_channel::bounded(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        self.waiters.lock().unwrap().insert(id, tx);
        let frame = json!({ "type": "invoke", "id": id, "cmd": cmd, "args": args }).to_string();
        self.outgoing.lock().unwrap().push_back(frame);
        rx
    }

    /// Every payload of an event, for as long as the receiver lives.
    pub fn subscribe(&self, event: &str) -> async_channel::Receiver<Value> {
        let (tx, rx) = async_channel::unbounded();
        self.listeners.lock().unwrap().push((event.to_owned(), tx));
        self.outgoing
            .lock()
            .unwrap()
            .push_back(json!({ "type": "listen", "event": event }).to_string());
        rx
    }
}

pub fn start() -> (Backend, async_channel::Receiver<Incoming>) {
    let (tx, rx) = async_channel::unbounded();
    let outgoing = std::sync::Arc::new(Mutex::new(VecDeque::new()));
    let backend = Backend {
        outgoing: outgoing.clone(),
        waiters: Default::default(),
        listeners: Default::default(),
    };
    let (waiters, listeners) = (backend.waiters.clone(), backend.listeners.clone());
    std::thread::spawn(move || {
        loop {
            if let Err(problem) = session(&tx, &outgoing, &waiters, &listeners) {
                if tx.send_blocking(Incoming::Problem(problem)).is_err() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    (backend, rx)
}

/// Stops the host this app started; it finishes its agents and saves, as on any shutdown.
pub fn stop_host() {
    let pid = HOST_PID.swap(0, Ordering::SeqCst);
    if pid > 0 {
        let _ = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }
}

pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/com.siruscode.native")
}

fn host_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let beside = exe.with_file_name("sirus-host");
    if beside.exists() {
        return Ok(beside);
    }
    let built = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/host/release/sirus-host");
    built
        .exists()
        .then_some(built)
        .ok_or_else(|| t("O sirus-host não foi encontrado.").into())
}

/// First launch: a consistent copy of the app's state, with everything automatic turned off.
fn seed(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let database = dir.join("state.db");
    if database.exists() {
        return Ok(());
    }
    let source = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/com.siruscode.app");
    let backup = format!(".backup '{}'", database.display());
    let status = Command::new("sqlite3")
        .arg(format!(
            "file:{}?mode=ro",
            source.join("state.db").display()
        ))
        .arg(backup)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(t("Não consegui copiar os dados do Sirus.").into());
    }
    let clean = r#"
update settings set json=json_set(json,'$.continueInterruptedSessions',json('false'),'$.ciAutoFix',json('false'),'$.prWatch',json('false'),'$.usageAutoResume',json('false'),'$.gitAutoFetch',json('false'),'$.gitAutoPull',json('false'),'$.appUpdateAutoCheck',json('false'),'$.appUpdateAutoDownload',json('false'));
delete from app_state where key in ('prWatches','ciAutoFix');
update sessions set json=json_set(json,'$.status','idle') where json_extract(json,'$.status') not in ('idle','completed');
delete from app_state where key in ('providerAccounts','selectedProviderAccounts');
update sessions set json=json_remove(json,'$.providerAccountId','$.accountBindings');
update sessions set json=json_remove(json,'$.nativeThread') where json_extract(json,'$.agent')='claude';
"#;
    let status = Command::new("sqlite3")
        .arg(&database)
        .arg(clean)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        let _ = std::fs::remove_file(&database);
        return Err(t("Não consegui preparar a cópia dos dados.").into());
    }
    // Account profiles aren't copied: a Claude login belongs to its folder, so this app uses the
    // CLI's default login. Skills are copied, because the host refuses a linked folder.
    let _ = Command::new("cp")
        .arg("-R")
        .arg(source.join("skills"))
        .arg(dir.join("skills"))
        .status();
    Ok(())
}

fn healthy() -> bool {
    health().is_some_and(|body| body.contains("\"app\":\"sirus\""))
}

fn health() -> Option<String> {
    http(([127, 0, 0, 1], PORT).into(), "GET /api/health", None)
}

/// A host an older build left running: it serves the window on the one port and has no
/// listener of its own for other devices, so Settings → Conexões could not turn it off.
fn stop_old_host() {
    let Some(body) = health() else { return };
    if body.contains("\"window\":true") {
        return;
    }
    let Ok(output) = Command::new("lsof")
        .args(["-ti", &format!("tcp:{PORT}"), "-sTCP:LISTEN"])
        .output()
    else {
        return;
    };
    for pid in String::from_utf8_lossy(&output.stdout).split_whitespace() {
        let _ = Command::new("kill").args(["-TERM", pid]).status();
    }
    for _ in 0..40 {
        if health().is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The phone's web app, built from this checkout: in the bundle, else the repository's `dist`.
fn ui_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundled = exe.parent()?.parent()?.join("Resources/ui");
    let built = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist");
    [bundled, built]
        .into_iter()
        .find(|dir| dir.join("index.html").is_file())
}

/// The web UI's folder and the address the window's own host serves it at: the hidden page that
/// draws diagrams and formulas loads Sirus's bundled Mermaid and KaTeX from there.
pub fn ui_assets() -> Option<(PathBuf, String)> {
    ui_dir().map(|dir| (dir, format!("http://127.0.0.1:{PORT}")))
}

fn http(address: SocketAddr, line: &str, body: Option<&str>) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(600)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let body = body.unwrap_or("");
    write!(stream, "{line} HTTP/1.1\r\nHost: 127.0.0.1:{PORT}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    Some(reply.split_once("\r\n\r\n")?.1.to_owned())
}

fn ensure_host(dir: &Path) -> Result<(), String> {
    stop_old_host();
    if healthy() {
        return Ok(());
    }
    let host = host_binary()?;
    let log = std::fs::File::create(dir.join("host.log")).map_err(|e| e.to_string())?;
    let mut command = Command::new(host);
    command
        .args([
            "serve",
            "--local-port",
            &PORT.to_string(),
            "--port",
            &REMOTE_PORT.to_string(),
            "--https-port",
            &HTTPS_PORT.to_string(),
            "--label",
            "Sirus Native",
            "--data-dir",
        ])
        .arg(dir);
    if let Some(ui) = ui_dir() {
        command.arg("--ui-dir").arg(ui);
    }
    let child = command
        .env("SIRUS_KEYCHAIN_APP", "Sirus Native")
        // Agents are offered the browser tools: this window runs them.
        .env("SIRUS_BROWSER_CLIENT", "1")
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| e.to_string())?;
    HOST_PID.store(child.id() as i32, Ordering::SeqCst);
    for _ in 0..40 {
        if healthy() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(t("O motor do Sirus não respondeu.").into())
}

/// Pairs this app as a device of its own host, keeping the token in the data folder.
fn pair(dir: &Path) -> Result<String, String> {
    let output = Command::new(host_binary()?)
        .args(["pair", "window"])
        .arg("--data-dir")
        .arg(dir)
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let code: String = text
        .lines()
        .find_map(|l| l.strip_prefix("Pairing code:"))
        .ok_or(t("O motor não gerou um código."))?
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let body = json!({ "code": code, "name": "Sirus Native" }).to_string();
    let reply = http(([127, 0, 0, 1], PORT).into(), "POST /api/pair", Some(&body))
        .ok_or(t("O pareamento não respondeu."))?;
    let value: Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
    let token = value["token"]
        .as_str()
        .ok_or(t("O pareamento foi recusado."))?
        .to_owned();
    let file = dir.join("native-token");
    std::fs::write(&file, &token).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    Ok(token)
}

/// Saves a chosen picture as the chat background the back end expects: `chat-<uuid>.jpg`.
pub fn save_background(path: &str, name: &str) -> bool {
    let folder = data_dir().join("backgrounds");
    let _ = std::fs::create_dir_all(&folder);
    Command::new("sips")
        .args(["-s", "format", "jpeg", "-Z", "2800", path, "--out"])
        .arg(folder.join(name))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// A real app's icon from the back end, as a file the interface can draw.
pub fn save_icon(id: &str, png: &str) -> Option<PathBuf> {
    if png.is_empty() {
        return None;
    }
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = Vec::with_capacity(png.len() / 4 * 3);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in png.bytes().filter(|c| *c != b'=') {
        buffer = (buffer << 6) | TABLE.iter().position(|t| *t == c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    let folder = data_dir().join("editor-icons");
    std::fs::create_dir_all(&folder).ok()?;
    let path = folder.join(format!("{id}.png"));
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

pub fn background_path(name: &str) -> PathBuf {
    data_dir().join("backgrounds").join(name)
}

fn session(
    tx: &async_channel::Sender<Incoming>,
    outgoing: &Mutex<VecDeque<String>>,
    waiters: &Waiters,
    listeners: &Listeners,
) -> Result<(), String> {
    let dir = data_dir();
    seed(&dir)?;
    ensure_host(&dir)?;
    let mut token = match std::fs::read_to_string(dir.join("native-token")) {
        Ok(token) => token,
        Err(_) => pair(&dir)?,
    };
    let (mut socket, _) = tungstenite::connect(format!("ws://127.0.0.1:{PORT}/api/socket"))
        .map_err(|e| e.to_string())?;
    let hello = |socket: &mut tungstenite::WebSocket<_>, token: &str| {
        socket.send(Message::text(
            json!({ "type": "hello", "token": token }).to_string(),
        ))
    };
    hello(&mut socket, &token).map_err(|e| e.to_string())?;
    loop {
        match socket.read().map_err(|e| e.to_string())? {
            Message::Text(text) => {
                let message: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                match message["type"].as_str() {
                    Some("ready") => break,
                    Some("unauthorized") => {
                        token = pair(&dir)?;
                        let (again, _) =
                            tungstenite::connect(format!("ws://127.0.0.1:{PORT}/api/socket"))
                                .map_err(|e| e.to_string())?;
                        socket = again;
                        hello(&mut socket, &token).map_err(|e| e.to_string())?;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    for event in [
        "session-updated",
        "agent-output",
        "agent-exit",
        "subagent-feed",
        "secret-state",
        "browser_tool_request",
        "login-vault-state",
        "computer-state",
        "remote-changed",
    ] {
        socket
            .send(Message::text(
                json!({ "type": "listen", "event": event }).to_string(),
            ))
            .map_err(|e| e.to_string())?;
    }
    // This window is the host's browser: agents' browser tools run here (browser_bridge.rs).
    socket
        .send(Message::text(
            json!({ "type": "invoke", "id": u64::MAX, "cmd": "register_browser_client", "args": {} })
                .to_string(),
        ))
        .map_err(|e| e.to_string())?;
    const STATE: u64 = 0;
    socket
        .send(Message::text(
            json!({ "type": "invoke", "id": STATE, "cmd": "load_state", "args": {} }).to_string(),
        ))
        .map_err(|e| e.to_string())?;
    if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_ref() {
        stream
            .set_read_timeout(Some(Duration::from_millis(15)))
            .map_err(|e| e.to_string())?;
    }
    loop {
        while let Some(frame) = {
            let next = outgoing.lock().unwrap().pop_front();
            next
        } {
            socket
                .send(Message::text(frame))
                .map_err(|e| e.to_string())?;
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                let mut message: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                if message["type"] == "result" {
                    let waiter = message["id"]
                        .as_u64()
                        .and_then(|id| waiters.lock().unwrap().remove(&id));
                    if let Some(waiter) = waiter {
                        let ok = message["ok"].as_bool().unwrap_or(false);
                        let _ = waiter.try_send(if ok {
                            Ok(message["value"].take())
                        } else {
                            Err(message["error"].take())
                        });
                        continue;
                    }
                }
                if message["type"] == "event" {
                    let name = message["event"].as_str().unwrap_or("");
                    listeners.lock().unwrap().retain(|(event, tx)| {
                        if event == name {
                            tx.try_send(message["payload"].clone()).is_ok()
                        } else {
                            !tx.is_closed()
                        }
                    });
                }
                let incoming = match message["type"].as_str() {
                    Some("result") if message["id"] == STATE => {
                        Incoming::Ready(message["value"].take())
                    }
                    Some("result") => {
                        let ok = message["ok"].as_bool().unwrap_or(false);
                        let value = if ok {
                            message["value"].take()
                        } else {
                            message["error"].take()
                        };
                        Incoming::Reply {
                            id: message["id"].as_u64().unwrap_or(0),
                            ok,
                            value,
                        }
                    }
                    Some("event") => Incoming::Event {
                        name: message["event"].as_str().unwrap_or("").to_owned(),
                        payload: message["payload"].take(),
                    },
                    _ => continue,
                };
                if tx.send_blocking(incoming).is_err() {
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err(tf("A conexão com o motor caiu: {}", &[&error])),
        }
    }
}

/// `git` in a folder, off the UI thread's concerns; empty when it fails.
pub fn git(path: &str, args: &[&str]) -> String {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}
