//! The investigation aggregate.
//!
//! An investigation is an append-only log of events. Its state is a fold over
//! that log, so the same log always produces the same investigation, whether
//! it is replayed from durable workflow history, a fixture or an export.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    DomainError,
    conclusion::Conclusion,
    environment::{
        InvestigationScope, SdkDescriptor, ServerVersion, TemporalEnvironment, Topology,
    },
    epistemic::Attested,
    evidence::{EffectiveObservation, Evidence, EvidenceForm, InterpretationCorrection},
    hypothesis::{HypothesisState, HypothesisTransition, RecordedState},
    ids::{
        ConclusionId, CorrectionId, EvidenceId, EvidenceRequestId, FindingId, HypothesisId,
        InvestigationId, PackId, RequestDefinitionId,
    },
    request::{EvidenceRequest, RequestStatus},
    symptom::{Symptom, SymptomClassification},
    time::IncidentWindow,
};

/// One event in an investigation's history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InvestigationEvent {
    Opened(Box<Opened>),
    ContextAmended(ContextAmendment),
    EvidenceRequested(EvidenceRequest),
    EvidenceSubmitted(Box<Evidence>),
    InterpretationCorrected(InterpretationCorrection),
    RequestDeclined {
        request: EvidenceRequestId,
        reason: String,
    },
    RequestSuperseded {
        request: EvidenceRequestId,
    },
    /// Every signal a request asked for is now informed.
    RequestAnswered {
        request: EvidenceRequestId,
    },
    Assessed(AssessmentRecord),
    ExplanationRecorded(ExplanationRecord),
    Concluded(Box<Conclusion>),
    ConclusionWithdrawn {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Opened {
    pub id: InvestigationId,
    pub title: String,
    pub symptom: Symptom,
    /// `E0`: the symptom description as an operator observation.
    pub symptom_evidence: Evidence,
    pub environment: TemporalEnvironment,
    pub scope: InvestigationScope,
    pub window: IncidentWindow,
}

/// A change to the investigation's context. `None` fields are unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextAmendment {
    pub server_version: Option<Attested<ServerVersion>>,
    pub sdks: Option<Vec<Attested<SdkDescriptor>>>,
    pub topology: Option<Attested<Topology>>,
    pub scope: Option<InvestigationScope>,
    pub window: Option<IncidentWindow>,
    pub classification: Option<SymptomClassification>,
}

impl ContextAmendment {
    pub fn is_empty(&self) -> bool {
        self == &ContextAmendment::default()
    }

    pub fn changed_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.server_version.is_some() {
            fields.push("server version");
        }
        if self.sdks.is_some() {
            fields.push("SDKs");
        }
        if self.topology.is_some() {
            fields.push("topology");
        }
        if self.scope.is_some() {
            fields.push("scope");
        }
        if self.window.is_some() {
            fields.push("incident window");
        }
        if self.classification.is_some() {
            fields.push("symptom classification");
        }
        fields
    }
}

/// The knowledge pack an assessment used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackRef {
    pub id: PackId,
    pub version: String,
    /// SHA-256 over the pack's files, so a later reader can tell whether the
    /// knowledge changed since this assessment.
    pub digest: String,
}

/// What prompted an assessment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "trigger", rename_all = "snake_case")]
pub enum AssessmentTrigger {
    Opened,
    Evidence { evidence: EvidenceId },
    Correction { correction: CorrectionId },
    Declined { request: EvidenceRequestId },
    ContextAmended,
    Reassessed,
}

/// A durable record of what the diagnostic engine concluded at a point in
/// time. The trail is built from these records, so it keeps saying what
/// Kyumei believed then even if knowledge later changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssessmentRecord {
    pub trigger: AssessmentTrigger,
    pub pack: PackRef,
    pub engine: String,
    pub states: BTreeMap<HypothesisId, RecordedState>,
    pub transitions: Vec<HypothesisTransition>,
    pub findings: Vec<FindingId>,
    pub next_request: Option<RequestDefinitionId>,
    pub conclusion: Option<ConclusionId>,
}

