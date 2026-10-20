use super::*;

const PACK: &str = r#"
[pack]
id = "test-1.31"
title = "Test"
version = "0.0.1"
server_family = "1.31"
verified_server_versions = ["1.31.0"]
description = "test pack"

[[source]]
id = "server"
kind = "repository"
title = "temporal"
repository = "https://github.com/temporalio/temporal"
revision = "v1.31.0"
commit = "83881961df2bad7d78d93a2d50778dbc8bd1a2fc"
retrieved = 2026-09-30

[[source]]
id = "kyumei"
kind = "authored"
title = "Kyumei heuristics"
retrieved = 2026-09-30
"#;

const METRICS: &str = r#"
[[metric]]
id = "sdk.slots"
name = "worker_task_slots_available"
category = "sdk-metric"
instrument = "gauge"
purpose = "free slots"
labels = [{ name = "worker_type", values = ["WorkflowWorker", "ActivityWorker"] }]
exposition = [{ name = "temporal_worker_task_slots_available", sdk_family = "core", condition = "default prefix" }]
scope = { sdk = [{ family = "core", revision = "sdk-core abc" }] }
verification = { status = "verified", method = "source-inspection", on = 2026-09-30 }
provenance = [{ source = "server", path = "x.rs", line = 1 }]

[[metric]]
id = "sdk.sts"
name = "workflow_task_schedule_to_start_latency"
category = "sdk-metric"
instrument = "histogram"
purpose = "sts"
exposition = [{ name = "temporal_workflow_task_schedule_to_start_latency", sdk_family = "core", unit = "milliseconds", condition = "default" }]
scope = { sdk = [{ family = "core", revision = "sdk-core abc" }] }
verification = { status = "verified", method = "source-inspection", on = 2026-09-30 }
provenance = [{ source = "server", path = "x.rs", line = 2 }]

[[heuristic]]
id = "h.free"
title = "free"
statement = "at least 5 free"
rationale = "because"
authored = 2026-09-30
provenance = [{ source = "kyumei" }]
"#;

const DIAG: &str = r#"
[diagnostic]
id = "d"
title = "D"
summary = "s"
path = { description = "p", layers = ["workers"] }

[[symptom]]
id = "sts"
title = "STS"
patterns = ["schedule[- ]to[- ]start"]
signal = "sts"

[[stage]]
id = "workers"
order = 1
title = "Workers"
question = "q"

[[signal]]
id = "slots"
title = "slots"
unit = "count"
description = "d"
source = [{ claim = "sdk.slots", labels = { worker_type = "WorkflowWorker" } }]

[[signal]]
id = "sts"
title = "sts"
unit = "seconds"
description = "d"
source = [{ claim = "sdk.sts" }]

[[hypothesis]]
id = "exhausted"
stage = "workers"
title = "Exhausted"
statement = "s"
question = "q?"
layer = "workers"
primary = true

[[request]]
id = "slots"
stage = "workers"
title = "Slots"
why = "w"
signals = ["slots"]
distinguishes = ["exhausted"]
cost = 1
collect = [{ method = "promql", label = "l", body = "b", claims = ["sdk.slots"] }]

[[rule]]
id = "free"
hypothesis = "exhausted"
effect = "eliminates"
statement = "slots stayed at {slots.min} while sts {sts.max}"
claims = ["h.free"]
when = { all = [
  { signal = "slots", stat = "min", op = "ge", value = 5 },
  { signal = "sts", stat = "max", op = "ge", value = 5 },
] }

[[conclusion]]
id = "c"
title = "C"
statement = "done"
when = { hypothesis = "exhausted", state = ["eliminated"] }
"#;

fn load(files: &[(&str, &str)]) -> Result<KnowledgeBase, LoadError> {
    KnowledgeBase::from_files(files)
}

fn problems(err: LoadError) -> Vec<String> {
    match err {
        LoadError::Inconsistent { problems, .. } => problems,
        other => vec![other.to_string()],
    }
}

#[test]
fn loads_a_consistent_pack() {
    let kb = load(&[
        ("t/1.31/pack.toml", PACK),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", DIAG),
    ])
    .expect("pack loads");
    let pack = &kb.packs()[0];
    assert_eq!(pack.diagnostics.len(), 1);
    assert_eq!(pack.digest.len(), 64);
    let (_, symptom, matched) = pack
        .classify("Workflow tasks take 30s schedule-to-start")
        .expect("classified");
    assert_eq!(symptom.id.as_str(), "sts");
    assert_eq!(matched, "schedule-to-start");
    let m = pack.match_metric("temporal_workflow_task_schedule_to_start_latency_bucket");
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].suffix, Some("_bucket"));
    // Gauges do not match histogram suffixes.
    assert!(
        pack.match_metric("temporal_worker_task_slots_available_count")
            .is_empty()
    );
}

#[test]
fn rejects_undeclared_label_constraints() {
    let bad = DIAG.replace(
        r#"labels = { worker_type = "WorkflowWorker" }"#,
        r#"labels = { poller_type = "workflow_task" }"#,
    );
    let err = load(&[
        ("t/1.31/pack.toml", PACK),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", &bad),
    ])
    .expect_err("must fail");
    let p = problems(err);
    assert!(
        p.iter()
            .any(|m| m.contains("does not declare label `poller_type`")),
        "{p:?}"
    );
}

#[test]
fn rejects_undeclared_label_values() {
    let bad = DIAG.replace("WorkflowWorker\" }", "Workflow\" }");
    let p = problems(
        load(&[
            ("t/1.31/pack.toml", PACK),
            ("t/1.31/metrics.toml", METRICS),
            ("t/1.31/diag.toml", &bad),
        ])
        .expect_err("must fail"),
    );
    assert!(
        p.iter().any(|m| m.contains("not a declared value")),
        "{p:?}"
    );
}

#[test]
fn rejects_contradictory_rules() {
    let contradictory = format!(
        "{DIAG}\n{}",
        r#"
[[rule]]
id = "free-again"
hypothesis = "exhausted"
effect = "supports"
statement = "contradiction"
when = { all = [
  { signal = "slots", stat = "min", op = "ge", value = 5 },
  { signal = "sts", stat = "max", op = "ge", value = 5 },
] }
"#
    );
    let p = problems(
        load(&[
            ("t/1.31/pack.toml", PACK),
            ("t/1.31/metrics.toml", METRICS),
            ("t/1.31/diag.toml", &contradictory),
        ])
        .expect_err("must fail"),
    );
    assert!(p.iter().any(|m| m.contains("opposing effects")), "{p:?}");
}

