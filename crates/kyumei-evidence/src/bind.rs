//! Relating evidence to diagnostic signals.
//!
//! Binding is conservative. A series binds to a signal when its metric name
//! matches a knowledge claim *and* its labels satisfy the signal's label
//! constraints, when the operator declared the signal, or when it answers a
//! request that expects exactly one signal. Every assumption is recorded as a
//! caution note the operator can correct.

use std::collections::BTreeMap;

use kyumei_domain::{
    BindingBasis, ClaimId, NoteSeverity, SignalBinding, SignalId, Unit, label_value_matches,
};
use kyumei_knowledge::{Claim, SignalDef, Transform};
use regex::Regex;
use std::sync::LazyLock;

use crate::IngestContext;

/// The outcome of resolving one observation.
#[derive(Clone, Debug, Default)]
pub struct Resolution {
    pub binding: Option<SignalBinding>,
    pub unit: Option<Unit>,
    pub transform: Option<Transform>,
    pub notes: Vec<(NoteSeverity, String)>,
}

impl Resolution {
    fn note(&mut self, severity: NoteSeverity, text: impl Into<String>) {
        self.notes.push((severity, text.into()));
    }
}

static IDENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[a-zA-Z_:][a-zA-Z0-9_:]*").expect("identifier pattern"));

/// The parts of a PromQL query that never name a metric: string literals,
/// label matchers, and the label lists of `by`, `without`, `on`, `ignoring`
/// and the group modifiers.
static NOT_SELECTORS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#""(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|\{[^}]*\}|\b(?:by|without|on|ignoring|group_left|group_right)\s*\([^)]*\)"#,
    )
    .expect("non-selector pattern")
});

/// A signal candidate for a metric series.
struct Candidate<'a> {
    signal: &'a SignalDef,
    claim: ClaimId,
    assumed_labels: Vec<String>,
    transform: Option<Transform>,
    unit: Option<Unit>,
    unit_condition: String,
    unit_alternatives: Vec<(Option<Unit>, String)>,
    exposed_name: String,
}

