use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::runtime::{AppHandle, State};
use parking_lot::Mutex;
use uuid::Uuid;

use crate::agent::{self, AgentProcess};
use crate::detect;
use crate::error::{Error, Result};
use crate::fs_tree;
use crate::git;
use crate::models::{
    AgentInstall, AgentProviderId, AppData, AppSettings, CreateSessionRequest, FileEntry,
    GitCommitResult, GitPushResult, GitStatus, HostInfo, Message, MessageRole, ProbeResult,
    Project, SendPromptRequest, Session, SessionStatus, Worktree, WorktreeLocationPref,
};
use crate::paths::{self, display_path, now_rfc3339};
use crate::persist;
use crate::pty_term::{self, PtyMap};
use crate::worktree;

pub struct AppState {
    pub data_path: PathBuf,
    pub worktree_root: PathBuf,
    pub data: Mutex<AppData>,
    pub agents: Mutex<HashMap<String, AgentProcess>>,
    pub ptys: Mutex<PtyMap>,
    pub closing: AtomicBool,
    pub attachment_picker: AtomicBool,
    pub attachments: crate::attachments::AttachmentState,
    pub usage: crate::provider_usage::UsageState,
    pub accounts: crate::provider_accounts::AccountState,
    pub close_guard: Mutex<crate::close::CloseGuard>,
    pub draft_checkpoint: Mutex<Option<std::time::Instant>>,
    /// One coalesced background checkpoint is pending (streaming output).
    pub checkpoint_pending: AtomicBool,
    pub catalogs: Mutex<
        HashMap<AgentProviderId, (Option<String>, crate::provider_models::ProviderModelList)>,
    >,
}

impl AppState {
    /// Fresh process state over loaded data; the desktop app and the headless host
    /// (ADR-131) build it the same way.
    pub fn new(data_path: PathBuf, worktree_root: PathBuf, data: AppData) -> Self {
        Self {
            data_path,
            worktree_root,
            data: Mutex::new(data),
            agents: Mutex::new(HashMap::new()),
            ptys: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            attachment_picker: AtomicBool::new(false),
            attachments: Default::default(),
            usage: crate::provider_usage::UsageState::default(),
            accounts: crate::provider_accounts::AccountState::default(),
            close_guard: Mutex::new(crate::close::CloseGuard::default()),
            draft_checkpoint: Mutex::new(None),
            checkpoint_pending: Default::default(),
            catalogs: Mutex::new(HashMap::new()),
        }
    }

    pub fn ensure_running(&self) -> Result<()> {
        if self.closing.load(Ordering::Acquire) {
            return Err(Error::agent("Sirus is closing"));
        }
        Ok(())
    }
    pub fn shutdown(&self) -> Result<()> {
        // Lock order excludes starts, admission and terminal creation during cleanup.
        let mut data = self.data.lock();
        self.closing.store(true, Ordering::Release);
        self.usage.stop();
        crate::commit_title::stop();
        self.accounts.stop();
        self.attachments.clear();
        crate::project_scripts::cancel_all();
        crate::agent_pool::clear();
        for process in self.agents.lock().values() {
            if let Err(error) = process.stop() {
                tracing::warn!(%error, "agent shutdown failed");
            }
        }
        let terminals = self.ptys.lock();
        if let Err(error) = crate::pty_term::kill_all(&terminals.values().collect::<Vec<_>>()) {
            tracing::warn!(%error, "terminal shutdown failed");
        }
        drop(terminals);
        for session in &mut data.sessions {
            if session.status.is_active() {
                session.status = SessionStatus::Stopped;
                crate::activity::sync(session);
                session.last_error = Some("Execution was interrupted when Sirus closed.".into());
            }
            session.pending_requests.clear();
            // A stream can fail before its process monitor publishes finality.
            // Unloaded transcripts are settled on disk and stay unread.
            if session
                .messages
                .loaded()
                .is_some_and(|list| list.iter().any(|message| message.streaming))
            {
                for message in &mut session.messages {
                    message.streaming = false;
                }
            }
        }
        persist::save(&self.data_path, &data)
    }

    /// Saves, then releases transcripts that are no longer needed in memory (ADR-099).
    pub fn persist(&self) -> Result<()> {
        let mut data = self.data.lock();
        persist::save(&self.data_path, &data)?;
        crate::transcript_store::evict(&mut data, &self.data_path, &[]);
        Ok(())
    }

    /// Locks the state with these sessions' transcripts loaded; unloaded ones are
    /// read from disk before the lock is taken (ADR-099).
    pub fn data_with_messages(&self, ids: &[&str]) -> parking_lot::MutexGuard<'_, AppData> {
        crate::transcript_store::lock_loaded(&self.data, &self.data_path, ids)
    }
}

pub(crate) fn session_cwd(data: &AppData, session: &Session) -> Result<PathBuf> {
    let project = data
        .projects
        .iter()
        .find(|project| project.id == session.project_id)
        .ok_or_else(|| Error::not_found("project not found"))?;
    let root = PathBuf::from(&project.path);
    // Name the folder: a generic "path does not exist" reads like a missing CLI.
    if !root.is_dir() {
        return Err(Error::invalid_path(format!(
            "The project folder was not found: {}. Move it back or remove the project and add it again.",
            root.display()
        )));
    }
    let project_root = paths::ensure_dir(&root)?;
    if session.worktree.isolated {
        let path = PathBuf::from(&session.worktree.path);
        if !path.exists() {
            // The folder went missing (deleted outside the app); its branch still holds the work.
            crate::worktree::restore(&project_root, &project.id, &session.worktree)?;
        }
        return paths::ensure_dir(&path);
    }
    let folder = PathBuf::from(&session.worktree.path);
    if !folder.is_dir() {
        return Err(Error::invalid_path(format!(
            "The working folder was not found: {}.",
            folder.display()
        )));
    }
    paths::ensure_within(&project_root, &folder)
}

fn find_session_mut<'a>(data: &'a mut AppData, id: &str) -> Result<&'a mut Session> {
    data.sessions
        .iter_mut()
        .find(|session| session.id == id)
        .ok_or_else(|| Error::not_found("session not found"))
}

/// How long a turn may stay "Starting" before it is stopped.
const START_LIMIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// A provider that never answers leaves its turn "Starting" forever: after [`START_LIMIT`]
/// the same turn, still not running, is stopped with a clear error so the person can retry.
fn watch_start(app: AppHandle, state: Arc<AppState>, snapshot: Session) {
    crate::runtime::async_runtime::spawn(async move {
        tokio::time::sleep(START_LIMIT).await;
        if !still_starting(&state.data.lock(), &snapshot) {
            return;
        }
        let _ = stop_one(&app, &state, &snapshot.id);
        let changed = {
            let mut data = state.data.lock();
            match find_session_mut(&mut data, &snapshot.id) {
                // `stop_one` has just marked the turn Stopped; it failed to start instead.
                Ok(session)
                    if same_run(session, &snapshot)
                        && matches!(
                            session.status,
                            SessionStatus::Stopped | SessionStatus::Starting
                        ) =>
                {
                    session.status = SessionStatus::Failed;
                    session.stopped_by_user_at = None;
                    session.last_error = Some(START_TIMEOUT_ERROR.into());
                    crate::activity::sync(session);
                    if let Some(last) = session.messages.last_mut() {
                        last.streaming = false;
                    }
                    Some(session.clone())
                }
                _ => None,
            }
        };
        if let Some(session) = changed {
            let _ = state.persist();
            crate::transcript_view::emit(&app, &session);
        }
    });
}

const START_TIMEOUT_ERROR: &str =
    "The agent did not start within 10 minutes and was stopped. Send the message again to retry.";

/// The turn begun in `snapshot` has not started running yet.
fn still_starting(data: &AppData, snapshot: &Session) -> bool {
    data.sessions.iter().any(|session| {
        session.id == snapshot.id
            && session.status == SessionStatus::Starting
            && same_run(session, snapshot)
    })
}

fn same_run(session: &Session, snapshot: &Session) -> bool {
    session
        .messages
        .last()
        .zip(snapshot.messages.last())
        .is_some_and(|(current, previous)| current.id == previous.id)
}

pub fn load_state(state: State<Arc<AppState>>) -> Result<serde_json::Value> {
    // Metadata only; transcripts load per session through `transcript_action` (ADR-048).
    crate::transcript_view::state_meta(&state.data.lock())
}

pub async fn transcript_action(
    state: State<'_, Arc<AppState>>,
    action: crate::transcript_view::Action,
) -> Result<crate::transcript_view::Response> {
    use crate::transcript_view::{self as view, Action};
    let state = state.inner().clone();
    native_task(move || match action {
        Action::Load {
            session_id,
            before,
            limit,
            lite,
        } => view::load(
            &state.data_with_messages(&[&session_id]),
            &session_id,
            before,
            limit,
            lite,
        ),
        Action::Search { query } => {
            let snapshot = view::search_snapshot(&state.data.lock());
            view::search(snapshot, &query)
        }
        Action::TurnReview {
            session_id,
            message_id,
        } => view::turn_review(
            &state.data_with_messages(&[&session_id]),
            &session_id,
            &message_id,
        ),
        Action::ToolOutput {
            session_id,
            message_id,
            step_id,
        } => view::tool_output(
            &state.data,
            &state.data_path,
            &session_id,
            &message_id,
            &step_id,
        ),
    })
    .await
}

pub async fn save_settings(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    settings: AppSettings,
) -> Result<AppSettings> {
    let state = state.inner().clone();
    native_task(move || {
        let saved = save_settings_native(&state, settings)?;
        crate::appearance::schedule(&app, state.clone());
        crate::window_snap::apply(&app, &saved);
        // The Mac window and paired phones follow settings changed elsewhere (ADR-130).
        let _ = crate::runtime::Emitter::emit(&app, "settings-changed", &saved);
        Ok(saved)
    })
    .await
}

fn save_settings_native(state: &AppState, mut settings: AppSettings) -> Result<AppSettings> {
    settings.validate_controls()?;
    crate::sidebar::validate(&settings)?;
    if settings.developer_logs {
        crate::diagnostics::prepare(&state.data_path)?;
    }
    settings.git_confirm_destructive = true;
    let mut data = state.data.lock();
    state.ensure_running()?;
    crate::sidebar::normalize(&mut settings, &data.projects, &data.sessions);
    // Admit the new settings only after their persistence succeeds. A failed
    // write must not become the source of a later appearance callback.
    let previous = std::mem::replace(&mut data.settings, settings.clone());
    if let Err(error) = persist::save(&state.data_path, &data) {
        data.settings = previous;
        return Err(error);
    }
    crate::computer_mcp::set_enabled(settings.computer_use_enabled);
    crate::computer_mcp::set_self_allowed(settings.computer_use_control_sirus);
    crate::agent_pool::set_limit(settings.keep_idle_agents as usize);
    crate::sirus_tools::set_enabled(settings.agents_manage_sessions);
    crate::browser_mcp::set_agent_access(settings.agent_browser_access);
    crate::connectors::set(&settings.connectors);
    Ok(settings)
}

pub async fn save_composer_draft(
    state: State<'_, Arc<AppState>>,
    key: String,
    value: String,
) -> Result<()> {
    let state = state.inner().clone();
    native_task(move || {
        let mut data = state.data.lock();
        state.ensure_running()?;
        crate::drafts::apply(&mut data, &key, value)?;
        let mut checkpoint = state.draft_checkpoint.lock();
        if checkpoint.is_none_or(|last| last.elapsed() >= std::time::Duration::from_millis(250)) {
            persist::save(&state.data_path, &data)?;
            *checkpoint = Some(std::time::Instant::now());
        }
        // Shutdown persists the latest admitted edit even between checkpoints.
        Ok(())
    })
    .await
}

pub async fn save_context_text(
    state: State<'_, Arc<AppState>>,
    key: String,
    value: String,
    flush: bool,
) -> Result<()> {
    let state = state.inner().clone();
    native_task(move || save_context_text_native(&state, &key, value, flush)).await
}

fn save_context_text_native(state: &AppState, key: &str, value: String, flush: bool) -> Result<()> {
    let mut data = state.data.lock();
    state.ensure_running()?;
    crate::context_text::apply(&mut data, key, value)?;
    let mut checkpoint = state.draft_checkpoint.lock();
    if flush
        || checkpoint.is_none_or(|last| last.elapsed() >= std::time::Duration::from_millis(250))
    {
        persist::save(&state.data_path, &data)?;
        *checkpoint = Some(std::time::Instant::now());
    }
    // References stay local metadata; shutdown persists the latest admitted edit.
    Ok(())
}

pub fn add_project(state: State<Arc<AppState>>, path: String) -> Result<Project> {
    // Paths typed in the "New project" dialog (or on the phone, ADR-131) may start with `~`;
    // they must be absolute existing folders.
    let home = crate::new_project::home_dir();
    let folder = crate::new_project::typed_folder(&path, home.as_deref())?;
    add_project_native(&state, &folder, None)
}

/// New project from a name: creates `<parent>/<slug>` with Git, a README and a
/// first commit (`new_project.rs`), then adds it like an opened folder, named as typed.
pub async fn create_project(
    state: State<'_, Arc<AppState>>,
    name: String,
    parent: String,
) -> Result<Project> {
    let state = state.inner().clone();
    native_task(move || {
        let folder = crate::new_project::create(&name, &parent)?;
        add_project_native(&state, &folder, Some(name.trim().to_string()))
    })
    .await
}

/// Clone from GitHub (or any https / ssh remote) into `<parent>/<folder or repo name>`, then
/// add it like an opened folder (`new_project::clone`). `folder` is the phone's optional name.
pub async fn clone_project(
    state: State<'_, Arc<AppState>>,
    source: String,
    parent: String,
    folder: Option<String>,
) -> Result<Project> {
    let state = state.inner().clone();
    native_task(move || {
        let (folder, name) = crate::new_project::clone(&source, &parent, folder.as_deref())?;
        add_project_native(&state, &folder, Some(name))
    })
    .await
}

fn add_project_native(
    state: &AppState,
    path: &std::path::Path,
    name: Option<String>,
) -> Result<Project> {
    let canonical = paths::ensure_dir(path)?;
    let name = name.unwrap_or_else(|| {
        canonical
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Project".into())
    });
    let now = now_rfc3339();
    let mut data = state.data.lock();
    if let Some(existing) = data
        .projects
        .iter_mut()
        .find(|project| project.path == display_path(&canonical))
    {
        existing.last_opened_at = now;
        let project = existing.clone();
        drop(data);
        state.persist()?;
        return Ok(project);
    }
    let project = Project {
        id: Uuid::new_v4().to_string(),
        name,
        path: display_path(&canonical),
        added_at: now.clone(),
        last_opened_at: now,
        look: Default::default(),
        appearance: None,
        scripts: Default::default(),
    };
    data.projects.insert(0, project.clone());
    drop(data);
    state.persist()?;
    Ok(project)
}

