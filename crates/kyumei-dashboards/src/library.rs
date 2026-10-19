//! The dashboards Kyumei has loaded, and the panels that read a metric.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    grafana::{self, Dashboard, Panel, Target},
    promql::{self, Query},
};

/// Where a dashboard came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A file under the directory dashboards were loaded from, relative to it.
    File(PathBuf),
    /// Uploaded through the interface, under the name it was uploaded with.
    Upload(String),
}

impl Origin {
    pub fn describe(&self) -> String {
        match self {
            Self::File(p) => p.display().to_string(),
            Self::Upload(name) => format!("uploaded as {name}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub dashboard: Dashboard,
    pub origin: Origin,
}

/// A file under the dashboards directory that is not a dashboard Kyumei can read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadProblem {
    pub path: PathBuf,
    pub reason: String,
}

/// One query of a chart panel that reads a metric.
#[derive(Clone, Debug)]
pub struct PanelMatch<'a> {
    pub entry: &'a Entry,
    pub panel: &'a Panel,
    pub target: &'a Target,
    pub query: Query,
    /// The metric the query reads, as written in it.
    pub metric: String,
}

#[derive(Clone, Debug, Default)]
pub struct Library {
    entries: Vec<Entry>,
}

impl Library {
    /// Every `.json` file under `dir`, in path order. Files that are not dashboards are
    /// listed with the reason, and a later dashboard with a uid already loaded replaces
    /// the earlier one.
    pub fn load(dir: &Path) -> (Self, Vec<LoadProblem>) {
        let mut files = Vec::new();
        collect(dir, &mut files);
        files.sort();
        let mut library = Self::default();
        let mut problems = Vec::new();
        for path in files {
            let relative = path.strip_prefix(dir).unwrap_or(&path).to_path_buf();
            match fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| grafana::parse(&text).map_err(|e| e.to_string()))
            {
                Ok(dashboard) => library.add(dashboard, Origin::File(relative)),
                Err(reason) => problems.push(LoadProblem {
                    path: relative,
                    reason,
                }),
            }
        }
        (library, problems)
    }

    /// Add a dashboard, replacing one with the same uid.
    pub fn add(&mut self, dashboard: Dashboard, origin: Origin) {
        self.entries.retain(|e| e.dashboard.uid != dashboard.uid);
        self.entries.push(Entry { dashboard, origin });
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, uid: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.dashboard.uid == uid)
    }

    /// The chart panels with a query reading any of `names`. A name matches a
    /// histogram's `_bucket`, `_sum` and `_count` series, and a counter's `_total`.
    pub fn panels_reading(&self, names: &[&str]) -> Vec<PanelMatch<'_>> {
        let mut out = Vec::new();
        for entry in &self.entries {
            for panel in entry.dashboard.panels.iter().filter(|p| p.is_chart()) {
                for target in panel.targets.iter().filter(|t| !t.hidden) {
                    let query = promql::parse(&target.expr);
                    let metric = query
                        .metrics()
                        .into_iter()
                        .find(|m| names.iter().any(|n| same_metric(m, n)))
                        .map(str::to_owned);
                    if let Some(metric) = metric {
                        out.push(PanelMatch {
                            entry,
                            panel,
                            target,
                            query,
                            metric,
                        });
                    }
                }
            }
        }
        out
    }
}

/// A metric name without the suffixes Prometheus adds to histograms and counters.
pub fn base_name(name: &str) -> &str {
    ["_bucket", "_sum", "_count", "_total"]
        .iter()
        .find_map(|s| name.strip_suffix(s))
        .unwrap_or(name)
}

fn same_metric(a: &str, b: &str) -> bool {
    a == b || base_name(a) == base_name(b)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panels_are_found_by_the_metrics_they_read() {
        let dir = std::env::temp_dir().join(format!("kyumei-dashboards-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("server")).expect("dir");
        fs::write(
            dir.join("server/test.json"),
            crate::grafana::tests::DASHBOARD,
        )
        .expect("write");
        fs::write(dir.join("notes.json"), r#"{"not": "a dashboard"}"#).expect("write");
        fs::write(dir.join("readme.md"), "not json at all").expect("write");
        let (library, problems) = Library::load(&dir);
        assert_eq!(library.entries().len(), 1);
        assert_eq!(
            library.entries()[0].origin,
            Origin::File(PathBuf::from("server/test.json"))
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].path, PathBuf::from("notes.json"));

        let found = library.panels_reading(&["queue_latency"]);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].panel.id, found[0].metric.as_str()),
            (3, "queue_latency_bucket")
        );
        assert_eq!(
            library.panels_reading(&["requests"]).len(),
            1,
            "a counter's _total"
        );
        assert!(
            library.panels_reading(&["up"]).is_empty(),
            "stat panels are not charts"
        );

        // A dashboard with the same uid replaces the loaded one.
        let mut library = library;
        let mut again = library.entries()[0].dashboard.clone();
        again.title = "Again".to_owned();
        library.add(again, Origin::Upload("again.json".to_owned()));
        assert_eq!(library.entries().len(), 1);
        assert_eq!(
            library.get("test-v1").map(|e| e.dashboard.title.as_str()),
            Some("Again")
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
