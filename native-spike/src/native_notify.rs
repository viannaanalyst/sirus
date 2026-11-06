//! Sounds and system banners, which the headless host cannot do: it has no bundle for the
//! notification center to answer to. The window plays and shows what the host asks for over the
//! `notification-native` event, and what Settings → Notificações tests.
use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use serde_json::Value;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SirusNativeNotificationDelegate"]
    struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        // Without this a banner is dropped while the app is in front, as during a test.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler
                .call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
        }
    }
);

thread_local! {
    static READY: Cell<bool> = const { Cell::new(false) };
    static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
}

/// Which of The three sounds a name stands for. An unknown name plays nothing.
fn cue_of(choice: &str) -> Option<crate::sound::Cue> {
    use crate::sound::Cue;
    Some(match choice {
        "finished" => Cue::Finished,
        "input" => Cue::Input,
        "linked" => Cue::LinkedActivity,
        _ => return None,
    })
}

pub fn play(choice: &str) {
    if let Some(cue) = cue_of(choice) {
        crate::sound::play(cue);
    }
}

/// The delegate and the permission prompt, once. The prompt only appears the first time.
fn prepare() -> Retained<UNUserNotificationCenter> {
    let center = UNUserNotificationCenter::currentNotificationCenter();
    if !READY.with(Cell::get) {
        READY.with(|ready| ready.set(true));
        if let Some(main) = MainThreadMarker::new() {
            let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(main), init] };
            center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            DELEGATE.with(|slot| *slot.borrow_mut() = Some(delegate));
        }
        let done = block2::RcBlock::new(
            move |_granted: objc2::runtime::Bool, _error: *mut objc2_foundation::NSError| {},
        );
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &done,
        );
    }
    center
}

pub fn banner(title: &str, body: &str) {
    let center = prepare();
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let id = format!("sirus-native.{stamp}");
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&id),
        &content,
        None,
    );
    center.addNotificationRequest_withCompletionHandler(&request, None);
}

/// What the host asked for, by its rules: in front of the person, only if they want that too.
pub fn deliver(alert: &Value, window_active: bool) {
    if window_active && alert["foreground"].as_bool() != Some(true) {
        return;
    }
    // The host says which sound the event takes, and only when sounds are on.
    if !alert["sound"].is_null()
        && let Some(cue) = alert["cue"].as_str()
    {
        play(cue);
    }
    if alert["system"].as_bool() == Some(true) {
        banner(
            alert["title"].as_str().unwrap_or("Sirus"),
            alert["body"].as_str().unwrap_or(""),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_saved_sounds_have_a_name() {
        assert!(cue_of("finished").is_some());
        assert!(cue_of("../evil").is_none());
    }
}
