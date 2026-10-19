//! Evidence ingestion and normalisation.
//!
//! Every submission becomes an [`Evidence`] record holding the raw bytes (or a
//! reference to them), what the operator declared, and Kyumei's reading of it:
//! observations bound to diagnostic signals where that can be justified.
//! Normalisation never destroys the original submission.
//!
//! Manual submission and a future `kyumei collect` both call [`ingest`]; only
//! the [`EvidenceOrigin`] differs.

mod attachment;
mod bind;
mod config;
mod fields;
mod prometheus;
mod tabular;
pub mod text;
pub mod timestamps;

use std::collections::BTreeMap;

use jiff::Timestamp;
use kyumei_domain::{
    AbsentReadsZero, BindingBasis, DeclaredContext, EpistemicStatus, Evidence, EvidenceForm,
    EvidenceId, EvidenceOrigin, EvidenceRequest, EvidenceRequestId, Interpretation,
    InterpretationNote, InvestigationScope, NoteSeverity, Observation, ObservationId,
    ObservedValue, RawBody, RawEvidence, SdkFamily, SeriesPoint, SignalBinding, SignalId,
    Statistic, Subject, SubmissionChannel, TimeWindow, Unit,
};
use kyumei_knowledge::{DiagnosticDef, Pack, SignalDef, Transform};
use sha2::{Digest, Sha256};

use crate::{bind::Resolution, prometheus::ParsedSeries};

/// Text above this size is stored by reference rather than inline. Every
/// durable activity receives the whole event log, so inline text is kept
/// small.
pub const MAX_INLINE_BYTES: usize = 64 * 1024;

/// Series longer than this are reduced to per-bucket extremes.
pub const MAX_POINTS: usize = 2_000;

/// Interpretation stops after this many observations.
pub const MAX_OBSERVATIONS: usize = 200;

/// How a Kyumei sample block's first line starts.
const SAMPLE_BLOCK_MARKER: &str = "-----BEGIN KYUMEI SAMPLES";

/// What the operator (or a collector) submitted.
#[derive(Clone, Debug)]
pub struct Submission {
    pub channel: SubmissionChannel,
    pub body: SubmissionBody,
    pub filename: Option<String>,
    pub media_type: Option<String>,
    pub declared: DeclaredContext,
    pub responds_to: Option<EvidenceRequestId>,
    /// Values entered through the structured form.
    pub structured: Option<StructuredValue>,
}

#[derive(Clone, Debug)]
pub enum SubmissionBody {
    Text(String),
    Bytes(Vec<u8>),
}

/// One value entered through the structured form.
#[derive(Clone, Debug, PartialEq)]
pub struct StructuredValue {
    pub signal: SignalId,
    pub statistic: Statistic,
    pub value: f64,
    pub unit: Option<Unit>,
}

/// The investigation context ingestion needs.
#[derive(Clone, Copy, Debug)]
pub struct IngestEnv<'a> {
    pub pack: &'a Pack,
    pub diagnostic: Option<&'a DiagnosticDef>,
    pub request: Option<&'a EvidenceRequest>,
    pub scope: &'a InvestigationScope,
    /// SDK implementations in use; empty when unknown.
    pub families: &'a [SdkFamily],
}

/// Internal: the environment plus this submission's declarations.
#[derive(Clone, Copy, Debug)]
pub(crate) struct IngestContext<'a> {
    pub pack: &'a Pack,
    pub diagnostic: Option<&'a DiagnosticDef>,
    pub request: Option<&'a EvidenceRequest>,
    pub scope: &'a InvestigationScope,
    pub families: &'a [SdkFamily],
    pub declared: &'a DeclaredContext,
}

/// The result of ingestion: the evidence record and, for binary submissions,
/// the bytes the caller must store under `raw.sha256`.
#[derive(Clone, Debug)]
pub struct Ingested {
    pub evidence: Evidence,
    pub blob: Option<Vec<u8>>,
}

/// Ingest one submission.
pub fn ingest(
    id: EvidenceId,
    received_at: Timestamp,
    origin: EvidenceOrigin,
    submission: Submission,
    env: IngestEnv<'_>,
) -> Ingested {
    let ctx = IngestContext {
        pack: env.pack,
        diagnostic: env.diagnostic,
        request: env.request,
        scope: env.scope,
        families: env.families,
        declared: &submission.declared,
    };
    let (raw, text, blob) = raw_evidence(&submission);
    let mut builder = Builder::new(&id, &ctx, received_at, submission.channel);

    let form = submission
        .declared
        .form
        .unwrap_or_else(|| detect(&submission, text.as_deref(), blob.as_deref()));

    match (form, text.as_deref()) {
        (EvidenceForm::MetricValues, _) => builder.structured(submission.structured.as_ref()),
        (EvidenceForm::OperatorObservation, Some(t)) => builder.observation_text(t),
        (EvidenceForm::PrometheusQueryOutput, Some(t)) => builder.prometheus_json(t),
        (EvidenceForm::SampleBlock, Some(t)) => builder.sample_blocks(t),
        (EvidenceForm::PrometheusExposition, Some(t)) => {
            let series = prometheus::parse_exposition(t, ctx.declared.window.map_or(received_at, |w| w.end));
            if ctx.declared.window.is_none() {
                builder.note(
                    NoteSeverity::Caution,
                    "Exposition samples carry no time; they are placed at submission time. State the window if they were scraped earlier.",
                    None,
                );
            }
            builder.series(series);
        }
        (EvidenceForm::MetricSeriesText, Some(t)) => builder.series(tabular::parse_series_text(t)),
        (EvidenceForm::Csv, Some(t)) => match tabular::parse_csv(t) {
            Ok(series) => builder.series(series),
            Err(e) => builder.note(NoteSeverity::Problem, format!("CSV not understood: {e}"), None),
        },
        (EvidenceForm::Json | EvidenceForm::CommandOutput, Some(t)) => builder.structured_output(t),
        (EvidenceForm::Configuration, Some(t)) => builder.configuration(t),
        (EvidenceForm::Log, Some(t)) => builder.log(t),
        (EvidenceForm::Attachment, _) => builder.attachment(&raw, blob.as_deref()),
        (_, None) => builder.note(
            NoteSeverity::Problem,
            "Binary content is only understood as an attachment.",
            None,
        ),
        (EvidenceForm::Simulation, Some(_)) => builder.note(
            NoteSeverity::Problem,
            "Kyumei records simulations itself; a submission is not read as one.",
            None,
        ),
        (EvidenceForm::Unrecognised, Some(_)) => builder.note(
            NoteSeverity::Caution,
            "Kyumei did not recognise this format. It is preserved as submitted; declare its form or add an observation describing it.",
            None,
        ),
    }

    let interpretation = builder.finish(form);
    Ingested {
        evidence: Evidence {
            id,
            submitted_at: received_at,
            origin,
            responds_to: submission.responds_to.clone(),
            raw,
            declared: submission.declared.clone(),
            interpretation,
            simulation: None,
        },
        blob,
    }
}

