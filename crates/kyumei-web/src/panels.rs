//! Dashboard panels opened in Kyumei: a panel's queries read live through Collect
//! now's endpoint and drawn as the panel draws them, inside an investigation or
//! on their own.

use std::collections::BTreeMap;

use jiff::Timestamp;
use kyumei_app::{
    EvidenceSubmission,
    dashboards::GrafanaLinks,
    panels::{self, PanelPlan, PanelRange},
};
use kyumei_dashboards::Entry;
use kyumei_domain::{
    DeclaredContext, EvidenceForm, EvidenceOrigin, EvidenceRequestId,
    Investigation as DomainInvestigation, InvestigationId, SubmissionChannel,
};
use kyumei_knowledge::{KnowledgeBase, Pack, ReferenceDef, ReferenceTarget};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, not_found, see_other},
        page, path_param,
        request::uri,
        route,
    },
    view::{Unescaped, View, component, view},
};

use crate::{
    components::code_block,
    pages::{service_error, state, submit, urlencode},
    preferences::zone,
};

path_param!(investigation);
path_param!(dashboard);
path_param!(panel);

/// A panel on its own, over a recent range.
#[page("/(app)/panels/{dashboard}/{panel}")]
pub async fn panel_page(cx: &Cx) -> Result<impl View> {
    let page = load(cx, None).await?;
    Ok(view! { panel_view(page: &page) })
}

/// A panel for an investigation: its context fills the variables, and its
/// incident window is the default range.
#[page("/(app)/investigations/{investigation}/panels/{dashboard}/{panel}")]
pub async fn investigation_panel_page(cx: &Cx) -> Result<impl View> {
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let page = load(cx, Some(id)).await?;
    Ok(view! { panel_view(page: &page) })
}

/// What the page found when it read the panel.
#[derive(Debug)]
enum Reading {
    /// The panel's dashboard is not loaded.
    NotLoaded,
    /// Variables nothing fills.
    Unfilled(Vec<String>),
    /// No metrics endpoint is configured.
    NoEndpoint,
    Failed(String),
    Drawn {
        svg: String,
        text: String,
        grafana_url: Option<String>,
        series: usize,
    },
}

/// Everything the panel page shows.
#[derive(Debug)]
pub struct PanelPage {
    back: Option<(String, String)>,
    title: String,
    panel_id: i64,
    dashboard_title: String,
    cited: Option<(Option<String>, String)>,
    reference: Option<ReferenceDef>,
    error: Option<String>,
    plan: Option<PanelPlan>,
    has_incident: bool,
    reading: Reading,
    read_note: Option<String>,
    command: Option<String>,
    evidence_action: Option<String>,
    answers: Option<(EvidenceRequestId, String)>,
}

