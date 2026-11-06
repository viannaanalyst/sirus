//! VS Code theme import (ADR-185). The back end alone talks to Open VSX and opens the packages:
//! the `.vsix` is downloaded over HTTPS from Open VSX hosts only, checked against its SHA-256,
//! inspected (entry count, expanded size, compression ratio) before anything is read, and the
//! theme JSON is reduced to the few colours and token rules Sirus uses. The renderer only ever
//! receives that bounded, validated value. Ported from t3code's Open VSX import (MIT), with the
//! same limits.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::error::{Error, Result};

const OPEN_VSX_API: &str = "https://open-vsx.org/api";
/// Open VSX serves package files from its own CDN host through a redirect.
const TRUSTED_HOSTS: [&str; 2] = ["open-vsx.org", "openvsx.eclipsecontent.org"];
const MAX_REDIRECTS: usize = 3;
const MAX_SEARCH_BYTES: usize = 512 * 1024;
const MAX_DETAIL_BYTES: usize = 256 * 1024;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_THEME_BYTES: usize = 256 * 1024;
const MAX_LOCAL_JSON_BYTES: u64 = 1024 * 1024;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const SEARCH_RESULTS: usize = 8;
const SEARCH_CANDIDATES: usize = 12;
pub const MAX_THEMES_PER_EXTENSION: usize = 40;
pub const MAX_INCLUDE_DEPTH: usize = 8;
const MAX_RESOLVED_FILES: usize = MAX_THEMES_PER_EXTENSION * MAX_INCLUDE_DEPTH;
const MAX_PATH_LENGTH: usize = 1024;

/// Zip hardening limits; tests use smaller ones to reach each branch.
#[derive(Debug, Clone, Copy)]
pub struct ZipLimits {
    pub max_package_bytes: usize,
    pub max_entries: usize,
    pub max_uncompressed: u64,
    pub max_ratio: u64,
}

pub const ZIP_LIMITS: ZipLimits = ZipLimits {
    max_package_bytes: 20 * 1024 * 1024,
    max_entries: 5_000,
    max_uncompressed: 100 * 1024 * 1024,
    max_ratio: 200,
};
/// Entries smaller than this are not judged by ratio: a few KiB of padding is not a bomb.
const RATIO_FLOOR: u64 = 1024;

/// What the stored theme keeps of a licence-checked package.
const SUPPORTED_LICENSES: [&str; 9] = [
    "0BSD",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "CC0-1.0",
    "ISC",
    "MIT",
    "MPL-2.0",
    "Unlicense",
];

/// The workbench colours Sirus maps (the renderer's `vscode-theme.ts` reads exactly these).
pub const COLOR_KEYS: &[&str] = &[
    "foreground",
    "descriptionForeground",
    "disabledForeground",
    "errorForeground",
    "focusBorder",
    "contrastBorder",
    "editor.background",
    "editor.foreground",
    "editor.selectionBackground",
    "editor.lineHighlightBackground",
    "editorCursor.foreground",
    "editorLineNumber.foreground",
    "editorError.foreground",
    "editorWarning.foreground",
    "editorInfo.foreground",
    "editorWidget.background",
    "editorGroup.border",
    "editorGutter.addedBackground",
    "editorGutter.deletedBackground",
    "editorGutter.modifiedBackground",
    "sideBar.background",
    "sideBar.foreground",
    "sideBar.border",
    "activityBar.background",
    "titleBar.activeBackground",
    "panel.background",
    "panel.border",
    "dropdown.background",
    "menu.background",
    "quickInput.background",
    "input.background",
    "input.border",
    "button.background",
    "button.foreground",
    "textLink.foreground",
    "textCodeBlock.background",
    "badge.background",
    "list.hoverBackground",
    "list.activeSelectionBackground",
    "list.inactiveSelectionBackground",
    "charts.blue",
    "charts.green",
    "charts.orange",
    "charts.purple",
    "charts.red",
    "charts.yellow",
    "terminal.background",
    "terminal.foreground",
    "terminal.selectionBackground",
    "terminalCursor.foreground",
    "terminal.ansiBlack",
    "terminal.ansiRed",
    "terminal.ansiGreen",
    "terminal.ansiYellow",
    "terminal.ansiBlue",
    "terminal.ansiMagenta",
    "terminal.ansiCyan",
    "terminal.ansiWhite",
    "terminal.ansiBrightBlack",
    "terminal.ansiBrightRed",
    "terminal.ansiBrightGreen",
    "terminal.ansiBrightYellow",
    "terminal.ansiBrightBlue",
    "terminal.ansiBrightMagenta",
    "terminal.ansiBrightCyan",
    "terminal.ansiBrightWhite",
];

// Stored-theme bounds (checked again by `validate_stored`).
pub const MAX_STORED_THEMES: usize = 48;
const MAX_THEME_NAME: usize = 80;
const MAX_SOURCE: usize = 200;
const MAX_TOKEN_RULES: usize = 400;
const MAX_SCOPES_PER_RULE: usize = 16;
const MAX_SCOPE_LENGTH: usize = 128;
const MAX_STORED_THEME_BYTES: usize = 48 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeKind {
    Light,
    Dark,
}

/// One TextMate rule reduced to what the code highlighter can show. An empty `scope` is the
/// theme's default text rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRule {
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<String>,
    /// Any of `italic`, `bold`, `underline`, `strikethrough`, space separated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_style: Option<String>,
}

/// An installed (or importable) VS Code colour theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VsCodeTheme {
    pub id: String,
    pub name: String,
    pub kind: ThemeKind,
    /// `open-vsx:<namespace>.<name>` or `local:<file name>`.
    pub source: String,
    pub colors: BTreeMap<String, String>,
    #[serde(default)]
    pub token_colors: Vec<TokenRule>,
}

/// An Open VSX search result.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeExtension {
    pub namespace: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub download_count: u64,
    pub version: String,
    pub license: String,
    pub icon_url: Option<String>,
    pub source_url: Option<String>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new("invalid", message)
}

// ---------------------------------------------------------------------------------------------
// Colours and JSONC
// ---------------------------------------------------------------------------------------------

/// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` as lowercase `#rrggbb` / `#rrggbbaa`.
pub fn normalize_color(value: &str) -> Option<String> {
    let digits = value.trim().strip_prefix('#')?;
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        3 | 4 => Some(format!(
            "#{}",
            digits
                .chars()
                .flat_map(|c| [c, c])
                .collect::<String>()
                .to_ascii_lowercase()
        )),
        6 | 8 => Some(format!("#{}", digits.to_ascii_lowercase())),
        _ => None,
    }
}

