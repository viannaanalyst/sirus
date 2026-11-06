//! ⌘F: the find bar over the open conversation (ports Sirus Code's `TranscriptSearchBar`).
//!
//! It searches each turn's user text and answer, case and accents folded, and walks the hits with
//! Enter, ⇧Enter, the arrows and wrap-around. Going to a hit scrolls its turn into view and washes
//! the turn with the accent. The markdown renderer takes no highlight ranges, so the match inside
//! the turn is not marked. Hits are computed when the query changes or the conversation gains
//! turns, never per frame. While the bar is up it takes the typing (`query_target`); Esc closes
//! it and gives the focus back to the composer, and never stops the agent.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;
use gpui::{Keystroke, Stateful};

/// Lowercase with accents and combining marks taken off: "Ação" folds to "acao".
pub(crate) fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            other => other,
        })
        .collect()
}

/// The indices of the turns whose (user text, answer) holds the query. A blank query matches nothing.
pub(crate) fn hits<'a>(turns: impl Iterator<Item = (&'a str, &'a str)>, query: &str) -> Vec<usize> {
    let needle = fold(query.trim());
    if needle.is_empty() {
        return Vec::new();
    }
    turns
        .enumerate()
        .filter(|(_, (user, answer))| {
            fold(user).contains(&needle) || fold(answer).contains(&needle)
        })
        .map(|(ix, _)| ix)
        .collect()
}

/// The hit after (or before) `at`, wrapping around. With no current hit: the first or the last.
pub(crate) fn wrap(at: Option<usize>, len: usize, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (at.map(|at| at.min(len - 1)), forward) {
        (None, true) => 0,
        (None, false) => len - 1,
        (Some(at), true) => (at + 1) % len,
        (Some(at), false) => (at + len - 1) % len,
    })
}

/// The bar's state: its ease, the query, and the hits of the conversation it was searched in.
pub(crate) struct State {
    shown: bool,
    /// Eases the bar in and out (0 hidden, 1 shown).
    phase: Tween,
    pub(super) query: String,
    /// The query the hits were found for; a difference means the query changed.
    searched: String,
    /// The conversation the hits belong to, and its turn count when they were found.
    scope: Option<(usize, usize)>,
    hits: Vec<usize>,
    /// The current hit: an index into `hits`.
    at: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            shown: false,
            phase: Tween::still(0., Instant::now()),
            query: String::new(),
            searched: String::new(),
            scope: None,
            hits: Vec::new(),
            at: None,
        }
    }
}

impl State {
    pub(crate) fn shown(&self) -> bool {
        self.shown
    }

    /// The turn of the current hit, while the bar is up (the wash marks it).
    pub(crate) fn current(&self) -> Option<usize> {
        if !self.shown {
            return None;
        }
        self.at.and_then(|at| self.hits.get(at).copied())
    }

    /// "3 de 12" as (3, 12); with no hits (0, 0).
    fn position(&self) -> (usize, usize) {
        (self.at.map_or(0, |at| at + 1), self.hits.len())
    }

    /// Moves to the next or previous hit, wrapping. Returns the turn to reveal.
    fn step(&mut self, forward: bool) -> Option<usize> {
        self.at = wrap(self.at, self.hits.len(), forward);
        self.at.map(|at| self.hits[at])
    }

    /// Brings the hits up to date with the query and the conversation. Runs each frame, but only
    /// searches when the query changed or the conversation's turn count did. Returns the turn to
    /// reveal when the query changed (its first hit).
    /// `text(i)` is the (user text, answer) of turn `i` of `count`.
    pub(crate) fn sync<'a>(
        &mut self,
        conversation: usize,
        count: usize,
        text: impl Fn(usize) -> (&'a str, &'a str),
    ) -> Option<usize> {
        if !self.shown {
            return None;
        }
        let query_changed = self.searched != self.query;
        let scope_changed = self.scope.is_none_or(|(c, _)| c != conversation);
        let turns_changed = self
            .scope
            .is_some_and(|(c, n)| c == conversation && n != count);
        if !query_changed && !scope_changed && !turns_changed {
            return None;
        }
        self.searched = self.query.clone();
        self.scope = Some((conversation, count));
        self.hits = hits((0..count).map(text), &self.query);
        self.at = if self.hits.is_empty() {
            None
        } else if query_changed {
            Some(0)
        } else {
            Some(self.at.unwrap_or(0).min(self.hits.len() - 1))
        };
        if query_changed { self.current() } else { None }
    }
}

