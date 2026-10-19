//! Grafana dashboards, read so that Kyumei can draw evidence the way a panel
//! draws it: its legend, unit, decimals, thresholds and legend statistics.
//!
//! Dashboards come from the operator: a directory loaded at startup, or an
//! upload. Kyumei bundles none. Panels are found by the metrics their queries
//! read, and checked against what Kyumei's claims say about those metrics.
//!
//! This crate knows Grafana's dashboard JSON and enough PromQL to read a
//! panel's queries. It knows nothing of Temporal or of the rest of Kyumei.

pub mod crosscheck;
pub mod grafana;
pub mod legend;
pub mod library;
pub mod promql;
pub mod units;

pub use crosscheck::{Finding, FindingKind, KnownSeries, cross_check};
pub use grafana::{
    Dashboard, LegendCalc, LegendDisplay, Panel, PanelKind, ParseError, Target, ThresholdMode,
    ThresholdStyle, Thresholds, Unreadable, parse,
};
pub use library::{Entry, Library, LoadProblem, Origin, PanelMatch};
