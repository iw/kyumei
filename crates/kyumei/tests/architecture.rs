//! Kyumei's dependency direction is part of its design, so it is tested.
//!
//!   kyumei-web (Topcoat)   kyumei-odori (Odori, Temporal SDK)
//!            \                 /
//!             ▼               ▼
//!                kyumei-app
//!                    │
//!     diagnostics ─ evidence ─ knowledge ─ diagrams
//!                    │
//!               kyumei-domain
//!
//! The domain knows nothing of frameworks; Topcoat appears only in the web
//! crate; Odori and the Temporal SDK only in the orchestration crate. The
//! knowledge extractor (`kyumei-extract`) stands apart: it writes knowledge files
//! and nothing depends on it.
//!
//! The companion collector stands apart too. `kyumei-samples` is the sample
//! block format alone, which the evidence interpreter reads; `kyumei-collect`
//! is the companion's HTTP client and binary, built on the format and nothing
//! else of Kyumei's, which the web crate also uses for Collect now:
//!
//!   kyumei-web ──▶ kyumei-collect ──▶ kyumei-samples ◀── kyumei-evidence
//!
//! `kyumei-dashboards` reads Grafana dashboards. Like the diagrams, it knows
//! nothing of Temporal or of the rest of Kyumei; the application layer pairs
//! its panels with the knowledge.
//!
//! `kyumei-sim` runs tempdes, the cluster simulator, and answers PromQL over
//! its scrapes. It is the only crate that knows tempdes, and of Kyumei it knows
//! only the sample block format, so simulated evidence can travel as collected
//! evidence does. The application layer uses it for the "big enough?" check,
//! and nothing else does. The extractor reads Temporal's dynamic configuration
//! with tempdes's parser, which the two share rather than copy:
//!
//!   kyumei-app ──▶ kyumei-sim ──▶ tempdes ◀── kyumei-extract

use std::{collections::BTreeMap, process::Command};

fn dependencies() -> BTreeMap<String, Vec<String>> {
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("metadata json");
    json["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|p| {
            let name = p["name"].as_str().unwrap_or_default().to_owned();
            let deps = p["dependencies"]
                .as_array()
                .expect("dependencies")
                .iter()
                .filter(|d| d["kind"].is_null())
                .map(|d| d["name"].as_str().unwrap_or_default().to_owned())
                .collect();
            (name, deps)
        })
        .collect()
}

fn forbid(deps: &BTreeMap<String, Vec<String>>, krate: &str, patterns: &[&str]) {
    let own = deps
        .get(krate)
        .unwrap_or_else(|| panic!("{krate} is a workspace member"));
    for d in own {
        for p in patterns {
            assert!(
                !(d == p || (p.ends_with('*') && d.starts_with(p.trim_end_matches('*')))),
                "{krate} must not depend on {d}"
            );
        }
    }
}

#[test]
fn dependency_direction_is_preserved() {
    let deps = dependencies();
    let frameworks = ["topcoat", "odori", "temporalio-*", "tokeira-*", "tokio"];
    forbid(&deps, "kyumei-domain", &["kyumei-*"]);
    forbid(&deps, "kyumei-domain", &frameworks);
    for core in [
        "kyumei-knowledge",
        "kyumei-evidence",
        "kyumei-diagrams",
        "kyumei-diagnostics",
    ] {
        forbid(&deps, core, &frameworks);
        forbid(&deps, core, &["kyumei-app", "kyumei-web", "kyumei-odori"]);
    }
    forbid(
        &deps,
        "kyumei-diagrams",
        &["kyumei-knowledge", "kyumei-diagnostics", "kyumei-evidence"],
    );
    forbid(
        &deps,
        "kyumei-app",
        &[
            "topcoat",
            "odori",
            "temporalio-*",
            "tokeira-*",
            "kyumei-web",
            "kyumei-odori",
        ],
    );
    forbid(
        &deps,
        "kyumei-web",
        &["odori", "temporalio-*", "tokeira-*", "kyumei-odori"],
    );
    forbid(&deps, "kyumei-odori", &["topcoat", "kyumei-web"]);
    // The sample block format is pure: no frameworks and none of Kyumei, so
    // the companion carries no model, agents or knowledge.
    forbid(&deps, "kyumei-samples", &frameworks);
    forbid(&deps, "kyumei-samples", &["kyumei-*"]);
    forbid(
        &deps,
        "kyumei-collect",
        &["topcoat", "odori", "temporalio-*", "tokeira-*"],
    );
    let collect = &deps["kyumei-collect"];
    assert!(
        collect
            .iter()
            .all(|d| !d.starts_with("kyumei-") || d == "kyumei-samples"),
        "kyumei-collect depends on no Kyumei crate but kyumei-samples: {collect:?}"
    );
    forbid(&deps, "kyumei-dashboards", &frameworks);
    forbid(&deps, "kyumei-dashboards", &["kyumei-*"]);
    for core in [
        "kyumei-domain",
        "kyumei-knowledge",
        "kyumei-evidence",
        "kyumei-diagnostics",
        "kyumei-diagrams",
        "kyumei-samples",
    ] {
        forbid(&deps, core, &["kyumei-dashboards"]);
    }
    // Only the interfaces read metrics over the network.
    for (krate, own) in &deps {
        if !["kyumei", "kyumei-web"].contains(&krate.as_str()) {
            assert!(
                !own.iter().any(|d| d == "kyumei-collect"),
                "{krate} must not depend on kyumei-collect"
            );
        }
    }
    // The extractor is a maintainer tool: it reads Temporal source and writes knowledge
    // files, and the rest of Kyumei never depends on it.
    forbid(&deps, "kyumei-extract", &frameworks);
    forbid(&deps, "kyumei-extract", &["kyumei-*"]);
    for (krate, own) in &deps {
        if krate != "kyumei-extract" {
            assert!(
                !own.iter().any(|d| d == "kyumei-extract"),
                "{krate} must not depend on kyumei-extract"
            );
        }
    }
    forbid(&deps, "kyumei-sim", &frameworks);
    let sim = &deps["kyumei-sim"];
    assert!(
        sim.iter()
            .all(|d| !d.starts_with("kyumei-") || d == "kyumei-samples"),
        "kyumei-sim depends on no Kyumei crate but kyumei-samples: {sim:?}"
    );
    let sim_users: Vec<&String> = deps
        .iter()
        .filter(|(_, d)| d.iter().any(|x| x == "kyumei-sim"))
        .map(|(k, _)| k)
        .collect();
    assert_eq!(
        sim_users,
        vec!["kyumei-app"],
        "only the application layer runs simulations"
    );
    let tempdes_users: Vec<&String> = deps
        .iter()
        .filter(|(_, d)| d.iter().any(|x| x == "tempdes"))
        .map(|(k, _)| k)
        .collect();
    assert_eq!(
        tempdes_users,
        vec!["kyumei-extract", "kyumei-sim"],
        "tempdes is confined to kyumei-sim, and the extractor's configuration parser"
    );
    let topcoat_users: Vec<&String> = deps
        .iter()
        .filter(|(_, d)| d.iter().any(|x| x == "topcoat"))
        .map(|(k, _)| k)
        .collect();
    assert_eq!(
        topcoat_users,
        vec!["kyumei-web"],
        "Topcoat is confined to the web crate"
    );
}
