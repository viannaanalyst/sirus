//! Recording the browser to a video (ADR-111, ADR-112), after Sirus Code's `browser-recorder.ts`.
//! Sirus Code takes snapshots of the page and encodes them in the web view with MediaRecorder;
//! here the same snapshots (`takeSnapshot`, which works for a hidden page when asked to paint)
//! are drawn into BGRA pixel buffers with the clicks and the key caption on top, and
//! AVAssetWriter encodes them to H.264 in an MP4 under Movies/Sirus. Everything runs on the
//! main thread; the frame loop is driven by `panel_browser`.
use crate::browser_tab::Answer;
use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{Encode, Encoding, MainThreadMarker, RefEncode, msg_send};
use objc2_app_kit::NSImage;
use objc2_foundation::{
    NSDictionary, NSError, NSMutableDictionary, NSNumber, NSRect, NSString, NSURL,
};
use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};
use serde_json::Value;
use std::{
    cell::RefCell,
    ffi::c_void,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Recordings stop by themselves after this long.
pub const MAX_RECORDING: Duration = Duration::from_secs(10 * 60);
/// How often the page's key presses and clicks are read.
pub const EVENTS_EVERY: Duration = Duration::from_millis(120);
/// The page script that hands over the key presses and clicks since the last read.
pub const INPUT_SCRIPT: &str = r#"(() => { const events = window.__sirusInput || []; window.__sirusInput = []; return JSON.stringify({ width: window.innerWidth, height: window.innerHeight, events }); })()"#;

// ---- What the recording shows over the page (`recording-overlay.ts`) ----

const RING_MS: i64 = 600;
const CAPTION_MS: i64 = 1500;
const CAPTION_KEYS: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    Key { key: String, at: i64 },
    Click { x: f64, y: f64, at: i64 },
}

/// The page's events (`{width, events: [{kind, key|x,y, at}]}`) and its viewport width.
pub fn parse_events(raw: &str) -> (Option<f64>, Vec<InputEvent>) {
    let value: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let width = value["width"].as_f64().filter(|w| *w > 0.);
    let events = value["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let at = e["at"].as_f64()? as i64;
            match e["kind"].as_str()? {
                "key" => Some(InputEvent::Key {
                    key: e["key"].as_str()?.chars().take(24).collect(),
                    at,
                }),
                "click" => Some(InputEvent::Click {
                    x: e["x"].as_f64()?,
                    y: e["y"].as_f64()?,
                    at,
                }),
                _ => None,
            }
        })
        .take(60)
        .collect();
    (width, events)
}

/// Clicks still animating at `now` (ms), with how far along their ring is (0 to 1).
pub fn click_rings(events: &[InputEvent], now: i64) -> Vec<(f64, f64, f64)> {
    events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Click { x, y, at } if now - at >= 0 && now - at < RING_MS => {
                Some((*x, *y, (now - at) as f64 / RING_MS as f64))
            }
            _ => None,
        })
        .collect()
}

/// Recent keys as one line: typed letters run together, other keys stand apart.
pub fn key_caption(events: &[InputEvent], now: i64) -> String {
    let keys: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            InputEvent::Key { key, at } if now - at < CAPTION_MS => Some(key.as_str()),
            _ => None,
        })
        .collect();
    let keys = &keys[keys.len().saturating_sub(CAPTION_KEYS)..];
    let mut parts: Vec<String> = Vec::new();
    let mut word = false;
    for key in keys {
        let mut chars = key.chars();
        let single = matches!((chars.next(), chars.next()), (Some(c), None)
            if (c.is_alphanumeric() || c.is_ascii_punctuation()) && c != '•');
        match parts.last_mut() {
            Some(last) if single && word => last.push_str(key),
            _ => parts.push((*key).to_owned()),
        }
        word = single;
    }
    parts.join(" ")
}

/// Bitrate for a frame size: width x height x fps x 0.05, between 2.5 and 50 Mbps.
pub fn bitrate(width: u32, height: u32, fps: u32) -> u32 {
    (width as f64 * height as f64 * fps as f64 * 0.05).clamp(2_500_000., 50_000_000.) as u32
}

/// The recording options in the settings (`browserRecordingFps`, `Keys`, `Clicks`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub fps: u32,
    pub keys: bool,
    pub clicks: bool,
}

