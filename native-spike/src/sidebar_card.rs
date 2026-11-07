//! The sidebar's hover card. A conversation row rests under the pointer
//! for 500 ms and a small card opens to its right, beside the row and not over it: the provider
//! and the model, what the conversation is doing, the full title, its branch and its folder.
//! Moving to another row switches the card at once; leaving the row and the card closes it; a
//! scroll of the list closes it. Nothing waits while the card is shut: the 500 ms is a task that
//! is dropped, and so cancelled, when the pointer leaves the row.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::activity_cards::{AMBER, GREEN, Kind, RED, pending_counts};
use super::*;
use gpui::Hsla;
use gpui::canvas;
use std::cell::Cell;
use std::rc::Rc;

/// How long the pointer rests on a row before its card opens.
const DELAY: Duration = Duration::from_millis(500);
const WIDTH: f32 = 260.;
/// The gap between the row and the card. The card's wrapper starts at the row's edge and holds
/// the gap, so the pointer crosses it without leaving the card's area.
const GAP: f32 = 6.;
/// A rough height, to keep the card on screen.
const HEIGHT: f32 = 170.;
/// How far the card starts toward the row before it slides out.
const SLIDE: f32 = 6.;

/// Where a row was last painted, in window coordinates. The row's probe fills it on paint, and
/// the hover listener reads it when the pointer arrives.
#[derive(Clone, Default)]
pub(crate) struct Place(Rc<Cell<Option<Bounds<Pixels>>>>);

impl Place {
    /// An invisible child that records the row's bounds on paint.
    pub(crate) fn probe(&self) -> impl IntoElement {
        let cell = self.0.clone();
        canvas(|_, _, _| {}, move |bounds, _, _, _| cell.set(Some(bounds)))
            .absolute()
            .size_full()
    }

    pub(crate) fn get(&self) -> Option<Bounds<Pixels>> {
        self.0.get()
    }
}

/// The hover card's state for the window.
#[derive(Default)]
pub(crate) struct State {
    /// The row under the pointer, with its place.
    row: Option<(usize, Bounds<Pixels>)>,
    /// The pointer is on the card itself.
    on_card: bool,
    /// The 500 ms wait for the row under the pointer; dropping it cancels the wait.
    wait: Option<gpui::Task<()>>,
    /// The card that is showing.
    open: Option<Open>,
}

struct Open {
    index: usize,
    row: Bounds<Pixels>,
    /// The fade and slide in, 0 to 1.
    fade: Tween,
}

/// The folder with the home directory written as `~`.
fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_owned(),
    }
}

