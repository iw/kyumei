//! `kyumei-collect`: read metric samples where they live and print them as a
//! Kyumei sample block, to paste into the investigation.

use std::{
    io::{Read as _, Write as _},
    path::PathBuf,
    process::ExitCode,
};

use anyhow::{Context as _, Result, bail};
use clap::{Args, Parser, Subcommand};
use jiff::{SignedDuration, Timestamp};
use kyumei_collect::{
    COLLECTOR, Collector, Endpoint, MAX_POINTS_PER_SERIES, fill, parse_query_range, parse_settings,
    qr,
    redact::{self, Redactor},
    window_of,
};
use kyumei_samples::{QueryResult, RequestCode, Samples, Window};

#[derive(Parser)]
#[command(
    name = "kyumei-collect",
    version,
    about = "Read metric samples from Prometheus or Grafana and print them as a Kyumei sample block.",
    long_about = "Read metric samples from Prometheus, or a Prometheus data source behind Grafana, and print them as a Kyumei sample block: a short block of text to copy from the terminal and paste into the evidence request in Kyumei.\n\nRun the command Kyumei shows under a request, or give queries and a window yourself. Credentials come from $KYUMEI_COLLECT_TOKEN, or --user with $KYUMEI_COLLECT_PASSWORD, or the files named by --token-file and --password-file; never from the command line.",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Write saved query_range output (JSON files, one per query) as a block, without querying anything.
    ///
    /// The block's window is the range the files cover, unless --from or --last gives one.
    Encode(EncodeArgs),
    /// Show everything a block holds: every series and every sample.
    Decode {
        /// A file holding the block; standard input when absent.
        file: Option<PathBuf>,
    },
    /// Look up pseudonyms that --redact wrote on this machine.
    Reveal {
        pseudonyms: Vec<String>,
        /// Where --redact keeps its key and pseudonyms.
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
}

#[derive(Args)]
struct WindowArgs {
    /// Start of the window, as RFC 3339 (2026-10-13T11:32:00Z).
    #[arg(long)]
    from: Option<String>,
    /// End of the window, as RFC 3339.
    #[arg(long)]
    to: Option<String>,
    /// The window ending now, such as 30m or 2h.
    #[arg(long, conflicts_with_all = ["from", "to"])]
    last: Option<String>,
    /// Step between samples, such as 15s.
    #[arg(long)]
    step: Option<String>,
}

