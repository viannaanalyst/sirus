//! The new tab's background: a field of letters that draws the project's robot in its colour. It
//! moves away from the pointer and a click sends a wave through it. Glyphs are shaped once and
//! painted cell by cell; between events nothing asks for a frame.
use std::cell::OnceCell;
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, FontId, GlyphId, Hsla, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Rgba, Window, point, prelude::*, px,
};

const CELL_W: f32 = 7.;
const CELL_H: f32 = 11.;
const FONT_SIZE: f32 = 10.;
/// How long a click's wave runs.
const WAVE_S: f32 = 2.4;
const WAVE_SPEED: f32 = 420.;
/// How long a stroke of ink stays before it has faded away.
const INK_S: f32 = 1.6;
const INK_POINTS: usize = 600;
/// From faint to dense.
const RAMP: &str = ".,:;-=+*#%@";

#[derive(Default)]
pub struct State {
    mouse: Option<Point<Pixels>>,
    waves: Vec<(Point<Pixels>, Instant)>,
    /// Ink drawn by dragging over the field: strokes of points, each fading on its own.
    ink: Vec<Vec<(Point<Pixels>, Instant)>>,
    drawing: bool,
}

impl State {
    /// Whether a wave is still running, so the next frame has something to draw.
    pub fn moving(&mut self, now: Instant) -> bool {
        self.waves
            .retain(|(_, at)| now.duration_since(*at).as_secs_f32() < WAVE_S);
        for stroke in &mut self.ink {
            stroke.retain(|(_, at)| now.duration_since(*at).as_secs_f32() < INK_S);
        }
        let drawing = self.drawing;
        let last = self.ink.len().saturating_sub(1);
        let mut at = 0;
        self.ink.retain(|stroke| {
            let keep = !stroke.is_empty() || (drawing && at == last);
            at += 1;
            keep
        });
        !self.waves.is_empty() || self.ink.iter().any(|stroke| !stroke.is_empty())
    }

    /// A press starts a stroke; moving with the button down extends it.
    fn ink_start(&mut self, at: Point<Pixels>, now: Instant) {
        self.drawing = true;
        self.ink.push(vec![(at, now)]);
    }

    fn ink_to(&mut self, at: Point<Pixels>, now: Instant) -> bool {
        if !self.drawing {
            return false;
        }
        if self.ink.iter().map(Vec::len).sum::<usize>() >= INK_POINTS {
            return false;
        }
        match self.ink.last_mut() {
            Some(stroke) => stroke.push((at, now)),
            None => self.ink.push(vec![(at, now)]),
        }
        true
    }

    fn point_at(&mut self, position: Option<Point<Pixels>>) -> bool {
        let changed = self.mouse != position;
        self.mouse = position;
        changed
    }

    fn click(&mut self, at: Point<Pixels>, now: Instant) {
        self.pulse(at, now);
    }

    /// Sends a wave out from `at`, as a click does: the first run's steps use it to change scene.
    pub fn pulse(&mut self, at: Point<Pixels>, now: Instant) {
        self.waves.push((at, now));
        if self.waves.len() > 4 {
            self.waves.remove(0);
        }
    }
}

/// Strength at a distance from a click, `age` seconds after it: a ring moving out and fading.
fn wave_at(distance: f32, age: f32) -> f32 {
    let front = age * WAVE_SPEED;
    (-(distance - front).powi(2) / 5200.).exp() * (-age * 1.5).exp()
}

/// A steady 0..1 per cell, for the grain around the robot.
fn grain(column: i32, row: i32) -> f32 {
    let seed = (column as u32).wrapping_mul(73_856_093) ^ (row as u32).wrapping_mul(19_349_663);
    (seed.wrapping_mul(2_654_435_761) >> 16 & 0xff) as f32 / 255.
}

/// How much of the robot a point shows: 0 outside it and in its eyes, more toward its middle.
/// `gaze` shifts the eyes toward the pointer, in the robot's 64-unit space.
fn figure(shape: &str, x: f32, y: f32, gaze: (f32, f32)) -> f32 {
    if !crate::project_avatar::shape_contains(shape, x, y) {
        return 0.;
    }
    let eye = |cx: f32| ((x - cx - gaze.0) / 5.).powi(2) + ((y - 31. - gaze.1) / 7.).powi(2) < 1.;
    if eye(23.) || eye(41.) {
        return 0.05;
    }
    let from_middle = ((x - 32.).powi(2) + (y - 32.).powi(2)).sqrt() / 32.;
    0.4 + 0.5 * (1. - from_middle).max(0.)
}

