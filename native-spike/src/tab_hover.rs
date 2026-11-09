//! The hover on a title tab, as the reference app draws it: the plate lifts from 4% to 7% ink
//! and the title brightens from 50% to full ink. Here both ride one eased level per tab instead
//! of switching at once, so the tab "lights up" and settles back.
use crate::motion::{Curve, Tween};
use std::collections::HashMap;
use std::time::Instant;

/// How long a tab takes to light up or fade back.
const GLOW: f32 = 0.14;

#[derive(Default)]
pub struct TabGlow {
    hovered: Option<u64>,
    levels: HashMap<u64, Tween>,
}

impl TabGlow {
    /// The pointer entered (`on`) or left the tab `key`; a late leave of a tab the pointer
    /// already moved off does not clear the newer one.
    pub fn hover(&mut self, key: u64, on: bool) {
        if on {
            self.hovered = Some(key);
        } else if self.hovered == Some(key) {
            self.hovered = None;
        }
    }

    /// Heads every level for its target; true while one still moves.
    pub fn step(&mut self, now: Instant) -> bool {
        if let Some(key) = self.hovered {
            self.levels
                .entry(key)
                .or_insert_with(|| Tween::still(0., now));
        }
        let hovered = self.hovered;
        for (key, level) in self.levels.iter_mut() {
            let to = if hovered == Some(*key) { 1. } else { 0. };
            level.set(to, now, GLOW, Curve::Quad);
        }
        self.levels
            .retain(|key, level| hovered == Some(*key) || !(level.done(now) && level.to == 0.));
        self.levels.values().any(|level| !level.done(now))
    }

    /// 0 at rest, 1 fully lit.
    pub fn level(&self, key: u64, now: Instant) -> f32 {
        self.levels.get(&key).map_or(0., |level| level.at(now))
    }
}

/// The plate's ink at `level`: what a tab at rest, not selected, shows up to the lifted one.
pub fn plate_ink(level: f32) -> f32 {
    0.04 + 0.03 * level
}

/// The title's ink at `level`.
pub fn title_ink(level: f32) -> f32 {
    0.5 + 0.5 * level
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_hovered_tab_lights_up_and_fades_back_once_left() {
        let start = Instant::now();
        let mut glow = TabGlow::default();
        glow.hover(7, true);
        assert!(glow.step(start));
        assert!(glow.level(7, start) < 0.01);
        let later = start + Duration::from_secs(1);
        assert!(!glow.step(later));
        assert!((glow.level(7, later) - 1.).abs() < 0.001);
        glow.hover(7, false);
        assert!(glow.step(later));
        let end = later + Duration::from_secs(1);
        assert!(!glow.step(end));
        assert_eq!(glow.level(7, end), 0.);
        assert!(glow.levels.is_empty());
    }

    #[test]
    fn a_late_leave_does_not_clear_the_newer_hover() {
        let mut glow = TabGlow::default();
        glow.hover(1, true);
        glow.hover(2, true);
        glow.hover(1, false);
        assert_eq!(glow.hovered, Some(2));
    }

    #[test]
    fn the_inks_run_from_rest_to_the_lifted_values() {
        assert_eq!(plate_ink(0.), 0.04);
        assert!((plate_ink(1.) - 0.07).abs() < 1e-6);
        assert_eq!(title_ink(0.), 0.5);
        assert_eq!(title_ink(1.), 1.);
    }
}
