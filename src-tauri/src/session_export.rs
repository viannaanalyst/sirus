//! Conversation export (ADR-060): the renderer formats an owned session's
//! conversation as Markdown; native code checks the session exists and writes
//! the text only to a path the person picks in the native save dialog.
use std::sync::Arc;

use crate::runtime::{AppHandle, State};

use crate::commands::AppState;
use crate::error::{Error, Result};

/// Largest exported conversation.
const MAX_MARKDOWN: usize = 8 * 1024 * 1024;

/// Returns `false` when the person cancelled the save dialog.
pub async fn export_conversation(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    markdown: String,
) -> Result<bool> {
    if markdown.len() > MAX_MARKDOWN || markdown.contains('\0') {
        return Err(Error::new(
            "invalid",
            "The conversation is too large to export.",
        ));
    }
    let title = {
        state.ensure_running()?;
        let data = state.data.lock();
        data.sessions
            .iter()
            .find(|session| session.id == session_id)
            .map(|session| session.title.clone())
            .ok_or_else(|| Error::not_found("session not found"))?
    };
    let Some(path) = pick_destination(&app, &file_name(&title)).await else {
        return Ok(false);
    };
    tokio::task::spawn_blocking(move || write(&path, &markdown))
        .await
        .map_err(|_| Error::new("persist", "Could not save the conversation."))??;
    Ok(true)
}

/// A safe default file name from the session title.
fn file_name(title: &str) -> String {
    let stem: String = title
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .take(80)
        .collect();
    let stem = stem.trim().trim_matches('.').trim();
    format!(
        "{}.md",
        if stem.is_empty() {
            "conversation"
        } else {
            stem
        }
    )
}

/// The headless host has no screen to show a picker on (ADR-131).
async fn pick_destination(_app: &AppHandle, _name: &str) -> Option<std::path::PathBuf> {
    None
}

/// Writes through a temporary sibling and renames, so a failed write never truncates an existing file.
fn write(path: &std::path::Path, markdown: &str) -> Result<()> {
    let path = if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
    {
        path.to_path_buf()
    } else {
        path.with_extension("md")
    };
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("persist", "Choose a folder to save into."))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| Error::new("persist", "Could not save the conversation."))?;
    std::io::Write::write_all(&mut file, markdown.as_bytes())
        .map_err(|_| Error::new("persist", "Could not save the conversation."))?;
    // A temporary file is private (0600); an exported document is an ordinary file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o644));
    }
    file.persist(&path)
        .map_err(|_| Error::new("persist", "Could not save the conversation."))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe_and_markdown() {
        assert_eq!(
            file_name("Fix login: rate limit"),
            "Fix login- rate limit.md"
        );
        assert_eq!(file_name("../../etc/passwd"), "-..-etc-passwd.md");
        assert_eq!(file_name("..."), "conversation.md");
        assert!(file_name(&"x".repeat(300)).len() <= 83);
    }

    #[test]
    fn writes_replace_through_a_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("chat");
        write(&target, "# One").unwrap();
        write(&dir.path().join("chat.md"), "# Two").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("chat.md")).unwrap(),
            "# Two"
        );
        assert!(!target.exists());
    }
}
