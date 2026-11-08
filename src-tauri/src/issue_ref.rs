//! The issue or pull request a conversation's first message names (the chip over the
//! conversation): `#123` in a GitHub checkout, or a github.com issue or pull URL, read with
//! `gh api`. Anything else, or a failed read, answers nothing.

use std::ffi::OsStr;
use std::sync::Arc;

use serde::Serialize;

use crate::commands::{native_task, AppState};
use crate::error::Result;
use crate::runtime::State;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IssueRef {
    pub repository: String,
    pub number: u64,
    /// `issue` or `pull`.
    pub kind: &'static str,
    pub title: String,
    /// `open`, `draft`, `closed` or `merged`.
    pub state: &'static str,
    pub url: String,
}

/// `owner/name` and the number a reference names; a bare `#123` takes the checkout's repository.
pub(crate) fn parse(reference: &str, checkout: Option<&str>) -> Option<(String, u64)> {
    let reference = reference.trim();
    let valid = |repository: &str| {
        crate::pull_requests::repository(&format!("https://github.com/{repository}")).is_some()
    };
    let number = |text: &str| {
        text.parse::<u64>()
            .ok()
            .filter(|n| (1..=10_000_000).contains(n))
    };
    if let Some(rest) = reference.strip_prefix('#') {
        let repository = checkout.filter(|repository| valid(repository))?;
        return Some((repository.to_owned(), number(rest)?));
    }
    let rest = reference
        .strip_prefix("https://github.com/")
        .or_else(|| reference.strip_prefix("http://github.com/"))
        .or_else(|| reference.strip_prefix("github.com/"))?;
    let parts: Vec<&str> = rest.split(['/', '#', '?']).collect();
    if parts.len() < 4 || !matches!(parts[2], "issues" | "pull") {
        return None;
    }
    let repository = format!("{}/{}", parts[0], parts[1]);
    valid(&repository).then_some((repository, number(parts[3])?))
}

fn read(repository: &str, value: &serde_json::Value) -> Option<IssueRef> {
    let number = value["number"].as_u64()?;
    let title: String = value["title"].as_str()?.chars().take(300).collect();
    let pull = value.get("pull_request").is_some_and(|p| !p.is_null());
    let state = match (value["state"].as_str()?, pull) {
        ("open", true) if value["draft"].as_bool() == Some(true) => "draft",
        ("open", _) => "open",
        ("closed", true) if value["pull_request"]["merged_at"].is_string() => "merged",
        _ => "closed",
    };
    let kind = if pull { "pull" } else { "issue" };
    Some(IssueRef {
        repository: repository.to_owned(),
        number,
        kind,
        title,
        state,
        url: format!(
            "https://github.com/{repository}/{}/{number}",
            if pull { "pull" } else { "issues" }
        ),
    })
}

pub async fn issue_reference(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    reference: String,
) -> Result<Option<IssueRef>> {
    let state = state.inner().clone();
    let checkout = if reference.trim().starts_with('#') {
        native_task(move || crate::commands::pull_request_context(&state, &session_id))
            .await
            .ok()
            .and_then(|context| context.repository)
    } else {
        None
    };
    let Some((repository, number)) = parse(&reference, checkout.as_deref()) else {
        return Ok(None);
    };
    let endpoint = format!("repos/{repository}/issues/{number}");
    Ok(crate::pull_requests::api(OsStr::new("gh"), &endpoint)
        .await
        .ok()
        .and_then(|value| read(&repository, &value)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn references_name_a_repository_and_a_number() {
        assert_eq!(parse("#12", Some("a/b")), Some(("a/b".into(), 12)));
        assert_eq!(parse("#12", None), None);
        assert_eq!(parse("#x", Some("a/b")), None);
        assert_eq!(
            parse("https://github.com/o/r/pull/7/files", None),
            Some(("o/r".into(), 7))
        );
        assert_eq!(
            parse("github.com/o/r/issues/9#issuecomment-1", None),
            Some(("o/r".into(), 9))
        );
        assert_eq!(parse("https://github.com/o/r/tree/main", None), None);
        assert_eq!(parse("https://github.com/../r/pull/1", None), None);
    }

    #[test]
    fn a_pull_reads_merged_draft_or_open() {
        let merged = json!({"number": 3, "title": "T", "state": "closed", "pull_request": {"merged_at": "2026-01-01"}});
        let item = read("o/r", &merged).unwrap();
        assert_eq!((item.kind, item.state), ("pull", "merged"));
        assert_eq!(item.url, "https://github.com/o/r/pull/3");
        let draft =
            json!({"number": 3, "title": "T", "state": "open", "draft": true, "pull_request": {}});
        assert_eq!(read("o/r", &draft).unwrap().state, "draft");
        let issue = json!({"number": 4, "title": "T", "state": "closed"});
        let item = read("o/r", &issue).unwrap();
        assert_eq!((item.kind, item.state), ("issue", "closed"));
    }
}
