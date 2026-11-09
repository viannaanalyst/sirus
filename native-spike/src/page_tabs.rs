//! Which page tabs (Pull requests, Tarefas) are open, so they come back when the app does:
//! their order, whether each is pinned, and which one was on screen. The shell saves a snapshot
//! when it changes and reopens it at launch; the parts that decide the order are pure.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// A page that lives as a title tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Pulls,
    Tasks,
}

impl Page {
    /// The strip's order, left to right: Pull requests, then Tarefas.
    const STRIP: [Page; 2] = [Page::Pulls, Page::Tasks];

    fn name(self) -> &'static str {
        match self {
            Page::Pulls => "pulls",
            Page::Tasks => "tasks",
        }
    }

    fn from_name(name: &str) -> Option<Page> {
        Page::STRIP.into_iter().find(|p| p.name() == name)
    }
}

/// What is open, pinned and showing.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Snapshot {
    /// Each open page and whether it is pinned.
    pub pages: Vec<(Page, bool)>,
    pub active: Option<Page>,
}

impl Snapshot {
    pub fn to_json(&self) -> Value {
        json!({
            "pages": self.pages.iter().map(|(page, pinned)| json!({
                "page": page.name(),
                "pinned": pinned,
            })).collect::<Vec<_>>(),
            "active": self.active.map(Page::name),
        })
    }

    pub fn from_json(value: &Value) -> Snapshot {
        let pages = value["pages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                Some((
                    Page::from_name(entry["page"].as_str()?)?,
                    entry["pinned"].as_bool().unwrap_or(false),
                ))
            })
            .collect();
        Snapshot {
            pages,
            active: value["active"].as_str().and_then(Page::from_name),
        }
    }

    /// The pages to reopen, in the strip's order, each once; the active one must be among them.
    pub fn restored(&self) -> Snapshot {
        let pages: Vec<(Page, bool)> = Page::STRIP
            .into_iter()
            .filter_map(|page| {
                self.pages
                    .iter()
                    .find(|(p, _)| *p == page)
                    .map(|(_, pinned)| (page, *pinned))
            })
            .collect();
        let active = self
            .active
            .filter(|page| pages.iter().any(|(p, _)| p == page));
        Snapshot { pages, active }
    }
}

pub fn file() -> PathBuf {
    crate::backend::data_dir().join("page-tabs.json")
}

pub fn write(path: &Path, snapshot: &Snapshot) {
    if snapshot.pages.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, snapshot.to_json().to_string());
}

pub fn read(path: &Path) -> Option<Snapshot> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(Snapshot::from_json(&serde_json::from_str(&text).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_come_back_in_the_strips_order_each_once_with_their_pins() {
        let saved = Snapshot {
            pages: vec![
                (Page::Tasks, true),
                (Page::Pulls, false),
                (Page::Tasks, false),
            ],
            active: Some(Page::Tasks),
        };
        let back = saved.restored();
        assert_eq!(back.pages, [(Page::Pulls, false), (Page::Tasks, true)]);
        assert_eq!(back.active, Some(Page::Tasks));
    }

    #[test]
    fn an_active_page_that_is_not_open_is_dropped() {
        let saved = Snapshot {
            pages: vec![(Page::Pulls, true)],
            active: Some(Page::Tasks),
        };
        assert_eq!(saved.restored().active, None);
    }

    #[test]
    fn a_snapshot_survives_the_file_and_ignores_unknown_pages() {
        let saved = Snapshot {
            pages: vec![(Page::Pulls, true), (Page::Tasks, false)],
            active: Some(Page::Pulls),
        };
        assert_eq!(Snapshot::from_json(&saved.to_json()), saved);
        let odd = json!({"pages": [{"page": "nope"}, {"page": "tasks"}], "active": "nope"});
        let read = Snapshot::from_json(&odd);
        assert_eq!(read.pages, [(Page::Tasks, false)]);
        assert_eq!(read.active, None);
    }
}
