//! Browser profiles (ADR-150), after `src-tauri/src/browser_profiles.rs`: which website data
//! store a tab uses. "default" is WebKit's default store, "private" a non-persistent store per
//! conversation's browser, and a custom profile a persistent store of its own: a
//! `WKWebsiteDataStore` with a UUID identifier (macOS 14 and later). The list is the `browserProfiles`
//! setting (at most ten, each `{id, name}`) beside `defaultBrowserProfile`; the host validates
//! it when the settings are saved, and the same rules are checked here first.
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::NSUUID;
use objc2_web_kit::WKWebsiteDataStore;
use serde_json::{Value, json};

pub const DEFAULT: &str = "default";
pub const PRIVATE: &str = "private";
/// Custom profiles kept in the settings.
pub const MAX_PROFILES: usize = 10;
pub const NAME_LIMIT: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

/// Which store a profile id names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Store {
    Default,
    Private,
    /// A persistent store with this UUID (16 bytes).
    Custom([u8; 16]),
}

/// The id as 16 bytes, only in the canonical lowercase hyphenated form (so one store never has two
/// spellings) and never the nil UUID, which WebKit refuses.
fn uuid_bytes(id: &str) -> Option<[u8; 16]> {
    let b = id.as_bytes();
    if b.len() != 36 || [8, 13, 18, 23].iter().any(|&i| b[i] != b'-') {
        return None;
    }
    let mut out = [0u8; 16];
    let mut nibbles = b
        .iter()
        .enumerate()
        .filter(|(i, _)| ![8, 13, 18, 23].contains(i))
        .map(|(_, c)| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        });
    for byte in &mut out {
        *byte = nibbles.next()?? << 4 | nibbles.next()??;
    }
    out.iter().any(|&v| v != 0).then_some(out)
}

pub fn store_of(id: &str) -> Option<Store> {
    match id {
        DEFAULT => Some(Store::Default),
        PRIVATE => Some(Store::Private),
        other => uuid_bytes(other).map(Store::Custom),
    }
}

