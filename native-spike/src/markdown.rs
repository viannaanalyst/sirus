//! Markdown renderer: pulldown-cmark events -> GPUI elements.
//! Paragraphs and headings are one `StyledText` each (inline styles are
//! highlight ranges), so they wrap naturally.
use crate::theme::{accent, content};
use gpui::{
    AnyElement, Div, FontStyle, FontWeight, HighlightStyle, SharedString, StrikethroughStyle,
    StyledText, div, prelude::*, px,
};
use crate::reply_rich::{self as rich, MediaKind};
use crate::reply_view::{self as view, Rich};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::iter::Peekable;
use std::ops::Range;

const BOLD: u8 = 1;
const ITALIC: u8 = 2;
const STRIKE: u8 = 4;
const LINK: u8 = 8;
const CODE: u8 = 16;

#[derive(Clone)]
enum Block {
    Para(Inline),
    Heading(HeadingLevel, Inline),
    Code(String),
    Quote(Vec<Block>),
    List(Option<u64>, Vec<Vec<Block>>),
    Table(Vec<Inline>, Vec<Vec<Inline>>),
    Rule,
    /// An image or video a reply links to on the Mac, shown inline (reply_view).
    Media { url: String, alt: String, kind: MediaKind },
    /// A link to an HTML file: offers "Abrir prévia".
    HtmlFile(String),
    /// A fenced ```mermaid block.
    Diagram(String),
    /// A fenced ```html block.
    Html(String),
    /// A block formula (`$$ .. $$` or a ```math fence).
    Math(String),
}

/// A link target found inside a paragraph, shown after it.
#[derive(Clone)]
enum Extra {
    Media { url: String, alt: String, kind: MediaKind },
    HtmlFile(String),
}

/// A formula inside a paragraph: where its source sits in the text (shown as code until the
/// drawing is ready, or when it fails).
#[derive(Clone)]
struct Maths {
    range: Range<usize>,
    tex: String,
    display: bool,
}

/// Plain text plus style flags over byte ranges. Spans may nest and overlap;
/// `finish` flattens them into the non-overlapping runs GPUI requires.
#[derive(Default, Clone)]
struct Inline {
    text: String,
    spans: Vec<(Range<usize>, u8)>,
    extras: Vec<Extra>,
    maths: Vec<Maths>,
    /// Web links: the text they cover and the address they open (see `web_url`).
    links: Vec<(Range<usize>, String)>,
}

/// The address a link opens in the browser: only http and https, as written in the reply.
/// Other schemes (mailto, file, custom) are not clickable here; files keep their chips.
pub fn web_url(url: &str) -> Option<String> {
    let value = url.trim();
    let lower = value.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let plain = !value.chars().any(|c| c.is_whitespace() || c.is_control());
    (plain && !host.is_empty() && !host.starts_with(['.', '-', ':'])).then(|| value.to_owned())
}

