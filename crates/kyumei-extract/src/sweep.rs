//! The version sweep: one claims file from the claims generated for several Temporal releases.
//!
//! Each release's generator output has one claim per setting or metric, citing that release.
//! The merge keeps one claim per ID, written as the baseline release renders it (or, for a
//! claim the baseline lacks, as the newest release that has it renders it), with:
//!
//! * `scope`: every release whose claim has the same identity: a setting's constructor,
//!   value type, default and constraint keys, or a metric's instrument and unit;
//! * `provenance`: one entry per distinct path, line and excerpt, naming every release where
//!   it holds;
//! * for a setting, a `[[dynamic_config.variant]]` per other identity, with its releases and
//!   provenance.
//!
//! A metric whose identity differs between releases is an error, so that such a change is
//! reviewed before it is modelled. Descriptions, categories and other informational fields
//! come from the release the claim is written as.

use std::{collections::BTreeMap, fmt::Write as _};

use anyhow::{Context as _, Result, bail};

use crate::claims::toml_str;

/// One release's generated claims.
#[derive(Clone, Debug)]
pub struct Release {
    /// The Temporal version, e.g. `1.29.7`.
    pub version: String,
    /// The Kyumei source ID its provenance cites, e.g. `temporal-server-v1.29.7`.
    pub source_id: String,
    /// The generator's output for the release.
    pub rendered: String,
}

/// What kind of claims are merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    DynamicConfig,
    Metric,
}

impl Kind {
    fn table(self) -> &'static str {
        match self {
            Self::DynamicConfig => "dynamic_config",
            Self::Metric => "metric",
        }
    }

    /// The fields that must match for two releases' claims to be the same claim.
    fn identity(self) -> &'static [&'static str] {
        match self {
            Self::DynamicConfig => &["setting", "value_type", "default", "constrained_by"],
            Self::Metric => &["instrument", "unit"],
        }
    }

    fn identity_text(self) -> &'static str {
        match self {
            Self::DynamicConfig => "constructor, value type, default and constraint keys",
            Self::Metric => "instrument and unit",
        }
    }
}

/// A release's claim, parsed.
struct Entry {
    body: String,
    identity: Vec<String>,
    path: String,
    line: i64,
    excerpt: String,
}

/// Merge the releases' claims into one file. `baseline` is the version the merged claims
/// are written as; it must be one of `releases`.
pub fn merge(kind: Kind, baseline: &str, releases: &[Release]) -> Result<String> {
    let mut ordered: Vec<&Release> = releases.iter().collect();
    ordered.sort_by_key(|r| version_key(&r.version));
    for pair in ordered.windows(2) {
        if pair[0].version == pair[1].version {
            bail!("release {} is given twice", pair[0].version);
        }
    }
    let base = ordered
        .iter()
        .find(|r| r.version == baseline)
        .with_context(|| format!("the baseline {baseline} is not among the releases"))?;

    // release index -> claim id -> entry
    let mut parsed: Vec<BTreeMap<String, Entry>> = Vec::new();
    for r in &ordered {
        parsed.push(
            parse(kind, &r.rendered).with_context(|| format!("reading release {}", r.version))?,
        );
    }
    let ids: std::collections::BTreeSet<&String> = parsed.iter().flat_map(|p| p.keys()).collect();

    let mut out = header(kind, base, &ordered);
    for id in ids {
        let present: Vec<usize> = (0..ordered.len())
            .filter(|&i| parsed[i].contains_key(id))
            .collect();
        let main = present
            .iter()
            .copied()
            .find(|&i| ordered[i].version == baseline)
            .or_else(|| present.last().copied())
            .expect("an id comes from at least one release");
        let main_entry = &parsed[main][id];

        // Group the releases by identity, the main identity first.
        let mut groups: Vec<(Vec<String>, Vec<usize>)> =
            vec![(main_entry.identity.clone(), vec![])];
        for &i in &present {
            let identity = &parsed[i][id].identity;
            match groups.iter_mut().find(|(g, _)| g == identity) {
                Some((_, members)) => members.push(i),
                None => groups.push((identity.clone(), vec![i])),
            }
        }
        if kind == Kind::Metric && groups.len() > 1 {
            let changed: Vec<String> = groups[1..]
                .iter()
                .flat_map(|(_, members)| members.iter().map(|&i| ordered[i].version.clone()))
                .collect();
            bail!(
                "{id}: its {} differ in {}; review the change before the sweep can model it",
                kind.identity_text(),
                changed.join(", ")
            );
        }

        let (_, in_scope) = &groups[0];
        let mut body = replace_scope(&main_entry.body, &versions(&ordered, in_scope))
            .with_context(|| format!("{id}: no scope line"))?;
        body = replace_provenance(&body, &provenance(&ordered, &parsed, id, in_scope))
            .with_context(|| format!("{id}: no provenance block"))?;
        for (identity, members) in &groups[1..] {
            let _ = write!(
                body,
                "\n[[dynamic_config.variant]]\nserver = [{servers}]\n",
                servers = versions(&ordered, members)
            );
            for (field, value) in kind.identity().iter().zip(identity) {
                let _ = writeln!(body, "{field} = {}", toml_str(value));
            }
            body.push_str(&provenance(&ordered, &parsed, id, members));
        }
        let _ = write!(out, "\n[[{}]]\n{}", kind.table(), body);
    }
    Ok(out)
}

