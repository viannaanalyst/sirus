//! Every file of a worktree, for the Arquivos search, and the fuzzy matcher that ranks them. After
//! Monocode: `list_project_files` in src-tauri/src/fs.rs (prefer `git ls-files -co
//! --exclude-standard`, which honors .gitignore; otherwise a bounded walk that skips vendor
//! folders) and `shared/lib/fuzzy.ts` (sequential match with bonuses for runs, path breaks and
//! camelCase; a file-name hit outranks a folder-only one). MIT, see LICENSE-MONOCODE-MIT.
//!
//! Everything here blocks: call it through `run_blocking`, never on the UI thread.
use std::path::Path;
use std::process::{Command, Stdio};

/// Files kept from one listing; a monorepo past this is searched in its first part.
const MAX_FILES: usize = 100_000;
/// `git ls-files` output beyond this is dropped for the walk, as Monocode does.
const MAX_LS_FILES_BYTES: usize = 8 * 1024 * 1024;
/// Folders the walk (for folders without Git) never enters.
const SKIPPED_DIRS: &[&str] = &[".git", "node_modules", "target"];

/// Runs `work` on its own thread; the answer arrives on the returned channel.
pub fn run_blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> async_channel::Receiver<T> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(work());
    });
    rx
}

/// The files of `root` as paths relative to it, with `/` separators.
pub fn list(root: &str) -> Vec<String> {
    git_ls_files(root).unwrap_or_else(|| walk(root))
}

fn git_ls_files(root: &str) -> Option<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-co", "--exclude-standard", "-z"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    if output.stdout.len() > MAX_LS_FILES_BYTES {
        return None;
    }
    let mut files: Vec<String> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .filter(|path| !path.ends_with('/') && !path.ends_with(".DS_Store"))
        .take(MAX_FILES)
        .collect();
    // A file in a merge conflict is listed once per stage.
    files.sort_unstable();
    files.dedup();
    Some(files)
}

/// A breadth-first walk for folders Git does not know: bounded by `MAX_FILES`, skipping
/// `SKIPPED_DIRS`, and not following symbolic links.
fn walk(root: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut queue = std::collections::VecDeque::from([String::new()]);
    while let Some(relative) = queue.pop_front() {
        let Ok(entries) = std::fs::read_dir(Path::new(root).join(&relative)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !SKIPPED_DIRS.contains(&name.as_str()) {
                    queue.push_back(path);
                }
            } else if kind.is_file() && name != ".DS_Store" {
                files.push(path);
                if files.len() >= MAX_FILES {
                    return files;
                }
            }
        }
    }
    files.sort_unstable();
    files
}

// ---- Fuzzy search ---------------------------------------------------------------------------

/// A file that matches the query: its relative path and the byte offsets of the matched characters.
pub struct Hit {
    pub path: String,
    /// Byte offset in `path` where the file name starts.
    pub name_start: usize,
    pub positions: Vec<usize>,
}

/// The best `limit` files for `query`, best first. A query of several words must match them all.
pub fn search(files: &[String], query: &str, limit: usize) -> Vec<Hit> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<&str> = query.split_whitespace().collect();
    // Score first, without positions; only the winners pay for them.
    let mut scored: Vec<(i64, usize)> = files
        .iter()
        .enumerate()
        .filter_map(|(index, path)| score_path(&tokens, path, false).map(|hit| (hit.0, index)))
        .collect();
    scored.sort_unstable_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| files[a.1].len().cmp(&files[b.1].len()))
            .then_with(|| files[a.1].cmp(&files[b.1]))
    });
    scored.truncate(limit);
    scored
        .into_iter()
        .filter_map(|(_, index)| {
            let path = &files[index];
            let (_, positions) = score_path(&tokens, path, true)?;
            Some(Hit {
                path: path.clone(),
                name_start: path.rfind('/').map_or(0, |at| at + 1),
                positions,
            })
        })
        .collect()
}

/// A file-name match counts 400 more than a match spread over the folders.
fn score_path(tokens: &[&str], path: &str, positions: bool) -> Option<(i64, Vec<usize>)> {
    let name_start = path.rfind('/').map_or(0, |at| at + 1);
    if let Some((score, found)) = match_tokens(tokens, &path[name_start..], positions) {
        return Some((
            score + 400,
            found.into_iter().map(|at| at + name_start).collect(),
        ));
    }
    match_tokens(tokens, path, positions)
}

fn match_tokens(tokens: &[&str], text: &str, positions: bool) -> Option<(i64, Vec<usize>)> {
    let mut total = 0;
    let mut found = Vec::new();
    for token in tokens {
        let (score, at) = match_token(token, text, positions)?;
        total += score;
        found.extend(at);
    }
    found.sort_unstable();
    found.dedup();
    Some((total, found))
}

fn is_break(c: char) -> bool {
    matches!(c, '/' | '\\' | '-' | '_' | '.' | ' ')
}

/// All of `token`'s characters, in order and ignoring case.
fn match_token(token: &str, text: &str, want_positions: bool) -> Option<(i64, Vec<usize>)> {
    let needle: Vec<char> = token.chars().flat_map(char::to_lowercase).collect();
    let mut positions = Vec::new();
    let (mut score, mut consecutive, mut next) = (0i64, 0i64, 0usize);
    let mut previous: Option<char> = None;
    let mut length = 0i64;
    for (at, c) in text.char_indices() {
        length += 1;
        if next < needle.len() && c.to_lowercase().next() == Some(needle[next]) {
            if want_positions {
                positions.push(at);
            }
            consecutive += 1;
            score += 1 + consecutive * 4;
            match previous {
                None => score += 14,
                Some(before) if is_break(before) => score += 14,
                Some(before) if c.is_uppercase() && !before.is_uppercase() => score += 10,
                _ => {}
            }
            next += 1;
        } else {
            consecutive = 0;
        }
        previous = Some(c);
    }
    if next < needle.len() {
        return None;
    }
    Some((score - (length - needle.len() as i64), positions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_hit_beats_folder_hit() {
        let files = vec![
            "src/panel_files.rs".to_owned(),
            "files/readme.md".to_owned(),
            "docs/a.md".to_owned(),
        ];
        let hits = search(&files, "files", 10);
        assert_eq!(hits[0].path, "src/panel_files.rs");
        assert!(search(&files, "zzz", 10).is_empty());
    }

    #[test]
    fn positions_are_byte_offsets_of_the_match() {
        let files = vec!["src/panel_files.rs".to_owned()];
        let hits = search(&files, "pf", 10);
        assert_eq!(hits[0].positions, vec![4, 10]);
    }
}
