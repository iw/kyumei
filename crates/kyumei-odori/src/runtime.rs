//! The durable investigation service: an embedded tokeira engine, Kyumei's
//! investigation worker, and (when a model is configured) Odori's agent
//! worker, behind the same `InvestigationService` port the web uses.

#![allow(missing_debug_implementations)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow};
use async_trait::async_trait;
use kyumei_app::{
    Command, CommandReceipt, ExplanationStatus, InvestigationService, InvestigationSnapshot,
    InvestigationSummary, OpenInvestigation, RuntimeInfo, ServiceError, blob::BlobStore, core,
};
use kyumei_domain::{Investigation, InvestigationId, RecordedEvent};
use kyumei_knowledge::KnowledgeBase;
use odori::{
    ConnectTarget, EmbeddedEngineConfig, EmbeddedEngineStartError, EmbeddedStartupPhase,
    EmbeddedStorageConfig, Engine, OdoriRuntime, SnapshotPolicyConfig, TokeiraConfig,
};
use serde::{Deserialize, Serialize};
use temporalio_client::{
    Client, ClientOptions, Connection, ConnectionOptions, RpcOptions, Url,
    WorkflowExecuteUpdateOptions, WorkflowIdConflictPolicy, WorkflowQueryOptions,
    WorkflowStartOptions,
    errors::{WorkflowQueryError, WorkflowUpdateError},
    tonic::Code,
};
use temporalio_sdk::{Runtime, Worker, WorkerOptions};

use crate::{
    activities::InvestigationActivities,
    agents::{EXPLAINER, Explainer},
    workflow::{
        CommandReply, ExplainConfig, InvestigationInput, InvestigationWorkflow, Submission,
        WorkflowView,
    },
};

pub const APP_QUEUE: &str = "kyumei-investigations";
pub const AGENT_QUEUE: &str = "kyumei-agents";

#[derive(Clone, Debug)]
pub struct DurableConfig {
    /// Holds the engine snapshot, the investigation index, projections and blobs.
    pub data_dir: PathBuf,
    /// How often the in-memory engine writes its snapshot.
    pub snapshot_interval: Duration,
    /// The model that rewrites explanations; `None` keeps them deterministic.
    pub explainer: Option<Explainer>,
    /// Run the simulations each assessment says are due.
    pub simulate: bool,
}

impl DurableConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            snapshot_interval: Duration::from_secs(5),
            explainer: None,
            simulate: false,
        }
    }
}

/// The investigation index and per-investigation projections on disk.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Index {
    investigations: Vec<InvestigationId>,
}

pub struct DurableInvestigations {
    kb: Arc<KnowledgeBase>,
    client: Client,
    blobs: Arc<BlobStore>,
    config: DurableConfig,
    explain: Option<ExplainConfig>,
    index: Mutex<Index>,
    /// How many events each projection on disk holds, as last read or written.
    projections: Mutex<BTreeMap<InvestigationId, usize>>,
    notes: Mutex<BTreeMap<InvestigationId, String>>,
    parts: tokio::sync::Mutex<Option<Parts>>,
}

struct Parts {
    engine: Engine,
    odori: Option<OdoriRuntime>,
    app: AppWorker,
}

impl DurableInvestigations {
    /// Start the engine (restoring its snapshot), the workers, and resume
    /// every known investigation.
    pub async fn start(kb: Arc<KnowledgeBase>, config: DurableConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir)
            .with_context(|| format!("create {}", config.data_dir.display()))?;
        let (engine, set_aside) = start_engine(
            &config.data_dir.join("engine.snapshot"),
            config.snapshot_interval,
        )
        .await?;

        let blobs = Arc::new(BlobStore::directory(config.data_dir.join("blobs"))?);

        // Odori's agent worker, only when a model is configured.
        let (odori, client, explain) = match &config.explainer {
            Some(explainer) => {
                let runtime = OdoriRuntime::builder(AGENT_QUEUE)
                    .connect(ConnectTarget::service_override(engine.service_override()))
                    .agents(explainer.registry())
                    .providers(explainer.providers())
                    .start()
                    .await?;
                let client = runtime.client();
                let explain = ExplainConfig {
                    agent: EXPLAINER.to_owned(),
                    agent_queue: AGENT_QUEUE.to_owned(),
                    provider: explainer.label().to_owned(),
                };
                (Some(runtime), client, Some(explain))
            }
            None => {
                let connection = Connection::connect(
                    ConnectionOptions::new(Url::parse("http://kyumei-embedded.invalid:7233")?)
                        .service_override(engine.service_override())
                        .dns_load_balancing(None)
                        .build(),
                )
                .await?;
                (
                    None,
                    Client::new(connection, ClientOptions::new("default").build())?,
                    None,
                )
            }
        };