impl Options {
    pub fn from_settings(settings: &Value) -> Self {
        Self {
            fps: if settings["browserRecordingFps"].as_u64() == Some(60) {
                60
            } else {
                30
            },
            keys: settings["browserRecordingKeys"].as_bool() != Some(false),
            clicks: settings["browserRecordingClicks"].as_bool() != Some(false),
        }
    }
    pub fn interval(&self) -> Duration {
        Duration::from_micros(1_000_000 / self.fps as u64)
    }
}

/// `Browser recording YYYY-MM-DD HH.MM.SS.mp4`.
pub fn file_name(stamp: &str) -> String {
    format!("Browser recording {stamp}.mp4")
}

/// A new recording's file in Movies/Sirus (the folder is created).
pub fn new_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("The Movies folder is unavailable.")?;
    let dir = Path::new(&home).join("Movies").join("Sirus");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M.%S").to_string();
    Ok(dir.join(file_name(&stamp)))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

// ---- Core Graphics, Core Video and AVFoundation, by hand ----

#[repr(C)]
struct CGContext {
    _private: [u8; 0],
}
unsafe impl RefEncode for CGContext {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGContext", &[]));
}
#[repr(C)]
struct CVBuffer {
    _private: [u8; 0],
}
unsafe impl RefEncode for CVBuffer {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CVBuffer", &[]));
}
#[repr(C)]
struct CVPool {
    _private: [u8; 0],
}
unsafe impl RefEncode for CVPool {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CVPixelBufferPool", &[]));
}
use crate::browser_crop::CGImage;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}
unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[
            Encoding::LongLong,
            Encoding::Int,
            Encoding::UInt,
            Encoding::LongLong,
        ],
    );
}
impl CMTime {
    /// `kCMTimeFlags_Valid`.
    fn millis(ms: i64) -> Self {
        Self {
            value: ms,
            timescale: 1000,
            flags: 1,
            epoch: 0,
        }
    }
}

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}
#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferPoolCreatePixelBuffer(
        allocator: *const c_void,
        pool: *const CVPool,
        out: *mut *mut CVBuffer,
    ) -> i32;
    fn CVPixelBufferLockBaseAddress(buffer: *mut CVBuffer, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: *mut CVBuffer, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buffer: *mut CVBuffer) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(buffer: *mut CVBuffer) -> usize;
    fn CVPixelBufferRelease(buffer: *mut CVBuffer);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> *const c_void;
    fn CGColorSpaceRelease(space: *const c_void);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *const c_void,
        info: u32,
    ) -> *const c_void;
    fn CGContextRelease(context: *const c_void);
    fn CGContextDrawImage(context: *mut CGContext, rect: NSRect, image: *const CGImage);
    fn CGContextSetRGBStrokeColor(context: *mut CGContext, r: f64, g: f64, b: f64, a: f64);
    fn CGContextSetRGBFillColor(context: *const c_void, r: f64, g: f64, b: f64, a: f64);
    fn CGContextSetLineWidth(context: *mut CGContext, width: f64);
    fn CGContextStrokeEllipseInRect(context: *mut CGContext, rect: NSRect);
    fn CGContextAddPath(context: *mut CGContext, path: *const c_void);
    fn CGContextFillPath(context: *mut CGContext);
    fn CGPathCreateWithRoundedRect(
        rect: NSRect,
        corner_width: f64,
        corner_height: f64,
        transform: *const c_void,
    ) -> *mut c_void;
    fn CGPathRelease(path: *mut c_void);
    fn CGImageGetWidth(image: *const CGImage) -> usize;
    fn CGImageGetHeight(image: *const CGImage) -> usize;
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(
        objc2_foundation::NSPoint::new(x, y),
        objc2_foundation::NSSize::new(width, height),
    )
}

fn number(value: f64) -> Retained<NSNumber> {
    NSNumber::new_f64(value)
}

fn class(name: &std::ffi::CStr) -> Result<&'static AnyClass, String> {
    AnyClass::get(name).ok_or_else(|| "Video recording is not available here.".to_owned())
}

/// The writer, its video input and the adaptor that feeds it pixel buffers.
struct Writer {
    writer: Retained<AnyObject>,
    input: Retained<AnyObject>,
    adaptor: Retained<AnyObject>,
    width: usize,
    height: usize,
}

