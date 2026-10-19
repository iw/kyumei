//! Pack validation.
//!
//! Loading fails loudly: every problem found is reported, with enough context
//! to fix it. Problems include dangling references, label constraints the
//! metric does not declare, unverifiable "verified" claims, hypotheses no rule
//! can move, requests that cannot discriminate anything, and contradictory
//! rules.

use std::collections::{BTreeMap, BTreeSet};

use kyumei_domain::{
    ClaimCategory, Effect, HypothesisId, ServerVersion, SignalId, VerificationMethod,
};
use regex::RegexBuilder;

use crate::{
    condition::Condition,
    model::{
        Claim, DiagnosticDef, Instrument, Pack, ReferenceKind, RuleDef, SourceKind,
        VerificationStatusDef, VersionOf,
    },
    template,
};

pub(crate) fn validate(pack: &Pack) -> Vec<String> {
    let mut problems = Vec::new();
    validate_manifest(pack, &mut problems);
    for claim in pack.claims.values() {
        validate_claim(pack, claim, &mut problems);
    }
    let mut seen_diagnostics = BTreeSet::new();
    let mut seen_signatures = BTreeSet::new();
    for diagnostic in &pack.diagnostics {
        if !seen_diagnostics.insert(diagnostic.id.clone()) {
            problems.push(format!("duplicate diagnostic `{}`", diagnostic.id));
        }
        for symptom in &diagnostic.symptoms {
            if !seen_signatures.insert(symptom.id.clone()) {
                problems.push(format!(
                    "symptom signature `{}` is defined by more than one diagnostic",
                    symptom.id
                ));
            }
        }
        validate_diagnostic(pack, diagnostic, &mut problems);
    }
    validate_references(pack, &mut problems);
    problems
}

/// References cite a repository pinned by commit, name a file of the right kind, and
/// link only to what the diagnostics define.
fn validate_references(pack: &Pack, problems: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    for r in &pack.references {
        let at = format!("reference `{}`", r.id);
        if !seen.insert(&r.id) {
            problems.push(format!("duplicate {at}"));
        }
        if r.title.trim().is_empty() {
            problems.push(format!("{at}: no title"));
        }
        if r.why.trim().is_empty() {
            problems.push(format!("{at}: does not say why it bears on what it links"));
        }
        match pack.sources.get(&r.source) {
            None => problems.push(format!("{at}: unknown source `{}`", r.source)),
            Some(s) if s.kind != SourceKind::Repository || s.commit.is_none() => {
                problems.push(format!(
                    "{at}: source `{}` must be a repository pinned by commit",
                    r.source
                ))
            }
            Some(_) => {}
        }
        if r.path.trim().is_empty() || r.path.starts_with('/') || r.path.contains("..") {
            problems.push(format!(
                "{at}: `{}` is not a path within the source",
                r.path
            ));
        }
        if let Some(v) = &r.version {
            if v.version.trim().is_empty() || !v.excerpt.contains(v.version.trim_end_matches('+')) {
                problems.push(format!(
                    "{at}: version `{}` is not in its excerpt `{}`",
                    v.version, v.excerpt
                ));
            }
            if v.line == 0
                || v.path.trim().is_empty()
                || v.path.starts_with('/')
                || v.path.contains("..")
            {
                problems.push(format!(
                    "{at}: the version's path and line must be within the source"
                ));
            }
            match (v.of, &v.dashboard, r.kind) {
                (VersionOf::NeedsDashboard, None, _) => problems.push(format!(
                    "{at}: a needed dashboard version names its dashboard"
                )),
                (VersionOf::Dashboard, _, kind) if kind != ReferenceKind::Panel => problems.push(
                    format!("{at}: only a panel reference carries its dashboard's own version"),
                ),
                _ => {}
            }
        }
        match (r.kind, &r.panel, &r.section) {
            (ReferenceKind::Panel, None, _) => {
                problems.push(format!("{at}: a panel reference names its `panel`"));
            }
            (ReferenceKind::Panel, Some(_), Some(_)) => {
                problems.push(format!("{at}: a panel has no section"));
            }
            (ReferenceKind::Panel, Some(_), None) if !r.path.ends_with(".json") => {
                problems.push(format!("{at}: a panel's path is its dashboard's JSON file"));
            }
            (ReferenceKind::Runbook | ReferenceKind::Playbook, Some(_), _) => {
                problems.push(format!("{at}: only a panel reference names a panel"));
            }
            (ReferenceKind::Runbook | ReferenceKind::Playbook, None, _)
                if !r.path.ends_with(".md") =>
            {
                problems.push(format!("{at}: a runbook or playbook is a Markdown file"));
            }
            _ => {}
        }
        if r.links.is_empty() {
            problems.push(format!("{at}: links nothing"));
        }
        for link in &r.links {
            let Some(d) = pack.diagnostics.iter().find(|d| d.id == link.diagnostic) else {
                problems.push(format!("{at}: unknown diagnostic `{}`", link.diagnostic));
                continue;
            };
            for c in &link.conclusions {
                if d.conclusion(c).is_none() {
                    problems.push(format!("{at}: `{}` has no conclusion `{c}`", d.id));
                }
            }
            for h in &link.hypotheses {
                if d.hypothesis(h).is_none() {
                    problems.push(format!("{at}: `{}` has no hypothesis `{h}`", d.id));
                }
            }
            for q in &link.requests {
                if d.request(q).is_none() {
                    problems.push(format!("{at}: `{}` has no request `{q}`", d.id));
                }
            }
        }
    }
}

