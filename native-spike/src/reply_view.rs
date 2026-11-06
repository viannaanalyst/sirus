//! How the rich pieces of a reply look in the transcript (see `reply_rich`): image and video tiles,
//! diagrams, formulas and the "Abrir prévia" controls. A tile has its final size from the first
//! frame (a placeholder until the file arrives), so nothing moves when it does. Anything that
//! cannot be shown falls back to the plain text or code the reply wrote.
use crate::i18n::t;
use crate::reply_rich::{self as rich, Act, Asset, MediaKind, Want};
use crate::theme::{self, accent, content};
use gpui::{
    AnyElement, App, ClickEvent, ElementId, ObjectFit, SharedString, StyledImage, Window, div, img,
    prelude::*, px,
};
use std::path::Path;
use std::rc::Rc;

/// What a reply is drawn with: whose session it belongs to (the host authorizes paths against
/// it), whether it is still streaming, and where clicks go.
#[derive(Clone)]
pub struct Rich {
    pub session: String,
    /// The session's folder, for relative video links ("" when unknown).
    pub cwd: String,
    pub streaming: bool,
    pub act: Rc<dyn Fn(&Act, &mut Window, &mut App)>,
}

const IMAGE_MAX: (f32, f32) = (480., 320.);
const IMAGE_SLOT: (f32, f32) = (320., 180.);

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_owned()
}

fn element_id(key: &str, extra: &str) -> ElementId {
    ElementId::Name(SharedString::from(format!("{key}{extra}")))
}

fn slot(size: (f32, f32)) -> gpui::Div {
    div()
        .w(px(size.0))
        .h(px(size.1))
        .flex_shrink_0()
        .rounded(px(crate::theme::RADIUS_MD))
        .overflow_hidden()
        .border_1()
        .border_color(content(0.08))
        .bg(content(0.04))
}

fn caption(text: String) -> AnyElement {
    div()
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(content(0.5))
        .child(text)
        .into_any_element()
}

/// The link the reply wrote, when its media cannot be shown.
fn link_text(label: &str) -> AnyElement {
    div()
        .text_color(accent())
        .child(label.to_owned())
        .into_any_element()
}

/// An image or video the reply links to. `alt` is the caption the reply gave it.
pub fn media(url: &str, alt: &str, kind: MediaKind, rich: Option<&Rich>) -> AnyElement {
    let path = rich::media_path(url);
    let name = file_name(&path);
    let label = if alt.trim().is_empty() {
        name.clone()
    } else {
        alt.trim().to_owned()
    };
    let Some(ctx) = rich else {
        return link_text(&label);
    };
    let want = match kind {
        MediaKind::Image => Want::Image {
            session: ctx.session.clone(),
            path,
        },
        MediaKind::Video => Want::Video {
            session: ctx.session.clone(),
            path,
            cwd: ctx.cwd.clone(),
        },
    };
    let key = want.key();
    let column = |tile: AnyElement| {
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(5.))
            .child(tile)
            .child(caption(label.clone()))
            .into_any_element()
    };
    match rich::asset(&want) {
        Asset::Pending => column(slot(IMAGE_SLOT).into_any_element()),
        Asset::Failed => link_text(&label),
        Asset::Picture { image, size } => {
            let (w, h) = rich::fit(size, IMAGE_MAX);
            let act = ctx.act.clone();
            let open = Act::Image {
                name: name.clone(),
                want,
            };
            column(
                slot((w, h))
                    .id(element_id(&key, ""))
                    .cursor_pointer()
                    .on_click(move |_: &ClickEvent, window, cx| act(&open, window, cx))
                    .child(img(image).size_full().object_fit(ObjectFit::Contain))
                    .into_any_element(),
            )
        }
        Asset::Video { path, poster } => {
            let act = ctx.act.clone();
            let open = Act::Video {
                name: name.clone(),
                path,
            };
            let (w, h) = poster
                .as_ref()
                .map_or(IMAGE_SLOT, |(_, size)| rich::fit(*size, IMAGE_MAX));
            let mut tile = slot((w, h))
                .id(element_id(&key, ""))
                .cursor_pointer()
                .relative()
                .on_click(move |_: &ClickEvent, window, cx| act(&open, window, cx));
            if let Some((image, _)) = poster {
                tile = tile.child(img(image).size_full().object_fit(ObjectFit::Cover));
            }
            column(
                tile.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(px(44.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(theme::white(0.9))
                                .child(theme::icon("play", 18.).text_color(theme::black(0.75))),
                        ),
                )
                .into_any_element(),
            )
        }
    }
}

