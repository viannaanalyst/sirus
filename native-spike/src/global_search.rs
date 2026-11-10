//! All-conversation search, as Sirus's command palette and its ⌘⇧F search bar do it: the
//! host's `transcript_action` search returns the messages that may contain the text (unloaded
//! transcripts included), and the window matches them again to cut a snippet. Message results
//! start at three characters and wait for the typing to pause. A reply for an older text is
//! dropped. Clicking a result opens its conversation.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the palette's private state.

use super::*;

/// Message search starts at this many characters, so one letter does not scan every transcript
/// (`PALETTE_MESSAGE_QUERY` in Sirus).
pub(crate) const MIN_CHARS: usize = 3;
/// How long the typing must rest before the host is asked.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// Rows of message results in ⌘K, and in the ⌘⇧F search that has the whole list to itself.
const PALETTE_ROWS: usize = 8;
const SEARCH_ROWS: usize = 30;

/// One conversation whose messages match: its id and the snippet of its latest matching message.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hit {
    pub session_id: String,
    pub snippet: String,
}

/// What the palette knows about the message search.
#[derive(Default)]
pub(crate) struct Search {
    /// The text last asked for (empty while it is too short).
    wanted: String,
    /// The text `hits` answer.
    done: String,
    hits: Vec<Hit>,
}

impl Search {
    /// The hits to show for `needle`: none until the host has answered exactly this text.
    fn answered(&self, needle: &str) -> &[Hit] {
        if self.done == needle { &self.hits } else { &[] }
    }

    /// Whether `needle` is long enough to search and still has no answer.
    fn pending(&self, needle: &str) -> bool {
        long_enough(needle) && self.done != needle
    }
}

/// The trimmed query when it is long enough to search the messages.
pub(crate) fn needle_of(query: &str) -> Option<String> {
    let needle = query.trim();
    long_enough(needle).then(|| needle.to_owned())
}

fn long_enough(needle: &str) -> bool {
    needle.chars().count() >= MIN_CHARS
}

/// Where `needle` first appears in `text`, ignoring case, as (start, end) in chars.
fn find_ci(text: &[char], needle: &str) -> Option<(usize, usize)> {
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return None;
    }
    'start: for start in 0..text.len() {
        let (mut at, mut taken) = (start, 0);
        while taken < needle.len() {
            let lower: Vec<char> = text.get(at)?.to_lowercase().collect();
            if needle[taken..].starts_with(&lower) {
                taken += lower.len();
                at += 1;
            } else {
                continue 'start;
            }
        }
        return Some((start, at));
    }
    None
}

/// The text around the first match: 45 characters before and 75 after, whitespace collapsed, with
/// an ellipsis where it was cut (the snippet of Sirus's `searchConversations`).
pub(crate) fn snippet(text: &str, needle: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let (start, end) = find_ci(&chars, needle)?;
    let from = start.saturating_sub(45);
    let to = (end + 75).min(chars.len());
    let body: String = chars[from..to].iter().collect();
    let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(format!(
        "{}{body}{}",
        if start > 45 { "…" } else { "" },
        if chars.len() > end + 75 { "…" } else { "" }
    ))
}

/// One hit per conversation in the host's `candidates` reply, from its latest message that holds
/// the text. Conversations the window does not know, and system messages, are left out.
pub(crate) fn hits_of(reply: &Value, needle: &str, known: impl Fn(&str) -> bool) -> Vec<Hit> {
    reply["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|session| {
            let id = session["sessionId"].as_str().filter(|id| known(id))?;
            let snippet = session["messages"]
                .as_array()?
                .iter()
                .rev()
                .filter(|message| message["role"] != "system")
                .find_map(|message| snippet(message["content"].as_str()?, needle))?;
            Some(Hit {
                session_id: id.to_owned(),
                snippet,
            })
        })
        .collect()
}

impl Shell {
    /// ⌘⇧F: the palette as an all-conversation message search; again to close it.
    pub(super) fn toggle_global_search(&mut self, cx: &mut Context<Self>) {
        self.appearance = None;
        if self.palette_ui.as_ref().is_some_and(|p| p.all) {
            self.palette_ui = None;
        } else {
            crate::segment_pill::forget("palette");
            self.palette_ui = Some(PaletteState {
                files: false,
                all: true,
                search: Search::default(),
                query: String::new(),
                selected: 0,
                scope: 0,
                pointer: None,
                scroll: gpui::ScrollHandle::new(),
                follow: std::cell::Cell::new(false),
                open: Tween::from_to(
                    0.,
                    1.,
                    Instant::now(),
                    if cx.reduce_motion() { 0. } else { MENU },
                    Curve::Expo,
                ),
            });
        }
        cx.notify();
    }

