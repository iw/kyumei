//! Go SDK metrics: name constants built from `TemporalMetricsPrefix` in
//! `internal/common/metrics`, and the handler calls that record them.
//!
//! ```go
//! WorkflowTaskScheduleToStartLatency = TemporalMetricsPrefix + "workflow_task_schedule_to_start_latency"
//! metricsHandler.Timer(metrics.WorkflowTaskScheduleToStartLatency).Record(scheduleToStart)
//! ```
//!
//! A call can pass the name through a local variable (`metric := TemporalRequest; …
//! handler.Counter(metric)`); assignments earlier in the same function are followed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use anyhow::{Context as _, Result, ensure};

use super::{
    Family, Kind, KindBasis, Scan, SdkMetric, kind_from_constant_name, line_text, unprefixed,
};
use crate::metrics_gen::{GoFile, go_module, ident_end};
use crate::registry_gen::{
    closing, collect_go_files, is_ident, is_space, relative, skip_opaque, split_args, string_const,
};

/// The package that defines the names.
const METRICS_DIR: &str = "internal/common/metrics";
/// The constant every name is built from.
const PREFIX: &str = "TemporalMetricsPrefix";
/// Handler methods and the kind of metric each records.
const HANDLER_METHODS: [(&str, Kind); 3] = [
    ("Counter", Kind::Counter),
    ("Gauge", Kind::Gauge),
    ("Timer", Kind::Duration),
];

/// A string constant in the metrics package.
#[derive(Clone, Debug)]
struct Const {
    expr: String,
    path: String,
    line: usize,
    excerpt: String,
}

/// Scan a Go SDK module (a checkout or the module cache's `go.temporal.io/sdk@vX`).
pub fn scan(root: &Path) -> Result<Scan> {
    ensure!(
        root.join(METRICS_DIR).is_dir(),
        "{} is not a Temporal Go SDK checkout (it has no {METRICS_DIR})",
        root.display()
    );
    let module = go_module(root)?;
    let mut paths = Vec::new();
    collect_go_files(root, &mut paths)?;
    paths.sort();
    let mut files = Vec::new();
    for path in &paths {
        let rel = relative(root, path);
        if rel.starts_with("test/") {
            continue; // integration tests
        }
        let src = std::fs::read_to_string(path).with_context(|| format!("reading {rel}"))?;
        files.push(GoFile::new(rel, src));
    }

    // The metrics package's string constants, and the byte ranges of their declarations.
    let mut consts: BTreeMap<String, Const> = BTreeMap::new();
    let mut decl_ranges: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for f in files.iter().filter(|f| f.dir() == METRICS_DIR) {
        for (name, c, range) in const_specs(f) {
            consts.insert(name, c);
            decl_ranges.entry(f.rel.clone()).or_default().push(range);
        }
    }
    let values = resolve_values(&consts);
    let metric_consts: BTreeSet<String> = consts
        .keys()
        .filter(|name| built_from_prefix(name, &consts, &mut BTreeSet::new()))
        .cloned()
        .collect();

    let packages: HashMap<&str, &str> = files
        .iter()
        .filter_map(|f| Some((f.dir(), f.package.as_deref()?)))
        .collect();
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut kinds: BTreeMap<String, Vec<(Kind, String)>> = BTreeMap::new();
    for f in &files {
        let imports = f.imports(&module, &packages);
        let aliases: BTreeSet<&str> = imports
            .iter()
            .filter(|(_, dir)| dir.as_str() == METRICS_DIR)
            .map(|(alias, _)| alias.as_str())
            .collect();
        let local = f.dir() == METRICS_DIR;
        if aliases.is_empty() && !local {
            continue;
        }
        let in_decl = |at: usize| {
            decl_ranges
                .get(&f.rel)
                .is_some_and(|ranges| ranges.iter().any(|(s, e)| (*s..*e).contains(&at)))
        };
        let reference = |text: &str| -> Option<String> {
            let text = text.trim();
            let name = match text.split_once('.') {
                Some((alias, name)) if aliases.contains(alias) => name,
                Some(_) => return None,
                None if local => text,
                None => return None,
            };
            metric_consts.contains(name).then(|| name.to_owned())
        };
        let b = f.src.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if let Some(next) = skip_opaque(b, i) {
                i = next;
                continue;
            }
            if !is_ident(b[i]) || (i > 0 && is_ident(b[i - 1])) {
                i += 1;
                continue;
            }
            let end = ident_end(b, i);
            let word = &f.src[i..end];
            let after_dot = i > 0 && b[i - 1] == b'.';
            // A reference: `metrics.Name`, or a bare name inside the package.
            if !after_dot {
                if b.get(end) == Some(&b'.') && aliases.contains(word) {
                    let sel_end = ident_end(b, end + 1);
                    if let Some(name) = reference(&f.src[i..sel_end]) {
                        referenced.insert(name);
                    }
                    i = sel_end;
                    continue;
                }
                if let Some(name) = reference(word)
                    && !in_decl(i)
                {
                    referenced.insert(name);
                }
            }
            // A handler call: `.Counter(arg)`.
            if let Some((_, kind)) = HANDLER_METHODS.iter().find(|(m, _)| *m == word)
                && preceded_by_dot(b, i)
                && b.get(end) == Some(&b'(')
                && let Some(args) = split_args(&f.src, end + 1)
                && let [arg] = args.as_slice()
            {
                let at = format!("{}:{}", f.rel, f.line_of(i));
                if let Some(name) = reference(arg) {
                    kinds.entry(name).or_default().push((*kind, at));
                } else if is_identifier(arg) {
                    for name in assigned_in_function(f, i, arg, &reference) {
                        kinds.entry(name).or_default().push((*kind, at.clone()));
                    }
                }
            }
            i = end;
        }
    }

    let mut scan = Scan {
        files_scanned: files.len(),
        ..Scan::default()
    };
    for name in &metric_consts {
        let c = &consts[name];
        let value = values.get(name).context("unresolved constant")?;
        let Some(metric) = unprefixed(value) else {
            scan.warnings
                .push(format!("{name} = {value:?} has no temporal_ prefix"));
            continue;
        };
        let recorded: Vec<&(Kind, String)> = kinds
            .get(name)
            .map(|k| k.iter().collect())
            .unwrap_or_default();
        let kind = match recorded.as_slice() {
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
            family: Family::Go,
            name: metric.to_owned(),
            kind: kind.0,
            kind_basis: kind.1,
            description: None,
            path: c.path.clone(),
            line: c.line,
            excerpt: c.excerpt.clone(),
        });
    }
    scan.metrics.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(scan)
}