        let activities = InvestigationActivities {
            kb: kb.clone(),
            blobs: blobs.clone(),
            simulate: config.simulate,
        };
        let app = AppWorker::start("kyumei-investigation-worker", client.clone(), move || {
            Ok(WorkerOptions::new(APP_QUEUE)
                .register_workflow::<InvestigationWorkflow>()?
                .register_activities(activities)
                .build())
        })
        .await?;

        let index: Index = match std::fs::read(config.data_dir.join("index.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Index::default(),
        };
        let service = Self {
            kb,
            client,
            blobs,
            config,
            explain,
            index: Mutex::new(index),
            projections: Mutex::new(BTreeMap::new()),
            notes: Mutex::new(BTreeMap::new()),
            parts: tokio::sync::Mutex::new(Some(Parts { engine, odori, app })),
        };
        service.resume_all(set_aside.as_deref()).await;
        Ok(service)
    }

    /// Make sure every indexed investigation has a live, up-to-date
    /// workflow. The engine restores its last snapshot, so a process that
    /// stopped between snapshots can come back with a workflow that is
    /// missing recent commands, or with none at all. The projection, kept up
    /// to the engine's log, is then authoritative: the workflow is restarted
    /// from it. `set_aside` is a snapshot the engine could not read.
    async fn resume_all(&self, set_aside: Option<&Path>) {
        for id in self.indexed() {
            let projection = self.read_projection(&id);
            let note = match (self.view(&id).await, projection) {
                (Ok(view), Some(events)) if extends(&events, &view.events) => {
                    let missing = match events.len() - view.events.len() {
                        1 => "1 event".to_owned(),
                        n => format!("{n} events"),
                    };
                    match self.replace_workflow(&id, events).await {
                        Ok(()) => format!(
                            "Restored {missing} recorded after the engine's last snapshot from the saved event log."
                        ),
                        Err(e) => format!(
                            "{missing} recorded after the engine's last snapshot could not be restored: {e}"
                        ),
                    }
                }
                // The engine is current. Its log can be the longer one: the
                // workflow records model explanations on its own, and a
                // command whose view could not be read left the projection
                // behind.
                (Ok(view), _) => {
                    self.keep_projection(&id, &view.events);
                    continue;
                }
                // Starting is a no-op if the engine has the workflow and the
                // query failed for another reason: a run is never replaced on
                // a guess.
                (Err(_), Some(events)) => match self.start_workflow(&id, events).await {
                    Ok(()) => match set_aside {
                        Some(path) => format!(
                            "Restored from the saved event log: the engine could not read its snapshot, which was kept as {}.",
                            path.file_name().map_or(path.display().to_string(), |n| n
                                .to_string_lossy()
                                .into_owned())
                        ),
                        None => {
                            "Restored from the saved event log after the engine lost it.".to_owned()
                        }
                    },
                    Err(e) => format!("Could not be resumed: {e}"),
                },
                (Err(_), None) => {
                    tracing::warn!(investigation = %id, "no workflow and no projection");
                    continue;
                }
            };
            tracing::warn!(investigation = %id, "{note}");
            if let Ok(mut n) = self.notes.lock() {
                n.insert(id, note);
            }
        }
    }

    /// Start a workflow from `events`, terminating any run the engine still
    /// has for the investigation.
    async fn replace_workflow(
        &self,
        id: &InvestigationId,
        events: Vec<RecordedEvent>,
    ) -> Result<()> {
        self.start_workflow_with(id, events, WorkflowIdConflictPolicy::TerminateExisting)
            .await
    }

