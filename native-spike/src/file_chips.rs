//! The files and folders named in a conversation's chips. A chip opens in the Files pane only when
//! it names something real inside the conversation's folder, so nothing looks clickable for
//! nothing. A name with a path is checked on disk; a bare name is looked up in the folder's file
//! list, refreshed in the background (`due` and `store`).
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How old a folder's file list may get before the next frame asks for it again.
const STALE: Duration = Duration::from_secs(15);

/// What a chip opens: an absolute path inside the conversation's folder, or a PDF outside it.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub path: String,
    pub folder: bool,
    /// Outside the folder: the host wouldn't read it, so the app reads it on the person's click.
    pub outside: bool,
}

/// A folder's files and folders by their last component, as paths relative to it.
#[derive(Default, PartialEq)]
pub struct Index {
    files: HashMap<String, Vec<String>>,
    folders: HashMap<String, Vec<String>>,
}

impl Index {
    pub fn build(paths: Vec<String>) -> Self {
        let mut index = Self::default();
        for path in paths {
            let mut parent = Path::new(&path).parent();
            while let Some(folder) = parent.filter(|p| !p.as_os_str().is_empty()) {
                if let (Some(name), Some(whole)) =
                    (folder.file_name().and_then(|n| n.to_str()), folder.to_str())
                {
                    let known = index.folders.entry(name.to_owned()).or_default();
                    if !known.iter().any(|k| k == whole) {
                        known.push(whole.to_owned());
                    }
                }
                parent = folder.parent();
            }
            let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
            index.files.entry(name).or_default().push(path);
        }
        index
    }

    /// The shallowest path ending in `tail`, whole components only; a tie goes to the first by name.
    fn find(list: Option<&Vec<String>>, tail: &str) -> Option<String> {
        list?
            .iter()
            .filter(|path| {
                path.as_str() == tail
                    || path
                        .strip_suffix(tail)
                        .is_some_and(|head| head.ends_with('/'))
            })
            .min_by_key(|path| (path.matches('/').count(), path.as_str()))
            .cloned()
    }

    fn lookup(&self, tail: &str) -> Option<(String, bool)> {
        let tail = tail.trim_end_matches('/');
        let name = tail.rsplit('/').next().unwrap_or(tail);
        Self::find(self.files.get(name), tail)
            .map(|path| (path, false))
            .or_else(|| Self::find(self.folders.get(name), tail).map(|path| (path, true)))
    }
}

/// `src/a.rs:12:4` names `src/a.rs`.
fn without_position(word: &str) -> &str {
    let mut word = word;
    for _ in 0..2 {
        match word.rsplit_once(':') {
            Some((head, tail))
                if !head.is_empty()
                    && !tail.is_empty()
                    && tail.bytes().all(|b| b.is_ascii_digit()) =>
            {
                word = head;
            }
            _ => break,
        }
    }
    word
}