/// The test diagnostic with a simulation of `exhausted`, read by `rule`.
fn with_simulation(simulation: &str, rule: &str) -> String {
    format!(
        "{DIAG}\n{}\n{simulation}\n{rule}",
        r#"
[[signal]]
id = "simulated_sts"
title = "simulated sts"
unit = "seconds"
description = "d"
"#
    )
}

const SIMULATION: &str = r#"
[[simulation]]
id = "size"
title = "Size"
hypothesis = "exhausted"
model = "task-queue-dispatch"
why = "w"
inputs = { add_rate = "slots.max", dispatch_limit = "slots.last", read_partitions = "slots.min", symptom = "sts.max" }
produces = "simulated_sts"
claims = ["h.free"]
"#;

const SIMULATED_RULE: &str = r#"
[[rule]]
id = "big-enough"
hypothesis = "exhausted"
effect = "supports"
statement = "simulated {simulated_sts.max}"
when = { signal = "simulated_sts", stat = "max", op = "ge", value = 5 }
"#;

#[test]
fn a_simulation_produces_a_signal_only_it_informs() {
    let diag = with_simulation(SIMULATION, SIMULATED_RULE);
    let kb = load(&[
        ("t/1.31/pack.toml", PACK),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", &diag),
    ])
    .unwrap_or_else(|e| panic!("{e}"));
    let d = &kb.packs()[0].diagnostics[0];
    let sim = d
        .simulation_producing(&"simulated_sts".into())
        .expect("simulation");
    assert_eq!(sim.model, SimulationModel::TaskQueueDispatch);
    assert_eq!(sim.inputs["symptom"].to_string(), "sts.max");
}

#[test]
fn rejects_simulations_that_could_settle_or_cannot_run() {
    let cases = [
        (
            with_simulation(
                SIMULATION,
                &SIMULATED_RULE.replace("supports", "establishes"),
            ),
            "reads simulated `simulated_sts`, so it can only support or weaken",
        ),
        (
            with_simulation(
                &SIMULATION.replace(", read_partitions = \"slots.min\"", ""),
                SIMULATED_RULE,
            ),
            "model `task-queue-dispatch` needs input `read_partitions`",
        ),
        (
            with_simulation(
                &SIMULATION.replace("symptom = ", "pollers = \"slots.max\", symptom = "),
                SIMULATED_RULE,
            ),
            "model `task-queue-dispatch` takes no input `pollers`",
        ),
        (
            with_simulation(SIMULATION, "").replace(
                "description = \"d\"\n\n\n[[simulation]]",
                "description = \"d\"\nsource = [{ claim = \"sdk.sts\" }]\n\n\n[[simulation]]",
            ),
            "a simulation produces it, so it can have no other source",
        ),
        (
            with_simulation(SIMULATION, ""),
            "no rule reads `simulated_sts`",
        ),
    ];
    for (diag, expected) in cases {
        let p = problems(
            load(&[
                ("t/1.31/pack.toml", PACK),
                ("t/1.31/metrics.toml", METRICS),
                ("t/1.31/diag.toml", &diag),
            ])
            .expect_err("must fail"),
        );
        assert!(p.iter().any(|m| m.contains(expected)), "{expected}: {p:?}");
    }
}

#[test]
fn rejects_statements_quoting_unread_signals() {
    let bad = DIAG.replace(
        "while sts {sts.max}",
        "while sts {slots.max} and {other.max}",
    );
    let p = problems(
        load(&[
            ("t/1.31/pack.toml", PACK),
            ("t/1.31/metrics.toml", METRICS),
            ("t/1.31/diag.toml", &bad),
        ])
        .expect_err("must fail"),
    );
    assert!(
        p.iter().any(|m| m.contains("unknown signal `other`")),
        "{p:?}"
    );
}

#[test]
fn a_baseline_is_quoted_only_where_it_is_certain_or_optional() {
    let with = |diag: &str| {
        load(&[
            ("t/1.31/pack.toml", PACK),
            ("t/1.31/metrics.toml", METRICS),
            ("t/1.31/diag.toml", diag),
        ])
    };
    let quoting = |text: &str| DIAG.replace("while sts {sts.max}", text);
    // The rule reads the maximum only, so the baseline may be missing.
    let p = problems(
        with(&quoting(
            "while sts {sts.max}, against about {sts.baseline} before the incident",
        ))
        .expect_err("must fail"),
    );
    assert!(
        p.iter()
            .any(|m| m.contains("quotes `{sts.baseline}` outside an optional segment")),
        "{p:?}"
    );
    with(&quoting(
        "while sts {sts.max}[, against about {sts.baseline} before the incident]",
    ))
    .expect("an optional comparison loads");
    // A rule whose condition reads the baseline can quote it plainly.
    let reads_baseline = quoting("while sts {sts.max} against about {sts.baseline}").replace(
        "  { signal = \"sts\", stat = \"max\", op = \"ge\", value = 5 },",
        "  { ratio = { numerator = \"sts.max\", denominator = \"sts.baseline\" }, op = \"ge\", value = 3 },",
    );
    with(&reads_baseline).expect("a baseline the condition read loads");
    // A conclusion can quote it when every rule that puts its hypothesis in the
    // required state read it.
    let concluding = |diag: &str| {
        diag.replace(
            "statement = \"done\"",
            "statement = \"sts reached {sts.max}, from about {sts.baseline}\"",
        )
    };
    with(&concluding(&reads_baseline)).expect("the eliminating rule read the baseline");
    let p = problems(with(&concluding(DIAG)).expect_err("must fail"));
    assert!(
        p.iter().any(|m| m.contains("conclusion `c`")
            && m.contains("quotes `{sts.baseline}` outside an optional segment")),
        "{p:?}"
    );
}

