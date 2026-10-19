//! The knowledge pack model.
//!
//! Types in this module are deserialised directly from pack files with
//! `deny_unknown_fields`, so a misspelt key is a load error rather than a
//! silently ignored setting.

use std::collections::BTreeMap;

use jiff::civil::Date;
use kyumei_domain::{
    ClaimCategory, ClaimId, ConclusionId, DiagnosticId, HypothesisId, PackId, Provenance,
    ReferenceId, RequestDefinitionId, RuleId, SdkFamily, SdkScope, ServerVersion, SignalId,
    SimulationId, SourceId, StageId, SymptomSignatureId, Unit, Verification, VerificationMethod,
    VersionScope,
};
use serde::{Deserialize, Serialize};

use crate::condition::{Condition, SignalStat};

/// `pack.toml`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    pub id: PackId,
    pub title: String,
    /// Content version of the pack itself.
    pub version: String,
    /// The Temporal Server minor version family the pack targets, e.g. "1.31".
    pub server_family: String,
    /// Exact server versions whose source was inspected.
    pub verified_server_versions: Vec<String>,
    /// Releases the version sweep generated claims for: their settings' and metrics' names,
    /// types and defaults are checked, though not every behaviour was read for them.
    #[serde(default)]
    pub swept_server_versions: Vec<String>,
    pub description: String,
}

/// A provenance source: a repository revision, a document, or Kyumei's own
/// authored heuristics.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDef {
    pub id: SourceId,
    pub kind: SourceKind,
    pub title: String,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// Tag or version, e.g. `v1.31.0`.
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    /// An immutable content checksum when there is no commit (e.g. a Go
    /// module's `h1:` hash from go.sum).
    #[serde(default)]
    pub checksum: Option<String>,
    /// When the source was inspected.
    #[serde(deserialize_with = "crate::dates::date")]
    pub retrieved: Date,
    /// When the pinned commit was made, for sources cited by commit.
    #[serde(default, deserialize_with = "crate::dates::option_date")]
    pub committed: Option<Date>,
    #[serde(default)]
    pub note: Option<String>,
}

impl SourceDef {
    /// Where to read a file of the source, and a section of it, at the pinned commit (or
    /// revision). Only GitHub repositories have such a link.
    pub fn file_url(&self, path: &str, anchor: Option<&str>) -> Option<String> {
        let repository = self.repository.as_deref()?.trim_end_matches('/');
        if !repository.starts_with("https://github.com/") {
            return None;
        }
        let at = self.commit.as_deref().or(self.revision.as_deref())?;
        Some(match anchor {
            Some(a) => format!("{repository}/blob/{at}/{path}#{a}"),
            None => format!("{repository}/blob/{at}/{path}"),
        })
    }

