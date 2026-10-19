//! Pages and form handlers.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, OnceLock, RwLock},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{Stream, stream};
use jiff::{SignedDuration, Timestamp};
use kyumei_app::{
    Command, ContextInput, CorrectionInput, EvidenceSubmission, InvestigationService,
    OpenInvestigation, SdkInput, ServiceError, StructuredInput,
    collect::CollectionPlan,
    config::{Config, DEPLOYMENTS, SDK_LANGUAGES},
    core::CommandError,
    dashboards::{GrafanaLinks, evidence_charts},
    workspace,
};
use kyumei_collect::Collector;
use kyumei_dashboards::Library;
use kyumei_domain::{
    CorrectionChange, DeclaredContext, DeploymentKind, DisplayZone, EvidenceForm, EvidenceId,
    EvidenceOrigin, EvidenceRequestId, InvestigationId, ObservationId, RawBody, SignalId,
    Statistic, SubmissionChannel, TimeWindow, Topology, Unit,
};
use kyumei_knowledge::{KnowledgeBase, Pack};
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        HeaderName, HeaderValue, Slot, StatusCode,
        content::{
            Css, Form, Js,
            multipart::Multipart,
            sse::{Event, KeepAlive, Sse},
        },
        error::{RouterErrorExt, SeeOther, bad_request, not_found, see_other},
        header, layout, page, path_param, query_params, route,
    },
    view::{Unescaped, View, class, error_boundary, view},
};

use crate::{
    components::{evidence_ref, inline_code, panel},
    preferences::{zone, zone_selector},
    views::{
        assessment, diagnosis, diagrams, evidence_list, findings, next_evidence, requests,
        situation, trail,
    },
};

/// Shared application state.
pub struct AppState {
    pub kb: Arc<KnowledgeBase>,
    pub service: Arc<dyn InvestigationService>,
    pub mode: String,
    /// Prometheus or Grafana, when Kyumei may read metrics itself (Collect now).
    pub collector: Option<Arc<Collector>>,
    /// Dashboards evidence is drawn like: loaded at startup, and uploaded.
    pub dashboards: Arc<RwLock<Library>>,
    /// Where uploaded dashboards are kept, so that they load again at startup.
    pub dashboards_dir: Option<PathBuf>,
    /// The operator's Grafana, for links to panels.
    pub grafana_url: Option<String>,
    /// Preferred time zones and environment profiles.
    pub config: Arc<Config>,
    /// The zones the selector offers, worked out on first use.
    pub zone_choices: OnceLock<Vec<(String, Vec<DisplayZone>)>>,
}

impl AppState {
    /// State with no collector, no dashboards and no Grafana.
    pub fn new(
        kb: Arc<KnowledgeBase>,
        service: Arc<dyn InvestigationService>,
        mode: String,
    ) -> Self {
        Self {
            kb,
            service,
            mode,
            collector: None,
            dashboards: Arc::new(RwLock::new(Library::default())),
            dashboards_dir: None,
            grafana_url: None,
            config: Arc::new(Config::default()),
            zone_choices: OnceLock::new(),
        }
    }

    /// The zones the selector offers, UTC first, grouped by region.
    pub fn zone_choices(&self) -> &[(String, Vec<DisplayZone>)] {
        self.zone_choices
            .get_or_init(|| self.config.time_zones.offered())
    }

    /// The loaded dashboards, even if a writer panicked while holding them.
    pub fn library(&self) -> std::sync::RwLockReadGuard<'_, Library> {
        self.dashboards
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("mode", &self.mode)
            .field(
                "collector",
                &self.collector.as_ref().map(|c| &c.endpoint().label),
            )
            .finish_non_exhaustive()
    }
}

pub fn state(cx: &Cx) -> &Arc<AppState> {
    app_context::<Arc<AppState>>(cx)
}

path_param!(investigation);
path_param!(evidence);
path_param!(request);
path_param!(knowledge_claim);

#[query_params(error = bad_request)]
#[derive(Debug)]
pub struct WorkspaceQuery {
    received: Option<String>,
    error: Option<String>,
    /// On the landing page: the environment profile to fill the form from.
    profile: Option<String>,
}

pub(crate) fn service_error(e: ServiceError) -> topcoat::Error {
    match e {
        ServiceError::NotFound(_) => not_found().into(),
        other => topcoat::Error::msg(other.to_string()),
    }
}

// ------------------------------------------------------------------ layout

#[layout("/(app)")]
pub async fn app_layout(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    let mode = state(cx).mode.clone();
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"Kyumei"</title>
                <link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E%3Ccircle cx='8' cy='8' r='6' fill='none' stroke='%239a4b00' stroke-width='2'/%3E%3C/svg%3E">
                <link rel="stylesheet" href="/app.css">
                <script src="/app.js" defer=""></script>
            </head>
            <body>
                <header class="topbar">
                    <a class="brand" href="/"><span class="brand-word">"Kyumei"</span> <span class="brand-kanji" lang="ja">"究明"</span></a>
                    <nav>
                        <a href="/">"Investigations"</a>
                        <a href="/knowledge">"Knowledge"</a>
                        <a href="/dashboards">"Dashboards"</a>
                    </nav>
                    zone_selector()
                </header>
                <main>
                    error_boundary(
                        fallback: |error| {
                            if error.downcast_ref::<topcoat::router::error::NotFoundError>().is_none() {
                                return Err(error);
                            }
                            Ok(view! {
                                (StatusCode::NOT_FOUND)
                                <section class="panel"><h2>"Not found"</h2><p>"There is nothing here. " <a href="/">"Back to investigations"</a></p></section>
                            })
                        },
                        (slot)
                    )
                </main>
                <footer class="footer">"Kyumei — evidence-driven diagnostics for Temporal. " (mode)</footer>
            </body>
        </html>
    })
}

// ---------------------------------------------------------------- landing

/// Every symptom Kyumei has a diagnostic for, as `(id, title)`: each once, as the
/// newest pack that has it names it.
fn symptom_options(kb: &KnowledgeBase) -> Vec<(String, String)> {
    let mut options: Vec<(String, String)> = Vec::new();
    for p in packs_newest_first(kb) {
        for (_, s) in p.symptoms() {
            if !options.iter().any(|(id, _)| *id == s.id.to_string()) {
                options.push((s.id.to_string(), s.title.clone()));
            }
        }
    }
    options
}

/// The packs, the newest minor release first.
fn packs_newest_first(kb: &KnowledgeBase) -> Vec<&Pack> {
    let mut packs: Vec<&Pack> = kb.packs().iter().collect();
    packs.sort_by_key(|p| std::cmp::Reverse(p.family()));
    packs
}

/// The pack a page shows: the one whose ID is asked for, or the newest.
fn shown_pack<'k>(kb: &'k KnowledgeBase, asked: Option<&str>) -> Option<&'k Pack> {
    match asked.filter(|a| !a.is_empty()) {
        Some(id) => kb.packs().iter().find(|p| p.manifest.id.as_str() == id),
        None => kb.select(None).map(|s| s.pack),
    }
}

/// Each pack's minor release with a link to the same page for it, and whether it is
/// the one shown, newest first; empty when there is only one pack.
fn pack_choices(
    kb: &KnowledgeBase,
    shown: &Pack,
    href: impl Fn(&Pack) -> String,
) -> Vec<(String, String, bool)> {
    if kb.packs().len() < 2 {
        return Vec::new();
    }
    packs_newest_first(kb)
        .into_iter()
        .map(|p| {
            (
                p.manifest.server_family.clone(),
                href(p),
                p.manifest.id == shown.manifest.id,
            )
        })
        .collect()
}

