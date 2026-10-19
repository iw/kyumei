//! The Temporal deployment under investigation, as far as it is known.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::epistemic::Attested;

/// A Temporal Server version. The patch component may be unknown ("1.31").
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ServerVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: Option<u32>,
}

impl ServerVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch: Some(patch),
        }
    }

    pub fn same_minor(&self, other: &ServerVersion) -> bool {
        self.major == other.major && self.minor == other.minor
    }

    /// Exact equality requires both patch components to be known.
    pub fn exactly(&self, other: &ServerVersion) -> bool {
        self.same_minor(other) && self.patch.is_some() && self.patch == other.patch
    }
}

/// Releases written compactly, as runs of consecutive patch releases within each minor
/// version: `1.27.0–1.27.4, 1.29.2, 1.31.0–1.31.2`.
pub fn describe_releases(versions: &[ServerVersion]) -> String {
    let mut sorted: Vec<&ServerVersion> = versions.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut runs: Vec<(&ServerVersion, &ServerVersion)> = Vec::new();
    for v in sorted {
        match runs.last_mut() {
            Some((_, last))
                if last.same_minor(v)
                    && last.patch.zip(v.patch).is_some_and(|(a, b)| b == a + 1) =>
            {
                *last = v;
            }
            _ => runs.push((v, v)),
        }
    }
    runs.iter()
        .map(|(a, b)| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}–{b}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for ServerVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.patch {
            Some(patch) => write!(f, "{}.{}.{}", self.major, self.minor, patch),
            None => write!(f, "{}.{}", self.major, self.minor),
        }
    }
}

impl FromStr for ServerVersion {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim().trim_start_matches(['v', 'V']);
        // Tolerate build metadata and pre-release suffixes but do not keep them.
        let core = trimmed.split(['-', '+', ' ']).next().unwrap_or_default();
        let mut parts = core.split('.');
        let parse = |p: Option<&str>| -> Result<Option<u32>, String> {
            p.map(|v| {
                v.parse::<u32>()
                    .map_err(|_| format!("`{s}` is not a Temporal Server version"))
            })
            .transpose()
        };
        let major = parse(parts.next())?.ok_or_else(|| format!("`{s}` has no major version"))?;
        let minor = parse(parts.next())?.ok_or_else(|| format!("`{s}` has no minor version"))?;
        let patch = parse(parts.next())?;
        if parts.next().is_some() {
            return Err(format!("`{s}` has too many version components"));
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

/// The language of a Temporal SDK.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SdkLanguage {
    Go,
    Java,
    TypeScript,
    Python,
    DotNet,
    Ruby,
    Rust,
    Php,
    Other(String),
}

impl SdkLanguage {
    /// Which metric implementation this SDK uses. Metric names, prefixes and
    /// duration units differ by implementation, not by language.
    pub fn family(&self) -> SdkFamily {
        match self {
            Self::TypeScript | Self::Python | Self::DotNet | Self::Ruby | Self::Rust => {
                SdkFamily::Core
            }
            Self::Go => SdkFamily::Go,
            Self::Java => SdkFamily::Java,
            Self::Php | Self::Other(_) => SdkFamily::Unknown,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Go => "Go",
            Self::Java => "Java",
            Self::TypeScript => "TypeScript",
            Self::Python => "Python",
            Self::DotNet => ".NET",
            Self::Ruby => "Ruby",
            Self::Rust => "Rust",
            Self::Php => "PHP",
            Self::Other(name) => name,
        }
    }
}

impl FromStr for SdkLanguage {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "" => return Err("empty SDK language".to_owned()),
            "go" | "golang" => Self::Go,
            "java" => Self::Java,
            "typescript" | "ts" | "node" | "nodejs" | "javascript" => Self::TypeScript,
            "python" | "py" => Self::Python,
            ".net" | "dotnet" | "c#" | "csharp" => Self::DotNet,
            "ruby" | "rb" => Self::Ruby,
            "rust" | "rs" => Self::Rust,
            "php" => Self::Php,
            _ => Self::Other(s.trim().to_owned()),
        })
    }
}

/// The metric-emitting implementation behind an SDK.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SdkFamily {
    /// SDKs built on the shared Rust core (TypeScript, Python, .NET, Ruby, Rust).
    Core,
    Go,
    Java,
    Unknown,
}

