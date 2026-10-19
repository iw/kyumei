# The reference investigation

`fixtures/investigations/signal-flood-wft-latency/` is the investigation Kyumei
was built around. It runs end to end with no model and no server:

```bash
cargo run -p kyumei -- replay              # in process
cargo run -p kyumei -- replay --durable    # on Odori workflows
```

`crates/kyumei-app/tests/golden.rs` replays it and checks every intermediate
state. `crates/kyumei-web/tests/workspace.rs` drives the same investigation
through the web interface, and `crates/kyumei-odori/tests/durable_golden.rs`
runs it across an engine restart.

## The incident

A batch job starts signalling every open order workflow in namespace
`commerce-prod`. Each Signal to an idle Workflow Execution schedules a Workflow
Task, so Workflow Tasks are added to `order-fulfillment` at about 2,100/s.

Months earlier, the Task Queue was reduced to two partitions, which gives
Matching a dispatch ceiling of 2 × 1,000 tasks/s. The excess waits in
Matching's backlog. Workflow Task schedule-to-start climbs to about 30
seconds, while the twelve Workers have more than a thousand free slots.

The scenario assumes the ceiling is in force, and its evidence shows dispatch
pinned there. In 1.31.0 that is not automatic: a partition applies the default
limit only after its effective rate has changed since it loaded
(`limit.dispatch-applied-on-change`). The diagnosis rests on the observed
pinning, not on the default alone.

The tempting conclusion is "add Workers", and it is wrong. More Workers
would add pollers to a Task Queue whose dispatch is already capped.

## Opening

The operator writes what they see:

> Since about 13:35 UTC, workflow tasks on the order-fulfillment task queue
> (namespace commerce-prod) are waiting about 30 seconds before a worker picks
> them up. Schedule-to-start is normally under 50ms. A batch job started
> signalling every open order workflow at around the same time. Worker CPU and
> memory look normal.

They state Temporal Server 1.31.0, Python SDK 1.25.0, the incident window
13:35–13:50 UTC and the topology. Kyumei then:

- records the description as evidence `E0`;
- recognises the symptom "Workflow Task schedule-to-start latency is
  elevated";
- reads "about 30 seconds" as an **operator assertion**, not an observed fact;
- selects the 1.31 knowledge pack, verified for exactly 1.31.0;
- opens nine hypotheses as **candidates**, six of them primary. Nothing is
  ruled in or out yet.

## Step by step

Each request names what is needed, why, how to collect it for the Python SDK
(Core-based, `temporal_` prefix, milliseconds), the time window, the grouping,
and the hypotheses it discriminates.

### 1. Worker execution capacity

**Request.** Free Workflow Task slots across Workers. This is the one cause
that adding Workers would fix, and one query confirms or rules it out.

```promql
sum by (task_queue) (temporal_worker_task_slots_available{worker_type="WorkflowWorker", namespace="commerce-prod", task_queue="order-fulfillment"})
```

**Evidence E1.** Prometheus API JSON, 31 samples from 13:25 to 13:55. Free
slots never fell below 1,071, against a baseline of 1,185.

**Result.** *Workflow Worker slots exhausted* is **eliminated**. At least 1,071
slots stayed free throughout, while schedule-to-start reached 30 s. The rule
requires at least three samples, so a single reading could not rule out the
whole window.

### 2. Workflow Task polling

**Request.** Pollers and poll results. Workers had free slots, so the question
is whether polls reached Matching fast enough.

**Evidence E2.** A Grafana CSV export with two columns,
`temporal_num_pollers{poller_type="workflow_task"}` and the poll-success rate.

**Result.**

- *No Workflow Task pollers* is **eliminated**: 12 pollers throughout.
- *Workflow Task poller starvation* is **supported**: 1,995 tasks/s across 12
  pollers is more than 50 per poller per second, so the pollers were rarely
  idle.

Kyumei does not conclude starvation from this. It is a supported hypothesis,
not an established one.

### 3. Poll failures and throttling

**Request.** Failed Workflow Task polls by status code. Signals are admitted at
a higher Frontend priority than polls (1 against 4), so a Signal flood near a
rate limit can starve polls.

**Evidence E3.** `long_request_failure` for `PollWorkflowTaskQueue`, all zero.

**Result.** *Workflow Task polls throttled or failing* is **eliminated**. No
poll failed during the window.

### 4. Matching backlog and dispatch

**Request.** DescribeTaskQueue with stats, captured during the incident. These
are **API fields**, not metrics:

- `stats.approximateBacklogAge`
- `stats.approximateBacklogCount`
- `stats.tasksAddRate`
- `stats.tasksDispatchRate`
- `effectiveRateLimit.requestsPerSecond`

The first instruction calls DescribeTaskQueue through grpcurl, the only one
that returns the effective rate limit. The Temporal CLI comes second: it prints
the backlog, the rates and the pollers, but not the limit. A PromQL query for
Matching's backlog age is offered to operators who can run neither.