fn validate_variants(
    pack: &Pack,
    d: &crate::model::DynamicConfigDef,
    at: &str,
    problems: &mut Vec<String>,
) {
    let mut seen: BTreeSet<&String> = d.scope.server.iter().collect();
    for v in &d.variants {
        if v.server.is_empty() {
            problems.push(format!("{at}: a variant names no release"));
        }
        for release in &v.server {
            if !pack.manifest.swept_server_versions.contains(release)
                && !pack.manifest.verified_server_versions.contains(release)
            {
                problems.push(format!(
                    "{at}: variant release `{release}` is neither verified nor swept by this pack"
                ));
            }
            match release.parse::<ServerVersion>() {
                Ok(parsed) if parsed.patch.is_none() => problems.push(format!(
                    "{at}: variant release `{release}` must name a patch release"
                )),
                Ok(_) => {}
                Err(e) => problems.push(format!("{at}: variant: {e}")),
            }
            if !seen.insert(release) {
                problems.push(format!(
                    "{at}: release `{release}` is described twice (in the scope or another variant)"
                ));
            }
        }
        if v.setting == d.setting
            && v.value_type == d.value_type
            && v.default == d.default
            && v.constrained_by == d.constrained_by
        {
            problems.push(format!(
                "{at}: a variant must differ from the setting as the claim states it"
            ));
        }
        if v.provenance.is_empty() {
            problems.push(format!("{at}: a variant needs provenance"));
        }
    }
}

fn validate_manifest(pack: &Pack, problems: &mut Vec<String>) {
    let mut swept = BTreeSet::new();
    for version in &pack.manifest.swept_server_versions {
        match version.parse::<ServerVersion>() {
            Ok(v) if v.patch.is_none() => problems.push(format!(
                "pack: swept server version `{version}` must name a patch release"
            )),
            Ok(_) => {}
            Err(e) => problems.push(format!("pack: {e}")),
        }
        if !swept.insert(version) {
            problems.push(format!(
                "pack: swept server version `{version}` is listed twice"
            ));
        }
    }
    for version in &pack.manifest.verified_server_versions {
        match version.parse::<ServerVersion>() {
            Ok(v) if v.patch.is_none() => problems.push(format!(
                "pack: verified server version `{version}` must name a patch release"
            )),
            Ok(v) if format!("{}.{}", v.major, v.minor) != pack.manifest.server_family => problems
                .push(format!(
                    "pack: verified version `{version}` is outside family `{}`",
                    pack.manifest.server_family
                )),
            Ok(_) => {}
            Err(e) => problems.push(format!("pack: {e}")),
        }
    }
}

