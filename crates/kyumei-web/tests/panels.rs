//! Linked panels opened in Kyumei: their queries read live and drawn as the panel
//! draws them, on their own and for an investigation, and added as evidence.

use std::sync::{Arc, RwLock};

use kyumei_app::{
    InvestigationService, LocalInvestigations,
    fixture::{self, Fixture, FixtureClock},
};
use kyumei_collect::{Collector, Endpoint};
use kyumei_dashboards::{Library, Origin};
use kyumei_domain::{EvidenceForm, EvidenceOrigin, InvestigationId};
use kyumei_knowledge::KnowledgeBase;
use kyumei_web::AppState;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use topcoat::router::{Method, Router, StatusCode};

mod common;
use common::send;

/// The operations repository's panel 82, as far as this test needs it: visibility
/// task latency in seconds, named by `task_type`, with the quantile `$p`.
const DASHBOARD: &str = r##"{
  "uid": "temporal-overview-v1", "title": "Temporal Server Dashboard v2.25.0", "version": 11,
  "templating": {"list": [
    {"name": "namespace", "type": "query", "query": "label_values(service_requests, namespace)"},
    {"name": "p", "type": "custom", "query": "0.95,0.99",
     "current": {"text": "0.99", "value": "0.99"},
     "options": [{"text": "0.95", "value": "0.95"}, {"text": "0.99", "value": "0.99"}]}
  ]},
  "panels": [
    {"id": 82, "type": "timeseries", "title": "Visibility Task End-to-End Latencies",
     "targets": [{"refId": "A", "legendFormat": "{{task_type}}",
       "expr": "histogram_quantile($p, sum by (le, task_type) (rate(task_latency_queue_bucket{namespace=\"$namespace\", task_type=~\"VisibilityTask.*\"}[$__rate_interval])))"}],
     "fieldConfig": {"defaults": {"unit": "s", "custom": {"thresholdsStyle": {"mode": "dashed"}},
       "thresholds": {"mode": "absolute", "steps": [{"color": "green", "value": null}, {"color": "orange", "value": 3}, {"color": "red", "value": 5}]}}},
     "options": {"legend": {"displayMode": "table", "calcs": ["min", "max", "mean", "lastNotNull"]}}}
  ]
}"##;

/// A Prometheus that answers any range query with two visibility series over
/// the asked range, and remembers the queries it was asked.
async fn prometheus() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = asked.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 8192];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let target = head.lines().next().unwrap_or_default().to_owned();
                if let Ok(mut l) = log.lock() {
                    l.push(target.clone());
                }
                let param = |name: &str| -> f64 {
                    target
                        .split(['?', '&', ' '])
                        .find_map(|p| p.strip_prefix(&format!("{name}=")))
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0.0)
                };
                let (start, end) = (param("start"), param("end"));
                let values = |base: f64| {
                    let mut out = Vec::new();
                    let mut t = start;
                    let mut i = 0.0;
                    while t <= end && i < 50.0 {
                        out.push(format!("[{t},\"{}\"]", base + i * 0.001));
                        t += 60.0;
                        i += 1.0;
                    }
                    out.join(",")
                };
                let body = format!(
                    r#"{{"status":"success","data":{{"resultType":"matrix","result":[{{"metric":{{"task_type":"VisibilityTaskStartExecution"}},"values":[{}]}},{{"metric":{{"task_type":"VisibilityTaskCloseExecution"}},"values":[{}]}}]}}}}"#,
                    values(0.02),
                    values(0.01)
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (url, asked)
}

fn library() -> Library {
    let mut l = Library::default();
    l.add(
        kyumei_dashboards::parse(DASHBOARD).expect("dashboard"),
        Origin::Upload("temporal-server.json".to_owned()),
    );
    l
}

async fn replayed(kb: &Arc<KnowledgeBase>) -> (Arc<dyn InvestigationService>, InvestigationId) {
    let fx =
        Fixture::load(&fixture::fixtures_dir().join("visibility-es-slow-writes")).expect("fixture");
    let clock = Arc::new(FixtureClock::new(fx.open.at));
    let local = Arc::new(LocalInvestigations::new(kb.clone(), clock.clone()));
    // Only opened: the visibility lag request is still to answer.
    let (id, _) = fixture::open(kb, local.as_ref(), &fx, Some(&clock), false)
        .await
        .expect("open");
    (local, id)
}

