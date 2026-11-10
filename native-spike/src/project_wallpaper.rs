//! The project's wallpaper behind the conversation card: an ordered-dither (Bayer 8x8) glow in the
//! project's colour that rises from the bottom middle and is gone before the top, so the text up
//! there sits on near black. Each project's glow has its own slight shape, drawn from its name.
//!
//! The picture is made at the card's size in device pixels, one dither cell per `CELL` points, on
//! a background thread through GPUI's asset cache, and painted 1:1 so the cells stay sharp.

use gpui::{App, Asset, ObjectFit, RenderImage, Window, prelude::*};
use image::RgbaImage;
use std::{cell::RefCell, future::Future, sync::Arc};

/// One dither cell, in points.
pub const CELL: f32 = 3.;
/// How bright the glow gets, 0..1.
const INTENSITY: f32 = 0.30;
/// How far up the glow reaches, as a share of the card's height.
const HEIGHT: f32 = 0.30;
/// How much the project's own shape bends the glow, 0..1.
const SHAPE: f32 = 0.35;
/// The card's size is rounded up to this many device pixels, so a resize reuses pictures.
const STEP: u32 = 64;
/// The darkest colour, the card's near black.
const BASE: [u8; 3] = [6, 6, 7];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub color: u32,
    pub seed: u32,
    /// The picture's size in device pixels.
    pub size: (u32, u32),
    /// One cell, in device pixels.
    pub cell: u32,
}

/// The 8x8 Bayer thresholds, each in (0, 1).
pub(crate) fn bayer8(x: u32, y: u32) -> f32 {
    // M(2n) = 4 M(n) + D, with D picked by the coarsest bit: the coarse bits weigh least.
    const D: [[u32; 2]; 2] = [[0, 2], [3, 1]];
    let v: u32 = (0..3)
        .map(|i| D[((y >> i) & 1) as usize][((x >> i) & 1) as usize] * 4u32.pow(2 - i))
        .sum();
    (v as f32 + 0.5) / 64.
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let m = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    [m(0), m(1), m(2)]
}

/// The six levels, black to the colour and a touch past it.
fn palette(color: u32) -> [[u8; 3]; 6] {
    let c = [(color >> 16) as u8, (color >> 8) as u8, color as u8];
    [
        BASE,
        mix(BASE, c, 0.16),
        mix(BASE, c, 0.34),
        mix(BASE, c, 0.58),
        mix(BASE, c, 0.85),
        mix(c, [255, 255, 255], 0.18),
    ]
}

/// A small xorshift, seeded by the project's name.
struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        let mut h = self.0.max(1);
        h ^= h << 13;
        h ^= h >> 17;
        h ^= h << 5;
        self.0 = h;
        (h % 10_000) as f32 / 10_000.
    }
}

/// FNV-1a of a name, the seed of its glow's shape.
pub fn seed_of(name: &str) -> u32 {
    name.bytes().fold(2_166_136_261u32, |h, b| {
        (h ^ b as u32).wrapping_mul(16_777_619)
    })
}

/// Smooth value noise on a 6x6 grid, in 0..1.
struct Noise([f32; 49]);
impl Noise {
    fn new(rng: &mut Rng) -> Self {
        let mut g = [0.; 49];
        for v in &mut g {
            *v = rng.next();
        }
        Noise(g)
    }
    fn at(&self, u: f32, v: f32) -> f32 {
        let s = |t: f32| t * t * (3. - 2. * t);
        let (x, y) = (u * 6., v * 6.);
        let (x0, y0) = ((x as usize).min(5), (y as usize).min(5));
        let (fx, fy) = (s(x - x0 as f32), s(y - y0 as f32));
        let g = |i: usize, j: usize| self.0[j * 7 + i];
        let (a, b, c, d) = (g(x0, y0), g(x0 + 1, y0), g(x0, y0 + 1), g(x0 + 1, y0 + 1));
        a + (b - a) * fx + (c - a) * fy + (a - b - c + d) * fx * fy
    }
}

/// The level, 0..=5, of the cell at `(u, v)` (0..1 across and down) in a field from `seed`.
fn level(u: f32, v: f32, centre: f32, width: f32, noise: &Noise, threshold: f32) -> usize {
    let dx = (u - centre) / width;
    let dy = (v - 1.06) / HEIGHT;
    let mut g = (-(dx * dx + dy * dy) * 1.15).exp();
    let n = noise.at(u, v);
    g *= 1. - SHAPE * 0.55 + SHAPE * 0.9 * n;
    g += SHAPE * 0.10 * (n - 0.55).max(0.) * (v - 0.35).max(0.);
    let mut value = (g * INTENSITY * 1.25).clamp(0., 1.);
    value *= ((v - 0.04) / 0.5).clamp(0., 1.).powf(0.6);
    ((value * 5. + threshold).floor() as i32).clamp(0, 5) as usize
}

