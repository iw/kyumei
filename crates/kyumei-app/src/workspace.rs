//! The investigation workspace as a view model.
//!
//! Everything the interface shows comes from here, derived from the
//! investigation, the current assessment and versioned knowledge. The web
//! layer only lays it out, so a different interface (or an export) shows the
//! same statements with the same references.

use std::collections::BTreeMap;

use kyumei_diagnostics::{Assessment, DiagnosticExplanation, Fact, explain};
use kyumei_diagrams::DiagramKind;
use kyumei_domain::{
    Applicability, ClaimId, DisplayZone, DomainRef, Effect, EpistemicStatus, Evidence, EvidenceId,
    HypothesisId, HypothesisState, Investigation, InvestigationStatus, RequestStatus, Statistic,
    TrailItem, UncertaintyKind,
};
use kyumei_knowledge::{Claim, DiagnosticDef, KnowledgeBase, Pack, ReferenceTarget};
use serde::Serialize;

use crate::service::{ExplanationStatus, InvestigationSnapshot};
use crate::{
    collect::CollectionPlan,
    dashboards::{GrafanaLinks, ReferenceView, reference_views},
    zone::localize,
};

#[derive(Clone, Debug, Serialize)]
pub struct Workspace {
    pub id: String,
    pub title: String,
    pub status: StatusView,
    pub situation: Situation,
    pub assessment: AssessmentView,
    pub next: Option<NextEvidence>,
    pub findings: Vec<FindingView>,
    pub evidence: Vec<EvidenceRow>,
    pub requests: Vec<RequestRow>,
    /// The request answered most recently, shown above the next one.
    pub last_answered: Option<AnsweredView>,
    pub trail: Vec<TrailRow>,
    pub diagrams: Vec<DiagramView>,
    pub explanation: ExplanationMeta,
    pub runtime: String,
    /// Something the runtime did to this investigation, such as restoring it
    /// from its projection after a restart.
    pub runtime_note: Option<String>,
    /// Signals that can be entered through the structured form.
    pub signals: Vec<(String, String)>,
    /// The zone times are shown in.
    pub zone: ZoneView,
}

