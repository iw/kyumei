//! Conclusions and residual uncertainty.

use std::collections::BTreeMap;

use kyumei_domain::{
    Conclusion, DomainRef, EpistemicStatus, EvidenceId, HypothesisId, HypothesisState,
    Investigation, NoteSeverity, ObservationId, Recommendation, RequestStatus, Uncertainty,
    UncertaintyKind,
};
use kyumei_knowledge::{DiagnosticDef, PackFit, Truth, template};

use crate::{
    evaluate::{Evaluation, HypothesisOutcome, Lookup},
    facts::{FactTable, SetAsideReason},
    select::RequestPlan,
};

/// Whether the reached conclusion is final: no conclusion listed before it
/// (the more specific diagnoses) could still be reached with more evidence.
pub fn is_final(
    d: &DiagnosticDef,
    eval: &Evaluation,
    reached: &kyumei_domain::ConclusionId,
) -> bool {
    let Some(position) = d.conclusions.iter().position(|c| &c.id == reached) else {
        return false;
    };
    d.conclusions[..position]
        .iter()
        .all(|earlier| !could_hold(d, &earlier.when, &eval.outcomes, 0))
}

/// Activations that read other dormant hypotheses are followed this deep;
/// beyond it, anything is assumed possible.
const MAX_ACTIVATION_DEPTH: usize = 8;

/// Whether more evidence could make `condition` hold. Resolved hypotheses
/// are fixed; unresolved ones, and signal conditions, could still change. A
/// dormant hypothesis is in no state, and could reach one only if its
/// activation could still hold.
fn could_hold(
    d: &DiagnosticDef,
    condition: &kyumei_knowledge::Condition,
    outcomes: &BTreeMap<HypothesisId, HypothesisOutcome>,
    depth: usize,
) -> bool {
    use kyumei_knowledge::Condition as C;
    match condition {
        C::All(items) => items.iter().all(|c| could_hold(d, c, outcomes, depth)),
        C::Any(items) => items.iter().any(|c| could_hold(d, c, outcomes, depth)),
        C::Not(inner) => could_fail(d, inner, outcomes, depth),
        C::Hypothesis { hypothesis, states } => match outcomes.get(hypothesis) {
            Some(o) if o.state.is_resolved() && !o.contested => states.contains(&o.state),
            Some(_) => true,
            None => {
                depth >= MAX_ACTIVATION_DEPTH
                    || d.hypothesis(hypothesis)
                        .and_then(|h| h.activation.as_ref())
                        .is_none_or(|a| could_hold(d, a, outcomes, depth + 1))
            }
        },
        C::Compare { .. } | C::Ratio { .. } | C::Equals { .. } => true,
    }
}

/// Whether more evidence could make `condition` fail.
fn could_fail(
    d: &DiagnosticDef,
    condition: &kyumei_knowledge::Condition,
    outcomes: &BTreeMap<HypothesisId, HypothesisOutcome>,
    depth: usize,
) -> bool {
    use kyumei_knowledge::Condition as C;
    match condition {
        C::All(items) => items.iter().any(|c| could_fail(d, c, outcomes, depth)),
        C::Any(items) => items.iter().all(|c| could_fail(d, c, outcomes, depth)),
        C::Not(inner) => could_hold(d, inner, outcomes, depth),
        C::Hypothesis { hypothesis, states } => match outcomes.get(hypothesis) {
            Some(o) if o.state.is_resolved() && !o.contested => !states.contains(&o.state),
            // Unresolved, or dormant: a dormant hypothesis is in no state.
            _ => true,
        },
        C::Compare { .. } | C::Ratio { .. } | C::Equals { .. } => true,
    }
}

