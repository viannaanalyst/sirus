//! The files a finished turn changed, under its reply (TurnChangeSummary.tsx, compact form): one
//! line with the file count and the added and removed lines. A click opens the list of files; a
//! file in it shows its diff of that turn, colored as the Alterações pane colors diffs. It opens in
//! place, under the turn: the side panel's panes take no turn, so the review is not a pane.
//!
//! The diffs come with the turn (`activity.review`); when the transcript left them out
//! (`diffOmitted`), the whole turn is fetched once with `transcript_action`, as TurnReviewPane does.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::message_actions::{Review, ReviewFile};
use super::*;
use crate::panel_changes::{diff_line, note};
use std::collections::{HashMap, HashSet};

/// Files a card shows before "Show more".
const TURN_REVIEW_SHOWN: usize = 3;
/// Diff lines drawn for one file; the rest are counted, not drawn.
const MAX_DIFF_LINES: usize = 1500;

/// A turn's totals: the files still in its review and their added and removed lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Totals {
    pub files: usize,
    pub additions: u64,
    pub deletions: u64,
}

/// The summary's numbers. Committed files have left the review (ADR-108), so they do not count;
/// binary files count as files, with no lines.
pub(crate) fn totals(review: &Review) -> Totals {
    review
        .files
        .iter()
        .filter(|file| !file.committed)
        .fold(Totals::default(), |mut total, file| {
            total.files += 1;
            if !file.binary {
                let (added, removed) = line_counts(file);
                total.additions += added;
                total.deletions += removed;
            }
            total
        })
}

/// A file's added and removed lines: the host's counts, else the counts of its diff text.
pub(crate) fn line_counts(file: &ReviewFile) -> (u64, u64) {
    match (file.additions, file.deletions) {
        (Some(added), Some(removed)) => (added, removed),
        _ => file.diff.as_deref().map_or((0, 0), count_diff),
    }
}

/// Counts the `+` and `-` lines after the first hunk header, so the `---`/`+++` file headers and
/// the hunk headers themselves are not counted.
fn count_diff(diff: &str) -> (u64, u64) {
    let mut counts = (0, 0);
    let mut in_hunk = false;
    for line in diff.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
        } else if !in_hunk {
            continue;
        } else if line.starts_with('+') {
            counts.0 += 1;
        } else if line.starts_with('-') {
            counts.1 += 1;
        }
    }
    counts
}

/// The window's state for the review rows: which turns are open, the file shown in each, and the
/// full diffs of turns whose transcript left them out.
#[derive(Default)]
pub(crate) struct State {
    open: HashSet<String>,
    /// Turns whose card Keep dismissed (the changes stay as they are).
    dismissed: HashSet<String>,
    picked: HashMap<String, String>,
    full: HashMap<String, Full>,
}

enum Full {
    Loading,
    /// Path to diff text, for the files whose text the transcript left out.
    Ready(HashMap<String, String>),
    Failed,
}

