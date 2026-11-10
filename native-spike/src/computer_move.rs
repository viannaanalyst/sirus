//! Moving a conversation to another computer (ADR-133, "Move to"). The window is on the source
//! computer; the target can be this Mac or any paired computer, so the steps run on two peers:
//! (1) the project on the target, found by its GitHub address or cloned there, (2) the
//! conversation's work pushed from the source as `sirus/move-…`, (3) `import_session` on the
//! target, (4) `mark_session_moved` on the source. The host commands validate and do the work;
//! this module only orders the calls and stops at the first failure.
use crate::i18n::t;
use crate::remote_tls::{Address, TlsStream};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tungstenite::{Message, WebSocket};

/// One computer's command surface, as far as a move needs it.
pub trait Peer {
    fn call(&self, command: &str, args: Value) -> Result<Value, String>;
}

/// The window's own backend as a peer: the active computer, or with `local` this Mac whatever
/// computer is active.
pub struct Linked {
    pub backend: crate::backend::Backend,
    pub local: bool,
}

impl Peer for Linked {
    fn call(&self, command: &str, args: Value) -> Result<Value, String> {
        let reply = if self.local {
            self.backend.request_local(command, args)
        } else {
            self.backend.request(command, args)
        };
        match reply.recv_blocking() {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(message_of(&error)),
            Err(_) => Err(t("A conexão com o computador caiu.").to_owned()),
        }
    }
}

/// A short-lived connection to a paired computer that is not the window's active one.
pub struct Oneshot {
    socket: std::cell::RefCell<WebSocket<TlsStream>>,
    next: std::cell::Cell<u64>,
}

impl Oneshot {
    pub fn open(address: &Address, token: &str) -> Result<Oneshot, String> {
        let mut socket = crate::remote_tls::open_socket(address)?;
        socket
            .send(Message::text(
                json!({ "type": "hello", "token": token }).to_string(),
            ))
            .map_err(|e| e.to_string())?;
        loop {
            match socket.read().map_err(|e| e.to_string())? {
                Message::Text(text) => {
                    let message: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                    match message["type"].as_str() {
                        Some("ready") => break,
                        Some("unauthorized") => {
                            return Err(
                                t("Esse computador não reconhece mais este Mac.").to_owned()
                            );
                        }
                        _ => {}
                    }
                }
                Message::Close(_) => return Err(t("A conexão com o computador caiu.").to_owned()),
                _ => {}
            }
        }
        Ok(Oneshot {
            socket: socket.into(),
            next: 1.into(),
        })
    }
}

