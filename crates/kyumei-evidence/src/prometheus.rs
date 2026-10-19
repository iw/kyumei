//! Prometheus HTTP API output and text exposition.

use std::collections::BTreeMap;

use jiff::Timestamp;
use kyumei_domain::SeriesPoint;
use serde_json::Value;

use crate::timestamps::from_epoch;

/// A parsed series before binding.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParsedSeries {
    pub metric: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub points: Vec<SeriesPoint>,
}

/// Whether a JSON document has the shape of Prometheus API output.
pub(crate) fn is_api_output(value: &Value) -> bool {
    data_object(value).is_some_and(|d| d.get("resultType").is_some() && d.get("result").is_some())
}

fn data_object(value: &Value) -> Option<&Value> {
    match value.get("data") {
        Some(data) if data.is_object() => Some(data),
        _ if value.get("resultType").is_some() => Some(value),
        _ => None,
    }
}

/// Parse `/api/v1/query` or `/api/v1/query_range` output.
pub(crate) fn parse_api_output(value: &Value) -> Result<Vec<ParsedSeries>, String> {
    if let Some(status) = value.get("status").and_then(Value::as_str)
        && status != "success"
    {
        let error = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!("Prometheus reported status `{status}`: {error}"));
    }
    let data = data_object(value).ok_or("no `data` object")?;
    let result_type = data
        .get("resultType")
        .and_then(Value::as_str)
        .ok_or("no `resultType`")?;
    let result = data.get("result").ok_or("no `result`")?;
    match result_type {
        "matrix" | "vector" => {
            let items = result.as_array().ok_or("`result` is not an array")?;
            items
                .iter()
                .map(|item| {
                    let mut labels: BTreeMap<String, String> = item
                        .get("metric")
                        .and_then(Value::as_object)
                        .map(|m| {
                            m.iter()
                                .map(|(k, v)| {
                                    (k.clone(), v.as_str().unwrap_or_default().to_owned())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let metric = labels.remove("__name__");
                    let mut points = Vec::new();
                    if let Some(values) = item.get("values").and_then(Value::as_array) {
                        for pair in values {
                            if let Some(p) = sample(pair) {
                                points.push(p);
                            }
                        }
                    }
                    if let Some(p) = item.get("value").and_then(sample) {
                        points.push(p);
                    }
                    Ok(ParsedSeries {
                        metric,
                        labels,
                        points,
                    })
                })
                .collect()
        }
        "scalar" => Ok(sample(result)
            .map(|p| {
                vec![ParsedSeries {
                    metric: None,
                    labels: BTreeMap::new(),
                    points: vec![p],
                }]
            })
            .unwrap_or_default()),
        other => Err(format!("unsupported resultType `{other}`")),
    }
}

/// `[<unix seconds>, "<value>"]`.
fn sample(pair: &Value) -> Option<SeriesPoint> {
    let arr = pair.as_array()?;
    let at = from_epoch(arr.first()?.as_f64()?)?;
    let raw = arr.get(1)?;
    let value = match raw {
        Value::String(s) => parse_prom_float(s)?,
        Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    Some(SeriesPoint { at, value })
}

fn parse_prom_float(s: &str) -> Option<f64> {
    match s {
        "NaN" => Some(f64::NAN),
        "+Inf" | "Inf" => Some(f64::INFINITY),
        "-Inf" => Some(f64::NEG_INFINITY),
        other => other.parse().ok(),
    }
}

/// Whether text looks like Prometheus exposition format.
pub(crate) fn looks_like_exposition(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return false;
    }
    if lines
        .iter()
        .any(|l| l.starts_with("# TYPE ") || l.starts_with("# HELP "))
    {
        return true;
    }
    let samples = lines
        .iter()
        .filter(|l| parse_sample_line(l).is_some())
        .count();
    samples * 10 >= lines.len() * 8
        && lines.iter().any(|l| {
            l.split_whitespace().next().is_some_and(|name| {
                name.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            })
        })
}

/// Parse exposition text. Samples without timestamps are placed at
/// `default_at`.
pub(crate) fn parse_exposition(text: &str, default_at: Timestamp) -> Vec<ParsedSeries> {
    let mut by_key: BTreeMap<(String, Vec<(String, String)>), ParsedSeries> = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, labels, value, ts)) = parse_sample_line(line) else {
            continue;
        };
        let at = ts.unwrap_or(default_at);
        let key = (name.clone(), labels.clone().into_iter().collect::<Vec<_>>());
        by_key
            .entry(key)
            .or_insert_with(|| ParsedSeries {
                metric: Some(name),
                labels,
                points: Vec::new(),
            })
            .points
            .push(SeriesPoint { at, value });
    }
    by_key.into_values().collect()
}

type SampleLine = (String, BTreeMap<String, String>, f64, Option<Timestamp>);

