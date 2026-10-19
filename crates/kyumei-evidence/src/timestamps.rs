//! Flexible timestamp and duration parsing for operator-supplied data.

use jiff::{Timestamp, civil::DateTime, tz::TimeZone};

/// Parse the timestamp shapes that appear in exports: RFC 3339, Unix seconds
/// (with fraction), Unix milliseconds, and `YYYY-MM-DD HH:MM[:SS]` (taken as
/// UTC).
pub fn parse_timestamp(text: &str) -> Option<Timestamp> {
    let t = text.trim().trim_matches('"');
    if t.is_empty() {
        return None;
    }
    if let Ok(ts) = t.parse::<Timestamp>() {
        return Some(ts);
    }
    if let Ok(number) = t.parse::<f64>() {
        return from_epoch(number);
    }
    let normalised = t.replace('T', " ");
    for format in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(dt) = DateTime::strptime(format, &normalised) {
            return dt.to_zoned(TimeZone::UTC).ok().map(|z| z.timestamp());
        }
    }
    None
}

/// Unix seconds or milliseconds; values beyond year ~2286 in seconds are
/// taken as milliseconds.
pub fn from_epoch(number: f64) -> Option<Timestamp> {
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    if number > 10_000_000_000.0 {
        Timestamp::from_millisecond(number as i64).ok()
    } else {
        let secs = number.trunc() as i64;
        let nanos = ((number.fract()) * 1e9).round() as i32;
        Timestamp::new(secs, nanos).ok()
    }
}

/// Parse a duration such as `31.2s`, `450ms`, `2m`, `1h30m` or a protobuf
/// JSON duration (`"31.200s"`). The Temporal CLI writes Go durations with a
/// space after every letter and whole days as `d`, so `4m 39.24s`, `450m s`
/// and `3d 0h 5m 2s` are read too. Returns seconds.
pub fn parse_duration_seconds(text: &str) -> Option<f64> {
    let t = text.trim().trim_matches('"');
    if t.is_empty() {
        return None;
    }
    if let Ok(number) = t.parse::<f64>() {
        return Some(number);
    }
    let mut total = 0.0;
    let mut rest = t;
    let mut matched = false;
    while !rest.is_empty() {
        let digits_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if digits_end == 0 {
            return None;
        }
        let value: f64 = rest[..digits_end].parse().ok()?;
        rest = &rest[digits_end..];
        let unit_end = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let unit: String = rest[..unit_end].split_whitespace().collect();
        rest = &rest[unit_end..];
        let factor = match unit.as_str() {
            "ns" => 1e-9,
            "us" | "µs" => 1e-6,
            "ms" => 1e-3,
            "s" | "" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            "d" => 86_400.0,
            _ => return None,
        };
        total += value * factor;
        matched = true;
    }
    matched.then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_timestamps() {
        let expected: Timestamp = "2026-09-30T13:35:00Z".parse().expect("ts");
        assert_eq!(parse_timestamp("2026-09-30T13:35:00Z"), Some(expected));
        assert_eq!(parse_timestamp("1790775300"), Some(expected));
        assert_eq!(parse_timestamp("1790775300000"), Some(expected));
        assert_eq!(parse_timestamp("2026-09-30 13:35:00"), Some(expected));
        assert_eq!(parse_timestamp("2026-09-30 13:35"), Some(expected));
        assert_eq!(
            parse_timestamp("\"2026-09-30T13:35:00.000Z\""),
            Some(expected)
        );
        assert_eq!(parse_timestamp("yesterday"), None);
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration_seconds("31.2s"), Some(31.2));
        assert_eq!(parse_duration_seconds("450ms"), Some(0.45));
        assert_eq!(parse_duration_seconds("1m30s"), Some(90.0));
        assert_eq!(parse_duration_seconds("\"12.5s\""), Some(12.5));
        assert_eq!(parse_duration_seconds("fast"), None);
    }

    #[test]
    fn parses_durations_as_the_temporal_cli_writes_them() {
        // `formatDuration` in the CLI spaces Go's `Duration.String()` after
        // every letter and writes 24 hours or more as days.
        let secs = |text: &str| parse_duration_seconds(text).expect(text);
        let close = |a: f64, b: f64| (a - b).abs() <= b.abs() * 1e-12;
        assert_eq!(secs("0s"), 0.0);
        assert!(close(secs("19.339312s"), 19.339312));
        assert!(close(secs("4m 39.25s"), 279.25));
        assert!(close(secs("450m s"), 0.45));
        assert!(close(secs("2m s"), 0.002));
        assert!(close(secs("500n s"), 5e-7));
        assert!(close(secs("1.5µs"), 1.5e-6));
        assert!(close(secs("3h 57m 5.297s"), 14_225.297));
        assert!(close(secs("3d 0h 5m 2s"), 259_502.0));
        assert_eq!(parse_duration_seconds("4 parsecs"), None);
    }
}