/// `Name = expr` specs of the file's `const` declarations: name, constant, declaration range.
fn const_specs(f: &GoFile) -> Vec<(String, Const, (usize, usize))> {
    let b = f.src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if let Some(next) = skip_opaque(b, i) {
            i = next;
            continue;
        }
        if !b[i..].starts_with(b"const")
            || (i > 0 && is_ident(b[i - 1]))
            || b.get(i + 5).is_some_and(|&c| is_ident(c))
        {
            i += 1;
            continue;
        }
        let mut k = i + 5;
        while k < b.len() && is_space(b[k]) {
            k += 1;
        }
        let (start, end) = if b.get(k) == Some(&b'(') {
            match closing(b, k) {
                Some(close) => (k + 1, close),
                None => break,
            }
        } else {
            (k, f.src[k..].find('\n').map_or(b.len(), |p| k + p))
        };
        let mut line_start = start;
        for line in f.src[start..end].split_inclusive('\n') {
            let at = line_start;
            line_start += line.len();
            let code = strip_line_comment(line);
            let Some((lhs, expr)) = code.split_once('=') else {
                continue;
            };
            let name = lhs.split_whitespace().next().unwrap_or("");
            if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
                continue;
            }
            let n = f.line_of(at + line.len() - line.trim_start().len());
            out.push((
                name.to_owned(),
                Const {
                    expr: expr.trim().to_owned(),
                    path: f.rel.clone(),
                    line: n,
                    excerpt: line_text(&f.src, n),
                },
                (start, end),
            ));
        }
        i = end;
    }
    out
}

/// The line without a trailing `//` comment (ignoring `//` inside string literals).
fn strip_line_comment(line: &str) -> &str {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            return &line[..i];
        }
        if let Some(next) = skip_opaque(b, i) {
            i = next;
            continue;
        }
        i += 1;
    }
    line
}

/// Every constant's value: string literals and other constants joined by `+`.
fn resolve_values(consts: &BTreeMap<String, Const>) -> BTreeMap<String, String> {
    fn value(
        name: &str,
        consts: &BTreeMap<String, Const>,
        seen: &mut BTreeSet<String>,
    ) -> Option<String> {
        if !seen.insert(name.to_owned()) {
            return None;
        }
        let mut out = String::new();
        for part in consts.get(name)?.expr.split('+') {
            let part = part.trim();
            match string_const(part) {
                Some(s) => out.push_str(&s),
                None if is_identifier(part) => out.push_str(&value(part, consts, seen)?),
                None => return None,
            }
        }
        Some(out)
    }
    consts
        .keys()
        .filter_map(|name| Some((name.clone(), value(name, consts, &mut BTreeSet::new())?)))
        .collect()
}

