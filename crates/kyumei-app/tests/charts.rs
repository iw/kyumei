//! Evidence drawn as the dashboard panel that reads the same metric draws it.

use std::sync::Arc;

use kyumei_app::{
    InvestigationService, LocalInvestigations,
    dashboards::{GrafanaLinks, evidence_charts},
    fixture::{self, Fixture, FixtureClock},
};
use kyumei_dashboards::{Library, Origin};
use kyumei_domain::DisplayZone;
use kyumei_knowledge::KnowledgeBase;

/// A dashboard written for this test, in the shape Grafana saves: a visibility lag
/// panel in milliseconds, grouped by `operation` as the operations dashboards do.
const DASHBOARD: &str = r##"{
  "uid": "test-visibility", "title": "Test Visibility v0.1", "version": 2, "schemaVersion": 38,
  "templating": {"list": [{"name": "p", "type": "custom", "query": "0.95,0.99"}]},
  "panels": [
    {"id": 1, "type": "row", "title": "Visibility", "panels": []},
    {"id": 9, "type": "timeseries", "title": "Visibility lag",
     "targets": [{"refId": "A", "legendFormat": "{{operation}}",
       "expr": "histogram_quantile($p, sum(rate(task_latency_queue_bucket{operation=~\"VisibilityTask.*\",namespace=\"$namespace\"}[$__rate_interval])) by (operation, le))"}],
     "fieldConfig": {"defaults": {"unit": "ms", "custom": {"thresholdsStyle": {"mode": "dashed"}},
       "thresholds": {"mode": "absolute", "steps": [{"color": "green", "value": null}, {"color": "orange", "value": 60000}, {"color": "red", "value": 900000}]}}},
     "options": {"legend": {"displayMode": "table", "calcs": ["min", "max", "mean", "lastNotNull"]}}},
    {"id": 10, "type": "timeseries", "title": "Something else",
     "targets": [{"refId": "A", "expr": "sum(rate(service_requests[1m]))"}]}
  ]
}"##;

async fn replayed(name: &str) -> (Arc<KnowledgeBase>, kyumei_domain::Investigation) {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = Fixture::load(&fixture::fixtures_dir().join(name)).expect("fixture");
    let clock = Arc::new(FixtureClock::new(fx.open.at));
    let svc = LocalInvestigations::new(kb.clone(), clock.clone());
    let (id, _) = fixture::replay(&kb, &svc, &fx, Some(&clock), true)
        .await
        .expect("replay");
    let inv = svc.snapshot(&id).await.expect("snapshot").investigation;
    (kb, inv)
}

