//! Inline badges in the composer and in sent messages (ADR-198): a link becomes the site's
//! favicon and domain, an `@file` the file's icon and name, and a `/command` a badge in the accent
//! color. The text is never changed: what is sent, copied and stored is the plain string. Only the
//! drawing differs.
//!
//! GPUI's `StyledText` has no inline boxes, so each badge is drawn in two parts. The text is laid
//! out with a run of non-breaking spaces where the badge goes (`Shown`): they are word characters
//! for the line wrapper, so the badge wraps as one unit with its neighbours, and the run is as wide
//! as the badge needs. The badge itself (a real element: icon, label, tooltip) is then painted over
//! that gap, at the place the layout reports (`overlay`). `Shown` maps byte indexes between the
//! plain text and the laid out one, so the caret, the selection and a click work in plain text and
//! never land inside a badge.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use gpui::{
    AnyElement, App, Asset, AvailableSpace, Hsla, ImageSource, RenderImage, Rgba, StyledText,
    TextLayout, prelude::*, px,
};

const NBSP: char = '\u{a0}';
/// Height, side padding and icon of a badge, in px.
const HEIGHT: f32 = 22.;
const PAD: f32 = 7.;
const ICON: f32 = 14.;
const GAP: f32 = 5.;
const FONT: f32 = 13.;
/// Longest label before it is cut with an ellipsis; the tooltip has the whole address.
const LONGEST: usize = 32;

// ---- Finding the tokens ----------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Url { domain: String, full: String },
    File { name: String, path: String },
    Command { name: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    pub range: Range<usize>,
    pub kind: Kind,
}

impl Badge {
    /// What the badge says.
    pub fn label(&self) -> String {
        let label = match &self.kind {
            Kind::Url { domain, .. } => domain.clone(),
            Kind::File { name, .. } => name.clone(),
            Kind::Command { name } => format!("/{name}"),
        };
        cut(&label)
    }
}

fn cut(label: &str) -> String {
    if label.chars().count() <= LONGEST {
        return label.to_owned();
    }
    let head: String = label.chars().take(LONGEST - 1).collect();
    format!("{head}…")
}

/// The commands, skills and files the composer recognizes.
#[derive(Default)]
pub struct Known {
    commands: HashSet<String>,
    files: HashSet<String>,
    signature: (usize, String, usize),
}

/// Built in the composer itself, besides the skills the host lists.
const BUILT_IN: [&str; 3] = ["commit", "stage", "compact"];

impl Known {
    pub fn new(
        commands: impl IntoIterator<Item = String>,
        files: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut known = Self {
            commands: commands.into_iter().map(|c| c.to_lowercase()).collect(),
            files: files.into_iter().collect(),
            signature: Default::default(),
        };
        known.commands.extend(BUILT_IN.map(str::to_owned));
        known
    }
}

static KNOWN: RwLock<Option<Known>> = RwLock::new(None);

/// Keeps the recognized names in step with the skills and files the shell has loaded.
pub fn sync(skills: &[(String, String)], files: &(String, Vec<(String, String)>)) {
    let signature = (skills.len(), files.0.clone(), files.1.len());
    let current = KNOWN
        .read()
        .ok()
        .is_some_and(|k| k.as_ref().is_some_and(|k| k.signature == signature));
    if current {
        return;
    }
    let mut known = Known::new(
        skills.iter().map(|(name, _)| name.clone()),
        files.1.iter().map(|(name, folder)| {
            if folder.is_empty() {
                name.clone()
            } else {
                format!("{folder}/{name}")
            }
        }),
    );
    known.signature = signature;
    if let Ok(mut slot) = KNOWN.write() {
        *slot = Some(known);
    }
}

/// The badges in `text` as the composer (`sent` false) or a sent message (`sent` true) shows them.
pub fn scan(text: &str, sent: bool) -> Vec<Badge> {
    let slot = KNOWN.read();
    let fallback = Known::new([], []);
    let known = slot
        .as_ref()
        .ok()
        .and_then(|k| k.as_ref())
        .unwrap_or(&fallback);
    scan_with(text, known, sent)
}

