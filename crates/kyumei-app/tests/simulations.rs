//! The "big enough?" check: simulations calibrated on an investigation's
//! evidence, recorded as simulated facts that can support or weaken a
//! hypothesis but never settle it.

use std::sync::Arc;

use kyumei_app::{
    InvestigationService, LocalInvestigations, core,
    fixture::{self, FixtureClock},
};
use kyumei_domain::{
    Effect, EpistemicStatus, EvidenceForm, EvidenceOrigin, HypothesisId, HypothesisState,
    NoteSeverity,
};
use kyumei_knowledge::KnowledgeBase;
use kyumei_sim::TEMPDES_COMMIT;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_golden_dispatch_limit_is_simulated() {
    let kb = Arc::new(KnowledgeBase::builtin().expect("knowledge"));
    let fx = fixture::golden().expect("fixture");
    let clock = Arc::new(FixtureClock::new(fx.open.at));
    let service = LocalInvestigations::new(kb.clone(), clock.clone()).with_simulation();
    let (id, _) = fixture::replay(&kb, &service, &fx, Some(clock.as_ref()), false)
        .await
        .expect("replay");
    let investigation = service.snapshot(&id).await.expect("snapshot").investigation;

    // One run, once its inputs were known, recorded as evidence of its own.
    let runs: Vec<_> = investigation
        .evidence
        .iter()
        .filter(|e| e.simulation.is_some())
        .collect();
    assert_eq!(runs.len(), 1, "{runs:#?}");
    let evidence = runs[0];
    let record = evidence.simulation.as_ref().expect("record");
    eprintln!("{:#?}", evidence.interpretation.notes);
    assert_eq!(record.simulation.as_str(), "dispatch-ceiling-size");
    assert_eq!(evidence.interpretation.form, EvidenceForm::Simulation);
    let EvidenceOrigin::Simulation {
        simulator,
        commit,
        seed,
    } = &evidence.origin
    else {
        panic!("origin: {:?}", evidence.origin);
    };
    assert_eq!(
        (simulator.as_str(), commit.as_str(), *seed),
        ("tempdes", TEMPDES_COMMIT, 1)
    );
    // Calibrated on the DescribeTaskQueue capture and the dynamic configuration.
    let calibrated: Vec<&str> = record.calibrated_on.iter().map(|e| e.as_str()).collect();
    assert_eq!(calibrated, ["E4", "E5"]);
    assert_eq!(record.inputs["read_partitions"], 2.0);
    assert!(record.failure.is_none(), "{record:#?}");
    assert!(
        evidence
            .raw
            .text()
            .is_some_and(|t| t.contains("task-queue-dispatch"))
    );
    // The fit, shown with the run.
    assert!(
        evidence
            .interpretation
            .notes
            .iter()
            .any(|n| n.severity == NoteSeverity::Info && n.text.contains("× the 30 s observed")),
        "{:#?}",
        evidence.interpretation.notes
    );
    let [observation] = evidence.interpretation.observations.as_slice() else {
        panic!("one observation");
    };
    assert_eq!(observation.status, EpistemicStatus::Simulated);

    // It supports the hypothesis, as a simulated fact, and settles nothing:
    // the observed facts established it already.
    let assessment = core::current_assessment(&kb, &investigation).expect("assessment");
    let finding = assessment
        .findings
        .iter()
        .find(|f| f.rule.as_str() == "ceiling-big-enough")
        .unwrap_or_else(|| panic!("{:#?}", assessment.findings));
    assert_eq!(finding.effect, Effect::Supports);
    assert!(finding.rests_on_simulation());
    assert_eq!(
        finding
            .evidence()
            .iter()
            .filter(|e| e.as_str() == evidence.id.as_str())
            .count(),
        1
    );
    let ceiling = assessment
        .hypothesis(&HypothesisId::new("dispatch-rate-ceiling"))
        .expect("hypothesis");
    assert_eq!(ceiling.state, HypothesisState::Established);
    assert_eq!(
        assessment.conclusion.as_ref().map(|c| c.id.as_str()),
        Some("dispatch-ceiling-under-signal-flood")
    );
    // Nothing more is due: the run is recorded with its inputs.
    assert!(assessment.simulations.is_empty());
}
