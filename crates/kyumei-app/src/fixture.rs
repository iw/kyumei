//! Fixture investigations: recorded evidence submissions replayed against any
//! [`InvestigationService`], with expectations checked after every step.
//!
//! A fixture is a directory holding `fixture.toml` and the evidence files it
//! references. The same fixture drives the in-process service in unit tests
//! and the durable Odori service in integration tests.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use jiff::Timestamp;
use kyumei_diagnostics::Assessment;
use kyumei_domain::{
    DeclaredContext, EvidenceForm, HypothesisState, InvestigationId, SignalId, SubmissionChannel,
    TimeWindow, Topology, Unit,
};
use kyumei_knowledge::KnowledgeBase;
use serde::Deserialize;

use crate::{
    core::{Command, EvidenceSubmission, OpenInvestigation, SdkInput},
    service::{Clock, InvestigationService, InvestigationSnapshot},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub fixture: FixtureMeta,
    pub open: OpenStep,
    #[serde(default, rename = "step")]
    pub steps: Vec<Step>,
    #[serde(skip)]
    pub dir: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureMeta {
    pub id: String,
    pub title: String,
    pub description: String,
    pub investigation: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenStep {
    #[serde(deserialize_with = "crate::toml_time::timestamp")]
    pub at: Timestamp,
    pub description: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub server_version: Option<String>,
    #[serde(default)]
    pub sdks: Vec<SdkInput>,
    #[serde(default)]
    pub namespaces: Vec<String>,
    #[serde(default)]
    pub task_queues: Vec<String>,
    #[serde(default, deserialize_with = "crate::toml_time::option_timestamp")]
    pub window_start: Option<Timestamp>,
    #[serde(default, deserialize_with = "crate::toml_time::option_timestamp")]
    pub window_end: Option<Timestamp>,
    #[serde(default)]
    pub topology: Option<Topology>,
    #[serde(default)]
    pub expect: Expect,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(deserialize_with = "crate::toml_time::timestamp")]
    pub at: Timestamp,
    /// The request definition this step answers; it must be the current request.
    pub answers: String,
    #[serde(default)]
    pub action: StepAction,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub channel: Option<SubmissionChannel>,
    #[serde(default)]
    pub form: Option<EvidenceForm>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub unit: Option<Unit>,
    #[serde(default)]
    pub signal: Option<String>,
    #[serde(default, deserialize_with = "crate::toml_time::option_timestamp")]
    pub window_start: Option<Timestamp>,
    #[serde(default, deserialize_with = "crate::toml_time::option_timestamp")]
    pub window_end: Option<Timestamp>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub expect: Expect,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepAction {
    #[default]
    Submit,
    Decline,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// Hypothesis states after the step.
    #[serde(default)]
    pub states: BTreeMap<String, HypothesisState>,
    /// The next request definition, or "none".
    #[serde(default)]
    pub next_request: Option<String>,
    /// Findings that must be present.
    #[serde(default)]
    pub findings: Vec<String>,
    /// The supportable conclusion, or "none".
    #[serde(default)]
    pub conclusion: Option<String>,
    /// Whether the investigation is finalised as concluded.
    #[serde(default)]
    pub concluded: Option<bool>,
    /// Signals that must be bound from this step's evidence.
    #[serde(default)]
    pub bound_signals: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("fixture {path}: {message}")]
    Load { path: String, message: String },
    #[error("step {step}: {message}")]
    Step { step: usize, message: String },
    #[error("expectation failed at {at}:\n  - {}", problems.join("\n  - "))]
    Expectation { at: String, problems: Vec<String> },
    #[error(transparent)]
    Service(#[from] crate::service::ServiceError),
}

impl Fixture {
    pub fn load(dir: &Path) -> Result<Self, FixtureError> {
        let path = dir.join("fixture.toml");
        let text = std::fs::read_to_string(&path).map_err(|e| FixtureError::Load {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        let mut f: Fixture = toml::from_str(&text).map_err(|e| FixtureError::Load {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        f.dir = dir.to_path_buf();
        Ok(f)
    }

    pub fn open_command(&self) -> OpenInvestigation {
        OpenInvestigation {
            id: InvestigationId::new(&self.fixture.investigation),
            title: self.open.title.clone(),
            description: self.open.description.clone(),
            signature: None,
            server_version: self.open.server_version.clone(),
            sdks: self.open.sdks.clone(),
            topology: self.open.topology.clone(),
            namespaces: self.open.namespaces.clone(),
            task_queues: self.open.task_queues.clone(),
            window_start: self.open.window_start,
            window_end: self.open.window_end,
            window_description: None,
            profile: None,
        }
    }

    fn submission(
        &self,
        index: usize,
        step: &Step,
        request: kyumei_domain::EvidenceRequestId,
    ) -> Result<EvidenceSubmission, FixtureError> {
        let file = step.file.as_ref().ok_or_else(|| FixtureError::Step {
            step: index,
            message: "a submit step needs a file".to_owned(),
        })?;
        let text =
            std::fs::read_to_string(self.dir.join(file)).map_err(|e| FixtureError::Step {
                step: index,
                message: format!("{file}: {e}"),
            })?;
        let window = match (step.window_start, step.window_end) {
            (Some(s), Some(e)) => TimeWindow::new(s, e).ok(),
            (Some(s), None) => TimeWindow::new(s, s).ok(),
            _ => None,
        };
        Ok(EvidenceSubmission {
            channel: step.channel.unwrap_or(SubmissionChannel::Paste),
            text: Some(text),
            bytes_base64: None,
            filename: Some(file.clone()),
            media_type: None,
            declared: DeclaredContext {
                form: step.form,
                window,
                query: step.query.clone(),
                signal: step.signal.as_deref().map(SignalId::new),
                unit: step.unit,
                labels: BTreeMap::new(),
                note: step.note.clone(),
            },
            responds_to: Some(request),
            structured: None,
            origin: Some(kyumei_domain::EvidenceOrigin::Fixture {
                fixture: self.fixture.id.clone(),
                step: index as u32 + 1,
            }),
            stored: None,
        })
    }
}

/// A clock that fixtures move.
#[derive(Debug)]
pub struct FixtureClock(Mutex<Timestamp>);

impl FixtureClock {
    pub fn new(at: Timestamp) -> Self {
        Self(Mutex::new(at))
    }

    pub fn set(&self, at: Timestamp) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = at;
    }
}

impl Clock for FixtureClock {
    fn now(&self) -> Timestamp {
        *self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// What one replayed step produced.
#[derive(Clone, Debug)]
pub struct StepReport {
    pub label: String,
    pub request: Option<String>,
    pub evidence: Option<String>,
    pub states: BTreeMap<String, (HypothesisState, bool)>,
    pub next_request: Option<String>,
    pub findings: Vec<String>,
    pub conclusion: Option<String>,
    pub concluded: bool,
}

/// Replay a fixture. With `check`, every expectation is enforced.
pub async fn replay(
    kb: &KnowledgeBase,
    service: &dyn InvestigationService,
    fixture: &Fixture,
    clock: Option<&FixtureClock>,
    check: bool,
) -> Result<(InvestigationId, Vec<StepReport>), FixtureError> {
    let (id, opened) = open(kb, service, fixture, clock, check).await?;
    let mut reports = vec![opened];
    for index in 0..fixture.steps.len() {
        reports.push(step(kb, service, fixture, &id, index, clock, check).await?);
    }
    Ok((id, reports))
}

/// Open the fixture's investigation.
pub async fn open(
    kb: &KnowledgeBase,
    service: &dyn InvestigationService,
    fixture: &Fixture,
    clock: Option<&FixtureClock>,
    check: bool,
) -> Result<(InvestigationId, StepReport), FixtureError> {
    if let Some(c) = clock {
        c.set(fixture.open.at);
    }
    let id = service.open(fixture.open_command()).await?;
    let snapshot = service.snapshot(&id).await?;
    let opened = step_report("open", kb, &snapshot, None, None)?;
    if check {
        verify("open", &fixture.open.expect, &opened, &snapshot, None)?;
    }
    Ok((id, opened))
}

/// Replay step `index` (zero-based) against an open investigation.
pub async fn step(
    kb: &KnowledgeBase,
    service: &dyn InvestigationService,
    fixture: &Fixture,
    id: &InvestigationId,
    index: usize,
    clock: Option<&FixtureClock>,
    check: bool,
) -> Result<StepReport, FixtureError> {
    let step = fixture.steps.get(index).ok_or_else(|| FixtureError::Step {
        step: index + 1,
        message: "no such step".to_owned(),
    })?;
    if let Some(c) = clock {
        c.set(step.at);
    }
    let snapshot = service.snapshot(id).await?;
    let current = snapshot
        .investigation
        .current_request()
        .cloned()
        .ok_or_else(|| FixtureError::Step {
            step: index + 1,
            message: format!(
                "expected an open request `{}` but there is none",
                step.answers
            ),
        })?;
    if current.definition.as_str() != step.answers {
        return Err(FixtureError::Step {
            step: index + 1,
            message: format!(
                "expected the current request to be `{}` but it is `{}`",
                step.answers, current.definition
            ),
        });
    }
    let (command, label) = match step.action {
        StepAction::Submit => (
            Command::SubmitEvidence(fixture.submission(index, step, current.id.clone())?),
            format!(
                "step {} ({})",
                index + 1,
                step.file.clone().unwrap_or_default()
            ),
        ),
        StepAction::Decline => (
            Command::DeclineRequest {
                request: current.id.clone(),
                reason: step
                    .reason
                    .clone()
                    .unwrap_or_else(|| "unavailable".to_owned()),
            },
            format!("step {} (decline {})", index + 1, step.answers),
        ),
    };
    let receipt = service.command(id, command).await?;
    let snapshot = service.snapshot(id).await?;
    let done = step_report(
        &label,
        kb,
        &snapshot,
        Some(current.id.to_string()),
        receipt.evidence.as_ref().map(|e| e.to_string()),
    )?;
    if check {
        verify(
            &label,
            &step.expect,
            &done,
            &snapshot,
            receipt.evidence.as_ref(),
        )?;
    }
    Ok(done)
}

fn step_report(
    label: &str,
    kb: &KnowledgeBase,
    snapshot: &InvestigationSnapshot,
    request: Option<String>,
    evidence: Option<String>,
) -> Result<StepReport, FixtureError> {
    let a: Assessment =
        crate::core::current_assessment(kb, &snapshot.investigation).map_err(|e| {
            FixtureError::Step {
                step: 0,
                message: e.to_string(),
            }
        })?;
    Ok(StepReport {
        label: label.to_owned(),
        request,
        evidence,
        states: a
            .hypotheses
            .iter()
            .map(|h| (h.id.to_string(), (h.state, h.contested)))
            .collect(),
        next_request: snapshot
            .investigation
            .current_request()
            .map(|r| r.definition.to_string()),
        findings: a.findings.iter().map(|f| f.id.to_string()).collect(),
        conclusion: a.conclusion.as_ref().map(|c| c.id.to_string()),
        concluded: snapshot.investigation.conclusion.is_some(),
    })
}

fn verify(
    at: &str,
    expect: &Expect,
    report: &StepReport,
    snapshot: &InvestigationSnapshot,
    evidence: Option<&kyumei_domain::EvidenceId>,
) -> Result<(), FixtureError> {
    let mut problems = Vec::new();
    for (h, want) in &expect.states {
        match report.states.get(h) {
            Some((got, _)) if got == want => {}
            Some((got, _)) => problems.push(format!("{h}: expected {want}, got {got}")),
            None => problems.push(format!("{h}: expected {want}, but it is not active")),
        }
    }
    if let Some(next) = &expect.next_request {
        let got = report
            .next_request
            .clone()
            .unwrap_or_else(|| "none".to_owned());
        if &got != next {
            problems.push(format!("next request: expected {next}, got {got}"));
        }
    }
    for f in &expect.findings {
        if !report.findings.contains(f) {
            problems.push(format!(
                "missing finding {f} (have: {})",
                report.findings.join(", ")
            ));
        }
    }
    if let Some(c) = &expect.conclusion {
        let got = report
            .conclusion
            .clone()
            .unwrap_or_else(|| "none".to_owned());
        if &got != c {
            problems.push(format!("conclusion: expected {c}, got {got}"));
        }
    }
    if let Some(done) = expect.concluded
        && done != report.concluded
    {
        problems.push(format!(
            "concluded: expected {done}, got {}",
            report.concluded
        ));
    }
    if !expect.bound_signals.is_empty() {
        let bound: Vec<String> = evidence
            .and_then(|id| snapshot.investigation.evidence(id))
            .map(|e| {
                e.interpretation
                    .observations
                    .iter()
                    .filter_map(|o| o.binding.as_ref().map(|b| b.signal.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        for s in &expect.bound_signals {
            if !bound.contains(s) {
                problems.push(format!(
                    "evidence did not bind `{s}` (bound: {})",
                    bound.join(", ")
                ));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(FixtureError::Expectation {
            at: at.to_owned(),
            problems,
        })
    }
}

/// The repository's fixtures directory.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/investigations")
}

/// The golden fixture.
pub fn golden() -> Result<Fixture, FixtureError> {
    Fixture::load(&fixtures_dir().join("signal-flood-wft-latency"))
}

/// Every fixture in the repository, in order of id.
pub fn all() -> Result<Vec<Fixture>, FixtureError> {
    let dir = fixtures_dir();
    let entries = std::fs::read_dir(&dir).map_err(|e| FixtureError::Load {
        path: dir.display().to_string(),
        message: e.to_string(),
    })?;
    let mut fixtures = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|e| FixtureError::Load {
                path: dir.display().to_string(),
                message: e.to_string(),
            })?
            .path();
        if path.join("fixture.toml").exists() {
            fixtures.push(Fixture::load(&path)?);
        }
    }
    fixtures.sort_by(|a, b| a.fixture.id.cmp(&b.fixture.id));
    Ok(fixtures)
}