/// The wallpaper's pixels, RGBA, `key.size` big.
pub fn paint(key: &Key) -> RgbaImage {
    let (w, h) = key.size;
    let cell = key.cell.max(1);
    let (cols, rows) = (w.div_ceil(cell).max(1), h.div_ceil(cell).max(1));
    let colors = palette(key.color);
    let mut rng = Rng(key.seed);
    let noise = Noise::new(&mut Rng(key.seed ^ 0x9e37_79b9));
    let centre = 0.5 + (rng.next() - 0.5) * 0.16;
    let width = 0.62 + (rng.next() - 0.5) * 0.12;
    let mut out = RgbaImage::new(w, h);
    for row in 0..rows {
        for col in 0..cols {
            let (u, v) = (col as f32 / cols as f32, row as f32 / rows as f32);
            let [r, g, b] = colors[level(u, v, centre, width, &noise, bayer8(col, row))];
            for y in row * cell..((row + 1) * cell).min(h) {
                for x in col * cell..((col + 1) * cell).min(w) {
                    out.put_pixel(x, y, image::Rgba([r, g, b, 255]));
                }
            }
        }
    }
    out
}

struct Texture;

impl Asset for Texture {
    type Source = Key;
    type Output = Arc<RenderImage>;

    fn load(key: Key, _cx: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        async move {
            let mut pixels = paint(&key);
            // GPUI takes BGRA.
            for pixel in pixels.pixels_mut() {
                pixel.0.swap(0, 2);
            }
            Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]))
        }
    }
}

thread_local! {
    /// The last finished picture, shown while a new size is made.
    static LAST: RefCell<Option<(Key, Arc<RenderImage>)>> = const { RefCell::new(None) };
}

/// The key for a card `card` points big at `scale`, its size rounded up to `STEP`.
pub fn key_for(color: u32, seed: u32, card: (f32, f32), scale: f32) -> Option<Key> {
    let round = |points: f32| {
        let pixels = (points * scale).ceil().max(0.) as u32;
        pixels.div_ceil(STEP) * STEP
    };
    let size = (round(card.0), round(card.1));
    (size.0 > 0 && size.1 > 0).then(|| Key {
        color,
        seed,
        size,
        cell: (CELL * scale).round().max(1.) as u32,
    })
}

/// The wallpaper for a card `card` points big, anchored to its bottom middle and painted 1:1;
/// the caller clips it to the card. None until a picture is ready.
pub fn layer(
    color: u32,
    seed: u32,
    card: (f32, f32),
    window: &mut Window,
    cx: &mut App,
) -> Option<gpui::AnyElement> {
    let scale = window.scale_factor();
    let key = key_for(color, seed, card, scale)?;
    let image = match window.use_asset::<Texture>(&key, cx) {
        Some(image) => {
            LAST.with(|last| *last.borrow_mut() = Some((key.clone(), image.clone())));
            Some((key, image))
        }
        // While a new size is made, the last one of the same project stands in.
        None => LAST.with(|last| {
            last.borrow()
                .clone()
                .filter(|(last, _)| last.color == color && last.seed == seed)
        }),
    };
    let (key, image) = image?;
    let (w, h) = (key.size.0 as f32 / scale, key.size.1 as f32 / scale);
    Some(
        gpui::img(image)
            .absolute()
            .bottom_0()
            .left(gpui::px((card.0 - w) / 2.))
            .w(gpui::px(w))
            .h(gpui::px(h))
            .object_fit(ObjectFit::Fill)
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bayer_matrix_holds_each_threshold_once() {
        let mut seen: Vec<u32> = (0..64)
            .map(|i| (bayer8(i % 8, i / 8) * 64. - 0.5).round() as u32)
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..64).collect::<Vec<_>>());
        assert_eq!(bayer8(0, 0), 0.5 / 64.);
    }

    #[test]
    fn the_glow_sits_at_the_bottom_and_the_top_stays_black() {
        let key = Key {
            color: 0xc2557a,
            seed: seed_of("Sirus"),
            size: (480, 300),
            cell: 6,
        };
        let image = paint(&key);
        let lit = |y0: u32, y1: u32| {
            (y0..y1)
                .flat_map(|y| (0..480).map(move |x| (x, y)))
                .filter(|(x, y)| image.get_pixel(*x, *y).0[..3] != BASE)
                .count()
        };
        assert_eq!(lit(0, 30), 0);
        assert!(lit(240, 300) > lit(120, 180));
        assert!(lit(240, 300) > 0);
    }

    #[test]
    fn a_resize_within_a_step_keeps_the_same_picture() {
        let a = key_for(1, 2, (801., 601.), 2.).unwrap();
        let b = key_for(1, 2, (820., 605.), 2.).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.size, (1664, 1216));
        assert_eq!(a.cell, 6);
        assert!(key_for(1, 2, (0., 600.), 2.).is_none());
    }
}
