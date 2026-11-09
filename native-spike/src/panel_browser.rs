//! The Navegador panel, after Sirus's `BrowserPanel.tsx`: tabs, a toolbar (back, forward,
//! reload, address, saved logins, copy link, screenshot, element picker, device mode, more menu),
//! the login offer bar and the page's JavaScript dialogs, over native WKWebViews parented to the
//! GPUI window's content view (technique from MIT-licensed code). The web view paints above
//! GPUI, so it hides whenever the panel isn't painted in a frame, while an overlay is open
//! (`set_covered`) and while one of the panel's own menus is.
//!
//! One `Browser` exists per conversation (`entity_for`): it keeps its tabs whether or not the panel
//! is shown, so an agent's browser tools (browser_bridge.rs) work with the panel closed. Page
//! scripts and result shapes are Sirus's (browser_js.rs).
use crate::backend::Backend;
use crate::browser_address::{self, LocalServer, Suggestion};
use crate::browser_crop;
use crate::browser_device::{self, Edge};
use crate::browser_js;
use crate::browser_pip::Pip;
use crate::browser_profiles::{self, Profile};
use crate::browser_record;
use crate::browser_tab::{
    self, Answer, Events, PendingDialog, Queue, TabDelegate, TabEvent, TabObserver,
};
use crate::i18n::{t, tf};
use crate::panel_files::{TextField, TextFieldEvent};
use crate::panels::PanelContext;
use crate::theme;
use crate::theme::Sinks as _;
use gpui::StyledImage as _;
use gpui::{
    Animation, AnimationExt, AnyElement, AnyView, App, Bounds, ClipboardItem, Context,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, FutureExt as _, KeyDownEvent,
    MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, SharedString, Subscription,
    Task, Transformation, UTF16Selection, Window, canvas, div, percentage, prelude::*, px,
};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{NSView, NSWindowOrderingMode};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_web_kit::{WKWebView, WKWebsiteDataStore};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ops::Range,
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

const TOOLBAR: f32 = 36.;
/// The tabs' row over the address.
const TAB_STRIP: f32 = 34.;
const MAX_TABS: usize = 8;
const GUTTER: f64 = 12.;
/// What the page leaves at its left edge: a native view takes every click over it, so the side
/// panel's resize grip needs ground of its own.
const GRIP_ROOM: f64 = 6.;
const NOTICE: Duration = Duration::from_millis(1800);
/// A press and its click on a menu's button, as one gesture.
const MENU_PRESS: Duration = Duration::from_millis(800);
/// The address's tools, most used first: the narrower the panel, the fewer stay in it, and the
/// rest wait in the `⋯` menu.
const PICK_RANK: usize = 0;
const SHOT_RANK: usize = 1;
const DEVICE_RANK: usize = 2;
const LINK_RANK: usize = 3;
const RECORD_RANK: usize = 4;
const PIP_RANK: usize = 5;
const TOOLS: usize = 6;

/// How many tools fit in a panel this wide. Until it is measured, the usual three.
fn tools_that_fit(width: f32) -> usize {
    if width <= 0. {
        return 3;
    }
    (((width - 340.) / 26.).max(0.) as usize).min(TOOLS)
}

/// How long a menu waits for the page's picture before covering the page anyway.
const FREEZE_LIMIT: Duration = Duration::from_millis(400);

/// Chrome DevTools' device list, as `DEVICE_PRESETS` in src-tauri/src/browser.rs.
pub const PRESETS: [(&str, &str, u32, u32); 17] = [
    ("iphone-se", "iPhone SE", 375, 667),
    ("iphone-xr", "iPhone XR", 414, 896),
    ("iphone-12-pro", "iPhone 12 Pro", 390, 844),
    ("iphone-14-pro-max", "iPhone 14 Pro Max", 430, 932),
    ("pixel-7", "Pixel 7", 412, 915),
    ("galaxy-s8-plus", "Samsung Galaxy S8+", 360, 740),
    ("galaxy-s20-ultra", "Samsung Galaxy S20 Ultra", 412, 915),
    ("ipad-mini", "iPad Mini", 768, 1024),
    ("ipad-air", "iPad Air", 820, 1180),
    ("ipad-pro", "iPad Pro", 1024, 1366),
    ("surface-pro-7", "Surface Pro 7", 912, 1368),
    ("surface-duo", "Surface Duo", 540, 720),
    ("galaxy-z-fold-5", "Galaxy Z Fold 5", 344, 882),
    ("zenbook-fold", "Asus Zenbook Fold", 853, 1280),
    ("galaxy-a51-71", "Samsung Galaxy A51/71", 412, 914),
    ("nest-hub", "Nest Hub", 1024, 600),
    ("nest-hub-max", "Nest Hub Max", 1280, 800),
];

thread_local! {
    static COVERED: Cell<bool> = const { Cell::new(false) };
    static GRIP_SHARE: Cell<f64> = const { Cell::new(1.) };
    static BOTTOM_ROOM: Cell<f64> = const { Cell::new(0.) };
    static LIVE: RefCell<Vec<Weak<Shared>>> = const { RefCell::new(Vec::new()) };
    static SESSIONS: RefCell<HashMap<String, Entity<Browser>>> = RefCell::new(HashMap::new());
    /// The custom profiles and the profile new tabs open in, from the settings.
    static PROFILES: RefCell<(Vec<Profile>, String)> =
        RefCell::new((Vec::new(), browser_profiles::DEFAULT.to_owned()));
}

/// Takes the browser profiles from the settings (when they load and when they are saved).
pub fn note_settings(settings: &Value) {
    PROFILES.with(|p| *p.borrow_mut() = browser_profiles::from_settings(settings));
}

fn profiles() -> Vec<Profile> {
    PROFILES.with(|p| p.borrow().0.clone())
}

/// The profile a new tab opens in, unless another is picked from the + menu.
fn default_profile() -> String {
    PROFILES.with(|p| p.borrow().1.clone())
}

/// How much of the resize grip's ground the page leaves free: a maximized panel has no edge
/// to drag, so the page reaches the card's side.
pub fn set_grip_share(share: f64) {
    GRIP_SHARE.with(|grip| grip.set(share));
}

/// A strip at the page's foot left free for something drawn over it (the conversation balloon).
pub fn set_bottom_room(room: f32) {
    BOTTOM_ROOM.with(|r| r.set(f64::from(room)));
}

/// Hides every browser web view while a menu, dialog or other overlay is open (native views
/// draw above GPUI), and lets them back when it closes.
pub fn set_covered(covered: bool) {
    COVERED.with(|c| c.set(covered));
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        live.retain(|w| w.strong_count() > 0);
        for shared in live.iter().filter_map(Weak::upgrade) {
            shared.apply();
        }
    });
}

pub fn new(context: PanelContext, cx: &mut App) -> AnyView {
    entity_for(&context.session_id, &context.backend, cx).into()
}

/// The conversation's browser, created on first use.
pub fn entity_for(session_id: &str, backend: &Backend, cx: &mut App) -> Entity<Browser> {
    if let Some(found) = SESSIONS.with(|s| s.borrow().get(session_id).cloned()) {
        return found;
    }
    let (id, backend) = (session_id.to_owned(), backend.clone());
    let entity = cx.new(|cx| Browser::new(id, backend, cx));
    SESSIONS.with(|s| s.borrow_mut().insert(session_id.to_owned(), entity.clone()));
    entity
}

/// The window's content view, which the web views are parented to.
pub fn parent_of(window: &Window) -> Option<Retained<NSView>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // GPUI keeps its AppKit content view alive for the window's lifetime; we retain our own.
    unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }
}

/// Icons of the panel (drawn like the rest: 24 px grid, white stroke).
pub fn asset(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "browser-camera" => include_bytes!("../assets/browser-camera.svg"),
        "browser-more" => include_bytes!("../assets/browser-more.svg"),
        "browser-sun" => include_bytes!("../assets/browser-sun.svg"),
        "browser-moon" => include_bytes!("../assets/browser-moon.svg"),
        "browser-link" => include_bytes!("../assets/browser-link.svg"),
        "browser-rotate" => include_bytes!("../assets/browser-rotate.svg"),
        "browser-lock" => include_bytes!("../assets/browser-lock.svg"),
        "browser-record" => include_bytes!("../assets/browser-record.svg"),
        "browser-pip" => include_bytes!("../assets/browser-pip.svg"),
        _ => return None,
    })
}

#[derive(Clone, Copy, Default)]
struct Layout {
    /// The device frame's CSS size in device mode.
    device: Option<(f64, f64)>,
    user_zoom: f64,
}

/// State shared between the view, the paint callback and the frame callbacks.
struct Shared {
    /// The active tab's web view.
    web: RefCell<Option<Retained<WKWebView>>>,
    parent: RefCell<Option<Retained<NSView>>>,
    /// The slot was painted in the frame being drawn (`frame_end` reads and resets it).
    painted: Cell<bool>,
    visible: Cell<bool>,
    alive: Cell<bool>,
    /// One of the panel's own menus is open over the page.
    menu: Cell<bool>,
    /// The page's width last drawn, which says how many tools the address has room for.
    width: Cell<f32>,
    /// The active tab is the blank page: the panel's own start screen shows instead.
    blank: Cell<bool>,
    layout: Cell<Layout>,
    applied_zoom: Cell<f64>,
    /// Where the device frame was painted, for the rails around it.
    geometry: Cell<Option<Geometry>>,
}

/// A device frame as painted, in window points (`zoom` converts them to GPUI pixels).
#[derive(Clone, Copy, PartialEq)]
struct Geometry {
    /// The frame's left edge from the page area's left, and its size.
    left: f64,
    frame: (f64, f64),
    /// Frame pixels per CSS pixel.
    scale: f64,
    zoom: f64,
}

impl Shared {
    fn apply(&self) {
        if let Some(web) = self.web.borrow().as_ref() {
            let hidden = COVERED.with(Cell::get)
                || self.menu.get()
                || self.blank.get()
                || !self.visible.get();
            // A hidden page must not keep the keys, or nothing in the window can be typed in.
            if hidden
                && let (Some(window), Some(parent)) = (web.window(), self.parent.borrow().as_ref())
                && window
                    .firstResponder()
                    .and_then(|responder| responder.downcast::<NSView>().ok())
                    .is_some_and(|view| view.isDescendantOf(web))
            {
                window.makeFirstResponder(Some(parent));
            }
            web.setHidden(hidden);
        }
    }
}

/// Runs at the end of every frame the window draws (the last child of the shell's root): a web
/// view whose slot wasn't painted in it is hidden, since native views draw above GPUI and the
/// panel is gone from the tree. Idle windows draw no frames, so this costs nothing at rest.
pub fn frame_end() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, _, _| {
            LIVE.with(|live| {
                for shared in live.borrow().iter().filter_map(Weak::upgrade) {
                    if shared.painted.replace(false) {
                        continue;
                    }
                    if shared.visible.replace(false) {
                        shared.apply();
                    }
                }
            });
        },
    )
    .absolute()
    .size_0()
}

#[derive(Clone, PartialEq)]
pub struct Device {
    pub width: u32,
    pub height: u32,
    pub preset: Option<String>,
}

impl Device {
    fn json(&self) -> Value {
        let mut value = json!({ "width": self.width, "height": self.height });
        if let Some(preset) = &self.preset {
            value["preset"] = json!(preset);
        }
        value
    }
    fn from_size(width: u32, height: u32) -> Self {
        let (width, height) = (width.clamp(240, 3840), height.clamp(240, 3840));
        let preset = PRESETS
            .iter()
            .find(|p| (p.2 == width && p.3 == height) || (p.2 == height && p.3 == width))
            .map(|p| p.0.to_owned());
        Self {
            width,
            height,
            preset,
        }
    }
    fn from_json(value: &Value) -> Option<Self> {
        let size = |key: &str| value[key].as_u64().map(|v| v as u32);
        let mut device = Self::from_size(size("width")?, size("height")?);
        device.preset = value["preset"].as_str().map(str::to_owned);
        Some(device)
    }
}

struct Tab {
    id: String,
    web: Retained<WKWebView>,
    _delegate: Retained<TabDelegate>,
    observer: Retained<TabObserver>,
    url: String,
    title: String,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    favicon: String,
    profile: String,
    crashes: Vec<Instant>,
}

impl Drop for Tab {
    fn drop(&mut self) {
        // WebKit objects must not be released while observed.
        self.observer.detach(&self.web);
    }
}

#[derive(Clone, PartialEq)]
enum Menu {
    More,
    NewTab,
    Presets,
    Fill,
    Logins,
    /// The form to add (or edit) a saved login.
    LoginForm,
    /// Custom browser profiles: add, rename, delete, and the one new tabs open in.
    Profiles,
}

/// The two size fields of the device bar.
struct DeviceInputs {
    width: Entity<TextField>,
    height: Entity<TextField>,
    _subs: Vec<Subscription>,
}

/// A device frame's rail being dragged.
struct Drag {
    edge: Edge,
    from: Point<Pixels>,
    start: (u32, u32),
    scale: f64,
}

/// The fields of the saved-login form (`SavedLoginsSettings.tsx`).
struct LoginForm {
    /// Set when editing a saved login: an empty password then keeps the saved one.
    id: Option<String>,
    site: Entity<TextField>,
    username: Entity<TextField>,
    password: Entity<TextField>,
    totp: Entity<TextField>,
    agent_use: bool,
    error: Option<String>,
    busy: bool,
    _subs: Vec<Subscription>,
}

/// The profile menu's fields: a new profile's name, and the name being edited.
struct ProfileForm {
    name: Entity<TextField>,
    rename: Entity<TextField>,
    renaming: Option<String>,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

/// A recording in progress; the frame loop is a task of the browser.
struct Recording {
    control: Rc<RecordControl>,
    _task: Task<()>,
}

/// How the person's stop (or an agent's) reaches the recording task, and who waits for the file.
#[derive(Default)]
struct RecordControl {
    /// Set to stop; `true` reveals the file in Finder afterwards.
    stop: Cell<Option<bool>>,
    waiters: RefCell<Vec<async_channel::Sender<Result<Value, String>>>>,
}

/// What a browser tool did so far: finished, or waiting on the page (answer decoded later).
pub enum Step {
    Done(Result<Value, String>),
    Script(Answer<String>),
    Shot(Answer<Vec<u8>>),
    /// A long action (recording) that answers a JSON value when it is done.
    Later(Answer<Value>),
}

pub struct Browser {
    shared: Rc<Shared>,
    session_id: String,
    backend: Backend,
    events: Events,
    tabs: Vec<Tab>,
    active: Option<String>,
    next_tab: u32,
    open: bool,
    device: Option<Device>,
    user_zoom: f64,
    appearance: String,
    dialog: Option<PendingDialog>,
    dialog_text: String,
    private_store: Option<Retained<WKWebsiteDataStore>>,
    pip: Option<Pip>,
    recording: Option<Recording>,
    /// Pages visited in this conversation's browser, newest first (the address bar's history).
    history: Vec<String>,
    suggest_open: bool,
    suggest_row: Option<usize>,
    /// The local servers listed on the blank page.
    servers: Vec<LocalServer>,
    device_inputs: Option<DeviceInputs>,
    /// The size fields need the device's size written into them.
    device_inputs_stale: bool,
    drag: Option<Drag>,
    /// The blank page should list the local servers again (set when it shows).
    want_servers: bool,
    login_form: Option<LoginForm>,
    profile_form: Option<ProfileForm>,
    // The address field.
    focus: FocusHandle,
    focus_pending: bool,
    text: String,
    anchor: usize,
    cursor: usize,
    marked: Option<Range<usize>>,
    last_url: String,
    last_active: Option<String>,
    menu: Option<Menu>,
    menu_since: Instant,
    menu_pressed_shut: Option<(Menu, Instant)>,
    /// The page as it looked when a menu opened: the web view hides under menus, this stays.
    frozen: Option<std::sync::Arc<gpui::Image>>,
    notice: Option<(String, Instant)>,
    annotating: bool,
    matches: Vec<Value>,
    matches_for: String,
    logins: Vec<Value>,
    /// Handles what the tabs report, when they say so.
    #[allow(dead_code)]
    pump: Option<Task<()>>,
    notice_timer: Option<Task<()>>,
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}
fn byte_at(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, c) in text.char_indices() {
        if units >= utf16 {
            return byte;
        }
        units += c.len_utf16();
    }
    text.len()
}

