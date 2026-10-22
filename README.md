# multi-machine-ai-orchestrator (`mta`)

[![CI](https://github.com/mojila/multi-machine-ai-orchestrator/actions/workflows/ci.yml/badge.svg)](https://github.com/mojila/multi-machine-ai-orchestrator/actions/workflows/ci.yml)

Terminal-first AI agent orchestrator: a Kanban board where day-to-day employee tasks are assigned to
configurable agents, executed against any OpenAI-compatible provider, and reviewed by a human.

**Status:** M0 (bootstrap) complete — the workspace, the `mta` CLI surface (`--version`, `doctor`,
`config`), the diagnostics baseline, and a minimal TUI exist and are covered by tests and CI. The event
store and the board land in M1. The full implementation plan is in **[docs/PLAN.md](docs/PLAN.md)**.

## Documentation

- [Documentation index](docs/README.md) — conventions, planned docs
- [Implementation plan](docs/PLAN.md) — product spec, architecture, schema, milestones, risks
- [Event-sourcing contract](docs/events.md) — events, commands, effects, deadlines, projections
- [Provider compatibility](docs/providers.md) — tested endpoints, quirks, test procedure
- [Sub-agents & skills](docs/agents.md) — reuse `.opencode/agents/*.md` and `.agents/` agents as
  delegation targets, and attach `.agents/skills/*/SKILL.md` as context (no OpenCode runtime required)
- [Build hygiene](docs/build.md) — bounded `target/`, profiles, size budgets, prune commands
- [Diagnostics](docs/observability.md) — events vs logs, level policy, sinks, redaction, correlation
  spans, rotation, support bundle
- [Architecture decision records](docs/adr/) — [0001 workspace](docs/adr/0001-workspace.md),
  [0002 version pinning](docs/adr/0002-version-pinning.md),
  [0003 engine owns store](docs/adr/0003-engine-owns-store.md),
  [0004 Jev decision provider](docs/adr/0004-jev-decision-provider.md),
  [0005 build size budgets](docs/adr/0005-build-size-budgets.md),
  [0006 OpenCode sub-agents](docs/adr/0006-opencode-subagents.md),
  [0007 context compaction](docs/adr/0007-context-compaction.md),
  [0008 mouse hit map](docs/adr/0008-mouse-hit-map.md),
  [0009 event-sourced core](docs/adr/0009-event-sourced-core.md),
  [0010 agent and skill discovery](docs/adr/0010-agent-and-skill-discovery.md),
  [0011 diagnostics](docs/adr/0011-observability-and-diagnostics.md),
  [0012 task estimation](docs/adr/0012-task-estimation.md),
  [0013 model selection and search](docs/adr/0013-model-selection-and-search.md)

## Intended shape

```
mta                     # launch the Kanban TUI (embeds the engine)
mta run <task-id>       # headless single run
mta run --scheduler     # cron-driven daily tasks, no TUI
mta digest              # standup summary
mta doctor              # config, DB, and provider probe
```

Rust workspace: `domain` · `store` · `provider` · `engine` · `tui` · `cli`. SQLite holds an
**append-only event log that is the source of truth**: every value the app shows is a projection
rebuilt from that log, every change is a validated command, and every side effect (HTTP, files,
subprocesses, timers) subscribes to events and reports back as a command — so there is no polling
loop, no sweep, and "why is this card like this?" is answered by an event. ratatui/crossterm for the
UI, tokio for runtime, a thin OpenAI chat-completions wire layer for chat providers, and a separate
**System One (Jev)** decision wire for typed `noul`/`choice`/`score` questions. When launched in a
project that defines OpenCode agents (`.opencode/agents/*.md`) or the tool-neutral `.agents/`
convention, those definitions become sub-agent targets: the assigned agent can `delegate` part of a
task as a child run with its own transcript and budget, while OpenCode itself is not required.
`.agents/skills/*/SKILL.md` entries are discovered alongside them as a **separate kind** — listed
with provenance and trust, attachable to a run as extra context, and never offered as the agent that
executes a run, because a skill declares no model, permissions, or tools. A skill directory is a
black box except its `SKILL.md`: its `references/` corpus is never scanned or attached. Every run fits
its model's context window on its own:
context is estimated per request and compaction happens automatically before the limit. The TUI is
fully keyboard-driven, with mouse click, wheel, and scrollbar drag mapping onto exactly the same
actions — no mouse-only capability, and capture is restored if the process crashes. The status
line carries live machine health (CPU, RAM, disk), with `S` for full detail — deliberately outside
the event log, because a snapshot of free memory is not reproducible and has no business in an
auditable log. Debug output follows the same split from the other side: `tracing` records go to
rotating files and a TUI overlay, are never replayed, and carry the ids that join a log line back to
the event that caused it — so a failing run is diagnosable without polluting the record that is the
source of truth, and no secret or prompt body reaches a file without an explicit opt-in.
Every card also carries an automatic effort estimate (human hours) and an optional run forecast
(wall-clock and cost for the assigned agent); the header and each column show open-effort subtotals, and
`T` opens a forecast overlay with throughput and a projected finish. Estimates are events too — with
provenance, confidence, and a manual pin — so a rebuild reproduces the board exactly, and the heuristic
works offline while Jev adds a bounded refinement only when the card is ambiguous.
Models are chosen, not memorised: `M` opens a searchable catalogue over configured profiles and each
provider's live `/models` (`ctx>=100k tools @openai`), sets an agent default as a model-only binding or a
one-run override, and refuses to start an unprofiled id until context window and capabilities exist —
so a convenient picker never becomes a run against an invented price. The choice is a config write
(comment-preserving, atomic, reloaded as events), not a hidden database field.
Multi-machine execution (daemon + workers) is deliberately deferred to v0.2;
replicating the event log is the intended protocol, and the crate boundaries are designed for it.

## Build and run

`rust-toolchain.toml` pins the toolchain (1.94) and the lockfile is committed, so:

```sh
cargo build --release
./target/release/mta --version
./target/release/mta doctor --logs   # config path, resolved logging, log files, crash markers
./target/release/mta config init     # write the commented default config (refuses to clobber)
./target/release/mta                 # TUI; `q` or Ctrl-C quits
```

Verification is the CI gate in [AGENTS.md](AGENTS.md) §7: `cargo fmt --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`,
`cargo audit`, `cargo tree -p mta-domain` (purity), and `scripts/size-report.sh --check`.

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

## Next step

Milestone M1 (domain + event store) per [PLAN.md §5](docs/PLAN.md#5-milestones).