/// A control that opens something in the viewer.
fn chip(
    id: ElementId,
    label: String,
    hint: Option<String>,
    act: Rc<dyn Fn(&Act, &mut Window, &mut App)>,
    open: Act,
) -> AnyElement {
    let mut row = div()
        .id(id)
        .h(px(26.))
        .px(px(10.))
        .rounded(px(crate::theme::RADIUS_MD))
        .border_1()
        .border_color(content(0.1))
        .bg(content(0.04))
        .hover(|s| s.bg(content(0.07)))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(7.))
        .text_size(px(12.5))
        .text_color(content(0.85))
        .on_click(move |_: &ClickEvent, window, cx| act(&open, window, cx))
        .child(theme::icon("globe", 13.).text_color(content(0.6)))
        .child(label);
    if let Some(hint) = hint {
        row = row.child(div().text_color(content(0.45)).child(hint));
    }
    div().flex().child(row).into_any_element()
}

/// "Abrir prévia" for an HTML file the reply links to.
pub fn html_file(url: &str, rich: Option<&Rich>) -> Option<AnyElement> {
    let ctx = rich?;
    let link = rich::media_path(url);
    Some(chip(
        element_id("html-file-", &link),
        t("Abrir prévia").to_owned(),
        Some(file_name(&link)),
        ctx.act.clone(),
        Act::HtmlFile {
            session: ctx.session.clone(),
            link,
        },
    ))
}

/// A fenced ```html block: its code with "Abrir prévia" above it, once the block is closed.
pub fn html_block(source: &str, code: AnyElement, rich: Option<&Rich>) -> AnyElement {
    let Some(ctx) = rich.filter(|ctx| !ctx.streaming) else {
        return code;
    };
    if rich::preview_page(source).is_none() {
        return code;
    }
    let key = Want::Diagram {
        source: source.to_owned(),
        light: false,
    }
    .key();
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(chip(
            element_id("html-block-", &key),
            t("Abrir prévia").to_owned(),
            Some("HTML".to_owned()),
            ctx.act.clone(),
            Act::Html {
                source: source.to_owned(),
            },
        ))
        .child(code)
        .into_any_element()
}

/// The drawing of a diagram or a block formula once it is ready; `code` (the source as the reply
/// wrote it) while it is not, when it failed, and while the reply is still streaming.
fn drawn_block(want: Want, code: AnyElement, rich: Option<&Rich>, center: bool) -> AnyElement {
    if rich.is_none() {
        return code;
    }
    match rich::asset(&want) {
        Asset::Picture { image, size } => {
            let (w, h) = rich::fit(size, (900., 2000.));
            let picture = div()
                .w(px(w))
                .h(px(h))
                .max_w_full()
                .flex_shrink_0()
                .child(img(image).size_full().object_fit(ObjectFit::Contain));
            div()
                .w_full()
                .flex()
                .when(center, |d| d.justify_center())
                .child(picture)
                .into_any_element()
        }
        _ => code,
    }
}

/// A fenced ```mermaid block. While the reply streams it stays code.
pub fn diagram(source: &str, code: AnyElement, rich: Option<&Rich>) -> AnyElement {
    if source.len() > crate::reply_render::MAX_SOURCE || rich.is_some_and(|ctx| ctx.streaming) {
        return code;
    }
    let want = Want::Diagram {
        source: source.to_owned(),
        light: theme::is_light(),
    };
    drawn_block(want, code, rich, false)
}

/// A block formula. `code` shows its source until the drawing is ready.
pub fn math_block(tex: &str, code: AnyElement, rich: Option<&Rich>) -> AnyElement {
    if tex.len() > 4000 {
        return code;
    }
    let want = Want::Math {
        tex: tex.to_owned(),
        display: true,
        light: theme::is_light(),
    };
    drawn_block(want, code, rich, true)
}

/// An inline formula's drawing: `Some(Some(..))` ready, `Some(None)` failed (keep the source),
/// `None` still being drawn.
pub fn inline_math(
    tex: &str,
    rich: Option<&Rich>,
) -> Option<Option<(std::sync::Arc<gpui::Image>, (f32, f32))>> {
    rich?;
    if tex.len() > 1000 {
        return Some(None);
    }
    let want = Want::Math {
        tex: tex.to_owned(),
        display: false,
        light: theme::is_light(),
    };
    match rich::asset(&want) {
        Asset::Picture { image, size } => Some(Some((image, size))),
        Asset::Pending => None,
        _ => Some(None),
    }
}

/// The picture of an inline formula, as tall as it was drawn.
pub fn inline_picture(image: std::sync::Arc<gpui::Image>, size: (f32, f32)) -> AnyElement {
    div()
        .w(px(size.0))
        .h(px(size.1))
        .flex_shrink_0()
        .child(img(image).size_full().object_fit(ObjectFit::Contain))
        .into_any_element()
}