    /// The commit, shortened as Git shortens it.
    pub fn short_commit(&self) -> Option<&str> {
        self.commit.as_deref().map(|c| &c[..c.len().min(7)])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    Repository,
    Documentation,
    Authored,
}

/// Version scope as written in pack files.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeDef {
    #[serde(default)]
    pub server: Vec<String>,
    #[serde(default)]
    pub sdk: Vec<SdkScopeDef>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkScopeDef {
    pub family: SdkFamily,
    pub revision: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationDef {
    pub status: VerificationStatusDef,
    #[serde(default)]
    pub method: Option<VerificationMethod>,
    #[serde(default, deserialize_with = "crate::dates::option_date")]
    pub on: Option<Date>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerificationStatusDef {
    Verified,
    Unverified,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvenanceDef {
    /// The source, or every source (such as several releases) where the same path, line
    /// and excerpt hold. Written `source = "id"` or `source = ["id", …]`.
    #[serde(rename = "source", deserialize_with = "one_or_many")]
    pub sources: Vec<SourceId>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub excerpt: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

impl ProvenanceDef {
    pub fn to_domain(&self) -> Provenance {
        Provenance {
            sources: self.sources.clone(),
            path: self.path.clone(),
            line: self.line,
            url: self.url.clone(),
            excerpt: self.excerpt.clone(),
            note: self.note.clone(),
        }
    }
}

/// How a metric is recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Instrument {
    Counter,
    Gauge,
    /// A histogram (Prometheus `_bucket`/`_sum`/`_count` series).
    Histogram,
    /// A server timer, exported as a histogram by the Prometheus reporter.
    Timer,
}

impl Instrument {
    pub fn label(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Histogram => "histogram",
            Self::Timer => "timer (histogram)",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabelDef {
    pub name: String,
    /// Enumerated values, when the source defines them.
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub purpose: Option<String>,
    /// Another label of the same metric that always carries the same value, such as
    /// `operation` kept beside `task_type` for compatibility.
    #[serde(default)]
    pub same_as: Option<String>,
}

/// One way a metric appears in an exporter.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpositionDef {
    /// The exposed series name, without `_bucket`/`_sum`/`_count`.
    pub name: String,
    /// Which SDK implementation exposes it this way (SDK metrics only).
    #[serde(default)]
    pub sdk_family: Option<SdkFamily>,
    /// The unit of exposed values (durations differ by exporter).
    #[serde(default)]
    pub unit: Option<Unit>,
    /// When this exposition applies, e.g. the exporter configuration.
    pub condition: String,
}

/// A metric definition: SDK or server.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricDef {
    pub id: ClaimId,
    /// The metric name as defined in source, without exporter prefixes.
    pub name: String,
    pub category: ClaimCategory,
    pub instrument: Instrument,
    #[serde(default)]
    pub unit: Option<Unit>,
    /// What the metric measures. A generated claim carries the source's description, or
    /// nothing when the source has none and no annotation supplies one.
    #[serde(default)]
    pub purpose: Option<String>,
    /// The Go identifier that defines a server metric, e.g. `metrics.ServiceRequests`.
    #[serde(default)]
    pub go_name: Option<String>,
    /// The packages whose code records a server metric (generated).
    #[serde(default)]
    pub recorded_in: Vec<String>,
    #[serde(default)]
    pub labels: Vec<LabelDef>,
    #[serde(default)]
    pub exposition: Vec<ExpositionDef>,
    #[serde(default)]
    pub notes: Vec<String>,
    pub scope: ScopeDef,
    pub verification: VerificationDef,
    #[serde(default)]
    pub provenance: Vec<ProvenanceDef>,
}

/// A dynamic configuration key.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicConfigDef {
    pub id: ClaimId,
    pub key: String,
    /// The setting constructor in source, e.g. `NewTaskQueueIntSetting`.
    pub setting: String,
    pub value_type: String,
    pub default: String,
    /// What the key can be constrained by, e.g. `task-queue`.
    pub constrained_by: String,
    pub description: String,
    #[serde(default)]
    pub notes: Vec<String>,
    pub scope: ScopeDef,
    pub verification: VerificationDef,
    #[serde(default)]
    pub provenance: Vec<ProvenanceDef>,
    /// How the setting differed in releases outside its scope (the version sweep).
    #[serde(default, rename = "variant")]
    pub variants: Vec<DynamicConfigVariantDef>,
}

/// A setting as it was in other releases: a different constructor, type, default or
/// constraint keys.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicConfigVariantDef {
    /// The releases it describes.
    pub server: Vec<String>,
    pub setting: String,
    pub value_type: String,
    pub default: String,
    pub constrained_by: String,
    #[serde(default)]
    pub provenance: Vec<ProvenanceDef>,
}

impl DynamicConfigDef {
    /// The variant describing `version`, when the setting differed there.
    pub fn variant_for(&self, version: &ServerVersion) -> Option<&DynamicConfigVariantDef> {
        self.variants.iter().find(|v| {
            v.server
                .iter()
                .filter_map(|s| s.parse::<ServerVersion>().ok())
                .any(|s| s.exactly(version))
        })
    }
}

impl DynamicConfigVariantDef {
    /// How the variant differs from the setting as its claim states it.
    pub fn difference(&self, main: &DynamicConfigDef) -> String {
        let mut parts = Vec::new();
        if self.default != main.default {
            parts.push(format!("default {}", self.default));
        }
        if self.value_type != main.value_type {
            parts.push(format!("type {}", self.value_type));
        }
        if self.constrained_by != main.constrained_by {
            parts.push(format!("constrained by {}", self.constrained_by));
        }
        if self.setting != main.setting && parts.is_empty() {
            parts.push(format!("constructor {}", self.setting));
        }
        parts.join(", ")
    }
}

/// An API response field (never a metric).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiFieldDef {
    pub id: ClaimId,
    /// Fully qualified proto message, e.g. `temporal.api.taskqueue.v1.TaskQueueStats`.
    pub message: String,
    pub field: String,
    /// The JSON (proto3 canonical) name, as it appears in `-o json` output.
    pub json_name: String,
    /// The dotted JSON path within the response when the name alone is
    /// ambiguous, e.g. `effectiveRateLimit.requestsPerSecond`.
    #[serde(default)]
    pub json_path: Option<String>,
    pub field_type: String,
    #[serde(default)]
    pub unit: Option<Unit>,
    pub purpose: String,
    /// RPCs that return the field.
    #[serde(default)]
    pub returned_by: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    pub scope: ScopeDef,
    pub verification: VerificationDef,
    #[serde(default)]
    pub provenance: Vec<ProvenanceDef>,
}

/// A statement about Temporal behaviour, a limit, an SDK configuration option
/// or a CLI command.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatementDef {
    pub id: ClaimId,
    pub category: ClaimCategory,
    pub title: String,
    pub statement: String,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub scope: ScopeDef,
    pub verification: VerificationDef,
    #[serde(default)]
    pub provenance: Vec<ProvenanceDef>,
}

