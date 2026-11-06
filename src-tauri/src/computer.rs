//! Native computer use (ADR-038): Accessibility reads/actions, per-process input,
//! ScreenCaptureKit window screenshots and the on-screen control indicator.
//! All Accessibility work runs on one dedicated worker thread (AX elements are
//! not `Send`), which also serializes calls into other applications.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub pid: i32,
    pub bundle_id: String,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Element {
    #[serde(rename = "ref")]
    pub reference: String,
    pub role: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    /// Window-relative points, origin at the window's top-left.
    pub frame: Frame,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub app: String,
    pub window: String,
    pub window_frame: Frame,
    pub elements: Vec<Element>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    pub supported: bool,
    pub accessibility: bool,
    pub screen_recording: bool,
}

/// Interactive roles worth a ref; static text is kept for reading results.
const ROLES: &[&str] = &[
    "AXButton",
    "AXCheckBox",
    "AXRadioButton",
    "AXPopUpButton",
    "AXMenuButton",
    "AXTextField",
    "AXTextArea",
    "AXComboBox",
    "AXSlider",
    "AXLink",
    "AXMenuItem",
    "AXIncrementor",
    "AXDisclosureTriangle",
    "AXColorWell",
    "AXRow",
    "AXCell",
    "AXTab",
    "AXStaticText",
    "AXHeading",
    "AXImage",
    "AXSegmentedControl",
    "AXSearchField",
    "AXStepper",
    "AXDateField",
];
const TEXT_LIMIT: usize = 160;

fn clip(text: &str) -> String {
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = clean.trim();
    if trimmed.chars().count() <= TEXT_LIMIT {
        trimmed.to_string()
    } else {
        format!("{}…", trimmed.chars().take(TEXT_LIMIT).collect::<String>())
    }
}