    /// Bring every projection up to the engine's log, drain the workers, then
    /// stop the engine (which writes its snapshot).
    pub async fn shutdown(&self) -> Result<()> {
        let Some(parts) = self.parts.lock().await.take() else {
            return Ok(());
        };
        // While the worker still answers queries. The next engine release may
        // not read this engine's snapshot, and then rebuilds from projections.
        if tokio::time::timeout(PROJECTION_REFRESH_TIMEOUT, self.refresh_projections())
            .await
            .is_err()
        {
            tracing::warn!("stopping before every projection was brought up to date");
        }
        parts.app.shutdown().await?;
        if let Some(odori) = parts.odori {
            odori.shutdown().await?;
        }
        parts
            .engine
            .shutdown()
            .await
            .map_err(|e| anyhow!("engine shutdown: {e}"))?;
        Ok(())
    }

    /// Apply a command once. A submission sent again under its id, as when the
    /// engine reports an update unprocessed after all, gets the reply it already
    /// had; the engine is also given the id as the update's, to deduplicate.
    pub async fn submit(
        &self,
        id: &InvestigationId,
        submission: Submission,
    ) -> Result<CommandReceipt, ServiceError> {
        // Large content goes to the blob store; the workflow carries its hash.
        let mut submission = submission;
        kyumei_app::blob::externalize(&mut submission.command, &self.blobs)
            .map_err(|e| ServiceError::Runtime(format!("could not store the submission: {e}")))?;
        let handle = self
            .client
            .get_workflow_handle::<InvestigationWorkflow>(id.to_string());
        let options = WorkflowExecuteUpdateOptions::builder()
            .update_id(submission.id.clone())
            .build();
        let mut resent = 0;
        let reply = loop {
            match handle
                .execute_update(
                    InvestigationWorkflow::submit_once,
                    submission.clone(),
                    options.clone(),
                )
                .await
            {
                // The engine rejects an update as unprocessed now and then,
                // sometimes after applying it. Sending it again under the same
                // id returns the reply it had, so nothing is applied twice.
                Err(e) if unprocessed(&e) && resent < UNPROCESSED_RESENDS => {
                    resent += 1;
                    tracing::warn!(
                        investigation = %id,
                        resent,
                        "the engine rejected a command's update as unprocessed; sending it again"
                    );
                    tokio::time::sleep(Duration::from_millis(50 * u64::from(resent))).await;
                }
                other => break other.map_err(|e| ServiceError::Runtime(e.to_string()))?,
            }
        };
        match reply {
            CommandReply::Applied { events, evidence } => {
                if let Ok(view) = self.view(id).await {
                    self.keep_projection(id, &view.events);
                }
                Ok(CommandReceipt { events, evidence })
            }
            CommandReply::Rejected { reason } => {
                Err(ServiceError::Command(core::CommandError::Invalid(reason)))
            }
        }
    }

    pub fn explainer(&self) -> Option<&str> {
        self.explain.as_ref().map(|e| e.provider.as_str())
    }

    async fn start_workflow(&self, id: &InvestigationId, events: Vec<RecordedEvent>) -> Result<()> {
        self.start_workflow_with(id, events, WorkflowIdConflictPolicy::UseExisting)
            .await
    }

    async fn start_workflow_with(
        &self,
        id: &InvestigationId,
        events: Vec<RecordedEvent>,
        conflict: WorkflowIdConflictPolicy,
    ) -> Result<()> {
        self.client
            .start_workflow(
                InvestigationWorkflow::run,
                InvestigationInput {
                    events,
                    explain: self.explain.clone(),
                },
                WorkflowStartOptions::new(APP_QUEUE, id.to_string())
                    .id_conflict_policy(conflict)
                    .build(),
            )
            .await?;
        Ok(())
    }

    async fn view(&self, id: &InvestigationId) -> Result<WorkflowView, WorkflowQueryError> {
        let handle = self
            .client
            .get_workflow_handle::<InvestigationWorkflow>(id.to_string());
        let mut attempt = 1;
        loop {
            let options = WorkflowQueryOptions::builder()
                .rpc_options(RpcOptions::builder().timeout(VIEW_QUERY_TIMEOUT).build())
                .build();
            match handle.query(InvestigationWorkflow::view, (), options).await {
                // A query changes nothing, so sending it again is safe.
                Err(e) if transient(&e) && attempt < VIEW_QUERY_ATTEMPTS => {
                    tracing::warn!(
                        investigation = %id,
                        attempt,
                        "the investigation's view query failed; sending it again: {e}"
                    );
                    tokio::time::sleep(Duration::from_millis(200 * u64::from(attempt))).await;
                    attempt += 1;
                }
                other => return other,
            }
        }
    }

