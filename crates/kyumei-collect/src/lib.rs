//! Reading metric samples for Kyumei.
//!
//! [`Collector`] runs range queries against a Prometheus-compatible API, or a
//! Prometheus data source behind Grafana's data source proxy, and gathers the
//! results as [`kyumei_samples::Samples`]. The `kyumei-collect` binary prints
//! them as a sample block to paste into Kyumei; Kyumei's own "Collect now"
//! uses the same collector when it can reach the endpoint itself.
//!
//! Nothing here writes to the endpoint: the collector only reads.

mod client;
mod endpoint;
mod fill;
pub mod qr;
pub mod redact;

pub use client::{CollectError, Collector, QueryOutput, parse_query_range, window_of};
pub use endpoint::{Auth, ENV_PASSWORD, ENV_TOKEN, ENV_USER, Endpoint, EndpointError, Source};
pub use fill::{fill, parse_settings, tally_spelling};

/// What blocks the companion prints say collected them.
pub const COLLECTOR: &str = concat!("kyumei-collect ", env!("CARGO_PKG_VERSION"));

/// The largest number of samples per series Prometheus returns for one query.
pub const MAX_POINTS_PER_SERIES: u64 = 11_000;
