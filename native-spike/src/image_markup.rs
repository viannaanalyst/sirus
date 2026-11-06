//! Drawing over a draft picture in the attachment viewer (ComposerImageAttachment.tsx in the old
//! app): the strokes, kept in the picture's own proportions so they follow any window size, and
//! their burning into a new PNG that takes the original's place in the composer.

use gpui::{Bounds, Pixels, Point};
use std::cell::Cell;
use std::io::Cursor;
use std::rc::Rc;

/// A point on the picture, each axis from 0 to 1.
pub(crate) type Mark = (f32, f32);

/// Points closer than this (in picture proportions) add nothing to a stroke.
const MIN_STEP: f32 = 0.001;

/// The viewer's drawing state, for the picture on show.
#[derive(Default)]
pub(crate) struct Markup {
    pub active: bool,
    pub strokes: Vec<Vec<Mark>>,
    pub drawing: bool,
    /// The burned picture is on its way to the host.
    pub busy: bool,
    /// The picture's size in pixels, read when the pencil is taken up.
    pub size: Option<(u32, u32)>,
    /// Where the picture's frame was painted, for turning the pointer into marks.
    pub stage: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Markup {
    /// Takes the pencil up or puts it down. A picture whose size can't be read stays undrawable.
    pub(crate) fn toggle(&mut self, bytes: &[u8]) {
        if self.size.is_none() {
            self.size = dimensions(bytes);
        }
        self.active = !self.active && self.size.is_some();
        self.drawing = false;
    }

    /// Starts a stroke at a window position.
    pub(crate) fn begin(&mut self, at: Point<Pixels>) {
        if let Some(mark) = self.mark_at(at) {
            self.strokes.push(vec![mark]);
            self.drawing = true;
        }
    }

    /// Extends the current stroke; true when it grew.
    pub(crate) fn extend(&mut self, at: Point<Pixels>) -> bool {
        let Some(mark) = self.mark_at(at) else {
            return false;
        };
        let Some(stroke) = self.strokes.last_mut().filter(|_| self.drawing) else {
            return false;
        };
        let last = stroke.last().copied().unwrap_or(mark);
        if (last.0 - mark.0).abs().max((last.1 - mark.1).abs()) < MIN_STEP {
            return false;
        }
        stroke.push(mark);
        true
    }

    fn mark_at(&self, at: Point<Pixels>) -> Option<Mark> {
        let frame = picture_frame(self.stage.get()?, self.size?);
        Some(to_mark(at, frame))
    }
}

/// The picture's rectangle inside its frame, as `ObjectFit::Contain` draws it.
pub(crate) fn picture_frame(stage: Bounds<Pixels>, size: (u32, u32)) -> Bounds<Pixels> {
    gpui::ObjectFit::Contain.get_bounds(
        stage,
        gpui::size(
            gpui::DevicePixels(size.0 as i32),
            gpui::DevicePixels(size.1 as i32),
        ),
    )
}

fn to_mark(at: Point<Pixels>, frame: Bounds<Pixels>) -> Mark {
    let along = |value: Pixels, start: Pixels, length: Pixels| {
        if length > gpui::px(0.) {
            ((value - start) / length).clamp(0., 1.)
        } else {
            0.
        }
    };
    (
        along(at.x, frame.origin.x, frame.size.width),
        along(at.y, frame.origin.y, frame.size.height),
    )
}

/// A stroke's width in picture pixels, so it reads the same on small and large pictures.
pub(crate) fn stroke_width(size: (u32, u32)) -> f32 {
    (size.0.max(size.1) as f32 / 300.).round().max(3.)
}

/// The picture's name once drawn on.
pub(crate) fn annotated_name(name: &str) -> String {
    let base = name.rsplit_once('.').map_or(name, |(base, _)| base);
    let base: String = base.chars().take(200).collect();
    format!("{base}-annotated.png")
}

fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// The picture with the strokes burned in, as a PNG.
pub(crate) fn burn(bytes: &[u8], strokes: &[Vec<Mark>], color: [u8; 3]) -> Option<Vec<u8>> {
    let mut canvas = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = canvas.dimensions();
    let radius = stroke_width(size) / 2.;
    let ink = image::Rgba([color[0], color[1], color[2], 255]);
    let place = |mark: Mark| (mark.0 * size.0 as f32, mark.1 * size.1 as f32);
    for stroke in strokes {
        let Some(first) = stroke.first() else {
            continue;
        };
        let mut from = place(*first);
        disc(&mut canvas, from, radius, ink);
        for mark in &stroke[1..] {
            let to = place(*mark);
            let steps = (to.0 - from.0)
                .abs()
                .max((to.1 - from.1).abs())
                .ceil()
                .max(1.);
            for step in 1..=steps as u32 {
                let along = step as f32 / steps;
                let point = (
                    from.0 + (to.0 - from.0) * along,
                    from.1 + (to.1 - from.1) * along,
                );
                disc(&mut canvas, point, radius, ink);
            }
            from = to;
        }
    }
    let mut png = Cursor::new(Vec::new());
    canvas.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(png.into_inner())
}

fn disc(canvas: &mut image::RgbaImage, center: (f32, f32), radius: f32, ink: image::Rgba<u8>) {
    let (width, height) = canvas.dimensions();
    let left = (center.0 - radius).floor().max(0.) as u32;
    let top = (center.1 - radius).floor().max(0.) as u32;
    let right = ((center.0 + radius).ceil() as u32).min(width);
    let bottom = ((center.1 + radius).ceil() as u32).min(height);
    for y in top..bottom {
        for x in left..right {
            let (dx, dy) = (x as f32 + 0.5 - center.0, y as f32 + 0.5 - center.1);
            if dx * dx + dy * dy <= radius * radius {
                canvas.put_pixel(x, y, ink);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px, size};

    fn white_png(width: u32, height: u32) -> Vec<u8> {
        let canvas = image::RgbaImage::from_pixel(width, height, image::Rgba([255; 4]));
        let mut png = Cursor::new(Vec::new());
        canvas.write_to(&mut png, image::ImageFormat::Png).unwrap();
        png.into_inner()
    }

    #[test]
    fn burning_keeps_the_size_and_inks_along_the_stroke() {
        let png = white_png(300, 100);
        let out = burn(&png, &[vec![(0.1, 0.5), (0.9, 0.5)]], [229, 72, 77]).unwrap();
        let burned = image::load_from_memory(&out).unwrap().to_rgba8();
        assert_eq!(burned.dimensions(), (300, 100));
        assert_eq!(burned.get_pixel(150, 50), &image::Rgba([229, 72, 77, 255]));
        assert_eq!(burned.get_pixel(150, 5), &image::Rgba([255; 4]));
    }

    #[test]
    fn a_single_click_leaves_a_dot() {
        let out = burn(&white_png(100, 100), &[vec![(0.5, 0.5)]], [1, 2, 3]).unwrap();
        let burned = image::load_from_memory(&out).unwrap().to_rgba8();
        assert_eq!(burned.get_pixel(50, 50), &image::Rgba([1, 2, 3, 255]));
    }

    #[test]
    fn strokes_at_the_edges_stay_inside_the_picture() {
        let strokes = [vec![(0., 0.), (1., 1.)]];
        assert!(burn(&white_png(10, 10), &strokes, [0, 0, 0]).is_some());
    }

    #[test]
    fn something_that_is_not_a_picture_does_not_burn_or_open_the_pencil() {
        assert!(burn(b"nope", &[], [0, 0, 0]).is_none());
        let mut markup = Markup::default();
        markup.toggle(b"nope");
        assert!(!markup.active);
    }

    #[test]
    fn the_pointer_maps_to_the_picture_not_to_its_frame() {
        // A wide frame leaves bars at the sides of a square picture.
        let mut markup = Markup {
            size: Some((100, 100)),
            ..Default::default()
        };
        markup.stage.set(Some(Bounds::new(
            point(px(0.), px(0.)),
            size(px(300.), px(100.)),
        )));
        markup.begin(point(px(150.), px(50.)));
        assert_eq!(markup.strokes[0][0], (0.5, 0.5));
        markup.begin(point(px(10.), px(50.)));
        assert_eq!(markup.strokes[1][0], (0., 0.5));
    }

    #[test]
    fn a_stroke_grows_only_while_drawing_and_only_when_it_moved() {
        let mut markup = Markup {
            size: Some((100, 100)),
            ..Default::default()
        };
        markup.stage.set(Some(Bounds::new(
            point(px(0.), px(0.)),
            size(px(100.), px(100.)),
        )));
        markup.begin(point(px(10.), px(10.)));
        assert!(!markup.extend(point(px(10.), px(10.))));
        assert!(markup.extend(point(px(20.), px(10.))));
        markup.drawing = false;
        assert!(!markup.extend(point(px(30.), px(10.))));
    }

    #[test]
    fn the_name_says_it_was_drawn_on() {
        assert_eq!(annotated_name("foto.final.jpg"), "foto.final-annotated.png");
        assert_eq!(annotated_name("imagem"), "imagem-annotated.png");
    }
}
