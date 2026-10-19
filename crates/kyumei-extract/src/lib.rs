//! Kyumei's knowledge extractors: programs that turn a pinned Temporal source checkout into
//! verified knowledge claims, so reference knowledge is generated rather than written by hand.
//!
//! * [`registry_gen`] finds every dynamic config setting a checkout registers. It is
//!   tempdes's, shared rather than copied, with the Go lexing the other extractors use.
//! * [`claims`] renders those settings as Kyumei `[[dynamic_config]]` claims with provenance,
//!   and compares a rendering with a committed file.
//! * [`metrics_gen`] finds every server metric definition and the packages that record it.
//! * [`metric_claims`] renders those as Kyumei `[[metric]]` claims, with an exposition for
//!   each name and unit the server's metrics configuration can produce.
//! * [`sdk_gen`] finds the metrics the Core, Go and Java SDKs define and record, and
//!   [`sdk_claims`] merges them into one claim per name, with each SDK's expositions.
//! * [`sweep`] merges the claims generated from several Temporal releases into one file,
//!   scoping each claim to the releases where it is the same.
//!
//! Generated claims are never edited by hand. Notes, extra provenance, and the parts of a
//! metric the source leaves out (labels, a purpose, a gauge's unit) go in annotation files,
//! which the knowledge loader merges into the generated claims.

pub mod claims;
pub mod metric_claims;
pub mod metrics_gen;
pub use tempdes::config::registry_gen;

pub mod registry {
    //! The dynamic config registry: every setting a Temporal checkout registers. tempdes's.
    pub use tempdes::config::dynamic::{RegistryFile, SettingDef};
}
pub mod sdk_claims;
pub mod sdk_gen;
pub mod sweep;
