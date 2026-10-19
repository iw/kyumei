//! Writing and reading sample blocks.
//!
//! ```text
//! -----BEGIN KYUMEI SAMPLES v1-----
//! Collected by kyumei-collect 0.1.0 from prometheus at 2026-10-14 09:12 UTC
//! Request R2 of inv-280a0dbd5c, 2026-10-13 11:32 UTC to 2026-10-13 12:03 UTC, step 15s
//! Query 1: sum by (task_queue) (temporal_worker_task_slots_available{…})
//!   task_queue=orders  ▇▇▇▇▆▅▃▁▁▁▂▅▇▇  min 0  max 812  mean 455  (125 samples)
//! :8VZNb9MwEP0rkc9…
//! :…
//! -----END KYUMEI SAMPLES (512 chars, crc32 4a17c09e)-----
//! ```
//!
//! Lines before the first `:` line are for people and are ignored. The `:`
//! lines are the payload: raw deflate of the binary samples, base64url without
//! padding, 76 characters a line. The last line states the payload's length
//! and the CRC-32 of the compressed bytes, so a block that lost a line, gained
//! one, or had a character changed is rejected with the reason.
//!
//! A terminal that wraps long lines when copying is tolerated: once the payload
//! has begun, a line of payload characters without the `:` continues it.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

use crate::{
    FORMAT_VERSION, Point, QueryResult, Samples, Series, Window,
    codec::{read_times, read_values, write_times, write_values},
    render,
    wire::{Reader, WireError, Writer},
};

const BEGIN: &str = "-----BEGIN KYUMEI SAMPLES";
const END: &str = "-----END KYUMEI SAMPLES";
const LINE: usize = 76;
const MAGIC: &[u8; 3] = b"KYS";
/// The most a payload may inflate to, so a hostile block cannot exhaust memory.
const MAX_INFLATED: usize = 64 << 20;

/// Why a block was rejected. The messages are written for the person who
/// pasted it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BlockError {
    #[error(
        "No Kyumei sample block was found. A block starts with a line `-----BEGIN KYUMEI SAMPLES v1-----`."
    )]
    NotFound,
    #[error(
        "The block's last line is missing. Copy everything from its -----BEGIN line to its -----END line."
    )]
    MissingEnd,
    #[error(
        "The block is incomplete: {missing} of its {expected} characters of samples are missing, about {lines} lines. Copy the whole block again."
    )]
    Incomplete {
        expected: usize,
        missing: usize,
        lines: usize,
    },
    #[error(
        "The block has {extra} characters more than its last line states, so something was added in copying. Copy the block again."
    )]
    TooLong { expected: usize, extra: usize },
    #[error(
        "The block's checksum does not match, so characters were changed in copying. Copy the block again."
    )]
    Checksum,
    #[error(
        "The block was written in format version {0}, which is newer than this version of Kyumei reads. Update Kyumei."
    )]
    UnsupportedVersion(u8),
    #[error("The block cannot be read: {0}.")]
    Corrupt(String),
}

impl From<WireError> for BlockError {
    fn from(e: WireError) -> Self {
        Self::Corrupt(e.0.to_owned())
    }
}

/// Write samples as a block of text, ending with a newline.
pub fn encode(s: &Samples) -> String {
    let compressed = miniz_oxide::deflate::compress_to_vec(&payload(s), 10);
    let crc = crc32fast::hash(&compressed);
    let text = URL_SAFE_NO_PAD.encode(&compressed);
    let mut out = format!("{BEGIN} v{FORMAT_VERSION}-----\n");
    for line in render::summary(s) {
        out.push_str(&line);
        out.push('\n');
    }
    for chunk in text.as_bytes().chunks(LINE) {
        out.push(':');
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push('\n');
    }
    out.push_str(&format!(
        "{END} ({} chars, crc32 {crc:08x})-----\n",
        text.len()
    ));
    out
}

/// Read the first block in `text`, which may hold other text around it.
pub fn decode(text: &str) -> Result<Samples, BlockError> {
    decode_all(text)
        .into_iter()
        .next()
        .unwrap_or(Err(BlockError::NotFound))
}

