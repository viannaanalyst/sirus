//! What may already own the quick composer's global hotkey: macOS system shortcuts (read from
//! `com.apple.symbolichotkeys`) and well-known apps that listen to the keyboard on their own.
//! Everything here is pure except `load_symbolic_hotkeys` and `bundle_installed`, which read the
//! system.

use serde_json::Value;

/// A shortcut in the settings' form (`ctrl+alt+space`), split up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    pub key: String,
}

impl Combo {
    /// Parses `meta+ctrl+alt+shift+key` (any order, modifiers optional); `None` without a key.
    pub fn parse(combo: &str) -> Option<Combo> {
        let mut parts: Vec<&str> = combo.split('+').collect();
        let key = parts.pop().filter(|key| !key.is_empty())?;
        let mut out = Combo {
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
            key: key.to_owned(),
        };
        for part in parts {
            match part {
                "ctrl" => out.ctrl = true,
                "alt" => out.alt = true,
                "shift" => out.shift = true,
                "meta" => out.meta = true,
                _ => return None,
            }
        }
        Some(out)
    }

    /// The modifier bits macOS stores in a symbolic hotkey.
    fn flags(&self) -> u64 {
        let mut flags = 0;
        if self.shift {
            flags |= SHIFT;
        }
        if self.ctrl {
            flags |= CTRL;
        }
        if self.alt {
            flags |= OPTION;
        }
        if self.meta {
            flags |= COMMAND;
        }
        flags
    }
}

const SHIFT: u64 = 0x20000;
const CTRL: u64 = 0x40000;
const OPTION: u64 = 0x80000;
const COMMAND: u64 = 0x100000;
const MODIFIERS: u64 = SHIFT | CTRL | OPTION | COMMAND;

/// The virtual key code (ANSI layout) of a key as the settings name it.
pub(crate) fn key_code(key: &str) -> Option<u64> {
    const KEYS: &[(&str, u64)] = &[
        ("a", 0),
        ("s", 1),
        ("d", 2),
        ("f", 3),
        ("h", 4),
        ("g", 5),
        ("z", 6),
        ("x", 7),
        ("c", 8),
        ("v", 9),
        ("b", 11),
        ("q", 12),
        ("w", 13),
        ("e", 14),
        ("r", 15),
        ("y", 16),
        ("t", 17),
        ("1", 18),
        ("2", 19),
        ("3", 20),
        ("4", 21),
        ("6", 22),
        ("5", 23),
        ("9", 25),
        ("7", 26),
        ("8", 28),
        ("0", 29),
        ("o", 31),
        ("u", 32),
        ("i", 34),
        ("p", 35),
        ("l", 37),
        ("j", 38),
        ("k", 40),
        ("n", 45),
        ("m", 46),
        ("tab", 48),
        ("space", 49),
    ];
    KEYS.iter()
        .find(|(name, _)| *name == key)
        .map(|(_, code)| *code)
}

/// The system shortcuts worth naming: (id in `AppleSymbolicHotKeys`, Portuguese name).
pub(crate) const SYSTEM_NAMES: &[(&str, &str)] = &[
    ("64", "Spotlight"),
    ("65", "Busca do Finder"),
    ("60", "Fonte de entrada anterior"),
    ("61", "Próxima fonte de entrada"),
    ("28", "Captura da tela inteira"),
    (
        "29",
        "Captura da tela inteira (para a área de transferência)",
    ),
    ("30", "Captura de uma seleção"),
    (
        "31",
        "Captura de uma seleção (para a área de transferência)",
    ),
    ("184", "Captura e gravação de tela"),
    ("32", "Mission Control"),
    ("33", "Janelas do app"),
    ("36", "Mostrar a mesa"),
    ("79", "Mover para o espaço à esquerda"),
    ("81", "Mover para o espaço à direita"),
];

/// The names of the enabled macOS shortcuts that use `combo`. `symbolic` is the content of
/// `com.apple.symbolichotkeys` (`{"AppleSymbolicHotKeys": {id: {enabled, value: {parameters}}}}`).
pub(crate) fn system_conflicts(symbolic: &Value, combo: &str) -> Vec<&'static str> {
    let Some(combo) = Combo::parse(combo) else {
        return Vec::new();
    };
    let Some(code) = key_code(&combo.key) else {
        return Vec::new();
    };
    let table = &symbolic["AppleSymbolicHotKeys"];
    SYSTEM_NAMES
        .iter()
        .filter(|(id, _)| {
            let entry = &table[*id];
            let enabled = match &entry["enabled"] {
                Value::Bool(on) => *on,
                Value::Number(n) => n.as_i64() != Some(0),
                _ => false,
            };
            let params = &entry["value"]["parameters"];
            enabled
                && params[1].as_u64() == Some(code)
                && params[2].as_u64().map(|flags| flags & MODIFIERS) == Some(combo.flags())
        })
        .map(|(_, name)| *name)
        .collect()
}

