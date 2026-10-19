//! Server metric definitions, and the code that records them.
//!
//! Temporal defines each server metric once, with a constructor from `common/metrics/defs.go`:
//!
//! ```go
//! ServiceRequests = NewCounterDef(
//!     "service_requests",
//!     WithDescription("The number of RPC requests received by the service."),
//! )
//! ```
//!
//! Tags are attached where a metric is recorded, not where it is defined, so the generator
//! makes two passes over a checkout with the lexer of [`crate::registry_gen`]:
//!
//! 1. **Definitions.** Every `New{Counter,Gauge,Timer,BytesHistogram,DimensionlessHistogram}Def(`
//!    call: the identifier it is assigned to, the metric's name and kind, its
//!    `WithDescription` text, and the line of the name literal.
//! 2. **Emitters.** Every reference to a definition outside the definition itself: a selector
//!    through an import of the defining package (`metrics.ServiceRequests`), or a bare
//!    identifier inside that package. Each reference names a package that records the metric.
//!    The tag helpers called in the same statement are kept as candidates for its labels.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context as _, Result, bail, ensure};

use crate::registry_gen::{
    collect_go_files, detect_version, is_func_decl, is_ident, is_space, one_line, relative,
    skip_opaque, split_args, string_const,
};

/// A definition constructor, `New<Kind>Def`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Counter,
    Gauge,
    Timer,
    BytesHistogram,
    DimensionlessHistogram,
}

impl Kind {
    const ALL: [Kind; 5] = [
        Kind::Counter,
        Kind::Gauge,
        Kind::Timer,
        Kind::BytesHistogram,
        Kind::DimensionlessHistogram,
    ];

    /// The `<Kind>` part of the constructor's name.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Counter => "Counter",
            Kind::Gauge => "Gauge",
            Kind::Timer => "Timer",
            Kind::BytesHistogram => "BytesHistogram",
            Kind::DimensionlessHistogram => "DimensionlessHistogram",
        }
    }

    /// The unit the constructor gives a definition (`common/metrics/defs.go`).
    fn implied_unit(self) -> Option<&'static str> {
        match self {
            Kind::BytesHistogram => Some("By"),
            Kind::DimensionlessHistogram => Some("1"),
            _ => None,
        }
    }
}

/// The `MetricUnit` constants of `common/metrics/consts.go`.
const UNITS: [(&str, &str); 4] = [
    ("Dimensionless", "1"),
    ("Milliseconds", "ms"),
    ("Bytes", "By"),
    ("Seconds", "s"),
];

/// One metric definition.
#[derive(Clone, Debug)]
pub struct Definition {
    pub name: String,
    pub kind: Kind,
    /// The identifier the definition is assigned to, e.g. `ServiceRequests`.
    pub ident: String,
    /// The defining file's package name, e.g. `metrics`.
    pub package: String,
    /// The defining package's directory in the checkout, e.g. `common/metrics`.
    pub dir: String,
    /// The `WithDescription` text, on one line.
    pub description: Option<String>,
    /// The unit as Temporal writes it (`By`, `1`, `ms`, `s`): implied by the constructor or
    /// set with `WithUnit`.
    pub unit: Option<String>,
    /// The file and line of the name literal, and that line's trimmed text.
    pub path: String,
    pub line: usize,
    pub excerpt: String,
    /// Where the definition is referenced, in path and line order.
    pub references: Vec<Reference>,
    /// Byte offset of the assigned identifier in `path`, so the pass over references can
    /// skip the definition itself.
    ident_at: usize,
}

impl Definition {
    /// The definition as Go code outside its package refers to it, e.g. `metrics.ServiceRequests`.
    pub fn go_name(&self) -> String {
        format!("{}.{}", self.package, self.ident)
    }

    /// The package directories that reference the definition, sorted and without repeats.
    pub fn recorded_in(&self) -> Vec<&str> {
        let mut dirs: Vec<&str> = self.references.iter().map(Reference::dir).collect();
        dirs.sort_unstable();
        dirs.dedup();
        dirs
    }
}