/// `root/a/../b` as `root/b`, without touching the disk.
fn tidy(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// What `word` names inside `root`. `kind` tells whether a path is a folder, or `None` when it
/// isn't there. A path outside `root` opens nothing, since the host wouldn't read it, except a
/// PDF an agent left elsewhere, like one in Downloads: only the reader opens it.
fn resolve(
    root: &str,
    word: &str,
    home: Option<&str>,
    index: Option<&Index>,
    kind: impl Fn(&Path) -> Option<bool>,
) -> Option<Target> {
    let word = without_position(word.trim());
    if root.is_empty() || word.is_empty() || word.chars().any(char::is_whitespace) {
        return None;
    }
    let root = Path::new(root);
    let home_relative = word.strip_prefix("~/");
    let elsewhere = word.starts_with('/') || home_relative.is_some();
    let written = match home_relative {
        Some(rest) => PathBuf::from(home?).join(rest),
        None => root.join(word),
    };
    let written = tidy(&written);
    if written.starts_with(root)
        && let Some(folder) = kind(&written)
    {
        return Some(Target {
            path: written.to_string_lossy().into_owned(),
            folder,
            outside: false,
        });
    }
    if elsewhere {
        let pdf = word.to_ascii_lowercase().ends_with(".pdf");
        return (pdf && kind(&written) == Some(false)).then(|| Target {
            path: written.to_string_lossy().into_owned(),
            folder: false,
            outside: true,
        });
    }
    let tail = word.trim_start_matches("./").trim_start_matches("../");
    let (found, folder) = index?.lookup(tail)?;
    Some(Target {
        path: root.join(found).to_string_lossy().into_owned(),
        folder,
        outside: false,
    })
}

struct Known {
    index: Option<Rc<Index>>,
    asked: Instant,
}

thread_local! {
    static KNOWN: RefCell<HashMap<String, Known>> = RefCell::new(HashMap::new());
    static LOCAL: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// A window on another computer: its folders aren't on this disk, so chips open by their path
/// alone (`target`).
pub fn set_local(local: bool) {
    LOCAL.set(local);
}

/// What a chip's `word` opens for a conversation in `root`, on this Mac.
pub fn target(root: &str, word: &str) -> Option<Target> {
    if !LOCAL.get() {
        // The folder is on another computer: this disk cannot say what exists, so a path inside
        // the conversation's folder that reads as a file (an extension) or a folder (a trailing
        // slash) opens; the Files pane asks the host and reports what is missing.
        let bare = without_position(word.trim());
        if !bare.is_empty() && !bare.contains('/') && !bare.starts_with('~') {
            return host_known(root, bare);
        }
        return resolve(root, word, None, None, |path| {
            let named = path.file_name()?.to_str()?;
            if word.trim().ends_with('/') {
                Some(true)
            } else {
                named.contains('.').then_some(false)
            }
        });
    }
    let index = KNOWN.with_borrow(|known| known.get(root).and_then(|k| k.index.clone()));
    let home = std::env::var("HOME").ok();
    resolve(root, word, home.as_deref(), index.as_deref(), |path| {
        std::fs::metadata(path).ok().map(|meta| meta.is_dir())
    })
}

/// What the host said about a bare name in a folder: `None` asked and still unanswered,
/// `Some(None)` not there, `Some(Some((relative path, is a folder)))` found.
type Answer = Option<Option<(String, bool)>>;

thread_local! {
    static HOST_NAMES: RefCell<HashMap<(String, String), Answer>> = RefCell::new(HashMap::new());
    static HOST_WANTED: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// A bare name on another computer opens where the host found it. Until it answers the chip is
/// plain, and the name is queued for `wanted`.
fn host_known(root: &str, name: &str) -> Option<Target> {
    let key = (root.to_owned(), name.to_owned());
    let known = HOST_NAMES.with_borrow_mut(|names| match names.get(&key) {
        Some(answer) => answer.clone(),
        None => {
            names.insert(key.clone(), None);
            HOST_WANTED.with_borrow_mut(|wanted| wanted.push(key.clone()));
            None
        }
    });
    let (found, folder) = known??;
    Some(Target {
        path: Path::new(root).join(found).to_string_lossy().into_owned(),
        folder,
        outside: false,
    })
}

/// The (folder, name) pairs waiting for the host, taken once.
pub fn wanted() -> Vec<(String, String)> {
    HOST_WANTED.with_borrow_mut(std::mem::take)
}

/// The shallowest entry of a `workspace_files` reply whose last component is `name`.
pub fn pick(reply: &serde_json::Value, name: &str) -> Option<(String, bool)> {
    reply["entries"]
        .as_array()?
        .iter()
        .filter_map(|entry| {
            let path = entry["path"].as_str()?;
            (path.trim_end_matches('/').rsplit('/').next() == Some(name)).then(|| {
                (
                    path.trim_end_matches('/').to_owned(),
                    entry["isDir"] == true,
                )
            })
        })
        .min_by_key(|(path, _)| (path.matches('/').count(), path.clone()))
}

/// Keeps the host's answer. True when chips should be drawn again.
pub fn answer(root: &str, name: &str, found: Option<(String, bool)>) -> bool {
    HOST_NAMES.with_borrow_mut(|names| {
        names
            .insert((root.to_owned(), name.to_owned()), Some(found))
            .is_none_or(|before| before.is_none())
    })
}

/// True when `root`'s file list should be read again; it counts as asked from here on.
pub fn due(root: &str, now: Instant) -> bool {
    KNOWN.with_borrow_mut(|known| match known.get_mut(root) {
        Some(entry) if now.duration_since(entry.asked) < STALE => false,
        Some(entry) => {
            entry.asked = now;
            true
        }
        None => {
            known.insert(
                root.to_owned(),
                Known {
                    index: None,
                    asked: now,
                },
            );
            true
        }
    })
}

/// Keeps a folder's new file list. True when it changed, so the chips are drawn again.
pub fn store(root: &str, index: Index) -> bool {
    KNOWN.with_borrow_mut(|known| {
        let Some(entry) = known.get_mut(root) else {
            return false;
        };
        if entry.index.as_deref() == Some(&index) {
            return false;
        }
        entry.index = Some(Rc::new(index));
        true
    })
}

/// A click that didn't travel: dragging across a chip selects its text instead of opening it.
pub fn plain_click(event: &gpui::ClickEvent) -> bool {
    match event {
        gpui::ClickEvent::Mouse(click) => {
            let moved = click.up.position - click.down.position;
            f32::from(moved.x).abs() < 4. && f32::from(moved.y).abs() < 4.
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(word: &str, index: &Index) -> Option<Target> {
        resolve(
            "/work/app",
            word,
            Some("/Users/me"),
            Some(index),
            |path| match path.to_str() {
                Some("/work/app/.env.example") => Some(false),
                Some("/work/app/src") | Some("/work/app") => Some(true),
                _ => None,
            },
        )
    }

    #[test]
    fn a_pdf_outside_the_folder_opens_and_nothing_else_does() {
        let there = |word: &str| {
            resolve("/work/app", word, Some("/Users/me"), None, |path| {
                path.starts_with("/Users/me/Downloads").then_some(false)
            })
        };
        assert_eq!(
            there("~/Downloads/plan.PDF"),
            Some(Target {
                path: "/Users/me/Downloads/plan.PDF".to_owned(),
                folder: false,
                outside: true,
            })
        );
        assert_eq!(there("~/Downloads/notes.txt"), None);
        assert_eq!(there("~/Documents/gone.pdf"), None);
    }

    fn file(path: &str) -> Option<Target> {
        Some(Target {
            path: path.to_owned(),
            folder: false,
            outside: false,
        })
    }

    fn folder(path: &str) -> Option<Target> {
        Some(Target {
            path: path.to_owned(),
            folder: true,
            outside: false,
        })
    }

    #[test]
    fn a_path_is_checked_on_disk_and_stays_inside_the_folder() {
        let index = Index::default();
        assert_eq!(
            found(".env.example", &index),
            file("/work/app/.env.example")
        );
        assert_eq!(
            found("./.env.example:12:4", &index),
            file("/work/app/.env.example")
        );
        assert_eq!(found("/work/app/src", &index), folder("/work/app/src"));
        assert_eq!(found("/work/app", &index), folder("/work/app"));
        assert_eq!(found("/work/other/.env.example", &index), None);
        assert_eq!(found("../../etc/hosts", &index), None);
        assert_eq!(found("~/notes.md", &index), None);
        assert_eq!(found("self.dock", &index), None);
        assert_eq!(found("not a path.rs", &index), None);
    }

    #[test]
    fn a_bare_name_is_looked_up_shallowest_first() {
        let index = Index::build(vec![
            "web/deep/src/mod.rs".into(),
            "core/mod.rs".into(),
            "web/HandsPraying.es.js".into(),
        ]);
        assert_eq!(found("mod.rs", &index), file("/work/app/core/mod.rs"));
        assert_eq!(
            found("src/mod.rs", &index),
            file("/work/app/web/deep/src/mod.rs")
        );
        assert_eq!(
            found("HandsPraying.es.js", &index),
            file("/work/app/web/HandsPraying.es.js")
        );
        // Whole components only.
        assert_eq!(found("Praying.es.js", &index), None);
        assert_eq!(found("deep", &index), folder("/work/app/web/deep"));
        assert_eq!(found("web/deep/", &index), folder("/work/app/web/deep"));
    }

    #[test]
    fn a_bare_name_on_another_computer_waits_for_the_host_and_opens_where_it_is() {
        set_local(false);
        assert_eq!(target("/h/app", "cofre.py"), None);
        assert_eq!(wanted(), vec![("/h/app".to_owned(), "cofre.py".to_owned())]);
        assert_eq!(target("/h/app", "cofre.py:3"), None);
        assert!(wanted().is_empty());
        let reply = serde_json::json!({"entries":[
            {"path":"a/b/cofre.py","isDir":false},{"path":"x/cofre.py","isDir":false},
            {"path":"cofre.py.bak","isDir":false}]});
        let found = pick(&reply, "cofre.py");
        assert_eq!(found, Some(("x/cofre.py".to_owned(), false)));
        assert!(answer("/h/app", "cofre.py", found));
        assert_eq!(
            target("/h/app", "cofre.py").map(|t| t.path),
            Some("/h/app/x/cofre.py".to_owned())
        );
        assert!(answer("/h/app", "nada.py", None));
        assert_eq!(target("/h/app", "nada.py"), None);
        set_local(true);
    }

    #[test]
    fn on_another_computer_a_path_inside_the_folder_opens_by_its_shape() {
        set_local(false);
        let at = |word: &str| target("/home/sirus/app", word).map(|t| (t.path, t.folder));
        assert_eq!(
            at("/home/sirus/app/src/a.mjs"),
            Some(("/home/sirus/app/src/a.mjs".into(), false))
        );
        assert_eq!(at("src/"), Some(("/home/sirus/app/src".into(), true)));
        assert_eq!(at("node"), None);
        assert_eq!(at("/etc/passwd.conf"), None);
        set_local(true);
    }
}
