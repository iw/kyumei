//! Collect now: Kyumei reads a request's queries from Prometheus itself and
//! adds what comes back as a sample block, the block `kyumei-collect` would
//! print.

use std::sync::Arc;

use kyumei_app::{
    InvestigationService, LocalInvestigations,
    fixture::{self, FixtureClock},
};
use kyumei_collect::{Collector, Endpoint};
use kyumei_domain::{EpistemicStatus, EvidenceForm, EvidenceOrigin, InvestigationId};
use kyumei_knowledge::KnowledgeBase;
use kyumei_web::AppState;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use topcoat::router::{Method, Router, StatusCode};

mod common;
use common::send;

const EMPTY: &str = r#"{"status":"success","data":{"resultType":"matrix","result":[]}}"#;

/// A Prometheus that answers the Worker slots query with the golden
/// investigation's first piece of evidence, and any other with no series.
async fn prometheus(slots: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 4096];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&head);
                let line = head.lines().next().unwrap_or_default();
                let body = if line.contains("temporal_worker_task_slots_available") {
                    slots
                } else {
                    EMPTY
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    url
}

struct Setup {
    router: Router,
    service: Arc<dyn InvestigationService>,
}

fn setup(collector: Option<Collector>) -> Setup {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let clock = Arc::new(FixtureClock::new(fx.open.at));
    let service: Arc<dyn InvestigationService> =
        Arc::new(LocalInvestigations::new(kb.clone(), clock));
    let router = kyumei_web::router_for(AppState {
        collector: collector.map(Arc::new),
        ..AppState::new(kb, service.clone(), "test".to_owned())
    });
    Setup { router, service }
}

/// Open the golden investigation, with or without its namespace and Task Queue.
async fn open(router: &Router, scoped: bool) -> String {
    let fx = fixture::golden().expect("fixture");
    let mut fields = vec![
        ("description", fx.open.description.as_str()),
        ("server_version", "1.31.0"),
        ("sdk_language", "python"),
        ("sdk_version", "1.25.0"),
        ("window_start", "2026-09-30T13:35"),
        ("window_end", "2026-09-30T13:50"),
    ];
    if scoped {
        fields.extend([
            ("namespace", "commerce-prod"),
            ("task_queue", "order-fulfillment"),
        ]);
    }
    let created = send(router, Method::POST, "/investigations", Some(&fields)).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    created
        .location
        .trim_start_matches("/investigations/")
        .to_owned()
}

