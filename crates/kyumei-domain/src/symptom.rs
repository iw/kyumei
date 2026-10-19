//! The symptom an investigation starts from.

use serde::{Deserialize, Serialize};

use crate::ids::{EvidenceId, SymptomSignatureId};

/// The operator's description of what they are seeing.
///
/// The description itself is recorded as evidence (`E0`, an operator
/// observation), so quantities read from it ("about 30 seconds") are operator
/// assertions with the same provenance machinery as any other evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symptom {
    /// Verbatim operator text.
    pub description: String,
    /// The recognised symptom signature, when one was recognised.
    pub classification: Option<SymptomClassification>,
    /// The evidence record holding the description.
    pub evidence: EvidenceId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymptomClassification {
    pub signature: SymptomSignatureId,
    pub basis: ClassificationBasis,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "basis", rename_all = "snake_case")]
pub enum ClassificationBasis {
    /// The operator chose the signature.
    OperatorSelected,
    /// A knowledge-pack pattern matched the description.
    PatternMatch { pattern: String },
    /// A model interpreted the description; the operator confirmed it.
    ModelInterpretation { confirmed: bool },
}

impl ClassificationBasis {
    pub fn describe(&self) -> String {
        match self {
            Self::OperatorSelected => "selected by the operator".to_owned(),
            Self::PatternMatch { pattern } => format!("matched “{pattern}” in the description"),
            Self::ModelInterpretation { confirmed: true } => {
                "model interpretation, confirmed by the operator".to_owned()
            }
            Self::ModelInterpretation { confirmed: false } => {
                "model interpretation (unconfirmed)".to_owned()
            }
        }
    }
}
