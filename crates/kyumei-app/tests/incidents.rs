//! Simulated incidents, each with one known cause, worked by Kyumei the way an
//! operator with Collect now works one. Each must end in the cause that was
//! injected.
//!
//! A run covers 20 minutes before the incident window, the five-minute window
//! itself and five minutes after it, the most any request pads its window by.
//! Requests with PromQL are answered as Collect now answers them. The Matching
//! queue request is answered with a DescribeTaskQueue capture taken from the
//! scrapes near the end of the window, and the partition request with the
//! scenario's dynamic configuration, as an operator would paste them.
//!
//! Set `KYUMEI_DEBUG_INCIDENT` to print each piece of evidence as Kyumei read it.

use std::{collections::BTreeMap, sync::Arc};

use jiff::{SignedDuration, Timestamp};
use kyumei_app::{
    InvestigationService, LocalInvestigations,
    collect::CollectionPlan,
    core::{self, Command, EvidenceSubmission, OpenInvestigation, SdkInput},
    fixture::FixtureClock,
};
use kyumei_domain::{
    DeclaredContext, EvidenceForm, EvidenceOrigin, EvidenceRequest, HypothesisState,
    InvestigationId, SubmissionChannel, TimeWindow,
};
use kyumei_knowledge::KnowledgeBase;
use kyumei_sim::{
    Run, Simulation, TEMPDES_COMMIT,
    promql::{self, Store},
};

/// When the simulation starts, on the wall clock.
const ORIGIN: &str = "2026-10-15T12:00:00Z";
const WARMUP: SignedDuration = SignedDuration::from_secs(30);
/// The incident window starts this long after the warm-up, so that the
/// longest pad before it is simulated too.
const BEFORE: SignedDuration = SignedDuration::from_mins(20);
const WINDOW: SignedDuration = SignedDuration::from_mins(5);
const AFTER: SignedDuration = SignedDuration::from_mins(5);

/// A cluster with one cause of slow Workflow Tasks.
struct Incident {
    name: &'static str,
    processes: u32,
    workflow_pollers: u32,
    workflow_slots: u32,
    start_rate: u32,
    /// How long a Worker takes over one Workflow Task: p50 and p99.
    wft_processing: (&'static str, &'static str),
    read_partitions: u32,
    write_partitions: u32,
    /// `admin.matchingNamespaceTaskqueueToPartitionDispatchRate`, when set.
    partition_dispatch_rate: Option<u32>,
    /// More dynamic configuration, in Temporal's format.
    extra_dynamic_config: &'static str,
    /// Signals per second to running workflows, steady throughout.
    signal_rate: Option<u32>,
    /// Signals per second from the start of the incident window.
    signal_surge: Option<u32>,
    /// The workflow's steps, in tempdes's format.
    steps: &'static str,
}

