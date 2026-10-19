//! The investigation service port and its in-process implementation.
//!
//! The web layer talks only to [`InvestigationService`]. The durable Odori
//! implementation lives in `kyumei-odori`; [`LocalInvestigations`] runs the
//! same core in process for tests, fixtures and development.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use jiff::Timestamp;
use kyumei_diagnostics::DiagnosticExplanation;
use kyumei_domain::{Investigation, InvestigationId, InvestigationStatus};
use kyumei_knowledge::KnowledgeBase;
use serde::{Deserialize, Serialize};

use crate::{
    blob::BlobStore,
    core::{self, Command, OpenInvestigation, Outcome},
};

/// Everything the application knows about one investigation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InvestigationSnapshot {
    pub investigation: Investigation,
    /// The latest model-written explanation, if one was produced and passed
    /// validation.
    pub model_explanation: Option<DiagnosticExplanation>,
    pub explanation: ExplanationStatus,
    pub runtime: RuntimeInfo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExplanationStatus {
    /// Explanations come from the deterministic engine only.
    Deterministic,
    /// A model explanation is being written for assessment `seq`.
    Pending { seq: u64 },
    /// The model explanation is current.
    Current { seq: u64 },
    /// The model explanation was rejected or failed; deterministic prose is shown.
    Fallback { seq: u64, reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInfo {
    /// `odori` or `in-process`.
    pub kind: String,
    /// The durable workflow ID, when durable.
    pub workflow_id: Option<String>,
    /// Something the operator should know about how the runtime holds this
    /// investigation, such as a restore from its projection.
    pub note: Option<String>,
}

/// A row in the investigation list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InvestigationSummary {
    pub id: InvestigationId,
    pub title: String,
    pub opened_at: Timestamp,
    pub updated_at: Timestamp,
    pub status: InvestigationStatus,
    pub evidence: usize,
    pub conclusion: Option<String>,
}

impl InvestigationSummary {
    pub fn of(i: &Investigation) -> Self {
        Self {
            id: i.id.clone(),
            title: i.title.clone(),
            opened_at: i.opened_at,
            updated_at: i.last_event_at(),
            status: i.status,
            evidence: i.evidence.len().saturating_sub(1),
            conclusion: i.conclusion.as_ref().map(|c| c.title.clone()),
        }
    }
}

/// What happened when a command was applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub events: usize,
    pub evidence: Option<kyumei_domain::EvidenceId>,
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("investigation {0} not found")]
    NotFound(InvestigationId),
    #[error(transparent)]
    Command(#[from] core::CommandError),
    #[error("{0}")]
    Runtime(String),
}

#[async_trait]
pub trait InvestigationService: Send + Sync {
    async fn open(&self, cmd: OpenInvestigation) -> Result<InvestigationId, ServiceError>;
    async fn command(
        &self,
        id: &InvestigationId,
        command: Command,
    ) -> Result<CommandReceipt, ServiceError>;
    async fn snapshot(&self, id: &InvestigationId) -> Result<InvestigationSnapshot, ServiceError>;
    async fn list(&self) -> Result<Vec<InvestigationSummary>, ServiceError>;
    /// Where binary evidence lives.
    fn blobs(&self) -> &BlobStore;
    /// A short description for the UI footer.
    fn describe(&self) -> String;
}

/// A clock the service reads when a command arrives. Tests and fixtures
/// supply their own.
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

#[derive(Debug, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// The most simulations one command leads to: a diagnostic has a handful.
const MAX_SIMULATIONS_PER_COMMAND: usize = 8;

/// An in-process service. Optionally journals each investigation's event log
/// to a directory so development servers survive restarts.
pub struct LocalInvestigations {
    kb: Arc<KnowledgeBase>,
    clock: Arc<dyn Clock>,
    state: Mutex<BTreeMap<InvestigationId, Investigation>>,
    journal: Option<PathBuf>,
    blobs: BlobStore,
    simulate: bool,
}

impl std::fmt::Debug for LocalInvestigations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalInvestigations")
            .field("journal", &self.journal)
            .finish_non_exhaustive()
    }
}

impl LocalInvestigations {
    pub fn new(kb: Arc<KnowledgeBase>, clock: Arc<dyn Clock>) -> Self {
        Self {
            kb,
            clock,
            state: Mutex::new(BTreeMap::new()),
            journal: None,
            blobs: BlobStore::memory(),
            simulate: false,
        }
    }

    /// After each command, run the simulations the assessment says are due and
    /// record them.
    pub fn with_simulation(mut self) -> Self {
        self.simulate = true;
        self
    }