/// Record the operator's symptom description as evidence `E0`.
///
/// Durations in the description ("~30 seconds") become operator assertions
/// about the symptom's signal.
pub fn ingest_symptom(
    id: EvidenceId,
    received_at: Timestamp,
    description: &str,
    signal: Option<&SignalDef>,
    pattern: Option<&str>,
) -> Evidence {
    let text = description.to_owned();
    let mut observations = vec![Observation {
        id: ObservationId::new(format!("{id}.O1")),
        evidence: id.clone(),
        status: EpistemicStatus::OperatorAssertion,
        subject: Subject::described("Symptom description"),
        binding: None,
        window: None,
        value: ObservedValue::Text { text: text.clone() },
    }];
    let mut notes = Vec::new();
    if let Some(signal) = signal.filter(|s| s.unit.is_duration()) {
        let mentions = text::duration_mentions(description);
        // The largest duration mentioned is the symptom's magnitude; smaller
        // ones are usually baselines ("normally under 50ms").
        if let Some(largest) = mentions
            .iter()
            .max_by(|a, b| a.seconds.total_cmp(&b.seconds))
        {
            let oid = ObservationId::new(format!("{id}.O2"));
            observations.push(Observation {
                id: oid.clone(),
                evidence: id.clone(),
                status: EpistemicStatus::OperatorAssertion,
                subject: Subject::described(format!(
                    "“{}” in the symptom description",
                    largest.text
                )),
                binding: Some(SignalBinding {
                    signal: signal.id.clone(),
                    basis: BindingBasis::SymptomText {
                        matched: pattern.unwrap_or(&largest.text).to_owned(),
                    },
                    claim: None,
                }),
                window: None,
                value: ObservedValue::Scalar {
                    unit: Some(Unit::Seconds),
                    value: Unit::Seconds
                        .convert(largest.seconds, signal.unit)
                        .unwrap_or(largest.seconds),
                    statistic: Statistic::Typical,
                },
            });
            notes.push(InterpretationNote {
                severity: NoteSeverity::Info,
                text: format!(
                    "Read “{}” as the operator's estimate of {}.",
                    largest.text, signal.title
                ),
                observation: Some(oid),
            });
        }
    }
    let bytes = text.as_bytes();
    Evidence {
        id,
        submitted_at: received_at,
        origin: EvidenceOrigin::Operator {
            channel: SubmissionChannel::Symptom,
        },
        responds_to: None,
        raw: RawEvidence {
            media_type: "text/plain; charset=utf-8".to_owned(),
            filename: None,
            sha256: sha256_hex(bytes),
            size_bytes: bytes.len() as u64,
            body: RawBody::Text { text },
        },
        declared: DeclaredContext::default(),
        interpretation: Interpretation {
            form: EvidenceForm::OperatorObservation,
            interpreter: "symptom/1".to_owned(),
            observations,
            notes,
        },
        simulation: None,
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Build the raw record. Returns the text view (if the bytes are text) and the
/// bytes to store by reference (if they are binary or large).
fn raw_evidence(s: &Submission) -> (RawEvidence, Option<String>, Option<Vec<u8>>) {
    let (bytes, text): (Vec<u8>, Option<String>) = match &s.body {
        SubmissionBody::Text(t) => (t.as_bytes().to_vec(), Some(t.clone())),
        SubmissionBody::Bytes(b) => {
            let text = if attachment::sniff_media_type(b).is_some() {
                None
            } else {
                String::from_utf8(b.clone()).ok()
            };
            (b.clone(), text)
        }
    };
    let sha = sha256_hex(&bytes);
    let media_type = s
        .media_type
        .clone()
        .filter(|m| !m.is_empty() && m != "application/octet-stream")
        .or_else(|| attachment::sniff_media_type(&bytes).map(str::to_owned))
        .unwrap_or_else(|| {
            if text.is_some() {
                "text/plain; charset=utf-8".to_owned()
            } else {
                "application/octet-stream".to_owned()
            }
        });
    let inline = text.as_ref().filter(|t| t.len() <= MAX_INLINE_BYTES);
    let (body, blob) = match inline {
        Some(t) => (RawBody::Text { text: t.clone() }, None),
        None => (
            RawBody::Stored {
                reference: sha.clone(),
            },
            Some(bytes.clone()),
        ),
    };
    (
        RawEvidence {
            media_type,
            filename: s.filename.clone(),
            body,
            sha256: sha,
            size_bytes: bytes.len() as u64,
        },
        text,
        blob,
    )
}

/// Detect the form of a submission.
pub fn detect_form(text: &str) -> EvidenceForm {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return EvidenceForm::Unrecognised;
    }
    if trimmed.contains(SAMPLE_BLOCK_MARKER) {
        return EvidenceForm::SampleBlock;
    }
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        return if prometheus::is_api_output(&value) {
            EvidenceForm::PrometheusQueryOutput
        } else {
            EvidenceForm::Json
        };
    }
    if config::looks_like_dynamic_config(trimmed) {
        return EvidenceForm::Configuration;
    }
    if tabular::looks_like_csv(trimmed) {
        return EvidenceForm::Csv;
    }
    if tabular::looks_like_series_text(trimmed) {
        return EvidenceForm::MetricSeriesText;
    }
    if prometheus::looks_like_exposition(trimmed) {
        return EvidenceForm::PrometheusExposition;
    }
    if text::looks_like_log(trimmed) {
        return EvidenceForm::Log;
    }
    if trimmed.lines().count() > 1 && trimmed.contains("  ") {
        return EvidenceForm::CommandOutput;
    }
    EvidenceForm::Unrecognised
}

