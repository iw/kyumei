# Knowledge

Everything Kyumei knows about Temporal lives in versioned knowledge packs under
`knowledge/`. The engine has no Temporal-specific code. Metric names,
configuration keys, API fields, behaviours, thresholds, hypotheses, rules and
evidence requests are all data. The data is loaded strictly, cross-checked,
and embedded in the binary at build time, so a binary always carries the exact
knowledge it was tested with. `--knowledge-dir` loads a different directory at
run time.

There is one pack per minor release of Temporal Server, each a directory of the
same shape:

```
knowledge/temporal/1.32/
├── pack.toml                  manifest: family 1.32, verified for 1.32.0 and 1.32.1
├── sources/sources.toml       where every claim was checked
├── metrics/sdk.toml           how SDK metrics are named and exported; SDK configuration
├── metrics/sdk-annotations.toml    labels, units and notes for generated SDK metrics
├── metrics/server.toml        how server metrics are named and exported
├── metrics/server-annotations.toml labels, purposes and notes for generated server metrics
├── api/task-queue.toml        DescribeTaskQueue fields, the CLI command, the Frontend's gRPC reflection
├── api/workflow.toml          DescribeWorkflowExecution and history event fields, the CLI commands
├── generated/dynamic-config.toml   every dynamic config setting, generated from source
├── generated/server-metrics.toml   every server metric, generated from source
├── generated/sdk-metrics.toml      every Core, Go and Java SDK metric, generated from source
├── dynamic-config/annotations.toml notes and extra provenance for generated settings
├── limits/frontend.toml       the Frontend's rate limits and how rejections are counted
├── limits/matching.toml       Task Queue dispatch limits: system, Worker and API rates
├── limits/persistence.toml    persistence rate limits, priorities, retries and metrics
├── limits/history.toml        history size and event count limits, the continue-as-new suggestion
├── behaviour/workflow-tasks.toml   how Workflow Tasks are scheduled and dispatched
├── behaviour/activity-tasks.toml   how scheduled Activities reach Matching
├── behaviour/workflow-task-failures.toml  how failed Workflow Tasks are retried and reported
├── behaviour/sticky-execution.toml  sticky queues, Worker caches and full-history replays
├── behaviour/history-tasks.toml   how History loads, schedules and retries its own tasks
├── behaviour/mutable-state-cache.toml  History's per-host Workflow cache and what a miss costs
├── behaviour/shard-ownership.toml   how shards move between History hosts, and draining on shutdown
├── behaviour/visibility.toml   how visibility records are written, limited, retried and queried
├── behaviour/java-workflow-threads.toml  Java's Workflow thread pool, what holds its threads, and evictions
├── behaviour/pollers.toml     which pollers DescribeTaskQueue lists
├── behaviour/request-metrics.toml  when request and failure counters first appear
├── references/operations.toml  runbooks, playbook sections and dashboard panels the diagnostics link to
└── diagnostics/
    ├── heuristics.toml        Kyumei's own thresholds (not Temporal behaviour)
    ├── wft-schedule-to-start.toml        Workflow Task schedule-to-start
    ├── activity-schedule-to-start.toml   Activity schedule-to-start
    ├── history-limits.toml               Workflows terminated by, or nearing, the history limits
    ├── history-task-lag.toml             History's own tasks (timers, transfer tasks) running late
    ├── java-workflow-threads.toml        Java Workers out of Workflow threads
    ├── mutable-state-cache.toml          History's Workflow cache missing, raising database reads
    ├── persistence-latency.toml          History's persistence latency
    ├── resource-exhausted.toml           requests rejected with ResourceExhausted
    ├── shard-churn.toml                  requests failing while History's shards change owner
    ├── sticky-cache.toml                 Workers replaying histories instead of using their cache
    ├── visibility.toml                   lists stale, or listing calls failing or slow
    └── workflow-task-failures.toml       Workflow Tasks failing and retried
```

`kyumei knowledge check` summarises each pack:

| Pack | Verified for | Settings and metrics swept for | Claims | Unverified |
|---|---|---|---|---|
| `temporal-1.31` | 1.31.0 to 1.31.4 | the 32 releases from 1.27.0 to 1.31.4 | 1,396 | 3 |
| `temporal-1.32` | 1.32.0 and 1.32.1 | 1.32.0 and 1.32.1 | 1,487 | 3 |

