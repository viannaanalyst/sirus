//! Sounds and system banners, which the headless host cannot do: it has no bundle for the
//! notification center to answer to. The window plays and shows what the host asks for over the
//! `notification-native` event, and what Settings → Notificações tests.
use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::NSSound;
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
            handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List,));
        }
    }
);

thread_local! {
    static READY: Cell<bool> = const { Cell::new(false) };
    static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    static SOUND: RefCell<Option<Retained<NSSound>>> = const { RefCell::new(None) };
}

/// The system's sound for a saved choice; an unknown name plays nothing.
fn sound_name(choice: &str) -> Option<&'static str> {
    Some(match choice {
        "glass" => "Glass",
        "ping" => "Ping",
        "pop" => "Pop",
        "submarine" => "Submarine",
        "tink" => "Tink",
        "hero" => "Hero",
        _ => return None,
    })
}

pub fn play(choice: &str) {
    let Some(name) = sound_name(choice) else {
        return;
    };
    let Some(sound) = NSSound::soundNamed(&NSString::from_str(name)) else {
        return;
    };
    SOUND.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(previous) = slot.take() {
            previous.stop();
        }
        sound.play();
        *slot = Some(sound);
    });
}

/// The delegate and the permission prompt, once. The prompt only appears the first time.
fn prepare() -> Retained<UNUserNotificationCenter> {
    let center = UNUserNotificationCenter::currentNotificationCenter();
    if !READY.with(Cell::get) {
        READY.with(|ready| ready.set(true));
        if let Some(main) = MainThreadMarker::new() {
            let delegate: Retained<Delegate> =
                unsafe { msg_send![Delegate::alloc(main), init] };
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
    if let Some(sound) = alert["sound"].as_str() {
        play(sound);
    }
    if alert["system"].as_bool() == Some(true) {
        banner(
            alert["title"].as_str().unwrap_or("Sirus Native"),
            alert["body"].as_str().unwrap_or(""),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_saved_sounds_have_a_name() {
        assert_eq!(sound_name("glass"), Some("Glass"));
        assert_eq!(sound_name("../evil"), None);
    }
}
