//! Markdown renderer: pulldown-cmark events -> GPUI elements.
//! Paragraphs and headings are one `StyledText` each (inline styles are
//! highlight ranges), so they wrap naturally.
use crate::theme::{accent, content};
use gpui::{
    AnyElement, Div, FontStyle, FontWeight, HighlightStyle, SharedString, StrikethroughStyle,
    StyledText, div, prelude::*, px,
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::iter::Peekable;
use std::ops::Range;

const BOLD: u8 = 1;
const ITALIC: u8 = 2;
const STRIKE: u8 = 4;
const LINK: u8 = 8;
const CODE: u8 = 16;

enum Block {
    Para(Inline),
    Heading(HeadingLevel, Inline),
    Code(String),
    Quote(Vec<Block>),
    List(Option<u64>, Vec<Vec<Block>>),
    Table(Vec<Inline>, Vec<Vec<Inline>>),
    Rule,
}

/// Plain text plus style flags over byte ranges. Spans may nest and overlap;
/// `finish` flattens them into the non-overlapping runs GPUI requires.
#[derive(Default)]
struct Inline {
    text: String,
    spans: Vec<(Range<usize>, u8)>,
}

pub fn render(text: &str) -> AnyElement {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let mut it = Parser::new_ext(text, options).peekable();
    let blocks = blocks(&mut it);
    stack(blocks, 10.)
        .text_size(px(14.))
        .line_height(px(23.))
        .text_color(content(0.88))
        .into_any_element()
}

fn is_inline(ev: &Event) -> bool {
    matches!(
        ev,
        Event::Text(_)
            | Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::Start(
                Tag::Emphasis
                    | Tag::Strong
                    | Tag::Strikethrough
                    | Tag::Link { .. }
                    | Tag::Image { .. }
            )
            | Event::End(
                TagEnd::Emphasis
                    | TagEnd::Strong
                    | TagEnd::Strikethrough
                    | TagEnd::Link
                    | TagEnd::Image
            )
    )
}

/// Consumes inline events up to the first block-level event or container end.
fn inline<'a, I: Iterator<Item = Event<'a>>>(it: &mut Peekable<I>) -> Inline {
    let mut inl = Inline::default();
    let mut open: Vec<(usize, u8)> = Vec::new();
    loop {
        match it.peek() {
            Some(ev) if is_inline(ev) => {}
            _ => break,
        }
        match it.next() {
            Some(Event::Text(t) | Event::Html(t) | Event::InlineHtml(t)) => inl.text.push_str(&t),
            Some(Event::Code(t)) => {
                let start = inl.text.len();
                inl.text.push_str(&t);
                inl.spans.push((start..inl.text.len(), CODE));
            }
            Some(Event::SoftBreak) => inl.text.push(' '),
            Some(Event::HardBreak) => inl.text.push('\n'),
            Some(Event::Start(tag)) => {
                let flag = match tag {
                    Tag::Strong => BOLD,
                    Tag::Emphasis => ITALIC,
                    Tag::Strikethrough => STRIKE,
                    Tag::Link { .. } => LINK,
                    _ => 0,
                };
                open.push((inl.text.len(), flag));
            }
            Some(Event::End(_)) => {
                if let Some((start, flag)) = open.pop() {
                    inl.spans.push((start..inl.text.len(), flag));
                }
            }
            _ => {}
        }
    }
    inl
}

fn blocks<'a, I: Iterator<Item = Event<'a>>>(it: &mut Peekable<I>) -> Vec<Block> {
    let mut out = Vec::new();
    loop {
        if matches!(it.peek(), None | Some(Event::End(_))) {
            break;
        }
        // Tight list items and bare text: a paragraph without a Paragraph tag.
        if matches!(it.peek(), Some(ev) if is_inline(ev)) {
            out.push(Block::Para(inline(it)));
            continue;
        }
        match it.next() {
            Some(Event::Rule) => out.push(Block::Rule),
            Some(Event::Start(Tag::Paragraph | Tag::HtmlBlock)) => {
                let p = inline(it);
                it.next();
                out.push(Block::Para(p));
            }
            Some(Event::Start(Tag::Heading { level, .. })) => {
                let p = inline(it);
                it.next();
                out.push(Block::Heading(level, p));
            }
            Some(Event::Start(Tag::BlockQuote(_))) => {
                let inner = blocks(it);
                it.next();
                out.push(Block::Quote(inner));
            }
            Some(Event::Start(Tag::CodeBlock(_))) => {
                let mut code = String::new();
                while let Some(Event::Text(t)) = it.peek() {
                    code.push_str(t);
                    it.next();
                }
                it.next();
                out.push(Block::Code(code));
            }
            Some(Event::Start(Tag::List(start))) => {
                let mut items = Vec::new();
                while matches!(it.peek(), Some(Event::Start(Tag::Item))) {
                    it.next();
                    items.push(blocks(it));
                    it.next();
                }
                it.next();
                out.push(Block::List(start, items));
            }
            Some(Event::Start(Tag::Table(_))) => {
                let (mut head, mut rows) = (Vec::new(), Vec::new());
                loop {
                    match it.next() {
                        Some(Event::Start(Tag::TableHead)) => head = cells(it),
                        Some(Event::Start(Tag::TableRow)) => rows.push(cells(it)),
                        _ => break,
                    }
                }
                out.push(Block::Table(head, rows));
            }
            _ => {}
        }
    }
    out
}

/// Reads cells of a TableHead or TableRow, consuming its end tag.
fn cells<'a, I: Iterator<Item = Event<'a>>>(it: &mut Peekable<I>) -> Vec<Inline> {
    let mut out = Vec::new();
    while let Some(ev) = it.next() {
        match ev {
            Event::Start(Tag::TableCell) => {
                out.push(inline(it));
                it.next();
            }
            Event::End(_) => break,
            _ => {}
        }
    }
    out
}

impl Inline {
    /// Flattens overlapping spans into sorted, non-overlapping highlight and
    /// font-family override ranges.
    fn finish(
        self,
    ) -> (
        String,
        Vec<(Range<usize>, HighlightStyle)>,
        Vec<(Range<usize>, SharedString)>,
    ) {
        let mut cuts = vec![0, self.text.len()];
        for (range, _) in &self.spans {
            cuts.push(range.start);
            cuts.push(range.end);
        }
        cuts.sort_unstable();
        cuts.dedup();
        let (mut highlights, mut fonts) = (Vec::new(), Vec::new());
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let flags = self
                .spans
                .iter()
                .filter(|(r, _)| r.start <= a && b <= r.end)
                .fold(0u8, |acc, (_, f)| acc | f);
            if a == b || flags == 0 {
                continue;
            }
            highlights.push((a..b, style(flags)));
            if flags & CODE != 0 {
                fonts.push((a..b, SharedString::from("Menlo")));
            }
        }
        (self.text, highlights, fonts)
    }
}

