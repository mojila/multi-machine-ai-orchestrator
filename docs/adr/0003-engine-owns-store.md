# ADR-0003: Engine owns the store; TUI consumes view models over events

- **Status:** Accepted — **amended by [ADR-0009](0009-event-sourced-core.md)**: the append-only event log,
  not the SQLite business tables, is the source of truth (tables are rebuildable projections). The rules
  that the engine owns the store, that the TUI never touches the database, and that the UI consumes view
  models over events all stand and are strengthened by ADR-0009.
- **Date:** 2026-10-21
- **Deciders:** project owner
- **Related:** [PLAN.md §3.1](../PLAN.md#31-runtime-shape), [PLAN.md §3.8](../PLAN.md#38-tui-architecture),
  [ADR-0009](0009-event-sourced-core.md), [docs/events.md](../events.md)

## Context

The TUI must stay responsive while runs stream from providers, tools execute, and the scheduler
mutates the board. SQLite is the single source of truth, and a later phase will replace the in-process
engine with a daemon. Two broad shapes were possible:

1. UI queries the database directly and subscribes to changes (shared store).
2. One owner of the store (engine) exposes state to the UI as view models over channels.

Option 1 is less code initially but couples rendering to SQL, puts database latency on the frame path,
and creates multi-writer contention and "who wins" questions once a scheduler and a headless CLI exist.

## Decision

The engine is the only component with a `Store` handle. The TUI receives:

- `DomainEvent`s on a broadcast channel for incremental updates, each carrying a monotonic revision;
- query replies for explicit requests (correlated by request id), used for snapshots and resync;
- on `broadcast::Lagged`, a full board snapshot rather than event replay.

The UI's state is a pure view model; `view()` performs no I/O and no database access. Writes flow from
the UI as `Action`s/effects to the engine. Short-lived CLI control commands (`task …`,
`approval approve|deny`) may write directly, are serialised by SQLite, and notify the engine; they
never start or mutate runs.

## Consequences

**Positive:** no database or network work on the render path; snapshot-testable UI built against
fixtures before the engine exists; a clean seam for a future daemon (same events over a socket);
single-writer semantics and CAS transitions are easy to reason about.

**Negative:** more plumbing (channels, request correlation, view-model mapping); duplicated shapes
between domain entities and VMs; the UI must handle resync and staleness explicitly.

**Follow-ups:** define a versioned event envelope; add a resync integration test that drops events;
revisit batching if event volume on large boards causes frame churn.
