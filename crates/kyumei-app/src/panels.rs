//! Dashboard panels opened in Kyumei with live data.
//!
//! A panel's queries are filled in for an investigation, or for a time range on
//! their own, and what the interface reads for them is drawn as the panel draws
//! it. Reading is the interface's job, since only it reaches the network; this
//! module plans and draws.

use std::{collections::BTreeMap, sync::LazyLock};

use jiff::{SignedDuration, Timestamp};
use kyumei_dashboards::{Entry, Panel, legend, units};
use kyumei_diagnostics::select::range_step;
use kyumei_diagrams::ChartSeries;
use kyumei_domain::{DisplayZone, Investigation, TimeWindow};
use kyumei_knowledge::{Pack, ReferenceDef, ReferenceKind};
use kyumei_samples::{Samples, Window};
use regex::Regex;
use serde::Serialize;

use crate::dashboards::{ChartView, Drawing, GrafanaLinks, PALETTE, assemble};

/// `$name`, `${name}`, `${name:format}` and `[[name]]`.
static VARIABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{(\w+)(?::[^}]*)?\}|\[\[(\w+)(?::[^\]]*)?\]\]|\$(\w+)")
        .unwrap_or_else(|e| panic!("{e}"))
});

/// How far before and after an incident window a panel is read, as requests do.
const BEFORE: SignedDuration = SignedDuration::from_mins(10);
const AFTER: SignedDuration = SignedDuration::from_mins(5);

/// What Grafana assumes a scrape interval is, for `$__rate_interval`.
const SCRAPE: SignedDuration = SignedDuration::from_secs(15);

/// The time a panel is read over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum PanelRange {
    /// The investigation's incident window, with the margins requests use.
    Incident,
    /// The hours up to now.
    Hours(i64),
}

impl PanelRange {
    /// The ranges offered, as `(token, label)`.
    pub const CHOICES: [(&'static str, &'static str); 4] = [
        ("incident", "Incident window"),
        ("1h", "Last hour"),
        ("6h", "Last 6 hours"),
        ("24h", "Last 24 hours"),
    ];

    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "incident" => Some(Self::Incident),
            "1h" => Some(Self::Hours(1)),
            "6h" => Some(Self::Hours(6)),
            "24h" => Some(Self::Hours(24)),
            _ => None,
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Incident => "incident",
            Self::Hours(6) => "6h",
            Self::Hours(24) => "24h",
            Self::Hours(_) => "1h",
        }
    }
}

/// A template variable the panel's queries use, as the panel page offers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct VariableView {
    pub name: String,
    pub value: Option<String>,
    /// Where the value came from: `chosen`, `investigation` or `dashboard`.
    pub source: &'static str,
    /// A custom variable's choices, such as the quantiles of `$p`.
    pub options: Vec<String>,
}

/// A panel's queries filled in and ready to read.
#[derive(Clone, Debug, Serialize)]
pub struct PanelPlan {
    pub dashboard_uid: String,
    pub panel_id: i64,
    pub range: PanelRange,
    pub window: TimeWindow,
    pub step: SignedDuration,
    /// The panel's visible queries, filled in, in its order.
    pub queries: Vec<String>,
    /// How each query names its series.
    pub legend_formats: Vec<Option<String>>,
    pub variables: Vec<VariableView>,
    /// Variables nothing fills; the queries can't run until they are given.
    pub unfilled: Vec<String>,
}

impl PanelPlan {
    /// The range in the form collectors take.
    pub fn sample_window(&self) -> Window {
        Window {
            start_ms: self.window.start.as_millisecond(),
            end_ms: self.window.end.as_millisecond(),
            step_ms: u64::try_from(self.step.as_millis()).unwrap_or(15_000),
        }
    }

    /// `15s`, `1m`, `5m`.
    pub fn step_text(&self) -> String {
        duration_text(self.step)
    }
}

