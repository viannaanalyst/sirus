//! Word and PowerPoint files, and web pages, as the HTML a web view shows in the attachment
//! viewer. The conversion is done here in Rust from the OOXML parts (`attachment_ooxml`): the
//! result keeps the document's own look (fonts, sizes, colors, alignment, lists, tables, pictures,
//! page width, slide geometry) and lets the web engine give text selection, copy and layout. Every
//! piece of document text is escaped and the page carries a Content-Security-Policy, so a file
//! cannot run code or reach the network. Pure and tested; no UI types.
use crate::attachment_ooxml::{
    self as ox, Rel, Zip, attr, child, children, escape, find, flag, val,
};
use roxmltree::Node;
use std::collections::HashMap;
use std::fmt::Write as _;

/// A converted Word document.
pub struct WordPage {
    pub html: String,
    /// The headings in order, for the outline; `id` is the element id to scroll to.
    pub headings: Vec<Heading>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub id: String,
}

/// A converted presentation.
pub struct Deck {
    pub html: String,
    pub slides: usize,
}

const DARK_LIGHT: &str =
    "body{background:#e8e8ec}@media (prefers-color-scheme:dark){body{background:#26262b}}";

/// Locks a page down: no scripts (unless `nonce`), no network, images only from the document.
fn policy(nonce: Option<&str>) -> String {
    let script = match nonce {
        Some(n) => format!("script-src 'nonce-{n}'"),
        None => "script-src 'none'".to_owned(),
    };
    format!(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:; {script}\">"
    )
}

// ==== Web pages ============================================================================

/// An HTML attachment as a page that cannot run scripts or load anything from the network. The
/// policy comes first, so the page's own markup cannot loosen it.
pub fn html_attachment(source: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{}</head><body>{}</body></html>",
        policy(None),
        source_without_scripts(source)
    )
}

/// Drops `<script>` elements, so they cannot show up as text; the policy already blocks them.
fn source_without_scripts(source: &str) -> String {
    let lower = source.to_ascii_lowercase();
    let mut out = String::with_capacity(source.len());
    let mut at = 0;
    while let Some(open) = lower[at..].find("<script") {
        let start = at + open;
        out.push_str(&source[at..start]);
        match lower[start..].find("</script>") {
            Some(end) => at = start + end + "</script>".len(),
            None => {
                at = source.len();
                break;
            }
        }
    }
    out.push_str(&source[at..]);
    out
}

// ==== Word =================================================================================

#[derive(Clone, Default)]
struct RunProps {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    caps: Option<bool>,
    size: Option<f32>,
    color: Option<String>,
    font: Option<String>,
    highlight: Option<String>,
    vert: Option<String>,
}

impl RunProps {
    fn read(node: Node) -> RunProps {
        RunProps {
            bold: flag(node, "b"),
            italic: flag(node, "i"),
            underline: child(node, "u").map(|u| attr(u, "val") != Some("none")),
            strike: flag(node, "strike"),
            caps: flag(node, "caps"),
            size: val(node, "sz")
                .and_then(|v| v.parse::<f32>().ok())
                .map(|v| v / 2.),
            color: val(node, "color")
                .filter(|c| *c != "auto" && is_hex(c))
                .map(|c| format!("#{c}")),
            font: child(node, "rFonts")
                .and_then(|f| attr(f, "ascii").or(attr(f, "hAnsi")))
                .map(str::to_owned),
            highlight: val(node, "highlight")
                .and_then(highlight_color)
                .map(str::to_owned),
            vert: val(node, "vertAlign").map(str::to_owned),
        }
    }

    /// Lets `other` win where it says something.
    fn over(&mut self, other: &RunProps) {
        macro_rules! take {
            ($($f:ident),*) => {$( if other.$f.is_some() { self.$f = other.$f.clone(); } )*};
        }
        take!(
            bold, italic, underline, strike, caps, size, color, font, highlight, vert
        );
    }

    fn css(&self) -> String {
        let mut s = String::new();
        if self.bold == Some(true) {
            s.push_str("font-weight:700;");
        }
        if self.italic == Some(true) {
            s.push_str("font-style:italic;");
        }
        let mut lines = Vec::new();
        if self.underline == Some(true) {
            lines.push("underline");
        }
        if self.strike == Some(true) {
            lines.push("line-through");
        }
        if !lines.is_empty() {
            let _ = write!(s, "text-decoration:{};", lines.join(" "));
        }
        if self.caps == Some(true) {
            s.push_str("text-transform:uppercase;");
        }
        if let Some(size) = self.size {
            let _ = write!(s, "font-size:{:.2}pt;", size);
        }
        if let Some(color) = &self.color {
            let _ = write!(s, "color:{color};");
        }
        if let Some(font) = self.font.as_deref().and_then(safe_font) {
            let _ = write!(s, "font-family:{font};");
        }
        if let Some(h) = &self.highlight {
            let _ = write!(s, "background:{h};");
        }
        match self.vert.as_deref() {
            Some("superscript") => s.push_str("vertical-align:super;font-size:smaller;"),
            Some("subscript") => s.push_str("vertical-align:sub;font-size:smaller;"),
            _ => {}
        }
        s
    }
}

fn is_hex(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn highlight_color(name: &str) -> Option<&'static str> {
    Some(match name {
        "yellow" => "#ffff00",
        "green" => "#00ff00",
        "cyan" => "#00ffff",
        "magenta" => "#ff00ff",
        "blue" => "#0000ff",
        "red" => "#ff0000",
        "darkBlue" => "#000080",
        "darkCyan" => "#008080",
        "darkGreen" => "#008000",
        "darkMagenta" => "#800080",
        "darkRed" => "#800000",
        "darkYellow" => "#808000",
        "darkGray" => "#808080",
        "lightGray" => "#c0c0c0",
        "black" => "#000000",
        _ => return None,
    })
}

