//! Dashboards and references in the workspace.
//!
//! - Evidence series drawn as the dashboard panel that reads the same metric
//!   draws them: its legend, unit, decimals, thresholds and legend statistics,
//!   citing the panel and where its dashboard came from.
//! - Loaded dashboards checked against what the knowledge says about the
//!   metrics their panels read.
//! - The runbooks, playbook sections and panels the diagnostics link to, cited
//!   at their source's pinned commit.
//!
//! Dashboards come from the operator; Kyumei bundles none.

use std::collections::BTreeMap;

use jiff::Timestamp;
use kyumei_dashboards::{
    Entry, KnownSeries, LegendDisplay, Library, PanelMatch, ThresholdMode, legend,
    library::base_name, promql, units,
};
use kyumei_diagrams::{
    Chart, ChartSeries, ChartThreshold, LegendStyle, nice_ticks, render_chart, render_chart_text,
    time_ticks,
};
use kyumei_domain::{
    ClaimId, DiagnosticId, DisplayZone, Evidence, EvidenceForm, Investigation, ObservedValue,
    SdkFamily, TimeWindow, Unit,
};
use kyumei_knowledge::{
    Claim, Instrument, KnowledgeBase, Pack, ReferenceDef, ReferenceKind, ReferenceTarget,
};
use serde::Serialize;

/// A runbook, playbook section or panel, as the workspace shows it.
#[derive(Clone, Debug, Serialize)]
pub struct ReferenceView {
    pub id: String,
    pub kind: &'static str,
    /// The kind with the alert number a runbook's file starts with: `Runbook 19b`.
    pub label: String,
    pub title: String,
    pub why: String,
    /// The material at its source's pinned commit.
    pub url: Option<String>,
    /// The source and commit, such as `tsurdilo/temporal-server-operations at
    /// 5a71c36, 9 October 2026`.
    pub cited: String,
    /// The version of what it cites, where the source declares one: `dashboard
    /// v2.25.0`, or `needs the Temporal Server dashboard v2.15.0+`.
    pub version: Option<String>,
    pub panel: Option<PanelRefView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PanelRefView {
    pub dashboard_title: String,
    pub id: i64,
    /// The panel in the operator's Grafana, over the incident window.
    pub grafana_url: Option<String>,
    /// The panel opened in Kyumei, with live data.
    pub open_url: String,
}

/// Where panels open: in Kyumei, with live data, and in the operator's Grafana,
/// and for what.
#[derive(Clone, Debug, Default)]
pub struct GrafanaLinks {
    pub base: Option<String>,
    pub window: Option<TimeWindow>,
    pub namespace: Option<String>,
    /// The investigation panels are opened for.
    pub investigation: Option<String>,
}

impl GrafanaLinks {
    pub fn for_investigation(base: Option<&str>, inv: &Investigation) -> Self {
        Self {
            base: base.map(|b| b.trim_end_matches('/').to_owned()),
            window: inv.window.as_window(),
            namespace: inv.scope.primary_namespace().map(str::to_owned),
            investigation: Some(inv.id.to_string()),
        }
    }

    /// The panel's page in Kyumei: in the investigation when there is one.
    pub fn open(&self, dashboard: &str, panel: i64) -> String {
        match &self.investigation {
            Some(id) => format!(
                "/investigations/{}/panels/{}/{panel}",
                encode(id),
                encode(dashboard)
            ),
            None => format!("/panels/{}/{panel}", encode(dashboard)),
        }
    }