fn validate_claim(pack: &Pack, claim: &Claim, problems: &mut Vec<String>) {
    let id = claim.id();
    let at = format!("claim `{id}`");

    // Provenance must point at declared sources.
    for p in claim.all_provenance() {
        if p.sources.is_empty() {
            problems.push(format!("{at}: provenance names no source"));
        }
        let mut seen = BTreeSet::new();
        for source in &p.sources {
            if !pack.sources.contains_key(source) {
                problems.push(format!("{at}: unknown provenance source `{source}`"));
            }
            if !seen.insert(source) {
                problems.push(format!("{at}: provenance names `{source}` twice"));
            }
        }
        if p.line.is_some() && p.path.is_none() {
            problems.push(format!("{at}: provenance has a line but no path"));
        }
    }

    // Categories must match the section a claim was declared in.
    match claim {
        Claim::Metric(m) => {
            if !m.category.is_metric() {
                problems.push(format!(
                    "{at}: `[[metric]]` with non-metric category `{}`",
                    m.category
                ));
            }
            if m.exposition.is_empty() {
                problems.push(format!("{at}: metric has no exposition names"));
            }
            for e in &m.exposition {
                if m.category == ClaimCategory::SdkMetric && e.sdk_family.is_none() {
                    problems.push(format!(
                        "{at}: SDK metric exposition `{}` must name its sdk_family",
                        e.name
                    ));
                }
                if m.category != ClaimCategory::SdkMetric && e.sdk_family.is_some() {
                    problems.push(format!(
                        "{at}: server metric exposition `{}` must not name an sdk_family",
                        e.name
                    ));
                }
                if matches!(m.instrument, Instrument::Histogram | Instrument::Timer)
                    && e.unit.is_none()
                {
                    problems.push(format!(
                        "{at}: histogram exposition `{}` must state its unit",
                        e.name
                    ));
                }
            }
            let mut label_names = BTreeSet::new();
            for label in &m.labels {
                if !label_names.insert(&label.name) {
                    problems.push(format!("{at}: duplicate label `{}`", label.name));
                }
            }
            for label in &m.labels {
                if let Some(other) = &label.same_as
                    && (other == &label.name || !m.labels.iter().any(|l| &l.name == other))
                {
                    problems.push(format!(
                        "{at}: label `{}` is the same as `{other}`, which the metric does not have",
                        label.name
                    ));
                }
            }
        }
        Claim::Statement(s) => {
            if s.category.is_metric()
                || matches!(
                    s.category,
                    ClaimCategory::ApiField
                        | ClaimCategory::DynamicConfig
                        | ClaimCategory::DiagnosticHeuristic
                )
            {
                problems.push(format!(
                    "{at}: `[[statement]]` cannot have category `{}`; use the dedicated section",
                    s.category
                ));
            }
        }
        Claim::Heuristic(h) => {
            let authored = h.provenance.iter().flat_map(|p| &p.sources).any(|id| {
                pack.sources
                    .get(id)
                    .is_some_and(|s| s.kind == SourceKind::Authored)
            });
            if !authored {
                problems.push(format!(
                    "{at}: a heuristic must cite an `authored` source so it is never mistaken for Temporal behaviour"
                ));
            }
            for relied in &h.relies_on {
                if !pack.claims.contains_key(relied) {
                    problems.push(format!("{at}: relies on unknown claim `{relied}`"));
                }
            }
        }
        Claim::DynamicConfig(d) => validate_variants(pack, d, &at, problems),
        Claim::ApiField(_) => {}
    }

    // Version scope must parse, and name releases the pack verified or swept;
    // verification must be substantiated.
    if let Some(scope) = claim.scope_def() {
        let known: BTreeSet<&String> = pack
            .manifest
            .verified_server_versions
            .iter()
            .chain(&pack.manifest.swept_server_versions)
            .collect();
        for v in &scope.server {
            if !known.contains(v) {
                problems.push(format!(
                    "{at}: release `{v}` is neither verified nor swept by this pack"
                ));
            }
            match v.parse::<ServerVersion>() {
                Ok(parsed) if parsed.patch.is_none() => problems.push(format!(
                    "{at}: server version `{v}` must name a patch release"
                )),
                Ok(_) => {}
                Err(e) => problems.push(format!("{at}: {e}")),
            }
        }
        if claim.category().is_server() && scope.server.is_empty() {
            problems.push(format!(
                "{at}: server claims must list the server versions verified"
            ));
        }
        if claim.category() == ClaimCategory::SdkMetric && scope.sdk.is_empty() {
            problems.push(format!(
                "{at}: SDK metrics must list the SDK revisions verified"
            ));
        }
    }
    if let Some(v) = claim.verification_def() {
        match v.status {
            VerificationStatusDef::Verified => {
                if v.method.is_none() || v.on.is_none() {
                    problems.push(format!("{at}: verified claims need `method` and `on`"));
                }
                if v.method == Some(VerificationMethod::Authored) {
                    problems.push(format!(
                        "{at}: only heuristics may be `authored`; Temporal claims need a source"
                    ));
                }
                let substantiated = claim
                    .provenance()
                    .iter()
                    .any(|p| (p.path.is_some() && p.line.is_some()) || p.url.is_some());
                if !substantiated {
                    problems.push(format!(
                        "{at}: verified claims need provenance with a path and line, or a URL"
                    ));
                }
                if v.method == Some(VerificationMethod::SourceInspection) {
                    let repo_backed =
                        claim
                            .provenance()
                            .iter()
                            .flat_map(|p| &p.sources)
                            .any(|id| {
                                pack.sources.get(id).is_some_and(|s| {
                                    s.kind == SourceKind::Repository
                                        && (s.commit.is_some() || s.checksum.is_some())
                                })
                            });
                    if !repo_backed {
                        problems.push(format!(
                            "{at}: source inspection must cite a repository source with a commit or checksum"
                        ));
                    }
                }
            }
            VerificationStatusDef::Unverified => {
                if v.reason.is_none() {
                    problems.push(format!("{at}: unverified claims must say why"));
                }
            }
        }
    }
}