/// The badges in `text`: links anywhere, `/command` and `@file` where a word starts. A command
/// must be one the composer knows; a file one in the project (a sent message, written when the
/// list may have been another, also takes a name that looks like a path).
pub fn scan_with(text: &str, known: &Known, sent: bool) -> Vec<Badge> {
    let mut badges = Vec::new();
    let mut start = None;
    let mut words = Vec::new();
    for (i, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                words.push(s..i);
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        words.push(s..text.len());
    }
    for word in words {
        let body = &text[word.clone()];
        if let Some(url) = url_in(body) {
            let full = &body[url.clone()];
            if let Some(domain) = domain_of(full) {
                badges.push(Badge {
                    range: word.start + url.start..word.start + url.end,
                    kind: Kind::Url {
                        domain,
                        full: full.to_owned(),
                    },
                });
            }
            continue;
        }
        let trimmed = body.trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
        let range = word.start..word.start + trimmed.len();
        if let Some(name) = trimmed.strip_prefix('/') {
            let simple = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'));
            if simple && known.commands.contains(&name.to_lowercase()) {
                badges.push(Badge {
                    range,
                    kind: Kind::Command {
                        name: name.to_owned(),
                    },
                });
            }
        } else if let Some(path) = trimmed.strip_prefix('@') {
            let pathlike = path.contains('.') || path.contains('/');
            let real = known.files.contains(path) || (sent && pathlike);
            if !path.is_empty() && !path.ends_with('/') && real {
                badges.push(Badge {
                    range,
                    kind: Kind::File {
                        name: path.rsplit('/').next().unwrap_or(path).to_owned(),
                        path: path.to_owned(),
                    },
                });
            }
        }
    }
    badges
}

/// Where a link sits in a word: after any opening bracket or quote, without the punctuation that
/// ends a sentence. A `)` stays when the link opened its own `(`.
fn url_in(word: &str) -> Option<Range<usize>> {
    let lead = word.len()
        - word
            .trim_start_matches(['(', '[', '<', '"', '\'', '*'])
            .len();
    let rest = &word[lead..];
    let scheme = ["https://", "http://"].into_iter().find(|s| {
        rest.get(..s.len())
            .is_some_and(|h| h.eq_ignore_ascii_case(s))
    })?;
    let mut end = rest.len();
    while let Some(last) = rest[..end].chars().next_back() {
        let drop = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '>' | ']' | '}' | '*' => true,
            ')' => {
                let link = &rest[..end];
                link.matches('(').count() < link.matches(')').count()
            }
            _ => false,
        };
        if !drop {
            break;
        }
        end -= last.len_utf8();
    }
    (end > scheme.len()).then_some(lead..lead + end)
}

