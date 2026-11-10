//! A progress bar drawn as dither cells instead of a smooth rounded fill. The filled part is
//! ordered-dithered (Bayer 8x8) from a half-inked start to a solid end, and the empty part is a
//! faint dotted track. One canvas paints the whole bar, so a marker laid over it stays on top.
use crate::project_wallpaper::bayer8;
use gpui::{App, Bounds, Hsla, IntoElement, Styled, Window, canvas, fill, point, px, size};

/// The bar's height, in logical pixels.
pub const HEIGHT: f32 = 6.;
/// One cell's side.
const CELL: f32 = 2.;
/// The space between cells, across and down.
const GAP: f32 = 0.4;
/// The ink at the fill's start; it rises to a solid cell at the fill's end.
const START: f32 = 0.55;
/// The empty track keeps a dot in every third column.
const TRACK_EVERY: usize = 3;
/// The track's dots are white at this alpha.
const TRACK_ALPHA: f32 = 0.06;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Dot {
    /// A cell of the fill, in the bar's colour.
    Ink,
    /// A cell of the empty track.
    Track,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Cell {
    x: f32,
    y: f32,
    dot: Dot,
}

/// Whether the cell at (col, row) is inked at `density` (0..1), by its Bayer threshold.
fn lit(col: usize, row: usize, density: f32) -> bool {
    density > bayer8((col % 8) as u32, (row % 8) as u32)
}

/// How many cells fit along `length`, with the gap between them.
fn fit(length: f32) -> usize {
    ((length + GAP) / (CELL + GAP)).floor().max(0.) as usize
}

/// The ink at `x` in a fill that ends at `filled`: rising from `START` to 1 at the fill's end.
fn density(x: f32, filled: f32) -> f32 {
    START + (1. - START) * ((x + CELL) / filled).min(1.)
}

/// The cells of a bar `width` by `height` whose fill is `share` (0..1) of its width. The rows are
/// centred across the height, so six pixels hold two rows.
fn cells(width: f32, height: f32, share: f32) -> Vec<Cell> {
    let pitch = CELL + GAP;
    let rows = fit(height);
    let columns = fit(width);
    let top = (height - (rows as f32 * pitch - GAP)) / 2.;
    let filled = share.clamp(0., 1.) * width;
    let mut out = Vec::new();
    for row in 0..rows {
        let y = top + row as f32 * pitch;
        for col in 0..columns {
            let x = col as f32 * pitch;
            if x + CELL / 2. < filled {
                if lit(col, row, density(x, filled)) {
                    out.push(Cell {
                        x,
                        y,
                        dot: Dot::Ink,
                    });
                }
            } else if col % TRACK_EVERY == 0 {
                out.push(Cell {
                    x,
                    y,
                    dot: Dot::Track,
                });
            }
        }
    }
    out
}

/// A bar filled to `share` (0..1) of its width, in `color`, dithered as described above. It takes
/// the width it is given and is `HEIGHT` tall.
pub fn pixel_bar(share: f32, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _: &mut Window, _: &mut App| {},
        move |bounds, _, window: &mut Window, _: &mut App| {
            let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            for cell in cells(width, height, share) {
                let tint = match cell.dot {
                    Dot::Ink => color,
                    Dot::Track => Hsla {
                        a: TRACK_ALPHA,
                        ..gpui::white()
                    },
                };
                window.paint_quad(fill(
                    Bounds::new(
                        bounds.origin + point(px(cell.x), px(cell.y)),
                        size(px(CELL), px(CELL)),
                    ),
                    tint,
                ));
            }
        },
    )
    .w_full()
    .h(px(HEIGHT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_pixels_hold_two_rows_centred() {
        assert_eq!(fit(HEIGHT), 2);
        let track = cells(100., HEIGHT, 0.);
        let mut ys: Vec<f32> = track.iter().map(|c| c.y).collect();
        ys.sort_by(f32::total_cmp);
        ys.dedup();
        // Two 2 px rows and the 0.4 px gap between them make 4.4 px, centred in 6 px.
        assert_eq!(ys.len(), 2);
        assert!((ys[0] - 0.8).abs() < 1e-4);
        assert!((ys[1] + CELL - 5.2).abs() < 1e-4);
    }

    #[test]
    fn the_empty_track_is_a_dot_in_every_third_column() {
        let track = cells(100., HEIGHT, 0.);
        assert!(track.iter().all(|c| c.dot == Dot::Track));
        // 41 columns fit in 100 px; columns 0, 3, ..., 39 are 14 of them, in each of 2 rows.
        assert_eq!(track.len(), 2 * 14);
        assert!(track.iter().all(|c| {
            let col = (c.x / (CELL + GAP)).round() as usize;
            col % TRACK_EVERY == 0
        }));
    }

    #[test]
    fn the_fill_starts_near_half_ink_and_is_solid_at_its_end() {
        assert!((density(0., 100.) - START).abs() < 0.01);
        assert!((density(50. - CELL, 50.) - 1.).abs() < 1e-6);
        let drawn = cells(100., HEIGHT, 0.5);
        // Inked cells stay inside the fill, and the track dots start after it.
        assert!(
            drawn
                .iter()
                .filter(|c| c.dot == Dot::Ink)
                .all(|c| c.x + CELL / 2. < 50.)
        );
        assert!(
            drawn
                .iter()
                .filter(|c| c.dot == Dot::Track)
                .all(|c| c.x + CELL / 2. >= 50.)
        );
    }

    #[test]
    fn a_full_fill_inks_every_cell_at_density_one() {
        for row in 0..8 {
            for col in 0..8 {
                assert!(lit(col, row, 1.));
                assert!(!lit(col, row, 0.));
            }
        }
        let inked = (0..8)
            .flat_map(|row| (0..8).map(move |col| lit(col, row, 0.55)))
            .filter(|on| *on)
            .count();
        assert!((33..=37).contains(&inked), "{inked} of 64 cells at 0.55");
    }
}
