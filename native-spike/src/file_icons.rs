//! File and folder icons as Sirus Code shows them (FileGlyph.tsx, lib/material-file-icon.ts): the
//! Material Icon Theme's SVGs, through react-material-icon-theme. Icon: Material Icon Theme by
//! Philipp Kief and Material Extensions (MIT), packed by Qalxry (MIT, © 2025). The SVGs live in
//! assets/file-icons/. gpui's svg() is a mask, so each icon is drawn in one color from `color`
//! rather than in the pack's own palette.
//!
//! Folders keep the pack's own colors: Monocode names each folder's icon with `getFolderIcon`
//! (react-material-icon-theme), which is the theme's `folderNames` table, ported to
//! folder_icon_data.rs (generated) along with the folder SVGs in assets/file-icons/folders/, in
//! closed and open variants. They are two-tone, so they are drawn as images, not as a mask.
use crate::folder_icon_data;
use gpui::{AnyElement, Hsla, img, prelude::*, px, rgb, svg};

/// The icon for `name` (a folder when `is_dir`), `side` points square, in its main color.
pub fn file_icon(name: &str, is_dir: bool, side: f32) -> AnyElement {
    if is_dir {
        return folder_icon(name, false, side);
    }
    let key = key_for(name);
    svg()
        .path(format!("file-icons/{key}.svg"))
        .size(px(side))
        .text_color(Hsla::from(rgb(color(key))))
        .flex_shrink_0()
        .into_any_element()
}

/// The folder icon for a folder called `name`, in the theme's colors: `docs`, `src`, `supabase`,
/// `node_modules`, `tests`, ... each have their own; any other name gets the plain folder.
pub fn folder_icon(name: &str, open: bool, side: f32) -> AnyElement {
    let id = folder_id(name);
    let suffix = if open { "-open" } else { "" };
    img(format!("file-icons/folders/{id}{suffix}.svg"))
        .size(px(side))
        .flex_shrink_0()
        .into_any_element()
}

/// The icon id for a folder name: the theme lowercases it and ignores leading `.`, `_`, `-` and
/// trailing `.`, `_` (so `.github`, `__tests__` and `_docs` match `github`, `tests` and `docs`).
fn folder_id(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    let key = lower
        .trim_start_matches(['.', '_', '-'])
        .trim_end_matches(['.', '_']);
    folder_icon_data::NAMES
        .binary_search_by(|(known, _)| (*known).cmp(key))
        .map_or("folder", |at| folder_icon_data::NAMES[at].1)
}

