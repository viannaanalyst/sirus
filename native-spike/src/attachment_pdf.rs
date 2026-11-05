//! Reading a PDF with the system: CoreGraphics draws the pages, PDFKit gives each page's text.
//! Nothing is bundled (macOS ships both). Every function blocks and is meant for a background
//! thread; each call parses the bytes it is given, so there is no handle to share between threads.
//!
//! The viewer shows PDFs in PDFKit's own view (`attachment_web`); this module checks a file opens
//! (password, damage, page limit) and gives the page sizes. The page rasters and page texts stay
//! for a thumbnail or text fallback and are covered by the tests.
#![allow(dead_code)]
use objc2::msg_send;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{NSData, NSString};
use std::ffi::c_void;

/// More pages than this are refused (Sirus Code's reader does the same, `reader.pdfLimit`).
pub const MAX_PAGES: usize = 1000;
/// The most pixels one page raster has.
const MAX_PIXELS: f64 = 16_000_000.;
/// The longest text kept for one page.
const MAX_PAGE_TEXT: usize = 60_000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGSize {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGAffineTransform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

type Ref = *const c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDataProviderCreateWithData(
        info: *mut c_void,
        data: *const c_void,
        size: usize,
        release: Option<extern "C" fn(*mut c_void, *const c_void, usize)>,
    ) -> Ref;
    fn CGDataProviderRelease(provider: Ref);
    fn CGPDFDocumentCreateWithProvider(provider: Ref) -> Ref;
    fn CGPDFDocumentRelease(document: Ref);
    fn CGPDFDocumentGetNumberOfPages(document: Ref) -> usize;
    fn CGPDFDocumentGetPage(document: Ref, number: usize) -> Ref;
    fn CGPDFDocumentIsEncrypted(document: Ref) -> bool;
    fn CGPDFDocumentIsUnlocked(document: Ref) -> bool;
    fn CGPDFDocumentUnlockWithPassword(document: Ref, password: *const i8) -> bool;
    fn CGPDFPageGetBoxRect(page: Ref, kind: i32) -> CGRect;
    fn CGPDFPageGetRotationAngle(page: Ref) -> i32;
    fn CGPDFPageGetDrawingTransform(
        page: Ref,
        kind: i32,
        rect: CGRect,
        rotate: i32,
        preserve_aspect_ratio: bool,
    ) -> CGAffineTransform;
    fn CGColorSpaceCreateDeviceRGB() -> Ref;
    fn CGColorSpaceRelease(space: Ref);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: Ref,
        info: u32,
    ) -> Ref;
    fn CGBitmapContextGetData(context: Ref) -> *mut c_void;
    fn CGBitmapContextGetBytesPerRow(context: Ref) -> usize;
    fn CGContextRelease(context: Ref);
    fn CGContextSetRGBFillColor(context: Ref, r: f64, g: f64, b: f64, a: f64);
    fn CGContextFillRect(context: Ref, rect: CGRect);
    fn CGContextConcatCTM(context: Ref, transform: CGAffineTransform);
    fn CGContextDrawPDFPage(context: Ref, page: Ref);
}

#[link(name = "PDFKit", kind = "framework")]
unsafe extern "C" {}

/// kCGPDFCropBox: what a viewer shows of a page.
const CROP_BOX: i32 = 1;
/// kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little: bytes in memory are B, G, R, A.
const BGRA: u32 = 2 | (2 << 12);

/// A loaded document, owning the CoreGraphics objects until dropped.
struct Document<'a> {
    provider: Ref,
    document: Ref,
    _bytes: std::marker::PhantomData<&'a [u8]>,
}