/// Virtual key codes (ANSI layout) for named keys and printable characters.
pub fn key_code(name: &str) -> Option<u16> {
    let named = match name.to_ascii_lowercase().as_str() {
        "return" | "enter" => 36,
        "tab" => 48,
        "space" => 49,
        "delete" | "backspace" => 51,
        "escape" | "esc" => 53,
        "forwarddelete" => 117,
        "home" => 115,
        "end" => 119,
        "pageup" => 116,
        "pagedown" => 121,
        "left" | "arrowleft" => 123,
        "right" | "arrowright" => 124,
        "down" | "arrowdown" => 125,
        "up" | "arrowup" => 126,
        "f1" => 122,
        "f2" => 120,
        "f3" => 99,
        "f4" => 118,
        "f5" => 96,
        "f6" => 97,
        "f7" => 98,
        "f8" => 100,
        "f9" => 101,
        "f10" => 109,
        "f11" => 103,
        "f12" => 111,
        _ => u16::MAX,
    };
    if named != u16::MAX {
        return Some(named);
    }
    let mut chars = name.chars();
    let (Some(character), None) = (chars.next(), chars.next()) else {
        return None;
    };
    Some(match character.to_ascii_lowercase() {
        'a' => 0,
        's' => 1,
        'd' => 2,
        'f' => 3,
        'h' => 4,
        'g' => 5,
        'z' => 6,
        'x' => 7,
        'c' => 8,
        'v' => 9,
        'b' => 11,
        'q' => 12,
        'w' => 13,
        'e' => 14,
        'r' => 15,
        'y' => 16,
        't' => 17,
        '1' => 18,
        '2' => 19,
        '3' => 20,
        '4' => 21,
        '6' => 22,
        '5' => 23,
        '=' => 24,
        '9' => 25,
        '7' => 26,
        '-' => 27,
        '8' => 28,
        '0' => 29,
        ']' => 30,
        'o' => 31,
        'u' => 32,
        '[' => 33,
        'i' => 34,
        'p' => 35,
        'l' => 37,
        'j' => 38,
        '\'' => 39,
        'k' => 40,
        ';' => 41,
        '\\' => 42,
        ',' => 43,
        '/' => 44,
        'n' => 45,
        'm' => 46,
        '.' => 47,
        '`' => 50,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use platform::*;

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::collections::HashMap;
    use std::ptr::{null, NonNull};
    use std::sync::mpsc;
    use std::sync::OnceLock;
    use std::time::Duration;

    use block2::RcBlock;
    #[allow(unused_imports)]
    use objc2::rc::Retained;
    use objc2::AllocAnyThread;
    use objc2_app_kit::{
        NSApplicationActivationOptions, NSApplicationActivationPolicy, NSRunningApplication,
        NSWorkspace, NSWorkspaceOpenConfiguration,
    };
    use objc2_application_services::{
        kAXTrustedCheckOptionPrompt, AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
        AXUIElement, AXValue, AXValueType,
    };
    use objc2_core_foundation::{
        CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect,
        CGSize,
    };
    use objc2_core_graphics::{
        CGDataProvider, CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID,
        CGEventType, CGImage, CGMouseButton, CGPreflightScreenCaptureAccess,
        CGRequestScreenCaptureAccess, CGScrollEventUnit,
    };
    use objc2_foundation::{NSError, NSString, NSURL};
    use objc2_screen_capture_kit::{
        SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
    };

    pub fn permissions() -> Permissions {
        Permissions {
            supported: true,
            accessibility: unsafe { AXIsProcessTrusted() },
            screen_recording: CGPreflightScreenCaptureAccess(),
        }
    }

    /// Shows the system Accessibility prompt (once per process per macOS rules).
    pub fn request_accessibility() -> bool {
        let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
        let value: &CFBoolean = CFBoolean::new(true);
        let options = CFDictionary::from_slices(&[key], &[value]);
        unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) }
    }

    pub fn request_screen_recording() -> bool {
        CGRequestScreenCaptureAccess()
    }

    /// Opens a fixed Privacy & Security pane; never a renderer-supplied URL.
    pub fn open_privacy_pane(screen_recording: bool) {
        let url = if screen_recording {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        } else {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        };
        if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    }

    /// Seconds since the person last used the keyboard, mouse or trackpad.
    pub fn idle_seconds() -> f64 {
        CGEventSource::seconds_since_last_event_type(
            CGEventSourceStateID::HIDSystemState,
            CGEventType(u32::MAX),
        )
    }

    fn info_of(app: &NSRunningApplication) -> AppInfo {
        AppInfo {
            pid: app.processIdentifier(),
            bundle_id: app
                .bundleIdentifier()
                .map(|id| id.to_string())
                .unwrap_or_default(),
            name: app
                .localizedName()
                .map(|name| name.to_string())
                .unwrap_or_default(),
            active: app.isActive(),
        }
    }

    fn all_pids() -> Vec<i32> {
        // `proc_listallpids` returns how many pids it wrote (its header says bytes). Grow the
        // buffer until it is not full, so a busy machine is never cut short.
        let mut capacity = 1024usize;
        loop {
            let mut pids = vec![0i32; capacity];
            // SAFETY: the buffer holds `capacity` pids, `capacity * 4` bytes.
            let filled = unsafe {
                libc::proc_listallpids(pids.as_mut_ptr().cast(), (capacity * 4) as libc::c_int)
            };
            if filled <= 0 {
                return Vec::new();
            }
            let filled = filled as usize;
            if filled < capacity || capacity >= 1 << 20 {
                pids.truncate(filled);
                pids.retain(|pid| *pid > 0);
                return pids;
            }
            capacity *= 4;
        }
    }

    /// Regular apps that are running now. `NSWorkspace.runningApplications` is a cache
    /// refreshed by main-queue notifications, so it goes stale (it never lists apps
    /// started after launch) when the process has no AppKit run loop, as in `sirus-host`.
    /// `NSRunningApplication` lookups by pid or bundle id ask LaunchServices directly, so
    /// the list is built from the live process table instead.
    pub fn running_apps() -> Vec<AppInfo> {
        let own = std::process::id() as i32;
        all_pids()
            .into_iter()
            .filter(|pid| *pid != own)
            .filter_map(NSRunningApplication::runningApplicationWithProcessIdentifier)
            .filter(|app| app.activationPolicy() == NSApplicationActivationPolicy::Regular)
            .map(|app| info_of(&app))
            .filter(|info| !info.bundle_id.is_empty())
            .collect()
    }

    /// The app's file name without `.app` ("Calculator" for "Calculadora"), a second name
    /// to match when the localized one differs from what the agent typed.
    pub fn app_file_name(pid: i32) -> Option<String> {
        let url =
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?.bundleURL()?;
        let name = url.lastPathComponent()?.to_string();
        Some(name.strip_suffix(".app").unwrap_or(&name).to_string())
    }

    /// The live app for a bundle id, regardless of any run loop.
    pub fn app_by_bundle(bundle_id: &str) -> Option<AppInfo> {
        let own = std::process::id() as i32;
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
            bundle_id,
        ))
        .iter()
        .filter(|app| app.processIdentifier() != own && !app.isTerminated())
        .map(|app| info_of(&app))
        .next()
    }

    /// Launches by bundle id without activating it, then waits for the process.
    /// The name Finder shows for an installed app, in the person's language: "Calculadora".
    pub fn installed_name(bundle_id: &str) -> Option<String> {
        let url = NSWorkspace::sharedWorkspace()
            .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
        let path = url.path()?;
        let name = objc2_foundation::NSFileManager::defaultManager()
            .displayNameAtPath(&path)
            .to_string();
        let name = name.strip_suffix(".app").unwrap_or(&name);
        (!name.is_empty()).then(|| name.to_owned())
    }

    pub fn launch(bundle_id: &str) -> Result<AppInfo, String> {
        let workspace = NSWorkspace::sharedWorkspace();
        let url = workspace
            .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))
            .ok_or_else(|| "that application is not installed".to_string())?;
        let configuration = NSWorkspaceOpenConfiguration::configuration();
        configuration.setActivates(false);
        // The completion's app is authoritative; `runningApplications` only refreshes on the main run loop.
        let (tx, rx) = mpsc::channel::<Option<AppInfo>>();
        let block = RcBlock::new(
            move |app: *mut NSRunningApplication, _error: *mut NSError| {
                let info = unsafe { app.as_ref() }.map(|app| AppInfo {
                    pid: app.processIdentifier(),
                    bundle_id: app
                        .bundleIdentifier()
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    name: app
                        .localizedName()
                        .map(|name| name.to_string())
                        .unwrap_or_default(),
                    active: false,
                });
                let _ = tx.send(info);
            },
        );
        workspace.openApplicationAtURL_configuration_completionHandler(
            &url,
            &configuration,
            Some(&block),
        );
        let launched = rx
            .recv_timeout(Duration::from_secs(15))
            .ok()
            .flatten()
            .filter(|app| app.pid > 0)
            .ok_or_else(|| "the application did not start".to_string())?;
        // Wait (bounded) until the system lists it, so the next tool call can find it.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(live) =
                NSRunningApplication::runningApplicationWithProcessIdentifier(launched.pid)
                    .filter(|app| !app.isTerminated())
            {
                return Ok(AppInfo {
                    active: false,
                    ..info_of(&live)
                });
            }
            if std::time::Instant::now() >= deadline {
                return Err("the application started but is not listed yet".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Brings an app to the front. Only used for explicit foreground requests.
    pub fn activate(pid: i32) -> Result<(), String> {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .ok_or_else(|| "the application is not running".to_string())?;
        #[allow(deprecated)]
        let activated = app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
        if activated {
            Ok(())
        } else {
            Err("the application could not be brought to the front".into())
        }
    }

    // ---------- Accessibility worker ----------

    struct Snapshot {
        pid: i32,
        origin: (f64, f64),
        elements: Vec<CFRetained<AXUIElement>>,
    }

    #[derive(Default)]
    pub struct Driver {
        snapshots: HashMap<String, Snapshot>,
    }

    type Job = Box<dyn FnOnce(&mut Driver) + Send>;
    static WORKER: OnceLock<parking_lot::Mutex<mpsc::Sender<Job>>> = OnceLock::new();

    fn worker() -> &'static parking_lot::Mutex<mpsc::Sender<Job>> {
        WORKER.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            std::thread::Builder::new()
                .name("sirus-computer".into())
                .spawn(move || {
                    let mut driver = Driver::default();
                    while let Ok(job) = rx.recv() {
                        job(&mut driver);
                    }
                })
                .expect("computer worker thread");
            parking_lot::Mutex::new(tx)
        })
    }

    /// Runs on the Accessibility worker and waits for the result (bounded).
    pub fn with_driver<T: Send + 'static>(
        job: impl FnOnce(&mut Driver) -> T + Send + 'static,
    ) -> Result<T, String> {
        let (tx, rx) = mpsc::channel();
        worker()
            .lock()
            .send(Box::new(move |driver| {
                let _ = tx.send(job(driver));
            }))
            .map_err(|_| "computer worker unavailable".to_string())?;
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|_| "the application did not respond in time".to_string())
    }

    fn cfstr(text: &str) -> CFRetained<CFString> {
        CFString::from_str(text)
    }

    fn ax_error(error: AXError) -> String {
        match error {
            AXError::APIDisabled => "Accessibility permission is not granted to Sirus Code".into(),
            AXError::CannotComplete => "the application did not respond".into(),
            AXError::ActionUnsupported => "that element does not support this action".into(),
            AXError::AttributeUnsupported => "that element does not support this value".into(),
            AXError::InvalidUIElement => "that element no longer exists; observe again".into(),
            _ => format!("Accessibility error {}", error.0),
        }
    }

    fn attribute(element: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
        let mut value: *const CFType = null();
        let error =
            unsafe { element.copy_attribute_value(&cfstr(name), NonNull::from(&mut value)) };
        if error != AXError::Success {
            return None;
        }
        NonNull::new(value as *mut CFType).map(|value| unsafe { CFRetained::from_raw(value) })
    }

    fn string_of(value: &CFType) -> Option<String> {
        if let Some(text) = value.downcast_ref::<CFString>() {
            return Some(text.to_string());
        }
        if let Some(number) = value.downcast_ref::<CFNumber>() {
            return number.as_f64().map(|n| {
                if n.fract() == 0.0 {
                    format!("{}", n as i64)
                } else {
                    format!("{n}")
                }
            });
        }
        if let Some(flag) = value.downcast_ref::<CFBoolean>() {
            return Some(flag.as_bool().to_string());
        }
        None
    }

    fn point_of(value: &CFType) -> Option<(f64, f64)> {
        let value = value.downcast_ref::<AXValue>()?;
        let mut point = CGPoint::new(0.0, 0.0);
        unsafe { value.value(AXValueType::CGPoint, NonNull::from(&mut point).cast()) }
            .then_some((point.x, point.y))
    }

    fn size_of(value: &CFType) -> Option<(f64, f64)> {
        let value = value.downcast_ref::<AXValue>()?;
        let mut size = CGSize::new(0.0, 0.0);
        unsafe { value.value(AXValueType::CGSize, NonNull::from(&mut size).cast()) }
            .then_some((size.width, size.height))
    }

    fn frame_of(element: &AXUIElement) -> Option<Frame> {
        let (x, y) = point_of(&*attribute(element, "AXPosition")?)?;
        let (width, height) = size_of(&*attribute(element, "AXSize")?)?;
        Some(Frame {
            x,
            y,
            width,
            height,
        })
    }

    fn elements_of(value: &CFType) -> Vec<CFRetained<AXUIElement>> {
        let Some(array) = value.downcast_ref::<CFArray>() else {
            return Vec::new();
        };
        (0..array.count())
            .filter_map(|index| {
                let item = unsafe { array.value_at_index(index) } as *mut CFType;
                let item = NonNull::new(item)?;
                let item = unsafe { CFRetained::retain(item) };
                item.downcast::<AXUIElement>().ok()
            })
            .collect()
    }

    fn application(pid: i32) -> CFRetained<AXUIElement> {
        let app = unsafe { AXUIElement::new_application(pid) };
        unsafe { app.set_messaging_timeout(2.0) };
        app
    }

    fn main_window(pid: i32, title: Option<&str>) -> Result<CFRetained<AXUIElement>, String> {
        if !unsafe { AXIsProcessTrusted() } {
            return Err(
                "Accessibility permission is not granted to Sirus Code. Open Settings → Computer."
                    .into(),
            );
        }
        let app = application(pid);
        let windows = attribute(&app, "AXWindows")
            .map(|value| elements_of(&value))
            .unwrap_or_default();
        if let Some(title) = title.filter(|title| !title.is_empty()) {
            let needle = title.to_lowercase();
            if let Some(window) = windows.iter().find(|window| {
                attribute(window, "AXTitle")
                    .and_then(|value| string_of(&value))
                    .is_some_and(|text| text.to_lowercase().contains(&needle))
            }) {
                return Ok(window.clone());
            }
        }
        for name in ["AXFocusedWindow", "AXMainWindow"] {
            if let Some(window) =
                attribute(&app, name).and_then(|value| value.downcast::<AXUIElement>().ok())
            {
                return Ok(window);
            }
        }
        windows.into_iter().next().ok_or_else(|| {
            "the application has no open window; launch or open one first".to_string()
        })
    }

    impl Driver {
        /// Bounded breadth-first read of one window; refs stay valid until the next observe.
        pub fn observe(
            &mut self,
            session: &str,
            app: &AppInfo,
            window_title: Option<&str>,
            limit: usize,
        ) -> Result<Observation, String> {
            let window = main_window(app.pid, window_title)?;
            let window_frame = frame_of(&window).unwrap_or_default();
            let title = attribute(&window, "AXTitle")
                .and_then(|value| string_of(&value))
                .unwrap_or_default();
            let mut queue = std::collections::VecDeque::from([(window.clone(), 0usize)]);
            let mut elements = Vec::new();
            let mut retained = Vec::new();
            let mut visited = 0usize;
            let mut truncated = false;
            while let Some((node, depth)) = queue.pop_front() {
                visited += 1;
                if visited > 6000 {
                    truncated = true;
                    break;
                }
                let role = attribute(&node, "AXRole")
                    .and_then(|value| string_of(&value))
                    .unwrap_or_default();
                if ROLES.contains(&role.as_str()) {
                    let subrole = attribute(&node, "AXSubrole")
                        .and_then(|value| string_of(&value))
                        .unwrap_or_default();
                    let secure = subrole == "AXSecureTextField" || role == "AXSecureTextField";
                    let label = ["AXTitle", "AXDescription", "AXPlaceholderValue", "AXHelp"]
                        .iter()
                        .find_map(|name| {
                            attribute(&node, name)
                                .and_then(|value| string_of(&value))
                                .filter(|text| !text.trim().is_empty())
                        })
                        .map(|text| clip(&text))
                        .unwrap_or_default();
                    // Password fields never expose their contents.
                    let value = if secure {
                        None
                    } else {
                        attribute(&node, "AXValue")
                            .and_then(|value| string_of(&value))
                            .map(|text| clip(&text))
                            .filter(|text| !text.is_empty())
                    };
                    let keep = !(matches!(
                        role.as_str(),
                        "AXStaticText" | "AXHeading" | "AXImage" | "AXRow" | "AXCell"
                    ) && label.is_empty()
                        && value.is_none());
                    if keep {
                        if elements.len() >= limit {
                            truncated = true;
                            break;
                        }
                        let frame = frame_of(&node).unwrap_or_default();
                        if frame.width > 0.0 && frame.height > 0.0 {
                            let disabled = attribute(&node, "AXEnabled")
                                .and_then(|value| value.downcast::<CFBoolean>().ok())
                                .is_some_and(|flag| !flag.as_bool());
                            let focused = attribute(&node, "AXFocused")
                                .and_then(|value| value.downcast::<CFBoolean>().ok())
                                .is_some_and(|flag| flag.as_bool());
                            elements.push(Element {
                                reference: format!("e{}", elements.len() + 1),
                                role: role.trim_start_matches("AX").to_string(),
                                label,
                                value,
                                disabled,
                                focused,
                                frame: Frame {
                                    x: frame.x - window_frame.x,
                                    y: frame.y - window_frame.y,
                                    width: frame.width,
                                    height: frame.height,
                                },
                            });
                            retained.push(node.clone());
                        }
                    }
                }
                if depth < 40 {
                    if let Some(children) = attribute(&node, "AXChildren") {
                        for child in elements_of(&children) {
                            queue.push_back((child, depth + 1));
                        }
                    }
                }
            }
            self.snapshots.insert(
                session.to_string(),
                Snapshot {
                    pid: app.pid,
                    origin: (window_frame.x, window_frame.y),
                    elements: retained,
                },
            );
            Ok(Observation {
                app: app.name.clone(),
                window: title,
                window_frame,
                elements,
                truncated,
            })
        }

        fn element(
            &self,
            session: &str,
            pid: i32,
            reference: &str,
        ) -> Result<CFRetained<AXUIElement>, String> {
            let snapshot = self
                .snapshots
                .get(session)
                .filter(|snapshot| snapshot.pid == pid)
                .ok_or_else(|| "observe the application first to get element refs".to_string())?;
            let index = reference
                .strip_prefix('e')
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|n| *n >= 1)
                .ok_or_else(|| "invalid element ref".to_string())?;
            snapshot
                .elements
                .get(index - 1)
                .cloned()
                .ok_or_else(|| "unknown element ref; observe again".to_string())
        }

        /// Global point for a window-relative point from the last observation.
        pub fn global_point(
            &self,
            session: &str,
            pid: i32,
            x: f64,
            y: f64,
        ) -> Result<(f64, f64), String> {
            let snapshot = self
                .snapshots
                .get(session)
                .filter(|snapshot| snapshot.pid == pid)
                .ok_or_else(|| {
                    "observe the application first; coordinates are window-relative".to_string()
                })?;
            Ok((snapshot.origin.0 + x, snapshot.origin.1 + y))
        }

        pub fn window_frame(&self, pid: i32) -> Option<Frame> {
            main_window(pid, None)
                .ok()
                .and_then(|window| frame_of(&window))
        }

        /// Prefers the semantic AXPress; falls back to a per-process click at its center.
        pub fn press(
            &self,
            session: &str,
            pid: i32,
            reference: &str,
            count: u32,
            button: MouseButton,
        ) -> Result<&'static str, String> {
            let element = self.element(session, pid, reference)?;
            if count == 1 && button == MouseButton::Left {
                let error = unsafe { element.perform_action(&cfstr("AXPress")) };
                if error == AXError::Success {
                    return Ok("pressed");
                }
                if error == AXError::InvalidUIElement {
                    return Err(ax_error(error));
                }
            }
            let frame = frame_of(&element)
                .ok_or_else(|| "that element has no position on screen".to_string())?;
            let (x, y) = frame.center();
            click_at(pid, x, y, count, button);
            Ok("clicked")
        }

        pub fn focus(&self, session: &str, pid: i32, reference: &str) -> Result<(), String> {
            let element = self.element(session, pid, reference)?;
            let error =
                unsafe { element.set_attribute_value(&cfstr("AXFocused"), CFBoolean::new(true)) };
            if error == AXError::Success {
                Ok(())
            } else {
                Err(ax_error(error))
            }
        }

        pub fn set_value(
            &self,
            session: &str,
            pid: i32,
            reference: &str,
            text: &str,
        ) -> Result<(), String> {
            let element = self.element(session, pid, reference)?;
            let error = unsafe { element.set_attribute_value(&cfstr("AXValue"), &cfstr(text)) };
            if error == AXError::Success {
                Ok(())
            } else {
                Err(ax_error(error))
            }
        }

        pub fn element_center(
            &self,
            session: &str,
            pid: i32,
            reference: &str,
        ) -> Result<(f64, f64), String> {
            let element = self.element(session, pid, reference)?;
            frame_of(&element)
                .map(|frame| frame.center())
                .ok_or_else(|| "that element has no position on screen".to_string())
        }
    }

    // ---------- Per-process input ----------

    fn post(pid: i32, event: Option<CFRetained<CGEvent>>) {
        CGEvent::post_to_pid(pid, event.as_deref());
        std::thread::sleep(Duration::from_millis(4));
    }

    pub fn click_at(pid: i32, x: f64, y: f64, count: u32, button: MouseButton) {
        let point = CGPoint::new(x, y);
        let (down, up, cg_button) = match button {
            MouseButton::Left => (
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGMouseButton::Left,
            ),
            MouseButton::Right => (
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGMouseButton::Right,
            ),
        };
        for click in 1..=count.clamp(1, 3) {
            for kind in [down, up] {
                let event = CGEvent::new_mouse_event(None, kind, point, cg_button);
                CGEvent::set_integer_value_field(
                    event.as_deref(),
                    CGEventField::MouseEventClickState,
                    click as i64,
                );
                post(pid, event);
            }
        }
    }

    pub fn modifier_flags(modifiers: &[String]) -> Result<CGEventFlags, String> {
        let mut flags = CGEventFlags(0);
        for modifier in modifiers {
            flags.0 |= match modifier.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "meta" => CGEventFlags::MaskCommand.0,
                "shift" => CGEventFlags::MaskShift.0,
                "option" | "alt" => CGEventFlags::MaskAlternate.0,
                "control" | "ctrl" => CGEventFlags::MaskControl.0,
                _ => return Err(format!("unknown modifier {modifier}")),
            };
        }
        Ok(flags)
    }

    pub fn press_key(pid: i32, code: u16, flags: CGEventFlags) {
        for down in [true, false] {
            let event = CGEvent::new_keyboard_event(None, code, down);
            CGEvent::set_flags(event.as_deref(), flags);
            post(pid, event);
        }
    }

    /// Sends text as Unicode key events to the app's focused element.
    pub fn type_text(pid: i32, text: &str) {
        let units: Vec<u16> = text.encode_utf16().collect();
        for chunk in units.chunks(16) {
            for down in [true, false] {
                let event = CGEvent::new_keyboard_event(None, 0, down);
                unsafe {
                    CGEvent::keyboard_set_unicode_string(
                        event.as_deref(),
                        chunk.len() as _,
                        chunk.as_ptr(),
                    )
                };
                post(pid, event);
            }
        }
    }

    pub fn scroll(pid: i32, x: f64, y: f64, dx: i32, dy: i32) {
        let event =
            CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Pixel, 2, -dy, -dx, 0);
        CGEvent::set_location(event.as_deref(), CGPoint::new(x, y));
        post(pid, event);
    }

    // ---------- Screenshots ----------

    pub struct Image {
        pub jpeg: Vec<u8>,
        pub width: u32,
        pub height: u32,
    }

    pub(crate) fn encode(image: &CGImage) -> Option<Image> {
        let width = CGImage::width(Some(image));
        let height = CGImage::height(Some(image));
        let row = CGImage::bytes_per_row(Some(image));
        if width == 0 || height == 0 || CGImage::bits_per_pixel(Some(image)) != 32 {
            return None;
        }
        let provider = CGImage::data_provider(Some(image))?;
        let data = CGDataProvider::data(Some(&provider))?;
        let bytes = data.to_vec();
        let mut rgb = Vec::with_capacity(width * height * 3);
        for line in 0..height {
            let start = line * row;
            let pixels = bytes.get(start..start + width * 4)?;
            for pixel in pixels.as_chunks::<4>().0 {
                // ScreenCaptureKit delivers BGRA.
                rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            }
        }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 72)
            .encode(
                &rgb,
                width as u32,
                height as u32,
                image::ExtendedColorType::Rgb8,
            )
            .ok()?;
        Some(Image {
            jpeg,
            width: width as u32,
            height: height as u32,
        })
    }

    fn distance(window: &SCWindow, frame: Frame) -> f64 {
        let rect: CGRect = unsafe { window.frame() };
        (rect.origin.x - frame.x).abs()
            + (rect.origin.y - frame.y).abs()
            + (rect.size.width - frame.width).abs()
            + (rect.size.height - frame.height).abs()
    }

    /// Captures the app window that best matches `frame`, longest side ≤ `max_side` pixels.
    pub fn screenshot(pid: i32, frame: Frame, max_side: f64) -> Result<Image, String> {
        if !CGPreflightScreenCaptureAccess() {
            return Err("Screen Recording permission is not granted to Sirus Code. Open Settings → Computer.".into());
        }
        let (tx, rx) = mpsc::channel::<Result<Image, String>>();
        let handler = RcBlock::new(
            move |content: *mut SCShareableContent, _error: *mut NSError| {
                let Some(content) = (unsafe { content.as_ref() }) else {
                    let _ = tx.send(Err("screen capture is unavailable".into()));
                    return;
                };
                let windows = unsafe { content.windows() };
                let best = windows
                    .iter()
                    .filter(|window| {
                        unsafe { window.owningApplication() }
                            .is_some_and(|app| unsafe { app.processID() } == pid)
                            && unsafe { window.windowLayer() } == 0
                    })
                    .min_by(|a, b| distance(a, frame).total_cmp(&distance(b, frame)));
                let Some(window) = best else {
                    let _ = tx.send(Err("no capturable window for that application".into()));
                    return;
                };
                let rect: CGRect = unsafe { window.frame() };
                let scale = (max_side / rect.size.width.max(rect.size.height).max(1.0)).min(2.0);
                let filter = unsafe {
                    SCContentFilter::initWithDesktopIndependentWindow(
                        SCContentFilter::alloc(),
                        &window,
                    )
                };
                let configuration = unsafe { SCStreamConfiguration::new() };
                unsafe {
                    configuration.setWidth((rect.size.width * scale).round().max(1.0) as usize);
                    configuration.setHeight((rect.size.height * scale).round().max(1.0) as usize);
                    configuration.setShowsCursor(false);
                    configuration.setIgnoreShadowsSingleWindow(true);
                }
                let tx = tx.clone();
                let done = RcBlock::new(move |image: *mut CGImage, _error: *mut NSError| {
                    let result = unsafe { image.as_ref() }
                        .and_then(encode)
                        .ok_or_else(|| "the window could not be captured".to_string());
                    let _ = tx.send(result);
                });
                unsafe {
                    SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                        &filter,
                        &configuration,
                        Some(&done),
                    )
                };
            },
        );
        unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| "screen capture timed out".to_string())?
    }
}

