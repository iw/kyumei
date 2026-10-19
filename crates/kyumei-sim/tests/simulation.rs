//! A small simulated cluster, scraped and queried as Kyumei queries Prometheus.

use kyumei_knowledge::KnowledgeBase;
use kyumei_samples::Window;
use kyumei_sim::{
    Simulation, TEMPDES_COMMIT,
    promql::{self, Store},
};

/// One frontend, history and matching pod, and an order workflow started 40
/// times a second, with a short warm-up and two measured minutes.
const SMALL: &str = r#"
name: kyumei-sim-small
seed: 7
warmup: 10s
duration: 120s
cluster:
  num_history_shards: 64
  replicas: { frontend: 1, history: 1, matching: 1, worker: 1 }
  resources:
    frontend: { cpu: 2 }
    history: { cpu: 2 }
    matching: { cpu: 2 }
    worker: { cpu: 1 }
  network: { client_rtt: 2ms, internal_rtt: 0.5ms }
  persistence:
    store: postgresql
    max_conns: { frontend: 20, history: 50, matching: 30, worker: 10 }
    capacity: 80
    latency:
      UpdateWorkflowExecution: { p50: 3ms, p99: 14ms }
      CreateWorkflowExecution: { p50: 4ms, p99: 18ms }
  visibility: { store: elasticsearch }
namespaces:
  - name: orders
workers:
  - name: order-workers
    namespace: orders
    task_queue: orders
    processes: 2
    workflow_pollers: 4
    activity_pollers: 4
    workflow_slots: 100
    activity_slots: 100
    sticky_cache_size: 1000
workflows:
  - type: OrderWorkflow
    namespace: orders
    task_queue: orders
    start_rate: 40/s
    starters: 2
    wft_processing: { p50: 2ms, p99: 12ms }
    steps:
      - activity: { count: 1, duration: { p50: 30ms, p99: 250ms } }
"#;

const STARTS: &str = r#"sum(rate(service_requests{service_name="frontend", operation="StartWorkflowExecution"}[1m]))"#;

#[test]
fn a_simulated_cluster_answers_queries_as_prometheus_would() {
    let run = Simulation::new(SMALL).run().expect("run");
    assert_eq!(run.scenario, "kyumei-sim-small");
    assert_eq!(run.seed, 7);
    // 130 simulated seconds, scraped every 15.
    let times: Vec<i64> = run.scrapes.iter().map(|s| s.at_ms).collect();
    assert_eq!(times.first(), Some(&15_000));
    assert_eq!(times.last(), Some(&120_000));
    assert_eq!(times.len(), 8);

    // After the warm-up, workflows start at about the scenario's rate.
    let store = Store::new(&run.scrapes);
    let answer = promql::evaluate_range(
        &promql::parse(STARTS).expect("parse"),
        &store,
        90_000,
        120_000,
        15_000,
    )
    .expect("evaluate");
    assert_eq!(answer.len(), 1, "one series, summed over pods");
    for (at, rate) in &answer[0].points {
        assert!((32.0..=48.0).contains(rate), "{rate}/s at {at} ms");
    }
}

#[test]
fn the_same_seed_gives_the_same_scrapes() {
    let a = Simulation::new(SMALL).run().expect("run");
    let b = Simulation::new(SMALL).run().expect("run");
    assert_eq!(a, b);
    let c = Simulation::new(SMALL).seed(8).run().expect("run");
    assert_ne!(a.scrapes, c.scrapes, "another seed, another run");
}