impl<'a> Document<'a> {
    fn open(bytes: &'a [u8]) -> Result<Self, Error> {
        if !bytes.starts_with(b"%PDF") && !bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
            return Err(Error::Invalid);
        }
        // SAFETY: the provider reads `bytes`, which outlives it (`'a`); there is no release callback.
        unsafe {
            let provider = CGDataProviderCreateWithData(
                std::ptr::null_mut(),
                bytes.as_ptr().cast(),
                bytes.len(),
                None,
            );
            if provider.is_null() {
                return Err(Error::Invalid);
            }
            let document = CGPDFDocumentCreateWithProvider(provider);
            if document.is_null() {
                CGDataProviderRelease(provider);
                return Err(Error::Invalid);
            }
            let this = Document {
                provider,
                document,
                _bytes: std::marker::PhantomData,
            };
            // Many PDFs are "encrypted" with an empty user password; anything else stays closed.
            if CGPDFDocumentIsEncrypted(document) && !CGPDFDocumentIsUnlocked(document) {
                let empty = [0i8];
                if !CGPDFDocumentUnlockWithPassword(document, empty.as_ptr()) {
                    return Err(Error::Locked);
                }
            }
            Ok(this)
        }
    }

    fn pages(&self) -> usize {
        // SAFETY: `document` is live.
        unsafe { CGPDFDocumentGetNumberOfPages(self.document) }
    }

    /// A page's displayed size in points (rotation applied).
    fn size(&self, index: usize) -> Option<(f64, f64)> {
        // SAFETY: pages are borrowed from the live document.
        unsafe {
            let page = CGPDFDocumentGetPage(self.document, index + 1);
            if page.is_null() {
                return None;
            }
            let rect = CGPDFPageGetBoxRect(page, CROP_BOX);
            let (w, h) = (rect.size.width.abs(), rect.size.height.abs());
            if w < 1. || h < 1. {
                return None;
            }
            Some(if CGPDFPageGetRotationAngle(page).rem_euclid(180) != 0 {
                (h, w)
            } else {
                (w, h)
            })
        }
    }
}