/// A diagnostic heuristic chosen by Kyumei's authors.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeuristicDef {
    pub id: ClaimId,
    pub title: String,
    pub statement: String,
    pub rationale: String,
    /// When the heuristic was written or last reviewed.
    #[serde(deserialize_with = "crate::dates::date")]
    pub authored: Date,
    #[serde(default)]
    pub relies_on: Vec<ClaimId>,
    #[serde(default)]
    pub provenance: Vec<ProvenanceDef>,
}

/// Any knowledge claim.
#[derive(Clone, Debug)]
pub enum Claim {
    Metric(MetricDef),
    DynamicConfig(DynamicConfigDef),
    ApiField(ApiFieldDef),
    Statement(StatementDef),
    Heuristic(HeuristicDef),
}

/// Notes and extra provenance for a claim defined in another file. Generated files are
/// never edited, so judgement about their claims lives in annotations, which the loader
/// merges into the claim.
///
/// A metric annotation can also fill in what a definition leaves out: labels (tags are
/// attached where a metric is recorded, not where it is defined), a purpose when the source
/// gives no description, and a unit when the definition states none. It never replaces what
/// the source says.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationDef {
    pub claim: ClaimId,
    /// The releases the annotation was reviewed for. Required for an annotation that sets a
    /// purpose, a unit or labels on a server metric, whose claim is then verified only for
    /// these releases.
    #[serde(default)]
    pub scope: Option<ScopeDef>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub provenance: Vec<ProvenanceDef>,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub unit: Option<Unit>,
    #[serde(default)]
    pub labels: Vec<LabelDef>,
}

impl AnnotationDef {
    pub(crate) fn is_empty(&self) -> bool {
        self.notes.is_empty()
            && self.provenance.is_empty()
            && self.purpose.is_none()
            && self.unit.is_none()
            && self.labels.is_empty()
    }

    /// Whether the annotation changes what a claim states, rather than only adding notes.
    pub(crate) fn changes_statement(&self) -> bool {
        self.purpose.is_some() || self.unit.is_some() || !self.labels.is_empty()
    }
}

/// What a metric claim shows when neither the source nor an annotation describes it.
pub const NOT_DESCRIBED: &str = "Not described in the source.";

