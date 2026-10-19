//! Kyumei's configuration file: the time zones operators prefer, and environment
//! profiles that fill in a new investigation's context.
//!
//! ```toml
//! time_zones = ["Europe/London", "America/New_York"]
//!
//! [[profile]]
//! name = "local-dev"
//! server_version = "1.25.1"
//! namespace = "orders"
//! deployment = "self-hosted"
//! ```

use std::path::{Path, PathBuf};

use kyumei_domain::DisplayZone;
use serde::Deserialize;

use crate::zone::TimeZones;

/// The Worker SDK languages the new-investigation form offers, as `(value, label)`.
pub const SDK_LANGUAGES: [(&str, &str); 8] = [
    ("go", "Go"),
    ("java", "Java"),
    ("typescript", "TypeScript"),
    ("python", "Python"),
    ("dotnet", ".NET"),
    ("ruby", "Ruby"),
    ("rust", "Rust"),
    ("php", "PHP"),
];

/// The deployments the form offers, as `(form value, label, configuration name)`.
pub const DEPLOYMENTS: [(&str, &str, &str); 3] = [
    (
        "self_hosted_kubernetes",
        "Self-hosted on Kubernetes",
        "kubernetes",
    ),
    ("self_hosted_other", "Self-hosted", "self-hosted"),
    ("temporal_cloud", "Temporal Cloud", "temporal-cloud"),
];

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

/// What the configuration file holds, as written.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    time_zones: Vec<String>,
    #[serde(default, rename = "profile")]
    profiles: Vec<Profile>,
}

/// An environment: the context a new investigation in it starts with. Every
/// field but the name is optional, and the form leaves out what a profile
/// doesn't set.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    /// A line shown beside the profile's name.
    pub description: Option<String>,
    pub server_version: Option<String>,
    pub sdk_language: Option<String>,
    pub sdk_version: Option<String>,
    pub namespace: Option<String>,
    pub task_queue: Option<String>,
    /// `kubernetes`, `self-hosted` or `temporal-cloud`.
    pub deployment: Option<String>,
    pub frontend_hosts: Option<u32>,
    pub history_hosts: Option<u32>,
    pub matching_hosts: Option<u32>,
    pub worker_processes: Option<u32>,
    pub persistence: Option<String>,
}

impl Profile {
    /// The deployment as the form names it, such as `self_hosted_other`.
    pub fn deployment_value(&self) -> Option<&'static str> {
        let d = self.deployment.as_deref()?.trim();
        DEPLOYMENTS
            .iter()
            .find(|(value, _, name)| *name == d || *value == d)
            .map(|(value, _, _)| *value)
    }
}

/// Kyumei's configuration, checked.
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// Where it was read from; `None` when there is no file.
    pub path: Option<PathBuf>,
    pub time_zones: TimeZones,
    pub profiles: Vec<Profile>,
}

impl Config {
    /// Read and check the file at `path`.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text, path)
    }

    /// The file at `path` when it exists, or no configuration.
    pub fn load_if_present(path: &Path) -> Result<Self, ConfigError> {
        if path.exists() {
            Self::load(path)
        } else {
            Ok(Self::default())
        }
    }

    /// Check configuration text read from `path`.
    pub fn parse(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let invalid = |message: String| ConfigError::Invalid {
            path: path.to_owned(),
            message,
        };
        let file: File = toml::from_str(text).map_err(|e| invalid(e.message().to_owned()))?;
        let mut zones = Vec::new();
        for name in &file.time_zones {
            let zone = DisplayZone::named(name).map_err(|_| {
                invalid(format!(
                    "time_zones: {name:?} is not a time zone; use IANA names such as \"Europe/London\""
                ))
            })?;
            zones.push(zone);
        }
        let mut names: Vec<&str> = Vec::new();
        for p in &file.profiles {
            let name = p.name.trim();
            if name.is_empty() {
                return Err(invalid("a profile needs a name".to_owned()));
            }
            if names.contains(&name) {
                return Err(invalid(format!("two profiles are named {name:?}")));
            }
            names.push(name);
            if let Some(l) = &p.sdk_language
                && !SDK_LANGUAGES.iter().any(|(v, _)| v == l)
            {
                return Err(invalid(format!(
                    "profile {name:?}: sdk_language {l:?} is not one of {}",
                    SDK_LANGUAGES.map(|(v, _)| v).join(", ")
                )));
            }
            if p.deployment.is_some() && p.deployment_value().is_none() {
                return Err(invalid(format!(
                    "profile {name:?}: deployment {:?} is not one of {}",
                    p.deployment.as_deref().unwrap_or_default(),
                    DEPLOYMENTS.map(|(_, _, n)| n).join(", ")
                )));
            }
        }
        Ok(Self {
            path: Some(path.to_owned()),
            time_zones: TimeZones::new(zones),
            profiles: file.profiles,
        })
    }

    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// One line for the startup banner.
    pub fn describe(&self) -> Option<String> {
        let path = self.path.as_ref()?;
        let zones = self.time_zones.preferred().len();
        let profiles = self.profiles.len();
        Some(format!(
            "configuration {}: {}, {}",
            path.display(),
            match zones {
                0 => "every time zone".to_owned(),
                1 => "1 preferred time zone".to_owned(),
                n => format!("{n} preferred time zones"),
            },
            match profiles {
                1 => "1 profile".to_owned(),
                n => format!("{n} profiles"),
            }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
time_zones = ["Europe/London", "America/New_York"]

[[profile]]
name = "local-dev"
description = "Temporal dev server on this machine"
server_version = "1.25.1"
namespace = "orders"
task_queue = "fulfil-q"
deployment = "self-hosted"
frontend_hosts = 1
history_hosts = 1
matching_hosts = 1
persistence = "SQLite (dev server)"
"#;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::parse(text, Path::new("config.toml"))
    }

    #[test]
    fn reads_zones_and_profiles() {
        let c = parse(EXAMPLE).expect("config");
        let zones: Vec<&str> = c
            .time_zones
            .preferred()
            .iter()
            .map(DisplayZone::name)
            .collect();
        assert_eq!(zones, ["Europe/London", "America/New_York"]);
        let p = c.profile("local-dev").expect("profile");
        assert_eq!(p.namespace.as_deref(), Some("orders"));
        assert_eq!(p.deployment_value(), Some("self_hosted_other"));
        assert_eq!(p.history_hosts, Some(1));
        assert_eq!(
            c.describe().as_deref(),
            Some("configuration config.toml: 2 preferred time zones, 1 profile")
        );
        assert!(parse("").expect("empty").profiles.is_empty());
    }

    #[test]
    fn mistakes_are_named() {
        let err = |text: &str| parse(text).expect_err("invalid").to_string();
        assert!(
            err(r#"time_zones = ["Europe/Lundun"]"#)
                .contains("\"Europe/Lundun\" is not a time zone")
        );
        assert!(
            err("[[profile]]\nname = \"a\"\n[[profile]]\nname = \"a\"")
                .contains("two profiles are named \"a\"")
        );
        assert!(
            err("[[profile]]\nname = \"a\"\nsdk_language = \"cobol\"")
                .contains("sdk_language \"cobol\"")
        );
        assert!(
            err("[[profile]]\nname = \"a\"\ndeployment = \"mainframe\"")
                .contains("kubernetes, self-hosted, temporal-cloud")
        );
        assert!(err("[[profile]]\nname = \"a\"\nnamspace = \"x\"").contains("unknown field"));
        assert!(err("timezones = []").contains("unknown field"));
    }
}
