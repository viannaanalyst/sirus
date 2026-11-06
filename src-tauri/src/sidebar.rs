use std::collections::HashSet;

use crate::error::{Error, Result};
use crate::models::{AppData, AppSettings, Project, ProjectAppearance, Session};

/// Eye styles and accessories the avatar draws (ADR-175, v2); anything else is refused.
pub const EYES: [&str; 4] = ["bars", "dots", "sleepy", "happy"];
pub const ACCESSORIES: [&str; 17] = [
    "none", "tophat", "crown", "antenna", "bow", "flower", "beanie", "headset", "scarf", "round",
    "square", "cloud", "ghost", "hex", "capsule", "tv", "bean",
];

/// The v2 eyes for a name saved before v2 (`dot` and `round` became `dots`, `wide` became `bars`).
fn current_eyes(name: &str) -> &str {
    match name {
        "dot" | "round" => "dots",
        "wide" => "bars",
        other => other,
    }
}

/// Reads `eyes`, mapping old names so appearances saved before v2 keep loading.
pub fn deserialize_eyes<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let name = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(current_eyes(&name).to_string())
}

/// Checks an avatar choice: `#rrggbb` colour, known eyes and, when given, a known accessory.
pub fn validate_appearance(appearance: &ProjectAppearance) -> Result<()> {
    let hex = appearance.color.strip_prefix('#').unwrap_or("");
    if hex.len() != 6
        || !hex.chars().all(|c| c.is_ascii_hexdigit())
        || !EYES.contains(&appearance.eyes.as_str())
        || appearance
            .accessory
            .as_deref()
            .is_some_and(|name| !ACCESSORIES.contains(&name))
    {
        return Err(Error::new("invalid", "invalid project appearance"));
    }
    Ok(())
}

/// Saves (or clears with `None`) a project's avatar; display metadata only.
pub fn set_appearance(
    data: &mut AppData,
    project_id: &str,
    appearance: Option<ProjectAppearance>,
) -> Result<Project> {
    let appearance = match appearance {
        Some(mut value) => {
            validate_appearance(&value)?;
            value.color = value.color.to_ascii_lowercase();
            Some(value)
        }
        None => None,
    };
    let project = data
        .projects
        .iter_mut()
        .find(|row| row.id == project_id)
        .ok_or_else(|| Error::not_found("project not found"))?;
    project.appearance = appearance;
    Ok(project.clone())
}

/// Most project folders the conversation list remembers as open (ADR-139, round 2).
pub const SIDEBAR_OPEN_FOLDERS_LIMIT: usize = 100;

pub fn validate(settings: &AppSettings) -> Result<()> {
    for ids in [
        &settings.sidebar_project_order,
        &settings.pinned_project_ids,
        &settings.pinned_session_ids,
    ] {
        if ids.len() > 4096 || ids.iter().any(|id| id.is_empty() || id.len() > 64) {
            return Err(Error::new(
                "invalid",
                "sidebar references exceed their bounds",
            ));
        }
    }
    if settings.sidebar_open_folders.as_ref().is_some_and(|ids| {
        ids.len() > SIDEBAR_OPEN_FOLDERS_LIMIT
            || ids.iter().any(|id| id.is_empty() || id.len() > 64)
    }) {
        return Err(Error::new(
            "invalid",
            "open sidebar folders exceed their bounds",
        ));
    }
    if settings.project_folders.len() > 64 {
        return Err(Error::new("invalid", "too many project folders"));
    }
    for folder in &settings.project_folders {
        let name = folder.name.trim();
        if folder.id.is_empty()
            || folder.id.len() > 64
            || name.is_empty()
            || name.chars().count() > 80
            || name.chars().any(char::is_control)
            || folder.project_ids.len() > 4096
            || folder
                .project_ids
                .iter()
                .any(|id| id.is_empty() || id.len() > 64)
        {
            return Err(Error::new("invalid", "project folder exceeds its bounds"));
        }
        crate::project_look::validate_look(&folder.look)?;
    }
    Ok(())
}

