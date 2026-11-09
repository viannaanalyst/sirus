use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::thread;

use crate::runtime::{AppHandle, Emitter, Manager};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

use crate::error::{Error, Result};
use crate::models::PtyOutputEvent;

pub struct PtySession {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    killer: Arc<Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>>,
    #[cfg(unix)]
    pid: Option<u32>,
    pub id: String,
    /// Owning session. Every command checks it before touching this PTY.
    pub session_id: String,
    /// Terminal tab identity. Unique per session.
    pub terminal_id: String,
}

#[derive(Default)]
struct Utf8Stream {
    pending: Vec<u8>,
}
impl Utf8Stream {
    fn decode(&mut self, bytes: &[u8], eof: bool) -> String {
        self.pending.extend_from_slice(bytes);
        let mut result = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    result.push_str(text);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    result.push_str(std::str::from_utf8(&self.pending[..valid]).unwrap());
                    self.pending.drain(..valid);
                    match error.error_len() {
                        Some(length) => {
                            self.pending.drain(..length);
                            result.push('\u{fffd}');
                        }
                        None => {
                            if eof {
                                result.push('\u{fffd}');
                                self.pending.clear();
                            }
                            break;
                        }
                    }
                }
            }
        }
        result
    }
}

#[cfg(unix)]
fn signal_owned_sessions(sessions: &[libc::pid_t], signal: libc::c_int) -> Result<()> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid="])
        .output()
        .map_err(|error| {
            Error::new(
                "pty",
                format!("cannot inspect owned terminal processes: {error}"),
            )
        })?;
    if !output.status.success() || output.stdout.len() > 1024 * 1024 {
        return Err(Error::new(
            "pty",
            "cannot inspect owned terminal processes safely",
        ));
    }
    let mut result = Ok(());
    for process in String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .filter_map(|value| value.parse::<libc::pid_t>().ok())
        .filter(|pid| *pid > 0)
    {
        // Recheck immediately before the signal, rejecting other sessions.
        // POSIX still has a small PID check/use window; this is not a pidfd API.
        if !sessions.contains(&unsafe { libc::getsid(process) }) {
            continue;
        }
        let code = unsafe { libc::kill(process, signal) };
        if code != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) && result.is_ok() {
                result = Err(Error::new("pty", error.to_string()));
            }
        }
    }
    result
}

/// Closes several terminals with one process scan per signal and one shared
/// grace period, so cleanup cost does not grow with the number of shells.
pub fn kill_all(terminals: &[&PtySession]) -> Result<()> {
    let mut result = Ok(());
    #[cfg(unix)]
    let mut sessions = Vec::new();
    for terminal in terminals {
        #[cfg(unix)]
        if let Some(pid) = terminal.pid.filter(|pid| *pid > 0) {
            sessions.push(pid as libc::pid_t);
            continue;
        }
        if let Err(error) = terminal.killer.lock().kill() {
            if result.is_ok() {
                result = Err(Error::new("pty", error.to_string()));
            }
        }
    }
    #[cfg(unix)]
    if !sessions.is_empty() {
        // Job control gives background processes separate groups. Enumerate only
        // numeric PIDs, then authorize every signal against our owned session IDs.
        // Detached processes with a different session ID are deliberately excluded.
        let hangup = signal_owned_sessions(&sessions, libc::SIGHUP);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let forced = signal_owned_sessions(&sessions, libc::SIGKILL);
        result = result.and(hangup).and(forced);
    }
    result
}

/// Input side of one PTY, usable without holding the terminal map lock: a
/// blocking write must never stop the reader from draining output.
#[derive(Clone)]
pub struct PtyWriter(Arc<Mutex<Box<dyn Write + Send>>>);

impl PtyWriter {
    pub fn write(&self, data: &str) -> Result<()> {
        self.0
            .lock()
            .write_all(data.as_bytes())
            .map_err(|err| Error::new("pty", err.to_string()))
    }
}

impl PtySession {
    pub fn writer(&self) -> PtyWriter {
        PtyWriter(self.writer.clone())
    }

    #[cfg(test)]
    pub fn write(&self, data: &str) -> Result<()> {
        self.writer().write(data)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .lock()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| Error::new("pty", err.to_string()))
    }

    pub fn kill(&self) -> Result<()> {
        kill_all(&[self])
    }
}

