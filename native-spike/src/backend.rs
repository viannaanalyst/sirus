//! The link to Sirus's back end: this app runs `sirus-host` (the same engine without a window,
//! ADR-131) on its own data folder and speaks the socket protocol the phone uses (ADR-080).
//! The folder starts as a copy of the Tauri app's state with automatic work turned off, so two
//! engines never share one database and the copy never resumes or acts on anything by itself.
//! Old Claude conversations start a fresh CLI thread here; their history still shows.
use crate::i18n::{t, tf};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tungstenite::Message;

/// The window's own connection: loopback only, always on.
const PORT: u16 = 7811;
/// The listener for other devices, on only while Settings → Conexões allows it. Sirus uses
/// 7710, so the two never clash on one Mac.
const REMOTE_PORT: u16 = 7711;
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
    /// Where the connection to another computer is.
    Link {
        id: String,
        status: crate::computers::Status,
    },
}

type Waiter = async_channel::Sender<Result<Value, Value>>;
type Listeners = std::sync::Arc<Mutex<Vec<(String, async_channel::Sender<Value>)>>>;

/// One connection's queue of frames to send and the callers waiting for replies.
#[derive(Default)]
struct Link {
    outgoing: Mutex<VecDeque<String>>,
    waiters: Mutex<HashMap<u64, Waiter>>,
    /// Ids of this Mac's own commands (`computers::runs_here`) sent while another computer is
    /// active: their replies still reach the window.
    always: Mutex<HashSet<u64>>,
    /// Another computer's link only: connected and past `hello`.
    open: AtomicBool,
}

impl Link {
    fn push(&self, frame: String) {
        self.outgoing.lock().unwrap().push_back(frame);
    }

    /// An id whose reply nobody reads: a waiter with no receiver swallows it, so even a refusal
    /// never reaches the window as a problem.
    fn quiet_id(&self) -> u64 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, _) = async_channel::bounded(1);
        self.waiters.lock().unwrap().insert(id, tx);
        id
    }

    /// Fails everything waiting and forgets what was queued: a command never outlives the
    /// connection it was meant for.
    fn drop_pending(&self) {
        self.outgoing.lock().unwrap().clear();
        self.always.lock().unwrap().clear();
        let waiters: Vec<Waiter> = self
            .waiters
            .lock()
            .unwrap()
            .drain()
            .map(|(_, w)| w)
            .collect();
        for waiter in waiters {
            let _ = waiter.try_send(Err(lost()));
        }
    }
}

fn lost() -> Value {
    json!({ "code": "remote", "message": t("A conexão com o computador caiu.") })
}

/// The one connection to another computer, and the signal that ends it.
struct Remote {
    stop: std::sync::Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct Backend {
    local: std::sync::Arc<Link>,
    /// The link of the current connection to another computer; each connection gets a fresh
    /// one, so a connection that is ending can never send what was queued for the next.
    remote: std::sync::Arc<Mutex<std::sync::Arc<Link>>>,
    /// The active computer's id; `None` is this Mac. Commands go to its link, except this
    /// Mac's own (`computers::runs_here`), which always go to the local one.
    active: std::sync::Arc<Mutex<Option<String>>>,
    control: std::sync::Arc<Mutex<Option<Remote>>>,
    listeners: Listeners,
    notify: async_channel::Sender<Incoming>,
}

/// The event a host sends the browser client it registered (browser_bridge.rs).
const BROWSER_REQUEST: &str = "browser_tool_request";

/// Whether an event from a connection reaches the window: this Mac's own events always come
/// from it, everything else from the active computer's connection only. Browser requests come
/// from this Mac's host and, while another computer is active, from that one too (ADR-201).
fn event_accepted(active_is_remote: bool, from_remote: bool, event: &str) -> bool {
    if event == BROWSER_REQUEST {
        !from_remote || active_is_remote
    } else if crate::computers::event_is_local(event) {
        !from_remote
    } else {
        from_remote == active_is_remote
    }
}

impl Backend {
    /// The link a command runs on, and whether it is another computer's.
    fn route(&self, cmd: &str) -> (std::sync::Arc<Link>, bool) {
        let remote = self.active.lock().unwrap().is_some() && !crate::computers::runs_here(cmd);
        if remote {
            (self.remote_link(), true)
        } else {
            (self.local.clone(), false)
        }
    }

