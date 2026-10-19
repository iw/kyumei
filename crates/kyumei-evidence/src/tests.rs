use std::collections::BTreeSet;

use jiff::Timestamp;
use kyumei_domain::{
    CollectionInstruction, CollectionMethod, EvidenceRequest, RequestStatus, RequestedWindow,
};
use kyumei_knowledge::KnowledgeBase;

use super::*;

const PACK: &str = r#"
[pack]
id = "t-1.31"
title = "t"
version = "0"
server_family = "1.31"
verified_server_versions = ["1.31.0"]
description = "t"

[[source]]
id = "src"
kind = "repository"
title = "t"
commit = "abc"
retrieved = 2026-09-30

[[metric]]
id = "sdk.slots"
name = "worker_task_slots_available"
category = "sdk-metric"
instrument = "gauge"
purpose = "free slots"
labels = [{ name = "worker_type", values = ["WorkflowWorker", "ActivityWorker"] }, { name = "task_queue" }]
exposition = [{ name = "temporal_worker_task_slots_available", sdk_family = "core", condition = "default" }]
scope = { sdk = [{ family = "core", revision = "x" }] }
verification = { status = "verified", method = "source-inspection", on = 2026-09-30 }
provenance = [{ source = "src", path = "a", line = 1 }]

[[metric]]
id = "sdk.sts"
name = "workflow_task_schedule_to_start_latency"
category = "sdk-metric"
instrument = "histogram"
purpose = "sts"
labels = [{ name = "task_queue" }]
exposition = [{ name = "temporal_workflow_task_schedule_to_start_latency", sdk_family = "core", unit = "milliseconds", condition = "default" }]
scope = { sdk = [{ family = "core", revision = "x" }] }
verification = { status = "verified", method = "source-inspection", on = 2026-09-30 }
provenance = [{ source = "src", path = "a", line = 2 }]

[[metric]]
id = "srv.requests"
name = "service_requests"
category = "frontend-metric"
instrument = "counter"
purpose = "requests"
labels = [{ name = "operation" }]
exposition = [{ name = "service_requests", condition = "default" }]
scope = { server = ["1.31.0"] }
verification = { status = "verified", method = "source-inspection", on = 2026-09-30 }
provenance = [{ source = "src", path = "a", line = 3 }]

[[metric]]
id = "srv.operation"
name = "operation"
category = "server-metric"
instrument = "counter"
purpose = "a metric whose name is also a common label name"
exposition = [{ name = "operation", condition = "default" }]
scope = { server = ["1.31.0"] }
verification = { status = "verified", method = "source-inspection", on = 2026-09-30 }
provenance = [{ source = "src", path = "a", line = 6 }]

[[api_field]]
id = "api.backlog_age"
message = "temporal.api.taskqueue.v1.TaskQueueStats"
field = "approximate_backlog_age"
json_name = "approximateBacklogAge"
json_path = "stats.approximateBacklogAge"
field_type = "google.protobuf.Duration"
unit = "seconds"
purpose = "age"
scope = { server = ["1.31.0"] }
verification = { status = "verified", method = "source-inspection", on = 2026-09-30 }
provenance = [{ source = "src", path = "a", line = 4 }]

[[api_field]]
id = "api.rate_limiting_active"
message = "temporal.api.taskqueue.v1.TaskQueueStats"
field = "rate_limiting_active"
json_name = "rateLimitingActive"
json_path = "stats.rateLimitingActive"
field_type = "bool"
unit = "flag"
purpose = "rate limiting"
scope = { server = ["1.31.0"] }
verification = { status = "verified", method = "source-inspection", on = 2026-09-30 }
provenance = [{ source = "src", path = "a", line = 7 }]

[[dynamic_config]]
id = "dc.read"
key = "matching.numTaskqueueReadPartitions"
setting = "NewTaskQueueIntSetting"
value_type = "int"
default = "4"
constrained_by = "task-queue"
description = "read partitions"
scope = { server = ["1.31.0"] }
verification = { status = "verified", method = "source-inspection", on = 2026-09-30 }
provenance = [{ source = "src", path = "a", line = 5 }]
"#;

const DIAG: &str = r#"
[diagnostic]
id = "d"
title = "d"
summary = "d"
path = { description = "p", layers = ["workers"] }

[[symptom]]
id = "sts"
title = "sts"
patterns = ["schedule-to-start"]
signal = "sts"

[[stage]]
id = "s"
order = 1
title = "s"
question = "q"

[[signal]]
id = "slots"
title = "Workflow slots"
unit = "count"
description = "d"
source = [{ claim = "sdk.slots", labels = { worker_type = "WorkflowWorker" } }]

[[signal]]
id = "sts"
title = "STS"
unit = "seconds"
description = "d"
source = [{ claim = "sdk.sts" }]

[[signal]]
id = "signal_rate"
title = "Signal rate"
unit = "per_second"
description = "d"
source = [{ claim = "srv.requests", labels = { operation = "SignalWorkflowExecution" }, transform = "rate" }]

[[signal]]
id = "start_rate"
title = "Start rate"
unit = "per_second"
description = "d"
source = [{ claim = "srv.requests", labels = { operation = "StartWorkflowExecution" }, transform = "rate" }]

[[signal]]
id = "backlog_age"
title = "Backlog age"
unit = "seconds"
description = "d"
source = [{ claim = "api.backlog_age" }]

[[signal]]
id = "rate_limited"
title = "Rate limiting active"
unit = "flag"
description = "d"
source = [{ claim = "api.rate_limiting_active" }]

[[signal]]
id = "read_partitions"
title = "Read partitions"
unit = "count"
description = "d"
source = [{ claim = "dc.read" }]

[[hypothesis]]
id = "h"
stage = "s"
title = "h"
statement = "h"
question = "h?"
layer = "workers"
primary = true

[[request]]
id = "r"
stage = "s"
title = "r"
why = "w"
signals = ["slots", "sts", "signal_rate", "backlog_age", "rate_limited", "read_partitions"]
distinguishes = ["h"]
cost = 1
collect = [{ method = "promql", label = "l", body = "b" }]

[[rule]]
id = "r1"
hypothesis = "h"
effect = "eliminates"
statement = "x"
when = { signal = "slots", stat = "min", op = "ge", value = 5 }
"#;

fn kb() -> KnowledgeBase {
    KnowledgeBase::from_files(&[("t/pack.toml", PACK), ("t/diag.toml", DIAG)]).expect("test pack")
}

