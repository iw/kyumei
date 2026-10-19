//! Facts: what the evidence says about each diagnostic signal.
//!
//! Observations bound to a signal are converted to the signal's unit, limited
//! to the incident window when it is known, and combined according to the
//! signal's policy. Observed facts take precedence over operator assertions
//! for the same signal; the assertions are kept so contradictions can be
//! reported.

use std::collections::BTreeMap;

use kyumei_domain::{
    EffectiveObservation, EpistemicStatus, EvidenceId, IncidentWindow, ObservationId,
    ObservedValue, SeriesPoint, SeriesSummary, SignalId, Statistic, TimeWindow, Unit,
};
use kyumei_knowledge::{CombinePolicy, DiagnosticDef, SignalDef};
use serde::{Deserialize, Serialize};

/// How a fact's coverage relates to the incident window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowFit {
    /// Statistics were computed from samples inside the incident window.
    Within,
    /// The incident window is unknown; statistics cover everything supplied.
    IncidentUnknown,
    /// The observation carries no time (configuration, an undated snapshot).
    Timeless,
}

/// Everything known about one signal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub signal: SignalId,
    pub unit: Unit,
    pub values: BTreeMap<Statistic, f64>,
    pub categorical: Option<String>,
    pub status: EpistemicStatus,
    pub observations: Vec<ObservationId>,
    pub evidence: Vec<EvidenceId>,
    pub coverage: Option<TimeWindow>,
    pub fit: WindowFit,
    /// Operator assertions superseded by observed facts.
    pub superseded_assertions: Vec<ObservationId>,
    /// The asserted typical value when observed facts superseded it.
    pub asserted_typical: Option<f64>,
}

impl Fact {
    pub fn get(&self, stat: Statistic) -> Option<f64> {
        self.values.get(&stat).copied().or(match stat {
            Statistic::Typical => self.values.get(&Statistic::Mean).copied(),
            _ => None,
        })
    }

    /// `stat` as a statement quotes it: a number in `unit`, or, for `last`
    /// of a categorical signal, its value as observed.
    pub fn display(&self, stat: Statistic, unit: Unit) -> Option<String> {
        match self.get(stat) {
            Some(value) => Some(stat.unit_of(unit).format_value(value)),
            None if stat == Statistic::Last => self.categorical.clone(),
            None => None,
        }
    }
}

/// An observation that could not inform a signal, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SetAside {
    pub observation: ObservationId,
    pub evidence: EvidenceId,
    pub signal: Option<SignalId>,
    pub reason: SetAsideReason,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SetAsideReason {
    Excluded,
    Unbound,
    NotInDiagnostic,
    OutsideWindow { coverage: Option<TimeWindow> },
    UnitMismatch { from: Unit, to: Unit },
    NoValue,
}

impl SetAsideReason {
    pub fn describe(&self) -> String {
        match self {
            Self::Excluded => "excluded by the operator".to_owned(),
            Self::Unbound => "not related to a diagnostic signal".to_owned(),
            Self::NotInDiagnostic => "its signal is not part of this diagnostic".to_owned(),
            Self::OutsideWindow { coverage } => match coverage {
                Some(w) => format!("covers {}, outside the incident window", w.display_utc()),
                None => "outside the incident window".to_owned(),
            },
            Self::UnitMismatch { from, to } => format!("{from} cannot be converted to {to}"),
            Self::NoValue => "carries no usable value".to_owned(),
        }
    }
}

/// A signal whose queries returned no series, or whose API field the response
/// left out, and nothing else informs it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Absence {
    pub signal: SignalId,
    pub queries: Vec<String>,
    pub observations: Vec<ObservationId>,
    pub evidence: Vec<EvidenceId>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FactTable {
    pub facts: BTreeMap<SignalId, Fact>,
    pub set_aside: Vec<SetAside>,
    /// Signals asked for and found to have no series. They have no value,
    /// so rules read them as unknown, but they need not be asked for again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub absent: BTreeMap<SignalId, Absence>,
}

impl FactTable {
    pub fn get(&self, signal: &SignalId) -> Option<&Fact> {
        self.facts.get(signal)
    }

    /// Whether `signal` was asked for and no series answered.
    pub fn is_absent(&self, signal: &SignalId) -> bool {
        self.absent.contains_key(signal)
    }
}

/// One observation's contribution to a signal.
struct Contribution {
    observation: ObservationId,
    evidence: EvidenceId,
    status: EpistemicStatus,
    kind: ContributionKind,
    coverage: Option<TimeWindow>,
    fit: WindowFit,
}