/// A font name as a CSS value, only when it is plain letters, digits, spaces and dashes.
fn safe_font(name: &str) -> Option<String> {
    (!name.is_empty()
        && name.len() < 60
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')))
    .then(|| format!("'{name}',sans-serif"))
}

#[derive(Clone, Default)]
struct ParaProps {
    align: Option<String>,
    before: Option<f32>,
    after: Option<f32>,
    line: Option<f32>,
    left: Option<f32>,
    first: Option<f32>,
    outline: Option<u8>,
    fill: Option<String>,
}

impl ParaProps {
    fn read(node: Node) -> ParaProps {
        let spacing = child(node, "spacing");
        let ind = child(node, "ind");
        let twips = |n: Option<Node>, key: &str| {
            n.and_then(|n| attr(n, key))
                .and_then(|v| v.parse::<f32>().ok())
                .map(|v| v / 15.)
        };
        let hanging = twips(ind, "hanging");
        ParaProps {
            align: val(node, "jc").map(str::to_owned),
            before: twips(spacing, "before"),
            after: twips(spacing, "after"),
            line: spacing
                .filter(|s| matches!(attr(*s, "lineRule"), None | Some("auto")))
                .and_then(|s| attr(s, "line"))
                .and_then(|v| v.parse::<f32>().ok())
                .map(|v| v / 240.),
            left: twips(ind, "left").or(twips(ind, "start")),
            first: hanging.map(|h| -h).or(twips(ind, "firstLine")),
            outline: val(node, "outlineLvl").and_then(|v| v.parse().ok()),
            fill: child(node, "shd")
                .and_then(|s| attr(s, "fill"))
                .filter(|f| is_hex(f))
                .map(|f| format!("#{f}")),
        }
    }

    fn over(&mut self, other: &ParaProps) {
        macro_rules! take {
            ($($f:ident),*) => {$( if other.$f.is_some() { self.$f = other.$f.clone(); } )*};
        }
        take!(align, before, after, line, left, first, outline, fill);
    }

    fn css(&self) -> String {
        let mut s = String::new();
        match self.align.as_deref() {
            Some("center") => s.push_str("text-align:center;"),
            Some("right" | "end") => s.push_str("text-align:right;"),
            Some("both" | "distribute") => s.push_str("text-align:justify;"),
            _ => {}
        }
        let _ = write!(
            s,
            "margin:{:.1}px 0 {:.1}px;",
            self.before.unwrap_or(0.),
            self.after.unwrap_or(0.)
        );
        if let Some(line) = self.line {
            let _ = write!(s, "line-height:{:.2};", line.max(0.8));
        }
        if let Some(left) = self.left {
            let _ = write!(s, "margin-left:{left:.1}px;");
        }
        if let Some(first) = self.first {
            let _ = write!(s, "text-indent:{first:.1}px;");
        }
        if let Some(fill) = &self.fill {
            let _ = write!(s, "background:{fill};");
        }
        s
    }
}

#[derive(Clone, Default)]
struct Style {
    name: String,
    based: Option<String>,
    para: ParaProps,
    run: RunProps,
}

#[derive(Default)]
struct Styles {
    by_id: HashMap<String, Style>,
    default_para: String,
    base_run: RunProps,
    base_para: ParaProps,
}

impl Styles {
    fn read(text: &str) -> Styles {
        let mut out = Styles::default();
        let Ok(doc) = ox::parse(text) else {
            return out;
        };
        let root = doc.root_element();
        if let Some(defaults) = child(root, "docDefaults") {
            if let Some(r) = child(defaults, "rPrDefault").and_then(|n| child(n, "rPr")) {
                out.base_run = RunProps::read(r);
            }
            if let Some(p) = child(defaults, "pPrDefault").and_then(|n| child(n, "pPr")) {
                out.base_para = ParaProps::read(p);
            }
        }
        for style in children(root, "style") {
            let Some(id) = attr(style, "styleId") else {
                continue;
            };
            if attr(style, "type") == Some("paragraph") && attr(style, "default") == Some("1") {
                out.default_para = id.to_owned();
            }
            out.by_id.insert(
                id.to_owned(),
                Style {
                    name: val(style, "name").unwrap_or("").to_owned(),
                    based: val(style, "basedOn").map(str::to_owned),
                    para: child(style, "pPr").map(ParaProps::read).unwrap_or_default(),
                    run: child(style, "rPr").map(RunProps::read).unwrap_or_default(),
                },
            );
        }
        out
    }

    /// The style and its ancestors, oldest first.
    fn chain(&self, id: &str) -> Vec<&Style> {
        let mut out = Vec::new();
        let mut next = Some(id);
        while let Some(current) = next {
            let Some(style) = self.by_id.get(current) else {
                break;
            };
            out.push(style);
            next = style.based.as_deref();
            if out.len() > 12 {
                break;
            }
        }
        out.reverse();
        out
    }

    fn para(&self, id: &str) -> (ParaProps, RunProps, Option<u8>) {
        let mut para = self.base_para.clone();
        let mut run = self.base_run.clone();
        let wanted = if id.is_empty() {
            &self.default_para
        } else {
            id
        };
        let mut level = None;
        for style in self.chain(wanted) {
            para.over(&style.para);
            run.over(&style.run);
            let name = style.name.to_ascii_lowercase();
            if let Some(n) = name
                .strip_prefix("heading ")
                .and_then(|n| n.trim().parse::<u8>().ok())
            {
                level = Some(n.clamp(1, 6));
            } else if name == "title" {
                level = Some(1);
            }
        }
        if let Some(outline) = para.outline {
            level = Some((outline + 1).clamp(1, 6));
        }
        (para, run, level)
    }

    fn run(&self, id: &str) -> RunProps {
        let mut run = RunProps::default();
        for style in self.chain(id) {
            run.over(&style.run);
        }
        run
    }
}

struct Level {
    format: String,
    text: String,
    left: Option<f32>,
    hanging: Option<f32>,
}

#[derive(Default)]
struct Numbering {
    /// numId -> (abstract id)
    nums: HashMap<String, String>,
    levels: HashMap<(String, u32), Level>,
}

impl Numbering {
    fn read(text: &str) -> Numbering {
        let mut out = Numbering::default();
        let Ok(doc) = ox::parse(text) else {
            return out;
        };
        let root = doc.root_element();
        for abs in children(root, "abstractNum") {
            let Some(id) = attr(abs, "abstractNumId") else {
                continue;
            };
            for lvl in children(abs, "lvl") {
                let ilvl = attr(lvl, "ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
                let ind = child(lvl, "pPr").and_then(|p| child(p, "ind"));
                let twips = |k: &str| {
                    ind.and_then(|i| attr(i, k))
                        .and_then(|v| v.parse::<f32>().ok())
                        .map(|v| v / 15.)
                };
                out.levels.insert(
                    (id.to_owned(), ilvl),
                    Level {
                        format: val(lvl, "numFmt").unwrap_or("decimal").to_owned(),
                        text: val(lvl, "lvlText").unwrap_or("").to_owned(),
                        left: twips("left").or(twips("start")),
                        hanging: twips("hanging"),
                    },
                );
            }
        }
        for num in children(root, "num") {
            if let (Some(id), Some(abs)) = (attr(num, "numId"), val(num, "abstractNumId")) {
                out.nums.insert(id.to_owned(), abs.to_owned());
            }
        }
        out
    }

    fn level(&self, num: &str, ilvl: u32) -> Option<&Level> {
        self.levels.get(&(self.nums.get(num)?.clone(), ilvl))
    }
}

fn roman(mut n: u32) -> String {
    let table = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, text) in table {
        while n >= value {
            out.push_str(text);
            n -= value;
        }
    }
    out
}

fn letters(n: u32, upper: bool) -> String {
    let mut n = n.max(1) - 1;
    let mut out = Vec::new();
    loop {
        out.push((b'a' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    let text: String = out.into_iter().rev().collect();
    if upper { text.to_uppercase() } else { text }
}

/// A list label: the level's text with `%1` .. `%9` replaced by the counters.
fn label(level: &Level, counters: &[u32]) -> String {
    if level.format == "bullet" {
        // Symbol fonts put bullets in the private use area, which a web view cannot show.
        let first = level.text.chars().next();
        return match first {
            Some(c) if c.is_alphanumeric() || ('\u{2022}'..='\u{25ff}').contains(&c) => {
                c.to_string()
            }
            _ => "\u{2022}".to_owned(),
        };
    }
    let mut out = String::new();
    let mut chars = level.text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%'
            && let Some(d) = chars.peek().and_then(|d| d.to_digit(10))
        {
            chars.next();
            let n = counters.get(d as usize - 1).copied().unwrap_or(1).max(1);
            out.push_str(&match level.format.as_str() {
                "lowerLetter" => letters(n, false),
                "upperLetter" => letters(n, true),
                "lowerRoman" => roman(n).to_lowercase(),
                "upperRoman" => roman(n),
                "none" => String::new(),
                _ => n.to_string(),
            });
        } else {
            out.push(c);
        }
    }
    out
}

struct Word<'a> {
    zip: &'a Zip<'a>,
    rels: HashMap<String, Rel>,
    styles: Styles,
    numbering: Numbering,
    counters: HashMap<String, Vec<u32>>,
    headings: Vec<Heading>,
    images: usize,
    out: String,
}

const EMU_PER_PX: f32 = 9525.;

impl Word<'_> {
    fn blocks(&mut self, parent: Node) {
        for node in parent.children().filter(Node::is_element) {
            match node.tag_name().name() {
                "p" => self.paragraph(node),
                "tbl" => self.table(node),
                "sdt" => {
                    if let Some(content) = child(node, "sdtContent") {
                        self.blocks(content);
                    }
                }
                "ins" | "customXml" | "smartTag" => self.blocks(node),
                _ => {}
            }
        }
    }

    fn paragraph(&mut self, node: Node) {
        let props = child(node, "pPr");
        let style_id = props.and_then(|p| val(p, "pStyle")).unwrap_or("");
        let (mut para, base_run, mut level) = self.styles.para(style_id);
        if let Some(p) = props {
            let direct = ParaProps::read(p);
            if let Some(o) = direct.outline {
                level = Some((o + 1).clamp(1, 6));
            }
            para.over(&direct);
        }
        // The paragraph mark's own run properties apply to its label only; runs bring theirs.
        let mut run_base = base_run;
        // A list item.
        let mut marker = String::new();
        if let Some(num) = props.and_then(|p| child(p, "numPr")) {
            let id = val(num, "numId").unwrap_or("0");
            let ilvl: u32 = val(num, "ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
            if id != "0"
                && let Some(l) = self.numbering.level(id, ilvl)
            {
                let counters = self.counters.entry(id.to_owned()).or_default();
                counters.resize(counters.len().max(ilvl as usize + 1), 0);
                counters[ilvl as usize] += 1;
                for deeper in counters.iter_mut().skip(ilvl as usize + 1) {
                    *deeper = 0;
                }
                let text = label(l, counters);
                let left = para.left.or(l.left).unwrap_or(36.);
                let hanging = l.hanging.unwrap_or(18.);
                para.left = Some(left);
                para.first = Some(-hanging);
                let _ = write!(
                    marker,
                    "<span class=\"n\" style=\"min-width:{hanging:.0}px\">{}</span>",
                    escape(&text)
                );
            }
        }
        let mut inner = String::new();
        let mut text = String::new();
        self.inline(node, &mut run_base, &mut inner, &mut text);
        if let Some(l) = level {
            let id = format!("h{}", self.headings.len());
            let title = text.trim().to_owned();
            if !title.is_empty() {
                self.headings.push(Heading {
                    level: l,
                    text: title.chars().take(200).collect(),
                    id: id.clone(),
                });
            }
            let _ = write!(
                self.out,
                "<h{l} id=\"{id}\" style=\"{}{}\">{marker}{inner}</h{l}>",
                para.css(),
                run_base.css()
            );
        } else if inner.is_empty() && marker.is_empty() {
            let _ = write!(self.out, "<p style=\"{}\">&nbsp;</p>", para.css());
        } else {
            let _ = write!(self.out, "<p style=\"{}\">{marker}{inner}</p>", para.css());
        }
    }

    /// The runs of `node` (a paragraph or a container inside one) as HTML; `text` collects the plain text.
    fn inline(&mut self, node: Node, base: &mut RunProps, html: &mut String, text: &mut String) {
        for item in node.children().filter(Node::is_element) {
            match item.tag_name().name() {
                "r" => self.run(item, base, html, text),
                "hyperlink" => {
                    let target = ox::rel_id(item)
                        .and_then(|id| self.rels.get(id))
                        .filter(|r| r.external)
                        .map(|r| r.target.clone())
                        .filter(|t| {
                            t.starts_with("https://")
                                || t.starts_with("http://")
                                || t.starts_with("mailto:")
                        });
                    let mut inner = String::new();
                    self.inline(item, base, &mut inner, text);
                    match target {
                        Some(href) => {
                            let _ = write!(html, "<a href=\"{}\">{inner}</a>", escape(&href));
                        }
                        None => html.push_str(&inner),
                    }
                }
                "ins" | "smartTag" | "fldSimple" | "customXml" => {
                    self.inline(item, base, html, text)
                }
                "sdt" => {
                    if let Some(content) = child(item, "sdtContent") {
                        self.inline(content, base, html, text);
                    }
                }
                _ => {}
            }
        }
    }

    fn run(&mut self, node: Node, base: &RunProps, html: &mut String, text: &mut String) {
        let mut props = base.clone();
        if let Some(rpr) = child(node, "rPr") {
            if let Some(style) = val(rpr, "rStyle") {
                props.over(&self.styles.run(style));
            }
            props.over(&RunProps::read(rpr));
        }
        let mut chunk = String::new();
        for item in node.children().filter(Node::is_element) {
            match item.tag_name().name() {
                "t" => {
                    let value = item.text().unwrap_or("");
                    text.push_str(value);
                    chunk.push_str(&escape(value));
                }
                "tab" => {
                    text.push(' ');
                    chunk.push_str("<span class=\"tab\"></span>");
                }
                "br" | "cr" => match attr(item, "type") {
                    Some("page") => chunk.push_str("<span class=\"pb\"></span>"),
                    _ => chunk.push_str("<br>"),
                },
                "noBreakHyphen" => chunk.push('\u{2011}'),
                "drawing" => chunk.push_str(&self.drawing(item)),
                "pict" | "object" => {
                    if let Some(data) = find(item, "imagedata").and_then(|d| attr(d, "id")) {
                        chunk.push_str(&self.picture(data, None, ""));
                    }
                }
                _ => {}
            }
        }
        if chunk.is_empty() {
            return;
        }
        let css = props.css();
        if css.is_empty() {
            html.push_str(&chunk);
        } else {
            let _ = write!(html, "<span style=\"{css}\">{chunk}</span>");
        }
    }

    fn drawing(&mut self, node: Node) -> String {
        let Some(blip) = find(node, "blip") else {
            return String::new();
        };
        let Some(id) = attr(blip, "embed") else {
            return String::new();
        };
        let size = find(node, "extent").and_then(|e| {
            Some((
                attr(e, "cx")?.parse::<f32>().ok()? / EMU_PER_PX,
                attr(e, "cy")?.parse::<f32>().ok()? / EMU_PER_PX,
            ))
        });
        let alt = find(node, "docPr")
            .and_then(|d| attr(d, "descr"))
            .unwrap_or("");
        self.picture(id, size, alt)
    }

    fn picture(&mut self, rel: &str, size: Option<(f32, f32)>, alt: &str) -> String {
        let Some(part) = self
            .rels
            .get(rel)
            .filter(|r| !r.external)
            .map(|r| r.target.clone())
        else {
            return String::new();
        };
        // A few dozen pictures at most: a document of thousands would be a zip bomb of pictures.
        if self.images >= 200 {
            return String::new();
        }
        let Some(url) = ox::image_data_url(self.zip, &part) else {
            return String::new();
        };
        self.images += 1;
        let style = match size {
            Some((w, h)) => {
                format!("width:{w:.0}px;height:auto;aspect-ratio:{w:.0}/{h:.0};max-width:100%")
            }
            None => "max-width:100%".to_owned(),
        };
        format!(
            "<img src=\"{url}\" alt=\"{}\" style=\"{style}\">",
            escape(alt)
        )
    }

    fn table(&mut self, node: Node) {
        let props = child(node, "tblPr");
        let grid = props
            .and_then(|p| val(p, "tblStyle"))
            .is_some_and(|s| s.contains("Grid"))
            || props.and_then(|p| child(p, "tblBorders")).is_some_and(|b| {
                b.children()
                    .filter(Node::is_element)
                    .any(|e| !matches!(attr(e, "val"), Some("nil" | "none")))
            });
        let widths: Vec<f32> = child(node, "tblGrid")
            .map(|g| {
                children(g, "gridCol")
                    .map(|c| {
                        attr(c, "w")
                            .and_then(|v| v.parse::<f32>().ok())
                            .unwrap_or(0.)
                            / 15.
                    })
                    .collect()
            })
            .unwrap_or_default();
        struct Cell<'a> {
            node: Node<'a, 'a>,
            col: usize,
            span: usize,
            merge: u8,
        }
        let mut rows: Vec<Vec<Cell>> = Vec::new();
        for tr in children(node, "tr") {
            let mut col = 0;
            let mut cells = Vec::new();
            for tc in children(tr, "tc") {
                let pr = child(tc, "tcPr");
                let span = pr
                    .and_then(|p| val(p, "gridSpan"))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1usize)
                    .max(1);
                let merge = match pr.and_then(|p| child(p, "vMerge")) {
                    None => 0,
                    Some(m) if attr(m, "val") == Some("restart") => 1,
                    Some(_) => 2,
                };
                cells.push(Cell {
                    node: tc,
                    col,
                    span,
                    merge,
                });
                col += span;
            }
            rows.push(cells);
        }
        let border = if grid {
            "border:1px solid #8a8a8a;"
        } else {
            ""
        };
        let _ = write!(
            self.out,
            "<table style=\"border-collapse:collapse;margin:8px 0;{}\">",
            if widths.iter().sum::<f32>() > 0. {
                "table-layout:fixed;"
            } else {
                ""
            }
        );
        if !widths.is_empty() {
            self.out.push_str("<colgroup>");
            for w in &widths {
                let _ = write!(self.out, "<col style=\"width:{w:.0}px\">");
            }
            self.out.push_str("</colgroup>");
        }
        for (ri, row) in rows.iter().enumerate() {
            self.out.push_str("<tr>");
            for cell in row {
                if cell.merge == 2 {
                    continue;
                }
                let mut rowspan = 1;
                if cell.merge == 1 {
                    for below in &rows[ri + 1..] {
                        if below.iter().any(|c| c.col == cell.col && c.merge == 2) {
                            rowspan += 1;
                        } else {
                            break;
                        }
                    }
                }
                let pr = child(cell.node, "tcPr");
                let fill = pr
                    .and_then(|p| child(p, "shd"))
                    .and_then(|s| attr(s, "fill"))
                    .filter(|f| is_hex(f))
                    .map(|f| format!("background:#{f};"))
                    .unwrap_or_default();
                let valign = match pr.and_then(|p| val(p, "vAlign")) {
                    Some("center") => "vertical-align:middle;",
                    Some("bottom") => "vertical-align:bottom;",
                    _ => "vertical-align:top;",
                };
                let _ = write!(
                    self.out,
                    "<td colspan=\"{}\" rowspan=\"{rowspan}\" style=\"{border}{fill}{valign}padding:3px 7px\">",
                    cell.span
                );
                let before = self.out.len();
                self.blocks(cell.node);
                if self.out.len() == before {
                    self.out.push_str("&nbsp;");
                }
                self.out.push_str("</td>");
            }
            self.out.push_str("</tr>");
        }
        self.out.push_str("</table>");
    }
}