    fn projection_path(&self, id: &InvestigationId) -> PathBuf {
        self.config
            .data_dir
            .join("projections")
            .join(format!("{id}.json"))
    }

    /// Write the projection when the engine's log, `events`, holds more than
    /// the projection on disk; a shorter log never replaces a longer one.
    fn keep_projection(&self, id: &InvestigationId, events: &[RecordedEvent]) {
        // One writer at a time, so two writes of a projection never interleave
        // and the longer log wins.
        let mut projections = self.projections.lock().unwrap_or_else(|p| p.into_inner());
        let on_disk = *projections
            .entry(id.clone())
            .or_insert_with(|| self.read_projection(id).map_or(0, |e| e.len()));
        if events.len() > on_disk && self.write_projection(id, events) {
            projections.insert(id.clone(), events.len());
        }
    }

    /// Bring every projection up to the engine's log, reading each projection
    /// from disk again.
    async fn refresh_projections(&self) {
        self.projections
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        for id in self.indexed() {
            if let Ok(view) = self.view(&id).await {
                self.keep_projection(&id, &view.events);
            }
        }
    }

    fn write_projection(&self, id: &InvestigationId, events: &[RecordedEvent]) -> bool {
        let path = self.projection_path(id);
        let write = || -> Result<()> {
            std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(events)?)?;
            std::fs::rename(tmp, &path)?;
            Ok(())
        };
        match write() {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(investigation = %id, "projection not written: {e}");
                false
            }
        }
    }

    fn read_projection(&self, id: &InvestigationId) -> Option<Vec<RecordedEvent>> {
        serde_json::from_slice(&std::fs::read(self.projection_path(id)).ok()?).ok()
    }

    fn indexed(&self) -> Vec<InvestigationId> {
        self.index
            .lock()
            .map(|i| i.investigations.clone())
            .unwrap_or_default()
    }

    fn save_index(&self) {
        let write = || -> anyhow::Result<()> {
            let json = {
                let index = self.index.lock().unwrap_or_else(|p| p.into_inner());
                serde_json::to_vec_pretty(&*index)?
            };
            let path = self.config.data_dir.join("index.json");
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, json)?;
            std::fs::rename(tmp, path)?;
            Ok(())
        };
        if let Err(e) = write() {
            tracing::warn!("investigation index not written: {e}");
        }
    }
}

/// How often a command's update is sent again after the engine rejected it
/// as unprocessed.
const UNPROCESSED_RESENDS: u32 = 3;

/// How long stopping waits for the projections to be brought up to date.
const PROJECTION_REFRESH_TIMEOUT: Duration = Duration::from_secs(30);

/// How long one query for an investigation's view may wait, and how many
/// times it is sent. Under load, the embedded engine can leave a query
/// unanswered until the client's default 30-second deadline; a fresh query is
/// answered.
const VIEW_QUERY_TIMEOUT: Duration = Duration::from_secs(10);
const VIEW_QUERY_ATTEMPTS: u32 = 3;

/// Whether a failed query is worth sending again: it ran out of time, or the
/// engine was briefly unavailable. The client reports its own deadline as
/// Cancelled ("Timeout expired"), the engine's as DeadlineExceeded.
fn transient(error: &WorkflowQueryError) -> bool {
    matches!(
        error,
        WorkflowQueryError::Rpc(status)
            if matches!(
                status.code(),
                Code::DeadlineExceeded | Code::Cancelled | Code::Unavailable
            )
    )
}

/// Whether the engine rejected an update without running any handler: the
/// workflow task that carried it completed without accepting or rejecting it
/// (`UnprocessedUpdate`). The embedded engine does this now and then under
/// load, while the workflow is also running an explanation.
fn unprocessed(error: &WorkflowUpdateError) -> bool {
    use temporalio_common::protos::temporal::api::failure::v1::failure::FailureInfo;
    matches!(
        error,
        WorkflowUpdateError::Failed(failure)
            if matches!(
                &failure.failure_info,
                Some(FailureInfo::ApplicationFailureInfo(info)) if info.r#type == "UnprocessedUpdate"
            )
    )
}