enum ContributionKind {
    Series {
        points: Vec<SeriesPoint>,
        baseline: Option<f64>,
    },
    Scalar {
        stat: Statistic,
        value: f64,
    },
    Categorical(String),
}

pub fn build(
    diagnostic: &DiagnosticDef,
    observations: &[EffectiveObservation],
    incident: &IncidentWindow,
) -> FactTable {
    let window = incident.as_window();
    let mut table = FactTable::default();
    let mut contributions: BTreeMap<SignalId, Vec<Contribution>> = BTreeMap::new();

    for eff in observations {
        let obs = &eff.observation;
        let set_aside = |reason: SetAsideReason| SetAside {
            observation: obs.id.clone(),
            evidence: obs.evidence.clone(),
            signal: obs.binding.as_ref().map(|b| b.signal.clone()),
            reason,
        };
        if eff.excluded {
            table.set_aside.push(set_aside(SetAsideReason::Excluded));
            continue;
        }
        let Some(binding) = &obs.binding else {
            if !matches!(obs.value, ObservedValue::Text { .. }) {
                table.set_aside.push(set_aside(SetAsideReason::Unbound));
            }
            continue;
        };
        let Some(signal) = diagnostic.signal(&binding.signal) else {
            table
                .set_aside
                .push(set_aside(SetAsideReason::NotInDiagnostic));
            continue;
        };
        if let ObservedValue::Absent { query } | ObservedValue::Omitted { field: query } =
            &obs.value
        {
            let a = table
                .absent
                .entry(signal.id.clone())
                .or_insert_with(|| Absence {
                    signal: signal.id.clone(),
                    queries: Vec::new(),
                    observations: Vec::new(),
                    evidence: Vec::new(),
                });
            if !a.queries.contains(query) {
                a.queries.push(query.clone());
            }
            a.observations.push(obs.id.clone());
            if !a.evidence.contains(&obs.evidence) {
                a.evidence.push(obs.evidence.clone());
            }
            continue;
        }
        let convert = |value: f64, unit: Option<Unit>| -> Result<f64, SetAsideReason> {
            let from = unit.unwrap_or(signal.unit);
            from.convert(value, signal.unit)
                .ok_or(SetAsideReason::UnitMismatch {
                    from,
                    to: signal.unit,
                })
        };
        let kind_and_fit: Result<
            (ContributionKind, Option<TimeWindow>, WindowFit),
            SetAsideReason,
        > = match &obs.value {
            ObservedValue::Series { unit, points } => {
                let converted: Result<Vec<SeriesPoint>, SetAsideReason> = points
                    .iter()
                    .filter(|p| p.value.is_finite())
                    .map(|p| convert(p.value, *unit).map(|value| SeriesPoint { at: p.at, value }))
                    .collect();
                match converted {
                    Err(e) => Err(e),
                    Ok(points) if points.is_empty() => Err(SetAsideReason::NoValue),
                    Ok(points) => series_contribution(points, window),
                }
            }
            // A dated point-in-time reading (an API snapshot) is one sample.
            ObservedValue::Scalar {
                unit,
                value,
                statistic: Statistic::Last,
            } if obs.window.is_some() => convert(*value, *unit).and_then(|v| {
                let at = obs.window.map(|w| w.end).unwrap_or_default();
                series_contribution(vec![SeriesPoint { at, value: v }], window)
            }),
            ObservedValue::Scalar {
                unit,
                value,
                statistic,
            } => convert(*value, *unit).and_then(|v| {
                let (fit, ok) = match (obs.window, window) {
                    (Some(ow), Some(iw)) => (WindowFit::Within, ow.overlaps(&iw)),
                    (None, _) => (WindowFit::Timeless, true),
                    (Some(_), None) => (WindowFit::IncidentUnknown, true),
                };
                if ok {
                    Ok((
                        ContributionKind::Scalar {
                            stat: *statistic,
                            value: v,
                        },
                        obs.window,
                        fit,
                    ))
                } else {
                    Err(SetAsideReason::OutsideWindow {
                        coverage: obs.window,
                    })
                }
            }),
            ObservedValue::Categorical { value } => Ok((
                ContributionKind::Categorical(value.clone()),
                obs.window,
                WindowFit::Timeless,
            )),
            ObservedValue::PatternMatches { count, .. } => Ok((
                ContributionKind::Scalar {
                    stat: Statistic::Samples,
                    value: *count as f64,
                },
                obs.window,
                WindowFit::Timeless,
            )),
            ObservedValue::Text { .. }
            | ObservedValue::Absent { .. }
            | ObservedValue::Omitted { .. } => Err(SetAsideReason::NoValue),
        };
        match kind_and_fit {
            Ok((kind, coverage, fit)) => {
                contributions
                    .entry(signal.id.clone())
                    .or_default()
                    .push(Contribution {
                        observation: obs.id.clone(),
                        evidence: obs.evidence.clone(),
                        status: obs.status,
                        kind,
                        coverage,
                        fit,
                    })
            }
            Err(reason) => table.set_aside.push(set_aside(reason)),
        }
    }

    for (signal_id, items) in contributions {
        let Some(signal) = diagnostic.signal(&signal_id) else {
            continue;
        };
        if let Some(fact) = combine(signal, items) {
            table.facts.insert(signal_id, fact);
        }
    }
    // Any value outweighs a query that found none.
    table.absent.retain(|s, _| !table.facts.contains_key(s));
    table
}