fn find_button(id: &'static str, name: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(24.))
        .rounded(px(crate::theme::RADIUS_SM))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(content(0.07)))
        .child(icon(name, 13.))
}

impl Shell {
    fn find_duration(cx: &Context<Self>) -> f32 {
        if cx.reduce_motion() { 0. } else { MENU }
    }

    /// ⌘F: shows the bar (keeping its query) and gives it the typing.
    pub(super) fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer_menu = None;
        self.context.close();
        let duration = Self::find_duration(cx);
        self.find.shown = true;
        self.find
            .phase
            .set(1., Instant::now(), duration, Curve::Expo);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Esc: hides the bar and returns the focus to the composer.
    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.shown = false;
        let duration = Self::find_duration(cx);
        self.find
            .phase
            .set(0., Instant::now(), duration, Curve::Expo);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn find_go(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some(turn) = self.find.step(forward) {
            self.reveal_find_turn(turn, cx);
        }
        cx.notify();
    }

    fn reveal_find_turn(&mut self, turn: usize, cx: &mut Context<Self>) {
        let state = self.list_state(self.active_conversation(), cx);
        state.pause_following_tail();
        // Item 0 is the conversation's header, so turn `n` is item `n + 1`.
        state.scroll_to_reveal_item(turn + 1);
    }

    /// The keys while the bar is up. Returns false for keys the rest of the shell handles (⌘ shortcuts,
    /// letters). Esc is taken here, so it never reaches the agent's stop.
    pub(super) fn find_keys(
        &mut self,
        keys: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.find.shown || keys.modifiers.platform {
            return false;
        }
        match keys.key.as_str() {
            "escape" => self.close_find(window, cx),
            "enter" => self.find_go(!keys.modifiers.shift, cx),
            "down" => self.find_go(true, cx),
            "up" => self.find_go(false, cx),
            "backspace" => {
                self.find.query.pop();
            }
            // The composer's caret keys mean nothing here.
            "left" | "right" | "home" | "end" | "delete" => {}
            _ => return false,
        }
        cx.stop_propagation();
        cx.notify();
        true
    }