#[test]
fn rejects_malformed_optional_segments() {
    for (text, expected) in [
        ("while sts [{sts.max}", "never closed"),
        ("while sts {sts.max}]", "closes no optional segment"),
        ("while sts [{sts.max} [{sts.min}]]", "opens inside another"),
        ("while sts {sts.max}[ at last]", "no placeholder"),
    ] {
        let p = problems(
            load(&[
                ("t/1.31/pack.toml", PACK),
                ("t/1.31/metrics.toml", METRICS),
                (
                    "t/1.31/diag.toml",
                    &DIAG.replace("while sts {sts.max}", text),
                ),
            ])
            .expect_err("must fail"),
        );
        assert!(p.iter().any(|m| m.contains(expected)), "{text}: {p:?}");
    }
}

#[test]
fn rejects_hypotheses_off_the_system_path() {
    let bad = DIAG.replace(
        r#"layers = ["workers"] }"#,
        r#"layers = ["frontend", "frontend"], labels = { matching = "M" } }"#,
    );
    let p = problems(
        load(&[
            ("t/1.31/pack.toml", PACK),
            ("t/1.31/metrics.toml", METRICS),
            ("t/1.31/diag.toml", &bad),
        ])
        .expect_err("must fail"),
    );
    for expected in [
        "layer `frontend` appears twice",
        "label for layer `matching`",
        "hypothesis `exhausted` is in layer `workers`, which is not on the system path",
    ] {
        assert!(p.iter().any(|m| m.contains(expected)), "{expected}: {p:?}");
    }
}

#[test]
fn rejects_verified_claims_without_provenance() {
    let bad = METRICS.replacen(
        r#"provenance = [{ source = "server", path = "x.rs", line = 1 }]"#,
        "",
        1,
    );
    let p = problems(
        load(&[
            ("t/1.31/pack.toml", PACK),
            ("t/1.31/metrics.toml", &bad),
            ("t/1.31/diag.toml", DIAG),
        ])
        .expect_err("must fail"),
    );
    assert!(
        p.iter()
            .any(|m| m.contains("verified claims need provenance")),
        "{p:?}"
    );
}

#[test]
fn rejects_heuristics_without_authored_source() {
    let bad = METRICS.replace(
        r#"provenance = [{ source = "kyumei" }]"#,
        r#"provenance = [{ source = "server", path = "x", line = 3 }]"#,
    );
    let p = problems(
        load(&[
            ("t/1.31/pack.toml", PACK),
            ("t/1.31/metrics.toml", &bad),
            ("t/1.31/diag.toml", DIAG),
        ])
        .expect_err("must fail"),
    );
    assert!(
        p.iter()
            .any(|m| m.contains("must cite an `authored` source")),
        "{p:?}"
    );
}

#[test]
fn rejects_unknown_fields_loudly() {
    let bad = DIAG.replace("cost = 1", "cost = 1\ncots = 2");
    let err = load(&[
        ("t/1.31/pack.toml", PACK),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", &bad),
    ])
    .expect_err("must fail");
    assert!(err.to_string().contains("cots"), "{err}");
}

#[test]
fn selects_packs_by_version() {
    let kb = load(&[
        ("t/1.31/pack.toml", PACK),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", DIAG),
    ])
    .expect("loads");
    let v = |s: &str| s.parse::<ServerVersion>().expect("version");
    assert_eq!(
        kb.select(Some(&v("1.31.0"))).map(|s| s.fit),
        Some(PackFit::Verified)
    );
    assert_eq!(
        kb.select(Some(&v("1.31.3"))).map(|s| s.fit),
        Some(PackFit::SameMinor)
    );
    assert_eq!(
        kb.select(Some(&v("1.29.1"))).map(|s| s.fit),
        Some(PackFit::OutsideFamily)
    );
    assert_eq!(
        kb.select(None).map(|s| s.fit),
        Some(PackFit::VersionUnknown)
    );
}

#[test]
fn builtin_packs_are_chosen_by_minor_release() {
    let kb = KnowledgeBase::builtin().expect("builtin");
    let v = |s: &str| s.parse::<ServerVersion>().expect("version");
    let chosen = |s: &str| {
        let selection = kb.select(Some(&v(s))).expect("a pack");
        (selection.pack.manifest.id.to_string(), selection.fit)
    };
    for release in ["1.32.0", "1.32.1"] {
        assert_eq!(
            chosen(release),
            ("temporal-1.32".to_owned(), PackFit::Verified)
        );
    }
    for release in ["1.31.0", "1.31.3", "1.31.4"] {
        assert_eq!(
            chosen(release),
            ("temporal-1.31".to_owned(), PackFit::Verified)
        );
    }
    for release in ["1.30.7", "1.30.8", "1.27.0"] {
        assert_eq!(
            chosen(release),
            ("temporal-1.31".to_owned(), PackFit::Swept)
        );
    }
    // A release no pack covers yet still reads with the nearest pack of its minor.
    assert_eq!(
        chosen("1.32.2"),
        ("temporal-1.32".to_owned(), PackFit::SameMinor)
    );
    // Outside every pack's minor, the nearest pack: the oldest for an older release.
    assert_eq!(
        chosen("1.26.0"),
        ("temporal-1.31".to_owned(), PackFit::OutsideFamily)
    );
    assert_eq!(
        chosen("1.33.0"),
        ("temporal-1.32".to_owned(), PackFit::OutsideFamily)
    );
    assert_eq!(
        kb.select(None).map(|s| s.pack.manifest.id.to_string()),
        Some("temporal-1.32".to_owned())
    );
}

/// A pack whose sweep covers 1.30.0 and 1.30.1 besides the verified 1.31.0.
fn swept_pack() -> String {
    PACK.replace(
        "verified_server_versions = [\"1.31.0\"]",
        "verified_server_versions = [\"1.31.0\"]\nswept_server_versions = [\"1.30.0\", \"1.30.1\", \"1.31.0\"]",
    ) + r#"
[[source]]
id = "server-1.30.0"
kind = "repository"
title = "temporal 1.30.0"
commit = "aaaa"
retrieved = 2026-10-13

[[source]]
id = "server-1.30.1"
kind = "repository"
title = "temporal 1.30.1"
commit = "bbbb"
retrieved = 2026-10-13
"#
}

const SWEPT_DC: &str = r#"
[[dynamic_config]]
id = "dc.matching.rps"
key = "matching.rps"
setting = "NewGlobalIntSetting"
value_type = "int"
default = "1200"
constrained_by = "global"
description = "MatchingRPS is request rate per second for each matching host"
scope = { server = ["1.30.1", "1.31.0"] }
verification = { status = "verified", method = "source-inspection", on = 2026-10-13 }
provenance = [
  { source = ["server-1.30.1", "server"], path = "common/dynamicconfig/constants.go", line = 1112, excerpt = "\"matching.rps\"," },
]

