//! The declarative condition language used by rules, conclusions and
//! hypothesis activation.
//!
//! Conditions use three-valued logic. A comparison against a signal with no
//! facts is `Unknown`, never `False`, and a rule fires only when its condition
//! is `True`. Absence of evidence therefore cannot become evidence of absence.

use std::{collections::BTreeSet, fmt};

use kyumei_domain::{HypothesisId, HypothesisState, SignalId, Statistic};
use serde::Deserialize;

use crate::LoadError;

#[derive(Clone, Debug, PartialEq)]
pub enum Condition {
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
    /// `signal.stat op value`.
    Compare {
        signal: SignalId,
        stat: Statistic,
        op: CompareOp,
        value: f64,
    },
    /// `(numerator.stat / denominator.stat) op value`.
    Ratio {
        numerator: SignalStat,
        denominator: SignalStat,
        op: CompareOp,
        value: f64,
    },
    /// A categorical signal equals `value` (case-insensitive).
    Equals {
        signal: SignalId,
        value: String,
    },
    /// A hypothesis is currently in one of `states`.
    Hypothesis {
        hypothesis: HypothesisId,
        states: Vec<HypothesisState>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SignalStat {
    pub signal: SignalId,
    pub stat: Statistic,
}

impl<'de> Deserialize<'de> for SignalStat {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let token = String::deserialize(deserializer)?;
        parse_signal_stat(&token, "signal statistic").map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for SignalStat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.signal, stat_token(self.stat))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl CompareOp {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::Le => "≤",
            Self::Gt => ">",
            Self::Ge => "≥",
            Self::Eq => "=",
            Self::Ne => "≠",
        }
    }

    pub fn holds(self, left: f64, right: f64) -> bool {
        match self {
            Self::Lt => left < right,
            Self::Le => left <= right,
            Self::Gt => left > right,
            Self::Ge => left >= right,
            Self::Eq => (left - right).abs() < f64::EPSILON,
            Self::Ne => (left - right).abs() >= f64::EPSILON,
        }
    }

    fn parse(token: &str, at: &str) -> Result<Self, LoadError> {
        Ok(match token {
            "lt" | "<" => Self::Lt,
            "le" | "<=" => Self::Le,
            "gt" | ">" => Self::Gt,
            "ge" | ">=" => Self::Ge,
            "eq" | "==" => Self::Eq,
            "ne" | "!=" => Self::Ne,
            other => {
                return Err(LoadError::invalid(
                    at,
                    format!("unknown comparison `{other}`"),
                ));
            }
        })
    }
}

/// Three-valued truth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Truth {
    True,
    False,
    Unknown,
}

impl Truth {
    fn from_bool(b: bool) -> Self {
        if b { Self::True } else { Self::False }
    }

    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

/// What a condition needs to know to be evaluated.
pub trait FactLookup {
    /// The value of `stat` over the facts for `signal`, if any facts exist.
    fn statistic(&self, signal: &SignalId, stat: Statistic) -> Option<f64>;
    /// The categorical value of `signal`, if a fact exists.
    fn categorical(&self, signal: &SignalId) -> Option<String>;
    /// The current state of an active hypothesis.
    fn hypothesis_state(&self, hypothesis: &HypothesisId) -> Option<HypothesisState>;
}

impl Condition {
    pub fn evaluate(&self, facts: &dyn FactLookup) -> Truth {
        match self {
            Self::All(items) => {
                let mut unknown = false;
                for item in items {
                    match item.evaluate(facts) {
                        Truth::False => return Truth::False,
                        Truth::Unknown => unknown = true,
                        Truth::True => {}
                    }
                }
                if unknown { Truth::Unknown } else { Truth::True }
            }
            Self::Any(items) => {
                let mut unknown = false;
                for item in items {
                    match item.evaluate(facts) {
                        Truth::True => return Truth::True,
                        Truth::Unknown => unknown = true,
                        Truth::False => {}
                    }
                }
                if unknown {
                    Truth::Unknown
                } else {
                    Truth::False
                }
            }
            Self::Not(inner) => inner.evaluate(facts).not(),
            Self::Compare {
                signal,
                stat,
                op,
                value,
            } => match facts.statistic(signal, *stat) {
                Some(actual) => Truth::from_bool(op.holds(actual, *value)),
                None => Truth::Unknown,
            },
            Self::Ratio {
                numerator,
                denominator,
                op,
                value,
            } => {
                let n = facts.statistic(&numerator.signal, numerator.stat);
                let d = facts.statistic(&denominator.signal, denominator.stat);
                match (n, d) {
                    (Some(n), Some(d)) if d != 0.0 => Truth::from_bool(op.holds(n / d, *value)),
                    _ => Truth::Unknown,
                }
            }
            Self::Equals { signal, value } => match facts.categorical(signal) {
                Some(actual) => Truth::from_bool(actual.eq_ignore_ascii_case(value)),
                None => Truth::Unknown,
            },
            Self::Hypothesis { hypothesis, states } => match facts.hypothesis_state(hypothesis) {
                Some(state) => Truth::from_bool(states.contains(&state)),
                // A dormant hypothesis is in no state.
                None => Truth::False,
            },
        }
    }

