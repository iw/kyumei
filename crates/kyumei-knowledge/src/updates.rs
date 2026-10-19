//! What is newer upstream than the sources a pack pins.
//!
//! This module compares; it doesn't fetch. The caller lists each repository's
//! tags and head (with `git ls-remote`, which works on any Git host) and hands
//! them in. Moving a pin stays a reviewed change: nothing here edits a pack.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{Pack, SourceKind};

/// A release version, `1.31.2`; pre-releases are not releases.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Release(pub u64, pub u64, pub u64);

impl std::fmt::Display for Release {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// `prefix` and release of a tag such as `v1.31.2`, `go1.26.4`, `1.25.0` or
/// `contrib/tally/v0.2.0`. `None` for anything else, pre-releases included.
pub fn parse_tag(tag: &str) -> Option<(String, Release)> {
    let start = tag
        .char_indices()
        .find(|(i, c)| {
            c.is_ascii_digit() && (*i == 0 || !tag[..*i].ends_with(|p: char| p.is_ascii_digit()))
        })
        .map(|(i, _)| i)?;
    let (prefix, rest) = tag.split_at(start);
    let mut parts = rest.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((prefix.to_owned(), Release(major, minor, patch)))
}

/// What a pack pins in one repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RepositoryPins {
    pub repository: String,
    /// Pinned releases by tag prefix (`v`, `go`, `contrib/tally/v`), oldest first.
    pub releases: BTreeMap<String, Vec<Release>>,
    /// Commits pinned without a release, with the branch they were taken from
    /// when the source says (`main`).
    pub commits: Vec<(String, Option<String>)>,
}

/// Every repository the pack pins, in name order.
pub fn pins(pack: &Pack) -> Vec<RepositoryPins> {
    let mut by_repository: BTreeMap<String, RepositoryPins> = BTreeMap::new();
    for s in pack.sources.values() {
        if !matches!(s.kind, SourceKind::Repository | SourceKind::Documentation) {
            continue;
        }
        let Some(repository) = s.repository.clone() else {
            continue;
        };
        let pins = by_repository
            .entry(repository.clone())
            .or_insert_with(|| RepositoryPins {
                repository,
                releases: BTreeMap::new(),
                commits: Vec::new(),
            });
        match s.revision.as_deref().and_then(parse_tag) {
            Some((prefix, release)) => {
                let list = pins.releases.entry(prefix).or_default();
                if !list.contains(&release) {
                    list.push(release);
                    list.sort();
                }
            }
            None => {
                // A commit: pinned outright, or named by a revision that is no
                // release (`main`, a describe string, a pseudo-version's hash).
                let commit = s.commit.clone().or_else(|| {
                    s.revision
                        .clone()
                        .filter(|r| r.len() >= 7 && r.chars().all(|c| c.is_ascii_hexdigit()))
                });
                if let Some(commit) = commit {
                    let branch = s
                        .revision
                        .clone()
                        .filter(|r| !r.chars().all(|c| c.is_ascii_hexdigit()) && !r.contains("-g"));
                    if !pins.commits.iter().any(|(c, _)| *c == commit) {
                        pins.commits.push((commit, branch));
                    }
                }
            }
        }
    }
    by_repository.into_values().collect()
}

/// Every repository any of `packs` pins, in name order, with all their pins together: a
/// release one pack pins is pinned, whichever pack it is.
pub fn pins_across<'a>(packs: impl IntoIterator<Item = &'a Pack>) -> Vec<RepositoryPins> {
    let mut by_repository: BTreeMap<String, RepositoryPins> = BTreeMap::new();
    for pack in packs {
        for p in pins(pack) {
            merge(&mut by_repository, p);
        }
    }
    by_repository.into_values().collect()
}

