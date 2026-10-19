//! Panels checked against what Kyumei's verified claims say about the metrics
//! they read: the names, the labels, the units, and thresholds that disagree
//! with other dashboards' for the same metric.

use std::collections::BTreeMap;

use crate::{
    grafana::Panel,
    library::{Entry, Library, base_name},
    promql,
    units::{format, seconds_per},
};

/// What Kyumei knows about one exposed name of a metric.
#[derive(Clone, Debug, PartialEq)]
pub struct KnownSeries {
    /// The claim that defines the metric.
    pub claim: String,
    /// The exposed name, without histogram suffixes.
    pub name: String,
    pub histogram: bool,
    /// For durations: how many seconds one exposed unit is (1 for seconds, 0.001 for
    /// milliseconds).
    pub seconds_per_unit: Option<f64>,
    /// The labels the claim lists; empty when they were not reviewed.
    pub labels: Vec<String>,
    /// Labels that carry the same value as another: `(label, same as)`.
    pub same_as: Vec<(String, String)>,
    /// The SDK whose exporter writes this name, such as `go`; `None` for the server.
    pub family: Option<String>,
}

/// Metrics of Kubernetes, the Go runtime and Prometheus itself, which no Temporal claim
/// covers.
const FOREIGN: &[&str] = &[
    "kube_",
    "process_",
    "go_",
    "node_",
    "container_",
    "machine_",
    "prometheus_",
    "scrape_",
    "promhttp_",
];

/// Labels Prometheus and Kubernetes add to every scraped series, which no claim lists.
const SCRAPE_LABELS: &[&str] = &[
    "le",
    "quantile",
    "instance",
    "job",
    "pod",
    "pod_name",
    "container",
    "node",
    "cluster",
    "endpoint",
    "service",
    "kubernetes_namespace",
    "k8s_namespace",
];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FindingKind {
    /// The panel reads a name no claim exposes.
    UnknownMetric,
    /// The panel uses a label the claim does not list.
    UnknownLabel,
    /// The panel uses a label Kyumei's requests read under another name; both carry
    /// the same value.
    SameValueLabel,
    /// The panel's time unit is not the one the metric is exposed in.
    UnitMismatch,
    /// The panel's thresholds are a different order of magnitude from other dashboards'
    /// for the same metric.
    ThresholdScale,
}

impl FindingKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::UnknownMetric => "Metric not in Kyumei's knowledge",
            Self::UnknownLabel => "Label the metric does not have",
            Self::SameValueLabel => "Same value, another label",
            Self::UnitMismatch => "Unit differs from the metric's",
            Self::ThresholdScale => "Thresholds out of scale",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub dashboard: String,
    pub dashboard_title: String,
    pub panel: i64,
    pub panel_title: String,
    pub kind: FindingKind,
    pub claim: Option<String>,
    pub detail: String,
}

/// A panel's thresholds, in seconds.
type Scaled<'a> = (&'a Entry, &'a Panel, Vec<f64>);

