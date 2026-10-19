//! Re-verify provenance and references against local checkouts.
//!
//! Each provenance entry with a path, line and excerpt is checked, in every
//! source it names, by reading the file under the source's local root and
//! confirming the excerpt appears on the recorded line (or within a few lines
//! of it, reported as drift).
//!
//! Each reference is checked in its source: the file exists, the document's
//! title and the section's heading and anchor are as recorded, and a panel is
//! in its dashboard under its title. A source's checkout must be at the commit
//! the source pins, when the checkout says which commit it is at.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use kyumei_domain::{ClaimId, ReferenceId, SourceId};
use serde_json::Value;

use crate::{
    markdown,
    model::{Pack, ReferenceDef, ReferenceKind, ReferenceVersion, VersionOf},
};

/// How far from the recorded line an excerpt may be and still count as found.
pub const DRIFT_WINDOW: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenanceCheck {
    pub claim: ClaimId,
    pub source: SourceId,
    pub path: String,
    pub line: u32,
    pub outcome: CheckOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The excerpt is on the recorded line.
    Confirmed,
    /// The excerpt is nearby; the recorded line should be updated.
    Drifted { found_at: u32 },
    /// The excerpt was not found near the recorded line.
    NotFound,
    /// The file does not exist under the root.
    FileMissing,
    /// No local root was supplied for the source.
    SourceUnavailable,
    /// The entry has no excerpt or line to check.
    NotCheckable,
}

impl CheckOutcome {
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::NotFound | Self::FileMissing)
    }

    pub fn label(&self) -> String {
        match self {
            Self::Confirmed => "confirmed".to_owned(),
            Self::Drifted { found_at } => format!("found at line {found_at}"),
            Self::NotFound => "EXCERPT NOT FOUND".to_owned(),
            Self::FileMissing => "FILE MISSING".to_owned(),
            Self::SourceUnavailable => "source not available locally".to_owned(),
            Self::NotCheckable => "nothing to check".to_owned(),
        }
    }
}

/// Check every provenance entry of every claim, in every source it names. `roots` maps
/// source IDs to local checkouts.
pub fn verify(pack: &Pack, roots: &BTreeMap<SourceId, PathBuf>) -> Vec<ProvenanceCheck> {
    let mut out = Vec::new();
    // A file's lines, normalised, read once however many claims cite it.
    let mut files: HashMap<PathBuf, Option<Vec<String>>> = HashMap::new();
    for claim in pack.claims.values() {
        for p in claim.all_provenance() {
            for source in &p.sources {
                let (Some(path), Some(line), Some(excerpt)) = (&p.path, p.line, &p.excerpt) else {
                    out.push(ProvenanceCheck {
                        claim: claim.id().clone(),
                        source: source.clone(),
                        path: p.path.clone().unwrap_or_default(),
                        line: p.line.unwrap_or_default(),
                        outcome: CheckOutcome::NotCheckable,
                    });
                    continue;
                };
                let outcome = match roots.get(source) {
                    None => CheckOutcome::SourceUnavailable,
                    Some(root) => {
                        let file = root.join(path);
                        let lines = files.entry(file.clone()).or_insert_with(|| {
                            fs::read_to_string(&file)
                                .ok()
                                .map(|text| text.lines().map(normalise).collect())
                        });
                        match lines {
                            None => CheckOutcome::FileMissing,
                            Some(lines) => locate_in(lines, line, &normalise(excerpt)),
                        }
                    }
                };
                out.push(ProvenanceCheck {
                    claim: claim.id().clone(),
                    source: source.clone(),
                    path: path.clone(),
                    line,
                    outcome,
                });
            }
        }
    }
    out
}

/// One reference checked in its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceCheck {
    pub reference: ReferenceId,
    pub source: SourceId,
    pub path: String,
    pub outcome: ReferenceOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceOutcome {
    Confirmed,
    /// No local root was supplied for the source.
    SourceUnavailable,
    /// The checkout is at another commit than the one the source pins.
    WrongCommit {
        found: String,
    },
    FileMissing,
    /// The document's first heading, or the dashboard's title, is not the recorded one.
    TitleChanged {
        found: String,
    },
    /// No heading has the recorded text.
    SectionMissing,
    /// The heading is there, but an earlier heading took its anchor.
    AnchorChanged {
        found: String,
    },
    NotADashboard {
        reason: String,
    },
    /// The dashboard has another uid.
    DashboardChanged {
        found: String,
    },
    PanelMissing,
    PanelTitleChanged {
        found: String,
    },
    /// The version declaration is no longer on its recorded line, or the dashboard's
    /// title names another version than its README.
    VersionChanged {
        found: String,
    },
}