    /// A panel opened on its own over the window, with the namespace variable set.
    pub fn panel(&self, dashboard: &str, panel: i64, window: Option<TimeWindow>) -> Option<String> {
        let base = self.base.as_deref()?;
        let mut url = format!("{base}/d/{}?viewPanel={panel}", encode(dashboard));
        if let Some(w) = window.or(self.window) {
            url.push_str(&format!(
                "&from={}&to={}",
                w.start.as_millisecond(),
                w.end.as_millisecond()
            ));
        }
        if let Some(ns) = &self.namespace {
            url.push_str(&format!("&var-namespace={}", encode(ns)));
        }
        Some(url)
    }
}

/// The references that bear on `target`, as views. SDK dashboard panels are kept to
/// the investigation's SDKs when they are known.
pub fn reference_views(
    pack: &Pack,
    diagnostic: &DiagnosticId,
    target: ReferenceTarget<'_>,
    families: &[SdkFamily],
    links: &GrafanaLinks,
) -> Vec<ReferenceView> {
    pack.references_for(diagnostic, target)
        .into_iter()
        .filter(|r| {
            r.panel
                .as_ref()
                .and_then(|p| p.sdk_family)
                .is_none_or(|f| families.is_empty() || families.contains(&f))
        })
        .map(|r| reference_view(pack, r, links))
        .collect()
}

pub fn reference_view(pack: &Pack, r: &ReferenceDef, links: &GrafanaLinks) -> ReferenceView {
    let source = pack.source(&r.source);
    ReferenceView {
        id: r.id.to_string(),
        kind: r.kind.label(),
        label: reference_label(r),
        title: r.display_title(),
        why: r.why.clone(),
        url: source.and_then(|s| s.file_url(&r.path, r.anchor().as_deref())),
        cited: source.map_or_else(|| r.source.to_string(), cited),
        version: r
            .version
            .as_ref()
            .map(kyumei_knowledge::ReferenceVersion::describe),
        panel: r.panel.as_ref().map(|p| PanelRefView {
            dashboard_title: p.dashboard_title.clone(),
            id: p.id,
            grafana_url: links.panel(&p.dashboard, p.id, None),
            open_url: links.open(&p.dashboard, p.id),
        }),
    }
}

/// `Runbook 19b` for a runbook whose file is `19b-….md`; the kind otherwise.
fn reference_label(r: &ReferenceDef) -> String {
    let file = r.path.rsplit('/').next().unwrap_or_default();
    let number = file.split('-').next().unwrap_or_default();
    if r.kind == ReferenceKind::Runbook
        && number.starts_with(|c: char| c.is_ascii_digit())
        && number.chars().all(|c| c.is_ascii_alphanumeric())
    {
        format!("{} {number}", r.kind.label())
    } else {
        r.kind.label().to_owned()
    }
}

/// `owner/repository at abc1234, 9 October 2026`.
fn cited(source: &kyumei_knowledge::SourceDef) -> String {
    let name = source
        .repository
        .as_deref()
        .map(|r| {
            r.trim_end_matches('/')
                .trim_start_matches("https://github.com/")
        })
        .unwrap_or(source.title.as_str());
    match (source.short_commit(), source.committed) {
        (Some(c), Some(date)) => format!("{name} at {c}, {}", date.strftime("%-d %B %Y")),
        (Some(c), None) => format!("{name} at {c}"),
        (None, _) => name.to_owned(),
    }
}

/// What the pack knows about every exposed metric name, for checking dashboards. A
/// server metric also carries `service_name` when the pack says every one does
/// (`server.metrics-service-name`).
pub fn known_series(pack: &Pack) -> Vec<KnownSeries> {
    let service_name = pack
        .claim(&ClaimId::new("server.metrics-service-name"))
        .is_some();
    let mut out = Vec::new();
    for claim in pack.claims.values() {
        let Claim::Metric(m) = claim else {
            continue;
        };
        let histogram = matches!(m.instrument, Instrument::Histogram | Instrument::Timer);
        for e in &m.exposition {
            let mut labels: Vec<String> = m.labels.iter().map(|l| l.name.clone()).collect();
            if service_name
                && e.sdk_family.is_none()
                && !labels.is_empty()
                && !labels.iter().any(|l| l == "service_name")
            {
                labels.push("service_name".to_owned());
            }
            out.push(KnownSeries {
                claim: m.id.to_string(),
                name: e.name.clone(),
                histogram,
                seconds_per_unit: e.unit.or(m.unit).and_then(seconds_of),
                labels,
                same_as: m
                    .labels
                    .iter()
                    .filter_map(|l| l.same_as.clone().map(|s| (l.name.clone(), s)))
                    .collect(),
                family: e.sdk_family.map(|f| f.token().to_owned()),
            });
        }
    }
    out
}

/// The loaded dashboards' panels checked against the pack's metric claims.
pub fn cross_check(library: &Library, pack: &Pack) -> Vec<kyumei_dashboards::Finding> {
    kyumei_dashboards::cross_check(library, &known_series(pack), &dashboard_families(pack))
}

/// The SDK each referenced dashboard is for, by uid, as the references say.
pub fn dashboard_families(pack: &Pack) -> BTreeMap<String, String> {
    pack.references
        .iter()
        .filter_map(|r| r.panel.as_ref())
        .filter_map(|p| {
            p.sdk_family
                .map(|f| (p.dashboard.clone(), f.token().to_owned()))
        })
        .collect()
}

fn seconds_of(unit: Unit) -> Option<f64> {
    match unit {
        Unit::Seconds => Some(1.0),
        Unit::Milliseconds => Some(0.001),
        _ => None,
    }
}

/// One evidence chart, drawn as a panel draws it.
#[derive(Clone, Debug, Serialize)]
pub struct ChartView {
    pub id: String,
    pub title: String,
    pub svg: String,
    pub text: String,
    /// The observations it draws.
    pub observations: Vec<String>,
    pub panel: PanelCitation,
}

/// The panel a chart is drawn as, and where its dashboard came from.
#[derive(Clone, Debug, Serialize)]
pub struct PanelCitation {
    pub panel_title: String,
    pub panel_id: i64,
    pub dashboard_title: String,
    pub dashboard_uid: String,
    pub dashboard_version: Option<i64>,
    /// Where Kyumei loaded the dashboard from.
    pub origin: String,
    /// The dashboard in the operations repository, when it is one of its dashboards.
    pub reference: Option<ReferenceView>,
    pub grafana_url: Option<String>,
    /// The panel opened in Kyumei, with live data.
    pub open_url: String,
}

/// Classic dashboard series colours, in order.
pub(crate) const PALETTE: &[&str] = &[
    "#7eb26d", "#eab839", "#6ed0e0", "#ef843c", "#e24d42", "#1f78c1", "#ba43a9", "#705da0",
    "#508642", "#cca300", "#447ebc", "#c15c17", "#890f02", "#0a437c", "#6d1f62", "#584477",
];

/// Charts for an evidence's series: each group of series of one metric, drawn as the
/// best matching chart panel of the loaded dashboards, and groups drawn by the same
/// panel drawn together. Series no panel can draw are left to the observation table.
pub fn evidence_charts(
    library: &Library,
    kb: &KnowledgeBase,
    inv: &Investigation,
    evidence: &Evidence,
    links: &GrafanaLinks,
    zone: &DisplayZone,
) -> Vec<ChartView> {
    if library.is_empty() {
        return Vec::new();
    }
    let Ok((pack, _, diagnostic)) = kyumei_diagnostics::select(kb, inv) else {
        return Vec::new();
    };
    let diagnostic = diagnostic.map(|d| d.id.clone());
    let request = evidence
        .responds_to
        .as_ref()
        .and_then(|r| inv.request(r))
        .map(|r| r.definition.clone());
    let queries = evidence_queries(evidence);

    // Group the series by what they measure: the claim they bind to, else their name.
    let mut groups: BTreeMap<String, Vec<&kyumei_domain::Observation>> = BTreeMap::new();
    let effective = evidence.effective_observations(&inv.corrections);
    for o in effective.iter().filter(|o| !o.excluded) {
        let o = &o.observation;
        if !matches!(o.value, ObservedValue::Series { .. }) {
            continue;
        }
        let key = o
            .binding
            .as_ref()
            .and_then(|b| b.claim.as_ref())
            .map(|c| format!("claim:{c}"))
            .or_else(|| o.subject.metric.clone().map(|m| format!("metric:{m}")));
        if let Some(key) = key {
            groups.entry(key).or_default().push(o);
        }
    }
    // The SDKs that exported the evidence: the investigation's, else what its own
    // series names tell.
    let families = {
        let declared = inv.environment.sdk_families();
        if declared.is_empty() {
            evidence_families(pack, groups.values().flatten().copied())
        } else {
            declared
        }
    };

    // The panel for each group, then the groups each panel draws.
    let mut chosen: Vec<(
        PanelMatch<'_>,
        Vec<&kyumei_domain::Observation>,
        Option<String>,
    )> = Vec::new();
    for (key, observations) in groups {
        let claim: Option<ClaimId> = key.strip_prefix("claim:").map(ClaimId::new);
        let mut names: Vec<String> = observations
            .iter()
            .filter_map(|o| exposed_name(o))
            .collect();
        if let Some(c) = &claim {
            for family in families.iter().copied().map(Some).chain([None]) {
                names.extend(pack.exposed_names(c, family).iter().map(|e| e.name.clone()));
            }
        }
        names.sort();
        names.dedup();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let query = queries
            .iter()
            .find(|q| {
                promql::parse(q)
                    .metrics()
                    .iter()
                    .any(|m| name_refs.iter().any(|n| base_name(m) == base_name(n)))
            })
            .cloned();
        let equivalent = equivalents(pack, claim.as_ref());
        let unit = observations.iter().find_map(|o| o.value.unit());
        let labels_present: Vec<String> = observations
            .iter()
            .flat_map(|o| o.subject.labels.keys().cloned())
            .collect();
        // The highest score wins; among equals, the dashboard loaded first.
        let best = library
            .panels_reading(&name_refs)
            .into_iter()
            .filter(|m| {
                drawable(
                    m,
                    pack,
                    &families,
                    query.as_deref(),
                    unit,
                    &observations,
                    &equivalent,
                )
            })
            .enumerate()
            .max_by_key(|(i, m)| {
                (
                    score(
                        m,
                        pack,
                        &families,
                        diagnostic.as_ref(),
                        request.as_ref(),
                        &labels_present,
                        &equivalent,
                    ),
                    std::cmp::Reverse(*i),
                )
            })
            .map(|(_, m)| m);
        if let Some(best) = best {
            chosen.push((best, observations, query));
        }
    }
    let mut panels: Vec<(String, i64)> = Vec::new();
    for (m, _, _) in &chosen {
        let key = (m.entry.dashboard.uid.clone(), m.panel.id);
        if !panels.contains(&key) {
            panels.push(key);
        }
    }
    panels
        .iter()
        .enumerate()
        .map(|(n, (uid, id))| {
            let parts: Vec<&(
                PanelMatch<'_>,
                Vec<&kyumei_domain::Observation>,
                Option<String>,
            )> = chosen
                .iter()
                .filter(|(m, _, _)| &m.entry.dashboard.uid == uid && m.panel.id == *id)
                .collect();
            draw(
                &format!("{}-chart-{}", evidence.id, n + 1),
                &parts,
                inv,
                pack,
                links,
                zone,
            )
        })
        .collect()
}

/// Time-axis ticks at round times in `zone`, labelled with its clock.
pub fn zone_ticks(start_ms: i64, end_ms: i64, zone: &DisplayZone) -> Vec<(i64, String)> {
    let offset_ms = Timestamp::from_millisecond(start_ms)
        .map_or(0, |t| i64::from(zone.offset_seconds(t)) * 1000);
    time_ticks(start_ms + offset_ms, end_ms + offset_ms, 6)
        .into_iter()
        .map(|t| t - offset_ms)
        .filter_map(|t| {
            Timestamp::from_millisecond(t)
                .ok()
                .map(|at| (t, zone.clock(at)))
        })
        .collect()
}

/// The name a series was exposed under: its metric, or the name it was bound by.
fn exposed_name(o: &kyumei_domain::Observation) -> Option<String> {
    o.subject
        .metric
        .clone()
        .or_else(|| match o.binding.as_ref().map(|b| &b.basis) {
            Some(kyumei_domain::BindingBasis::MetricName { exposed_name }) => {
                Some(exposed_name.clone())
            }
            _ => None,
        })
}

/// The SDKs whose exporters write every name among the series, when the names tell:
/// `temporal_workflow_task_execution_failed_total` only Java's.
fn evidence_families<'o>(
    pack: &Pack,
    observations: impl Iterator<Item = &'o kyumei_domain::Observation>,
) -> Vec<SdkFamily> {
    let mut families: Option<Vec<SdkFamily>> = None;
    for o in observations {
        let Some(name) = exposed_name(o) else {
            continue;
        };
        let mut these: Vec<SdkFamily> = pack
            .match_metric(&name)
            .iter()
            .filter_map(|m| m.exposition.sdk_family)
            .collect();
        these.sort();
        these.dedup();
        if these.is_empty() {
            continue;
        }
        families = Some(match families {
            None => these,
            Some(f) => f.into_iter().filter(|x| these.contains(x)).collect(),
        });
    }
    families.unwrap_or_default()
}