/// Check every chart panel of every dashboard against `known`. `families` names the SDK
/// a dashboard is for, by uid, when that is known: its units are checked against that
/// SDK's exporter, else against any that writes the name.
pub fn cross_check(
    library: &Library,
    known: &[KnownSeries],
    families: &BTreeMap<String, String>,
) -> Vec<Finding> {
    let by_name: BTreeMap<&str, Vec<&KnownSeries>> =
        known.iter().fold(BTreeMap::new(), |mut m, k| {
            m.entry(k.name.as_str()).or_insert_with(Vec::new).push(k);
            m
        });
    let lookup = |metric: &str| -> Option<&Vec<&KnownSeries>> {
        by_name
            .get(metric)
            .or_else(|| by_name.get(base_name(metric)))
    };
    let mut findings = Vec::new();
    // Thresholds in seconds per claim, for comparing dashboards.
    let mut scaled: BTreeMap<String, Vec<Scaled<'_>>> = BTreeMap::new();

    for entry in library.entries() {
        for panel in entry.dashboard.panels.iter().filter(|p| p.is_chart()) {
            let mut finding = |kind: FindingKind, claim: Option<&str>, detail: String| {
                let f = Finding {
                    dashboard: entry.dashboard.uid.clone(),
                    dashboard_title: entry.dashboard.title.clone(),
                    panel: panel.id,
                    panel_title: panel.title.clone(),
                    kind,
                    claim: claim.map(str::to_owned),
                    detail,
                };
                if !findings.contains(&f) {
                    findings.push(f);
                }
            };
            for target in panel.targets.iter().filter(|t| !t.hidden) {
                let query = promql::parse(&target.expr);
                let quantile = query.uses("histogram_quantile");
                for selector in &query.selectors {
                    let Some(metric) = selector.metric.as_deref() else {
                        continue;
                    };
                    if metric == "up" || FOREIGN.iter().any(|f| metric.starts_with(f)) {
                        continue;
                    }
                    let Some(series) = lookup(metric) else {
                        finding(
                            FindingKind::UnknownMetric,
                            None,
                            format!(
                                "`{metric}` is not a name any of Kyumei's metric claims exposes."
                            ),
                        );
                        continue;
                    };
                    let k = series[0];
                    // Labels: matchers on this selector, and the query's grouping.
                    let mut used: Vec<&str> =
                        selector.matchers.iter().map(|m| m.label.as_str()).collect();
                    used.extend(query.grouped_by.iter().map(String::as_str));
                    for label in used {
                        if label == "__name__"
                            || SCRAPE_LABELS.contains(&label)
                            || k.labels.is_empty()
                        {
                            continue;
                        }
                        if !k.labels.iter().any(|l| l == label) {
                            finding(
                                FindingKind::UnknownLabel,
                                Some(&k.claim),
                                format!(
                                    "`{metric}` has no label `{label}` in Kyumei's reviewed label set ({}).",
                                    k.labels.join(", ")
                                ),
                            );
                        } else if let Some((_, other)) = k.same_as.iter().find(|(l, _)| l == label)
                        {
                            finding(
                                FindingKind::SameValueLabel,
                                Some(&k.claim),
                                format!(
                                    "The panel reads `{label}` of `{metric}`, which carries the same value as `{other}`, the label Kyumei's requests use. Either works; a legend written with one is filled from the other."
                                ),
                            );
                        }
                    }
                    // Units: a quantile of a duration histogram is in the histogram's unit,
                    // which may differ between the SDKs that write the same name.
                    let family = families.get(&entry.dashboard.uid);
                    let scales: Vec<f64> = series
                        .iter()
                        .filter(|s| {
                            family.is_none() || s.family.is_none() || s.family.as_ref() == family
                        })
                        .filter_map(|s| s.seconds_per_unit)
                        .collect();
                    let panel_scale = panel.unit.as_deref().and_then(seconds_per);
                    let same = |a: f64, b: f64| (a - b).abs() <= f64::EPSILON * a.max(b);
                    if quantile
                        && k.histogram
                        && let (Some(panel_scale), Some(&metric_scale)) =
                            (panel_scale, scales.first())
                    {
                        if !scales.iter().any(|s| same(*s, panel_scale)) {
                            finding(
                                FindingKind::UnitMismatch,
                                Some(&k.claim),
                                format!(
                                    "The panel's unit is {}, but `{metric}` is exposed in {}: its values read {}× too {}.",
                                    unit_name(panel_scale),
                                    unit_name(metric_scale),
                                    ratio(panel_scale, metric_scale),
                                    if panel_scale > metric_scale {
                                        "large"
                                    } else {
                                        "small"
                                    },
                                ),
                            );
                        }
                        let levels: Vec<f64> = panel
                            .thresholds
                            .levels()
                            .map(|(v, _)| v * panel_scale)
                            .collect();
                        if !levels.is_empty() {
                            scaled
                                .entry(k.claim.clone())
                                .or_default()
                                .push((entry, panel, levels));
                        }
                    }
                }
            }
        }
    }

    // A panel whose thresholds sit a hundredfold or more from those of at least two
    // other dashboards for the same metric, which agree with each other more than with it.
    let far = |a: f64, b: f64| a > 0.0 && b > 0.0 && (a / b >= 100.0 || b / a >= 100.0);
    let top = |levels: &[f64]| levels.iter().copied().fold(f64::NAN, f64::max);
    for (claim, panels) in &scaled {
        for (i, (entry, panel, levels)) in panels.iter().enumerate() {
            let others: Vec<&Scaled<'_>> = panels
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, p)| p)
                .collect();
            if others.len() < 2 || others.iter().any(|(_, _, l)| !far(top(levels), top(l))) {
                continue;
            }
            let unit = panel.unit.as_deref();
            let shown: Vec<String> = panel
                .thresholds
                .levels()
                .map(|(v, _)| format(unit, v, None))
                .collect();
            let elsewhere: Vec<String> = others
                .iter()
                .map(|(e, p, l)| {
                    let at: Vec<String> = l.iter().map(|s| format(Some("s"), *s, None)).collect();
                    format!(
                        "{} panel {} at {}",
                        e.dashboard.title,
                        p.id,
                        at.join(" and ")
                    )
                })
                .collect();
            findings.push(Finding {
                dashboard: entry.dashboard.uid.clone(),
                dashboard_title: entry.dashboard.title.clone(),
                panel: panel.id,
                panel_title: panel.title.clone(),
                kind: FindingKind::ThresholdScale,
                claim: Some(claim.clone()),
                detail: format!(
                    "Thresholds at {} are a hundredfold or more from other dashboards' for the same metric: {}.",
                    shown.join(" and "),
                    elsewhere.join("; ")
                ),
            });
        }
    }
    findings.sort_by(|a, b| {
        (&a.dashboard_title, a.panel, &a.kind).cmp(&(&b.dashboard_title, b.panel, &b.kind))
    });
    findings
}

