//! The transcript's follow of its end, as a glide. GPUI's `FollowMode::Tail` snaps the list to
//! the end in the very layout that grew it, so a sent message or a growing reply jumped. The list
//! is kept paused instead and this chases the end a little each frame: fast at first, easing as
//! it arrives. Scrolling up leaves the end; coming
//! back to it, or sending, follows again. The rules are plain functions, tested without a window.
//! This module is a child of `shell` (`#[path]`).

use std::time::Instant;

/// How quickly the remaining distance closes, per second (a 1/e every 125 ms).
const EASE: f32 = 8.;
/// The fastest the list may travel, in px per second.
const SPEED: f32 = 1600.;
/// The jump button's rate: 1/e every 33 ms, so a view closes in about 250 ms.
const FAST_EASE: f32 = 30.;
/// The longest the jump button's glide takes, in seconds.
#[cfg(test)]
const FAST_MAX: f32 = 0.28;
/// Within this many px of the end counts as at the end.
const AT_END: f32 = 2.;
/// A move up of more than this, that this did not make, is the person scrolling away.
const MOVED_UP: f32 = 3.;

pub(crate) struct Glide {
    /// The list follows its end.
    pub stick: bool,
    /// The last turn is anchored to the top of the view when the person sent it from here, with
    /// room below it, so a sent message glides up and stays there.
    pub anchored: bool,
    /// The jump button's glide: quick whatever the distance, until it arrives.
    fast: bool,
    expected: Option<f32>,
    last: Option<Instant>,
}

pub(crate) struct Plan {
    /// Pixels to scroll down by this frame.
    pub scroll: f32,
    /// More frames are needed.
    pub moving: bool,
}

impl Default for Glide {
    fn default() -> Self {
        Self {
            stick: true,
            anchored: false,
            fast: false,
            expected: None,
            last: None,
        }
    }
}

impl Glide {
    /// Sending, or the jump button: follow the end again, and keep the sent turn at the top.
    pub fn follow(&mut self, anchored: bool) {
        self.stick = true;
        self.anchored |= anchored;
        self.expected = None;
    }

    /// The jump button: follow the end quickly. A long way back lands a view above it first,
    /// then the last view glides in under `FAST_MAX`.
    pub fn follow_fast(&mut self) {
        self.follow(false);
        self.fast = true;
    }

    /// The person (or a search hit, or a trail tick) took the list somewhere else.
    pub fn leave(&mut self) {
        self.stick = false;
        self.fast = false;
        self.expected = None;
    }

    /// One frame. `current` and `max` are the scroll offset and its largest value in px, `view`
    /// the viewport's height.
    pub fn step(&mut self, current: f32, max: f32, view: f32, now: Instant, reduce: bool) -> Plan {
        let dt = self
            .last
            .map_or(1. / 60., |at| {
                now.saturating_duration_since(at).as_secs_f32()
            })
            .clamp(1. / 120., 1. / 20.);
        self.last = Some(now);
        if self
            .expected
            .is_some_and(|expected| current < expected - MOVED_UP)
        {
            self.stick = false;
            self.fast = false;
        }
        if !self.stick {
            self.expected = None;
            if max - current <= AT_END {
                self.stick = true;
            } else {
                return Plan {
                    scroll: 0.,
                    moving: false,
                };
            }
        }
        let remaining = (max - current).max(0.);
        if remaining < 0.5 {
            self.fast = false;
            self.expected = Some(current);
            return Plan {
                scroll: 0.,
                moving: false,
            };
        }
        let scroll = if reduce {
            remaining
        } else if self.fast {
            let eased = (remaining * (1. - (-FAST_EASE * dt).exp())).max(1.);
            // Further than a view: land a view above the end, glide only that last view.
            let jump = (remaining - view.max(1.)).max(0.);
            (eased + jump).min(remaining)
        } else {
            let eased = (remaining * (1. - (-EASE * dt).exp()))
                .min(SPEED * dt)
                .max(0.5);
            // Far away (a long way back to the end): come within a view first, then glide.
            let jump = (remaining - 2. * view.max(1.)).max(0.);
            (eased + jump).min(remaining)
        };
        self.expected = Some(current + scroll);
        Plan {
            scroll,
            moving: scroll < remaining,
        }
    }

