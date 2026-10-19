//! Simulated Temporal clusters for Kyumei.
//!
//! [tempdes](https://github.com/iw/tempdes) simulates a Temporal Server 1.31.0
//! cluster request by request. This crate runs a tempdes scenario and scrapes
//! its simulated metrics every few simulated seconds, as Prometheus scrapes a
//! real cluster. It then answers PromQL queries over those scrapes, with the
//! subset of PromQL Kyumei's evidence requests use. The answers are
//! [`kyumei_samples::Samples`], what Collect now reads from Prometheus, so
//! simulated evidence travels the same way as collected evidence.
//!
//! This is the only crate that knows tempdes.

mod collect;
pub mod dispatch;
pub mod promql;
mod run;
mod scrape;

pub use collect::CollectError;
pub use run::{Run, RunError, Scrape, Simulation};
pub use scrape::{Sample, ScrapeError, parse_exposition};

/// The tempdes commit Kyumei is built against: the `rev` of the workspace's
/// tempdes dependency. Simulated evidence cites it.
pub const TEMPDES_COMMIT: &str = "22b8dc97f181c88f70ba723842b293668645f2a8";