fn unit_name(seconds: f64) -> &'static str {
    match seconds {
        s if (s - 1.0).abs() < 1e-12 => "seconds",
        s if (s - 1e-3).abs() < 1e-15 => "milliseconds",
        s if (s - 1e-6).abs() < 1e-18 => "microseconds",
        s if (s - 1e-9).abs() < 1e-21 => "nanoseconds",
        s if (s - 60.0).abs() < 1e-9 => "minutes",
        s if (s - 3600.0).abs() < 1e-9 => "hours",
        _ => "another time unit",
    }
}

fn ratio(a: f64, b: f64) -> String {
    let r = if a > b { a / b } else { b / a };
    format!("{r:.0}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Origin, parse};

    fn dashboard(uid: &str, unit: &str, levels: &[f64], expr: &str) -> String {
        let steps: Vec<String> = std::iter::once(r#"{"color": "green", "value": null}"#.to_owned())
            .chain(
                levels
                    .iter()
                    .map(|v| format!(r#"{{"color": "red", "value": {v}}}"#)),
            )
            .collect();
        format!(
            r#"{{"uid": "{uid}", "title": "{uid}", "panels": [
              {{"id": 1, "type": "timeseries", "title": "Latency",
                "targets": [{{"refId": "A", "expr": {expr}}}],
                "fieldConfig": {{"defaults": {{"unit": "{unit}",
                  "thresholds": {{"steps": [{}]}}}}}}}}]}}"#,
            steps.join(", "),
            expr = serde_json::to_string(expr).unwrap_or_default()
        )
    }

    fn known(name: &str, scale: f64, family: Option<&str>) -> KnownSeries {
        KnownSeries {
            claim: "sdk.latency".into(),
            name: name.into(),
            histogram: true,
            seconds_per_unit: Some(scale),
            labels: vec!["namespace".into(), "task_type".into(), "operation".into()],
            same_as: vec![("operation".into(), "task_type".into())],
            family: family.map(str::to_owned),
        }
    }

    fn library(dashboards: &[String]) -> Library {
        let mut l = Library::default();
        for d in dashboards {
            l.add(
                parse(d).expect("dashboard"),
                Origin::Upload("t.json".into()),
            );
        }
        l
    }

    #[test]
    fn names_labels_and_units_are_checked_against_the_claims() {
        let q = r#"histogram_quantile(0.99, sum by (le, operation, pod) (rate(latency_bucket{namespace="x", region="eu"}[1m])))"#;
        let l = library(&[
            dashboard("a", "s", &[], q),
            dashboard(
                "b",
                "s",
                &[],
                "sum(rate(unknown_total[1m])) + sum(kube_pod_info) + up",
            ),
        ]);
        let found = cross_check(
            &l,
            &[
                known("latency", 0.001, Some("core")),
                known("latency", 1.0, Some("go")),
            ],
            &BTreeMap::new(),
        );
        let kinds: Vec<(&str, &FindingKind)> = found
            .iter()
            .map(|f| (f.dashboard.as_str(), &f.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("a", &FindingKind::UnknownLabel),
                ("a", &FindingKind::SameValueLabel),
                ("b", &FindingKind::UnknownMetric),
            ],
            "{found:#?}"
        );
        assert!(found[0].detail.contains("no label `region`"));
        // Seconds suit the Go exporter; a Core dashboard in seconds does not.
        let core: BTreeMap<String, String> = [("a".to_owned(), "core".to_owned())].into();
        let found = cross_check(
            &l,
            &[
                known("latency", 0.001, Some("core")),
                known("latency", 1.0, Some("go")),
            ],
            &core,
        );
        let unit = found
            .iter()
            .find(|f| f.kind == FindingKind::UnitMismatch)
            .expect("unit");
        assert_eq!(unit.dashboard, "a");
        assert!(
            unit.detail
                .contains("unit is seconds, but `latency_bucket` is exposed in milliseconds"),
            "{}",
            unit.detail
        );
    }

    #[test]
    fn thresholds_far_from_other_dashboards_for_the_same_metric_are_reported() {
        let q = "histogram_quantile(0.99, sum by (le) (rate(latency_bucket[1m])))";
        let l = library(&[
            dashboard("ms", "ms", &[1000.0, 5000.0], q),
            dashboard("s", "s", &[1.0, 5.0], q),
            dashboard("off", "s", &[1000.0, 5000.0], q),
        ]);
        let k = [
            known("latency", 0.001, Some("core")),
            known("latency", 1.0, Some("go")),
        ];
        let found: Vec<Finding> = cross_check(&l, &k, &BTreeMap::new())
            .into_iter()
            .filter(|f| f.kind == FindingKind::ThresholdScale)
            .collect();
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].dashboard, "off");
        assert!(
            found[0]
                .detail
                .contains("Thresholds at 16.7 min and 1.39 hour"),
            "{}",
            found[0].detail
        );
    }
}
