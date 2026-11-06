//! Syntax highlighting for the file view, with tree-sitter. The palette comes from CodeMirror's
//! Lezer parsers and nine color groups (keyword, callable, string, type, tag, number, comment,
//! property, meta; `editorLanguage.ts`); tree-sitter is the Rust counterpart of that approach, a real
//! parser rather than patterns, and its capture names fall into the same groups. The colors of the
//! groups are the `--syntax-*` palette (src/styles/index.css, MIT, see LICENSE-THIRD-PARTY-MIT);
//! they are chosen where the text is drawn, so a light/dark flip needs no new parse.
//!
//! This module has no UI types: `highlight` is blocking, meant for a background thread.
use std::ops::Range;
use std::sync::OnceLock;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// The color groups.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Keyword,
    Callable,
    String,
    Type,
    Tag,
    Number,
    Comment,
    Property,
    Meta,
}

/// One line's colored byte ranges (relative to the line's start), in order.
pub type LineSpans = Vec<(Range<usize>, Group)>;

/// Text beyond this is shown plain: parsing it would cost more than the colors are worth.
const MAX_BYTES: usize = 2_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    Js,
    Jsx,
    Ts,
    Tsx,
    Json,
    Python,
    Css,
    Html,
    Bash,
    Go,
    Yaml,
    Toml,
}

const LANGS: usize = 13;

/// Capture names the highlighter reports, with the group each belongs to. A capture such as
/// `string.special.key` takes the longest name listed that is a prefix of it.
const NAMES: &[(&str, Group)] = &[
    ("keyword", Group::Keyword),
    ("boolean", Group::Keyword),
    ("conditional", Group::Keyword),
    ("repeat", Group::Keyword),
    ("include", Group::Keyword),
    ("storageclass", Group::Keyword),
    ("constant.builtin", Group::Keyword),
    ("variable.builtin", Group::Keyword),
    ("function", Group::Callable),
    ("method", Group::Callable),
    ("function.macro", Group::Callable),
    ("label", Group::Callable),
    ("string", Group::String),
    ("string.special.key", Group::Property),
    ("escape", Group::String),
    ("type", Group::Type),
    ("constructor", Group::Type),
    ("namespace", Group::Type),
    ("module", Group::Type),
    ("tag", Group::Tag),
    ("number", Group::Number),
    ("comment", Group::Comment),
    ("property", Group::Property),
    ("field", Group::Property),
    ("attribute", Group::Meta),
];

fn lang_of(name: &str) -> Option<Lang> {
    let lower = name.to_ascii_lowercase();
    if lower == "dockerfile" || lower.starts_with("dockerfile.") {
        return None;
    }
    if matches!(
        lower.as_str(),
        ".bashrc" | ".bash_profile" | ".zshrc" | ".zprofile" | ".profile"
    ) {
        return Some(Lang::Bash);
    }
    let ext = lower.rsplit_once('.').map(|(_, ext)| ext)?;
    Some(match ext {
        "rs" => Lang::Rust,
        "js" | "mjs" | "cjs" => Lang::Js,
        "jsx" => Lang::Jsx,
        "ts" | "mts" | "cts" => Lang::Ts,
        "tsx" => Lang::Tsx,
        "json" | "jsonc" | "json5" => Lang::Json,
        "py" | "pyi" => Lang::Python,
        "css" => Lang::Css,
        "html" | "htm" | "vue" | "svelte" => Lang::Html,
        "sh" | "bash" | "zsh" => Lang::Bash,
        "go" => Lang::Go,
        "yml" | "yaml" => Lang::Yaml,
        "toml" => Lang::Toml,
        _ => return None,
    })
}

/// True when `name` (a file name) is a language this module can color.
pub fn supports(name: &str) -> bool {
    lang_of(name).is_some()
}