impl Shell {
    /// The card under a finished turn that changed files: the count
    /// and totals, Undo, Keep and Review, the first three files and "Show more". Keep dismisses it.
    pub(super) fn turn_review_view(
        &self,
        conversation: usize,
        index: usize,
        turn: &Turn,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // A reply still running may change its files; the card waits for it to finish.
        if turn.live {
            return None;
        }
        let review = turn.review.as_ref()?;
        let key = turn.message_id.clone()?;
        if self.turn_review.dismissed.contains(&key) {
            return None;
        }
        let total = totals(review);
        if total.files == 0 {
            return None;
        }
        let open = self.turn_review.open.contains(&key);
        let needs_full = review
            .files
            .iter()
            .any(|file| file.omitted && !file.committed && file.diff.is_none());
        let label = if total.files == 1 {
            t("1 arquivo alterado").to_owned()
        } else {
            tf("{} arquivos alterados", &[&total.files])
        };
        let session = self.conversations[conversation].session_id.clone();
        let visible = review.files.iter().filter(|file| !file.committed).count();
        let undoable: Vec<String> = review
            .undoable()
            .iter()
            .map(|file| file.path.clone())
            .collect();
        let can_undo = !undoable.is_empty()
            && !self.conversations[conversation].working()
            && self.message_actions.pending.is_none();

        let undo = {
            let (session, message, key) = (session.clone(), key.clone(), key.clone());
            review_button("turn-review-undo", t("Desfazer"), can_undo).when(can_undo, |d| {
                d.on_click(cx.listener(move |this, _, _, cx| {
                    this.check_undo(
                        key.clone(),
                        session.clone(),
                        message.clone(),
                        undoable.clone(),
                        cx,
                    );
                    cx.notify();
                }))
            })
        };
        let keep = {
            let key = key.clone();
            review_button("turn-review-keep", t("Manter"), true).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.turn_review.dismissed.insert(key.clone());
                    cx.notify();
                },
            ))
        };
        let review_open = {
            let (session, message, key) = (session.clone(), key.clone(), key.clone());
            review_button("turn-review-open", t("Revisar"), true).on_click(cx.listener(
                move |this, _, _, cx| {
                    if !this.turn_review.open.contains(&key) {
                        this.toggle_turn_review(
                            key.clone(),
                            session.clone(),
                            message.clone(),
                            needs_full,
                            cx,
                        );
                    }
                },
            ))
        };
        let head = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(10.))
            .child(
                div()
                    .size(px(32.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .bg(content(0.08))
                    .child(icon("git-compare", 16.).text_color(content(0.55))),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(content(0.8))
                            .child(label),
                    )
                    .child(tally(total.additions, total.deletions)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(undo)
                    .child(keep)
                    .child(review_open),
            );

        let mut card = div()
            .id(("turn-changes", index))
            .mt(px(6.))
            .flex()
            .flex_col()
            .rounded(px(crate::theme::RADIUS_LG))
            .border_1()
            .border_color(content(0.07))
            .bg(content(0.03))
            .overflow_hidden()
            .child(head);
        if visible > 0 {
            card = card.child(self.turn_review_files(conversation, index, turn, open, cx));
        }
        if visible > TURN_REVIEW_SHOWN {
            let hidden = visible - TURN_REVIEW_SHOWN;
            let more = if open {
                t("Mostrar menos").to_owned()
            } else {
                tf("Mostrar mais {} arquivos", &[&hidden])
            };
            let (session, message, key) = (session.clone(), key.clone(), key.clone());
            card = card.child(
                div()
                    .id(("turn-review-more", index))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .w_full()
                    .h(px(32.))
                    .px(px(12.))
                    .border_t_1()
                    .border_color(content(0.05))
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .cursor_pointer()
                    .hover(|style| style.bg(content(0.05)).text_color(content(0.72)))
                    .child(icon(if open { "chevron-up" } else { "chevron-down" }, 12.))
                    .child(more)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_turn_review(
                            key.clone(),
                            session.clone(),
                            message.clone(),
                            needs_full,
                            cx,
                        );
                    })),
            );
        }
        Some(card.into_any_element())
    }

    /// The files of a review: the first few, or all of them when open. Each row opens its diff.
    fn turn_review_files(
        &self,
        conversation: usize,
        index: usize,
        turn: &Turn,
        open: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(review) = turn.review.as_ref() else {
            return div().into_any_element();
        };
        let key = turn.message_id.clone().unwrap_or_default();
        let session = self.conversations[conversation].session_id.clone();
        let picked = self.turn_review.picked.get(&key).cloned();
        let limit = if open { usize::MAX } else { TURN_REVIEW_SHOWN };
        let mut column = div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(content(0.05))
            .py(px(4.));
        if review.expired {
            column = column.child(div().px(px(12.)).py(px(4.)).child(note(
                t("Os diffs antigos foram liberados. O resumo e a confirmação foram preservados."),
                false,
            )));
        }
        for (position, file) in review
            .files
            .iter()
            .filter(|file| !file.committed)
            .enumerate()
            .take(limit)
        {
            let active = picked.as_deref() == Some(file.path.as_str());
            let path = file.path.clone();
            let row_key = key.clone();
            let row_session = session.clone();
            let omitted = file.omitted && !file.committed && file.diff.is_none();
            let (added, removed) = line_counts(file);
            let mut row = div()
                .id(("turn-file", index * 4096 + position))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(30.))
                .px(px(12.))
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(if active { content(0.92) } else { content(0.66) })
                .hover(|style| style.bg(content(0.05)).text_color(content(0.95)))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .text_ellipsis()
                        .font_family("Menlo")
                        .child(file.path.clone()),
                );
            row = if file.binary {
                row.child(
                    div()
                        .text_color(content(0.45))
                        .child(t("Arquivo binário: prévia textual indisponível.")),
                )
            } else {
                row.child(tally(added, removed))
            };
            column = column.child(row.on_click(cx.listener(move |this, _, _, cx| {
                this.pick_turn_review_file(
                    row_key.clone(),
                    path.clone(),
                    row_session.clone(),
                    omitted,
                    cx,
                );
            })));
            if active {
                column = column.child(self.turn_review_diff(index, &key, file, review.expired));
            }
        }
        column.into_any_element()
    }

    /// One file's diff of the turn: colored lines, capped, or the reason there is none.
    fn turn_review_diff(
        &self,
        index: usize,
        key: &str,
        file: &ReviewFile,
        expired: bool,
    ) -> AnyElement {
        let diff = file
            .diff
            .clone()
            .or_else(|| match self.turn_review.full.get(key) {
                Some(Full::Ready(texts)) => texts.get(&file.path).cloned(),
                _ => None,
            });
        let Some(diff) = diff else {
            let message = match self.turn_review.full.get(key) {
                _ if expired => note(
                    t(
                        "Os diffs antigos foram liberados. O resumo e a confirmação foram preservados.",
                    ),
                    false,
                ),
                _ if file.binary => note(t("Arquivo binário: prévia textual indisponível."), false),
                Some(Full::Loading) => note(t("Carregando…"), false),
                Some(Full::Failed) => note(t("Não foi possível ler o diff."), true),
                // The host held its text when the transcript left it out, and has no more.
                _ if file.omitted => note(t("Não foi possível ler o diff."), true),
                // Counted, but too long to keep.
                _ if line_counts(file) != (0, 0) => note(
                    t("Este diff excedeu o limite de armazenamento da revisão."),
                    false,
                ),
                _ => note(
                    t(
                        "O conteúdo textual não mudou; o arquivo ou suas permissões foram alterados.",
                    ),
                    false,
                ),
            };
            return div().mb(px(4.)).child(message).into_any_element();
        };
        let lines: Vec<&str> = diff.lines().collect();
        let mut body = div()
            .id(("turn-diff", index))
            .mb(px(4.))
            .max_h(px(360.))
            .overflow_y_scroll()
            .overflow_x_hidden()
            .rounded(px(crate::theme::RADIUS_SM))
            .bg(content(0.03))
            .py(px(4.));
        for line in lines.iter().take(MAX_DIFF_LINES) {
            body = body.child(diff_line(line));
        }
        if lines.len() > MAX_DIFF_LINES {
            body = body.child(note(
                &tf("… {} linhas a mais", &[&(lines.len() - MAX_DIFF_LINES)]),
                false,
            ));
        }
        body.into_any_element()
    }

    /// Opens or closes a turn's list. The first open of a turn whose diffs were left out fetches
    /// the whole turn once.
    fn toggle_turn_review(
        &mut self,
        key: String,
        session: String,
        message: String,
        needs_full: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.turn_review.open.remove(&key) {
            self.turn_review.open.insert(key.clone());
            if needs_full && !self.turn_review.full.contains_key(&key) {
                self.fetch_turn_review(key, session, message, cx);
            }
        }
        cx.notify();
    }

    fn pick_turn_review_file(
        &mut self,
        key: String,
        path: String,
        session: String,
        omitted: bool,
        cx: &mut Context<Self>,
    ) {
        if self.turn_review.picked.get(&key) == Some(&path) {
            self.turn_review.picked.remove(&key);
        } else {
            // The closed card shows its first files too, so a row may need the texts first.
            if omitted && should_fetch(self.turn_review.full.get(&key)) {
                self.fetch_turn_review(key.clone(), session, key.clone(), cx);
            }
            self.turn_review.picked.insert(key, path);
        }
        cx.notify();
    }

    /// Reads the turn with its diff texts (`turnReview`, as the transcript action sends it).
    fn fetch_turn_review(
        &mut self,
        key: String,
        session: String,
        message: String,
        cx: &mut Context<Self>,
    ) {
        self.turn_review.full.insert(key.clone(), Full::Loading);
        let rx = self.backend.request(
            "transcript_action",
            json!({ "action": { "type": "turnReview", "sessionId": session, "messageId": message } }),
        );
        cx.spawn(async move |this, cx| {
            let full = match rx.recv().await {
                Ok(Ok(value)) => match Review::of(&value["review"]) {
                    Some(review) => Full::Ready(
                        review
                            .files
                            .into_iter()
                            .filter_map(|file| Some((file.path, file.diff?)))
                            .collect(),
                    ),
                    None => Full::Failed,
                },
                _ => Full::Failed,
            };
            let _ = this.update(cx, |this, cx| {
                this.turn_review.full.insert(key, full);
                cx.notify();
            });
        })
        .detach();
    }
}