#[derive(Args)]
struct OutputArgs {
    /// Replace these labels' values with pseudonyms, such as namespace,task_queue.
    #[arg(long, value_delimiter = ',')]
    redact: Vec<String>,
    /// Round values to this many significant digits (1 to 17). Without it, values are exact.
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=17))]
    round: Option<u8>,
    /// Also print the block as a QR code.
    #[arg(long)]
    qr: bool,
    /// Where --redact keeps its key and pseudonyms [default: ~/.config/kyumei-collect].
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[derive(Args)]
struct RunArgs {
    /// A request code copied from a Kyumei evidence request.
    #[arg(long, conflicts_with_all = ["query", "metric"])]
    code: Option<String>,
    /// A PromQL range query to run. Repeatable.
    #[arg(long)]
    query: Vec<String>,
    /// A metric to read as it is. Repeatable.
    #[arg(long)]
    metric: Vec<String>,
    /// A value for a placeholder in the code's queries, as NAME=VALUE. Repeatable.
    #[arg(long = "set", value_name = "NAME=VALUE")]
    set: Vec<String>,
    /// The Prometheus API, such as http://prometheus:9090.
    #[arg(long, env = "KYUMEI_COLLECT_URL", conflicts_with = "grafana")]
    url: Option<String>,
    /// Grafana, to reach a Prometheus data source through Grafana's data source proxy.
    #[arg(long, env = "KYUMEI_COLLECT_GRAFANA", requires = "datasource")]
    grafana: Option<String>,
    /// The Prometheus data source's UID in Grafana.
    #[arg(long, env = "KYUMEI_COLLECT_DATASOURCE")]
    datasource: Option<String>,
    /// A Mimir, Cortex or Thanos tenant, sent as X-Scope-OrgID.
    #[arg(long, env = "KYUMEI_COLLECT_TENANT")]
    tenant: Option<String>,
    /// A file holding a bearer token. Without it, $KYUMEI_COLLECT_TOKEN is used.
    #[arg(long)]
    token_file: Option<PathBuf>,
    /// A user for basic authentication. The password comes from --password-file or $KYUMEI_COLLECT_PASSWORD.
    #[arg(long, env = "KYUMEI_COLLECT_USER")]
    user: Option<String>,
    /// A file holding the basic authentication password.
    #[arg(long)]
    password_file: Option<PathBuf>,
    /// Extra CA certificates to trust, as PEM.
    #[arg(long)]
    ca_file: Option<PathBuf>,
    #[command(flatten)]
    window: WindowArgs,
    #[command(flatten)]
    output: OutputArgs,
    /// Print the queries and window that would be read, and stop.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct EncodeArgs {
    /// query_range output, as JSON, one file per query.
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// The request code the queries came from.
    #[arg(long, conflicts_with = "query")]
    code: Option<String>,
    /// The query each file answers, in the same order. Repeatable.
    #[arg(long)]
    query: Vec<String>,
    /// Values for the code's placeholders, as NAME=VALUE. Repeatable.
    #[arg(long = "set", value_name = "NAME=VALUE")]
    set: Vec<String>,
    #[command(flatten)]
    window: WindowArgs,
    #[command(flatten)]
    output: OutputArgs,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Encode(args)) => encode(args),
        Some(Command::Decode { file }) => decode(file),
        Some(Command::Reveal {
            pseudonyms,
            state_dir,
        }) => reveal(&pseudonyms, state_dir),
        None => run(cli.run),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // The reader stopped early, as `| head` does: not an error.
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe) =>
        {
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("kyumei-collect: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: RunArgs) -> Result<()> {
    let code = args.code.as_deref().map(RequestCode::decode).transpose()?;
    let queries = queries(code.as_ref(), &args.query, &args.metric, &args.set)?;
    let window = window(code.as_ref(), &args.window)?;
    if args.dry_run {
        eprintln!(
            "Would read {} from {} to {} every {}:",
            plural(queries.len(), "query", "queries"),
            utc(window.start_ms),
            utc(window.end_ms),
            step_text(window.step_ms)
        );
        for q in &queries {
            println!("{q}");
        }
        return Ok(());
    }
    let endpoint = match (&args.url, &args.grafana, &args.datasource) {
        (Some(url), _, _) => Endpoint::prometheus(url)?,
        (None, Some(grafana), Some(ds)) => Endpoint::grafana(grafana, ds)?,
        _ => bail!("say where to read: --url for Prometheus, or --grafana with --datasource ($KYUMEI_COLLECT_URL works too)"),
    }
    .with_credentials(
        |k| std::env::var(k).ok(),
        args.token_file.as_deref(),
        args.user.clone(),
        args.password_file.as_deref(),
    )?
    .with_tenant(args.tenant.clone())
    .with_ca_file(args.ca_file.as_deref())?;
    let label = endpoint.label.clone();
    let collector = Collector::new(endpoint)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("start the async runtime")?;
    let (mut samples, warnings) = runtime.block_on(collector.collect(
        &queries,
        window,
        COLLECTOR,
        Timestamp::now().as_millisecond(),
    ))?;
    for w in warnings {
        eprintln!("warning from {label}: {w}");
    }
    if let Some(c) = &code {
        samples.investigation = Some(c.investigation.clone());
        samples.request = Some(c.request.clone());
    }
    finish(samples, &args.output, &format!("Read from {label}"))
}

fn encode(args: EncodeArgs) -> Result<()> {
    let code = args.code.as_deref().map(RequestCode::decode).transpose()?;
    let queries = queries(code.as_ref(), &args.query, &[], &args.set)?;
    if queries.len() != args.files.len() {
        bail!(
            "{} and {}: give one query for each file, in the same order",
            plural(args.files.len(), "file", "files"),
            plural(queries.len(), "query", "queries")
        );
    }
    let mut results = Vec::new();
    for (file, query) in args.files.iter().zip(&queries) {
        let body = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
        let out = parse_query_range(&body).with_context(|| format!("read {}", file.display()))?;
        results.push(QueryResult {
            query: query.clone(),
            series: out.series,
        });
    }
    // The files' own range, unless one is given: a code's window is what was
    // asked for, and saved output may cover more or less of it.
    let window = match (&args.window.from, &args.window.last, window_of(&results)) {
        (Some(_), _, _) | (None, Some(_), _) => window(code.as_ref(), &args.window)?,
        (None, None, Some(covered)) => match &args.window.step {
            Some(step) => Window {
                step_ms: duration_ms(step)?,
                ..covered
            },
            None => covered,
        },
        (None, None, None) if code.is_some() => window(code.as_ref(), &args.window)?,
        (None, None, None) => bail!("the files hold no samples; give --from and --to"),
    };
    let samples = Samples {
        collector: COLLECTOR.to_owned(),
        source: "file".to_owned(),
        collected_at_ms: Timestamp::now().as_millisecond(),
        investigation: code.as_ref().map(|c| c.investigation.clone()),
        request: code.as_ref().map(|c| c.request.clone()),
        window,
        rounded_to: None,
        redacted: Vec::new(),
        queries: results,
    };
    finish(samples, &args.output, "Encoded")
}

/// Round, redact, print the block (and QR code), and say what it holds.
fn finish(mut samples: Samples, output: &OutputArgs, verb: &str) -> Result<()> {
    if let Some(digits) = output.round {
        for q in &mut samples.queries {
            for s in &mut q.series {
                for p in &mut s.points {
                    p.value = kyumei_samples::round_significant(p.value, digits);
                }
            }
        }
        samples.rounded_to = Some(digits);
    }
    if !output.redact.is_empty() {
        let dir = state_dir(output.state_dir.clone())?;
        let mut redactor = Redactor::load_or_create(&dir, output.redact.iter().cloned())
            .with_context(|| format!("prepare pseudonyms in {}", dir.display()))?;
        redactor.apply(&mut samples);
        redactor
            .save(&dir)
            .with_context(|| format!("record pseudonyms in {}", dir.display()))?;
    }
    let block = kyumei_samples::encode(&samples);
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(block.as_bytes())?;
    if output.qr {
        match qr::render(&qr::essential(&block)) {
            Ok(code) => writeln!(stdout, "\n{code}")?,
            Err(e) => eprintln!("No QR code: {e}."),
        }
    }
    let series: usize = samples.queries.iter().map(|q| q.series.len()).sum();
    let points: usize = samples
        .queries
        .iter()
        .flat_map(|q| &q.series)
        .map(|s| s.points.len())
        .sum();
    eprintln!(
        "{verb}: {}, {}, {}. Paste the block into the request in Kyumei.",
        plural(samples.queries.len(), "query", "queries"),
        plural(series, "series", "series"),
        plural(points, "sample", "samples")
    );
    Ok(())
}

fn decode(file: Option<PathBuf>) -> Result<()> {
    let text = match file {
        Some(f) => std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?,
        None => {
            let mut t = String::new();
            std::io::stdin().read_to_string(&mut t)?;
            t
        }
    };
    let blocks = kyumei_samples::decode_all(&text);
    if blocks.is_empty() {
        bail!("{}", kyumei_samples::BlockError::NotFound);
    }
    let mut stdout = std::io::stdout().lock();
    for (i, block) in blocks.into_iter().enumerate() {
        let s = block.map_err(|e| anyhow::anyhow!("block {}: {e}", i + 1))?;
        writeln!(
            stdout,
            "Block {}: collected by {} from {} at {}",
            i + 1,
            s.collector,
            s.source,
            utc(s.collected_at_ms)
        )?;
        if let Some(r) = &s.request {
            writeln!(
                stdout,
                "Request {r} of {}",
                s.investigation.as_deref().unwrap_or("an investigation")
            )?;
        }
        writeln!(
            stdout,
            "Window {} to {}, step {}",
            utc(s.window.start_ms),
            utc(s.window.end_ms),
            step_text(s.window.step_ms)
        )?;
        if let Some(d) = s.rounded_to {
            writeln!(stdout, "Values rounded to {d} significant digits")?;
        }
        if !s.redacted.is_empty() {
            writeln!(
                stdout,
                "Pseudonyms replace the values of {}",
                s.redacted.join(", ")
            )?;
        }
        for (qi, q) in s.queries.iter().enumerate() {
            writeln!(stdout, "\nQuery {}: {}", qi + 1, q.query)?;
            if q.series.is_empty() {
                writeln!(stdout, "  no series")?;
            }
            for series in &q.series {
                let labels: Vec<String> = series
                    .labels
                    .iter()
                    .map(|(k, v)| format!("{k}={v:?}"))
                    .collect();
                writeln!(stdout, "  {{{}}}", labels.join(", "))?;
                for p in &series.points {
                    writeln!(stdout, "    {}  {}", utc_seconds(p.at_ms), exact(p.value))?;
                }
            }
        }
        writeln!(stdout)?;
    }
    Ok(())
}

fn reveal(pseudonyms: &[String], dir: Option<PathBuf>) -> Result<()> {
    let dir = state_dir(dir)?;
    for found in redact::reveal(&dir, pseudonyms)? {
        let p = &found.pseudonym;
        match found.original {
            Some((label, value)) => println!("{p}  {label}={value:?}"),
            None => println!("{p}  not recorded in {}", dir.display()),
        }
    }
    Ok(())
}

fn state_dir(dir: Option<PathBuf>) -> Result<PathBuf> {
    dir.or_else(redact::default_state_dir)
        .context("no home directory; give --state-dir")
}

/// The queries to run: a code's, filled from --set, or those given.
fn queries(
    code: Option<&RequestCode>,
    queries: &[String],
    metrics: &[String],
    set: &[String],
) -> Result<Vec<String>> {
    let values = parse_settings(set).map_err(anyhow::Error::msg)?;
    let raw: Vec<String> = match code {
        Some(c) => c.queries.clone(),
        None => queries.iter().chain(metrics).cloned().collect(),
    };
    if raw.is_empty() {
        bail!("give a request code (--code), or queries (--query or --metric)");
    }
    raw.iter()
        .map(|q| {
            fill(q, &values).map_err(|missing| {
                let flags: Vec<String> = missing.iter().map(|m| format!("--set {m}=…")).collect();
                anyhow::anyhow!(
                    "the queries need values Kyumei does not know: give {}",
                    flags.join(" ")
                )
            })
        })
        .collect()
}

fn window(code: Option<&RequestCode>, args: &WindowArgs) -> Result<Window> {
    let step_ms = match (&args.step, code) {
        (Some(s), _) => duration_ms(s)?,
        (None, Some(c)) => c.step_ms,
        (None, None) => 15_000,
    };
    let (start_ms, end_ms) = match (&args.from, &args.to, &args.last, code) {
        (Some(from), to, _, _) => {
            let end = match to {
                Some(t) => time_ms(t)?,
                None => Timestamp::now().as_millisecond(),
            };
            (time_ms(from)?, end)
        }
        (None, _, Some(last), _) => {
            let end = Timestamp::now().as_millisecond();
            (end - i64::try_from(duration_ms(last)?)?, end)
        }
        (None, _, None, Some(c)) => (c.start_ms, c.end_ms),
        (None, _, None, None) => {
            let end = Timestamp::now().as_millisecond();
            (end - 30 * 60 * 1000, end)
        }
    };
    if end_ms <= start_ms {
        bail!("the window ends before it starts");
    }
    if step_ms == 0 {
        bail!("the step must be longer than zero");
    }
    let points = (end_ms - start_ms) as u64 / step_ms + 1;
    if points > MAX_POINTS_PER_SERIES {
        bail!(
            "{points} samples per series is more than Prometheus returns ({MAX_POINTS_PER_SERIES}); raise --step"
        );
    }
    Ok(Window {
        start_ms,
        end_ms,
        step_ms,
    })
}

fn time_ms(text: &str) -> Result<i64> {
    let t: Timestamp = text.trim().parse().with_context(|| {
        format!("`{text}` is not an RFC 3339 time, such as 2026-10-13T11:32:00Z")
    })?;
    Ok(t.as_millisecond())
}

fn duration_ms(text: &str) -> Result<u64> {
    let d: SignedDuration = text
        .trim()
        .parse()
        .with_context(|| format!("`{text}` is not a duration, such as 15s, 5m or 2h"))?;
    if d.is_negative() {
        bail!("`{text}` is negative");
    }
    Ok(u64::try_from(d.as_millis())?)
}

fn utc(ms: i64) -> String {
    Timestamp::from_millisecond(ms)
        .map(|t| t.strftime("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|_| format!("{ms} ms"))
}

fn utc_seconds(ms: i64) -> String {
    Timestamp::from_millisecond(ms)
        .map(|t| t.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|_| format!("{ms} ms"))
}

/// A value as Prometheus writes it, every digit kept.
fn exact(v: f64) -> String {
    if v.is_nan() {
        "NaN".to_owned()
    } else if v.is_infinite() {
        if v > 0.0 { "+Inf" } else { "-Inf" }.to_owned()
    } else {
        format!("{v}")
    }
}

fn step_text(ms: u64) -> String {
    match ms {
        ms if ms % 3_600_000 == 0 => format!("{}h", ms / 3_600_000),
        ms if ms % 60_000 == 0 => format!("{}m", ms / 60_000),
        ms if ms % 1000 == 0 => format!("{}s", ms / 1000),
        ms => format!("{ms}ms"),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