#[test]
fn answers_travel_as_a_sample_block() {
    let run = Simulation::new(SMALL)
        .scrape_every_ms(10_000)
        .run()
        .expect("run");
    // Simulated time zero stands for 13:00 UTC on 15 October 2026.
    let origin = 1_792_069_200_000;
    let window = Window {
        start_ms: origin + 60_000,
        end_ms: origin + 120_000,
        step_ms: 30_000,
    };
    let samples = run
        .collect(&[STARTS.to_owned()], window, origin, origin + 130_000)
        .expect("collect");
    assert_eq!(samples.source, "a simulation of kyumei-sim-small (seed 7)");
    assert_eq!(
        samples.collector,
        format!("tempdes {}", &TEMPDES_COMMIT[..7])
    );
    let points = &samples.queries[0].series[0].points;
    let times: Vec<i64> = points.iter().map(|p| p.at_ms).collect();
    assert_eq!(
        times,
        vec![origin + 60_000, origin + 90_000, origin + 120_000]
    );
    let block = kyumei_samples::encode(&samples);
    assert_eq!(kyumei_samples::decode(&block).expect("decode"), samples);
}

#[test]
fn every_promql_query_the_knowledge_asks_is_understood() {
    let kb = KnowledgeBase::builtin().expect("knowledge");
    // `{namespace}`, `{task_queue}`: the context Kyumei fills in.
    let placeholder = regex::Regex::new(r"\{[a-z_]+\}").expect("regex");
    let mut read = 0;
    for pack in kb.packs() {
        for d in &pack.diagnostics {
            for r in &d.requests {
                for c in r.collect.iter().filter(|c| c.method.label() == "PromQL") {
                    let query = placeholder.replace_all(&c.body, "x");
                    promql::parse(&query).unwrap_or_else(|e| panic!("{} {}: {e}", d.id, r.id));
                    read += 1;
                }
            }
        }
    }
    assert!(read >= 73, "{read} queries");
}

/// The Workflow Task diagnostic's queries, as a Go SDK Worker's metrics answer them, for the
/// request they belong to.
fn workflow_task_queries(kb: &KnowledgeBase) -> Vec<(String, String)> {
    let d = kb.packs()[0]
        .diagnostics
        .iter()
        .find(|d| d.id.as_str() == "wft-schedule-to-start")
        .expect("the diagnostic");
    d.requests
        .iter()
        .flat_map(|r| {
            r.collect
                .iter()
                .filter(|c| c.method.label() == "PromQL")
                .filter(|c| {
                    c.sdk_family.is_empty() || c.sdk_family.iter().any(|f| f.token() == "go")
                })
                .map(move |c| {
                    let query = c
                        .body
                        .replace("{namespace}", "orders")
                        .replace("{task_queue}", "orders");
                    (r.id.to_string(), query)
                })
        })
        .collect()
}

#[test]
fn the_workflow_task_requests_read_a_simulated_cluster() {
    let kb = KnowledgeBase::builtin().expect("knowledge");
    let run = Simulation::new(SMALL).run().expect("run");
    let store = Store::new(&run.scrapes);
    // Nothing fails, is rejected or is signalled in the small cluster.
    let quiet = ["poll-failures", "poll-rejections", "signal-rate"];
    let queries = workflow_task_queries(&kb);
    assert!(queries.len() >= 8, "{queries:?}");
    for (request, query) in queries {
        let answer = promql::evaluate_range(
            &promql::parse(&query).expect("parse"),
            &store,
            60_000,
            120_000,
            15_000,
        )
        .expect("evaluate");
        if quiet.contains(&request.as_str()) {
            assert!(answer.is_empty(), "{request}: {query}");
        } else {
            assert!(!answer.is_empty(), "{request} reads nothing: {query}");
            assert!(
                answer.iter().all(|s| s.points.len() == 5),
                "{request}: {answer:?}"
            );
        }
    }
}

#[test]
fn the_pinned_commit_is_the_one_built() {
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
        .expect("Cargo.lock");
    let source = lock
        .split("[[package]]")
        .find(|p| p.contains("name = \"tempdes\""))
        .and_then(|p| p.lines().find(|l| l.starts_with("source = ")))
        .expect("tempdes in Cargo.lock");
    assert!(
        source.ends_with(&format!("#{TEMPDES_COMMIT}\"")),
        "Cargo.lock has {source}; TEMPDES_COMMIT is {TEMPDES_COMMIT}"
    );
}
