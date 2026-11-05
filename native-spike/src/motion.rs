//! The strip and sidebar animate with one small tween. Durations and curves follow
//! Monocode's tab_motion.rs and shell.rs (200 ms ease-out, 160 ms reorder).
use std::time::Instant;

pub const TAB_OPEN: f32 = 0.20;
pub const REORDER: f32 = 0.16;
pub const SIDEBAR: f32 = 0.24;
pub const DISCLOSE: f32 = 0.22;
pub const SWITCH: f32 = 0.16;
pub const MENU: f32 = 0.12;

#[derive(Clone, Copy, PartialEq)]
pub enum Curve {
    /// cubic-bezier(1/3, 2/3, 2/3, 1): quadratic ease-out.
    Quad,
    /// cubic-bezier(.22, 1, .36, 1).
    Expo,
}

fn bezier(t: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let at = |a: f32, b: f32, s: f32| 3. * a * s * (1. - s).powi(2) + 3. * b * s * s * (1. - s) + s.powi(3);
    let (mut low, mut high) = (0f32, 1f32);
    for _ in 0..12 {
        let mid = (low + high) / 2.;
        if at(x1, x2, mid) < t { low = mid } else { high = mid }
    }
    at(y1, y2, (low + high) / 2.)
}

pub fn eased(t: f32, curve: Curve) -> f32 {
    let t = t.clamp(0., 1.);
    match curve {
        Curve::Quad => 1. - (1. - t) * (1. - t),
        Curve::Expo => bezier(t, 0.22, 1., 0.36, 1.),
    }
}

#[derive(Clone, Copy)]
pub struct Tween {
    from: f32,
    pub to: f32,
    start: Instant,
    duration: f32,
    curve: Curve,
}

impl Tween {
    pub fn still(value: f32, now: Instant) -> Self {
        Self { from: value, to: value, start: now, duration: 0., curve: Curve::Quad }
    }
    pub fn from_to(from: f32, to: f32, now: Instant, duration: f32, curve: Curve) -> Self {
        Self { from, to, start: now, duration, curve }
    }
    pub fn at(&self, now: Instant) -> f32 {
        if self.duration <= 0. {
            return self.to;
        }
        let t = now.saturating_duration_since(self.start).as_secs_f32() / self.duration;
        self.from + (self.to - self.from) * eased(t, self.curve)
    }
    pub fn done(&self, now: Instant) -> bool {
        self.duration <= 0. || now.saturating_duration_since(self.start).as_secs_f32() >= self.duration
    }
    /// Heads for `to` from wherever it is now; a repeated target keeps the running motion.
    pub fn set(&mut self, to: f32, now: Instant, duration: f32, curve: Curve) {
        if (to - self.to).abs() < 0.01 {
            return;
        }
        *self = Self { from: self.at(now), to, start: now, duration, curve };
    }
    pub fn snap(&mut self, value: f32, now: Instant) {
        *self = Self::still(value, now);
    }
}
