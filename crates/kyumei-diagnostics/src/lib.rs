//! Kyumei's deterministic diagnostic engine.
//!
//! Given an investigation and versioned knowledge, [`assess`] determines:
//!
//! * the facts the evidence supports, per diagnostic signal;
//! * which rules fire, and so which hypotheses are supported, weakened,
//!   eliminated or established;
//! * which evidence request would best discriminate what remains open;
//! * whether a conclusion is supportable, and what uncertainty remains.
//!
//! The engine has no clock, no I/O and no model. The same investigation and
//! knowledge always produce the same assessment, so it runs identically in
//! tests, fixtures and durable workflow activities.

pub mod conclude;
pub mod diagrams;
pub mod evaluate;
pub mod explain;
pub mod facts;
pub mod select;
pub mod simulate;

use std::collections::BTreeMap;

use kyumei_domain::{
    AssessmentRecord, AssessmentTrigger, Conclusion, DiagnosticFinding, DiagnosticId, HypothesisId,
    HypothesisState, HypothesisTransition, Investigation, PackRef, RecordedState, Uncertainty,
};
use kyumei_knowledge::{DiagnosticDef, KnowledgeBase, Pack, PackFit};
use serde::{Deserialize, Serialize};

pub use evaluate::{Firing, HypothesisOutcome};
pub use explain::{DiagnosticExplanation, ExplanationDraft};
pub use facts::{Fact, FactTable, WindowFit};
pub use select::RequestPlan;
pub use simulate::SimulationPlan;

/// Engine identity recorded with every assessment.
pub const ENGINE: &str = concat!("kyumei-diagnostics/", env!("CARGO_PKG_VERSION"));

/// The engine's complete view of an investigation at one point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assessment {
    pub engine: String,
    pub pack: PackRef,
    pub fit: Fit,
    pub diagnostic: Option<DiagnosticId>,
    pub facts: FactTable,
    /// Active hypotheses in diagnostic-path order.
    pub hypotheses: Vec<HypothesisOutcome>,
    pub findings: Vec<DiagnosticFinding>,
    pub firings: Vec<Firing>,
    /// Candidate requests, best first.
    pub plans: Vec<RequestPlan>,
    /// Simulations the evidence can calibrate and that haven't run.
    #[serde(default)]
    pub simulations: Vec<SimulationPlan>,
    pub conclusion: Option<Conclusion>,
    /// The conclusion is final: no more specific diagnosis remains possible.
    pub conclusion_final: bool,
    pub uncertainties: Vec<Uncertainty>,
    pub stable: bool,
}

/// Serializable [`PackFit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    Verified,
    SameMinor,
    VersionUnknown,
    Swept,
    OutsideFamily,
}

impl From<PackFit> for Fit {
    fn from(f: PackFit) -> Self {
        match f {
            PackFit::Verified => Self::Verified,
            PackFit::SameMinor => Self::SameMinor,
            PackFit::VersionUnknown => Self::VersionUnknown,
            PackFit::Swept => Self::Swept,
            PackFit::OutsideFamily => Self::OutsideFamily,
        }
    }
}

impl Assessment {
    pub fn hypothesis(&self, id: &HypothesisId) -> Option<&HypothesisOutcome> {
        self.hypotheses.iter().find(|h| &h.id == id)
    }

    pub fn finding(&self, id: &kyumei_domain::FindingId) -> Option<&DiagnosticFinding> {
        self.findings.iter().find(|f| &f.id == id)
    }

    pub fn next(&self) -> Option<&RequestPlan> {
        self.plans.first()
    }

    /// A compact record for the investigation log, with transitions relative
    /// to `previous`.
    pub fn record(
        &self,
        trigger: AssessmentTrigger,
        previous: Option<&AssessmentRecord>,
    ) -> AssessmentRecord {
        let states: BTreeMap<HypothesisId, RecordedState> = self
            .hypotheses
            .iter()
            .map(|h| {
                (
                    h.id.clone(),
                    RecordedState {
                        state: h.state,
                        contested: h.contested,
                    },
                )
            })
            .collect();
        let transitions = self
            .hypotheses
            .iter()
            .filter_map(|h| {
                let before = previous.and_then(|p| p.states.get(&h.id)).map(|s| s.state);
                (before != Some(h.state)).then(|| HypothesisTransition {
                    hypothesis: h.id.clone(),
                    from: before,
                    to: h.state,
                    because: h.findings.clone(),
                })
            })
            .collect();
        AssessmentRecord {
            trigger,
            pack: self.pack.clone(),
            engine: self.engine.clone(),
            states,
            transitions,
            findings: self.findings.iter().map(|f| f.id.clone()).collect(),
            next_request: self.next().map(|p| p.definition.clone()),
            conclusion: self.conclusion.as_ref().map(|c| c.id.clone()),
        }
    }
}