fn relative_luminance(color: &str) -> f64 {
    let channel = |start: usize| {
        let value = u8::from_str_radix(&color[start..start + 2], 16).unwrap_or(0) as f64 / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
}

/// Removes a BOM, `//` and `/* */` comments and trailing commas so serde_json can read the
/// JSONC VS Code themes use. Strings are left alone.
pub fn strip_jsonc(source: &str) -> String {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let chars: Vec<char> = source.chars().collect();
    let mut without_comments = String::with_capacity(source.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c == '"' {
            without_comments.push(c);
            index += 1;
            while index < chars.len() {
                let s = chars[index];
                without_comments.push(s);
                index += 1;
                if s == '\\' && index < chars.len() {
                    without_comments.push(chars[index]);
                    index += 1;
                } else if s == '"' {
                    break;
                }
            }
        } else if c == '/' && chars.get(index + 1) == Some(&'/') {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
        } else if c == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index < chars.len()
                && !(chars[index] == '*' && chars.get(index + 1) == Some(&'/'))
            {
                index += 1;
            }
            index = (index + 2).min(chars.len());
            without_comments.push(' ');
        } else {
            without_comments.push(c);
            index += 1;
        }
    }
    let chars: Vec<char> = without_comments.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c == '"' {
            out.push(c);
            index += 1;
            while index < chars.len() {
                let s = chars[index];
                out.push(s);
                index += 1;
                if s == '\\' && index < chars.len() {
                    out.push(chars[index]);
                    index += 1;
                } else if s == '"' {
                    break;
                }
            }
        } else if c == ',' {
            let mut ahead = index + 1;
            while ahead < chars.len() && chars[ahead].is_whitespace() {
                ahead += 1;
            }
            if !matches!(chars.get(ahead), Some('}') | Some(']')) {
                out.push(c);
            }
            index += 1;
        } else {
            out.push(c);
            index += 1;
        }
    }
    out
}

pub fn parse_jsonc_object(source: &str, what: &str) -> Result<serde_json::Map<String, Value>> {
    match serde_json::from_str::<Value>(&strip_jsonc(source)) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(invalid(format!("{what} is not valid JSON."))),
    }
}

// ---------------------------------------------------------------------------------------------
// Package paths and zip inspection
// ---------------------------------------------------------------------------------------------

/// Joins `input` onto `from_dir` (a folder inside the package, no trailing slash), refusing
/// absolute paths and any `..` that would climb above the first `protected` segments.
pub fn join_package_path(from_dir: &str, input: &str, protected: usize) -> Result<String> {
    if input.len() > MAX_PATH_LENGTH
        || input.contains('\0')
        || input.starts_with('/')
        || input.starts_with('\\')
        || input.as_bytes().get(1) == Some(&b':')
    {
        return Err(invalid("Theme path is not a safe relative package path."));
    }
    let normalized = input.replace('\\', "/");
    let mut segments: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    for segment in normalized.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.len() <= protected {
                    return Err(invalid("Theme path escapes the extension package."));
                }
                segments.pop();
            }
            other => segments.push(if other.contains('\0') { "_" } else { other }),
        }
    }
    Ok(segments.join("/"))
}

fn check_entry_name(name: &str) -> Result<()> {
    if name.len() > MAX_PATH_LENGTH
        || name.contains('\0')
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.as_bytes().get(1) == Some(&b':')
        || name.replace('\\', "/").split('/').any(|part| part == "..")
    {
        return Err(invalid("That extension package has an unsafe file path."));
    }
    Ok(())
}

pub type Package<'a> = ZipArchive<Cursor<&'a [u8]>>;

/// Opens a package after checking its size, entry count, expanded size and compression ratios
/// from the central directory, before any entry is read.
pub fn open_package<'a>(bytes: &'a [u8], limits: ZipLimits) -> Result<Package<'a>> {
    if bytes.len() > limits.max_package_bytes {
        return Err(invalid(
            "That theme extension is too large to import safely.",
        ));
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| invalid("That extension package could not be opened."))?;
    if archive.len() > limits.max_entries {
        return Err(invalid("That extension package has too many files."));
    }
    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .map_err(|_| invalid("That extension package has an invalid ZIP directory."))?;
        check_entry_name(entry.name())?;
        let (size, packed) = (entry.size(), entry.compressed_size());
        total = total.saturating_add(size);
        if total > limits.max_uncompressed {
            return Err(invalid(
                "That extension package expands beyond the safe import limit.",
            ));
        }
        if size >= RATIO_FLOOR && (packed == 0 || size / packed > limits.max_ratio) {
            return Err(invalid(
                "That extension package has an unsafe compression ratio.",
            ));
        }
    }
    Ok(archive)
}

/// Reads one entry as UTF-8 text, never more than `limit` bytes whatever the headers claim.
fn read_entry(archive: &mut Package<'_>, path: &str, limit: usize, what: &str) -> Result<String> {
    let entry = archive
        .by_name(path)
        .map_err(|_| invalid(format!("{what} is missing from the extension package.")))?;
    if entry.size() > limit as u64 {
        return Err(invalid(format!("{what} is too large.")));
    }
    let mut bytes = Vec::new();
    entry
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid(format!("{what} could not be read.")))?;
    if bytes.len() > limit {
        return Err(invalid(format!("{what} is too large.")));
    }
    String::from_utf8(bytes).map_err(|_| invalid(format!("{what} is not valid text.")))
}

// ---------------------------------------------------------------------------------------------
// Theme files, includes and sanitizing
// ---------------------------------------------------------------------------------------------

/// Where theme JSON files come from: a package, or the folder of a local file.
pub trait ThemeFiles {
    fn read(&mut self, path: &str) -> Result<String>;
}

struct PackageFiles<'a, 'b>(&'b mut Package<'a>);

impl ThemeFiles for PackageFiles<'_, '_> {
    fn read(&mut self, path: &str) -> Result<String> {
        read_entry(self.0, path, MAX_THEME_BYTES, path)
    }
}

struct FolderFiles(std::path::PathBuf);

impl ThemeFiles for FolderFiles {
    fn read(&mut self, path: &str) -> Result<String> {
        let root = self
            .0
            .canonicalize()
            .map_err(|_| invalid("The theme folder is not readable."))?;
        let file = root
            .join(path)
            .canonicalize()
            .map_err(|_| invalid(format!("{path} is missing.")))?;
        if !file.starts_with(&root) {
            return Err(invalid("Theme path escapes its folder."));
        }
        let meta = std::fs::metadata(&file).map_err(|_| invalid(format!("{path} is missing.")))?;
        if !meta.is_file() || meta.len() > MAX_THEME_BYTES as u64 {
            return Err(invalid(format!("{path} is too large or not a file.")));
        }
        std::fs::read_to_string(&file).map_err(|_| invalid(format!("{path} is not valid text.")))
    }
}

/// A theme file reduced to what Sirus maps.
#[derive(Debug, Clone, Default)]
pub struct RawTheme {
    pub colors: BTreeMap<String, String>,
    pub token_colors: Vec<TokenRule>,
    pub kind_hint: Option<String>,
    pub name: Option<String>,
}

fn sanitize_font_style(value: &str) -> Option<String> {
    let mut words: Vec<&str> = Vec::new();
    for word in value.split_whitespace() {
        if ["italic", "bold", "underline", "strikethrough"].contains(&word)
            && !words.contains(&word)
        {
            words.push(word);
        }
    }
    (!words.is_empty()).then(|| words.join(" "))
}