    /// The signal statistics known whenever the condition evaluates to
    /// `truth`. A comparison is only ever True or False over known values, so
    /// text shown because a condition held can quote what it read. `held` says
    /// what a hypothesis in one of the given states ensures.
    pub fn known_when(
        &self,
        truth: bool,
        held: &dyn Fn(&HypothesisId, &[HypothesisState]) -> BTreeSet<SignalStat>,
    ) -> BTreeSet<SignalStat> {
        match self {
            Self::All(items) | Self::Any(items) => {
                // `all` is True when every item is, and False when one is;
                // `any` the other way round. When every item has the value,
                // all of their statistics are known; when only one need have
                // it, only what they all share.
                let every = matches!(self, Self::All(_)) == truth;
                let sets = items.iter().map(|c| c.known_when(truth, held));
                if every {
                    sets.flatten().collect()
                } else {
                    intersection(sets)
                }
            }
            Self::Not(inner) => inner.known_when(!truth, held),
            Self::Compare { signal, stat, .. } => BTreeSet::from([SignalStat {
                signal: signal.clone(),
                stat: *stat,
            }]),
            Self::Ratio {
                numerator,
                denominator,
                ..
            } => BTreeSet::from([numerator.clone(), denominator.clone()]),
            Self::Equals { signal, .. } => BTreeSet::from([SignalStat {
                signal: signal.clone(),
                stat: Statistic::Last,
            }]),
            Self::Hypothesis { hypothesis, states } if truth => held(hypothesis, states),
            // A dormant hypothesis is in no state, so being outside the states
            // says nothing about the facts.
            Self::Hypothesis { .. } => BTreeSet::new(),
        }
    }

    /// Signals (with statistics) the condition reads, in order of appearance.
    pub fn signal_stats(&self) -> Vec<SignalStat> {
        let mut out = Vec::new();
        self.walk(&mut |c| match c {
            Self::Compare { signal, stat, .. } => out.push(SignalStat {
                signal: signal.clone(),
                stat: *stat,
            }),
            Self::Ratio {
                numerator,
                denominator,
                ..
            } => {
                out.push(numerator.clone());
                out.push(denominator.clone());
            }
            Self::Equals { signal, .. } => out.push(SignalStat {
                signal: signal.clone(),
                stat: Statistic::Last,
            }),
            _ => {}
        });
        out
    }

    pub fn signals(&self) -> Vec<SignalId> {
        let mut signals: Vec<SignalId> =
            self.signal_stats().into_iter().map(|s| s.signal).collect();
        signals.sort();
        signals.dedup();
        signals
    }

    pub fn hypotheses(&self) -> Vec<HypothesisId> {
        let mut out = Vec::new();
        self.walk(&mut |c| {
            if let Self::Hypothesis { hypothesis, .. } = c {
                out.push(hypothesis.clone());
            }
        });
        out.sort();
        out.dedup();
        out
    }

    /// Whether the condition, or any part of it, reads hypothesis states.
    pub fn reads_hypotheses(&self) -> bool {
        !self.hypotheses().is_empty()
    }

    fn walk(&self, visit: &mut dyn FnMut(&Condition)) {
        visit(self);
        match self {
            Self::All(items) | Self::Any(items) => items.iter().for_each(|c| c.walk(visit)),
            Self::Not(inner) => inner.walk(visit),
            _ => {}
        }
    }

    /// A compact rendering for provenance displays.
    pub fn describe(&self) -> String {
        match self {
            Self::All(items) => items
                .iter()
                .map(Condition::describe)
                .collect::<Vec<_>>()
                .join(" and "),
            Self::Any(items) => format!(
                "({})",
                items
                    .iter()
                    .map(Condition::describe)
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
            Self::Not(inner) => format!("not ({})", inner.describe()),
            Self::Compare {
                signal,
                stat,
                op,
                value,
            } => format!("{signal}.{} {} {value}", stat_token(*stat), op.symbol()),
            Self::Ratio {
                numerator,
                denominator,
                op,
                value,
            } => format!("{numerator} / {denominator} {} {value}", op.symbol()),
            Self::Equals { signal, value } => format!("{signal} = {value}"),
            Self::Hypothesis { hypothesis, states } => format!(
                "{hypothesis} is {}",
                states
                    .iter()
                    .map(|s| s.label())
                    .collect::<Vec<_>>()
                    .join("/")
            ),
        }
    }
}

pub(crate) fn stat_token(stat: Statistic) -> &'static str {
    match stat {
        Statistic::Min => "min",
        Statistic::Max => "max",
        Statistic::Mean => "mean",
        Statistic::First => "first",
        Statistic::Last => "last",
        Statistic::Delta => "delta",
        Statistic::Samples => "samples",
        Statistic::Typical => "typical",
        Statistic::Baseline => "baseline",
    }
}

impl<'de> Deserialize<'de> for Condition {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        RawCondition::deserialize(deserializer)?
            .into_condition("condition")
            .map_err(serde::de::Error::custom)
    }
}

