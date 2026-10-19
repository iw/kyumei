//! Diagrams derived from an assessment.
//!
//! Every node's status comes from the assessment itself, so a diagram cannot
//! contradict the textual diagnosis rendered from the same assessment.

use std::collections::BTreeMap;

use kyumei_diagrams::{
    Annotation, Diagram, DiagramKind, Edge, Node, NodeId, NodeRole, NodeStatus, Relation,
};
use kyumei_domain::{
    DomainRef, Effect, EpistemicStatus, HypothesisId, HypothesisState, Investigation, SignalId,
    Statistic,
};
use kyumei_knowledge::{DiagnosticDef, Truth};

use crate::{Assessment, evaluate::Lookup};

pub fn status_of(state: HypothesisState, contested: bool) -> NodeStatus {
    if contested && !state.is_resolved() {
        return NodeStatus::Open;
    }
    match state {
        HypothesisState::Candidate => NodeStatus::Open,
        HypothesisState::Supported => NodeStatus::Supported,
        HypothesisState::Weakened => NodeStatus::Weakened,
        HypothesisState::Eliminated => NodeStatus::Eliminated,
        HypothesisState::Established => NodeStatus::Established,
    }
}

/// All three diagrams, for the workspace and exports.
pub fn all(d: &DiagnosticDef, a: &Assessment, investigation: &Investigation) -> Vec<Diagram> {
    let mut out = vec![diagnostic_path(d, a, investigation), system_path(d, a)];
    if !a.findings.is_empty() {
        out.push(evidence_relationship(d, a, None));
    }
    out
}

/// The reasoning path: one question per stage, answered by its primary
/// hypothesis.
pub fn diagnostic_path(
    d: &DiagnosticDef,
    a: &Assessment,
    investigation: &Investigation,
) -> Diagram {
    let symptom = investigation
        .symptom
        .classification
        .as_ref()
        .and_then(|c| d.symptoms.iter().find(|s| s.id == c.signature));
    let symptom_title = symptom.map_or_else(
        || investigation.symptom.description.clone(),
        |s| s.title.clone(),
    );
    // The magnitude of the symptom the description matched, else the first.
    let magnitude = symptom
        .or_else(|| d.symptoms.first())
        .and_then(|s| a.facts.get(&s.signal))
        .and_then(|f| {
            f.get(Statistic::Max).map(|v| {
                let who = match f.status {
                    EpistemicStatus::ObservedFact => "observed",
                    EpistemicStatus::Simulated => "simulated",
                    _ => "operator",
                };
                format!("{who}: up to {}", f.unit.format_value(v))
            })
        });
    let mut nodes = vec![Node {
        id: NodeId::new("symptom"),
        label: truncate(&symptom_title, 90),
        detail: magnitude,
        role: NodeRole::Symptom,
        status: NodeStatus::Neutral,
        refers_to: vec![DomainRef::Evidence(investigation.symptom.evidence.clone())],
    }];
    let mut edges = Vec::new();
    let mut previous = (NodeId::new("symptom"), None::<NodeStatus>);
    let mut stages: Vec<_> = d.stages.iter().collect();
    stages.sort_by_key(|s| s.order);
    let states: BTreeMap<HypothesisId, HypothesisState> = a
        .hypotheses
        .iter()
        .map(|o| (o.id.clone(), o.state))
        .collect();
    let lookup = Lookup {
        facts: &a.facts,
        states: &states,
    };
    for stage in stages {
        let Some(primary) = d
            .hypotheses
            .iter()
            .find(|h| h.stage == stage.id && h.primary)
        else {
            continue;
        };
        // A question whose precondition the evidence has ruled out is not on
        // this investigation's path.
        if a.hypothesis(&primary.id).is_none()
            && primary
                .activation
                .as_ref()
                .is_some_and(|c| c.evaluate(&lookup) == Truth::False)
        {
            continue;
        }
        let (status, detail, refs) = match a.hypothesis(&primary.id) {
            Some(o) => (
                status_of(o.state, o.contested),
                if o.contested {
                    format!("{} (contested)", primary.title)
                } else {
                    primary.title.clone()
                },
                std::iter::once(DomainRef::Hypothesis(primary.id.clone()))
                    .chain(o.findings.iter().cloned().map(DomainRef::Finding))
                    .collect(),
            ),
            None => (
                NodeStatus::Open,
                format!("{} — not yet considered", primary.title),
                vec![DomainRef::Hypothesis(primary.id.clone())],
            ),
        };
        let id = NodeId::new(format!("q-{}", stage.id));
        nodes.push(Node {
            id: id.clone(),
            label: truncate(&primary.question, 90),
            detail: Some(truncate(&detail, 120)),
            role: NodeRole::Question,
            status,
            refers_to: refs,
        });
        edges.push(Edge {
            from: previous.0.clone(),
            to: id.clone(),
            relation: Relation::LeadsTo,
            label: previous.1.map(|s| s.answer().to_owned()),
            refers_to: vec![],
        });
        previous = (id, Some(status));
    }
    if let Some(c) = &a.conclusion {
        let final_ = investigation.status == kyumei_domain::InvestigationStatus::Concluded;
        nodes.push(Node {
            id: NodeId::new("conclusion"),
            label: truncate(&c.title, 90),
            detail: Some(if final_ {
                "Diagnosis supported by the evidence trail".to_owned()
            } else {
                "Supportable; the remaining path is still being checked".to_owned()
            }),
            role: NodeRole::Conclusion,
            status: if final_ {
                NodeStatus::Established
            } else {
                NodeStatus::Supported
            },
            refers_to: vec![DomainRef::Conclusion(c.id.clone())],
        });
        edges.push(Edge {
            from: previous.0,
            to: NodeId::new("conclusion"),
            relation: Relation::LeadsTo,
            label: previous.1.map(|s| s.answer().to_owned()),
            refers_to: vec![],
        });
    }
    Diagram {
        id: "diagnostic-path".to_owned(),
        title: "Diagnostic path".to_owned(),
        kind: DiagramKind::DiagnosticPath,
        description: format!(
            "The investigation's path from the symptom ({symptom_title}) through one question per stage. Each question shows its answer: open, likely, unlikely, yes or no."
        ),
        nodes,
        edges,
        annotations: vec![],
    }
}