pub fn conclude(
    d: &DiagnosticDef,
    facts: &FactTable,
    eval: &Evaluation,
    uncertainties: &[Uncertainty],
) -> Option<Conclusion> {
    let states: BTreeMap<HypothesisId, HypothesisState> = eval
        .outcomes
        .iter()
        .map(|(k, v)| (k.clone(), v.state))
        .collect();
    let lookup = Lookup {
        facts,
        states: &states,
    };
    let def = d
        .conclusions
        .iter()
        .find(|c| c.when.evaluate(&lookup) == Truth::True)?;
    let render = |text: &str| {
        template::render(text, |ss| {
            let fact = facts.get(&ss.signal)?;
            fact.display(ss.stat, fact.unit)
        })
    };
    let mut statement = render(&def.statement);
    for clause in &def.clauses {
        if clause.when.evaluate(&lookup) == Truth::True {
            statement.push(' ');
            statement.push_str(&render(&clause.text));
        }
    }
    let by_state = |state: HypothesisState| -> Vec<HypothesisId> {
        d.hypotheses
            .iter()
            .filter(|h| eval.outcomes.get(&h.id).is_some_and(|o| o.state == state))
            .map(|h| h.id.clone())
            .collect()
    };
    let mut evidence: Vec<EvidenceId> = eval.findings.iter().flat_map(|f| f.evidence()).collect();
    evidence.sort();
    evidence.dedup();
    let mut observations: Vec<ObservationId> = eval
        .findings
        .iter()
        .flat_map(|f| f.observations())
        .collect();
    observations.sort();
    observations.dedup();
    Some(Conclusion {
        id: def.id.clone(),
        title: def.title.clone(),
        statement,
        established: by_state(HypothesisState::Established),
        supported: by_state(HypothesisState::Supported),
        eliminated: by_state(HypothesisState::Eliminated),
        findings: eval.findings.iter().map(|f| f.id.clone()).collect(),
        evidence,
        observations,
        recommendations: def
            .recommendations
            .iter()
            .map(|r| Recommendation {
                text: render(&r.text),
                caution: r.caution.clone(),
                claims: r.claims.clone(),
            })
            .collect(),
        residual_uncertainty: uncertainties.to_vec(),
    })
}

