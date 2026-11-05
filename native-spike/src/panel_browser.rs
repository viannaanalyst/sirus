//! The Navegador panel: a 36 px toolbar (back, forward, reload, address field) over a native
//! WKWebView parented to the GPUI window's content view (technique from Monocode's html.rs, MIT).
//! The web view paints above GPUI, so it hides whenever the panel isn't painted in a frame and
//! while an overlay is open (`set_covered`).
use crate::i18n::t;
use crate::panels::PanelContext;
use crate::theme;
use gpui::{
    AnyView, App, Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler,
    FocusHandle, KeyDownEvent, MouseButton, Pixels, Point, Render, Task, Transformation,
    UTF16Selection, Window, canvas, div, percentage, prelude::*, px,
};
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{NSView, NSWindowOrderingMode};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration, WKWebsiteDataStore};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

const TOOLBAR: f32 = 36.;
const TICK: Duration = Duration::from_millis(60);
const STALE: Duration = Duration::from_millis(100);

thread_local! {
    static COVERED: Cell<bool> = const { Cell::new(false) };
    static LIVE: RefCell<Vec<Weak<Shared>>> = const { RefCell::new(Vec::new()) };
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
    let _ = context;
    cx.new(|cx| Browser::new(cx)).into()
}

/// State shared between the view, the paint callback and the frame callbacks.
struct Shared {
    web: RefCell<Option<Retained<WKWebView>>>,
    /// When the web view's slot was last painted.
    painted: Cell<Option<Instant>>,
    /// A refresh was forced to find out whether the slot still paints.
    probing: Cell<bool>,
    visible: Cell<bool>,
    alive: Cell<bool>,
}

impl Shared {
    fn apply(&self) {
        if let Some(web) = self.web.borrow().as_ref() {
            web.setHidden(COVERED.with(Cell::get) || !self.visible.get());
        }
    }
    /// Runs at the start of a frame, before it draws.
    fn check(self: &Rc<Self>, window: &mut Window) {
        if !self.alive.get() || !self.visible.get() {
            return;
        }
        if self.painted.get().is_some_and(|t| t.elapsed() <= STALE) {
            self.probing.set(false);
        } else if !self.probing.get() {
            // Nothing painted us lately; force one draw to tell "idle" from "gone".
            self.probing.set(true);
            window.refresh();
        } else {
            self.visible.set(false);
            self.apply();
        }
    }
}

struct Browser {
    shared: Rc<Shared>,
    focus: FocusHandle,
    focus_pending: bool,
    text: String,
    anchor: usize,
    cursor: usize,
    marked: Option<Range<usize>>,
    last_url: String,
    can_back: bool,
    can_forward: bool,
    tick: Option<Task<()>>,
}

fn normalize(input: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() {
        return None;
    }
    if t.contains("://")
        || ["about:", "data:", "file:"]
            .iter()
            .any(|p| t.starts_with(p))
    {
        return Some(t.to_owned());
    }
    let local = t.starts_with("localhost") || t.starts_with("127.0.0.1") || t.starts_with("[::1]");
    Some(format!("{}://{t}", if local { "http" } else { "https" }))
}

fn load(web: &WKWebView, url: &str) {
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
        unsafe { web.loadRequest(&NSURLRequest::requestWithURL(&url)) };
    }
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

impl Browser {
    fn new(cx: &mut Context<Self>) -> Self {
        let shared = Rc::new(Shared {
            web: RefCell::new(None),
            painted: Cell::new(None),
            probing: Cell::new(false),
            visible: Cell::new(false),
            alive: Cell::new(true),
        });
        LIVE.with(|live| live.borrow_mut().push(Rc::downgrade(&shared)));
        Self {
            shared,
            focus: cx.focus_handle(),
            focus_pending: true,
            text: String::new(),
            anchor: 0,
            cursor: 0,
            marked: None,
            last_url: String::new(),
            can_back: false,
            can_forward: false,
            tick: None,
        }
    }

    fn with_web<R>(&self, f: impl FnOnce(&WKWebView) -> R) -> Option<R> {
        self.shared.web.borrow().as_ref().map(|w| f(w))
    }