/// Where the problem sits on the request path.
pub fn system_path(d: &DiagnosticDef, a: &Assessment) -> Diagram {
    let mut nodes = Vec::new();
    let mut annotations = Vec::new();
    for layer in &d.path.layers {
        let related: Vec<(&kyumei_knowledge::HypothesisDef, &crate::HypothesisOutcome)> = d
            .hypotheses
            .iter()
            .filter(|h| h.layer == *layer)
            .filter_map(|h| a.hypothesis(&h.id).map(|o| (h, o)))
            .collect();
        let suspected: Vec<_> = related
            .iter()
            .filter(|(_, o)| o.state.is_leading() && !o.contested)
            .collect();
        let cleared = !related.is_empty()
            && related
                .iter()
                .all(|(_, o)| o.state == HypothesisState::Eliminated);
        let status = if !suspected.is_empty() {
            NodeStatus::Suspected
        } else if cleared {
            NodeStatus::Cleared
        } else {
            NodeStatus::Neutral
        };
        let id = NodeId::new(format!("layer-{}", layer.token()));
        let refs: Vec<DomainRef> = related
            .iter()
            .map(|(h, _)| DomainRef::Hypothesis(h.id.clone()))
            .collect();
        if let Some((h, o)) = suspected
            .iter()
            .max_by_key(|(_, o)| o.state == HypothesisState::Established)
        {
            annotations.push(Annotation {
                target: id.clone(),
                text: format!(
                    "{}: {}",
                    if o.state == HypothesisState::Established {
                        "established"
                    } else {
                        "suspected"
                    },
                    h.title
                ),
                refers_to: vec![DomainRef::Hypothesis(h.id.clone())],
            });
        } else if cleared {
            let titles: Vec<&str> = related.iter().map(|(h, _)| h.title.as_str()).collect();
            annotations.push(Annotation {
                target: id.clone(),
                text: format!("ruled out: {}", titles.join("; ")),
                refers_to: refs.clone(),
            });
        }
        nodes.push(Node {
            id,
            label: d.path.label(*layer).to_owned(),
            detail: None,
            role: NodeRole::Component,
            status,
            refers_to: refs,
        });
    }
    let edges = nodes
        .windows(2)
        .map(|w| Edge {
            from: w[0].id.clone(),
            to: w[1].id.clone(),
            relation: Relation::FlowsTo,
            label: None,
            refers_to: vec![],
        })
        .collect();
    Diagram {
        id: "system-path".to_owned(),
        title: "System path".to_owned(),
        kind: DiagramKind::SystemPath,
        description: format!(
            "{} The diagram marks where evidence places or clears the constraint.",
            d.path.description
        ),
        nodes,
        edges,
        annotations,
    }
}

