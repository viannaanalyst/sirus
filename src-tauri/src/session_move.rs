//! Moving a conversation to another of the owner's computers (ADR-133).
//!
//! The source (usually the Mac) snapshots the session's checkout, including uncommitted and
//! untracked files, as one commit on top of `HEAD` with a private index (the person's files,
//! branch and staging area stay as they are) and pushes it to `origin` as
//! `sirus/move-<short id>`. The target (usually a `sirus-host` server) fetches that branch,
//! checks it out in a new isolated worktree and creates a session with the same provider and
//! model, the copied transcript (text only, bounded) and a pending handoff recap, so the first
//! turn there knows the conversation. The source session is then marked as moved.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::commands::{native_task, AppState};
use crate::error::{Error, Result};
use crate::models::{
    AgentProviderId, AppData, CreateSessionRequest, Message, MessageRole, Project, Session,
    SessionMoveNote, WorktreeLocationPref,
};
use crate::paths::now_rfc3339;
use crate::runtime::{AppHandle, State};

/// Messages copied with a moved conversation: the most recent ones.
pub const MAX_MOVED_MESSAGES: usize = 2000;
/// One copied message's text.
pub const MAX_MOVED_MESSAGE_BYTES: usize = 256 * 1024;
/// The whole copied transcript (the remote socket accepts 32 MiB per message).
pub const MAX_MOVED_BYTES: usize = 8 * 1024 * 1024;
/// A computer's name as shown in the moved notes.
const MAX_COMPUTER_LABEL: usize = 60;
const BRANCH_PREFIX: &str = "sirus/move-";

/// One copied message: who said it, what, and when. Activity, attachments and visuals stay
/// on the source computer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MovedMessage {
    pub role: MessageRole,
    pub content: String,
    pub created_at: String,
}

/// What the source computer hands to the target after pushing the work.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMoveExport {
    /// `https://github.com/owner/repo`: how the target finds or clones the project.
    pub remote_url: String,
    pub branch: String,
    pub commit: String,
    pub title: String,
    pub agent: AgentProviderId,
    pub model: Option<String>,
    pub messages: Vec<MovedMessage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportSessionRequest {
    pub project_id: String,
    pub branch: String,
    pub title: String,
    pub agent: AgentProviderId,
    #[serde(default)]
    pub model: Option<String>,
    pub messages: Vec<MovedMessage>,
    /// The computer the conversation comes from, as the person calls it.
    pub from_computer: String,
    /// Set when that computer is out of reach (ADR-133 addendum): nothing was pushed, so the
    /// work starts from this branch as `origin` has it (else its default branch), on `branch`.
    #[serde(default)]
    pub from_origin: Option<String>,
}

/// The branch that carries a session's work to another computer.
pub fn move_branch(session_id: &str) -> String {
    let short: String = session_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    format!("{BRANCH_PREFIX}{short}")
}

fn valid_move_branch(branch: &str) -> bool {
    branch.strip_prefix(BRANCH_PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= 40
            && rest
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !rest.starts_with('-')
    })
}

/// A computer's name for the notes: trimmed, one line, bounded.
pub fn computer_label(value: &str) -> Result<String> {
    let label = value.trim();
    if label.is_empty()
        || label.chars().count() > MAX_COMPUTER_LABEL
        || label.chars().any(char::is_control)
    {
        return Err(Error::new(
            "invalid",
            "The computer's name must have 1–60 characters on one line.",
        ));
    }
    Ok(label.to_string())
}

