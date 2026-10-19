//! `kyumei knowledge updates`: what is newer upstream than the sources the knowledge
//! pins. It reads, and changes nothing; moving a pin stays a reviewed change.

use std::{collections::BTreeSet, process::Stdio, time::Duration};

use anyhow::{Context as _, Result};
use kyumei_knowledge::{
    KnowledgeBase, Pack, ReferenceKind, VersionOf,
    updates::{self, RepositoryPins, RepositoryUpdate, Upstream},
};
use serde::Serialize;
use tokio::{process::Command, task::JoinSet, time::timeout};

const GIT_TIMEOUT: Duration = Duration::from_secs(60);

/// What changed in a GitHub repository between its pinned commit and its head.
#[derive(Debug, Default, Serialize)]
struct Changes {
    commits: u64,
    files: Vec<String>,
    /// Files the knowledge cites that changed.
    cited: Vec<String>,
    /// `Temporal Server Dashboard: v2.25.0 → v2.26.0`.
    versions: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Report {
    #[serde(flatten)]
    update: Option<RepositoryUpdate>,
    repository: String,
    changes: Option<Changes>,
    error: Option<String>,
}

pub async fn run(kb: &KnowledgeBase, json: bool) -> Result<()> {
    let mut reports = Vec::new();
    // A release is new only if no pack pins it, so the packs' pins are read together.
    {
        let mut tasks = JoinSet::new();
        for pins in updates::pins_across(kb.packs()) {
            tasks.spawn(async move {
                let upstream = ls_remote(&pins.repository).await;
                (pins, upstream)
            });
        }
        let mut found = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            found.push(joined.context("a repository check panicked")?);
        }
        found.sort_by(|a, b| a.0.repository.cmp(&b.0.repository));
        let http = reqwest::Client::builder()
            .user_agent(concat!("kyumei/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()?;
        for (pins, upstream) in found {
            reports.push(match upstream {
                Err(e) => Report {
                    repository: pins.repository.clone(),
                    update: None,
                    changes: None,
                    error: Some(e),
                },
                Ok(upstream) => {
                    let update = updates::compare(&pins, &upstream);
                    let changes = match update
                        .commits
                        .iter()
                        .find_map(|c| c.head.as_ref().map(|h| (c.pinned.clone(), h.clone())))
                    {
                        Some((pinned, head)) => match pinning_pack(kb, &pins.repository, &pinned) {
                            Some(pack) => changes(&http, pack, &pins, &pinned, &head).await.ok(),
                            None => None,
                        },
                        None => None,
                    };
                    Report {
                        repository: pins.repository.clone(),
                        update: Some(update),
                        changes,
                        error: None,
                    }
                }
            });
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
        return Ok(());
    }
    print_reports(&reports);
    Ok(())
}

/// A pack that pins `commit` in `repository`, to say which of its cited files changed.
fn pinning_pack<'k>(kb: &'k KnowledgeBase, repository: &str, commit: &str) -> Option<&'k Pack> {
    kb.packs().iter().find(|p| {
        updates::pins(p)
            .iter()
            .any(|q| q.repository == repository && q.commits.iter().any(|(c, _)| c == commit))
    })
}

/// The repository's release tags and head commit, from `git ls-remote`.
async fn ls_remote(repository: &str) -> std::result::Result<Upstream, String> {
    let run = |args: Vec<&str>| {
        let mut c = Command::new("git");
        c.args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    };
    let tags = timeout(
        GIT_TIMEOUT,
        run(vec!["ls-remote", "--tags", "--refs", repository]).output(),
    );
    let head = timeout(
        GIT_TIMEOUT,
        run(vec!["ls-remote", repository, "HEAD"]).output(),
    );
    let (tags, head) = tokio::join!(tags, head);
    let read = |r: std::result::Result<std::io::Result<std::process::Output>, _>| match r {
        Err(_) => Err(format!(
            "no answer within {} seconds",
            GIT_TIMEOUT.as_secs()
        )),
        Ok(Err(e)) => Err(format!("git could not run: {e}")),
        Ok(Ok(o)) if !o.status.success() => Err(String::from_utf8_lossy(&o.stderr)
            .lines()
            .last()
            .unwrap_or("git ls-remote failed")
            .to_owned()),
        Ok(Ok(o)) => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
    };
    Ok(Upstream::from_ls_remote(&read(tags)?, &read(head)?))
}

/// For a GitHub repository: the commits and files since the pinned commit, the
/// cited files among them, and dashboard versions that moved.
async fn changes(
    http: &reqwest::Client,
    pack: &Pack,
    pins: &RepositoryPins,
    pinned: &str,
    head: &str,
) -> Result<Changes> {
    let Some(name) = pins
        .repository
        .trim_end_matches('/')
        .strip_prefix("https://github.com/")
    else {
        anyhow::bail!("not a GitHub repository");
    };
    let get = |url: String| {
        let mut r = http
            .get(url)
            .header("Accept", "application/vnd.github+json");
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            r = r.bearer_auth(token);
        }
        r
    };
    let compare: serde_json::Value = get(format!(
        "https://api.github.com/repos/{name}/compare/{pinned}...{head}"
    ))
    .send()
    .await?
    .error_for_status()?
    .json()
    .await?;
    let files: Vec<String> = compare["files"]
        .as_array()
        .map(|f| {
            f.iter()
                .filter_map(|x| x["filename"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    // What the knowledge cites in this repository.
    let sources: BTreeSet<_> = pack
        .sources
        .values()
        .filter(|s| s.repository.as_deref() == Some(pins.repository.as_str()))
        .map(|s| s.id.clone())
        .collect();
    let references: Vec<_> = pack
        .references
        .iter()
        .filter(|r| sources.contains(&r.source))
        .collect();
    let cited_paths: BTreeSet<&str> = references
        .iter()
        .flat_map(|r| {
            std::iter::once(r.path.as_str()).chain(r.version.as_ref().map(|v| v.path.as_str()))
        })
        .collect();
    let cited: Vec<String> = files
        .iter()
        .filter(|f| cited_paths.contains(f.as_str()))
        .cloned()
        .collect();
    // Dashboards whose title gives another version now.
    let mut versions = Vec::new();
    let mut seen = BTreeSet::new();
    for r in references
        .iter()
        .filter(|r| r.kind == ReferenceKind::Panel && cited.contains(&r.path))
    {
        let (Some(v), Some(p)) = (&r.version, &r.panel) else {
            continue;
        };
        if v.of != VersionOf::Dashboard || !seen.insert(r.path.clone()) {
            continue;
        }
        let raw: serde_json::Value = get(format!(
            "https://raw.githubusercontent.com/{name}/{head}/{}",
            r.path
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
        let title = raw["title"].as_str().unwrap_or_default();
        if !title.contains(&v.version) {
            versions.push(format!("{}: {} → “{title}”", p.dashboard_title, v.version));
        }
    }
    Ok(Changes {
        commits: compare["ahead_by"].as_u64().unwrap_or_default(),
        files,
        cited,
        versions,
    })
}

fn print_reports(reports: &[Report]) {
    println!(
        "Upstream of the {} repositories the knowledge pins:",
        reports.len()
    );
    let mut current = 0;
    for r in reports {
        let name = r
            .repository
            .trim_end_matches('/')
            .trim_start_matches("https://github.com/")
            .trim_start_matches("https://");
        if let Some(e) = &r.error {
            println!("? {name}: not checked: {e}");
            continue;
        }
        let Some(u) = &r.update else { continue };
        if u.is_current() {
            current += 1;
            continue;
        }
        println!("• {name}");
        for rel in &u.releases {
            let line = |v: &updates::Release| format!("{}{v}", rel.prefix);
            if !rel.patches.is_empty() {
                println!(
                    "    patches after the pinned releases: {}",
                    rel.patches.iter().map(line).collect::<Vec<_>>().join(", ")
                );
            }
            if let Some(n) = &rel.newest {
                println!(
                    "    newest release {} (pinned up to {})",
                    line(n),
                    line(&rel.pinned)
                );
            }
        }
        for c in u.commits.iter().filter(|c| c.head.is_some()) {
            let head = c.head.as_deref().unwrap_or_default();
            let branch = c
                .branch
                .as_deref()
                .map(|b| format!(" on {b}"))
                .unwrap_or_default();
            match &r.changes {
                Some(ch) => {
                    // GitHub lists at most 300 files.
                    let files = match ch.files.len() {
                        n if n >= 300 => "at least 300 files".to_owned(),
                        n => plural(n as u64, "file"),
                    };
                    println!(
                        "    {}{branch} since {}, now at {}; {files} changed",
                        plural(ch.commits, "commit"),
                        short(&c.pinned),
                        short(head),
                    );
                    if !ch.cited.is_empty() {
                        println!("    cited files changed: {}", ch.cited.join(", "));
                    }
                    for v in &ch.versions {
                        println!("    dashboard version moved: {v}");
                    }
                }
                None => println!(
                    "    pinned at {}{branch}; now at {}",
                    short(&c.pinned),
                    short(head)
                ),
            }
        }
    }
    println!("{current} of {} repositories up to date.", reports.len());
    if current < reports.len() {
        println!(
            "Moving a pin is a reviewed change: add or update the source in the pack's sources.toml, then re-run `kyumei knowledge verify`."
        );
    }
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}