    fn remote_link(&self) -> std::sync::Arc<Link> {
        self.remote.lock().unwrap().clone()
    }

    fn frame(id: u64, cmd: &str, args: Value) -> String {
        json!({ "type": "invoke", "id": id, "cmd": cmd, "args": args }).to_string()
    }

    /// Queues a frame; false (and nothing queued) when another computer's link is not open.
    fn send(&self, cmd: &str, id: u64, frame: String) -> bool {
        let (link, remote) = self.route(cmd);
        if remote && !link.open.load(Ordering::SeqCst) {
            return false;
        }
        if !remote && crate::computers::runs_here(cmd) {
            link.always.lock().unwrap().insert(id);
        }
        link.push(frame);
        true
    }

    pub fn invoke(&self, cmd: &str, args: Value) -> u64 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        if !self.send(cmd, id, Self::frame(id, cmd, args)) {
            let _ = self.notify.try_send(Incoming::Reply {
                id,
                ok: false,
                value: lost(),
            });
        }
        id
    }

    /// Runs a command; the reply arrives on the returned channel.
    pub fn request(&self, cmd: &str, args: Value) -> async_channel::Receiver<Result<Value, Value>> {
        let (tx, rx) = async_channel::bounded(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (link, _) = self.route(cmd);
        link.waiters.lock().unwrap().insert(id, tx.clone());
        if !self.send(cmd, id, Self::frame(id, cmd, args)) {
            link.waiters.lock().unwrap().remove(&id);
            let _ = tx.try_send(Err(lost()));
        }
        rx
    }

    /// Sends a command to the active computer's connection whatever its name, its reply
    /// dropped: a browser tool's answer goes back to the host that asked (ADR-201).
    pub fn invoke_remote(&self, cmd: &str, args: Value) {
        let link = self.remote_link();
        if !link.open.load(Ordering::SeqCst) {
            return;
        }
        let id = link.quiet_id();
        link.push(Self::frame(id, cmd, args));
    }

    /// Runs a command on this Mac whatever computer is active (a move's target).
    pub fn request_local(
        &self,
        cmd: &str,
        args: Value,
    ) -> async_channel::Receiver<Result<Value, Value>> {
        let (tx, rx) = async_channel::bounded(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        self.local.waiters.lock().unwrap().insert(id, tx);
        self.local.push(Self::frame(id, cmd, args));
        rx
    }

    /// Every payload of an event, for as long as the receiver lives.
    pub fn subscribe(&self, event: &str) -> async_channel::Receiver<Value> {
        let (tx, rx) = async_channel::unbounded();
        self.listeners.lock().unwrap().push((event.to_owned(), tx));
        let listen = json!({ "type": "listen", "event": event }).to_string();
        self.local.push(listen.clone());
        let remote = self.remote_link();
        if remote.open.load(Ordering::SeqCst) && !crate::computers::event_is_local(event) {
            remote.push(listen);
        }
        rx
    }

    /// Makes `id` the active computer (`None`: this Mac). Does not connect.
    pub fn activate(&self, id: Option<&str>) {
        *self.active.lock().unwrap() = id.map(str::to_owned);
    }

    /// Asks the active computer for its state again; `Incoming::Ready` follows.
    pub fn reload(&self) {
        let (link, remote) = self.route("load_state");
        if remote && !link.open.load(Ordering::SeqCst) {
            return;
        }
        link.push(Self::frame(STATE, "load_state", json!({})));
    }

    /// Connects to another computer's socket with its token and keeps it up, retrying after a
    /// drop, until `disconnect_remote` or the host no longer accepts the token. The token stays
    /// in this thread's memory.
    pub fn connect_remote(&self, id: &str, address: crate::remote_tls::Address, token: String) {
        self.disconnect_remote();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        *self.control.lock().unwrap() = Some(Remote { stop: stop.clone() });
        let link = std::sync::Arc::new(Link::default());
        *self.remote.lock().unwrap() = link.clone();
        // Pages on that computer's localhost open through tunnels (tunnel.rs).
        crate::tunnel::activate(address.clone(), token.clone());
        let backend = self.clone();
        let id = id.to_owned();
        std::thread::spawn(move || backend.remote_loop(&id, &address, &token, &stop, &link));
    }

    /// Ends the connection to the other computer and fails what was waiting on it.
    pub fn disconnect_remote(&self) {
        if let Some(remote) = self.control.lock().unwrap().take() {
            remote.stop.store(true, Ordering::SeqCst);
        }
        crate::tunnel::deactivate();
        let link = self.remote_link();
        link.open.store(false, Ordering::SeqCst);
        link.drop_pending();
    }

    fn remote_loop(
        &self,
        id: &str,
        address: &crate::remote_tls::Address,
        token: &str,
        stop: &AtomicBool,
        link: &Link,
    ) {
        use crate::computers::Status;
        let status = |status| {
            if !stop.load(Ordering::SeqCst) {
                let _ = self.notify.try_send(Incoming::Link {
                    id: id.to_owned(),
                    status,
                });
            }
        };
        let mut delay = 2;
        while !stop.load(Ordering::SeqCst) {
            status(Status::Connecting);
            link.drop_pending();
            match self.remote_session(address, token, stop, link, &status) {
                Err(Failure::Unauthorized) => {
                    status(Status::Unauthorized);
                    break;
                }
                Err(Failure::Lost) if stop.load(Ordering::SeqCst) => break,
                Ok(()) => break,
                Err(Failure::Lost) => {}
            }
            link.open.store(false, Ordering::SeqCst);
            link.drop_pending();
            status(Status::Lost);
            // Back off 2, 4, 8 up to 15 seconds, checking the stop flag as it waits.
            for _ in 0..delay * 4 {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            delay = (delay * 2).min(15);
        }
    }

    fn remote_session(
        &self,
        address: &crate::remote_tls::Address,
        token: &str,
        stop: &AtomicBool,
        link: &Link,
        status: &dyn Fn(crate::computers::Status),
    ) -> Result<(), Failure> {
        let mut socket = crate::remote_tls::open_socket(address).map_err(|_| Failure::Lost)?;
        socket
            .send(Message::text(
                json!({ "type": "hello", "token": token }).to_string(),
            ))
            .map_err(|_| Failure::Lost)?;
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            match socket.read().map_err(|_| Failure::Lost)? {
                Message::Text(text) => {
                    let message: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                    match message["type"].as_str() {
                        Some("ready") => break,
                        Some("unauthorized") => return Err(Failure::Unauthorized),
                        _ => {}
                    }
                }
                Message::Close(_) => return Err(Failure::Lost),
                _ => {}
            }
        }
        let mut events: Vec<String> = LISTEN.iter().map(|e| (*e).to_owned()).collect();
        events.extend(
            self.listeners
                .lock()
                .unwrap()
                .iter()
                .map(|(e, _)| e.clone()),
        );
        events.sort();
        events.dedup();
        for event in events
            .iter()
            .filter(|e| !crate::computers::event_is_local(e))
        {
            socket
                .send(Message::text(
                    json!({ "type": "listen", "event": event }).to_string(),
                ))
                .map_err(|_| Failure::Lost)?;
        }
        // This window runs that computer's browser too, when it has no window of its own; a
        // host that has one refuses, and the reply is dropped either way (ADR-201).
        socket
            .send(Message::text(Self::frame(
                link.quiet_id(),
                "register_browser_client",
                json!({}),
            )))
            .map_err(|_| Failure::Lost)?;
        socket
            .send(Message::text(Self::frame(STATE, "load_state", json!({}))))
            .map_err(|_| Failure::Lost)?;
        socket
            .get_ref()
            .sock
            .set_read_timeout(Some(Duration::from_millis(15)))
            .map_err(|_| Failure::Lost)?;
        link.open.store(true, Ordering::SeqCst);
        status(crate::computers::Status::Open);
        let result = self.pump(&mut socket, link, true, Some(stop));
        link.open.store(false, Ordering::SeqCst);
        result.map_err(|_| Failure::Lost)
    }

    /// Sends queued frames and delivers what arrives, until the connection ends.
    fn pump<S: Read + Write>(
        &self,
        socket: &mut tungstenite::WebSocket<S>,
        link: &Link,
        from_remote: bool,
        stop: Option<&AtomicBool>,
    ) -> Result<(), String> {
        let mut heard = Instant::now();
        let mut pinged = Instant::now();
        loop {
            if stop.is_some_and(|stop| stop.load(Ordering::SeqCst)) {
                return Ok(());
            }
            while let Some(frame) = {
                let next = link.outgoing.lock().unwrap().pop_front();
                next
            } {
                socket
                    .send(Message::text(frame))
                    .map_err(|e| e.to_string())?;
            }
            if from_remote {
                // A silent connection is told apart from a quiet host with a ping every 20
                // seconds; a minute without hearing anything ends it.
                if heard.elapsed() > Duration::from_secs(60) {
                    return Err(t("A conexão com o computador caiu.").into());
                }
                if pinged.elapsed() > Duration::from_secs(20) {
                    pinged = Instant::now();
                    socket
                        .send(Message::Ping(Vec::new().into()))
                        .map_err(|e| e.to_string())?;
                }
            }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    heard = Instant::now();
                    let message: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                    if !self.deliver(link, from_remote, message) {
                        return Ok(());
                    }
                }
                Ok(Message::Close(_)) if from_remote => {
                    return Err(t("A conexão com o computador caiu.").into());
                }
                Ok(_) => heard = Instant::now(),
                Err(tungstenite::Error::Io(error))
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(error) => return Err(tf("A conexão com o motor caiu: {}", &[&error])),
            }
        }
    }