/// The embedded SVG for an asset path such as "file-icons/ts.svg"; for the main AssetSource.
pub fn asset(path: &str) -> Option<&'static [u8]> {
    let bytes: &'static [u8] = match path {
        "file-icons/c.svg" => include_bytes!("../assets/file-icons/c.svg"),
        "file-icons/cargo-toml.svg" => include_bytes!("../assets/file-icons/cargo-toml.svg"),
        "file-icons/conf.svg" => include_bytes!("../assets/file-icons/conf.svg"),
        "file-icons/cpp.svg" => include_bytes!("../assets/file-icons/cpp.svg"),
        "file-icons/css.svg" => include_bytes!("../assets/file-icons/css.svg"),
        "file-icons/csv.svg" => include_bytes!("../assets/file-icons/csv.svg"),
        "file-icons/dockerfile.svg" => include_bytes!("../assets/file-icons/dockerfile.svg"),
        "file-icons/docx.svg" => include_bytes!("../assets/file-icons/docx.svg"),
        "file-icons/env.svg" => include_bytes!("../assets/file-icons/env.svg"),
        "file-icons/file.svg" => include_bytes!("../assets/file-icons/file.svg"),
        "file-icons/folder.svg" => include_bytes!("../assets/file-icons/folder.svg"),
        "file-icons/gif.svg" => include_bytes!("../assets/file-icons/gif.svg"),
        "file-icons/gitignore.svg" => include_bytes!("../assets/file-icons/gitignore.svg"),
        "file-icons/go.svg" => include_bytes!("../assets/file-icons/go.svg"),
        "file-icons/h.svg" => include_bytes!("../assets/file-icons/h.svg"),
        "file-icons/html.svg" => include_bytes!("../assets/file-icons/html.svg"),
        "file-icons/ini.svg" => include_bytes!("../assets/file-icons/ini.svg"),
        "file-icons/java.svg" => include_bytes!("../assets/file-icons/java.svg"),
        "file-icons/jpg.svg" => include_bytes!("../assets/file-icons/jpg.svg"),
        "file-icons/js.svg" => include_bytes!("../assets/file-icons/js.svg"),
        "file-icons/json.svg" => include_bytes!("../assets/file-icons/json.svg"),
        "file-icons/jsx.svg" => include_bytes!("../assets/file-icons/jsx.svg"),
        "file-icons/kt.svg" => include_bytes!("../assets/file-icons/kt.svg"),
        "file-icons/license.svg" => include_bytes!("../assets/file-icons/license.svg"),
        "file-icons/lock.svg" => include_bytes!("../assets/file-icons/lock.svg"),
        "file-icons/log.svg" => include_bytes!("../assets/file-icons/log.svg"),
        "file-icons/makefile.svg" => include_bytes!("../assets/file-icons/makefile.svg"),
        "file-icons/md.svg" => include_bytes!("../assets/file-icons/md.svg"),
        "file-icons/mp3.svg" => include_bytes!("../assets/file-icons/mp3.svg"),
        "file-icons/mp4.svg" => include_bytes!("../assets/file-icons/mp4.svg"),
        "file-icons/package-json.svg" => include_bytes!("../assets/file-icons/package-json.svg"),
        "file-icons/package-lock.svg" => include_bytes!("../assets/file-icons/package-lock.svg"),
        "file-icons/pdf.svg" => include_bytes!("../assets/file-icons/pdf.svg"),
        "file-icons/php.svg" => include_bytes!("../assets/file-icons/php.svg"),
        "file-icons/png.svg" => include_bytes!("../assets/file-icons/png.svg"),
        "file-icons/py.svg" => include_bytes!("../assets/file-icons/py.svg"),
        "file-icons/rb.svg" => include_bytes!("../assets/file-icons/rb.svg"),
        "file-icons/rs.svg" => include_bytes!("../assets/file-icons/rs.svg"),
        "file-icons/scss.svg" => include_bytes!("../assets/file-icons/scss.svg"),
        "file-icons/sh.svg" => include_bytes!("../assets/file-icons/sh.svg"),
        "file-icons/svelte.svg" => include_bytes!("../assets/file-icons/svelte.svg"),
        "file-icons/svg.svg" => include_bytes!("../assets/file-icons/svg.svg"),
        "file-icons/swift.svg" => include_bytes!("../assets/file-icons/swift.svg"),
        "file-icons/toml.svg" => include_bytes!("../assets/file-icons/toml.svg"),
        "file-icons/ts.svg" => include_bytes!("../assets/file-icons/ts.svg"),
        "file-icons/tsconfig-json.svg" => include_bytes!("../assets/file-icons/tsconfig-json.svg"),
        "file-icons/tsx.svg" => include_bytes!("../assets/file-icons/tsx.svg"),
        "file-icons/txt.svg" => include_bytes!("../assets/file-icons/txt.svg"),
        "file-icons/vue.svg" => include_bytes!("../assets/file-icons/vue.svg"),
        "file-icons/webp.svg" => include_bytes!("../assets/file-icons/webp.svg"),
        "file-icons/xlsx.svg" => include_bytes!("../assets/file-icons/xlsx.svg"),
        "file-icons/xml.svg" => include_bytes!("../assets/file-icons/xml.svg"),
        "file-icons/yml.svg" => include_bytes!("../assets/file-icons/yml.svg"),
        "file-icons/zip.svg" => include_bytes!("../assets/file-icons/zip.svg"),
        _ => return folder_icon_data::asset(path),
    };
    Some(bytes)
}