impl ReferenceOutcome {
    pub fn is_failure(&self) -> bool {
        !matches!(self, Self::Confirmed | Self::SourceUnavailable)
    }

    pub fn label(&self) -> String {
        match self {
            Self::Confirmed => "confirmed".to_owned(),
            Self::SourceUnavailable => "source not available locally".to_owned(),
            Self::WrongCommit { found } => format!("CHECKOUT IS AT {found}, NOT THE PINNED COMMIT"),
            Self::FileMissing => "FILE MISSING".to_owned(),
            Self::TitleChanged { found } => format!("TITLE IS NOW “{found}”"),
            Self::SectionMissing => "SECTION MISSING".to_owned(),
            Self::AnchorChanged { found } => format!("SECTION'S ANCHOR IS NOW #{found}"),
            Self::NotADashboard { reason } => format!("NOT A DASHBOARD: {reason}"),
            Self::DashboardChanged { found } => format!("DASHBOARD UID IS NOW {found}"),
            Self::PanelMissing => "PANEL MISSING".to_owned(),
            Self::PanelTitleChanged { found } => format!("PANEL'S TITLE IS NOW “{found}”"),
            Self::VersionChanged { found } => format!("VERSION NOT WHERE RECORDED: {found}"),
        }
    }
}

/// Check every reference in its source. `roots` maps source IDs to local checkouts.
pub fn verify_references(pack: &Pack, roots: &BTreeMap<SourceId, PathBuf>) -> Vec<ReferenceCheck> {
    let mut commits: HashMap<&SourceId, Option<String>> = HashMap::new();
    pack.references
        .iter()
        .map(|r| {
            let outcome = match roots.get(&r.source) {
                None => ReferenceOutcome::SourceUnavailable,
                Some(root) => {
                    let pinned = pack.sources.get(&r.source).and_then(|s| s.commit.clone());
                    let at = commits
                        .entry(&r.source)
                        .or_insert_with(|| checkout_commit(root));
                    match (pinned, at.as_ref()) {
                        (Some(pinned), Some(at))
                            if !at.starts_with(&pinned) && !pinned.starts_with(at.as_str()) =>
                        {
                            ReferenceOutcome::WrongCommit { found: at.clone() }
                        }
                        _ => check_reference(r, root),
                    }
                }
            };
            ReferenceCheck {
                reference: r.id.clone(),
                source: r.source.clone(),
                path: r.path.clone(),
                outcome,
            }
        })
        .collect()
}

fn check_reference(r: &ReferenceDef, root: &Path) -> ReferenceOutcome {
    let Ok(text) = fs::read_to_string(root.join(&r.path)) else {
        return ReferenceOutcome::FileMissing;
    };
    let outcome = match r.kind {
        ReferenceKind::Runbook | ReferenceKind::Playbook => check_document(r, &text),
        ReferenceKind::Panel => check_panel(r, &text),
    };
    match (&outcome, &r.version) {
        (ReferenceOutcome::Confirmed, Some(v)) => check_version(r, v, root),
        _ => outcome,
    }
}

/// The version's declaration is on its line, and a dashboard's title agrees with it.
fn check_version(r: &ReferenceDef, v: &ReferenceVersion, root: &Path) -> ReferenceOutcome {
    let Ok(text) = fs::read_to_string(root.join(&v.path)) else {
        return ReferenceOutcome::FileMissing;
    };
    let line = text.lines().nth(v.line as usize - 1).unwrap_or_default();
    if !line.contains(&v.excerpt) {
        return ReferenceOutcome::VersionChanged {
            found: line.trim().chars().take(160).collect(),
        };
    }
    if v.of == VersionOf::Dashboard
        && let Some(p) = &r.panel
        && !p.dashboard_title.contains(&v.version)
    {
        return ReferenceOutcome::VersionChanged {
            found: format!("the dashboard's title is “{}”", p.dashboard_title),
        };
    }
    ReferenceOutcome::Confirmed
}