/// A record that an explanation was produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplanationRecord {
    pub author: ExplanationAuthor,
    /// SHA-256 of the serialised explanation.
    pub digest: String,
    /// The event sequence number of the assessment it explains.
    pub assessment_seq: u64,
    /// Why a model explanation was not used, when it was not.
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "author", rename_all = "snake_case")]
pub enum ExplanationAuthor {
    Deterministic {
        engine: String,
    },
    Model {
        provider: String,
        model: Option<String>,
        run: String,
    },
}

impl ExplanationAuthor {
    pub fn describe(&self) -> String {
        match self {
            Self::Deterministic { engine } => format!("deterministic ({engine})"),
            Self::Model {
                provider, model, ..
            } => match model {
                Some(model) => format!("model ({provider}, {model})"),
                None => format!("model ({provider})"),
            },
        }
    }
}

/// An event with its position and time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedEvent {
    pub seq: u64,
    pub at: Timestamp,
    pub event: InvestigationEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvestigationStatus {
    Open,
    Concluded,
}

impl InvestigationStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Concluded => "diagnosis reached",
        }
    }
}

/// The investigation, folded from its events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Investigation {
    pub id: InvestigationId,
    pub title: String,
    pub opened_at: Timestamp,
    pub symptom: Symptom,
    pub environment: TemporalEnvironment,
    pub scope: InvestigationScope,
    pub window: IncidentWindow,
    pub evidence: Vec<Evidence>,
    pub corrections: Vec<InterpretationCorrection>,
    pub requests: Vec<EvidenceRequest>,
    /// Assessments with the sequence number of the event that recorded them.
    pub assessments: Vec<(u64, AssessmentRecord)>,
    pub explanations: Vec<(u64, ExplanationRecord)>,
    pub conclusion: Option<Conclusion>,
    pub status: InvestigationStatus,
    pub events: Vec<RecordedEvent>,
}

impl Investigation {
    /// Start an investigation from its `Opened` event.
    pub fn open(at: Timestamp, opened: Opened) -> Result<Self, DomainError> {
        if opened.symptom.evidence != opened.symptom_evidence.id {
            return Err(DomainError::Invariant(
                "the symptom must reference its own evidence record".to_owned(),
            ));
        }
        let event = RecordedEvent {
            seq: 0,
            at,
            event: InvestigationEvent::Opened(Box::new(opened.clone())),
        };
        Ok(Self {
            id: opened.id,
            title: opened.title,
            opened_at: at,
            symptom: opened.symptom,
            environment: opened.environment,
            scope: opened.scope,
            window: opened.window,
            evidence: vec![opened.symptom_evidence],
            corrections: Vec::new(),
            requests: Vec::new(),
            assessments: Vec::new(),
            explanations: Vec::new(),
            conclusion: None,
            status: InvestigationStatus::Open,
            events: vec![event],
        })
    }

    /// Rebuild an investigation from its complete event log.
    pub fn replay(events: &[RecordedEvent]) -> Result<Self, DomainError> {
        let (first, rest) = events.split_first().ok_or(DomainError::EmptyLog)?;
        let InvestigationEvent::Opened(opened) = &first.event else {
            return Err(DomainError::Invariant(
                "an investigation log must begin with `opened`".to_owned(),
            ));
        };
        let mut investigation = Self::open(first.at, (**opened).clone())?;
        for event in rest {
            investigation.apply(event.at, event.event.clone())?;
        }
        Ok(investigation)
    }