    /// Polls the page's address and history state, and arms the frame check.
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.with_web(|web| {
            let url = unsafe { web.URL() }
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string());
            (
                url.unwrap_or_default(),
                unsafe { web.canGoBack() },
                unsafe { web.canGoForward() },
            )
        });
        if let Some((url, back, forward)) = state {
            let mut changed = back != self.can_back || forward != self.can_forward;
            self.can_back = back;
            self.can_forward = forward;
            if url != self.last_url {
                self.last_url = url.clone();
                if !self.focus.is_focused(window) {
                    self.text = if url == "about:blank" {
                        String::new()
                    } else {
                        url
                    };
                    self.anchor = self.text.len();
                    self.cursor = self.text.len();
                }
                changed = true;
            }
            if changed {
                cx.notify();
            }
        }
        let shared = self.shared.clone();
        window.on_next_frame(move |window, _| shared.check(window));
    }

    fn navigate(&mut self, cx: &mut Context<Self>) {
        let Some(url) = normalize(&self.text) else {
            return;
        };
        self.text = url.clone();
        self.anchor = self.text.len();
        self.cursor = self.text.len();
        self.with_web(|web| load(web, &url));
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
        match keys.key.as_str() {
            "enter" => self.navigate(cx),
            "escape" => {
                self.text = if self.last_url == "about:blank" {
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
        self.marked = None;
        cx.stop_propagation();
        cx.notify();
    }

    fn button(
        &self,
        id: &'static str,
        path: &'static str,
        flip: bool,
        enabled: bool,
        cx: &mut Context<Self>,
        action: fn(&WKWebView),
    ) -> gpui::Stateful<gpui::Div> {
        let mut icon =
            theme::icon(path, 15.).text_color(theme::content(if enabled { 0.7 } else { 0.25 }));
        if flip {
            icon = icon.with_transformation(Transformation::rotate(percentage(0.5)));
        }
        let button = div()
            .id(id)
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .flex_shrink_0()
            .child(icon);
        if enabled {
            button
                .hover(|s| s.bg(theme::content(0.08)))
                .on_click(cx.listener(move |this, _, _, _| {
                    this.with_web(action);
                }))
        } else {
            button
        }
    }
}

impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending {
            self.focus_pending = false;
            window.focus(&self.focus, cx);
        }
        if self.tick.is_none() {
            self.tick = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(TICK).await;
                    if this
                        .update_in(cx, |this, window, cx| this.poll(window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        let focused = self.focus.is_focused(window);
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
                    cx.notify();
                }),
            )
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(26.))
            .px(px(9.))
            .rounded_md()
            .bg(theme::field_fill())
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
                            .bg(theme::accent_wash(0.35))
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
        let page = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| paint_web(&shared, bounds, window),
        )
        .size_full();

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(TOOLBAR))
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(8.))
                    .child(self.button(
                        "browser-back",
                        "arrow-left",
                        false,
                        self.can_back,
                        cx,
                        |w| unsafe {
                            w.goBack();
                        },
                    ))
                    .child(self.button(
                        "browser-forward",
                        "arrow-left",
                        true,
                        self.can_forward,
                        cx,
                        |w| unsafe {
                            w.goForward();
                        },
                    ))
                    .child(
                        self.button("browser-reload", "refresh", false, true, cx, |w| unsafe {
                            w.reload();
                        }),
                    )
                    .child(field),
            )
            .child(div().flex_1().w_full().child(page))
    }
}

/// Hands keyboard focus back from the web view to GPUI's content view.
fn reclaim_keyboard(window: &Window) {
    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::AppKit(handle) = handle.as_raw()
    {
        // GPUI keeps its AppKit content view alive for the window's lifetime.
        let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        if let Some(ns_window) = view.window() {
            ns_window.makeFirstResponder(Some(view));
        }
    }
}

fn paint_web(shared: &Rc<Shared>, bounds: Bounds<Pixels>, window: &mut Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // GPUI keeps its AppKit content view alive throughout paint.
    let parent = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    // AppKit frames are in window points; GPUI's are divided by the interface zoom.
    let zoom = window.content_zoom();
    let bounds = bounds.map(|v| v * zoom);
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return;
    }
    let (x, y) = (f64::from(bounds.origin.x), f64::from(bounds.origin.y));
    let (w, h) = (f64::from(bounds.size.width), f64::from(bounds.size.height));
    let y = if parent.isFlipped() {
        y
    } else {
        parent.bounds().size.height - y - h
    };
    let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));

    let mut slot = shared.web.borrow_mut();
    if slot.is_none() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let web = unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.setWebsiteDataStore(&WKWebsiteDataStore::defaultDataStore(mtm));
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), rect, &config)
        };
        load(&web, "about:blank");
        parent.addSubview(&web);
        *slot = Some(web);
    }
    if let Some(web) = slot.as_ref() {
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
    }
    drop(slot);
    shared.painted.set(Some(Instant::now()));
    shared.probing.set(false);
    shared.visible.set(true);
    shared.apply();
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.shared.alive.set(false);
        if let Some(web) = self.shared.web.borrow_mut().take() {
            web.removeFromSuperview();
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