#[async_trait]
impl InvestigationService for DurableInvestigations {
    async fn open(&self, cmd: OpenInvestigation) -> Result<InvestigationId, ServiceError> {
        // Opening is deterministic: compute the first events here, then hand
        // them to a new workflow, which owns the investigation from now on.
        let investigation = core::open(&self.kb, &cmd, jiff::Timestamp::now())?;
        let id = investigation.id.clone();
        self.start_workflow(&id, investigation.events.clone())
            .await
            .map_err(|e| ServiceError::Runtime(e.to_string()))?;
        if let Ok(mut index) = self.index.lock()
            && !index.investigations.contains(&id)
        {
            index.investigations.push(id.clone());
        }
        self.save_index();
        self.keep_projection(&id, &investigation.events);
        Ok(id)
    }

    async fn command(
        &self,
        id: &InvestigationId,
        command: Command,
    ) -> Result<CommandReceipt, ServiceError> {
        self.submit(
            id,
            Submission {
                id: uuid::Uuid::new_v4().to_string(),
                command,
            },
        )
        .await
    }

    async fn snapshot(&self, id: &InvestigationId) -> Result<InvestigationSnapshot, ServiceError> {
        let view = self.view(id).await.map_err(|e| match e {
            WorkflowQueryError::NotFound(_) => ServiceError::NotFound(id.clone()),
            e => ServiceError::Runtime(format!("could not read investigation {id}: {e}")),
        })?;
        self.keep_projection(id, &view.events);
        let investigation = Investigation::replay(&view.events)
            .map_err(|e| ServiceError::Runtime(e.to_string()))?;
        // Show a model explanation only when it covers the latest assessment.
        let latest = investigation.assessments.last().map(|(seq, _)| *seq);
        let model_explanation = match &view.explanation_status {
            ExplanationStatus::Current { seq } if Some(*seq) == latest => view.explanation.clone(),
            _ => None,
        };
        let explanation = match (&view.explanation_status, latest) {
            (ExplanationStatus::Current { seq }, Some(l)) if *seq != l => {
                ExplanationStatus::Pending { seq: l }
            }
            (status, _) => status.clone(),
        };
        Ok(InvestigationSnapshot {
            investigation,
            model_explanation,
            explanation,
            runtime: RuntimeInfo {
                kind: "odori".to_owned(),
                workflow_id: Some(id.to_string()),
                note: self.notes.lock().ok().and_then(|n| n.get(id).cloned()),
            },
        })
    }