fn detect(s: &Submission, text: Option<&str>, blob: Option<&[u8]>) -> EvidenceForm {
    match s.channel {
        SubmissionChannel::Observation | SubmissionChannel::Symptom => {
            return EvidenceForm::OperatorObservation;
        }
        SubmissionChannel::StructuredEntry => return EvidenceForm::MetricValues,
        _ => {}
    }
    if blob.is_some_and(|b| attachment::sniff_media_type(b).is_some()) {
        return EvidenceForm::Attachment;
    }
    match text {
        Some(t) => detect_form(t),
        None => EvidenceForm::Attachment,
    }
}

/// Accumulates observations and notes for one evidence record.
struct Builder<'a> {
    id: &'a EvidenceId,
    ctx: &'a IngestContext<'a>,
    received_at: Timestamp,
    channel: SubmissionChannel,
    observations: Vec<Observation>,
    notes: Vec<InterpretationNote>,
    interpreter: &'static str,
}

impl<'a> Builder<'a> {
    fn new(
        id: &'a EvidenceId,
        ctx: &'a IngestContext<'a>,
        received_at: Timestamp,
        channel: SubmissionChannel,
    ) -> Self {
        Self {
            id,
            ctx,
            received_at,
            channel,
            observations: Vec::new(),
            notes: Vec::new(),
            interpreter: "unrecognised/1",
        }
    }

    fn next_id(&self) -> ObservationId {
        ObservationId::new(format!("{}.O{}", self.id, self.observations.len() + 1))
    }

    fn note(
        &mut self,
        severity: NoteSeverity,
        text: impl Into<String>,
        observation: Option<ObservationId>,
    ) {
        self.notes.push(InterpretationNote {
            severity,
            text: text.into(),
            observation,
        });
    }

    /// Telemetry parsed by Kyumei is observed fact; anything the operator
    /// typed is an assertion.
    fn status(&self) -> EpistemicStatus {
        match self.channel {
            SubmissionChannel::Paste | SubmissionChannel::Upload | SubmissionChannel::Collector => {
                EpistemicStatus::ObservedFact
            }
            _ => EpistemicStatus::OperatorAssertion,
        }
    }

    fn push(
        &mut self,
        subject: Subject,
        resolution: Resolution,
        window: Option<TimeWindow>,
        value: ObservedValue,
    ) {
        let id = self.next_id();
        for (severity, text) in resolution.notes {
            self.note(severity, text, Some(id.clone()));
        }
        let status = self.status();
        self.observations.push(Observation {
            id,
            evidence: self.id.clone(),
            status,
            subject,
            binding: resolution.binding,
            window,
            value,
        });
    }

    fn structured(&mut self, value: Option<&StructuredValue>) {
        self.interpreter = "structured-entry/1";
        let Some(v) = value else {
            self.note(NoteSeverity::Problem, "No value was entered.", None);
            return;
        };
        let Some(signal) = self.ctx.signal(&v.signal) else {
            self.note(
                NoteSeverity::Problem,
                format!("`{}` is not a signal of this diagnostic.", v.signal),
                None,
            );
            return;
        };
        let resolution = Resolution {
            binding: Some(SignalBinding {
                signal: signal.id.clone(),
                basis: BindingBasis::StructuredEntry,
                claim: signal.source.first().map(|s| s.claim.clone()),
            }),
            unit: v.unit.or(Some(signal.unit)),
            transform: None,
            notes: vec![(
                NoteSeverity::Info,
                "Entered by hand: treated as an operator assertion, not observed telemetry."
                    .to_owned(),
            )],
        };
        let unit = resolution.unit;
        self.push(
            Subject::described(signal.title.clone()),
            resolution,
            self.ctx.declared.window,
            ObservedValue::Scalar {
                unit,
                value: v.value,
                statistic: v.statistic,
            },
        );
    }

    fn observation_text(&mut self, text: &str) {
        self.interpreter = "operator-observation/1";
        let declared_signal = self
            .ctx
            .declared
            .signal
            .as_ref()
            .and_then(|s| self.ctx.signal(s))
            .map(|s| (s.id.clone(), s.title.clone()));
        self.push(
            Subject::described("Operator observation"),
            Resolution::default(),
            self.ctx.declared.window,
            ObservedValue::Text {
                text: text.to_owned(),
            },
        );
        if let Some((signal, title)) = declared_signal {
            self.note(
                NoteSeverity::Info,
                format!("The observation is about {title} ({signal}); enter a value to let rules use it."),
                None,
            );
        }
    }

    fn prometheus_json(&mut self, text: &str) {
        self.interpreter = "prometheus-api/1";
        let value: serde_json::Value = match serde_json::from_str(text.trim()) {
            Ok(v) => v,
            Err(e) => {
                self.note(NoteSeverity::Problem, format!("Not valid JSON: {e}"), None);
                return;
            }
        };
        match prometheus::parse_api_output(&value) {
            Ok(series) => self.series(series),
            Err(e) => self.note(NoteSeverity::Problem, e, None),
        }
    }