/// A reference to a definition: where a metric is recorded.
#[derive(Clone, Debug)]
pub struct Reference {
    pub path: String,
    pub line: usize,
    /// Tag helpers called in the same statement, e.g. `metrics.OperationTag`. Tags attached
    /// earlier, with a handler's `WithTags`, are not seen.
    pub tags: Vec<String>,
}

impl Reference {
    /// The directory of the referencing file: its Go package.
    pub fn dir(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(dir, _)| dir)
    }
}

/// The definitions of a checkout, plus diagnostics for whoever runs the generator.
#[derive(Debug)]
pub struct Generated {
    pub temporal_version: String,
    /// Sorted by name.
    pub metrics: Vec<Definition>,
    pub files_scanned: usize,
    /// Definitions the generator could not read, and other surprises.
    pub warnings: Vec<String>,
}

/// Find every metric definition in a Temporal server source checkout, and every reference to
/// one. `version` overrides the `ServerVersion` found in the source.
pub fn generate(root: &Path, version: Option<String>) -> Result<Generated> {
    ensure!(
        root.join("common/metrics").is_dir(),
        "{} is not a Temporal server checkout (it has no common/metrics)",
        root.display()
    );
    let temporal_version = match version {
        Some(v) => v,
        None => detect_version(root)
            .context("no ServerVersion in the checkout; pass --temporal-version")?,
    };
    let module = go_module(root)?;
    let mut paths = Vec::new();
    collect_go_files(root, &mut paths)?;
    paths.sort();
    let mut files = Vec::with_capacity(paths.len());
    for path in &paths {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        files.push(GoFile::new(
            relative(root, path),
            String::from_utf8_lossy(&bytes).into_owned(),
        ));
    }

    let mut warnings = Vec::new();
    let mut metrics = Vec::new();
    for file in files.iter().filter(|f| f.src.contains("Def(")) {
        metrics.extend(file.definitions(&mut warnings));
    }
    let mut by_name: BTreeMap<&str, &Definition> = BTreeMap::new();
    for d in &metrics {
        if let Some(prev) = by_name.insert(&d.name, d) {
            // The server refuses to start with a duplicate (common/metrics/registry.go), so
            // this is the generator misreading something.
            bail!(
                "metric {} is defined twice: {}:{} and {}:{}",
                d.name,
                prev.path,
                prev.line,
                d.path,
                d.line
            );
        }
    }

    // Package names by directory, for imports without an alias.
    let packages: HashMap<&str, &str> = files
        .iter()
        .filter_map(|f| Some((f.dir(), f.package.as_deref()?)))
        .collect();
    // Definitions by package directory and identifier.
    let mut index: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (i, d) in metrics.iter().enumerate() {
        index
            .entry(d.dir.clone())
            .or_default()
            .insert(d.ident.clone(), i);
    }
    for file in &files {
        let imports = file.imports(&module, &packages);
        for (i, at) in file.references(&imports, &index) {
            let d = &mut metrics[i];
            if d.path == file.rel && d.ident_at == at {
                continue;
            }
            let line = file.line_of(at);
            d.references.push(Reference {
                path: file.rel.clone(),
                line,
                tags: file.tags_in_statement(at),
            });
        }
    }
    for d in &mut metrics {
        d.references
            .sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        d.references
            .dedup_by(|a, b| a.path == b.path && a.line == b.line);
    }
    metrics.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Generated {
        temporal_version,
        metrics,
        files_scanned: files.len(),
        warnings,
    })
}

/// The module path in `go.mod`, e.g. `go.temporal.io/server`.
pub(crate) fn go_module(root: &Path) -> Result<String> {
    let text = std::fs::read_to_string(root.join("go.mod")).context("reading go.mod")?;
    text.lines()
        .find_map(|l| l.trim().strip_prefix("module "))
        .map(|m| m.trim().trim_matches('"').to_owned())
        .context("go.mod names no module")
}