fn danger() -> gpui::Hsla {
    gpui::rgb(0xec8a85).into()
}

fn blank_url(url: &str) -> bool {
    url.is_empty() || url == "about:blank"
}

/// Clamps the person's zoom to 50-200 % in whole percents (`clamp_user_zoom`).
fn clamp_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        (zoom * 100.).round().clamp(50., 200.) / 100.
    } else {
        1.
    }
}

/// The device frame inside the space, and the zoom that keeps its CSS size (`fitDevice`).
pub fn fit_device(available: (f64, f64), size: (f64, f64)) -> (f64, f64, f64) {
    let scale = (available.0 / size.0)
        .min(available.1 / size.1)
        .clamp(0.1, 1.);
    ((size.0 * scale).floor(), (size.1 * scale).floor(), scale)
}

impl Browser {
    fn new(session_id: String, backend: Backend, cx: &mut Context<Self>) -> Self {
        let shared = Rc::new(Shared {
            web: RefCell::new(None),
            parent: RefCell::new(None),
            painted: Cell::new(false),
            visible: Cell::new(false),
            alive: Cell::new(true),
            menu: Cell::new(false),
            width: Cell::new(0.),
            blank: Cell::new(true),
            layout: Cell::new(Layout {
                device: None,
                user_zoom: 1.,
            }),
            applied_zoom: Cell::new(f64::NAN),
            geometry: Cell::new(None),
        });
        LIVE.with(|live| live.borrow_mut().push(Rc::downgrade(&shared)));
        let (events, woken) = Queue::new();
        // The tabs wake this task (delegate callbacks, key-value observing); idle, it sleeps.
        let pump = cx.spawn(async move |this, cx| {
            while woken.recv().await.is_ok() {
                let gone = cx.update(|app| {
                    let Some(window) = app.windows().first().copied() else {
                        return false;
                    };
                    window
                        .update(app, |_, window, app| {
                            this.update(app, |this, cx| this.poll(window, cx)).is_err()
                        })
                        .unwrap_or(false)
                });
                if gone {
                    break;
                }
            }
        });
        Self {
            shared,
            session_id,
            backend,
            events,
            tabs: Vec::new(),
            active: None,
            next_tab: 1,
            open: false,
            device: None,
            user_zoom: 1.,
            appearance: "system".into(),
            dialog: None,
            dialog_text: String::new(),
            private_store: None,
            pip: None,
            recording: None,
            history: Vec::new(),
            suggest_open: false,
            suggest_row: None,
            servers: Vec::new(),
            device_inputs: None,
            device_inputs_stale: false,
            drag: None,
            want_servers: true,
            login_form: None,
            profile_form: None,
            focus: cx.focus_handle(),
            focus_pending: true,
            text: String::new(),
            anchor: 0,
            cursor: 0,
            marked: None,
            last_url: String::new(),
            last_active: None,
            menu: None,
            menu_since: Instant::now(),
            menu_pressed_shut: None,
            frozen: None,
            notice: None,
            annotating: false,
            matches: Vec::new(),
            matches_for: String::new(),
            logins: Vec::new(),
            pump: Some(pump),
            notice_timer: None,
        }
    }

    pub fn set_parent(&self, parent: Option<Retained<NSView>>) {
        if self.shared.parent.borrow().is_none() {
            *self.shared.parent.borrow_mut() = parent;
        }
    }

    // ---- Tabs ----

    fn active_tab(&self) -> Option<&Tab> {
        let id = self.active.as_ref()?;
        self.tabs.iter().find(|t| &t.id == id)
    }

