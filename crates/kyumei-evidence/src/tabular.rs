//! CSV exports and plain `timestamp value` series.

use std::collections::BTreeMap;

use kyumei_domain::SeriesPoint;

use crate::{
    prometheus::{ParsedSeries, parse_name_and_labels},
    timestamps::parse_timestamp,
};

/// Whether text is a delimited table whose first column is time.
pub(crate) fn looks_like_csv(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 2 {
        return false;
    }
    let delimiter = detect_delimiter(lines[0]);
    let Some(delimiter) = delimiter else {
        return false;
    };
    let width = split_row(lines[0], delimiter).len();
    width >= 2
        && lines
            .iter()
            .skip(1)
            .take(20)
            .all(|l| split_row(l, delimiter).len() == width)
        && lines.iter().skip(1).take(5).all(|l| {
            split_row(l, delimiter)
                .first()
                .and_then(|c| parse_timestamp(c))
                .is_some()
        })
}

fn detect_delimiter(header: &str) -> Option<u8> {
    if header.contains('\t') {
        Some(b'\t')
    } else if header.contains(',') {
        Some(b',')
    } else if header.contains(';') {
        Some(b';')
    } else {
        None
    }
}

fn split_row(line: &str, delimiter: u8) -> Vec<String> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(delimiter)
        .flexible(true)
        .from_reader(line.as_bytes());
    reader
        .records()
        .next()
        .and_then(Result::ok)
        .map(|r| r.iter().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Parse a CSV export: "wide" (Time, series…) or "long" (Time, Series, Value).
pub(crate) fn parse_csv(text: &str) -> Result<Vec<ParsedSeries>, String> {
    let header_line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or("empty CSV")?;
    let delimiter = detect_delimiter(header_line).ok_or("no delimiter")?;
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| e.to_string())?
        .iter()
        .map(str::to_owned)
        .collect();
    let lower: Vec<String> = headers.iter().map(|h| h.to_ascii_lowercase()).collect();

    // Long format: a series-name column and a value column.
    let series_col = lower
        .iter()
        .position(|h| h == "series" || h == "metric" || h == "name");
    let value_col = lower.iter().position(|h| h == "value");
    let mut out: BTreeMap<String, ParsedSeries> = BTreeMap::new();
    for record in reader.records() {
        let record = record.map_err(|e| e.to_string())?;
        let Some(at) = record.get(0).and_then(parse_timestamp) else {
            continue;
        };
        match (series_col, value_col) {
            (Some(s), Some(v)) => {
                let name = record.get(s).unwrap_or_default().to_owned();
                let Some(value) = record.get(v).and_then(|x| x.parse::<f64>().ok()) else {
                    continue;
                };
                out.entry(name.clone())
                    .or_insert_with(|| series_from_header(&name))
                    .points
                    .push(SeriesPoint { at, value });
            }
            _ => {
                for (i, header) in headers.iter().enumerate().skip(1) {
                    let Some(value) = record.get(i).and_then(|x| x.parse::<f64>().ok()) else {
                        continue;
                    };
                    out.entry(header.clone())
                        .or_insert_with(|| series_from_header(header))
                        .points
                        .push(SeriesPoint { at, value });
                }
            }
        }
    }
    if out.is_empty() {
        return Err("no numeric samples found".to_owned());
    }
    Ok(out.into_values().collect())
}

/// A CSV column header may be `name{labels}`, `{labels}` or free text.
fn series_from_header(header: &str) -> ParsedSeries {
    match parse_name_and_labels(header) {
        Some((name, labels)) if !name.is_empty() || !labels.is_empty() => ParsedSeries {
            metric: (!name.is_empty()).then_some(name),
            labels,
            points: Vec::new(),
        },
        _ => {
            let mut labels = BTreeMap::new();
            if !header.trim().is_empty() && !header.eq_ignore_ascii_case("value") {
                labels.insert("series".to_owned(), header.trim().to_owned());
            }
            ParsedSeries {
                metric: None,
                labels,
                points: Vec::new(),
            }
        }
    }
}

/// Whether text is `timestamp value` lines.
pub(crate) fn looks_like_series_text(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    !lines.is_empty()
        && lines
            .iter()
            .filter(|l| parse_series_line(l).is_some())
            .count()
            * 10
            >= lines.len() * 8
}

fn parse_series_line(line: &str) -> Option<SeriesPoint> {
    let parts: Vec<&str> = line
        .split([',', '\t', ' '])
        .filter(|p| !p.is_empty())
        .collect();
    let (ts_text, value_text) = match parts.as_slice() {
        [ts, value] => ((*ts).to_owned(), *value),
        // `2026-09-30 13:35:00 180`
        [date, time, value] => (format!("{date} {time}"), *value),
        _ => return None,
    };
    Some(SeriesPoint {
        at: parse_timestamp(&ts_text)?,
        value: value_text.parse().ok()?,
    })
}

pub(crate) fn parse_series_text(text: &str) -> Vec<ParsedSeries> {
    let points: Vec<SeriesPoint> = text
        .lines()
        .map(str::trim)
        .filter_map(parse_series_line)
        .collect();
    if points.is_empty() {
        return Vec::new();
    }
    vec![ParsedSeries {
        metric: None,
        labels: BTreeMap::new(),
        points,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wide_grafana_csv() {
        let text = "\"Time\",\"temporal_num_pollers{poller_type=\"\"workflow_task\"\"}\"\n2026-09-30 13:35:00,60\n2026-09-30 13:36:00,58\n";
        assert!(looks_like_csv(text));
        let series = parse_csv(text).expect("csv");
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].metric.as_deref(), Some("temporal_num_pollers"));
        assert_eq!(
            series[0].labels.get("poller_type").map(String::as_str),
            Some("workflow_task")
        );
        assert_eq!(series[0].points.len(), 2);
    }

    #[test]
    fn parses_long_csv() {
        let text = "Time,Series,Value\n2026-09-30T13:35:00Z,a,1\n2026-09-30T13:35:00Z,b,2\n2026-09-30T13:36:00Z,a,3\n";
        let series = parse_csv(text).expect("csv");
        assert_eq!(series.len(), 2);
    }

    #[test]
    fn parses_series_text() {
        let text = "2026-09-30T13:35:00Z 180\n2026-09-30T13:36:00Z 142\n";
        assert!(looks_like_series_text(text));
        let s = parse_series_text(text);
        assert_eq!(s[0].points.len(), 2);
        assert!(looks_like_series_text("1790775300 12.5\n1790775360 13"));
    }
}
