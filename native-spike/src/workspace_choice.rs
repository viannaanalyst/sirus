//! Where a new conversation works: the project's folder, a new worktree, or one that exists.
//! The pure part (labels and the `create_session` request); the menu lives in `shell.rs`.

use serde_json::{Value, json};

use crate::i18n::t;

/// What the user picks under the composer.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// The project's own folder.
    Folder,
    /// A worktree made for this conversation.
    NewWorktree,
    /// A worktree another session already works in.
    Existing { session: String },
}

/// The `create_session` request for a choice.
pub fn request(choice: &Choice, project_id: &str, agent: &str, model: Option<&str>) -> Value {
    let mut request = json!({ "projectId": project_id, "agent": agent, "model": model });
    match choice {
        Choice::Folder => request["isolatedWorktree"] = json!(false),
        Choice::NewWorktree => request["isolatedWorktree"] = json!(true),
        Choice::Existing { session } => {
            request["isolatedWorktree"] = json!(true);
            request["shareWorktreeOf"] = json!(session);
        }
    }
    request
}

/// The folder's last component, a worktree's name.
pub fn folder_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// The chip's main label: "Pasta do projeto" or "Worktree · name".
pub fn label(isolated: bool, worktree_path: &str) -> String {
    if isolated {
        format!("Worktree · {}", folder_name(worktree_path))
    } else {
        t("Pasta do projeto").to_owned()
    }
}

/// The muted branch beside it, empty when unknown.
pub fn branch_label(branch: &str) -> String {
    branch
        .strip_prefix("refs/heads/")
        .unwrap_or(branch)
        .to_owned()
}

/// Whether leaving this checkout may delete its worktree: made for sessions and used by no other.
pub fn removable(isolated: bool, others_in_it: usize) -> bool {
    isolated && others_in_it == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_request_is_plain() {
        let r = request(&Choice::Folder, "p1", "claude", None);
        assert_eq!(r["isolatedWorktree"], false);
        assert!(r.get("shareWorktreeOf").is_none());
        assert_eq!(r["projectId"], "p1");
        assert!(r["model"].is_null());
    }

    #[test]
    fn new_worktree_request_is_isolated() {
        let r = request(&Choice::NewWorktree, "p1", "codex", Some("gpt"));
        assert_eq!(r["isolatedWorktree"], true);
        assert!(r.get("shareWorktreeOf").is_none());
        assert_eq!(r["model"], "gpt");
        assert_eq!(r["agent"], "codex");
    }

    #[test]
    fn existing_request_shares_the_session() {
        let choice = Choice::Existing {
            session: "s9".into(),
        };
        let r = request(&choice, "p1", "claude", None);
        assert_eq!(r["isolatedWorktree"], true);
        assert_eq!(r["shareWorktreeOf"], "s9");
    }

    #[test]
    fn labels_for_each_mode() {
        assert_eq!(label(true, "/w/p/fix-bug/"), "Worktree · fix-bug");
        assert_eq!(branch_label("refs/heads/main"), "main");
        assert_eq!(branch_label("feat/x"), "feat/x");
        assert_eq!(branch_label(""), "");
        assert!(!label(false, "/a/b").starts_with("Worktree"));
    }

    #[test]
    fn only_an_unshared_worktree_is_removed() {
        assert!(removable(true, 0));
        assert!(!removable(true, 1));
        assert!(!removable(false, 0));
    }
}
