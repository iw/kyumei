//! The dashboards of tsurdilo/temporal-server-operations, read from a checkout at
//! the commit Kyumei's references pin (5a71c36). Kyumei does not bundle them, so this
//! runs only when one is given:
//!
//! ```bash
//! KYUMEI_OPERATIONS_SOURCE=/path/to/temporal-server-operations \
//!   cargo test -p kyumei-dashboards --test operations -- --nocapture
//! ```

use std::path::PathBuf;

use kyumei_dashboards::{LegendCalc, Library, ThresholdStyle};

fn library() -> Option<Library> {
    let root = PathBuf::from(std::env::var_os("KYUMEI_OPERATIONS_SOURCE")?);
    let (library, problems) = Library::load(&root.join("observability/dashboards"));
    assert!(problems.is_empty(), "{problems:#?}");
    Some(library)
}

#[test]
fn every_dashboard_reads() {
    let Some(library) = library() else {
        return;
    };
    assert_eq!(library.entries().len(), 10);
    for e in library.entries() {
        let charts = e.dashboard.panels.iter().filter(|p| p.is_chart()).count();
        eprintln!(
            "{:<70} {:>3} panels, {:>3} charts, {} unreadable",
            e.dashboard.title,
            e.dashboard.panels.len(),
            charts,
            e.dashboard.unreadable.len()
        );
        for u in &e.dashboard.unreadable {
            eprintln!("    unreadable {:?} {}: {}", u.id, u.title, u.reason);
        }
    }
    let server = library
        .get("temporal-overview-v1")
        .expect("server dashboard");
    let panel = server
        .dashboard
        .panels
        .iter()
        .find(|p| p.id == 82)
        .expect("panel 82");
    assert_eq!(panel.title, "Visibility Task End-to-End Latencies");
    assert_eq!(panel.row.as_deref(), Some("Visibility"));
    assert_eq!(panel.unit.as_deref(), Some("s"));
    assert_eq!(panel.threshold_style, ThresholdStyle::Dashed);
    assert_eq!(
        panel
            .thresholds
            .levels()
            .map(|(v, _)| v)
            .collect::<Vec<_>>(),
        vec![3.0, 5.0]
    );
    assert_eq!(
        panel.legend.calcs,
        vec![
            LegendCalc::Min,
            LegendCalc::Max,
            LegendCalc::Mean,
            LegendCalc::LastNotNull
        ]
    );
    let found = library.panels_reading(&["task_latency_queue"]);
    assert!(
        found.iter().any(|m| m.panel.id == 82),
        "{:?}",
        found.iter().map(|m| m.panel.id).collect::<Vec<_>>()
    );
}
