# Diagnostics: structured logs for debugging

`mta` has two records and only one of them is the truth:

| | Event log (durable) | Diagnostic log (ephemeral) |
|---|---|---|
| Purpose | the domain record: what happened to boards, tasks, runs | why it happened, how long it took, what it talked to |
| Storage | SQLite `events`, append-only, retained per `[events] retention_days` | rotating files under `$XDG_STATE_HOME/mta/logs` |
| Authority | **source of truth**; every projection rebuilds from it | **never** authoritative; loss is harmless |
| Replay | replayed and rebuilt by `mta replay` | never replayed, never parsed by the engine |
| Content | validated payloads with `schema_version`, ids, actors | free-form records with levels, targets, fields |
| Guarantee | every state change is here | *at most* once, dropped under pressure |

**The rule that follows:** if a fact must survive a restart, it is an event — never a log line. A
log
record may quote an event (`seq`, `command_id`, `causation_id`) but never substitutes for one. This
is
why there is no `LogEmitted` event and no `logs` table: making diagnostic output replayable would
put
every `hyper` and `sqlx` record in the audit trail and destroy its value.

Design rationale: [ADR-0011](adr/0011-observability-and-diagnostics.md).

## 1. Levels, and what belongs at each

| Level | Meaning | Examples |
|---|---|---|
| `error` | a human should look: a run failed, an effect exhausted its retries, a projection could not advance | `effect failed after 5 attempts`, `run failed: provider 400`, `panic` |
| `warn` | degraded but handled: retried, fell back, an invariant was restored | `retry 2/5 after 429`, `provider retry via fallback model`, `logs pruned to max_total_mb` |
| `info` | lifecycle a user could care about — low volume, default | `run started`, `run finished`, `approval requested`, `catalog scan: 12 agents, 3 skills`, `projection lag 0.4 s` |
| `debug` | per-step detail for debugging a specific run | request sizes, context-estimator inputs, task-estimate inputs, chosen tool, decide outcomes, span timings |
| `trace` | per-token / per-frame detail; off in the TUI by default | streaming deltas, frame timings, SQL statements |

Two disciplines make the levels useful rather than decorative:

1. **`info` is a budget, not a dumping ground.** A successful run emits a bounded number of `info`
   records (start, finished, and one per approval) — asserted by a test, because a log where `info`
   means "everything" makes `debug` meaningless on the day you need it.
2. **Nothing is logged per token at any level above `trace`.** Token deltas already travel on the
   ephemeral channel; a per-token log record is a self-inflicted disk problem (§9).
3. **Estimation is `debug`, not `info`.** An estimate changes whenever a card is edited or a run ends;
   logging each pass at `info` would make the default level depend on how much the user types. The
   `estimate` span carries the ids and the outcome for diagnosis, and the value itself is an event, not
   a log line.

`error` and `warn` are never sampled or rate-limited away; hot-path noise is suppressed at
`debug`/`trace`
with a dedup window instead (§4).

## 2. Sinks

Three sinks, each with a job:

| Sink | Where | Enabled when |
|---|---|---|
| Console | **stderr** (never stdout — ratatui paints stdout) | not in TUI mode |
| File | `$XDG_STATE_HOME/mta/logs/mta.log` (rotating) | default on |
| Ring | in-process, `[logging] ring_lines` records | always; feeds the TUI overlay (never persisted) |

**TUI safety is a hard rule.** While the alternate screen is active, nothing may write to the
terminal:
a stray record paints over the frame and a stray `println!` can leave the terminal in a state the
user
has to `reset`. So:

- entering the TUI **disables the console sink** through the same RAII guard that owns the alternate
  screen, bracketed paste, and mouse capture — so a panic cannot release the screen without also
  restoring the sinks;
- the file sink is **forced on** in TUI mode. `console = true, file = false` under `mta tui` is not
  a
  supported configuration (you would be debugging blind, and a crash would leave nothing behind):
  startup enables the file sink and says so once, at `warn`;
- library crates deny `clippy::print_stdout` and `clippy::print_stderr`, so the only way to emit
  diagnostics is the subscriber. The few intentional terminal writes (the TUI, `mta --version`,
  help)
  are `#[allow]`ed with a comment.

