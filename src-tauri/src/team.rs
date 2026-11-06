//! Team orchestration (ADR-043). A coordinator session proposes up to three tasks in a read-only
//! planning turn; nothing runs until the person confirms. Each confirmed task runs as its own
//! session in an isolated worktree with the person's chosen approval profile. "Merge all" is an
//! explicit action that applies finished work to the coordinator checkout in plan order, without
//! commits, and stops at the first conflict without applying it. No model answers approvals,
//! starts work or merges on the person's behalf.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::runtime::{AppHandle, Manager};
use serde::{Deserialize, Serialize};

use crate::commands::{native_task, session_cwd, AppState};
use crate::error::{Error, Result};
use crate::models::{AgentProviderId, AppData, ApprovalMode, Session, SessionStatus};

pub const MAX_TASKS: usize = 3;
const PLAN_OPEN: &str = "<sirus_team_plan>";
const PLAN_CLOSE: &str = "</sirus_team_plan>";
const MAX_PLAN_BYTES: usize = 32 * 1024;
const MAX_TITLE_CHARS: usize = 120;
const MAX_INSTRUCTION_CHARS: usize = 4000;
const MAX_PATHS: usize = 8;
const MAX_PATH_CHARS: usize = 200;
const MAX_CATALOG_MODELS: usize = 8;
const MAX_PATCH_BYTES: usize = 8 * 1024 * 1024;
/// Coordinators need a verified read-only planning turn; workers need native approvals.
pub const TEAM_PROVIDERS: [AgentProviderId; 3] = [
    AgentProviderId::Codex,
    AgentProviderId::Claude,
    AgentProviderId::OpenCode,
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TeamStatus {
    Planning,
    Proposed,
    Running,
    /// Every task settled; finished work can be merged.
    Ready,
    /// Merge finished: every finished task was merged or skipped.
    Done,
    Stopped,
    Failed,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    Pending,
    Running,
    Done,
    Failed,
    Stopped,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MergeState {
    Merged,
    Skipped,
    Conflict,
    OutOfScope,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TeamTask {
    pub id: String,
    pub title: String,
    pub instructions: String,
    pub provider: AgentProviderId,
    pub model: Option<String>,
    pub paths: Vec<String>,
    pub after: Option<String>,
    pub worker_session_id: Option<String>,
    pub state: TaskState,
    pub merge: Option<MergeState>,
    /// Bounded, secret-free explanation for a stopped merge.
    pub note: Option<String>,
    /// Explicit per-task consent to merge changes outside the planned paths.
    #[serde(default)]
    pub allow_outside: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    /// The coordinator response that carries the plan.
    pub message_id: String,
    pub status: TeamStatus,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub approval: Option<ApprovalMode>,
    pub tasks: Vec<TeamTask>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TeamWorker {
    pub coordinator_session_id: String,
    pub task_id: String,
}

impl Team {
    pub fn planning(message_id: String) -> Self {
        Self {
            message_id,
            status: TeamStatus::Planning,
            summary: None,
            error: None,
            approval: None,
            tasks: vec![],
        }
    }
    /// A new plan cannot replace a team that still owns proposed, running or unmerged work.
    pub fn blocks_new_plan(&self) -> bool {
        matches!(
            self.status,
            TeamStatus::Planning | TeamStatus::Proposed | TeamStatus::Running | TeamStatus::Ready
        )
    }
}

/// Models offered to the coordinator: enabled team providers with their cached native catalogs.
pub type Catalog = Vec<(AgentProviderId, Vec<(String, String)>)>;
pub fn catalog(state: &AppState, data: &AppData) -> Catalog {
    let catalogs = state.catalogs.lock();
    TEAM_PROVIDERS
        .iter()
        .filter(|provider| !data.settings.disabled_providers.contains(provider))
        .map(|provider| {
            let models = catalogs
                .get(provider)
                .filter(|(path, _)| path.as_ref() == data.settings.provider_paths.get(provider))
                .map(|(_, list)| {
                    list.models
                        .iter()
                        .take(MAX_CATALOG_MODELS)
                        .map(|model| (model.id.clone(), model.display_name.clone()))
                        .collect()
                })
                .unwrap_or_default();
            (provider.clone(), models)
        })
        .collect()
}

/// Read-only planning instructions wrapped around the person's visible request.
pub fn planning_prompt(request: &str, catalog: &Catalog) -> String {
    let agents = catalog
        .iter()
        .map(|(provider, models)| {
            let models = if models.is_empty() {
                "default".to_owned()
            } else {
                models
                    .iter()
                    .map(|(id, name)| format!("{id} ({name})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!("- {}: {models}", provider.key())
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You are the coordinator of a small team of coding agents in Sirus. Investigate the request and the repository read-only, then propose a plan. Do not edit files.\n\n\
Split the work into 1 to {MAX_TASKS} tasks that different agents can do in parallel, each in its own copy of the repository. Prefer fewer tasks; use 1 task when the work is small. Give each task a short title, complete self-contained instructions, and the files or folders it may change (relative paths: exact files or \"folder/**\"). A task may wait for one earlier task with \"after\". Avoid giving two tasks the same files.\n\n\
Assign each task to one of these agents, using the provider and model ids exactly as listed (use null for the provider's default model):\n{agents}\n\n\
Write one short sentence explaining the split, then end your reply with exactly one block:\n\
{PLAN_OPEN}\n{{\"summary\":\"one sentence for the person\",\"tasks\":[{{\"id\":\"t1\",\"title\":\"...\",\"instructions\":\"...\",\"provider\":\"codex\",\"model\":null,\"paths\":[\"src/feature/**\"],\"after\":null}}]}}\n{PLAN_CLOSE}\n\n\
User request:\n{request}"
    )
}

#[derive(Deserialize)]
struct RawPlan {
    #[serde(default)]
    summary: Option<String>,
    tasks: Vec<RawTask>,
}
#[derive(Deserialize)]
struct RawTask {
    id: String,
    title: String,
    instructions: String,
    provider: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    after: Option<String>,
}

fn printable(value: &str, max: usize, what: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max {
        return Err(Error::agent(format!("Team plan has an invalid {what}.")));
    }
    if value
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(Error::agent(format!("Team plan has an invalid {what}.")));
    }
    Ok(value.to_owned())
}

/// Planned areas are relative exact files or `folder/**`; nothing else is matched.
pub fn valid_scope(path: &str) -> bool {
    let core = path.strip_suffix("/**").unwrap_or(path);
    !core.is_empty()
        && path.chars().count() <= MAX_PATH_CHARS
        && !core.contains('*')
        && !core.starts_with('/')
        && !core.contains('\\')
        && !core.chars().any(char::is_control)
        && core
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
pub fn in_scope(path: &str, scopes: &[String]) -> bool {
    scopes.iter().any(|scope| match scope.strip_suffix("/**") {
        Some(dir) => path.starts_with(dir) && path.as_bytes().get(dir.len()) == Some(&b'/'),
        None => path == scope,
    })
}

fn provider_from(key: &str) -> Option<AgentProviderId> {
    TEAM_PROVIDERS
        .iter()
        .find(|provider| provider.key() == key)
        .cloned()
}

/// Checks one assignment against the offered catalog. Unknown providers/models are refused.
pub fn validate_assignment(
    provider: &AgentProviderId,
    model: Option<&str>,
    catalog: &Catalog,
) -> Result<()> {
    let (_, models) = catalog
        .iter()
        .find(|(offered, _)| offered == provider)
        .ok_or_else(|| Error::agent("Team task uses a provider that is unavailable."))?;
    match model {
        None => Ok(()),
        Some(model) if models.iter().any(|(id, _)| id == model) => Ok(()),
        Some(_) => Err(Error::agent("Team task uses a model that is unavailable.")),
    }
}

/// Extracts and validates the coordinator's plan. Model output is untrusted: sizes, ids,
/// assignments, paths and ordering are all checked; the visible reply excludes the block.
pub fn parse_plan(
    content: &str,
    catalog: &Catalog,
) -> Result<(String, Option<String>, Vec<TeamTask>)> {
    let start = content
        .rfind(PLAN_OPEN)
        .ok_or_else(|| Error::agent("The coordinator did not return a team plan."))?;
    let body_start = start + PLAN_OPEN.len();
    let end = content[body_start..]
        .find(PLAN_CLOSE)
        .map(|offset| body_start + offset)
        .ok_or_else(|| Error::agent("The coordinator did not finish the team plan."))?;
    let body = content[body_start..end].trim();
    if body.len() > MAX_PLAN_BYTES {
        return Err(Error::agent("Team plan is too large."));
    }
    let raw: RawPlan = serde_json::from_str(body)
        .map_err(|_| Error::agent("The coordinator returned an invalid team plan."))?;
    if raw.tasks.is_empty() || raw.tasks.len() > MAX_TASKS {
        return Err(Error::agent("A team plan needs 1 to 3 tasks."));
    }
    let mut seen = HashSet::new();
    let mut tasks = Vec::with_capacity(raw.tasks.len());
    for task in raw.tasks {
        let id = task.id.trim().to_owned();
        if id.is_empty()
            || id.len() > 16
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            || !seen.insert(id.clone())
        {
            return Err(Error::agent("Team plan has an invalid task id."));
        }
        let provider = provider_from(task.provider.trim())
            .ok_or_else(|| Error::agent("Team task uses a provider that is unavailable."))?;
        let model = task
            .model
            .map(|model| model.trim().to_owned())
            .filter(|model| !model.is_empty() && model != "default");
        validate_assignment(&provider, model.as_deref(), catalog)?;
        if task.paths.is_empty() || task.paths.len() > MAX_PATHS {
            return Err(Error::agent("Each team task needs 1 to 8 paths."));
        }
        let paths = task
            .paths
            .iter()
            .map(|path| path.trim().trim_start_matches("./").to_owned())
            .collect::<Vec<_>>();
        if !paths.iter().all(|path| valid_scope(path)) {
            return Err(Error::agent("Team plan has an invalid path."));
        }
        // Dependencies point only to earlier tasks, so plan order is a valid merge order.
        let after = task
            .after
            .map(|after| after.trim().to_owned())
            .filter(|after| !after.is_empty());
        if let Some(after) = &after {
            if after == &id || !tasks.iter().any(|earlier: &TeamTask| &earlier.id == after) {
                return Err(Error::agent(
                    "Team plan has an invalid dependency between tasks.",
                ));
            }
        }
        tasks.push(TeamTask {
            id,
            title: printable(&task.title, MAX_TITLE_CHARS, "title")?,
            instructions: printable(&task.instructions, MAX_INSTRUCTION_CHARS, "instruction")?,
            provider,
            model,
            paths,
            after,
            worker_session_id: None,
            state: TaskState::Pending,
            merge: None,
            note: None,
            allow_outside: false,
        });
    }
    let summary = raw
        .summary
        .and_then(|summary| printable(&summary, 300, "summary").ok());
    let visible = format!(
        "{}\n{}",
        content[..start].trim_end(),
        content[end + PLAN_CLOSE.len()..].trim_start()
    )
    .trim()
    .to_owned();
    Ok((visible, summary, tasks))
}

/// The worker receives only its own task, never the coordinator transcript.
pub fn worker_prompt(task: &TeamTask) -> String {
    format!(
        "You are one member of a Sirus team, working in your own copy of the repository.\n\n\
Task: {}\n\n{}\n\n\
Only change these paths: {}. Do not commit, create or switch branches, push, or change files outside these paths. When you finish, reply with a short summary of what you changed.",
        task.title,
        task.instructions,
        task.paths.join(", ")
    )
}

/* ---------- Lifecycle ---------- */

fn emit(app: &AppHandle, session: &Session) {
    crate::transcript_view::emit(app, session);
}
fn coordinator_mut<'a>(data: &'a mut AppData, id: &str) -> Option<&'a mut Session> {
    data.sessions.iter_mut().find(|session| session.id == id)
}

/// Called after every native turn settles. Planning turns become proposals; worker turns update
/// their task and start dependents whose prerequisite finished. Never starts unconfirmed work.
pub fn settled(app: &AppHandle, state: &Arc<AppState>, session_id: &str) {
    let app = app.clone();
    let state = state.clone();
    let session_id = session_id.to_owned();
    crate::runtime::async_runtime::spawn(async move {
        let starts = {
            let mut data = state.data_with_messages(&[&session_id]);
            let catalog = catalog(&state, &data);
            let mut starts = vec![];
            if let Some(session) = coordinator_mut(&mut data, &session_id) {
                if let Some(team) = session
                    .team
                    .as_mut()
                    .filter(|team| team.status == TeamStatus::Planning)
                {
                    let message = session
                        .messages
                        .iter_mut()
                        .find(|message| message.id == team.message_id);
                    let outcome = message
                        .as_ref()
                        .filter(|_| session.status == SessionStatus::Completed)
                        .ok_or_else(|| Error::agent("The planning turn did not complete."))
                        .and_then(|message| parse_plan(&message.content, &catalog));
                    match outcome {
                        Ok((visible, summary, tasks)) => {
                            if let Some(message) = message {
                                message.content = visible;
                            }
                            team.status = TeamStatus::Proposed;
                            team.summary = summary;
                            team.tasks = tasks;
                        }
                        Err(error) => {
                            team.status = TeamStatus::Failed;
                            team.error = Some(error.to_string());
                        }
                    }
                    emit(&app, session);
                }
            }
            let link = data
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .and_then(|session| Some((session.team_worker.clone()?, session.status.clone())));
            if let Some((link, status)) = link {
                starts = advance(&mut data, &link, &status);
                if let Some(coordinator) = data
                    .sessions
                    .iter()
                    .find(|session| session.id == link.coordinator_session_id)
                {
                    emit(&app, coordinator);
                }
            }
            starts
        };
        let _ = state.persist();
        dispatch(&app, &state, starts).await;
    });
}

/// Records one worker settlement and returns dependent tasks that may start now.
fn advance(
    data: &mut AppData,
    link: &TeamWorker,
    status: &SessionStatus,
) -> Vec<(String, String, String)> {
    let Some(team) = coordinator_mut(data, &link.coordinator_session_id)
        .and_then(|session| session.team.as_mut())
    else {
        return vec![];
    };
    let Some(task) = team.tasks.iter_mut().find(|task| task.id == link.task_id) else {
        return vec![];
    };
    if task.merge == Some(MergeState::Merged) {
        return vec![];
    }
    task.state = match status {
        SessionStatus::Completed => TaskState::Done,
        SessionStatus::Failed => TaskState::Failed,
        SessionStatus::Stopped => TaskState::Stopped,
        _ => return vec![],
    };
    let mut starts = vec![];
    if team.status == TeamStatus::Running {
        for index in 0..team.tasks.len() {
            let ready = team.tasks[index].state == TaskState::Pending
                && team.tasks[index].after.as_ref().is_some_and(|after| {
                    team.tasks
                        .iter()
                        .any(|task| &task.id == after && task.state == TaskState::Done)
                });
            if ready {
                if let Some(worker) = team.tasks[index].worker_session_id.clone() {
                    team.tasks[index].state = TaskState::Running;
                    starts.push((
                        link.coordinator_session_id.clone(),
                        team.tasks[index].id.clone(),
                        worker,
                    ));
                }
            }
        }
        // A failed or stopped prerequisite leaves its dependents unstarted.
        let blocked = |team: &Team, task: &TeamTask| {
            task.after.as_ref().is_some_and(|after| {
                team.tasks.iter().any(|other| {
                    &other.id == after
                        && matches!(other.state, TaskState::Failed | TaskState::Stopped)
                })
            })
        };
        let stuck = team
            .tasks
            .iter()
            .filter(|task| task.state == TaskState::Pending && blocked(team, task))
            .map(|task| task.id.clone())
            .collect::<Vec<_>>();
        for task in &mut team.tasks {
            if stuck.contains(&task.id) {
                task.state = TaskState::Stopped;
                task.note = Some("Its prerequisite did not finish.".into());
            }
        }
        if team
            .tasks
            .iter()
            .all(|task| !matches!(task.state, TaskState::Pending | TaskState::Running))
        {
            team.status = TeamStatus::Ready;
        }
    }
    starts
}

/// Sends each started task to its worker session through the normal admission path.
async fn dispatch(app: &AppHandle, state: &Arc<AppState>, starts: Vec<(String, String, String)>) {
    for (coordinator, task_id, worker) in starts {
        send_task(app, state, coordinator, task_id, worker, None).await;
    }
}

/// One worker turn: the task prompt, or `custom` (conflict resolution), under the team approval.
async fn send_task(
    app: &AppHandle,
    state: &Arc<AppState>,
    coordinator: String,
    task_id: String,
    worker: String,
    custom: Option<String>,
) {
    {
        let (prompt, approval) = {
            let data = state.data.lock();
            let team = data
                .sessions
                .iter()
                .find(|session| session.id == coordinator)
                .and_then(|session| session.team.as_ref());
            match team.and_then(|team| {
                team.tasks
                    .iter()
                    .find(|task| task.id == task_id)
                    .map(|task| {
                        (
                            custom.clone().unwrap_or_else(|| worker_prompt(task)),
                            team.approval,
                        )
                    })
            }) {
                Some(found) => found,
                None => return,
            }
        };
        let request = crate::models::SendPromptRequest {
            queued_after: None,
            debugging: false,
            goal: None,
            session_id: worker.clone(),
            attachment_ids: vec![],
            attachment_owner: String::new(),
            prompt,
            execution: crate::models::ExecutionOptions {
                approval,
                ..Default::default()
            },
            team: false,
            client_request_id: None,
        };
        let result =
            crate::commands::send_prompt(app.clone(), app.state::<Arc<AppState>>(), request).await;
        if let Err(error) = result {
            let mut data = state.data.lock();
            if let Some(session) = coordinator_mut(&mut data, &coordinator) {
                if let Some(task) = session
                    .team
                    .as_mut()
                    .and_then(|team| team.tasks.iter_mut().find(|task| task.id == task_id))
                {
                    task.state = TaskState::Failed;
                    task.note = Some(short(&error.to_string()));
                }
                if let Some(team) = session.team.as_mut() {
                    if team
                        .tasks
                        .iter()
                        .all(|task| !matches!(task.state, TaskState::Pending | TaskState::Running))
                    {
                        team.status = TeamStatus::Ready;
                    }
                }
                emit(app, session);
            }
            drop(data);
            let _ = state.persist();
        }
    }
}

fn short(value: &str) -> String {
    value
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect()
}

/// After a restart nothing keeps running: planning fails, unstarted tasks stop, and settled work
/// remains mergeable. Never restarts processes.
pub fn recover(data: &mut AppData) -> bool {
    let statuses = data
        .sessions
        .iter()
        .map(|session| (session.id.clone(), session.status.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    let mut changed = false;
    for session in &mut data.sessions {
        let Some(team) = session.team.as_mut() else {
            continue;
        };
        match team.status {
            TeamStatus::Planning => {
                team.status = TeamStatus::Failed;
                team.error = Some("Planning was interrupted when Sirus closed.".into());
                changed = true;
            }
            TeamStatus::Running => {
                for task in &mut team.tasks {
                    if matches!(task.state, TaskState::Pending | TaskState::Running) {
                        task.state = match task
                            .worker_session_id
                            .as_ref()
                            .and_then(|id| statuses.get(id))
                        {
                            Some(SessionStatus::Completed) => TaskState::Done,
                            Some(SessionStatus::Failed) => TaskState::Failed,
                            _ => TaskState::Stopped,
                        };
                    }
                }
                team.status = TeamStatus::Stopped;
                changed = true;
            }
            _ => {}
        }
    }
    changed
}

/* ---------- Actions ---------- */

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskEdit {
    pub id: String,
    pub title: String,
    pub provider: AgentProviderId,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum TeamAction {
    /// Confirms the proposal (titles/assignments edited, tasks removed by omission).
    Start {
        session_id: String,
        tasks: Vec<TaskEdit>,
        approval: ApprovalMode,
    },
    Stop {
        session_id: String,
    },
    Merge {
        session_id: String,
    },
    Skip {
        session_id: String,
        task_id: String,
    },
    MergeAnyway {
        session_id: String,
        task_id: String,
    },
    /// Brings the current checkout into a conflicted helper's worktree with a three-way merge;
    /// leftover conflicts go back to that helper to resolve in its own copy.
    Resolve {
        session_id: String,
        task_id: String,
    },
    Discard {
        session_id: String,
    },
    Cleanup {
        session_id: String,
        confirm: bool,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamActionResponse {
    pub sessions: Vec<Session>,
    pub removed: Vec<String>,
}

fn team_mut<'a>(data: &'a mut AppData, session_id: &str) -> Result<&'a mut Team> {
    coordinator_mut(data, session_id)
        .ok_or_else(|| Error::not_found("session not found"))?
        .team
        .as_mut()
        .ok_or_else(|| Error::not_found("this session has no team"))
}
fn snapshot(data: &AppData, session_id: &str) -> Vec<Session> {
    let mut sessions = data
        .sessions
        .iter()
        .filter(|session| {
            session.id == session_id
                || session
                    .team_worker
                    .as_ref()
                    .is_some_and(|link| link.coordinator_session_id == session_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    sessions.sort_by_key(|session| session.id != session_id);
    sessions
}

pub async fn team_action(
    app: AppHandle,
    state: crate::runtime::State<'_, Arc<AppState>>,
    action: TeamAction,
) -> Result<TeamActionResponse> {
    let state = state.inner().clone();
    match action {
        TeamAction::Start {
            session_id,
            tasks,
            approval,
        } => {
            let shared = state.clone();
            let id = session_id.clone();
            let starts = native_task(move || start(&shared, &id, tasks, approval)).await?;
            for session in snapshot(&state.data.lock(), &session_id) {
                emit(&app, &session);
            }
            dispatch(&app, &state, starts).await;
            Ok(TeamActionResponse {
                sessions: snapshot(&state.data.lock(), &session_id),
                removed: vec![],
            })
        }
        TeamAction::Stop { session_id } => {
            let workers = {
                let mut data = state.data.lock();
                let team = team_mut(&mut data, &session_id)?;
                if team.status != TeamStatus::Running {
                    return Err(Error::agent("This team is not running."));
                }
                team.status = TeamStatus::Stopped;
                for task in &mut team.tasks {
                    if task.state == TaskState::Pending {
                        task.state = TaskState::Stopped;
                    }
                }
                team.tasks
                    .iter()
                    .filter(|task| task.state == TaskState::Running)
                    .filter_map(|task| task.worker_session_id.clone())
                    .collect::<Vec<_>>()
            };
            state.persist()?;
            for worker in workers {
                crate::commands::stop_agent(app.clone(), app.state::<Arc<AppState>>(), worker)
                    .await?;
            }
            let sessions = snapshot(&state.data.lock(), &session_id);
            if let Some(coordinator) = sessions.first() {
                emit(&app, coordinator);
            }
            Ok(TeamActionResponse {
                sessions,
                removed: vec![],
            })
        }
        TeamAction::Merge { session_id } => merge_response(&app, &state, session_id, None).await,
        TeamAction::Skip {
            session_id,
            task_id,
        } => merge_response(&app, &state, session_id, Some((task_id, false))).await,
        TeamAction::MergeAnyway {
            session_id,
            task_id,
        } => merge_response(&app, &state, session_id, Some((task_id, true))).await,
        TeamAction::Resolve {
            session_id,
            task_id,
        } => {
            let shared = state.clone();
            let (id, task) = (session_id.clone(), task_id.clone());
            let outcome = native_task(move || prepare_resolve(&shared, &id, &task)).await?;
            let sessions = snapshot(&state.data.lock(), &session_id);
            if let Some(coordinator) = sessions.first() {
                emit(&app, coordinator);
            }
            if let Some((worker, prompt)) = outcome {
                send_task(
                    &app,
                    &state,
                    session_id.clone(),
                    task_id,
                    worker,
                    Some(prompt),
                )
                .await;
            }
            Ok(TeamActionResponse {
                sessions: snapshot(&state.data.lock(), &session_id),
                removed: vec![],
            })
        }
        TeamAction::Discard { session_id } => {
            let sessions = {
                let mut data = state.data.lock();
                let session = coordinator_mut(&mut data, &session_id)
                    .ok_or_else(|| Error::not_found("session not found"))?;
                if !session.team.as_ref().is_some_and(|team| {
                    matches!(team.status, TeamStatus::Proposed | TeamStatus::Failed)
                }) {
                    return Err(Error::agent("Only a proposed plan can be discarded."));
                }
                session.team = None;
                emit(&app, session);
                snapshot(&data, &session_id)
            };
            state.persist()?;
            Ok(TeamActionResponse {
                sessions,
                removed: vec![],
            })
        }
        TeamAction::Cleanup {
            session_id,
            confirm,
        } => {
            if !confirm {
                return Err(Error::confirmation_required(
                    "removing team worktrees deletes those working copies. confirm explicitly.",
                ));
            }
            let workers = {
                let data = state.data.lock();
                let team = data
                    .sessions
                    .iter()
                    .find(|session| session.id == session_id)
                    .and_then(|session| session.team.as_ref())
                    .ok_or_else(|| Error::not_found("this session has no team"))?;
                if !matches!(team.status, TeamStatus::Done | TeamStatus::Stopped) {
                    return Err(Error::agent(
                        "Merge or skip every finished task before removing worktrees.",
                    ));
                }
                team.tasks
                    .iter()
                    .filter_map(|task| {
                        let worker = task.worker_session_id.clone()?;
                        let path = data
                            .sessions
                            .iter()
                            .find(|session| session.id == worker)
                            .filter(|session| session.worktree.isolated)
                            .map(|session| PathBuf::from(&session.worktree.path));
                        Some((worker, path, task.title.clone()))
                    })
                    .collect::<Vec<_>>()
            };
            let mut removed = vec![];
            for (worker, path, title) in workers {
                let shared = state.clone();
                let id = worker.clone();
                native_task(move || {
                    // Merges leave helper edits uncommitted; keep them on the helper's own
                    // branch so the worktree is clean and is removed without force.
                    if let Some(path) = path.filter(|path| path.is_dir()) {
                        preserve_helper_work(&path, &title)?;
                    }
                    crate::commands::delete_session_native(&shared, id, true, true)
                })
                .await?;
                removed.push(worker);
            }
            let sessions = {
                let mut data = state.data.lock();
                if let Some(team) = coordinator_mut(&mut data, &session_id)
                    .and_then(|session| session.team.as_mut())
                {
                    for task in &mut team.tasks {
                        task.worker_session_id = None;
                    }
                }
                snapshot(&data, &session_id)
            };
            state.persist()?;
            if let Some(coordinator) = sessions.first() {
                emit(&app, coordinator);
            }
            Ok(TeamActionResponse { sessions, removed })
        }
    }
}

/// Commits a helper's remaining changes to its own app-owned branch (hooks and signing
/// disabled, fixed Sirus identity). Nothing is discarded: the branch keeps the work
/// after its worktree is removed. Fails if the worktree is still dirty afterwards.
fn preserve_helper_work(path: &Path, title: &str) -> Result<()> {
    if !crate::git::status(path)?.dirty {
        return Ok(());
    }
    let added = crate::git::run(path, &["add", "-A"])?;
    if !added.status.success() {
        return Err(Error::git("Cannot save a helper's changes before cleanup."));
    }
    let identity = [
        ("GIT_AUTHOR_NAME", std::ffi::OsStr::new("Sirus")),
        ("GIT_AUTHOR_EMAIL", std::ffi::OsStr::new("team@sirus.local")),
        ("GIT_COMMITTER_NAME", std::ffi::OsStr::new("Sirus")),
        (
            "GIT_COMMITTER_EMAIL",
            std::ffi::OsStr::new("team@sirus.local"),
        ),
    ];
    let message = format!(
        "Sirus team task: {}",
        title.chars().take(120).collect::<String>()
    );
    let committed = crate::git::run_env(
        path,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--no-verify",
            "-q",
            "-m",
            &message,
        ],
        &identity,
    )?;
    if !committed.status.success() || crate::git::status(path)?.dirty {
        return Err(Error::git(
            "A helper's changes could not be saved, so its worktree was kept.",
        ));
    }
    Ok(())
}

/// Confirms a proposal: validates edits, creates one isolated worktree session per task and
/// returns the tasks that may start immediately.
fn start(
    state: &AppState,
    session_id: &str,
    edits: Vec<TaskEdit>,
    approval: ApprovalMode,
) -> Result<Vec<(String, String, String)>> {
    if approval == ApprovalMode::Full {
        return Err(Error::agent(
            "Team workers use Ask or Auto approvals; Full access is unavailable for teams.",
        ));
    }
    // Helpers start from the coordinator checkout as it is now, uncommitted work included.
    // Snapshotting runs Git, so it happens before the data lock and is rechecked after.
    let snapshot_cwd = {
        let data = state.data.lock();
        let coordinator = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        session_cwd(&data, coordinator)?
    };
    let base = snapshot_base(&snapshot_cwd, &state.worktree_root.join(".team-snapshots"))?;
    let mut data = state.data.lock();
    state.ensure_running()?;
    let catalog = catalog(state, &data);
    let coordinator = data
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
        .ok_or_else(|| Error::not_found("session not found"))?;
    let team = coordinator
        .team
        .clone()
        .filter(|team| team.status == TeamStatus::Proposed)
        .ok_or_else(|| Error::agent("There is no proposed plan to start."))?;
    if edits.is_empty() || edits.len() > MAX_TASKS {
        return Err(Error::agent("A team needs 1 to 3 tasks."));
    }
    let mut seen = HashSet::new();
    let mut tasks = vec![];
    for edit in &edits {
        let original = team
            .tasks
            .iter()
            .find(|task| task.id == edit.id)
            .ok_or_else(|| Error::agent("Team tasks can only be edited or removed."))?;
        if !seen.insert(edit.id.clone()) {
            return Err(Error::agent("Team tasks can only be edited or removed."));
        }
        let model = edit.model.clone().filter(|model| !model.trim().is_empty());
        validate_assignment(&edit.provider, model.as_deref(), &catalog)?;
        let mut task = original.clone();
        task.title = printable(&edit.title, MAX_TITLE_CHARS, "title")?;
        task.provider = edit.provider.clone();
        task.model = model;
        tasks.push(task);
    }
    // Keep plan order; a removed prerequisite removes the dependency.
    tasks.sort_by_key(|task| {
        team.tasks
            .iter()
            .position(|original| original.id == task.id)
    });
    let kept = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
    for task in &mut tasks {
        if task
            .after
            .as_ref()
            .is_some_and(|after| !kept.contains(after))
        {
            task.after = None;
        }
    }
    if session_cwd(&data, &coordinator)? != snapshot_cwd {
        return Err(Error::agent(
            "The coordinator workspace changed; try again.",
        ));
    }
    let mut starts = vec![];
    let mut created: Vec<String> = vec![];
    for task in &mut tasks {
        let request = crate::models::CreateSessionRequest {
            project_id: coordinator.project_id.clone(),
            title: Some(task.title.clone()),
            agent: task.provider.clone(),
            isolated_worktree: true,
            model: task.model.clone(),
            share_worktree_of: None,
        };
        let worker = match crate::commands::create_session_locked(state, &mut data, request, &base)
        {
            Ok(worker) => worker,
            Err(error) => {
                // Roll back worktrees created for this confirmation before reporting.
                for id in &created {
                    remove_worker(&mut data, id);
                }
                return Err(error);
            }
        };
        created.push(worker.id.clone());
        task.worker_session_id = Some(worker.id.clone());
        if let Some(session) = coordinator_mut(&mut data, &worker.id) {
            session.team_worker = Some(TeamWorker {
                coordinator_session_id: session_id.to_owned(),
                task_id: task.id.clone(),
            });
            session.execution.approval = Some(approval);
        }
        if task.after.is_none() {
            task.state = TaskState::Running;
            starts.push((session_id.to_owned(), task.id.clone(), worker.id));
        }
    }
    let team = team_mut(&mut data, session_id)?;
    team.tasks = tasks;
    team.status = TeamStatus::Running;
    team.approval = Some(approval);
    drop(data);
    state.persist()?;
    Ok(starts)
}

/// The coordinator checkout as it is now, including uncommitted and untracked (not ignored)
/// files, as a private commit on top of `HEAD`. A temporary index keeps the person's staging
/// area and files untouched. Without local changes this is just `HEAD`.
pub(crate) fn snapshot_base(cwd: &Path, scratch: &Path) -> Result<String> {
    snapshot_commit(cwd, scratch, "Sirus team: uncommitted changes")
}

/// [`snapshot_base`] with its own commit message (moving a conversation, ADR-133, uses it too).
pub(crate) fn snapshot_commit(cwd: &Path, scratch: &Path, message: &str) -> Result<String> {
    let head = crate::git::run_ok(cwd, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    if crate::git::status(cwd)?.changes.is_empty() {
        return Ok(head);
    }
    std::fs::create_dir_all(scratch)?;
    let index = scratch.join(format!("team-index-{}", uuid::Uuid::new_v4()));
    // Starting from a copy of the real index keeps Git's stat cache, so unchanged files are not rehashed.
    let real = crate::git::run_ok(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?;
    let _ = std::fs::copy(&real, &index);
    let env = [("GIT_INDEX_FILE", index.as_os_str())];
    let result = (|| {
        let added = crate::git::run_env(cwd, &["add", "-A"], &env)?;
        if !added.status.success() {
            return Err(Error::git("Cannot snapshot your uncommitted changes."));
        }
        let tree = crate::git::run_env(cwd, &["write-tree"], &env)?;
        if !tree.status.success() {
            return Err(Error::git("Cannot snapshot your uncommitted changes."));
        }
        let tree = String::from_utf8_lossy(&tree.stdout).trim().to_owned();
        let identity = [
            ("GIT_AUTHOR_NAME", std::ffi::OsStr::new("Sirus")),
            ("GIT_AUTHOR_EMAIL", std::ffi::OsStr::new("sirus@localhost")),
            ("GIT_COMMITTER_NAME", std::ffi::OsStr::new("Sirus")),
            (
                "GIT_COMMITTER_EMAIL",
                std::ffi::OsStr::new("sirus@localhost"),
            ),
        ];
        let commit = crate::git::run_env(
            cwd,
            &["commit-tree", &tree, "-p", &head, "-m", message],
            &identity,
        )?;
        if !commit.status.success() {
            return Err(Error::git("Cannot snapshot your uncommitted changes."));
        }
        Ok(String::from_utf8_lossy(&commit.stdout).trim().to_owned())
    })();
    let _ = std::fs::remove_file(&index);
    let _ = std::fs::remove_file(index.with_extension("lock"));
    result
}

fn remove_worker(data: &mut AppData, id: &str) {
    if let Some(session) = data.sessions.iter().find(|session| session.id == id) {
        if let Some(project) = data
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
        {
            let _ = crate::worktree::remove(&PathBuf::from(&project.path), &session.worktree, true);
        }
    }
    data.sessions.retain(|session| session.id != id);
}

/// Moves a helper's own changes onto a snapshot of `target` with Git's three-way merge.
/// Returns the files left with conflict markers (at most 20 names).
fn rebase_helper(
    target: &Path,
    worker_cwd: &Path,
    branch: &str,
    scratch: &Path,
) -> Result<Vec<String>> {
    crate::git::run_ok(worker_cwd, &["check-ref-format", "--branch", branch])?;
    let base = snapshot_base(target, scratch)?;
    let dirty = !crate::git::status(worker_cwd)?.changes.is_empty();
    if dirty {
        crate::git::run_ok(
            worker_cwd,
            &[
                "stash",
                "push",
                "--include-untracked",
                "-m",
                "Sirus team: resolve",
            ],
        )?;
    }
    crate::git::run_ok(worker_cwd, &["checkout", "-q", "-B", branch, &base])?;
    if dirty {
        // A conflicting pop keeps the stash and leaves markers for the helper; that is expected.
        let _ = crate::git::run(worker_cwd, &["stash", "pop"])?;
    }
    let unmerged = crate::git::run(
        worker_cwd,
        &["diff", "--name-only", "--diff-filter=U", "-z"],
    )?;
    let conflicted = unmerged
        .stdout
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .take(20)
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect::<Vec<_>>();
    Ok(conflicted)
}

/// Rebases a conflicted helper onto the coordinator checkout as it is now (merged teammates and
/// the person's latest edits included). Its own changes are stashed, the app-owned branch moves
/// to that snapshot, and the stash is popped with Git's three-way merge. Nothing touches the
/// person's files. Returns the helper and a resolution prompt when conflicts remain.
fn prepare_resolve(
    state: &AppState,
    session_id: &str,
    task_id: &str,
) -> Result<Option<(String, String)>> {
    let (target, worker_id, worker_cwd, branch) = {
        let mut data = state.data.lock();
        state.ensure_running()?;
        let coordinator = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .cloned()
            .ok_or_else(|| Error::not_found("session not found"))?;
        let target = session_cwd(&data, &coordinator)?;
        let team = team_mut(&mut data, session_id)?;
        if !matches!(team.status, TeamStatus::Ready | TeamStatus::Stopped) {
            return Err(Error::agent(
                "Wait until every task finishes before resolving.",
            ));
        }
        let task = team
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or_else(|| Error::not_found("team task not found"))?;
        if task.merge != Some(MergeState::Conflict) {
            return Err(Error::agent(
                "Only a task that stopped on a conflict can be resolved.",
            ));
        }
        let worker_id = task
            .worker_session_id
            .clone()
            .ok_or_else(|| Error::not_found("team worker session not found"))?;
        let worker = data
            .sessions
            .iter()
            .find(|session| session.id == worker_id)
            .ok_or_else(|| Error::not_found("team worker session not found"))?;
        if worker.status.is_active() || !worker.worktree.isolated {
            return Err(Error::agent("This helper cannot be resolved right now."));
        }
        let branch = worker.worktree.branch.clone();
        (
            target,
            worker_id.clone(),
            session_cwd(&data, worker)?,
            branch,
        )
    };
    let conflicted = rebase_helper(
        &target,
        &worker_cwd,
        &branch,
        &state.worktree_root.join(".team-snapshots"),
    )?;
    let mut data = state.data.lock();
    let team = team_mut(&mut data, session_id)?;
    let task = team
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .ok_or_else(|| Error::not_found("team task not found"))?;
    task.merge = None;
    task.note = None;
    if conflicted.is_empty() {
        // The three-way merge settled it; the task is ready to merge again.
        task.state = TaskState::Done;
        drop(data);
        state.persist()?;
        return Ok(None);
    }
    task.state = TaskState::Running;
    team.status = TeamStatus::Running;
    drop(data);
    state.persist()?;
    Ok(Some((
        worker_id,
        format!(
            "The person's latest work and your teammates' merged changes are now in your copy, on top of your own changes. Git left conflict markers in: {}.\n\nResolve every conflict so both sides keep working together and your task's intent is preserved. Remove all conflict markers (<<<<<<<, =======, >>>>>>>). Do not commit, create or switch branches, or push. When you finish, reply with a short summary of how you resolved it.",
            conflicted.join(", ")
        ),
    )))
}

async fn merge_response(
    app: &AppHandle,
    state: &Arc<AppState>,
    session_id: String,
    decision: Option<(String, bool)>,
) -> Result<TeamActionResponse> {
    let shared = state.clone();
    let id = session_id.clone();
    native_task(move || merge(&shared, &id, decision)).await?;
    let sessions = snapshot(&state.data.lock(), &session_id);
    if let Some(coordinator) = sessions.first() {
        emit(app, coordinator);
    }
    Ok(TeamActionResponse {
        sessions,
        removed: vec![],
    })
}

struct Pending {
    task_id: String,
    worker_cwd: PathBuf,
    paths: Vec<String>,
    allow_outside: bool,
}

/// "Merge all": plan order, one task at a time, applying each finished task's changes to the
/// coordinator checkout without committing. Stops at the first conflict or out-of-area change
/// and applies nothing from that task.
fn merge(state: &AppState, session_id: &str, decision: Option<(String, bool)>) -> Result<()> {
    let (target, queue) = {
        let mut data = state.data.lock();
        state.ensure_running()?;
        let coordinator = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .cloned()
            .ok_or_else(|| Error::not_found("session not found"))?;
        let target = session_cwd(&data, &coordinator)?;
        let team = team_mut(&mut data, session_id)?;
        // A stopped team can still merge the tasks that finished before the stop.
        if !matches!(team.status, TeamStatus::Ready | TeamStatus::Stopped) {
            return Err(Error::agent(
                "Wait until every task finishes before merging.",
            ));
        }
        if let Some((task_id, merge_anyway)) = &decision {
            let task = team
                .tasks
                .iter_mut()
                .find(|task| &task.id == task_id)
                .ok_or_else(|| Error::not_found("team task not found"))?;
            if !matches!(
                task.merge,
                Some(MergeState::Conflict | MergeState::OutOfScope)
            ) {
                return Err(Error::agent(
                    "Only a stopped merge can be skipped or forced.",
                ));
            }
            if *merge_anyway {
                if task.merge != Some(MergeState::OutOfScope) {
                    return Err(Error::agent(
                        "Only changes outside the planned paths can be merged anyway.",
                    ));
                }
                task.allow_outside = true;
                task.merge = None;
            } else {
                task.merge = Some(MergeState::Skipped);
            }
            task.note = None;
        }
        let tasks = team.tasks.clone();
        let mut queue = vec![];
        for task in tasks.iter().filter(|task| {
            task.state == TaskState::Done
                && !matches!(task.merge, Some(MergeState::Merged | MergeState::Skipped))
        }) {
            let worker = task
                .worker_session_id
                .as_ref()
                .and_then(|id| data.sessions.iter().find(|session| &session.id == id))
                .ok_or_else(|| Error::not_found("team worker session not found"))?;
            if worker.status.is_active() {
                return Err(Error::agent(
                    "A team worker is running again; wait for it to finish.",
                ));
            }
            queue.push(Pending {
                task_id: task.id.clone(),
                worker_cwd: session_cwd(&data, worker)?,
                paths: task.paths.clone(),
                allow_outside: task.allow_outside,
            });
        }
        (target, queue)
    };
    for pending in queue {
        let outcome = merge_one(&target, &pending);
        let stop = !matches!(outcome, Ok(MergeState::Merged));
        {
            let mut data = state.data.lock();
            let team = team_mut(&mut data, session_id)?;
            if let Some(task) = team
                .tasks
                .iter_mut()
                .find(|task| task.id == pending.task_id)
            {
                match &outcome {
                    Ok(MergeState::OutOfScope) => {
                        task.merge = Some(MergeState::OutOfScope);
                    }
                    Ok(state) => {
                        task.merge = Some(*state);
                        task.note = None;
                    }
                    Err(error) => {
                        task.merge = Some(MergeState::Conflict);
                        task.note = Some(short(&error.to_string()));
                    }
                }
                if let Ok(MergeState::OutOfScope) = outcome {
                    task.note = Some("Changed files outside its planned paths.".into());
                }
            }
        }
        state.persist()?;
        if stop {
            return Ok(());
        }
    }
    let mut data = state.data.lock();
    let team = team_mut(&mut data, session_id)?;
    if team.tasks.iter().all(|task| {
        task.state != TaskState::Done
            || matches!(task.merge, Some(MergeState::Merged | MergeState::Skipped))
    }) {
        team.status = TeamStatus::Done;
    }
    drop(data);
    state.persist()
}

/// One task: collect the worker's changes, check paths and local edits, then apply atomically.
fn merge_one(target: &Path, pending: &Pending) -> Result<MergeState> {
    let worker = &pending.worker_cwd;
    // The worker worktree is app-owned; staging there only prepares a reviewable patch.
    crate::git::run_ok(worker, &["add", "-A"])?;
    let names = crate::git::run(
        worker,
        &[
            "diff",
            "--cached",
            "--no-renames",
            "--name-only",
            "-z",
            "HEAD",
        ],
    )?;
    if !names.status.success() {
        return Err(Error::git("Cannot read the worker's changes."));
    }
    let changed = names
        .stdout
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect::<Vec<_>>();
    if changed.is_empty() {
        return Ok(MergeState::Merged);
    }
    if !pending.allow_outside && !changed.iter().all(|path| in_scope(path, &pending.paths)) {
        return Ok(MergeState::OutOfScope);
    }
    let patch = crate::git::run(
        worker,
        &[
            "diff",
            "--cached",
            "--no-renames",
            "--binary",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
        ],
    )?;
    if !patch.status.success() {
        return Err(Error::git("Cannot read the worker's changes."));
    }
    if patch.stdout.len() > MAX_PATCH_BYTES {
        return Err(Error::git(
            "The worker's changes exceed the 8 MiB merge limit.",
        ));
    }
    // A resolution that left conflict markers must never reach the person's files.
    if patch
        .stdout
        .split(|byte| *byte == b'\n')
        .any(|line| line.starts_with(b"+<<<<<<< ") || line.starts_with(b"+>>>>>>> "))
    {
        return Err(Error::git("The changes still contain conflict markers."));
    }
    // Helpers start from the person's uncommitted state, so local edits are expected. `git apply
    // --check` verifies every hunk's context against the current file: edits the person made
    // since the team started merge only when they touch different lines; otherwise it stops.
    let check = crate::git::run_input(
        target,
        &["apply", "--check", "--binary", "--whitespace=nowarn", "-"],
        &patch.stdout,
    )?;
    if !check.status.success() {
        let reason = String::from_utf8_lossy(&check.stderr).into_owned();
        return Err(Error::git(if reason.trim().is_empty() {
            "The changes do not apply cleanly.".to_owned()
        } else {
            reason
        }));
    }
    let applied = crate::git::run_input(
        target,
        &["apply", "--binary", "--whitespace=nowarn", "-"],
        &patch.stdout,
    )?;
    if !applied.status.success() {
        return Err(Error::git("The changes do not apply cleanly."));
    }
    Ok(MergeState::Merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        vec![
            (
                AgentProviderId::Codex,
                vec![("gpt-6.1-sol".into(), "GPT-6.1-Sol".into())],
            ),
            (AgentProviderId::Claude, vec![]),
        ]
    }
    fn plan(tasks: &str) -> String {
        format!("I split it in two.\n{PLAN_OPEN}\n{{\"summary\":\"Two parts\",\"tasks\":{tasks}}}\n{PLAN_CLOSE}")
    }

    #[test]
    fn cleanup_preserves_merged_helper_edits_on_the_helper_branch() {
        let repo = crate::git::tests::Repo::new();
        let root = repo.cwd();
        let tree = repo.0.join("helper");
        crate::git::run_ok(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "sirus/helper-1",
                tree.to_str().unwrap(),
            ],
        )
        .unwrap();
        std::fs::write(tree.join("file.txt"), "after\n").unwrap();
        std::fs::write(tree.join("new.md"), "notes\n").unwrap();
        preserve_helper_work(&tree, "Paginate orders").unwrap();
        assert!(!crate::git::status(&tree).unwrap().dirty);
        let log =
            crate::git::run_ok(&root, &["log", "-1", "--format=%an %s", "sirus/helper-1"]).unwrap();
        assert_eq!(log, "Sirus Sirus team task: Paginate orders");
        let files = crate::git::run_ok(
            &root,
            &["show", "--name-only", "--format=", "sirus/helper-1"],
        )
        .unwrap();
        assert!(files.contains("file.txt") && files.contains("new.md"));
        // A clean worktree is left alone; the normal non-force removal now succeeds.
        preserve_helper_work(&tree, "unchanged").unwrap();
        let removed =
            crate::git::run(&root, &["worktree", "remove", "--", tree.to_str().unwrap()]).unwrap();
        assert!(removed.status.success());
        assert!(crate::git::run_ok(&root, &["rev-parse", "--verify", "sirus/helper-1"]).is_ok());
    }

    /// Live contract check: the real CLI, read-only in a disposable repository, must
    /// answer the planning prompt with a plan the native parser accepts. Spends one
    /// small turn. Run with LIVE_TEAM_PROVIDER=claude|codex and `--ignored`.
    #[test]
    #[ignore]
    fn live_planning_contract() {
        let provider = std::env::var("LIVE_TEAM_PROVIDER").unwrap_or_else(|_| "claude".into());
        let repo = crate::git::tests::Repo::new();
        let root = repo.0.clone();
        for (path, body) in [
            (
                "package.json",
                "{\"name\":\"shop\",\"scripts\":{\"test\":\"node --test\"}}\n",
            ),
            (
                "src/api/orders.js",
                "export function listOrders(db) { return db.orders; }\n",
            ),
            (
                "src/ui/OrdersPage.js",
                "export function OrdersPage(orders) { return orders.map(o => o.id).join(','); }\n",
            ),
            (
                "tests/orders.test.js",
                "import test from 'node:test';\ntest('placeholder', () => {});\n",
            ),
        ] {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, body).unwrap();
        }
        let catalog: Catalog = vec![
            (AgentProviderId::Codex, vec![]),
            (AgentProviderId::Claude, vec![]),
        ];
        let request = std::env::var("LIVE_TEAM_REQUEST").unwrap_or_else(|_| {
            "Add pagination (page and pageSize) to listOrders, show page controls in OrdersPage, and add tests for both.".into()
        });
        let prompt = planning_prompt(&request, &catalog);
        let output = match provider.as_str() {
            "codex" => {
                let last = root.join(".last-message.txt");
                let status = std::process::Command::new("codex")
                    .args([
                        "exec",
                        "--sandbox",
                        "read-only",
                        "--skip-git-repo-check",
                        "-o",
                    ])
                    .arg(&last)
                    .arg(&prompt)
                    .current_dir(&root)
                    .stdin(std::process::Stdio::null())
                    .status()
                    .unwrap();
                assert!(status.success());
                std::fs::read_to_string(last).unwrap()
            }
            _ => {
                let out = std::process::Command::new("claude")
                    .args(["-p", "--permission-mode", "plan", "--output-format", "text"])
                    .arg(&prompt)
                    .current_dir(&root)
                    .stdin(std::process::Stdio::null())
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                String::from_utf8(out.stdout).unwrap()
            }
        };
        println!("--- {provider} reply ---\n{output}\n---");
        let (summary, _, tasks) = parse_plan(&output, &catalog).expect("valid team plan");
        println!("summary: {summary}");
        for task in &tasks {
            println!(
                "task {} [{} {:?}] paths={:?} after={:?}: {}",
                task.id,
                task.provider.key(),
                task.model,
                task.paths,
                task.after,
                task.title
            );
        }
        assert!(!tasks.is_empty() && tasks.len() <= MAX_TASKS);
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&root)
            .output()
            .unwrap();
        let changed = String::from_utf8_lossy(&status.stdout)
            .lines()
            .filter(|line| !line.ends_with(".last-message.txt"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        println!("workspace changes after planning: {changed:?}");
    }

    #[test]
    fn parses_a_valid_plan_and_hides_the_block() {
        let content = plan(
            r#"[{"id":"t1","title":"API","instructions":"Add the route","provider":"codex","model":"gpt-6.1-sol","paths":["src/api/**","package.json"]},{"id":"t2","title":"Tests","instructions":"Cover it","provider":"claude","model":null,"paths":["./tests/api/**"],"after":"t1"}]"#,
        );
        let (visible, summary, tasks) = parse_plan(&content, &catalog()).unwrap();
        assert_eq!(visible, "I split it in two.");
        assert_eq!(summary.as_deref(), Some("Two parts"));
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1].paths, vec!["tests/api/**"]);
        assert_eq!(tasks[1].after.as_deref(), Some("t1"));
        assert_eq!(tasks[0].state, TaskState::Pending);
    }

    #[test]
    fn refuses_untrusted_plans() {
        let task = |extra: &str| {
            plan(&format!(
                r#"[{{"id":"t1","title":"A","instructions":"B","provider":"codex","paths":["src/**"]{extra}}}]"#
            ))
        };
        for bad in [
            plan("[]"),
            plan(&format!(
                "[{}]",
                [r#"{"id":"x","title":"A","instructions":"B","provider":"codex","paths":["a"]}"#;
                    4]
                    .join(",")
            )),
            task(r#","model":"unknown""#),
            plan(
                r#"[{"id":"t1","title":"A","instructions":"B","provider":"cursor","paths":["src/**"]}]"#,
            ),
            plan(
                r#"[{"id":"t1","title":"A","instructions":"B","provider":"opencode","paths":["src/**"]}]"#,
            ),
            plan(
                r#"[{"id":"t1","title":"A","instructions":"B","provider":"codex","paths":["../etc/**"]}]"#,
            ),
            plan(
                r#"[{"id":"t1","title":"A","instructions":"B","provider":"codex","paths":["/abs"]}]"#,
            ),
            plan(
                r#"[{"id":"t1","title":"A","instructions":"B","provider":"codex","paths":["src/*.ts"]}]"#,
            ),
            plan(r#"[{"id":"t1","title":"A","instructions":"B","provider":"codex","paths":[]}]"#),
            task(r#","after":"t1""#),
            task(r#","after":"t9""#),
            "no plan here".into(),
            format!("{PLAN_OPEN}{{not json"),
        ] {
            assert!(parse_plan(&bad, &catalog()).is_err(), "{bad}");
        }
    }

    #[test]
    fn scopes_match_exact_files_and_folders_only() {
        let scopes = vec!["src/ui/**".to_owned(), "package.json".to_owned()];
        assert!(in_scope("src/ui/a.tsx", &scopes));
        assert!(in_scope("src/ui/deep/b.css", &scopes));
        assert!(in_scope("package.json", &scopes));
        assert!(!in_scope("src/uix/a.tsx", &scopes));
        assert!(!in_scope("src/ui", &scopes));
        assert!(!in_scope("package-lock.json", &scopes));
        assert!(valid_scope("a/b.ts") && valid_scope("a/**"));
        for bad in ["", "/a", "a/../b", "a\\b", "a/*", "**", "./a", "a//b"] {
            assert!(!valid_scope(bad), "{bad}");
        }
    }

    #[test]
    fn worker_prompt_carries_only_its_task() {
        let task = TeamTask {
            id: "t1".into(),
            title: "API".into(),
            instructions: "Add the route".into(),
            provider: AgentProviderId::Codex,
            model: None,
            paths: vec!["src/api/**".into()],
            after: None,
            worker_session_id: None,
            state: TaskState::Pending,
            merge: None,
            note: None,
            allow_outside: false,
        };
        let prompt = worker_prompt(&task);
        assert!(prompt.contains("Add the route") && prompt.contains("src/api/**"));
        assert!(prompt.contains("Do not commit"));
    }

    #[test]
    fn advance_starts_dependents_only_after_success_and_stops_blocked_ones() {
        let mut data: AppData = serde_json::from_value(serde_json::json!({
            "projects": [], "settings": {},
            "sessions": [{"id":"c","projectId":"p","title":"Lead","agent":"claude","status":"completed","createdAt":"t","lastActivityAt":"t","worktree":{"path":"/x","branch":"main","isolated":false},"messages":[],
                "team": {"messageId":"m","status":"running","summary":null,"error":null,"approval":"ask","tasks":[
                    {"id":"t1","title":"A","instructions":"a","provider":"codex","model":null,"paths":["a/**"],"after":null,"workerSessionId":"w1","state":"running","merge":null,"note":null},
                    {"id":"t2","title":"B","instructions":"b","provider":"claude","model":null,"paths":["b/**"],"after":"t1","workerSessionId":"w2","state":"pending","merge":null,"note":null}]}}]
        })).unwrap();
        let link = TeamWorker {
            coordinator_session_id: "c".into(),
            task_id: "t1".into(),
        };
        let starts = advance(&mut data, &link, &SessionStatus::Completed);
        assert_eq!(starts, vec![("c".into(), "t2".into(), "w2".into())]);
        let team = data.sessions[0].team.as_ref().unwrap();
        assert_eq!(team.tasks[1].state, TaskState::Running);
        assert_eq!(team.status, TeamStatus::Running);

        // A failed prerequisite never starts its dependent.
        let team = data.sessions[0].team.as_mut().unwrap();
        team.tasks[0].state = TaskState::Running;
        team.tasks[1].state = TaskState::Pending;
        assert!(advance(&mut data, &link, &SessionStatus::Failed).is_empty());
        let team = data.sessions[0].team.as_ref().unwrap();
        assert_eq!(team.tasks[1].state, TaskState::Stopped);
        assert_eq!(team.status, TeamStatus::Ready);
    }

    #[test]
    fn recovery_never_resumes_work() {
        let mut data: AppData = serde_json::from_value(serde_json::json!({
            "projects": [], "settings": {},
            "sessions": [
                {"id":"c","projectId":"p","title":"Lead","agent":"claude","status":"completed","createdAt":"t","lastActivityAt":"t","worktree":{"path":"/x","branch":"main","isolated":false},"messages":[],
                "team": {"messageId":"m","status":"running","summary":null,"error":null,"approval":"ask","tasks":[
                    {"id":"t1","title":"A","instructions":"a","provider":"codex","model":null,"paths":["a/**"],"after":null,"workerSessionId":"w1","state":"running","merge":null,"note":null},
                    {"id":"t2","title":"B","instructions":"b","provider":"claude","model":null,"paths":["b/**"],"after":"t1","workerSessionId":"w2","state":"pending","merge":null,"note":null}]}},
                {"id":"w1","projectId":"p","title":"A","agent":"codex","status":"completed","createdAt":"t","lastActivityAt":"t","worktree":{"path":"/y","branch":"b","isolated":true},"messages":[]}]
        })).unwrap();
        assert!(recover(&mut data));
        let team = data.sessions[0].team.as_ref().unwrap();
        assert_eq!(team.status, TeamStatus::Stopped);
        assert_eq!(team.tasks[0].state, TaskState::Done);
        assert_eq!(team.tasks[1].state, TaskState::Stopped);
    }

    #[test]
    fn actions_are_closed() {
        assert!(serde_json::from_value::<TeamAction>(serde_json::json!({"type":"start","sessionId":"s","tasks":[],"approval":"ask","extra":1})).is_err());
        assert!(serde_json::from_value::<TeamAction>(
            serde_json::json!({"type":"run","sessionId":"s"})
        )
        .is_err());
        assert!(serde_json::from_value::<TeamAction>(
            serde_json::json!({"type":"cleanup","sessionId":"s","confirm":true})
        )
        .is_ok());
    }

    #[test]
    fn merge_applies_without_commits_and_stops_safely() {
        use std::fs;
        let repo = crate::git::tests::Repo::new();
        let head = crate::git::run_ok(&repo.cwd(), &["rev-parse", "HEAD"]).unwrap();
        let tree = |name: &str| {
            let tree = crate::worktree::create_isolated_at(
                &repo.cwd(),
                &repo.0.join("worktrees"),
                &format!("{name}-0000000"),
                name,
                "sirus/{session-name}-{id}",
                &head,
            )
            .unwrap();
            PathBuf::from(tree.path)
        };
        let pending = |cwd: PathBuf, paths: &[&str]| Pending {
            task_id: "t".into(),
            worker_cwd: cwd,
            paths: paths.iter().map(|p| p.to_string()).collect(),
            allow_outside: false,
        };

        // A finished worker edits a tracked file and adds a new one inside its area.
        let first = tree("first");
        fs::write(first.join("file.txt"), "after\n").unwrap();
        fs::create_dir_all(first.join("src")).unwrap();
        fs::write(first.join("src/new.ts"), "export {};\n").unwrap();
        let done = pending(first, &["file.txt", "src/**"]);
        assert_eq!(merge_one(&repo.cwd(), &done).unwrap(), MergeState::Merged);
        assert_eq!(
            fs::read_to_string(repo.cwd().join("file.txt")).unwrap(),
            "after\n"
        );
        assert!(repo.cwd().join("src/new.ts").exists());
        // Applied to the working tree only: no commit was created.
        assert_eq!(
            crate::git::run_ok(&repo.cwd(), &["rev-parse", "HEAD"]).unwrap(),
            head
        );

        // Changes outside the planned paths stop before anything is applied.
        let outside = tree("outside");
        fs::write(outside.join("other.txt"), "x\n").unwrap();
        assert_eq!(
            merge_one(&repo.cwd(), &pending(outside.clone(), &["src/**"])).unwrap(),
            MergeState::OutOfScope
        );
        assert!(!repo.cwd().join("other.txt").exists());

        // A file the person changed differently since the helper's base stops as a conflict.
        let local = tree("local");
        fs::write(local.join("file.txt"), "worker\n").unwrap();
        assert!(merge_one(&repo.cwd(), &pending(local, &["file.txt"])).is_err());
        assert_eq!(
            fs::read_to_string(repo.cwd().join("file.txt")).unwrap(),
            "after\n"
        );

        // A real conflict against committed state applies nothing.
        crate::git::run_ok(&repo.cwd(), &["add", "-A"]).unwrap();
        crate::git::run_ok(&repo.cwd(), &["commit", "-qm", "merged first"]).unwrap();
        let conflict = tree("conflict");
        fs::write(conflict.join("file.txt"), "different\n").unwrap();
        assert!(merge_one(&repo.cwd(), &pending(conflict, &["file.txt"])).is_err());
        assert_eq!(
            fs::read_to_string(repo.cwd().join("file.txt")).unwrap(),
            "after\n"
        );
        assert!(crate::git::status(&repo.cwd()).unwrap().changes.is_empty());
    }

    #[test]
    fn helpers_start_from_uncommitted_work_without_touching_the_index() {
        use std::fs;
        let repo = crate::git::tests::Repo::new();
        let head = crate::git::run_ok(&repo.cwd(), &["rev-parse", "HEAD"]).unwrap();
        // Clean checkout: the base is simply HEAD.
        assert_eq!(
            snapshot_base(&repo.cwd(), &repo.0.join("scratch")).unwrap(),
            head
        );
        // Uncommitted edit, an untracked file and an ignored file.
        fs::write(repo.cwd().join("file.txt"), "draft\n").unwrap();
        fs::write(repo.cwd().join("new.txt"), "new\n").unwrap();
        fs::write(repo.cwd().join(".gitignore"), "secret.env\n").unwrap();
        fs::write(repo.cwd().join("secret.env"), "TOKEN=x\n").unwrap();
        let staged_before =
            crate::git::run_ok(&repo.cwd(), &["diff", "--cached", "--name-only"]).unwrap();
        let base = snapshot_base(&repo.cwd(), &repo.0.join("scratch")).unwrap();
        assert_ne!(base, head);
        // The person's staging area and files are untouched.
        assert_eq!(
            crate::git::run_ok(&repo.cwd(), &["diff", "--cached", "--name-only"]).unwrap(),
            staged_before
        );
        assert_eq!(
            crate::git::run_ok(&repo.cwd(), &["rev-parse", "HEAD"]).unwrap(),
            head
        );
        assert_eq!(
            fs::read_to_string(repo.cwd().join("file.txt")).unwrap(),
            "draft\n"
        );
        // A helper worktree sees the uncommitted work but not ignored files.
        let tree = crate::worktree::create_isolated_at(
            &repo.cwd(),
            &repo.0.join("worktrees"),
            "seed-0000000",
            "seed",
            "sirus/{session-name}-{id}",
            &base,
        )
        .unwrap();
        let worker = PathBuf::from(&tree.path);
        assert_eq!(
            fs::read_to_string(worker.join("file.txt")).unwrap(),
            "draft\n"
        );
        assert!(worker.join("new.txt").exists());
        assert!(!worker.join("secret.env").exists());
        // Only the helper's own edit comes back, onto the still-uncommitted file.
        fs::write(worker.join("file.txt"), "draft\nhelper\n").unwrap();
        let pending = Pending {
            task_id: "t".into(),
            worker_cwd: worker,
            paths: vec!["file.txt".into()],
            allow_outside: false,
        };
        assert_eq!(
            merge_one(&repo.cwd(), &pending).unwrap(),
            MergeState::Merged
        );
        assert_eq!(
            fs::read_to_string(repo.cwd().join("file.txt")).unwrap(),
            "draft\nhelper\n"
        );
        assert_eq!(
            fs::read_to_string(repo.cwd().join("new.txt")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn resolve_rebases_a_conflicted_helper_without_touching_the_person() {
        use std::fs;
        let repo = crate::git::tests::Repo::new();
        let head = crate::git::run_ok(&repo.cwd(), &["rev-parse", "HEAD"]).unwrap();
        let make = |name: &str| {
            let tree = crate::worktree::create_isolated_at(
                &repo.cwd(),
                &repo.0.join("worktrees"),
                &format!("{name}-0000000"),
                name,
                "sirus/{session-name}-{id}",
                &head,
            )
            .unwrap();
            (PathBuf::from(&tree.path), tree.branch)
        };
        let scratch = repo.0.join("scratch");
        // The helper and the person changed the same line.
        let (worker, branch) = make("conflict");
        fs::write(worker.join("file.txt"), "worker\n").unwrap();
        fs::write(repo.cwd().join("file.txt"), "person\n").unwrap();
        let pending = Pending {
            task_id: "t".into(),
            worker_cwd: worker.clone(),
            paths: vec!["file.txt".into()],
            allow_outside: false,
        };
        assert!(merge_one(&repo.cwd(), &pending).is_err());
        let conflicted = rebase_helper(&repo.cwd(), &worker, &branch, &scratch).unwrap();
        assert_eq!(conflicted, vec!["file.txt".to_owned()]);
        assert!(fs::read_to_string(worker.join("file.txt"))
            .unwrap()
            .contains("<<<<<<<"));
        // The person's files never changed, and unresolved markers can never be merged.
        assert_eq!(
            fs::read_to_string(repo.cwd().join("file.txt")).unwrap(),
            "person\n"
        );
        assert!(merge_one(&repo.cwd(), &pending).is_err());
        // The helper resolves in its own copy; only its resolution comes back.
        fs::write(worker.join("file.txt"), "person\nworker\n").unwrap();
        assert_eq!(
            merge_one(&repo.cwd(), &pending).unwrap(),
            MergeState::Merged
        );
        assert_eq!(
            fs::read_to_string(repo.cwd().join("file.txt")).unwrap(),
            "person\nworker\n"
        );

        // Non-overlapping edits settle with the three-way merge alone.
        let (clean, clean_branch) = make("clean");
        fs::write(clean.join("other.txt"), "helper\n").unwrap();
        assert!(rebase_helper(&repo.cwd(), &clean, &clean_branch, &scratch)
            .unwrap()
            .is_empty());
        assert_eq!(
            fs::read_to_string(clean.join("other.txt")).unwrap(),
            "helper\n"
        );
        assert_eq!(
            fs::read_to_string(clean.join("file.txt")).unwrap(),
            "person\nworker\n"
        );
    }
}
