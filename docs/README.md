# Documentation

This directory holds the plans and design documentation for **`mta`**, the AI agent
orchestrator with a Kanban TUI.

## Index

| Document | Purpose |
|---|---|
| [PLAN.md](PLAN.md) | Implementation plan: product spec, architecture, schema, milestones, risks, backlog |
| [events.md](events.md) | Event-sourcing contract: events, commands, effects, deadlines, projections, replay |
| [providers.md](providers.md) | OpenAI-compatible provider compatibility matrix, test procedure, known quirks |
| [agents.md](agents.md) | Agent and skill compatibility: multi-root discovery (`.opencode`, `.agents`), field/permission mapping, trust gates, delegation, skill attachment |
| [observability.md](observability.md) | Diagnostics: the two-records rule, levels, sinks, TUI safety, redaction, correlation spans, rotation, support bundle |
| [build.md](build.md) | Build hygiene: bounded `target/`, size-aware profiles, budgets, prune commands |
| [adr/](adr/) | Architecture decision records — one file per decision, numbered |

## Conventions

- **Plans** live at `docs/PLAN.md` (or `docs/plans/<topic>.md` for plans that outlive the main one).
  Update the plan rather than starting a parallel document when scope changes; record the change in an
  ADR if it reverses a decision.
- **Decisions** go in `docs/adr/NNNN-slug.md`, using [the template](adr/TEMPLATE.md). ADRs are
  immutable once accepted: supersede them with a new ADR instead of rewriting history.
- **Provider notes** (tested versions, quirks, capability gaps) go in [providers.md](providers.md),
  never only in commit messages.
- **State changes go through the log.** If a change alters persisted state, it is an event in
  [events.md](events.md) applied by a validated command — not a row write. Adding a new event type or
  changing an existing payload is a contract change: update `events.md` in the same PR (ADR-0009).
- **Estimates are events, not render-time math.** A card's effort and run forecast are `TaskEstimated`
  facts with provenance, confidence, and an idempotent `inputs_hash`; a rebuild must reproduce them
  without running the estimator or a model ([ADR-0012](adr/0012-task-estimation.md)).
- **Diagnostics are not the audit trail.** Debug output goes through `tracing` into rotating files,
  is never replayed, and is never the only record of a state change — that is what events are for
  ([observability.md](observability.md), ADR-0011). A decider does not log at all (`mta-domain` has no
  `tracing` dependency), and no secret, header, or prompt body reaches a record without an explicit
  opt-in.
- Diagrams are ASCII/Mermaid in Markdown so they render in the terminal and on GitHub alike.
- Keep secrets and personal data out of docs, examples, and screenshots.

## Planned documents (created as the corresponding milestone lands)

| Document | Milestone |
|---|---|
| `docs/config.md` — full configuration reference | M6 |
| `docs/security.md` — threat model, tool policies, approval semantics | M6 |
| `docs/architecture.md` — engine/TUI deep dive and run protocol (the event contract itself lives in [events.md](events.md)) | M7 |
| `docs/decisions.md` — Jev policies, thresholds, calibration reports | M7 |
| `docs/contributing.md` — dev setup, testing, snapshot workflow | M8 |
| `docs/demo.md` — fresh-install walkthrough and asciinema script | M8 |

**Status:** planning stage — no code exists yet; see [PLAN.md §5](PLAN.md#5-milestones).