#[page("/(app)")]
pub async fn landing(cx: &Cx) -> Result<impl View> {
    let st = state(cx).clone();
    let list = st.service.list().await.map_err(service_error)?;
    let symptoms = symptom_options(&st.kb);
    let q = query_params::<WorkspaceQuery>(cx)?;
    let error = q.error.clone();
    let z = zone(cx);
    let now = Timestamp::now();
    let zone_label = z.label(now);
    // The window starts as the last hour; a profile fills the context.
    let from = z.input_value(
        now.checked_sub(SignedDuration::from_hours(1))
            .unwrap_or(now),
    );
    let to = z.input_value(now);
    let profiles = st.config.profiles.clone();
    let chosen = q
        .profile
        .as_deref()
        .and_then(|n| st.config.profile(n))
        .cloned();
    let p = chosen.clone().unwrap_or_default();
    let count = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
    let deployment = p.deployment_value();
    let placeholder = format!(
        "e.g. Since about 13:35 {zone_label}, workflow tasks on order-fulfillment wait ~30 seconds before a worker picks them up. A batch job started signalling order workflows at the same time."
    );
    Ok(view! {
        <section class="landing">
            <form method="post" action="/investigations" class="open-form" id="new">
                <h1><label for="description">"What are you seeing?"</label></h1>
                if let Some(e) = &error {
                    <p class="error" role="alert">(e.clone())</p>
                }
                <textarea id="description" name="description" rows="5" required="" placeholder=(placeholder)></textarea>
                <p class="fine muted">"Describe it in your own words. Leave anything you don’t know blank: unknown stays unknown."</p>
                if !profiles.is_empty() {
                    <div class="profiles" role="group" aria-label="Environment profile">
                        <span class="profiles-label">"Environment"</span>
                        <a href="/#new" class=(class!("profile-link", "current" if chosen.is_none()))>"none"</a>
                        for pr in &profiles {
                            <a href=(format!("/?profile={}#new", urlencode(&pr.name)))
                                class=(class!("profile-link", "current" if chosen.as_ref().is_some_and(|c| c.name == pr.name)))
                                title=(pr.description.clone().unwrap_or_default())>(pr.name.clone())</a>
                        }
                    </div>
                    if let Some(c) = &chosen {
                        <input type="hidden" name="profile" value=(c.name.clone())>
                        <p class="fine muted">"Context filled from the " <b>(c.name.clone())</b> " profile. Change anything that differs; what you submit is recorded as your assertion."</p>
                    }
                }
                <details class="context" open="">
                    <summary>"Context you already know"</summary>
                    <div class="grid">
                        <label>"Temporal Server version"<input name="server_version" placeholder="1.31.0" value=(p.server_version.clone().unwrap_or_default())></label>
                        <label>"Worker SDK"
                            <select name="sdk_language">
                                <option value="">"unknown"</option>
                                for (v, l) in SDK_LANGUAGES {
                                    <option value=(v) selected=(p.sdk_language.as_deref() == Some(v))>(l)</option>
                                }
                            </select>
                        </label>
                        <label>"SDK version"<input name="sdk_version" placeholder="e.g. 1.25.0" value=(p.sdk_version.clone().unwrap_or_default())></label>
                        <label>"Namespace"<input name="namespace" spellcheck="false" value=(p.namespace.clone().unwrap_or_default())></label>
                        <label>"Task Queue"<input name="task_queue" spellcheck="false" value=(p.task_queue.clone().unwrap_or_default())></label>
                        <label>"Incident from (" (zone_label.clone()) ")"<input type="datetime-local" name="window_start" value=(from)></label>
                        <label>"Incident to (" (zone_label.clone()) ")"<input type="datetime-local" name="window_end" value=(to)></label>
                        <label>"Deployment"
                            <select name="deployment">
                                <option value="">"unknown"</option>
                                for (v, l, _) in DEPLOYMENTS {
                                    <option value=(v) selected=(deployment == Some(v))>(l)</option>
                                }
                            </select>
                        </label>
                        <label>"Frontend hosts"<input name="frontend_hosts" inputmode="numeric" value=(count(p.frontend_hosts))></label>
                        <label>"History hosts"<input name="history_hosts" inputmode="numeric" value=(count(p.history_hosts))></label>
                        <label>"Matching hosts"<input name="matching_hosts" inputmode="numeric" value=(count(p.matching_hosts))></label>
                        <label>"Worker processes"<input name="worker_processes" inputmode="numeric" value=(count(p.worker_processes))></label>
                        <label>"Persistence"<input name="persistence" placeholder="e.g. PostgreSQL 16" value=(p.persistence.clone().unwrap_or_default())></label>
                        <label>"Symptom"
                            <select name="signature">
                                <option value="">"recognise from my description"</option>
                                for (id, title) in &symptoms {
                                    <option value=(id.clone())>(title.clone())</option>
                                }
                            </select>
                        </label>
                    </div>
                    <p class="fine muted">"The incident window starts as the last hour. Move it to when the symptom began and ended, or clear it if you don’t know."</p>
                </details>
                <div class="actions"><button type="submit" class="btn btn-primary">"Start investigation"</button></div>
            </form>
            panel(title: "Investigations", id: Some("investigations"),
                if list.is_empty() {
                    <p class="muted">"None yet."</p>
                }
                <table class="list">
                    <tbody>
                        for i in &list {
                            <tr>
                                <td><a href=(("/investigations/", i.id.to_string()))>(i.title.clone())</a>
                                    <div class="fine muted"><code>(i.id.to_string())</code></div></td>
                                <td class="fine">(i.status.label())
                                    if let Some(c) = &i.conclusion { <div>(c.clone())</div> }</td>
                                <td class="fine muted">(i.evidence) " evidence · updated " (z.date_time(i.updated_at))</td>
                            </tr>
                        }
                    </tbody>
                </table>
            )
        </section>
    })
}

#[derive(Debug, Deserialize)]
pub struct OpenForm {
    description: String,
    title: Option<String>,
    signature: Option<String>,
    server_version: Option<String>,
    sdk_language: Option<String>,
    sdk_version: Option<String>,
    namespace: Option<String>,
    task_queue: Option<String>,
    window_start: Option<String>,
    window_end: Option<String>,
    deployment: Option<String>,
    frontend_hosts: Option<String>,
    history_hosts: Option<String>,
    matching_hosts: Option<String>,
    worker_processes: Option<String>,
    persistence: Option<String>,
    profile: Option<String>,
}

/// A window field, unless it still shows `current` as the form wrote it.
fn changed<'a>(
    field: Option<&'a str>,
    current: Option<Timestamp>,
    zone: &DisplayZone,
) -> Option<&'a str> {
    field.filter(|v| Some(v.trim().to_owned()) != current.map(|t| zone.input_value(t)))
}

/// A time typed into a form: a `datetime-local` value, read in `zone`, or a
/// time written with its own offset.
fn time(field: Option<&str>, zone: &DisplayZone) -> std::result::Result<Option<Timestamp>, String> {
    match field.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => zone
            .parse_input(s)
            .ok()
            .or_else(|| kyumei_evidence::timestamps::parse_timestamp(s))
            .map(Some)
            .ok_or_else(|| format!("“{s}” is not a time Kyumei understands")),
    }
}

fn count(field: Option<&str>) -> Option<u32> {
    field.and_then(|s| s.trim().parse().ok())
}

fn topology(f: &OpenForm) -> Option<Topology> {
    let deployment = match f.deployment.as_deref() {
        Some("self_hosted_kubernetes") => DeploymentKind::SelfHostedKubernetes,
        Some("self_hosted_other") => DeploymentKind::SelfHostedOther,
        Some("temporal_cloud") => DeploymentKind::TemporalCloud,
        _ => DeploymentKind::Unknown,
    };
    let t = Topology {
        deployment,
        frontend_hosts: count(f.frontend_hosts.as_deref()),
        history_hosts: count(f.history_hosts.as_deref()),
        matching_hosts: count(f.matching_hosts.as_deref()),
        worker_processes: count(f.worker_processes.as_deref()),
        persistence: f.persistence.clone().filter(|p| !p.trim().is_empty()),
        notes: None,
    };
    (!t.is_empty()).then_some(t)
}

fn new_id() -> InvestigationId {
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    InvestigationId::new(format!("inv-{}", &uuid[..10]))
}

#[route(POST "/investigations")]
pub async fn create(cx: &Cx, Form(f): Form<OpenForm>) -> Result<SeeOther> {
    let st = state(cx).clone();
    let z = zone(cx);
    let fail = |m: String| see_other(format!("/?error={}", urlencode(&m)));
    let (start, end) = match (
        time(f.window_start.as_deref(), &z),
        time(f.window_end.as_deref(), &z),
    ) {
        (Ok(s), Ok(e)) => (s, e),
        (Err(m), _) | (_, Err(m)) => return Ok(fail(m)),
    };
    let cmd = OpenInvestigation {
        id: new_id(),
        title: f.title.clone(),
        description: f.description.clone(),
        signature: f
            .signature
            .clone()
            .filter(|s| !s.is_empty())
            .map(Into::into),
        server_version: f.server_version.clone(),
        sdks: f
            .sdk_language
            .clone()
            .filter(|l| !l.is_empty())
            .map(|language| SdkInput {
                language,
                version: f.sdk_version.clone().filter(|v| !v.trim().is_empty()),
            })
            .into_iter()
            .collect(),
        topology: topology(&f),
        namespaces: f.namespace.clone().into_iter().collect(),
        task_queues: f.task_queue.clone().into_iter().collect(),
        window_start: start,
        window_end: end,
        window_description: None,
        profile: f.profile.clone().filter(|p| st.config.profile(p).is_some()),
    };
    match st.service.open(cmd).await {
        Ok(id) => Ok(see_other(format!("/investigations/{id}"))),
        Err(ServiceError::Command(CommandError::Invalid(m))) => Ok(fail(m)),
        Err(e) => Err(service_error(e)),
    }
}

// --------------------------------------------------------------- workspace

#[page("/(app)/investigations/{investigation}")]
pub async fn workspace_page(cx: &Cx) -> Result<impl View> {
    let st = state(cx).clone();
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let snapshot = st.service.snapshot(&id).await.map_err(service_error)?;
    let current = kyumei_app::core::current_assessment(&st.kb, &snapshot.investigation)
        .map_err(|e| topcoat::Error::msg(e.to_string()))?;
    let z = zone(cx);
    let w = workspace::build_with(&st.kb, &snapshot, &current, st.grafana_url.as_deref(), &z);
    let q = query_params::<WorkspaceQuery>(cx)?;
    let window = &snapshot.investigation.window;
    let window_inputs = (
        window.start.map(|t| z.input_value(t)).unwrap_or_default(),
        window.end.map(|t| z.input_value(t)).unwrap_or_default(),
    );
    let received = q
        .received
        .clone()
        .and_then(|e| received_note(&snapshot.investigation, &st.kb, &w, &z, &e));
    let error = q.error.clone();
    let collector = st.collector.as_ref().map(|c| c.endpoint().label.clone());
    let version = version_of(&snapshot);
    let symptoms = symptom_options(&st.kb);
    // Without a recognised symptom there is nothing to investigate yet, so
    // the context form, where one is chosen, starts open.
    let unrecognised = w.situation.symptom_title.is_none();
    Ok(view! {
        let w = w;
        <div class="workspace" data-investigation=(w.id.clone()) data-version=(version)>
            <div class="ws-head">
                <h1>(w.title.clone())</h1>
                <span class=(class!("status", format!("status-{}", w.status.token))) role="status">(w.status.label.clone())</span>
                <a class="btn btn-quiet" href=(format!("/investigations/{}/export.md", w.id))>"Export Markdown"</a>
                <details class="edit-context" id="edit-context" open=(unrecognised)>
                    <summary class="btn btn-quiet">"Edit context"</summary>
                    context_form(w: &w, symptoms: &symptoms, window: &window_inputs)
                </details>
            </div>
            <div class="live-banner" hidden="">"This investigation changed. " <a href="">"Reload"</a></div>
            if let Some(note) = &w.runtime_note {
                <div class="notice" role="status">(note.clone())</div>
            }
            if let Some(r) = &received {
                <div class="notice notice-received" role="status">
                    (r.text.clone()) " "
                    <a href=(format!("/investigations/{}/evidence/{}", w.id, r.evidence))>"Open " (r.evidence.clone())</a>
                </div>
            }
            situation(w: &w)
            <div class="columns">
                <div class="primary">
                    diagnosis(w: &w)
                    next_evidence(w: &w, error: error.as_deref(), collector: collector.as_deref())
                    assessment(w: &w)
                    findings(w: &w)
                    if !w.diagrams.is_empty() {
                        diagrams(w: &w)
                    }
                </div>
                <aside class="secondary">
                    trail(rows: &w.trail, investigation: &w.id, zone: &w.zone.label)
                    evidence_list(w: &w)
                    requests(w: &w)
                    <p class="fine muted runtime">"Runtime: " (w.runtime.clone()) " · Explanation: " (w.explanation.author.clone())</p>
                </aside>
            </div>
        </div>
    })
}

fn version_of(s: &kyumei_app::InvestigationSnapshot) -> String {
    format!("{}-{:?}", s.investigation.events.len(), s.explanation)
}