pub(crate) fn resolve_metric(
    ctx: &IngestContext<'_>,
    metric: Option<&str>,
    labels: &BTreeMap<String, String>,
) -> Resolution {
    let mut r = Resolution::default();
    let query = ctx.declared.query.as_deref().unwrap_or_default();

    // 1. The operator's declaration wins.
    if let Some(signal) = &ctx.declared.signal {
        let Some(def) = ctx.signal(signal) else {
            r.note(
                NoteSeverity::Problem,
                format!("Declared signal `{signal}` is not part of this diagnostic."),
            );
            return r;
        };
        let exposed = metric
            .map(str::to_owned)
            .or_else(|| metric_in_query(ctx, query));
        let matched = exposed.as_deref().and_then(|name| {
            candidates(ctx, name, labels, query)
                .into_iter()
                .find(|c| c.signal.id == def.id)
        });
        r.binding = Some(SignalBinding {
            signal: def.id.clone(),
            basis: BindingBasis::OperatorDeclared,
            claim: matched.as_ref().map(|c| c.claim.clone()),
        });
        r.transform = matched.as_ref().and_then(|c| c.transform);
        r.unit = resolve_unit(
            ctx,
            &mut r.notes,
            matched.as_ref().and_then(|c| c.unit),
            def,
        );
        return r;
    }

    // 2. The metric name (from the data, else from the query).
    let exposed = metric
        .map(str::to_owned)
        .or_else(|| metric_in_query(ctx, query));
    if let Some(name) = exposed.as_deref() {
        let mut found = candidates(ctx, name, labels, query);
        if found.len() > 1 {
            // Prefer candidates that needed no label assumptions, then the
            // signals the answered request expects.
            let exact: Vec<usize> = found
                .iter()
                .enumerate()
                .filter(|(_, c)| c.assumed_labels.is_empty())
                .map(|(i, _)| i)
                .collect();
            if exact.len() == 1 {
                found = vec![found.swap_remove(exact[0])];
            } else if let Some(request) = ctx.request {
                let expected: Vec<usize> = found
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| request.expected_signals.contains(&c.signal.id))
                    .map(|(i, _)| i)
                    .collect();
                if expected.len() == 1 {
                    found = vec![found.swap_remove(expected[0])];
                }
            }
        }
        match found.len() {
            1 => {
                let c = found.remove(0);
                for label in &c.assumed_labels {
                    let constraint = c
                        .signal
                        .source
                        .iter()
                        .find(|s| s.claim == c.claim)
                        .and_then(|s| s.labels.get(label))
                        .cloned()
                        .unwrap_or_default();
                    r.note(
                        NoteSeverity::Caution,
                        format!(
                            "Series has no `{label}` label; assumed `{label}=\"{constraint}\"` for {}. Correct this if the query was not filtered that way.",
                            c.signal.title
                        ),
                    );
                }
                r.binding = Some(SignalBinding {
                    signal: c.signal.id.clone(),
                    basis: BindingBasis::MetricName {
                        exposed_name: c.exposed_name.clone(),
                    },
                    claim: Some(c.claim.clone()),
                });
                r.transform = c.transform;
                r.unit = resolve_unit(ctx, &mut r.notes, c.unit, c.signal);
                if ctx.declared.unit.is_none()
                    && !c.unit_alternatives.is_empty()
                    && c.signal.unit.is_duration()
                {
                    let alternatives: Vec<String> = c
                        .unit_alternatives
                        .iter()
                        .map(|(u, cond)| {
                            format!(
                                "{} ({cond})",
                                u.map_or("unknown".to_owned(), |u| u.to_string())
                            )
                        })
                        .collect();
                    r.note(
                        NoteSeverity::Caution,
                        format!(
                            "Assumed {} for `{}`: {} Alternatives: {}. Correct the unit if your exporter differs.",
                            c.unit.map_or("the signal's unit".to_owned(), |u| u.to_string()),
                            c.exposed_name,
                            c.unit_condition,
                            alternatives.join("; ")
                        ),
                    );
                }
                return r;
            }
            n if n > 1 => {
                let names: Vec<String> = found.iter().map(|c| c.signal.id.to_string()).collect();
                r.note(
                    NoteSeverity::Caution,
                    format!(
                        "`{name}` could inform {}; state which signal this is.",
                        names.join(" or ")
                    ),
                );
                return r;
            }
            _ => {
                let recognised: Vec<String> = ctx
                    .pack
                    .match_metric(name)
                    .iter()
                    .map(|m| format!("{} `{}`", m.metric.category.label(), m.metric.name))
                    .collect();
                if let Some(first) = recognised.first() {
                    let label_mismatch = ctx.diagnostic.is_some_and(|d| {
                        d.signals.iter().any(|s| {
                            s.source.iter().any(|src| {
                                ctx.pack
                                    .match_metric(name)
                                    .iter()
                                    .any(|m| m.metric.id == src.claim)
                            })
                        })
                    });
                    if label_mismatch {
                        r.note(
                            NoteSeverity::Caution,
                            format!(
                                "Recognised {first}, but its labels do not match any signal this diagnostic uses (check `worker_type`, `poller_type` or `operation`)."
                            ),
                        );
                    } else {
                        r.note(
                            NoteSeverity::Info,
                            format!("Recognised {first}; this diagnostic does not use it."),
                        );
                    }
                    return r;
                }
            }
        }
    }

    // 3. The answered request, when it expects exactly one signal.
    if let Some(request) = ctx.request
        && let [only] = request.expected_signals.as_slice()
        && let Some(def) = ctx.signal(only)
    {
        r.binding = Some(SignalBinding {
            signal: def.id.clone(),
            basis: BindingBasis::RequestExpectation {
                request: request.id.clone(),
            },
            claim: None,
        });
        r.note(
            NoteSeverity::Caution,
            format!(
                "No recognised metric name; treated as {} because it answers {}.",
                def.title, request.id
            ),
        );
        r.unit = resolve_unit(ctx, &mut r.notes, None, def);
        return r;
    }

    r.note(
        NoteSeverity::Caution,
        match exposed {
            Some(name) => format!("`{name}` is not a metric this knowledge pack describes."),
            None => "No metric name or declared signal; kept as unbound data.".to_owned(),
        },
    );
    r
}