    /// Every sample block in the text: each query's series bound as if its
    /// Prometheus output had been pasted with that query.
    fn sample_blocks(&mut self, text: &str) {
        self.interpreter = "sample-block/1";
        let blocks = kyumei_samples::decode_all(text);
        if blocks.is_empty() {
            self.note(
                NoteSeverity::Problem,
                kyumei_samples::BlockError::NotFound.to_string(),
                None,
            );
            return;
        }
        let many = blocks.len() > 1;
        for (i, block) in blocks.into_iter().enumerate() {
            let label = if many {
                format!("Block {}: ", i + 1)
            } else {
                String::new()
            };
            match block {
                Ok(samples) => self.sample_block(samples, &label),
                Err(e) => self.note(NoteSeverity::Problem, format!("{label}{e}"), None),
            }
        }
    }

    fn sample_block(&mut self, s: kyumei_samples::Samples, label: &str) {
        let when = Timestamp::from_millisecond(s.collected_at_ms)
            .map(|t| t.strftime("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default();
        self.note(
            NoteSeverity::Info,
            format!(
                "{label}Collected by {} from {} at {when}.",
                s.collector, s.source
            ),
            None,
        );
        if let (Some(collected_for), Some(request)) = (&s.request, self.ctx.request)
            && collected_for != request.id.as_str()
        {
            self.note(
                NoteSeverity::Caution,
                format!(
                    "{label}The block was collected for request {collected_for}, not {}: check that it answers this request.",
                    request.id
                ),
                None,
            );
        }
        if let Some(digits) = s.rounded_to {
            self.note(
                NoteSeverity::Caution,
                format!("{label}Values were rounded to {digits} significant digits when they were collected."),
                None,
            );
        }
        if !s.redacted.is_empty() {
            self.note(
                NoteSeverity::Info,
                format!(
                    "{label}The values of {} are pseudonyms: kyumei-collect reveal shows them on the machine that collected the block.",
                    s.redacted.join(", ")
                ),
                None,
            );
        }
        let window = match (
            Timestamp::from_millisecond(s.window.start_ms),
            Timestamp::from_millisecond(s.window.end_ms),
        ) {
            (Ok(start), Ok(end)) => TimeWindow::new(start, end).ok(),
            _ => None,
        };
        // Each query's sample times, for a witness to vouch for another's empty result.
        let sampled: BTreeMap<String, Vec<i64>> = s
            .queries
            .iter()
            .map(|q| {
                let mut times: Vec<i64> = q
                    .series
                    .iter()
                    .flat_map(|series| series.points.iter().map(|p| p.at_ms))
                    .collect();
                times.sort_unstable();
                times.dedup();
                (q.query.clone(), times)
            })
            .collect();
        for q in s.queries {
            if q.series.is_empty() {
                if let Some((zero, times)) = self.witnessed(&q.query, &sampled) {
                    self.note(
                        NoteSeverity::Info,
                        format!(
                            "{label}`{}` returned no series, while `{}` has {} samples over the same window. The counter has no series until it first counts (`{}`), so nothing was counted, and it is read as zero.",
                            q.query,
                            zero.witness,
                            times.len(),
                            zero.because
                        ),
                        None,
                    );
                    let declared = DeclaredContext {
                        query: Some(q.query.clone()),
                        window: window.or(self.ctx.declared.window),
                        ..self.ctx.declared.clone()
                    };
                    let points = times
                        .iter()
                        .filter_map(|t| Timestamp::from_millisecond(*t).ok())
                        .map(|at| SeriesPoint { at, value: 0.0 })
                        .collect();
                    // Bound through the query like any answer, then marked as read
                    // from the witness.
                    let first = self.observations.len();
                    self.series_with(
                        vec![ParsedSeries {
                            metric: None,
                            labels: BTreeMap::new(),
                            points,
                        }],
                        &declared,
                    );
                    for o in &mut self.observations[first..] {
                        o.subject.description = Some("no series, read as zero".to_owned());
                        if let Some(b) = o.binding.as_mut() {
                            b.basis = BindingBasis::AbsentWithWitness {
                                witness: zero.witness.clone(),
                                because: zero.because.clone(),
                            };
                        }
                    }
                    continue;
                }
                self.note(
                    NoteSeverity::Info,
                    format!("{label}`{}` returned no series. Kyumei records that none was found, which is not evidence that the value was zero.", q.query),
                    None,
                );
                self.absent(&DeclaredContext {
                    query: Some(q.query.clone()),
                    window: window.or(self.ctx.declared.window),
                    ..self.ctx.declared.clone()
                });
                continue;
            }
            let declared = DeclaredContext {
                query: Some(q.query.clone()),
                window: window.or(self.ctx.declared.window),
                ..self.ctx.declared.clone()
            };
            let parsed = q
                .series
                .into_iter()
                .map(|series| {
                    let mut labels = series.labels;
                    let metric = labels.remove("__name__");
                    ParsedSeries {
                        metric,
                        labels,
                        points: series
                            .points
                            .iter()
                            .filter_map(|p| {
                                Timestamp::from_millisecond(p.at_ms)
                                    .ok()
                                    .map(|at| SeriesPoint { at, value: p.value })
                            })
                            .collect(),
                    }
                })
                .collect();
            let first = self.observations.len();
            self.series_with(parsed, &declared);
            if self.is_witness(&q.query) {
                // A witness is read to vouch for another query's empty result. Its
                // series are kept only where they inform a signal of their own.
                let dropped: Vec<ObservationId> = self.observations[first..]
                    .iter()
                    .filter(|o| o.binding.is_none())
                    .map(|o| o.id.clone())
                    .collect();
                self.observations.retain(|o| !dropped.contains(&o.id));
                self.notes.retain(|n| {
                    n.observation
                        .as_ref()
                        .is_none_or(|id| !dropped.contains(id))
                });
            }
        }
    }

    /// Whether `query` is the witness of one of the answered request's queries.
    fn is_witness(&self, query: &str) -> bool {
        self.ctx.request.is_some_and(|r| {
            r.collect.iter().any(|c| {
                c.absent_reads_zero
                    .as_ref()
                    .is_some_and(|z| z.witness == query)
            })
        })
    }

    /// Record that `declared`'s query returned no series over its window, bound
    /// to the signal the query informs. It carries no value.
    fn absent(&mut self, declared: &DeclaredContext) {
        let ctx = IngestContext {
            declared,
            ..*self.ctx
        };
        let resolution = bind::resolve_metric(&ctx, None, &declared.labels);
        let Some(binding) = resolution.binding else {
            return;
        };
        self.push(
            Subject::described("no series".to_owned()),
            Resolution {
                binding: Some(binding),
                ..Resolution::default()
            },
            declared.window,
            ObservedValue::Absent {
                query: declared.query.clone().unwrap_or_default(),
            },
        );
    }

    /// The witness that lets `query`'s empty result read as zero, with its sample
    /// times, when the request being answered names one and the block has it.
    fn witnessed<'s>(
        &self,
        query: &str,
        sampled: &'s BTreeMap<String, Vec<i64>>,
    ) -> Option<(AbsentReadsZero, &'s Vec<i64>)> {
        let zero = self
            .ctx
            .request?
            .collect
            .iter()
            .find(|c| c.body == query)?
            .absent_reads_zero
            .clone()?;
        let times = sampled.get(&zero.witness).filter(|t| !t.is_empty())?;
        Some((zero, times))
    }

    fn series(&mut self, series: Vec<ParsedSeries>) {
        let declared = self.ctx.declared.clone();
        self.series_with(series, &declared);
    }

    /// Bind and record series, reading the query, window and labels from
    /// `declared`: the submission's own, or one query of a sample block.
    fn series_with(&mut self, series: Vec<ParsedSeries>, declared: &DeclaredContext) {
        let ctx = IngestContext {
            declared,
            ..*self.ctx
        };
        if self.interpreter == "unrecognised/1" {
            self.interpreter = "series/1";
        }
        if series.is_empty() {
            self.note(
                NoteSeverity::Problem,
                "No samples were found. An empty result is not evidence that the value was zero.",
                None,
            );
            return;
        }
        let query = declared.query.clone().unwrap_or_default();
        let first = self.observations.len();
        let total = series.len();
        if total > MAX_OBSERVATIONS {
            self.note(
                NoteSeverity::Problem,
                format!("Only the first {MAX_OBSERVATIONS} of {total} series were interpreted; narrow the query (for example with sum by)."),
                None,
            );
        }
        for s in series.into_iter().take(MAX_OBSERVATIONS) {
            let mut labels = s.labels.clone();
            for (k, v) in &declared.labels {
                labels.entry(k.clone()).or_insert_with(|| v.clone());
            }
            let resolution = bind::resolve_metric(&ctx, s.metric.as_deref(), &labels);
            let mut points = s.points.clone();
            points.sort_by_key(|p| p.at);
            let mut extra = Vec::new();
            if points.len() > MAX_POINTS {
                let original = points.len();
                points = downsample(&points, MAX_POINTS);
                extra.push((
                    NoteSeverity::Info,
                    format!("{original} samples were reduced to {} (each bucket's minimum and maximum kept, so extremes are preserved).", points.len()),
                ));
            }
            let mut unit = resolution.unit;
            if resolution.transform == Some(Transform::Rate) {
                let already_rate = ["rate(", "irate(", "deriv("]
                    .iter()
                    .any(|f| query.contains(f));
                if already_rate {
                    unit = Some(Unit::PerSecond);
                } else if query.contains("increase(") {
                    extra.push((
                        NoteSeverity::Caution,
                        "`increase()` returns a count per range window, not a per-second rate. Correct the unit if rules should use it.".to_owned(),
                    ));
                } else if is_cumulative(&points) {
                    points = to_rate(&points);
                    unit = Some(Unit::PerSecond);
                    extra.push((
                        NoteSeverity::Info,
                        "Cumulative counter values were converted to a per-second rate.".to_owned(),
                    ));
                } else {
                    unit = Some(Unit::PerSecond);
                    extra.push((
                        NoteSeverity::Caution,
                        "The values are not cumulative, so they were read as a per-second rate. Correct the unit if they are counts.".to_owned(),
                    ));
                }
            }
            let window = window_of(&points).or(declared.window);
            let subject = Subject {
                metric: s.metric.clone(),
                labels,
                description: None,
            };
            let mut resolution = resolution;
            resolution.notes.extend(extra);
            self.push(
                subject,
                resolution,
                window,
                ObservedValue::Series { unit, points },
            );
        }
        self.absent_from_breakdown(&query, first);
    }

    /// Read a requested signal as zero when a breakdown leaves it out.
    ///
    /// Prometheus returns no series for label values that did not occur. So
    /// when an answer breaks a metric down by a label (its bound series carry
    /// the label and the query does not filter it), a signal the request
    /// expects on the same metric, with no series for its value of that
    /// label, did not occur at the answer's sample times. Signals the query
    /// filtered out, and metrics not broken down by the label, are left alone.
    fn absent_from_breakdown(&mut self, query: &str, first: usize) {
        let (Some(request), Some(diagnostic)) = (self.ctx.request, self.ctx.diagnostic) else {
            return;
        };
        let bound: Vec<&Observation> = self.observations[first..]
            .iter()
            .filter(|o| o.binding.is_some())
            .collect();
        let mut inferred = Vec::new();
        for id in &request.expected_signals {
            if bound
                .iter()
                .any(|o| o.binding.as_ref().is_some_and(|b| &b.signal == id))
            {
                continue;
            }
            let Some(signal) = diagnostic.signal(id) else {
                continue;
            };
            for source in signal.source.iter().filter(|s| !s.labels.is_empty()) {
                let siblings: Vec<&Observation> = bound
                    .iter()
                    .copied()
                    .filter(|o| {
                        o.binding.as_ref().and_then(|b| b.claim.as_ref()) == Some(&source.claim)
                    })
                    .collect();
                if siblings.is_empty() {
                    continue;
                }
                let mut by = Vec::new();
                let fits = source.labels.iter().all(|(label, value)| {
                    if bind::query_filters(query, label, value) {
                        return true;
                    }
                    let broken_down = !bind::query_matches_label(query, label)
                        && siblings
                            .iter()
                            .all(|o| o.subject.labels.contains_key(label));
                    if broken_down {
                        by.push(label.clone());
                    }
                    broken_down
                });
                let mut instants: Vec<Timestamp> = siblings
                    .iter()
                    .flat_map(|o| match &o.value {
                        ObservedValue::Series { points, .. } => {
                            points.iter().map(|p| p.at).collect()
                        }
                        _ => Vec::new(),
                    })
                    .collect();
                instants.sort();
                instants.dedup();
                if !fits || by.is_empty() || instants.is_empty() {
                    continue;
                }
                let metric = siblings[0].subject.metric.clone();
                inferred.push((signal, source, by, instants, metric));
                break;
            }
        }
        for (signal, source, by, instants, metric) in inferred {
            let constraint: Vec<String> = by
                .iter()
                .filter_map(|l| source.labels.get(l).map(|v| format!("{l}=\"{v}\"")))
                .collect();
            let points: Vec<SeriesPoint> = instants
                .iter()
                .map(|at| SeriesPoint {
                    at: *at,
                    value: 0.0,
                })
                .collect();
            let resolution = Resolution {
                binding: Some(SignalBinding {
                    signal: signal.id.clone(),
                    basis: BindingBasis::AbsentFromBreakdown { by: by.clone() },
                    claim: Some(source.claim.clone()),
                }),
                unit: Some(signal.unit),
                transform: None,
                notes: vec![(
                    NoteSeverity::Info,
                    format!(
                        "No series has {}, so {} is read as zero at the answer's {} sample times: Prometheus returns no series for label values that did not occur. Correct this if the series is missing for another reason.",
                        constraint.join(", "),
                        signal.title,
                        points.len()
                    ),
                )],
            };
            let window = window_of(&points);
            self.push(
                Subject {
                    metric,
                    labels: source.labels.clone(),
                    description: Some("absent from the breakdown".to_owned()),
                },
                resolution,
                window,
                ObservedValue::Series {
                    unit: Some(signal.unit),
                    points,
                },
            );
        }
    }

    fn structured_output(&mut self, text: &str) {
        let trimmed = text.trim();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            self.interpreter = "json-fields/1";
            let mut matched = 0;
            let first = self.observations.len();
            let flat = fields::flatten(&value);
            let mixed = fields::mixed_type_lists(&value);
            let in_mixed = |path: &str| {
                mixed
                    .iter()
                    .any(|(list, _)| path == list || path.starts_with(&format!("{list}[")))
            };
            for field in flat.iter().filter(|f| !in_mixed(&f.path)) {
                matched += usize::from(self.field(&field.path, &field.key, &field.value));
            }
            matched += self.omitted_flags(&flat, first);
            if !mixed.is_empty() {
                let lists: Vec<String> =
                    mixed.iter().map(|(list, _)| format!("`{list}`")).collect();
                let mut types: Vec<String> = mixed.iter().flat_map(|(_, t)| t.clone()).collect();
                types.sort();
                types.dedup();
                let (verb, them) = if lists.len() == 1 {
                    ("describes", "it")
                } else {
                    ("describe", "them")
                };
                self.note(
                    NoteSeverity::Caution,
                    format!(
                        "{} {verb} the {} Task Queues together, so Kyumei left {them} unread: it cannot tell which rows are about the Task Queue this request is for. Describe one type, with `--task-queue-type`.",
                        fields::and_list(&lists),
                        fields::and_list(&types),
                    ),
                    None,
                );
            } else if matched == 0 {
                self.note(
                    NoteSeverity::Caution,
                    "No field this knowledge pack describes was found. The JSON is preserved as submitted.",
                    None,
                );
            }
            return;
        }
        self.interpreter = "cli-table/1";
        let known = self.known_field_names();
        let tables = fields::text_tables(trimmed, &known);
        let mut matched = 0;
        for table in &tables {
            for row in &table.rows {
                for (header, cell) in table.headers.iter().zip(row) {
                    let scalar = cell
                        .parse::<f64>()
                        .map(fields::FieldScalar::Number)
                        .unwrap_or_else(|_| fields::FieldScalar::Text(cell.clone()));
                    matched += usize::from(self.field(header, header, &scalar));
                }
            }
        }
        if matched == 0 {
            self.note(
                NoteSeverity::Caution,
                "Kyumei found no recognised table in this output. It is preserved as submitted; `-o json` output is easier to interpret.",
                None,
            );
        }
    }

