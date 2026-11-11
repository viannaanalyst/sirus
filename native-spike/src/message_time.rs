//! The time shown with a message in the transcript: "17:54" for today, and "dd/MM HH:mm" (pt) or
//! "MM/dd HH:mm" (en) for another day, in the machine's zone.

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};

/// The host's ISO timestamp (`createdAt`) as Unix milliseconds.
pub fn parse_iso(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.timestamp_millis())
}

/// The label for a moment: the time alone on the day of `now`, else the date before it.
pub fn format_at(at: NaiveDateTime, now: NaiveDateTime, english: bool) -> String {
    if at.date() == now.date() {
        at.format("%H:%M").to_string()
    } else if english {
        at.format("%m/%d %H:%M").to_string()
    } else {
        at.format("%d/%m %H:%M").to_string()
    }
}

/// The label for Unix milliseconds, in the local zone and the app's language.
pub fn label(ms: i64) -> Option<String> {
    let at = Local.timestamp_millis_opt(ms).single()?.naive_local();
    Some(format_at(
        at,
        Local::now().naive_local(),
        crate::i18n::is_en(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M").unwrap()
    }

    #[test]
    fn today_shows_only_the_time() {
        let now = at("2026-11-11T20:00");
        assert_eq!(format_at(at("2026-11-11T05:04"), now, false), "05:04");
        assert_eq!(format_at(at("2026-11-11T17:54"), now, true), "17:54");
    }

    #[test]
    fn another_day_adds_the_date_in_the_locale_order() {
        let now = at("2026-11-12T09:00");
        assert_eq!(format_at(at("2026-11-11T17:54"), now, false), "11/11 17:54");
        assert_eq!(format_at(at("2026-03-25T08:05"), now, false), "25/03 08:05");
        assert_eq!(format_at(at("2026-03-25T08:05"), now, true), "03/25 08:05");
    }

    #[test]
    fn parses_the_hosts_timestamps() {
        assert_eq!(parse_iso("1970-01-01T00:00:01.500Z"), Some(1500));
        assert_eq!(parse_iso("not a date"), None);
    }
}
