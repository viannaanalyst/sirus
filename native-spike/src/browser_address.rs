//! The address bar's logic, after Sirus Code's `browser-url.ts`, `BrowserPanel.tsx` and
//! `local-servers.ts`: what typed text loads (an address or a Google search), the suggestions under
//! the field (open tabs, then history, then a search for what was typed), and the local servers
//! listed on the blank page. Pure functions, no GPUI.
use serde_json::Value;

/// How many pages one conversation's history keeps (`noteBrowserUrl`).
pub const HISTORY_LIMIT: usize = 50;
/// Suggestion rows under the field.
const SUGGESTIONS: usize = 6;
/// Servers listed on the blank page.
const MAX_SERVERS: usize = 8;

fn local_host(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    ["localhost", "127.0.0.1", "[::1]"].iter().any(|host| {
        lower.strip_prefix(host).is_some_and(|rest| {
            let rest = rest.strip_prefix(':').map_or(rest, |ports| {
                ports.trim_start_matches(|c: char| c.is_ascii_digit())
            });
            rest.is_empty() || rest.starts_with(['/', '?', '#'])
        })
    })
}

/// Text with a dot between two non-empty parts and no slash, query or space before it:
/// `example.com`, `docs.rs/serde`.
fn host_like(value: &str) -> bool {
    let end = value.find(['/', '?', '#']).unwrap_or(value.len());
    let host = &value[..end];
    match host.find('.') {
        Some(dot) => dot > 0 && dot + 1 < host.len() && !host.contains(char::is_whitespace),
        None => false,
    }
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The search page for text that is not an address.
pub fn search_url(query: &str) -> String {
    format!(
        "https://www.google.com/search?q={}",
        percent_encode(query.trim())
    )
}

/// Turns typed text into a page to load: a URL as it is, a host with a scheme (http for this
/// machine, https otherwise), anything else a Google search (`normalizeBrowserAddressInput`).
pub fn normalize_input(input: &str) -> Option<String> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower == "about:blank" {
        return Some(value.to_owned());
    }
    if !value.contains(char::is_whitespace) && (local_host(value) || host_like(value)) {
        let scheme = if local_host(value) { "http" } else { "https" };
        return Some(format!("{scheme}://{value}"));
    }
    Some(search_url(value))
}

/// Remembers a visited page at the front of the history: web pages only, once each.
pub fn note_visit(history: &mut Vec<String>, url: &str) {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return;
    }
    if history.first().is_some_and(|first| first == url) {
        return;
    }
    history.retain(|entry| entry != url);
    history.insert(0, url.to_owned());
    history.truncate(HISTORY_LIMIT);
}

/// A row under the address field.
#[derive(Clone, Debug, PartialEq)]
pub enum Suggestion {
    /// An open tab: selecting it switches to the tab.
    Tab {
        id: String,
        url: String,
        title: String,
    },
    /// A page visited before.
    History { url: String },
    /// Search for the typed text.
    Search { query: String },
}

impl Suggestion {
    /// The main line.
    pub fn title(&self) -> &str {
        match self {
            Self::Tab { url, title, .. } => {
                if title.is_empty() {
                    url
                } else {
                    title
                }
            }
            Self::History { url } => url,
            Self::Search { query } => query,
        }
    }
    /// The grey line under it, when there is one.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Tab { url, title, .. } if !title.is_empty() => Some(url),
            _ => None,
        }
    }
}

/// Open tabs, then history (the tabs' own addresses left out), narrowed by the typed text in
/// the address or the title, the page on screen left out, at most six (`suggestions`). When the
/// text is not itself an address, the last row searches for it.
pub fn suggestions(
    query: &str,
    tabs: &[(String, String, String)],
    history: &[String],
    active_url: &str,
) -> Vec<Suggestion> {
    let query = query.trim();
    let needle = query.to_lowercase();
    let mut entries: Vec<Suggestion> = tabs
        .iter()
        .filter(|(_, url, _)| !url.is_empty() && url != "about:blank")
        .map(|(id, url, title)| Suggestion::Tab {
            id: id.clone(),
            url: url.clone(),
            title: title.clone(),
        })
        .collect();
    for url in history {
        if !entries
            .iter()
            .any(|entry| matches!(entry, Suggestion::Tab { url: tab, .. } if tab == url))
        {
            entries.push(Suggestion::History { url: url.clone() });
        }
    }
    let mut unique: Vec<Suggestion> = Vec::new();
    for entry in entries {
        let url = match &entry {
            Suggestion::Tab { url, .. } | Suggestion::History { url } => url.clone(),
            Suggestion::Search { .. } => continue,
        };
        let seen = unique.iter().any(|other| match other {
            Suggestion::Tab { url: other, .. } | Suggestion::History { url: other } => {
                *other == url
            }
            Suggestion::Search { .. } => false,
        });
        if !seen {
            unique.push(entry);
        }
    }
    let mut rows: Vec<Suggestion> = unique
        .into_iter()
        .filter(|entry| match entry {
            Suggestion::Tab { url, title, .. } => {
                url != active_url
                    && (needle.is_empty()
                        || url.to_lowercase().contains(&needle)
                        || title.to_lowercase().contains(&needle))
            }
            Suggestion::History { url } => {
                url != active_url && (needle.is_empty() || url.to_lowercase().contains(&needle))
            }
            Suggestion::Search { .. } => false,
        })
        .take(SUGGESTIONS)
        .collect();
    // Typing something that would be searched, not opened: offer the search itself.
    if !query.is_empty()
        && normalize_input(query)
            .is_some_and(|url| url.starts_with("https://www.google.com/search?q="))
    {
        rows.truncate(SUGGESTIONS - 1);
        rows.push(Suggestion::Search {
            query: query.to_owned(),
        });
    }
    rows
}

