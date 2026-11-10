//! Small pure helpers for working on a remote computer's folders through its host (ADR-202): the
//! window cannot read that disk, so the Files search, attachments and new projects ask the host.
use crate::project_files::{self, Hit};
use serde_json::{Value, json};

/// What the host is asked for a typed search: the first word. The host lists the files whose path
/// holds it (`workspace_files`); the whole query then ranks them here, as a local search does.
pub fn host_query(query: &str) -> String {
    query
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches('/')
        .to_owned()
}

/// The hits for `query` among the files a `workspace_files` reply lists (folders left out).
pub fn ranked_hits(reply: &Value, query: &str, limit: usize) -> Vec<Hit> {
    let files: Vec<String> = reply["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["isDir"].as_bool() != Some(true))
        .filter_map(|entry| entry["path"].as_str().map(str::to_owned))
        .collect();
    project_files::search(&files, query, limit)
}

/// Where a clone goes on the host when nothing else is said: next to the most recent project (its
/// parent folder, as the phone does), or `~/Projetos`, which the host expands and may create.
pub fn clone_parent(recent_project_path: Option<&str>) -> String {
    recent_project_path
        .map(|path| path.trim_end_matches('/'))
        .and_then(|path| path.rsplit_once('/'))
        .map(|(parent, _)| parent)
        .filter(|parent| !parent.is_empty())
        .map_or_else(|| "~/Projetos".to_owned(), str::to_owned)
}

/// The `clone_project` arguments for what was typed: `owner/repo` or an address, and an optional
/// folder name. The host checks the address and the name; this only refuses an empty source.
pub fn clone_args(source: &str, folder: &str, parent: &str) -> Result<Value, &'static str> {
    let source = source.trim();
    if source.is_empty() {
        return Err("Digite dono/repositório ou o endereço do repositório.");
    }
    let folder = folder.trim();
    Ok(json!({
        "source": source,
        "parent": parent,
        "folder": (!folder.is_empty()).then_some(folder),
    }))
}

/// The `add_project` arguments for a folder typed as a path on the host: absolute, or `~`-based.
pub fn folder_args(path: &str) -> Result<Value, &'static str> {
    let path = path.trim();
    if path.is_empty() {
        return Err("Digite o caminho da pasta no computador.");
    }
    if !(path.starts_with('/') || path == "~" || path.starts_with("~/")) {
        return Err("Use o caminho completo, começando por / ou ~/.");
    }
    Ok(json!({ "path": path }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_host_is_asked_for_the_first_word_only() {
        assert_eq!(host_query("  panel files "), "panel");
        assert_eq!(host_query("/src/"), "src");
        assert_eq!(host_query("   "), "");
    }

    #[test]
    fn a_clone_goes_next_to_the_latest_project_or_into_projetos() {
        assert_eq!(clone_parent(Some("/home/dev/work/app/")), "/home/dev/work");
        assert_eq!(clone_parent(Some("/app")), "~/Projetos");
        assert_eq!(clone_parent(None), "~/Projetos");
    }

    #[test]
    fn clone_and_folder_arguments_are_checked_for_the_obvious() {
        let args = clone_args(" acme/app ", " ", "/srv").unwrap();
        assert_eq!(args["source"], "acme/app");
        assert!(args["folder"].is_null());
        assert_eq!(
            clone_args("acme/app", "my-app", "/srv").unwrap()["folder"],
            "my-app"
        );
        assert!(clone_args("  ", "", "/srv").is_err());
        assert_eq!(folder_args(" ~/code/app ").unwrap()["path"], "~/code/app");
        assert!(folder_args("code/app").is_err());
        assert!(folder_args("").is_err());
    }

    #[test]
    fn a_hosts_listing_is_ranked_without_its_folders() {
        let reply = json!({ "entries": [
            { "path": "src", "isDir": true },
            { "path": "src/panel_files.rs", "isDir": false },
            { "path": "docs/files.md", "isDir": false },
        ], "truncated": false });
        let hits = ranked_hits(&reply, "files", 10);
        let paths: Vec<_> = hits.iter().map(|hit| hit.path.as_str()).collect();
        assert!(paths.contains(&"src/panel_files.rs") && paths.contains(&"docs/files.md"));
        assert!(!paths.contains(&"src"));
        assert!(ranked_hits(&json!({}), "x", 10).is_empty());
    }
}
