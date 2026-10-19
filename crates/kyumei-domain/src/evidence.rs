//! Evidence: what the operator (or a future collector) supplied.
//!
//! The raw submission is immutable. Kyumei's interpretation of it is stored
//! beside it, and operator corrections are stored separately and applied on
//! read, so the original submission is never altered.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    ids::{
        CorrectionId, EvidenceId, EvidenceRequestId, HypothesisId, ObservationId, SignalId,
        SimulationId,
    },
    observation::{BindingBasis, Observation, SignalBinding},
    time::TimeWindow,
    units::Unit,
};

/// One piece of evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub id: EvidenceId,
    pub submitted_at: Timestamp,
    pub origin: EvidenceOrigin,
    /// The request this evidence answers, if any.
    pub responds_to: Option<EvidenceRequestId>,
    pub raw: RawEvidence,
    /// What the operator told Kyumei about the submission.
    pub declared: DeclaredContext,
    /// Kyumei's reading of the submission at the time it was received.
    pub interpretation: Interpretation,
    /// What a simulation was calibrated with and found, when this records one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simulation: Option<SimulationRecord>,
}

/// What a simulation was calibrated with and what it found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimulationRecord {
    /// The knowledge pack's simulation.
    pub simulation: SimulationId,
    pub hypothesis: HypothesisId,
    /// The model's inputs by role, as the facts gave them. Always finite.
    pub inputs: BTreeMap<String, f64>,
    /// The evidence those facts came from.
    pub calibrated_on: Vec<EvidenceId>,
    /// The period the run stands for.
    pub window: TimeWindow,
    /// The rates were divided by this to keep the run small; their ratio is
    /// unchanged.
    pub scale: f64,
    /// What the run measured, by name. Always finite.
    pub results: BTreeMap<String, f64>,
    /// Why the run gave no result, when it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

/// How the evidence reached Kyumei.
///
/// Manual submission and a future `kyumei collect` produce the same evidence
/// objects; only this origin differs. The diagnostic engine ignores it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceOrigin {
    Operator {
        channel: SubmissionChannel,
    },
    Collector {
        name: String,
        version: String,
    },
    Fixture {
        fixture: String,
        step: u32,
    },
    /// A run of a simulator, at the commit it was built from, with the seed
    /// that makes it repeatable.
    Simulation {
        simulator: String,
        commit: String,
        seed: u64,
    },
}

impl EvidenceOrigin {
    pub fn describe(&self) -> String {
        match self {
            Self::Operator { channel } => format!("Operator ({})", channel.label()),
            Self::Collector { name, version } => format!("Collector {name} {version}"),
            Self::Fixture { fixture, step } => format!("Fixture {fixture} step {step}"),
            Self::Simulation {
                simulator,
                commit,
                seed,
            } => format!(
                "Simulation by {simulator} {}, seed {seed}",
                commit.get(..7).unwrap_or(commit)
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionChannel {
    Symptom,
    Paste,
    Upload,
    StructuredEntry,
    Observation,
    /// Read from Prometheus or Grafana by Kyumei itself ("Collect now").
    Collector,
}

impl SubmissionChannel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Symptom => "symptom report",
            Self::Paste => "pasted",
            Self::Upload => "uploaded file",
            Self::StructuredEntry => "structured entry",
            Self::Observation => "written observation",
            Self::Collector => "collected",
        }
    }
}

/// The unmodified submission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawEvidence {
    pub media_type: String,
    pub filename: Option<String>,
    pub body: RawBody,
    /// Hex SHA-256 of the exact submitted bytes.
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RawBody {
    /// Text kept inline, byte-for-byte as submitted.
    Text { text: String },
    /// Binary content (screenshots) kept in the evidence store, addressed by
    /// its SHA-256.
    Stored { reference: String },
}

impl RawEvidence {
    pub fn text(&self) -> Option<&str> {
        match &self.body {
            RawBody::Text { text } => Some(text),
            RawBody::Stored { .. } => None,
        }
    }
}

/// What the operator declared alongside the submission. All optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredContext {
    /// Operator's statement of the evidence form, overriding detection.
    pub form: Option<EvidenceForm>,
    /// The period the evidence covers.
    pub window: Option<TimeWindow>,
    /// The query or command that produced it.
    pub query: Option<String>,
    /// The signal the operator says this is.
    pub signal: Option<SignalId>,
    /// The unit the values are in.
    pub unit: Option<Unit>,
    /// Extra labels the operator attached (e.g. `worker_type=WorkflowWorker`).
    pub labels: BTreeMap<String, String>,
    pub note: Option<String>,
}

/// The recognised forms of evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceForm {
    OperatorObservation,
    /// Values entered through the structured form.
    MetricValues,
    /// Prometheus HTTP API output (`/api/v1/query` or `/api/v1/query_range`).
    PrometheusQueryOutput,
    /// Samples written by `kyumei-collect` (or Kyumei's Collect now) as a
    /// Kyumei sample block.
    SampleBlock,
    /// Prometheus text exposition (`/metrics` scrape).
    PrometheusExposition,
    /// Time series pasted as text (`timestamp value` lines).
    MetricSeriesText,
    Csv,
    Json,
    Log,
    Configuration,
    CommandOutput,
    Attachment,
    /// A simulation's result, recorded by Kyumei itself.
    Simulation,
    Unrecognised,
}