pub async fn remove_project(state: State<'_, Arc<AppState>>, project_id: String) -> Result<()> {
    let state = state.inner().clone();
    native_task(move || remove_project_native(&state, project_id)).await
}

fn remove_project_native(state: &AppState, project_id: String) -> Result<()> {
    let mut data = state.data.lock();
    if data.sessions.iter().any(|session| {
        session.project_id == project_id
            && (session.status.is_active() || state.agents.lock().contains_key(&session.id))
    }) {
        return Err(Error::agent(
            "stop running sessions before removing this project",
        ));
    }
    let owned = data
        .sessions
        .iter()
        .filter(|session| session.project_id == project_id)
        .map(|session| session.id.clone())
        .collect::<HashSet<_>>();
    for session_id in &owned {
        crate::agent_pool::discard(session_id);
    }
    // Terminals are keyed by terminal id; match them by their owning session.
    let terminals = {
        let mut ptys = state.ptys.lock();
        let ids = ptys
            .values()
            .filter(|pty| owned.contains(&pty.session_id))
            .map(|pty| pty.terminal_id.clone())
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|terminal_id| ptys.remove(&terminal_id))
            .collect::<Vec<_>>()
    };
    data.projects.retain(|project| project.id != project_id);
    let (removed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut data.sessions)
        .into_iter()
        .partition(|session| session.project_id == project_id);
    data.sessions = kept;
    for session in &removed {
        crate::subagent_feed::forget(&session.id);
    }
    crate::review_guides::prune(&mut data);
    crate::pr_watch::prune(&mut data);
    crate::drafts::prune(&mut data);
    crate::context_text::prune(&mut data);
    crate::sidebar::prune(&mut data);
    state.attachments.prune(&data);
    drop(data);
    // Read before the save removes their transcript files.
    let thumbnails = crate::thumbnails::shown_by(&removed);
    // Shell cleanup scans processes and waits; keep it outside the data lock.
    let killed = crate::pty_term::kill_all(&terminals.iter().collect::<Vec<_>>());
    state.persist()?;
    crate::thumbnails::release_later(&state.data_path, thumbnails, &state.data.lock());
    killed
}

pub fn open_project(state: State<Arc<AppState>>, project_id: String) -> Result<Project> {
    let mut data = state.data.lock();
    let project = data
        .projects
        .iter_mut()
        .find(|project| project.id == project_id)
        .ok_or_else(|| Error::not_found("project not found"))?;
    paths::ensure_dir(&PathBuf::from(&project.path))?;
    project.last_opened_at = now_rfc3339();
    let clone = project.clone();
    drop(data);
    state.persist()?;
    Ok(clone)
}

pub async fn rename_project(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    name: String,
) -> Result<Project> {
    let state = state.inner().clone();
    native_task(move || {
        let mut data = state.data.lock();
        state.ensure_running()?;
        let project = crate::sidebar::rename_project(&mut data, &project_id, &name)?;
        persist::save(&state.data_path, &data)?;
        Ok(project)
    })
    .await
}

/// Saves a project's avatar choices, or clears them with `null` (ADR-175).
pub async fn set_project_appearance(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    appearance: Option<crate::models::ProjectAppearance>,
) -> Result<Project> {
    let state = state.inner().clone();
    native_task(move || {
        let mut data = state.data.lock();
        state.ensure_running()?;
        let project = crate::sidebar::set_appearance(&mut data, &project_id, appearance)?;
        persist::save(&state.data_path, &data)?;
        Ok(project)
    })
    .await
}

// Disk/process work runs off the native event thread. Paths are authorized in Rust,
// and read-only operations release the state lock before waiting on Git or the FS.
pub(crate) async fn native_task<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    crate::runtime::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|_| Error::new("native", "Native operation could not complete."))?
}

