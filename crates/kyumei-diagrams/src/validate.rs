//! Structural validation. A diagram that fails validation is never rendered.

use std::collections::{BTreeMap, BTreeSet};

use crate::ir::{Diagram, DiagramKind, NodeId, NodeRole, Relation};

pub const MAX_NODES: usize = 48;
pub const MAX_LABEL: usize = 96;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind} diagram `{id}`: {}", problems.join("; "))]
pub struct DiagramError {
    pub id: String,
    pub kind: &'static str,
    pub problems: Vec<String>,
}

pub fn validate(d: &Diagram) -> Result<(), DiagramError> {
    let mut problems = Vec::new();
    if d.title.trim().is_empty() {
        problems.push("empty title".to_owned());
    }
    if d.description.trim().is_empty() {
        problems.push("a description is required for assistive technology".to_owned());
    }
    if d.nodes.is_empty() {
        problems.push("no nodes".to_owned());
    }
    if d.nodes.len() > MAX_NODES {
        problems.push(format!(
            "{} nodes exceeds the limit of {MAX_NODES}",
            d.nodes.len()
        ));
    }
    let mut ids = BTreeSet::new();
    for n in &d.nodes {
        if !ids.insert(&n.id) {
            problems.push(format!("duplicate node `{}`", n.id));
        }
        if n.label.trim().is_empty() {
            problems.push(format!("node `{}` has no label", n.id));
        }
        if n.label.chars().count() > MAX_LABEL {
            problems.push(format!("node `{}` label is too long", n.id));
        }
    }
    let role = |id: &NodeId| d.nodes.iter().find(|n| &n.id == id).map(|n| n.role);
    for e in &d.edges {
        if e.from == e.to {
            problems.push(format!("self edge on `{}`", e.from));
        }
        for end in [&e.from, &e.to] {
            if !ids.contains(end) {
                problems.push(format!("edge references unknown node `{end}`"));
            }
        }
    }
    for a in &d.annotations {
        if !ids.contains(&a.target) {
            problems.push(format!("annotation references unknown node `{}`", a.target));
        }
    }

    match d.kind {
        DiagramKind::DiagnosticPath => {
            for n in &d.nodes {
                if !matches!(
                    n.role,
                    NodeRole::Symptom
                        | NodeRole::Question
                        | NodeRole::Hypothesis
                        | NodeRole::Conclusion
                ) {
                    problems.push(format!(
                        "node `{}` role is not allowed on a diagnostic path",
                        n.id
                    ));
                }
            }
            if d.nodes
                .iter()
                .filter(|n| n.role == NodeRole::Symptom)
                .count()
                != 1
            {
                problems.push("a diagnostic path starts from exactly one symptom".to_owned());
            }
            if d.edges.iter().any(|e| e.relation != Relation::LeadsTo) {
                problems.push("diagnostic path edges must be `leads_to`".to_owned());
            }
            if has_cycle(d) {
                problems.push("a diagnostic path cannot contain a cycle".to_owned());
            }
        }
        DiagramKind::SystemPath => {
            for n in &d.nodes {
                if n.role != NodeRole::Component {
                    problems.push(format!("node `{}` must be a component", n.id));
                }
            }
            if d.edges.iter().any(|e| e.relation != Relation::FlowsTo) {
                problems.push("system path edges must be `flows_to`".to_owned());
            }
            let mut out_degree: BTreeMap<&NodeId, usize> = BTreeMap::new();
            for e in &d.edges {
                *out_degree.entry(&e.from).or_default() += 1;
            }
            if out_degree.values().any(|n| *n > 1) {
                problems.push("a system path is a single chain".to_owned());
            }
            if has_cycle(d) {
                problems.push("a system path cannot contain a cycle".to_owned());
            }
        }
        DiagramKind::EvidenceRelationship => {
            for e in &d.edges {
                if !e.relation.is_evidential() {
                    problems.push(
                        "evidence relationships must support, weaken, eliminate or establish"
                            .to_owned(),
                    );
                }
                if role(&e.from) != Some(NodeRole::Observation) {
                    problems.push(format!(
                        "edge from `{}` must start at an observation",
                        e.from
                    ));
                }
                if role(&e.to) != Some(NodeRole::Hypothesis) {
                    problems.push(format!("edge to `{}` must end at a hypothesis", e.to));
                }
            }
        }
    }

    problems.dedup();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(DiagramError {
            id: d.id.clone(),
            kind: d.kind.label(),
            problems,
        })
    }
}

fn has_cycle(d: &Diagram) -> bool {
    let mut adjacency: BTreeMap<&NodeId, Vec<&NodeId>> = BTreeMap::new();
    for e in &d.edges {
        adjacency.entry(&e.from).or_default().push(&e.to);
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Visiting,
        Done,
    }
    fn visit<'a>(
        n: &'a NodeId,
        adjacency: &BTreeMap<&'a NodeId, Vec<&'a NodeId>>,
        marks: &mut BTreeMap<&'a NodeId, Mark>,
    ) -> bool {
        match marks.get(n) {
            Some(Mark::Visiting) => return true,
            Some(Mark::Done) => return false,
            None => {}
        }
        marks.insert(n, Mark::Visiting);
        for next in adjacency.get(n).into_iter().flatten() {
            if visit(next, adjacency, marks) {
                return true;
            }
        }
        marks.insert(n, Mark::Done);
        false
    }
    let mut marks = BTreeMap::new();
    d.nodes.iter().any(|n| visit(&n.id, &adjacency, &mut marks))
}