**Panic hook.** `ratatui::init()` already installs a hook that restores the terminal; `mta` wraps
it:
record the panic at `error` (message plus a backtrace when `RUST_BACKTRACE` is set), flush the file
sink, write a `crash-<unix-ts>.json` marker (version, resolved config hash, log path — no secrets,
no
buffer dump), then restore the terminal and re-raise. The next startup reports the marker once
("previous session ended abnormally at …; log: …") and deletes it. This is diagnostics only: the
domain
side of an interrupted run is `CrashRecovered` on the event log ([events.md](events.md) §5.1).

## 3. Configuration and precedence

```toml
[logging]
level = "info"              # error | warn | info | debug | trace | off
directives = []             # per-target overrides, e.g. ["mta_engine::effects=debug", "hyper=warn"]
format = "human"            # human | json
ansi = "auto"               # auto | always | never
console = true              # stderr sink (auto-disabled while the TUI owns the screen)
file = true                 # rotating file sink (forced on in TUI mode)
dir = ""                    # default $XDG_STATE_HOME/mta/logs
rotate = "daily"            # daily | hourly | never
retain_files = 7            # files kept per rotation series
max_file_mb = 16            # rotate early rather than grow past this
max_total_mb = 128          # hard cap: oldest files pruned beyond it
ring_lines = 2000           # in-process buffer for the TUI overlay (not persisted)
log_prompts = false         # include prompt/response text
log_tool_output = false     # include tool stdout/stderr bodies
max_field_bytes = 2048      # truncate any recorded field ("… (+1234 bytes)")
dedup_window_ms = 1000      # collapse identical repeated records into a count
```

**Precedence:** `--log-level <level>` > `RUST_LOG` > `[logging]` > `info`. `RUST_LOG` outranks the
config deliberately: it is the conventional ad-hoc debugging knob, and an explicit flag still beats
it.
`mta doctor --logs` prints the resolved level, directives, sinks, and the origin of each
(`"warn (from RUST_LOG)"`), the same way effective config values are reported elsewhere — a setting
you
cannot interrogate is a setting you cannot trust.

**The filter parser is ours.** `tracing-subscriber`'s `env-filter` feature is deliberately **not**
enabled: it pulls `regex-automata` + `matchers`, the largest single piece of a typical tracing
stack, for
a grammar that is `target=level` pairs. `mta` parses `directives` and `RUST_LOG` itself (longest
matching
target wins, then `level`, then `info`), which is ~60 lines, table-tested, and fails fast: an
unknown
level at startup is an error with exit code 2, not a silently ignored record. The `log` bridge is
enabled so dependency `log` records (which some crates still emit) are visible rather than dropped.

## 4. Correlation: every record can be joined to the log

Records carry the ids needed to answer "why is this card like this?" from the *other* direction —
given a
complaint about a run, `grep <run_id>` yields its whole causal chain.

Spans (nesting mirrors the architecture):

| Span | Fields | Wraps |
|---|---|---|
| `command` | `command_id`, `kind`, `stream_type`, `stream_id` | the write path: decide → append → publish → project → effect |
| `decide` | `stream_type`, `stream_id`, `version` | the pure decider call (recorded, never logged from — §7) |
| `effect` / `effect_attempt` | `effect_id`, `kind`, `attempt` | one effect, and each at-least-once attempt |
| `run` / `step` | `run_id`, `step_id`, `tool`, `msg_seq` | run and step lifetime |
| `provider` | `provider`, `model`, `stream` | one chat or decision request |
| `tool` | `name`, `argv_len`, `pid` | one tool invocation |
| `catalog_scan` | `scan_id`, `roots`, `kind` | one `ScanCatalogs` pass |
| `model_catalogue` | `provider`, `discovered`, `cached`, `duration_ms`, `outcome` | one live `GET /models` refresh (never per keystroke; no header or key field) |
| `estimate` | `task_id`, `source`, `estimator_version`, `samples`, `confidence` | one `EstimateTask` pass (heuristic, calibration, refinement) |
| `projection` | `name`, `from_seq`, `to_seq` | one projection advance |
| `deadline` | `deadline_id`, `kind`, `due_at`, `late_ms` | one `FireDeadline` → `DeadlineReached` |