    /// Run and record the simulations due for `investigation`, one at a time:
    /// each records its inputs, so the next assessment no longer asks for it.
    async fn simulate_due(&self, investigation: &mut Investigation) -> Result<(), ServiceError> {
        for _ in 0..MAX_SIMULATIONS_PER_COMMAND {
            let assessment = core::current_assessment(&self.kb, investigation)?;
            let Some(plan) = assessment.simulations.first().cloned() else {
                return Ok(());
            };
            let run = tokio::task::spawn_blocking(move || crate::simulation::run(&plan))
                .await
                .map_err(|e| ServiceError::Runtime(e.to_string()))?;
            let command = Command::RecordSimulation(Box::new(run));
            let outcome = core::handle(&self.kb, investigation, &command, self.clock.now())?;
            core::apply(investigation, &outcome)?;
        }
        Ok(())
    }

    /// Persist event logs as JSON under `dir`, and restore any found there.
    pub fn with_journal(mut self, dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let mut restored = BTreeMap::new();
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let events: Vec<kyumei_domain::RecordedEvent> =
                    serde_json::from_slice(&std::fs::read(&path)?)?;
                let investigation = Investigation::replay(&events)?;
                restored.insert(investigation.id.clone(), investigation);
            }
        }
        self.state = Mutex::new(restored);
        self.blobs = BlobStore::directory(dir.join("blobs"))?;
        self.journal = Some(dir);
        Ok(self)
    }

    fn persist(&self, investigation: &Investigation) -> Result<(), ServiceError> {
        if let Some(dir) = &self.journal {
            let path = dir.join(format!("{}.json", investigation.id));
            let json = serde_json::to_vec_pretty(&investigation.events)
                .map_err(|e| ServiceError::Runtime(e.to_string()))?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, json).map_err(|e| ServiceError::Runtime(e.to_string()))?;
            std::fs::rename(&tmp, &path).map_err(|e| ServiceError::Runtime(e.to_string()))?;
        }
        Ok(())
    }

    fn with<T>(&self, f: impl FnOnce(&mut BTreeMap<InvestigationId, Investigation>) -> T) -> T {
        let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut guard)
    }
}

#[async_trait]
impl InvestigationService for LocalInvestigations {
    async fn open(&self, cmd: OpenInvestigation) -> Result<InvestigationId, ServiceError> {
        let at = self.clock.now();
        let investigation = core::open(&self.kb, &cmd, at)?;
        let id = investigation.id.clone();
        self.persist(&investigation)?;
        self.with(|m| m.insert(id.clone(), investigation));
        Ok(id)
    }

    async fn command(
        &self,
        id: &InvestigationId,
        command: Command,
    ) -> Result<CommandReceipt, ServiceError> {
        let at = self.clock.now();
        let current = self
            .with(|m| m.get(id).cloned())
            .ok_or_else(|| ServiceError::NotFound(id.clone()))?;
        let mut command = command;
        crate::blob::resolve(&mut command, &self.blobs).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                ServiceError::Command(core::CommandError::Invalid(e.to_string()))
            }
            _ => ServiceError::Runtime(e.to_string()),
        })?;
        let outcome: Outcome = core::handle(&self.kb, &current, &command, at)?;
        for (sha, bytes) in &outcome.blobs {
            self.blobs
                .put(sha, bytes)
                .map_err(|e| ServiceError::Runtime(e.to_string()))?;
        }
        let mut updated = current;
        core::apply(&mut updated, &outcome)?;
        if self.simulate {
            self.simulate_due(&mut updated).await?;
        }
        self.persist(&updated)?;
        self.with(|m| m.insert(id.clone(), updated));
        Ok(CommandReceipt {
            events: outcome.events.len(),
            evidence: outcome.evidence,
        })
    }

    async fn snapshot(&self, id: &InvestigationId) -> Result<InvestigationSnapshot, ServiceError> {
        let investigation = self
            .with(|m| m.get(id).cloned())
            .ok_or_else(|| ServiceError::NotFound(id.clone()))?;
        Ok(InvestigationSnapshot {
            investigation,
            model_explanation: None,
            explanation: ExplanationStatus::Deterministic,
            runtime: RuntimeInfo {
                kind: "in-process".to_owned(),
                workflow_id: None,
                note: None,
            },
        })
    }

    async fn list(&self) -> Result<Vec<InvestigationSummary>, ServiceError> {
        let mut rows: Vec<InvestigationSummary> =
            self.with(|m| m.values().map(InvestigationSummary::of).collect());
        rows.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        Ok(rows)
    }

    fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    fn describe(&self) -> String {
        let simulations = if self.simulate {
            format!(
                ", simulations by tempdes {}",
                &crate::simulation::commit()[..7]
            )
        } else {
            String::new()
        };
        match &self.journal {
            Some(dir) => format!("in-process, journalled to {}{simulations}", dir.display()),
            None => format!("in-process (not durable){simulations}"),
        }
    }
}
