//! Synthetic incidents for a Kyumei demo, against a Temporal 1.31 server, on Temporal's Rust SDK.
//!
//! The Worker runs an Invoice Workflow whose one Activity waits as long as the Worker is told
//! to, as a slow downstream call would, and serves the SDK's metrics for Prometheus. The load
//! starts Invoice Workflows at a steady rate. `describe` prints DescribeTaskQueue with
//! statistics, as JSON to paste into Kyumei. RUNBOOK.md has the two incidents step by step.
//!
//! ```text
//! kyumei-demo worker   --task-queue invoices --slots 10 --activity-seconds 0.5 --metrics-port 9101
//! kyumei-demo load     --task-queue invoices --rate 6
//! kyumei-demo describe --task-queue payouts
//! ```

mod invoice;

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, ensure};
use clap::{Args, Parser, Subcommand, ValueEnum};
use temporalio_client::{
    Client, ClientOptions, Connection, ConnectionOptions, Url, WorkflowStartOptions,
};
use temporalio_common::telemetry::{
    HistogramBucketOverrides, Logger, PrometheusExporterOptions, TelemetryOptions,
    metrics::{CoreMeter, TemporalMeter},
    start_prometheus_metric_exporter, telemetry_init,
};
use temporalio_sdk::{
    Runtime, Worker, WorkerOptions,
    runtime::{
        RuntimeOptions,
        worker_tuner::{FixedSizeSlotSupplier, TunerHolder},
    },
};
use tokio::{task::JoinSet, time::Instant};

use invoice::{Invoice, Payments};

/// Finer than the SDK's default buckets, which jump from 10 s to 100 s, so the schedule-to-start
/// quantiles follow the incident closely, up to half an hour of backlog. Milliseconds.
const SCHEDULE_TO_START_BUCKETS_MS: [f64; 25] = [
    100.0,
    250.0,
    500.0,
    1_000.0,
    2_500.0,
    5_000.0,
    10_000.0,
    20_000.0,
    30_000.0,
    45_000.0,
    60_000.0,
    90_000.0,
    120_000.0,
    180_000.0,
    240_000.0,
    300_000.0,
    360_000.0,
    420_000.0,
    480_000.0,
    540_000.0,
    600_000.0,
    750_000.0,
    900_000.0,
    1_200_000.0,
    1_800_000.0,
];

/// The SDK's default for each kind of slot; only Activity slots are the demo's to change.
const DEFAULT_SLOTS: usize = 100;

#[derive(Parser)]
#[command(about = "Synthetic incidents for a Kyumei demo, against a Temporal 1.31 server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a Worker
    Worker(WorkerArgs),
    /// Start Invoice Workflows at a steady rate
    Load(LoadArgs),
    /// Print DescribeTaskQueue with statistics, as JSON
    Describe(DescribeArgs),
}

#[derive(Args)]
struct Queue {
    #[arg(long, default_value = "billing")]
    namespace: String,
    #[arg(long, default_value = "invoices")]
    task_queue: String,
}

#[derive(Args)]
struct WorkerArgs {
    /// Temporal Frontend, host:port
    #[arg(long, default_value = "127.0.0.1:7233")]
    address: String,
    #[command(flatten)]
    queue: Queue,
    /// Activity slots
    #[arg(long, default_value_t = DEFAULT_SLOTS)]
    slots: usize,
    /// Activities per second for the whole Task Queue (max_task_queue_activities_per_second)
    #[arg(long)]
    task_queue_rate: Option<f64>,
    /// How long each Activity takes, in seconds
    #[arg(long, default_value_t = 0.1)]
    activity_seconds: f64,
    #[arg(long, default_value_t = 9101)]
    metrics_port: u16,
    #[arg(long)]
    identity: Option<String>,
}

#[derive(Args)]
struct LoadArgs {
    /// Temporal Frontend, host:port
    #[arg(long, default_value = "127.0.0.1:7233")]
    address: String,
    #[command(flatten)]
    queue: Queue,
    /// Workflows started per second
    #[arg(long, default_value_t = 5.0)]
    rate: f64,
    /// Seconds to run; 0 runs until stopped
    #[arg(long, default_value_t = 0.0)]
    duration: f64,
}