    fn tab_mut(&mut self, id: &str) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    fn store_for(&mut self, profile: &str, mtm: MainThreadMarker) -> Retained<WKWebsiteDataStore> {
        match browser_profiles::store_of(profile) {
            Some(browser_profiles::Store::Private) => self
                .private_store
                .get_or_insert_with(|| unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) })
                .clone(),
            Some(browser_profiles::Store::Custom(bytes)) => {
                browser_profiles::custom_store(bytes, mtm)
            }
            _ => unsafe { WKWebsiteDataStore::defaultDataStore(mtm) },
        }
    }

    /// Opens a tab and makes it the active one. Needs the window's content view (set by the
    /// first paint, or by `set_parent`).
    pub fn new_tab(&mut self, url: Option<&str>, profile: &str) -> Result<String, String> {
        let mtm = MainThreadMarker::new().ok_or("the browser must run on the main thread")?;
        let parent = self
            .shared
            .parent
            .borrow()
            .clone()
            .ok_or("the Sirus window has no browser area yet")?;
        if self.tabs.len() >= MAX_TABS {
            // The new tab becomes active, so the oldest one can go.
            let oldest = self.tabs.remove(0);
            if self.pip.as_ref().is_some_and(|p| p.tab_id == oldest.id) {
                self.dismiss_pip();
            }
            oldest.web.removeFromSuperview();
        }
        // A deleted profile's tabs would otherwise open in its (recreated) store.
        let profile = if browser_profiles::store_of(profile).is_some() {
            profile
        } else {
            browser_profiles::DEFAULT
        };
        let id = format!("tab-{}", self.next_tab);
        self.next_tab += 1;
        let store = self.store_for(profile, mtm);
        let made = browser_tab::create(
            &id,
            self.events.clone(),
            &store,
            self.user_zoom,
            &self.appearance,
            mtm,
        );
        parent.addSubview(&made.web);
        if let Some(url) = url.filter(|u| browser_tab::allowed_url(u)) {
            browser_tab::load(&made.web, url);
        }
        self.tabs.push(Tab {
            id: id.clone(),
            web: made.web,
            _delegate: made.delegate,
            observer: made.observer,
            url: String::new(),
            title: String::new(),
            loading: false,
            can_back: false,
            can_forward: false,
            favicon: String::new(),
            profile: profile.to_owned(),
            crashes: Vec::new(),
        });
        self.open = true;
        self.select(&id);
        Ok(id)
    }

    fn select(&mut self, id: &str) {
        if !self.tabs.iter().any(|t| t.id == id) {
            return;
        }
        if self.annotating
            && let Some(old) = self.active_tab()
        {
            let _ = browser_tab::evaluate(&old.web, browser_js::ANNOTATE_CANCEL);
        }
        self.active = Some(id.to_owned());
        for tab in &self.tabs {
            // The tab in the floating window stays shown there.
            if !self.pip.as_ref().is_some_and(|p| p.tab_id == tab.id) {
                tab.web.setHidden(true);
            }
        }
        // The pane paints the active tab, unless that one is in the floating window.
        let web = self
            .active_tab()
            .filter(|t| !self.pip.as_ref().is_some_and(|p| p.tab_id == t.id))
            .map(|t| t.web.clone());
        *self.shared.web.borrow_mut() = web;
        self.shared.applied_zoom.set(f64::NAN);
        self.sync_layout();
        self.annotating = false;
        self.shared.apply();
    }

    fn close_tab(&mut self, id: &str) {
        let Some(at) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        if self.pip.as_ref().is_some_and(|p| p.tab_id == id) {
            self.dismiss_pip();
        }
        let tab = self.tabs.remove(at);
        tab.web.removeFromSuperview();
        if self.active.as_deref() == Some(id) {
            let next = self
                .tabs
                .get(at.min(self.tabs.len().saturating_sub(1)))
                .map(|t| t.id.clone());
            self.active = None;
            *self.shared.web.borrow_mut() = None;
            match next {
                Some(next) => self.select(&next),
                None => {
                    self.shared.blank.set(true);
                    self.shared.apply();
                }
            }
        }
        if self.dialog.as_ref().is_some_and(|d| d.tab_id == id) {
            self.dialog = None;
        }
    }

    /// Closes every tab: the session browser is shut (`browser_close`).
    pub fn close_all(&mut self) {
        self.dismiss_pip();
        for tab in self.tabs.drain(..) {
            tab.web.removeFromSuperview();
        }
        self.active = None;
        self.dialog = None;
        self.open = false;
        *self.shared.web.borrow_mut() = None;
        self.shared.blank.set(true);
        self.shared.apply();
    }

    /// A link the person clicked (Settings → Navegador → Abrir links no): a new tab, or the
    /// first one when none is open.
    pub fn open_link(&mut self, url: &str) -> Result<(), String> {
        if self.tabs.is_empty() {
            self.ensure_open()?;
            self.navigate_active(url);
        } else {
            self.new_tab(Some(url), &default_profile())?;
            self.open = true;
        }
        Ok(())
    }

    pub fn ensure_open(&mut self) -> Result<(), String> {
        if self.tabs.is_empty() {
            self.new_tab(None, &default_profile())?;
        }
        self.open = true;
        Ok(())
    }

    fn navigate_active(&mut self, url: &str) {
        if self.ensure_open().is_err() {
            return;
        }
        if let Some(tab) = self.active_tab() {
            browser_tab::load(&tab.web, url);
        }
    }

    /// Frames and zoom of the active page when the panel isn't painting it (an agent's tab).
    fn sync_layout(&mut self) {
        self.shared.layout.set(Layout {
            device: self
                .device
                .as_ref()
                .map(|d| (d.width as f64, d.height as f64)),
            user_zoom: self.user_zoom,
        });
        if self.shared.visible.get() {
            return;
        }
        // A hidden page keeps the device's layout (or a laptop's) for agents (ADR-112).
        let (w, h) = self
            .device
            .as_ref()
            .map(|d| {
                (
                    d.width as f64 * self.user_zoom,
                    d.height as f64 * self.user_zoom,
                )
            })
            .unwrap_or((1280., 800.));
        for tab in &self.tabs {
            if self.pip.as_ref().is_some_and(|p| p.tab_id == tab.id) {
                continue;
            }
            tab.web
                .setFrame(NSRect::new(NSPoint::new(0., 0.), NSSize::new(w, h)));
            unsafe { tab.web.setPageZoom(self.user_zoom) };
        }
    }

    // ---- Page state ----

    /// Reads every tab's address, title, loading and history state; true when one changed.
    fn read_tabs(&mut self) -> bool {
        let mut changed = false;
        for tab in &mut self.tabs {
            let (url, title, loading, back, forward) = unsafe {
                (
                    tab.web
                        .URL()
                        .and_then(|u| u.absoluteString())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    tab.web.title().map(|s| s.to_string()).unwrap_or_default(),
                    tab.web.isLoading(),
                    tab.web.canGoBack(),
                    tab.web.canGoForward(),
                )
            };
            if tab.url != url
                || tab.title != title
                || tab.loading != loading
                || tab.can_back != back
                || tab.can_forward != forward
            {
                let navigated = tab.url != url;
                tab.url = url;
                tab.title = title;
                tab.loading = loading;
                tab.can_back = back;
                tab.can_forward = forward;
                changed = true;
                if navigated || (!loading && tab.favicon.is_empty()) {
                    tab.favicon = String::new();
                }
            }
        }
        if changed
            && let Some(pip) = &self.pip
            && let Some(tab) = self.tabs.iter().find(|t| t.id == pip.tab_id)
        {
            pip.set_title(if tab.title.is_empty() {
                &tab.url
            } else {
                &tab.title
            });
        }
        changed
    }

    /// Reads each tab's address, title and history, and handles what the pages asked for. Runs
    /// when a tab reports a change, never on a timer.
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (events, _) = self.events.take();
        let mut changed = self.read_tabs();
        if let Some(blank) = self.active_tab().map(|t| blank_url(&t.url)) {
            if self.shared.blank.get() != blank {
                self.shared.blank.set(blank);
                self.shared.apply();
                // The blank page lists the servers listening now.
                self.want_servers |= blank;
            }
        }
        // The address field follows the active page unless it is being edited.
        let url = self.active_tab().map(|t| t.url.clone()).unwrap_or_default();
        let switched = self.last_active != self.active;
        if (url != self.last_url || switched) && (!self.focus.is_focused(window) || switched) {
            self.text = if blank_url(&url) {
                String::new()
            } else {
                url.clone()
            };
            self.anchor = self.text.len();
            self.cursor = self.text.len();
        }
        if url != self.last_url || switched {
            // Private tabs leave no history behind.
            if self
                .active_tab()
                .is_some_and(|t| t.profile != browser_profiles::PRIVATE)
            {
                browser_address::note_visit(&mut self.history, &url);
            }
            self.last_url = url;
            self.last_active = self.active.clone();
            self.refresh_matches(cx);
            changed = true;
        }
        for (tab_id, event) in events {
            changed = true;
            match event {
                TabEvent::Popup(url) => {
                    let profile = self
                        .tabs
                        .iter()
                        .find(|t| t.id == tab_id)
                        .map(|t| t.profile.clone())
                        .unwrap_or_else(|| "default".into());
                    let _ = self.new_tab(Some(&url), &profile);
                }
                TabEvent::Dialog(dialog) => {
                    // One at a time: a second one is dismissed so the page never freezes.
                    if self.dialog.is_none() {
                        self.dialog_text = dialog.default_text.clone().unwrap_or_default();
                        self.dialog = Some(dialog);
                    }
                }
                TabEvent::Login { frame_url, body } => {
                    let private = self
                        .tabs
                        .iter()
                        .find(|t| t.id == tab_id)
                        .is_none_or(|t| t.profile == "private");
                    if !private {
                        self.backend.invoke(
                            "login_vault_note_submission",
                            json!({ "sessionId": self.session_id, "frameUrl": frame_url, "raw": body }),
                        );
                    }
                }
                TabEvent::PipClosed => {
                    if self.pip.as_ref().is_some_and(|p| p.tab_id == tab_id) {
                        self.dismiss_pip();
                    }
                }
                TabEvent::Crashed => {
                    if let Some(tab) = self.tab_mut(&tab_id) {
                        // Three reloads in thirty seconds at most.
                        let now = Instant::now();
                        tab.crashes
                            .retain(|t| now.duration_since(*t) < Duration::from_secs(30));
                        if tab.crashes.len() < 3 {
                            tab.crashes.push(now);
                            unsafe {
                                if tab.web.reload().is_none() && browser_tab::allowed_url(&tab.url)
                                {
                                    browser_tab::load(&tab.web, &tab.url.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Saved logins that match the active page's exact origin (the key button).
    fn refresh_matches(&mut self, cx: &mut Context<Self>) {
        let url = self.active_tab().map(|t| t.url.clone()).unwrap_or_default();
        if !url.starts_with("http") {
            self.matches.clear();
            self.matches_for.clear();
            return;
        }
        let stamp = format!("{url}\u{0}{}", crate::browser_bridge::offer_count());
        if self.matches_for == stamp {
            return;
        }
        self.matches_for = stamp;
        let reply = self
            .backend
            .request("login_vault_matches", json!({ "url": url }));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(value)) = reply.recv().await {
                let _ = this.update(cx, |this, cx| {
                    this.matches = value.as_array().cloned().unwrap_or_default();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// The session browser's state, as `BrowserSessionState` serializes it.
    /// True while the active page is loading (read from the web view, not the last wake-up).
    pub fn page_loading(&self) -> bool {
        self.active_tab()
            .is_some_and(|t| unsafe { t.web.isLoading() })
    }

    /// The state with every tab's address, title and loading as they are now.
    pub fn fresh_state(&mut self) -> Value {
        self.read_tabs();
        self.state_json()
    }

    pub fn state_json(&self) -> Value {
        json!({
            "sessionId": self.session_id,
            "open": self.open,
            "tabs": self.tabs.iter().map(|t| json!({
                "id": t.id, "url": t.url, "title": t.title, "loading": t.loading,
                "canGoBack": t.can_back, "canGoForward": t.can_forward,
                "faviconUrl": t.favicon, "profile": t.profile,
            })).collect::<Vec<_>>(),
            "activeTabId": self.active,
            "device": self.device.as_ref().map(Device::json),
            "zoom": self.user_zoom,
            "appearance": self.appearance,
            "dialog": self.dialog.as_ref().map(|d| {
                let mut v = json!({ "tabId": d.tab_id, "kind": d.kind, "message": d.message, "url": d.url });
                if let Some(text) = &d.default_text { v["defaultText"] = json!(text); }
                v
            }),
        })
    }

    pub fn set_device(&mut self, device: Option<Device>) {
        self.device = device;
        self.device_inputs_stale = true;
        self.sync_layout();
    }

    pub fn set_appearance(&mut self, mode: &str) {
        self.appearance = mode.to_owned();
        for tab in &self.tabs {
            browser_tab::apply_appearance(&tab.web, mode);
        }
    }

    fn set_user_zoom(&mut self, zoom: f64) {
        self.user_zoom = clamp_zoom(zoom);
        self.shared.applied_zoom.set(f64::NAN);
        self.sync_layout();
    }

    pub fn answer_dialog(&mut self, accept: bool, text: Option<&str>) -> Option<Value> {
        let dialog = self.dialog.take()?;
        let info = json!({
            "tabId": dialog.tab_id, "kind": dialog.kind, "message": dialog.message, "url": dialog.url,
        });
        dialog.answer(accept, text);
        Some(info)
    }

    // ---- Agent tools ----

    /// Starts one browser tool. Arguments were validated by the host (`browser_bridge.rs`).
    pub fn tool(&mut self, tool: &str, args: &Value, cx: &mut Context<Self>) -> Step {
        let mtm = MainThreadMarker::new();
        // Tools answer with the pages as they are now, not as the last wake-up saw them.
        self.read_tabs();
        let done = |value: Value| Step::Done(Ok(value));
        let page = |this: &mut Self, body: &str, input: Value| -> Step {
            let (Some(mtm), Ok(())) = (mtm, this.ensure_open()) else {
                return Step::Done(Err("No browser tab is open.".into()));
            };
            match this.active_tab() {
                Some(tab) => Step::Script(browser_tab::call_page(&tab.web, body, &input, mtm)),
                None => Step::Done(Err("No browser tab is open.".into())),
            }
        };
        match tool {
            "browser_status" | "browser_tabs" => done(self.state_json()),
            "browser_open" => {
                if let Err(error) = self.ensure_open() {
                    return Step::Done(Err(error));
                }
                if let Some(url) = args["url"].as_str() {
                    self.navigate_active(url);
                }
                done(self.state_json())
            }
            "browser_navigate" => {
                self.navigate_active(args["url"].as_str().unwrap_or("about:blank"));
                done(self.state_json())
            }
            "browser_back" | "browser_forward" | "browser_reload" => {
                if let Some(tab) = self.active_tab() {
                    unsafe {
                        match tool {
                            "browser_back" => {
                                tab.web.goBack();
                            }
                            "browser_forward" => {
                                tab.web.goForward();
                            }
                            _ => {
                                tab.web.reload();
                            }
                        }
                    }
                }
                done(self.state_json())
            }
            "browser_close" => {
                self.close_all();
                done(self.state_json())
            }
            "browser_device" => {
                self.set_device(Device::from_json(&args["device"]));
                done(self.state_json())
            }
            "browser_color_scheme" => {
                self.set_appearance(args["scheme"].as_str().unwrap_or("system"));
                done(self.state_json())
            }
            "browser_screenshot" => match self.active_tab() {
                Some(tab) => Step::Shot(browser_tab::snapshot(&tab.web)),
                None => Step::Done(Err("No browser tab is open.".into())),
            },
            "browser_dialog" => {
                let accept = args["accept"].as_bool().unwrap_or(false);
                let answered = self.answer_dialog(accept, args["text"].as_str());
                match answered {
                    Some(info) => done(json!({ "answered": info, "status": self.state_json() })),
                    None => Step::Done(Err("No dialog is open in this browser.".into())),
                }
            }
            "browser_snapshot" => page(self, browser_js::SNAPSHOT, json!({})),
            "browser_click" => page(self, browser_js::CLICK, args.clone()),
            "browser_type" => page(self, browser_js::TYPE, args.clone()),
            "browser_press" => page(self, browser_js::PRESS, args.clone()),
            "browser_wait_for" => page(self, browser_js::WAIT, args.clone()),
            "browser_scroll" => page(self, browser_js::SCROLL, args.clone()),
            "browser_logs" => page(self, browser_js::LOGS, args.clone()),
            "browser_hover" => page(self, browser_js::HOVER, args.clone()),
            "browser_select" => page(self, browser_js::SELECT, args.clone()),
            "browser_drag" => page(self, browser_js::DRAG, args.clone()),
            "browser_upload" => page(self, browser_js::UPLOAD, args.clone()),
            "browser_network" => page(self, browser_js::NETWORK_READ, args.clone()),
            "page_script" => {
                let body = args["body"].as_str().unwrap_or_default().to_owned();
                page(self, &body, args["args"].clone())
            }
            "fill_login_page" => {
                // The values travel only as the script's argument, never in its source.
                let input = args["script_args"].clone();
                match args["tabId"]
                    .as_str()
                    .and_then(|id| self.tabs.iter().find(|t| t.id == id))
                {
                    Some(tab) => match mtm {
                        Some(mtm) => Step::Script(browser_tab::call_page(
                            &tab.web,
                            browser_js::LOGIN_FILL,
                            &input,
                            mtm,
                        )),
                        None => Step::Done(Err("the browser must run on the main thread".into())),
                    },
                    None => Step::Done(Err("No browser tab is open.".into())),
                }
            }
            "browser_record" => match args["action"].as_str() {
                Some("stop") => Step::Later(self.stop_recording(false)),
                _ => Step::Later(self.start_recording(cx)),
            },
            _ => Step::Done(Err("unknown browser tool".into())),
        }
    }

    // ---- The address field ----

    fn navigate(&mut self, cx: &mut Context<Self>) {
        let Some(url) = browser_address::normalize_input(&self.text) else {
            return;
        };
        self.close_suggestions();
        if !browser_tab::allowed_url(&url) {
            self.say(t("Só endereços http e https abrem aqui."), cx);
            return;
        }
        self.text = url.clone();
        self.anchor = self.text.len();
        self.cursor = self.text.len();
        self.navigate_active(&url);
        cx.notify();
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }
    fn insert(&mut self, text: &str) {
        let r = self.selection();
        self.text.replace_range(r.clone(), text);
        self.cursor = r.start + text.len();
        self.anchor = self.cursor;
    }
    fn step(&self, from: usize, forward: bool) -> usize {
        if forward {
            self.text[from..]
                .chars()
                .next()
                .map_or(from, |c| from + c.len_utf8())
        } else {
            self.text[..from]
                .chars()
                .next_back()
                .map_or(from, |c| from - c.len_utf8())
        }
    }
    fn move_to(&mut self, to: usize, extend: bool) {
        self.cursor = to;
        if !extend {
            self.anchor = to;
        }
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        let (shift, command) = (keys.modifiers.shift, keys.modifiers.platform);
        let sel = self.selection();
        let rows = if self.suggest_open {
            self.suggestion_rows()
        } else {
            Vec::new()
        };
        match keys.key.as_str() {
            "enter" => match self.suggest_row.and_then(|row| rows.get(row)) {
                Some(row) => self.pick_suggestion(&row.clone(), cx),
                None => self.navigate(cx),
            },
            "down" if !rows.is_empty() => {
                self.suggest_row = Some(
                    self.suggest_row
                        .map_or(0, |row| (row + 1).min(rows.len() - 1)),
                );
            }
            "up" if !rows.is_empty() => {
                self.suggest_row = match self.suggest_row {
                    Some(row) if row > 0 => Some(row - 1),
                    _ => None,
                };
            }
            // The first Escape closes the suggestions, the next one puts the address back.
            "escape" if self.suggest_open => self.close_suggestions(),
            "escape" => {
                self.text = if blank_url(&self.last_url) {
                    String::new()
                } else {
                    self.last_url.clone()
                };
                self.move_to(self.text.len(), false);
            }
            "backspace" if !sel.is_empty() => self.insert(""),
            "backspace" => {
                self.anchor = self.step(self.cursor, false);
                self.insert("");
            }
            "delete" if !sel.is_empty() => self.insert(""),
            "delete" => {
                self.anchor = self.step(self.cursor, true);
                self.insert("");
            }
            "left" if command => self.move_to(0, shift),
            "right" if command => self.move_to(self.text.len(), shift),
            "left" if !sel.is_empty() && !shift => self.move_to(sel.start, false),
            "right" if !sel.is_empty() && !shift => self.move_to(sel.end, false),
            "left" => self.move_to(self.step(self.cursor, false), shift),
            "right" => self.move_to(self.step(self.cursor, true), shift),
            "home" => self.move_to(0, shift),
            "end" => self.move_to(self.text.len(), shift),
            "a" if command => {
                self.anchor = 0;
                self.cursor = self.text.len();
            }
            "c" if command => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.text[sel].to_owned()))
            }
            "x" if command => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.text[sel].to_owned()));
                self.insert("");
            }
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.insert(&text.replace(['\n', '\r'], ""));
                }
            }
            _ => return,
        }
        // Editing the text brings the suggestions back, narrowed by it.
        if matches!(keys.key.as_str(), "backspace" | "delete")
            || (keys.key == "v" && command)
            || (keys.key == "x" && command)
        {
            self.suggest_open = true;
            self.suggest_row = None;
        }
        self.marked = None;
        cx.stop_propagation();
        cx.notify();
    }

    // ---- The person's actions ----

    fn open_menu(&mut self, menu: Option<Menu>, cx: &mut Context<Self>) {
        // The press on this menu's own button already closed it (the scrim sees the press first
        // and the button the click): that press must not open it again.
        if self
            .menu_pressed_shut
            .take()
            .is_some_and(|(shut, at)| Some(shut) == menu && at.elapsed() < MENU_PRESS)
        {
            return;
        }
        self.menu = if self.menu == menu { None } else { menu };
        self.menu_since = Instant::now();
        let web = self.shared.web.borrow().clone();
        match web
            .filter(|_| self.menu.is_some() && !self.shared.menu.get() && self.shared.visible.get())
        {
            // The page hides once its picture is ready, so it never leaves a hole.
            Some(web) => self.freeze_page(&web, cx),
            None => {
                self.shared.menu.set(self.menu.is_some());
                self.shared.apply();
            }
        }
        if self.menu == Some(Menu::Logins) {
            let reply = self.backend.request("login_vault_list", json!({}));
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(value)) = reply.recv().await {
                    let _ = this.update(cx, |this, cx| {
                        this.logins = value.as_array().cloned().unwrap_or_default();
                        cx.notify();
                    });
                }
            })
            .detach();
        }
        cx.notify();
    }

    fn toggle_device(&mut self, cx: &mut Context<Self>) {
        if self.device.is_some() {
            self.set_device(None);
        } else {
            self.set_device(Some(Device {
                width: 390,
                height: 844,
                preset: Some("iphone-12-pro".into()),
            }));
            self.device_inputs_stale = true;
        }
        cx.notify();
    }

    fn freeze_page(&mut self, web: &WKWebView, cx: &mut Context<Self>) {
        let shot = browser_tab::snapshot(web);
        cx.spawn(async move |this, cx| {
            let picture = shot.recv().await.ok().and_then(Result::ok);
            let _ = this.update(cx, |this, cx| {
                this.frozen = picture.map(|png| {
                    std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png))
                });
                this.cover_page(cx);
            });
        })
        .detach();
        // A page that never answers must not keep the menu under it.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FREEZE_LIMIT).await;
            let _ = this.update(cx, |this, cx| this.cover_page(cx));
        })
        .detach();
    }

    fn cover_page(&mut self, cx: &mut Context<Self>) {
        if self.menu.is_some() && !self.shared.menu.replace(true) {
            self.shared.apply();
            cx.notify();
        }
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.menu = None;
            self.shared.menu.set(false);
            self.shared.apply();
            cx.notify();
        }
    }

    /// Shows a short notice over the page; one timer clears it, nothing polls for it.
    fn say(&mut self, text: &str, cx: &mut Context<Self>) {
        let since = Instant::now();
        self.notice = Some((text.to_owned(), since));
        self.notice_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE).await;
            let _ = this.update(cx, |this, cx| {
                if this.notice.as_ref().is_some_and(|(_, at)| *at == since) {
                    this.notice = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn copy_link(&mut self, cx: &mut Context<Self>) {
        let Some(url) = self
            .active_tab()
            .map(|t| t.url.clone())
            .filter(|u| browser_tab::allowed_url(u) && !blank_url(u))
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        self.say(t("Link copiado"), cx);
    }

    /// The camera: the visible page as a PNG, attached to the composer.
    fn capture_to_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let answer = browser_tab::snapshot(&tab.web);
        cx.spawn_in(window, async move |this, cx| {
            let result = answer.recv().await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(Ok(bytes)) => {
                    let path = std::env::temp_dir().join(format!("browser-{}.png", stamp()));
                    if std::fs::write(&path, bytes).is_ok() {
                        attach(vec![path], window, cx);
                    }
                }
                _ => this.say(t("Não foi possível capturar a página."), cx),
            });
        })
        .detach();
    }

    /// The element picker (ADR-151): marks elements on the page; finishing attaches the picture
    /// and a text file naming each element's selector.
    fn toggle_annotate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(web) = self.active_tab().map(|t| t.web.clone()) else {
            return;
        };
        if !self.annotating {
            self.annotating = true;
            let _ = browser_tab::evaluate(&web, browser_js::ANNOTATE_START);
            cx.notify();
            return;
        }
        self.annotating = false;
        cx.notify();
        let measured = browser_tab::evaluate(&web, browser_js::ANNOTATE_MEASURE);
        cx.spawn_in(window, async move |this, cx| {
            let raw = match measured.recv().await {
                Ok(Ok(raw)) => raw,
                _ => String::new(),
            };
            let capture = parse_annotation(&raw);
            let shot = if capture.elements.is_empty() {
                None
            } else {
                Some(browser_tab::snapshot(&web))
            };
            let png = match shot {
                Some(shot) => shot.recv().await.ok().and_then(Result::ok),
                None => None,
            };
            let _ = browser_tab::evaluate(&web, browser_js::ANNOTATE_FINISH);
            let _ = this.update_in(cx, |this, window, cx| {
                if capture.elements.is_empty() {
                    this.say(t("Nenhum elemento foi marcado."), cx);
                    return;
                }
                let stamp = stamp();
                let dir = std::env::temp_dir();
                let mut files = Vec::new();
                // The picture holds only the marked elements (and a margin), not the whole page.
                if let Some(png) = png.and_then(|png| crop_annotation(&png, &capture)) {
                    let path = dir.join(format!("annotation-{stamp}.png"));
                    if std::fs::write(&path, png).is_ok() {
                        files.push(path);
                    }
                }
                let text = dir.join(format!("annotation-{stamp}.txt"));
                if std::fs::write(&text, annotation_text(&capture)).is_ok() {
                    files.push(text);
                }
                attach(files, window, cx);
                this.say(t("Anotação anexada"), cx);
            });
        })
        .detach();
    }

    fn fill(&mut self, login_id: &str, part: &str, cx: &mut Context<Self>) {
        self.close_menu(cx);
        let reply = self.backend.request(
            "login_vault_autofill",
            json!({ "sessionId": self.session_id, "loginId": login_id, "part": part }),
        );
        cx.spawn(async move |this, cx| {
            if let Ok(result) = reply.recv().await {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(_) => this.say(t("Login preenchido."), cx),
                    Err(error) => {
                        let text = error["message"]
                            .as_str()
                            .or(error.as_str())
                            .unwrap_or("")
                            .to_owned();
                        this.say(&text, cx);
                    }
                });
            }
        })
        .detach();
    }

    fn clear_data(&mut self, everything: bool, cx: &mut Context<Self>) {
        self.close_menu(cx);
        crate::browser_data::clear(everything);
        self.say(
            t(if everything {
                "Cookies limpos em todas as abas do navegador"
            } else {
                "Cache limpo"
            }),
            cx,
        );
    }

    // ---- Drawing ----

    fn button(
        &self,
        id: &'static str,
        path: &'static str,
        flip: bool,
        enabled: bool,
        on: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let mut icon =
            theme::icon(path, 15.).text_color(theme::content(if enabled { 0.7 } else { 0.25 }));
        if on {
            icon = icon.text_color(theme::accent());
        }
        if flip {
            icon = icon.with_transformation(Transformation::rotate(percentage(0.5)));
        }
        let button = div()
            .id(id)
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(crate::theme::RADIUS_MD))
            .flex_shrink_0()
            .when(on, |d| d.bg(theme::accent_wash(0.16)))
            .child(icon);
        if enabled {
            button
                .cursor_pointer()
                .sinks()
                .hover(|s| s.bg(theme::content(0.08)))
                .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
        } else {
            button
        }
    }

    /// The record button: a dot, red while recording (the toggle is the same for an agent's).
    fn record_button(&self, has_page: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let recording = self.recording.is_some();
        let enabled = recording || has_page;
        let button = div()
            .id("browser-record")
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(crate::theme::RADIUS_MD))
            .flex_shrink_0()
            .when(recording, |d| d.bg(danger().opacity(0.16)))
            .child(theme::icon("browser-record", 15.).text_color(if recording {
                danger()
            } else {
                theme::content(if enabled { 0.7 } else { 0.25 })
            }));
        if enabled {
            button
                .cursor_pointer()
                .sinks()
                .hover(|s| s.bg(theme::content(0.08)))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_recording(cx)))
        } else {
            button
        }
    }

    fn tab_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("browser-tabs")
            .h(px(TAB_STRIP))
            .pt(px(4.))
            .w_full()
            .flex_shrink_0()
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(4.))
            .overflow_x_scroll()
            .children(self.tabs.iter().map(|tab| {
                let selected = self.active.as_deref() == Some(&tab.id);
                let (select_id, close_id) = (tab.id.clone(), tab.id.clone());
                let title = if tab.title.is_empty() {
                    if blank_url(&tab.url) {
                        t("Nova aba").to_owned()
                    } else {
                        tab.url.clone()
                    }
                } else {
                    tab.title.clone()
                };
                let lead = if tab.loading {
                    theme::icon("refresh", 12.)
                        .text_color(theme::content(0.6))
                        .with_animation(
                            SharedString::from(format!("spin:{}", tab.id)),
                            Animation::new(Duration::from_millis(900)).repeat(),
                            |icon, t| {
                                icon.with_transformation(Transformation::rotate(percentage(t)))
                            },
                        )
                        .into_any_element()
                } else {
                    theme::icon("globe", 12.)
                        .text_color(theme::content(0.55))
                        .into_any_element()
                };
                div()
                    .id(SharedString::from(format!("tab:{}", tab.id)))
                    .group("tab")
                    .h(px(26.))
                    .min_w(px(60.))
                    .max_w(px(150.))
                    .px(px(8.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .rounded(px(crate::theme::RADIUS_MD))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(theme::content(if selected { 0.95 } else { 0.6 }))
                    .when(selected, |d| d.bg(theme::content(0.1)))
                    .when(!selected, |d| d.hover(|s| s.bg(theme::content(0.06))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(&select_id);
                        this.close_menu(cx);
                        cx.notify();
                    }))
                    .child(lead)
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(
                        div()
                            .id(SharedString::from(format!("tab-close:{}", tab.id)))
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(crate::theme::RADIUS_SM))
                            .flex_shrink_0()
                            .hover(|s| s.bg(theme::content(0.14)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(&close_id);
                                cx.notify();
                            }))
                            .child(theme::icon("close", 9.).text_color(theme::content(0.6))),
                    )
            }))
            .child(self.button(
                "browser-new-tab",
                "plus",
                false,
                self.open,
                self.menu == Some(Menu::NewTab),
                cx,
                |this, _, cx| {
                    this.open_menu(Some(Menu::NewTab), cx);
                },
            ))
            .into_any_element()
    }

    fn item(
        &self,
        id: impl Into<SharedString>,
        label: String,
        checked: bool,
        enabled: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        self.iconed_item(None, id, label, checked, enabled, cx, action)
    }

    #[allow(clippy::too_many_arguments)]
    fn iconed_item(
        &self,
        glyph: Option<&'static str>,
        id: impl Into<SharedString>,
        label: String,
        checked: bool,
        enabled: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let row = div()
            .id(id.into())
            .h(px(28.))
            .px(px(10.))
            .rounded(px(crate::theme::RADIUS_MD))
            .flex()
            .items_center()
            .gap(px(8.))
            .text_size(px(12.5))
            .text_color(theme::content(if enabled { 0.9 } else { 0.35 }))
            .when_some(glyph, |d, glyph| {
                d.child(
                    theme::icon(glyph, 14.)
                        .flex_shrink_0()
                        .text_color(theme::content(if enabled { 0.65 } else { 0.25 })),
                )
            })
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when(checked, |d| {
                d.child(theme::icon("check", 12.).text_color(theme::accent()))
            });
        if enabled {
            row.cursor_pointer()
                .hover(|s| s.bg(theme::content(0.08)))
                .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
                .into_any_element()
        } else {
            row.into_any_element()
        }
    }

    fn divider() -> AnyElement {
        div()
            .h(px(1.))
            .mx(px(6.))
            .my(px(4.))
            .bg(theme::content(0.1))
            .into_any_element()
    }

    fn menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.clone()?;
        let active = self.active_tab().map(|t| (t.id.clone(), t.url.clone()));
        let page = active.as_ref().is_some_and(|(_, url)| !blank_url(url));
        let mut rows: Vec<AnyElement> = Vec::new();
        let (mut right, mut left) = (Some(8.), None::<f32>);
        let mut width = 236.;
        match menu {
            Menu::More => {
                let fit = tools_that_fit(self.shared.width.get());
                if fit <= PICK_RANK {
                    rows.push(self.iconed_item(
                        Some("pointer"),
                        "m-pick",
                        t("Selecionar elemento").into(),
                        self.annotating,
                        page,
                        cx,
                        |this, window, cx| {
                            this.close_menu(cx);
                            this.toggle_annotate(window, cx);
                        },
                    ));
                }
                if fit <= SHOT_RANK {
                    rows.push(self.iconed_item(
                        Some("browser-camera"),
                        "m-shot",
                        t("Capturar a página").into(),
                        false,
                        page,
                        cx,
                        |this, window, cx| {
                            this.close_menu(cx);
                            this.capture_to_composer(window, cx);
                        },
                    ));
                }
                if fit <= DEVICE_RANK {
                    rows.push(self.iconed_item(
                        Some("smartphone"),
                        "m-device",
                        t("Tamanho de aparelho").into(),
                        self.device.is_some(),
                        page,
                        cx,
                        |this, _, cx| {
                            this.close_menu(cx);
                            this.toggle_device(cx);
                        },
                    ));
                }
                if fit <= LINK_RANK {
                    rows.push(self.iconed_item(
                        Some("browser-link"),
                        "m-link",
                        t("Copiar link").into(),
                        false,
                        page,
                        cx,
                        |this, _, cx| {
                            this.copy_link(cx);
                            this.close_menu(cx);
                        },
                    ));
                }
                if fit <= RECORD_RANK {
                    let recording = self.recording.is_some();
                    rows.push(
                        self.iconed_item(
                            Some("browser-record"),
                            "m-record",
                            t(if recording {
                                "Parar a gravação"
                            } else {
                                "Gravar a página"
                            })
                            .into(),
                            false,
                            recording || page,
                            cx,
                            |this, _, cx| {
                                this.toggle_recording(cx);
                                this.close_menu(cx);
                            },
                        ),
                    );
                }
                if fit <= PIP_RANK {
                    rows.push(self.iconed_item(
                        Some("browser-pip"),
                        "m-pip",
                        t("Janela flutuante").into(),
                        self.pip.is_some(),
                        self.pip.is_some() || page,
                        cx,
                        |this, _, cx| {
                            this.toggle_pip(cx);
                            this.close_menu(cx);
                        },
                    ));
                }
                if fit < TOOLS {
                    rows.push(Self::divider());
                }
                rows.push(self.iconed_item(
                    Some("refresh"),
                    "m-hard",
                    t("Recarregar sem cache").into(),
                    false,
                    page,
                    cx,
                    |this, _, cx| {
                        if let Some(tab) = this.active_tab() {
                            unsafe { tab.web.reloadFromOrigin() };
                        }
                        this.close_menu(cx);
                    },
                ));
                rows.push(self.iconed_item(
                    Some("bug"),
                    "m-inspect",
                    t("Inspetor da web").into(),
                    false,
                    page,
                    cx,
                    |this, _, cx| {
                        if let Some(tab) = this.active_tab() {
                            crate::browser_data::inspector(&tab.web);
                        }
                        this.close_menu(cx);
                    },
                ));
                rows.push(self.iconed_item(
                    Some("globe"),
                    "m-system",
                    t("Abrir no navegador padrão").into(),
                    false,
                    page,
                    cx,
                    |this, _, cx| {
                        if let Some(url) = this.active_tab().map(|t| t.url.clone()) {
                            let _ = std::process::Command::new("open").arg(url).spawn();
                        }
                        this.close_menu(cx);
                    },
                ));
                rows.push(Self::divider());
                for (mode, label) in [
                    ("system", t("Seguir o sistema")),
                    ("light", t("Páginas claras")),
                    ("dark", t("Páginas escuras")),
                ] {
                    rows.push(self.iconed_item(
                        Some(match mode {
                            "light" => "sun",
                            "dark" => "moon",
                            _ => "monitor",
                        }),
                        format!("m-scheme-{mode}"),
                        label.into(),
                        self.appearance == mode,
                        true,
                        cx,
                        move |this, _, cx| {
                            this.set_appearance(mode);
                            this.close_menu(cx);
                        },
                    ));
                }
                rows.push(Self::divider());
                let zoom = self.user_zoom;
                rows.push(self.iconed_item(
                    Some("plus"),
                    "m-zoom-in",
                    t("Aumentar zoom").into(),
                    false,
                    zoom < 2.,
                    cx,
                    |this, _, cx| {
                        this.set_user_zoom(this.user_zoom + 0.1);
                        cx.notify();
                    },
                ));
                rows.push(self.iconed_item(
                    Some("minus"),
                    "m-zoom-out",
                    t("Diminuir zoom").into(),
                    false,
                    zoom > 0.5,
                    cx,
                    |this, _, cx| {
                        this.set_user_zoom(this.user_zoom - 0.1);
                        cx.notify();
                    },
                ));
                rows.push(self.iconed_item(
                    Some("search"),
                    "m-zoom-reset",
                    format!("{} · {}%", t("Tamanho real"), (zoom * 100.).round()),
                    false,
                    zoom != 1.,
                    cx,
                    |this, _, cx| {
                        this.set_user_zoom(1.);
                        cx.notify();
                    },
                ));
                rows.push(Self::divider());
                rows.push(self.iconed_item(
                    Some("key"),
                    "m-logins",
                    t("Logins salvos…").into(),
                    false,
                    true,
                    cx,
                    |this, _, cx| {
                        this.menu = None;
                        this.open_menu(Some(Menu::Logins), cx);
                    },
                ));
                rows.push(Self::divider());
                rows.push(self.iconed_item(
                    Some("trash"),
                    "m-cookies",
                    t("Limpar cookies").into(),
                    false,
                    true,
                    cx,
                    |this, _, cx| this.clear_data(true, cx),
                ));
                rows.push(self.iconed_item(
                    Some("trash"),
                    "m-cache",
                    t("Limpar cache").into(),
                    false,
                    true,
                    cx,
                    |this, _, cx| this.clear_data(false, cx),
                ));
            }
            Menu::NewTab => {
                right = None;
                left = Some(8.);
                width = 220.;
                let default = default_profile();
                let mut ids = vec![
                    browser_profiles::DEFAULT.to_owned(),
                    browser_profiles::PRIVATE.to_owned(),
                ];
                ids.extend(profiles().into_iter().map(|p| p.id));
                for id in ids {
                    let label = self.profile_name(&id);
                    let checked = id == default;
                    rows.push(self.item(
                        format!("n-{id}"),
                        label,
                        checked,
                        true,
                        cx,
                        move |this, _, cx| {
                            let _ = this.new_tab(None, &id);
                            this.close_menu(cx);
                        },
                    ));
                }
                rows.push(Self::divider());
                rows.push(self.item(
                    "n-manage",
                    t("Gerenciar perfis…").into(),
                    false,
                    true,
                    cx,
                    |this, _, cx| {
                        this.ensure_profile_form(cx);
                        this.open_menu_fresh(Menu::Profiles, cx);
                    },
                ));
            }
            Menu::Profiles => {
                right = None;
                left = Some(8.);
                width = 300.;
                rows.extend(self.profiles_rows(cx));
            }
            Menu::LoginForm => {
                width = 320.;
                rows.extend(self.login_form_rows(cx));
            }
            Menu::Presets => {
                right = None;
                left = Some(8.);
                width = 280.;
                let current = self.device.as_ref().and_then(|d| d.preset.clone());
                for (id, label, w, h) in PRESETS {
                    rows.push(self.item(
                        format!("p-{id}"),
                        format!("{label} · {w} × {h}"),
                        current.as_deref() == Some(id),
                        true,
                        cx,
                        move |this, _, cx| {
                            this.set_device(Some(Device {
                                width: w,
                                height: h,
                                preset: Some(id.into()),
                            }));
                            this.close_menu(cx);
                        },
                    ));
                }
            }
            Menu::Fill => {
                for (i, login) in self.matches.iter().enumerate() {
                    let id = login["id"].as_str().unwrap_or("").to_owned();
                    let name = login["username"].as_str().unwrap_or("").to_owned();
                    let (fill_id, code_id) = (id.clone(), id);
                    rows.push(self.item(
                        format!("f-{i}"),
                        tf("Preencher {}", &[&name]),
                        false,
                        true,
                        cx,
                        move |this, _, cx| this.fill(&fill_id, "login", cx),
                    ));
                    if login["hasTotp"].as_bool() == Some(true) {
                        rows.push(self.item(
                            format!("f-code-{i}"),
                            tf("Preencher o código de {}", &[&name]),
                            false,
                            true,
                            cx,
                            move |this, _, cx| this.fill(&code_id, "totp", cx),
                        ));
                    }
                }
            }
            Menu::Logins => {
                width = 320.;
                if self.logins.is_empty() {
                    rows.push(self.item(
                        "l-none",
                        t("Nenhum login salvo.").into(),
                        false,
                        false,
                        cx,
                        |_, _, _| {},
                    ));
                }
                for (i, login) in self.logins.iter().enumerate() {
                    let id = login["id"].as_str().unwrap_or("").to_owned();
                    let site =
                        crate::browser_bridge::site_label(login["origin"].as_str().unwrap_or(""));
                    let name = login["username"].as_str().unwrap_or("").to_owned();
                    let agent = login["agentUse"].as_bool() == Some(true);
                    let (toggle_id, delete_id) = (id.clone(), id);
                    let edit_login = login.clone();
                    let delete_site = site.clone();
                    rows.push(
                        div()
                            .id(("login-row", i))
                            .min_h(px(34.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.5))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_color(theme::content(0.92))
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(11.))
                                            .text_color(theme::content(0.5))
                                            .child(site),
                                    ),
                            )
                            .child(
                                div()
                                    .id(("login-agent", i))
                                    .h(px(22.))
                                    .px(px(7.))
                                    .rounded(px(crate::theme::RADIUS_SM))
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .text_size(px(11.))
                                    .text_color(if agent {
                                        theme::on_accent()
                                    } else {
                                        theme::content(0.7)
                                    })
                                    .bg(if agent {
                                        theme::accent()
                                    } else {
                                        theme::content(0.08)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.backend.invoke(
                                            "login_vault_set_agent_use",
                                            json!({ "id": toggle_id, "enabled": !agent }),
                                        );
                                        if let Some(row) = this
                                            .logins
                                            .iter_mut()
                                            .find(|l| l["id"] == toggle_id.as_str())
                                        {
                                            row["agentUse"] = json!(!agent);
                                        }
                                        cx.notify();
                                    }))
                                    .child(t("Agentes")),
                            )
                            .child(self.icon_button(
                                ("login-edit", i),
                                "pencil",
                                cx,
                                move |this, _, cx| this.open_login_form(Some(edit_login.clone()), cx),
                            ))
                            .child(self.icon_button(
                                ("login-delete", i),
                                "trash",
                                cx,
                                move |_, window, cx| {
                                    let (id, entity) = (delete_id.clone(), cx.entity());
                                    ask(
                                        window,
                                        cx,
                                        tf("Excluir o login de {}?", &[&delete_site]),
                                        t("A senha é removida do Chaveiro. Isso não pode ser desfeito.")
                                            .to_owned(),
                                        t("Excluir"),
                                        move |cx| {
                                            entity.update(cx, |this, cx| this.delete_login(&id, cx))
                                        },
                                    );
                                },
                            ))
                            .into_any_element(),
                    );
                }
                rows.push(Self::divider());
                rows.push(self.item(
                    "l-add",
                    t("Adicionar login").into(),
                    false,
                    true,
                    cx,
                    |this, _, cx| this.open_login_form(None, cx),
                ));
            }
        }
        if rows.is_empty() {
            return None;
        }
        let since = self.menu_since;
        Some(
            div()
                .absolute()
                .top(px(2.))
                .when_some(right, |d, v| d.right(px(v)))
                .when_some(left, |d, v| d.left(px(v)))
                .w(px(width))
                .max_h(px(420.))
                .overflow_hidden()
                .p(px(4.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .flex()
                .flex_col()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(rows)
                .with_animation(
                    SharedString::from(format!("menu:{:?}", since)),
                    Animation::new(Duration::from_millis(120)),
                    |el, t| el.opacity(t),
                )
                .into_any_element(),
        )
    }

    /// A small icon button inside a menu row.
    fn icon_button(
        &self,
        id: impl Into<gpui::ElementId>,
        icon: &'static str,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .size(px(22.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .sinks()
            .hover(|s| s.bg(theme::content(0.1)))
            .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
            .child(theme::icon(icon, 13.).text_color(theme::content(0.6)))
    }

    /// A small text button (the form's Save and Cancel).
    fn text_button(
        &self,
        id: &'static str,
        label: &'static str,
        main: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .h(px(24.))
            .px(px(10.))
            .rounded(px(crate::theme::RADIUS_SM))
            .flex()
            .items_center()
            .cursor_pointer()
            .sinks()
            .text_size(px(12.))
            .text_color(if main {
                theme::on_accent()
            } else {
                theme::content(0.85)
            })
            .bg(if main {
                theme::accent()
            } else {
                theme::content(0.08)
            })
            .hover(|s| s.opacity(0.88))
            .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
            .child(t(label))
    }

    /// A caption over a field in a form.
    fn caption(text: &str) -> AnyElement {
        div()
            .px(px(10.))
            .pt(px(6.))
            .pb(px(2.))
            .text_size(px(11.))
            .text_color(theme::content(0.5))
            .child(text.to_owned())
            .into_any_element()
    }

    /// A text field inside a form row (its own id keeps hover and focus state apart).
    fn field_row(id: &'static str, field: &Entity<TextField>) -> AnyElement {
        div()
            .id(id)
            .px(px(8.))
            .child(field.clone())
            .into_any_element()
    }

    fn profiles_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![Self::caption(t("Novas abas abrem em"))];
        let default = default_profile();
        let custom = profiles();
        let mut ids = vec![
            browser_profiles::DEFAULT.to_owned(),
            browser_profiles::PRIVATE.to_owned(),
        ];
        ids.extend(custom.iter().map(|p| p.id.clone()));
        let renaming = self.profile_form.as_ref().and_then(|f| f.renaming.clone());
        for (i, id) in ids.into_iter().enumerate() {
            let built_in = i < 2;
            let name = self.profile_name(&id);
            if renaming.as_deref() == Some(id.as_str())
                && let Some(form) = &self.profile_form
            {
                rows.push(Self::field_row("profile-rename", &form.rename));
                continue;
            }
            let checked = id == default;
            let (pick_id, rename_id, delete_id) = (id.clone(), id.clone(), id.clone());
            let delete_name = name.clone();
            let mut row = div()
                .id(("profile-row", i))
                .h(px(28.))
                .px(px(10.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.5))
                .text_color(theme::content(0.9))
                .cursor_pointer()
                .hover(|s| s.bg(theme::content(0.08)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.save_profiles(profiles(), pick_id.clone(), cx);
                }))
                .child(div().flex_1().min_w_0().truncate().child(name))
                .when(checked, |d| {
                    d.child(theme::icon("check", 12.).text_color(theme::accent()))
                });
            if !built_in {
                row = row
                    .child(self.icon_button(
                        ("profile-rename-button", i),
                        "pencil",
                        cx,
                        move |this, _, cx| this.start_rename(&rename_id, cx),
                    ))
                    .child(self.icon_button(
                        ("profile-delete", i),
                        "trash",
                        cx,
                        move |_, window, cx| {
                            let (id, entity) = (delete_id.clone(), cx.entity());
                            ask(
                                window,
                                cx,
                                tf("Excluir {}?", &[&delete_name]),
                                t("Os cookies, logins e dados dos sites são removidos deste Mac, e as abas dele fecham.")
                                    .to_owned(),
                                t("Excluir perfil"),
                                move |cx| delete_profile(id, entity, cx),
                            );
                        },
                    ));
            }
            rows.push(row.into_any_element());
        }
        rows.push(Self::divider());
        if let Some(form) = &self.profile_form {
            rows.push(Self::caption(t("Adicionar perfil")));
            rows.push(Self::field_row("profile-name", &form.name));
            if !browser_profiles::supported() {
                rows.push(Self::caption(t(
                    "Perfis personalizados precisam do macOS 14 ou mais recente.",
                )));
            }
            if let Some(error) = &form.error {
                rows.push(
                    div()
                        .px(px(10.))
                        .pt(px(4.))
                        .text_size(px(11.))
                        .text_color(danger())
                        .child(error.clone())
                        .into_any_element(),
                );
            }
            rows.push(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .flex()
                    .child(self.text_button(
                        "profile-add",
                        "Adicionar perfil",
                        true,
                        cx,
                        |this, _, cx| this.add_profile(cx),
                    ))
                    .into_any_element(),
            );
        }
        rows
    }

    fn login_form_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(form) = &self.login_form else {
            return Vec::new();
        };
        let mut rows = vec![
            Self::caption(t("Site")),
            Self::field_row("login-site", &form.site),
            Self::caption(t("Usuário ou e-mail")),
            Self::field_row("login-user", &form.username),
            Self::caption(t("Senha")),
            Self::field_row("login-password", &form.password),
            Self::caption(t("Segredo do autenticador (opcional)")),
            Self::field_row("login-totp", &form.totp),
        ];
        let on = form.agent_use;
        rows.push(
            div()
                .id("login-agent-use")
                .mt(px(6.))
                .px(px(10.))
                .h(px(28.))
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(theme::content(0.85))
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(form) = &mut this.login_form {
                        form.agent_use = !form.agent_use;
                    }
                    cx.notify();
                }))
                .child(div().flex_1().child(t("Agentes podem usar este login")))
                .child(
                    div()
                        .w(px(28.))
                        .h(px(16.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .px(px(2.))
                        .when(on, |d| d.justify_end())
                        .bg(if on {
                            theme::accent()
                        } else {
                            theme::content(0.18)
                        })
                        .child(div().size(px(12.)).rounded_full().bg(gpui::white())),
                )
                .into_any_element(),
        );
        if let Some(error) = &form.error {
            rows.push(
                div()
                    .px(px(10.))
                    .pt(px(4.))
                    .text_size(px(11.))
                    .text_color(danger())
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        let busy = form.busy;
        rows.push(
            div()
                .px(px(8.))
                .py(px(8.))
                .flex()
                .gap(px(6.))
                .child(self.text_button(
                    "login-save",
                    "Salvar login",
                    true,
                    cx,
                    move |this, _, cx| {
                        if !busy {
                            this.save_login(cx)
                        }
                    },
                ))
                .child(
                    self.text_button("login-cancel", "Cancelar", false, cx, |this, _, cx| {
                        this.login_form = None;
                        this.open_menu_fresh(Menu::Logins, cx);
                    }),
                )
                .into_any_element(),
        );
        rows
    }

    fn offer_bar(&self) -> Option<AnyElement> {
        let offer = crate::browser_bridge::offers_for(&self.session_id)
            .into_iter()
            .next()?;
        let id = offer["id"].as_str().unwrap_or("").to_owned();
        let update = offer["update"].as_bool() == Some(true);
        let site = crate::browser_bridge::site_label(offer["origin"].as_str().unwrap_or(""));
        let title = tf(
            if update {
                "Atualizar a senha salva de {}?"
            } else {
                "Salvar login de {}?"
            },
            &[&site],
        );
        let name = offer["username"].as_str().unwrap_or("").to_owned();
        let button = |key: &'static str,
                      label: &'static str,
                      main: bool,
                      id: String,
                      save: bool,
                      agents: bool| {
            div()
                .id(key)
                .h(px(24.))
                .px(px(9.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(if main {
                    theme::on_accent()
                } else {
                    theme::content(0.85)
                })
                .bg(if main {
                    theme::accent()
                } else {
                    theme::content(0.08)
                })
                .hover(|s| s.opacity(0.88))
                .on_click(move |_, _, cx| {
                    crate::browser_bridge::answer_offer(&id, save, agents, cx)
                })
                .child(t(label))
        };
        Some(
            div()
                .mx(px(8.))
                .mb(px(4.))
                .p(px(10.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .bg(theme::field_fill())
                .flex()
                .items_start()
                .gap(px(10.))
                .child(
                    theme::icon("browser-lock", 14.)
                        .text_color(theme::content(0.6))
                        .mt(px(1.)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme::content(0.95))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(theme::content(0.5))
                                .child(tf("Usuário: {}. Guardado no Chaveiro do macOS.", &[&name])),
                        )
                        .child(
                            div()
                                .mt(px(6.))
                                .flex()
                                .flex_wrap()
                                .gap(px(6.))
                                .child(button(
                                    "offer-save",
                                    if update { "Atualizar" } else { "Salvar" },
                                    true,
                                    id.clone(),
                                    true,
                                    false,
                                ))
                                .when(!update, |d| {
                                    d.child(button(
                                        "offer-agents",
                                        "Salvar, agentes podem usar",
                                        false,
                                        id.clone(),
                                        true,
                                        true,
                                    ))
                                })
                                .child(button("offer-no", "Agora não", false, id, false, false)),
                        ),
                )
                .into_any_element(),
        )
    }

    fn dialog_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.dialog.as_ref()?;
        let title = match dialog.kind {
            "alert" => t("A página diz"),
            "confirm" => t("A página pede confirmação"),
            _ => t("A página pede um valor"),
        };
        let prompt = dialog.kind == "prompt";
        let alert = dialog.kind == "alert";
        let button = |id: &'static str, label: &'static str, main: bool| {
            div()
                .id(id)
                .h(px(24.))
                .px(px(10.))
                .rounded(px(crate::theme::RADIUS_SM))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(if main {
                    theme::on_accent()
                } else {
                    theme::content(0.85)
                })
                .bg(if main {
                    theme::accent()
                } else {
                    theme::content(0.08)
                })
                .hover(|s| s.opacity(0.88))
                .child(t(label))
        };
        Some(
            div()
                .mx(px(8.))
                .mb(px(4.))
                .p(px(10.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .bg(theme::field_fill())
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme::content(0.5))
                        .child(format!("{title} · {}", dialog.url)),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(theme::content(0.95))
                        .child(dialog.message.clone()),
                )
                .when(prompt, |d| {
                    d.child(
                        div()
                            .h(px(24.))
                            .px(px(8.))
                            .rounded(px(crate::theme::RADIUS_SM))
                            .bg(theme::content(0.07))
                            .flex()
                            .items_center()
                            .text_size(px(12.))
                            .text_color(theme::content(0.9))
                            .child(if self.dialog_text.is_empty() {
                                t("(vazio)").to_owned()
                            } else {
                                self.dialog_text.clone()
                            }),
                    )
                })
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(6.))
                        .when(!alert, |d| {
                            d.child(button("dialog-cancel", "Cancelar", false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.answer_dialog(false, None);
                                    cx.notify();
                                }),
                            ))
                        })
                        .child(button("dialog-ok", "OK", true).on_click(cx.listener(
                            |this, _, _, cx| {
                                let text = this.dialog_text.clone();
                                this.answer_dialog(true, Some(&text));
                                cx.notify();
                            },
                        ))),
                )
                .into_any_element(),
        )
    }

    fn device_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let device = self.device.as_ref()?;
        let label = device
            .preset
            .as_deref()
            .and_then(|id| PRESETS.iter().find(|p| p.0 == id))
            .map_or(t("Tamanho personalizado").to_owned(), |p| p.1.to_owned());
        Some(
            div()
                .h(px(30.))
                .w_full()
                .flex_shrink_0()
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .id("device-preset")
                        .h(px(24.))
                        .px(px(8.))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .cursor_pointer()
                        .bg(theme::content(0.07))
                        .hover(|s| s.bg(theme::content(0.12)))
                        .text_size(px(12.))
                        .text_color(theme::content(0.9))
                        .on_click(
                            cx.listener(|this, _, _, cx| this.open_menu(Some(Menu::Presets), cx)),
                        )
                        .child(label)
                        .child(theme::icon("chevron-down", 10.).text_color(theme::content(0.5))),
                )
                .when_some(self.device_inputs.as_ref(), |d, inputs| {
                    d.child(
                        div()
                            .id("device-width")
                            .w(px(58.))
                            .child(inputs.width.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::content(0.5))
                            .child("×"),
                    )
                    .child(
                        div()
                            .id("device-height")
                            .w(px(58.))
                            .child(inputs.height.clone()),
                    )
                })
                .child(self.button(
                    "device-rotate",
                    "browser-rotate",
                    false,
                    true,
                    false,
                    cx,
                    |this, _, cx| {
                        if let Some(d) = this.device.take() {
                            this.set_device(Some(Device::from_size(d.height, d.width)));
                        }
                        cx.notify();
                    },
                ))
                .child(div().flex_1())
                .child(self.button(
                    "device-close",
                    "close",
                    false,
                    true,
                    false,
                    cx,
                    |this, _, cx| {
                        this.set_device(None);
                        cx.notify();
                    },
                ))
                .into_any_element(),
        )
    }
}