/// One non-test Go file of the checkout.
pub(crate) struct GoFile {
    /// Path relative to the checkout, with `/` separators.
    pub(crate) rel: String,
    pub(crate) src: String,
    pub(crate) package: Option<String>,
    line_starts: Vec<usize>,
}

impl GoFile {
    pub(crate) fn new(rel: String, src: String) -> Self {
        let package = src
            .lines()
            .find_map(|l| l.strip_prefix("package "))
            .and_then(|rest| rest.split_whitespace().next())
            .map(str::to_owned);
        let line_starts = std::iter::once(0)
            .chain(src.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self {
            rel,
            src,
            package,
            line_starts,
        }
    }

    pub(crate) fn dir(&self) -> &str {
        self.rel.rsplit_once('/').map_or("", |(dir, _)| dir)
    }

    /// The 1-based line holding byte `at`.
    pub(crate) fn line_of(&self, at: usize) -> usize {
        self.line_starts.partition_point(|&s| s <= at)
    }

    /// The text of 1-based line `n`, without its newline.
    pub(crate) fn line_text(&self, n: usize) -> Option<&str> {
        let start = *self.line_starts.get(n.checked_sub(1)?)?;
        let end = self
            .line_starts
            .get(n)
            .map_or(self.src.len(), |&next| next - 1);
        Some(self.src[start..end].trim_end_matches('\r'))
    }

    /// The metric definitions in this file. Constructor calls the generator cannot read
    /// become warnings.
    fn definitions(&self, warnings: &mut Vec<String>) -> Vec<Definition> {
        let b = self.src.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if let Some(next) = skip_opaque(b, i) {
                i = next;
                continue;
            }
            if b[i] == b'N'
                && (i == 0 || !is_ident(b[i - 1]))
                && let Some((kind, args_start)) = match_constructor(b, i)
            {
                // skip the constructors' own declarations in defs.go
                if !is_func_decl(&self.src, i) {
                    match self.definition(i, kind, args_start) {
                        Ok(d) => out.push(d),
                        Err(e) => warnings.push(format!("{}:{}: {e:#}", self.rel, self.line_of(i))),
                    }
                }
                i = args_start;
                continue;
            }
            i += 1;
        }
        out
    }

    /// Read the constructor call at `at`, whose arguments start at `args_start`.
    fn definition(&self, at: usize, kind: Kind, args_start: usize) -> Result<Definition> {
        let args = split_args(&self.src, args_start).context("the call is not closed")?;
        let first = args.first().context("the call has no arguments")?;
        let name = string_const(first)
            .with_context(|| format!("the name `{first}` is not a string constant"))?;
        let mut description = None;
        let mut unit = kind.implied_unit().map(str::to_owned);
        for option in &args[1..] {
            let (func, inner) =
                option_call(option).with_context(|| format!("unrecognised option `{option}`"))?;
            match func {
                "WithDescription" => {
                    let text = string_const(inner).with_context(|| {
                        format!("the description `{inner}` is not a string constant")
                    })?;
                    description = Some(one_line(&text));
                }
                "WithUnit" => {
                    unit = Some(
                        unit_value(inner)
                            .with_context(|| format!("unrecognised unit `{inner}`"))?,
                    );
                }
                other => bail!("unrecognised option `{other}`"),
            }
        }
        if kind == Kind::Timer && unit.is_some() {
            // The handlers set a timer's unit themselves (tally_metrics_handler.go,
            // otel_metrics_handler.go); a declared one would change the OpenTelemetry name.
            bail!("timer {name} declares a unit, which the exposition rules do not cover");
        }
        let src = self.src.as_bytes();
        let ident_at = assigned_ident(src, at).context("not assigned to a variable")?;
        let ident = self.src[ident_at..ident_end(src, ident_at)].to_owned();
        let package = self
            .package
            .clone()
            .context("the file has no package clause")?;
        // Cite the line of the name literal, as dynamic config claims cite the key.
        let first_line = self.line_of(at);
        let quoted = [format!("\"{name}\""), format!("`{name}`")];
        let (line, excerpt) = (first_line..first_line + 6)
            .filter_map(|n| Some((n, self.line_text(n)?)))
            .find(|(_, text)| quoted.iter().any(|q| text.contains(q.as_str())))
            .map(|(n, text)| (n, text.trim().to_owned()))
            .with_context(|| format!("the literal {:?} is not near the constructor", name))?;
        Ok(Definition {
            name,
            kind,
            ident,
            package,
            dir: self.dir().to_owned(),
            description,
            unit,
            path: self.rel.clone(),
            line,
            excerpt,
            references: Vec::new(),
            ident_at,
        })
    }