/// Plan reading `panel` of `entry`: in `investigation` when there is one, over
/// `range` (by default the incident window when it is known, else the last
/// hour), with the variable values `chosen` on the page taking precedence.
pub fn plan(
    entry: &Entry,
    panel: &Panel,
    investigation: Option<&Investigation>,
    range: Option<PanelRange>,
    chosen: &BTreeMap<String, String>,
    now: Timestamp,
) -> PanelPlan {
    let incident = investigation.and_then(|i| i.window.as_window());
    let range = match (range, incident) {
        (Some(PanelRange::Incident) | None, Some(_)) => PanelRange::Incident,
        (Some(PanelRange::Incident) | None, None) => PanelRange::Hours(1),
        (Some(r), _) => r,
    };
    let window = match (range, incident) {
        (PanelRange::Incident, Some(w)) => {
            let padded = w.padded(BEFORE, AFTER);
            // Never ask for the future.
            TimeWindow {
                start: padded.start.min(now),
                end: padded.end.min(now),
            }
        }
        (PanelRange::Hours(_), _) | (PanelRange::Incident, None) => {
            let h = if let PanelRange::Hours(h) = range {
                h
            } else {
                1
            };
            TimeWindow {
                start: now
                    .checked_sub(SignedDuration::from_hours(h))
                    .unwrap_or(now),
                end: now,
            }
        }
    };
    let step = range_step(window.duration());
    // Grafana's: at least four scrapes, and a step plus one scrape.
    let rate_interval = (step + SCRAPE).max(SCRAPE * 4);

    let targets: Vec<_> = panel
        .targets
        .iter()
        .filter(|t| !t.hidden && !t.expr.trim().is_empty())
        .collect();
    let mut names: Vec<String> = Vec::new();
    for t in &targets {
        for c in VARIABLE.captures_iter(&t.expr) {
            let name = c
                .get(1)
                .or_else(|| c.get(2))
                .or_else(|| c.get(3))
                .map_or("", |m| m.as_str());
            if !name.is_empty() && !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
    }

    let mut values: BTreeMap<String, String> = BTreeMap::new();
    values.insert("__rate_interval".to_owned(), duration_text(rate_interval));
    values.insert("__interval".to_owned(), duration_text(step));
    values.insert("__range".to_owned(), duration_text(window.duration()));
    let mut variables = Vec::new();
    for name in names.iter().filter(|n| !n.starts_with("__")) {
        let definition = entry.dashboard.variables.iter().find(|v| &v.name == name);
        let options = definition.map(|d| d.options.clone()).unwrap_or_default();
        let from_investigation = investigation.and_then(|i| match name.as_str() {
            "namespace" => i.scope.primary_namespace(),
            "taskqueue" | "task_queue" => i.scope.primary_task_queue(),
            _ => None,
        });
        let from_dashboard = definition.and_then(|d| {
            d.current
                .clone()
                .filter(|c| !c.is_empty() && !c.starts_with('$') && c != "All")
                .or_else(|| d.options.first().cloned())
        });
        let (value, source) = match chosen.get(name).map(|v| v.trim()).filter(|v| !v.is_empty()) {
            Some(v) => (Some(v.to_owned()), "chosen"),
            None => match from_investigation {
                Some(v) => (Some(v.to_owned()), "investigation"),
                None => (from_dashboard, "dashboard"),
            },
        };
        if let Some(v) = &value {
            values.insert(name.clone(), v.clone());
        }
        variables.push(VariableView {
            name: name.clone(),
            value,
            source,
            options,
        });
    }
    let unfilled = variables
        .iter()
        .filter(|v| v.value.is_none())
        .map(|v| v.name.clone())
        .collect();
    PanelPlan {
        dashboard_uid: entry.dashboard.uid.clone(),
        panel_id: panel.id,
        range,
        window,
        step,
        queries: targets
            .iter()
            .map(|t| legend::fill_variables(&t.expr, &values))
            .collect(),
        legend_formats: targets.iter().map(|t| t.legend_format.clone()).collect(),
        variables,
        unfilled,
    }
}

/// What was read for `plan`, drawn as `panel` of `entry` draws it.
#[allow(clippy::too_many_arguments)]
pub fn chart(
    entry: &Entry,
    panel: &Panel,
    plan: &PanelPlan,
    samples: &Samples,
    pack: &Pack,
    links: &GrafanaLinks,
    zone: &DisplayZone,
) -> ChartView {
    let unit = panel.unit.as_deref();
    let variables: BTreeMap<String, String> = plan
        .variables
        .iter()
        .filter_map(|v| v.value.clone().map(|value| (v.name.clone(), value)))
        .collect();
    let none = |_: &str| None;
    let mut series = Vec::new();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (i, result) in samples.queries.iter().enumerate() {
        let format = plan.legend_formats.get(i).cloned().flatten();
        for s in &result.series {
            let points: Vec<(i64, f64)> = s.points.iter().map(|p| (p.at_ms, p.value)).collect();
            for (_, v) in &points {
                if v.is_finite() {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
            }
            let mut labels = s.labels.clone();
            let metric = labels.remove("__name__");
            let name = legend::series_name(
                format.as_deref(),
                metric.as_deref(),
                &labels,
                &variables,
                none,
            );
            let values: Vec<f64> = points.iter().map(|(_, v)| *v).collect();
            let statistics = panel
                .legend
                .calcs
                .iter()
                .map(|c| {
                    legend::statistic(c, &values).map_or_else(
                        || "—".to_owned(),
                        |v| units::format(unit, v, panel.decimals),
                    )
                })
                .collect();
            series.push(ChartSeries {
                name,
                color: PALETTE[series.len() % PALETTE.len()].to_owned(),
                points,
                statistics,
            });
        }
    }
    let mut notes = Vec::new();
    if series.is_empty() {
        notes.push("The queries returned no series over this range.".to_owned());
    }
    let description = format!(
        "{} series read live, drawn as panel {} of {}.",
        series.len(),
        panel.id,
        entry.dashboard.title
    );
    assemble(
        &format!("panel-{}-{}", entry.dashboard.uid, panel.id),
        entry,
        panel,
        Drawing {
            series,
            notes,
            start: plan.window.start.as_millisecond(),
            end: plan.window.end.as_millisecond(),
            lo,
            hi,
            range_word: "the range read",
        },
        description,
        Vec::new(),
        pack,
        links,
        zone,
    )
}

/// The pack's reference to this panel, when the operations repository links to it.
pub fn reference<'a>(pack: &'a Pack, dashboard: &str, panel: i64) -> Option<&'a ReferenceDef> {
    pack.references.iter().find(|r| {
        r.kind == ReferenceKind::Panel
            && r.panel
                .as_ref()
                .is_some_and(|p| p.dashboard == dashboard && p.id == panel)
    })
}