impl Drop for Document<'_> {
    fn drop(&mut self) {
        // SAFETY: both were created in `open` and are released once.
        unsafe {
            CGPDFDocumentRelease(self.document);
            CGDataProviderRelease(self.provider);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Locked,
    Limit,
}

/// A PDF's pages and text.
pub struct Info {
    /// Each page's displayed size in points.
    pub sizes: Vec<(f32, f32)>,
    /// Each page's text, empty for a scan.
    pub texts: Vec<String>,
}

/// The size and text of every page.
pub fn open(bytes: &[u8]) -> Result<Info, Error> {
    let document = Document::open(bytes)?;
    let count = document.pages();
    if count == 0 {
        return Err(Error::Invalid);
    }
    if count > MAX_PAGES {
        return Err(Error::Limit);
    }
    let sizes = (0..count)
        .map(|i| {
            document
                .size(i)
                .map_or((612., 792.), |(w, h)| (w as f32, h as f32))
        })
        .collect();
    drop(document);
    let mut texts = page_texts(bytes);
    texts.resize(count, String::new());
    Ok(Info { sizes, texts })
}

/// PDFKit's text of each page.
fn page_texts(bytes: &[u8]) -> Vec<String> {
    autoreleasepool(|_| {
        let Some(class) = AnyClass::get(c"PDFDocument") else {
            return Vec::new();
        };
        let data = NSData::with_bytes(bytes);
        // SAFETY: plain Objective-C calls on objects made here; the selectors exist on PDFDocument/PDFPage.
        unsafe {
            let allocated: *mut AnyObject = msg_send![class, alloc];
            let document: Option<Retained<AnyObject>> =
                Retained::from_raw(msg_send![allocated, initWithData: &*data]);
            let Some(document) = document else {
                return Vec::new();
            };
            let count: usize = msg_send![&*document, pageCount];
            (0..count.min(MAX_PAGES))
                .map(|index| {
                    let page: Option<Retained<AnyObject>> =
                        msg_send![&*document, pageAtIndex: index];
                    let text: Option<Retained<NSString>> =
                        page.and_then(|page| msg_send![&*page, string]);
                    let mut text = text.map(|t| t.to_string()).unwrap_or_default();
                    if text.len() > MAX_PAGE_TEXT {
                        let mut end = MAX_PAGE_TEXT;
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        text.truncate(end);
                    }
                    text.trim().replace("\r\n", "\n").replace('\r', "\n")
                })
                .collect()
        }
    })
}

/// A page drawn on white, `width` pixels wide, as BGRA with premultiplied alpha (what GPUI's
/// `RenderImage` takes).
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

pub fn render(bytes: &[u8], index: usize, width: u32) -> Option<Raster> {
    let document = Document::open(bytes).ok()?;
    let (page_w, page_h) = document.size(index)?;
    let mut scale = f64::from(width.max(8)) / page_w;
    if page_w * page_h * scale * scale > MAX_PIXELS {
        scale = (MAX_PIXELS / (page_w * page_h)).sqrt();
    }
    let (w, h) = (
        (page_w * scale).round().max(1.),
        (page_h * scale).round().max(1.),
    );
    // SAFETY: CoreGraphics objects are created and released here; the pixels are copied out
    // before the context goes.
    unsafe {
        let page = CGPDFDocumentGetPage(document.document, index + 1);
        let space = CGColorSpaceCreateDeviceRGB();
        let context = CGBitmapContextCreate(
            std::ptr::null_mut(),
            w as usize,
            h as usize,
            8,
            0,
            space,
            BGRA,
        );
        CGColorSpaceRelease(space);
        if context.is_null() {
            return None;
        }
        let area = CGRect {
            origin: CGPoint::default(),
            size: CGSize {
                width: w,
                height: h,
            },
        };
        CGContextSetRGBFillColor(context, 1., 1., 1., 1.);
        CGContextFillRect(context, area);
        // The drawing transform only ever shrinks a page, so the zoom goes in first.
        CGContextConcatCTM(
            context,
            CGAffineTransform {
                a: w / page_w,
                b: 0.,
                c: 0.,
                d: h / page_h,
                tx: 0.,
                ty: 0.,
            },
        );
        let page_area = CGRect {
            origin: CGPoint::default(),
            size: CGSize {
                width: page_w,
                height: page_h,
            },
        };
        CGContextConcatCTM(
            context,
            CGPDFPageGetDrawingTransform(page, CROP_BOX, page_area, 0, true),
        );
        CGContextDrawPDFPage(context, page);
        let data = CGBitmapContextGetData(context).cast::<u8>();
        let stride = CGBitmapContextGetBytesPerRow(context);
        let row = w as usize * 4;
        let mut bgra = Vec::with_capacity(row * h as usize);
        if !data.is_null() {
            for y in 0..h as usize {
                bgra.extend_from_slice(std::slice::from_raw_parts(data.add(y * stride), row));
            }
        }
        CGContextRelease(context);
        (bgra.len() == row * h as usize).then_some(Raster {
            width: w as u32,
            height: h as u32,
            bgra,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF with a line of text, written by hand so the test needs no files.
    fn tiny_pdf() -> Vec<u8> {
        let content = "BT /F1 24 Tf 72 700 Td (Hello PDF page) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        out.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        out
    }

    #[test]
    fn a_pdf_gives_its_pages_text_and_raster() {
        let bytes = tiny_pdf();
        let info = open(&bytes).unwrap();
        assert_eq!(info.sizes, vec![(612., 792.)]);
        assert!(info.texts[0].contains("Hello PDF page"), "{:?}", info.texts);
        let raster = render(&bytes, 0, 306).unwrap();
        assert_eq!((raster.width, raster.height), (306, 396));
        assert_eq!(raster.bgra.len(), 306 * 396 * 4);
        // White paper with some ink on it.
        assert!(raster.bgra.chunks(4).any(|p| p[0] < 128));
        assert!(raster.bgra.chunks(4).any(|p| p[0] == 255 && p[3] == 255));
        assert!(render(&bytes, 3, 306).is_none());
    }

    #[test]
    fn what_is_not_a_pdf_is_refused() {
        assert_eq!(open(b"just text").err(), Some(Error::Invalid));
        assert_eq!(open(b"%PDF-1.4 broken").err(), Some(Error::Invalid));
    }
}
