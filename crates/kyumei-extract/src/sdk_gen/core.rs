//! Core (Rust) metrics: every instrument registered with a meter call outside test code.
//!
//! ```rust,ignore
//! wf_completed_counter: meter.counter(MetricParameters {
//!     name: "workflow_completed".into(),
//!     description: "Count of successfully completed workflows".into(),
//!     unit: "".into(),
//! }),
//! let mem_usage = meter.gauge_f64("resource_slots_mem_usage".into());
//! ```
//!
//! The name is a string literal or a `&str` constant. The kind comes from the meter method,
//! or from the return type of a local closure that wraps one
//! (`let counter_with_in_mem = |params: MetricParameters| -> Counter { … }`).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context as _, Result};

use super::{Family, Kind, KindBasis, Scan, SdkMetric, files_with_extension, line_of, line_text};
use crate::registry_gen::{is_ident, is_space};

/// Directories under a crate's `src` that hold only test support.
const SKIP_DIRS: [&str; 4] = ["tests", "test_help", "benches", "examples"];

/// Scan a Core checkout (`crates/*/src`).
pub fn scan(root: &Path) -> Result<Scan> {
    let crates = root.join("crates");
    anyhow::ensure!(
        crates.is_dir(),
        "{} is not a Temporal Rust SDK checkout (it has no crates/)",
        root.display()
    );
    let mut paths = Vec::new();
    let mut crate_dirs: Vec<_> = std::fs::read_dir(&crates)
        .with_context(|| format!("reading {}", crates.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("src").is_dir())
        .collect();
    crate_dirs.sort();
    for dir in crate_dirs {
        files_with_extension(&dir.join("src"), ".rs", &SKIP_DIRS, &mut paths)?;
    }
    let mut files = Vec::new();
    for path in &paths {
        let src =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let rel = crate::registry_gen::relative(root, path);
        let masked = mask_test_items(&src);
        files.push((rel, src, masked));
    }

    // `&str` constants anywhere, for names given as constants.
    let mut consts: HashMap<String, Vec<Literal>> = HashMap::new();
    for (rel, src, masked) in &files {
        for (name, lit) in str_consts(rel, src, masked) {
            consts.entry(name).or_default().push(lit);
        }
    }

    let mut scan = Scan {
        files_scanned: files.len(),
        ..Scan::default()
    };
    for (rel, src, masked) in &files {
        if !masked.contains("MetricParameters")
            && !METER_METHODS.iter().any(|(m, _)| masked.contains(m))
        {
            continue;
        }
        for r in registrations(masked) {
            match resolve(&r, rel, src, &consts) {
                Ok(m) => scan.metrics.push(m),
                Err(e) => scan
                    .warnings
                    .push(format!("{rel}:{}: {e:#}", line_of(src, r.at))),
            }
        }
    }
    scan.metrics.sort_by(|a, b| a.name.cmp(&b.name));
    let mut seen: HashMap<String, (String, Kind)> = HashMap::new();
    scan.metrics.retain(|m| match seen.get(&m.name) {
        Some((at, kind)) => {
            if *kind != m.kind {
                scan.warnings.push(format!(
                    "{} is registered as a {} at {at} and as a {} at {}:{}",
                    m.name,
                    kind.label(),
                    m.kind.label(),
                    m.path,
                    m.line
                ));
            }
            false
        }
        None => {
            seen.insert(m.name.clone(), (format!("{}:{}", m.path, m.line), m.kind));
            true
        }
    });
    Ok(scan)
}

/// Meter methods and the kind of instrument each creates.
const METER_METHODS: [(&str, Kind); 7] = [
    ("counter", Kind::Counter),
    ("gauge", Kind::Gauge),
    ("gauge_f64", Kind::Gauge),
    ("up_down_counter", Kind::Gauge),
    ("histogram_duration", Kind::Duration),
    ("histogram", Kind::Histogram),
    ("histogram_f64", Kind::Histogram),
];

/// Instrument types a closure can return, and their kinds.
const INSTRUMENT_TYPES: [(&str, Kind); 7] = [
    ("Counter", Kind::Counter),
    ("Gauge", Kind::Gauge),
    ("GaugeF64", Kind::Gauge),
    ("UpDownCounter", Kind::Gauge),
    ("HistogramDuration", Kind::Duration),
    ("Histogram", Kind::Histogram),
    ("HistogramF64", Kind::Histogram),
];

/// A string literal's value and where it is.
#[derive(Clone, Debug)]
struct Literal {
    value: String,
    path: String,
    line: usize,
    excerpt: String,
}

/// A meter call found in the source.
#[derive(Debug)]
struct Registration {
    /// The method or closure called.
    call: String,
    name: String,
    description: Option<String>,
    /// Byte offset of the name expression.
    at: usize,
}

fn resolve(
    r: &Registration,
    rel: &str,
    src: &str,
    consts: &HashMap<String, Vec<Literal>>,
) -> Result<SdkMetric> {
    let kind = METER_METHODS
        .iter()
        .find(|(m, _)| *m == r.call)
        .map(|(_, k)| *k)
        .or_else(|| closure_kind(src, &r.call))
        .with_context(|| {
            format!(
                "`{}` is not a meter method or a closure returning an instrument",
                r.call
            )
        })?;
    let expr = strip_conversion(&r.name);
    let (name, path, line, excerpt) = if let Some(value) = rust_string(expr) {
        let line = line_of(src, r.at);
        (value, rel.to_owned(), line, line_text(src, line))
    } else {
        let ident = expr.rsplit("::").next().unwrap_or(expr);
        let candidates = consts
            .get(ident)
            .with_context(|| format!("the name `{expr}` is not a string or a known constant"))?;
        anyhow::ensure!(
            candidates.len() == 1,
            "the constant `{ident}` is defined {} times",
            candidates.len()
        );
        let lit = &candidates[0];
        (
            lit.value.clone(),
            lit.path.clone(),
            lit.line,
            lit.excerpt.clone(),
        )
    };
    Ok(SdkMetric {
        family: Family::Core,
        name,
        kind,
        kind_basis: KindBasis::Recording,
        description: r
            .description
            .as_deref()
            .map(strip_conversion)
            .and_then(rust_string)
            .map(|d| crate::registry_gen::one_line(&d))
            .filter(|d| !d.is_empty()),
        path,
        line,
        excerpt,
    })
}

/// `"x".into()` → `"x"`, `NAME.to_string()` → `NAME`.
fn strip_conversion(expr: &str) -> &str {
    let mut e = expr.trim();
    for suffix in [".into()", ".to_string()", ".to_owned()", ".into_owned()"] {
        if let Some(rest) = e.strip_suffix(suffix) {
            e = rest.trim_end();
        }
    }
    e
}

/// The return type of `let <name> = |…| -> Type {` in `src`, as an instrument kind.
fn closure_kind(src: &str, name: &str) -> Option<Kind> {
    let needle = format!("let {name} = |");
    let at = src.find(&needle)?;
    let rest = &src[at + needle.len()..];
    let body = &rest[..rest.find('{')?];
    let ret = body.rsplit("->").next()?.trim();
    INSTRUMENT_TYPES
        .iter()
        .find(|(t, _)| *t == ret)
        .map(|(_, k)| *k)
}

/// Every meter call in `masked` (test code blanked out).
fn registrations(masked: &str) -> Vec<Registration> {
    let b = masked.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if let Some(next) = rust_opaque(b, i) {
            i = next;
            continue;
        }
        if !is_ident(b[i]) || (i > 0 && is_ident(b[i - 1])) {
            i += 1;
            continue;
        }
        let end = word_end(b, i);
        let word = &masked[i..end];
        if word == "MetricParameters" {
            // `call(MetricParameters { … })`
            let open = skip_space(b, end);
            let before = skip_space_back(b, i);
            if b.get(open) == Some(&b'{')
                && before > 0
                && b[before - 1] == b'('
                && let Some(close) = rust_closing(b, open)
            {
                let call_end = skip_space_back(b, before - 1);
                let mut call_start = call_end;
                while call_start > 0 && is_ident(b[call_start - 1]) {
                    call_start -= 1;
                }
                let fields = fields(masked, open + 1, close);
                if let Some((name, at)) = fields.get("name") {
                    out.push(Registration {
                        call: masked[call_start..call_end].to_owned(),
                        name: name.clone(),
                        description: fields.get("description").map(|(d, _)| d.clone()),
                        at: *at,
                    });
                }
                i = close + 1;
                continue;
            }
        } else if METER_METHODS.iter().any(|(m, _)| *m == word)
            && skip_space_back(b, i) > 0
            && b[skip_space_back(b, i) - 1] == b'.'
            && b.get(end) == Some(&b'(')
            && let Some(close) = rust_closing(b, end)
        {
            // `meter.gauge_f64("name".into())`
            let arg = masked[end + 1..close].trim();
            let at = end
                + 1
                + (masked[end + 1..close].len() - masked[end + 1..close].trim_start().len());
            // Only a literal or a constant names a metric; anything else (the C bridge's
            // `options.into()`) passes on a name chosen at run time.
            let name = strip_conversion(arg);
            let named = rust_string(name).is_some()
                || (!name.is_empty()
                    && name
                        .bytes()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_'));
            if named
                && !arg.contains(',')
                && (arg.ends_with(".into()") || arg.ends_with(".to_string()"))
            {
                out.push(Registration {
                    call: word.to_owned(),
                    name: arg.to_owned(),
                    description: None,
                    at,
                });
            }
        }
        i = end;
    }
    out
}

/// The fields of a struct literal between `start` and `end`: name → (expression, offset).
fn fields(src: &str, start: usize, end: usize) -> HashMap<String, (String, usize)> {
    let b = src.as_bytes();
    let mut out = HashMap::new();
    let mut depth = 0usize;
    let mut seg = start;
    let mut i = start;
    let mut push = |from: usize, to: usize| {
        let text = &src[from..to];
        let Some(colon) = text.find(':') else {
            return;
        };
        let name = text[..colon].trim();
        let expr = &text[colon + 1..];
        if !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            let lead = expr.len() - expr.trim_start().len();
            out.insert(
                name.to_owned(),
                (expr.trim().to_owned(), from + colon + 1 + lead),
            );
        }
    };
    while i < end {
        if let Some(next) = rust_opaque(b, i) {
            i = next;
            continue;
        }
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                push(seg, i);
                seg = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    push(seg, end);
    out
}

/// `const NAME: &str = "value";` and `static NAME: &str = "value";` outside test code.
fn str_consts(rel: &str, src: &str, masked: &str) -> Vec<(String, Literal)> {
    let b = masked.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if let Some(next) = rust_opaque(b, i) {
            i = next;
            continue;
        }
        if !is_ident(b[i]) || (i > 0 && is_ident(b[i - 1])) {
            i += 1;
            continue;
        }
        let end = word_end(b, i);
        if matches!(&masked[i..end], "const" | "static") {
            let name_start = skip_space(b, end);
            let name_end = word_end(b, name_start);
            let colon = skip_space(b, name_end);
            if name_end > name_start && b.get(colon) == Some(&b':') {
                let eq = masked[colon..].find('=').map(|p| colon + p);
                let ty = eq.map(|e| masked[colon + 1..e].trim());
                if let (Some(eq), Some(ty)) = (eq, ty)
                    && matches!(ty, "&str" | "&'static str")
                {
                    let lit_start = skip_space(b, eq + 1);
                    if let Some(lit_end) = rust_opaque(b, lit_start)
                        && let Some(value) = rust_string(&masked[lit_start..lit_end])
                        && b.get(skip_space(b, lit_end)) == Some(&b';')
                    {
                        out.push((
                            masked[name_start..name_end].to_owned(),
                            Literal {
                                value,
                                path: rel.to_owned(),
                                line: line_of(src, lit_start),
                                excerpt: line_text(src, line_of(src, lit_start)),
                            },
                        ));
                    }
                }
            }
        }
        i = end;
    }
    out
}