/// Whether a constant's value is built from [`PREFIX`], directly or through another constant.
fn built_from_prefix(
    name: &str,
    consts: &BTreeMap<String, Const>,
    seen: &mut BTreeSet<String>,
) -> bool {
    if name == PREFIX || !seen.insert(name.to_owned()) {
        return false;
    }
    let Some(c) = consts.get(name) else {
        return false;
    };
    c.expr.split('+').map(str::trim).any(|part| {
        part == PREFIX || (is_identifier(part) && built_from_prefix(part, consts, seen))
    })
}

fn is_identifier(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && !s.as_bytes()[0].is_ascii_digit()
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// Whether the identifier at `at` follows a `.` (possibly on the previous line).
fn preceded_by_dot(b: &[u8], at: usize) -> bool {
    let mut k = at;
    while k > 0 && is_space(b[k - 1]) {
        k -= 1;
    }
    k > 0 && b[k - 1] == b'.'
}

/// The metric constants assigned to variable `var` earlier in the function holding byte `at`.
fn assigned_in_function(
    f: &GoFile,
    at: usize,
    var: &str,
    reference: &impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let var = var.trim();
    let start = f.src[..at].rfind("\nfunc ").unwrap_or(0);
    let region = &f.src[start..at];
    let b = region.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = region[i..].find(var) {
        let s = i + p;
        let e = s + var.len();
        i = e;
        if (s > 0 && (is_ident(b[s - 1]) || b[s - 1] == b'.'))
            || b.get(e).is_some_and(|&c| is_ident(c))
        {
            continue;
        }
        let rest = region[e..].trim_start();
        let rest = rest
            .strip_prefix(":=")
            .or_else(|| rest.strip_prefix('=').filter(|r| !r.starts_with('=')));
        let Some(rest) = rest else {
            continue;
        };
        let rest = rest.trim_start();
        let expr_end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(rest.len());
        if let Some(name) = reference(&rest[..expr_end])
            && !out.contains(&name)
        {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONSTANTS: &str = r#"package metrics

// Metrics keys
const (
	TemporalMetricsPrefix = "temporal_"

	WorkflowCompletedCounter = TemporalMetricsPrefix + "workflow_completed" // a comment
	WorkflowTaskScheduleToStartLatency = TemporalMetricsPrefix + "workflow_task_schedule_to_start_latency"
	NumPoller                = TemporalMetricsPrefix + "num_pollers"
	TemporalRequest          = TemporalMetricsPrefix + "request"
	TemporalRequestFailure   = TemporalRequest + "_failure"
	WorkflowActiveThreadCount = TemporalMetricsPrefix + "workflow_active_thread_count"
	ActivityTaskErrorCounter  = TemporalMetricsPrefix + "activity_task_error"
)

const (
	ClientTagValue = "temporal_go"
)
"#;

    const GRPC: &str = r#"package metrics

func (h handler) record(longPoll bool, failed bool) {
	metric := TemporalRequest
	h.Counter(metric).Inc(1)
	if failed {
		h.Counter(TemporalRequestFailure).Inc(1)
	}
}
"#;

    const WORKER: &str = r#"package internal

import (
	"go.temporal.io/sdk/internal/common/metrics"
)

func (w *worker) poll() {
	w.handler.
		Timer(metrics.WorkflowTaskScheduleToStartLatency).Record(d)
	w.handler.Gauge(metrics.NumPoller).Update(1)
	var counter string
	counter = metrics.WorkflowCompletedCounter
	w.handler.Counter(counter).Inc(1)
	emit(metrics.ActivityTaskErrorCounter)
}
"#;

    fn checkout() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (rel, text) in [
            ("go.mod", "module go.temporal.io/sdk\n"),
            ("internal/common/metrics/constants.go", CONSTANTS),
            ("internal/common/metrics/grpc.go", GRPC),
            ("internal/worker.go", WORKER),
        ] {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, text).expect("write");
        }
        dir
    }

    #[test]
    fn names_get_their_kinds_from_the_recording_calls() {
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
                // referenced, never recorded directly: the name says counter
                ("activity_task_error", Kind::Counter, false),
                ("num_pollers", Kind::Gauge, true),
                ("request", Kind::Counter, true),
                ("request_failure", Kind::Counter, true),
                ("workflow_completed", Kind::Counter, true),
                (
                    "workflow_task_schedule_to_start_latency",
                    Kind::Duration,
                    true
                ),
            ]
        );
        assert_eq!(
            scan.unrecorded,
            ["WorkflowActiveThreadCount (workflow_active_thread_count)"]
        );
        let completed = scan
            .metrics
            .iter()
            .find(|m| m.name == "workflow_completed")
            .expect("completed");
        assert_eq!(completed.line, 7);
        assert_eq!(
            completed.excerpt,
            "WorkflowCompletedCounter = TemporalMetricsPrefix + \"workflow_completed\" // a comment"
        );
    }
}