pub(crate) fn session_path(state: &AppState, id: &str) -> Result<PathBuf> {
    let data = state.data.lock();
    let session = data
        .sessions
        .iter()
        .find(|session| session.id == id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    session_cwd(&data, session)
}

pub async fn git_identity(path: String) -> Result<crate::models::GitIdentity> {
    native_task(move || {
        let canonical = paths::ensure_dir(&PathBuf::from(path))?;
        git::identity(&canonical)
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDiff {
    project_id: String,
    additions: u32,
    deletions: u32,
}

/// Uncommitted `+N -M` of every project checkout, for the sidebar's project rows.
pub async fn project_diff_stats(state: State<'_, Arc<AppState>>) -> Result<Vec<ProjectDiff>> {
    let projects: Vec<(String, String)> = state
        .data
        .lock()
        .projects
        .iter()
        .map(|project| (project.id.clone(), project.path.clone()))
        .collect();
    // Polled for the sidebar: the app's own background Git (ADR-143).
    native_task(move || {
        crate::process_priority::background(|| {
            Ok(projects
                .into_iter()
                .filter_map(|(project_id, path)| {
                    let canonical = paths::ensure_dir(&PathBuf::from(path)).ok()?;
                    let (additions, deletions) = git::diff_totals(&canonical).ok()?;
                    Some(ProjectDiff {
                        project_id,
                        additions,
                        deletions,
                    })
                })
                .collect())
        })
    })
    .await
}

pub async fn git_status(state: State<'_, Arc<AppState>>, session_id: String) -> Result<GitStatus> {
    let state = state.inner().clone();
    // Refreshed after every change and on focus: polling, so it runs below the person.
    native_task(move || {
        crate::process_priority::background(|| git::status(&session_path(&state, &session_id)?))
    })
    .await
}

pub async fn git_diff(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> Result<String> {
    let state = state.inner().clone();
    native_task(move || git::diff_file(&session_path(&state, &session_id)?, &path)).await
}

pub async fn list_branches(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<Vec<crate::models::BranchInfo>> {
    let state = state.inner().clone();
    native_task(move || {
        let path = {
            let data = state.data.lock();
            data.projects
                .iter()
                .find(|project| project.id == project_id)
                .map(|project| PathBuf::from(&project.path))
                .ok_or_else(|| Error::not_found("project not found"))?
        };
        git::list_branches(&path)
    })
    .await
}

pub async fn checkout_branch(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    branch: String,
    confirm: bool,
) -> Result<()> {
    if !confirm {
        return Err(Error::new(
            "confirmation_required",
            "Switching branches requires confirmation.",
        ));
    }
    let state = state.inner().clone();
    native_task(move || {
        let path = {
            let data = state.data.lock();
            data.projects
                .iter()
                .find(|project| project.id == project_id)
                .map(|project| PathBuf::from(&project.path))
                .ok_or_else(|| Error::not_found("project not found"))?
        };
        git::checkout_branch(&path, &branch)
    })
    .await
}

pub async fn create_branch(
    state: State<'_, Arc<AppState>>,
    project_id: String,
    branch: String,
    confirm: bool,
) -> Result<()> {
    if !confirm {
        return Err(Error::new(
            "confirmation_required",
            "Creating a branch requires confirmation.",
        ));
    }
    let state = state.inner().clone();
    native_task(move || {
        let path = {
            let data = state.data.lock();
            data.projects
                .iter()
                .find(|project| project.id == project_id)
                .map(|project| PathBuf::from(&project.path))
                .ok_or_else(|| Error::not_found("project not found"))?
        };
        git::create_branch(&path, &branch)
    })
    .await
}

pub async fn git_workspace_action(
    state: State<'_, Arc<AppState>>,
    action: crate::git_workspace::Action,
) -> Result<crate::git_workspace::Response> {
    let state = state.inner().clone();
    native_task(move || {
        use crate::git_workspace::{self as workspace, Action, Response};
        let session_id = action.session_id().to_owned();
        let cwd = session_path(&state, &session_id)?;
        let recheck = || -> Result<()> {
            state.ensure_running()?;
            if session_path(&state, &session_id)? != cwd {
                return Err(Error::invalid_path(
                    "Workspace changed. Refresh before continuing.",
                ));
            }
            Ok(())
        };
        recheck()?;
        let result = match action {
            Action::Snapshot { .. } => Response::Snapshot {
                snapshot: workspace::snapshot(&cwd)?,
            },
            Action::Stage {
                paths,
                expected_index,
                ..
            } => Response::Snapshot {
                snapshot: workspace::prepare_admitted(
                    &cwd,
                    &paths,
                    true,
                    &expected_index,
                    recheck,
                )?,
            },
            Action::Unstage {
                paths,
                expected_index,
                ..
            } => Response::Snapshot {
                snapshot: workspace::prepare_admitted(
                    &cwd,
                    &paths,
                    false,
                    &expected_index,
                    recheck,
                )?,
            },
            Action::Discard {
                paths,
                expected_index,
                confirm_delete,
                ..
            } => Response::Snapshot {
                snapshot: workspace::discard_admitted(
                    &cwd,
                    &paths,
                    &expected_index,
                    confirm_delete,
                    recheck,
                )?,
            },
            Action::Diff {
                path,
                staged,
                expected_index,
                ..
            } => Response::Diff {
                diff: workspace::diff(&cwd, &path, staged, &expected_index)?,
            },
            Action::History { from, skip, .. } => {
                let (entries, truncated) = workspace::history_from(&cwd, &from, skip)?;
                Response::History { entries, truncated }
            }
            Action::Pull { confirm, .. } => {
                if !confirm {
                    return Err(Error::new("invalid", "Pull requires confirmation."));
                }
                let (branch, summary) = git::pull(&cwd)?;
                Response::Pulled { branch, summary }
            }
        };
        recheck()?;
        Ok(result)
    })
    .await
}

pub async fn git_commit(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    message: String,
    expected_index: Option<String>,
    amend: Option<bool>,
) -> Result<GitCommitResult> {
    let state = state.inner().clone();
    native_task(move || {
        let cwd = session_path(&state, &session_id)?;
        state.ensure_running()?;
        let admit = || {
            state.ensure_running()?;
            if session_path(&state, &session_id)? != cwd {
                return Err(Error::invalid_path(
                    "Workspace changed. Refresh before continuing.",
                ));
            }
            Ok(())
        };
        let result = if amend.unwrap_or(false) {
            git::amend_checked(&cwd, &message, expected_index.as_deref(), admit)?
        } else {
            git::commit_checked(&cwd, &message, expected_index.as_deref(), admit)?
        };
        if session_path(&state, &session_id)? != cwd {
            return Err(Error::invalid_path(
                "Workspace changed. Refresh before continuing.",
            ));
        }
        // Committed files leave the turn reviews of the sessions working here (ADR-108).
        let paths = crate::turn_review::committed_paths(&cwd, &result.hash);
        let mut data = state.data.lock();
        let workspace = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .map(|session| session.worktree.path.clone());
        let changed = workspace
            .map(|path| crate::turn_review::mark_committed(&mut data, &path, &paths))
            .unwrap_or_default();
        if !changed.is_empty() {
            for session in data
                .sessions
                .iter()
                .filter(|session| changed.contains(&session.id))
            {
                crate::transcript_view::emit(&app, session);
            }
            if let Err(error) = persist::save(&state.data_path, &data) {
                tracing::warn!(%error, "cannot persist committed review files");
            }
        }
        Ok(result)
    })
    .await
}

pub async fn git_push(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<GitPushResult> {
    let state = state.inner().clone();
    native_task(move || git::push(&session_path(&state, &session_id)?)).await
}

pub async fn project_remote_url(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<Option<String>> {
    let state = state.inner().clone();
    native_task(move || {
        let path = {
            let data = state.data.lock();
            data.projects
                .iter()
                .find(|project| project.id == project_id)
                .map(|project| PathBuf::from(&project.path))
                .ok_or_else(|| Error::not_found("project not found"))?
        };
        git::remote_web_url(&path)
    })
    .await
}

pub(crate) fn pull_request_context(
    state: &AppState,
    session_id: &str,
) -> Result<crate::pull_requests::Context> {
    state.ensure_running()?;
    crate::pull_requests::inspect(session_path(state, session_id)?)
}

fn validate_pull_request_context(
    state: &AppState,
    session_id: &str,
    expected: &crate::pull_requests::Context,
) -> Result<()> {
    if &pull_request_context(state, session_id)? != expected {
        return Err(Error::new(
            "stale",
            "The PR workspace changed. Refresh to load its current state.",
        ));
    }
    Ok(())
}

pub async fn session_pull_request(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<crate::pull_requests::PullRequestSnapshot> {
    let state = state.inner().clone();
    let initial_state = state.clone();
    let initial_id = session_id.clone();
    let context = native_task(move || pull_request_context(&initial_state, &initial_id)).await?;
    let snapshot = match &context.forge {
        Some(remote) => crate::forge::load(session_id.clone(), &context, remote).await,
        None => crate::pull_requests::load(session_id.clone(), &context).await,
    };
    native_task(move || {
        // Never publish another branch/repository's result after selection or Git changes.
        validate_pull_request_context(&state, &session_id, &context)?;
        Ok(snapshot)
    })
    .await
}

pub async fn list_worktrees(
    state: State<'_, Arc<AppState>>,
    project_id: String,
) -> Result<Vec<crate::models::GitWorktree>> {
    let state = state.inner().clone();
    native_task(move || {
        let path = {
            let data = state.data.lock();
            data.projects
                .iter()
                .find(|project| project.id == project_id)
                .map(|project| PathBuf::from(&project.path))
                .ok_or_else(|| Error::not_found("project not found"))?
        };
        git::list_worktrees(&path)
    })
    .await
}

pub async fn list_dir(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: Option<String>,
    show_ignored: Option<bool>,
) -> Result<Vec<FileEntry>> {
    let state = state.inner().clone();
    native_task(move || {
        let cwd = session_path(&state, &session_id)?;
        let dir = path.map(PathBuf::from).unwrap_or_else(|| cwd.clone());
        fs_tree::list_children(&cwd, &dir, show_ignored.unwrap_or(false))
    })
    .await
}

/// Read-only text search in the session's workspace for the Files pane (ADR-095).
pub async fn search_workspace_text(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    query: String,
    case_sensitive: Option<bool>,
) -> Result<crate::models::TextSearchResult> {
    let state = state.inner().clone();
    native_task(move || {
        state.ensure_running()?;
        let root = session_path(&state, &session_id)?;
        let result = crate::text_search::search(&root, &query, case_sensitive.unwrap_or(false))?;
        state.ensure_running()?;
        if session_path(&state, &session_id)? != root {
            return Err(Error::invalid_path("workspace changed during text search"));
        }
        Ok(result)
    })
    .await
}

pub async fn create_workspace_entry(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    kind: crate::models::WorkspaceEntryKind,
    name: String,
    parent_path: Option<String>,
) -> Result<FileEntry> {
    let state = state.inner().clone();
    native_task(move || {
        create_workspace_entry_native(&state, &session_id, kind, &name, parent_path.as_deref())
    })
    .await
}

/// Moves one session-owned workspace entry to the system Trash (recoverable). Explicit and
/// confirmed in the UI; never a permanent delete.
pub async fn trash_workspace_entry(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    path: String,
) -> Result<()> {
    let state = state.inner().clone();
    native_task(move || {
        let data = state.data.lock();
        state.ensure_running()?;
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        let root = session_cwd(&data, session)?;
        crate::workspace_entries::trash_entry(&root, std::path::Path::new(&path))
    })
    .await
}

fn create_workspace_entry_native(
    state: &AppState,
    session_id: &str,
    kind: crate::models::WorkspaceEntryKind,
    name: &str,
    parent_path: Option<&str>,
) -> Result<FileEntry> {
    // Keep owner metadata stable through this small filesystem mutation. Session
    // or project deletion must not overtake a worker waiting to create an entry.
    let data = state.data.lock();
    state.ensure_running()?;
    let session = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    let root = session_cwd(&data, session)?;
    crate::workspace_entries::create_entry(
        &root,
        parent_path.map(std::path::Path::new),
        kind,
        name,
        || {
            state.ensure_running()?;
            if session_cwd(&data, session)? != root {
                return Err(Error::invalid_path(
                    "Workspace destination changed. Try again.",
                ));
            }
            Ok(())
        },
    )
}

fn workspace_files_root(data: &AppData, owner: &crate::skills::Owner) -> Result<PathBuf> {
    if let Some(id) = &owner.session_id {
        let session = data
            .sessions
            .iter()
            .find(|session| &session.id == id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        if owner
            .project_id
            .as_ref()
            .is_some_and(|id| id != &session.project_id)
        {
            return Err(Error::invalid_path("foreign workspace owner"));
        }
        return session_cwd(data, session);
    }
    let project = data
        .projects
        .iter()
        .find(|project| Some(&project.id) == owner.project_id.as_ref())
        .ok_or_else(|| Error::not_found("project not found"))?;
    paths::ensure_dir(std::path::Path::new(&project.path))
}

pub async fn workspace_files(
    state: State<'_, Arc<AppState>>,
    owner: crate::skills::Owner,
    query: String,
) -> Result<fs_tree::WorkspaceFiles> {
    let state = state.inner().clone();
    native_task(move || {
        state.ensure_running()?;
        let root = workspace_files_root(&state.data.lock(), &owner)?;
        let result = fs_tree::suggest_files(&root, &query)?;
        state.ensure_running()?;
        if workspace_files_root(&state.data.lock(), &owner)? != root {
            return Err(Error::invalid_path("workspace changed during file search"));
        }
        Ok(result)
    })
    .await
}

pub async fn detect_agents(state: State<'_, Arc<AppState>>) -> Result<Vec<AgentInstall>> {
    let overrides = state.data.lock().settings.provider_paths.clone();
    let installs = crate::runtime::async_runtime::spawn_blocking(move || {
        detect::detect_with_overrides(&overrides)
    })
    .await
    .map_err(|err| Error::agent(err.to_string()))?;
    let mut tasks = tokio::task::JoinSet::new();
    for mut install in installs {
        tasks.spawn(async move {
            if install.installed {
                if let Some(path) = &install.path {
                    install.version = detect::probe_version(path).await;
                }
            }
            // Each alternative install reports its own version (bounded probes, concurrently).
            let current = install.path.clone();
            let version = install.version.clone();
            let mut probes = tokio::task::JoinSet::new();
            let paths: Vec<String> = install
                .candidates
                .iter()
                .map(|item| item.path.clone())
                .collect();
            for (index, path) in paths.into_iter().enumerate() {
                if current.as_deref() == Some(path.as_str()) && version.is_some() {
                    install.candidates[index].version = version.clone();
                    continue;
                }
                probes.spawn(async move { (index, detect::probe_version(&path).await) });
            }
            while let Some(Ok((index, version))) = probes.join_next().await {
                install.candidates[index].version = version;
            }
            install
        });
    }
    let mut result = Vec::new();
    while let Some(install) = tasks.join_next().await {
        result.push(install.map_err(|err| Error::agent(err.to_string()))?);
    }
    Ok(result)
}

pub async fn create_session(
    state: State<'_, Arc<AppState>>,
    request: CreateSessionRequest,
) -> Result<Session> {
    let state = state.inner().clone();
    native_task(move || create_session_native(&state, request)).await
}

fn create_session_native(state: &AppState, request: CreateSessionRequest) -> Result<Session> {
    let start = match base_branch_start(state, &request)? {
        Some(commit) => commit,
        None => new_worktree_start(state, &request),
    };
    let mut data = state.data.lock();
    let session = create_session_locked(state, &mut data, request, &start)?;
    drop(data);
    state.persist()?;
    Ok(session)
}

/// Where a requested isolated worktree starts (ADR-130): origin's default branch after a
/// fetch when "Start new worktrees from origin" is on, else `HEAD`. Runs before the data lock
/// is taken, since the fetch reaches the network.
pub(crate) fn new_worktree_start(state: &AppState, request: &CreateSessionRequest) -> String {
    let root = {
        let data = state.data.lock();
        if !request.isolated_worktree
            || request.share_worktree_of.is_some()
            || !data.settings.worktrees_from_origin
        {
            return "HEAD".into();
        }
        data.projects
            .iter()
            .find(|project| project.id == request.project_id)
            .map(|project| PathBuf::from(&project.path))
    };
    let commit = root.and_then(|root| worktree::origin_commit(&root));
    worktree::start_point(true, commit)
}

/// The commit a new isolated worktree starts from when the request names a local branch to start
/// from; None when it names none. A branch that does not exist is refused rather than silently
/// starting somewhere else.
fn base_branch_start(state: &AppState, request: &CreateSessionRequest) -> Result<Option<String>> {
    let Some(branch) = request
        .base_branch
        .as_deref()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
    else {
        return Ok(None);
    };
    if !request.isolated_worktree || request.share_worktree_of.is_some() {
        return Ok(None);
    }
    let root = state
        .data
        .lock()
        .projects
        .iter()
        .find(|project| project.id == request.project_id)
        .map(|project| PathBuf::from(&project.path))
        .ok_or_else(|| Error::not_found("project not found"))?;
    worktree::branch_commit(&root, branch)
        .map(Some)
        .ok_or_else(|| Error::new("invalid", "Branch not found"))
}

/// Creates and inserts one session under an already-held data lock. Isolated worktrees start at
/// `start` (a commit or `HEAD`). The caller persists.
/// A new isolated worktree for `id`, under the configured location and branch pattern.
fn isolated_worktree(
    state: &AppState,
    data: &AppData,
    project: &crate::models::Project,
    id: &str,
    title: &str,
    start: &str,
) -> Result<Worktree> {
    let pattern = data.settings.worktree_branch_pattern.clone();
    let parent = if matches!(
        data.settings.worktree_location,
        WorktreeLocationPref::Custom
    ) {
        data.settings
            .worktree_base_path
            .as_ref()
            .map(|path| PathBuf::from(path).join(&project.id))
    } else {
        None
    }
    .unwrap_or_else(|| state.worktree_root.join(&project.id));
    worktree::create_isolated_at(
        &PathBuf::from(&project.path),
        &parent,
        id,
        title,
        &pattern,
        start,
    )
}

/// The folder of `source`, a session of the same project, for a new session that works beside
/// it. Cleanup keeps a worktree while another session still uses it (ADR-163).
fn shared_worktree(data: &AppData, project_id: &str, source: &str) -> Result<Worktree> {
    let tree = data
        .sessions
        .iter()
        .find(|session| session.id == source && session.project_id == project_id)
        .map(|session| session.worktree.clone())
        .ok_or_else(|| Error::not_found("session not found"))?;
    if !PathBuf::from(&tree.path).is_dir() {
        return Err(Error::new(
            "invalid",
            "That session's worktree no longer exists.",
        ));
    }
    Ok(tree)
}

pub(crate) fn create_session_locked(
    state: &AppState,
    data: &mut AppData,
    request: CreateSessionRequest,
    start: &str,
) -> Result<Session> {
    state.ensure_running()?;
    let project = data
        .projects
        .iter()
        .find(|project| project.id == request.project_id)
        .cloned()
        .ok_or_else(|| Error::not_found("project not found"))?;
    if !data.provider_enabled(&request.agent) {
        return Err(Error::agent("this provider is disabled in Settings"));
    }
    if let Some(model) = &request.model {
        if data
            .settings
            .disabled_models
            .contains(&format!("{}::{model}", request.agent.key()))
        {
            return Err(Error::agent("this model is disabled in Settings"));
        }
    }
    let now = now_rfc3339();
    let id = Uuid::new_v4().to_string();
    let title = request
        .title
        .map(|title| title.trim().to_string())
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "New session".into());
    if title.chars().count() > 200 {
        return Err(Error::new(
            "invalid",
            "session title exceeds 200 characters",
        ));
    }
    paths::ensure_dir(&PathBuf::from(&project.path))?;

    let shared = match &request.share_worktree_of {
        Some(source) => Some(shared_worktree(data, &project.id, source)?),
        None => None,
    };
    let worktree = if let Some(tree) = shared {
        tree
    } else if request.isolated_worktree {
        isolated_worktree(state, data, &project, &id, &title, start)?
    } else {
        let identity = git::identity(&PathBuf::from(&project.path))?;
        Worktree {
            path: project.path.clone(),
            branch: identity.branch.unwrap_or_else(|| "unknown".into()),
            isolated: false,
        }
    };

    let account_id = crate::provider_accounts::selected(data, &request.agent);
    let session = Session {
        context_usage: None,
        delegated_by: None,
        usage_limit: None,
        stopped_by_user_at: None,
        auto_continuations: 0,
        snoozed_until: None,
        snooze_reminder_at: None,
        goal: None,
        pinned_message_ids: vec![],
        fork_origin: None,
        import_origin: None,
        moved_to: None,
        moved_from: None,
        handoff: None,
        account_bindings: HashMap::from([(request.agent.clone(), account_id.clone())]),
        id,
        title,
        project_id: project.id,
        provider_account_id: account_id,
        agent: request.agent,
        status: SessionStatus::Idle,
        created_at: now.clone(),
        last_activity_at: now,
        worktree,
        messages: Default::default(),
        last_error: None,
        model: request.model.clone(),
        native_thread: None,
        execution: Default::default(),
        pending_requests: vec![],
        team: None,
        team_worker: None,
        side_chat: None,
        pull_ask: None,
        // A new isolated worktree runs the project's Setup script before its first turn.
        scripts: crate::project_scripts::SessionScripts::new(request.isolated_worktree),
    };
    data.sessions.insert(0, session.clone());
    Ok(session)
}

pub async fn fork_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    message_id: String,
) -> Result<Session> {
    let state = state.inner().clone();
    native_task(move || fork_session_native(&state, &session_id, &message_id)).await
}

fn fork_session_native(state: &AppState, session_id: &str, message_id: &str) -> Result<Session> {
    let mut data = state.data_with_messages(&[session_id]);
    state.ensure_running()?;
    let source = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    // Ask conversations stay read-only beside their pull request (ADR-050).
    if source.pull_ask.is_some() {
        return Err(Error::agent("An Ask conversation cannot be forked."));
    }
    if state.agents.lock().contains_key(session_id) {
        return Err(Error::agent(
            "Stop or finish the source session before forking.",
        ));
    }
    if !data.provider_enabled(&source.agent)
        || source.model.as_ref().is_some_and(|model| {
            data.settings
                .disabled_models
                .contains(&format!("{}::{model}", source.agent.key()))
        })
    {
        return Err(Error::agent(
            "Enable this provider and model in Settings before forking.",
        ));
    }
    let cwd = session_cwd(&data, source)?;
    let id = Uuid::new_v4().to_string();
    // Validate the boundary and budget before any Git/filesystem mutation.
    let mut fork = crate::transcript::fork_snapshot(
        source,
        message_id,
        &id,
        &now_rfc3339(),
        source.worktree.clone(),
    )?;
    // Outputs stored apart travel with the copy, which saves them under its own ids (ADR-147).
    crate::tool_output::restore_fork(&state.data_path, source, &mut fork.messages[..]);
    let identity = git::identity(&cwd)?;
    fork.worktree = if identity.is_repo {
        let parent = if matches!(
            data.settings.worktree_location,
            WorktreeLocationPref::Custom
        ) {
            data.settings
                .worktree_base_path
                .as_ref()
                .map(|path| PathBuf::from(path).join(&source.project_id))
        } else {
            None
        }
        .unwrap_or_else(|| state.worktree_root.join(&source.project_id));
        worktree::create_isolated(
            &cwd,
            &parent,
            &id,
            &fork.title,
            &data.settings.worktree_branch_pattern,
        )?
    } else {
        // Never give two sessions destructive ownership of the source worktree.
        if source.worktree.isolated {
            return Err(Error::git(
                "The source worktree is no longer a Git repository.",
            ));
        }
        Worktree {
            path: display_path(&cwd),
            branch: identity.branch.unwrap_or_else(|| "unknown".into()),
            isolated: false,
        }
    };
    fork.scripts = crate::project_scripts::SessionScripts::new(fork.worktree.isolated);
    data.sessions.insert(0, fork.clone());
    // Keep native identity and transcript together under the admission lock.
    persist::save(&state.data_path, &data)?;
    Ok(fork)
}

pub async fn handoff_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    message_id: String,
    agent: AgentProviderId,
    model: Option<String>,
    new_worktree: Option<bool>,
) -> Result<Session> {
    let state = state.inner().clone();
    native_task(move || {
        handoff_session_native(
            &state,
            &session_id,
            &message_id,
            agent,
            model,
            new_worktree.unwrap_or(false),
        )
    })
    .await
}

fn handoff_session_native(
    state: &AppState,
    session_id: &str,
    message_id: &str,
    agent: AgentProviderId,
    model: Option<String>,
    new_worktree: bool,
) -> Result<Session> {
    // A move to a new worktree starts from the source checkout as it is now,
    // uncommitted work included (the same private snapshot Team uses). The
    // snapshot runs Git before the lock; the workspace is rechecked after.
    let snapshot = if new_worktree {
        let cwd = session_path(state, session_id)?;
        let base = crate::team::snapshot_base(&cwd, &state.worktree_root.join(".team-snapshots"))?;
        Some((cwd, base))
    } else {
        None
    };
    let mut data = state.data_with_messages(&[session_id]);
    state.ensure_running()?;
    let source = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
        .ok_or_else(|| Error::not_found("session not found"))?;
    if source.pull_ask.is_some() {
        return Err(Error::agent("An Ask conversation cannot be handed off."));
    }
    if state.agents.lock().contains_key(session_id) || source.status.is_active() {
        return Err(Error::agent(
            "Stop or finish the source session before handing it off.",
        ));
    }
    if !data.provider_enabled(&agent) {
        return Err(Error::agent(
            "Enable this provider in Settings before handing off.",
        ));
    }
    if let Some(model) = &model {
        if data
            .settings
            .disabled_models
            .contains(&format!("{}::{model}", agent.key()))
        {
            return Err(Error::agent(
                "Enable this model in Settings before handing off.",
            ));
        }
    }
    let provider_account_id = crate::provider_accounts::selected(&data, &agent);
    let id = Uuid::new_v4().to_string();
    // A handoff continues in the source workspace, or in a new isolated worktree
    // seeded from it; uncommitted work is kept either way.
    let worktree = match &snapshot {
        Some((cwd, base)) => {
            if &session_cwd(&data, &source)? != cwd {
                return Err(Error::agent("The workspace changed; try again."));
            }
            let project = data
                .projects
                .iter()
                .find(|project| project.id == source.project_id)
                .cloned()
                .ok_or_else(|| Error::not_found("project not found"))?;
            isolated_worktree(state, &data, &project, &id, &source.title, base)?
        }
        None => source.worktree.clone(),
    };
    let created = snapshot.is_some().then(|| worktree.clone());
    let handoff = crate::transcript::handoff_snapshot(
        &source,
        message_id,
        &id,
        &now_rfc3339(),
        agent,
        model,
        provider_account_id,
        worktree,
    );
    let mut handoff = match handoff {
        Ok(handoff) => handoff,
        Err(error) => {
            // Never leave the new worktree behind; it holds nothing yet.
            if let (Some(tree), Some(project)) = (
                &created,
                data.projects
                    .iter()
                    .find(|project| project.id == source.project_id),
            ) {
                let _ = worktree::remove(&PathBuf::from(&project.path), tree, true);
            }
            return Err(error);
        }
    };
    handoff.scripts = crate::project_scripts::SessionScripts::new(created.is_some());
    data.sessions.insert(0, handoff.clone());
    persist::save(&state.data_path, &data)?;
    Ok(handoff)
}

pub async fn dismiss_handoff(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Session> {
    let state = state.inner().clone();
    native_task(move || {
        let mut data = state.data.lock();
        state.ensure_running()?;
        let previous = find_session_mut(&mut data, &session_id)?.handoff.take();
        let session = find_session_mut(&mut data, &session_id)?.clone();
        if let Err(error) = persist::save(&state.data_path, &data) {
            find_session_mut(&mut data, &session_id)?.handoff = previous;
            return Err(error);
        }
        Ok(session)
    })
    .await
}

pub async fn set_message_pinned(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    message_id: String,
    pinned: bool,
) -> Result<Vec<String>> {
    let state = state.inner().clone();
    native_task(move || set_message_pinned_native(&state, &session_id, &message_id, pinned)).await
}

fn set_message_pinned_native(
    state: &AppState,
    session_id: &str,
    message_id: &str,
    pinned: bool,
) -> Result<Vec<String>> {
    let mut data = state.data.lock();
    state.ensure_running()?;
    let session = find_session_mut(&mut data, session_id)?;
    let previous = session.pinned_message_ids.clone();
    crate::transcript::set_pinned(session, message_id, pinned)?;
    let result = session.pinned_message_ids.clone();
    if let Err(error) = persist::save(&state.data_path, &data) {
        find_session_mut(&mut data, session_id)?.pinned_message_ids = previous;
        return Err(error);
    }
    Ok(result)
}

pub fn set_session_agent(
    state: State<Arc<AppState>>,
    session_id: String,
    agent: AgentProviderId,
) -> Result<Session> {
    let mut data = state.data.lock();
    let account_id = crate::provider_accounts::selected(&data, &agent);
    let session = find_session_mut(&mut data, &session_id)?;
    if session.status.is_active() {
        return Err(Error::agent(
            "cannot change a running session provider or model",
        ));
    }
    if session.agent != agent {
        let account_id = crate::provider_accounts::bound_account(session, &agent, &account_id);
        apply_session_selection(session, agent, None);
        session.provider_account_id = account_id;
    }
    let clone = session.clone();
    drop(data);
    state.persist()?;
    Ok(clone)
}

pub fn rename_session(
    state: State<Arc<AppState>>,
    session_id: String,
    title: String,
) -> Result<serde_json::Value> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        return Err(Error::new(
            "invalid",
            "session title must contain 1–200 characters",
        ));
    }
    let mut data = state.data.lock();
    let session = find_session_mut(&mut data, &session_id)?;
    session.title = title.to_string();
    session.last_activity_at = now_rfc3339();
    // Metadata only: callers read the title and time, and a long transcript would make a
    // rename from a paired phone wait on megabytes it never uses.
    let meta = crate::transcript_view::session_meta(session)?;
    drop(data);
    state.persist()?;
    Ok(meta)
}

pub async fn delete_session(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    remove_worktree: bool,
    confirm: bool,
) -> Result<()> {
    let state = state.inner().clone();
    native_task(move || delete_session_native(&state, session_id, remove_worktree, confirm)).await
}

pub(crate) fn delete_session_native(
    state: &AppState,
    session_id: String,
    remove_worktree: bool,
    confirm: bool,
) -> Result<()> {
    let mut data = state.data.lock();
    let session = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
        .ok_or_else(|| Error::not_found("session not found"))?;
    let project = data
        .projects
        .iter()
        .find(|project| project.id == session.project_id)
        .cloned();
    if session.status.is_active() || state.agents.lock().contains_key(&session_id) {
        return Err(Error::agent("stop the agent before deleting this session"));
    }
    // Side chats share their parent's workspace and go away with it (ADR-049).
    if remove_worktree && session.side_chat.is_some() {
        return Err(Error::agent(
            "A side chat uses its main session's workspace; it cannot remove it.",
        ));
    }
    // An Ask conversation runs in the project checkout (ADR-050).
    if remove_worktree && session.pull_ask.is_some() {
        return Err(Error::agent(
            "An Ask conversation uses the project's checkout; it cannot remove it.",
        ));
    }
    let side_chats = crate::side_chat::children(&data.sessions, &session_id);
    // Another session started in this worktree (delegation, ADR-163) still works there.
    if remove_worktree
        && session.worktree.isolated
        && data.sessions.iter().any(|item| {
            item.id != session_id
                && !side_chats.contains(&item.id)
                && item.worktree.path == session.worktree.path
        })
    {
        return Err(Error::agent(
            "Another session still works in this worktree; delete without removing it.",
        ));
    }
    {
        let agents = state.agents.lock();
        if data.sessions.iter().any(|item| {
            side_chats.contains(&item.id)
                && (item.status.is_active() || agents.contains_key(&item.id))
        }) {
            return Err(Error::agent(
                "Stop this session's side chat before deleting it.",
            ));
        }
    }
    crate::project_scripts::cancel(&session_id);
    crate::agent_pool::discard(&session_id);
    for side in &side_chats {
        crate::agent_pool::discard(side);
    }
    if remove_worktree {
        let project = project.ok_or_else(|| Error::not_found("project not found"))?;
        worktree::remove(&PathBuf::from(project.path), &session.worktree, confirm)?;
    }
    let terminals = {
        let mut ptys = state.ptys.lock();
        let owned = ptys
            .values()
            .filter(|pty| pty.session_id == session_id || side_chats.contains(&pty.session_id))
            .map(|pty| pty.terminal_id.clone())
            .collect::<Vec<_>>();
        owned
            .into_iter()
            .filter_map(|terminal_id| ptys.remove(&terminal_id))
            .collect::<Vec<_>>()
    };
    let (removed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut data.sessions)
        .into_iter()
        .partition(|item| item.id == session_id || side_chats.contains(&item.id));
    data.sessions = kept;
    crate::review_guides::prune(&mut data);
    crate::pr_watch::prune(&mut data);
    crate::drafts::prune(&mut data);
    crate::context_text::prune(&mut data);
    crate::sidebar::prune(&mut data);
    state.attachments.prune(&data);
    drop(data);
    // Read before the save removes their transcript files.
    let thumbnails = crate::thumbnails::shown_by(&removed);
    // Shell cleanup scans processes and waits; keep it outside the data lock.
    let killed = crate::pty_term::kill_all(&terminals.iter().collect::<Vec<_>>());
    state.persist()?;
    crate::thumbnails::release_later(&state.data_path, thumbnails, &state.data.lock());
    killed
}

/// "Send again" (a turn that ended without a reply): the conversation's last message
/// goes out once more with the same execution options and, while the app still holds
/// them, the same attachments.
pub async fn retry_last_turn(
    app: crate::runtime::AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<serde_json::Value> {
    let (mut request, sent) = {
        let data = state.data_with_messages(&[&session_id]);
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        if session.status.is_active() || session.status == SessionStatus::Waiting {
            return Err(Error::new("busy", "The conversation is still working."));
        }
        let message = session
            .messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .ok_or_else(|| Error::new("invalid", "There is no message to send again."))?;
        let request = SendPromptRequest {
            queued_after: None,
            debugging: false,
            goal: None,
            session_id: session.id.clone(),
            // Filled below, after the lock: images may be admitted again from their copies.
            attachment_ids: Vec::new(),
            attachment_owner: format!("session:{}", session.id),
            prompt: message.content.clone(),
            execution: session.execution.clone(),
            team: false,
            // A new turn by choice: reusing the original send's id would make it a no-op.
            // The busy check above already refuses a second click while this one starts.
            client_request_id: None,
        };
        (request, message.attachments.clone())
    };
    request.attachment_ids =
        crate::attachments::retry_ids(&state, &request.attachment_owner, &sent)?;
    send_prompt(app, state, request).await
}

/// Under the admission lock: the session as it is now when this send was admitted
/// before (a retried or doubled delivery), so the caller gets what the original
/// send returned instead of a second turn.
fn replayed_send(
    session: &Session,
    client_request_id: Option<&str>,
) -> Result<Option<serde_json::Value>> {
    match client_request_id {
        Some(id) if crate::execution::already_sent(session, id) => {
            crate::transcript_view::session_tail(session).map(Some)
        }
        _ => Ok(None),
    }
}

pub async fn send_prompt(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    request: SendPromptRequest,
) -> Result<serde_json::Value> {
    let mut request = request;
    if request.team {
        // A team plan is always a read-only planning turn (ADR-043).
        if request.debugging {
            return Err(Error::agent("Debug and team modes cannot be combined."));
        }
        request.execution.planning = true;
        if request.execution.approval == Some(crate::models::ApprovalMode::Full) {
            request.execution.approval = Some(crate::models::ApprovalMode::Ask);
        }
    }
    let prompt = request.prompt.trim().to_string();
    if request.debugging && request.execution.planning {
        return Err(Error::agent("Debug and planning modes cannot be combined."));
    }
    let admitted_goal = request
        .goal
        .as_deref()
        .map(crate::goals::validate)
        .transpose()?;
    if prompt.is_empty() {
        return Err(Error::new("invalid", "prompt is empty"));
    }
    if prompt.len() > 64 * 1024 {
        return Err(Error::new("invalid", "prompt exceeds the 64 KiB limit"));
    }
    let client_request_id =
        crate::execution::client_request_id(request.client_request_id.as_deref())?;
    if request.execution.effort.is_some() || request.execution.fast {
        ensure_model_catalog(state.inner(), &request.session_id).await;
    }

    // A standalone `/compact` is the provider's own compaction, never text for the model (ADR-057).
    let compacting = crate::codex::is_compact(&prompt);

    // Skill discovery/file reads run on workers before any turn mutation.
    // A skill token may stand anywhere in the prompt (ADR-037), so any slash word triggers discovery.
    let skill_input = if !compacting
        && prompt
            .split_whitespace()
            .any(|token| token.len() > 1 && token.starts_with('/'))
    {
        let state = state.inner().clone();
        let owner = crate::skills::Owner {
            project_id: None,
            session_id: Some(request.session_id.clone()),
        };
        let visible_prompt = prompt.clone();
        Some(native_task(move || crate::skills::prepare(&state, &owner, &visible_prompt)).await?)
    } else {
        None
    };

    // The turn reads this transcript, and a side chat's also its main session's.
    let parent = state
        .data
        .lock()
        .sessions
        .iter()
        .find(|session| session.id == request.session_id)
        .and_then(|session| session.side_chat.as_ref())
        .map(|origin| origin.parent_session_id.clone());
    let (session_snapshot, cwd, provider, model, overrides, process_prompt, attachments) = {
        let mut data = state.data_with_messages(
            &[Some(request.session_id.as_str()), parent.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
        );
        state.ensure_running()?;
        let snapshot = data
            .sessions
            .iter()
            .find(|session| session.id == request.session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        // Checked under the same lock that records the user message below, before any
        // check the original turn itself would now fail (busy, stale queue context).
        if let Some(tail) = replayed_send(snapshot, client_request_id.as_deref())? {
            return Ok(tail);
        }
        crate::execution::validate_queued_context(snapshot, request.queued_after.as_ref())?;
        // An Ask conversation about a pull request is read-only on every turn (ADR-050).
        let pull_ask = snapshot.pull_ask.clone();
        if pull_ask.is_some() {
            if request.team || request.debugging {
                return Err(Error::agent(
                    "An Ask conversation cannot coordinate a team or debug.",
                ));
            }
            request.execution = crate::pull_ask::read_only(
                &snapshot.agent,
                snapshot.model.as_deref(),
                &request.execution,
            )?;
        }
        let cwd = session_cwd(&data, snapshot)?;
        if let Some((context, _, _)) = &skill_input {
            if context.cwd.as_ref() != Some(&cwd)
                || context.provider.as_ref() != Some(&snapshot.agent)
                || context.account_id.as_ref() != Some(&snapshot.provider_account_id)
                || context.disabled != data.settings.disabled_skills
            {
                return Err(Error::new(
                    "stale",
                    "Skill context changed. Review the current session and try again.",
                ));
            }
        }
        let attachments = state.attachments.resolve(
            &request.attachment_owner,
            &request.attachment_ids,
            snapshot,
        )?;
        let catalogs = state.catalogs.lock();
        let catalog = catalogs
            .get(&snapshot.agent)
            .filter(|(path, _)| path.as_ref() == data.settings.provider_paths.get(&snapshot.agent))
            .map(|(_, list)| list);
        crate::execution::validate(
            &snapshot.agent,
            snapshot.model.as_deref(),
            &request.execution,
            catalog,
        )?;
        let process_model = if snapshot.agent == AgentProviderId::Cursor {
            crate::execution::cursor_model(snapshot.model.as_deref(), &request.execution, catalog)?
        } else {
            snapshot.model.clone()
        };
        drop(catalogs);
        let team_catalog = request.team.then(|| crate::team::catalog(&state, &data));
        // A side chat's turn carries the main session as it is right now (ADR-049).
        let side_recap = match &snapshot.side_chat {
            Some(origin) => {
                if request.team {
                    return Err(Error::agent("A side chat cannot coordinate a team."));
                }
                let parent = data
                    .sessions
                    .iter()
                    .find(|item| item.id == origin.parent_session_id)
                    .ok_or_else(|| {
                        Error::not_found("The main session of this side chat was removed.")
                    })?;
                Some(crate::side_chat::recap(parent))
            }
            None => None,
        };
        let rotate_at_limit = data.settings.rotate_at_context_limit;
        let session = find_session_mut(&mut data, &request.session_id)?;
        if session.status.is_active() || state.agents.lock().contains_key(&request.session_id) {
            return Err(Error::agent("session already has a running agent"));
        }
        if compacting {
            if let Some(reason) = crate::transcript::compaction_refusal(
                session,
                !attachments.is_empty(),
                request.team || request.debugging,
            ) {
                return Err(Error::agent(reason));
            }
        }
        if request.team {
            if !crate::team::TEAM_PROVIDERS.contains(&session.agent) {
                return Err(Error::agent(
                    "A team coordinator must be Codex, Claude Code or OpenCode.",
                ));
            }
            if session.team_worker.is_some() {
                return Err(Error::agent(
                    "A team worker cannot coordinate another team.",
                ));
            }
            if session
                .team
                .as_ref()
                .is_some_and(crate::team::Team::blocks_new_plan)
            {
                return Err(Error::agent(
                    "Finish, merge or discard the current team before planning another.",
                ));
            }
        }
        if session.agent == AgentProviderId::Codex {
            crate::codex::validate_binding(session, &display_path(&cwd))?;
        } else if session.agent == AgentProviderId::Claude {
            crate::claude::validate_binding(session, &display_path(&cwd))?;
        } else if session.agent == AgentProviderId::OpenCode {
            crate::opencode::validate_binding(session, &display_path(&cwd))?;
        } else if crate::acp_cli::handles(&session.agent) {
            crate::acp_cli::validate_binding(session, &display_path(&cwd))?;
        }
        // A conversation near the end of its window continues in a fresh provider session
        // seeded with a brief built by code (ADR-181), before the prompt is assembled.
        let rotation = if compacting || request.team {
            None
        } else {
            crate::rotation::apply(session, rotate_at_limit)
        };
        // `/compact` goes alone and verbatim: no recap, wrapper, goal or instructions around it.
        let process_prompt = if compacting {
            prompt.clone()
        } else {
            let images = attachments.iter().filter(|file| file.image()).count() as u64;
            let files = attachments.len() as u64 - images;
            let process_prompt = crate::transcript::process_prompt_with(
                session,
                &prompt,
                crate::token_budget::attachments(images, files),
            )?;
            // A pending handoff recap travels with the first prompt only; the
            // persisted user message keeps the visible text.
            let process_prompt = crate::transcript::consume_handoff(session, process_prompt);
            let process_prompt = match &side_recap {
                Some(recap) => crate::side_chat::wrap(recap, &process_prompt),
                None => process_prompt,
            };
            let process_prompt = match &pull_ask {
                Some(origin) => crate::pull_ask::wrap(origin, &process_prompt),
                None => process_prompt,
            };
            let process_prompt = crate::attachments::file_prompt(&process_prompt, &attachments);
            let process_prompt = match &skill_input {
                Some((_, instructions, _)) if !instructions.is_empty() => {
                    format!("{instructions}\n\nUser request:\n{process_prompt}")
                }
                _ => process_prompt,
            };
            let process_prompt = crate::skills::with_agent_hint(session, process_prompt);
            let process_prompt = crate::execution::debug_prompt(request.debugging, process_prompt);
            let process_prompt = crate::goals::prompt(
                admitted_goal
                    .as_ref()
                    .map(|goal| goal.as_deref())
                    .unwrap_or(session.goal.as_deref()),
                process_prompt,
            )?;
            match &team_catalog {
                Some(catalog) => crate::team::planning_prompt(&process_prompt, catalog),
                None => process_prompt,
            }
        };
        state.attachments.mark_sent(&request.attachment_ids);
        if let Some(goal) = &admitted_goal {
            session.goal = goal.clone();
        }
        // A compaction carries no choices of its own: the session keeps its approval, effort,
        // Fast and plan mode, so the kept process still matches and the person's settings survive.
        if !compacting {
            session.execution = request.execution.clone();
        }
        session.pending_requests.clear();
        let user = Message {
            id: Uuid::new_v4().to_string(),
            session_id: session.id.clone(),
            role: MessageRole::User,
            content: prompt.clone(),
            created_at: now_rfc3339(),
            streaming: false,
            activity: None,
            steers: Vec::new(),
            attachments: attachments
                .iter()
                .map(|file| file.summary(&state.data_path))
                .collect(),
            launched: vec![],
            visuals: Vec::new(),
            client_request_id,
            rotation,
        };
        session.messages.push(user);
        session.messages.push(Message {
            id: Uuid::new_v4().to_string(),
            session_id: session.id.clone(),
            role: MessageRole::Agent,
            content: String::new(),
            created_at: now_rfc3339(),
            streaming: true,
            activity: None,
            steers: Vec::new(),
            attachments: Vec::new(),
            launched: vec![],
            visuals: Vec::new(),
            client_request_id: None,
            rotation: None,
        });
        if let Some(message) = session.messages.last_mut() {
            let mut activity =
                crate::activity::TurnActivity::new(session.agent.clone(), session.model.clone());
            // Invoked skills open the turn's timeline, ahead of any tool work.
            for name in skill_input.iter().flat_map(|(_, _, names)| names) {
                activity.observe(crate::activity::skill(name));
            }
            message.activity = Some(activity);
            if request.team {
                session.team = Some(crate::team::Team::planning(message.id.clone()));
            }
        }
        session.status = SessionStatus::Starting;
        session.last_error = None;
        session.usage_limit = None;
        // Writing again lets the app's own follow-ups (PR watch, CI auto-fix) run (ADR-101).
        session.stopped_by_user_at = None;
        // Any prompt but a crash continuation restarts that chain's count (ADR-118).
        if !crate::crash_resume::automatic(&session.id) {
            session.auto_continuations = 0;
        }
        session.last_activity_at = now_rfc3339();
        if session.title == "New session" {
            session.title = prompt.chars().take(42).collect();
        }
        let snapshot = session.clone();
        let provider = snapshot.agent.clone();
        let model = process_model;

        let overrides = data.settings.provider_paths.clone();
        (
            snapshot,
            display_path(&cwd),
            provider,
            model,
            overrides,
            process_prompt,
            attachments,
        )
    };
    if let Err(error) = state.persist() {
        let mut data = state.data.lock();
        if let Some(session) = find_session_mut(&mut data, &session_snapshot.id)
            .ok()
            .filter(|session| {
                same_run(session, &session_snapshot) && session.status == SessionStatus::Starting
            })
        {
            session.status = SessionStatus::Failed;
            crate::activity::sync(session);
            session.last_error = Some(error.to_string());
            if let Some(message) = session.messages.last_mut() {
                message.streaming = false;
            }
            crate::transcript_view::emit(&app, session);
        }
        return Err(error);
    }

    {
        let data = state.data.lock();
        let current = data.sessions.iter().find(|session| {
            session.id == session_snapshot.id
                && same_run(session, &session_snapshot)
                && session.status == SessionStatus::Starting
        });
        if let Some(session) = current {
            crate::transcript_view::emit(&app, session);
        } else {
            return Err(Error::agent(
                "execution was cancelled before startup completed",
            ));
        }
    }
    // A new worktree's Setup script (and any script still running) goes first;
    // its failure is shown but never blocks the turn (ADR-078).
    crate::project_scripts::before_turn(&app, state.inner(), &session_snapshot.id).await;
    // Armed while the turn is still Starting: capture, spawn and the provider handshake.
    watch_start(app.clone(), state.inner().clone(), session_snapshot.clone());
    let review_root = PathBuf::from(&cwd);
    let review_session = session_snapshot.clone();
    let capture = native_task(move || {
        Ok(crate::turn_review::Capture::begin(
            review_root,
            &review_session,
        ))
    })
    .await;
    // Every post-admission error travels through the startup failure settlement below.
    let admission = (|| -> Result<()> {
        let data = state.data.lock();
        state.ensure_running()?;
        let session = data
            .sessions
            .iter()
            .find(|session| {
                session.id == session_snapshot.id
                    && same_run(session, &session_snapshot)
                    && session.status == SessionStatus::Starting
            })
            .ok_or_else(|| Error::agent("execution was cancelled during workspace capture"))?;
        if display_path(&session_cwd(&data, session)?) != cwd {
            return Err(Error::agent("workspace changed during turn admission"));
        }
        Ok(())
    })();
    let (review, scope) = match capture {
        Ok(review) => (
            Some(review),
            admission.and_then(|()| {
                crate::provider_accounts::scope(
                    &state,
                    &provider,
                    &session_snapshot.provider_account_id,
                )
            }),
        ),
        Err(error) => (None, Err(error)),
    };
    let process = if let Err(error) = scope {
        Err(error)
    } else if provider == AgentProviderId::Codex {
        crate::codex::start(
            session_snapshot.clone(),
            cwd,
            process_prompt,
            overrides,
            scope.unwrap().home,
        )
        .await
    } else if provider == AgentProviderId::Claude {
        let choices = crate::claude::LaunchChoices::of(&state.data.lock().settings);
        crate::claude::start(
            session_snapshot.clone(),
            cwd,
            process_prompt,
            overrides,
            scope.unwrap().home,
            choices,
        )
        .await
    } else if provider == AgentProviderId::OpenCode {
        crate::opencode::start(session_snapshot.clone(), cwd, process_prompt, overrides).await
    } else if crate::acp_cli::handles(&provider) {
        crate::acp_cli::start(session_snapshot.clone(), cwd, process_prompt, overrides).await
    } else {
        agent::start(
            provider,
            cwd,
            process_prompt,
            model,
            overrides,
            request.execution,
        )
        .await
    };

    match process {
        Ok((process, mut started)) => {
            started.review = review;
            if let Some(run) = &mut started.codex {
                run.attachments = attachments;
            }
            let admitted = {
                let mut data = state.data.lock();
                if let Ok(session) = find_session_mut(&mut data, &session_snapshot.id) {
                    if !state.closing.load(Ordering::Acquire)
                        && session.status == SessionStatus::Starting
                        && same_run(session, &session_snapshot)
                    {
                        state
                            .agents
                            .lock()
                            .insert(session_snapshot.id.clone(), process);
                        session.status = SessionStatus::Running;
                        crate::activity::sync(session);
                        crate::transcript_view::emit(&app, session);
                        None
                    } else {
                        Some(process)
                    }
                } else {
                    Some(process)
                }
            };
            if let Some(process) = admitted {
                process.stop()?;
                started.reap().await;
                return Err(Error::agent(
                    "execution was cancelled before startup completed",
                ));
            }
            started.monitor(
                app.clone(),
                state.inner().clone(),
                session_snapshot.id.clone(),
            );
            // A disk failure must not orphan an already-spawned process.
            state.persist()?;
            let data = state.data.lock();
            let session = data
                .sessions
                .iter()
                .find(|session| session.id == session_snapshot.id)
                .unwrap_or(&session_snapshot);
            crate::transcript_view::session_tail(session)
        }
        Err(err) => {
            let mut data = state.data.lock();
            if let Some(session) = find_session_mut(&mut data, &session_snapshot.id)
                .ok()
                .filter(|session| {
                    same_run(session, &session_snapshot)
                        && session.status == SessionStatus::Starting
                })
            {
                session.status = SessionStatus::Failed;
                crate::activity::sync(session);
                session.last_error = Some(err.to_string());
                if let Some(last) = session.messages.last_mut() {
                    last.streaming = false;
                    if last.content.is_empty() {
                        last.content = err.to_string();
                    }
                }
            }
            drop(data);
            let persisted = state.persist();
            if let Some(session) = state
                .data
                .lock()
                .sessions
                .iter()
                .find(|s| s.id == session_snapshot.id)
            {
                crate::transcript_view::emit(&app, session);
            }
            persisted?;
            Err(err)
        }
    }
}

/// Typed response to an existing callback. Renderer cannot supply vendor protocol IDs or tool inputs.
pub async fn respond_agent_request(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    request: crate::models::RespondAgentRequest,
) -> Result<()> {
    let (sender, receiver) = {
        let mut data = state.data.lock();
        state.ensure_running()?;
        let session = find_session_mut(&mut data, &request.session_id)?;
        let native = matches!(
            session.agent,
            AgentProviderId::Codex | AgentProviderId::Claude | AgentProviderId::OpenCode
        ) || crate::acp_cli::handles(&session.agent);
        if !native || !session.status.is_active() {
            return Err(Error::agent("No live native request for this session"));
        }
        let index = session
            .pending_requests
            .iter()
            .position(|pending| {
                pending.request_id == request.request_id
                    && pending.generation == request.generation
                    && pending.turn_id == request.turn_id
            })
            .ok_or_else(|| Error::agent("Request is stale, foreign or already answered"))?;
        if session.agent == AgentProviderId::Claude {
            crate::claude::validate_response(&session.pending_requests[index], &request.response)?;
        } else if session.agent == AgentProviderId::OpenCode {
            crate::opencode::validate_response(
                &session.pending_requests[index],
                &request.response,
            )?;
        } else if crate::acp_cli::handles(&session.agent) {
            crate::acp_cli::validate_response(&session.pending_requests[index], &request.response)?;
        } else {
            crate::codex::answer(&session.pending_requests[index], &request.response)?;
        }
        let agents = state.agents.lock();
        let process = agents
            .get(&request.session_id)
            .filter(|process| process.generation.as_deref() == Some(&request.generation))
            .ok_or_else(|| Error::agent("Native process generation no longer exists"))?;
        let sender = process
            .replies
            .clone()
            .ok_or_else(|| Error::agent("Provider does not accept interactive responses"))?;
        let (result, receiver) = tokio::sync::oneshot::channel();
        sender
            .try_send(crate::codex::Inbound::Answer(crate::codex::Reply {
                request: request.clone(),
                result,
            }))
            .map_err(|_| Error::agent("Native response queue is unavailable"))?;
        // Reserve under the same data lock: duplicate IPC cannot enqueue another authorization.
        session.pending_requests.remove(index);
        crate::transcript_view::emit(&app, session);
        (sender, receiver)
    };
    drop(sender);
    tokio::time::timeout(std::time::Duration::from_secs(10), receiver)
        .await
        .map_err(|_| Error::agent("Native response timed out; stop this turn before retrying"))?
        .map_err(|_| Error::agent("Native process ended before accepting response"))?
}

/// Longest instruction steered into a running reply.
const STEER_LIMIT: usize = 64 * 1024;

/// Sends an instruction into the session's running Codex or Claude reply (ADR-062).
/// Text and, optionally, screenshots and files from the composer's draft (`attachment_ids`
/// of `attachment_owner`, resolved as `send_prompt` does). The running turn keeps its model
/// and approvals. Once the provider accepts it, the instruction is recorded on the reply at
/// its current length, with its attachments, and the attachments count as sent.
pub async fn steer_turn(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    text: String,
    attachment_ids: Option<Vec<String>>,
    attachment_owner: Option<String>,
) -> Result<()> {
    let attachment_ids = attachment_ids.unwrap_or_default();
    let text = text.trim().to_owned();
    if text.is_empty() || text.len() > STEER_LIMIT || text.contains('\0') {
        return Err(Error::new(
            "invalid",
            "Instructions must have 1 to 65,536 bytes of text.",
        ));
    }
    // An instruction is text for the model; `/compact` waits for the reply to end (ADR-057).
    if crate::codex::is_compact(&text) {
        return Err(Error::agent(
            "Wait for the reply to finish, then send /compact.",
        ));
    }
    let (receiver, generation, summaries) = {
        let data = state.data.lock();
        state.ensure_running()?;
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        if !matches!(
            session.agent,
            AgentProviderId::Codex | AgentProviderId::Claude
        ) || !session.status.is_active()
        {
            return Err(Error::agent("This reply cannot take instructions now."));
        }
        let owner = attachment_owner.unwrap_or_else(|| format!("session:{}", session.id));
        let attachments = state
            .attachments
            .resolve(&owner, &attachment_ids, session)?;
        let summaries: Vec<_> = attachments
            .iter()
            .map(|file| file.summary(&state.data_path))
            .collect();
        let text = crate::attachments::file_prompt(&text, &attachments);
        if text.len() > STEER_LIMIT + 16 * 1024 {
            return Err(Error::new(
                "invalid",
                "The instruction and its files are too long.",
            ));
        }
        // Compacting takes no instructions; the composer queues the message instead.
        let request = session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::User);
        if request.is_some_and(|message| crate::codex::is_compact(&message.content)) {
            return Err(Error::agent(
                "The conversation is being compacted. Send the message after it finishes.",
            ));
        }
        let agents = state.agents.lock();
        let process = agents
            .get(&session_id)
            .ok_or_else(|| Error::agent("This reply cannot take instructions now."))?;
        let sender = process
            .replies
            .clone()
            .ok_or_else(|| Error::agent("This reply cannot take instructions now."))?;
        // Taken once: a second steer, or a send, of the same attachments is refused.
        if !state.attachments.claim(&attachment_ids) {
            return Err(Error::new(
                "attachment",
                "These attachments were already sent.",
            ));
        }
        let (result, receiver) = tokio::sync::oneshot::channel();
        if sender
            .try_send(crate::codex::Inbound::Steer {
                text,
                attachments,
                result,
            })
            .is_err()
        {
            state.attachments.unclaim(&attachment_ids);
            return Err(Error::agent("This reply cannot take instructions now."));
        }
        (receiver, process.generation.clone(), summaries)
    };
    let delivered = tokio::time::timeout(std::time::Duration::from_secs(15), receiver)
        .await
        .map_err(|_| Error::agent("The provider did not take the instruction in time."))
        .and_then(|received| {
            received.map_err(|_| Error::agent("The reply ended before taking the instruction."))
        })
        .and_then(|result| result);
    if let Err(error) = delivered {
        state.attachments.unclaim(&attachment_ids);
        return Err(error);
    }
    let mut data = state.data_with_messages(&[&session_id]);
    let still_running = state
        .agents
        .lock()
        .get(&session_id)
        .is_some_and(|process| process.generation == generation);
    if let Ok(session) = find_session_mut(&mut data, &session_id) {
        if let Some(message) = session
            .messages
            .iter_mut()
            .rev()
            .find(|message| message.role == MessageRole::Agent)
            .filter(|message| still_running && message.streaming)
        {
            let offset = message.content.encode_utf16().count();
            message.steers.push(crate::models::Steer {
                text,
                at: now_rfc3339(),
                offset,
                attachments: summaries,
            });
        }
        crate::transcript_view::emit(&app, session);
    }
    crate::persist::save(&state.data_path, &data)
}

/// Queues a message for one running subagent of the session's Claude turn (ADR-113); a
/// PreToolUse hook hands it over before the subagent's next tool call.
pub async fn steer_subagent(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    task_id: String,
    text: String,
) -> Result<()> {
    let text = text.trim().to_string();
    if task_id.is_empty() || task_id.len() > 192 || task_id.chars().any(char::is_control) {
        return Err(Error::new("invalid", "Unknown subagent."));
    }
    if text.is_empty() || text.chars().count() > 4000 {
        return Err(Error::new(
            "invalid",
            "Write a message of up to 4,000 characters.",
        ));
    }
    let receiver = {
        let data = state.data.lock();
        state.ensure_running()?;
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        if session.agent != AgentProviderId::Claude || !session.status.is_active() {
            return Err(Error::agent("This subagent is not running."));
        }
        let agents = state.agents.lock();
        let sender = agents
            .get(&session_id)
            .and_then(|process| process.replies.clone())
            .ok_or_else(|| Error::agent("This subagent is not running."))?;
        let (result, receiver) = tokio::sync::oneshot::channel();
        sender
            .try_send(crate::codex::Inbound::SteerSubagent {
                task_id,
                text,
                result,
            })
            .map_err(|_| Error::agent("This subagent is not running."))?;
        receiver
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), receiver)
        .await
        .map_err(|_| Error::agent("Claude did not take the message in time."))?
        .map_err(|_| Error::agent("The reply ended before the message was queued."))?
}

/// Moves one running foreground subagent of the session's Claude turn to the background
/// (ADR-111): the turn continues and the subagent reports when it finishes.
pub async fn background_subagent(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    task_id: String,
    tool_use_id: String,
) -> Result<()> {
    let valid = |value: &str| {
        !value.is_empty() && value.len() <= 192 && !value.chars().any(char::is_control)
    };
    if !valid(&task_id) || !valid(&tool_use_id) {
        return Err(Error::new("invalid", "Unknown subagent."));
    }
    let receiver = {
        let data = state.data.lock();
        state.ensure_running()?;
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        if session.agent != AgentProviderId::Claude || !session.status.is_active() {
            return Err(Error::agent("This subagent is not running."));
        }
        let agents = state.agents.lock();
        let sender = agents
            .get(&session_id)
            .and_then(|process| process.replies.clone())
            .ok_or_else(|| Error::agent("This subagent is not running."))?;
        let (result, receiver) = tokio::sync::oneshot::channel();
        sender
            .try_send(crate::codex::Inbound::BackgroundTask {
                task_id,
                tool_use_id,
                result,
            })
            .map_err(|_| Error::agent("This subagent is not running."))?;
        receiver
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), receiver)
        .await
        .map_err(|_| Error::agent("Claude did not move the subagent in time."))?
        .map_err(|_| Error::agent("The reply ended before moving the subagent."))?
}

/// Stops one background subagent of the session's running Claude turn (ADR-097). The CLI's
/// own notification settles its row; the rest of the turn keeps running.
pub async fn stop_background_task(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    task_id: String,
) -> Result<()> {
    if task_id.is_empty() || task_id.len() > 192 || task_id.chars().any(char::is_control) {
        return Err(Error::new("invalid", "Unknown background task."));
    }
    let receiver = {
        let data = state.data.lock();
        state.ensure_running()?;
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        if session.agent != AgentProviderId::Claude || !session.status.is_active() {
            return Err(Error::agent("This background task is not running."));
        }
        let agents = state.agents.lock();
        let sender = agents
            .get(&session_id)
            .and_then(|process| process.replies.clone())
            .ok_or_else(|| Error::agent("This background task is not running."))?;
        let (result, receiver) = tokio::sync::oneshot::channel();
        sender
            .try_send(crate::codex::Inbound::StopTask { task_id, result })
            .map_err(|_| Error::agent("This background task is not running."))?;
        receiver
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), receiver)
        .await
        .map_err(|_| Error::agent("Claude did not stop the task in time."))?
        .map_err(|_| Error::agent("The reply ended before stopping the task."))?
}

pub async fn stop_agent(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<()> {
    // Sessions this turn launched (delegation, `sirus_*` tools) stop with it, and the
    // ones they launched, while they still run that turn (ADR-101).
    let mut seen = std::collections::HashSet::new();
    let mut queue = vec![session_id];
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) || seen.len() > 64 {
            continue;
        }
        let targets = crate::launch::launched_by_session(&id);
        let mut ids: Vec<&str> = vec![id.as_str()];
        ids.extend(targets.iter().map(String::as_str));
        let launched = {
            let data = state.data_with_messages(&ids);
            crate::launch::launched_running(&data, &id)
        };
        // Tasks it delegated stop too, through the saved link, even after a restart (ADR-114).
        let delegated = crate::delegation::active_children(&state.data.lock(), &id);
        stop_one(&app, state.inner(), &id)?;
        queue.extend(launched);
        queue.extend(delegated);
    }
    Ok(())
}

/// The Stop button's path for one session: interrupts the turn (Claude background subagents
/// with it), cancels its scripts and marks it stopped by the person (ADR-101).
pub(crate) fn stop_one(app: &AppHandle, state: &Arc<AppState>, session_id: &str) -> Result<()> {
    let mut data = state.data_with_messages(&[session_id]);
    let running = if let Some(process) = state.agents.lock().get(session_id) {
        process.interrupt()?;
        true
    } else {
        false
    };
    // A Setup or On finish script the turn waits for stops with it (ADR-078).
    crate::project_scripts::cancel(session_id);
    if let Ok(session) = find_session_mut(&mut data, session_id) {
        if running || session.status.is_active() {
            session.stopped_by_user_at = Some(now_rfc3339());
        }
        session.pending_requests.clear();
        session.status = SessionStatus::Stopped;
        crate::activity::sync(session);
        session.last_activity_at = now_rfc3339();
        for message in &mut session.messages {
            message.streaming = false;
        }
    }
    crate::persist::save(&state.data_path, &data)?;
    if let Some(session) = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)
    {
        crate::transcript_view::emit(app, session);
    }
    Ok(())
}

const MAX_TERMINALS_PER_SESSION: usize = 8;

fn is_valid_terminal_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn owned_terminal<'a>(
    ptys: &'a HashMap<String, pty_term::PtySession>,
    session_id: &str,
    terminal_id: &str,
) -> Result<&'a pty_term::PtySession> {
    ptys.get(terminal_id)
        .filter(|pty| pty.session_id == session_id)
        .ok_or_else(|| Error::not_found("terminal is not running"))
}

pub fn start_terminal(
    app: AppHandle,
    state: State<Arc<AppState>>,
    session_id: String,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> Result<()> {
    if !is_valid_terminal_id(&terminal_id) {
        return Err(Error::new("invalid", "invalid terminal id"));
    }
    let (cwd, source) = {
        let data = state.data.lock();
        state.ensure_running()?;
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        let cwd = session_cwd(&data, session)?;
        let ptys = state.ptys.lock();
        if let Some(existing) = ptys.get(&terminal_id) {
            if existing.session_id != session_id {
                return Err(Error::not_found("terminal is not running"));
            }
            existing.resize(cols, rows)?;
            return Ok(());
        }
        check_terminal_limit(&ptys, &session_id)?;
        (
            cwd,
            crate::infisical::terminal_source_in(&data, &session_id)
                .ok()
                .flatten(),
        )
    };
    // The project's Infisical keys come over the network, so no lock is held meanwhile.
    let secrets = if source.is_some() {
        let fetched = crate::runtime::async_runtime::block_on(async {
            tokio::time::timeout(
                TERMINAL_KEYS_TIMEOUT,
                crate::infisical::terminal_env_pairs(source),
            )
            .await
        });
        match fetched {
            Ok(Ok(pairs)) => pairs,
            Ok(Err(error)) => {
                tracing::warn!(%error, "the Infisical keys could not be loaded for a terminal");
                Vec::new()
            }
            Err(_) => {
                tracing::warn!("the Infisical keys timed out for a terminal");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let mut ptys = state.ptys.lock();
    // Another call may have opened the same terminal while the keys loaded.
    if ptys.contains_key(&terminal_id) {
        return Ok(());
    }
    check_terminal_limit(&ptys, &session_id)?;
    let pty = pty_term::start(
        app,
        session_id,
        terminal_id.clone(),
        display_path(&cwd),
        cols,
        rows,
        secrets,
    )?;
    ptys.insert(terminal_id, pty);
    Ok(())
}

/// Bounded per session: a renderer bug cannot spawn unbounded shells.
fn check_terminal_limit(ptys: &pty_term::PtyMap, session_id: &str) -> Result<()> {
    if ptys
        .values()
        .filter(|pty| pty.session_id == session_id)
        .count()
        >= MAX_TERMINALS_PER_SESSION
    {
        return Err(Error::new("invalid", "session reached the terminal limit"));
    }
    Ok(())
}

/// How long a new terminal waits for the project's Infisical keys.
const TERMINAL_KEYS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn write_terminal(
    state: State<Arc<AppState>>,
    session_id: String,
    terminal_id: String,
    data: String,
) -> Result<()> {
    let writer = owned_terminal(&state.ptys.lock(), &session_id, &terminal_id)?.writer();
    writer.write(&data)
}

pub fn resize_terminal(
    state: State<Arc<AppState>>,
    session_id: String,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> Result<()> {
    let ptys = state.ptys.lock();
    owned_terminal(&ptys, &session_id, &terminal_id)?.resize(cols, rows)
}

pub async fn stop_terminal(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    terminal_id: String,
) -> Result<()> {
    let state = state.inner().clone();
    native_task(move || {
        let terminal = {
            let mut ptys = state.ptys.lock();
            match ptys.get(&terminal_id) {
                Some(pty) if pty.session_id == session_id => ptys.remove(&terminal_id),
                _ => None,
            }
        };
        if let Some(terminal) = terminal {
            terminal.kill()?;
        }
        Ok(())
    })
    .await
}

pub async fn host_info(state: State<'_, Arc<AppState>>) -> Result<HostInfo> {
    // Report the Git binary native operations actually run.
    let git_path = which::which(crate::git::git_binary())
        .ok()
        .map(|path| path.display().to_string());
    let git_version = if let Some(path) = &git_path {
        detect::probe_version(path).await
    } else {
        None
    };
    Ok(HostInfo {
        appearance_support: crate::appearance::support(),
        git_detected: git_path.is_some(),
        git_path,
        git_version,
        shell: crate::detect::default_shell(),
        data_dir: state
            .data_path
            .parent()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| state.data_path.display().to_string()),
        worktree_root: state.worktree_root.display().to_string(),
        capabilities: crate::models::HostCapabilities::current(),
        host_name: machine_host_name(),
    })
}

/// The machine's host name without a trailing `.local`; empty when it cannot be read.
pub(crate) fn machine_host_name() -> String {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer is valid for its whole length; the name is NUL-terminated or
        // truncated at the buffer's end, which `split` below handles.
        let status = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
        if status == 0 {
            let raw = buffer.split(|byte| *byte == 0).next().unwrap_or_default();
            return tidy_host_name(&String::from_utf8_lossy(raw));
        }
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .map(|name| tidy_host_name(&name))
        .unwrap_or_default()
}

fn tidy_host_name(name: &str) -> String {
    let name = name.trim();
    name.strip_suffix(".local").unwrap_or(name).to_string()
}

pub async fn probe_provider(
    state: State<'_, Arc<AppState>>,
    id: AgentProviderId,
    path: Option<String>,
) -> Result<ProbeResult> {
    let overrides = state.data.lock().settings.provider_paths.clone();
    let install = detect::resolve_with_overrides(&id, &overrides);
    let target = path
        .or_else(|| install.as_ref().and_then(|item| item.path.clone()))
        .or_else(|| install.map(|item| item.binary));
    let Some(target) = target else {
        return Ok(ProbeResult {
            ok: false,
            version: None,
            message: "No executable to probe".into(),
        });
    };
    let (ok, version, message) = detect::probe_executable(&target).await;
    Ok(ProbeResult {
        ok,
        version,
        message,
    })
}

fn apply_session_selection(session: &mut Session, agent: AgentProviderId, model: Option<String>) {
    let model = model.filter(|value| !value.trim().is_empty());
    if session.agent == agent && session.model == model {
        return;
    }
    let previous = std::mem::replace(&mut session.agent, agent);
    session.model = model;
    // Another model has another window; the next turn reports it again.
    session.context_usage = None;
    let same_provider = previous == session.agent;
    match &mut session.native_thread {
        // Claude (--model with --resume), Codex (thread/resume with a model) and OpenCode
        // (session/set_config_option) continue their own thread on another model, so a model
        // change within one provider keeps the thread and needs no handoff recap.
        Some(identity) if same_provider => identity.model = session.model.clone(),
        _ => {
            session.native_thread = None;
            // The new provider continues this same conversation, not a blank one.
            crate::transcript::arm_in_session_handoff(session, previous);
            if let Some(origin) = &mut session.fork_origin {
                origin.seeded_native_thread_id = None;
            }
        }
    }
    session.last_activity_at = now_rfc3339();
}

pub fn set_session_model(
    state: State<Arc<AppState>>,
    session_id: String,
    agent: AgentProviderId,
    model: Option<String>,
) -> Result<serde_json::Value> {
    let mut data = state.data.lock();
    let account_id = crate::provider_accounts::selected(&data, &agent);
    let session = find_session_mut(&mut data, &session_id)?;
    if session.status.is_active() {
        return Err(Error::agent(
            "cannot change a running session provider or model",
        ));
    }
    let changed_provider = session.agent != agent;
    let account_id = if changed_provider {
        crate::provider_accounts::bound_account(session, &agent, &account_id)
    } else {
        session.provider_account_id.clone()
    };
    apply_session_selection(session, agent, model);
    if changed_provider {
        session.provider_account_id = account_id;
    }
    // Metadata only: the caller copies the provider, model and account fields.
    let meta = crate::transcript_view::session_meta(session)?;
    drop(data);
    state.persist()?;
    Ok(meta)
}

/// Effort and Fast are checked against the provider's model catalog, which only a model list
/// fills. After a restart or a CLI update nothing may have listed it yet, or the session's model
/// may be new to it: the catalog is listed here instead of refusing the turn.
async fn ensure_model_catalog(state: &Arc<AppState>, session_id: &str) {
    let (agent, model, account, overrides) = {
        let data = state.data.lock();
        let Some(session) = data.sessions.iter().find(|item| item.id == session_id) else {
            return;
        };
        (
            session.agent.clone(),
            session.model.clone(),
            session.provider_account_id.clone(),
            data.settings.provider_paths.clone(),
        )
    };
    let known = state
        .catalogs
        .lock()
        .get(&agent)
        .is_some_and(|(path, list)| {
            path.as_ref() == overrides.get(&agent)
                && list.models.iter().any(|row| {
                    Some(&row.id) == model.as_ref()
                        && row.availability == crate::provider_models::ModelAvailability::Available
                })
        });
    if known {
        return;
    }
    let catalog =
        crate::provider_models::list_for_provider(agent.clone(), &overrides, &account, false).await;
    remember_catalog(state, &agent, overrides.get(&agent).cloned(), catalog);
}

/// Keeps a provider's model catalog, unless a read that failed (a fallback list, or none)
/// would replace one the CLI itself reported for the same executable: losing it would turn
/// away every turn with an effort or Fast choice. Returns the catalog kept.
fn remember_catalog(
    state: &AppState,
    agent: &AgentProviderId,
    path: Option<String>,
    catalog: crate::provider_models::ProviderModelList,
) -> crate::provider_models::ProviderModelList {
    let mut catalogs = state.catalogs.lock();
    if let Some((_, previous)) = catalogs
        .get(agent)
        .filter(|(known, previous)| keeps_previous(known, previous, &path, &catalog))
    {
        return previous.clone();
    }
    catalogs.insert(agent.clone(), (path, catalog.clone()));
    catalog
}

fn keeps_previous(
    known: &Option<String>,
    previous: &crate::provider_models::ProviderModelList,
    path: &Option<String>,
    next: &crate::provider_models::ProviderModelList,
) -> bool {
    let reported = |list: &crate::provider_models::ProviderModelList| {
        list.source == "cli" && !list.models.is_empty()
    };
    known == path && reported(previous) && !reported(next)
}

pub async fn list_provider_models(
    state: State<'_, Arc<AppState>>,
    id: AgentProviderId,
    refresh: Option<bool>,
) -> Result<crate::provider_models::ProviderModelList> {
    let (overrides, account) = {
        let data = state.data.lock();
        (
            data.settings.provider_paths.clone(),
            crate::provider_accounts::selected(&data, &id),
        )
    };
    let catalog = crate::provider_models::list_for_provider(
        id.clone(),
        &overrides,
        &account,
        refresh.unwrap_or(false),
    )
    .await;
    Ok(remember_catalog(
        &state,
        &id,
        overrides.get(&id).cloned(),
        catalog,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_session_shares_only_an_existing_worktree_of_its_own_project() {
        let folder = std::env::temp_dir();
        let path = folder.to_string_lossy().into_owned();
        let data: AppData = serde_json::from_value(serde_json::json!({
            "projects": [], "settings": {},
            "sessions": [
                {"id":"here","title":"A","projectId":"p","agent":"codex","status":"idle","createdAt":"t","lastActivityAt":"t","worktree":{"path":path,"branch":"feature","isolated":true},"lastError":null},
                {"id":"gone","title":"B","projectId":"p","agent":"codex","status":"idle","createdAt":"t","lastActivityAt":"t","worktree":{"path":"/no/such/sirus/worktree","branch":"old","isolated":true},"lastError":null}
            ]
        }))
        .unwrap();
        let tree = shared_worktree(&data, "p", "here").unwrap();
        assert_eq!(
            (tree.path.as_str(), tree.branch.as_str(), tree.isolated),
            (path.as_str(), "feature", true)
        );
        assert!(
            shared_worktree(&data, "other", "here").is_err(),
            "another project's session"
        );
        assert!(shared_worktree(&data, "p", "missing").is_err());
        assert!(
            shared_worktree(&data, "p", "gone").is_err(),
            "a removed folder"
        );
    }

    #[test]
    fn a_failed_catalog_read_keeps_the_one_the_cli_reported() {
        let list = |source: &str, models: usize| {
            crate::provider_models::ProviderModelList {
            provider: AgentProviderId::Claude,
            source: source.into(),
            note: String::new(),
            models: (0..models)
                .map(|_| serde_json::from_value(serde_json::json!({"id":"opus","displayName":"Opus","availability":"available"})).unwrap())
                .collect(),
        }
        };
        let here = Some("/bin/claude".to_string());
        assert!(keeps_previous(
            &None,
            &list("cli", 2),
            &None,
            &list("adapter-docs", 4)
        ));
        assert!(keeps_previous(
            &None,
            &list("cli", 2),
            &None,
            &list("cli", 0)
        ));
        assert!(!keeps_previous(
            &None,
            &list("cli", 2),
            &None,
            &list("cli", 3)
        ));
        assert!(!keeps_previous(
            &None,
            &list("adapter-docs", 4),
            &None,
            &list("adapter-docs", 4)
        ));
        // Another executable's catalog says nothing about this one.
        assert!(!keeps_previous(
            &None,
            &list("cli", 2),
            &here,
            &list("adapter-docs", 4)
        ));
    }

    #[test]
    fn only_the_same_turn_still_starting_is_stopped_by_the_start_watch() {
        let session = |status: &str, message: &str| -> Session {
            serde_json::from_value(serde_json::json!({
                "id": "s", "title": "T", "projectId": "p", "agent": "codex", "status": status,
                "createdAt": "t", "lastActivityAt": "t", "lastError": null,
                "worktree": {"path": "/x", "branch": "main", "isolated": false},
                "messages": [{"id": message, "sessionId": "s", "role": "agent", "content": "", "createdAt": "t", "streaming": true}]
            }))
            .unwrap()
        };
        let snapshot = session("starting", "turn-1");
        let data = |current: Session| AppData {
            sessions: vec![current],
            ..AppData::default()
        };
        assert!(still_starting(
            &data(session("starting", "turn-1")),
            &snapshot
        ));
        assert!(
            !still_starting(&data(session("running", "turn-1")), &snapshot),
            "it started"
        );
        assert!(
            !still_starting(&data(session("starting", "turn-2")), &snapshot),
            "a later turn"
        );
    }

    #[test]
    fn host_name_drops_the_local_suffix() {
        assert_eq!(
            tidy_host_name("Gabriels-MacBook.local\n"),
            "Gabriels-MacBook"
        );
        assert_eq!(tidy_host_name("vps-1"), "vps-1");
        assert!(!machine_host_name().ends_with(".local"));
    }

    #[test]
    #[cfg(unix)]
    fn workspace_entry_creation_requires_live_session_project_and_destination() {
        use crate::models::WorkspaceEntryKind;
        let root = tempfile::tempdir().unwrap();
        let foreign = tempfile::tempdir().unwrap();
        let data: AppData = serde_json::from_value(serde_json::json!({
            "projects": [{"id":"p","name":"Fixture","path":display_path(root.path()),"addedAt":"time","lastOpenedAt":"time"}],
            "settings": {},
            "sessions": [{"id":"s","projectId":"p","title":"Task","agent":"codex","status":"idle","createdAt":"time","lastActivityAt":"time",
                "worktree":{"path":display_path(root.path()),"branch":"fixture","isolated":false},"messages":[],"lastError":null}]
        })).unwrap();
        let state = AppState {
            data_path: root.path().join("state.json"),
            worktree_root: root.path().join("trees"),
            data: Mutex::new(data),
            agents: Mutex::new(HashMap::new()),
            ptys: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            attachment_picker: AtomicBool::new(false),
            attachments: Default::default(),
            usage: Default::default(),
            accounts: Default::default(),
            close_guard: Default::default(),
            draft_checkpoint: Mutex::new(None),
            checkpoint_pending: Default::default(),
            catalogs: Mutex::new(HashMap::new()),
        };
        assert!(create_workspace_entry_native(
            &state,
            "unknown",
            WorkspaceEntryKind::File,
            "nope",
            None
        )
        .is_err());
        assert!(create_workspace_entry_native(
            &state,
            "s",
            WorkspaceEntryKind::File,
            "nope",
            Some(&display_path(foreign.path()))
        )
        .is_err());
        assert!(!foreign.path().join("nope").exists());
        let created =
            create_workspace_entry_native(&state, "s", WorkspaceEntryKind::File, "owned", None)
                .unwrap();
        assert_eq!(std::fs::read(created.path).unwrap(), b"");
        state.closing.store(true, Ordering::Release);
        assert!(
            create_workspace_entry_native(&state, "s", WorkspaceEntryKind::File, "nope", None)
                .is_err()
        );
        state.closing.store(false, Ordering::Release);
        state.data.lock().projects.clear();
        assert!(
            create_workspace_entry_native(&state, "s", WorkspaceEntryKind::File, "nope", None)
                .is_err()
        );
        state.data.lock().sessions.clear();
        assert!(
            create_workspace_entry_native(&state, "s", WorkspaceEntryKind::File, "nope", None)
                .is_err()
        );
        assert!(!root.path().join("nope").exists());
    }

    fn fork_fixture(repo: &crate::git::tests::Repo) -> AppState {
        let tree = worktree::create_isolated(
            &repo.cwd(),
            &repo.0.join("source-trees"),
            "aaaaaaaa-source",
            "Source",
            "sirus/{session-name}",
        )
        .unwrap();
        let data: AppData = serde_json::from_value(serde_json::json!({
            "projects":[{"id":"p","name":"Fixture","path":display_path(&repo.cwd()),"addedAt":"time","lastOpenedAt":"time"}], "settings":{},
            "sessions":[{"id":"s","projectId":"p","title":"Task","agent":"codex","providerAccountId":"bound-profile","status":"completed","createdAt":"time","lastActivityAt":"time","worktree":tree,
                "nativeThread":{"threadId":"exact","sessionId":"s","projectId":"p","cwd":tree.path,"model":null},
                "messages":[{"id":"answer","sessionId":"s","role":"agent","content":"Context","createdAt":"time","streaming":false}]}]
        })).unwrap();
        AppState {
            data_path: repo.0.join("state.json"),
            worktree_root: repo.0.join("fork-trees"),
            data: Mutex::new(data),
            agents: Mutex::new(HashMap::new()),
            ptys: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            attachment_picker: AtomicBool::new(false),
            attachments: Default::default(),
            usage: Default::default(),
            accounts: Default::default(),
            close_guard: Default::default(),
            draft_checkpoint: Mutex::new(None),
            checkpoint_pending: Default::default(),
            catalogs: Mutex::new(HashMap::new()),
        }
    }

    #[test]
    fn workspace_file_owner_uses_project_or_session_tree_and_rejects_foreign_ids() {
        let project = tempfile::tempdir().unwrap();
        let isolated = tempfile::tempdir().unwrap();
        let mut data: AppData = serde_json::from_value(serde_json::json!({
            "projects": [{"id":"p","name":"Fixture","path":display_path(project.path()),"addedAt":"time","lastOpenedAt":"time"}],
            "settings": {},
            "sessions": [{"id":"s","projectId":"p","title":"Task","agent":"codex","status":"idle","createdAt":"time","lastActivityAt":"time",
                "worktree":{"path":display_path(isolated.path()),"branch":"fixture","isolated":true},"messages":[],"lastError":null}]
        })).unwrap();
        let owner = |project: Option<&str>, session: Option<&str>| crate::skills::Owner {
            project_id: project.map(String::from),
            session_id: session.map(String::from),
        };
        assert_eq!(
            workspace_files_root(&data, &owner(Some("p"), None)).unwrap(),
            project.path().canonicalize().unwrap()
        );
        assert_eq!(
            workspace_files_root(&data, &owner(None, Some("s"))).unwrap(),
            isolated.path().canonicalize().unwrap()
        );
        assert!(workspace_files_root(&data, &owner(Some("foreign"), Some("s"))).is_err());
        assert!(workspace_files_root(&data, &owner(Some("missing"), None)).is_err());
        assert!(workspace_files_root(&data, &owner(None, Some("missing"))).is_err());
        assert!(workspace_files_root(&data, &owner(None, None)).is_err());
        data.sessions[0].worktree.isolated = false;
        assert!(workspace_files_root(&data, &owner(None, Some("s"))).is_err());
        data.projects.clear();
        assert!(workspace_files_root(&data, &owner(None, Some("s"))).is_err());
    }

    #[test]
    fn fork_uses_source_head_in_owned_tree_and_persists_origin_without_copying_dirty_files() {
        let repo = crate::git::tests::Repo::new();
        let state = fork_fixture(&repo);
        let source = state.data.lock().sessions[0].clone();
        let cwd = PathBuf::from(&source.worktree.path);
        std::fs::write(cwd.join("file.txt"), "source commit\n").unwrap();
        git::run_ok(&cwd, &["add", "--", "file.txt"]).unwrap();
        git::run_ok(&cwd, &["commit", "-qm", "source changes"]).unwrap();
        std::fs::write(cwd.join("file.txt"), "uncommitted\n").unwrap();
        let fork = fork_session_native(&state, "s", "answer").unwrap();
        assert!(fork.worktree.isolated);
        assert_ne!(fork.worktree.path, source.worktree.path);
        assert_eq!(
            std::fs::read_to_string(PathBuf::from(&fork.worktree.path).join("file.txt")).unwrap(),
            "source commit\n"
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("file.txt")).unwrap(),
            "uncommitted\n"
        );
        assert_eq!(fork.provider_account_id, "bound-profile");
        let loaded = persist::load_or_create(&state.data_path).unwrap();
        assert_eq!(loaded.sessions.len(), 2);
        assert_eq!(
            loaded.sessions[0]
                .fork_origin
                .as_ref()
                .unwrap()
                .source_session_id,
            "s"
        );
        assert!(loaded.sessions[0].native_thread.is_none());
        assert_eq!(
            loaded.sessions[1].native_thread.as_ref().unwrap().thread_id,
            "exact"
        );
        let pins = set_message_pinned_native(&state, "s", "answer", true).unwrap();
        assert_eq!(pins, vec!["answer"]);
        assert_eq!(
            persist::load_or_create(&state.data_path).unwrap().sessions[1].pinned_message_ids,
            vec!["answer"]
        );
        assert!(set_message_pinned_native(&state, &fork.id, "answer", true).is_err());
    }

    #[test]
    fn context_flush_and_shutdown_keep_latest_admitted_text_and_refuse_closing() {
        let repo = crate::git::tests::Repo::new();
        let state = fork_fixture(&repo);
        save_context_text_native(&state, "session:s", "first".into(), false).unwrap();
        save_context_text_native(&state, "session:s", "blur flush".into(), true).unwrap();
        assert_eq!(
            persist::load_or_create(&state.data_path)
                .unwrap()
                .context_texts["session:s"],
            "blur flush"
        );
        save_context_text_native(&state, "session:s", "latest admitted".into(), false).unwrap();
        state.shutdown().unwrap();
        assert_eq!(
            persist::load_or_create(&state.data_path)
                .unwrap()
                .context_texts["session:s"],
            "latest admitted"
        );
        assert!(save_context_text_native(&state, "session:s", "after close".into(), true).is_err());
        assert_eq!(
            state.data.lock().context_texts["session:s"],
            "latest admitted"
        );
    }

    #[test]
    fn pr_context_refuses_foreign_removed_changed_and_closing_sessions() {
        let repo = crate::git::tests::Repo::new();
        let state = fork_fixture(&repo);
        assert!(pull_request_context(&state, "foreign").is_err());
        let expected = pull_request_context(&state, "s").unwrap();
        validate_pull_request_context(&state, "s", &expected).unwrap();
        git::run_ok(&expected.cwd, &["checkout", "-b", "changed-pr-branch"]).unwrap();
        assert!(validate_pull_request_context(&state, "s", &expected).is_err());
        state.closing.store(true, Ordering::Release);
        assert!(pull_request_context(&state, "s").is_err());
        state.closing.store(false, Ordering::Release);
        state.data.lock().sessions.clear();
        assert!(validate_pull_request_context(&state, "s", &expected).is_err());
    }

    #[test]
    fn failed_appearance_save_preserves_latest_successful_settings() {
        let repo = crate::git::tests::Repo::new();
        let state = fork_fixture(&repo);
        let mut first = state.data.lock().settings.clone();
        first.theme = crate::models::ThemePref::Light;
        let first = save_settings_native(&state, first).unwrap();
        let original_path = state.data_path.clone();
        let mut state = state;
        state.data_path = repo.0.join("blocked-state.json");
        std::fs::create_dir(&state.data_path).unwrap();
        let mut rejected = first.clone();
        rejected.theme = crate::models::ThemePref::Translucent;
        rejected.dock_icon = crate::models::DockIcon::SmokedGlass;
        assert!(save_settings_native(&state, rejected).is_err());
        assert_eq!(
            serde_json::to_value(&state.data.lock().settings).unwrap(),
            serde_json::to_value(&first).unwrap()
        );
        assert_eq!(
            serde_json::to_value(persist::load_or_create(&original_path).unwrap().settings)
                .unwrap(),
            serde_json::to_value(first).unwrap()
        );
    }

    #[test]
    fn context_persistence_failure_retains_admitted_text_for_retry() {
        let repo = crate::git::tests::Repo::new();
        let state = fork_fixture(&repo);
        // A directory at the target file makes the atomic replacement fail.
        std::fs::create_dir_all(&state.data_path).unwrap();
        assert!(
            save_context_text_native(&state, "project:p", "retain for retry".into(), true).is_err()
        );
        assert_eq!(
            state.data.lock().context_texts["project:p"],
            "retain for retry"
        );
        std::fs::remove_dir(&state.data_path).unwrap();
        save_context_text_native(&state, "project:p", "retain for retry".into(), true).unwrap();
        assert_eq!(
            persist::load_or_create(&state.data_path)
                .unwrap()
                .context_texts["project:p"],
            "retain for retry"
        );
    }

    #[test]
    fn fork_rejects_live_handles_and_invalid_boundaries_before_creating_worktrees() {
        let repo = crate::git::tests::Repo::new();
        let state = fork_fixture(&repo);
        assert!(fork_session_native(&state, "s", "foreign").is_err());
        assert!(!state.worktree_root.exists());
        let (cancel, _) = tokio::sync::watch::channel(false);
        state.agents.lock().insert(
            "s".into(),
            AgentProcess {
                cancel,
                replies: None,
                generation: None,
                #[cfg(unix)]
                pid: None,
            },
        );
        assert!(fork_session_native(&state, "s", "answer").is_err());
        assert!(!state.worktree_root.exists());
    }

    #[test]
    fn selecting_within_a_provider_preserves_the_native_thread() {
        let mut session: Session = serde_json::from_value(serde_json::json!({
            "id":"s", "projectId":"p", "title":"Task", "agent":"codex", "model":"model",
            "status":"completed", "createdAt":"time", "lastActivityAt":"time",
            "worktree":{"path":"/fixture", "branch":"main", "isolated":false}, "messages":[],
            "nativeThread":{"threadId":"exact", "sessionId":"s", "projectId":"p", "cwd":"/fixture", "model":"model"}
        })).unwrap();
        apply_session_selection(&mut session, AgentProviderId::Codex, Some("model".into()));
        assert_eq!(session.native_thread.as_ref().unwrap().thread_id, "exact");
        assert_eq!(session.last_activity_at, "time");
        let bound = session.clone();
        apply_session_selection(
            &mut session,
            AgentProviderId::Codex,
            Some("different".into()),
        );
        // Same provider, another model: the native thread continues on it, without a handoff.
        let thread = session.native_thread.as_ref().unwrap();
        assert_eq!(thread.thread_id, "exact");
        assert_eq!(thread.model.as_deref(), Some("different"));
        assert!(session.handoff.is_none());
        let mut session = bound;
        apply_session_selection(&mut session, AgentProviderId::Claude, None);
        assert!(session.native_thread.is_none());
        assert!(session.model.is_none());
    }

    #[test]
    fn shutdown_persists_interruption_and_refuses_new_processes() {
        let temp = crate::git::tests::Repo::new();
        let data_path = temp.0.join("state.json");
        let mut data: AppData = serde_json::from_value(serde_json::json!({
            "projects": [], "settings": {},
            "sessions": [{"id":"one","title":"Task","projectId":"project","agent":"codex","status":"running","createdAt":"time","lastActivityAt":"time","worktree":{"path":"/unused","branch":"main","isolated":false},"lastError":null,"messages":[{"id":"message","sessionId":"one","role":"agent","content":"partial output","createdAt":"time","streaming":true},{"id":"diagnostic","sessionId":"one","role":"system","content":"warning","createdAt":"time","streaming":true}]}]
        })).unwrap();
        let mut failed = data.sessions[0].clone();
        failed.id = "failed".into();
        failed.status = SessionStatus::Failed;
        failed.last_error = Some("Output limit".into());
        data.sessions.push(failed);
        crate::drafts::apply(&mut data, "session:one", "Latest unsent draft".into()).unwrap();
        let state = AppState {
            data_path: data_path.clone(),
            worktree_root: temp.0.join("trees"),
            data: Mutex::new(data),
            agents: Mutex::new(HashMap::new()),
            ptys: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            attachment_picker: AtomicBool::new(false),
            attachments: Default::default(),
            usage: crate::provider_usage::UsageState::default(),
            accounts: crate::provider_accounts::AccountState::default(),
            close_guard: Mutex::new(crate::close::CloseGuard::default()),
            draft_checkpoint: Mutex::new(None),
            checkpoint_pending: Default::default(),
            catalogs: Mutex::new(HashMap::new()),
        };
        assert!(state.ensure_running().is_ok());
        state.shutdown().unwrap();
        assert!(state.ensure_running().is_err());
        let first_shutdown = std::fs::read(&data_path).unwrap();
        let saved = persist::load_or_create(&data_path).unwrap();
        assert_eq!(saved.composer_drafts["session:one"], "Latest unsent draft");
        assert_eq!(saved.sessions[0].status, SessionStatus::Stopped);
        assert!(saved.sessions[0]
            .last_error
            .as_deref()
            .unwrap()
            .contains("interrupted"));
        assert_eq!(saved.sessions[0].messages[0].content, "partial output");
        assert!(saved.sessions[0]
            .messages
            .iter()
            .all(|message| !message.streaming));
        assert_eq!(saved.sessions[1].status, SessionStatus::Failed);
        assert_eq!(
            saved.sessions[1].last_error.as_deref(),
            Some("Output limit")
        );
        assert!(saved.sessions[1]
            .messages
            .iter()
            .all(|message| !message.streaming));
        state.shutdown().unwrap();
        assert_eq!(std::fs::read(&data_path).unwrap(), first_shutdown);
    }

    /// `send_prompt`'s idempotency steps in order: the id is validated, then, under the
    /// one lock that also records the turn, a known id replays instead of admitting.
    fn admit_once(
        session: &Mutex<Session>,
        raw: Option<&str>,
    ) -> Result<Option<serde_json::Value>> {
        let id = crate::execution::client_request_id(raw)?;
        let mut session = session.lock();
        if let Some(tail) = replayed_send(&session, id.as_deref())? {
            return Ok(Some(tail));
        }
        let turn = session.messages.len();
        let message = |role: MessageRole, client_request_id: Option<String>| Message {
            id: format!("m{turn}-{role:?}"),
            session_id: "s".into(),
            role,
            content: "Fix the build".into(),
            created_at: "t".into(),
            streaming: false,
            activity: None,
            steers: Vec::new(),
            attachments: Vec::new(),
            launched: vec![],
            visuals: Vec::new(),
            client_request_id,
            rotation: None,
        };
        session.messages.push(message(MessageRole::User, id));
        session.messages.push(message(MessageRole::Agent, None));
        Ok(None)
    }

    fn idempotency_session() -> Mutex<Session> {
        Mutex::new(
            serde_json::from_value(serde_json::json!({
                "id":"s", "projectId":"p", "title":"Task", "agent":"codex", "model":null,
                "status":"completed", "createdAt":"t", "lastActivityAt":"t",
                "worktree":{"path":"/fixture", "branch":"main", "isolated":false}, "lastError":null,
                "messages":[]
            }))
            .unwrap(),
        )
    }

    fn turns(session: &Mutex<Session>) -> usize {
        session
            .lock()
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::User)
            .count()
    }

    #[test]
    fn a_send_delivered_twice_starts_one_turn_and_replays_the_session() {
        let session = std::sync::Arc::new(idempotency_session());
        let id = "6F1C2B4A-8D3E-4F5A-9B7C-0D1E2F3A4B5C";
        let threads = (0..4)
            .map(|_| {
                let session = session.clone();
                std::thread::spawn(move || admit_once(&session, Some(id)).unwrap())
            })
            .collect::<Vec<_>>();
        let replays = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(Option::is_some)
            .collect::<Vec<_>>();
        assert_eq!(turns(&session), 1);
        assert_eq!(replays.len(), 3);
        // The replay is the session tail, as the original send returned it.
        let tail = replays[0].as_ref().unwrap();
        assert_eq!(tail["id"], "s");
        assert!(tail["messages"].is_array());
        // Stored lowercase and kept through a save: a retry after a restart still matches.
        let stored = serde_json::to_value(&session.lock().messages[0]).unwrap();
        assert_eq!(stored["clientRequestId"], id.to_lowercase());
        let reread: Message = serde_json::from_value(stored).unwrap();
        assert!(crate::execution::already_sent(
            &Session {
                messages: vec![reread].into(),
                ..session.lock().clone()
            },
            &id.to_lowercase()
        ));
        assert!(admit_once(&session, Some(&id.to_lowercase()))
            .unwrap()
            .is_some());
        assert_eq!(turns(&session), 1);
    }

    #[test]
    fn different_send_ids_and_sends_without_one_are_each_admitted() {
        let session = idempotency_session();
        for id in [
            Some("0b5e9a3c-1d2f-4e6a-8b7c-9d0e1f2a3b4c"),
            Some("1c6f0b4d-2e3a-4f7b-9c8d-0e1f2a3b4c5d"),
            None,
            None,
        ] {
            assert!(admit_once(&session, id).unwrap().is_none());
        }
        assert_eq!(turns(&session), 4);
    }

    #[test]
    fn a_malformed_send_id_is_refused_before_admission() {
        let session = idempotency_session();
        for raw in [
            "",
            "not-a-uuid",
            "0b5e9a3c1d2f4e6a8b7c9d0e1f2a3b4c",
            "{0b5e9a3c-1d2f-4e6a-8b7c-9d0e1f2a3b4c}",
            "urn:uuid:0b5e9a3c-1d2f-4e6a-8b7c-9d0e1f2a3b4c",
            &"a".repeat(4096),
        ] {
            let error = admit_once(&session, Some(raw)).unwrap_err();
            assert!(
                matches!(
                    error,
                    Error::App {
                        code: "invalid",
                        ..
                    }
                ),
                "{raw}"
            );
        }
        assert_eq!(turns(&session), 0);
        let legacy: SendPromptRequest =
            serde_json::from_value(serde_json::json!({"sessionId":"s", "prompt":"Next"})).unwrap();
        assert!(legacy.client_request_id.is_none());
    }
}
