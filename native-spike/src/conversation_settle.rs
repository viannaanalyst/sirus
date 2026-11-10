//! Idle conversations settle (ADR-198). A conversation's age is the time since its last activity,
//! or since the owner last opened it when that is later. Its sidebar row fades as the age nears the
//! limit, a thin bar under it fills (once it is a day old), a faint dotted texture covers it, stronger
//! as it ages, and at the limit it leaves the list for the docked group. These functions are
//! the whole decision; `shell_settle.rs` draws and stops what they say.

use chrono::{DateTime, Duration, Utc};

/// A row's opacity at the limit. It fades linearly from full when the conversation is fresh.
pub const FADED: f64 = 0.45;

/// A conversation younger than this shows no bar, so "Hoje" has nothing to puzzle over.
pub const BAR_FROM: Duration = Duration::days(1);

/// The bar's opacity, and that of the faint track behind it.
pub const BAR_ALPHA: f32 = 0.55;
pub const TRACK_ALPHA: f32 = 0.08;

/// Every row has the texture: its dots go from the first opacity on a fresh row to the second at
/// the limit, `TEXTURE_GAP` px apart, rising a little faster early on (`TEXTURE_CURVE`).
pub const TEXTURE_ALPHA: (f32, f32) = (0.05, 0.24);
pub const TEXTURE_CURVE: f64 = 0.8;
pub const TEXTURE_GAP: f32 = 6.;

/// The longest limit the Settings field takes; the host refuses more.
pub const MAX_DAYS: u32 = 365;

/// The idle limit, or None when settling is off. The days are the Settings field's number.
pub fn limit_of(enabled: bool, days: u32) -> Option<Duration> {
    (enabled && days > 0).then(|| Duration::days(i64::from(days.min(MAX_DAYS))))
}

/// A stored activity time (RFC 3339), if it is one.
pub fn parse(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|when| when.to_utc())
}

/// How far `age` has got toward `limit`: 0 when fresh, 1 at the limit and past it.
pub fn progress(age: Duration, limit: Duration) -> f64 {
    let limit = limit.num_milliseconds();
    if limit <= 0 {
        return 0.;
    }
    (age.num_milliseconds().max(0) as f64 / limit as f64).clamp(0., 1.)
}

/// The row's opacity for an `age`: 1.0 when fresh, falling linearly to `FADED` at the limit.
pub fn opacity(age: Duration, limit: Duration) -> f32 {
    (1. - (1. - FADED) * progress(age, limit)) as f32
}

/// The accent bar's share of the row's width: 0 under a day old (no bar), then proportional to
/// the age, the whole width at the limit.
pub fn fill(age: Duration, limit: Duration) -> f32 {
    if age < BAR_FROM {
        return 0.;
    }
    progress(age, limit) as f32
}

/// The opacity of the dotted texture behind a row: `TEXTURE_ALPHA.0` on a fresh row, rising with
/// the age to `TEXTURE_ALPHA.1` at the limit (and past it). No limit, no texture.
pub fn texture(age: Duration, limit: Duration) -> f32 {
    if limit <= Duration::zero() {
        return 0.;
    }
    let (low, high) = TEXTURE_ALPHA;
    low + (high - low) * progress(age, limit).powf(TEXTURE_CURVE) as f32
}

/// The alpha to paint so that, under a row drawn at `row_fade`, it shows as `alpha`. The bar and
/// the texture both sit inside the row, so the row's fade would otherwise dim them.
pub fn lifted(alpha: f32, row_fade: f32) -> f32 {
    (alpha / row_fade.max(0.1)).min(1.)
}

/// The later of the last activity and the time the owner last opened the conversation.
pub fn effective_activity(
    last_activity: Option<DateTime<Utc>>,
    opened: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    last_activity.max(opened)
}