/// A link's site as the badge names it: the host without `www.` (with its port, if any).
pub fn domain_of(url: &str) -> Option<String> {
    let after = url.split_once("://")?.1;
    let authority = after.split(['/', '?', '#']).next()?;
    let hostport = authority.rsplit('@').next()?;
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, p),
        _ => (hostport, ""),
    };
    let host = host.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let labels: Vec<&str> = host.split('.').collect();
    let valid = !host.is_empty()
        && labels.iter().all(|l| {
            !l.is_empty()
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
        && (host == "localhost"
            || (labels.len() > 1 && labels.last().is_some_and(|t| t.len() >= 2)));
    valid.then(|| {
        if port.is_empty() {
            host.to_owned()
        } else {
            format!("{host}:{port}")
        }
    })
}

/// Whether a favicon can be asked for: a public name, not an address or a local server.
fn fetchable(domain: &str) -> bool {
    !domain.contains(':')
        && domain != "localhost"
        && !domain.ends_with(".local")
        && !domain.chars().all(|c| c.is_ascii_digit() || c == '.')
}

// ---- Where the caret may go ------------------------------------------------------------------

/// Where one step back from `at` ends: `plain` (a character back), or the start of the badge it
/// is at the end of or inside.
pub fn back_in(badges: &[Badge], at: usize, plain: usize) -> usize {
    badges
        .iter()
        .find(|b| b.range.start < at && at <= b.range.end)
        .map_or(plain, |b| b.range.start.min(plain))
}

/// Where one step forward from `at` ends: `plain`, or the end of the badge it is at the start of.
pub fn forward_in(badges: &[Badge], at: usize, plain: usize) -> usize {
    badges
        .iter()
        .find(|b| b.range.start <= at && at < b.range.end)
        .map_or(plain, |b| b.range.end.max(plain))
}

/// What Backspace at `at` deletes: the whole badge before the caret, else the character.
#[cfg(test)]
pub fn backspace_range(text: &str, badges: &[Badge], at: usize) -> Range<usize> {
    let plain = text[..at].char_indices().next_back().map_or(0, |(i, _)| i);
    back_in(badges, at, plain)..at
}

pub fn step_back(text: &str, at: usize, plain: usize) -> usize {
    back_in(&scan(text, false), at, plain)
}

pub fn step_forward(text: &str, at: usize, plain: usize) -> usize {
    forward_in(&scan(text, false), at, plain)
}

// ---- The laid out text -----------------------------------------------------------------------

/// One badge and where its gap is in the laid out text.
pub struct Span {
    pub logical: Range<usize>,
    pub shown: Range<usize>,
    pub badge: Badge,
}

/// A text as it is laid out, with a gap of non-breaking spaces for each badge.
pub struct Shown {
    pub text: String,
    pub plain: String,
    pub spans: Vec<Span>,
}

#[derive(Clone, Copy, PartialEq)]
enum Round {
    Nearest,
    Down,
    Up,
}

impl Shown {
    /// `nbsp` is the width of a non-breaking space and `label` the width of a word at the badge's
    /// size, both in px.
    pub fn build(plain: &str, badges: Vec<Badge>, nbsp: f32, label: &dyn Fn(&str) -> f32) -> Self {
        let mut text = String::with_capacity(plain.len());
        let mut spans = Vec::new();
        let mut at = 0;
        for badge in badges {
            if badge.range.start < at {
                continue;
            }
            text.push_str(&plain[at..badge.range.start]);
            let lead = if matches!(badge.kind, Kind::Command { .. }) {
                0.
            } else {
                ICON + GAP
            };
            let width = 2. + 2. * PAD + lead + label(&badge.label()) + 1.;
            let count = (width / nbsp.max(1.)).ceil().max(1.) as usize;
            let begin = text.len();
            text.extend(std::iter::repeat_n(NBSP, count));
            spans.push(Span {
                logical: badge.range.clone(),
                shown: begin..text.len(),
                badge: badge.clone(),
            });
            at = badge.range.end;
        }
        text.push_str(&plain[at..]);
        Self {
            text,
            plain: plain.to_owned(),
            spans,
        }
    }

    pub fn is_plain(&self) -> bool {
        self.spans.is_empty()
    }

    fn to_shown_by(&self, at: usize, round: Round) -> usize {
        let mut delta = 0isize;
        for span in &self.spans {
            if at <= span.logical.start {
                break;
            }
            if at < span.logical.end {
                let start = match round {
                    Round::Down => true,
                    Round::Up => false,
                    Round::Nearest => at - span.logical.start <= span.logical.end - at,
                };
                return if start {
                    span.shown.start
                } else {
                    span.shown.end
                };
            }
            delta = span.shown.end as isize - span.logical.end as isize;
        }
        (at as isize + delta) as usize
    }

    fn to_plain_by(&self, at: usize, round: Round) -> usize {
        let mut delta = 0isize;
        for span in &self.spans {
            if at <= span.shown.start {
                break;
            }
            if at < span.shown.end {
                let start = match round {
                    Round::Down => true,
                    Round::Up => false,
                    Round::Nearest => at - span.shown.start <= span.shown.end - at,
                };
                return if start {
                    span.logical.start
                } else {
                    span.logical.end
                };
            }
            delta = span.logical.end as isize - span.shown.end as isize;
        }
        (at as isize + delta).max(0) as usize
    }

    /// A byte of the plain text, as a byte of the laid out one; inside a badge, its nearer edge.
    pub fn to_shown(&self, at: usize) -> usize {
        self.to_shown_by(at, Round::Nearest)
    }

    /// A selection in the plain text, as the laid out one (a badge it touches is taken whole).
    pub fn range_to_shown(&self, range: Range<usize>) -> Range<usize> {
        self.to_shown_by(range.start, Round::Down)..self.to_shown_by(range.end, Round::Up)
    }

    /// A byte of the laid out text, as a byte of the plain one; never inside a badge.
    pub fn to_plain(&self, at: usize) -> usize {
        self.to_plain_by(at, Round::Nearest).min(self.plain.len())
    }

    /// The plain text a stretch of the laid out text stands for, badges whole.
    pub fn plain_of(&self, range: Range<usize>) -> &str {
        let start = self
            .to_plain_by(range.start, Round::Down)
            .min(self.plain.len());
        let end = self.to_plain_by(range.end, Round::Up).min(self.plain.len());
        &self.plain[start..end.max(start)]
    }
}

/// Builds the laid out text for `plain`, at `size` px for the text around the badges.
pub fn shown_for(plain: &str, sent: bool, size: f32, cx: &App) -> Rc<Shown> {
    let badges = scan(plain, sent);
    if badges.is_empty() {
        return Rc::new(Shown::build(plain, badges, 1., &|_| 0.));
    }
    let system = cx.text_system();
    let font = system.resolve_font(&gpui::font(".SystemUIFont"));
    let width = |text: &str, size: f32| -> f32 {
        text.chars()
            .filter_map(|c| system.advance(font, px(size), c).ok())
            .map(|a| f32::from(a.width))
            .sum()
    };
    let nbsp = width("\u{a0}", size);
    let nbsp = if nbsp > 0.5 { nbsp } else { size * 0.25 };
    Rc::new(Shown::build(plain, badges, nbsp, &|label| {
        width(label, FONT)
    }))
}

thread_local! {
    /// The laid out texts drawn lately, by the laid out text and by the plain one, for the transcript's
    /// selection: it addresses the drawn text and copies the plain one.
    static DRAWN: RefCell<(HashMap<String, Rc<Shown>>, HashMap<String, Rc<Shown>>)> =
        RefCell::new(Default::default());
}

/// Notes a text just drawn with badges, so a selection over it can be copied as plain text.
pub fn remember(shown: &Rc<Shown>) {
    if shown.is_plain() {
        return;
    }
    DRAWN.with_borrow_mut(|(by_shown, by_plain)| {
        if by_shown.len() > 64 {
            by_shown.clear();
            by_plain.clear();
        }
        by_shown.insert(shown.text.clone(), shown.clone());
        by_plain.insert(shown.plain.clone(), shown.clone());
    });
}

/// The text a message is drawn as, if it has badges; else the message itself.
pub fn displayed(plain: &str) -> String {
    DRAWN
        .with_borrow(|(_, by_plain)| by_plain.get(plain).map(|s| s.text.clone()))
        .unwrap_or_else(|| plain.to_owned())
}

/// The plain text of a stretch of a drawn text: `range` of `text` when it has no badges.
pub fn plain_slice(text: &str, range: Range<usize>) -> String {
    let drawn = DRAWN.with_borrow(|(by_shown, _)| by_shown.get(text).cloned());
    match drawn {
        Some(shown) => shown.plain_of(range).to_owned(),
        None => text[range].to_owned(),
    }
}

/// `shown` as styled text: the `/` and `@` words that are not badges in the accent color, and the
/// find bar's matches filled.
pub fn styled(shown: &Shown) -> StyledText {
    let plain = &shown.plain;
    let tokens: Vec<Range<usize>> = crate::data::token_ranges(plain)
        .into_iter()
        .filter(|t| {
            !shown
                .spans
                .iter()
                .any(|s| s.logical.start <= t.start && t.end <= s.logical.end)
        })
        .map(|t| shown.range_to_shown(t))
        .collect();
    let found: Vec<(Range<usize>, Hsla)> = crate::markdown::search_ranges(plain)
        .into_iter()
        .map(|(range, fill)| (shown.range_to_shown(range), fill))
        .collect();
    let mut cuts = vec![0, shown.text.len()];
    for range in tokens.iter().chain(found.iter().map(|(range, _)| range)) {
        cuts.push(range.start);
        cuts.push(range.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let highlights: Vec<_> = cuts
        .windows(2)
        .filter_map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let inside = |range: &Range<usize>| range.start <= a && b <= range.end;
            let token = tokens.iter().any(inside);
            let fill = found
                .iter()
                .find(|(range, _)| inside(range))
                .map(|(_, fill)| *fill);
            (token || fill.is_some()).then(|| {
                (
                    a..b,
                    gpui::HighlightStyle {
                        color: token.then(crate::theme::accent),
                        background_color: fill,
                        ..Default::default()
                    },
                )
            })
        })
        .collect();
    StyledText::new(shown.text.clone()).with_highlights(highlights)
}

// ---- Drawing the badges ----------------------------------------------------------------------

/// The ink, fill and edge of a badge.
fn tone(kind: &Kind) -> (Hsla, Hsla, Hsla) {
    let light = crate::theme::is_light();
    match kind {
        Kind::Url { .. } => (
            crate::theme::content(0.92),
            crate::theme::content(0.04),
            crate::theme::border(),
        ),
        Kind::File { .. } => (
            gpui::rgb(if light { 0x33448f } else { 0xcfd8ff }).into(),
            crate::theme::content(0.04),
            crate::theme::border(),
        ),
        Kind::Command { .. } => {
            let accent = crate::theme::accent().to_rgb();
            let (toward, by) = if light { (0., 0.35) } else { (1., 0.55) };
            let ink = Rgba {
                r: accent.r + (toward - accent.r) * by,
                g: accent.g + (toward - accent.g) * by,
                b: accent.b + (toward - accent.b) * by,
                a: 1.,
            };
            (
                ink.into(),
                crate::theme::accent_wash(0.10),
                crate::theme::accent_wash(0.27),
            )
        }
    }
}

fn badge_element(
    span: &Span,
    key: u64,
    ix: usize,
    width: f32,
    height: f32,
    favicon: Option<Arc<RenderImage>>,
) -> AnyElement {
    let (ink, fill, edge) = tone(&span.badge.kind);
    let lead: Option<AnyElement> = match &span.badge.kind {
        Kind::Url { .. } => Some(match favicon {
            Some(image) => gpui::img(ImageSource::Render(image))
                .size(px(ICON))
                .rounded(px(3.))
                .flex_shrink_0()
                .into_any_element(),
            None => crate::theme::icon("globe", ICON)
                .text_color(crate::theme::content(0.5))
                .into_any_element(),
        }),
        Kind::File { name, .. } => Some(crate::file_icons::file_icon(name, false, ICON)),
        Kind::Command { .. } => None,
    };
    let tip = match &span.badge.kind {
        Kind::Url { full, .. } => Some(full.clone()),
        Kind::File { path, .. } => Some(path.clone()),
        Kind::Command { .. } => None,
    };
    let shell = gpui::div()
        .id(gpui::ElementId::named_usize(
            format!("inline-badge-{key:x}"),
            ix,
        ))
        .w(px(width))
        .h(px(height))
        .px(px(PAD))
        .flex()
        .items_center()
        .gap(px(GAP))
        .rounded(px(crate::theme::RADIUS_SM))
        .border_1()
        .border_color(edge)
        .bg(fill)
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(px(FONT))
        .line_height(px(height - 4.))
        .text_color(ink)
        .children(lead)
        .child(gpui::div().flex_shrink_0().child(span.badge.label()));
    match tip {
        Some(tip) => shell
            .tooltip(crate::shell::message_actions::text_tip(tip))
            .into_any_element(),
        None => shell.into_any_element(),
    }
}

/// The badges painted over the gaps `shown` leaves in the text `layout` laid out. Put it over
/// the text, in a box of the text's size; `key` tells one text's badges from another's.
pub fn overlay(layout: TextLayout, shown: Rc<Shown>, key: u64) -> gpui::Canvas<Vec<AnyElement>> {
    gpui::canvas(
        move |_, window, cx| {
            let line = f32::from(layout.line_height());
            let height = HEIGHT.min(line);
            let mut made = Vec::new();
            for (ix, span) in shown.spans.iter().enumerate() {
                let Some(start) = layout.position_for_index(span.shown.start) else {
                    continue;
                };
                // The gap's right edge, unless the line broke inside it.
                let width = match layout.position_for_index(span.shown.end) {
                    Some(end) if end.y == start.y && end.x > start.x => f32::from(end.x - start.x),
                    _ => {
                        let count = span.shown.len() / NBSP.len_utf8();
                        count as f32 * f32::from(layout.line_height()) * 0.25
                    }
                };
                let favicon = match &span.badge.kind {
                    Kind::Url { domain, .. } if fetchable(domain) => {
                        window.use_asset::<Favicon>(&domain.clone(), cx).flatten()
                    }
                    _ => None,
                };
                let mut element = badge_element(span, key, ix, width, height, favicon);
                let origin = gpui::point(start.x, start.y + px((line - height) / 2.));
                element.prepaint_as_root(
                    origin,
                    gpui::size(
                        AvailableSpace::Definite(px(width)),
                        AvailableSpace::Definite(px(height)),
                    ),
                    window,
                    cx,
                );
                made.push(element);
            }
            made
        },
        |_, mut elements, window, cx| {
            for element in &mut elements {
                element.paint(window, cx);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

// ---- Favicons --------------------------------------------------------------------------------

/// A site's favicon, loaded off the main thread and kept in memory (GPUI's asset cache) and on
/// disk under the app's cache folder. None until it arrives, and if the site has none.
struct Favicon;

impl Asset for Favicon {
    type Source = String;
    type Output = Option<Arc<RenderImage>>;

    fn load(
        domain: String,
        _: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        async move { load_favicon(&domain) }
    }
}

fn cache_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Caches/com.siruscode.native/favicons")
}

fn load_favicon(domain: &str) -> Option<Arc<RenderImage>> {
    let dir = cache_dir();
    let safe: String = domain
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let png = dir.join(format!("{safe}.png"));
    if let Some(image) = std::fs::read(&png).ok().and_then(|b| decode(&b)) {
        return Some(image);
    }
    // A site without one is not asked again for a day.
    let miss = dir.join(format!("{safe}.none"));
    let fresh = std::fs::metadata(&miss)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() < 24 * 3600);
    if fresh {
        return None;
    }
    let sources = [
        format!("https://www.google.com/s2/favicons?domain={domain}&sz=32"),
        format!("https://{domain}/favicon.ico"),
    ];
    for url in sources {
        let Some(bytes) = fetch(&url) else { continue };
        let Some(small) = shrink(&bytes) else {
            continue;
        };
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&png, &small);
        return decode(&small);
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&miss, b"");
    None
}

fn fetch(url: &str) -> Option<Vec<u8>> {
    let output = std::process::Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--location",
            "--proto",
            "=https",
            "--max-time",
            "8",
            "--max-filesize",
            "524288",
            url,
        ])
        .output()
        .ok()?;
    (output.status.success() && !output.stdout.is_empty()).then_some(output.stdout)
}

