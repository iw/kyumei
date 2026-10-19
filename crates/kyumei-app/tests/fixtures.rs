//! Every fixture investigation replays with its expectations met, and the
//! investigation can branch, revisit hypotheses and work around missing
//! evidence.

use std::sync::Arc;

use kyumei_app::{
    Command, CorrectionInput, InvestigationService, LocalInvestigations,
    fixture::{self, Fixture, FixtureClock},
};
use kyumei_domain::{CorrectionChange, HypothesisState, RequestStatus};
use kyumei_knowledge::KnowledgeBase;

fn kb() -> Arc<KnowledgeBase> {
    Arc::new(KnowledgeBase::builtin().expect("knowledge"))
}

fn service(kb: &Arc<KnowledgeBase>, fx: &Fixture) -> (LocalInvestigations, Arc<FixtureClock>) {
    let clock = Arc::new(FixtureClock::new(fx.open.at));
    (LocalInvestigations::new(kb.clone(), clock.clone()), clock)
}

#[tokio::test]
async fn every_fixture_replays() {
    let kb = kb();
    let mut names = Vec::new();
    for fx in fixture::all().expect("fixtures load") {
        let (svc, clock) = service(&kb, &fx);
        if let Err(e) = fixture::replay(&kb, &svc, &fx, Some(&clock), true).await {
            panic!("fixture {}: {e}", fx.fixture.id);
        }
        names.push(fx.fixture.id.clone());
    }
    assert_eq!(
        names,
        vec![
            "activity-api-rate-limit",
            "activity-slot-exhaustion",
            "activity-worker-rate-limit",
            "activity-worker-rate-limit-cli",
            "activity-worker-rate-limit-reconnects",
            "activity-worker-rate-limit-reported",
            "history-approaching-limits",
            "history-cache-full",
            "history-cache-shard-moves",
            "history-count-limit",
            "history-crash-slow-acquire",
            "history-deploy-churn",
            "history-size-limit",
            "history-tasks-busy-workflow",
            "history-tasks-throttled",
            "java-cache-fills-threads",
            "java-signal-handler-threads",
            "namespace-rate-limit-surge",
            "nondeterminism-after-deploy",
            "persistence-database-slow",
            "persistence-host-limit",
            "persistence-pool-saturation",
            "poll-throttling",
            "signal-flood-uneven-partitions",
            "signal-flood-wft-latency",
            "sticky-cache-disabled",
            "sticky-cache-full",
            "sticky-worker-crashes",
            "timer-burst-scheduler",
            "visibility-collected-blocks",
            "visibility-es-rejecting-writes",
            "visibility-es-slow-writes",
            "visibility-list-rate-limited",
            "visibility-slow-queries",
            "worker-saturation",
            "workflow-code-error",
            "workflow-task-timeouts",
        ]
    );
}

/// The same investigations on a Temporal 1.32 server: the 1.32 pack carries the
/// twelve diagnostics, and every fixture reaches the same states, requests and
/// conclusions on it.
#[tokio::test]
async fn every_fixture_replays_on_the_1_32_pack() {
    let kb = kb();
    for mut fx in fixture::all().expect("fixtures load") {
        fx.open.server_version = Some("1.32.1".to_owned());
        let (svc, clock) = service(&kb, &fx);
        let (id, _) = fixture::replay(&kb, &svc, &fx, Some(&clock), true)
            .await
            .unwrap_or_else(|e| panic!("fixture {} on 1.32: {e}", fx.fixture.id));
        let investigation = svc.snapshot(&id).await.expect("snapshot").investigation;
        let assessment =
            kyumei_app::core::current_assessment(&kb, &investigation).expect("assessment");
        assert_eq!(
            assessment.pack.id.as_str(),
            "temporal-1.32",
            "{}",
            fx.fixture.id
        );
    }
}