#[tokio::test]
async fn visibility_lag_is_drawn_as_its_panel() {
    let utc = DisplayZone::utc();
    let (kb, inv) = replayed("visibility-es-slow-writes").await;
    let mut library = Library::default();
    library.add(
        kyumei_dashboards::parse(DASHBOARD).expect("dashboard"),
        Origin::Upload("test-visibility.json".to_owned()),
    );
    let e1 = inv.evidence(&"E1".into()).expect("E1");
    let links = GrafanaLinks::for_investigation(Some("https://grafana.example.com/"), &inv);
    let charts = evidence_charts(&library, &kb, &inv, e1, &links, &utc);
    assert_eq!(charts.len(), 1);
    let chart = &charts[0];
    assert_eq!(chart.title, "Visibility lag");
    assert_eq!(chart.observations.len(), 3);
    assert_eq!(
        (chart.panel.panel_id, chart.panel.dashboard_uid.as_str()),
        (9, "test-visibility")
    );
    assert!(
        chart.panel.reference.is_none(),
        "not a dashboard of the operations repository"
    );

    // The legend names series by `operation`, filled from `task_type`; values are in
    // the panel's unit, milliseconds, written as durations.
    let text = &chart.text;
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "Visibility lag");
    assert!(
        lines[1].contains("Min") && lines[1].ends_with("Last *"),
        "{text}"
    );
    assert!(text.contains("  VisibilityTaskStartExecution"), "{text}");
    assert!(
        text.contains("min"),
        "a lag of minutes is written in minutes: {text}"
    );
    assert!(
        text.contains("Thresholds: 1 min (orange), 15 min (red)"),
        "{text}"
    );
    assert!(text.contains("Drawn as panel “Visibility lag” (id 9) of “Test Visibility v0.1”, version 2, uid test-visibility, loaded from uploaded as test-visibility.json."), "{text}");
    assert!(
        text.contains("by `operation`; these carry the same value as `task_type`"),
        "{text}"
    );
    assert!(
        chart.svg.contains("stroke-dasharray"),
        "the thresholds are dashed"
    );
    let link = chart.panel.grafana_url.as_deref().expect("grafana link");
    assert!(
        link.starts_with("https://grafana.example.com/d/test-visibility?viewPanel=9&from="),
        "{link}"
    );
    assert!(link.ends_with("&var-namespace=orders"), "{link}");

    // Evidence no loaded panel reads is not drawn.
    let e2 = inv.evidence(&"E2".into()).expect("E2");
    assert!(evidence_charts(&library, &kb, &inv, e2, &links, &utc).is_empty());
    assert!(evidence_charts(&Library::default(), &kb, &inv, e1, &links, &utc).is_empty());
}

/// With the operations repository's dashboards (KYUMEI_OPERATIONS_SOURCE, a checkout
/// at 5a71c36), fixture evidence draws as their panels: legend, unit and thresholds.
#[tokio::test]
async fn fixture_evidence_draws_as_the_operations_panels() {
    let Some(root) = std::env::var_os("KYUMEI_OPERATIONS_SOURCE") else {
        return;
    };
    let (library, problems) =
        Library::load(&std::path::Path::new(&root).join("observability/dashboards"));
    assert!(problems.is_empty(), "{problems:#?}");
    let links = GrafanaLinks::default();
    let utc = DisplayZone::utc();
    let mut drawn = 0;
    let mut total = 0;
    for fx in fixture::all().expect("fixtures") {
        let (kb, inv) = replayed(&fx.fixture.id).await;
        for e in &inv.evidence {
            let charts = evidence_charts(&library, &kb, &inv, e, &links, &utc);
            total += 1;
            if charts.is_empty() {
                continue;
            }
            drawn += 1;
            for c in &charts {
                eprintln!(
                    "{:<34} {:<4} {:<52} panel {:>4} of {}",
                    fx.fixture.id, e.id, c.title, c.panel.panel_id, c.panel.dashboard_title
                );
            }
        }
    }
    eprintln!("{drawn} of {total} evidence drawn as a panel");

    let (kb, inv) = replayed("visibility-es-slow-writes").await;
    let e1 = inv.evidence(&"E1".into()).expect("E1");
    let charts = evidence_charts(&library, &kb, &inv, e1, &links, &utc);
    let chart = &charts[0];
    assert_eq!(chart.panel.dashboard_uid, "temporal-overview-v1");
    assert_eq!(chart.panel.panel_id, 82);
    assert_eq!(chart.title, "Visibility Task End-to-End Latencies");
    assert!(
        chart.text.contains("Thresholds: 3 s (orange), 5 s (red)"),
        "{}",
        chart.text
    );
    assert!(
        chart.text.contains("  VisibilityTaskStartExecution"),
        "{}",
        chart.text
    );
    assert!(
        chart
            .text
            .lines()
            .nth(1)
            .is_some_and(|l| l.ends_with("Last *")),
        "{}",
        chart.text
    );
    let reference = chart
        .panel
        .reference
        .as_ref()
        .expect("cites the repository");
    assert_eq!(
        reference.cited,
        "tsurdilo/temporal-server-operations at 5a71c36, 9 October 2026"
    );
    assert!(drawn > 40, "{drawn} of {total}");
}