**Evidence E4.** The JSON from DescribeTaskQueue through grpcurl, captured at
13:48:30. It counts as one sample at that instant.

**Result.** The investigation turns here:

- *Workflow Tasks accumulating in Matching* is **established**. The backlog
  was 31.4 s old (61,840 tasks), comparable to the 30 s schedule-to-start.
- *Workflow Tasks reaching Matching late* is **eliminated**. The backlog
  accounts for at least 80% of the latency.
- *Task Queue dispatch rate limit reached* is **established**. Dispatch ran
  at 1,995/s against an effective limit of 2,000/s while tasks were added at
  2,112/s.
- *Workflow Task poller starvation* is **eliminated** by the same facts,
  revisiting step 2. Dispatch was capped by the rate limit, not by pollers.
  The pollers-busy finding still stands, so the hypothesis is marked
  *contested*, and the uncertainty list says so.

A conclusion is now supportable: *Task Queue dispatch rate limit reached*. It
is shown as **provisional**, because a more specific conclusion can still
hold. The upstream stage has not been examined.

### 5. Partitions and the dispatch limit

**Request.** Partition configuration: the `matching.numTaskqueueReadPartitions`
and `matching.numTaskqueueWritePartitions` overrides for this Task Queue.

**Evidence E5.** A dynamic-config YAML excerpt setting both to 2, constrained
to `commerce-prod` / `order-fulfillment`.

**Result.**

- The 2,000/s limit is 2 read partitions × 1,000/s, Matching's default
  per-partition dispatch rate. The ceiling comes from the partition count, not
  from an API or Worker rate limit.
- *More write partitions than read partitions* is **eliminated**.

### 6. Upstream load

**Request.** SignalWorkflowExecution rate before and during the incident, from
the Frontend's `service_requests` metric. It is unprefixed and has
`service_name="frontend"`. With the OpenTelemetry framework, the metric is
`service_requests_total`.

**Evidence E6.** Samples from 13:15 to 13:55, starting twenty minutes before
the incident.

**Result.** *Signal flood driving Workflow Tasks* is **established**. Signals
rose from a baseline of about 34.4/s to 2,187/s. That is more than five times
the baseline, and at least 80% of the peak Workflow Task add rate.

## The diagnosis

No more specific conclusion can hold, and there is no request left that could
change it, so the diagnosis is final:

> **Task Queue dispatch rate limit reached under a Signal flood**
>
> Workflow Task schedule-to-start rose because Matching dispatched Workflow
> Tasks at the Task Queue's effective rate limit (2,000/s) while a Signal flood
> added them at up to 2,112/s. The excess waited in Matching's backlog, which
> reached 31.4 s old — the schedule-to-start latency. Worker execution slots
> stayed free throughout (at least 1,071) and no Workflow Task polls failed, so
> more Workers would not have reduced the latency. The limit follows from the
> Task Queue's 2 read partitions at Matching's default per-partition dispatch
> limit. SignalWorkflowExecution rose from about 34.4/s to 2,187/s; each Signal
> to an idle execution schedules a Workflow Task.

Each recommendation cites the claims it relies on:

1. Pace the Signal-sending job below the dispatch limit.
2. If the rate is expected, raise read partitions first, then write
   partitions. This comes with a caution about Matching and persistence load,
   and notes that busy pollers may need to grow too.
3. Do not add Workers or Workflow Task slots for this symptom.

The remaining uncertainty is stated:

- Poller starvation is contested.
- Four findings rest partly on the operator's "about 30 seconds", because
  schedule-to-start itself was never measured.

## What the fixture checks

`fixture.toml` states the expected outcome after every step:

- hypothesis states;
- the next request;
- findings;
- the signals the evidence bound;
- whether a conclusion is reached, and whether it is final.

The replay fails on any difference. Two more fixtures cover other branches of
this diagnostic:

- **worker-saturation:** slots at zero; the conclusion is Worker execution
  capacity.
- **poll-throttling:** polls rejected with `ResourceExhausted`; the
  conclusion is poll throttling.

The other diagnostics' reference and branch fixtures are listed in the
[README](../README.md#diagnostics-and-fixtures). Every fixture also replays on
Odori, on one engine, in `crates/kyumei-odori/tests/durable_golden.rs`.

`tests/fixtures.rs` also checks four behaviours:

- **Declining a request** produces an alternative request.
- **A correction** that unbinds evidence re-asks the original request and
  reopens the hypothesis.
- **An investigation with unknown context** (no version, SDK or scope) still
  asks precise questions, with unfilled placeholders shown as such.
- **Every fixture** replays.

## Trying it in the interface

```bash
cargo run -p kyumei -- serve --demo 3
```

This opens <http://127.0.0.1:7373/> with the investigation after step 3. Paste
`e4-describe-task-queue.json` into *Next evidence* to see the turn at step 4,
then continue with E5 and E6.