    /// The palette's text changed: ask the host once the typing rests, unless it already answered.
    pub(super) fn palette_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.palette_ui.as_mut().filter(|p| !p.files) else {
            return;
        };
        let wanted = needle_of(&state.query);
        let key = wanted.clone().unwrap_or_default();
        if state.search.wanted == key {
            return;
        }
        state.search.wanted = key;
        let Some(needle) = wanted else {
            cx.notify();
            return;
        };
        if state.search.done == needle {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.palette_search_fire(needle, cx));
        })
        .detach();
    }

    fn palette_search_fire(&mut self, needle: String, cx: &mut Context<Self>) {
        if !self
            .palette_ui
            .as_ref()
            .is_some_and(|p| p.search.wanted == needle)
        {
            return;
        }
        let call = self.backend.invoke(
            "transcript_action",
            json!({ "action": { "type": "search", "query": needle } }),
        );
        self.waiting.insert(call, Waiting::PaletteSearch(needle));
        cx.notify();
    }

    /// The host answered a search; a failed one counts as no results.
    pub(super) fn palette_search_arrived(
        &mut self,
        needle: String,
        ok: bool,
        reply: &Value,
        cx: &mut Context<Self>,
    ) {
        if !self
            .palette_ui
            .as_ref()
            .is_some_and(|p| p.search.wanted == needle)
        {
            return;
        }
        let mut hits = if ok {
            let known: std::collections::HashSet<&str> = self
                .conversations
                .iter()
                .map(|c| c.session_id.as_str())
                .collect();
            hits_of(reply, &needle, |id| known.contains(id))
        } else {
            Vec::new()
        };
        // The most recent conversation first.
        let activity = |id: &str| {
            self.conversations
                .iter()
                .find(|c| c.session_id == id)
                .map(|c| c.last_activity.clone())
                .unwrap_or_default()
        };
        hits.sort_by_key(|hit| std::cmp::Reverse(activity(&hit.session_id)));
        if let Some(state) = self.palette_ui.as_mut() {
            state.search.done = needle;
            state.search.hits = hits;
            state.selected = 0;
        }
        cx.notify();
    }

    /// The message rows for the palette: the host's hits as `PaletteHit::Message`, leaving out
    /// the conversations already listed by title in ⌘K.
    pub(super) fn palette_message_hits(&self, needle: &str, listed: &[usize]) -> Vec<PaletteHit> {
        let Some(state) = self.palette_ui.as_ref() else {
            return Vec::new();
        };
        let limit = if state.all { SEARCH_ROWS } else { PALETTE_ROWS };
        state
            .search
            .answered(needle)
            .iter()
            .filter_map(|hit| {
                let ix = self
                    .conversations
                    .iter()
                    .position(|c| c.session_id == hit.session_id)?;
                (state.all || !listed.contains(&ix))
                    .then(|| PaletteHit::Message(ix, hit.snippet.clone()))
            })
            .take(limit)
            .collect()
    }

    /// What the palette says when it has nothing to list.
    pub(super) fn palette_note(&self) -> &'static str {
        let Some(state) = self.palette_ui.as_ref().filter(|p| !p.files) else {
            return t("Nada encontrado.");
        };
        let needle = state.query.trim();
        if state.all && !long_enough(needle) {
            t("Digite ao menos 3 letras.")
        } else if state.search.pending(needle) {
            t("Buscando…")
        } else {
            t("Nada encontrado.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_starts_at_three_characters() {
        assert_eq!(needle_of("ab"), None);
        assert_eq!(needle_of("  ab  "), None);
        assert_eq!(needle_of(" abc "), Some("abc".to_owned()));
        assert_eq!(needle_of("ção"), Some("ção".to_owned()));
    }

    #[test]
    fn matching_ignores_case_and_returns_char_positions() {
        let text: Vec<char> = "Olá, AÇÃO feita".chars().collect();
        assert_eq!(find_ci(&text, "ação"), Some((5, 9)));
        assert_eq!(find_ci(&text, "zzz"), None);
        assert_eq!(find_ci(&text, ""), None);
    }

    #[test]
    fn a_snippet_keeps_the_match_and_marks_where_it_was_cut() {
        assert_eq!(
            snippet("fix the bug", "BUG").as_deref(),
            Some("fix the bug")
        );
        let long = format!("{}needle{}", "a ".repeat(60), " b".repeat(60));
        let cut = snippet(&long, "needle").unwrap();
        assert!(cut.starts_with('…') && cut.ends_with('…'));
        assert!(cut.contains("needle"));
        assert!(snippet("nothing here", "needle").is_none());
        assert_eq!(
            snippet("a\n\n  b needle", "needle").as_deref(),
            Some("a b needle")
        );
    }

    #[test]
    fn a_reply_gives_one_hit_per_known_conversation_from_its_latest_message() {
        let reply = json!({ "sessions": [
            { "sessionId": "a", "messages": [
                { "role": "user", "content": "first deploy note" },
                { "role": "agent", "content": "second deploy note" },
                { "role": "system", "content": "deploy system" },
            ]},
            { "sessionId": "gone", "messages": [{ "role": "user", "content": "deploy" }] },
            { "sessionId": "b", "messages": [{ "role": "user", "content": "unrelated" }] },
        ]});
        let hits = hits_of(&reply, "deploy", |id| id != "gone");
        assert_eq!(
            hits,
            vec![Hit {
                session_id: "a".into(),
                snippet: "second deploy note".into()
            }]
        );
        assert!(hits_of(&json!({}), "deploy", |_| true).is_empty());
    }

    #[test]
    fn hits_show_only_for_the_text_they_answer() {
        let mut search = Search::default();
        search.done = "abc".into();
        search.hits = vec![Hit {
            session_id: "a".into(),
            snippet: "x".into(),
        }];
        assert_eq!(search.answered("abc").len(), 1);
        assert!(search.answered("abcd").is_empty());
        assert!(!search.pending("abc"));
        assert!(search.pending("abcd"));
        assert!(!search.pending("ab"));
    }
}