fn build(lang: Lang) -> Option<HighlightConfiguration> {
    macro_rules! config {
        ($language:expr, $name:expr, $query:expr) => {
            HighlightConfiguration::new($language.into(), $name, &$query, "", "").ok()
        };
    }
    let js = tree_sitter_javascript::HIGHLIGHT_QUERY;
    let jsx = tree_sitter_javascript::JSX_HIGHLIGHT_QUERY;
    let mut config = match lang {
        Lang::Rust => config!(
            tree_sitter_rust::LANGUAGE,
            "rust",
            tree_sitter_rust::HIGHLIGHTS_QUERY
        ),
        Lang::Js => config!(
            tree_sitter_javascript::LANGUAGE,
            "javascript",
            format!("{js}\n{jsx}")
        ),
        Lang::Jsx => config!(
            tree_sitter_javascript::LANGUAGE,
            "jsx",
            format!("{js}\n{jsx}")
        ),
        Lang::Ts => config!(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            "typescript",
            format!("{js}\n{}", tree_sitter_typescript::HIGHLIGHTS_QUERY)
        ),
        Lang::Tsx => config!(
            tree_sitter_typescript::LANGUAGE_TSX,
            "tsx",
            format!("{js}\n{jsx}\n{}", tree_sitter_typescript::HIGHLIGHTS_QUERY)
        ),
        Lang::Json => config!(
            tree_sitter_json::LANGUAGE,
            "json",
            tree_sitter_json::HIGHLIGHTS_QUERY
        ),
        Lang::Python => config!(
            tree_sitter_python::LANGUAGE,
            "python",
            tree_sitter_python::HIGHLIGHTS_QUERY
        ),
        Lang::Css => config!(
            tree_sitter_css::LANGUAGE,
            "css",
            tree_sitter_css::HIGHLIGHTS_QUERY
        ),
        Lang::Html => config!(
            tree_sitter_html::LANGUAGE,
            "html",
            tree_sitter_html::HIGHLIGHTS_QUERY
        ),
        Lang::Bash => config!(
            tree_sitter_bash::LANGUAGE,
            "bash",
            tree_sitter_bash::HIGHLIGHT_QUERY
        ),
        Lang::Go => config!(
            tree_sitter_go::LANGUAGE,
            "go",
            tree_sitter_go::HIGHLIGHTS_QUERY
        ),
        Lang::Yaml => config!(
            tree_sitter_yaml::LANGUAGE,
            "yaml",
            tree_sitter_yaml::HIGHLIGHTS_QUERY
        ),
        Lang::Toml => config!(
            tree_sitter_toml_ng::LANGUAGE,
            "toml",
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY
        ),
    }?;
    let names: Vec<&str> = NAMES.iter().map(|(name, _)| *name).collect();
    config.configure(&names);
    Some(config)
}

/// The language's configuration, built on first use (compiling a grammar's queries takes a moment)
/// and kept for the life of the process.
fn config(lang: Lang) -> Option<&'static HighlightConfiguration> {
    static CONFIGS: [OnceLock<Option<HighlightConfiguration>>; LANGS] =
        [const { OnceLock::new() }; LANGS];
    CONFIGS[lang as usize].get_or_init(|| build(lang)).as_ref()
}

/// The colored ranges of `text`, one list per line (lines as `str::lines` splits them), for the file
/// called `name`. None when the language is not known, the text is too big, or parsing fails.
pub fn highlight(name: &str, text: &str) -> Option<Vec<LineSpans>> {
    let lang = lang_of(name)?;
    if text.len() > MAX_BYTES {
        return None;
    }
    let config = config(lang)?;
    // Where each line starts, and where its text ends (before a "\r\n").
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
    let ends: Vec<usize> = starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let end = starts.get(index + 1).map_or(text.len(), |next| next - 1);
            if text[start..end].ends_with('\r') {
                end - 1
            } else {
                end
            }
        })
        .collect();
    let mut lines: Vec<LineSpans> = vec![Vec::new(); starts.len()];

    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config, text.as_bytes(), None, None, |_| None)
        .ok()?;
    let mut stack: Vec<Group> = Vec::new();
    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(highlight) => {
                let (name, group) = NAMES[highlight.0];
                // HTML's attribute names are properties; elsewhere an attribute is metadata.
                stack.push(if lang == Lang::Html && name == "attribute" {
                    Group::Property
                } else {
                    group
                });
            }
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let Some(&group) = stack.last() else { continue };
                let mut at = start;
                while at < end {
                    let line = starts.partition_point(|&line_start| line_start <= at) - 1;
                    let stop = end.min(ends[line]);
                    if at < stop {
                        lines[line].push((at - starts[line]..stop - starts[line], group));
                        at = stop;
                    }
                    // Past this line's text: on to the next line.
                    let next = starts.get(line + 1).copied().unwrap_or(text.len());
                    if at < next {
                        at = next;
                    }
                }
            }
        }
    }
    Some(lines)
}
