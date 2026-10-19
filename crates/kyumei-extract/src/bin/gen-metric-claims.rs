//! Generate Kyumei's server metric claims from a Temporal server source checkout.
//!
//! ```text
//! git clone --depth 1 --branch v1.31.0 https://github.com/temporalio/temporal ../temporal
//! cargo run -p kyumei-extract --bin gen-metric-claims -- ../temporal \
//!     -o knowledge/temporal/1.31/generated/server-metrics.toml
//! cargo run -p kyumei-extract --bin gen-metric-claims -- ../temporal \
//!     --check knowledge/temporal/1.31/generated/server-metrics.toml
//! cargo run -p kyumei-extract --bin gen-metric-claims -- ../temporal --emitters emitters.md
//! ```
//!
//! With `--sweep <checkout>` for each other release, each claim is scoped to every release
//! where it is the same (see `kyumei_extract::sweep`).

use std::{path::PathBuf, process::ExitCode};

use anyhow::Context as _;
use clap::Parser;
use kyumei_extract::{
    claims::{self, ClaimOptions},
    metric_claims, metrics_gen, sweep,
};

/// Generate Kyumei `[[metric]]` claims (every server metric's name, kind, description,
/// recording packages and Prometheus expositions) from a Temporal server source checkout.
#[derive(Parser)]
#[command(name = "gen-metric-claims", version)]
struct Args {
    /// Temporal server source checkout (github.com/temporalio/temporal at a release tag)
    source: PathBuf,
    /// Write the claims to this file instead of stdout
    #[arg(short, long, value_name = "FILE", conflicts_with = "check")]
    output: Option<PathBuf>,
    /// Compare with an existing claims file instead of writing; exit 1 when it is out of date
    #[arg(long, value_name = "FILE")]
    check: Option<PathBuf>,
    /// Also write a Markdown report of where each metric is recorded, with the tag helpers
    /// called there, for reviewing label sets
    #[arg(long, value_name = "FILE")]
    emitters: Option<PathBuf>,
    /// Temporal version to record [default: ServerVersion in common/headers/version_checker.go]
    #[arg(long, value_name = "VERSION")]
    temporal_version: Option<String>,
    /// Kyumei source ID the provenance cites [default: temporal-server-v<VERSION>]
    #[arg(long, value_name = "ID")]
    source_id: Option<String>,
    /// Verification date [default: today, or the checked file's date with --check]
    #[arg(long, value_name = "YYYY-MM-DD")]
    verified_on: Option<String>,
    /// Another release's checkout. Each claim is then scoped to every release where it is the
    /// same, and cites each one (the version sweep). Repeatable.
    #[arg(long, value_name = "DIR")]
    sweep: Vec<PathBuf>,
}

fn main() -> anyhow::Result<ExitCode> {
    let args = Args::parse();
    let g = metrics_gen::generate(&args.source, args.temporal_version)?;
    for w in &g.warnings {
        eprintln!("warning: {w}");
    }
    let unrecorded = g.metrics.iter().filter(|m| m.references.is_empty()).count();
    eprintln!(
        "{} metrics for Temporal {} from {} Go files; {unrecorded} are never referenced",
        g.metrics.len(),
        g.temporal_version,
        g.files_scanned
    );
    for (name, metrics) in metric_claims::shared_names(&g)? {
        eprintln!(
            "warning: {} can export `{name}`: evidence under that name is ambiguous",
            metrics.join(" and ")
        );
    }
    if let Some(path) = &args.emitters {
        std::fs::write(path, metric_claims::emitters_report(&g))
            .with_context(|| format!("writing {}", path.display()))?;
        eprintln!("wrote {}", path.display());
    }

    let existing = match &args.check {
        Some(path) => Some(
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?,
        ),
        None => None,
    };
    let verified_on = match (&args.verified_on, &existing) {
        (Some(date), _) => date.clone(),
        (None, Some(text)) => {
            claims::verified_on(text).context("the checked file has no claims")?
        }
        (None, None) => jiff::Zoned::now().date().to_string(),
    };
    let options = ClaimOptions {
        source_id: args
            .source_id
            .unwrap_or_else(|| format!("temporal-server-v{}", g.temporal_version)),
        verified_on,
    };
    let mut text = metric_claims::render(&g, &options)?;
    if !args.sweep.is_empty() {
        let mut releases = vec![sweep::Release {
            version: g.temporal_version.clone(),
            source_id: options.source_id.clone(),
            rendered: text,
        }];
        for dir in &args.sweep {
            let other = metrics_gen::generate(dir, None)
                .with_context(|| format!("generating from {}", dir.display()))?;
            let opts = ClaimOptions {
                source_id: format!("temporal-server-v{}", other.temporal_version),
                verified_on: options.verified_on.clone(),
            };
            eprintln!(
                "{} metrics for Temporal {} (sweep)",
                other.metrics.len(),
                other.temporal_version
            );
            releases.push(sweep::Release {
                version: other.temporal_version.clone(),
                rendered: metric_claims::render(&other, &opts)?,
                source_id: opts.source_id,
            });
        }
        text = sweep::merge(sweep::Kind::Metric, &g.temporal_version, &releases)?;
    }

    if let (Some(path), Some(old)) = (&args.check, &existing) {
        if *old == text {
            eprintln!("{} is up to date", path.display());
            return Ok(ExitCode::SUCCESS);
        }
        let changes = claims::diff(old, &text)?;
        for c in &changes {
            println!("{c}");
        }
        eprintln!(
            "{} is out of date ({}); regenerate it with -o",
            path.display(),
            if changes.is_empty() {
                "formatting only".to_owned()
            } else {
                format!("{} differences", changes.len())
            }
        );
        return Ok(ExitCode::FAILURE);
    }

    match &args.output {
        Some(path) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("creating {}", dir.display()))?;
            }
            std::fs::write(path, &text).with_context(|| format!("writing {}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
        None => print!("{text}"),
    }
    Ok(ExitCode::SUCCESS)
}
