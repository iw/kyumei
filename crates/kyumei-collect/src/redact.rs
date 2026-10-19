//! `--redact`: replacing label values with pseudonyms before a block leaves
//! the machine.
//!
//! A pseudonym is `r_` and ten hex digits of an HMAC-SHA256 of the label and
//! value, keyed with a secret created on first use and kept in the state
//! directory. The same value gets the same pseudonym every time on this
//! machine, so several blocks for one investigation agree, and nobody without
//! the key can recover it. Each pseudonym written is recorded beside the key,
//! so `kyumei-collect reveal` can look it up here.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::LazyLock,
};

use hmac::{Hmac, Mac};
use kyumei_samples::Samples;
use regex::Regex;
use sha2::Sha256;

const KEY_FILE: &str = "redaction.key";
const PSEUDONYMS_FILE: &str = "pseudonyms.tsv";

/// `label op "value"`, with PromQL string escapes in the value.
static MATCHER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([A-Za-z_][A-Za-z0-9_]*)(\s*)(=~|!~|!=|=)(\s*)"((?:[^"\\]|\\.)*)""#)
        .unwrap_or_else(|e| panic!("matcher pattern: {e}"))
});

/// The default state directory: `$XDG_CONFIG_HOME/kyumei-collect`, else
/// `~/.config/kyumei-collect`.
pub fn default_state_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("kyumei-collect"))
}

#[derive(Debug)]
pub struct Redactor {
    key: [u8; 32],
    labels: BTreeSet<String>,
    written: BTreeMap<String, (String, String)>,
}

impl Redactor {
    pub fn new(key: [u8; 32], labels: impl IntoIterator<Item = String>) -> Self {
        Self {
            key,
            labels: labels
                .into_iter()
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty())
                .collect(),
            written: BTreeMap::new(),
        }
    }

    /// The key in `dir`, created (readable by this user only) if absent.
    pub fn load_or_create(
        dir: &Path,
        labels: impl IntoIterator<Item = String>,
    ) -> io::Result<Self> {
        create_private_dir(dir)?;
        let path = dir.join(KEY_FILE);
        let key = match fs::read_to_string(&path) {
            Ok(text) => parse_key(text.trim()).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not a redaction key", path.display()),
                )
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let mut key = [0u8; 32];
                getrandom::getrandom(&mut key).map_err(|e| io::Error::other(e.to_string()))?;
                write_private(&path, &hex(&key))?;
                key
            }
            Err(e) => return Err(e),
        };
        Ok(Self::new(key, labels))
    }

    pub fn pseudonym(&mut self, label: &str, value: &str) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.key)
            .unwrap_or_else(|e| panic!("HMAC takes any key: {e}"));
        mac.update(label.as_bytes());
        mac.update(&[0]);
        mac.update(value.as_bytes());
        let tag = mac.finalize().into_bytes();
        let pseudonym = format!("r_{}", hex(&tag[..5]));
        self.written
            .insert(pseudonym.clone(), (label.to_owned(), value.to_owned()));
        pseudonym
    }

    /// Replace the redacted labels' values in every series and query.
    pub fn apply(&mut self, samples: &mut Samples) {
        if self.labels.is_empty() {
            return;
        }
        for q in &mut samples.queries {
            q.query = self.redact_query(&q.query);
            for series in &mut q.series {
                let labels = std::mem::take(&mut series.labels);
                series.labels = labels
                    .into_iter()
                    .map(|(k, v)| {
                        let v = if self.labels.contains(&k) {
                            self.pseudonym(&k, &v)
                        } else {
                            v
                        };
                        (k, v)
                    })
                    .collect();
            }
        }
        samples.redacted = self.labels.iter().cloned().collect();
    }

    /// Replace the values a query matches redacted labels against. A regex's
    /// alternatives, such as Kyumei's `a-b|a_b`, are replaced one by one.
    pub fn redact_query(&mut self, query: &str) -> String {
        let mut out = String::with_capacity(query.len());
        let mut last = 0;
        for m in MATCHER.captures_iter(query) {
            let (Some(all), Some(label)) = (m.get(0), m.get(1)) else {
                continue;
            };
            if !self.labels.contains(label.as_str()) {
                continue;
            }
            let op = &m[3];
            let raw = unescape_string(&m[5]);
            let value = if op == "=" || op == "!=" {
                self.pseudonym(label.as_str(), &raw)
            } else {
                let (open, inner, close) =
                    match raw.strip_prefix("(?:").and_then(|r| r.strip_suffix(')')) {
                        Some(inner) => ("(?:", inner.to_owned(), ")"),
                        None => ("", raw.clone(), ""),
                    };
                let alternatives: Vec<String> = split_alternatives(&inner)
                    .iter()
                    .map(|alt| self.pseudonym(label.as_str(), &unescape_regex(alt)))
                    .collect();
                format!("{open}{}{close}", alternatives.join("|"))
            };
            out.push_str(&query[last..all.start()]);
            out.push_str(&format!(
                "{}{}{op}{}\"{value}\"",
                label.as_str(),
                &m[2],
                &m[4]
            ));
            last = all.end();
        }
        out.push_str(&query[last..]);
        out
    }

    /// Append the pseudonyms written so far to the record in `dir`.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let path = dir.join(PSEUDONYMS_FILE);
        let known = read_pseudonyms(&path)?;
        // The table maps pseudonyms back to real values: this user's only.
        let mut options = fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            options.mode(0o600);
            if path.exists() {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
            }
        }
        let mut file = options.open(&path)?;
        for (pseudonym, (label, value)) in &self.written {
            if !known.contains_key(pseudonym) {
                writeln!(
                    file,
                    "{pseudonym}\t{label}\t{}",
                    value.replace(['\t', '\n'], " ")
                )?;
            }
        }
        Ok(())
    }
}