async fn load(cx: &Cx, investigation: Option<InvestigationId>) -> Result<PanelPage> {
    let st = state(cx).clone();
    let z = zone(cx);
    let uid = path_param::<Dashboard>(cx).to_owned();
    let panel_id: i64 = path_param::<Panel>(cx)
        .parse()
        .map_err(|_| topcoat::Error::from(not_found()))?;
    let query = query_pairs(uri(cx).query().unwrap_or_default());
    let snapshot = match &investigation {
        Some(id) => Some(st.service.snapshot(id).await.map_err(service_error)?),
        None => None,
    };
    let inv = snapshot.as_ref().map(|s| &s.investigation);
    let pack = pack_for(&st.kb, inv).ok_or_else(|| topcoat::Error::msg("no knowledge loaded"))?;
    let reference = panels::reference(pack, &uid, panel_id).cloned();
    let found: Option<(Entry, kyumei_dashboards::Panel)> = {
        let library = st.library();
        panels::find(&library, &uid, panel_id).map(|(e, p)| (e.clone(), p.clone()))
    };
    let range = query.get("range").and_then(|r| PanelRange::parse(r));
    let error = query.get("error").cloned();
    let chosen: BTreeMap<String, String> = query
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "range" | "error"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let now = Timestamp::now();
    let plan = found
        .as_ref()
        .map(|(e, p)| panels::plan(e, p, inv, range, &chosen, now));
    let links = match inv {
        Some(i) => GrafanaLinks::for_investigation(st.grafana_url.as_deref(), i),
        None => GrafanaLinks {
            base: st.grafana_url.clone(),
            ..GrafanaLinks::default()
        },
    };

    let reading = match (&found, &plan, &st.collector) {
        (None, _, _) | (_, None, _) => Reading::NotLoaded,
        (_, Some(plan), _) if !plan.unfilled.is_empty() => Reading::Unfilled(plan.unfilled.clone()),
        (_, _, None) => Reading::NoEndpoint,
        (Some((entry, panel)), Some(plan), Some(collector)) => {
            match collector
                .collect(
                    &plan.queries,
                    plan.sample_window(),
                    &format!("kyumei {}", env!("CARGO_PKG_VERSION")),
                    now.as_millisecond(),
                )
                .await
            {
                Err(e) => Reading::Failed(e.to_string()),
                Ok((samples, _)) => {
                    let series = samples.queries.iter().map(|q| q.series.len()).sum();
                    let c = panels::chart(entry, panel, plan, &samples, pack, &links, &z);
                    Reading::Drawn {
                        svg: c.svg,
                        text: c.text,
                        grafana_url: c.panel.grafana_url,
                        series,
                    }
                }
            }
        }
    };

    let title = found
        .as_ref()
        .map(|(_, p)| p.title.clone())
        .or_else(|| reference.as_ref().map(|r| r.title.clone()))
        .unwrap_or_else(|| format!("Panel {panel_id}"));
    let dashboard_title = found
        .as_ref()
        .map(|(e, _)| e.dashboard.title.clone())
        .or_else(|| {
            reference
                .as_ref()
                .and_then(|r| r.panel.as_ref().map(|p| p.dashboard_title.clone()))
        })
        .unwrap_or_else(|| uid.clone());
    let cited = reference.as_ref().map(|r| citation(pack, r));
    // Inside an investigation, what adding the reading as evidence would answer.
    let answers = inv
        .zip(reference.as_ref())
        .and_then(|(i, r)| answers(&st.kb, i, r));
    let back = investigation.as_ref().map(|id| {
        (
            format!("/investigations/{id}"),
            inv.map(|i| i.title.clone()).unwrap_or_default(),
        )
    });
    let has_incident = inv.is_some_and(|i| i.window.as_window().is_some());
    let read_note = plan.as_ref().map(|p| {
        format!(
            "Read from {} at {}, over {}, every {}.",
            st.collector
                .as_ref()
                .map_or_else(String::new, |c| c.endpoint().label.clone()),
            z.time(now),
            z.window(&p.window),
            p.step_text()
        )
    });
    let command = plan.as_ref().map(collect_command);
    let evidence_action = investigation.as_ref().map(|id| {
        format!(
            "/investigations/{id}/panels/{}/{panel_id}/evidence",
            urlencode(&uid)
        )
    });
    Ok(PanelPage {
        back,
        title,
        panel_id,
        dashboard_title,
        cited,
        reference,
        error,
        plan,
        has_incident,
        reading,
        read_note,
        command,
        evidence_action,
        answers,
    })
}