/// Labels that carry the same value as another, both ways round.
fn equivalents(pack: &Pack, claim: Option<&ClaimId>) -> impl Fn(&str) -> Option<String> + use<> {
    let pairs: Vec<(String, String)> = claim
        .and_then(|c| pack.claim(c))
        .and_then(Claim::as_metric)
        .map(|m| {
            m.labels
                .iter()
                .filter_map(|l| l.same_as.clone().map(|s| (l.name.clone(), s)))
                .collect()
        })
        .unwrap_or_default();
    move |label: &str| {
        pairs.iter().find_map(|(a, b)| {
            if a == label {
                Some(b.clone())
            } else if b == label {
                Some(a.clone())
            } else {
                None
            }
        })
    }
}

/// Whether a panel's query shows the same quantity as the evidence: one metric (not a
/// ratio of two), a quantile only for a quantile, a percentage only for a ratio, a
/// dashboard and a name for the evidence's SDK, and filters that keep the series.
fn drawable(
    m: &PanelMatch<'_>,
    pack: &Pack,
    families: &[SdkFamily],
    query: Option<&str>,
    unit: Option<Unit>,
    observations: &[&kyumei_domain::Observation],
    equivalent: &dyn Fn(&str) -> Option<String>,
) -> bool {
    let metrics = m.query.metrics();
    if metrics
        .iter()
        .map(|n| base_name(n))
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != 1
    {
        return false;
    }
    let family_ok = |f: SdkFamily| families.is_empty() || families.contains(&f);
    if dashboard_family(pack, &m.entry.dashboard.uid).is_some_and(|f| !family_ok(f)) {
        return false;
    }
    let name_families: Vec<SdkFamily> = pack
        .match_metric(&m.metric)
        .iter()
        .filter_map(|x| x.exposition.sdk_family)
        .collect();
    if !name_families.is_empty()
        && !families.is_empty()
        && !name_families.iter().any(|f| families.contains(f))
    {
        return false;
    }
    if let Some(q) = query
        && q.contains("histogram_quantile") != m.query.uses("histogram_quantile")
    {
        return false;
    }
    if m.panel
        .unit
        .as_deref()
        .and_then(units::percent_scale)
        .is_some()
        && unit != Some(Unit::Ratio)
    {
        return false;
    }
    // The panel's filters must keep at least one of the series.
    let Some(selector) = m
        .query
        .selectors
        .iter()
        .find(|s| s.metric.as_deref() == Some(m.metric.as_str()))
    else {
        return true;
    };
    observations.iter().any(|o| {
        selector.matchers.iter().all(|matcher| {
            if matcher.value.contains('$') || matcher.label == "__name__" {
                return true;
            }
            let value = o
                .subject
                .labels
                .get(&matcher.label)
                .or_else(|| equivalent(&matcher.label).and_then(|e| o.subject.labels.get(&e)));
            let Some(value) = value else {
                return true;
            };
            let full = |pattern: &str| {
                regex::Regex::new(&format!("^(?:{pattern})$")).is_ok_and(|r| r.is_match(value))
            };
            match matcher.op {
                promql::MatchOp::Equal => value == &matcher.value,
                promql::MatchOp::NotEqual => value != &matcher.value,
                promql::MatchOp::Regex => full(&matcher.value),
                promql::MatchOp::NotRegex => !full(&matcher.value),
            }
        })
    })
}