impl Incident {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            processes: 2,
            workflow_pollers: 4,
            workflow_slots: 100,
            start_rate: 40,
            wft_processing: ("2ms", "12ms"),
            read_partitions: 4,
            write_partitions: 4,
            partition_dispatch_rate: None,
            extra_dynamic_config: "",
            signal_rate: None,
            signal_surge: None,
            steps: "      - activity: { count: 1, duration: { p50: 30ms, p99: 250ms } }\n",
        }
    }

    fn origin() -> Timestamp {
        ORIGIN.parse().expect("origin")
    }

    fn window() -> (Timestamp, Timestamp) {
        let start = Self::origin() + WARMUP + BEFORE;
        (start, start + WINDOW)
    }

    /// The partition settings as the operator's dynamic configuration has them.
    fn dynamic_config(&self) -> String {
        let constraints =
            "    constraints:\n      namespace: \"orders\"\n      taskQueueName: \"orders\"\n";
        let mut dc = format!(
            "matching.numTaskqueueReadPartitions:\n  - value: {}\n{constraints}\
             matching.numTaskqueueWritePartitions:\n  - value: {}\n{constraints}",
            self.read_partitions, self.write_partitions
        );
        if let Some(rate) = self.partition_dispatch_rate {
            dc.push_str(&format!(
                "admin.matchingNamespaceTaskqueueToPartitionDispatchRate:\n  - value: {rate}\n{constraints}"
            ));
        }
        dc.push_str(self.extra_dynamic_config);
        dc
    }

    /// DescribeTaskQueue's effective limit: the per-partition dispatch rate
    /// times the read partitions, whether or not Matching applies it.
    fn effective_rate_limit(&self) -> u32 {
        self.partition_dispatch_rate.unwrap_or(1_000).min(10_000) * self.read_partitions
    }

    fn scenario(&self) -> String {
        let minutes = (BEFORE + WINDOW + AFTER).as_mins();
        let indented: String = self
            .dynamic_config()
            .lines()
            .map(|l| format!("  {l}\n"))
            .collect();
        let load = self.signal_rate.map_or(String::new(), |rate| {
            format!("load:\n  signals: [ {{ workflow_type: OrderWorkflow, rate: {rate}/s }} ]\n")
        });
        let events = self.signal_surge.map_or(String::new(), |rate| {
            format!(
                "events:\n  - {{ at: {}s, signal_rate: {{ workflow_type: OrderWorkflow, rate: {rate}/s }}, label: signals surge }}\n",
                (WARMUP + BEFORE).as_secs()
            )
        });
        format!(
            r#"name: {name}
seed: 11
warmup: {warmup}s
duration: {minutes}m
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
{indented}namespaces:
  - name: orders
workers:
  - name: order-workers
    namespace: orders
    task_queue: orders
    processes: {processes}
    workflow_pollers: {pollers}
    activity_pollers: 4
    workflow_slots: {slots}
    activity_slots: 100
    sticky_cache_size: 0
workflows:
  - type: OrderWorkflow
    namespace: orders
    task_queue: orders
    start_rate: {rate}/s
    starters: 2
    wft_processing: {{ p50: {p50}, p99: {p99} }}
    steps:
{steps}{load}{events}"#,
            name = self.name,
            warmup = WARMUP.as_secs(),
            processes = self.processes,
            pollers = self.workflow_pollers,
            slots = self.workflow_slots,
            rate = self.start_rate,
            p50 = self.wft_processing.0,
            p99 = self.wft_processing.1,
            steps = self.steps,
        )
    }
}

/// How the investigation ended.
#[derive(Debug)]
struct Outcome {
    conclusion: Option<String>,
    concluded: bool,
    states: BTreeMap<String, HypothesisState>,
    answered: Vec<String>,
}

impl Outcome {
    fn state(&self, hypothesis: &str) -> HypothesisState {
        self.states[hypothesis]
    }

    /// Every answer settled its request: none had to be asked again.
    fn asked_once(&self) -> bool {
        let mut seen = std::collections::BTreeSet::new();
        self.answered.iter().all(|r| seen.insert(r))
    }
}

