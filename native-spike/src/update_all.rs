//! "Atualizar tudo" (ADR-207): one place that updates this Mac's app, the provider CLIs on this Mac
//! and on every paired computer, and the Sirus host on each paired Linux computer, with a row per
//! machine in Settings → Avançado → Atualizações.
//!
//! The work runs off the interface, one thread per computer: this Mac's host through the window's
//! own connection, every paired computer through a short-lived connection of its own
//! (`computer_move::Oneshot`), so a computer does not need to be the active one. Each thread sends
//! its rows' steps as `update_all` on the shell's native channel; the shell keeps them in `Board`
//! and the page draws them. The app itself goes through app_update.rs as before.
use crate::computer_move::{Linked, Oneshot, Peer};
use crate::i18n::{t, tf};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How often a host's update is asked about while it runs.
const POLL: Duration = Duration::from_secs(4);
/// A release build of the host takes minutes on a small server; past this the row stops waiting.
const POLL_FOR: Duration = Duration::from_secs(45 * 60);

/// Where one row stands.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum Step {
    #[default]
    Unknown,
    Checking,
    Updating,
    /// A new host is being compiled.
    Building,
    Installing,
    /// Nothing to do; the detail (a version or a commit) may be empty.
    UpToDate(String),
    /// Updates on offer, as one line.
    Available(String),
    /// Done; the line says what changed.
    Done(String),
    /// A new host is ready and waits for this many conversations to finish.
    Waiting(u64),
    /// The new host failed its health check and the previous one is back.
    RolledBack(String),
    Failed(String),
    /// Nothing to update here, and why.
    NotHere(String),
}

impl Step {
    /// The row has stopped moving: a thread watching it can let go.
    pub fn settled(&self) -> bool {
        !matches!(
            self,
            Step::Unknown | Step::Checking | Step::Updating | Step::Building | Step::Installing
        )
    }

    /// The row's line, in the app's language.
    pub fn text(&self) -> String {
        match self {
            Step::Unknown => "—".to_owned(),
            Step::Checking => t("Buscando atualização…").to_owned(),
            Step::Updating => t("Atualizando…").to_owned(),
            Step::Building => t("Compilando a versão nova…").to_owned(),
            Step::Installing => t("Instalando…").to_owned(),
            Step::UpToDate(detail) if detail.is_empty() => t("Atualizado").to_owned(),
            Step::UpToDate(detail) => tf("Atualizado · {}", &[detail]),
            Step::Available(line) | Step::Done(line) | Step::NotHere(line) => line.clone(),
            Step::Waiting(1) => t("Esperando 1 conversa terminar").to_owned(),
            Step::Waiting(n) => tf("Esperando {} conversas terminarem", &[n]),
            Step::RolledBack(message) if message.is_empty() => {
                t("Voltou para a versão anterior").to_owned()
            }
            Step::RolledBack(message) => tf(
                "Voltou para a versão anterior: {}",
                &[&crate::i18n::from_host(message)],
            ),
            Step::Failed(message) => tf("Falhou: {}", &[&crate::i18n::from_host(message)]),
        }
    }
}

/// Every row's step, by row key (`row_key`), and how many threads are still at work.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Board {
    pub rows: BTreeMap<String, Step>,
    /// Threads updating something.
    pub workers: usize,
    /// Threads only reading where things stand (`Job::probe`); they do not make a run.
    pub probes: usize,
}

impl Board {
    pub fn running(&self) -> bool {
        self.workers > 0
    }

    pub fn step(&self, key: &str) -> Step {
        self.rows.get(key).cloned().unwrap_or_default()
    }

    /// The board as the Settings page reads it from its data.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_value(value: &Value) -> Board {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }
}

/// `providers:<computer>` or `host:<computer>`; this Mac is `computers::HOME`.
pub fn row_key(part: &str, computer: &str) -> String {
    format!("{part}:{computer}")
}

/// What a thread sends the shell.
pub fn message(row: &str, step: &Step) -> Value {
    json!({ "row": row, "step": step })
}

/// What one thread does on one computer.
#[derive(Clone, Debug, Default)]
pub struct Job {
    pub computer: String,
    pub name: String,
    /// Update the provider CLIs: those named, or (None) every one with a one-click update.
    pub providers: Option<Option<Vec<String>>>,
    /// Ask the host's updater: `Some(true)` installs even while conversations run.
    pub host: Option<bool>,
    /// Only read where things stand; nothing is updated.
    pub probe: bool,
}