/// The zone a page's times are written in, for headings over lists of times.
#[derive(Clone, Debug, Serialize)]
pub struct ZoneView {
    /// `Europe/London`, or `UTC`.
    pub name: String,
    /// `BST`, `UTC−03:00` or `UTC`, as at the investigation's latest event.
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct StatusView {
    pub token: &'static str,
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Situation {
    pub symptom: String,
    pub symptom_title: Option<String>,
    pub classification: Option<String>,
    pub scope: Vec<(String, String)>,
    pub window: String,
    /// The window in UTC, for a tooltip when another zone is shown.
    pub window_utc: String,
    pub environment: Vec<EnvRow>,
    pub knowledge: String,
    pub knowledge_verified: bool,
    /// The pack's ID, for links to its pages; empty when there is none.
    pub knowledge_pack: String,
    /// Runbooks and playbooks for the symptom as a whole.
    pub references: Vec<ReferenceView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EnvRow {
    pub label: String,
    pub value: String,
    pub status: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct AssessmentView {
    pub summary: String,
    pub knows: Vec<FactRow>,
    pub leading: Vec<HypothesisRow>,
    pub open: Vec<HypothesisRow>,
    pub eliminated: Vec<HypothesisRow>,
    pub uncertainties: Vec<UncertaintyRow>,
    pub conclusion: Option<ConclusionView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FactRow {
    pub signal: String,
    pub title: String,
    pub summary: String,
    pub status: EpistemicStatus,
    pub evidence: Vec<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HypothesisRow {
    pub id: String,
    pub title: String,
    pub statement: String,
    pub state: HypothesisState,
    pub contested: bool,
    pub stage: String,
    pub findings: Vec<String>,
    /// Runbooks and playbook sections for this hypothesis.
    pub references: Vec<ReferenceView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UncertaintyRow {
    pub kind: UncertaintyKind,
    pub label: &'static str,
    pub text: String,
    pub refs: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConclusionView {
    pub title: String,
    pub statement: String,
    /// Supportable, but evidence the path still calls for is outstanding.
    pub provisional: bool,
    pub recommendations: Vec<RecommendationView>,
    pub findings: Vec<String>,
    pub evidence: Vec<String>,
    /// Runbooks, playbook sections and panels for this conclusion.
    pub references: Vec<ReferenceView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecommendationView {
    pub text: String,
    pub caution: Option<String>,
    pub claims: Vec<ClaimView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct NextEvidence {
    pub request: String,
    pub definition: String,
    pub title: String,
    pub why: String,
    pub rationale: String,
    pub collect: Vec<CollectView>,
    pub window: String,
    pub group_by: Vec<String>,
    pub distinguishes: Vec<String>,
    pub expects: Vec<String>,
    /// Evidence already received for it, and what is still missing.
    pub received: Vec<String>,
    pub missing: Vec<String>,
    pub optional: bool,
    /// Reading the PromQL queries with `kyumei-collect`, when the request has
    /// any and a window.
    pub collector: Option<CollectorView>,
    /// Why its PromQL queries can't be read yet, when they can't.
    pub collect_blocked: Option<String>,
    /// Runbooks, playbook sections and dashboard panels for this request.
    pub references: Vec<ReferenceView>,
}

/// The request's PromQL queries as one `kyumei-collect` command.
#[derive(Clone, Debug, Serialize)]
pub struct CollectorView {
    /// The command to copy: the request code, then a `--set` for each value
    /// Kyumei does not know.
    pub command: String,
    pub unfilled: Vec<String>,
    /// The values still needed, as the context form names them.
    pub missing: Option<String>,
    pub queries: usize,
    pub step: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CollectView {
    pub method: String,
    pub label: String,
    pub body: String,
    pub applies_to: Option<String>,
    pub unit: Option<String>,
    pub unfilled: Vec<String>,
    pub claims: Vec<ClaimView>,
    /// The witness that lets an empty result read as zero, and the claim why.
    pub witness: Option<(String, ClaimView)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ClaimView {
    pub id: String,
    /// The pack it is read from, for a link to its page there.
    pub pack: String,
    pub name: String,
    pub category: String,
    pub verified: bool,
    pub applicability: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FindingView {
    pub id: String,
    pub hypothesis: String,
    pub hypothesis_id: String,
    pub effect: Effect,
    pub statement: String,
    pub basis: Vec<BasisRow>,
    pub claims: Vec<ClaimView>,
    pub rests_on_assertion: bool,
    /// A simulated fact is among those it rests on.
    pub rests_on_simulation: bool,
    pub rule: String,
    pub condition: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct BasisRow {
    pub signal: String,
    pub statistic: String,
    pub value: String,
    pub status: EpistemicStatus,
    pub evidence: Vec<String>,
    pub observations: Vec<String>,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvidenceRow {
    pub id: String,
    pub submitted: String,
    pub submitted_utc: String,
    pub form: String,
    pub origin: String,
    pub responds_to: Option<String>,
    pub subject: String,
    pub bound: Vec<String>,
    pub unbound: usize,
    pub cautions: usize,
    pub corrected: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RequestRow {
    pub id: String,
    pub title: String,
    pub status: String,
    pub issued: String,
    pub issued_utc: String,
    pub responses: Vec<String>,
}

/// A request and the evidence that answered it.
#[derive(Clone, Debug, Serialize)]
pub struct AnsweredView {
    pub request: String,
    pub title: String,
    pub evidence: Vec<String>,
    /// `14:21`, in the workspace's zone.
    pub at: String,
    pub at_utc: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrailRow {
    pub time: String,
    /// The same moment in UTC, to the second, for a tooltip.
    pub utc: String,
    pub kind: &'static str,
    pub text: String,
    pub refs: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiagramView {
    pub kind: DiagramKind,
    pub title: String,
    pub svg: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExplanationMeta {
    pub author: String,
    pub status: String,
    pub notes: Vec<String>,
}

/// Build the workspace, with times in UTC. `assessment` is the current
/// assessment of `snapshot.investigation`.
pub fn build(
    kb: &KnowledgeBase,
    snapshot: &InvestigationSnapshot,
    assessment: &Assessment,
) -> Workspace {
    build_with(kb, snapshot, assessment, None, &DisplayZone::utc())
}

/// The workspace with times in `zone`, and dashboard panels linked into the
/// operator's Grafana at `grafana`.
pub fn build_with(
    kb: &KnowledgeBase,
    snapshot: &InvestigationSnapshot,
    assessment: &Assessment,
    grafana: Option<&str>,
    zone: &DisplayZone,
) -> Workspace {
    let inv = &snapshot.investigation;
    let (pack, fit, diagnostic) = match kyumei_diagnostics::select(kb, inv) {
        Ok(x) => x,
        Err(_) => return empty(inv, zone),
    };
    // Kyumei writes times in UTC; they are shown in `zone`.
    let reference = time_reference(inv);
    let text = |s: &str| localize(s, zone, reference);
    let links = GrafanaLinks::for_investigation(grafana, inv);
    let families = inv.environment.sdk_families();
    let references = |target: ReferenceTarget<'_>| -> Vec<ReferenceView> {
        diagnostic.map_or_else(Vec::new, |d| {
            reference_views(pack, &d.id, target, &families, &links)
        })
    };
    // Hypotheses and the symptom link to what to read; panels belong with requests.
    let documents = |target: ReferenceTarget<'_>| -> Vec<ReferenceView> {
        references(target)
            .into_iter()
            .filter(|r| r.panel.is_none())
            .collect()
    };
    let version = inv.environment.server_version.as_ref().map(|v| &v.value);
    let claim = |id: &ClaimId| claim_view(pack, id, version);

    let deterministic = diagnostic.map(|d| explain::deterministic(d, assessment, inv));
    let explanation: Option<&DiagnosticExplanation> = snapshot
        .model_explanation
        .as_ref()
        .or(deterministic.as_ref());

    let title_of = |h: &HypothesisId| {
        diagnostic
            .and_then(|d| d.hypothesis(h))
            .map_or(h.to_string(), |x| x.title.clone())
    };

    // Situation.
    let situation = Situation {
        symptom: inv.symptom.description.clone(),
        symptom_title: inv
            .symptom
            .classification
            .as_ref()
            .and_then(|c| pack.diagnostic_for(&c.signature))
            .map(|(_, s)| s.title.clone()),
        classification: inv
            .symptom
            .classification
            .as_ref()
            .map(|c| c.basis.describe()),
        scope: vec![
            (
                "Namespace".to_owned(),
                list_or_unknown(&inv.scope.namespaces),
            ),
            (
                "Task Queue".to_owned(),
                list_or_unknown(&inv.scope.task_queues),
            ),
        ],
        window: zone.incident_window(&inv.window),
        window_utc: inv.window.display(),
        environment: environment_rows(inv),
        knowledge: format!(
            "{} v{} — {}",
            pack.manifest.title,
            pack.manifest.version,
            match fit {
                kyumei_knowledge::PackFit::Verified =>
                    "verified for this server version".to_owned(),
                kyumei_knowledge::PackFit::SameMinor =>
                    "same minor version; this patch release unverified".to_owned(),
                kyumei_knowledge::PackFit::VersionUnknown => format!(
                    "server version unknown; verified for {} only",
                    pack.manifest.verified_server_versions.join(", ")
                ),
                kyumei_knowledge::PackFit::Swept => format!(
                    "settings and metrics checked for this release; behaviours read for {}",
                    pack.manifest.verified_server_versions.join(", ")
                ),
                kyumei_knowledge::PackFit::OutsideFamily =>
                    "outside this pack's verified versions".to_owned(),
            }
        ),
        knowledge_verified: fit == kyumei_knowledge::PackFit::Verified,
        knowledge_pack: pack.manifest.id.to_string(),
        references: documents(ReferenceTarget::Diagnostic),
    };

    // What Kyumei knows.
    let knows: Vec<FactRow> = assessment
        .facts
        .facts
        .values()
        .map(|f| fact_row(diagnostic, f, &text))
        .collect();

    let row = |h: &kyumei_diagnostics::HypothesisOutcome| {
        let def = diagnostic.and_then(|d| d.hypothesis(&h.id));
        HypothesisRow {
            id: h.id.to_string(),
            title: title_of(&h.id),
            statement: def.map_or_else(String::new, |d| text(&d.statement)),
            state: h.state,
            contested: h.contested,
            stage: def
                .and_then(|d| diagnostic.and_then(|dd| dd.stage(&d.stage)))
                .map_or_else(String::new, |s| s.title.clone()),
            findings: h.findings.iter().map(ToString::to_string).collect(),
            references: documents(ReferenceTarget::Hypothesis(&h.id)),
        }
    };
    let leading = assessment
        .hypotheses
        .iter()
        .filter(|h| h.state.is_leading())
        .map(row)
        .collect();
    let open = assessment
        .hypotheses
        .iter()
        .filter(|h| {
            matches!(
                h.state,
                HypothesisState::Candidate | HypothesisState::Weakened
            )
        })
        .map(row)
        .collect();
    let eliminated = assessment
        .hypotheses
        .iter()
        .filter(|h| h.state == HypothesisState::Eliminated)
        .map(row)
        .collect();
    let uncertainties = assessment
        .uncertainties
        .iter()
        .map(|u| UncertaintyRow {
            kind: u.kind,
            label: u.kind.label(),
            text: text(&u.text),
            refs: u.refs.iter().map(ref_label).collect(),
        })
        .collect();
    let provisional = inv.status != InvestigationStatus::Concluded;
    let conclusion = assessment.conclusion.as_ref().map(|c| ConclusionView {
        title: c.title.clone(),
        statement: text(&c.statement),
        provisional,
        recommendations: c
            .recommendations
            .iter()
            .map(|r| RecommendationView {
                text: text(&r.text),
                caution: r.caution.as_deref().map(&text),
                claims: r.claims.iter().map(&claim).collect(),
            })
            .collect(),
        findings: c.findings.iter().map(ToString::to_string).collect(),
        references: references(ReferenceTarget::Conclusion(&c.id)),
        evidence: c.evidence.iter().map(ToString::to_string).collect(),
    });

    // Findings.
    let findings = assessment
        .findings
        .iter()
        .map(|f| {
            let rule = diagnostic.and_then(|d| d.rule(&f.rule));
            let explained = explanation
                .and_then(|e| e.findings.iter().find(|x| x.finding == f.id))
                .map(|x| x.text.clone());
            FindingView {
                id: f.id.to_string(),
                hypothesis: title_of(&f.hypothesis),
                hypothesis_id: f.hypothesis.to_string(),
                effect: f.effect,
                statement: text(&explained.unwrap_or_else(|| f.statement.clone())),
                basis: f
                    .basis
                    .iter()
                    .map(|b| BasisRow {
                        signal: diagnostic
                            .and_then(|d| d.signal(&b.signal))
                            .map_or(b.signal.to_string(), |s| s.title.clone()),
                        statistic: b.statistic.label().to_owned(),
                        value: b.display(),
                        status: b.status,
                        evidence: b.evidence.iter().map(ToString::to_string).collect(),
                        observations: b.observations.iter().map(ToString::to_string).collect(),
                        sources: sources_for(inv, &b.observations, zone),
                    })
                    .collect(),
                claims: f.claims.iter().map(&claim).collect(),
                rests_on_assertion: f.rests_on_assertion(),
                rests_on_simulation: f.rests_on_simulation(),
                rule: f.rule.to_string(),
                condition: rule.map_or_else(String::new, |r| r.when.describe()),
            }
        })
        .collect();

    // Next evidence.
    let next = inv.current_request().map(|r| {
        let responded: Vec<String> = r.responses.iter().map(ToString::to_string).collect();
        let missing: Vec<String> = r
            .expected_signals
            .iter()
            .filter(|s| assessment.facts.get(s).is_none())
            .map(|s| {
                diagnostic
                    .and_then(|d| d.signal(s))
                    .map_or(s.to_string(), |x| x.title.clone())
            })
            .collect();
        let plan = assessment
            .plans
            .iter()
            .find(|p| p.definition == r.definition);
        NextEvidence {
            request: r.id.to_string(),
            definition: r.definition.to_string(),
            title: r.title.clone(),
            why: text(&r.why),
            rationale: plan.map_or_else(String::new, |p| text(&p.rationale)),
            collect: r
                .collect
                .iter()
                .map(|c| CollectView {
                    method: c.method.label().to_owned(),
                    label: c.label.clone(),
                    body: c.body.clone(),
                    applies_to: c.applies_to.clone(),
                    unit: c.unit.map(|u| u.to_string()),
                    unfilled: c.unfilled.clone(),
                    claims: c.claims.iter().map(&claim).collect(),
                    witness: c
                        .absent_reads_zero
                        .as_ref()
                        .map(|z| (z.witness.clone(), claim(&z.because))),
                })
                .collect(),
            window: text(&r.window.text),
            group_by: r.group_by.clone(),
            distinguishes: r.distinguishes.iter().map(&title_of).collect(),
            expects: r
                .expected_signals
                .iter()
                .map(|s| {
                    diagnostic
                        .and_then(|d| d.signal(s))
                        .map_or(s.to_string(), |x| x.title.clone())
                })
                .collect(),
            received: responded,
            missing,
            optional: inv.status == InvestigationStatus::Concluded,
            references: references(ReferenceTarget::Request(&r.definition)),
            collector: CollectionPlan::for_request(r).map(|p| CollectorView {
                command: p.command(&inv.id, &r.id),
                missing: p.missing(),
                queries: p.queries.len(),
                step: p.step_text(),
                unfilled: p.unfilled,
            }),
            collect_blocked: (r.window.window.is_none()
                && r.collect
                    .iter()
                    .any(|c| c.method == kyumei_domain::CollectionMethod::Promql))
            .then(|| {
                match (inv.window.start, inv.window.end) {
                    (Some(_), None) => "the incident window has no end",
                    (None, Some(_)) => "the incident window has no start",
                    _ => "the incident window is unknown",
                }
                .to_owned()
            }),
        }
    });
    let last_answered = inv
        .trail()
        .iter()
        .rev()
        .find_map(|t| match &t.item {
            TrailItem::RequestAnswered { request } => Some((request.clone(), t.at)),
            _ => None,
        })
        .and_then(|(id, at)| {
            inv.request(&id).map(|r| AnsweredView {
                request: r.id.to_string(),
                title: r.title.clone(),
                evidence: r.responses.iter().map(ToString::to_string).collect(),
                at: zone.clock(at),
                at_utc: utc(at),
            })
        });

    let status = match (inv.status, &assessment.conclusion, &next) {
        (InvestigationStatus::Concluded, _, _) => StatusView {
            token: "concluded",
            label: "Diagnosis reached".to_owned(),
        },
        (_, Some(_), _) => StatusView {
            token: "provisional",
            label: "Diagnosis supportable — completing the path".to_owned(),
        },
        (_, None, Some(_)) => StatusView {
            token: "awaiting",
            label: "Awaiting evidence".to_owned(),
        },
        (_, None, None) => StatusView {
            token: "stalled",
            label: "No further evidence defined".to_owned(),
        },
    };

    let evidence = inv
        .evidence
        .iter()
        .map(|e| evidence_row(inv, e, zone))
        .collect();
    let requests = inv
        .requests
        .iter()
        .map(|r| RequestRow {
            id: r.id.to_string(),
            title: r.title.clone(),
            status: match &r.status {
                RequestStatus::Declined { reason } => format!("unavailable: {reason}"),
                other => other.label().to_owned(),
            },
            issued: zone.clock(r.issued_at),
            issued_utc: utc(r.issued_at),
            responses: r.responses.iter().map(ToString::to_string).collect(),
        })
        .collect();

    let diagrams = explanation
        .map(|e| {
            e.diagrams
                .iter()
                .filter_map(|d| {
                    kyumei_diagrams::render(d).ok().map(|r| DiagramView {
                        kind: d.kind,
                        title: d.title.clone(),
                        svg: r.svg,
                        text: r.text,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let summary = explanation.map_or_else(String::new, |e| text(&e.summary));
    let explanation_meta = ExplanationMeta {
        author: explanation.map_or_else(|| "none".to_owned(), |e| e.author.describe()),
        status: match &snapshot.explanation {
            ExplanationStatus::Deterministic => "Written by the deterministic engine".to_owned(),
            ExplanationStatus::Pending { .. } => {
                "A model is rewording this; deterministic text shown meanwhile".to_owned()
            }
            ExplanationStatus::Current { .. } => {
                "Model wording, checked against the evidence".to_owned()
            }
            ExplanationStatus::Fallback { reason, .. } => {
                format!(
                    "Model wording rejected ({}); deterministic text shown",
                    text(reason)
                )
            }
        },
        notes: explanation
            .map(|e| e.notes.iter().map(|n| text(n)).collect())
            .unwrap_or_default(),
    };

    Workspace {
        id: inv.id.to_string(),
        title: inv.title.clone(),
        status,
        situation,
        assessment: AssessmentView {
            summary,
            knows,
            leading,
            open,
            eliminated,
            uncertainties,
            conclusion,
        },
        next,
        findings,
        evidence,
        requests,
        last_answered,
        trail: trail(inv, diagnostic, zone),
        diagrams,
        explanation: explanation_meta,
        runtime: snapshot.runtime.kind.clone(),
        runtime_note: snapshot.runtime.note.as_deref().map(&text),
        signals: diagnostic
            .map(|d| {
                d.signals
                    .iter()
                    .map(|s| (s.id.to_string(), s.title.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        zone: zone_view(inv, zone),
    }
}

/// The instant whose date a time written without one is taken to fall on.
fn time_reference(inv: &Investigation) -> jiff::Timestamp {
    inv.window.start.unwrap_or(inv.opened_at)
}

fn zone_view(inv: &Investigation, zone: &DisplayZone) -> ZoneView {
    let latest = inv.events.last().map_or(inv.opened_at, |e| e.at);
    ZoneView {
        name: zone.name().to_owned(),
        label: zone.label(latest),
    }
}

fn empty(inv: &Investigation, zone: &DisplayZone) -> Workspace {
    Workspace {
        id: inv.id.to_string(),
        title: inv.title.clone(),
        status: StatusView {
            token: "stalled",
            label: "No knowledge loaded".to_owned(),
        },
        situation: Situation {
            symptom: inv.symptom.description.clone(),
            symptom_title: None,
            classification: None,
            scope: vec![],
            window: zone.incident_window(&inv.window),
            window_utc: inv.window.display(),
            environment: vec![],
            knowledge: "none".to_owned(),
            knowledge_verified: false,
            knowledge_pack: String::new(),
            references: vec![],
        },
        assessment: AssessmentView {
            summary: String::new(),
            knows: vec![],
            leading: vec![],
            open: vec![],
            eliminated: vec![],
            uncertainties: vec![],
            conclusion: None,
        },
        next: None,
        findings: vec![],
        evidence: vec![],
        requests: vec![],
        last_answered: None,
        trail: vec![],
        diagrams: vec![],
        explanation: ExplanationMeta {
            author: "none".to_owned(),
            status: String::new(),
            notes: vec![],
        },
        runtime: String::new(),
        runtime_note: None,
        signals: vec![],
        zone: zone_view(inv, zone),
    }
}

pub fn claim_view(
    pack: &Pack,
    id: &ClaimId,
    version: Option<&kyumei_domain::ServerVersion>,
) -> ClaimView {
    match pack.claim(id) {
        Some(c) => ClaimView {
            id: id.to_string(),
            pack: pack.manifest.id.to_string(),
            name: c.name(),
            category: c.category().label().to_owned(),
            verified: c.verification().is_verified(),
            applicability: applicability_text(c, pack.applicability(id, version)),
        },
        None => ClaimView {
            id: id.to_string(),
            pack: pack.manifest.id.to_string(),
            name: id.to_string(),
            category: "unknown".to_owned(),
            verified: false,
            applicability: "not in the knowledge pack".to_owned(),
        },
    }
}

fn applicability_text(c: &Claim, a: Applicability) -> String {
    if matches!(c, Claim::Heuristic(_)) {
        return "Kyumei heuristic".to_owned();
    }
    if !c.verification().is_verified() {
        return "unverified".to_owned();
    }
    match a {
        Applicability::NotServerVersioned => {
            let scope = c.scope();
            if scope.sdk.is_empty() {
                "verified".to_owned()
            } else {
                format!(
                    "verified against {}",
                    scope
                        .sdk
                        .iter()
                        .map(|s| s.revision.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        other => other.describe(),
    }
}

fn fact_row(d: Option<&DiagnosticDef>, f: &Fact, text: &impl Fn(&str) -> String) -> FactRow {
    let title = d
        .and_then(|d| d.signal(&f.signal))
        .map_or(f.signal.to_string(), |s| s.title.clone());
    let u = f.unit;
    let summary = if let Some(c) = &f.categorical {
        format!("= {c}")
    } else if let Some(t) = f.values.get(&Statistic::Typical) {
        format!("about {}", u.format_value(*t))
    } else {
        let mut parts = Vec::new();
        let samples = f.values.get(&Statistic::Samples).copied().unwrap_or(0.0);
        if samples <= 1.0 {
            if let Some(v) = f.get(Statistic::Last) {
                parts.push(format!("{} (one reading)", u.format_value(v)));
            }
        } else {
            if let (Some(min), Some(max)) = (f.get(Statistic::Min), f.get(Statistic::Max)) {
                if (max - min).abs() < f64::EPSILON {
                    parts.push(format!("{} throughout", u.format_value(min)));
                } else {
                    parts.push(format!("{} – {}", u.format_value(min), u.format_value(max)));
                }
            }
            if let Some(b) = f.get(Statistic::Baseline) {
                parts.push(format!("baseline {}", u.format_value(b)));
            }
            parts.push(format!("{samples:.0} samples"));
        }
        parts.join(", ")
    };
    let note = match f.fit {
        kyumei_diagnostics::WindowFit::Within => None,
        kyumei_diagnostics::WindowFit::IncidentUnknown => {
            Some("incident window unknown: all samples used".to_owned())
        }
        kyumei_diagnostics::WindowFit::Timeless => None,
    }
    .map(|n| text(&n));
    FactRow {
        signal: f.signal.to_string(),
        title,
        summary,
        status: f.status,
        evidence: f.evidence.iter().map(ToString::to_string).collect(),
        note,
    }
}

fn sources_for(
    inv: &Investigation,
    observations: &[kyumei_domain::ObservationId],
    zone: &DisplayZone,
) -> Vec<String> {
    let mut out = Vec::new();
    for o in observations {
        for e in &inv.evidence {
            if let Some(obs) = e.observation(o) {
                let origin = e.origin.describe();
                let window = obs
                    .window
                    .map(|w| zone.window(&w))
                    .unwrap_or_else(|| "undated".to_owned());
                let line = format!(
                    "{} — {} ({}), {}",
                    e.id,
                    e.interpretation.form.label(),
                    origin,
                    window
                );
                if !out.contains(&line) {
                    out.push(line);
                }
            }
        }
    }
    out
}

fn evidence_row(inv: &Investigation, e: &Evidence, zone: &DisplayZone) -> EvidenceRow {
    let effective = e.effective_observations(&inv.corrections);
    let bound: Vec<String> = effective
        .iter()
        .filter(|o| !o.excluded)
        .filter_map(|o| o.observation.binding.as_ref().map(|b| b.signal.to_string()))
        .fold(Vec::new(), |mut acc, s| {
            if !acc.contains(&s) {
                acc.push(s);
            }
            acc
        });
    EvidenceRow {
        id: e.id.to_string(),
        submitted: zone.clock(e.submitted_at),
        submitted_utc: utc(e.submitted_at),
        form: e.interpretation.form.label().to_owned(),
        origin: e.origin.describe(),
        responds_to: e.responds_to.as_ref().map(ToString::to_string),
        subject: e
            .interpretation
            .observations
            .iter()
            .find_map(|o| {
                o.subject
                    .metric
                    .clone()
                    .or_else(|| o.subject.description.clone())
            })
            .unwrap_or_else(|| e.interpretation.form.label().to_owned()),
        bound,
        unbound: effective
            .iter()
            .filter(|o| {
                o.observation.binding.is_none()
                    && !matches!(
                        o.observation.value,
                        kyumei_domain::ObservedValue::Text { .. }
                    )
            })
            .count(),
        cautions: e
            .interpretation
            .notes
            .iter()
            .filter(|n| n.severity >= kyumei_domain::NoteSeverity::Caution)
            .count(),
        corrected: inv.corrections.iter().any(|c| c.evidence == e.id),
    }
}

/// The audit trail as rows of text, with times in `zone`.
pub fn trail(inv: &Investigation, d: Option<&DiagnosticDef>, zone: &DisplayZone) -> Vec<TrailRow> {
    let reference = time_reference(inv);
    let localized = |s: String| localize(&s, zone, reference);
    let title = |h: &HypothesisId| {
        d.and_then(|d| d.hypothesis(h))
            .map_or(h.to_string(), |x| x.title.clone())
    };
    let mut rows: Vec<TrailRow> = Vec::new();
    let mut considered: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for entry in inv.trail() {
        let time = zone.clock(entry.at);
        let (kind, text, refs): (&'static str, String, Vec<String>) = match &entry.item {
            TrailItem::SymptomRecorded { evidence } => (
                "symptom",
                "Symptom recorded".to_owned(),
                vec![evidence.to_string()],
            ),
            TrailItem::ContextAmended { fields } => (
                "context",
                format!("Context amended: {}", fields.join(", ")),
                vec![],
            ),
            TrailItem::EvidenceRequested { request, title, .. } => (
                "request",
                format!("Requested: {title}"),
                vec![request.to_string()],
            ),
            TrailItem::EvidenceSupplied {
                evidence,
                form,
                subject,
                responds_to,
            } => (
                "evidence",
                {
                    let informs: Vec<String> = inv
                        .evidence(evidence)
                        .map(|e| {
                            let mut titles: Vec<String> = Vec::new();
                            for o in &e.interpretation.observations {
                                if let Some(b) = &o.binding {
                                    let t = d
                                        .and_then(|d| d.signal(&b.signal))
                                        .map_or(b.signal.to_string(), |s| s.title.clone());
                                    if !titles.contains(&t) {
                                        titles.push(t);
                                    }
                                }
                            }
                            titles
                        })
                        .unwrap_or_default();
                    if informs.is_empty() {
                        format!(
                            "{evidence} supplied: {subject} ({})",
                            form.label().to_lowercase()
                        )
                    } else {
                        format!(
                            "{evidence} supplied ({}): {}",
                            form.label().to_lowercase(),
                            informs.join("; ")
                        )
                    }
                },
                std::iter::once(evidence.to_string())
                    .chain(responds_to.iter().map(ToString::to_string))
                    .collect(),
            ),
            TrailItem::InterpretationCorrected {
                evidence,
                correction,
                change,
            } => (
                "correction",
                format!("Interpretation corrected: {change}"),
                vec![evidence.to_string(), correction.to_string()],
            ),
            TrailItem::RequestDeclined { request, reason } => (
                "declined",
                format!("Unavailable: {reason}"),
                vec![request.to_string()],
            ),
            TrailItem::RequestSuperseded { request } => (
                "superseded",
                "Request superseded".to_owned(),
                vec![request.to_string()],
            ),
            TrailItem::RequestAnswered { request } => (
                "answered",
                "Request answered".to_owned(),
                vec![request.to_string()],
            ),
            TrailItem::HypothesisConsidered { hypothesis, state } => {
                if *state == HypothesisState::Candidate {
                    considered
                        .entry(entry.seq)
                        .or_default()
                        .push(title(hypothesis));
                    continue;
                }
                (
                    "hypothesis",
                    format!("{} considered: {}", title(hypothesis), state.label()),
                    vec![hypothesis.to_string()],
                )
            }
            TrailItem::HypothesisChanged {
                hypothesis,
                from,
                to,
                because,
            } => (
                "transition",
                match from {
                    Some(f) => format!("{} {} (was {})", title(hypothesis), to.label(), f.label()),
                    None => format!("{} {}", title(hypothesis), to.label()),
                },
                std::iter::once(hypothesis.to_string())
                    .chain(because.iter().map(ToString::to_string))
                    .collect(),
            ),
            TrailItem::ExplanationRecorded { author } => (
                "explanation",
                format!("Explanation written ({author})"),
                vec![],
            ),
            TrailItem::Concluded { conclusion, title } => (
                "conclusion",
                format!("Diagnosis: {title}"),
                vec![conclusion.to_string()],
            ),
            TrailItem::ConclusionWithdrawn { reason } => (
                "withdrawn",
                format!("Diagnosis withdrawn: {reason}"),
                vec![],
            ),
        };
        rows.push(TrailRow {
            time,
            utc: utc(entry.at),
            kind,
            text: localized(text),
            refs,
        });
    }
    // Newly considered hypotheses are collapsed into one row per assessment,
    // placed before the request that assessment issued.
    for (seq, names) in considered.into_iter().rev() {
        let (at, at_utc) = inv
            .events
            .iter()
            .find(|e| e.seq == seq)
            .map(|e| (zone.clock(e.at), utc(e.at)))
            .unwrap_or_default();
        let position = inv
            .trail()
            .iter()
            .filter(|t| {
                !matches!(
                    t.item,
                    TrailItem::HypothesisConsidered {
                        state: HypothesisState::Candidate,
                        ..
                    }
                )
            })
            .position(|t| t.seq > seq)
            .unwrap_or(rows.len())
            .min(rows.len());
        rows.insert(
            position,
            TrailRow {
                time: at,
                utc: at_utc,
                kind: "hypothesis",
                text: if names.len() == 1 {
                    format!("Considering {}", names[0])
                } else {
                    format!("Considering {} hypotheses", names.len())
                },
                refs: vec![],
            },
        );
    }
    rows
}

fn environment_rows(inv: &Investigation) -> Vec<EnvRow> {
    let env = &inv.environment;
    let status = |s: EpistemicStatus| s.token();
    let mut rows = vec![EnvRow {
        label: "Temporal Server".to_owned(),
        value: env
            .server_version
            .as_ref()
            .map_or_else(|| "unknown".to_owned(), |v| v.value.to_string()),
        status: env
            .server_version
            .as_ref()
            .map_or("unknown", |v| status(v.status)),
    }];
    if env.sdks.is_empty() {
        rows.push(EnvRow {
            label: "Worker SDK".to_owned(),
            value: "unknown".to_owned(),
            status: "unknown",
        });
    }
    for s in &env.sdks {
        rows.push(EnvRow {
            label: "Worker SDK".to_owned(),
            value: s.value.to_string(),
            status: status(s.status),
        });
    }
    if let Some(p) = &env.profile {
        rows.push(EnvRow {
            label: "Profile".to_owned(),
            value: p.clone(),
            status: "asserted",
        });
    }
    match &env.topology {
        Some(t) => {
            let v = &t.value;
            let mut parts = vec![v.deployment.label().to_owned()];
            for (n, label) in [
                (v.frontend_hosts, "Frontend"),
                (v.history_hosts, "History"),
                (v.matching_hosts, "Matching"),
                (v.worker_processes, "Worker processes"),
            ] {
                if let Some(n) = n {
                    parts.push(format!("{n} {label}"));
                }
            }
            if let Some(p) = &v.persistence {
                parts.push(p.clone());
            }
            rows.push(EnvRow {
                label: "Topology".to_owned(),
                value: parts.join(" · "),
                status: status(t.status),
            });
        }
        None => rows.push(EnvRow {
            label: "Topology".to_owned(),
            value: "unknown".to_owned(),
            status: "unknown",
        }),
    }
    rows
}

fn list_or_unknown(v: &[String]) -> String {
    if v.is_empty() {
        "unknown".to_owned()
    } else {
        v.join(", ")
    }
}

pub fn ref_label(r: &DomainRef) -> String {
    match r {
        DomainRef::Evidence(id) => id.to_string(),
        DomainRef::Observation(id) => id.to_string(),
        DomainRef::Hypothesis(id) => id.to_string(),
        DomainRef::Finding(id) => id.to_string(),
        DomainRef::Request(id) => id.to_string(),
        DomainRef::Claim(id) => id.to_string(),
        DomainRef::Conclusion(id) => id.to_string(),
        DomainRef::Signal(id) => id.to_string(),
    }
}

/// `2026-10-14 13:21:05 UTC`.
fn utc(at: jiff::Timestamp) -> String {
    DisplayZone::utc().date_time_seconds(at)
}

/// Evidence IDs a set of findings rests on.
pub fn evidence_of(findings: &[kyumei_domain::DiagnosticFinding]) -> Vec<EvidenceId> {
    let mut ids: Vec<EvidenceId> = findings.iter().flat_map(|f| f.evidence()).collect();
    ids.sort();
    ids.dedup();
    ids
}
