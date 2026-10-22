# ADR-0012: Event-sourced, auto-derived task estimates

- **Status:** Proposed
- **Date:** 2026-10-22
- **Deciders:** @mojila
- **Related:** [PLAN.md §2.3](../PLAN.md#23-tui-specification), [PLAN.md §3.3](../PLAN.md#33-domain-model-and-state-machines),
  [PLAN.md §3.4](../PLAN.md#34-persistence-sqlite), [PLAN.md §3.7](../PLAN.md#37-orchestration-policy-engine),
  [events.md §5.1](../events.md#51-board-and-task-boardid-taskid), [events.md §6](../events.md#6-commands),
  [ADR-0009](0009-event-sourced-core.md), [ADR-0004](0004-jev-decision-provider.md)

## Context

A kanban board tells you what is in flight. It does not tell you whether the week is already full. The
user asked for an **automatic estimate on every card and an overall estimate on the board**, which is a
planning question: how much work is open, and when is it likely to finish?

The obvious implementation — compute a number in the render path or write a mutable `estimate` column
from a background job — conflicts with the architecture already chosen:

- **The log is the truth (ADR-0009).** Any persisted planning number must be an event, or a rebuild of
  the projections produces a different board from the one the user saw.
- **Deciders are pure.** A `decide` cannot call a model, read a clock, or query calibration history.
- **No polling.** The engine has no tick; anything that needs to happen on change is an effect reacting
  to an event, with an idempotency key.
- **Jev is advisory and optional (ADR-0004).** Estimation must work with no provider, no key, and no
  network, and a model must never be a hard dependency for showing a board.
- **Two different quantities are being conflated.** A two-hour writing task can be a four-minute agent
  run. Human effort drives planning; run duration and cost drive the forecast. One number cannot
  calibrate the other.

Options considered:

1. **Derive the estimate at render time** from the card fields. No storage, but the header total changes
   when the formula changes, a manual override has nowhere to live, and a replay cannot reproduce the
   screen the user saw. Rejected.
2. **A mutable `estimate` column plus a periodic recalculation job.** Simple to write, but it is a
   shadow write path outside the log, and "periodic" is exactly the polling cadence the engine removed.
   Rejected.
3. **One LLM/Jev estimate per card.** Semantically appealing, but it costs a call per edit, is slow and
   non-deterministic, fails closed when the provider is down, and gives no explanation for the number.
   Rejected as the primary path; kept as an optional refinement.
4. **Event-sourced estimate: pure heuristic first, calibration from recorded actuals, optional Jev
   refinement, displayed per card and in aggregate.** Chosen.

## Decision

### 1. Two numbers, one primary

Every task carries:

- `estimate_minutes` — expected **human effort**, the planning number. Integer minutes, rounded to 15,
  clamped to `[15, 480]` by default.
- `run_forecast` — expected agent **wall-clock and cost** for the assigned agent/model. Optional; absent
  when no agent is assigned or no history exists.

They are stored and calibrated separately. Effort never calibrates from agent time, and a run forecast
never pretends to be human effort.

### 2. Estimates are events

| type | stream | payload |
|---|---|---|
| `TaskEstimated` | `task:<id>` | `estimate_minutes`, `run_forecast?`, `confidence`, `source`, `estimator_version`, `inputs_hash`, `evidence` |
| `TaskEstimateCleared` | `task:<id>` | `reason` |
| `TaskEffortRecorded` | `task:<id>` | `actual_minutes`, `estimate_minutes` snapshot, `tags[]`, `agent_id?`, `source` |

Commands: `EstimateTask` (automatic), `SetTaskEstimate` (pin), `ClearTaskEstimate`, `RecordTaskEffort`.
`EstimateTask` may also append `DecisionRequested{purpose: "task_estimate"}` when it asks for a
refinement; the existing `CallDecisionModel` → `RecordDecisionAnswer` path does the rest.

The value is **stored in the event**. A rebuild of `tasks` reads it; it never re-runs the heuristic and
never calls Jev. `estimator_version` is an integer bumped when the formula changes, so old estimates keep
their meaning. `source` is one of `heuristic` (no calibration applied), `learned` (a calibrated bucket
contributed; the formula is still the heuristic), `decision` (Jev-refined), or `user` (pinned). `source`
and `confidence` describe the **effort** value; the run forecast carries its own sample count in
`evidence.samples` and is omitted entirely below `min_samples`.

### 3. The heuristic

Pure integer/float arithmetic in `mta-domain`, no dependency and no I/O:

```text
base        = tag_minutes[first matching tag] ?? default_minutes        # default 60
size        = min(ceil(description_chars / description_step_chars),    # step 400 chars
                  description_max_steps) * description_step_minutes    # step 15 min, max 4
deps        = min(dependency_count * dependency_bonus_pct, 50)         # percent
minutes     = (base + size) * priority_multiplier * (1 + deps / 100)
minutes     = round_to(minutes * calibration_ratio(tag), 15)
minutes     = clamp(minutes, min_minutes, max_minutes)
```

`calibration_ratio` is the EWMA of `actual_minutes / estimate_minutes` for the card's first known tag,
applied only when that bucket has at least `min_samples` (3) actuals. It is clamped to `0.25..4.0`, and
the EWMA alpha is `0.2`. **Both constants and the bucket table live in code, not in `config.toml`** — a
config change must not alter a projection rebuild. The bucket with too few samples contributes nothing.

The function takes the effective `[estimation]` values and the looked-up bucket as an argument struct; it
reads no config, no clock, and no database itself. The effect performs the bucket lookup and passes the
numbers in, which is what keeps the estimator pure and its unit tests trivial.

Confidence starts at `0.35`; add `0.15` for a known tag, `0.10` for a description of at least 120 chars,
and `0.20` once the bucket has 5 actuals (cap `0.80`). A user pin is `1.0`.

Children: a parent displays `Σ children + 10% coordination`, but **totals are leaf-only** so a parent and
its subtasks are never counted twice. Parent cards are excluded from board and column sums.

### 4. Learning

- **Effort** learns only from explicit actuals (`mta task actual`, or the optional field in the accept
  dialog). Elapsed calendar time is not an actual: a card can sit in Review for a week without work.
- **Run forecast** learns from completed runs' `duration_ms` and `cost_micro_usd`, grouped by
  `(agent, model, tag)`, stored in the `estimate_accuracy` projection. No projection state depends on
  user config, so rebuilds are byte-identical.

### 5. Idempotence and triggers

`inputs_hash` covers the fields the estimate was computed from (`title`, `description`, `tags`,
`priority`, dependencies, parent/children, assignee). `EstimateTask` rejects an unchanged hash as a
no-op; the effect subscribes to `TaskCreated`, `TaskEdited`, assignment/dependency changes,
`TaskEffortRecorded`, terminal run events, and `EngineStarted` (backfill only). There is **no sweep and
no timer**. A decision refinement is limited to one in flight per card.

### 6. Overrides

A manual value is `source = user` and is sticky: auto-estimation never overwrites it, and only
`SetTaskEstimate`, `ClearTaskEstimate`, or `EstimateTask{force_auto: true}` changes that. Clearing shows
`?` until the next automatic pass. Estimates never gate a run, a lane move, or a budget reservation.

### 7. Display

- Card prefix: `~2h` automatic, `=2h` pinned, `?` unknown.
- Column header: card count + open-effort subtotal.
- Header: leaf-only **remaining** total (Backlog/Todo/Doing only; Review and Done are spent effort),
  `?n` for unestimated cards, filtered as `Σ 12h/38h`.
- Detail strip: `est 1h (learned, 0.72) · run ~4m / $0.03 · board 38h open → ~2d at 24h/day`.
- `T` overlay: totals by column/tag/agent, throughput, projected finish, run forecast, calibration
  table, and the unestimated list.
- A finish date appears only after `throughput_min_days` (3) of history; otherwise the overlay says
  `insufficient history`. An unestimated card contributes `?`, never `0`.

### 8. Jev refinement

One `score` question over five buckets (`<1h`, `1–2h`, `2–4h`, `4–8h`, `>1d`; midpoints `30, 90, 180,
360, 600`, clamped to `max_minutes`). It runs only when `refine_with_decision` is on, the description is
short or the tag is uncalibrated, and confidence is below `refine_below_confidence` (0.55). The answer
applies as an estimate only above `min_confidence`; otherwise the heuristic stands. The state is the
card excerpt, not the whole database, and follows `[decisions].store_state`. The call is recorded with
`purpose = "task_estimate"` and `DecisionApplied{applied_by: "policy"}`.

## Consequences

**Positive:**

- Every card and the board total are explainable: value, source, confidence, estimator version, and the
  evidence (sample count or decision id) are all in the log.
- A replay or `projections rebuild` reproduces the board the user saw, including after the formula
  changes; old events keep their version.
- Works offline with no provider; Jev is a bounded, optional improvement rather than a dependency.
- A manual pin survives edits and recalibration, which is what makes the number trustworthy.
- The same event-driven pattern as `ScanCatalogs`: an idle board appends nothing.

**Negative / costs:**

- Three new event types, four commands, two projection tables, and an effect to maintain; the domain
  enum and `docs/events.md` grow.
- The heuristic will be wrong on unusual cards. The design optimises for honest provenance and quick
  correction over false precision.
- Calibration is only as good as the actuals the user records; without them, effort confidence stays
  low and the projected finish is withheld.
- The `T` overlay and the card editor add TUI surface area and snapshots.
- `TaskEffortRecorded` carries a snapshot of the estimate in force; if the estimate is later edited, the
  historical ratio stays as recorded (which is the point, but it means the projection is not simply
  "current estimate vs actual").

**Follow-ups:**

- Record actuals from the review dialog in M6; consider an "elapsed vs effort" comparison only after
  real data exists (it is not a substitute for an actual).
- Add per-agent effort multipliers if calibration shows a stable agent effect; today calibration is
  per tag, and the run forecast is per `(agent, model, tag)`.
- Revisit the bucket table and the `480`-minute cap after a month of real boards; a change bumps
  `estimator_version` and is applied by `mta estimate recompute`, never by rewriting old events.
- Decide whether `>1d` cards should be split automatically (probably a suggestion, not an action).
