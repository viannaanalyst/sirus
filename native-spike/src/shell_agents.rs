//! The pill over the project rail while agents work in other projects: robots and a count, which
//! opens a list of those projects, one click from each. A child module of the shell, so it
//! reaches its private state.
use super::*;
use crate::theme::Sinks as _;
use gpui::{Transformation, radians};
use std::cell::Cell;
use std::rc::Rc;

const OPEN: f32 = 0.18;
const LIST_WIDTH: f32 = 236.;
/// The robots the pill stacks; a longer list is still counted in full.
const STACKED: usize = 3;

#[derive(Default)]
pub(super) struct AgentsPill {
    /// How far the list is open.
    open: Option<Tween>,
    /// The pill's fade-in, kept from when it last had something to show.
    appear: Cell<Option<Tween>>,
    /// Where the pill sits in the footer, for the list above it.
    slot: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// A project with an agent at work: the conversation a click opens and how long it has run.
struct Elsewhere {
    project: usize,
    conversation: usize,
    seconds: Option<u64>,
}

/// "48s" or "2m 14s".
fn elapsed_label(seconds: u64) -> String {
    if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

impl Shell {
    /// Other projects with a working conversation, the one running longest first.
    fn working_elsewhere(&self) -> Vec<Elsewhere> {
        let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let mut found: Vec<Elsewhere> = self
            .projects
            .iter()
            .enumerate()
            .filter(|(project, _)| Some(*project) != self.current_project)
            .filter_map(|(project, list)| {
                let seconds_of = |conversation: usize| {
                    let turn = self.conversations[conversation].turns.last()?;
                    let start = turn.started_ms?;
                    Some(now.saturating_sub(start).saturating_sub(turn.paused_ms) / 1000)
                };
                let conversation = list
                    .conversations
                    .iter()
                    .copied()
                    .filter(|ix| self.conversations[*ix].working())
                    .max_by_key(|ix| seconds_of(*ix))?;
                Some(Elsewhere {
                    project,
                    conversation,
                    seconds: seconds_of(conversation),
                })
            })
            .collect();
        found.sort_by_key(|place| std::cmp::Reverse(place.seconds));
        found
    }

    fn toggle_agents_list(&mut self, cx: &mut Context<Self>) {
        let length = if cx.reduce_motion() { 0. } else { OPEN };
        let now = Instant::now();
        let mut tween = self.agents.open.unwrap_or_else(|| Tween::still(0., now));
        let to = if tween.to > 0.5 { 0. } else { 1. };
        tween.set(to, now, length, Curve::Expo);
        self.agents.open = Some(tween);
        cx.notify();
    }

    /// `column` is the width to center in, over `bottom` points from the window's foot. With
    /// the sidebar closed nothing but the composer is under that spot, so the pill goes in the
    /// footer's row instead (`in_footer`), and only its list floats. Gives (what floats, the
    /// pill for the footer).
    pub(super) fn agents_pill_view(
        &mut self,
        column: f32,
        bottom: f32,
        in_footer: bool,
        now: Instant,
        moving: &mut bool,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<AnyElement>) {
        // The settings have their own column, where the rail isn't.
        let places = if self.appearance.is_some() || self.usage_page_open() {
            Vec::new()
        } else {
            self.working_elsewhere()
        };
        if places.is_empty() {
            self.agents.open = None;
            self.agents.appear.set(None);
            return (None, None);
        }
        let length = if cx.reduce_motion() { 0. } else { OPEN };
        let appear = self
            .agents
            .appear
            .get()
            .unwrap_or_else(|| Tween::from_to(0., 1., now, length, Curve::Expo));
        self.agents.appear.set(Some(appear));
        *moving |= !appear.done(now);
        let open = self.agents.open.as_ref().map_or(0., |tween| {
            *moving |= !tween.done(now);
            tween.at(now).clamp(0., 1.)
        });
        let showing = self
            .agents
            .open
            .as_ref()
            .is_some_and(|tween| tween.to > 0.5);
        let count = places.len();
        let stack = places.iter().take(STACKED).enumerate().map(|(at, place)| {
            div()
                .flex_shrink_0()
                .when(at > 0, |d| d.ml(px(-6.)))
                .child(self.project_avatar_for(place.project, if in_footer { 14. } else { 17. }))
        });
        let slot = self.agents.slot.clone();
        // Frosted, so it takes the color of the sidebar under it. Its corners keep the model
        // picker's proportions: 10px on the 30px floating pill, 7px on the 22px footer one.
        let radius = if in_footer {
            crate::theme::RADIUS_SM
        } else {
            crate::theme::RADIUS_MD
        };
        let pill = div()
            .id("agents-pill")
            .relative()
            .flex_shrink_0()
            .h(px(if in_footer { 22. } else { 30. }))
            .pl(px(8.))
            .pr(px(9.))
            .rounded(px(radius))
            .child(crate::theme::floating_surface(radius))
            .when(!in_footer, |d| d.shadow_lg())
            .flex()
            .items_center()
            .gap(px(7.))
            .text_size(px(if in_footer { 11.5 } else { 12.5 }))
            .text_color(content(0.92))
            .whitespace_nowrap()
            .opacity(appear.at(now).clamp(0., 1.))
            .cursor_pointer()
            .sinks()
            .hover(|s| s.bg(content(0.06)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| this.toggle_agents_list(cx)))
            .child(
                gpui::canvas(move |bounds, _, _| slot.set(Some(bounds)), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
            .child(div().flex().items_center().children(stack))
            .child(tf("{} trabalhando", &[&count]))
            .child(
                icon("chevron-up", 11.)
                    .text_color(content(0.55))
                    .with_transformation(Transformation::rotate(radians(
                        std::f32::consts::PI * open,
                    ))),
            );
        let track = crate::hover_track::list("agents-list");
        let list = (open > 0.001).then(|| {
            let rows = places.iter().enumerate().map(|(at, place)| {
                let conversation = place.conversation;
                track.row(
                    div()
                        .id(("agents-row", at))
                        .relative()
                        .h(px(34.))
                        .px(px(8.))
                        .rounded(px(crate::theme::RADIUS_MD))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(13.))
                        .text_color(content(0.92))
                        .cursor_pointer()
                        .sinks()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.agents.open = None;
                            this.open(conversation, cx);
                            cx.notify();
                        }))
                        .child(self.project_avatar_for(place.project, 17.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(self.projects[place.project].name.clone()),
                        )
                        .children(place.seconds.map(|seconds| {
                            div()
                                .flex_shrink_0()
                                .text_size(px(11.5))
                                .text_color(content(0.5))
                                .child(elapsed_label(seconds))
                        })),
                    at,
                    false,
                )
            });
            div()
                .id("agents-list")
                .occlude()
                .relative()
                .top(px((1. - open) * 6.))
                .mb(px(6.))
                .w(px(LIST_WIDTH.min(column - 16.).max(160.)))
                .p(px(4.))
                .opacity(open)
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(track.pill())
                .children(rows)
        });
        // A press outside closes the list; closed, this takes no clicks at all.
        let scrim = showing.then(|| {
            div()
                .id("agents-scrim")
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.toggle_agents_list(cx)),
                )
        });
        let (left, column) = match self.agents.slot.get().filter(|_| in_footer) {
            Some(slot) => {
                let width = LIST_WIDTH + 16.;
                ((f32::from(slot.center().x) - width / 2.).max(0.), width)
            }
            None => (0., column),
        };
        let (footer_pill, floating_pill) = if in_footer {
            (Some(pill.into_any_element()), None)
        } else {
            (None, Some(pill))
        };
        let floats = (list.is_some() || floating_pill.is_some()).then(|| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .children(scrim)
                .child(
                    div()
                        .absolute()
                        .left(px(left))
                        .bottom(px(bottom))
                        .w(px(column))
                        .flex()
                        .flex_col()
                        .items_center()
                        .children(list)
                        .children(floating_pill),
                )
                .into_any_element()
        });
        (floats, footer_pill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_reads_in_seconds_then_minutes() {
        assert_eq!(elapsed_label(48), "48s");
        assert_eq!(elapsed_label(60), "1m 0s");
        assert_eq!(elapsed_label(134), "2m 14s");
    }
}
