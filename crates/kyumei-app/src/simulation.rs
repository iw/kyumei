//! The "big enough?" check: run the simulations an assessment says are due,
//! and record what each found as evidence of its own.
//!
//! A simulation is calibrated on the investigation's observed facts and run
//! with tempdes, at the commit Kyumei is built against and a fixed seed, so
//! the same inputs give the same result. What it found is a simulated fact:
//! it can support or weaken a hypothesis, never settle one.

use std::{collections::BTreeMap, time::Duration};

use jiff::Timestamp;
use kyumei_diagnostics::{SimulationPlan, simulate::SYMPTOM};
use kyumei_domain::{
    BindingBasis, DeclaredContext, EpistemicStatus, Evidence, EvidenceForm, EvidenceId,
    EvidenceOrigin, Interpretation, InterpretationNote, NoteSeverity, Observation, ObservationId,
    ObservedValue, RawBody, RawEvidence, SignalBinding, SignalId, SimulationRecord, Statistic,
    Subject, Unit, format_number,
};
use kyumei_knowledge::{DiagnosticDef, SimulationModel};
use kyumei_sim::{TEMPDES_COMMIT, dispatch::DispatchCase};
use serde::{Deserialize, Serialize};

/// How long one run may take.
pub const BUDGET: Duration = Duration::from_secs(120);
/// Every run uses this seed, so the same inputs give the same result.
pub const SEED: u64 = 1;
pub const SIMULATOR: &str = "tempdes";

/// The tempdes commit simulations run.
pub fn commit() -> &'static str {
    TEMPDES_COMMIT
}

/// A finished run, as the command that records it carries it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimulationRun {
    pub record: SimulationRecord,
    /// The signal the result informs.
    pub produces: SignalId,
    pub simulator: String,
    pub commit: String,
    pub seed: u64,
    /// The scenario, in the simulator's format.
    pub scenario: String,
    /// Real time the run took.
    pub wall_ms: u64,
}

impl SimulationRun {
    /// The value the produced signal takes, when the run gave one.
    pub fn value(&self) -> Option<f64> {
        self.record.results.get(RESULT).copied()
    }
}

/// The result each model's produced signal reads.
const RESULT: &str = "schedule_to_start_max";

/// Run `plan`. A run that fails is recorded too, with why, so that it is not
/// tried again for the same inputs.
pub fn run(plan: &SimulationPlan) -> SimulationRun {
    match plan.model {
        SimulationModel::TaskQueueDispatch => dispatch(plan),
    }
}

fn dispatch(plan: &SimulationPlan) -> SimulationRun {
    let input = |role: &str| plan.inputs.get(role).copied();
    let case = DispatchCase {
        add_rate: input("add_rate").unwrap_or_default(),
        dispatch_limit: input("dispatch_limit").unwrap_or_default(),
        read_partitions: input("read_partitions")
            .map_or(1, |p| p.round().clamp(1.0, 1_000.0) as u32),
        duration_s: plan.window.duration().as_secs().max(60) as u64,
        seed: SEED,
    };
    let started = std::time::Instant::now();
    let outcome = if case.dispatch_limit <= 0.0 || case.add_rate <= 0.0 {
        Err("the add rate and the dispatch limit must be above zero".to_owned())
    } else {
        case.run(BUDGET).map_err(|e| e.to_string())
    };
    let mut results = BTreeMap::new();
    let failure = match outcome {
        Ok(r) => {
            for (name, value) in [
                (RESULT, r.schedule_to_start_max),
                ("backlog_age_max", r.backlog_age_max),
                ("add_rate", r.add_rate),
                ("dispatch_rate", r.dispatch_rate),
            ] {
                if value.is_finite() {
                    results.insert(name.to_owned(), value);
                }
            }
            None
        }
        Err(e) => Some(e),
    };
    SimulationRun {
        record: SimulationRecord {
            simulation: plan.simulation.clone(),
            hypothesis: plan.hypothesis.clone(),
            inputs: plan.inputs.clone(),
            calibrated_on: plan.calibrated_on.clone(),
            window: plan.window,
            scale: case.scale(),
            results,
            failure,
        },
        produces: plan.produces.clone(),
        simulator: SIMULATOR.to_owned(),
        commit: TEMPDES_COMMIT.to_owned(),
        seed: SEED,
        scenario: case.scenario(),
        wall_ms: started.elapsed().as_millis() as u64,
    }
}