/// Simulate `incident`, then work its investigation to the end.
async fn work(incident: &Incident) -> Outcome {
    let scenario = incident.scenario();
    let run = tokio::task::spawn_blocking(move || Simulation::new(scenario).run())
        .await
        .expect("join")
        .expect("simulation");
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let (start, end) = Incident::window();
    let now = end + AFTER + SignedDuration::from_mins(1);
    let clock = Arc::new(FixtureClock::new(now));
    let service = LocalInvestigations::new(kb.clone(), clock);
    let id = service
        .open(OpenInvestigation {
            id: InvestigationId::new(format!("inv-sim-{}", incident.name)),
            title: None,
            description: format!(
                "Since about {} UTC, Workflow Tasks on the orders Task Queue (namespace orders) \
                 are waiting before a Worker picks them up.",
                start.strftime("%H:%M")
            ),
            signature: None,
            server_version: Some("1.31.0".to_owned()),
            sdks: vec![SdkInput {
                language: "go".to_owned(),
                version: Some("1.44.0".to_owned()),
            }],
            topology: None,
            namespaces: vec!["orders".to_owned()],
            task_queues: vec!["orders".to_owned()],
            window_start: Some(start),
            window_end: Some(end),
            window_description: None,
            profile: None,
        })
        .await
        .expect("open");

    let mut answered = Vec::new();
    for _ in 0..12 {
        let snapshot = service.snapshot(&id).await.expect("snapshot");
        let assessment =
            core::current_assessment(&kb, &snapshot.investigation).expect("assessment");
        if assessment.conclusion_final {
            break;
        }
        let Some(request) = snapshot.investigation.current_request().cloned() else {
            break;
        };
        let submission = answer(incident, &run, &request, now);
        service
            .command(&id, Command::SubmitEvidence(submission))
            .await
            .expect("submit");
        answered.push(request.definition.to_string());
    }
    let snapshot = service.snapshot(&id).await.expect("snapshot");
    let assessment = core::current_assessment(&kb, &snapshot.investigation).expect("assessment");
    if std::env::var("KYUMEI_DEBUG_INCIDENT").is_ok() {
        for e in &snapshot.investigation.evidence {
            eprintln!("== {} {:?}", e.id, e.interpretation.interpreter);
            for n in &e.interpretation.notes {
                eprintln!("   note {:?}: {}", n.severity, n.text);
            }
            for o in &e.interpretation.observations {
                eprintln!(
                    "   {} {:?} {:?} {}",
                    o.id,
                    o.binding.as_ref().map(|b| b.signal.to_string()),
                    o.subject.description,
                    o.value.brief()
                );
            }
        }
    }
    Outcome {
        conclusion: assessment.conclusion.as_ref().map(|c| c.id.to_string()),
        concluded: assessment.conclusion_final,
        states: assessment
            .hypotheses
            .iter()
            .map(|h| (h.id.to_string(), h.state))
            .collect(),
        answered,
    }
}

fn origin() -> EvidenceOrigin {
    EvidenceOrigin::Collector {
        name: "tempdes".to_owned(),
        version: TEMPDES_COMMIT[..7].to_owned(),
    }
}

/// What the operator would bring back for `request`.
fn answer(
    incident: &Incident,
    run: &Run,
    request: &EvidenceRequest,
    now: Timestamp,
) -> EvidenceSubmission {
    match request.definition.as_str() {
        "matching-queue-stats" => {
            // Captured half a minute before the window ends.
            let at = Incident::window().1 - SignedDuration::from_secs(30);
            pasted(
                request,
                describe_task_queue(incident, run, at),
                "describe-task-queue.json",
                Some(TimeWindow::new(at, at).expect("window")),
            )
        }
        "partition-config" => pasted(
            request,
            incident.dynamic_config(),
            "dynamicconfig.yaml",
            None,
        ),
        _ => {
            let plan = CollectionPlan::for_request(request).expect("a PromQL request");
            assert!(plan.missing().is_none(), "{:?}", plan.unfilled);
            let mut samples = run
                .collect(
                    &plan.queries,
                    plan.sample_window(),
                    Incident::origin().as_millisecond(),
                    now.as_millisecond(),
                )
                .expect("collect");
            samples.request = Some(request.id.to_string());
            EvidenceSubmission {
                declared: DeclaredContext {
                    form: Some(EvidenceForm::SampleBlock),
                    ..DeclaredContext::default()
                },
                responds_to: Some(request.id.clone()),
                media_type: Some("text/plain".to_owned()),
                origin: Some(origin()),
                ..EvidenceSubmission::text(
                    SubmissionChannel::Collector,
                    kyumei_samples::encode(&samples),
                )
            }
        }
    }
}

fn pasted(
    request: &EvidenceRequest,
    text: String,
    filename: &str,
    window: Option<TimeWindow>,
) -> EvidenceSubmission {
    EvidenceSubmission {
        declared: DeclaredContext {
            window,
            ..DeclaredContext::default()
        },
        filename: Some(filename.to_owned()),
        responds_to: Some(request.id.clone()),
        origin: Some(origin()),
        ..EvidenceSubmission::text(SubmissionChannel::Paste, text)
    }
}

