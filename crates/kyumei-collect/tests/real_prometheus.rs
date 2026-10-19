//! Against a real Prometheus, when one is given: a block returns every
//! sample exactly as Prometheus sent it.
//!
//! ```bash
//! KYUMEI_PROMETHEUS_URL=http://127.0.0.1:9090 \
//!   cargo test -p kyumei-collect --test real_prometheus -- --ignored --nocapture
//! ```
//!
//! The queries read Prometheus's own metrics, so any Prometheus that has run
//! for a while will do, and they are chosen to return what the encodings must
//! keep: NaN, the infinities, -0, gaps, noisy decimals and raw counters. Two
//! read a Temporal Server's counters, when Prometheus scrapes one: restart it
//! during the window to see counter resets.
//! `KYUMEI_PROMETHEUS_QUERIES` replaces them (separated by `;;`), and
//! `KYUMEI_PROMETHEUS_WINDOW` sets the window ending now (default `30m`).

use std::collections::BTreeMap;

use jiff::{SignedDuration, Timestamp};
use kyumei_collect::{Collector, Endpoint};
use kyumei_samples::{QueryResult, Samples, Window, decode, encode};
use serde_json::Value;

const QUERIES: &[&str] = &[
    // Gauges with whole numbers, and noisy decimals.
    "up",
    "scrape_samples_scraped",
    "scrape_duration_seconds",
    "process_resident_memory_bytes",
    // Raw counters, which reset when a target restarts.
    "prometheus_http_requests_total",
    "process_cpu_seconds_total",
    "service_requests",
    "persistence_requests",
    // Rates, and gaps where a filter drops samples.
    "rate(prometheus_http_requests_total[1m])",
    "rate(prometheus_http_requests_total[1m]) > 0",
    // NaN where a histogram had no observations, and from 0/0.
    "histogram_quantile(0.9, rate(prometheus_http_request_duration_seconds_bucket[1m]))",
    "(up - up) / 0",
    // The infinities and -0.
    "up / 0",
    "-up / 0",
    "up * -0",
];

/// The queries whose series are counters.
const COUNTERS: &[&str] = &[
    "prometheus_http_requests_total",
    "process_cpu_seconds_total",
    "service_requests",
    "persistence_requests",
];

fn queries() -> Vec<String> {
    match std::env::var("KYUMEI_PROMETHEUS_QUERIES") {
        Ok(q) => q.split(";;").map(str::trim).map(str::to_owned).collect(),
        Err(_) => QUERIES.iter().map(|q| (*q).to_owned()).collect(),
    }
}

/// `query_range`'s own output, read without the collector: each series'
/// labels, then `[seconds, "value"]` pairs as Prometheus wrote them.
async fn raw(
    url: &str,
    query: &str,
    w: Window,
) -> Vec<(BTreeMap<String, String>, Vec<(f64, String)>)> {
    let seconds = |ms: i64| format!("{}.{:03}", ms.div_euclid(1000), ms.rem_euclid(1000));
    let bytes = reqwest::Client::new()
        .get(format!("{url}/api/v1/query_range"))
        .query(&[
            ("query", query.to_owned()),
            ("start", seconds(w.start_ms)),
            ("end", seconds(w.end_ms)),
            ("step", seconds(w.step_ms as i64)),
        ])
        .send()
        .await
        .expect("query")
        .bytes()
        .await
        .expect("body");
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(body["status"], "success", "{query}: {body}");
    body["data"]["result"]
        .as_array()
        .expect("result")
        .iter()
        .map(|s| {
            let labels = s["metric"]
                .as_object()
                .expect("metric")
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().expect("label").to_owned()))
                .collect();
            let values = s["values"]
                .as_array()
                .expect("values")
                .iter()
                .map(|p| {
                    (
                        p[0].as_f64().expect("time"),
                        p[1].as_str().expect("value").to_owned(),
                    )
                })
                .collect();
            (labels, values)
        })
        .collect()
}

