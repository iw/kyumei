//! Statement templates.
//!
//! Rule and conclusion statements are written in the knowledge pack with
//! `{signal.stat}` placeholders, e.g. `{workflow_slots_available.min}`. The
//! engine renders them from the same facts the rule evaluated, so the prose can
//! never quote a number the rule did not see.
//!
//! Text in square brackets is an optional segment, kept without its brackets
//! only when every placeholder in it has a value. A baseline needs samples from
//! before the incident window, which evidence does not always reach, so a
//! comparison with one is written as an optional segment and the sentence reads
//! whole without it.
//!
//! A placeholder can divide one statistic by another, `{a.stat/b.stat}`, where
//! the quotient has a unit Kyumei can name: tasks waiting over tasks dispatched
//! a second is how long they take to clear. A quotient by zero has no value.

use kyumei_domain::{SignalId, Statistic, Unit, format_duration};
use regex::Regex;
use std::sync::LazyLock;

use crate::condition::SignalStat;

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{([a-z0-9_]+)\.([a-z]+)(?:/([a-z0-9_]+)\.([a-z]+))?\}")
        .expect("placeholder pattern")
});

/// A placeholder: one statistic, or one divided by another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placeholder {
    pub stat: SignalStat,
    /// The divisor of a quotient.
    pub per: Option<SignalStat>,
}

impl Placeholder {
    /// The statistics it reads.
    pub fn stats(&self) -> impl Iterator<Item = &SignalStat> {
        std::iter::once(&self.stat).chain(&self.per)
    }
}

/// What a statistic renders as: its text, and its number in its unit when it
/// has one, which a quotient needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    pub text: String,
    pub number: Option<(f64, Unit)>,
}

/// What a missing value renders as outside an optional segment.
const UNKNOWN: &str = "unknown";

/// A piece of a template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part<'a> {
    /// Text that is always rendered.
    Text(&'a str),
    /// The inside of an optional segment.
    Optional(&'a str),
}

/// Split `template` into text and optional segments.
pub fn parts(template: &str) -> Result<Vec<Part<'_>>, String> {
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find(['[', ']']) {
        if rest[open..].starts_with(']') {
            return Err("`]` closes no optional segment".to_owned());
        }
        let inside = &rest[open + 1..];
        let close = match inside.find(['[', ']']) {
            Some(at) if inside[at..].starts_with(']') => at,
            Some(_) => return Err("an optional segment opens inside another".to_owned()),
            None => return Err("an optional segment is never closed".to_owned()),
        };
        if open > 0 {
            parts.push(Part::Text(&rest[..open]));
        }
        parts.push(Part::Optional(&inside[..close]));
        rest = &inside[close + 1..];
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest));
    }
    Ok(parts)
}

/// Every placeholder in `template`.
pub fn placeholders(template: &str) -> Result<Vec<Placeholder>, String> {
    PLACEHOLDER
        .captures_iter(template)
        .map(|caps| placeholder(&caps))
        .collect()
}

fn placeholder(caps: &regex::Captures<'_>) -> Result<Placeholder, String> {
    let stat = |signal: &str, stat: &str| -> Result<SignalStat, String> {
        Ok(SignalStat {
            signal: SignalId::new(signal),
            stat: stat.parse::<Statistic>()?,
        })
    };
    Ok(Placeholder {
        stat: stat(&caps[1], &caps[2])?,
        per: match (caps.get(3), caps.get(4)) {
            (Some(signal), Some(s)) => Some(stat(signal.as_str(), s.as_str())?),
            _ => None,
        },
    })
}

/// Replace placeholders with what `value` gives each statistic. An optional
/// segment with a value missing is left out; elsewhere a missing value renders
/// as `unknown`.
pub fn render(template: &str, value: impl Fn(&SignalStat) -> Option<Value>) -> String {
    match parts(template) {
        Ok(parts) => parts
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) => fill(text, &value, Some(UNKNOWN)),
                Part::Optional(text) => fill(text, &value, None),
            })
            .collect(),
        // Validation rejects such brackets, so render the text as written.
        Err(_) => fill(template, &value, Some(UNKNOWN)).unwrap_or_default(),
    }
}