/// Engine failures. These are configuration problems, never "no evidence".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssessError {
    #[error("no knowledge pack is loaded")]
    NoPack,
}

/// The pack and diagnostic an investigation uses.
pub fn select<'k>(
    kb: &'k KnowledgeBase,
    investigation: &Investigation,
) -> Result<(&'k Pack, PackFit, Option<&'k DiagnosticDef>), AssessError> {
    let version = investigation
        .environment
        .server_version
        .as_ref()
        .map(|v| &v.value);
    let selection = kb.select(version).ok_or(AssessError::NoPack)?;
    let diagnostic = investigation
        .symptom
        .classification
        .as_ref()
        .and_then(|c| selection.pack.diagnostic_for(&c.signature))
        .map(|(d, _)| d);
    Ok((selection.pack, selection.fit, diagnostic))
}

/// Assess an investigation.
pub fn assess(
    kb: &KnowledgeBase,
    investigation: &Investigation,
) -> Result<Assessment, AssessError> {
    let (pack, fit, diagnostic) = select(kb, investigation)?;
    let Some(d) = diagnostic else {
        return Ok(Assessment {
            engine: ENGINE.to_owned(),
            pack: pack.pack_ref(),
            fit: fit.into(),
            diagnostic: None,
            facts: FactTable::default(),
            hypotheses: Vec::new(),
            findings: Vec::new(),
            firings: Vec::new(),
            plans: Vec::new(),
            simulations: Vec::new(),
            conclusion: None,
            conclusion_final: false,
            uncertainties: vec![Uncertainty {
                kind: kyumei_domain::UncertaintyKind::MissingContext,
                text: "The symptom has not been matched to a diagnostic Kyumei knows. Choose the closest symptom to begin.".to_owned(),
                refs: vec![],
            }],
            stable: true,
        });
    };
    let observations = investigation.effective_observations();
    let facts = facts::build(d, &observations, &investigation.window);
    let eval = evaluate::evaluate(pack, d, &facts);
    let plans = select::plan(d, &eval.outcomes, &facts, investigation);
    let simulations = simulate::due(d, &eval.outcomes, &facts, investigation);
    let uncertainties = conclude::uncertainties(
        d,
        fit,
        &pack.verified_server_versions(),
        investigation,
        &facts,
        &eval,
        &plans,
    );
    let conclusion = conclude::conclude(d, &facts, &eval, &uncertainties);
    let conclusion_final = conclusion
        .as_ref()
        .is_some_and(|c| conclude::is_final(d, &eval, &c.id));
    let order = |id: &HypothesisId| {
        d.hypotheses
            .iter()
            .position(|h| &h.id == id)
            .map(|i| (d.stage_order(&d.hypotheses[i].stage), i))
            .unwrap_or((u32::MAX, usize::MAX))
    };
    let mut hypotheses: Vec<HypothesisOutcome> = eval.outcomes.values().cloned().collect();
    hypotheses.sort_by_key(|h| order(&h.id));
    let mut findings = eval.findings.clone();
    findings.sort_by_key(|f| order(&f.hypothesis));
    Ok(Assessment {
        engine: ENGINE.to_owned(),
        pack: pack.pack_ref(),
        fit: fit.into(),
        diagnostic: Some(d.id.clone()),
        facts,
        hypotheses,
        findings,
        firings: eval.firings,
        plans,
        simulations,
        conclusion,
        conclusion_final,
        uncertainties,
        stable: eval.stable,
    })
}

/// Hypotheses grouped by state, in path order.
pub fn by_state(a: &Assessment) -> BTreeMap<HypothesisState, Vec<&HypothesisOutcome>> {
    let mut map: BTreeMap<HypothesisState, Vec<&HypothesisOutcome>> = BTreeMap::new();
    for h in &a.hypotheses {
        map.entry(h.state).or_default().push(h);
    }
    map
}
