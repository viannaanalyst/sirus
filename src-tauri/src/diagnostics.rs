//! Opt-in bounded lifecycle metadata. Never accepts text from projects, tools or the renderer.
use crate::{
    error::{Error, Result},
    models::{AgentProviderId, AppData, SessionStatus},
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
};

const MAX_BYTES: u64 = 256 * 1024;
#[derive(Default, PartialEq, Eq)]
struct Summary {
    projects: usize,
    sessions: usize,
    statuses: [usize; 7],
    providers: [usize; 10],
}
impl Summary {
    fn from_data(data: &AppData) -> Self {
        let mut summary = Self {
            projects: data.projects.len(),
            sessions: data.sessions.len(),
            ..Self::default()
        };
        for session in &data.sessions {
            let status = match session.status {
                SessionStatus::Idle => 0,
                SessionStatus::Starting => 1,
                SessionStatus::Running => 2,
                SessionStatus::Waiting => 3,
                SessionStatus::Completed => 4,
                SessionStatus::Failed => 5,
                SessionStatus::Stopped => 6,
            };
            let provider = match session.agent {
                AgentProviderId::Codex => 0,
                AgentProviderId::Claude => 1,
                AgentProviderId::OpenCode => 2,
                AgentProviderId::Cursor => 3,
                AgentProviderId::Grok => 4,
                AgentProviderId::Antigravity => 5,
                AgentProviderId::Droid => 6,
                AgentProviderId::Pi => 7,
                AgentProviderId::Devin => 8,
                AgentProviderId::Hermes => 9,
            };
            summary.statuses[status] += 1;
            summary.providers[provider] += 1;
        }
        summary
    }
}
#[derive(Default)]
struct Journal {
    previous: Option<Summary>,
}
fn reject_non_regular(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => {
            Err(Error::new("diagnostics", "Logs must be regular files."))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
pub fn prepare(state_path: &Path) -> Result<PathBuf> {
    let root = state_path
        .parent()
        .ok_or_else(|| Error::invalid_path("Data directory is unavailable."))?
        .canonicalize()?;
    let dir = root.join("logs");
    match fs::create_dir(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    if !fs::symlink_metadata(&dir)?.file_type().is_dir() {
        return Err(Error::invalid_path("Logs directory must not be a symlink."));
    }
    Ok(dir)
}
impl Journal {
    fn observe(&mut self, state_path: &Path, data: &AppData) -> Result<()> {
        if !data.settings.developer_logs {
            self.previous = None;
            return Ok(());
        }
        let summary = Summary::from_data(data);
        if self.previous.as_ref() == Some(&summary) {
            return Ok(());
        }
        let dir = prepare(state_path)?;
        let current = dir.join("lifecycle.jsonl");
        let previous = dir.join("lifecycle.previous.jsonl");
        reject_non_regular(&current)?;
        reject_non_regular(&previous)?;
        let record = serde_json::json!({
            "timestamp": crate::paths::now_rfc3339(), "kind": "lifecycle", "projects": summary.projects, "sessions": summary.sessions,
            "statuses": { "idle":summary.statuses[0], "starting":summary.statuses[1], "running":summary.statuses[2], "waiting":summary.statuses[3], "completed":summary.statuses[4], "failed":summary.statuses[5], "stopped":summary.statuses[6] },
            "providers": { "codex":summary.providers[0], "claude":summary.providers[1], "opencode":summary.providers[2], "cursor":summary.providers[3], "grok":summary.providers[4], "antigravity":summary.providers[5], "droid":summary.providers[6], "pi":summary.providers[7], "devin":summary.providers[8], "hermes":summary.providers[9] }
        }).to_string() + "\n";
        if fs::metadata(&current)
            .is_ok_and(|meta| meta.len().saturating_add(record.len() as u64) > MAX_BYTES)
        {
            fs::rename(&current, &previous)?;
        }
        let mut options = fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
        }
        let mut file = options.open(&current)?;
        file.write_all(record.as_bytes())?;
        self.previous = Some(summary);
        Ok(())
    }
}
pub fn observe(state_path: &Path, data: &AppData) {
    static JOURNALS: OnceLock<Mutex<HashMap<PathBuf, Journal>>> = OnceLock::new();
    let mut journals = JOURNALS.get_or_init(Mutex::default).lock();
    // Production has one state file; keep disposable test/alternate roots bounded too.
    if !journals.contains_key(state_path) && journals.len() >= 8 {
        journals.clear();
    }
    if journals
        .entry(state_path.to_path_buf())
        .or_default()
        .observe(state_path, data)
        .is_err()
    {
        tracing::warn!("Sirus lifecycle diagnostics could not be written");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logs_are_opt_in_metadata_only_and_suppress_unchanged_snapshots() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut journal = Journal::default();
        let mut data = crate::models::AppData::default();
        journal.observe(&path, &data).unwrap();
        assert!(!temp.0.join("logs").exists());
        data.settings.developer_logs = true;
        data.composer_drafts
            .insert("project:secret".into(), "TOKEN_DO_NOT_LOG".into());
        journal.observe(&path, &data).unwrap();
        journal.observe(&path, &data).unwrap();
        let text = std::fs::read_to_string(temp.0.join("logs/lifecycle.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(!text.contains("TOKEN_DO_NOT_LOG"));
        assert!(!text.contains("secret"));
        data.settings.developer_logs = false;
        journal.observe(&path, &data).unwrap();
        assert_eq!(
            std::fs::read_to_string(temp.0.join("logs/lifecycle.jsonl")).unwrap(),
            text
        );
    }
    #[test]
    fn journals_rotate_with_bounded_owned_files_and_refuse_symlinks() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        prepare(&path).unwrap();
        let current = temp.0.join("logs/lifecycle.jsonl");
        std::fs::write(&current, vec![b' '; MAX_BYTES as usize]).unwrap();
        let mut data = crate::models::AppData::default();
        data.settings.developer_logs = true;
        Journal::default().observe(&path, &data).unwrap();
        assert!(std::fs::metadata(&current).unwrap().len() < 2048);
        assert_eq!(
            std::fs::metadata(temp.0.join("logs/lifecycle.previous.jsonl"))
                .unwrap()
                .len(),
            MAX_BYTES
        );
        #[cfg(unix)]
        {
            let other = temp.0.join("untouched.txt");
            std::fs::write(&other, "USER_DATA").unwrap();
            std::fs::remove_file(&current).unwrap();
            std::os::unix::fs::symlink(&other, &current).unwrap();
            assert!(Journal::default().observe(&path, &data).is_err());
            assert_eq!(std::fs::read_to_string(other).unwrap(), "USER_DATA");
        }
    }
}
