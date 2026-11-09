//! Which optional entries the sidebar shows (the three-dots menu beside its title).
//! Kept in a small file of its own in the data folder, like `terminal_place`. One line
//! in `ITEMS` per entry; an entry the file does not mention is shown. Pure, so it is tested.

use serde_json::{Map, Value};

/// One optional sidebar entry.
pub struct Item {
    /// The key in the saved file and in the shell's checks.
    pub id: &'static str,
    /// The Portuguese label, which is also the translation key.
    pub label: &'static str,
    pub icon: &'static str,
}

/// The entries the menu lists, in order. The owner adds one line here per new entry.
pub const ITEMS: [Item; 5] = [
    Item {
        id: "new",
        label: "Novo",
        icon: "square-plus",
    },
    Item {
        id: "search",
        label: "Buscar",
        icon: "search",
    },
    Item {
        id: "pulls",
        label: "Pull requests",
        icon: "git-pull-request",
    },
    Item {
        id: "tasks",
        label: "Tarefas",
        icon: "task-clock",
    },
    Item {
        id: "projects",
        label: "Projetos",
        icon: "folder",
    },
];

/// The navigation rows under the project's name, in their default order. The owner drags them to
/// reorder; the saved order (`sidebarNavOrder` in the settings) is merged with this list.
pub const NAV: [&str; 3] = ["new", "pulls", "tasks"];

/// The height of one navigation slot, in px, and the distance a press travels before it drags.
pub const NAV_SLOT: f32 = 32.;
pub const NAV_DRAG_AFTER: f32 = 4.;

/// The saved order merged with `known`: saved entries that still exist, once each, in their saved
/// order, then the entries the saved order does not mention (new in this version) at the end.
pub fn merge_order(saved: &[String], known: &[&'static str]) -> Vec<&'static str> {
    let mut order: Vec<&'static str> = Vec::new();
    for id in saved {
        if let Some(found) = known.iter().find(|k| **k == id.as_str())
            && !order.contains(found)
        {
            order.push(found);
        }
    }
    for id in known {
        if !order.contains(id) {
            order.push(id);
        }
    }
    order
}

/// The slot the dragged row's top edge is nearest, among `count` slots.
pub fn slot_at(top: f32, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    ((top / NAV_SLOT + 0.5).floor().max(0.) as usize).min(count - 1)
}

/// `full` with `id` moved to the `to`-th place among the shown entries. Hidden entries keep
/// their places relative to the others.
pub fn place(
    full: &[&'static str],
    id: &str,
    to: usize,
    shown: impl Fn(&str) -> bool,
) -> Vec<&'static str> {
    let mut visible: Vec<&'static str> = full.iter().copied().filter(|i| shown(i)).collect();
    let Some(from) = visible.iter().position(|i| *i == id) else {
        return full.to_vec();
    };
    let moved = visible.remove(from);
    visible.insert(to.min(visible.len()), moved);
    let mut next = visible.into_iter();
    full.iter()
        .map(|i| if shown(i) { next.next().unwrap_or(*i) } else { *i })
        .collect()
}

/// The entries the user turned off.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Hidden(Vec<&'static str>);

impl Hidden {
    pub fn shown(&self, id: &str) -> bool {
        !self.0.contains(&id)
    }

    /// Flips one entry; an id that is not in `ITEMS` changes nothing.
    pub fn toggle(&mut self, id: &str) {
        let Some(item) = ITEMS.iter().find(|item| item.id == id) else {
            return;
        };
        match self.0.iter().position(|hidden| *hidden == id) {
            Some(at) => {
                self.0.remove(at);
            }
            None => self.0.push(item.id),
        }
    }

    pub fn to_json(&self) -> Value {
        let map: Map<String, Value> = ITEMS
            .iter()
            .map(|item| (item.id.to_owned(), Value::Bool(self.shown(item.id))))
            .collect();
        Value::Object(map)
    }

    /// Anything missing or not a boolean is shown.
    pub fn from_json(value: &Value) -> Self {
        Self(
            ITEMS
                .iter()
                .filter(|item| value[item.id].as_bool() == Some(false))
                .map(|item| item.id)
                .collect(),
        )
    }
}

fn file() -> std::path::PathBuf {
    crate::backend::data_dir().join("native-sidebar.json")
}

pub fn load() -> Hidden {
    std::fs::read(file())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .map_or_else(Hidden::default, |value| Hidden::from_json(&value))
}

pub fn save(hidden: &Hidden) {
    let _ = std::fs::create_dir_all(crate::backend::data_dir());
    let _ = std::fs::write(file(), hidden.to_json().to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn everything_is_shown_by_default() {
        let hidden = Hidden::default();
        assert!(ITEMS.iter().all(|item| hidden.shown(item.id)));
        assert_eq!(Hidden::from_json(&json!({})), hidden);
    }

    #[test]
    fn only_an_explicit_false_hides_an_entry() {
        let hidden = Hidden::from_json(&json!({ "search": false, "pulls": "no" }));
        assert!(!hidden.shown("search"));
        assert!(hidden.shown("pulls"));
        assert_eq!(Hidden::from_json(&json!(null)), Hidden::default());
        assert_eq!(Hidden::from_json(&json!([false])), Hidden::default());
    }

    #[test]
    fn the_choice_round_trips_through_json() {
        let mut hidden = Hidden::default();
        hidden.toggle("pulls");
        assert_eq!(Hidden::from_json(&hidden.to_json()), hidden);
        assert!(hidden.shown("search") && !hidden.shown("pulls"));
        hidden.toggle("pulls");
        assert_eq!(Hidden::from_json(&hidden.to_json()), Hidden::default());
    }

    fn owned(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|i| (*i).to_owned()).collect()
    }

    #[test]
    fn the_saved_order_wins_and_new_entries_go_last() {
        let known = ["new", "pulls", "tasks"];
        assert_eq!(merge_order(&[], &known), known);
        assert_eq!(
            merge_order(&owned(&["tasks", "new"]), &known),
            ["tasks", "new", "pulls"]
        );
    }

    #[test]
    fn removed_and_repeated_entries_leave_the_order() {
        let known = ["new", "tasks"];
        assert_eq!(
            merge_order(&owned(&["usage", "tasks", "tasks", "new", "pulls"]), &known),
            ["tasks", "new"]
        );
    }

    #[test]
    fn the_nearest_slot_is_clamped() {
        assert_eq!(slot_at(-20., 3), 0);
        assert_eq!(slot_at(15., 3), 0);
        assert_eq!(slot_at(17., 3), 1);
        assert_eq!(slot_at(500., 3), 2);
        assert_eq!(slot_at(10., 0), 0);
    }

    #[test]
    fn moving_a_row_keeps_hidden_entries_in_place() {
        let full = ["new", "pulls", "tasks"];
        assert_eq!(place(&full, "tasks", 0, |_| true), ["tasks", "new", "pulls"]);
        assert_eq!(place(&full, "new", 2, |_| true), ["pulls", "tasks", "new"]);
        // "pulls" is hidden: tasks moves before new, and pulls stays in the middle.
        assert_eq!(
            place(&full, "tasks", 0, |i| i != "pulls"),
            ["tasks", "pulls", "new"]
        );
        assert_eq!(place(&full, "search", 0, |_| true), full);
    }

    #[test]
    fn an_unknown_entry_is_ignored() {
        let mut hidden = Hidden::default();
        hidden.toggle("notes");
        assert_eq!(hidden, Hidden::default());
        assert_eq!(
            Hidden::from_json(&json!({ "notes": false })),
            Hidden::default()
        );
    }
}