fn style(flags: u8) -> HighlightStyle {
    HighlightStyle {
        color: if flags & LINK != 0 {
            Some(accent())
        } else if flags & BOLD != 0 {
            Some(content(1.))
        } else {
            None
        },
        font_weight: (flags & BOLD != 0).then_some(FontWeight::SEMIBOLD),
        font_style: (flags & ITALIC != 0).then_some(FontStyle::Italic),
        background_color: (flags & CODE != 0).then(|| content(0.09)),
        strikethrough: (flags & STRIKE != 0).then(|| StrikethroughStyle {
            thickness: px(1.),
            color: None,
        }),
        ..Default::default()
    }
}

fn text(inl: Inline) -> StyledText {
    let (s, highlights, fonts) = inl.finish();
    StyledText::new(s)
        .with_highlights(highlights)
        .with_font_family_overrides(fonts)
}

fn stack(blocks: Vec<Block>, gap: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(gap))
        .children(blocks.into_iter().map(block))
}

fn block(b: Block) -> AnyElement {
    match b {
        Block::Para(p) => text(p).into_any_element(),
        Block::Heading(level, p) => {
            let size: f32 = match level {
                HeadingLevel::H1 => 18.,
                HeadingLevel::H2 => 16.,
                _ => 14.5,
            };
            div()
                .mt(px(6.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(size))
                .line_height(px(size * 1.3))
                .text_color(content(1.))
                .child(text(p))
                .into_any_element()
        }
        Block::Code(code) => div()
            .rounded(px(8.))
            .bg(content(0.05))
            .border_1()
            .border_color(content(0.08))
            .p(px(10.))
            .font_family("Menlo")
            .text_size(px(12.5))
            .line_height(px(19.))
            .child(StyledText::new(code.trim_end_matches('\n').to_string()))
            .into_any_element(),
        Block::Quote(inner) => div()
            .border_l_2()
            .border_color(content(0.15))
            .pl(px(12.))
            .text_color(content(0.6))
            .child(stack(inner, 10.))
            .into_any_element(),
        Block::List(start, items) => {
            let rows = items.into_iter().enumerate().map(|(i, item)| {
                let marker = match start {
                    Some(n) => format!("{}.", n + i as u64),
                    None => "•".to_string(),
                };
                // Nested lists sit in the content column, so each level adds one 18 px marker column.
                div()
                    .flex()
                    .flex_row()
                    .child(div().w(px(18.)).flex_shrink_0().child(marker))
                    .child(div().flex_1().min_w_0().child(stack(item, 4.)))
            });
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(rows)
                .into_any_element()
        }
        Block::Table(head, rows) => {
            let row = |cells: Vec<Inline>, header: bool| {
                div()
                    .flex()
                    .flex_row()
                    .border_b_1()
                    .border_color(content(0.08))
                    .when(header, |d| {
                        d.font_weight(FontWeight::SEMIBOLD).bg(content(0.04))
                    })
                    .children(cells.into_iter().map(|c| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(10.))
                            .py(px(6.))
                            .text_size(px(13.))
                            .child(text(c))
                    }))
            };
            div()
                .flex()
                .flex_col()
                .rounded(px(8.))
                .border_1()
                .border_color(content(0.08))
                .overflow_hidden()
                .child(row(head, true))
                .children(rows.into_iter().map(|r| row(r, false)))
                .into_any_element()
        }
        Block::Rule => div().w_full().h(px(1.)).bg(content(0.1)).into_any_element(),
    }
}