fn merge(by_repository: &mut BTreeMap<String, RepositoryPins>, p: RepositoryPins) {
    let merged = by_repository
        .entry(p.repository.clone())
        .or_insert_with(|| RepositoryPins {
            repository: p.repository.clone(),
            releases: BTreeMap::new(),
            commits: Vec::new(),
        });
    for (prefix, releases) in p.releases {
        let list = merged.releases.entry(prefix).or_default();
        for r in releases {
            if !list.contains(&r) {
                list.push(r);
            }
        }
        list.sort();
    }
    for c in p.commits {
        if !merged.commits.contains(&c) {
            merged.commits.push(c);
        }
    }
}

/// What a repository has upstream: its tags and the commit its default branch is at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Upstream {
    pub tags: Vec<String>,
    pub head: Option<String>,
}

impl Upstream {
    /// Read `git ls-remote --tags --refs` and `git ls-remote <repo> HEAD` output.
    pub fn from_ls_remote(tags: &str, head: &str) -> Self {
        Self {
            tags: tags
                .lines()
                .filter_map(|l| l.split_whitespace().nth(1))
                .filter_map(|r| r.strip_prefix("refs/tags/"))
                .map(str::to_owned)
                .collect(),
            head: head
                .lines()
                .find(|l| l.ends_with("HEAD"))
                .and_then(|l| l.split_whitespace().next())
                .map(str::to_owned),
        }
    }
}

/// What is newer than one pinned line of releases.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReleaseUpdate {
    pub prefix: String,
    /// The newest release pinned with this prefix.
    pub pinned: Release,
    /// Patch releases after a pinned release, in that release's minor line.
    pub patches: Vec<Release>,
    /// The newest release with this prefix, when it is past every pinned line.
    pub newest: Option<Release>,
}

/// A pinned commit and where the branch is now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommitUpdate {
    pub pinned: String,
    pub branch: Option<String>,
    /// The default branch's commit, when it is another one.
    pub head: Option<String>,
}

/// One repository's updates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RepositoryUpdate {
    pub repository: String,
    pub releases: Vec<ReleaseUpdate>,
    pub commits: Vec<CommitUpdate>,
}

impl RepositoryUpdate {
    pub fn is_current(&self) -> bool {
        self.releases
            .iter()
            .all(|r| r.patches.is_empty() && r.newest.is_none())
            && self.commits.iter().all(|c| c.head.is_none())
    }
}

