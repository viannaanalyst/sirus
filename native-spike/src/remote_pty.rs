//! A terminal whose shell runs on another computer (ADR-204). The host already runs shells for
//! the phone (`start_terminal`, `write_terminal`, `resize_terminal`, `stop_terminal`, and the
//! `pty-output` / `pty-exit` events); on a remote computer the window drives the same commands
//! over the connection it holds (`RemoteLink`) and feeds what arrives into its own engine, so the
//! grid, the keys and the links are drawn exactly as for a shell on this Mac.
//!
//! The host runs each command as its own task, so two writes sent together could land out of
//! order: one thread sends them one at a time, joining what was typed meanwhile into the next.

use crate::backend::{Backend, RemoteLink};
use crate::i18n::t;
use crate::terminal_engine::{SharedEngine, lock};
use alacritty_terminal::event::Event as TermEvent;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;

/// The events that carry a terminal's stream: only the terminal they name wants them, so the
/// backend hands them to its listeners and not to the window.
pub fn stream_event(name: &str) -> bool {
    matches!(name, "pty-output" | "pty-exit")
}

/// The terminal ids this window uses on a computer, as (its own prefix, this connection's). Every
/// terminal id starts with the connection's, so once connected the window can ask the host to stop
/// whatever it left there under its own prefix and no longer holds (`stop_terminals`).
pub fn prefixes(generation: u64) -> (String, String) {
    terminal_prefixes(install_id(), launch_id(), generation)
}

fn terminal_prefixes(install: &str, launch: u32, generation: u64) -> (String, String) {
    let prefix = format!("sirus-{install}-");
    let keep = format!("{prefix}{launch:08x}g{generation}-");
    (prefix, keep)
}

/// A fresh terminal id on the connection of `generation`.
fn next_id(generation: u64) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let (_, keep) = prefixes(generation);
    format!("{keep}{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Eight hex characters that name this Mac's windows on every computer, kept in the data folder;
/// two Macs paired with one computer never stop each other's shells.
fn install_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let path = crate::backend::data_dir().join("terminal-prefix");
        if let Some(saved) = std::fs::read_to_string(&path)
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| valid_install_id(text))
        {
            return saved;
        }
        let fresh = format!("{:08x}", random_u32());
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, &fresh);
        fresh
    })
}

