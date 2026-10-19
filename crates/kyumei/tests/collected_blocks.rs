//! The `visibility-collected-blocks` fixture is `visibility-es-slow-writes`
//! answered with kyumei-collect sample blocks. Its blocks are made here, as
//! `kyumei-collect encode FILE… --code CODE` makes them: the other fixture's
//! query output, under the code Kyumei shows for each request.
//!
//! The test fails when a block no longer holds exactly that: other samples,
//! or another request's queries. `KYUMEI_REGENERATE=1` writes them again.

use std::sync::Arc;

use jiff::SignedDuration;
use kyumei_app::{
    InvestigationService, LocalInvestigations,
    collect::CollectionPlan,
    fixture::{self, Fixture, FixtureClock},
};
use kyumei_collect::{COLLECTOR, parse_query_range, window_of};
use kyumei_knowledge::KnowledgeBase;
use kyumei_samples::{QueryResult, Samples, decode, encode};

/// Each step's block, and the query output it holds: one file for each of
/// the request's queries, in order, relative to the fixture.
const BLOCKS: [(&str, &[&str]); 4] = [
    (
        "evidence/e1-record-lag.block",
        &["../visibility-es-slow-writes/evidence/e1-record-lag.json"],
    ),
    (
        "evidence/e2-refusals.block",
        &["../visibility-es-slow-writes/evidence/e2-refusals.json"],
    ),
    (
        "evidence/e3-errors.block",
        &["sources/e3-errors.json", "sources/e3-dlq-writes.json"],
    ),
    (
        "evidence/e4-store-latency.block",
        &["../visibility-es-slow-writes/evidence/e4-store-latency.json"],
    ),
];

#[tokio::test]
async fn collected_blocks_hold_the_query_output_exactly() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let dir = fixture::fixtures_dir();
    let fx = Fixture::load(&dir.join("visibility-collected-blocks")).expect("fixture");
    // The original, opened as the blocks fixture's investigation, is asked
    // the same requests with the same codes.
    let mut original = Fixture::load(&dir.join("visibility-es-slow-writes")).expect("original");
    original.fixture.investigation = fx.fixture.investigation.clone();
    assert_eq!(original.steps.len(), BLOCKS.len());
    let clock = Arc::new(FixtureClock::new(original.open.at));
    let service = LocalInvestigations::new(kb.clone(), clock.clone());
    let (id, _) = fixture::open(&kb, &service, &original, Some(&clock), true)
        .await
        .expect("open");

    let regenerate = std::env::var_os("KYUMEI_REGENERATE").is_some();
    let mut stale = Vec::new();
    for (index, (block, sources)) in BLOCKS.iter().enumerate() {
        let step = &fx.steps[index];
        assert_eq!(step.file.as_deref(), Some(*block));
        let snapshot = service.snapshot(&id).await.expect("snapshot");
        let request = snapshot
            .investigation
            .current_request()
            .expect("an open request");
        assert_eq!(request.definition.as_str(), step.answers);
        let code = CollectionPlan::for_request(request)
            .expect("PromQL over a window")
            .code(&id, &request.id);
        assert_eq!(
            code.queries.len(),
            sources.len(),
            "{block}: one file for each of {:?}",
            code.queries
        );
        let queries: Vec<QueryResult> = code
            .queries
            .iter()
            .zip(sources.iter())
            .map(|(query, source)| {
                let body = std::fs::read(fx.dir.join(source)).expect("source");
                QueryResult {
                    query: query.clone(),
                    series: parse_query_range(&body).expect("query output").series,
                }
            })
            .collect();
        let expected = Samples {
            collector: COLLECTOR.to_owned(),
            source: "file".to_owned(),
            collected_at_ms: (step.at - SignedDuration::from_mins(1)).as_millisecond(),
            investigation: Some(code.investigation.clone()),
            request: Some(code.request.clone()),
            window: window_of(&queries).expect("samples"),
            rounded_to: None,
            redacted: Vec::new(),
            queries,
        };
        let path = fx.dir.join(block);
        if regenerate {
            std::fs::write(&path, encode(&expected)).expect("write block");
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if decode(&text).ok().as_ref() != Some(&expected) {
            stale.push(*block);
        }
        fixture::step(&kb, &service, &original, &id, index, Some(&clock), true)
            .await
            .expect("step");
    }
    assert!(
        stale.is_empty(),
        "out of date: {stale:?}. Run `KYUMEI_REGENERATE=1 cargo test -p kyumei --test collected_blocks` to write them again."
    );
}
