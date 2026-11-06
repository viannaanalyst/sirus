//! Global window snap (ADR-054). An opt-in system-wide shortcut captures the
//! frontmost window of the app in front into one short-lived native capture.
//! The renderer is told the app name and claims the capture for the composer
//! it has open; nothing is sent. The renderer cannot name a window, app or
//! file, and a capture expires if nobody claims it.
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::runtime::{AppHandle, Emitter, State};
use serde::{Deserialize, Serialize};

use crate::attachments::PromptAttachment;
use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::AppSettings;

/// A capture older than this is dropped instead of admitted.
const CLAIM_WINDOW: Duration = Duration::from_secs(60);
/// Longest side of the captured image, in pixels.
const MAX_SIDE: f64 = 1800.0;

struct Pending {
    nonce: String,
    app: String,
    at: Instant,
    jpeg: Vec<u8>,
}

static PENDING: parking_lot::Mutex<Option<Pending>> = parking_lot::const_mutex(None);

/// Event `window-snap`: what happened after the shortcut was pressed.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SnapEvent {
    /// A capture is waiting to be claimed for the open composer.
    Ready { nonce: String, app: String },
    /// Screen Recording is not granted to Sirus.
    NeedsPermission,
    /// Sirus itself is in front; there is no other window to snap.
    OwnWindow,
    /// The window could not be captured.
    Failed,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    /// Admits the pending capture into `owner`'s unsent draft.
    Claim { nonce: String, owner: String },
    /// Drops the pending capture.
    Discard { nonce: String },
}

/// Closed renderer surface: claim or drop the one pending capture.
pub async fn window_snap_action(
    state: State<'_, Arc<AppState>>,
    action: Action,
) -> Result<Vec<PromptAttachment>> {
    let state = state.inner().clone();
    crate::commands::native_task(move || match action {
        Action::Discard { nonce } => {
            take(&nonce);
            Ok(Vec::new())
        }
        Action::Claim { nonce, owner } => {
            let pending = take(&nonce)
                .filter(|pending| pending.at.elapsed() <= CLAIM_WINDOW)
                .ok_or_else(|| Error::new("attachment", "That window snap has expired."))?;
            crate::attachments::admit_native_capture(
                &state,
                &owner,
                file_name(&pending.app),
                pending.jpeg,
            )
        }
    })
    .await
}

fn take(nonce: &str) -> Option<Pending> {
    let mut slot = PENDING.lock();
    if slot.as_ref().is_some_and(|pending| pending.nonce == nonce) {
        slot.take()
    } else {
        None
    }
}

/// `<App> window.jpg`, keeping only printable characters that are valid in a file name.
fn file_name(app: &str) -> String {
    let clean: String = app
        .chars()
        .filter(|ch| !ch.is_control() && !matches!(ch, '/' | '\\' | ':'))
        .take(60)
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        "Window.jpg".into()
    } else {
        format!("{clean} window.jpg")
    }
}

fn publish(app: &AppHandle, event: SnapEvent) {
    let _ = app.emit("window-snap", event);
}

/// Registers or removes the shortcut to match settings. Call after load and save.
pub fn apply(app: &AppHandle, settings: &AppSettings) {
    platform::apply(
        app,
        settings.window_snap_enabled,
        settings.window_snap_shortcut,
    );
}

mod platform {
    use super::*;
    pub fn apply(_app: &AppHandle, _enabled: bool, _shortcut: crate::models::WindowSnapShortcut) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_printable_and_bounded() {
        assert_eq!(file_name("Google Chrome"), "Google Chrome window.jpg");
        assert_eq!(file_name("a/b\\c:d\n"), "abcd window.jpg");
        assert_eq!(file_name("  "), "Window.jpg");
        assert!(file_name(&"x".repeat(500)).len() <= 80);
    }

    #[test]
    fn a_capture_is_taken_once_and_only_with_its_nonce() {
        *PENDING.lock() = Some(Pending {
            nonce: "n1".into(),
            app: "App".into(),
            at: Instant::now(),
            jpeg: vec![1],
        });
        assert!(take("other").is_none());
        assert!(take("n1").is_some());
        assert!(take("n1").is_none());
    }

    #[test]
    fn actions_are_closed() {
        assert!(serde_json::from_value::<Action>(
            serde_json::json!({"type":"claim","nonce":"n","owner":"session:s"})
        )
        .is_ok());
        assert!(serde_json::from_value::<Action>(
            serde_json::json!({"type":"claim","nonce":"n","owner":"o","path":"/x"})
        )
        .is_err());
        assert!(serde_json::from_value::<Action>(serde_json::json!({"type":"capture"})).is_err());
    }
}