impl Shell {
    /// The pointer came to a conversation row, or left it. `row` is where the row was painted.
    pub(super) fn sidebar_card_hover(
        &mut self,
        index: usize,
        row: Option<Bounds<Pixels>>,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if hovered {
            let Some(row) = row else { return };
            let blocked = self.sidebar_card_blocked(cx);
            let state = &mut self.sidebar_card;
            state.row = Some((index, row));
            if let Some(open) = state.open.as_mut() {
                // Another row while the card is up: it moves at once, without the wait.
                open.index = index;
                open.row = row;
                cx.notify();
            } else if !blocked {
                state.wait = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(DELAY).await;
                    let _ = this.update(cx, |this, cx| this.sidebar_card_show(index, cx));
                }));
            }
        } else {
            let state = &mut self.sidebar_card;
            if state.row.is_some_and(|(row, _)| row == index) {
                state.row = None;
                state.wait = None;
            }
            self.sidebar_card_settle(cx);
        }
    }

    /// The pointer came to the card or left it.
    pub(super) fn sidebar_card_on_card(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.sidebar_card.on_card = hovered;
        if !hovered {
            self.sidebar_card_settle(cx);
        }
    }

    /// The list scrolled: the card and the wait go, and the next hover starts again.
    pub(super) fn sidebar_card_scrolled(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.sidebar_card;
        if state.open.is_some() || state.wait.is_some() || state.row.is_some() {
            *state = State::default();
            cx.notify();
        }
    }

    /// Shuts the card unless the pointer is on the card or on a row. Leave and enter for one
    /// move arrive together, so this runs after both and never closes a card that the pointer
    /// has just moved on to another row.
    fn sidebar_card_settle(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.sidebar_card;
                if state.row.is_none() && !state.on_card && state.open.is_some() {
                    state.open = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The wait ended with the pointer still on the row: the card opens and fades in.
    fn sidebar_card_show(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((row_index, row)) = self.sidebar_card.row else {
            return;
        };
        if row_index != index || self.sidebar_card_blocked(cx) {
            return;
        }
        let duration = if cx.reduce_motion() { 0. } else { MENU };
        self.sidebar_card.wait = None;
        self.sidebar_card.open = Some(Open {
            index,
            row,
            fade: Tween::from_to(0., 1., Instant::now(), duration, Curve::Expo),
        });
        cx.notify();
    }

    /// Nothing opens while a rename, a drag or a menu is up.
    fn sidebar_card_blocked(&self, cx: &gpui::App) -> bool {
        cx.has_active_drag()
            || self.context_menu.is_some()
            || self.project_menu.is_some()
            || self.menu.is_some()
            || self.add_menu.is_some()
            || self.composer_menu.is_some()
            || self.palette_ui.is_some()
            || self.appearance.is_some()
            || self.confirm.is_some()
            || self.usage_popup.is_some()
            || self.environment.is_some()
            || self.theme_menu.is_some()
            || self.session_filter.open.is_some()
    }

    /// The card beside its row, or nothing.
    pub(super) fn sidebar_card_view(
        &self,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.sidebar_card.open.as_ref()?;
        if self.sidebar_card_blocked(cx) {
            return None;
        }
        let conversation = self.conversations.get(open.index)?;
        if !open.fade.done(now) {
            *moving = true;
        }
        let phase = open.fade.at(now).clamp(0., 1.);
        let (viewport_width, viewport_height) = (self.viewport_width, self.viewport_height);
        let right = f32::from(open.row.right());
        let card_left = (right + GAP).min(viewport_width - WIDTH - 8.).max(8.);
        let top = f32::from(open.row.top())
            .min(viewport_height - HEIGHT - 8.)
            .max(8.);
        let model = conversation
            .model
            .clone()
            .or_else(|| {
                conversation
                    .turns
                    .last()
                    .and_then(|turn| turn.model.clone())
            })
            .filter(|model| !model.is_empty());
        let project_path = self
            .project_of(open.index)
            .map(|project| self.projects[project].path.clone());
        let folder = Some(conversation.worktree.clone())
            .filter(|path| !path.is_empty())
            .or_else(|| project_path.clone())
            .unwrap_or_default();
        let linked = project_path.is_some_and(|path| path != folder);
        // What matters first: a waiting request, then work, then an unseen result, then the age.
        let (permissions, questions) = pending_counts(&conversation.pending);
        let (state, state_color): (String, Hsla) = if permissions > 0 {
            (t("Precisa de aprovação").to_owned(), rgb(AMBER).into())
        } else if questions > 0 {
            (t("Aguardando resposta").to_owned(), rgb(AMBER).into())
        } else if conversation.working() {
            (format!("{}…", t("Trabalhando")), accent())
        } else {
            match self.cards.result_of(&conversation.session_id) {
                Some(Kind::Failed) => (t("Falhou").to_owned(), rgb(RED).into()),
                Some(_) => (t("Concluída").to_owned(), rgb(GREEN).into()),
                None => (conversation.age(), content(0.45)),
            }
        };
        let detail = |label: &'static str, value: String| {
            div()
                .flex()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(72.))
                        .flex_shrink_0()
                        .text_color(content(0.45))
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(content(0.8))
                        .child(value),
                )
        };
        let view = cx.entity();
        let provider = conversation.provider;
        Some(
            div()
                .id("sidebar-card")
                .absolute()
                .left(px(card_left - GAP + (1. - phase) * -SLIDE))
                .top(px(top))
                .w(px(WIDTH + GAP))
                .pl(px(GAP))
                .opacity(phase)
                .occlude()
                .on_hover(move |hovered: &bool, _, cx| {
                    view.update(cx, |this, cx| this.sidebar_card_on_card(*hovered, cx));
                })
                .child(
                    div()
                        .relative()
                        .w_full()
                        .flex()
                        .flex_col()
                        .px(px(12.))
                        .py(px(10.))
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(content(0.14))
                        .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.))
                                .text_size(px(11.))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .text_color(content(0.55))
                                        .child(crate::tab_tile::mark_tile(
                                            &crate::tab_tile::TileSize {
                                                w: 14.,
                                                h: 14.,
                                                radius: 4.,
                                            },
                                            Face::provider(provider),
                                            provider_glyph(provider, 10.),
                                        ))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .child(model.unwrap_or_default()),
                                        ),
                                )
                                .child(div().flex_shrink_0().text_color(state_color).child(state)),
                        )
                        .child(
                            div()
                                .mt(px(6.))
                                .text_size(px(13.))
                                .line_height(px(20.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(content(1.))
                                .child(conversation.title.replace('\n', " ")),
                        )
                        .child(
                            div()
                                .mt(px(8.))
                                .pt(px(8.))
                                .border_t_1()
                                .border_color(content(0.08))
                                .text_size(px(11.))
                                .line_height(px(16.))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .when(!conversation.branch.is_empty(), |d| {
                                    d.child(detail("Branch", conversation.branch.clone()))
                                })
                                .child(detail(
                                    if linked { "Worktree" } else { t("Pasta") },
                                    tilde(&folder),
                                )),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_shows_home_as_a_tilde() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            assert_eq!(tilde(&format!("{home}/Projetos/app")), "~/Projetos/app");
            assert_eq!(tilde(&home), "~");
        }
        assert_eq!(tilde("/opt/app"), "/opt/app");
    }
}
