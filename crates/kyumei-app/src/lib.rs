//! Kyumei's application layer.
//!
//! The web interface depends on this crate and nothing below it: commands,
//! the [`service::InvestigationService`] port, workspace view models,
//! exports and fixture replay. Durable orchestration (kyumei-odori) plugs in
//! behind the same port.

pub mod blob;
pub mod collect;
pub mod config;
pub mod core;
pub mod dashboards;
pub mod export;
pub mod fixture;
pub mod panels;
pub mod service;
pub mod simulation;
pub mod workspace;
pub mod zone;

mod toml_time;

pub use core::{
    Command, ContextInput, CorrectionInput, EvidenceSubmission, OpenInvestigation, SdkInput,
    StoredContent, StructuredInput,
};
pub use service::{
    Clock, CommandReceipt, ExplanationStatus, InvestigationService, InvestigationSnapshot,
    InvestigationSummary, LocalInvestigations, RuntimeInfo, ServiceError, SystemClock,
};
