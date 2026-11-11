//! "Aperte ⌘Q de novo": while the second press is awaited, the window dims, a dithered glow in the
//! accent colour rises from its edges, and the keys sit large in the middle with the words and a
//! pixel bar that empties with the time left. It passes clicks through and is gone after the
//! press window; reduced motion shows it without fading. This module is a child of `shell`
//! (`#[path]`).

use super::*;
use crate::visuals::dither::{Grid, inked};

/// How dark the window gets at most.
const DIM: f32 = 0.55;
/// Where the edge glow starts, as a share of the half-diagonal from the centre.
const GLOW_FROM: f32 = 0.72;
/// How quickly the glow thickens past that point.
const GLOW_RISE: f32 = 1.5;
const KEY: f32 = 72.;

impl Shell {
    pub(super) fn quit_overlay(&self, cx: &gpui::App) -> Option<AnyElement> {
        let (_, at) = self.press_again.filter(|(key, _)| *key == Press::Quit)?;
        let since = at.elapsed().as_secs_f32();
        let (shown, left) = if cx.reduce_motion() {
            (1., 1.)
        } else {
            self.send_moving.set(true);
            (press_notice_shown(since), 1. - since / PRESS_WINDOW)
        };
        let tone = accent();
        let backdrop = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                window.paint_quad(gpui::fill(bounds, crate::theme::black(DIM * shown)));
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let grid = Grid::new(bounds.origin, w, h);
                let glow = gpui::Hsla {
                    a: 0.35 * shown,
                    ..tone
                };
                grid.paint(window, |col, row| {
                    let (x, y) = Grid::center(col, row);
                    let (nx, ny) = ((x - w / 2.) / (w / 2.), (y - h / 2.) / (h / 2.));
                    let tone = ((nx * nx + ny * ny).sqrt() - GLOW_FROM).max(0.) * GLOW_RISE;
                    inked(col, row, tone).then_some(glow)
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let key = |label: &'static str| {
            div()
                .min_w(px(KEY))
                .h(px(KEY))
                .px(px(16.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(content(0.18))
                .bg(content(0.1))
                .shadow_lg()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(34.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(content(1.))
                .child(label)
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(backdrop)
                .child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .opacity(shown)
                        .child(div().flex().gap(px(12.)).child(key("⌘")).child(key("Q")))
                        .child(
                            div()
                                .mt(px(20.))
                                .text_size(px(15.))
                                .text_color(content(0.9))
                                .child(t("Aperte ⌘Q de novo para fechar o Sirus")),
                        )
                        .child(
                            div()
                                .mt(px(16.))
                                .w(px(260.))
                                .h(px(crate::pixel_bar::HEIGHT))
                                .child(crate::pixel_bar::pixel_bar(left.clamp(0., 1.), tone)),
                        ),
                )
                .into_any_element(),
        )
    }
}
