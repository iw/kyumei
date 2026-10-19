@AGENTS.md

<!-- Claude Code does not read AGENTS.md natively; this import is the bridge.
     Cross-agent rules belong in AGENTS.md. Claude-only notes follow. -->

## Working with Claude Code

- Work in the session you are in. Don't start or propose side sessions; take follow-up
  work in turn, in the same session.
- Say where things stand in plain prose: what changed, what is uncommitted, and the one
  decision needed, with its options in words. Don't rely on task lists or next-step
  buttons to carry it.
- Keep tool calls few. Batch checks into one command, and ask before long runs, such as
  the full test suite or a soak, unless the task plainly needs them.
- Credit yourself in every commit with `Co-Authored-By: Claude <model> <noreply@anthropic.com>`,
  naming the model you are, as in `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
