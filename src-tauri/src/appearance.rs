//! Closed, persisted appearance preferences and native-only window effects.
use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::{AppSettings, AppearanceSupport, ThemePref};
use std::sync::Arc;

pub fn support() -> AppearanceSupport {
    AppearanceSupport {
        translucency: false,
        dock_icon: false,
    }
}

/// Bounds for kept terminal lines (the settings offer 2000, 5000 and 10000).
pub const TERMINAL_SCROLLBACK: (u32, u32) = (1000, 10_000);

pub fn validate(settings: &AppSettings) -> Result<()> {
    if settings.theme == ThemePref::Translucent {
        return Err(Error::new(
            "invalid_settings",
            "Choose System, Light or Dark; translucency is a material preference.",
        ));
    }
    for (name, value, min, max) in [
        ("darkWindowOpacity", settings.dark_window_opacity, 25, 100),
        ("lightWindowOpacity", settings.light_window_opacity, 25, 100),
        ("darkSidebarOpacity", settings.dark_sidebar_opacity, 25, 100),
        (
            "lightSidebarOpacity",
            settings.light_sidebar_opacity,
            25,
            100,
        ),
        ("translucentOpacity", settings.translucent_opacity, 25, 100),
        ("uiFontSize", settings.ui_font_size, 11, 18),
        ("codeFontSize", settings.code_font_size, 10, 22),
        ("terminalFontSize", settings.terminal_font_size, 10, 22),
        // xterm keeps every scrollback line in memory, per terminal.
        (
            "terminalScrollback",
            settings.terminal_scrollback,
            TERMINAL_SCROLLBACK.0,
            TERMINAL_SCROLLBACK.1,
        ),
    ] {
        if !(min..=max).contains(&value) {
            return Err(Error::new(
                "invalid_settings",
                format!("{name} must be an integer from {min} to {max}."),
            ));
        }
    }
    Ok(())
}