Record fields, when in scope: `stream_type`, `stream_id`, `seq`, `command_id`, `correlation_id`,
`causation_id`, `run_id`, `task_id`, `provider`, `model`, `attempt`, `duration_ms`, `outcome`.
Estimation records add `estimate_minutes`, `estimate_source`, `estimator_version`, `samples`, and
`inputs_hash`; they never carry the task title or description (the `inputs_hash` is enough to join an
estimate to the card, and the task text stays in the event log where it belongs).
`json`
format emits these as structured fields so `jq` works; `human` prints them as a suffix.

Model-catalogue records carry the provider name, the number of ids returned, and whether the cache was
used; they never carry the request's `Authorization` header, an API key, or prompt text. Model ids are
not secrets. A picker config write is logged at `info` as `path`, `changed_keys` count, and `outcome`
only — never the file body, which may contain unrelated configuration.

## 5. Redaction: secrets never reach a record

1. **Secrets are a type, not a string.** Config holds `Secret(String)`; its
   `Debug`/`Display`/`Serialize`
   render `<redacted>`, so a derived `Debug` in a log record cannot leak one.
2. **Key indirection is logged by name.** Config stores `api_key_env`; the *name* is logged, the
   value
   never is, even at `trace`.
3. **Headers are never formatted.** `Authorization` and friends are excluded at the wire layer, not
   filtered afterwards by a pattern that might miss.
4. **Prompt and response text is off by default.** `log_prompts = false` records sizes and a hash,
   not
   content; `log_tool_output = false` records exit status and byte counts. Both are per-session
   opt-ins, and turning them on logs a `warn` naming the risk once.
5. **Transcripts shown in the UI get the same scrub** as anything written to disk, reusing the
   redactor
   §3.10 of the plan already requires.
6. **A property test enforces it.** For every configured secret value, at every level, and with
   `log_prompts`/`log_tool_output` both `true`, no captured record contains the value or a substring
   of
   it. A test that only greps the happy path would not have caught the leak this rule exists to
   prevent.

## 6. What `mta` never logs

- **API keys, tokens, `Authorization` headers, env secrets** (§5).
- **Host metrics at any level above `debug`**, and never as events. The status line and `S` overlay
  read
  the live sampler; a log line per sample is a disk problem with no debugging value
  ([ADR-0009](adr/0009-event-sourced-core.md) decision 8).
- **Token deltas at any level above `trace`.** They are ephemeral UI data
  ([events.md](events.md) §2.1).
- **Whole artifacts.** Digests, exports, and tool outputs are files; records quote paths, sizes, and
  hashes, truncated to `max_field_bytes`.
- **Anything at all from a decider** (§7).

## 7. Purity: deciders do not log

`decide(state, command) -> Result<Vec<Event>, Rejection>` is a pure function: no clock, no
randomness,
no I/O, no database, no network — enforced by a lint and a property test, because replay determinism
depends on it. A log call is I/O, so **a decider cannot log**, and this is structural rather than
conventional: `mta-domain` does not depend on `tracing` at all, and `cargo tree -p mta-domain` is
asserted to stay free of it.

Diagnostics therefore live on the outside of the purity boundary:

- the **command processor** records the command, and, on rejection, the returned reason — the
  decider's
  `Rejection` is a structured value, so the log gets a typed reason instead of a rule that tried to
  explain itself;
- **effects** record attempts, durations, and outcomes (they are allowed I/O);
- the `decide` **span** times the call. A span is metadata around a pure call, not a record inside
  it.

The payoff is that a mysterious decision can be re-derived from the log *and* the replay produces
the
same events — which is exactly what a log statement inside the rule would have made impossible to
prove.

## 8. Surfaces

**TUI**

- `O` opens the **diagnostics overlay**: the ring buffer's last `ring_lines` records, with level and
  target filters, `/` search, `f` follow, `j`/`k` (and the wheel,
  [ADR-0008](adr/0008-mouse-hit-map.md)),
  `Enter` to expand a record's fields, and `y` to copy the record (ids included) to the clipboard.
  It is scrollable history, not the event log: the `E` overlay shows *events* and stays the audit
  view.
- The status line shows `warn`/`error` counts with a dot; `O` jumps to the first error of the
  session.
- `:log <level>` changes the level at runtime (no restart) through the subscriber's reload handle,
  so you
  can turn `debug` on for one reproduction and off again. The overlay header shows the effective
  level
  and, when relaxed, a `debug (this session)` badge, so nobody wonders why a run is suddenly chatty.