    /// Validate and append one event.
    pub fn apply(&mut self, at: Timestamp, event: InvestigationEvent) -> Result<u64, DomainError> {
        let seq = self.events.len() as u64;
        match &event {
            InvestigationEvent::Opened(_) => {
                return Err(DomainError::Invariant(
                    "an investigation can only be opened once".to_owned(),
                ));
            }
            InvestigationEvent::ContextAmended(amendment) => {
                if amendment.is_empty() {
                    return Err(DomainError::Invariant("empty context amendment".to_owned()));
                }
                let a = amendment.clone();
                if let Some(v) = a.server_version {
                    self.environment.server_version = Some(v);
                }
                if let Some(sdks) = a.sdks {
                    self.environment.sdks = sdks;
                }
                if let Some(t) = a.topology {
                    self.environment.topology = Some(t);
                }
                if let Some(scope) = a.scope {
                    self.scope = scope;
                }
                if let Some(window) = a.window {
                    self.window = window;
                }
                if let Some(classification) = a.classification {
                    self.symptom.classification = Some(classification);
                }
            }
            InvestigationEvent::EvidenceRequested(request) => {
                if self.request(&request.id).is_some() {
                    return Err(DomainError::Duplicate(format!("request {}", request.id)));
                }
                if !request.status.is_open() {
                    return Err(DomainError::Invariant(format!(
                        "request {} must be issued open",
                        request.id
                    )));
                }
                self.requests.push(request.clone());
            }
            InvestigationEvent::EvidenceSubmitted(evidence) => {
                if self.evidence(&evidence.id).is_some() {
                    return Err(DomainError::Duplicate(format!("evidence {}", evidence.id)));
                }
                for obs in &evidence.interpretation.observations {
                    if obs.evidence != evidence.id {
                        return Err(DomainError::Invariant(format!(
                            "observation {} does not belong to evidence {}",
                            obs.id, evidence.id
                        )));
                    }
                }
                if let Some(request_id) = &evidence.responds_to {
                    let request = self
                        .requests
                        .iter_mut()
                        .find(|r| &r.id == request_id)
                        .ok_or_else(|| DomainError::Unknown(format!("request {request_id}")))?;
                    request.responses.push(evidence.id.clone());
                    if let RequestStatus::Answered { evidence: previous } = &mut request.status {
                        previous.push(evidence.id.clone());
                    }
                }
                self.evidence.push((**evidence).clone());
            }
            InvestigationEvent::InterpretationCorrected(correction) => {
                if self.corrections.iter().any(|c| c.id == correction.id) {
                    return Err(DomainError::Duplicate(format!(
                        "correction {}",
                        correction.id
                    )));
                }
                let evidence = self.evidence(&correction.evidence).ok_or_else(|| {
                    DomainError::Unknown(format!("evidence {}", correction.evidence))
                })?;
                if let Some(obs) = &correction.observation
                    && evidence.observation(obs).is_none()
                {
                    return Err(DomainError::Unknown(format!("observation {obs}")));
                }
                self.corrections.push(correction.clone());
            }
            InvestigationEvent::RequestDeclined { request, reason } => {
                let r = self
                    .requests
                    .iter_mut()
                    .find(|r| &r.id == request)
                    .ok_or_else(|| DomainError::Unknown(format!("request {request}")))?;
                if !matches!(r.status, RequestStatus::Open | RequestStatus::Superseded) {
                    return Err(DomainError::Invariant(format!(
                        "request {request} is {} and cannot be declined",
                        r.status.label()
                    )));
                }
                r.status = RequestStatus::Declined {
                    reason: reason.clone(),
                };
            }
            InvestigationEvent::RequestSuperseded { request } => {
                let r = self
                    .requests
                    .iter_mut()
                    .find(|r| &r.id == request)
                    .ok_or_else(|| DomainError::Unknown(format!("request {request}")))?;
                if r.status.is_open() {
                    r.status = RequestStatus::Superseded;
                }
            }
            InvestigationEvent::RequestAnswered { request } => {
                let r = self
                    .requests
                    .iter_mut()
                    .find(|r| &r.id == request)
                    .ok_or_else(|| DomainError::Unknown(format!("request {request}")))?;
                if !matches!(r.status, RequestStatus::Open | RequestStatus::Superseded) {
                    return Err(DomainError::Invariant(format!(
                        "request {request} is {} and cannot be answered",
                        r.status.label()
                    )));
                }
                r.status = RequestStatus::Answered {
                    evidence: r.responses.clone(),
                };
            }
            InvestigationEvent::Assessed(record) => {
                self.assessments.push((seq, record.clone()));
            }
            InvestigationEvent::ExplanationRecorded(record) => {
                self.explanations.push((seq, record.clone()));
            }
            InvestigationEvent::Concluded(conclusion) => {
                self.conclusion = Some((**conclusion).clone());
                self.status = InvestigationStatus::Concluded;
            }
            InvestigationEvent::ConclusionWithdrawn { .. } => {
                if self.conclusion.is_none() {
                    return Err(DomainError::Invariant(
                        "no conclusion to withdraw".to_owned(),
                    ));
                }
                self.conclusion = None;
                self.status = InvestigationStatus::Open;
            }
        }
        self.events.push(RecordedEvent { seq, at, event });
        Ok(seq)
    }

