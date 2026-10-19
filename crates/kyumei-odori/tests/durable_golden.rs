//! The golden investigation on Odori: durable workflows on the embedded
//! engine, a restart in the middle, and a validated model explanation.

use std::{sync::Arc, time::Duration};

use kyumei_app::{ExplanationStatus, InvestigationService, fixture};
use kyumei_domain::ExplanationAuthor;
use kyumei_knowledge::KnowledgeBase;
use kyumei_odori::{DurableConfig, DurableInvestigations, Explainer};

fn config(dir: &std::path::Path) -> DurableConfig {
    DurableConfig {
        data_dir: dir.to_path_buf(),
        snapshot_interval: Duration::from_secs(60),
        explainer: Some(Explainer::Scripted),
        simulate: false,
    }
}

/// Set `KYUMEI_TEST_LOG` to a file path to keep the logs of a run that hangs,
/// which libtest's output capture would otherwise discard.
fn log_to_file_if_asked() {
    if let Ok(path) = std::env::var("KYUMEI_TEST_LOG")
        && let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_thread_names(true)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .try_init();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn golden_investigation_survives_restart_on_odori() {
    log_to_file_if_asked();
    let dir = tempfile::tempdir().expect("tempdir");
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");

    // Lifetime 1: open and answer the first three requests.
    let service = DurableInvestigations::start(kb.clone(), config(dir.path()))
        .await
        .expect("start");
    let (id, _) = fixture::open(&kb, &service, &fx, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    for i in 0..3 {
        fixture::step(&kb, &service, &fx, &id, i, None, true)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let before = service.snapshot(&id).await.expect("snapshot");
    service.shutdown().await.expect("shutdown");
    drop(service);
    assert!(
        dir.path().join("engine.snapshot").exists(),
        "the engine wrote its snapshot"
    );

    // Lifetime 2: a new engine restores the snapshot; the investigation
    // continues exactly where it stopped.
    let service = DurableInvestigations::start(kb.clone(), config(dir.path()))
        .await
        .expect("restart");
    let after = service.snapshot(&id).await.expect("snapshot after restart");
    let core_events = |s: &kyumei_app::InvestigationSnapshot| {
        s.investigation
            .events
            .iter()
            .filter(|e| {
                !matches!(
                    e.event,
                    kyumei_domain::InvestigationEvent::ExplanationRecorded(_)
                )
            })
            .count()
    };
    assert_eq!(
        core_events(&after),
        core_events(&before),
        "no evidence or assessment lost"
    );
    assert_eq!(
        after.investigation.evidence.len(),
        before.investigation.evidence.len()
    );
    assert_eq!(
        after
            .investigation
            .current_request()
            .map(|r| r.definition.clone()),
        before
            .investigation
            .current_request()
            .map(|r| r.definition.clone())
    );
    assert_eq!(after.runtime.kind, "odori");
    let listed = service.list().await.expect("list");
    assert!(listed.iter().any(|s| s.id == id));
    for i in 3..fx.steps.len() {
        fixture::step(&kb, &service, &fx, &id, i, None, true)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let done = service.snapshot(&id).await.expect("snapshot");
    assert!(done.investigation.conclusion.is_some(), "concluded");

    // The explanation agent (an Odori child workflow) catches up with the
    // final assessment, and its draft passes validation.
    // Allow for a workflow task the engine has to time out and retry (10 s)
    // when the machine is busy, more than once in a heavy soak.
    let mut explained = None;
    for _ in 0..1200 {
        let s = service.snapshot(&id).await.expect("snapshot");
        if let ExplanationStatus::Current { .. } = s.explanation {
            explained = s.model_explanation;
            break;
        }
        if let ExplanationStatus::Fallback { reason, .. } = &s.explanation {
            panic!("model explanation rejected: {reason}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let explanation = explained.expect("a model explanation for the final assessment");
    assert!(matches!(
        explanation.author,
        ExplanationAuthor::Model { .. }
    ));
    assert!(explanation.summary.starts_with("In short:"));
    assert_eq!(
        explanation.findings.len(),
        done.investigation
            .latest_assessment()
            .map_or(0, |a| a.findings.len())
    );
    service.shutdown().await.expect("shutdown");
}

/// A process that stops between engine snapshots loses nothing: the engine
/// comes back with an older workflow, and the projection written after every
/// command restores what the snapshot lacks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commands_after_the_last_snapshot_survive_a_crash() {
    log_to_file_if_asked();
    let dir = tempfile::tempdir().expect("tempdir");
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let config = || DurableConfig {
        data_dir: dir.path().to_path_buf(),
        snapshot_interval: Duration::from_secs(3600),
        explainer: None,
        simulate: false,
    };
    let snapshot_file = dir.path().join("engine.snapshot");

    // Lifetime 1: open and answer one request; a clean stop writes a snapshot.
    let service = DurableInvestigations::start(kb.clone(), config())
        .await
        .expect("start");
    let (id, _) = fixture::open(&kb, &service, &fx, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    fixture::step(&kb, &service, &fx, &id, 0, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    service.shutdown().await.expect("shutdown");
    drop(service);
    let stale = std::fs::read(&snapshot_file).expect("snapshot written");

    // Lifetime 2: answer the next request, then simulate a crash before the
    // next snapshot by putting the older snapshot back.
    let service = DurableInvestigations::start(kb.clone(), config())
        .await
        .expect("restart");
    fixture::step(&kb, &service, &fx, &id, 1, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let before = service.snapshot(&id).await.expect("snapshot");
    service.shutdown().await.expect("shutdown");
    drop(service);
    std::fs::write(&snapshot_file, stale).expect("restore the older snapshot");

    // Lifetime 3: the engine restores the older workflow; Kyumei restores
    // the events it is missing from the projection, and the investigation
    // continues from where the operator left it.
    let service = DurableInvestigations::start(kb.clone(), config())
        .await
        .expect("restart after crash");
    let after = service.snapshot(&id).await.expect("snapshot after crash");
    let seqs = |s: &kyumei_app::InvestigationSnapshot| {
        s.investigation
            .events
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>()
    };
    assert_eq!(seqs(&after), seqs(&before), "no event lost");
    assert_eq!(
        after.investigation.evidence.len(),
        before.investigation.evidence.len(),
        "the evidence submitted after the snapshot is still there"
    );
    assert!(
        after
            .runtime
            .note
            .as_deref()
            .is_some_and(|n| n.starts_with("Restored ")
                && n.ends_with(
                    "recorded after the engine's last snapshot from the saved event log."
                )),
        "the restore is reported: {:?}",
        after.runtime.note
    );
    fixture::step(&kb, &service, &fx, &id, 2, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    service.shutdown().await.expect("shutdown");
}

/// A snapshot the engine cannot read (written by an older engine release,
/// or damaged) does not lose investigations: the file is kept, the engine
/// starts empty, and every investigation is restored from its projection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreadable_snapshot_is_kept_and_investigations_restored() {
    log_to_file_if_asked();
    let dir = tempfile::tempdir().expect("tempdir");
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let config = || DurableConfig {
        data_dir: dir.path().to_path_buf(),
        snapshot_interval: Duration::from_secs(3600),
        explainer: None,
        simulate: false,
    };
    let snapshot_file = dir.path().join("engine.snapshot");

    let service = DurableInvestigations::start(kb.clone(), config())
        .await
        .expect("start");
    let (id, _) = fixture::open(&kb, &service, &fx, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    for i in 0..2 {
        fixture::step(&kb, &service, &fx, &id, i, None, true)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let before = service.snapshot(&id).await.expect("snapshot");
    service.shutdown().await.expect("shutdown");
    drop(service);
    let unreadable = b"written by another engine release".to_vec();
    std::fs::write(&snapshot_file, &unreadable).expect("replace the snapshot");

    let service = DurableInvestigations::start(kb.clone(), config())
        .await
        .expect("starts despite the unreadable snapshot");
    let after = service.snapshot(&id).await.expect("restored");
    let seqs = |s: &kyumei_app::InvestigationSnapshot| {
        s.investigation
            .events
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>()
    };
    assert_eq!(seqs(&after), seqs(&before), "no event lost");
    let note = after.runtime.note.clone().unwrap_or_default();
    assert!(
        note.starts_with(
            "Restored from the saved event log: the engine could not read its snapshot"
        ),
        "{note}"
    );
    let kept: Vec<_> = std::fs::read_dir(dir.path())
        .expect("data dir")
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("engine.snapshot.unreadable-")
        })
        .collect();
    assert_eq!(kept.len(), 1, "the unreadable snapshot is kept");
    assert_eq!(
        std::fs::read(kept[0].path()).expect("kept snapshot"),
        unreadable
    );
    fixture::step(&kb, &service, &fx, &id, 2, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    service.shutdown().await.expect("shutdown");
}

/// A projection that fell behind the engine's log (a command whose view could
/// not be read, a model explanation recorded after the last command) is
/// brought up to date when Kyumei starts and before it stops, so an engine
/// that cannot read its predecessor's snapshot loses nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_projection_left_behind_is_brought_up_to_date() {
    log_to_file_if_asked();
    let dir = tempfile::tempdir().expect("tempdir");
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let config = || DurableConfig {
        data_dir: dir.path().to_path_buf(),
        snapshot_interval: Duration::from_secs(3600),
        explainer: None,
        simulate: false,
    };
    let read = |path: &std::path::Path| -> Vec<kyumei_domain::RecordedEvent> {
        serde_json::from_slice(&std::fs::read(path).expect("projection")).expect("events")
    };
    let leave_behind = |path: &std::path::Path| {
        let first = read(path).into_iter().take(1).collect::<Vec<_>>();
        std::fs::write(path, serde_json::to_vec(&first).expect("json")).expect("write");
    };

    let service = DurableInvestigations::start(kb.clone(), config())
        .await
        .expect("start");
    let (id, _) = fixture::open(&kb, &service, &fx, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    for i in 0..2 {
        fixture::step(&kb, &service, &fx, &id, i, None, true)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let all = service
        .snapshot(&id)
        .await
        .expect("snapshot")
        .investigation
        .events;
    let projection = dir.path().join("projections").join(format!("{id}.json"));
    assert_eq!(read(&projection), all);
    service.shutdown().await.expect("shutdown");
    drop(service);

    // The engine's snapshot holds every event; the projection only the first.
    leave_behind(&projection);
    let service = DurableInvestigations::start(kb.clone(), config())
        .await
        .expect("restart");
    assert_eq!(
        read(&projection),
        all,
        "brought up to date when Kyumei starts"
    );

    leave_behind(&projection);
    service.shutdown().await.expect("shutdown");
    drop(service);
    assert_eq!(
        read(&projection),
        all,
        "brought up to date before Kyumei stops"
    );

    // The next engine cannot read the snapshot and rebuilds from the projection.
    std::fs::write(
        dir.path().join("engine.snapshot"),
        b"written by another engine release",
    )
    .expect("replace the snapshot");
    let service = DurableInvestigations::start(kb.clone(), config())
        .await
        .expect("starts despite the unreadable snapshot");
    let after = service.snapshot(&id).await.expect("restored");
    assert_eq!(after.investigation.events, all, "no event lost");
    service.shutdown().await.expect("shutdown");
}

/// Every fixture replays on Odori with its expectations met, one after
/// another on one engine, and each conclusion survives a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_fixture_replays_on_odori() {
    log_to_file_if_asked();
    let dir = tempfile::tempdir().expect("tempdir");
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fixtures = fixture::all().expect("fixtures load");
    let config = || DurableConfig {
        data_dir: dir.path().to_path_buf(),
        snapshot_interval: Duration::from_secs(60),
        explainer: None,
        simulate: false,
    };

    let service = DurableInvestigations::start(kb.clone(), config())
        .await
        .expect("start");
    let mut replayed = Vec::new();
    for fx in &fixtures {
        let (id, _) = fixture::replay(&kb, &service, fx, None, true)
            .await
            .unwrap_or_else(|e| panic!("fixture {}: {e}", fx.fixture.id));
        let conclusion = service
            .snapshot(&id)
            .await
            .expect("snapshot")
            .investigation
            .conclusion
            .map(|c| c.id);
        replayed.push((fx.fixture.id.clone(), id, conclusion));
    }
    assert!(
        replayed.iter().all(|(_, _, c)| c.is_some()),
        "every fixture concludes: {replayed:?}"
    );
    service.shutdown().await.expect("shutdown");
    drop(service);

    let service = DurableInvestigations::start(kb.clone(), config())
        .await
        .expect("restart");
    for (fixture, id, conclusion) in &replayed {
        let restored = service
            .snapshot(id)
            .await
            .unwrap_or_else(|e| panic!("fixture {fixture}: {e}"));
        assert_eq!(
            restored.investigation.conclusion.map(|c| c.id).as_ref(),
            conclusion.as_ref(),
            "fixture {fixture}"
        );
    }
    service.shutdown().await.expect("shutdown");
}

/// Prometheus answers a quantile over a step without traffic with NaN. Those
/// samples travel through the engine's JSON payloads and come back as NaN.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nan_samples_are_recorded_and_restored() {
    use kyumei_app::{Command, EvidenceSubmission};
    use kyumei_domain::{DeclaredContext, EvidenceForm, ObservedValue, SubmissionChannel};
    use kyumei_samples::{Point, QueryResult, Samples, Series, Window};

    log_to_file_if_asked();
    let dir = tempfile::tempdir().expect("tempdir");
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let service = DurableInvestigations::start(kb.clone(), config(dir.path()))
        .await
        .expect("start");
    let (id, _) = fixture::open(&kb, &service, &fx, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let request = service
        .snapshot(&id)
        .await
        .expect("snapshot")
        .investigation
        .current_request()
        .expect("request")
        .clone();
    let start: jiff::Timestamp = "2026-09-30T13:20:00Z".parse().expect("ts");
    let start = start.as_millisecond();
    let samples = Samples {
        collector: "kyumei test".to_owned(),
        source: "prometheus".to_owned(),
        collected_at_ms: start + 3_600_000,
        investigation: Some(id.to_string()),
        request: Some(request.id.to_string()),
        window: Window {
            start_ms: start,
            end_ms: start + 40 * 60_000,
            step_ms: 60_000,
        },
        rounded_to: None,
        redacted: Vec::new(),
        queries: vec![QueryResult {
            query: "temporal_worker_task_slots_available{worker_type=\"WorkflowWorker\"}"
                .to_owned(),
            series: vec![Series {
                labels: [("task_queue".to_owned(), "order-fulfillment".to_owned())].into(),
                points: (0..40)
                    .map(|i| Point {
                        at_ms: start + i * 60_000,
                        value: if i % 3 == 0 {
                            f64::NAN
                        } else {
                            100.0 + i as f64
                        },
                    })
                    .collect(),
            }],
        }],
    };
    let submission = EvidenceSubmission {
        declared: DeclaredContext {
            form: Some(EvidenceForm::SampleBlock),
            ..DeclaredContext::default()
        },
        responds_to: Some(request.id.clone()),
        ..EvidenceSubmission::text(
            SubmissionChannel::Collector,
            kyumei_samples::encode(&samples),
        )
    };
    service
        .command(&id, Command::SubmitEvidence(submission))
        .await
        .expect("a block with NaN samples is applied");

    let nan_points = |inv: &kyumei_domain::Investigation| -> usize {
        inv.evidence
            .iter()
            .flat_map(|e| &e.interpretation.observations)
            .map(|o| match &o.value {
                ObservedValue::Series { points, .. } => {
                    points.iter().filter(|p| p.value.is_nan()).count()
                }
                _ => 0,
            })
            .sum()
    };
    // Later commands carry the event log, NaN samples included.
    let observation = |text: &str| {
        Command::SubmitEvidence(EvidenceSubmission::text(
            SubmissionChannel::Observation,
            text,
        ))
    };
    service
        .command(&id, observation("Workers restarted at 13:40."))
        .await
        .expect("a later command is applied");
    let before = service.snapshot(&id).await.expect("snapshot").investigation;
    assert_eq!(nan_points(&before), 14, "NaN samples are kept as NaN");
    service.shutdown().await.expect("shutdown");
    drop(service);

    let service = DurableInvestigations::start(kb.clone(), config(dir.path()))
        .await
        .expect("restart");
    let after = service.snapshot(&id).await.expect("snapshot").investigation;
    assert_eq!(nan_points(&after), 14, "and restored as NaN");
    service
        .command(&id, observation("Workers were scaled up at 13:45."))
        .await
        .expect("a command after the restart is applied");
    service.shutdown().await.expect("shutdown");
}

/// A submission sent again under its id, as after the engine reports an update
/// unprocessed although it applied it, gets the reply it had and is applied once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_submission_sent_again_is_applied_once() {
    use kyumei_app::{Command, EvidenceSubmission};
    use kyumei_domain::SubmissionChannel;
    use kyumei_odori::workflow::Submission;

    log_to_file_if_asked();
    let dir = tempfile::tempdir().expect("tempdir");
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let service = DurableInvestigations::start(kb.clone(), config(dir.path()))
        .await
        .expect("start");
    let (id, _) = fixture::open(&kb, &service, &fx, None, true)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let submission = Submission {
        id: "submission-1".to_owned(),
        command: Command::SubmitEvidence(EvidenceSubmission::text(
            SubmissionChannel::Observation,
            "Workers restarted at 13:40.",
        )),
    };
    let first = service
        .submit(&id, submission.clone())
        .await
        .expect("first");
    let again = service.submit(&id, submission).await.expect("again");
    assert_eq!(first.evidence, again.evidence, "the same reply");
    assert_eq!(first.events, again.events);
    let inv = service.snapshot(&id).await.expect("snapshot").investigation;
    assert_eq!(
        inv.evidence.len(),
        2,
        "E0 and one observation: {:?}",
        inv.evidence
            .iter()
            .map(|e| e.id.to_string())
            .collect::<Vec<_>>()
    );
    service.shutdown().await.expect("shutdown");
}

/// On Odori, the simulation the golden investigation's evidence calls for
/// runs as an activity, and its result is recorded in the workflow's history.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_golden_dispatch_limit_is_simulated_on_odori() {
    log_to_file_if_asked();
    let dir = tempfile::tempdir().expect("tempdir");
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let service = DurableInvestigations::start(
        kb.clone(),
        DurableConfig {
            simulate: true,
            ..DurableConfig::new(dir.path())
        },
    )
    .await
    .expect("start");
    let (id, _) = fixture::replay(&kb, &service, &fx, None, false)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let mut simulated = None;
    for _ in 0..1200 {
        let s = service.snapshot(&id).await.expect("snapshot");
        if let Some(e) = s
            .investigation
            .evidence
            .iter()
            .find(|e| e.simulation.is_some())
        {
            simulated = Some((e.clone(), s.investigation.clone()));
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (evidence, investigation) = simulated.expect("a simulation, recorded");
    let record = evidence.simulation.as_ref().expect("record");
    assert_eq!(record.simulation.as_str(), "dispatch-ceiling-size");
    assert!(record.failure.is_none(), "{record:#?}");
    assert!(matches!(
        evidence.origin,
        kyumei_domain::EvidenceOrigin::Simulation { seed: 1, .. }
    ));
    let assessment = kyumei_app::core::current_assessment(&kb, &investigation).expect("assessment");
    assert!(
        assessment
            .findings
            .iter()
            .any(|f| f.rule.as_str() == "ceiling-big-enough" && f.rests_on_simulation()),
        "{:#?}",
        assessment.findings
    );
    // It runs once: the next assessments find it recorded.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let later = service.snapshot(&id).await.expect("snapshot");
    assert_eq!(
        later
            .investigation
            .evidence
            .iter()
            .filter(|e| e.simulation.is_some())
            .count(),
        1
    );
    service.shutdown().await.expect("shutdown");
}