// ---- Picture in picture, recording, suggestions, device sizes, profiles and the login form ----

/// Hands `f` the window's root view, the shell (the popup for confirmations, the settings).
fn with_shell(
    window: &mut Window,
    cx: &mut App,
    f: impl FnOnce(&mut crate::shell::Shell, &mut Context<crate::shell::Shell>),
) {
    if let Some(Some(shell)) = window.root::<crate::shell::Shell>() {
        shell.update(cx, f);
    }
}

/// Asks in the shell's popup; `action` runs only on yes.
fn ask(
    window: &mut Window,
    cx: &mut App,
    title: String,
    message: String,
    label: &'static str,
    action: impl FnOnce(&mut App) + 'static,
) {
    with_shell(window, cx, |shell, cx| {
        shell.confirm(title, message, label, action, cx)
    });
}

fn error_text(error: &Value) -> String {
    error["message"]
        .as_str()
        .or(error.as_str())
        .unwrap_or("")
        .to_owned()
}

const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

impl Browser {
    // ---- Picture in picture ----

    /// Puts the floating tab back into the pane (or just closes the window).
    fn dismiss_pip(&mut self) {
        let Some(pip) = self.pip.take() else {
            return;
        };
        pip.dismiss();
        if let Some(tab) = self.tabs.iter().find(|t| t.id == pip.tab_id) {
            tab.web.removeFromSuperview();
            tab.web.setHidden(true);
            if let Some(parent) = self.shared.parent.borrow().as_ref() {
                parent.addSubview(&tab.web);
            }
        }
        self.shared.applied_zoom.set(f64::NAN);
        if let Some(active) = self.active.clone() {
            self.select(&active);
        }
    }