fn truncate_bytes(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The transcript as it travels: user and agent text only, finished messages, each bounded,
/// the most recent ones within the total limit.
pub fn export_messages(messages: &[Message]) -> Vec<MovedMessage> {
    let mut out = Vec::new();
    let mut total = 0usize;
    for message in messages.iter().rev() {
        if message.streaming
            || message.role == MessageRole::System
            || message.content.trim().is_empty()
        {
            continue;
        }
        let content = truncate_bytes(&message.content, MAX_MOVED_MESSAGE_BYTES).replace('\0', "");
        if out.len() >= MAX_MOVED_MESSAGES || total + content.len() > MAX_MOVED_BYTES {
            break;
        }
        total += content.len();
        out.push(MovedMessage {
            role: message.role.clone(),
            content,
            created_at: message.created_at.clone(),
        });
    }
    out.reverse();
    out
}

/// Strict checks on an incoming move: the branch is a move branch, the title and transcript
/// are within their limits, no system rows, no NUL bytes.
pub fn validate_import(request: &ImportSessionRequest) -> Result<()> {
    let invalid = |message: &str| Err(Error::new("invalid", message.to_string()));
    if !valid_move_branch(&request.branch) {
        return invalid("The moved work must come on a sirus/move-… branch.");
    }
    let title = request.title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        return invalid("The session title must have 1–200 characters.");
    }
    computer_label(&request.from_computer)?;
    if request.messages.len() > MAX_MOVED_MESSAGES {
        return invalid("The conversation has too many messages to move.");
    }
    let mut total = 0usize;
    for message in &request.messages {
        if message.role == MessageRole::System {
            return invalid("Only user and agent messages can be moved.");
        }
        if message.content.len() > MAX_MOVED_MESSAGE_BYTES || message.content.contains('\0') {
            return invalid("A message is too large to move.");
        }
        if message.created_at.len() > 64 {
            return invalid("A message has an invalid date.");
        }
        total += message.content.len();
    }
    if total > MAX_MOVED_BYTES {
        return invalid("The conversation is too large to move.");
    }
    if request
        .model
        .as_ref()
        .is_some_and(|model| model.len() > 200 || model.chars().any(char::is_control))
    {
        return invalid("The model name is not valid.");
    }
    Ok(())
}

/// The copied transcript as this computer's messages of `session_id`. Dates that do not parse
/// become now.
pub fn imported_messages(session_id: &str, messages: &[MovedMessage], now: &str) -> Vec<Message> {
    messages
        .iter()
        .map(|message| Message {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            role: message.role.clone(),
            content: message.content.clone(),
            created_at: chrono::DateTime::parse_from_rfc3339(&message.created_at)
                .map(|_| message.created_at.clone())
                .unwrap_or_else(|_| now.to_string()),
            streaming: false,
            activity: None,
            steers: Vec::new(),
            attachments: Vec::new(),
            launched: Vec::new(),
            visuals: Vec::new(),
            client_request_id: None,
            rotation: None,
        })
        .collect()
}

fn project_of(data: &AppData, project_id: &str) -> Result<Project> {
    data.projects
        .iter()
        .find(|project| project.id == project_id)
        .cloned()
        .ok_or_else(|| Error::not_found("project not found"))
}

/// Where this computer keeps a project's isolated worktrees (the same rule as new sessions).
fn worktree_parent(state: &AppState, data: &AppData, project: &Project) -> PathBuf {
    if matches!(
        data.settings.worktree_location,
        WorktreeLocationPref::Custom
    ) {
        if let Some(base) = &data.settings.worktree_base_path {
            return PathBuf::from(base).join(&project.id);
        }
    }
    state.worktree_root.join(&project.id)
}

/// Source side: snapshots the session's work, pushes it to `origin` as `sirus/move-<id>` and
/// returns what the target needs. Refused while the session runs, without confirmation, or
/// when the project has no GitHub `origin` (the other computer could not get the files).
pub async fn prepare_session_move(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    target: String,
    confirm: bool,
) -> Result<SessionMoveExport> {
    if !confirm {
        return Err(Error::confirmation_required(
            "Moving a conversation pushes a branch to GitHub and needs confirmation.",
        ));
    }
    let target = computer_label(&target)?;
    let state = state.inner().clone();
    native_task(move || {
        state.ensure_running()?;
        let (session, cwd) = {
            let data = state.data_with_messages(&[&session_id]);
            let session = data
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .cloned()
                .ok_or_else(|| Error::not_found("session not found"))?;
            let cwd = crate::commands::session_cwd(&data, &session)?;
            (session, cwd)
        };
        if session.status.is_active()
            || state.agents.lock().contains_key(&session.id)
            || session.messages.iter().any(|message| message.streaming)
        {
            return Err(Error::agent(
                "Stop the conversation before moving it to another computer.",
            ));
        }
        if session.side_chat.is_some() {
            return Err(Error::new("invalid", "Side chats move with their conversation."));
        }
        if session.pull_ask.is_some() {
            return Err(Error::new(
                "invalid",
                "Ask conversations about a pull request stay on this computer.",
            ));
        }
        let Some(remote_url) = crate::git::remote_web_url(&cwd)? else {
            return Err(Error::new(
                "no_remote",
                "This project has no GitHub remote (origin), so the other computer cannot get its files. Publish it to GitHub first.",
            ));
        };
        let scratch = std::env::temp_dir().join("sirus-move");
        let message = format!(
            "Move \"{}\" to {target}\n\nUncommitted work from Sirus, continued on another computer.",
            session.title.chars().take(120).collect::<String>()
        );
        let commit = crate::team::snapshot_commit(&cwd, &scratch, &message)?;
        let branch = move_branch(&session.id);
        crate::git::push_commit_to_branch(&cwd, &commit, &branch)?;
        Ok(SessionMoveExport {
            remote_url,
            branch,
            commit,
            title: session.title.clone(),
            agent: session.agent.clone(),
            model: session.model.clone(),
            messages: export_messages(&session.messages),
        })
    })
    .await
}

