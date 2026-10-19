//! Generate Kyumei's dynamic config claims from a Temporal server source checkout.
//!
//! ```text
//! git clone --depth 1 --branch v1.31.0 https://github.com/temporalio/temporal ../temporal
//! cargo run -p kyumei-extract --bin gen-dc-claims -- ../temporal \
//!     -o knowledge/temporal/1.31/generated/dynamic-config.toml
//! cargo run -p kyumei-extract --bin gen-dc-claims -- ../temporal \
//!     --check knowledge/temporal/1.31/generated/dynamic-config.toml
//! ```
//!
//! With `--sweep <checkout>` for each other release, each claim is scoped to every release
//! where it is the same (see `kyumei_extract::sweep`).
//!
//! Adapted from tempdes' `gen-dc-registry`, which writes the same registry as JSON.

use std::{path::PathBuf, process::ExitCode};

use anyhow::Context as _;
use clap::Parser;
use kyumei_extract::{
    claims::{self, ClaimOptions},
    registry_gen, sweep,
};

/// Generate Kyumei `[[dynamic_config]]` claims (every setting's key, scope, type, default,
/// description and source line) from a Temporal server source checkout.
#[derive(Parser)]
#[command(name = "gen-dc-claims", version)]
struct Args {
    /// Temporal server source checkout (github.com/temporalio/temporal at a release tag)
    source: PathBuf,
    /// Write the claims to this file instead of stdout
    #[arg(short, long, value_name = "FILE", conflicts_with = "check")]
    output: Option<PathBuf>,
    /// Compare with an existing claims file instead of writing; exit 1 when it is out of date
    #[arg(long, value_name = "FILE")]
    check: Option<PathBuf>,
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
    let g = registry_gen::generate(&args.source, args.temporal_version)?;
    for w in &g.warnings {
        eprintln!("warning: {w}");
    }
    eprintln!(
        "{} settings for Temporal {} from {} Go files",
        g.registry.settings.len(),
        g.registry.temporal_version,
        g.files_scanned
    );
    if !g.unevaluated.is_empty() {
        eprintln!(
            "{} defaults are not constant expressions and are recorded as Go source:",
            g.unevaluated.len()
        );
        for u in &g.unevaluated {
            eprintln!("  {u}");
        }
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
            .unwrap_or_else(|| format!("temporal-server-v{}", g.registry.temporal_version)),
        verified_on,
    };
    let mut text = claims::render(&g.registry, &args.source, &options)?;
    if !args.sweep.is_empty() {
        let mut releases = vec![sweep::Release {
            version: g.registry.temporal_version.clone(),
            source_id: options.source_id.clone(),
            rendered: text,
        }];
        for dir in &args.sweep {
            let other = registry_gen::generate(dir, None)
                .with_context(|| format!("generating from {}", dir.display()))?;
            let opts = ClaimOptions {
                source_id: format!("temporal-server-v{}", other.registry.temporal_version),
                verified_on: options.verified_on.clone(),
            };
            eprintln!(
                "{} settings for Temporal {} (sweep)",
                other.registry.settings.len(),
                other.registry.temporal_version
            );
            releases.push(sweep::Release {
                version: other.registry.temporal_version.clone(),
                rendered: claims::render(&other.registry, dir, &opts)?,
                source_id: opts.source_id,
            });
        }
        text = sweep::merge(
            sweep::Kind::DynamicConfig,
            &g.registry.temporal_version,
            &releases,
        )?;
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
