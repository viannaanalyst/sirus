//! A failure as a toast at the top of the window. It leaves by itself or with its X, and sending
//! or switching conversation clears it, as those always cleared the problem.

use super::*;

/// Long enough to read a sentence or two.
const SHOWN_FOR: Duration = Duration::from_secs(8);

impl Shell {
    pub(super) fn problem_toast(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.problem_shown != self.problem {
            self.problem_shown = self.problem.clone();
            if let Some(shown) = self.problem.clone() {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SHOWN_FOR).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.problem.as_deref() == Some(shown.as_str()) {
                            this.problem = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        let problem = self.problem.clone()?;
        let color = if self.notice == self.problem {
            content(0.9)
        } else {
            rgb(0xe08a8a).into()
        };
        Some(
            div()
                .absolute()
                .top(px(crate::theme::TITLE_BAR + 6.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("problem-toast")
                        .relative()
                        .max_w(px(520.))
                        .rounded(px(crate::theme::RADIUS_LG))
                        .border_1()
                        .border_color(crate::theme::border())
                        .shadow_lg()
                        .pl(px(12.))
                        .pr(px(6.))
                        .py(px(6.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(match problem.split_once('\n') {
                            // A title with a line under it (the Git button's results).
                            Some((title, detail)) => div()
                                .min_w_0()
                                .line_height(px(18.))
                                .text_color(color)
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(title.to_owned()),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .line_clamp(2)
                                        .text_color(content(0.55))
                                        .child(detail.to_owned()),
                                ),
                            None => div()
                                .min_w_0()
                                .text_size(px(13.))
                                .line_height(px(18.))
                                .line_clamp(3)
                                .text_color(color)
                                .child(problem.clone()),
                        })
                        .child(
                            div()
                                .id("problem-toast-close")
                                .flex_shrink_0()
                                .size(px(20.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(content(0.1)))
                                .tooltip(message_actions::name_tip(t("Fechar")))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.problem = None;
                                    cx.notify();
                                }))
                                .child(icon("close", 10.).text_color(content(0.5))),
                        ),
                )
                .into_any_element(),
        )
    }
}
