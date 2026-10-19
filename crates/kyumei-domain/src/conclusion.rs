//! Conclusions and the uncertainty that remains.

use serde::{Deserialize, Serialize};

use crate::ids::{
    ClaimId, ConclusionId, DomainRef, EvidenceId, FindingId, HypothesisId, ObservationId,
};

/// An evidence-backed diagnosis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conclusion {
    pub id: ConclusionId,
    pub title: String,
    pub statement: String,
    pub established: Vec<HypothesisId>,
    pub supported: Vec<HypothesisId>,
    pub eliminated: Vec<HypothesisId>,
    /// Every finding the conclusion rests on.
    pub findings: Vec<FindingId>,
    pub evidence: Vec<EvidenceId>,
    pub observations: Vec<ObservationId>,
    pub recommendations: Vec<Recommendation>,
    pub residual_uncertainty: Vec<Uncertainty>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recommendation {
    pub text: String,
    pub caution: Option<String>,
    pub claims: Vec<ClaimId>,
}

/// Something Kyumei does not know, or knows only weakly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uncertainty {
    pub kind: UncertaintyKind,
    pub text: String,
    pub refs: Vec<DomainRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyKind {
    /// A hypothesis is still open.
    OpenHypothesis,
    /// Evidence points both ways.
    Contested,
    /// A finding rests on an operator assertion rather than telemetry.
    AssertionOnly,
    /// Knowledge is not verified for the target server version.
    VersionUnverified,
    /// Evidence does not cover the incident window.
    WindowMismatch,
    /// Kyumei assumed a unit or signal the operator should confirm.
    InterpretationAssumed,
    /// Requested evidence could not be obtained.
    EvidenceUnavailable,
    /// Context that would sharpen the investigation is unknown.
    MissingContext,
    /// Supplied evidence could not be related to the knowledge pack.
    Unmapped,
}

impl UncertaintyKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenHypothesis => "Open hypothesis",
            Self::Contested => "Contested",
            Self::AssertionOnly => "Rests on assertion",
            Self::VersionUnverified => "Version unverified",
            Self::WindowMismatch => "Window mismatch",
            Self::InterpretationAssumed => "Assumed interpretation",
            Self::EvidenceUnavailable => "Evidence unavailable",
            Self::MissingContext => "Missing context",
            Self::Unmapped => "Unmapped evidence",
        }
    }
}
