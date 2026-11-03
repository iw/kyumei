# Kyumei demo: two incidents, one symptom

Two Task Queues in the `billing` Namespace on Temporal Server 1.31.2 start
their Activities late, and on-call reports both the same way. The causes are
opposite, and so are the fixes. Kyumei separates them using the evidence it
asks for.

| | `invoices` | `payouts` |
|---|---|---|
| What happened | The payment provider slowed from 0.5 s to 3 s a call, so the Worker's 10 Activity slots stay full | A Worker release set `max_task_queue_activities_per_second` to 5, which limits the whole Task Queue |
| First reaction | — | A second Worker is added, and nothing changes |
| Kyumei's diagnosis | Activity Worker execution slots exhausted | Workers' Task Queue rate limit reached |
| Fix | More Activity slots: the SDK's default of 100 | Remove the option from every Worker |

The dev machine runs Temporal, Prometheus and Grafana in Docker Compose, and
the Workers and the load, which use Temporal's Rust SDK. Kyumei and the
browser run there too, or on a laptop that reaches the dev machine's
Prometheus and Grafana through an SSH tunnel.

## What the dev machine needs

- **Docker with Compose.** The first start pulls about 0.9 GB of images from
  Docker Hub: Temporal Server 1.31.2 and its admin tools, the Temporal UI,
  PostgreSQL 17, Prometheus and Grafana. `.env` pins their versions. Another
  1.31.x release also works: change `TEMPORAL_VERSION` in `.env` and
  `server_version` in `kyumei-config.toml` to match. `.env` also says where
  each image comes from. To pull through a mirror or from a private registry,
  set `TEMPORAL_SERVER_IMAGE` and the other `_IMAGE` variables there, or in
  your shell, which Compose prefers to `.env`.
