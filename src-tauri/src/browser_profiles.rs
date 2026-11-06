//! Browser profiles (ADR-150): which website data store a browser tab uses.
//! "Default" is WebKit's default store (cookies shared with every tab before profiles),
//! "Private" is a non-persistent store per session browser, dropped with its last tab,
//! and custom profiles are persistent stores of their own (`WKWebsiteDataStore` with an
//! identifier, macOS 14 and later). The list lives in settings and is validated here.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const DEFAULT_PROFILE: &str = "default";
pub const PRIVATE_PROFILE: &str = "private";
/// Custom profiles kept in settings.
pub const MAX_PROFILES: usize = 10;
const NAME_LIMIT: usize = 40;

/// A custom persistent profile: its data store identifier (a UUID) and the name people see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfile {
    pub id: String,
    pub name: String,
}

pub fn default_profile_id() -> String {
    DEFAULT_PROFILE.into()
}

/// Which store a profile id names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Store {
    Default,
    Private,
    /// A persistent store with this UUID.
    Custom(uuid::Uuid),
}

fn custom_id(id: &str) -> Option<uuid::Uuid> {
    let parsed = uuid::Uuid::parse_str(id).ok()?;
    // Only the canonical lowercase form, so one store never has two spellings.
    (parsed.hyphenated().to_string() == id && !parsed.is_nil()).then_some(parsed)
}

pub fn store_of(id: &str) -> Option<Store> {
    match id {
        DEFAULT_PROFILE => Some(Store::Default),
        PRIVATE_PROFILE => Some(Store::Private),
        other => custom_id(other).map(Store::Custom),
    }
}

fn invalid(message: &str) -> Error {
    Error::new("invalid_settings", message)
}

/// Settings check: at most ten custom profiles with canonical UUID ids, short distinct
/// names (never "Default" or "Private"), and a default that names a known profile.
pub fn validate(profiles: &[BrowserProfile], default: &str) -> Result<()> {
    if profiles.len() > MAX_PROFILES {
        return Err(invalid("Keep at most 10 browser profiles."));
    }
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for profile in profiles {
        if custom_id(&profile.id).is_none() || !ids.insert(profile.id.as_str()) {
            return Err(invalid("Invalid browser profile id."));
        }
        let name = profile.name.trim();
        if name.is_empty()
            || name != profile.name
            || name.chars().count() > NAME_LIMIT
            || name.chars().any(char::is_control)
        {
            return Err(invalid("A browser profile needs a short name."));
        }
        let folded = name.to_lowercase();
        if folded == DEFAULT_PROFILE || folded == PRIVATE_PROFILE || !names.insert(folded) {
            return Err(invalid("Browser profile names must be distinct."));
        }
    }
    if default != DEFAULT_PROFILE
        && default != PRIVATE_PROFILE
        && !profiles.iter().any(|profile| profile.id == default)
    {
        return Err(invalid("The default browser profile does not exist."));
    }
    Ok(())
}

/// The profile id an agent or the pane asked for, by id or by name (any case); `None`
/// is the default from settings. An unknown request is an error, a stale default is not.
pub fn resolve(
    profiles: &[BrowserProfile],
    default: &str,
    requested: Option<&str>,
) -> Result<String> {
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        let known = default == DEFAULT_PROFILE
            || default == PRIVATE_PROFILE
            || profiles.iter().any(|profile| profile.id == default);
        return Ok(if known {
            default.into()
        } else {
            DEFAULT_PROFILE.into()
        });
    };
    if requested.eq_ignore_ascii_case(DEFAULT_PROFILE) {
        return Ok(DEFAULT_PROFILE.into());
    }
    if requested.eq_ignore_ascii_case(PRIVATE_PROFILE) {
        return Ok(PRIVATE_PROFILE.into());
    }
    profiles
        .iter()
        .find(|profile| profile.id == requested || profile.name.eq_ignore_ascii_case(requested))
        .map(|profile| profile.id.clone())
        .ok_or_else(|| Error::not_found("There is no browser profile with that name."))
}