/// How far the eyes shift for a pointer at (`mx`, `my`) in a field `width` by `height`: up to
/// about 3.5 units sideways and 2.5 up or down.
fn gaze_toward(mx: f32, my: f32, width: f32, height: f32) -> (f32, f32) {
    let across = ((mx - width / 2.) / width.max(1.)).clamp(-0.5, 0.5);
    let down = ((my - height / 2.) / height.max(1.)).clamp(-0.5, 0.5);
    (across * 7., down * 5.)
}

/// The value a cell is drawn with, 0 to about 1.
fn cell_value(body: f32, wave: f32, near: f32, grain: f32) -> f32 {
    body * 0.85 + wave * 0.8 + near * 0.5 + grain * 0.1
}

fn glyphs(window: &Window) -> (FontId, Vec<GlyphId>) {
    let font = gpui::font("Menlo");
    let layout = window.text_system().layout_line(
        RAMP,
        px(FONT_SIZE),
        &[gpui::TextRun {
            len: RAMP.len(),
            font: font.clone(),
            color: gpui::white(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let font_id = window.text_system().resolve_font(&font);
    let ids = layout
        .runs
        .iter()
        .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.id))
        .collect();
    (font_id, ids)
}

thread_local! {
    static GLYPHS: OnceCell<(FontId, Vec<GlyphId>)> = const { OnceCell::new() };
}

/// Seconds between the robot's hops, and how long a spark lives.
const BEAT: f32 = std::f32::consts::PI / 6.8;
const SPARK_LIFE: f32 = 0.9;
const SPARKS: u32 = 4;

/// A steady number from 0 to 1 for two inputs, so a spark needs no state between frames.
fn noise(a: f32, b: f32) -> f32 {
    ((a * 12.9898 + b * 78.233).sin() * 43758.547).rem_euclid(1.)
}

/// One spark at `clock`: where it is from the robot's starting line (x, rise above it) and how
/// visible, or None once it has landed or faded. `beat` is the hop that threw it.
fn spark_at(beat: u32, spark: u32, width: f32, clock: f32) -> Option<(f32, f32, f32)> {
    let born = beat as f32 * BEAT;
    let age = clock - born;
    if !(0. ..SPARK_LIFE).contains(&age) {
        return None;
    }
    let (b, s) = (beat as f32, spark as f32);
    let (robot, _) = walk(width, born);
    // Thrown back, away from the way the robot is going.
    let back = if walk(width, born + 0.01).0 >= robot {
        -1.
    } else {
        1.
    };
    let x = robot + 13. + (noise(b, s) - 0.5) * 10. + back * (10. + noise(s, b) * 26.) * age;
    let rise = 3. + (22. + noise(b, s + 9.) * 26.) * age - 58. * age * age;
    (rise > 1.).then_some((x, rise, (1. - age / SPARK_LIFE) * 0.9))
}

/// The letters that jump as the robot hops along the composer's top edge while the agent works.
/// `width` hands the line's width to the robot's walk.
pub fn sparks(color: u32, clock: f32, width: std::rc::Rc<std::cell::Cell<f32>>) -> AnyElement {
    let tint = Hsla::from(Rgba {
        r: ((color >> 16) & 0xff) as f32 / 255.,
        g: ((color >> 8) & 0xff) as f32 / 255.,
        b: (color & 0xff) as f32 / 255.,
        a: 1.,
    });
    gpui::canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _: &mut App| {
            let (left, bottom) = (f32::from(bounds.origin.x), f32::from(bounds.bottom()));
            let across = f32::from(bounds.size.width);
            width.set(across);
            let (font_id, ids) = GLYPHS.with(|cell| cell.get_or_init(|| glyphs(window)).clone());
            if ids.is_empty() {
                return;
            }
            let now = (clock / BEAT) as u32;
            for beat in now.saturating_sub((SPARK_LIFE / BEAT) as u32 + 1)..=now {
                for spark in 0..SPARKS {
                    let Some((x, rise, alpha)) = spark_at(beat, spark, across, clock) else {
                        continue;
                    };
                    let letter =
                        (noise(spark as f32 + 2., beat as f32) * ids.len() as f32) as usize;
                    let _ = window.paint_glyph(
                        point(px(left + x), px(bottom - rise)),
                        font_id,
                        ids[letter.min(ids.len() - 1)],
                        px(FONT_SIZE),
                        Hsla { a: alpha, ..tint },
                    );
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// Where the robot walks along a line `width` wide, there and back at 110 px/s, and its hop.
pub fn walk(width: f32, clock: f32) -> (f32, f32) {
    let span = (width - 46.).max(1.);
    let period = span / 110. * 2.;
    let phase = (clock % period) / period;
    let along = if phase < 0.5 {
        phase * 2.
    } else {
        2. - phase * 2.
    };
    (8. + span * along, (clock * 6.8).sin().abs() * 4.)
}

/// The field, filling its parent. `set` reaches the shell's `State` from the mouse listeners.
pub fn field<V: 'static>(
    shape: &'static str,
    color: u32,
    state: &State,
    set: fn(&mut V) -> &mut State,
    view: gpui::WeakEntity<V>,
    still: bool,
) -> AnyElement {
    field_with(shape, color, state, set, view, still, false)
}

/// The same field; with `gaze` the robot's eyes follow the pointer too (the first run's).
pub fn field_with<V: 'static>(
    shape: &'static str,
    color: u32,
    state: &State,
    set: fn(&mut V) -> &mut State,
    view: gpui::WeakEntity<V>,
    still: bool,
    gaze: bool,
) -> AnyElement {
    let mouse = if still { None } else { state.mouse };
    let waves = if still {
        Vec::new()
    } else {
        state.waves.clone()
    };
    let ink = if still { Vec::new() } else { state.ink.clone() };
    let tint = Rgba {
        r: ((color >> 16) & 0xff) as f32 / 255.,
        g: ((color >> 8) & 0xff) as f32 / 255.,
        b: (color & 0xff) as f32 / 255.,
        a: 1.,
    };
    gpui::canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _: &mut App| {
            if !still {
                let moved = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let inside = bounds.contains(&event.position).then_some(event.position);
                        let held = event.pressed_button == Some(MouseButton::Left);
                        let _ = moved.update(cx, |this, cx| {
                            let mut changed = set(this).point_at(inside);
                            if held && let Some(at) = inside {
                                changed |= set(this).ink_to(at, Instant::now());
                            }
                            if changed {
                                cx.notify();
                            }
                        });
                    }
                });
                let clicked = view.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && bounds.contains(&event.position) {
                        let at = event.position;
                        let _ = clicked.update(cx, |this, cx| {
                            set(this).click(at, Instant::now());
                            if event.button == MouseButton::Left {
                                set(this).ink_start(at, Instant::now());
                            }
                            cx.notify();
                        });
                    }
                });
                let lifted = view.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = lifted.update(cx, |this, _| set(this).drawing = false);
                    }
                });
            }
            let now_ink = Instant::now();
            for stroke in &ink {
                for pair in stroke.windows(2) {
                    let ((from, _), (to, at)) = (pair[0], pair[1]);
                    let fade = 1. - now_ink.duration_since(at).as_secs_f32() / INK_S;
                    if fade <= 0. {
                        continue;
                    }
                    let mut path = gpui::PathBuilder::stroke(px(2.5 + 1.5 * fade));
                    path.move_to(from);
                    path.line_to(to);
                    if let Ok(path) = path.build() {
                        window.paint_path(
                            path,
                            Hsla {
                                a: 0.85 * fade * fade,
                                ..Hsla::from(tint)
                            },
                        );
                    }
                }
            }
            let (font_id, ids) = GLYPHS.with(|cell| cell.get_or_init(|| glyphs(window)).clone());
            if ids.is_empty() {
                return;
            }
            let now = Instant::now();
            let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let scale = height * 0.68 / 64.;
            let (left, top) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let ring: Vec<(f32, f32, f32)> = waves
                .iter()
                .map(|(at, start)| {
                    (
                        f32::from(at.x) - left,
                        f32::from(at.y) - top,
                        now.duration_since(*start).as_secs_f32(),
                    )
                })
                .collect();
            let pointer = mouse.map(|m| (f32::from(m.x) - left, f32::from(m.y) - top));
            let look = match pointer {
                Some((mx, my)) if gaze => gaze_toward(mx, my, width, height),
                _ => (0., 0.),
            };
            let columns = (width / CELL_W).ceil() as i32;
            let rows = (height / CELL_H).ceil() as i32;
            for row in 0..rows {
                for column in 0..columns {
                    let (x, y) = (column as f32 * CELL_W, row as f32 * CELL_H);
                    let body = figure(
                        shape,
                        32. + (x - width / 2.) / scale,
                        32. + (y - height * 0.46) / scale,
                        look,
                    );
                    let wave: f32 = ring
                        .iter()
                        .map(|(wx, wy, age)| wave_at((x - wx).hypot(y - wy), *age))
                        .sum();
                    let near = pointer.map_or(0., |(mx, my)| {
                        (-((x - mx).powi(2) + (y - my).powi(2)) / 5200.).exp()
                    });
                    let value = cell_value(body, wave, near, grain(column, row));
                    if value < 0.09 {
                        continue;
                    }
                    let at = ((value * ids.len() as f32) as usize).min(ids.len() - 1);
                    let shade = Hsla::from(tint);
                    let origin = point(
                        px(left + x + wave * (x - width / 2.) * 0.02),
                        px(top + y - wave * 7.),
                    );
                    let _ = window.paint_glyph(
                        origin + point(px(0.), px(FONT_SIZE)),
                        font_id,
                        ids[at],
                        px(FONT_SIZE),
                        Hsla {
                            a: if gaze {
                                (value * 0.9).min(0.8)
                            } else {
                                (value * 0.6).min(0.55)
                            },
                            ..shade
                        },
                    );
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wave_is_a_ring_that_moves_out_and_fades() {
        assert!(wave_at(WAVE_SPEED, 1.) > wave_at(0., 1.));
        assert!(wave_at(WAVE_SPEED * 2., 2.) < wave_at(WAVE_SPEED, 1.));
    }

    #[test]
    fn the_robot_shows_with_its_eyes_empty_and_nothing_outside_it() {
        assert!(figure("round", 32., 50., (0., 0.)) > 0.3);
        assert!(figure("round", 23., 31., (0., 0.)) < 0.1);
        assert_eq!(figure("round", 1., 1., (0., 0.)), 0.);
    }

    #[test]
    fn the_eyes_follow_the_pointer() {
        // Looking right moves the left eye's hole to the right: its old middle fills in.
        assert!(figure("round", 23., 31., (5.5, 0.)) > 0.3);
        assert!(figure("round", 28.5, 31., (5.5, 0.)) < 0.1);
        let (x, y) = gaze_toward(1000., 800., 1000., 800.);
        assert!((x - 3.5).abs() < 0.01 && (y - 2.5).abs() < 0.01);
        assert_eq!(gaze_toward(500., 400., 1000., 800.), (0., 0.));
        assert!(gaze_toward(9000., -9000., 1000., 800.).0 <= 3.5);
    }

    #[test]
    fn a_pulse_starts_a_wave_and_keeps_the_latest_four() {
        let mut state = State::default();
        let start = Instant::now();
        for _ in 0..6 {
            state.pulse(point(px(1.), px(1.)), start);
        }
        assert_eq!(state.waves.len(), 4);
        assert!(state.moving(start));
    }

    #[test]
    fn ink_fades_and_stops_asking_for_frames() {
        let mut state = State::default();
        let start = Instant::now();
        state.ink_start(point(px(1.), px(1.)), start);
        assert!(state.ink_to(point(px(5.), px(5.)), start));
        assert!(state.moving(start));
        state.drawing = false;
        assert!(!state.ink_to(point(px(9.), px(9.)), start));
        assert!(!state.moving(start + std::time::Duration::from_secs(2)));
        assert!(state.ink.is_empty());
    }

    #[test]
    fn finished_waves_stop_asking_for_frames() {
        let mut state = State::default();
        let start = Instant::now();
        state.click(point(px(1.), px(1.)), start);
        assert!(state.moving(start));
        assert!(!state.moving(start + std::time::Duration::from_secs_f32(WAVE_S + 0.1)));
    }

    #[test]
    fn the_robot_walks_there_and_back_inside_the_line() {
        let width = 500.;
        let period = (width - 46.) / 110. * 2.;
        for step in 0..40 {
            let (x, hop) = walk(width, step as f32 * 0.37);
            assert!((8. ..=width - 38.).contains(&x), "{x}");
            assert!((0. ..=4.).contains(&hop));
        }
        assert!((walk(width, 0.).0 - walk(width, period).0).abs() < 0.01);
    }

    #[test]
    fn a_spark_rises_fades_and_lands() {
        let beat = 40;
        let born = beat as f32 * BEAT;
        assert!(spark_at(beat, 0, 600., born - 0.1).is_none());
        let (_, rise, alpha) = spark_at(beat, 0, 600., born + 0.1).unwrap();
        assert!(rise > 1. && alpha > 0.7);
        assert!(spark_at(beat, 0, 600., born + SPARK_LIFE).is_none());
    }

    #[test]
    fn grain_alone_stays_faint() {
        assert!(cell_value(0., 0., 0., 1.) < 0.11);
    }
}