pub fn start(
    app: AppHandle,
    session_id: String,
    terminal_id: String,
    cwd: String,
    cols: u16,
    rows: u16,
    secrets: Vec<(String, String)>,
) -> Result<PtySession> {
    let system = native_pty_system();
    let pair = system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| Error::new("pty", err.to_string()))?;

    let shell = crate::detect::default_shell();
    let mut cmd = CommandBuilder::new(shell);
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    // Apps opened from Finder get no locale, and zsh then prints accents as `\M-^C`.
    if let Some(lang) = utf8_lang(
        std::env::var("LANG").ok().as_deref(),
        std::env::var("LC_ALL").ok().as_deref(),
        std::env::var("LC_CTYPE").ok().as_deref(),
    ) {
        cmd.env("LANG", lang);
    }
    cmd.env("COLORTERM", "truecolor");
    // The project's Infisical keys; the terminal's own variables are never replaced. The values
    // only move into the environment.
    for (key, value) in project_keys(secrets) {
        cmd.env(key, value);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|err| Error::new("pty", err.to_string()))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|err| Error::new("pty", err.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|err| Error::new("pty", err.to_string()))?;

    let killer = child.clone_killer();
    #[cfg(unix)]
    let pid = child.process_id();
    let id = uuid::Uuid::new_v4().to_string();
    let reader_id = id.clone();
    let session_for_read = session_id.clone();
    let terminal_for_read = terminal_id.clone();
    // The reader only reads; the emitter drains whatever arrived meanwhile into one
    // event. Heavy output batches itself without timers, idle output stays immediate.
    let (chunks, received) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if chunks.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    tracing::debug!(%error, "PTY reader closed");
                    break;
                }
            }
        }
    });
    thread::spawn(move || {
        let mut decoder = Utf8Stream::default();
        let mut eof = false;
        while !eof {
            let mut bytes = match received.recv() {
                Ok(bytes) => bytes,
                Err(_) => {
                    eof = true;
                    Vec::new()
                }
            };
            while !eof && bytes.len() < MAX_BATCH {
                match received.try_recv() {
                    Ok(more) => bytes.extend_from_slice(&more),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => eof = true,
                }
            }
            let data = decoder.decode(&bytes, eof);
            let state = app.state::<Arc<crate::commands::AppState>>();
            // Check ownership, then emit without holding the terminal map lock.
            let current = state
                .ptys
                .lock()
                .get(&terminal_for_read)
                .is_some_and(|pty| pty.id == reader_id);
            if current && !data.is_empty() {
                let _ = app.emit(
                    "pty-output",
                    PtyOutputEvent {
                        session_id: session_for_read.clone(),
                        terminal_id: terminal_for_read.clone(),
                        data,
                    },
                );
            }
        }
        let _ = child.wait();
        let state = app.state::<Arc<crate::commands::AppState>>();
        let mut ptys = state.ptys.lock();
        // A delayed exit from an old PTY must never remove a reopened terminal.
        let current = ptys
            .get(&terminal_for_read)
            .is_some_and(|pty| pty.id == reader_id);
        if current {
            ptys.remove(&terminal_for_read);
        }
        if !current {
            return;
        }
        let _ = app.emit(
            "pty-exit",
            PtyOutputEvent {
                session_id: session_for_read,
                terminal_id: terminal_for_read,
                data: String::new(),
            },
        );
    });

    Ok(PtySession {
        writer: Arc::new(Mutex::new(writer)),
        master: Arc::new(Mutex::new(pair.master)),
        killer: Arc::new(Mutex::new(killer)),
        #[cfg(unix)]
        pid,
        id,
        session_id,
        terminal_id,
    })
}

/// Variables the terminal sets itself; a project's key never replaces them.
const OWN_ENV: [&str; 4] = ["TERM", "COLORTERM", "TERM_PROGRAM", "PWD"];

/// The keys that may go into the shell's environment.
fn project_keys(secrets: Vec<(String, String)>) -> Vec<(String, String)> {
    secrets
        .into_iter()
        .filter(|(key, _)| !OWN_ENV.contains(&key.as_str()))
        .collect()
}

/// Largest single `pty-output` payload assembled from queued reads.
const MAX_BATCH: usize = 64 * 1024;

/// Keyed by terminal id. Each entry is owner-checked against its session.
pub type PtyMap = HashMap<String, PtySession>;

