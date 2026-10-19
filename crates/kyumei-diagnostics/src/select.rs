//! Choosing and rendering the next evidence request.
//!
//! Selection is deterministic and explainable. A request is a candidate when
//! it can still move an open hypothesis and has not already been answered,
//! declined or satisfied. Candidates are ordered by:
//!
//! 1. whether they target a hypothesis evidence has not yet weakened,
//! 2. the diagnostic path's stage order (serial investigation),
//! 3. collection cost,
//! 4. the number of open hypotheses they discriminate,
//! 5. pack order.

use jiff::{SignedDuration, Timestamp};
use kyumei_domain::{
    AbsentReadsZero, CollectionInstruction, CollectionMethod, EvidenceRequest, EvidenceRequestId,
    HypothesisId, HypothesisState, Investigation, RequestDefinitionId, RequestStatus,
    RequestedWindow, SdkFamily, StageId, tally_spelling,
};
use kyumei_knowledge::{DiagnosticDef, RequestDef};
use serde::{Deserialize, Serialize};

use crate::{evaluate::HypothesisOutcome, facts::FactTable};

/// A candidate evidence request with the reasons for its ranking.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestPlan {
    pub definition: RequestDefinitionId,
    pub stage: StageId,
    pub targets: Vec<HypothesisId>,
    pub cost: u8,
    /// Whether every target has already been weakened by evidence.
    pub deferred: bool,
    pub rationale: String,
}

pub fn plan(
    d: &DiagnosticDef,
    outcomes: &std::collections::BTreeMap<HypothesisId, HypothesisOutcome>,
    facts: &FactTable,
    investigation: &Investigation,
) -> Vec<RequestPlan> {
    let mut candidates: Vec<(usize, RequestPlan, u32)> = Vec::new();
    for (index, def) in d.requests.iter().enumerate() {
        let history: Vec<&EvidenceRequest> = investigation
            .requests
            .iter()
            .filter(|r| r.definition == def.id)
            .collect();
        let declined = history
            .iter()
            .any(|r| matches!(r.status, RequestStatus::Declined { .. }));
        // A request with a response counts as answered once its signals are
        // informed, as the follow-up to this assessment records; otherwise a
        // request answered only by values the operator typed is issued again.
        let answered = history
            .iter()
            .any(|r| matches!(r.status, RequestStatus::Answered { .. }) || !r.responses.is_empty());
        let observed = def.signals.iter().all(|s| {
            facts
                .get(s)
                .is_some_and(|f| f.status == kyumei_domain::EpistemicStatus::ObservedFact)
        });
        // A signal whose queries found no series counts too: asking again
        // would find none again.
        let informed = def
            .signals
            .iter()
            .all(|s| facts.get(s).is_some() || facts.is_absent(s));
        // An answered request whose evidence no longer informs its signals
        // (after a correction, say) can be asked again.
        if declined || observed || (answered && informed) {
            continue;
        }
        let targets: Vec<&HypothesisOutcome> = def
            .distinguishes
            .iter()
            .filter_map(|h| outcomes.get(h))
            .filter(|o| !o.state.is_resolved() || o.contested)
            .collect();
        if targets.is_empty() {
            continue;
        }
        let deferred = targets
            .iter()
            .all(|o| o.state == HypothesisState::Weakened && !o.contested);
        let stage_order = d.stage_order(&def.stage);
        let rationale = rationale(d, def, &targets, deferred);
        candidates.push((
            index,
            RequestPlan {
                definition: def.id.clone(),
                stage: def.stage.clone(),
                targets: targets.iter().map(|o| o.id.clone()).collect(),
                cost: def.cost,
                deferred,
                rationale,
            },
            stage_order,
        ));
    }
    candidates.sort_by_key(|(index, plan, stage)| {
        (
            plan.deferred,
            *stage,
            plan.cost,
            std::cmp::Reverse(plan.targets.len()),
            *index,
        )
    });
    candidates.into_iter().map(|(_, p, _)| p).collect()
}

fn rationale(
    d: &DiagnosticDef,
    def: &RequestDef,
    targets: &[&HypothesisOutcome],
    deferred: bool,
) -> String {
    let stage = d
        .stage(&def.stage)
        .map_or(def.stage.to_string(), |s| s.title.clone());
    let titles: Vec<String> = targets
        .iter()
        .filter_map(|o| d.hypothesis(&o.id))
        .map(|h| h.title.clone())
        .collect();
    let effort = match def.cost {
        1 => "a single query",
        2 => "one or two queries",
        3 => "a command or configuration lookup",
        _ => "some effort to collect",
    };
    let lead = if deferred {
        "Remaining check"
    } else {
        "Next open question on the diagnostic path"
    };
    format!(
        "{lead} ({stage}). It discriminates: {}. Cost: {effort}.",
        titles.join("; ")
    )
}

