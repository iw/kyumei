//! Values written in a panel's unit, as the panel writes them: `1.20 s`, `450 ms`,
//! `2.10 K req/s`, `95.0%`, `3.00 MiB`.
//!
//! Durations move to the next unit up once they reach it, counts take the short
//! scale's suffixes, and a value without decimals set by the panel keeps three
//! significant digits, without trailing zeros.

/// How many seconds one of a time unit is, for converting evidence into it.
pub fn seconds_per(unit: &str) -> Option<f64> {
    Some(match unit {
        "ns" => 1e-9,
        "µs" | "us" => 1e-6,
        "ms" => 1e-3,
        "s" | "dtdurations" | "dtdurationms" => 1.0,
        "m" => 60.0,
        "h" => 3_600.0,
        "d" => 86_400.0,
        _ => return None,
    })
}

/// Whether the unit is a percentage written 0–100 (`percent`) or 0–1 (`percentunit`).
pub fn percent_scale(unit: &str) -> Option<f64> {
    match unit {
        "percent" => Some(100.0),
        "percentunit" => Some(1.0),
        _ => None,
    }
}

/// `value` written as a panel with `unit` and `decimals` writes it.
pub fn format(unit: Option<&str>, value: f64, decimals: Option<u32>) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    let unit = unit.unwrap_or("short");
    if let Some(scale) = seconds_per(unit) {
        return duration(value * scale, decimals);
    }
    match unit {
        "none" => number(value, decimals),
        "percent" => format!("{}%", number(value, decimals)),
        "percentunit" => format!("{}%", number(value * 100.0, decimals)),
        "reqps" => counted(value, decimals, " req/s"),
        "rps" | "ops" | "opm" | "rpm" | "wps" | "iops" => {
            let suffix = match unit {
                "rps" | "ops" => " ops/s",
                "opm" | "rpm" => " ops/min",
                "wps" => " wps",
                _ => " io/s",
            };
            counted(value, decimals, suffix)
        }
        "bytes" | "decbytes" | "bits" | "decbits" => bytes(unit, value, decimals),
        "short" => counted(value, decimals, ""),
        // Units Kyumei does not know are written as numbers with the unit after them.
        other => format!("{} {other}", number(value, decimals)),
    }
}

/// Decimals for a value when the panel sets none: enough for three significant
/// digits, none for whole numbers.
pub fn auto_decimals(value: f64) -> u32 {
    if value == 0.0 || !value.is_finite() || value.fract() == 0.0 {
        return 0;
    }
    let magnitude = value.abs().log10().floor() as i32;
    (2 - magnitude).clamp(0, 20) as u32
}

fn number(value: f64, decimals: Option<u32>) -> String {
    match decimals {
        Some(d) => format!("{value:.0$}", d as usize),
        None => {
            let d = auto_decimals(value) as usize;
            let text = format!("{value:.d$}");
            if text.contains('.') {
                text.trim_end_matches('0').trim_end_matches('.').to_owned()
            } else {
                text
            }
        }
    }
}

/// A count on the short scale: K, Mil, Bil, Tri.
fn counted(value: f64, decimals: Option<u32>, suffix: &str) -> String {
    const STEPS: [&str; 5] = ["", " K", " Mil", " Bil", " Tri"];
    let mut v = value;
    let mut step = 0;
    while v.abs() >= 1000.0 && step < STEPS.len() - 1 {
        v /= 1000.0;
        step += 1;
    }
    format!("{}{}{suffix}", number(v, decimals), STEPS[step])
}

fn bytes(unit: &str, value: f64, decimals: Option<u32>) -> String {
    let (factor, names): (f64, &[&str]) = match unit {
        "bytes" => (1024.0, &["B", "KiB", "MiB", "GiB", "TiB", "PiB"]),
        "decbytes" => (1000.0, &["B", "kB", "MB", "GB", "TB", "PB"]),
        "bits" => (1024.0, &["b", "Kib", "Mib", "Gib", "Tib", "Pib"]),
        _ => (1000.0, &["b", "kb", "Mb", "Gb", "Tb", "Pb"]),
    };
    let mut v = value;
    let mut i = 0;
    while v.abs() >= factor && i < names.len() - 1 {
        v /= factor;
        i += 1;
    }
    format!("{} {}", number(v, decimals), names[i])
}

/// A duration in seconds, in the largest unit it reaches.
fn duration(seconds: f64, decimals: Option<u32>) -> String {
    let abs = seconds.abs();
    let (value, name) = if abs == 0.0 {
        (0.0, "s")
    } else if abs < 1e-6 {
        (seconds * 1e9, "ns")
    } else if abs < 1e-3 {
        (seconds * 1e6, "µs")
    } else if abs < 1.0 {
        (seconds * 1e3, "ms")
    } else if abs < 60.0 {
        (seconds, "s")
    } else if abs < 3_600.0 {
        (seconds / 60.0, "min")
    } else if abs < 86_400.0 {
        (seconds / 3_600.0, "hour")
    } else if abs < 604_800.0 {
        (seconds / 86_400.0, "day")
    } else if abs < 31_536_000.0 {
        (seconds / 604_800.0, "week")
    } else {
        (seconds / 31_536_000.0, "year")
    };
    format!("{} {name}", number(value, decimals))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_move_up_a_unit_when_they_reach_it() {
        assert_eq!(format(Some("s"), 0.45, None), "450 ms");
        assert_eq!(format(Some("s"), 1.2, None), "1.2 s");
        assert_eq!(format(Some("s"), 99.11, None), "1.65 min");
        assert_eq!(format(Some("ms"), 1200.0, None), "1.2 s");
        assert_eq!(format(Some("ms"), 0.5, None), "500 µs");
        assert_eq!(format(Some("s"), 7200.0, None), "2 hour");
        assert_eq!(format(Some("s"), 30.0, Some(1)), "30.0 s");
        assert_eq!(seconds_per("ms"), Some(0.001));
    }

    #[test]
    fn counts_rates_percentages_and_bytes() {
        assert_eq!(format(Some("short"), 2_112.0, None), "2.11 K");
        assert_eq!(format(None, 512.0, None), "512");
        assert_eq!(format(Some("reqps"), 2_000.0, None), "2 K req/s");
        assert_eq!(format(Some("ops"), 0.25, None), "0.25 ops/s");
        assert_eq!(format(Some("percent"), 95.5, None), "95.5%");
        assert_eq!(format(Some("percentunit"), 0.987, None), "98.7%");
        assert_eq!(format(Some("bytes"), 31_457_280.0, None), "30 MiB");
        assert_eq!(format(Some("decbytes"), 1_500.0, None), "1.5 kB");
        assert_eq!(format(Some("none"), 41.237, Some(2)), "41.24");
        assert_eq!(format(Some("s"), f64::NAN, None), "NaN");
        assert_eq!(format(Some("watt"), 3.0, None), "3 watt");
    }

    #[test]
    fn decimals_keep_a_few_significant_digits() {
        assert_eq!(auto_decimals(0.00497), 5);
        assert_eq!(auto_decimals(0.0757), 4);
        assert_eq!(auto_decimals(1.234), 2);
        assert_eq!(auto_decimals(31.4), 1);
        assert_eq!(auto_decimals(812.0), 0);
        assert_eq!(auto_decimals(2112.5), 0);
    }
}
