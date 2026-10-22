# ADR-0011: Structured diagnostics with `tracing`, kept separate from the event log

- **Status:** Proposed
- **Date:** 2026-10-21
- **Deciders:** project owner
- **Related:** [docs/observability.md](../observability.md), [docs/events.md](../events.md),
  [ADR-0002 version pinning](0002-version-pinning.md), [ADR-0005 build size
  budgets](0005-build-size-budgets.md),
  [ADR-0009 event-sourced core](0009-event-sourced-core.md),
  [PLAN.md §3.10](../PLAN.md#310-security-and-observability)

## Context

ADR-0009 made the append-only `events` log the source of truth, and PLAN.md §3.10 builds the audit
trail
on it: `mta events tail|show`, `mta decisions`, `mta run export`, and projections that can be
rebuilt. The
plan then had no answer at all for the *other* question — the one asked while something is going
wrong:

- **The event log is not a debugger.** It records what the domain decided, not how long a request
  took,
  which of five retry attempts got a 429, what the estimator thought the prompt weighed, or which
  subprocess consumed a step's budget. It cannot: those are non-deterministic observations, and
  determinism is exactly what makes the log replayable (ADR-0009 decision 5). Writing them as events
  would
  put `hyper`/`sqlx` chatter into the audit trail and make the only unbounded table grow on ambient
  noise.
- **There was no level, sink, format, or precedence story.** PLAN.md §3.9 ended with two loose lines
  ("never log headers or message content at `info`"; "logs under `$XDG_STATE_HOME/mta/logs` via
  `tracing-appender`") that named a crate but specified nothing, and `tracing` was otherwise
  mentioned
  only in passing in build.md.
- **A TUI makes logging a correctness problem, not a preference.** Anything written to stdout or
  stderr
  while the alternate screen is active paints over the frame — and a stray `println!` in a retry
  path is
  invisible in tests and obvious only in a demo.
- **The purity rule cuts the other way.** Deciders are clock/RNG/IO-free so replay is deterministic
  (PLAN.md §6, ADR-0009). A log call is I/O. "Add logging" therefore has to say where it is allowed
  to
  happen, not just which crate to pull in.
- **Secrets are in scope.** Providers are configured with API keys, and prompts and tool output can
  carry
  credentials and private data. A logging facility that can leak a key is worse than none.

Options considered:

1. **`log` + `env_logger`** — the smallest possible dependency. No spans, so no request/attempt
   correlation without hand-threading ids; `env_logger`'s own filter machinery pulls a regex engine;
   the
   `log` facade has no notion of structured fields, which is the thing that makes a log joinable to
   the
   event log; and the TUI still needs a bespoke file sink and an in-process buffer.
2. **`tracing` + `tracing-subscriber`** — spans with fields, structured records, a `log` bridge for
   dependencies that still use `log`, per-target filtering, JSON or human output, and a reload
   handle for
   runtime level changes.
3. **Hand-rolled writer** — full control of format and cost. Rejected: it re-implements level
   filtering,
   target matching, rotation, and the async-safe writer, and the first thing it would get wrong is
   the
   thing `tracing` is tested for (records from many tasks interleaving without corrupting the file).
4. **`tracing-opentelemetry` / remote export** — real distributed tracing, out of scope for a
   single-process v0.1, and it inverts the local-first posture (nothing leaves the machine by
   default).

## Decision

Adopt option 2, with the boundaries below. The full contract lives in
[docs/observability.md](../observability.md); this record fixes the choices that are expensive to
reverse.

1. **Two records, one truth.** Diagnostic logs are ephemeral, non-authoritative, and never replayed.
   The
   engine never reads them; loss is harmless. There is **no** `LogEmitted` event and **no** `logs`
   table.
   The rule downstream code depends on: *if a fact must survive a restart, it is an event, not a log
   line.*
2. **Pinned exactly, MSRV checked against 1.94** (ADR-0002's standing rule):
   `tracing = "=0.1.44"` (MSRV 1.65), `tracing-subscriber = "=0.3.23"` with
   `default-features = false` and features `std`, `fmt`, `ansi`, `json`, `tracing-log` (MSRV 1.65),
   and
   `tracing-appender = "=0.2.5"` (MSRV 1.63). All three sit well under the workspace MSRV, so no
   toolchain
   bump is implied.
3. **`env-filter` is deliberately not enabled.** It drags in `regex-automata` + `matchers` — the
   largest
   single component of a typical `tracing` stack — for a grammar that is `target=level` pairs. `mta`
   parses
   `[logging] directives` and `RUST_LOG` itself (longest matching target, then the global level),
   which is
   small, table-tested, and can fail fast on an unknown level instead of silently dropping records.
4. **Levels are a contract, not decoration.** `error` = someone must act, `warn` = degraded but
   handled,
   `info` = user-visible lifecycle and bounded per run, `debug` = per-step detail, `trace` =
   per-token /
   per-frame. Nothing logs per token above `trace`; `error`/`warn` are never sampled away.
5. **Two sinks, plus an in-process ring.** Console (stderr only — never stdout) and a rotating file
   sink
   under `$XDG_STATE_HOME/mta/logs`, both configured by `[logging]`; plus a ring buffer feeding the
   TUI
   overlay, which is presentation-only and never persisted. Rotation honours `retain_files`,
   `max_file_mb`, and a hard `max_total_mb` cap, so an unattended `debug` session cannot fill the
   disk —
   the same posture ADR-0005 takes for `target/`.
6. **The TUI owns the terminal, so it silences the console sink.** Entering the TUI disables the
   console
   sink through the same RAII guard that owns the alternate screen, bracketed paste, and mouse
   capture, and
   forces the file sink on (debugging a TUI with no file sink is not a supported configuration).
   Library
   crates deny `clippy::print_stdout`/`print_stderr`, and the panic hook writes a `crash-<ts>.json`
   marker
   before restoring the terminal, so a crash leaves evidence instead of a blank screen.
7. **Deciders cannot log, structurally.** `mta-domain` does not depend on `tracing` at all; the
   command
   processor and effects record the decider's structured `Rejection` and the surrounding `decide`
   span. A
   pure call stays pure, and the log cannot become a second, non-replayable source of behaviour.
8. **Secrets are a type and a test.** `Secret` redacts on `Debug`/`Display`/`Serialize`; keys are
   logged by
   `api_key_env` *name*; `Authorization` headers are never formatted; prompt text and tool output
   are off
   by default (`log_prompts`, `log_tool_output`). A property test asserts no configured secret
   appears in
   any captured record at any level, including with both content opt-ins enabled.
9. **Every record can be joined to the event log.** Spans (`command`, `decide`, `effect`,
   `effect_attempt`, `run`, `step`, `provider`, `tool`, `catalog_scan`, `projection`, `deadline`)
   carry
   `command_id`/`stream_id`/`seq`/`run_id`/`task_id`, so "why is this card like this?" is answerable
   by
   grepping the log for an id that came out of `events`.
10. **Diagnostics are local.** No remote export, no shipping, no telemetry; `mta doctor --logs`
    reports the
    resolved settings, and `mta doctor --bundle` produces a redacted support archive. The crate
    choice
    keeps an OTel export possible later without changing call sites.

## Consequences

**Positive:** the two questions "what happened?" (events) and "what was it doing?" (logs) get
different,
honest answers, and neither compromises the other. Debugging a live run becomes a `run_id` grep
across a
file that has request timings, retry attempts, and estimator inputs — information the log
deliberately
cannot hold. Runtime level changes (`:log debug`) mean an expensive level costs one reproduction
instead
of a restart. The `42000`-style terminal corruption class of bug is closed by construction rather
than by
discipline: sinks and screen share one guard, and the library crates cannot print at all. Exact pins
plus
verified MSRVs keep the ADR-0002 rule non-negotiable, and the pins were checked (1.65/1.65/1.63),
not
assumed — which is the failure mode that produced the `sysinfo` pin in the first place.

**Negative / costs:** a second logging vocabulary to learn alongside the event log, and the
discipline to
keep them apart — the failure mode is a developer reaching for `info!` where an event is the right
answer,
so the rule is stated in observability.md §1 and enforced by the audit tests. The release binary
grows by
the `tracing` stack; the delta is measured and booked in M0 rather than guessed, and the biggest
optional
component (`env-filter`) is already excluded. The no-`env-filter` decision means writing and
maintaining a
small parser, and it means `RUST_LOG` supports the subset `mta` documents rather than the whole
upstream
grammar. Prompts being off by default makes some reproductions require a deliberate opt-in.

**Follow-ups:** per-run or `run_id`-scoped log files; OTel export behind a feature once the v0.2
daemon/worker split exists; recording level changes as events so a debug session is itself
auditable; a
prompt-inclusion path that requires a per-session approval rather than a config flag; decide whether
the
`dedup_window_ms` collapse should also apply to repeated `debug` records from the same span.