/// The asset key for a file name: special names first, then `.env*`, then the extension.
fn key_for(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    match lower.as_str() {
        "package.json" => return "package-json",
        "package-lock.json" => return "package-lock",
        "tsconfig.json" => return "tsconfig-json",
        "cargo.toml" => return "cargo-toml",
        "cargo.lock" => return "lock",
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => return "gitignore",
        "dockerfile" => return "dockerfile",
        "makefile" => return "makefile",
        "license" | "license.md" | "license.txt" => return "license",
        _ => {}
    }
    if lower == ".env" || lower.starts_with(".env.") {
        return "env";
    }
    // Hidden names with no other dot (".bashrc") have no extension, as in materialFileIcon.
    let Some((stem, extension)) = lower.rsplit_once('.') else {
        return "file";
    };
    if stem.is_empty() {
        return "file";
    }
    match extension {
        "ts" | "mts" | "cts" => "ts",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "js" | "mjs" | "cjs" => "js",
        "json" | "json5" | "jsonc" => "json",
        "md" | "mdx" | "markdown" => "md",
        "rs" => "rs",
        "css" => "css",
        "scss" | "sass" => "scss",
        "html" | "htm" => "html",
        "py" | "pyi" => "py",
        "go" => "go",
        "yml" | "yaml" => "yml",
        "toml" => "toml",
        "lock" => "lock",
        "ini" => "ini",
        "conf" | "cfg" => "conf",
        "svg" => "svg",
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "gif" => "gif",
        "webp" => "webp",
        "sh" | "bash" | "zsh" | "fish" => "sh",
        "txt" => "txt",
        "log" => "log",
        "pdf" => "pdf",
        "zip" | "tar" | "gz" | "tgz" | "rar" | "7z" | "bz2" | "xz" => "zip",
        "java" => "java",
        "c" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "h" | "hh" => "h",
        "php" => "php",
        "rb" => "rb",
        "swift" => "swift",
        "kt" | "kts" => "kt",
        "vue" => "vue",
        "svelte" => "svelte",
        "xml" => "xml",
        "csv" | "tsv" => "csv",
        "xlsx" | "xls" | "ods" => "xlsx",
        "docx" | "doc" | "odt" => "docx",
        "mp4" | "mov" | "webm" | "mkv" => "mp4",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => "mp3",
        _ => "file",
    }
}

/// Main color per icon, from the fallback colors in lib/file-icons.ts.
fn color(key: &str) -> u32 {
    match key {
        "ts" | "tsconfig-json" => 0x3178c6,
        "tsx" | "jsx" => 0x61dafb,
        "js" => 0xf7df1e,
        "json" => 0xf5c542,
        "md" | "docx" => 0x6cb6ff,
        "rs" | "cargo-toml" => 0xdea584,
        "css" => 0x42a5f5,
        "scss" => 0xcd6799,
        "html" => 0xe44d26,
        "py" => 0x3776ab,
        "go" => 0x00add8,
        "yml" | "toml" | "ini" | "conf" | "env" => 0xa78bfa,
        "lock" => 0xf59e0b,
        "package-json" | "package-lock" => 0xcb3837,
        "gitignore" => 0xf05032,
        "svg" | "png" | "jpg" | "gif" | "webp" | "csv" | "xlsx" => 0x22c55e,
        "sh" => 0x4ade80,
        "txt" | "log" | "license" | "makefile" => 0x94a3b8,
        "pdf" => 0xef4444,
        "zip" => 0xf97316,
        "java" => 0xf89820,
        "c" | "h" | "cpp" => 0x659ad2,
        "php" => 0x777bb4,
        "rb" => 0xcc342d,
        "swift" => 0xf05138,
        "kt" => 0xa97bff,
        "vue" => 0x42b883,
        "svelte" => 0xff3e00,
        "xml" => 0xe37933,
        "mp4" => 0xc084fc,
        "mp3" => 0x38bdf8,
        "dockerfile" => 0x2496ed,
        "folder" => 0x8b95a7,
        _ => 0x7d7d7d,
    }
}
