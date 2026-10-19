//! Versioned Temporal diagnostic knowledge.
//!
//! Knowledge lives as data under `knowledge/` in the repository: metric
//! definitions, dynamic configuration keys, API fields, behaviours, Kyumei's
//! own heuristics, and the diagnostics (hypotheses, evidence requests, rules
//! and conclusions) that use them. Every externally verifiable claim records
//! the Temporal versions it was verified against and where.
//!
//! Loading is strict. Malformed files, dangling references and internally
//! contradictory definitions fail with every problem listed.

pub mod condition;
pub mod markdown;
pub mod model;
pub mod template;
pub mod updates;
pub mod verify;

mod dates;
mod load;
mod validate;

use std::{collections::BTreeMap, fs, path::Path};

use kyumei_domain::{
    Applicability, ClaimId, ClaimSummary, DiagnosticId, PackRef, SdkFamily, ServerVersion,
    SourceId, SymptomSignatureId,
};
use regex::RegexBuilder;

pub use condition::{CompareOp, Condition, FactLookup, SignalStat, Truth};
pub use model::*;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

/// Knowledge loading failures.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LoadError {
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
    #[error("{at}: {message}")]
    Invalid { at: String, message: String },
    #[error("knowledge pack `{pack}` is inconsistent:\n  - {}", problems.join("\n  - "))]
    Inconsistent { pack: String, problems: Vec<String> },
    #[error("{0}")]
    Io(String),
}

impl LoadError {
    pub(crate) fn invalid(at: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            at: at.to_owned(),
            message: message.into(),
        }
    }
}

/// All loaded packs.
#[derive(Clone, Debug)]
pub struct KnowledgeBase {
    packs: Vec<Pack>,
}

/// The pack chosen for an investigation and how well it applies.
#[derive(Clone, Copy, Debug)]
pub struct PackSelection<'a> {
    pub pack: &'a Pack,
    pub fit: PackFit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackFit {
    /// The target version is one the pack was verified against.
    Verified,
    /// Same minor version, different (unverified) patch release.
    SameMinor,
    /// The target version is unknown.
    VersionUnknown,
    /// The version sweep covered the target: its settings and metrics are checked, but not
    /// every behaviour was read for it.
    Swept,
    /// The target version is outside the pack's family.
    OutsideFamily,
}

impl KnowledgeBase {
    /// The knowledge embedded at build time.
    pub fn builtin() -> Result<Self, LoadError> {
        Self::from_files(embedded::FILES)
    }

    /// Load every pack under `root` (a directory shaped like `knowledge/`).
    pub fn load_dir(root: &Path) -> Result<Self, LoadError> {
        let mut owned = Vec::new();
        collect(root, root, &mut owned)?;
        owned.sort();
        let borrowed: Vec<(&str, &str)> = owned
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        Self::from_files(&borrowed)
    }

    /// Assemble packs from `(relative path, contents)` pairs. Each directory
    /// containing a `pack.toml` is a pack; files below it belong to it.
    pub fn from_files(files: &[(&str, &str)]) -> Result<Self, LoadError> {
        let roots: Vec<String> = files
            .iter()
            .filter_map(|(p, _)| p.strip_suffix("pack.toml"))
            .map(str::to_owned)
            .collect();
        if roots.is_empty() {
            return Err(LoadError::Io("no pack.toml found in knowledge".to_owned()));
        }
        let mut packs = Vec::new();
        for root in &roots {
            let mut members: Vec<(&str, &str)> = files
                .iter()
                .filter(|(p, _)| {
                    p.starts_with(root.as_str())
                        // Belongs to the deepest pack root that contains it.
                        && !roots
                            .iter()
                            .any(|other| other.len() > root.len() && p.starts_with(other.as_str()))
                })
                .copied()
                .collect();
            members.sort();
            packs.push(load::load_pack(root.trim_end_matches('/'), &members)?);
        }
        let mut ids = std::collections::BTreeSet::new();
        for pack in &packs {
            if !ids.insert(pack.manifest.id.clone()) {
                return Err(LoadError::Invalid {
                    at: pack.manifest.id.to_string(),
                    message: "two packs share an id".to_owned(),
                });
            }
        }
        Ok(Self { packs })
    }

    pub fn packs(&self) -> &[Pack] {
        &self.packs
    }

