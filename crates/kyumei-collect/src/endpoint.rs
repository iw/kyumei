//! Where samples are read from, and with which credentials.
//!
//! Credentials come from environment variables or files, never from command
//! line arguments, which other users of the machine can see.

use std::path::Path;

/// Environment variables the collector reads.
pub const ENV_TOKEN: &str = "KYUMEI_COLLECT_TOKEN";
pub const ENV_USER: &str = "KYUMEI_COLLECT_USER";
pub const ENV_PASSWORD: &str = "KYUMEI_COLLECT_PASSWORD";

/// The kind of API behind an endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The Prometheus HTTP API, as Prometheus, Thanos, Mimir and
    /// VictoriaMetrics serve it.
    Prometheus,
    /// Grafana's data source proxy in front of a Prometheus data source.
    Grafana,
}

impl Source {
    pub fn token(self) -> &'static str {
        match self {
            Self::Prometheus => "prometheus",
            Self::Grafana => "grafana",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    None,
    Bearer(String),
    Basic { user: String, password: String },
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Bearer(_) => f.write_str("Bearer(…)"),
            Self::Basic { user, .. } => write!(f, "Basic({user}, …)"),
        }
    }
}

/// A Prometheus-compatible query API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub source: Source,
    /// The URL `/api/v1/query_range` is appended to.
    pub api_base: String,
    /// What to call it in messages, without credentials.
    pub label: String,
    pub auth: Auth,
    /// `X-Scope-OrgID`, for Mimir, Cortex and Thanos tenants.
    pub tenant: Option<String>,
    /// Extra CA certificates (PEM) to trust.
    pub ca_pem: Option<Vec<u8>>,
}

#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    #[error("{0} is not an http or https URL")]
    Url(String),
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("{0} is empty")]
    Empty(String),
    #[error(
        "the URL carries credentials; give them in $KYUMEI_COLLECT_TOKEN, or $KYUMEI_COLLECT_USER and $KYUMEI_COLLECT_PASSWORD, or a file"
    )]
    CredentialsInUrl,
    #[error("{0} has a query or fragment; give the API's base URL")]
    NotABase(String),
}

impl Endpoint {
    /// A Prometheus API at `url`, such as `http://prometheus:9090`.
    pub fn prometheus(url: &str) -> Result<Self, EndpointError> {
        let base = base_url(url)?;
        Ok(Self {
            source: Source::Prometheus,
            label: format!("Prometheus at {base}"),
            api_base: base,
            auth: Auth::None,
            tenant: None,
            ca_pem: None,
        })
    }

    /// Grafana at `url`, reaching the Prometheus data source `datasource` (its
    /// UID) through Grafana's data source proxy.
    pub fn grafana(url: &str, datasource: &str) -> Result<Self, EndpointError> {
        let base = base_url(url)?;
        Ok(Self {
            source: Source::Grafana,
            label: format!("Grafana at {base}, data source {datasource}"),
            api_base: format!("{base}/api/datasources/proxy/uid/{datasource}"),
            auth: Auth::None,
            tenant: None,
            ca_pem: None,
        })
    }

    /// Take credentials from `token_file` or `$KYUMEI_COLLECT_TOKEN` (a bearer
    /// token, such as a Grafana service account token), else from `user` and
    /// `password_file` or `$KYUMEI_COLLECT_PASSWORD` (basic authentication).
    pub fn with_credentials(
        mut self,
        env: impl Fn(&str) -> Option<String>,
        token_file: Option<&Path>,
        user: Option<String>,
        password_file: Option<&Path>,
    ) -> Result<Self, EndpointError> {
        let token = match token_file {
            Some(p) => Some(read_secret(p)?),
            None => env(ENV_TOKEN).filter(|t| !t.trim().is_empty()),
        };
        let user = user.or_else(|| env(ENV_USER)).filter(|u| !u.is_empty());
        self.auth = match (token, user) {
            (Some(t), _) => Auth::Bearer(t.trim().to_owned()),
            (None, Some(user)) => {
                let password = match password_file {
                    Some(p) => read_secret(p)?,
                    None => env(ENV_PASSWORD).unwrap_or_default(),
                };
                Auth::Basic {
                    user,
                    password: password.trim_end_matches(['\r', '\n']).to_owned(),
                }
            }
            (None, None) => Auth::None,
        };
        Ok(self)
    }