fn check_document(r: &ReferenceDef, text: &str) -> ReferenceOutcome {
    let headings = markdown::headings(text);
    let title = headings
        .first()
        .map(|h| h.text.as_str())
        .unwrap_or_default();
    if normalise(title) != normalise(&r.title) {
        return ReferenceOutcome::TitleChanged {
            found: title.to_owned(),
        };
    }
    let Some(section) = &r.section else {
        return ReferenceOutcome::Confirmed;
    };
    match headings
        .iter()
        .find(|h| normalise(&h.text) == normalise(section))
    {
        None => ReferenceOutcome::SectionMissing,
        Some(h) if Some(&h.anchor) != r.anchor().as_ref() => ReferenceOutcome::AnchorChanged {
            found: h.anchor.clone(),
        },
        Some(_) => ReferenceOutcome::Confirmed,
    }
}

fn check_panel(r: &ReferenceDef, text: &str) -> ReferenceOutcome {
    let Some(panel) = &r.panel else {
        return ReferenceOutcome::PanelMissing;
    };
    let dashboard: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return ReferenceOutcome::NotADashboard {
                reason: e.to_string(),
            };
        }
    };
    let uid = dashboard["uid"].as_str().unwrap_or_default();
    if uid != panel.dashboard {
        return ReferenceOutcome::DashboardChanged {
            found: uid.to_owned(),
        };
    }
    let title = dashboard["title"].as_str().unwrap_or_default();
    if normalise(title) != normalise(&panel.dashboard_title) {
        return ReferenceOutcome::TitleChanged {
            found: title.to_owned(),
        };
    }
    match find_panel(&dashboard["panels"], panel.id) {
        None => ReferenceOutcome::PanelMissing,
        Some(p) => {
            let found = p["title"].as_str().unwrap_or_default();
            if normalise(found) == normalise(&r.title) {
                ReferenceOutcome::Confirmed
            } else {
                ReferenceOutcome::PanelTitleChanged {
                    found: found.to_owned(),
                }
            }
        }
    }
}

/// A panel by `id`, among the panels and the rows' panels.
fn find_panel(panels: &Value, id: i64) -> Option<&Value> {
    panels.as_array()?.iter().find_map(|p| {
        if p["id"].as_i64() == Some(id) {
            Some(p)
        } else {
            find_panel(&p["panels"], id)
        }
    })
}

/// The commit a Git checkout is at, read from `.git` without running Git: a detached
/// `HEAD`, or the branch it names, loose or packed. `None` when the root is not a
/// checkout (such as a `git archive` export) or the commit cannot be read.
pub fn checkout_commit(root: &Path) -> Option<String> {
    let mut git = root.join(".git");
    if git.is_file() {
        // A worktree: `.git` names the real directory.
        let pointer = fs::read_to_string(&git).ok()?;
        git = PathBuf::from(pointer.strip_prefix("gitdir:")?.trim());
    }
    let head = fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref:") else {
        return is_commit(head).then(|| head.to_owned());
    };
    let reference = reference.trim();
    if let Ok(loose) = fs::read_to_string(git.join(reference)) {
        let loose = loose.trim();
        return is_commit(loose).then(|| loose.to_owned());
    }
    let common = fs::read_to_string(git.join("commondir"))
        .ok()
        .map(|c| git.join(c.trim()))
        .unwrap_or(git);
    fs::read_to_string(common.join("packed-refs"))
        .ok()?
        .lines()
        .find_map(|l| {
            let (commit, name) = l.split_once(' ')?;
            (name == reference && is_commit(commit)).then(|| commit.to_owned())
        })
}

fn is_commit(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn normalise(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
fn locate(text: &str, line: u32, excerpt: &str) -> CheckOutcome {
    let lines: Vec<String> = text.lines().map(normalise).collect();
    locate_in(&lines, line, &normalise(excerpt))
}

/// Find a normalised excerpt on `line` of normalised `lines`, or within the drift window.
fn locate_in(lines: &[String], line: u32, needle: &str) -> CheckOutcome {
    let at = |n: u32| -> bool {
        n >= 1 && (n as usize) <= lines.len() && lines[n as usize - 1].contains(needle)
    };
    if at(line) {
        return CheckOutcome::Confirmed;
    }
    for d in 1..=DRIFT_WINDOW {
        for candidate in [line.saturating_sub(d), line + d] {
            if at(candidate) {
                return CheckOutcome::Drifted {
                    found_at: candidate,
                };
            }
        }
    }
    CheckOutcome::NotFound
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_excerpts_with_drift() {
        let text = "a\nb\n  const X =   4\nd\n";
        assert_eq!(locate(text, 3, "const X = 4"), CheckOutcome::Confirmed);
        assert_eq!(
            locate(text, 2, "const X = 4"),
            CheckOutcome::Drifted { found_at: 3 }
        );
        assert_eq!(locate(text, 3, "const Y"), CheckOutcome::NotFound);
    }
}
