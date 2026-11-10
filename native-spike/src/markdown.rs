//! Markdown renderer: pulldown-cmark events -> GPUI elements.
//! Paragraphs and headings are one `StyledText` each (inline styles are
//! highlight ranges), so they wrap naturally.
use crate::reply_rich::{self as rich, MediaKind};
use crate::reply_view::{self as view, Rich};
use crate::theme::{accent, content};
use gpui::{
    AnyElement, Div, FontStyle, FontWeight, HighlightStyle, SharedString, StrikethroughStyle,
    StyledText, div, prelude::*, px,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::iter::Peekable;
use std::ops::Range;

const BOLD: u8 = 1;
const ITALIC: u8 = 2;
const STRIKE: u8 = 4;
const LINK: u8 = 8;
const CODE: u8 = 16;
const SEARCH: u8 = 32;
const SEARCH_NOW: u8 = 64;

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
    Media {
        url: String,
        alt: String,
        kind: MediaKind,
    },
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
    Media {
        url: String,
        alt: String,
        kind: MediaKind,
    },
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
        let starts_word = text[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        if starts_word {
            continue;
        }
        let run = text[at..]
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`'))
            .map_or(text.len(), |i| at + i);
        let mut end = run;
        loop {
            let candidate = &text[at..end];
            let Some(last) = candidate.chars().next_back() else {
                break;
            };
            let unbalanced_close =
                last == ')' && candidate.matches(')').count() > candidate.matches('(').count();
            if matches!(
                last,
                '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '*' | '_' | '~'
            ) || unbalanced_close
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
    let outer = RICH.take();
    let drawn = draw(parsed(text), None);
    RICH.set(outer);
    drawn
}

/// A reply with its rich pieces: linked images and videos, diagrams, formulas and HTML previews.
/// A fence still open at the end of a streaming reply stays plain code.
pub fn render_rich(text: &str, rich: &Rich) -> AnyElement {
    let mut blocks = parsed(text);
    if rich.streaming && rich::ends_in_open_fence(text) {
        demote_last(&mut blocks);
    }
    let outer = RICH.replace(Some(rich.clone()));
    let drawn = draw(blocks, Some(rich));
    RICH.set(outer);
    drawn
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
    stack(blocks, 16., rich)
        .text_size(px(14.))
        .line_height(px(24.))
        // The reading contrast: prose a step back, emphasis at full ink.
        .text_color(content(0.78))
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
                                inl.extras.push(Extra::Media {
                                    url,
                                    alt: label,
                                    kind,
                                });
                                continue;
                            }
                        }
                        // A link keeps its words; the file it names is shown after the paragraph.
                        Some((url, false)) => {
                            if let Some(web) = web_url(&url) {
                                inl.links.push((start..inl.text.len(), web));
                            }
                            if let Some(kind) = rich::local_media_kind(&url) {
                                inl.extras.push(Extra::Media {
                                    url,
                                    alt: label,
                                    kind,
                                });
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
    inl.maths.push(Maths {
        range,
        tex: tex.trim().to_owned(),
        display,
    });
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

/// A found word's fill: the accent on the current turn, amber on the others.
fn search_fill(flags: u8) -> Option<gpui::Hsla> {
    if flags & SEARCH_NOW != 0 {
        Some(gpui::Hsla {
            a: 0.62,
            ..accent()
        })
    } else if flags & SEARCH != 0 {
        let amber: gpui::Hsla = gpui::rgb(0xe2c08d).into();
        Some(gpui::Hsla { a: 0.46, ..amber })
    } else {
        None
    }
}

/// What ⌘F marks in the turn being drawn: the query, which occurrence of the turn is the current
/// one (the prompt's come first, then the reply's, in drawing order), and how many the prompt has.
struct Find {
    query: String,
    current: Option<usize>,
    prompt: usize,
    prompt_seen: usize,
    reply_seen: usize,
}

thread_local! {
    static FIND: std::cell::RefCell<Option<Find>> = const { std::cell::RefCell::new(None) };
    /// The reply being drawn, for the file chips in its text.
    static RICH: std::cell::RefCell<Option<Rich>> = const { std::cell::RefCell::new(None) };
}

/// What ⌘F marks in the text drawn next: the query, the turn's current occurrence (an index among
/// the turn's occurrences, if the current hit is in this turn) and the prompt's occurrence count.
pub fn set_search(query: Option<(&str, Option<usize>, usize)>) {
    FIND.with_borrow_mut(|find| {
        *find = query.map(|(text, current, prompt)| Find {
            query: text.to_owned(),
            current,
            prompt,
            prompt_seen: 0,
            reply_seen: 0,
        })
    });
}

/// The flag of the next occurrence drawn: the current one is stronger than the rest.
fn next_flag(find: &mut Find, prompt: bool) -> u8 {
    let nth = if prompt {
        find.prompt_seen += 1;
        find.prompt_seen - 1
    } else {
        find.reply_seen += 1;
        find.prompt + find.reply_seen - 1
    };
    if find.current == Some(nth) {
        SEARCH_NOW
    } else {
        SEARCH
    }
}

/// Every occurrence of `query` the reply `source` draws, in drawing order (what `mark_search` and
/// `code_block` will mark).
pub fn count_matches(source: &str, query: &str) -> usize {
    fn walk(blocks: &[Block], query: &str) -> usize {
        blocks
            .iter()
            .map(|block| match block {
                Block::Para(p) | Block::Heading(_, p) => {
                    crate::shell::transcript_find::ranges(&p.text, query).len()
                }
                Block::Code(code) => {
                    crate::shell::transcript_find::ranges(code.trim_end_matches('\n'), query).len()
                }
                Block::Quote(inner) => walk(inner, query),
                Block::List(_, items) => items.iter().map(|item| walk(item, query)).sum(),
                Block::Table(head, rows) => head
                    .iter()
                    .chain(rows.iter().flatten())
                    .map(|cell| crate::shell::transcript_find::ranges(&cell.text, query).len())
                    .sum(),
                _ => 0,
            })
            .sum()
    }
    walk(&parsed(source), query)
}

/// A found word's fill for the prompt's text, drawn outside the markdown.
pub fn search_ranges(text: &str) -> Vec<(Range<usize>, gpui::Hsla)> {
    FIND.with_borrow_mut(|find| {
        let Some(find) = find else {
            return Vec::new();
        };
        crate::shell::transcript_find::ranges(text, &find.query)
            .into_iter()
            .filter_map(|range| {
                let flag = next_flag(find, true);
                search_fill(flag).map(|fill| (range, fill))
            })
            .collect()
    })
}

fn mark_search(inl: &mut Inline) {
    FIND.with_borrow_mut(|find| {
        if let Some(find) = find {
            for range in crate::shell::transcript_find::ranges(&inl.text, &find.query) {
                let flag = next_flag(find, false);
                inl.spans.push((range, flag));
            }
        }
    });
}

fn style(flags: u8) -> HighlightStyle {
    HighlightStyle {
        color: if flags & LINK != 0 {
            Some(accent())
        } else if flags & (BOLD | ITALIC | CODE) != 0 {
            Some(content(1.))
        } else {
            None
        },
        font_weight: (flags & BOLD != 0).then_some(FontWeight::SEMIBOLD),
        font_style: (flags & ITALIC != 0).then_some(FontStyle::Italic),
        background_color: search_fill(flags),
        strikethrough: (flags & STRIKE != 0).then(|| StrikethroughStyle {
            thickness: px(1.),
            color: None,
        }),
        ..Default::default()
    }
}

fn text(mut inl: Inline) -> AnyElement {
    autolink(&mut inl);
    mark_search(&mut inl);
    if inl.spans.iter().any(|(_, flags)| flags & CODE != 0) {
        return flowing(inl);
    }
    let links = std::mem::take(&mut inl.links);
    let (s, highlights, fonts) = inl.finish();
    let s = SharedString::from(s);
    let styled = StyledText::new(s.clone())
        .with_highlights(highlights)
        .with_font_family_overrides(fonts);
    crate::shell::transcript_select::wrap_links(styled, s, links)
}

/// A file named in code gets its icon in the pill.
fn is_file_chip(code: &str) -> bool {
    !code.chars().any(char::is_whitespace)
        && code
            .rsplit('/')
            .next()
            .is_some_and(|name| name.contains('.'))
}

/// The address an inline code span opens when the whole span is one web address
/// (`` `http://127.0.0.1:8771/` ``): the span's text must be that address and nothing else.
fn code_url(code: &str) -> Option<String> {
    if code != code.trim() {
        return None;
    }
    web_url(code)
}

/// A paragraph with inline code: words flow and wrap one by one, and each piece of code is a pill
///, which a single run of text cannot draw. The pieces still read and
/// select as one paragraph (`wrap_piece`).
fn flowing(inl: Inline) -> AnyElement {
    let mut cuts = vec![0, inl.text.len()];
    for (range, _) in &inl.spans {
        cuts.push(range.start);
        cuts.push(range.end);
    }
    for (range, _) in &inl.links {
        cuts.push(range.start);
        cuts.push(range.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let flags_at = |a: usize, b: usize| {
        inl.spans
            .iter()
            .filter(|(r, _)| r.start <= a && b <= r.end)
            .fold(0u8, |acc, (_, f)| acc | f)
    };
    // Runs of one style, each split further at line breaks.
    let mut lines: Vec<Vec<(String, u8, Option<String>)>> = vec![Vec::new()];
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a == b {
            continue;
        }
        let flags = flags_at(a, b);
        let link = inl
            .links
            .iter()
            .find(|(r, _)| r.start <= a && b <= r.end)
            .map(|(_, url)| url.clone());
        for (at, part) in inl.text[a..b].split('\n').enumerate() {
            if at > 0 {
                lines.push(Vec::new());
            }
            if !part.is_empty() {
                lines.last_mut().expect("starts with one line").push((
                    part.to_owned(),
                    flags,
                    link.clone(),
                ));
            }
        }
    }
    let line_count = lines.len();
    let rows = lines.into_iter().enumerate().map(|(line_at, runs)| {
        // Every piece but the line's last reads on into the next one when copied.
        let mut pieces: Vec<(String, u8, Option<String>, bool)> = Vec::new();
        for (text, flags, link) in runs {
            if flags & CODE != 0 {
                pieces.push((text, flags, link, true));
            } else {
                for word in text.split_inclusive(char::is_whitespace) {
                    pieces.push((word.to_owned(), flags, link.clone(), false));
                }
            }
        }
        let count = pieces.len();
        let _ = line_count;
        let _ = line_at;
        let children = pieces
            .into_iter()
            .enumerate()
            .map(|(at, (text, flags, link, code))| {
                let glue = at + 1 < count;
                let shared = SharedString::from(text);
                let len = shared.len();
                let link = link.or_else(|| if code { code_url(&shared) } else { None });
                let opens_link = code && link.is_some();
                let links = link.map(|url| vec![(0..len, url)]).unwrap_or_default();
                let highlight = if code {
                    HighlightStyle {
                        color: Some(content(1.)),
                        background_color: search_fill(flags),
                        ..Default::default()
                    }
                } else {
                    style(flags)
                };
                let styled = StyledText::new(shared.clone()).with_highlights([(0..len, highlight)]);
                let piece = crate::shell::transcript_select::wrap_piece(
                    styled,
                    shared.clone(),
                    links,
                    glue,
                );
                if !code {
                    return div()
                        .flex_shrink_0()
                        .max_w_full()
                        .child(piece)
                        .into_any_element();
                }
                let opens = is_file_chip(&shared)
                    .then(|| RICH.with_borrow(Clone::clone))
                    .flatten()
                    .and_then(|rich| {
                        let target = crate::file_chips::target(&rich.cwd, &shared)?;
                        Some((rich.act, target))
                    });
                div()
                    .relative()
                    .flex()
                    .flex_shrink_0()
                    .max_w_full()
                    .items_center()
                    .gap(px(4.))
                    .min_h(px(24.))
                    .mx(px(2.))
                    .px(px(6.))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .bg(content(0.08))
                    // A web address in code opens like a link: pointing hand and a tint.
                    .when(opens_link, |d| {
                        d.cursor_pointer().hover(|s| s.bg(content(0.14)))
                    })
                    .font_family("Menlo")
                    .text_size(px(11.2))
                    .line_height(px(24.))
                    .when(is_file_chip(&shared), |d| {
                        d.child(crate::file_icons::file_icon(&shared, false, 14.))
                    })
                    .child(piece)
                    // Over the text, whose wrapper keeps the I-beam, and under no one's press: a
                    // drag from here still selects.
                    .when_some(opens, |d, (act, target)| {
                        let act_on = rich::Act::OpenPath(target);
                        d.child(
                            div()
                                .id(SharedString::from(format!("file-chip-{at}-{shared}")))
                                .absolute()
                                .inset_0()
                                .rounded(px(crate::theme::RADIUS_SM))
                                .cursor_pointer()
                                .hover(|s| s.bg(content(0.06)))
                                .on_click(move |event, window, cx| {
                                    if crate::file_chips::plain_click(event) {
                                        act(&act_on, window, cx);
                                    }
                                }),
                        )
                    })
                    .into_any_element()
            });
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .min_w_0()
            .children(children)
    });
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .children(rows)
        .into_any_element()
}

/// The space above a block in a stack: 24 around a rule, 8 above a list, else the stack's gap.
fn gap_above(prev: Option<&Block>, next: &Block, gap: f32) -> f32 {
    if matches!(next, Block::Rule) || matches!(prev, Some(Block::Rule)) {
        24.
    } else if matches!(next, Block::List(..)) {
        8.
    } else {
        gap
    }
}

fn stack(blocks: Vec<Block>, gap: f32, rich: Option<&Rich>) -> Div {
    // A page link means nothing where replies have no viewer to open it in.
    let blocks: Vec<Block> = blocks
        .into_iter()
        .filter(|b| rich.is_some() || !matches!(b, Block::HtmlFile(_)))
        .collect();
    // Each block's space above, as an offset from the stack's gap.
    let offsets: Vec<f32> = blocks
        .iter()
        .enumerate()
        .map(|(at, b)| gap_above(at.checked_sub(1).map(|prev| &blocks[prev]), b, gap) - gap)
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(px(gap))
        .children(
            blocks
                .into_iter()
                .zip(offsets)
                .enumerate()
                .map(|(at, (b, offset))| {
                    let el = block(b, rich);
                    // Only a block that needs more or less room than the gap is wrapped to shift it.
                    if at == 0 || offset == 0. {
                        el
                    } else {
                        div().mt(px(offset)).child(el).into_any_element()
                    }
                }),
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
                HeadingLevel::H1 => 22.,
                HeadingLevel::H2 => 18.,
                _ => 16.,
            };
            div()
                // With the stack's 16 between blocks, 24 above a heading.
                .mt(px(8.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(size))
                .line_height(px(if size >= 22. { 30. } else { 27. }))
                .text_color(content(1.))
                .child(text(p))
                .into_any_element()
        }
        Block::Code(code) => code_block(&code),
        Block::Media { url, alt, kind } => view::media(&url, &alt, kind, rich),
        Block::HtmlFile(url) => {
            view::html_file(&url, rich).unwrap_or_else(|| div().into_any_element())
        }
        Block::Diagram(source) => view::diagram(&source, code_block_marked(&source, false), rich),
        Block::Html(source) => view::html_block(&source, code_block_marked(&source, false), rich),
        Block::Math(tex) => {
            let fallback = format!("$$\n{}\n$$", tex.trim());
            view::math_block(&tex, code_block_marked(&fallback, false), rich)
        }
        Block::Quote(inner) => div()
            .ml(px(4.))
            .pl(px(16.))
            .border_l_4()
            .border_color(content(0.2))
            .child(stack(inner, 16., rich))
            .into_any_element(),
        Block::List(start, items) => {
            let rows = items.into_iter().enumerate().map(|(i, item)| {
                let marker = match start {
                    Some(n) => format!("{}.", n + i as u64),
                    None => "•".to_string(),
                };
                // Nested lists sit in the content column, so each level adds one 24 px marker column.
                div()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(24.))
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .child(marker),
                    )
                    .child(div().flex_1().min_w_0().child(stack(item, 16., rich)))
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
                            .py(px(8.))
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .child(text(c))
                    }))
            };
            div()
                .flex()
                .flex_col()
                .rounded(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_color(content(0.1))
                .bg(content(0.06))
                .overflow_hidden()
                .child(row(head, true))
                .children(rows.into_iter().map(|r| row(r, false)))
                .into_any_element()
        }
        Block::Rule => div().w_full().h(px(1.)).bg(content(0.1)).into_any_element(),
    }
}

