//! Drive the interface through the golden investigation with real form posts.

use std::sync::Arc;

use kyumei_app::{
    InvestigationService, LocalInvestigations,
    fixture::{self, FixtureClock},
};
use kyumei_knowledge::KnowledgeBase;
use topcoat::router::{Method, StatusCode};

mod common;
use common::send;

#[tokio::test]
async fn golden_investigation_through_the_interface() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let clock = Arc::new(FixtureClock::new(fx.open.at));
    let service: Arc<dyn InvestigationService> =
        Arc::new(LocalInvestigations::new(kb.clone(), clock.clone()));
    let router = kyumei_web::router(kb.clone(), service.clone(), "test".to_owned());

    let landing = send(&router, Method::GET, "/", None).await;
    assert_eq!(landing.status, StatusCode::OK);
    assert!(landing.body.starts_with("<!DOCTYPE html>"));
    assert!(landing.body.contains("What are you seeing?"));
    // Each symptom is offered once, though both packs diagnose it.
    assert_eq!(
        landing
            .body
            .matches("value=\"wft-schedule-to-start-elevated\"")
            .count(),
        1
    );

    // Open the investigation as an operator would.
    let created = send(
        &router,
        Method::POST,
        "/investigations",
        Some(&[
            ("description", fx.open.description.as_str()),
            ("server_version", "1.31.0"),
            ("sdk_language", "python"),
            ("sdk_version", "1.25.0"),
            ("namespace", "commerce-prod"),
            ("task_queue", "order-fulfillment"),
            ("window_start", "2026-09-30T13:35"),
            ("window_end", "2026-09-30T13:50"),
            ("deployment", "self_hosted_kubernetes"),
            ("matching_hosts", "4"),
        ]),
    )
    .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    let workspace_path = created.location.clone();
    assert!(workspace_path.starts_with("/investigations/inv-"));
    let id = workspace_path
        .trim_start_matches("/investigations/")
        .to_owned();

    let page = send(&router, Method::GET, &workspace_path, None).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Next evidence required"));
    assert!(
        page.body
            .contains("Free Workflow Task slots across Workers")
    );
    assert!(page.body.contains("temporal_worker_task_slots_available{worker_type=&quot;WorkflowWorker&quot;, namespace=~&quot;commerce-prod|commerce_prod&quot;"),
        "the query is rendered with the investigation's scope filled in, in both spellings for Go Workers");
    assert!(
        !page.body.contains("&lt;svg"),
        "diagrams are not double-escaped"
    );
    // The request links its runbook section and, for the Python Workers, the Core SDK
    // dashboard's panel, at the operations repository's pinned commit.
    assert!(page.body.contains("Where operators look"));
    assert!(page.body.contains("/blob/5a71c3656cc0bca831b61142b64c3672cc974b1b/observability/alerts/sdk/runbooks/14-worker-slots-exhausted.md#workflowworker"));
    assert!(
        page.body
            .contains("panel 34 of Temporal Core SDK Dashboard")
    );
    assert!(
        !page.body.contains("Temporal Go SDK Dashboard"),
        "only the investigation's SDK"
    );
    assert!(page.body.contains("Runbooks for this symptom"));

    // Answer each request from the fixture through the paste form.
    for step in &fx.steps {
        let snapshot = service
            .snapshot(&id.as_str().into())
            .await
            .expect("snapshot");
        let request = snapshot
            .investigation
            .current_request()
            .expect("open request");
        assert_eq!(request.definition.as_str(), step.answers);
        let text = std::fs::read_to_string(fx.dir.join(step.file.as_ref().expect("file")))
            .expect("evidence file");
        let window_start = step.window_start.map(|t| t.to_string()).unwrap_or_default();
        let reply = send(
            &router,
            Method::POST,
            &format!("/investigations/{id}/evidence"),
            Some(&[
                ("mode", "paste"),
                ("request", request.id.as_str()),
                ("text", &text),
                ("query", step.query.as_deref().unwrap_or("")),
                ("window_start", &window_start),
                ("note", step.note.as_deref().unwrap_or("")),
            ]),
        )
        .await;
        assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.body);
        assert!(reply.location.contains("?received=E"), "{}", reply.location);
        let page = send(&router, Method::GET, &reply.location, None).await;
        assert_eq!(page.status, StatusCode::OK);
        assert!(page.body.contains("received"));
    }

    let page = send(&router, Method::GET, &workspace_path, None).await;
    assert!(page.body.contains("Diagnosis reached"));
    assert!(
        page.body
            .contains("Task Queue dispatch rate limit reached under a Signal flood")
    );
    assert!(page.body.contains("<svg"), "diagrams render inline");
    // A 1.31 investigation's claims link to the 1.31 knowledge.
    assert!(page.body.contains("/knowledge?pack=temporal-1.31"));
    assert!(page.body.contains("?pack=temporal-1.31\" title="));
    assert!(page.body.contains("Evidence relationships"));
    assert!(page.body.contains("F-slots-headroom"));

    // Evidence: raw beside interpretation, then a correction that keeps the raw intact.
    let evidence = send(
        &router,
        Method::GET,
        &format!("/investigations/{id}/evidence/E1"),
        None,
    )
    .await;
    assert_eq!(evidence.status, StatusCode::OK);
    assert!(evidence.body.contains("As submitted"));
    assert!(evidence.body.contains("Kyumei’s interpretation"));
    let corrected = send(
        &router,
        Method::POST,
        &format!("/investigations/{id}/evidence/E1/corrections"),
        Some(&[
            ("observation", "E1.O1"),
            ("change", "exclude"),
            ("reason", "checking the interface"),
        ]),
    )
    .await;
    assert_eq!(
        corrected.status,
        StatusCode::SEE_OTHER,
        "{}",
        corrected.body
    );
    let after = service
        .snapshot(&id.as_str().into())
        .await
        .expect("snapshot");
    let e1 = after.investigation.evidence(&"E1".into()).expect("E1");
    assert_eq!(
        e1.raw.text().map(str::len),
        Some(
            std::fs::read_to_string(fx.dir.join("evidence/e1-worker-slots.json"))
                .expect("e1")
                .len()
        ),
        "raw evidence is preserved byte for byte"
    );
    let page = send(&router, Method::GET, &workspace_path, None).await;
    assert!(
        page.body.contains("Diagnosis withdrawn")
            || page.body.contains("Workflow Worker slots exhausted"),
        "excluding E1 reopens Worker capacity"
    );

    let md = send(
        &router,
        Method::GET,
        &format!("/investigations/{id}/export.md"),
        None,
    )
    .await;
    assert_eq!(md.status, StatusCode::OK);
    assert!(md.content_type.starts_with("text/markdown"));
    assert!(md.body.contains("## Trail"));

    // The knowledge overview counts the newest pack's claims by kind, and offers the
    // others; search lists them, each once.
    let knowledge = send(&router, Method::GET, "/knowledge", None).await;
    assert_eq!(knowledge.status, StatusCode::OK);
    assert!(
        knowledge
            .body
            .contains("Temporal Server 1.32 diagnostic knowledge")
    );
    assert!(knowledge.body.contains("kind=dynamic-config"));
    assert!(knowledge.body.contains("Claims a diagnostic uses"));
    assert!(knowledge.body.contains("/knowledge?pack=temporal-1.31"));
    let older = send(&router, Method::GET, "/knowledge?pack=temporal-1.31", None).await;
    assert!(
        older
            .body
            .contains("Temporal Server 1.31 diagnostic knowledge")
    );
    let once = send(
        &router,
        Method::GET,
        "/knowledge?q=numTaskqueueReadPartitions&kind=dynamic-config",
        None,
    )
    .await;
    assert!(once.body.contains("1 matching claims"), "{}", once.body);
    let unknown = send(&router, Method::GET, "/knowledge?pack=temporal-1.0", None).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let found = send(&router, Method::GET, "/knowledge?q=worker_task_slots", None).await;
    assert!(found.body.contains("worker_task_slots_available"));
    // Every generated setting is searchable; filters narrow by kind and use.
    let settings = send(
        &router,
        Method::GET,
        "/knowledge?kind=dynamic-config&used=1",
        None,
    )
    .await;
    assert!(
        settings
            .body
            .contains("matching.numTaskqueueReadPartitions")
    );
    assert!(
        !settings.body.contains("history.rps<"),
        "settings no diagnostic uses are filtered out"
    );
    let unused = send(&router, Method::GET, "/knowledge?q=history.rps", None).await;
    assert!(
        unused
            .body
            .contains("HistoryRPS is request rate per second")
    );
    // A claim page says what uses the claim.
    let partitions = send(
        &router,
        Method::GET,
        "/knowledge/dc.matching.numTaskqueueReadPartitions",
        None,
    )
    .await;
    assert!(partitions.body.contains("Used by"));
    // A claim is read from the newest pack that has it, with links to the others.
    assert!(
        partitions
            .body
            .contains("Temporal Server 1.32 diagnostic knowledge")
    );
    assert!(
        partitions
            .body
            .contains("/knowledge/dc.matching.numTaskqueueReadPartitions?pack=temporal-1.31")
    );
    let only_newer = send(
        &router,
        Method::GET,
        "/knowledge/api.task_queue_stats.rate_limiting_active?pack=temporal-1.31",
        None,
    )
    .await;
    assert_eq!(only_newer.status, StatusCode::NOT_FOUND);
    assert!(
        partitions
            .body
            .contains("namespace, taskQueueName, taskType")
    );
    assert!(
        partitions
            .body
            .contains("NewTaskQueueIntSettingWithConstrainedDefault")
    );
    assert!(partitions.body.contains("signal read_partitions"));
    assert!(partitions.body.contains("rule limit-from-partitions"));
    assert!(
        partitions
            .body
            .contains("Pollers are spread across read partitions."),
        "annotation notes are merged into the generated claim"
    );
    let claim = send(
        &router,
        Method::GET,
        "/knowledge/sdk.worker_task_slots_available",
        None,
    )
    .await;
    assert_eq!(claim.status, StatusCode::OK);
    assert!(
        claim
            .body
            .contains("crates/sdk-core/src/telemetry/metrics.rs:21")
    );
    assert!(claim.body.contains("https://github.com/tokeira/sdk-rust/blob/acac9e3f98c64eda54805f65779ddf2b313c8cb2/crates/sdk-core/src/telemetry/metrics.rs#L21"));
    // Server metrics are generated: a claim page shows the definition, the packages that
    // record the metric, the reviewed label set and every exposition.
    let backlog = send(
        &router,
        Method::GET,
        "/knowledge/server.approximate_backlog_age_seconds",
        None,
    )
    .await;
    assert!(
        backlog
            .body
            .contains("metrics.ApproximateBacklogAgeSeconds")
    );
    assert!(backlog.body.contains("service/matching"));
    assert!(
        backlog.body.contains("worker_version"),
        "the annotated label set is merged into the generated claim"
    );
    assert!(backlog.body.contains("signal backlog_age"));
    let latency = send(
        &router,
        Method::GET,
        "/knowledge/server.service_latency",
        None,
    )
    .await;
    assert!(latency.body.contains("service_latency_milliseconds"));
    assert!(latency.body.contains("recordTimerInSeconds"));
    assert!(latency.body.contains("Not described in the source."));
    let worker = send(
        &router,
        Method::GET,
        "/knowledge?kind=worker-service-metric",
        None,
    )
    .await;
    assert!(worker.body.contains("batcher_processor_requests"));
    assert!(!worker.body.contains("service_latency<"));

    let missing = send(&router, Method::GET, "/investigations/nope", None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let css = send(&router, Method::GET, "/app.css", None).await;
    assert!(css.content_type.starts_with("text/css"));
}