#[tokio::test]
async fn collect_now_reads_the_request_and_answers_it() {
    let fx = fixture::golden().expect("fixture");
    let slots: &'static str = Box::leak(
        std::fs::read_to_string(fx.dir.join("evidence/e1-worker-slots.json"))
            .expect("e1")
            .into_boxed_str(),
    );
    let url = prometheus(slots).await;
    let Setup { router, service } = setup(Some(
        Collector::new(Endpoint::prometheus(&url).expect("endpoint")).expect("collector"),
    ));
    let id = open(&router, true).await;

    let page = send(&router, Method::GET, &format!("/investigations/{id}"), None).await;
    assert!(page.body.contains(">Collect now</button>"), "{}", page.body);
    assert!(page.body.contains(&format!("from Prometheus at {url}")));
    assert!(page.body.contains("kyumei-collect --code kyq1."));
    assert!(
        !page.body.contains("--set namespace="),
        "the scope is known, so nothing is left to fill"
    );

    let inv = InvestigationId::new(&id);
    let snapshot = service.snapshot(&inv).await.expect("snapshot");
    let request = snapshot
        .investigation
        .current_request()
        .expect("request")
        .clone();
    assert_eq!(request.definition.as_str(), "workflow-worker-slots");
    let reply = send(
        &router,
        Method::POST,
        &format!("/investigations/{id}/requests/{}/collect", request.id),
        None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.body);
    assert!(
        reply.location.contains("?received=E1"),
        "{}",
        reply.location
    );

    let snapshot = service.snapshot(&inv).await.expect("snapshot");
    let e = snapshot.investigation.evidence(&"E1".into()).expect("E1");
    assert_eq!(
        e.origin,
        EvidenceOrigin::Collector {
            name: "kyumei".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    );
    assert_eq!(e.responds_to.as_ref(), Some(&request.id));
    assert_eq!(e.interpretation.form, EvidenceForm::SampleBlock);
    let bound: Vec<_> = e
        .interpretation
        .observations
        .iter()
        .filter(|o| o.binding.is_some())
        .collect();
    assert!(!bound.is_empty(), "{:#?}", e.interpretation);
    assert!(
        bound
            .iter()
            .all(|o| o.status == EpistemicStatus::ObservedFact)
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
            .any(|n| n.starts_with("Collected by kyumei ") && n.contains(" from prometheus at ")),
        "{notes:?}"
    );
    // The same evidence as pasting the query's output moves the investigation on.
    let next = snapshot
        .investigation
        .current_request()
        .expect("next request");
    assert_eq!(next.definition.as_str(), "workflow-pollers");

    // The page says what was read, what it changed and what comes next, and the
    // next-evidence box says which request was just answered.
    let page = send(&router, Method::GET, &reply.location, None).await;
    assert!(
        page.body.contains("E1 received, read by Collect now: "),
        "{}",
        page.body
    );
    assert!(page.body.contains(&format!(", answering {}.", request.id)));
    assert!(
        page.body
            .contains(&format!("Next: {} {}.", next.id, next.title))
    );
    assert!(page.body.contains("Open E1</a>"));
    assert!(
        page.body.contains(&format!(
            "<code>{}</code> {} — answered by",
            request.id, request.title
        )),
        "the answered line"
    );

    let raw = send(
        &router,
        Method::GET,
        &format!("/investigations/{id}/evidence/E1/raw"),
        None,
    )
    .await;
    assert_eq!(raw.status, StatusCode::OK);
    assert!(
        raw.content_type.starts_with("text/plain"),
        "{}",
        raw.content_type
    );
    assert!(
        raw.body.starts_with("-----BEGIN KYUMEI SAMPLES v1-----"),
        "{}",
        raw.body
    );
    let block = kyumei_samples::decode(&raw.body).expect("the raw evidence is the block");
    assert_eq!(block.request.as_deref(), Some(request.id.as_str()));
    assert_eq!(block.investigation.as_deref(), Some(id.as_str()));
}

