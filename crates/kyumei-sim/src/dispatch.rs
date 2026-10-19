//! A Task Queue held to a dispatch rate limit, and nothing else.
//!
//! The scenario asks how long Workflow Tasks wait when Matching dispatches them
//! at the Task Queue's limit while they are added faster, with Workers, pollers
//! and slots to spare. Each workflow is a single Workflow Task, so the start
//! rate is the add rate.
//!
//! The wait a rate limit causes depends on how far adds exceed it and for how
//! long, not on the rates themselves: the backlog grows by the excess, and
//! drains at the limit. So the run divides both rates by the same factor to
//! stay small, keeping their ratio, and reports the factor.

use std::time::{Duration, Instant};

use crate::{
    Run, RunError, Simulation,
    promql::{self, Store},
};

/// The highest dispatch limit a run simulates; larger ones are scaled down to it.
const SIMULATED_LIMIT: f64 = 100.0;
/// Simulated time before the measured window, kept short so that the backlog
/// starts growing when the window does.
const WARMUP_S: u64 = 10;
const NAMESPACE: &str = "kyumei";
const TASK_QUEUE: &str = "kyumei";

/// What the evidence says about the Task Queue.
#[derive(Clone, Debug, PartialEq)]
pub struct DispatchCase {
    /// Workflow Tasks added per second.
    pub add_rate: f64,
    /// The Task Queue's effective dispatch limit, in tasks per second.
    pub dispatch_limit: f64,
    /// The read partitions the limit is shared across.
    pub read_partitions: u32,
    /// How long adds ran at that rate: the measured window, in seconds.
    pub duration_s: u64,
    pub seed: u64,
}

/// What the run found.
#[derive(Clone, Debug, PartialEq)]
pub struct DispatchResult {
    /// The scenario, in tempdes's format.
    pub scenario: String,
    pub seed: u64,
    /// Both rates were divided by this.
    pub scale: f64,
    /// Workflow Task schedule-to-start, the mean over a minute, at its highest
    /// in the window, in seconds. Tasks leave a rate-limited backlog in the
    /// order they arrived, so a minute's waits are close together and the mean
    /// stands for its 95th percentile without a histogram's bucket bounds.
    pub schedule_to_start_max: f64,
    /// The age of Matching's oldest backlogged task at its highest, in seconds.
    pub backlog_age_max: f64,
    /// Tasks added and dispatched per second over the window's last minute,
    /// at the run's scale.
    pub add_rate: f64,
    pub dispatch_rate: f64,
    /// Real time the run took.
    pub wall: Duration,
}

impl DispatchCase {
    /// The factor both rates are divided by.
    pub fn scale(&self) -> f64 {
        (self.dispatch_limit / SIMULATED_LIMIT).max(1.0)
    }

    /// The scenario, in tempdes's format.
    pub fn scenario(&self) -> String {
        let scale = self.scale();
        let partitions = self.read_partitions.max(1);
        let per_partition = self.dispatch_limit / scale / f64::from(partitions);
        let start_rate = self.add_rate / scale;
        let constraints = format!(
            "      constraints:\n        namespace: \"{NAMESPACE}\"\n        taskQueueName: \"{TASK_QUEUE}\"\n"
        );
        format!(
            r#"name: task-queue-dispatch
seed: {seed}
warmup: {WARMUP_S}s
duration: {duration}s
cluster:
  num_history_shards: 64
  replicas: {{ frontend: 1, history: 1, matching: 1, worker: 1 }}
  resources:
    frontend: {{ cpu: 2 }}
    history: {{ cpu: 2 }}
    matching: {{ cpu: 2 }}
    worker: {{ cpu: 1 }}
  network: {{ client_rtt: 2ms, internal_rtt: 0.5ms }}
  persistence:
    store: postgresql
    max_conns: {{ frontend: 20, history: 50, matching: 30, worker: 10 }}
    capacity: 80
    latency:
      UpdateWorkflowExecution: {{ p50: 3ms, p99: 14ms }}
      CreateWorkflowExecution: {{ p50: 4ms, p99: 18ms }}
  visibility: {{ store: elasticsearch }}
dynamic_config:
  matching.numTaskqueueReadPartitions:
    - value: {partitions}
{constraints}  matching.numTaskqueueWritePartitions:
    - value: {partitions}
{constraints}  admin.matchingNamespaceTaskqueueToPartitionDispatchRate:
    - value: {per_partition}
{constraints}namespaces:
  - name: {NAMESPACE}
workers:
  - name: workers
    namespace: {NAMESPACE}
    task_queue: {TASK_QUEUE}
    processes: 4
    workflow_pollers: 16
    activity_pollers: 1
    workflow_slots: 1000
    activity_slots: 10
    sticky_cache_size: 0
workflows:
  - type: Probe
    namespace: {NAMESPACE}
    task_queue: {TASK_QUEUE}
    start_rate: {start_rate}/s
    starters: 4
    wft_processing: {{ p50: 2ms, p99: 12ms }}
"#,
            seed = self.seed,
            duration = self.duration_s.max(60),
            per_partition = round(per_partition),
            start_rate = round(start_rate),
        )
    }