/// A Word document (.docx) as one HTML page.
pub fn docx_to_html(bytes: &[u8]) -> Result<WordPage, String> {
    let zip = Zip::open(bytes)?;
    let text = zip.read_text("word/document.xml").ok_or("no document")?;
    let doc = ox::parse(&text)?;
    let body = child(doc.root_element(), "body").ok_or("no body")?;
    let mut word = Word {
        zip: &zip,
        rels: ox::rels(&zip, "word/document.xml"),
        styles: zip
            .read_text("word/styles.xml")
            .map(|s| Styles::read(&s))
            .unwrap_or_default(),
        numbering: zip
            .read_text("word/numbering.xml")
            .map(|s| Numbering::read(&s))
            .unwrap_or_default(),
        counters: HashMap::new(),
        headings: Vec::new(),
        images: 0,
        out: String::new(),
    };
    // The page: width and margins of the last section.
    let section = child(body, "sectPr");
    let page = section
        .and_then(|s| child(s, "pgSz"))
        .and_then(|s| attr(s, "w"))
        .and_then(|v| v.parse::<f32>().ok())
        .map_or(794., |v| v / 15.);
    let margin = |key: &str, default: f32| {
        section
            .and_then(|s| child(s, "pgMar"))
            .and_then(|m| attr(m, key))
            .and_then(|v| v.parse::<f32>().ok())
            .map_or(default, |v| v / 15.)
    };
    let (top, right, bottom, left) = (
        margin("top", 96.),
        margin("right", 96.),
        margin("bottom", 96.),
        margin("left", 96.),
    );
    word.blocks(body);
    let css = format!(
        "{DARK_LIGHT}html,body{{margin:0}}body{{padding:20px 12px;font:11pt/1.25 -apple-system,'Helvetica Neue',sans-serif;color:#242424}}\
         .sheet{{box-sizing:border-box;background:#fff;width:{page:.0}px;max-width:100%;min-height:200px;margin:0 auto;padding:{top:.0}px {right:.0}px {bottom:.0}px {left:.0}px;box-shadow:0 1px 6px rgba(0,0,0,.35);overflow-wrap:anywhere}}\
         h1,h2,h3,h4,h5,h6{{font-weight:inherit;line-height:1.25}}\
         h1{{font-size:20pt}}h2{{font-size:16pt}}h3{{font-size:13pt}}h4,h5,h6{{font-size:11pt}}\
         a{{color:#0563c1}}img{{vertical-align:middle}}td img{{max-width:100%}}\
         .n{{display:inline-block;text-indent:0}}.tab{{display:inline-block;width:2.5em}}\
         .pb{{display:block;height:0;border-top:1px dashed #bbb;margin:18px -8px}}p,td p{{min-height:1em}}td p{{margin-top:0;margin-bottom:0}}"
    );
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{}<style>{css}</style></head><body><div class=\"sheet\">{}</div></body></html>",
        policy(None),
        word.out
    );
    Ok(WordPage {
        html,
        headings: word.headings,
    })
}

// ==== PowerPoint ===========================================================================

#[derive(Default)]
struct Theme {
    colors: HashMap<String, String>,
    map: HashMap<String, String>,
}

