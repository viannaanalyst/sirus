//! Undo a settled turn's file changes (ADR-061). Each retained turn diff is
//! applied in reverse with `git apply -R`, one file at a time, after a
//! `--check`: a file changed again afterwards in the same lines is refused and
//! left untouched, never overwritten. The index, commits and other files stay
//! as they are. Paths come only from the native review, never from the renderer.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::runtime::{AppHandle, State};
use serde::{Deserialize, Serialize};

use crate::commands::{session_cwd, AppState};
use crate::error::{Error, Result};
use crate::models::{ChangeKind, MessageRole, SessionStatus};
use crate::turn_review::{allowed_name, ReviewFile};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UndoRequest {
    pub session_id: String,
    pub message_id: String,
    /// Files of that turn to undo; `None` undoes every file that can be.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
    pub confirm: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UndoOutcome {
    pub undone: Vec<String>,
    /// Files left as they are: changed again since the turn, or no retained diff.
    pub refused: Vec<String>,
}

/// Why a turn cannot be undone right now (ADR-187). Everything here is read-only and local.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UndoBlock {
    /// Git `HEAD` moved since the turn (commit, checkout, rebase, pull).
    HeadMoved { was: String, now: String },
    /// Another session in the same working copy is running, or has unkept changes to these files.
    OtherSession {
        session_id: String,
        title: String,
        running: bool,
        files: Vec<String>,
    },
    /// These files changed again after the turn, in the same lines: by a later reply of this
    /// session (`later_turn`) or by the person.
    ChangedAgain {
        files: Vec<String>,
        later_turn: bool,
    },
}

impl UndoBlock {
    pub fn message(&self) -> String {
        match self {
            Self::HeadMoved { was, now } => format!(
                "Git HEAD moved since this turn ({} to {}): a commit, checkout, rebase or pull happened. Nothing was undone. Open the turn's diff and revert it by hand, or use git revert on the new commit.",
                short(was),
                short(now)
            ),
            Self::OtherSession {
                title,
                running: true,
                ..
            } => format!(
                "\"{title}\" is working in this folder right now. Nothing was undone: wait for it to finish, so its edits are not overwritten. Open it from the session list."
            ),
            Self::OtherSession { title, files, .. } => format!(
                "\"{title}\" has unkept changes to {} in this folder. Nothing was undone: keep or undo them in that session first. Open it from the session list.",
                list(files)
            ),
            Self::ChangedAgain {
                files,
                later_turn: true,
            } => format!(
                "{} changed again in a later reply of this session. Nothing was undone: undo that reply first, or revert by hand from the diff.",
                list(files)
            ),
            Self::ChangedAgain { files, .. } => format!(
                "{} was edited after this turn, in the same lines. Nothing was undone: revert by hand from the diff.",
                list(files)
            ),
        }
    }
}

fn short(id: &str) -> &str {
    id.get(..7).unwrap_or(id)
}