/// Read every block in `text`, in order. Text outside blocks is ignored.
pub fn decode_all(text: &str) -> Vec<Result<Samples, BlockError>> {
    let mut results = Vec::new();
    let mut lines = text.lines().map(str::trim).peekable();
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix(BEGIN) else {
            continue;
        };
        let version = rest
            .trim()
            .trim_end_matches('-')
            .trim()
            .strip_prefix('v')
            .and_then(|v| v.parse::<u8>().ok());
        let mut payload = String::new();
        let mut started = false;
        let mut end = None;
        while let Some(&next) = lines.peek() {
            if next.starts_with(BEGIN) {
                break;
            }
            lines.next();
            if let Some(rest) = next.strip_prefix(END) {
                end = Some(rest.to_owned());
                break;
            }
            let (candidate, marked) = match next.strip_prefix(':') {
                Some(c) => (c, true),
                None => (next, false),
            };
            let is_payload = !candidate.is_empty()
                && candidate
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
            if is_payload && (marked || started) {
                started = true;
                payload.push_str(candidate);
            }
        }
        results.push(match (version, end) {
            (Some(v), _) if v > FORMAT_VERSION => Err(BlockError::UnsupportedVersion(v)),
            (_, None) => Err(BlockError::MissingEnd),
            (_, Some(end)) => read_block(&payload, &end),
        });
    }
    results
}

fn read_block(payload: &str, end: &str) -> Result<Samples, BlockError> {
    let (expected, crc) = parse_end(end)?;
    if payload.len() < expected {
        let missing = expected - payload.len();
        return Err(BlockError::Incomplete {
            expected,
            missing,
            lines: missing.div_ceil(LINE),
        });
    }
    if payload.len() > expected {
        return Err(BlockError::TooLong {
            expected,
            extra: payload.len() - expected,
        });
    }
    let compressed = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| BlockError::Checksum)?;
    if crc32fast::hash(&compressed) != crc {
        return Err(BlockError::Checksum);
    }
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(&compressed, MAX_INFLATED)
        .map_err(|_| BlockError::Corrupt("the samples do not decompress".to_owned()))?;
    read_payload(&raw)
}

/// `(512 chars, crc32 4a17c09e)-----`
fn parse_end(end: &str) -> Result<(usize, u32), BlockError> {
    let changed = || BlockError::Corrupt("its last line was changed".to_owned());
    let inner = end
        .trim()
        .trim_end_matches('-')
        .trim()
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(changed)?;
    let (chars, crc) = inner.split_once(',').ok_or_else(changed)?;
    let chars = chars
        .trim()
        .strip_suffix("chars")
        .and_then(|n| n.trim().parse::<usize>().ok())
        .ok_or_else(changed)?;
    let crc = crc
        .trim()
        .strip_prefix("crc32")
        .and_then(|c| u32::from_str_radix(c.trim(), 16).ok())
        .ok_or_else(changed)?;
    Ok((chars, crc))
}

fn optional(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or_default()
}

fn payload(s: &Samples) -> Vec<u8> {
    let mut w = Writer::default();
    w.bytes(MAGIC);
    w.u8(FORMAT_VERSION);
    w.u8(s.rounded_to.unwrap_or(0));
    w.str(&s.collector);
    w.str(&s.source);
    w.ivarint(s.collected_at_ms);
    w.str(optional(&s.investigation));
    w.str(optional(&s.request));
    w.ivarint(s.window.start_ms);
    w.ivarint(s.window.end_ms);
    w.uvarint(s.window.step_ms);
    w.uvarint(s.redacted.len() as u64);
    for label in &s.redacted {
        w.str(label);
    }
    w.uvarint(s.queries.len() as u64);
    for q in &s.queries {
        w.str(&q.query);
        w.uvarint(q.series.len() as u64);
        for series in &q.series {
            w.uvarint(series.labels.len() as u64);
            for (k, v) in &series.labels {
                w.str(k);
                w.str(v);
            }
            w.uvarint(series.points.len() as u64);
            if !series.points.is_empty() {
                let times: Vec<i64> = series.points.iter().map(|p| p.at_ms).collect();
                let values: Vec<f64> = series.points.iter().map(|p| p.value).collect();
                write_times(&mut w, &times);
                write_values(&mut w, &values);
            }
        }
    }
    w.buf
}