/// Prometheus's text for a value, as the bits it stands for.
fn bits(text: &str) -> u64 {
    match text {
        "NaN" => f64::NAN.to_bits(),
        "+Inf" => f64::INFINITY.to_bits(),
        "-Inf" => f64::NEG_INFINITY.to_bits(),
        t => t.parse::<f64>().expect("a number").to_bits(),
    }
}

#[tokio::test]
#[ignore = "needs a Prometheus: set KYUMEI_PROMETHEUS_URL"]
async fn real_query_results_come_back_from_a_block_exactly() {
    let url = std::env::var("KYUMEI_PROMETHEUS_URL").expect("KYUMEI_PROMETHEUS_URL");
    let url = url.trim_end_matches('/').to_owned();
    let span: SignedDuration = std::env::var("KYUMEI_PROMETHEUS_WINDOW")
        .unwrap_or_else(|_| "30m".to_owned())
        .parse()
        .expect("a duration");
    let end = Timestamp::now().as_millisecond() / 1000 * 1000;
    let window = Window {
        start_ms: end - span.as_millis() as i64,
        end_ms: end,
        step_ms: 15_000,
    };
    let collector =
        Collector::new(Endpoint::prometheus(&url).expect("endpoint")).expect("collector");
    let queries = queries();
    let (samples, warnings) = collector
        .collect(&queries, window, "real-prometheus test", end)
        .await
        .expect("collect");
    assert!(warnings.is_empty(), "{warnings:?}");

    // The collector read what Prometheus sent, value for value.
    let mut seen = BTreeMap::<&str, usize>::new();
    for (q, result) in queries.iter().zip(&samples.queries) {
        let expected = raw(&url, q, window).await;
        assert_eq!(result.series.len(), expected.len(), "{q}: series");
        for (series, (labels, values)) in result.series.iter().zip(&expected) {
            assert_eq!(&series.labels, labels, "{q}");
            assert_eq!(series.points.len(), values.len(), "{q} {labels:?}");
            for (p, (at, text)) in series.points.iter().zip(values) {
                assert_eq!(p.at_ms, (at * 1000.0).round() as i64, "{q}");
                assert_eq!(
                    p.value.to_bits(),
                    bits(text),
                    "{q} {labels:?} at {at}: {text}"
                );
            }
            let points = &series.points;
            *seen.entry("samples").or_default() += points.len();
            *seen.entry("NaN").or_default() += points.iter().filter(|p| p.value.is_nan()).count();
            *seen.entry("infinite").or_default() +=
                points.iter().filter(|p| p.value.is_infinite()).count();
            *seen.entry("-0").or_default() += points
                .iter()
                .filter(|p| p.value == 0.0 && p.value.is_sign_negative())
                .count();
            *seen.entry("gaps").or_default() += points
                .windows(2)
                .filter(|w| w[1].at_ms - w[0].at_ms > window.step_ms as i64)
                .count();
            if COUNTERS.contains(&q.as_str()) {
                *seen.entry("counter resets").or_default() += points
                    .windows(2)
                    .filter(|w| w[1].value < w[0].value)
                    .count();
            }
        }
    }

    // A block returns the samples exactly, one query at a time and all together.
    for result in &samples.queries {
        let one = Samples {
            queries: vec![QueryResult {
                query: result.query.clone(),
                series: result.series.clone(),
            }],
            ..samples.clone()
        };
        assert_eq!(
            decode(&encode(&one)).expect("decode"),
            one,
            "{}",
            result.query
        );
    }
    let block = encode(&samples);
    assert_eq!(decode(&block).expect("decode"), samples);

    let lines = block.lines().filter(|l| l.starts_with(':')).count();
    eprintln!(
        "{seen:?}; one block of {} queries: {lines} lines",
        queries.len()
    );
    if std::env::var_os("KYUMEI_PROMETHEUS_QUERIES").is_none() {
        for kind in ["NaN", "infinite", "-0", "gaps"] {
            assert!(
                seen.get(kind).copied().unwrap_or(0) > 0,
                "no {kind} in {seen:?}"
            );
        }
    }
}
