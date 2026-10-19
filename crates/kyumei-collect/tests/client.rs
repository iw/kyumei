//! The collector against a stand-in Prometheus: what it sends, and how it
//! reads what comes back.

use std::sync::{Arc, Mutex};

use kyumei_collect::{CollectError, Collector, ENV_TOKEN, Endpoint};
use kyumei_samples::{Window, decode, encode};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const MATRIX: &str = r#"{"status":"success","data":{"resultType":"matrix","result":[
    {"metric":{"__name__":"temporal_worker_task_slots_available","namespace":"orders","worker_type":"WorkflowWorker"},
     "values":[[1791105120,"0"],[1791105135,"0"],[1791105150,"NaN"],[1791105165,"2"]]}]}}"#;

const EMPTY: &str = r#"{"status":"success","warnings":["the query hit 1 of 3 stores"],"data":{"resultType":"matrix","result":[]}}"#;

const BAD_QUERY: &str =
    r#"{"status":"error","errorType":"bad_data","error":"1:14: parse error: unexpected \"}\""}"#;

/// A server that answers each request as `respond` says for its request line,
/// and keeps each request's head.
async fn stand_in(respond: fn(&str) -> (u16, &'static str)) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 4096];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let (status, body) = respond(head.lines().next().unwrap_or_default());
                log.lock().expect("log").push(head);
                let response = format!(
                    "HTTP/1.1 {status} Stand-in\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), seen)
}

fn window() -> Window {
    Window {
        start_ms: 1_791_105_120_000,
        end_ms: 1_791_105_165_000,
        step_ms: 15_000,
    }
}

fn token(value: &'static str) -> impl Fn(&str) -> Option<String> {
    move |k| (k == ENV_TOKEN).then(|| value.to_owned())
}

#[tokio::test]
async fn a_range_query_carries_the_window_credentials_and_tenant() {
    let (url, seen) = stand_in(|_| (200, MATRIX)).await;
    let endpoint = Endpoint::prometheus(&url)
        .expect("endpoint")
        .with_credentials(token("t0ken"), None, None, None)
        .expect("credentials")
        .with_tenant(Some("team-a".to_owned()));
    let collector = Collector::new(endpoint).expect("collector");
    let out = collector
        .query_range(r#"sum(x{namespace="orders"})"#, window())
        .await
        .expect("query");

    assert_eq!(out.series.len(), 1);
    let points = &out.series[0].points;
    assert_eq!(points.len(), 4);
    assert_eq!(points[0].at_ms, 1_791_105_120_000);
    assert!(points[2].value.is_nan());
    assert_eq!(points[3].value, 2.0);

    let head = seen.lock().expect("seen")[0].to_ascii_lowercase();
    let line = head.lines().next().unwrap_or_default().to_owned();
    assert!(line.starts_with("get /api/v1/query_range?"), "{line}");
    assert!(
        line.contains("query=sum%28x%7bnamespace%3d%22orders%22%7d%29"),
        "{line}"
    );
    assert!(
        line.contains("&start=1791105120&end=1791105165&step=15 "),
        "{line}"
    );
    assert!(
        head.contains("\r\nauthorization: bearer t0ken\r\n"),
        "{head}"
    );
    assert!(head.contains("\r\nx-scope-orgid: team-a\r\n"), "{head}");
}

#[tokio::test]
async fn grafana_is_read_through_its_data_source_proxy() {
    let (url, seen) = stand_in(|_| (200, MATRIX)).await;
    let endpoint = Endpoint::grafana(&url, "prom-uid").expect("endpoint");
    let collector = Collector::new(endpoint).expect("collector");
    collector.query_range("up", window()).await.expect("query");
    let head = seen.lock().expect("seen")[0].clone();
    assert!(
        head.starts_with("GET /api/datasources/proxy/uid/prom-uid/api/v1/query_range?"),
        "{head}"
    );
}

#[tokio::test]
async fn every_query_of_a_request_is_read_into_one_block() {
    let (url, _) = stand_in(|line| {
        if line.contains("query=up") {
            (200, EMPTY)
        } else {
            (200, MATRIX)
        }
    })
    .await;
    let collector =
        Collector::new(Endpoint::prometheus(&url).expect("endpoint")).expect("collector");
    let queries = vec![
        "temporal_worker_task_slots_available".to_owned(),
        "up".to_owned(),
    ];
    let (samples, warnings) = collector
        .collect(
            &queries,
            window(),
            "kyumei-collect 0.1.0",
            1_791_105_200_000,
        )
        .await
        .expect("collect");
    assert_eq!(warnings, vec!["the query hit 1 of 3 stores"]);
    assert_eq!(samples.source, "prometheus");
    assert_eq!(samples.queries.len(), 2);
    assert_eq!(samples.queries[0].series.len(), 1);
    assert!(samples.queries[1].series.is_empty());
    assert_eq!(decode(&encode(&samples)).expect("decode"), samples);
}

#[tokio::test]
async fn refused_credentials_say_so_without_repeating_them() {
    let (url, _) = stand_in(|_| (401, r#"{"message":"invalid API key"}"#)).await;
    let endpoint = Endpoint::prometheus(&url)
        .expect("endpoint")
        .with_credentials(token("t0ken"), None, None, None)
        .expect("credentials");
    let e = Collector::new(endpoint)
        .expect("collector")
        .query_range("up", window())
        .await
        .expect_err("unauthorised");
    assert!(
        matches!(e, CollectError::Unauthorised { status: 401, .. }),
        "{e:?}"
    );
    assert!(!e.to_string().contains("t0ken"), "{e}");
}

#[tokio::test]
async fn a_bad_query_gives_prometheus_reason() {
    let (url, _) = stand_in(|_| (400, BAD_QUERY)).await;
    let e = Collector::new(Endpoint::prometheus(&url).expect("endpoint"))
        .expect("collector")
        .query_range("sum(x{)", window())
        .await
        .expect_err("bad query");
    assert_eq!(
        e.to_string(),
        r#"the query was refused: 1:14: parse error: unexpected "}""#
    );
}

#[tokio::test]
async fn an_answer_that_is_not_prometheus_output_gives_its_status() {
    let (url, _) = stand_in(|_| (502, "<html>Bad Gateway</html>")).await;
    let e = Collector::new(Endpoint::prometheus(&url).expect("endpoint"))
        .expect("collector")
        .query_range("up", window())
        .await
        .expect_err("bad gateway");
    assert!(
        matches!(&e, CollectError::Http { status: 502, detail, .. } if detail.contains("Bad Gateway")),
        "{e:?}"
    );
}

#[tokio::test]
async fn an_unreachable_endpoint_is_named_without_the_query() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    drop(listener);
    let e = Collector::new(Endpoint::prometheus(&url).expect("endpoint"))
        .expect("collector")
        .query_range(r#"x{namespace="secret-namespace"}"#, window())
        .await
        .expect_err("unreachable");
    assert!(matches!(e, CollectError::Unreachable { .. }), "{e:?}");
    let text = e.to_string();
    assert!(text.contains(&url), "{text}");
    assert!(!text.contains("secret-namespace"), "{text}");
}