/// The bare web addresses in plain text (`https://example.com/a`), trailing punctuation left out.
pub fn bare_urls(text: &str) -> Vec<Range<usize>> {
    let lower = text.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while from < lower.len() {
        let Some(at) = ["https://", "http://"]
            .iter()
            .filter_map(|scheme| lower[from..].find(scheme).map(|i| from + i))
            .min()
        else {
            break;
        };
        from = at + 1;
        let starts_word = text[..at].chars().next_back().is_some_and(char::is_alphanumeric);
        if starts_word {
            continue;
        }
        let run = text[at..]
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`'))
            .map_or(text.len(), |i| at + i);
        let mut end = run;
        loop {
            let candidate = &text[at..end];
            let Some(last) = candidate.chars().next_back() else { break };
            let unbalanced_close = last == ')'
                && candidate.matches(')').count() > candidate.matches('(').count();
            if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '*' | '_' | '~')
                || unbalanced_close
            {
                end -= last.len_utf8();
            } else {
                break;
            }
        }
        if web_url(&text[at..end]).is_some() {
            found.push(at..end);
            from = end;
        }
    }
    found
}

/// Makes the bare web addresses of a paragraph links, unless they are code or already a link.
fn autolink(inl: &mut Inline) {
    for range in bare_urls(&inl.text) {
        let overlaps = |r: &Range<usize>| r.start < range.end && range.start < r.end;
        let in_code = inl.spans.iter().any(|(r, f)| f & CODE != 0 && overlaps(r));
        if in_code || inl.links.iter().any(|(r, _)| overlaps(r)) {
            continue;
        }
        let url = inl.text[range.clone()].to_owned();
        inl.spans.push((range.clone(), LINK));
        inl.links.push((range, url));
    }
}

thread_local! {
    /// Parsed text by its hash: a frame only lays out what's on screen, and the words don't change.
    static PARSED: std::cell::RefCell<std::collections::HashMap<u64, Vec<Block>>> = Default::default();
}

fn parsed(text: &str) -> Vec<Block> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    let key = hasher.finish();
    PARSED.with_borrow_mut(|cache| {
        if let Some(found) = cache.get(&key) {
            return found.clone();
        }
        let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_MATH;
        let mut it = Parser::new_ext(text, options).peekable();
        let parsed = blocks(&mut it);
        if cache.len() >= 400 {
            cache.clear();
        }
        cache.insert(key, parsed.clone());
        parsed
    })
}

pub fn render(text: &str) -> AnyElement {
    draw(parsed(text), None)
}

/// A reply with its rich pieces: linked images and videos, diagrams, formulas and HTML previews.
/// A fence still open at the end of a streaming reply stays plain code.
pub fn render_rich(text: &str, rich: &Rich) -> AnyElement {
    let mut blocks = parsed(text);
    if rich.streaming && rich::ends_in_open_fence(text) {
        demote_last(&mut blocks);
    }
    draw(blocks, Some(rich))
}

/// The last block, if it is a diagram or an HTML page, back to the code the reply is writing.
fn demote_last(blocks: &mut [Block]) {
    match blocks.last_mut() {
        Some(Block::Quote(inner)) => demote_last(inner),
        Some(Block::List(_, items)) => {
            if let Some(last) = items.last_mut() {
                demote_last(last);
            }
        }
        Some(last @ (Block::Diagram(_) | Block::Html(_))) => {
            if let Block::Diagram(code) | Block::Html(code) = last {
                *last = Block::Code(std::mem::take(code));
            }
        }
        _ => {}
    }
}

fn draw(blocks: Vec<Block>, rich: Option<&Rich>) -> AnyElement {
    stack(blocks, 10., rich)
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
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
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
    let mut open: Vec<(usize, u8, Option<(String, bool)>)> = Vec::new();
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
            Some(Event::InlineMath(t)) => math_source(&mut inl, &t, false),
            Some(Event::DisplayMath(t)) => math_source(&mut inl, &t, true),
            Some(Event::Start(tag)) => {
                let (flag, dest) = match tag {
                    Tag::Strong => (BOLD, None),
                    Tag::Emphasis => (ITALIC, None),
                    Tag::Strikethrough => (STRIKE, None),
                    Tag::Link { dest_url, .. } => (LINK, Some((dest_url.to_string(), false))),
                    Tag::Image { dest_url, .. } => (0, Some((dest_url.to_string(), true))),
                    _ => (0, None),
                };
                open.push((inl.text.len(), flag, dest));
            }
            Some(Event::End(_)) => {
                if let Some((start, flag, dest)) = open.pop() {
                    let label = inl.text[start..].trim().to_owned();
                    match dest {
                        // A linked image the reply shows is taken out of the text: its alt text is the caption.
                        Some((url, true)) => {
                            if let Some(kind) = rich::local_media_kind(&url) {
                                inl.text.truncate(start);
                                inl.spans.retain(|(r, _)| r.end <= start);
                                inl.maths.retain(|m| m.range.end <= start);
                                inl.extras.push(Extra::Media { url, alt: label, kind });
                                continue;
                            }
                        }
                        // A link keeps its words; the file it names is shown after the paragraph.
                        Some((url, false)) => {
                            if let Some(web) = web_url(&url) {
                                inl.links.push((start..inl.text.len(), web));
                            }
                            if let Some(kind) = rich::local_media_kind(&url) {
                                inl.extras.push(Extra::Media { url, alt: label, kind });
                            } else if rich::is_html_file(&url) {
                                inl.extras.push(Extra::HtmlFile(url));
                            }
                        }
                        None => {}
                    }
                    inl.spans.push((start..inl.text.len(), flag));
                }
            }
            _ => {}
        }
    }
    inl
}

/// A formula's source stays in the text, as code, until its drawing replaces it.
fn math_source(inl: &mut Inline, tex: &str, display: bool) {
    let start = inl.text.len();
    let fence = if display { "$$" } else { "$" };
    inl.text.push_str(&format!("{fence}{tex}{fence}"));
    let range = start..inl.text.len();
    inl.spans.push((range.clone(), CODE));
    inl.maths.push(Maths { range, tex: tex.trim().to_owned(), display });
}

/// A paragraph, then what it links to: block formulas split it, media and pages follow it.
fn push_paragraph(out: &mut Vec<Block>, mut p: Inline) {
    let extras = std::mem::take(&mut p.extras);
    let displays: Vec<Maths> = p.maths.iter().filter(|m| m.display).cloned().collect();
    if displays.is_empty() {
        if !p.text.trim().is_empty() || extras.is_empty() {
            out.push(Block::Para(p));
        }
    } else {
        let mut at = 0;
        for m in displays {
            let before = p.slice(at..m.range.start);
            if !before.text.trim().is_empty() {
                out.push(Block::Para(before));
            }
            out.push(Block::Math(m.tex));
            at = m.range.end;
        }
        let rest = p.slice(at..p.text.len());
        if !rest.text.trim().is_empty() {
            out.push(Block::Para(rest));
        }
    }
    out.extend(extras.into_iter().map(|extra| match extra {
        Extra::Media { url, alt, kind } => Block::Media { url, alt, kind },
        Extra::HtmlFile(url) => Block::HtmlFile(url),
    }));
}

fn blocks<'a, I: Iterator<Item = Event<'a>>>(it: &mut Peekable<I>) -> Vec<Block> {
    let mut out = Vec::new();
    loop {
        if matches!(it.peek(), None | Some(Event::End(_))) {
            break;
        }
        // Tight list items and bare text: a paragraph without a Paragraph tag.
        if matches!(it.peek(), Some(ev) if is_inline(ev)) {
            let p = inline(it);
            push_paragraph(&mut out, p);
            continue;
        }
        match it.next() {
            Some(Event::Rule) => out.push(Block::Rule),
            Some(Event::Start(Tag::Paragraph | Tag::HtmlBlock)) => {
                let p = inline(it);
                it.next();
                push_paragraph(&mut out, p);
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
            Some(Event::Start(Tag::CodeBlock(kind))) => {
                let lang = match &kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase(),
                    CodeBlockKind::Indented => String::new(),
                };
                let mut code = String::new();
                while let Some(Event::Text(t)) = it.peek() {
                    code.push_str(t);
                    it.next();
                }
                it.next();
                out.push(match lang.as_str() {
                    "mermaid" | "mmd" => Block::Diagram(code),
                    "html" | "htm" => Block::Html(code),
                    "math" | "katex" => Block::Math(code),
                    _ => Block::Code(code),
                });
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
    /// The part of the text in `r`, with the styles and formulas that fall inside it.
    fn slice(&self, r: Range<usize>) -> Inline {
        let inside = |s: &Range<usize>| {
            let (a, b) = (s.start.max(r.start), s.end.min(r.end));
            (a < b).then(|| a - r.start..b - r.start)
        };
        Inline {
            text: self.text[r.clone()].to_owned(),
            spans: self
                .spans
                .iter()
                .filter_map(|(s, f)| inside(s).map(|s| (s, *f)))
                .collect(),
            extras: Vec::new(),
            maths: self
                .maths
                .iter()
                .filter(|m| r.start <= m.range.start && m.range.end <= r.end)
                .map(|m| Maths {
                    range: m.range.start - r.start..m.range.end - r.start,
                    ..m.clone()
                })
                .collect(),
            links: self
                .links
                .iter()
                .filter_map(|(s, url)| inside(s).map(|s| (s, url.clone())))
                .collect(),
        }
    }

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

fn text(mut inl: Inline) -> AnyElement {
    autolink(&mut inl);
    let links = std::mem::take(&mut inl.links);
    let (s, highlights, fonts) = inl.finish();
    let s = SharedString::from(s);
    let styled = StyledText::new(s.clone())
        .with_highlights(highlights)
        .with_font_family_overrides(fonts);
    crate::shell::transcript_select::wrap_links(styled, s, links)
}

fn stack(blocks: Vec<Block>, gap: f32, rich: Option<&Rich>) -> Div {
    div().flex().flex_col().gap(px(gap)).children(
        blocks
            .into_iter()
            // A page link means nothing where replies have no viewer to open it in.
            .filter(|b| rich.is_some() || !matches!(b, Block::HtmlFile(_)))
            .map(|b| block(b, rich)),
    )
}

/// A paragraph with formulas, once every formula is drawn (or failed): words and pictures in a
/// wrapping row. While one is still being drawn the paragraph stays text, formulas as code.
fn math_para(p: Inline, rich: Option<&Rich>) -> AnyElement {
    let mut drawn = Vec::new();
    for m in &p.maths {
        match view::inline_math(&m.tex, rich) {
            Some(result) => drawn.push(result),
            None => return text(p),
        }
    }
    if drawn.iter().all(Option::is_none) {
        return text(p);
    }
    let mut items: Vec<AnyElement> = Vec::new();
    let mut at = 0;
    for (m, picture) in p.maths.iter().zip(drawn) {
        push_words(&mut items, p.slice(at..m.range.start));
        match picture {
            Some((image, size)) => items.push(view::inline_picture(image, size)),
            None => push_words(&mut items, p.slice(m.range.clone())),
        }
        at = m.range.end;
    }
    push_words(&mut items, p.slice(at..p.text.len()));
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .children(items)
        .into_any_element()
}

/// One element per word, each keeping its trailing space, so the row wraps between words.
fn push_words(items: &mut Vec<AnyElement>, piece: Inline) {
    let mut start = 0;
    let spaces: Vec<usize> = piece.text.match_indices(' ').map(|(i, _)| i + 1).collect();
    for end in spaces {
        items.push(text(piece.slice(start..end)));
        start = end;
    }
    if start < piece.text.len() {
        items.push(text(piece.slice(start..piece.text.len())));
    }
}

fn block(b: Block, rich: Option<&Rich>) -> AnyElement {
    match b {
        Block::Para(p) if rich.is_some() && !p.maths.is_empty() => math_para(p, rich),
        Block::Para(p) => text(p),
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
        Block::Code(code) => code_block(&code),
        Block::Media { url, alt, kind } => view::media(&url, &alt, kind, rich),
        Block::HtmlFile(url) => view::html_file(&url, rich).unwrap_or_else(|| div().into_any_element()),
        Block::Diagram(source) => view::diagram(&source, code_block(&source), rich),
        Block::Html(source) => view::html_block(&source, code_block(&source), rich),
        Block::Math(tex) => {
            let fallback = format!("$$\n{}\n$$", tex.trim());
            view::math_block(&tex, code_block(&fallback), rich)
        }
        Block::Quote(inner) => div()
            .border_l_2()
            .border_color(content(0.15))
            .pl(px(12.))
            .text_color(content(0.6))
            .child(stack(inner, 10., rich))
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
                    .child(div().min_w(px(18.)).flex_shrink_0().whitespace_nowrap().child(marker))
                    .child(div().flex_1().min_w_0().child(stack(item, 4., rich)))
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
                .rounded(px(crate::theme::RADIUS_MD))
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


fn code_block(code: &str) -> AnyElement {
    div()
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(content(0.05))
            .border_1()
            .border_color(content(0.08))
            .p(px(10.))
            .font_family("Menlo")
            .text_size(px(12.5))
            .line_height(px(19.))
            .child({
                let code = SharedString::from(code.trim_end_matches('\n').to_string());
                crate::shell::transcript_select::wrap(StyledText::new(code.clone()), code)
            })
            .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks_of(text: &str) -> Vec<Block> {
        let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_MATH;
        blocks(&mut Parser::new_ext(text, options).peekable())
    }

    fn kinds(blocks: &[Block]) -> Vec<&'static str> {
        blocks
            .iter()
            .map(|b| match b {
                Block::Para(_) => "para",
                Block::Media { kind: MediaKind::Image, .. } => "image",
                Block::Media { kind: MediaKind::Video, .. } => "video",
                Block::HtmlFile(_) => "html-file",
                Block::Diagram(_) => "diagram",
                Block::Html(_) => "html",
                Block::Math(_) => "math",
                Block::Code(_) => "code",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn a_linked_image_leaves_the_text_and_becomes_a_block() {
        let found = blocks_of("![the shot](/tmp/shot.png)");
        assert_eq!(kinds(&found), ["image"]);
        let Block::Media { url, alt, .. } = &found[0] else { panic!() };
        assert_eq!((url.as_str(), alt.as_str()), ("/tmp/shot.png", "the shot"));
        // Remote images are not fetched: their alt text stays.
        assert_eq!(kinds(&blocks_of("![x](https://a.com/x.png)")), ["para"]);
    }

    #[test]
    fn links_keep_their_words_and_add_what_they_name() {
        let found = blocks_of("See [the clip](out/demo.mp4) and [page](site/index.html).");
        assert_eq!(kinds(&found), ["para", "video", "html-file"]);
        let Block::Para(p) = &found[0] else { panic!() };
        assert_eq!(p.text, "See the clip and page.");
        assert_eq!(kinds(&blocks_of("[docs](https://a.com/index.html)")), ["para"]);
    }

    #[test]
    fn fences_are_told_apart_by_language() {
        let found = blocks_of("```mermaid\ngraph TD\n```\n\n```html\n<p>x</p>\n```\n\n```rust\nfn a() {}\n```\n\n```math\nx^2\n```");
        assert_eq!(kinds(&found), ["diagram", "html", "code", "math"]);
    }

    #[test]
    fn formulas_stay_as_code_in_the_text_and_block_ones_split_the_paragraph() {
        let found = blocks_of("Euler: $e^{i\\pi}+1=0$ done.");
        let Block::Para(p) = &found[0] else { panic!() };
        assert_eq!(p.text, "Euler: $e^{i\\pi}+1=0$ done.");
        assert_eq!(p.maths.len(), 1);
        assert_eq!(&p.text[p.maths[0].range.clone()], "$e^{i\\pi}+1=0$");
        assert!(!p.maths[0].display);
        let found = blocks_of("Before\n\n$$\nx^2\n$$\n\nAfter");
        assert_eq!(kinds(&found), ["para", "math", "para"]);
        // Money is not math.
        let found = blocks_of("It costs $5 and $10.");
        let Block::Para(p) = &found[0] else { panic!() };
        assert!(p.maths.is_empty());
    }

    #[test]
    fn slicing_keeps_styles_and_formula_positions() {
        let found = blocks_of("a **bold** $x$ z");
        let Block::Para(p) = &found[0] else { panic!() };
        let tail = p.slice(2..p.text.len());
        assert_eq!(tail.text, "bold $x$ z");
        assert_eq!(tail.maths[0].range, 5..8);
        assert!(tail.spans.iter().any(|(r, f)| *r == (0..4) && f & BOLD != 0));
    }

    #[test]
    fn a_streaming_fence_goes_back_to_code() {
        let mut found = blocks_of("```mermaid\ngraph TD\n  A --> B");
        demote_last(&mut found);
        assert_eq!(kinds(&found), ["code"]);
        let mut found = blocks_of("- item\n\n  ```html\n  <p>\n");
        demote_last(&mut found);
        let Block::List(_, items) = &found[0] else { panic!() };
        assert_eq!(kinds(&items[0]), ["para", "code"]);
    }
    #[test]
    fn only_http_and_https_open_directly() {
        assert_eq!(web_url("https://example.com/a?b=1").as_deref(), Some("https://example.com/a?b=1"));
        assert_eq!(web_url(" http://x.org ").as_deref(), Some("http://x.org"));
        assert_eq!(web_url("HTTPS://Example.com").as_deref(), Some("HTTPS://Example.com"));
        for url in [
            "mailto:a@b.c",
            "file:///tmp/x.html",
            "javascript:alert(1)",
            "ftp://x.org",
            "/Users/me/a.png",
            "https://",
            "https:///path",
            "https://exa mple.com",
            "https://.bad",
        ] {
            assert_eq!(web_url(url), None, "{url}");
        }
    }

    #[test]
    fn bare_urls_stop_before_punctuation() {
        let text = "veja https://example.com/a?b=1. Depois (http://x.org/a_(b)) e xhttps://no.";
        let found: Vec<&str> = bare_urls(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(found, ["https://example.com/a?b=1", "http://x.org/a_(b)"]);
        assert!(bare_urls("sem link aqui, ftp://x.org").is_empty());
    }

    #[test]
    fn autolink_keeps_code_and_markdown_links_as_they_are() {
        let found = blocks_of("ver `https://a.com` e [site](https://b.com) e https://c.com");
        let Block::Para(p) = &found[0] else { panic!() };
        let mut inl = p.clone();
        autolink(&mut inl);
        let urls: Vec<&str> = inl.links.iter().map(|(_, url)| url.as_str()).collect();
        assert_eq!(urls, ["https://b.com", "https://c.com"]);
        assert_eq!(&inl.text[inl.links[1].0.clone()], "https://c.com");
    }
}
