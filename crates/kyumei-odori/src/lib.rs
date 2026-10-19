//! Durable investigation orchestration on Odori.
//!
//! Each investigation is a long-running workflow on the embedded tokeira
//! engine. It waits for the operator for as long as it takes — minutes or
//! days — and survives restarts through the engine's snapshot. Operator
//! commands are workflow updates; the deterministic core runs in activities;
//! model explanations are Odori agent runs started as child workflows.
//!
//! The workflow orchestrates the domain engine; it never becomes it.

pub mod activities;
pub mod agents;
pub mod runtime;
pub mod workflow;

pub use agents::Explainer;
pub use runtime::{DurableConfig, DurableInvestigations};
