//! The strip and sidebar animate with one small tween. Durations and curves follow
//! Monocode's tab_motion.rs and shell.rs (200 ms ease-out, 160 ms reorder).
use std::time::Instant;

pub const TAB_OPEN: f32 = 0.20;
pub const REORDER: f32 = 0.16;
pub const SIDEBAR: f32 = 0.24;
pub const DISCLOSE: f32 = 0.34;
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
    let at = |a: f32, b: f32, s: f32| {
        3. * a * s * (1. - s).powi(2) + 3. * b * s * s * (1. - s) + s.powi(3)
    };
    let (mut low, mut high) = (0f32, 1f32);
    for _ in 0..12 {
        let mid = (low + high) / 2.;
        if at(x1, x2, mid) < t {
            low = mid
        } else {
            high = mid
        }
    }
    at(y1, y2, (low + high) / 2.)
}

pub fn eased(t: f32, curve: Curve) -> f32 {
    let t = t.clamp(0., 1.);
    if t == 0. || t == 1. {
        return t;
    }
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
        Self {
            from: value,
            to: value,
            start: now,
            duration: 0.,
            curve: Curve::Quad,
        }
    }
    pub fn from_to(from: f32, to: f32, now: Instant, duration: f32, curve: Curve) -> Self {
        Self {
            from,
            to,
            start: now,
            duration,
            curve,
        }
    }
    pub fn at(&self, now: Instant) -> f32 {
        if self.duration <= 0. {
            return self.to;
        }
        let t = now.saturating_duration_since(self.start).as_secs_f32() / self.duration;
        self.from + (self.to - self.from) * eased(t, self.curve)
    }
    pub fn done(&self, now: Instant) -> bool {
        self.duration <= 0.
            || now.saturating_duration_since(self.start).as_secs_f32() >= self.duration
    }
    /// Heads for `to` from wherever it is now; a repeated target keeps the running motion.
    pub fn set(&mut self, to: f32, now: Instant, duration: f32, curve: Curve) {
        if (to - self.to).abs() < 0.01 {
            return;
        }
        *self = Self {
            from: self.at(now),
            to,
            start: now,
            duration,
            curve,
        };
    }
    pub fn snap(&mut self, value: f32, now: Instant) {
        *self = Self::still(value, now);
    }
}

/// How far a motion that starts at `start`, waits `delay` and then runs `duration` has got at
/// `now`, eased: 0 before it begins, 1 once it ends. A zero duration (reduced motion) is 1.
pub fn progress(start: Instant, now: Instant, delay: f32, duration: f32, curve: Curve) -> f32 {
    if duration <= 0. {
        return 1.;
    }
    let seconds = now.saturating_duration_since(start).as_secs_f32() - delay;
    eased(seconds / duration, curve)
}

/// A number that counts from the value it showed to the one it should show.
#[derive(Clone, Copy)]
pub struct Count {
    from: f64,
    to: f64,
    start: Instant,
    duration: f32,
}

impl Count {
    /// Shows `value` with no motion; the first target counts up from it.
    pub fn at_rest(value: f64, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            start: now,
            duration: 0.,
        }
    }
    pub fn value(&self, now: Instant) -> f64 {
        let t = progress(self.start, now, 0., self.duration, Curve::Expo) as f64;
        self.from + (self.to - self.from) * t
    }
    pub fn done(&self, now: Instant) -> bool {
        self.duration <= 0.
            || now.saturating_duration_since(self.start).as_secs_f32() >= self.duration
    }
    /// The value to show for `target`. A new target restarts the count from what is on screen;
    /// the same target keeps the count running.
    pub fn sample(&mut self, target: f64, now: Instant, duration: f32) -> f64 {
        if (target - self.to).abs() > 1e-9 {
            *self = Self {
                from: self.value(now),
                to: target,
                start: now,
                duration,
            };
        }
        self.value(now)
    }
}

/// A spinner that finishes the turn it is on before it stops, so it rests upright.
#[derive(Clone, Copy, Default)]
pub struct Spin {
    start: Option<Instant>,
    stop: Option<Instant>,
}

