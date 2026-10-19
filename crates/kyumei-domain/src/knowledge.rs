//! Knowledge claims and their provenance.
//!
//! A knowledge claim is anything Kyumei asserts about Temporal itself: that a
//! metric exists and what it means, a dynamic configuration key and its
//! default, an API field, a behaviour, or a diagnostic heuristic Kyumei's
//! authors chose. Every claim carries the versions it was verified against and
//! the sources that verify it.

use std::fmt;

use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::{
    environment::{SdkFamily, ServerVersion},
    ids::{ClaimId, SourceId},
};

/// The kind of thing a claim is about. Never confuse these: an API field is
/// not a Prometheus metric, and SDK metrics are not server metrics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaimCategory {
    SdkMetric,
    FrontendMetric,
    HistoryMetric,
    MatchingMetric,
    /// A metric only the Worker service records: the server's internal worker for system
    /// workflows (scanners, archival, batch operations), not an SDK Worker.
    WorkerServiceMetric,
    /// A server metric recorded by more than one service, by code they share, or by nothing
    /// the source shows.
    ServerMetric,
    ApiField,
    DynamicConfig,
    SdkConfiguration,
    CliCommand,
    Limit,
    Behaviour,
    /// A threshold or interpretation chosen by Kyumei's authors, not a
    /// statement about Temporal.
    DiagnosticHeuristic,
}

impl ClaimCategory {
    pub const ALL: [ClaimCategory; 13] = [
        Self::SdkMetric,
        Self::FrontendMetric,
        Self::HistoryMetric,
        Self::MatchingMetric,
        Self::WorkerServiceMetric,
        Self::ServerMetric,
        Self::ApiField,
        Self::DynamicConfig,
        Self::SdkConfiguration,
        Self::CliCommand,
        Self::Limit,
        Self::Behaviour,
        Self::DiagnosticHeuristic,
    ];

    /// The name pack files and URLs use, e.g. `dynamic-config`.
    pub fn token(self) -> &'static str {
        match self {
            Self::SdkMetric => "sdk-metric",
            Self::FrontendMetric => "frontend-metric",
            Self::HistoryMetric => "history-metric",
            Self::MatchingMetric => "matching-metric",
            Self::WorkerServiceMetric => "worker-service-metric",
            Self::ServerMetric => "server-metric",
            Self::ApiField => "api-field",
            Self::DynamicConfig => "dynamic-config",
            Self::SdkConfiguration => "sdk-configuration",
            Self::CliCommand => "cli-command",
            Self::Limit => "limit",
            Self::Behaviour => "behaviour",
            Self::DiagnosticHeuristic => "diagnostic-heuristic",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.token() == token)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SdkMetric => "SDK metric",
            Self::FrontendMetric => "Frontend metric",
            Self::HistoryMetric => "History metric",
            Self::MatchingMetric => "Matching metric",
            Self::WorkerServiceMetric => "Worker service metric",
            Self::ServerMetric => "Server metric",
            Self::ApiField => "API field",
            Self::DynamicConfig => "Dynamic configuration",
            Self::SdkConfiguration => "SDK configuration",
            Self::CliCommand => "CLI command",
            Self::Limit => "Limit",
            Self::Behaviour => "Behaviour",
            Self::DiagnosticHeuristic => "Kyumei diagnostic heuristic",
        }
    }

    pub fn is_metric(self) -> bool {
        matches!(
            self,
            Self::SdkMetric
                | Self::FrontendMetric
                | Self::HistoryMetric
                | Self::MatchingMetric
                | Self::WorkerServiceMetric
                | Self::ServerMetric
        )
    }

    pub fn is_server(self) -> bool {
        matches!(
            self,
            Self::FrontendMetric
                | Self::HistoryMetric
                | Self::MatchingMetric
                | Self::WorkerServiceMetric
                | Self::ServerMetric
                | Self::DynamicConfig
                | Self::Limit
        )
    }
}

impl fmt::Display for ClaimCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Where a claim can be checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The source, or every source (such as several releases) the citation holds in.
    pub sources: Vec<SourceId>,
    /// Path within the source repository or document.
    pub path: Option<String>,
    pub line: Option<u32>,
    pub url: Option<String>,
    /// The verbatim text that supports the claim.
    pub excerpt: Option<String>,
    pub note: Option<String>,
}