/// Signals whose sources match `name` with satisfiable label constraints.
///
/// Expositions for SDK families the investigation does not use are ignored.
/// When several expositions of the same metric fit (for example Core's
/// default milliseconds and its optional seconds), the first listed wins and
/// the alternatives are recorded so the unit can be flagged as assumed.
fn candidates<'a>(
    ctx: &IngestContext<'a>,
    name: &str,
    labels: &BTreeMap<String, String>,
    query: &str,
) -> Vec<Candidate<'a>> {
    let Some(diagnostic) = ctx.diagnostic else {
        return Vec::new();
    };
    let matches: Vec<_> = ctx
        .pack
        .match_metric(name)
        .into_iter()
        .filter(|m| {
            ctx.families.is_empty()
                || m.exposition
                    .sdk_family
                    .is_none_or(|f| ctx.families.contains(&f))
        })
        .collect();
    let mut out: Vec<Candidate<'a>> = Vec::new();
    for m in &matches {
        for signal in &diagnostic.signals {
            for source in signal.source.iter().filter(|s| s.claim == m.metric.id) {
                let mut assumed = Vec::new();
                let mut ok = true;
                for (label, value) in &source.labels {
                    match labels.get(label) {
                        Some(actual) if label_value_matches(actual, value) => {}
                        Some(_) => ok = false,
                        None if query_filters(query, label, value) => {}
                        // The query kept another value of the label, so its
                        // series cannot be this source's.
                        None if query_pins_other(query, label, value) => ok = false,
                        None => assumed.push(label.clone()),
                    }
                }
                if !ok {
                    continue;
                }
                let unit = m.exposition.unit.or(m.metric.unit);
                if let Some(existing) = out.iter_mut().find(|c| c.signal.id == signal.id) {
                    if unit != existing.unit {
                        existing
                            .unit_alternatives
                            .push((unit, m.exposition.condition.clone()));
                    }
                    continue;
                }
                out.push(Candidate {
                    signal,
                    claim: m.metric.id.clone(),
                    assumed_labels: assumed,
                    transform: source.transform,
                    unit,
                    unit_condition: m.exposition.condition.clone(),
                    unit_alternatives: Vec::new(),
                    exposed_name: m.exposition.name.clone(),
                });
            }
        }
    }
    out
}

/// Whether the query text filters `label` to `value`.
pub(crate) fn query_filters(query: &str, label: &str, value: &str) -> bool {
    let compact: String = query.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains(&format!("{label}=\"{value}\""))
        || compact.contains(&format!("{label}='{value}'"))
}

/// Whether the query has an equality matcher `label="other"` for a value other
/// than `value`, so that every series it returns has that other value.
pub(crate) fn query_pins_other(query: &str, label: &str, value: &str) -> bool {
    let compact: String = query.chars().filter(|c| !c.is_whitespace()).collect();
    ['"', '\''].iter().any(|quote| {
        let pattern = format!("{label}={quote}");
        compact.match_indices(&pattern).any(|(at, _)| {
            let delimited = compact[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c == '{' || c == ',');
            let rest = &compact[at + pattern.len()..];
            delimited && rest.split(*quote).next().is_some_and(|v| v != value)
        })
    })
}

/// Whether any label matcher in the query names `label` (`=`, `!=`, `=~`, `!~`).
pub(crate) fn query_matches_label(query: &str, label: &str) -> bool {
    let compact: String = query.chars().filter(|c| !c.is_whitespace()).collect();
    ["=", "!=", "!~"]
        .iter()
        .any(|op| compact.contains(&format!("{label}{op}")))
}

