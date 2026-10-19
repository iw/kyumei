//! The investigation core: commands in, events out.
//!
//! This is deterministic and free of I/O and clocks. The durable workflow
//! (kyumei-odori) and the in-process service both call it, so an
//! investigation behaves identically in tests, fixtures and production.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use jiff::Timestamp;
use kyumei_diagnostics::{Assessment, assess};
use kyumei_domain::{
    AssessmentTrigger, Attested, ClassificationBasis, ContextAmendment, CorrectionChange,
    DeclaredContext, EvidenceId, EvidenceOrigin, EvidenceRequestId, IncidentWindow,
    InterpretationCorrection, Investigation, InvestigationEvent, InvestigationId,
    InvestigationScope, ObservationId, Opened, SdkDescriptor, SdkLanguage, ServerVersion,
    Statistic, SubmissionChannel, Symptom, SymptomClassification, SymptomSignatureId,
    TemporalEnvironment, Topology, Unit,
};
use kyumei_evidence::{
    IngestEnv, StructuredValue, Submission, SubmissionBody, ingest, ingest_symptom,
};
use kyumei_knowledge::KnowledgeBase;
use serde::{Deserialize, Serialize};

/// The largest evidence submission accepted, in bytes.
pub const MAX_SUBMISSION_BYTES: usize = 8 * 1024 * 1024;

/// Everything needed to open an investigation. Unknown stays unknown.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenInvestigation {
    pub id: InvestigationId,
    pub title: Option<String>,
    pub description: String,
    /// A symptom signature the operator chose explicitly.
    pub signature: Option<SymptomSignatureId>,
    pub server_version: Option<String>,
    pub sdks: Vec<SdkInput>,
    pub topology: Option<Topology>,
    pub namespaces: Vec<String>,
    pub task_queues: Vec<String>,
    pub window_start: Option<Timestamp>,
    pub window_end: Option<Timestamp>,
    pub window_description: Option<String>,
    /// The environment profile the context came from.
    pub profile: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkInput {
    pub language: String,
    pub version: Option<String>,
}

/// A command against an open investigation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    SubmitEvidence(EvidenceSubmission),
    CorrectInterpretation(CorrectionInput),
    DeclineRequest {
        request: EvidenceRequestId,
        reason: String,
    },
    AmendContext(ContextInput),
    Reassess,
    /// A simulation the assessment said was due, run and ready to record.
    RecordSimulation(Box<crate::simulation::SimulationRun>),
}

impl Command {
    pub fn describe(&self) -> &'static str {
        match self {
            Self::SubmitEvidence(_) => "submit evidence",
            Self::CorrectInterpretation(_) => "correct an interpretation",
            Self::DeclineRequest { .. } => "mark a request unavailable",
            Self::AmendContext(_) => "amend the context",
            Self::Reassess => "reassess",
            Self::RecordSimulation(_) => "record a simulation",
        }
    }
}

/// A serialisable evidence submission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvidenceSubmission {
    pub channel: SubmissionChannel,
    pub text: Option<String>,
    /// Binary content, base64-encoded.
    pub bytes_base64: Option<String>,
    pub filename: Option<String>,
    pub media_type: Option<String>,
    pub declared: DeclaredContext,
    pub responds_to: Option<EvidenceRequestId>,
    pub structured: Option<StructuredInput>,
    pub origin: Option<EvidenceOrigin>,
    /// The content, already in the blob store, in place of `text` or
    /// `bytes_base64`. Large submissions travel this way so that durable
    /// workflow payloads stay small; see [`crate::blob::externalize`].
    #[serde(default)]
    pub stored: Option<StoredContent>,
}

/// Submission content held in the blob store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredContent {
    pub sha256: String,
    /// UTF-8 text (pasted, or a text file) rather than binary content.
    pub text: bool,
    pub size_bytes: u64,
}