impl SdkFamily {
    /// The name as knowledge files write it: `core`, `go`, `java`.
    pub fn token(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Go => "go",
            Self::Java => "java",
            Self::Unknown => "unknown",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Core => "Core-based SDKs (TypeScript, Python, .NET, Ruby, Rust)",
            Self::Go => "Go SDK",
            Self::Java => "Java SDK",
            Self::Unknown => "SDK (implementation unknown)",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Core => "Core SDKs",
            Self::Go => "Go SDK",
            Self::Java => "Java SDK",
            Self::Unknown => "Unknown SDK",
        }
    }
}

/// One SDK in use by the affected Workers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkDescriptor {
    pub language: SdkLanguage,
    pub version: Option<String>,
}

impl fmt::Display for SdkDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.version {
            Some(version) => write!(f, "{} SDK {version}", self.language.label()),
            None => write!(f, "{} SDK (version unknown)", self.language.label()),
        }
    }
}

/// How the cluster is deployed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentKind {
    SelfHostedKubernetes,
    SelfHostedOther,
    TemporalCloud,
    #[default]
    Unknown,
}

impl DeploymentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::SelfHostedKubernetes => "Self-hosted on Kubernetes",
            Self::SelfHostedOther => "Self-hosted",
            Self::TemporalCloud => "Temporal Cloud",
            Self::Unknown => "Unknown",
        }
    }

    /// Whether Temporal Server metrics are plausibly available to the operator.
    pub fn server_metrics_expected(self) -> Option<bool> {
        match self {
            Self::SelfHostedKubernetes | Self::SelfHostedOther => Some(true),
            Self::TemporalCloud => Some(false),
            Self::Unknown => None,
        }
    }
}

/// Deployment topology. Every field is optional; unknown stays unknown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    pub deployment: DeploymentKind,
    pub frontend_hosts: Option<u32>,
    pub history_hosts: Option<u32>,
    pub matching_hosts: Option<u32>,
    pub worker_processes: Option<u32>,
    pub persistence: Option<String>,
    pub notes: Option<String>,
}

impl Topology {
    pub fn is_empty(&self) -> bool {
        self == &Topology::default()
    }
}

/// What is known about the environment.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporalEnvironment {
    pub server_version: Option<Attested<ServerVersion>>,
    pub sdks: Vec<Attested<SdkDescriptor>>,
    pub topology: Option<Attested<Topology>>,
    /// The environment profile the operator started from, when one was used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

impl TemporalEnvironment {
    /// The SDK families in use, when any SDK is known.
    pub fn sdk_families(&self) -> Vec<SdkFamily> {
        let mut families: Vec<SdkFamily> = self
            .sdks
            .iter()
            .map(|sdk| sdk.value.language.family())
            .collect();
        families.sort();
        families.dedup();
        families
    }
}

/// The part of the system the symptom affects.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvestigationScope {
    pub namespaces: Vec<String>,
    pub task_queues: Vec<String>,
}

impl InvestigationScope {
    pub fn primary_namespace(&self) -> Option<&str> {
        self.namespaces.first().map(String::as_str)
    }

    pub fn primary_task_queue(&self) -> Option<&str> {
        self.task_queues.first().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_versions() {
        assert_eq!(
            "1.31.0".parse::<ServerVersion>(),
            Ok(ServerVersion::new(1, 31, 0))
        );
        assert_eq!(
            "v1.31.0".parse::<ServerVersion>(),
            Ok(ServerVersion::new(1, 31, 0))
        );
        let minor_only: ServerVersion = "1.31".parse().expect("minor");
        assert_eq!(minor_only.patch, None);
        assert!(!minor_only.exactly(&ServerVersion::new(1, 31, 0)));
        assert!(minor_only.same_minor(&ServerVersion::new(1, 31, 0)));
        assert!("banana".parse::<ServerVersion>().is_err());
    }

    #[test]
    fn sdk_family_follows_implementation() {
        assert_eq!(SdkLanguage::Python.family(), SdkFamily::Core);
        assert_eq!(SdkLanguage::Go.family(), SdkFamily::Go);
        assert_eq!(
            "ts".parse::<SdkLanguage>().map(|l| l.family()),
            Ok(SdkFamily::Core)
        );
    }
}