fn validate_diagnostic(pack: &Pack, d: &DiagnosticDef, problems: &mut Vec<String>) {
    let at = format!("diagnostic `{}`", d.id);

    let dup = |kind: &str, ids: Vec<String>, problems: &mut Vec<String>| {
        let mut seen = BTreeSet::new();
        for id in ids {
            if !seen.insert(id.clone()) {
                problems.push(format!("{at}: duplicate {kind} `{id}`"));
            }
        }
    };
    dup(
        "stage",
        d.stages.iter().map(|s| s.id.to_string()).collect(),
        problems,
    );
    dup(
        "signal",
        d.signals.iter().map(|s| s.id.to_string()).collect(),
        problems,
    );
    dup(
        "hypothesis",
        d.hypotheses.iter().map(|s| s.id.to_string()).collect(),
        problems,
    );
    dup(
        "request",
        d.requests.iter().map(|s| s.id.to_string()).collect(),
        problems,
    );
    dup(
        "rule",
        d.rules.iter().map(|s| s.id.to_string()).collect(),
        problems,
    );
    dup(
        "conclusion",
        d.conclusions.iter().map(|s| s.id.to_string()).collect(),
        problems,
    );
    dup(
        "simulation",
        d.simulations.iter().map(|s| s.id.to_string()).collect(),
        problems,
    );

    let mut orders = BTreeSet::new();
    for stage in &d.stages {
        if !orders.insert(stage.order) {
            problems.push(format!("{at}: two stages share order {}", stage.order));
        }
    }

    if d.symptoms.is_empty() {
        problems.push(format!("{at}: no symptom signatures"));
    }
    for s in &d.symptoms {
        if d.signal(&s.signal).is_none() {
            problems.push(format!(
                "{at}: symptom `{}` names unknown signal `{}`",
                s.id, s.signal
            ));
        }
        for p in &s.patterns {
            if let Err(e) = RegexBuilder::new(p).case_insensitive(true).build() {
                problems.push(format!("{at}: symptom `{}` pattern `{p}`: {e}", s.id));
            }
        }
    }

    // Signals: sources must be real claims whose declared labels allow the
    // constraints (this is what catches `num_pollers{worker_type=...}`).
    for signal in &d.signals {
        let sat = format!("{at}: signal `{}`", signal.id);
        let simulated = d.simulation_producing(&signal.id).is_some();
        if signal.source.is_empty() && !simulated {
            problems.push(format!("{sat}: no sources"));
        }
        if !signal.source.is_empty() && simulated {
            problems.push(format!(
                "{sat}: a simulation produces it, so it can have no other source"
            ));
        }
        for source in &signal.source {
            let Some(claim) = pack.claims.get(&source.claim) else {
                problems.push(format!("{sat}: unknown claim `{}`", source.claim));
                continue;
            };
            match claim {
                Claim::Metric(m) => {
                    for (label, value) in &source.labels {
                        match m.labels.iter().find(|l| &l.name == label) {
                            None => problems.push(format!(
                                "{sat}: metric `{}` does not declare label `{label}`",
                                m.name
                            )),
                            Some(def) if !def.values.is_empty() && !def.values.contains(value) => {
                                problems.push(format!(
                                    "{sat}: `{value}` is not a declared value of `{}` label `{label}` (declared: {})",
                                    m.name,
                                    def.values.join(", ")
                                ))
                            }
                            Some(_) => {}
                        }
                    }
                    if source.transform.is_some() && m.instrument != Instrument::Counter {
                        problems.push(format!(
                            "{sat}: `rate` transform on non-counter `{}`",
                            m.name
                        ));
                    }
                }
                Claim::ApiField(_) | Claim::DynamicConfig(_) => {
                    if !source.labels.is_empty() {
                        problems.push(format!(
                            "{sat}: label constraints only apply to metrics (`{}` is not a metric)",
                            source.claim
                        ));
                    }
                }
                Claim::Statement(_) | Claim::Heuristic(_) => problems.push(format!(
                    "{sat}: `{}` is not a metric, API field or configuration key",
                    source.claim
                )),
            }
        }
    }

    // The system path: every hypothesis must sit on it.
    let mut layers = BTreeSet::new();
    if d.path.layers.is_empty() {
        problems.push(format!("{at}: the system path has no layers"));
    }
    for layer in &d.path.layers {
        if !layers.insert(*layer) {
            problems.push(format!(
                "{at}: layer `{}` appears twice on the system path",
                layer.token()
            ));
        }
    }
    for layer in d.path.labels.keys() {
        if !layers.contains(layer) {
            problems.push(format!(
                "{at}: label for layer `{}`, which is not on the system path",
                layer.token()
            ));
        }
    }

    // Hypotheses.
    for h in &d.hypotheses {
        if !layers.contains(&h.layer) {
            problems.push(format!(
                "{at}: hypothesis `{}` is in layer `{}`, which is not on the system path",
                h.id,
                h.layer.token()
            ));
        }
        if d.stage(&h.stage).is_none() {
            problems.push(format!(
                "{at}: hypothesis `{}` in unknown stage `{}`",
                h.id, h.stage
            ));
        }
        if let Some(activation) = &h.activation {
            check_condition(
                d,
                activation,
                &format!("{at}: hypothesis `{}` activation", h.id),
                problems,
            );
            if activation.hypotheses().contains(&h.id) {
                problems.push(format!("{at}: hypothesis `{}` activates on itself", h.id));
            }
        }
    }
    for stage in &d.stages {
        let primaries = d
            .hypotheses
            .iter()
            .filter(|h| h.stage == stage.id && h.primary)
            .count();
        if primaries != 1 {
            problems.push(format!(
                "{at}: stage `{}` must have exactly one primary hypothesis (has {primaries})",
                stage.id
            ));
        }
    }

    // Rules.
    let mut movers: BTreeMap<HypothesisId, Vec<Effect>> = BTreeMap::new();
    for rule in &d.rules {
        let rat = format!("{at}: rule `{}`", rule.id);
        if d.hypothesis(&rule.hypothesis).is_none() {
            problems.push(format!("{rat}: unknown hypothesis `{}`", rule.hypothesis));
        }
        check_condition(d, &rule.when, &rat, problems);
        if rule.when.hypotheses().contains(&rule.hypothesis) {
            problems.push(format!(
                "{rat}: a rule cannot depend on the state it changes"
            ));
        }
        if rule.when.signals().is_empty() {
            problems.push(format!("{rat}: a rule must rest on at least one signal"));
        }
        check_template(d, &rule.statement, Some(&rule.when), &rat, problems);
        for claim in &rule.claims {
            if !pack.claims.contains_key(claim) {
                problems.push(format!("{rat}: unknown claim `{claim}`"));
            }
        }
        movers
            .entry(rule.hypothesis.clone())
            .or_default()
            .push(rule.effect);
    }
    for h in &d.hypotheses {
        if !movers.contains_key(&h.id) {
            problems.push(format!(
                "{at}: hypothesis `{}` can never be resolved: no rule affects it",
                h.id
            ));
        }
    }
    // Contradictions: identical conditions with opposing effects.
    for (i, a) in d.rules.iter().enumerate() {
        for b in d.rules.iter().skip(i + 1) {
            if a.hypothesis == b.hypothesis
                && a.when == b.when
                && a.effect.is_positive() != b.effect.is_positive()
            {
                problems.push(format!(
                    "{at}: rules `{}` and `{}` have identical conditions but opposing effects on `{}`",
                    a.id, b.id, a.hypothesis
                ));
            }
        }
    }

    // Requests: must reference real things and be able to discriminate.
    let mut producible: BTreeSet<SignalId> = BTreeSet::new();
    for request in &d.requests {
        let qat = format!("{at}: request `{}`", request.id);
        if d.stage(&request.stage).is_none() {
            problems.push(format!("{qat}: unknown stage `{}`", request.stage));
        }
        if !(1..=5).contains(&request.cost) {
            problems.push(format!("{qat}: cost must be 1–5"));
        }
        if request.collect.is_empty() {
            problems.push(format!("{qat}: no collection instructions"));
        }
        for s in &request.signals {
            if d.signal(s).is_none() {
                problems.push(format!("{qat}: unknown signal `{s}`"));
            }
            producible.insert(s.clone());
        }
        for h in &request.distinguishes {
            if d.hypothesis(h).is_none() {
                problems.push(format!("{qat}: unknown hypothesis `{h}`"));
                continue;
            }
            let can_move = d.rules.iter().any(|r| {
                &r.hypothesis == h && r.when.signals().iter().any(|s| request.signals.contains(s))
            });
            if !can_move {
                problems.push(format!(
                    "{qat}: claims to distinguish `{h}` but no rule on `{h}` reads its signals"
                ));
            }
        }
        for c in &request.collect {
            for claim in &c.claims {
                if !pack.claims.contains_key(claim) {
                    problems.push(format!(
                        "{qat}: collection `{}` cites unknown claim `{claim}`",
                        c.label
                    ));
                }
            }
            if let Some(zero) = &c.absent_reads_zero {
                if c.method != kyumei_domain::CollectionMethod::Promql {
                    problems.push(format!(
                        "{qat}: collection `{}` has a witness, but only a PromQL query can",
                        c.label
                    ));
                }
                if !pack.claims.contains_key(&zero.because) {
                    problems.push(format!(
                        "{qat}: collection `{}` reads no series as zero because of unknown claim `{}`",
                        c.label, zero.because
                    ));
                }
                if zero.witness.trim().is_empty() || zero.witness.trim() == c.body.trim() {
                    problems.push(format!(
                        "{qat}: collection `{}` needs a witness query other than its own",
                        c.label
                    ));
                }
            }
        }
    }
    // Simulations: a model's inputs from observed facts, a signal of their own,
    // and rules that can only support or weaken.
    for sim in &d.simulations {
        let mat = format!("{at}: simulation `{}`", sim.id);
        if d.hypothesis(&sim.hypothesis).is_none() {
            problems.push(format!("{mat}: unknown hypothesis `{}`", sim.hypothesis));
        }
        for role in sim.model.required() {
            if !sim.inputs.contains_key(*role) {
                problems.push(format!(
                    "{mat}: model `{}` needs input `{role}`",
                    sim.model.token()
                ));
            }
        }
        for (role, input) in &sim.inputs {
            if !sim.model.required().contains(&role.as_str())
                && !sim.model.optional().contains(&role.as_str())
            {
                problems.push(format!(
                    "{mat}: model `{}` takes no input `{role}`",
                    sim.model.token()
                ));
            }
            if d.signal(&input.signal).is_none() {
                problems.push(format!(
                    "{mat}: input `{role}` reads unknown signal `{}`",
                    input.signal
                ));
            } else if d.simulation_producing(&input.signal).is_some() {
                problems.push(format!(
                    "{mat}: input `{role}` reads `{}`, which a simulation produces",
                    input.signal
                ));
            }
        }
        if d.signal(&sim.produces).is_none() {
            problems.push(format!("{mat}: produces unknown signal `{}`", sim.produces));
        }
        if d.simulations
            .iter()
            .filter(|o| o.produces == sim.produces)
            .count()
            > 1
        {
            problems.push(format!(
                "{mat}: another simulation produces `{}` too",
                sim.produces
            ));
        }
        for claim in &sim.claims {
            if !pack.claims.contains_key(claim) {
                problems.push(format!("{mat}: unknown claim `{claim}`"));
            }
        }
        let readers: Vec<&RuleDef> = d
            .rules
            .iter()
            .filter(|r| r.when.signals().contains(&sim.produces))
            .collect();
        if readers.is_empty() {
            problems.push(format!(
                "{mat}: no rule reads `{}`, so the simulation could never count",
                sim.produces
            ));
        }
        for r in readers {
            if matches!(r.effect, Effect::Establishes | Effect::Eliminates) {
                problems.push(format!(
                    "{at}: rule `{}` reads simulated `{}`, so it can only support or weaken",
                    r.id, sim.produces
                ));
            }
        }
        producible.insert(sim.produces.clone());
    }
    for rule in &d.rules {
        for s in rule.when.signals() {
            let from_symptom = d.symptoms.iter().any(|sym| sym.signal == s);
            if !producible.contains(&s) && !from_symptom {
                problems.push(format!(
                    "{at}: rule `{}` reads `{s}`, which no request collects",
                    rule.id
                ));
            }
        }
    }

    // Conclusions.
    for c in &d.conclusions {
        let cat = format!("{at}: conclusion `{}`", c.id);
        check_condition(d, &c.when, &cat, problems);
        if !c.when.reads_hypotheses() {
            problems.push(format!(
                "{cat}: a conclusion must rest on hypothesis states"
            ));
        }
        check_template(d, &c.statement, None, &cat, problems);
        for clause in &c.clauses {
            check_condition(d, &clause.when, &cat, problems);
            check_template(d, &clause.text, None, &cat, problems);
        }
        for r in &c.recommendations {
            for claim in &r.claims {
                if !pack.claims.contains_key(claim) {
                    problems.push(format!(
                        "{cat}: recommendation cites unknown claim `{claim}`"
                    ));
                }
            }
        }
    }
}

fn check_condition(d: &DiagnosticDef, c: &Condition, at: &str, problems: &mut Vec<String>) {
    for s in c.signal_stats() {
        if d.signal(&s.signal).is_none() {
            problems.push(format!("{at}: unknown signal `{}`", s.signal));
        }
    }
    for h in c.hypotheses() {
        if d.hypothesis(&h).is_none() {
            problems.push(format!("{at}: unknown hypothesis `{h}`"));
        }
    }
}

fn check_template(
    d: &DiagnosticDef,
    text: &str,
    condition: Option<&Condition>,
    at: &str,
    problems: &mut Vec<String>,
) {
    match template::placeholders(text) {
        Err(e) => problems.push(format!("{at}: statement placeholder: {e}")),
        Ok(found) => {
            for p in found {
                if d.signal(&p.signal).is_none() {
                    problems.push(format!(
                        "{at}: statement uses unknown signal `{}`",
                        p.signal
                    ));
                }
                if let Some(condition) = condition
                    && !condition.signals().contains(&p.signal)
                {
                    problems.push(format!(
                        "{at}: statement quotes `{}` but the rule's condition does not read it",
                        p.signal
                    ));
                }
            }
        }
    }
}
