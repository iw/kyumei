//! Range queries against a Prometheus-compatible API.

use std::{collections::BTreeMap, time::Duration};

use kyumei_samples::{Point, QueryResult, Samples, Series, Window};
use serde_json::Value;

use crate::endpoint::{Auth, Endpoint};

/// How long one query may take before the collector gives up on it.
const QUERY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum CollectError {
    #[error("could not set up the HTTP client: {0}")]
    Client(String),
    #[error("could not reach {endpoint}: {detail}")]
    Unreachable { endpoint: String, detail: String },
    #[error(
        "{endpoint} refused the credentials (HTTP {status}); check the token or user and password"
    )]
    Unauthorised { endpoint: String, status: u16 },
    #[error("{endpoint} answered HTTP {status}: {detail}")]
    Http {
        endpoint: String,
        status: u16,
        detail: String,
    },
    #[error("the query was refused: {0}")]
    Query(String),
    #[error("the response is not Prometheus query output: {0}")]
    Response(String),
}

/// Reads range queries from one endpoint.
#[derive(Debug)]
pub struct Collector {
    endpoint: Endpoint,
    http: reqwest::Client,
}

/// One query's series, and any warnings the server sent with them.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryOutput {
    pub series: Vec<Series>,
    pub warnings: Vec<String>,
}

impl Collector {
    pub fn new(endpoint: Endpoint) -> Result<Self, CollectError> {
        let mut builder = reqwest::Client::builder()
            .timeout(QUERY_TIMEOUT)
            .user_agent(concat!("kyumei-collect/", env!("CARGO_PKG_VERSION")));
        if let Some(pem) = &endpoint.ca_pem {
            for cert in reqwest::Certificate::from_pem_bundle(pem)
                .map_err(|e| CollectError::Client(format!("the CA file is not PEM: {e}")))?
            {
                builder = builder.add_root_certificate(cert);
            }
        }
        let http = builder
            .build()
            .map_err(|e| CollectError::Client(e.to_string()))?;
        Ok(Self { endpoint, http })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Run one range query.
    pub async fn query_range(
        &self,
        query: &str,
        window: Window,
    ) -> Result<QueryOutput, CollectError> {
        let url = format!("{}/api/v1/query_range", self.endpoint.api_base);
        let mut request = self.http.get(&url).query(&[
            ("query", query.to_owned()),
            ("start", seconds(window.start_ms)),
            ("end", seconds(window.end_ms)),
            ("step", seconds(window.step_ms as i64)),
        ]);
        request = match &self.endpoint.auth {
            Auth::None => request,
            Auth::Bearer(token) => request.bearer_auth(token),
            Auth::Basic { user, password } => request.basic_auth(user, Some(password)),
        };
        if let Some(tenant) = &self.endpoint.tenant {
            request = request.header("X-Scope-OrgID", tenant);
        }
        let response = request
            .send()
            .await
            .map_err(|e| CollectError::Unreachable {
                endpoint: self.endpoint.label.clone(),
                detail: cause(e),
            })?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| CollectError::Unreachable {
                endpoint: self.endpoint.label.clone(),
                detail: cause(e),
            })?;
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(CollectError::Unauthorised {
                endpoint: self.endpoint.label.clone(),
                status: status.as_u16(),
            });
        }
        // Prometheus answers a bad query with 400 and an error in the body.
        match parse_query_range(&body) {
            Err(CollectError::Response(_)) if !status.is_success() => Err(CollectError::Http {
                endpoint: self.endpoint.label.clone(),
                status: status.as_u16(),
                detail: String::from_utf8_lossy(&body).chars().take(300).collect(),
            }),
            other => other,
        }
    }

    /// Run every query over `window` and gather the results as samples.
    pub async fn collect(
        &self,
        queries: &[String],
        window: Window,
        collector: &str,
        collected_at_ms: i64,
    ) -> Result<(Samples, Vec<String>), CollectError> {
        let mut results = Vec::new();
        let mut warnings = Vec::new();
        for query in queries {
            let out = self.query_range(query, window).await?;
            warnings.extend(out.warnings);
            results.push(QueryResult {
                query: query.clone(),
                series: out.series,
            });
        }
        Ok((
            Samples {
                collector: collector.to_owned(),
                source: self.endpoint.source.token().to_owned(),
                collected_at_ms,
                investigation: None,
                request: None,
                window,
                rounded_to: None,
                redacted: Vec::new(),
                queries: results,
            },
            warnings,
        ))
    }
}

/// What went wrong, in the words of its innermost cause, such as
/// "Connection refused", and never with the request URL, which carries the
/// query.
fn cause(e: reqwest::Error) -> String {
    if e.is_timeout() {
        return format!("no answer within {} seconds", QUERY_TIMEOUT.as_secs());
    }
    let e = e.without_url();
    let mut innermost: &dyn std::error::Error = &e;
    while let Some(source) = innermost.source() {
        innermost = source;
    }
    innermost.to_string()
}