/// `name{a="b",c="d"} 1.5 1700000000000`.
pub(crate) fn parse_sample_line(line: &str) -> Option<SampleLine> {
    let (name_and_labels, rest) = match line.find('{') {
        Some(open) => {
            let close = find_label_end(line, open)?;
            (&line[..=close], line[close + 1..].trim())
        }
        None => {
            let mut parts = line.splitn(2, char::is_whitespace);
            let name = parts.next()?;
            (name, parts.next()?.trim())
        }
    };
    let (name, labels) = parse_name_and_labels(name_and_labels)?;
    let mut fields = rest.split_whitespace();
    let value = parse_prom_float(fields.next()?)?;
    let ts = fields
        .next()
        .and_then(|t| t.parse::<f64>().ok())
        .and_then(|ms| Timestamp::from_millisecond(ms as i64).ok());
    if fields.next().is_some() {
        return None;
    }
    Some((name, labels, value, ts))
}

fn find_label_end(line: &str, open: usize) -> Option<usize> {
    let mut in_quotes = false;
    let mut escaped = false;
    for (i, ch) in line.char_indices().skip_while(|(i, _)| *i <= open) {
        match ch {
            _ if escaped => escaped = false,
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            '}' if !in_quotes => return Some(i),
            _ => {}
        }
    }
    None
}

/// Parse `name{a="b"}` (as in exposition lines and CSV headers).
pub(crate) fn parse_name_and_labels(text: &str) -> Option<(String, BTreeMap<String, String>)> {
    let text = text.trim();
    let (name, body) = match text.find('{') {
        Some(open) if text.ends_with('}') => (&text[..open], &text[open + 1..text.len() - 1]),
        Some(_) => return None,
        None => (text, ""),
    };
    let valid_name = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':');
    if !valid_name && !name.is_empty() {
        return None;
    }
    let mut labels = BTreeMap::new();
    let mut rest = body.trim();
    while !rest.is_empty() {
        let eq = rest.find('=')?;
        let key = rest[..eq].trim().to_owned();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let mut value = String::new();
        let mut escaped = false;
        let mut end = None;
        for (i, ch) in after.char_indices().skip(1) {
            match ch {
                _ if escaped => {
                    value.push(match ch {
                        'n' => '\n',
                        other => other,
                    });
                    escaped = false;
                }
                '\\' => escaped = true,
                c if c == quote => {
                    end = Some(i);
                    break;
                }
                c => value.push(c),
            }
        }
        let end = end?;
        labels.insert(key, value);
        rest = after[end + 1..]
            .trim_start()
            .trim_start_matches(',')
            .trim_start();
    }
    let name = if name.is_empty() {
        String::new()
    } else {
        name.to_owned()
    };
    Some((name, labels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_query_range_output() {
        let json: Value = serde_json::from_str(
            r#"{"status":"success","data":{"resultType":"matrix","result":[
                {"metric":{"__name__":"temporal_worker_task_slots_available","worker_type":"WorkflowWorker"},
                 "values":[[1790775300,"180"],[1790775360,"142"]]}]}}"#,
        )
        .expect("json");
        assert!(is_api_output(&json));
        let series = parse_api_output(&json).expect("parsed");
        assert_eq!(series.len(), 1);
        assert_eq!(
            series[0].metric.as_deref(),
            Some("temporal_worker_task_slots_available")
        );
        assert_eq!(
            series[0].labels.get("worker_type").map(String::as_str),
            Some("WorkflowWorker")
        );
        assert_eq!(series[0].points.len(), 2);
        assert_eq!(series[0].points[1].value, 142.0);
    }

    #[test]
    fn reports_prometheus_errors() {
        let json: Value = serde_json::from_str(
            r#"{"status":"error","errorType":"bad_data","error":"parse error"}"#,
        )
        .expect("json");
        assert!(
            parse_api_output(&json)
                .expect_err("error")
                .contains("parse error")
        );
    }

    #[test]
    fn parses_exposition_lines() {
        let text = "# HELP x y\n# TYPE temporal_num_pollers gauge\ntemporal_num_pollers{poller_type=\"workflow_task\",task_queue=\"orders\"} 5\ntemporal_num_pollers{poller_type=\"sticky_workflow_task\"} 3 1790775300000\n";
        assert!(looks_like_exposition(text));
        let at: Timestamp = "2026-09-30T13:40:00Z".parse().expect("ts");
        let series = parse_exposition(text, at);
        assert_eq!(series.len(), 2);
        let wf = series
            .iter()
            .find(|s| s.labels.get("poller_type").map(String::as_str) == Some("workflow_task"))
            .expect("series");
        assert_eq!(wf.points[0].at, at);
        assert_eq!(wf.points[0].value, 5.0);
    }

    #[test]
    fn parses_labels_with_escapes() {
        let (name, labels) = parse_name_and_labels(r#"m{a="x\"y",b='z'}"#).expect("parsed");
        assert_eq!(name, "m");
        assert_eq!(labels.get("a").map(String::as_str), Some("x\"y"));
        assert_eq!(labels.get("b").map(String::as_str), Some("z"));
    }
}
