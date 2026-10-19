//! Rule evaluation and hypothesis states.
//!
//! States are a function of the current facts, never of the order evidence
//! arrived in. That is what lets later evidence revisit an earlier
//! hypothesis: re-evaluation simply produces a different state.

use std::collections::{BTreeMap, BTreeSet};

use kyumei_domain::{
    ClaimId, DiagnosticFinding, Effect, EpistemicStatus, FactBasis, FindingId, HypothesisId,
    HypothesisState, SignalId, Statistic, format_number,
};
use kyumei_knowledge::{
    Claim, Condition, DiagnosticDef, FactLookup, Pack, RuleDef, Truth, template,
};
use serde::{Deserialize, Serialize};

use crate::facts::FactTable;

/// The state of one hypothesis after evaluation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HypothesisOutcome {
    pub id: HypothesisId,
    pub state: HypothesisState,
    pub contested: bool,
    pub findings: Vec<FindingId>,
}

/// A rule that fired.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Firing {
    pub rule: kyumei_domain::RuleId,
    pub hypothesis: HypothesisId,
    /// The effect applied, after any downgrade.
    pub effect: Effect,
    /// The rule wanted to eliminate or establish, but every fact it read was
    /// an operator assertion, or it read a simulated fact, so it was
    /// downgraded to weaken or support.
    pub downgraded_from: Option<Effect>,
}

#[derive(Debug)]
pub struct Evaluation {
    pub outcomes: BTreeMap<HypothesisId, HypothesisOutcome>,
    pub findings: Vec<DiagnosticFinding>,
    pub firings: Vec<Firing>,
    pub stable: bool,
}

pub(crate) struct Lookup<'a> {
    pub facts: &'a FactTable,
    pub states: &'a BTreeMap<HypothesisId, HypothesisState>,
}

impl FactLookup for Lookup<'_> {
    fn statistic(&self, signal: &SignalId, stat: Statistic) -> Option<f64> {
        self.facts.get(signal).and_then(|f| f.get(stat))
    }

    fn categorical(&self, signal: &SignalId) -> Option<String> {
        self.facts.get(signal).and_then(|f| {
            f.categorical.clone().or_else(|| {
                f.get(Statistic::Last)
                    .map(|v| format_number(v).replace(',', ""))
            })
        })
    }

    fn hypothesis_state(&self, hypothesis: &HypothesisId) -> Option<HypothesisState> {
        self.states.get(hypothesis).copied()
    }
}

const MAX_ITERATIONS: usize = 16;

pub fn evaluate(pack: &Pack, d: &DiagnosticDef, facts: &FactTable) -> Evaluation {
    let mut states: BTreeMap<HypothesisId, HypothesisState> = d
        .hypotheses
        .iter()
        .filter(|h| h.activation.is_none())
        .map(|h| (h.id.clone(), HypothesisState::Candidate))
        .collect();
    let mut firings = Vec::new();
    let mut stable = false;

    for _ in 0..MAX_ITERATIONS {
        let lookup = Lookup {
            facts,
            states: &states,
        };
        let active: BTreeSet<HypothesisId> = d
            .hypotheses
            .iter()
            .filter(|h| {
                h.activation
                    .as_ref()
                    .is_none_or(|a| a.evaluate(&lookup) == Truth::True)
            })
            .map(|h| h.id.clone())
            .collect();
        firings = d
            .rules
            .iter()
            .filter(|r| active.contains(&r.hypothesis))
            .filter(|r| r.when.evaluate(&lookup) == Truth::True)
            .map(|r| {
                // Assertions alone, and simulations at all, can only move a
                // hypothesis, never settle it.
                let capped = only_assertions(&r.when, facts) || reads_simulated(&r.when, facts);
                let (effect, downgraded_from) = match r.effect {
                    Effect::Eliminates if capped => (Effect::Weakens, Some(Effect::Eliminates)),
                    Effect::Establishes if capped => (Effect::Supports, Some(Effect::Establishes)),
                    e => (e, None),
                };
                Firing {
                    rule: r.id.clone(),
                    hypothesis: r.hypothesis.clone(),
                    effect,
                    downgraded_from,
                }
            })
            .collect();
        let next: BTreeMap<HypothesisId, HypothesisState> = active
            .iter()
            .map(|h| {
                let effects: Vec<Effect> = firings
                    .iter()
                    .filter(|f| &f.hypothesis == h)
                    .map(|f| f.effect)
                    .collect();
                (h.clone(), state_from(&effects).0)
            })
            .collect();
        if next == states {
            stable = true;
            break;
        }
        states = next;
    }

    let findings: Vec<DiagnosticFinding> = firings
        .iter()
        .filter_map(|f| d.rule(&f.rule).map(|r| finding(pack, d, r, f, facts)))
        .collect();
    let outcomes = states
        .iter()
        .map(|(h, state)| {
            let effects: Vec<Effect> = firings
                .iter()
                .filter(|f| &f.hypothesis == h)
                .map(|f| f.effect)
                .collect();
            let (_, contested) = state_from(&effects);
            (
                h.clone(),
                HypothesisOutcome {
                    id: h.clone(),
                    state: *state,
                    contested,
                    findings: findings
                        .iter()
                        .filter(|f| &f.hypothesis == h)
                        .map(|f| f.id.clone())
                        .collect(),
                },
            )
        })
        .collect();
    Evaluation {
        outcomes,
        findings,
        firings,
        stable,
    }
}

