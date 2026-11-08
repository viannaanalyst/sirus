//! The saved settings, for the views that are not the shell: the shell notes them when they load
//! and each time one changes, and a panel reads the key it needs while it draws. Everything runs
//! on the main thread.
use serde_json::Value;
use std::cell::{Cell, RefCell};

thread_local! {
    static SETTINGS: RefCell<Value> = const { RefCell::new(Value::Null) };
    static STILL: Cell<bool> = const { Cell::new(false) };
}

/// Keeps the settings the shell just loaded or saved, and whether motion stays still.
pub fn note(settings: &Value) {
    SETTINGS.with(|s| *s.borrow_mut() = settings.clone());
    STILL.with(|still| still.set(reduce_motion()));
}

/// `reduce_motion` as of the last `note`, cheap enough for every tween.
pub fn still() -> bool {
    STILL.with(Cell::get)
}

/// A switch, or `default` while the settings have not come yet or lack it.
pub fn flag(key: &str, default: bool) -> bool {
    SETTINGS.with(|s| s.borrow()[key].as_bool().unwrap_or(default))
}

/// A choice, or `default`.
pub fn choice(key: &str, default: &str) -> String {
    SETTINGS.with(|s| {
        s.borrow()[key]
            .as_str()
            .filter(|v| !v.is_empty())
            .unwrap_or(default)
            .to_owned()
    })
}

/// A number, or `default`.
pub fn number(key: &str, default: u64) -> u64 {
    SETTINGS.with(|s| s.borrow()[key].as_u64().unwrap_or(default))
}

/// Whether the motion that is only decoration should stay still: the switch in Settings →
/// Aparência, or the Mac's own Reduce motion (Accessibility → Display).
pub fn reduce_motion() -> bool {
    flag("reduceMotion", false) || system_reduces_motion()
}

#[cfg(target_os = "macos")]
fn system_reduces_motion() -> bool {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    // SAFETY: NSWorkspace's shared instance always exists and answers this BOOL property.
    unsafe {
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        !workspace.is_null() && msg_send![workspace, accessibilityDisplayShouldReduceMotion]
    }
}

#[cfg(not(target_os = "macos"))]
fn system_reduces_motion() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_keys_fall_back_to_their_defaults() {
        note(&json!({ "gitShowUntracked": false, "terminalScrollback": 10000 }));
        assert!(!flag("gitShowUntracked", true));
        assert!(flag("foldFinishedTurns", true));
        assert_eq!(number("terminalScrollback", 5000), 10000);
        assert_eq!(choice("linkOpenTarget", "app"), "app");
    }
}