/// What [`reveal`] found for one pseudonym.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revealed {
    pub pseudonym: String,
    /// The label and value it stands for, when this machine wrote it.
    pub original: Option<(String, String)>,
}

/// Look up pseudonyms recorded in `dir`.
pub fn reveal(dir: &Path, pseudonyms: &[String]) -> io::Result<Vec<Revealed>> {
    let known = read_pseudonyms(&dir.join(PSEUDONYMS_FILE))?;
    Ok(pseudonyms
        .iter()
        .map(|p| Revealed {
            pseudonym: p.clone(),
            original: known.get(p.trim()).cloned(),
        })
        .collect())
}

fn read_pseudonyms(path: &Path) -> io::Result<BTreeMap<String, (String, String)>> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e),
    };
    Ok(text
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, '\t');
            Some((
                parts.next()?.to_owned(),
                (parts.next()?.to_owned(), parts.next()?.to_owned()),
            ))
        })
        .collect())
}

/// `dir` and any parents missing, the last readable by this user only.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents.as_bytes())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_key(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
        key[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(key)
}

/// A PromQL string literal's content, unescaped.
fn unescape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A regex alternative's literal text, with its escapes removed.
fn unescape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    out
}

/// Split a regex on its top-level `|`, leaving escaped ones alone.
fn split_alternatives(regex: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut depth = 0i32;
    let mut chars = regex.chars();
    while let Some(c) = chars.next() {
        if c == '|' && depth == 0 {
            parts.push(String::new());
            continue;
        }
        if let Some(current) = parts.last_mut() {
            current.push(c);
            match c {
                '\\' => current.extend(chars.next()),
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use kyumei_samples::{Point, QueryResult, Series, Window};

    use super::*;

    fn redactor() -> Redactor {
        Redactor::new([7u8; 32], ["namespace".to_owned(), "task_queue".to_owned()])
    }

    #[test]
    fn pseudonyms_are_stable_and_differ_by_label() {
        let mut r = redactor();
        let a = r.pseudonym("namespace", "orders");
        assert_eq!(a, r.pseudonym("namespace", "orders"));
        assert_ne!(a, r.pseudonym("task_queue", "orders"));
        assert!(a.starts_with("r_") && a.len() == 12, "{a}");
        assert_ne!(
            a,
            Redactor::new([8u8; 32], []).pseudonym("namespace", "orders")
        );
    }

    #[test]
    fn queries_lose_the_values_of_redacted_labels_only() {
        let mut r = redactor();
        let q = r#"sum by (task_queue) (x{namespace=~"commerce-prod|commerce_prod", task_queue="orders", worker_type="WorkflowWorker"})"#;
        let out = r.redact_query(q);
        assert!(!out.contains("commerce"), "{out}");
        assert!(!out.contains("orders"), "{out}");
        assert!(out.contains(r#"worker_type="WorkflowWorker""#), "{out}");
        assert!(out.contains("sum by (task_queue)"), "{out}");
        let ns = (
            r.pseudonym("namespace", "commerce-prod"),
            r.pseudonym("namespace", "commerce_prod"),
        );
        assert!(
            out.contains(&format!(r#"namespace=~"{}|{}""#, ns.0, ns.1)),
            "{out}"
        );
    }

    #[test]
    fn series_labels_are_replaced_and_the_block_says_which() {
        let mut r = redactor();
        let mut samples = Samples {
            collector: "kyumei-collect".to_owned(),
            source: "prometheus".to_owned(),
            collected_at_ms: 0,
            investigation: None,
            request: None,
            window: Window {
                start_ms: 0,
                end_ms: 1,
                step_ms: 1,
            },
            rounded_to: None,
            redacted: Vec::new(),
            queries: vec![QueryResult {
                query: r#"x{namespace="orders"}"#.to_owned(),
                series: vec![Series {
                    labels: BTreeMap::from([
                        ("namespace".to_owned(), "orders".to_owned()),
                        ("operation".to_owned(), "Poll".to_owned()),
                    ]),
                    points: vec![Point {
                        at_ms: 0,
                        value: 1.0,
                    }],
                }],
            }],
        };
        r.apply(&mut samples);
        let labels = &samples.queries[0].series[0].labels;
        assert_eq!(labels["namespace"], r.pseudonym("namespace", "orders"));
        assert_eq!(labels["operation"], "Poll");
        assert_eq!(samples.redacted, vec!["namespace", "task_queue"]);
    }

    #[test]
    fn the_key_and_record_stay_in_the_state_directory() {
        let dir = std::env::temp_dir().join(format!("kyumei-collect-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut first = Redactor::load_or_create(&dir, ["namespace".to_owned()]).expect("create");
        let p = first.pseudonym("namespace", "orders");
        first.save(&dir).expect("save");
        let mut second = Redactor::load_or_create(&dir, ["namespace".to_owned()]).expect("load");
        assert_eq!(second.pseudonym("namespace", "orders"), p);
        second.save(&dir).expect("save again");
        let found = reveal(&dir, &[p.clone(), "r_0000000000".to_owned()]).expect("reveal");
        assert_eq!(
            found[0].original,
            Some(("namespace".to_owned(), "orders".to_owned()))
        );
        assert_eq!(found[1].original, None);
        let record = fs::read_to_string(dir.join(PSEUDONYMS_FILE)).expect("record");
        assert_eq!(record.lines().count(), 1, "{record}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: PathBuf| fs::metadata(p).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode(dir.join(KEY_FILE)), 0o600);
            assert_eq!(
                mode(dir.join(PSEUDONYMS_FILE)),
                0o600,
                "the table reveals values"
            );
            assert_eq!(mode(dir.clone()), 0o700);
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