/// The releases' versions as a TOML list's contents.
fn versions(ordered: &[&Release], members: &[usize]) -> String {
    members
        .iter()
        .map(|&i| toml_str(&ordered[i].version))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A provenance block naming, for each distinct path, line and excerpt, every release where
/// it holds, oldest first.
fn provenance(
    ordered: &[&Release],
    parsed: &[BTreeMap<String, Entry>],
    id: &str,
    members: &[usize],
) -> String {
    // (path, line, excerpt) and the sources it holds in.
    type Citation<'a> = (&'a str, i64, &'a str);
    let mut groups: Vec<(Citation<'_>, Vec<&str>)> = Vec::new();
    for &i in members {
        let e = &parsed[i][id];
        let key = (e.path.as_str(), e.line, e.excerpt.as_str());
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, sources)) => sources.push(&ordered[i].source_id),
            None => groups.push((key, vec![&ordered[i].source_id])),
        }
    }
    let mut out = String::from("provenance = [\n");
    for ((path, line, excerpt), sources) in groups {
        let source = match sources.as_slice() {
            [one] => toml_str(one),
            many => format!(
                "[{}]",
                many.iter()
                    .map(|s| toml_str(s))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        let _ = writeln!(
            out,
            "  {{ source = {source}, path = {path}, line = {line}, excerpt = {excerpt} }},",
            path = toml_str(path),
            excerpt = toml_str(excerpt),
        );
    }
    out.push_str("]\n");
    out
}

fn header(kind: Kind, base: &Release, ordered: &[&Release]) -> String {
    let first = ordered.first().map_or("", |r| r.version.as_str());
    let last = ordered.last().map_or("", |r| r.version.as_str());
    let (what, tool, sources) = match kind {
        Kind::DynamicConfig => (
            "Dynamic configuration",
            "gen-dc-claims",
            "every setting the sources register",
        ),
        Kind::Metric => (
            "Metrics",
            "gen-metric-claims",
            "every metric the sources define",
        ),
    };
    let mut out = format!(
        "# {what} of Temporal Server {first} to {last} ({n} releases): {sources}.\n\
         #\n\
         # Generated by {tool} from {base_source} and the {others} other releases of the version\n\
         # sweep. Do not edit: regenerate it, and put notes and extra provenance in an annotation\n\
         # file instead (docs/knowledge.md).\n\
         #\n\
         #   cargo run -p kyumei-extract --bin {tool} -- <{base_version} checkout> \\\n\
         #       --sweep <checkout of each other release> -o <this file>\n\
         #\n\
         # Each claim is written as {base_version} renders it, or as the newest release that has\n\
         # it. `scope` lists every release where its {identity} are the same,\n\
         # and `provenance` cites the line in each of them.",
        n = ordered.len(),
        base_source = base.source_id,
        others = ordered.len() - 1,
        base_version = base.version,
        identity = kind.identity_text(),
    );
    out.push_str(match kind {
        Kind::DynamicConfig => {
            " A setting that differed in other releases has a\n\
             # `[[dynamic_config.variant]]` for each other form.\n"
        }
        Kind::Metric => {
            "\n#\n\
             # `recorded_in` lists the packages whose code refers to the definition; the category\n\
             # follows from them. Expositions are the names and units the Prometheus endpoint can\n\
             # export, by metrics framework and options (claim `server.metrics-framework`).\n"
        }
    });
    out
}

/// Split a rendered file into its claims.
fn parse(kind: Kind, rendered: &str) -> Result<BTreeMap<String, Entry>> {
    let marker = format!("\n[[{}]]\n", kind.table());
    let mut out = BTreeMap::new();
    for chunk in rendered.split(&marker).skip(1) {
        let body = format!("{}\n", chunk.trim_end_matches('\n'));
        let table: toml::Table = toml::from_str(&body).context("a claim is not valid TOML")?;
        let id = table
            .get("id")
            .and_then(toml::Value::as_str)
            .context("a claim has no id")?
            .to_owned();
        let identity = kind
            .identity()
            .iter()
            .map(|f| match table.get(*f) {
                Some(toml::Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => String::new(),
            })
            .collect();
        let provenance = match table.get("provenance").and_then(toml::Value::as_array) {
            Some(p) if p.len() == 1 => &p[0],
            _ => bail!("{id}: a generated claim must have exactly one provenance entry"),
        };
        let field = |name: &str| provenance.get(name).cloned();
        let path = field("path")
            .and_then(|v| v.as_str().map(str::to_owned))
            .with_context(|| format!("{id}: provenance has no path"))?;
        let line = field("line")
            .and_then(|v| v.as_integer())
            .with_context(|| format!("{id}: provenance has no line"))?;
        let excerpt = field("excerpt")
            .and_then(|v| v.as_str().map(str::to_owned))
            .with_context(|| format!("{id}: provenance has no excerpt"))?;
        if out
            .insert(
                id.clone(),
                Entry {
                    body,
                    identity,
                    path,
                    line,
                    excerpt,
                },
            )
            .is_some()
        {
            bail!("{id} appears twice");
        }
    }
    Ok(out)
}

fn replace_scope(body: &str, versions: &str) -> Option<String> {
    let start = body.find("\nscope = { server = [")? + 1;
    let end = start + body[start..].find('\n')?;
    Some(format!(
        "{}scope = {{ server = [{versions}] }}{}",
        &body[..start],
        &body[end..]
    ))
}

fn replace_provenance(body: &str, block: &str) -> Option<String> {
    let start = body.find("\nprovenance = [\n")? + 1;
    let end = start + body[start..].find("\n]\n")? + 3;
    Some(format!("{}{block}{}", &body[..start], &body[end..]))
}

/// A sortable form of a `major.minor.patch` version.
pub fn version_key(version: &str) -> (u32, u32, u32) {
    let mut parts = version
        .split('.')
        .map(|p| p.parse::<u32>().unwrap_or(u32::MAX));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dc(version: &str, default: &str, line: u32) -> Release {
        let source = format!("temporal-server-v{version}");
        Release {
            version: version.to_owned(),
            source_id: source.clone(),
            rendered: format!(
                "# header\n\n[[dynamic_config]]\nid = \"dc.a.b\"\nkey = \"a.b\"\nsetting = \"NewGlobalIntSetting\"\nvalue_type = \"int\"\ndefault = \"{default}\"\nconstrained_by = \"global\"\ndescription = \"d {version}\"\nscope = {{ server = [\"{version}\"] }}\nverification = {{ status = \"verified\", method = \"source-inspection\", on = 2026-10-13 }}\nprovenance = [\n  {{ source = \"{source}\", path = \"c.go\", line = {line}, excerpt = '\"a.b\",' }},\n]\n"
            ),
        }
    }

    #[test]
    fn same_claims_share_one_scope_and_group_their_provenance() {
        let releases = [
            dc("1.30.0", "4", 10),
            dc("1.31.0", "4", 12),
            dc("1.29.0", "4", 10),
        ];
        let merged = merge(Kind::DynamicConfig, "1.31.0", &releases).expect("merge");
        let table: toml::Table = toml::from_str(&merged).expect("valid TOML");
        let claim = &table["dynamic_config"].as_array().expect("claims")[0];
        assert_eq!(claim["description"].as_str(), Some("d 1.31.0"));
        assert_eq!(
            claim["scope"]["server"].as_array().map(Vec::len),
            Some(3),
            "{merged}"
        );
        let provenance = claim["provenance"].as_array().expect("provenance");
        assert_eq!(provenance.len(), 2, "{merged}");
        assert_eq!(
            provenance[0]["source"].as_array().map(Vec::len),
            Some(2),
            "1.29.0 and 1.30.0 share line 10"
        );
        assert_eq!(
            provenance[1]["source"].as_str(),
            Some("temporal-server-v1.31.0")
        );
        assert!(claim.get("variant").is_none());
    }

    #[test]
    fn a_changed_default_becomes_a_variant() {
        let releases = [
            dc("1.29.0", "2", 10),
            dc("1.30.0", "4", 11),
            dc("1.31.0", "4", 12),
        ];
        let merged = merge(Kind::DynamicConfig, "1.31.0", &releases).expect("merge");
        let table: toml::Table = toml::from_str(&merged).expect("valid TOML");
        let claim = &table["dynamic_config"].as_array().expect("claims")[0];
        let scope: Vec<&str> = claim["scope"]["server"]
            .as_array()
            .expect("scope")
            .iter()
            .filter_map(toml::Value::as_str)
            .collect();
        assert_eq!(scope, ["1.30.0", "1.31.0"]);
        let variants = claim["variant"].as_array().expect("variants");
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0]["default"].as_str(), Some("2"));
        assert_eq!(
            variants[0]["provenance"][0]["source"].as_str(),
            Some("temporal-server-v1.29.0")
        );
    }

    #[test]
    fn a_claim_missing_from_the_baseline_is_written_as_its_newest_release() {
        let mut old = dc("1.29.0", "2", 10);
        old.rendered = old.rendered.replace("a.b", "old.key");
        let releases = [old, dc("1.31.0", "4", 12)];
        let merged = merge(Kind::DynamicConfig, "1.31.0", &releases).expect("merge");
        let table: toml::Table = toml::from_str(&merged).expect("valid TOML");
        let claims = table["dynamic_config"].as_array().expect("claims");
        assert_eq!(claims.len(), 2);
        let old = claims
            .iter()
            .find(|c| c["key"].as_str() == Some("old.key"))
            .expect("the removed setting is kept");
        assert_eq!(old["scope"]["server"][0].as_str(), Some("1.29.0"));
    }

    #[test]
    fn versions_sort_numerically() {
        assert!(version_key("1.9.0") < version_key("1.27.0"));
        assert!(version_key("1.27.10") > version_key("1.27.9"));
    }
}