fn router(
    kb: Arc<KnowledgeBase>,
    service: Arc<dyn InvestigationService>,
    collector: Option<Collector>,
) -> Router {
    kyumei_web::router_for(AppState {
        collector: collector.map(Arc::new),
        dashboards: Arc::new(RwLock::new(library())),
        ..AppState::new(kb, service, "test".to_owned())
    })
}

#[tokio::test]
async fn a_linked_panel_reads_live_and_becomes_evidence() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let (url, asked) = prometheus().await;
    let (service, id) = replayed(&kb).await;
    let collector =
        Collector::new(Endpoint::prometheus(&url).expect("endpoint")).expect("collector");
    let router = router(kb, service.clone(), Some(collector));

    // The workspace links the request's panel to its page in Kyumei.
    let workspace = send(&router, Method::GET, &format!("/investigations/{id}"), None).await;
    let open = format!("/investigations/{id}/panels/temporal-overview-v1/82");
    assert!(
        workspace.body.contains(&format!("href=\"{open}\"")),
        "the panel opens in Kyumei"
    );

    // On its own, the namespace is unknown, so nothing is read yet.
    let page = send(
        &router,
        Method::GET,
        "/panels/temporal-overview-v1/82",
        None,
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.content_type.starts_with("text/html"),
        "{}",
        page.content_type
    );
    assert!(
        page.body
            .contains("Fill in $namespace above to read the panel."),
        "{}",
        page.body
    );
    assert!(asked.lock().expect("log").is_empty());

    // For the investigation: its namespace, its incident window, read live.
    let page = send(&router, Method::GET, &open, None).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Visibility Task End-to-End Latencies"));
    assert!(
        page.body
            .contains("tsurdilo/temporal-server-operations at 5a71c36"),
        "cited"
    );
    assert!(
        page.body
            .contains("<svg xmlns=\"http://www.w3.org/2000/svg\" class=\"kc\"")
    );
    assert!(page.body.contains("VisibilityTaskStartExecution"));
    assert!(page.body.contains("from the investigation"));
    assert!(
        page.body
            .contains(&format!("Read from Prometheus at {url}"))
    );
    assert!(page.body.contains(">Add as evidence</button>"));
    assert!(
        page.body.contains("It answers the open request"),
        "panel 82 is linked to the open request"
    );
    let queries = asked.lock().expect("log").clone();
    assert_eq!(queries.len(), 1, "{queries:?}");
    assert!(queries[0].contains("namespace%3D%22"), "{}", queries[0]);
    assert!(
        queries[0].contains("histogram_quantile%280.99"),
        "the dashboard's default quantile"
    );

    // A different quantile and range chosen on the page.
    let page = send(
        &router,
        Method::GET,
        &format!("{open}?range=6h&p=0.95"),
        None,
    )
    .await;
    assert!(page.body.contains("<option value=\"0.95\" selected"));
    assert!(page.body.contains("<option value=\"6h\" selected"));

    // Added as evidence: the same sample block Collect now records, citing the panel.
    let added = send(
        &router,
        Method::POST,
        &format!("{open}/evidence"),
        Some(&[("range", "incident"), ("p", "0.99")]),
    )
    .await;
    assert_eq!(added.status, StatusCode::SEE_OTHER, "{}", added.body);
    assert!(added.location.contains("?received=E"), "{}", added.location);
    let inv = service.snapshot(&id).await.expect("snapshot").investigation;
    let e = inv.evidence.last().expect("evidence");
    assert_eq!(e.interpretation.form, EvidenceForm::SampleBlock);
    assert!(matches!(&e.origin, EvidenceOrigin::Collector { name, .. } if name == "kyumei"));
    assert!(e.responds_to.is_some(), "it answers the open request");
    assert!(
        e.declared.note.as_deref().is_some_and(
            |n| n.starts_with("Read as panel “Visibility Task End-to-End Latencies” (id 82)")
        ),
        "{:?}",
        e.declared.note
    );
}