fn at() -> Timestamp {
    "2026-09-30T13:52:00Z".parse().expect("ts")
}

fn paste(text: &str) -> Submission {
    Submission {
        channel: SubmissionChannel::Paste,
        body: SubmissionBody::Text(text.to_owned()),
        filename: None,
        media_type: None,
        declared: DeclaredContext::default(),
        responds_to: None,
        structured: None,
    }
}

fn run(kb: &KnowledgeBase, submission: Submission, request: Option<&EvidenceRequest>) -> Evidence {
    let pack = &kb.packs()[0];
    let scope = InvestigationScope {
        namespaces: vec!["commerce-prod".into()],
        task_queues: vec!["order-fulfillment".into()],
    };
    ingest(
        "E1".into(),
        at(),
        EvidenceOrigin::Operator {
            channel: submission.channel,
        },
        submission,
        IngestEnv {
            pack,
            diagnostic: pack.diagnostics.first(),
            request,
            scope: &scope,
            families: &[],
        },
    )
    .evidence
}

fn request(expected: &[&str], unit: Option<Unit>) -> EvidenceRequest {
    EvidenceRequest {
        id: "R1".into(),
        definition: "r".into(),
        issued_at: at(),
        title: "t".into(),
        why: "w".into(),
        collect: vec![CollectionInstruction {
            method: CollectionMethod::Promql,
            label: "l".into(),
            body: "b".into(),
            applies_to: None,
            unit,
            claims: vec![],
            unfilled: vec![],
            absent_reads_zero: None,
        }],
        window: RequestedWindow {
            window: None,
            text: String::new(),
        },
        group_by: vec![],
        distinguishes: vec![],
        expected_signals: expected.iter().map(|s| SignalId::new(*s)).collect(),
        responses: vec![],
        status: RequestStatus::Open,
    }
}

#[test]
fn preserves_raw_bytes_exactly() {
    let kb = kb();
    let text = "  {\"status\":\"success\",\"data\":{\"resultType\":\"vector\",\"result\":[]}}\n";
    let e = run(&kb, paste(text), None);
    assert_eq!(e.raw.text(), Some(text));
    assert_eq!(e.raw.sha256, sha256_hex(text.as_bytes()));
    assert_eq!(e.raw.size_bytes, text.len() as u64);
    // An empty result is reported, never read as zero.
    assert!(e.interpretation.observations.is_empty());
    assert!(
        e.interpretation
            .notes
            .iter()
            .any(|n| n.text.contains("not evidence that the value was zero"))
    );
}

#[test]
fn binds_metric_series_by_name_and_labels() {
    let kb = kb();
    let text = r#"{"status":"success","data":{"resultType":"matrix","result":[
        {"metric":{"__name__":"temporal_worker_task_slots_available","worker_type":"WorkflowWorker"},"values":[[1790775300,"180"],[1790775360,"142"]]},
        {"metric":{"__name__":"temporal_worker_task_slots_available","worker_type":"ActivityWorker"},"values":[[1790775300,"0"]]}
    ]}}"#;
    let e = run(&kb, paste(text), None);
    assert_eq!(e.interpretation.form, EvidenceForm::PrometheusQueryOutput);
    let obs = &e.interpretation.observations;
    assert_eq!(obs.len(), 2);
    assert_eq!(obs[0].status, EpistemicStatus::ObservedFact);
    assert_eq!(
        obs[0].binding.as_ref().map(|b| b.signal.as_str()),
        Some("slots")
    );
    // The activity worker series must not inform the workflow slots signal.
    assert!(obs[1].binding.is_none());
}

#[test]
fn label_filter_in_query_satisfies_constraints() {
    let kb = kb();
    let mut s = paste(
        r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{"task_queue":"order-fulfillment"},"values":[[1790775300,"150"]]}]}}"#,
    );
    s.declared.query = Some(r#"sum by (task_queue) (temporal_worker_task_slots_available{worker_type="WorkflowWorker"})"#.into());
    let e = run(&kb, s, None);
    let o = &e.interpretation.observations[0];
    assert_eq!(o.binding.as_ref().map(|b| b.signal.as_str()), Some("slots"));
    assert!(
        !e.interpretation
            .notes
            .iter()
            .any(|n| n.text.contains("assumed"))
    );
}

#[test]
fn histogram_quantile_output_takes_unit_from_query() {
    let kb = kb();
    let mut s = paste(
        r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{},"values":[[1790775300,"31000"]]}]}}"#,
    );
    s.declared.query = Some("histogram_quantile(0.95, sum by (le) (rate(temporal_workflow_task_schedule_to_start_latency_bucket[1m])))".into());
    let e = run(&kb, s, None);
    let o = &e.interpretation.observations[0];
    assert_eq!(o.binding.as_ref().map(|b| b.signal.as_str()), Some("sts"));
    assert_eq!(o.value.unit(), Some(Unit::Milliseconds));
}

const BY_OPERATION: &str = r#"{"status":"success","data":{"resultType":"matrix","result":[
    {"metric":{"operation":"SignalWorkflowExecution"},"values":[[1790775300,"34.1"],[1790775360,"2187"],[1790775420,"2150"]]}
]}}"#;

#[test]
fn a_breakdown_reads_absent_label_values_as_zero() {
    let kb = kb();
    let req = request(&["signal_rate", "start_rate"], None);
    let mut s = paste(BY_OPERATION);
    s.declared.query = Some(
        r#"sum by (operation) (rate(service_requests{namespace="commerce-prod"}[1m]))"#.into(),
    );
    s.responds_to = Some("R1".into());
    let e = run(&kb, s, Some(&req));
    let obs = &e.interpretation.observations;
    assert_eq!(obs.len(), 2, "{obs:#?}");
    assert_eq!(
        obs[0].binding.as_ref().map(|b| b.signal.as_str()),
        Some("signal_rate")
    );
    let zero = &obs[1];
    let binding = zero.binding.as_ref().expect("bound");
    assert_eq!(binding.signal.as_str(), "start_rate");
    assert_eq!(
        binding.basis,
        BindingBasis::AbsentFromBreakdown {
            by: vec!["operation".to_owned()]
        }
    );
    assert_eq!(zero.status, EpistemicStatus::ObservedFact);
    let ObservedValue::Series { unit, points } = &zero.value else {
        panic!("a zero series: {zero:?}");
    };
    assert_eq!(*unit, Some(Unit::PerSecond));
    assert_eq!(points.len(), 3);
    assert!(points.iter().all(|p| p.value == 0.0));
    assert!(
        e.interpretation
            .notes
            .iter()
            .any(|n| n.text.contains("operation=\"StartWorkflowExecution\"")
                && n.text.contains("read as zero"))
    );
}

