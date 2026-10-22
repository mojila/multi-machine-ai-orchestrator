# AGENTS.md — development rules for `mta`

This repository builds `mta`, a terminal-first AI agent orchestrator: a kanban board whose cards are
executed by configurable agents, on top of an append-only event log. It is currently **planning stage**:
there is no Rust code yet. The documents in `docs/` are the specification; code is written to match them,
not the other way round.

These rules apply to humans and to AI coding agents alike. If a rule blocks a task, change the rule in a
PR with an ADR — do not silently work around it.

## 1. Read before writing

| If you are touching… | Read first |
|---|---|
| almost anything | [docs/PLAN.md](docs/PLAN.md) §2–§3, [docs/README.md](docs/README.md) |
| state, persistence, scheduling, timers | [docs/events.md](docs/events.md), [ADR-0009](docs/adr/0009-event-sourced-core.md) |
| agents, skills, `.agents/` discovery | [docs/agents.md](docs/agents.md), [ADR-0006](docs/adr/0006-opencode-subagents.md), [ADR-0010](docs/adr/0010-agent-and-skill-discovery.md) |
| estimates, card meta, forecast overlay | [PLAN.md §3.3](docs/PLAN.md#33-domain-model-and-state-machines), [ADR-0012](docs/adr/0012-task-estimation.md), [events.md §5.1](docs/events.md#51-board-and-task-boardid-taskid) |
| logging, diagnostics, panic handling | [docs/observability.md](docs/observability.md), [ADR-0011](docs/adr/0011-observability-and-diagnostics.md) |
| dependencies, MSRV, build size | [docs/build.md](docs/build.md), [ADR-0002](docs/adr/0002-version-pinning.md), `size-budget.toml` |

Do not create a second design document for something the plan already covers. Update the plan and, if a
decision reverses, add an ADR under `docs/adr/NNNN-slug.md` using the template.

## 2. The event-sourcing contract is not optional

1. **The log is the source of truth.** Persisted state changes are validated commands that append events.
   There are no row writes outside the projector, no "just update the table" shortcuts, and no shadow
   audit table.
2. **Deciders are pure.** `decide` may not read the clock, generate randomness, do I/O, query the
   database, log, or spawn tasks. Time, ids, and external facts arrive in the command payload and are
   recorded in the event.
3. **Expected version on every append.** A stale version is rejected (`CommandRejected`); it never
   clobbers. A repeated `command_id` is a no-op, not a second event.
4. **Effects subscribe to events and report back as commands.** An effect never appends directly. Derive
   its idempotency key from the originating event (`hash(event_id, effect_name)`), so a crash-and-retry
   produces exactly one result.
5. **No polling.** Anything periodic is a deadline on the timer wheel, fired as `FireDeadline`. No
   `sleep` loop, no sweeper, no `notify`-plus-poll.
6. **Payloads are add-only.** Adding an optional field is compatible; changing a field's meaning requires
   a new `schema_version` and an upcaster that runs on read during replay. A fold tolerates unknown
   added fields and fails loudly rather than guessing a changed meaning.
7. **Ephemeral state stays out of the log.** Streaming token deltas, host CPU/RAM/disk samples, and
   diagnostic log records are never events. The one exception the design allows is the opt-in,
   debounced `HostPressureObserved` crossing.
8. **Vocabulary is canonical.** Events are `PascalCase`, past tense, no `Event` suffix. Streams are
   `board:`, `task:`, `run:`, `agent:`, `skill:`, `schedule:`, `digest:`, `system`. The envelope is
   `seq`, `event_id`, `stream_type`, `stream_id`, `type`, `version`, `schema_version`, `at`, `actor`,
   `correlation_id`, `causation_id`, `payload_json`.

When you add an event, command, effect, projection, or deadline, update **both** `docs/events.md` and the
code in the same change. `docs/events.md` is the interface other crates code against.

## 3. Estimation rules (ADR-0012)

- Estimates are **events**, not render-time math: `TaskEstimated`, `TaskEstimateCleared`,
  `TaskEffortRecorded` on `task:<id>`. The value is stored in the event; replay never runs the estimator.
- The heuristic is a **pure function in `mta-domain`** over card fields plus a calibration bucket. No
  network, no model, no clock, no randomness.
- `inputs_hash` is the idempotency key for automatic estimates. An unchanged card appends nothing; do
  not add a timer or a "recalculate every N seconds" path.
- Effort (human minutes) and run forecast (agent duration/cost) are separate quantities and separate
  calibration sources. Never infer human effort from elapsed wall-clock time.
- Calibration constants (EWMA alpha, ratio clamp, bucket mapping) live in **code**, not config, so a
  config edit cannot change a projection rebuild. A formula change bumps `estimator_version`.
- A `source = user` estimate is sticky. Only an explicit `SetTaskEstimate`, `ClearTaskEstimate`, or
  `EstimateTask { force_auto: true }` replaces it.
- Board and column totals are **leaf-only** (a parent and its subtasks are counted once) and unestimated
  cards contribute `?`, never `0`. An estimate never gates a run, a lane move, or a budget reservation.

## 4. Purity, dependencies, and MSRV

- `mta-domain` must not depend on `tracing`, `sqlx`, `reqwest`, `tokio`, or any I/O crate. It is the
  deterministic core. The CI check is `cargo tree -p mta-domain` plus a review gate.
- No `println!`/`eprintln!` in library crates. Diagnostics go through the logging facade described in
  `docs/observability.md`; the TUI owns the screen and writes nothing to stdout/stderr.
- **Exact pins, committed lockfile, MSRV 1.94.** Do not bump `rust-version`, `rust-toolchain.toml`, or a
  pinned crate without a verified reason (crates.io metadata, not a guess) and an ADR-0002 update.
  Pre-1.0 crates are pinned exactly because a patch release can break the build.
- A new dependency needs: a stated job, an MSRV check, a feature-set review (trim defaults), and a note
  in `docs/build.md` plus `size-budget.toml` if it affects the shipped binary. Prefer no dependency: the
  estimation, filter parser, and diagnostic filter are deliberately hand-written.
- Run `scripts/size-report.sh --check` before proposing a dependency or profile change. Never raise a
  budget silently; record why in the PR and, if it is structural, in an ADR.

## 5. Discovery, skills, and untrusted files

- Agent globs are **narrow**: `.opencode/agents/**/*.md`, `.agents/agents/**/*.md`, `.agents/*.md`, and
  the legacy `.opencode/agent/**`. Never use `.agents/**/*.md` — that registers a skill's
  `references/*.md` corpus as agents.
- A skill directory is a black box except `SKILL.md` and optional `agents/openai.yaml|yml`.
  `references/`, `assets/`, `scripts/`, and `evals/` are recorded by name and never read.
- Project-scoped files are **untrusted input**. Trust is bound to the content hash, checked before first
  delegation (agents) or attach (skills); an untrusted or hash-mismatched file cannot run.
- Agent permissions may only **restrict** the global policy, never widen it. A malformed file is a
  warning, never a startup failure.
- `skill` is a separate kind: listed with provenance, attachable as context, and never selectable as the
  executor; `delegate` must refuse a skill id.

**Config writes are a single, checked path.** The TUI and CLI never rewrite TOML directly. A model
selection, binding, or new `[[models]]` profile goes through the engine's `ConfigEditor`:
comment-preserving `toml_edit`, validate before write, atomic temp+rename, one `.bak`, and a hash check
that aborts if the file changed since the picker opened; a failed `ReloadConfig` restores the backup.
A decider must never write a file, and a config write must never include a secret. A selection is a
model-only `[agents.model_bindings]` entry — never a sparse `[[agents.definitions]]` entry, which would
freeze the rest of the definition ([ADR-0013](docs/adr/0013-model-selection-and-search.md)).

## 6. Diagnostics and secrets

- Events answer "what happened"; logs answer "what was it doing". Do not mix them: no `LogEmitted` event,
  no `logs` table, no log line as the only record of a state change.
- Log at `info` sparingly (lifecycle only), `debug` for per-step detail, `trace` for per-token/frame.
  Never log per token above `trace`. Estimation passes are `debug`.
- Never log a secret, an `Authorization` header, or a prompt/tool body unless the corresponding
  `log_prompts` / `log_tool_output` opt-in is on. Use the `Secret` newtype; log `api_key_env`, not the
  value. A support bundle must survive the redaction audit.
- The TUI installs its panic hook **before** entering the alternate screen, and the RAII guard restores
  cursor, paste, and mouse capture on normal exit, panic, and SIGTERM.

## 7. Verification before you call it done

Run what exists; do not claim a check you did not run. As the workspace lands, the gate is:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo audit
cargo tree -d                      # fail on duplicate terminal crates
scripts/size-report.sh --check
cargo insta test --check           # snapshots; INSTA_UPDATE=no in CI
```

Plus the targeted tests for the area you touched:

- event/command change → replay determinism, per-stream version contiguity, command idempotency, and a
  projection rebuild equality test;
- discovery change → the black-box skill fixture (one skill, zero agents) and the precedence matrix;
- estimation change → golden heuristic tables, `inputs_hash` no-op, leaf-only totals, and a
  replay-with-estimator-disabled test;
- TUI change → snapshots at 40×15, 80×24, 120×40, 200×50, plus key/pointer parity and `NO_COLOR`.

If a check cannot run in this environment, say so explicitly in the PR or hand-off; never mark it green.

## 8. Change hygiene

- Conventional commits: `feat(scope): …`, `fix(scope): …`, `docs: …`, `chore: …`, `test(scope): …`.
  One logical change per commit/PR.
- A PR that changes behaviour updates the plan, `events.md`, and the relevant ADR **in the same PR**.
  A PR that only adds documentation still links the issue and the milestone.
- Keep `.agents/` out of git (it is gitignored and pinned by `skills-lock.json`); never commit local
  skills, agent files, or a database.
- Keep secrets and personal data out of docs, fixtures, commit messages, and screenshots.
- Do not invent scope: multi-machine transport, a daemon, and OpenCode runtime parity are explicitly
  deferred (PLAN §1.2/§9). If a task seems to need them, stop and raise it.

## 9. Definition of done

A change is done when the code matches the documented contract, the contract documents are updated, the
relevant tests and the CI gate pass, the size budget is respected, and the hand-off states exactly what
was verified — and what was not.