/// `WKWebsiteDataStore(forIdentifier:)` arrived in macOS 14.
pub fn custom_supported() -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};
        NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
            majorVersion: 14,
            minorVersion: 0,
            patchVersion: 0,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, name: &str) -> BrowserProfile {
        BrowserProfile {
            id: id.into(),
            name: name.into(),
        }
    }

    const WORK: &str = "6f1c2a4e-1b7d-4d39-9f4e-2f0f3c7f6a11";
    const TEST: &str = "0b7f5a7e-58e4-4a83-a8fd-7d3b3c9e2c42";

    #[test]
    fn profiles_validate_ids_names_count_and_default() {
        assert!(validate(&[], DEFAULT_PROFILE).is_ok());
        assert!(validate(&[], PRIVATE_PROFILE).is_ok());
        assert!(validate(&[profile(WORK, "Work")], WORK).is_ok());
        assert!(validate(&[], WORK).is_err());
        assert!(validate(&[profile("work", "Work")], DEFAULT_PROFILE).is_err());
        assert!(validate(&[profile(&WORK.to_uppercase(), "Work")], DEFAULT_PROFILE).is_err());
        assert!(validate(
            &[profile("00000000-0000-0000-0000-000000000000", "Nil")],
            DEFAULT_PROFILE
        )
        .is_err());
        assert!(validate(
            &[profile(WORK, "Work"), profile(WORK, "Other")],
            DEFAULT_PROFILE
        )
        .is_err());
        assert!(validate(
            &[profile(WORK, "Work"), profile(TEST, "work")],
            DEFAULT_PROFILE
        )
        .is_err());
        assert!(validate(&[profile(WORK, "Private")], DEFAULT_PROFILE).is_err());
        assert!(validate(&[profile(WORK, " Work")], DEFAULT_PROFILE).is_err());
        assert!(validate(&[profile(WORK, "")], DEFAULT_PROFILE).is_err());
        assert!(validate(&[profile(WORK, "a\nb")], DEFAULT_PROFILE).is_err());
        assert!(validate(&[profile(WORK, &"x".repeat(41))], DEFAULT_PROFILE).is_err());
        let many: Vec<_> = (0..11)
            .map(|index| profile(&uuid::Uuid::new_v4().to_string(), &format!("P{index}")))
            .collect();
        assert!(validate(&many, DEFAULT_PROFILE).is_err());
        assert!(validate(&many[..10], DEFAULT_PROFILE).is_ok());
    }

    #[test]
    fn requests_resolve_by_id_or_name_and_stale_defaults_fall_back() {
        let list = [profile(WORK, "Work")];
        assert_eq!(
            resolve(&list, DEFAULT_PROFILE, None).unwrap(),
            DEFAULT_PROFILE
        );
        assert_eq!(resolve(&list, WORK, None).unwrap(), WORK);
        assert_eq!(resolve(&list, TEST, None).unwrap(), DEFAULT_PROFILE);
        assert_eq!(resolve(&list, DEFAULT_PROFILE, Some("work")).unwrap(), WORK);
        assert_eq!(resolve(&list, DEFAULT_PROFILE, Some(WORK)).unwrap(), WORK);
        assert_eq!(
            resolve(&list, WORK, Some("Private")).unwrap(),
            PRIVATE_PROFILE
        );
        assert_eq!(resolve(&list, WORK, Some(" ")).unwrap(), WORK);
        assert!(resolve(&list, DEFAULT_PROFILE, Some("Personal")).is_err());
        assert_eq!(
            store_of(WORK),
            Some(Store::Custom(uuid::Uuid::parse_str(WORK).unwrap()))
        );
        assert_eq!(store_of("private"), Some(Store::Private));
        assert_eq!(store_of("Private"), None);
    }
}