impl EvidenceSubmission {
    pub fn text(channel: SubmissionChannel, text: impl Into<String>) -> Self {
        Self {
            channel,
            text: Some(text.into()),
            bytes_base64: None,
            filename: None,
            media_type: None,
            declared: DeclaredContext::default(),
            responds_to: None,
            structured: None,
            origin: None,
            stored: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StructuredInput {
    pub signal: String,
    pub statistic: Statistic,
    #[serde(with = "kyumei_domain::float")]
    pub value: f64,
    pub unit: Option<Unit>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorrectionInput {
    pub evidence: EvidenceId,
    pub observation: Option<ObservationId>,
    pub change: CorrectionChange,
    pub reason: String,
}

/// Context changes; `None` leaves a field unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextInput {
    pub server_version: Option<String>,
    pub sdks: Option<Vec<SdkInput>>,
    pub topology: Option<Topology>,
    pub namespaces: Option<Vec<String>>,
    pub task_queues: Option<Vec<String>>,
    pub window_start: Option<Timestamp>,
    pub window_end: Option<Timestamp>,
    pub signature: Option<SymptomSignatureId>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    Invalid(String),
    #[error("knowledge: {0}")]
    Knowledge(String),
    #[error("domain: {0}")]
    Domain(#[from] kyumei_domain::DomainError),
}

/// What a command produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub events: Vec<(Timestamp, InvestigationEvent)>,
    /// Binary evidence to store under its SHA-256.
    pub blobs: Vec<(String, Vec<u8>)>,
    /// The evidence created, if any.
    pub evidence: Option<EvidenceId>,
}

/// Open an investigation: record the symptom as evidence `E0`, assess, and
/// issue the first evidence request.
pub fn open(
    kb: &KnowledgeBase,
    cmd: &OpenInvestigation,
    at: Timestamp,
) -> Result<Investigation, CommandError> {
    if cmd.description.trim().is_empty() {
        return Err(CommandError::Invalid(
            "describe what you are seeing".to_owned(),
        ));
    }
    let environment = TemporalEnvironment {
        server_version: parse_version(cmd.server_version.as_deref())?
            .map(|v| Attested::asserted(v, at)),
        sdks: parse_sdks(&cmd.sdks, at),
        topology: cmd
            .topology
            .clone()
            .filter(|t| !t.is_empty())
            .map(|t| Attested::asserted(t, at)),
        profile: cmd
            .profile
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned),
    };
    let window = window(
        cmd.window_start,
        cmd.window_end,
        cmd.window_description.clone(),
    )?;
    let selection = kb
        .select(environment.server_version.as_ref().map(|v| &v.value))
        .ok_or_else(|| CommandError::Knowledge("no knowledge pack loaded".to_owned()))?;
    let pack = selection.pack;
    let classification = match &cmd.signature {
        Some(sig) => {
            pack.diagnostic_for(sig)
                .ok_or_else(|| CommandError::Invalid(format!("unknown symptom `{sig}`")))?;
            Some((
                SymptomClassification {
                    signature: sig.clone(),
                    basis: ClassificationBasis::OperatorSelected,
                },
                None,
            ))
        }
        None => pack.classify(&cmd.description).map(|(_, s, matched)| {
            (
                SymptomClassification {
                    signature: s.id.clone(),
                    basis: ClassificationBasis::PatternMatch {
                        pattern: matched.clone(),
                    },
                },
                Some(matched),
            )
        }),
    };
    let symptom_signal = classification
        .as_ref()
        .and_then(|(c, _)| pack.diagnostic_for(&c.signature))
        .and_then(|(d, s)| d.signal(&s.signal));
    let e0 = ingest_symptom(
        EvidenceId::new("E0"),
        at,
        &cmd.description,
        symptom_signal,
        classification.as_ref().and_then(|(_, m)| m.as_deref()),
    );
    let title = cmd
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| {
            let base = classification
                .as_ref()
                .and_then(|(c, _)| pack.diagnostic_for(&c.signature))
                .map_or_else(
                    || first_words(&cmd.description, 8),
                    |(_, s)| s.title.clone(),
                );
            match cmd.task_queues.first() {
                Some(tq) => format!("{base} on {tq}"),
                None => base,
            }
        });
    let opened = Opened {
        id: cmd.id.clone(),
        title,
        symptom: Symptom {
            description: cmd.description.clone(),
            classification: classification.map(|(c, _)| c),
            evidence: e0.id.clone(),
        },
        symptom_evidence: e0,
        environment,
        scope: InvestigationScope {
            namespaces: clean(&cmd.namespaces),
            task_queues: clean(&cmd.task_queues),
        },
        window,
    };
    let mut investigation = Investigation::open(at, opened)?;
    for (t, e) in follow_up(kb, &investigation, AssessmentTrigger::Opened, at, false)? {
        investigation.apply(t, e)?;
    }
    Ok(investigation)
}

/// Handle a command against an investigation. The investigation is not
/// modified; apply the returned events to advance it.
pub fn handle(
    kb: &KnowledgeBase,
    investigation: &Investigation,
    command: &Command,
    at: Timestamp,
) -> Result<Outcome, CommandError> {
    let mut scratch = investigation.clone();
    let mut events: Vec<(Timestamp, InvestigationEvent)> = Vec::new();
    let mut blobs = Vec::new();
    let mut evidence_id = None;
    let mut push =
        |scratch: &mut Investigation, e: InvestigationEvent| -> Result<(), CommandError> {
            scratch.apply(at, e.clone())?;
            events.push((at, e));
            Ok(())
        };

    let trigger = match command {
        Command::SubmitEvidence(sub) => {
            let request = match &sub.responds_to {
                Some(id) => Some(
                    investigation
                        .request(id)
                        .ok_or_else(|| CommandError::Invalid(format!("unknown request {id}")))?,
                ),
                None => None,
            };
            let body = match (&sub.text, &sub.bytes_base64) {
                (_, Some(b64)) => SubmissionBody::Bytes(STANDARD.decode(b64).map_err(|e| {
                    CommandError::Invalid(format!("attachment is not valid base64: {e}"))
                })?),
                (Some(text), None) => SubmissionBody::Text(text.clone()),
                (None, None) if sub.structured.is_some() => SubmissionBody::Text(String::new()),
                (None, None) => {
                    return Err(CommandError::Invalid("the submission is empty".to_owned()));
                }
            };
            let size = match &body {
                SubmissionBody::Text(t) => t.len(),
                SubmissionBody::Bytes(b) => b.len(),
            };
            if size > MAX_SUBMISSION_BYTES {
                return Err(CommandError::Invalid(format!(
                    "the submission is {size} bytes; the limit is {MAX_SUBMISSION_BYTES}"
                )));
            }
            let structured = sub.structured.as_ref().map(|s| StructuredValue {
                signal: s.signal.as_str().into(),
                statistic: s.statistic,
                value: s.value,
                unit: s.unit,
            });
            // Structured entries preserve exactly what was typed as the raw evidence.
            let body = match (&structured, body) {
                (Some(s), SubmissionBody::Text(t)) if t.trim().is_empty() => SubmissionBody::Text(
                    serde_json::to_string_pretty(&serde_json::json!({
                        "signal": s.signal, "statistic": s.statistic, "value": s.value, "unit": s.unit,
                        "window": sub.declared.window, "note": sub.declared.note,
                    }))
                    .unwrap_or_default(),
                ),
                (_, b) => b,
            };
            let (pack, _, diagnostic) = kyumei_diagnostics::select(kb, investigation)
                .map_err(|e| CommandError::Knowledge(e.to_string()))?;
            let families = investigation.environment.sdk_families();
            let id = investigation.next_evidence_id();
            let ingested = ingest(
                id.clone(),
                at,
                sub.origin.clone().unwrap_or(EvidenceOrigin::Operator {
                    channel: sub.channel,
                }),
                Submission {
                    channel: sub.channel,
                    body,
                    filename: sub.filename.clone(),
                    media_type: sub.media_type.clone(),
                    declared: sub.declared.clone(),
                    responds_to: sub.responds_to.clone(),
                    structured,
                },
                IngestEnv {
                    pack,
                    diagnostic,
                    request,
                    scope: &investigation.scope,
                    families: &families,
                },
            );
            if let Some(blob) = ingested.blob {
                blobs.push((ingested.evidence.raw.sha256.clone(), blob));
            }
            push(
                &mut scratch,
                InvestigationEvent::EvidenceSubmitted(Box::new(ingested.evidence)),
            )?;
            evidence_id = Some(id.clone());
            AssessmentTrigger::Evidence { evidence: id }
        }
        Command::CorrectInterpretation(c) => {
            if c.reason.trim().is_empty() {
                return Err(CommandError::Invalid(
                    "say why the interpretation is wrong".to_owned(),
                ));
            }
            if let CorrectionChange::BindSignal { signal } = &c.change {
                let (_, _, diagnostic) = kyumei_diagnostics::select(kb, investigation)
                    .map_err(|e| CommandError::Knowledge(e.to_string()))?;
                if diagnostic.and_then(|d| d.signal(signal)).is_none() {
                    return Err(CommandError::Invalid(format!(
                        "`{signal}` is not a signal of this diagnostic"
                    )));
                }
            }
            let id = investigation.next_correction_id();
            push(
                &mut scratch,
                InvestigationEvent::InterpretationCorrected(InterpretationCorrection {
                    id: id.clone(),
                    evidence: c.evidence.clone(),
                    observation: c.observation.clone(),
                    change: c.change.clone(),
                    reason: c.reason.trim().to_owned(),
                    at,
                }),
            )?;
            AssessmentTrigger::Correction { correction: id }
        }
        Command::DeclineRequest { request, reason } => {
            let reason = if reason.trim().is_empty() {
                "not available to the operator".to_owned()
            } else {
                reason.trim().to_owned()
            };
            push(
                &mut scratch,
                InvestigationEvent::RequestDeclined {
                    request: request.clone(),
                    reason,
                },
            )?;
            AssessmentTrigger::Declined {
                request: request.clone(),
            }
        }
        Command::AmendContext(c) => {
            let amendment = amendment(c, investigation, at)?;
            if let Some(c) = &amendment.classification
                && !kb
                    .packs()
                    .iter()
                    .any(|p| p.diagnostic_for(&c.signature).is_some())
            {
                return Err(CommandError::Invalid(format!(
                    "unknown symptom `{}`",
                    c.signature
                )));
            }
            if amendment.is_empty() {
                return Err(CommandError::Invalid("nothing changed".to_owned()));
            }
            push(&mut scratch, InvestigationEvent::ContextAmended(amendment))?;
            AssessmentTrigger::ContextAmended
        }
        Command::Reassess => AssessmentTrigger::Reassessed,
        Command::RecordSimulation(run) => {
            let (_, _, diagnostic) = kyumei_diagnostics::select(kb, investigation)
                .map_err(|e| CommandError::Knowledge(e.to_string()))?;
            let d = diagnostic.ok_or_else(|| {
                CommandError::Invalid("no diagnostic applies, so nothing was simulated".to_owned())
            })?;
            let id = investigation.next_evidence_id();
            let evidence = crate::simulation::evidence(d, id.clone(), at, run)
                .map_err(CommandError::Invalid)?;
            push(
                &mut scratch,
                InvestigationEvent::EvidenceSubmitted(Box::new(evidence)),
            )?;
            evidence_id = Some(id.clone());
            AssessmentTrigger::Evidence { evidence: id }
        }
    };

    let context_changed = matches!(command, Command::AmendContext(_));
    for (t, e) in follow_up(kb, &scratch, trigger, at, context_changed)? {
        push(&mut scratch, e)?;
        let _ = t;
    }
    Ok(Outcome {
        events,
        blobs,
        evidence: evidence_id,
    })
}

/// Assess, then settle requests and the conclusion.
fn follow_up(
    kb: &KnowledgeBase,
    investigation: &Investigation,
    trigger: AssessmentTrigger,
    at: Timestamp,
    context_changed: bool,
) -> Result<Vec<(Timestamp, InvestigationEvent)>, CommandError> {
    let assessment =
        assess(kb, investigation).map_err(|e| CommandError::Knowledge(e.to_string()))?;
    let mut events = Vec::new();
    let record = assessment.record(trigger, investigation.latest_assessment());
    events.push((at, InvestigationEvent::Assessed(record)));

    // Requests whose signals are now all informed are answered. A signal whose
    // queries found no series is informed as far as asking goes.
    for r in investigation.requests.iter().filter(|r| r.status.is_open()) {
        let informed = !r.expected_signals.is_empty()
            && r.expected_signals
                .iter()
                .all(|s| assessment.facts.get(s).is_some() || assessment.facts.is_absent(s))
            && (!r.responses.is_empty()
                || r.expected_signals.iter().all(|s| {
                    assessment
                        .facts
                        .get(s)
                        .is_some_and(|f| f.status == kyumei_domain::EpistemicStatus::ObservedFact)
                }));
        if informed {
            events.push((
                at,
                InvestigationEvent::RequestAnswered {
                    request: r.id.clone(),
                },
            ));
        }
    }
    let answered: Vec<&EvidenceRequestId> = events
        .iter()
        .filter_map(|(_, e)| match e {
            InvestigationEvent::RequestAnswered { request } => Some(request),
            _ => None,
        })
        .collect();

    // Conclude once no more specific diagnosis remains possible; further
    // evidence is then optional.
    let finalisable = assessment.conclusion.is_some() && assessment.conclusion_final;
    let current = investigation
        .current_request()
        .filter(|r| !answered.contains(&&r.id));

    // The next request, unless the diagnosis is final.
    let best = if finalisable {
        None
    } else {
        assessment.plans.first()
    };
    let stale = match current {
        Some(open) if context_changed => rendered_differently(kb, investigation, open, at)?,
        _ => false,
    };
    match (current, best) {
        (Some(open), Some(plan)) if open.definition == plan.definition && !stale => {}
        (current, Some(plan)) => {
            if let Some(open) = current {
                events.push((
                    at,
                    InvestigationEvent::RequestSuperseded {
                        request: open.id.clone(),
                    },
                ));
            }
            let (_, _, diagnostic) = kyumei_diagnostics::select(kb, investigation)
                .map_err(|e| CommandError::Knowledge(e.to_string()))?;
            if let Some(def) = diagnostic.and_then(|d| d.request(&plan.definition))
                && let Some(d) = diagnostic
            {
                let request = kyumei_diagnostics::select::render_request(
                    def,
                    d,
                    investigation,
                    investigation.next_request_id(),
                    at,
                );
                events.push((at, InvestigationEvent::EvidenceRequested(request)));
            }
        }
        (Some(open), None) => {
            events.push((
                at,
                InvestigationEvent::RequestSuperseded {
                    request: open.id.clone(),
                },
            ));
        }
        (None, None) => {}
    }

    // Conclusion changes.
    let new_id = assessment.conclusion.as_ref().map(|c| &c.id);
    let old_id = investigation.conclusion.as_ref().map(|c| &c.id);
    match (&investigation.conclusion, &assessment.conclusion) {
        (Some(_), None) => events.push((
            at,
            InvestigationEvent::ConclusionWithdrawn {
                reason: "the evidence no longer supports it".to_owned(),
            },
        )),
        (Some(_), Some(new)) if old_id != new_id => {
            events.push((
                at,
                InvestigationEvent::ConclusionWithdrawn {
                    reason: format!("superseded by “{}”", new.title),
                },
            ));
            events.push((at, InvestigationEvent::Concluded(Box::new(new.clone()))));
        }
        (None, Some(new)) if finalisable => {
            events.push((at, InvestigationEvent::Concluded(Box::new(new.clone()))));
        }
        _ => {}
    }
    Ok(events)
}

/// Whether `request`, rendered again for the investigation as it now is,
/// would read differently. A context change can fill a placeholder, move the
/// window, rename the Task Queue or select another SDK's query.
fn rendered_differently(
    kb: &KnowledgeBase,
    investigation: &Investigation,
    request: &kyumei_domain::EvidenceRequest,
    at: Timestamp,
) -> Result<bool, CommandError> {
    let (_, _, diagnostic) = kyumei_diagnostics::select(kb, investigation)
        .map_err(|e| CommandError::Knowledge(e.to_string()))?;
    let Some((d, def)) = diagnostic.and_then(|d| d.request(&request.definition).map(|r| (d, r)))
    else {
        return Ok(false);
    };
    let fresh =
        kyumei_diagnostics::select::render_request(def, d, investigation, request.id.clone(), at);
    Ok(fresh.why != request.why
        || fresh.collect != request.collect
        || fresh.window != request.window)
}

fn amendment(
    c: &ContextInput,
    investigation: &Investigation,
    at: Timestamp,
) -> Result<ContextAmendment, CommandError> {
    let mut a = ContextAmendment::default();
    if let Some(v) = parse_version(c.server_version.as_deref())?
        && investigation
            .environment
            .server_version
            .as_ref()
            .map(|x| &x.value)
            != Some(&v)
    {
        a.server_version = Some(Attested::asserted(v, at));
    }
    if let Some(sdks) = &c.sdks {
        let parsed = parse_sdks(sdks, at);
        let same = parsed.len() == investigation.environment.sdks.len()
            && parsed
                .iter()
                .zip(&investigation.environment.sdks)
                .all(|(a, b)| a.value == b.value);
        if !same {
            a.sdks = Some(parsed);
        }
    }
    if let Some(t) = &c.topology
        && investigation
            .environment
            .topology
            .as_ref()
            .map(|x| &x.value)
            != Some(t)
    {
        a.topology = Some(Attested::asserted(t.clone(), at));
    }
    let namespaces = c.namespaces.as_ref().map(|v| clean(v));
    let task_queues = c.task_queues.as_ref().map(|v| clean(v));
    if namespaces
        .as_ref()
        .is_some_and(|n| n != &investigation.scope.namespaces)
        || task_queues
            .as_ref()
            .is_some_and(|t| t != &investigation.scope.task_queues)
    {
        a.scope = Some(InvestigationScope {
            namespaces: namespaces.unwrap_or_else(|| investigation.scope.namespaces.clone()),
            task_queues: task_queues.unwrap_or_else(|| investigation.scope.task_queues.clone()),
        });
    }
    if c.window_start.is_some() || c.window_end.is_some() {
        let w = window(
            c.window_start.or(investigation.window.start),
            c.window_end.or(investigation.window.end),
            investigation.window.description.clone(),
        )?;
        if w != investigation.window {
            a.window = Some(w);
        }
    }
    if let Some(sig) = &c.signature
        && investigation
            .symptom
            .classification
            .as_ref()
            .map(|x| &x.signature)
            != Some(sig)
    {
        a.classification = Some(SymptomClassification {
            signature: sig.clone(),
            basis: ClassificationBasis::OperatorSelected,
        });
    }
    Ok(a)
}

fn parse_version(v: Option<&str>) -> Result<Option<ServerVersion>, CommandError> {
    match v.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => v
            .parse::<ServerVersion>()
            .map(Some)
            .map_err(CommandError::Invalid),
    }
}