/// Where the thread connects.
pub enum Target {
    /// This Mac, through the window's own connection.
    Mac(crate::backend::Backend),
    Remote {
        address: crate::remote_tls::Address,
        token: String,
    },
}

type Out = async_channel::Sender<(&'static str, Value)>;

/// Runs a job on its own thread; its last message is `{"ended": computer}`. Every message of a
/// probe carries `"probe": true`.
pub fn spawn(job: Job, target: Target, out: Out) {
    std::thread::spawn(move || {
        let send = |mut value: Value| {
            value["probe"] = json!(job.probe);
            let _ = out.send_blocking(("update_all", value));
        };
        run(&job, target, &send);
        send(json!({ "ended": job.computer }));
    });
}

fn run(job: &Job, target: Target, send: &dyn Fn(Value)) {
    let providers_row = row_key("providers", &job.computer);
    let host_row = row_key("host", &job.computer);
    let wants_host = job.probe || job.host.is_some();
    let open = |target: &Target| -> Result<Box<dyn Peer>, String> {
        match target {
            Target::Mac(backend) => Ok(Box::new(Linked {
                backend: backend.clone(),
                local: true,
            })),
            Target::Remote { address, token } => {
                Ok(Box::new(Oneshot::open(address, token)?) as Box<dyn Peer>)
            }
        }
    };
    let peer = match open(&target) {
        Ok(peer) => peer,
        Err(error) => {
            let failed = Step::Failed(error);
            if job.providers.is_some() || job.probe {
                send(message(&providers_row, &failed));
            }
            if wants_host && matches!(target, Target::Remote { .. }) {
                send(message(&host_row, &failed));
            }
            return;
        }
    };
    let remote = matches!(target, Target::Remote { .. });
    // The host first: its build takes the longest, and the CLIs update while it compiles.
    let mut requested_at = None;
    if remote && wants_host {
        let action = match job.host {
            Some(now) if !job.probe => json!({ "type": "request", "now": now }),
            _ => json!({ "type": "status" }),
        };
        if !job.probe {
            send(message(&host_row, &Step::Checking));
        }
        match peer.call("host_update_action", json!({ "action": action })) {
            Ok(status) => {
                requested_at = status["requestedAt"].as_i64();
                let step = host_step(&status, requested_at);
                send(message(&host_row, &step));
                if step.settled() {
                    // Nothing to watch: done (or the request was refused before it was written).
                    requested_at = None;
                    ask_if_waiting(job, &step, send);
                }
            }
            Err(error) => send(message(&host_row, &host_error(&error))),
        }
    }
    if let Some(only) = &job.providers {
        if !job.probe {
            send(message(&providers_row, &Step::Checking));
        }
        let step = providers(peer.as_ref(), only.clone(), job.probe, &providers_row, send);
        send(message(&providers_row, &step));
    } else if job.probe {
        let step = providers(peer.as_ref(), None, true, &providers_row, send);
        send(message(&providers_row, &step));
    }
    let Some(requested_at) = requested_at else {
        return;
    };
    // Watch the host's update until it settles. A host that restarts drops the connection; the
    // next round connects again.
    let deadline = Instant::now() + POLL_FOR;
    let mut connected = Some(peer);
    let mut last = Step::Checking;
    while Instant::now() < deadline {
        std::thread::sleep(POLL);
        if connected.is_none() {
            connected = open(&target).ok();
        }
        let Some(link) = connected.as_ref() else {
            continue;
        };
        match link.call(
            "host_update_action",
            json!({ "action": { "type": "status" } }),
        ) {
            Ok(status) => {
                let step = host_step(&status, Some(requested_at));
                if step != last {
                    send(message(&host_row, &step));
                    last = step.clone();
                }
                if step.settled() {
                    ask_if_waiting(job, &step, send);
                    return;
                }
            }
            Err(_) => connected = None,
        }
    }
    send(message(
        &host_row,
        &Step::Failed(t("O computador demorou demais para responder.").to_owned()),
    ));
}

/// A build that waits for conversations, after an update the person started without `now`: the
/// shell asks whether to install now.
fn ask_if_waiting(job: &Job, step: &Step, send: &dyn Fn(Value)) {
    if let (Step::Waiting(count), Some(false), false) = (step, job.host, job.probe) {
        send(json!({ "ask": { "computer": job.computer, "name": job.name, "count": count } }));
    }
}

/// A host older than `host_update_action` answers that it does not know it.
fn host_error(error: &str) -> Step {
    if error.contains("host_update_action") || error.to_ascii_lowercase().contains("unknown") {
        Step::NotHere(t("Este computador ainda não sabe atualizar pelo app.").to_owned())
    } else {
        Step::Failed(error.to_owned())
    }
}

/// A row's step from the host's `host_update_action` answer. With a request out, a status older
/// than it (or a request not yet picked up) is still the check starting.
pub fn host_step(status: &Value, requested_at: Option<i64>) -> Step {
    if status["supported"].as_bool() != Some(true) {
        return Step::NotHere(t("O Sirus deste computador atualiza junto com o app.").to_owned());
    }
    if status["updater"].as_bool() != Some(true) {
        return Step::NotHere(t("O atualizador não está instalado neste computador.").to_owned());
    }
    let at = status["at"].as_i64();
    if let Some(asked) = requested_at
        && (status["requestPending"].as_bool() == Some(true) || at.is_none_or(|at| at < asked))
    {
        return Step::Checking;
    }
    let short = |key: &str| {
        status[key]
            .as_str()
            .map(|commit| commit.chars().take(8).collect::<String>())
            .unwrap_or_default()
    };
    let message = status["message"].as_str().unwrap_or("").to_owned();
    match status["phase"].as_str() {
        Some("checking") => Step::Checking,
        Some("building") => Step::Building,
        Some("installing") => Step::Installing,
        Some("waiting") => Step::Waiting(status["waitingFor"].as_u64().unwrap_or(1).max(1)),
        Some("installed") => Step::Done(tf("Instalado · {}", &[&short("commit")])),
        Some("rolledBack") => Step::RolledBack(message),
        Some("failed") => Step::Failed(if message.is_empty() {
            t("A atualização não terminou.").to_owned()
        } else {
            message
        }),
        _ => Step::UpToDate(short("running")),
    }
}

/// Checks a computer's CLIs and, unless `probe`, updates `only` (or every one with a one-click
/// update). The Mac's run also sends its raw results, for the provider cards (toast.rs).
fn providers(
    peer: &dyn Peer,
    only: Option<Vec<String>>,
    probe: bool,
    row: &str,
    send: &dyn Fn(Value),
) -> Step {
    let list = match peer.call("provider_updates", json!({ "refresh": !probe })) {
        Ok(list) => list,
        Err(error) => return Step::Failed(error),
    };
    let offers = offers(&list);
    if probe {
        return available_step(&offers);
    }
    let chosen: Vec<String> = match only {
        Some(named) => named,
        None => offers
            .iter()
            .filter(|offer| offer.command)
            .map(|offer| offer.provider.clone())
            .collect(),
    };
    if chosen.is_empty() {
        return available_step(&offers);
    }
    send(message(row, &Step::Updating));
    let reply = peer.call(
        "update_providers",
        json!({ "providers": chosen, "confirm": true }),
    );
    send(json!({
        "row": row,
        "providers": chosen,
        "ok": reply.is_ok(),
        "results": match &reply { Ok(value) => value.clone(), Err(error) => json!({ "message": error }) },
    }));
    match reply {
        Ok(results) => results_step(&results),
        Err(error) => Step::Failed(error),
    }
}

/// A CLI with a newer version.
#[derive(Debug, PartialEq)]
pub struct Offer {
    pub provider: String,
    pub installed: String,
    pub latest: String,
    /// Sirus can run its update.
    pub command: bool,
}

pub fn offers(list: &Value) -> Vec<Offer> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["updateAvailable"].as_bool() == Some(true))
        .filter_map(|entry| {
            Some(Offer {
                provider: entry["provider"]
                    .as_str()
                    .filter(|s| !s.is_empty())?
                    .to_owned(),
                installed: entry["installedVersion"].as_str().unwrap_or("").to_owned(),
                latest: entry["latestVersion"].as_str().unwrap_or("").to_owned(),
                command: entry["updateSupported"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

fn available_step(offers: &[Offer]) -> Step {
    if offers.is_empty() {
        return Step::UpToDate(String::new());
    }
    let line = offers
        .iter()
        .map(|offer| {
            let name = crate::shell::provider_title(&offer.provider);
            if offer.command {
                format!("{name} {} → {}", offer.installed, offer.latest)
            } else {
                tf(
                    "{} {} (atualize por fora do Sirus)",
                    &[&name, &offer.latest],
                )
            }
        })
        .collect::<Vec<_>>()
        .join(" · ");
    Step::Available(line)
}

/// `update_providers`' answer, one entry per CLI: done when every one ran, failed when one did not.
pub fn results_step(results: &Value) -> Step {
    let mut lines = Vec::new();
    let mut failed = false;
    for entry in results.as_array().into_iter().flatten() {
        let name = crate::shell::provider_title(entry["provider"].as_str().unwrap_or(""));
        let version = entry["installedVersion"].as_str().unwrap_or("");
        lines.push(match entry["status"].as_str() {
            Some("updated") => tf("{} atualizado para {}", &[&name, &version]),
            Some("unchanged") => tf("{} já está na versão {}", &[&name, &version]),
            _ => {
                failed = true;
                let message = entry["message"].as_str().unwrap_or("");
                format!("{name}: {}", crate::i18n::from_host(message))
            }
        });
    }
    let line = lines.join(" · ");
    if failed {
        Step::Failed(line)
    } else if line.is_empty() {
        Step::UpToDate(String::new())
    } else {
        Step::Done(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(extra: Value) -> Value {
        let mut base = json!({ "supported": true, "updater": true, "running": "0bc2b226aabb" });
        for (key, value) in extra.as_object().unwrap() {
            base[key] = value.clone();
        }
        base
    }

    #[test]
    fn a_host_status_becomes_its_row() {
        assert!(matches!(
            host_step(&json!({ "supported": false }), None),
            Step::NotHere(_)
        ));
        assert!(matches!(
            host_step(&json!({ "supported": true, "updater": false }), None),
            Step::NotHere(_)
        ));
        assert_eq!(
            host_step(&status(json!({ "phase": "upToDate", "at": 5 })), None),
            Step::UpToDate("0bc2b226".into())
        );
        assert_eq!(
            host_step(
                &status(json!({ "phase": "waiting", "waitingFor": 2, "at": 5 })),
                None
            ),
            Step::Waiting(2)
        );
        assert_eq!(
            host_step(&status(json!({ "phase": "building", "at": 5 })), None),
            Step::Building
        );
        assert!(matches!(
            host_step(&status(json!({ "phase": "rolledBack", "at": 5 })), None),
            Step::RolledBack(_)
        ));
    }

    #[test]
    fn a_status_older_than_the_request_is_still_the_check_starting() {
        let old = status(json!({ "phase": "installed", "commit": "a7ad9092ff", "at": 90 }));
        assert_eq!(host_step(&old, Some(100)), Step::Checking);
        let pending = status(json!({ "phase": "upToDate", "at": 120, "requestPending": true }));
        assert_eq!(host_step(&pending, Some(100)), Step::Checking);
        let fresh = status(json!({ "phase": "installed", "commit": "a7ad9092ff", "at": 130 }));
        assert!(matches!(host_step(&fresh, Some(100)), Step::Done(_)));
    }

    #[test]
    fn offers_and_results_read_as_lines() {
        let list = json!([
            { "provider": "claude", "updateAvailable": true, "updateSupported": true,
              "installedVersion": "2.1.0", "latestVersion": "2.2.0" },
            { "provider": "codex", "updateAvailable": false, "updateSupported": true },
        ]);
        let found = offers(&list);
        assert_eq!(found.len(), 1);
        assert!(found[0].command && found[0].latest == "2.2.0");
        assert!(matches!(available_step(&found), Step::Available(_)));
        assert_eq!(available_step(&[]), Step::UpToDate(String::new()));
        let ok =
            json!([{ "provider": "claude", "status": "updated", "installedVersion": "2.2.0" }]);
        assert!(matches!(results_step(&ok), Step::Done(_)));
        let bad = json!([{ "provider": "claude", "status": "failed", "message": "EACCES" }]);
        assert!(matches!(results_step(&bad), Step::Failed(_)));
    }

    #[test]
    fn steps_settle_and_survive_the_page_data() {
        assert!(!Step::Building.settled() && Step::Waiting(1).settled());
        let mut board = Board::default();
        board
            .rows
            .insert(row_key("host", "pc-vps"), Step::Waiting(3));
        board.workers = 1;
        let back = Board::from_value(&board.to_value());
        assert_eq!(back.step("host:pc-vps"), Step::Waiting(3));
        assert!(back.running());
    }
}
