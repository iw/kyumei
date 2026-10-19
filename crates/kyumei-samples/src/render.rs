//! The readable lines at the top of a block. Readers ignore them; they are
//! there so a person can see in the terminal what the block holds.

use jiff::Timestamp;

use crate::{Point, Samples, Series};

/// Sparkline width in characters.
const SPARK_WIDTH: usize = 32;
/// Series listed per query before the rest are counted.
const SERIES_SHOWN: usize = 20;
const QUERY_SHOWN: usize = 90;
const LABELS_SHOWN: usize = 40;

const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub(crate) fn summary(s: &Samples) -> Vec<String> {
    let mut lines = vec![format!(
        "Collected by {} from {} at {}",
        s.collector,
        s.source,
        minute(s.collected_at_ms)
    )];
    let mut range = String::new();
    if let Some(r) = &s.request {
        range.push_str(&format!("Request {r}"));
        if let Some(i) = &s.investigation {
            range.push_str(&format!(" of {i}"));
        }
        range.push_str(", ");
    }
    range.push_str(&format!(
        "{} to {}, step {}",
        minute(s.window.start_ms),
        minute(s.window.end_ms),
        step(s.window.step_ms)
    ));
    lines.push(range);
    if let Some(d) = s.rounded_to {
        lines.push(format!("Values rounded to {d} significant digits"));
    }
    if !s.redacted.is_empty() {
        lines.push(format!(
            "Pseudonyms replace the values of {}",
            s.redacted.join(", ")
        ));
    }
    for (i, q) in s.queries.iter().enumerate() {
        lines.push(format!(
            "Query {}: {}",
            i + 1,
            shorten(&q.query, QUERY_SHOWN)
        ));
        if q.series.is_empty() {
            lines.push("  no series".to_owned());
        }
        let width = q
            .series
            .iter()
            .take(SERIES_SHOWN)
            .map(|s| label_text(s).chars().count())
            .max()
            .unwrap_or(0);
        for series in q.series.iter().take(SERIES_SHOWN) {
            lines.push(series_line(series, width));
        }
        if q.series.len() > SERIES_SHOWN {
            lines.push(format!(
                "  and {} more series",
                q.series.len() - SERIES_SHOWN
            ));
        }
    }
    lines
}

fn series_line(s: &Series, width: usize) -> String {
    let labels = label_text(s);
    let pad = width.saturating_sub(labels.chars().count());
    let finite: Vec<f64> = s
        .points
        .iter()
        .map(|p| p.value)
        .filter(|v| v.is_finite())
        .collect();
    let stats = if finite.is_empty() {
        "no finite values".to_owned()
    } else {
        let min = finite.iter().copied().fold(f64::INFINITY, f64::min);
        let max = finite.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mean = finite.iter().sum::<f64>() / finite.len() as f64;
        format!(
            "min {}  max {}  mean {}",
            compact_number(min),
            compact_number(max),
            compact_number(mean)
        )
    };
    format!(
        "  {labels}{}  {}  {stats}  ({} samples)",
        " ".repeat(pad),
        sparkline(&s.points),
        s.points.len()
    )
}

fn label_text(s: &Series) -> String {
    let mut parts = Vec::new();
    if let Some(name) = s.labels.get("__name__") {
        parts.push(name.clone());
    }
    for (k, v) in &s.labels {
        if k != "__name__" {
            parts.push(format!("{k}={v}"));
        }
    }
    if parts.is_empty() {
        return "(no labels)".to_owned();
    }
    shorten(&parts.join(", "), LABELS_SHOWN)
}