/// Target side: the project here whose GitHub `origin` is `remote_url` (compared as GitHub web
/// addresses, case-insensitively), if any.
pub async fn find_project_by_remote(
    state: State<'_, Arc<AppState>>,
    remote_url: String,
) -> Result<Option<Project>> {
    let Some(wanted) = crate::git::normalize_github_web_url(remote_url.trim()) else {
        return Err(Error::new("invalid", "not a GitHub address"));
    };
    let state = state.inner().clone();
    native_task(move || {
        let projects = state.data.lock().projects.clone();
        Ok(projects.into_iter().find(|project| {
            crate::git::remote_web_url(&PathBuf::from(&project.path))
                .ok()
                .flatten()
                .is_some_and(|url| url.eq_ignore_ascii_case(&wanted))
        }))
    })
    .await
}

/// Target side: checks the moved branch out in a new isolated worktree and creates a session
/// with the same provider and model, the copied transcript and a pending handoff recap.
pub async fn import_session(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    request: ImportSessionRequest,
) -> Result<Session> {
    validate_import(&request)?;
    let state = state.inner().clone();
    native_task(move || {
        state.ensure_running()?;
        let (project, parent) = {
            let data = state.data.lock();
            let project = project_of(&data, &request.project_id)?;
            if !data.provider_enabled(&request.agent) {
                return Err(Error::agent("this provider is disabled in Settings"));
            }
            let parent = worktree_parent(&state, &data, &project);
            (project, parent)
        };
        let root = PathBuf::from(&project.path);
        let (worktree, start) = match request.from_origin.as_deref() {
            None => {
                crate::git::fetch_origin_branch(&root, &request.branch)?;
                let worktree = crate::worktree::create_from_origin_branch(
                    &root,
                    &parent,
                    request.title.trim(),
                    &request.branch,
                )?;
                (worktree, None)
            }
            Some(wanted) => {
                let start = if crate::git::fetch_origin_branch(&root, wanted).is_ok() {
                    wanted.to_string()
                } else {
                    let fallback = crate::git::origin_default_branch(&root)
                        .ok_or_else(|| Error::git("origin has no default branch"))?;
                    crate::git::fetch_origin_branch(&root, &fallback)?;
                    fallback
                };
                let worktree = crate::worktree::create_from_origin_start(
                    &root,
                    &parent,
                    request.title.trim(),
                    &request.branch,
                    &start,
                )?;
                (worktree, Some(start))
            }
        };

        let created = (|| {
            let mut data = state.data.lock();
            let mut session = crate::commands::create_session_locked(
                &state,
                &mut data,
                CreateSessionRequest {
                    project_id: project.id.clone(),
                    title: Some(request.title.trim().to_string()),
                    agent: request.agent.clone(),
                    isolated_worktree: false,
                    model: request.model.clone(),
                    share_worktree_of: None,
                    base_branch: None,
                },
                "HEAD",
            )?;
            let now = now_rfc3339();
            session.worktree = worktree.clone();
            session.scripts = crate::project_scripts::SessionScripts::new(true);
            session.messages = imported_messages(&session.id, &request.messages, &now).into();
            session.moved_from = Some(SessionMoveNote {
                computer: computer_label(&request.from_computer)?,
                branch: request.branch.clone(),
                at: now,
            });
            crate::transcript::arm_in_session_handoff(&mut session, request.agent.clone());
            if let (Some(start), Some(handoff)) = (&start, session.handoff.as_mut()) {
                handoff.brief = format!(
                    "{}\n\n{}",
                    offline_note(&request.from_computer, start),
                    handoff.brief
                );
            }
            if let Some(slot) = data.sessions.iter_mut().find(|item| item.id == session.id) {
                *slot = session.clone();
            }
            drop(data);
            state.persist()?;
            Ok::<_, Error>(session)
        })();
        match created {
            Ok(session) => {
                crate::transcript_view::emit(&app, &session);
                Ok(session)
            }
            Err(error) => {
                let _ = crate::worktree::remove(&root, &worktree, true);
                Err(error)
            }
        }
    })
    .await
}