/// A notice about evidence just received.
struct Received {
    evidence: String,
    text: String,
}

/// What a just-submitted piece of evidence held, what Kyumei now knows from it,
/// what it changed, and what comes next.
fn received_note(
    inv: &kyumei_domain::Investigation,
    kb: &KnowledgeBase,
    w: &workspace::Workspace,
    zone: &DisplayZone,
    evidence: &str,
) -> Option<Received> {
    let e = inv.evidence(&EvidenceId::new(evidence))?;
    let (_, _, d) = kyumei_diagnostics::select(kb, inv).ok()?;
    let seq = inv
        .events
        .iter()
        .find(|ev| matches!(&ev.event, kyumei_domain::InvestigationEvent::EvidenceSubmitted(x) if x.id == e.id))?
        .seq;
    let changes: Vec<String> = inv
        .assessments
        .iter()
        .filter(|(s, _)| *s > seq)
        .take(1)
        .flat_map(|(_, a)| a.transitions.iter())
        .filter(|t| t.from.is_some())
        .map(|t| {
            let title = d
                .and_then(|d| d.hypothesis(&t.hypothesis))
                .map_or(t.hypothesis.to_string(), |h| h.title.clone());
            format!("{title}: {}", t.to.label())
        })
        .collect();
    // What it held: series and their samples, over what time.
    let series: Vec<&kyumei_domain::Observation> = e
        .interpretation
        .observations
        .iter()
        .filter(|o| matches!(o.value, kyumei_domain::ObservedValue::Series { .. }))
        .collect();
    let samples: usize = series
        .iter()
        .map(|o| match &o.value {
            kyumei_domain::ObservedValue::Series { points, .. } => points.len(),
            _ => 0,
        })
        .sum();
    let covers = series
        .iter()
        .filter_map(|o| o.window)
        .reduce(|a, b| a.union(&b));
    let source = match &e.origin {
        EvidenceOrigin::Collector { name, .. } if name == "kyumei" => {
            "read by Collect now".to_owned()
        }
        other => format!("from {}", other.describe()),
    };
    let mut first = format!("{} received, {source}", e.id);
    if !series.is_empty() {
        first.push_str(&format!(
            ": {} series, {samples} {}",
            series.len(),
            if samples == 1 { "sample" } else { "samples" }
        ));
        if let Some(window) = covers {
            first.push_str(&format!(", {}", zone.window(&window)));
        }
    }
    if let Some(r) = &e.responds_to {
        first.push_str(&format!(", answering {r}"));
    }
    let mut sentences = vec![first];
    // What Kyumei now knows from it.
    for f in w
        .assessment
        .knows
        .iter()
        .filter(|f| f.evidence.iter().any(|x| x == evidence))
    {
        sentences.push(format!("{}: {}", f.title, f.summary));
    }
    if changes.is_empty() {
        let bound = e
            .interpretation
            .observations
            .iter()
            .filter(|o| o.binding.is_some())
            .count();
        sentences.push(if bound == 0 {
            "It relates to none of the diagnostic's signals, so no hypothesis changed".to_owned()
        } else {
            "No hypothesis changed".to_owned()
        });
    } else {
        sentences.push(changes.join("; "));
    }
    match &w.next {
        Some(n) => sentences.push(format!("Next: {} {}", n.request, n.title)),
        None if w.assessment.conclusion.is_some() => {
            sentences.push("No further evidence is needed".to_owned());
        }
        None => {}
    }
    Some(Received {
        evidence: e.id.to_string(),
        text: format!("{}.", sentences.join(". ")),
    })
}

#[topcoat::view::component]
pub async fn context_form(
    w: &workspace::Workspace,
    symptoms: &[(String, String)],
    window: &(String, String),
) -> Result<impl View> {
    let zone_label = w.zone.label.clone();
    let get = |label: &str| {
        w.situation
            .scope
            .iter()
            .find(|(k, _)| k == label)
            .map(|(_, v)| v.clone())
            .filter(|v| v != "unknown")
            .unwrap_or_default()
    };
    let version = w
        .situation
        .environment
        .iter()
        .find(|e| e.label == "Temporal Server")
        .map(|e| e.value.clone())
        .filter(|v| v != "unknown")
        .unwrap_or_default();
    Ok(view! {
        <form method="post" action=(format!("/investigations/{}/context", w.id)) class="context-form">
            <div class="grid">
                <label>"Temporal Server version"<input name="server_version" value=(version)></label>
                <label>"Namespace"<input name="namespace" value=(get("Namespace"))></label>
                <label>"Task Queue"<input name="task_queue" value=(get("Task Queue"))></label>
                <label>"Worker SDK"
                    <select name="sdk_language">
                        <option value="">"unchanged"</option>
                        for (v, l) in [("go", "Go"), ("java", "Java"), ("typescript", "TypeScript"), ("python", "Python"), ("dotnet", ".NET"), ("ruby", "Ruby"), ("rust", "Rust")] {
                            <option value=(v)>(l)</option>
                        }
                    </select>
                </label>
                <label>"SDK version"<input name="sdk_version"></label>
                <label>"Incident from (" (zone_label.clone()) ")"<input type="datetime-local" name="window_start" value=(window.0.clone())></label>
                <label>"Incident to (" (zone_label.clone()) ")"<input type="datetime-local" name="window_end" value=(window.1.clone())></label>
                <label>"Symptom"
                    <select name="signature">
                        <option value="">"unchanged"</option>
                        for (id, title) in symptoms {
                            <option value=(id.clone())>(title.clone())</option>
                        }
                    </select>
                </label>
            </div>
            <div class="actions"><button type="submit" class="btn btn-secondary">"Update context"</button></div>
        </form>
    })
}

#[derive(Debug, Deserialize)]
pub struct ContextForm {
    server_version: Option<String>,
    namespace: Option<String>,
    task_queue: Option<String>,
    sdk_language: Option<String>,
    sdk_version: Option<String>,
    window_start: Option<String>,
    window_end: Option<String>,
    signature: Option<String>,
}

#[route(POST "/investigations/{investigation}/context")]
pub async fn amend_context(cx: &Cx, Form(f): Form<ContextForm>) -> Result<SeeOther> {
    let st = state(cx).clone();
    let z = zone(cx);
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let back = |q: String| see_other(format!("/investigations/{id}{q}"));
    // The form shows the current window; a field left as shown is unchanged,
    // even though it is written to the minute.
    let current = st
        .service
        .snapshot(&id)
        .await
        .map_err(service_error)?
        .investigation
        .window;
    let (start, end) = match (
        time(changed(f.window_start.as_deref(), current.start, &z), &z),
        time(changed(f.window_end.as_deref(), current.end, &z), &z),
    ) {
        (Ok(s), Ok(e)) => (s, e),
        (Err(m), _) | (_, Err(m)) => return Ok(back(format!("?error={}", urlencode(&m)))),
    };
    let input = ContextInput {
        server_version: f.server_version.clone(),
        sdks: f
            .sdk_language
            .clone()
            .filter(|l| !l.is_empty())
            .map(|language| {
                vec![SdkInput {
                    language,
                    version: f.sdk_version.clone(),
                }]
            }),
        topology: None,
        namespaces: f.namespace.clone().map(|n| vec![n]),
        task_queues: f.task_queue.clone().map(|t| vec![t]),
        window_start: start,
        window_end: end,
        signature: f
            .signature
            .clone()
            .filter(|s| !s.is_empty())
            .map(Into::into),
    };
    match st.service.command(&id, Command::AmendContext(input)).await {
        Ok(_) => Ok(back(String::new())),
        Err(ServiceError::Command(CommandError::Invalid(m))) => {
            Ok(back(format!("?error={}", urlencode(&m))))
        }
        Err(e) => Err(service_error(e)),
    }
}

// ---------------------------------------------------------------- evidence

#[derive(Debug, Deserialize)]
pub struct EvidenceFormInput {
    mode: String,
    request: Option<String>,
    text: Option<String>,
    query: Option<String>,
    form: Option<String>,
    unit: Option<String>,
    window_start: Option<String>,
    window_end: Option<String>,
    signal: Option<String>,
    statistic: Option<String>,
    value: Option<String>,
    note: Option<String>,
}

fn declared(
    form: Option<&str>,
    unit: Option<&str>,
    query: Option<&str>,
    window: (Option<&str>, Option<&str>),
    note: Option<&str>,
    zone: &DisplayZone,
) -> std::result::Result<DeclaredContext, String> {
    let start = time(window.0, zone)?;
    let end = time(window.1, zone)?;
    let window = match (start, end) {
        (Some(s), Some(e)) => Some(TimeWindow::new(s, e).map_err(|e| e.to_string())?),
        (Some(s), None) | (None, Some(s)) => Some(TimeWindow { start: s, end: s }),
        _ => None,
    };
    Ok(DeclaredContext {
        form: form.and_then(EvidenceForm::from_token),
        window,
        query: query
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(str::to_owned),
        signal: None,
        unit: unit
            .filter(|u| !u.is_empty())
            .and_then(|u| u.parse::<Unit>().ok()),
        labels: Default::default(),
        note: note
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned),
    })
}

pub(crate) async fn submit(
    st: &AppState,
    id: &InvestigationId,
    submission: EvidenceSubmission,
) -> Result<SeeOther> {
    match st
        .service
        .command(id, Command::SubmitEvidence(submission))
        .await
    {
        Ok(r) => Ok(see_other(match r.evidence {
            Some(e) => format!("/investigations/{id}?received={e}"),
            None => format!("/investigations/{id}"),
        })),
        Err(ServiceError::Command(CommandError::Invalid(m))) => Ok(see_other(format!(
            "/investigations/{id}?error={}#next",
            urlencode(&m)
        ))),
        Err(e) => Err(service_error(e)),
    }
}