    /// Record the boolean fields a response leaves out. Protobuf JSON omits a
    /// boolean when it is false, so a response that has the field's parent (a
    /// DescribeTaskQueue `stats`, say) but not the field answers it, though not
    /// with a value: rules read it as unknown, and the request is not asked again.
    /// The parent must be an object: the Temporal CLI prints `stats` as a list
    /// of its own rows, which never carry the field, so there its absence says
    /// nothing. Returns how many were recorded.
    fn omitted_flags(&mut self, flat: &[fields::FieldValue], first: usize) -> usize {
        let Some(diagnostic) = self.ctx.diagnostic else {
            return 0;
        };
        let under = |prefix: &str| {
            flat.iter().any(|f| {
                f.path.starts_with(&format!("{prefix}.")) || f.path.contains(&format!(".{prefix}."))
            })
        };
        let mut omitted = Vec::new();
        for signal in &diagnostic.signals {
            let bound = self.observations[first..]
                .iter()
                .any(|o| o.binding.as_ref().is_some_and(|b| b.signal == signal.id));
            if bound {
                continue;
            }
            for source in &signal.source {
                let Some(kyumei_knowledge::Claim::ApiField(f)) = self.ctx.pack.claim(&source.claim)
                else {
                    continue;
                };
                let Some((parent, _)) = f.json_path.as_deref().and_then(|p| p.rsplit_once('.'))
                else {
                    continue;
                };
                if f.field_type == "bool" && under(parent) {
                    omitted.push((
                        signal.id.clone(),
                        source.claim.clone(),
                        f.json_path.clone().unwrap_or_default(),
                    ));
                    break;
                }
            }
        }
        let count = omitted.len();
        for (signal, claim, path) in omitted {
            let resolution = Resolution {
                binding: Some(SignalBinding {
                    signal,
                    basis: BindingBasis::FieldName {
                        field: path.clone(),
                    },
                    claim: Some(claim),
                }),
                ..Resolution::default()
            };
            let window = self.ctx.declared.window.or(Some(TimeWindow {
                start: self.received_at,
                end: self.received_at,
            }));
            self.push(
                Subject::described(path.clone()),
                resolution,
                window,
                ObservedValue::Omitted { field: path },
            );
        }
        count
    }