fn code_block(code: &str) -> AnyElement {
    code_block_marked(code, true)
}

/// `marked` is false for a code block that only stands in for something else (a diagram's source).
fn code_block_marked(code: &str, marked: bool) -> AnyElement {
    let block = code_block_body(code, marked);
    if !marked {
        return block;
    }
    // A copy button in the corner, on hover; the group is named by the text so blocks don't share it.
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hasher);
    let hash = hasher.finish();
    let group: SharedString = format!("code-copy-{hash:x}").into();
    let text = code.trim_end_matches('\n').to_owned();
    let copied = COPIED.with(|c| {
        c.get()
            .is_some_and(|(h, at)| h == hash && at.elapsed() < COPIED_FOR)
    });
    div()
        .relative()
        .group(group.clone())
        .child(block)
        .child(
            div()
                .id(group.clone())
                .absolute()
                .top(px(6.))
                .right(px(6.))
                .size(px(24.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .justify_center()
                .text_color(content(if copied { 0.9 } else { 0.5 }))
                .bg(content(0.06))
                .when(!copied, |d| d.invisible())
                .group_hover(group, |s| s.visible())
                .cursor_pointer()
                .hover(|s| s.bg(content(0.12)).text_color(content(0.9)))
                .child(crate::theme::icon(
                    if copied { "check" } else { "copy" },
                    13.,
                ))
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                    COPIED.with(|c| c.set(Some((hash, std::time::Instant::now()))));
                    cx.refresh_windows();
                    cx.spawn(async move |cx| {
                        cx.background_executor().timer(COPIED_FOR).await;
                        let _ = cx.update(|cx| cx.refresh_windows());
                    })
                    .detach();
                }),
        )
        .into_any_element()
}

