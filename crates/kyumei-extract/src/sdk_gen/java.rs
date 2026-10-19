//! Java SDK metrics: `static final String` constants built from `TEMPORAL_METRICS_PREFIX`, and
//! the tally `Scope` calls that record them.
//!
//! ```java
//! public static final String WORKFLOW_TASK_SCHEDULE_TO_START_LATENCY =
//!     TEMPORAL_METRICS_PREFIX + "workflow_task_schedule_to_start_latency";
//! metricsScope.timer(MetricsType.WORKFLOW_TASK_SCHEDULE_TO_START_LATENCY).record(…);
//! ```
//!
//! A call can pass the name through a variable or field; assignments in the same file are
//! followed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result, ensure};

use super::{
    Family, Kind, KindBasis, Scan, SdkMetric, files_with_extension, kind_from_constant_name,
    line_of, line_text, unprefixed,
};
use crate::registry_gen::{is_ident, is_space};

/// The constant every name is built from.
const PREFIX: &str = "TEMPORAL_METRICS_PREFIX";
/// tally `Scope` methods and the kind of metric each records.
const SCOPE_METHODS: [(&str, Kind); 4] = [
    ("counter", Kind::Counter),
    ("gauge", Kind::Gauge),
    ("timer", Kind::Duration),
    ("histogram", Kind::Histogram),
];
/// Directories that hold no production code.
const SKIP_DIRS: [&str; 4] = ["test", "build", "testFixtures", "jmh"];

/// A `static final String` constant.
#[derive(Clone, Debug)]
struct Const {
    /// The expression's parts: string literals (quoted) and constant names.
    parts: Vec<String>,
    path: String,
    line: usize,
    excerpt: String,
    /// Byte range of the declaration in its file.
    range: (usize, usize),
}

struct JavaFile {
    rel: String,
    src: String,
}

