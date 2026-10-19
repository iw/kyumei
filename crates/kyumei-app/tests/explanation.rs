//! Model-written explanations are checked against the structured state they
//! explain. They may reword; they may not add numbers, findings or states.

use std::sync::Arc;

use kyumei_app::{
    InvestigationService, LocalInvestigations,
    fixture::{self, FixtureClock},
};
use kyumei_diagnostics::{ExplanationDraft, explain};
use kyumei_domain::ExplanationAuthor;
use kyumei_knowledge::KnowledgeBase;

async fn golden() -> (Arc<KnowledgeBase>, kyumei_domain::Investigation) {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let clock = Arc::new(FixtureClock::new(fx.open.at));
    let svc = LocalInvestigations::new(kb.clone(), clock.clone());
    let (id, _) = fixture::replay(&kb, &svc, &fx, Some(&clock), true)
        .await
        .expect("replay");
    let inv = svc.snapshot(&id).await.expect("snapshot").investigation;
    (kb, inv)
}

fn draft(summary: &str) -> ExplanationDraft {
    ExplanationDraft {
        summary: summary.to_owned(),
        findings: vec![],
        next_action: None,
        diagram: None,
    }
}

fn model() -> ExplanationAuthor {
    ExplanationAuthor::Model {
        provider: "test".into(),
        model: None,
        run: "run-1".into(),
    }
}

#[tokio::test]
async fn faithful_drafts_are_accepted() {
    let (kb, inv) = golden().await;
    let (_, _, d) = kyumei_diagnostics::select(&kb, &inv).expect("select");
    let d = d.expect("diagnostic");
    let a = kyumei_app::core::current_assessment(&kb, &inv).expect("assessment");
    let base = explain::deterministic(d, &a, &inv);
    let good = ExplanationDraft {
        summary: "Matching could only dispatch about 2,000 Workflow Tasks per second on this Task Queue, but the Signal flood added up to 2,112 per second, so tasks queued in Matching for about 31.4 s. Workers had free slots throughout.".into(),
        findings: vec![kyumei_diagnostics::explain::DraftFinding {
            finding: "F-slots-headroom".into(),
            text: "Workers always had at least 1,071 free Workflow Task slots, so they were never the bottleneck.".into(),
        }],
        next_action: None,
        diagram: Some(kyumei_diagnostics::explain::DiagramProposal {
            kind: kyumei_diagrams::DiagramKind::EvidenceRelationship,
            focus: vec!["dispatch-rate-ceiling".into(), "worker-slot-exhaustion".into()],
        }),
    };
    let merged = explain::apply_draft(&base, &good, d, &a, &inv, model()).expect("accepted");
    assert!(matches!(merged.author, ExplanationAuthor::Model { .. }));
    assert!(merged.summary.contains("2,112"));
    assert_eq!(
        merged.findings.len(),
        base.findings.len(),
        "the engine's findings are all kept"
    );
    assert_eq!(
        merged.diagrams[0].kind,
        kyumei_diagrams::DiagramKind::EvidenceRelationship
    );
    assert!(
        merged.diagrams[0].nodes.len()
            < base
                .diagrams
                .iter()
                .find(|x| x.kind == kyumei_diagrams::DiagramKind::EvidenceRelationship)
                .expect("rel")
                .nodes
                .len(),
        "the proposal focused the diagram"
    );
}

#[tokio::test]
async fn invented_numbers_are_rejected() {
    let (kb, inv) = golden().await;
    let (_, _, d) = kyumei_diagnostics::select(&kb, &inv).expect("select");
    let d = d.expect("diagnostic");
    let a = kyumei_app::core::current_assessment(&kb, &inv).expect("assessment");
    let base = explain::deterministic(d, &a, &inv);
    let problems = explain::apply_draft(
        &base,
        &draft("Matching's CPU reached 97% and the backlog hit 250,000 tasks."),
        d,
        &a,
        &inv,
        model(),
    )
    .expect_err("rejected");
    assert!(problems.iter().any(|p| p.contains("97")), "{problems:?}");
    assert!(
        problems.iter().any(|p| p.contains("250,000")),
        "{problems:?}"
    );
}

#[tokio::test]
async fn contradicting_states_are_rejected() {
    let (kb, inv) = golden().await;
    let (_, _, d) = kyumei_diagnostics::select(&kb, &inv).expect("select");
    let d = d.expect("diagnostic");
    let a = kyumei_app::core::current_assessment(&kb, &inv).expect("assessment");
    let base = explain::deterministic(d, &a, &inv);
    let problems = explain::apply_draft(
        &base,
        &draft("Workflow Worker slots exhausted is confirmed as the root cause; add more Workers."),
        d,
        &a,
        &inv,
        model(),
    )
    .expect_err("rejected");
    assert!(
        problems.iter().any(|p| p.contains("established")),
        "{problems:?}"
    );
    let problems = explain::apply_draft(
        &base,
        &draft("Signal flood driving Workflow Tasks has been ruled out."),
        d,
        &a,
        &inv,
        model(),
    )
    .expect_err("rejected");
    assert!(
        problems.iter().any(|p| p.contains("ruled out")),
        "{problems:?}"
    );
}

#[tokio::test]
async fn unknown_findings_and_diagrams_are_rejected() {
    let (kb, inv) = golden().await;
    let (_, _, d) = kyumei_diagnostics::select(&kb, &inv).expect("select");
    let d = d.expect("diagnostic");
    let a = kyumei_app::core::current_assessment(&kb, &inv).expect("assessment");
    let base = explain::deterministic(d, &a, &inv);
    let mut bad = draft("Matching dispatch was the constraint.");
    bad.findings
        .push(kyumei_diagnostics::explain::DraftFinding {
            finding: "F-made-up".into(),
            text: "x".into(),
        });
    bad.diagram = Some(kyumei_diagnostics::explain::DiagramProposal {
        kind: kyumei_diagrams::DiagramKind::EvidenceRelationship,
        focus: vec!["matching-host-cpu".into()],
    });
    let problems = explain::apply_draft(&base, &bad, d, &a, &inv, model()).expect_err("rejected");
    assert!(problems.iter().any(|p| p.contains("F-made-up")));
    assert!(problems.iter().any(|p| p.contains("matching-host-cpu")));
}

#[tokio::test]
async fn the_deterministic_explanation_matches_the_assessment() {
    let (kb, inv) = golden().await;
    let (_, _, d) = kyumei_diagnostics::select(&kb, &inv).expect("select");
    let d = d.expect("diagnostic");
    let a = kyumei_app::core::current_assessment(&kb, &inv).expect("assessment");
    let e = explain::deterministic(d, &a, &inv);
    assert_eq!(
        e.summary,
        a.conclusion.as_ref().expect("conclusion").statement
    );
    // Every diagram node that depicts a hypothesis shows that hypothesis's state.
    for diagram in &e.diagrams {
        kyumei_diagrams::validate(diagram).expect("valid");
        for node in &diagram.nodes {
            for r in &node.refers_to {
                if let kyumei_domain::DomainRef::Hypothesis(h) = r
                    && node.role != kyumei_diagrams::NodeRole::Component
                {
                    let outcome = a.hypothesis(h).expect("active");
                    assert_eq!(
                        node.status,
                        kyumei_diagnostics::diagrams::status_of(outcome.state, outcome.contested),
                        "{} in {}",
                        h,
                        diagram.title
                    );
                }
            }
        }
    }
}
