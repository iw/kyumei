# Contributing to Kyumei

The most useful contributions are fixes, knowledge verified against Temporal's source,
fixtures that pin a diagnosis down, and diagnostics for symptoms Kyumei doesn't yet
recognise.

## Getting started

Install Rust with [rustup](https://rustup.rs); `rust-toolchain.toml` pins the toolchain
and its components. Then:

```console
cargo run -p kyumei -- serve --demo 3 --demo-fixture activity-worker-rate-limit
```

and open <http://127.0.0.1:7373/>. [README.md](README.md) introduces Kyumei,
[docs/architecture.md](docs/architecture.md) its crates, and
[docs/knowledge.md](docs/knowledge.md) the knowledge packs.

## Before you open a pull request

Run from the repository root:

```console
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run -p kyumei --locked -- knowledge check
```

- A knowledge change cites Temporal's source at a pinned revision, by path, line and
  excerpt, and passes `kyumei knowledge verify` against the sources you have;
  [docs/knowledge.md](docs/knowledge.md) explains how.
- A change to a diagnosis comes with a fixture under `fixtures/investigations/`.
- Docs describe the current state, in British spelling.

## Pull requests

- Open them against `main`. CI must pass.
- Keep each commit to one coherent change, and say why in its message.
- If a coding agent helped, credit it with a `Co-Authored-By` or `Assisted-by` trailer,
  as [AGENTS.md](AGENTS.md) sets out. Agents working here follow AGENTS.md, and Claude
  Code also [CLAUDE.md](CLAUDE.md).

## Bugs and vulnerabilities

Report a bug as an issue with the symptom, the evidence you gave Kyumei, and what it
concluded: an investigation's Export Markdown has all three. Report a vulnerability
privately, as [SECURITY.md](SECURITY.md) describes.

## Licence

By contributing, you agree that your contributions are licensed under the
[Apache License, Version 2.0](LICENSE), the licence of this repository.
