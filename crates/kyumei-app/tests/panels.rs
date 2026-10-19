//! Every panel the knowledge links to opens in Kyumei with its queries filled in
//! (KYUMEI_OPERATIONS_SOURCE, a checkout of the operations repository at 5a71c36).

use std::{collections::BTreeMap, sync::Arc};

use kyumei_app::{
    InvestigationService, LocalInvestigations, OpenInvestigation,
    fixture::FixtureClock,
    panels::{self, PanelRange},
};
use kyumei_dashboards::Library;
use kyumei_domain::Investigation;
use kyumei_knowledge::{KnowledgeBase, ReferenceKind};

/// An investigation scoped to a namespace and a Task Queue, with a known window.
async fn scoped(kb: Arc<KnowledgeBase>) -> Investigation {
    let at = |s: &str| -> jiff::Timestamp { s.parse().expect("timestamp") };
    let clock = Arc::new(FixtureClock::new(at("2026-10-14T13:40:00Z")));
    let service = LocalInvestigations::new(kb, clock);
    let id = service
        .open(OpenInvestigation {
            id: "inv-panels".into(),
            description: "Lists and searches are stale".to_owned(),
            server_version: Some("1.31.0".to_owned()),
            namespaces: vec!["orders".to_owned()],
            task_queues: vec!["fulfil-q".to_owned()],
            window_start: Some(at("2026-10-14T13:03:00Z")),
            window_end: Some(at("2026-10-14T13:33:00Z")),
            ..Default::default()
        })
        .await
        .expect("open");
    service.snapshot(&id).await.expect("snapshot").investigation
}

#[tokio::test]
async fn every_linked_panel_opens_with_its_queries_filled_in() {
    let Some(root) = std::env::var_os("KYUMEI_OPERATIONS_SOURCE") else {
        return;
    };
    let (library, problems) =
        Library::load(&std::path::Path::new(&root).join("observability/dashboards"));
    assert!(problems.is_empty(), "{problems:#?}");
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let pack = &kb.packs()[0];
    let inv = scoped(kb.clone()).await;
    let now = "2026-10-14T14:00:00Z".parse().expect("ts");
    let mut opened = 0;
    for r in pack
        .references
        .iter()
        .filter(|r| r.kind == ReferenceKind::Panel)
    {
        let p = r.panel.as_ref().expect("panel");
        let (entry, panel) = panels::find(&library, &p.dashboard, p.id)
            .unwrap_or_else(|| panic!("{} panel {} is not in the dashboards", p.dashboard, p.id));
        for range in [None, Some(PanelRange::Hours(24))] {
            let plan = panels::plan(entry, panel, Some(&inv), range, &BTreeMap::new(), now);
            assert!(
                plan.unfilled.is_empty(),
                "{} panel {}: {:?}",
                p.dashboard,
                p.id,
                plan.unfilled
            );
            assert!(!plan.queries.is_empty(), "{} panel {}", p.dashboard, p.id);
            for q in &plan.queries {
                assert!(!q.contains('$'), "{} panel {}: {q}", p.dashboard, p.id);
            }
        }
        opened += 1;
    }
    assert_eq!(opened, 58);
}
