//! Kyumei's pure diagnostic domain model.
//!
//! This crate has no knowledge of web frameworks, model providers, durable
//! execution or Temporal clients. It defines what an investigation *is*:
//! evidence and its provenance, observations, hypotheses and their states,
//! findings, conclusions and the uncertainty that remains.
//!
//! Every diagnostic statement can answer "where did this come from?": an
//! observation points at its evidence, a finding at its observations and
//! knowledge claims, and a conclusion at its findings.

pub mod conclusion;
pub mod environment;
pub mod epistemic;
pub mod evidence;
pub mod finding;
pub mod float;
pub mod hypothesis;
pub mod ids;
pub mod investigation;
pub mod knowledge;
pub mod observation;
pub mod request;
pub mod symptom;
pub mod time;
pub mod units;

use jiff::Timestamp;

pub use conclusion::{Conclusion, Recommendation, Uncertainty, UncertaintyKind};
pub use environment::{
    DeploymentKind, InvestigationScope, SdkDescriptor, SdkFamily, SdkLanguage, ServerVersion,
    TemporalEnvironment, Topology, describe_releases,
};
pub use epistemic::{Attested, EpistemicStatus, StatementSource};
pub use evidence::{
    CorrectionChange, DeclaredContext, EffectiveObservation, Evidence, EvidenceForm,
    EvidenceOrigin, Interpretation, InterpretationCorrection, InterpretationNote, NoteSeverity,
    RawBody, RawEvidence, SimulationRecord, SubmissionChannel,
};
pub use finding::{DiagnosticFinding, Effect, FactBasis};
pub use hypothesis::{HypothesisState, HypothesisTransition, RecordedState};
pub use ids::*;
pub use investigation::{
    AssessmentRecord, AssessmentTrigger, ContextAmendment, ExplanationAuthor, ExplanationRecord,
    Investigation, InvestigationEvent, InvestigationStatus, Opened, PackRef, RecordedEvent,
    TrailEntry, TrailItem,
};
pub use knowledge::{
    Applicability, ClaimCategory, ClaimSummary, Provenance, SdkScope, Verification,
    VerificationMethod, VersionScope,
};
pub use observation::{
    BindingBasis, Observation, ObservedValue, SeriesPoint, SeriesSummary, SignalBinding, Subject,
    label_value_matches, tally_spelling,
};
pub use request::{
    AbsentReadsZero, CollectionInstruction, CollectionMethod, EvidenceRequest, RequestStatus,
    RequestedWindow,
};
pub use symptom::{ClassificationBasis, Symptom, SymptomClassification};
pub use time::{DisplayZone, IncidentWindow, TimeWindow};
pub use units::{Statistic, Unit, format_number};

/// Domain invariant violations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("window end {end} precedes start {start}")]
    InvalidWindow { start: Timestamp, end: Timestamp },
    #[error("duplicate {0}")]
    Duplicate(String),
    #[error("unknown {0}")]
    Unknown(String),
    #[error("invariant violated: {0}")]
    Invariant(String),
    #[error("an investigation log cannot be empty")]
    EmptyLog,
}
