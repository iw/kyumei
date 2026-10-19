//! Grafana dashboard JSON: the panels Kyumei can draw evidence like, and the
//! ones it cannot read, listed rather than guessed at.
//!
//! Dashboards are read as Grafana saves them (schema versions 36 to 39, the
//! `timeseries` panel), as the HTTP API exports them (`{"dashboard": …}`), and
//! the older `graph` panel's unit, legend and thresholds.

use serde_json::Value;

/// A dashboard, as far as Kyumei reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct Dashboard {
    pub uid: String,
    pub title: String,
    /// Grafana's count of saves, which it calls the version.
    pub version: Option<i64>,
    pub schema_version: Option<i64>,
    pub variables: Vec<Variable>,
    /// Panels with Prometheus queries, in the dashboard's order.
    pub panels: Vec<Panel>,
    /// Panels that could not be read, and why.
    pub unreadable: Vec<Unreadable>,
}

/// A template variable, such as `$namespace` or the quantile `$p`.
#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    pub name: String,
    pub kind: String,
    /// A custom variable's choices.
    pub options: Vec<String>,
    pub current: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Panel {
    pub id: i64,
    pub title: String,
    pub kind: PanelKind,
    /// The row the panel sits in.
    pub row: Option<String>,
    pub targets: Vec<Target>,
    pub unit: Option<String>,
    pub decimals: Option<u32>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub thresholds: Thresholds,
    pub threshold_style: ThresholdStyle,
    pub legend: Legend,
}

