//! Simulations the evidence can calibrate now.
//!
//! A diagnostic's simulation asks whether a hypothesis's cause, at the size
//! the evidence shows, is big enough to produce the symptom. It is due once
//! every input its model needs is known: the calibration inputs as observed
//! facts, and the symptom it is compared with as an observed fact or the
//! operator's word. It runs once for those inputs over the incident window,
//! not for a hypothesis the evidence has eliminated, and only for a
//! self-hosted server of the release the simulator models.

use std::collections::BTreeMap;

use kyumei_domain::{
    DeploymentKind, EpistemicStatus, EvidenceId, HypothesisId, HypothesisState, Investigation,
    SignalId, SimulationId, TimeWindow,
};
use kyumei_knowledge::{DiagnosticDef, SimulationModel};
use serde::{Deserialize, Serialize};

use crate::{evaluate::HypothesisOutcome, facts::FactTable};

/// The input a simulation's result is compared with. Unlike the others, the
/// operator's word may give it, and a change to it needs no new run.
pub const SYMPTOM: &str = "symptom";

/// The shortest window a simulation covers.
const MIN_WINDOW_SECONDS: i64 = 60;

/// A simulation to run, with its model's inputs read from the facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimulationPlan {
    pub simulation: SimulationId,
    pub hypothesis: HypothesisId,
    pub model: SimulationModel,
    pub inputs: BTreeMap<String, f64>,
    /// The evidence the calibration inputs came from.
    pub calibrated_on: Vec<EvidenceId>,
    /// The incident window the run stands for.
    pub window: TimeWindow,
    pub produces: SignalId,
}

/// The simulations due for `investigation`.
pub fn due(
    d: &DiagnosticDef,
    outcomes: &BTreeMap<HypothesisId, HypothesisOutcome>,
    facts: &FactTable,
    investigation: &Investigation,
) -> Vec<SimulationPlan> {
    // Temporal Cloud has no server to simulate.
    let cloud = investigation
        .environment
        .topology
        .as_ref()
        .is_some_and(|t| t.value.deployment == DeploymentKind::TemporalCloud);
    let Some(window) = investigation.window.as_window() else {
        return Vec::new();
    };
    if cloud || window.duration().as_secs() < MIN_WINDOW_SECONDS {
        return Vec::new();
    }
    let version = investigation
        .environment
        .server_version
        .as_ref()
        .map(|v| &v.value);
    d.simulations
        .iter()
        .filter(|sim| version.is_some_and(|v| v.same_minor(&sim.model.models())))
        .filter_map(|sim| {
            let state = outcomes.get(&sim.hypothesis)?.state;
            if state == HypothesisState::Eliminated {
                return None;
            }
            let mut inputs = BTreeMap::new();
            let mut calibrated_on = Vec::new();
            for (role, input) in &sim.inputs {
                let required = sim.model.required().contains(&role.as_str());
                let value = facts
                    .get(&input.signal)
                    .filter(|f| {
                        f.status == EpistemicStatus::ObservedFact
                            || (role == SYMPTOM && f.status == EpistemicStatus::OperatorAssertion)
                    })
                    .and_then(|f| f.get(input.stat).filter(|v| v.is_finite()).map(|v| (v, f)));
                match value {
                    Some((v, fact)) => {
                        inputs.insert(role.clone(), v);
                        if role != SYMPTOM {
                            calibrated_on.extend(fact.evidence.iter().cloned());
                        }
                    }
                    None if required => return None,
                    None => {}
                }
            }
            calibrated_on.sort();
            calibrated_on.dedup();
            let already = investigation
                .evidence
                .iter()
                .filter_map(|e| e.simulation.as_ref())
                .any(|r| r.simulation == sim.id && r.window == window && same(&r.inputs, &inputs));
            (!already).then(|| SimulationPlan {
                simulation: sim.id.clone(),
                hypothesis: sim.hypothesis.clone(),
                model: sim.model,
                inputs,
                calibrated_on,
                window,
                produces: sim.produces.clone(),
            })
        })
        .collect()
}

/// Whether two runs' calibration inputs are the same; the symptom they are
/// compared with doesn't change a run.
fn same(a: &BTreeMap<String, f64>, b: &BTreeMap<String, f64>) -> bool {
    let calibration = |m: &BTreeMap<String, f64>| -> Vec<(String, f64)> {
        m.iter()
            .filter(|(k, _)| k.as_str() != SYMPTOM)
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    };
    let (a, b) = (calibration(a), calibration(b));
    a.len() == b.len()
        && a.iter().zip(&b).all(|((ka, va), (kb, vb))| {
            ka == kb && (va - vb).abs() <= 1e-9 * va.abs().max(vb.abs()).max(1.0)
        })
}