#[component]
pub async fn panel_view(page: &PanelPage) -> Result<impl View> {
    let PanelPage {
        back,
        title,
        panel_id,
        dashboard_title,
        cited,
        reference,
        error,
        plan,
        has_incident,
        reading,
        read_note,
        command,
        evidence_action,
        answers,
    } = page;
    let has_incident = *has_incident;
    let queries: Vec<(String, String)> = plan
        .as_ref()
        .map(|p| {
            p.queries
                .iter()
                .enumerate()
                .map(|(i, q)| (format!("panel-query-{i}"), q.clone()))
                .collect()
        })
        .unwrap_or_default();
    Ok(view! {
        <div class="panel-page">
            <p class="crumbs">
                if let Some((href, t)) = &back {
                    <a href=(href.clone())>(t.clone())</a> " › "
                }
                "Panel"
            </p>
            <h1>(title.clone())</h1>
            <p class="fine muted">
                "Panel " (panel_id.to_string()) " of " (dashboard_title.clone())
                if let Some((url, text)) = &cited {
                    " · "
                    if let Some(u) = url { <a href=(u.clone()) rel="noreferrer">(text.clone())</a> } else { (text.clone()) }
                }
            </p>
            if let Some(r) = &reference {
                <p class="panel-why">(r.why.clone())</p>
            }
            if let Some(e) = &error {
                <p class="error" role="alert">(e.clone())</p>
            }
            if let Some(p) = &plan {
                <form method="get" class="panel-controls">
                    <label>"Range"
                        <select name="range">
                            for (token, label) in PanelRange::CHOICES {
                                if token != "incident" || has_incident {
                                    <option value=(token) selected=(p.range.token() == token)>(label)</option>
                                }
                            }
                        </select>
                    </label>
                    for v in &p.variables {
                        <label>(format!("${}", v.name))
                            if v.options.is_empty() {
                                <input name=(v.name.clone()) value=(v.value.clone().unwrap_or_default()) spellcheck="false">
                            } else {
                                <select name=(v.name.clone())>
                                    for o in &v.options {
                                        <option value=(o.clone()) selected=(v.value.as_deref() == Some(o.as_str()))>(o.clone())</option>
                                    }
                                </select>
                            }
                            if v.source == "investigation" {
                                <span class="fine muted">" from the investigation"</span>
                            }
                        </label>
                    }
                    <button type="submit" class="btn btn-secondary">"Show"</button>
                </form>
            }
            match &reading {
                Reading::NotLoaded => {
                    <section class="panel">
                        <p>"Kyumei hasn’t loaded the dashboard this panel is in"
                            if let Some(r) = &reference { ", " <code>(r.path.clone())</code> }
                            ". Start Kyumei with " <code>"--dashboards"</code> " pointing at your copy of the dashboards, or upload the file on "
                            <a href="/dashboards">"Dashboards"</a> "."</p>
                    </section>
                },
                Reading::Unfilled(names) => {
                    <p class="flag">"Fill in " (names.iter().map(|n| format!("${n}")).collect::<Vec<_>>().join(" and ")) " above to read the panel."</p>
                },
                Reading::NoEndpoint => {
                    <section class="panel">
                        <p>"Kyumei isn’t set up to read metrics. Start it with " <code>"--prometheus-url"</code> ", or "
                            <code>"--grafana-url"</code> " and " <code>"--grafana-datasource"</code> "; or run this where your metrics can be read:"</p>
                        if let Some(c) = &command {
                            code_block(id: "panel-command", body: c)
                        }
                    </section>
                },
                Reading::Failed(e) => {
                    <p class="error" role="alert">"Reading the panel failed: " (e.clone())</p>
                },
                Reading::Drawn { svg, text, grafana_url, series } => {
                    <figure class="chart panel-chart">
                        (Unescaped::new_unchecked(svg.clone()))
                        <figcaption class="fine chart-cite">
                            if let Some(n) = &read_note { (n.clone()) " " }
                            (series.to_string()) " series."
                            if let Some(g) = grafana_url {
                                " " <a href=(g.clone()) rel="noreferrer">"Open the panel in Grafana"</a>
                            }
                        </figcaption>
                        <details>
                            <summary class="fine">"Text version"</summary>
                            <pre class="raw">(text.clone())</pre>
                        </details>
                    </figure>
                    if let (Some(action), Some(p)) = (&evidence_action, &plan) {
                        <form method="post" action=(action.clone()) class="panel-evidence">
                            <input type="hidden" name="range" value=(p.range.token())>
                            for v in &p.variables {
                                if let Some(value) = &v.value {
                                    <input type="hidden" name=(v.name.clone()) value=(value.clone())>
                                }
                            }
                            <button type="submit" class="btn btn-primary">"Add as evidence"</button>
                            <span class="fine muted">
                                match &answers {
                                    Some((r, t)) => { "It answers the open request " <code>(r.to_string())</code> " " (t.clone()) "." },
                                    None => { "Kyumei reads it as evidence; it answers no open request." },
                                }
                            </span>
                        </form>
                    }
                },
            }
            if !queries.is_empty() {
                <details class="panel-queries">
                    <summary class="fine">"Queries as run"</summary>
                    for (qid, q) in &queries {
                        code_block(id: qid, body: q)
                    }
                </details>
            }
        </div>
    })
}