    fn toggle_pip(&mut self, cx: &mut Context<Self>) {
        if self.pip.is_some() {
            self.dismiss_pip();
            cx.notify();
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some((id, web, title)) = self.active_tab().filter(|t| !blank_url(&t.url)).map(|t| {
            (
                t.id.clone(),
                t.web.clone(),
                if t.title.is_empty() {
                    t.url.clone()
                } else {
                    t.title.clone()
                },
            )
        }) else {
            return;
        };
        if self.annotating {
            let _ = browser_tab::evaluate(&web, browser_js::ANNOTATE_CANCEL);
            self.annotating = false;
        }
        self.pip = Some(Pip::open(&web, &id, &title, self.events.clone(), mtm));
        unsafe { web.setPageZoom(self.user_zoom) };
        self.select(&id);
        cx.notify();
    }

    // ---- Recording ----

    /// Starts recording the active tab. The answer is `{recording: true}` once the first frame is
    /// in, or why it could not start.
    pub fn start_recording(&mut self, cx: &mut Context<Self>) -> Answer<Value> {
        let (sender, receiver) = async_channel::bounded(1);
        let fail = |message: &str| {
            let _ = sender.try_send(Err(message.to_owned()));
        };
        if self.recording.is_some() {
            fail("This browser is already recording.");
            return receiver;
        }
        let Some((id, web)) = self
            .active_tab()
            .filter(|t| !blank_url(&t.url))
            .map(|t| (t.id.clone(), t.web.clone()))
        else {
            fail("Open a page with browser_open first.");
            return receiver;
        };
        let Some(mtm) = MainThreadMarker::new() else {
            fail("the browser must run on the main thread");
            return receiver;
        };
        let control = Rc::new(RecordControl::default());
        let settings = self.backend.request("load_state", json!({}));
        let task_control = control.clone();
        let task = cx.spawn(async move |this, cx| {
            let control = task_control;
            let abandon = |message: String, cx: &mut gpui::AsyncApp| {
                let _ = sender.try_send(Err(message));
                let _ = this.update(cx, |this, cx| {
                    this.recording = None;
                    cx.notify();
                });
            };
            let options = match settings.recv().await {
                Ok(Ok(state)) => browser_record::Options::from_settings(&state["settings"]),
                _ => browser_record::Options::from_settings(&Value::Null),
            };
            let path = match browser_record::new_path() {
                Ok(path) => path,
                Err(error) => return abandon(error, cx),
            };
            let recorder = browser_record::Recorder::new(path, options);
            let first = browser_record::capture(&recorder, &web, mtm)
                .recv()
                .with_timeout(FRAME_TIMEOUT, cx.background_executor())
                .await;
            if let Err(error) = flatten(first) {
                let _ = browser_record::finish(&recorder);
                return abandon(error, cx);
            }
            let _ = sender.try_send(Ok(json!({ "recording": true })));
            let begun = Instant::now();
            let mut last_events: Option<Instant> = None;
            loop {
                if control.stop.get().is_some() || begun.elapsed() > browser_record::MAX_RECORDING {
                    break;
                }
                // The tab closing ends the recording with what it has.
                if !this
                    .update(cx, |this, _| this.tabs.iter().any(|t| t.id == id))
                    .unwrap_or(false)
                {
                    break;
                }
                let tick = Instant::now();
                if (options.keys || options.clicks)
                    && last_events.is_none_or(|at| at.elapsed() >= browser_record::EVENTS_EVERY)
                {
                    last_events = Some(Instant::now());
                    let read = browser_tab::evaluate(&web, browser_record::INPUT_SCRIPT)
                        .recv()
                        .with_timeout(Duration::from_secs(1), cx.background_executor())
                        .await;
                    if let Ok(Ok(Ok(raw))) = read {
                        recorder.borrow_mut().note_events(&raw);
                    }
                }
                let frame = browser_record::capture(&recorder, &web, mtm)
                    .recv()
                    .with_timeout(FRAME_TIMEOUT, cx.background_executor())
                    .await;
                if flatten(frame).is_err() {
                    break;
                }
                let wait = options.interval().saturating_sub(tick.elapsed());
                if !wait.is_zero() {
                    cx.background_executor().timer(wait).await;
                }
            }
            let finished = browser_record::finish(&recorder)
                .recv()
                .with_timeout(Duration::from_secs(30), cx.background_executor())
                .await;
            let reveal = control.stop.get().unwrap_or(false);
            let result = match flatten(finished) {
                Ok(Some(path)) => Ok(json!({ "path": path.to_string_lossy(), "saved": true })),
                Ok(None) => Ok(json!({ "path": Value::Null, "saved": false })),
                Err(error) => Err(error),
            };
            for waiter in control.waiters.take() {
                let _ = waiter.try_send(result.clone());
            }
            let _ = this.update(cx, |this, cx| {
                this.recording = None;
                match &result {
                    Ok(value) if value["saved"] == true => {
                        if reveal && let Some(path) = value["path"].as_str() {
                            let _ = std::process::Command::new("open")
                                .arg("-R")
                                .arg(path)
                                .spawn();
                        }
                        this.say(t("Gravação salva em Filmes › Sirus"), cx);
                    }
                    Err(error) => this.say(error, cx),
                    _ => {}
                }
                cx.notify();
            });
        });
        self.recording = Some(Recording {
            control,
            _task: task,
        });
        cx.notify();
        receiver
    }

    /// Ends the recording. The answer is the file's path (`reveal` shows it in Finder afterwards).
    pub fn stop_recording(&mut self, reveal: bool) -> Answer<Value> {
        let (sender, receiver) = async_channel::bounded(1);
        match &self.recording {
            None => {
                let _ = sender.try_send(Err("This browser is not recording.".to_owned()));
            }
            Some(recording) => {
                recording.control.waiters.borrow_mut().push(sender);
                if recording.control.stop.get().is_none() {
                    recording.control.stop.set(Some(reveal));
                }
            }
        }
        receiver
    }

    fn toggle_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            let _ = self.stop_recording(true);
            return;
        }
        let started = self.start_recording(cx);
        cx.spawn(async move |this, cx| {
            if let Ok(Err(error)) = started.recv().await {
                let _ = this.update(cx, |this, cx| this.say(&error, cx));
            }
        })
        .detach();
    }

    // ---- The blank page's local servers ----

    fn refresh_servers(&mut self, cx: &mut Context<Self>) {
        let reply = self.backend.request(
            "local_servers_list",
            json!({ "sessionId": self.session_id }),
        );
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(list)) = reply.recv().await {
                let _ = this.update(cx, |this, cx| {
                    let servers = browser_address::local_servers(&list);
                    if servers != this.servers {
                        this.servers = servers;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    // ---- The address bar's suggestions ----

    /// The rows under the address field: what was typed narrows them; text that is just the
    /// page's own address counts as nothing typed.
    fn suggestion_rows(&self) -> Vec<Suggestion> {
        let active = self.active_tab().map(|t| t.url.clone()).unwrap_or_default();
        let query = if self.text == active {
            ""
        } else {
            self.text.as_str()
        };
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .map(|t| (t.id.clone(), t.url.clone(), t.title.clone()))
            .collect();
        browser_address::suggestions(query, &tabs, &self.history, &active)
    }

    fn close_suggestions(&mut self) {
        self.suggest_open = false;
        self.suggest_row = None;
    }

    fn pick_suggestion(&mut self, suggestion: &Suggestion, cx: &mut Context<Self>) {
        self.close_suggestions();
        match suggestion {
            Suggestion::Tab { id, .. } => self.select(id),
            other => {
                if let Some(url) = browser_address::suggestion_url(other) {
                    self.text = url.clone();
                    self.move_to(self.text.len(), false);
                    self.navigate_active(&url);
                }
            }
        }
        cx.notify();
    }

    // ---- Device mode ----

    /// The two size fields, built the first time device mode shows its bar.
    fn ensure_device_inputs(&mut self, cx: &mut Context<Self>) {
        if self.device_inputs.is_some() {
            return;
        }
        let make = |placeholder: &'static str, cx: &mut Context<Self>| {
            let field = cx.new(|cx| TextField::new(placeholder, cx));
            let sub = cx.subscribe(
                &field,
                |this, field, event: &TextFieldEvent, cx| match event {
                    TextFieldEvent::Changed => {
                        let text = field.read(cx).text().to_owned();
                        let digits = browser_device::digits(&text);
                        if digits != text {
                            field.update(cx, |f, cx| f.set_text(&digits, cx));
                        }
                    }
                    TextFieldEvent::Submit => this.commit_device_size(cx),
                    TextFieldEvent::Cancel => {
                        this.device_inputs_stale = true;
                        cx.notify();
                    }
                },
            );
            (field, sub)
        };
        let (width, a) = make("W", cx);
        let (height, b) = make("H", cx);
        self.device_inputs = Some(DeviceInputs {
            width,
            height,
            _subs: vec![a, b],
        });
        self.device_inputs_stale = true;
    }

    /// Writes the device's size into the fields, unless one is being typed in.
    fn sync_device_inputs(&mut self, window: &Window, cx: &mut Context<Self>) {
        let (Some(inputs), Some(device)) = (&self.device_inputs, &self.device) else {
            return;
        };
        for (field, value) in [
            (&inputs.width, device.width),
            (&inputs.height, device.height),
        ] {
            if !field.read(cx).focus_handle().is_focused(window) {
                field.update(cx, |f, cx| f.set_text(&value.to_string(), cx));
            }
        }
        self.device_inputs_stale = false;
    }

    /// Applies the typed width and height (Enter).
    fn commit_device_size(&mut self, cx: &mut Context<Self>) {
        let Some(inputs) = &self.device_inputs else {
            return;
        };
        let (w, h) = (
            browser_device::parse_side(inputs.width.read(cx).text()),
            browser_device::parse_side(inputs.height.read(cx).text()),
        );
        if let (Some(w), Some(h), Some(device)) = (w, h, &self.device) {
            let next = Device::from_size(w, h);
            if next.width != device.width || next.height != device.height {
                self.set_device(Some(next));
            }
        }
        self.device_inputs_stale = true;
        cx.notify();
    }

    fn drag_to(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let Some(drag) = &self.drag else {
            return;
        };
        // GPUI's pixels to the window points the frame is measured in.
        let zoom = window.content_zoom() as f64;
        let dx = f64::from(position.x - drag.from.x) * zoom;
        let dy = f64::from(position.y - drag.from.y) * zoom;
        let (w, h) = browser_device::dragged(drag.edge, drag.start, dx, dy, drag.scale);
        if self
            .device
            .as_ref()
            .is_some_and(|d| (d.width, d.height) != (w, h))
        {
            self.set_device(Some(Device::from_size(w, h)));
            self.device_inputs_stale = true;
            cx.notify();
        }
    }

    // ---- Browser profiles ----

    fn profile_name(&self, id: &str) -> String {
        match id {
            browser_profiles::DEFAULT => t("Padrão").to_owned(),
            browser_profiles::PRIVATE => t("Privado").to_owned(),
            other => profiles()
                .into_iter()
                .find(|p| p.id == other)
                .map_or_else(|| other.to_owned(), |p| p.name),
        }
    }

    fn ensure_profile_form(&mut self, cx: &mut Context<Self>) {
        if self.profile_form.is_some() {
            return;
        }
        let name = cx.new(|cx| TextField::new(t("Nome do perfil"), cx));
        let rename = cx.new(|cx| TextField::new(t("Nome do perfil"), cx));
        let a = cx.subscribe(&name, |this, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Submit => this.add_profile(cx),
            TextFieldEvent::Cancel => this.close_menu(cx),
            TextFieldEvent::Changed => {
                if let Some(form) = &mut this.profile_form {
                    form.error = None;
                }
                cx.notify();
            }
        });
        let b = cx.subscribe(&rename, |this, _, event: &TextFieldEvent, cx| match event {
            TextFieldEvent::Submit => this.commit_rename(cx),
            TextFieldEvent::Cancel => {
                if let Some(form) = &mut this.profile_form {
                    form.renaming = None;
                    form.error = None;
                }
                cx.notify();
            }
            TextFieldEvent::Changed => {
                if let Some(form) = &mut this.profile_form {
                    form.error = None;
                }
                cx.notify();
            }
        });
        self.profile_form = Some(ProfileForm {
            name,
            rename,
            renaming: None,
            error: None,
            _subs: vec![a, b],
        });
    }

    fn profile_error(&mut self, message: &'static str, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.profile_form {
            form.error = Some(t(message).to_owned());
        }
        cx.notify();
    }

    /// Saves the list through the shell, which keeps the settings (and tells the host).
    fn save_profiles(&mut self, list: Vec<Profile>, default: String, cx: &mut Context<Self>) {
        let json = browser_profiles::to_json(&list);
        let Some(window) = cx.windows().first().copied() else {
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            with_shell(window, cx, |shell, cx| {
                shell.save_browser_profiles(json, &default, cx)
            })
        });
        cx.notify();
    }

    fn add_profile(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.profile_form else {
            return;
        };
        let name = form.name.read(cx).text().trim().to_owned();
        let mut list = profiles();
        if !browser_profiles::supported() {
            return self.profile_error(
                "Perfis personalizados precisam do macOS 14 ou mais recente.",
                cx,
            );
        }
        if list.len() >= browser_profiles::MAX_PROFILES {
            return self.profile_error("Mantenha no máximo 10 perfis personalizados.", cx);
        }
        if browser_profiles::name_problem(&list, &name, None) {
            return self.profile_error(
                "Use um nome curto que ainda não existe, nem Padrão nem Privado.",
                cx,
            );
        }
        list.push(Profile {
            id: browser_profiles::new_id(),
            name,
        });
        self.save_profiles(list, default_profile(), cx);
        if let Some(form) = &self.profile_form {
            form.name.update(cx, |f, cx| f.set_text("", cx));
        }
    }

    fn start_rename(&mut self, id: &str, cx: &mut Context<Self>) {
        let name = profiles()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.name)
            .unwrap_or_default();
        if let Some(form) = &mut self.profile_form {
            form.renaming = Some(id.to_owned());
            form.error = None;
            form.rename.update(cx, |f, cx| {
                f.set_text(&name, cx);
                f.focus(cx);
            });
        }
        cx.notify();
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some((id, name)) = self.profile_form.as_ref().and_then(|f| {
            Some((
                f.renaming.clone()?,
                f.rename.read(cx).text().trim().to_owned(),
            ))
        }) else {
            return;
        };
        let mut list = profiles();
        let unchanged = list.iter().any(|p| p.id == id && p.name == name);
        if !unchanged && browser_profiles::name_problem(&list, &name, Some(&id)) {
            return self.profile_error(
                "Use um nome curto que ainda não existe, nem Padrão nem Privado.",
                cx,
            );
        }
        if let Some(profile) = list.iter_mut().find(|p| p.id == id) {
            profile.name = name;
        }
        if let Some(form) = &mut self.profile_form {
            form.renaming = None;
        }
        self.save_profiles(list, default_profile(), cx);
    }

    /// Closes every tab of a profile (before its data goes).
    fn close_profile_tabs(&mut self, id: &str) {
        let doomed: Vec<String> = self
            .tabs
            .iter()
            .filter(|t| t.profile == id)
            .map(|t| t.id.clone())
            .collect();
        for tab in doomed {
            self.close_tab(&tab);
        }
    }

    // ---- Saved logins ----

    fn open_login_form(&mut self, edit: Option<Value>, cx: &mut Context<Self>) {
        let editing = edit.is_some();
        let field = |placeholder: &'static str, cx: &mut Context<Self>| {
            let field = cx.new(|cx| TextField::new(placeholder, cx));
            let sub = cx.subscribe(&field, |this, _, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Submit => this.save_login(cx),
                TextFieldEvent::Cancel => this.open_menu_fresh(Menu::Logins, cx),
                TextFieldEvent::Changed => {
                    if let Some(form) = &mut this.login_form {
                        form.error = None;
                    }
                }
            });
            (field, sub)
        };
        let (site, a) = field(t("https://exemplo.com"), cx);
        let (username, b) = field(t("Usuário ou e-mail"), cx);
        let (password, c) = field(
            if editing {
                t("Deixe vazio para manter a senha salva")
            } else {
                t("Senha")
            },
            cx,
        );
        let (totp, d) = field(
            if editing {
                t("Salvo. Digite outro para trocar, ou um traço para remover")
            } else {
                t("Segredo do autenticador (opcional)")
            },
            cx,
        );
        password.update(cx, |f, _| f.set_masked(true));
        let id = edit
            .as_ref()
            .and_then(|l| l["id"].as_str())
            .map(str::to_owned);
        if let Some(login) = &edit {
            site.update(cx, |f, cx| {
                f.set_text(login["origin"].as_str().unwrap_or(""), cx)
            });
            username.update(cx, |f, cx| {
                f.set_text(login["username"].as_str().unwrap_or(""), cx)
            });
        }
        site.update(cx, |f, cx| f.focus(cx));
        self.login_form = Some(LoginForm {
            id,
            site,
            username,
            password,
            totp,
            agent_use: edit.as_ref().is_some_and(|l| l["agentUse"] == true),
            error: None,
            busy: false,
            _subs: vec![a, b, c, d],
        });
        self.open_menu_fresh(Menu::LoginForm, cx);
    }

    /// Shows `menu` even when it is the one on screen (`open_menu` toggles).
    fn open_menu_fresh(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.menu = None;
        self.open_menu(Some(menu), cx);
    }

    /// The form as the host's `login_vault_save` takes it (`loginInput`): an edit keeps the saved
    /// password when the field is empty, a new login needs one; for the authenticator secret
    /// empty keeps it and a lone dash clears it.
    fn login_input(&self, cx: &App) -> Option<Value> {
        let form = self.login_form.as_ref()?;
        let site = form.site.read(cx).text().trim().to_owned();
        let username = form.username.read(cx).text().trim().to_owned();
        let password = form.password.read(cx).text().to_owned();
        let totp = form.totp.read(cx).text().trim().to_owned();
        login_input_value(
            form.id.as_deref(),
            &site,
            &username,
            &password,
            &totp,
            form.agent_use,
        )
    }

    fn save_login(&mut self, cx: &mut Context<Self>) {
        if self.login_form.as_ref().is_none_or(|f| f.busy) {
            return;
        }
        let Some(input) = self.login_input(cx) else {
            if let Some(form) = &mut self.login_form {
                form.error = Some(t("Informe um site, um usuário e uma senha.").to_owned());
            }
            cx.notify();
            return;
        };
        if let Some(form) = &mut self.login_form {
            form.busy = true;
            form.error = None;
        }
        let reply = self
            .backend
            .request("login_vault_save", json!({ "input": input }));
        cx.spawn(async move |this, cx| {
            let result = reply.recv().await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Ok(_)) => {
                    this.login_form = None;
                    this.matches_for.clear();
                    this.open_menu_fresh(Menu::Logins, cx);
                }
                Ok(Err(error)) => {
                    if let Some(form) = &mut this.login_form {
                        form.busy = false;
                        form.error = Some(error_text(&error));
                    }
                    cx.notify();
                }
                Err(_) => {
                    if let Some(form) = &mut this.login_form {
                        form.busy = false;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn delete_login(&mut self, id: &str, cx: &mut Context<Self>) {
        self.backend
            .invoke("login_vault_delete", json!({ "id": id }));
        self.logins.retain(|l| l["id"] != id);
        self.matches_for.clear();
        cx.notify();
    }
}

/// Deletes a custom profile: its tabs close in every conversation's browser, then its data goes
/// from the disk, and only then does the profile leave the settings (a failure keeps it listed).
fn delete_profile(id: String, origin: Entity<Browser>, cx: &mut App) {
    let sessions: Vec<Entity<Browser>> = SESSIONS.with(|s| s.borrow().values().cloned().collect());
    for entity in sessions {
        entity.update(cx, |browser, cx| {
            browser.close_profile_tabs(&id);
            cx.notify();
        });
    }
    let (sender, done) = async_channel::bounded(1);
    let started = MainThreadMarker::new().is_some_and(|mtm| {
        browser_profiles::remove_data(
            &id,
            move |error| {
                let _ = sender.try_send(error);
            },
            mtm,
        )
    });
    cx.spawn(async move |cx| {
        let failure = if started {
            done.recv().await.ok().flatten()
        } else {
            None
        };
        let _ = cx.update(|cx| match failure {
            Some(message) => origin.update(cx, |browser, cx| browser.say(&message, cx)),
            None => {
                let mut list = profiles();
                list.retain(|p| p.id != id);
                let default = if default_profile() == id {
                    browser_profiles::DEFAULT.to_owned()
                } else {
                    default_profile()
                };
                origin.update(cx, |browser, cx| browser.save_profiles(list, default, cx));
            }
        });
    })
    .detach();
}

/// The host's login input from the form's text (`loginInput` in `login-vault.ts`).
fn login_input_value(
    id: Option<&str>,
    site: &str,
    username: &str,
    password: &str,
    totp: &str,
    agent_use: bool,
) -> Option<Value> {
    let (site, username) = (site.trim(), username.trim());
    if site.is_empty() || username.is_empty() || (id.is_none() && password.is_empty()) {
        return None;
    }
    let totp = totp.trim();
    Some(json!({
        "id": id,
        "origin": site,
        "username": username,
        "password": if password.is_empty() { Value::Null } else { json!(password) },
        "totp": match totp { "" => Value::Null, "-" => json!(""), other => json!(other) },
        "agentUse": agent_use,
    }))
}

/// Flattens a timed-out receive of a one-shot answer.
fn flatten<T>(
    received: Result<Result<Result<T, String>, async_channel::RecvError>, gpui::Timeout>,
) -> Result<T, String> {
    match received {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("Could not capture the page.".into()),
        Err(_) => Err("The page did not answer in time.".into()),
    }
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// Attaches files to the conversation on screen, as the composer's clip does.
fn attach(paths: Vec<std::path::PathBuf>, window: &mut Window, cx: &mut App) {
    if let Some(Some(shell)) = window.root::<crate::shell::Shell>() {
        shell.update(cx, |shell, cx| shell.attach_paths(paths, cx));
    }
}

pub struct Annotated {
    pub selector: String,
    pub label: String,
    /// Where the element is in the viewport (CSS pixels): x, y, width, height.
    pub rect: browser_crop::Element,
}

pub struct Annotation {
    pub url: String,
    pub width: f64,
    pub height: f64,
    pub elements: Vec<Annotated>,
}

/// Bounded parse of the measure script's result (at most 20 elements with a selector).
pub fn parse_annotation(raw: &str) -> Annotation {
    let value: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let url = browser_tab::truncate(value["url"].as_str().unwrap_or("").trim(), 2048);
    Annotation {
        url: if browser_tab::allowed_url(&url) {
            url
        } else {
            String::new()
        },
        width: value["width"]
            .as_f64()
            .filter(|v| v.is_finite())
            .unwrap_or(0.)
            .clamp(0., 16384.),
        height: value["height"]
            .as_f64()
            .filter(|v| v.is_finite())
            .unwrap_or(0.)
            .clamp(0., 16384.),
        elements: value["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let selector = e["selector"].as_str()?.trim();
                let side = |key: &str| e[key].as_f64().filter(|v| v.is_finite()).unwrap_or(0.);
                (!selector.is_empty()).then(|| Annotated {
                    selector: browser_tab::truncate(selector, 400),
                    label: browser_tab::truncate(e["label"].as_str().unwrap_or("").trim(), 300),
                    rect: (side("x"), side("y"), side("width"), side("height")),
                })
            })
            .take(20)
            .collect(),
    }
}

/// The screenshot cropped to the marked elements: the rectangle that holds them all, padded and
/// mapped from the viewport's CSS pixels to the picture's pixels (`cropAnnotationPng`).
pub fn crop_annotation(png: &[u8], capture: &Annotation) -> Option<Vec<u8>> {
    let elements: Vec<_> = capture.elements.iter().map(|e| e.rect).collect();
    let image = browser_crop::png_size(png)?;
    let rect = browser_crop::crop_rect(
        &elements,
        (capture.width, capture.height),
        image,
        browser_crop::PADDING,
    )?;
    browser_crop::crop_png(png, rect)
}

/// The text attachment: page, viewport and one numbered line per element (`annotationText`).
pub fn annotation_text(capture: &Annotation) -> String {
    let mut lines = vec![
        if capture.url.is_empty() {
            "Page: (unknown)".to_owned()
        } else {
            format!("Page: {}", capture.url)
        },
        format!(
            "Viewport: {}×{}",
            capture.width.round(),
            capture.height.round()
        ),
        String::new(),
        "Annotated elements:".to_owned(),
    ];
    for (i, e) in capture.elements.iter().enumerate() {
        lines.push(format!("{}. {} — {}", i + 1, e.label, e.selector));
    }
    lines.join("\n")
}

impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.shared.parent.borrow().is_none() {
            self.set_parent(parent_of(window));
        }
        if self.focus_pending {
            self.focus_pending = false;
            window.focus(&self.focus, cx);
        }
        // The panel opening is what opens the browser, as in Sirus.
        if self.tabs.is_empty() && !self.open && self.shared.parent.borrow().is_some() {
            let _ = self.ensure_open();
        }
        self.shared.layout.set(Layout {
            device: self
                .device
                .as_ref()
                .map(|d| (d.width as f64, d.height as f64)),
            user_zoom: self.user_zoom,
        });
        if self.device.is_some() {
            self.ensure_device_inputs(cx);
            if self.device_inputs_stale {
                self.sync_device_inputs(window, cx);
            }
        }
        let focused = self.focus.is_focused(window);
        if !focused && self.suggest_open {
            self.close_suggestions();
        }
        let floating = self
            .pip
            .as_ref()
            .is_some_and(|p| self.active.as_deref() == Some(p.tab_id.as_str()));
        let suggestions = if self.suggest_open && focused {
            self.suggestion_rows()
        } else {
            Vec::new()
        };
        // Native views draw above GPUI: a menu or the suggestion list hides the page.
        self.shared
            .menu
            .set(self.menu.is_some() || !suggestions.is_empty());
        if self.want_servers && self.active_tab().is_some_and(|t| blank_url(&t.url)) {
            self.want_servers = false;
            self.refresh_servers(cx);
        }
        let sel = self.selection();
        let ink = theme::content(0.9);
        let segment = |s: &str| {
            div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .child(s.to_owned())
        };
        let field = div()
            .id("browser-address")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    reclaim_keyboard(window);
                    this.anchor = 0;
                    this.cursor = this.text.len();
                    this.suggest_open = true;
                    this.suggest_row = None;
                    this.close_menu(cx);
                    cx.notify();
                }),
            )
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(9.))
            .overflow_hidden()
            .flex()
            .items_center()
            .text_size(px(12.5))
            .text_color(ink)
            .child(if self.text.is_empty() && !focused {
                div()
                    .text_color(theme::content(0.35))
                    .child(t("Digite um endereço"))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .child(segment(&self.text[..sel.start]))
                    .child(if sel.is_empty() {
                        div()
                            .w(px(1.))
                            .h(px(15.))
                            .flex_shrink_0()
                            .bg(if focused {
                                theme::accent()
                            } else {
                                gpui::transparent_black()
                            })
                            .into_any_element()
                    } else {
                        segment(&self.text[sel.clone()])
                            .bg(theme::text_selection())
                            .into_any_element()
                    })
                    .child(segment(&self.text[sel.end..]))
                    .into_any_element()
            })
            .child({
                let entity = cx.entity();
                let focus = self.focus.clone();
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx);
                    },
                )
                .absolute()
                .size_full()
            });

        let shared = self.shared.clone();
        let frozen = self.frozen.clone().filter(|_| self.shared.menu.get());
        let page = div()
            .relative()
            .size_full()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| paint_web(&shared, bounds, window),
                )
                .size_full(),
            )
            .when_some(frozen, |d, frozen| {
                d.child(
                    gpui::img(frozen)
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .object_fit(gpui::ObjectFit::Contain),
                )
            });

        let active = self.active_tab();
        let (can_back, can_forward, has_page) = (
            active.is_some_and(|t| t.can_back),
            active.is_some_and(|t| t.can_forward),
            active.is_some_and(|t| !blank_url(&t.url)),
        );
        let has_tab = active.is_some();
        let blank = has_tab && !has_page;
        let fit = tools_that_fit(self.shared.width.get());
        // One piece: the address and, at its end, the tools for the page it shows.
        let address = div()
            .flex_1()
            .min_w_0()
            .h(px(30.))
            .pr(px(2.))
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(theme::field_fill())
            .flex()
            .items_center()
            .child(field)
            .when(!self.matches.is_empty(), |d| {
                d.child(self.button(
                    "browser-fill",
                    "key",
                    false,
                    true,
                    self.menu == Some(Menu::Fill),
                    cx,
                    |this, _, cx| this.open_menu(Some(Menu::Fill), cx),
                ))
            })
            .when(fit > LINK_RANK, |d| {
                d.child(self.button(
                    "browser-link",
                    "browser-link",
                    false,
                    has_page,
                    false,
                    cx,
                    |this, _, cx| this.copy_link(cx),
                ))
            })
            .when(fit > SHOT_RANK, |d| {
                d.child(self.button(
                    "browser-shot",
                    "browser-camera",
                    false,
                    has_page,
                    false,
                    cx,
                    |this, window, cx| this.capture_to_composer(window, cx),
                ))
            })
            .when(fit > PICK_RANK, |d| {
                d.child(self.button(
                    "browser-pick",
                    "pointer",
                    false,
                    has_page,
                    self.annotating,
                    cx,
                    |this, window, cx| this.toggle_annotate(window, cx),
                ))
            })
            .when(fit > DEVICE_RANK, |d| {
                d.child(self.button(
                    "browser-device",
                    "smartphone",
                    false,
                    has_page && !floating,
                    self.device.is_some(),
                    cx,
                    |this, _, cx| this.toggle_device(cx),
                ))
            })
            .when(self.recording.is_some() || fit > RECORD_RANK, |d| {
                d.child(self.record_button(has_page, cx))
            })
            .when(self.pip.is_some() || fit > PIP_RANK, |d| {
                d.child(self.button(
                    "browser-pip",
                    "browser-pip",
                    false,
                    self.pip.is_some() || has_page,
                    self.pip.is_some(),
                    cx,
                    |this, _, cx| this.toggle_pip(cx),
                ))
            });
        let toolbar = div()
            .h(px(TOOLBAR))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(8.))
            .child(self.button(
                "browser-back",
                "arrow-left",
                false,
                can_back,
                false,
                cx,
                |this, _, _| {
                    if let Some(tab) = this.active_tab() {
                        unsafe { tab.web.goBack() };
                    }
                },
            ))
            .child(self.button(
                "browser-forward",
                "arrow-left",
                true,
                can_forward,
                false,
                cx,
                |this, _, _| {
                    if let Some(tab) = this.active_tab() {
                        unsafe { tab.web.goForward() };
                    }
                },
            ))
            .child(self.button(
                "browser-reload",
                "refresh",
                false,
                has_tab,
                false,
                cx,
                |this, _, _| {
                    if let Some(tab) = this.active_tab() {
                        unsafe { tab.web.reload() };
                    }
                },
            ))
            .child(div().w(px(4.)).flex_shrink_0())
            .child(address)
            .child(div().w(px(4.)).flex_shrink_0())
            .child(self.button(
                "browser-more",
                "browser-more",
                false,
                true,
                self.menu == Some(Menu::More),
                cx,
                |this, _, cx| this.open_menu(Some(Menu::More), cx),
            ));

        let notice = self.notice.as_ref().map(|(text, _)| text.clone());
        let menu = self.menu_view(cx);
        let start = (blank && !floating).then(|| {
            let column = div()
                .absolute()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .child(theme::icon("globe", 22.).text_color(theme::content(0.4)));
            if self.servers.is_empty() {
                return column.child(
                    div()
                        .text_size(px(12.5))
                        .text_color(theme::content(0.55))
                        .child(t("Comece uma nova página")),
                );
            }
            column
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(theme::content(0.7))
                        .child(t("Servidores locais")),
                )
                .child(
                    div()
                        .w_full()
                        .max_w(px(340.))
                        .px(px(12.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(self.servers.iter().enumerate().map(|(i, server)| {
                            let url = server.url.clone();
                            div()
                                .id(("local-server", i))
                                .h(px(28.))
                                .px(px(10.))
                                .rounded(px(crate::theme::RADIUS_MD))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(8.))
                                .cursor_pointer()
                                .text_size(px(12.5))
                                .text_color(theme::content(0.8))
                                .hover(|s| s.bg(theme::content(0.08)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.navigate_active(&url);
                                    cx.notify();
                                }))
                                .child(div().min_w_0().truncate().child(server.url.clone()))
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .truncate()
                                        .text_size(px(11.))
                                        .text_color(theme::content(0.45))
                                        .child(server.process.clone()),
                                )
                        })),
                )
        });
        let in_window = floating.then(|| {
            div()
                .absolute()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .child(theme::icon("browser-pip", 22.).text_color(theme::content(0.4)))
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(theme::content(0.7))
                        .child(t("Esta página está numa janela flutuante.")),
                )
                .child(
                    div()
                        .id("browser-pip-back")
                        .h(px(26.))
                        .px(px(10.))
                        .rounded(px(crate::theme::RADIUS_MD))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(12.5))
                        .text_color(theme::content(0.8))
                        .hover(|s| s.bg(theme::content(0.08)))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_pip(cx)))
                        .child(t("Trazer de volta ao painel")),
                )
        });
        // The rails around a device frame resize it by dragging (the frame is centred).
        let rails: Vec<AnyElement> = match (self.shared.geometry.get(), &self.device) {
            (Some(g), Some(device)) if !floating => browser_device::EDGES
                .iter()
                .enumerate()
                .map(|(i, edge)| {
                    let edge = *edge;
                    let (x, y, w, h) = browser_device::rail(edge, g.left, g.frame);
                    let start = (device.width, device.height);
                    let rail = div()
                        .id(("device-rail", i))
                        .absolute()
                        .left(px((x / g.zoom) as f32))
                        .top(px((y / g.zoom) as f32))
                        .w(px((w / g.zoom) as f32))
                        .h(px((h / g.zoom) as f32))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.drag = Some(Drag {
                                    edge,
                                    from: event.position,
                                    start,
                                    scale: g.scale,
                                });
                                cx.notify();
                            }),
                        );
                    match edge {
                        Edge::Left | Edge::Right => rail.cursor_col_resize(),
                        Edge::Bottom => rail.cursor_row_resize(),
                        Edge::BottomLeft => rail.cursor_nesw_resize(),
                        Edge::BottomRight => rail.cursor_nwse_resize(),
                    }
                    .into_any_element()
                })
                .collect(),
            _ => Vec::new(),
        };
        let chrome = TOOLBAR + if self.tabs.is_empty() { 0. } else { TAB_STRIP };
        let suggest_el = (!suggestions.is_empty()).then(|| {
            div()
                .absolute()
                .top(px(chrome))
                .left(px(8.))
                .right(px(8.))
                .p(px(4.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .child(crate::theme::floating_surface(crate::theme::RADIUS_LG))
                .shadow_lg()
                .flex()
                .flex_col()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(crate::hover_track::list("address-suggestions").pill())
                .children(suggestions.iter().cloned().enumerate().map(|(i, row)| {
                    let selected = self.suggest_row == Some(i);
                    let title = match &row {
                        Suggestion::Search { query } => tf("Pesquisar por \"{}\"", &[query]),
                        other => other.title().to_owned(),
                    };
                    let detail = row.detail().map(str::to_owned);
                    let icon = if matches!(row, Suggestion::Search { .. }) {
                        "search"
                    } else {
                        "globe"
                    };
                    crate::hover_track::list("address-suggestions").row(
                        div()
                            .id(("suggestion", i))
                            .px(px(10.))
                            .py(px(5.))
                            .rounded(px(crate::theme::RADIUS_MD))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .cursor_pointer()
                            .when(selected, |d| d.bg(theme::content(0.08)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.pick_suggestion(&row, cx);
                                }),
                            )
                            .child(theme::icon(icon, 12.).text_color(theme::content(0.5)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(12.5))
                                            .text_color(theme::content(0.9))
                                            .child(title),
                                    )
                                    .when_some(detail, |d, detail| {
                                        d.child(
                                            div()
                                                .truncate()
                                                .text_size(px(11.))
                                                .text_color(theme::content(0.45))
                                                .child(detail),
                                        )
                                    }),
                            ),
                        i,
                        selected,
                    )
                }))
                .with_animation(
                    "suggestions",
                    Animation::new(Duration::from_millis(100)),
                    |el, t| el.opacity(t),
                )
        });
        let dragging = self.drag.as_ref().map(|d| d.edge);
        let none = (!has_tab).then(|| {
            div()
                .absolute()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .child(theme::icon("globe", 22.).text_color(theme::content(0.4)))
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(theme::content(0.55))
                        .child(t("Nenhuma aba aberta")),
                )
                .child(
                    div()
                        .id("browser-first-tab")
                        .h(px(26.))
                        .px(px(10.))
                        .rounded(px(crate::theme::RADIUS_MD))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(12.5))
                        .text_color(theme::content(0.8))
                        .hover(|s| s.bg(theme::content(0.08)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let _ = this.new_tab(None, &default_profile());
                            cx.notify();
                        }))
                        .child(t("Nova aba")),
                )
        });

        div()
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .when(!self.tabs.is_empty(), |d| d.child(self.tab_strip(cx)))
            .child(toolbar)
            .when_some(self.offer_bar(), |d, bar| d.child(bar))
            .when_some(self.device_bar(cx), |d, bar| d.child(bar))
            .when_some(self.dialog_bar(cx), |d, bar| d.child(bar))
            .child({
                let device = self.device.is_some();
                div()
                    .flex_1()
                    .w_full()
                    .relative()
                    .child(page)
                    .when(device, |d| d.bg(theme::content(0.04)))
                    .children(start)
                    .children(none)
                    .children(in_window)
                    .children(rails)
            })
            .children(suggest_el)
            .when_some(menu, |d, menu| {
                // A click outside closes it; the menu itself stops the click.
                d.child(
                    div()
                        .id("browser-menu-scrim")
                        .absolute()
                        .size_full()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.menu_pressed_shut =
                                    this.menu.clone().map(|menu| (menu, Instant::now()));
                                this.close_menu(cx);
                            }),
                        )
                        .child(
                            div()
                                .absolute()
                                .top(px(chrome))
                                .right_0()
                                .left_0()
                                .child(menu),
                        ),
                )
            })
            .when_some(notice, |d, text| {
                d.child(
                    div()
                        .absolute()
                        .top(px(chrome + 4.))
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .max_w(px(320.))
                                .px(px(12.))
                                .py(px(6.))
                                .rounded(px(crate::theme::RADIUS_MD))
                                .child(crate::theme::floating_surface(crate::theme::RADIUS_MD))
                                .border_1()
                                .border_color(crate::theme::border())
                                .shadow_lg()
                                .text_size(px(12.))
                                .text_color(theme::content(0.92))
                                .child(text),
                        ),
                )
            })
            .when_some(dragging, |d, edge| {
                // While a rail is held, the whole panel follows the pointer.
                let overlay = div()
                    .id("device-drag")
                    .absolute()
                    .size_full()
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                        this.drag_to(event.position, window, cx)
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                            this.drag = None;
                            cx.notify();
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                            this.drag = None;
                            cx.notify();
                        }),
                    );
                d.child(match edge {
                    Edge::Left | Edge::Right => overlay.cursor_col_resize(),
                    Edge::Bottom => overlay.cursor_row_resize(),
                    Edge::BottomLeft => overlay.cursor_nesw_resize(),
                    Edge::BottomRight => overlay.cursor_nwse_resize(),
                })
            })
    }
}