/// `text` with its placeholders replaced. A missing value is written as
/// `missing`, or, without one, leaves nothing to render.
fn fill(
    text: &str,
    value: &impl Fn(&SignalStat) -> Option<Value>,
    missing: Option<&str>,
) -> Option<String> {
    let mut complete = true;
    let filled = PLACEHOLDER.replace_all(text, |caps: &regex::Captures<'_>| {
        let Ok(p) = placeholder(caps) else {
            return caps[0].to_owned();
        };
        rendered(&p, value).unwrap_or_else(|| {
            complete = false;
            missing.unwrap_or_default().to_owned()
        })
    });
    (complete || missing.is_some()).then(|| filled.into_owned())
}

/// How `p` reads. A quotient that yields seconds is an estimate of how long
/// something takes, so it reads in minutes or hours once it is long.
fn rendered(p: &Placeholder, value: &impl Fn(&SignalStat) -> Option<Value>) -> Option<String> {
    let first = value(&p.stat)?;
    let Some(per) = &p.per else {
        return Some(first.text);
    };
    let (numerator, unit) = first.number?;
    let (denominator, per_unit) = value(per)?.number?;
    let unit = unit.per(per_unit)?;
    if denominator <= 0.0 {
        return None;
    }
    let quotient = numerator / denominator;
    quotient.is_finite().then(|| match unit {
        Unit::Seconds => format_duration(quotient),
        unit => unit.format_value(quotient),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Value {
        Value {
            text: s.to_owned(),
            number: None,
        }
    }

    #[test]
    fn finds_and_renders_placeholders() {
        let t = "free slots never fell below {workflow_slots_available.min} while latency reached {wft_schedule_to_start_latency.max}";
        let found = placeholders(t).expect("placeholders");
        assert_eq!(found.len(), 2);
        let out = render(t, |k| match k.signal.as_str() {
            "workflow_slots_available" => Some(text("142")),
            _ => None,
        });
        assert_eq!(
            out,
            "free slots never fell below 142 while latency reached unknown"
        );
    }

    #[test]
    fn divides_a_count_by_a_rate_into_a_duration() {
        let t = "The {backlog_count.last} waiting take at least {backlog_count.last/tasks_dispatch_rate.last} to clear[, or {backlog_count.last/tasks_add_rate.last} at the add rate].";
        let found = placeholders(t).expect("placeholders");
        assert_eq!(found.len(), 3);
        assert_eq!(
            found[1].stats().map(|s| s.to_string()).collect::<Vec<_>>(),
            ["backlog_count.last", "tasks_dispatch_rate.last"]
        );
        let out = render(t, |k| match k.signal.as_str() {
            "backlog_count" => Some(Value {
                text: "25,171".to_owned(),
                number: Some((25_171.0, Unit::Count)),
            }),
            "tasks_dispatch_rate" => Some(Value {
                text: "4.98/s".to_owned(),
                number: Some((4.98, Unit::PerSecond)),
            }),
            // Nothing is arriving: a quotient by zero has no value.
            _ => Some(Value {
                text: "0/s".to_owned(),
                number: Some((0.0, Unit::PerSecond)),
            }),
        });
        assert_eq!(out, "The 25,171 waiting take at least 84 min to clear.");
    }

    #[test]
    fn an_optional_segment_shows_only_with_its_values() {
        let t = "Writes took up to {write_latency.max}[, against about {write_latency.baseline} before the incident].";
        fn value(baseline: Option<&'static str>) -> impl Fn(&SignalStat) -> Option<Value> {
            move |k| match k.stat {
                Statistic::Max => Some(text("4.2 s")),
                _ => baseline.map(text),
            }
        }
        assert_eq!(
            render(t, value(Some("80 ms"))),
            "Writes took up to 4.2 s, against about 80 ms before the incident."
        );
        assert_eq!(render(t, value(None)), "Writes took up to 4.2 s.");
        assert_eq!(
            parts(t).expect("parts"),
            vec![
                Part::Text("Writes took up to {write_latency.max}"),
                Part::Optional(", against about {write_latency.baseline} before the incident"),
                Part::Text("."),
            ]
        );
    }

    #[test]
    fn optional_segments_close_and_do_not_nest() {
        assert!(parts("a [b").is_err());
        assert!(parts("a] b").is_err());
        assert!(parts("a [b [c] d]").is_err());
        assert_eq!(parts("").expect("empty"), Vec::new());
    }
}
