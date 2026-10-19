//! API fields in JSON (CLI `-o json`, API responses) and in the
//! CLI's text tables.
//!
//! API fields are never treated as metrics: they bind only through knowledge
//! claims of category "API field".

use serde_json::Value;

/// A leaf value found under a JSON key.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FieldValue {
    /// Dotted path, e.g. `stats.approximateBacklogAge`.
    pub path: String,
    /// The final key.
    pub key: String,
    pub value: FieldScalar,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FieldScalar {
    Number(f64),
    Text(String),
    /// A repeated field; the element count.
    Count(usize),
    /// `null`: an empty list where the field is repeated, and otherwise
    /// nothing.
    Null,
}

/// Walk `value`, yielding every key with a scalar value and every array's
/// length.
pub(crate) fn flatten(value: &Value) -> Vec<FieldValue> {
    let mut out = Vec::new();
    walk(value, "", &mut out);
    out
}

fn walk(value: &Value, path: &str, out: &mut Vec<FieldValue>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                match v {
                    Value::Number(n) => out.push(FieldValue {
                        path: child,
                        key: k.clone(),
                        value: FieldScalar::Number(n.as_f64().unwrap_or(f64::NAN)),
                    }),
                    Value::String(s) => out.push(FieldValue {
                        path: child,
                        key: k.clone(),
                        value: s
                            .parse::<f64>()
                            .map(FieldScalar::Number)
                            .unwrap_or_else(|_| FieldScalar::Text(s.clone())),
                    }),
                    Value::Bool(b) => out.push(FieldValue {
                        path: child,
                        key: k.clone(),
                        value: FieldScalar::Text(b.to_string()),
                    }),
                    Value::Array(items) => {
                        out.push(FieldValue {
                            path: child.clone(),
                            key: k.clone(),
                            value: FieldScalar::Count(items.len()),
                        });
                        for (i, item) in items.iter().enumerate() {
                            walk(item, &format!("{child}[{i}]"), out);
                        }
                    }
                    Value::Object(_) => walk(v, &child, out),
                    Value::Null => out.push(FieldValue {
                        path: child,
                        key: k.clone(),
                        value: FieldScalar::Null,
                    }),
                }
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk(item, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// Lists whose rows describe more than one Task Queue type, with the types.
///
/// The Temporal CLI prints `task-queue describe` as rows that each name their
/// `taskQueueType`, and without `--task-queue-type` it describes the Workflow
/// and the Activity Task Queue together, so its rows cannot all be read as the
/// queue a request is about.
pub(crate) fn mixed_type_lists(value: &Value) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    find_mixed(value, "", &mut out);
    out
}

fn find_mixed(value: &Value, path: &str, out: &mut Vec<(String, Vec<String>)>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let child = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                find_mixed(v, &child, out);
            }
        }
        Value::Array(items) => {
            let mut types: Vec<String> = items
                .iter()
                .filter_map(|item| item.get("taskQueueType")?.as_str().map(str::to_owned))
                .collect();
            types.sort();
            types.dedup();
            if types.len() > 1 {
                out.push((path.to_owned(), types));
            }
            for (i, item) in items.iter().enumerate() {
                find_mixed(item, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// `a`, `a and b`, `a, b and c`.
pub(crate) fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A whitespace-aligned table as printed by the Temporal CLI.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextTable {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Find tables whose header row contains at least one of `known` column
/// names (compared after normalising `ApproximateBacklogCount` and
/// `approximate_backlog_count` to the same form).
pub(crate) fn text_tables(text: &str, known: &[String]) -> Vec<TextTable> {
    let known_norm: Vec<String> = known.iter().map(|k| normalise(k)).collect();
    let lines: Vec<&str> = text.lines().collect();
    let mut tables = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let headers: Vec<String> = lines[i].split_whitespace().map(str::to_owned).collect();
        let hits = headers
            .iter()
            .filter(|h| known_norm.contains(&normalise(h)))
            .count();
        if hits == 0 || headers.len() < 2 {
            i += 1;
            continue;
        }
        let mut rows = Vec::new();
        let mut j = i + 1;
        while j < lines.len() && !lines[j].trim().is_empty() {
            let cells = split_cells(lines[j], headers.len());
            if cells.len() == headers.len() {
                rows.push(cells);
            }
            j += 1;
        }
        tables.push(TextTable { headers, rows });
        i = j;
    }
    tables
}

/// Split a row into `n` cells: on runs of two or more spaces when that yields
/// the right count, otherwise on single whitespace with the remainder merged
/// into the last cell.
fn split_cells(line: &str, n: usize) -> Vec<String> {
    let wide: Vec<String> = line
        .split("  ")
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
        .collect();
    if wide.len() == n {
        return wide;
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() == n {
        return parts.into_iter().map(str::to_owned).collect();
    }
    Vec::new()
}

/// `ApproximateBacklogCount`, `approximate_backlog_count` and
/// `approximateBacklogCount` all normalise to `approximatebacklogcount`.
pub(crate) fn normalise(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_nested_json() {
        let v: Value = serde_json::from_str(
            r#"{"stats":{"approximateBacklogCount":"41200","approximateBacklogAge":"31.4s"},"pollers":[{"identity":"a"},{"identity":"b"}]}"#,
        )
        .expect("json");
        let f = flatten(&v);
        assert!(
            f.iter()
                .any(|x| x.key == "approximateBacklogCount"
                    && x.value == FieldScalar::Number(41200.0))
        );
        assert!(
            f.iter().any(|x| x.key == "approximateBacklogAge"
                && x.value == FieldScalar::Text("31.4s".into()))
        );
        assert!(
            f.iter()
                .any(|x| x.key == "pollers" && x.value == FieldScalar::Count(2))
        );
    }

    #[test]
    fn finds_lists_that_mix_task_queue_types() {
        // `temporal task-queue describe -o json` without `--task-queue-type`.
        let v: Value = serde_json::from_str(
            r#"{"reachability":null,"pollers":null,"stats":[
                {"buildId":"UNVERSIONED","taskQueueType":"activity","approximateBacklogCount":0},
                {"buildId":"UNVERSIONED","taskQueueType":"workflow","approximateBacklogCount":25}]}"#,
        )
        .expect("json");
        assert_eq!(
            mixed_type_lists(&v),
            vec![(
                "stats".to_owned(),
                vec!["activity".to_owned(), "workflow".to_owned()]
            )]
        );
        let one: Value = serde_json::from_str(
            r#"{"stats":[{"buildId":"UNVERSIONED","taskQueueType":"activity"},{"buildId":"b1","taskQueueType":"activity"}]}"#,
        )
        .expect("json");
        assert!(mixed_type_lists(&one).is_empty());
        assert_eq!(
            and_list(&["`stats`".to_owned(), "`pollers`".to_owned()]),
            "`stats` and `pollers`"
        );
    }

    #[test]
    fn finds_cli_tables() {
        let text = "Task Queue Statistics:\n  BuildID  TaskQueueType  ApproximateBacklogCount  ApproximateBacklogAge  TasksAddRate  TasksDispatchRate\n  UNVERSIONED  workflow  41200  31.4s  1850.2  612.7\n\n";
        let tables = text_tables(text, &["approximate_backlog_count".to_owned()]);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].rows.len(), 1);
        assert_eq!(tables[0].rows[0][3], "31.4s");
        assert_eq!(normalise("TasksAddRate"), normalise("tasks_add_rate"));
    }
}