/// Hands keyboard focus back from the web view to GPUI's content view.
/// Whether the keys are with GPUI's content view rather than with a web view.
pub fn keyboard_home(window: &Window) -> bool {
    parent_of(window).is_some_and(|view| {
        view.window().is_some_and(|ns_window| {
            let responder: &objc2_app_kit::NSResponder = &view;
            ns_window
                .firstResponder()
                .is_some_and(|current| std::ptr::eq(&*current, responder))
        })
    })
}

pub fn reclaim_keyboard(window: &Window) {
    if let Some(view) = parent_of(window)
        && let Some(ns_window) = view.window()
    {
        let responder: &objc2_app_kit::NSResponder = &view;
        let holds = ns_window
            .firstResponder()
            .is_some_and(|current| std::ptr::eq(&*current, responder));
        if !holds {
            ns_window.makeFirstResponder(Some(&view));
        }
    }
}

fn paint_web(shared: &Rc<Shared>, bounds: Bounds<Pixels>, window: &mut Window) {
    let width = f32::from(bounds.size.width);
    if tools_that_fit(shared.width.replace(width)) != tools_that_fit(width) {
        window.refresh();
    }
    if shared.parent.borrow().is_none() {
        *shared.parent.borrow_mut() = parent_of(window);
    }
    let parent = shared.parent.borrow().clone();
    let Some(parent) = parent else {
        return;
    };
    // AppKit frames are in window points; GPUI's are divided by the interface zoom.
    let zoom = window.content_zoom();
    let bounds = bounds.map(|v| v * zoom);
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return;
    }
    let (x, y) = (f64::from(bounds.origin.x), f64::from(bounds.origin.y));
    let room = BOTTOM_ROOM
        .with(Cell::get)
        .min(f64::from(bounds.size.height) / 2.);
    let (w, h) = (
        f64::from(bounds.size.width),
        f64::from(bounds.size.height) - room,
    );
    let layout = shared.layout.get();
    let grip = GRIP_ROOM * GRIP_SHARE.with(Cell::get);
    // Device mode: a frame of the device's aspect, centred, scaled so its CSS size is kept.
    let (fx, fy, fw, fh, page_zoom) = match layout.device {
        Some((dw, dh)) => {
            let (sw, sh, scale) = fit_device(
                ((w - GUTTER * 2.).max(1.), (h - GUTTER * 2.).max(1.)),
                (dw, dh),
            );
            (
                x + ((w - sw) / 2.).floor(),
                y + GUTTER,
                sw,
                sh,
                scale * layout.user_zoom,
            )
        }
        None => (x + grip, y, (w - grip).max(1.), h, layout.user_zoom),
    };
    let fy = if parent.isFlipped() {
        fy
    } else {
        parent.bounds().size.height - fy - fh
    };
    let rect = NSRect::new(NSPoint::new(fx, fy), NSSize::new(fw, fh));
    let geometry = layout.device.map(|(dw, dh)| Geometry {
        left: fx - x,
        frame: (fw, fh),
        scale: fit_device(
            ((w - GUTTER * 2.).max(1.), (h - GUTTER * 2.).max(1.)),
            (dw, dh),
        )
        .2,
        zoom: f64::from(zoom),
    });
    if shared.geometry.replace(geometry) != geometry {
        // The rails are laid out from this; draw once more with the new frame.
        window.refresh();
    }
    if let Some(web) = shared.web.borrow().as_ref() {
        // Re-adding the view would drop its keyboard focus, so only raise it when it isn't on top.
        let view: &NSView = web;
        let on_top = parent
            .subviews()
            .lastObject()
            .is_some_and(|last| std::ptr::eq(&*last, view));
        if !on_top {
            parent.addSubview_positioned_relativeTo(view, NSWindowOrderingMode::Above, None);
        }
        web.setFrame(rect);
        if shared.applied_zoom.get() != page_zoom {
            unsafe { web.setPageZoom(page_zoom) };
            shared.applied_zoom.set(page_zoom);
        }
    }
    shared.painted.set(true);
    shared.visible.set(true);
    shared.apply();
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.shared.alive.set(false);
        for tab in self.tabs.drain(..) {
            tab.web.removeFromSuperview();
        }
    }
}