/// Everything Kyumei does not know, or knows only weakly.
pub fn uncertainties(
    d: &DiagnosticDef,
    fit: PackFit,
    verified: &[kyumei_domain::ServerVersion],
    investigation: &Investigation,
    facts: &FactTable,
    eval: &Evaluation,
    plans: &[RequestPlan],
) -> Vec<Uncertainty> {
    let mut out = Vec::new();
    let verified_list = verified
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let version = investigation
        .environment
        .server_version
        .as_ref()
        .map(|v| v.value.to_string());
    match fit {
        PackFit::Verified => {}
        PackFit::VersionUnknown => out.push(Uncertainty {
            kind: UncertaintyKind::VersionUnverified,
            text: format!(
                "The Temporal Server version is unknown. Metric names, defaults and behaviours here are verified only for {verified_list}."
            ),
            refs: vec![],
        }),
        PackFit::SameMinor => out.push(Uncertainty {
            kind: UncertaintyKind::VersionUnverified,
            text: format!(
                "Knowledge is verified for {verified_list}; Temporal Server {} is the same minor release but was not itself inspected.",
                version.clone().unwrap_or_default()
            ),
            refs: vec![],
        }),
        PackFit::Swept => out.push(Uncertainty {
            kind: UncertaintyKind::VersionUnverified,
            text: format!(
                "Temporal Server {} is covered by the knowledge pack's version sweep: the names, types and defaults of its settings and metrics are checked, but behaviours and limits were read for {verified_list}. Claims verified only for those releases are marked so.",
                version.clone().unwrap_or_default()
            ),
            refs: vec![],
        }),
        PackFit::OutsideFamily => out.push(Uncertainty {
            kind: UncertaintyKind::VersionUnverified,
            text: format!(
                "Temporal Server {} is outside this knowledge pack ({verified_list}). Treat metric names, defaults and behaviours as unverified.",
                version.unwrap_or_default()
            ),
            refs: vec![],
        }),
    }

    if !eval.stable {
        out.push(Uncertainty {
            kind: UncertaintyKind::Contested,
            text: "The diagnostic rules did not settle on stable hypothesis states; the assessment may change with more evidence.".to_owned(),
            refs: vec![],
        });
    }

    if investigation.window.as_window().is_none() {
        out.push(Uncertainty {
            kind: UncertaintyKind::MissingContext,
            text: "The incident window is unknown, so statistics cover all supplied samples rather than the incident alone.".to_owned(),
            refs: vec![],
        });
    }
    if investigation.scope.task_queues.is_empty() {
        out.push(Uncertainty {
            kind: UncertaintyKind::MissingContext,
            text: "The affected Task Queue is unknown; queries cannot be filtered to it."
                .to_owned(),
            refs: vec![],
        });
    }
    if investigation.environment.sdks.is_empty() {
        out.push(Uncertainty {
            kind: UncertaintyKind::MissingContext,
            text: "The Worker SDK is unknown. SDK metric names and duration units differ by SDK, so every variant is shown.".to_owned(),
            refs: vec![],
        });
    }

    // Open and contested hypotheses.
    for h in &d.hypotheses {
        let Some(o) = eval.outcomes.get(&h.id) else {
            continue;
        };
        if o.contested {
            out.push(Uncertainty {
                kind: UncertaintyKind::Contested,
                text: format!("Evidence both supports and undermines “{}”.", h.title),
                refs: finding_refs(o, &h.id),
            });
        } else if !o.state.is_resolved() {
            let next = plans.iter().find(|p| p.targets.contains(&h.id));
            let text = match (o.state, next) {
                (state, Some(p)) => format!(
                    "“{}” is {}; {} would distinguish it.",
                    h.title,
                    state.label(),
                    d.request(&p.definition)
                        .map_or(p.definition.to_string(), |r| r.title.clone())
                ),
                (state, None) => format!(
                    "“{}” is {}; no further evidence request is defined for it.",
                    h.title,
                    state.label()
                ),
            };
            out.push(Uncertainty {
                kind: UncertaintyKind::OpenHypothesis,
                text,
                refs: finding_refs(o, &h.id),
            });
        }
    }

    // Findings that rest on operator assertions.
    for f in &eval.findings {
        let asserted: Vec<String> = f
            .basis
            .iter()
            .filter(|b| b.status == EpistemicStatus::OperatorAssertion)
            .filter_map(|b| {
                d.signal(&b.signal)
                    .map(|s| format!("{} ({})", s.title, b.display()))
            })
            .collect();
        if !asserted.is_empty() {
            out.push(Uncertainty {
                kind: UncertaintyKind::AssertionOnly,
                text: format!(
                    "{} rests partly on operator assertions: {}.",
                    f.id,
                    asserted.join(", ")
                ),
                refs: vec![DomainRef::Finding(f.id.clone())],
            });
        }
    }

    // Asserted estimates that telemetry contradicts.
    for fact in facts.facts.values() {
        if let (Some(asserted), Some(max), Some(min)) = (
            fact.asserted_typical,
            fact.get(kyumei_domain::Statistic::Max),
            fact.get(kyumei_domain::Statistic::Min),
        ) && (asserted > max * 2.0 || asserted < min * 0.5)
        {
            let title = d
                .signal(&fact.signal)
                .map_or(fact.signal.to_string(), |s| s.title.clone());
            out.push(Uncertainty {
                kind: UncertaintyKind::Contested,
                text: format!(
                    "The operator's estimate of {title} ({}) differs from the observed range ({} to {}).",
                    fact.unit.format_value(asserted),
                    fact.unit.format_value(min),
                    fact.unit.format_value(max)
                ),
                refs: fact.superseded_assertions.iter().cloned().map(DomainRef::Observation).collect(),
            });
        }
    }

    // Evidence set aside.
    for s in &facts.set_aside {
        let (kind, relevant) = match &s.reason {
            SetAsideReason::OutsideWindow { .. } => (UncertaintyKind::WindowMismatch, true),
            SetAsideReason::UnitMismatch { .. } => (UncertaintyKind::InterpretationAssumed, true),
            SetAsideReason::Unbound => (UncertaintyKind::Unmapped, true),
            _ => (UncertaintyKind::Unmapped, false),
        };
        if relevant {
            out.push(Uncertainty {
                kind,
                text: format!("{} was set aside: {}.", s.observation, s.reason.describe()),
                refs: vec![DomainRef::Observation(s.observation.clone())],
            });
        }
    }

    // Assumptions made while interpreting evidence that facts now rely on.
    let used: std::collections::BTreeSet<&ObservationId> = facts
        .facts
        .values()
        .flat_map(|f| f.observations.iter())
        .collect();
    for e in &investigation.evidence {
        for note in &e.interpretation.notes {
            if note.severity == NoteSeverity::Caution
                && note.observation.as_ref().is_some_and(|o| used.contains(o))
                && let Some(o) = &note.observation
            {
                out.push(Uncertainty {
                    kind: UncertaintyKind::InterpretationAssumed,
                    text: format!("{o}: {}", note.text),
                    refs: vec![
                        DomainRef::Observation(o.clone()),
                        DomainRef::Evidence(e.id.clone()),
                    ],
                });
            }
        }
    }

    // Requests the operator could not satisfy.
    for r in &investigation.requests {
        if let RequestStatus::Declined { reason } = &r.status {
            out.push(Uncertainty {
                kind: UncertaintyKind::EvidenceUnavailable,
                text: format!("{} ({}) could not be obtained: {reason}", r.id, r.title),
                refs: vec![DomainRef::Request(r.id.clone())],
            });
        }
    }
    out
}

fn finding_refs(o: &HypothesisOutcome, h: &HypothesisId) -> Vec<DomainRef> {
    std::iter::once(DomainRef::Hypothesis(h.clone()))
        .chain(o.findings.iter().cloned().map(DomainRef::Finding))
        .collect()
}