/// The state implied by a set of effects, and whether evidence conflicts.
pub fn state_from(effects: &[Effect]) -> (HypothesisState, bool) {
    let count = |e: Effect| effects.iter().filter(|x| **x == e).count();
    let (establish, eliminate, support, weaken) = (
        count(Effect::Establishes),
        count(Effect::Eliminates),
        count(Effect::Supports),
        count(Effect::Weakens),
    );
    let positive = establish + support;
    let negative = eliminate + weaken;
    let contested = positive > 0 && negative > 0;
    let state = match () {
        // Direct contradiction: neither conclusion is safe.
        _ if establish > 0 && eliminate > 0 => HypothesisState::Candidate,
        _ if eliminate > 0 => HypothesisState::Eliminated,
        _ if establish > 0 => HypothesisState::Established,
        _ if support > weaken => HypothesisState::Supported,
        _ if weaken > support => HypothesisState::Weakened,
        _ => HypothesisState::Candidate,
    };
    (state, contested)
}

/// Whether `condition` reads a simulated fact.
fn reads_simulated(condition: &Condition, facts: &FactTable) -> bool {
    condition
        .signals()
        .iter()
        .filter_map(|s| facts.get(s))
        .any(|f| f.status == EpistemicStatus::Simulated)
}

fn only_assertions(condition: &Condition, facts: &FactTable) -> bool {
    let statuses: Vec<EpistemicStatus> = condition
        .signals()
        .iter()
        .filter_map(|s| facts.get(s).map(|f| f.status))
        .collect();
    !statuses.is_empty() && statuses.iter().all(|s| *s != EpistemicStatus::ObservedFact)
}

