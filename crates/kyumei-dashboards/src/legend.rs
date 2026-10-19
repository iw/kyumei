//! Series names and legend statistics, as a panel's legend shows them.

use std::{collections::BTreeMap, sync::LazyLock};

use regex::Regex;

use crate::grafana::LegendCalc;

/// `$name`, `${name}`, `${name:format}` and `[[name]]`.
static VARIABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{(\w+)(?::[^}]*)?\}|\[\[(\w+)(?::[^\]]*)?\]\]|\$(\w+)")
        .unwrap_or_else(|e| panic!("{e}"))
});

/// `{{label}}` and `{{ label }}`.
static LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{\s*([^}\s]+)\s*\}\}").unwrap_or_else(|e| panic!("{e}")));

/// Replace template variables Kyumei knows, such as `$namespace` or the quantile
/// `$p`, leaving the rest as they are.
pub fn fill_variables(text: &str, variables: &BTreeMap<String, String>) -> String {
    VARIABLE
        .replace_all(text, |c: &regex::Captures<'_>| {
            let name = c
                .get(1)
                .or_else(|| c.get(2))
                .or_else(|| c.get(3))
                .map_or("", |m| m.as_str());
            variables
                .get(name)
                .cloned()
                .unwrap_or_else(|| c[0].to_owned())
        })
        .into_owned()
}

/// A series' name as the panel's legend writes it. `{{label}}` takes the label's value;
/// a label the series lacks is looked up under `equivalent`, a label known to carry the
/// same value. Without a legend format, or when none of its labels can be found, the
/// series is named by its labels.
pub fn series_name(
    format: Option<&str>,
    metric: Option<&str>,
    labels: &BTreeMap<String, String>,
    variables: &BTreeMap<String, String>,
    equivalent: impl Fn(&str) -> Option<String>,
) -> String {
    let fallback = || {
        let pairs: Vec<String> = labels.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect();
        match (metric, pairs.is_empty()) {
            (Some(m), true) => m.to_owned(),
            (Some(m), false) => format!("{m}{{{}}}", pairs.join(", ")),
            (None, false) => format!("{{{}}}", pairs.join(", ")),
            (None, true) => "value".to_owned(),
        }
    };
    let Some(format) = format.filter(|f| !f.trim().is_empty()) else {
        return fallback();
    };
    let format = fill_variables(format, variables);
    let mut placeholders = 0;
    let mut found = 0;
    let name = LABEL
        .replace_all(&format, |c: &regex::Captures<'_>| {
            placeholders += 1;
            let label = &c[1];
            match labels
                .get(label)
                .cloned()
                .or_else(|| equivalent(label).and_then(|e| labels.get(&e).cloned()))
            {
                Some(v) => {
                    found += 1;
                    v
                }
                None => String::new(),
            }
        })
        .trim()
        .to_owned();
    if name.is_empty() || (placeholders > 0 && found == 0) {
        fallback()
    } else {
        name
    }
}

/// One legend statistic over a series' values; `None` when it has no value or Kyumei
/// does not compute it. NaN values are skipped, as Grafana skips nulls.
pub fn statistic(calc: &LegendCalc, values: &[f64]) -> Option<f64> {
    let real: Vec<f64> = values.iter().copied().filter(|v| !v.is_nan()).collect();
    let sum = || real.iter().sum::<f64>();
    match calc {
        LegendCalc::Min => real.iter().copied().reduce(f64::min),
        LegendCalc::Max => real.iter().copied().reduce(f64::max),
        LegendCalc::Mean => (!real.is_empty()).then(|| sum() / real.len() as f64),
        LegendCalc::Last => values.last().copied(),
        LegendCalc::LastNotNull => real.last().copied(),
        LegendCalc::First => values.first().copied(),
        LegendCalc::FirstNotNull => real.first().copied(),
        LegendCalc::Sum => (!real.is_empty()).then(sum),
        LegendCalc::Count => Some(real.len() as f64),
        LegendCalc::Range => {
            let min = real.iter().copied().reduce(f64::min)?;
            let max = real.iter().copied().reduce(f64::max)?;
            Some(max - min)
        }
        LegendCalc::Other(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn names_follow_the_legend_format_and_its_equivalents() {
        let vars = labels(&[("p", "0.99"), ("namespace", "orders")]);
        let series = labels(&[("task_type", "VisibilityTaskCloseExecution")]);
        let none = |_: &str| None;
        let alias = |l: &str| (l == "operation").then(|| "task_type".to_owned());
        assert_eq!(
            series_name(Some("{{operation}}"), None, &series, &vars, alias),
            "VisibilityTaskCloseExecution"
        );
        assert_eq!(
            series_name(Some("{{ operation }}"), None, &series, &vars, none),
            "{task_type=\"VisibilityTaskCloseExecution\"}",
            "a format none of whose labels the series has falls back to its labels"
        );
        assert_eq!(
            series_name(Some("p$p {{task_type}}"), None, &series, &vars, none),
            "p0.99 VisibilityTaskCloseExecution"
        );
        assert_eq!(
            series_name(Some("total rps"), None, &series, &vars, none),
            "total rps"
        );
        assert_eq!(
            series_name(
                None,
                Some("up"),
                &labels(&[("job", "temporal")]),
                &vars,
                none
            ),
            "up{job=\"temporal\"}"
        );
        assert_eq!(
            fill_variables("ns=${namespace:raw} [[p]] $missing", &vars),
            "ns=orders 0.99 $missing"
        );
    }

    #[test]
    fn statistics_skip_nan() {
        let v = [f64::NAN, 2.0, 5.0, 1.0, f64::NAN];
        assert_eq!(statistic(&LegendCalc::Min, &v), Some(1.0));
        assert_eq!(statistic(&LegendCalc::Max, &v), Some(5.0));
        assert_eq!(statistic(&LegendCalc::Mean, &v), Some(8.0 / 3.0));
        assert!(statistic(&LegendCalc::Last, &v).is_some_and(f64::is_nan));
        assert_eq!(statistic(&LegendCalc::LastNotNull, &v), Some(1.0));
        assert_eq!(statistic(&LegendCalc::Range, &v), Some(4.0));
        assert_eq!(statistic(&LegendCalc::Mean, &[f64::NAN]), None);
        assert_eq!(statistic(&LegendCalc::Other("variance".into()), &v), None);
    }
}