fn list(files: &[String]) -> String {
    match files {
        [] => "files".to_owned(),
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} and {} more", rest.len()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckRequest {
    pub session_id: String,
    pub message_id: String,
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

/// Read-only: the reason undo would be refused, or `None`. The UI asks before the person confirms.
pub async fn check_turn_undo(
    state: State<'_, Arc<AppState>>,
    request: CheckRequest,
) -> Result<Option<UndoBlock>> {
    let state = state.inner().clone();
    let request = UndoRequest {
        session_id: request.session_id,
        message_id: request.message_id,
        paths: request.paths,
        confirm: true,
    };
    crate::commands::native_task(move || Ok(plan(&state, &request)?.block)).await
}

pub async fn undo_turn_changes(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    request: UndoRequest,
) -> Result<UndoOutcome> {
    if !request.confirm {
        return Err(Error::confirmation_required(
            "Confirm undoing this turn's changes first.",
        ));
    }
    let state = state.inner().clone();
    let worker = state.clone();
    let (session_id, message_id) = (request.session_id.clone(), request.message_id.clone());
    let outcome = crate::commands::native_task(move || {
        let planned = plan(&worker, &request)?;
        if let Some(block) = planned.block {
            return Err(Error::new("invalid", block.message()));
        }
        let outcome = undo_files(&planned.cwd, planned.files);
        record(&worker, &request, &outcome.undone)?;
        Ok(outcome)
    })
    .await?;
    let data = state.data_with_messages(&[&session_id]);
    if let Some(session) = data.sessions.iter().find(|s| s.id == session_id) {
        let from = session
            .messages
            .iter()
            .position(|message| message.id == message_id)
            .unwrap_or_else(|| crate::transcript_view::current_turn(session));
        crate::transcript_view::emit_from(&app, session, from);
    }
    Ok(outcome)
}

struct Planned {
    cwd: PathBuf,
    files: Vec<ReviewFile>,
    block: Option<UndoBlock>,
}

/// The owned, idle session's workspace, the requested files of a settled turn that are not
/// undone yet, and the reason to refuse, if any.
fn plan(state: &AppState, request: &UndoRequest) -> Result<Planned> {
    state.ensure_running()?;
    // Sessions sharing this working copy need their transcripts loaded to read their reviews.
    let mut ids = vec![request.session_id.clone()];
    {
        let data = state.data_with_messages(&[&request.session_id]);
        let session = data
            .sessions
            .iter()
            .find(|s| s.id == request.session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        let cwd = session_cwd(&data, session)?;
        ids.extend(neighbours(&data.sessions, &session.id, &cwd).map(|other| other.id.clone()));
    }
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let data = state.data_with_messages(&ids);
    let session = data
        .sessions
        .iter()
        .find(|s| s.id == request.session_id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    if is_active(&session.status) {
        return Err(Error::new(
            "invalid",
            "Wait for the agent to finish before undoing changes.",
        ));
    }
    let review = session
        .messages
        .iter()
        .find(|m| {
            m.id == request.message_id
                && m.session_id == session.id
                && m.role == MessageRole::Agent
                && !m.streaming
        })
        .and_then(|m| m.activity.as_ref())
        .filter(|activity| activity.ended_at.is_some())
        .and_then(|activity| activity.review.as_ref())
        .ok_or_else(|| Error::not_found("turn review not available"))?;
    if let Some(paths) = &request.paths {
        if paths.is_empty()
            || paths.len() > review.files.len()
            || paths
                .iter()
                .any(|path| !review.files.iter().any(|file| &file.path == path))
        {
            return Err(Error::new(
                "invalid",
                "Those files are not part of this turn.",
            ));
        }
    }
    let files: Vec<ReviewFile> = review
        .files
        .iter()
        // Committed files left the review (ADR-108); undoing them would rewrite history's content.
        .filter(|file| file.undone_at.is_none() && file.committed_at.is_none())
        .filter(|file| {
            request
                .paths
                .as_ref()
                .is_none_or(|paths| paths.contains(&file.path))
        })
        .cloned()
        .collect();
    let cwd = session_cwd(&data, session)?;
    let block = find_block(
        &data.sessions,
        session,
        &request.message_id,
        review,
        &cwd,
        &files,
    );
    Ok(Planned { cwd, files, block })
}

/// Sessions whose agent folder is the same working copy (isolated worktrees have their own path).
fn neighbours<'a>(
    sessions: &'a [crate::models::Session],
    own: &'a str,
    cwd: &Path,
) -> impl Iterator<Item = &'a crate::models::Session> {
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    sessions.iter().filter(move |other| {
        other.id != own && std::fs::canonicalize(&other.worktree.path).is_ok_and(|path| path == cwd)
    })
}

fn is_active(status: &SessionStatus) -> bool {
    matches!(
        status,
        SessionStatus::Starting | SessionStatus::Running | SessionStatus::Waiting
    )
}

/// Files of a session's turns that are still waiting for the person (not kept, undone or committed).
fn unkept_paths(session: &crate::models::Session) -> BTreeSet<&str> {
    session
        .messages
        .iter()
        .filter_map(|m| m.activity.as_ref()?.review.as_ref())
        .filter(|review| review.kept_at.is_none())
        .flat_map(|review| &review.files)
        .filter(|file| file.undone_at.is_none() && file.committed_at.is_none())
        .map(|file| file.path.as_str())
        .collect()
}

/// The first reason to refuse, in order: moved `HEAD`, another session, edits after the turn.
fn find_block(
    sessions: &[crate::models::Session],
    session: &crate::models::Session,
    message_id: &str,
    review: &crate::turn_review::TurnReview,
    cwd: &Path,
    files: &[ReviewFile],
) -> Option<UndoBlock> {
    if files.is_empty() {
        return None;
    }
    if let Some(was) = &review.head {
        if let Some(now) = crate::turn_review::head_of(cwd).filter(|now| now != was) {
            return Some(UndoBlock::HeadMoved {
                was: was.clone(),
                now,
            });
        }
    }
    let paths: BTreeSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
    for other in neighbours(sessions, &session.id, cwd) {
        let running = is_active(&other.status);
        let touched: Vec<String> = unkept_paths(other)
            .intersection(&paths)
            .map(|path| (*path).to_owned())
            .collect();
        if running || !touched.is_empty() {
            return Some(UndoBlock::OtherSession {
                session_id: other.id.clone(),
                title: other.title.clone(),
                running,
                files: touched,
            });
        }
    }
    let later: BTreeSet<&str> = session
        .messages
        .iter()
        .skip_while(|m| m.id != message_id)
        .skip(1)
        .filter_map(|m| m.activity.as_ref()?.review.as_ref())
        .flat_map(|review| &review.files)
        .filter(|file| file.undone_at.is_none())
        .map(|file| file.path.as_str())
        .collect();
    let conflicts = conflicts(cwd, files);
    (!conflicts.is_empty()).then(|| UndoBlock::ChangedAgain {
        later_turn: conflicts.iter().any(|path| later.contains(path.as_str())),
        files: conflicts,
    })
}

/// Files whose reverse patch no longer applies cleanly (a dry run, read-only).
fn conflicts(cwd: &Path, files: &[ReviewFile]) -> Vec<String> {
    let prefix = repo_prefix(cwd);
    files
        .iter()
        .filter_map(|file| Some((file, patch_for(file)?)))
        .filter(|(_, patch)| !apply(cwd, prefix.as_deref(), patch, true))
        .map(|(file, _)| file.path.clone())
        .collect()
}

/// The reverse patch for one file, when it has a retained text diff and a safe name.
fn patch_for(file: &ReviewFile) -> Option<String> {
    (!file.binary && allowed_name(&file.path))
        .then(|| file.diff.as_deref().and_then(|diff| patch(file, diff)))
        .flatten()
}

/// All or nothing: every patch is checked first, and a single conflict leaves every file as it
/// is. Files without a usable diff (binary, expired) are refused on their own.
fn undo_files(cwd: &Path, files: Vec<ReviewFile>) -> UndoOutcome {
    let prefix = repo_prefix(cwd);
    let mut outcome = UndoOutcome {
        undone: vec![],
        refused: vec![],
    };
    let mut ready = vec![];
    for file in files {
        match patch_for(&file) {
            Some(patch) => ready.push((file.path, patch)),
            None => outcome.refused.push(file.path),
        }
    }
    if !ready
        .iter()
        .all(|(_, patch)| apply(cwd, prefix.as_deref(), patch, true))
    {
        outcome
            .refused
            .extend(ready.into_iter().map(|(path, _)| path));
        return outcome;
    }
    for (path, patch) in ready {
        if apply(cwd, prefix.as_deref(), &patch, false) {
            outcome.undone.push(path);
        } else {
            outcome.refused.push(path);
        }
    }
    outcome
}

/// Inside a repository `git apply` reads patch paths from the repository root;
/// the review's paths are relative to the session workspace.
fn repo_prefix(cwd: &Path) -> Option<String> {
    let output = crate::git::bounded_probe(cwd, &["rev-parse", "--show-prefix"]).ok()?;
    let prefix = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (output.status.success() && !prefix.is_empty()).then_some(prefix)
}

/// A git patch for one reviewed file: its retained hunks under headers that say
/// whether the turn created, deleted or changed it.
fn patch(file: &ReviewFile, diff: &str) -> Option<String> {
    // Names Git would quote (quotes, backslashes) are left as they are.
    if file.path.contains(['"', '\\']) {
        return None;
    }
    let hunks = &diff[diff.find("\n@@")? + 1..];
    let name = &file.path;
    let header = match file.kind {
        ChangeKind::Added => {
            format!(
                "diff --git a/{name} b/{name}\nnew file mode 100644\n--- /dev/null\n+++ b/{name}\n"
            )
        }
        ChangeKind::Deleted => format!(
            "diff --git a/{name} b/{name}\ndeleted file mode 100644\n--- a/{name}\n+++ /dev/null\n"
        ),
        _ => format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n"),
    };
    Some(format!(
        "{header}{hunks}{}",
        if hunks.ends_with('\n') { "" } else { "\n" }
    ))
}

/// `git apply -R` on the working tree only; `check` makes it a dry run.
fn apply(cwd: &Path, prefix: Option<&str>, patch: &str, check: bool) -> bool {
    let directory = prefix.map(|prefix| format!("--directory={prefix}"));
    let mut args = vec!["apply", "-R", "--whitespace=nowarn"];
    if check {
        args.push("--check");
    }
    if let Some(directory) = &directory {
        args.push(directory);
    }
    args.push("-");
    crate::git::run_input(cwd, &args, patch.as_bytes()).is_ok_and(|output| output.status.success())
}

/// Marks undone files on the same settled turn and persists.
fn record(state: &AppState, request: &UndoRequest, undone: &[String]) -> Result<()> {
    if undone.is_empty() {
        return Ok(());
    }
    let mut data = state.data_with_messages(&[&request.session_id]);
    let now = crate::paths::now_rfc3339();
    if let Some(review) = data
        .sessions
        .iter_mut()
        .find(|s| s.id == request.session_id)
        .and_then(|session| {
            session
                .messages
                .iter_mut()
                .find(|m| m.id == request.message_id)
        })
        .and_then(|m| m.activity.as_mut()?.review.as_mut())
    {
        for file in &mut review.files {
            if undone.contains(&file.path) {
                file.undone_at.get_or_insert_with(|| now.clone());
            }
        }
    }
    crate::persist::save(&state.data_path, &data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, kind: ChangeKind, diff: &str) -> ReviewFile {
        ReviewFile {
            diff_omitted: false,
            path: path.into(),
            kind,
            additions: 0,
            deletions: 0,
            binary: false,
            diff: Some(diff.into()),
            undone_at: None,
            committed_at: None,
        }
    }

    fn git(cwd: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap()
            .status
            .success());
    }

    /// A real repository with `app/edit.txt` committed as the turn left it.
    fn repo() -> (tempfile::TempDir, PathBuf) {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path().to_path_buf();
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.email", "t@example.com"]);
        git(&root, &["config", "user.name", "T"]);
        std::fs::write(root.join("edit.txt"), "a\nB\nc\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "one"]);
        (repo, root)
    }

    fn edit_file() -> ReviewFile {
        file(
            "edit.txt",
            ChangeKind::Modified,
            "--- a/edit.txt\n+++ b/edit.txt\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n",
        )
    }

    fn review(
        head: Option<String>,
        files: Vec<ReviewFile>,
        kept: bool,
    ) -> crate::turn_review::TurnReview {
        crate::turn_review::TurnReview {
            files,
            partial: false,
            shared_workspace: true,
            kept_at: kept.then(|| "time".to_owned()),
            expired: false,
            head,
        }
    }

    /// A session whose agent replies are `(message id, review)`.
    fn session(
        id: &str,
        path: &Path,
        status: SessionStatus,
        replies: Vec<(&str, crate::turn_review::TurnReview)>,
    ) -> crate::models::Session {
        let mut session: crate::models::Session = serde_json::from_value(serde_json::json!({
            "id": id, "title": format!("Session {id}"), "projectId": "project", "agent": "codex", "status": "idle",
            "createdAt": "2026-10-09T00:00:00Z", "lastActivityAt": "2026-10-09T00:00:00Z",
            "worktree": {"path": path, "branch": "main", "isolated": false}, "lastError": null,
            "messages": replies.iter().map(|(message, _)| serde_json::json!({
                "id": message, "sessionId": id, "role": "agent", "content": "Done", "createdAt": "time", "streaming": false
            })).collect::<Vec<_>>()
        }))
        .unwrap();
        session.status = status;
        for (message, (_, review)) in session.messages.iter_mut().zip(replies) {
            let mut activity =
                crate::activity::TurnActivity::new(crate::models::AgentProviderId::Codex, None);
            activity.review = Some(review);
            message.activity = Some(activity);
        }
        session
    }

    /// The block for undoing `files` of the reply `m1` of session `own`.
    fn block(sessions: &[crate::models::Session], cwd: &Path) -> Option<UndoBlock> {
        let own = &sessions[0];
        let review = own.messages[0]
            .activity
            .as_ref()
            .unwrap()
            .review
            .as_ref()
            .unwrap();
        find_block(sessions, own, "m1", review, cwd, &review.files)
    }

    #[test]
    fn an_untouched_turn_in_an_unmoved_repository_is_allowed() {
        let (_repo, root) = repo();
        let head = crate::turn_review::head_of(&root);
        assert!(head.is_some());
        let own = session(
            "own",
            &root,
            SessionStatus::Completed,
            vec![("m1", review(head, vec![edit_file()], false))],
        );
        assert_eq!(block(&[own], &root), None);
    }

    #[test]
    fn a_moved_head_refuses_with_a_hint_and_changes_nothing() {
        let (_repo, root) = repo();
        let head = crate::turn_review::head_of(&root);
        std::fs::write(root.join("other.txt"), "x\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "two"]);
        let own = session(
            "own",
            &root,
            SessionStatus::Completed,
            vec![("m1", review(head.clone(), vec![edit_file()], false))],
        );
        let found = block(&[own], &root).unwrap();
        assert!(
            matches!(&found, UndoBlock::HeadMoved { was, now } if Some(was) == head.as_ref() && now != was)
        );
        assert!(found.message().contains("revert it by hand"));
        assert_eq!(
            std::fs::read_to_string(root.join("edit.txt")).unwrap(),
            "a\nB\nc\n"
        );
    }

    #[test]
    fn turns_without_a_recorded_head_are_not_blocked_by_it() {
        let (_repo, root) = repo();
        let own = session(
            "own",
            &root,
            SessionStatus::Completed,
            vec![("m1", review(None, vec![edit_file()], false))],
        );
        assert_eq!(block(&[own], &root), None);
    }

    #[test]
    fn another_session_in_the_same_working_copy_blocks_when_running_or_unkept() {
        let (_repo, root) = repo();
        let elsewhere = tempfile::tempdir().unwrap();
        let head = crate::turn_review::head_of(&root);
        let own = || {
            session(
                "own",
                &root,
                SessionStatus::Completed,
                vec![("m1", review(head.clone(), vec![edit_file()], false))],
            )
        };
        // Running in the same folder: refused by name.
        let running = session("busy", &root, SessionStatus::Running, vec![]);
        let found = block(&[own(), running], &root).unwrap();
        assert!(
            matches!(&found, UndoBlock::OtherSession { session_id, running: true, .. } if session_id == "busy")
        );
        assert!(found.message().contains("Session busy"));
        // Idle with unkept changes to the same file.
        let unkept = session(
            "unkept",
            &root,
            SessionStatus::Completed,
            vec![("x", review(None, vec![edit_file()], false))],
        );
        let found = block(&[own(), unkept], &root).unwrap();
        assert!(
            matches!(&found, UndoBlock::OtherSession { running: false, files, .. } if files == &["edit.txt"])
        );
        // Kept changes, other files, and sessions in another folder do not conflict.
        let mut other_file = edit_file();
        other_file.path = "unrelated.txt".into();
        let sessions = vec![
            own(),
            session(
                "kept",
                &root,
                SessionStatus::Completed,
                vec![("x", review(None, vec![edit_file()], true))],
            ),
            session(
                "disjoint",
                &root,
                SessionStatus::Completed,
                vec![("x", review(None, vec![other_file], false))],
            ),
            session(
                "isolated",
                elsewhere.path(),
                SessionStatus::Running,
                vec![("x", review(None, vec![edit_file()], false))],
            ),
        ];
        assert_eq!(block(&sessions, &root), None);
    }

    #[test]
    fn later_edits_by_the_person_or_a_later_turn_refuse_by_name() {
        let (_repo, root) = repo();
        let head = crate::turn_review::head_of(&root);
        std::fs::write(root.join("edit.txt"), "a\nPERSON\nc\n").unwrap();
        let own = || {
            session(
                "own",
                &root,
                SessionStatus::Completed,
                vec![("m1", review(head.clone(), vec![edit_file()], false))],
            )
        };
        let found = block(&[own()], &root).unwrap();
        assert_eq!(
            found,
            UndoBlock::ChangedAgain {
                files: vec!["edit.txt".into()],
                later_turn: false
            }
        );
        assert!(found
            .message()
            .contains("edit.txt was edited after this turn"));
        // The same file was reported by a later reply of this session.
        let mut later = own();
        let mut next = later.messages[0].clone();
        next.id = "m2".into();
        later.messages.push(next);
        let found = block(&[later], &root).unwrap();
        assert!(matches!(
            found,
            UndoBlock::ChangedAgain {
                later_turn: true,
                ..
            }
        ));
        assert_eq!(
            std::fs::read_to_string(root.join("edit.txt")).unwrap(),
            "a\nPERSON\nc\n"
        );
    }

    #[test]
    fn undo_is_all_or_nothing_and_keeps_later_edits() {
        let repo = tempfile::tempdir().unwrap();
        let root = repo.path();
        git(root, &["init", "-q"]);
        let cwd = root.join("app");
        std::fs::create_dir(&cwd).unwrap();
        std::fs::write(cwd.join("edit.txt"), "a\nB\nc\n").unwrap();
        std::fs::write(cwd.join("new file.txt"), "fresh\n").unwrap();
        std::fs::write(cwd.join("later.txt"), "x\nPERSON\nz\n").unwrap();
        let files = vec![
            file(
                "edit.txt",
                ChangeKind::Modified,
                "--- a/edit.txt\n+++ b/edit.txt\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n",
            ),
            file(
                "new file.txt",
                ChangeKind::Added,
                "--- a/new file.txt\n+++ b/new file.txt\n@@ -0,0 +1 @@\n+fresh\n",
            ),
            file(
                "gone.txt",
                ChangeKind::Deleted,
                "--- a/gone.txt\n+++ b/gone.txt\n@@ -1,2 +0,0 @@\n-one\n-two\n",
            ),
            // The person edited the same line after the turn: refused, left as is.
            file(
                "later.txt",
                ChangeKind::Modified,
                "--- a/later.txt\n+++ b/later.txt\n@@ -1,3 +1,3 @@\n x\n-y\n+AGENT\n z\n",
            ),
            file(
                "../outside.txt",
                ChangeKind::Modified,
                "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n",
            ),
        ];
        // One conflict (later.txt) and one unsafe name: nothing at all is changed.
        let outcome = undo_files(&cwd, files.clone());
        assert!(outcome.undone.is_empty());
        assert_eq!(
            outcome.refused,
            [
                "../outside.txt",
                "edit.txt",
                "new file.txt",
                "gone.txt",
                "later.txt"
            ]
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("edit.txt")).unwrap(),
            "a\nB\nc\n"
        );
        assert!(cwd.join("new file.txt").exists());
        assert!(!cwd.join("gone.txt").exists());
        // Without the conflicting file, the rest goes back together.
        let files: Vec<_> = files
            .into_iter()
            .filter(|f| f.path != "later.txt")
            .collect();
        let outcome = undo_files(&cwd, files);
        assert_eq!(outcome.undone, ["edit.txt", "new file.txt", "gone.txt"]);
        assert_eq!(outcome.refused, ["../outside.txt"]);
        assert_eq!(
            std::fs::read_to_string(cwd.join("edit.txt")).unwrap(),
            "a\nb\nc\n"
        );
        assert!(!cwd.join("new file.txt").exists());
        assert_eq!(
            std::fs::read_to_string(cwd.join("gone.txt")).unwrap(),
            "one\ntwo\n"
        );
        assert_eq!(
            std::fs::read_to_string(cwd.join("later.txt")).unwrap(),
            "x\nPERSON\nz\n"
        );
    }

    #[test]
    fn workspaces_outside_git_and_missing_diffs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "new\n").unwrap();
        let mut binary = file("bin", ChangeKind::Modified, "");
        binary.binary = true;
        let outcome = undo_files(
            dir.path(),
            vec![
                file(
                    "f.txt",
                    ChangeKind::Modified,
                    "--- a/f.txt\n+++ b/f.txt\n@@ -1 +1 @@\n-old\n+new\n",
                ),
                binary,
            ],
        );
        assert_eq!(outcome.undone, ["f.txt"]);
        assert_eq!(outcome.refused, ["bin"]);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
            "old\n"
        );
    }

    #[test]
    fn requests_are_closed_and_confirmed() {
        assert!(serde_json::from_value::<UndoRequest>(
            serde_json::json!({"sessionId":"s","messageId":"m","confirm":true})
        )
        .is_ok());
        assert!(serde_json::from_value::<UndoRequest>(
            serde_json::json!({"sessionId":"s","messageId":"m","confirm":true,"cwd":"/"})
        )
        .is_err());
    }
}