[[dynamic_config.variant]]
server = ["1.30.0"]
setting = "NewGlobalIntSetting"
value_type = "int"
default = "600"
constrained_by = "global"
provenance = [
  { source = "server-1.30.0", path = "common/dynamicconfig/constants.go", line = 1100, excerpt = "\"matching.rps\"," },
]
"#;

#[test]
fn a_swept_release_gets_its_own_fit_and_a_variant_its_own_values() {
    let pack = swept_pack();
    let kb = load(&[
        ("t/1.31/pack.toml", &pack),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", DIAG),
        ("t/1.31/dc.toml", SWEPT_DC),
    ])
    .unwrap_or_else(|e| panic!("{e}"));
    let v = |s: &str| s.parse::<ServerVersion>().expect("version");
    assert_eq!(
        kb.select(Some(&v("1.30.1"))).map(|s| s.fit),
        Some(PackFit::Swept)
    );
    assert_eq!(
        kb.select(Some(&v("1.29.0"))).map(|s| s.fit),
        Some(PackFit::OutsideFamily)
    );
    let pack = &kb.packs()[0];
    let rps = "dc.matching.rps".into();
    assert_eq!(
        pack.applicability(&rps, Some(&v("1.30.1"))),
        Applicability::Verified
    );
    assert_eq!(
        pack.applicability(&rps, Some(&v("1.30.0"))),
        Applicability::VerifiedDifferently {
            difference: "default 600".to_owned()
        }
    );
    // One citation, checked in both releases it names.
    let claim = pack.claim(&rps).expect("claim");
    assert_eq!(claim.all_provenance().len(), 2);
    assert_eq!(claim.provenance()[0].sources.len(), 2);
}

#[test]
fn rejects_unknown_releases_and_overlapping_variants() {
    let pack = swept_pack();
    let bad = SWEPT_DC.replace(
        "scope = { server = [\"1.30.1\", \"1.31.0\"] }",
        "scope = { server = [\"1.30.0\", \"1.30.2\", \"1.31.0\"] }",
    );
    let p = problems(
        load(&[
            ("t/1.31/pack.toml", &pack),
            ("t/1.31/metrics.toml", METRICS),
            ("t/1.31/diag.toml", DIAG),
            ("t/1.31/dc.toml", &bad),
        ])
        .expect_err("must fail"),
    );
    for expected in [
        "release `1.30.2` is neither verified nor swept",
        "release `1.30.0` is described twice",
    ] {
        assert!(p.iter().any(|m| m.contains(expected)), "{expected}: {p:?}");
    }
}

#[test]
fn an_annotation_narrows_a_server_metric_to_the_releases_it_was_reviewed_for() {
    let pack = swept_pack();
    let generated = GENERATED_METRICS
        .replace(
            "scope = { server = [\"1.31.0\"] }",
            "scope = { server = [\"1.30.0\", \"1.30.1\", \"1.31.0\"] }",
        )
        .replace(
            "provenance = [{ source = \"server\"",
            "provenance = [{ source = [\"server-1.30.0\", \"server-1.30.1\", \"server\"]",
        );
    let reviewed = r#"
[[annotation]]
claim = "server.service_requests"
scope = { server = ["1.30.1", "1.31.0"] }
labels = [{ name = "operation" }]
"#;
    let kb = load(&[
        ("t/1.31/pack.toml", &pack),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", DIAG),
        ("t/1.31/generated.toml", &generated),
        ("t/1.31/annotations.toml", reviewed),
    ])
    .unwrap_or_else(|e| panic!("{e}"));
    let pack = &kb.packs()[0];
    let requests = pack
        .claim(&"server.service_requests".into())
        .and_then(Claim::as_metric)
        .expect("metric");
    assert_eq!(requests.scope.server, ["1.30.1", "1.31.0"]);
    let latency = pack
        .claim(&"server.service_latency".into())
        .and_then(Claim::as_metric)
        .expect("metric");
    assert_eq!(
        latency.scope.server.len(),
        3,
        "unannotated claims keep their scope"
    );

    let unreviewed = reviewed.replace("scope = { server = [\"1.30.1\", \"1.31.0\"] }\n", "");
    let p = problems(
        load(&[
            ("t/1.31/pack.toml", &swept_pack()),
            ("t/1.31/metrics.toml", METRICS),
            ("t/1.31/diag.toml", DIAG),
            ("t/1.31/generated.toml", &generated),
            ("t/1.31/annotations.toml", &unreviewed),
        ])
        .expect_err("must fail"),
    );
    assert!(
        p.iter()
            .any(|m| m.contains("must list the releases it was reviewed for")),
        "{p:?}"
    );
}

#[test]
fn builtin_pack_loads_and_validates() {
    let kb = KnowledgeBase::builtin().unwrap_or_else(|e| panic!("{e}"));
    let pack = &kb.packs()[0];
    assert_eq!(pack.manifest.id.as_str(), "temporal-1.31");
    let d = pack
        .diagnostic(&"wft-schedule-to-start".into())
        .expect("diagnostic");
    assert_eq!(d.stages.len(), 6);
    assert!(
        pack.classify("workflow tasks are taking ~30s schedule to start")
            .is_some()
    );
    assert!(
        pack.classify("Workflow Tasks on order-fulfillment are waiting about 30 seconds to start")
            .is_some()
    );
}

