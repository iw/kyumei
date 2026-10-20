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

use kyumei_domain::{SignalId, Statistic};
use regex::Regex;
use std::sync::LazyLock;

use crate::condition::SignalStat;

static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([a-z0-9_]+)\.([a-z]+)\}").expect("placeholder pattern"));

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

/// Every `{signal.stat}` placeholder in `template`.
pub fn placeholders(template: &str) -> Result<Vec<SignalStat>, String> {
    PLACEHOLDER
        .captures_iter(template)
        .map(|caps| {
            let stat = caps[2].parse::<Statistic>()?;
            Ok(SignalStat {
                signal: SignalId::new(&caps[1]),
                stat,
            })
        })
        .collect()
}

/// Replace placeholders using `render`. An optional segment with a value
/// missing is left out; elsewhere a missing value renders as `unknown`.
pub fn render(template: &str, render: impl Fn(&SignalStat) -> Option<String>) -> String {
    match parts(template) {
        Ok(parts) => parts
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) => fill(text, &render, Some(UNKNOWN)),
                Part::Optional(text) => fill(text, &render, None),
            })
            .collect(),
        // Validation rejects such brackets, so render the text as written.
        Err(_) => fill(template, &render, Some(UNKNOWN)).unwrap_or_default(),
    }
}

/// `text` with its placeholders replaced. A missing value is written as
/// `missing`, or, without one, leaves nothing to render.
fn fill(
    text: &str,
    render: &impl Fn(&SignalStat) -> Option<String>,
    missing: Option<&str>,
) -> Option<String> {
    let mut complete = true;
    let filled = PLACEHOLDER.replace_all(text, |caps: &regex::Captures<'_>| {
        let Ok(stat) = caps[2].parse::<Statistic>() else {
            return caps[0].to_owned();
        };
        let key = SignalStat {
            signal: SignalId::new(&caps[1]),
            stat,
        };
        render(&key).unwrap_or_else(|| {
            complete = false;
            missing.unwrap_or_default().to_owned()
        })
    });
    (complete || missing.is_some()).then(|| filled.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_renders_placeholders() {
        let t = "free slots never fell below {workflow_slots_available.min} while latency reached {wft_schedule_to_start_latency.max}";
        let found = placeholders(t).expect("placeholders");
        assert_eq!(found.len(), 2);
        let out = render(t, |k| match k.signal.as_str() {
            "workflow_slots_available" => Some("142".to_owned()),
            _ => None,
        });
        assert_eq!(
            out,
            "free slots never fell below 142 while latency reached unknown"
        );
    }

    #[test]
    fn an_optional_segment_shows_only_with_its_values() {
        let t = "Writes took up to {write_latency.max}[, against about {write_latency.baseline} before the incident].";
        fn value(baseline: Option<&'static str>) -> impl Fn(&SignalStat) -> Option<String> {
            move |k| match k.stat {
                Statistic::Max => Some("4.2 s".to_owned()),
                _ => baseline.map(str::to_owned),
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