impl Writer {
    fn create(path: &Path, width: usize, height: usize, opts: Options) -> Result<Self, String> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let writer: Retained<AnyObject> = unsafe {
            let alloc: Allocated<AnyObject> = msg_send![class(c"AVAssetWriter")?, alloc];
            let made: Result<Retained<AnyObject>, Retained<NSError>> = msg_send![
                alloc,
                initWithURL: &*url,
                fileType: &*NSString::from_str("public.mpeg-4"),
                error: _
            ];
            made.map_err(|error| error.localizedDescription().to_string())?
        };
        let strings = |pairs: &[(&str, Retained<AnyObject>)]| {
            let dictionary: Retained<NSMutableDictionary<NSString, AnyObject>> =
                NSMutableDictionary::new();
            for (key, value) in pairs {
                let key = NSString::from_str(key);
                unsafe {
                    dictionary
                        .setObject_forKey(value, objc2::runtime::ProtocolObject::from_ref(&*key))
                };
            }
            dictionary
        };
        let any = |value: Retained<NSNumber>| -> Retained<AnyObject> {
            unsafe { Retained::cast_unchecked(value) }
        };
        let compression = strings(&[(
            "AverageBitRate",
            any(number(bitrate(width as u32, height as u32, opts.fps) as f64)),
        )]);
        let settings = strings(&[
            ("AVVideoCodecKey", unsafe {
                Retained::cast_unchecked(NSString::from_str("avc1"))
            }),
            ("AVVideoWidthKey", any(number(width as f64))),
            ("AVVideoHeightKey", any(number(height as f64))),
            ("AVVideoCompressionPropertiesKey", unsafe {
                Retained::cast_unchecked(compression)
            }),
        ]);
        let input: Retained<AnyObject> = unsafe {
            msg_send![
                class(c"AVAssetWriterInput")?,
                assetWriterInputWithMediaType: &*NSString::from_str("vide"),
                outputSettings: &*settings
            ]
        };
        let attributes = strings(&[
            // kCVPixelFormatType_32BGRA
            ("PixelFormatType", any(number(0x4247_5241u32 as f64))),
            ("Width", any(number(width as f64))),
            ("Height", any(number(height as f64))),
        ]);
        let adaptor: Retained<AnyObject> = unsafe {
            let _: () = msg_send![&*input, setExpectsMediaDataInRealTime: Bool::YES];
            msg_send![
                class(c"AVAssetWriterInputPixelBufferAdaptor")?,
                assetWriterInputPixelBufferAdaptorWithAssetWriterInput: &*input,
                sourcePixelBufferAttributes: &*attributes
            ]
        };
        unsafe {
            let added: Bool = msg_send![&*writer, canAddInput: &*input];
            if !added.as_bool() {
                return Err("Video recording is not available here.".into());
            }
            let _: () = msg_send![&*writer, addInput: &*input];
            let started: Bool = msg_send![&*writer, startWriting];
            if !started.as_bool() {
                return Err(error_of(&writer));
            }
            let _: () = msg_send![&*writer, startSessionAtSourceTime: CMTime::millis(0)];
        }
        Ok(Self {
            writer,
            input,
            adaptor,
            width,
            height,
        })
    }

    /// Draws `image` and the overlays into a pixel buffer and appends it at `at` ms.
    fn append(&mut self, image: *const CGImage, at: i64, overlay: &Overlay) -> Result<(), String> {
        unsafe {
            let ready: Bool = msg_send![&*self.input, isReadyForMoreMediaData];
            if !ready.as_bool() {
                // The encoder is behind: this frame is dropped, the next one catches up.
                return Ok(());
            }
            let pool: *const CVPool = msg_send![&*self.adaptor, pixelBufferPool];
            let mut buffer: *mut CVBuffer = std::ptr::null_mut();
            if pool.is_null()
                || CVPixelBufferPoolCreatePixelBuffer(std::ptr::null(), pool, &mut buffer) != 0
                || buffer.is_null()
            {
                return Err(error_of(&self.writer));
            }
            CVPixelBufferLockBaseAddress(buffer, 0);
            let space = CGColorSpaceCreateDeviceRGB();
            // Premultiplied alpha first, little-endian 32 bits: BGRA.
            let context = CGBitmapContextCreate(
                CVPixelBufferGetBaseAddress(buffer),
                self.width,
                self.height,
                8,
                CVPixelBufferGetBytesPerRow(buffer),
                space,
                2 | (2 << 12),
            );
            CGColorSpaceRelease(space);
            if !context.is_null() {
                let (w, h) = (self.width as f64, self.height as f64);
                let ctx = context as *mut CGContext;
                CGContextDrawImage(ctx, rect(0., 0., w, h), image);
                overlay.draw(ctx, w, h);
                CGContextRelease(context);
            }
            let appended: Bool = msg_send![
                &*self.adaptor,
                appendPixelBuffer: buffer,
                withPresentationTime: CMTime::millis(at)
            ];
            CVPixelBufferUnlockBaseAddress(buffer, 0);
            CVPixelBufferRelease(buffer);
            if !appended.as_bool() {
                return Err(error_of(&self.writer));
            }
        }
        Ok(())
    }
}

