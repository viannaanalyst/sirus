//! The working spinner, the shimmering label and the tree rail, ported from Monocode's
//! session_mark.rs and activity.rs (MIT, LICENSE-MONOCODE-MIT).
use crate::theme::{content, rail_color};
use gpui::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    LayoutId, Pixels, StyledText, Window, div, point, prelude::*, px, rgb, svg,
};
use std::f32::consts::TAU;

const ROWS: [u32; 3] = [0x6f84a3, 0xe8b583, 0xe47fa6];
pub const SPINNER_PERIOD: f32 = 1.2;

/// A soft wave across a 12 px 3×3 grid, at `fraction` of its period.
pub fn spinner_at(fraction: f32) -> gpui::Div {
    let phase = fraction * TAU;
    div().size(px(12.)).flex().flex_col().children((0..3).map(|row| {
        div().flex().children((0..3).map(move |col| {
            let wave = (0.5 - 0.5 * (phase - (col + row) as f32 * 0.9).cos()).powf(3.52);
            let peak = 0.93 * (1. - 0.65 * (col as f32 - row as f32).abs() / 2.);
            let mut color: Hsla = rgb(ROWS[row]).into();
            color.a = 0.07 + peak * wave;
            div().size(px(4.)).flex().items_center().justify_center().child(
                div().size(px(2.5 * (0.89 + 0.11 * peak * wave))).rounded_full().bg(color),
            )
        }))
    }))
}

/// Shades a fixed glyph layout, so the sweep never changes kerning.
pub fn shimmer(text: String, elapsed: f32) -> AnyElement {
    Shimmer { text: StyledText::new(text), elapsed }.into_any_element()
}

struct Shimmer {
    text: StyledText,
    elapsed: f32,
}
impl IntoElement for Shimmer {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Shimmer {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        self.text.request_layout(None, None, window, cx)
    }
    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, state: &mut (), window: &mut Window, cx: &mut App) {
        self.text.prepaint(None, None, bounds, state, window, cx);
    }
    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), _: &mut (), window: &mut Window, _: &mut App) {
        let layout = self.text.layout();
        let line_height = layout.line_height();
        let lines = layout.line_layouts();
        window.paint_layer(bounds, |window| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for wrapped in &lines {
                    let line = &wrapped.unwrapped_layout;
                    let baseline = (line_height - line.ascent - line.descent) / 2. + line.ascent;
                    let width = f32::from(line.width).max(1.);
                    let center = -0.25 + (self.elapsed / 1.6).fract() * 1.5;
                    for run in &line.runs {
                        for glyph in &run.glyphs {
                            let x = f32::from(glyph.position.x + line.font_size * 0.25) / width;
                            let sweep = (1. - (x - center).abs() / (2. / 7.)).max(0.);
                            let origin = bounds.origin + point(glyph.position.x, baseline + glyph.position.y);
                            let _ = window.paint_glyph(origin, run.font_id, glyph.id, line.font_size, content(0.4 + 0.6 * sweep));
                        }
                    }
                }
            });
        });
    }
}

/// The rail under a tile, dropping from its centre into a child row.
pub const TILE_RAIL_X: f32 = crate::tab_tile::ACTIVITY_TILE.w / 2. - 0.5;
pub const TILE_RAIL_INDENT: f32 = crate::tab_tile::ACTIVITY_TILE.w + 8.;
pub fn rail(last: bool) -> impl IntoElement {
    let color = rail_color();
    div()
        .absolute().top_0().left(px(TILE_RAIL_X)).w(px(9.)).h_full()
        .child(div().absolute().top_0().left_0().w(px(1.)).map(|d| if last { d.h(px(6.)) } else { d.h_full() }).bg(color))
        .child(svg().path("tool-branch").absolute().left_0().top(px(5.)).size(px(9.)).text_color(color))
}
