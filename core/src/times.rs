//! A time as the user reads it, in the menu, the windows and the
//! notifications: every such wording is made here, in the Mac's time zone.
//! (File names and the log keep their own machine formats.)

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

/// `14:02`.
pub(crate) fn clock(at: Timestamp, tz: &TimeZone) -> String {
    at.to_zoned(tz.clone()).strftime("%H:%M").to_string()
}

/// `Nov 3`.
pub(crate) fn date(at: Timestamp, tz: &TimeZone) -> String {
    at.to_zoned(tz.clone()).strftime("%b %-d").to_string()
}

/// `Nov 3, 14:02`.
pub(crate) fn date_and_clock(at: Timestamp, tz: &TimeZone) -> String {
    format!("{}, {}", date(at, tz), clock(at, tz))
}

/// A coming day as the menu names it: "today", a weekday within the coming
/// week, else the date ("Nov 3").
pub(crate) fn day(at: Timestamp, now: Timestamp, tz: &TimeZone) -> String {
    let date = at.to_zoned(tz.clone()).date();
    let today = now.to_zoned(tz.clone()).date();
    let days_ahead = today
        .until(date)
        .map(|span| span.get_days())
        .unwrap_or(i32::MAX);
    match days_ahead {
        0 => "today".to_owned(),
        1..=6 => date.strftime("%a").to_string(),
        _ => self::date(at, tz),
    }
}

/// The day and the time: `Thu 09:00`, `today 23:00`, `Nov 12 00:00`.
pub(crate) fn day_and_clock(at: Timestamp, now: Timestamp, tz: &TimeZone) -> String {
    format!("{} {}", day(at, now, tz), clock(at, tz))
}

/// `16:40` for a time today, else its day (`Thu`, `Nov 3`).
pub(crate) fn clock_today_else_day(at: Timestamp, now: Timestamp, tz: &TimeZone) -> String {
    let day = day(at, now, tz);
    if day == "today" { clock(at, tz) } else { day }
}

/// How long until `at`, minutes rounded up: `in 40 min`, `in 2 h`,
/// `in 1 h 40 min`.
pub(crate) fn countdown(at: Timestamp, now: Timestamp) -> String {
    let minutes = (at.duration_since(now).as_secs() + 59) / 60;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("in {minutes} min"),
        (hours, 0) => format!("in {hours} h"),
        (hours, minutes) => format!("in {hours} h {minutes} min"),
    }
}

/// How long ago `at` was: `just now`, `5 min ago`, `3 h ago`, `yesterday`,
/// `4 days ago`. A time in the future reads `just now`.
pub(crate) fn ago(at: Timestamp, now: Timestamp) -> String {
    let minutes = now.duration_since(at).max(SignedDuration::ZERO).as_mins();
    match minutes {
        0 => "just now".into(),
        1..60 => format!("{minutes} min ago"),
        60..1440 => format!("{} h ago", minutes / 60),
        1440..2880 => "yesterday".into(),
        _ => format!("{} days ago", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn paris() -> TimeZone {
        TimeZone::get("Europe/Paris").unwrap()
    }

    #[test]
    fn a_time_reads_in_the_macs_time_zone() {
        let time = at("2026-11-03T13:02:00Z");
        assert_eq!(clock(time, &paris()), "14:02");
        assert_eq!(date(time, &paris()), "Nov 3");
        assert_eq!(date_and_clock(time, &paris()), "Nov 3, 14:02");
        assert_eq!(date(at("2026-11-03T23:30:00Z"), &paris()), "Nov 4");
    }

    #[test]
    fn a_coming_day_is_today_then_a_weekday_then_a_date() {
        // A Tuesday, 10:00 in Paris.
        let now = at("2026-11-03T09:00:00Z");
        let tz = paris();
        assert_eq!(day(at("2026-11-03T22:00:00Z"), now, &tz), "today");
        assert_eq!(day(at("2026-11-03T23:00:00Z"), now, &tz), "Wed");
        assert_eq!(day(at("2026-11-09T09:00:00Z"), now, &tz), "Mon");
        assert_eq!(day(at("2026-11-10T09:00:00Z"), now, &tz), "Nov 10");
        assert_eq!(day(at("2026-11-02T09:00:00Z"), now, &tz), "Nov 2");

        assert_eq!(
            day_and_clock(at("2026-11-05T08:00:00Z"), now, &tz),
            "Thu 09:00"
        );
        assert_eq!(
            day_and_clock(at("2026-11-03T22:00:00Z"), now, &tz),
            "today 23:00"
        );
        assert_eq!(
            clock_today_else_day(at("2026-11-03T15:40:00Z"), now, &tz),
            "16:40"
        );
        assert_eq!(
            clock_today_else_day(at("2026-11-05T15:40:00Z"), now, &tz),
            "Thu"
        );
    }

    #[test]
    fn a_countdown_rounds_minutes_up() {
        let now = at("2026-11-03T09:00:00Z");
        assert_eq!(countdown(at("2026-11-03T09:39:01Z"), now), "in 40 min");
        assert_eq!(countdown(at("2026-11-03T11:00:00Z"), now), "in 2 h");
        assert_eq!(countdown(at("2026-11-03T10:39:30Z"), now), "in 1 h 40 min");
        assert_eq!(countdown(now, now), "in 0 min");
    }

    #[test]
    fn an_age_goes_from_just_now_to_days() {
        let now = at("2026-11-03T09:00:00Z");
        assert_eq!(ago(at("2026-11-03T08:59:30Z"), now), "just now");
        assert_eq!(ago(at("2026-11-03T09:05:00Z"), now), "just now");
        assert_eq!(ago(at("2026-11-03T08:55:00Z"), now), "5 min ago");
        assert_eq!(ago(at("2026-11-03T05:10:00Z"), now), "3 h ago");
        assert_eq!(ago(at("2026-11-02T08:00:00Z"), now), "yesterday");
        assert_eq!(ago(at("2026-10-30T08:00:00Z"), now), "4 days ago");
    }
}