fn hsl(rgb: (f32, f32, f32)) -> (f32, f32, f32) {
    let (r, g, b) = (rgb.0 / 255., rgb.1 / 255., rgb.2 / 255.);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.;
    if (max - min).abs() < 1e-6 {
        return (0., 0., l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2. - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < 1e-6 {
        (g - b) / d + if g < b { 6. } else { 0. }
    } else if (max - g).abs() < 1e-6 {
        (b - r) / d + 2.
    } else {
        (r - g) / d + 4.
    };
    (h / 6., s, l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s.abs() < 1e-6 {
        return (l * 255., l * 255., l * 255.);
    }
    let q = if l < 0.5 { l * (1. + s) } else { l + s - l * s };
    let p = 2. * l - q;
    let channel = |mut t: f32| {
        if t < 0. {
            t += 1.;
        }
        if t > 1. {
            t -= 1.;
        }
        let v = if t < 1. / 6. {
            p + (q - p) * 6. * t
        } else if t < 0.5 {
            q
        } else if t < 2. / 3. {
            p + (q - p) * (2. / 3. - t) * 6.
        } else {
            p
        };
        v * 255.
    };
    (channel(h + 1. / 3.), channel(h), channel(h - 1. / 3.))
}

impl Theme {
    fn read(zip: &Zip, master: &str) -> Theme {
        let mut theme = Theme::default();
        if let Some(text) = zip.read_text(master)
            && let Ok(doc) = ox::parse(&text)
            && let Some(map) = find(doc.root_element(), "clrMap")
        {
            for a in map.attributes() {
                theme.map.insert(a.name().to_owned(), a.value().to_owned());
            }
        }
        let rels = ox::rels(zip, master);
        let theme_part = rels
            .values()
            .find(|r| r.kind == "theme")
            .map(|r| r.target.clone());
        if let Some(text) = theme_part.and_then(|p| zip.read_text(&p))
            && let Ok(doc) = ox::parse(&text)
            && let Some(scheme) = find(doc.root_element(), "clrScheme")
        {
            for c in scheme.children().filter(Node::is_element) {
                let value = c
                    .children()
                    .find(|n| n.is_element())
                    .and_then(|n| attr(n, "val").filter(|v| is_hex(v)).or(attr(n, "lastClr")));
                if let Some(v) = value {
                    theme
                        .colors
                        .insert(c.tag_name().name().to_owned(), format!("#{v}"));
                }
            }
        }
        theme
    }

    /// A color element (`solidFill`'s child, `srgbClr`, `schemeClr`...) as a CSS color.
    fn color(&self, node: Node) -> Option<String> {
        let base = match node.tag_name().name() {
            "srgbClr" => attr(node, "val")
                .filter(|v| is_hex(v))
                .map(|v| format!("#{v}"))?,
            "sysClr" => format!("#{}", attr(node, "lastClr").unwrap_or("000000")),
            "prstClr" => match attr(node, "val")? {
                "white" => "#ffffff".to_owned(),
                "red" => "#ff0000".to_owned(),
                "green" => "#008000".to_owned(),
                "blue" => "#0000ff".to_owned(),
                "yellow" => "#ffff00".to_owned(),
                "gray" | "grey" => "#808080".to_owned(),
                _ => "#000000".to_owned(),
            },
            "schemeClr" => {
                let name = attr(node, "val")?;
                let mapped = self.map.get(name).map_or(name, String::as_str);
                self.colors
                    .get(mapped)
                    .or_else(|| self.colors.get(name))
                    .cloned()?
            }
            _ => return None,
        };
        let hex = base.trim_start_matches('#');
        let channel = |i: usize| {
            u8::from_str_radix(hex.get(i..i + 2)?, 16)
                .ok()
                .map(f32::from)
        };
        let mut rgb = (channel(0)?, channel(2)?, channel(4)?);
        let mut alpha = 1.;
        for m in node.children().filter(Node::is_element) {
            let v = attr(m, "val")
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.)
                / 100_000.;
            match m.tag_name().name() {
                "lumMod" | "lumOff" => {
                    let (h, s, l) = hsl(rgb);
                    let l = if m.tag_name().name() == "lumMod" {
                        l * v
                    } else {
                        l + v
                    };
                    rgb = from_hsl(h, s, l.clamp(0., 1.));
                }
                "tint" => {
                    rgb = (
                        rgb.0 + (255. - rgb.0) * (1. - v),
                        rgb.1 + (255. - rgb.1) * (1. - v),
                        rgb.2 + (255. - rgb.2) * (1. - v),
                    )
                }
                "shade" => rgb = (rgb.0 * v, rgb.1 * v, rgb.2 * v),
                "alpha" => alpha = v,
                _ => {}
            }
        }
        let (r, g, b) = (
            rgb.0.round().clamp(0., 255.) as u8,
            rgb.1.round().clamp(0., 255.) as u8,
            rgb.2.round().clamp(0., 255.) as u8,
        );
        Some(if alpha < 0.999 {
            format!("rgba({r},{g},{b},{alpha:.2})")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}")
        })
    }

    /// The color a fill node (`solidFill`, `gradFill`...) paints with, if any.
    fn fill(&self, props: Node) -> Option<String> {
        if let Some(solid) = child(props, "solidFill") {
            return solid
                .children()
                .find(Node::is_element)
                .and_then(|c| self.color(c));
        }
        child(props, "gradFill")
            .and_then(|g| find(g, "gs"))
            .and_then(|gs| gs.children().find(Node::is_element))
            .and_then(|c| self.color(c))
    }
}

/// Where a shape's inherited text styles and placement come from, nearest first.
struct Sources<'a> {
    layout: Option<Node<'a, 'a>>,
    master: Option<Node<'a, 'a>>,
}

struct Xf {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    rot: f32,
    flip_h: bool,
    flip_v: bool,
}

/// Maps a group's child coordinates onto the slide.
#[derive(Clone, Copy)]
struct Tx {
    ox: f32,
    oy: f32,
    sx: f32,
    sy: f32,
}

impl Tx {
    const ID: Tx = Tx {
        ox: 0.,
        oy: 0.,
        sx: 1.,
        sy: 1.,
    };
}

fn xfrm_of(node: Node) -> Option<Xf> {
    let xf = find(node, "xfrm")?;
    let off = child(xf, "off")?;
    let ext = child(xf, "ext")?;
    let n = |node: Node, k: &str| {
        attr(node, k)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.)
    };
    Some(Xf {
        x: n(off, "x"),
        y: n(off, "y"),
        w: n(ext, "cx"),
        h: n(ext, "cy"),
        rot: attr(xf, "rot")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.)
            / 60_000.,
        flip_h: attr(xf, "flipH") == Some("1"),
        flip_v: attr(xf, "flipV") == Some("1"),
    })
}

fn px(emu: f32) -> f32 {
    emu / EMU_PER_PX
}

/// The placeholder in `tree` that matches `(type, idx)`.
fn placeholder<'a>(tree: Node<'a, 'a>, kind: &str, idx: &str) -> Option<Node<'a, 'a>> {
    let same = |a: &str, b: &str| {
        a == b || (a == "ctrTitle" && b == "title") || (a == "title" && b == "ctrTitle")
    };
    let shapes = || {
        tree.descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "sp")
    };
    let ph = |sp: Node<'a, 'a>| find(sp, "ph");
    shapes()
        .find(|sp| ph(*sp).is_some_and(|p| !idx.is_empty() && attr(p, "idx") == Some(idx)))
        .or_else(|| {
            shapes().find(|sp| {
                ph(*sp).is_some_and(|p| {
                    same(attr(p, "type").unwrap_or("body"), kind)
                        && (idx.is_empty() || attr(p, "idx").is_none() || kind != "body")
                })
            })
        })
}

#[derive(Default, Clone)]
struct LvlProps {
    size: Option<f32>,
    bold: Option<bool>,
    italic: Option<bool>,
    color: Option<String>,
    font: Option<String>,
    left: Option<f32>,
    indent: Option<f32>,
    align: Option<String>,
    bullet: Option<String>,
    autonum: Option<String>,
    no_bullet: bool,
    before: Option<f32>,
    line: Option<f32>,
}

struct Deck2<'a> {
    zip: &'a Zip<'a>,
    theme: Theme,
    out: String,
    images: usize,
    rels: HashMap<String, Rel>,
    sources: Sources<'a>,
}

fn lvl_props(theme: &Theme, node: Node) -> LvlProps {
    let mut p = LvlProps {
        left: attr(node, "marL")
            .and_then(|v| v.parse::<f32>().ok())
            .map(px),
        indent: attr(node, "indent")
            .and_then(|v| v.parse::<f32>().ok())
            .map(px),
        align: attr(node, "algn").map(str::to_owned),
        ..LvlProps::default()
    };
    if child(node, "buNone").is_some() {
        p.no_bullet = true;
    }
    p.bullet = child(node, "buChar")
        .and_then(|b| attr(b, "char"))
        .map(str::to_owned);
    p.autonum = child(node, "buAutoNum")
        .and_then(|b| attr(b, "type"))
        .map(str::to_owned);
    if let Some(sp) = child(node, "spcBef")
        .and_then(|s| child(s, "spcPts"))
        .and_then(|s| attr(s, "val"))
        .and_then(|v| v.parse::<f32>().ok())
    {
        p.before = Some(sp / 100. * 4. / 3.);
    }
    if let Some(pct) = child(node, "lnSpc")
        .and_then(|s| child(s, "spcPct"))
        .and_then(|s| attr(s, "val"))
        .and_then(|v| v.parse::<f32>().ok())
    {
        p.line = Some(pct / 100_000.);
    }
    if let Some(d) = child(node, "defRPr").or(if node.tag_name().name() == "rPr" {
        Some(node)
    } else {
        None
    }) {
        p.size = attr(d, "sz")
            .and_then(|v| v.parse::<f32>().ok())
            .map(|v| v / 100.);
        p.bold = attr(d, "b").map(|v| v == "1");
        p.italic = attr(d, "i").map(|v| v == "1");
        p.color = theme.fill(d);
        p.font = child(d, "latin")
            .and_then(|l| attr(l, "typeface"))
            .filter(|f| !f.starts_with('+'))
            .map(str::to_owned);
    }
    p
}