pub fn normalize(settings: &mut AppSettings, projects: &[Project], sessions: &[Session]) {
    let project_ids: HashSet<_> = projects.iter().map(|row| row.id.as_str()).collect();
    let session_ids: HashSet<_> = sessions
        .iter()
        .filter(|row| project_ids.contains(row.project_id.as_str()))
        .map(|row| row.id.as_str())
        .collect();
    retain_unique(&mut settings.sidebar_project_order, &project_ids);
    retain_unique(&mut settings.pinned_project_ids, &project_ids);
    if let Some(open) = settings.sidebar_open_folders.as_mut() {
        retain_unique(open, &project_ids);
        open.truncate(SIDEBAR_OPEN_FOLDERS_LIMIT);
    }
    // Folders keep only existing projects, each in one folder, and unique ids.
    let mut placed = HashSet::new();
    let mut folder_ids = HashSet::new();
    settings
        .project_folders
        .retain(|folder| folder_ids.insert(folder.id.clone()));
    for folder in &mut settings.project_folders {
        folder
            .project_ids
            .retain(|id| project_ids.contains(id.as_str()) && placed.insert(id.clone()));
    }
    retain_unique(&mut settings.pinned_session_ids, &session_ids);
    // Rail items removed from the closed set (the old drafts filter, the Automations page)
    // drop out of saved settings.
    let rail: HashSet<&str> = crate::models::RAIL_ITEMS.into_iter().collect();
    retain_unique(&mut settings.rail_item_order, &rail);
    retain_unique(&mut settings.hidden_rail_items, &rail);
}

fn retain_unique(ids: &mut Vec<String>, owned: &HashSet<&str>) {
    let mut seen = HashSet::new();
    ids.retain(|id| owned.contains(id.as_str()) && seen.insert(id.clone()));
    ids.truncate(4096);
}

pub fn prune(data: &mut AppData) {
    normalize(&mut data.settings, &data.projects, &data.sessions);
}

