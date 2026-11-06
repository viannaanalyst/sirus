//! Project actions: saved commands that run in a terminal with one click, from
//! the menu of the footer's button, which also opens the editor.
//! This file holds what is kept and decided; the window draws it in `shell_actions.rs`.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The icons an action can wear, in picker order; the first is the default.
pub const ICONS: [&str; 6] = ["play", "test-tube", "hammer", "rocket", "bug", "package"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub name: String,
    pub command: String,
    pub icon: String,
}

impl Action {
    pub fn icon(&self) -> &'static str {
        ICONS
            .iter()
            .find(|icon| **icon == self.icon)
            .copied()
            .unwrap_or(ICONS[0])
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Project {
    #[serde(default)]
    actions: Vec<Action>,
}

fn opens_by_default() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Saved {
    #[serde(default)]
    projects: HashMap<String, Project>,
    /// Running an action brings its terminal up; off, it runs out of sight.
    #[serde(default = "opens_by_default")]
    pub opens_terminal: bool,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            projects: HashMap::new(),
            opens_terminal: true,
        }
    }
}

impl Saved {
    pub fn of(&self, project: &str) -> &[Action] {
        self.projects
            .get(project)
            .map_or(&[], |saved| saved.actions.as_slice())
    }

    pub fn find(&self, project: &str, id: &str) -> Option<&Action> {
        self.of(project).iter().find(|action| action.id == id)
    }

    /// Replaces the action with this id, or adds it.
    pub fn put(&mut self, project: &str, action: Action) {
        let saved = self.projects.entry(project.to_owned()).or_default();
        match saved.actions.iter_mut().find(|old| old.id == action.id) {
            Some(slot) => *slot = action,
            None => saved.actions.push(action),
        }
    }

    pub fn remove(&mut self, project: &str, id: &str) {
        if let Some(saved) = self.projects.get_mut(project) {
            saved.actions.retain(|action| action.id != id);
            if saved.actions.is_empty() {
                self.projects.remove(project);
            }
        }
    }
}

fn file() -> std::path::PathBuf {
    crate::backend::data_dir().join("native-actions.json")
}

pub fn load() -> Saved {
    std::fs::read(file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(saved: &Saved) {
    if let Ok(bytes) = serde_json::to_vec_pretty(saved) {
        let _ = std::fs::write(file(), bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(id: &str) -> Action {
        Action {
            id: id.into(),
            name: id.to_uppercase(),
            command: format!("run {id}"),
            icon: "rocket".into(),
        }
    }

    #[test]
    fn editing_keeps_its_place_and_the_last_one_leaves_no_project() {
        let mut saved = Saved::default();
        saved.put("p", action("a"));
        saved.put("p", action("b"));
        let mut edited = action("a");
        edited.command = "new".into();
        saved.put("p", edited);
        assert_eq!(saved.of("p")[0].command, "new");
        assert_eq!(saved.of("p").len(), 2);
        saved.remove("p", "a");
        saved.remove("p", "b");
        assert!(saved.of("p").is_empty());
    }

    #[test]
    fn an_unknown_icon_wears_the_first() {
        let mut odd = action("a");
        odd.icon = "gone".into();
        assert_eq!(odd.icon(), ICONS[0]);
        assert_eq!(action("a").icon(), "rocket");
    }

    #[test]
    fn a_file_without_the_setting_keeps_the_terminal_coming_up() {
        let saved: Saved = serde_json::from_str("{}").unwrap();
        assert!(saved.opens_terminal);
    }
}
