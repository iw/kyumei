//! Request codes: one line, copied from a Kyumei evidence request, that tells
//! `kyumei-collect` which queries to run over which window.
//!
//! A code is `kyq1.` followed by base64url (no padding) of raw deflate of a
//! small JSON object. Queries keep Kyumei's placeholders, such as
//! `<namespace>`, for values Kyumei does not know; the collector fills them.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

const PREFIX: &str = "kyq1.";
const MAX_INFLATED: usize = 1 << 20;

/// What a Kyumei evidence request asks a collector to run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestCode {
    #[serde(rename = "i")]
    pub investigation: String,
    #[serde(rename = "r")]
    pub request: String,
    /// PromQL range queries, possibly with `<name>` placeholders.
    #[serde(rename = "q")]
    pub queries: Vec<String>,
    #[serde(rename = "s")]
    pub start_ms: i64,
    #[serde(rename = "e")]
    pub end_ms: i64,
    #[serde(rename = "p")]
    pub step_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CodeError {
    #[error("This is not a Kyumei request code: codes start with `{PREFIX}`.")]
    NotACode,
    #[error("The request code is damaged: copy it again from the request.")]
    Damaged,
    #[error("The request code is not valid: {0}.")]
    Invalid(&'static str),
}

impl RequestCode {
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        let compressed = miniz_oxide::deflate::compress_to_vec(&json, 10);
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(compressed))
    }

    pub fn decode(code: &str) -> Result<Self, CodeError> {
        let body = code
            .trim()
            .strip_prefix(PREFIX)
            .ok_or(CodeError::NotACode)?;
        let compressed = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| CodeError::Damaged)?;
        let json = miniz_oxide::inflate::decompress_to_vec_with_limit(&compressed, MAX_INFLATED)
            .map_err(|_| CodeError::Damaged)?;
        let code: Self = serde_json::from_slice(&json).map_err(|_| CodeError::Damaged)?;
        if code.queries.is_empty() {
            return Err(CodeError::Invalid("it names no queries"));
        }
        if code.end_ms <= code.start_ms {
            return Err(CodeError::Invalid("its window ends before it starts"));
        }
        if code.step_ms == 0 {
            return Err(CodeError::Invalid("its step is zero"));
        }
        Ok(code)
    }

    /// The `<name>` placeholders in the queries, in first-seen order.
    pub fn placeholders(&self) -> Vec<String> {
        let mut names = Vec::new();
        for q in &self.queries {
            for name in placeholders(q) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }
}

/// The `<name>` placeholders in one query: a name of letters, digits and
/// underscores between angle brackets.
pub fn placeholders(query: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = query;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        match after.find('>') {
            Some(close)
                if close > 0
                    && after[..close]
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
            {
                names.push(after[..close].to_owned());
                rest = &after[close + 1..];
            }
            _ => rest = after,
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code() -> RequestCode {
        RequestCode {
            investigation: "inv-280a0dbd5c".to_owned(),
            request: "R2".to_owned(),
            queries: vec![
                "sum by (task_queue) (temporal_worker_task_slots_available{worker_type=\"WorkflowWorker\", namespace=\"<namespace>\", task_queue=\"<task_queue>\"})".to_owned(),
                "sum(rate(temporal_long_request_failure_total{namespace=\"<namespace>\"}[1m])) > 0".to_owned(),
            ],
            start_ms: 1_791_104_520_000,
            end_ms: 1_791_106_380_000,
            step_ms: 15_000,
        }
    }

    #[test]
    fn codes_round_trip_and_stay_one_short_line() {
        let c = code();
        let text = c.encode();
        assert!(text.starts_with("kyq1."));
        assert!(text.len() < 400, "{} chars: {text}", text.len());
        assert!(
            text.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
            "shell-safe: {text}"
        );
        assert_eq!(RequestCode::decode(&format!("  {text}\n")), Ok(c));
    }

    #[test]
    fn placeholders_are_listed_once_and_comparisons_are_not_mistaken_for_them() {
        assert_eq!(code().placeholders(), vec!["namespace", "task_queue"]);
        assert!(placeholders("rate(x[1m]) < 5 and y > 2").is_empty());
    }

    #[test]
    fn damaged_or_foreign_codes_are_refused() {
        let text = code().encode();
        assert_eq!(RequestCode::decode("hello"), Err(CodeError::NotACode));
        let truncated = &text[..text.len() - 10];
        assert_eq!(RequestCode::decode(truncated), Err(CodeError::Damaged));
    }
}