/// Render a request definition for this investigation.
pub fn render_request(
    def: &RequestDef,
    d: &DiagnosticDef,
    investigation: &Investigation,
    id: EvidenceRequestId,
    issued_at: Timestamp,
) -> EvidenceRequest {
    let window = requested_window(def, investigation);
    // An SDK whose implementation is unknown could be any family, so it shows every variant.
    let families: Vec<SdkFamily> = investigation
        .environment
        .sdk_families()
        .into_iter()
        .filter(|f| *f != SdkFamily::Unknown)
        .collect();
    let namespace = investigation.scope.primary_namespace();
    let task_queue = investigation.scope.primary_task_queue();
    let step = window
        .window
        .map_or("30s".to_owned(), |w| step_for(w.duration()));

    let collect = def
        .collect
        .iter()
        .filter(|c| {
            c.sdk_family.is_empty()
                || families.is_empty()
                || c.sdk_family.iter().any(|f| families.contains(f))
        })
        .map(|c| {
            let mut unfilled = Vec::new();
            let mut fill = |key: &str, value: Option<String>, text: &mut String, query: bool| {
                let placeholder = format!("{{{key}}}");
                if text.contains(&placeholder) {
                    match value {
                        Some(v) if query => *text = fill_body(c.method, text, &placeholder, &v),
                        Some(v) => *text = text.replace(&placeholder, &v),
                        None => {
                            *text = text.replace(&placeholder, &format!("<{key}>"));
                            unfilled.push(key.to_owned());
                        }
                    }
                }
            };
            let mut body = c.body.clone();
            let mut witness = c.absent_reads_zero.as_ref().map(|z| z.witness.clone());
            let mut label = c.label.clone();
            if let Some(note) = &c.note {
                label = format!("{label} — {note}");
            }
            let mut claims = c.claims.clone();
            // Tally writes characters other than letters, digits and `_` in
            // label values as `_`: the server's default framework does, and so
            // do Go Workers using tally. Core and Java Workers do not.
            let through_tally = c.method == CollectionMethod::Promql
                && (c.sdk_family.is_empty() || c.sdk_family.contains(&SdkFamily::Go));
            if through_tally {
                let mut respelled = Vec::new();
                for (key, value) in [("namespace", namespace), ("task_queue", task_queue)] {
                    if let Some(value) = value
                        && let Some(tally) = match_both_spellings(&mut body, key, value)
                    {
                        respelled.push(format!("`{value}` also as `{tally}`"));
                    }
                    // The witness selects the same series, so it is spelled the same way.
                    if let (Some(value), Some(w)) = (value, witness.as_mut()) {
                        match_both_spellings(w, key, value);
                    }
                }
                if !respelled.is_empty() {
                    label = format!(
                        "{label} — matches {}, the spelling tally writes",
                        respelled.join(" and ")
                    );
                    let cites =
                        |prefix: &str| c.claims.iter().any(|x| x.as_str().starts_with(prefix));
                    if cites("server.") {
                        claims.push("server.metrics-label-values".into());
                    }
                    if cites("sdk.") {
                        claims.push("sdk.go-tally-label-values".into());
                    }
                }
            }
            // The body is filled for its method, and so is the witness, which
            // only a PromQL query has. The label is prose.
            let mut texts = vec![(&mut body, true), (&mut label, false)];
            if let Some(w) = witness.as_mut() {
                texts.push((w, true));
            }
            for (text, query) in texts {
                fill("namespace", namespace.map(str::to_owned), text, query);
                fill("task_queue", task_queue.map(str::to_owned), text, query);
                fill(
                    "start",
                    window.window.map(|w| w.start.to_string()),
                    text,
                    query,
                );
                fill("end", window.window.map(|w| w.end.to_string()), text, query);
                fill("step", Some(step.clone()), text, query);
            }
            unfilled.sort();
            unfilled.dedup();
            // Say which SDKs a variant is for when the operator could be looking at another:
            // the SDK is unknown, or the Workers use several.
            let shown: Vec<&str> = c
                .sdk_family
                .iter()
                .filter(|f| families.is_empty() || families.contains(f))
                .map(|f| f.short_label())
                .collect();
            let applies_to = (!shown.is_empty() && (families.is_empty() || families.len() > 1))
                .then(|| shown.join(", "));
            let absent_reads_zero =
                c.absent_reads_zero
                    .as_ref()
                    .zip(witness)
                    .map(|(z, witness)| AbsentReadsZero {
                        witness,
                        because: z.because.clone(),
                    });
            CollectionInstruction {
                method: c.method,
                label,
                body,
                applies_to,
                unit: c.unit,
                claims,
                unfilled,
                absent_reads_zero,
            }
        })
        .collect();

    let why = def
        .why
        .replace("{task_queue}", task_queue.unwrap_or("the Task Queue"))
        .replace("{namespace}", namespace.unwrap_or("the namespace"));
    EvidenceRequest {
        id,
        definition: def.id.clone(),
        issued_at,
        title: def.title.clone(),
        why,
        collect,
        window,
        group_by: def.group_by.clone(),
        distinguishes: def
            .distinguishes
            .iter()
            .filter(|h| d.hypothesis(h).is_some())
            .cloned()
            .collect(),
        expected_signals: def.signals.clone(),
        responses: Vec::new(),
        status: RequestStatus::Open,
    }
}