impl EntityInputHandler for Browser {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let (start, end) = (
            byte_at(&self.text, range.start),
            byte_at(&self.text, range.end),
        );
        *adjusted = Some(range);
        Some(self.text[start..end].to_owned())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let r = self.selection();
        Some(UTF16Selection {
            range: utf16_len(&self.text[..r.start])..utf16_len(&self.text[..r.end]),
            reversed: self.cursor < self.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone()
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let marked = self.marked.take();
        if let Some(range) = range.or(marked) {
            self.anchor = byte_at(&self.text, range.start);
            self.cursor = byte_at(&self.text, range.end);
        }
        self.insert(&text.replace(['\n', '\r'], ""));
        self.suggest_open = true;
        self.suggest_row = None;
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let marked = self.marked.take();
        if let Some(range) = range.or(marked) {
            self.anchor = byte_at(&self.text, range.start);
            self.cursor = byte_at(&self.text, range.end);
        }
        let start = utf16_len(&self.text[..self.selection().start]);
        self.insert(new_text);
        self.marked = (!new_text.is_empty()).then(|| start..start + utf16_len(new_text));
        self.suggest_open = true;
        self.suggest_row = None;
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_keeps_as_many_tools_as_the_panel_has_room_for() {
        assert_eq!(tools_that_fit(0.), 3);
        assert_eq!(tools_that_fit(280.), 0);
        assert_eq!(tools_that_fit(418.), 3);
        assert_eq!(tools_that_fit(1200.), TOOLS);
    }

    #[test]
    fn devices_clamp_and_name_known_sizes_either_way_round() {
        assert_eq!(
            Device::from_size(390, 844).preset.as_deref(),
            Some("iphone-12-pro")
        );
        assert_eq!(
            Device::from_size(844, 390).preset.as_deref(),
            Some("iphone-12-pro")
        );
        let custom = Device::from_size(10, 99_999);
        assert_eq!((custom.width, custom.height), (240, 3840));
        assert!(custom.preset.is_none());
        let parsed =
            Device::from_json(&json!({"width": 412, "height": 915, "preset": "pixel-7"})).unwrap();
        assert_eq!(parsed.preset.as_deref(), Some("pixel-7"));
        assert!(Device::from_json(&Value::Null).is_none());
    }

    #[test]
    fn device_frames_fit_and_keep_the_css_size() {
        let (w, h, scale) = fit_device((400., 400.), (800., 1000.));
        assert_eq!((w, h), (320., 400.));
        assert!((scale - 0.4).abs() < 1e-9);
        // Never scaled up, never below a tenth.
        assert_eq!(fit_device((2000., 2000.), (390., 844.)).2, 1.);
        assert_eq!(fit_device((10., 10.), (1000., 1000.)).2, 0.1);
    }

    #[test]
    fn zoom_steps_between_half_and_double() {
        assert_eq!(clamp_zoom(0.1), 0.5);
        assert_eq!(clamp_zoom(3.0), 2.0);
        assert_eq!(clamp_zoom(1.1 + 0.1), 1.2);
        assert_eq!(clamp_zoom(f64::NAN), 1.0);
    }

    #[test]
    fn annotations_are_bounded_and_numbered() {
        let raw = json!({
            "url": "https://example.com/a", "width": 800.4, "height": 600.0,
            "entries": [
                {"selector": "button.buy", "label": "Buy", "x": 10.5, "y": 20, "width": 80, "height": "wide"},
                {"selector": "  ", "label": "nothing"},
            ],
        })
        .to_string();
        let capture = parse_annotation(&raw);
        assert_eq!(capture.elements.len(), 1);
        assert_eq!(capture.elements[0].rect, (10.5, 20., 80., 0.));
        let text = annotation_text(&capture);
        assert!(text.starts_with("Page: https://example.com/a\nViewport: 800×600"));
        assert!(text.contains("1. Buy — button.buy"));
        let hostile = parse_annotation(r#"{"url":"javascript:alert(1)"}"#);
        assert!(hostile.url.is_empty());
        assert_eq!(
            annotation_text(&hostile).lines().next(),
            Some("Page: (unknown)")
        );
    }
}
