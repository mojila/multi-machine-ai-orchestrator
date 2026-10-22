# Architecture Decision Records

Short, numbered records of decisions that are expensive to reverse. Format: context → decision →
consequences, with alternatives considered.

## Index

| ADR | Title | Status |
|---|---|---|
| [0001](0001-workspace.md) | Cargo workspace with `domain` / `store` / `provider` / `engine` / `tui` / `cli` crates | Proposed (M0) |
| [0002](0002-version-pinning.md) | Pin MSRV 1.94 and exact pre-1.0 ratatui crate versions | Proposed (M0) |
| [0003](0003-engine-owns-store.md) | Engine owns the store; TUI consumes view models over events | Proposed (M0) |
| [0004](0004-jev-decision-provider.md) | Jev is a first-class decision provider, not a chat provider | Proposed (M3) |
| [0005](0005-build-size-budgets.md) | Build artifacts and binary size are budgeted, not unbounded | Proposed (M0) |
| [0006](0006-opencode-subagents.md) | Reuse OpenCode agent definitions as the sub-agent roster | Proposed (M5.5) |
| [0007](0007-context-compaction.md) | Pre-flight rolling context compaction with a calibrated estimator | Proposed (M5) |
| [0008](0008-mouse-hit-map.md) | Pointer input via a render-time hit map, with strict keyboard parity | Proposed (M2) |
| [0009](0009-event-sourced-core.md) | Event-sourced core: the append-only log is the source of truth | Proposed (M1) |
| [0010](0010-agent-and-skill-discovery.md) | Discover agents and skills from multiple roots, with skills as a separate kind | Proposed (M5.5) |
| [0011](0011-observability-and-diagnostics.md) | Structured diagnostics with `tracing`, kept separate from the event log | Proposed (M0) |
| [0012](0012-task-estimation.md) | Event-sourced auto-estimation: heuristic, calibration, optional Jev refinement | Proposed (M2) |
| [0013](0013-model-selection-and-search.md) | Searchable model selection, persisted as a config binding; live discovery is ephemeral | Proposed (M6) |

ADRs 0001–0003 are scheduled as deliverables of milestone M0 (see
[PLAN.md](../PLAN.md#m0--bootstrap-2-days)); ADR-0004 is drafted with the provider design and accepted
in M3; ADR-0005 is accepted in M0; ADR-0006 is accepted in M5.5; ADR-0007 is accepted in M5; ADR-0008 is
accepted in M2; ADR-0009 is accepted in M1, together with its contract document
[events.md](../events.md). ADR-0010 extends ADR-0006's discovery roots and is accepted in M5.5. They are
listed here so the numbering is reserved.

ADR-0003's rule that the engine owns the store and the UI never touches the database stands;
ADR-0009 amends the part of it that made SQLite tables (rather than the event log) the source of truth.
ADR-0011 adds the diagnostics half of the observability story (the event log is the audit trail; a
`tracing` log is not) and is accepted in M0 with its contract in [observability.md](../observability.md).
ADR-0012 makes task estimates event-sourced facts rather than derived UI values: the heuristic and the
event/command contract land in M2, the forecast overlay in M6, and calibration plus the optional Jev
refinement in M7. ADR-0013 adds a searchable model picker: the shared query parser and `models list
--search` land in M3, the picker, profile form, and atomic config writer in M6; a selection is a
model-only `[agents.model_bindings]` entry recorded through `ReloadConfig`, while the live catalogue is
ephemeral and never an event.

## Rules

- One decision per file, filename `NNNN-slug.md`, four-digit sequential number.
- Status is one of `Proposed`, `Accepted`, `Rejected`, `Superseded by NNNN`, `Deprecated`.
- Accepted ADRs are not edited except for status/links; supersede instead.
- Use [TEMPLATE.md](TEMPLATE.md) as the starting point.