fn error_of(writer: &AnyObject) -> String {
    let error: Option<Retained<NSError>> = unsafe { msg_send![writer, error] };
    error.map_or_else(
        || "The recording could not be written.".to_owned(),
        |e| e.localizedDescription().to_string(),
    )
}

/// What goes over a frame: click rings and the key caption.
#[derive(Default)]
pub struct Overlay {
    pub rings: Vec<(f64, f64, f64)>,
    pub caption: String,
    /// Frame pixels per CSS pixel of the page.
    pub scale: f64,
}

impl Overlay {
    unsafe fn draw(&self, context: *mut CGContext, width: f64, height: f64) {
        unsafe {
            let boost = (self.scale / 1.5).max(1.);
            for (x, y, progress) in &self.rings {
                CGContextSetRGBStrokeColor(context, 1., 196. / 255., 0., 1. - progress);
                CGContextSetLineWidth(context, 3. * boost);
                let radius = (10. + 22. * progress) * boost;
                // Page coordinates run down from the top; the bitmap's run up.
                CGContextStrokeEllipseInRect(
                    context,
                    rect(
                        x * self.scale - radius,
                        height - y * self.scale - radius,
                        radius * 2.,
                        radius * 2.,
                    ),
                );
            }
            if self.caption.is_empty() {
                return;
            }
            let size = (width.min(height) / 22.).round().max(18.);
            let (Ok(font_class), Ok(color_class), Ok(context_class)) = (
                class(c"NSFont"),
                class(c"NSColor"),
                class(c"NSGraphicsContext"),
            ) else {
                return;
            };
            let font: *mut AnyObject =
                msg_send![font_class, systemFontOfSize: size, weight: 0.3f64];
            let white: *mut AnyObject = msg_send![color_class, whiteColor];
            let attributes: Retained<NSDictionary<NSString, AnyObject>> = {
                let dictionary: Retained<NSMutableDictionary<NSString, AnyObject>> =
                    NSMutableDictionary::new();
                for (key, value) in [("NSFont", font), ("NSColor", white)] {
                    let key = NSString::from_str(key);
                    if let Some(value) = value.as_ref() {
                        dictionary.setObject_forKey(
                            value,
                            objc2::runtime::ProtocolObject::from_ref(&*key),
                        );
                    }
                }
                Retained::into_super(dictionary)
            };
            let text = NSString::from_str(&self.caption);
            let measured: objc2_foundation::NSSize =
                msg_send![&*text, sizeWithAttributes: &*attributes];
            let box_width = measured.width + size * 1.4;
            let box_height = size * 1.8;
            let x = ((width - box_width) / 2.).floor();
            // `size * 2.6` up from the bottom edge, as the canvas draws it.
            let y = size * 2.6 - box_height;
            let y = y.max(0.);
            let path = CGPathCreateWithRoundedRect(
                rect(x, y, box_width, box_height),
                size * 0.5,
                size * 0.5,
                std::ptr::null(),
            );
            CGContextSetRGBFillColor(context.cast(), 20. / 255., 20. / 255., 20. / 255., 0.78);
            CGContextAddPath(context, path);
            CGContextFillPath(context);
            CGPathRelease(path);
            let graphics: *mut AnyObject = msg_send![
                context_class,
                graphicsContextWithCGContext: context,
                flipped: Bool::NO
            ];
            if !graphics.is_null() {
                let _: () = msg_send![context_class, saveGraphicsState];
                let _: () = msg_send![context_class, setCurrentContext: graphics];
                let point = objc2_foundation::NSPoint::new(
                    x + size * 0.7,
                    y + (box_height - measured.height) / 2.,
                );
                let _: () = msg_send![&*text, drawAtPoint: point, withAttributes: &*attributes];
                let _: () = msg_send![context_class, restoreGraphicsState];
            }
        }
    }
}