impl Spin {
    const PERIOD: f32 = 0.9;
    /// The fraction of a turn to draw, or None at rest.
    pub fn turn(&mut self, busy: bool, now: Instant) -> Option<f32> {
        if busy {
            self.stop = None;
        }
        let start = match self.start {
            Some(start) => start,
            None if busy => {
                self.start = Some(now);
                now
            }
            None => return None,
        };
        let seconds = |at: Instant| at.saturating_duration_since(start).as_secs_f32();
        if !busy {
            let stop = *self.stop.get_or_insert(now);
            let end = (seconds(stop) / Self::PERIOD).ceil() * Self::PERIOD;
            if seconds(now) >= end {
                *self = Self::default();
                return None;
            }
        }
        Some((seconds(now) % Self::PERIOD) / Self::PERIOD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn later(t: Instant, ms: u64) -> Instant {
        t + Duration::from_millis(ms)
    }

    #[test]
    fn curves_start_at_zero_end_at_one_and_never_go_back() {
        for curve in [Curve::Quad, Curve::Expo] {
            assert_eq!(eased(0., curve), 0.);
            assert!((eased(1., curve) - 1.).abs() < 1e-3);
            let mut last = 0.;
            for i in 0..=100 {
                let v = eased(i as f32 / 100., curve);
                assert!(v >= last - 1e-4);
                last = v;
            }
            assert_eq!(eased(2., curve), eased(1., curve));
        }
    }

    #[test]
    fn a_tween_runs_from_to_and_is_done_after_its_duration() {
        let t0 = Instant::now();
        let tween = Tween::from_to(10., 20., t0, 0.2, Curve::Expo);
        assert_eq!(tween.at(t0), 10.);
        let mid = tween.at(later(t0, 100));
        assert!(mid > 10. && mid < 20.);
        assert!(!tween.done(later(t0, 100)));
        assert!(tween.done(later(t0, 200)));
        assert!((tween.at(later(t0, 500)) - 20.).abs() < 1e-3);
    }

    #[test]
    fn a_zero_length_tween_is_at_its_target_at_once() {
        let t0 = Instant::now();
        let tween = Tween::from_to(0., 1., t0, 0., Curve::Expo);
        assert_eq!(tween.at(t0), 1.);
        assert!(tween.done(t0));
    }

    #[test]
    fn set_retargets_from_where_it_is_and_ignores_a_repeat() {
        let t0 = Instant::now();
        let mut tween = Tween::from_to(0., 1., t0, 0.2, Curve::Expo);
        let at = later(t0, 50);
        let seen = tween.at(at);
        tween.set(0., at, 0.15, Curve::Expo);
        assert!((tween.at(at) - seen).abs() < 1e-4);
        assert!(tween.at(later(t0, 120)) < seen);
        let before = tween.at(later(t0, 120));
        tween.set(0., later(t0, 120), 0.15, Curve::Expo);
        assert_eq!(tween.at(later(t0, 120)), before);
    }

    #[test]
    fn progress_waits_for_its_delay_and_reduced_motion_is_final() {
        let t0 = Instant::now();
        assert_eq!(progress(t0, later(t0, 100), 0.32, 0.2, Curve::Quad), 0.);
        let mid = progress(t0, later(t0, 420), 0.32, 0.2, Curve::Quad);
        assert!(mid > 0. && mid < 1.);
        assert_eq!(progress(t0, later(t0, 600), 0.32, 0.2, Curve::Quad), 1.);
        assert_eq!(progress(t0, t0, 0.32, 0., Curve::Quad), 1.);
    }

    #[test]
    fn a_count_goes_from_the_old_value_to_the_new() {
        let t0 = Instant::now();
        let mut count = Count::at_rest(0., t0);
        assert_eq!(count.sample(100., t0, 0.32), 0.);
        let mid = count.sample(100., later(t0, 160), 0.32);
        assert!(mid > 0. && mid < 100.);
        assert!(!count.done(later(t0, 160)));
        assert!((count.sample(100., later(t0, 400), 0.32) - 100.).abs() < 1e-6);
        assert!(count.done(later(t0, 400)));
    }

    #[test]
    fn a_new_target_counts_from_what_is_showing() {
        let t0 = Instant::now();
        let mut count = Count::at_rest(0., t0);
        count.sample(100., t0, 0.32);
        let shown = count.sample(100., later(t0, 100), 0.32);
        let restart = count.sample(40., later(t0, 100), 0.32);
        assert!((restart - shown).abs() < 1e-6);
        assert!((count.sample(40., later(t0, 600), 0.32) - 40.).abs() < 1e-6);
    }

    #[test]
    fn a_count_with_no_duration_shows_the_final_value() {
        let t0 = Instant::now();
        let mut count = Count::at_rest(5., t0);
        assert_eq!(count.sample(80., t0, 0.), 80.);
        assert!(count.done(t0));
    }

    #[test]
    fn a_spinner_finishes_its_turn_before_resting() {
        let t0 = Instant::now();
        let mut spin = Spin::default();
        assert_eq!(spin.turn(false, t0), None);
        assert_eq!(spin.turn(true, t0), Some(0.));
        let quarter = spin.turn(true, later(t0, 225)).unwrap();
        assert!((quarter - 0.25).abs() < 0.01);
        // Stopped at 0.45 s: it carries on to 0.9 s, then rests.
        assert!(spin.turn(false, later(t0, 450)).is_some());
        assert!(spin.turn(false, later(t0, 800)).is_some());
        assert_eq!(spin.turn(false, later(t0, 950)), None);
        assert_eq!(spin.turn(false, later(t0, 1200)), None);
    }

    #[test]
    fn a_spinner_that_is_busy_again_keeps_turning() {
        let t0 = Instant::now();
        let mut spin = Spin::default();
        spin.turn(true, t0);
        spin.turn(false, later(t0, 300));
        assert!(spin.turn(true, later(t0, 500)).is_some());
        assert!(spin.turn(true, later(t0, 2000)).is_some());
    }
}