#[tokio::test]
async fn collect_now_needs_an_endpoint_and_the_scope() {
    // Without an endpoint the page offers the companion command only.
    let Setup { router, service } = setup(None);
    let id = open(&router, false).await;
    let page = send(&router, Method::GET, &format!("/investigations/{id}"), None).await;
    assert!(!page.body.contains(">Collect now</button>"));
    assert!(page.body.contains("kyumei-collect --code kyq1."));
    assert!(page.body.contains("--set namespace= --set task_queue="));
    let request = service
        .snapshot(&InvestigationId::new(&id))
        .await
        .expect("snapshot")
        .investigation
        .current_request()
        .expect("request")
        .id
        .clone();
    let reply = send(
        &router,
        Method::POST,
        &format!("/investigations/{id}/requests/{request}/collect"),
        None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert!(
        reply.location.contains("error=Kyumei+is+not+set+up"),
        "{}",
        reply.location
    );

    // With one, it still needs the values the queries are scoped by.
    let Setup { router, service } = setup(Some(
        Collector::new(Endpoint::prometheus("http://127.0.0.1:9").expect("endpoint"))
            .expect("collector"),
    ));
    let id = open(&router, false).await;
    let page = send(&router, Method::GET, &format!("/investigations/{id}"), None).await;
    assert!(!page.body.contains(">Collect now</button>"));
    assert!(page.body.contains(
        "once Kyumei knows the namespace and the Task Queue: add them under Edit context"
    ));
    let request = service
        .snapshot(&InvestigationId::new(&id))
        .await
        .expect("snapshot")
        .investigation
        .current_request()
        .expect("request")
        .id
        .clone();
    let reply = send(
        &router,
        Method::POST,
        &format!("/investigations/{id}/requests/{request}/collect"),
        None,
    )
    .await;
    assert!(
        reply
            .location
            .contains("error=Collect+now+needs+the+namespace+and+the+Task+Queue+first"),
        "{}",
        reply.location
    );
}

#[tokio::test]
async fn an_unreachable_endpoint_is_reported_on_the_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    drop(listener);
    let Setup { router, service } = setup(Some(
        Collector::new(Endpoint::prometheus(&url).expect("endpoint")).expect("collector"),
    ));
    let id = open(&router, true).await;
    let inv = InvestigationId::new(&id);
    let before = service
        .snapshot(&inv)
        .await
        .expect("snapshot")
        .investigation;
    let request = before.current_request().expect("request").id.clone();
    let reply = send(
        &router,
        Method::POST,
        &format!("/investigations/{id}/requests/{request}/collect"),
        None,
    )
    .await;
    assert!(
        reply.location.contains("error=Collect+now+did+not+finish"),
        "{}",
        reply.location
    );
    assert!(reply.location.ends_with("#next"));
    let after = service
        .snapshot(&inv)
        .await
        .expect("snapshot")
        .investigation;
    assert_eq!(
        after.evidence.len(),
        before.evidence.len(),
        "nothing is added"
    );
}

#[tokio::test]
async fn collect_now_says_when_the_window_has_no_end() {
    let Setup { router, .. } = setup(Some(
        Collector::new(Endpoint::prometheus("http://127.0.0.1:9").expect("endpoint"))
            .expect("collector"),
    ));
    let fx = fixture::golden().expect("fixture");
    let fields = [
        ("description", fx.open.description.as_str()),
        ("server_version", "1.31.0"),
        ("namespace", "commerce-prod"),
        ("task_queue", "order-fulfillment"),
        ("window_start", "2026-09-30T13:35"),
        ("window_end", ""),
    ];
    let created = send(&router, Method::POST, "/investigations", Some(&fields)).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    let page = send(&router, Method::GET, &created.location, None).await;
    assert!(!page.body.contains(">Collect now</button>"));
    assert!(
        page.body
            .contains("Collect now can't read these queries yet: the incident window has no end."),
        "{}",
        page.body
    );
    assert!(page.body.contains("href=\"#edit-context\""));
}

#[tokio::test]
async fn a_failure_counter_is_shown_with_its_witness() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let clock = Arc::new(FixtureClock::new(fx.open.at));
    let service: Arc<dyn InvestigationService> =
        Arc::new(LocalInvestigations::new(kb.clone(), clock.clone()));
    let router = kyumei_web::router_for(AppState::new(
        kb.clone(),
        service.clone(),
        "test".to_owned(),
    ));
    let (id, _) = fixture::open(&kb, &*service, &fx, Some(clock.as_ref()), true)
        .await
        .expect("open");
    for step in 0..2 {
        fixture::step(&kb, &*service, &fx, &id, step, Some(clock.as_ref()), true)
            .await
            .expect("step");
    }
    let snapshot = service.snapshot(&id).await.expect("snapshot");
    let request = snapshot.investigation.current_request().expect("request");
    assert_eq!(request.definition.as_str(), "poll-failures");

    let page = send(&router, Method::GET, &format!("/investigations/{id}"), None).await;
    assert!(
        page.body
            .contains("With it, read the count of all the same requests."),
        "{}",
        page.body
    );
    assert!(page.body.contains("id=\"collect-0-witness\""));
    assert!(page.body.contains("temporal_long_request{operation="));
    assert!(
        page.body
            .contains("sdk.long-poll-failures-counted-on-failure")
    );
}
