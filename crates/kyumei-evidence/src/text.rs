//! Free text: symptom descriptions, operator observations and logs.

use regex::Regex;
use std::sync::LazyLock;

use crate::timestamps::parse_timestamp;

/// A duration mentioned in prose, e.g. "~30 seconds".
#[derive(Clone, Debug, PartialEq)]
pub struct QuantityMention {
    pub seconds: f64,
    pub text: String,
}

static DURATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        (?P<prefix>~|≈|about|around|approximately|approx\.?|roughly|nearly|almost|over|more\s+than|up\s+to|>\s*)?
        \s*
        (?P<number>\d+(?:\.\d+)?)
        \s*
        (?P<unit>milliseconds?|millis|ms|seconds?|secs?|s|minutes?|mins?|m|hours?|hrs?|h)
        \b",
    )
    .expect("duration pattern")
});

/// Durations mentioned in `text`, in order.
///
/// Clock times ("13:35") are not durations and are skipped.
pub fn duration_mentions(text: &str) -> Vec<QuantityMention> {
    DURATION
        .captures_iter(text)
        .filter_map(|caps| {
            let whole = caps.get(0)?;
            // Skip the minutes part of a clock time such as 13:35m or 13:35.
            let before = &text[..whole.start()];
            if before.trim_end().ends_with(':') {
                return None;
            }
            let number: f64 = caps["number"].parse().ok()?;
            let unit = caps["unit"].to_ascii_lowercase();
            let factor = match unit.as_str() {
                u if u.starts_with("ms") || u.starts_with("milli") => 0.001,
                u if u.starts_with('h') => 3600.0,
                "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
                _ => 1.0,
            };
            Some(QuantityMention {
                seconds: number * factor,
                text: whole.as_str().trim().to_owned(),
            })
        })
        .collect()
}

static LOG_LEVEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(level|lvl)["']?\s*[=:]\s*["']?(debug|info|warn|warning|error|fatal)|\b(DEBUG|INFO|WARN|WARNING|ERROR|FATAL)\b"#)
        .expect("log level pattern")
});

/// Whether text looks like log output.
pub(crate) fn looks_like_log(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return false;
    }
    let levelled = lines.iter().filter(|l| LOG_LEVEL.is_match(l)).count();
    let timestamped = lines
        .iter()
        .filter(|l| leading_timestamp(l).is_some())
        .count();
    levelled * 2 >= lines.len() || (timestamped * 10 >= lines.len() * 8 && lines.len() >= 2)
}

/// The timestamp at the start of a log line, or in a JSON log's `ts`/`time`.
pub(crate) fn leading_timestamp(line: &str) -> Option<jiff::Timestamp> {
    let trimmed = line.trim();
    if trimmed.starts_with('{')
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        for key in ["ts", "time", "timestamp", "@timestamp"] {
            if let Some(t) = v.get(key) {
                let text = match t {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if let Some(ts) = parse_timestamp(&text) {
                    return Some(ts);
                }
            }
        }
        return None;
    }
    let first: String = trimmed
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    parse_timestamp(trimmed.split_whitespace().next().unwrap_or_default())
        .or_else(|| parse_timestamp(&first))
}

/// Summary of a log excerpt.
pub(crate) struct LogSummary {
    pub lines: usize,
    pub errors: usize,
    pub warnings: usize,
    pub first: Option<jiff::Timestamp>,
    pub last: Option<jiff::Timestamp>,
}

pub(crate) fn summarise_log(text: &str) -> LogSummary {
    let mut summary = LogSummary {
        lines: 0,
        errors: 0,
        warnings: 0,
        first: None,
        last: None,
    };
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        summary.lines += 1;
        if let Some(caps) = LOG_LEVEL.captures(line) {
            let level = caps
                .get(2)
                .or_else(|| caps.get(3))
                .map(|m| m.as_str().to_ascii_lowercase())
                .unwrap_or_default();
            match level.as_str() {
                "error" | "fatal" => summary.errors += 1,
                "warn" | "warning" => summary.warnings += 1,
                _ => {}
            }
        }
        if let Some(ts) = leading_timestamp(line) {
            summary.first = Some(summary.first.map_or(ts, |f| f.min(ts)));
            summary.last = Some(summary.last.map_or(ts, |l| l.max(ts)));
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_durations_not_clock_times() {
        let m = duration_mentions(
            "Since 13:35 workflow tasks wait ~30 seconds before starting; p99 was 45s at 13:41.",
        );
        let secs: Vec<f64> = m.iter().map(|q| q.seconds).collect();
        assert_eq!(secs, vec![30.0, 45.0]);
        assert_eq!(m[0].text, "~30 seconds");
    }

    #[test]
    fn converts_units() {
        assert_eq!(duration_mentions("about 1500ms")[0].seconds, 1.5);
        assert_eq!(duration_mentions("over 2 minutes")[0].seconds, 120.0);
    }

    #[test]
    fn recognises_logs() {
        let log = "2026-09-30T13:36:01Z WARN matching task write throttled\n2026-09-30T13:36:02Z ERROR something\n";
        assert!(looks_like_log(log));
        let s = summarise_log(log);
        assert_eq!((s.lines, s.errors, s.warnings), (2, 1, 1));
        assert!(s.first.is_some());
    }
}