#[test]
fn grouping_labels_and_functions_are_not_metric_names() {
    let kb = kb();
    let mut s = paste(BY_OPERATION);
    s.declared.query = Some(
        r#"sum by (operation) (rate(service_requests{namespace="commerce-prod", operation=~".+"}[1m]))"#
            .into(),
    );
    let e = run(&kb, s, None);
    let o = &e.interpretation.observations[0];
    assert_eq!(
        o.binding.as_ref().map(|b| &b.basis),
        Some(&BindingBasis::MetricName {
            exposed_name: "service_requests".into()
        })
    );
}

#[test]
fn absence_means_nothing_outside_a_breakdown() {
    let kb = kb();
    let req = request(&["signal_rate", "start_rate"], None);

    // The query filtered the label: other values were never asked for.
    let mut s = paste(
        r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{},"values":[[1790775300,"34.1"],[1790775360,"2187"]]}]}}"#,
    );
    s.declared.query =
        Some(r#"sum(rate(service_requests{operation="SignalWorkflowExecution"}[1m]))"#.into());
    s.responds_to = Some("R1".into());
    let e = run(&kb, s, Some(&req));
    assert_eq!(e.interpretation.observations.len(), 1);

    // Evidence that answers no request infers nothing.
    let mut s = paste(BY_OPERATION);
    s.declared.query = Some("sum by (operation) (rate(service_requests[1m]))".into());
    let e = run(&kb, s, None);
    assert_eq!(e.interpretation.observations.len(), 1);
}

#[test]
fn unnamed_series_bind_to_the_answered_request() {
    let kb = kb();
    let req = request(&["slots"], Some(Unit::Count));
    let mut s = paste("2026-09-30T13:35:00Z 180\n2026-09-30T13:36:00Z 142\n");
    s.responds_to = Some("R1".into());
    let e = run(&kb, s, Some(&req));
    assert_eq!(e.interpretation.form, EvidenceForm::MetricSeriesText);
    let o = &e.interpretation.observations[0];
    assert!(matches!(
        o.binding.as_ref().map(|b| &b.basis),
        Some(BindingBasis::RequestExpectation { .. })
    ));
    assert!(
        e.interpretation
            .notes
            .iter()
            .any(|n| n.severity == NoteSeverity::Caution)
    );
}