/// Blank out `#[cfg(test)]` items (keeping newlines, so lines still match the source).
pub(crate) fn mask_test_items(src: &str) -> String {
    const ATTR: &[u8] = b"#[cfg(test)]";
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    while i < b.len() {
        if let Some(next) = rust_opaque(b, i) {
            i = next;
            continue;
        }
        if b[i..].starts_with(ATTR) {
            let mut j = i + ATTR.len();
            let mut end = b.len();
            while j < b.len() {
                if let Some(next) = rust_opaque(b, j) {
                    j = next;
                    continue;
                }
                match b[j] {
                    b';' => {
                        end = j + 1;
                        break;
                    }
                    b'{' => {
                        end = rust_closing(b, j).map_or(b.len(), |c| c + 1);
                        break;
                    }
                    _ => j += 1,
                }
            }
            for c in &mut out[i..end] {
                if *c != b'\n' {
                    *c = b' ';
                }
            }
            i = end;
            continue;
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

// --- Rust lexing -----------------------------------------------------------------------------

/// If a comment, string, raw string, byte string or character literal starts at `i`, the
/// index just past it. A lifetime or label (`'a`) is code.
pub(crate) fn rust_opaque(b: &[u8], i: usize) -> Option<usize> {
    match b[i] {
        b'/' => match b.get(i + 1) {
            Some(b'/') => Some(find(b, i + 2, b"\n").unwrap_or(b.len())),
            Some(b'*') => {
                let mut depth = 1;
                let mut j = i + 2;
                while j < b.len() {
                    if b[j..].starts_with(b"/*") {
                        depth += 1;
                        j += 2;
                    } else if b[j..].starts_with(b"*/") {
                        depth -= 1;
                        j += 2;
                        if depth == 0 {
                            return Some(j);
                        }
                    } else {
                        j += 1;
                    }
                }
                Some(b.len())
            }
            _ => None,
        },
        b'"' => Some(string_end(b, i + 1)),
        b'\'' => char_end(b, i),
        b'r' | b'b' if i == 0 || !is_ident(b[i - 1]) => prefixed_literal_end(b, i),
        _ => None,
    }
}

fn find(b: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    b.get(from..)?
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|p| from + p)
}

/// Just past the `"` that closes a string whose body starts at `j`.
fn string_end(b: &[u8], mut j: usize) -> usize {
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

/// `'x'`, `'\n'`, `'\u{1F600}'`: just past the closing quote. `None` for a lifetime.
fn char_end(b: &[u8], i: usize) -> Option<usize> {
    if *b.get(i + 1)? == b'\\' {
        let mut j = i + 3;
        while j < b.len() && j < i + 14 {
            if b[j] == b'\'' {
                return Some(j + 1);
            }
            j += 1;
        }
        return None;
    }
    let len = match b[i + 1] {
        c if c < 0x80 => 1,
        c if c >= 0xF0 => 4,
        c if c >= 0xE0 => 3,
        _ => 2,
    };
    (b.get(i + 1 + len) == Some(&b'\'')).then_some(i + 2 + len)
}

/// `r"…"`, `r#"…"#`, `b"…"`, `br"…"`, `b'x'`: just past the literal. `None` for an
/// identifier such as `r#type` or `bytes`.
fn prefixed_literal_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    if b[j] == b'b' {
        j += 1;
        match b.get(j) {
            Some(b'\'') => return char_end(b, j),
            Some(b'"') => return Some(string_end(b, j + 1)),
            Some(b'r') => {}
            _ => return None,
        }
    }
    j += 1; // the `r`
    let hashes = b[j..].iter().take_while(|&&c| c == b'#').count();
    j += hashes;
    if b.get(j) != Some(&b'"') {
        return None;
    }
    j += 1;
    while j < b.len() {
        if b[j] == b'"'
            && b[j + 1..]
                .iter()
                .take(hashes)
                .filter(|&&c| c == b'#')
                .count()
                == hashes
        {
            return Some(j + 1 + hashes);
        }
        j += 1;
    }
    Some(b.len())
}

/// The value of a Rust string literal (plain or raw), or `None` if `expr` is not one.
pub(crate) fn rust_string(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if let Some(rest) = expr.strip_prefix('r') {
        let hashes = rest.bytes().take_while(|&c| c == b'#').count();
        let body = rest[hashes..].strip_prefix('"')?;
        let body = body.strip_suffix(&"#".repeat(hashes))?.strip_suffix('"')?;
        return Some(body.to_owned());
    }
    let body = expr.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '0' => out.push('\0'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            '\n' => {
                // a line continuation drops the newline and the next line's indentation
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

/// The index of the bracket that closes the one opened at `open`.
fn rust_closing(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        if let Some(next) = rust_opaque(b, i) {
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

fn skip_space_back(b: &[u8], mut at: usize) -> usize {
    while at > 0 && is_space(b[at - 1]) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    const METRICS: &str = r#"use std::time::Duration;

pub(super) const NUM_POLLERS_NAME: &str = "num_pollers";
/// A doc comment mentioning MetricParameters { name: "nope".into() }
pub const SCHED_NAME: &str =
    "workflow_task_schedule_to_start_latency";

fn lifetimes<'a>(x: &'a str) -> &'a str { x }

impl Instruments {
    fn new(meter: &TemporalMeter) -> Self {
        let counter_with_in_mem = |params: MetricParameters| -> Counter {
            meter.counter(params)
        };
        Self {
            completed: meter.counter(MetricParameters {
                name: "workflow_completed".into(),
                description: "Count of successfully completed workflows".into(),
                unit: "".into(),
            }),
            pollers: meter.gauge(MetricParameters {
                name: NUM_POLLERS_NAME.into(),
                description: "Current number of active pollers per queue type".into(),
                unit: "".into(),
            }),
            sched: meter.histogram_duration(MetricParameters {
                name: SCHED_NAME.into(),
                unit: "duration".into(),
                description:
                    "Histogram of workflow task schedule-to-start latencies"
                        .into(),
            }),
            hits: counter_with_in_mem(MetricParameters {
                name: "sticky_cache_hit".into(),
                description: r"Count of hits".into(),
                unit: "".into(),
            }),
        }
    }
}

fn tuner(meter: &TemporalMeter) {
    let mem = meter.gauge_f64("resource_slots_mem_usage".into());
    let c = '"';
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        let x = meter.counter(MetricParameters { name: "test_only".into(), description: "".into(), unit: "".into() });
    }
}
"#;

    fn checkout() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("crates/sdk-core/src/telemetry/metrics.rs");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, METRICS).expect("write");
        dir
    }

    #[test]
    fn registrations_are_read_with_their_kinds() {
        let dir = checkout();
        let scan = scan(dir.path()).expect("scan");
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
        let found: Vec<(&str, Kind)> = scan
            .metrics
            .iter()
            .map(|m| (m.name.as_str(), m.kind))
            .collect();
        assert_eq!(
            found,
            [
                ("num_pollers", Kind::Gauge),
                ("resource_slots_mem_usage", Kind::Gauge),
                ("sticky_cache_hit", Kind::Counter),
                ("workflow_completed", Kind::Counter),
                ("workflow_task_schedule_to_start_latency", Kind::Duration),
            ]
        );
        let sched = &scan.metrics[4];
        // A constant name cites the constant's literal, on the line after its declaration.
        assert_eq!(sched.line, 6);
        assert_eq!(
            sched.excerpt,
            "\"workflow_task_schedule_to_start_latency\";"
        );
        assert_eq!(
            sched.description.as_deref(),
            Some("Histogram of workflow task schedule-to-start latencies")
        );
        let completed = &scan.metrics[3];
        assert_eq!(completed.excerpt, "name: \"workflow_completed\".into(),");
        assert_eq!(
            scan.metrics[2].description.as_deref(),
            Some("Count of hits")
        );
    }

    #[test]
    fn rust_literals_and_lifetimes_lex() {
        let b = b"'a' 'b> \"x\\\"y\" r#\"q\"# b'z' /* a /* nested */ comment */";
        assert_eq!(rust_opaque(b, 0), Some(3));
        assert_eq!(rust_opaque(b, 4), None, "a lifetime is code");
        assert_eq!(rust_opaque(b, 8), Some(14));
        assert_eq!(rust_opaque(b, 15), Some(21));
        assert_eq!(rust_opaque(b, 22), Some(26));
        assert_eq!(rust_opaque(b, 27), Some(b.len()));
        assert_eq!(rust_string(r#""a\"b""#).as_deref(), Some("a\"b"));
        assert_eq!(rust_string("r#\"x\"y\"#").as_deref(), Some("x\"y"));
    }
}
