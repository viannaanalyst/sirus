//! The effort bar's fill as ordered-dither pixels (the approved "D+" look): 2 px cells that thin
//! out toward the empty end and heat from cool violet through the model's tone to a cream core at
//! the thumb, with a slow twinkle, embers that rise from the tip and cool as they go, and with Fast
//! a pixel lightning strike now and then. The empty part keeps a faint dotted track.
use gpui::{Bounds, Hsla, Pixels, Rgba, Window, fill, point, px, size};

pub struct Energy {
    /// How much of the bar is filled, 0 to 1.
    pub fill: f32,
    pub tone: Hsla,
    pub soft: Hsla,
    pub cool: Hsla,
    /// Higher effort is hotter and faster, 0 to 1.
    pub heat: f32,
    pub embers: usize,
    pub fast: bool,
    /// Seconds; a constant stops the motion.
    pub time: f32,
}

pub fn blend(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

/// One cell, in px.
const CELL: f32 = 2.;
/// The cream the hottest cells and new embers reach.
const CREAM: u32 = 0xffe2c4;

/// The 8x8 Bayer threshold of a cell, in (0, 1).
fn bayer(x: usize, y: usize) -> f32 {
    const D: [[u32; 2]; 2] = [[0, 2], [3, 1]];
    let v: u32 = (0..3)
        .map(|i| D[(y >> i) & 1][(x >> i) & 1] * 4u32.pow(2 - i as u32))
        .sum();
    (v as f32 + 0.5) / 64.
}

/// A cheap hash of two integers, in 0..1.
fn hash(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9e37_79b1) ^ b.wrapping_mul(0x85eb_ca77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h % 10_000) as f32 / 10_000.
}

/// The colour `u` (0..1) along the bar: cool, soft, tone, then cream as it heats.
fn ramp(energy: &Energy, u: f32) -> Hsla {
    let cream: Hsla = gpui::rgb(CREAM).into();
    let dim = blend(energy.cool, gpui::black(), 0.45);
    let stops = [
        dim,
        energy.cool,
        energy.soft,
        energy.tone,
        blend(energy.tone, cream, 0.5),
        cream,
    ];
    let at = u.clamp(0., 0.9999) * (stops.len() - 1) as f32;
    let i = at as usize;
    blend(stops[i], stops[i + 1], at - i as f32)
}

/// Whether a cell centre lies inside the pill (a capsule `width` by `height`).
fn in_pill(cx: f32, cy: f32, width: f32, height: f32) -> bool {
    let r = height / 2.;
    let nearest = cx.clamp(r, (width - r).max(r));
    let (dx, dy) = (cx - nearest, cy - r);
    dx * dx + dy * dy <= r * r
}

pub fn paint(bounds: Bounds<Pixels>, energy: &Energy, window: &mut Window) {
    let (x, y) = (f32::from(bounds.left()), f32::from(bounds.top()));
    let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let cols = (width / CELL).ceil() as usize;
    let rows = (height / CELL).ceil() as usize;
    let filled = ((width * energy.fill) / CELL).round() as usize;
    let time = energy.time;
    let mut cell = |cx: usize, cy: f32, color: Hsla| {
        let (px_, py_) = (cx as f32 * CELL, cy * CELL);
        if in_pill(px_ + CELL / 2., py_ + CELL / 2., width, height) {
            window.paint_quad(fill(
                Bounds::new(
                    point(px(x + px_), px(y + py_)),
                    size(px(CELL - 0.4), px(CELL - 0.4)),
                ),
                color,
            ));
        }
    };

    // The empty part: a faint dotted track.
    let track = Hsla {
        a: 0.06,
        ..gpui::white()
    };
    for cx in (filled + 2..cols).step_by(3) {
        for cy in (1..rows).step_by(3) {
            cell(cx, cy as f32, track);
        }
    }

    // The fill: denser and hotter toward the thumb, thinner at the top and bottom edges.
    let half = (rows.max(2) - 1) as f32 / 2.;
    for cx in 0..filled {
        let u = cx as f32 / (filled.max(2) - 1) as f32;
        let twinkle =
            0.06 * (time * 1.8 + cx as f32 * 0.9).sin() * (time * 0.7 + cx as f32 * 0.31).sin();
        let density =
            (0.12 + 0.86 * u.powf(1.8) + twinkle + if energy.fast { 0.05 } else { 0. }).min(1.);
        let color = ramp(energy, 0.15 + 0.62 * u + 0.23 * u.powi(4) * energy.heat);
        for cy in 0..rows {
            let edge = (cy as f32 - half).abs() / half.max(1.);
            if density * (1. - 0.35 * edge * edge) > bayer(cx, cy) {
                cell(cx, cy as f32, color);
            }
        }
    }

    // The core: the last cells before the thumb flicker cream.
    let cream: Hsla = gpui::rgb(CREAM).into();
    let tick = (time * 30.) as usize;
    for cx in filled.saturating_sub(5)..filled {
        for cy in 1..rows.saturating_sub(1) {
            if (cx + cy + tick) % 4 == 0 {
                cell(cx, cy as f32, cream);
            }
        }
    }

    // Embers: each rises from near the tip on its own cycle, cooling and fading as it climbs.
    for i in 0..energy.embers {
        let speed = 0.55 + hash(i as u32, 7) * 0.5;
        let phase = time * speed + hash(i as u32, 11);
        let (cycle, t) = (phase.floor() as u32, phase.fract());
        let start = filled as f32 - 1. - hash(i as u32, cycle) * 8.;
        let drift = (hash(i as u32, cycle + 99) - 0.5) * 3. * t;
        let cx = (start + drift).round().max(0.) as usize;
        let cy = (rows as f32 - 1.) * (1. - t);
        let color = if t < 0.4 {
            cream
        } else if t < 0.7 {
            blend(energy.tone, cream, 0.5)
        } else {
            energy.tone
        };
        cell(
            cx,
            cy.round(),
            Hsla {
                a: color.a * (1. - t),
                ..color
            },
        );
    }

    // Fast: a pixel lightning strike crosses the bar now and then.
    if energy.fast {
        let strike = (time / 1.4).fract();
        if strike < 0.08 {
            let seed = (time / 1.4).floor() as u32;
            let mut cx = (filled as f32 * (0.4 + 0.5 * hash(seed, 3))) as i32;
            for cy in 0..rows {
                cell(cx.max(0) as usize, cy as f32, Hsla { a: 0.9, ..cream });
                cx += if hash(seed, cy as u32) < 0.5 { -1 } else { 1 };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bayer_matrix_holds_each_threshold_once() {
        let mut seen: Vec<u32> = (0..64)
            .map(|i| (bayer(i % 8, i / 8) * 64. - 0.5).round() as u32)
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn cells_outside_the_pill_ends_are_skipped() {
        assert!(in_pill(16., 16., 256., 32.));
        assert!(in_pill(128., 1., 256., 32.));
        assert!(!in_pill(1., 1., 256., 32.));
        assert!(!in_pill(255., 31., 256., 32.));
    }
}