async fn replayed(kb: &Arc<KnowledgeBase>, fx: &Fixture) -> kyumei_diagnostics::Assessment {
    let (svc, clock) = service(kb, fx);
    let (id, _) = fixture::replay(kb, &svc, fx, Some(&clock), false)
        .await
        .expect("replay");
    let investigation = svc.snapshot(&id).await.expect("snapshot").investigation;
    kyumei_app::core::current_assessment(kb, &investigation).expect("assessment")
}

fn named(id: &str) -> Fixture {
    fixture::all()
        .expect("fixtures load")
        .into_iter()
        .find(|f| f.fixture.id == id)
        .expect("fixture")
}

/// On 1.32 DescribeTaskQueue says when the rate limiter held dispatch back. With
/// dispatch below the limit it reports, that alone shows the limit binding, and
/// the conclusion says why dispatch ran below it; without the report the limit
/// stays undecided.
#[tokio::test]
async fn reported_rate_limiting_settles_a_limit_dispatch_runs_below() {
    let kb = kb();
    let fx = named("signal-flood-uneven-partitions");
    let assessment = replayed(&kb, &fx).await;
    let conclusion = assessment.conclusion.expect("a conclusion").statement;
    assert!(
        conclusion.contains("held Workflow Tasks back"),
        "{conclusion}"
    );
    assert!(
        conclusion.contains("DescribeTaskQueue reported the rate limiter holding dispatch back"),
        "{conclusion}"
    );
    assert!(conclusion.contains("below the limit"), "{conclusion}");
    assert!(
        conclusion.contains("Tasks are written to 3 of the 4 read partitions"),
        "{conclusion}"
    );
    assert!(!conclusion.contains("pinned at the limit"), "{conclusion}");

    // The same capture without rateLimitingActive, as a 1.31 server returns it.
    let mut without = fx.clone();
    let step = without
        .steps
        .iter_mut()
        .find(|s| s.answers == "matching-queue-stats")
        .expect("the capture");
    let capture =
        std::fs::read_to_string(fx.dir.join(step.file.as_deref().expect("file"))).expect("capture");
    let mut json: serde_json::Value = serde_json::from_str(&capture).expect("json");
    for pointer in ["/stats", "/statsByPriorityKey/3"] {
        json.pointer_mut(pointer)
            .and_then(serde_json::Value::as_object_mut)
            .expect("stats")
            .remove("rateLimitingActive");
    }
    let path = std::env::temp_dir().join(format!(
        "kyumei-capture-without-flag-{}.json",
        std::process::id()
    ));
    std::fs::write(&path, json.to_string()).expect("write capture");
    step.file = Some(path.display().to_string());
    let assessment = replayed(&kb, &without).await;
    std::fs::remove_file(&path).expect("remove capture");
    let ceiling = assessment
        .hypotheses
        .iter()
        .find(|h| h.id.as_str() == "dispatch-rate-ceiling")
        .expect("ceiling");
    assert_eq!(ceiling.state, HypothesisState::Candidate);
    assert!(assessment.conclusion.is_none());
}

/// Where dispatch is pinned at the limit, the report adds the server's own
/// account of the throttling to the conclusion.
#[tokio::test]
async fn reported_rate_limiting_corroborates_a_pinned_limit() {
    let kb = kb();
    let assessment = replayed(&kb, &named("activity-worker-rate-limit-reported")).await;
    let conclusion = assessment.conclusion.expect("a conclusion").statement;
    assert!(conclusion.contains("pinned at the limit"), "{conclusion}");
    assert!(
        conclusion.contains("DescribeTaskQueue reported the rate limiter holding dispatch back"),
        "{conclusion}"
    );
    assert!(!conclusion.contains("below the limit"), "{conclusion}");
}

