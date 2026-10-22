# ADR-0001: Cargo workspace with layered crates

- **Status:** Accepted
- **Date:** 2026-10-21
- **Deciders:** project owner
- **Related:** [PLAN.md §3.2](../PLAN.md#32-workspace-layout)

## Context

The product is a TUI that embeds an agent engine, a provider client, and a SQLite store. The repository
is also named `multi-machine-ai-orchestrator`, and a future phase will split execution into a daemon
and remote workers. A single-crate binary would compile faster to start but would couple the TUI to
engine internals, make provider conformance tests depend on the whole app, and force a large
restructuring when the daemon split arrives.

Options considered:

1. One binary crate with modules (`ui`, `engine`, `provider`, `store`).
2. A workspace with crates per layer.
3. Separate repositories per component.

## Decision

Use a Cargo workspace (resolver 3) with:

`mta-domain` (pure types and state machines, no I/O) ← `mta-store` (sqlx/SQLite) ← `mta-engine`
(scheduler, run executor, tools) ← `mta-cli` (binary `mta`); `mta-provider` depends only on
`mta-domain`; `mta-tui` depends only on `mta-domain` and consumes view models over channels.

Dependencies may not point upward: the UI never imports engine internals or store rows, and the
domain crate depends on no runtime.

## Consequences

**Positive:** provider and domain tests run without a terminal or database; the engine and provider
are reusable from a future `mta-daemon`/`mta-worker`; compile units are smaller and parallelism better;
the "UI never touches the DB" rule is enforced by the dependency graph rather than review.

**Negative:** more manifests and boilerplate; cross-crate changes need coordination; workspace-wide
version pinning is required to avoid duplicate ratatui/crossterm crates.

**Follow-ups:** enforce the dependency direction in CI (e.g. `cargo deny` bans plus a small
`cargo metadata` check); add crates only when a boundary is real, not pre-emptively.
