//! Holes in the browser's page for what floats over it. The page is a native view, drawn above
//! everything GPUI paints, so a card over it used to hide the whole page. Instead the page's layer
//! gets a mask with the card's rectangle cut out, and the card shows through that hole.
//!
//! The floating cards report their bounds while the frame is laid out (`report`); the browser's
//! end-of-frame hook takes them (`take_reported`), so a frame without cards clears the holes.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_app_kit::NSView;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;
use std::ffi::c_void;

/// A rectangle in window points, from the top-left: (x, y, width, height).
pub type Rect = (f64, f64, f64, f64);

/// The corner radius of a hole, the cards' own.
const RADIUS: f64 = 12.;

thread_local! {
    static REPORTED: RefCell<Vec<Rect>> = const { RefCell::new(Vec::new()) };
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPathCreateMutable() -> *mut c_void;
    fn CGPathAddRect(path: *mut c_void, transform: *const c_void, rect: NSRect);
    fn CGPathAddRoundedRect(
        path: *mut c_void,
        transform: *const c_void,
        rect: NSRect,
        corner_width: f64,
        corner_height: f64,
    );
    fn CGPathRelease(path: *mut c_void);
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

/// A floating card's rectangle in this frame.
pub fn report(rect: Rect) {
    REPORTED.with(|r| r.borrow_mut().push(rect));
}

/// The rectangles reported since the last call.
pub fn take_reported() -> Vec<Rect> {
    REPORTED.with(|r| std::mem::take(&mut *r.borrow_mut()))
}

/// The parts of `holes` over a page at `frame`, in the page's own coordinates from its top-left,
/// clipped to it.
pub fn local_holes(frame: Rect, holes: &[Rect]) -> Vec<Rect> {
    let (fx, fy, fw, fh) = frame;
    holes
        .iter()
        .filter_map(|&(x, y, w, h)| {
            let (left, top) = ((x - fx).max(0.), (y - fy).max(0.));
            let (right, bottom) = ((x + w - fx).min(fw), (y + h - fy).min(fh));
            (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
        })
        .collect()
}

/// Masks `web`, painted at `frame`, so `holes` show what is under it; no holes, no mask.
pub fn apply(web: &NSView, frame: Rect, holes: &[Rect]) {
    let local = local_holes(frame, holes);
    web.setWantsLayer(true);
    // SAFETY: a layer-backed view's `layer` is a CALayer; CAShapeLayer and CGPath are used as
    // documented, and the path is released after the layer has retained it.
    unsafe {
        let layer: Option<Retained<AnyObject>> = msg_send![web, layer];
        let Some(layer) = layer else {
            return;
        };
        if local.is_empty() {
            let none: *const AnyObject = std::ptr::null();
            let _: () = msg_send![&*layer, setMask: none];
            return;
        }
        let Some(class) = AnyClass::get(c"CAShapeLayer") else {
            return;
        };
        let mask: Retained<AnyObject> = msg_send![class, layer];
        let (_, _, fw, fh) = frame;
        let flipped = web.isFlipped();
        let path = CGPathCreateMutable();
        CGPathAddRect(path, std::ptr::null(), rect(0., 0., fw, fh));
        for (x, y, w, h) in local {
            // The layer's geometry follows the view's: from the top when it is flipped.
            let y = if flipped { y } else { fh - y - h };
            CGPathAddRoundedRect(path, std::ptr::null(), rect(x, y, w, h), RADIUS, RADIUS);
        }
        let _: () = msg_send![&*mask, setFrame: rect(0., 0., fw, fh)];
        let _: () = msg_send![&*mask, setPath: path];
        let even_odd = NSString::from_str("even-odd");
        let _: () = msg_send![&*mask, setFillRule: &*even_odd];
        let _: () = msg_send![&*layer, setMask: &*mask];
        CGPathRelease(path);
    }
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hole_is_clipped_to_the_page_and_made_local() {
        let frame = (100., 50., 800., 600.);
        // A card at the window's top right, partly above and right of the page.
        let holes = local_holes(frame, &[(820., 40., 300., 120.)]);
        assert_eq!(holes, vec![(720., 0., 80., 110.)]);
        assert!(local_holes(frame, &[(0., 0., 50., 50.)]).is_empty());
    }
}