pub fn rename_project(data: &mut AppData, project_id: &str, name: &str) -> Result<Project> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 200 || name.chars().any(char::is_control) {
        return Err(Error::new(
            "invalid",
            "project name must contain 1–200 printable characters",
        ));
    }
    let project = data
        .projects
        .iter_mut()
        .find(|row| row.id == project_id)
        .ok_or_else(|| Error::not_found("project not found"))?;
    // Display metadata only: never rename a directory, worktree or native thread.
    project.name = name.to_owned();
    Ok(project.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> AppData {
        serde_json::from_value(serde_json::json!({
            "settings": {},
            "projects": [{"id":"p","name":"Project","path":"/owned","addedAt":"time","lastOpenedAt":"time"}],
            "sessions": [{"id":"s","title":"Task","projectId":"p","agent":"codex","status":"running","createdAt":"time","lastActivityAt":"time","worktree":{"path":"/owned","branch":"main","isolated":false},"lastError":null,"messages":[]}]
        })).unwrap()
    }

    #[test]
    fn folders_keep_existing_projects_once_and_reject_bad_looks() {
        let mut data = fixture();
        data.settings.project_folders = vec![
            crate::models::ProjectFolder {
                id: "f".into(),
                name: "InChurch".into(),
                project_ids: vec!["p".into(), "gone".into()],
                ..Default::default()
            },
            crate::models::ProjectFolder {
                id: "g".into(),
                name: "Other".into(),
                project_ids: vec!["p".into()],
                ..Default::default()
            },
            crate::models::ProjectFolder {
                id: "f".into(),
                name: "Duplicate".into(),
                ..Default::default()
            },
        ];
        prune(&mut data);
        assert_eq!(data.settings.project_folders.len(), 2);
        assert_eq!(data.settings.project_folders[0].project_ids, ["p"]);
        assert!(data.settings.project_folders[1].project_ids.is_empty());
        assert!(validate(&data.settings).is_ok());
        data.settings.project_folders[0].look.logo = Some("https://example.com/x.png".into());
        assert!(validate(&data.settings).is_err());
        data.settings.project_folders[0].look.logo = None;
        data.settings.project_folders[0].name = "  ".into();
        assert!(validate(&data.settings).is_err());
    }

    #[test]
    fn pin_references_are_bounded_owned_and_do_not_change_execution() {
        let mut data = fixture();
        data.settings.pinned_project_ids = vec!["p".into(), "foreign".into(), "p".into()];
        data.settings.pinned_session_ids = vec!["s".into(), "missing".into()];
        prune(&mut data);
        assert_eq!(data.settings.pinned_project_ids, ["p"]);
        assert_eq!(data.settings.pinned_session_ids, ["s"]);
        assert!(data.sessions[0].status.is_active());
        assert_eq!(data.sessions[0].worktree.path, "/owned");
        data.settings.pinned_project_ids = vec!["p".into(); 4097];
        assert!(validate(&data.settings).is_err());
        data.settings.pinned_project_ids = vec!["x".repeat(65)];
        assert!(validate(&data.settings).is_err());
    }

    #[test]
    fn open_list_folders_are_bounded_and_keep_existing_projects_once() {
        let mut data = fixture();
        assert!(validate(&data.settings).is_ok());
        data.settings.sidebar_open_folders = Some(vec!["p".into(), "gone".into(), "p".into()]);
        assert!(validate(&data.settings).is_ok());
        prune(&mut data);
        assert_eq!(
            data.settings.sidebar_open_folders.as_deref(),
            Some(&["p".to_owned()][..])
        );
        // Every folder closed stays an empty list, not the default.
        data.settings.sidebar_open_folders = Some(vec!["gone".into()]);
        prune(&mut data);
        assert_eq!(data.settings.sidebar_open_folders, Some(Vec::new()));
        data.settings.sidebar_open_folders = Some(vec!["p".into(); SIDEBAR_OPEN_FOLDERS_LIMIT + 1]);
        assert!(validate(&data.settings).is_err());
        data.settings.sidebar_open_folders = Some(vec![String::new()]);
        assert!(validate(&data.settings).is_err());
        data.settings.sidebar_open_folders = Some(vec!["x".repeat(65)]);
        assert!(validate(&data.settings).is_err());
        for value in [
            serde_json::json!(["p"]),
            serde_json::json!([]),
            serde_json::Value::Null,
        ] {
            let saved: AppSettings =
                serde_json::from_value(serde_json::json!({ "sidebarOpenFolders": value })).unwrap();
            assert_eq!(
                serde_json::to_value(saved).unwrap()["sidebarOpenFolders"],
                value
            );
        }
    }

    #[test]
    fn removed_rail_items_drop_out_of_saved_settings() {
        let mut data = fixture();
        data.settings.rail_item_order = vec![
            "drafts".into(),
            "automations".into(),
            "inbox".into(),
            "pulls".into(),
            "pulls".into(),
        ];
        data.settings.hidden_rail_items =
            vec!["drafts".into(), "automations".into(), "pulls".into()];
        prune(&mut data);
        assert_eq!(data.settings.rail_item_order, ["pulls"]);
        assert_eq!(data.settings.hidden_rail_items, ["pulls"]);
        assert!(data.settings.validate_controls().is_ok());
    }

    #[test]
    fn retired_activity_view_settings_load_and_are_dropped_on_save() {
        let legacy: AppSettings = serde_json::from_str(
            r#"{"sidebarActivityView":true,"doneSessions":[{"id":"s","at":"2026-10-04T12:00:00Z"}]}"#,
        )
        .unwrap();
        let saved = serde_json::to_value(&legacy).unwrap();
        assert!(saved.get("sidebarActivityView").is_none());
        assert!(saved.get("doneSessions").is_none());
    }

    #[test]
    fn metadata_removal_prunes_pins() {
        let mut data = fixture();
        data.settings.sidebar_project_order = vec!["p".into()];
        data.settings.pinned_project_ids = vec!["p".into()];
        data.settings.pinned_session_ids = vec!["s".into()];
        data.projects.clear();
        prune(&mut data);
        assert!(data.settings.sidebar_project_order.is_empty());
        assert!(data.settings.pinned_project_ids.is_empty());
        assert!(data.settings.pinned_session_ids.is_empty());
    }

    #[test]
    fn old_settings_and_round_trip_preserve_sidebar_organization() {
        let legacy: AppSettings = serde_json::from_str("{}").unwrap();
        assert!(legacy.sidebar_project_order.is_empty());
        assert!(legacy.pinned_session_ids.is_empty());
        let mut data = fixture();
        data.settings.sidebar_project_order = vec!["p".into()];
        data.settings.pinned_project_ids = vec!["p".into()];
        data.settings.pinned_session_ids = vec!["s".into()];
        let restored: AppData =
            serde_json::from_slice(&serde_json::to_vec(&data).unwrap()).unwrap();
        assert_eq!(restored.settings.sidebar_project_order, ["p"]);
        assert_eq!(restored.settings.pinned_project_ids, ["p"]);
        assert_eq!(restored.settings.pinned_session_ids, ["s"]);
    }

    #[test]
    fn manual_order_retains_owned_order_and_rejects_oversized_lists() {
        let mut data = fixture();
        let mut other = data.projects[0].clone();
        other.id = "second".into();
        data.projects.push(other);
        data.settings.sidebar_project_order = vec![
            "second".into(),
            "foreign".into(),
            "p".into(),
            "second".into(),
        ];
        prune(&mut data);
        assert_eq!(data.settings.sidebar_project_order, ["second", "p"]);
        data.settings.sidebar_project_order = vec!["p".into(); 4097];
        assert!(validate(&data.settings).is_err());
        data.settings.sidebar_project_order = vec!["x".repeat(65)];
        assert!(validate(&data.settings).is_err());
    }

    #[test]
    fn appearance_is_validated_saved_and_cleared() {
        let mut data = fixture();
        let good = || ProjectAppearance {
            color: "#8B7CF6".into(),
            eyes: "dots".into(),
            accessory: Some("crown".into()),
        };
        assert!(set_appearance(&mut data, "foreign", Some(good())).is_err());
        for bad in [
            ProjectAppearance {
                color: "red".into(),
                ..good()
            },
            ProjectAppearance {
                color: "#12345".into(),
                ..good()
            },
            ProjectAppearance {
                color: "#12345g".into(),
                ..good()
            },
            ProjectAppearance {
                eyes: "laser".into(),
                ..good()
            },
            ProjectAppearance {
                eyes: "".into(),
                ..good()
            },
            ProjectAppearance {
                accessory: Some("cape".into()),
                ..good()
            },
        ] {
            assert!(set_appearance(&mut data, "p", Some(bad)).is_err());
        }
        assert!(data.projects[0].appearance.is_none());
        let saved = set_appearance(&mut data, "p", Some(good())).unwrap();
        assert_eq!(saved.appearance.unwrap().color, "#8b7cf6");
        // It survives a JSON round trip (state.db keeps the project as JSON) and old data loads.
        let restored: AppData =
            serde_json::from_slice(&serde_json::to_vec(&data).unwrap()).unwrap();
        let kept = restored.projects[0].appearance.as_ref().unwrap();
        assert_eq!(kept.eyes, "dots");
        assert_eq!(kept.accessory.as_deref(), Some("crown"));
        assert!(fixture().projects[0].appearance.is_none());
        // No accessory is valid (the name's default applies) and is not written out.
        let plain = ProjectAppearance {
            accessory: None,
            ..good()
        };
        assert!(set_appearance(&mut data, "p", Some(plain.clone())).is_ok());
        assert!(!serde_json::to_string(&plain).unwrap().contains("accessory"));
        for accessory in ACCESSORIES {
            let named = ProjectAppearance {
                accessory: Some(accessory.into()),
                ..good()
            };
            assert!(validate_appearance(&named).is_ok(), "{accessory}");
        }
        for eyes in EYES {
            let named = ProjectAppearance {
                eyes: eyes.into(),
                ..good()
            };
            assert!(validate_appearance(&named).is_ok(), "{eyes}");
        }
        assert!(set_appearance(&mut data, "p", None)
            .unwrap()
            .appearance
            .is_none());
    }

    #[test]
    fn appearance_saved_before_v2_still_loads() {
        // The v1 shape: old eye names plus `mouth` and `cheeks`, no accessory.
        let load = |json: &str| -> ProjectAppearance { serde_json::from_str(json).unwrap() };
        for (old, now) in [
            ("dot", "dots"),
            ("round", "dots"),
            ("wide", "bars"),
            ("sleepy", "sleepy"),
        ] {
            let appearance = load(&format!(
                r##"{{"color":"#5b8def","eyes":"{old}","mouth":"smile","cheeks":true}}"##
            ));
            assert_eq!(appearance.eyes, now, "{old}");
            assert_eq!(appearance.accessory, None);
            assert!(validate_appearance(&appearance).is_ok());
            let written = serde_json::to_string(&appearance).unwrap();
            assert!(!written.contains("mouth") && !written.contains("cheeks"));
        }
        // The new fields round trip.
        let appearance = load(r##"{"color":"#fff000","eyes":"happy","accessory":"scarf"}"##);
        assert_eq!(appearance.eyes, "happy");
        assert_eq!(appearance.accessory.as_deref(), Some("scarf"));
        // An unknown eye name is kept as is, so validation (on save) refuses it.
        assert!(validate_appearance(&load(r##"{"color":"#fff000","eyes":"laser"}"##)).is_err());
    }

    #[test]
    fn rename_only_changes_owned_project_display_name() {
        let mut data = fixture();
        assert!(rename_project(&mut data, "foreign", "Name").is_err());
        for name in ["", "   ", "line\nline", &"x".repeat(201)] {
            assert!(rename_project(&mut data, "p", name).is_err());
        }
        let renamed = rename_project(&mut data, "p", "  New name  ").unwrap();
        assert_eq!(renamed.name, "New name");
        assert_eq!(renamed.path, "/owned");
        assert_eq!(data.sessions[0].project_id, "p");
        assert_eq!(data.sessions[0].worktree.path, "/owned");
    }
}