#[derive(Args)]
struct DescribeArgs {
    /// The Frontend's HTTP API, host:port
    #[arg(long, default_value = "127.0.0.1:7243")]
    http_address: String,
    #[command(flatten)]
    queue: Queue,
    #[arg(long, value_enum, default_value_t = QueueType::Activity)]
    r#type: QueueType,
}

#[derive(Clone, Copy, ValueEnum)]
enum QueueType {
    Activity,
    Workflow,
}

impl QueueType {
    fn api_name(self) -> &'static str {
        match self {
            Self::Activity => "TASK_QUEUE_TYPE_ACTIVITY",
            Self::Workflow => "TASK_QUEUE_TYPE_WORKFLOW",
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Worker(args) => run_worker(args).await,
        Command::Load(args) => run_load(args).await,
        Command::Describe(args) => run_describe(args).await,
    }
}

async fn run_worker(args: WorkerArgs) -> Result<()> {
    let overrides = [
        "activity_schedule_to_start_latency",
        "workflow_task_schedule_to_start_latency",
    ]
    .into_iter()
    .map(|name| (name.to_owned(), SCHEDULE_TO_START_BUCKETS_MS.to_vec()))
    .collect();
    let prometheus = start_prometheus_metric_exporter(
        PrometheusExporterOptions::builder()
            .socket_addr(SocketAddr::from(([0, 0, 0, 0], args.metrics_port)))
            .histogram_bucket_overrides(HistogramBucketOverrides { overrides })
            .build(),
    )
    .with_context(|| format!("serve metrics on :{}", args.metrics_port))?;
    let meter: Arc<dyn CoreMeter> = prometheus.meter;
    // The client records its own calls (temporal_request and the like, which the Core SDK
    // dashboard lists Namespaces from) only through a meter it is given, prefixed as the
    // runtime's is. The runtime keeps its telemetry to itself, so this is a second instance
    // over the same Prometheus registry, without logging.
    let client_meter = telemetry_init(TelemetryOptions::builder().metrics(meter.clone()).build())?
        .get_temporal_metric_meter();
    let telemetry = TelemetryOptions::builder()
        .metrics(meter)
        .logging(Logger::Console {
            filter: "warn".to_owned(),
            format: None,
        })
        .build();
    let runtime = Runtime::from_current_tokio(
        RuntimeOptions::builder()
            .telemetry_options(telemetry)
            .build()
            .map_err(|e| anyhow!(e))?,
    )?;

    let identity = args.identity.clone().unwrap_or_else(|| {
        format!(
            "{}-worker-{}@{}",
            args.queue.task_queue,
            args.metrics_port,
            gethostname::gethostname().to_string_lossy()
        )
    });
    let client = connect(&args.address, &args.queue.namespace, client_meter).await?;
    let tuner = TunerHolder::builder()
        .workflow_task_slot_supplier(FixedSizeSlotSupplier::new(DEFAULT_SLOTS))
        .activity_task_slot_supplier(FixedSizeSlotSupplier::new(args.slots))
        .local_activity_task_slot_supplier(FixedSizeSlotSupplier::new(DEFAULT_SLOTS))
        .nexus_task_slot_supplier(FixedSizeSlotSupplier::new(DEFAULT_SLOTS))
        .build();
    let options = WorkerOptions::new(args.queue.task_queue.clone())
        .register_workflow::<Invoice>()?
        .register_activities(Payments {
            seconds: args.activity_seconds,
        })
        .tuner(tuner)
        .maybe_max_task_queue_activities_per_second(args.task_queue_rate)
        .client_identity_override(identity.clone())
        .build();
    let mut worker = Worker::new(&runtime, client, options)?;

    let limit = match args.task_queue_rate {
        Some(rate) => format!("{rate}/s for the whole Task Queue"),
        None => "none".to_owned(),
    };
    println!(
        "{} Worker {identity}: {} Activity slots, Activities take {} s, Task Queue rate limit \
         {limit}; metrics on :{}. Ctrl-C stops it.",
        utc_now(),
        args.slots,
        args.activity_seconds,
        args.metrics_port
    );
    let shutdown = worker.shutdown_handle();
    tokio::spawn(async move {
        stop_requested().await;
        shutdown();
    });
    worker.run().await?;
    println!("{} Worker stopped.", utc_now());
    Ok(())
}