#[tokio::test]
async fn context_changes_rerender_the_open_request() {
    let kb = kb();
    let fx = fixture::golden().expect("golden");
    let (svc, clock) = service(&kb, &fx);
    let (id, _) = fixture::open(&kb, &svc, &fx, Some(&clock), true)
        .await
        .expect("open");
    let before = svc.snapshot(&id).await.expect("snapshot").investigation;
    let r1 = before.current_request().expect("R1").clone();
    assert!(r1.collect.iter().all(|c| c.unfilled.is_empty()));
    // Go Workers may export through tally, which writes the `-` as `_`.
    assert!(
        r1.collect[0]
            .body
            .contains(r#"task_queue=~"order-fulfillment|order_fulfillment""#),
        "{}",
        r1.collect[0].body
    );
    assert!(
        r1.collect[0]
            .label
            .contains("`order-fulfillment` also as `order_fulfillment`")
    );
    assert!(
        r1.collect[0]
            .claims
            .iter()
            .any(|c| c.as_str() == "sdk.go-tally-label-values")
    );

    // The operator realises the Task Queue is named differently.
    svc.command(
        &id,
        Command::AmendContext(kyumei_app::ContextInput {
            task_queues: Some(vec!["order-fulfillment-v2".into()]),
            ..Default::default()
        }),
    )
    .await
    .expect("amend");
    let after = svc.snapshot(&id).await.expect("snapshot").investigation;
    assert!(matches!(
        after.request(&r1.id).map(|r| &r.status),
        Some(RequestStatus::Superseded)
    ));
    let r2 = after.current_request().expect("re-issued");
    assert_eq!(r2.definition, r1.definition, "the same question");
    assert!(
        r2.collect[0]
            .body
            .contains(r#"task_queue=~"order-fulfillment-v2|order_fulfillment_v2""#),
        "rendered for the new Task Queue: {}",
        r2.collect[0].body
    );

    // A change that does not affect the request leaves it alone.
    svc.command(
        &id,
        Command::AmendContext(kyumei_app::ContextInput {
            server_version: Some("1.31.1".into()),
            ..Default::default()
        }),
    )
    .await
    .expect("amend");
    let last = svc.snapshot(&id).await.expect("snapshot").investigation;
    assert_eq!(
        last.current_request().map(|r| r.id.clone()),
        Some(r2.id.clone())
    );
}

#[tokio::test]
async fn declined_evidence_leads_to_an_alternative() {
    let kb = kb();
    let fx = fixture::golden().expect("golden");
    let (svc, clock) = service(&kb, &fx);
    let (id, _) = fixture::open(&kb, &svc, &fx, Some(&clock), true)
        .await
        .expect("open");

    let first = svc.snapshot(&id).await.expect("snapshot").investigation;
    let r1 = first.current_request().expect("R1").clone();
    assert_eq!(r1.definition.as_str(), "workflow-worker-slots");
    svc.command(
        &id,
        Command::DeclineRequest {
            request: r1.id.clone(),
            reason: "no SDK metrics in this cluster".into(),
        },
    )
    .await
    .expect("decline");

    let after = svc.snapshot(&id).await.expect("snapshot").investigation;
    assert!(matches!(
        after.request(&r1.id).map(|r| &r.status),
        Some(RequestStatus::Declined { .. })
    ));
    let next = after.current_request().expect("an alternative request");
    assert_eq!(
        next.definition.as_str(),
        "wft-schedule-to-start",
        "the same question, asked another way"
    );
    let a = kyumei_app::core::current_assessment(&kb, &after).expect("assessment");
    assert!(
        a.uncertainties
            .iter()
            .any(|u| u.text.contains("no SDK metrics")),
        "the gap is recorded as uncertainty"
    );
    let worker = a
        .hypothesis(&"worker-slot-exhaustion".into())
        .expect("active");
    assert_eq!(
        worker.state,
        HypothesisState::Candidate,
        "absence of evidence is not evidence of absence"
    );
}

#[tokio::test]
async fn corrections_revisit_hypotheses_without_touching_raw_evidence() {
    let kb = kb();
    let fx = fixture::golden().expect("golden");
    let (svc, clock) = service(&kb, &fx);
    let (id, _) = fixture::open(&kb, &svc, &fx, Some(&clock), true)
        .await
        .expect("open");
    fixture::step(&kb, &svc, &fx, &id, 0, Some(&clock), true)
        .await
        .expect("E1");
    let state = |inv: &kyumei_domain::Investigation| {
        kyumei_app::core::current_assessment(&kb, inv)
            .expect("assessment")
            .hypothesis(&"worker-slot-exhaustion".into())
            .map(|h| h.state)
    };
    let before = svc.snapshot(&id).await.expect("snapshot").investigation;
    assert_eq!(state(&before), Some(HypothesisState::Eliminated));
    let raw = before.evidence(&"E1".into()).expect("E1").raw.clone();

    // The operator says the series was really the activity workers' slots.
    svc.command(
        &id,
        Command::CorrectInterpretation(CorrectionInput {
            evidence: "E1".into(),
            observation: Some("E1.O1".into()),
            change: CorrectionChange::Unbind,
            reason: "the dashboard query was filtered to ActivityWorker".into(),
        }),
    )
    .await
    .expect("correct");
    let corrected = svc.snapshot(&id).await.expect("snapshot").investigation;
    assert_eq!(
        state(&corrected),
        Some(HypothesisState::Candidate),
        "the elimination is revisited"
    );
    assert_eq!(
        corrected.evidence(&"E1".into()).expect("E1").raw,
        raw,
        "raw evidence is unchanged"
    );
    assert_eq!(
        corrected
            .current_request()
            .map(|r| r.definition.as_str().to_owned())
            .as_deref(),
        Some("workflow-worker-slots"),
        "Worker capacity is asked about again"
    );
    let trail = corrected.trail();
    assert!(trail.iter().any(|t| matches!(
        t.item,
        kyumei_domain::TrailItem::InterpretationCorrected { .. }
    )));
    assert!(trail.iter().any(|t| matches!(
        &t.item,
        kyumei_domain::TrailItem::HypothesisChanged {
            to: HypothesisState::Candidate,
            from: Some(HypothesisState::Eliminated),
            ..
        }
    )));

    // Binding it back restores the elimination.
    svc.command(
        &id,
        Command::CorrectInterpretation(CorrectionInput {
            evidence: "E1".into(),
            observation: Some("E1.O1".into()),
            change: CorrectionChange::BindSignal {
                signal: "workflow_slots_available".into(),
            },
            reason: "checked: it was WorkflowWorker after all".into(),
        }),
    )
    .await
    .expect("rebind");
    let restored = svc.snapshot(&id).await.expect("snapshot").investigation;
    assert_eq!(state(&restored), Some(HypothesisState::Eliminated));
}

#[tokio::test]
async fn unknown_context_stays_unknown() {
    let kb = kb();
    let clock = Arc::new(FixtureClock::new(
        "2026-09-30T13:52:00Z".parse().expect("ts"),
    ));
    let svc = LocalInvestigations::new(kb.clone(), clock);
    let id = svc
        .open(kyumei_app::OpenInvestigation {
            id: "inv-sparse".into(),
            description: "workflow tasks are slow to start, schedule to start is high".into(),
            ..Default::default()
        })
        .await
        .expect("open");
    let inv = svc.snapshot(&id).await.expect("snapshot").investigation;
    assert!(inv.environment.server_version.is_none());
    assert!(inv.window.as_window().is_none());
    let request = inv.current_request().expect("still asks for evidence");
    assert!(
        request
            .collect
            .iter()
            .any(|c| c.unfilled.contains(&"namespace".to_owned())),
        "placeholders stay visible"
    );
    let a = kyumei_app::core::current_assessment(&kb, &inv).expect("assessment");
    let kinds: Vec<_> = a.uncertainties.iter().map(|u| u.kind).collect();
    assert!(kinds.contains(&kyumei_domain::UncertaintyKind::VersionUnverified));
    assert!(kinds.contains(&kyumei_domain::UncertaintyKind::MissingContext));
}

/// Every SDK-specific evidence request, rendered for the Workers' SDK.
async fn rendered_for(
    language: Option<&str>,
) -> std::collections::BTreeMap<String, Vec<kyumei_domain::CollectionInstruction>> {
    let kb = kb();
    let fx = fixture::golden().expect("golden");
    let (svc, clock) = service(&kb, &fx);
    let (id, _) = fixture::open(&kb, &svc, &fx, Some(&clock), true)
        .await
        .expect("open");
    let sdks = language.map_or_else(Vec::new, |language| {
        vec![kyumei_app::SdkInput {
            language: language.to_owned(),
            version: None,
        }]
    });
    svc.command(
        &id,
        Command::AmendContext(kyumei_app::ContextInput {
            sdks: Some(sdks),
            ..Default::default()
        }),
    )
    .await
    .expect("amend");
    let investigation = svc.snapshot(&id).await.expect("snapshot").investigation;
    let pack = &kb.packs()[0];
    let d = pack
        .diagnostic(&"wft-schedule-to-start".into())
        .expect("diagnostic");
    d.requests
        .iter()
        .filter(|r| {
            [
                "workflow-worker-slots",
                "wft-schedule-to-start",
                "workflow-pollers",
                "poll-failures",
            ]
            .contains(&r.id.as_str())
        })
        .map(|def| {
            let r = kyumei_diagnostics::select::render_request(
                def,
                d,
                &investigation,
                "R".into(),
                fx.open.at,
            );
            (def.id.to_string(), r.collect)
        })
        .collect()
}

#[tokio::test]
async fn java_workers_get_java_queries() {
    let java = rendered_for(Some("java")).await;
    let bodies =
        |request: &str| -> Vec<String> { java[request].iter().map(|c| c.body.clone()).collect() };
    // Gauges keep their names in every SDK.
    assert_eq!(bodies("workflow-worker-slots").len(), 1);
    assert!(bodies("workflow-worker-slots")[0].contains("temporal_worker_task_slots_available{"));
    // Micrometer exports timers as `_seconds` histograms, in seconds.
    let sts = &java["wft-schedule-to-start"];
    assert_eq!(sts.len(), 1, "only the Java variant: {sts:?}");
    assert!(
        sts[0]
            .body
            .contains("temporal_workflow_task_schedule_to_start_latency_seconds_bucket{")
    );
    assert!(
        sts[0]
            .body
            .contains(r#"task_queue=~"order-fulfillment(:sticky)?""#)
    );
    assert_eq!(sts[0].unit, Some(kyumei_domain::Unit::Seconds));
    // Counters gain `_total`.
    let pollers = bodies("workflow-pollers");
    assert!(pollers.iter().any(|b| b.contains("temporal_num_pollers{")));
    assert!(
        pollers
            .iter()
            .any(|b| b.contains("temporal_workflow_task_queue_poll_succeed_total{"))
    );
    assert!(
        !pollers
            .iter()
            .any(|b| b.contains("temporal_workflow_task_queue_poll_succeed{"))
    );
    assert_eq!(bodies("poll-failures").len(), 1);
    assert!(bodies("poll-failures")[0].contains("temporal_long_request_failure_total{"));
    // The Java SDK labels sticky polls `<queue>:sticky`, and their failures count too.
    assert!(bodies("poll-failures")[0].contains(r#"task_queue=~"order-fulfillment(:sticky)?""#));
    // With one SDK, the instructions need no "applies to" label.
    assert!(java.values().flatten().all(|c| c.applies_to.is_none()));

    // Core-based Workers get the Core queries, without Java's.
    let python = rendered_for(Some("python")).await;
    let sts = &python["wft-schedule-to-start"];
    assert_eq!(sts.len(), 1);
    assert_eq!(sts[0].unit, Some(kyumei_domain::Unit::Milliseconds));
    // Core labels each poll with its Task Queue, so another queue's failed polls stay out.
    let failures = &python["poll-failures"];
    assert_eq!(failures.len(), 1);
    assert!(
        failures[0]
            .body
            .contains(r#"task_queue="order-fulfillment""#)
    );
    // The Go SDK labels its long polls `task_queue="none"`, so its query reads the Namespace.
    let go = rendered_for(Some("go")).await;
    assert_eq!(go["poll-failures"].len(), 1);
    assert!(!go["poll-failures"][0].body.contains("task_queue"));
    assert!(
        !python
            .values()
            .flatten()
            .any(|c| c.body.contains("_total{"))
    );
}

#[tokio::test]
async fn an_unknown_sdk_gets_every_variant_labelled() {
    let unknown = rendered_for(None).await;
    let sts = &unknown["wft-schedule-to-start"];
    let applies: Vec<Option<&str>> = sts.iter().map(|c| c.applies_to.as_deref()).collect();
    assert_eq!(
        applies,
        [Some("Core SDKs"), Some("Go SDK"), Some("Java SDK")]
    );
    let failures: Vec<Option<&str>> = unknown["poll-failures"]
        .iter()
        .map(|c| c.applies_to.as_deref())
        .collect();
    assert_eq!(
        failures,
        [Some("Core SDKs"), Some("Go SDK"), Some("Java SDK")]
    );
    // A gauge's one query applies to every SDK.
    assert_eq!(unknown["workflow-worker-slots"][0].applies_to, None);
}

#[tokio::test]
async fn a_swept_release_is_told_what_was_checked_for_it() {
    let kb = kb();
    let fx = fixture::golden().expect("golden");
    let (svc, clock) = service(&kb, &fx);
    let (id, _) = fixture::open(&kb, &svc, &fx, Some(&clock), true)
        .await
        .expect("open");
    svc.command(
        &id,
        Command::AmendContext(kyumei_app::ContextInput {
            server_version: Some("1.29.3".to_owned()),
            ..Default::default()
        }),
    )
    .await
    .expect("amend");
    let investigation = svc.snapshot(&id).await.expect("snapshot").investigation;
    let assessment = kyumei_diagnostics::assess(&kb, &investigation).expect("assess");
    assert_eq!(assessment.fit, kyumei_diagnostics::Fit::Swept);
    assert!(
        assessment
            .uncertainties
            .iter()
            .any(|u| u.text.contains("version sweep")),
        "{:?}",
        assessment.uncertainties
    );
    // A setting the sweep found unchanged is verified for the release; a statement read
    // for 1.31 only is not.
    let pack = &kb.packs()[0];
    let v = "1.29.3".parse().expect("version");
    assert!(
        pack.applicability(&"dc.matching.numTaskqueueReadPartitions".into(), Some(&v))
            .is_verified()
    );
    assert!(
        !pack
            .applicability(&"limit.dispatch-applied-on-change".into(), Some(&v))
            .is_verified()
    );
}

#[tokio::test]
async fn findings_quote_categorical_values_as_observed() {
    let kb = kb();
    let mut statements = Vec::new();
    for name in ["history-size-limit", "history-approaching-limits"] {
        let fx = Fixture::load(&fixture::fixtures_dir().join(name)).expect("fixture");
        let (svc, clock) = service(&kb, &fx);
        let (id, _) = fixture::replay(&kb, &svc, &fx, Some(&clock), true)
            .await
            .expect("replay");
        let investigation = svc.snapshot(&id).await.expect("snapshot").investigation;
        let assessment = kyumei_diagnostics::assess(&kb, &investigation).expect("assess");
        statements.extend(assessment.findings.into_iter().map(|f| f.statement));
    }
    assert!(
        statements
            .iter()
            .any(|s| s.contains("\"Workflow history size exceeds limit.\"")),
        "{statements:#?}"
    );
    assert!(
        statements
            .iter()
            .any(|s| s.contains("still running (WORKFLOW_EXECUTION_STATUS_RUNNING)")),
        "{statements:#?}"
    );
}
