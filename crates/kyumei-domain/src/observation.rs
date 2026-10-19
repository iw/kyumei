//! Observations: normalised statements derived from evidence.
//!
//! An observation always points back at the evidence it came from. The raw
//! evidence is never modified; an observation is Kyumei's reading of it, and an
//! operator can correct that reading.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    epistemic::EpistemicStatus,
    ids::{
        ClaimId, CorrectionId, EvidenceId, EvidenceRequestId, ObservationId, SignalId, SimulationId,
    },
    time::TimeWindow,
    units::{Statistic, Unit},
};

/// One normalised statement read from a piece of evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub id: ObservationId,
    pub evidence: EvidenceId,
    /// Observed fact for telemetry/config/output; operator assertion for
    /// typed observations and the symptom report.
    pub status: EpistemicStatus,
    pub subject: Subject,
    /// The diagnostic signal this observation informs, when known.
    pub binding: Option<SignalBinding>,
    /// The period the observation covers, when known.
    pub window: Option<TimeWindow>,
    pub value: ObservedValue,
}

/// What the observation is about, in the words of the evidence itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    /// The metric name exactly as it appeared (e.g. `temporal_num_pollers`).
    pub metric: Option<String>,
    pub labels: BTreeMap<String, String>,
    /// A short description when there is no metric (config key, log pattern,
    /// CLI field, operator statement).
    pub description: Option<String>,
}

impl Subject {
    pub fn metric(name: impl Into<String>, labels: BTreeMap<String, String>) -> Self {
        Self {
            metric: Some(name.into()),
            labels,
            description: None,
        }
    }

    pub fn described(text: impl Into<String>) -> Self {
        Self {
            metric: None,
            labels: BTreeMap::new(),
            description: Some(text.into()),
        }
    }

    /// `name{a="b",c="d"}`, `{a="b"}` for a series without a name (as
    /// aggregations return them), or the description.
    pub fn display(&self) -> String {
        let labels = || {
            let labels: Vec<String> = self
                .labels
                .iter()
                .map(|(k, v)| format!("{k}=\"{v}\""))
                .collect();
            format!("{{{}}}", labels.join(","))
        };
        match &self.metric {
            Some(metric) if self.labels.is_empty() => metric.clone(),
            Some(metric) => format!("{metric}{}", labels()),
            None if self.description.is_none() && !self.labels.is_empty() => labels(),
            None => self
                .description
                .clone()
                .unwrap_or_else(|| "(unlabelled)".to_owned()),
        }
    }
}

/// Why an observation is associated with a signal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "basis", rename_all = "snake_case")]
pub enum BindingBasis {
    /// The metric name and labels matched a knowledge claim's exposition.
    MetricName { exposed_name: String },
    /// The evidence answered a request that expected this signal.
    RequestExpectation { request: EvidenceRequestId },
    /// The operator said which signal this is when submitting.
    OperatorDeclared,
    /// Entered through the structured value form.
    StructuredEntry,
    /// A configuration key or API field mapped directly.
    FieldName { field: String },
    /// A log pattern defined in the knowledge pack matched.
    LogPattern { pattern: String },
    /// Classified from the operator's symptom description.
    SymptomText { matched: String },
    /// An operator correction re-bound it.
    Correction { correction: CorrectionId },
    /// The answer broke the metric down by these labels and had no series
    /// for the signal's values, so the signal reads as zero.
    AbsentFromBreakdown { by: Vec<String> },
    /// The query returned no series while its witness, a counter the same
    /// source keeps whenever this one could count, had samples: nothing was
    /// counted, so the signal reads as zero.
    AbsentWithWitness { witness: String, because: ClaimId },
    /// The result of the knowledge pack's simulation.
    Simulation { simulation: SimulationId },
}