    /// Routes one frame to its waiter, listeners and the window; false once the window is gone.
    fn deliver(&self, link: &Link, from_remote: bool, mut message: Value) -> bool {
        let active_is_remote = self.active.lock().unwrap().is_some();
        if message["type"] == "result" {
            let id = message["id"].as_u64();
            let waiter = id.and_then(|id| link.waiters.lock().unwrap().remove(&id));
            if let Some(waiter) = waiter {
                let ok = message["ok"].as_bool().unwrap_or(false);
                let _ = waiter.try_send(if ok {
                    Ok(message["value"].take())
                } else {
                    Err(message["error"].take())
                });
                return true;
            }
            let own = id.is_some_and(|id| link.always.lock().unwrap().remove(&id));
            // The state of a computer that is no longer the active one, or a reply to a command
            // meant for it, is dropped rather than shown on the wrong computer.
            let wanted = own || from_remote == active_is_remote;
            if !wanted {
                return true;
            }
        }
        if message["type"] == "event" {
            let name = message["event"].as_str().unwrap_or("").to_owned();
            if !event_accepted(active_is_remote, from_remote, &name) {
                return true;
            }
            if name == BROWSER_REQUEST && message["payload"].is_object() {
                // The answer goes back on the connection the request came from.
                message["payload"]["via"] = json!(if from_remote { "remote" } else { "local" });
            }
            self.listeners.lock().unwrap().retain(|(event, tx)| {
                if *event == name {
                    tx.try_send(message["payload"].clone()).is_ok()
                } else {
                    !tx.is_closed()
                }
            });
        }
        let incoming = match message["type"].as_str() {
            Some("result") if message["id"] == STATE => Incoming::Ready(message["value"].take()),
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
            _ => return true,
        };
        self.notify.send_blocking(incoming).is_ok()
    }
}

