//! Kyumei sample blocks.
//!
//! A sample block carries the results of one or more Prometheus range queries
//! as a short block of text: a few readable lines that say what was collected
//! and sketch each series, then the samples themselves, compressed, as
//! base64url text in 76-column lines, and a last line with their length and
//! CRC-32. It survives being copied out of a terminal and pasted into Kyumei,
//! and Kyumei rejects a block that lost or changed characters on the way
//! instead of misreading it.
//!
//! Samples are exact: every timestamp to the millisecond, and every value bit
//! for bit, NaN and infinities included. A writer may round values to a number
//! of significant digits for a smaller block; the block then says so.
//!
//! [`RequestCode`] is the other direction: one line, copied from a Kyumei
//! evidence request, that tells `kyumei-collect` which queries to run over
//! which window.
//!
//! The format is specified in `docs/collector.md`.

mod block;
mod code;
mod codec;
mod render;
mod wire;

use std::collections::BTreeMap;

pub use block::{BlockError, decode, decode_all, encode};
pub use code::{CodeError, RequestCode, placeholders};
pub use codec::round_significant;
pub use render::compact_number;

/// The format version this crate writes. Readers reject newer versions.
pub const FORMAT_VERSION: u8 = 1;

/// Metric samples collected for a Kyumei evidence request, or for queries
/// given by hand.
#[derive(Clone, Debug, PartialEq)]
pub struct Samples {
    /// What collected them, such as `kyumei-collect 0.1.0`.
    pub collector: String,
    /// Where they were read: `prometheus`, `grafana` or `file`.
    pub source: String,
    /// When they were read, in milliseconds since the Unix epoch.
    pub collected_at_ms: i64,
    /// The investigation and request they answer, when a request code was used.
    pub investigation: Option<String>,
    pub request: Option<String>,
    /// The range the queries were run over.
    pub window: Window,
    /// The significant digits values were rounded to; `None` when exact.
    pub rounded_to: Option<u8>,
    /// Labels whose values were replaced by pseudonyms before the block was written.
    pub redacted: Vec<String>,
    pub queries: Vec<QueryResult>,
}

/// A range query's range: start and end in milliseconds since the Unix epoch,
/// and the step between samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub start_ms: i64,
    pub end_ms: i64,
    pub step_ms: u64,
}

/// One query and the series it returned. A query that returned nothing has
/// no series.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryResult {
    pub query: String,
    pub series: Vec<Series>,
}

/// One series: its labels (with `__name__` when the query kept it) and its
/// samples in time order.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub labels: BTreeMap<String, String>,
    pub points: Vec<Point>,
}

/// One sample. Equality compares the value's bits, so NaN equals itself and
/// `0.0` differs from `-0.0`: blocks are exact to the bit.
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub at_ms: i64,
    pub value: f64,
}

impl PartialEq for Point {
    fn eq(&self, other: &Self) -> bool {
        self.at_ms == other.at_ms && self.value.to_bits() == other.value.to_bits()
    }
}

impl Eq for Point {}