/// Apps that listen to the keyboard on their own. A BEST GUESS: the `.app` names are what these
/// apps usually install as and the combos are what they ship with, which people change; nothing
/// here is read from the apps themselves. (display name, `.app` names, usual combos)
pub(crate) const KNOWN_APPS: &[(&str, &[&str], &[&str])] = &[
    (
        "Fluid Voice",
        &["FluidVoice.app", "Fluid Voice.app"],
        &["alt+space"],
    ),
    ("Raycast", &["Raycast.app"], &["alt+space"]),
    (
        "Alfred",
        &["Alfred 5.app", "Alfred 4.app", "Alfred.app"],
        &["alt+space"],
    ),
    ("ChatGPT", &["ChatGPT.app"], &["alt+space"]),
    (
        "Superwhisper",
        &["superwhisper.app", "Superwhisper.app"],
        &["alt+space"],
    ),
    ("Claude", &["Claude.app"], &["alt+space"]),
    ("LaunchBar", &["LaunchBar.app"], &["meta+space"]),
    ("Quicksilver", &["Quicksilver.app"], &["ctrl+space"]),
];

/// The known apps installed per `installed` (asked with each `.app` name) whose usual combos
/// include `combo`: (name, usual combo).
pub(crate) fn app_conflicts(
    combo: &str,
    installed: impl Fn(&str) -> bool,
) -> Vec<(&'static str, &'static str)> {
    KNOWN_APPS
        .iter()
        .filter(|(_, bundles, _)| bundles.iter().any(|bundle| installed(bundle)))
        .filter_map(|(name, _, combos)| {
            combos
                .iter()
                .find(|usual| **usual == combo)
                .map(|usual| (*name, *usual))
        })
        .collect()
}

/// Whether an app bundle sits in one of the folders apps are installed to.
pub(crate) fn bundle_installed(bundle: &str) -> bool {
    let mut folders = vec![
        std::path::PathBuf::from("/Applications"),
        std::path::PathBuf::from("/Applications/Setapp"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        folders.push(std::path::Path::new(&home).join("Applications"));
    }
    folders.iter().any(|folder| folder.join(bundle).exists())
}

/// The system's symbolic hotkeys, read once (`defaults` exports the live domain, `plutil` turns
/// it into JSON). `Null` when it cannot be read.
pub(crate) fn load_symbolic_hotkeys() -> &'static Value {
    static LOADED: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    LOADED.get_or_init(|| {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let Ok(export) = Command::new("/usr/bin/defaults")
            .args(["export", "com.apple.symbolichotkeys", "-"])
            .output()
        else {
            return Value::Null;
        };
        let Ok(mut plutil) = Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return Value::Null;
        };
        if let Some(mut stdin) = plutil.stdin.take() {
            let _ = stdin.write_all(&export.stdout);
        }
        plutil
            .wait_with_output()
            .ok()
            .and_then(|out| serde_json::from_slice(&out.stdout).ok())
            .unwrap_or(Value::Null)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn symbolic() -> Value {
        json!({ "AppleSymbolicHotKeys": {
            "64": { "enabled": true, "value": { "parameters": [32, 49, 1048576] } },
            "60": { "enabled": true, "value": { "parameters": [32, 49, 262144] } },
            "61": { "enabled": false, "value": { "parameters": [32, 49, 786432] } },
            "65": { "enabled": true, "value": { "parameters": [32, 49, 1572864] } },
            "28": { "enabled": true, "value": { "parameters": [51, 20, 1179648] } },
        } })
    }

    #[test]
    fn parses_combos() {
        let combo = Combo::parse("meta+ctrl+alt+shift+k").unwrap();
        assert!(combo.meta && combo.ctrl && combo.alt && combo.shift);
        assert_eq!(combo.key, "k");
        assert_eq!(Combo::parse("space").unwrap().flags(), 0);
        assert!(Combo::parse("hyper+k").is_none());
        assert!(Combo::parse("alt+").is_none());
    }

    #[test]
    fn matches_enabled_system_shortcuts() {
        let symbolic = symbolic();
        assert_eq!(system_conflicts(&symbolic, "meta+space"), ["Spotlight"]);
        assert_eq!(
            system_conflicts(&symbolic, "ctrl+space"),
            ["Fonte de entrada anterior"]
        );
        assert_eq!(
            system_conflicts(&symbolic, "meta+alt+space"),
            ["Busca do Finder"]
        );
        assert_eq!(
            system_conflicts(&symbolic, "meta+shift+3"),
            ["Captura da tela inteira"]
        );
    }

    #[test]
    fn ignores_disabled_and_other_combos() {
        let symbolic = symbolic();
        // Entry 61 is ctrl+alt+space but off.
        assert!(system_conflicts(&symbolic, "ctrl+alt+space").is_empty());
        assert!(system_conflicts(&symbolic, "alt+space").is_empty());
        assert!(system_conflicts(&Value::Null, "meta+space").is_empty());
        assert!(system_conflicts(&symbolic, "ctrl+alt+f12").is_empty());
    }

    #[test]
    fn matches_installed_apps() {
        let has = |bundle: &str| bundle == "Raycast.app" || bundle == "FluidVoice.app";
        let found = app_conflicts("alt+space", has);
        assert_eq!(
            found,
            [("Fluid Voice", "alt+space"), ("Raycast", "alt+space")]
        );
        assert!(app_conflicts("ctrl+alt+space", has).is_empty());
        assert!(app_conflicts("alt+space", |_| false).is_empty());
        assert!(app_conflicts("meta+space", has).is_empty());
    }
}
