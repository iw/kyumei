# Kyumei contributor guidance

These instructions are for coding agents working in this repository, and apply to all of
it. Read [README.md](README.md) first, then [docs/architecture.md](docs/architecture.md)
and [docs/knowledge.md](docs/knowledge.md) before changing a crate's boundaries or a
knowledge pack's format. [docs/decisions.md](docs/decisions.md) says why things are as
they are.

## What Kyumei is

Kyumei helps an operator find out why a running Temporal deployment is misbehaving. It
keeps competing hypotheses open, asks for the one piece of evidence that best tells them
apart, and traces every conclusion to the evidence behind it. Keep it that way:

- The engine has no Temporal-specific code. Metric names, settings, API fields,
  behaviours, thresholds, hypotheses, rules and requests are data, in the knowledge packs
  under `knowledge/temporal/`.
- Reasoning is deterministic. A model may reword an explanation, which is then checked
  against the assessment. It never decides a fact, a rule's outcome or a diagram's
  content.
- Kyumei is not a dashboard and not a chatbot. It shows metrics inside Kyumei, drawn as
  the matching Grafana panel would draw them, rather than sending the operator elsewhere
  to see them.

## Sources of truth

- Temporal's own sources, at the revisions each pack pins in `sources/sources.toml`: the
  server, the API, the SDKs and the CLI. A claim about Temporal is verified by reading
  that source, and cites the path, the line and a verbatim excerpt.
- Documentation is not verification. A claim known only from documentation is marked
  `unverified`, with the reason.
- Kyumei's own thresholds are heuristics. They cite the `kyumei-heuristics` source and
  are never presented as Temporal behaviour.
- [tsurdilo/temporal-server-operations](https://github.com/tsurdilo/temporal-server-operations)
  has no licence. Cite and link its runbooks, playbooks and dashboards; never copy their
  text or dashboards into this repository.

## Architecture boundaries

The Workspace table in the README says what each crate owns.

- `kyumei-domain` is the pure model, with no I/O.
- Diagnosis runs in activities, never in workflow code, and commands reach an
  investigation as workflow updates (`kyumei-odori`). The in-process service
  (`--in-process`) must reach the same assessments.
- `knowledge/temporal/*/generated/` is produced by the `kyumei-extract` generators. Never
  edit it by hand: change the generator and regenerate.
- tempdes is used only through `kyumei-sim`, pinned by Git revision. Don't copy its code.
- A new dependency is a design decision, and a Git dependency needs a pinned revision.

## Knowledge changes

- A new or changed claim needs its verification method and date, and provenance with the
  path, line and excerpt from a pinned source. Its scope lists the releases it holds in.
- Run `kyumei knowledge check`, and `kyumei knowledge verify --source <id>=<path> …` for
  every source you have locally; [docs/knowledge.md](docs/knowledge.md) shows how.
- When a diagnosis changes, add or update a fixture under `fixtures/investigations/`.
  Every fixture replays in process and on Odori.
- After a change, recheck any count the docs quote: claims, citations, sources, fixtures.

## Writing

- Docs and comments describe the current state: no history of earlier versions, no dated
  anecdotes, no "since" or "used to". Rewrite the affected text rather than appending to
  it.
- British English spelling: behaviour, licence.
- Describe a situation concretely. Never use the phrase "locked-down environment".
- Comments explain why a decision is necessary, not what the next line does.

## Rust standards

- Edition 2024. The toolchain is pinned in `rust-toolchain.toml` (1.98.1), and the
  workspace declares `rust-version` 1.98. Don't change either as a side effect of other
  work.
- `unsafe_code` is forbidden across the workspace.
- Avoid `unwrap` and `panic` outside tests. When an invariant makes failure impossible,
  use `expect` with a message that states the invariant.
- Use `--locked` with Cargo. A dependency change is deliberate and reviewed.
- Don't sleep in tests; wait for the condition the test needs to observe.

## Verification

Run from the repository root before asking for a commit:

```console
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run -p kyumei --locked -- knowledge check
```

CI runs the first three on every push to `main` and every pull request. The test suite
includes the durable Odori tests and takes several minutes; a quiet build is not a
stalled one.

## Data and safety

- An operator's Kyumei data directory holds real investigations. Back it up, rehearse on
  a copy, and get explicit approval before anything rewrites it. Demos and screenshots
  use a throwaway `--data-dir` and an empty `--config`.
- Never put credentials, real cluster names, hosts or personal paths in fixtures,
  screenshots, docs or code.
- Ask before downloading anything (a clone, a release binary, a module), saying what,
  from where and how large. To read another repository at a revision, archive it into a
  scratch directory; never change someone's existing checkout.

## Git

- Treat uncommitted changes you didn't make as someone's work. Don't discard, overwrite
  or fold them in without approval.
- Commit and push only when the maintainer asks, and keep each commit to one coherent
  change.
- Never rewrite published history without the maintainer's approval of the exact
  operation.

## Commit attribution (required)

The maintainer is the Git author. Agents are credited with trailers at the end of the
message, after a blank line:

- `Co-Authored-By: <Agent> <email>` for every agent that authored part of the change:
  code, knowledge, documentation or tests.
- `Assisted-by: <Agent> <email>` for every agent that assisted without authoring: review,
  verification or research.

Claude's identity is `Claude <noreply@anthropic.com>`, and its trailers here name the
model, as in `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Pull requests
name the agents that took part.

## Definition of done

A change is done when its claims are verified with provenance, the knowledge and the docs
describe the current state, the fixtures and tests that cover it pass, formatting, Clippy
and the knowledge check are clean, and agent attribution is recorded.
