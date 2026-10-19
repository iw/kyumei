//! What kind of statement something is, and where it came from.

use serde::{Deserialize, Serialize};

use crate::ids::{ClaimId, EvidenceId, ObservationId, RuleId};
use jiff::Timestamp;

/// The epistemic standing of a statement.
///
/// Kyumei renders every diagnostic statement with one of these so an operator
/// can always tell a measurement from a belief.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicStatus {
    /// Established from supplied telemetry, configuration or output.
    ObservedFact,
    /// Stated by the operator without supporting telemetry.
    OperatorAssertion,
    /// Produced by a simulation calibrated on observed facts. Like an
    /// assertion, it can support or weaken a hypothesis, never settle one.
    Simulated,
    /// Derived by a diagnostic rule from facts and assertions.
    Inference,
    /// A candidate explanation that is still open.
    Hypothesis,
    /// A candidate explanation that evidence has ruled out.
    EliminatedHypothesis,
    /// A diagnosis supported by the evidence trail.
    Conclusion,
    /// Not known. Unknown is never treated as false.
    Unknown,
}

impl EpistemicStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::ObservedFact => "Observed fact",
            Self::OperatorAssertion => "Operator assertion",
            Self::Simulated => "Simulated fact",
            Self::Inference => "Inference",
            Self::Hypothesis => "Hypothesis",
            Self::EliminatedHypothesis => "Eliminated hypothesis",
            Self::Conclusion => "Conclusion",
            Self::Unknown => "Unknown",
        }
    }

    /// Short machine-friendly token, used for styling and text exports.
    pub fn token(self) -> &'static str {
        match self {
            Self::ObservedFact => "observed",
            Self::OperatorAssertion => "asserted",
            Self::Simulated => "simulated",
            Self::Inference => "inferred",
            Self::Hypothesis => "hypothesis",
            Self::EliminatedHypothesis => "eliminated",
            Self::Conclusion => "conclusion",
            Self::Unknown => "unknown",
        }
    }
}

/// Where a statement came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StatementSource {
    /// The operator typed it.
    Operator { at: Timestamp },
    /// It was read from submitted evidence.
    Evidence {
        evidence: EvidenceId,
        observation: Option<ObservationId>,
    },
    /// It comes from versioned knowledge (a documented default, for example).
    Knowledge { claim: ClaimId },
    /// A diagnostic rule derived it.
    Inference { rule: RuleId },
}

/// A value together with its standing and origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attested<T> {
    pub value: T,
    pub status: EpistemicStatus,
    pub source: StatementSource,
}

impl<T> Attested<T> {
    pub fn asserted(value: T, at: Timestamp) -> Self {
        Self {
            value,
            status: EpistemicStatus::OperatorAssertion,
            source: StatementSource::Operator { at },
        }
    }

    pub fn observed(value: T, evidence: EvidenceId, observation: Option<ObservationId>) -> Self {
        Self {
            value,
            status: EpistemicStatus::ObservedFact,
            source: StatementSource::Evidence {
                evidence,
                observation,
            },
        }
    }
}
