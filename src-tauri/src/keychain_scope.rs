//! Which Keychain items this process owns.
//!
//! Sirus Native runs its own headless host next to Sirus. Sharing one Keychain item made
//! each app rewrite the other's secrets, and macOS then asked for the login password. With
//! `SIRUS_KEYCHAIN_APP` set, the host keeps its secrets under that app's name. It copies an
//! existing Sirus secret only when macOS allows it without asking, and never writes back.
#![cfg(target_os = "macos")]

use security_framework::passwords::{get_generic_password, set_generic_password};

const SHARED_APP: &str = "Sirus";

fn separate_app() -> Option<String> {
    std::env::var("SIRUS_KEYCHAIN_APP")
        .ok()
        .filter(|name| !name.is_empty() && name != SHARED_APP)
}

/// The Keychain service for `kind`, such as `connectors`.
pub fn service(kind: &str) -> String {
    format!("{} {kind}", separate_app().as_deref().unwrap_or(SHARED_APP))
}

#[link(name = "Security", kind = "framework")]
extern "C" {
    fn SecKeychainSetUserInteractionAllowed(allowed: u8) -> i32;
}

/// Copies a Sirus secret into this app's item if macOS gives it up without a prompt.
pub fn adopt_shared(kind: &str, account: &str) -> Option<Vec<u8>> {
    let own = separate_app().map(|app| format!("{app} {kind}"))?;
    let shared = format!("{SHARED_APP} {kind}");
    // SAFETY: a plain flag call. It is process-wide, so it is restored right after the read.
    unsafe { SecKeychainSetUserInteractionAllowed(0) };
    let bytes = get_generic_password(&shared, account);
    unsafe { SecKeychainSetUserInteractionAllowed(1) };
    let bytes = bytes.ok()?;
    let _ = set_generic_password(&own, account, &bytes);
    Some(bytes)
}