#[cfg(not(target_os = "macos"))]
mod fallback {
    use super::*;
    pub fn permissions() -> Permissions {
        Permissions {
            supported: false,
            accessibility: false,
            screen_recording: false,
        }
    }
}
#[cfg(not(target_os = "macos"))]
pub use fallback::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_codes_cover_named_and_printable_keys() {
        assert_eq!(key_code("Return"), Some(36));
        assert_eq!(key_code("esc"), Some(53));
        assert_eq!(key_code("a"), Some(0));
        assert_eq!(key_code("A"), Some(0));
        assert_eq!(key_code("9"), Some(25));
        assert_eq!(key_code("f12"), Some(111));
        assert_eq!(key_code("hello"), None);
        assert_eq!(key_code("ç"), None);
    }

    /// Live check of the app list without a run loop (the `sirus-host` case):
    /// `cargo test live_app_lookup -- --ignored --nocapture`. Quits Calculator first.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn live_app_lookup() {
        let _ = std::process::Command::new("pkill")
            .args(["-x", "Calculator"])
            .status();
        std::thread::sleep(std::time::Duration::from_millis(1000));
        assert!(app_by_bundle("com.apple.calculator").is_none());
        let launched = launch("com.apple.calculator").expect("launch");
        let listed = running_apps();
        println!("launched {launched:?}, listed {listed:?}");
        assert!(listed
            .iter()
            .any(|app| app.bundle_id == "com.apple.calculator"));
        assert_eq!(
            app_by_bundle("com.apple.calculator").map(|app| app.pid),
            Some(launched.pid)
        );
        let _ = std::process::Command::new("pkill")
            .args(["-x", "Calculator"])
            .status();
    }

    /// Live check against Calculator: `cargo test live_calculator -- --ignored --nocapture`.
    /// Needs Accessibility (and Screen Recording for the screenshot) for the process running tests.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn live_calculator() {
        // ScreenCaptureKit needs the window-server connection the app's NSApplication
        // owns. Run with `--test-threads=1` so the test is on the main thread.
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            let _ = objc2_app_kit::NSApplication::sharedApplication(mtm);
        }
        extern "C" {
            fn CGMainDisplayID() -> u32;
        }
        // Opens the window-server connection when the test is not on the main thread.
        let _ = unsafe { CGMainDisplayID() };
        let started = std::time::Instant::now();
        println!("permissions: {:?}", permissions());
        let app = running_apps()
            .into_iter()
            .find(|app| app.bundle_id == "com.apple.calculator")
            .unwrap_or_else(|| launch("com.apple.calculator").expect("launch"));
        std::thread::sleep(std::time::Duration::from_millis(800));
        let target = app.clone();
        let observed = with_driver(move |driver| driver.observe("live", &target, None, 200))
            .unwrap()
            .unwrap();
        println!(
            "buttons: {:?}",
            observed
                .elements
                .iter()
                .filter(|element| element.role == "Button")
                .map(|element| element.label.as_str())
                .collect::<Vec<_>>()
        );
        println!(
            "observe: {} elements in {:?}; window {:?}",
            observed.elements.len(),
            started.elapsed(),
            observed.window_frame
        );
        let find = |label: &str| {
            observed
                .elements
                .iter()
                .find(|element| {
                    element.role == "Button" && element.label.eq_ignore_ascii_case(label)
                })
                .map(|element| element.reference.clone())
        };
        // English or Portuguese (pt-BR) Calculator labels.
        fn localized(label: &str) -> &str {
            match label {
                "All Clear" => "Limpar Tudo",
                "Add" => "Adicionar",
                "Equals" => "Igual a",
                other => other,
            }
        }
        for label in ["All Clear", "7", "Add", "8", "Equals"] {
            let Some(reference) = find(label).or_else(|| find(localized(label))).or_else(|| {
                if label == "All Clear" {
                    find("Clear")
                } else {
                    None
                }
            }) else {
                println!("no button {label}");
                continue;
            };
            let pid = app.pid;
            let pressed = with_driver(move |driver| {
                driver.press("live", pid, &reference, 1, MouseButton::Left)
            })
            .unwrap();
            println!("{label}: {pressed:?}");
        }
        let target = app.clone();
        let after = with_driver(move |driver| driver.observe("live", &target, None, 200))
            .unwrap()
            .unwrap();
        let display: Vec<_> = after
            .elements
            .iter()
            .filter(|element| element.role == "StaticText")
            .map(|element| element.value.clone().unwrap_or_default())
            .collect();
        println!("display texts: {display:?}");
        match screenshot(app.pid, after.window_frame, 1280.0) {
            Ok(image) => println!(
                "screenshot {}x{} jpeg {} bytes",
                image.width,
                image.height,
                image.jpeg.len()
            ),
            Err(error) => println!("screenshot: {error}"),
        }
        println!("total {:?}", started.elapsed());
    }

    #[test]
    fn clipped_text_is_single_line_and_bounded() {
        assert_eq!(clip("  a\nb  "), "a b");
        let long = "x".repeat(400);
        assert!(clip(&long).chars().count() <= TEXT_LIMIT + 1);
    }
}