/// Why a connection to another computer ended.
enum Failure {
    Lost,
    Unauthorized,
}

/// The id `load_state` is asked with: its reply becomes `Incoming::Ready`.
const STATE: u64 = 0;

/// Events listened to on every connection, before anything subscribes.
const LISTEN: &[&str] = &[
    "session-updated",
    "agent-output",
    "agent-exit",
    "subagent-feed",
    "secret-state",
    "browser_tool_request",
    "login-vault-state",
    "computer-state",
    "computer-pointer",
    "remote-changed",
    // Cards for other conversations, and the sound and banner the host asks the window for.
    "notification-activity",
    "notification-native",
    // This Mac's provider CLI updates, as offers (toast.rs).
    "provider-updates",
    // Scheduled prompts saved, deleted, switched or run (tasks_page.rs).
    "tasks-updated",
];

pub fn start() -> (Backend, async_channel::Receiver<Incoming>) {
    let (tx, rx) = async_channel::unbounded();
    let backend = Backend {
        local: Default::default(),
        remote: Default::default(),
        active: Default::default(),
        control: Default::default(),
        listeners: Default::default(),
        notify: tx.clone(),
    };
    let local = backend.clone();
    std::thread::spawn(move || {
        loop {
            if let Err(problem) = local.session() {
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

/// First launch: a consistent copy of Sirus's state, when this computer has it, with
/// everything automatic turned off. Without it (a new computer, or Sirus's folder deleted)
/// the host starts with an empty state of its own.
fn seed(dir: &Path) -> Result<(), String> {
    let source = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/com.siruscode.app");
    seed_from(dir, &source)
}

fn seed_from(dir: &Path, source: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let database = dir.join("state.db");
    if database.exists() || !source.join("state.db").is_file() {
        return Ok(());
    }
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

/// The committed Mermaid and KaTeX files (`assets/reply-render`): in the bundle, else the checkout.
fn reply_assets_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundled = exe.parent()?.parent()?.join("Resources/reply-render");
    let built = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/reply-render");
    [bundled, built]
        .into_iter()
        .find(|dir| dir.join("mermaid.js").is_file())
}

/// That folder and the address the window's own host serves it at (`/assets/`): the hidden page
/// that draws diagrams and formulas loads the libraries from there.
pub fn reply_assets() -> Option<(PathBuf, String)> {
    reply_assets_dir().map(|dir| (dir, format!("http://127.0.0.1:{PORT}")))
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
            "--label",
            "Sirus",
            "--data-dir",
        ])
        .arg(dir);
    if let Some(assets) = reply_assets_dir() {
        command.arg("--assets-dir").arg(assets);
    }
    // Started from an agent's shell, the window inherits that session's Claude variables. The
    // host and its agents must use their own login and folders, not that session's.
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CLAUDE_") || name == "CLAUDECODE" {
            command.env_remove(&key);
        }
    }
    // Usage history also reads Sirus's named account profiles (read-only, logs only).
    if let Some(accounts) = code_account_profiles() {
        command.env("SIRUS_USAGE_EXTRA_ACCOUNTS", accounts);
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

/// Sirus's `provider-accounts` folder, when it exists on this computer.
fn code_account_profiles() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HOME")?)
        .join("Library/Application Support/com.siruscode.app/provider-accounts");
    dir.is_dir().then_some(dir)
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
    let body = json!({ "code": code, "name": "Sirus" }).to_string();
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

pub fn background_path(name: &str) -> PathBuf {
    data_dir().join("backgrounds").join(name)
}

impl Backend {
    /// This Mac's own host: starts it if needed, pairs, and serves the window until it drops.
    fn session(&self) -> Result<(), String> {
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
        for event in LISTEN {
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
        socket
            .send(Message::text(Self::frame(STATE, "load_state", json!({}))))
            .map_err(|e| e.to_string())?;
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_ref() {
            stream
                .set_read_timeout(Some(Duration::from_millis(15)))
                .map_err(|e| e.to_string())?;
        }
        self.pump(&mut socket, &self.local, false, None)
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

#[cfg(test)]
mod tests {
    #[test]
    fn a_missing_sirus_code_folder_leaves_an_empty_start() {
        let root = std::env::temp_dir().join(format!("sirus-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join("data");
        assert!(seed_from(&data, &root.join("no-such-sirus-code")).is_ok());
        assert!(data.is_dir() && !data.join("state.db").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    use super::*;

    fn backend() -> (Backend, async_channel::Receiver<Incoming>) {
        let (tx, rx) = async_channel::unbounded();
        let backend = Backend {
            local: Default::default(),
            remote: Default::default(),
            active: Default::default(),
            control: Default::default(),
            listeners: Default::default(),
            notify: tx,
        };
        (backend, rx)
    }

    #[test]
    fn events_come_from_the_active_computer_and_this_macs_own_from_this_mac() {
        // Local active: local events pass, a stray remote one does not.
        assert!(event_accepted(false, false, "session-updated"));
        assert!(!event_accepted(false, true, "session-updated"));
        // Remote active: its session events pass, this Mac's do not.
        assert!(event_accepted(true, true, "session-updated"));
        assert!(!event_accepted(true, false, "session-updated"));
        // This Mac's own events keep coming from it, never from a server.
        assert!(event_accepted(true, false, "computer-state"));
        assert!(!event_accepted(true, true, "computer-state"));
        // Browser requests come from this Mac's host, and from the active computer's (ADR-201).
        assert!(event_accepted(true, false, "browser_tool_request"));
        assert!(event_accepted(false, false, "browser_tool_request"));
        assert!(event_accepted(true, true, "browser_tool_request"));
        assert!(!event_accepted(false, true, "browser_tool_request"));
    }

    #[test]
    fn a_browser_request_says_which_connection_to_answer_on() {
        let (backend, _incoming) = backend();
        let requests = backend.subscribe("browser_tool_request");
        backend.activate(Some("pc-vps"));
        let link = backend.remote_link();
        let request = |via_remote| {
            let frame = json!({ "type": "event", "event": "browser_tool_request",
                "payload": { "id": "a", "tool": "browser_status", "via": "forged" } });
            let link = if via_remote { &link } else { &backend.local };
            assert!(backend.deliver(link, via_remote, frame));
            requests.try_recv().unwrap()
        };
        assert_eq!(request(true)["via"], "remote");
        assert_eq!(request(false)["via"], "local");

        // The answer to the remote one goes to the remote link, only while it is open.
        backend.invoke_remote("browser_tool_result", json!({ "id": "a" }));
        assert!(link.outgoing.lock().unwrap().is_empty());
        link.open.store(true, Ordering::SeqCst);
        backend.invoke_remote("browser_tool_result", json!({ "id": "a" }));
        let sent = link.outgoing.lock().unwrap().pop_front().unwrap();
        assert!(sent.contains("browser_tool_result"));
        assert!(
            backend
                .local
                .outgoing
                .lock()
                .unwrap()
                .iter()
                .all(|f| !f.contains("browser_tool_result"))
        );
    }

    #[test]
    fn commands_stay_local_until_a_remote_is_active_and_never_fall_back() {
        let (backend, replies) = backend();
        backend.invoke("send_prompt", json!({}));
        assert_eq!(backend.local.outgoing.lock().unwrap().len(), 1);

        backend.activate(Some("pc-vps"));
        // This Mac's own commands still go to this Mac.
        backend.invoke("browser_tool_result", json!({}));
        backend.invoke("pick_folder", json!({}));
        assert_eq!(backend.local.outgoing.lock().unwrap().len(), 3);
        // A data command with the remote link not open fails; it is not queued locally.
        let id = backend.invoke("send_prompt", json!({}));
        assert_eq!(backend.local.outgoing.lock().unwrap().len(), 3);
        assert!(backend.remote_link().outgoing.lock().unwrap().is_empty());
        match replies.try_recv() {
            Ok(Incoming::Reply {
                id: got, ok: false, ..
            }) => assert_eq!(got, id),
            _ => panic!("expected a failed reply"),
        }
        let waiting = backend.request("load_state", json!({}));
        assert!(matches!(waiting.try_recv(), Ok(Err(_))));

        // Once the link is open, data commands go there.
        backend.remote_link().open.store(true, Ordering::SeqCst);
        backend.invoke("send_prompt", json!({}));
        assert_eq!(backend.remote_link().outgoing.lock().unwrap().len(), 1);
        assert_eq!(backend.local.outgoing.lock().unwrap().len(), 3);
    }

    #[test]
    fn dropping_the_link_fails_waiters_and_forgets_queued_frames() {
        let (backend, _replies) = backend();
        backend.activate(Some("pc"));
        backend.remote_link().open.store(true, Ordering::SeqCst);
        let waiting = backend.request("git_status", json!({}));
        backend.disconnect_remote();
        assert!(matches!(waiting.try_recv(), Ok(Err(_))));
        assert!(backend.remote_link().outgoing.lock().unwrap().is_empty());
        assert!(!backend.remote_link().open.load(Ordering::SeqCst));
    }

    #[test]
    fn the_state_of_a_computer_that_is_not_active_is_dropped() {
        let (backend, replies) = backend();
        let ready =
            json!({ "type": "result", "id": STATE, "ok": true, "value": { "projects": [] } });
        // Active is this Mac: a remote state is dropped, a local one is delivered.
        assert!(backend.deliver(&backend.remote_link(), true, ready.clone()));
        assert!(replies.try_recv().is_err());
        assert!(backend.deliver(&backend.local, false, ready.clone()));
        assert!(matches!(replies.try_recv(), Ok(Incoming::Ready(_))));
        // Active is a remote: the reverse, but a reply to this Mac's own command still arrives.
        backend.activate(Some("pc"));
        assert!(backend.deliver(&backend.local, false, ready));
        assert!(replies.try_recv().is_err());
        backend.local.always.lock().unwrap().insert(77);
        let own = json!({ "type": "result", "id": 77, "ok": true, "value": null });
        assert!(backend.deliver(&backend.local, false, own));
        assert!(matches!(
            replies.try_recv(),
            Ok(Incoming::Reply { id: 77, .. })
        ));
    }
}