    /// Frames are paced by the caller; this is the time a glide of `px` takes at most, for tests.
    #[cfg(test)]
    fn frame(now: Instant, n: u32) -> Instant {
        now + std::time::Duration::from_millis(16 * n as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(glide: &mut Glide, mut current: f32, max: f32, frames: u32) -> (f32, u32) {
        let start = Instant::now();
        for n in 0..frames {
            let plan = glide.step(current, max, 600., Glide::frame(start, n), false);
            current += plan.scroll;
            assert!(current <= max + 0.001, "never past the end");
            if !plan.moving && plan.scroll == 0. {
                return (current, n);
            }
        }
        (current, frames)
    }

    #[test]
    fn a_sent_message_glides_to_the_end_and_stops() {
        let mut glide = Glide::default();
        let (end, frames) = run(&mut glide, 1000., 1600., 400);
        assert!(end > 1599.4, "arrives: {end}");
        assert!(frames > 15, "over several frames, not one jump: {frames}");
        assert!(frames < 120, "and not forever: {frames}");
    }

    #[test]
    fn a_growing_reply_closes_its_few_pixels_gently() {
        let mut glide = Glide::default();
        let first = glide.step(1000., 1020., 600., Instant::now(), false);
        assert!(
            first.scroll > 0. && first.scroll < 20.,
            "a part of it: {}",
            first.scroll
        );
        assert!(first.moving);
    }

    #[test]
    fn a_long_way_back_comes_within_two_views_first() {
        let mut glide = Glide::default();
        let plan = glide.step(0., 10_000., 600., Instant::now(), false);
        assert!(plan.scroll > 8_700., "{}", plan.scroll);
    }

    #[test]
    fn scrolling_up_leaves_the_end_and_reaching_it_again_follows() {
        let mut glide = Glide::default();
        let now = Instant::now();
        glide.step(1000., 1100., 600., now, false);
        // The list is found well above where the last frame put it.
        let plan = glide.step(700., 1100., 600., Glide::frame(now, 1), false);
        assert!(!glide.stick && plan.scroll == 0.);
        // The person scrolls back to the bottom.
        glide.step(1099., 1100., 600., Glide::frame(now, 2), false);
        assert!(glide.stick);
    }

    #[test]
    fn sending_follows_again_from_anywhere_and_anchors_the_turn() {
        let mut glide = Glide::default();
        glide.leave();
        glide.follow(true);
        assert!(glide.stick && glide.anchored);
        assert!(glide.step(100., 900., 600., Instant::now(), false).scroll > 0.);
    }

    #[test]
    fn reduced_motion_goes_straight_to_the_end() {
        let mut glide = Glide::default();
        let plan = glide.step(100., 900., 600., Instant::now(), true);
        assert_eq!(plan.scroll, 800.);
        assert!(!plan.moving);
    }

    #[test]
    fn the_jump_button_arrives_within_the_cap_from_any_distance() {
        for far in [800., 5_000., 200_000.] {
            let mut glide = Glide::default();
            glide.leave();
            glide.follow_fast();
            let start = Instant::now();
            let mut current = 0.;
            let mut frames = 0;
            while frames < 200 {
                let plan = glide.step(current, far, 600., Glide::frame(start, frames), false);
                current += plan.scroll;
                if !plan.moving {
                    break;
                }
                frames += 1;
            }
            let took = frames as f32 * 0.016;
            assert!(current > far - 1., "arrives: {current}");
            assert!(took <= FAST_MAX, "{far}: {took}s");
        }
    }
}
