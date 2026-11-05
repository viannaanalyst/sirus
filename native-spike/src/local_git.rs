//! Git read locally, in a worktree, for the Alterações history: a commit's changed files with their
//! added and removed lines, and one file's diff. The host back end offers neither, so this runs
//! `git` itself (as `backend::git` does). Everything blocks: call it through
//! `project_files::run_blocking`, never on the UI thread.
//!
//! A merge commit is shown against its first parent, the way Monocode and GitHub list it (a bare
//! `git show` of a merge prints only a combined diff of conflicts). The root commit is shown against
//! the empty tree.
use crate::i18n::{t, tf};
use std::process::{Command, Stdio};

/// Files listed for one commit; a bigger one is cut here.
const MAX_FILES: usize = 3000;
/// A diff longer than this is cut at a line.
const MAX_DIFF_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct CommitFile {
    pub path: String,
    /// A, M, D, T, ... as `git diff --name-status` prints it.
    pub status: char,
    /// None for a binary file.
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

pub struct CommitFiles {
    pub files: Vec<CommitFile>,
    /// More than one parent: the files are those changed against the first.
    pub merge: bool,
    /// The commit had more files than `MAX_FILES`.
    pub truncated: bool,
}

fn git(root: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| tf("Não foi possível executar o git: {}", &[&error]))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if message.is_empty() {
            t("O git falhou.").to_owned()
        } else {
            message
        })
    }
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

/// What to compare `sha` with (its first parent, or the empty tree for a root commit) and whether
/// it is a merge.
fn base_of(root: &str, sha: &str) -> Result<(String, bool), String> {
    if sha.is_empty() || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(t("Commit inválido.").to_owned());
    }
    // "<sha> <parent> <parent>...": one line, the commit first.
    let line = text(git(root, &["rev-list", "--parents", "-n", "1", sha])?);
    let mut ids = line.split_whitespace();
    if ids.next().is_none() {
        return Err(t("Commit não encontrado.").to_owned());
    }
    let parents: Vec<&str> = ids.collect();
    match parents.first() {
        Some(first) => Ok(((*first).to_owned(), parents.len() > 1)),
        None => Ok((
            text(git(root, &["hash-object", "-t", "tree", "/dev/null"])?)
                .trim()
                .to_owned(),
            false,
        )),
    }
}

/// The files `sha` changed, in path order, with their line counts.
pub fn commit_files(root: &str, sha: &str) -> Result<CommitFiles, String> {
    let (base, merge) = base_of(root, sha)?;
    let diff = |flag: &'static str| {
        git(
            root,
            &[
                "diff",
                "--no-ext-diff",
                "--no-renames",
                flag,
                "-z",
                &base,
                sha,
            ],
        )
    };
    let numstat = diff("--numstat")?;
    let statuses = diff("--name-status")?;

    // "<status>\0<path>\0" pairs.
    let mut status_of = std::collections::HashMap::new();
    let mut fields = statuses
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        status_of.insert(
            String::from_utf8_lossy(path).into_owned(),
            status.first().copied().map_or('M', char::from),
        );
    }
    // "<added>\t<removed>\t<path>\0"; "-" counts mean binary.
    let mut files: Vec<CommitFile> = numstat
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let record = String::from_utf8_lossy(record);
            let mut parts = record.splitn(3, '\t');
            let (added, removed, path) = (parts.next()?, parts.next()?, parts.next()?.to_owned());
            Some(CommitFile {
                status: status_of.get(&path).copied().unwrap_or('M'),
                added: added.parse().ok(),
                removed: removed.parse().ok(),
                path,
            })
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let truncated = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    Ok(CommitFiles {
        files,
        merge,
        truncated,
    })
}

/// One file's unified diff in `sha`.
pub fn commit_file_diff(root: &str, sha: &str, path: &str) -> Result<String, String> {
    let (base, _) = base_of(root, sha)?;
    let bytes = git(
        root,
        &[
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--no-color",
            "--no-renames",
            &base,
            sha,
            "--",
            path,
        ],
    )?;
    let mut diff = text(bytes);
    if diff.len() > MAX_DIFF_BYTES {
        let mut cut = MAX_DIFF_BYTES;
        while !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        let cut = diff[..cut].rfind('\n').unwrap_or(cut);
        diff.truncate(cut);
        diff.push_str(t("\n… diff truncado."));
    }
    Ok(diff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn root_normal_and_merge_commits() {
        let dir = std::env::temp_dir().join(format!("sirus-local-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-q", "-m", "root"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        run(&dir, &["commit", "-qam", "second"]);
        run(&dir, &["checkout", "-q", "-b", "side", "HEAD~1"]);
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-q", "-m", "side"]);
        run(&dir, &["checkout", "-q", "main"]);
        run(&dir, &["merge", "-q", "--no-ff", "-m", "merge", "side"]);
        let root = dir.to_str().unwrap();
        let sha = |rev: &str| {
            text(git(root, &["rev-parse", rev]).unwrap())
                .trim()
                .to_owned()
        };

        let first = commit_files(root, &sha("HEAD~2")).unwrap();
        assert_eq!(first.files.len(), 1);
        assert_eq!(
            (first.files[0].status, first.files[0].added),
            ('A', Some(1))
        );
        assert!(
            commit_file_diff(root, &sha("HEAD~2"), "a.txt")
                .unwrap()
                .contains("+one")
        );

        let second = commit_files(root, &sha("HEAD^1")).unwrap();
        assert_eq!(
            (second.files[0].added, second.files[0].removed),
            (Some(1), Some(0))
        );

        let merge = commit_files(root, &sha("HEAD")).unwrap();
        assert!(merge.merge);
        assert_eq!(
            merge
                .files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["b.txt"]
        );
        assert!(
            commit_file_diff(root, &sha("HEAD"), "b.txt")
                .unwrap()
                .contains("+b")
        );
        assert!(commit_files(root, "zzzz").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
