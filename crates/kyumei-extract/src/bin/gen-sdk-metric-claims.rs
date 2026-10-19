//! Generate Kyumei's SDK metric claims from the Core, Go and Java SDK sources.
//!
//! ```text
//! cargo run -p kyumei-extract --bin gen-sdk-metric-claims -- \
//!     --core ../sdk-rust --go "$(go env GOMODCACHE)/go.temporal.io/sdk@v1.44.0" \
//!     --java ../sdk-java -o knowledge/temporal/1.31/generated/sdk-metrics.toml
//! ```
//!
//! The default source IDs and revisions are the ones the knowledge pack pins; pass others
//! when generating from different revisions.

use std::{path::PathBuf, process::ExitCode};

use anyhow::Context as _;
use clap::Parser;
use kyumei_extract::{
    claims,
    sdk_claims::{self, FamilySource, SdkClaimOptions},
    sdk_gen,
};

/// Generate Kyumei `[[metric]]` claims for SDK metrics (names, kinds, Core's descriptions,
/// per-SDK provenance and Prometheus expositions) from the SDK sources.
#[derive(Parser)]
#[command(name = "gen-sdk-metric-claims", version)]
struct Args {
    /// Rust SDK (Core) checkout
    #[arg(long, value_name = "DIR")]
    core: PathBuf,
    /// Go SDK module (a checkout, or go.temporal.io/sdk@vX in the module cache)
    #[arg(long, value_name = "DIR")]
    go: PathBuf,
    /// Java SDK checkout
    #[arg(long, value_name = "DIR")]
    java: PathBuf,
    /// Write the claims to this file instead of stdout
    #[arg(short, long, value_name = "FILE", conflicts_with = "check")]
    output: Option<PathBuf>,
    /// Compare with an existing claims file instead of writing; exit 1 when it is out of date
    #[arg(long, value_name = "FILE")]
    check: Option<PathBuf>,
    /// Verification date [default: today, or the checked file's date with --check]
    #[arg(long, value_name = "YYYY-MM-DD")]
    verified_on: Option<String>,
    #[arg(long, value_name = "ID", default_value = "sdk-core-acac9e3f")]
    core_source: String,
    #[arg(long, value_name = "TEXT", default_value = "sdk-rust acac9e3f")]
    core_revision: String,
    #[arg(long, value_name = "ID", default_value = "sdk-go-v1.44.0")]
    go_source: String,
    #[arg(long, value_name = "TEXT", default_value = "sdk-go v1.44.0")]
    go_revision: String,
    #[arg(long, value_name = "ID", default_value = "sdk-java-v1.40.0")]
    java_source: String,
    #[arg(long, value_name = "TEXT", default_value = "sdk-java v1.40.0")]
    java_revision: String,
}

fn main() -> anyhow::Result<ExitCode> {
    let args = Args::parse();
    let g = sdk_gen::generate(Some(&args.core), Some(&args.go), Some(&args.java))?;
    for (family, scan) in &g.scans {
        for w in &scan.warnings {
            eprintln!("warning: {}: {w}", family.token());
        }
        eprintln!(
            "{}: {} metrics from {} files",
            family.token(),
            scan.metrics.len(),
            scan.files_scanned
        );
        if !scan.unrecorded.is_empty() {
            eprintln!(
                "  defined but never recorded: {}",
                scan.unrecorded.join(", ")
            );
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
    let options = SdkClaimOptions {
        verified_on,
        core: FamilySource {
            source_id: args.core_source,
            revision: args.core_revision,
        },
        go: FamilySource {
            source_id: args.go_source,
            revision: args.go_revision,
        },
        java: FamilySource {
            source_id: args.java_source,
            revision: args.java_revision,
        },
    };
    let text = sdk_claims::render(&g, &options)?;
    eprintln!("{} claims", g.by_name().len());

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