/// Rewrite the label matchers that compare against `{key}` in a PromQL query
/// so they match `value` as written and as tally spells it, when the two
/// differ. Returns tally's spelling if anything was rewritten; other
/// occurrences of the placeholder are left for the plain fill.
fn match_both_spellings(query: &mut String, key: &str, value: &str) -> Option<String> {
    let tally = tally_spelling(value);
    let placeholder = format!("{{{key}}}");
    if tally == value || !query.contains(&placeholder) {
        return None;
    }
    let either = format!("{}|{tally}", string_escaped(&regex_escaped(value)));
    let before = query.clone();
    *query = query
        .replace(&format!("!=\"{placeholder}\""), &format!("!~\"{either}\""))
        .replace(&format!("=\"{placeholder}\""), &format!("=~\"{either}\""))
        .replace(&format!("=~\"{placeholder}"), &format!("=~\"(?:{either})"))
        .replace(&format!("!~\"{placeholder}"), &format!("!~\"(?:{either})"));
    (*query != before).then_some(tally)
}

/// Put `value` where `placeholder` stands in an instruction's query or command,
/// escaped for what its method writes: a PromQL query or a shell command. A
/// Temporal API call's values stand in the JSON request it sends, so they are
/// escaped as JSON strings before they are quoted for the shell. Other bodies
/// take a value as it is.
fn fill_body(method: CollectionMethod, body: &str, placeholder: &str, value: &str) -> String {
    match method {
        CollectionMethod::Promql => fill_promql(body, placeholder, value),
        CollectionMethod::TemporalCli => fill_shell(body, placeholder, value),
        CollectionMethod::TemporalApi => fill_shell(body, placeholder, &json_escaped(value)),
        CollectionMethod::DynamicConfig
        | CollectionMethod::Logs
        | CollectionMethod::OperatorCheck => body.replace(placeholder, value),
    }
}

/// Put `value` where `placeholder` stands in a PromQL query, escaped for its
/// place: inside a regex matcher's string as a regex literal, inside any other
/// string as a string literal, and elsewhere, such as a range `[{step}]`, as it
/// is. A value can then never end the string it stands in.
fn fill_promql(query: &str, placeholder: &str, value: &str) -> String {
    let mut out = String::with_capacity(query.len());
    let mut rest = query;
    // Inside a double-quoted string, and whether it is a regex matcher's.
    let mut string: Option<bool> = None;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with(placeholder) {
            out.push_str(&match string {
                Some(true) => string_escaped(&regex_escaped(value)),
                Some(false) => string_escaped(value),
                None => value.to_owned(),
            });
            rest = &rest[placeholder.len()..];
            continue;
        }
        let mut len = c.len_utf8();
        match (string, c) {
            // An escape sequence: keep it whole.
            (Some(_), '\\') => len += rest[len..].chars().next().map_or(0, char::len_utf8),
            (Some(_), '"') => string = None,
            (None, '"') => {
                let before = out.trim_end();
                string = Some(before.ends_with("=~") || before.ends_with("!~"));
            }
            _ => {}
        }
        out.push_str(&rest[..len]);
        rest = &rest[len..];
    }
    out
}