impl LvlProps {
    fn fill_from(&mut self, other: &LvlProps) {
        macro_rules! fill {
            ($($f:ident),*) => {$( if self.$f.is_none() { self.$f = other.$f.clone(); } )*};
        }
        fill!(
            size, bold, italic, color, font, left, indent, align, bullet, autonum, before, line
        );
        if !self.no_bullet && other.no_bullet && self.bullet.is_none() && self.autonum.is_none() {
            self.no_bullet = true;
        }
    }
}

impl<'a> Deck2<'a> {
    /// The paragraph-level styles for `lvl` (1-based) from a shape's own list style down to the master's.
    fn levels(&self, shape: Node<'a, 'a>, kind: &str, idx: &str, lvl: usize) -> LvlProps {
        let name = format!("lvl{lvl}pPr");
        let mut acc = LvlProps::default();
        let own = |sp: Node<'a, 'a>| {
            find(sp, "lstStyle")
                .and_then(|l| child(l, &name))
                .map(|n| lvl_props(&self.theme, n))
        };
        if let Some(p) = own(shape) {
            acc.fill_from(&p);
        }
        for tree in [self.sources.layout, self.sources.master]
            .into_iter()
            .flatten()
        {
            if let Some(sp) = placeholder(tree, kind, idx)
                && let Some(p) = own(sp)
            {
                acc.fill_from(&p);
            }
        }
        if let Some(master) = self.sources.master {
            let style = match kind {
                "title" | "ctrTitle" => "titleStyle",
                "body" | "subTitle" | "obj" => "bodyStyle",
                _ => "otherStyle",
            };
            if let Some(p) = find(master, style).and_then(|s| child(s, &name)) {
                acc.fill_from(&lvl_props(&self.theme, p));
            }
        }
        acc
    }