    fn known_field_names(&self) -> Vec<String> {
        self.ctx
            .pack
            .claims
            .values()
            .filter_map(|c| match c {
                kyumei_knowledge::Claim::ApiField(f) => Some(f.json_name.clone()),
                _ => None,
            })
            .collect()
    }

    /// Bind one API field value. Returns whether it bound to a signal.
    fn field(&mut self, path: &str, key: &str, value: &fields::FieldScalar) -> bool {
        let matches: Vec<(SignalId, kyumei_domain::ClaimId, Option<Unit>, Unit)> =
            bind::resolve_field(self.ctx, path, key)
                .into_iter()
                .map(|(signal, claim, unit)| (signal.id.clone(), claim, unit, signal.unit))
                .collect();
        let Some((signal, claim, field_unit, signal_unit)) = matches.into_iter().next() else {
            return false;
        };
        let repeated = matches!(
            self.ctx.pack.claim(&claim),
            Some(kyumei_knowledge::Claim::ApiField(f)) if f.field_type.starts_with("repeated ")
        );
        let boolean = matches!(
            self.ctx.pack.claim(&claim),
            Some(kyumei_knowledge::Claim::ApiField(f)) if f.field_type == "bool"
        );
        let (value, unit) = match value {
            // A boolean field, in JSON or a CLI table, is a 0/1 flag.
            fields::FieldScalar::Text(t)
                if boolean
                    && (t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("false")) =>
            {
                (
                    if t.eq_ignore_ascii_case("true") {
                        1.0
                    } else {
                        0.0
                    },
                    Some(Unit::Flag),
                )
            }
            fields::FieldScalar::Number(n) => (*n, field_unit.or(Some(signal_unit))),
            fields::FieldScalar::Count(n) => (*n as f64, Some(Unit::Count)),
            // Go's encoding/json writes an empty list as `null`.
            fields::FieldScalar::Null if repeated => (0.0, Some(Unit::Count)),
            fields::FieldScalar::Null => return false,
            fields::FieldScalar::Text(t) => match timestamps::parse_duration_seconds(t) {
                Some(secs)
                    if field_unit.is_some_and(Unit::is_duration) || signal_unit.is_duration() =>
                {
                    (secs, Some(Unit::Seconds))
                }
                _ => {
                    let resolution = Resolution {
                        binding: Some(SignalBinding {
                            signal,
                            basis: BindingBasis::FieldName {
                                field: path.to_owned(),
                            },
                            claim: Some(claim),
                        }),
                        ..Resolution::default()
                    };
                    self.push(
                        Subject::described(path.to_owned()),
                        resolution,
                        self.ctx.declared.window,
                        ObservedValue::Categorical { value: t.clone() },
                    );
                    return true;
                }
            },
        };
        let resolution = Resolution {
            binding: Some(SignalBinding {
                signal,
                basis: BindingBasis::FieldName {
                    field: path.to_owned(),
                },
                claim: Some(claim),
            }),
            unit,
            ..Resolution::default()
        };
        let window = match self.ctx.declared.window {
            Some(w) => Some(w),
            None => {
                self.note(
                    NoteSeverity::Caution,
                    format!(
                        "`{path}` is a point-in-time value with no capture time; assumed captured when submitted. Correct the window if it was captured earlier."
                    ),
                    None,
                );
                Some(TimeWindow {
                    start: self.received_at,
                    end: self.received_at,
                })
            }
        };
        self.push(
            Subject::described(path.to_owned()),
            resolution,
            window,
            ObservedValue::Scalar {
                unit,
                value,
                statistic: Statistic::Last,
            },
        );
        true
    }