impl Peer for Oneshot {
    fn call(&self, command: &str, args: Value) -> Result<Value, String> {
        let id = self.next.get();
        self.next.set(id + 1);
        let mut socket = self.socket.borrow_mut();
        let frame = json!({ "type": "invoke", "id": id, "cmd": command, "args": args });
        socket
            .send(Message::text(frame.to_string()))
            .map_err(|e| e.to_string())?;
        // A clone or an import can take minutes; reads wake every 10 seconds to check the clock.
        let deadline = Instant::now() + Duration::from_secs(20 * 60);
        loop {
            match socket.read() {
                Ok(Message::Text(text)) => {
                    let mut message: Value =
                        serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                    if message["type"] == "result" && message["id"] == id {
                        return if message["ok"].as_bool() == Some(true) {
                            Ok(message["value"].take())
                        } else {
                            Err(message_of(&message["error"]))
                        };
                    }
                }
                Ok(Message::Close(_)) => {
                    return Err(t("A conexão com o computador caiu.").to_owned());
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if Instant::now() > deadline {
                        return Err(t("O computador demorou demais para responder.").to_owned());
                    }
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

pub(crate) fn message_of(error: &Value) -> String {
    error["message"]
        .as_str()
        .or_else(|| error.as_str())
        .unwrap_or_else(|| t("O computador recusou o pedido."))
        .to_owned()
}

pub struct Plan {
    pub session_id: String,
    pub project_id: String,
    /// The computers' names as the person calls them, for the notes on both conversations.
    pub source_label: String,
    pub target_label: String,
}

/// What the target now has.
#[derive(Debug, PartialEq)]
pub struct Moved {
    pub session_id: String,
    pub project_id: String,
}

/// Where a clone lands on a computer: next to a Mac's own projects, or `~/projects` on a server.
fn clone_parent(info: &Value) -> &'static str {
    let capabilities = &info["capabilities"];
    let server = capabilities["headless"].as_bool() == Some(true)
        || capabilities["platform"]
            .as_str()
            .is_some_and(|p| p != "macos");
    if server { "~/projects" } else { "~/Projetos" }
}

/// Runs the four steps. Each failure comes back as the host's own message; nothing is rolled
/// back by this side (the host removes what a failed import made).
pub fn run(source: &dyn Peer, target: &dyn Peer, plan: &Plan) -> Result<Moved, String> {
    let remote = source.call(
        "project_remote_url",
        json!({ "projectId": plan.project_id }),
    )?;
    let Some(remote_url) = remote.as_str().filter(|u| !u.is_empty()) else {
        return Err(t("Este projeto não tem um remote do GitHub (origin): publique-o no GitHub antes de mover.").to_owned());
    };
    let found = target.call("find_project_by_remote", json!({ "remoteUrl": remote_url }))?;
    let project = if found["id"].is_string() {
        found
    } else {
        let info = target.call("host_info", json!({})).unwrap_or(Value::Null);
        target.call(
            "clone_project",
            json!({ "source": remote_url, "parent": clone_parent(&info), "folder": null }),
        )?
    };
    let project_id = project["id"]
        .as_str()
        .ok_or(t("O computador não devolveu o projeto."))?
        .to_owned();
    let exported = source.call(
        "prepare_session_move",
        json!({ "sessionId": plan.session_id, "target": plan.target_label, "confirm": true }),
    )?;
    let created = target.call(
        "import_session",
        json!({ "request": {
            "projectId": project_id,
            "branch": exported["branch"],
            "title": exported["title"],
            "agent": exported["agent"],
            "model": exported["model"],
            "messages": exported["messages"],
            "fromComputer": plan.source_label,
        }}),
    )?;
    let session_id = created["id"]
        .as_str()
        .ok_or(t("O computador não devolveu a conversa."))?
        .to_owned();
    // Noting it on the source is a courtesy: the conversation already lives on the target.
    let _ = source.call(
        "mark_session_moved",
        json!({ "sessionId": plan.session_id, "target": plan.target_label, "branch": exported["branch"] }),
    );
    Ok(Moved {
        session_id,
        project_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        name: &'static str,
        log: std::rc::Rc<RefCell<Vec<String>>>,
        replies: Vec<(&'static str, Result<Value, String>)>,
    }

    impl Peer for Fake {
        fn call(&self, command: &str, _: Value) -> Result<Value, String> {
            self.log
                .borrow_mut()
                .push(format!("{}:{command}", self.name));
            self.replies
                .iter()
                .find(|(c, _)| *c == command)
                .map_or(Ok(Value::Null), |(_, r)| r.clone())
        }
    }

    fn plan() -> Plan {
        Plan {
            session_id: "s1".into(),
            project_id: "p1".into(),
            source_label: "Este Mac".into(),
            target_label: "vps".into(),
        }
    }

    #[test]
    fn a_move_finds_the_project_pushes_imports_then_marks() {
        let log = std::rc::Rc::new(RefCell::new(Vec::new()));
        let source = Fake {
            name: "src",
            log: log.clone(),
            replies: vec![
                ("project_remote_url", Ok(json!("https://github.com/o/r"))),
                (
                    "prepare_session_move",
                    Ok(
                        json!({"branch":"sirus/move-s1","title":"T","agent":"claude","model":null,"messages":[]}),
                    ),
                ),
            ],
        };
        let target = Fake {
            name: "dst",
            log: log.clone(),
            replies: vec![
                ("find_project_by_remote", Ok(json!({"id":"tp"}))),
                ("import_session", Ok(json!({"id":"new"}))),
            ],
        };
        let moved = run(&source, &target, &plan()).unwrap();
        assert_eq!(
            moved,
            Moved {
                session_id: "new".into(),
                project_id: "tp".into()
            }
        );
        assert_eq!(
            *log.borrow(),
            [
                "src:project_remote_url",
                "dst:find_project_by_remote",
                "src:prepare_session_move",
                "dst:import_session",
                "src:mark_session_moved"
            ]
        );
    }

    #[test]
    fn a_missing_project_is_cloned_and_a_failure_stops_the_move() {
        let log = std::rc::Rc::new(RefCell::new(Vec::new()));
        let source = Fake {
            name: "src",
            log: log.clone(),
            replies: vec![
                ("project_remote_url", Ok(json!("https://github.com/o/r"))),
                (
                    "prepare_session_move",
                    Err("Stop the conversation first.".into()),
                ),
            ],
        };
        let target = Fake {
            name: "dst",
            log: log.clone(),
            replies: vec![
                ("find_project_by_remote", Ok(Value::Null)),
                (
                    "host_info",
                    Ok(json!({"capabilities":{"headless":true,"platform":"linux"}})),
                ),
                ("clone_project", Ok(json!({"id":"cloned"}))),
            ],
        };
        let error = run(&source, &target, &plan()).unwrap_err();
        assert_eq!(error, "Stop the conversation first.");
        let log = log.borrow();
        assert!(log.contains(&"dst:clone_project".to_owned()));
        assert!(
            !log.iter()
                .any(|c| c.ends_with("import_session") || c.ends_with("mark_session_moved"))
        );
    }

    #[test]
    fn no_github_remote_refuses_before_touching_the_target() {
        let log = std::rc::Rc::new(RefCell::new(Vec::new()));
        let source = Fake {
            name: "src",
            log: log.clone(),
            replies: vec![("project_remote_url", Ok(Value::Null))],
        };
        let target = Fake {
            name: "dst",
            log: log.clone(),
            replies: vec![],
        };
        assert!(run(&source, &target, &plan()).is_err());
        assert_eq!(*log.borrow(), ["src:project_remote_url"]);
        assert_eq!(
            clone_parent(&json!({"capabilities":{"platform":"macos"}})),
            "~/Projetos"
        );
        assert_eq!(
            clone_parent(&json!({"capabilities":{"headless":true}})),
            "~/projects"
        );
    }
}
