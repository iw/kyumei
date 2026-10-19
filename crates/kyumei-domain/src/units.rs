//! Units and summary statistics.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Units Kyumei understands well enough to convert.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Seconds,
    Milliseconds,
    /// A dimensionless count (slots, pollers, tasks, partitions).
    Count,
    /// Events per second, e.g. a PromQL `rate(...)` result.
    PerSecond,
    /// A ratio in `[0, 1]`.
    Ratio,
    Bytes,
    /// A boolean flag expressed as 0/1.
    Flag,
}

impl Unit {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Seconds => "s",
            Self::Milliseconds => "ms",
            Self::Count => "",
            Self::PerSecond => "/s",
            Self::Ratio => "",
            Self::Bytes => "B",
            Self::Flag => "",
        }
    }

    pub fn is_duration(self) -> bool {
        matches!(self, Self::Seconds | Self::Milliseconds)
    }

    /// Convert `value` from `self` to `target`, when the units are compatible.
    pub fn convert(self, value: f64, target: Unit) -> Option<f64> {
        match (self, target) {
            (a, b) if a == b => Some(value),
            (Self::Milliseconds, Self::Seconds) => Some(value / 1000.0),
            (Self::Seconds, Self::Milliseconds) => Some(value * 1000.0),
            _ => None,
        }
    }

    /// Render a value in this unit for humans (`31.2 s`, `4,812/s`, `180`).
    pub fn format_value(self, value: f64) -> String {
        let number = format_number(value);
        match self {
            // Under a tenth of a second reads better in milliseconds.
            Self::Seconds if value != 0.0 && value.abs() < 0.1 => {
                format!("{} ms", format_number(value * 1000.0))
            }
            Self::Seconds => format!("{number} s"),
            Self::Milliseconds => format!("{number} ms"),
            Self::PerSecond => format!("{number}/s"),
            // Binary multiples, as Temporal's size limits are written.
            Self::Bytes if value.abs() >= 1024.0 * 1024.0 => {
                format!("{} MiB", format_number(value / (1024.0 * 1024.0)))
            }
            Self::Bytes if value.abs() >= 1024.0 => {
                format!("{} KiB", format_number(value / 1024.0))
            }
            Self::Bytes => format!("{number} B"),
            Self::Ratio => format!("{:.0}%", value * 100.0),
            Self::Flag if value == 1.0 => "yes".to_owned(),
            Self::Flag if value == 0.0 => "no".to_owned(),
            Self::Count | Self::Flag => number,
        }
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Seconds => "seconds",
            Self::Milliseconds => "milliseconds",
            Self::Count => "count",
            Self::PerSecond => "per second",
            Self::Ratio => "ratio",
            Self::Bytes => "bytes",
            Self::Flag => "flag",
        })
    }
}

impl FromStr for Unit {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "s" | "sec" | "secs" | "second" | "seconds" => Ok(Self::Seconds),
            "ms" | "millisecond" | "milliseconds" | "millis" => Ok(Self::Milliseconds),
            "count" | "" | "n" | "number" => Ok(Self::Count),
            "/s" | "per_second" | "per second" | "persecond" | "rps" | "qps" => Ok(Self::PerSecond),
            "ratio" | "fraction" => Ok(Self::Ratio),
            "bytes" | "b" => Ok(Self::Bytes),
            "flag" | "bool" | "boolean" => Ok(Self::Flag),
            other => Err(format!("unknown unit `{other}`")),
        }
    }
}

/// A summary statistic over a series, or the statistic a scalar represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Statistic {
    Min,
    Max,
    Mean,
    First,
    Last,
    /// Last minus first.
    Delta,
    /// Number of samples.
    Samples,
    /// A representative value, e.g. an operator's "about 30 seconds".
    Typical,
    /// The mean of samples before the incident window: what normal looked like.
    Baseline,
}

impl Statistic {
    pub const ALL: [Statistic; 9] = [
        Self::Min,
        Self::Max,
        Self::Mean,
        Self::First,
        Self::Last,
        Self::Delta,
        Self::Samples,
        Self::Typical,
        Self::Baseline,
    ];

    /// The unit this statistic of a signal measured in `unit` is expressed
    /// in. A sample count is a count, whatever the signal measures.
    pub fn unit_of(self, unit: Unit) -> Unit {
        match self {
            Self::Samples => Unit::Count,
            _ => unit,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Min => "minimum",
            Self::Max => "maximum",
            Self::Mean => "mean",
            Self::First => "first",
            Self::Last => "last",
            Self::Delta => "change",
            Self::Samples => "samples",
            Self::Typical => "typical",
            Self::Baseline => "baseline",
        }
    }
}

impl fmt::Display for Statistic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for Statistic {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "min" | "minimum" => Ok(Self::Min),
            "max" | "maximum" | "peak" => Ok(Self::Max),
            "mean" | "avg" | "average" => Ok(Self::Mean),
            "first" => Ok(Self::First),
            "last" | "latest" | "current" => Ok(Self::Last),
            "delta" | "change" => Ok(Self::Delta),
            "samples" | "count" => Ok(Self::Samples),
            "typical" | "approx" | "approximately" | "about" | "value" => Ok(Self::Typical),
            "baseline" | "before" | "normal" => Ok(Self::Baseline),
            other => Err(format!("unknown statistic `{other}`")),
        }
    }
}

/// Human formatting: thousands separators, at most one decimal for values
/// under 100, none above.
pub fn format_number(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    let abs = value.abs();
    let rendered = if abs >= 100.0 || value.fract() == 0.0 {
        format!("{:.0}", value)
    } else if abs >= 10.0 {
        format!("{:.1}", value)
    } else {
        format!("{:.2}", value)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    };
    group_thousands(&rendered)
}

fn group_thousands(rendered: &str) -> String {
    let (sign, rest) = rendered
        .strip_prefix('-')
        .map_or(("", rendered), |r| ("-", r));
    let (int, frac) = rest
        .split_once('.')
        .map_or((rest, None), |(i, f)| (i, Some(f)));
    if int.len() <= 3 {
        return rendered.to_owned();
    }
    let mut grouped = String::with_capacity(int.len() + int.len() / 3);
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match frac {
        Some(frac) => format!("{sign}{grouped}.{frac}"),
        None => format!("{sign}{grouped}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_for_operators() {
        assert_eq!(format_number(4812.0), "4,812");
        assert_eq!(format_number(31.24), "31.2");
        assert_eq!(format_number(0.5), "0.5");
        assert_eq!(format_number(180.0), "180");
        assert_eq!(format_number(1_250_000.0), "1,250,000");
        assert_eq!(Unit::PerSecond.format_value(4812.0), "4,812/s");
        assert_eq!(Unit::Bytes.format_value(52_609_024.0), "50.2 MiB");
        assert_eq!(Unit::Bytes.format_value(5_734.0), "5.6 KiB");
        assert_eq!(Unit::Bytes.format_value(512.0), "512 B");
        assert_eq!(Unit::Seconds.format_value(0.0021), "2.1 ms");
        assert_eq!(Unit::Seconds.format_value(0.59), "0.59 s");
        assert_eq!(Unit::Seconds.format_value(0.0), "0 s");
        assert_eq!(Unit::Flag.format_value(1.0), "yes");
        assert_eq!(Unit::Flag.format_value(0.0), "no");
    }

    #[test]
    fn converts_durations() {
        assert_eq!(
            Unit::Milliseconds.convert(31_000.0, Unit::Seconds),
            Some(31.0)
        );
        assert_eq!(Unit::Count.convert(3.0, Unit::Seconds), None);
    }
}