/// A favicon in whatever format as a 28 px PNG (twice the size it is drawn at).
fn shrink(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    let small = image.resize_to_fill(28, 28, image::imageops::FilterType::Triangle);
    let mut png = Vec::new();
    small
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

fn decode(png: &[u8]) -> Option<Arc<RenderImage>> {
    let mut pixels = image::load_from_memory(png).ok()?.into_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Known {
        Known::new(
            ["frontend-design".to_owned()],
            ["src/main.rs".to_owned(), "README.md".to_owned()],
        )
    }

    fn urls(text: &str) -> Vec<String> {
        scan_with(text, &known(), false)
            .into_iter()
            .filter(|b| matches!(b.kind, Kind::Url { .. }))
            .map(|b| text[b.range].to_owned())
            .collect()
    }

    #[test]
    fn a_link_ends_before_the_punctuation_that_closes_a_sentence() {
        assert_eq!(urls("veja https://a.com/x."), ["https://a.com/x"]);
        assert_eq!(urls("veja https://a.com/x, e"), ["https://a.com/x"]);
        assert_eq!(urls("ok https://a.com!?"), ["https://a.com"]);
        assert_eq!(urls("(https://a.com/x)"), ["https://a.com/x"]);
        assert_eq!(urls("<https://a.com/x>"), ["https://a.com/x"]);
    }

    #[test]
    fn a_link_keeps_its_own_parentheses_and_query() {
        assert_eq!(
            urls("https://en.wikipedia.org/wiki/Rust_(language)"),
            ["https://en.wikipedia.org/wiki/Rust_(language)"]
        );
        assert_eq!(
            urls("(https://en.wikipedia.org/wiki/Rust_(language))."),
            ["https://en.wikipedia.org/wiki/Rust_(language)"]
        );
        assert_eq!(
            urls("https://a.com/s?q=rust&page=2#top"),
            ["https://a.com/s?q=rust&page=2#top"]
        );
    }

    #[test]
    fn something_that_is_not_a_link_is_left_alone() {
        assert!(urls("https://").is_empty());
        assert!(urls("https://exemplo").is_empty());
        assert!(urls("ftp://a.com e www.a.com").is_empty());
        assert!(urls("https://a.c").is_empty());
    }

    #[test]
    fn the_label_is_the_domain_without_www() {
        assert_eq!(
            domain_of("https://www.infisical.com/docs?x=1").as_deref(),
            Some("infisical.com")
        );
        assert_eq!(domain_of("http://Docs.Rs/gpui").as_deref(), Some("docs.rs"));
        assert_eq!(
            domain_of("https://user:pw@a.com:8443/p").as_deref(),
            Some("a.com:8443")
        );
        assert_eq!(
            domain_of("http://localhost:3000/x").as_deref(),
            Some("localhost:3000")
        );
        assert_eq!(domain_of("https://a.com#frag").as_deref(), Some("a.com"));
        assert_eq!(domain_of("https://-a.com"), None);
        let badge = &scan_with("https://www.infisical.com/docs", &known(), false)[0];
        assert_eq!(badge.label(), "infisical.com");
    }

    #[test]
    fn a_mention_starts_a_word_so_an_email_is_not_one() {
        let text = "mande para eu@exemplo.com e veja @src/main.rs, depois @README.md.";
        let found = scan_with(text, &known(), false);
        let names: Vec<_> = found.iter().map(|b| &text[b.range.clone()]).collect();
        assert_eq!(names, ["@src/main.rs", "@README.md"]);
        assert_eq!(found[0].label(), "main.rs");
    }

    #[test]
    fn a_mention_of_an_unknown_file_is_a_badge_only_in_a_sent_message() {
        let text = "veja @outro/arquivo.rs e @alguem";
        assert!(scan_with(text, &known(), false).is_empty());
        let sent = scan_with(text, &known(), true);
        assert_eq!(sent.len(), 1);
        assert_eq!(&text[sent[0].range.clone()], "@outro/arquivo.rs");
    }

    #[test]
    fn a_slash_inside_a_path_is_not_a_command() {
        let text = "/commit agora, abra src/main.rs e /usr/bin/env e /frontend-design.";
        let found = scan_with(text, &known(), false);
        let names: Vec<_> = found.iter().map(|b| &text[b.range.clone()]).collect();
        assert_eq!(names, ["/commit", "/frontend-design"]);
        assert!(scan_with("/desconhecido", &known(), false).is_empty());
        assert_eq!(scan_with("/COMMIT", &known(), false).len(), 1);
    }

    #[test]
    fn backspace_after_a_badge_takes_the_whole_token() {
        let text = "olhe /commit e https://a.com/x agora";
        let badges = scan_with(text, &known(), false);
        let commit = &badges[0].range;
        assert_eq!(
            backspace_range(text, &badges, commit.end),
            commit.start..commit.end
        );
        // After the space that follows it, only the space goes.
        assert_eq!(
            backspace_range(text, &badges, commit.end + 1),
            commit.end..commit.end + 1
        );
        let link = &badges[1].range;
        assert_eq!(
            backspace_range(text, &badges, link.end),
            link.start..link.end
        );
        // Plain text keeps its one character.
        assert_eq!(backspace_range(text, &badges, 3), 2..3);
    }

    #[test]
    fn the_arrows_step_over_a_badge() {
        let text = "a /commit b";
        let badges = scan_with(text, &known(), false);
        let b = &badges[0].range;
        assert_eq!(back_in(&badges, b.end, b.end - 1), b.start);
        assert_eq!(forward_in(&badges, b.start, b.start + 1), b.end);
        assert_eq!(forward_in(&badges, 0, 1), 1);
        assert_eq!(back_in(&badges, b.end + 1, b.end), b.end);
    }

    #[test]
    fn indexes_map_between_the_text_and_the_laid_out_one() {
        let text = "ab /commit cd";
        let badges = scan_with(text, &known(), false);
        let shown = Shown::build(text, badges, 4., &|_| 40.);
        assert_eq!(shown.spans.len(), 1);
        let gap = shown.spans[0].shown.clone();
        assert!(gap.len() >= 2 * (60. / 4.) as usize);
        assert!(shown.text.starts_with("ab "));
        assert!(shown.text.ends_with(" cd"));
        // Outside the badge the two agree up to the gap's width.
        assert_eq!(shown.to_shown(3), 3);
        assert_eq!(shown.to_shown(10), gap.end);
        assert_eq!(shown.to_shown(11), gap.end + 1);
        assert_eq!(shown.to_plain(gap.end + 1), 11);
        // Inside it, the nearer edge.
        assert_eq!(shown.to_plain(gap.start + 1), 3);
        assert_eq!(shown.to_plain(gap.end - 1), 10);
        assert_eq!(shown.range_to_shown(5..6), gap);
        // Copying part of the gap copies the whole token.
        assert_eq!(shown.plain_of(gap.start + 2..gap.start + 4), "/commit");
        assert_eq!(shown.plain_of(0..gap.end + 2), "ab /commit c");
    }
}
