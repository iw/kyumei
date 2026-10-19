//! Typed diagnostic diagrams.
//!
//! Diagrams are data: semantic nodes and relationships that reference domain
//! objects (evidence, observations, hypotheses, findings). Neither a model nor
//! the diagnostic engine draws anything; Kyumei validates the structure and
//! renders it deterministically as accessible SVG or as text.
//!
//! Three kinds are approved for v1:
//!
//! * **Diagnostic path** — the reasoning path through the hypotheses.
//! * **System path** — where in the request path the suspected problem sits.
//! * **Evidence relationship** — how observations support or eliminate
//!   hypotheses.
//!
//! Evidence series are drawn as charts: lines over time, with a dashboard
//! panel's thresholds and legend, prepared by the caller.

mod chart;
mod ir;
mod svg;
mod text;
mod validate;

pub use chart::{
    Chart, ChartSeries, ChartThreshold, LegendStyle, nice_ticks, render_chart, render_chart_text,
    time_ticks,
};
pub use ir::{
    Annotation, Diagram, DiagramKind, Edge, Node, NodeId, NodeRole, NodeStatus, Relation,
};
pub use svg::render_svg;
pub use text::render_text;
pub use validate::{DiagramError, MAX_LABEL, MAX_NODES, validate};

/// Validate, then render both forms.
pub fn render(d: &Diagram) -> Result<Rendered, DiagramError> {
    validate(d)?;
    Ok(Rendered {
        svg: render_svg(d),
        text: render_text(d),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    pub svg: String,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use kyumei_domain::{DomainRef, HypothesisId};

    use super::*;

    fn node(id: &str, label: &str, role: NodeRole, status: NodeStatus) -> Node {
        Node {
            id: NodeId::new(id),
            label: label.to_owned(),
            detail: None,
            role,
            status,
            refers_to: vec![],
        }
    }

    fn edge(from: &str, to: &str, relation: Relation, label: Option<&str>) -> Edge {
        Edge {
            from: NodeId::new(from),
            to: NodeId::new(to),
            relation,
            label: label.map(str::to_owned),
            refers_to: vec![],
        }
    }

    fn path() -> Diagram {
        Diagram {
            id: "path".into(),
            title: "Diagnostic path".into(),
            kind: DiagramKind::DiagnosticPath,
            description: "From the symptom through each question.".into(),
            nodes: vec![
                node(
                    "s",
                    "High WFT schedule-to-start",
                    NodeRole::Symptom,
                    NodeStatus::Neutral,
                ),
                Node {
                    refers_to: vec![DomainRef::Hypothesis(HypothesisId::new(
                        "worker-slot-exhaustion",
                    ))],
                    ..node(
                        "q1",
                        "Worker capacity exhausted?",
                        NodeRole::Question,
                        NodeStatus::Eliminated,
                    )
                },
                node(
                    "q2",
                    "Poller starvation?",
                    NodeRole::Question,
                    NodeStatus::Open,
                ),
            ],
            edges: vec![
                edge("s", "q1", Relation::LeadsTo, None),
                edge("q1", "q2", Relation::LeadsTo, Some("NO")),
            ],
            annotations: vec![],
        }
    }

    #[test]
    fn renders_a_valid_path() {
        let d = path();
        let r = render(&d).expect("valid");
        assert!(r.svg.starts_with("<svg"));
        assert!(r.svg.contains("role=\"img\""));
        assert!(r.svg.contains("<title id="));
        assert!(r.svg.contains("Worker capacity exhausted?"));
        assert!(
            r.text
                .contains("[✕] Worker capacity exhausted? — eliminated")
        );
        assert!(r.text.contains("│ NO"));
        assert_eq!(d.references().len(), 1);
    }

    #[test]
    fn rejects_dangling_edges_and_cycles() {
        let mut d = path();
        d.edges.push(edge("q2", "missing", Relation::LeadsTo, None));
        assert!(validate(&d).is_err());
        let mut d = path();
        d.edges.push(edge("q2", "s", Relation::LeadsTo, None));
        let err = validate(&d).expect_err("cycle");
        assert!(err.problems.iter().any(|p| p.contains("cycle")));
    }

    #[test]
    fn evidence_edges_must_run_from_observations_to_hypotheses() {
        let d = Diagram {
            id: "rel".into(),
            title: "Evidence".into(),
            kind: DiagramKind::EvidenceRelationship,
            description: "d".into(),
            nodes: vec![
                node(
                    "o",
                    "slots ≥ 142",
                    NodeRole::Observation,
                    NodeStatus::Observed,
                ),
                node(
                    "h",
                    "Worker saturation",
                    NodeRole::Hypothesis,
                    NodeStatus::Eliminated,
                ),
            ],
            edges: vec![edge("h", "o", Relation::Eliminates, None)],
            annotations: vec![],
        };
        assert!(validate(&d).is_err());
        let ok = Diagram {
            edges: vec![edge("o", "h", Relation::Eliminates, None)],
            ..d
        };
        let r = render(&ok).expect("valid");
        assert!(
            r.svg.contains("kd-rel-eliminates"),
            "the edge carries its relation"
        );
        assert!(r.svg.contains(">eliminates</text>"), "the legend names it");
        assert!(r.text.contains("slots ≥ 142 ─✕► eliminates"));
    }

    #[test]
    fn text_in_svg_is_escaped() {
        let mut d = path();
        d.nodes[2].label = "<script>alert(1)</script>".into();
        let r = render(&d).expect("valid");
        assert!(!r.svg.contains("<script>"));
        assert!(r.svg.contains("&lt;script&gt;"));
    }
}
