//! Which optional entries the sidebar shows (the three-dots menu beside its title, as in
//! Monocode). Kept in a small file of its own in the data folder, like `terminal_place`. One line
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
pub const ITEMS: [Item; 4] = [
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
        id: "projects",
        label: "Projetos",
        icon: "folder",
    },
];

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
