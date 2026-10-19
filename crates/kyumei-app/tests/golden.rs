//! The golden investigation, end to end, with no model and no durable runtime.

use std::sync::Arc;

use kyumei_app::{
    InvestigationService, LocalInvestigations,
    fixture::{self, FixtureClock},
};
use kyumei_knowledge::KnowledgeBase;

#[tokio::test]
async fn golden_signal_flood_investigation() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fixture = fixture::golden().expect("fixture");
    let clock = Arc::new(FixtureClock::new(fixture.open.at));
    let service = LocalInvestigations::new(kb.clone(), clock.clone());
    let (id, reports) = match fixture::replay(&kb, &service, &fixture, Some(&clock), true).await {
        Ok(r) => r,
        Err(e) => panic!("{e}"),
    };
    for r in &reports {
        eprintln!(
            "{:<40} next={:<22} conclusion={:?} concluded={}",
            r.label,
            r.next_request.clone().unwrap_or_default(),
            r.conclusion,
            r.concluded
        );
    }
    let snapshot = service.snapshot(&id).await.expect("snapshot");
    let conclusion = snapshot.investigation.conclusion.expect("concluded");
    eprintln!("\n{}\n", conclusion.statement);
    assert!(
        conclusion.statement.contains("2,000/s"),
        "{}",
        conclusion.statement
    );
    assert!(
        !conclusion
            .recommendations
            .iter()
            .any(|r| r.text.to_lowercase().starts_with("add worker"))
    );
}

/// The workspace model and the Markdown export render the golden diagnosis,
/// every finding's evidence, and all three diagrams.
#[tokio::test]
async fn golden_workspace_and_export() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fixture = fixture::golden().expect("fixture");
    let clock = Arc::new(FixtureClock::new(fixture.open.at));
    let service = LocalInvestigations::new(kb.clone(), clock.clone());
    let (id, _) = fixture::replay(&kb, &service, &fixture, Some(&clock), true)
        .await
        .expect("replay");
    let snapshot = service.snapshot(&id).await.expect("snapshot");
    let assessment =
        kyumei_app::core::current_assessment(&kb, &snapshot.investigation).expect("assessment");
    let ws = kyumei_app::workspace::build(&kb, &snapshot, &assessment);
    assert_eq!(ws.status.token, "concluded");
    assert_eq!(
        ws.diagrams.len(),
        3,
        "diagnostic path, system path and evidence relationships"
    );
    for d in &ws.diagrams {
        assert!(d.svg.contains("role=\"img\""));
        assert!(!d.text.is_empty());
    }
    for f in &ws.findings {
        assert!(!f.basis.is_empty(), "{} has no basis", f.id);
        for b in &f.basis {
            assert!(
                !b.sources.is_empty(),
                "{} basis {} has no source",
                f.id,
                b.signal
            );
        }
    }
    let md = kyumei_app::export::markdown(&ws);
    assert!(md.contains("## Diagnosis"));
    assert!(md.contains("Task Queue dispatch rate limit reached under a Signal flood"));
    std::fs::write(std::env::temp_dir().join("kyumei-golden.md"), &md).expect("write");
    let path_text = &ws.diagrams[0].text;
    eprintln!("{path_text}");
    eprintln!("{}", ws.diagrams[1].text);
    for t in &ws.trail {
        eprintln!("{} {:<11} {}", t.time, t.kind, t.text);
    }
}

/// Values the operator types answer the request they respond to: Kyumei moves
/// on to another request instead of issuing the same one again.
#[tokio::test]
async fn typed_values_answer_their_request() {
    use kyumei_app::{Command, EvidenceSubmission, OpenInvestigation, SdkInput, StructuredInput};
    use kyumei_domain::{
        DeclaredContext, InvestigationId, RequestStatus, SignalId, Statistic, SubmissionChannel,
        TimeWindow,
    };

    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let at = |s: &str| -> jiff::Timestamp { s.parse().expect("timestamp") };
    let clock = Arc::new(FixtureClock::new(at("2026-10-13T15:31:38Z")));
    let service = LocalInvestigations::new(kb.clone(), clock);
    let id = service
        .open(OpenInvestigation {
            id: InvestigationId::new("inv-typed-values"),
            title: None,
            description: "Elevated schedule to start latencies".to_owned(),
            signature: None,
            server_version: Some("1.31.2".to_owned()),
            sdks: vec![SdkInput {
                language: "java".to_owned(),
                version: None,
            }],
            topology: None,
            namespaces: Vec::new(),
            task_queues: Vec::new(),
            window_start: Some(at("2026-10-13T11:42:00Z")),
            window_end: Some(at("2026-10-13T11:58:00Z")),
            window_description: None,
            profile: None,
        })
        .await
        .expect("open");
    let first = service
        .snapshot(&id)
        .await
        .expect("snapshot")
        .investigation
        .current_request()
        .cloned()
        .expect("a first request");
    assert_eq!(first.definition.as_str(), "workflow-worker-slots");

    let window =
        TimeWindow::new(at("2026-10-13T11:42:00Z"), at("2026-10-13T11:58:00Z")).expect("window");
    service
        .command(
            &id,
            Command::SubmitEvidence(EvidenceSubmission {
                channel: SubmissionChannel::StructuredEntry,
                text: None,
                bytes_base64: None,
                filename: None,
                media_type: Some("application/json".to_owned()),
                declared: DeclaredContext {
                    signal: Some(SignalId::new("workflow_slots_available")),
                    window: Some(window),
                    ..DeclaredContext::default()
                },
                responds_to: Some(first.id.clone()),
                structured: Some(StructuredInput {
                    signal: "workflow_slots_available".to_owned(),
                    statistic: Statistic::Mean,
                    value: 4769.0,
                    unit: None,
                }),
                origin: None,
                stored: None,
            }),
        )
        .await
        .expect("submit");

    let investigation = service.snapshot(&id).await.expect("snapshot").investigation;
    let answered = investigation
        .requests
        .iter()
        .find(|r| r.id == first.id)
        .expect("the first request");
    assert!(
        matches!(answered.status, RequestStatus::Answered { .. }),
        "{:?}",
        answered.status
    );
    let next = investigation.current_request().expect("a next request");
    assert_ne!(
        next.definition, first.definition,
        "the request the typed values answered was issued again"
    );
}