/// Where a referenced panel's dashboard lives in the operations repository,
/// for the page to say which file to load when it isn't loaded.
pub fn find<'a>(
    library: &'a kyumei_dashboards::Library,
    dashboard: &str,
    panel: i64,
) -> Option<(&'a Entry, &'a Panel)> {
    let entry = library.get(dashboard)?;
    let panel = entry.dashboard.panels.iter().find(|p| p.id == panel)?;
    Some((entry, panel))
}

/// `15s`, `1m`, `1h30m`: a duration as Prometheus and Grafana write it.
fn duration_text(d: SignedDuration) -> String {
    let s = d.as_secs().max(1);
    match s {
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kyumei_dashboards::{Library, Origin};

    const DASHBOARD: &str = r##"{
      "uid": "test-overview", "title": "Test Overview v1.0", "version": 3,
      "templating": {"list": [
        {"name": "namespace", "type": "query", "query": "label_values(service_requests, namespace)"},
        {"name": "p", "type": "custom", "query": "0.50,0.95,0.99",
         "current": {"text": "0.99", "value": "0.99"},
         "options": [{"text": "0.50", "value": "0.50"}, {"text": "0.95", "value": "0.95"}, {"text": "0.99", "value": "0.99"}]}
      ]},
      "panels": [
        {"id": 82, "type": "timeseries", "title": "Visibility lag",
         "targets": [{"refId": "A", "legendFormat": "{{task_type}}",
           "expr": "histogram_quantile($p, sum by (le, task_type) (rate(task_latency_queue_bucket{namespace=\"$namespace\", task_type=~\"VisibilityTask.*\"}[$__rate_interval])))"}],
         "fieldConfig": {"defaults": {"unit": "s", "custom": {"thresholdsStyle": {"mode": "dashed"}},
           "thresholds": {"mode": "absolute", "steps": [{"color": "green", "value": null}, {"color": "orange", "value": 3}]}}},
         "options": {"legend": {"displayMode": "table", "calcs": ["max", "lastNotNull"]}}}
      ]
    }"##;

    fn library() -> Library {
        let mut l = Library::default();
        l.add(
            kyumei_dashboards::parse(DASHBOARD).expect("dashboard"),
            Origin::Upload("test.json".to_owned()),
        );
        l
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("timestamp")
    }

    #[test]
    fn variables_come_from_the_page_then_the_investigation_then_the_dashboard() {
        let library = library();
        let (entry, panel) = find(&library, "test-overview", 82).expect("panel");
        let now = ts("2026-10-14T14:00:00Z");

        // No investigation and nothing chosen: the namespace is unknown.
        let p = plan(entry, panel, None, None, &BTreeMap::new(), now);
        assert_eq!(p.range, PanelRange::Hours(1));
        assert_eq!(p.window.start, ts("2026-10-14T13:00:00Z"));
        assert_eq!(p.step_text(), "15s");
        assert_eq!(p.unfilled, ["namespace"]);
        assert!(
            p.queries[0].contains("histogram_quantile(0.99,"),
            "{}",
            p.queries[0]
        );
        assert!(
            p.queries[0].contains("[1m]"),
            "$__rate_interval: {}",
            p.queries[0]
        );

        // Chosen on the page.
        let chosen = BTreeMap::from([
            ("namespace".to_owned(), "orders".to_owned()),
            ("p".to_owned(), "0.95".to_owned()),
        ]);
        let p = plan(entry, panel, None, Some(PanelRange::Hours(6)), &chosen, now);
        assert!(p.unfilled.is_empty());
        assert_eq!(p.step_text(), "1m");
        assert!(p.queries[0].contains("histogram_quantile(0.95,"));
        assert!(p.queries[0].contains("namespace=\"orders\""));
        assert!(
            p.queries[0].contains("[1m15s]") || p.queries[0].contains("[75s]"),
            "{}",
            p.queries[0]
        );
        let pv = p.variables.iter().find(|v| v.name == "p").expect("p");
        assert_eq!(pv.options, ["0.50", "0.95", "0.99"]);
        assert_eq!(pv.source, "chosen");
    }

    #[test]
    fn samples_are_drawn_as_the_panel() {
        let library = library();
        let (entry, panel) = find(&library, "test-overview", 82).expect("panel");
        let now = ts("2026-10-14T14:00:00Z");
        let chosen = BTreeMap::from([("namespace".to_owned(), "orders".to_owned())]);
        let p = plan(entry, panel, None, None, &chosen, now);
        let start = p.window.start.as_millisecond();
        let samples = Samples {
            collector: "kyumei test".to_owned(),
            source: "prometheus".to_owned(),
            collected_at_ms: now.as_millisecond(),
            investigation: None,
            request: None,
            window: p.sample_window(),
            rounded_to: None,
            redacted: Vec::new(),
            queries: vec![kyumei_samples::QueryResult {
                query: p.queries[0].clone(),
                series: vec![kyumei_samples::Series {
                    labels: BTreeMap::from([(
                        "task_type".to_owned(),
                        "VisibilityTaskStartExecution".to_owned(),
                    )]),
                    points: (0..5)
                        .map(|i| kyumei_samples::Point {
                            at_ms: start + i * 15_000,
                            value: 0.02 * (i + 1) as f64,
                        })
                        .collect(),
                }],
            }],
        };
        let kb = kyumei_knowledge::KnowledgeBase::builtin().expect("knowledge");
        let pack = &kb.packs()[0];
        let c = chart(
            entry,
            panel,
            &p,
            &samples,
            pack,
            &GrafanaLinks::default(),
            &DisplayZone::utc(),
        );
        assert_eq!(c.title, "Visibility lag");
        assert!(
            c.text.contains("  VisibilityTaskStartExecution"),
            "{}",
            c.text
        );
        assert!(
            c.text.contains("100 ms"),
            "the maximum, in the panel's unit: {}",
            c.text
        );
        assert!(
            c.text
                .contains("Thresholds outside the range read, not drawn: 3 s (orange)."),
            "{}",
            c.text
        );
        assert_eq!(c.panel.open_url, "/panels/test-overview/82");
    }
}