fn parse_sdks(sdks: &[SdkInput], at: Timestamp) -> Vec<Attested<SdkDescriptor>> {
    sdks.iter()
        .filter_map(|s| {
            s.language.parse::<SdkLanguage>().ok().map(|language| {
                Attested::asserted(
                    SdkDescriptor {
                        language,
                        version: s.version.clone().filter(|v| !v.trim().is_empty()),
                    },
                    at,
                )
            })
        })
        .collect()
}

fn window(
    start: Option<Timestamp>,
    end: Option<Timestamp>,
    description: Option<String>,
) -> Result<IncidentWindow, CommandError> {
    if let (Some(s), Some(e)) = (start, end)
        && e < s
    {
        return Err(CommandError::Invalid(
            "the incident window ends before it starts".to_owned(),
        ));
    }
    Ok(IncidentWindow {
        start,
        end,
        description: description.filter(|d| !d.trim().is_empty()),
    })
}

fn clean(values: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for v in values.iter().map(|v| v.trim()).filter(|v| !v.is_empty()) {
        if !out.iter().any(|o| o == v) {
            out.push(v.to_owned());
        }
    }
    out
}

fn first_words(text: &str, n: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().take(n).collect();
    let mut s = words.join(" ");
    if text.split_whitespace().count() > n {
        s.push('…');
    }
    s
}

/// The current assessment of an investigation.
pub fn current_assessment(
    kb: &KnowledgeBase,
    investigation: &Investigation,
) -> Result<Assessment, CommandError> {
    assess(kb, investigation).map_err(|e| CommandError::Knowledge(e.to_string()))
}

/// Apply an outcome's events to an investigation.
pub fn apply(investigation: &mut Investigation, outcome: &Outcome) -> Result<(), CommandError> {
    for (at, event) in &outcome.events {
        investigation.apply(*at, event.clone())?;
    }
    Ok(())
}