/// One recording in progress.
pub struct Recorder {
    pub path: PathBuf,
    pub opts: Options,
    writer: Option<Writer>,
    started: Instant,
    last_ms: i64,
    /// The page's key presses and clicks (page clock, ms) and its viewport width in CSS pixels.
    events: Vec<InputEvent>,
    viewport: f64,
    pub frames: u64,
}

pub type Shared = Rc<RefCell<Recorder>>;

impl Recorder {
    pub fn new(path: PathBuf, opts: Options) -> Shared {
        Rc::new(RefCell::new(Self {
            path,
            opts,
            writer: None,
            started: Instant::now(),
            last_ms: -1,
            events: Vec::new(),
            viewport: 0.,
            frames: 0,
        }))
    }

    /// Takes what the page reported since the last read.
    pub fn note_events(&mut self, raw: &str) {
        let (width, events) = parse_events(raw);
        if let Some(width) = width {
            self.viewport = width;
        }
        let now = now_ms();
        self.events.extend(events);
        self.events.retain(|event| {
            let at = match event {
                InputEvent::Key { at, .. } | InputEvent::Click { at, .. } => *at,
            };
            now - at < 2500
        });
        let extra = self.events.len().saturating_sub(40);
        self.events.drain(..extra);
    }

    /// Draws one snapshot into the video.
    fn push(&mut self, image: *const CGImage) -> Result<(), String> {
        let (iw, ih) = unsafe { (CGImageGetWidth(image), CGImageGetHeight(image)) };
        if iw == 0 || ih == 0 {
            return Err("Could not capture the page.".into());
        }
        if self.writer.is_none() {
            // Even sizes keep H.264 encoders happy; the size is fixed for the whole recording.
            let (w, h) = (iw - iw % 2, ih - ih % 2);
            if w < 2 || h < 2 {
                return Err("Could not capture the page.".into());
            }
            self.writer = Some(Writer::create(&self.path, w, h, self.opts)?);
            self.started = Instant::now();
        }
        let at = (self.started.elapsed().as_millis() as i64).max(self.last_ms + 1);
        self.last_ms = at;
        let writer = self.writer.as_mut().expect("writer was just created");
        let width = writer.width as f64;
        let viewport = if self.viewport > 0. {
            self.viewport
        } else {
            width
        };
        let now = now_ms();
        let overlay = Overlay {
            rings: if self.opts.clicks {
                click_rings(&self.events, now)
            } else {
                Vec::new()
            },
            caption: if self.opts.keys {
                key_caption(&self.events, now)
            } else {
                String::new()
            },
            scale: width / viewport.max(1.),
        };
        writer.append(image, at, &overlay)?;
        self.frames += 1;
        Ok(())
    }
}

/// Asks for one snapshot of the page and draws it into the recording when it arrives. Hidden
/// pages paint only when asked (`afterScreenUpdates`).
pub fn capture(recorder: &Shared, web: &WKWebView, mtm: MainThreadMarker) -> Answer<()> {
    let (sender, receiver) = async_channel::bounded(1);
    let recorder = recorder.clone();
    let handler = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let result = unsafe {
            match image.as_ref() {
                None => Err(error
                    .as_ref()
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "Could not capture the page.".into())),
                Some(image) => {
                    let cg: *const CGImage = msg_send![
                        image,
                        CGImageForProposedRect: std::ptr::null_mut::<NSRect>(),
                        context: Option::<&AnyObject>::None,
                        hints: Option::<&AnyObject>::None
                    ];
                    if cg.is_null() {
                        Err("Could not capture the page.".into())
                    } else {
                        recorder.borrow_mut().push(cg)
                    }
                }
            }
        };
        let _ = sender.try_send(result);
    });
    unsafe {
        let config = WKSnapshotConfiguration::new(mtm);
        config.setAfterScreenUpdates(web.isHidden());
        let width = web.frame().size.width.clamp(160., 1280.);
        config.setSnapshotWidth(Some(&number(width)));
        web.takeSnapshotWithConfiguration_completionHandler(Some(&config), &handler);
    }
    receiver
}

