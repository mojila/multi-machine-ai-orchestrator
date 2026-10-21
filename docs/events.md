# Event sourcing: the contract for the log, commands, effects, and projections

`mta` is event-sourced. The append-only event log is the **source of truth**, every other table
is a **rebuildable projection**, and all mutation goes through a validated command. This
document is the contract for that core: the event envelope, the write path, the catalogue of
events, the commands that produce them, the effects that react to them, the deadlines that
replace every polling sweep, and the replay, rebuild, retention, and schema-evolution rules.

This is a contract, not a tutorial. It states what exists, what may be appended, and what may
not. Design rationale and the alternatives considered live in
[ADR-0009](adr/0009-event-sourced-core.md); this document does not repeat them. Architecture
context: [PLAN.md §3](PLAN.md#31-runtime-shape), the persistence layout in [PLAN.md
§3.4](PLAN.md#34-persistence-sqlite), the sub-agent and trust model in [agents.md](agents.md),
and the provider wires in [providers.md](providers.md).

**Status:** design agreed; implemented alongside the M1 (domain + store), M4 (run persistence),
and M5 (agent loop) milestones of [PLAN.md §5](PLAN.md#5-milestones). No Rust code exists yet.

**Reading rules.**

- Field lists are abbreviated: `x[]` is an array, `x?` is optional, `{a,b}` is a nested object.
- Event names are `PascalCase`, past tense, and take **no `Event` suffix**. Commands and effects
  are `PascalCase` imperative.
- One event type = one payload schema, forever. Evolve by adding a field (with an `upcaster`) or
  by adding a new type — never by silently changing what an existing type means (§12).
- Every payload bullet below is normative; the behaviour tables are descriptive of the fold.

## 1. Purpose and scope

This is the authoritative catalogue of the event-sourced core: the seven streams, the ~85
canonical event types, the command log, the effects, the deadline kinds, the nine projections,
and the snapshots/replay/GC rules. It is what a fold, a decider, a projector, or an effect
subscribes to. It is **not** the schema reference for projections (that is the SQL in [PLAN.md
§3.4](PLAN.md#34-persistence-sqlite)), not the provider contract ([providers.md](providers.md)),
and not the agent-definition contract ([agents.md](agents.md)) — those documents may reference
events, but this one owns their names and payloads. If a feature needs an event, a command, or
an effect that is not listed here, it goes in §15 Open questions until it is added deliberately;
inventing a name at the call site is a bug.

## 2. The one rule

**The log is the source of truth; every table is a projection.** Three consequences follow, and
every rule later in this document is a special case of them:

1. A projection can be deleted and rebuilt from the log at any time, with identical results (§10,
   §14). Nothing may depend on a projection that could not be derived from events.
2. **No component writes a projection directly.** The projector is the only writer of projection
   tables (§9). A command handler that "just updates the row" has bypassed the log and broken the
   contract.
3. The only durable writes are: append a command, append a validated event, or write a per-stream
   snapshot. Everything else is derived.

### 2.1 Ephemeral state is not in the log

**Streaming token deltas, step and tool progress, hover state, host metrics, and the TUI repaint
cadence are never events.** They travel on in-process channels and nowhere else:

| Ephemeral (channel only) | Durable counterpart (in the log) |
|---|---|
| `LiveTextDelta` (assistant token deltas) | `MessageAppended` when the step's text settles |
| tool/step progress spinners, elapsed counters | `RunStepStarted`, `RunStepCompleted` |
| hover highlight, selection, scroll offset | nothing — presentation only, reconstructed locally |
| TUI repaint tick (default 250 ms) | nothing — see §8 |
| host samples: CPU, RAM/swap, disk, top procs (`watch`, default 1 s) | nothing — non-deterministic, so logging them would break replay; the opt-in `HostPressureObserved` (§5.8) is the only logged form |

These values are **cosmetic, droppable, and must never be required to reconstruct state**. A
client that misses every delta and reads only the log must still render the same board,
transcript, and run status. This is what keeps the log from being flooded by one event per
token; a run that streams 40 000 tokens appends a small constant number of events for that step
(a request, one `MessageAppended`, a completion). The one cadence that remains in the system is
the TUI repaint interval, and it is presentation-only: it computes clocks and relative times
from already-projected rows and never reads or writes domain state. Host metrics follow the same
rule for a stronger reason: a machine's instantaneous CPU or free memory is not reproducible, so
an event carrying it would make replay non-deterministic. **Replaying a log never reads the
host**, and a projection rebuilt on a different machine is byte-identical.

## 3. Event envelope

Every event, on every stream, has exactly this envelope. It is the only durable record of a
domain fact.

| Column | Type | Meaning |
|---|---|---|
| `seq` | INTEGER | Global, monotonic total order. **Gaps are legal**; not an error. |
| `event_id` | TEXT | ULID/UUIDv7; unique, sortable, the address for `mta events show`. |
| `stream_id` | TEXT | The target stream; one of the seven in §3.1. |
| `stream_type` | TEXT | The aggregate kind (same names, without `<id>`). |
| `type` | TEXT | The event type, e.g. `TaskMoved`. One type = one schema, forever. |
| `version` | INTEGER | Per-stream position `1..n`, contiguous. Concurrency token (§9). |
| `schema_version` | INTEGER | Payload shape version for this `type`; see §12. |
| `at` | INTEGER | Unix ms UTC, chosen at **command time**; never read inside `decide` (§4). |
| `actor` | TEXT | `user:<id>`, `agent:<id>`, `engine`, `scheduler`, or `effect:<name>`. |
| `correlation_id` | TEXT | The `command_id` that caused this chain; the chain's idempotency key. |
| `causation_id` | TEXT | The parent `event_id`, or null at a chain root; links parent and child. |
| `payload_json` | TEXT | The typed payload; shape fixed per `type` and `schema_version`. |

**Why gaps in `seq` are legal.** `seq` is assigned by the append-only store when the event row
is committed. A transaction that appends several events and then rolls back leaves the
autoincrement value advanced, so the next committed event does not have the previous committed
`seq` plus one. Consumers must compare `seq > offset` and never assume `seq == offset + 1`.
Ordering is still total and consistent; only density is not guaranteed. `mta doctor --events`
reports the largest observed gap as information, not as an error. (Whether to make `seq` gapless
with a counter table is undecided; see §15.)

**Per-stream `version` is contiguous.** Unlike `seq`, a stream's versions must be exactly `1..n`
with no gaps, because they are the concurrency token. An append at the wrong version is rejected
(§9). Streams are the unit of optimistic concurrency, of snapshots, and of aggregate loading.

Envelope example (one `MessageAppended` on a run stream):

```json
{
  "seq": 4187,
  "event_id": "0192f3a1-7c44-7b21-9e0a-1f2c3d4e5f60",
  "stream_id": "run:7c1e9a00-2b41-7d55-9a10-0c1d2e3f4a5b",
  "stream_type": "run",
  "type": "MessageAppended",
  "version": 9,
  "schema_version": 1,
  "at": 1761106440000,
  "actor": "agent:ops",
  "correlation_id": "cmd_01J9Z3K7P0",
  "causation_id": "0192f3a1-7b33-7b21-9e0a-1f2c3d4e5f10",
  "payload_json": {
    "message_id": "m_01J9Z3K8Q1",
    "role": "assistant",
    "kind": "message",
    "source": "assistant",
    "content_ref": "messages/7c1e/0009.txt",
    "tokens_est": 812
  }
}
```

### 3.1 Streams (aggregates)

| Stream | Aggregate | Holds |
|---|---|---|
| `board:<id>` | Board | columns, WIP limits, priority order |
| `task:<id>` | Task | card fields, lane, position, deps, assignee, task-scoped decisions |
| `run:<id>` | Run | life cycle, steps, transcript references, approvals, usage, budget |
| `agent:<id>` | Agent | discovered/trusted/replaced definitions and trust state |
| `skill:<id>` | Skill | discovered/trusted/changed `SKILL.md` entries (§5.6) |
| `schedule:<id>` | Schedule | registration, due/fired/skipped/disabled |
| `digest:<id>` | Digest | digest artefact life cycle (requested/completed/failed, §5.8) |
| `system` | singleton | engine life cycle, lock, config, projections, GC, deadlines, day rollover |

Approvals and decisions are **events on the stream they belong to**, never separate streams. An
approval for a run step lives on `run:<id>`; an `auto_assign`, `spec_gate`, or `priority`
decision lives on `task:<id>`; a trust event lives on the `agent:<id>` or `skill:<id>` stream it is
about. Child runs are their own `run:<id>` stream; the parent records `SubagentStarted` and
`SubagentCompleted` on **its own** stream, linked to the child by `causation_id`.

**Agents and skills are separate aggregates** even though they share a discovery scan and one roster
surface ([agents.md §2](agents.md#2-discovery)): an agent defines a model and a tool policy, a skill
defines neither and can never execute a run. Sharing one stream would make `agent:<id>` mean two things
and leave the `skill` kind's life cycle unmodelled, so a skill gets `skill:<id>`. The split is also what
keeps trust auditable per kind: `AgentTrusted` and `SkillTrusted` carry the same `(project, source_hash)`
key but live on their own streams, so revoking a skill's trust cannot be confused with revoking an
agent's.

## 4. The write path

All mutation follows the same six steps. There is no other way to change durable state.

1. **Command.** The issuer (TUI, CLI, scheduler, or an effect returning a fact) appends a command
   to the durable command log with its `command_id`, issuer, and `at`. The command carries every id
   and timestamp the resulting events will need.
2. **Idempotency check.** If a command with the same `command_id` has already produced events, the
   command is a no-op. This makes effect retries safe (§7).
3. **Load aggregate.** The decider loads the latest snapshot for the target stream, then folds the
   tail of events after `snapshot.seq`. The result is the aggregate state at version `v`.
4. **Decide.** A **pure** function `decide(state, command) -> Result<Vec<Event>, Rejection>` runs.
   It performs no I/O (see purity rules below) and returns either the events to append or a
   rejection.
5. **Append.** Events are inserted with `version = v+1, v+2, …` and a fresh global `seq`, inside
   one transaction. The append is accepted only if the stream's current version still equals `v`
   (optimistic concurrency). A lost race appends nothing and emits `CommandRejected` (§9).
6. **Publish, project, effect.** Committed events are published to in-process subscribers. The
   projector advances its offsets by folding the new events; effect subscribers run their side
   effects, which return as new commands (step 1).

```mermaid
sequenceDiagram
    participant S as Source (TUI / CLI / effect)
    participant L as Event log + command log
    participant D as Decider (pure)
    participant P as Projector
    participant E as Effect runner
    S->>L: append Command(command_id, at, ids)
    D->>L: load snapshot + tail -> state (v)
    D->>D: decide(state, command) -> Events | Rejection
    D->>L: append Events(expected_version = v)
    L-->>P: publish (seq, stream, type, payload)
    P->>P: fold into projections; advance offsets
    L-->>E: publish (event types)
    E->>E: run side effect (HTTP / tool / subagent)
    E->>L: append Command(result, same command_id)
```

**Deciders are pure.** Inside `decide` there is no clock, no randomness, no database read, and
no network. Ids and timestamps are chosen by the command issuer and stored in the command, so
replaying the same command against the same state always yields the same events. Anything that
cannot be pure is an **effect**: it runs outside `decide`, observes events, and returns its
result as a new command. A single `Utc::now()`, a `rand::random()`, or a `sqlx::query` inside a
decider silently breaks replay determinism; this is enforced by lint and by the tests in §14.

**External facts arrive as commands.** An HTTP reply, a tool's exit status, or a timer firing is
not appended directly. It is wrapped in a command (`RecordLlmResult`, `RecordToolResult`,
`FireDeadline`) and validated by the decider before any event is appended. No event is ever
appended unvalidated. `command_id` gives idempotency, so a retried effect cannot double-append:
the same fact replayed twice produces one event set.

## 5. The event catalogue

Every canonical event, grouped by the stream it lives on. A `schema_version` column is implied
and is `1` for every event until an upcaster raises it (§12); the version is recorded per row in
the log and is what the reader uses to choose an upcaster. "producer → consumers" names the
command (or the run loop) that appends the event on the left and the non-projector reactors on
the right; **every event is always consumed by its projections**, so those are not repeated in
the consumer list. Payload fields are listed per group under "Payloads".

### 5.1 Board and task (`board:<id>`, `task:<id>`)

| type | stream | fold → projections | producer → consumers |
|---|---|---|---|
| `BoardCreated` | board | board row + columns → board | bootstrap/CLI → TUI |
| `BoardConfigured` | board | wip + priority order → board | decider → scheduler,TUI |
| `TaskCreated` | task | insert card → tasks,search | AddTask → scheduler,TUI |
| `TaskEdited` | task | apply field diff → tasks,search | EditTask → TUI |
| `TaskMoved` | task | set column/position → tasks,board | MoveTask → scheduler,TUI |
| `TaskReordered` | task | set position → tasks | ReorderTask → TUI |
| `TaskAssigned` | task | set assignee → tasks,agents | AssignTask → scheduler,TUI |
| `TaskUnassigned` | task | clear assignee → tasks | decider → TUI |
| `TaskDependencyAdded` | task | add edge → tasks | AddDependency → scheduler |
| `TaskDependencyRemoved` | task | drop edge → tasks | decider → scheduler |
| `TaskFeedbackAdded` | task | append feedback → tasks | AddFeedback → scheduler |
| `TaskCompleted` | task | lane = done → tasks | MoveTask → TUI |
| `TaskReopened` | task | lane = todo → tasks | decider → scheduler,TUI |
| `TaskDeleted` | task | remove card → tasks,search | DeleteTask → TUI |

**Payloads**

- `BoardCreated` — `name`, `columns[]`.
- `BoardConfigured` — `wip_limits`, `priority_order`.
- `TaskCreated` — `board_id`, `column`, `title`, `description`, `priority`, `due_at`, `tags[]`,
  `position`, `actor`.
- `TaskEdited` — `changes` as `{field: {old, new}}`.
- `TaskMoved` — `from`, `to`, `position`.
- `TaskReordered` — `column`, `position`.
- `TaskAssigned` — `agent_id`, `mode`.
- `TaskUnassigned` — `reason`.
- `TaskDependencyAdded` — `depends_on`.
- `TaskDependencyRemoved` — `depends_on`.
- `TaskFeedbackAdded` — `text`, `run_id?`.
- `TaskCompleted` — `run_id?`.
- `TaskReopened` — `run_id`.
- `TaskDeleted` — `reason`.

### 5.2 Run life cycle (`run:<id>`)

| type | stream | fold → projections | producer → consumers |
|---|---|---|---|
| `RunQueued` | run | insert queued run → runs | StartRun → scheduler |
| `RunStarted` | run | status = running → runs | run loop → TUI |
| `RunStepStarted` | run | open step → steps | run loop → TUI |
| `RunStepCompleted` | run | close step → steps | run loop → TUI |
| `RunSuspended` | run | status = awaiting_approval → runs | run loop → TUI |
| `RunResumed` | run | status = running → runs | ApproveApproval → TUI |
| `RunSucceeded` | run | status = succeeded, summary → runs,tasks | run loop → TUI |
| `RunFailed` | run | status = failed, error → runs | run loop → TUI,digests |
| `RunBlocked` | run | status = blocked, reason → runs,tasks | run loop → TUI |
| `RunCancelled` | run | status = cancelled → runs | CancelRun → TUI |
| `RunInterrupted` | run | status = interrupted → runs | bootstrap → TUI |
| `RunUsageRecorded` | run | add usage/cost → run_costs,runs | RecordLlmResult → budgets |
| `RunBudgetReserved` | run | reserve day budget → budgets | StartRun → scheduler |
| `RunBudgetExhausted` | run | status + scope → runs,budgets | run loop → TUI,budgets |

**Payloads**

- `RunQueued` — `task_id`, `agent_id`, `trigger`, `model_alias`, `budget`, `skill_ids[]`,
  `parent_run_id?`, `depth`.
- `RunStarted` — `workspace_path`, `model_alias`, `attached_skills[]` (each `{id, source_hash, body}`;
  see §5.6 and [agents.md §7.3](agents.md#73-skill-attachment)); the bodies travel in the event for
  the same reason `agent_snapshot_json` does — a replay must not depend on a file still existing or still
  saying the same thing.
- `RunStepStarted` — `step_id`, `kind`, `index`.
- `RunStepCompleted` — `step_id`, `outcome`, `duration_ms`.
- `RunSuspended` — `step_id`, `approval_id`, `reason`.
- `RunResumed` — `approval_id`, `decision`.
- `RunSucceeded` — `summary`, `artifacts[]`.
- `RunFailed` — `error_class`, `message`, `retryable`.
- `RunBlocked` — `reason`, `detail`.
- `RunCancelled` — `by`, `reason`.
- `RunInterrupted` — `detected_by`, `last_seq`.
- `RunUsageRecorded` — `step_id`, `input_tokens`, `output_tokens`, `cost_micro_usd`, `estimated`.
- `RunBudgetReserved` — `reserved_micro_usd`, `day_key`.
- `RunBudgetExhausted` — `scope`, `limit_micro_usd`, `spent_micro_usd`.

### 5.3 Steps and transcript (`run:<id>`)

`MessageAppended` is the **single place transcript content lives**; `LlmCallCompleted` carries
metrics only (usage, finish reason, cost, latency), never text. This is what keeps token deltas
out of the log (§2.1) and keeps a transcript reconstructable from events.

| type | stream | fold → projections | producer → consumers |
|---|---|---|---|
| `LlmCallRequested` | run | open llm step → steps | `AdvanceRun` → TUI |
| `LlmCallCompleted` | run | metrics on step → steps,run_costs | `RecordLlmResult` → budgets,TUI |
| `LlmCallFailed` | run | error on step → steps | `RecordLlmResult` → TUI |
| `MessageAppended` | run | append message → messages,search | `RecordLlmResult` → TUI |
| `ContextTruncated` | run | mark truncated refs → steps | `AdvanceRun` → TUI |
| `ToolCallRequested` | run | open tool step → steps | `AdvanceRun` → TUI |
| `ToolCallCompleted` | run | result on step → steps | `RecordToolResult` → TUI |
| `ToolCallFailed` | run | error on step → steps | `RecordToolResult` → TUI |
| `ToolCallDenied` | run | denied on step → steps | `AdvanceRun` → TUI |
| `CompactionPerformed` | run | compaction row → runs,steps | `CompactRun` → TUI |
| `CompactionSkipped` | run | counters only → steps | `CompactRun` → TUI |
| `CompactionFailed` | run | counters only → steps | `CompactRun` → TUI |
| `DelegateRequested` | run | open subagent step → steps | `AdvanceRun` → scheduler |
| `SubagentStarted` | run | child linkage → runs,steps | `RecordSubagentStart` → TUI |
| `SubagentCompleted` | run | child result → runs,steps,run_costs | `RecordSubagentResult` → TUI,budgets |

**Payloads**

- `LlmCallRequested` — `step_id`, `model_alias`, `provider`, `message_range`, `est_tokens`,
  `attempt`.
- `LlmCallCompleted` — `step_id`, `tool_calls[]`, `usage{input,output}`, `finish_reason`,
  `cost_micro_usd`, `estimated`, `attempts`, `duration_ms`.
- `LlmCallFailed` — `step_id`, `error_class`, `message`, `attempts`, `retryable`.
- `MessageAppended` — `message_id`, `role`, `kind`, `source`, `content_ref`, `tokens_est`; `kind`
  ∈ {`message`, `compaction_summary`}, `source` ∈ {`user`, `assistant`, `tool`, `compaction`}.
- `ContextTruncated` — `step_id`, `truncated_refs[]`, `tokens_before`, `tokens_after`; appended when
  `on_overflow = truncate` shrinks the request before it is sent, so the lossy step is auditable.
- `ToolCallRequested` — `step_id`, `tool`, `args`, `policy_decision`.
- `ToolCallCompleted` — `step_id`, `tool`, `result_summary`, `result_ref`, `duration_ms`,
  `exit_code?`.
- `ToolCallFailed` — `step_id`, `tool`, `error_class`, `message`.
- `ToolCallDenied` — `step_id`, `tool`, `reason`, `policy`.
- `CompactionPerformed` — `compaction_id`, `trigger`, `tokens_before`, `tokens_after`,
  `summary_tokens`, `model_alias`, `covered_from_seq`, `covered_through_seq`, `fallback`,
  `cost_micro_usd`; `trigger` ∈ {`auto`, `manual`, `overflow_retry`}.
- `CompactionSkipped` — `reason`.
- `CompactionFailed` — `reason`, `fallback_used`.
- `DelegateRequested` — `parent_step_id`, `child_run_id`, `agent_id`, `brief`.
- `SubagentStarted` — `child_run_id`, `agent_id`.
- `SubagentCompleted` — `child_run_id`, `status`, `summary`, `cost_micro_usd`, `duration_ms`.

### 5.4 Approvals (`run:<id>`)

An approval is the only suspension point in the run state machine ([PLAN.md
§3.3](PLAN.md#33-domain-model-and-state-machines)); it lives on the run stream it belongs to.

| type | stream | fold → projections | producer → consumers |
|---|---|---|---|
| `ApprovalRequested` | run | insert pending → approvals,deadlines | `AdvanceRun` → TUI,deadlines |
| `ApprovalDecided` | run | set decision → approvals | ApproveApproval/DenyApproval → TUI |
| `ApprovalExpired` | run | set expired → approvals | FireDeadline → TUI |

**Payloads**

- `ApprovalRequested` — `approval_id`, `step_id`, `kind`, `payload_hash`, `payload_ref`, `prompt`,
  `expires_at`, `tool?`.
- `ApprovalDecided` — `approval_id`, `decision`, `by`, `note`, `typed_confirmation`.
- `ApprovalExpired` — `approval_id`, `deadline_at`.

### 5.5 Decisions (`run:<id>`, `task:<id>`, `agent:<id>`)

A decision is one typed question set asked of a decision model (Jev / System One) and its
answers. It is advisory unless a configured policy applies it. It is an event on the stream it
belongs to: a `decide` call inside a run lives on `run:<id>`; an `auto_assign`, `spec_gate`, or
`priority` decision lives on `task:<id>`. The exact owning stream for engine-policy decisions is
a documented choice, not a new stream (§3.1).

| type | stream | fold → projections | producer → consumers |
|---|---|---|---|
| `DecisionRequested` | owner | insert request → decisions | `AdvanceRun` (agent tool)/engine → TUI |
| `DecisionAnswered` | owner | answers + confidence → decisions | RecordDecisionAnswer → TUI |
| `DecisionFailed` | owner | error + fallback → decisions | RecordDecisionAnswer → TUI |
| `DecisionApplied` | owner | applied flag → decisions,tasks | engine → TUI |
| `DecisionOutcomeRecorded` | owner | outcome → decisions | engine → digests |

**Payloads**

- `DecisionRequested` — `decision_id`, `purpose`, `model_alias`, `question_count`, `state_ref`.
- `DecisionAnswered` — `decision_id`, `answers[]`, `confidence_min`, `latency_ms`,
  `cost_micro_usd`, `model_version`.
- `DecisionFailed` — `decision_id`, `error_class`, `fallback_used`.
- `DecisionApplied` — `decision_id`, `applied_by`, `effect_summary`.
- `DecisionOutcomeRecorded` — `decision_id`, `outcome`, `note`.

### 5.6 Agents (`agent:<id>`) and skills (`skill:<id>`)

The catalogue is discovered from OpenCode Markdown, `.agents`-style Markdown, and `mta` config: agent
definitions on `agent:<id>`, skills on `skill:<id>`. Discovered entries are untrusted input until a human
approves them by content hash; that decision is itself an event. A skill is attachable to a run but is
never a delegation target ([agents.md §2.2](agents.md#22-why-skills-are-not-agents)).

| type | stream | fold → projections | producer → consumers |
|---|---|---|---|
| `AgentDiscovered` | agent | upsert + provenance → agents | `ScanCatalogs` → TUI |
| `AgentChanged` | agent | update hash + diff → agents | `ScanCatalogs` → TUI |
| `AgentUnavailable` | agent | mark unavailable → agents | `ScanCatalogs` → TUI |
| `AgentDefinitionReplaced` | agent | replace/tombstone → agents | ReloadConfig → TUI |
| `AgentTrustRequested` | agent | insert pending trust → agents,deadlines | `AdvanceRun` (delegation gate) → TUI |
| `AgentTrusted` | agent | record trust by hash → agents | TrustAgent → scheduler |
| `AgentTrustRevoked` | agent | drop trust → agents | RevokeAgentTrust → scheduler |
| `SkillDiscovered` | skill | upsert + provenance → skills | `ScanCatalogs` → TUI |
| `SkillChanged` | skill | update hash + diff → skills | `ScanCatalogs` → TUI |
| `SkillUnavailable` | skill | mark unavailable → skills | `ScanCatalogs` → TUI |
| `SkillTrusted` | skill | record trust by hash → skills | TrustSkill → TUI |
| `SkillTrustRevoked` | skill | drop trust → skills | RevokeSkillTrust → TUI |

**Payloads**

- `AgentDiscovered` — `source`, `path`, `source_hash`, `mode`, `name`.
- `AgentChanged` — `source_hash`, `diff_summary`.
- `AgentUnavailable` — `path`, `reason`.
- `AgentDefinitionReplaced` — `definition_ref`, `disabled`.
- `AgentTrustRequested` — `source_hash`, `project`, `prompt_excerpt`.
- `AgentTrusted` — `source_hash`, `project`, `by`.
- `AgentTrustRevoked` — `project`, `reason`.
- `SkillDiscovered` — `source`, `path`, `source_hash`, `name`, `description`, `body_bytes`,
  `interface` (optional `agents/openai.yaml` metadata), `resources[]` (names and counts of
  `references/`/`assets/`/`scripts/`/`evals/`, **contents never read**).
- `SkillChanged` — `source_hash`, `diff_summary`, `body_bytes`.
- `SkillUnavailable` — `path`, `reason`.
- `SkillTrusted` — `source_hash`, `project`, `by`; the same trust key shape as an agent, on the skill's
  own stream ([agents.md §6.4](agents.md#64-trust-gate-for-project-skills)).
- `SkillTrustRevoked` — `project`, `reason`.

There is deliberately **no `SkillTrustRequested`**: a skill's trust is decided at attachment, before the
run exists, so there is no in-run suspension point and no deadline to arm — a `StartRun` naming an
untrusted skill is rejected rather than queued
([agents.md §6.4](agents.md#64-trust-gate-for-project-skills)). That asymmetry with
`AgentTrustRequested` (which exists because delegation happens inside an open step) is
intentional and is why the two kinds do not share one event set.

### 5.7 Schedules and time (`schedule:<id>`, `system`)

| type | stream | fold → projections | producer → consumers |
|---|---|---|---|
| `ScheduleRegistered` | schedule | insert schedule → schedules | RegisterSchedule → scheduler |
| `ScheduleDue` | schedule | arm due mark → schedules,deadlines | RegisterSchedule → deadlines |
| `ScheduleFired` | schedule | link run/task → schedules | FireDeadline → scheduler |
| `ScheduleSkipped` | schedule | skip reason → schedules | FireDeadline → TUI |
| `ScheduleDisabled` | schedule | disable → schedules | DisableSchedule → scheduler |
| `DeadlineReached` | system | close deadline → deadlines | FireDeadline → deadlines |
| `DayRolled` | system | reset day counters → budgets | RollDay → budgets,digests |

**Payloads**

- `ScheduleRegistered` — `name`, `cron`, `board_id`, `column`, `template_ref`, `timezone`.
- `ScheduleDue` — `due_at`, `local_date`.
- `ScheduleFired` — `run_id?`, `task_id?`.
- `ScheduleSkipped` — `reason`.
- `ScheduleDisabled` — `reason`.
- `DeadlineReached` — `deadline_id`, `kind`, `ref`; `kind` is one of the five §8 values:
  `approval_expiry`, `retry_backoff`, `run_timeout`, `schedule_due`, `provider_retry`.
- `DayRolled` — `day_key`, `previous_day_key`, `timezone`.

### 5.8 System (`system`) and digests (`digest:<id>`)

| type | stream | fold → projections | producer → consumers |
|---|---|---|---|
| `EngineStarted` | system | record start → system | bootstrap → TUI |
| `EngineStopped` | system | record stop → system | bootstrap → TUI |
| `LockAcquired` | system | set lock holder → system | AcquireEngineLock → CLI |
| `LockReleased` | system | clear lock → system | ReleaseEngineLock → CLI |
| `ConfigReloaded` | system | record hash/sections → system | ReloadConfig → engine |
| `CrashRecovered` | system | reconcile interrupted → runs,system | bootstrap → scheduler,TUI |
| `ProjectionRebuilt` | system | record rebuild → system | RebuildProjection → offset |
| `SnapshotTaken` | system | record snapshot → system | TakeSnapshot → snapshots |
| `GcPerformed` | system | record prune → system | RunGc → backups |
| `CommandRejected` | system | record rejection → system | any decider → TUI |
| `EffectFailed` | system | record failure → system | effect runner → TUI |
| `HostPressureObserved` | system | record crossing → system | `ObserveHostPressure` → TUI |

**Payloads**

- `EngineStarted` — `version`, `db_path`, `config_hash`, `resumed_from_seq`.
- `EngineStopped` — `reason`, `last_seq`.
- `LockAcquired` — `pid`, `host`.
- `LockReleased` — `pid`.
- `ConfigReloaded` — `config_hash`, `changed_sections[]`, `by`.
- `CrashRecovered` — `interrupted_run_ids[]`, `replayed_to_seq`, `reissued_effects[]`.
- `ProjectionRebuilt` — `name`, `from_seq`, `to_seq`, `duration_ms`, `rows`.
- `SnapshotTaken` — `stream_id`, `version`, `seq`.
- `GcPerformed` — `pruned_before_seq`, `events_pruned`, `snapshots_pruned`, `bytes`.
- `CommandRejected` — `command_id`, `type`, `reason`, `expected_version`, `actual_version`.
- `EffectFailed` — `effect_id`, `event_id`, `kind`, `attempts`, `error_class`.
- `HostPressureObserved` — `metric` (`cpu|mem|disk`), `value_pct`, `threshold_pct`,
  `run_ids_active[]`, `sampled_at`. **Opt-in** (`[host].observe_pressure`) and **debounced**:
  appended only on a rising threshold crossing and never more often than
  `[host].observe_pressure_cooldown_s`. It exists so a run failure caused by host pressure can be
  *explained* from the log; samples themselves are ephemeral (§2.1) and are never appended. It is
  the only event in this document produced by the sampler, and it is deliberately a *crossing*, not
  a *reading* — a reading would make replay non-deterministic.

**Digests (`digest:<id>`).** A digest is its own short-lived aggregate with its own stream id (a
ULID), so a digest's request, artifact, and failure are one replayable chain rather than loose
system facts. This resolves the earlier ambiguity about whether the declared `digest:<id>` stream
was reserved for the future: it is used now.

| type | stream | fold → projections | producer → consumers |
|---|---|---|---|
| `DigestRequested` | digest | insert digest → digests | `RequestDigest` → digests |
| `DigestCompleted` | digest | write digest → digests | `ComputeDigest` → TUI |
| `DigestFailed` | digest | fallback digest → digests | `ComputeDigest` → TUI |

- `DigestRequested` — `scope`, `since`, `until`, `out_path`.
- `DigestCompleted` — `path`, `model_alias`, `fallback`, `cost_micro_usd`.
- `DigestFailed` — `reason`, `fallback_used`.

## 6. Commands

Commands are the only door into the log. They are appended to a **durable command log** so that
a crashed engine can find work that was requested but never completed, and so that a retried
effect has a stable `command_id` to dedupe on. Every command is deduped by its `command_id`. A
command either produces a validated event set or is rejected; it never writes a projection.

| command | issuer → stream | may append | rejected when |
|---|---|---|---|
| `AddTask` | TUI, CLI → task | TaskCreated | board/column missing, WIP |
| `EditTask` | TUI, CLI → task | TaskEdited | unknown field, stale version |
| `MoveTask` | TUI, engine → task | TaskMoved (+Completed/Reopened) | illegal transition |
| `ReorderTask` | TUI → task | TaskReordered | stale position |
| `AssignTask` | TUI, engine → task | TaskAssigned | unknown/ineligible agent |
| `DeleteTask` | TUI, CLI → task | TaskDeleted | active root run |
| `AddFeedback` | TUI, engine → task | TaskFeedbackAdded | — |
| `AddDependency` | TUI, engine → task | TaskDependencyAdded | cycle, self-dep |
| `StartRun` | TUI,scheduler → run | RunQueued, RunBudgetReserved | active root run, budget, unknown/untrusted skill or skill hash mismatch |
| `TrustSkill` | TUI, CLI → skill | SkillTrusted | unknown skill, hash changed |
| `RevokeSkillTrust` | TUI, CLI → skill | SkillTrustRevoked | no such trust |
| `ScanCatalogs` | bootstrap, CLI, watcher → agent, skill | Agent*/Skill* discovery events | no root changed since the last scan |
| `RetryRun` | TUI, engine → run | RunQueued | attempts gone, budget |
| `CancelRun` | TUI, CLI → run | RunCancelled | already terminal |
| `CompactRun` | TUI, CLI → run | CompactionPerformed/Skipped/Failed | run terminal |
| `ApproveApproval` | TUI,CLI → run | ApprovalDecided, RunResumed | not pending |
| `DenyApproval` | TUI,CLI → run | ApprovalDecided, RunFailed | not pending |
| `TrustAgent` | TUI, CLI → agent | AgentTrusted | unknown agent, hash changed |
| `RevokeAgentTrust` | TUI, CLI → agent | AgentTrustRevoked | no such trust |
| `ReloadConfig` | engine → system | ConfigReloaded (+agent diffs) | invalid config |
| `RegisterSchedule` | CLI,config → schedule | ScheduleRegistered (+Due) | bad cron/dup |
| `DisableSchedule` | TUI, CLI → schedule | ScheduleDisabled | already disabled |
| `BeginRun` | engine → run | RunStarted | not queued, budget, no slot |
| `AdvanceRun` | run loop → run | RunStepStarted + (`LlmCallRequested` \| `ToolCallRequested` \| `ApprovalRequested` \| `DelegateRequested`), `ContextTruncated`, `ToolCallDenied` | terminal, suspended, or a step already open |
| `RecordSubagentStart` | engine → run | SubagentStarted | child unknown |
| `RecordSubagentResult` | engine → run | SubagentCompleted | child unknown |
| `RecordLlmResult` | effect → run | LlmCallCompleted,MessageAppended | stale step |
| `RecordToolResult` | effect → run | ToolCallCompleted, ToolCallFailed | stale step |
| `RecordDecisionAnswer` | effect → owner | DecisionAnswered, DecisionFailed | malformed answer |
| `RequestDigest` | CLI, schedule → system | DigestRequested | overlap in flight |
| `RebuildProjection` | CLI → system | ProjectionRebuilt | unknown projection |
| `TakeSnapshot` | engine, CLI → system | SnapshotTaken | stream at same version |
| `RunGc` | CLI, engine → system | GcPerformed | retention off, no force |
| `AcquireEngineLock` | bootstrap → system | LockAcquired | lock held by live pid |
| `ReleaseEngineLock` | bootstrap → system | LockReleased | not holder |
| `FireDeadline` | timer wheel → system + owner | DeadlineReached (+expiry/fire) | stale deadline |
| `RollDay` | timer wheel → system | DayRolled | day already rolled |
| `ObserveHostPressure` | sampler → system | HostPressureObserved | `observe_pressure` off, below threshold, or inside the cool-down |

**The run loop.** A run advances because something asks it to. The **run loop** is the engine
component that drives a run stream: it subscribes to that stream's events and submits the next
command when the run is neither terminal nor suspended. It is a decider client, not a decider — it
owns no state and appends nothing directly. Its commands are:

- `BeginRun` — take a `RunQueued` run and mark it started, after slot and budget checks.
- `AdvanceRun` — fit context (possibly appending `ContextTruncated`), then open the next step and
  request the next action.
- `RecordSubagentStart` / `RecordSubagentResult` — mirror a child run's life cycle onto the parent,
  so the parent's transcript is complete without reading another stream.

Because `AdvanceRun` is a command with an expected version, two run-loop wake-ups racing on the
same run cannot open two steps: the second is rejected as a stale version, which is the same
mechanism that guards every other invariant.

`RecordLlmResult` also appends `LlmCallFailed` on error and `RunUsageRecorded` for usage;
`FireDeadline` also appends the deadline's consequence (`ApprovalExpired`, `ScheduleFired`, etc.).
Event names in the table are the per-type names from §5, abbreviated only where a cell would not
otherwise fit.

**Discovery is a command, not a direct write.** The filesystem scan is an effect: it reads the roots and
submits **`ScanCatalogs`**, whose decider diffs what is on disk against the `agents` / `skills`
projections and appends only the differences (`AgentDiscovered`/`Changed`/`Unavailable` on `agent:<id>`,
`SkillDiscovered`/`Changed`/`Unavailable` on `skill:<id>`). This is why those events have a named producer
at all — before this, discovery events were the one set attributed to a vague "bootstrap/scan" with no
command. A scan that finds nothing new is a no-op and appends nothing, so a directory watcher firing
repeatedly cannot flood the log. `ScanCatalogs` is also the only command that targets two streams: its
`may append` column lists both, and each event still lands on the stream of the entry it is about.

**The external-facts rule.** An HTTP reply, a tool's exit status, or a timer firing **arrives as
a command** (`RecordLlmResult`, `RecordToolResult`, `FireDeadline`) and is validated by the
decider before any event is appended. An effect never appends an event directly; it appends a
command and lets the decider decide. `command_id` is the idempotency key for the whole chain, so
a retried effect that re-submits the same fact produces exactly one event set — the second
submission finds its `command_id` already applied and becomes a no-op. `CommandRejected` is
emitted for a rejected command that carried a `command_id` and expected version, so a conflict
is visible in the log rather than silent.

## 7. Effects

Effects are the engine's side of the loop: subscribers to events, running **outside** any
decider, never called from a decider. An effect reads the events it subscribes to, does I/O, and
returns its result as a command (§6). Delivery is **at-least-once**: a crash between doing the
work and appending the result command causes the effect to be re-issued after restart, and the
`command_id` (derived from the originating event) makes the re-issue idempotent.

| effect | triggered by | action | result command | retry / cancel |
|---|---|---|---|---|
| `CallChatModel` | LlmCallRequested | stream chat | RecordLlmResult | backoff; cancel |
| `CallDecisionModel` | DecisionRequested | POST+validate | RecordDecisionAnswer | 429/5xx/cancel |
| `ExecuteTool` | ToolCallRequested | run a tool sandboxed | RecordToolResult | kill group; cancel |
| `RunSubagent` | DelegateRequested | start a child run | child's own events | inherits cancel |
| `SummariseContext` | CompactRun | summarise an old span | RecordLlmResult | fallback |
| `WriteArtifact` | RunSucceeded | persist + hash files | folded next run event | path+hash |
| `ComputeDigest` | DigestRequested | render Markdown | DigestCompleted/Failed | fallback; cancel |
| `ScanCatalogs` | EngineStarted, ConfigReloaded, fs watcher | read the discovery roots in §5.6 | `ScanCatalogs` | re-issued on restart; the diff makes it idempotent |

**Idempotency key = the `command_id` of the originating command.** Each effect derives its
command id deterministically from the event it is reacting to (for example `hash(event_id,
effect_name)`), so a restart that re-issues the same effect cannot append its result twice.
`EffectFailed` is appended when an effect exhausts its retries; **a pending effect with no
result event is re-issued after a crash**, and `CrashRecovered` lists the effects that were
reissued. Cancellation is hierarchical and follows the run: cancelling a run cancels its in-flight
effects, including child-run effects and tool process groups
([PLAN.md §3.3](PLAN.md#33-domain-model-and-state-machines)). `SummariseContext` and
`WriteArtifact` are not interruptible mid-call; they rely on idempotency instead of cancellation.

## 8. Deadlines and the timer wheel

There is **no `tick` and no sweeper** in the domain. Anything that needed a poll is modelled as
a *deadline*: an event that requires a future action arms one; the timer wheel arms the
earliest; when it fires it appends the `FireDeadline` command, whose decider appends
`DeadlineReached` (and any consequent events, such as `ApprovalExpired` or `ScheduleFired`).
Pending deadlines are **derived from events**, so they survive a restart by replay, not by a
startup scan.

Deadline kinds, and the events that arm and cancel them:

| kind | armed by | cancelled by | on fire |
|---|---|---|---|
| `approval_expiry` | ApprovalRequested | ApprovalDecided | ApprovalExpired, RunFailed |
| `retry_backoff` | RunFailed (retryable) | RetryRun | RetryRun (new run) |
| `run_timeout` | RunStarted | RunSucceeded/Failed/Cancelled | RunCancelled (`timeout`) |
| `schedule_due` | ScheduleRegistered | ScheduleDisabled | ScheduleFired/Skipped |
| `provider_retry` | LlmCallFailed, DecisionFailed | retry recorded | re-issue effect command |

`FireDeadline` carries the `deadline_id`; the decider verifies the deadline is still open (not
already fired, not cancelled by a later event) before appending `DeadlineReached`. If it is
stale — the approval was decided, the schedule disabled — the command is a no-op and appends
nothing. Firing is therefore deterministic under a fake clock and testable without wall-clock
waits (§14).

**The only cadence left is the TUI repaint.** The default 250 ms interval redraws clocks,
spinners, and relative times from already-projected rows. It is presentation-only: it never arms
a deadline, never reads domain state to make a decision, and can be disabled entirely without
changing behaviour. Polling was **removed, not reduced** ([ADR-0009
§7](adr/0009-event-sourced-core.md)).

## 9. Invariants and concurrency

| invariant | enforced by |
|---|---|
| Per-stream `version` is contiguous `1..n` | structure: unique `(stream_id, version)` on append |
| Global `seq` monotonic; gaps allowed | structure: append-only store assigns `seq` |
| Writes serialised on `version` | decider: append requires the expected version |
| Unique active **root** run per task | decider: checked in `decide`, not by an index |
| One `pending` approval per step | decider: reject a second request for the step |
| One active `compaction_summary` per run | decider: a new compaction supersedes it |
| No projection writes outside the projector | structure: the projector holds the write handle |
| Aggregate = snapshot + tail | structure: fold snapshot, then events after it |
| External facts cannot append unvalidated | structure: deciders are the only appenders |

**Optimistic concurrency.** The concurrency token is the stream version. To append, a decider
supplies the version it loaded; the append succeeds only if that is still the stream's current
version. Two commands racing on the same stream cannot both win: the loser appends nothing and
emits `CommandRejected` with `expected_version` and `actual_version`. This replaces the CRUD
design's DB compare-and-swap (`UPDATE … WHERE status = ?`) and moves "one active root run per
task" out of a partial unique index and into `decide`. The trade-off is explicit: the invariant
now lives in code that must be tested, not in a schema constraint that cannot be bypassed —
which is why the decider tests in §14 exist.

## 10. Projections and offsets

A projection is a read model. It is written **only** by the projector, which reads committed
events in `seq` order, folds them, and advances its offset in the same transaction where the
fold is safe to repeat. Offsets live in `projection_offsets(name, last_seq, rebuilt_at)`; the
difference between the log head and a projection's `last_seq` is the projection lag, and the
status line shows it because a stale offset and a UI bug look identical from outside.

| projection | purpose | primary source events | notes |
|---|---|---|---|
| `board` | columns, order, WIP, priority | Board*, TaskMoved | small; instant rebuild |
| `tasks` | the kanban cards | Task* events | card list; ordering renormalised on fold |
| `runs` | run status, totals, linkage | run life-cycle events | one row per run |
| `steps` | the audit/step rows | step events | includes compaction and subagent steps |
| `messages` | transcript | MessageAppended | `content_ref` resolved at read |
| `approvals` | pending/decided approvals | approval events | one pending per step |
| `decisions` | Jev calls, answers, outcomes | decision events | advisory; `applied`/`outcome` |
| `agents` | roster + trust + provenance | agent events | source, hash, effective policy |
| `skills` | attachable skills + trust + resource names | skill events | never a delegation target; `references/` contents not stored |
| `budgets` | day reservations and spend | budget/usage events | integer micro-USD |
| `schedules` | cron registrations, last fire | schedule events | dedup `(name, local_date)` |
| `deadlines` | the timer wheel's read model | deadline-arming events | earliest due is the arm |
| `run_costs` | per-step cost rollup | RunUsageRecorded, SubagentCompleted | sums to run totals |
| `search` | FTS over tasks and messages | Task*, MessageAppended | external-content FTS5 |
| `digests` | produced digest artefacts | DigestRequested/Completed/Failed | path + fallback flag |

Incremental apply is the normal path: a new event advances one offset. A **full rebuild** folds
the log from `seq = 0` (or from the oldest retained point, §11). Rebuild time is bounded by the
retention window: with the default 90 days and the default `snapshot_interval`, `mta projections
rebuild <name>` is expected to complete within a few seconds on a personal board; `mta
projections verify` re-derives a projection and diffs it against the live table, turning silent
corruption into a test failure.

## 11. Snapshots, replay, rebuild, GC, and retention

**Snapshots** are per-stream: the aggregate state at a stream version, taken every
`snapshot_interval` events (default below). Loading a stream folds the snapshot then the tail,
so cost is proportional to the tail, not to the whole stream.
`SnapshotTaken{stream_id,version,seq}` records each one on the `system` stream. Snapshots are an
optimisation and a post-prune authority (§11, retention), never the source of truth while the
events above them exist.

**Replay** is the operation that proves the log is authoritative:

```sh
mta replay --from-seq N --dry-run   # fold from N, print the diff, append nothing
mta replay --from-seq N             # apply after the diff is accepted
```

`--dry-run` diffs the replayed result against the live projections and **must** be identical; a
nonzero diff is a determinism bug, not something to force through. `mta projections
rebuild|verify` rebuilds or verifies a single projection. `mta projections list` shows every
offset and its lag.

**Retention and GC.** Events are the truth, but they are not kept forever by default:

```toml
[events]
retention_days = 90        # 0 = keep forever; pruning is opt-in and off when 0
snapshot_interval = 500    # events per stream between snapshots
rebuild_on_start = false   # verify offsets at startup; never rebuild silently
live_deltas = true         # ephemeral streaming channel (never part of the log)
```

Pruning is **opt-in** (it never runs when `retention_days = 0`) and always keeps the
`GcPerformed` record. GC only prunes events that are older than `retention_days`, below the
minimum `projection_offsets.last_seq`, **and** below each stream's last snapshot. The honest
statement of what that means:

> After pruning, the log is **authoritative above the watermark** (the pruned-before `seq`) and
> **snapshots are authoritative below it**. A stream can be rebuilt from its snapshot plus the
> remaining tail, but the individual events below the watermark are gone.

This tension with "the log is the truth" cannot be fully resolved — only bounded and documented.
Two rules bound it: a run's audit export is written **before** its stream's events are pruned,
so an exported run remains complete even after its events are gone; and pruning never touches a
stream whose audit export is missing or stale. (Whether that export is mandatory or advisory is
undecided; see §15.) `mta doctor --events` reports the watermark, the pruned counts, and any
stream that would be affected by the next GC pass.

## 12. Schema evolution and upcasting

Payloads are **add-only**. A field may be added; a field's meaning may **never** change in
place.

- Each event type carries `schema_version`, starting at 1. Adding an optional field does not
  require a version bump.
- When a field's meaning or shape changes, a new `schema_version` is published and an **upcaster**
  maps the old payload to the new one **on read during replay**. Old logs stay foldable; the
  upcaster is a pure function with its own tests (§14).
- A fold must tolerate unknown **added** fields: a payload with fields the current binary does not
  know about is read, ignoring the unknowns, never rejected.
- A fold must never guess at **changed** meaning. If a payload cannot be interpreted without
  assuming a new meaning, that is a missing upcaster, and reading fails loudly rather than
  silently misinterpreting.

A new event type is the other, preferred evolution: rather than widen an existing event's
meaning, add a type. One event type = one payload schema, forever.

## 13. Worked example

One task, one root run, one tool that needs approval, one compaction, and one delegation, in
`seq` order. `stream` shortcuts: `t1 = task:<t1>`, `r1 = run:<r1>`, `r2 = run:<r2>` (child).
Payloads are abbreviated to the fields that matter for the fold.

| seq | stream | event | key payload | projection effect |
|---|---|---|---|---|
| 1 | t1 | TaskCreated | column=todo, priority=2 | tasks += card; search index |
| 2 | t1 | TaskAssigned | agent_id=ops, mode=manual | tasks.assignee = ops |
| 3 | r1 | RunQueued | task_id=t1, trigger=manual | runs += queued row |
| 4 | r1 | RunBudgetReserved | reserved=500000, day=2026-10-21 | budgets += reservation |
| 5 | r1 | RunStarted | model_alias=smart | runs.status = running |
| 6 | r1 | LlmCallRequested | step_id=s1, est_tokens=12000 | steps += open llm step |
| 7 | r1 | MessageAppended | role=assistant, kind=message | messages += m1; search index |
| 8 | r1 | LlmCallCompleted | usage{in,out}, cost=340 | steps metrics; run_costs += |
| 9 | r1 | ToolCallRequested | tool=run_command | steps += open tool step |
| 10 | r1 | ApprovalRequested | step=s2, expires_at=… | approvals += pending |
| 11 | r1 | RunSuspended | step=s2, reason=action | runs.status = awaiting_approval |
| 12 | r1 | ApprovalDecided | decision=approved, by=user | approvals.approved |
| 13 | r1 | RunResumed | approval_id=a1 | runs.status = running |
| 14 | r1 | ToolCallCompleted | exit_code=0 | steps closed |
| 15 | r1 | CompactionPerformed | trigger=auto, 187K→24K | runs.compactions += 1 |
| 16 | r1 | DelegateRequested | child_run_id=r2, agent=reviewer | steps += open subagent step |
| 17 | r1 | SubagentStarted | child_run_id=r2 | runs += child link |
| 18 | r1 | SubagentCompleted | status=succeeded, cost=80 | steps closed; run_costs += |
| 19 | r1 | RunSucceeded | summary=…, artifacts[] | runs.status = succeeded |
| 20 | t1 | TaskCompleted | run_id=r1 | tasks.lane = done |

Resulting projection rows (excerpt):

| projection | resulting row (abbreviated) |
|---|---|
| `tasks` | `t1`: column=done, assignee=ops, result_summary=… |
| `runs` | `r1`: status=succeeded, compactions=1, cost=420 µUSD; `r2`: status=succeeded |
| `steps` | `s1` llm_call closed; `s2` tool_call closed; `s3` subagent_call closed (`r2`) |
| `messages` | `m1` assistant; `m2` compaction_summary (covered_from..through) |
| `approvals` | `a1` approved by user |
| `run_costs` | one row per step; sums to `runs.cost_micro_usd` |
| `budgets` | day 2026-10-21 reserved and spent updated |

## 14. Testing plan

Every property below is a test, not an aspiration. The fixture log is the golden record: it
replays to a known board, and any behaviour change must update the fixture deliberately.

| test | what it proves | mechanism |
|---|---|---|
| replay determinism | fold has no clock/random input | freeze the clock; assert identical runs |
| snapshot + tail == replay-from-zero | snapshots are an optimisation | fold two ways |
| projection rebuild is byte-identical | projections are derived | rebuild, then diff |
| command idempotency | same `command_id` twice ⇒ one event set | submit twice; assert one set |
| optimistic-concurrency conflict | a lost race appends nothing | race; assert `CommandRejected` |
| snapshot + tail under a mid-stream cut | tails load correctly | cut at each version |
| upcaster round-trip | old payloads stay foldable | upcast old → fold == new |
| crash mid-effect ⇒ re-issue once | at-least-once, no double append | drop result; restart |
| deadline determinism | firing is deterministic | advance a fake clock |
| UI needs no event to render | deltas are droppable | rebuild UI from projections |
| GC below the watermark | pruning is transparent | prune, rebuild, diff |
| golden fixture log | the system replays to a known board | `mta replay --dry-run` |
| idle scan appends nothing | discovery is diff-based, not a poll | submit `ScanCatalogs` twice unchanged; assert the second set is empty |
| attached skill text survives a file edit | prompt inputs are in the log, not on disk | attach, edit `SKILL.md`, replay; assert the run's prompt is byte-identical and `SkillChanged` is a separate stream fact |

## 15. Open questions

Decisions taken while writing this document are recorded first; genuinely undecided items follow.

### 15.1 Resolved here

| Question | Decision |
|---|---|
| `seq` gaplessness | **Gap-tolerant.** `seq` is monotonic and never reused; a rolled-back transaction may leave a hole, and a hole is not an error. `mta doctor --events` checks monotonicity and offset coverage, never contiguity of `seq`. Per-stream `version` *is* contiguous and is checked. |
| Audit export before pruning | **Mandatory.** `mta run export` is a hard precondition of pruning a run's events; `mta gc` refuses (and says which runs) rather than warning. |
| Command attribution for "requested" and start events | **The run loop commands in §6:** `BeginRun`, `AdvanceRun`, `RecordSubagentStart`, `RecordSubagentResult`. No event is orphaned. |
| Digest events' stream | **`digest:<id>`** (§5.8). The declared stream is in use now, and one digest is one replayable chain. |
| `DeadlineReached` stream | **`system`** (the timer wheel's own fact), with the *consequence* event (`ApprovalExpired`, `ScheduleFired`, `RunInterrupted`, …) on the owning stream. One fire, two streams, linked by `causation_id`. |
| Agent trust stream | **`agent:<id>`**, with the trust key `(project, source_hash)` in the payload. A rename or move is `AgentChanged` on the same stream; the trust projection re-evaluates by hash, so a rename alone does not re-prompt. |
| Are skills a stream of their own? | **Yes, `skill:<id>`** (§5.6), and skills are a **separate kind**, not agent rows. An agent defines a model and a tool policy; a skill defines neither, is attachable but never executable, and has its own trust life cycle. Folding skills into `agent:<id>` would make one stream mean two things and leave the kind unmodelled. |
| Where does an attached skill's body live? | **In the log**, on `RunStarted.attached_skills[]` (`{id, source_hash, body}`), projected to `runs.skill_snapshot_json` — the same rule as `agent_snapshot_json`, for the same reason: replay must not read a file whose content can change under the log. Attachment happens only at run creation, so no step can be replayed against different prompt text. |
| Who produces the discovery events? | **`ScanCatalogs`** (§6). The scan is an effect that submits a command; its decider appends only the diffs, so an idle watcher is a no-op. This closes the last "producer = bootstrap/scan" attribution gap. |
| `schedule:<id>` derivation | **A surrogate ULID.** `name` is unique and human-facing; renaming a schedule is `ScheduleDisabled` + `ScheduleRegistered` on the same stream (or an added `ScheduleRenamed` when it earns its keep), never a new stream. |
| `RunSuspended` vs `ApprovalRequested` | **Both stay.** `ApprovalRequested` is the request; `RunSuspended` is the run's state change, and the pair keeps the run state machine readable without joining to the approvals projection. They are appended by the same command. |
| Do host metrics (CPU/RAM/disk) belong in the log? | **No.** Samples are ephemeral (§2.1) because they are not reproducible and a log carrying them would replay differently on every machine. The single logged form is the opt-in, debounced `HostPressureObserved` **crossing**: a threshold being crossed is a durable, explainable fact, whereas a reading is a transient measurement. |

### 15.2 Still open

1. **`MessageAppended` granularity.** One event per message is fine for personal runs; if
   transcripts reach tens of thousands of rows per run, a batched append (one event covering a
   message range) may be needed.
2. **`search` (FTS) rebuild source.** Rebuild the FTS index from the raw events, or from the
   `messages` and `tasks` projections? Events are the purer source; the projections are faster and
   already normalised. (Working assumption: events, with the projections as an optimisation.)
3. **Snapshot policy per stream type.** `snapshot_interval = 500` events is one number for every
   stream; a long run stream may want a different bound than a task stream that lives for years.
4. **Projection snapshotting.** Whether to snapshot *projections* (not just aggregates) to cut
   cold-start replay on a very large log.
5. **Undo semantics.** Which events are compensatable (`TaskMoved`, `TaskEdited`, `TaskDeleted`)
   and which are not (anything with an external side effect). Deferred to post-v0.1.