/// What loading a suggestion does: its URL, or the search page.
pub fn suggestion_url(suggestion: &Suggestion) -> Option<String> {
    match suggestion {
        Suggestion::Tab { .. } => None,
        Suggestion::History { url } => Some(url.clone()),
        Suggestion::Search { query } => Some(search_url(query)),
    }
}

/// One row of the blank page's list of local servers.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalServer {
    pub url: String,
    pub process: String,
}

/// The rows of `local_servers_list`'s answer (url, port, process…), at most eight, one per port.
pub fn local_servers(list: &Value) -> Vec<LocalServer> {
    let mut seen = Vec::new();
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let url = item["url"].as_str()?.to_owned();
            let port = item["port"].as_u64();
            if port.is_some() && seen.contains(&port) {
                return None;
            }
            seen.push(port);
            Some(LocalServer {
                url,
                process: item["process"].as_str().unwrap_or("").to_owned(),
            })
        })
        .take(MAX_SERVERS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typed_text_becomes_an_address_or_a_search() {
        assert_eq!(normalize_input("  "), None);
        assert_eq!(
            normalize_input("https://a.dev/x").as_deref(),
            Some("https://a.dev/x")
        );
        assert_eq!(
            normalize_input("about:blank").as_deref(),
            Some("about:blank")
        );
        assert_eq!(
            normalize_input("example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            normalize_input("docs.rs/serde?x=1").as_deref(),
            Some("https://docs.rs/serde?x=1")
        );
        assert_eq!(
            normalize_input("localhost:3000/app").as_deref(),
            Some("http://localhost:3000/app")
        );
        assert_eq!(
            normalize_input("127.0.0.1").as_deref(),
            Some("http://127.0.0.1")
        );
        assert_eq!(
            normalize_input("rust async book").as_deref(),
            Some("https://www.google.com/search?q=rust%20async%20book")
        );
        // A sentence with a dot in it is still a search.
        assert_eq!(
            normalize_input("what is rust. really").as_deref(),
            Some("https://www.google.com/search?q=what%20is%20rust.%20really")
        );
        assert_eq!(
            normalize_input("localhostile").as_deref(),
            Some("https://www.google.com/search?q=localhostile")
        );
        assert_eq!(
            normalize_input("café").as_deref(),
            Some("https://www.google.com/search?q=caf%C3%A9")
        );
    }

    #[test]
    fn history_keeps_web_pages_once_most_recent_first() {
        let mut history = Vec::new();
        note_visit(&mut history, "about:blank");
        note_visit(&mut history, "file:///etc/passwd");
        note_visit(&mut history, "https://a.dev");
        note_visit(&mut history, "https://b.dev");
        note_visit(&mut history, "https://a.dev");
        note_visit(&mut history, "https://a.dev");
        assert_eq!(history, ["https://a.dev", "https://b.dev"]);
        for i in 0..80 {
            note_visit(&mut history, &format!("https://site{i}.dev"));
        }
        assert_eq!(history.len(), HISTORY_LIMIT);
        assert_eq!(history[0], "https://site79.dev");
    }

    #[test]
    fn suggestions_list_tabs_then_history_then_a_search() {
        let tabs = vec![
            (
                "tab-1".to_owned(),
                "https://docs.rs".to_owned(),
                "Docs".to_owned(),
            ),
            (
                "tab-2".to_owned(),
                "https://now.dev".to_owned(),
                String::new(),
            ),
            ("tab-3".to_owned(), "about:blank".to_owned(), String::new()),
        ];
        let history = vec![
            "https://now.dev".to_owned(),
            "https://old.dev/docs".to_owned(),
            "https://other.dev".to_owned(),
        ];
        // Empty text: everything but the page on screen and duplicates.
        let all = suggestions("", &tabs, &history, "https://now.dev");
        assert_eq!(all.len(), 3);
        assert!(matches!(&all[0], Suggestion::Tab { id, .. } if id == "tab-1"));
        assert!(matches!(&all[1], Suggestion::History { url } if url == "https://old.dev/docs"));
        // Narrowed by address or title, case blind; non-addresses end with a search row.
        let docs = suggestions("DOCS", &tabs, &history, "");
        assert_eq!(docs.len(), 3);
        assert_eq!(docs[0].title(), "Docs");
        assert_eq!(docs[0].detail(), Some("https://docs.rs"));
        assert_eq!(
            docs[2],
            Suggestion::Search {
                query: "DOCS".into()
            }
        );
        assert_eq!(
            suggestion_url(&docs[2]).as_deref(),
            Some("https://www.google.com/search?q=DOCS")
        );
        assert_eq!(suggestion_url(&docs[0]), None);
        // Something that is an address needs no search row.
        let host = suggestions("old.dev", &tabs, &history, "");
        assert_eq!(host.len(), 1);
        // At most six rows, the search among them.
        let many: Vec<String> = (0..20).map(|i| format!("https://s{i}.dev/rust")).collect();
        let rows = suggestions("rust", &[], &many, "");
        assert_eq!(rows.len(), 6);
        assert!(matches!(rows[5], Suggestion::Search { .. }));
    }

    #[test]
    fn local_servers_show_once_per_port() {
        let list = json!([
            { "url": "http://localhost:3000", "port": 3000, "process": "node" },
            { "url": "http://127.0.0.1:3000", "port": 3000, "process": "node" },
            { "url": "http://localhost:5173", "port": 5173, "process": "vite" },
            { "port": 1 },
        ]);
        let servers = local_servers(&list);
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[1].process, "vite");
        assert!(local_servers(&Value::Null).is_empty());
    }
}