impl BindingBasis {
    pub fn describe(&self) -> String {
        match self {
            Self::MetricName { exposed_name } => format!("metric name `{exposed_name}`"),
            Self::RequestExpectation { request } => format!("answer to request {request}"),
            Self::OperatorDeclared => "declared by operator".to_owned(),
            Self::StructuredEntry => "structured entry".to_owned(),
            Self::FieldName { field } => format!("field `{field}`"),
            Self::LogPattern { pattern } => format!("log pattern `{pattern}`"),
            Self::SymptomText { matched } => format!("symptom text “{matched}”"),
            Self::Correction { correction } => format!("operator correction {correction}"),
            Self::AbsentFromBreakdown { by } => {
                let labels: Vec<String> = by.iter().map(|l| format!("`{l}`")).collect();
                format!("absent from a breakdown by {}", labels.join(", "))
            }
            Self::AbsentWithWitness { witness, because } => {
                format!("no series, while its witness `{witness}` had samples ({because})")
            }
            Self::Simulation { simulation } => format!("simulation `{simulation}`"),
        }
    }
}

/// The link between an observation and a diagnostic signal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalBinding {
    pub signal: SignalId,
    pub basis: BindingBasis,
    /// The knowledge claim describing the metric/field, when identified.
    pub claim: Option<ClaimId>,
}

/// One sample of a time series.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesPoint {
    pub at: Timestamp,
    #[serde(with = "crate::float")]
    pub value: f64,
}

/// The value an observation carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum ObservedValue {
    /// A time series.
    Series {
        unit: Option<Unit>,
        points: Vec<SeriesPoint>,
    },
    /// A single number representing a statistic (e.g. "min 142").
    Scalar {
        unit: Option<Unit>,
        #[serde(with = "crate::float")]
        value: f64,
        statistic: Statistic,
    },
    /// A categorical value (configuration, enum field).
    Categorical { value: String },
    /// Matches of a log pattern.
    PatternMatches {
        pattern: String,
        count: u64,
        examples: Vec<String>,
    },
    /// Free text: the operator's words, or an unparsed excerpt.
    Text { text: String },
    /// A query that returned no series over the observation's window. Not a
    /// value, and never read as zero, but known: asking again returns nothing
    /// more.
    Absent { query: String },
    /// An API field the response leaves out. Protobuf JSON omits a field at its
    /// default, a boolean when it is false, so this is not read as any value; but
    /// asking again returns the same response.
    Omitted { field: String },
}

impl ObservedValue {
    pub fn unit(&self) -> Option<Unit> {
        match self {
            Self::Series { unit, .. } | Self::Scalar { unit, .. } => *unit,
            _ => None,
        }
    }

    pub fn with_unit(&self, new_unit: Unit) -> ObservedValue {
        match self {
            Self::Series { points, .. } => Self::Series {
                unit: Some(new_unit),
                points: points.clone(),
            },
            Self::Scalar {
                value, statistic, ..
            } => Self::Scalar {
                unit: Some(new_unit),
                value: *value,
                statistic: *statistic,
            },
            other => other.clone(),
        }
    }

    /// A short rendering for trails and tables.
    pub fn brief(&self) -> String {
        match self {
            Self::Series { unit, points } => {
                let summary = SeriesSummary::of(points);
                match summary {
                    Some(s) => {
                        let u = unit.unwrap_or(Unit::Count);
                        format!(
                            "{} samples, min {}, max {}, last {}",
                            s.samples,
                            u.format_value(s.min),
                            u.format_value(s.max),
                            u.format_value(s.last)
                        )
                    }
                    None => "empty series".to_owned(),
                }
            }
            Self::Scalar {
                unit,
                value,
                statistic,
            } => format!(
                "{} {}",
                statistic.label(),
                unit.unwrap_or(Unit::Count).format_value(*value)
            ),
            Self::Categorical { value } => format!("= {value}"),
            Self::PatternMatches { count, pattern, .. } => {
                format!("{count} line(s) matching “{pattern}”")
            }
            Self::Text { text } => {
                let mut t: String = text.chars().take(120).collect();
                if text.chars().count() > 120 {
                    t.push('…');
                }
                t
            }
            Self::Absent { .. } => "no series".to_owned(),
            Self::Omitted { .. } => "not in the response".to_owned(),
        }
    }
}

