//! Prometheus's text exposition format, as tempdes writes it.

use std::collections::BTreeMap;

/// One sample of one series.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub metric: String,
    pub labels: BTreeMap<String, String>,
    pub value: f64,
}

#[derive(Debug, PartialEq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct ScrapeError {
    pub line: usize,
    pub message: String,
}

/// Read a scrape: comment lines are skipped, and every other line is
/// `name{label="value",…} value`, with an optional timestamp after the value.
pub fn parse_exposition(text: &str) -> Result<Vec<Sample>, ScrapeError> {
    let mut samples = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let sample = parse_line(line).map_err(|message| ScrapeError {
            line: i + 1,
            message,
        })?;
        samples.push(sample);
    }
    Ok(samples)
}

fn parse_line(line: &str) -> Result<Sample, String> {
    let name_end = line
        .find(|c: char| c == '{' || c.is_whitespace())
        .ok_or("a sample needs a value")?;
    let metric = line[..name_end].to_owned();
    if metric.is_empty() {
        return Err("a sample needs a metric name".to_owned());
    }
    let mut rest = &line[name_end..];
    let mut labels = BTreeMap::new();
    if let Some(after) = rest.strip_prefix('{') {
        let mut s = after;
        loop {
            s = s.trim_start();
            if let Some(r) = s.strip_prefix('}') {
                rest = r;
                break;
            }
            let eq = s.find('=').ok_or("a label without a value")?;
            let name = s[..eq].trim().to_owned();
            let (value, r) = quoted(s[eq + 1..].trim_start())?;
            labels.insert(name, value);
            s = r.trim_start();
            s = s.strip_prefix(',').unwrap_or(s);
        }
    }
    let value = rest
        .split_whitespace()
        .next()
        .ok_or("a sample needs a value")?;
    let value = number(value).ok_or_else(|| format!("`{value}` is not a number"))?;
    Ok(Sample {
        metric,
        labels,
        value,
    })
}

/// A double-quoted label value, unescaped, and what follows it.
fn quoted(s: &str) -> Result<(String, &str), String> {
    let Some(body) = s.strip_prefix('"') else {
        return Err("a label value must be quoted".to_owned());
    };
    let mut value = String::new();
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        if escaped {
            value.push(if c == 'n' { '\n' } else { c });
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Ok((value, &body[i + 1..]));
        } else {
            value.push(c);
        }
    }
    Err("a label value is not closed".to_owned())
}

/// A sample value or a bucket bound: a float, or `+Inf`, `-Inf` or `NaN`.
pub(crate) fn number(s: &str) -> Option<f64> {
    match s {
        "+Inf" | "Inf" => Some(f64::INFINITY),
        "-Inf" => Some(f64::NEG_INFINITY),
        "NaN" => Some(f64::NAN),
        _ => s.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_read_with_their_labels() {
        let text = "# TYPE service_requests counter\n\
                    service_requests{service_name=\"frontend\",operation=\"StartWorkflowExecution\"} 1200\n\
                    service_latency_bucket{operation=\"a\\\"b\",le=\"+Inf\"} 7 1700000000000\n\
                    numshards_gauge 64\n";
        let samples = parse_exposition(text).expect("parse");
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].metric, "service_requests");
        assert_eq!(samples[0].labels["operation"], "StartWorkflowExecution");
        assert_eq!(samples[0].value, 1200.0);
        assert_eq!(samples[1].labels["operation"], "a\"b");
        assert_eq!(samples[1].labels["le"], "+Inf");
        assert_eq!(samples[2].metric, "numshards_gauge");
        assert!(samples[2].labels.is_empty());
    }

    #[test]
    fn a_broken_line_is_reported_with_its_number() {
        let e = parse_exposition("ok 1\nbroken{x=\"y\"} lots\n").expect_err("broken");
        assert_eq!(e.line, 2);
        assert!(e.message.contains("lots"), "{e}");
    }
}
