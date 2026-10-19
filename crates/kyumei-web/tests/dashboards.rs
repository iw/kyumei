//! Dashboards through the interface: an upload kept with the data, evidence drawn as
//! its panel, the panels checked against the knowledge, and the runbooks and panels
//! the diagnosis links to.

use std::sync::{Arc, RwLock};

use kyumei_app::{
    InvestigationService, LocalInvestigations,
    fixture::{self, Fixture, FixtureClock},
};
use kyumei_knowledge::KnowledgeBase;
use kyumei_web::AppState;
use topcoat::router::{Body, Method, StatusCode, header, request::Request, to_bytes};

mod common;
use common::send;

/// A dashboard written for this test: visibility lag in milliseconds, by `operation`.
const DASHBOARD: &str = r##"{
  "uid": "test-visibility", "title": "Test Visibility v0.1", "version": 2, "schemaVersion": 38,
  "panels": [
    {"id": 9, "type": "timeseries", "title": "Visibility lag",
     "targets": [{"refId": "A", "legendFormat": "{{operation}}",
       "expr": "histogram_quantile(0.99, sum(rate(task_latency_queue_bucket{operation=~\"VisibilityTask.*\"}[$__rate_interval])) by (operation, le))"}],
     "fieldConfig": {"defaults": {"unit": "ms", "custom": {"thresholdsStyle": {"mode": "dashed"}},
       "thresholds": {"mode": "absolute", "steps": [{"color": "green", "value": null}, {"color": "orange", "value": 60000}]}}},
     "options": {"legend": {"displayMode": "table", "calcs": ["min", "max", "lastNotNull"]}}},
    {"id": 10, "type": "timeseries", "title": "Units",
     "targets": [{"refId": "A", "expr": "histogram_quantile(0.99, sum by (le) (rate(task_latency_queue_bucket[1m])))"}],
     "fieldConfig": {"defaults": {"unit": "ms"}}}
  ]
}"##;

async fn upload(router: &topcoat::router::Router, name: &str, bytes: &str) -> (StatusCode, String) {
    let boundary = "kyumei-test-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/json\r\n\r\n{bytes}\r\n--{boundary}--\r\n"
    );
    let request = Request::builder()
        .method(Method::POST)
        .uri("/dashboards")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("request");
    let response = router.handle(request).await;
    let (parts, body) = response.into_parts();
    let _ = to_bytes(body, usize::MAX).await;
    let location = parts
        .headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    (parts.status, location)
}

#[tokio::test]
async fn dashboards_are_uploaded_kept_checked_and_drawn() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx =
        Fixture::load(&fixture::fixtures_dir().join("visibility-es-slow-writes")).expect("fixture");
    let clock = Arc::new(FixtureClock::new(fx.open.at));
    let service = Arc::new(LocalInvestigations::new(kb.clone(), clock.clone()));
    let (id, _) = fixture::replay(&kb, service.as_ref(), &fx, Some(&clock), true)
        .await
        .expect("replay");
    let dir = std::env::temp_dir().join(format!("kyumei-web-dashboards-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let service: Arc<dyn InvestigationService> = service;
    let router = kyumei_web::router_for(AppState {
        dashboards: Arc::new(RwLock::new(kyumei_dashboards::Library::default())),
        dashboards_dir: Some(dir.clone()),
        grafana_url: Some("https://grafana.example.com".to_owned()),
        ..AppState::new(kb, service, "test".to_owned())
    });

    // Nothing loaded yet: the page says how to load dashboards, and evidence has no charts.
    let page = send(&router, Method::GET, "/dashboards", None).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.content_type.starts_with("text/html"),
        "{}",
        page.content_type
    );
    assert!(page.location.is_empty());
    assert!(page.body.contains("No dashboards are loaded"));
    let evidence = send(
        &router,
        Method::GET,
        &format!("/investigations/{id}/evidence/E1"),
        None,
    )
    .await;
    assert!(!evidence.body.contains("As your dashboards draw it"));

    // What is not a dashboard is refused; a dashboard is loaded and kept.
    let (status, location) = upload(&router, "notes.json", r#"{"title": "notes"}"#).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(
        location.contains("error=notes.json+cannot+be+loaded"),
        "{location}"
    );
    let (status, location) = upload(&router, "visibility.json", DASHBOARD).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location, "/dashboards?added=test-visibility");
    assert!(
        dir.join("test-visibility.json").is_file(),
        "the upload is kept with the data"
    );

    let page = send(&router, Method::GET, "/dashboards", None).await;
    assert!(page.body.contains("Test Visibility v0.1"));
    assert!(page.body.contains("uploaded as visibility.json"));
    assert!(
        page.body.contains("Same value, another label"),
        "the panel reads `operation`"
    );
    // The panels are checked against the newest pack, and any other on request.
    assert!(page.body.contains("Checked against the 1.32 knowledge"));
    assert!(page.body.contains("/dashboards?pack=temporal-1.31#checks"));
    let older = send(&router, Method::GET, "/dashboards?pack=temporal-1.31", None).await;
    assert!(older.body.contains("Checked against the 1.31 knowledge"));
    assert!(older.body.contains("?pack=temporal-1.31"));

    // E1 is drawn as the panel, in its unit, with a link into Grafana.
    let evidence = send(
        &router,
        Method::GET,
        &format!("/investigations/{id}/evidence/E1"),
        None,
    )
    .await;
    assert!(evidence.body.contains("As your dashboards draw it"));
    assert!(
        evidence
            .body
            .contains("<svg xmlns=\"http://www.w3.org/2000/svg\" class=\"kc\"")
    );
    assert!(
        evidence
            .body
            .contains("Panel “Visibility lag” (id 9) of Test Visibility v0.1, version 2")
    );
    assert!(
        evidence
            .body
            .contains("https://grafana.example.com/d/test-visibility?viewPanel=9")
    );
    assert!(evidence.body.contains("VisibilityTaskStartExecution"));

    // The diagnosis links its runbooks at the pinned commit.
    let workspace = send(&router, Method::GET, &format!("/investigations/{id}"), None).await;
    assert!(workspace.body.contains("Runbooks and playbooks"));
    assert!(workspace.body.contains("https://github.com/tsurdilo/temporal-server-operations/blob/5a71c3656cc0bca831b61142b64c3672cc974b1b/observability/alerts/server/runbooks/84-visibility-store-not-acknowledging-writes.md"));
    assert!(
        workspace
            .body
            .contains("tsurdilo/temporal-server-operations at 5a71c36")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