#[test]
fn builtin_symptoms_route_to_their_diagnostics() {
    let kb = KnowledgeBase::builtin().unwrap_or_else(|e| panic!("{e}"));
    let cases = [
        (
            "Since about 10:19 UTC, activities on the invoices task queue take over 20 seconds to start",
            "activity-schedule-to-start",
            "activity-schedule-to-start-elevated",
        ),
        (
            "Payout activities are starting late since the release",
            "activity-schedule-to-start",
            "activity-schedule-to-start-elevated",
        ),
        (
            "Workflow tasks take 30 seconds to be picked up",
            "wft-schedule-to-start",
            "wft-schedule-to-start-elevated",
        ),
        (
            "Completed Workflows still show as Running in the Web UI",
            "visibility",
            "visibility-lag",
        ),
        (
            "New Workflows don't show up in the Web UI's list for minutes",
            "visibility",
            "visibility-lag",
        ),
        (
            "The search attributes we upsert are out of date in searches",
            "visibility",
            "visibility-lag",
        ),
        (
            "ListWorkflowExecutions fails with namespace rate limit exceeded",
            "visibility",
            "visibility-errors",
        ),
        (
            "Listing workflows in the billing namespace is very slow",
            "visibility",
            "visibility-errors",
        ),
        (
            "Elasticsearch is rejecting bulk requests from History",
            "visibility",
            "visibility-errors",
        ),
        (
            "Our Java Workers log WorkflowRejectedExecutionError and Workflow Tasks time out",
            "java-workflow-threads",
            "java-workflow-threads",
        ),
        (
            "The Workers keep evicting cached Workflows to free threads, so they replay",
            "java-workflow-threads",
            "java-workflow-threads",
        ),
        (
            "Since the deploy our workers spend seconds replaying workflows",
            "sticky-cache",
            "sticky-cache",
        ),
        (
            "Timers fire late by about a minute",
            "history-task-lag",
            "history-task-lag",
        ),
        (
            "Starts are rejected with ResourceExhausted: namespace rate limit exceeded",
            "resource-exhausted",
            "resource-exhausted-errors",
        ),
    ];
    for pack in kb.packs() {
        for (description, diagnostic, symptom) in cases {
            let (d, s, _) = pack
                .classify(description)
                .unwrap_or_else(|| panic!("unclassified: {description}"));
            assert_eq!(
                (d.id.as_str(), s.id.as_str()),
                (diagnostic, symptom),
                "{description}"
            );
        }
    }
}

/// Re-verifies the builtin pack's provenance when local checkouts are
/// available. Set KYUMEI_VERIFY_SOURCES to `id=path;id=path`.
#[test]
fn builtin_provenance_matches_local_sources() {
    let Ok(spec) = std::env::var("KYUMEI_VERIFY_SOURCES") else {
        return;
    };
    let roots: std::collections::BTreeMap<kyumei_domain::SourceId, std::path::PathBuf> = spec
        .split(';')
        .filter_map(|pair| pair.split_once('='))
        .map(|(id, path)| (id.into(), path.into()))
        .collect();
    let kb = KnowledgeBase::builtin().expect("builtin");
    let checks = verify::verify(&kb.packs()[0], &roots);
    let failures: Vec<String> = checks
        .iter()
        .filter(|c| {
            c.outcome.is_failure() || matches!(c.outcome, verify::CheckOutcome::Drifted { .. })
        })
        .map(|c| {
            format!(
                "{} {}:{} [{}] {}",
                c.claim,
                c.path,
                c.line,
                c.source,
                c.outcome.label()
            )
        })
        .collect();
    assert!(
        failures.is_empty(),
        "provenance problems:\n{}",
        failures.join("\n")
    );
    let confirmed = checks
        .iter()
        .filter(|c| c.outcome == verify::CheckOutcome::Confirmed)
        .count();
    eprintln!("{confirmed} provenance excerpts confirmed");
    let references = verify::verify_references(&kb.packs()[0], &roots);
    let wrong: Vec<String> = references
        .iter()
        .filter(|c| c.outcome.is_failure())
        .map(|c| format!("{} {} {}", c.reference, c.path, c.outcome.label()))
        .collect();
    assert!(
        wrong.is_empty(),
        "reference problems:\n{}",
        wrong.join("\n")
    );
    let confirmed = references
        .iter()
        .filter(|c| c.outcome == verify::ReferenceOutcome::Confirmed)
        .count();
    eprintln!("{confirmed} references confirmed");
}

const GENERATED_DC: &str = r#"
[[dynamic_config]]
id = "dc.matching.rps"
key = "matching.rps"
setting = "NewGlobalIntSetting"
value_type = "int"
default = "1200"
constrained_by = "global"
description = "MatchingRPS is request rate per second for each matching host"
scope = { server = ["1.31.0"] }
verification = { status = "verified", method = "source-inspection", on = 2026-10-03 }
provenance = [
  { source = "server", path = "common/dynamicconfig/constants.go", line = 1112, excerpt = "\"matching.rps\"," },
]

[[heuristic]]
id = "heuristic.busy"
title = "Busy"
statement = "Busy means busy."
rationale = "Because."
authored = 2026-10-03
provenance = [{ source = "kyumei" }]
"#;

#[test]
fn annotations_merge_into_generated_claims() {
    // The annotation file sorts before the claims it annotates: order does not matter.
    let annotations = r#"
[[annotation]]
claim = "dc.matching.rps"
notes = ["Per Matching host."]
provenance = [
  { source = "server", path = "service/matching/handler.go", line = 10, excerpt = "rateLimiter" },
]
"#;
    let kb = load(&[
        ("t/1.31/pack.toml", PACK),
        ("t/1.31/a-annotations.toml", annotations),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", DIAG),
        ("t/1.31/z-generated.toml", GENERATED_DC),
    ])
    .expect("pack loads");
    let claim = kb.packs()[0]
        .claim(&"dc.matching.rps".into())
        .expect("claim");
    assert_eq!(claim.notes(), ["Per Matching host."]);
    assert_eq!(claim.provenance().len(), 2);
    assert_eq!(
        claim.provenance()[1].path.as_deref(),
        Some("service/matching/handler.go")
    );
}

#[test]
fn rejects_annotations_that_cannot_apply() {
    let annotations = r#"
[[annotation]]
claim = "dc.nope"
notes = ["Nothing to annotate."]

[[annotation]]
claim = "dc.matching.rps"

[[annotation]]
claim = "heuristic.busy"
notes = ["Heuristics are edited directly."]
"#;
    let err = load(&[
        ("t/1.31/pack.toml", PACK),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", DIAG),
        ("t/1.31/generated.toml", GENERATED_DC),
        ("t/1.31/annotations.toml", annotations),
    ])
    .expect_err("must fail");
    let p = problems(err);
    assert!(
        p.iter()
            .any(|x| x.contains("annotation for unknown claim `dc.nope`")),
        "{p:?}"
    );
    assert!(
        p.iter()
            .any(|x| x.contains("annotation for `dc.matching.rps` adds nothing")),
        "{p:?}"
    );
    assert!(
        p.iter()
            .any(|x| x.contains("heuristics are written by hand")),
        "{p:?}"
    );
}