/// How observations bear on hypotheses. `focus` limits it to some hypotheses.
pub fn evidence_relationship(
    d: &DiagnosticDef,
    a: &Assessment,
    focus: Option<&[HypothesisId]>,
) -> Diagram {
    let in_focus = |h: &HypothesisId| focus.is_none_or(|f| f.contains(h));
    let mut nodes: Vec<Node> = Vec::new();
    let mut edges = Vec::new();
    let mut fact_nodes: BTreeMap<SignalId, NodeId> = BTreeMap::new();
    let mut fact_stats: BTreeMap<SignalId, Vec<(Statistic, String)>> = BTreeMap::new();

    for f in a.findings.iter().filter(|f| in_focus(&f.hypothesis)) {
        for b in &f.basis {
            let stats = fact_stats.entry(b.signal.clone()).or_default();
            if !stats.iter().any(|(s, _)| *s == b.statistic) {
                stats.push((b.statistic, b.display()));
            }
        }
    }
    for (signal, stats) in &fact_stats {
        let Some(fact) = a.facts.get(signal) else {
            continue;
        };
        let title = d
            .signal(signal)
            .map_or(signal.to_string(), |s| s.title.clone());
        let values: Vec<String> = stats
            .iter()
            .map(|(s, v)| format!("{} {v}", s.label()))
            .collect();
        let id = NodeId::new(format!("fact-{signal}"));
        nodes.push(Node {
            id: id.clone(),
            label: truncate(&title, 80),
            detail: Some(truncate(&values.join(", "), 90)),
            role: NodeRole::Observation,
            status: match fact.status {
                EpistemicStatus::ObservedFact => NodeStatus::Observed,
                EpistemicStatus::Simulated => NodeStatus::Simulated,
                _ => NodeStatus::Asserted,
            },
            refers_to: fact
                .observations
                .iter()
                .cloned()
                .map(DomainRef::Observation)
                .chain(fact.evidence.iter().cloned().map(DomainRef::Evidence))
                .collect(),
        });
        fact_nodes.insert(signal.clone(), id);
    }
    for o in a
        .hypotheses
        .iter()
        .filter(|o| in_focus(&o.id) && !o.findings.is_empty())
    {
        let title = d
            .hypothesis(&o.id)
            .map_or(o.id.to_string(), |h| h.title.clone());
        nodes.push(Node {
            id: NodeId::new(format!("h-{}", o.id)),
            label: truncate(&title, 80),
            detail: Some(if o.contested {
                "contested".to_owned()
            } else {
                o.state.label().to_owned()
            }),
            role: NodeRole::Hypothesis,
            status: status_of(o.state, o.contested),
            refers_to: vec![DomainRef::Hypothesis(o.id.clone())],
        });
    }
    for f in a.findings.iter().filter(|f| in_focus(&f.hypothesis)) {
        let to = NodeId::new(format!("h-{}", f.hypothesis));
        for b in &f.basis {
            let Some(from) = fact_nodes.get(&b.signal) else {
                continue;
            };
            if edges.iter().any(|e: &Edge| &e.from == from && e.to == to) {
                continue;
            }
            edges.push(Edge {
                from: from.clone(),
                to: to.clone(),
                relation: match f.effect {
                    Effect::Supports => Relation::Supports,
                    Effect::Weakens => Relation::Weakens,
                    Effect::Eliminates => Relation::Eliminates,
                    Effect::Establishes => Relation::Establishes,
                },
                label: None,
                refers_to: vec![DomainRef::Finding(f.id.clone())],
            });
        }
    }
    Diagram {
        id: "evidence-relationship".to_owned(),
        title: "Evidence relationships".to_owned(),
        kind: DiagramKind::EvidenceRelationship,
        description: "Observations on the left, hypotheses on the right. Each line is a finding: the observation supports, weakens, eliminates or establishes the hypothesis.".to_owned(),
        nodes,
        edges,
        annotations: vec![],
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
    t.push('…');
    t
}
