//! Stable, human-readable identifiers.
//!
//! Identifiers are plain strings so that fixtures, diagrams and exports stay
//! legible (`E2`, `E2.O1`, `worker-slot-exhaustion`). Each kind of identifier
//! is its own type so an evidence identifier can never be passed where a
//! hypothesis identifier is expected.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

string_id!(
    /// One investigation, e.g. `inv-7f3a9c`.
    InvestigationId
);
string_id!(
    /// One submitted piece of evidence, e.g. `E3`. `E0` is the symptom report.
    EvidenceId
);
string_id!(
    /// One observation derived from evidence, e.g. `E3.O1`.
    ObservationId
);
string_id!(
    /// One operator correction to an interpretation, e.g. `C1`.
    CorrectionId
);
string_id!(
    /// One evidence request issued to the operator, e.g. `R2`.
    EvidenceRequestId
);
string_id!(
    /// A knowledge-pack evidence request definition, e.g. `workflow-worker-slots`.
    RequestDefinitionId
);
string_id!(
    /// A knowledge-pack hypothesis, e.g. `worker-slot-exhaustion`.
    HypothesisId
);
string_id!(
    /// A knowledge-pack simulation, e.g. `dispatch-ceiling-size`.
    SimulationId
);
string_id!(
    /// One diagnostic finding. Findings are derived from rules, so the
    /// identifier is stable for a given rule within an investigation.
    FindingId
);
string_id!(
    /// A knowledge-pack diagnostic rule.
    RuleId
);
string_id!(
    /// A diagnostic signal: a quantity the diagnostic rules reason about,
    /// independent of which metric or API field supplied it.
    SignalId
);
string_id!(
    /// A versioned knowledge claim (metric definition, dynamic config key,
    /// API field, behaviour, heuristic).
    ClaimId
);
string_id!(
    /// A provenance source (repository revision, document, authored heuristics).
    SourceId
);
string_id!(
    /// A stage of a diagnostic path, e.g. `worker-capacity`.
    StageId
);
string_id!(
    /// A knowledge-pack conclusion definition.
    ConclusionId
);
string_id!(
    /// A diagnostic definition within a pack, e.g. `wft-schedule-to-start`.
    DiagnosticId
);
string_id!(
    /// A recognised symptom signature, e.g. `wft-schedule-to-start-elevated`.
    SymptomSignatureId
);
string_id!(
    /// A knowledge pack, e.g. `temporal-1.31`.
    PackId
);
string_id!(
    /// A reference to operations material outside a pack (a runbook, a playbook
    /// section, a dashboard panel), e.g. `ops.runbook.sdk-19`.
    ReferenceId
);

/// A reference to any addressable object in an investigation.
///
/// Diagrams, explanations and uncertainties use these so that every rendered
/// claim can be traced back to the object that justifies it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum DomainRef {
    Evidence(EvidenceId),
    Observation(ObservationId),
    Hypothesis(HypothesisId),
    Finding(FindingId),
    Request(EvidenceRequestId),
    Claim(ClaimId),
    Conclusion(ConclusionId),
    Signal(SignalId),
}

impl fmt::Display for DomainRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Evidence(id) => write!(f, "evidence:{id}"),
            Self::Observation(id) => write!(f, "observation:{id}"),
            Self::Hypothesis(id) => write!(f, "hypothesis:{id}"),
            Self::Finding(id) => write!(f, "finding:{id}"),
            Self::Request(id) => write!(f, "request:{id}"),
            Self::Claim(id) => write!(f, "claim:{id}"),
            Self::Conclusion(id) => write!(f, "conclusion:{id}"),
            Self::Signal(id) => write!(f, "signal:{id}"),
        }
    }
}