/// The first metric name in `query` that a knowledge claim recognises.
///
/// Label names, label values, grouping lists and function names are skipped:
/// in `sum by (operation) (rate(service_requests[1m]))` the metric is
/// `service_requests`, although the server also has a metric named `operation`.
fn metric_in_query(ctx: &IngestContext<'_>, query: &str) -> Option<String> {
    let selectors = NOT_SELECTORS.replace_all(query, " ");
    IDENT
        .find_iter(&selectors)
        .filter(|m| !selectors[m.end()..].trim_start().starts_with('('))
        .map(|m| m.as_str())
        .find(|ident| !ctx.pack.match_metric(ident).is_empty())
        .map(str::to_owned)
}

fn resolve_unit(
    ctx: &IngestContext<'_>,
    notes: &mut Vec<(NoteSeverity, String)>,
    exposed: Option<Unit>,
    signal: &SignalDef,
) -> Option<Unit> {
    if let Some(unit) = ctx.declared.unit {
        return Some(unit);
    }
    if let Some(unit) = exposed {
        return Some(unit);
    }
    let from_request = ctx.request.and_then(|r| {
        let units: Vec<Unit> = r.collect.iter().filter_map(|c| c.unit).collect();
        match units.as_slice() {
            [first, rest @ ..] if rest.iter().all(|u| u == first) => Some(*first),
            _ => None,
        }
    });
    if let Some(unit) = from_request {
        return Some(unit);
    }
    if signal.unit.is_duration() {
        notes.push((
            NoteSeverity::Caution,
            format!(
                "Unit not stated; assumed {} for {}. SDK exporters differ (milliseconds vs seconds) — correct this if needed.",
                signal.unit, signal.title
            ),
        ));
    }
    Some(signal.unit)
}

/// Signals fed by the API field at `path` (a dotted JSON path whose last
/// segment is `key`). Fields with a declared `json_path` match only that path.
pub(crate) fn resolve_field<'a>(
    ctx: &IngestContext<'a>,
    path: &str,
    key: &str,
) -> Vec<(&'a SignalDef, ClaimId, Option<Unit>)> {
    let Some(diagnostic) = ctx.diagnostic else {
        return Vec::new();
    };
    let clean_path: String = strip_indexes(path);
    let norm_key = crate::fields::normalise(key);
    let mut out = Vec::new();
    for signal in &diagnostic.signals {
        for source in &signal.source {
            let Some(Claim::ApiField(field)) = ctx.pack.claim(&source.claim) else {
                continue;
            };
            let matched = match &field.json_path {
                Some(p) => clean_path == *p || clean_path.ends_with(&format!(".{p}")),
                None => {
                    crate::fields::normalise(&field.json_name) == norm_key
                        || crate::fields::normalise(&field.field) == norm_key
                }
            };
            if matched {
                out.push((signal, source.claim.clone(), field.unit));
            }
        }
    }
    out
}

pub(crate) fn strip_indexes(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut depth = 0;
    for ch in path.chars() {
        match ch {
            '[' => depth += 1,
            ']' => depth -= 1,
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Signals fed by a dynamic configuration key.
pub(crate) fn resolve_config<'a>(
    ctx: &IngestContext<'a>,
    key: &str,
) -> Vec<(&'a SignalDef, ClaimId)> {
    let Some(diagnostic) = ctx.diagnostic else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for signal in &diagnostic.signals {
        for source in &signal.source {
            if let Some(Claim::DynamicConfig(dc)) = ctx.pack.claim(&source.claim)
                && dc.key == key
            {
                out.push((signal, source.claim.clone()));
            }
        }
    }
    out
}

/// Whether `id` names a signal of the active diagnostic.
impl IngestContext<'_> {
    pub(crate) fn signal(&self, id: &SignalId) -> Option<&SignalDef> {
        self.diagnostic.and_then(|d| d.signal(id))
    }
}
