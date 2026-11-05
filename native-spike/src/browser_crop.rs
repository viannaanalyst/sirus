//! Cropping an element-picker screenshot to the marked elements (`browser-annotation.ts`):
//! the rectangle that holds every element, padded and mapped from the viewport's CSS pixels to
//! the picture's pixels, and the crop itself through Core Graphics.
use objc2::rc::Retained;
use objc2::{AllocAnyThread, Encoding, RefEncode, msg_send};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_foundation::{NSData, NSDictionary, NSPoint, NSRect, NSSize};

/// Core Graphics' `CGImageRef` target, so message sends carry its real type.
#[repr(C)]
pub struct CGImage {
    _private: [u8; 0],
}
unsafe impl RefEncode for CGImage {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGImage", &[]));
}

/// Pixels kept around the marked elements, in the page's CSS pixels.
pub const PADDING: f64 = 16.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A marked element as measured in the viewport: `(x, y, width, height)` in CSS pixels.
pub type Element = (f64, f64, f64, f64);

/// The part of a picture of `image` pixels that holds every element of a `viewport` (CSS px),
/// or `None` when nothing is marked or the sizes are unusable (`annotationCropRect`).
pub fn crop_rect(
    elements: &[Element],
    viewport: (f64, f64),
    image: (u32, u32),
    padding: f64,
) -> Option<CropRect> {
    if elements.is_empty()
        || !(viewport.0 > 0. && viewport.1 > 0. && viewport.0.is_finite() && viewport.1.is_finite())
        || image.0 == 0
        || image.1 == 0
        || elements
            .iter()
            .any(|e| ![e.0, e.1, e.2, e.3].iter().all(|v| v.is_finite()))
    {
        return None;
    }
    let left = elements.iter().map(|e| e.0).fold(f64::INFINITY, f64::min) - padding;
    let top = elements.iter().map(|e| e.1).fold(f64::INFINITY, f64::min) - padding;
    let right = elements
        .iter()
        .map(|e| e.0 + e.2)
        .fold(f64::NEG_INFINITY, f64::max)
        + padding;
    let bottom = elements
        .iter()
        .map(|e| e.1 + e.3)
        .fold(f64::NEG_INFINITY, f64::max)
        + padding;
    let (scale_x, scale_y) = (image.0 as f64 / viewport.0, image.1 as f64 / viewport.1);
    let x = (left * scale_x).floor().max(0.);
    let y = (top * scale_y).floor().max(0.);
    let width = ((right * scale_x).ceil().min(image.0 as f64) - x) as i64;
    let height = ((bottom * scale_y).ceil().min(image.1 as f64) - y) as i64;
    if width < 1 || height < 1 || x >= image.0 as f64 || y >= image.1 as f64 {
        return None;
    }
    Some(CropRect {
        x: x as u32,
        y: y as u32,
        width: width as u32,
        height: height as u32,
    })
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageCreateWithImageInRect(image: *const CGImage, rect: NSRect) -> *mut CGImage;
    fn CGImageRelease(image: *const CGImage);
}

fn decode(png: &[u8]) -> Option<Retained<NSBitmapImageRep>> {
    NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &NSData::with_bytes(png))
}

/// The pixel size of a PNG.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let rep = decode(png)?;
    let (width, height) = (rep.pixelsWide(), rep.pixelsHigh());
    (width > 0 && height > 0).then_some((width as u32, height as u32))
}

/// The PNG cropped to `rect` (pixels from the top left), as PNG bytes.
pub fn crop_png(png: &[u8], rect: CropRect) -> Option<Vec<u8>> {
    let rep = decode(png)?;
    let (width, height) = (rep.pixelsWide() as u32, rep.pixelsHigh() as u32);
    if rect.x.checked_add(rect.width)? > width || rect.y.checked_add(rect.height)? > height {
        return None;
    }
    unsafe {
        let source: *const CGImage = msg_send![&*rep, CGImage];
        if source.is_null() {
            return None;
        }
        let cropped = CGImageCreateWithImageInRect(
            source,
            NSRect::new(
                NSPoint::new(rect.x as f64, rect.y as f64),
                NSSize::new(rect.width as f64, rect.height as f64),
            ),
        );
        if cropped.is_null() {
            return None;
        }
        let made: Option<Retained<NSBitmapImageRep>> =
            msg_send![NSBitmapImageRep::alloc(), initWithCGImage: cropped];
        CGImageRelease(cropped);
        made?
            .representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
            .map(|data| data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crop_holds_every_marked_element_with_padding() {
        // Two elements in a 1000x500 viewport, shot at 2x.
        let elements = [(100., 50., 100., 40.), (300., 200., 50., 50.)];
        let rect = crop_rect(&elements, (1000., 500.), (2000, 1000), 16.).unwrap();
        assert_eq!(
            rect,
            CropRect {
                x: 168,
                y: 68,
                width: 564,
                height: 464
            }
        );
    }

    #[test]
    fn the_crop_stays_inside_the_picture() {
        let at_edge = [(0., 0., 1000., 500.)];
        let rect = crop_rect(&at_edge, (1000., 500.), (1000, 500), 16.).unwrap();
        assert_eq!(
            rect,
            CropRect {
                x: 0,
                y: 0,
                width: 1000,
                height: 500
            }
        );
        // An element scrolled out of the viewport leaves nothing to crop.
        assert_eq!(
            crop_rect(&[(0., 900., 10., 10.)], (1000., 500.), (1000, 500), 16.),
            None
        );
    }

    #[test]
    fn unusable_input_crops_nothing() {
        let one = [(1., 1., 1., 1.)];
        assert_eq!(crop_rect(&[], (10., 10.), (10, 10), 16.), None);
        assert_eq!(crop_rect(&one, (0., 10.), (10, 10), 16.), None);
        assert_eq!(crop_rect(&one, (10., 10.), (0, 10), 16.), None);
        assert_eq!(
            crop_rect(&[(f64::NAN, 0., 1., 1.)], (10., 10.), (10, 10), 16.),
            None
        );
    }
}