    pub fn with_tenant(mut self, tenant: Option<String>) -> Self {
        self.tenant = tenant.filter(|t| !t.is_empty());
        self
    }

    pub fn with_ca_file(mut self, path: Option<&Path>) -> Result<Self, EndpointError> {
        if let Some(p) = path {
            self.ca_pem = Some(std::fs::read(p).map_err(|source| EndpointError::Read {
                path: p.display().to_string(),
                source,
            })?);
        }
        Ok(self)
    }
}

fn base_url(url: &str) -> Result<String, EndpointError> {
    let trimmed = url.trim().trim_end_matches('/');
    let Some(rest) = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
    else {
        return Err(EndpointError::Url(url.to_owned()));
    };
    let authority = rest.split('/').next().unwrap_or_default();
    // Command lines and messages show the URL, so credentials stay out of it.
    if authority.contains('@') {
        return Err(EndpointError::CredentialsInUrl);
    }
    if authority.is_empty() {
        return Err(EndpointError::Url(url.to_owned()));
    }
    if rest.contains(['?', '#']) {
        return Err(EndpointError::NotABase(url.to_owned()));
    }
    Ok(trimmed.to_owned())
}

fn read_secret(path: &Path) -> Result<String, EndpointError> {
    let text = std::fs::read_to_string(path).map_err(|source| EndpointError::Read {
        path: path.display().to_string(),
        source,
    })?;
    let secret = text.trim().to_owned();
    if secret.is_empty() {
        return Err(EndpointError::Empty(path.display().to_string()));
    }
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn grafana_endpoints_go_through_the_data_source_proxy() {
        let e = Endpoint::grafana("https://grafana.example.com/", "prom-uid").expect("endpoint");
        assert_eq!(
            e.api_base,
            "https://grafana.example.com/api/datasources/proxy/uid/prom-uid"
        );
        assert_eq!(e.source, Source::Grafana);
        assert!(Endpoint::prometheus("prometheus:9090").is_err());
        assert!(Endpoint::prometheus("http://").is_err());
        assert!(matches!(
            Endpoint::prometheus("http://p:9090/?x=1"),
            Err(EndpointError::NotABase(_))
        ));
        let path = Endpoint::prometheus("https://mimir.example.com/prometheus/").expect("path");
        assert_eq!(path.api_base, "https://mimir.example.com/prometheus");
    }

    #[test]
    fn urls_with_credentials_are_refused_without_repeating_them() {
        let e = Endpoint::prometheus("https://ops:s3cret@prometheus.example.com")
            .expect_err("credentials");
        assert!(matches!(e, EndpointError::CredentialsInUrl));
        assert!(!e.to_string().contains("s3cret"));
    }

    #[test]
    fn credentials_come_from_the_environment_and_never_print() {
        let e = Endpoint::prometheus("http://p:9090")
            .expect("endpoint")
            .with_credentials(env(&[(ENV_TOKEN, "s3cret\n")]), None, None, None)
            .expect("credentials");
        assert_eq!(e.auth, Auth::Bearer("s3cret".to_owned()));
        assert!(!format!("{e:?}").contains("s3cret"));

        let e = Endpoint::prometheus("http://p:9090")
            .expect("endpoint")
            .with_credentials(
                env(&[(ENV_USER, "ops"), (ENV_PASSWORD, "pw")]),
                None,
                None,
                None,
            )
            .expect("credentials");
        assert_eq!(
            e.auth,
            Auth::Basic {
                user: "ops".to_owned(),
                password: "pw".to_owned()
            }
        );
        assert!(!format!("{e:?}").contains("pw"));
    }
}