/// Compare what is pinned with what is upstream.
pub fn compare(pins: &RepositoryPins, upstream: &Upstream) -> RepositoryUpdate {
    let mut available: BTreeMap<&str, Vec<Release>> = BTreeMap::new();
    let parsed: Vec<(String, Release)> =
        upstream.tags.iter().filter_map(|t| parse_tag(t)).collect();
    for (prefix, release) in &parsed {
        available.entry(prefix.as_str()).or_default().push(*release);
    }
    let releases = pins
        .releases
        .iter()
        .map(|(prefix, pinned)| {
            let mut upstream = available.get(prefix.as_str()).cloned().unwrap_or_default();
            upstream.sort();
            upstream.dedup();
            let newest_pinned = *pinned.last().unwrap_or(&Release(0, 0, 0));
            // Later patches of each pinned minor line, unless a later pin covers them.
            let mut patches: Vec<Release> = Vec::new();
            for p in pinned {
                let latest_pinned_in_line = pinned
                    .iter()
                    .filter(|x| x.0 == p.0 && x.1 == p.1)
                    .max()
                    .copied()
                    .unwrap_or(*p);
                for r in upstream
                    .iter()
                    .filter(|r| r.0 == p.0 && r.1 == p.1 && *r > &latest_pinned_in_line)
                {
                    if !patches.contains(r) {
                        patches.push(*r);
                    }
                }
            }
            patches.sort();
            let newest = upstream
                .last()
                .copied()
                .filter(|n| (n.0, n.1) > (newest_pinned.0, newest_pinned.1));
            ReleaseUpdate {
                prefix: prefix.clone(),
                pinned: newest_pinned,
                patches,
                newest,
            }
        })
        .collect();
    let commits = pins
        .commits
        .iter()
        .map(|(commit, branch)| CommitUpdate {
            pinned: commit.clone(),
            branch: branch.clone(),
            head: upstream
                .head
                .clone()
                .filter(|h| !h.starts_with(commit.as_str()) && !commit.starts_with(h.as_str())),
        })
        .collect();
    RepositoryUpdate {
        repository: pins.repository.clone(),
        releases,
        commits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_read_with_their_prefixes() {
        assert_eq!(
            parse_tag("v1.31.2"),
            Some(("v".to_owned(), Release(1, 31, 2)))
        );
        assert_eq!(
            parse_tag("1.25.0"),
            Some((String::new(), Release(1, 25, 0)))
        );
        assert_eq!(
            parse_tag("go1.26.4"),
            Some(("go".to_owned(), Release(1, 26, 4)))
        );
        assert_eq!(
            parse_tag("contrib/tally/v0.2.0"),
            Some(("contrib/tally/v".to_owned(), Release(0, 2, 0)))
        );
        assert_eq!(
            parse_tag("v1.32.0-rc.1"),
            None,
            "pre-releases are not releases"
        );
        assert_eq!(parse_tag("v0.7.0-67-gacac9e3f"), None);
        assert_eq!(parse_tag("main"), None);
    }

    #[test]
    fn newer_releases_and_commits_are_found() {
        let pins = RepositoryPins {
            repository: "https://github.com/example/server".to_owned(),
            releases: BTreeMap::from([(
                "v".to_owned(),
                vec![Release(1, 30, 6), Release(1, 31, 2)],
            )]),
            commits: vec![("5a71c36".to_owned(), Some("main".to_owned()))],
        };
        let upstream = Upstream::from_ls_remote(
            "aaa\trefs/tags/v1.30.6\nbbb\trefs/tags/v1.30.7\nccc\trefs/tags/v1.31.2\nddd\trefs/tags/v1.31.3\neee\trefs/tags/v1.32.0\nfff\trefs/tags/v1.33.0-rc.1\nggg\trefs/tags/contrib/x/v0.1.0\n",
            "0123456789abcdef\tHEAD\n",
        );
        let u = compare(&pins, &upstream);
        assert_eq!(u.releases[0].pinned, Release(1, 31, 2));
        assert_eq!(
            u.releases[0].patches,
            [Release(1, 30, 7), Release(1, 31, 3)]
        );
        assert_eq!(u.releases[0].newest, Some(Release(1, 32, 0)));
        assert_eq!(u.commits[0].head.as_deref(), Some("0123456789abcdef"));
        assert!(!u.is_current());

        let current = Upstream::from_ls_remote(
            "ccc\trefs/tags/v1.31.2\naaa\trefs/tags/v1.30.6\n",
            "5a71c3656cc0\tHEAD\n",
        );
        assert!(compare(&pins, &current).is_current());
    }

    #[test]
    fn a_release_another_pack_pins_is_not_new() {
        let pack_pins = |releases: Vec<Release>| RepositoryPins {
            repository: "https://github.com/example/server".to_owned(),
            releases: BTreeMap::from([("v".to_owned(), releases)]),
            commits: Vec::new(),
        };
        let mut by_repository = BTreeMap::new();
        merge(
            &mut by_repository,
            pack_pins(vec![Release(1, 30, 8), Release(1, 31, 4)]),
        );
        merge(
            &mut by_repository,
            pack_pins(vec![Release(1, 32, 0), Release(1, 32, 1)]),
        );
        let pins = by_repository.into_values().next().expect("one repository");
        let upstream = Upstream::from_ls_remote(
            "a\trefs/tags/v1.30.8\nb\trefs/tags/v1.31.4\nc\trefs/tags/v1.32.0\nd\trefs/tags/v1.32.1\n",
            "e\tHEAD\n",
        );
        assert!(
            compare(&pins, &upstream).is_current(),
            "{:?}",
            compare(&pins, &upstream)
        );
    }
}