/// Only stored numeric values are normalized; incoming saves are rejected if
/// out of range. Unknown families/themes/icons fail closed during serde decoding.
pub fn normalize(settings: &mut AppSettings) {
    for (value, min, max, fallback) in [
        (&mut settings.dark_window_opacity, 25, 100, 85),
        (&mut settings.light_window_opacity, 25, 100, 85),
        (&mut settings.dark_sidebar_opacity, 25, 100, 72),
        (&mut settings.light_sidebar_opacity, 25, 100, 38),
        (&mut settings.translucent_opacity, 25, 100, 85),
        (&mut settings.ui_font_size, 11, 18, 13),
        (&mut settings.code_font_size, 10, 22, 13),
        (&mut settings.terminal_font_size, 10, 22, 13),
        (
            &mut settings.terminal_scrollback,
            TERMINAL_SCROLLBACK.0,
            TERMINAL_SCROLLBACK.1,
            2000,
        ),
    ] {
        if !(min..=max).contains(value) {
            *value = fallback;
        }
    }
    if settings.theme == ThemePref::Translucent {
        settings.theme = ThemePref::Dark;
        settings.dark_window_translucent = true;
        settings.dark_window_opacity = settings.translucent_opacity;
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowIntent {
    glass: bool,
    light: bool,
    follows_system: bool,
    background: [u8; 3],
}

#[allow(dead_code)]
fn window_intent(settings: &AppSettings, system_light: bool) -> WindowIntent {
    let light =
        settings.theme == ThemePref::Light || (settings.theme == ThemePref::System && system_light);
    let glass = if light {
        settings.light_sidebar_translucent || settings.light_window_translucent
    } else {
        settings.dark_sidebar_translucent || settings.dark_window_translucent
    };
    WindowIntent {
        glass: support().translucency && glass,
        light,
        follows_system: settings.theme == ThemePref::System,
        background: if light { [244, 244, 245] } else { [12, 12, 12] },
    }
}

pub fn schedule(app: &crate::runtime::AppHandle, state: Arc<AppState>) {
    schedule_current(app, state, false);
}

/// The headless host has no window to dress (ADR-131).
fn schedule_current(_app: &crate::runtime::AppHandle, _state: Arc<AppState>, _force_dock: bool) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DockIcon, MonoFont, UiFont};

    #[test]
    fn legacy_defaults_preserve_system_and_migrate_neutral_glass() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        let defaults = serde_json::to_value(&settings).unwrap();
        assert_eq!(defaults["theme"], "dark");
        assert_eq!(defaults["darkSidebarOpacity"], 72);
        assert_eq!(defaults["lightSidebarOpacity"], 38);
        assert_eq!(defaults["translucentOpacity"], 85);
        assert_eq!(defaults["systemUiFont"], true);
        assert_eq!(defaults["uiFont"], "inter");
        assert_eq!(defaults["uiFontSize"], 13);
        assert_eq!(defaults["codeFont"], "sfMono");
        assert_eq!(defaults["codeFontSize"], 13);
        assert_eq!(defaults["terminalFont"], "sfMono");
        assert_eq!(defaults["terminalFontSize"], 13);
        assert_eq!(defaults["fontSmoothing"], true);
        assert_eq!(defaults["dockIcon"], "default");
        assert_eq!(defaults["darkSidebarTranslucent"], false);
        assert_eq!(defaults["lightSidebarTranslucent"], false);
        // Sounds are opt-in (ADR-171): a missing key stays off, and an explicit true survives.
        assert_eq!(defaults["agentSounds"], false);
        let sounds: AppSettings = serde_json::from_str(r#"{"agentSounds":true}"#).unwrap();
        assert!(sounds.agent_sounds);
        let migrated: AppSettings =
            serde_json::from_str(r#"{"theme":"system","glass":true}"#).unwrap();
        assert_eq!(migrated.theme, ThemePref::System);
        assert!(!window_intent(&migrated, false).glass);
        assert_eq!(serde_json::to_value(migrated).unwrap()["theme"], "system");
        assert_eq!(
            serde_json::to_value(serde_json::from_value::<AppSettings>(defaults.clone()).unwrap())
                .unwrap(),
            defaults
        );
    }

    #[test]
    fn incoming_ranges_are_validated_and_stored_ranges_normalized() {
        for (field, min, max, fallback) in [
            ("darkWindowOpacity", 25, 100, 85),
            ("lightWindowOpacity", 25, 100, 85),
            ("darkSidebarOpacity", 25, 100, 72),
            ("lightSidebarOpacity", 25, 100, 38),
            ("translucentOpacity", 25, 100, 85),
            ("uiFontSize", 11, 18, 13),
            ("codeFontSize", 10, 22, 13),
            ("terminalFontSize", 10, 22, 13),
            ("terminalScrollback", 1000, 10_000, 2000),
        ] {
            for value in [min, max] {
                let settings: AppSettings =
                    serde_json::from_value(serde_json::json!({field: value})).unwrap();
                assert!(settings.validate_controls().is_ok(), "{field}:{value}");
            }
            for value in [min - 1, max + 1] {
                let mut settings: AppSettings =
                    serde_json::from_value(serde_json::json!({field: value})).unwrap();
                assert!(settings.validate_controls().is_err(), "{field}:{value}");
                normalize(&mut settings);
                assert_eq!(serde_json::to_value(settings).unwrap()[field], fallback);
            }
            for value in [
                serde_json::json!(1.5),
                serde_json::json!(-1),
                serde_json::json!("50"),
            ] {
                assert!(
                    serde_json::from_value::<AppSettings>(serde_json::json!({field: value}))
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn closed_families_theme_and_dock_choices_reject_renderer_values() {
        for field in ["uiFont", "codeFont", "terminalFont", "dockIcon", "theme"] {
            for value in [
                serde_json::json!("/tmp/font"),
                serde_json::json!("https://font"),
                serde_json::json!(42),
                serde_json::Value::Null,
            ] {
                assert!(
                    serde_json::from_value::<AppSettings>(serde_json::json!({field: value}))
                        .is_err()
                );
            }
        }
        for font in [
            "inter",
            "geist",
            "dmSans",
            "plexSans",
            "humanist",
            "helvetica",
        ] {
            assert!(serde_json::from_value::<UiFont>(serde_json::json!(font)).is_ok());
        }
        for font in [
            "plexMono",
            "jetbrains",
            "fira",
            "geistMono",
            "source",
            "roboto",
            "ubuntu",
            "sfMono",
            "menlo",
            "cascadia",
            "hack",
            "consolas",
        ] {
            assert!(serde_json::from_value::<MonoFont>(serde_json::json!(font)).is_ok());
        }
        assert!(serde_json::from_value::<DockIcon>(serde_json::json!("smokedGlass")).is_ok());
    }

    #[test]
    fn dock_alternatives_survive_settings_round_trip() {
        for choice in ["default", "smokedGlass", "white"] {
            let settings: AppSettings =
                serde_json::from_value(serde_json::json!({ "dockIcon": choice })).unwrap();
            let value = serde_json::to_value(&settings).unwrap();
            assert_eq!(value["dockIcon"], choice);
            let restored: AppSettings = serde_json::from_value(value).unwrap();
            assert_eq!(restored.dock_icon, settings.dock_icon);
        }
    }

    #[test]
    fn native_intent_tracks_each_theme_and_independent_sidebar_choices() {
        for (theme, dark, light, glass, background) in [
            (ThemePref::Dark, false, true, false, [12, 12, 12]),
            (ThemePref::Dark, true, false, true, [12, 12, 12]),
            (ThemePref::Light, true, false, false, [244, 244, 245]),
            (ThemePref::Light, false, true, true, [244, 244, 245]),
            (ThemePref::System, false, false, false, [12, 12, 12]),
        ] {
            let settings = AppSettings {
                theme,
                dark_sidebar_translucent: dark,
                light_sidebar_translucent: light,
                ..AppSettings::default()
            };
            let intent = window_intent(&settings, false);
            assert!(!intent.glass, "{glass}");
            assert_eq!(intent.light, settings.theme == ThemePref::Light);
            assert_eq!(intent.background, background);
        }
        let mut settings = AppSettings {
            theme: ThemePref::System,
            dark_window_translucent: true,
            light_window_translucent: false,
            ..AppSettings::default()
        };
        let dark = window_intent(&settings, false);
        let light = window_intent(&settings, true);
        assert!(dark.follows_system && light.follows_system);
        assert!(!dark.light && light.light);
        assert!(!dark.glass);
        assert!(!light.glass);
        settings.theme = ThemePref::Dark;
        assert!(!window_intent(&settings, true).follows_system);
        assert!(!window_intent(&settings, true).light);
        settings.theme = ThemePref::Translucent;
        settings.translucent_opacity = 43;
        assert!(validate(&settings).is_err());
        normalize(&mut settings);
        assert_eq!(settings.theme, ThemePref::Dark);
        assert!(settings.dark_window_translucent);
        assert_eq!(settings.dark_window_opacity, 43);
        assert!(validate(&settings).is_ok());
        assert!(!support().dock_icon);
    }
}
