# Architecture

Kyumei has one job: turn evidence that an operator brings back into a
diagnosis that can be audited. Diagnosis is deterministic, explanations may
be reworded by a model under guard, and investigations are durable.

## Crates and dependency direction

```
  kyumei-web (Topcoat)        kyumei-odori (Odori, tokeira, Temporal SDK)
     │      \                        /
     │       ▼                      ▼
     │              kyumei-app ─────────────────────────────┐
     │                  │                                   │
     │  kyumei-diagnostics ─ kyumei-evidence ─ kyumei-knowledge ─ kyumei-diagrams
     │                  │          │                        │
     │           kyumei-domain     │                        ▼
     ▼                             ▼                   kyumei-sim ──▶ tempdes
  kyumei-collect ───────────▶ kyumei-samples ◀─────────────┘             ▲
                                                       kyumei-extract ───┘
```

- **kyumei-domain** is the vocabulary: investigations, evidence, observations,
  requests, hypotheses, findings, conclusions, provenance and epistemic status.
  It depends on no framework. Times are UTC throughout; `DisplayZone` writes
  them in a zone the operator chooses and reads times typed in it.
- **kyumei-knowledge** loads versioned knowledge packs, validates them strictly,
  and implements the condition language. Its `updates` module compares what a
  pack pins with what a repository has upstream, given the tags and head the
  caller fetched. See [knowledge.md](knowledge.md).
- **kyumei-evidence** turns a submission into an `Evidence` record. The raw
  submission is kept, and Kyumei's reading of it sits beside it.
- **kyumei-diagrams** is a typed diagram IR with validation, accessible SVG and
  text rendering, and time series charts drawn from data the caller prepares.
  It knows nothing about Temporal.
- **kyumei-dashboards** reads Grafana dashboard JSON: panels, their queries
  (enough PromQL to say which metrics and labels they read), units, decimals,
  thresholds and legends. It loads a directory of dashboards, finds the panels
  that read a metric, and checks panels against metric facts it is given. It
  depends on nothing else of Kyumei's.
- **kyumei-diagnostics** is the engine. It has no clock, no I/O and no model.
- **kyumei-app** holds the commands, the `InvestigationService` port, the
  workspace view model, the Markdown export and fixture replay. Its
  `simulation` module runs the simulations an assessment says are due, with
  `kyumei-sim`, and turns each run into evidence of its own. Its
  `dashboards` module pairs the knowledge with dashboards: it chooses the panel
  for each group of evidence series, converts values into the panel's unit,
  builds the chart, and turns the pack's references into the links the
  workspace shows. Its `panels` module plans a linked panel's reading (the
  range, the variables filled from the investigation, the queries) and draws
  what was read; the interface does the reading. `config` reads the
  configuration file (preferred time zones and environment profiles), and
  `zone` rewrites the UTC times in Kyumei's own text into the chosen zone.
- **kyumei-odori** runs investigations as durable Odori workflows.
- **kyumei-web** is the Topcoat interface, and the only crate that knows
  Topcoat.
- **kyumei-samples** is the sample block format and request codes. It
  depends on no framework and on nothing else of Kyumei's. `kyumei-evidence`
  reads blocks with it, and `kyumei-app` writes request codes.
- **kyumei-collect** is the companion: a read-only client for the Prometheus
  HTTP API and Grafana's data source proxy, and the `kyumei-collect` binary.
  Of Kyumei it knows only `kyumei-samples`, so the companion carries no
  model, agents or knowledge. The web crate uses its client for Collect now.
  See [collector.md](collector.md).