impl Panel {
    /// A time series panel, which Kyumei can draw.
    pub fn is_chart(&self) -> bool {
        matches!(self.kind, PanelKind::TimeSeries | PanelKind::Graph)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelKind {
    TimeSeries,
    /// The pre-8.0 graph panel.
    Graph,
    Stat,
    Gauge,
    BarGauge,
    Table,
    Other(String),
}

impl PanelKind {
    fn parse(kind: &str) -> Self {
        match kind {
            "timeseries" => Self::TimeSeries,
            "graph" => Self::Graph,
            "stat" | "singlestat" => Self::Stat,
            "gauge" => Self::Gauge,
            "bargauge" => Self::BarGauge,
            "table" | "table-old" => Self::Table,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// One query of a panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub ref_id: String,
    pub expr: String,
    /// How the panel names each series, such as `{{operation}}`.
    pub legend_format: Option<String>,
    pub hidden: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thresholds {
    pub mode: ThresholdMode,
    /// In order; the first, with no value, is the base colour below every threshold.
    pub steps: Vec<ThresholdStep>,
}

impl Thresholds {
    /// The thresholds that have a value: every step but the base.
    pub fn levels(&self) -> impl Iterator<Item = (f64, &str)> {
        self.steps
            .iter()
            .filter_map(|s| s.value.map(|v| (v, s.color.as_str())))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThresholdMode {
    #[default]
    Absolute,
    /// Percentages of the panel's range, which Kyumei does not draw.
    Percentage,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThresholdStep {
    /// `None` for the base step.
    pub value: Option<f64>,
    pub color: String,
}

/// How a time series panel shows its thresholds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThresholdStyle {
    #[default]
    Off,
    Line,
    Dashed,
    Area,
    LineAndArea,
    DashedAndArea,
}

impl ThresholdStyle {
    fn parse(mode: &str) -> Self {
        match mode {
            "line" => Self::Line,
            "dashed" => Self::Dashed,
            "area" => Self::Area,
            "line+area" => Self::LineAndArea,
            "dashed+area" => Self::DashedAndArea,
            _ => Self::Off,
        }
    }

    pub fn shows_lines(self) -> bool {
        matches!(
            self,
            Self::Line | Self::Dashed | Self::LineAndArea | Self::DashedAndArea
        )
    }

    pub fn dashed(self) -> bool {
        matches!(self, Self::Dashed | Self::DashedAndArea)
    }

    pub fn shows_areas(self) -> bool {
        matches!(self, Self::Area | Self::LineAndArea | Self::DashedAndArea)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Legend {
    /// The statistics shown for each series.
    pub calcs: Vec<LegendCalc>,
    pub display: LegendDisplay,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LegendCalc {
    Min,
    Max,
    Mean,
    Last,
    /// The last value that is not null or NaN: Grafana's "Last *".
    LastNotNull,
    First,
    FirstNotNull,
    Sum,
    Count,
    Range,
    /// Statistics Kyumei does not compute, kept so the column is accounted for.
    Other(String),
}

impl LegendCalc {
    fn parse(calc: &str) -> Self {
        match calc {
            "min" => Self::Min,
            "max" => Self::Max,
            "mean" | "avg" => Self::Mean,
            "last" | "current" => Self::Last,
            "lastNotNull" => Self::LastNotNull,
            "first" => Self::First,
            "firstNotNull" => Self::FirstNotNull,
            "sum" | "total" => Self::Sum,
            "count" => Self::Count,
            "range" => Self::Range,
            other => Self::Other(other.to_owned()),
        }
    }

    /// The column heading Grafana gives the statistic.
    pub fn heading(&self) -> String {
        match self {
            Self::Min => "Min".to_owned(),
            Self::Max => "Max".to_owned(),
            Self::Mean => "Mean".to_owned(),
            Self::Last => "Last".to_owned(),
            Self::LastNotNull => "Last *".to_owned(),
            Self::First => "First".to_owned(),
            Self::FirstNotNull => "First *".to_owned(),
            Self::Sum => "Total".to_owned(),
            Self::Count => "Count".to_owned(),
            Self::Range => "Range".to_owned(),
            Self::Other(name) => name.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LegendDisplay {
    #[default]
    List,
    Table,
    Hidden,
}

/// A panel Kyumei could not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unreadable {
    pub id: Option<i64>,
    pub title: String,
    pub reason: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("not JSON: {0}")]
    Json(String),
    #[error("not a Grafana dashboard: it has no panels")]
    NoPanels,
    #[error("the dashboard has no uid")]
    NoUid,
}

/// Read a dashboard: one saved by Grafana, or exported by its HTTP API.
pub fn parse(json: &str) -> Result<Dashboard, ParseError> {
    let root: Value = serde_json::from_str(json).map_err(|e| ParseError::Json(e.to_string()))?;
    let d = if root["panels"].is_array() {
        &root
    } else if root["dashboard"]["panels"].is_array() {
        &root["dashboard"]
    } else {
        return Err(ParseError::NoPanels);
    };
    let uid = d["uid"]
        .as_str()
        .filter(|u| !u.is_empty())
        .ok_or(ParseError::NoUid)?;
    let mut dashboard = Dashboard {
        uid: uid.to_owned(),
        title: text(&d["title"]),
        version: d["version"].as_i64(),
        schema_version: d["schemaVersion"].as_i64(),
        variables: d["templating"]["list"]
            .as_array()
            .map(|vars| vars.iter().filter_map(variable).collect())
            .unwrap_or_default(),
        panels: Vec::new(),
        unreadable: Vec::new(),
    };
    read_panels(&d["panels"], None, &mut dashboard);
    Ok(dashboard)
}

fn read_panels(panels: &Value, row: Option<&str>, out: &mut Dashboard) {
    // An expanded row is followed by its panels; a collapsed row keeps them inside it.
    let mut current = row.map(str::to_owned);
    for p in panels.as_array().into_iter().flatten() {
        let kind = p["type"].as_str().unwrap_or_default();
        let title = text(&p["title"]);
        if kind == "row" {
            read_panels(&p["panels"], Some(&title), out);
            current = Some(title);
            continue;
        }
        if matches!(kind, "text" | "news" | "dashlist" | "alertlist" | "") {
            continue;
        }
        match panel(p, current.as_deref()) {
            Ok(Some(panel)) => out.panels.push(panel),
            Ok(None) => {}
            Err(reason) => out.unreadable.push(Unreadable {
                id: p["id"].as_i64(),
                title,
                reason,
            }),
        }
    }
}

/// A panel with Prometheus queries; `None` for one that has none to read.
fn panel(p: &Value, row: Option<&str>) -> Result<Option<Panel>, String> {
    if p.get("libraryPanel").is_some() && p.get("targets").is_none() {
        return Err(
            "a library panel: export the dashboard with its library panels included".to_owned(),
        );
    }
    let datasource = datasource_type(&p["datasource"]);
    if let Some(other) = datasource.as_deref().filter(|t| !is_prometheus(t)) {
        return Err(format!("its data source is {other}, not Prometheus"));
    }
    let id = p["id"].as_i64().ok_or("it has no id")?;
    let targets: Vec<Target> = p["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| {
            datasource_type(&t["datasource"])
                .as_deref()
                .is_none_or(is_prometheus)
        })
        .filter_map(|t| {
            let expr = t["expr"].as_str()?.trim();
            (!expr.is_empty()).then(|| Target {
                ref_id: text(&t["refId"]),
                expr: expr.to_owned(),
                legend_format: t["legendFormat"]
                    .as_str()
                    .filter(|l| !l.is_empty() && *l != "__auto")
                    .map(str::to_owned),
                hidden: t["hide"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    if targets.is_empty() {
        return Ok(None);
    }
    let kind = PanelKind::parse(p["type"].as_str().unwrap_or_default());
    let defaults = &p["fieldConfig"]["defaults"];
    let mut panel = Panel {
        id,
        title: text(&p["title"]),
        kind: kind.clone(),
        row: row.map(str::to_owned),
        targets,
        unit: defaults["unit"]
            .as_str()
            .filter(|u| !u.is_empty())
            .map(str::to_owned),
        decimals: defaults["decimals"]
            .as_u64()
            .and_then(|d| u32::try_from(d).ok()),
        min: defaults["min"].as_f64(),
        max: defaults["max"].as_f64(),
        thresholds: thresholds(&defaults["thresholds"]),
        threshold_style: ThresholdStyle::parse(
            defaults["custom"]["thresholdsStyle"]["mode"]
                .as_str()
                .unwrap_or_default(),
        ),
        legend: legend(&p["options"]["legend"]),
    };
    if kind == PanelKind::Graph {
        graph_panel(p, &mut panel);
    }
    Ok(Some(panel))
}

/// The pre-8.0 graph panel keeps its unit on the left axis and its legend as flags.
fn graph_panel(p: &Value, panel: &mut Panel) {
    if panel.unit.is_none() {
        panel.unit = p["yaxes"][0]["format"]
            .as_str()
            .filter(|u| !u.is_empty())
            .map(str::to_owned);
    }
    if panel.decimals.is_none() {
        panel.decimals = p["decimals"].as_u64().and_then(|d| u32::try_from(d).ok());
    }
    let l = &p["legend"];
    if panel.legend.calcs.is_empty() {
        for (flag, calc) in [
            ("min", LegendCalc::Min),
            ("max", LegendCalc::Max),
            ("avg", LegendCalc::Mean),
            ("current", LegendCalc::Last),
            ("total", LegendCalc::Sum),
        ] {
            if l[flag].as_bool() == Some(true) {
                panel.legend.calcs.push(calc);
            }
        }
        panel.legend.display = match (l["show"].as_bool(), l["alignAsTable"].as_bool()) {
            (Some(false), _) => LegendDisplay::Hidden,
            (_, Some(true)) => LegendDisplay::Table,
            _ => LegendDisplay::List,
        };
    }
    if panel.thresholds.steps.is_empty() {
        let mut steps: Vec<ThresholdStep> = p["thresholds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["op"].as_str().is_none_or(|op| op == "gt"))
            .filter_map(|t| {
                Some(ThresholdStep {
                    value: Some(t["value"].as_f64()?),
                    color: match t["colorMode"].as_str() {
                        Some("critical") => "red".to_owned(),
                        Some("warning") => "orange".to_owned(),
                        Some("ok") => "green".to_owned(),
                        _ => t["lineColor"].as_str().unwrap_or("red").to_owned(),
                    },
                })
            })
            .collect();
        if !steps.is_empty() {
            steps.insert(
                0,
                ThresholdStep {
                    value: None,
                    color: "green".to_owned(),
                },
            );
            panel.thresholds.steps = steps;
            panel.threshold_style = ThresholdStyle::Line;
        }
    }
}

fn thresholds(t: &Value) -> Thresholds {
    Thresholds {
        mode: match t["mode"].as_str() {
            Some("percentage") => ThresholdMode::Percentage,
            _ => ThresholdMode::Absolute,
        },
        steps: t["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| ThresholdStep {
                value: s["value"].as_f64(),
                color: s["color"].as_str().unwrap_or("green").to_owned(),
            })
            .collect(),
    }
}

fn legend(l: &Value) -> Legend {
    Legend {
        calcs: l["calcs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(LegendCalc::parse)
            .collect(),
        display: match (l["showLegend"].as_bool(), l["displayMode"].as_str()) {
            (Some(false), _) | (_, Some("hidden")) => LegendDisplay::Hidden,
            (_, Some("table")) => LegendDisplay::Table,
            _ => LegendDisplay::List,
        },
    }
}

fn variable(v: &Value) -> Option<Variable> {
    let name = v["name"].as_str()?.to_owned();
    let options = v["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| o["value"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    let options = if options.is_empty() && v["type"].as_str() == Some("custom") {
        v["query"]
            .as_str()
            .unwrap_or_default()
            .split(',')
            .map(|o| o.trim().to_owned())
            .filter(|o| !o.is_empty())
            .collect()
    } else {
        options
    };
    let current = match &v["current"]["value"] {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => a.first().and_then(Value::as_str).map(str::to_owned),
        _ => None,
    };
    Some(Variable {
        name,
        kind: text(&v["type"]),
        options,
        current,
    })
}

/// A data source's type: written as `{"type": "prometheus", "uid": …}`, or a name.
fn datasource_type(d: &Value) -> Option<String> {
    match d {
        Value::Object(o) => o.get("type").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

fn is_prometheus(kind: &str) -> bool {
    matches!(kind, "prometheus" | "datasource") || kind.contains("prometheus")
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A dashboard written for these tests, in the shape Grafana saves.
    pub(crate) const DASHBOARD: &str = r##"{
      "uid": "test-v1", "title": "Test Dashboard v1.0", "version": 3, "schemaVersion": 38,
      "templating": {"list": [
        {"name": "namespace", "type": "query", "current": {"value": "orders"}},
        {"name": "p", "type": "custom", "query": "0.50,0.95,0.99", "current": {"value": "0.99"}}
      ]},
      "panels": [
        {"id": 1, "type": "row", "title": "Latency", "collapsed": false, "panels": []},
        {"id": 2, "type": "text", "title": "", "options": {"content": "notes"}},
        {"id": 3, "type": "timeseries", "title": "Queue Latency",
         "datasource": {"type": "prometheus", "uid": "${DS}"},
         "targets": [{"refId": "A", "legendFormat": "{{task}}",
                      "expr": "histogram_quantile($p, sum by (task, le) (rate(queue_latency_bucket{namespace=\"$namespace\"}[$__rate_interval])))"}],
         "fieldConfig": {"defaults": {"unit": "s", "custom": {"thresholdsStyle": {"mode": "dashed"}},
            "thresholds": {"mode": "absolute", "steps": [{"color": "green", "value": null}, {"color": "orange", "value": 3}, {"color": "red", "value": 5}]}}},
         "options": {"legend": {"displayMode": "table", "calcs": ["min", "max", "mean", "lastNotNull"]}}},
        {"id": 4, "type": "row", "title": "Collapsed", "collapsed": true, "panels": [
          {"id": 5, "type": "graph", "title": "Old Graph", "decimals": 1,
           "targets": [{"refId": "A", "expr": "sum(rate(requests_total[1m]))"}],
           "yaxes": [{"format": "reqps"}, {"format": "short"}],
           "legend": {"show": true, "alignAsTable": true, "min": true, "max": true, "avg": false, "current": true},
           "thresholds": [{"value": 100, "op": "gt", "colorMode": "critical"}]}
        ]},
        {"id": 6, "type": "timeseries", "title": "Logs", "datasource": {"type": "loki", "uid": "L"},
         "targets": [{"refId": "A", "expr": "{app=\"x\"}"}]},
        {"id": 7, "type": "stat", "title": "Total", "targets": [{"refId": "A", "expr": "sum(up)"}]},
        {"id": 8, "type": "timeseries", "title": "Library", "libraryPanel": {"uid": "lib-1", "name": "Shared"}}
      ]
    }"##;

    #[test]
    fn reads_panels_inside_rows_and_lists_what_it_cannot_read() {
        let d = parse(DASHBOARD).expect("dashboard");
        assert_eq!((d.uid.as_str(), d.version), ("test-v1", Some(3)));
        assert_eq!(
            d.panels.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![3, 5, 7]
        );
        let latency = &d.panels[0];
        assert_eq!(latency.row.as_deref(), Some("Latency"));
        assert_eq!(latency.unit.as_deref(), Some("s"));
        assert_eq!(latency.threshold_style, ThresholdStyle::Dashed);
        assert_eq!(
            latency.thresholds.levels().collect::<Vec<_>>(),
            vec![(3.0, "orange"), (5.0, "red")]
        );
        assert_eq!(latency.legend.display, LegendDisplay::Table);
        assert_eq!(latency.legend.calcs[3].heading(), "Last *");
        assert_eq!(
            latency.targets[0].legend_format.as_deref(),
            Some("{{task}}")
        );
        let graph = &d.panels[1];
        assert_eq!(graph.row.as_deref(), Some("Collapsed"));
        assert_eq!(
            (graph.unit.as_deref(), graph.decimals),
            (Some("reqps"), Some(1))
        );
        assert_eq!(
            graph.legend.calcs,
            vec![LegendCalc::Min, LegendCalc::Max, LegendCalc::Last]
        );
        assert_eq!(
            graph.thresholds.levels().collect::<Vec<_>>(),
            vec![(100.0, "red")]
        );
        assert!(!d.panels[2].is_chart());
        let reasons: Vec<(Option<i64>, &str)> = d
            .unreadable
            .iter()
            .map(|u| (u.id, u.reason.as_str()))
            .collect();
        assert_eq!(
            reasons,
            vec![
                (Some(6), "its data source is loki, not Prometheus"),
                (
                    Some(8),
                    "a library panel: export the dashboard with its library panels included"
                ),
            ]
        );
        assert_eq!(d.variables[1].options, vec!["0.50", "0.95", "0.99"]);
        assert_eq!(d.variables[0].current.as_deref(), Some("orders"));
    }

    #[test]
    fn reads_an_api_export_and_refuses_what_is_not_a_dashboard() {
        let export = format!(r#"{{"meta": {{"slug": "x"}}, "dashboard": {DASHBOARD}}}"#);
        assert_eq!(parse(&export).expect("export").uid, "test-v1");
        assert_eq!(parse(r#"{"title": "x"}"#), Err(ParseError::NoPanels));
        assert_eq!(parse(r#"{"panels": []}"#), Err(ParseError::NoUid));
        assert!(matches!(parse("not json"), Err(ParseError::Json(_))));
    }
}