    /// Simulate the case, stopping after `budget` of real time.
    pub fn run(&self, budget: Duration) -> Result<DispatchResult, RunError> {
        let started = Instant::now();
        let scenario = self.scenario();
        let run = Simulation::new(scenario.clone())
            .seed(self.seed)
            .budget(budget)
            .run()?;
        let read = |query: &str, from_ms: i64| -> Vec<f64> { values(&run, query, from_ms) };
        let start = run.warmup_ms + 60_000;
        let end_minute = run.warmup_ms + run.duration_ms - 60_000;
        let queue =
            format!(r#"namespace="{NAMESPACE}", taskqueue="{TASK_QUEUE}", task_type="Workflow""#);
        let max = |v: Vec<f64>| v.into_iter().fold(0.0_f64, f64::max);
        let last = |v: Vec<f64>| v.last().copied().unwrap_or(0.0);
        Ok(DispatchResult {
            seed: self.seed,
            scale: self.scale(),
            schedule_to_start_max: max(mean_waits(&run, start)),
            backlog_age_max: max(read(
                &format!("max(approximate_backlog_age_seconds{{{queue}}})"),
                start,
            )),
            add_rate: last(read(
                r#"sum(rate(service_requests{service_name="matching", operation="AddWorkflowTask"}[1m]))"#,
                end_minute,
            )),
            dispatch_rate: last(read(
                &format!("sum(rate(poll_success{{{queue}}}[1m]))"),
                end_minute,
            )),
            scenario,
            wall: started.elapsed(),
        })
    }
}

/// The values of `query` every 15 s from `from_ms` to the end of the run.
fn values(run: &Run, query: &str, from_ms: i64) -> Vec<f64> {
    let Ok(expr) = promql::parse(query) else {
        return Vec::new();
    };
    let end = run.warmup_ms + run.duration_ms;
    promql::evaluate_range(&expr, &Store::new(&run.scrapes), from_ms, end, 15_000)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|s| s.points.into_iter().map(|(_, v)| v))
        .filter(|v| v.is_finite())
        .collect()
}

/// Each minute's mean Workflow Task schedule-to-start, every 15 s from `from_ms`.
fn mean_waits(run: &Run, from_ms: i64) -> Vec<f64> {
    let series = |part: &str| {
        let query = format!(
            r#"sum(rate(temporal_workflow_task_schedule_to_start_latency_{part}{{namespace="{NAMESPACE}", task_queue="{TASK_QUEUE}"}}[1m]))"#
        );
        values(run, &query, from_ms)
    };
    series("sum")
        .into_iter()
        .zip(series("count"))
        .filter(|(_, count)| *count > 0.0)
        .map(|(sum, count)| sum / count)
        .collect()
}

/// A rate as the scenario writes it: to three decimal places.
fn round(rate: f64) -> f64 {
    (rate * 1_000.0).round() / 1_000.0
}
