//! Parsing `--since 7d` and saying when things happened in words.

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use jiff::tz::TimeZone;

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const WEEK: u64 = 7 * DAY;
/// The average Gregorian month, which is the honest unit for "4 months ago".
const MONTH: u64 = 2_629_746;
const YEAR: u64 = 12 * MONTH;

/// `"7d"` -> a week of seconds. Accepts `s`, `m`, `h`, `d`, `w`, `mo`, `y`,
/// and a bare number meaning days.
pub fn parse_duration(spec: &str) -> Result<u64> {
    let spec = spec.trim().to_lowercase();
    if spec.is_empty() {
        bail!("empty duration");
    }

    let split = spec
        .find(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .unwrap_or(spec.len());
    let (number, unit) = spec.split_at(split);

    let value: f64 = number
        .parse()
        .with_context(|| format!("{spec:?} does not start with a number"))?;
    if value < 0.0 {
        bail!("{spec:?} is negative");
    }

    let scale = match unit.trim() {
        // A bare number is days: `--since 7` means the obvious thing.
        "" | "d" | "day" | "days" => DAY,
        "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "m" | "min" | "mins" | "minute" | "minutes" => MINUTE,
        "h" | "hr" | "hrs" | "hour" | "hours" => HOUR,
        "w" | "week" | "weeks" => WEEK,
        "mo" | "month" | "months" => MONTH,
        "y" | "yr" | "year" | "years" => YEAR,
        other => bail!("unknown time unit {other:?} — try 30m, 12h, 7d, 2w, 3mo"),
    };

    Ok((value * scale as f64) as u64)
}

/// `604800` -> `"7 days"`. Rounded to whatever unit reads best.
pub fn humanize(seconds: u64) -> String {
    let (value, unit) = match seconds {
        s if s < MINUTE => return format!("{s} seconds"),
        s if s < HOUR => (s / MINUTE, "minute"),
        s if s < DAY => (s / HOUR, "hour"),
        s if s < 2 * WEEK => (s / DAY, "day"),
        s if s < 2 * MONTH => (s / WEEK, "week"),
        s if s < 2 * YEAR => (s / MONTH, "month"),
        s => (s / YEAR, "year"),
    };
    if value == 1 {
        format!("1 {unit}")
    } else {
        format!("{value} {unit}s")
    }
}

/// `"3 days ago"`, or `"just now"` for anything within the minute.
pub fn ago(seconds: u64) -> String {
    if seconds < MINUTE {
        return "just now".to_string();
    }
    format!("{} ago", humanize(seconds))
}

/// When something happened, in local time.
///
/// Within the last week that means `"Monday, 14:32"` — the form people
/// actually use when reconstructing what they were doing. Older than that
/// needs a date.
pub fn moment(unix_seconds: u64, now: u64) -> String {
    let Some(zoned) = local(unix_seconds) else {
        return "unknown".to_string();
    };

    let elapsed = now.saturating_sub(unix_seconds);
    let format = if elapsed < DAY {
        "today, %H:%M"
    } else if elapsed < 2 * DAY {
        "yesterday, %H:%M"
    } else if elapsed < WEEK {
        "%A, %H:%M"
    } else if elapsed < YEAR {
        "%-d %b, %H:%M"
    } else {
        "%-d %b %Y"
    };

    zoned.strftime(format).to_string()
}

/// A calendar date with no time of day, for "last used" columns.
pub fn date(unix_seconds: u64) -> String {
    match local(unix_seconds) {
        Some(zoned) => zoned.strftime("%-d %b %Y").to_string(),
        None => "unknown".to_string(),
    }
}

fn local(unix_seconds: u64) -> Option<jiff::Zoned> {
    let timestamp = Timestamp::from_second(unix_seconds.min(i64::MAX as u64) as i64).ok()?;
    Some(timestamp.to_zoned(TimeZone::system()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_with_or_without_a_unit() {
        assert_eq!(parse_duration("7d").unwrap(), 7 * DAY);
        assert_eq!(parse_duration("7").unwrap(), 7 * DAY);
        assert_eq!(parse_duration("24h").unwrap(), DAY);
        assert_eq!(parse_duration("30m").unwrap(), 30 * MINUTE);
        assert_eq!(parse_duration("2w").unwrap(), 2 * WEEK);
        assert_eq!(parse_duration("3mo").unwrap(), 3 * MONTH);
        assert_eq!(parse_duration("1y").unwrap(), YEAR);
    }

    #[test]
    fn durations_accept_spelled_out_units_and_spacing() {
        assert_eq!(parse_duration(" 2 weeks ").unwrap(), 2 * WEEK);
        assert_eq!(parse_duration("1HOUR").unwrap(), HOUR);
        assert_eq!(parse_duration("0.5d").unwrap(), DAY / 2);
    }

    #[test]
    fn nonsense_durations_explain_themselves() {
        let error = parse_duration("banana").unwrap_err().to_string();
        assert!(error.contains("does not start with a number"), "{error}");

        let error = parse_duration("5 fortnights").unwrap_err().to_string();
        assert!(error.contains("unknown time unit"), "{error}");

        assert!(parse_duration("").is_err());
    }

    #[test]
    fn humanized_durations_pick_a_readable_unit() {
        assert_eq!(humanize(30), "30 seconds");
        assert_eq!(humanize(90), "1 minute");
        assert_eq!(humanize(3 * HOUR), "3 hours");
        assert_eq!(humanize(3 * DAY), "3 days");
        assert_eq!(humanize(3 * WEEK), "3 weeks");
        assert_eq!(humanize(4 * MONTH), "4 months");
        assert_eq!(humanize(3 * YEAR), "3 years");
    }

    #[test]
    fn recent_things_happened_just_now() {
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(2 * DAY), "2 days ago");
    }

    #[test]
    fn moments_get_more_precise_the_more_recent_they_are() {
        let now = 1_800_000_000;
        assert!(moment(now - 3600, now).starts_with("today,"));
        assert!(moment(now - 30 * HOUR, now).starts_with("yesterday,"));
        // Four days back names the weekday.
        let weekday = moment(now - 4 * DAY, now);
        assert!(weekday.contains(','), "{weekday}");
        assert!(!weekday.starts_with("today"), "{weekday}");
        // A year back needs the year.
        assert!(moment(now - 2 * YEAR, now).len() >= 8);
    }

    #[test]
    fn dates_render_without_a_time() {
        let rendered = date(1_800_000_000);
        assert!(
            rendered.contains("2027") || rendered.contains("2026"),
            "{rendered}"
        );
        assert!(!rendered.contains(':'), "{rendered}");
    }
}