impl Claim {
    /// Merge an annotation into this claim. Heuristics are Kyumei's own judgements, written
    /// by hand and never generated, so they take no annotations.
    pub(crate) fn annotate(&mut self, a: &AnnotationDef) -> Result<(), &'static str> {
        if a.scope.is_some() && !a.changes_statement() {
            return Err(
                "only an annotation that sets a purpose, a unit or labels takes a scope; notes cite their own sources",
            );
        }
        match self {
            Self::Metric(m) => {
                if a.changes_statement() && m.category.is_server() && a.scope.is_none() {
                    return Err(
                        "an annotation that sets a purpose, a unit or labels on a server metric must list the releases it was reviewed for (`scope`)",
                    );
                }
                if let Some(scope) = &a.scope
                    && !m.scope.server.iter().any(|v| scope.server.contains(v))
                {
                    return Err("none of the annotation's releases is in the claim's scope");
                }
                if a.purpose.is_some() && m.purpose.is_some() {
                    return Err("the claim already states a purpose; add a note instead");
                }
                if a.unit.is_some() && m.unit.is_some() {
                    return Err("the claim already states a unit");
                }
                if !a.labels.is_empty() && !m.labels.is_empty() {
                    return Err("the claim already declares labels");
                }
                if m.purpose.is_none() {
                    m.purpose.clone_from(&a.purpose);
                }
                m.unit = m.unit.or(a.unit);
                if m.labels.is_empty() {
                    m.labels.clone_from(&a.labels);
                }
                // What the annotation adds was reviewed only for its releases.
                if let Some(scope) = &a.scope {
                    let (kept, dropped): (Vec<String>, Vec<String>) = m
                        .scope
                        .server
                        .iter()
                        .cloned()
                        .partition(|v| scope.server.contains(v));
                    if !dropped.is_empty() {
                        let releases = |vs: &[String]| {
                            let parsed: Vec<ServerVersion> =
                                vs.iter().filter_map(|v| v.parse().ok()).collect();
                            kyumei_domain::describe_releases(&parsed)
                        };
                        m.notes.push(format!(
                            "The definition is the same in {}, but the labels, purpose or unit reviewed for it hold only for {}.",
                            releases(&dropped),
                            releases(&kept)
                        ));
                    }
                    m.scope.server = kept;
                }
            }
            _ if a.purpose.is_some() || a.unit.is_some() || !a.labels.is_empty() => {
                return Err("only metric annotations can set a purpose, a unit or labels");
            }
            _ => {}
        }
        let (notes, provenance) = match self {
            Self::Metric(m) => (&mut m.notes, &mut m.provenance),
            Self::DynamicConfig(d) => (&mut d.notes, &mut d.provenance),
            Self::ApiField(f) => (&mut f.notes, &mut f.provenance),
            Self::Statement(s) => (&mut s.notes, &mut s.provenance),
            Self::Heuristic(_) => {
                return Err("heuristics are written by hand; edit the heuristic instead");
            }
        };
        notes.extend(a.notes.iter().cloned());
        provenance.extend(a.provenance.iter().cloned());
        Ok(())
    }

    pub fn id(&self) -> &ClaimId {
        match self {
            Self::Metric(m) => &m.id,
            Self::DynamicConfig(d) => &d.id,
            Self::ApiField(a) => &a.id,
            Self::Statement(s) => &s.id,
            Self::Heuristic(h) => &h.id,
        }
    }

    pub fn category(&self) -> ClaimCategory {
        match self {
            Self::Metric(m) => m.category,
            Self::DynamicConfig(_) => ClaimCategory::DynamicConfig,
            Self::ApiField(_) => ClaimCategory::ApiField,
            Self::Statement(s) => s.category,
            Self::Heuristic(_) => ClaimCategory::DiagnosticHeuristic,
        }
    }

    /// The name an operator would recognise.
    pub fn name(&self) -> String {
        match self {
            Self::Metric(m) => m.name.clone(),
            Self::DynamicConfig(d) => d.key.clone(),
            Self::ApiField(a) => format!("{}.{}", short_message(&a.message), a.field),
            Self::Statement(s) => s.title.clone(),
            Self::Heuristic(h) => h.title.clone(),
        }
    }

    pub fn purpose(&self) -> String {
        match self {
            Self::Metric(m) => m
                .purpose
                .clone()
                .unwrap_or_else(|| NOT_DESCRIBED.to_owned()),
            Self::DynamicConfig(d) => d.description.clone(),
            Self::ApiField(a) => a.purpose.clone(),
            Self::Statement(s) => s.statement.clone(),
            Self::Heuristic(h) => h.statement.clone(),
        }
    }

    /// Every provenance entry, a setting's variants' included.
    pub fn all_provenance(&self) -> Vec<&ProvenanceDef> {
        let mut out: Vec<&ProvenanceDef> = self.provenance().iter().collect();
        if let Self::DynamicConfig(d) = self {
            out.extend(d.variants.iter().flat_map(|v| v.provenance.iter()));
        }
        out
    }

    pub fn provenance(&self) -> &[ProvenanceDef] {
        match self {
            Self::Metric(m) => &m.provenance,
            Self::DynamicConfig(d) => &d.provenance,
            Self::ApiField(a) => &a.provenance,
            Self::Statement(s) => &s.provenance,
            Self::Heuristic(h) => &h.provenance,
        }
    }

    pub fn notes(&self) -> &[String] {
        match self {
            Self::Metric(m) => &m.notes,
            Self::DynamicConfig(d) => &d.notes,
            Self::ApiField(a) => &a.notes,
            Self::Statement(s) => &s.notes,
            Self::Heuristic(_) => &[],
        }
    }

    pub fn scope_def(&self) -> Option<&ScopeDef> {
        match self {
            Self::Metric(m) => Some(&m.scope),
            Self::DynamicConfig(d) => Some(&d.scope),
            Self::ApiField(a) => Some(&a.scope),
            Self::Statement(s) => Some(&s.scope),
            Self::Heuristic(_) => None,
        }
    }

    pub fn verification_def(&self) -> Option<&VerificationDef> {
        match self {
            Self::Metric(m) => Some(&m.verification),
            Self::DynamicConfig(d) => Some(&d.verification),
            Self::ApiField(a) => Some(&a.verification),
            Self::Statement(s) => Some(&s.verification),
            Self::Heuristic(_) => None,
        }
    }

    /// The version scope in domain terms. Parsing was validated at load.
    pub fn scope(&self) -> VersionScope {
        let Some(def) = self.scope_def() else {
            return VersionScope::default();
        };
        VersionScope {
            server: def
                .server
                .iter()
                .filter_map(|v| v.parse::<ServerVersion>().ok())
                .collect(),
            sdk: def
                .sdk
                .iter()
                .map(|s| SdkScope {
                    family: s.family,
                    revision: s.revision.clone(),
                })
                .collect(),
        }
    }

    /// Verification in domain terms.
    pub fn verification(&self) -> Verification {
        if let Self::Heuristic(h) = self {
            return Verification::Verified {
                method: VerificationMethod::Authored,
                on: h.authored,
            };
        }
        match self.verification_def() {
            None => Verification::Unverified {
                reason: "no verification recorded".to_owned(),
            },
            Some(v) => match (v.status, v.method, v.on) {
                (VerificationStatusDef::Verified, Some(method), Some(on)) => {
                    Verification::Verified { method, on }
                }
                _ => Verification::Unverified {
                    reason: v
                        .reason
                        .clone()
                        .unwrap_or_else(|| "not verified against a source".to_owned()),
                },
            },
        }
    }

    pub fn as_metric(&self) -> Option<&MetricDef> {
        match self {
            Self::Metric(m) => Some(m),
            _ => None,
        }
    }
}

