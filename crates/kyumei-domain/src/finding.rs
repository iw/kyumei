//! Findings: what a diagnostic rule concluded from facts.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    epistemic::EpistemicStatus,
    ids::{ClaimId, EvidenceId, FindingId, HypothesisId, ObservationId, RuleId, SignalId},
    units::{Statistic, Unit},
};

/// How a finding bears on a hypothesis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Supports,
    Weakens,
    Eliminates,
    Establishes,
}

impl Effect {
    pub fn label(self) -> &'static str {
        match self {
            Self::Supports => "supports",
            Self::Weakens => "weakens",
            Self::Eliminates => "eliminates",
            Self::Establishes => "establishes",
        }
    }

    pub fn is_positive(self) -> bool {
        matches!(self, Self::Supports | Self::Establishes)
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One fact a finding rests on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FactBasis {
    pub signal: SignalId,
    pub statistic: Statistic,
    #[serde(with = "crate::float")]
    pub value: f64,
    pub unit: Unit,
    /// The value of a categorical fact, such as an enum name; `value` is
    /// then 0 and not shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Observed fact or operator assertion.
    pub status: EpistemicStatus,
    pub observations: Vec<ObservationId>,
    pub evidence: Vec<EvidenceId>,
}

impl FactBasis {
    /// The value as shown to an operator.
    pub fn display(&self) -> String {
        self.category
            .clone()
            .unwrap_or_else(|| self.unit.format_value(self.value))
    }
}

/// A diagnostic finding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticFinding {
    pub id: FindingId,
    pub rule: RuleId,
    pub hypothesis: HypothesisId,
    pub effect: Effect,
    /// The rendered statement, e.g. "Worker execution saturation is not
    /// supported: …".
    pub statement: String,
    pub basis: Vec<FactBasis>,
    /// Knowledge relied on: metric definitions, thresholds, behaviours.
    pub claims: Vec<ClaimId>,
    /// Always [`EpistemicStatus::Inference`].
    pub status: EpistemicStatus,
}

impl DiagnosticFinding {
    /// True when any fact behind the finding is only an operator assertion.
    pub fn rests_on_assertion(&self) -> bool {
        self.basis
            .iter()
            .any(|b| b.status == EpistemicStatus::OperatorAssertion)
    }

    /// True when any fact behind the finding came from a simulation.
    pub fn rests_on_simulation(&self) -> bool {
        self.basis
            .iter()
            .any(|b| b.status == EpistemicStatus::Simulated)
    }

    pub fn evidence(&self) -> Vec<EvidenceId> {
        let mut ids: Vec<EvidenceId> = self
            .basis
            .iter()
            .flat_map(|b| b.evidence.iter().cloned())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    pub fn observations(&self) -> Vec<ObservationId> {
        let mut ids: Vec<ObservationId> = self
            .basis
            .iter()
            .flat_map(|b| b.observations.iter().cloned())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }
}