/// Whether a turn's diff texts should be asked for: not while on their way or once they came.
fn should_fetch(full: Option<&Full>) -> bool {
    matches!(full, None | Some(Full::Failed))
}

/// `+added −removed` in the diff colors, as the summary and the file rows show them.
/// A card button: Undo, Keep and Review
fn review_button(id: &'static str, label: &str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(content(if enabled { 0.75 } else { 0.3 }))
        .bg(content(0.08))
        .when(enabled, |d| {
            d.cursor_pointer()
                .sinks()
                .hover(|style| style.bg(content(0.12)).text_color(content(1.)))
        })
        .child(label.to_owned())
}

fn tally(added: u64, removed: u64) -> Div {
    let (add, remove) = diff_colors(0);
    div()
        .flex_shrink_0()
        .flex()
        .gap(px(6.))
        .font_family("Menlo")
        .text_size(px(11.))
        .child(div().text_color(add).child(format!("+{added}")))
        .child(div().text_color(remove).child(format!("−{removed}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(
        path: &str,
        additions: Option<u64>,
        deletions: Option<u64>,
        diff: Option<&str>,
    ) -> ReviewFile {
        ReviewFile {
            path: path.to_owned(),
            binary: false,
            reversible: true,
            undone: false,
            committed: false,
            additions,
            deletions,
            diff: diff.map(str::to_owned),
            omitted: false,
        }
    }

    fn review(files: Vec<ReviewFile>) -> Review {
        Review {
            files,
            expired: false,
        }
    }

    #[test]
    fn the_texts_are_asked_for_once_and_again_after_a_failure() {
        assert!(should_fetch(None));
        assert!(should_fetch(Some(&Full::Failed)));
        assert!(!should_fetch(Some(&Full::Loading)));
        assert!(!should_fetch(Some(&Full::Ready(HashMap::new()))));
    }

    #[test]
    fn totals_add_the_host_counts() {
        let total = totals(&review(vec![
            file("a.rs", Some(3), Some(1), None),
            file("b.rs", Some(0), Some(4), None),
        ]));
        assert_eq!(
            total,
            Totals {
                files: 2,
                additions: 3,
                deletions: 5
            }
        );
    }

    #[test]
    fn committed_files_leave_the_summary() {
        let mut done = file("done.rs", Some(9), Some(9), None);
        done.committed = true;
        let total = totals(&review(vec![done, file("a.rs", Some(1), Some(0), None)]));
        assert_eq!(total.files, 1);
        assert_eq!((total.additions, total.deletions), (1, 0));
    }

    #[test]
    fn binary_files_count_without_lines() {
        let mut logo = file("logo.png", None, None, None);
        logo.binary = true;
        let total = totals(&review(vec![logo]));
        assert_eq!(
            total,
            Totals {
                files: 1,
                additions: 0,
                deletions: 0
            }
        );
    }

    #[test]
    fn missing_counts_come_from_the_diff_after_the_hunk() {
        let diff = "--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,3 @@\n keep\n-old\n+new\n+more\n";
        assert_eq!(line_counts(&file("a.rs", None, None, Some(diff))), (2, 1));
        // A line that starts with `++` inside a hunk is an addition, not a file header.
        let tricky = "@@ -0,0 +1 @@\n+++counter\n";
        assert_eq!(line_counts(&file("c.rs", None, None, Some(tricky))), (1, 0));
    }

    #[test]
    fn a_file_with_no_counts_and_no_diff_adds_nothing() {
        assert_eq!(line_counts(&file("x.rs", None, None, None)), (0, 0));
    }

    #[test]
    fn empty_review_has_no_totals() {
        assert_eq!(totals(&review(Vec::new())), Totals::default());
    }
}