fn read_payload(raw: &[u8]) -> Result<Samples, BlockError> {
    let mut r = Reader::new(raw);
    if r.bytes(3)? != MAGIC {
        return Err(BlockError::Corrupt(
            "it is not a Kyumei sample block".to_owned(),
        ));
    }
    let version = r.u8()?;
    if version != FORMAT_VERSION {
        return Err(BlockError::UnsupportedVersion(version));
    }
    let rounded_to = match r.u8()? {
        0 => None,
        d => Some(d),
    };
    let collector = r.str()?;
    let source = r.str()?;
    let collected_at_ms = r.ivarint()?;
    let some = |s: String| (!s.is_empty()).then_some(s);
    let investigation = some(r.str()?);
    let request = some(r.str()?);
    let window = Window {
        start_ms: r.ivarint()?,
        end_ms: r.ivarint()?,
        step_ms: r.uvarint()?,
    };
    let redacted = (0..r.count()?)
        .map(|_| r.str())
        .collect::<Result<Vec<_>, _>>()?;
    let mut queries = Vec::new();
    for _ in 0..r.count()? {
        let query = r.str()?;
        let mut series = Vec::new();
        for _ in 0..r.count()? {
            let mut labels = std::collections::BTreeMap::new();
            for _ in 0..r.count()? {
                let k = r.str()?;
                let v = r.str()?;
                labels.insert(k, v);
            }
            let n = r.count()?;
            let points = if n == 0 {
                Vec::new()
            } else {
                let times = read_times(&mut r, n)?;
                let values = read_values(&mut r, n)?;
                times
                    .into_iter()
                    .zip(values)
                    .map(|(at_ms, value)| Point { at_ms, value })
                    .collect()
            };
            series.push(Series { labels, points });
        }
        queries.push(QueryResult { query, series });
    }
    if r.remaining() != 0 {
        return Err(BlockError::Corrupt(
            "there is more data after the samples".to_owned(),
        ));
    }
    Ok(Samples {
        collector,
        source,
        collected_at_ms,
        investigation,
        request,
        window,
        rounded_to,
        redacted,
        queries,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn series(labels: &[(&str, &str)], points: &[(i64, f64)]) -> Series {
        Series {
            labels: labels
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>(),
            points: points
                .iter()
                .map(|&(at_ms, value)| Point { at_ms, value })
                .collect(),
        }
    }

    fn sample() -> Samples {
        let start = 1_791_105_120_000; // 2026-10-13 11:32 UTC
        let even: Vec<(i64, f64)> = (0..125)
            .map(|i| (start + i * 15_000, 812.0 - (i % 40) as f64 * 20.0))
            .collect();
        let gappy: Vec<(i64, f64)> = (0..125)
            .filter(|i| !(40..55).contains(i))
            .map(|i| (start + i * 15_000, 0.033_333_333_333 * i as f64))
            .collect();
        let odd = vec![
            (start, f64::NAN),
            (start + 15_000, f64::INFINITY),
            (start + 30_000, f64::NEG_INFINITY),
            (start + 45_000, -0.0),
            (start + 60_123, 5e-324),
        ];
        let counter = vec![
            (start, 100.0),
            (start + 15_000, 250.0),
            (start + 30_000, 3.0),
            (start + 45_000, 90.0),
        ];
        Samples {
            collector: "kyumei-collect 0.1.0".to_owned(),
            source: "prometheus".to_owned(),
            collected_at_ms: start + 3_600_000,
            investigation: Some("inv-280a0dbd5c".to_owned()),
            request: Some("R2".to_owned()),
            window: Window {
                start_ms: start,
                end_ms: start + 1_860_000,
                step_ms: 15_000,
            },
            rounded_to: None,
            redacted: Vec::new(),
            queries: vec![
                QueryResult {
                    query: "sum by (task_queue) (temporal_worker_task_slots_available{worker_type=\"WorkflowWorker\"})".to_owned(),
                    series: vec![
                        series(&[("task_queue", "orders")], &even),
                        series(&[("task_queue", "billing")], &gappy),
                    ],
                },
                QueryResult {
                    query: "temporal_request_failure_total".to_owned(),
                    series: vec![
                        series(&[("__name__", "odd"), ("operation", "Poll")], &odd),
                        series(&[("__name__", "temporal_request_failure_total")], &counter),
                    ],
                },
                QueryResult {
                    query: "absent_metric".to_owned(),
                    series: Vec::new(),
                },
            ],
        }
    }

    #[test]
    fn blocks_round_trip_bit_for_bit() {
        let s = sample();
        let text = encode(&s);
        assert_eq!(decode(&text), Ok(s));
    }

    #[test]
    fn a_block_is_found_among_other_text_and_survives_wrapped_lines() {
        let s = sample();
        let text = encode(&s);
        // A narrow terminal wraps each payload line at 40 columns when copied.
        let wrapped: String = text
            .lines()
            .flat_map(|l| {
                if l.starts_with(':') {
                    let chars: Vec<char> = l.chars().collect();
                    chars
                        .chunks(40)
                        .map(|c| c.iter().collect::<String>())
                        .collect::<Vec<_>>()
                } else {
                    vec![l.to_owned()]
                }
            })
            .map(|l| format!("  {l}  \r\n"))
            .collect();
        let pasted = format!("$ kyumei-collect --code …\r\n{wrapped}$ \r\n");
        assert_eq!(decode(&pasted), Ok(s));
    }

    #[test]
    fn several_blocks_are_read_in_order() {
        let a = sample();
        let mut b = sample();
        b.request = Some("R3".to_owned());
        let text = format!("{}\n{}", encode(&a), encode(&b));
        assert_eq!(decode_all(&text), vec![Ok(a), Ok(b)]);
    }

    #[test]
    fn a_missing_line_is_reported_as_incomplete() {
        let text = encode(&sample());
        let lines: Vec<&str> = text.lines().collect();
        let first_payload = lines
            .iter()
            .position(|l| l.starts_with(':'))
            .expect("payload");
        let damaged: Vec<&str> = lines
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != first_payload + 1)
            .map(|(_, l)| *l)
            .collect();
        match decode(&damaged.join("\n")) {
            Err(BlockError::Incomplete { missing, lines, .. }) => {
                assert_eq!(missing, LINE);
                assert_eq!(lines, 1);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_changed_character_fails_the_checksum() {
        let text = encode(&sample());
        let at = text.find("\n:").expect("payload") + 10;
        let mut bytes = text.into_bytes();
        bytes[at] = if bytes[at] == b'A' { b'B' } else { b'A' };
        let damaged = String::from_utf8(bytes).expect("utf8");
        assert_eq!(decode(&damaged), Err(BlockError::Checksum));
    }

    #[test]
    fn a_missing_end_and_added_text_are_reported() {
        let text = encode(&sample());
        let without_end: String = text
            .lines()
            .filter(|l| !l.starts_with(END))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(decode(&without_end), Err(BlockError::MissingEnd));

        let end = text.find(END).expect("end");
        let added = format!("{}:AAAA\n{}", &text[..end], &text[end..]);
        assert!(matches!(
            decode(&added),
            Err(BlockError::TooLong { extra: 4, .. })
        ));
    }

    #[test]
    fn newer_versions_and_missing_blocks_are_reported() {
        let text = encode(&sample()).replace("SAMPLES v1", "SAMPLES v2");
        assert_eq!(decode(&text), Err(BlockError::UnsupportedVersion(2)));
        assert_eq!(decode("nothing here"), Err(BlockError::NotFound));
    }

    #[test]
    fn readable_lines_describe_the_samples() {
        let text = encode(&sample());
        assert!(
            text.contains("Collected by kyumei-collect 0.1.0 from prometheus"),
            "{text}"
        );
        assert!(text.contains("Request R2 of inv-280a0dbd5c"), "{text}");
        assert!(text.contains("task_queue=orders"), "{text}");
        assert!(text.contains("(125 samples)"), "{text}");
        assert!(text.contains("no series"), "{text}");
        assert!(text.lines().all(|l| l.chars().count() <= 140), "{text}");
    }

    #[test]
    fn an_even_two_series_half_hour_fits_in_a_few_lines() {
        let s = sample();
        let text = encode(&s);
        let payload_lines = text.lines().filter(|l| l.starts_with(':')).count();
        assert!(payload_lines <= 15, "{payload_lines} payload lines\n{text}");
    }
}
