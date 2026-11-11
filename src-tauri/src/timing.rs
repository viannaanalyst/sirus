//! Stage timings of the send path, printed to stderr when `SIRUS_TIMING=1` (ADR-211).
//! Costs one cached env read when off.

use std::sync::OnceLock;
use std::time::Instant;

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("SIRUS_TIMING").is_some_and(|v| v != "0"))
}

/// Prints each stage with the time since the previous mark and since the clock started.
pub(crate) struct Clock {
    label: &'static str,
    start: Instant,
    last: Instant,
}

impl Clock {
    pub(crate) fn new(label: &'static str) -> Self {
        let now = Instant::now();
        Self {
            label,
            start: now,
            last: now,
        }
    }
    pub(crate) fn mark(&mut self, stage: &str) {
        if !enabled() {
            return;
        }
        let now = Instant::now();
        eprintln!(
            "[timing] {} {stage}: +{:.1}ms (total {:.1}ms)",
            self.label,
            (now - self.last).as_secs_f64() * 1e3,
            (now - self.start).as_secs_f64() * 1e3
        );
        self.last = now;
    }
}
