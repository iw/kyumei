//! Dynamic configuration fragments (the server's YAML file format).
//!
//! Only the shape Temporal's file-based dynamic config uses is understood:
//!
//! ```yaml
//! matching.numTaskqueueReadPartitions:
//!   - value: 8
//!     constraints:
//!       namespace: "commerce-prod"
//!       taskQueueName: "order-fulfillment"
//! ```

use std::collections::BTreeMap;

use regex::Regex;
use std::sync::LazyLock;

/// One `- value:` entry of a key.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConfigEntry {
    pub key: String,
    pub value: String,
    pub constraints: BTreeMap<String, String>,
}

static KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z][A-Za-z0-9_.\-]*)\s*:\s*$").expect("key pattern"));
static VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*-\s*value\s*:\s*(.+?)\s*$").expect("value pattern"));
static CONSTRAINTS_INLINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*constraints\s*:\s*\{(.*)\}\s*$").expect("inline pattern"));
static CONSTRAINTS_BLOCK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*constraints\s*:\s*$").expect("block pattern"));
static PAIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*([A-Za-z][A-Za-z0-9_]*)\s*:\s*["']?([^"',}]*)["']?\s*$"#)
        .expect("pair pattern")
});

pub(crate) fn looks_like_dynamic_config(text: &str) -> bool {
    let mut keys = 0;
    let mut values = 0;
    for line in text.lines() {
        if KEY.is_match(line) && line.contains('.') {
            keys += 1;
        }
        if VALUE.is_match(line) {
            values += 1;
        }
    }
    keys >= 1 && values >= 1
}

pub(crate) fn parse_dynamic_config(text: &str) -> Vec<ConfigEntry> {
    let mut entries = Vec::new();
    let mut key: Option<String> = None;
    let mut current: Option<ConfigEntry> = None;
    let mut in_block = false;
    for raw in text.lines() {
        let line = raw.split(" #").next().unwrap_or(raw);
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(caps) = KEY.captures(line)
            && !line.starts_with(char::is_whitespace)
        {
            entries.extend(current.take());
            key = Some(caps[1].to_owned());
            in_block = false;
            continue;
        }
        if let Some(caps) = VALUE.captures(line) {
            entries.extend(current.take());
            if let Some(k) = &key {
                current = Some(ConfigEntry {
                    key: k.clone(),
                    value: unquote(&caps[1]),
                    constraints: BTreeMap::new(),
                });
            }
            in_block = false;
            continue;
        }
        if let Some(caps) = CONSTRAINTS_INLINE.captures(line) {
            if let Some(entry) = current.as_mut() {
                for pair in caps[1].split(',') {
                    if let Some((k, v)) = pair.split_once(':') {
                        entry
                            .constraints
                            .insert(k.trim().to_owned(), unquote(v.trim()));
                    }
                }
            }
            continue;
        }
        if CONSTRAINTS_BLOCK.is_match(line) {
            in_block = true;
            continue;
        }
        if in_block && let (Some(entry), Some(caps)) = (current.as_mut(), PAIR.captures(line)) {
            entry
                .constraints
                .insert(caps[1].to_owned(), caps[2].trim().to_owned());
        }
    }
    entries.extend(current);
    entries
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim_matches('\'').to_owned()
}

/// How specific an entry is for a namespace and task queue: `None` when a
/// constraint excludes them, otherwise the number of matching constraints.
pub(crate) fn specificity(
    entry: &ConfigEntry,
    namespace: Option<&str>,
    task_queue: Option<&str>,
    task_type: Option<&str>,
) -> Option<usize> {
    let mut score = 0;
    for (k, v) in &entry.constraints {
        let target = match k.as_str() {
            "namespace" => namespace,
            "taskQueueName" => task_queue,
            "taskType" => task_type,
            // Constraints Kyumei cannot evaluate exclude the entry.
            _ => return None,
        };
        match target {
            Some(t) if t.eq_ignore_ascii_case(v) => score += 1,
            _ => return None,
        }
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
matching.numTaskqueueReadPartitions:
  - value: 4
  - value: 8
    constraints:
      namespace: "commerce-prod"
      taskQueueName: "order-fulfillment"
matching.numTaskqueueWritePartitions:
  - value: 8
    constraints: {namespace: "commerce-prod", taskQueueName: "order-fulfillment"}
"#;

    #[test]
    fn parses_entries_and_constraints() {
        assert!(looks_like_dynamic_config(SAMPLE));
        let entries = parse_dynamic_config(SAMPLE);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].value, "8");
        assert_eq!(
            entries[1]
                .constraints
                .get("taskQueueName")
                .map(String::as_str),
            Some("order-fulfillment")
        );
        assert_eq!(entries[2].constraints.len(), 2);
    }

    #[test]
    fn most_specific_entry_wins() {
        let entries = parse_dynamic_config(SAMPLE);
        let ns = Some("commerce-prod");
        let tq = Some("order-fulfillment");
        assert_eq!(specificity(&entries[0], ns, tq, Some("Workflow")), Some(0));
        assert_eq!(specificity(&entries[1], ns, tq, Some("Workflow")), Some(2));
        assert_eq!(
            specificity(&entries[1], ns, Some("other"), Some("Workflow")),
            None
        );
    }
}
