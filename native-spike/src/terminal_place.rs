//! Where the terminal lives (Settings → Geral → Terminal) and what ⌘J and ⇧⌘J do for each place.
//! Kept apart from the look, in its own small file in the data folder. Pure, so it is tested.

use serde_json::{Value, json};
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Placement {
    /// A strip under the conversation.
    Bottom,
    /// The side panel's Terminal pane: today's behaviour.
    #[default]
    Side,
    /// Terminals as tabs in the title bar.
    TitleBar,
}

impl Placement {
    pub const ALL: [Placement; 3] = [Placement::Bottom, Placement::Side, Placement::TitleBar];

    pub fn id(self) -> &'static str {
        match self {
            Placement::Bottom => "bottom",
            Placement::Side => "side",
            Placement::TitleBar => "titlebar",
        }
    }

    /// The Portuguese label, which is also the translation key.
    pub fn label(self) -> &'static str {
        match self {
            Placement::Bottom => "Embaixo",
            Placement::Side => "Painel lateral",
            Placement::TitleBar => "Barra de título",
        }
    }

    pub fn from_id(id: &str) -> Option<Placement> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }
}

pub fn to_json(placement: Placement) -> Value {
    json!({ "placement": placement.id() })
}

/// An unknown or missing value is the default, the side panel.
pub fn from_json(value: &Value) -> Placement {
    value["placement"]
        .as_str()
        .and_then(Placement::from_id)
        .unwrap_or_default()
}

fn file() -> std::path::PathBuf {
    crate::backend::data_dir().join("native-terminal.json")
}

pub fn load() -> Placement {
    std::fs::read(file())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .map_or_else(Placement::default, |value| from_json(&value))
}

fn save(placement: Placement) {
    let _ = std::fs::create_dir_all(crate::backend::data_dir());
    let _ = std::fs::write(file(), to_json(placement).to_string());
}

static CURRENT: Mutex<Option<Placement>> = Mutex::new(None);

/// The saved placement, read from disk once; Settings and the shell both ask for it.
pub fn current() -> Placement {
    *CURRENT.lock().unwrap().get_or_insert_with(load)
}

pub fn set(placement: Placement) {
    *CURRENT.lock().unwrap() = Some(placement);
    save(placement);
}

/// What ⌘J (`split` false) or ⇧⌘J (`split` true) does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Show the side panel's Terminal pane.
    OpenSide,
    /// Show the bottom strip, or hide it if it is the terminal that is open.
    ToggleBottom,
    /// A new terminal tab in the title bar.
    NewTitleTab,
    /// A shell beside the focused one, inside the focused title tab.
    SplitTitleTab,
    /// One more terminal in the bottom strip or the side panel.
    AddInPanel,
}

pub fn shortcut(placement: Placement, split: bool, title_tab_focused: bool) -> Action {
    match (placement, split) {
        (Placement::Side, false) => Action::OpenSide,
        (Placement::Bottom, false) => Action::ToggleBottom,
        (Placement::TitleBar, false) => Action::NewTitleTab,
        (Placement::TitleBar, true) if title_tab_focused => Action::SplitTitleTab,
        (Placement::TitleBar, true) => Action::NewTitleTab,
        (Placement::Side | Placement::Bottom, true) => Action::AddInPanel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_round_trips_through_json() {
        for p in Placement::ALL {
            assert_eq!(from_json(&to_json(p)), p);
        }
    }

    #[test]
    fn unknown_or_missing_placement_is_the_side_panel() {
        assert_eq!(from_json(&json!({})), Placement::Side);
        assert_eq!(
            from_json(&json!({"placement": "floating"})),
            Placement::Side
        );
        assert_eq!(from_json(&json!({"placement": 3})), Placement::Side);
    }

    #[test]
    fn shortcuts_follow_the_placement() {
        use Action::*;
        assert_eq!(shortcut(Placement::Side, false, false), OpenSide);
        assert_eq!(shortcut(Placement::Bottom, false, false), ToggleBottom);
        assert_eq!(shortcut(Placement::TitleBar, false, true), NewTitleTab);
        assert_eq!(shortcut(Placement::TitleBar, true, true), SplitTitleTab);
        assert_eq!(shortcut(Placement::TitleBar, true, false), NewTitleTab);
        assert_eq!(shortcut(Placement::Side, true, false), AddInPanel);
        assert_eq!(shortcut(Placement::Bottom, true, true), AddInPanel);
    }
}