#[route(POST "/investigations/{investigation}/evidence")]
pub async fn add_evidence(cx: &Cx, Form(f): Form<EvidenceFormInput>) -> Result<SeeOther> {
    let st = state(cx).clone();
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let back_with =
        |m: String| see_other(format!("/investigations/{id}?error={}#next", urlencode(&m)));
    let mut declared = match declared(
        f.form.as_deref(),
        f.unit.as_deref(),
        f.query.as_deref(),
        (f.window_start.as_deref(), f.window_end.as_deref()),
        f.note.as_deref(),
        &zone(cx),
    ) {
        Ok(d) => d,
        Err(m) => return Ok(back_with(m)),
    };
    let request = f.request.clone().filter(|r| !r.is_empty()).map(Into::into);
    let submission = match f.mode.as_str() {
        "structured" => {
            let Some(value) = f
                .value
                .as_deref()
                .and_then(|v| v.trim().replace(',', "").parse::<f64>().ok())
                .filter(|v| v.is_finite())
            else {
                return Ok(back_with("enter a number".to_owned()));
            };
            let signal = f.signal.clone().unwrap_or_default();
            declared.signal = Some(SignalId::new(&signal));
            EvidenceSubmission {
                channel: SubmissionChannel::StructuredEntry,
                text: None,
                bytes_base64: None,
                filename: None,
                media_type: Some("application/json".to_owned()),
                declared: declared.clone(),
                responds_to: request,
                structured: Some(StructuredInput {
                    signal,
                    statistic: f
                        .statistic
                        .as_deref()
                        .and_then(|s| s.parse::<Statistic>().ok())
                        .unwrap_or(Statistic::Typical),
                    value,
                    unit: declared.unit,
                }),
                origin: None,
                stored: None,
            }
        }
        "observation" => EvidenceSubmission {
            declared,
            responds_to: request,
            ..EvidenceSubmission::text(
                SubmissionChannel::Observation,
                f.text.clone().unwrap_or_default(),
            )
        },
        _ => {
            let text = f.text.clone().unwrap_or_default();
            if text.trim().is_empty() {
                return Ok(back_with("paste something first".to_owned()));
            }
            EvidenceSubmission {
                declared,
                responds_to: request,
                ..EvidenceSubmission::text(SubmissionChannel::Paste, text)
            }
        }
    };
    submit(&st, &id, submission).await
}

#[route(POST "/investigations/{investigation}/evidence/upload")]
pub async fn upload_evidence(cx: &Cx, mut multipart: Multipart) -> Result<SeeOther> {
    let st = state(cx).clone();
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let mut file: Option<(Option<String>, Option<String>, Vec<u8>)> = None;
    let mut fields = BTreeMap::<String, String>::new();
    while let Some(field) = multipart.next_field().await? {
        let name = field.name().unwrap_or_default().to_owned();
        if name == "file" {
            let filename = field.file_name().map(str::to_owned);
            let content_type = field.content_type().map(str::to_owned);
            let bytes = field.bytes().await?;
            file = Some((filename, content_type, bytes.to_vec()));
        } else {
            let text = field.text().await?;
            fields.insert(name, text);
        }
    }
    let Some((filename, media_type, bytes)) = file.filter(|(_, _, b)| !b.is_empty()) else {
        return Ok(see_other(format!(
            "/investigations/{id}?error={}#next",
            urlencode("choose a file to upload")
        )));
    };
    let g = |k: &str| fields.get(k).map(String::as_str);
    let declared = match declared(
        g("form"),
        g("unit"),
        g("query"),
        (g("window_start"), g("window_end")),
        g("note"),
        &zone(cx),
    ) {
        Ok(d) => d,
        Err(m) => {
            return Ok(see_other(format!(
                "/investigations/{id}?error={}#next",
                urlencode(&m)
            )));
        }
    };
    let (text, bytes_base64) = match String::from_utf8(bytes.clone()) {
        Ok(t)
            if !media_type
                .as_deref()
                .unwrap_or_default()
                .starts_with("image/") =>
        {
            (Some(t), None)
        }
        _ => (None, Some(STANDARD.encode(&bytes))),
    };
    let submission = EvidenceSubmission {
        channel: SubmissionChannel::Upload,
        text,
        bytes_base64,
        filename,
        media_type,
        declared,
        responds_to: g("request").filter(|r| !r.is_empty()).map(Into::into),
        structured: None,
        origin: None,
        stored: None,
    };
    submit(&st, &id, submission).await
}

#[derive(Debug, Deserialize)]
pub struct DeclineForm {
    reason: String,
}

#[route(POST "/investigations/{investigation}/requests/{request}/decline")]
pub async fn decline(cx: &Cx, Form(f): Form<DeclineForm>) -> Result<SeeOther> {
    let st = state(cx).clone();
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let request = path_param::<Request>(cx).to_owned();
    match st
        .service
        .command(
            &id,
            Command::DeclineRequest {
                request: request.into(),
                reason: f.reason,
            },
        )
        .await
    {
        Ok(_) => Ok(see_other(format!("/investigations/{id}"))),
        Err(ServiceError::Command(CommandError::Invalid(m))) => Ok(see_other(format!(
            "/investigations/{id}?error={}",
            urlencode(&m)
        ))),
        Err(e) => Err(service_error(e)),
    }
}