/// What the agent learns first when a conversation continues here while its computer is off.
pub fn offline_note(from_computer: &str, start: &str) -> String {
    format!(
        "This conversation continued here while {} was off. The checkout is `{start}` as GitHub has it: \
         changes that were only on {} (uncommitted or not pushed) are not here. Check the files \
         before relying on earlier edits.",
        from_computer.trim(),
        from_computer.trim()
    )
}

/// Source side, after the target took the conversation: notes where it went.
pub fn mark_session_moved(
    app: AppHandle,
    state: State<Arc<AppState>>,
    session_id: String,
    target: String,
    branch: String,
) -> Result<()> {
    let computer = computer_label(&target)?;
    if !valid_move_branch(&branch) {
        return Err(Error::new("invalid", "not a move branch"));
    }
    let mut data = state.data.lock();
    let session = data
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    session.moved_to = Some(SessionMoveNote {
        computer,
        branch,
        at: now_rfc3339(),
    });
    crate::transcript_view::emit(&app, session);
    drop(data);
    state.persist()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: MessageRole, content: &str) -> Message {
        Message {
            id: Uuid::new_v4().to_string(),
            session_id: "s".into(),
            role,
            content: content.into(),
            created_at: "2026-10-13T10:00:00Z".into(),
            streaming: false,
            activity: None,
            steers: vec![],
            attachments: vec![],
            launched: vec![],
            visuals: vec![],
            client_request_id: None,
            rotation: None,
        }
    }

    fn request(messages: Vec<MovedMessage>) -> ImportSessionRequest {
        ImportSessionRequest {
            project_id: "p".into(),
            branch: "sirus/move-1a2b3c4d".into(),
            title: "Fix the login".into(),
            agent: AgentProviderId::Claude,
            model: Some("opus".into()),
            messages,
            from_computer: "Mac".into(),
            from_origin: None,
        }
    }

    fn moved(role: MessageRole, content: &str) -> MovedMessage {
        MovedMessage {
            role,
            content: content.into(),
            created_at: "2026-10-13T10:00:00Z".into(),
        }
    }

    #[test]
    fn move_branches_are_short_and_safe() {
        assert_eq!(
            move_branch("1A2B3C4D-aaaa-bbbb-cccc-000000000000"),
            "sirus/move-1a2b3c4d"
        );
        assert!(valid_move_branch("sirus/move-1a2b3c4d"));
        for refused in [
            "main",
            "sirus/move-",
            "sirus/move-../main",
            "sirus/move-a b",
            "sirus/move--x",
            "sirus/move-ABC",
            "sirus/move-a/b",
            "refs/heads/sirus/move-a",
        ] {
            assert!(!valid_move_branch(refused), "{refused}");
        }
    }

    #[test]
    fn exports_keep_finished_conversation_text_only() {
        let mut streaming = message(MessageRole::Agent, "half");
        streaming.streaming = true;
        let messages = vec![
            message(MessageRole::User, "hello"),
            message(MessageRole::System, "stderr noise"),
            message(MessageRole::Agent, "  "),
            message(MessageRole::Agent, "hi there"),
            streaming,
        ];
        let out = export_messages(&messages);
        assert_eq!(
            out.iter()
                .map(|item| item.content.as_str())
                .collect::<Vec<_>>(),
            ["hello", "hi there"]
        );
        // An export always passes the import's checks.
        assert!(validate_import(&request(out)).is_ok());
    }

    #[test]
    fn exports_are_bounded_from_the_most_recent() {
        let big = "é".repeat(MAX_MOVED_MESSAGE_BYTES);
        let messages: Vec<_> = (0..40).map(|_| message(MessageRole::User, &big)).collect();
        let out = export_messages(&messages);
        assert!(out
            .iter()
            .all(|item| item.content.len() <= MAX_MOVED_MESSAGE_BYTES));
        assert!(out.iter().map(|item| item.content.len()).sum::<usize>() <= MAX_MOVED_BYTES);
        assert!(validate_import(&request(out)).is_ok());
        let many: Vec<_> = (0..MAX_MOVED_MESSAGES + 5)
            .map(|index| message(MessageRole::User, &index.to_string()))
            .collect();
        let out = export_messages(&many);
        assert_eq!(out.len(), MAX_MOVED_MESSAGES);
        assert_eq!(
            out.last().unwrap().content,
            (MAX_MOVED_MESSAGES + 4).to_string()
        );
    }

    #[test]
    fn imports_are_validated_strictly() {
        assert!(validate_import(&request(vec![moved(MessageRole::User, "hi")])).is_ok());
        let mut bad = request(vec![]);
        bad.branch = "main".into();
        assert!(validate_import(&bad).is_err());
        let mut bad = request(vec![]);
        bad.title = " ".into();
        assert!(validate_import(&bad).is_err());
        let mut bad = request(vec![]);
        bad.from_computer = "Mac\nmini".into();
        assert!(validate_import(&bad).is_err());
        let mut bad = request(vec![]);
        bad.model = Some("x\u{7}".into());
        assert!(validate_import(&bad).is_err());
        assert!(validate_import(&request(vec![moved(MessageRole::System, "x")])).is_err());
        assert!(validate_import(&request(vec![moved(MessageRole::User, "a\0b")])).is_err());
        let huge = "x".repeat(MAX_MOVED_MESSAGE_BYTES + 1);
        assert!(validate_import(&request(vec![moved(MessageRole::User, &huge)])).is_err());
        let big = "x".repeat(MAX_MOVED_MESSAGE_BYTES);
        let total =
            vec![moved(MessageRole::User, &big); MAX_MOVED_BYTES / MAX_MOVED_MESSAGE_BYTES + 1];
        assert!(validate_import(&request(total)).is_err());
        let count = vec![moved(MessageRole::User, "x"); MAX_MOVED_MESSAGES + 1];
        assert!(validate_import(&request(count)).is_err());
        // Unknown fields are refused, too.
        let extra = serde_json::json!({
            "projectId": "p", "branch": "sirus/move-a", "title": "t", "agent": "claude",
            "messages": [], "fromComputer": "Mac", "worktreePath": "/etc"
        });
        assert!(serde_json::from_value::<ImportSessionRequest>(extra).is_err());
        let extra_message = serde_json::json!({ "role": "user", "content": "x", "createdAt": "", "attachments": [] });
        assert!(serde_json::from_value::<MovedMessage>(extra_message).is_err());
    }

    #[test]
    fn imported_messages_belong_to_the_new_session_and_arm_the_recap() {
        let list = imported_messages(
            "new",
            &[
                moved(MessageRole::User, "fix login"),
                MovedMessage {
                    role: MessageRole::Agent,
                    content: "done".into(),
                    created_at: "yesterday".into(),
                },
            ],
            "2026-10-13T12:00:00Z",
        );
        assert!(list
            .iter()
            .all(|item| item.session_id == "new" && !item.streaming));
        assert_eq!(list[0].created_at, "2026-10-13T10:00:00Z");
        assert_eq!(list[1].created_at, "2026-10-13T12:00:00Z");
        assert_ne!(list[0].id, list[1].id);
    }

    #[test]
    fn moved_work_travels_on_a_branch_without_touching_the_source_checkout() {
        use crate::git::{self, tests::Repo};
        let origin = Repo::new();
        let mac = crate::worktree::tests::clone_of(&origin);
        std::fs::write(mac.join("file.txt"), "changed on the mac\n").unwrap();
        std::fs::write(mac.join("new.txt"), "untracked\n").unwrap();
        let head = git::run_ok(&mac, &["rev-parse", "HEAD"]).unwrap();
        let scratch = origin.0.join("scratch");
        let commit = crate::team::snapshot_commit(&mac, &scratch, "Move").unwrap();
        let branch = move_branch("1a2b3c4d-0000");
        git::push_commit_to_branch(&mac, &commit, &branch).unwrap();
        // The Mac's checkout, branch and staging area are as they were.
        assert_eq!(git::run_ok(&mac, &["rev-parse", "HEAD"]).unwrap(), head);
        let status = git::run_ok(&mac, &["status", "--porcelain"]).unwrap();
        assert!(
            status.contains("M file.txt")
                && !status.contains("MM")
                && status.contains("?? new.txt"),
            "{status}"
        );
        assert!(git::push_commit_to_branch(&mac, "not-a-commit", &branch).is_err());

        // The other computer checks the branch out in a new worktree, tracking origin.
        let server = origin.0.join("server");
        git::run_ok(
            &origin.0,
            &[
                "clone",
                "-q",
                &crate::paths::display_path(&origin.cwd()),
                &crate::paths::display_path(&server),
            ],
        )
        .unwrap();
        git::fetch_origin_branch(&server, &branch).unwrap();
        let parent = origin.0.join("worktrees");
        let tree =
            crate::worktree::create_from_origin_branch(&server, &parent, "Fix login", &branch)
                .unwrap();
        assert!(tree.isolated);
        assert_eq!(tree.branch, branch);
        let path = std::path::PathBuf::from(&tree.path);
        assert_eq!(
            std::fs::read_to_string(path.join("file.txt")).unwrap(),
            "changed on the mac\n"
        );
        assert!(path.join("new.txt").exists());
        let upstream = git::run_ok(&path, &["rev-parse", "--abbrev-ref", "@{upstream}"]).unwrap();
        assert_eq!(upstream, format!("origin/{branch}"));
        // Moving the same conversation here twice is refused instead of clobbering it.
        assert!(
            crate::worktree::create_from_origin_branch(&server, &parent, "Fix login", &branch)
                .is_err()
        );
    }

    #[test]
    fn continuing_while_the_source_is_off_starts_from_origin_without_tracking_it() {
        use crate::git::{self, tests::Repo};
        let origin = Repo::new();
        let server = origin.0.join("server");
        git::run_ok(
            &origin.0,
            &[
                "clone",
                "-q",
                &crate::paths::display_path(&origin.cwd()),
                &crate::paths::display_path(&server),
            ],
        )
        .unwrap();
        // The Mac's branch was never pushed: the fetch fails and the default branch is used.
        assert!(git::fetch_origin_branch(&server, "feature/only-on-mac").is_err());
        let start = git::origin_default_branch(&server).unwrap();
        git::fetch_origin_branch(&server, &start).unwrap();
        let branch = move_branch("9f8e7d6c-0000");
        let parent = origin.0.join("worktrees");
        let tree = crate::worktree::create_from_origin_start(
            &server,
            &parent,
            "Fix login",
            &branch,
            &start,
        )
        .unwrap();
        assert_eq!(tree.branch, branch);
        let path = std::path::PathBuf::from(&tree.path);
        assert_eq!(
            git::run_ok(&path, &["rev-parse", "HEAD"]).unwrap(),
            git::run_ok(&server, &["rev-parse", &format!("origin/{start}")]).unwrap()
        );
        // Pushes never land on the default branch by accident.
        assert!(git::run_ok(&path, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_err());
        assert!(offline_note("Mac", &start).contains(&format!("`{start}`")));
    }

    #[test]
    fn computer_labels_are_one_bounded_line() {
        assert_eq!(computer_label("  sirus-vps ").unwrap(), "sirus-vps");
        assert!(computer_label("").is_err());
        assert!(computer_label(&"x".repeat(61)).is_err());
        assert!(computer_label("a\tb").is_err());
    }
}