/// One value of `query` at `at`, or zero when no series answers.
fn instant(store: &Store, query: &str, at_ms: i64) -> f64 {
    promql::evaluate_range(
        &promql::parse(query).expect("query"),
        store,
        at_ms,
        at_ms,
        1_000,
    )
    .expect("evaluate")
    .first()
    .and_then(|s| s.points.first())
    .map_or(0.0, |(_, v)| *v)
}

/// DescribeTaskQueue through grpcurl (`-emit-defaults`) at `at`, from what the
/// simulated Matching reported then.
fn describe_task_queue(incident: &Incident, run: &Run, at: Timestamp) -> String {
    let store = Store::new(&run.scrapes);
    let t = at.as_millisecond() - Incident::origin().as_millisecond();
    let queue = r#"namespace="orders", taskqueue="orders", task_type="Workflow""#;
    let backlog = instant(
        &store,
        &format!("sum(approximate_backlog_count{{{queue}}})"),
        t,
    );
    let age = instant(
        &store,
        &format!("max(approximate_backlog_age_seconds{{{queue}}})"),
        t,
    );
    let added = instant(
        &store,
        r#"sum(rate(service_requests{service_name="matching", operation="AddWorkflowTask"}[1m]))"#,
        t,
    );
    let dispatched = instant(
        &store,
        &format!("sum(rate(poll_success{{{queue}}}[1m]))"),
        t,
    );
    let seen = at.strftime("%Y-%m-%dT%H:%M:%SZ");
    let pollers: Vec<String> = (0..incident.processes)
        .map(|i| {
            format!(
                r#"{{ "lastAccessTime": "{seen}", "identity": "{}@order-workers-{i}", "ratePerSecond": 100000 }}"#,
                4100 + i
            )
        })
        .collect();
    let stats = format!(
        r#"{{ "approximateBacklogCount": "{backlog:.0}", "approximateBacklogAge": "{age:.3}s", "tasksAddRate": {added:.1}, "tasksDispatchRate": {dispatched:.1} }}"#
    );
    let pollers = format!("[{}]", pollers.join(", "));
    format!(
        r#"{{
  "pollers": {pollers},
  "stats": {stats},
  "statsByPriorityKey": {{ "3": {stats} }},
  "effectiveRateLimit": {{ "requestsPerSecond": {}, "rateLimitSource": "RATE_LIMIT_SOURCE_SYSTEM" }}
}}"#,
        incident.effective_rate_limit()
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn throttled_polls_are_found() {
    // Start and Respond calls reserve the namespace's budget; polls get what is left.
    let incident = Incident {
        extra_dynamic_config: "frontend.namespaceRPS:\n  - value: 200\n",
        ..Incident::new("poll-throttling")
    };
    let outcome = work(&incident).await;
    assert!(outcome.asked_once(), "{outcome:#?}");
    eprintln!("{}: {outcome:#?}", incident.name);
    assert_eq!(outcome.conclusion.as_deref(), Some("poll-throttling"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dispatch_ceiling_is_found() {
    // Two partitions at 50 tasks/s each, while about 120 Workflow Tasks a second arrive.
    let incident = Incident {
        start_rate: 60,
        read_partitions: 2,
        write_partitions: 2,
        partition_dispatch_rate: Some(50),
        signal_rate: Some(2),
        ..Incident::new("dispatch-ceiling")
    };
    let outcome = work(&incident).await;
    assert!(outcome.asked_once(), "{outcome:#?}");
    eprintln!("{}: {outcome:#?}", incident.name);
    assert_eq!(outcome.conclusion.as_deref(), Some("dispatch-ceiling"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn starved_pollers_are_found() {
    // One poller can't keep up with 300 Workflow Tasks a second, with slots to spare.
    let incident = Incident {
        processes: 1,
        workflow_pollers: 1,
        start_rate: 150,
        signal_rate: Some(2),
        ..Incident::new("poller-starvation")
    };
    let outcome = work(&incident).await;
    assert!(outcome.asked_once(), "{outcome:#?}");
    eprintln!("{}: {outcome:#?}", incident.name);
    assert_eq!(outcome.conclusion.as_deref(), Some("poller-starvation"));
    assert_eq!(
        outcome.state("poller-starvation"),
        HypothesisState::Established
    );
    // Not final: ruling the dispatch ceiling out takes three samples of the
    // dispatch rate, and one DescribeTaskQueue capture is one.
    assert_ne!(
        outcome.state("dispatch-rate-ceiling"),
        HypothesisState::Established
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signal_flood_against_a_dispatch_ceiling_is_found() {
    // The golden investigation: idle workflows signalled at 120/s from the
    // window's start, against two partitions at 50 tasks/s each. Workflows wait
    // half an hour, so most of those signalled are idle and each Signal
    // schedules a Workflow Task.
    let incident = Incident {
        start_rate: 20,
        read_partitions: 2,
        write_partitions: 2,
        partition_dispatch_rate: Some(50),
        signal_rate: Some(2),
        signal_surge: Some(120),
        steps: "      - activity: { count: 1, duration: { p50: 30ms, p99: 250ms } }\n      - timer: 30m\n",
        ..Incident::new("signal-flood")
    };
    let outcome = work(&incident).await;
    assert!(outcome.asked_once(), "{outcome:#?}");
    eprintln!("{}: {outcome:#?}", incident.name);
    assert_eq!(
        outcome.conclusion.as_deref(),
        Some("dispatch-ceiling-under-signal-flood")
    );
    assert_eq!(outcome.state("signal-flood"), HypothesisState::Established);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slots_that_never_free_are_found() {
    let incident = Incident {
        workflow_slots: 2,
        wft_processing: ("50ms", "150ms"),
        ..Incident::new("worker-slot-exhaustion")
    };
    let started = std::time::Instant::now();
    let outcome = work(&incident).await;
    assert!(outcome.asked_once(), "{outcome:#?}");
    eprintln!(
        "{} took {:?}: {outcome:#?}",
        incident.name,
        started.elapsed()
    );
    assert_eq!(
        outcome.conclusion.as_deref(),
        Some("worker-slot-exhaustion")
    );
    assert!(outcome.concluded, "{outcome:#?}");
    assert_eq!(
        outcome.state("worker-slot-exhaustion"),
        HypothesisState::Established
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn absent_pollers_are_found() {
    // Workflows start on a Task Queue no Worker polls, so every Worker metric
    // for it is missing and Matching's backlog only grows.
    let incident = Incident {
        processes: 0,
        start_rate: 5,
        ..Incident::new("pollers-absent")
    };
    let outcome = work(&incident).await;
    eprintln!("{}: {outcome:#?}", incident.name);
    assert!(outcome.asked_once(), "{outcome:#?}");
    assert_eq!(outcome.conclusion.as_deref(), Some("pollers-absent"));
    assert!(outcome.concluded, "{outcome:#?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_arrival_at_matching_is_found() {
    // History's transfer queue readers may poll twice a second across the host's
    // 64 shards, so each shard's Workflow Tasks reach Matching up to half a
    // minute after History schedules them. Matching hands them straight out.
    let incident = Incident {
        extra_dynamic_config: "history.transferProcessorMaxPollHostRPS:\n  - value: 2\n",
        ..Incident::new("history-dispatch-delay")
    };
    let outcome = work(&incident).await;
    eprintln!("{}: {outcome:#?}", incident.name);
    assert!(outcome.asked_once(), "{outcome:#?}");
    assert_eq!(
        outcome.conclusion.as_deref(),
        Some("history-dispatch-delay")
    );
    // Not final: one DescribeTaskQueue capture can't rule out the dispatch ceiling.
    assert_eq!(
        outcome.state("dispatch-rate-ceiling"),
        HypothesisState::Candidate
    );
}