const GENERATED_METRICS: &str = r#"
[[metric]]
id = "server.service_requests"
name = "service_requests"
category = "server-metric"
instrument = "counter"
unit = "count"
purpose = "The number of RPC requests received by the service."
go_name = "metrics.ServiceRequests"
recorded_in = ["common/rpc/interceptor", "service/frontend"]
scope = { server = ["1.31.0"] }
verification = { status = "verified", method = "source-inspection", on = 2026-10-03 }
provenance = [{ source = "server", path = "common/metrics/metric_defs.go", line = 625, excerpt = "\"service_requests\"," }]

[[metric.exposition]]
name = "service_requests"
condition = "The default tally framework or OpenTelemetry with `withoutCounterSuffix: true`."

[[metric.exposition]]
name = "service_requests_total"
condition = "OpenTelemetry with its default options."

[[metric]]
id = "server.approximate_backlog_age_seconds"
name = "approximate_backlog_age_seconds"
category = "matching-metric"
instrument = "gauge"
go_name = "metrics.ApproximateBacklogAgeSeconds"
recorded_in = ["service/matching"]
scope = { server = ["1.31.0"] }
verification = { status = "verified", method = "source-inspection", on = 2026-10-03 }
provenance = [{ source = "server", path = "common/metrics/metric_defs.go", line = 1197, excerpt = "NewGaugeDef(\"approximate_backlog_age_seconds\")" }]

[[metric.exposition]]
name = "approximate_backlog_age_seconds"
condition = "Either metrics framework, with any options."

[[metric]]
id = "server.service_latency"
name = "service_latency"
category = "server-metric"
instrument = "timer"
go_name = "metrics.ServiceLatency"
recorded_in = ["common/rpc/interceptor"]
scope = { server = ["1.31.0"] }
verification = { status = "verified", method = "source-inspection", on = 2026-10-03 }
provenance = [{ source = "server", path = "common/metrics/metric_defs.go", line = 654, excerpt = "NewTimerDef(\"service_latency\")" }]

[[metric.exposition]]
name = "service_latency"
unit = "seconds"
condition = "The default tally framework or OpenTelemetry with `withoutUnitSuffix: true` and `recordTimerInSeconds: true`."

[[metric.exposition]]
name = "service_latency_milliseconds"
unit = "milliseconds"
condition = "OpenTelemetry with its default options."

[[metric.exposition]]
name = "service_latency"
unit = "milliseconds"
condition = "OpenTelemetry with `withoutUnitSuffix: true`."
"#;

#[test]
fn metric_annotations_fill_what_the_source_leaves_out() {
    let annotations = r#"
[[annotation]]
claim = "server.approximate_backlog_age_seconds"
scope = { server = ["1.31.0"] }
purpose = "Age of the oldest backlog task."
unit = "seconds"
labels = [{ name = "namespace" }, { name = "task_type", values = ["Workflow", "Activity", "Nexus"] }]

[[annotation]]
claim = "server.service_requests"
scope = { server = ["1.31.0"] }
labels = [{ name = "operation" }]
notes = ["Rejected calls are counted too."]
"#;
    let kb = load(&[
        ("t/1.31/pack.toml", PACK),
        ("t/1.31/a-annotations.toml", annotations),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", DIAG),
        ("t/1.31/z-generated.toml", GENERATED_METRICS),
    ])
    .expect("pack loads");
    let pack = &kb.packs()[0];
    let age = pack
        .claim(&"server.approximate_backlog_age_seconds".into())
        .and_then(Claim::as_metric)
        .expect("age");
    assert_eq!(age.unit, Some(kyumei_domain::Unit::Seconds));
    assert_eq!(
        age.purpose.as_deref(),
        Some("Age of the oldest backlog task.")
    );
    assert_eq!(age.labels.len(), 2);
    assert_eq!(age.recorded_in, ["service/matching"]);

    // The source's description stays the purpose; the annotation adds labels and a note.
    let requests = pack
        .claim(&"server.service_requests".into())
        .expect("requests");
    assert_eq!(
        requests.purpose(),
        "The number of RPC requests received by the service."
    );
    assert_eq!(requests.notes(), ["Rejected calls are counted too."]);
    let requests = requests.as_metric().expect("metric");
    assert_eq!(requests.go_name.as_deref(), Some("metrics.ServiceRequests"));
    assert_eq!(requests.labels[0].name, "operation");

    // A metric nobody described says so.
    let latency = pack
        .claim(&"server.service_latency".into())
        .expect("latency");
    assert_eq!(latency.purpose(), model::NOT_DESCRIBED);
}

#[test]
fn a_bare_timer_name_matches_both_of_its_units() {
    let kb = load(&[
        ("t/1.31/pack.toml", PACK),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", DIAG),
        ("t/1.31/generated.toml", GENERATED_METRICS),
    ])
    .expect("pack loads");
    let pack = &kb.packs()[0];
    let units: Vec<_> = pack
        .match_metric("service_latency_bucket")
        .iter()
        .map(|m| m.exposition.unit)
        .collect();
    assert_eq!(
        units,
        [
            Some(kyumei_domain::Unit::Seconds),
            Some(kyumei_domain::Unit::Milliseconds)
        ]
    );
    let total = pack.match_metric("service_requests_total");
    assert_eq!(total.len(), 1);
    assert_eq!(total[0].metric.id.as_str(), "server.service_requests");
}