/// How long a code block's copy button shows its check mark.
const COPIED_FOR: std::time::Duration = std::time::Duration::from_millis(1400);

thread_local! {
    /// The code block copied last and when.
    static COPIED: std::cell::Cell<Option<(u64, std::time::Instant)>> =
        const { std::cell::Cell::new(None) };
}

fn code_block_body(code: &str, marked: bool) -> AnyElement {
    div()
        .rounded(px(crate::theme::RADIUS_MD))
        .bg(content(0.05))
        .border_1()
        .border_color(crate::theme::border())
        .p(px(10.))
        .font_family("Menlo")
        .text_size(px(12.))
        .line_height(px(20.))
        .child({
            let code = SharedString::from(code.trim_end_matches('\n').to_string());
            let found: Vec<(Range<usize>, HighlightStyle)> = if marked {
                FIND.with_borrow_mut(|find| {
                    let Some(find) = find else {
                        return Vec::new();
                    };
                    crate::shell::transcript_find::ranges(&code, &find.query)
                        .into_iter()
                        .map(|range| {
                            let flag = next_flag(find, false);
                            (
                                range,
                                HighlightStyle {
                                    background_color: search_fill(flag),
                                    ..Default::default()
                                },
                            )
                        })
                        .collect()
                })
            } else {
                Vec::new()
            };
            crate::shell::transcript_select::wrap(
                StyledText::new(code.clone()).with_highlights(found),
                code,
            )
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
                Block::Media {
                    kind: MediaKind::Image,
                    ..
                } => "image",
                Block::Media {
                    kind: MediaKind::Video,
                    ..
                } => "video",
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
        let Block::Media { url, alt, .. } = &found[0] else {
            panic!()
        };
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
        assert_eq!(
            kinds(&blocks_of("[docs](https://a.com/index.html)")),
            ["para"]
        );
    }

    #[test]
    fn fences_are_told_apart_by_language() {
        let found = blocks_of(
            "```mermaid\ngraph TD\n```\n\n```html\n<p>x</p>\n```\n\n```rust\nfn a() {}\n```\n\n```math\nx^2\n```",
        );
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
        assert!(
            tail.spans
                .iter()
                .any(|(r, f)| *r == (0..4) && f & BOLD != 0)
        );
    }

    #[test]
    fn a_streaming_fence_goes_back_to_code() {
        let mut found = blocks_of("```mermaid\ngraph TD\n  A --> B");
        demote_last(&mut found);
        assert_eq!(kinds(&found), ["code"]);
        let mut found = blocks_of("- item\n\n  ```html\n  <p>\n");
        demote_last(&mut found);
        let Block::List(_, items) = &found[0] else {
            panic!()
        };
        assert_eq!(kinds(&items[0]), ["para", "code"]);
    }
    #[test]
    fn only_http_and_https_open_directly() {
        assert_eq!(
            web_url("https://example.com/a?b=1").as_deref(),
            Some("https://example.com/a?b=1")
        );
        assert_eq!(web_url(" http://x.org ").as_deref(), Some("http://x.org"));
        assert_eq!(
            web_url("HTTPS://Example.com").as_deref(),
            Some("HTTPS://Example.com")
        );
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
    fn a_code_span_links_only_when_it_is_one_web_address() {
        assert_eq!(
            code_url("http://127.0.0.1:8771/").as_deref(),
            Some("http://127.0.0.1:8771/")
        );
        assert_eq!(
            code_url("https://a.com/x?y=1").as_deref(),
            Some("https://a.com/x?y=1")
        );
        assert_eq!(code_url("curl http://a.com"), None);
        assert_eq!(code_url("http://a.com e http://b.com"), None);
        assert_eq!(code_url(" http://a.com"), None);
        assert_eq!(code_url("src/main.rs"), None);
        assert_eq!(code_url("ftp://a.com"), None);
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