/// The series' shape in at most [`SPARK_WIDTH`] characters: each character is
/// the mean of its share of the samples, scaled from the lowest to the highest.
fn sparkline(points: &[Point]) -> String {
    let n = points.len();
    if n == 0 {
        return String::new();
    }
    let width = n.min(SPARK_WIDTH);
    let buckets: Vec<Option<f64>> = (0..width)
        .map(|b| {
            let slice = &points[b * n / width..(b + 1) * n / width];
            let finite: Vec<f64> = slice
                .iter()
                .map(|p| p.value)
                .filter(|v| v.is_finite())
                .collect();
            (!finite.is_empty()).then(|| finite.iter().sum::<f64>() / finite.len() as f64)
        })
        .collect();
    let lo = buckets
        .iter()
        .flatten()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let hi = buckets
        .iter()
        .flatten()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    buckets
        .into_iter()
        .map(|b| match b {
            None => ' ',
            Some(_) if hi <= lo => LEVELS[3],
            Some(v) => LEVELS[(((v - lo) / (hi - lo)) * 7.0).round().clamp(0.0, 7.0) as usize],
        })
        .collect()
}

/// A number in at most about five characters: `812`, `45.5`, `0.0333`, `1.2k`,
/// `3.4M`, `2.1e-5`.
pub fn compact_number(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    let a = v.abs();
    let sign = if v < 0.0 { "-" } else { "" };
    let trimmed = |s: String| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s
        }
    };
    let body = if a == 0.0 {
        "0".to_owned()
    } else if a >= 1e9 {
        format!("{:.2e}", a)
    } else if a >= 1e6 {
        format!("{}M", trimmed(format!("{:.1}", a / 1e6)))
    } else if a >= 1e4 {
        format!("{}k", trimmed(format!("{:.1}", a / 1e3)))
    } else if a >= 100.0 {
        format!("{a:.0}")
    } else if a >= 1.0 {
        trimmed(format!("{a:.2}"))
    } else if a >= 1e-3 {
        // Three significant digits.
        let decimals = (2 - a.log10().floor() as i32).max(0) as usize;
        trimmed(format!("{a:.decimals$}"))
    } else {
        format!("{a:.1e}")
    };
    format!("{sign}{body}")
}

fn minute(ms: i64) -> String {
    Timestamp::from_millisecond(ms).map_or_else(
        |_| format!("{ms} ms"),
        |t| t.strftime("%Y-%m-%d %H:%M UTC").to_string(),
    )
}

fn step(ms: u64) -> String {
    match ms {
        0 => "none".to_owned(),
        ms if ms % 3_600_000 == 0 => format!("{}h", ms / 3_600_000),
        ms if ms % 60_000 == 0 => format!("{}m", ms / 60_000),
        ms if ms % 1000 == 0 => format!("{}s", ms / 1000),
        ms => format!("{ms}ms"),
    }
}

fn shorten(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_compact() {
        assert_eq!(compact_number(812.0), "812");
        assert_eq!(compact_number(45.5), "45.5");
        assert_eq!(compact_number(0.033333), "0.0333");
        assert_eq!(compact_number(12_345.0), "12.3k");
        assert_eq!(compact_number(3_400_000.0), "3.4M");
        assert_eq!(compact_number(0.000_021), "2.1e-5");
        assert_eq!(compact_number(-4.0), "-4");
        assert_eq!(compact_number(0.0), "0");
        assert_eq!(compact_number(f64::NAN), "NaN");
    }

    #[test]
    fn sparklines_scale_from_lowest_to_highest() {
        let points: Vec<Point> = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]
            .iter()
            .enumerate()
            .map(|(i, v)| Point {
                at_ms: i as i64,
                value: *v,
            })
            .collect();
        assert_eq!(sparkline(&points), "▁▂▃▄▅▆▇█");
        let flat = vec![
            Point {
                at_ms: 0,
                value: 5.0
            };
            3
        ];
        assert_eq!(sparkline(&flat), "▄▄▄");
    }

    #[test]
    fn steps_read_naturally() {
        assert_eq!(step(15_000), "15s");
        assert_eq!(step(60_000), "1m");
        assert_eq!(step(500), "500ms");
        assert_eq!(step(3_600_000), "1h");
    }
}
