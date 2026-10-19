//! Evidence requests: what Kyumei asks the operator for next.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    ids::{ClaimId, EvidenceId, EvidenceRequestId, HypothesisId, RequestDefinitionId, SignalId},
    time::TimeWindow,
    units::Unit,
};

/// A concrete request issued to the operator, rendered for this
/// investigation's environment, scope and window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRequest {
    pub id: EvidenceRequestId,
    pub definition: RequestDefinitionId,
    pub issued_at: Timestamp,
    pub title: String,
    /// Why this evidence matters now.
    pub why: String,
    /// Ways to collect it, most specific first.
    pub collect: Vec<CollectionInstruction>,
    pub window: RequestedWindow,
    pub group_by: Vec<String>,
    /// The hypotheses this evidence discriminates between.
    pub distinguishes: Vec<HypothesisId>,
    /// The signals Kyumei expects the evidence to inform.
    pub expected_signals: Vec<SignalId>,
    /// Evidence submitted in answer, possibly partial.
    #[serde(default)]
    pub responses: Vec<EvidenceId>,
    pub status: RequestStatus,
}

/// One way of collecting the requested evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollectionInstruction {
    pub method: CollectionMethod,
    pub label: String,
    /// The rendered query, command or check.
    pub body: String,
    /// When this instruction applies (e.g. "Go SDK").
    pub applies_to: Option<String>,
    /// The unit the result will be in, when known.
    pub unit: Option<Unit>,
    /// Knowledge claims the instruction relies on.
    pub claims: Vec<ClaimId>,
    /// Placeholders that could not be filled because context is unknown.
    pub unfilled: Vec<String>,
    /// What lets an empty result of this query read as zero, when something does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absent_reads_zero: Option<AbsentReadsZero>,
}

/// A counter's query can return no series because the counter never counted: its
/// source creates it, with its label values, at its first increment. A witness,
/// a counter the same source keeps whenever this one could count, tells that
/// apart from no data at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbsentReadsZero {
    /// The witness's query, rendered as the instruction's is.
    pub witness: String,
    /// The claim that the counter has no series until it first counts.
    pub because: ClaimId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CollectionMethod {
    Promql,
    TemporalCli,
    /// A Temporal API call made directly, such as with grpcurl.
    TemporalApi,
    DynamicConfig,
    Logs,
    OperatorCheck,
}

impl CollectionMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Promql => "PromQL",
            Self::TemporalCli => "Temporal CLI",
            Self::TemporalApi => "Temporal API",
            Self::DynamicConfig => "Dynamic config",
            Self::Logs => "Logs",
            Self::OperatorCheck => "Check",
        }
    }
}

/// The window the evidence should cover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestedWindow {
    pub window: Option<TimeWindow>,
    /// Human rendering, including guidance when the window is unknown.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RequestStatus {
    /// Awaiting evidence (it may already have partial responses).
    Open,
    /// Every expected signal is now informed.
    Answered { evidence: Vec<EvidenceId> },
    /// The operator cannot obtain it.
    Declined { reason: String },
    /// A different request became more useful before this one was answered.
    Superseded,
}

impl RequestStatus {
    pub fn is_open(&self) -> bool {
        matches!(self, Self::Open)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Answered { .. } => "answered",
            Self::Declined { .. } => "unavailable",
            Self::Superseded => "superseded",
        }
    }
}