/// The on-disk form of a condition. Exactly one shape must be present.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCondition {
    #[serde(default)]
    all: Option<Vec<RawCondition>>,
    #[serde(default)]
    any: Option<Vec<RawCondition>>,
    #[serde(default)]
    not: Option<Box<RawCondition>>,
    #[serde(default)]
    signal: Option<SignalId>,
    #[serde(default)]
    stat: Option<String>,
    #[serde(default)]
    op: Option<String>,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    ratio: Option<RawRatio>,
    #[serde(default)]
    equals: Option<String>,
    #[serde(default)]
    hypothesis: Option<HypothesisId>,
    #[serde(default)]
    state: Option<Vec<HypothesisState>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRatio {
    numerator: String,
    denominator: String,
}

fn parse_signal_stat(token: &str, at: &str) -> Result<SignalStat, LoadError> {
    let (signal, stat) = token
        .rsplit_once('.')
        .ok_or_else(|| LoadError::invalid(at, format!("`{token}` must be `signal.stat`")))?;
    let stat = stat
        .parse::<Statistic>()
        .map_err(|e| LoadError::invalid(at, e))?;
    Ok(SignalStat {
        signal: SignalId::new(signal),
        stat,
    })
}

impl RawCondition {
    pub fn into_condition(self, at: &str) -> Result<Condition, LoadError> {
        let shapes = [
            self.all.is_some(),
            self.any.is_some(),
            self.not.is_some(),
            self.ratio.is_some(),
            self.equals.is_some(),
            self.hypothesis.is_some(),
            self.signal.is_some() && self.equals.is_none(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if shapes != 1 {
            return Err(LoadError::invalid(
                at,
                "a condition must have exactly one of `all`, `any`, `not`, `signal`+`stat`+`op`+`value`, `ratio`, `signal`+`equals`, or `hypothesis`+`state`",
            ));
        }
        if let Some(items) = self.all {
            if items.is_empty() {
                return Err(LoadError::invalid(at, "`all` must not be empty"));
            }
            return items
                .into_iter()
                .enumerate()
                .map(|(i, c)| c.into_condition(&format!("{at}.all[{i}]")))
                .collect::<Result<Vec<_>, _>>()
                .map(Condition::All);
        }
        if let Some(items) = self.any {
            if items.is_empty() {
                return Err(LoadError::invalid(at, "`any` must not be empty"));
            }
            return items
                .into_iter()
                .enumerate()
                .map(|(i, c)| c.into_condition(&format!("{at}.any[{i}]")))
                .collect::<Result<Vec<_>, _>>()
                .map(Condition::Any);
        }
        if let Some(inner) = self.not {
            return Ok(Condition::Not(Box::new(
                inner.into_condition(&format!("{at}.not"))?,
            )));
        }
        if let Some(ratio) = self.ratio {
            let op = CompareOp::parse(
                self.op
                    .as_deref()
                    .ok_or_else(|| LoadError::invalid(at, "`ratio` needs `op`"))?,
                at,
            )?;
            let value = self
                .value
                .ok_or_else(|| LoadError::invalid(at, "`ratio` needs `value`"))?;
            return Ok(Condition::Ratio {
                numerator: parse_signal_stat(&ratio.numerator, at)?,
                denominator: parse_signal_stat(&ratio.denominator, at)?,
                op,
                value,
            });
        }
        if let Some(value) = self.equals {
            let signal = self
                .signal
                .ok_or_else(|| LoadError::invalid(at, "`equals` needs `signal`"))?;
            return Ok(Condition::Equals { signal, value });
        }
        if let Some(hypothesis) = self.hypothesis {
            let states = self
                .state
                .filter(|s| !s.is_empty())
                .ok_or_else(|| LoadError::invalid(at, "`hypothesis` needs a non-empty `state`"))?;
            return Ok(Condition::Hypothesis { hypothesis, states });
        }
        let signal = self
            .signal
            .ok_or_else(|| LoadError::invalid(at, "missing `signal`"))?;
        let stat = self
            .stat
            .as_deref()
            .ok_or_else(|| LoadError::invalid(at, "comparison needs `stat`"))?
            .parse::<Statistic>()
            .map_err(|e| LoadError::invalid(at, e))?;
        let op = CompareOp::parse(
            self.op
                .as_deref()
                .ok_or_else(|| LoadError::invalid(at, "comparison needs `op`"))?,
            at,
        )?;
        let value = self
            .value
            .ok_or_else(|| LoadError::invalid(at, "comparison needs `value`"))?;
        Ok(Condition::Compare {
            signal,
            stat,
            op,
            value,
        })
    }
}

/// The statistics every set holds; none when there are no sets, since nothing
/// is then known to hold.
pub(crate) fn intersection(
    mut sets: impl Iterator<Item = BTreeSet<SignalStat>>,
) -> BTreeSet<SignalStat> {
    let Some(first) = sets.next() else {
        return BTreeSet::new();
    };
    sets.fold(first, |known, set| {
        known.intersection(&set).cloned().collect()
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    struct Facts(BTreeMap<(String, Statistic), f64>);

    impl FactLookup for Facts {
        fn statistic(&self, signal: &SignalId, stat: Statistic) -> Option<f64> {
            self.0.get(&(signal.to_string(), stat)).copied()
        }
        fn categorical(&self, _: &SignalId) -> Option<String> {
            None
        }
        fn hypothesis_state(&self, _: &HypothesisId) -> Option<HypothesisState> {
            None
        }
    }

    fn cmp(signal: &str, stat: Statistic, op: CompareOp, value: f64) -> Condition {
        Condition::Compare {
            signal: signal.into(),
            stat,
            op,
            value,
        }
    }

    #[test]
    fn missing_facts_are_unknown_not_false() {
        let facts = Facts(BTreeMap::new());
        let c = cmp("slots", Statistic::Min, CompareOp::Ge, 5.0);
        assert_eq!(c.evaluate(&facts), Truth::Unknown);
        assert_eq!(Condition::Not(Box::new(c)).evaluate(&facts), Truth::Unknown);
    }

    #[test]
    fn a_condition_that_holds_has_read_its_statistics() {
        let stat = |signal: &str, stat| SignalStat {
            signal: signal.into(),
            stat,
        };
        let none = |_: &HypothesisId, _: &[HypothesisState]| BTreeSet::new();
        let max = cmp("a", Statistic::Max, CompareOp::Ge, 5.0);
        let ratio = Condition::Ratio {
            numerator: stat("a", Statistic::Max),
            denominator: stat("a", Statistic::Baseline),
            op: CompareOp::Ge,
            value: 3.0,
        };
        let both = Condition::All(vec![max.clone(), ratio.clone()]);
        assert_eq!(
            both.known_when(true, &none),
            BTreeSet::from([stat("a", Statistic::Max), stat("a", Statistic::Baseline)])
        );
        // Either item may be the one that holds: only what they share is known.
        let either = Condition::Any(vec![max.clone(), ratio.clone()]);
        assert_eq!(
            either.known_when(true, &none),
            BTreeSet::from([stat("a", Statistic::Max)])
        );
        // `not any` holds only when every item is False, over known values.
        let neither = Condition::Not(Box::new(either));
        assert_eq!(
            neither.known_when(true, &none),
            both.known_when(true, &none)
        );
        let held = |h: &HypothesisId, states: &[HypothesisState]| {
            if h.as_str() == "h" && states == [HypothesisState::Established] {
                BTreeSet::from([stat("b", Statistic::Baseline)])
            } else {
                BTreeSet::new()
            }
        };
        let established = Condition::Hypothesis {
            hypothesis: HypothesisId::new("h"),
            states: vec![HypothesisState::Established],
        };
        assert_eq!(
            established.known_when(true, &held),
            BTreeSet::from([stat("b", Statistic::Baseline)])
        );
        assert!(
            Condition::Not(Box::new(established))
                .known_when(true, &held)
                .is_empty()
        );
    }

    #[test]
    fn three_valued_connectives() {
        let mut map = BTreeMap::new();
        map.insert(("a".to_owned(), Statistic::Max), 10.0);
        let facts = Facts(map);
        let t = cmp("a", Statistic::Max, CompareOp::Gt, 5.0);
        let f = cmp("a", Statistic::Max, CompareOp::Lt, 5.0);
        let u = cmp("b", Statistic::Max, CompareOp::Lt, 5.0);
        assert_eq!(
            Condition::All(vec![t.clone(), u.clone()]).evaluate(&facts),
            Truth::Unknown
        );
        assert_eq!(
            Condition::All(vec![f.clone(), u.clone()]).evaluate(&facts),
            Truth::False
        );
        assert_eq!(
            Condition::Any(vec![t, u.clone()]).evaluate(&facts),
            Truth::True
        );
        assert_eq!(Condition::Any(vec![f, u]).evaluate(&facts), Truth::Unknown);
    }
}