async fn run_load(args: LoadArgs) -> Result<()> {
    ensure!(args.rate > 0.0, "--rate must be above 0");
    let client = connect(&args.address, &args.queue.namespace, None).await?;
    let prefix = utc_clock().replace(':', "");
    let tally = Arc::new(Mutex::new(Tally::default()));
    let mut starts = JoinSet::new();
    println!(
        "{} Starting {} Invoice Workflows a second on {}. Ctrl-C stops it.",
        utc_now(),
        args.rate,
        args.queue.task_queue
    );

    let stop = stop_requested();
    tokio::pin!(stop);
    let begin = Instant::now();
    let mut report = begin + Duration::from_secs(10);
    for i in 0_i64.. {
        if args.duration > 0.0 && begin.elapsed().as_secs_f64() >= args.duration {
            break;
        }
        // Each start is due at its place in the schedule, so a slow start doesn't slow the rest.
        let due = begin + Duration::from_secs_f64(i as f64 / args.rate);
        tokio::select! {
            () = &mut stop => break,
            () = tokio::time::sleep_until(due) => {}
        }
        let (start_client, start_tally) = (client.clone(), Arc::clone(&tally));
        let options = WorkflowStartOptions::new(
            &args.queue.task_queue,
            format!("{}-{prefix}-{i}", args.queue.task_queue),
        )
        .build();
        starts.spawn(async move {
            let outcome = start_client.start_workflow(Invoice::run, i, options).await;
            lock(&start_tally).record(outcome.err().map(|e| e.to_string()));
        });
        while starts.try_join_next().is_some() {}
        if Instant::now() >= report {
            lock(&tally).report();
            report += Duration::from_secs(10);
        }
    }
    while starts.join_next().await.is_some() {}
    let tally = lock(&tally);
    println!(
        "{} Stopped: {} started, {} failed.",
        utc_now(),
        tally.started,
        tally.failed
    );
    Ok(())
}

async fn run_describe(args: DescribeArgs) -> Result<()> {
    let mut url = Url::parse(&format!("http://{}/", args.http_address))
        .with_context(|| format!("--http-address {}", args.http_address))?;
    url.path_segments_mut()
        .map_err(|()| anyhow!("--http-address {} cannot take a path", args.http_address))?
        .extend([
            "api",
            "v1",
            "namespaces",
            &args.queue.namespace,
            "task-queues",
            &args.queue.task_queue,
        ]);
    url.query_pairs_mut()
        .append_pair("taskQueueType", args.r#type.api_name())
        .append_pair("reportStats", "true");
    let response = reqwest::get(url.as_str())
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = response.status();
    let body = response.text().await?;
    ensure!(status.is_success(), "GET {url}: {status}: {body}");
    let json: serde_json::Value =
        serde_json::from_str(&body).context("the response is not JSON")?;
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

async fn connect(address: &str, namespace: &str, meter: Option<TemporalMeter>) -> Result<Client> {
    let target =
        Url::parse(&format!("http://{address}")).with_context(|| format!("--address {address}"))?;
    let options = ConnectionOptions::new(target)
        .maybe_metrics_meter(meter)
        .build();
    let connection = Connection::connect(options)
        .await
        .with_context(|| format!("connect to {address}"))?;
    Ok(Client::new(
        connection,
        ClientOptions::new(namespace).build(),
    )?)
}

/// What the load has started, and the commonest reasons starts failed since the last report.
#[derive(Default)]
struct Tally {
    started: u64,
    failed: u64,
    errors: HashMap<String, u64>,
}

impl Tally {
    fn record(&mut self, error: Option<String>) {
        match error {
            None => self.started += 1,
            Some(message) => {
                self.failed += 1;
                let message: String = message.chars().take(120).collect();
                *self.errors.entry(message).or_default() += 1;
            }
        }
    }

    fn report(&mut self) {
        println!(
            "{} {} started, {} failed",
            utc_now(),
            self.started,
            self.failed
        );
        let mut errors: Vec<_> = self.errors.drain().collect();
        errors.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
        for (message, n) in errors.into_iter().take(3) {
            println!("    {n} × {message}");
        }
    }
}

fn lock(tally: &Mutex<Tally>) -> std::sync::MutexGuard<'_, Tally> {
    tally
        .lock()
        .expect("nothing panics while holding the tally")
}

/// Resolves on Ctrl-C, or on SIGTERM where there is one.
async fn stop_requested() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The time in UTC, to the second, as the incident window takes it.
fn utc_now() -> String {
    format!("{} UTC", utc_clock())
}

fn utc_clock() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}