/// Restrict a series to the incident window and compute its baseline.
fn series_contribution(
    points: Vec<SeriesPoint>,
    window: Option<TimeWindow>,
) -> Result<(ContributionKind, Option<TimeWindow>, WindowFit), SetAsideReason> {
    let full = SeriesSummary::of(&points).map(|s| s.window());
    match window {
        None => Ok((
            ContributionKind::Series {
                points,
                baseline: None,
            },
            full,
            WindowFit::IncidentUnknown,
        )),
        Some(w) => {
            let before: Vec<f64> = points
                .iter()
                .filter(|p| p.at < w.start)
                .map(|p| p.value)
                .collect();
            let inside: Vec<SeriesPoint> = points
                .iter()
                .copied()
                .filter(|p| w.contains(p.at))
                .collect();
            if inside.is_empty() {
                return Err(SetAsideReason::OutsideWindow { coverage: full });
            }
            let baseline =
                (!before.is_empty()).then(|| before.iter().sum::<f64>() / before.len() as f64);
            let coverage = SeriesSummary::of(&inside).map(|s| s.window());
            Ok((
                ContributionKind::Series {
                    points: inside,
                    baseline,
                },
                coverage,
                WindowFit::Within,
            ))
        }
    }
}

fn combine(signal: &SignalDef, items: Vec<Contribution>) -> Option<Fact> {
    let observed: Vec<&Contribution> = items
        .iter()
        .filter(|c| c.status == EpistemicStatus::ObservedFact)
        .collect();
    let asserted: Vec<&Contribution> = items
        .iter()
        .filter(|c| c.status != EpistemicStatus::ObservedFact)
        .collect();
    let (used, superseded, status) = if !observed.is_empty() {
        (observed, asserted, EpistemicStatus::ObservedFact)
    } else if !asserted.is_empty()
        && asserted
            .iter()
            .all(|c| c.status == EpistemicStatus::Simulated)
    {
        (asserted, Vec::new(), EpistemicStatus::Simulated)
    } else {
        (asserted, Vec::new(), EpistemicStatus::OperatorAssertion)
    };
    if used.is_empty() {
        return None;
    }

    let mut values: BTreeMap<Statistic, f64> = BTreeMap::new();
    let mut categorical = None;

    // Series.
    let series: Vec<(&Vec<SeriesPoint>, Option<f64>)> = used
        .iter()
        .filter_map(|c| match &c.kind {
            ContributionKind::Series { points, baseline } => Some((points, *baseline)),
            _ => None,
        })
        .collect();
    if !series.is_empty() {
        let summed = (signal.combine == CombinePolicy::Sum && series.len() > 1)
            .then(|| sum_aligned(&series.iter().map(|(p, _)| *p).collect::<Vec<_>>()))
            .flatten();
        match summed {
            Some(points) => {
                if let Some(s) = SeriesSummary::of(&points) {
                    insert_summary(&mut values, &s);
                }
                let baselines: Vec<f64> = series.iter().filter_map(|(_, b)| *b).collect();
                if baselines.len() == series.len() {
                    values.insert(Statistic::Baseline, baselines.iter().sum());
                }
            }
            None => {
                let summaries: Vec<SeriesSummary> = series
                    .iter()
                    .filter_map(|(p, _)| SeriesSummary::of(p))
                    .collect();
                if !summaries.is_empty() {
                    let samples: usize = summaries.iter().map(|s| s.samples).sum();
                    let n = summaries.len() as f64;
                    values.insert(
                        Statistic::Min,
                        summaries
                            .iter()
                            .map(|s| s.min)
                            .fold(f64::INFINITY, f64::min),
                    );
                    values.insert(
                        Statistic::Max,
                        summaries
                            .iter()
                            .map(|s| s.max)
                            .fold(f64::NEG_INFINITY, f64::max),
                    );
                    values.insert(
                        Statistic::Mean,
                        summaries
                            .iter()
                            .map(|s| s.mean * s.samples as f64)
                            .sum::<f64>()
                            / samples as f64,
                    );
                    let first = summaries.iter().map(|s| s.first).sum::<f64>() / n;
                    let last = summaries.iter().map(|s| s.last).sum::<f64>() / n;
                    values.insert(Statistic::First, first);
                    values.insert(Statistic::Last, last);
                    values.insert(Statistic::Delta, last - first);
                    values.insert(Statistic::Samples, samples as f64);
                    let baselines: Vec<f64> = series.iter().filter_map(|(_, b)| *b).collect();
                    if !baselines.is_empty() {
                        values.insert(
                            Statistic::Baseline,
                            baselines.iter().sum::<f64>() / baselines.len() as f64,
                        );
                    }
                }
            }
        }
    }

    // Scalars refine or supply statistics.
    for c in &used {
        match &c.kind {
            ContributionKind::Scalar { stat, value } => {
                let entry = values.entry(*stat).or_insert(*value);
                *entry = match stat {
                    Statistic::Min => entry.min(*value),
                    Statistic::Max => entry.max(*value),
                    _ => *value,
                };
            }
            ContributionKind::Categorical(value) => {
                categorical = Some(value.clone());
                if let Ok(n) = value.trim().parse::<f64>() {
                    for stat in [
                        Statistic::Min,
                        Statistic::Max,
                        Statistic::Mean,
                        Statistic::Last,
                    ] {
                        values.entry(stat).or_insert(n);
                    }
                }
            }
            ContributionKind::Series { .. } => {}
        }
    }
    // An asserted "typical" value is all an operator estimate offers; let it
    // stand for the other magnitude statistics so rules can read it (the
    // finding records that it rests on an assertion).
    if let Some(typical) = values.get(&Statistic::Typical).copied() {
        for stat in [
            Statistic::Min,
            Statistic::Max,
            Statistic::Mean,
            Statistic::Last,
        ] {
            values.entry(stat).or_insert(typical);
        }
    }

    let asserted_typical = superseded.iter().find_map(|c| match &c.kind {
        ContributionKind::Scalar {
            stat: Statistic::Typical,
            value,
        } => Some(*value),
        _ => None,
    });
    let coverage = used
        .iter()
        .filter_map(|c| c.coverage)
        .reduce(|a, b| a.union(&b));
    let fit = if used.iter().any(|c| c.fit == WindowFit::Within) {
        WindowFit::Within
    } else if used.iter().any(|c| c.fit == WindowFit::IncidentUnknown) {
        WindowFit::IncidentUnknown
    } else {
        WindowFit::Timeless
    };
    let mut evidence: Vec<EvidenceId> = used.iter().map(|c| c.evidence.clone()).collect();
    evidence.sort();
    evidence.dedup();
    Some(Fact {
        signal: signal.id.clone(),
        unit: signal.unit,
        values,
        categorical,
        status,
        observations: used.iter().map(|c| c.observation.clone()).collect(),
        evidence,
        coverage,
        fit,
        superseded_assertions: superseded.iter().map(|c| c.observation.clone()).collect(),
        asserted_typical,
    })
}

fn insert_summary(values: &mut BTreeMap<Statistic, f64>, s: &SeriesSummary) {
    for stat in [
        Statistic::Min,
        Statistic::Max,
        Statistic::Mean,
        Statistic::First,
        Statistic::Last,
        Statistic::Delta,
        Statistic::Samples,
    ] {
        values.insert(stat, s.statistic(stat));
    }
}

/// Point-wise sum over timestamps present in every series.
fn sum_aligned(series: &[&Vec<SeriesPoint>]) -> Option<Vec<SeriesPoint>> {
    let mut by_time: BTreeMap<jiff::Timestamp, (usize, f64)> = BTreeMap::new();
    for points in series {
        for p in points.iter() {
            let e = by_time.entry(p.at).or_insert((0, 0.0));
            e.0 += 1;
            e.1 += p.value;
        }
    }
    let summed: Vec<SeriesPoint> = by_time
        .into_iter()
        .filter(|(_, (n, _))| *n == series.len())
        .map(|(at, (_, value))| SeriesPoint { at, value })
        .collect();
    (!summed.is_empty()).then_some(summed)
}