    async fn list(&self) -> Result<Vec<InvestigationSummary>, ServiceError> {
        let mut rows = Vec::new();
        for id in self.indexed() {
            let events = match self.view(&id).await {
                Ok(v) => {
                    self.keep_projection(&id, &v.events);
                    v.events
                }
                Err(_) => match self.read_projection(&id) {
                    Some(e) => e,
                    None => continue,
                },
            };
            if let Ok(i) = Investigation::replay(&events) {
                rows.push(InvestigationSummary::of(&i));
            }
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        Ok(rows)
    }

    fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    fn describe(&self) -> String {
        let explainer = self
            .explain
            .as_ref()
            .map_or("deterministic explanations".to_owned(), |e| {
                format!("explanations by {}", e.provider)
            });
        let simulations = if self.config.simulate {
            format!(
                ", simulations by tempdes {}",
                &kyumei_app::simulation::commit()[..7]
            )
        } else {
            String::new()
        };
        format!(
            "Odori durable workflows on the embedded tokeira engine ({}; snapshot every {}s), {explainer}{simulations}",
            self.config.data_dir.display(),
            self.config.snapshot_interval.as_secs()
        )
    }
}

/// Start the embedded engine, restoring its snapshot.
///
/// An engine that cannot read the snapshot (one written by an older engine
/// release, or a damaged file) fails during runtime restoration and does not
/// say why. The snapshot is then kept under a new name, never deleted, and
/// the engine starts empty: every investigation is restored from its
/// projection, which holds every applied event. Returns where the unreadable
/// snapshot was kept.
async fn start_engine(snapshot: &Path, interval: Duration) -> Result<(Engine, Option<PathBuf>)> {
    let config = || {
        let mut server = TokeiraConfig::default();
        server.policy.snapshot = Some(SnapshotPolicyConfig {
            location: snapshot.to_path_buf(),
            interval_ms: interval.as_millis().max(1) as u64,
        });
        EmbeddedEngineConfig {
            server,
            storage: EmbeddedStorageConfig::InMemory,
            ..EmbeddedEngineConfig::default()
        }
    };
    let not_started =
        |e: EmbeddedEngineStartError| anyhow!("the embedded engine did not start: {e}");
    match Engine::start_with_embedded_config(config()).await {
        Ok(engine) => Ok((engine, None)),
        Err(EmbeddedEngineStartError::Phase {
            phase: EmbeddedStartupPhase::RuntimeRestore,
        }) if snapshot.exists() => {
            let kept = snapshot.with_file_name(format!(
                "engine.snapshot.unreadable-{}",
                jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ")
            ));
            std::fs::rename(snapshot, &kept)
                .with_context(|| format!("keep the unreadable {}", snapshot.display()))?;
            tracing::warn!(
                kept = %kept.display(),
                "the embedded engine could not read its snapshot; starting empty and restoring investigations from their projections"
            );
            let engine = Engine::start_with_embedded_config(config())
                .await
                .map_err(not_started)?;
            Ok((engine, Some(kept)))
        }
        Err(e) => Err(not_started(e)),
    }
}

/// Whether `longer` is `shorter` followed by more events.
fn extends(longer: &[RecordedEvent], shorter: &[RecordedEvent]) -> bool {
    longer.len() > shorter.len() && longer.iter().zip(shorter).all(|(a, b)| a.seq == b.seq)
}

/// Kyumei's worker on its own OS thread: workflow futures are `!Send`, so the
/// worker runs on a current-thread runtime (the same shape as Odori's).
struct AppWorker {
    shutdown: Box<dyn Fn() + Send + Sync>,
    thread: std::thread::JoinHandle<Result<()>>,
}

impl AppWorker {
    async fn start<F>(name: &str, client: Client, build_options: F) -> Result<Self>
    where
        F: FnOnce() -> Result<WorkerOptions> + Send + 'static,
    {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Box<dyn Fn() + Send + Sync>>();
        let thread =
            std::thread::Builder::new()
                .name(name.to_owned())
                .spawn(move || -> Result<()> {
                    let local = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .context("build worker runtime")?;
                    local.block_on(async move {
                        let runtime = Runtime::from_current_tokio(Default::default())?;
                        let options = build_options()?;
                        let mut worker = Worker::new(&runtime, client, options)?;
                        if ready_tx.send(Box::new(worker.shutdown_handle())).is_err() {
                            return Ok(());
                        }
                        // A worker that stops on its own leaves commands
                        // waiting; say so when it happens, not at shutdown.
                        if let Err(e) = worker.run().await {
                            tracing::error!("the investigation worker stopped: {e:#}");
                            return Err(e.into());
                        }
                        tracing::info!("the investigation worker stopped");
                        Ok(())
                    })
                })?;
        let shutdown = ready_rx
            .await
            .context("the investigation worker died before it was ready")?;
        Ok(Self { shutdown, thread })
    }

    async fn shutdown(self) -> Result<()> {
        (self.shutdown)();
        let thread = self.thread;
        tokio::task::spawn_blocking(move || {
            thread
                .join()
                .map_err(|_| anyhow!("worker thread panicked"))?
        })
        .await??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use temporalio_common::protos::temporal::api::failure::v1::{
        ApplicationFailureInfo, Failure, failure::FailureInfo,
    };

    use super::*;

    fn failed(kind: &str) -> WorkflowUpdateError {
        WorkflowUpdateError::Failed(Box::new(Failure {
            message: "Workflow Update is rejected because it wasn't processed by worker.".into(),
            failure_info: Some(FailureInfo::ApplicationFailureInfo(
                ApplicationFailureInfo {
                    r#type: kind.into(),
                    non_retryable: true,
                    ..Default::default()
                },
            )),
            ..Default::default()
        }))
    }

    #[test]
    fn only_unprocessed_updates_are_sent_again() {
        assert!(unprocessed(&failed("UnprocessedUpdate")));
        assert!(!unprocessed(&failed("CommandRejected")));
        assert!(!unprocessed(&WorkflowUpdateError::Other(
            "the engine is shut down".into()
        )));
    }
}