/// A value as a regex literal: RE2's metacharacters escaped, and nothing else,
/// since RE2 refuses an escaped letter it has no meaning for.
fn regex_escaped(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            "\\.+*?()|[]{}^$"
                .contains(c)
                .then_some('\\')
                .into_iter()
                .chain([c])
        })
        .collect()
}

/// A value inside a double-quoted PromQL string.
fn string_escaped(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Put `value` where `placeholder` stands in a shell command, quoted for its
/// place as bash and zsh read it: inside single quotes, with each `'` closed,
/// escaped and reopened; inside double quotes, as it is unless it holds a
/// character they end on or expand, and then single-quoted between them; and
/// elsewhere, as it is when it is a plain word and single-quoted when not. A
/// plain name therefore reads as written, and a value can never end the word or
/// string it stands in.
fn fill_shell(command: &str, placeholder: &str, value: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    // The quote the command is inside, if any.
    let mut quote: Option<char> = None;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with(placeholder) {
            out.push_str(&match quote {
                Some('\'') => value.replace('\'', r"'\''"),
                Some(_) if value.contains(['"', '\\', '$', '`', '!']) => {
                    format!("\"{}\"", single_quoted(value))
                }
                None if !plain_word(value) => single_quoted(value),
                _ => value.to_owned(),
            });
            rest = &rest[placeholder.len()..];
            continue;
        }
        let mut len = c.len_utf8();
        match (quote, c) {
            // Inside single quotes only the quote that ends them is special.
            (Some('\''), '\'') => quote = None,
            (Some('\''), _) => {}
            // An escaped character: keep it whole.
            (_, '\\') => len += rest[len..].chars().next().map_or(0, char::len_utf8),
            (Some(_), '"') => quote = None,
            (None, '\'' | '"') => quote = Some(c),
            _ => {}
        }
        out.push_str(&rest[..len]);
        rest = &rest[len..];
    }
    out
}

/// Whether a shell reads `value`, unquoted, as one word that is just `value`:
/// letters, digits and `-_.,:/@%+` never split, quote or expand. `=` and `~`
/// are left out, since at a word's start zsh reads `=name` as a command's path
/// and both shells read `~` as a home directory.
fn plain_word(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || "-_.,:/@%+".contains(c))
}

/// A value as one single-quoted shell word, with each `'` closed, escaped and
/// reopened.
fn single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// A value inside a JSON string.
fn json_escaped(value: &str) -> String {
    let json = serde_json::Value::from(value).to_string();
    // Without the quotes around it.
    json[1..json.len() - 1].to_owned()
}

fn requested_window(def: &RequestDef, investigation: &Investigation) -> RequestedWindow {
    match investigation.window.as_window() {
        Some(w) => {
            let padded = w.padded(
                SignedDuration::from_mins(i64::from(def.window.pad_before_minutes)),
                SignedDuration::from_mins(i64::from(def.window.pad_after_minutes)),
            );
            let (before, after) = (def.window.pad_before_minutes, def.window.pad_after_minutes);
            let text = if before == 0 && after == 0 {
                format!("{} (the incident window)", w.display_utc())
            } else {
                format!(
                    "{} (the incident window plus {before} min before and {after} min after)",
                    padded.display_utc()
                )
            };
            RequestedWindow {
                window: Some(padded),
                text: match &def.window.guidance {
                    Some(g) => format!("{text}. {g}"),
                    None => text,
                },
            }
        }
        None => RequestedWindow {
            window: None,
            text: def.window.guidance.clone().unwrap_or_else(|| {
                "The incident window is unknown: cover the period when the symptom was present, plus at least 10 minutes before it began.".to_owned()
            }),
        },
    }
}

/// A PromQL range step that keeps a range query to a few hundred points.
/// The step for range queries over a window: 15 seconds up to an hour, a
/// minute up to six hours, five minutes beyond.
pub fn range_step(duration: SignedDuration) -> SignedDuration {
    match duration.as_secs() / 60 {
        0..=60 => SignedDuration::from_secs(15),
        61..=360 => SignedDuration::from_mins(1),
        _ => SignedDuration::from_mins(5),
    }
}

