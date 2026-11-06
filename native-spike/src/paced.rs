//! The streamed reply, revealed at a steady pace instead of in the chunks the model sends
//! (the rhythm of the `wordFade.tsx`): at least 90 characters a second, a backlog caught
//! up within 220 ms, a half-written word held back for 150 ms, and the shown text moved only
//! every 30 ms. The per-word fade of 320 ms is not drawn here. The rules are plain functions,
//! tested without a window. This module is a child of `shell` (`#[path]`).

use std::time::{Duration, Instant};

const FLOOR: f32 = 90.;
const CATCH_UP: f32 = 0.22;
const HOLD: Duration = Duration::from_millis(150);
const COMMIT: Duration = Duration::from_millis(30);

pub(crate) struct Pacer {
    /// Characters of the reply shown.
    shown: usize,
    /// Characters a second this stretch of text is revealed at, set when new text arrives.
    rate: f32,
    /// The length the rate was set for.
    seen: usize,
    /// Characters owed to the person but not yet earned by time (the fraction of a step).
    owed: f32,
    last_commit: Instant,
    /// Since when the end of what is shown would cut a word.
    cut_since: Option<Instant>,
}

impl Pacer {
    pub fn new(now: Instant) -> Self {
        Self {
            shown: 0,
            owed: 0.,
            rate: FLOOR,
            seen: 0,
            last_commit: now,
            cut_since: None,
        }
    }

    /// The part of `text` to show at `now`, and whether more frames are needed. `live` is false
    /// once the reply ended: then a half-written word is a whole one.
    pub fn view<'a>(&mut self, text: &'a str, now: Instant, live: bool) -> (&'a str, bool) {
        let total = text.chars().count();
        if self.shown > total {
            self.shown = total;
        }
        if total != self.seen {
            self.seen = total;
            // New text: the backlog is shown within CATCH_UP, and never slower than the floor.
            self.rate = FLOOR.max(total.saturating_sub(self.shown) as f32 / CATCH_UP);
        }
        let elapsed = now.saturating_duration_since(self.last_commit);
        if elapsed >= COMMIT && self.shown < total {
            self.owed += self.rate * elapsed.as_secs_f32();
            self.last_commit = now;
            let step = self.owed.floor() as usize;
            if step > 0 {
                self.owed -= step as f32;
                let mut target = (self.shown + step).min(total);
                if live && target > 0 {
                    // Do not stop inside a word unless it has waited long enough. At the end
                    // of the text the word may still be growing.
                    let chars: Vec<char> = text.chars().collect();
                    let cuts = !chars[target - 1].is_whitespace()
                        && chars.get(target).is_none_or(|c| !c.is_whitespace());
                    if cuts {
                        let since = *self.cut_since.get_or_insert(now);
                        if now.saturating_duration_since(since) < HOLD {
                            target = chars[..target]
                                .iter()
                                .rposition(|c| c.is_whitespace())
                                .map_or(self.shown, |at| at + 1)
                                .max(self.shown);
                        } else {
                            self.cut_since = None;
                        }
                    } else {
                        self.cut_since = None;
                    }
                }
                self.shown = target;
            }
        } else if elapsed >= COMMIT {
            self.last_commit = now;
        }
        let end = text
            .char_indices()
            .nth(self.shown)
            .map_or(text.len(), |(at, _)| at);
        (&text[..end], self.shown < total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_chunk_is_shown_over_time_not_at_once() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        let text =
            "Hello there, this is a reply that arrived in one go and goes on and on. ".repeat(3);
        let (first, more) = pacer.view(&text, at(start, 40), true);
        assert!(first.len() < text.len() / 2 && more, "{}", first.len());
        let (later, _) = pacer.view(&text, at(start, 400), true);
        assert!(later.len() > first.len());
    }

    #[test]
    fn a_backlog_is_caught_up_within_a_fifth_of_a_second() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        let text = "word ".repeat(400);
        let mut now = 0;
        loop {
            now += 30;
            let (_, more) = pacer.view(&text, at(start, now), false);
            if !more || now > 400 {
                break;
            }
        }
        assert!(now <= 330, "caught up at {now} ms");
    }

    #[test]
    fn a_half_written_word_waits_and_then_is_shown() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        let text = "one two thr";
        // Enough time to want every character.
        let (held, _) = pacer.view(text, at(start, 200), true);
        assert!(held.ends_with(' ') || held.is_empty(), "{held:?}");
        let (shown, _) = pacer.view(text, at(start, 500), true);
        assert_eq!(shown, "one two thr");
    }

    #[test]
    fn multibyte_text_is_cut_on_characters() {
        let start = Instant::now();
        let mut pacer = Pacer::new(start);
        let text = "ação é bênção ".repeat(20);
        let (part, _) = pacer.view(&text, at(start, 100), false);
        assert!(text.starts_with(part));
    }
}