Each has the twelve diagnostics and 98 references to operations material. An
investigation uses the pack of its server's minor release. One of an older
release uses the 1.31 pack, which says what its sweep checked for it (see
[The version sweep](#the-version-sweep)); one of a newer release, or with no
version, uses the newest.

| Diagnostic | Stages | Hypotheses | Requests | Rules | Conclusions |
|---|---|---|---|---|---|
| `wft-schedule-to-start` | 6 | 9 | 8 | 31 | 7 |
| `activity-schedule-to-start` | 6 | 11 | 7 | 29 | 8 |
| `persistence-latency` | 4 | 4 | 4 | 12 | 5 |
| `resource-exhausted` | 2 | 6 | 2 | 12 | 6 |
| `workflow-task-failures` | 3 | 5 | 4 | 13 | 7 |
| `history-limits` | 2 | 5 | 3 | 16 | 7 |
| `sticky-cache` | 4 | 5 | 5 | 13 | 5 |
| `history-task-lag` | 4 | 6 | 4 | 12 | 7 |
| `mutable-state-cache` | 3 | 4 | 3 | 8 | 5 |
| `shard-churn` | 3 | 3 | 3 | 6 | 4 |
| `visibility` | 5 | 8 | 6 | 16 | 8 |
| `java-workflow-threads` | 3 | 4 | 3 | 10 | 3 |

The table counts the 1.31 pack. In the 1.32 pack the two schedule-to-start
diagnostics have three more rules each, which read DescribeTaskQueue's
`rateLimitingActive`.

Most of the claims are generated from source (see
[Generated claims](#generated-claims)): in the 1.31 pack, the 653 dynamic
configuration settings and 552 server metrics of Temporal Server 1.27.0 to
1.31.4; in the 1.32 pack, the 684 settings and 611 metrics of 1.32.0 and 1.32.1;
and in both, the 61 metrics of the Core, Go and Java SDKs.

## Claims

A claim is one statement Kyumei relies on. There are five kinds, each with its
own TOML table, and each claim has a category. Metrics and statements state
their category; the other kinds have a fixed one.

| Table | Category | Example |
| --- | --- | --- |
| `[[metric]]` | `sdk-metric`; for the server, `frontend-metric`, `history-metric`, `matching-metric`, `worker-service-metric` (the server's internal Worker service), or `server-metric` (recorded by several services or by shared code) | `sdk.worker_task_slots_available` |
| `[[api_field]]` | API field | `api.task_queue_stats.approximate_backlog_age` |
| `[[dynamic_config]]` | dynamic configuration | `dc.matching.numTaskqueueReadPartitions` |
| `[[statement]]` | `behaviour`, `limit`, `sdk-configuration` or `cli-command` | `behaviour.frontend-priorities` |
| `[[heuristic]]` | Kyumei diagnostic heuristic | `heuristic.dispatch-at-limit` |

The distinctions are deliberate and enforced:

- **Metrics** record their source name and each *exposition* name separately.
  Kyumei never assumes a `temporal_` prefix:
  - Core-based SDKs add the prefix by default.
  - The Go and Java SDKs hard-code it into the name.
  - Each SDK's exporters then add their own suffixes: `_total` on counters and a
    unit on durations, depending on the exporter and its options (claims
    `sdk.core-prometheus-naming`, `sdk.go-metrics-naming` and
    `sdk.java-metrics-naming`).
  - Server metrics have no prefix unless the server configures one. Their
    exposition names depend on the metrics framework: under OpenTelemetry,
    counters gain `_total` and timers and histograms a unit suffix, unless
    options turn them off (claim `server.metrics-framework`).

  An SDK exposition must name its SDK family, and a server exposition must not.
  A histogram exposition must state its unit, because units differ by exporter:
  Core exports milliseconds by default, Go and Java SDKs seconds, the server's
  default tally framework seconds, and its OpenTelemetry framework
  milliseconds.
- **Labels** are per claim. Server metrics tag the Task Queue as `taskqueue`,
  while SDK metrics use `task_queue`. Core's sticky poller type is
  `sticky_workflow_task`, while Go's and Java's is `workflow_sticky_task`. A
  label that always carries another label's value says so with `same_as`: the
  History task metrics keep `operation` beside `task_type` for compatibility.
  Every server metric also carries `service_name`
  (`server.metrics-service-name`), whether or not its reviewed labels list it.
- **API fields are not metrics.** They are read from DescribeTaskQueue output
  by JSON path, never scraped, and are labelled as API fields everywhere.
- **Dynamic configuration** records the key, its setting constructor, its
  type, its default and the constraints it can be set by.
- **Heuristics** are Kyumei's own thresholds, such as "dispatch at 95% of the
  limit is pinned". They must cite the `authored` source, and the interface
  presents them as Kyumei's judgement, never as Temporal behaviour. A
  heuristic lists the claims it `relies_on`.

### Version scope, verification and provenance

Every claim that is not a heuristic carries the following:

```toml
scope = { server = ["1.31.0", "1.31.1", "1.31.2"] }   # or sdk = [{ family, revision }]
verification = { status = "verified", method = "source-inspection", on = 2026-09-30 }
provenance = [
  { source = ["temporal-server-v1.31.0", "temporal-server-v1.31.1", "temporal-server-v1.31.2"],
    path = "service/matching/ratelimit_manager.go",
    line = 200, excerpt = "return r.effectiveRPS * float64(r.numReadPartitions), r.rateLimitSource" },
]
```

A citation names one source, or every source where the same path, line and
excerpt hold. The verifier checks it in each.

- A **verified** claim needs a method, a date, and provenance with a path and
  line, or a URL. Source inspection must cite a repository source pinned by
  commit or checksum.
- An **unverified** claim must say why. Every claim in both packs is verified
  today, the Temporal CLI commands against CLI source.
- **Server versions** must name a patch release, such as `1.31.0`, never just
  `1.31`, and one the pack verified or swept.

For an investigation, each claim's **applicability** is derived from the
target server version:

| Applicability | Meaning |
| --- | --- |
| verified | verified for exactly this version |
| verified, where it differs | a setting that had another default, type or constraint in this release: the difference is shown |
| same minor, unverified patch | verified for another patch release of the same minor version only |
| unverified | not verified for this version |
| target version unknown | the operator has not said which version is running |
| not server-versioned | an SDK claim or a heuristic |

The applicability is shown next to every claim a finding uses. A claim cited
for an unverified version is never presented as verified.

### Sources

The 1.31 pack's `sources/sources.toml` lists 62 sources:

- Temporal Server `v1.31.0`, at commit `83881961`, and the 31 other releases
  of the version sweep, `v1.27.0` to `v1.31.4`, each pinned by commit;
- `go.temporal.io/api v1.62.8`, the version the server pins;
- tally `v4.1.17` and the OpenTelemetry Prometheus exporter `v0.56.0`, the
  versions the server pins, which decide how server metrics are exported, and
  the tally pseudo-version and exporter `v0.57.0` that some 1.27 releases pin;
- the Rust SDK and Core at commit `acac9e3f`, cross-checked against the Core
  bundled with Python SDK 1.25.0;
- the Python SDK `1.25.0`;
- the Go SDK `v1.44.0`, and its tally (`contrib/tally/v0.2.0`) and
  OpenTelemetry (`contrib/opentelemetry/v0.8.0`) metrics handlers;
- the Java SDK `v1.40.0`;
- Micrometer `v1.9.9` and `v1.13.6`, the versions the Java SDK builds and
  tests against, and the Prometheus Java client `v1.2.1` that 1.13.6 pins;
- the Temporal CLI `v1.7.0` to `v1.9.1`, each pinned by commit, and the Go SDK
  `v1.41.1` that CLI 1.7.0 to 1.8.3 build with;
- the Go standard library `go1.26.2`, `go1.26.4` and `go1.26.8`, the versions
  the 1.31 releases' `go.mod` requires, for how `database/sql` waits for a pooled
  connection;
- Kyumei's authored heuristics;
- [tsurdilo/temporal-server-operations](https://github.com/tsurdilo/temporal-server-operations)
  at commit `5a71c36`, made on 9 October 2026, the playbooks, alert runbooks
  and Grafana dashboards the [references](#references) cite.

The 1.32 pack lists 28: Temporal Server `v1.32.0` and `v1.32.1`,
`go.temporal.io/api v1.63.5`, Go `go1.26.8`, and of the others those the 1.32
releases pin too (tally `v4.1.17`, the exporter `v0.56.0`, the Go SDK `v1.44.0`),
with the SDKs, the Temporal CLI tags, the heuristics and the operations
repository.

### Re-verifying

`kyumei knowledge verify` re-reads every excerpt from local checkouts. An
excerpt found within three lines of the recorded line is reported as drift,
with the line it is now on. One found nowhere near it is a failure.

```bash
kyumei knowledge verify \
  --source temporal-server-v1.31.0=../temporal \
  --source temporal-api-go-v1.62.8=$(go env GOMODCACHE)/go.temporal.io/api@v1.62.8 \
  --source tally-v4.1.17=$(go env GOMODCACHE)/github.com/uber-go/tally/v4@v4.1.17 \
  --source otel-exporter-prometheus-v0.56.0=$(go env GOMODCACHE)/go.opentelemetry.io/otel/exporters/prometheus@v0.56.0 \
  --source sdk-core-acac9e3f=../sdk-rust \
  --source sdk-python-1.25.0=../sdk-python \
  --source sdk-go-v1.44.0=$(go env GOMODCACHE)/go.temporal.io/sdk@v1.44.0 \
  --source sdk-go-contrib-tally-v0.2.0=$(go env GOMODCACHE)/go.temporal.io/sdk/contrib/tally@v0.2.0 \
  --source sdk-go-contrib-opentelemetry-v0.8.0=../sdk-go/contrib/opentelemetry \
  --source sdk-java-v1.40.0=../sdk-java \
  --source micrometer-v1.9.9=../micrometer-1.9.9 \
  --source micrometer-v1.13.6=../micrometer-1.13.6 \
  --source prometheus-client-java-v1.2.1=../client_java \
  --source sdk-go-v1.41.1=$(go env GOMODCACHE)/go.temporal.io/sdk@v1.41.1 \
  --source temporal-cli-v1.7.0=<checkout of temporalio/cli v1.7.0> \
  --source temporal-server-operations-5a71c36=../temporal-server-operations \
  --source go-1.26.2=$(go env GOMODCACHE)/golang.org/toolchain@v0.0.1-go1.26.2.darwin-arm64 \
  --source go-1.26.4=<golang.org/toolchain@v0.0.1-go1.26.4 module> \
  --source tally-v4.1.17-0.20240412215630-22fe011f5ff0=<module> \
  --source otel-exporter-prometheus-v0.57.0=<module> \
  --source temporal-server-v1.27.0=<checkout of v1.27.0>   # and so on for each swept release,
                                                           # and each CLI tag to v1.9.1
```

The 1.32 pack's sources are given the same way: `temporal-server-v1.32.0`,
`temporal-server-v1.32.1`, `temporal-api-go-v1.63.5` and `go-1.26.8`, with the
shared ones.

A checkout of each swept release can be made without touching a working
repository with `git archive v1.29.7 | tar -x -C <dir>`.

Each root must be checked out at the source's revision. For a Git checkout,
`verify` reads the commit it is at and refuses the references of a source pinned
to another. A Go module's paths are
relative to the module root, so a handler from the `sdk-go` repository is
given as its `contrib/…` directory.

The same check runs as a test when `KYUMEI_VERIFY_SOURCES` lists
`id=path` pairs separated by `;`. Against the sources at hand, every one of the
43,519 citations it checks is confirmed at its exact line, the 98 references at
`5a71c36` among them. Kyumei's heuristics cite nothing to check, and the
citations of a source not at hand, such as a Micrometer release, are listed as
skipped.

### Statements read from source

Behaviour and limit statements cannot be generated: someone reads the code and
states what it does, citing each line the statement depends on. They are read
one diagnostic area at a time. `limits/` covers:
- the Frontend's host, namespace, visibility and concurrent long-running request
  limits, the order they apply in, and how their rejections are counted;
- persistence limits per host, namespace and shard, their priorities, and where
  throttled calls are retried and counted;
- a SQL datastore's connection pool, what it reports, and that a full pool
  makes calls wait inside the time `persistence_latency` measures;
- Matching's dispatch limits from the system default, a Worker and the API,
  and what DescribeTaskQueue reports about them;
- the history size, event count and mutable state limits, how History
  terminates a Workflow that passes one, and the continue-as-new suggestion it
  makes before them.

`behaviour/` and `metrics/sdk.toml` cover how Workflow and Activity Tasks are
scheduled, that Workers reserve a slot before each poll, which Worker option
limits a whole Activity Task Queue, and the Frontend's priorities.
`behaviour/workflow-task-failures.toml` covers how History retries a failed
Workflow Task, which attempts reach its history and its metrics, and what the
SDKs report and count for each failed attempt. Its SDK statements are
verified against Core, Go and Java together.
`behaviour/sticky-execution.toml` covers how a Workflow's tasks reach the
Worker that caches it, what happens when that Worker is gone, which tasks
replay a whole history, and how large each SDK's Workflow cache is.
`behaviour/history-tasks.toml` covers how History loads its own tasks,
waits for a host-wide scheduler, retries throttled or failed attempts, and
what each `task_latency_*` metric measures.
`behaviour/mutable-state-cache.toml` covers History's per-host Workflow cache,
what a miss costs, and why a shard that moves starts with a cold cache.
`behaviour/shard-ownership.toml` covers how shards move when History hosts
join or leave, what requests see meanwhile, and the shutdown drain settings.
`behaviour/visibility.toml` covers how History writes visibility records
through a queue of its own, how SQL and Elasticsearch take the writes, the
per-host visibility persistence limits, how failing writes are retried and
moved to the DLQ, the Frontend's rejection of listing calls, and the slow
query log. `behaviour/java-workflow-threads.toml` covers the Java SDK's
Workflow thread pool, what holds its threads (cached Workflows, Signal and
Update handlers, and branches run in threads of their own), and what a Worker
does when none is free: evict a cached Workflow, or fail the task.

Reading can overturn an earlier statement. `limit.task-queue-dispatch` said a
partition's default dispatch limit applied whenever no other rate was set. The
code shows the limit is applied only when the effective rate changes after the
partition loads. The statement now says what is reported and what is enforced,
and `limit.dispatch-applied-on-change` gives the detail.

## Generated claims

Reference knowledge that Temporal declares in a regular form is generated by a
program, not written by hand. Today that is every dynamic configuration setting
the server registers, every metric it defines, and every metric the Core, Go
and Java SDKs define. The generators are in the `kyumei-extract` crate. The
server and Go generators share a small Go lexer; Core and Java have their own.

### Dynamic configuration

`gen-dc-claims` walks a Temporal checkout and
finds every `New<Scope><Type>Setting…(` call. It records each setting's key,
constructor, value type, default, constraint keys and description, and cites
the line holding the key literal as provenance. Its parser is tempdes's
`config::registry_gen`, the one `gen-dc-registry` runs, which the extractor
depends on rather than copies.

```bash
git clone --depth 1 --branch v1.31.0 https://github.com/temporalio/temporal ../temporal
cargo run -p kyumei-extract --bin gen-dc-claims -- ../temporal \
  -o knowledge/temporal/1.31/generated/dynamic-config.toml
```

- **Check mode.** `--check FILE` regenerates in memory and compares. It reuses
  the file's verification date, so it reports only what changed in the source,
  one line per claim field, and exits 1 when anything did.
- **As a test.** `crates/kyumei-extract/tests/up_to_date.rs` makes the same
  check, version sweep included, when `KYUMEI_TEMPORAL_SOURCE` points at a
  v1.31.0 checkout and `KYUMEI_TEMPORAL_SWEEP` lists the other releases'
  checkouts separated by `;`, and skips otherwise.
- **Constraint keys.** `constrained_by` uses the YAML keys an operator writes,
  read from `common/dynamicconfig/setting_gen.go` and `yaml_loader.go`:

  | Scope | `constrained_by` |
  | --- | --- |
  | Global | `global` |
  | Namespace | `namespace` |
  | NamespaceID | `namespaceId` |
  | TaskQueue | `namespace, taskQueueName, taskType` |
  | ShardID | `shardId` |
  | TaskType | `historyTaskType` |
  | Destination | `namespace, destination` |
  | ChasmTaskType | `chasmTaskType` |

- **Defaults** that are not Go constants are kept as Go source, with a note
  saying so. Two settings in 1.31.0 have one.

### Server metrics

`gen-metric-claims` makes two passes over a checkout:

1. **Definitions.** It finds every
   `New{Counter,Gauge,Timer,BytesHistogram,DimensionlessHistogram}Def(` call,
   and records the identifier the definition is assigned to, the name, the kind
   and the `WithDescription` text. It cites the line holding the name literal.
   1.31.0 has 544: 539 in `common/metrics/metric_defs.go`, 4 in CHASM
   libraries and 1 in Elasticsearch visibility. A duplicate name stops the
   generator, because the server would refuse to start with one.
2. **Emitters.** It finds every reference to a definition outside the
   definition itself: a selector through an import of the defining package,
   such as `metrics.ServiceRequests`, or a bare identifier inside it. Comments,
   strings and struct fields are skipped. The packages that refer to a metric
   become its `recorded_in`. 8 definitions in 1.31.0 have no reference, and
   their claims say the server may never record them.

```bash
cargo run -p kyumei-extract --bin gen-metric-claims -- ../temporal \
  -o knowledge/temporal/1.31/generated/server-metrics.toml
```

Each claim, with ID `server.<name>`, holds:

- **Category**, from `recorded_in`. A metric recorded only under
  `service/frontend`, `service/history`, `service/matching` or
  `service/worker` is that service's metric. Anything recorded by more than
  one service, by shared code such as `common/` or `chasm/`, or by nothing, is
  a `server-metric`. In 1.31.0 there are 12 Frontend, 245 History, 53 Matching,
  56 Worker service and 178 server metrics.
- **Instrument and unit**, from the constructor. A counter counts. A byte
  histogram is bytes and a dimensionless one a count. A gauge has no unit,
  because the definition does not state one; an annotation can supply it. A
  timer's unit depends on the exporter, so each of its expositions states it.
- **Purpose**, from the description. 106 definitions have one; the others show
  "Not described in the source." until an annotation supplies a purpose.
- **Expositions**: every name and unit the Prometheus endpoint can export,
  with the configuration that produces each. The default framework comes
  first, because evidence binding prefers the first match and records the
  others as alternatives.

  | Kind | tally (default) | OpenTelemetry (default options) |
  | --- | --- | --- |
  | counter | `name` | `name_total`; `name` with `withoutCounterSuffix` |
  | gauge | `name` | `name` |
  | timer | `name`, seconds | `name_milliseconds`, milliseconds; `name` with `withoutUnitSuffix`; `name_seconds`, seconds, with `recordTimerInSeconds` |
  | byte histogram | `name`, bytes | `name_bytes`; `name` with `withoutUnitSuffix` |
  | dimensionless histogram | `name` | `name_ratio`; `name` with `withoutUnitSuffix` |

  The rules come from Temporal's metrics handlers, tally's Prometheus reporter
  and the OpenTelemetry exporter at the versions the server pins. The claim
  `server.metrics-framework` cites each of them.

`--emitters FILE` also writes a Markdown report listing every place each metric
is recorded, with the tag helpers called in that statement. Tags are attached
where a metric is recorded, often earlier through a handler's `WithTags`, so
the report gives candidates for reviewing a label set, not the label set
itself. Check mode and the `up_to_date` test work as they do for dynamic
configuration.

### SDK metrics

`gen-sdk-metric-claims` reads three checkouts and writes one claim per metric
name, `sdk.<name>`:

```bash
cargo run -p kyumei-extract --bin gen-sdk-metric-claims -- \
  --core ../sdk-rust --go "$(go env GOMODCACHE)/go.temporal.io/sdk@v1.44.0" \
  --java ../sdk-java -o knowledge/temporal/1.31/generated/sdk-metrics.toml
```

Each SDK is read where it defines and records its metrics:

| SDK | Names | Kind |
| --- | --- | --- |
| Core | `meter.counter(MetricParameters { name, description, unit })` and the like, outside `#[cfg(test)]` code; a name can be a `&str` constant | the meter method, or the return type of a local closure wrapping one |
| Go | constants built from `TemporalMetricsPrefix` in `internal/common/metrics` | the handler call that records the name (`Counter`, `Gauge`, `Timer`), following one local assignment |
| Java | `static final String` constants built from `TEMPORAL_METRICS_PREFIX` | the tally `Scope` call (`counter`, `gauge`, `timer`), following assignments in the same file |

A name that is referenced but never reaches a recording call that way gets its
kind from the constant's name (`…_COUNTER`, `…Latency`), and the claim says
so; three Java counters do. A name nothing refers to is not that SDK's: Go
defines `workflow_active_thread_count` and `workflow_task_no_completion` but
never records them.

In 1.31's pack, 40 of the 61 metrics are recorded by all three SDKs. The rest
belong to one or two, such as Java's `workflow_active_thread_count` and Core's
`activity_task_received`. A claim's purpose is Core's description when Core
gives one; Go and Java give none. Each SDK family gets its own
expositions, following its exporters' code at the cited versions:

| SDK | Counter | Duration histogram | Gauge |
| --- | --- | --- | --- |
| Core, Prometheus exporter | `temporal_x`; `_total` with `counters_total_suffix` | `temporal_x` in ms; seconds with `use_seconds_for_durations`; `_milliseconds` or `_seconds` with `unit_suffix` | `temporal_x` |
| Go, tally handler | `temporal_x`; `_total` with `NewPrometheusNamingScope` | `temporal_x` in seconds; `_seconds` with `NewPrometheusNamingScope` | `temporal_x` |
| Go, OpenTelemetry handler | `temporal_x` (up-down counters export as gauges); `_total` with `UseMonotonicCounters` | `temporal_x_seconds`, seconds | `temporal_x` |
| Java, Micrometer Prometheus registry | `temporal_x_total` | `temporal_x_seconds`, seconds, with a `_seconds_max` gauge | `temporal_x` |

The same name in one family can mean two units: a bare Core duration name is
milliseconds by default but seconds with `use_seconds_for_durations`. Binding
then takes the first and says the unit was assumed, unless the operator states
it.

`KYUMEI_SDK_SOURCES="core=PATH;go=PATH;java=PATH"` enables the SDK check in the
`up_to_date` test.

### Annotations

Generated files are never edited. Judgement about a generated claim goes in an
annotation, which the loader merges into the claim with the same ID:

```toml
[[annotation]]
claim = "dc.matching.numTaskqueueReadPartitions"
notes = ["Pollers are spread across read partitions."]
provenance = [
  { source = "temporal-server-v1.31.0", path = "common/dynamicconfig/shared_constants.go",
    line = 9, excerpt = "const GlobalDefaultNumTaskQueuePartitions = 4" },
]
```

Annotations can add notes and provenance to metrics, dynamic configuration,
API fields and statements. The verifier re-checks their provenance like any
other. Heuristics are Kyumei's own and written by hand, so they take no
annotations.

A metric annotation can also supply what a definition leaves out:

- `labels`, the reviewed label set;
- a `purpose`, when the source gives no description;
- a `unit`, when the definition states none.

It can never replace what the source says. For example, a purpose for a metric
the source describes is an error; add a note instead.

```toml
[[annotation]]
claim = "server.approximate_backlog_age_seconds"
purpose = "Age of the oldest task in a Task Queue partition's backlog, in seconds."
unit = "seconds"
labels = [{ name = "namespace" }, { name = "taskqueue" }, { name = "task_type", values = ["Workflow", "Activity", "Nexus"] }]
provenance = [
  { source = "temporal-server-v1.31.0", path = "service/matching/task_queue_partition_manager.go",
    line = 1347, excerpt = "metrics.ApproximateBacklogAgeSeconds.With(versionHandler).Record(age.AsDuration().Seconds())" },
]
```

Every metric a diagnostic signal reads needs a reviewed label set, because
signal label constraints are checked against it. In 1.31's pack, 44 server
metrics have one, in `metrics/server-annotations.toml`, and 15 SDK metrics, in
`metrics/sdk-annotations.toml`.

An annotation that sets labels, a purpose or a unit on a server metric was
reviewed for particular releases, and must say which with
`scope = { server = [...] }`. The claim is then verified only for those
releases, with a note saying the definition is the same in the others.
Notes-only annotations keep their own citations and narrow nothing.

## The version sweep

The 1.31 pack's generators run against every release from 1.27.0 to 1.31.4,
and one file holds the result:

```bash
cargo run -p kyumei-extract --bin gen-dc-claims -- ../temporal-1.31.0 \
  --sweep ../temporal-1.27.0 --sweep ../temporal-1.27.1 … \
  -o knowledge/temporal/1.31/generated/dynamic-config.toml
```

- **Scope.** A generated claim lists every release where it is the same:
  for a setting, the same constructor, type, default and constraint keys; for a
  metric, the same instrument and unit. It is written as 1.31.0 has it, or as
  the newest release that has it, so settings and metrics that were removed
  stay recognisable, scoped to the releases they existed in.
- **Provenance.** One citation per distinct path, line and excerpt, naming
  every release where it holds.
- **Variants.** A setting that differed in other releases has a
  `[[dynamic_config.variant]]` for each other form, with its releases and
  citations. For one of those releases the claim is *verified, where it
  differs*, and the difference is shown. A metric that changed would stop the
  generator until the change is reviewed; none has.
- **Hand-written claims** (statements, API fields and annotations) extend to a
  release only where every server file they cite is byte-identical, and every
  module they cite is pinned at the same version or, at another version, has
  the cited file unchanged. A few were extended by reading the changed code:
  the Frontend limits to 1.31.2, whose only change is an unrelated streaming
  interceptor, and `server.metrics-framework` to every release. Reading 1.27
  found that its OpenTelemetry exporter is not given the metrics prefix, so
  `server.metrics-prefix` holds from 1.28.0. 1.30.7, 1.30.8, 1.31.3 and 1.31.4
  changed the cited files only by adding settings and metrics, or away from
  the cited code, which was read before their claims were extended.
- **Citations follow their code.** A short excerpt, such as a setting's default
  `0,`, occurs on many lines, so a citation at another release is compared,
  with the lines around it, to the claim's citation at 1.31.0. A default or
  description in `constants.go` is cited inside the definition of the setting
  the claim names.
- **Pack fit.** `swept_server_versions` in `pack.toml` lists the releases. For
  one of them outside the pack's family, the investigation uses the pack and
  says that names, types and defaults are checked for that release while
  behaviours were read for 1.31.

### A new minor release

A new minor release gets a pack of its own, so the older pack stays as it was
verified. The 1.32 pack is made this way:

- **Generated claims** come from the newest patch, swept for the others:
  `gen-dc-claims` and `gen-metric-claims` on `v1.32.1`, with `--sweep` for
  `v1.32.0`. SDK metric claims are carried as they are.
- **Hand-written claims** are carried forward, and each citation is found again
  at the new releases next to the code it cited before. Where the lines around
  it changed, the claim is read again at the new release and rewritten where
  it no longer holds. Among other things, the 1.32 pack says that:
  - a write partition is chosen by backlog when managed partition scaling
    (`matching.partitionScaler`) is on;
  - DescribeTaskQueue reports throttled dispatch (`rateLimitingActive`);
  - the poll-wait switch is `system.pollWaitForNamespaceRateLimitToken`, and
    Matching reads it too;
  - with `system.retryUnboundedOnSystemResourceExhausted` on, the History
    client keeps retrying a System-scoped refusal for up to a minute;
  - SQLite honours `maxConns`;
  - slow visibility counts and gets are logged, and Elasticsearch answering a
    read with HTTP 429 is ResourceExhausted, not an error;
  - dlq_writes carries the namespace and task type, and replication's progress
    cache reports as `replication_progress`.
- **Claims found inaccurate at the older release** while reading them again
  are corrected in both packs.
- **Diagnostics** move unchanged when the metrics, settings and fields their
  signals read exist under the same names, which the validator checks, and
  every fixture is replayed on the new pack. Where the new release reports
  more, they use it: the 1.32 schedule-to-start diagnostics read whether the
  rate limiter held dispatch back.

## References

`references/operations.toml` links the diagnostics to operations material
outside the pack: the alert runbooks, playbook sections and Grafana dashboard
panels of [tsurdilo/temporal-server-operations](https://github.com/tsurdilo/temporal-server-operations).
A reference names what it bears on (the diagnostic as a whole, conclusions,
hypotheses or requests) and says why, in Kyumei's words:

```toml
[[reference]]
id = "ops.runbook.sdk-14.workflow-worker"
kind = "runbook"                       # runbook, playbook or panel
title = "Worker Task Slots Exhausted"  # the document's first heading, or the panel's title
source = "temporal-server-operations-5a71c36"
path = "observability/alerts/sdk/runbooks/14-worker-slots-exhausted.md"
section = "WorkflowWorker"             # a heading as written; its anchor is made from it
why = "Alert 14 fires when a Worker has no free slot. This section covers Workflow Task slots: what to check when Kyumei finds them exhausted."
link = [
  { diagnostic = "wft-schedule-to-start", conclusions = ["worker-slot-exhaustion"], hypotheses = ["worker-slot-exhaustion"], requests = ["workflow-worker-slots"] },
]
```

A panel reference names its dashboard and panel instead of a section, and the
SDK an SDK dashboard is for:

```toml
panel = { dashboard = "temporal-sdk-core-v1", dashboard_title = "Temporal Core SDK Dashboard (TypeScript, Python, .NET, Ruby) v1.3.0", id = 34, sdk_family = "core" }
```

The repository has no tags or releases; its parts carry their own versions. A
reference records the version of what it cites where the repository declares
one, with the line that declares it:

```toml
# A panel: its dashboard's version, as the dashboard's title gives it.
version = { version = "v2.25.0", of = "dashboard", path = "observability/dashboards/server/temporal-server.json", line = 9240, excerpt = "\"title\": \"Temporal Server Dashboard v2.25.0\"" }

# A playbook: the version of the dashboard it needs, from the playbooks index.
version = { version = "v2.15.0+", of = "needs-dashboard", dashboard = "Temporal Server", path = "playbooks/README.md", line = 65, excerpt = "[Temporal Server](../observability/dashboards/server/temporal-server-readme.md#16-visibility) v2.15.0+" }
```

All 58 panel references and the 13 playbook sections carry a version; runbooks
declare none. A dashboard's version comes from its title, which is part of the
dashboard itself, rather than from its README's "Current version" line, which
can lag behind: at `5a71c36` the Task Queue Partitions README says v1.1.0 while
the dashboard's title says v1.4.0. Citations show the version with the commit
and its date, from the source's `committed` date: "tsurdilo/temporal-server-operations
at 5a71c36, 9 October 2026".

There are 98: 27 runbooks, 13 playbook sections and 58 panels. Every diagnostic
with a matching runbook links it; history limits and Java's Workflow threads
have none, and link only panels. The source has no licence, so Kyumei cites and
links to it at the pinned commit, and copies none of it: titles and headings
appear only to cite them.

Loading checks that a reference cites a repository pinned by commit, names a
Markdown file for a runbook or playbook and a dashboard's JSON for a panel, and
links only to conclusions, hypotheses and requests its diagnostics define.
`kyumei knowledge verify --source temporal-server-operations-5a71c36=<checkout>`
checks each reference in the checkout: the file exists, the document's first
heading is the recorded title, the section's heading exists and has the anchor
Kyumei links (GitHub's, numbered after an earlier heading with the same text),
and the panel is in its dashboard, by uid, under its title. A version's
excerpt must still be on its line, and a dashboard's version in its title. A
checkout at another commit than the pin fails every reference. On 14 October
2026 all 98 were confirmed at `5a71c36`, with their 71 versions.

The references are written by a script that holds the hand-written links and
reasons, and reads titles, headings, panels and versions from a checkout at the
pin, so they are exact.

Moving the pin is a reviewed change, like a source upgrade: fetch the new
commit, update the source, and fix what `verify` reports.

## Updates upstream

`kyumei knowledge updates` lists what is newer than the sources the packs pin.
For each repository it reads the tags and the default branch's commit with
`git ls-remote`, which works on any Git host, and reports:

- patch releases after a pinned release, within its minor line, and the newest
  release when it is past every pinned line, by tag prefix (`v`, `go`,
  `contrib/tally/v`);
- for a source pinned by commit, how many commits the branch has moved on and,
  on GitHub, the files that changed: for the operations repository, the cited
  files among them, and dashboards whose title now gives another version.

The packs' pins are read together: a release one pack pins is not newer for
another, so 1.32.1, which the 1.32 pack pins, is not listed as newer than the
1.31 pack's 1.31.4. Changed files are those cited by the pack that pins the
commit.

Pre-releases are not counted. GitHub's compare API is used without
authentication, or with `$GITHUB_TOKEN` when it is set. `--json` prints the same
for scripts. Nothing is changed: moving a pin stays a reviewed change.

## Diagnostics

A diagnostic file has a `[diagnostic]` header and these sections.

```toml
[diagnostic]
id = "activity-schedule-to-start"
title = "Elevated Activity schedule-to-start latency"
summary = "…"
precedence = 5

[diagnostic.path]
description = "The path an Activity Task takes: …"
layers = ["clients", "frontend", "history", "matching", "workers"]
labels = { clients = "Workflows scheduling Activities" }
```

- **Precedence.** An operator's description is matched against the
  diagnostics' symptom patterns in order of precedence, lowest first (100 when
  unset), then id. Java Workflow thread exhaustion (4) comes first, since its
  patterns name Java's thread pool, which descriptions of Workflow Task
  failures and cache evictions may also mention. The Activity diagnostic (5),
  history limits (6), Workflow Task failures (7), Workflow cache misses (8)
  and shard churn (9) come before Workflow Task schedule-to-start (10), whose
  patterns also match "schedule-to-start". Visibility (11) comes before History task lag (12),
  which would take a lagging visibility queue for History's queues in
  general, and History's Workflow cache (18) after them. The persistence
  latency diagnostic (20) comes before ResourceExhausted (30), so a description
  of slow persistence that mentions throttling is read as a latency, and a
  listing call rejected at its rate limit is read as a visibility error.
- **Path.** The system diagram follows the layers listed, in order, under the
  diagnostic's own labels. Every hypothesis's layer must be on the path.

| Section | Purpose |
| --- | --- |
| `[[symptom]]` | Regular expressions that recognise the symptom in the operator's description, and the signal it concerns; a diagnostic can have several |
| `[[stage]]` | The serial diagnostic path, ordered, each with the question it answers |
| `[[signal]]` | A diagnostic quantity: unit, how to combine series, and the claims that can supply it, with label constraints |
| `[[hypothesis]]` | A candidate cause on a stage and a system layer, with its implication if true |
| `[[request]]` | Evidence to ask for: why, which signals and hypotheses it informs, cost (1–5), grouping, window padding, and per-SDK collection instructions |
| `[[rule]]` | A condition over facts and hypothesis states, and its effect on one hypothesis |
| `[[conclusion]]` | A diagnosis, from most to least specific, with optional clauses and recommendations that cite claims |
| `[[simulation]]` | A scenario that tests whether a hypothesis's cause is big enough: its model, the facts that calibrate it, and the signal its result informs |

### Conditions

Conditions use three-valued logic. A comparison against a signal with no facts
is *unknown*, `not unknown` is *unknown*, and a rule fires only on *true*.
Missing evidence therefore never counts against a hypothesis.

```toml
when = { signal = "workflow_slots_available", stat = "min", op = "ge", value = 5 }
when = { ratio = { numerator = "tasks_dispatch_rate.max", denominator = "dispatch_rate_limit.last" }, op = "ge", value = 0.95 }
when = { signal = "rate_limit_source", equals = "RATE_LIMIT_SOURCE_WORKER" }   # case-insensitive
when = { hypothesis = "dispatch-rate-ceiling", state = ["established"] }
when = { all = [ ... ] }   # also: any = [ ... ], not = { ... }
```

- **Categorical signals.** A JSON string bound to a signal, such as
  DescribeTaskQueue's `rateLimitSource`, is a categorical fact: only `equals`
  reads it, and a finding that rests on it cites the value and its evidence.
- **Flags.** A boolean API field, such as DescribeTaskQueue's
  `rateLimitingActive`, binds as 1 or 0 to a signal with the unit `flag`, and
  is shown as yes or no. `max` then reads whether any capture reported it.
- **Statistics:** `min`, `max`, `mean`, `first`, `last`, `delta`, `samples`,
  `typical` (an operator's "about 30 seconds") and `baseline` (the mean before
  the incident window).
- **Operators:** `lt`, `le`, `gt`, `ge`, `eq` and `ne`.
- **"Throughout" rules.** A rule that claims something held throughout the
  window must also require `samples ≥ 3`. One reading can show that something
  happened, not that it persisted.

### Effects and states

A rule **supports**, **weakens**, **eliminates** or **establishes** its
hypothesis. A hypothesis's state is derived from every rule that currently
fires on it:

| Firing rules | State |
| --- | --- |
| an establish and an eliminate | candidate, marked contested |
| any eliminate | eliminated |
| any establish | established |
| more supports than weakens | supported |
| more weakens than supports | weakened |
| none, or balanced | candidate |

If every fact an eliminate or establish rule reads is an operator assertion,
the effect is downgraded to weaken or support. An operator's words alone can
never rule a cause in or out. A rule that reads a simulated fact at all is
downgraded the same way.

A hypothesis with an `activation` condition is considered only once it holds.
The Activity diagnostic asks which limit caps dispatch (Worker, API or system)
only after dispatch is established as pinned at a limit. Visibility follows
the write path only while records could be late, and the listing calls only
while they are not known to be. A conclusion that needs an inactive
hypothesis keeps a later one from being final only while its activation could
still hold.

### Requests

```toml
[[request]]
id = "workflow-worker-slots"
stage = "worker-capacity"
why = "…"
signals = ["workflow_slots_available"]
distinguishes = ["worker-slot-exhaustion"]
cost = 1
group_by = ["task_queue"]
window = { pad_before_minutes = 10, pad_after_minutes = 5 }

[[request.collect]]
method = "promql"            # or temporal-api, temporal-cli, dynamic-config
sdk_family = ["core", "go"]  # optional: one family or several; shown only for those SDKs
body = "sum by (task_queue) (temporal_worker_task_slots_available{…, task_queue=\"{task_queue}\"})"
unit = "count"
claims = ["sdk.worker_task_slots_available"]
# optional, for a failure counter: see Witnesses below
absent_reads_zero = { witness = 'sum(rate(temporal_long_request{…}[1m]))', because = "sdk.long-poll-failures-counted-on-failure" }
```

- **Methods.** An instruction is a PromQL query (`promql`), a Temporal API
  call such as grpcurl's (`temporal-api`), a Temporal CLI command
  (`temporal-cli`), a dynamic config lookup (`dynamic-config`), a log search
  (`logs`) or a check (`operator-check`). The instructions are listed most
  specific first: the DescribeTaskQueue requests put grpcurl, which returns
  every signal they expect, before the CLI, which leaves the rate limit out.
- **Placeholders.** Bodies may use `{namespace}`, `{task_queue}`, `{start}`,
  `{end}` and `{step}`. Each value is escaped for where it stands. In PromQL,
  a value inside a string is a string literal, or a regex literal in a regex
  matcher. In a Temporal CLI command or a Temporal API call, a value is quoted
  for bash and zsh: as a bare word, a plain name (letters, digits and
  `-_.,:/@%+`) stands as it is and any other is single-quoted; inside single
  quotes, each `'` in it closes them, follows escaped and reopens them; inside
  double quotes, a value holding `"`, `\`, `$`, `` ` `` or `!` closes them,
  stands single-quoted and reopens them. A Temporal API call's values stand in
  the request's JSON, so they are JSON strings first. Other methods' bodies
  take values as they are. A value the investigation does not know is
  rendered as `<task_queue>` and listed as unfilled, never guessed.
- **SDK variants.** An instruction without `sdk_family` applies to every SDK. One
  with it is shown only when the Workers use one of those SDKs. When the SDK is
  unknown, or the Workers use several, every variant is shown with the SDKs it
  is for. Where the SDKs export a metric under different names (Java's counters
  end in `_total`), each gets its own variant.
- **Breakdowns.** Prometheus returns no series for a label value that never
  occurred. So when an answer breaks a metric down by a label (its series
  carry the label and the query does not filter it), a signal the request
  expects on the same metric, whose value of that label has no series, is
  read as zero at the answer's sample times, with a note saying so. One
  `sum by (resource_exhausted_cause, resource_exhausted_scope)` answer
  therefore rules out every cause it does not show. An answer that filters
  the label, or answers no request, implies nothing about other values.
- **Witnesses.** A failure counter that never counted has no series: the SDKs
  and the server create one, with its label values, at its first increment.
  `absent_reads_zero` names a witness, a counter the same source keeps
  whenever this one could count (every long poll beside the failed ones, every
  request beside the rejected ones), and the statement that says so. The
  witness is rendered as the query is, and Collect now and `kyumei-collect`
  run it with the query. When the query returns no series and the witness has
  samples in the same answer, the signal reads as zero at the witness's sample
  times. Its binding says it was read beside the witness, and a note cites the
  statement. Without the witness's samples, an empty result is still no
  evidence. The validator requires a PromQL instruction, a witness other than
  the query itself, and a statement the pack has.

- **No series.** A query that returns no series, with no witness to vouch for
  it, is recorded as an observation with no value, bound to the signal its
  query informs. Rules read that signal as unknown. The request counts as
  answered, since asking again would find nothing again.
- **Left out.** Protobuf JSON omits a boolean when it is false. A response
  that has a boolean field's parent but not the field, such as a
  DescribeTaskQueue `stats` without `rateLimitingActive`, records the field as
  left out of the response, with no value, the same way. The parent must be an
  object: the Temporal CLI's `stats` is a list of its own rows, which never
  carry the field, so there its absence records nothing.
- **One Task Queue type.** API fields bind by JSON path, list indexes aside,
  so the CLI's rows bind as the API's fields do. A list whose rows name more
  than one `taskQueueType`, as the CLI prints without `--task-queue-type`, is
  left unread with a caution: summed, its rows would give one Task Queue the
  other's backlog.

### Simulations

```toml
[[signal]]
id = "ceiling_simulated_sts"
title = "Schedule-to-start simulated with the dispatch limit alone"
unit = "seconds"
description = "…"          # no source: the simulation informs it

[[simulation]]
id = "dispatch-ceiling-size"
title = "Schedule-to-start with the dispatch limit alone"
hypothesis = "dispatch-rate-ceiling"
model = "task-queue-dispatch"
why = "…"
inputs = { add_rate = "tasks_add_rate.max", dispatch_limit = "dispatch_rate_limit.last", read_partitions = "read_partitions.last", symptom = "wft_schedule_to_start.max" }
produces = "ceiling_simulated_sts"
claims = ["heuristic.simulation-reproduces", "limit.task-queue-dispatch"]
```

- **Models.** `model` names a scenario `kyumei-sim` can build, and so the
  inputs it takes. `task-queue-dispatch` runs one Task Queue whose only
  constraint is its dispatch limit, and needs `add_rate`, `dispatch_limit`,
  `read_partitions` and `symptom`. Each input reads a statistic of a signal.
- **When it runs.** The calibration inputs must be observed facts. `symptom`,
  the latency the result is compared with, may be the operator's word. A
  simulation runs once for its inputs over the incident window, not for an
  eliminated hypothesis, and only for a self-hosted server of the release the
  model is of.
- **What it produces.** The produced signal has no source of its own. Its
  value, from the run, is a simulated fact. Rules compare it with the symptom,
  as `ceiling-big-enough` (supports) and `ceiling-falls-short` (weakens) do with
  `heuristic.simulation-reproduces`'s bounds.

### Rule and conclusion text

Rule statements and conclusion text quote facts with placeholders such as
`{backlog_age.max}`. They are rendered with the fact's unit, for example
`31.4 s`, `2,000/s` or `50.2 MiB`. `{signal.last}` of a categorical signal
quotes its value as observed, such as a termination reason. The validator
rejects a statement that quotes a signal its rule's condition does not read,
so every number in a finding is one the rule actually tested.

Text in square brackets is an optional segment. It shows, without its
brackets, only when every placeholder in it has a value:

```toml
statement = "Writes to the visibility store took up to {write_latency.max}[, against about {write_latency.baseline} before the incident]."
```

A baseline exists only when the evidence has samples from before the incident
window, so a statement quotes `{signal.baseline}` outside an optional segment
only where a baseline is certain:

- in a rule whose condition reads it;
- in a conclusion whose condition requires a hypothesis state that only rules
  reading it can lead to;
- in a clause whose own condition reads it.

Any other statistic of a signal with samples in the window always has a value.

## Validation

A pack that fails any check does not load. Every problem is reported at once.

- **Structure.** Every table is parsed with `deny_unknown_fields`.
- **Duplicates and references.**
  - Duplicate IDs and unknown references are errors. A reference can name a
    source, claim, stage, signal or hypothesis.
- **Metrics.**
  - Metrics need exposition names, with the SDK-family and histogram-unit rules
    above.
  - A metric's labels are unique.
  - A signal's label constraints must use labels the metric declares, with
    values the metric allows. This catches mistakes such as
    `num_pollers{worker_type=…}`, since that metric's label is `poller_type`.
  - A `rate` transform applies only to counters.
  - Label constraints apply only to metrics.
- **Verification.** Claims follow the verification, provenance, version and
  authored-source rules above.
- **Annotations.** An annotation must name an existing claim and add
  something, and cannot target a heuristic. Only a metric annotation can set
  labels, a purpose or a unit, and only where the claim has none.
- **Diagnostic coherence.**
  - Every hypothesis must have a rule that can affect it.
  - Every signal a rule reads must be collected by some request.
  - Every hypothesis a request claims to distinguish must have a rule that
    reads that request's signals.
  - A simulation's inputs must be exactly those its model needs, plus any it
    can use, and read signals no simulation produces.
  - A simulation's produced signal has no other source, one producer, and at
    least one rule that reads it. A rule that reads it may only support or
    weaken.
  - Two rules with identical conditions cannot have opposing effects.
  - A conclusion must rest on hypothesis states.
  - A statement's optional segments are closed, not nested, and each quotes a
    placeholder. A baseline is quoted outside one only where it is certain.
  - Stage orders are unique.
  - Symptom patterns must compile.
  - A request's cost is between 1 and 5.

`crates/kyumei-knowledge/src/tests.rs` loads malformed and contradictory
packs to check that they are rejected:

- undeclared labels and label values;
- opposing rules;
- statements quoting unread signals, quoting a baseline that may be missing,
  or with malformed optional segments;
- verified claims without provenance;
- heuristics without an authored source;
- unknown fields;
- annotations that cannot apply, or that would replace what the source says;
- simulations that could settle a hypothesis, lack an input or take one their
  model doesn't, or produce a signal with another source or no reader.

## Adding knowledge

1. **Put the claim in the directory for its kind.** Give it an ID prefixed by
   that kind, such as `sdk.`, `server.`, `api.`, `limit.`, `behaviour.` or
   `heuristic.`. Dynamic configuration, server metrics and SDK metrics are
   generated: regenerate them rather than adding a `[[dynamic_config]]` or a
   `[[metric]]`, and put what you know about one in an annotation.
2. **Verify it against source at a pinned revision.** Record the path, the
   line and a verbatim excerpt. If you cannot, mark it unverified and say why.
3. **Keep judgement separate.** Put thresholds and interpretations in
   `heuristics.toml`, and explain the rationale.
4. **Run the checks.**
   - Run `kyumei knowledge check`.
   - Run `kyumei knowledge verify` against your checkouts.
   - Run `cargo test -p kyumei-knowledge`.
5. **Prove behaviour changes with a fixture.** If the change affects a
   diagnosis, add or update a fixture under `fixtures/investigations/` so the
   new behaviour is tested end to end.

A new server minor version gets a new pack directory, such as
`knowledge/temporal/1.33/`, made as [A new minor release](#a-new-minor-release)
describes. Its claims must not be copied as verified without re-verification.