/// Milliseconds as the seconds Prometheus takes: `1791105120.5`.
fn seconds(ms: i64) -> String {
    let (s, frac) = (ms.div_euclid(1000), ms.rem_euclid(1000));
    if frac == 0 {
        s.to_string()
    } else {
        format!("{s}.{frac:03}")
    }
}

/// The window saved results cover, with their most common step.
pub fn window_of(results: &[QueryResult]) -> Option<Window> {
    let times: Vec<i64> = results
        .iter()
        .flat_map(|q| &q.series)
        .flat_map(|s| s.points.iter().map(|p| p.at_ms))
        .collect();
    let start_ms = *times.iter().min()?;
    let end_ms = *times.iter().max()?;
    let mut gaps: Vec<i64> = results
        .iter()
        .flat_map(|q| &q.series)
        .flat_map(|s| s.points.windows(2).map(|w| w[1].at_ms - w[0].at_ms))
        .filter(|g| *g > 0)
        .collect();
    gaps.sort_unstable();
    let step_ms = gaps.get(gaps.len() / 2).copied().unwrap_or(15_000) as u64;
    Some(Window {
        start_ms,
        end_ms,
        step_ms,
    })
}

/// Parse `/api/v1/query_range` (or `/api/v1/query`) output.
pub fn parse_query_range(body: &[u8]) -> Result<QueryOutput, CollectError> {
    let value: Value =
        serde_json::from_slice(body).map_err(|e| CollectError::Response(e.to_string()))?;
    if let Some(status) = value.get("status").and_then(Value::as_str)
        && status != "success"
    {
        let error = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        return Err(CollectError::Query(error.to_owned()));
    }
    let warnings = value
        .get("warnings")
        .and_then(Value::as_array)
        .map(|w| {
            w.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let data = value
        .get("data")
        .ok_or_else(|| CollectError::Response("no `data`".to_owned()))?;
    let result_type = data
        .get("resultType")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = data
        .get("result")
        .and_then(Value::as_array)
        .ok_or_else(|| CollectError::Response("no `result` list".to_owned()))?;
    if !matches!(result_type, "matrix" | "vector") {
        return Err(CollectError::Response(format!(
            "`{result_type}` results are not series"
        )));
    }
    let mut series = Vec::new();
    for item in result {
        let labels: BTreeMap<String, String> = item
            .get("metric")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let mut points = Vec::new();
        let pairs = item
            .get("values")
            .and_then(Value::as_array)
            .cloned()
            .or_else(|| item.get("value").map(|v| vec![v.clone()]))
            .unwrap_or_default();
        for pair in &pairs {
            points.push(point(pair)?);
        }
        series.push(Series { labels, points });
    }
    Ok(QueryOutput { series, warnings })
}

/// `[<unix seconds>, "<value>"]`, to the millisecond and the exact value.
fn point(pair: &Value) -> Result<Point, CollectError> {
    let bad = || CollectError::Response(format!("`{pair}` is not a sample"));
    let arr = pair.as_array().ok_or_else(bad)?;
    let at = arr.first().and_then(Value::as_f64).ok_or_else(bad)?;
    let value = match arr.get(1) {
        Some(Value::String(s)) => match s.as_str() {
            "NaN" => f64::NAN,
            "+Inf" | "Inf" => f64::INFINITY,
            "-Inf" => f64::NEG_INFINITY,
            other => other.parse().map_err(|_| bad())?,
        },
        Some(Value::Number(n)) => n.as_f64().ok_or_else(bad)?,
        _ => return Err(bad()),
    };
    Ok(Point {
        at_ms: (at * 1000.0).round() as i64,
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_output_parses_exactly() {
        let body = br#"{"status":"success","warnings":["partial response"],"data":{"resultType":"matrix","result":[
            {"metric":{"__name__":"up","job":"temporal"},"values":[[1791105120,"1"],[1791105135.5,"NaN"],[1791105150,"+Inf"],[1791105165,"0.1"]]},
            {"metric":{},"values":[]}]}}"#;
        let out = parse_query_range(body).expect("parse");
        assert_eq!(out.warnings, vec!["partial response"]);
        assert_eq!(out.series.len(), 2);
        let p = &out.series[0].points;
        assert_eq!(p[1].at_ms, 1_791_105_135_500);
        assert!(p[1].value.is_nan());
        assert_eq!(p[2].value, f64::INFINITY);
        assert_eq!(p[3].value.to_bits(), 0.1f64.to_bits());
        assert_eq!(out.series[0].labels["__name__"], "up");
    }

    #[test]
    fn a_refused_query_reports_prometheus_reason() {
        let body = br#"{"status":"error","errorType":"bad_data","error":"1:5: parse error: unexpected end of input"}"#;
        match parse_query_range(body) {
            Err(CollectError::Query(m)) => assert!(m.contains("parse error"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn times_are_sent_as_seconds() {
        assert_eq!(seconds(1_791_105_120_000), "1791105120");
        assert_eq!(seconds(1_791_105_120_500), "1791105120.500");
        assert_eq!(seconds(15_000), "15");
    }
}