    /// The bar over the transcript's top right, easing in and out.
    pub(super) fn find_bar(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let phase = self.find.phase.at(now).clamp(0., 1.);
        if !self.find.shown && phase <= 0.01 {
            return None;
        }
        if !self.find.phase.done(now) {
            *moving = true;
        }
        let (at, count) = self.find.position();
        let query = self.find.query.clone();
        let placeholder = query.is_empty();
        let shown = self.find.shown;
        Some(
            div()
                .id("transcript-find")
                .occlude()
                .absolute()
                .top(px(12. + (1. - phase) * -6.))
                .right(px(16.))
                .opacity(phase)
                .flex()
                .items_center()
                .gap(px(4.))
                .h(px(34.))
                .pl(px(10.))
                .pr(px(4.))
                .rounded(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_color(content(0.12))
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(icon("search", 13.).text_color(content(0.5)))
                .child(
                    div()
                        .w(px(180.))
                        .min_w_0()
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .text_size(px(13.))
                        .text_color(if placeholder {
                            content(0.4)
                        } else {
                            content(0.92)
                        })
                        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().child(
                            if placeholder {
                                t("Buscar nesta conversa").to_owned()
                            } else {
                                query
                            },
                        ))
                        .when(shown, |d| {
                            d.child(div().flex_shrink_0().w(px(1.5)).h(px(15.)).bg(content(0.8)))
                        }),
                )
                .when(!placeholder, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(tf("{} de {}", &[&at, &count])),
                    )
                })
                .child(
                    find_button("find-previous", "chevron-up")
                        .on_click(cx.listener(|this, _, _, cx| this.find_go(false, cx))),
                )
                .child(
                    find_button("find-next", "chevron-down")
                        .on_click(cx.listener(|this, _, _, cx| this.find_go(true, cx))),
                )
                .child(
                    find_button("find-close", "close")
                        .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_lowercases_and_drops_accents() {
        assert_eq!(fold("Ação"), "acao");
        assert_eq!(fold("ÉPOCA çãõ"), "epoca cao");
        // Decomposed input (e + combining acute) folds the same way.
        assert_eq!(fold("cafe\u{301}"), "cafe");
        assert_eq!(fold("Straße"), "straße");
    }

    #[test]
    fn hits_match_user_text_and_answers_case_and_accent_insensitive() {
        let turns = [
            ("Onde fica a configuração?", "Em Ajustes."),
            ("olá", "Tudo bem, a CONFIGURAÇÃO foi salva."),
            ("nada aqui", "Nem aqui."),
            ("Mais uma pergunta", "Resposta sobre configuracao"),
        ];
        let pairs = || turns.iter().map(|(u, a)| (*u, *a));
        assert_eq!(hits(pairs(), "configuracao"), vec![0, 1, 3]);
        assert_eq!(hits(pairs(), "AJUSTES"), vec![0]);
        assert_eq!(hits(pairs(), "  olá  "), vec![1]);
        assert_eq!(hits(pairs(), "inexistente"), Vec::<usize>::new());
    }

    #[test]
    fn blank_query_matches_nothing() {
        let turns = [("texto", "resposta")];
        assert!(hits(turns.iter().copied(), "").is_empty());
        assert!(hits(turns.iter().copied(), "   ").is_empty());
    }

    #[test]
    fn wrap_goes_around_both_ways() {
        assert_eq!(wrap(None, 3, true), Some(0));
        assert_eq!(wrap(None, 3, false), Some(2));
        assert_eq!(wrap(Some(0), 3, true), Some(1));
        assert_eq!(wrap(Some(2), 3, true), Some(0));
        assert_eq!(wrap(Some(0), 3, false), Some(2));
        assert_eq!(wrap(Some(1), 3, false), Some(0));
        assert_eq!(wrap(Some(0), 1, true), Some(0));
        assert_eq!(
            wrap(Some(7), 3, true),
            Some(0),
            "a stale index is clamped first"
        );
        assert_eq!(wrap(Some(0), 0, true), None);
    }

    /// Turns as (user, answer) pairs, the way the shell hands them to `sync`.
    fn sync_with(state: &mut State, conversation: usize, turns: &[(&str, &str)]) -> Option<usize> {
        state.sync(conversation, turns.len(), |i| turns[i])
    }

    #[test]
    fn state_walks_hits_with_wrap_around() {
        let mut state = State {
            shown: true,
            ..Default::default()
        };
        let turns = [("a", "x"), ("b", "x"), ("c", "y"), ("d", "x")];
        state.query = "x".into();
        assert_eq!(sync_with(&mut state, 0, &turns), Some(0));
        assert_eq!(state.position(), (1, 3));
        assert_eq!(state.step(true), Some(1));
        assert_eq!(state.step(true), Some(3));
        assert_eq!(state.step(true), Some(0), "wraps forward");
        assert_eq!(state.step(false), Some(3), "wraps backward");
        assert_eq!(state.current(), Some(3));
    }

    #[test]
    fn sync_searches_only_when_the_query_or_the_turns_change() {
        let mut state = State {
            shown: true,
            ..Default::default()
        };
        let mut turns = vec![("a", "x")];
        state.query = "x".into();
        assert_eq!(sync_with(&mut state, 0, &turns), Some(0));
        // Same query, same turns: nothing to do.
        assert_eq!(sync_with(&mut state, 0, &turns), None);
        // A new turn is searched without moving the reveal.
        turns.push(("b", "x"));
        assert_eq!(sync_with(&mut state, 0, &turns), None);
        assert_eq!(state.position(), (1, 2));
        // A new query searches again and reveals its first hit.
        state.query = "zzz".into();
        assert_eq!(sync_with(&mut state, 0, &turns), None);
        assert_eq!(state.position(), (0, 0));
        assert_eq!(state.current(), None);
    }

    #[test]
    fn a_hidden_bar_has_no_current_hit() {
        let mut state = State::default();
        state.query = "x".into();
        let turns = [("a", "x")];
        assert_eq!(
            sync_with(&mut state, 0, &turns),
            None,
            "a hidden bar does not search"
        );
        state.shown = true;
        assert_eq!(sync_with(&mut state, 0, &turns), Some(0));
        state.shown = false;
        assert_eq!(state.current(), None);
    }
}