/// Collect now: read the request's PromQL queries from the configured
/// Prometheus or Grafana over the request's window, and add what comes back as
/// a sample block answering the request. It is the block `kyumei-collect`
/// would print, so it is interpreted, kept and replayed the same way.
#[route(POST "/investigations/{investigation}/requests/{request}/collect")]
pub async fn collect_now(cx: &Cx) -> Result<SeeOther> {
    let st = state(cx).clone();
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let request = EvidenceRequestId::new(path_param::<Request>(cx));
    let back_with =
        |m: &str| see_other(format!("/investigations/{id}?error={}#next", urlencode(m)));
    let Some(collector) = st.collector.clone() else {
        return Ok(back_with(
            "Kyumei is not set up to read metrics. Start it with --prometheus-url, or --grafana-url and --grafana-datasource; or run the kyumei-collect command and paste its block.",
        ));
    };
    let snapshot = st.service.snapshot(&id).await.map_err(service_error)?;
    let r = snapshot
        .investigation
        .requests
        .iter()
        .find(|r| r.id == request)
        .cloned()
        .ok_or_not_found()?;
    if !r.status.is_open() {
        return Ok(back_with(&format!(
            "Request {request} is no longer open: collect for the current request."
        )));
    }
    let Some(plan) = CollectionPlan::for_request(&r) else {
        return Ok(back_with(&format!(
            "Request {request} has no PromQL queries over a known window for Kyumei to read."
        )));
    };
    if let Some(missing) = plan.missing() {
        return Ok(back_with(&format!(
            "Collect now needs {missing} first: add {} under Edit context.",
            if plan.unfilled.len() == 1 {
                "it"
            } else {
                "them"
            },
        )));
    }
    let (mut samples, warnings) = match collector
        .collect(
            &plan.queries,
            plan.sample_window(),
            &format!("kyumei {}", env!("CARGO_PKG_VERSION")),
            Timestamp::now().as_millisecond(),
        )
        .await
    {
        Ok(collected) => collected,
        Err(e) => return Ok(back_with(&format!("Collect now did not finish: {e}"))),
    };
    samples.investigation = Some(id.to_string());
    samples.request = Some(request.to_string());
    let declared = DeclaredContext {
        form: Some(EvidenceForm::SampleBlock),
        note: (!warnings.is_empty()).then(|| {
            format!(
                "{} warned: {}",
                collector.endpoint().label,
                warnings.join("; ")
            )
        }),
        ..DeclaredContext::default()
    };
    let submission = EvidenceSubmission {
        declared,
        responds_to: Some(request),
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

#[query_params(error = bad_request)]
#[derive(Debug)]
pub struct DashboardsQuery {
    added: Option<String>,
    error: Option<String>,
    pack: Option<String>,
}

/// The dashboards evidence is drawn like, the panels Kyumei cannot read, and how the
/// panels' queries compare with what the knowledge says about the metrics they read.
#[page("/(app)/dashboards")]
pub async fn dashboards_page(cx: &Cx) -> Result<impl View> {
    let st = state(cx).clone();
    let q = query_params::<DashboardsQuery>(cx)?;
    let library = st.library().clone();
    let pack = shown_pack(&st.kb, q.pack.as_deref())
        .ok_or_not_found()?
        .clone();
    let checks = kyumei_app::dashboards::cross_check(&library, &pack);
    let choices = pack_choices(&st.kb, &pack, |p| {
        format!("/dashboards?pack={}#checks", p.manifest.id)
    });
    let pack_id = pack.manifest.id.to_string();
    let cited = |uid: &str| {
        pack.references
            .iter()
            .find(|r| r.panel.as_ref().is_some_and(|x| x.dashboard == uid))
            .map(|r| kyumei_app::dashboards::reference_view(&pack, r, &Default::default()))
    };
    struct Row {
        title: String,
        uid: String,
        version: Option<i64>,
        origin: String,
        charts: usize,
        panels: usize,
        unreadable: Vec<(Option<i64>, String, String)>,
        source: Option<kyumei_app::dashboards::ReferenceView>,
        findings: usize,
    }
    let rows: Vec<Row> = library
        .entries()
        .iter()
        .map(|e| Row {
            title: e.dashboard.title.clone(),
            uid: e.dashboard.uid.clone(),
            version: e.dashboard.version,
            origin: e.origin.describe(),
            charts: e.dashboard.panels.iter().filter(|p| p.is_chart()).count(),
            panels: e.dashboard.panels.len(),
            unreadable: e
                .dashboard
                .unreadable
                .iter()
                .map(|u| (u.id, u.title.clone(), u.reason.clone()))
                .collect(),
            source: cited(&e.dashboard.uid),
            findings: checks
                .iter()
                .filter(|f| f.dashboard == e.dashboard.uid)
                .count(),
        })
        .collect();
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for f in &checks {
        match kinds.iter_mut().find(|(k, _)| k == f.kind.label()) {
            Some((_, n)) => *n += 1,
            None => kinds.push((f.kind.label().to_owned(), 1)),
        }
    }
    let can_keep = st.dashboards_dir.is_some();
    Ok(view! {
        <div class="dashboards-page">
            <h1>"Dashboards"</h1>
            <p class="lede">"Kyumei draws evidence the way the panel that reads the same metric draws it: its legend, unit, decimals and thresholds. The dashboards are yours: a directory given with " <code>"--dashboards"</code> ", or dashboards uploaded here. Kyumei bundles none."</p>
            if let Some(uid) = &q.added {
                <div class="notice" role="status">"Loaded dashboard " <code>(uid.clone())</code> "."</div>
            }
            if let Some(e) = &q.error {
                <p class="error" role="alert">(e.clone())</p>
            }
            <section class="panel">
                <header class="panel-head"><h2>"Loaded"</h2><span class="panel-note">(rows.len().to_string()) " dashboards"</span></header>
                if rows.is_empty() {
                    <p class="muted">"No dashboards are loaded. Start Kyumei with " <code>"--dashboards DIR"</code> ", or upload a dashboard's JSON below. The dashboards of tsurdilo/temporal-server-operations are in its " <code>"observability/dashboards"</code> " directory."</p>
                } else {
                    <table class="dash-table">
                        <thead><tr><th>"Dashboard"</th><th>"Panels"</th><th>"From"</th><th>"Checks"</th></tr></thead>
                        <tbody>
                            for r in &rows {
                                <tr>
                                    <td>(r.title.clone())
                                        <div class="fine muted">"uid " <code>(r.uid.clone())</code> if let Some(v) = r.version { ", version " (v.to_string()) }</div>
                                        if let Some(src) = &r.source {
                                            <div class="fine">"Kyumei cites it from "
                                                if let Some(url) = &src.url { <a href=(url.clone()) rel="noreferrer">(src.cited.clone())</a> } else { (src.cited.clone()) }
                                            </div>
                                        }
                                        for (id, title, reason) in &r.unreadable {
                                            <div class="flag">"Panel " (id.map_or_else(String::new, |i| i.to_string())) " “" (title.clone()) "” is not read: " (reason.clone())</div>
                                        }
                                    </td>
                                    <td class="fine">(r.charts.to_string()) " charts of " (r.panels.to_string())</td>
                                    <td class="fine">(r.origin.clone())</td>
                                    <td class="fine">if r.findings > 0 { <a href=(format!("#checks-{}", r.uid))>(r.findings.to_string()) " differences"</a> } else { "none" }</td>
                                </tr>
                            }
                        </tbody>
                    </table>
                }
                <form method="post" action="/dashboards" enctype="multipart/form-data" class="evidence-form">
                    <label>"Upload a dashboard (Grafana JSON, as saved or exported)"
                        <input type="file" name="file" accept=".json,application/json" required="">
                    </label>
                    if !can_keep {
                        <p class="fine">"Kyumei keeps uploads until it stops."</p>
                    }
                    <div class="actions"><button type="submit" class="btn btn-primary">"Load dashboard"</button></div>
                </form>
            </section>
            <section class="panel" id="checks">
                <header class="panel-head"><h2>"Checked against the " (pack.manifest.server_family.clone()) " knowledge"</h2>
                    <span class="panel-note">(kinds.iter().map(|(k, n)| format!("{k}: {n}")).collect::<Vec<_>>().join(" · "))</span>
                </header>
                <p class="fine">"Each chart panel's queries, compared with what Kyumei's verified claims say about the metrics they read: their names, their labels, their units, and thresholds that differ by a hundredfold from other dashboards' for the same metric."</p>
                if !choices.is_empty() {
                    <nav class="pack-choice" aria-label="Knowledge pack">"Temporal Server"
                        for (family, href, current) in &choices {
                            if *current { <strong aria-current="page">(family.clone())</strong> } else { <a href=(href.clone())>(family.clone())</a> }
                        }
                    </nav>
                }
                if checks.is_empty() {
                    <p class="muted">"No differences."</p>
                } else {
                    for r in &rows {
                        if r.findings > 0 {
                            <h3 id=(format!("checks-{}", r.uid))>(r.title.clone())</h3>
                            <ul class="findings-list">
                                for f in checks.iter().filter(|f| f.dashboard == r.uid) {
                                    <li><span class="ref-kind">(f.kind.label())</span> " Panel " (f.panel.to_string()) " “" (f.panel_title.clone()) "”: " inline_code(text: &f.detail)
                                        if let Some(c) = &f.claim { " " <a class="fine" href=(format!("/knowledge/{c}?pack={pack_id}"))>(c.clone())</a> }
                                    </li>
                                }
                            </ul>
                        }
                    }
                }
            </section>
        </div>
    })
}

/// Load an uploaded dashboard, and keep it with the data when Kyumei has somewhere to.
#[route(POST "/dashboards")]
pub async fn upload_dashboard(cx: &Cx, mut multipart: Multipart) -> Result<SeeOther> {
    let st = state(cx).clone();
    let back = |m: &str| see_other(format!("/dashboards?error={}", urlencode(m)));
    let mut upload: Option<(String, Vec<u8>)> = None;
    while let Some(field) = multipart.next_field().await? {
        if field.name() == Some("file") {
            let name = field.file_name().unwrap_or("dashboard.json").to_owned();
            upload = Some((name, field.bytes().await?.to_vec()));
        }
    }
    let Some((name, bytes)) = upload.filter(|(_, b)| !b.is_empty()) else {
        return Ok(back("Choose a dashboard's JSON file to upload."));
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(back("That file is not text: a dashboard is JSON."));
    };
    let dashboard = match kyumei_dashboards::parse(&text) {
        Ok(d) => d,
        Err(e) => return Ok(back(&format!("{name} cannot be loaded: {e}."))),
    };
    let uid = dashboard.uid.clone();
    if let Some(dir) = &st.dashboards_dir {
        let file: String = uid
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let kept = std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::write(dir.join(format!("{file}.json")), &text));
        if let Err(e) = kept {
            return Ok(back(&format!(
                "{name} could not be kept in {}: {e}.",
                dir.display()
            )));
        }
    }
    st.dashboards
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .add(dashboard, kyumei_dashboards::Origin::Upload(name));
    Ok(see_other(format!("/dashboards?added={}", urlencode(&uid))))
}

/// A simulation's run, as the evidence page shows it.
struct SimulationPanel {
    hypothesis: String,
    commit: Option<String>,
    calibrated_on: Vec<String>,
    inputs: Vec<(String, String)>,
    results: Vec<(String, String)>,
    scale: Option<String>,
    failure: Option<String>,
}

/// Raw evidence beside Kyumei's interpretation, with corrections.
#[page("/(app)/investigations/{investigation}/evidence/{evidence}")]
pub async fn evidence_page(cx: &Cx) -> Result<impl View> {
    let st = state(cx).clone();
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let eid = EvidenceId::new(path_param::<Evidence>(cx));
    let snapshot = st.service.snapshot(&id).await.map_err(service_error)?;
    let inv = snapshot.investigation;
    let e = inv.evidence(&eid).cloned().ok_or_not_found()?;
    let effective = e.effective_observations(&inv.corrections);
    let corrections: Vec<_> = inv
        .corrections
        .iter()
        .filter(|c| c.evidence == eid)
        .cloned()
        .collect();
    let (_, _, d) =
        kyumei_diagnostics::select(&st.kb, &inv).map_err(|x| topcoat::Error::msg(x.to_string()))?;
    let signals: Vec<(String, String)> = d
        .map(|d| {
            d.signals
                .iter()
                .map(|s| (s.id.to_string(), s.title.clone()))
                .collect()
        })
        .unwrap_or_default();
    let signal_title = |s: &SignalId| {
        d.and_then(|d| d.signal(s))
            .map_or(s.to_string(), |x| x.title.clone())
    };
    let z = zone(cx);
    let charts = {
        let links = GrafanaLinks::for_investigation(st.grafana_url.as_deref(), &inv);
        evidence_charts(&st.library(), &st.kb, &inv, &e, &links, &z)
    };
    // Large text is stored by reference; show its beginning.
    let (raw_text, raw_truncated) = match &e.raw.body {
        RawBody::Text { text } => (Some(text.clone()), false),
        RawBody::Stored { reference } if e.raw.media_type.starts_with("text/") => {
            match st.service.blobs().get(reference)? {
                Some(bytes) => {
                    let head = &bytes[..bytes.len().min(RAW_PREVIEW_BYTES)];
                    // Stop before a character the cut split.
                    let end = std::str::from_utf8(head).map_or_else(|e| e.valid_up_to(), str::len);
                    let text = String::from_utf8_lossy(&head[..end]).into_owned();
                    (Some(text), end < bytes.len())
                }
                None => (None, false),
            }
        }
        RawBody::Stored { .. } => (None, false),
    };
    let size = format_size(e.raw.size_bytes);
    let rows: Vec<ObservationRow> = effective
        .iter()
        .map(|o| {
            let ob = &o.observation;
            ObservationRow {
                id: ob.id.to_string(),
                subject: ob.subject.display(),
                informs: ob
                    .binding
                    .as_ref()
                    .map_or("—".to_owned(), |b| signal_title(&b.signal)),
                basis: ob
                    .binding
                    .as_ref()
                    .map_or(String::new(), |b| b.basis.describe()),
                value: ob.value.brief(),
                window: ob.window.map_or("undated".to_owned(), |w| z.window(&w)),
                excluded: o.excluded,
                corrections: o.corrections.iter().map(ToString::to_string).collect(),
            }
        })
        .collect();
    let notes: Vec<_> = e
        .interpretation
        .notes
        .iter()
        .cloned()
        .map(|mut n| {
            n.text = kyumei_app::zone::localize(&n.text, &z, e.submitted_at);
            n
        })
        .collect();
    // A simulation's run: what it was calibrated on, and what it found.
    let simulation = e.simulation.as_ref().map(|r| {
        let (inputs, results) = kyumei_app::simulation::quantities(r);
        let hypothesis = d
            .and_then(|d| d.hypothesis(&r.hypothesis))
            .map_or(r.hypothesis.to_string(), |h| h.title.clone());
        let commit = match &e.origin {
            kyumei_domain::EvidenceOrigin::Simulation { commit, .. } => Some(commit.clone()),
            _ => None,
        };
        SimulationPanel {
            hypothesis,
            commit,
            calibrated_on: r.calibrated_on.iter().map(ToString::to_string).collect(),
            inputs,
            results,
            scale: (r.scale > 1.0).then(|| kyumei_domain::format_number(r.scale)),
            failure: r.failure.clone(),
        }
    });
    let zone_label = z.label(e.submitted_at);
    let title = format!("{} — {}", e.id, e.interpretation.form.label());
    let investigation_title = inv.title.clone();
    let raw_href = format!("/investigations/{id}/evidence/{eid}/raw");
    Ok(view! {
        <div class="evidence-page">
            <p class="crumbs"><a href=(format!("/investigations/{id}"))>(investigation_title)</a> " › " (e.id.to_string())</p>
            <h1>(title)</h1>
            <p class="fine muted">
                "Submitted " (z.date_time(e.submitted_at)) " · " (e.origin.describe())
                if let Some(r) = &e.responds_to { " · answers " <code>(r.to_string())</code> }
                " · " (e.raw.media_type.clone()) " · " (e.raw.size_bytes) " bytes · sha256 " <code class="hash">(e.raw.sha256[..16].to_owned()) "…"</code>
            </p>
            <div class="split">
                if let Some(s) = &simulation {
                    panel(title: "Simulation", note: Some("A simulated fact: it can support or weaken a hypothesis, never settle one"),
                        <dl class="kv">
                            <div><dt>"Tests"</dt><dd>(s.hypothesis.clone())</dd></div>
                            if let Some(c) = &s.commit {
                                <div><dt>"tempdes"</dt><dd><a href=(format!("https://github.com/iw/tempdes/commit/{c}"))><code>(c.chars().take(7).collect::<String>())</code></a></dd></div>
                            }
                            <div><dt>"Calibrated on"</dt><dd>for c in &s.calibrated_on { evidence_ref(investigation: id.as_str(), id: c) " " }</dd></div>
                            for (label, value) in &s.inputs {
                                <div><dt>(label.clone())</dt><dd class="num">(value.clone())</dd></div>
                            }
                            if let Some(k) = &s.scale {
                                <div><dt>"Rates divided by"</dt><dd class="num">(k.clone())</dd></div>
                            }
                        </dl>
                        if let Some(why) = &s.failure {
                            <p class="flag">"No result: " (why.clone())</p>
                        }
                        if !s.results.is_empty() {
                            <h3>"In the simulation"</h3>
                            <dl class="kv">
                                for (label, value) in &s.results {
                                    <div><dt>(label.clone())</dt><dd class="num">(value.clone())</dd></div>
                                }
                            </dl>
                        }
                    )
                }
                panel(title: if simulation.is_some() { "Scenario" } else { "As submitted" }, note: Some(if simulation.is_some() { "The tempdes scenario, exactly as run" } else { "Preserved exactly; never modified" }),
                    match &raw_text {
                        Some(t) => <pre class="raw">(t.clone())</pre>,
                        None => <p>"Binary content (" (size.clone()) "). " <a href=(raw_href.clone())>"Download the original"</a></p>,
                    }
                    if raw_truncated {
                        <p class="fine">"Showing the beginning of " (size.clone()) ". " <a href=(raw_href.clone())>"Download the original"</a></p>
                    }
                    if let Some(q) = &e.declared.query {
                        <p class="fine">"Query: " <code>(q.clone())</code></p>
                    }
                    if let Some(n) = &e.declared.note {
                        <p class="fine">"Note: " (n.clone())</p>
                    }
                )
                panel(title: "Kyumei’s interpretation", note: Some(e.interpretation.interpreter.as_str()),
                    <table class="observations">
                        <tbody>
                            for r in &rows {
                                <tr id=(r.id.clone()) class=(class!("excluded" if r.excluded))>
                                    <td class="obs-id"><code>(r.id.clone())</code></td>
                                    <td>
                                        <code class="subject">(r.subject.clone())</code>
                                        <div class="obs-value">(r.value.clone())</div>
                                        <div class="fine muted">(r.window.clone())</div>
                                    </td>
                                    <td class="obs-informs">(r.informs.clone()) <div class="fine muted">(r.basis.clone())</div>
                                        if r.excluded { <span class="flag">"excluded"</span> }
                                        if !r.corrections.is_empty() { <span class="tag">"corrected " (r.corrections.join(", "))</span> }
                                    </td>
                                </tr>
                            }
                        </tbody>
                    </table>
                    if !notes.is_empty() {
                        <ul class="notes">
                            for n in &notes {
                                <li class=(format!("note-{:?}", n.severity).to_lowercase())>
                                    if let Some(o) = &n.observation { <code>(o.to_string())</code> " " }
                                    (n.text.clone())
                                </li>
                            }
                        </ul>
                    }
                    if !corrections.is_empty() {
                        <h3>"Corrections"</h3>
                        <ul>
                            for c in &corrections {
                                <li><code>(c.id.to_string())</code> " " (c.observation.as_ref().map(|o| o.to_string()).unwrap_or_else(|| "all observations".to_owned())) ": " (c.change.describe()) " — " <em>(c.reason.clone())</em></li>
                            }
                        </ul>
                    }
                    <details class="correct">
                        <summary class="btn btn-secondary">"Correct this interpretation"</summary>
                        <form method="post" action=(format!("/investigations/{id}/evidence/{eid}/corrections")) class="evidence-form">
                            <p class="fine">"Corrections change how Kyumei reads the evidence. The submission itself is never changed."</p>
                            <div class="row">
                                <label>"Observation"
                                    <select name="observation">
                                        <option value="">"all observations"</option>
                                        for r in &rows {
                                            <option value=(r.id.clone())>(r.id.clone())</option>
                                        }
                                    </select>
                                </label>
                                <label>"Change"
                                    <select name="change">
                                        <option value="bind">"It is about this signal"</option>
                                        <option value="unbind">"It is not about any signal"</option>
                                        <option value="unit">"Its unit is"</option>
                                        <option value="window">"It covers this window"</option>
                                        <option value="exclude">"Disregard it"</option>
                                        <option value="include">"Use it again"</option>
                                    </select>
                                </label>
                                <label>"Signal"
                                    <select name="signal">
                                        <option value="">"—"</option>
                                        for (sid, t) in &signals {
                                            <option value=(sid.clone())>(t.clone())</option>
                                        }
                                    </select>
                                </label>
                                <label>"Unit"
                                    <select name="unit">
                                        <option value="">"—"</option>
                                        for u in [Unit::Seconds, Unit::Milliseconds, Unit::Count, Unit::PerSecond, Unit::Ratio] {
                                            <option value=(u.to_string())>(u.to_string())</option>
                                        }
                                    </select>
                                </label>
                            </div>
                            <div class="row">
                                <label>"From (" (zone_label.clone()) ")"<input type="datetime-local" name="window_start"></label>
                                <label>"To (" (zone_label.clone()) ")"<input type="datetime-local" name="window_end"></label>
                                <label class="grow">"Why"<input name="reason" required="" placeholder="e.g. the query was filtered to ActivityWorker"></label>
                            </div>
                            <div class="actions"><button type="submit" class="btn btn-primary">"Apply correction"</button></div>
                        </form>
                    </details>
                )
            </div>
            if !charts.is_empty() {
                panel(title: "As your dashboards draw it", note: Some("Each chart is drawn as the panel that reads the same metric"),
                    <div class="charts">
                        for c in &charts {
                            <figure class="chart" id=(c.id.clone())>
                                (Unescaped::new_unchecked(c.svg.clone()))
                                <figcaption class="fine chart-cite">
                                    "Panel “" (c.panel.panel_title.clone()) "” (id " (c.panel.panel_id.to_string()) ") of " (c.panel.dashboard_title.clone())
                                    if let Some(v) = c.panel.dashboard_version { ", version " (v.to_string()) }
                                    ", from " (c.panel.origin.clone()) "."
                                    if let Some(r) = &c.panel.reference {
                                        " The dashboard is from "
                                        if let Some(url) = &r.url { <a href=(url.clone()) rel="noreferrer">(r.cited.clone())</a> } else { (r.cited.clone()) }
                                        "."
                                    }
                                    " " <a href=(c.panel.open_url.clone())>"Open the panel with live data"</a>
                                    if let Some(g) = &c.panel.grafana_url {
                                        " · " <a href=(g.clone()) rel="noreferrer">"in Grafana"</a>
                                    }
                                </figcaption>
                                <details>
                                    <summary class="fine">"Text version"</summary>
                                    <pre class="raw">(c.text.clone())</pre>
                                </details>
                            </figure>
                        }
                    </div>
                )
            }
        </div>
    })
}

/// How much of a large stored text the evidence page shows.
const RAW_PREVIEW_BYTES: usize = 64 * 1024;

fn format_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MiB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{:.0} KiB", b as f64 / 1024.0),
        b => format!("{b} bytes"),
    }
}