/// Higher for the panel the request links to, a dashboard known to be for the
/// evidence's SDK, a query grouping by the series' labels, and a time series panel.
fn score(
    m: &PanelMatch<'_>,
    pack: &Pack,
    families: &[SdkFamily],
    diagnostic: Option<&DiagnosticId>,
    request: Option<&kyumei_domain::RequestDefinitionId>,
    labels: &[String],
    equivalent: &dyn Fn(&str) -> Option<String>,
) -> (u8, u8, usize, u8) {
    let referenced = match (diagnostic, request) {
        (Some(d), Some(r)) => pack
            .references_for(d, ReferenceTarget::Request(r))
            .iter()
            .any(|r| {
                r.panel
                    .as_ref()
                    .is_some_and(|p| p.dashboard == m.entry.dashboard.uid && p.id == m.panel.id)
            }),
        _ => false,
    };
    let grouping = m
        .query
        .grouped_by
        .iter()
        .filter(|g| *g != "le")
        .filter(|g| labels.contains(g) || equivalent(g).is_some_and(|e| labels.contains(&e)))
        .count();
    let timeseries = u8::from(m.panel.kind == kyumei_dashboards::PanelKind::TimeSeries);
    let for_sdk = u8::from(
        dashboard_family(pack, &m.entry.dashboard.uid).is_some_and(|f| families.contains(&f)),
    );
    (u8::from(referenced), for_sdk, grouping, timeseries)
}