/// Closes the file. The answer is the path, or `None` when no frame was ever written (the empty
/// file is removed).
pub fn finish(recorder: &Shared) -> Answer<Option<PathBuf>> {
    let (sender, receiver) = async_channel::bounded(1);
    let (path, writer, frames) = {
        let mut rec = recorder.borrow_mut();
        (rec.path.clone(), rec.writer.take(), rec.frames)
    };
    let Some(writer) = writer else {
        let _ = std::fs::remove_file(&path);
        let _ = sender.try_send(Ok(None));
        return receiver;
    };
    let writer_object = writer.writer.clone();
    let done_path = path.clone();
    let handler = RcBlock::new(move || {
        let failed = error_of_status(&writer_object);
        let result = match failed {
            Some(error) => {
                let _ = std::fs::remove_file(&done_path);
                Err(error)
            }
            None if frames == 0 => {
                let _ = std::fs::remove_file(&done_path);
                Ok(None)
            }
            None => Ok(Some(done_path.clone())),
        };
        let _ = sender.try_send(result);
    });
    unsafe {
        let _: () = msg_send![&*writer.input, markAsFinished];
        let _: () = msg_send![&*writer.writer, finishWritingWithCompletionHandler: &*handler];
    }
    // The block holds the writer until it has run.
    receiver
}

/// The writer's error text when it ended in the failed state (status 3).
fn error_of_status(writer: &AnyObject) -> Option<String> {
    let status: isize = unsafe { msg_send![writer, status] };
    (status == 3).then(|| error_of(writer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(key: &str, at: i64) -> InputEvent {
        InputEvent::Key {
            key: key.into(),
            at,
        }
    }

    #[test]
    fn recordings_are_mp4_files_named_by_time() {
        assert_eq!(
            file_name("2026-10-11 22.00.00"),
            "Browser recording 2026-10-11 22.00.00.mp4"
        );
    }

    #[test]
    fn bitrate_follows_the_frame_and_stays_between_bounds() {
        assert_eq!(bitrate(100, 100, 30), 2_500_000);
        assert_eq!(bitrate(1920, 1080, 60), 1920 * 1080 * 60 / 20);
        assert_eq!(bitrate(7680, 4320, 60), 50_000_000);
    }

    #[test]
    fn options_come_from_the_settings() {
        let on = Options::from_settings(&json!({}));
        assert_eq!(
            on,
            Options {
                fps: 30,
                keys: true,
                clicks: true
            }
        );
        let off = Options::from_settings(&json!({
            "browserRecordingFps": 60, "browserRecordingKeys": false, "browserRecordingClicks": false
        }));
        assert_eq!(
            off,
            Options {
                fps: 60,
                keys: false,
                clicks: false
            }
        );
        assert_eq!(
            Options::from_settings(&json!({ "browserRecordingFps": 15 })).fps,
            30
        );
        assert_eq!(off.interval(), Duration::from_micros(16_666));
    }

    #[test]
    fn page_events_parse_bounded() {
        let raw = r#"{"width":1280,"height":800,"events":[
            {"kind":"key","key":"a","at":10},
            {"kind":"click","x":5.5,"y":6,"at":20},
            {"kind":"scroll","at":30},
            {"kind":"key","at":40}]}"#;
        let (width, events) = parse_events(raw);
        assert_eq!(width, Some(1280.));
        assert_eq!(
            events,
            [
                key("a", 10),
                InputEvent::Click {
                    x: 5.5,
                    y: 6.,
                    at: 20
                }
            ]
        );
        assert_eq!(parse_events("not json"), (None, Vec::new()));
    }

    #[test]
    fn clicks_ring_for_six_hundred_milliseconds() {
        let events = [
            InputEvent::Click {
                x: 1.,
                y: 2.,
                at: 1000,
            },
            InputEvent::Click {
                x: 3.,
                y: 4.,
                at: 100,
            },
        ];
        assert_eq!(click_rings(&events, 1300), [(1., 2., 0.5)]);
        assert!(click_rings(&events, 1600).is_empty());
        assert!(click_rings(&events, 999).is_empty());
    }

    #[test]
    fn typed_letters_run_together_and_other_keys_stand_apart() {
        let events = [
            key("h", 100),
            key("i", 120),
            key("↩", 140),
            key("⌘K", 160),
            key("l", 180),
            key("o", 200),
            key("•", 210),
            key("•", 220),
            key("old", -2000),
        ];
        assert_eq!(key_caption(&events, 1000), "hi ↩ ⌘K lo • •");
        // Keys older than 1.5 s drop off the caption.
        assert_eq!(key_caption(&events, 1700), "• •");
        assert_eq!(key_caption(&[], 0), "");
    }

    #[test]
    fn the_caption_keeps_the_last_twelve_keys() {
        let events: Vec<_> = (0..20).map(|i| key("Tab", 100 + i)).collect();
        assert_eq!(key_caption(&events, 200).split(' ').count(), 12);
    }
}