/// One observation, as the evidence page shows it.
struct ObservationRow {
    id: String,
    subject: String,
    informs: String,
    basis: String,
    value: String,
    window: String,
    excluded: bool,
    corrections: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CorrectionForm {
    observation: Option<String>,
    change: String,
    signal: Option<String>,
    unit: Option<String>,
    window_start: Option<String>,
    window_end: Option<String>,
    reason: String,
}

#[route(POST "/investigations/{investigation}/evidence/{evidence}/corrections")]
pub async fn correct(cx: &Cx, Form(f): Form<CorrectionForm>) -> Result<SeeOther> {
    let st = state(cx).clone();
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let eid = EvidenceId::new(path_param::<Evidence>(cx));
    let back = |m: &str| {
        see_other(format!(
            "/investigations/{id}/evidence/{eid}?error={}",
            urlencode(m)
        ))
    };
    let change = match f.change.as_str() {
        "bind" => match f.signal.clone().filter(|s| !s.is_empty()) {
            Some(s) => CorrectionChange::BindSignal { signal: s.into() },
            None => return Ok(back("choose a signal")),
        },
        "unbind" => CorrectionChange::Unbind,
        "unit" => match f.unit.as_deref().and_then(|u| u.parse::<Unit>().ok()) {
            Some(unit) => CorrectionChange::SetUnit { unit },
            None => return Ok(back("choose a unit")),
        },
        "window" => match (
            time(f.window_start.as_deref(), &zone(cx)),
            time(f.window_end.as_deref(), &zone(cx)),
        ) {
            (Ok(Some(s)), Ok(Some(e))) => match TimeWindow::new(s, e) {
                Ok(window) => CorrectionChange::SetWindow { window },
                Err(e) => return Ok(back(&e.to_string())),
            },
            _ => return Ok(back("give both ends of the window")),
        },
        "exclude" => CorrectionChange::Exclude,
        "include" => CorrectionChange::Include,
        _ => return Err(bad_request("unknown correction").into()),
    };
    let input = CorrectionInput {
        evidence: eid.clone(),
        observation: f
            .observation
            .clone()
            .filter(|o| !o.is_empty())
            .map(ObservationId::new),
        change,
        reason: f.reason.clone(),
    };
    match st
        .service
        .command(&id, Command::CorrectInterpretation(input))
        .await
    {
        Ok(_) => Ok(see_other(format!("/investigations/{id}/evidence/{eid}"))),
        Err(ServiceError::Command(CommandError::Invalid(m))) => Ok(back(&m)),
        Err(e) => Err(service_error(e)),
    }
}

#[route(GET "/investigations/{investigation}/evidence/{evidence}/raw")]
pub async fn raw_evidence(cx: &Cx) -> Result<([(HeaderName, HeaderValue); 2], Vec<u8>)> {
    let st = state(cx).clone();
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let eid = EvidenceId::new(path_param::<Evidence>(cx));
    let snapshot = st.service.snapshot(&id).await.map_err(service_error)?;
    let e = snapshot
        .investigation
        .evidence(&eid)
        .cloned()
        .ok_or_not_found()?;
    let bytes = match &e.raw.body {
        RawBody::Text { text } => text.as_bytes().to_vec(),
        RawBody::Stored { reference } => st.service.blobs().get(reference)?.ok_or_not_found()?,
    };
    let filename = e.raw.filename.clone().unwrap_or_else(|| format!("{eid}"));
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_str(&e.raw.media_type)
                    .unwrap_or(HeaderValue::from_static("application/octet-stream")),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!(
                    "attachment; filename=\"{}\"",
                    filename.replace('"', "")
                ))?,
            ),
        ],
        bytes,
    ))
}