fn sanitize_scope(value: &str) -> Option<String> {
    let scope = value.trim();
    (!scope.is_empty()
        && scope.len() <= MAX_SCOPE_LENGTH
        && scope
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-+*#$@:/ ".contains(c)))
    .then(|| scope.to_string())
}

fn sanitize_token_colors(value: Option<&Value>) -> Vec<TokenRule> {
    let Some(Value::Array(rules)) = value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for rule in rules {
        if out.len() >= MAX_TOKEN_RULES {
            break;
        }
        let Some(rule) = rule.as_object() else {
            continue;
        };
        let scopes: Vec<String> = match rule.get("scope") {
            Some(Value::String(text)) => text.split(',').filter_map(sanitize_scope).collect(),
            Some(Value::Array(list)) => list
                .iter()
                .filter_map(Value::as_str)
                .flat_map(|text| text.split(','))
                .filter_map(sanitize_scope)
                .collect(),
            _ => Vec::new(),
        };
        let scopes: Vec<String> = scopes.into_iter().take(MAX_SCOPES_PER_RULE).collect();
        let settings = rule.get("settings").and_then(Value::as_object);
        let foreground = settings
            .and_then(|s| s.get("foreground"))
            .and_then(Value::as_str)
            .and_then(normalize_color);
        let font_style = settings
            .and_then(|s| s.get("fontStyle"))
            .and_then(Value::as_str)
            .and_then(sanitize_font_style);
        let had_scope = rule.get("scope").is_some();
        if (had_scope && scopes.is_empty()) || (foreground.is_none() && font_style.is_none()) {
            continue;
        }
        out.push(TokenRule {
            scope: scopes,
            foreground,
            font_style,
        });
    }
    out
}

fn sanitize_theme(value: &serde_json::Map<String, Value>) -> RawTheme {
    let mut colors = BTreeMap::new();
    if let Some(Value::Object(map)) = value.get("colors") {
        for key in COLOR_KEYS {
            if let Some(color) = map
                .get(*key)
                .and_then(Value::as_str)
                .and_then(normalize_color)
            {
                colors.insert((*key).to_string(), color);
            }
        }
    }
    RawTheme {
        colors,
        token_colors: sanitize_token_colors(value.get("tokenColors")),
        kind_hint: value
            .get("type")
            .and_then(Value::as_str)
            .map(|s| s.to_ascii_lowercase()),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    }
}

/// Loads a theme file and everything it `include`s (depth 8 at most, no cycles, a bounded
/// number of files); an including file's colours and rules override the included ones.
pub fn load_theme(
    files: &mut dyn ThemeFiles,
    path: &str,
    protected: usize,
    cache: &mut BTreeMap<String, RawTheme>,
    budget: &mut usize,
    ancestors: &mut Vec<String>,
) -> Result<RawTheme> {
    if ancestors.len() >= MAX_INCLUDE_DEPTH {
        return Err(invalid("Theme includes are nested too deeply."));
    }
    if ancestors.iter().any(|ancestor| ancestor == path) {
        return Err(invalid("Theme includes contain a cycle."));
    }
    if let Some(known) = cache.get(path) {
        return Ok(known.clone());
    }
    *budget += 1;
    if *budget > MAX_RESOLVED_FILES {
        return Err(invalid("That extension references too many theme files."));
    }
    let object = parse_jsonc_object(&files.read(path)?, path)?;
    let own = sanitize_theme(&object);
    let resolved = match object.get("include").and_then(Value::as_str) {
        None => own,
        Some(include) => {
            let directory = path.rsplit_once('/').map_or("", |(dir, _)| dir);
            let include_path = join_package_path(directory, include, protected)?;
            ancestors.push(path.to_string());
            let base = load_theme(files, &include_path, protected, cache, budget, ancestors);
            ancestors.pop();
            let base = base?;
            let mut colors = base.colors;
            colors.extend(own.colors);
            let mut token_colors = base.token_colors;
            token_colors.extend(own.token_colors);
            token_colors.truncate(MAX_TOKEN_RULES);
            RawTheme {
                colors,
                token_colors,
                kind_hint: own.kind_hint.or(base.kind_hint),
                name: own.name.or(base.name),
            }
        }
    };
    cache.insert(path.to_string(), resolved.clone());
    Ok(resolved)
}

fn short_hash(value: &str) -> String {
    Sha256::digest(value.as_bytes())[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn kind_of(
    ui_theme: Option<&str>,
    hint: Option<&str>,
    colors: &BTreeMap<String, String>,
) -> ThemeKind {
    match ui_theme {
        Some("vs") | Some("hc-light") => return ThemeKind::Light,
        Some("vs-dark") | Some("hc-black") => return ThemeKind::Dark,
        _ => {}
    }
    match hint {
        Some("light") | Some("hc-light") => return ThemeKind::Light,
        Some("dark") | Some("hc-black") | Some("hc") => return ThemeKind::Dark,
        _ => {}
    }
    match colors.get("editor.background") {
        Some(background) if relative_luminance(background) > 0.4 => ThemeKind::Light,
        _ => ThemeKind::Dark,
    }
}

fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_THEME_NAME)
        .collect()
}

/// Builds the stored theme for one contribution.
pub fn build_theme(
    source: &str,
    path: &str,
    label: &str,
    ui_theme: Option<&str>,
    raw: RawTheme,
) -> Result<VsCodeTheme> {
    if raw.colors.is_empty() && raw.token_colors.is_empty() {
        return Err(invalid("That file is not a VS Code colour theme."));
    }
    let name = clean_name(if label.trim().is_empty() {
        raw.name.as_deref().unwrap_or("Imported theme")
    } else {
        label
    });
    let theme = VsCodeTheme {
        id: format!("vsc-{}", short_hash(&format!("{source}\0{path}\0{name}"))),
        kind: kind_of(ui_theme, raw.kind_hint.as_deref(), &raw.colors),
        name: if name.is_empty() {
            "Imported theme".into()
        } else {
            name
        },
        source: source.chars().take(MAX_SOURCE).collect(),
        colors: raw.colors,
        token_colors: raw.token_colors,
    };
    check_theme(&theme)?;
    Ok(theme)
}

/// Gives themes sharing a name a numeric suffix (and so distinct ids).
fn separate_names(themes: &mut [VsCodeTheme]) {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for theme in themes.iter_mut() {
        let count = seen.entry(theme.name.to_lowercase()).or_insert(0);
        *count += 1;
        if *count > 1 {
            theme.name = clean_name(&format!("{} ({})", theme.name, count));
            theme.id = format!(
                "vsc-{}",
                short_hash(&format!("{}\0{}", theme.id, theme.name))
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Packages
// ---------------------------------------------------------------------------------------------

fn manifest_license(manifest: &serde_json::Map<String, Value>) -> Option<String> {
    manifest
        .get("license")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
}

/// What a downloaded package must match: the Open VSX entry the user chose.
pub struct Expected {
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub license: String,
}

/// Reads every colour theme an extension package contributes.
pub fn themes_from_package(
    bytes: &[u8],
    source: &str,
    expected: Option<&Expected>,
    limits: ZipLimits,
) -> Result<Vec<VsCodeTheme>> {
    let mut package = open_package(bytes, limits)?;
    let manifest = parse_jsonc_object(
        &read_entry(
            &mut package,
            "extension/package.json",
            MAX_MANIFEST_BYTES,
            "Extension manifest",
        )?,
        "Extension manifest",
    )?;
    if let Some(expected) = expected {
        let text = |key: &str| manifest.get(key).and_then(Value::as_str).unwrap_or("");
        if !text("publisher").eq_ignore_ascii_case(&expected.namespace)
            || !text("name").eq_ignore_ascii_case(&expected.name)
            || text("version") != expected.version
        {
            return Err(invalid(
                "That extension package does not match the selected Open VSX theme.",
            ));
        }
        if !manifest_license(&manifest).is_some_and(|l| l.eq_ignore_ascii_case(&expected.license)) {
            return Err(invalid(
                "That extension package does not match its advertised license.",
            ));
        }
    }
    let contributions: Vec<&serde_json::Map<String, Value>> = manifest
        .get("contributes")
        .and_then(|c| c.get("themes"))
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();
    if contributions.is_empty() {
        return Err(invalid("That extension does not contain color themes."));
    }
    if contributions.len() > MAX_THEMES_PER_EXTENSION {
        return Err(invalid(
            "That extension contains too many color themes to import safely.",
        ));
    }
    let mut files = PackageFiles(&mut package);
    let mut cache = BTreeMap::new();
    let mut budget = 0;
    let mut themes = Vec::new();
    for contribution in contributions {
        let path = contribution
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("A theme in that extension has no path."))?;
        let path = join_package_path("extension", path, 1)?;
        let raw = load_theme(
            &mut files,
            &path,
            1,
            &mut cache,
            &mut budget,
            &mut Vec::new(),
        )
        .map_err(|_| {
            invalid("One or more color themes in that extension could not be imported safely.")
        })?;
        let label = contribution
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("");
        let ui = contribution.get("uiTheme").and_then(Value::as_str);
        themes.push(build_theme(source, &path, label, ui, raw).map_err(|_| {
            invalid("One or more color themes in that extension could not be imported safely.")
        })?);
    }
    separate_names(&mut themes);
    Ok(themes)
}

/// A single theme file picked from disk; its includes resolve inside its own folder.
pub fn theme_from_file(path: &std::path::Path) -> Result<Vec<VsCodeTheme>> {
    let meta = std::fs::metadata(path).map_err(|_| invalid("Could not read that file."))?;
    if meta.len() > MAX_LOCAL_JSON_BYTES.max(ZIP_LIMITS.max_package_bytes as u64) {
        return Err(invalid("That file is too large to import."));
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| invalid("Could not read that file."))?
        .to_string();
    let source = format!("local:{}", file_name.chars().take(120).collect::<String>());
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("vsix") => {
            let bytes = std::fs::read(path).map_err(|_| invalid("Could not read that file."))?;
            themes_from_package(&bytes, &source, None, ZIP_LIMITS)
        }
        Some("json") | Some("jsonc") => {
            if meta.len() > MAX_LOCAL_JSON_BYTES {
                return Err(invalid("That theme file is too large."));
            }
            let folder = path.parent().unwrap_or(std::path::Path::new("."));
            let mut files = FolderFiles(folder.to_path_buf());
            let raw = load_theme(
                &mut files,
                &file_name,
                0,
                &mut BTreeMap::new(),
                &mut 0,
                &mut Vec::new(),
            )?;
            let label = raw.name.clone().unwrap_or_else(|| {
                file_name
                    .rsplit_once('.')
                    .map_or(file_name.clone(), |(stem, _)| stem.to_string())
            });
            Ok(vec![build_theme(&source, &file_name, &label, None, raw)?])
        }
        _ => Err(invalid("Choose a .vsix extension or a .json theme file.")),
    }
}

// ---------------------------------------------------------------------------------------------
// Storage validation
// ---------------------------------------------------------------------------------------------

fn check_theme(theme: &VsCodeTheme) -> Result<()> {
    let bad = |what: &str| {
        Err(Error::new(
            "invalid_settings",
            format!("Invalid VS Code theme: {what}."),
        ))
    };
    let id_ok = theme.id.strip_prefix("vsc-").is_some_and(|rest| {
        rest.len() == 16 && rest.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
    });
    if !id_ok {
        return bad("id");
    }
    if theme.name.trim().is_empty()
        || theme.name.chars().count() > MAX_THEME_NAME
        || theme.name.chars().any(char::is_control)
    {
        return bad("name");
    }
    if theme.source.chars().count() > MAX_SOURCE || theme.source.chars().any(char::is_control) {
        return bad("source");
    }
    if theme.colors.len() > COLOR_KEYS.len()
        || theme.colors.iter().any(|(key, value)| {
            !COLOR_KEYS.contains(&key.as_str()) || normalize_color(value).as_deref() != Some(value)
        })
    {
        return bad("colors");
    }
    if theme.token_colors.len() > MAX_TOKEN_RULES {
        return bad("token rules");
    }
    for rule in &theme.token_colors {
        let scope_ok = rule.scope.len() <= MAX_SCOPES_PER_RULE
            && rule
                .scope
                .iter()
                .all(|scope| sanitize_scope(scope).as_deref() == Some(scope));
        let color_ok = rule
            .foreground
            .as_deref()
            .is_none_or(|c| normalize_color(c).as_deref() == Some(c));
        let style_ok = rule
            .font_style
            .as_deref()
            .is_none_or(|s| sanitize_font_style(s).as_deref() == Some(s));
        if !scope_ok || !color_ok || !style_ok {
            return bad("token rule");
        }
    }
    if serde_json::to_vec(theme).map_or(true, |bytes| bytes.len() > MAX_STORED_THEME_BYTES) {
        return bad("size");
    }
    Ok(())
}

/// Settings validation: bounded list, unique ids, and active themes that exist with the slot's kind.
pub fn validate_stored(
    themes: &[VsCodeTheme],
    light: Option<&str>,
    dark: Option<&str>,
) -> Result<()> {
    if themes.len() > MAX_STORED_THEMES {
        return Err(Error::new(
            "invalid_settings",
            "Too many VS Code themes are installed.",
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for theme in themes {
        check_theme(theme)?;
        if !ids.insert(theme.id.as_str()) {
            return Err(Error::new(
                "invalid_settings",
                "Invalid VS Code theme: duplicate id.",
            ));
        }
    }
    for (active, kind) in [(light, ThemeKind::Light), (dark, ThemeKind::Dark)] {
        if let Some(id) = active {
            if !themes.iter().any(|t| t.id == id && t.kind == kind) {
                return Err(Error::new(
                    "invalid_settings",
                    "The chosen VS Code theme is not installed.",
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Open VSX
// ---------------------------------------------------------------------------------------------

/// HTTPS on an Open VSX host, no credentials in the URL.
pub fn trusted_url(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none_or(|port| port == 443)
        && url
            .host_str()
            .is_some_and(|host| TRUSTED_HOSTS.contains(&host.to_ascii_lowercase().as_str()))
}

fn trusted_text(value: Option<&Value>) -> Option<String> {
    let url = reqwest::Url::parse(value?.as_str()?).ok()?;
    trusted_url(&url).then(|| url.to_string())
}

fn public_source_url(value: Option<&Value>) -> Option<String> {
    let raw = match value? {
        Value::String(text) => text.as_str(),
        Value::Object(map) => map.get("url")?.as_str()?,
        _ => return None,
    };
    let url = reqwest::Url::parse(raw).ok()?;
    (url.scheme() == "https" && url.username().is_empty() && url.password().is_none())
        .then(|| url.to_string())
}

fn client() -> Result<&'static reqwest::Client> {
    static HTTP: OnceLock<std::result::Result<reqwest::Client, ()>> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(DOWNLOAD_TIMEOUT)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if trusted_url(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .retry(reqwest::retry::never())
            .user_agent("sirus-code")
            .build()
            .map_err(|_| ())
    })
    .as_ref()
    .map_err(|_| Error::new("native", "Cannot start the theme download."))
}

async fn get_capped(
    url: &str,
    limit: usize,
    timeout: Duration,
    too_large: &str,
) -> Result<Vec<u8>> {
    let parsed = reqwest::Url::parse(url).map_err(|_| invalid("Not an Open VSX address."))?;
    if !trusted_url(&parsed) {
        return Err(invalid("Themes can only be downloaded from open-vsx.org."));
    }
    let mut response = client()?
        .get(parsed)
        .timeout(timeout)
        .send()
        .await
        .map_err(|_| Error::new("network", "Open VSX could not be reached."))?;
    if !response.status().is_success() {
        return Err(Error::new("network", "Open VSX is unavailable right now."));
    }
    if response
        .content_length()
        .is_some_and(|len| len > limit as u64)
    {
        return Err(invalid(too_large));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::new("network", "Open VSX stopped responding."))?
    {
        if body.len() + chunk.len() > limit {
            return Err(invalid(too_large));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn identifier_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

struct Resolved {
    extension: ThemeExtension,
    vsix_url: String,
    sha256_url: String,
}

/// The Open VSX detail document of an extension, or none when its licence is not accepted or
/// its package is too large.
fn resolve_detail(value: &Value) -> Result<Option<Resolved>> {
    let malformed = || Error::new("network", "Open VSX returned malformed theme details.");
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let files = value.get("files").ok_or_else(malformed)?;
    let (namespace, name, version, license) = (
        text("namespace"),
        text("name"),
        text("version"),
        text("license"),
    );
    let vsix_url = trusted_text(files.get("download")).ok_or_else(malformed)?;
    let sha256_url = trusted_text(files.get("sha256")).ok_or_else(malformed)?;
    if !identifier_ok(&namespace)
        || !identifier_ok(&name)
        || version.is_empty()
        || version.len() > 64
    {
        return Err(malformed());
    }
    if !SUPPORTED_LICENSES.contains(&license.as_str()) {
        return Ok(None);
    }
    if value
        .get("downloadSizes")
        .and_then(|sizes| sizes.get("universal"))
        .and_then(Value::as_u64)
        .is_some_and(|size| size > ZIP_LIMITS.max_package_bytes as u64)
    {
        return Ok(None);
    }
    let display = text("displayName");
    Ok(Some(Resolved {
        extension: ThemeExtension {
            display_name: if display.is_empty() {
                name.clone()
            } else {
                display
            },
            description: text("description").chars().take(300).collect(),
            download_count: value
                .get("downloadCount")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            icon_url: trusted_text(files.get("icon")),
            source_url: public_source_url(value.get("repository"))
                .or_else(|| public_source_url(value.get("homepage")))
                .or_else(|| public_source_url(value.get("url"))),
            namespace,
            name,
            version,
            license,
        },
        vsix_url,
        sha256_url,
    }))
}

async fn fetch_detail(namespace: &str, name: &str) -> Result<Option<Resolved>> {
    if !identifier_ok(namespace) || !identifier_ok(name) {
        return Err(invalid("Not a valid extension name."));
    }
    let body = get_capped(
        &format!("{OPEN_VSX_API}/{namespace}/{name}"),
        MAX_DETAIL_BYTES,
        SEARCH_TIMEOUT,
        "Open VSX returned an unexpectedly large detail response.",
    )
    .await?;
    let value: Value = serde_json::from_slice(&body)
        .map_err(|_| Error::new("network", "Open VSX returned an unreadable response."))?;
    resolve_detail(&value)
}

const SORTS: [&str; 4] = ["downloadCount", "rating", "timestamp", "relevance"];

pub async fn vscode_themes_search(
    query: String,
    sort: Option<String>,
) -> Result<Vec<ThemeExtension>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if query.chars().count() > 100 {
        return Err(invalid("That search is too long."));
    }
    let sort = sort.unwrap_or_else(|| "downloadCount".into());
    if !SORTS.contains(&sort.as_str()) {
        return Err(invalid("Unknown sort order."));
    }
    let mut url = reqwest::Url::parse(&format!("{OPEN_VSX_API}/-/search"))
        .map_err(|_| Error::new("native", "Cannot build the search address."))?;
    url.query_pairs_mut()
        .append_pair("query", query)
        .append_pair("category", "Themes")
        .append_pair("sortBy", &sort)
        .append_pair("sortOrder", "desc")
        .append_pair("size", &SEARCH_CANDIDATES.to_string());
    let body = get_capped(
        url.as_str(),
        MAX_SEARCH_BYTES,
        SEARCH_TIMEOUT,
        "Open VSX returned an unexpectedly large response.",
    )
    .await?;
    let value: Value = serde_json::from_slice(&body)
        .map_err(|_| Error::new("network", "Open VSX returned an unreadable response."))?;
    let identities: Vec<(String, String)> = value
        .get("extensions")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::new(
                "network",
                "Open VSX returned an unreadable search response.",
            )
        })?
        .iter()
        .filter_map(|item| {
            let namespace = item.get("namespace")?.as_str()?.to_string();
            let name = item.get("name")?.as_str()?.to_string();
            (identifier_ok(&namespace) && identifier_ok(&name)).then_some((namespace, name))
        })
        .take(SEARCH_CANDIDATES)
        .collect();
    let mut tasks = Vec::new();
    for (namespace, name) in &identities {
        let (namespace, name) = (namespace.clone(), name.clone());
        tasks.push(crate::runtime::async_runtime::spawn(async move {
            fetch_detail(&namespace, &name).await
        }));
    }
    let mut found = Vec::new();
    let mut completed = 0;
    for task in tasks {
        if let Ok(Ok(detail)) = task.await {
            completed += 1;
            if let Some(resolved) = detail {
                found.push(resolved.extension);
            }
        }
    }
    if !identities.is_empty() && completed == 0 {
        return Err(Error::new(
            "network",
            "Open VSX theme details are unavailable right now.",
        ));
    }
    found.truncate(SEARCH_RESULTS);
    Ok(found)
}

pub async fn vscode_themes_fetch(namespace: String, name: String) -> Result<Vec<VsCodeTheme>> {
    let Some(resolved) = fetch_detail(&namespace, &name).await? else {
        return Err(invalid(
            "That theme's license is not one Sirus Code can import, or it is too large.",
        ));
    };
    let package = get_capped(
        &resolved.vsix_url,
        ZIP_LIMITS.max_package_bytes,
        DOWNLOAD_TIMEOUT,
        "That theme extension is too large to import safely.",
    )
    .await?;
    let checksum = get_capped(
        &resolved.sha256_url,
        256,
        SEARCH_TIMEOUT,
        "That Open VSX checksum response is invalid.",
    )
    .await?;
    let expected_hash = String::from_utf8_lossy(&checksum)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if expected_hash.len() != 64 || !expected_hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid("That Open VSX theme has an invalid checksum."));
    }
    let actual: String = Sha256::digest(&package)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected_hash {
        return Err(invalid("That Open VSX theme failed its integrity check."));
    }
    let extension = resolved.extension;
    let expected = Expected {
        namespace: extension.namespace.clone(),
        name: extension.name.clone(),
        version: extension.version.clone(),
        license: extension.license.clone(),
    };
    let source = format!("open-vsx:{}.{}", extension.namespace, extension.name);
    crate::commands::native_task(move || {
        themes_from_package(&package, &source, Some(&expected), ZIP_LIMITS)
    })
    .await
}

/// Opens the native picker for a `.vsix` or `.json` theme; none when cancelled.
pub async fn vscode_themes_import_local(
    app: crate::runtime::AppHandle,
) -> Result<Option<Vec<VsCodeTheme>>> {
    let Some(path) = pick_theme_file(&app).await else {
        return Ok(None);
    };
    crate::commands::native_task(move || theme_from_file(&path).map(Some)).await
}

/// The headless host has no screen to show a picker on (ADR-131).
async fn pick_theme_file(_app: &crate::runtime::AppHandle) -> Option<std::path::PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    fn build_zip(entries: &[(&str, Vec<u8>)], method: CompressionMethod) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default().compression_method(method),
                )
                .unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn stored(entries: &[(&str, &str)]) -> Vec<u8> {
        let owned: Vec<(&str, Vec<u8>)> = entries
            .iter()
            .map(|(n, d)| (*n, d.as_bytes().to_vec()))
            .collect();
        build_zip(&owned, CompressionMethod::Stored)
    }

    fn message(result: Result<impl std::fmt::Debug>) -> String {
        result.unwrap_err().to_string()
    }

    // --- zip hardening -------------------------------------------------------------------

    #[test]
    fn rejects_packages_over_the_size_limit() {
        let limits = ZipLimits {
            max_package_bytes: 100,
            ..ZIP_LIMITS
        };
        let bytes = build_zip(&[("a.txt", vec![b'x'; 500])], CompressionMethod::Stored);
        assert!(message(open_package(&bytes, limits)).contains("too large"));
    }

    #[test]
    fn rejects_more_than_5000_entries() {
        let names: Vec<String> = (0..5001).map(|i| format!("f{i}")).collect();
        let entries: Vec<(&str, Vec<u8>)> =
            names.iter().map(|n| (n.as_str(), Vec::new())).collect();
        let bytes = build_zip(&entries, CompressionMethod::Stored);
        assert!(message(open_package(&bytes, ZIP_LIMITS)).contains("too many files"));
        let fine: Vec<(&str, Vec<u8>)> = entries.into_iter().take(5000).collect();
        assert!(open_package(&build_zip(&fine, CompressionMethod::Stored), ZIP_LIMITS).is_ok());
    }

    #[test]
    fn rejects_expansion_beyond_the_total_limit() {
        let limits = ZipLimits {
            max_uncompressed: 3000,
            ..ZIP_LIMITS
        };
        let bytes = build_zip(
            &[("a", vec![1u8; 2000]), ("b", vec![2u8; 2000])],
            CompressionMethod::Stored,
        );
        assert!(message(open_package(&bytes, limits)).contains("expands beyond"));
    }

    #[test]
    fn rejects_unsafe_compression_ratios() {
        let bytes = build_zip(
            &[("bomb.json", vec![0u8; 2 * 1024 * 1024])],
            CompressionMethod::Deflated,
        );
        assert!(bytes.len() < 20_000);
        assert!(message(open_package(&bytes, ZIP_LIMITS)).contains("compression ratio"));
    }

    #[test]
    fn accepts_ordinary_deflated_json() {
        let text = "{\"colors\": {\"editor.background\": \"#101010\"}}".repeat(20);
        let bytes = build_zip(
            &[("t.json", text.into_bytes())],
            CompressionMethod::Deflated,
        );
        assert!(open_package(&bytes, ZIP_LIMITS).is_ok());
    }

    #[test]
    fn rejects_entries_that_climb_out_of_the_package() {
        let bytes = stored(&[("extension/../../evil.json", "{}")]);
        assert!(message(open_package(&bytes, ZIP_LIMITS)).contains("unsafe file path"));
        let bytes = stored(&[("/etc/passwd", "x")]);
        assert!(open_package(&bytes, ZIP_LIMITS).is_err());
    }

    #[test]
    fn reads_never_exceed_the_entry_limit() {
        let bytes = stored(&[("big.json", &"a".repeat(MAX_THEME_BYTES + 10))]);
        let mut package = open_package(&bytes, ZIP_LIMITS).unwrap();
        assert!(message(read_entry(
            &mut package,
            "big.json",
            MAX_THEME_BYTES,
            "big.json"
        ))
        .contains("too large"));
    }

    // --- paths and JSONC -------------------------------------------------------------------

    #[test]
    fn joins_paths_inside_the_package_only() {
        assert_eq!(
            join_package_path("extension", "./themes/a.json", 1).unwrap(),
            "extension/themes/a.json"
        );
        assert_eq!(
            join_package_path("extension/themes", "../base.json", 1).unwrap(),
            "extension/base.json"
        );
        assert!(join_package_path("extension", "../x.json", 1).is_err());
        assert!(join_package_path("extension/themes", "../../x.json", 1).is_err());
        assert!(join_package_path("extension", "/etc/passwd", 1).is_err());
        assert!(join_package_path("extension", "C:/x.json", 1).is_err());
        assert!(join_package_path("extension", "a\\..\\..\\b.json", 1).is_err());
    }

    #[test]
    fn strips_comments_and_trailing_commas_but_not_strings() {
        let source = "\u{feff}{ // line\n \"a\": \"http://x/*y*/\", /* block */ \"b\": [1, 2,], }";
        let value: Value = serde_json::from_str(&strip_jsonc(source)).unwrap();
        assert_eq!(value["a"], "http://x/*y*/");
        assert_eq!(value["b"], serde_json::json!([1, 2]));
    }

    #[test]
    fn normalizes_hex_colors_only() {
        assert_eq!(normalize_color("#ABC").as_deref(), Some("#aabbcc"));
        assert_eq!(normalize_color("#AbCd").as_deref(), Some("#aabbccdd"));
        assert_eq!(normalize_color(" #112233 ").as_deref(), Some("#112233"));
        assert_eq!(normalize_color("#11223344").as_deref(), Some("#11223344"));
        assert!(normalize_color("red").is_none());
        assert!(normalize_color("#12345").is_none());
        assert!(normalize_color("#gg0000").is_none());
        assert!(normalize_color("url(x)").is_none());
    }

    // --- include resolution ------------------------------------------------------------------

    fn theme_package(files: &[(&str, &str)], contributes: &str) -> Vec<u8> {
        let manifest = format!(
            "{{\"publisher\":\"acme\",\"name\":\"night\",\"version\":\"1.0.0\",\"license\":\"MIT\",\"contributes\":{{\"themes\":{contributes}}}}}"
        );
        let mut all: Vec<(String, String)> = vec![("extension/package.json".into(), manifest)];
        all.extend(
            files
                .iter()
                .map(|(n, d)| (format!("extension/{n}"), (*d).to_string())),
        );
        let refs: Vec<(&str, &str)> = all.iter().map(|(n, d)| (n.as_str(), d.as_str())).collect();
        stored(&refs)
    }

    #[test]
    fn include_chains_merge_with_the_including_file_winning() {
        let bytes = theme_package(
            &[
                (
                    "themes/base.json",
                    r##"{"colors":{"editor.background":"#111111","foreground":"#eeeeee"},"tokenColors":[{"scope":"comment","settings":{"foreground":"#666666"}}]}"##,
                ),
                (
                    "themes/main.json",
                    r##"{"include":"./base.json","colors":{"editor.background":"#222222"},"tokenColors":[{"scope":["keyword","storage"],"settings":{"foreground":"#ff0000","fontStyle":"bold italic"}}]}"##,
                ),
            ],
            r##"[{"label":"Night","uiTheme":"vs-dark","path":"./themes/main.json"}]"##,
        );
        let themes = themes_from_package(&bytes, "open-vsx:acme.night", None, ZIP_LIMITS).unwrap();
        assert_eq!(themes.len(), 1);
        let theme = &themes[0];
        assert_eq!(theme.name, "Night");
        assert_eq!(theme.kind, ThemeKind::Dark);
        assert_eq!(theme.colors["editor.background"], "#222222");
        assert_eq!(theme.colors["foreground"], "#eeeeee");
        assert_eq!(theme.token_colors.len(), 2);
        assert_eq!(theme.token_colors[0].scope, vec!["comment"]);
        assert_eq!(
            theme.token_colors[1].font_style.as_deref(),
            Some("bold italic")
        );
    }

    fn chain(depth: usize) -> Vec<(String, String)> {
        (0..=depth)
            .map(|i| {
                let body = if i < depth {
                    format!(
                        "{{\"include\":\"./t{}.json\",\"colors\":{{\"foreground\":\"#ffffff\"}}}}",
                        i + 1
                    )
                } else {
                    "{\"colors\":{\"editor.background\":\"#000000\"}}".to_string()
                };
                (format!("t{i}.json"), body)
            })
            .collect()
    }

    fn load_chain(depth: usize) -> Result<RawTheme> {
        let owned = chain(depth);
        let refs: Vec<(&str, &str)> = owned
            .iter()
            .map(|(n, d)| (n.as_str(), d.as_str()))
            .collect();
        let bytes = stored(&refs);
        let mut package = open_package(&bytes, ZIP_LIMITS).unwrap();
        let mut files = PackageFiles(&mut package);
        load_theme(
            &mut files,
            "t0.json",
            0,
            &mut BTreeMap::new(),
            &mut 0,
            &mut Vec::new(),
        )
    }

    #[test]
    fn include_depth_is_capped_at_eight() {
        // Seven includes means eight files, the deepest chain allowed.
        assert!(load_chain(7).is_ok());
        assert!(message(load_chain(8)).contains("nested too deeply"));
    }

    #[test]
    fn include_cycles_are_rejected() {
        let bytes = stored(&[
            (
                "a.json",
                r##"{"include":"./b.json","colors":{"foreground":"#ffffff"}}"##,
            ),
            ("b.json", r##"{"include":"./a.json"}"##),
        ]);
        let mut package = open_package(&bytes, ZIP_LIMITS).unwrap();
        let mut files = PackageFiles(&mut package);
        let result = load_theme(
            &mut files,
            "a.json",
            0,
            &mut BTreeMap::new(),
            &mut 0,
            &mut Vec::new(),
        );
        assert!(message(result).contains("cycle"));
    }

    #[test]
    fn includes_cannot_leave_the_extension_folder() {
        let bytes = theme_package(
            &[(
                "themes/main.json",
                r##"{"include":"../../package.json","colors":{"foreground":"#fff"}}"##,
            )],
            r##"[{"label":"Escape","uiTheme":"vs","path":"./themes/main.json"}]"##,
        );
        assert!(themes_from_package(&bytes, "x", None, ZIP_LIMITS).is_err());
    }

    #[test]
    fn extensions_with_more_than_40_themes_are_refused() {
        let list: Vec<String> = (0..41)
            .map(|i| format!("{{\"label\":\"T{i}\",\"uiTheme\":\"vs\",\"path\":\"./t.json\"}}"))
            .collect();
        let bytes = theme_package(
            &[("t.json", r##"{"colors":{"foreground":"#111111"}}"##)],
            &format!("[{}]", list.join(",")),
        );
        assert!(message(themes_from_package(&bytes, "x", None, ZIP_LIMITS))
            .contains("too many color themes"));
    }

    #[test]
    fn package_identity_and_license_must_match_the_listing() {
        let bytes = theme_package(
            &[("t.json", r##"{"colors":{"foreground":"#111111"}}"##)],
            r##"[{"label":"A","uiTheme":"vs","path":"./t.json"}]"##,
        );
        let expected = |license: &str, version: &str| Expected {
            namespace: "ACME".into(),
            name: "night".into(),
            version: version.into(),
            license: license.into(),
        };
        assert!(
            themes_from_package(&bytes, "x", Some(&expected("MIT", "1.0.0")), ZIP_LIMITS).is_ok()
        );
        assert!(message(themes_from_package(
            &bytes,
            "x",
            Some(&expected("MIT", "2.0.0")),
            ZIP_LIMITS
        ))
        .contains("does not match"));
        assert!(message(themes_from_package(
            &bytes,
            "x",
            Some(&expected("ISC", "1.0.0")),
            ZIP_LIMITS
        ))
        .contains("license"));
    }

    #[test]
    fn duplicate_labels_get_distinct_names_and_ids() {
        let bytes = theme_package(
            &[("t.json", r##"{"colors":{"foreground":"#111111"}}"##)],
            r##"[{"label":"Same","uiTheme":"vs","path":"./t.json"},{"label":"Same","uiTheme":"vs-dark","path":"./t.json"}]"##,
        );
        let themes = themes_from_package(&bytes, "x", None, ZIP_LIMITS).unwrap();
        assert_eq!(themes[1].name, "Same (2)");
        assert_ne!(themes[0].id, themes[1].id);
    }

    #[test]
    fn kind_comes_from_the_contribution_then_the_file_then_the_background() {
        let none = BTreeMap::new();
        let light_bg = BTreeMap::from([("editor.background".to_string(), "#fafafa".to_string())]);
        assert_eq!(kind_of(Some("vs"), Some("dark"), &none), ThemeKind::Light);
        assert_eq!(kind_of(Some("hc-black"), None, &none), ThemeKind::Dark);
        assert_eq!(kind_of(None, Some("light"), &none), ThemeKind::Light);
        assert_eq!(kind_of(None, None, &light_bg), ThemeKind::Light);
        assert_eq!(kind_of(None, None, &none), ThemeKind::Dark);
    }

    #[test]
    fn sanitizing_drops_unknown_keys_bad_colors_and_bad_scopes() {
        let value: Value = serde_json::from_str(r##"{"colors":{"editor.background":"#ABCDEF","evil.key":"#000000","foreground":"url(x)"},
            "tokenColors":[{"scope":"a, b","settings":{"foreground":"#123","fontStyle":"bold bogus"}},
            {"scope":"<script>","settings":{"foreground":"#123456"}},{"settings":{"foreground":"#abcdef"}},{"scope":"x","settings":{}}]}"##).unwrap();
        let raw = sanitize_theme(value.as_object().unwrap());
        assert_eq!(raw.colors.len(), 1);
        assert_eq!(raw.colors["editor.background"], "#abcdef");
        assert_eq!(raw.token_colors.len(), 2);
        assert_eq!(raw.token_colors[0].scope, vec!["a", "b"]);
        assert_eq!(raw.token_colors[0].foreground.as_deref(), Some("#112233"));
        assert_eq!(raw.token_colors[0].font_style.as_deref(), Some("bold"));
        assert!(raw.token_colors[1].scope.is_empty());
    }

    #[test]
    fn local_json_includes_stay_inside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("base.json"),
            r##"{"colors":{"editor.background":"#101010"}}"##,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("main.json"),
            r##"{"name":"Local","include":"./base.json","colors":{"foreground":"#ffffff"}}"##,
        )
        .unwrap();
        let themes = theme_from_file(&dir.path().join("main.json")).unwrap();
        assert_eq!(themes[0].name, "Local");
        assert_eq!(themes[0].colors.len(), 2);
        assert!(themes[0].source.starts_with("local:"));
        std::fs::write(
            dir.path().join("bad.json"),
            r##"{"include":"../outside.json"}"##,
        )
        .unwrap();
        assert!(theme_from_file(&dir.path().join("bad.json")).is_err());
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        assert!(theme_from_file(&dir.path().join("notes.txt")).is_err());
    }

    // --- Open VSX ----------------------------------------------------------------------------

    #[test]
    fn only_open_vsx_https_hosts_are_trusted() {
        let url = |text: &str| reqwest::Url::parse(text).unwrap();
        assert!(trusted_url(&url("https://open-vsx.org/api/a/b")));
        assert!(trusted_url(&url(
            "https://openvsx.eclipsecontent.org/a/b.vsix"
        )));
        assert!(!trusted_url(&url("http://open-vsx.org/api")));
        assert!(!trusted_url(&url("https://open-vsx.org.evil.com/api")));
        assert!(!trusted_url(&url("https://evil.com/open-vsx.org")));
        assert!(!trusted_url(&url("https://user:pw@open-vsx.org/api")));
        assert!(!trusted_url(&url("https://open-vsx.org:8443/api")));
    }

    fn detail(license: &str, download: &str) -> Value {
        serde_json::json!({
            "namespace": "acme", "name": "night", "version": "1.0.0", "license": license,
            "displayName": "Night", "downloadCount": 12,
            "files": { "download": download, "sha256": "https://open-vsx.org/api/acme/night/1.0.0/file/x.sha256" }
        })
    }

    #[test]
    fn detail_documents_are_checked_for_license_and_host() {
        let ok = resolve_detail(&detail(
            "MIT",
            "https://open-vsx.org/api/acme/night/1.0.0/file/x.vsix",
        ))
        .unwrap();
        assert_eq!(ok.unwrap().extension.display_name, "Night");
        assert!(
            resolve_detail(&detail("GPL-3.0", "https://open-vsx.org/x.vsix"))
                .unwrap()
                .is_none()
        );
        assert!(resolve_detail(&detail("MIT", "https://evil.example/x.vsix")).is_err());
    }

    // --- storage ------------------------------------------------------------------------------

    fn sample(id: &str, kind: ThemeKind) -> VsCodeTheme {
        VsCodeTheme {
            id: id.into(),
            name: "Sample".into(),
            kind,
            source: "local:x.json".into(),
            colors: BTreeMap::from([("editor.background".into(), "#101010".into())]),
            token_colors: vec![TokenRule {
                scope: vec!["comment".into()],
                foreground: Some("#777777".into()),
                font_style: Some("italic".into()),
            }],
        }
    }

    #[test]
    fn stored_themes_are_validated_and_bounded() {
        let ok = sample("vsc-0123456789abcdef", ThemeKind::Dark);
        assert!(validate_stored(std::slice::from_ref(&ok), None, Some(&ok.id)).is_ok());
        // The slot's kind must match.
        assert!(validate_stored(std::slice::from_ref(&ok), Some(&ok.id), None).is_err());
        // Unknown active id.
        assert!(validate_stored(&[], None, Some("vsc-0123456789abcdef")).is_err());
        // Duplicates.
        assert!(validate_stored(&[ok.clone(), ok.clone()], None, None).is_err());
        // Bad id, colour, key, scope, size.
        let mut bad = ok.clone();
        bad.id = "evil".into();
        assert!(check_theme(&bad).is_err());
        let mut bad = ok.clone();
        bad.colors.insert("editor.background".into(), "red".into());
        assert!(check_theme(&bad).is_err());
        let mut bad = ok.clone();
        bad.colors.insert("unknown.key".into(), "#000000".into());
        assert!(check_theme(&bad).is_err());
        let mut bad = ok.clone();
        bad.token_colors[0].scope = vec!["<x>".into()];
        assert!(check_theme(&bad).is_err());
        let mut bad = ok.clone();
        bad.token_colors = vec![bad.token_colors[0].clone(); MAX_TOKEN_RULES + 1];
        assert!(check_theme(&bad).is_err());
        let mut bad = ok.clone();
        bad.name = "x".repeat(MAX_THEME_NAME + 1);
        assert!(check_theme(&bad).is_err());
        // Too many installed themes.
        let many: Vec<VsCodeTheme> = (0..=MAX_STORED_THEMES)
            .map(|i| sample(&format!("vsc-{i:016x}"), ThemeKind::Dark))
            .collect();
        assert!(validate_stored(&many, None, None).is_err());
    }

    #[test]
    fn stored_themes_round_trip_as_camel_case_json() {
        let json = serde_json::to_value(sample("vsc-0123456789abcdef", ThemeKind::Light)).unwrap();
        assert_eq!(json["kind"], "light");
        assert_eq!(json["tokenColors"][0]["fontStyle"], "italic");
        let back: VsCodeTheme = serde_json::from_value(json).unwrap();
        assert_eq!(back.kind, ThemeKind::Light);
    }
}