#[test]
fn rejects_metric_annotations_that_replace_the_source() {
    let annotations = r#"
[[annotation]]
claim = "server.service_requests"
scope = { server = ["1.31.0"] }
purpose = "Requests."

[[annotation]]
claim = "server.service_requests"
scope = { server = ["1.31.0"] }
unit = "seconds"

[[annotation]]
claim = "server.approximate_backlog_age_seconds"
scope = { server = ["1.31.0"] }
labels = [{ name = "namespace" }]

[[annotation]]
claim = "server.approximate_backlog_age_seconds"
scope = { server = ["1.31.0"] }
labels = [{ name = "taskqueue" }]

[[annotation]]
claim = "dc.matching.rps"
labels = [{ name = "namespace" }]
"#;
    let err = load(&[
        ("t/1.31/pack.toml", PACK),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", DIAG),
        ("t/1.31/generated.toml", GENERATED_METRICS),
        ("t/1.31/generated-dc.toml", GENERATED_DC),
        ("t/1.31/annotations.toml", annotations),
    ])
    .expect_err("must fail");
    let p = problems(err);
    for expected in [
        "annotation for `server.service_requests`: the claim already states a purpose",
        "annotation for `server.service_requests`: the claim already states a unit",
        "annotation for `server.approximate_backlog_age_seconds`: the claim already declares labels",
        "annotation for `dc.matching.rps`: only metric annotations can set",
    ] {
        assert!(p.iter().any(|x| x.contains(expected)), "{expected}: {p:?}");
    }
}

const OPS_SOURCE: &str = r#"
[[source]]
id = "ops"
kind = "repository"
title = "Operations"
repository = "https://github.com/example/operations"
revision = "main"
commit = "0123456789abcdef0123456789abcdef01234567"
retrieved = 2026-10-14
"#;

const REFERENCES: &str = r#"
[[reference]]
id = "ops.runbook.slots"
kind = "runbook"
title = "Slots Exhausted"
source = "ops"
path = "runbooks/slots.md"
section = "Workflow `Workers`"
why = "What to check when the slots are exhausted."
link = [
  { diagnostic = "d", hypotheses = ["exhausted"], requests = ["slots"] },
  { diagnostic = "d" },
]

[[reference]]
id = "ops.panel.slots"
kind = "panel"
title = "Free Slots"
source = "ops"
path = "dashboards/sdk.json"
panel = { dashboard = "sdk-v1", dashboard_title = "SDK v1.0", id = 7, sdk_family = "core" }
why = "Free slots per Task Queue."
link = [{ diagnostic = "d", requests = ["slots"], conclusions = ["c"] }]
"#;

fn pack_with_references(references: &str) -> Result<KnowledgeBase, LoadError> {
    let sources = format!("{PACK}\n{OPS_SOURCE}");
    load(&[
        ("t/1.31/pack.toml", &sources),
        ("t/1.31/metrics.toml", METRICS),
        ("t/1.31/diag.toml", DIAG),
        ("t/1.31/references.toml", references),
    ])
}

#[test]
fn references_link_what_the_diagnostics_define() {
    let kb = pack_with_references(REFERENCES).expect("pack loads");
    let pack = &kb.packs()[0];
    let d: DiagnosticId = "d".into();
    let ids = |target| {
        pack.references_for(&d, target)
            .iter()
            .map(|r| r.id.to_string())
            .collect::<Vec<_>>()
    };
    let (request, hypothesis, conclusion) = ("slots".into(), "exhausted".into(), "c".into());
    assert_eq!(
        ids(ReferenceTarget::Request(&request)),
        vec!["ops.runbook.slots", "ops.panel.slots"]
    );
    assert_eq!(
        ids(ReferenceTarget::Hypothesis(&hypothesis)),
        vec!["ops.runbook.slots"]
    );
    assert_eq!(
        ids(ReferenceTarget::Conclusion(&conclusion)),
        vec!["ops.panel.slots"]
    );
    assert_eq!(ids(ReferenceTarget::Diagnostic), vec!["ops.runbook.slots"]);
    let runbook = pack
        .reference(&"ops.runbook.slots".into())
        .expect("runbook");
    assert_eq!(runbook.anchor().as_deref(), Some("workflow-workers"));
    assert_eq!(
        runbook.display_title(),
        "Slots Exhausted — Workflow Workers"
    );
    let source = pack.source(&runbook.source).expect("source");
    assert_eq!(
        source
            .file_url(&runbook.path, runbook.anchor().as_deref())
            .as_deref(),
        Some(
            "https://github.com/example/operations/blob/0123456789abcdef0123456789abcdef01234567/runbooks/slots.md#workflow-workers"
        )
    );
}

#[test]
fn rejects_references_that_link_nothing_real_or_cite_loosely() {
    let cases = [
        (
            REFERENCES.replace("hypotheses = [\"exhausted\"]", "hypotheses = [\"missing\"]"),
            "`d` has no hypothesis `missing`",
        ),
        (
            REFERENCES.replace("diagnostic = \"d\" }", "diagnostic = \"nope\" }"),
            "unknown diagnostic `nope`",
        ),
        (
            REFERENCES.replace(
                "path = \"runbooks/slots.md\"",
                "path = \"runbooks/slots.json\"",
            ),
            "a runbook or playbook is a Markdown file",
        ),
        (
            REFERENCES.replace(
                "panel = { dashboard",
                "section = \"x\"\npanel = { dashboard",
            ),
            "a panel has no section",
        ),
        (
            REFERENCES.replace(
                "source = \"ops\"\npath = \"dashboards",
                "source = \"kyumei\"\npath = \"dashboards",
            ),
            "must be a repository pinned by commit",
        ),
        (
            REFERENCES.replace("why = \"Free slots per Task Queue.\"", "why = \" \""),
            "does not say why",
        ),
        (
            REFERENCES.replace("path = \"runbooks/slots.md\"", "path = \"../slots.md\""),
            "is not a path within the source",
        ),
    ];
    for (references, expected) in cases {
        let problems = problems(pack_with_references(&references).expect_err(expected));
        assert!(
            problems.iter().any(|p| p.contains(expected)),
            "expected `{expected}` in {problems:#?}"
        );
    }
}

