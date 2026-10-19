//! SDK metric definitions from the Core (Rust), Go and Java SDK sources.
//!
//! Each SDK defines its metric names in one place and records them elsewhere:
//!
//! * **Core** registers every instrument with a meter call that carries its name, description
//!   and unit: `meter.counter(MetricParameters { name: "workflow_completed".into(), … })`. The
//!   call says the kind. Names are unprefixed; the meter adds `temporal_`.
//! * **Go** builds each name as a constant, `TemporalMetricsPrefix + "workflow_completed"`, in
//!   `internal/common/metrics`. The kind is the handler method that records it: `Counter`,
//!   `Gauge` or `Timer`.
//! * **Java** does the same with `TEMPORAL_METRICS_PREFIX + "workflow_completed"` in its two
//!   `MetricsType` classes, recorded through tally's `counter`, `gauge`, `timer` or `histogram`.
//!
//! For Go and Java a recording can pass the name through a variable first. The scanners follow
//! one assignment in the same function (Go) or file (Java). A name that is referenced but
//! never reaches a handler that way gets its kind from the constant's name (`…Counter`,
//! `…_LATENCY`), and the claim says so. A name nothing refers to is not recorded by that SDK.

pub mod core;
pub mod go;
pub mod java;

use std::path::Path;

use anyhow::Result;

/// An SDK implementation family, as Kyumei's knowledge pack names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Family {
    Core,
    Go,
    Java,
}

impl Family {
    pub const ALL: [Family; 3] = [Family::Core, Family::Go, Family::Java];

    /// The `sdk_family` token.
    pub fn token(self) -> &'static str {
        match self {
            Family::Core => "core",
            Family::Go => "go",
            Family::Java => "java",
        }
    }
}

/// How an SDK records a metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Counter,
    Gauge,
    /// A histogram of durations: Core's `histogram_duration`, a Go or Java timer.
    Duration,
    /// A histogram of other values.
    Histogram,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Counter => "counter",
            Kind::Gauge => "gauge",
            Kind::Duration => "duration histogram",
            Kind::Histogram => "histogram",
        }
    }
}

/// Where a metric's kind was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KindBasis {
    /// The call that records it.
    Recording,
    /// The constant's name, because every recording passes the name through code the scanner
    /// does not follow. Holds the constant.
    ConstantName(String),
}

/// One metric one SDK defines and records.
#[derive(Clone, Debug)]
pub struct SdkMetric {
    pub family: Family,
    /// The name without the `temporal_` prefix.
    pub name: String,
    pub kind: Kind,
    pub kind_basis: KindBasis,
    /// The description the source gives (Core only).
    pub description: Option<String>,
    /// The file and line of the name's string literal, and that line's trimmed text.
    pub path: String,
    pub line: usize,
    pub excerpt: String,
}

/// What one SDK's scan found.
#[derive(Debug, Default)]
pub struct Scan {
    pub metrics: Vec<SdkMetric>,
    pub files_scanned: usize,
    /// Names the SDK defines but never refers to again.
    pub unrecorded: Vec<String>,
    pub warnings: Vec<String>,
}

/// The metrics of every SDK, scanned from their checkouts.
#[derive(Debug, Default)]
pub struct Generated {
    pub scans: Vec<(Family, Scan)>,
}

impl Generated {
    /// Every metric, by name, with the families that record it in family order.
    pub fn by_name(&self) -> std::collections::BTreeMap<&str, Vec<&SdkMetric>> {
        let mut out: std::collections::BTreeMap<&str, Vec<&SdkMetric>> = Default::default();
        for (_, scan) in &self.scans {
            for m in &scan.metrics {
                out.entry(m.name.as_str()).or_default().push(m);
            }
        }
        for metrics in out.values_mut() {
            metrics.sort_by_key(|m| m.family);
        }
        out
    }
}

/// Scan whichever checkouts are given.
pub fn generate(core: Option<&Path>, go: Option<&Path>, java: Option<&Path>) -> Result<Generated> {
    let mut scans = Vec::new();
    if let Some(root) = core {
        scans.push((Family::Core, core::scan(root)?));
    }
    if let Some(root) = go {
        scans.push((Family::Go, go::scan(root)?));
    }
    if let Some(root) = java {
        scans.push((Family::Java, java::scan(root)?));
    }
    Ok(Generated { scans })
}

/// The kind a Go or Java constant's name implies, for names recorded only indirectly.
pub(crate) fn kind_from_constant_name(constant: &str) -> Option<Kind> {
    let upper = constant.to_ascii_uppercase();
    if upper.ends_with("COUNTER") {
        Some(Kind::Counter)
    } else if upper.ends_with("LATENCY") {
        Some(Kind::Duration)
    } else {
        None
    }
}

/// Strip the `temporal_` prefix Go and Java write into their names.
pub(crate) fn unprefixed(name: &str) -> Option<&str> {
    name.strip_prefix("temporal_").filter(|n| !n.is_empty())
}

/// The 1-based line holding byte `at` of `src`.
pub(crate) fn line_of(src: &str, at: usize) -> usize {
    src[..at.min(src.len())]
        .bytes()
        .filter(|&c| c == b'\n')
        .count()
        + 1
}

/// The trimmed text of 1-based line `n` of `src`.
pub(crate) fn line_text(src: &str, n: usize) -> String {
    src.lines()
        .nth(n.saturating_sub(1))
        .unwrap_or("")
        .trim()
        .to_owned()
}

/// Files under `dir` with extension `ext`, sorted, skipping hidden directories and any
/// directory named in `skip`.
pub(crate) fn files_with_extension(
    dir: &Path,
    ext: &str,
    skip: &[&str],
    out: &mut Vec<std::path::PathBuf>,
) -> Result<()> {
    use anyhow::Context as _;
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .collect::<Result<_, _>>()
        .with_context(|| format!("reading {}", dir.display()))?;
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if !name.starts_with('.') && !skip.contains(&name.as_ref()) {
                files_with_extension(&entry.path(), ext, skip, out)?;
            }
        } else if kind.is_file() && name.ends_with(ext) {
            out.push(entry.path());
        }
    }
    Ok(())
}