    fn configuration(&mut self, text: &str) {
        self.interpreter = "dynamic-config/1";
        let entries = config::parse_dynamic_config(text);
        if entries.is_empty() {
            self.note(
                NoteSeverity::Caution,
                "No dynamic configuration entries were recognised. The text is preserved as submitted.",
                None,
            );
            return;
        }
        let namespace = self.ctx.scope.primary_namespace();
        let task_queue = self.ctx.scope.primary_task_queue();
        let mut by_key: BTreeMap<String, Vec<config::ConfigEntry>> = BTreeMap::new();
        for e in entries {
            by_key.entry(e.key.clone()).or_default().push(e);
        }
        for (key, entries) in by_key {
            let targets = bind::resolve_config(self.ctx, &key);
            // The most specific entry that applies to the investigation's scope.
            let effective = entries
                .iter()
                .filter_map(|e| {
                    config::specificity(e, namespace, task_queue, Some("Workflow")).map(|s| (s, e))
                })
                .max_by_key(|(s, _)| *s)
                .map(|(_, e)| e.clone());
            for entry in &entries {
                let is_effective = effective.as_ref() == Some(entry);
                let mut labels = entry.constraints.clone();
                labels.insert("key".to_owned(), key.clone());
                let subject = Subject {
                    metric: None,
                    labels,
                    description: Some(key.clone()),
                };
                let mut resolution = Resolution::default();
                if is_effective && let Some((signal, claim)) = targets.first() {
                    resolution.binding = Some(SignalBinding {
                        signal: signal.id.clone(),
                        basis: BindingBasis::FieldName { field: key.clone() },
                        claim: Some(claim.clone()),
                    });
                    if namespace.is_none() || task_queue.is_none() {
                        resolution.notes.push((
                            NoteSeverity::Caution,
                            "Namespace or Task Queue is unknown, so constrained entries cannot be matched precisely.".to_owned(),
                        ));
                    }
                } else if !targets.is_empty() {
                    resolution.notes.push((
                        NoteSeverity::Info,
                        "Applies to a different scope, or is overridden by a more specific entry."
                            .to_owned(),
                    ));
                }
                self.push(
                    subject,
                    resolution,
                    self.ctx.declared.window,
                    ObservedValue::Categorical {
                        value: entry.value.clone(),
                    },
                );
            }
        }
        self.note(
            NoteSeverity::Info,
            "Keys absent from this fragment are not assumed to be at their defaults: the fragment may be partial.",
            None,
        );
    }

