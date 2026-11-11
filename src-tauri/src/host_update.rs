//! "Atualizar tudo" for a Linux computer's host (ADR-207). The updater runs as root
//! (`scripts/vps/sirus-host-update`, ADR-206) and this host as its own user, so the two talk
//! through two small files in the host user's home: the host writes a request
//! (`.sirus-host-update-request`: `check` or `now`), which a systemd path unit turns into an
//! updater run within seconds, and the updater writes where it stands
//! (`.sirus-host-update-status`, JSON). The Mac app's host is updated with the app (ADR-140) and
//! answers `supported: false`.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::AppState;
use crate::error::Result;
use crate::runtime::State;

const REQUEST: &str = ".sirus-host-update-request";
const STATUS: &str = ".sirus-host-update-status";
/// The commit the updater installed last (ADR-206).
const BUILT: &str = ".sirus-host-built";
/// Where `install-updater.sh` puts the updater.
const UPDATER: &str = "/usr/local/sbin/sirus-host-update";
const MAX_STATUS_BYTES: u64 = 16 * 1024;
const MAX_MESSAGE_CHARS: usize = 300;

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    /// Where the host's update stands.
    Status,
    /// Asks the updater to run now. `now` installs even while conversations run (the window
    /// asked first); otherwise a new build installs once they are done.
    Request { now: bool },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostUpdate {
    /// False on the Mac (its host comes with the app) and on any computer that is not Linux.
    pub supported: bool,
    /// The updater is installed on this computer.
    pub updater: bool,
    pub version: String,
    /// The commit the updater installed last, when it did.
    pub running: Option<String>,
    /// Conversations a restart would cut right now.
    pub busy: usize,
    /// A request the updater has not picked up yet.
    pub request_pending: bool,
    /// What the updater wrote last: checking, upToDate, building, waiting, installing,
    /// installed, rolledBack or failed.
    pub phase: Option<String>,
    /// The commit that phase is about.
    pub commit: Option<String>,
    pub waiting_for: Option<u64>,
    /// When the updater wrote it, in Unix seconds of this computer's clock.
    pub at: Option<i64>,
    pub message: Option<String>,
    /// For a request: when it was written, on the same clock as `at`. A status is the answer to
    /// the request once `at` is not older and the request is no longer pending.
    pub requested_at: Option<i64>,
}

pub async fn host_update_action(
    state: State<'_, Arc<AppState>>,
    action: Action,
) -> Result<HostUpdate> {
    let busy = state
        .data
        .lock()
        .sessions
        .iter()
        .filter(|session| session.status.is_active())
        .count();
    let home = crate::new_project::home_dir();
    let supported = cfg!(target_os = "linux") && home.is_some();
    let updater = supported && Path::new(UPDATER).exists();
    let Some(home) = home.filter(|_| supported) else {
        return Ok(HostUpdate {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            busy,
            ..HostUpdate::default()
        });
    };
    // Without the updater nobody would read a request; the answer says so (`updater: false`).
    let requested_at = match action {
        Action::Request { now } if updater => {
            write_request(&home, now)?;
            Some(chrono::Utc::now().timestamp())
        }
        _ => None,
    };
    let mut status = read(&home, busy, updater);
    status.requested_at = requested_at;
    Ok(status)
}

/// Written next to the file and renamed into place, so the updater never reads half of it.
fn write_request(home: &Path, now: bool) -> Result<()> {
    let target = home.join(REQUEST);
    let draft = home.join(format!("{REQUEST}.{}", std::process::id()));
    std::fs::write(&draft, if now { "now\n" } else { "check\n" })?;
    std::fs::rename(&draft, &target).inspect_err(|_| {
        let _ = std::fs::remove_file(&draft);
    })?;
    Ok(())
}

fn read(home: &Path, busy: usize, updater: bool) -> HostUpdate {
    let text = |name: &str| -> Option<String> {
        let path: PathBuf = home.join(name);
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > MAX_STATUS_BYTES {
            return None;
        }
        std::fs::read_to_string(path).ok()
    };
    let mut status = text(STATUS)
        .map(|raw| parse_status(&raw))
        .unwrap_or_default();
    status.supported = true;
    status.updater = updater;
    status.version = env!("CARGO_PKG_VERSION").to_owned();
    status.busy = busy;
    status.request_pending = home.join(REQUEST).exists();
    if status.running.is_none() {
        status.running = text(BUILT).and_then(|raw| commit(raw.trim()));
    }
    status
}

/// The updater's status file, with every field checked: a phase from the known set, commits as
/// hex, a short message.
fn parse_status(raw: &str) -> HostUpdate {
    let value: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let text = |key: &str| value[key].as_str().map(str::trim).filter(|s| !s.is_empty());
    const PHASES: &[&str] = &[
        "checking",
        "upToDate",
        "building",
        "waiting",
        "installing",
        "installed",
        "rolledBack",
        "failed",
    ];
    HostUpdate {
        phase: text("phase")
            .filter(|phase| PHASES.contains(phase))
            .map(str::to_owned),
        commit: text("commit").and_then(commit),
        running: text("running").and_then(commit),
        waiting_for: value["waitingFor"].as_u64(),
        at: value["at"].as_i64(),
        message: text("message").map(|m| m.chars().take(MAX_MESSAGE_CHARS).collect()),
        ..HostUpdate::default()
    }
}

fn commit(raw: &str) -> Option<String> {
    (raw.len() >= 7 && raw.len() <= 64 && raw.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_file_is_read_field_by_field() {
        let status = parse_status(
            r#"{"phase":"waiting","commit":"a7ad9092ffee","running":"0bc2b226aa","waitingFor":2,"at":1731300000,"message":""}"#,
        );
        assert_eq!(status.phase.as_deref(), Some("waiting"));
        assert_eq!(status.commit.as_deref(), Some("a7ad9092ffee"));
        assert_eq!(status.running.as_deref(), Some("0bc2b226aa"));
        assert_eq!(status.waiting_for, Some(2));
        assert_eq!(status.at, Some(1731300000));
        assert_eq!(status.message, None);
    }

    #[test]
    fn unknown_phases_and_odd_commits_are_dropped() {
        let status = parse_status(r#"{"phase":"rm -rf","commit":"main; reboot","at":"x"}"#);
        assert_eq!((status.phase, status.commit, status.at), (None, None, None));
        assert_eq!(parse_status("not json"), HostUpdate::default());
    }

    #[test]
    fn a_request_lands_whole_and_the_status_reads_back() {
        let home = std::env::temp_dir().join(format!("sirus-host-update-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        write_request(&home, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join(REQUEST)).unwrap(),
            "now\n"
        );
        std::fs::write(home.join(BUILT), "0bc2b226aabb\n").unwrap();
        std::fs::write(
            home.join(STATUS),
            r#"{"phase":"installed","commit":"a7ad9092ff","at":5}"#,
        )
        .unwrap();
        let status = read(&home, 1, true);
        assert!(status.supported && status.updater && status.request_pending);
        assert_eq!(status.phase.as_deref(), Some("installed"));
        assert_eq!(status.running.as_deref(), Some("0bc2b226aabb"));
        assert_eq!(status.busy, 1);
        std::fs::remove_dir_all(&home).unwrap();
    }
}