/// A fresh random profile id in the canonical form (a version 4 UUID).
pub fn new_id() -> String {
    let mut bytes = [0u8; 16];
    // The kernel's random source: no extra dependency.
    unsafe { libc::arc4random_buf(bytes.as_mut_ptr().cast(), bytes.len()) };
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// True when the name cannot be used. `keep` is the profile being renamed.
pub fn name_problem(profiles: &[Profile], name: &str, keep: Option<&str>) -> bool {
    let name = name.trim();
    let folded = name.to_lowercase();
    name.is_empty()
        || name.chars().count() > NAME_LIMIT
        || name.chars().any(char::is_control)
        || folded == DEFAULT
        || folded == PRIVATE
        || ["padrão", "privado", "default", "private"].contains(&folded.as_str())
        || profiles
            .iter()
            .any(|p| Some(p.id.as_str()) != keep && p.name.to_lowercase() == folded)
}

/// The profile list and default from the settings object. Entries that do not pass the host's
/// validation are left out, so a hand-edited file never breaks the menu.
pub fn from_settings(settings: &Value) -> (Vec<Profile>, String) {
    let mut profiles: Vec<Profile> = Vec::new();
    for item in settings["browserProfiles"].as_array().into_iter().flatten() {
        let (Some(id), Some(name)) = (item["id"].as_str(), item["name"].as_str()) else {
            continue;
        };
        let candidate = Profile {
            id: id.to_owned(),
            name: name.to_owned(),
        };
        if uuid_bytes(id).is_some()
            && !profiles.iter().any(|p| p.id == id)
            && !name_problem(&profiles, name, None)
            && name == name.trim()
            && profiles.len() < MAX_PROFILES
        {
            profiles.push(candidate);
        }
    }
    let default = settings["defaultBrowserProfile"]
        .as_str()
        .filter(|id| *id == DEFAULT || *id == PRIVATE || profiles.iter().any(|p| p.id == *id))
        .unwrap_or(DEFAULT)
        .to_owned();
    (profiles, default)
}

pub fn to_json(profiles: &[Profile]) -> Value {
    Value::Array(
        profiles
            .iter()
            .map(|p| json!({ "id": p.id, "name": p.name }))
            .collect(),
    )
}

/// Whether the system has `WKWebsiteDataStore(forIdentifier:)` (macOS 14).
pub fn supported() -> bool {
    objc2::class!(WKWebsiteDataStore).responds_to(objc2::sel!(dataStoreForIdentifier:))
}

/// The persistent store of a custom profile; the default store when the system lacks them.
pub fn custom_store(bytes: [u8; 16], mtm: MainThreadMarker) -> Retained<WKWebsiteDataStore> {
    unsafe {
        if supported() {
            WKWebsiteDataStore::dataStoreForIdentifier(&NSUUID::from_bytes(bytes), mtm)
        } else {
            WKWebsiteDataStore::defaultDataStore(mtm)
        }
    }
}

/// Deletes a custom profile's data from disk; every tab using it must be closed first. `done`
/// gets the system's error text if it failed. False when nothing was started.
pub fn remove_data(
    id: &str,
    done: impl Fn(Option<String>) + 'static,
    mtm: MainThreadMarker,
) -> bool {
    let Some(Store::Custom(bytes)) = store_of(id) else {
        return false;
    };
    if !supported() {
        return false;
    }
    let handler = block2::RcBlock::new(move |error: *mut objc2_foundation::NSError| {
        done(unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string()));
    });
    unsafe {
        WKWebsiteDataStore::removeDataStoreForIdentifier_completionHandler(
            &NSUUID::from_bytes(bytes),
            &handler,
            mtm,
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: &str = "6f1c2a4e-1b7d-4d39-9f4e-2f0f3c7f6a11";
    const TEST: &str = "0b7f5a7e-58e4-4a83-a8fd-7d3b3c9e2c42";

    fn profile(id: &str, name: &str) -> Profile {
        Profile {
            id: id.into(),
            name: name.into(),
        }
    }

    #[test]
    fn ids_name_built_in_and_canonical_custom_stores() {
        assert_eq!(store_of("default"), Some(Store::Default));
        assert_eq!(store_of("private"), Some(Store::Private));
        assert!(matches!(store_of(WORK), Some(Store::Custom(b)) if b[0] == 0x6f && b[15] == 0x11));
        assert_eq!(store_of(&WORK.to_uppercase()), None);
        assert_eq!(store_of("00000000-0000-0000-0000-000000000000"), None);
        assert_eq!(store_of("work"), None);
        assert_eq!(store_of(&WORK[..35]), None);
    }

    #[test]
    fn new_ids_are_canonical_version_4_and_distinct() {
        let (a, b) = (new_id(), new_id());
        assert_ne!(a, b);
        assert!(matches!(store_of(&a), Some(Store::Custom(_))));
        assert_eq!(&a[14..15], "4");
        assert!(["8", "9", "a", "b"].contains(&&a[19..20]));
    }

    #[test]
    fn names_are_short_distinct_and_not_built_in() {
        let list = vec![profile(WORK, "Work")];
        assert!(!name_problem(&list, "Personal", None));
        assert!(name_problem(&list, "  ", None));
        assert!(name_problem(&list, "work", None));
        assert!(!name_problem(&list, "work", Some(WORK)));
        assert!(name_problem(&list, "Default", None));
        assert!(name_problem(&list, "PRIVATE", None));
        assert!(name_problem(&list, &"x".repeat(41), None));
        assert!(!name_problem(&list, &"x".repeat(40), None));
        assert!(name_problem(&list, "a\u{7}b", None));
    }

    #[test]
    fn settings_are_read_leniently() {
        let settings = json!({
            "browserProfiles": [
                { "id": WORK, "name": "Work" },
                { "id": WORK, "name": "Again" },
                { "id": "nope", "name": "Bad id" },
                { "id": TEST, "name": " Padded " },
                { "id": TEST, "name": "Test" },
                { "name": "No id" },
            ],
            "defaultBrowserProfile": TEST,
        });
        let (profiles, default) = from_settings(&settings);
        assert_eq!(profiles, [profile(WORK, "Work"), profile(TEST, "Test")]);
        assert_eq!(default, TEST);
        // A default that is gone falls back.
        let (_, default) = from_settings(&json!({ "defaultBrowserProfile": WORK }));
        assert_eq!(default, "default");
        let (_, default) = from_settings(&json!({ "defaultBrowserProfile": "private" }));
        assert_eq!(default, "private");
        assert_eq!(to_json(&[profile(WORK, "Work")])[0]["name"], "Work");
    }
}