/// Read the panel again and add what comes back as evidence, citing the panel.
#[route(POST "/investigations/{investigation}/panels/{dashboard}/{panel}/evidence")]
pub async fn add_panel_evidence(
    cx: &Cx,
    Form(fields): Form<BTreeMap<String, String>>,
) -> Result<SeeOther> {
    let st = state(cx).clone();
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let uid = path_param::<Dashboard>(cx);
    let panel_id: i64 = path_param::<Panel>(cx)
        .parse()
        .map_err(|_| topcoat::Error::from(not_found()))?;
    let back = format!("/investigations/{id}/panels/{}/{panel_id}", urlencode(uid));
    let fail = |m: &str| see_other(format!("{back}?error={}", urlencode(m)));
    let Some(collector) = st.collector.clone() else {
        return Ok(fail("Kyumei is not set up to read metrics."));
    };
    let inv = st
        .service
        .snapshot(&id)
        .await
        .map_err(service_error)?
        .investigation;
    let Some((entry, panel)) = ({
        let library = st.library();
        panels::find(&library, uid, panel_id).map(|(e, p)| (e.clone(), p.clone()))
    }) else {
        return Ok(fail("the panel's dashboard is not loaded"));
    };
    let range = fields.get("range").and_then(|r| PanelRange::parse(r));
    let chosen: BTreeMap<String, String> = fields
        .into_iter()
        .filter(|(k, _)| !matches!(k.as_str(), "range" | "error"))
        .collect();
    let now = Timestamp::now();
    let plan = panels::plan(&entry, &panel, Some(&inv), range, &chosen, now);
    if !plan.unfilled.is_empty() {
        return Ok(fail("fill in the panel's variables first"));
    }
    let pack =
        pack_for(&st.kb, Some(&inv)).ok_or_else(|| topcoat::Error::msg("no knowledge loaded"))?;
    let answers = panels::reference(pack, uid, panel_id)
        .and_then(|r| answers(&st.kb, &inv, r))
        .map(|(r, _)| r);
    let (mut samples, warnings) = match collector
        .collect(
            &plan.queries,
            plan.sample_window(),
            &format!("kyumei {}", env!("CARGO_PKG_VERSION")),
            now.as_millisecond(),
        )
        .await
    {
        Ok(read) => read,
        Err(e) => return Ok(fail(&format!("Reading the panel failed: {e}"))),
    };
    samples.investigation = Some(id.to_string());
    samples.request = answers.as_ref().map(ToString::to_string);
    let mut note = format!(
        "Read as panel “{}” (id {}) of {}",
        panel.title, panel.id, entry.dashboard.title
    );
    if !warnings.is_empty() {
        note.push_str(&format!(
            "; {} warned: {}",
            collector.endpoint().label,
            warnings.join("; ")
        ));
    }
    let submission = EvidenceSubmission {
        declared: DeclaredContext {
            form: Some(EvidenceForm::SampleBlock),
            note: Some(note),
            ..DeclaredContext::default()
        },
        responds_to: answers,
        media_type: Some("text/plain".to_owned()),
        origin: Some(EvidenceOrigin::Collector {
            name: "kyumei".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }),
        ..EvidenceSubmission::text(
            SubmissionChannel::Collector,
            kyumei_samples::encode(&samples),
        )
    };
    submit(&st, &id, submission).await
}

/// The pack the investigation is assessed with, or the newest one.
fn pack_for<'a>(kb: &'a KnowledgeBase, inv: Option<&DomainInvestigation>) -> Option<&'a Pack> {
    match inv.and_then(|i| kyumei_diagnostics::select(kb, i).ok()) {
        Some((pack, _, _)) => Some(pack),
        None => kb.select(None).map(|s| s.pack),
    }
}

/// The open request this panel's reference links to, and its title.
fn answers(
    kb: &KnowledgeBase,
    inv: &DomainInvestigation,
    r: &ReferenceDef,
) -> Option<(EvidenceRequestId, String)> {
    let (_, _, d) = kyumei_diagnostics::select(kb, inv).ok()?;
    let current = inv.current_request()?;
    r.bears_on(&d?.id, ReferenceTarget::Request(&current.definition))
        .then(|| (current.id.clone(), current.title.clone()))
}

/// `tsurdilo/temporal-server-operations at 5a71c36, 9 October 2026`, linked to the
/// dashboard file at the pin.
fn citation(pack: &Pack, r: &ReferenceDef) -> (Option<String>, String) {
    let view = kyumei_app::dashboards::reference_view(pack, r, &GrafanaLinks::default());
    (view.url, view.cited)
}

/// The companion command that reads the panel's queries elsewhere.
fn collect_command(plan: &PanelPlan) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
    let mut parts = vec!["kyumei-collect".to_owned()];
    for q in &plan.queries {
        parts.push(format!("--query {}", quote(q)));
    }
    parts.push(format!("--from {}", plan.window.start));
    parts.push(format!("--to {}", plan.window.end));
    parts.push(format!("--step {}", plan.step_text()));
    parts.join(" \\\n  ")
}

/// A query string's pairs, decoded.
fn query_pairs(query: &str) -> BTreeMap<String, String> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            Some((decode(k)?, decode(v)?))
        })
        .collect()
}

fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}