impl EvidenceForm {
    pub const ALL: [EvidenceForm; 14] = [
        Self::OperatorObservation,
        Self::MetricValues,
        Self::PrometheusQueryOutput,
        Self::SampleBlock,
        Self::PrometheusExposition,
        Self::MetricSeriesText,
        Self::Csv,
        Self::Json,
        Self::Log,
        Self::Configuration,
        Self::CommandOutput,
        Self::Attachment,
        Self::Simulation,
        Self::Unrecognised,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::OperatorObservation => "Operator observation",
            Self::MetricValues => "Structured metric values",
            Self::PrometheusQueryOutput => "Prometheus query output",
            Self::SampleBlock => "Kyumei sample block",
            Self::PrometheusExposition => "Prometheus exposition text",
            Self::MetricSeriesText => "Time series (text)",
            Self::Csv => "CSV",
            Self::Json => "JSON",
            Self::Log => "Log lines",
            Self::Configuration => "Configuration",
            Self::CommandOutput => "Command output",
            Self::Attachment => "Attachment",
            Self::Simulation => "Simulation",
            Self::Unrecognised => "Unrecognised",
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::OperatorObservation => "observation",
            Self::MetricValues => "metric-values",
            Self::PrometheusQueryOutput => "prometheus-json",
            Self::SampleBlock => "sample-block",
            Self::PrometheusExposition => "prometheus-text",
            Self::MetricSeriesText => "series-text",
            Self::Csv => "csv",
            Self::Json => "json",
            Self::Log => "log",
            Self::Configuration => "config",
            Self::CommandOutput => "command",
            Self::Attachment => "attachment",
            Self::Simulation => "simulation",
            Self::Unrecognised => "unrecognised",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.token() == token)
    }
}

/// Kyumei's reading of a submission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Interpretation {
    pub form: EvidenceForm,
    /// Which interpreter produced this, with its version, e.g. `prometheus-api/1`.
    pub interpreter: String,
    pub observations: Vec<Observation>,
    pub notes: Vec<InterpretationNote>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationNote {
    pub severity: NoteSeverity,
    pub text: String,
    pub observation: Option<ObservationId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteSeverity {
    Info,
    /// An assumption was made that the operator should check.
    Caution,
    /// Part of the submission could not be used.
    Problem,
}

/// An operator's correction to Kyumei's interpretation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretationCorrection {
    pub id: CorrectionId,
    pub evidence: EvidenceId,
    /// `None` applies the change to every observation of the evidence.
    pub observation: Option<ObservationId>,
    pub change: CorrectionChange,
    pub reason: String,
    pub at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum CorrectionChange {
    /// This observation is actually about `signal`.
    BindSignal { signal: SignalId },
    /// This observation should not inform any signal.
    Unbind,
    /// The values are in `unit`, not the unit Kyumei assumed.
    SetUnit { unit: Unit },
    /// The observation covers `window`.
    SetWindow { window: TimeWindow },
    /// Disregard this observation entirely.
    Exclude,
    /// Reinstate an excluded observation.
    Include,
}

impl CorrectionChange {
    pub fn describe(&self) -> String {
        match self {
            Self::BindSignal { signal } => format!("treat as signal `{signal}`"),
            Self::Unbind => "do not use for any signal".to_owned(),
            Self::SetUnit { unit } => format!("values are in {unit}"),
            Self::SetWindow { window } => format!("covers {}", window.display_utc()),
            Self::Exclude => "exclude from the investigation".to_owned(),
            Self::Include => "include again".to_owned(),
        }
    }
}

/// An observation as it stands after corrections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectiveObservation {
    pub observation: Observation,
    pub excluded: bool,
    /// Corrections that changed this observation, oldest first.
    pub corrections: Vec<CorrectionId>,
}

impl Evidence {
    /// The interpretation with `corrections` applied in order. The stored
    /// interpretation and the raw submission are untouched.
    pub fn effective_observations(
        &self,
        corrections: &[InterpretationCorrection],
    ) -> Vec<EffectiveObservation> {
        self.interpretation
            .observations
            .iter()
            .map(|observation| {
                let mut effective = EffectiveObservation {
                    observation: observation.clone(),
                    excluded: false,
                    corrections: Vec::new(),
                };
                for correction in corrections.iter().filter(|c| {
                    c.evidence == self.id
                        && c.observation
                            .as_ref()
                            .is_none_or(|target| target == &observation.id)
                }) {
                    apply_correction(&mut effective, correction);
                }
                effective
            })
            .collect()
    }

    pub fn observation(&self, id: &ObservationId) -> Option<&Observation> {
        self.interpretation
            .observations
            .iter()
            .find(|o| &o.id == id)
    }
}

fn apply_correction(effective: &mut EffectiveObservation, correction: &InterpretationCorrection) {
    let obs = &mut effective.observation;
    match &correction.change {
        CorrectionChange::BindSignal { signal } => {
            obs.binding = Some(SignalBinding {
                signal: signal.clone(),
                basis: BindingBasis::Correction {
                    correction: correction.id.clone(),
                },
                claim: obs
                    .binding
                    .as_ref()
                    .filter(|b| &b.signal == signal)
                    .and_then(|b| b.claim.clone()),
            });
        }
        CorrectionChange::Unbind => obs.binding = None,
        CorrectionChange::SetUnit { unit } => obs.value = obs.value.with_unit(*unit),
        CorrectionChange::SetWindow { window } => obs.window = Some(*window),
        CorrectionChange::Exclude => effective.excluded = true,
        CorrectionChange::Include => effective.excluded = false,
    }
    effective.corrections.push(correction.id.clone());
}