    /// Import names in this file that refer to packages of `module`, mapped to the package's
    /// directory. The name is the alias, or the imported package's own name. A dot import
    /// maps from `.`.
    pub(crate) fn imports(
        &self,
        module: &str,
        packages: &HashMap<&str, &str>,
    ) -> HashMap<String, String> {
        let mut out = HashMap::new();
        let mut add = |spec: &str| {
            let spec = spec.trim();
            let (alias, path) = match spec.split_once(char::is_whitespace) {
                Some((alias, path)) if !spec.starts_with('"') => (Some(alias), path.trim()),
                _ => (None, spec),
            };
            let Some(dir) = path
                .trim_matches('"')
                .strip_prefix(module)
                .and_then(|p| p.strip_prefix('/'))
            else {
                return;
            };
            let name = match alias {
                Some("_") => return,
                Some(alias) => alias.to_owned(),
                None => packages
                    .get(dir)
                    .map_or_else(|| dir.rsplit('/').next().unwrap_or(dir), |p| p)
                    .to_owned(),
            };
            out.insert(name, dir.to_owned());
        };
        let mut in_block = false;
        for line in self.src.lines() {
            let line = line.split("//").next().unwrap_or("").trim();
            if in_block {
                if line.starts_with(')') {
                    in_block = false;
                } else if !line.is_empty() {
                    add(line);
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("import") {
                let rest = rest.trim_start();
                match rest.strip_prefix('(') {
                    Some(after) => {
                        in_block = true;
                        if !after.trim().is_empty() {
                            add(after);
                        }
                    }
                    None => add(rest),
                }
            } else if ["func ", "type ", "var ", "const "]
                .iter()
                .any(|kw| line.starts_with(kw))
            {
                break; // imports come before any declaration
            }
        }
        out
    }

    /// Every reference to a definition: `(definition index, byte offset)`. `imports` maps
    /// this file's import names to package directories; `index` maps package directories to
    /// their definitions by identifier.
    fn references(
        &self,
        imports: &HashMap<String, String>,
        index: &HashMap<String, HashMap<String, usize>>,
    ) -> Vec<(usize, usize)> {
        let b = self.src.as_bytes();
        let local = index.get(self.dir());
        let dotted = imports.get(".").and_then(|dir| index.get(dir));
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if let Some(next) = skip_opaque(b, i) {
                i = next;
                continue;
            }
            if !is_ident(b[i]) {
                i += 1;
                continue;
            }
            let end = ident_end(b, i);
            let word = &self.src[i..end];
            if i > 0 && b[i - 1] == b'.' {
                i = end; // a field or method, not a package-level name
                continue;
            }
            if b.get(end) == Some(&b'.')
                && let Some(defs) = imports.get(word).and_then(|dir| index.get(dir))
            {
                let sel_end = ident_end(b, end + 1);
                if let Some(&d) = defs.get(&self.src[end + 1..sel_end]) {
                    out.push((d, i));
                }
                i = sel_end;
                continue;
            }
            if let Some(&d) = local
                .and_then(|defs| defs.get(word))
                .or_else(|| dotted.and_then(|defs| defs.get(word)))
            {
                out.push((d, i));
            }
            i = end;
        }
        out
    }

    /// Tag helpers called in the statement that holds byte `at`: calls of a function whose
    /// name ends in `Tag`, as written, with the first argument of `StringTag`.
    fn tags_in_statement(&self, at: usize) -> Vec<String> {
        let b = self.src.as_bytes();
        let start = self.line_starts[self.line_of(at) - 1];
        let end = statement_end(b, start);
        let mut tags: Vec<String> = Vec::new();
        let mut i = start;
        while i < end {
            if let Some(next) = skip_opaque(b, i) {
                i = next;
                continue;
            }
            if !is_ident(b[i]) || (i > 0 && is_ident(b[i - 1])) {
                i += 1;
                continue;
            }
            let word_end = ident_end(b, i);
            let word = &self.src[i..word_end];
            let qualified_start = qualifier_start(b, i);
            if word.ends_with("Tag")
                && word.as_bytes()[0].is_ascii_uppercase()
                && b.get(word_end) == Some(&b'(')
            {
                let mut tag = self.src[qualified_start..word_end].to_owned();
                if word == "StringTag"
                    && let Some(args) = split_args(&self.src, word_end + 1)
                    && let Some(first) = args.first()
                {
                    tag = format!("{tag}({})", short(first));
                }
                if !tags.contains(&tag) {
                    tags.push(tag);
                }
            }
            i = word_end;
        }
        tags
    }
}

/// `New<Kind>Def` then `(`, at `at`: the kind and the index just past the `(`.
fn match_constructor(b: &[u8], at: usize) -> Option<(Kind, usize)> {
    let rest = b[at..].strip_prefix(b"New")?;
    for kind in Kind::ALL {
        let Some(rest) = rest
            .strip_prefix(kind.word().as_bytes())
            .and_then(|r| r.strip_prefix(b"Def"))
        else {
            continue;
        };
        let mut k = b.len() - rest.len();
        while k < b.len() && is_space(b[k]) {
            k += 1;
        }
        if b.get(k) == Some(&b'(') {
            return Some((kind, k + 1));
        }
    }
    None
}

/// `WithDescription("…")` or `metrics.WithUnit(Bytes)`: the function's name and its argument,
/// without the trailing comma Go allows after the last argument.
fn option_call(option: &str) -> Option<(&str, &str)> {
    let open = option.find('(')?;
    let inner = option.get(open + 1..)?.strip_suffix(')')?.trim();
    let inner = inner.strip_suffix(',').unwrap_or(inner);
    let func = option[..open].trim();
    let func = func.rsplit_once('.').map_or(func, |(_, f)| f);
    func.bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        .then_some((func, inner))
}

/// The value of a `WithUnit` argument: a string literal or a `MetricUnit` constant.
fn unit_value(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if let Some(s) = string_const(expr) {
        return Some(s);
    }
    let name = expr.rsplit_once('.').map_or(expr, |(_, n)| n);
    UNITS
        .iter()
        .find(|(constant, _)| *constant == name)
        .map(|(_, value)| (*value).to_owned())
}

/// The start of the identifier that the constructor call at `at` is assigned to:
/// `Ident = NewCounterDef(` or `Ident = metrics.NewCounterDef(`, `=` or `:=`.
fn assigned_ident(b: &[u8], at: usize) -> Option<usize> {
    let mut k = at;
    // a package qualifier, `metrics.`
    if k > 0 && b[k - 1] == b'.' {
        k -= 1;
        while k > 0 && is_ident(b[k - 1]) {
            k -= 1;
        }
    }
    while k > 0 && is_space(b[k - 1]) {
        k -= 1;
    }
    if k == 0 || b[k - 1] != b'=' {
        return None;
    }
    k -= 1;
    if k > 0 && b[k - 1] == b':' {
        k -= 1;
    } else if k > 0 && matches!(b[k - 1], b'=' | b'!' | b'<' | b'>') {
        return None; // a comparison, not an assignment
    }
    while k > 0 && is_space(b[k - 1]) {
        k -= 1;
    }
    let end = k;
    while k > 0 && is_ident(b[k - 1]) {
        k -= 1;
    }
    (k < end && !b[k].is_ascii_digit()).then_some(k)
}

/// The index just past the identifier starting at `at`.
pub(crate) fn ident_end(b: &[u8], at: usize) -> usize {
    let mut k = at;
    while k < b.len() && is_ident(b[k]) {
        k += 1;
    }
    k
}

/// The start of `pkg.Name` when the identifier at `at` is qualified, else `at`.
fn qualifier_start(b: &[u8], at: usize) -> usize {
    if at < 2 || b[at - 1] != b'.' || !is_ident(b[at - 2]) {
        return at;
    }
    let mut k = at - 1;
    while k > 0 && is_ident(b[k - 1]) {
        k -= 1;
    }
    k
}

/// The end of the statement that starts at `start`: the first newline outside brackets after
/// a token that ends a Go statement. A line that ends in `.`, `,`, an operator or an opening
/// bracket continues on the next line. At most 40 lines.
fn statement_end(b: &[u8], start: usize) -> usize {
    let mut depth = 0usize;
    let mut last = b';';
    let mut lines = 0;
    let mut i = start;
    while i < b.len() {
        if let Some(next) = skip_opaque(b, i) {
            if b[i] != b'/' {
                last = b'"';
            }
            i = next;
            continue;
        }
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'\n' => {
                lines += 1;
                let continues = matches!(
                    last,
                    b'.' | b','
                        | b'('
                        | b'['
                        | b'{'
                        | b'+'
                        | b'-'
                        | b'*'
                        | b'/'
                        | b'&'
                        | b'|'
                        | b'='
                );
                if (depth == 0 && !continues) || lines >= 40 {
                    return i;
                }
            }
            _ => {}
        }
        if !is_space(b[i]) {
            last = b[i];
        }
        i += 1;
    }
    b.len()
}

