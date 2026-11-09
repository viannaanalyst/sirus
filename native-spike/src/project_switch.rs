//! The ⌥Tab switcher's cards (style A): each project as a miniature of its latest
//! conversation, drawn from the transcript at a tiny size rather than captured from the screen.
//! It shows the sidebar's titles, the last message and the start of the reply, and the composer.
//! The provider's tile sits on the corner, with the title, and the robot and name under it.

use super::*;
use gpui::AnimationExt as _;

const CARD_W: f32 = 236.;
const MINI_H: f32 = 140.;

impl Shell {
    /// The conversation a project's card shows: the one last looked at, else its newest.
    fn switch_conversation(&self, project: usize) -> Option<usize> {
        let list = &self.projects[project].conversations;
        self.remembered
            .iter()
            .rev()
            .find(|ix| list.contains(ix))
            .copied()
            .or_else(|| list.first().copied())
    }

    pub(super) fn switch_card(
        &self,
        project: usize,
        at: usize,
        chosen: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let conversation = self
            .switch_conversation(project)
            .map(|ix| &self.conversations[ix]);
        let provider = conversation.map_or("claude", |c| c.provider);
        let title = conversation
            .map(|c| c.title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| t("Nova conversa").to_owned());
        let last = conversation.and_then(|c| c.turns.last());
        let user = last.map_or(String::new(), |turn| turn.user.clone());
        let reply = last.map_or(String::new(), |turn| {
            turn.reply.clone().unwrap_or_else(|| turn.answer.clone())
        });
        let titles: Vec<String> = self.projects[project]
            .conversations
            .iter()
            .take(9)
            .map(|ix| self.conversations[*ix].title.clone())
            .collect();
        let tiny = |text: String, alpha: f32| {
            div()
                .text_size(px(4.6))
                .line_height(px(6.5))
                .text_color(content(alpha))
                .child(text)
        };
        let sidebar = div()
            .w(px(54.))
            .h_full()
            .flex_shrink_0()
            .px(px(5.))
            .py(px(7.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .bg(content(0.03))
            .border_r_1()
            .border_color(content(0.06))
            .children(titles.into_iter().enumerate().map(|(i, title)| {
                div()
                    .px(px(2.))
                    .rounded(px(2.))
                    .when(i == 0, |d| d.bg(content(0.08)))
                    .truncate()
                    .child(tiny(title, 0.55))
            }));
        let body = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(5.))
            .when(!user.is_empty(), |d| {
                d.child(
                    div().flex().justify_end().child(
                        div()
                            .max_w(px(110.))
                            .px(px(4.))
                            .py(px(2.))
                            .rounded(px(3.))
                            .bg(content(0.1))
                            .line_clamp(2)
                            .child(tiny(user, 0.85)),
                    ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .line_clamp(11)
                    .child(tiny(reply, 0.6)),
            )
            .child(
                div()
                    .h(px(15.))
                    .flex_shrink_0()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(content(0.12)),
            );
        let mini = div()
            .w_full()
            .h(px(MINI_H))
            .rounded(px(8.))
            .overflow_hidden()
            .bg(gpui::black().opacity(0.35))
            .border_1()
            .border_color(content(0.08))
            .flex()
            .child(sidebar)
            .child(body);
        let state = self.project_avatar_state_of(project);
        let reduce = cx.reduce_motion();
        let tile = mark_tile(
            &crate::tab_tile::TileSize {
                w: 30.,
                h: 30.,
                radius: 8.,
            },
            Face::provider(provider),
            provider_glyph(provider, 16.),
        );
        let white = |alpha: f32| gpui::hsla(0., 0., 1., alpha);
        let accent = crate::theme::accent();
        let card = div()
            .id(("project-switch", at))
            .relative()
            .w(px(CARD_W))
            .p(px(10.))
            .rounded(px(16.))
            .cursor_pointer()
            .opacity(if chosen { 1. } else { 0.62 })
            .when(chosen, |d| {
                d.bg(gpui::linear_gradient(
                    180.,
                    gpui::linear_color_stop(white(0.07), 0.),
                    gpui::linear_color_stop(white(0.02), 1.),
                ))
                .shadow(vec![
                    gpui::BoxShadow {
                        color: accent,
                        offset: gpui::point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(1.5),
                        inset: true,
                    },
                    gpui::BoxShadow {
                        color: accent.opacity(0.14),
                        offset: gpui::point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(4.),
                        inset: false,
                    },
                ])
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && let Some(switch) = this.project_switch.as_mut() {
                    switch.at = at;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.project_switch = None;
                this.switch_project(project, cx);
            }))
            .child(mini)
            .child(
                div()
                    .absolute()
                    .left(px(4.))
                    .top(px(4.))
                    .shadow_md()
                    .child(tile),
            )
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when(state != crate::project_avatar::AvatarState::Idle, |d| {
                        d.child(self.project_avatar_in(project, 16., state))
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(content(if chosen { 1. } else { 0.85 }))
                            .child(title),
                    ),
            )
            .child(
                div()
                    .mt(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.5))
                    .text_color(content(0.55))
                    .child(self.project_avatar_for(project, 16.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(self.projects[project].name.clone()),
                    ),
            );
        // Opacity and ring ease between chosen and not, quickly; reduced motion jumps.
        if reduce {
            return card.into_any_element();
        }
        card.with_animation(
            ("project-switch-fade", at * 2 + usize::from(chosen)),
            gpui::Animation::new(Duration::from_millis(130)),
            move |el, t| {
                let from = if chosen { 0.62 } else { 1. };
                let to = if chosen { 1. } else { 0.62 };
                el.opacity(from + (to - from) * t)
            },
        )
        .into_any_element()
    }
}
