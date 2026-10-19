//! The committed generated claims are exactly what the generators write for their sources.
//! Set `KYUMEI_TEMPORAL_SOURCE` to a Temporal v1.31.0 checkout and `KYUMEI_TEMPORAL_SWEEP` to
//! the checkouts of the other swept releases, separated by `;`, and `KYUMEI_SDK_SOURCES` to
//! `core=PATH;go=PATH;java=PATH` for the pinned SDKs, to run the checks; without them the tests
//! report that they skipped.

use std::path::{Path, PathBuf};

use kyumei_extract::{
    claims::{self, ClaimOptions},
    metric_claims, metrics_gen, registry_gen,
    sdk_claims::{self, FamilySource, SdkClaimOptions},
    sdk_gen, sweep,
};

fn source() -> Option<PathBuf> {
    let source = std::env::var("KYUMEI_TEMPORAL_SOURCE").ok();
    if source.is_none() {
        eprintln!("skipping: set KYUMEI_TEMPORAL_SOURCE to a Temporal v1.31.0 checkout");
    }
    source.map(PathBuf::from)
}

/// The other releases of the version sweep, and the baseline.
fn sweep_sources() -> Option<(PathBuf, Vec<PathBuf>)> {
    let source = source()?;
    let Ok(spec) = std::env::var("KYUMEI_TEMPORAL_SWEEP") else {
        eprintln!(
            "skipping: set KYUMEI_TEMPORAL_SWEEP to the other swept releases' checkouts, separated by `;`"
        );
        return None;
    };
    let others = spec
        .split(';')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect();
    Some((source, others))
}

fn committed(file: &str) -> (PathBuf, String) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../knowledge/temporal/1.31/generated")
        .join(file);
    let text = std::fs::read_to_string(&path).expect("committed claims");
    (path, text)
}

fn options(version: &str, committed: &str) -> ClaimOptions {
    ClaimOptions {
        source_id: format!("temporal-server-v{version}"),
        verified_on: claims::verified_on(committed).expect("verification date"),
    }
}

fn assert_same(path: &Path, committed: &str, generated: &str, generator: &str) {
    if generated != committed {
        let changes = claims::diff(committed, generated).expect("diff");
        panic!(
            "{} is out of date; regenerate it with {generator} -o. {} differences, first: {:?}",
            path.display(),
            changes.len(),
            changes.iter().take(5).collect::<Vec<_>>()
        );
    }
}

#[test]
fn generated_dynamic_config_matches_the_sources() {
    let Some((source, others)) = sweep_sources() else {
        return;
    };
    let (path, committed) = committed("dynamic-config.toml");
    let mut releases = Vec::new();
    for dir in std::iter::once(&source).chain(&others) {
        let generated = registry_gen::generate(dir, None).expect("generate");
        let options = options(&generated.registry.temporal_version, &committed);
        releases.push(sweep::Release {
            version: generated.registry.temporal_version.clone(),
            rendered: claims::render(&generated.registry, dir, &options).expect("render"),
            source_id: options.source_id,
        });
    }
    let baseline = releases[0].version.clone();
    let text = sweep::merge(sweep::Kind::DynamicConfig, &baseline, &releases).expect("merge");
    assert_same(&path, &committed, &text, "gen-dc-claims --sweep");
}

#[test]
fn generated_server_metrics_match_the_sources() {
    let Some((source, others)) = sweep_sources() else {
        return;
    };
    let (path, committed) = committed("server-metrics.toml");
    let mut releases = Vec::new();
    for dir in std::iter::once(&source).chain(&others) {
        let generated = metrics_gen::generate(dir, None).expect("generate");
        let options = options(&generated.temporal_version, &committed);
        releases.push(sweep::Release {
            version: generated.temporal_version.clone(),
            rendered: metric_claims::render(&generated, &options).expect("render"),
            source_id: options.source_id,
        });
    }
    let baseline = releases[0].version.clone();
    let text = sweep::merge(sweep::Kind::Metric, &baseline, &releases).expect("merge");
    assert_same(&path, &committed, &text, "gen-metric-claims --sweep");
}

#[test]
fn generated_sdk_metrics_match_the_sources() {
    let Ok(spec) = std::env::var("KYUMEI_SDK_SOURCES") else {
        eprintln!("skipping: set KYUMEI_SDK_SOURCES to core=PATH;go=PATH;java=PATH");
        return;
    };
    let roots: std::collections::BTreeMap<&str, PathBuf> = spec
        .split(';')
        .filter_map(|pair| pair.split_once('='))
        .map(|(family, path)| (family, PathBuf::from(path)))
        .collect();
    let (path, committed) = committed("sdk-metrics.toml");
    let generated = sdk_gen::generate(
        Some(&roots["core"]),
        Some(&roots["go"]),
        Some(&roots["java"]),
    )
    .expect("generate");
    for (family, scan) in &generated.scans {
        assert!(
            scan.warnings.is_empty(),
            "{}: {:?}",
            family.token(),
            scan.warnings
        );
    }
    let source = |id: &str, revision: &str| FamilySource {
        source_id: id.to_owned(),
        revision: revision.to_owned(),
    };
    let options = SdkClaimOptions {
        verified_on: claims::verified_on(&committed).expect("verification date"),
        core: source("sdk-core-acac9e3f", "sdk-rust acac9e3f"),
        go: source("sdk-go-v1.44.0", "sdk-go v1.44.0"),
        java: source("sdk-java-v1.40.0", "sdk-java v1.40.0"),
    };
    let text = sdk_claims::render(&generated, &options).expect("render");
    assert_same(&path, &committed, &text, "gen-sdk-metric-claims");
}