/// A UTF-8 `LANG` for the shell when the environment does not already select
/// a UTF-8 character set (through `LC_ALL`, `LC_CTYPE` or `LANG`).
pub fn utf8_lang(
    lang: Option<&str>,
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
) -> Option<String> {
    let utf8 = |value: &str| {
        let lower = value.to_ascii_lowercase();
        lower.contains("utf-8") || lower.contains("utf8")
    };
    let chosen = [lc_all, lc_ctype, lang]
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty());
    // An explicit LC_ALL wins over LANG anyway, so it is the person's choice to keep.
    if chosen.is_some_and(utf8) || lc_all.is_some_and(|value| !value.is_empty()) {
        return None;
    }
    // Keep the person's language and region when one is set, only switching the encoding.
    let base = lang
        .filter(|value| !value.is_empty() && *value != "C" && *value != "POSIX")
        .map(|value| value.split(['.', '@']).next().unwrap_or(value).to_string())
        .filter(|value| {
            value.len() >= 2 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
    Some(format!("{}.UTF-8", base.unwrap_or_else(|| "en_US".into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_keys_never_replace_the_terminals_own_variables() {
        let kept = project_keys(vec![
            ("TERM".into(), "dumb".into()),
            ("COLORTERM".into(), "x".into()),
            ("TERM_PROGRAM".into(), "x".into()),
            ("PWD".into(), "/tmp".into()),
            ("API_KEY".into(), "abc".into()),
        ]);
        assert_eq!(kept, vec![("API_KEY".to_owned(), "abc".to_owned())]);
    }

    #[test]
    fn shells_get_a_utf8_locale_only_when_missing() {
        assert_eq!(utf8_lang(None, None, None).as_deref(), Some("en_US.UTF-8"));
        assert_eq!(
            utf8_lang(Some("C"), None, None).as_deref(),
            Some("en_US.UTF-8")
        );
        assert_eq!(
            utf8_lang(Some("pt_BR.ISO8859-1"), None, None).as_deref(),
            Some("pt_BR.UTF-8")
        );
        assert_eq!(utf8_lang(Some("pt_BR.UTF-8"), None, None), None);
        assert_eq!(utf8_lang(None, Some("en_US.UTF-8"), None), None);
        assert_eq!(utf8_lang(None, None, Some("UTF-8")), None);
        assert_eq!(utf8_lang(Some("pt_BR.UTF-8"), Some("C"), None), None);
    }
    #[cfg(unix)]
    #[test]
    fn terminal_close_terminates_shell_ignoring_hangup() {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "trap '' HUP; printf READY; while :; do sleep 1; done"]);
        let mut child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut ready = [0u8; 5];
        reader.read_exact(&mut ready).unwrap();
        assert_eq!(&ready, b"READY");
        let terminal = PtySession {
            writer: Arc::new(Mutex::new(pair.master.take_writer().unwrap())),
            master: Arc::new(Mutex::new(pair.master)),
            killer: Arc::new(Mutex::new(child.clone_killer())),
            pid: child.process_id(),
            id: "owned-test".into(),
            session_id: "owned-session".into(),
            terminal_id: "owned-test".into(),
        };
        terminal.kill().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                unsafe {
                    libc::kill(child.process_id().unwrap() as i32, libc::SIGKILL);
                }
                child.wait().unwrap();
                panic!("terminal shell survived Close");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    #[cfg(unix)]
    #[test]
    fn terminal_close_hangs_up_normal_background_jobs() {
        use std::io::BufRead;
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new("/bin/zsh");
        command.args(["-f"]);
        let mut child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(reader.take(8192))
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let Some(value) = line.split("SIRUS_BG:").last() {
                    if let Ok(pid) = value.trim().parse::<i32>() {
                        let _ = sender.send(pid);
                    }
                }
            }
        });
        let shell = child.process_id().unwrap() as i32;
        let terminal = PtySession {
            writer: Arc::new(Mutex::new(pair.master.take_writer().unwrap())),
            master: Arc::new(Mutex::new(pair.master)),
            killer: Arc::new(Mutex::new(child.clone_killer())),
            pid: child.process_id(),
            id: "owned-job-test".into(),
            session_id: "owned-session".into(),
            terminal_id: "owned-job-test".into(),
        };
        terminal
            .write("setopt HUP; sleep 30 & print -r -- SIRUS_BG:$!\n")
            .unwrap();
        let job = receiver.recv_timeout(std::time::Duration::from_secs(2));
        let groups = job.as_ref().ok().map(|job| {
            (unsafe { libc::getsid(*job) }, unsafe {
                libc::getpgid(*job)
            })
        });
        let mut unrelated = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        assert_ne!(unsafe { libc::getsid(unrelated.id() as i32) }, shell);
        terminal.kill().unwrap();
        let unrelated_alive = unrelated.try_wait().unwrap().is_none();
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        assert!(
            unrelated_alive,
            "cleanup must not signal processes from another session"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while child.try_wait().unwrap().is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "owned terminal shell survived Close"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let job = job.expect("owned shell must report its background job");
        assert_eq!(groups.unwrap().0, shell);
        assert_ne!(groups.unwrap().1, shell);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while unsafe { libc::kill(job, 0) } == 0 {
            if std::time::Instant::now() >= deadline {
                if unsafe { libc::getsid(job) } == shell {
                    unsafe {
                        libc::kill(job, libc::SIGKILL);
                    }
                }
                let state = std::process::Command::new("ps")
                    .args(["-p", &job.to_string(), "-o", "stat=,command="])
                    .output()
                    .unwrap();
                panic!(
                    "normal background job survived terminal Close: {}",
                    String::from_utf8_lossy(&state.stdout)
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn preserves_multibyte_text_between_pty_reads() {
        let mut decoder = Utf8Stream::default();
        let text = "ação 🚂\u{1b}[0m";
        let mut output = String::new();
        for byte in text.as_bytes() {
            output.push_str(&decoder.decode(&[*byte], false));
        }
        output.push_str(&decoder.decode(&[], true));
        assert_eq!(output, text);
    }
    #[test]
    fn replaces_invalid_bytes_without_losing_following_output() {
        let mut decoder = Utf8Stream::default();
        assert_eq!(decoder.decode(&[0xff, b'a', 0xf0], false), "\u{fffd}a");
        assert_eq!(decoder.decode(&[], true), "\u{fffd}");
    }
}