fn step_for(duration: SignedDuration) -> String {
    let step = range_step(duration);
    match step.as_secs() {
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

#[cfg(test)]
mod tests {
    use kyumei_domain::CollectionMethod::{self, TemporalApi, TemporalCli};

    use super::{fill_body, fill_promql, fill_shell, match_both_spellings};

    /// The Activity Task Queue requests' commands, as the packs write them.
    const CLI: &str = "temporal task-queue describe --namespace {namespace} --task-queue {task_queue} --task-queue-type activity -o json";
    const GRPCURL: &str = r#"grpcurl -plaintext -emit-defaults -d '{"namespace": "{namespace}", "taskQueue": {"name": "{task_queue}"}, "taskQueueType": "TASK_QUEUE_TYPE_ACTIVITY", "reportStats": true}' <frontend-host:7233> temporal.api.workflowservice.v1.WorkflowService/DescribeTaskQueue"#;

    fn filled(method: CollectionMethod, body: &str, namespace: &str, task_queue: &str) -> String {
        let body = fill_body(method, body, "{namespace}", namespace);
        fill_body(method, &body, "{task_queue}", task_queue)
    }

    fn rewrite(query: &str, key: &str, value: &str) -> (String, Option<String>) {
        let mut q = query.to_owned();
        let tally = match_both_spellings(&mut q, key, value);
        (q, tally)
    }

    #[test]
    fn a_value_tally_respells_is_matched_both_ways() {
        let (q, tally) = rewrite(
            r#"sum(rate(service_requests{namespace="{namespace}"}[1m]))"#,
            "namespace",
            "orders-prod",
        );
        assert_eq!(
            q,
            r#"sum(rate(service_requests{namespace=~"orders-prod|orders_prod"}[1m]))"#
        );
        assert_eq!(tally.as_deref(), Some("orders_prod"));
    }

    #[test]
    fn a_value_tally_keeps_is_left_alone() {
        let query = r#"sum(rate(service_requests{namespace="{namespace}"}[1m]))"#;
        assert_eq!(
            rewrite(query, "namespace", "orders_prod"),
            (query.to_owned(), None)
        );
    }

    #[test]
    fn values_never_end_the_string_they_stand_in() {
        let (q, _) = rewrite(r#"x{task_queue="{task_queue}"}"#, "task_queue", r#"a"b\c"#);
        assert_eq!(q, r#"x{task_queue=~"a\"b\\\\c|a_b_c"}"#);
        let plain = fill_promql(
            r#"x{task_queue="{task_queue}", namespace=~"{task_queue}.*"}"#,
            "{task_queue}",
            r#"a"b\c.d"#,
        );
        assert_eq!(
            plain,
            r#"x{task_queue="a\"b\\c.d", namespace=~"a\"b\\\\c\\.d.*"}"#
        );
        // Outside strings a value stands as it is.
        assert_eq!(
            fill_promql("rate(x[{step}])", "{step}", "15s"),
            "rate(x[15s])"
        );
        // Escaped quotes inside a string do not end it.
        assert_eq!(
            fill_promql(r#"x{a="q\"{p}", b=~"{p}"}"#, "{p}", "v.w"),
            r#"x{a="q\"v.w", b=~"v\\.w"}"#
        );
    }

    #[test]
    fn letters_beyond_ascii_are_not_escaped() {
        let (q, tally) = rewrite(r#"x{namespace="{namespace}"}"#, "namespace", "café-q");
        assert_eq!(q, r#"x{namespace=~"café-q|caf__q"}"#);
        assert_eq!(tally.as_deref(), Some("caf__q"));
    }

    #[test]
    fn regex_matchers_and_negations_keep_their_meaning() {
        let (q, _) = rewrite(
            r#"x{task_queue=~"{task_queue}(:sticky)?", namespace!="{task_queue}"}"#,
            "task_queue",
            "acme.orders",
        );
        assert_eq!(
            q,
            r#"x{task_queue=~"(?:acme\\.orders|acme_orders)(:sticky)?", namespace!~"acme\\.orders|acme_orders"}"#
        );
    }

    #[test]
    fn commands_show_plain_names_as_written() {
        assert_eq!(
            filled(TemporalCli, CLI, "orders-prod", "acme.orders/eu:v2"),
            "temporal task-queue describe --namespace orders-prod --task-queue acme.orders/eu:v2 --task-queue-type activity -o json"
        );
        assert_eq!(
            filled(TemporalApi, GRPCURL, "orders-prod", "café-q"),
            r#"grpcurl -plaintext -emit-defaults -d '{"namespace": "orders-prod", "taskQueue": {"name": "café-q"}, "taskQueueType": "TASK_QUEUE_TYPE_ACTIVITY", "reportStats": true}' <frontend-host:7233> temporal.api.workflowservice.v1.WorkflowService/DescribeTaskQueue"#
        );
    }

    #[test]
    fn a_cli_command_quotes_names_the_shell_would_split_or_expand() {
        let cli = |task_queue| filled(TemporalCli, CLI, "orders", task_queue);
        let with = |word: &str| {
            format!(
                "temporal task-queue describe --namespace orders --task-queue {word} --task-queue-type activity -o json"
            )
        };
        assert_eq!(cli("order queue"), with("'order queue'"));
        assert_eq!(cli("it's"), with(r"'it'\''s'"));
        assert_eq!(cli(r#"the "fast" one"#), with(r#"'the "fast" one'"#));
        assert_eq!(cli("$HOME"), with("'$HOME'"));
        assert_eq!(cli("$(id)`id`"), with("'$(id)`id`'"));
        assert_eq!(cli(r"a\b"), with(r"'a\b'"));
        assert_eq!(cli("q;rm -rf *"), with("'q;rm -rf *'"));
        // Words zsh or bash would expand at their start, and no word at all.
        assert_eq!(cli("=orders"), with("'=orders'"));
        assert_eq!(cli("~orders"), with("'~orders'"));
        assert_eq!(cli(""), with("''"));
        assert_eq!(
            filled(TemporalCli, CLI, "my ns", "q"),
            "temporal task-queue describe --namespace 'my ns' --task-queue q --task-queue-type activity -o json"
        );
    }

    #[test]
    fn a_temporal_api_call_escapes_names_as_json_then_for_the_single_quotes() {
        let api = |task_queue| filled(TemporalApi, GRPCURL, "orders", task_queue);
        let with = |name: &str| {
            format!(
                r#"grpcurl -plaintext -emit-defaults -d '{{"namespace": "orders", "taskQueue": {{"name": "{name}"}}, "taskQueueType": "TASK_QUEUE_TYPE_ACTIVITY", "reportStats": true}}' <frontend-host:7233> temporal.api.workflowservice.v1.WorkflowService/DescribeTaskQueue"#
            )
        };
        // Inside single quotes a space or `$` is only a character.
        assert_eq!(api("order queue"), with("order queue"));
        assert_eq!(api("$HOME"), with("$HOME"));
        // A double quote or backslash is escaped for JSON, a single quote for
        // the shell: the quotes close, an escaped `'` follows, and they reopen.
        assert_eq!(api(r#"the "fast" one"#), with(r#"the \"fast\" one"#));
        assert_eq!(api(r"a\b"), with(r"a\\b"));
        assert_eq!(api("it's"), with(r"it'\''s"));
        assert_eq!(api("tab\there"), with(r"tab\there"));
        assert_eq!(
            filled(TemporalApi, GRPCURL, r#"o'k "ns""#, "q"),
            r#"grpcurl -plaintext -emit-defaults -d '{"namespace": "o'\''k \"ns\"", "taskQueue": {"name": "q"}, "taskQueueType": "TASK_QUEUE_TYPE_ACTIVITY", "reportStats": true}' <frontend-host:7233> temporal.api.workflowservice.v1.WorkflowService/DescribeTaskQueue"#
        );
    }

    #[test]
    fn shell_values_are_quoted_for_the_quotes_they_stand_in() {
        // Inside double quotes a value is as it is, unless it holds a
        // character they end on or expand: then it stands single-quoted
        // between them.
        assert_eq!(
            fill_shell(r#"echo "q={p}""#, "{p}", "a b"),
            r#"echo "q=a b""#
        );
        assert_eq!(
            fill_shell(r#"echo "q={p}""#, "{p}", "$x!"),
            r#"echo "q="'$x!'"""#
        );
        assert_eq!(
            fill_shell(r#"echo "q={p}""#, "{p}", r#"it's "x""#),
            r#"echo "q="'it'\''s "x"'"""#
        );
        // An escaped quote opens nothing, and a closed string is left behind.
        assert_eq!(
            fill_shell(r#"echo \"'x' {p}"#, "{p}", "a b"),
            r#"echo \"'x' 'a b'"#
        );
        assert_eq!(
            fill_shell(r#"echo "a\"b" '{p}'"#, "{p}", "it's"),
            r#"echo "a\"b" 'it'\''s'"#
        );
    }
}