/// An argument for a report, on one line and at most 60 characters.
fn short(s: &str) -> String {
    let s = one_line(s);
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(59).collect::<String>())
    } else {
        s
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const METRIC_DEFS: &str = r#"package metrics

// ServiceRequests is a comment that mentions metrics.ServiceLatency, which is not a use.
var (
	ServiceRequests = NewCounterDef(
		"service_requests",
		WithDescription("The number of RPC requests received by the service."),
	)
	ServicePending = NewGaugeDef("service_pending_requests")
	ServiceLatency = NewTimerDef("service_latency")
	BlobSize       = NewBytesHistogramDef("event_blob_size")
	TaskCount      = NewDimensionlessHistogramDef(
		"tasks_per_update",
		WithDescription(
			`Tasks ` +
				"per update.",
		),
	)
	Unused         = NewCounterDef("never_recorded")
)

func NewCounterDef(name string, opts ...Option) counterDefinition { return counterDefinition{} }

func record(h Handler) {
	ServicePending.With(h).Record(1)
}
"#;

    const TELEMETRY: &str = r#"package interceptor

import (
	"context"

	"go.temporal.io/server/common/metrics"
	m2 "go.temporal.io/server/common/metrics"
)

func (ti *TelemetryInterceptor) emit(h metrics.Handler, op string) {
	metrics.ServiceRequests.With(h).Record(
		1,
		metrics.OperationTag(op),
		metrics.StringTag("cause", "x"),
	)
	m2.ServiceLatency.With(h).Record(0) // metrics.Unused here is a comment
	_ = "metrics.Unused in a string"
	_ = ti.metrics.ServiceRequests // a field, not the package
}
"#;

    const MATCHER: &str = r#"package matching

import "go.temporal.io/server/common/metrics"

func (m *matcher) poll() {
	metrics.BlobSize.With(m.handler).Record(10)
	h := m.handler.WithTags(metrics.NamespaceTag("ns"))
	metrics.TaskCount.With(h).Record(1)
}
"#;

    pub(crate) fn checkout() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let write = |rel: &str, text: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, text).expect("write");
        };
        write("go.mod", "module go.temporal.io/server\n\ngo 1.24\n");
        write("common/metrics/metric_defs.go", METRIC_DEFS);
        write("common/rpc/interceptor/telemetry.go", TELEMETRY);
        write("service/matching/matcher.go", MATCHER);
        write(
            "common/headers/version_checker.go",
            "package headers\n\nconst ServerVersion = \"1.31.0\"\n",
        );
        dir
    }

    fn metric<'a>(g: &'a Generated, name: &str) -> &'a Definition {
        g.metrics.iter().find(|m| m.name == name).expect(name)
    }

    #[test]
    fn definitions_are_read_with_their_name_line() {
        let dir = checkout();
        let g = generate(dir.path(), None).expect("generate");
        assert!(g.warnings.is_empty(), "{:?}", g.warnings);
        assert_eq!(g.temporal_version, "1.31.0");
        let names: Vec<&str> = g.metrics.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "event_blob_size",
                "never_recorded",
                "service_latency",
                "service_pending_requests",
                "service_requests",
                "tasks_per_update"
            ]
        );
        let requests = metric(&g, "service_requests");
        assert_eq!(requests.kind, Kind::Counter);
        assert_eq!(requests.go_name(), "metrics.ServiceRequests");
        assert_eq!(requests.dir, "common/metrics");
        assert_eq!(
            requests.description.as_deref(),
            Some("The number of RPC requests received by the service.")
        );
        // The constructor is on line 5; the name literal is on line 6.
        assert_eq!(requests.line, 6);
        assert_eq!(requests.excerpt, "\"service_requests\",");
        let count = metric(&g, "tasks_per_update");
        assert_eq!(count.unit.as_deref(), Some("1"));
        assert_eq!(count.description.as_deref(), Some("Tasks per update."));
        assert_eq!(metric(&g, "event_blob_size").unit.as_deref(), Some("By"));
        assert_eq!(metric(&g, "service_latency").unit, None);
    }

    #[test]
    fn references_name_the_packages_that_record_a_metric() {
        let dir = checkout();
        let g = generate(dir.path(), None).expect("generate");
        let requests = metric(&g, "service_requests");
        assert_eq!(requests.recorded_in(), ["common/rpc/interceptor"]);
        let reference = &requests.references[0];
        assert_eq!(reference.line, 11);
        assert_eq!(
            reference.tags,
            ["metrics.OperationTag", "metrics.StringTag(\"cause\")"]
        );
        // An aliased import is a reference too.
        assert_eq!(
            metric(&g, "service_latency").recorded_in(),
            ["common/rpc/interceptor"]
        );
        // A bare use inside the defining package counts; the definition itself does not.
        assert_eq!(
            metric(&g, "service_pending_requests").recorded_in(),
            ["common/metrics"]
        );
        // Tags attached earlier, with WithTags, are not in the recording statement.
        let count = metric(&g, "tasks_per_update");
        assert_eq!(count.recorded_in(), ["service/matching"]);
        assert!(count.references[0].tags.is_empty());
        // Comments, strings and fields are not uses.
        assert!(metric(&g, "never_recorded").references.is_empty());
    }

    #[test]
    fn unreadable_definitions_are_reported() {
        let dir = checkout();
        std::fs::write(
            dir.path().join("common/metrics/extra.go"),
            "package metrics\n\nvar Dynamic = NewCounterDef(name)\nvar Odd = NewGaugeDef(\"odd\", WithSomething(1))\n",
        )
        .expect("write");
        let g = generate(dir.path(), None).expect("generate");
        assert_eq!(g.warnings.len(), 2, "{:?}", g.warnings);
        assert!(g.warnings[0].contains("the name `name` is not a string constant"));
        assert!(g.warnings[1].contains("unrecognised option `WithSomething`"));
    }

    #[test]
    fn a_duplicate_name_stops_the_generator() {
        let dir = checkout();
        std::fs::write(
            dir.path().join("common/metrics/extra.go"),
            "package metrics\n\nvar Again = NewCounterDef(\"service_requests\")\n",
        )
        .expect("write");
        let err = generate(dir.path(), None).expect_err("duplicate");
        assert!(err.to_string().contains("defined twice"), "{err}");
    }
}