    fn text(
        &mut self,
        body: Node<'a, 'a>,
        shape: Node<'a, 'a>,
        kind: &str,
        idx: &str,
        in_cell: bool,
    ) -> String {
        let scale = find(body, "normAutofit")
            .and_then(|n| attr(n, "fontScale"))
            .and_then(|v| v.parse::<f32>().ok())
            .map_or(1., |v| v / 100_000.);
        let mut out = String::new();
        let mut counters: Vec<u32> = Vec::new();
        for p in children(body, "p") {
            let ppr = child(p, "pPr");
            let lvl = ppr
                .and_then(|n| attr(n, "lvl"))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0)
                + 1;
            let mut props = ppr.map(|n| lvl_props(&self.theme, n)).unwrap_or_default();
            props.size = None;
            props.bold = None;
            props.italic = None;
            props.color = None;
            props.font = None;
            if let Some(d) = ppr.and_then(|n| child(n, "defRPr")) {
                let own = lvl_props(&self.theme, d);
                props.size = own.size;
            }
            props.fill_from(&self.levels(shape, kind, idx, lvl.min(9)));
            let bulleted =
                !props.no_bullet && !in_cell && (props.bullet.is_some() || props.autonum.is_some());
            if props.autonum.is_some() && bulleted {
                counters.resize(lvl.max(counters.len()), 0);
                counters[lvl - 1] += 1;
                counters.truncate(lvl);
            } else {
                counters.clear();
            }
            let mut css = String::new();
            let mut left = props.left.unwrap_or(0.);
            let mut indent = props.indent.unwrap_or(0.);
            if bulleted && indent >= 0. {
                // A bullet with no hanging indent of its own still gets room to hang in.
                left += 20.;
                indent = -20.;
            }
            let _ = write!(
                css,
                "margin:0;padding-left:{:.1}px;text-indent:{indent:.1}px;",
                left.max(0.)
            );
            if let Some(color) = &props.color {
                let _ = write!(css, "color:{color};");
            }
            match props.align.as_deref() {
                Some("ctr") => css.push_str("text-align:center;"),
                Some("r") => css.push_str("text-align:right;"),
                Some("just") => css.push_str("text-align:justify;"),
                _ => {}
            }
            if let Some(b) = props.before {
                let _ = write!(css, "margin-top:{b:.1}px;");
            }
            if let Some(l) = props.line {
                let _ = write!(css, "line-height:{:.2};", l * 1.2);
            }
            let mut runs = String::new();
            let default_size = props.size.unwrap_or(18.) * scale;
            for item in p.children().filter(Node::is_element) {
                match item.tag_name().name() {
                    "r" | "fld" => {
                        let text = child(item, "t").and_then(|t| t.text()).unwrap_or("");
                        if text.is_empty() {
                            continue;
                        }
                        let rpr = child(item, "rPr");
                        let own = rpr.map(|n| lvl_props(&self.theme, n)).unwrap_or_default();
                        let size = own.size.map_or(default_size, |s| s * scale);
                        let mut style = format!("font-size:{:.2}px;", size * 4. / 3.);
                        if own.bold.or(props.bold) == Some(true) {
                            style.push_str("font-weight:700;");
                        }
                        if own.italic.or(props.italic) == Some(true) {
                            style.push_str("font-style:italic;");
                        }
                        if let Some(r) = rpr {
                            let u = attr(r, "u").is_some_and(|u| u != "none");
                            let s = attr(r, "strike").is_some_and(|s| s != "noStrike");
                            if u || s {
                                let _ = write!(
                                    style,
                                    "text-decoration:{}{}{};",
                                    if u { "underline" } else { "" },
                                    if u && s { " " } else { "" },
                                    if s { "line-through" } else { "" }
                                );
                            }
                            match attr(r, "baseline").and_then(|b| b.parse::<i32>().ok()) {
                                Some(b) if b > 0 => {
                                    style.push_str("vertical-align:super;font-size:70%;")
                                }
                                Some(b) if b < 0 => {
                                    style.push_str("vertical-align:sub;font-size:70%;")
                                }
                                _ => {}
                            }
                        }
                        if let Some(c) = own.color.or(props.color.clone()) {
                            let _ = write!(style, "color:{c};");
                        } else if !in_cell {
                            let fallback = self
                                .theme
                                .colors
                                .get(self.theme.map.get("tx1").map_or("dk1", String::as_str))
                                .cloned()
                                .unwrap_or_else(|| "#000000".into());
                            let _ = write!(style, "color:{fallback};");
                        }
                        if let Some(f) = own
                            .font
                            .or(props.font.clone())
                            .as_deref()
                            .and_then(safe_font)
                        {
                            let _ = write!(style, "font-family:{f};");
                        }
                        let _ = write!(runs, "<span style=\"{style}\">{}</span>", escape(text));
                    }
                    "br" => runs.push_str("<br>"),
                    _ => {}
                }
            }
            if bulleted {
                let mark = match (&props.bullet, &props.autonum) {
                    (_, Some(kind)) => {
                        let n = counters.last().copied().unwrap_or(1);
                        match kind.as_str() {
                            k if k.starts_with("alphaLc") => format!("{}.", letters(n, false)),
                            k if k.starts_with("alphaUc") => format!("{}.", letters(n, true)),
                            k if k.starts_with("romanLc") => {
                                format!("{}.", roman(n).to_lowercase())
                            }
                            k if k.starts_with("romanUc") => format!("{}.", roman(n)),
                            _ => format!("{n}."),
                        }
                    }
                    (Some(c), None)
                        if c.chars().all(|c| {
                            c.is_alphanumeric()
                                || ('\u{2010}'..='\u{27bf}').contains(&c)
                                || c == '-'
                        }) =>
                    {
                        c.clone()
                    }
                    _ => "\u{2022}".to_owned(),
                };
                let hang = -indent;
                let _ = write!(
                    out,
                    "<p style=\"{css}\"><span class=\"b\" style=\"width:{hang:.0}px\">{}</span>{runs}</p>",
                    escape(&mark)
                );
            } else if runs.is_empty() {
                let _ = write!(
                    out,
                    "<p style=\"{css}font-size:{:.1}px\">&nbsp;</p>",
                    default_size * 4. / 3.
                );
            } else {
                let _ = write!(out, "<p style=\"{css}\">{runs}</p>");
            }
        }
        out
    }

    fn picture(&mut self, rel: &str) -> Option<String> {
        if self.images >= 120 {
            return None;
        }
        let part = self.rels.get(rel).filter(|r| !r.external)?.target.clone();
        let url = ox::image_data_url(self.zip, &part)?;
        self.images += 1;
        Some(url)
    }

    fn shapes(&mut self, tree: Node<'a, 'a>, tx: Tx) {
        for node in tree.children().filter(Node::is_element) {
            match node.tag_name().name() {
                "sp" | "cxnSp" => self.shape(node, tx),
                "pic" => self.pic(node, tx),
                "graphicFrame" => self.frame(node, tx),
                "grpSp" => {
                    let xf = child(node, "grpSpPr").and_then(|p| child(p, "xfrm"));
                    let inner = xf.and_then(|xf| {
                        let n = |c: Node, k: &str| attr(c, k).and_then(|v| v.parse::<f32>().ok());
                        let (off, ext) = (child(xf, "off")?, child(xf, "ext")?);
                        let (choff, chext) = (child(xf, "chOff")?, child(xf, "chExt")?);
                        let (sx, sy) = (
                            n(ext, "cx")? / n(chext, "cx")?.max(1.),
                            n(ext, "cy")? / n(chext, "cy")?.max(1.),
                        );
                        Some(Tx {
                            ox: tx.ox + (n(off, "x")? - n(choff, "x")? * sx) * tx.sx,
                            oy: tx.oy + (n(off, "y")? - n(choff, "y")? * sy) * tx.sy,
                            sx: sx * tx.sx,
                            sy: sy * tx.sy,
                        })
                    });
                    self.shapes(node, inner.unwrap_or(tx));
                }
                _ => {}
            }
        }
    }

    /// A box at the shape's place: `left/top/width/height` and the rotation, as CSS.
    fn place(xf: &Xf, tx: Tx) -> String {
        let mut s = format!(
            "left:{:.1}px;top:{:.1}px;width:{:.1}px;height:{:.1}px;",
            px(tx.ox + xf.x * tx.sx),
            px(tx.oy + xf.y * tx.sy),
            px(xf.w * tx.sx),
            px(xf.h * tx.sy)
        );
        let mut t = String::new();
        if xf.rot != 0. {
            let _ = write!(t, "rotate({:.2}deg) ", xf.rot);
        }
        if xf.flip_h || xf.flip_v {
            let _ = write!(
                t,
                "scale({},{})",
                if xf.flip_h { -1 } else { 1 },
                if xf.flip_v { -1 } else { 1 }
            );
        }
        if !t.is_empty() {
            let _ = write!(s, "transform:{t};");
        }
        s
    }

    fn shape(&mut self, node: Node<'a, 'a>, tx: Tx) {
        let ph = find(node, "ph");
        let (kind, idx) = ph.map_or(("other", ""), |p| {
            (
                attr(p, "type").unwrap_or("body"),
                attr(p, "idx").unwrap_or(""),
            )
        });
        let sp_pr = child(node, "spPr");
        let mut xf = sp_pr.and_then(xfrm_of);
        if xf.is_none() && ph.is_some() {
            xf = [self.sources.layout, self.sources.master]
                .into_iter()
                .flatten()
                .find_map(|tree| {
                    placeholder(tree, kind, idx)
                        .and_then(|sp| child(sp, "spPr"))
                        .and_then(xfrm_of)
                });
        }
        let Some(xf) = xf else {
            return;
        };
        if node.tag_name().name() == "cxnSp" {
            let w = px(xf.w * tx.sx);
            let h = px(xf.h * tx.sy);
            let ln = sp_pr.and_then(|p| child(p, "ln"));
            let color = ln
                .and_then(|l| self.theme.fill(l))
                .unwrap_or_else(|| "#888888".into());
            let width = ln
                .and_then(|l| attr(l, "w"))
                .and_then(|v| v.parse::<f32>().ok())
                .map_or(1., |v| (px(v)).max(1.));
            let (y1, y2) = if xf.flip_v { (h, 0.) } else { (0., h) };
            let (x1, x2) = if xf.flip_h { (w, 0.) } else { (0., w) };
            let _ = write!(
                self.out,
                "<svg class=\"sh\" style=\"left:{:.1}px;top:{:.1}px;overflow:visible\" width=\"{:.1}\" height=\"{:.1}\"><line x1=\"{x1:.1}\" y1=\"{y1:.1}\" x2=\"{x2:.1}\" y2=\"{y2:.1}\" stroke=\"{color}\" stroke-width=\"{width:.1}\"/></svg>",
                px(tx.ox + xf.x * tx.sx),
                px(tx.oy + xf.y * tx.sy),
                w.max(1.),
                h.max(1.)
            );
            return;
        }
        let mut style = Self::place(&xf, tx);
        if let Some(props) = sp_pr {
            if let Some(fill) = self.theme.fill(props) {
                let _ = write!(style, "background:{fill};");
            }
            if let Some(blip) = child(props, "blipFill")
                .and_then(|b| child(b, "blip"))
                .and_then(|b| attr(b, "embed"))
                .map(str::to_owned)
                && let Some(url) = self.picture(&blip)
            {
                let _ = write!(style, "background:url({url}) center/cover;");
            }
            if let Some(ln) = child(props, "ln")
                && child(ln, "noFill").is_none()
                && let Some(color) = self.theme.fill(ln)
            {
                let w = attr(ln, "w")
                    .and_then(|v| v.parse::<f32>().ok())
                    .map_or(1., |v| px(v).max(1.));
                let _ = write!(
                    style,
                    "box-sizing:border-box;border:{w:.1}px solid {color};"
                );
            }
            match child(props, "prstGeom").and_then(|g| attr(g, "prst")) {
                Some("ellipse") => style.push_str("border-radius:50%;"),
                Some("roundRect") => {
                    let r = px(xf.w.min(xf.h) * tx.sx.min(tx.sy)) * 0.1667;
                    let _ = write!(style, "border-radius:{r:.1}px;");
                }
                Some("triangle") => style.push_str("clip-path:polygon(50% 0,0 100%,100% 100%);"),
                Some("diamond") => {
                    style.push_str("clip-path:polygon(50% 0,100% 50%,50% 100%,0 50%);")
                }
                _ => {}
            }
        }
        let mut inner = String::new();
        let mut anchor = "flex-start";
        let mut pad = (px(45_720.), px(91_440.));
        if let Some(body) = child(node, "txBody") {
            if let Some(pr) = child(body, "bodyPr") {
                anchor = match attr(pr, "anchor") {
                    Some("ctr") => "center",
                    Some("b") => "flex-end",
                    _ => "flex-start",
                };
                let inset = |k: &str, d: f32| {
                    attr(pr, k)
                        .and_then(|v| v.parse::<f32>().ok())
                        .map_or(px(d), px)
                };
                pad = (inset("tIns", 45_720.), inset("lIns", 91_440.));
            }
            inner = self.text(body, node, kind, idx, false);
        }
        let ph_anchor = if anchor == "flex-start"
            && child(node, "txBody")
                .and_then(|b| child(b, "bodyPr"))
                .and_then(|p| attr(p, "anchor"))
                .is_none()
        {
            [self.sources.layout, self.sources.master]
                .into_iter()
                .flatten()
                .find_map(|t| {
                    placeholder(t, kind, idx)
                        .and_then(|sp| find(sp, "bodyPr"))
                        .and_then(|p| attr(p, "anchor"))
                        .map(|a| match a {
                            "ctr" => "center",
                            "b" => "flex-end",
                            _ => "flex-start",
                        })
                })
        } else {
            None
        };
        let _ = write!(
            self.out,
            "<div class=\"sh\" style=\"{style}\"><div class=\"tx\" style=\"justify-content:{};padding:{:.1}px {:.1}px\">{inner}</div></div>",
            ph_anchor.unwrap_or(anchor),
            pad.0,
            pad.1
        );
    }

    fn pic(&mut self, node: Node<'a, 'a>, tx: Tx) {
        let Some(xf) = child(node, "spPr").and_then(xfrm_of) else {
            return;
        };
        let Some(blip) = find(node, "blip")
            .and_then(|b| attr(b, "embed"))
            .map(str::to_owned)
        else {
            return;
        };
        let Some(url) = self.picture(&blip) else {
            return;
        };
        let crop = child(node, "blipFill").and_then(|b| child(b, "srcRect"));
        let c = |k: &str| {
            crop.and_then(|n| attr(n, k))
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.)
                / 1000.
        };
        let (l, t, r, b) = (c("l"), c("t"), c("r"), c("b"));
        let (iw, ih) = (
            100. / (100. - l - r).max(1.) * 100.,
            100. / (100. - t - b).max(1.) * 100.,
        );
        let _ = write!(
            self.out,
            "<div class=\"sh\" style=\"{}overflow:hidden\"><img src=\"{url}\" style=\"position:absolute;left:{:.2}%;top:{:.2}%;width:{iw:.2}%;height:{ih:.2}%\"></div>",
            Self::place(&xf, tx),
            -l * iw / 100.,
            -t * ih / 100.
        );
    }

    fn frame(&mut self, node: Node<'a, 'a>, tx: Tx) {
        let Some(xf) = xfrm_of(node) else {
            return;
        };
        let Some(table) = find(node, "tbl") else {
            let _ = write!(
                self.out,
                "<div class=\"sh missing\" style=\"{}\"><span>{}</span></div>",
                Self::place(&xf, tx),
                escape(crate::i18n::t("Conteúdo não exibido"))
            );
            return;
        };
        let cols: Vec<f32> = child(table, "tblGrid")
            .map(|g| {
                children(g, "gridCol")
                    .map(|c| {
                        px(attr(c, "w")
                            .and_then(|v| v.parse::<f32>().ok())
                            .unwrap_or(0.))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let first_row = child(table, "tblPr").is_some_and(|p| attr(p, "firstRow") == Some("1"));
        let banded = child(table, "tblPr").is_some_and(|p| attr(p, "bandRow") == Some("1"));
        let accent = self
            .theme
            .colors
            .get("accent1")
            .cloned()
            .unwrap_or_else(|| "#4472c4".into());
        let _ = write!(
            self.out,
            "<div class=\"sh\" style=\"{}\"><table style=\"border-collapse:collapse;width:100%\"><colgroup>",
            Self::place(&xf, tx)
        );
        for w in &cols {
            let _ = write!(self.out, "<col style=\"width:{w:.1}px\">");
        }
        self.out.push_str("</colgroup>");
        for (ri, tr) in children(table, "tr").enumerate() {
            let h = px(attr(tr, "h")
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.));
            let _ = write!(self.out, "<tr style=\"height:{h:.1}px\">");
            for tc in children(tr, "tc") {
                if attr(tc, "hMerge") == Some("1") || attr(tc, "vMerge") == Some("1") {
                    continue;
                }
                let pr = child(tc, "tcPr");
                let header = first_row && ri == 0;
                let mut cell = String::from(
                    "border:1px solid rgba(255,255,255,.7);padding:4px 8px;vertical-align:middle;",
                );
                let own_fill = pr.and_then(|p| self.theme.fill(p));
                let fill = own_fill.clone().or_else(|| {
                    if header {
                        Some(accent.clone())
                    } else if banded {
                        Some(if ri % 2 == 1 {
                            "rgba(68,114,196,.22)".to_owned()
                        } else {
                            "rgba(68,114,196,.1)".to_owned()
                        })
                    } else {
                        None
                    }
                });
                if let Some(f) = fill {
                    let _ = write!(cell, "background:{f};");
                }
                if header && own_fill.is_none() {
                    cell.push_str("color:#fff;font-weight:700;");
                }
                let span = attr(tc, "gridSpan").unwrap_or("1");
                let rowspan = attr(tc, "rowSpan").unwrap_or("1");
                let text = child(tc, "txBody")
                    .map(|b| self.text(b, node, "other", "", true))
                    .unwrap_or_default();
                let _ = write!(
                    self.out,
                    "<td colspan=\"{}\" rowspan=\"{}\" style=\"{cell}\">{text}</td>",
                    escape(span),
                    escape(rowspan)
                );
            }
            self.out.push_str("</tr>");
        }
        self.out.push_str("</table></div>");
    }
}

const DECK_SCRIPT: &str = r#"
const slides=[...document.querySelectorAll('#deck>.slide')];
const W=+document.body.dataset.w,H=+document.body.dataset.h;
const strip=document.getElementById('strip'),stage=document.getElementById('stage'),fit=document.getElementById('fit'),notes=document.getElementById('notes');
let cur=-1;const tell=i=>{try{window.webkit.messageHandlers.sirus.postMessage(String(i))}catch(e){}};
const thumbs=slides.map((s,i)=>{const t=document.createElement('button');t.className='thumb';t.setAttribute('aria-label','Slide '+(i+1));
const c=s.cloneNode(true);c.style.transform='scale('+(150/W)+')';t.style.height=(H*150/W)+'px';t.append(c);
const n=document.createElement('span');n.textContent=i+1;t.append(n);t.onclick=()=>show(i);strip.append(t);return t});
function layout(){const s=Math.min((stage.clientWidth-40)/W,(stage.clientHeight-(notes.offsetHeight||0)-40)/H);fit.style.width=W*s+'px';fit.style.height=H*s+'px';const c=fit.firstChild;if(c)c.style.transform='scale('+s+')'}
function show(i){i=Math.max(0,Math.min(slides.length-1,i));if(i===cur)return;cur=i;fit.replaceChildren(slides[i].cloneNode(true));
const nt=slides[i].dataset.notes||'';notes.textContent=nt;notes.hidden=!nt;
thumbs.forEach((t,j)=>t.classList.toggle('on',j===i));thumbs[i].scrollIntoView({block:'nearest'});layout();tell(i)}
addEventListener('resize',layout);
addEventListener('keydown',e=>{const k=e.key;if(e.metaKey||e.ctrlKey)return;
if(k==='ArrowRight'||k==='ArrowDown'||k==='PageDown'||k===' '){show(cur+1);e.preventDefault()}
else if(k==='ArrowLeft'||k==='ArrowUp'||k==='PageUp'){show(cur-1);e.preventDefault()}
else if(k==='Home'){show(0);e.preventDefault()}else if(k==='End'){show(slides.length-1);e.preventDefault()}});
window.sirusShow=show;show(0);
"#;

/// A presentation (.pptx) as a page with a strip of thumbnails and the slide shown large; the
/// page's script reports the slide shown to the app and takes `sirusShow(i)` from it.
pub fn pptx_to_html(bytes: &[u8]) -> Result<Deck, String> {
    let zip = Zip::open(bytes)?;
    let text = zip
        .read_text("ppt/presentation.xml")
        .ok_or("no presentation")?;
    let doc = ox::parse(&text)?;
    let root = doc.root_element();
    let size = find(root, "sldSz").ok_or("no slide size")?;
    let n = |k: &str| {
        attr(size, k)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.)
    };
    let (width, height) = (px(n("cx")), px(n("cy")));
    if width < 10. || height < 10. {
        return Err("bad slide size".to_owned());
    }
    let deck_rels = ox::rels(&zip, "ppt/presentation.xml");
    let order: Vec<String> = find(root, "sldIdLst")
        .map(|l| {
            children(l, "sldId")
                .filter_map(|s| {
                    ox::rel_id(s)
                        .and_then(|id| deck_rels.get(id))
                        .map(|r| r.target.clone())
                })
                .collect()
        })
        .unwrap_or_default();
    if order.is_empty() || order.len() > 300 {
        return Err("no slides".to_owned());
    }
    let mut slides = String::new();
    let mut shown = 0;
    for part in &order {
        let Some(text) = zip.read_text(part) else {
            continue;
        };
        let Ok(doc) = ox::parse(&text) else {
            continue;
        };
        let rels = ox::rels(&zip, part);
        let layout_part = rels
            .values()
            .find(|r| r.kind == "slideLayout")
            .map(|r| r.target.clone());
        let layout_text = layout_part.as_deref().and_then(|p| zip.read_text(p));
        let layout_doc = layout_text.as_deref().and_then(|t| ox::parse(t).ok());
        let master_part = layout_part.as_deref().and_then(|p| {
            ox::rels(&zip, p)
                .into_values()
                .find(|r| r.kind == "slideMaster")
                .map(|r| r.target)
        });
        let master_text = master_part.as_deref().and_then(|p| zip.read_text(p));
        let master_doc = master_text.as_deref().and_then(|t| ox::parse(t).ok());
        let theme = master_part
            .as_deref()
            .map(|m| Theme::read(&zip, m))
            .unwrap_or_default();
        let slide_root = doc.root_element();
        // Background: the slide's own, then its layout's, then the master's.
        let bg = [
            Some(slide_root),
            layout_doc.as_ref().map(|d| d.root_element()),
            master_doc.as_ref().map(|d| d.root_element()),
        ]
        .into_iter()
        .flatten()
        .find_map(|r| {
            find(r, "bg")
                .and_then(|b| child(b, "bgPr").or_else(|| child(b, "bgRef")))
                .and_then(|p| {
                    theme.fill(p).or_else(|| {
                        p.children()
                            .find(Node::is_element)
                            .and_then(|c| theme.color(c))
                    })
                })
        });
        let mut deck = Deck2 {
            zip: &zip,
            theme,
            out: String::new(),
            images: 0,
            rels: rels.clone(),
            sources: Sources {
                layout: layout_doc.as_ref().map(|d| d.root_element()),
                master: master_doc.as_ref().map(|d| d.root_element()),
            },
        };
        if let Some(tree) = find(slide_root, "spTree") {
            deck.shapes(tree, Tx::ID);
        }
        let notes_text = rels
            .values()
            .find(|r| r.kind == "notesSlide")
            .and_then(|r| zip.read_text(&r.target))
            .map(|t| notes_of(&t))
            .unwrap_or_default();
        let hidden = attr(slide_root, "show") == Some("0");
        let _ = write!(
            slides,
            "<div class=\"slide\" data-notes=\"{}\"{} style=\"width:{width:.0}px;height:{height:.0}px;background:{}\">{}</div>",
            escape(&notes_text),
            if hidden { " data-hidden=\"1\"" } else { "" },
            bg.unwrap_or_else(|| "#ffffff".into()),
            deck.out
        );
        shown += 1;
    }
    if shown == 0 {
        return Err("no readable slides".to_owned());
    }
    let css = format!(
        "{DARK_LIGHT}html,body{{margin:0;height:100%;overflow:hidden}}body{{display:flex;font:13px -apple-system,sans-serif;color:#222}}\
         #strip{{width:178px;flex:none;overflow-y:auto;padding:10px 0;box-sizing:border-box;border-right:1px solid rgba(128,128,128,.25);display:flex;flex-direction:column;align-items:center;gap:10px}}\
         .thumb{{position:relative;width:150px;flex:none;padding:0;border:2px solid transparent;outline:0;border-radius:4px;background:none;cursor:pointer;overflow:hidden;box-sizing:content-box}}\
         .thumb.on{{border-color:#4b8ff5}}.thumb .slide{{transform-origin:0 0;pointer-events:none}}\
         .thumb>span{{position:absolute;left:4px;bottom:3px;font-size:10px;color:#fff;background:rgba(0,0,0,.55);padding:0 4px;border-radius:3px}}\
         #stage{{flex:1;min-width:0;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:10px;overflow:hidden}}\
         #fit{{position:relative;flex:none;box-shadow:0 2px 14px rgba(0,0,0,.45);overflow:hidden}}\
         #deck{{display:none}}.slide{{position:relative;overflow:hidden;transform-origin:0 0;color:#000}}\
         #fit .slide{{transform-origin:0 0}}#fit .slide[data-hidden]::after{{content:'';position:absolute;inset:0;background:rgba(255,255,255,.5)}}\
         .sh{{position:absolute;box-sizing:border-box}}.tx{{display:flex;flex-direction:column;width:100%;height:100%;box-sizing:border-box;overflow:hidden;overflow-wrap:break-word}}\
         .tx p{{white-space:pre-wrap}}.tx .b{{display:inline-block;text-indent:0}}\
         .missing{{display:flex;align-items:center;justify-content:center;border:1px dashed rgba(128,128,128,.6);color:#888;font-size:14px}}\
         #notes{{max-width:80%;max-height:90px;overflow:auto;font-size:12px;opacity:.75;white-space:pre-wrap;user-select:text;-webkit-user-select:text}}\
         @media (prefers-color-scheme:dark){{body{{color:#ddd}}}}"
    );
    let nonce = "sirusdeck";
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{}<style>{css}</style></head><body data-w=\"{width:.0}\" data-h=\"{height:.0}\">\
         <div id=\"strip\"></div><div id=\"stage\"><div id=\"fit\"></div><div id=\"notes\" hidden></div></div><div id=\"deck\">{slides}</div>\
         <script nonce=\"{nonce}\">{DECK_SCRIPT}</script></body></html>",
        policy(Some(nonce))
    );
    Ok(Deck {
        html,
        slides: shown,
    })
}