fn short_message(message: &str) -> &str {
    message.rsplit('.').next().unwrap_or(message)
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// One diagnostic: a symptom family with its hypotheses, evidence requests,
/// rules and conclusions.
#[derive(Clone, Debug)]
pub struct DiagnosticDef {
    pub id: DiagnosticId,
    pub title: String,
    pub summary: String,
    /// When several diagnostics' symptoms match a description, the lowest precedence wins.
    pub precedence: u32,
    /// The request path the system diagram follows.
    pub path: PathDef,
    pub symptoms: Vec<SymptomDef>,
    pub stages: Vec<StageDef>,
    pub signals: Vec<SignalDef>,
    pub hypotheses: Vec<HypothesisDef>,
    pub requests: Vec<RequestDef>,
    pub rules: Vec<RuleDef>,
    pub conclusions: Vec<ConclusionDef>,
    pub simulations: Vec<SimulationDef>,
}

impl DiagnosticDef {
    pub fn stage(&self, id: &StageId) -> Option<&StageDef> {
        self.stages.iter().find(|s| &s.id == id)
    }

    pub fn signal(&self, id: &SignalId) -> Option<&SignalDef> {
        self.signals.iter().find(|s| &s.id == id)
    }

    pub fn hypothesis(&self, id: &HypothesisId) -> Option<&HypothesisDef> {
        self.hypotheses.iter().find(|h| &h.id == id)
    }

    pub fn request(&self, id: &RequestDefinitionId) -> Option<&RequestDef> {
        self.requests.iter().find(|r| &r.id == id)
    }

    pub fn rule(&self, id: &RuleId) -> Option<&RuleDef> {
        self.rules.iter().find(|r| &r.id == id)
    }

    pub fn simulation(&self, id: &SimulationId) -> Option<&SimulationDef> {
        self.simulations.iter().find(|s| &s.id == id)
    }

    /// The simulation whose result informs `signal`, when one does.
    pub fn simulation_producing(&self, signal: &SignalId) -> Option<&SimulationDef> {
        self.simulations.iter().find(|s| &s.produces == signal)
    }

    pub fn conclusion(&self, id: &ConclusionId) -> Option<&ConclusionDef> {
        self.conclusions.iter().find(|c| &c.id == id)
    }

    pub fn stage_order(&self, id: &StageId) -> u32 {
        self.stage(id).map_or(u32::MAX, |s| s.order)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymptomDef {
    pub id: SymptomSignatureId,
    pub title: String,
    /// Case-insensitive regular expressions matched against the description.
    pub patterns: Vec<String>,
    /// Quantities found in the description are asserted for this signal.
    pub signal: SignalId,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageDef {
    pub id: StageId,
    pub order: u32,
    pub title: String,
    pub question: String,
}

/// Where in the request path a hypothesis locates the problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SystemLayer {
    Clients,
    Frontend,
    History,
    Matching,
    Persistence,
    Workers,
}

impl SystemLayer {
    pub fn token(self) -> &'static str {
        match self {
            Self::Clients => "clients",
            Self::Frontend => "frontend",
            Self::History => "history",
            Self::Matching => "matching",
            Self::Persistence => "persistence",
            Self::Workers => "workers",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Clients => "Clients",
            Self::Frontend => "Frontend",
            Self::History => "History",
            Self::Matching => "Matching",
            Self::Persistence => "Persistence",
            Self::Workers => "Workers",
        }
    }
}

/// The request path a diagnostic's system diagram follows.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathDef {
    /// What the path shows, for the diagram's description.
    pub description: String,
    /// The layers in order, from the callers to where the work completes.
    pub layers: Vec<SystemLayer>,
    /// Names that replace a layer's own on this path, e.g. "Signal senders".
    #[serde(default)]
    pub labels: BTreeMap<SystemLayer, String>,
}

impl PathDef {
    pub fn label(&self, layer: SystemLayer) -> &str {
        self.labels
            .get(&layer)
            .map_or(layer.label(), String::as_str)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalDef {
    pub id: SignalId,
    pub title: String,
    pub unit: Unit,
    pub description: String,
    /// How several observations of this signal combine.
    #[serde(default)]
    pub combine: CombinePolicy,
    #[serde(default)]
    pub source: Vec<SignalSourceDef>,
}

/// How facts for one signal combine when several series inform it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CombinePolicy {
    /// Keep the extremes: min of minimums, max of maximums (default).
    #[default]
    Extremes,
    /// Sum series point-wise by timestamp (e.g. per-partition rates).
    Sum,
}

/// A way a signal can be read from evidence.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalSourceDef {
    /// The metric, API field or dynamic config claim.
    pub claim: ClaimId,
    /// Required label values (metrics only).
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// For counters supplied raw: convert cumulative values to a per-second rate.
    #[serde(default)]
    pub transform: Option<Transform>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transform {
    Rate,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HypothesisDef {
    pub id: HypothesisId,
    pub stage: StageId,
    pub title: String,
    pub statement: String,
    /// The yes/no question this hypothesis answers on the diagnostic path.
    pub question: String,
    pub layer: SystemLayer,
    /// Shown on the diagnostic path as the stage's primary question.
    #[serde(default)]
    pub primary: bool,
    /// When present, the hypothesis is only considered once this holds.
    #[serde(default)]
    pub activation: Option<Condition>,
    /// What established means for the operator.
    #[serde(default)]
    pub implication: Option<String>,
}

/// A simulation that asks whether a hypothesis's cause, at the size the
/// evidence shows, is big enough to produce the symptom.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationDef {
    pub id: SimulationId,
    pub title: String,
    pub hypothesis: HypothesisId,
    /// What the scenario models, and so which inputs it takes.
    pub model: SimulationModel,
    pub why: String,
    /// Each input the model takes, read from a statistic of an observed
    /// fact, e.g. `add_rate = "tasks_add_rate.max"`.
    pub inputs: BTreeMap<String, SignalStat>,
    /// The signal the run's result informs, as a simulated fact.
    pub produces: SignalId,
    /// What the model and its use rest on.
    #[serde(default)]
    pub claims: Vec<ClaimId>,
}

/// The scenarios Kyumei can build for a simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SimulationModel {
    /// One Task Queue whose Workflow Tasks Matching dispatches at a rate
    /// limit, with Workers, pollers and slots to spare: the wait the limit
    /// alone causes. It reports the symptom's schedule-to-start.
    TaskQueueDispatch,
}

impl SimulationModel {
    pub fn token(self) -> &'static str {
        match self {
            Self::TaskQueueDispatch => "task-queue-dispatch",
        }
    }

    /// The Temporal release the simulator models. Investigations of other
    /// minor releases get no simulation.
    pub fn models(self) -> ServerVersion {
        ServerVersion::new(1, 31, 0)
    }

    /// The inputs the model needs.
    pub fn required(self) -> &'static [&'static str] {
        match self {
            Self::TaskQueueDispatch => {
                &["add_rate", "dispatch_limit", "read_partitions", "symptom"]
            }
        }
    }

    /// The inputs the model uses when they are known.
    pub fn optional(self) -> &'static [&'static str] {
        match self {
            Self::TaskQueueDispatch => &[],
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestDef {
    pub id: RequestDefinitionId,
    pub stage: StageId,
    pub title: String,
    pub why: String,
    pub signals: Vec<SignalId>,
    pub distinguishes: Vec<HypothesisId>,
    /// Relative collection effort, 1 (one query) to 5 (hard to obtain).
    pub cost: u8,
    #[serde(default)]
    pub window: WindowDef,
    #[serde(default)]
    pub group_by: Vec<String>,
    /// Access the operator needs, e.g. `sdk-metrics`, `server-metrics`, `cli`.
    #[serde(default)]
    pub requires: Vec<String>,
    pub collect: Vec<CollectDef>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowDef {
    /// Minutes of context before the incident window.
    #[serde(default = "default_pad_before")]
    pub pad_before_minutes: u32,
    #[serde(default = "default_pad_after")]
    pub pad_after_minutes: u32,
    /// Guidance when the incident window is unknown.
    #[serde(default)]
    pub guidance: Option<String>,
}

impl Default for WindowDef {
    fn default() -> Self {
        Self {
            pad_before_minutes: default_pad_before(),
            pad_after_minutes: default_pad_after(),
            guidance: None,
        }
    }
}

fn default_pad_before() -> u32 {
    10
}

fn default_pad_after() -> u32 {
    5
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectDef {
    pub method: kyumei_domain::CollectionMethod,
    pub label: String,
    /// The query or command, with `{namespace}`, `{task_queue}`, `{start}`,
    /// `{end}` and `{step}` placeholders.
    pub body: String,
    /// The SDK families the instruction is for: one (`"go"`) or several (`["core", "go"]`).
    /// Empty means every family.
    #[serde(default, deserialize_with = "one_or_many")]
    pub sdk_family: Vec<SdkFamily>,
    #[serde(default)]
    pub unit: Option<Unit>,
    #[serde(default)]
    pub claims: Vec<ClaimId>,
    #[serde(default)]
    pub note: Option<String>,
    /// A witness that lets this query's empty result read as zero.
    #[serde(default)]
    pub absent_reads_zero: Option<AbsentReadsZeroDef>,
}

/// An empty result reads as zero when the witness, a counter the same source keeps
/// whenever this query's counter could count, has series over the same window.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbsentReadsZeroDef {
    /// A PromQL query, with the same placeholders as the instruction's body.
    pub witness: String,
    /// The statement that the counter has no series until it first counts.
    pub because: ClaimId,
}

/// A value written either alone or as a list.
fn one_or_many<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        One(T),
        Many(Vec<T>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(one) => vec![one],
        OneOrMany::Many(many) => many,
    })
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    pub id: RuleId,
    pub hypothesis: HypothesisId,
    pub effect: kyumei_domain::Effect,
    pub when: Condition,
    /// Statement template; `{signal.stat}` placeholders render facts.
    pub statement: String,
    /// Knowledge the rule relies on (metric definitions, heuristics).
    #[serde(default)]
    pub claims: Vec<ClaimId>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConclusionDef {
    pub id: ConclusionId,
    pub title: String,
    /// Statement template; may use `{signal.stat}` placeholders.
    pub statement: String,
    pub when: Condition,
    /// Sentences appended when their own condition holds.
    #[serde(default, rename = "clause")]
    pub clauses: Vec<ClauseDef>,
    #[serde(default, rename = "recommendation")]
    pub recommendations: Vec<RecommendationDef>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClauseDef {
    pub when: Condition,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecommendationDef {
    pub text: String,
    #[serde(default)]
    pub caution: Option<String>,
    #[serde(default)]
    pub claims: Vec<ClaimId>,
}

// ---------------------------------------------------------------------------
// References to operations material
// ---------------------------------------------------------------------------

/// Operations material outside the pack (a runbook, a section of a playbook, a
/// dashboard panel) and what in the diagnostics it bears on. Kyumei cites it at
/// its source's pinned commit and links to it; it copies none of it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceDef {
    pub id: ReferenceId,
    pub kind: ReferenceKind,
    /// The material's own title: a document's first heading, or a panel's title.
    pub title: String,
    /// Why it bears on what it links, in Kyumei's words.
    pub why: String,
    pub source: SourceId,
    /// The file, relative to the source's root.
    pub path: String,
    /// A section of the document: its heading as written, which its anchor is made from.
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub panel: Option<PanelRefDef>,
    /// The version of what it cites, where the source gives one.
    #[serde(default)]
    pub version: Option<ReferenceVersion>,
    #[serde(rename = "link")]
    pub links: Vec<ReferenceLinkDef>,
}

/// A version the source declares for what a reference cites, and where it says so.
/// A repository without tags or releases versions its parts on its own: a
/// dashboard's README gives its current version, and a playbook names the version
/// of the dashboard it needs.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReferenceVersion {
    /// `v2.25.0`, or `v2.15.0+` for the least version needed.
    pub version: String,
    pub of: VersionOf,
    /// The dashboard a playbook needs, by name: `Temporal Server`.
    #[serde(default)]
    pub dashboard: Option<String>,
    /// Where the source declares the version, relative to its root.
    pub path: String,
    pub line: u32,
    /// The declaration, verbatim, on that line.
    pub excerpt: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VersionOf {
    /// The cited dashboard's own version.
    Dashboard,
    /// The version of a dashboard the cited document needs.
    NeedsDashboard,
}

impl ReferenceVersion {
    /// `dashboard v2.25.0`, or `needs the Temporal Server dashboard v2.15.0+`.
    pub fn describe(&self) -> String {
        match (self.of, &self.dashboard) {
            (VersionOf::Dashboard, _) => format!("dashboard {}", self.version),
            (VersionOf::NeedsDashboard, Some(d)) => {
                format!("needs the {d} dashboard {}", self.version)
            }
            (VersionOf::NeedsDashboard, None) => format!("needs dashboard {}", self.version),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReferenceKind {
    /// An alert's runbook.
    Runbook,
    /// A playbook, or a section of one.
    Playbook,
    /// A panel of a Grafana dashboard.
    Panel,
}

impl ReferenceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Runbook => "Runbook",
            Self::Playbook => "Playbook",
            Self::Panel => "Dashboard panel",
        }
    }
}

/// A panel of a dashboard in the source.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PanelRefDef {
    /// The dashboard's `uid`.
    pub dashboard: String,
    /// The dashboard's title, as it names itself.
    pub dashboard_title: String,
    /// The panel's `id` in the dashboard.
    pub id: i64,
    /// The SDK whose metrics the dashboard shows, for an SDK dashboard.
    #[serde(default)]
    pub sdk_family: Option<SdkFamily>,
}

/// What a reference bears on in one diagnostic. A link that names nothing else
/// bears on the diagnostic as a whole: its symptom.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceLinkDef {
    pub diagnostic: DiagnosticId,
    #[serde(default)]
    pub conclusions: Vec<ConclusionId>,
    #[serde(default)]
    pub hypotheses: Vec<HypothesisId>,
    #[serde(default)]
    pub requests: Vec<RequestDefinitionId>,
}

impl ReferenceLinkDef {
    pub fn is_whole(&self) -> bool {
        self.conclusions.is_empty() && self.hypotheses.is_empty() && self.requests.is_empty()
    }
}

/// What in a diagnostic a reference can bear on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceTarget<'a> {
    /// The diagnostic as a whole, its symptom.
    Diagnostic,
    Conclusion(&'a ConclusionId),
    Hypothesis(&'a HypothesisId),
    Request(&'a RequestDefinitionId),
}

impl ReferenceDef {
    /// The anchor of the section, as GitHub makes it from the heading.
    pub fn anchor(&self) -> Option<String> {
        self.section.as_deref().map(crate::markdown::heading_anchor)
    }

    /// Whether the reference bears on `target` in `diagnostic`.
    pub fn bears_on(&self, diagnostic: &DiagnosticId, target: ReferenceTarget<'_>) -> bool {
        self.links
            .iter()
            .filter(|l| &l.diagnostic == diagnostic)
            .any(|l| match target {
                ReferenceTarget::Diagnostic => l.is_whole(),
                ReferenceTarget::Conclusion(c) => l.conclusions.contains(c),
                ReferenceTarget::Hypothesis(h) => l.hypotheses.contains(h),
                ReferenceTarget::Request(r) => l.requests.contains(r),
            })
    }

    /// The material's name with its section: "Playbook — Section".
    pub fn display_title(&self) -> String {
        match &self.section {
            Some(section) => format!("{} — {}", self.title, crate::markdown::plain(section)),
            None => self.title.clone(),
        }
    }
}

/// A loaded, validated pack.
#[derive(Clone, Debug)]
pub struct Pack {
    pub manifest: PackManifest,
    /// SHA-256 over the pack's files (path and contents), hex.
    pub digest: String,
    pub sources: BTreeMap<SourceId, SourceDef>,
    pub claims: BTreeMap<ClaimId, Claim>,
    pub diagnostics: Vec<DiagnosticDef>,
    /// Runbooks, playbook sections and dashboard panels the diagnostics link to.
    pub references: Vec<ReferenceDef>,
    /// The files the pack was loaded from, relative to the knowledge root.
    pub files: Vec<String>,
}