/// The evidence that records `run`.
pub fn evidence(
    d: &DiagnosticDef,
    id: EvidenceId,
    at: Timestamp,
    run: &SimulationRun,
) -> Result<Evidence, String> {
    let sim = d.simulation(&run.record.simulation).ok_or_else(|| {
        format!(
            "`{}` is not a simulation of this diagnostic",
            run.record.simulation
        )
    })?;
    if sim.produces != run.produces {
        return Err(format!(
            "simulation `{}` produces `{}`, not `{}`",
            sim.id, sim.produces, run.produces
        ));
    }
    let signal = d
        .signal(&sim.produces)
        .ok_or_else(|| format!("unknown signal `{}`", sim.produces))?;
    let mut notes = notes(sim.model, run);
    let mut observations = Vec::new();
    if let Some(value) = run.value() {
        let oid = ObservationId::new(format!("{id}.O1"));
        notes.push(InterpretationNote {
            severity: NoteSeverity::Caution,
            text: "tempdes models Temporal 1.31.0 on EKS, with Workers that follow the Go SDK. A run that reproduces the latency is consistent with the cause, not proof of it, so it can support or weaken the hypothesis but never settle it.".to_owned(),
            observation: Some(oid.clone()),
        });
        observations.push(Observation {
            id: oid,
            evidence: id.clone(),
            status: EpistemicStatus::Simulated,
            subject: Subject::described(sim.title.clone()),
            binding: Some(SignalBinding {
                signal: signal.id.clone(),
                basis: BindingBasis::Simulation {
                    simulation: sim.id.clone(),
                },
                claim: None,
            }),
            window: Some(run.record.window),
            value: ObservedValue::Scalar {
                unit: Some(signal.unit),
                value,
                statistic: Statistic::Max,
            },
        });
    }
    let text = run.scenario.clone();
    Ok(Evidence {
        id,
        submitted_at: at,
        origin: EvidenceOrigin::Simulation {
            simulator: run.simulator.clone(),
            commit: run.commit.clone(),
            seed: run.seed,
        },
        responds_to: None,
        raw: RawEvidence {
            media_type: "application/yaml".to_owned(),
            filename: Some(format!("{}.yaml", sim.id)),
            sha256: kyumei_evidence::sha256_hex(text.as_bytes()),
            size_bytes: text.len() as u64,
            body: RawBody::Text { text },
        },
        declared: DeclaredContext {
            form: Some(EvidenceForm::Simulation),
            window: Some(run.record.window),
            ..DeclaredContext::default()
        },
        interpretation: Interpretation {
            form: EvidenceForm::Simulation,
            interpreter: "simulation/1".to_owned(),
            observations,
            notes,
        },
        simulation: Some(run.record.clone()),
    })
}

/// What the run was, what it was calibrated on, and how close it came.
fn notes(model: SimulationModel, run: &SimulationRun) -> Vec<InterpretationNote> {
    let r = &run.record;
    let input = |role: &str| r.inputs.get(role).copied();
    let mut notes = Vec::new();
    let mut note = |severity, text: String| {
        notes.push(InterpretationNote {
            severity,
            text,
            observation: None,
        })
    };
    let calibrated_on: Vec<String> = r.calibrated_on.iter().map(ToString::to_string).collect();
    let minutes = r.window.duration().as_secs() / 60;
    match model {
        SimulationModel::TaskQueueDispatch => {
            let partitions = input("read_partitions").map_or_else(|| "?".to_owned(), format_number);
            note(
                NoteSeverity::Info,
                format!(
                    "Simulated by tempdes {} with seed {}: one Task Queue whose only constraint is its dispatch limit, for the {minutes}-minute incident window. Calibrated on {}: {} Workflow Tasks a second added against a limit of {} a second, over {partitions} read partitions.",
                    run.commit.get(..7).unwrap_or(&run.commit),
                    run.seed,
                    calibrated_on.join(", "),
                    input("add_rate").map_or("?".to_owned(), format_number),
                    input("dispatch_limit").map_or("?".to_owned(), format_number),
                ),
            );
            if r.scale > 1.0 {
                note(
                    NoteSeverity::Info,
                    format!(
                        "Both rates were divided by {} to keep the run small. Their ratio, which sets the wait, is unchanged.",
                        format_number(r.scale)
                    ),
                );
            }
        }
    }
    match (&r.failure, run.value()) {
        (Some(why), _) => note(
            NoteSeverity::Problem,
            format!("The simulation gave no result: {why}."),
        ),
        (None, Some(value)) => {
            let observed = input(SYMPTOM);
            let fit = observed
                .filter(|o| *o > 0.0)
                .map(|o| {
                    format!(
                        ", {}× the {} observed",
                        format_number(value / o),
                        Unit::Seconds.format_value(o)
                    )
                })
                .unwrap_or_default();
            note(
                NoteSeverity::Info,
                format!(
                    "Schedule-to-start reached {} in the simulation{fit}.",
                    Unit::Seconds.format_value(value)
                ),
            );
        }
        (None, None) => note(
            NoteSeverity::Problem,
            "The simulation measured no schedule-to-start.".to_owned(),
        ),
    }
    notes
}

/// Labelled values, each with its unit.
pub type Quantities = Vec<(String, String)>;

/// A run's inputs and results as an operator reads them: a label and a value
/// with its unit.
pub fn quantities(record: &SimulationRecord) -> (Quantities, Quantities) {
    let show = |name: &str, value: f64, result: bool| -> (String, String) {
        let (label, unit) = match (name, result) {
            ("add_rate", false) => ("Workflow Tasks added", Unit::PerSecond),
            ("dispatch_limit", false) => ("Dispatch limit", Unit::PerSecond),
            ("read_partitions", false) => ("Read partitions", Unit::Count),
            (SYMPTOM, false) => ("Schedule-to-start observed", Unit::Seconds),
            (RESULT, true) => ("Schedule-to-start", Unit::Seconds),
            ("backlog_age_max", true) => ("Oldest backlogged task", Unit::Seconds),
            ("add_rate", true) => ("Added, at the run's scale", Unit::PerSecond),
            ("dispatch_rate", true) => ("Dispatched, at the run's scale", Unit::PerSecond),
            _ => (name, Unit::Count),
        };
        (label.to_owned(), unit.format_value(value))
    };
    (
        record
            .inputs
            .iter()
            .map(|(k, v)| show(k, *v, false))
            .collect(),
        record
            .results
            .iter()
            .map(|(k, v)| show(k, *v, true))
            .collect(),
    )
}