/// The SDK a dashboard is for, when the references say.
fn dashboard_family(pack: &Pack, uid: &str) -> Option<SdkFamily> {
    pack.references.iter().find_map(|r| {
        r.panel
            .as_ref()
            .filter(|p| p.dashboard == uid)
            .and_then(|p| p.sdk_family)
    })
}

/// The queries an evidence was collected with: the one declared with it, and each
/// query of a sample block.
fn evidence_queries(evidence: &Evidence) -> Vec<String> {
    let mut queries: Vec<String> = evidence.declared.query.iter().cloned().collect();
    if evidence.interpretation.form == EvidenceForm::SampleBlock
        && let Some(text) = evidence.raw.text()
    {
        for samples in kyumei_samples::decode_all(text).into_iter().flatten() {
            queries.extend(samples.queries.into_iter().map(|q| q.query));
        }
    }
    queries
}

/// One chart: a panel and the groups of series it draws, each named by its own query's
/// legend format.
fn draw(
    id: &str,
    parts: &[&(
        PanelMatch<'_>,
        Vec<&kyumei_domain::Observation>,
        Option<String>,
    )],
    inv: &Investigation,
    pack: &Pack,
    links: &GrafanaLinks,
    zone: &DisplayZone,
) -> ChartView {
    let first = &parts[0].0;
    let panel = first.panel;
    let entry: &Entry = first.entry;
    let unit = panel.unit.as_deref();
    let mut notes = Vec::new();
    let mut filled_from = Vec::new();
    let mut series = Vec::new();
    let mut observed = Vec::new();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut start, mut end) = (i64::MAX, i64::MIN);
    for (m, observations, query) in parts.iter().map(|p| (&p.0, &p.1, &p.2)) {
        let claim = observations
            .iter()
            .find_map(|o| o.binding.as_ref().and_then(|b| b.claim.clone()));
        let equivalent = equivalents(pack, claim.as_ref());
        let mut variables: BTreeMap<String, String> = BTreeMap::new();
        if let Some(ns) = inv.scope.primary_namespace() {
            variables.insert("namespace".to_owned(), ns.to_owned());
        }
        variables.insert("__rate_interval".to_owned(), "1m".to_owned());
        if let Some(p) = query.as_deref().and_then(quantile) {
            variables.insert("p".to_owned(), p);
        }
        let legend_format = m.target.legend_format.as_deref();
        for o in observations.iter() {
            let ObservedValue::Series { unit: from, points } = &o.value else {
                continue;
            };
            let factor = conversion(*from, unit);
            let points: Vec<(i64, f64)> = points
                .iter()
                .map(|p| (p.at.as_millisecond(), p.value * factor))
                .collect();
            for (t, v) in &points {
                start = start.min(*t);
                end = end.max(*t);
                if v.is_finite() {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
            }
            for label in legend_labels(legend_format.unwrap_or_default()) {
                if let Some(e) = equivalent(&label)
                    && !o.subject.labels.contains_key(&label)
                    && o.subject.labels.contains_key(&e)
                {
                    filled_from.push((label, e));
                }
            }
            let name = legend::series_name(
                legend_format,
                o.subject.metric.as_deref(),
                &o.subject.labels,
                &variables,
                &equivalent,
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
            observed.push(o.id.to_string());
        }
    }
    filled_from.sort();
    filled_from.dedup();
    for (label, from) in &filled_from {
        notes.push(format!(
            "The panel's legend names series by `{label}`; these carry the same value as `{from}`, which Kyumei read."
        ));
    }
    let evidence = parts[0]
        .1
        .first()
        .map_or_else(String::new, |o| o.evidence.to_string());
    let description = format!(
        "{} series from evidence {evidence}, drawn as panel {} of {}.",
        series.len(),
        panel.id,
        entry.dashboard.title
    );
    assemble(
        id,
        entry,
        panel,
        Drawing {
            series,
            notes,
            start,
            end,
            lo,
            hi,
            range_word: "the evidence's range",
        },
        description,
        observed,
        pack,
        links,
        zone,
    )
}

/// Series ready to be drawn as a panel, and what the chart says about them.
pub(crate) struct Drawing {
    pub series: Vec<ChartSeries>,
    pub notes: Vec<String>,
    /// The time and value ranges the series cover.
    pub start: i64,
    pub end: i64,
    pub lo: f64,
    pub hi: f64,
    /// What the thresholds fall outside of, in a note: `the evidence's range`.
    pub range_word: &'static str,
}

/// A chart of `drawing` as `panel` of `entry` draws it: its axes, thresholds,
/// legend and the citation of its panel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn assemble(
    id: &str,
    entry: &Entry,
    panel: &kyumei_dashboards::Panel,
    drawing: Drawing,
    description: String,
    observed: Vec<String>,
    pack: &Pack,
    links: &GrafanaLinks,
    zone: &DisplayZone,
) -> ChartView {
    let Drawing {
        series,
        mut notes,
        start,
        end,
        lo,
        hi,
        range_word,
    } = drawing;
    let unit = panel.unit.as_deref();
    let (lo, hi) = if lo.is_finite() { (lo, hi) } else { (0.0, 1.0) };
    let lo = panel.min.unwrap_or(if lo >= 0.0 { 0.0 } else { lo });
    let ticks = match unit.and_then(units::seconds_per) {
        Some(scale) => duration_ticks(lo, panel.max.unwrap_or(hi.max(lo)), scale),
        None => nice_ticks(lo, panel.max.unwrap_or(hi.max(lo)), 4),
    };
    let hi = panel
        .max
        .unwrap_or_else(|| ticks.last().copied().filter(|t| *t >= hi).unwrap_or(hi));
    let y_ticks: Vec<(f64, String)> = ticks
        .iter()
        .map(|t| (*t, units::format(unit, *t, panel.decimals)))
        .collect();
    let x_ticks = zone_ticks(start, end, zone);

    let mut thresholds = Vec::new();
    if panel.thresholds.mode == ThresholdMode::Percentage {
        notes.push(
            "The panel's thresholds are percentages of its range, which Kyumei does not draw."
                .to_owned(),
        );
    } else if panel.threshold_style.shows_lines() || panel.threshold_style.shows_areas() {
        let mut outside = Vec::new();
        for (value, color) in panel.thresholds.levels() {
            let label = format!("{} ({color})", units::format(unit, value, panel.decimals));
            let in_range = value >= lo && value <= hi;
            if !in_range {
                outside.push(label.clone());
            }
            thresholds.push(ChartThreshold {
                value,
                label,
                color: color_hex(color),
                dashed: panel.threshold_style.dashed(),
                line: in_range && panel.threshold_style.shows_lines(),
            });
        }
        if !outside.is_empty() {
            notes.push(format!(
                "Thresholds outside {range_word}, not drawn: {}.",
                outside.join(", ")
            ));
        }
    }

    let reference = pack
        .references
        .iter()
        .find(|r| {
            r.kind == ReferenceKind::Panel
                && r.panel
                    .as_ref()
                    .is_some_and(|p| p.dashboard == entry.dashboard.uid)
        })
        .map(|r| {
            let mut v = reference_view(pack, r, links);
            v.title = entry.dashboard.title.clone();
            v.kind = "Dashboard";
            v.label = "Dashboard".to_owned();
            v.why = String::new();
            v.panel = None;
            v
        });
    let version = entry
        .dashboard
        .version
        .map(|v| format!(", version {v}"))
        .unwrap_or_default();
    let mut caption = vec![format!(
        "Drawn as panel “{}” (id {}) of “{}”{version}, uid {}, loaded from {}.",
        panel.title,
        panel.id,
        entry.dashboard.title,
        entry.dashboard.uid,
        entry.origin.describe()
    )];
    if let Some(r) = &reference {
        caption.push(format!("The dashboard is from {}.", r.cited));
    }
    caption.extend(notes);
    let window = match (
        Timestamp::from_millisecond(start),
        Timestamp::from_millisecond(end),
    ) {
        (Ok(s), Ok(e)) if start <= end => TimeWindow::new(s, e).ok(),
        _ => None,
    };
    let chart = Chart {
        id: id.to_owned(),
        title: panel.title.clone(),
        description,
        x_range: (start.min(end), end.max(start)),
        y_range: (lo, hi),
        x_ticks,
        y_ticks,
        series,
        thresholds,
        threshold_areas: panel.threshold_style.shows_areas(),
        legend_columns: panel.legend.calcs.iter().map(|c| c.heading()).collect(),
        legend: match panel.legend.display {
            LegendDisplay::Hidden => LegendStyle::Hidden,
            LegendDisplay::List => LegendStyle::List,
            LegendDisplay::Table => LegendStyle::Table,
        },
        caption,
    };
    ChartView {
        id: id.to_owned(),
        title: panel.title.clone(),
        svg: render_chart(&chart),
        text: render_chart_text(&chart),
        observations: observed,
        panel: PanelCitation {
            panel_title: panel.title.clone(),
            panel_id: panel.id,
            dashboard_title: entry.dashboard.title.clone(),
            dashboard_uid: entry.dashboard.uid.clone(),
            dashboard_version: entry.dashboard.version,
            origin: entry.origin.describe(),
            reference,
            grafana_url: links.panel(&entry.dashboard.uid, panel.id, window),
            open_url: links.open(&entry.dashboard.uid, panel.id),
        },
    }
}