/// Whether a conversation has settled: its age has reached the limit and it is not working. A
/// conversation with no known activity, or with settling off (no limit), never settles.
pub fn is_settled(
    activity: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    limit: Option<Duration>,
    working: bool,
) -> bool {
    match (activity, limit) {
        (Some(activity), Some(limit)) => !working && now - activity >= limit,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        parse(value).expect("a valid time")
    }

    fn week() -> Duration {
        Duration::days(7)
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_fresh_row_is_fully_opaque_and_its_bar_is_empty() {
        assert!(close(opacity(Duration::zero(), week()), 1.));
        assert!(close(fill(Duration::zero(), week()), 0.));
    }

    #[test]
    fn opacity_falls_linearly_and_stops_at_the_floor() {
        // Halfway to the limit: 1 - 0.55 / 2.
        assert!(close(
            opacity(Duration::days(3) + Duration::hours(12), week()),
            0.725
        ));
        assert!(close(opacity(week(), week()), 0.45));
        assert!(close(opacity(Duration::days(30), week()), 0.45));
        // A time in the future counts as fresh.
        assert!(close(opacity(-Duration::days(1), week()), 1.));
    }

    #[test]
    fn the_bar_is_hidden_under_a_day_and_fills_with_the_age_after() {
        assert!(close(fill(Duration::hours(23), week()), 0.));
        assert!(close(fill(Duration::days(1), week()), 1. / 7.));
        assert!(close(fill(Duration::hours(84), week()), 0.5));
        assert!(close(fill(week(), week()), 1.));
        assert!(close(fill(Duration::days(9), week()), 1.));
        assert!(close(fill(-Duration::days(1), week()), 0.));
        // No limit, nothing to fill toward.
        assert!(close(fill(Duration::days(9), Duration::zero()), 0.));
    }

    #[test]
    fn every_row_has_a_faint_texture_that_grows_with_the_age() {
        assert!(close(texture(Duration::zero(), week()), 0.05));
        assert!(texture(Duration::hours(12), week()) > 0.05);
        assert!(texture(Duration::days(1), week()) < texture(Duration::days(4), week()));
        assert!(close(texture(week(), week()), 0.24));
        assert!(close(texture(Duration::days(30), week()), 0.24));
        // Settling off: no texture.
        assert!(close(texture(Duration::days(9), Duration::zero()), 0.));
    }

    #[test]
    fn a_three_day_old_row_shows_its_texture_at_or_above_0_12() {
        // What the eye gets: the lifted alpha times the row's fade, as the canvas inside the row
        // is painted under the row's element opacity.
        let (age, limit) = (Duration::days(3), week());
        let row_fade = opacity(age, limit);
        let shown = lifted(texture(age, limit), row_fade) * row_fade;
        assert!(shown >= 0.12, "effective texture alpha {shown}");
        assert!(close(shown, 0.14647));
        // At the limit the lifted value still reaches the full texture alpha.
        let row_fade = opacity(week(), limit);
        assert!(close(lifted(TEXTURE_ALPHA.1, row_fade) * row_fade, 0.24));
    }

    #[test]
    fn the_row_fade_is_compensated_so_the_overlays_keep_their_alpha() {
        let fade = FADED as f32;
        assert!(close(lifted(TEXTURE_ALPHA.1, fade) * fade, 0.24));
        assert!(close(lifted(TRACK_ALPHA, fade) * fade, TRACK_ALPHA));
        assert!(close(lifted(BAR_ALPHA, 1.), BAR_ALPHA));
        // A colour cannot go past full opacity, so at the limit the bar shows at the row's fade.
        assert!(close(lifted(BAR_ALPHA, fade), 1.));
    }

    #[test]
    fn the_limit_is_none_when_settling_is_off_or_zero_days() {
        assert_eq!(limit_of(false, 7), None);
        assert_eq!(limit_of(true, 0), None);
        assert_eq!(limit_of(true, 7), Some(week()));
    }

    #[test]
    fn a_conversation_settles_at_the_limit_and_not_before() {
        let now = at("2026-11-09T12:00:00Z");
        let limit = Some(week());
        let just_under = now - week() + Duration::seconds(1);
        assert!(!is_settled(Some(just_under), now, limit, false));
        assert!(is_settled(Some(now - week()), now, limit, false));
        assert!(is_settled(
            Some(now - Duration::days(40)),
            now,
            limit,
            false
        ));
    }

    #[test]
    fn a_working_conversation_never_settles() {
        let now = at("2026-11-09T12:00:00Z");
        let old = Some(now - Duration::days(30));
        assert!(!is_settled(old, now, Some(week()), true));
    }

    #[test]
    fn no_activity_or_no_limit_never_settles() {
        let now = at("2026-11-09T12:00:00Z");
        assert!(!is_settled(None, now, Some(week()), false));
        assert!(!is_settled(
            Some(now - Duration::days(30)),
            now,
            None,
            false
        ));
    }

    #[test]
    fn opening_a_settled_conversation_makes_it_fresh() {
        let now = at("2026-11-09T12:00:00Z");
        let last = Some(now - Duration::days(30));
        let opened = Some(now - Duration::hours(1));
        let effective = effective_activity(last, opened);
        assert_eq!(effective, opened);
        assert!(!is_settled(effective, now, Some(week()), false));
        // A later send is later still, and an older open does not hold it back.
        assert_eq!(effective_activity(Some(now), opened), Some(now));
        assert_eq!(effective_activity(last, None), last);
    }

    #[test]
    fn the_stored_times_must_parse() {
        assert!(parse("2026-11-09T12:00:00Z").is_some());
        assert!(parse("").is_none());
        assert!(parse("ontem").is_none());
    }
}