fn finding(
    pack: &Pack,
    d: &DiagnosticDef,
    rule: &RuleDef,
    firing: &Firing,
    facts: &FactTable,
) -> DiagnosticFinding {
    let basis: Vec<FactBasis> = rule
        .when
        .signal_stats()
        .into_iter()
        .filter_map(|ss| {
            let fact = facts.get(&ss.signal)?;
            let (value, category) = match fact.get(ss.stat) {
                Some(value) => (value, None),
                None => (0.0, Some(fact.categorical.clone()?)),
            };
            Some(FactBasis {
                signal: ss.signal.clone(),
                statistic: ss.stat,
                value,
                unit: ss.stat.unit_of(fact.unit),
                category,
                status: fact.status,
                observations: fact.observations.clone(),
                evidence: fact.evidence.clone(),
            })
        })
        .fold(Vec::new(), |mut acc: Vec<FactBasis>, b| {
            if !acc
                .iter()
                .any(|x| x.signal == b.signal && x.statistic == b.statistic)
            {
                acc.push(b);
            }
            acc
        });
    let mut statement = template::render(&rule.statement, |ss| {
        let fact = facts.get(&ss.signal)?;
        let unit = d.signal(&ss.signal).map_or(fact.unit, |s| s.unit);
        fact.display(ss.stat, unit)
    });
    if let Some(original) = firing.downgraded_from {
        let why = if reads_simulated(&rule.when, facts) {
            "rests on a simulation"
        } else {
            "rests only on operator assertions"
        };
        statement.push_str(&format!(
            " (Would {} {}, but {why}.)",
            original.label().trim_end_matches('s'),
            d.hypothesis(&rule.hypothesis)
                .map_or("it", |h| h.title.as_str())
        ));
    }
    // Knowledge relied on: the rule's own claims plus the definitions of the
    // metrics/fields that supplied its facts.
    let mut claims: Vec<ClaimId> = rule.claims.clone();
    for s in rule.when.signals() {
        if let Some(signal) = d.signal(&s) {
            for source in &signal.source {
                let supplied = facts
                    .get(&s)
                    .is_some_and(|_| pack.claim(&source.claim).is_some());
                if supplied && !claims.contains(&source.claim) {
                    // Only cite sources of the kind that actually supplied data.
                    if matches!(
                        pack.claim(&source.claim),
                        Some(Claim::Metric(_) | Claim::ApiField(_) | Claim::DynamicConfig(_))
                    ) {
                        claims.push(source.claim.clone());
                    }
                }
            }
        }
    }
    DiagnosticFinding {
        id: FindingId::new(format!("F-{}", rule.id)),
        rule: rule.id.clone(),
        hypothesis: rule.hypothesis.clone(),
        effect: firing.effect,
        statement,
        basis,
        claims,
        status: EpistemicStatus::Inference,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{Fact, WindowFit};
    use kyumei_domain::Unit;
    use kyumei_knowledge::KnowledgeBase;

    const PACK: &str = r#"
[pack]
id = "t-1.31"
title = "t"
version = "0"
server_family = "1.31"
verified_server_versions = ["1.31.0"]
description = "t"

[[source]]
id = "src"
kind = "repository"
title = "t"
commit = "abc"
retrieved = 2026-09-30

[[metric]]
id = "sdk.slots"
name = "worker_task_slots_available"
category = "sdk-metric"
instrument = "gauge"
purpose = "free slots"
exposition = [{ name = "temporal_worker_task_slots_available", sdk_family = "core", condition = "default" }]
scope = { sdk = [{ family = "core", revision = "x" }] }
verification = { status = "verified", method = "source-inspection", on = 2026-09-30 }
provenance = [{ source = "src", path = "a", line = 1 }]

[diagnostic]
id = "d"
title = "d"
summary = "d"
path = { description = "p", layers = ["workers"] }

[[symptom]]
id = "slots"
title = "slots"
patterns = ["slots"]
signal = "slots"

[[stage]]
id = "s"
order = 1
title = "s"
question = "q"

[[signal]]
id = "slots"
title = "Slots"
unit = "count"
description = "d"
source = [{ claim = "sdk.slots" }]

[[hypothesis]]
id = "h"
stage = "s"
title = "Slots exhausted"
statement = "h"
question = "h?"
layer = "workers"
primary = true

[[request]]
id = "r"
stage = "s"
title = "r"
why = "w"
signals = ["slots"]
distinguishes = ["h"]
cost = 1
collect = [{ method = "promql", label = "l", body = "b" }]

[[rule]]
id = "headroom"
hypothesis = "h"
effect = "eliminates"
statement = "Slots stayed free."
when = { signal = "slots", stat = "min", op = "ge", value = 5 }
"#;

    #[test]
    fn a_simulated_fact_never_settles_a_hypothesis() {
        let kb = KnowledgeBase::from_files(&[("t/pack.toml", PACK)]).expect("pack");
        let pack = &kb.packs()[0];
        let d = &pack.diagnostics[0];
        let table = |status| FactTable {
            facts: [(
                SignalId::new("slots"),
                Fact {
                    signal: SignalId::new("slots"),
                    unit: Unit::Count,
                    values: [(Statistic::Min, 10.0)].into(),
                    categorical: None,
                    status,
                    observations: Vec::new(),
                    evidence: Vec::new(),
                    coverage: None,
                    fit: WindowFit::Timeless,
                    superseded_assertions: Vec::new(),
                    asserted_typical: None,
                },
            )]
            .into(),
            ..FactTable::default()
        };
        let h = HypothesisId::new("h");
        let observed = evaluate(pack, d, &table(EpistemicStatus::ObservedFact));
        assert_eq!(observed.outcomes[&h].state, HypothesisState::Eliminated);
        let simulated = evaluate(pack, d, &table(EpistemicStatus::Simulated));
        assert_eq!(simulated.outcomes[&h].state, HypothesisState::Weakened);
        assert_eq!(
            simulated.firings[0].downgraded_from,
            Some(Effect::Eliminates)
        );
        assert!(
            simulated.findings[0]
                .statement
                .ends_with("(Would eliminate Slots exhausted, but rests on a simulation.)"),
            "{}",
            simulated.findings[0].statement
        );
        assert!(simulated.findings[0].rests_on_simulation());
    }

    #[test]
    fn states_follow_effects() {
        use Effect::*;
        assert_eq!(state_from(&[]), (HypothesisState::Candidate, false));
        assert_eq!(
            state_from(&[Eliminates]),
            (HypothesisState::Eliminated, false)
        );
        assert_eq!(
            state_from(&[Supports, Eliminates]),
            (HypothesisState::Eliminated, true)
        );
        assert_eq!(
            state_from(&[Establishes, Eliminates]),
            (HypothesisState::Candidate, true)
        );
        assert_eq!(
            state_from(&[Supports, Supports, Weakens]),
            (HypothesisState::Supported, true)
        );
        assert_eq!(
            state_from(&[Supports, Weakens]),
            (HypothesisState::Candidate, true)
        );
        assert_eq!(state_from(&[Weakens]), (HypothesisState::Weakened, false));
    }
}