- Records are never the source of a UI value: the run panel, board, and costs all read projections.

**CLI**

```
mta --log-level <level>            # any subcommand; also accepts --log-format human|json
mta doctor --logs [--follow]       # resolved level/directives/sinks, paths, sizes, last records
mta doctor --bundle [--out f.tgz]  # redacted support bundle (§10)
```

**What to look at in order.** (1) The card and its run panel. (2) The `E` overlay for the event
chain.
(3) `O` for what the code was doing while those events were appended. (4) `mta doctor --logs` when
the
TUI itself is the suspect, and `mta doctor --bundle` when handing a problem to someone else.

## 9. Rotation and disk discipline

Diagnostics are bounded by policy for the same reason `target/` is ([build.md](build.md)): an
unattended orchestrator that logs a chatty provider at `debug` must not be able to fill the disk.

- Rotation is `daily` (default) or `hourly`, with `retain_files` per series, plus `max_file_mb`
  early
  rotation and a hard `max_total_mb` cap; the oldest files are pruned first, and the prune itself is
  a
  `warn` with counts.
- `mta doctor --logs` reports the directory, file count, total size, oldest record, and how much of
  `max_total_mb` is in use.
- Pruning is silent-safe: losing diagnostic history never affects a run, a projection, or the audit
  trail (contrast `[events]`, where pruning requires a frozen export first — §3.10 of the plan).
- The write path is lossy on purpose (a bounded channel drops records under pressure, counted and
  reported as a `warn`), because blocking the engine on a log write would make diagnostics a source
  of
  failure.

## 10. The support bundle

`mta doctor --bundle` writes a redacted archive so a problem can be handed over without a screen
recording: `mta --version`, rustc and OS/kernel versions, resolved config with secrets redacted and
the
config hash, resolved `[logging]`, `doctor` check results, `doctor --host` snapshot, event-log head
`seq` and per-stream counts, `doctor --events` output, and the last N records per level from the log
tail. It **never** includes API keys, prompt text (unless `log_prompts` is on, in which case the
bundle
says so), the SQLite database, artifacts, or workspace contents. For the *domain* record of one run
the
right artifact remains `mta run export <id>` (§3.10 of the plan) — the bundle explains the software,
the
export explains the run.

## 11. Tests and lints

| Area | Check |
|---|---|
| Levels | a full successful fixture run emits ≤ N `info` records; `info` never appears per token or per frame |
| Redaction | the no-secret property test of §5, across all levels, with prompts and tool output enabled |
| TUI safety | with the alternate screen active, zero bytes reach stdout and stderr; a simulated panic restores sinks, terminal, and writes the crash marker |
| Correlation | records for a fixture run carry `run_id`/`seq`/`command_id` that join to the event log; one `effect_attempt` record per attempt |
| Purity | `cargo tree -p mta-domain` contains no `tracing`; the existing decider purity lint passes |
| Rotation | a synthetic flood respects `max_file_mb`, `retain_files`, and `max_total_mb` in a temp dir, with a prune `warn` |
| Filter parser | table tests for `directives` + `RUST_LOG`, longest-target-wins, unknown level → exit 2 |
| Format | `json` output parses and contains the documented field set; `ansi = "never"` emits no escapes |
| Lints | `clippy::print_stdout`/`print_stderr` denied in library crates; `dbg!`/`todo!` denied workspace-wide |

`mta-testkit` is a **dev-only** crate providing `capture::logs(level) -> Capture` (an in-memory
subscriber plus the accumulated records) for these tests. It is never shipped and is excluded from
`size-budget.toml`, which budgets the `mta` binary.

## 12. Non-goals and backlog

**Not in v0.1:** remote/OTel export and log shipping (nothing leaves the machine by default);
metrics/Prometheus endpoints; a per-run log file; log-driven alerting; a log viewer window beyond
the
overlay; `env-filter`-style full directive grammar (regex filters, span filters).

**Backlog:** per-run log files or a `run_id`-scoped file sink; `tracing-opentelemetry` export behind
a
feature when the v0.2 daemon/worker split lands (the crate choice keeps that door open); log-level
changes as events so a debug session is itself auditable; a `--log-include-prompts` guard that
requires
an explicit per-session approval dialog rather than a config flag.