    fn log(&mut self, text: &str) {
        self.interpreter = "log/1";
        let summary = text::summarise_log(text);
        let window = match (summary.first, summary.last) {
            (Some(first), Some(last)) => TimeWindow::new(first, last).ok(),
            _ => None,
        }
        .or(self.ctx.declared.window);
        self.push(
            Subject::described("Log excerpt"),
            Resolution::default(),
            window,
            ObservedValue::Text {
                text: format!(
                    "{} line(s): {} error, {} warning",
                    summary.lines, summary.errors, summary.warnings
                ),
            },
        );
        self.note(
            NoteSeverity::Info,
            "Log lines are preserved verbatim. This diagnostic has no verified log patterns, so they do not move hypotheses; add an observation describing what they show.",
            None,
        );
    }

    fn attachment(&mut self, raw: &RawEvidence, bytes: Option<&[u8]>) {
        self.interpreter = "attachment/1";
        let info = bytes.and_then(attachment::image_info);
        let text = match &info {
            Some(i) => format!("{} image, {}×{} px", i.format, i.width, i.height),
            None => format!("{} attachment, {} bytes", raw.media_type, raw.size_bytes),
        };
        self.push(
            Subject::described(
                raw.filename
                    .clone()
                    .unwrap_or_else(|| "Attachment".to_owned()),
            ),
            Resolution::default(),
            self.ctx.declared.window,
            ObservedValue::Text { text },
        );
        self.note(
            NoteSeverity::Info,
            "Kyumei does not read images. Describe what the screenshot shows as an observation, or paste the underlying data.",
            None,
        );
    }

    fn finish(self, form: EvidenceForm) -> Interpretation {
        Interpretation {
            form,
            interpreter: self.interpreter.to_owned(),
            observations: self.observations,
            notes: self.notes,
        }
    }
}

/// Keep each bucket's minimum and maximum, in time order.
fn downsample(points: &[SeriesPoint], max: usize) -> Vec<SeriesPoint> {
    let buckets = (max / 2).max(1);
    let size = points.len().div_ceil(buckets);
    let mut out = Vec::with_capacity(max);
    for chunk in points.chunks(size) {
        let lo = chunk.iter().min_by(|a, b| a.value.total_cmp(&b.value));
        let hi = chunk.iter().max_by(|a, b| a.value.total_cmp(&b.value));
        if let (Some(lo), Some(hi)) = (lo, hi) {
            if lo.at <= hi.at {
                out.push(*lo);
                if hi.at != lo.at {
                    out.push(*hi);
                }
            } else {
                out.push(*hi);
                out.push(*lo);
            }
        }
    }
    out
}

fn window_of(points: &[SeriesPoint]) -> Option<TimeWindow> {
    let first = points.iter().map(|p| p.at).min()?;
    let last = points.iter().map(|p| p.at).max()?;
    TimeWindow::new(first, last).ok()
}

/// Monotonic non-decreasing apart from counter resets, with growth.
fn is_cumulative(points: &[SeriesPoint]) -> bool {
    if points.len() < 3 {
        return false;
    }
    let decreases = points
        .windows(2)
        .filter(|w| w[1].value < w[0].value)
        .count();
    let increases = points
        .windows(2)
        .filter(|w| w[1].value > w[0].value)
        .count();
    decreases <= 1 && increases * 2 >= points.len() - 1
}

/// Per-second rate between consecutive samples, handling counter resets.
fn to_rate(points: &[SeriesPoint]) -> Vec<SeriesPoint> {
    points
        .windows(2)
        .filter_map(|w| {
            let dt = w[1].at.duration_since(w[0].at).as_secs_f64();
            if dt <= 0.0 {
                return None;
            }
            let delta = if w[1].value >= w[0].value {
                w[1].value - w[0].value
            } else {
                w[1].value
            };
            Some(SeriesPoint {
                at: w[1].at,
                value: delta / dt,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