// ------------------------------------------------------------------ export

#[route(GET "/investigations/{investigation}/export.md")]
pub async fn export(cx: &Cx) -> Result<([(HeaderName, HeaderValue); 2], String)> {
    let st = state(cx).clone();
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let snapshot = st.service.snapshot(&id).await.map_err(service_error)?;
    let current = kyumei_app::core::current_assessment(&st.kb, &snapshot.investigation)
        .map_err(|e| topcoat::Error::msg(e.to_string()))?;
    let w = workspace::build_with(
        &st.kb,
        &snapshot,
        &current,
        st.grafana_url.as_deref(),
        &zone(cx),
    );
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/markdown; charset=utf-8"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"kyumei-{id}.md\""))?,
            ),
        ],
        kyumei_app::export::markdown(&w),
    ))
}

/// Server-sent "changed" events, so an open workspace notices updates from
/// other tabs and from the explanation agent.
#[route(GET "/investigations/{investigation}/events")]
pub async fn events(cx: &Cx) -> Result<Sse<impl Stream<Item = Result<Event>> + use<>>> {
    let st = state(cx).clone();
    let id = InvestigationId::new(path_param::<Investigation>(cx));
    let initial = st
        .service
        .snapshot(&id)
        .await
        .map(|s| version_of(&s))
        .map_err(service_error)?;
    let s = stream::unfold((st, id, initial), |(st, id, last)| async move {
        loop {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            match st.service.snapshot(&id).await {
                Ok(snapshot) => {
                    let v = version_of(&snapshot);
                    if v != last {
                        let event = Event::new().event("changed").data(v.clone());
                        return Some((Ok(event), (st, id, v)));
                    }
                }
                Err(_) => return None,
            }
        }
    });
    Ok(Sse::new(s).keep_alive(KeepAlive::new()))
}

// --------------------------------------------------------------- knowledge

#[query_params(error = bad_request)]
#[derive(Debug)]
pub struct KnowledgeQuery {
    q: Option<String>,
    kind: Option<String>,
    status: Option<String>,
    used: Option<String>,
    pack: Option<String>,
}

/// How many matching claims the knowledge page lists at once.
const KNOWLEDGE_ROWS: usize = 200;

/// One claim in the knowledge search results.
struct ClaimRow {
    order: usize,
    id: String,
    name: String,
    kind: &'static str,
    purpose: String,
    unverified: bool,
    used_by: String,
}

#[page("/(app)/knowledge")]
pub async fn knowledge_index(cx: &Cx) -> Result<impl View> {
    use kyumei_domain::ClaimCategory;
    let st = state(cx).clone();
    let query = query_params::<KnowledgeQuery>(cx)?;
    let q = query.q.clone().unwrap_or_default();
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    let kind = query.kind.as_deref().and_then(ClaimCategory::from_token);
    let status = query.status.clone().unwrap_or_default();
    let used_only = query.used.as_deref().is_some_and(|u| !u.is_empty());
    let searching = !words.is_empty() || kind.is_some() || !status.is_empty() || used_only;
    let p = shown_pack(&st.kb, query.pack.as_deref())
        .ok_or_not_found()?
        .clone();
    let pack_id = p.manifest.id.to_string();
    // The same search in another pack.
    let choices = pack_choices(&st.kb, &p, |other| {
        let mut href = format!("/knowledge?pack={}", other.manifest.id);
        for (name, value) in [
            ("q", query.q.as_deref()),
            ("kind", query.kind.as_deref()),
            ("status", query.status.as_deref()),
            ("used", query.used.as_deref()),
        ] {
            if let Some(v) = value.filter(|v| !v.is_empty()) {
                href.push_str(&format!("&{name}={}", urlencode(v)));
            }
        }
        href
    });

    let mut counts: BTreeMap<ClaimCategory, usize> = BTreeMap::new();
    let mut used_count = 0usize;
    let mut rows = Vec::new();
    {
        let uses = p.claim_uses();
        for c in p.claims.values() {
            *counts.entry(c.category()).or_default() += 1;
            let claim_uses = uses.get(c.id());
            let diagnostics: Vec<String> = claim_uses
                .into_iter()
                .flatten()
                .filter_map(|u| u.diagnostic.as_ref().map(ToString::to_string))
                .fold(Vec::new(), |mut acc, d| {
                    if !acc.contains(&d) {
                        acc.push(d);
                    }
                    acc
                });
            if !diagnostics.is_empty() {
                used_count += 1;
            }
            let verified = c.verification().is_verified();
            let keep = searching
                && kind.is_none_or(|k| k == c.category())
                && match status.as_str() {
                    "verified" => verified,
                    "unverified" => !verified,
                    _ => true,
                }
                && (!used_only || !diagnostics.is_empty())
                && {
                    let text = format!("{} {} {}", c.id(), c.name(), c.purpose()).to_lowercase();
                    words.iter().all(|w| text.contains(w.as_str()))
                };
            if keep {
                rows.push(ClaimRow {
                    order: ClaimCategory::ALL
                        .iter()
                        .position(|k| *k == c.category())
                        .unwrap_or_default(),
                    id: c.id().to_string(),
                    name: c.name(),
                    kind: c.category().label(),
                    purpose: first_sentence(&c.purpose()),
                    unverified: !verified,
                    used_by: if !diagnostics.is_empty() {
                        diagnostics.join(", ")
                    } else if claim_uses.is_some() {
                        "a heuristic".to_owned()
                    } else {
                        String::new()
                    },
                });
            }
        }
    }
    rows.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.name.cmp(&b.name)));
    let matched = rows.len();
    rows.truncate(KNOWLEDGE_ROWS);
    let kinds: Vec<(&'static str, &'static str, usize)> = ClaimCategory::ALL
        .iter()
        .filter_map(|c| counts.get(c).map(|n| (c.token(), c.label(), *n)))
        .collect();

    Ok(view! {
        <div class="knowledge">
            <h1>"Knowledge"</h1>
            <p class="muted">"Every metric, configuration key, API field and behaviour Kyumei relies on, with the versions it was verified against and where. Kyumei’s own thresholds are listed separately as heuristics."</p>
            if !choices.is_empty() {
                <nav class="pack-choice" aria-label="Knowledge pack">"Temporal Server"
                    for (family, href, current) in &choices {
                        if *current { <strong aria-current="page">(family.clone())</strong> } else { <a href=(href.clone())>(family.clone())</a> }
                    }
                </nav>
            }
            <form method="get" action="/knowledge" class="knowledge-search" role="search">
                <input type="hidden" name="pack" value=(pack_id.clone())>
                <label class="grow">"Search"
                    <input type="search" name="q" value=(q.clone()) spellcheck="false" placeholder="A key, a metric or words, e.g. numTaskqueue">
                </label>
                <label>"Kind"
                    <select name="kind">
                        <option value="">"any"</option>
                        for (token, label, n) in &kinds {
                            <option value=(*token) selected=(kind.is_some_and(|k| k.token() == *token))>(*label) " (" (*n) ")"</option>
                        }
                    </select>
                </label>
                <label>"Status"
                    <select name="status">
                        <option value="">"any"</option>
                        <option value="verified" selected=(status == "verified")>"verified"</option>
                        <option value="unverified" selected=(status == "unverified")>"unverified"</option>
                    </select>
                </label>
                <label class="check"><input type="checkbox" name="used" value="1" checked=(used_only)>"Used by a diagnostic"</label>
                <button type="submit" class="btn btn-secondary">"Search"</button>
            </form>
            if searching {
                <p class="fine muted">
                    (matched) " matching claims"
                    if matched > KNOWLEDGE_ROWS { "; showing the first " (KNOWLEDGE_ROWS) ". Narrow the search to see the rest." }
                    " " <a href=(format!("/knowledge?pack={pack_id}"))>"Clear"</a>
                </p>
                if !rows.is_empty() {
                    <table class="claims-table">
                        <thead><tr><th>"Claim"</th><th>"Kind"</th><th>"Used by"</th></tr></thead>
                        <tbody>
                            for r in &rows {
                                <tr>
                                    <td>
                                        <a href=(format!("/knowledge/{}?pack={pack_id}", r.id))><code>(r.name.clone())</code></a>
                                        if r.unverified { " " <span class="flag">"unverified"</span> }
                                        <div class="fine muted">(r.purpose.clone())</div>
                                    </td>
                                    <td class="fine">(r.kind)</td>
                                    <td class="fine">(r.used_by.clone())</td>
                                </tr>
                            }
                        </tbody>
                    </table>
                }
            } else {
                <section class="panel">
                    <header class="panel-head"><h2>(p.manifest.title.clone())</h2>
                        <span class="panel-note">"v" (p.manifest.version.clone()) " · verified for " (p.manifest.verified_server_versions.join(", ")) " · " <code class="hash">(p.digest[..12].to_owned())</code></span></header>
                    <p>(p.manifest.description.clone())</p>
                    <ul class="claim-kinds">
                        for (token, label, n) in &kinds {
                            <li><a href=(format!("/knowledge?pack={pack_id}&kind={token}"))>(*label)</a> " " <span class="muted">(*n)</span></li>
                        }
                    </ul>
                    <p><a href=(format!("/knowledge?pack={pack_id}&used=1"))>"Claims a diagnostic uses"</a> " " <span class="muted">(used_count)</span></p>
                    <h3>"Sources"</h3>
                    <ul class="claim-list">
                        for s in p.sources.values() {
                            <li><code>(s.id.to_string())</code> " " (s.title.clone())
                                <span class="fine muted">" " (s.revision.clone().unwrap_or_default()) " " (s.commit.clone().map(|c| c[..12.min(c.len())].to_owned()).unwrap_or_default())</span></li>
                        }
                    </ul>
                </section>
            }
        </div>
    })
}