/// How a claim was verified, if at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Verification {
    Verified {
        method: VerificationMethod,
        on: Date,
    },
    Unverified {
        reason: String,
    },
}

impl Verification {
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerificationMethod {
    /// Read in the source at the recorded revision.
    SourceInspection,
    /// Read in versioned documentation.
    Documentation,
    /// Authored by Kyumei; the claim is a heuristic, not a Temporal fact.
    Authored,
}

impl VerificationMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::SourceInspection => "source inspection",
            Self::Documentation => "documentation",
            Self::Authored => "authored heuristic",
        }
    }
}

/// Which versions a claim was verified against.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionScope {
    /// Exact Temporal Server versions whose source was inspected.
    pub server: Vec<ServerVersion>,
    /// SDK implementations whose source was inspected, with revision.
    pub sdk: Vec<SdkScope>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkScope {
    pub family: SdkFamily,
    /// Human-readable revision, e.g. `sdk-go v1.36.0`.
    pub revision: String,
}

/// Whether a claim is known to apply to a particular deployment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "applicability", rename_all = "snake_case")]
pub enum Applicability {
    /// Verified against exactly this version.
    Verified,
    /// Verified against exactly this version, where it differed from what the claim states
    /// (a setting's other default, say).
    VerifiedDifferently { difference: String },
    /// Verified for another patch release of the same minor version.
    SameMinorUnverifiedPatch { verified: Vec<ServerVersion> },
    /// Not verified for this version.
    Unverified { verified: Vec<ServerVersion> },
    /// The target version is unknown, so applicability cannot be established.
    TargetVersionUnknown { verified: Vec<ServerVersion> },
    /// Not a server-versioned claim (SDK metric, heuristic).
    NotServerVersioned,
}

impl Applicability {
    pub fn describe(&self) -> String {
        let list = |v: &[ServerVersion]| {
            v.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        match self {
            Self::Verified => "verified for this server version".to_owned(),
            Self::VerifiedDifferently { difference } => {
                format!("verified for this server version, where it differs: {difference}")
            }
            Self::SameMinorUnverifiedPatch { verified } => {
                format!(
                    "verified for {}; this patch release is unverified",
                    list(verified)
                )
            }
            Self::Unverified { verified } => {
                format!(
                    "unverified for this server version (verified: {})",
                    list(verified)
                )
            }
            Self::TargetVersionUnknown { verified } => format!(
                "server version unknown; verified only for {}",
                list(verified)
            ),
            Self::NotServerVersioned => "not tied to a server version".to_owned(),
        }
    }

    pub fn is_verified(&self) -> bool {
        matches!(
            self,
            Self::Verified | Self::VerifiedDifferently { .. } | Self::NotServerVersioned
        )
    }
}

impl VersionScope {
    /// Whether a server-versioned claim applies to `target`.
    pub fn applicability(&self, target: Option<&ServerVersion>) -> Applicability {
        if self.server.is_empty() {
            return Applicability::NotServerVersioned;
        }
        let verified = self.server.clone();
        match target {
            None => Applicability::TargetVersionUnknown { verified },
            Some(target) if self.server.iter().any(|v| v.exactly(target)) => {
                Applicability::Verified
            }
            Some(target) if self.server.iter().any(|v| v.same_minor(target)) => {
                Applicability::SameMinorUnverifiedPatch { verified }
            }
            Some(_) => Applicability::Unverified { verified },
        }
    }
}

/// A short, displayable summary of a claim; the full definition lives in the
/// knowledge pack.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimSummary {
    pub id: ClaimId,
    pub category: ClaimCategory,
    pub name: String,
    pub purpose: String,
    pub scope: VersionScope,
    pub verification: Verification,
    pub provenance: Vec<Provenance>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applicability_is_exact() {
        let scope = VersionScope {
            server: vec![ServerVersion::new(1, 31, 0)],
            sdk: vec![],
        };
        assert_eq!(
            scope.applicability(Some(&ServerVersion::new(1, 31, 0))),
            Applicability::Verified
        );
        assert!(matches!(
            scope.applicability(Some(&ServerVersion::new(1, 31, 2))),
            Applicability::SameMinorUnverifiedPatch { .. }
        ));
        assert!(matches!(
            scope.applicability(Some(&ServerVersion::new(1, 29, 0))),
            Applicability::Unverified { .. }
        ));
        assert!(matches!(
            scope.applicability(None),
            Applicability::TargetVersionUnknown { .. }
        ));
    }
}
