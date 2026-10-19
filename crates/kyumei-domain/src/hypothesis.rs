//! Hypothesis states.
//!
//! Kyumei never shows a numeric confidence. A hypothesis is in one of five
//! explainable states, each justified by findings.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{FindingId, HypothesisId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisState {
    /// Under consideration; no evidence has moved it yet.
    Candidate,
    /// Evidence points toward it; not yet established.
    Supported,
    /// Evidence points away from it; not ruled out.
    Weakened,
    /// Evidence rules it out.
    Eliminated,
    /// Evidence establishes it.
    Established,
}

impl HypothesisState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Supported => "supported",
            Self::Weakened => "weakened",
            Self::Eliminated => "eliminated",
            Self::Established => "established",
        }
    }

    /// A glyph that carries the state without colour.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Candidate => "?",
            Self::Supported => "+",
            Self::Weakened => "−",
            Self::Eliminated => "✕",
            Self::Established => "✓",
        }
    }

    /// Resolved hypotheses need no further evidence.
    pub fn is_resolved(self) -> bool {
        matches!(self, Self::Eliminated | Self::Established)
    }

    /// Leading hypotheses are those evidence currently favours.
    pub fn is_leading(self) -> bool {
        matches!(self, Self::Supported | Self::Established)
    }
}

impl fmt::Display for HypothesisState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A recorded change of hypothesis state and the findings that caused it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HypothesisTransition {
    pub hypothesis: HypothesisId,
    /// `None` when the hypothesis was first considered.
    pub from: Option<HypothesisState>,
    pub to: HypothesisState,
    pub because: Vec<FindingId>,
}

/// A hypothesis state as recorded in an assessment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedState {
    pub state: HypothesisState,
    /// Evidence both supports and undermines it.
    pub contested: bool,
}