    pub fn evidence(&self, id: &EvidenceId) -> Option<&Evidence> {
        self.evidence.iter().find(|e| &e.id == id)
    }

    pub fn request(&self, id: &EvidenceRequestId) -> Option<&EvidenceRequest> {
        self.requests.iter().find(|r| &r.id == id)
    }

    /// The most recently issued request that is still open.
    pub fn current_request(&self) -> Option<&EvidenceRequest> {
        self.requests.iter().rev().find(|r| r.status.is_open())
    }

    pub fn latest_assessment(&self) -> Option<&AssessmentRecord> {
        self.assessments.last().map(|(_, a)| a)
    }

    /// Every observation with operator corrections applied.
    pub fn effective_observations(&self) -> Vec<EffectiveObservation> {
        self.evidence
            .iter()
            .flat_map(|e| e.effective_observations(&self.corrections))
            .collect()
    }

    pub fn next_evidence_id(&self) -> EvidenceId {
        EvidenceId::new(format!("E{}", self.evidence.len()))
    }

    pub fn next_request_id(&self) -> EvidenceRequestId {
        EvidenceRequestId::new(format!("R{}", self.requests.len() + 1))
    }

    pub fn next_correction_id(&self) -> CorrectionId {
        CorrectionId::new(format!("C{}", self.corrections.len() + 1))
    }

    pub fn last_event_at(&self) -> Timestamp {
        self.events.last().map_or(self.opened_at, |e| e.at)
    }

