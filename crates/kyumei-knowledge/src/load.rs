//! Reading pack files.

use std::collections::BTreeMap;

use kyumei_domain::DiagnosticId;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    LoadError,
    model::{
        AnnotationDef, ApiFieldDef, Claim, ConclusionDef, DiagnosticDef, DynamicConfigDef,
        HeuristicDef, HypothesisDef, MetricDef, Pack, PackManifest, PathDef, ReferenceDef,
        RequestDef, RuleDef, SignalDef, SimulationDef, SourceDef, StageDef, StatementDef,
        SymptomDef,
    },
    validate,
};

/// Everything a single pack file may contain.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    pack: Option<PackManifest>,
    #[serde(default)]
    source: Vec<SourceDef>,
    #[serde(default)]
    metric: Vec<MetricDef>,
    #[serde(default)]
    dynamic_config: Vec<DynamicConfigDef>,
    #[serde(default)]
    api_field: Vec<ApiFieldDef>,
    #[serde(default)]
    statement: Vec<StatementDef>,
    #[serde(default)]
    heuristic: Vec<HeuristicDef>,
    #[serde(default)]
    annotation: Vec<AnnotationDef>,
    #[serde(default)]
    diagnostic: Option<DiagnosticHeader>,
    #[serde(default)]
    symptom: Vec<SymptomDef>,
    #[serde(default)]
    stage: Vec<StageDef>,
    #[serde(default)]
    signal: Vec<SignalDef>,
    #[serde(default)]
    hypothesis: Vec<HypothesisDef>,
    #[serde(default)]
    request: Vec<RequestDef>,
    #[serde(default)]
    rule: Vec<RuleDef>,
    #[serde(default)]
    conclusion: Vec<ConclusionDef>,
    #[serde(default)]
    simulation: Vec<SimulationDef>,
    #[serde(default)]
    reference: Vec<ReferenceDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticHeader {
    id: DiagnosticId,
    title: String,
    summary: String,
    /// Symptoms are matched in order of precedence, lowest first.
    #[serde(default = "default_precedence")]
    precedence: u32,
    path: PathDef,
}

fn default_precedence() -> u32 {
    100
}

impl RawFile {
    fn has_diagnostic_sections(&self) -> bool {
        !(self.symptom.is_empty()
            && self.stage.is_empty()
            && self.signal.is_empty()
            && self.hypothesis.is_empty()
            && self.request.is_empty()
            && self.rule.is_empty()
            && self.conclusion.is_empty()
            && self.simulation.is_empty())
    }
}

/// Assemble one pack from its files. `files` are `(path, contents)` with
/// paths relative to the knowledge root; the first path component(s) up to
/// `pack.toml` form the pack root.
pub(crate) fn load_pack(root: &str, files: &[(&str, &str)]) -> Result<Pack, LoadError> {
    let mut manifest = None;
    let mut sources = BTreeMap::new();
    let mut claims: BTreeMap<_, Claim> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut problems = Vec::new();
    let mut annotations: Vec<(String, AnnotationDef)> = Vec::new();
    let mut references = Vec::new();
    let mut hasher = Sha256::new();
    let mut names = Vec::new();

    for (path, contents) in files {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(contents.as_bytes());
        hasher.update([0]);
        names.push((*path).to_owned());

        let raw: RawFile = toml::from_str(contents).map_err(|e| LoadError::Parse {
            path: (*path).to_owned(),
            message: e.to_string(),
        })?;

        if let Some(m) = raw.pack.clone() {
            if manifest.is_some() {
                problems.push(format!("{path}: a second [pack] manifest"));
            }
            manifest = Some(m);
        }
        for source in raw.source.iter().cloned() {
            if sources.insert(source.id.clone(), source.clone()).is_some() {
                problems.push(format!("{path}: duplicate source `{}`", source.id));
            }
        }
        let mut add = |claim: Claim| {
            let id = claim.id().clone();
            if claims.insert(id.clone(), claim).is_some() {
                problems.push(format!("{path}: duplicate claim `{id}`"));
            }
        };
        raw.metric
            .iter()
            .cloned()
            .for_each(|m| add(Claim::Metric(m)));
        raw.dynamic_config
            .iter()
            .cloned()
            .for_each(|d| add(Claim::DynamicConfig(d)));
        raw.api_field
            .iter()
            .cloned()
            .for_each(|a| add(Claim::ApiField(a)));
        raw.statement
            .iter()
            .cloned()
            .for_each(|s| add(Claim::Statement(s)));
        raw.heuristic
            .iter()
            .cloned()
            .for_each(|h| add(Claim::Heuristic(h)));
        annotations.extend(
            raw.annotation
                .iter()
                .map(|a| ((*path).to_owned(), a.clone())),
        );
        references.extend(raw.reference.iter().cloned());

        match (&raw.diagnostic, raw.has_diagnostic_sections()) {
            (Some(header), _) => diagnostics.push(DiagnosticDef {
                id: header.id.clone(),
                title: header.title.clone(),
                summary: header.summary.clone(),
                precedence: header.precedence,
                path: header.path.clone(),
                symptoms: raw.symptom,
                stages: raw.stage,
                signals: raw.signal,
                hypotheses: raw.hypothesis,
                requests: raw.request,
                rules: raw.rule,
                conclusions: raw.conclusion,
                simulations: raw.simulation,
            }),
            (None, true) => problems.push(format!(
                "{path}: diagnostic sections without a [diagnostic] header"
            )),
            (None, false) => {}
        }
    }

    // Annotations apply once every file's claims are known, whatever the file order.
    for (path, a) in &annotations {
        if a.is_empty() {
            problems.push(format!("{path}: annotation for `{}` adds nothing", a.claim));
            continue;
        }
        match claims.get_mut(&a.claim) {
            None => problems.push(format!(
                "{path}: annotation for unknown claim `{}`",
                a.claim
            )),
            Some(claim) => {
                if let Err(why) = claim.annotate(a) {
                    problems.push(format!("{path}: annotation for `{}`: {why}", a.claim));
                }
            }
        }
    }

    let manifest = manifest.ok_or_else(|| LoadError::Invalid {
        at: root.to_owned(),
        message: "no pack.toml with a [pack] manifest".to_owned(),
    })?;

    // A description is matched against symptoms in this order.
    diagnostics.sort_by(|a, b| (a.precedence, &a.id).cmp(&(b.precedence, &b.id)));

    let pack = Pack {
        manifest,
        digest: hex(&hasher.finalize()),
        sources,
        claims,
        diagnostics,
        references,
        files: names,
    };

    problems.extend(validate::validate(&pack));
    if problems.is_empty() {
        Ok(pack)
    } else {
        Err(LoadError::Inconsistent {
            pack: root.to_owned(),
            problems,
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