fn first_sentence(s: &str) -> String {
    let end = s.find(". ").map_or(s.len(), |i| i + 1);
    s[..end].to_owned()
}

#[query_params(error = bad_request)]
#[derive(Debug)]
pub struct ClaimQuery {
    pack: Option<String>,
}

#[page("/(app)/knowledge/{knowledge_claim}")]
pub async fn claim_page(cx: &Cx) -> Result<impl View> {
    let st = state(cx).clone();
    let id: String = path_param::<KnowledgeClaim>(cx).to_owned();
    let asked = query_params::<ClaimQuery>(cx)?.pack.clone();
    // The pack asked for, or the newest that has the claim.
    let holding: Vec<&Pack> = packs_newest_first(&st.kb)
        .into_iter()
        .filter(|p| p.claim(&id.as_str().into()).is_some())
        .collect();
    let pack: Pack = match asked.as_deref().filter(|a| !a.is_empty()) {
        Some(a) => holding.iter().find(|p| p.manifest.id.as_str() == a),
        None => holding.first(),
    }
    .copied()
    .ok_or_not_found()?
    .clone();
    let claim = pack.claim(&id.as_str().into()).ok_or_not_found()?.clone();
    let pack_id = pack.manifest.id.to_string();
    let elsewhere: Vec<(String, String)> = holding
        .iter()
        .filter(|p| p.manifest.id != pack.manifest.id)
        .map(|p| {
            (
                p.manifest.server_family.clone(),
                format!("/knowledge/{id}?pack={}", p.manifest.id),
            )
        })
        .collect();
    let summary = pack.claim_summary(&id.as_str().into()).ok_or_not_found()?;
    let scope = claim.scope();
    // Every citation, a setting's other releases' included.
    let provenance: Vec<kyumei_domain::Provenance> = claim
        .all_provenance()
        .into_iter()
        .map(kyumei_knowledge::ProvenanceDef::to_domain)
        .collect();
    let links: Vec<(String, Option<String>, Option<String>)> = provenance
        .iter()
        .map(|p| {
            // Link to the newest source a citation names.
            let source = p.sources.last().and_then(|id| pack.source(id));
            let place = match (&p.path, p.line) {
                (Some(path), Some(line)) => format!("{path}:{line}"),
                (Some(path), None) => path.clone(),
                _ => String::new(),
            };
            let url = source.and_then(|s| {
                let repo = s.repository.as_ref()?.trim_end_matches(".git").to_owned();
                let rev = s.commit.clone().or(s.revision.clone())?;
                let path = p.path.as_ref()?;
                Some(match p.line {
                    Some(l) => format!("{repo}/blob/{rev}/{path}#L{l}"),
                    None => format!("{repo}/blob/{rev}/{path}"),
                })
            });
            (
                format!("{} {}", sources_label(&p.sources), place),
                url.or(p.url.clone()),
                p.excerpt.clone(),
            )
        })
        .collect();
    let metric = claim.as_metric().cloned();
    let setting = match &claim {
        kyumei_knowledge::Claim::DynamicConfig(d) => Some(d.clone()),
        _ => None,
    };
    // Who uses this claim: grouped by diagnostic, then heuristics that rely on it.
    let mut used_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut relied_on_by: Vec<String> = Vec::new();
    for u in pack
        .claim_uses()
        .remove(&id.as_str().into())
        .unwrap_or_default()
    {
        match &u.diagnostic {
            Some(d) => {
                let title = pack
                    .diagnostics
                    .iter()
                    .find(|x| &x.id == d)
                    .map_or(d.to_string(), |x| x.title.clone());
                used_by.entry(title).or_default().push(u.by);
            }
            None => relied_on_by.push(u.by.trim_start_matches("heuristic ").to_owned()),
        }
    }
    Ok(view! {
        <div class="claim-page">
            <p class="crumbs"><a href=(format!("/knowledge?pack={pack_id}"))>"Knowledge"</a> " › " (summary.category.label())</p>
            <h1><code>(summary.name.clone())</code></h1>
            <dl class="kv">
                <div><dt>"Kind"</dt><dd>(summary.category.label())</dd></div>
                <div><dt>"Pack"</dt><dd>(pack.manifest.title.clone())
                    if !elsewhere.is_empty() {
                        <span class="fine muted">" · also in the "
                            for (i, (family, href)) in elsewhere.iter().enumerate() {
                                if i > 0 { ", " }
                                <a href=(href.clone())>(family.clone())</a>
                            }
                            " knowledge"
                        </span>
                    }
                </dd></div>
                <div><dt>"Purpose"</dt><dd>(summary.purpose.clone())</dd></div>
                <div><dt>"Applicable"</dt><dd>
                    if !scope.server.is_empty() {
                        "Temporal Server " (kyumei_domain::describe_releases(&scope.server)) " "
                    }
                    for s in &scope.sdk {
                        <div>(s.family.label()) ": " (s.revision.clone())</div>
                    }
                    if scope.server.is_empty() && scope.sdk.is_empty() { "not tied to a version" }
                </dd></div>
                <div><dt>"Verification"</dt><dd>
                    match &summary.verification {
                        kyumei_domain::Verification::Verified { method, on } => { "verified by " (method.label()) " on " (on.to_string()) },
                        kyumei_domain::Verification::Unverified { reason } => <span class="flag">"unverified: " (reason.clone())</span>,
                    }
                </dd></div>
                if let Some(d) = &setting {
                    <div><dt>"Key"</dt><dd><code>(d.key.clone())</code></dd></div>
                    <div><dt>"Type"</dt><dd>(d.value_type.clone())</dd></div>
                    <div><dt>"Default"</dt><dd><code>(d.default.clone())</code></dd></div>
                    <div><dt>"Constrained by"</dt><dd>(d.constrained_by.clone())</dd></div>
                    <div><dt>"Registered with"</dt><dd><code>(d.setting.clone())</code></dd></div>
                    if !d.variants.is_empty() {
                        <div><dt>"In other releases"</dt><dd>
                            for v in &d.variants {
                                <div>"Temporal Server " (releases(&v.server)) ": " (v.difference(d))</div>
                            }
                        </dd></div>
                    }
                }
                if let Some(m) = &metric {
                    <div><dt>"Instrument"</dt><dd>(m.instrument.label())</dd></div>
                    if let Some(u) = m.unit {
                        <div><dt>"Unit"</dt><dd>(u.to_string())</dd></div>
                    }
                    if let Some(go) = &m.go_name {
                        <div><dt>"Defined as"</dt><dd><code>(go.clone())</code></dd></div>
                    }
                    if m.go_name.is_some() {
                        <div><dt>"Recorded in"</dt><dd>
                            if m.recorded_in.is_empty() { <span class="muted">"no package refers to it"</span> }
                            for dir in &m.recorded_in { <div><code>(dir.clone())</code></div> }
                        </dd></div>
                    }
                    if !m.labels.is_empty() {
                        <div><dt>"Labels"</dt><dd>
                            for l in &m.labels {
                                <div><code>(l.name.clone())</code>
                                    if !l.values.is_empty() { " — " <span class="fine">(l.values.join(", "))</span> }
                                    if let Some(p) = &l.purpose { <div class="fine muted">(p.clone())</div> }
                                </div>
                            }
                        </dd></div>
                    }
                    <div><dt>"Exposed as"</dt><dd>
                        for e in &m.exposition {
                            <div><code>(e.name.clone())</code>
                                if let Some(f) = e.sdk_family { " " <span class="tag">(f.short_label())</span> }
                                if let Some(u) = e.unit { " " <span class="tag">(u.to_string())</span> }
                                <div class="fine muted">(e.condition.clone())</div>
                            </div>
                        }
                    </dd></div>
                }
            </dl>
            if !claim.notes().is_empty() {
                <h3>"Notes"</h3>
                <ul>for n in claim.notes() { <li>(n.clone())</li> }</ul>
            }
            <h3>"Used by"</h3>
            if used_by.is_empty() && relied_on_by.is_empty() {
                <p class="muted">"Nothing uses this claim yet: no diagnostic reads it and no heuristic relies on it."</p>
            } else {
                <ul class="claim-list">
                    for (title, by) in &used_by {
                        <li><strong>(title.clone())</strong> ": " (by.join(", "))</li>
                    }
                    for h in &relied_on_by {
                        <li>"Heuristic " <a href=(format!("/knowledge/{h}?pack={pack_id}"))><code>(h.clone())</code></a> " relies on it"</li>
                    }
                </ul>
            }
            <h3>"Verified against"</h3>
            <ul class="provenance">
                for (place, url, excerpt) in &links {
                    <li>
                        match url {
                            Some(u) => <a href=(u.clone()) rel="noreferrer">(place.clone())</a>,
                            None => <span>(place.clone())</span>,
                        }
                        if let Some(x) = excerpt { <pre class="excerpt">(x.clone())</pre> }
                    </li>
                }
            </ul>
        </div>
    })
}

/// Releases named as strings, written compactly.
fn releases(versions: &[String]) -> String {
    let parsed: Vec<kyumei_domain::ServerVersion> =
        versions.iter().filter_map(|v| v.parse().ok()).collect();
    kyumei_domain::describe_releases(&parsed)
}

/// A citation's sources: its ID, or the releases of the server sources it names.
fn sources_label(sources: &[kyumei_domain::SourceId]) -> String {
    let server: Option<Vec<String>> = sources
        .iter()
        .map(|s| {
            s.as_str()
                .strip_prefix("temporal-server-v")
                .map(str::to_owned)
        })
        .collect();
    match server {
        Some(versions) if versions.len() > 1 => format!("Temporal Server {}", releases(&versions)),
        _ => sources
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

// ------------------------------------------------------------------ assets

#[route(GET "/app.css")]
pub async fn stylesheet() -> Result<Css<&'static str>> {
    Ok(Css(include_str!("assets/app.css")))
}

#[route(GET "/app.js")]
pub async fn script() -> Result<Js<&'static str>> {
    Ok(Js(include_str!("assets/app.js")))
}

#[route(GET "/health")]
pub async fn health(cx: &Cx) -> Result<String> {
    Ok(format!("ok {}", state(cx).service.describe()))
}

pub(crate) fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