/// Scan a Java SDK checkout (every `src/main/java` tree).
pub fn scan(root: &Path) -> Result<Scan> {
    ensure!(
        root.join("temporal-sdk").is_dir(),
        "{} is not a Temporal Java SDK checkout (it has no temporal-sdk/)",
        root.display()
    );
    let mut paths = Vec::new();
    files_with_extension(root, ".java", &SKIP_DIRS, &mut paths)?;
    let mut files = Vec::new();
    for path in paths {
        let rel = crate::registry_gen::relative(root, &path);
        if !rel.contains("/src/main/java/") {
            continue;
        }
        let src = std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
        files.push(JavaFile { rel, src });
    }

    // Constants by simple name; the prefix constant exists once per MetricsType class.
    let mut consts: BTreeMap<String, Vec<Const>> = BTreeMap::new();
    for f in &files {
        if !f.src.contains("static final String") {
            continue;
        }
        for (name, c) in string_consts(f) {
            consts.entry(name).or_default().push(c);
        }
    }
    let mut metric_consts: BTreeMap<String, Const> = BTreeMap::new();
    let mut warnings = Vec::new();
    for (name, defs) in &consts {
        let metric: Vec<&Const> = defs
            .iter()
            .filter(|c| built_from_prefix(c, &consts, &mut BTreeSet::new()))
            .collect();
        match metric.as_slice() {
            [] => {}
            [one] => {
                metric_consts.insert(name.clone(), (*one).clone());
            }
            many => warnings.push(format!(
                "{name} is defined {} times: {}",
                many.len(),
                many.iter()
                    .map(|c| format!("{}:{}", c.path, c.line))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut kinds: BTreeMap<String, Vec<(Kind, String)>> = BTreeMap::new();
    for f in &files {
        let b = f.src.as_bytes();
        let decls: Vec<(usize, usize)> = metric_consts
            .values()
            .filter(|c| c.path == f.rel)
            .map(|c| c.range)
            .collect();
        let mut i = 0;
        while i < b.len() {
            if let Some(next) = java_opaque(b, i) {
                i = next;
                continue;
            }
            if !is_ident(b[i]) || (i > 0 && is_ident(b[i - 1])) {
                i += 1;
                continue;
            }
            let end = word_end(b, i);
            let word = &f.src[i..end];
            if metric_consts.contains_key(word) && !decls.iter().any(|(s, e)| (*s..*e).contains(&i))
            {
                referenced.insert(word.to_owned());
            }
            if let Some((_, kind)) = SCOPE_METHODS.iter().find(|(m, _)| *m == word)
                && preceded_by_dot(b, i)
                && b.get(end) == Some(&b'(')
                && let Some(close) = java_closing(b, end)
            {
                let arg = f.src[end + 1..close].trim();
                let first = arg.split(',').next().unwrap_or("").trim();
                let at = format!("{}:{}", f.rel, line_of(&f.src, i));
                let simple = first.rsplit('.').next().unwrap_or(first);
                if metric_consts.contains_key(simple) && is_reference(first) {
                    kinds
                        .entry(simple.to_owned())
                        .or_default()
                        .push((*kind, at));
                } else if is_reference(first) && !first.contains('.') {
                    for name in assigned_in_file(&f.src, first, &metric_consts) {
                        kinds.entry(name).or_default().push((*kind, at.clone()));
                    }
                }
            }
            i = end;
        }
    }

    let mut scan = Scan {
        files_scanned: files.len(),
        warnings,
        ..Scan::default()
    };
    for (name, c) in &metric_consts {
        let Some(value) = value(name, &consts, &mut BTreeSet::new()) else {
            scan.warnings.push(format!(
                "{name} at {}:{} does not resolve to a string",
                c.path, c.line
            ));
            continue;
        };
        let Some(metric) = unprefixed(&value) else {
            scan.warnings
                .push(format!("{name} = {value:?} has no temporal_ prefix"));
            continue;
        };
        let recorded = kinds.get(name).map(Vec::as_slice).unwrap_or_default();
        let (kind, basis) = match recorded {
            [] if !referenced.contains(name) => {
                scan.unrecorded.push(format!("{name} ({metric})"));
                continue;
            }
            [] => match kind_from_constant_name(name) {
                Some(kind) => (kind, KindBasis::ConstantName(name.clone())),
                None => {
                    scan.warnings.push(format!(
                        "{name} ({metric}) is referenced, but no recording or name says its kind"
                    ));
                    continue;
                }
            },
            [(first, _), rest @ ..] => {
                if let Some((other, at)) = rest.iter().find(|(k, _)| k != first) {
                    scan.warnings.push(format!(
                        "{name} is recorded as a {} and, at {at}, as a {}",
                        first.label(),
                        other.label()
                    ));
                }
                (*first, KindBasis::Recording)
            }
        };
        scan.metrics.push(SdkMetric {
            family: Family::Java,
            name: metric.to_owned(),
            kind,
            kind_basis: basis,
            description: None,
            path: c.path.clone(),
            line: c.line,
            excerpt: c.excerpt.clone(),
        });
    }
    scan.metrics.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(scan)
}

/// A constant's value, resolving names in its expression.
fn value(
    name: &str,
    consts: &BTreeMap<String, Vec<Const>>,
    seen: &mut BTreeSet<String>,
) -> Option<String> {
    if !seen.insert(name.to_owned()) {
        return None;
    }
    // The prefix constant is defined in each MetricsType class with the same value.
    let defs = consts.get(name)?;
    let mut out = String::new();
    for part in &defs[0].parts {
        match java_string(part) {
            Some(s) => out.push_str(&s),
            None => out.push_str(&value(part.rsplit('.').next()?, consts, seen)?),
        }
    }
    Some(out)
}

/// Whether a constant is built from [`PREFIX`], directly or through another constant.
fn built_from_prefix(
    c: &Const,
    consts: &BTreeMap<String, Vec<Const>>,
    seen: &mut BTreeSet<String>,
) -> bool {
    c.parts.iter().any(|part| {
        let simple = part.rsplit('.').next().unwrap_or(part);
        java_string(part).is_none()
            && (simple == PREFIX
                || (seen.insert(simple.to_owned())
                    && consts.get(simple).is_some_and(|defs| {
                        defs.iter().any(|d| built_from_prefix(d, consts, seen))
                    })))
    })
}

/// `static final String NAME = "…" + OTHER;` declarations.
fn string_consts(f: &JavaFile) -> Vec<(String, Const)> {
    let b = f.src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if let Some(next) = java_opaque(b, i) {
            i = next;
            continue;
        }
        if !b[i..].starts_with(b"String") || (i > 0 && is_ident(b[i - 1])) {
            i += 1;
            continue;
        }
        let end = word_end(b, i);
        if end != i + 6 {
            i = end;
            continue;
        }
        // The modifiers since the previous statement must include `static final`.
        let decl_start = f.src[..i].rfind([';', '{', '}']).map_or(0, |p| p + 1);
        let modifiers = &f.src[decl_start..i];
        let name_start = skip_space(b, end);
        let name_end = word_end(b, name_start);
        let eq = skip_space(b, name_end);
        if !(modifiers.split_whitespace().any(|w| w == "static")
            && modifiers.split_whitespace().any(|w| w == "final"))
            || name_end == name_start
            || b.get(eq) != Some(&b'=')
        {
            i = end;
            continue;
        }
        let Some(semi) = statement_end(b, eq + 1) else {
            break;
        };
        let expr = &f.src[eq + 1..semi];
        if let Some(parts) = expression_parts(expr) {
            // Cite the line of the first string literal, or of the name.
            let lit = expr.find('"').map_or(name_start, |p| eq + 1 + p);
            let line = line_of(&f.src, lit);
            out.push((
                f.src[name_start..name_end].to_owned(),
                Const {
                    parts,
                    path: f.rel.clone(),
                    line,
                    excerpt: line_text(&f.src, line),
                    range: (decl_start, semi + 1),
                },
            ));
        }
        i = semi + 1;
    }
    out
}

/// The parts of `"a" + NAME + Other.NAME`, or `None` for anything else.
fn expression_parts(expr: &str) -> Option<Vec<String>> {
    let b = expr.as_bytes();
    let mut parts = Vec::new();
    let mut seg = 0;
    let mut i = 0;
    while i <= b.len() {
        if i < b.len()
            && let Some(next) = java_opaque(b, i)
        {
            i = next;
            continue;
        }
        if i == b.len() || b[i] == b'+' {
            let part = expr[seg..i].trim();
            if java_string(part).is_none() && !is_reference(part) {
                return None;
            }
            parts.push(part.to_owned());
            seg = i + 1;
        }
        i += 1;
    }
    Some(parts)
}

/// `NAME` or `Class.NAME` or `io.temporal.worker.MetricsType.NAME`.
fn is_reference(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|seg| {
            !seg.is_empty()
                && !seg.as_bytes()[0].is_ascii_digit()
                && seg
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$')
        })
}

/// The metric constants assigned to `var` anywhere in the file (`var = MetricsType.NAME;`).
fn assigned_in_file(src: &str, var: &str, metric_consts: &BTreeMap<String, Const>) -> Vec<String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = src[from..].find(var) {
        let s = from + p;
        let e = s + var.len();
        from = e;
        if (s > 0 && (is_ident(b[s - 1]) || b[s - 1] == b'.'))
            || b.get(e).is_some_and(|&c| is_ident(c))
        {
            continue;
        }
        let rest = src[e..].trim_start();
        let Some(rest) = rest.strip_prefix('=').filter(|r| !r.starts_with('=')) else {
            continue;
        };
        let rest = rest.trim_start();
        let end = rest.find(';').unwrap_or(rest.len());
        let expr = rest[..end].trim();
        let simple = expr.rsplit('.').next().unwrap_or(expr);
        if is_reference(expr)
            && metric_consts.contains_key(simple)
            && !out.iter().any(|n| n == simple)
        {
            out.push(simple.to_owned());
        }
    }
    out
}

/// The `;` that ends the statement starting at `from`, outside literals and brackets.
fn statement_end(b: &[u8], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = from;
    while i < b.len() {
        if let Some(next) = java_opaque(b, i) {
            i = next;
            continue;
        }
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.checked_sub(1)?,
            b';' if depth == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// The `)` that closes the `(` at `open`.
fn java_closing(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        if let Some(next) = java_opaque(b, i) {
            i = next;
            continue;
        }
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// If a comment, string, text block or character literal starts at `i`, the index just past it.
fn java_opaque(b: &[u8], i: usize) -> Option<usize> {
    match b[i] {
        b'/' => match b.get(i + 1) {
            Some(b'/') => Some(find(b, i + 2, b"\n").unwrap_or(b.len())),
            Some(b'*') => Some(find(b, i + 2, b"*/").map_or(b.len(), |j| j + 2)),
            _ => None,
        },
        b'"' if b[i..].starts_with(b"\"\"\"") => {
            let mut j = i + 3;
            while j < b.len() {
                if b[j] == b'\\' {
                    j += 2;
                } else if b[j..].starts_with(b"\"\"\"") {
                    return Some(j + 3);
                } else {
                    j += 1;
                }
            }
            Some(b.len())
        }
        b'"' | b'\'' => {
            let q = b[i];
            let mut j = i + 1;
            while j < b.len() {
                match b[j] {
                    b'\\' => j += 2,
                    b'\n' => return Some(j),
                    c if c == q => return Some(j + 1),
                    _ => j += 1,
                }
            }
            Some(b.len())
        }
        _ => None,
    }
}

/// The value of a Java string literal, or `None` if `s` is not one.
fn java_string(s: &str) -> Option<String> {
    let body = s.trim().strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            _ => return None,
        }
    }
    Some(out)
}

fn find(b: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    b.get(from..)?
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|p| from + p)
}

fn word_end(b: &[u8], at: usize) -> usize {
    let mut k = at;
    while k < b.len() && is_ident(b[k]) {
        k += 1;
    }
    k
}

fn skip_space(b: &[u8], mut at: usize) -> usize {
    while at < b.len() && is_space(b[at]) {
        at += 1;
    }
    at
}

fn preceded_by_dot(b: &[u8], at: usize) -> bool {
    let mut k = at;
    while k > 0 && is_space(b[k - 1]) {
        k -= 1;
    }
    k > 0 && b[k - 1] == b'.'
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKER_TYPES: &str = r#"package io.temporal.worker;

public final class MetricsType {
  public static final String TEMPORAL_METRICS_PREFIX = "temporal_";

  public static final String WORKFLOW_COMPLETED_COUNTER =
      TEMPORAL_METRICS_PREFIX + "workflow_completed";
  public static final String WORKFLOW_TASK_SCHEDULE_TO_START_LATENCY =
      TEMPORAL_METRICS_PREFIX + "workflow_task_schedule_to_start_latency";
  // "a comment with = and ;"
  public static final String NUM_POLLERS = TEMPORAL_METRICS_PREFIX + "num_pollers";
  public static final String WORKFLOW_ACTIVE_THREAD_COUNT =
      TEMPORAL_METRICS_PREFIX + "workflow_active_thread_count";
}
"#;

    const CLIENT_TYPES: &str = r#"package io.temporal.serviceclient;

public final class MetricsType {
  public static final String TEMPORAL_METRICS_PREFIX = "temporal_";
  public static final String TEMPORAL_LONG_REQUEST = TEMPORAL_METRICS_PREFIX + "long_request";
  public static final String TEMPORAL_LONG_REQUEST_FAILURE = TEMPORAL_LONG_REQUEST + "_failure";
  static final String NOT_A_METRIC = "temporal_but_not_built_from_the_prefix";
}
"#;

    const POLL: &str = r#"package io.temporal.internal.worker;

class WorkflowPollTask {
  private String completion;

  void poll() {
    metricsScope
        .timer(MetricsType.WORKFLOW_TASK_SCHEDULE_TO_START_LATENCY)
        .record(d);
    MetricsTag.tagged(metricsScope, PollerType.WORKFLOW_TASK).gauge(MetricsType.NUM_POLLERS).update(1);
    String text = """
        .counter(MetricsType.NUM_POLLERS)
        """;
    scope.counter(MetricsType.TEMPORAL_LONG_REQUEST_FAILURE).inc(1);
  }

  void done() {
    completion = MetricsType.WORKFLOW_COMPLETED_COUNTER;
  }
}
"#;

    fn checkout() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (rel, text) in [
            (
                "temporal-sdk/src/main/java/io/temporal/worker/MetricsType.java",
                WORKER_TYPES,
            ),
            (
                "temporal-serviceclient/src/main/java/io/temporal/serviceclient/MetricsType.java",
                CLIENT_TYPES,
            ),
            (
                "temporal-sdk/src/main/java/io/temporal/internal/worker/WorkflowPollTask.java",
                POLL,
            ),
            (
                "temporal-sdk/src/test/java/io/temporal/Ignored.java",
                "class Ignored { void t() { s.counter(MetricsType.WORKFLOW_ACTIVE_THREAD_COUNT); } }",
            ),
        ] {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, text).expect("write");
        }
        dir
    }

    #[test]
    fn constants_get_their_kinds_from_scope_calls() {
        let dir = checkout();
        let scan = scan(dir.path()).expect("scan");
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
        let found: Vec<(&str, Kind, bool)> = scan
            .metrics
            .iter()
            .map(|m| {
                (
                    m.name.as_str(),
                    m.kind,
                    m.kind_basis == KindBasis::Recording,
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                ("long_request_failure", Kind::Counter, true),
                ("num_pollers", Kind::Gauge, true),
                // only assigned to a field here: the name says counter
                ("workflow_completed", Kind::Counter, false),
                (
                    "workflow_task_schedule_to_start_latency",
                    Kind::Duration,
                    true
                ),
            ]
        );
        // Test sources do not count, so this one is never recorded; neither is a constant
        // built some other way.
        assert_eq!(
            scan.unrecorded,
            [
                "TEMPORAL_LONG_REQUEST (long_request)",
                "WORKFLOW_ACTIVE_THREAD_COUNT (workflow_active_thread_count)"
            ]
        );
        let completed = &scan.metrics[2];
        assert_eq!(completed.line, 7);
        assert_eq!(
            completed.excerpt,
            "TEMPORAL_METRICS_PREFIX + \"workflow_completed\";"
        );
    }
}