fn valid_install_id(text: &str) -> bool {
    text.len() == 8 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Different at each launch, so a launch's terminals are told apart from the last one's.
fn launch_id() -> u32 {
    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *ID.get_or_init(random_u32)
}

fn random_u32() -> u32 {
    use std::io::Read;
    let mut bytes = [0u8; 4];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
    if read.is_ok() {
        return u32::from_le_bytes(bytes);
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    nanos ^ std::process::id().rotate_left(16)
}

/// What the window asks of the shell, in order.
#[derive(Debug, PartialEq, Eq)]
enum Op {
    Write(Vec<u8>),
    Resize(u16, u16),
    Stop,
}

/// One round of queued requests as few commands: writes in a row become one, and of resizes in a
/// row only the last counts. Nothing moves past a Stop.
fn coalesce(ops: Vec<Op>) -> Vec<Op> {
    let mut out: Vec<Op> = Vec::new();
    for op in ops {
        match (out.last_mut(), op) {
            (Some(Op::Stop), _) => break,
            (Some(Op::Write(bytes)), Op::Write(more)) => bytes.extend(more),
            (Some(Op::Resize(cols, rows)), Op::Resize(c, r)) => (*cols, *rows) = (c, r),
            (_, op) => out.push(op),
        }
    }
    out
}

/// What a stream event says about terminal `id`: its output, or that it ended (the host sends an
/// exit with no data, and never an empty output). `None` for another terminal's.
fn stream_for<'a>(payload: &'a Value, id: &str) -> Option<Stream<'a>> {
    if payload["terminalId"].as_str() != Some(id) {
        return None;
    }
    match payload["data"].as_str().unwrap_or("") {
        "" => Some(Stream::Exit),
        data => Some(Stream::Output(data)),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Stream<'a> {
    Output(&'a str),
    Exit,
}

/// Whether a host's error means the terminal is gone (it exited while the link was down).
fn terminal_gone(error: &Value) -> bool {
    error["code"].as_str() == Some("not_found")
}

/// Runs a command on the connection and waits for its answer, on a thread of the terminal's.
fn ask(link: &RemoteLink, cmd: &str, args: Value) -> Result<Value, Value> {
    link.request(cmd, args)
        .recv_blocking()
        .unwrap_or_else(|_| Err(Value::Null))
}

fn note(engine: &SharedEngine, text: &str) {
    lock(engine).feed(text.as_bytes());
}

/// The shell of one terminal on another computer.
pub struct RemotePty {
    ops: mpsc::Sender<Op>,
    link: RemoteLink,
    session_id: String,
    terminal_id: String,
    /// The shell ended, or never started: nothing more goes to the host.
    ended: Arc<AtomicBool>,
    stream: async_channel::Receiver<Value>,
}

impl RemotePty {
    /// Starts a shell in the conversation's folder on the computer behind `link`; its output goes
    /// into `engine`, and wakeups and its end arrive as the same events a local shell sends.
    pub fn spawn(
        backend: &Backend,
        link: RemoteLink,
        session_id: String,
        (columns, lines): (u16, u16),
        engine: SharedEngine,
        events: async_channel::Sender<TermEvent>,
    ) -> Self {
        let terminal_id = next_id(link.generation());
        let ended = Arc::new(AtomicBool::new(false));
        let (ops, queue) = mpsc::channel::<Op>();
        // Subscribed before the shell starts, so its first prompt is not missed.
        let stream = backend.subscribe_all(&["pty-output", "pty-exit"]);
        let end = {
            let ended = ended.clone();
            let engine = engine.clone();
            let events = events.clone();
            move |text: &str| {
                if !ended.swap(true, Ordering::SeqCst) {
                    note(&engine, text);
                    let _ = events.try_send(TermEvent::Exit);
                }
            }
        };
        if session_id.is_empty() {
            end(&format!(
                "\x1b[33m{}\x1b[0m\r\n",
                t("Abra uma conversa para usar o terminal neste computador.")
            ));
            stream.close();
        } else {
            let reader = {
                let stream = stream.clone();
                let terminal_id = terminal_id.clone();
                let engine = engine.clone();
                let events = events.clone();
                let ops = ops.clone();
                let end = end.clone();
                move || {
                    while let Ok(payload) = stream.recv_blocking() {
                        match stream_for(&payload, &terminal_id) {
                            None => {}
                            Some(Stream::Exit) => {
                                end("\r\n[process exited]\r\n");
                                break;
                            }
                            Some(Stream::Output(data)) => {
                                let reply = {
                                    let mut engine = lock(&engine);
                                    engine.feed(data.as_bytes());
                                    engine.take_replies()
                                };
                                if !reply.is_empty() {
                                    let _ = ops.send(Op::Write(reply));
                                }
                                let _ = events.try_send(TermEvent::Wakeup);
                            }
                        }
                    }
                }
            };
            let writer = {
                let link = link.clone();
                let ids = json!({ "sessionId": session_id, "terminalId": terminal_id });
                let ended = ended.clone();
                let end = end.clone();
                move || {
                    let mut start = ids.clone();
                    start["cols"] = json!(columns);
                    start["rows"] = json!(lines);
                    if let Err(error) = ask(&link, "start_terminal", start) {
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .map_or_else(|| error.to_string(), crate::i18n::from_host);
                        end(&format!("\x1b[31m{message}\x1b[0m\r\n"));
                        return;
                    }
                    while let Ok(first) = queue.recv() {
                        let mut batch = vec![first];
                        batch.extend(queue.try_iter());
                        for op in coalesce(batch) {
                            if ended.load(Ordering::SeqCst) {
                                return;
                            }
                            let mut args = ids.clone();
                            let (command, args) = match op {
                                Op::Stop => {
                                    link.send("stop_terminal", args);
                                    return;
                                }
                                Op::Write(bytes) => {
                                    args["data"] = json!(String::from_utf8_lossy(&bytes));
                                    ("write_terminal", args)
                                }
                                Op::Resize(cols, rows) => {
                                    args["cols"] = json!(cols);
                                    args["rows"] = json!(rows);
                                    ("resize_terminal", args)
                                }
                            };
                            // One at a time, so the host takes them in order. A dropped link
                            // loses what was sent meanwhile; the shell itself keeps running.
                            if let Err(error) = ask(&link, command, args) {
                                if terminal_gone(&error) {
                                    end("\r\n[process exited]\r\n");
                                    return;
                                }
                            }
                        }
                    }
                }
            };
            let spawned = std::thread::Builder::new()
                .name("sirus-remote-pty-read".into())
                .spawn(reader)
                .and_then(|_| {
                    std::thread::Builder::new()
                        .name("sirus-remote-pty-write".into())
                        .spawn(writer)
                });
            if let Err(error) = spawned {
                end(&format!("\x1b[31m{error}\x1b[0m\r\n"));
            }
        }
        Self {
            ops,
            link,
            session_id,
            terminal_id,
            ended,
            stream,
        }
    }

    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.ops.send(Op::Write(bytes));
    }

    pub fn resize(&self, columns: u16, lines: u16) {
        let _ = self.ops.send(Op::Resize(columns, lines));
    }

    /// Asks what holds the shell: a command line, or null while the shell itself is in front.
    pub fn foreground(&self) -> Option<async_channel::Receiver<Result<Value, Value>>> {
        if self.ended.load(Ordering::SeqCst) {
            return None;
        }
        Some(self.link.request(
            "terminal_foreground",
            json!({ "sessionId": self.session_id, "terminalId": self.terminal_id }),
        ))
    }
}

impl Drop for RemotePty {
    /// Closing the terminal ends its shell on the other computer.
    fn drop(&mut self) {
        if !self.ended.load(Ordering::SeqCst) {
            let _ = self.ops.send(Op::Stop);
        }
        self.stream.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_ids_carry_the_window_and_the_connection() {
        let (prefix, keep) = terminal_prefixes("ab12cd34", 0xfeed, 3);
        assert_eq!(prefix, "sirus-ab12cd34-");
        assert_eq!(keep, "sirus-ab12cd34-0000feedg3-");
        assert!(keep.starts_with(&prefix));
        // Within the host's limits for a terminal id: 64 characters of letters, digits, - and _.
        let id = format!("{keep}{}", u64::MAX);
        assert!(id.len() <= 64);
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        // Another launch or connection has another prefix to keep.
        assert_ne!(terminal_prefixes("ab12cd34", 0xbeef, 3).1, keep);
        assert_ne!(terminal_prefixes("ab12cd34", 0xfeed, 4).1, keep);
    }

    #[test]
    fn only_a_saved_eight_hex_id_is_taken() {
        assert!(valid_install_id("ab12cd34"));
        assert!(!valid_install_id("ab12cd3"));
        assert!(!valid_install_id("ab12cd3g"));
        assert!(!valid_install_id("ab12-d34"));
    }

    #[test]
    fn queued_requests_join_but_keep_their_order() {
        let ops = vec![
            Op::Write(b"l".to_vec()),
            Op::Write(b"s\r".to_vec()),
            Op::Resize(80, 24),
            Op::Resize(100, 30),
            Op::Write(b"x".to_vec()),
            Op::Stop,
            Op::Write(b"late".to_vec()),
        ];
        assert_eq!(
            coalesce(ops),
            vec![
                Op::Write(b"ls\r".to_vec()),
                Op::Resize(100, 30),
                Op::Write(b"x".to_vec()),
                Op::Stop,
            ]
        );
    }

    #[test]
    fn a_terminal_reads_only_its_own_stream() {
        let output = json!({ "sessionId": "s", "terminalId": "t1", "data": "hi" });
        let exit = json!({ "sessionId": "s", "terminalId": "t1", "data": "" });
        assert_eq!(stream_for(&output, "t1"), Some(Stream::Output("hi")));
        assert_eq!(stream_for(&exit, "t1"), Some(Stream::Exit));
        assert_eq!(stream_for(&output, "t2"), None);
        assert_eq!(stream_for(&json!(null), "t1"), None);
    }

    #[test]
    fn only_a_missing_terminal_ends_it() {
        assert!(terminal_gone(
            &json!({ "code": "not_found", "message": "terminal is not running" })
        ));
        assert!(!terminal_gone(
            &json!({ "code": "remote", "message": "lost" })
        ));
    }

    #[test]
    fn streams_stay_with_their_terminals() {
        assert!(stream_event("pty-output") && stream_event("pty-exit"));
        assert!(!stream_event("agent-output"));
    }
}