    /// Choose the pack for a target server version.
    pub fn select(&self, version: Option<&ServerVersion>) -> Option<PackSelection<'_>> {
        let latest = self.packs.iter().max_by_key(|p| p.family())?;
        let Some(version) = version else {
            return Some(PackSelection {
                pack: latest,
                fit: PackFit::VersionUnknown,
            });
        };
        let family = format!("{}.{}", version.major, version.minor);
        let pack = self
            .packs
            .iter()
            .find(|p| p.manifest.server_family == family);
        // A pack verified for the version comes first, then one whose sweep covered it.
        let fit = |p: &Pack| {
            if p.verified_server_versions()
                .iter()
                .any(|v| v.exactly(version))
            {
                Some(PackFit::Verified)
            } else if p.swept_server_versions().iter().any(|v| v.exactly(version)) {
                Some(PackFit::Swept)
            } else {
                None
            }
        };
        if let Some(pack) = pack.or_else(|| {
            self.packs
                .iter()
                .filter(|p| fit(p).is_some())
                .max_by_key(|p| p.family())
        }) {
            return Some(PackSelection {
                pack,
                fit: fit(pack).unwrap_or(PackFit::SameMinor),
            });
        }
        // Otherwise the nearest: the oldest pack for a release older than every pack's.
        let release = Some((version.major, version.minor));
        let nearest = if self.packs.iter().all(|p| release < p.family()) {
            self.packs
                .iter()
                .min_by_key(|p| p.family())
                .unwrap_or(latest)
        } else {
            latest
        };
        Some(PackSelection {
            pack: nearest,
            fit: PackFit::OutsideFamily,
        })
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> Result<(), LoadError> {
    let entries =
        fs::read_dir(dir).map_err(|e| LoadError::Io(format!("{}: {e}", dir.display())))?;
    for entry in entries {
        let path = entry.map_err(|e| LoadError::Io(e.to_string()))?.path();
        if path.is_dir() {
            collect(root, &path, out)?;
        } else if path.extension().is_some_and(|e| e == "toml") {
            let contents = fs::read_to_string(&path)
                .map_err(|e| LoadError::Io(format!("{}: {e}", path.display())))?;
            let relative = path
                .strip_prefix(root)
                .map_err(|e| LoadError::Io(e.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            out.push((relative, contents));
        }
    }
    Ok(())
}

/// A metric claim matched from an exposed series name.
#[derive(Clone, Copy, Debug)]
pub struct MetricMatch<'a> {
    pub metric: &'a MetricDef,
    pub exposition: &'a ExpositionDef,
    /// `_bucket`, `_sum` or `_count` when the series was a histogram part.
    pub suffix: Option<&'static str>,
}

impl Pack {
    pub fn pack_ref(&self) -> PackRef {
        PackRef {
            id: self.manifest.id.clone(),
            version: self.manifest.version.clone(),
            digest: self.digest.clone(),
        }
    }

    pub fn verified_server_versions(&self) -> Vec<ServerVersion> {
        self.manifest
            .verified_server_versions
            .iter()
            .filter_map(|v| v.parse().ok())
            .collect()
    }

    pub fn swept_server_versions(&self) -> Vec<ServerVersion> {
        self.manifest
            .swept_server_versions
            .iter()
            .filter_map(|v| v.parse().ok())
            .collect()
    }

    /// The minor release the pack targets, as `(major, minor)`, to order packs by.
    pub fn family(&self) -> Option<(u32, u32)> {
        let (major, minor) = self.manifest.server_family.split_once('.')?;
        Some((major.parse().ok()?, minor.parse().ok()?))
    }

    pub fn claim(&self, id: &ClaimId) -> Option<&Claim> {
        self.claims.get(id)
    }

    pub fn source(&self, id: &SourceId) -> Option<&SourceDef> {
        self.sources.get(id)
    }

    pub fn diagnostic(&self, id: &DiagnosticId) -> Option<&DiagnosticDef> {
        self.diagnostics.iter().find(|d| &d.id == id)
    }

    pub fn reference(&self, id: &kyumei_domain::ReferenceId) -> Option<&ReferenceDef> {
        self.references.iter().find(|r| &r.id == id)
    }

    /// The references that bear on `target` in `diagnostic`: runbooks first, then
    /// playbooks, then panels.
    pub fn references_for(
        &self,
        diagnostic: &DiagnosticId,
        target: ReferenceTarget<'_>,
    ) -> Vec<&ReferenceDef> {
        let mut found: Vec<&ReferenceDef> = self
            .references
            .iter()
            .filter(|r| r.bears_on(diagnostic, target))
            .collect();
        found.sort_by_key(|r| r.kind);
        found
    }

    /// The diagnostic that owns a symptom signature.
    pub fn diagnostic_for(
        &self,
        signature: &SymptomSignatureId,
    ) -> Option<(&DiagnosticDef, &SymptomDef)> {
        self.diagnostics.iter().find_map(|d| {
            d.symptoms
                .iter()
                .find(|s| &s.id == signature)
                .map(|s| (d, s))
        })
    }

    /// Every recognised symptom, for the landing page.
    pub fn symptoms(&self) -> Vec<(&DiagnosticDef, &SymptomDef)> {
        self.diagnostics
            .iter()
            .flat_map(|d| d.symptoms.iter().map(move |s| (d, s)))
            .collect()
    }

    /// Classify a free-text description by the pack's symptom patterns.
    /// Returns the matching symptom and the matched text.
    pub fn classify(&self, description: &str) -> Option<(&DiagnosticDef, &SymptomDef, String)> {
        for (diagnostic, symptom) in self.symptoms() {
            for pattern in &symptom.patterns {
                let Ok(re) = RegexBuilder::new(pattern).case_insensitive(true).build() else {
                    continue;
                };
                if let Some(m) = re.find(description) {
                    return Some((diagnostic, symptom, m.as_str().to_owned()));
                }
            }
        }
        None
    }

    /// Find metric claims whose exposition matches an exposed series name.
    pub fn match_metric(&self, exposed: &str) -> Vec<MetricMatch<'_>> {
        let candidates: Vec<(&str, Option<&'static str>)> = {
            let mut c = vec![(exposed, None)];
            for suffix in ["_bucket", "_sum", "_count"] {
                if let Some(base) = exposed.strip_suffix(suffix) {
                    c.push((base, Some(suffix)));
                }
            }
            c
        };
        let mut out = Vec::new();
        for claim in self.claims.values() {
            let Claim::Metric(metric) = claim else {
                continue;
            };
            for exposition in &metric.exposition {
                for (name, suffix) in &candidates {
                    let histogram =
                        matches!(metric.instrument, Instrument::Histogram | Instrument::Timer);
                    if exposition.name == *name && (suffix.is_none() || histogram) {
                        out.push(MetricMatch {
                            metric,
                            exposition,
                            suffix: *suffix,
                        });
                    }
                }
            }
        }
        out
    }

    /// The claim as a displayable summary.
    pub fn claim_summary(&self, id: &ClaimId) -> Option<ClaimSummary> {
        let claim = self.claim(id)?;
        Some(ClaimSummary {
            id: id.clone(),
            category: claim.category(),
            name: claim.name(),
            purpose: claim.purpose(),
            scope: claim.scope(),
            verification: claim.verification(),
            provenance: claim
                .provenance()
                .iter()
                .map(ProvenanceDef::to_domain)
                .collect(),
        })
    }

    /// Whether a claim applies to the target server version. A setting that differed in
    /// the target release is verified, with the difference.
    pub fn applicability(&self, id: &ClaimId, target: Option<&ServerVersion>) -> Applicability {
        let claim = self.claim(id);
        if let (Some(Claim::DynamicConfig(d)), Some(target)) = (claim, target)
            && let Some(variant) = d.variant_for(target)
        {
            return Applicability::VerifiedDifferently {
                difference: variant.difference(d),
            };
        }
        claim
            .map(Claim::scope)
            .unwrap_or_default()
            .applicability(target)
    }

    /// Exposition names of a metric for an SDK family (all, when `None`).
    pub fn exposed_names(&self, id: &ClaimId, family: Option<SdkFamily>) -> Vec<&ExpositionDef> {
        self.claim(id)
            .and_then(Claim::as_metric)
            .map(|m| {
                m.exposition
                    .iter()
                    .filter(|e| {
                        family.is_none() || e.sdk_family.is_none() || e.sdk_family == family
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Claims grouped by category, for the knowledge browser.
    pub fn claims_by_category(&self) -> BTreeMap<kyumei_domain::ClaimCategory, Vec<&Claim>> {
        let mut map: BTreeMap<_, Vec<&Claim>> = BTreeMap::new();
        for claim in self.claims.values() {
            map.entry(claim.category()).or_default().push(claim);
        }
        map
    }

    /// Where each claim is used: by a diagnostic's signals, requests, rules and conclusions,
    /// or by a heuristic that relies on it. Claims nothing uses are absent.
    pub fn claim_uses(&self) -> BTreeMap<ClaimId, Vec<ClaimUse>> {
        let mut uses: BTreeMap<ClaimId, Vec<ClaimUse>> = BTreeMap::new();
        let mut add = |claim: &ClaimId, diagnostic: Option<&DiagnosticId>, by: String| {
            let list = uses.entry(claim.clone()).or_default();
            let used = ClaimUse {
                diagnostic: diagnostic.cloned(),
                by,
            };
            if !list.contains(&used) {
                list.push(used);
            }
        };
        for d in &self.diagnostics {
            for signal in &d.signals {
                for source in &signal.source {
                    add(&source.claim, Some(&d.id), format!("signal {}", signal.id));
                }
            }
            for request in &d.requests {
                for claim in request.collect.iter().flat_map(|c| &c.claims) {
                    add(claim, Some(&d.id), format!("request {}", request.id));
                }
            }
            for rule in &d.rules {
                for claim in &rule.claims {
                    add(claim, Some(&d.id), format!("rule {}", rule.id));
                }
            }
            for conclusion in &d.conclusions {
                for claim in conclusion.recommendations.iter().flat_map(|r| &r.claims) {
                    add(claim, Some(&d.id), format!("conclusion {}", conclusion.id));
                }
            }
        }
        for claim in self.claims.values() {
            if let Claim::Heuristic(h) = claim {
                for relied in &h.relies_on {
                    add(relied, None, format!("heuristic {}", h.id));
                }
            }
        }
        uses
    }
}

/// One use of a claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimUse {
    /// The diagnostic that uses it, or `None` when a heuristic relies on it.
    pub diagnostic: Option<DiagnosticId>,
    /// What uses it, e.g. `signal workflow_slots_available` or `heuristic heuristic.poller-busy`.
    pub by: String,
}

#[cfg(test)]
mod tests;
