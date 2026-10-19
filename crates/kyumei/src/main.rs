//! `kyumei` — evidence-driven diagnostics for Temporal.

mod updates;

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use kyumei_app::{
    InvestigationService, LocalInvestigations, SystemClock,
    config::Config,
    fixture::{self, Fixture, FixtureClock},
};
use kyumei_collect::{Collector, Endpoint};
use kyumei_knowledge::{KnowledgeBase, verify};
use kyumei_odori::{DurableConfig, DurableInvestigations, Explainer};

#[derive(Debug, Parser)]
#[command(
    name = "kyumei",
    version,
    about = "Kyumei 究明 — evidence-driven diagnostics for Temporal"
)]
struct Cli {
    /// Load knowledge from this directory instead of the built-in packs.
    #[arg(long, global = true)]
    knowledge_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Serve the investigation workspace.
    Serve(Box<ServeArgs>),
    /// Replay a fixture investigation and print each step. No model is used.
    Replay {
        /// Fixture name under fixtures/investigations, or a path to one.
        #[arg(default_value = "signal-flood-wft-latency")]
        fixture: String,
        /// Run on Odori durable workflows instead of in process.
        #[arg(long)]
        durable: bool,
    },
    /// Inspect and verify the knowledge packs.
    Knowledge {
        #[command(subcommand)]
        command: KnowledgeCommand,
    },
    /// Read Grafana dashboards as Kyumei draws evidence with them.
    Dashboards {
        #[command(subcommand)]
        command: DashboardsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum DashboardsCommand {
    /// Load the dashboards under DIR, list the panels Kyumei cannot read, and check
    /// every panel's queries against what the knowledge says about the metrics.
    Check {
        /// A directory of dashboard JSON files, searched recursively.
        dir: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum KnowledgeCommand {
    /// Load and validate the knowledge, and summarise it.
    Check,
    /// Re-verify every provenance excerpt against local source checkouts.
    Verify {
        /// `source-id=path`, e.g. `temporal-server-v1.31.0=../temporal`. Repeatable.
        #[arg(long = "source", value_name = "ID=PATH")]
        sources: Vec<String>,
    },
    /// List what is newer upstream than the sources the knowledge pins: releases
    /// after the pinned ones, and commits after pinned commits. Reads tags and heads
    /// with `git ls-remote`, and GitHub's compare API (with $GITHUB_TOKEN when set);
    /// changes nothing.
    Updates {
        /// Print the findings as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ExplainerArg {
    None,
    Claude,
    Codex,
}

#[derive(Debug, clap::Args)]
struct ServeArgs {
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:7373")]
    addr: SocketAddr,
    /// Where investigations, the engine snapshot and blobs are kept. Without it,
    /// `kyumei` in the platform's local data directory (~/.local/share on Linux,
    /// ~/Library/Application Support on macOS, %LOCALAPPDATA% on Windows).
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Preferred time zones and environment profiles. Without it, $KYUMEI_CONFIG,
    /// else `kyumei/config.toml` in the platform's configuration directory
    /// (~/.config on Linux, ~/Library/Application Support on macOS, %APPDATA% on
    /// Windows), read when it exists.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Run investigations in process (journalled) instead of on Odori.
    #[arg(long)]
    in_process: bool,
    /// Have a model reword explanations (validated against the evidence).
    #[arg(long, value_enum, default_value_t = ExplainerArg::None)]
    explainer: ExplainerArg,
    /// Don't run the simulations that check whether a cause is big enough.
    #[arg(long)]
    no_simulate: bool,
    /// Preload the reference investigation, replayed up to this many evidence steps.
    #[arg(long, value_name = "STEPS")]
    demo: Option<usize>,
    /// Also preload this fixture's investigation, replayed in full. Repeatable.
    #[arg(long = "demo-fixture", value_name = "NAME")]
    demo_fixtures: Vec<String>,
    /// Seconds between engine snapshots.
    #[arg(long, default_value_t = 5)]
    snapshot_interval: u64,
    /// Grafana dashboards to draw evidence like: a directory of dashboard JSON, such as
    /// tsurdilo/temporal-server-operations's observability/dashboards. Dashboards
    /// uploaded in the interface are kept in the data directory and load too.
    #[arg(long, value_name = "DIR")]
    dashboards: Option<PathBuf>,
    #[command(flatten)]
    metrics: MetricsArgs,
}

/// Where Collect now reads a request's PromQL queries. Credentials come from
/// files or the environment, as for `kyumei-collect`, never from the command
/// line.
#[derive(Debug, clap::Args)]
#[command(next_help_heading = "Collect now")]
struct MetricsArgs {
    /// Read requests' queries from this Prometheus API (or Thanos, Mimir,
    /// VictoriaMetrics), such as http://prometheus:9090.
    #[arg(long, value_name = "URL", conflicts_with = "grafana_datasource")]
    prometheus_url: Option<String>,
    /// Your Grafana: dashboard panels link to it, and with --grafana-datasource Collect
    /// now reads through its data source proxy.
    #[arg(long, value_name = "URL")]
    grafana_url: Option<String>,
    /// The Prometheus data source's UID in Grafana, for Collect now.
    #[arg(long, value_name = "UID", requires = "grafana_url")]
    grafana_datasource: Option<String>,
    /// A Mimir, Cortex or Thanos tenant, sent as X-Scope-OrgID.
    #[arg(long, value_name = "TENANT")]
    metrics_tenant: Option<String>,
    /// A file holding a bearer token. Without it, $KYUMEI_COLLECT_TOKEN is used.
    #[arg(long, value_name = "FILE")]
    metrics_token_file: Option<PathBuf>,
    /// A user for basic authentication. The password comes from
    /// --metrics-password-file or $KYUMEI_COLLECT_PASSWORD.
    #[arg(long, value_name = "USER")]
    metrics_user: Option<String>,
    /// A file holding the basic authentication password.
    #[arg(long, value_name = "FILE")]
    metrics_password_file: Option<PathBuf>,
    /// Extra CA certificates to trust, as PEM.
    #[arg(long, value_name = "FILE")]
    metrics_ca_file: Option<PathBuf>,
}

impl MetricsArgs {
    /// The collector for Collect now, if an endpoint is given.
    fn collector(&self) -> Result<Option<Arc<Collector>>> {
        let endpoint = match (
            &self.prometheus_url,
            &self.grafana_url,
            &self.grafana_datasource,
        ) {
            (Some(url), _, _) => Endpoint::prometheus(url)?,
            (None, Some(url), Some(uid)) => Endpoint::grafana(url, uid)?,
            _ => return Ok(None),
        };
        let endpoint = endpoint
            .with_credentials(
                |k| std::env::var(k).ok(),
                self.metrics_token_file.as_deref(),
                self.metrics_user.clone(),
                self.metrics_password_file.as_deref(),
            )?
            .with_tenant(self.metrics_tenant.clone())
            .with_ca_file(self.metrics_ca_file.as_deref())?;
        Ok(Some(Arc::new(Collector::new(endpoint)?)))
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "kyumei=info,temporalio_sdk=error,temporalio_sdk_core=error,warn".into()
            }),
        )
        .with_target(false)
        .init();
    let cli = Cli::parse();
    let kb = Arc::new(match &cli.knowledge_dir {
        Some(dir) => KnowledgeBase::load_dir(dir)?,
        None => KnowledgeBase::builtin()?,
    });
    match cli.command {
        Command::Serve(args) => serve(kb, *args).await,
        Command::Replay { fixture, durable } => replay(kb, &fixture, durable).await,
        Command::Knowledge {
            command: KnowledgeCommand::Updates { json },
        } => updates::run(&kb, json).await,
        Command::Knowledge { command } => knowledge(&kb, command),
        Command::Dashboards { command } => dashboards(&kb, command),
    }
}

/// `kyumei/config.toml` in the platform's configuration directory.
fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("kyumei").join("config.toml"))
}

/// `kyumei` in the platform's local data directory, or `.kyumei` in the working
/// directory on a platform without one.
fn default_data_dir() -> PathBuf {
    dirs::data_local_dir().map_or_else(|| PathBuf::from(".kyumei"), |d| d.join("kyumei"))
}

async fn serve(kb: Arc<KnowledgeBase>, args: ServeArgs) -> Result<()> {
    let data_dir = args.data_dir.clone().unwrap_or_else(default_data_dir);
    std::fs::create_dir_all(&data_dir).with_context(|| format!("create {}", data_dir.display()))?;
    let data_dir = data_dir.canonicalize()?;
    let config = match args
        .config
        .clone()
        .or_else(|| std::env::var_os("KYUMEI_CONFIG").map(PathBuf::from))
    {
        Some(path) => Config::load(&path)?,
        None => match default_config_path() {
            Some(path) => Config::load_if_present(&path)?,
            None => Config::default(),
        },
    };
    let collector = args.metrics.collector()?;
    let listener = tokio::net::TcpListener::bind(args.addr)
        .await
        .with_context(|| format!("bind {}", args.addr))?;

    let (service, durable): (
        Arc<dyn InvestigationService>,
        Option<Arc<DurableInvestigations>>,
    ) = if args.in_process {
        let mut local = LocalInvestigations::new(kb.clone(), Arc::new(SystemClock))
            .with_journal(data_dir.join("journal"))?;
        if !args.no_simulate {
            local = local.with_simulation();
        }
        (Arc::new(local), None)
    } else {
        let explainer = match args.explainer {
            ExplainerArg::None => None,
            ExplainerArg::Claude => Some(Explainer::Claude),
            ExplainerArg::Codex => Some(Explainer::Codex),
        };
        if let Some(e) = &explainer {
            if let Err(problem) = e.preflight().await {
                bail!("the {} explainer is not available: {problem}", e.label());
            }
            // Model harnesses run in the process's working directory; give
            // them an empty one rather than this repository or the user's home.
            let sandbox = data_dir.join("agent-sandbox");
            std::fs::create_dir_all(&sandbox)?;
            std::env::set_current_dir(&sandbox)?;
        }
        let config = DurableConfig {
            data_dir: data_dir.join("odori"),
            snapshot_interval: Duration::from_secs(args.snapshot_interval.max(1)),
            explainer,
            simulate: !args.no_simulate,
        };
        let durable = Arc::new(DurableInvestigations::start(kb.clone(), config).await?);
        (durable.clone(), Some(durable))
    };

    if let Some(steps) = args.demo {
        preload_demo(&kb, service.as_ref(), &fixture::golden()?, steps).await?;
    }
    for name in &args.demo_fixtures {
        let fx = find_fixture(name)?;
        let steps = fx.steps.len();
        preload_demo(&kb, service.as_ref(), &fx, steps).await?;
    }

    let mode = service.describe();
    let reads = collector.as_ref().map(|c| c.endpoint().label.clone());
    let uploads = data_dir.join("dashboards");
    let library = load_dashboards(args.dashboards.as_deref(), &uploads);
    let grafana_url = args.metrics.grafana_url.clone();
    let router = kyumei_web::router_for(kyumei_web::AppState {
        collector,
        dashboards: Arc::new(std::sync::RwLock::new(library)),
        dashboards_dir: Some(uploads),
        grafana_url,
        config: Arc::new(config.clone()),
        ..kyumei_web::AppState::new(kb, service, mode.clone())
    });
    let addr = listener.local_addr()?;
    eprintln!("Kyumei 究明 serving http://{addr}/");
    eprintln!("  {mode}");
    if let Some(source) = reads {
        eprintln!("  Collect now reads {source}");
    }
    if let Some(c) = config.describe() {
        eprintln!("  {c}");
    }
    kyumei_web::serve(listener, router).await?;
    if let Some(d) = durable {
        eprintln!("Stopping: draining workers and writing the engine snapshot…");
        d.shutdown().await?;
    }
    Ok(())
}

/// The dashboards given with `--dashboards`, then those uploaded earlier, which replace
/// a given one with the same uid. Files that are not dashboards are reported.
fn load_dashboards(dir: Option<&Path>, uploads: &Path) -> kyumei_dashboards::Library {
    let mut library = kyumei_dashboards::Library::default();
    for (source, dir) in dir
        .map(|d| ("--dashboards", d))
        .into_iter()
        .chain([("uploaded", uploads)])
    {
        if !dir.is_dir() {
            if source == "--dashboards" {
                eprintln!("  dashboards: {} is not a directory", dir.display());
            }
            continue;
        }
        let (loaded, problems) = kyumei_dashboards::Library::load(dir);
        for p in problems {
            eprintln!(
                "  dashboards: {} is not loaded: {}",
                dir.join(&p.path).display(),
                p.reason
            );
        }
        for e in loaded.entries() {
            library.add(e.dashboard.clone(), e.origin.clone());
        }
    }
    if !library.is_empty() {
        let unreadable: usize = library
            .entries()
            .iter()
            .map(|e| e.dashboard.unreadable.len())
            .sum();
        eprintln!(
            "  {} dashboards loaded{}",
            library.entries().len(),
            if unreadable > 0 {
                format!("; {unreadable} panels cannot be read (see /dashboards)")
            } else {
                String::new()
            }
        );
    }
    library
}

fn dashboards(kb: &KnowledgeBase, command: DashboardsCommand) -> Result<()> {
    match command {
        DashboardsCommand::Check { dir } => {
            let (library, problems) = kyumei_dashboards::Library::load(&dir);
            for p in &problems {
                println!("✕ {} is not a dashboard: {}", p.path.display(), p.reason);
            }
            // Every pack checks the panels, since a dashboard serves every release.
            let mut packs: Vec<&kyumei_knowledge::Pack> = kb.packs().iter().collect();
            packs.sort_by_key(|p| p.family());
            if packs.is_empty() {
                bail!("no knowledge pack");
            }
            let checks: Vec<(&str, kyumei_dashboards::Finding)> = packs
                .iter()
                .flat_map(|p| {
                    kyumei_app::dashboards::cross_check(&library, p)
                        .into_iter()
                        .map(|c| (p.manifest.server_family.as_str(), c))
                })
                .collect();
            for e in library.entries() {
                let d = &e.dashboard;
                let charts = d.panels.iter().filter(|p| p.is_chart()).count();
                println!(
                    "{} (uid {}{}): {} panels with queries, {} charts, from {}",
                    d.title,
                    d.uid,
                    d.version
                        .map(|v| format!(", version {v}"))
                        .unwrap_or_default(),
                    d.panels.len(),
                    charts,
                    e.origin.describe()
                );
                for u in &d.unreadable {
                    println!(
                        "  · panel {:?} “{}” is not read: {}",
                        u.id, u.title, u.reason
                    );
                }
                for (family, c) in checks.iter().filter(|(_, c)| c.dashboard == d.uid) {
                    println!(
                        "  [{family}] {} — panel {} “{}”: {}",
                        c.kind.label(),
                        c.panel,
                        c.panel_title,
                        c.detail
                    );
                }
            }
            let counts: Vec<String> = packs
                .iter()
                .map(|p| {
                    let family = p.manifest.server_family.as_str();
                    let n = checks.iter().filter(|(f, _)| *f == family).count();
                    format!("{n} from the {family} knowledge")
                })
                .collect();
            println!(
                "{} dashboards; differences: {}.",
                library.entries().len(),
                counts.join(", ")
            );
            Ok(())
        }
    }
}

/// Load a fixture's investigation into the service so the interface has
/// something to show. Skipped if it already exists.
async fn preload_demo(
    kb: &KnowledgeBase,
    service: &dyn InvestigationService,
    fx: &Fixture,
    steps: usize,
) -> Result<()> {
    let id = kyumei_domain::InvestigationId::new(&fx.fixture.investigation);
    if service.snapshot(&id).await.is_ok() {
        eprintln!("Demo investigation {id} already exists; leaving it as it is.");
        return Ok(());
    }
    let (id, _) = fixture::open(kb, service, fx, None, false).await?;
    for i in 0..steps.min(fx.steps.len()) {
        fixture::step(kb, service, fx, &id, i, None, false).await?;
    }
    let loaded = steps.min(fx.steps.len());
    if loaded == fx.steps.len() {
        eprintln!("Demo investigation {id} preloaded with all {loaded} evidence steps");
    } else {
        eprintln!(
            "Demo investigation {id} preloaded with {loaded} of {} evidence steps; the rest are in {}",
            fx.steps.len(),
            fx.dir
                .join("evidence")
                .canonicalize()
                .unwrap_or_else(|_| fx.dir.join("evidence"))
                .display()
        );
    }
    Ok(())
}

fn find_fixture(name: &str) -> Result<Fixture> {
    let path = Path::new(name);
    let dir = if path.join("fixture.toml").exists() {
        path.to_path_buf()
    } else {
        fixture::fixtures_dir().join(name)
    };
    Ok(Fixture::load(&dir)?)
}

async fn replay(kb: Arc<KnowledgeBase>, name: &str, durable: bool) -> Result<()> {
    let fx = find_fixture(name)?;
    println!("{} — {}\n", fx.fixture.id, fx.fixture.title);
    let (id, reports, snapshot) = if durable {
        let dir = std::env::temp_dir().join(format!("kyumei-replay-{}", std::process::id()));
        let service = DurableInvestigations::start(kb.clone(), DurableConfig::new(&dir)).await?;
        let (id, reports) = fixture::replay(&kb, &service, &fx, None, true).await?;
        let snapshot = service.snapshot(&id).await?;
        service.shutdown().await?;
        let _ = std::fs::remove_dir_all(dir);
        (id, reports, snapshot)
    } else {
        let clock = Arc::new(FixtureClock::new(fx.open.at));
        let service = LocalInvestigations::new(kb.clone(), clock.clone());
        let (id, reports) = fixture::replay(&kb, &service, &fx, Some(&clock), true).await?;
        let snapshot = service.snapshot(&id).await?;
        (id, reports, snapshot)
    };
    for r in &reports {
        let states: Vec<String> = r
            .states
            .iter()
            .filter(|(_, (s, _))| *s != kyumei_domain::HypothesisState::Candidate)
            .map(|(h, (s, c))| format!("{h}={}{}", s.label(), if *c { "*" } else { "" }))
            .collect();
        println!(
            "{:<44} next: {}",
            r.label,
            r.next_request.clone().unwrap_or_else(|| "—".to_owned())
        );
        if !states.is_empty() {
            println!("    {}", states.join("  "));
        }
    }
    let assessment = kyumei_app::core::current_assessment(&kb, &snapshot.investigation)?;
    let ws = kyumei_app::workspace::build(&kb, &snapshot, &assessment);
    println!();
    match &ws.assessment.conclusion {
        Some(c) => println!("Diagnosis: {}\n\n{}\n", c.title, c.statement),
        None => println!("No diagnosis reached.\n"),
    }
    for d in &ws.diagrams {
        println!("{}\n\n{}", d.title, d.text);
    }
    println!(
        "Investigation {id} replayed {} ({}).",
        if durable { "on Odori" } else { "in process" },
        snapshot.runtime.kind
    );
    Ok(())
}

fn knowledge(kb: &KnowledgeBase, command: KnowledgeCommand) -> Result<()> {
    match command {
        KnowledgeCommand::Check => {
            for p in kb.packs() {
                println!(
                    "{} v{} ({}), digest {}",
                    p.manifest.title,
                    p.manifest.version,
                    p.manifest.id,
                    &p.digest[..16]
                );
                println!(
                    "  verified for Temporal Server {}",
                    p.manifest.verified_server_versions.join(", ")
                );
                let swept = p.swept_server_versions();
                if !swept.is_empty() {
                    println!(
                        "  settings and metrics swept for {} ({} releases)",
                        kyumei_domain::describe_releases(&swept),
                        swept.len()
                    );
                }
                for (category, claims) in p.claims_by_category() {
                    let unverified = claims
                        .iter()
                        .filter(|c| !c.verification().is_verified())
                        .count();
                    println!(
                        "  {:<28} {:>3}{}",
                        category.label(),
                        claims.len(),
                        if unverified > 0 {
                            format!(" ({unverified} unverified)")
                        } else {
                            String::new()
                        }
                    );
                }
                if !p.references.is_empty() {
                    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
                    for r in &p.references {
                        *kinds.entry(r.kind.label()).or_default() += 1;
                    }
                    let kinds: Vec<String> = kinds
                        .iter()
                        .map(|(k, n)| format!("{n} {}", k.to_lowercase()))
                        .collect();
                    println!(
                        "  {:<28} {:>3} ({})",
                        "References",
                        p.references.len(),
                        kinds.join(", ")
                    );
                }
                for d in &p.diagnostics {
                    println!(
                        "  diagnostic {}: {} stages, {} hypotheses, {} requests, {} rules, {} conclusions",
                        d.id,
                        d.stages.len(),
                        d.hypotheses.len(),
                        d.requests.len(),
                        d.rules.len(),
                        d.conclusions.len()
                    );
                }
            }
            println!("Knowledge is valid.");
            Ok(())
        }
        // Reading upstream is asynchronous; `main` runs it before this.
        KnowledgeCommand::Updates { .. } => Ok(()),
        KnowledgeCommand::Verify { sources } => {
            let roots: BTreeMap<kyumei_domain::SourceId, PathBuf> = sources
                .iter()
                .map(|s| {
                    s.split_once('=')
                        .map(|(id, path)| (id.into(), PathBuf::from(path)))
                        .with_context(|| format!("`{s}` is not ID=PATH"))
                })
                .collect::<Result<_>>()?;
            let mut failed = 0;
            for (id, root) in &roots {
                match verify::checkout_commit(root) {
                    Some(commit) => println!("· source {id} is checked out at {commit}"),
                    None => println!(
                        "· source {id}: {} is not a Git checkout, so its commit is not checked",
                        root.display()
                    ),
                }
            }
            for p in kb.packs() {
                for c in verify::verify_references(p, &roots) {
                    let label = c.outcome.label();
                    if c.outcome.is_failure() {
                        failed += 1;
                        println!("✕ {:<56} {} {}", c.reference.to_string(), c.path, label);
                    } else if c.outcome == verify::ReferenceOutcome::Confirmed {
                        println!("✓ {:<56} {}", c.reference.to_string(), c.path);
                    } else {
                        println!("· {:<56} {} ({})", c.reference.to_string(), c.source, label);
                    }
                }
                for c in verify::verify(p, &roots) {
                    let label = c.outcome.label();
                    if c.outcome.is_failure()
                        || matches!(c.outcome, verify::CheckOutcome::Drifted { .. })
                    {
                        failed += 1;
                        println!(
                            "✕ {:<56} {}:{} {}",
                            c.claim.to_string(),
                            c.path,
                            c.line,
                            label
                        );
                    } else if c.outcome == verify::CheckOutcome::Confirmed {
                        println!("✓ {:<56} {}:{}", c.claim.to_string(), c.path, c.line);
                    } else {
                        println!("· {:<56} {} ({})", c.claim.to_string(), c.source, label);
                    }
                }
            }
            if failed > 0 {
                bail!("{failed} provenance excerpt(s) or reference(s) did not match");
            }
            Ok(())
        }
    }
}
