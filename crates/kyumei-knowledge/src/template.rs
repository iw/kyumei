//! Statement templates.
//!
//! Rule and conclusion statements are written in the knowledge pack with
//! `{signal.stat}` placeholders, e.g. `{workflow_slots_available.min}`. The
//! engine renders them from the same facts the rule evaluated, so the prose can
//! never quote a number the rule did not see.

use kyumei_domain::{SignalId, Statistic};
use regex::Regex;
use std::sync::LazyLock;

use crate::condition::SignalStat;

static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([a-z0-9_]+)\.([a-z]+)\}").expect("placeholder pattern"));

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

/// Replace placeholders using `render`; unknown values render as `unknown`.
pub fn render(template: &str, render: impl Fn(&SignalStat) -> Option<String>) -> String {
    PLACEHOLDER
        .replace_all(template, |caps: &regex::Captures<'_>| {
            let Ok(stat) = caps[2].parse::<Statistic>() else {
                return caps[0].to_owned();
            };
            let key = SignalStat {
                signal: SignalId::new(&caps[1]),
                stat,
            };
            render(&key).unwrap_or_else(|| "unknown".to_owned())
        })
        .into_owned()
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
}