#[test]
fn references_verify_in_their_source() {
    use verify::ReferenceOutcome as O;
    let kb = pack_with_references(REFERENCES).expect("pack loads");
    let pack = &kb.packs()[0];
    let root = std::env::temp_dir().join(format!("kyumei-references-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("runbooks")).expect("dir");
    std::fs::create_dir_all(root.join("dashboards")).expect("dir");
    let write = |path: &str, text: &str| std::fs::write(root.join(path), text).expect("write");
    let runbook = "# Slots Exhausted\n\n```\n## Workflow `Workers`\n```\n\n## Workflow `Workers`\n\nCheck them.\n";
    let dashboard = r#"{"uid": "sdk-v1", "title": "SDK v1.0", "panels": [
        {"id": 1, "type": "row", "title": "Workers", "panels": [{"id": 7, "type": "timeseries", "title": "Free Slots"}]}]}"#;
    write("runbooks/slots.md", runbook);
    write("dashboards/sdk.json", dashboard);
    let roots: std::collections::BTreeMap<kyumei_domain::SourceId, std::path::PathBuf> =
        [("ops".into(), root.clone())].into();
    let outcomes = |roots: &std::collections::BTreeMap<_, _>| {
        verify::verify_references(pack, roots)
            .into_iter()
            .map(|c| (c.reference.to_string(), c.outcome))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        outcomes(&roots),
        vec![
            ("ops.runbook.slots".to_owned(), O::Confirmed),
            ("ops.panel.slots".to_owned(), O::Confirmed),
        ]
    );

    // A heading that moved under an earlier twin, a renamed panel.
    write(
        "runbooks/slots.md",
        &format!("{runbook}\n## Workflow `Workers`\n").replace(
            "## Workflow `Workers`\n\nCheck",
            "## Workflow Workers\n\nCheck",
        ),
    );
    write(
        "dashboards/sdk.json",
        &dashboard.replace("\"Free Slots\"", "\"Slots Free\""),
    );
    assert_eq!(
        outcomes(&roots),
        vec![
            (
                "ops.runbook.slots".to_owned(),
                O::AnchorChanged {
                    found: "workflow-workers-1".to_owned()
                }
            ),
            (
                "ops.panel.slots".to_owned(),
                O::PanelTitleChanged {
                    found: "Slots Free".to_owned()
                }
            ),
        ]
    );

    // Another dashboard, a missing section, and a checkout at another commit.
    write("runbooks/slots.md", "# Slots Exhausted\n");
    write(
        "dashboards/sdk.json",
        &dashboard.replace("sdk-v1", "sdk-v2"),
    );
    assert_eq!(
        outcomes(&roots),
        vec![
            ("ops.runbook.slots".to_owned(), O::SectionMissing),
            (
                "ops.panel.slots".to_owned(),
                O::DashboardChanged {
                    found: "sdk-v2".to_owned()
                }
            ),
        ]
    );
    std::fs::create_dir_all(root.join(".git")).expect("dir");
    write(".git/HEAD", "ref: refs/heads/main\n");
    write(
        ".git/packed-refs",
        "# pack-refs\nfedcba9876543210fedcba9876543210fedcba98 refs/heads/main\n",
    );
    assert_eq!(
        verify::checkout_commit(&root).as_deref(),
        Some("fedcba9876543210fedcba9876543210fedcba98")
    );
    assert!(
        outcomes(&roots)
            .iter()
            .all(|(_, o)| matches!(o, O::WrongCommit { .. }))
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn reference_versions_are_checked_where_the_source_declares_them() {
    use verify::ReferenceOutcome as O;
    let versioned = REFERENCES
        .replace(
            "panel = { dashboard = \"sdk-v1\"",
            "version = { version = \"v1.0\", of = \"dashboard\", path = \"dashboards/sdk.json\", line = 1, excerpt = \"\\\"title\\\": \\\"SDK v1.0\\\"\" }\npanel = { dashboard = \"sdk-v1\"",
        )
        .replace(
            "path = \"runbooks/slots.md\"",
            "path = \"runbooks/slots.md\"\nversion = { version = \"v1.0+\", of = \"needs-dashboard\", dashboard = \"SDK\", path = \"runbooks/slots.md\", line = 3, excerpt = \"[SDK](../dashboards/sdk.json) v1.0+\" }",
        );
    let kb = pack_with_references(&versioned).expect("pack loads");
    let pack = &kb.packs()[0];
    let runbook = pack
        .reference(&"ops.runbook.slots".into())
        .expect("runbook");
    assert_eq!(
        runbook
            .version
            .as_ref()
            .map(ReferenceVersion::describe)
            .as_deref(),
        Some("needs the SDK dashboard v1.0+")
    );

    let root = std::env::temp_dir().join(format!("kyumei-versions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("runbooks")).expect("dir");
    std::fs::create_dir_all(root.join("dashboards")).expect("dir");
    let write = |path: &str, text: &str| std::fs::write(root.join(path), text).expect("write");
    let runbook = "# Slots Exhausted\n\nNeeds the [SDK](../dashboards/sdk.json) v1.0+ dashboard.\n\n## Workflow `Workers`\n\nCheck them.\n";
    let dashboard = "{\"uid\": \"sdk-v1\", \"title\": \"SDK v1.0\", \"panels\": [\n  {\"id\": 7, \"type\": \"timeseries\", \"title\": \"Free Slots\"}]}";
    write("runbooks/slots.md", runbook);
    write("dashboards/sdk.json", dashboard);
    let roots: std::collections::BTreeMap<kyumei_domain::SourceId, std::path::PathBuf> =
        [("ops".into(), root.clone())].into();
    let outcomes = |roots: &std::collections::BTreeMap<_, _>| {
        verify::verify_references(pack, roots)
            .into_iter()
            .map(|c| c.outcome)
            .collect::<Vec<_>>()
    };
    assert_eq!(outcomes(&roots), vec![O::Confirmed, O::Confirmed]);

    // The declaration moved off its line.
    write(
        "runbooks/slots.md",
        &runbook.replace("\nNeeds", "\n\nNeeds"),
    );
    assert!(matches!(outcomes(&roots)[0], O::VersionChanged { .. }));
    let _ = std::fs::remove_dir_all(&root);

    // Versions that can't be right are refused.
    let cases = [
        (
            versioned.replace(
                "excerpt = \"[SDK](../dashboards/sdk.json) v1.0+\"",
                "excerpt = \"[SDK](../dashboards/sdk.json)\"",
            ),
            "is not in its excerpt",
        ),
        (
            versioned.replace(
                "of = \"needs-dashboard\", dashboard = \"SDK\", ",
                "of = \"dashboard\", ",
            ),
            "only a panel reference carries its dashboard's own version",
        ),
        (
            versioned.replace("dashboard = \"SDK\", ", ""),
            "a needed dashboard version names its dashboard",
        ),
    ];
    for (references, expected) in cases {
        let problems = problems(pack_with_references(&references).expect_err(expected));
        assert!(
            problems.iter().any(|p| p.contains(expected)),
            "expected `{expected}` in {problems:#?}"
        );
    }
}