/// Ticks for a duration axis at a round interval of time (1 s, 30 s, 2 min, 1 hour),
/// in the panel's unit, `scale` seconds each.
fn duration_ticks(lo: f64, hi: f64, scale: f64) -> Vec<f64> {
    const STEPS: &[f64] = &[
        1e-6, 2e-6, 5e-6, 1e-5, 2e-5, 5e-5, 1e-4, 2e-4, 5e-4, 1e-3, 2e-3, 5e-3, 0.01, 0.02, 0.05,
        0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0,
        3600.0, 7200.0, 10800.0, 21600.0, 43200.0, 86400.0, 172800.0, 604800.0,
    ];
    let (lo_s, hi_s) = (lo * scale, hi * scale);
    if !(lo_s.is_finite() && hi_s.is_finite()) || hi_s <= lo_s {
        return nice_ticks(lo, hi, 4);
    }
    let Some(step) = STEPS.iter().copied().find(|s| (hi_s - lo_s) / s <= 4.0) else {
        return nice_ticks(lo, hi, 4);
    };
    let first = (lo_s / step).floor() as i64;
    let last = (hi_s / step).ceil() as i64;
    (first..=last).map(|i| i as f64 * step / scale).collect()
}

/// The labels a legend format names: `{{label}}`.
fn legend_labels(format: &str) -> Vec<String> {
    format
        .split("{{")
        .skip(1)
        .filter_map(|rest| rest.split_once("}}").map(|(l, _)| l.trim().to_owned()))
        .filter(|l| !l.is_empty())
        .collect()
}