#[test]
fn cumulative_counters_become_rates() {
    let kb = kb();
    let text = r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{"__name__":"service_requests","operation":"SignalWorkflowExecution"},"values":[[1790775300,"1000"],[1790775360,"7000"],[1790775420,"13000"]]}]}}"#;
    let e = run(&kb, paste(text), None);
    let o = &e.interpretation.observations[0];
    assert_eq!(
        o.binding.as_ref().map(|b| b.signal.as_str()),
        Some("signal_rate")
    );
    match &o.value {
        ObservedValue::Series { unit, points } => {
            assert_eq!(*unit, Some(Unit::PerSecond));
            assert_eq!(points.len(), 2);
            assert_eq!(points[0].value, 100.0);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn api_json_fields_bind_through_api_claims() {
    let kb = kb();
    let mut s =
        paste(r#"{"stats":{"approximateBacklogCount":"41200","approximateBacklogAge":"31.4s"}}"#);
    s.declared.window = TimeWindow::new(
        "2026-09-30T13:44:00Z".parse().expect("ts"),
        "2026-09-30T13:44:00Z".parse().expect("ts"),
    )
    .ok();
    let e = run(&kb, s, None);
    let o = e
        .interpretation
        .observations
        .iter()
        .find(|o| {
            o.binding
                .as_ref()
                .is_some_and(|b| b.signal.as_str() == "backlog_age")
        })
        .expect("backlog age bound");
    assert!(matches!(o.value, ObservedValue::Scalar { value, .. } if (value - 31.4).abs() < 1e-9));
}

#[test]
fn a_boolean_api_field_binds_as_a_flag() {
    let kb = kb();
    let flag = |json: &str| {
        let mut s = paste(json);
        s.declared.window = TimeWindow::new(
            "2026-09-30T13:44:00Z".parse().expect("ts"),
            "2026-09-30T13:44:00Z".parse().expect("ts"),
        )
        .ok();
        run(&kb, s, None)
            .interpretation
            .observations
            .iter()
            .find(|o| {
                o.binding
                    .as_ref()
                    .is_some_and(|b| b.signal.as_str() == "rate_limited")
            })
            .map(|o| o.value.clone())
    };
    let scalar = |value| ObservedValue::Scalar {
        unit: Some(Unit::Flag),
        value,
        statistic: Statistic::Last,
    };
    assert_eq!(
        flag(r#"{"stats":{"rateLimitingActive":true,"tasksAddRate":12}}"#),
        Some(scalar(1.0))
    );
    assert_eq!(
        flag(r#"{"stats":{"rateLimitingActive":false,"tasksAddRate":12}}"#),
        Some(scalar(0.0))
    );
    // Protobuf JSON leaves out a false boolean: statistics without it answer the
    // field, with no value, and a response without statistics says nothing.
    assert_eq!(
        flag(r#"{"stats":{"tasksAddRate":12}}"#),
        Some(ObservedValue::Omitted {
            field: "stats.rateLimitingActive".to_owned()
        })
    );
    assert_eq!(flag(r#"{"pollers":[]}"#), None);
}

#[test]
fn dynamic_config_binds_only_the_effective_entry() {
    let kb = kb();
    let text = "matching.numTaskqueueReadPartitions:\n  - value: 4\n  - value: 8\n    constraints:\n      namespace: \"commerce-prod\"\n      taskQueueName: \"order-fulfillment\"\n";
    let e = run(&kb, paste(text), None);
    assert_eq!(e.interpretation.form, EvidenceForm::Configuration);
    let bound: Vec<&Observation> = e
        .interpretation
        .observations
        .iter()
        .filter(|o| o.binding.is_some())
        .collect();
    assert_eq!(bound.len(), 1);
    assert_eq!(
        bound[0].value,
        ObservedValue::Categorical { value: "8".into() }
    );
}

#[test]
fn symptom_durations_are_assertions() {
    let kb = kb();
    let pack = &kb.packs()[0];
    let signal = pack.diagnostics[0].signal(&"sts".into());
    let e = ingest_symptom(
        "E0".into(),
        at(),
        "Since 13:35 workflow tasks wait ~30 seconds (normally under 50ms).",
        signal,
        Some("schedule-to-start"),
    );
    let o = &e.interpretation.observations[1];
    assert_eq!(o.status, EpistemicStatus::OperatorAssertion);
    assert!(
        matches!(o.value, ObservedValue::Scalar { value, statistic: Statistic::Typical, .. } if value == 30.0)
    );
}

#[test]
fn structured_entries_are_assertions() {
    let kb = kb();
    let mut s = paste("");
    s.channel = SubmissionChannel::StructuredEntry;
    s.structured = Some(StructuredValue {
        signal: "slots".into(),
        statistic: Statistic::Min,
        value: 142.0,
        unit: None,
    });
    let e = run(&kb, s, None);
    let o = &e.interpretation.observations[0];
    assert_eq!(o.status, EpistemicStatus::OperatorAssertion);
    assert!(matches!(
        o.binding.as_ref().map(|b| &b.basis),
        Some(BindingBasis::StructuredEntry)
    ));
}

#[test]
fn detects_forms() {
    assert_eq!(
        detect_form("temporal_num_pollers{poller_type=\"workflow_task\"} 5"),
        EvidenceForm::PrometheusExposition
    );
    assert_eq!(
        detect_form("Time,Value\n2026-09-30T13:35:00Z,1\n2026-09-30T13:36:00Z,2"),
        EvidenceForm::Csv
    );
    assert_eq!(detect_form("{\"a\":1}"), EvidenceForm::Json);
    assert_eq!(
        detect_form("2026-09-30T13:36:01Z WARN x\n2026-09-30T13:36:02Z ERROR y"),
        EvidenceForm::Log
    );
    assert_eq!(detect_form("hello"), EvidenceForm::Unrecognised);
}

#[test]
fn large_series_keep_their_extremes() {
    let start: Timestamp = "2026-09-30T00:00:00Z".parse().expect("ts");
    let points: Vec<SeriesPoint> = (0..10_000)
        .map(|i| SeriesPoint {
            at: start
                .checked_add(jiff::SignedDuration::from_secs(i))
                .expect("ts"),
            value: if i == 7_777 {
                999.0
            } else if i == 1_234 {
                -5.0
            } else {
                (i % 50) as f64
            },
        })
        .collect();
    let reduced = downsample(&points, MAX_POINTS);
    assert!(reduced.len() <= MAX_POINTS);
    assert!(reduced.iter().any(|p| p.value == 999.0));
    assert!(reduced.iter().any(|p| p.value == -5.0));
    assert!(reduced.windows(2).all(|w| w[0].at <= w[1].at));
}

#[test]
fn large_text_is_stored_by_reference() {
    let kb = kb();
    let big = "2026-09-30T13:35:00Z 1\n".repeat(20_000);
    let pack = &kb.packs()[0];
    let scope = InvestigationScope::default();
    let ingested = ingest(
        "E1".into(),
        at(),
        EvidenceOrigin::Operator {
            channel: SubmissionChannel::Paste,
        },
        paste(&big),
        IngestEnv {
            pack,
            diagnostic: pack.diagnostics.first(),
            request: None,
            scope: &scope,
            families: &[],
        },
    );
    assert!(matches!(ingested.evidence.raw.body, RawBody::Stored { .. }));
    assert_eq!(ingested.blob.as_deref().map(<[u8]>::len), Some(big.len()));
    assert_eq!(ingested.evidence.raw.sha256, sha256_hex(big.as_bytes()));
}

// --- the shipped knowledge pack: every SDK family's names bind with their own units ---------

const MATRIX: &str = r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{},"values":[[1790775300,"31"]]}]}}"#;

fn run_builtin(submission: Submission, families: &[SdkFamily]) -> Evidence {
    let kb = KnowledgeBase::builtin().expect("builtin knowledge");
    let pack = &kb.packs()[0];
    let diagnostic = pack.diagnostic(&"wft-schedule-to-start".into());
    let scope = InvestigationScope {
        namespaces: vec!["commerce-prod".into()],
        task_queues: vec!["order-fulfillment".into()],
    };
    ingest(
        "E1".into(),
        at(),
        EvidenceOrigin::Operator {
            channel: submission.channel,
        },
        submission,
        IngestEnv {
            pack,
            diagnostic,
            request: None,
            scope: &scope,
            families,
        },
    )
    .evidence
}

fn queried(query: &str) -> Submission {
    let mut s = paste(MATRIX);
    s.declared.query = Some(query.to_owned());
    s
}

#[test]
fn java_series_bind_under_their_micrometer_names() {
    let e = run_builtin(
        queried(
            r#"histogram_quantile(0.95, sum by (le) (rate(temporal_workflow_task_schedule_to_start_latency_seconds_bucket{task_queue=~"order-fulfillment(:sticky)?"}[1m])))"#,
        ),
        &[SdkFamily::Java],
    );
    let o = &e.interpretation.observations[0];
    assert_eq!(
        o.binding.as_ref().map(|b| b.signal.as_str()),
        Some("wft_schedule_to_start")
    );
    assert_eq!(o.value.unit(), Some(Unit::Seconds));
    assert!(
        !e.interpretation
            .notes
            .iter()
            .any(|n| n.text.contains("Assumed")),
        "Micrometer exports seconds only: {:?}",
        e.interpretation.notes
    );

    let e = run_builtin(
        queried(
            r#"sum(rate(temporal_workflow_task_queue_poll_succeed_total{namespace="commerce-prod"}[1m]))"#,
        ),
        &[SdkFamily::Java],
    );
    let o = &e.interpretation.observations[0];
    assert_eq!(
        o.binding.as_ref().map(|b| b.signal.as_str()),
        Some("wft_poll_succeed_rate")
    );
}

#[test]
fn a_core_duration_name_alone_leaves_its_unit_assumed() {
    let e = run_builtin(
        queried(
            "histogram_quantile(0.95, sum by (le) (rate(temporal_workflow_task_schedule_to_start_latency_bucket[1m])))",
        ),
        &[SdkFamily::Core],
    );
    let o = &e.interpretation.observations[0];
    assert_eq!(o.value.unit(), Some(Unit::Milliseconds));
    assert!(
        e.interpretation
            .notes
            .iter()
            .any(|n| n.text.contains("Assumed milliseconds")
                && n.text.contains("use_seconds_for_durations")),
        "{:?}",
        e.interpretation.notes
    );
}

/// A block with two queries: free slots by Task Queue, and requests by
/// operation (a breakdown with no StartWorkflowExecution series).
fn block_samples(request: Option<&str>) -> kyumei_samples::Samples {
    use kyumei_samples::{Point, QueryResult, Samples, Series, Window};
    let start = 1_790_775_300_000;
    let points = |values: &[f64]| -> Vec<Point> {
        values
            .iter()
            .enumerate()
            .map(|(i, v)| Point {
                at_ms: start + i as i64 * 60_000,
                value: *v,
            })
            .collect()
    };
    Samples {
        collector: "kyumei-collect 0.1.0".to_owned(),
        source: "prometheus".to_owned(),
        collected_at_ms: start + 3_600_000,
        investigation: Some("inv-test".to_owned()),
        request: request.map(str::to_owned),
        window: Window {
            start_ms: start,
            end_ms: start + 120_000,
            step_ms: 60_000,
        },
        rounded_to: None,
        redacted: Vec::new(),
        queries: vec![
            QueryResult {
                query: r#"sum by (task_queue) (temporal_worker_task_slots_available{worker_type="WorkflowWorker", task_queue="order-fulfillment"})"#.to_owned(),
                series: vec![Series {
                    labels: [("task_queue".to_owned(), "order-fulfillment".to_owned())].into(),
                    points: points(&[180.0, 142.0, 0.0]),
                }],
            },
            QueryResult {
                query: r#"sum by (operation) (rate(service_requests{namespace="commerce-prod"}[1m]))"#.to_owned(),
                series: vec![Series {
                    labels: [("operation".to_owned(), "SignalWorkflowExecution".to_owned())].into(),
                    points: points(&[34.1, 2187.0, 2150.0]),
                }],
            },
            QueryResult {
                query: "temporal_long_request_failure_total".to_owned(),
                series: Vec::new(),
            },
        ],
    }
}

#[test]
fn a_sample_block_binds_each_query_like_its_prometheus_output() {
    let kb = kb();
    let req = request(&["slots", "signal_rate", "start_rate"], None);
    let text = format!(
        "$ kyumei-collect --code kyq1.x\n{}$ \n",
        kyumei_samples::encode(&block_samples(Some("R1")))
    );
    let mut s = paste(&text);
    s.responds_to = Some("R1".into());
    let e = run(&kb, s, Some(&req));
    assert_eq!(e.interpretation.form, EvidenceForm::SampleBlock);
    assert_eq!(e.interpretation.interpreter, "sample-block/1");
    let obs = &e.interpretation.observations;
    let bound: Vec<&str> = obs
        .iter()
        .filter_map(|o| o.binding.as_ref().map(|b| b.signal.as_str()))
        .collect();
    assert_eq!(
        bound,
        vec!["slots", "signal_rate", "start_rate"],
        "{obs:#?}"
    );
    assert!(
        obs.iter()
            .all(|o| o.status == EpistemicStatus::ObservedFact)
    );
    let ObservedValue::Series { points, .. } = &obs[0].value else {
        panic!("series: {:?}", obs[0].value);
    };
    assert_eq!(
        points.iter().map(|p| p.value).collect::<Vec<_>>(),
        vec![180.0, 142.0, 0.0]
    );
    // The breakdown's missing operation is read as zero within its own query.
    assert_eq!(
        obs[2].binding.as_ref().map(|b| &b.basis),
        Some(&BindingBasis::AbsentFromBreakdown {
            by: vec!["operation".to_owned()]
        })
    );
    let notes: Vec<&str> = e
        .interpretation
        .notes
        .iter()
        .map(|n| n.text.as_str())
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("Collected by kyumei-collect 0.1.0 from prometheus")),
        "{notes:#?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("temporal_long_request_failure_total` returned no series")),
        "{notes:#?}"
    );
}

/// An answer whose query returned nothing, with a witness of four samples unless
/// `witnessed` is false. (The test pack's counters stand in for a failure counter
/// and the counter its source keeps beside it.)
fn witnessed_block(witnessed: bool) -> (kyumei_samples::Samples, EvidenceRequest) {
    use kyumei_samples::{Point, QueryResult, Series};
    let failures = r#"sum(rate(service_requests{operation="SignalWorkflowExecution"}[1m]))"#;
    let polls = r#"sum(rate(service_requests{operation="StartWorkflowExecution"}[1m]))"#;
    let mut samples = block_samples(Some("R1"));
    samples.queries = vec![QueryResult {
        query: failures.to_owned(),
        series: Vec::new(),
    }];
    if witnessed {
        samples.queries.push(QueryResult {
            query: polls.to_owned(),
            series: vec![Series {
                labels: BTreeMap::new(),
                points: (0..4)
                    .map(|i| Point {
                        at_ms: samples.window.start_ms + i * 60_000,
                        value: 12.5,
                    })
                    .collect(),
            }],
        });
    }
    let mut req = request(&["signal_rate"], Some(Unit::PerSecond));
    req.collect[0].body = failures.to_owned();
    req.collect[0].absent_reads_zero = Some(AbsentReadsZero {
        witness: polls.to_owned(),
        because: "server.request-errors-counted-on-error".into(),
    });
    (samples, req)
}

#[test]
fn a_counter_with_no_series_reads_as_zero_beside_its_witness() {
    let kb = kb();
    let (samples, req) = witnessed_block(true);
    let mut s = paste(&kyumei_samples::encode(&samples));
    s.responds_to = Some("R1".into());
    let e = run(&kb, s, Some(&req));
    let obs = &e.interpretation.observations;
    assert_eq!(obs.len(), 2, "the zero and the witness: {obs:#?}");
    let zero = obs
        .iter()
        .find(|o| {
            matches!(
                o.binding.as_ref().map(|b| &b.basis),
                Some(BindingBasis::AbsentWithWitness { .. })
            )
        })
        .unwrap_or_else(|| panic!("a witnessed zero: {obs:#?}"));
    assert_eq!(
        zero.binding.as_ref().map(|b| b.signal.as_str()),
        Some("signal_rate")
    );
    let ObservedValue::Series { points, .. } = &zero.value else {
        panic!("series: {:?}", zero.value);
    };
    assert_eq!(points.len(), 4);
    assert!(points.iter().all(|p| p.value == 0.0));
    assert!(
        e.interpretation
            .notes
            .iter()
            .any(|n| n.text.contains("is read as zero")
                && n.text.contains("server.request-errors-counted-on-error")),
        "{:#?}",
        e.interpretation.notes
    );
}

#[test]
fn without_its_witness_no_series_is_recorded_as_absent() {
    let kb = kb();
    let (samples, req) = witnessed_block(false);
    let mut s = paste(&kyumei_samples::encode(&samples));
    s.responds_to = Some("R1".into());
    let e = run(&kb, s, Some(&req));
    let [absent] = e.interpretation.observations.as_slice() else {
        panic!("one observation: {:#?}", e.interpretation.observations);
    };
    assert_eq!(
        absent.binding.as_ref().map(|b| b.signal.as_str()),
        Some("signal_rate")
    );
    assert!(
        matches!(&absent.value, ObservedValue::Absent { query } if query.contains("SignalWorkflowExecution")),
        "{:?}",
        absent.value
    );
    assert!(e.interpretation.notes.iter().any(|n| {
        n.text
            .contains("which is not evidence that the value was zero")
    }));
}

#[test]
fn a_query_pinned_to_another_value_does_not_bind_where_a_label_is_assumed() {
    let kb = kb();
    let mut s = paste(
        r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{},"values":[[1790775300,"12"]]}]}}"#,
    );
    s.declared.query =
        Some(r#"sum(rate(service_requests{operation="PollWorkflowTaskQueue"}[1m]))"#.into());
    let e = run(&kb, s, None);
    assert!(
        e.interpretation
            .observations
            .iter()
            .all(|o| o.binding.is_none()),
        "{:#?}",
        e.interpretation.observations
    );
    assert!(
        !e.interpretation
            .notes
            .iter()
            .any(|n| n.text.contains("assumed")),
        "{:#?}",
        e.interpretation.notes
    );
}

#[test]
fn a_damaged_block_is_reported_and_not_read() {
    let kb = kb();
    let text = kyumei_samples::encode(&block_samples(None));
    let at = text.find("\n:").expect("payload") + 12;
    let mut bytes = text.into_bytes();
    bytes[at] = if bytes[at] == b'A' { b'B' } else { b'A' };
    let e = run(&kb, paste(&String::from_utf8(bytes).expect("utf8")), None);
    assert_eq!(e.interpretation.form, EvidenceForm::SampleBlock);
    assert!(e.interpretation.observations.is_empty());
    assert!(
        e.interpretation
            .notes
            .iter()
            .any(|n| n.severity == NoteSeverity::Problem && n.text.contains("checksum")),
        "{:#?}",
        e.interpretation.notes
    );
}

#[test]
fn blocks_say_when_they_answer_another_request_or_were_altered_on_purpose() {
    let kb = kb();
    let req = request(&["slots"], None);
    let mut samples = block_samples(Some("R7"));
    samples.rounded_to = Some(3);
    samples.redacted = vec!["namespace".to_owned()];
    let mut s = paste(&kyumei_samples::encode(&samples));
    s.responds_to = Some("R1".into());
    let e = run(&kb, s, Some(&req));
    let notes: Vec<&str> = e
        .interpretation
        .notes
        .iter()
        .map(|n| n.text.as_str())
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("collected for request R7, not R1")),
        "{notes:#?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("rounded to 3 significant digits")),
        "{notes:#?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("namespace are pseudonyms")),
        "{notes:#?}"
    );
}

#[test]
fn collected_evidence_is_observed_fact() {
    let kb = kb();
    let mut s = paste(&kyumei_samples::encode(&block_samples(None)));
    s.channel = SubmissionChannel::Collector;
    let e = run(&kb, s, None);
    assert!(!e.interpretation.observations.is_empty());
    assert!(
        e.interpretation
            .observations
            .iter()
            .all(|o| o.status == EpistemicStatus::ObservedFact)
    );
}

// --- Temporal CLI and API JSON, read by the shipped packs -----------------------------

/// `temporal task-queue describe --task-queue-type activity -o json`, as CLI 1.7.0
/// to 1.9.1 print it: the CLI's own rows, lists per Build ID and Task Queue type,
/// ages as Go durations with spaces, and no rate limit.
const CLI_ACTIVITY: &str = r#"{
  "reachability": null,
  "pollers": [
    { "buildId": "UNVERSIONED", "taskQueueType": "activity", "identity": "5100@invoice-worker-6c8d9f7b5-a1kq", "lastAccessTime": "2026-10-01T09:06:29.812Z", "ratePerSecond": 100 },
    { "buildId": "UNVERSIONED", "taskQueueType": "activity", "identity": "5107@invoice-worker-6c8d9f7b5-b2mr", "lastAccessTime": "2026-10-01T09:06:28.775Z", "ratePerSecond": 100 }
  ],
  "stats": [
    { "buildId": "UNVERSIONED", "taskQueueType": "activity", "approximateBacklogCount": 19480, "approximateBacklogAge": "2m 10.402s", "backlogIncreaseRate": 50.6, "tasksAddRate": 150.2, "tasksDispatchRate": 99.6 }
  ]
}"#;

/// The same command for the Workflow Task Queue with no Worker polling it: the
/// CLI prints no pollers as `null`, and a sub-second age as `450m s`.
const CLI_WORKFLOW: &str = r#"{
  "reachability": null,
  "pollers": null,
  "stats": [
    { "buildId": "UNVERSIONED", "taskQueueType": "workflow", "approximateBacklogCount": 25, "approximateBacklogAge": "450m s", "backlogIncreaseRate": 1.3016486, "tasksAddRate": 1.3016486, "tasksDispatchRate": 0 }
  ]
}"#;

/// Without `--task-queue-type` the Frontend describes both types, one row each
/// (CLI 1.8.0 against Server 1.31.2).
const CLI_BOTH_TYPES: &str = r#"{
  "reachability": null,
  "pollers": null,
  "stats": [
    { "buildId": "UNVERSIONED", "taskQueueType": "activity", "approximateBacklogCount": 0, "approximateBacklogAge": "0s", "backlogIncreaseRate": 0, "tasksAddRate": 0, "tasksDispatchRate": 0 },
    { "buildId": "UNVERSIONED", "taskQueueType": "workflow", "approximateBacklogCount": 25, "approximateBacklogAge": "19.339312s", "backlogIncreaseRate": 1.3016486, "tasksAddRate": 1.3016486, "tasksDispatchRate": 0 }
  ]
}"#;

/// DescribeTaskQueue through grpcurl with `-emit-defaults`: the API's own JSON,
/// with false and zero values printed. Server 1.32 adds `rateLimitingActive`.
const GRPCURL_ACTIVITY_1_32: &str = r#"{
  "pollers": [],
  "taskQueueStatus": null,
  "versionsInfo": {},
  "stats": { "approximateBacklogCount": "19480", "approximateBacklogAge": "130.402s", "tasksAddRate": 150.2, "tasksDispatchRate": 99.6, "rateLimitingActive": false },
  "config": null,
  "effectiveRateLimit": { "requestsPerSecond": 100, "rateLimitSource": "RATE_LIMIT_SOURCE_WORKER" },
  "statsByPriorityKey": { "3": { "approximateBacklogCount": "19480", "approximateBacklogAge": "130.402s", "tasksAddRate": 150.2, "tasksDispatchRate": 99.6, "rateLimitingActive": false } }
}"#;

/// Ingest `json` in the shipped pack for `family`, for `diagnostic`. Returns the
/// signals of `request` the evidence informs, those it leaves uninformed, and the
/// evidence.
fn answer_in_pack(
    family: &str,
    diagnostic: &str,
    request: &str,
    json: &str,
) -> (Vec<String>, Vec<String>, Evidence) {
    let kb = KnowledgeBase::builtin().expect("builtin knowledge");
    let pack = kb
        .packs()
        .iter()
        .find(|p| p.manifest.server_family == family)
        .expect("pack");
    let diagnostic = pack.diagnostic(&diagnostic.into()).expect("diagnostic");
    let expected = &diagnostic
        .request(&request.into())
        .expect("request")
        .signals;
    let scope = InvestigationScope {
        namespaces: vec!["billing-prod".into()],
        task_queues: vec!["invoice-activities".into()],
    };
    let mut s = paste(json);
    s.declared.window = Some(TimeWindow {
        start: at(),
        end: at(),
    });
    let e = ingest(
        "E4".into(),
        at(),
        EvidenceOrigin::Operator { channel: s.channel },
        s,
        IngestEnv {
            pack,
            diagnostic: Some(diagnostic),
            request: None,
            scope: &scope,
            families: &[],
        },
    )
    .evidence;
    let bound: BTreeSet<String> = e
        .interpretation
        .observations
        .iter()
        .filter_map(|o| o.binding.as_ref().map(|b| b.signal.to_string()))
        .collect();
    let (informed, uninformed): (Vec<String>, Vec<String>) = expected
        .iter()
        .map(ToString::to_string)
        .partition(|s| bound.contains(s));
    (informed, uninformed, e)
}

fn value_of<'e>(e: &'e Evidence, signal: &str) -> &'e ObservedValue {
    &e.interpretation
        .observations
        .iter()
        .find(|o| {
            o.binding
                .as_ref()
                .is_some_and(|b| b.signal.as_str() == signal)
        })
        .unwrap_or_else(|| panic!("no observation for {signal}"))
        .value
}

#[test]
fn the_cli_json_informs_the_activity_statistics_but_not_the_rate_limit() {
    for family in ["1.31", "1.32"] {
        let (informed, uninformed, e) = answer_in_pack(
            family,
            "activity-schedule-to-start",
            "activity-queue-stats",
            CLI_ACTIVITY,
        );
        assert_eq!(
            informed,
            [
                "backlog_age",
                "backlog_count",
                "tasks_add_rate",
                "tasks_dispatch_rate"
            ],
            "{family}"
        );
        // The CLI never prints the limit, its source or, on 1.32, rateLimitingActive,
        // so the request stays open for them: a list-shaped `stats` without the flag
        // is not a protobuf omission.
        let mut left = vec!["dispatch_rate_limit", "rate_limit_source"];
        if family == "1.32" {
            left.push("rate_limiting_active");
        }
        assert_eq!(uninformed, left, "{family}");
        assert!(
            !e.interpretation
                .observations
                .iter()
                .any(|o| matches!(o.value, ObservedValue::Omitted { .. })),
            "{family}"
        );
        match value_of(&e, "backlog_age") {
            ObservedValue::Scalar { value, unit, .. } => {
                assert!((value - 130.402).abs() < 1e-9, "{value}");
                assert_eq!(*unit, Some(Unit::Seconds));
            }
            other => panic!("{other:?}"),
        }
        match value_of(&e, "backlog_count") {
            ObservedValue::Scalar { value, .. } => assert_eq!(*value, 19_480.0),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn the_cli_json_informs_the_workflow_statistics_and_pollers_but_not_the_rate_limit() {
    for family in ["1.31", "1.32"] {
        let (informed, uninformed, e) = answer_in_pack(
            family,
            "wft-schedule-to-start",
            "matching-queue-stats",
            CLI_WORKFLOW,
        );
        assert_eq!(
            informed,
            [
                "backlog_age",
                "backlog_count",
                "tasks_add_rate",
                "tasks_dispatch_rate",
                "listed_pollers"
            ],
            "{family}"
        );
        let mut left = vec!["dispatch_rate_limit"];
        if family == "1.32" {
            left.insert(0, "rate_limiting_active");
        }
        let mut uninformed = uninformed;
        uninformed.sort();
        left.sort();
        assert_eq!(uninformed, left, "{family}");
        match value_of(&e, "backlog_age") {
            ObservedValue::Scalar { value, .. } => assert!((value - 0.45).abs() < 1e-12),
            other => panic!("{other:?}"),
        }
        match value_of(&e, "listed_pollers") {
            ObservedValue::Scalar { value, .. } => assert_eq!(*value, 0.0),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn cli_rows_for_both_task_queue_types_are_left_unread() {
    let (informed, _, e) = answer_in_pack(
        "1.31",
        "activity-schedule-to-start",
        "activity-queue-stats",
        CLI_BOTH_TYPES,
    );
    // Summed, the rows would give the Activity Task Queue the Workflow backlog.
    assert!(informed.is_empty(), "{informed:?}");
    let notes: Vec<&str> = e
        .interpretation
        .notes
        .iter()
        .map(|n| n.text.as_str())
        .collect();
    assert!(
        notes.iter().any(|n| n
            .starts_with("`stats` describes the activity and workflow Task Queues together")
            && n.contains("`--task-queue-type`")),
        "{notes:#?}"
    );
    assert!(!notes.iter().any(|n| n.contains("No field")), "{notes:#?}");
}

#[test]
fn describe_task_queue_through_grpcurl_informs_every_signal() {
    let (_, uninformed, e) = answer_in_pack(
        "1.32",
        "activity-schedule-to-start",
        "activity-queue-stats",
        GRPCURL_ACTIVITY_1_32,
    );
    assert!(uninformed.is_empty(), "{uninformed:?}");
    match value_of(&e, "dispatch_rate_limit") {
        ObservedValue::Scalar { value, .. } => assert_eq!(*value, 100.0),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        value_of(&e, "rate_limit_source"),
        &ObservedValue::Categorical {
            value: "RATE_LIMIT_SOURCE_WORKER".to_owned()
        }
    );
    // `-emit-defaults` prints false, which then reads as a value.
    match value_of(&e, "rate_limiting_active") {
        ObservedValue::Scalar { value, unit, .. } => {
            assert_eq!(*value, 0.0);
            assert_eq!(*unit, Some(Unit::Flag));
        }
        other => panic!("{other:?}"),
    }
}

/// `temporal workflow describe -o json` for a terminated Workflow, as CLI 1.7.0 to
/// 1.9.1 print it: DescribeWorkflowExecution's own JSON, with the close event the
/// CLI adds for a Workflow that is no longer running.
const CLI_DESCRIBE_TERMINATED: &str = r#"{
  "executionConfig": { "taskQueue": { "name": "ingest", "kind": "TASK_QUEUE_KIND_NORMAL" }, "defaultWorkflowTaskTimeout": "10s" },
  "workflowExecutionInfo": {
    "execution": { "workflowId": "import-batch-20261009-0412", "runId": "0199c6a1-3e0d-7f4b-b8a2-90d1c4e7a5f3" },
    "type": { "name": "ImportBatchWorkflow" },
    "startTime": "2026-10-09T04:12:44.302Z",
    "closeTime": "2026-10-09T10:58:03.117Z",
    "status": "WORKFLOW_EXECUTION_STATUS_TERMINATED",
    "historyLength": "9412",
    "taskQueue": "ingest",
    "stateTransitionCount": "9401",
    "historySizeBytes": "52611207"
  },
  "closeEvent": {
    "eventId": "9412",
    "eventTime": "2026-10-09T10:58:03.117Z",
    "eventType": "EVENT_TYPE_WORKFLOW_EXECUTION_TERMINATED",
    "taskId": "184683594",
    "workflowExecutionTerminatedEventAttributes": { "reason": "Workflow history size exceeds limit.", "identity": "history-service" }
  }
}"#;

/// The same command for a Workflow still retrying a failing Workflow Task.
const CLI_DESCRIBE_STUCK: &str = r#"{
  "workflowExecutionInfo": {
    "execution": { "workflowId": "order-7f3c2a91", "runId": "0199c7e2-51aa-7c3d-9e0b-4f6a2d1c8b97" },
    "status": "WORKFLOW_EXECUTION_STATUS_RUNNING",
    "historyLength": "11"
  },
  "pendingWorkflowTask": {
    "state": "PENDING_WORKFLOW_TASK_STATE_SCHEDULED",
    "scheduledTime": "2026-10-09T14:37:51.004Z",
    "originalScheduledTime": "2026-10-09T14:02:10.886Z",
    "attempt": 14
  }
}"#;

/// `temporal workflow show --output json`: the whole History, oldest event first.
const CLI_SHOW_TERMINATED: &str = r#"{
  "events": [
    { "eventId": "1", "eventType": "EVENT_TYPE_WORKFLOW_EXECUTION_STARTED", "workflowExecutionStartedEventAttributes": { "workflowType": { "name": "ImportBatchWorkflow" } } },
    { "eventId": "2", "eventType": "EVENT_TYPE_WORKFLOW_TASK_SCHEDULED", "workflowTaskScheduledEventAttributes": { "attempt": 1 } },
    { "eventId": "9412", "eventType": "EVENT_TYPE_WORKFLOW_EXECUTION_TERMINATED", "workflowExecutionTerminatedEventAttributes": { "reason": "Workflow history size exceeds limit.", "identity": "history-service" } }
  ]
}"#;

#[test]
fn one_cli_describe_of_a_terminated_workflow_informs_its_size_and_why_it_ended() {
    for family in ["1.31", "1.32"] {
        let (informed, uninformed, e) = answer_in_pack(
            family,
            "history-limits",
            "workflow-size",
            CLI_DESCRIBE_TERMINATED,
        );
        assert_eq!(
            informed,
            ["workflow_status", "history_length", "history_size_bytes"],
            "{family}"
        );
        assert!(uninformed.is_empty(), "{family}: {uninformed:?}");
        assert_eq!(
            value_of(&e, "workflow_status"),
            &ObservedValue::Categorical {
                value: "WORKFLOW_EXECUTION_STATUS_TERMINATED".to_owned()
            }
        );
        match value_of(&e, "history_size_bytes") {
            ObservedValue::Scalar { value, .. } => assert_eq!(*value, 52_611_207.0),
            other => panic!("{other:?}"),
        }
        // The close event the CLI adds answers the termination request too.
        let (informed, _, e) = answer_in_pack(
            family,
            "history-limits",
            "termination-event",
            CLI_DESCRIBE_TERMINATED,
        );
        assert_eq!(informed, ["termination_reason"], "{family}");
        assert_eq!(
            value_of(&e, "termination_reason"),
            &ObservedValue::Categorical {
                value: "Workflow history size exceeds limit.".to_owned()
            }
        );
    }
}

#[test]
fn a_cli_describe_of_a_stuck_workflow_informs_its_pending_attempt() {
    for family in ["1.31", "1.32"] {
        let (informed, _, e) = answer_in_pack(
            family,
            "workflow-task-failures",
            "retry-attempts",
            CLI_DESCRIBE_STUCK,
        );
        assert!(
            informed.contains(&"pending_attempt".to_owned()),
            "{family}: {informed:?}"
        );
        match value_of(&e, "pending_attempt") {
            ObservedValue::Scalar { value, .. } => assert_eq!(*value, 14.0),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn the_cli_history_informs_why_the_workflow_ended() {
    for family in ["1.31", "1.32"] {
        let (informed, uninformed, e) = answer_in_pack(
            family,
            "history-limits",
            "termination-event",
            CLI_SHOW_TERMINATED,
        );
        assert_eq!(informed, ["termination_reason"], "{family}");
        assert!(uninformed.is_empty(), "{family}");
        assert_eq!(
            value_of(&e, "termination_reason"),
            &ObservedValue::Categorical {
                value: "Workflow history size exceeds limit.".to_owned()
            }
        );
    }
}