    /// The compact audit trail.
    pub fn trail(&self) -> Vec<TrailEntry> {
        let mut entries = Vec::new();
        let mut seen_hypotheses: BTreeMap<HypothesisId, HypothesisState> = BTreeMap::new();
        for recorded in &self.events {
            let mut push = |item: TrailItem| {
                entries.push(TrailEntry {
                    seq: recorded.seq,
                    at: recorded.at,
                    item,
                })
            };
            match &recorded.event {
                InvestigationEvent::Opened(opened) => push(TrailItem::SymptomRecorded {
                    evidence: opened.symptom_evidence.id.clone(),
                }),
                InvestigationEvent::ContextAmended(amendment) => push(TrailItem::ContextAmended {
                    fields: amendment
                        .changed_fields()
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                }),
                InvestigationEvent::EvidenceRequested(request) => {
                    push(TrailItem::EvidenceRequested {
                        request: request.id.clone(),
                        definition: request.definition.clone(),
                        title: request.title.clone(),
                    })
                }
                InvestigationEvent::EvidenceSubmitted(evidence) => {
                    push(TrailItem::EvidenceSupplied {
                        evidence: evidence.id.clone(),
                        form: evidence.interpretation.form,
                        subject: evidence_subject(evidence),
                        responds_to: evidence.responds_to.clone(),
                    })
                }
                InvestigationEvent::InterpretationCorrected(c) => {
                    push(TrailItem::InterpretationCorrected {
                        evidence: c.evidence.clone(),
                        correction: c.id.clone(),
                        change: c.change.describe(),
                    })
                }
                InvestigationEvent::RequestDeclined { request, reason } => {
                    push(TrailItem::RequestDeclined {
                        request: request.clone(),
                        reason: reason.clone(),
                    })
                }
                InvestigationEvent::RequestSuperseded { request } => {
                    push(TrailItem::RequestSuperseded {
                        request: request.clone(),
                    })
                }
                InvestigationEvent::RequestAnswered { request } => {
                    push(TrailItem::RequestAnswered {
                        request: request.clone(),
                    })
                }
                InvestigationEvent::Assessed(record) => {
                    for t in &record.transitions {
                        let item = match (t.from, seen_hypotheses.get(&t.hypothesis)) {
                            (None, None) => TrailItem::HypothesisConsidered {
                                hypothesis: t.hypothesis.clone(),
                                state: t.to,
                            },
                            _ => TrailItem::HypothesisChanged {
                                hypothesis: t.hypothesis.clone(),
                                from: t.from,
                                to: t.to,
                                because: t.because.clone(),
                            },
                        };
                        seen_hypotheses.insert(t.hypothesis.clone(), t.to);
                        push(item);
                    }
                }
                InvestigationEvent::ExplanationRecorded(record) => {
                    push(TrailItem::ExplanationRecorded {
                        author: record.author.describe(),
                    })
                }
                InvestigationEvent::Concluded(conclusion) => push(TrailItem::Concluded {
                    conclusion: conclusion.id.clone(),
                    title: conclusion.title.clone(),
                }),
                InvestigationEvent::ConclusionWithdrawn { reason } => {
                    push(TrailItem::ConclusionWithdrawn {
                        reason: reason.clone(),
                    })
                }
            }
        }
        entries
    }
}

/// The most specific subject of a piece of evidence, for trails.
fn evidence_subject(evidence: &Evidence) -> String {
    let subjects: Vec<String> = evidence
        .interpretation
        .observations
        .iter()
        .filter_map(|o| {
            o.subject
                .metric
                .clone()
                .or_else(|| o.subject.description.clone())
        })
        .collect();
    let mut unique: Vec<String> = Vec::new();
    for s in subjects {
        if !unique.contains(&s) {
            unique.push(s);
        }
    }
    match unique.len() {
        0 => evidence.interpretation.form.label().to_owned(),
        1 => unique.remove(0),
        n => format!("{} and {} more", unique[0], n - 1),
    }
}

/// One line of the investigation trail.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrailEntry {
    pub seq: u64,
    pub at: Timestamp,
    pub item: TrailItem,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrailItem {
    SymptomRecorded {
        evidence: EvidenceId,
    },
    ContextAmended {
        fields: Vec<String>,
    },
    EvidenceRequested {
        request: EvidenceRequestId,
        definition: RequestDefinitionId,
        title: String,
    },
    EvidenceSupplied {
        evidence: EvidenceId,
        form: EvidenceForm,
        subject: String,
        responds_to: Option<EvidenceRequestId>,
    },
    InterpretationCorrected {
        evidence: EvidenceId,
        correction: CorrectionId,
        change: String,
    },
    RequestDeclined {
        request: EvidenceRequestId,
        reason: String,
    },
    RequestSuperseded {
        request: EvidenceRequestId,
    },
    RequestAnswered {
        request: EvidenceRequestId,
    },
    HypothesisConsidered {
        hypothesis: HypothesisId,
        state: HypothesisState,
    },
    HypothesisChanged {
        hypothesis: HypothesisId,
        from: Option<HypothesisState>,
        to: HypothesisState,
        because: Vec<FindingId>,
    },
    ExplanationRecorded {
        author: String,
    },
    Concluded {
        conclusion: ConclusionId,
        title: String,
    },
    ConclusionWithdrawn {
        reason: String,
    },
}