/// A description Kyumei does not recognise opens an investigation that says
/// so; choosing the symptom under "Edit context" starts the diagnosis.
#[tokio::test]
async fn an_unrecognised_symptom_can_be_chosen() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let clock = Arc::new(FixtureClock::new(
        "2026-09-30T13:52:00Z".parse().expect("timestamp"),
    ));
    let service: Arc<dyn InvestigationService> =
        Arc::new(LocalInvestigations::new(kb.clone(), clock));
    let router = kyumei_web::router(kb.clone(), service.clone(), "test".to_owned());

    let created = send(
        &router,
        Method::POST,
        "/investigations",
        Some(&[
            (
                "description",
                "Orders are stuck and customers are complaining.",
            ),
            ("namespace", "commerce-prod"),
            ("task_queue", "order-fulfillment"),
        ]),
    )
    .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    let path = created.location.clone();
    let id = path.trim_start_matches("/investigations/").to_owned();
    let page = send(&router, Method::GET, &path, None).await;
    assert!(page.body.contains("has not been matched to a diagnostic"));
    assert!(
        page.body
            .contains(r#"<details class="edit-context" id="edit-context" open"#),
        "the form to choose a symptom starts open"
    );
    let snapshot = service
        .snapshot(&id.as_str().into())
        .await
        .expect("snapshot");
    assert!(snapshot.investigation.current_request().is_none());

    let unknown = send(
        &router,
        Method::POST,
        &format!("{path}/context"),
        Some(&[("signature", "no-such-symptom")]),
    )
    .await;
    assert!(unknown.location.contains("error="), "{}", unknown.location);

    let chosen = send(
        &router,
        Method::POST,
        &format!("{path}/context"),
        Some(&[("signature", "wft-schedule-to-start-elevated")]),
    )
    .await;
    assert_eq!(chosen.status, StatusCode::SEE_OTHER, "{}", chosen.body);
    assert!(!chosen.location.contains("error="), "{}", chosen.location);
    let snapshot = service
        .snapshot(&id.as_str().into())
        .await
        .expect("snapshot");
    let request = snapshot
        .investigation
        .current_request()
        .expect("the diagnosis starts");
    assert_eq!(request.definition.as_str(), "workflow-worker-slots");
    let page = send(&router, Method::GET, &path, None).await;
    assert!(
        page.body
            .contains("Free Workflow Task slots across Workers")
    );
}
