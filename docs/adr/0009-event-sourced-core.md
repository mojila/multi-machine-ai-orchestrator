# ADR-0009: Event-sourced core — the append-only log is the source of truth

- **Status:** Proposed
- **Date:** 2026-10-21
- **Deciders:** project owner
- **Related:** [ADR-0003 engine owns store](0003-engine-owns-store.md),
  [ADR-0006 OpenCode sub-agents](0006-opencode-subagents.md),
  [ADR-0007 context compaction](0007-context-compaction.md),
  [docs/events.md](../events.md), [PLAN.md §3.1](../PLAN.md#31-runtime-shape)

## Context

v0.1 was designed as a CRUD application with an event *notification* layer: SQLite tables held the
state of record, the engine mutated rows directly, and `RunStateChanged` was broadcast so the TUI could
refresh. Everything else was polling or a synchronous call:

- a **250 ms TUI tick** for clocks, spinners, and relative times;
- a **30 s scheduler tick** that rebuilt the ready set from the database on every pass;
- an **engine lock row** with a `notify` file-watch plus a **1 s pending-approval poll** so
  short-lived CLI control commands could be noticed by a running engine;
- an **approval expiry sweep** on the engine tick, on startup, and (lazily) inside `approval list`;
- a **startup reconciliation sweep** that scanned for `running` rows with no live process;
- **direct row reads** for the digest, budget counters, and the agent roster;
- DB **compare-and-swap** and partial unique indexes as the concurrency mechanism.

The owner asked for the application to be event-based for most operations. Taking that seriously raises
the question of *how* event-based: notification-only events on top of mutable tables, or the log itself
as the state of record.

Polling and sweep-based designs have three concrete costs here. They couple correctness to a cadence (a
1 s approval poll is a latency floor and a wasted query; an expiry sweep means state depends on *when
something was last looked at*, which makes `approval list` a mutating command). They make
"why is this task like this?" unanswerable — row mutation leaves no causation chain. And they fit the
deferred multi-machine work badly: replicated tables need a change log anyway, so the change log
eventually becomes the real protocol.

Options considered:

1. **Keep CRUD tables, replace polling with push notifications.** Smallest diff; leaves causation,
   audit, and replication unaddressed.
2. **Event-sourced core.** The append-only log is the source of truth; every table becomes a
   rebuildable projection; all mutation goes through validated commands; side effects are effects that
   append their results as events.
3. **Hybrid: event-source only runs and approvals** (the durable, resumable part), keep tasks and board
   as CRUD.

Option 1 fails the request. Option 3 creates two mental models and two consistency stories, and the
board is precisely where audit matters least but rebuildability is cheapest — splitting is more
confusion for less work avoided.

## Decision

1. **The append-only event log is the source of truth.** No component writes a projection directly.
   A projection can be dropped and rebuilt from the log at any time; that is what makes it a projection.
2. **One envelope, one global order.** Every event carries `seq` (global, monotonic; **gaps are legal**
   after a rolled-back transaction and must not be treated as errors), `event_id`, `stream_id`,
   `stream_type`, `type`, `version` (per-stream, contiguous), `schema_version`, `at`, `actor`,
   `correlation_id`, `causation_id`, and a JSON payload. See [docs/events.md](../events.md) for the
   catalogue.
3. **All mutation is a command.** The path is command → load aggregate (latest snapshot + tail) →
   `decide` → append with the **expected stream version** → publish → project → effects.
   Optimistic concurrency on `version` replaces DB compare-and-swap; "one active root run per task"
   moves from a partial unique index into the decider.
4. **Deciders are pure.** No clock, no randomness, no database reads, no network inside `decide`.
   Timestamps and ids are chosen at command time and stored in the payload, which is what makes replay
   deterministic. Anything that cannot be pure is an **effect**.
5. **External facts are commands, never direct appends.** An HTTP reply, a tool's exit status, or a
   timer firing arrives as a command (`RecordLlmResult`, `RecordToolResult`, `FireDeadline`) and is
   validated before an event is appended. `command_id` is the idempotency key, so an at-least-once
   effect retry cannot double-append.
6. **Effects subscribe to events; they are never called from a decider.** `CallChatModel`,
   `CallDecisionModel`, `ExecuteTool`, `RunSubagent`, `SummariseContext`, `WriteArtifact`,
   `ComputeDigest`. A pending effect with no result event is re-issued after a crash.
7. **Polling and sweeps are removed, not reduced.** Deadlines are *derived state*: the event that needs
   a future action arms a deadline, a single timer wheel arms the earliest one, and firing emits
   `FireDeadline` → `DeadlineReached`. Approval expiry, retry backoff, run timeouts, and cron schedules
   all work this way. Startup reconciliation becomes replay plus `CrashRecovered`. The scheduler
   becomes event-reactive: it recomputes the ready set when a relevant event arrives, not on a tick.
   The only remaining cadence is the TUI's repaint interval, which is **presentation-only** and never
   touches domain state.
8. **Transcript content has exactly one home.** `MessageAppended` carries message content and role;
   `LlmCallCompleted` carries metrics (usage, finish reason, cost, latency). Streaming token deltas are
   **not** logged — they travel on an ephemeral in-process channel, are droppable, and are never needed
   to reconstruct state. **Host metrics (CPU, RAM/swap, disk, top processes) obey the same rule for a
   stronger reason:** an instantaneous reading is not reproducible, so logging it would make replay
   non-deterministic and a rebuild on another machine differ from the original. Samples stay on an
   ephemeral `watch` channel; the only logged form is an opt-in, debounced `HostPressureObserved`
   *threshold crossing*, which is a durable fact ("the box was under pressure while these runs were
   active") rather than a measurement.
9. **Short-lived CLI control commands stop writing rows and stop needing a poll.** A command is appended
   to the durable command log; if no engine holds the write lock the CLI processes it itself, otherwise
   it enqueues it and wakes the engine. `notify` + 1 s approval poll is deleted.
10. **Retention is explicit and honest.** `[events] retention_days` (default 90, `0` = forever) prunes
    events below the minimum projection offset *and* below each stream's last snapshot, keeping the
    `GcPerformed` record. Pruning means the log is authoritative above the watermark and snapshots are
    authoritative below it; a run's audit export is written before its stream is pruned.
11. **Schema evolution is add-only with upcasters.** Payloads gain fields; meaning never changes in
    place. Replay upcasts on read, so old logs stay foldable.
12. **The log is user-visible.** `mta events tail`, `mta events show <id>` (with its causation chain),
    `mta replay --dry-run`, `mta projections list|rebuild|verify`, `mta doctor --events`. A feature that
    cannot be explained without the log is a feature nobody can debug.

## Consequences

**Positive:** "why is the board like this?" has an answer — the event, its command, and its causation
chain. Every read model becomes disposable, so a schema or UI change means a rebuild, not a migration
gamble, and `mta projections verify` turns silent corruption into a test failure. Latency stops depending
on a cadence: approvals, cancellations, and control commands are push. Crash recovery becomes replay,
which is testable with fixtures instead of simulated process kills. The v0.2 daemon/worker protocol
already exists in the log, so replicating a log is strictly easier than replicating mutable tables —
this materially de-risks the multi-machine roadmap the repo is named for. Auditing, budget accounting,
and Jev decision outcomes all land in one ordered stream instead of being reconstructed from six tables.

**Negative:** this is a genuinely larger M1 — an event store, an envelope, a decider framework,
projection offsets, snapshots, replay/rebuild tooling, and upcasters, before the first kanban card can
be drawn. Replay determinism is a discipline that can be violated invisibly (one `Utc::now()` inside a
decider and the log stops being authoritative), so it needs lint-level and test-level enforcement
rather than good intentions. Debugging moves up a level of indirection: reading the current tables no
longer explains their contents, and the `events`/`causation` views are mandatory tools. Storage grows
faster than the CRUD design and the retention/GC story has an inherent tension with "the log is the
truth" that cannot be fully resolved — only bounded and documented. Projection lag becomes a new class
of bug: a UI bug and a stale offset look identical from the outside, so the status line must show it.

**Follow-ups:** `seq` stays gap-tolerant (resolved in [events.md](../events.md) §15.1 — a counter table
would serialise every append for no real gain); a frozen per-run audit export **is** mandatory before its
stream is pruned (also §15.1); add an undo path as compensating commands (`mta undo`) once the log is
live — cheap and high-value, out of scope for v0.1; revisit `MessageAppended` granularity if transcripts
exceed tens of thousands of rows per run; consider snapshotting projections themselves to cut cold-start
replay on a large log.