- **Your copy of
  [tsurdilo/temporal-server-operations](https://github.com/tsurdilo/temporal-server-operations)**,
  at `5a71c36`, the commit whose panels Kyumei links. Grafana loads its
  Temporal Server and Core SDK dashboards from it, and so does Kyumei. It has
  no licence, so this repository includes none of it. `.env` expects it
  beside this repository; if it lives elsewhere, set `OPS_DASHBOARDS` there
  and give Kyumei's `--dashboards` the same directory. From the repository
  root:

  ```bash
  git clone https://github.com/tsurdilo/temporal-server-operations ../temporal-server-operations
  ```

  ```bash
  git -C ../temporal-server-operations checkout 5a71c36
  ```
- **Rust**, through [rustup](https://rustup.rs). The repository's
  `rust-toolchain.toml` pins 1.98.1, and rustup fetches it if it's missing.
  The SDK's build also runs `protoc`, the Protocol Buffers compiler
  (`brew install protobuf`, or `apt install protobuf-compiler`).
- **The Temporal CLI** (`brew install temporal`), to describe the payouts
  Task Queue. Kyumei's knowledge of its output is checked against CLI 1.7.0
  to 1.9.1.
- **This repository.** Kyumei builds from its root. `demo/` holds the
  demo's Compose stack (`compose.yaml`, `.env` and `compose/`), Kyumei's
  profiles for it (`kyumei-config.toml`) and the demo's Rust program, a crate
  of its own outside Kyumei's workspace.
- **Free local ports**: 7233 (Temporal), 7243 (Temporal's HTTP API), 8233
  (Temporal UI), 9090 (Prometheus), 3000 (Grafana), 9101–9103 (Worker
  metrics) and 7380 (Kyumei). Compose publishes its ports on 127.0.0.1 only,
  because its Grafana gives anyone who reaches it admin rights, so a laptop
  reaches them through an SSH tunnel (step 6).

Run the commands below from `demo/`, except where a step says otherwise, each
in its own terminal (or tmux window). Ctrl-C stops any of them.

Build the demo's program once. The first build downloads about 51 MB of
crates from crates.io:

```bash
cargo build --release --locked
```

## Before the audience arrives (at least 30 minutes ahead)

Kyumei compares the incident with the 20 minutes before it, so let the
healthy baseline run at least that long before breaking anything.

1. Start Temporal, Prometheus and Grafana. The command returns once the
   `billing` Namespace exists. If the demo has run on this machine before,
   clear the last run first, as **Starting again** describes.

   ```bash
   docker compose up -d --wait
   ```

   The Temporal UI is at <http://127.0.0.1:8233>, Prometheus at
   <http://127.0.0.1:9090>, and Grafana at <http://127.0.0.1:3000>, with no
   login.

2. Start the two Workers, both healthy:

   ```bash
   target/release/kyumei-demo worker --task-queue invoices --slots 10 --activity-seconds 0.5 --metrics-port 9101
   ```

   ```bash
   target/release/kyumei-demo worker --task-queue payouts --slots 200 --activity-seconds 0.1 --metrics-port 9102
   ```

3. Start the load: 6 invoices and 15 payouts a second.

   ```bash
   target/release/kyumei-demo load --task-queue invoices --rate 6
   ```

   ```bash
   target/release/kyumei-demo load --task-queue payouts --rate 15
   ```

   Each load prints how many Workflows it has started every 10 seconds.
   `failed` should stay at 0.

4. On Prometheus's targets page (<http://127.0.0.1:9090/targets>), `temporal`
   and the Workers on 9101 and 9102 should be up. 9103 is down, with
   "connection refused", until the team scales out to a second payouts
   Worker. That is expected.

5. From the repository root, build Kyumei and start it. `.kyumei-demo` is the
   demo's own data directory, which Git ignores, so the audience sees only the
   demo's investigations:

   ```bash
   cargo build --locked -p kyumei
   ```

   ```bash
   target/debug/kyumei serve --addr 127.0.0.1:7380 --data-dir .kyumei-demo --config demo/kyumei-config.toml --prometheus-url http://127.0.0.1:9090 --grafana-url http://127.0.0.1:3000 --dashboards ../temporal-server-operations/observability/dashboards --explainer claude
   ```

   `--grafana-url` opens Kyumei's panels in Grafana, over the incident
   window. `--explainer claude` rewords Kyumei's text through the `claude`
   CLI, which must be logged in where Kyumei runs. Leave it out, or use
   `--explainer none`, to show only the deterministic text.

6. To run Kyumei on a laptop instead, tunnel Prometheus and Grafana from the
   dev machine, then start Kyumei on the laptop as in step 5, but with
   `--prometheus-url http://127.0.0.1:19090`:

   ```bash
   ssh -N -L 19090:127.0.0.1:9090 -L 3000:127.0.0.1:3000 <dev-machine>
   ```

   The laptop needs its own checkout of this repository, with the operations
   repository beside it, as on the dev machine.

## Break both (about 10 minutes before you show Kyumei)

Write down the time. Each Worker prints the UTC time it started.

1. **invoices: the payment provider slows down.** Stop the invoices Worker
   and start it again with 3-second Activities:

   ```bash
   target/release/kyumei-demo worker --task-queue invoices --slots 10 --activity-seconds 3 --metrics-port 9101
   ```

2. **payouts: a release sets a Task Queue-wide rate.** Stop the payouts
   Worker and start it again with the option set:

   ```bash
   target/release/kyumei-demo worker --task-queue payouts --slots 200 --activity-seconds 0.1 --task-queue-rate 5 --metrics-port 9102
   ```

3. **Two minutes later, the team scales out.** Add a second payouts Worker
   from the same release:

   ```bash
   target/release/kyumei-demo worker --task-queue payouts --slots 200 --activity-seconds 0.1 --task-queue-rate 5 --metrics-port 9103
   ```

Give it at least five minutes. The invoices backlog grows by about 2.7 tasks a
second and the payouts backlog by about 10.

## In Kyumei

Open `http://127.0.0.1:7380`. Times are in UTC unless you pick another zone
under **Times in**. The form's labels name the zone it reads.

### invoices

1. Under **Environment**, choose **billing-invoices**. It fills in the server
   version, SDK, Namespace, Task Queue and topology.
2. Describe it, for example: *Since about 11:00 UTC, invoice activities on the
   invoices task queue take over a minute to start.*
3. Set **Incident from** to a minute or two after you broke it, so the window
   holds only the incident (the Worker's slots fill within seconds). Set
   **Incident to** about an hour from now. The form ends the window at the
   moment it opened, but the incident is still going on, and Kyumei sets
   aside evidence from after the window ends. Edit context can move the end
   later, but it cannot clear it.
4. Open the investigation. Kyumei recognises *Activity schedule-to-start
   latency is elevated on invoices*.
5. Under **Next evidence**, press **Collect now** for each request: free
   Activity slots, then schedule-to-start. With free slots at or near 0 and
   schedule-to-start in minutes, Kyumei establishes **Activity Worker
   execution slots exhausted** and says no further evidence is needed. It
   advises adding Worker processes or slots, or shortening Activity
   execution, and checking first that the Activities aren't waiting on a slow
   downstream dependency. The other hypotheses stay open rather than ruled
   out, because Kyumei stops asking once the evidence supports a diagnosis.

### payouts

1. Choose **billing-payouts**, which records two Worker processes. Describe
   it, for example: *Since about 11:00 UTC, payout activities on the payouts
   task queue take over a minute to start. We added a second worker and it
   made no difference. Workers look idle.* Set the same window.
2. Press **Collect now** for each request until Kyumei asks for **Activity
   Task Queue backlog, rates and rate limit**. On the way it rules out
   exhausted slots (at least 200 free), missing pollers and failing polls.
3. That request asks for DescribeTaskQueue. Describe the Task Queue with the
   Temporal CLI on the dev machine, and paste its JSON under **Output, export
   or text**, with the command under **Query or command used**. Paste it
   straight away: Kyumei takes the time you submit it as the time it was
   captured. Don't press Collect now on this request: its query is a
   fallback that reads only the backlog's age.

   ```bash
   temporal task-queue describe --namespace billing --task-queue payouts --task-queue-type activity -o json
   ```

   The CLI doesn't print the effective rate limit, but each poller row
   carries the rate its Worker sends, and Kyumei reads the Workers' 5/s from
   them. Keep `--task-queue-type activity`: without it the CLI lists the
   Workflow Task Queue's rows too, and Kyumei leaves them unread.
4. Kyumei establishes **Workers' Task Queue rate limit reached**: every
   polling Worker sends 5/s, dispatch is pinned there, and the backlog waits
   behind it, so more Workers would not have raised the limit. It says how
   long the backlog takes to clear at that rate, and advises raising or
   removing the option on *every* Worker, because the latest poll's value
   wins. A limit set through the API, or the system default, stays weakened
   rather than ruled out: either would have to equal the Workers' rate.
5. **Collect now** for the last request. It compares the Activities
   Workflows scheduled before and during the incident, rules out a fan-out,
   and completes the diagnosis.

## Fix both and watch them recover

- **invoices:** stop the Worker and start it with the SDK's default of 100
  slots. A backlog of about 2,600 drains in about two minutes.

  ```bash
  target/release/kyumei-demo worker --task-queue invoices --slots 100 --activity-seconds 3 --metrics-port 9101
  ```

- **payouts:** stop both Workers and start them again without
  `--task-queue-rate`. A Worker that still sends the option would set the
  limit again. The effective limit returns to the server's default of
  4,000/s, and the Rust Workers drain more than 150 tasks a second. The
  backlog has grown by about 10 a second since the break, so 10,000 or more
  should clear in a minute or two.

  ```bash
  target/release/kyumei-demo worker --task-queue payouts --slots 200 --activity-seconds 0.1 --metrics-port 9102
  ```

  ```bash
  target/release/kyumei-demo worker --task-queue payouts --slots 200 --activity-seconds 0.1 --metrics-port 9103
  ```

To show the recovery in Kyumei, open the schedule-to-start evidence, choose
**Open the panel with live data**, and watch the latency fall. Describing
the payouts Task Queue again with the Temporal CLI then shows no backlog.

## Starting again

Stop the load and the Workers, then the stack with `docker compose stop`.
Temporal's databases live in memory, so the next `docker compose up -d --wait`
starts with an empty `billing` Namespace. Prometheus keeps its data: for a
clean slate, run `docker compose down -v` to delete it too, then stop Kyumei
and delete `.kyumei-demo` in the repository root. Leave at least 20 minutes
of healthy baseline before breaking things again.

## If something looks wrong

- **A fact shows "one reading", or evidence is "outside the incident
  window".** The window ends before the evidence was captured. Under **Edit
  context**, set a later **Incident to**.
- **invoices is still "supported" after the schedule-to-start request.** The
  window starts before the slots filled, so it holds samples with free slots.
  Under **Edit context**, move **Incident from** a minute or two later.
- **Kyumei doesn't recognise the description.** Under **Context you already
  know**, set **Symptom** to *Activity schedule-to-start latency is elevated*.
- **Kyumei keeps asking for the payouts backlog request.** It needs the
  Temporal CLI's description, pasted, as step 3 under payouts says. Collect
  now there reads only the backlog's age.
- **Collect now returns no series.** Check Prometheus's targets page, and
  that the investigation's Namespace and Task Queue are `billing` and
  `invoices` or `payouts`.
- **The payouts diagnosis stays "supportable" after the last request.** The
  baseline before the incident was shorter than 20 minutes, so Kyumei can't
  rule out a fan-out. The diagnosis itself still stands.
- **`docker compose up` stops on "bind source path does not exist".**
  `OPS_DASHBOARDS` in `.env` doesn't point at your copy's
  `observability/dashboards`.
- **`docker compose up` can't pull an image.** If Docker Hub is out of reach,
  pull through your mirror or registry: set the image's `_IMAGE` variable in
  `.env`, or in your shell.
- **A service seems missing.** `docker compose ps -a` lists them all.
  `temporal-schema` and `temporal-namespace` exit with 0 once they have
  done their work. `docker compose logs temporal` shows the server's log.
- **The build stops in `temporalio-protos`.** It needs `protoc`; install it
  and build again.