/// The quantile of a `histogram_quantile(0.99, …)` query, as text.
fn quantile(query: &str) -> Option<String> {
    let rest = query.split("histogram_quantile(").nth(1)?;
    let q = rest.split(',').next()?.trim();
    q.parse::<f64>().ok().map(|_| q.to_owned())
}

/// How much to multiply evidence in `from` by to write it in the panel's unit.
fn conversion(from: Option<Unit>, panel: Option<&str>) -> f64 {
    let Some(panel) = panel else {
        return 1.0;
    };
    match (from, units::seconds_per(panel), units::percent_scale(panel)) {
        (Some(u), Some(panel_seconds), _) => seconds_of(u).map_or(1.0, |s| s / panel_seconds),
        (Some(Unit::Ratio), _, Some(scale)) => scale,
        _ => 1.0,
    }
}

/// A dashboard's named colour as CSS.
fn color_hex(name: &str) -> String {
    match name {
        "green" => "#73bf69",
        "dark-green" => "#37872d",
        "semi-dark-green" => "#56a64b",
        "light-green" => "#96d98d",
        "yellow" => "#fade2a",
        "dark-yellow" => "#e0b400",
        "semi-dark-yellow" => "#f2cc0c",
        "orange" => "#ff9830",
        "dark-orange" => "#fa6400",
        "semi-dark-orange" => "#ff780a",
        "red" => "#f2495c",
        "dark-red" => "#c4162a",
        "semi-dark-red" => "#e02f44",
        "light-red" => "#ff7383",
        "blue" => "#5794f2",
        "dark-blue" => "#1f60c4",
        "purple" => "#b877d9",
        "dark-purple" => "#8f3bb8",
        "text" | "gray" | "grey" => "#8e8e8e",
        other => return other.to_owned(),
    }
    .to_owned()
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