/// The speaker notes of a notes page: the paragraphs of its body placeholder.
fn notes_of(xml: &str) -> String {
    let Ok(doc) = ox::parse(xml) else {
        return String::new();
    };
    let mut lines = Vec::new();
    for sp in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "sp")
    {
        if find(sp, "ph").and_then(|p| attr(p, "type")) != Some("body") {
            continue;
        }
        for p in sp
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "p")
        {
            let line: String = p
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "t")
                .filter_map(|t| t.text())
                .collect();
            lines.push(line);
        }
    }
    lines.join("\n").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachment_ooxml::tests::zip_store;

    const W: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"";

    fn docx(body: &str) -> Vec<u8> {
        let document = format!("<w:document {W}><w:body>{body}</w:body></w:document>");
        let rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId5\" Type=\"http://x/image\" Target=\"media/a.png\"/><Relationship Id=\"rId6\" Type=\"http://x/hyperlink\" Target=\"javascript:alert(1)\" TargetMode=\"External\"/></Relationships>";
        zip_store(&[
            ("word/document.xml", document.as_bytes()),
            ("word/_rels/document.xml.rels", rels.as_bytes()),
            ("word/media/a.png", b"\x89PNG-bytes"),
        ])
    }

    #[test]
    fn a_word_document_keeps_text_styles_headings_and_pictures() {
        let bytes = docx(
            "<w:p><w:pPr><w:outlineLvl w:val=\"0\"/></w:pPr><w:r><w:t>Intro &amp; &lt;b&gt;scope</w:t></w:r></w:p>\
             <w:p><w:r><w:rPr><w:b/><w:color w:val=\"C00000\"/></w:rPr><w:t>red</w:t></w:r><w:r><w:drawing><wp:inline><wp:extent cx=\"952500\" cy=\"476250\"/><a:graphic><a:graphicData><a:blip r:embed=\"rId5\"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>\
             <w:sectPr><w:pgSz w:w=\"11906\"/></w:sectPr>",
        );
        let page = docx_to_html(&bytes).unwrap();
        assert_eq!(page.headings.len(), 1);
        assert_eq!(page.headings[0].text, "Intro & <b>scope");
        assert!(
            page.html.contains("Intro &amp; &lt;b&gt;scope"),
            "text is escaped"
        );
        assert!(page.html.contains("font-weight:700;color:#C00000"));
        assert!(page.html.contains("<img src=\"data:image/png;base64,"));
        assert!(page.html.contains("width:100px"), "extent EMU becomes px");
        assert!(page.html.contains("width:794px"), "A4 page width");
        assert!(page.html.contains("script-src 'none'"));
    }

    #[test]
    fn a_document_cannot_smuggle_links_or_markup() {
        let bytes = docx(
            "<w:p><w:hyperlink r:id=\"rId6\"><w:r><w:t>x</w:t></w:r></w:hyperlink><w:r><w:t>&lt;script&gt;alert(1)&lt;/script&gt;</w:t></w:r></w:p>",
        );
        let page = docx_to_html(&bytes).unwrap();
        assert!(!page.html.contains("javascript:"));
        assert!(!page.html.contains("<script>"));
    }

    #[test]
    fn lists_count_and_tables_merge() {
        let numbering = format!(
            "<w:numbering {W}><w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\"><w:numFmt w:val=\"decimal\"/><w:lvlText w:val=\"%1.\"/></w:lvl></w:abstractNum><w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num></w:numbering>"
        );
        let item = "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr><w:r><w:t>i</w:t></w:r></w:p>";
        let table = "<w:tbl><w:tr><w:tc><w:tcPr><w:vMerge w:val=\"restart\"/></w:tcPr><w:p/></w:tc><w:tc><w:tcPr><w:gridSpan w:val=\"2\"/></w:tcPr><w:p/></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc><w:tc><w:p/></w:tc><w:tc><w:p/></w:tc></w:tr></w:tbl>";
        let document = format!("<w:document {W}><w:body>{item}{item}{table}</w:body></w:document>");
        let bytes = zip_store(&[
            ("word/document.xml", document.as_bytes()),
            ("word/numbering.xml", numbering.as_bytes()),
        ]);
        let html = docx_to_html(&bytes).unwrap().html;
        assert!(html.contains(">1.</span>") && html.contains(">2.</span>"));
        assert!(html.contains("rowspan=\"2\"") && html.contains("colspan=\"2\""));
    }

    #[test]
    fn labels_and_numerals() {
        assert_eq!(roman(2024), "MMXXIV");
        assert_eq!(letters(27, false), "aa");
        assert_eq!(letters(3, true), "C");
    }

    #[test]
    fn colors_follow_the_theme_and_modifiers() {
        let mut theme = Theme::default();
        theme.colors.insert("dk1".into(), "#000000".into());
        theme.colors.insert("accent1".into(), "#4472c4".into());
        theme.map.insert("tx1".into(), "dk1".into());
        let doc = ox::parse("<a:schemeClr xmlns:a='u' val='tx1'/>").unwrap();
        assert_eq!(theme.color(doc.root_element()).as_deref(), Some("#000000"));
        let doc = ox::parse(
            "<a:schemeClr xmlns:a='u' val='accent1'><a:alpha val='50000'/></a:schemeClr>",
        )
        .unwrap();
        assert_eq!(
            theme.color(doc.root_element()).as_deref(),
            Some("rgba(68,114,196,0.50)")
        );
        let doc =
            ox::parse("<a:srgbClr xmlns:a='u' val='808080'><a:lumMod val='50000'/></a:srgbClr>")
                .unwrap();
        assert_eq!(theme.color(doc.root_element()).as_deref(), Some("#404040"));
    }

    #[test]
    fn a_deck_gives_its_slides_in_order_with_inherited_places() {
        let ns = "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"";
        let rel = |items: &[(&str, &str, &str)]| {
            let body: String = items
                .iter()
                .map(|(i, t, g)| {
                    format!("<Relationship Id=\"{i}\" Type=\"http://x/{t}\" Target=\"{g}\"/>")
                })
                .collect();
            format!(
                "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{body}</Relationships>"
            )
        };
        let presentation = format!(
            "<p:presentation {ns}><p:sldIdLst><p:sldId id=\"256\" r:id=\"rId2\"/><p:sldId id=\"257\" r:id=\"rId1\"/></p:sldIdLst><p:sldSz cx=\"9144000\" cy=\"5143500\"/></p:presentation>"
        );
        let slide = |text: &str| {
            format!(
                "<p:sld {ns}><p:cSld><p:spTree><p:sp><p:nvSpPr><p:nvPr><p:ph type=\"title\"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"
            )
        };
        let layout = format!(
            "<p:sldLayout {ns}><p:cSld><p:spTree><p:sp><p:nvSpPr><p:nvPr><p:ph type=\"title\"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x=\"952500\" y=\"952500\"/><a:ext cx=\"1905000\" cy=\"952500\"/></a:xfrm></p:spPr></p:sp></p:spTree></p:cSld></p:sldLayout>"
        );
        let (r1, r2) = (
            rel(&[
                ("rId1", "slide", "slides/slide1.xml"),
                ("rId2", "slide", "slides/slide2.xml"),
            ]),
            rel(&[("rId1", "slideLayout", "../slideLayouts/l.xml")]),
        );
        let (s1, s2) = (slide("First"), slide("Second"));
        let bytes = zip_store(&[
            ("ppt/presentation.xml", presentation.as_bytes()),
            ("ppt/_rels/presentation.xml.rels", r1.as_bytes()),
            ("ppt/slides/slide1.xml", s1.as_bytes()),
            ("ppt/slides/slide2.xml", s2.as_bytes()),
            ("ppt/slides/_rels/slide1.xml.rels", r2.as_bytes()),
            ("ppt/slides/_rels/slide2.xml.rels", r2.as_bytes()),
            ("ppt/slideLayouts/l.xml", layout.as_bytes()),
        ]);
        let deck = pptx_to_html(&bytes).unwrap();
        assert_eq!(deck.slides, 2);
        // The order is the deck's (rId2 first), not the file names'.
        assert!(deck.html.find("Second").unwrap() < deck.html.find("First").unwrap());
        // The title has no place of its own: the layout's (100px, 100px, 200px, 100px) is used.
        assert!(
            deck.html
                .contains("left:100.0px;top:100.0px;width:200.0px;height:100.0px")
        );
        assert!(deck.html.contains("script-src 'nonce-"));
        assert!(pptx_to_html(b"nope").is_err());
    }

    #[test]
    fn web_pages_lose_scripts_and_network() {
        let page = html_attachment(
            "<p>hi</p><SCRIPT>alert(1)</SCRIPT><img src=\"https://x/y.png\"><script>no end",
        );
        assert!(page.starts_with("<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\""));
        assert!(!page.to_lowercase().contains("<script"));
        assert!(page.contains("<p>hi</p>"));
        assert!(page.contains("default-src 'none'"));
    }

    /// The fixtures of /tmp/viewer-fixtures, when they exist: written to `out-*.html` to look at.
    #[test]
    fn fixture_files_convert() {
        let dir = std::path::Path::new("/tmp/viewer-fixtures");
        let Ok(bytes) = std::fs::read(dir.join("sample.docx")) else {
            return;
        };
        let page = docx_to_html(&bytes).unwrap();
        assert!(page.headings.len() >= 3);
        let deck = pptx_to_html(&std::fs::read(dir.join("sample.pptx")).unwrap()).unwrap();
        assert_eq!(deck.slides, 3);
    }
}