- **kyumei-sim** runs [tempdes](https://github.com/iw/tempdes), a
  discrete-event simulator of Temporal Server clusters, and scrapes its
  simulated metrics every 15 simulated seconds, as Prometheus scrapes a
  cluster. Its `promql` module answers the PromQL Kyumei's requests use over
  those scrapes, and a run's answers are the sample blocks Collect now
  records. Its `dispatch` module builds and runs the scenario for the "big
  enough?" check of a Task Queue's dispatch limit. Of Kyumei it knows only
  `kyumei-samples`, and only `kyumei-app` uses it. `kyumei-app`'s tests work
  simulated incidents, each with one cause, through the Workflow Task
  diagnostic.
- **kyumei** is the binary. It finds the data directory and configuration file
  in the platform's places, and runs `kyumei knowledge updates`, which fetches
  tags and heads with `git ls-remote` and changed files from GitHub.
- **kyumei-extract** stands apart: maintainer tools that generate knowledge
  claims from Temporal and SDK checkouts (`gen-dc-claims`, `gen-metric-claims`,
  `gen-sdk-metric-claims`). It reads dynamic configuration with tempdes's
  `config::registry_gen` and lexes Go with its helpers. It depends on no
  Kyumei crate, and none depends on it.

`crates/kyumei/tests/architecture.rs` enforces these rules with `cargo metadata`:

- the domain has no framework dependencies;
- Topcoat appears only in `kyumei-web`;
- Odori and the Temporal SDK appear only in `kyumei-odori`;
- the engine never depends on the application layers;
- `kyumei-extract` is independent in both directions;
- `kyumei-samples` depends on no framework and no other Kyumei crate, and
  `kyumei-collect` on no Kyumei crate but `kyumei-samples`;
- only `kyumei-web` and the `kyumei` binary use `kyumei-collect`, so only
  they reach a metrics endpoint;
- `kyumei-dashboards` depends on no framework and no other Kyumei crate, and
  no core crate depends on it;
- tempdes appears only in `kyumei-sim`, which depends on no framework and no
  Kyumei crate but `kyumei-samples`, and in `kyumei-extract` for its parser;
- only `kyumei-app` uses `kyumei-sim`.

The spec's section 16 stops after "Topcoat → application". The rest of the
direction is inferred, as recorded in [decisions.md](decisions.md).

## An investigation is an event log

`Investigation` is an aggregate rebuilt by folding `InvestigationEvent`s:

| Event | Meaning |
| --- | --- |
| `Opened` | Symptom, declared context and the description recorded as evidence `E0` |
| `ContextAmended` | Server version, SDKs, scope, window or topology changed |
| `EvidenceRequested` | A rendered request was issued |
| `EvidenceSubmitted` | Raw evidence plus Kyumei's interpretation |
| `InterpretationCorrected` | The operator corrected a binding, value, unit, window or exclusion |
| `RequestDeclined` / `RequestSuperseded` / `RequestAnswered` | Request lifecycle |
| `Assessed` | A full assessment: facts, hypothesis states, findings, next request |
| `ExplanationRecorded` | A validated model explanation for one assessment |
| `Concluded` / `ConclusionWithdrawn` | The diagnosis changed |

The audit trail in the interface is derived from these events and from
nothing else. Replaying the log reproduces the investigation exactly.

Commands (`SubmitEvidence`, `CorrectInterpretation`, `DeclineRequest`,
`AmendContext`, `Reassess`, `RecordSimulation`) go through
`kyumei_app::core::handle`. This pure
function takes the investigation, the knowledge, the command and a timestamp,
and returns the new events. It ingests evidence, re-assesses, settles the
request lifecycle, chooses the next request and changes the conclusion. The
same function runs in tests, in fixture replay, in the in-process service and
inside the durable workflow's activities.

## The diagnostic engine

`kyumei_diagnostics::assess` runs six steps over the current investigation and
the knowledge pack.

1. **Facts** (`facts.rs`). Observations bound to a signal are converted to the
   signal's unit and limited to the incident window. They are then combined by
   the signal's policy, for example summed across Workers per timestamp.
   - Each fact keeps its statistics (min, max, mean, first, last, delta,
     samples, typical) and a pre-window baseline.
   - Observed facts take precedence over operator assertions for the same
     signal. The assertions are kept so contradictions can be shown.
   - A signal informed only by a simulation is a simulated fact.
   - A dated point-in-time capture, such as a DescribeTaskQueue snapshot,
     counts as one sample.
   - A signal whose queries returned no series, and that nothing else
     informs, is recorded as absent: it has no value, but it need not be
     asked for again. So is a boolean API field a response leaves out, as
     protobuf JSON does when it is false.
2. **Rules** (`evaluate.rs`). Every rule's condition is evaluated with
   three-valued logic. A comparison against a missing fact is *unknown*, and a
   rule fires only when its condition is *true*.
   - A firing supports, weakens, eliminates or establishes one hypothesis.
   - A rule that would eliminate or establish but reads only operator
     assertions, or reads a simulated fact at all, is downgraded to weaken or
     support.
   - States are a function of the current facts only, iterated to a fixed
     point, so later evidence can revisit an earlier hypothesis.
   - Contradictory firings leave a hypothesis a contested *candidate*.
3. **Selection** (`select.rs`). The next request is the candidate that:
   1. targets a hypothesis evidence has not yet weakened;
   2. is earliest in the diagnostic path's stage order;
   3. is cheapest;
   4. discriminates the most open hypotheses;
   5. comes first in pack order.

   A request already answered is asked again when its evidence no longer
   informs its signals, for example after a correction unbinds it. Requests
   are rendered for the investigation's SDK family, namespace, Task Queue and
   window. Unfilled placeholders are listed instead of guessed.
4. **Conclusion** (`conclude.rs`). Conclusions are ordered from most to least
   specific, and the first whose condition holds is the diagnosis. It is
   *final* only when no more specific conclusion could still hold given the
   open hypotheses. Until then it is shown as provisional, and the remaining
   uncertainty is listed.
5. **Simulations** (`simulate.rs`). A diagnostic's simulation is due once
   its inputs are known: the calibration inputs as observed facts, and the
   symptom it is compared with as an observed fact or the operator's word. It
   is due once for those inputs over the incident window, not for an
   eliminated hypothesis, and only for a self-hosted server of the release
   the simulator models.
6. **Explanation and diagrams** (`explain.rs`, `diagrams.rs`). These are a
   structured `DiagnosticExplanation` and diagram IR, both built from the same
   assessment.

## Evidence ingestion

`kyumei_evidence::ingest` is the only entry point, for manual submission and
for Collect now. Only the `EvidenceOrigin` differs.

**Detection.** Written observations and typed structured values are known by
their channel. Binary uploads with a recognised media type, such as
screenshots, are attachments, and only their metadata is read. Text is tried
in this order:

1. a Kyumei sample block, known by its first line wherever it appears;
2. JSON, either Prometheus API output (matrix, vector or scalar) or other JSON
   such as DescribeTaskQueue output;
3. dynamic-config YAML;
4. a CSV export (wide or long);
5. `timestamp value` lines;
6. Prometheus exposition text;
7. logs;
8. column-aligned command output, such as a CLI table.

**Sample blocks.** A block that fails its length or checksum check is
reported and nothing is read from it. Otherwise each of its queries is read
as Prometheus output pasted with that query, over the block's window, so
binding and absent-value inference work per query. A query with no series is
noted and nothing is inferred from it. Notes record who collected the block
and when, a block collected for another request, rounding and pseudonyms.
Evidence from a paste, an upload or Collect now is observed telemetry.

**Normalisation.**

- Counters that were already rate-transformed are recognised as per-second.
- Undated API snapshots are assumed captured at submission, and the
  interpretation says so.
- Large series are downsampled, keeping each bucket's minimum and maximum.
- Payloads are capped: 256 KiB inline, 2,000 points per series and 200
  observations.

**Binding.** Binding is conservative. An observation is bound to a diagnostic
signal only when one of these holds:

- its metric name matches a knowledge claim for the investigation's SDK family
  *and* its labels satisfy the signal's label constraints;
- its JSON path names a known API field, or its key is a known
  dynamic-configuration key;
- the operator declared the signal;
- the request it answers expects exactly one signal;
- the answer breaks the signal's metric down by a label and has no series
  for the signal's value of it, so the signal reads as zero (see
  [knowledge.md](knowledge.md#requests)).

When the metric name comes from the submitted query rather than the data,
label names, label values, grouping lists and function names in the query are
skipped: `sum by (operation) (rate(service_requests[1m]))` is about
`service_requests`, though the server also has a metric named `operation`.

Every binding records its basis. Every assumption, such as an assumed unit or
capture time, becomes a caution the operator can correct.

**Corrections** are events. `effective_observations` applies them over the
untouched raw interpretation.

## Durable investigations on Odori

`kyumei-odori` implements `InvestigationService` with one workflow execution
per investigation, on an embedded tokeira engine.

```
web ──update(submit_once)──► InvestigationWorkflow ──activity──► core::handle (deterministic)
                             │
                             ├── activity: simulate (tempdes, when a simulation is due)
                             ├── child workflow: Odori AgentRun ("kyumei-explainer")
                             │     on queue kyumei-agents, when --explainer is set
                             └── activities: prepare_explanation, validate_explanation
```

- **State.** The event log lives in workflow state. Commands arrive as workflow
  **updates**, and each update runs `handle_command` as an activity, so all
  diagnosis happens outside workflow code. A knowledge or engine upgrade
  therefore can never make replay non-deterministic. Updates are serialised
  with a busy flag. Each command travels as a submission with an id, which is
  also the update's id; the workflow remembers its latest replies, so a
  submission sent again is answered, not applied twice. The workflow
  orchestrates and never diagnoses.
- **Simulations.** After each new assessment, and before its explanation,
  the run loop calls the `simulate` activity. It assesses the investigation,
  runs the first simulation that is due, if any, and returns the run. The
  workflow records it as a `RecordSimulation` command, so its evidence is in
  the history like any other. The step is behind the `simulation` patch
  marker, so histories from before it replay without it. The activity does
  nothing unless the runtime was started with simulations on.
- **Explanations.** When a model is configured, the run loop writes an
  explanation after each new assessment:
  1. `prepare_explanation` builds the prompt context from the assessment.
  2. Odori's `AgentRun` child workflow drafts prose.
  3. `validate_explanation` checks the draft.
  4. Only a draft that passes is recorded, as `ExplanationRecorded`.

  A failure is surfaced with its cause chain, and the deterministic text stays.
- **Persistence.** The engine runs with in-memory storage and writes a snapshot
  every `--snapshot-interval` seconds and on shutdown. Kyumei keeps a
  **projection** of each event log under `projections/`, plus an index of
  investigations. A projection is written after every applied command, and
  brought up to date whenever Kyumei reads a longer log from the engine: at
  start, when it shows or lists investigations, and before it stops. Explanations
  are recorded after the command that asked for them.
- **Restart.** On start, every indexed investigation is checked against its
  projection:
  - If the engine knows the workflow and it is up to date, nothing happens. If
    its log is longer than the projection, the projection is brought up to
    date.
  - If the engine lost the workflow, or cannot read its snapshot (an engine
    upgrade, a damaged file), the workflow is started again from the
    projection.
  - If the engine restored a snapshot older than the projection (the process
    stopped between snapshots), the stale workflow is terminated and a new run
    starts from the projection.

  In both recovery cases the workspace says so in its runtime note.
  `tests/durable_golden.rs` covers a restart in the middle of the reference
  investigation, a simulated crash between snapshots, a projection left behind
  the engine, an unreadable snapshot, and every fixture replayed on one engine
  and restored after a restart.
- **Commands the engine leaves unprocessed.** A command is a workflow
  update. If the engine rejects one as unprocessed (the workflow task that
  carried it completed without accepting it, so no handler ran), the service
  sends it again, up to three times.
- **Model sandbox.** Model harnesses run with their working directory set to
  an empty `agent-sandbox` inside the data directory.

`LocalInvestigations` in `kyumei-app` is the same service in process, with an
optional JSON journal. It backs `--in-process`, the tests and `kyumei replay`.
With simulations on, it runs and records the ones due after each command.

## Explanations

The engine always produces a deterministic `DiagnosticExplanation`. It contains:

- a summary;
- one entry per finding, each with its facts, their epistemic status, the
  evidence and observation IDs, and the knowledge claims used;
- the remaining uncertainty;
- the next action;
- the diagrams.

A model receives exactly this structure and returns an `ExplanationDraft`.
The draft can only reword: it replaces the summary, the text of existing
findings and the next action. The facts, evidence references and claims stay
those of the deterministic explanation. It may also propose which diagram to
show first, or a hypothesis focus for the evidence relationship diagram.

`apply_draft` rejects the whole draft if any text:

- is empty;
- introduces a number that is not in the evidence (small counting numbers
  aside);
- contradicts a hypothesis state;
- refers to an unknown finding or hypothesis.

A rejected draft leaves the deterministic prose in place, and the interface
shows why.

## Diagrams

Diagrams are data. Three kinds are built from the assessment:

- **diagnostic path**: the stages and the hypotheses on them;
- **system path**: the layers the diagnostic declares, from the callers to
  where the work completes, with the suspected location;
- **evidence relationship**: observations → the hypotheses they support,
  weaken, eliminate or establish.

`kyumei-diagrams` validates every diagram before rendering it.

- **All diagrams** need a title and a description, a bounded number of
  uniquely identified and labelled nodes, no self edges, and edges and
  annotations that reference known nodes.
- **A diagnostic path** starts from exactly one symptom, uses only `leads_to`
  edges, and is acyclic.
- **A system path** is a single acyclic chain of components.
- **An evidence relationship diagram** only has evidential edges from
  observations to hypotheses.

Each diagram is rendered as SVG with a title, a description and a text
equivalent. Because diagrams and text come from the same assessment, they
cannot disagree.

## Web

`kyumei-web` is a Topcoat application.

- **Pages.** The landing page lists investigations and opens new ones. The
  investigation workspace shows Situation, Current assessment, Next evidence
  and Investigation trail. There is a page per piece of evidence, with raw and
  interpreted views side by side and a correction form. Knowledge pages show
  each claim's provenance.
- **Actions.** Forms post to routes that issue commands. Uploads are
  multipart. **Collect now** posts to
  `/investigations/{id}/requests/{request}/collect`: the route builds the
  request's collection plan in `kyumei-app`, reads it with the configured
  `kyumei_collect::Collector`, and submits the sample block as evidence with
  a `Collector` origin. Without an endpoint, or with values the queries still
  need, it explains on the request instead.
- **Live updates.** A server-sent-event stream tells an open workspace to
  refresh when the investigation changes elsewhere, for example when a model
  explanation lands or another tab submits evidence.
- **Dashboards.** `/dashboards` lists the loaded dashboards, the panels that
  cannot be read, and the cross-checks; uploads post there and are kept under
  the data directory. Evidence pages draw their charts with the loaded
  dashboards, which live in the application state behind a lock, since an
  upload changes them while the server runs.
- **Linked panels.** `/panels/{dashboard}/{panel}` and
  `/investigations/{id}/panels/{dashboard}/{panel}` plan the panel's reading in
  `kyumei-app`, read it with the same `Collector` as Collect now, and draw it.
  **Add as evidence** posts to the investigation's panel route plus
  `/evidence`, which reads the panel again and submits the block like Collect
  now.
- **Time zone.** A cookie keeps each browser's zone; the configuration decides
  which zones it may be. Every page builds its view model in that zone, and
  forms read typed times in it.
- **Testing.** `tests/workspace.rs` drives the whole reference investigation
  through real form posts against the router. `tests/dashboards.rs` uploads a
  dashboard and checks the charts and references. `tests/collect.rs` drives
  Collect now against a stand-in Prometheus, and `tests/panels.rs` a linked
  panel (and, opt-in, every linked server panel against a real Prometheus).
  `tests/preferences.rs` covers the zone, profiles and the default window.