#[tokio::test]
async fn a_panel_says_what_it_needs() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let (service, id) = replayed(&kb).await;
    let router = router(kb, service, None);

    // No metrics endpoint: the queries, and the command to read them elsewhere.
    let page = send(
        &router,
        Method::GET,
        &format!("/investigations/{id}/panels/temporal-overview-v1/82"),
        None,
    )
    .await;
    assert!(page.body.contains("Kyumei isn’t set up to read metrics."));
    assert!(page.body.contains("kyumei-collect"));
    assert!(
        page.body.contains("--query &#39;histogram_quantile(0.99,")
            || page.body.contains("--query 'histogram_quantile(0.99,")
    );

    // A referenced panel whose dashboard isn't loaded says which file to load.
    let page = send(
        &router,
        Method::GET,
        "/panels/temporal-sdk-core-v1/22",
        None,
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body
            .contains("Kyumei hasn’t loaded the dashboard this panel is in")
    );
    assert!(
        page.body
            .contains("observability/dashboards/sdk/temporal-sdk-core.json")
    );
    let missing = send(
        &router,
        Method::GET,
        "/panels/temporal-overview-v1/not-a-number",
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

/// With the operations repository's dashboards (KYUMEI_OPERATIONS_SOURCE) and a real
/// Prometheus (KYUMEI_PROMETHEUS_URL), every linked server panel draws over the last
/// day, with its legend, unit and thresholds.
#[tokio::test]
async fn linked_server_panels_draw_from_a_real_prometheus() {
    let (Some(root), Some(url)) = (
        std::env::var_os("KYUMEI_OPERATIONS_SOURCE"),
        std::env::var("KYUMEI_PROMETHEUS_URL").ok(),
    ) else {
        return;
    };
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let (library, problems) =
        Library::load(&std::path::Path::new(&root).join("observability/dashboards"));
    assert!(problems.is_empty(), "{problems:#?}");
    let service: Arc<dyn InvestigationService> = Arc::new(LocalInvestigations::new(
        kb.clone(),
        Arc::new(kyumei_app::SystemClock),
    ));
    let collector =
        Collector::new(Endpoint::prometheus(&url).expect("endpoint")).expect("collector");
    let router = kyumei_web::router_for(AppState {
        collector: Some(Arc::new(collector)),
        dashboards: Arc::new(RwLock::new(library)),
        ..AppState::new(kb.clone(), service, "test".to_owned())
    });
    let namespace =
        std::env::var("KYUMEI_PANELS_NAMESPACE").unwrap_or_else(|_| "orders".to_owned());
    let pack = &kb.packs()[0];
    let mut drawn = 0;
    let mut with_series = 0;
    for r in pack
        .references
        .iter()
        .filter(|r| r.kind == kyumei_knowledge::ReferenceKind::Panel)
    {
        let p = r.panel.as_ref().expect("panel");
        if p.sdk_family.is_some() {
            continue;
        }
        let path = format!(
            "/panels/{}/{}?range=24h&namespace={namespace}&taskqueue=fulfil-q",
            p.dashboard, p.id
        );
        let page = send(&router, Method::GET, &path, None).await;
        assert_eq!(page.status, StatusCode::OK, "{path}");
        assert!(
            page.body.contains("class=\"kc\""),
            "{path} did not draw: {}",
            page.body
                .split("<main>")
                .nth(1)
                .unwrap_or_default()
                .chars()
                .take(600)
                .collect::<String>()
        );
        drawn += 1;
        // The caption ends "…, every 1m. 2 series."
        let series: usize = page
            .body
            .split(" series.")
            .next()
            .and_then(|before| before.rsplit(|c: char| !c.is_ascii_digit()).next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        if series > 0 {
            with_series += 1;
        }
        eprintln!(
            "{:<32} {:>4}  {:>3} series  {}",
            p.dashboard, p.id, series, r.title
        );
    }
    eprintln!("{drawn} server panels drawn, {with_series} with series");
    assert!(drawn > 0);
}