/// Summary statistics of a series.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesSummary {
    pub samples: usize,
    #[serde(with = "crate::float")]
    pub min: f64,
    #[serde(with = "crate::float")]
    pub max: f64,
    #[serde(with = "crate::float")]
    pub mean: f64,
    #[serde(with = "crate::float")]
    pub first: f64,
    #[serde(with = "crate::float")]
    pub last: f64,
    pub first_at: Timestamp,
    pub last_at: Timestamp,
}

impl SeriesSummary {
    /// Summarise the finite points of a series (sorted by time).
    pub fn of(points: &[SeriesPoint]) -> Option<SeriesSummary> {
        let mut finite: Vec<SeriesPoint> = points
            .iter()
            .copied()
            .filter(|p| p.value.is_finite())
            .collect();
        if finite.is_empty() {
            return None;
        }
        finite.sort_by_key(|p| p.at);
        let (mut min, mut max, mut sum) = (f64::INFINITY, f64::NEG_INFINITY, 0.0);
        for p in &finite {
            min = min.min(p.value);
            max = max.max(p.value);
            sum += p.value;
        }
        let first = finite[0];
        let last = finite[finite.len() - 1];
        Some(SeriesSummary {
            samples: finite.len(),
            min,
            max,
            mean: sum / finite.len() as f64,
            first: first.value,
            last: last.value,
            first_at: first.at,
            last_at: last.at,
        })
    }

    pub fn statistic(&self, statistic: Statistic) -> f64 {
        match statistic {
            Statistic::Min => self.min,
            Statistic::Max => self.max,
            Statistic::Mean | Statistic::Typical => self.mean,
            Statistic::First => self.first,
            Statistic::Last => self.last,
            Statistic::Delta => self.last - self.first,
            Statistic::Samples => self.samples as f64,
            // A single series summary has no separate baseline; callers that
            // know the incident window compute it from the points before it.
            Statistic::Baseline => self.first,
        }
    }

    pub fn window(&self) -> TimeWindow {
        TimeWindow {
            start: self.first_at,
            end: self.last_at,
        }
    }
}

/// How tally writes a metric label value: every character other than an ASCII
/// letter, digit or `_` becomes `_`. The server's default metrics framework
/// does this, and so do Go Workers using the SDK's tally options; the same
/// value can therefore arrive in either spelling.
pub fn tally_spelling(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Whether a label value seen in evidence is `expected`, as written or as
/// tally spells it.
pub fn label_value_matches(actual: &str, expected: &str) -> bool {
    actual == expected || actual == tally_spelling(expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarises_series_in_time_order() {
        let t = |s: &str| -> Timestamp { s.parse().expect("ts") };
        let points = vec![
            SeriesPoint {
                at: t("2026-09-30T13:40:00Z"),
                value: 150.0,
            },
            SeriesPoint {
                at: t("2026-09-30T13:35:00Z"),
                value: 190.0,
            },
            SeriesPoint {
                at: t("2026-09-30T13:45:00Z"),
                value: f64::NAN,
            },
        ];
        let s = SeriesSummary::of(&points).expect("summary");
        assert_eq!(s.samples, 2);
        assert_eq!(s.first, 190.0);
        assert_eq!(s.last, 150.0);
        assert_eq!(s.statistic(Statistic::Delta), -40.0);
    }

    #[test]
    fn renders_subjects() {
        let mut labels = BTreeMap::new();
        labels.insert("worker_type".to_owned(), "WorkflowWorker".to_owned());
        let s = Subject::metric("temporal_worker_task_slots_available", labels);
        assert_eq!(
            s.display(),
            "temporal_worker_task_slots_available{worker_type=\"WorkflowWorker\"}"
        );
        // Aggregations return series without a name.
        let unnamed = Subject {
            metric: None,
            labels: [(
                "task_type".to_owned(),
                "VisibilityTaskCloseExecution".to_owned(),
            )]
            .into(),
            description: None,
        };
        assert_eq!(
            unnamed.display(),
            "{task_type=\"VisibilityTaskCloseExecution\"}"
        );
        assert_eq!(
            Subject::described("Workers restarted").display(),
            "Workers restarted"
        );
    }
}
