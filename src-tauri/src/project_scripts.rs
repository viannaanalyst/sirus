//! Project scripts (ADR-078): two optional shell scripts the owner saves per project.
//! Setup runs in a session's new isolated worktree before its first turn; On finish
//! runs there after each turn that completes or fails. They run as the person, like
//! the terminal (`$SHELL -l -c`), with a deadline and a bounded output tail, and never
//! block the conversation: a failed run is shown with Run again.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::runtime::{AppHandle, State};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{watch, Notify};

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::{Project, Session, SessionStatus};
use crate::paths::{display_path, now_rfc3339};

/// Largest saved script, per kind.
pub const MAX_SCRIPT_BYTES: usize = 8 * 1024;
const SETUP_LIMIT: Duration = Duration::from_secs(10 * 60);
const FINISH_LIMIT: Duration = Duration::from_secs(2 * 60);
/// Last bytes of combined stdout/stderr kept with a run.
const OUTPUT_TAIL: usize = 16 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectScripts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_finish: Option<String>,
    /// The page Setup serves, a loopback http(s) URL (ADR-151).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    /// Open `preview_url` in the session's browser once it answers after Setup starts.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub open_preview: bool,
}

impl ProjectScripts {
    pub fn is_empty(&self) -> bool {
        self.setup.is_none() && self.on_finish.is_none() && self.preview_url.is_none()
    }
    fn get(&self, kind: ScriptKind) -> Option<&str> {
        match kind {
            ScriptKind::Setup => self.setup.as_deref(),
            ScriptKind::Finish => self.on_finish.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScriptKind {
    Setup,
    Finish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptRun {
    pub id: String,
    pub kind: ScriptKind,
    pub status: RunStatus,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub output: String,
}

/// Per session: whether Setup is still due, and the latest run of each kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionScripts {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub setup_pending: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<ScriptRun>,
}

impl SessionScripts {
    /// A session whose isolated worktree was just created owes its Setup run.
    pub fn new(new_worktree: bool) -> Self {
        Self {
            setup_pending: new_worktree,
            runs: Vec::new(),
        }
    }
    pub fn is_empty(&self) -> bool {
        !self.setup_pending && self.runs.is_empty()
    }
}

/// A trimmed script, `None` when blank. Bounded, without NUL bytes.
pub fn validate(text: Option<String>) -> Result<Option<String>> {
    let Some(text) = text else { return Ok(None) };
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.len() > MAX_SCRIPT_BYTES || text.contains('\0') {
        return Err(Error::new(
            "invalid",
            "A project script must be text of at most 8 KiB.",
        ));
    }
    Ok(Some(text.replace("\r\n", "\n")))
}

/// A preview URL: blank is none; anything but a loopback http(s) URL with a port is refused.
pub fn validate_preview_url(url: Option<String>) -> Result<Option<String>> {
    let Some(url) = url.filter(|url| !url.trim().is_empty()) else {
        return Ok(None);
    };
    crate::local_servers::loopback_url(&url)
        .map(Some)
        .ok_or_else(|| {
            Error::new(
                "invalid",
                "The preview address must be a localhost http(s) URL with a port, like http://localhost:5173.",
            )
        })
}

/// How long the preview watch keeps trying after its script ends.
const PREVIEW_GRACE: Duration = Duration::from_secs(90);

/// Waits for the preview URL to answer while the script runs (and a little after), then
/// asks the window to open it in the session's browser (`project-preview`).
async fn watch_preview(
    app: AppHandle,
    session_id: String,
    url: String,
    mut done: watch::Receiver<bool>,
) {
    use crate::runtime::Emitter;
    let deadline = tokio::time::Instant::now() + SETUP_LIMIT + PREVIEW_GRACE;
    let mut ended: Option<tokio::time::Instant> = None;
    while tokio::time::Instant::now() < deadline {
        if crate::local_servers::responds(&url).await {
            let _ = app.emit(
                "project-preview",
                serde_json::json!({ "sessionId": session_id, "url": url }),
            );
            return;
        }
        if ended.is_none() && *done.borrow_and_update() {
            ended = Some(tokio::time::Instant::now());
        }
        if ended.is_some_and(|at| at.elapsed() > PREVIEW_GRACE) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
}

/// Runs left `running` by a closed app are reported as cancelled.
pub fn recover(session: &mut Session) -> bool {
    let mut changed = false;
    for run in &mut session.scripts.runs {
        if run.status == RunStatus::Running {
            run.status = RunStatus::Cancelled;
            run.finished_at.get_or_insert_with(now_rfc3339);
            changed = true;
        }
    }
    changed
}

struct Live {
    cancel: Arc<Notify>,
    done: watch::Receiver<bool>,
}

fn live() -> &'static Mutex<HashMap<String, Live>> {
    static LIVE: OnceLock<Mutex<HashMap<String, Live>>> = OnceLock::new();
    LIVE.get_or_init(Default::default)
}

/// Stops the session's running script, if any (Stop, delete).
pub fn cancel(session_id: &str) {
    if let Some(run) = live().lock().get(session_id) {
        run.cancel.notify_one();
    }
}

pub fn cancel_all() {
    for run in live().lock().values() {
        run.cancel.notify_one();
    }
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    /// Saves both scripts; a blank script is removed.
    Save {
        project_id: String,
        setup: Option<String>,
        on_finish: Option<String>,
        #[serde(default)]
        preview_url: Option<String>,
        #[serde(default)]
        open_preview: bool,
    },
    /// Runs the project's script of `kind` again in the session's worktree.
    Run {
        session_id: String,
        kind: ScriptKind,
    },
    /// Stops the session's running script.
    Cancel { session_id: String },
    /// Hides the latest run of `kind`.
    Dismiss {
        session_id: String,
        kind: ScriptKind,
    },
}

/// Closed project-script surface. `Save` returns the project; the other actions
/// publish the session through `session-updated`.
pub async fn project_scripts_action(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    action: Action,
) -> Result<Option<Project>> {
    let state = state.inner().clone();
    match action {
        Action::Save {
            project_id,
            setup,
            on_finish,
            preview_url,
            open_preview,
        } => {
            let preview_url = validate_preview_url(preview_url)?;
            let scripts = ProjectScripts {
                setup: validate(setup)?,
                on_finish: validate(on_finish)?,
                open_preview: open_preview && preview_url.is_some(),
                preview_url,
            };
            crate::commands::native_task(move || {
                let mut data = state.data.lock();
                state.ensure_running()?;
                let project = data
                    .projects
                    .iter_mut()
                    .find(|row| row.id == project_id)
                    .ok_or_else(|| Error::not_found("project not found"))?;
                let previous = std::mem::replace(&mut project.scripts, scripts);
                let saved = project.clone();
                if let Err(error) = crate::persist::save(&state.data_path, &data) {
                    if let Some(project) = data.projects.iter_mut().find(|row| row.id == project_id)
                    {
                        project.scripts = previous;
                    }
                    return Err(error);
                }
                Ok(Some(saved))
            })
            .await
        }
        Action::Run { session_id, kind } => {
            {
                let data = state.data.lock();
                let session = data
                    .sessions
                    .iter()
                    .find(|session| session.id == session_id)
                    .ok_or_else(|| Error::not_found("session not found"))?;
                if session.status.is_active() || state.agents.lock().contains_key(&session_id) {
                    return Err(Error::agent(
                        "Wait for the agent to finish before running a project script.",
                    ));
                }
            }
            let started = start(&app, &state, &session_id, kind)?
                .ok_or_else(|| Error::not_found("This project has no script of that kind."))?;
            crate::runtime::async_runtime::spawn(complete(app, state, session_id, started));
            Ok(None)
        }
        Action::Cancel { session_id } => {
            cancel(&session_id);
            Ok(None)
        }
        Action::Dismiss { session_id, kind } => {
            let mut data = state.data.lock();
            let session = data
                .sessions
                .iter_mut()
                .find(|session| session.id == session_id)
                .ok_or_else(|| Error::not_found("session not found"))?;
            session
                .scripts
                .runs
                .retain(|run| run.kind != kind || run.status == RunStatus::Running);
            crate::transcript_view::emit(&app, session);
            crate::persist::save(&state.data_path, &data)?;
            Ok(None)
        }
    }
}

/// Before a turn starts: waits for a script still running in the session, then
/// runs Setup once if the session's new worktree owes it. Never fails the turn.
pub async fn before_turn(app: &AppHandle, state: &Arc<AppState>, session_id: &str) {
    let running = live().lock().get(session_id).map(|run| run.done.clone());
    if let Some(mut done) = running {
        let _ = done.wait_for(|done| *done).await;
    }
    let due = {
        let mut data = state.data.lock();
        let Some(index) = data.sessions.iter().position(|s| s.id == session_id) else {
            return;
        };
        let session = &mut data.sessions[index];
        let pending = std::mem::take(&mut session.scripts.setup_pending);
        let isolated = session.worktree.isolated;
        let project_id = session.project_id.clone();
        pending
            && isolated
            && data
                .projects
                .iter()
                .any(|project| project.id == project_id && project.scripts.setup.is_some())
    };
    if !due {
        return;
    }
    match start(app, state, session_id, ScriptKind::Setup) {
        Ok(Some(started)) => complete(app.clone(), state.clone(), session_id.into(), started).await,
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "cannot start the setup script"),
    }
}

/// After a turn settles: On finish runs in the session's own worktree when the
/// turn completed or failed (not after Stop). Callers may hold `state.data`.
pub fn settled(app: &AppHandle, state: &Arc<AppState>, session_id: &str) {
    let (app, state, session_id) = (app.clone(), state.clone(), session_id.to_owned());
    crate::runtime::async_runtime::spawn(async move {
        let due = {
            let data = state.data.lock();
            data.sessions
                .iter()
                .find(|session| session.id == session_id)
                .is_some_and(|session| {
                    matches!(
                        session.status,
                        SessionStatus::Completed | SessionStatus::Failed
                    ) && session.worktree.isolated
                        && session.side_chat.is_none()
                        && data.projects.iter().any(|project| {
                            project.id == session.project_id && project.scripts.on_finish.is_some()
                        })
                })
        };
        if !due || live().lock().contains_key(&session_id) {
            return;
        }
        match start(&app, &state, &session_id, ScriptKind::Finish) {
            Ok(Some(started)) => complete(app, state, session_id, started).await,
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "cannot start the on-finish script"),
        }
    });
}

struct Started {
    run_id: String,
    script: String,
    cwd: PathBuf,
    env: Vec<(&'static str, String)>,
    limit: Duration,
    cancel: Arc<Notify>,
    done: watch::Sender<bool>,
    /// Setup's preview URL to open once it answers (ADR-151).
    preview: Option<String>,
}

/// Records a running run and registers it; `None` when the project has no such script.
fn start(
    app: &AppHandle,
    state: &AppState,
    session_id: &str,
    kind: ScriptKind,
) -> Result<Option<Started>> {
    let mut data = state.data.lock();
    state.ensure_running()?;
    let session = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    let project = data
        .projects
        .iter()
        .find(|project| project.id == session.project_id)
        .ok_or_else(|| Error::not_found("project not found"))?;
    let Some(script) = project.scripts.get(kind).map(str::to_owned) else {
        return Ok(None);
    };
    if !session.worktree.isolated {
        return Err(Error::agent(
            "Project scripts run only in a session's own worktree.",
        ));
    }
    // Any script run (Setup, On finish, Run again) opens the preview once it answers (ADR-151).
    let preview = project
        .scripts
        .open_preview
        .then(|| project.scripts.preview_url.clone())
        .flatten();
    let cwd = crate::commands::session_cwd(&data, session)?;
    let env = vec![
        ("SIRUS_PROJECT_ROOT", project.path.clone()),
        ("SIRUS_WORKTREE_PATH", display_path(&cwd)),
        ("SIRUS_SESSION_ID", session.id.clone()),
    ];
    let mut registry = live().lock();
    if registry.contains_key(session_id) {
        return Err(Error::agent(
            "A project script is already running in this session.",
        ));
    }
    let run = ScriptRun {
        id: uuid::Uuid::new_v4().to_string(),
        kind,
        status: RunStatus::Running,
        started_at: now_rfc3339(),
        finished_at: None,
        exit_code: None,
        output: String::new(),
    };
    let run_id = run.id.clone();
    let session = data
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    session.scripts.runs.retain(|item| item.kind != kind);
    session.scripts.runs.push(run);
    crate::transcript_view::emit(app, session);
    if let Err(error) = crate::persist::save(&state.data_path, &data) {
        tracing::warn!(%error, "cannot persist a project script start");
    }
    let cancel = Arc::new(Notify::new());
    let (done, receiver) = watch::channel(false);
    registry.insert(
        session_id.to_owned(),
        Live {
            cancel: cancel.clone(),
            done: receiver,
        },
    );
    Ok(Some(Started {
        run_id,
        script,
        cwd,
        env,
        limit: match kind {
            ScriptKind::Setup => SETUP_LIMIT,
            ScriptKind::Finish => FINISH_LIMIT,
        },
        cancel,
        done,
        preview,
    }))
}

async fn complete(app: AppHandle, state: Arc<AppState>, session_id: String, started: Started) {
    if let Some(url) = started.preview.clone() {
        crate::runtime::async_runtime::spawn(watch_preview(
            app.clone(),
            session_id.clone(),
            url,
            started.done.subscribe(),
        ));
    }
    let shell = PathBuf::from(crate::detect::default_shell());
    let (status, code, output) = execute(
        &shell,
        &started.script,
        &started.cwd,
        &started.env,
        started.limit,
        &started.cancel,
    )
    .await;
    {
        let mut data = state.data.lock();
        if let Some(session) = data.sessions.iter_mut().find(|s| s.id == session_id) {
            if let Some(run) = session
                .scripts
                .runs
                .iter_mut()
                .find(|run| run.id == started.run_id)
            {
                run.status = status;
                run.exit_code = code;
                run.output = output;
                run.finished_at = Some(now_rfc3339());
            }
            crate::transcript_view::emit(&app, session);
        }
        if let Err(error) = crate::persist::save(&state.data_path, &data) {
            tracing::warn!(%error, "cannot persist a project script result");
        }
    }
    live().lock().remove(&session_id);
    let _ = started.done.send(true);
}

/// Runs `script` through `shell -l -c` in `cwd`, in its own process group, until it
/// exits, `limit` passes or `cancel` fires. Returns the outcome and the output tail.
pub(crate) async fn execute(
    shell: &Path,
    script: &str,
    cwd: &Path,
    env: &[(&'static str, String)],
    limit: Duration,
    cancel: &Notify,
) -> (RunStatus, Option<i32>, String) {
    let mut command = tokio::process::Command::new(shell);
    command
        .args(["-l", "-c", script])
        .current_dir(cwd)
        .env("PATH", crate::detect::cli_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in env {
        command.env(key, value);
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return (
                RunStatus::Failed,
                None,
                format!("Could not start {}: {error}", shell.display()),
            )
        }
    };
    let tail = Mutex::new(Vec::<u8>::new());
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let pid = child.id();
    let outcome = {
        let work = async {
            tokio::join!(pump(stdout, &tail), pump(stderr, &tail));
            child.wait().await
        };
        tokio::select! {
            status = work => match status {
                Ok(status) if status.success() => (RunStatus::Succeeded, status.code()),
                Ok(status) => (RunStatus::Failed, status.code()),
                Err(_) => (RunStatus::Failed, None),
            },
            () = tokio::time::sleep(limit) => (RunStatus::TimedOut, None),
            () = cancel.notified() => (RunStatus::Cancelled, None),
        }
    };
    if matches!(outcome.0, RunStatus::TimedOut | RunStatus::Cancelled) {
        #[cfg(unix)]
        if let Some(pid) = pid {
            // The leader is not reaped yet, so its group id still belongs to this run.
            unsafe {
                libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            }
        }
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    }
    let bytes = tail.into_inner();
    (outcome.0, outcome.1, clean_output(&bytes))
}

async fn pump<R: AsyncRead + Unpin>(reader: Option<R>, tail: &Mutex<Vec<u8>>) {
    let Some(mut reader) = reader else { return };
    let mut buffer = [0u8; 8192];
    while let Ok(count) = reader.read(&mut buffer).await {
        if count == 0 {
            break;
        }
        let mut tail = tail.lock();
        tail.extend_from_slice(&buffer[..count]);
        if tail.len() > OUTPUT_TAIL * 2 {
            let excess = tail.len() - OUTPUT_TAIL;
            tail.drain(..excess);
        }
    }
}

/// The last `OUTPUT_TAIL` bytes as text, without terminal colour codes; a carriage
/// return keeps only what was drawn last on its line (progress bars).
pub(crate) fn clean_output(bytes: &[u8]) -> String {
    let truncated = bytes.len() > OUTPUT_TAIL;
    let bytes = &bytes[bytes.len().saturating_sub(OUTPUT_TAIL)..];
    let text = String::from_utf8_lossy(bytes);
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            plain.push(ch);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else {
            chars.next();
        }
    }
    let lines = plain
        .split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            line.rsplit('\r').next().unwrap_or(line)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let lines = lines.trim_end().to_string();
    if truncated {
        format!("…\n{lines}")
    } else {
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sirus-scripts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn scripts_are_trimmed_bounded_and_blank_means_none() {
        assert_eq!(validate(None).unwrap(), None);
        assert_eq!(validate(Some("  \n ".into())).unwrap(), None);
        assert_eq!(
            validate(Some(" npm install\r\n".into()))
                .unwrap()
                .as_deref(),
            Some("npm install")
        );
        assert!(validate(Some("a\0b".into())).is_err());
        assert!(validate(Some("x".repeat(MAX_SCRIPT_BYTES + 1))).is_err());
    }

    #[test]
    fn empty_scripts_are_omitted_from_json() {
        let project: Project = serde_json::from_value(serde_json::json!({
            "id":"p","name":"P","path":"/tmp","addedAt":"t","lastOpenedAt":"t"
        }))
        .unwrap();
        assert!(project.scripts.is_empty());
        let value = serde_json::to_value(&project).unwrap();
        assert!(value.get("scripts").is_none());
        let scripts = ProjectScripts {
            setup: Some("npm install".into()),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&scripts).unwrap(),
            serde_json::json!({"setup":"npm install"})
        );
        let preview: ProjectScripts = serde_json::from_value(
            serde_json::json!({"previewUrl":"http://localhost:5173","openPreview":true}),
        )
        .unwrap();
        assert!(preview.open_preview && !preview.is_empty());
        assert!(SessionScripts::new(false).is_empty());
        assert_eq!(
            serde_json::to_value(SessionScripts::new(true)).unwrap(),
            serde_json::json!({"setupPending":true})
        );
    }

    #[test]
    fn preview_urls_are_loopback_only_and_save_accepts_older_clients() {
        assert_eq!(validate_preview_url(None).unwrap(), None);
        assert_eq!(validate_preview_url(Some("  ".into())).unwrap(), None);
        assert_eq!(
            validate_preview_url(Some("http://localhost:3000/".into())).unwrap(),
            Some("http://localhost:3000/".into())
        );
        assert!(validate_preview_url(Some("https://example.com:443".into())).is_err());
        assert!(validate_preview_url(Some("http://localhost".into())).is_err());
        let action: Action = serde_json::from_value(serde_json::json!({
            "type":"save","projectId":"p","setup":null,"onFinish":null
        }))
        .unwrap();
        assert!(matches!(
            action,
            Action::Save {
                preview_url: None,
                open_preview: false,
                ..
            }
        ));
    }

    #[test]
    fn output_keeps_the_tail_without_colours_or_redrawn_progress() {
        assert_eq!(
            clean_output(b"\x1b[32mok\x1b[0m\r\n10%\r50%\r100%\ndone\n"),
            "ok\n100%\ndone"
        );
        let long = vec![b'a'; OUTPUT_TAIL + 10];
        let text = clean_output(&long);
        assert!(text.starts_with("…\n"));
        assert_eq!(text.len(), "…\n".len() + OUTPUT_TAIL);
    }

    #[tokio::test]
    async fn runs_in_the_worktree_and_reports_failures_and_deadlines() {
        let dir = temp();
        let cancel = Notify::new();
        let env = [("SIRUS_WORKTREE_PATH", display_path(&dir))];
        let shell = Path::new("/bin/sh");
        let (status, code, output) = execute(
            shell,
            "pwd -P; echo \"$SIRUS_WORKTREE_PATH\"; echo warn >&2",
            &dir,
            &env,
            Duration::from_secs(10),
            &cancel,
        )
        .await;
        assert_eq!((status, code), (RunStatus::Succeeded, Some(0)));
        let canonical = display_path(&dir.canonicalize().unwrap());
        assert!(output.contains(&canonical), "{output}");
        assert!(output.contains("warn"));

        let (status, code, _) =
            execute(shell, "exit 3", &dir, &[], Duration::from_secs(10), &cancel).await;
        assert_eq!((status, code), (RunStatus::Failed, Some(3)));

        let started = std::time::Instant::now();
        let (status, _, _) = execute(
            shell,
            "sleep 30",
            &dir,
            &[],
            Duration::from_millis(200),
            &cancel,
        )
        .await;
        assert_eq!(status, RunStatus::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));

        cancel.notify_one();
        let (status, _, _) = execute(
            shell,
            "sleep 30",
            &dir,
            &[],
            Duration::from_secs(30),
            &cancel,
        )
        .await;
        assert_eq!(status, RunStatus::Cancelled);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn interrupted_runs_recover_as_cancelled() {
        let mut session: Session = serde_json::from_value(serde_json::json!({
            "id":"s","projectId":"p","title":"T","agent":"codex","status":"idle",
            "createdAt":"t","lastActivityAt":"t","worktree":{"path":"/tmp","branch":"b","isolated":true},
            "lastError":null,
            "scripts":{"runs":[{"id":"r","kind":"setup","status":"running","startedAt":"t","output":""}]}
        }))
        .unwrap();
        assert!(recover(&mut session));
        assert_eq!(session.scripts.runs[0].status, RunStatus::Cancelled);
        assert!(!recover(&mut session));
    }
}
