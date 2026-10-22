# Implementation Plan — `mta`: AI Agent Orchestrator with a Kanban TUI

**Status:** Draft for review · **Date:** 2026-10-21 · **Target:** v0.1 (local, single user)
**Document:** `docs/PLAN.md` · **Docs index:** [README.md](README.md)
**Stack:** Rust 2024, MSRV **1.94** · ratatui 0.30.2 · crossterm 0.29 · tokio 1.54 · SQLite (sqlx 0.9,
bundled) · reqwest 0.13 (rustls) · tracing 0.1.44 (diagnostics, [observability.md](observability.md))
**Working name:** `mta` (binary), repo `multi-machine-ai-orchestrator`

---

## 1. Problem and goals

Knowledge workers accumulate small, well-scoped tasks (draft a doc, triage a mailbox, summarise a
report, prepare a deck, chase a follow-up). Today those tasks live in a board, but the *doing* is
manual. The goal is a terminal-first tool where a board of ordinary employee tasks can be handed to
configurable AI agents, executed with visible steps and human approval gates, and reviewed as work
products — not as a chat window.

### 1.1 Goals

| # | Goal | Measurable outcome |
|---|---|---|
| G1 | Kanban board in the terminal | 5-column board, 200+ cards, navigation and editing with keyboard or mouse |
| G2 | Any OpenAI-compatible provider | `base_url` + key + model in config; works against OpenAI, OpenRouter, Groq, Ollama/vLLM/LM Studio without code changes |
| G3 | Agent execution, not chat | Each card can be assigned to an agent; a run produces a transcript, artifacts, and a result summary |
| G4 | Human-in-the-loop | Configurable approval gates; agents cannot mark work done on their own |
| G5 | Day-to-day orchestration | Priority/WIP limits, dependencies, scheduling, auto-assignment, standup digest, per-day cost cap |
| G6 | Safe and auditable | Every LLM call, tool call, and decision persisted; workspace-scoped file access; secrets never stored in the DB |
| G7 | Fast, robust TUI | <16 ms frame at 200 cards; terminal restored on panic/exit (incl. mouse capture); keyboard and pointer parity; no blocking I/O on the UI thread |
| G8 | Jev as a decision provider | Configure a System One (Jev) provider; agents and engine policies ask typed `noul`/`choice`/`score` questions; every answer is persisted; decisions are advisory by default |
| G9 | Reuse existing OpenCode agent rosters and `.agents` skills | Open `mta` in a project with `.opencode/agents/*.md` or `.agents/`; those agents appear as delegation targets, run under `mta`'s budgets/approvals/audit, and their permission rules can only restrict (never widen) `mta` policy; `.agents/skills/*/SKILL.md` appear as attachable context, never as an executor |
| G10 | Event-sourced core: the log explains the state | Every persisted change is a validated command appended to an append-only event log; every view is a projection that can be dropped and rebuilt to the same bytes; no scheduling, approval, or expiry logic depends on a polling cadence |
| G11 | Debuggable by construction | Every command, effect attempt, provider call, decision, and deadline carries ids that join it to the event log, into a rotating log under `$XDG_STATE_HOME/mta/logs`; secrets and prompt text never reach a record (a property test proves it); the TUI never writes to the terminal; `mta doctor --logs` reports the resolved level, directives, and sinks |
| G12 | Plan with estimates, not guesses | Every card carries an automatic effort estimate with provenance and confidence; the board shows per-column and overall totals plus a projected finish; estimates are event-sourced, reproducible, manually overridable, and calibrated from recorded actuals — never from a silent mutable field |
| G13 | Choose models by search, not by memorised aliases | A picker searches configured and live-discovered models with filters, sets an agent's default or a one-run override, and requires a profile before an unconfigured model can run; a selection survives restart in config and is visible in `agents show` |

### 1.2 Non-goals for v0.1

- Multi-user / team collaboration, accounts, permissions.
- Web or mobile UI.
- **Multi-machine execution** (despite the repo name): v0.1 is a single local process. The internal
  boundaries (engine trait, run protocol, append-only events) are designed so a daemon + remote
  workers can be added in v0.2 without rewriting the domain (see §9).
- Email/calendar/Jira/Slack integrations. These arrive as tool packs later.
- Fine-tuning, RAG over a document corpus, embeddings.
- Jev image (`box`) questions and image assets: v0.1 sends text/JSON state only; the wire types are
  designed so `box` can be added without changing the decision API.
- Acting as an MCP client. Kenari's MCP server also exposes Jev, but v0.1 calls the HTTP API directly
  so the provider is configurable like any other; MCP tool packs are v0.2.
- Windows as a first-class target (crossterm should work; CI coverage comes after M4).
- **OpenCode runtime parity:** `mta` parses OpenCode agent definitions and `.agents`-style skills
  ([agents.md](agents.md)) but does not execute OpenCode, load its plugins, MCP servers, or LSP; skills are
  attachable prompt text, never an executed workflow, and agents defined only inside `opencode.json(c)` are
  not read in v0.1.

### 1.3 Assumptions (confirm before M1)

1. **Task domain is generic knowledge work**, not only software. Agent behaviour is driven by
   per-agent system prompts + a tool allowlist; a "software" template pack is one example, not the core.
2. Default autonomy is **supervised**: reads are free, writes stay inside the run workspace, shell and
   outbound side effects require approval.
3. Single user, single machine, local SQLite; the TUI embeds the orchestrator.
4. A "day" is the local timezone day; budgets reset at local midnight.
5. API keys are available via environment variables or an OS keyring; the tool never *requires* a
   key for local providers.
6. **Jev is a decision model, not a chatbot.** It answers typed questions about a supplied state and
   cannot stream text or call tools. Access is via kenari (`https://kenari.id/v1`, `kn-` key, free
   `jev-1-13:free`) or any endpoint implementing the same System One contract.
7. Jev's answers **inform** the human and the engine; unless a policy explicitly applies a decision
   (e.g. headless auto-assign above a confidence threshold), they never authorise a side effect.

### 1.4 Success metrics for the first internal release

- Create, assign, run, review, and close a task without leaving the TUI.
- A failing/looping agent is always stoppable within 2 s and leaves a readable transcript.
- Provider swap requires editing config only; new runs pick it up without a rebuild (the running
  engine reloads valid config on file change or `:reload`; in-flight runs keep their resolved model).
- Zero known paths where a tool writes outside its workspace without an approval record.
- Every Jev decision is reproducible from the stored request/response, shows confidence and model
  version, and low-confidence answers produce a human prompt rather than an action.
- Every open card carries an effort estimate or an explicit `?`; the header, column subtotals, and
  `mta estimate` agree, a manual estimate survives an edit and a projection rebuild, and a projected
  finish is shown only when the recorded history supports it.

---

## 2. Product definition

### 2.1 Core concepts

| Concept | Meaning |
|---|---|
| **Board** | A workflow with ordered columns. v0.1 ships one board, but the schema supports many. |
| **Column** | Ordered lane with a `kind` — `backlog`, `todo`, `doing`, `review`, `done` — that drives automation. Names are user-editable. |
| **Task** | Unit of work: title, description, priority, due date, tags, assignee, dependencies, parent. |
| **Estimate** | A card's expected **effort** (human minutes, the planning number) plus an optional **run forecast** (wall-clock and cost for the assigned agent). Derived automatically, with `source`, `confidence`, and an `inputs_hash`; a manual override is just another `TaskEstimated` with `source = user`. Event-sourced, never a mutable side column. |
| **Agent** | Named persona: system prompt, chat model alias, optional decision model alias, tool allowlist/policy, budgets, tags used for auto-assignment. Definitions come from `mta` config, the DB, or OpenCode/`.agents` Markdown files. |
| **Model profile** | A `[[models]]` entry: alias, provider, model id, context window, max output, pricing, capabilities, params. The profile is the only thing a run can resolve; a live `/models` result that has no profile is discoverable but not selectable until one is added. |
| **Model binding** | A model-only override for an agent: `[agents.model_bindings].<agent-name> = "<alias>"`. It cannot change a prompt, tool policy, or budget, so it is safe to apply to a file-discovered agent without triggering whole-definition replace. Precedence: per-run override > binding > definition `model` > inherit. |
| **Agent source** | Provenance of a definition — `config`, `db`, `opencode-global`, `opencode-project`, `agents-global`, `agents-project` — with path, content hash, and (for project files) a trust record. |
| **Skill** | A `.agents/skills/<name>/SKILL.md` workflow: name, description, and instruction text, plus optional present-only `references/`. A **separate catalogue kind** — attachable to a run as extra context, never selectable as the agent that executes it (no model, no permissions, no tools). |
| **Sub-agent** | A child run of the same task, launched by an agent through the `delegate` tool: fresh context, own model/policy/budget/transcript, depth-limited, no extra board card. |
| **Run** | One execution of a task by an agent. Has steps, token/cost accounting, status, result. |
| **Step** | `llm_call`, `decision_call`, `tool_call`, `subagent_call`, `compaction`, `approval`, `note` — the audit unit inside a run. |
| **Artifact** | A file produced in the run workspace and registered against the task (path + hash). |
| **Approval** | A pending human decision that blocks a run (destructive tool, spend, "finish"). |
| **Decision** | One typed question set asked of a decision model (Jev/System One) about a `state`, plus its answers. Advisory unless a configured policy applies it. |
| **Decision provider** | A provider whose wire protocol is System One (`POST /v1/systemone`), not OpenAI chat. Used for `noul` (confidence), `choice`, and `score` questions. |
| **Event** | Append-only domain event for the activity feed, digests, and future multi-machine fan-out. |

### 2.2 Primary user flows

1. **Morning triage** — open TUI, see board + "today" digest of overnight runs; move cards from
   Backlog to Todo; set priorities.
2. **Delegate** — open a card, press `a`, pick an agent from a fuzzy list (Jev's suggested agents and
   confidence appear first when the decision provider is enabled); optionally attach a `.agents` skill as
   extra context from the same dialog and choose a one-run **model** override in the searchable picker
   (`M`, or the model row); press `r` (or move to Doing with
   auto-run on) to start a run; watch live status on the card. When the project defines OpenCode or
   `.agents` agents, the assigned agent may delegate parts of the work to them (`delegate` tool → child run).
3. **Supervise** — open the run panel: streaming assistant text, tool calls with args/result excerpts,
   typed Jev decisions (`q → answer`, confidence), nested sub-agent runs, token/cost counters. Approve or
   deny gated actions inline, including child-run and first-use trust approvals.
4. **Review** — successful runs move the card to Review with a summary and artifacts. The human
   accepts (→ Done, with optional comment) or sends back (→ Todo with feedback appended to the task,
   which becomes context for the next run).
5. **Standup** — `mta digest` (or a scheduled run) produces a Markdown summary of yesterday's
   completions, blockers, spend, and pending approvals.

### 2.3 TUI specification

```
┌ mta · board:work ─── 2 running · $0.84 today · 1 approval · Σ 38h (~7d) ─── ? help ┐
│ BACKLOG 4 · 10h  │ TODO 3 · 6.5h │ IN PROGRESS 2 · 5h │ REVIEW 1 · 1h │ DONE 12    │
│ ┌──────────────┐ │ ┌───────────┐ │ ┌───────────────┐ │ ┌───────────┐ │ ┌────────┐ │
│ │#51 Vendor    │ │ │#44 Q3 deck│ │ │▸#42 Inbox     │ │ │#39 Notes  │ │ │#31 ... │ │
│ │    renewal   │ │ │    outline│ │ │   triage      │ │ │   cleanup │ │ │        │ │
│ │    ~3h high  │ │ │    ~2h    │ │ │   ~1h @ops    │ │ │   ~1h     │ │ │        │ │
│ └──────────────┘ │ └───────────┘ │ └───────────────┘ │ └───────────┘ │ └────────┘ │
│ ┌──────────────┐ │               │                   │               │            │
│ │#50 ...       │ │               │                   │               │            │
├───────────────────────────────────────────────────────────────────────────────────┤
│ #42 Inbox triage · Doing · agent:ops · run 7c1e · 62% · $0.11                     │
│ est 1h (learned, 0.72) · run ~4m / $0.03 · board 38h open · 5.5h/day → ~7d        │
│ Latest: classified 34 messages; 3 need your decision → approval pending            │
│ [enter] open [a] assign [r] run [m] move [n] new [/] filter [T] forecast [^p] palette │
└──────────────────────────────────────────────── engine ok · 21:04 ─────────────────┘
```

**Layout rules**

- Header (1 line): board name, running count, daily spend, pending approvals, the **remaining-effort
  total** (`Σ 38h (~7d)`), and the help hint. The total counts Backlog, Todo, and Doing only — never Done
  or Review, whose effort is already spent — counts **leaf cards only** so a parent and its subtasks are
  not double-counted, and appends `?n` when `n` open cards have no estimate. With an active filter it
  reads `Σ 12h/38h` (visible/total). Below 100 columns it collapses
  to `Σ 38h`; below 80 the whole total is dropped and `T` is the only place to see it.
- Board (flex): columns equal-width; horizontal scroll when the terminal is narrower than
  `columns × 24`; on `< 60` cols fall back to a single-column list with a lane switcher; each column
  header shows its card count and **effort subtotal** (`TODO 3 · 6.5h`, hidden under 100 cols; Review and
  Done show spent effort rather than remaining work);
  each column shows a scrollbar when its cards overflow.
- Detail strip (3–4 lines): selected card context + estimate line + latest run line + contextual key
  hints; the hints are pointer targets (clicking one runs that hint's action). The estimate line reads
  `est 1h (learned, 0.72) · run ~4m / $0.03 · board 38h open · 5.5h/day → ~7d` and degrades to
  `est 1h (heuristic)` or `est ?` — it never invents a forecast when history is missing.
- Status line (1 line): engine health (chat and decision providers), clock, host readout, projection
  lag and applied event `seq`, background job count, and a diagnostic `warn`/`error` badge (`O` jumps to
  the first error of the session). The host readout is compact
  (`cpu 19% mem 43% disk 34%`, coloured at the warn thresholds); below 100 columns it collapses to a
  single worst-metric glyph, and below 80 columns it is dropped entirely — the `S` overlay always has
  the full picture.
- Overlays: card detail/editor, run transcript (scrollable), decision log, event log, diagnostics log,
  forecast/estimates, system monitor, command palette, agent/skill picker, **model picker and model
  profile form**, help, approval dialog, confirm dialog. One overlay
  at a time; `Esc` always closes; unsaved edits prompt; clicking outside closes a non-destructive overlay
  but never a confirm dialog. The event log (`E`) is the audit view; the diagnostics log (`O`) is the
  ephemeral debug view — a different record with a different lifetime
  ([observability.md](observability.md) §1).

**Keyboard model** (vim-flavoured, arrows always work)

| Key | Action |
|---|---|
| `h`/`l`, `←`/`→` | Previous/next column |
| `j`/`k`, `↓`/`↑` | Next/previous card |
| `H`/`L` or `m` | Move card to adjacent column (guarded by state machine) |
| `J`/`K` | Reorder within column |
| `1..5` | Jump to column |
| `Enter` | Open card detail |
| `n` / `e` / `d` | New / edit / delete card (delete requires confirm + typed title) |
| `a` | Assign agent (fuzzy picker) |
| `A` | Catalogue roster overlay: agents and skills grouped by kind, mode, model, source, trust state, warnings, effective permissions |
| `r` / `x` | Run (or re-run) / cancel run |
| `v` | Open run transcript |
| `C` | Force context compaction now (run panel; shows tokens before/after) |
| `D` | Open the decision log for the selected card/run (typed Q&A, confidence, model) |
| `E` | Open the event log overlay (recent events; `Enter` expands the causation chain) |
| `S` | Open the system monitor overlay (CPU per core + load + temp, RAM/swap, disks per mount, top processes) |
| `T` | Open the forecast overlay: open-effort totals by column/tag/agent, throughput, projected finish, run-time/cost forecast, calibration table, unestimated cards |
| `M` | Open the model catalogue: fuzzy search + filters over configured and live-discovered models; set an agent default (binding) or a one-run override, or add a profile for an unconfigured model |
| `O` | Open the diagnostics log overlay (level/target filters, search, follow; `:log <level>` retunes live) |
| `y` | Copy task ID / run ID (context-dependent) |
| `/` | Filter: `#tag`, `@agent`, `is:blocked`, `due:<date>`, free text (FTS) |
| `Ctrl-P` | Command palette (`:move done`, `:new`, `:agent add`, `:board …`) |
| `g`/`G` | Top/bottom |
| `?` | Help (keymap table, generated from one source of truth) |
| `q` / `Ctrl-C` | Quit (confirm if runs are active) |

In the card editor, the `estimate` field accepts `2h`/`90m` to pin, `-` to clear, `a` to force an
automatic re-estimate, and blank to keep automatic control. These are field values, not extra keys, so
editing a title is unchanged.

**Mouse model** (`[ui].mouse`, default on; every gesture maps to an existing key action — there are no
mouse-only features)

| Gesture | Over | Action |
|---|---|---|
| left click | card | Select the card and focus its column (same as `j`/`k` + `Enter` selection) |
| double click | card | Open card detail (same as `Enter`) |
| left click | column header | Focus that column |
| left click | a detail-strip hint (`[enter] open`, `[r] run`) | Run that hint's action |
| left click | status-line host readout | Open the system monitor (same as `S`) |
| wheel up/down | board | Scroll the focused column; in the single-column fallback, move between columns |
| `Shift`+wheel | board | Scroll horizontally |
| wheel | run panel / overlay | Scroll the transcript or list by `[ui].scroll_lines` (default 3) |
| click | scrollbar track / thumb | Page-jump / start a drag |
| drag | scrollbar thumb | Scroll continuously (clamped at both ends, never overscrolls) |
| click | palette row | Execute that command, with the palette's own confirmation |
| click / double click | model-picker row | Select / choose the model (same as `Enter`); clicking the search field focuses it |
| click | picker filter chip / `[refresh]` hint | Apply that filter / force a discovery refresh (same as `Ctrl-R`) |
| click | approval `[approve]` / `[deny]` | The same guarded action; destructive approvals still open the confirm dialog |
| click | outside a modal | `Esc` (close) — except confirm dialogs, which ignore outside clicks |
| hover | any region | Nothing, unless `[ui].hover = true`: then only a colour highlight |

Pointer rules:

- A click dispatches the **same `Action`** the documented key would. Policy checks, confirmations, typed
  confirmations, and approval requirements are never bypassed by pointer input — a click can never
  approve a destructive action that a keypress could not.
- Mouse regions are recomputed every frame; a click is applied only when its coordinates still fall
  inside the recorded rect **and** the entity it referenced still exists. Stale clicks after a resize or
  board change are dropped, never guessed.
- Terminals report motion events very frequently; `Moved` is ignored unless `hover = true`, and hover
  only recolours a region — it never changes selection or scroll offset.
- Text selection: capturing the mouse breaks plain drag-select in most terminals. Use `Shift`+drag (the
  terminal normally passes it through), copy with `y` / OSC-52, or set `[ui].mouse = false` for a
  pure-keyboard session and native selection. This is a config choice, not a per-run toggle.

The hit-map architecture, the capture-lifecycle invariant, and the alternatives considered are recorded
in [ADR-0008](adr/0008-mouse-hit-map.md).

**Rendering/UX constraints**

- Never colour-only: blocked/overdue/running are also marked with a glyph. Estimates use a prefix, not a
  colour: `~2h` automatic, `=2h` pinned by the user, `?` not estimated; low confidence adds a `·?`
  suffix rather than changing the hue.
- Aggregate estimates are **honest about their inputs**: a finish projection appears only when there is
  enough throughput history (`[estimation].throughput_min_days`, default 3), otherwise the overlay says
  `insufficient history`; an unestimated card contributes `?`, never `0`.
- Respect `NO_COLOR`; theme via config (16-colour default; 256/truecolor accents).
- All async work is off the render path; render is a pure `view(&AppState, frame)`.
- Long titles truncate with `…` at a measured width (`unicode-width`, wide CJK safe).

### 2.4 CLI surface

```
mta                     # launch TUI (embeds engine)
mta tui --board work
mta task add "Draft Q3 deck" --column todo --agent writer --due 2026-10-25 --estimate 2h
mta task list|move|assign|show
mta task estimate <id> [2h|--auto|--clear]   # pin, force a re-estimate, or clear one card
mta task actual <id> 3h                      # record actual effort (the calibration input)
mta estimate [--board] [--json]              # open-effort totals, per column/tag/agent, projected finish
mta estimate recompute [--task <id>|--all]   # bounded, idempotent re-estimate (no background sweep)
mta run <task-id> [--follow]        # headless single run
mta run <task-id> --model <alias>   # one-run model override (not persisted; validated against a profile)
mta run <task-id> --skills <id,...>   # attach .agents skills as extra context (never as the executor)
mta run <task-id> --compact=auto|off [--compact-at N]   # per-run context policy override
mta runs compact <id>               # force a compaction at the next safe point
mta run --scheduler                 # cron/scheduler mode, no TUI
mta digest [--since 24h] [--out -]
mta agents list [--all] [--json] [--source <root>] [--kind agent|skill]   # one catalogue view
mta agents show <id>                # provenance, model resolution, effective permissions, warnings
mta agents validate                 # scan every root + both kinds; non-zero on errors (CI)
mta agents trust <id>               # headless trust for the current project (prints file hash)
mta agents add|edit                 # mta-managed agents (config/DB), unrelated to discovery
mta skills list [--all] [--json]    # convenience: `agents list --kind skill`
mta skills show <id>                # description, body size, present resources, interface, trust state
mta skills trust <id>               # headless trust for a project skill (prints file hash)
mta models list [--modality systemone] [--context]   # from configured providers (GET /models)
mta models list --search "ctx>=100k tools @openai" [--all] [--json] [--refresh]
                                     # same query grammar as the TUI picker (ADR-0013)
mta models show <alias>              # profile, resolution, availability, shadowing
mta models add <provider/model> [--alias <a>] [--context N] [--tools] [--json-mode]
                                    # append a [[models]] profile (atomic config edit + ReloadConfig)
mta models set|unset <agent> [alias] # write/remove an [agents.model_bindings] entry
mta models refresh [--provider <name>]   # one-shot discovery; never a background poll
mta decide --model jev --state-file ctx.json --questions q.json   # debug one typed decision
mta decisions [--task <id>] [--run <id>]  # audit log of decisions and outcomes
mta events tail [--follow] [--kind <type>] [--stream <id>]   # the log itself, as it is written
mta events show <event-id>          # payload + correlation and causation chain (why this happened)
mta replay --from-seq <n> [--dry-run]        # replay/diff the log against the current projections
mta projections list|verify|rebuild [--name <p>]   # read-model health and rebuild
mta snapshot take|list              # per-stream snapshots that bound replay
mta run export <id> [--out -]       # frozen audit export (required before its stream is pruned)
mta doctor [--provider <name>] [--context] [--events] [--host] [--estimate] [--logs [--follow]] [--bundle]
                                                     # health, lag, effects, host, calibration, diagnostics, bundle
mta config init|validate|path
```

Global flags (before or after the subcommand): `--log-level <error|warn|info|debug|trace|off>` and
`--log-format <human|json>`, plus `--config` / `--db`. Precedence is
`--log-level` > `RUST_LOG` > `[logging]` > `info`; `mta doctor --logs` prints the resolved value *and*
where it came from ([observability.md](observability.md) §3).

`mta tui` and `mta run --scheduler` both write to the same SQLite file, but **v0.1 refuses to run
both at once**: the engine holds a **lease** (projected from `LockAcquired`/`LockReleased` events) and
refuses to start if a live lease exists. The TUI owns the engine; a future daemon mode removes this
constraint (§9). Short-lived **control commands** (`task …`, `approval approve|deny`, `mta runs
compact`) are allowed while a lease is held: they append a **command** to the durable command log and
wake the engine, which validates and applies it as an event. When no engine is running, the CLI takes
the lease itself, processes the command, and releases it — so the same command path serves both cases.
There is no row-writing side channel, no `notify` watch, and no pending-approval poll; a control command
can never start or mutate a run directly.

---

## 3. Architecture

### 3.1 Runtime shape

```
┌───────────────────────────── one process ──────────────────────────────┐
│  ratatui/crossterm ─► Action ─► AppState ─► view(&AppState) → HitMap    │
│        ▲                            ▲                                   │
│        │ InputEvent                 │ DomainEvent stream (resume @seq)   │
│  ┌─────┴────────────────────────────┴────────────────────────────────┐  │
│  │ Engine (tokio tasks)                                              │  │
│  │  CommandProcessor ─► decide(state, cmd) ─► append (expect version) │  │
│  │        ▲                                        │                  │  │
│  │        │ result commands                        ▼                  │  │
│  │  EffectRunners ◄── subscribe ── EventBus ──► Projectors ──► read    │  │
│  │   (HTTP · fs · subprocess)                     models (SQLite)      │  │
│  │  Scheduler (event-reactive) ─► TimerWheel ─► FireDeadline          │  │
│  └───────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

- **The log is the source of truth.** `events` is append-only and totally ordered by `seq`; every other
  table (`tasks`, `runs`, `steps`, `messages`, `approvals`, `decisions`, `agents`, `budgets`, …) is a
  **projection** with a row in `projection_offsets`. Nothing writes a projection except its projector;
  any projection can be dropped and rebuilt from the log. Contract and catalogue:
  [events.md](events.md); rationale: [ADR-0009](adr/0009-event-sourced-core.md).
- **Single writer, optimistic concurrency.** The engine owns the log and a **one-connection write pool**
  (separate read-only pool for queries). Appends are serialised and carry the **expected per-stream
  `version`**, which replaces compare-and-swap (`UPDATE … WHERE status = ?`); a mismatch rejects the
  command with `CommandRejected` and appends nothing. Foreign keys, CAS, and the "one active root run
  per task" index are gone from the write path — that invariant is now checked in the decider.
- **The UI never touches the DB:** it is a projection client. It subscribes to the event stream from its
  own `last_seq` and folds events into the view model; a cold start reads a read-model snapshot, and
  `broadcast::Lagged` is handled by **replaying from the durable log from `last_seq`** rather than
  discarding state and guessing. This keeps SQLite contention trivial and makes a future daemon split a
  transport swap.
- **No blocking in render:** DB/HTTP/FS all live in the engine (as effects); the UI task selects on
  terminal input, the event stream, a **presentation-only repaint interval** for relative times and
  spinners, and a `watch` channel of **host samples** from a small sampler task (`[host].sample_ms`).
  Neither the repaint interval nor the sampler mutates domain state or is read by the engine; the sampler
  reads `/proc`-equivalent host data and never the log (§3.8).
- **Deadlines, not sweeps.** Timer-driven work (approval expiry, retry backoff, run timeouts, cron
  schedules) is armed by the event that creates it and fired by one timer wheel; approval expiry and the
  startup reconciliation sweep are gone, replaced by `DeadlineReached` and `CrashRecovered`.
- **Cancellation:** one `CancellationToken` per run, hierarchical (run → tool → HTTP request);
  cancellation is a command that produces `RunCancelled`, so an aborted run is as auditable as any other.
- **Crash safety:** panic hook + RAII guard restore the terminal; every append is one transaction;
  recovery is replay from the last projection offset, re-issuing effects that have no result event
  (idempotent via `command_id`) and resolving any deadline that passed while the process was dead.

### 3.2 Workspace layout

```
Cargo.toml                     # [workspace], resolver = "3", shared lints, size-aware profiles
size-budget.toml               # MiB budgets for target/ and the release binary (docs/build.md)
.gitignore                     # target/, artifacts, editor files
scripts/                       # size-report.sh, prune-target.sh
crates/
  mta-domain/                  # pure types + state machines + event enum, no I/O, no tracing
    src/{task,board,agent,skill,run,estimate,event,command,ids,error}.rs
                                  # estimate.rs: pure heuristic + calibration bucket (ADR-0012)
  mta-store/                   # sqlx + migrations, event log, projections, snapshots, replay
    migrations/0001_init.sql …
  mta-provider/                # OpenAI-compatible chat + System One (Jev) decisions
    src/{openai,stream,systemone,models,error,capabilities}.rs
  mta-engine/                  # command processor, effects, scheduler, executor, agent loop, tools
    src/effects/               # CallChatModel, ExecuteTool, EstimateTask, … (subscribe → command)
    src/agents/                # agent discovery (.opencode + .agents agents), parsing, merging, trust
    src/skills/                # .agents/skills discovery (SKILL.md only), trust, attachment assembly
  mta-tui/                     # app state, update(), widgets, keymap, host sampler, snapshots
    src/host.rs                # HostMetrics trait + sysinfo impl + sampler task (ephemeral, §3.8)
    src/forecast.rs            # BoardVm estimate subtotals + the T overlay view model (ADR-0012)
    src/diagnostics.rs         # log ring buffer (O overlay); presentation-only, never persisted
  mta-cli/                     # bin `mta`: clap, wiring, config loading, logging init, headless modes
    src/logging/               # subscriber setup, sinks, filter parser, panic hook, crash marker (§3.10)
  mta-testkit/                 # dev-only (never shipped, outside size-budget.toml): log capture for tests
docs/
  README.md                    # documentation index and conventions
  PLAN.md                      # this implementation plan
  events.md                    # event-sourcing contract: events, commands, effects, projections
  providers.md                 # tested providers + known quirks
  agents.md                    # OpenCode agent compatibility + sub-agent/delegation contract
  observability.md             # diagnostics: levels, sinks, redaction, spans, rotation
  build.md                     # bounded target/ and binary-size policy
  adr/0001-workspace.md …      # decisions recorded as they are made
README.md                      # project entry point → docs/
```

`mta-testkit` is a workspace member that is **never shipped**: it exists so tests can assert on log
records (level policy, redaction, correlation ids) without a subscriber per test file. Because it is a
dev-dependency everywhere it is used, it is excluded from `size-budget.toml` — the budget that matters is
the `mta` binary ([build.md](build.md)).

A workspace (rather than one crate) is deliberate: the future multi-machine split needs
`domain`, `provider`, and `engine` usable from a daemon and a remote worker; it also makes compile
times and dependency hygiene better. No crate may depend "upward": `domain ← store ← engine ← cli`,
`provider` depends on `domain` only, `tui` depends on `domain` only (view models are domain types) and
never on `engine` internals or `store` rows.

**Version pinning:** `rust-version = "1.94"` in the workspace (sqlx 0.9 requires it) and a
`rust-toolchain.toml` with `channel = "1.94"`, not `"stable"` — otherwise CI on an older image fails
late. `Cargo.lock` is committed (this is a binary). ratatui 0.30 is a facade over pre-1.0,
semver-unstable crates (`ratatui-core 0.1`, `ratatui-widgets 0.3`, `ratatui-crossterm 0.1`); pin
`ratatui = "=0.30.2"` and `ratatui-textarea = "=0.9.3"`, and rely on `cargo tree -d` CI to catch
duplicate crossterm/ratatui-core. sqlx needs explicit `runtime-tokio` (defaults have **no** runtime);
`sqlite` pulls bundled SQLite with FTS5 enabled. reqwest needs `json` + `stream` (for
`bytes_stream()`); default TLS is rustls/aws-lc-rs, which affects cross-build toolchains, and the
default `system-proxy` feature must be **disabled explicitly** so egress cannot silently bypass the
SSRF/allowlist policy (§3.10). `sysinfo` is pinned `=0.38.4`, not `=0.39.x`: 0.38.4 declares MSRV
1.88 and fits the workspace, while 0.39.3+ declares **1.95** and would force an MSRV bump for a
presentation-only widget. Taking the newer crate is a deliberate trigger to raise `rust-version`
and `rust-toolchain.toml` together; until then the pin holds and `cargo tree -d` guards duplication.

### 3.3 Domain model and state machines

**Identifiers:** UUIDv7 as TEXT (sortable, no coordination needed later). **Timestamps:** INTEGER
Unix milliseconds UTC; formatting at the edge. **Version column** on tasks for optimistic concurrency
(future remote workers, and cheap conflict detection if two runs update a card).

**Model resolution.** A run has exactly one resolved chat model, chosen at `StartRun`/`BeginRun` in this
order: the run's `model_alias` override (from the run dialog or `mta run --model`, never persisted) →
`[agents.model_bindings].<agent-name>` → the agent definition's `model` → the parent/root run's model
when `[agents].inherit_model = true` → unresolved (the run cannot start). A binding is model-only and
never merges a prompt, tool policy, or budget; that is what makes it safe for file-discovered agents
([ADR-0013](adr/0013-model-selection-and-search.md)). `RunQueued.model_source` records the winning layer
(`override | binding | definition | inherit`) so a run can explain itself, and `RunStarted` snapshots the
resolved alias. An alias that has no `[[models]]` profile — including a live-discovered id the user has
not profiled — is rejected; the picker opens the profile form instead of starting a run with an invented
context window or price.

**Column kind drives automation:**

| Transition | Guard |
|---|---|
| any → `backlog` | always allowed by user |
| backlog → todo | user only |
| todo → doing | agent assigned OR auto-assign possible; dependencies satisfied; run slot available; lane WIP respected when `enforce_wip` |
| doing → review | engine on successful run, or user |
| doing → review | engine on final failure when `review_on_failure`, with `blocked_reason = "run_failed"` |
| review → done | **user only** (v0.1); requires no pending approval |
| review → todo | user only; feedback appended to task context |
| other lanes → done | only via explicit palette force-move (`:move done --force`) |
| doing → todo | allowed on cancel/failure, with reason |

`blocked` is a **flag** (`blocked_reason`, `blocked_at`), not a lane: a card can be blocked while in
Todo or Doing; the UI shows a glyph and the filter `is:blocked` catches it. This avoids a sixth
column that everyone has to maintain.

**Run state machine**

`queued`, `running`, `awaiting_approval`, and terminal: `succeeded`, `failed`, `cancelled`,
`budget_exhausted`, `blocked`, `interrupted`.

```
queued ─────────────► running ─────────────► succeeded
  │                     │  ▲                  failed              (error after retries, or denied)
  └──► cancelled        │  │                  cancelled           (user, incl. mid-approval)
                        │  └── approval ok    budget_exhausted    (steps/tokens/$/wall clock)
                        ├──► awaiting_approval ──► failed         (denied or expired)
                        ├──► blocked            (needs human decision; terminal for this run)
                        └──► interrupted        (engine restart; from CrashRecovered)
```

- Every state change is a **command validated in `decide` and appended with the run stream's expected
  `version`**; a version mismatch rejects the command (`CommandRejected`) instead of clobbering. The
  resulting event is durable before it is published, so the UI can always be reconstructed from the log.
- `blocked` and `interrupted` are **terminal for that run**: resuming means a new run stream with
  `trigger = retry`, carrying the previous run id, not resurrecting an old one. This keeps the audit trail
  honest and makes the state graph acyclic.
- Exactly **one non-terminal root run per task** is an invariant of the `StartRun` decider (checked
  against the `runs` projection inside the append transaction), not an index on a mutable table. Sub-agent
  runs carry `parent_run_id` and may be active alongside their parent; their limits are per-parent
  (§3.6.1), not per-task.
- Approval decisions are explicit: `approved → running`, `denied → failed (error='denied')`,
  `expired → failed (error='approval_expired')`. `expired` lives on `approvals.status`, not on the run.
- `awaiting_approval` is the **only** suspension point. `ask_human` creates a question-type approval;
  a destructive tool creates an action-type approval; `blocked` is the run-level outcome of
  "cannot proceed safely" (loop detected, missing credentials, policy refusal). A sub-agent can suspend
  on its own approval while the parent's `delegate` step stays open; the parent is *waiting*, not
  suspended, so this does not add a second suspension state.
- The **task** has an independent `blocked_reason`/`blocked_at` flag (lane-independent). A blocked run
  sets it; the human clears it when resuming.
- On startup the engine appends `EngineStarted` and then `CrashRecovered`, which replays from the last
  projection offset, marks any run with no live process as `RunInterrupted`, expires orphaned approvals
  via their deadlines, and re-issues effects with no result event (implemented in M4). The UI offers
  "retry" from the card.

**Step model:** assistant text lives in `messages` (appended by `MessageAppended`); each model call is a
`steps` projection row of kind `llm_call` carrying the request/response metadata and its own token/cost
columns; each Jev call is a `decision_call` step carrying the typed request/answers; each tool invocation
is a `tool_call` row. Payloads are truncated to a configurable byte cap, with the full payload in the run
workspace when large and referenced by `result_ref`. Run totals are a fold over the run stream
(`RunUsageRecorded`) and are therefore always consistent with the log; the `run_costs` projection
materialises them for fast queries.

**Estimation model.** Every task carries one **effort** number and an optional **run forecast**:

| Field | Meaning | Unit / rule |
|---|---|---|
| `estimate_minutes` | expected human effort for the card | integer minutes, rounded to 15; clamped to `[15, 480]` by default |
| `estimate_source` | who produced the current value | `heuristic` (no calibration), `learned` (a calibrated bucket was applied — the formula is still the heuristic), `decision` (Jev-refined), `user` (pinned) |
| `estimate_confidence` | how much to trust it | `0..=1`; the heuristic starts at 0.35 (max 0.80 with signals), a calibrated bucket adds, a Jev answer uses its normalised confidence, a user pin is 1.0 |
| `run_forecast` | expected run wall-clock + cost for the assigned agent/model | `duration_ms`, `cost_micro_usd`; absent without an assignee or history |
| `inputs_hash` | hash of the fields the estimate was computed from | makes an unchanged card a **no-op**, not a new event |
| `estimator_version` | integer bumped when the formula or feature set changes | a replay never recomputes; it reads the stored value |

The estimate is **an event** (`TaskEstimated`, [events.md §5.1](events.md#51-board-and-task-boardid-taskid)),
not a side column: the decider validates it, the projection materialises it on `tasks`, and the TUI fold
maintains its own totals. A rebuild of `tasks` from the log therefore reproduces every estimate exactly,
including after the heuristic changes — old events keep their `estimator_version`, and a formula change
applies only to newly estimated cards or an explicit `mta estimate recompute`.

**Effort is human effort, not agent time.** They are deliberately separate: a 2-hour writing task can be
a 4-minute agent run. Calibration therefore has two independent sources:

- **Effort** calibrates from `TaskEffortRecorded` actuals (`mta task actual`, or the accept dialog).
  Without actuals the heuristic stands and confidence stays low.
- **Run forecast** calibrates from completed runs' `duration_ms` and `cost_micro_usd`, grouped by
  `(agent, model, tag)`, so it learns how *this* agent performs this *kind* of task.

Totals are **leaf-only**: a card with subtasks rolls up `Σ children + 10% coordination` for display but is
excluded from board/column totals, so parent and child are never counted twice. The header's remaining
total counts Backlog, Todo, and Doing; Review and Done show spent effort in their column subtotals.
`?` cards contribute nothing to the sum and are counted separately; they are never treated as zero.

A manual estimate (`source = user`) is sticky: an automatic re-estimate never overwrites it, and only
`--auto`/`a` or `--clear` returns the card to automatic control. Estimates are advisory planning aids:
they never block a start, never change a lane, and never size a budget reservation. They are not on the
critical path of the scheduler.

**Why a heuristic first.** The deterministic estimator is a pure function in `mta-domain` over the card's
fields (`title`, `description` length, tags, priority, dependency count, parent/children) plus the
calibration bucket. It needs no network, no model, and no randomness, which keeps `decide` total and
replay deterministic. Jev is an optional **refinement** for exactly the cases the heuristic cannot judge
(a long, vague card with an unknown tag): one `score` question over five size buckets, recorded with
`purpose = "task_estimate"` on the task stream. The heuristic answer is written first and refined later,
so the card is never blank while a decision call is in flight; a disabled, unavailable, or under-confident
Jev changes nothing and costs nothing. The formula and its bucket table live in
[ADR-0012](adr/0012-task-estimation.md).

### 3.4 Persistence (SQLite)

Location: `$XDG_DATA_HOME/mta/mta.db` (`directories` crate), overridable by `--db` or `MTA_DB`.
Connection options, applied **per connection** via `SqliteConnectOptions` (pragmas are not global):
`journal_mode=WAL`, `foreign_keys=ON`, `busy_timeout=5000`, `synchronous=NORMAL`. Writes use a
one-connection pool; reads use a separate read-only pool. Migrations embedded with
`sqlx::migrate!`; forward-only, tested from empty DB on every CI run.

**Two classes of table.** The log tables are infrastructure: `events` is the source of truth, and
`commands`, `snapshots`, `projection_offsets`, `deadlines`, and `effects` are what make it replayable,
observable, and crash-safe. Everything after them — boards, columns, agents, skills, tasks, runs, messages,
steps, approvals, decisions, artifacts, search — is a **read model (projection)**: derived from the log,
never written except by its projector, and rebuildable at any time with `mta projections rebuild`.

```sql
-- ── Source of truth and infrastructure (writable) ───────────────────────────────────────────────
CREATE TABLE events (                     -- append-only; never UPDATEd or DELETEd except by `mta gc`
  seq INTEGER PRIMARY KEY AUTOINCREMENT,  -- global total order; gaps are legal after a rolled-back tx
  event_id TEXT NOT NULL UNIQUE,          -- ULID/UUIDv7: stable across machines and replays
  stream_type TEXT NOT NULL,              -- board|task|run|agent|schedule|digest|system
  stream_id TEXT NOT NULL,                -- e.g. 'task:01J…'
  type TEXT NOT NULL,                     -- TaskCreated, LlmCallCompleted, … (docs/events.md)
  version INTEGER NOT NULL,               -- per-stream, contiguous: optimistic concurrency
  schema_version INTEGER NOT NULL DEFAULT 1,
  at INTEGER NOT NULL,                    -- wall clock, decided by the command, never by the fold
  actor TEXT NOT NULL,                    -- user:<name> | agent:<id> | engine | schedule:<id>
  correlation_id TEXT NOT NULL,           -- the command that caused this chain
  causation_id TEXT,                      -- the event (or command) that directly caused this one
  payload_json TEXT NOT NULL,
  UNIQUE (stream_id, version)
);
CREATE INDEX events_stream ON events(stream_id, version);
CREATE INDEX events_type ON events(type, seq);
CREATE INDEX events_correlation ON events(correlation_id);
CREATE INDEX events_causation ON events(causation_id);

CREATE TABLE commands (                   -- durable command log: the write API and the recovery queue
  command_id TEXT PRIMARY KEY,            -- idempotency key: a retried effect cannot double-append
  type TEXT NOT NULL,                     -- AddTask, RecordLlmResult, FireDeadline, …
  at INTEGER NOT NULL, actor TEXT NOT NULL,
  target_stream TEXT NOT NULL,
  expected_version INTEGER,               -- optimistic concurrency; NULL = no check
  payload_json TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued','applied','rejected')),
  result_json TEXT,                       -- appended seqs / rejection reason
  applied_at INTEGER
);
CREATE INDEX commands_status ON commands(status, at);

CREATE TABLE snapshots (                  -- bounds replay: latest snapshot + tail == replay from seq 1
  stream_id TEXT NOT NULL, version INTEGER NOT NULL,
  seq INTEGER NOT NULL,                   -- global seq this snapshot folds through
  schema_version INTEGER NOT NULL DEFAULT 1,
  state_json TEXT NOT NULL, taken_at INTEGER NOT NULL,
  PRIMARY KEY (stream_id, version)
);

CREATE TABLE projection_offsets (         -- one row per read model; drives lag display and rebuild
  name TEXT PRIMARY KEY,
  last_seq INTEGER NOT NULL DEFAULT 0,
  rows INTEGER NOT NULL DEFAULT 0,
  rebuilt_at INTEGER, rebuilt_ms INTEGER
);

CREATE TABLE deadlines (                  -- projected from events; armed by one timer wheel, no sweeps
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,                     -- approval_expiry|retry_backoff|run_timeout|schedule_due|
                                          -- provider_retry
  ref TEXT NOT NULL,                      -- run:<id> / approval:<id> / schedule:<id>
  due_at INTEGER NOT NULL, armed_by_seq INTEGER NOT NULL,
  fired_at INTEGER, cancelled_by_seq INTEGER
);
CREATE INDEX deadlines_due ON deadlines(due_at) WHERE fired_at IS NULL AND cancelled_by_seq IS NULL;

CREATE TABLE effects (                    -- at-least-once side effects awaiting their result event
  id TEXT PRIMARY KEY,
  event_seq INTEGER NOT NULL REFERENCES events(seq) ON DELETE CASCADE,
  kind TEXT NOT NULL,                     -- CallChatModel|ExecuteTool|RunSubagent|ComputeDigest|…
  ref TEXT NOT NULL,                      -- run:<id>:step:<id>
  idempotency_key TEXT NOT NULL,          -- == the originating command_id
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','done','failed')),
  attempts INTEGER NOT NULL DEFAULT 0,
  not_before INTEGER,                     -- retry backoff schedule (a deadline, not a sleep loop)
  last_error TEXT
);
CREATE INDEX effects_pending ON effects(status, not_before) WHERE status = 'pending';
```

**Read models (projections).** The tables below are derived. Foreign keys, indexes, and even the
projection-only `CHECK`/unique constraints are defence in depth; the authoritative rules are checked in
deciders before an event is appended ([events.md](events.md) §9).

```sql
CREATE TABLE process_leases (              -- projection of LockAcquired/LockReleased (§3.1)
  id TEXT PRIMARY KEY,                     -- 'engine'
  pid INTEGER NOT NULL, host TEXT NOT NULL,
  acquired_at INTEGER NOT NULL, heartbeat_at INTEGER NOT NULL
);

CREATE TABLE boards (
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL
);

CREATE TABLE columns (
  id TEXT PRIMARY KEY,
  board_id TEXT NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('backlog','todo','doing','review','done')),
  position INTEGER NOT NULL,
  wip_limit INTEGER,
  auto_run INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX columns_board_pos ON columns(board_id, position);

CREATE TABLE agents (
  id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, role TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'agent' CHECK (kind IN ('agent','skill')),
                                    -- only 'agent' is stored here in v0.1; skills live in their own
                                    -- table (below) because they carry no model/policy. The column is
                                    -- present so a future kind can share the roster surface.
  model_alias TEXT,                 -- the definition's declared model; NULL = no declaration (§3.6.1)
  model_binding TEXT,               -- [agents.model_bindings] value (model-only); NULL = none
  effective_model TEXT,             -- agent-level resolution: binding > definition; NULL = inherit/unresolved
  model_source TEXT,                -- binding | definition | inherit | none (why effective_model won)
  system_prompt TEXT NOT NULL,
  decision_model TEXT,               -- optional override; falls back to [decisions].model
  tools_json TEXT NOT NULL,          -- computed allowlist (allow + ask tools)
  tool_policy_json TEXT NOT NULL DEFAULT '{}',  -- per tool: effect + winning rule + origin
  tags_json TEXT NOT NULL,           -- free-form labels used by auto-assign (§3.7)
  max_steps INTEGER NOT NULL DEFAULT 20,
  max_usd_per_run REAL,
  mode TEXT NOT NULL DEFAULT 'primary' CHECK (mode IN ('primary','subagent','all')),
  source TEXT NOT NULL DEFAULT 'db'
    CHECK (source IN ('db','config','opencode-global','opencode-project','agents-global','agents-project')),
  source_path TEXT, source_hash TEXT,      -- sha256 of the raw file, for trust + audit
  definition_json TEXT,                    -- normalised spec + warnings (file agents)
  last_seen_at INTEGER,                    -- removed files are disabled, never deleted
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);

CREATE TABLE skills (                      -- .agents/skills/<id>/SKILL.md; attachable, never executable
  id TEXT PRIMARY KEY,                       -- the skill DIRECTORY name (§agents.md §2)
  name TEXT NOT NULL,                       -- frontmatter name; must equal id, mismatch warns
  description TEXT NOT NULL,                -- the one line offered to the model when attached
  body TEXT NOT NULL,                       -- SKILL.md body, wrapped with a provenance header
  body_bytes INTEGER NOT NULL,
  interface_json TEXT,                      -- optional agents/openai.yaml display metadata
  resources_json TEXT NOT NULL DEFAULT '[]',-- names/counts of references,assets,scripts,evals ONLY
  source TEXT NOT NULL DEFAULT 'agents-project'
    CHECK (source IN ('agents-global','agents-project')),
  source_path TEXT NOT NULL, source_hash TEXT NOT NULL,  -- sha256 of SKILL.md
  definition_json TEXT,                     -- raw frontmatter + warnings
  last_seen_at INTEGER,                     -- removed files are disabled, never deleted
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);

CREATE TABLE agent_trust (                 -- project agent approved by a human (§agents.md §6.3)
  project_path TEXT NOT NULL,
  agent_name TEXT NOT NULL,
  source_hash TEXT NOT NULL,
  decided_by TEXT NOT NULL,
  decided_at INTEGER NOT NULL,
  PRIMARY KEY (project_path, agent_name, source_hash)
);

CREATE TABLE skill_trust (                 -- project skill approved by a human (§agents.md §6.4);
  project_path TEXT NOT NULL,              -- mirrors agent_trust because the kinds are separate
  skill_id TEXT NOT NULL,
  source_hash TEXT NOT NULL,
  decided_by TEXT NOT NULL,
  decided_at INTEGER NOT NULL,
  PRIMARY KEY (project_path, skill_id, source_hash)
);

CREATE TABLE tasks (
  id TEXT PRIMARY KEY,
  board_id TEXT NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
  column_id TEXT NOT NULL REFERENCES columns(id) ON DELETE RESTRICT,
  parent_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
  title TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
  priority INTEGER NOT NULL DEFAULT 2,          -- 1 high, 2 normal, 3 low
  position INTEGER NOT NULL,
  assignee_agent_id TEXT REFERENCES agents(id),
  blocked_reason TEXT, blocked_at INTEGER,
  due_at INTEGER,
  tags_json TEXT NOT NULL DEFAULT '[]',
  result_summary TEXT, result_json TEXT,
  estimate_minutes INTEGER,                 -- NULL = never estimated → UI shows `?`
  estimate_source TEXT,                     -- heuristic | learned | decision | user
  estimate_confidence REAL,
  run_forecast_duration_ms INTEGER,         -- NULL until an agent + history exist
  run_forecast_cost_micro_usd INTEGER,
  estimated_at INTEGER,
  estimate_inputs_hash TEXT,                -- unchanged hash ⇒ EstimateTask appends nothing
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  version INTEGER NOT NULL DEFAULT 1   -- optimistic-UI guard only; the task stream's version is authority
);
CREATE INDEX tasks_board_col_pos ON tasks(board_id, column_id, position);
CREATE INDEX tasks_due ON tasks(due_at) WHERE due_at IS NOT NULL;
CREATE INDEX tasks_estimate ON tasks(board_id, estimate_minutes) WHERE estimate_minutes IS NOT NULL;
-- Ordering is integer with gap-free renormalisation: moves/inserts rewrite affected rows inside one
-- transaction (offset by +100000, then assign 1..n). No UNIQUE(position) so reordering never needs
-- temporary collisions; repository tests assert contiguity after every mutation.

CREATE TABLE task_deps (
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  depends_on_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  PRIMARY KEY (task_id, depends_on_id)
);

-- Calibration inputs. Actuals are explicit human numbers, never inferred from elapsed time.
CREATE TABLE task_effort (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  actual_minutes INTEGER NOT NULL CHECK (actual_minutes > 0),
  estimate_minutes INTEGER NOT NULL,        -- snapshot in force when the actual was recorded
  recorded_at INTEGER NOT NULL
);
CREATE INDEX task_effort_task ON task_effort(task_id, recorded_at);

-- EWMA calibrators, rebuilt from TaskEffortRecorded + run terminal events. The EWMA alpha and the
-- ratio clamp are fixed constants in the estimator (ADR-0012), not config: a projection rebuild must
-- not change when the user edits config.toml.
CREATE TABLE estimate_accuracy (
  scope TEXT NOT NULL,                      -- 'tag' | 'agent_tag'
  key TEXT NOT NULL,                        -- tag name, or '<agent-id>|<tag>'
  samples INTEGER NOT NULL,
  effort_ratio REAL NOT NULL DEFAULT 1.0,   -- actual/estimated, clamped 0.25..4.0
  run_duration_ms REAL NOT NULL DEFAULT 0,
  run_cost_micro_usd REAL NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (scope, key)
);
-- Throughput (effort-minutes completed per working day) is a read-time query over task_effort and the
-- TaskCompleted timestamps, not a stored projection: the window is a display choice, not a fact.

CREATE TABLE runs (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  agent_id TEXT NOT NULL REFERENCES agents(id),
  parent_run_id TEXT REFERENCES runs(id) ON DELETE CASCADE,   -- sub-agent run (§3.6.1)
  parent_step_id TEXT REFERENCES steps(id) ON DELETE SET NULL,
  depth INTEGER NOT NULL DEFAULT 0,                            -- 0 = root run
  status TEXT NOT NULL CHECK (status IN
    ('queued','running','awaiting_approval','succeeded','failed','cancelled',
     'budget_exhausted','blocked','interrupted')),     -- §3.3
  trigger TEXT NOT NULL,              -- manual | schedule | retry | subtask
  model_alias TEXT,                   -- resolved model for this run, from RunQueued/RunStarted
  model_source TEXT NOT NULL DEFAULT 'definition'
    CHECK (model_source IN ('override','binding','definition','inherit')),
                                      -- which layer chose it (ADR-0013); override is never persisted
  retry_of TEXT REFERENCES runs(id) ON DELETE SET NULL,
  day_key TEXT NOT NULL,              -- local calendar day 'YYYY-MM-DD', computed in Rust (not SQLite localtime)
  started_at INTEGER, ended_at INTEGER,
  tokens_in INTEGER NOT NULL DEFAULT 0,
  tokens_out INTEGER NOT NULL DEFAULT 0,
  cost_micro_usd INTEGER NOT NULL DEFAULT 0,   -- integer micro-USD: stable cap boundaries
  cost_estimated INTEGER NOT NULL DEFAULT 0,   -- 1 when usage was absent and we estimated
  context_peak_tokens INTEGER NOT NULL DEFAULT 0,  -- max estimated prompt tokens seen (§3.6.2)
  compactions INTEGER NOT NULL DEFAULT 0,          -- completed compactions, for board/status display
  agent_snapshot_json TEXT,          -- system prompt + effective policy + model resolution, from
                                     -- RunStarted; so editing an agent file never rewrites history (§3.6.1)
  skill_snapshot_json TEXT,          -- attached skills {id, source_hash, body}, from RunStarted;
                                     -- a replay never re-reads the file (§3.6.1, agents.md §7.3)
  error TEXT
);
CREATE INDEX runs_task ON runs(task_id, started_at DESC);
CREATE INDEX runs_status ON runs(status);
CREATE INDEX runs_day ON runs(day_key);
CREATE INDEX runs_parent ON runs(parent_run_id);
-- "one active root run per task": enforced in the `decide` step for StartRun (checked against the
-- `runs` projection inside the append transaction), with this index as a performance aid and backstop.
-- It is deliberately NOT the mechanism: an event-sourced write is rejected before it is appended, so
-- there is no constraint-violation error to interpret (§3.7).
CREATE INDEX runs_active_root ON runs(task_id)
  WHERE status IN ('queued','running','awaiting_approval') AND parent_run_id IS NULL;

CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  role TEXT NOT NULL,                 -- system | user | assistant | tool
  kind TEXT NOT NULL DEFAULT 'message'
    CHECK (kind IN ('message','compaction_summary')),  -- summary is user-role data, never system (§3.6.2)
  content TEXT NOT NULL DEFAULT '',
  tool_calls_json TEXT,
  tool_call_id TEXT,
  compacted INTEGER NOT NULL DEFAULT 0,      -- 1 = replaced by a summary; kept for audit, excluded from context
  created_at INTEGER NOT NULL,
  UNIQUE (run_id, seq)
);
CREATE INDEX messages_live ON messages(run_id, seq) WHERE compacted = 0;

CREATE TABLE compactions (                 -- one row per compaction; at most one active summary per run
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  trigger TEXT NOT NULL,                   -- auto | manual | overflow_retry
  from_seq INTEGER NOT NULL,
  through_seq INTEGER NOT NULL,            -- summary covers messages <= through_seq
  messages_compacted INTEGER NOT NULL,
  summary_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  tokens_before INTEGER NOT NULL,
  tokens_after INTEGER NOT NULL,
  summary_tokens INTEGER NOT NULL,
  model_alias TEXT NOT NULL,
  cost_micro_usd INTEGER NOT NULL DEFAULT 0,
  fallback INTEGER NOT NULL DEFAULT 0,     -- 1 = deterministic digest, summariser unavailable/disabled
  created_at INTEGER NOT NULL,
  UNIQUE (run_id, seq)
);

CREATE TABLE steps (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  kind TEXT NOT NULL,                 -- llm_call | decision_call | tool_call | subagent_call
                                      -- | compaction | approval | note
  state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','closed','aborted')),
  payload_json TEXT NOT NULL,         -- request/response or tool args/result (large payloads on disk)
  provider_request_id TEXT,           -- id from the provider response (llm_call only)
  tokens_in INTEGER NOT NULL DEFAULT 0,
  tokens_out INTEGER NOT NULL DEFAULT 0,
  cost_micro_usd INTEGER NOT NULL DEFAULT 0,
  started_at INTEGER NOT NULL, ended_at INTEGER,
  error TEXT,
  UNIQUE (run_id, seq)
);
CREATE INDEX steps_run_seq ON steps(run_id, seq);

CREATE TABLE approvals (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  step_id TEXT NOT NULL REFERENCES steps(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,                 -- action | question
  action TEXT NOT NULL,               -- tool name / 'finish' / 'ask_human'
  payload_json TEXT NOT NULL,         -- exactly what the human is approving
  args_hash TEXT NOT NULL,            -- sha256 of canonical payload; resume verifies it
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK (status IN ('pending','approved','denied','expired')),
  expires_at INTEGER NOT NULL,        -- a `deadline`, not a sweeper: expiry fires as ApprovalExpired
  requested_at INTEGER NOT NULL,
  decided_at INTEGER, decided_by TEXT, decided_note TEXT
);
CREATE INDEX approvals_pending ON approvals(status) WHERE status = 'pending';
CREATE UNIQUE INDEX approvals_one_pending_step ON approvals(step_id) WHERE status = 'pending';

CREATE TABLE decisions (               -- every Jev/System One call, inside a run or engine policy
  id TEXT PRIMARY KEY,
  at INTEGER NOT NULL,
  purpose TEXT NOT NULL,               -- agent_tool | auto_assign | priority | approval_advice
                                       -- | failure_triage | spec_gate
  run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
  task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
  provider TEXT NOT NULL,              -- configured provider name
  model_requested TEXT NOT NULL,
  model_answered TEXT,                 -- e.g. jev-1.13.0
  state_excerpt TEXT NOT NULL,         -- per config: full | excerpt (default 500 chars) | hash
  questions_json TEXT NOT NULL,
  answers_json TEXT,
  tokens_in INTEGER NOT NULL DEFAULT 0,
  tokens_out INTEGER NOT NULL DEFAULT 0,
  cost_micro_usd INTEGER NOT NULL DEFAULT 0,
  latency_ms INTEGER,
  confidence REAL,                     -- normalised: max(noul,1-noul) | choice/score confidence
  applied INTEGER NOT NULL DEFAULT 0,  -- 1 when a policy actually used it (e.g. auto-assign)
  outcome TEXT,                        -- accepted | rejected | succeeded | failed (recorded later)
  error TEXT
);
CREATE INDEX decisions_task ON decisions(task_id, at DESC);
CREATE INDEX decisions_purpose ON decisions(purpose, at DESC);

CREATE TABLE artifacts (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
  label TEXT NOT NULL, path TEXT NOT NULL,        -- relative to run workspace
  mime TEXT, sha256 TEXT, bytes INTEGER,
  created_at INTEGER NOT NULL
);

CREATE VIRTUAL TABLE tasks_fts USING fts5(
  title, description, content='tasks', content_rowid='rowid'
);
-- external-content FTS: the delete trigger must issue the FTS5 'delete' command with the old row
-- values, otherwise stale terms linger. Filter strings are parsed separately and any MATCH operand is
-- quoted with embedded quotes doubled; raw user input never reaches MATCH.
-- `search` is itself a projection of TaskCreated/TaskEdited/TaskDeleted, so it can be rebuilt too.
-- Log retention is `[events] retention_days` (default 90, 0 = forever); `mta gc` prunes below the
-- watermark only, after `mta run export` has frozen any run whose events it would drop (§3.10).
```

**Durability contract for steps and messages:** a step's whole life is a short event sequence —
`RunStepStarted` → (`LlmCallRequested` | `ToolCallRequested`) → `MessageAppended` →
(`LlmCallCompleted` | `LlmCallFailed` | `ToolCallCompleted` | `ToolCallFailed`) → `RunStepCompleted` —
appended one event per `decide`+append transaction, so a crash between two of them leaves a *readable*
partial step rather than a torn row. Assistant text is buffered during streaming and appended as a
single `MessageAppended` when the call settles (success, failure, cancel, or timeout); a truncated
stream still appends the partial content and closes the step with an error outcome, so the transcript
never shows a half-written message and the projection never has to infer a row's state. Streaming token
deltas are *never* logged — they go over the ephemeral channel (ADR-0009 §8), which is why "flush at
settle" is cheap. Because each event append is its own transaction, the invariant to protect is not
"all-or-nothing rows" but **per-stream version contiguity**: a `SnapshotTaken`/`verify` pass asserts that
a stream's versions run `1..=n` with no holes. This is tested with injected cancellation at every await
point, including between the append and the projection.

A compaction is likewise a short event sequence on the run stream (`CompactionPerformed` carrying the
covered span, plus the `MessageAppended{kind=compaction_summary}`); because the covered span is recorded
in the event and the projection is derived from it, a crash can never leave both a summary and its
source span live in the context — replay produces exactly one of the two, deterministically.

**Storage traits (`mta-store`), never repositories.** There is no `TaskRepo` writing rows. Three traits
express the model:

- `EventLog` — `append(stream_id, expected_version, events) -> Result<Vec<Seq>>`, `read_stream(id,
  from_seq)`, `read_after(seq, limit)`, `global_after(seq)`. The only writer in the system.
- `CommandBus` — `submit(Command) -> CommandId`, `await_result(id)`, and a subscription for the engine's
  `CommandProcessor`. Used by the CLI, the TUI, effects, and the timer wheel alike.
- `Projection` — `name()`, `apply(&Event)`, `offset()`, `truncate_and_rebuild(&EventLog)`; implemented by
  `board`, `tasks`, `runs`, `steps`, `messages`, `approvals`, `decisions`, `agents`, `skills`, `budgets`,
  `schedules`, `deadlines`, `run_costs`, `search`, `digests`, `system` (engine life cycle, lease,
  config hash, GC/snapshot records) and `process_leases`.

Reads for the engine's deciders go through projection *queries* (typed views over the read models), never
ad-hoc SQL in a decider. The TUI sees only *view models* (`CardVm`, `BoardVm`, `RunVm`) folded from the
event stream, never rows. This keeps rendering independent of SQL, makes snapshot tests trivial, and
means a projection can be rebuilt mid-render without the UI noticing anything but a lag figure.

### 3.5 Provider layer — chat and typed decisions

**Wire subset used (keep it deliberately small):**

- `POST {base_url}/chat/completions` with `stream: true`, `model`, `messages`, optional `tools`,
  `tool_choice`, `temperature`, `max_tokens`, `response_format: {type: "json_object"}` (only when the
  model profile declares support).
- `GET {base_url}/models` for `mta models list` / `doctor`.
- Auth: `Authorization: Bearer <key>` by default; per-provider extra headers; optional
  `api-key` header mode for Azure-style gateways (v1.1).
- Parse: `choices[0].delta.content`, `choices[0].delta.tool_calls[]` (assemble by `index`), `usage`
  (optional — many local servers omit it in streaming), `finish_reason`, and `error.message` on
  non-2xx. Treat `content: null` as empty when tool calls are present (common in Ollama/vLLM).
- Tolerate SSE framing differences: comments (`:`), `event:` lines, `data: [DONE]`, chunk boundaries
  splitting UTF-8/JSON (buffer bytes, decode on complete lines). Never assume one chunk = one event.
- Retry `408/409/429/5xx` and connection errors with exponential backoff + jitter, honour
  `Retry-After`, cap attempts; never retry after any assistant delta has been emitted (avoid
  duplicated tool calls) — append the partial `MessageAppended`, the failing `LlmCallFailed`, and
  close the step with an error outcome, surfacing the error to the run instead. Retries are **effect
  attempts**, not a sleep loop inside the call: the `CallChatModel` effect for a step records
  `attempts` and a `not_before` backoff, and each attempt is a command whose result becomes an event, so
  a retried call is visible in the log and a crash mid-backoff resumes with the same idempotency key.
  Tool side effects only run after a complete stream, so a retried call can never execute a tool twice.
- **Error classes:** map failures to `Auth | RateLimited | ContextOverflow | Server | Network |
  Protocol | Cancelled`. A context-length 400 (`context_length_exceeded`, `maximum context length`,
  `too many tokens`, `prompt is too long`) is **not** retried by the generic loop: it becomes
  `ContextOverflow`, which asks the engine for one emergency compaction and a single retry (§3.6.2).
  Retrying a too-long prompt unchanged only burns latency and money.

**Abstraction**

```rust
#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn stream_chat(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError>;   // ChatStream = Stream<Item = Result<StreamEvent, _>>
}

pub enum StreamEvent {
    TextDelta(String),
    ToolCallDelta { index: u32, id: Option<String>, name: Option<String>, args: Option<String> },
    Usage { input: u32, output: u32 },
    Finished { reason: FinishReason },
}
```

Capabilities are declared per model profile (`tools`, `json_mode`, `streaming`, `vision`, `context`).
The engine degrades: no `tools` capability → the agent gets a single-shot "answer as JSON" prompt and
no tool loop, and the run is labelled `degraded` in the UI. This is how exotic OpenAI-compatible
servers stay usable without per-provider code.

**Model catalogue and search.** The picker and `mta models list` read one merged catalogue: `[[models]]`
profiles (full metadata), plus a shallow live view from each enabled provider's `GET /models`
(`provider/model-id`, owner, no context/pricing/capabilities), plus recent use from the `runs`
projection. A live-only row is marked `not configured` and is **not selectable**: choosing it opens a
profile form (alias, provider, model id, required context window, optional pricing, capability toggles),
which appends a `[[models]]` entry before the original selection completes. Capability defaults are
conservative (`streaming = true`, `tools = false`, `json_mode = false`), and unknown pricing is `0` with
an `unknown` badge, so a guessed profile is visible rather than silently wrong.

The catalogue is **ephemeral** (like host metrics): a per-provider in-memory cache with
`[model_selection].discovery_ttl_s` (default 300), refreshed when the picker opens if stale, and forced by
`Ctrl-R` / `mta models refresh`. No background poll, no network on a keystroke, and no catalogue event —
a replay with every provider offline produces identical domain state. A failed provider is a row group
with its error while the configured models remain usable.

Search is a small hand-written parser, not a regex engine: free terms fuzzy-match alias, model id, and
provider; `@provider`; `ctx>=100k` / `ctx<=32k` / `ctx=200k` (also `m`); `tools`, `no-tools`, `json`,
`stream`, `systemone`, `chat`, `free`, `configured`, `unconfigured`, `available`, `unavailable`,
`recent`; `-token` negates; `"a phrase"` is literal. Ranking is deterministic: exact alias → alias prefix
→ word boundary → subsequence → provider/id substring; ties by configured, available, recent, then alias.
An empty query shows configured + available + recently used. The same parser backs the CLI and the TUI,
so `--json` and the picker cannot disagree. `mta-provider` owns it; it does no I/O.

**Configuration** (see Appendix A): providers (transport + auth), models (alias → provider, model id,
params, pricing, capabilities), agents (alias + prompt + tool allowlist). Model aliases are resolved
at run start, so swapping models is config-only. A model selection is written by the engine's
`ConfigEditor` (`toml_edit`, atomic temp+rename, validate-before-write, hash-checked against external
edits, `.bak` restore on a failed reload) and then recorded through `ReloadConfig`; no decider writes a
file, and no secret is ever written ([ADR-0013](adr/0013-model-selection-and-search.md)).

**Conformance suite** (`mta-provider/tests`): recorded fixtures for
OpenAI, OpenRouter, Groq, Ollama, vLLM, LM Studio + `httpmock` for failures. Cases: plain text
stream, tool-call stream split mid-JSON, multiple parallel tool calls, usage chunk, 429 with
`Retry-After`, 400 with error JSON, truncated stream, non-SSE JSON error body, UTF-8 split across
chunks. `mta doctor --provider x` performs a live probe (models list + 1-token completion) and prints
a capability matrix. `docs/providers.md` records tested versions and quirks.

### 3.5.1 Decision provider — Jev / System One

Jev is **not** an OpenAI chat model. It answers typed questions about a `state` and returns one typed
answer per question; there is no streaming, no tool calling, and no free-form text. Forcing it into the
`ChatModel` trait would either fail or turn it into a bad chatbot. It therefore gets a second,
first-class provider wire: **`kind = "systemone"`** with its own `DecisionModel` trait, used by agents
and by engine policies at explicit decision points.

**Wire contract** (`POST {base_url}/systemone`, bearer `kn-` key):

```jsonc
{
  "model": "jev-1-13:free",
  "state": "text, object or array to evaluate",
  "questions": {
    "ready":   { "type": "noul",   "instructions": "Is the report ready to submit today?",
                 "criteria": { "true": "all sections complete and reviewed",
                               "false": "anything material is missing or unreviewed" } },
    "next":    { "type": "choice", "instructions": "What is the best next action?",
                 "criteria": { "a": "Wait until Friday", "b": "Draft the missing finance section",
                               "c": "Ask the reviewer to return early", "d": "Submit drafts only" } },
    "urgency": { "type": "score",  "instructions": "How urgent is this work?",
                 "criteria": ["not urgent", "somewhat urgent", "urgent", "critical"] }
  }
}
```

Response (verified against `jev-1-13:free`):

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "ready":   { "type": "noul",   "noul": 0.02 },
    "next":    { "type": "choice", "choice": "b", "confidence": 0.97,
                 "probabilities": { "a": 0, "b": 0.98, "c": 0.02, "d": 0 } },
    "urgency": { "type": "score",  "score": 2.13, "confidence": 0.81,
                 "legend": { "0": "not urgent", "1": "somewhat urgent",
                             "2": "urgent", "3": "critical" },
                 "probabilities": { "0": 0, "1": 0.03, "2": 0.81, "3": 0.16 } }
  },
  "usage": { "input_tokens": 12, "output_tokens": 20 }
}
```

**Rust surface** (`mta-provider/src/systemone.rs`):

```rust
#[async_trait]
pub trait DecisionModel: Send + Sync {
    async fn decide(&self, req: DecisionRequest, cancel: CancellationToken)
        -> Result<DecisionResponse, ProviderError>;
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Noul   { instructions: Value, criteria: Option<Value> },
    Choice { instructions: Value, criteria: BTreeMap<String, Value> },
    Score  { instructions: Value, criteria: Vec<Value> },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul   { noul: f32 },
    Choice { choice: String, confidence: Option<f32>,
             #[serde(default)] probabilities: BTreeMap<String, f32> },
    Score  { score: f32, confidence: Option<f32>, legend: Option<BTreeMap<String, String>>,
             #[serde(default)] probabilities: BTreeMap<String, f32> },
    #[serde(rename = "box")] BoxAnswer { /* reserved for v0.1.1 */ },
}
```

Client rules:

- Validate before sending: 1–128 questions, `state` ≤ 512 KiB, non-empty instructions, choice has ≥2
  options, score rubric is a non-empty ordered array, no `box` in v0.1.
- Validate the response: every requested id answered exactly once; answer `type` matches the question;
  `choice` is one of the criteria keys; `score` within `0..=n-1`; confidences in `0..=1`; `noul` in
  `0..=1`. A malformed answer is **not** coerced — it is recorded as an error and the decision fails
  closed.
- **Normalised confidence:** for `noul`, `noul` is P(true), so confidence is `max(p, 1-p)`; for
  `choice` and `score`, use the returned `confidence` (falling back to the top probability). `min_confidence`
  always applies to this normalised value. Separately, a `noul` **answer** is compared to its own
  threshold (`true_threshold`, default 0.5) — certainty that the answer is false is not a reason to
  proceed.
- No streaming and no retry after a 400. Retry only `429`/`5xx`/timeouts, honouring `Retry-After`.
  `402 insufficient_balance` disables the provider for the session (header warning) and policies fall
  back to deterministic behaviour; `model_not_found` is retried once, then reported.
- Model discovery uses `GET /v1/models?modality=systemone`; a plain `/models` call deliberately omits
  System One models. Free ids cost Rp 0; `mta models list --modality systemone` shows the live list.
- **Billing:** System One bills input tokens only (output is reported, not billed). Prices are in IDR;
  convert to the canonical `cost_micro_usd` using a configured `usd_idr_rate` (or the public
  `GET /api/public/pricing` rate), record the raw IDR price in the decision row for audit, and label
  the converted amount approximate. Free models write 0.
- `box`/image questions are typed but rejected in v0.1; the enum reserves the variant so adding them
  is additive.

**Where decisions are used** (all recorded in `decisions`, all advisory unless marked *apply*):

| Purpose | Question shape | Default behaviour |
|---|---|---|
| `agent_tool` — the agent's `decide` tool | Any subset; the chat model authors state + questions | Answer returned to the agent loop; nothing applied |
| `auto_assign` | `choice` over eligible agents, plus `noul` "is any agent a fit?" | Suggests (fit p ≥ 0.5); *applies* in headless mode only when `apply_auto_assign = true` and confidence ≥ threshold |
| `priority` | `score` over P1–P3 from title/description/due date | Suggests a priority badge; applies only in headless mode if configured |
| `task_estimate` | `score` over five effort buckets (`<1h`, `1–2h`, `2–4h`, `4–8h`, `>1d`) from title, description excerpt, tags, priority, dependencies, and similar completed work | Applies as a *planning estimate* only above `min_confidence`; otherwise the deterministic heuristic stands. It never gates a run, a move, or a budget reservation |
| `approval_advice` | `noul` "is this action destructive/irreversible?" + `choice` risk class | **Never auto-approves**; shown in the approval dialog with its confidence |
| `failure_triage` | `choice` {retry, reassign, escalate, abandon} on error + run summary | Suggests the next step in the run panel; engine applies only if configured |
| `spec_gate` | `noul` "is this task specific enough to start?" | If p(specific) < `spec_threshold` (default 0.6) or confidence is low → `ask_human` before spending tokens |

Configuration is intentionally separate from chat models, so a user can run entirely local chat models
and still use Jev (or no decision provider at all):

```toml
[[providers]]
name = "kenari"
kind = "systemone"
base_url = "https://kenari.id/v1"
api_key_env = "KENARI_API_KEY"
timeout_secs = 60
max_retries = 2

[[models]]
alias = "jev"
provider = "kenari"
model = "jev-1-13:free"
modality = "systemone"
usd_idr_rate = 16000          # optional; used only for display/budget conversion

[decisions]
enabled = true
model = "jev"
min_confidence = 0.70
store_state = "excerpt"       # full | excerpt | hash (privacy vs debuggability)
apply_auto_assign = false
apply_priority = false
apply_failure_triage = false
approval_advice = true
failure_triage = true
spec_gate = true
spec_threshold = 0.60
```

When Jev is disabled, unavailable, or under-confident, every policy falls back to its deterministic
rule (position order, least-busy agent, no priority change, human approval). Decisions are never a
hard dependency for running the board. Calibration is possible later because `decisions.outcome` is
filled in when the human accepts/rejects, a run succeeds/fails, or an applied assignment is overridden.

### 3.6 Agent runtime

**Loop (bounded):**

1. **Assemble and fit context** (§3.6.2): system prompt + task (title, description, feedback history,
   parent/sibling summaries) + active compaction summary + live messages + tool schemas. The task,
   feedback, and sibling summaries come from projections; the message span comes from the `messages`
   projection (rebuilt from `MessageAppended`). Estimate the prompt size; compact before sending if it
   reaches the trigger. Never send a request that would exceed the model's window — fail closed instead.
2. Stream a completion. Token deltas go to the UI over the **ephemeral channel** only; nothing is logged
   while streaming. When the call settles the engine appends one `MessageAppended` (assistant text,
   however much arrived) plus `LlmCallCompleted` (usage, finish reason, cost, latency, attempts).
3. If `tool_calls`: validate each against the allowlist and JSON schema → policy check (approval?) →
   append `ToolCallRequested` → execute as an effect → append `ToolCallCompleted`/`ToolCallFailed`/
   `ToolCallDenied` → continue the loop. Tool results are appended once, after the tool returns.
4. If no tool calls: require the final answer to be a JSON envelope
   `{summary, details, artifacts:[], next_action, confidence}` (schema-guided, with a repair retry).
   Append `RunSucceeded{summary, artifacts[]}`; artifacts are registered by a `WriteArtifact` effect, not
   a direct row insert.
5. Enforce budgets after every appended event: steps, tokens, USD, wall clock, and a **loop detector**
   (hash of `(tool_name, args)` repeated 3× → stop with `RunBlocked{reason: "loop_detected"}`). Because
   every step is an event, the budget check is a fold over the run stream, not an in-memory counter a
   crash can lose.

**Decisions inside the loop:** the agent's system prompt explains that `decide` exists and is cheaper
and more calibrated than asking the chat model to rate its own confidence. Typical uses are
classification (which queue/tag), pairwise comparison (which draft is better against a rubric), and
go/no-go gates. The system prompt also states that Jev output is evidence, not authority: below
`min_confidence` the agent must call `ask_human` instead of proceeding. The chat model authors the
questions and the state; the engine checks size/typing and records the whole exchange.

**Built-in tools (v0.1)**

| Tool | Default policy | Notes |
|---|---|---|
| `read_file`, `list_dir`, `grep_files` | allow inside workspace | path canonicalised, symlink escape denied |
| `write_file`, `edit_file` | allow inside workspace | same containment; writes outside → approval |
| `run_command` | **approval required**, sandboxed cwd | argv array only (no shell string unless policy allows); timeout; output cap; env scrubbed of secrets |
| `http_fetch` | allow GET; POST/PUT approval | size cap, timeout, content-type allowlist; response marked untrusted |
| `decide` | allow when a decision model is configured | sends typed `noul`/`choice`/`score` questions to Jev; enforces 128-question/512 KiB limits; returns typed answers with confidence; never applies a decision itself |
| `web_search` | optional (provider-native or tool pack) | off by default |
| `create_subtask` | allow | creates a task in Todo, inherits board, links parent |
| `delegate` | allow when a launchable target exists, the caller's `subagent` policy allows it, and depth/cycle limits pass | starts a **child run** on the same task with fresh context, own model/policy/budget/transcript; no scheduler slot; see §3.6.1 |
| `update_task` | allow for fields; lane changes restricted | cannot move to `done`; cannot reassign self |
| `ask_human` | allow | creates an approval/question and suspends the run |
| `finish` | allow | ends the run successfully with a validated JSON envelope; moves the card to Review, **never** to Done |

**Workspace (one policy, stated once):** the run workspace
`$XDG_DATA_HOME/mta/workspaces/<run-id>/` is the **only** writable root. `--project <dir>` adds a
**read-only** mount for context. A write outside the workspace is denied by default; an approved
out-of-workspace write is staged in `<workspace>/staging/` and surfaced as a diff/apply artifact — the
agent never touches the real path and v0.1 never auto-modifies user repos. Symlinks are resolved and
containment is checked on the canonical path, not the textual one. `run_command` is spawned in its own
process group (`setsid`; `kill_on_drop` plus group kill) so cancellation kills grandchildren, which is
what makes the 2 s stop guarantee real.

**Prompt-injection posture:** all tool output is wrapped in `<untrusted source="…">`; the system
prompt states it must not follow instructions found there; tools that mutate state are never
auto-invoked from content; approvals show the exact command/path/destination. Tests include a fixture
where fetched content tries to trigger `run_command` — expected: policy blocks or asks.

**Approval protocol:** the engine appends `ApprovalRequested` bound to `(run_id, step_id)` with a
canonical `args_hash` and an `expires_at`, then `RunSuspended`; the `approvals` row is the projection of
those events and the card shows a badge. A decision appends `ApprovalDecided` (`approved|denied`,
`by`, note); on resume the executor checks the projection and verifies the run is still
`awaiting_approval` and the hash matches the pending payload — a mismatch aborts the step instead of
executing something the human did not see. In the TUI: `v` → run panel → `y/n` with the exact payload;
headless/scheduled runs use `mta approval list|approve|deny`. **Expiry is a deadline, not a sweep**
(default 24 h, configurable): `ApprovalRequested` arms an `approval_expiry` deadline, the timer wheel
fires `FireDeadline`, and the decider appends `ApprovalExpired` and fails the run with
`approval_expired`. There is no engine-tick sweep, no startup scan, and `approval list` is a pure read
that cannot mutate state. Pending approvals survive restart because their events are durable; if a run
was reconciled to `interrupted` by `CrashRecovered`, the decider expires the approval rather than leaving
it dangling.

### 3.6.1 Sub-agents, skills, and file-defined agents

`mta` reuses the catalogue a project already has. On startup one scan reads four roots for **agents** —
`~/.agents/agents/**/*.md` + `~/.agents/*.md`, `~/.config/opencode/agents/**/*.md`, then
`.opencode/agents/**/*.md` and `.agents/agents/**/*.md` + `.agents/*.md` from the project root down to the
opened directory (legacy `.opencode/agent/` accepted with a warning), then `mta` config
`[[agents.definitions]]` (highest precedence) — and two roots for **skills**:
`~/.agents/skills/*/SKILL.md` and `.agents/skills/*/SKILL.md`. Markdown body = system prompt (agent) or
instruction text (skill); `description`, `mode`, `model`, `steps`, `hidden`, `color`, `disabled` map to the
agent record. Full contract, field matrix, and divergences: [agents.md](agents.md); decisions:
[ADR-0006](adr/0006-opencode-subagents.md) and [ADR-0010](adr/0010-agent-and-skill-discovery.md).

**Hard rules**

1. **Permissions only restrict.** OpenCode `permissions` (V2 ordered list) or `permission` (V1 map) are
   translated to `{allow, ask, deny}` per `mta` tool and intersected with the global policy:
   `effective = strictest(global, agent)`; `allow` in a file can never widen `mta`'s policy. Last match
   wins within one definition; unmatched actions fall back to the global policy, not to `allow`. The
   `skill` action restricts which skills an agent may attach, never what a skill may do.
2. **Whole-definition replace**, not OpenCode's field merge: a winning definition comes from exactly one
   file/entry, so `mta agents show` is a truthful explanation. `disabled = true` is a tombstone.
3. **Project files are untrusted.** First delegation to a project agent — and the first after its
   `source_hash` changes — suspends the child at an approval showing path, prompt excerpt, effective
   permissions, model, and hash (§6.3 of agents.md). Trust is stored per `(project, agent, hash)`.
4. **The sandbox still applies.** OpenCode `edit: allow` means "write inside the run workspace"; real
   repo changes remain staged artifacts behind an approval. `external_directory` can never be widened.
5. **No OpenCode runtime.** No subprocess, plugins, MCP, or LSP; skills are *attached prompt text*, never an
   executed workflow; `opencode.json(c)` agents and `#variant` routing are out of scope for v0.1. Missing
   capabilities degrade; they never fail silently.
6. **Skills are a separate kind.** A `.agents/skills/<id>/SKILL.md` entry has no model, policy, or mode, so
   it can be attached to a run but can never be the agent that executes one, and `delegate` refuses it. Its
   `references/`, `assets/`, `scripts/`, and `evals/` directories are recorded by name and **never read** —
   a skill directory is a black box except `SKILL.md` and its optional `agents/openai.yaml`. This is why the
   `.agents` agent glob is `.agents/agents/**` and never `.agents/**`.

**Delegation:** the `delegate` tool takes `{agent, instructions, context?, skills?, timeout_secs?}` and
appends `DelegateRequested` on the parent run stream, which creates a **child run stream** (`parent_run_id`,
`parent_step_id`, `depth`); the parent gets `SubagentStarted`/`SubagentCompleted` on its own stream,
linked to the child by `causation_id`. Children start with fresh context (no transcript inheritance), get
their own model (or inherit when unresolved and `inherit_model = true`), own budget (capped by the
parent's remaining budget), and own transcript. They do **not** consume scheduler run slots (that would
deadlock two delegating parents at `max_concurrent_runs = 2`); concurrency is limited per parent
(default 1, sequential, because they share one workspace). Depth defaults to 1, with a cycle check
(A→B→A refused) and `max_children`. Cancellation is hierarchical (parent cancel → child cancel → child
tool process group) and is itself events on both streams. A child failure is returned as a tool result
(`status`, error summary) and the parent decides whether to retry, delegate elsewhere, or abort. A
child's approval suspends the child only; the parent's step stays open (the child's `RunSuspended` and
`ApprovalRequested` live on the child's stream).

**Skill attachment** happens once, at run creation. `StartRun` carries `skill_ids[]`; the decider rejects
the command if an id is unknown, disabled, or untrusted (agents.md §6.4), and `RunStarted.attached_skills[]`
carries `{id, source_hash, body}` for each attached skill, projected to `runs.skill_snapshot_json` — the
same rule as `agent_snapshot_json`, so a replay never reads a file that may have changed. Mid-run
attach/detach is deliberately out of scope: it would let prompt composition change under a fixed stream
`version`. Total attached bytes are capped by `[skills].max_attached_bytes` (default 64 KiB) and a
bound-exceeded attachment is rejected with the file named, never truncated silently.

**Efficiency:** the catalogue's provenance (`source`, `source_path`, `source_hash`, `definition_json`) is a
projection fed by `AgentDiscovered`/`AgentChanged`/`AgentUnavailable` (agents) and
`SkillDiscovered`/`SkillChanged`/`SkillUnavailable` (skills); a filesystem watcher submits `ScanCatalogs`,
whose decider appends only the diffs, so a changed definition is an event rather than a row update and an
idle watcher appends nothing. In-flight runs keep their snapshot, and each run stores `agent_snapshot_json`
and `skill_snapshot_json` (from `RunStarted`), so editing a prompt or a skill never rewrites history. Trust
is `AgentTrusted`/`AgentTrustRevoked` and `SkillTrusted`/`SkillTrustRevoked`, each keyed by
`(project, id, hash)` on its own stream. `mta agents validate` gives CI and headless setups a one-shot scan
of every root and both kinds; `mta doctor` reports roots, skips, unresolved models, untrusted entries, and
unrecognised skill roots. TUI: `A` opens the catalogue roster grouped by kind; `subagent_call` steps are
collapsed to a child summary line; `Enter` opens the child transcript; child approvals count in the header
badge; attached skills show as chips on the run header.

### 3.6.2 Context assembly and auto-compaction

Long runs outgrow the model window. `mta` treats context as a budgeted, persisted resource: it
compacts **before** a request would overflow, with a deterministic fallback, and it never lets a
provider reject a request for length the engine could see coming. Rationale and alternatives:
[ADR-0007](adr/0007-context-compaction.md).

**Definitions**

- `context_window` and `max_output_tokens` are per-model settings (`[[models]]`); if absent they default
  to 200_000 / 8_192 and `mta doctor` warns that the value is assumed.
- `[context].auto_compact` (default **true**) is the master switch.
- `[context].compact_at_tokens` (default **200_000**) is the absolute prompt-token trigger.
- `[context].compact_at_ratio` (default 0.90) caps the trigger relative to the window.
- Effective trigger `T = min(compact_at_tokens, floor(context_window × compact_at_ratio))`; hard request
  budget `B = context_window − max_output_tokens − 1_024`. For a 200K model `T = 180K` and `B ≈ 191K`,
  so the request **plus** the completion still fits. For a 1M-token model the absolute 200K trigger wins.
  The default never runs a 200K prompt against a 200K window.

**Configurability (four levels, highest wins).** Auto-compaction is a policy, not fixed behaviour:

| Level | Where | Keys |
|---|---|---|
| run | `mta run --compact=auto\|off [--compact-at N]`; TUI `:compact on\|off` (session default) | overrides everything for that run |
| agent | `[[agents.definitions]]` entry | `auto_compact`, `compact_at_tokens` |
| model | `[[models]]` entry | `auto_compact`, `compact_at_tokens`, `compact_at_ratio` |
| global | `[context]` | all context keys |

Overrides **replace** rather than merge (the config rule used elsewhere), and the effective value plus
the level that supplied it are printed by `mta agents show`, `mta models list --context`,
`mta doctor --context`, and the run panel's context badge.

Three behaviours are deliberately independent, so "off" is never ambiguous:

- **`auto_compact = false`** → the engine never summarises on its own. The pre-flight guard still runs;
  what happens when the request does not fit is `on_overflow`.
- **`on_overflow`** (default `truncate`) → the request still exceeds `B` after the fit attempt. `truncate`
  replaces the largest tool results with head/tail plus a reference to the full text kept on the step
  (visible in the transcript, recorded as an event); `block` stops with `blocked: context_overflow`
  immediately. Neither costs a model call.
- **`summariser_failure`** (default `digest`) → a compaction is due but the summariser errors or times
  out: `digest` writes a deterministic, visibly lossy summary; `block` stops the run.

Manual compaction — `C` in the run panel, `:compact`, `mta runs compact <id>` — is always available
while a run is active regardless of the switch, and appends the same `CompactionPerformed` event with
`trigger = manual`. So `auto_compact = false` means "I compact when I choose", not "the run dies".

**Token accounting (no tokenizer dependency).** Tokeniser crates are model-specific and heavy, so the
estimator is calibrated at runtime:

- `usage.prompt_tokens` from the provider is authoritative for the request just sent (persisted on the
  `llm_call` step); billing always prefers real usage.
- The next request is estimated from its serialized bytes:
  `ceil(bytes × r × 1.08) + 4 × messages`, where `r` starts at 0.26 tokens/byte (≈3.8 chars/token) and
  is updated per `(provider, model)` with an EWMA of `prompt_tokens / prompt_bytes` whenever usage is
  present, clamped to `[0.10, 0.75]`.
- The estimate is used only for the trigger and the guard, and is labelled `estimated` in the UI; it is
  never used to report spend when usage exists.

**Algorithm (pre-flight, between steps, never mid-stream).** `ensure_context` runs before every
`llm_call`:

1. Build the live context: system prompt → the single active `compaction_summary` message → messages
   with `seq > summary.through_seq` and `compacted = 0` → tool schemas.
2. If `estimate ≤ B` **and** (`estimate < T` or auto-compaction is off for this run), send as-is.
   Otherwise (auto-compaction on and `estimate ≥ T`) compact:
   - choose a cut that keeps the system prompt, the last `keep_recent_messages` (default 6) and at least
     `keep_recent_tokens` (default 24K) of recent context;
   - never split a tool-call group (an assistant message with `tool_calls` and its following `tool`
     results are atomic), and never drop the current task message or a pending approval question;
   - summarise the older span with the summariser model (`[context].model`; empty = the run's own
     model): objective, constraints, decisions, files/URLs touched, verified facts, open questions,
     failures, next step. Target ≤ `summary_target_tokens` (default 3K); individual tool results are
     truncated to `max_tool_result_tokens` (default 20K) in the summariser prompt;
   - append, as one command's events: `MessageAppended` for the summary (role `user`,
     `kind = 'compaction_summary'`, wrapped as model-generated data, **never** role `system`) and
     `CompactionPerformed{compaction_id, trigger, tokens_before, tokens_after, summary_tokens,
     covered_from_seq, covered_through_seq, model_alias, fallback, cost_micro_usd}`. The projector marks
     the covered span `compacted = 1` and the previous summary compacted, and updates the run's
     `compactions` / `context_peak_tokens` read model. A crash between the two events replays to *either*
     the old state or the new state (the projection applies both in order), never a half-compacted
     prompt.
3. Re-estimate. Still above `B`? Append `ContextTruncated{step_id, truncated_refs[], tokens_before,
   tokens_after}` and apply `on_overflow` (`truncate` the largest tool results to
   `max_tool_result_tokens`, keeping head/tail plus an artifact reference / step id for the full text),
   then retry once; if it is `block`, or truncation was not enough, append `RunBlocked{reason:
   "context_overflow"}` before any request is sent.
4. A provider `ContextOverflow` error (the estimate was wrong) forces one compaction with
   `trigger = overflow_retry` and retries the step once when auto-compaction is enabled for the run;
   with `auto_compact = false` it falls through to `on_overflow`. A second failure blocks the run — there
   is no third attempt.

**Rolling summaries.** Each compaction folds the previous summary plus the newly old span into one new
summary, so at most one summary is active and memory does not grow with run length. The compaction step
keeps `tokens_before`, `tokens_after`, `summary_tokens`, the covered `from_seq..through_seq`, the model,
and cost. Every `llm_call` step records the exact message sequence range it sent, so a transcript can be
reconstructed for audit.

**Summariser failure.** Governed by `summariser_failure`: with the default `digest`, an unavailable,
timing-out, or misconfigured summariser produces a deterministic digest instead — message/tool counts,
tool names used, artifact paths, errors, and the task title, under the same wrapper, with
`fallback = 1` — so runs never die because a summariser failed, but the digest is visibly marked "lossy"
in the TUI. With `block`, the run stops at `blocked: summariser_unavailable` instead of writing a lossy
summary.

**Cost and budgets.** Summariser calls are real spend: they are `compaction` steps charged to the run
and to the daily budget (`cost_estimated` set when usage is absent). Compaction does not reset token,
step, or USD budgets. The default summariser is the run's own model, so transcripts are not silently
sent to a second provider; `[context].model = "fast"` is an explicit egress/cost choice.

**Sub-agents.** Each child run compacts independently against its own model and window; a parent's
summary never includes a child's transcript, only the child's bounded result summary (§3.6.1).

**Surfaces.** Run panel header shows `ctx 62% (124K/200K) · 1 compaction`, with the effective trigger and
its source level in the detail line; compaction steps render collapsed as
`⇣ compacted 187K → 24K (summary 2.9K, 4.1s)`; `C` forces a compaction at the next safe point and
`:compact on|off` sets the session default (manual compaction stays available either way). CLI:
`mta runs compact <id>` for a headless nudge, `mta run --compact=auto|off --compact-at N` per run.
`mta doctor --context` prints each model's window, effective trigger, the level that supplied it,
whether auto-compaction is on, and the estimator calibration sample count.

### 3.7 Orchestration policy (engine)

- **Queue (event-reactive, no tick):** the scheduler subscribes to the log and recomputes the `ready` set
  only when a relevant event arrives — a task created/moved/assigned, a dependency completed, a run
  reaching a terminal state, a budget window rolling, or a run timeout firing. "Ready" means: column
  kind `doing`/`todo` with `auto_run`, agent assigned, deps satisfied, task not blocked, run slot
  available; ordering is deterministic by (priority, due date, position, id). Idempotence replaces the
  old \"DB rejects a duplicate\" trick: `StartRun` is validated in `decide` against the `runs` projection
  inside the append transaction, so a second active **root** run per task is rejected with
  `CommandRejected` and nothing is appended (the `runs_active_root` index is a backstop only, §3.4).
  Because the trigger is an event rather than a cadence, a start is prompt (no up-to-30 s latency) and a
  restart replays the same ready set deterministically. Sub-agent runs are started by the executor, not
  the scheduler, and are bounded per parent (§3.6.1).
- **Concurrency (two different counts, named separately):** **run slots** are hard — global
  `max_concurrent_runs` (default 2) + per-agent cap (default 1). **Lane WIP limits** are advisory for
  humans (the UI warns and refuses auto-starts, but manual moves are allowed) unless
  `enforce_wip = true`, in which case the move is rejected with a clear error. A run occupies a run
  slot and the lane holds its card; neither is inferred from the other.
- **Assignment:** manual by default. `auto_assign = true` per column matches task tags against
  `agents.tags_json`, tie-broken by least busy. With a decision provider, Jev first answers a `noul` ("is
  any agent a fit?") and a `choice` over the eligible agents; the ranked suggestion appears first in
  the picker. In headless mode it is applied only when `apply_auto_assign = true` and confidence ≥
  `min_confidence`, and the decision is closed with `DecisionApplied{applied_by: "policy"}`. No eligible
  agent → the card stays in Todo and the engine appends `TaskUnassigned{reason: "no_agent"}`.
- **Priority inference:** optional `score` question over P1–P3 using title, description, due date, and
  tags. It pre-fills a priority badge; the task is only changed (`TaskEdited{changes.priority}`) when
  `apply_priority = true`. Humans always see the suggested value and confidence before/after applying.
- **Estimation:** an `EstimateTask` effect subscribes to the events that can change an estimate —
  `TaskCreated`, `TaskEdited`, `TaskDependencyAdded/Removed`, `TaskAssigned/Unassigned`,
  `TaskEffortRecorded`, run terminal events, a `DecisionAnswered` with
  `purpose = "task_estimate"`, and `EngineStarted` (backfill only) — and submits
  `EstimateTask` for the affected streams. There is **no sweep and no recalculation on a timer**: an idle
  board appends nothing, and a command whose `inputs_hash` matches the current estimate is rejected as a
  no-op before an event is written. The effect computes the deterministic heuristic in-process, reads the
  `estimate_accuracy` bucket, and writes the heuristic result; when
  `[estimation].refine_with_decision` is on, the card is vague (`description` under 120 chars), the tag is
  uncalibrated, and confidence is below `refine_below_confidence`, the same `EstimateTask` also appends
  `DecisionRequested{purpose: "task_estimate"}`, and the existing decision pipeline answers it with a
  `score` over five buckets. The refinement lands as a second `TaskEstimated` (`source = decision`); the
  card is never blank in the meantime. A decision call is bounded (one per changed card, none while the
  previous one is in flight), and a failed/under-confident answer simply leaves the heuristic in place.
  Confidence, `source`, and the bucket's sample count are visible in the card detail and the `T` overlay,
  so a guess is never dressed up as a fact. Estimates never gate a start, a lane move, or a budget
  reservation (§3.3).
- **Spec gate:** before an auto-started run, an optional `noul` "is this task specific enough to
  start?" If P(true) is below `spec_threshold` (default 0.6) or normalised confidence is below
  `min_confidence`, the engine appends `RunBlocked{reason: "needs_detail"}` and an `ApprovalRequested`
  question, and no run is started. This is the cheapest place to stop a bad run.
- **Failures and retries:** each attempt is its own run **stream**. A retry is a new `RunQueued` with
  `trigger = retry` and `retry_of = <previous>`; the failed stream stays terminal. `max_retries` (default
  1) applies with a `retry_backoff` deadline (the event arms it; the timer wheel fires it — no sleep loop
  and no tick). When enabled, `failure_triage` asks Jev to choose {retry, reassign, escalate, abandon}
  from the error and run summary; the choice is shown in the run panel and applied automatically only
  when `apply_failure_triage = true` (default false). On final failure the card goes to Review when
  `review_on_failure = true` (default) — with `blocked_reason = "run_failed"` so it is visually distinct
  from a success awaiting acceptance — otherwise it stays in Doing, blocked. Success always → Review;
  only a human moves Review → Done.
- **Schedules:** `[[schedule]]` entries with a unique `name`, cron expression, board/column, task
  template (title, prompt/tags, agent). `RegisterSchedule` appends `ScheduleRegistered`, which arms a
  `schedule_due` deadline at the next cron instant; the timer wheel fires it, the decider appends
  `ScheduleDue` and then either `ScheduleFired` (creating the task) or `ScheduleSkipped{reason}`.
  Deduplication is by `(schedule.name, local_date)` checked in the decider against the `schedules`
  projection, so a restart or a re-fire cannot double-create. Implemented with a small cron parser
  (`cron` crate) plus the shared timer wheel; no external scheduler and no 30 s tick.
- **Digest:** nightly (via a `schedule_due` deadline) or on demand; `RequestDigest` reads the log,
  `runs`, and open approvals and asks the configured "summariser" model alias to write Markdown (with a
  deterministic fallback if the model is unavailable). The artifact is written by a `ComputeDigest`
  effect, and the result is recorded as `DigestCompleted`/`DigestFailed`. Output to
  `$XDG_DATA_HOME/mta/digests/YYYY-MM-DD.md` and the TUI digest overlay.
- **Cost and daily budget:** money is integer `cost_micro_usd`. When the provider sends `usage`, cost is
  exact; when it does not (common for local streaming servers), tokens are estimated from bytes
  (configurable chars-per-token, +20% conservative) and the run is marked `cost_estimated`, so local runs
  still count toward budgets instead of silently costing zero. The day key (`YYYY-MM-DD` in the
  configured timezone) is decided at command time and carried on the event — SQLite `localtime` is never
  used, and a run spanning midnight counts on its start day, which is also what makes replay
  deterministic. Before starting a run, the engine appends `RunBudgetReserved` against the remaining
  daily budget; if that would exceed `daily_budget_usd`, the run is **not started** (the task is blocked
  with `blocked_reason = "daily_budget"`, header badge). A run that hits its own cap mid-loop ends with
  `RunBudgetExhausted`. Both are visible in the digest.

### 3.8 TUI architecture

- **Pattern:** Elm-ish, no global mutable state. `AppState` (board VM, selection, overlay stack,
  filter, command prompt, agent list, model catalogue VM, run transcripts, toasts, connection/engine
  status, applied `seq`, estimate totals and forecast), + `Action` enum + `update(&mut AppState, Action)
  -> Vec<Command>` + `view(&AppState, &mut Frame) -> HitMap`. Commands are requests to the engine
  (`StartRun`, `MoveTask`, `LoadTranscript`, `LoadModels`, `DecideApproval`) and are named **`Command`**,
  deliberately not `Effect`, because "effect" is now the engine's word for a side effect it runs
  (ADR-0009 §6); one name per concept. The engine replies with **events** — the same events that are
  appended to the log — which the UI folds into its view models, and with query results for ephemeral
  data (`LoadModels` returns a `ModelCatalogueVm`, like a transcript load). Fully testable without a
  terminal; the hit map is returned from the pure render so it
  can be asserted in tests. `BoardVm` maintains the per-column estimate subtotals and the header total
  incrementally as it folds events; a cold start seeds them from one aggregate query, so `view()` never
  sums the board itself.
- **Event loop:** `tokio::select!` over (a) `crossterm::event::EventStream` (`event-stream` feature),
  (b) the engine's event stream, resumed from the UI's own `last_seq`, (c) a **presentation-only repaint
  interval** (`[ui].repaint_ms`, default 250) that exists solely to refresh relative times and spinners
  and never reads or writes domain state, (d) `SIGTERM` / `SIGHUP` (config reload). **`Ctrl-C` is read as
  a `KeyEvent`**, not a signal: raw mode suppresses SIGINT, so a `tokio::signal::ctrl_c` future would
  never fire. Batching: drain pending events, then render once if dirty. On `broadcast::Lagged`, the UI
  **replays from the durable log from its `last_seq`** instead of discarding state and refetching a
  snapshot (§3.1); a cold start reads a read-model snapshot and then resumes from that offset. The status
  line shows projection lag and the applied `seq`, so "stale UI" is distinguishable from "stale
  projection".
- **Host metrics (ephemeral, never logged):** CPU, RAM/swap, and disk usage are sampled by a
  dedicated task on `[host].sample_ms` (default 1000) and delivered to the UI over a
  `tokio::sync::watch` channel, so the reader always sees the newest sample and a slow tick can never
  build a backlog. This is **presentation-only state**: like streaming token deltas, host readings are
  neither deterministic nor meaningful to replay, so they never enter the event log (ADR-0009 §8) and
  a full replay produces identical projections with or without them. Sampling is deliberately decoupled
  from `[ui].repaint_ms` — a repaint redraws from the last sample and never triggers a re-read of
  `/proc`. `sysinfo` (pinned `=0.38.4`; 0.39 requires Rust 1.95 and would force an MSRV bump, ADR-0002)
  sits behind a thin `HostMetrics` trait, so the TUI depends on the trait and tests use
  `FakeHostMetrics`; a failed or partial sample degrades to the last good values plus a `stale` marker
  rather than blanking the widget. CPU percentages need two refreshes to be meaningful, so the first
  sample after startup is discarded. A ring buffer of the last 60 samples feeds `Sparkline` history in
  the overlay. The only way host state can reach the log is the opt-in, debounced
  `HostPressureObserved` event (§3.9), which exists so a run failure can be explained rather than
  merely observed.
- **Model catalogue (ephemeral, never logged):** the picker renders a `ModelCatalogueVm` fetched with
  `LoadModels` and cached by the engine per provider for `[model_selection].discovery_ttl_s`; typing
  filters that snapshot and never performs I/O. `Ctrl-R` sends a refresh. `ModelPicker` is a reusable
  overlay with a `ratatui-textarea` search field, a virtualised result list, filter chips, badges
  (`configured` / `not configured`, `available` / `unavailable`, `ctx`, `tools`, `json`, `free`), and
  keyboard/pointer parity. `ModelProfileForm` is a small modal that gates an unconfigured id: it requires
  a context window, defaults capabilities conservatively, and submits the same `ConfigEditor` write as
  `mta models add`. Selection returns a `ModelRef` to whoever opened the picker (agent editor, run dialog,
  or catalogue browser); the write itself is an engine `ConfigEditor` call followed by `ReloadConfig`,
  never a direct file write from the render path.
- **Widgets:** stock `Block`, `Paragraph`, `List`, `Table`, `Tabs`, `Clear`, `Scrollbar` plus custom
  `BoardWidget` (lays out columns, handles horizontal scroll, computes visible card ranges),
  `CardWidget` (title wrap, badges, progress bar, status glyph), `RunPanel` (virtualised transcript
  list — only render visible lines; decision steps show `question → answer (confidence)`),
  `DecisionLog` (purpose, question, answer, confidence, model, applied flag), `ApprovalDialog`,
  `Palette`, `HelpOverlay`, `Toast`, `AgentRoster` (mode/source/trust/warnings), `ModelPicker`
  (search + filters + virtualised rows), `ModelProfileForm`, `SubagentStep`
  (collapsed child run summary), `CompactionStep` (tokens before → after, fallback badge),
  `EventLogOverlay` (the newest events, `seq · type · stream · actor`, expandable to a causation chain —
  the debugging surface that event sourcing owes its users), `SystemOverlay` (`S`: per-core CPU bars with
  load average and package temperature when the platform exposes it, RAM/swap with a `Sparkline` history,
  one row per real mount with size/used/available/percent, and the top processes by RSS — the overlay is
  full detail, the status line is the summary; it renders from the newest sample and shows a `stale`
  badge if sampling stops), `ForecastOverlay` (`T`: open effort by column/tag/agent, unestimated count,
  throughput in effort-minutes per working day, projected finish, run-time/cost forecast, and the
  calibration table `bucket · samples · ratio`; `e` recomputes the selected card, `E` the whole board
  after a confirmation because it is N commands). Overflowing columns and the run panel render
  a `Scrollbar` whose track and thumb are themselves hit targets; `ApprovalDialog` buttons and the
  detail-strip key hints are registered as pointer regions too.
- **Pointer input (hit map):** ratatui has no hit testing, so `view()` returns a `HitMap`: an ordered
  list of `(Rect, RegionId)` in draw order, plus a layout version counter, stored back into `AppState`
  by the render loop (rationale and alternatives: [ADR-0008](adr/0008-mouse-hit-map.md)). Regions cover
  cards, column headers and bodies, scrollbar tracks/thumbs,
  detail-strip hints, status-line items, overlay rows, palette rows, and approval buttons. `RegionId`
  resolves to an `Action` through the **same table the keymap uses**, so there is one action vocabulary,
  not two code paths — pointer input can never reach a capability the keyboard lacks. `MouseEventKind`
  `Down`/`Up`/`Drag`/`Scroll{Up,Down,Left,Right}` are mapped by coordinate lookup; `Moved` is ignored
  unless `[ui].hover = true`; a double-click (`[ui].double_click_ms`, default 400) fires the card's
  open action. Coordinates outside any region, or inside a region whose entity has since disappeared,
  are dropped as stale.
- **Text editing:** `ratatui-textarea` 0.9 in the card editor and palette (single-line mode); card
  description supports multi-line with `Ctrl-S` (or `Ctrl-Enter`) save / `Esc` cancel /
  confirm-on-dirty.
- **Rendering performance:** cache formatted card lines keyed by `(view revision, width)`, where the
  revision is the highest applied event `seq` (not the row's own version, which is only a projection
  artefact); diff-based draw is already handled by ratatui. Target <16 ms with 200 cards at 120×40.
  Virtualise lists and transcript; no string allocation per cell per frame (reuse `Line<'static>` where
  hot).
- **Terminal lifecycle:** `ratatui::init()` **already installs the panic hook** and enters the
  alternate screen; install any custom hook *before* init (or wrap `ratatui::run`), never after, or
  restore is lost. `restore()` does not undo paste/mouse modes, so bracketed paste **and mouse capture
  are enabled and disabled by the same RAII guard** — symmetric at startup, on normal exit, on panic,
  and on `SIGTERM`. Mouse capture is on when `[ui].mouse = true` (default); `Shift`+drag passes through
  to native terminal selection. Raw mode also disables IXON on most platforms, but `Ctrl-S` save is
  documented as requiring flow control off, with `Ctrl-Enter` as the alternative binding. On exit —
  including panic and SIGTERM — restore cursor, leave alternate screen, disable paste, and disable
  mouse capture (a leaked capture leaves the user unable to drag-select in their shell).
- **Testing:** `TestBackend` snapshots (`insta`) at 40×15, 80×24, 120×40, 200×50 for board, detail,
  run panel, palette, empty/loading/error states; a key-sequence harness drives `update()` and
  asserts state transitions; CI uses `cargo insta test --check` / `INSTA_UPDATE=no` (`cargo insta
  review` is interactive and is a local workflow only). Pointer tests are unit tests over `HitMap`:
  rect→region resolution with z-order (overlay beats board, later draw wins), click-equals-key
  equivalence for every region, wheel clamping at both ends, scrollbar drag math, stale-map drop after
  a resize, `Moved` ignored unless `hover`, and "clicking Approve on a destructive approval still opens
  the confirm dialog". A render harness asserts the RAII guard disables mouse capture on drop and on a
  panicking update.

### 3.9 Configuration and secrets

- Load order: `/etc/mta/config.toml` (optional) → `$XDG_CONFIG_HOME/mta/config.toml` → `./.mta/config.toml`
  (project-local, teams) → `--config` → env (`MTA_*`) → CLI flags. **Merge rules:** scalars replace;
  tables merge field-wise; named arrays (`[[providers]]`, `[[models]]`, `[[agents.definitions]]`,
  `[[schedule]]`) merge **by `name`/`alias`, whole-entry replace** — an entry in a higher layer
  replaces the lower one entirely, additions are appended, and `disabled = true` acts as a tombstone so
  a project can switch off an inherited entry. Anonymous/duplicate names are a validation error.
  `mta config validate` prints the effective config with secrets redacted.
- **Context budget:** `[context]` sets the master switch `auto_compact` (default true), the compaction
  trigger (`compact_at_tokens`, default 200K, and `compact_at_ratio`, default 0.90), recent-context
  retention (`keep_recent_messages`, `keep_recent_tokens`), `max_tool_result_tokens`,
  `summary_target_tokens`, the summariser `model`, `on_overflow` (`truncate` | `block`), and
  `summariser_failure` (`digest` | `block`). Per-model `context_window` / `max_output_tokens` are what
  make the trigger correct; missing values default to 200K / 8K with a `doctor` warning. `[[models]]`
  and `[[agents.definitions]]` may override `auto_compact` (and the trigger keys), and
  `mta run --compact=off --compact-at N` overrides them for a single run. Overrides replace, never
  merge. Values are re-read from the live config for each new `llm_call` step; an active summary keeps
  the settings it was built with, so a mid-run change cannot corrupt an existing compaction.
- **Estimation:** `[estimation]` controls the auto-estimator — `enabled` (default true),
  `default_minutes`, `min_minutes` / `max_minutes`, `round_to_minutes`, `workday_hours` (display only),
  the `tag_minutes` table, `priority_multiplier`, the description-size steps, and the dependency bonus.
  `learn_from_actuals` (default true) turns calibration on; `throughput_window_days` (default 14) and
  `throughput_min_days` (default 3) control the projected finish, and `refine_with_decision` /
  `refine_below_confidence` govern the optional Jev `score` refinement. The EWMA alpha, the calibration
  ratio clamp, and the bucket mapping are **constants in the estimator, not config** — a config change
  must not alter a projection rebuild (ADR-0012). The estimator is dependency-free integer/float
  arithmetic in `mta-domain`; no new crate is added. An estimate never gates a run, a lane move, or a
  budget reservation, so a bad configuration degrades planning quality, not execution.
- **Model selection:** `[model_selection]` governs the picker — `discover` (default true; query
  `/models`), `discovery_ttl_s` (default 300), `include_unconfigured` (default true; show, never
  silently run), `recent_limit` (default 5), and `write_target` (`auto` | `global` | `project`).
  Agent defaults live in `[agents.model_bindings]` (`name = "alias"`), a model-only layer that never
  merges a prompt, tool policy, or budget; precedence is per-run override > binding > definition
  `model` > `inherit_model` ([ADR-0013](adr/0013-model-selection-and-search.md)). A selection is
  written only by the engine's `ConfigEditor`: `toml_edit` preserves comments and ordering, the new
  document is parsed and validated before an atomic temp+rename, one `.bak` is kept, and a hash captured
  when the picker opened aborts the write if the file changed underneath. A failed `ReloadConfig`
  restores the backup, so the file and the UI cannot disagree. The editor refuses to write through a
  symlink pointer swap, preserves the file mode, and never writes a secret or an API key. Config is
  never written from a decider; the durable result is the `ReloadConfig` diff.
- **Pointer/UI:** `[ui].mouse` (default **true**) enables pointer input; `scroll_lines` (default 3) is
  the wheel-notch distance; `hover` (default false) highlights the region under the pointer;
  `double_click_ms` (default 400) sets the double-click window; `repaint_ms` (default 250) is the
  presentation-only redraw cadence (relative times and spinners) and never touches domain state. Mouse
  capture is toggled by the same RAII guard as bracketed paste and restored on panic, SIGTERM, and exit,
  so a crash cannot leave the terminal unable to drag-select; `mouse = false` gives a pure-keyboard
  session with native selection. Every pointer gesture maps to an existing keyboard `Action` — there is
  no mouse-only capability, and `mouse = false` loses no functionality.
- **Agent and skill discovery:** `[agents]` controls the agent roots — `opencode_discovery`,
  `agents_dir_discovery`, `include_global`, `extra_paths`, `trust_project` (`ask|always|never`),
  `max_depth`, `max_children`, `max_concurrent_children`, `max_file_kib`, `inherit_model`, `watch` — and
  `[skills]` controls the `.agents/skills` kind — `enabled`, `discovery`, `trust_project`,
  `max_attached_bytes` (default 64 KiB, enforced at attach time), `max_file_kib`. Discovery is read-only
  and best-effort: a malformed file is a diagnostic, never a startup failure; changed files are picked
  up by the watcher for new runs while in-flight runs keep their snapshot. `[[agents.definitions]]`
  overrides file agents by `name`; `disabled = true` removes an inherited definition. Skills are never
  overridable by config in v0.1 — a skill is a file, and there is nothing about it config could usefully
  replace.
- **Event log:** `[events]` governs the source of truth — `retention_days` (default 90; `0` keeps
  everything forever), `snapshot_interval` (events per stream between snapshots, default 500),
  `rebuild_on_start` (`verify` default | `always` | `never`), and `live_deltas` (default true; streaming
  token deltas over the ephemeral channel, never the log). `mta gc` prunes only below the watermark set
  by the minimum projection offset and each stream's last snapshot, and only after the affected runs
  have been exported ([events.md](events.md), §3.10). Raising retention or disabling GC is a
  size decision, not just a correctness one: the log is the only unbounded table.
- **Host metrics:** `[host]` governs the system monitor — `enabled` (default true), `sample_ms`
  (default 1000), `top_processes` (default 5), and the warn thresholds `cpu_warn_pct`, `mem_warn_pct`,
  `disk_warn_pct` (all default 85/85/90) that colour the status line and set the header badge.
  `mounts` defaults to **real filesystems only** — `statvfs`-backed mounts are listed and synthetic ones
  (`tmpfs`, `devtmpfs`, `squashfs`, `overlay`, `efivarfs`) are excluded, because `/sys/firmware/efi/efivars`
  reporting 60% full is noise, not disk pressure. `observe_pressure` (default **false**) is the one way
  host state reaches the log: when enabled, a threshold crossing appends `HostPressureObserved` on the
  `system` stream, **rising-edge only with a cool-down** (`observe_pressure_cooldown_s`, default 300), so
  it is an audit breadcrumb and not a disguised poll. Samples themselves are never logged, and
  `sample_ms` is clamped to a floor so it cannot be used as a busy-loop knob. `mta doctor --host` prints
  the same one-shot report headlessly.
- **Reload:** the engine watches the config file, and the TUI's `:reload` submits a `ReloadConfig`
  command. A valid config appends `ConfigReloaded{config_hash, changed_sections[]}` and the new
  `Arc<Config>` is used to resolve models/agents at the start of each **new** run (a fold-relevant
  change is an event, so replay sees it); in-flight runs keep their resolved model and budget. A model
  binding diff also appends `AgentModelBound`/`AgentModelUnbound` on `agent:<id>`, and an idle reload
  appends nothing. Invalid
  config is rejected with a toast + log line, `CommandRejected` is appended, and the old config stays
  active. This is how "provider swap without restart" is actually true, and why a picker selection and a
  hand edit of the same file converge on the same events.
- **Decision config:** decision providers are ordinary `[[providers]]` entries with
  `kind = "systemone"`; `[decisions]` selects the default model alias, confidence threshold, which
  policies are enabled, and `store_state` (`full | excerpt | hash`). An agent may override
  `decision_model`. `usd_idr_rate` (or the fetched public rate) converts IDR pricing for budgets and
  display; the raw IDR amount is retained in the decision row.
- Keys: `api_key_env = "OPENAI_API_KEY"` (chat) / `KENARI_API_KEY` (Jev) preferred; `api_key_file`
  (must be 0600) next; OS keyring optional feature. Plain `api_key` is accepted but warns and is
  redacted in logs.
- Never log request headers or message content at `info`; prompt text and tool output require the
  explicit `log_prompts` / `log_tool_output` opt-ins and are still redacted.
- **Diagnostics:** `[logging]` governs the *ephemeral* debug log, which is deliberately **not** the event
  log ([observability.md](observability.md),
  [ADR-0011](adr/0011-observability-and-diagnostics.md)) — `level`, `directives` (per-target overrides,
  longest match wins), `format` (`human`|`json`), `ansi` (`auto`|`always`|`never`), `console` (stderr
  sink; auto-disabled while the TUI owns the screen), `file`, `dir` (default
  `$XDG_STATE_HOME/mta/logs`), `rotate` (`daily`|`hourly`|`never`), `retain_files`, `max_file_mb`,
  `max_total_mb` (hard cap), `ring_lines` (TUI overlay buffer, never persisted), `log_prompts`,
  `log_tool_output`, `max_field_bytes` (truncation), and `dedup_window_ms` (collapse repeated records).
  Precedence: `--log-level` > `RUST_LOG` > `[logging]` > `info`, with the resolved value and its origin
  printed by `mta doctor --logs`. A record carries the ids (`command_id`, `stream_id`, `seq`, `run_id`)
  that join it to the log, so a log line is always traceable to the event that caused it, and the TUI's
  `O` overlay tails the ring while `:log <level>` retunes live.

### 3.10 Security and observability

- Audit trail **is the log**: `mta events tail|show` and `mta decisions` read the append-only `events`
  stream (with `correlation_id`/`causation_id` giving the chain from human intent to outcome), and the
  `steps`/`decisions`/`messages` projections are just convenient views over it. `mta run export <id>
  [--out -]` writes a frozen JSON/Markdown audit bundle (events, steps, messages, decisions, artifacts)
  that makes a run shareable without the DB and is **required before `mta gc` prunes that run's events**.
  Each run stores `agent_snapshot_json` (system prompt, effective policy, model resolution, source hash),
  so the audit trail remains accurate after agent files or config change.
- Log integrity: `mta doctor --events` checks per-stream version contiguity, projection offsets against
  the log head, unreplayed events, pending effects/`EffectFailed`, armed-but-overdue deadlines, and log
  growth against `[events] retention_days`. `mta projections verify` rebuilds a projection in a scratch
  table and diffs it against the live one, so silent projection corruption becomes a failing check
  instead of a mystery.
- Jev receives task/run context, so decision state is assembled explicitly (never the whole DB or
  environment), secrets are redacted first, and `store_state = "hash"` is available for sensitive
  boards. Decision answers are validated model output: they never authorise a tool call, an approval,
  or a lane change without the configured policy and confidence threshold.
- Secrets are scrubbed from tool env, transcripts shown in the UI (redaction pass over configured
  key values), and error messages.
- **Agent definitions are untrusted input.** A cloned repo can ship `.opencode/agents/*.md` with a
  hostile prompt. Project agents require a hash-bound trust decision before their first delegation, the
  prompt carries a provenance header, and their permission rules can only restrict the global policy
  (never grant). Prompt-injection tests cover an agent file that tries to escalate permissions or exfiltrate
  secrets; expected: policy unchanged, trust dialog shows the real source, delegation blocked until
  approved.
- Network egress: only configured provider hosts + explicit `http_fetch`; `http_fetch` resolves the
  host, denies private/link-local/loopback ranges (SSRF guard, override per-provider/tool config), and
  disables environment proxies by default so `HTTP(S)_PROXY` cannot silently bypass the allowlist.
  Provider calls go to the configured `base_url` only, with redirects disabled.
- Rate limits: token-bucket per provider (requests/min + concurrent streams), configurable; 429s
  respect `Retry-After`.
- **Diagnostic logs are a different record, on purpose.** The audit trail is the event log; the debug log
  is an ephemeral `tracing` stream with levels, per-target directives, three sinks (stderr, a rotating
  file, an in-process ring feeding the `O` overlay), redaction, and bounded rotation — the engine never
  reads it and losing it changes no state. It is where the non-deterministic detail that the event log must
  **not** carry actually lives: durations, retry attempts, HTTP status codes, estimator inputs, selected
  model resolution. Contract: [observability.md](observability.md); rationale:
  [ADR-0011](adr/0011-observability-and-diagnostics.md).
- **Deciders do not log.** `mta-domain` has no `tracing` dependency at all, so the purity rule that keeps
  replay deterministic (no clock, no RNG, no I/O in `decide`) is structural rather than a review
  convention: the command processor records the decider's structured rejection and the enclosing `decide`
  span from the outside. `cargo tree -p mta-domain` is asserted to stay free of `tracing`.
- **Secrets stay out of diagnostics.** A `Secret` newtype redacts on `Debug`/`Display`/`Serialize`, keys are
  identified by their `api_key_env` **name**, `Authorization` headers are never formatted, prompt text and
  tool output are off by default, and a property test asserts that no configured secret value appears in
  any captured record at any level — including with `log_prompts` and `log_tool_output` enabled. Library
  crates deny `clippy::print_stdout`/`print_stderr`, so the subscriber is the only way to emit
  diagnostics, and the console sink is disabled by the same RAII guard that owns the alternate screen
  (a record cannot paint over the frame).
- Health: `mta doctor` checks DB integrity (`PRAGMA quick_check`), migrations, config, provider
  connectivity, disk space for workspaces, lease state, and (with `--events`) log health, projection
  lag, and pending effects; `--host` prints a one-shot CPU/RAM/disk report (the `S` overlay's data,
  headless); `--logs [--follow]` prints the resolved level/directives/sinks, the log path, file count and
  size against the caps, and the last records; `--bundle` writes the redacted support archive
  ([observability.md](observability.md) §10). Host readings are never logged at `info`, and no sample is
  ever written to the event log.

### 3.11 Bounded build artifacts and binary size

Rust's `target/` is append-mostly and will grow without a policy: dependency DWARF, incremental
caches, test binaries, stale artifacts, and cross-compilation dirs all accumulate. The project treats
build output as a **budgeted resource**, not an unlimited cache. Full rationale and commands:
[build.md](build.md); decision: [ADR-0005](adr/0005-build-size-budgets.md).

- **Profiles** (in `Cargo.toml`, M0): `[profile.dev] debug = "line-tables-only"`,
  `[profile.dev.package."*"] debug = false` (largest single win), `[profile.test]` same,
  `[profile.release] debug = 0, strip = "debuginfo", lto = "thin", codegen-units = 1`.
  `panic = "abort"` is rejected: it breaks the test harness and weakens terminal-restore-on-panic.
- **Budgets:** `size-budget.toml` holds MiB limits for total `target/`, dev/release/incremental, and
  the stripped `mta` binary. Calibrated in M0 against the first real build (2.3 MiB binary, 932 MiB
  `target/`) and re-tightened in M6 with a measured full-feature binary.
- **Enforcement:** `scripts/size-report.sh --check` fails CI on breach and prints the top consumers;
  `scripts/prune-target.sh [--aggressive]` reclaims space safely. `target/`, coverage (`target/cov`),
  and distribution (`target/dist`) use separate target dirs so the dev cache stays lean.
- **Hygiene:** `CARGO_INCREMENTAL=0` in CI; duplicate crate versions stay a hard gate (`cargo-deny`
  `multiple-versions = "deny"` with commented skips, plus a CI grep that fails on a duplicate terminal
  crate); `cargo vendor` is never committed; a global
  `target-dir` is never pinned in `.cargo/config.toml`.
- **Binary discipline:** trimmed `reqwest`/`tokio`/`clap`/`tracing` features, one version per crate via
  `[workspace.dependencies]`, and `cargo bloat --release --crates` as the diagnostic before any budget
  raise (which requires a reviewed reason, not a silent bump).
- **Growth visibility:** optionally append `date,total_mb,release_mb` to `docs/reports/build-size.tsv`
  from the CI size job; a twice-repeated breach requires a build change, not another cleanup note.

---

## 4. Key decisions and rejected alternatives

| Decision | Choice | Why | Rejected |
|---|---|---|---|
| UI toolkit | ratatui 0.30.2 + crossterm 0.29 | mature, immediate-mode, testable with `TestBackend`; 0.30 split crates but the facade stays stable | egui (not terminal), cursive (less active, harder custom widgets) |
| Async runtime | tokio multi-thread | streaming HTTP + process tools + signals | smol/async-std (ecosystem) |
| Storage | SQLite via sqlx holding an append-only event log; every other table a rebuildable projection | relational queries for board/order/FTS + migrations; zero-ops; one writer, no shared mutable state | JSON files (ordering/queries/FTS), Postgres (overkill locally) |
| Architecture | event-sourced core: commands → validated events → projections; engine owns the log, UI folds the event stream | audit and "why?" for free, replayable/rebuildable reads, no polling cadence, daemon split becomes a transport swap, log replication *is* the v0.2 protocol | CRUD tables with notification events (no causation, polling stays), event-source only runs (two mental models) |
| Architecture detail | engine owns store; UI view-model over events | no blocking UI, clean daemon split later | UI queries DB directly (lock contention, coupling) |
| Host metrics (CPU/RAM/disk) | ephemeral sampler task over a `watch` channel, `sysinfo` behind a `HostMetrics` trait, presentation-only; opt-in debounced `HostPressureObserved` is the sole logged form | operators need machine headroom on screen, but readings are non-deterministic and meaningless to replay — logging them would violate determinism and bloat the only unbounded table; the trait keeps the widget testable and platform-specific | sampling into the event log (non-deterministic replay, log spam), shelling out to `free`/`df`/`top` and parsing text (fragile, locale- and platform-dependent), re-reading `/proc` on every repaint |
| Diagnostics | ephemeral `tracing` log kept separate from the event log: levels, per-target directives, stderr + rotating file + TUI ring, redaction, and deciders that cannot log | "what happened" (events) and "what was it doing" (logs) have different lifetimes and different determinism requirements; telemetry written as events would break replay and bloat the only unbounded table, while a log the router never reads is free to be lossy. Skipping `env-filter` keeps the filter grammar small and the regex automata out of the binary | `log` + `env_logger` (no spans or structured fields, a regex filter anyway, and still a bespoke file sink and TUI buffer), telemetry as events (non-deterministic replay), a hand-rolled writer, OpenTelemetry export in v0.1 |
| Provider abstraction | thin OpenAI-chat wire + separate System One (Jev) decision wire, capability flags | "OpenAI-compatible" is the requested standard; Jev is deliberately not chat and forcing it into that trait would fail | per-vendor SDKs, generic multi-protocol layer |
| Jev integration | first-class `DecisionModel` with typed questions, advisory by default | `noul`/`choice`/`score` are calibrated and auditable; cheap vs another chat call | prompting the chat model to self-rate; MCP-only; auto-approving from model output |
| Task estimation | event-sourced `TaskEstimated`/`TaskEffortRecorded`; deterministic heuristic first, then calibration from actuals, then an optional Jev `score` refinement; effort primary + run forecast; manual pin wins; leaf-only totals | an estimate is a persisted, replayable fact with provenance — recomputing it at render time would change under the user and break rebuild identity; human effort and agent time are different numbers, so they calibrate separately; the heuristic works offline and costs nothing | computing totals in `view()` from mutable columns (unstable, not replayable), one opaque LLM estimate per card (cost, latency, unauditable), story points only (no duration/cost forecast), a separate estimation table written outside the log |
| Model selection | searchable picker over configured + live-discovered models; unconfigured ids need a profile; agent default persisted as a model-only `[agents.model_bindings]` entry; per-run override lives only in the command; selection recorded via `ReloadConfig` | the user needs to find models, not memorise aliases; a binding cannot freeze a file agent's prompt/permissions (whole-definition replace), and the profile gate prevents runs against an invented context window or price; discovery is non-reproducible metadata, so it stays out of the log | read-only roster + `$EDITOR` (no search), sparse `[[agents.definitions]]` override (freezes the definition), `AgentModelSelected` in the DB (splits model identity from its config profile), auto-adding every discovered id (guesses pricing/capabilities) |
| Agent loop | explicit step machine with budgets + approvals | auditable, testable, safe | free-form agent framework (opaque, unbounded) |
| Sub-agents | reuse OpenCode Markdown definitions; child runs on `mta`'s runtime; permissions restrict only | the roster already exists on disk; one execution/audit/budget model; untrusted-project gate | shelling out to `opencode` (bypasses budgets/approvals/audit), an mta-only format (duplicated prompts), field-level merge (opaque effective policy) |
| Catalogue discovery | multi-root: `.opencode/agents/**`, `.agents/agents/**` (+ `.agents/*.md`), globals in `~/.config/opencode/agents` and `~/.agents`, config last; narrow globs, and a skill directory is a black box except `SKILL.md` | that is where agents and skills actually live on a real machine; one scan, one roster surface; the narrow glob is what stops `references/*.md` being registered as agents | a single `.opencode/agents` root (misses the `.agents` convention), `.agents/**/*.md` (registers a skill's reference corpus as agents), scanning every `.md` under the project (slow, wrong) |
| Skills | a separate `kind` on its own `skill:<id>` stream: listed, trust-gated, **attachable** to a run at creation, never executable | a skill declares no model, permissions, or tools, so "running" one means inventing a policy a cloned file would then select; attachment still delivers the workflow's value | skills as agents (invented policy), skills ignored (loses the convention users have), skills as executable workflows (a runtime `mta` does not have), mid-run attach (prompt composition changes under a fixed stream version) |
| Board state | columns authoritative, `kind` drives automation | flexible names/lanes without losing semantics | fixed five statuses (inflexible) |
| Multi-machine | deferred, boundaries prepared | scope control; repo name reflects v0.2 | building transport in v0.1 |
| Frontend of engine | TUI embeds engine (v0.1) | simplest correct thing | daemon in v0.1 |

---

## 5. Milestones

Estimates assume one focused developer; each milestone ends with a demoable artifact and green CI.

### M0 — Bootstrap (2 days)

- Workspace, `rust-toolchain.toml` pinned to 1.94, edition 2024, `rust-version`, shared lints,
  committed `Cargo.lock`, CI (linux/macOS) running fmt, clippy, test, `cargo deny`, `cargo audit`,
  `cargo tree -d` (fail on duplicate terminal crates).
- `mta --version`, `mta doctor` stub, `mta config init`; license/deny configuration.
- Build hygiene baseline: `.gitignore`, size-aware profiles, `size-budget.toml`,
  `scripts/size-report.sh` / `prune-target.sh`, and a CI job running `size-report.sh --check`.
  Calibrate the placeholder budgets to the first real build.
- **Diagnostics baseline:** `mta-cli/src/logging/` (subscriber, console + rotating file sinks, the
  `directives`/`RUST_LOG` parser, the wrapped panic hook and `crash-<ts>.json` marker), `[logging]` in
  config, the `Secret` newtype and the no-secret property test, `mta-testkit`'s log capture, the
  `print_stdout`/`print_stderr` lint denies, the `cargo tree -p mta-domain` purity check, and
  `mta doctor --logs`. Measure and book the binary-size delta in `size-budget.toml`
  ([observability.md](observability.md), [ADR-0011](adr/0011-observability-and-diagnostics.md)).
- Minimal TUI that draws a frame, handles `q`, and restores the terminal on panic (smoke test).
- Review and accept the drafted ADRs ([adr/](adr/)): 0001 workspace boundaries, 0002 version pinning and
  pre-1.0 ratatui crates, 0003 engine-owns-store (as amended by 0009), 0004 Jev as a decision provider,
  0005 build-size budgets, 0006 OpenCode sub-agents, 0007 context compaction, 0008 mouse hit map,
  **0009 event-sourced core** — whose `docs/events.md` contract is an M1 deliverable — and **0011
  diagnostics**. ADR-0012 (task estimation) is accepted in M2 with the heuristic/event slice and refined
  in M6/M7.

**Exit:** clean checkout builds and tests; duplicate-dependency gate green; panic test leaves the
terminal usable; a secret passed through every log level appears in no captured record; `doctor --logs`
reports sink paths and resolved level; `scripts/size-report.sh --check` passes with budgets reflecting the
measured first build.

### M1 — Domain + event store (7 days)

- `mta-domain`: IDs, entities, column-kind guards, task/run state machines (§3.3), transition errors, and
  the **event and command enums** (`event.rs`, `command.rs`), including the estimation events/commands
  (`TaskEstimated`, `TaskEffortRecorded`, `EstimateTask`, `SetTaskEstimate`, `ClearTaskEstimate`,
  `RecordTaskEffort`). Unit tests + proptest: no illegal
  transition succeeds, all legal ones do, and every command's `decide` is total (either a valid event set
  or a typed rejection — never a panic).
- **Event store (the source of truth):** `events` envelope + append with expected per-stream `version`
  (optimistic concurrency), `commands` log with idempotency by `command_id`, `snapshots`,
  `projection_offsets`, per-connection pragmas, write/read pools.
- **Projection framework:** a `Projection` trait, incremental apply from an offset, truncate-and-rebuild,
  and the `board`/`tasks` projections first; position renormalisation and FTS with correct delete
  triggers derived from events. `mta projections list|verify|rebuild`, `mta events tail|show`,
  `mta replay --dry-run`, `mta snapshot take|list`.
- **Deciders:** `AddTask`/`EditTask`/`MoveTask`/`AssignTask`, with "one active root run per task" checked
  in `decide` (not by a DB index) and `StartRun`/`CancelRun` stubbed for M4.
- Compatibility/contract doc: **`docs/events.md` lands here** — it is the interface other crates code
  against, exactly as `providers.md` lands with M3 and `agents.md` with M5.5.
- CLI `task add|list|move|assign|show` + `board init`, now writing **commands** rather than rows; no AI
  yet.
- Store tests: migrate from empty, reopen, per-stream version contiguity, `skipping a version is
  rejected`, `same command_id twice ⇒ one event set`, projection rebuild is byte-identical,
  `snapshot + tail == replay from seq 1`, upcaster round-trip, GC below the watermark leaves projections
  unchanged.

**Exit:** create/move/list tasks entirely from the CLI against a temp DB **through the command path**;
dropping and rebuilding every projection reproduces the same board; replay of the CLI session's log is
byte-identical; invariant tests green; `EXPLAIN QUERY PLAN` reviewed for board queries.

### M2 — TUI shell + read-only board (6 days)

- `AppState`/`Action`/`update`/`view -> HitMap`, layout and breakpoints, event loop, terminal lifecycle
  (§3.8), keymap, filter parser, help overlay, palette skeleton, toasts.
- `BoardWidget`/`CardWidget` rendering from `BoardVm`; fixtures + a seed generator feed a
  `FakeEventSource` that emits domain events on the same stream the engine will publish — **the TUI is
  built against the event stream and view models, not the engine**, so it can land before the engine and
  de-risk layout/perf early. The seed generator literally reports a recorded event log fixture through
  the same fold path, which is also how M1's golden replay fixture gets reused.
- **Estimates — first slice:** the pure heuristic estimator in `mta-domain` (tags, description size,
  priority, dependencies, children; integer minutes, 15-minute rounding, clamps), `BoardVm` incremental
  per-column subtotals + leaf-only open total, `~`/`=`/`?` prefixes on cards, the header total
  (`Σ 38h (~7d)` where history allows), and the column subtotals. Read-only: no engine, no Jev, no
  calibration; `FakeEventSource` feeds `TaskEstimated` events so the fold and the layouts are proven at
  four terminal sizes before the engine exists. Confidence/degradation is snapshot-tested (`?`, low
  confidence, filtered totals, `<100`/`<80` column collapse).
- **Host metrics — first slice:** `HostMetrics` trait + `sysinfo = "=0.38.4"` impl, sampler task on
  `[host].sample_ms` feeding a `watch` channel, the compact status-line readout (`cpu %·mem %·disk %`)
  with warn colouring and the narrow-terminal degradation ladder, and the `S` overlay (per-core CPU,
  load, RAM/swap, per-mount disks, top processes, sparkline history). `FakeHostMetrics` drives the tests;
  the `SystemOverlay` is snapshot-tested at the same four sizes as the board, including the `stale`
  badge. This lands in M2 precisely because it touches the status line, the overlay stack, and the event
  loop that M2 already builds — and because M2 has no engine, it proves the sampler is independent of it.
- **Mouse:** RAII capture guard symmetric with bracketed paste (restored on exit/panic/SIGTERM), hit-map
  build during render, click-to-select card/column, wheel scroll with clamping, overflow scrollbars;
  click-equals-key, z-order, stale-map, and motion-flood tests.
- Snapshots at 4 sizes; 200-card frame benchmark; `NO_COLOR` and wide-char tests.

**Exit:** browse/filter/scroll a 200-card seeded board with keyboard or mouse under 16 ms/frame; every
clicked region triggers its documented key action; the status line shows live CPU/RAM/disk and `S` opens
full detail without the engine running; cards show automatic estimates (`~`/`=`/`?`) and the header and
column totals agree with the folded fixture; snapshots stable; no engine required to demo.

### M3 — Provider layer (5 days)

- `mta-provider`: config types, `ChatModel` trait, OpenAI chat client, SSE parser, tool-call delta
  assembly, retries/backoff, error normalisation, capabilities, `/models`, proxy/redirect policy.
- **System One (Jev)**: `DecisionModel` trait, `POST /v1/systemone` client, typed `noul`/`choice`/
  `score` request/response types, pre-flight and answer validation, IDR→USD cost conversion,
  `GET /v1/models?modality=systemone`, degradation/disable behaviour on 402/404.
- `mta models list [--modality systemone] [--search <query>] [--json] [--refresh]`, the shared pure
  model-query parser (fuzzy + `@provider` + `ctx>=` + capability/modality tokens), `mta doctor
  --provider` (chat probe + decision probe),
  `mta decide --state-file --questions` debugging command, hidden `mta chat "hello"` smoke command.
- `httpmock` conformance suite (§3.5, §3.5.1) with checked-in fixtures, including byte-split SSE,
  UTF-8-split cases, malformed answers, and a recorded real Jev response.
- Live smoke script (ignored by default) for Ollama (no key) + one hosted provider + Jev free model.

**Exit:** against local Ollama/vLLM and a hosted OpenAI-compatible endpoint, streaming text and a tool
call both work; a real `jev-1-13:free` call (when a key is present) returns validated `noul`, `choice`,
and `score` answers; malformed/stream-split fixtures pass; provider failure never panics or corrupts a
run.

### M4 — Tools + event-sourced run lifecycle (5 days)

- Tool runtime (`read/list/grep/write/edit/run_command/http_fetch/create_subtask/update_task/
  ask_human/finish`), canonical-path containment, process-group kill, timeouts, output caps, SSRF and
  proxy policy, secret scrubbing.
- **Effect runner:** subscribe to events, run effects (`CallChatModel`, `ExecuteTool`, `RunSubagent`,
  `WriteArtifact`, `EstimateTask` — the base heuristic + `inputs_hash` no-op lands here), submit results
  as commands, dedupe by `idempotency_key`, back off with a
  `provider_retry` deadline, and re-issue pending effects after a crash; `EffectFailed` for durable
  failures.
- Run lifecycle as events (`RunQueued/RunStarted/RunStepStarted/RunStepCompleted/RunSucceeded/RunFailed/
  RunCancelled/RunInterrupted/RunUsageRecorded`), workspace lifecycle, staging, cost accounting in
  micro-USD; `runs/messages/steps/approvals/artifacts` are projections of those events.
- `decisions` projection over `DecisionRequested/Answered/Failed/Applied/OutcomeRecorded`: answers,
  confidence, model version, latency, cost, `applied` and `outcome`, state retention policy
  (`full|excerpt|hash`).
- **Deadlines:** the `deadlines` projection, one timer wheel, and the `FireDeadline` command;
  `CrashRecovered` on startup (replay tail, expire orphaned approvals, mark interrupted runs) replaces
  the startup reconciliation sweep.
- `ScriptedModel` test double; direct tool-harness tests (escape attempts, symlinks, cancellation, output
  limits), effect-idempotency tests (crash mid-effect ⇒ exactly one result event), and projection
  invariant tests.
- CLI `mta run` plumbing **without** the agent loop (single scripted step) to prove wiring.

**Exit:** every tool has adversarial tests; every step leaves a contiguous event sequence regardless of
where it is killed; a crash mid-effect re-issues exactly once; interrupted runs are recovered from the
log with no sweep; no path writes outside the workspace without an `ApprovalRequested` event.

### M5 — Agent loop, approvals + context compaction (6 days)

- Agent loop as a step machine over events, budgets (steps/tokens/$/wall clock), cost estimation
  fallback, loop detector, JSON result envelope + repair retry, approval suspend/resume with hash
  verification and `approval_expiry` **deadlines** (no sweeper), retries as new run streams with a
  `retry_backoff` deadline.
- **Context assembly + auto-compaction (§3.6.2):** byte-calibrated token estimator with per-model EWMA,
  trigger `min(200K, 90% × context_window)`, pre-flight `ensure_context`, rolling summaries with exactly
  one active summary, protected tool-call groups, tool-result truncation with artifact references
  (`ContextTruncated`), deterministic fallback digest when the summariser fails, `ContextOverflow` → one
  emergency compaction + single retry, and a hard `blocked: context_overflow` guard before any oversized
  request. All of it expressed as events: `CompactionPerformed` / `CompactionSkipped` / `CompactionFailed`
  plus `MessageAppended{kind = compaction_summary}`, projected into `compactions`,
  `messages.compacted`, `runs.context_peak_tokens`, `ctx %` in the run panel, `C` / `:compact`,
  `mta doctor --context`.
- **Configurable policy (§3.6.2):** `auto_compact` master switch with per-run (`--compact`, TUI
  `:compact`), per-agent, and per-model overrides; `on_overflow` (`truncate` | `block`) and
  `summariser_failure` (`digest` | `block`) as independent behaviours; effective-value + source shown by
  `agents show`, `models list --context`, `doctor --context`, and the run badge; manual compaction always
  available while a run is active.
- `decide` tool: schema-validated pass-through to the decision model, confidence surfaced to the agent,
  full decision recorded as `DecisionRequested`/`DecisionAnswered` on the run stream (projected into the
  `decisions` read model and the run transcript).
- `mta run <task> --follow`, `mta approval list|approve|deny`, `mta decisions`.
- E2E tests with `ScriptedModel`: success, tool failure, budget exhaustion, cancellation, approval
  approve/deny/expiry (fake-clock deadline), restart with a pending approval, injection fixture, loop
  detection, compaction trigger/rolling/fallback/overflow-retry; decision tests with a scripted decision
  model (valid, under-confident, malformed, provider down).

**Exit:** a headless run on a sample task produces a transcript, artifacts, and a Review move; a
`decide` call is validated, persisted, and visible; every failure path leaves a terminal run event and a
replayable stream; cancellation stops in-flight work (including child processes) within 2 s.

### M5.5 — Sub-agents, skills, and file-defined agents (4 days)

- Discovery + parsing across all roots (ADR-0010): the agent roots (global `.agents/agents` + `.agents/*.md`,
  `~/.config/opencode/agents`, project `.opencode/agents`, project `.agents/agents` + `.agents/*.md`, legacy
  `agent/`, config) **and** the skill roots (global and project `.agents/skills/*/SKILL.md`); narrow globs
  and the black-box rule (a skill directory is never descended into); `ScanCatalogs` submitted by the
  watcher and by startup; nested IDs, precedence (including `.opencode/agents` vs `.agents/agents` within one
  directory), size/count/symlink limits, diagnostics; V2 `permissions` lists and V1 `permission` maps.
- Field mapping (`description`, `mode`, `model`, `steps`, `hidden`, `color`, `disabled`, V1 `tools`),
  model resolution, whole-definition replace, tombstones, the hash-bound trust gates (agents **and** skills).
- Skills as a kind: `SKILL.md` frontmatter + optional `agents/openai.yaml`, resource **names only**,
  `skill:<id>` stream and `skills` projection, `SkillTrusted`/`SkillTrustRevoked`, and the **refusal** to
  treat a skill as a delegation target.
- Skill attachment: `StartRun --skills`, `RunStarted.attached_skills[]` with bodies inlined,
  `runs.skill_snapshot_json`, the byte cap, the provenance header, prompt ordering, and the rejections
  (unknown/disabled/untrusted/over-cap) that must all fail without appending a run.
- `delegate` tool + child runs: parent linkage via `DelegateRequested`/`SubagentStarted`, fresh context,
  own budget/transcript, no scheduler slot, hierarchical cancellation, depth/cycle limits, result
  contract, child approvals, optional child `skills[]`.
- Persistence: the `agents`, `agent_trust`, `skills`, and `skill_trust` **projections** fed by
  `AgentDiscovered`/`AgentChanged`/`AgentUnavailable`/`AgentDefinitionReplaced`/`AgentTrusted`/
  `AgentTrustRevoked` and `SkillDiscovered`/`SkillChanged`/`SkillUnavailable`/`SkillTrusted`/
  `SkillTrustRevoked`; `agent_snapshot_json` and `skill_snapshot_json` carried on `RunStarted`;
  `runs.parent_run_id`/`depth` projected from the child run's events.
- CLI `agents list|show|validate|trust` (+ `--kind`) and `skills list|show|trust`; `doctor` discovery
  summary per kind; TUI catalogue roster (`A`) grouped by kind, the run-header skill chip, and collapsed
  child steps. Fixtures and property tests from [agents.md §11](agents.md#11-test-plan-m55).

**Exit:** running `mta` in a project with `.opencode/agents/*.md` **and** `.agents/` lists those agents and
skills with correct provenance and kind; a skill directory carrying `references/*.md` registers exactly one
skill and zero agents; a run delegates to an agent (approving the trust dialog first) with a child
transcript/budget/cancellation as specified, and attaches a skill whose text appears in the prompt and in
the log; a malformed or hostile file of either kind cannot crash startup, widen policy, or be silently run
as an agent.

### M6 — Interactive TUI (7 days)

- Card editor (title/description/priority/due/tags), assign picker, move/reorder with guards, typed
  confirmations, command palette actions, agent/model manager (read + enable/disable; create/edit via
  `$EDITOR` on config).
- **Model selection + search (ADR-0013):** the `M` catalogue and the shared `ModelPicker` (fuzzy query
  parser, filter tokens, ranking, `Ctrl-R` discovery refresh), a model row in the agent editor and the
  run dialog (per-run override), `ModelProfileForm` for unconfigured live ids, `ConfigEditor`
  (comment-preserving `toml_edit`, atomic write, hash check, `.bak` restore) plus
  `[agents.model_bindings]` → `ReloadConfig` → `AgentModelBound`, `mta models show|add|set|unset`, and
  `mta run --model`. Snapshot and pointer-parity tests for the picker and form.
- **Estimate capture + forecast:** the estimate field in the card editor (blank = auto, `2h` = pin,
  `-` = clear, `a` = recompute), the accept dialog's optional actual effort (which appends
  `TaskEffortRecorded`), the `T` forecast overlay (open effort by column/tag/agent, throughput,
  projected finish, run-time/cost forecast, the calibration table, the `?` list, and `e`/`E` bounded
  recompute with a confirmation), and pointer parity for its rows. `BoardVm` updates the header and
  column totals from the same `TaskEstimated` events it already folds.
- **Mouse in overlays:** palette rows execute on click, approval buttons dispatch the same guarded
  action (destructive approvals still confirm), outside-click closes a non-destructive modal, run-panel
  transcript scroll + step select/expand, scrollbar drag in long lists, hit-map parity tests.
- Run panel with virtualised live streaming, decision steps (`question → answer + confidence`),
  collapsed `subagent_call` steps with `Enter` → child transcript, child approvals inline, approval
  dialog (payload + hash-bound decisions + Jev's advisory verdict), cancel, retry, decision log overlay
  (`D`), **event log overlay (`E`, with causation chain and projection lag in the status line)**,
  **diagnostics log overlay (`O`: ring tail, level/target filters, search, follow, `:log <level>` live
  retune, `warn`/`error` badge)**, agent
  roster overlay (`A`, source/trust/warnings), **system monitor overlay (`S`) with host-pressure badges
  and the opt-in debounced `HostPressureObserved` breadcrumb**, Jev-ranked agent suggestions in the
  picker, digest overlay placeholder.
- Engine wiring in the binary; snapshots for every new state; keyboard harness; manual QA script
  (tmux, 40/80/120/200 columns, resize, paste, mouse on/off, SIGTERM, panic).

**Exit:** the full flow in §2.2 works entirely in the TUI, with keyboard or mouse; approval decisions in
the UI and from the CLI behave identically; no pointer path bypasses a confirmation; snapshots stable;
a model can be found by search, profiled if newly discovered, bound to an agent or used for one run, and
the choice survives a restart without hand-editing TOML.

### M7 — Orchestration (6 days)

- **Event-reactive scheduler:** recompute the ready set from relevant events (no tick), run slots, WIP
  policy, dependencies, auto-assign by skills, retries/failure policy, engine lease (projected from
  `LockAcquired`/`LockReleased`), and CLI control commands appended to the durable command log.
- **Timer wheel + deadlines:** one wheel armed from the `deadlines` projection; `FireDeadline` →
  approval expiry, retry backoff, run timeout, `schedule_due`; `CrashRecovered` replaces startup
  reconciliation.
- Decision policies: spec gate before auto-start, Jev-ranked auto-assign, priority inference, failure
  triage, confidence thresholds, `apply_*` switches, decision `outcome` recording, deterministic
  fallback when Jev is disabled/unavailable/under-confident.
- **Estimation policy:** calibration and refinement in the `EstimateTask` effect built in M4
  (actual/run-terminal/decision triggers, one refinement in flight, no sweep), the `estimate_accuracy`
  projection from `TaskEffortRecorded` + run terminal events with the fixed EWMA constants, throughput
  and projected finish, the optional `purpose = "task_estimate"` Jev `score` refinement, the CLI
  (`mta estimate`, `mta task estimate|actual`, `mta estimate recompute`), and `mta doctor --estimate`.
  Tests: golden heuristic tables, calibration fixtures, insufficient-history honesty, Jev-disabled
  fallback, and an idle board appending nothing.
- Cron schedules + task templates with dedupe (`ScheduleRegistered`/`ScheduleDue`/`ScheduleFired`/
  `ScheduleSkipped`); budget reservation (`RunBudgetReserved`); daily/monthly windows via `DayRolled`;
  standup digest (model + deterministic fallback) including decisions, low-confidence rate, and spend.
- TUI: schedule view, budget/approval badges, digest rendering.
- Tests: scheduler determinism under event replay (same log ⇒ same start order), deadline determinism
  with a fake clock, WIP/dependency matrices, budget boundaries and estimated costs, digest accuracy
  against a seeded log.

**Exit:** an overnight simulated schedule creates and runs tasks with no polling loop anywhere in the
engine; the daily cap stops new work cleanly and visibly; digest matches seeded truth; killing the engine
and restarting reproduces the same ready set and re-arms the same deadlines; an idle board appends no
estimation events and `mta estimate` agrees with the TUI header.

### M8 — Hardening and release (4 days)

- **Log operations:** `mta projections verify` (scratch rebuild + diff) and `rebuild`, `mta replay
  --dry-run` diffing before commit, per-stream snapshots under a real load, `mta gc` with the retention
  watermark and the `mta run export` precondition, and a projection-rebuild time budget.
  **Diagnostics operations:** `mta doctor --logs`/`--bundle`, rotation and the `max_total_mb` prune under
  a synthetic `debug` flood, log-dir usage reported against the caps, and a redaction audit of the bundle
  (a support archive that leaks a key is a release blocker).
- Crash/recovery: lease takeover, orphaned workspaces, backup/restore command, `CrashRecovered` exercised
  by killing the process mid-run and mid-effect.
- Performance pass (200 cards, 10k events, 5k messages), startup < 300 ms including cold projection
  check, replay throughput (events/s), memory sanity, release binary within the size budget.
- Docs: README with asciinema demo, provider guide, config reference, **event-sourcing contract pointer
  ([events.md](events.md)) and a "why is this card like this?" walkthrough**, approval/security model,
  troubleshooting (including "which log do I read?" — [observability.md](observability.md)),
  contributor guide.
- Packaging: `cargo dist` (linux x86_64/aarch64, macOS), shell completions, man page; CI runs
  `cargo audit`/`deny`.
- Optional: Dockerfile for headless scheduler.

**Exit:** v0.1 tag; fresh-machine install → configure provider → run a task in under 10 minutes; a
deliberately corrupted projection is caught by `verify` and repaired by `rebuild`; release checklist in
§8 signed off; migrations pass from empty and from every released tag (there is no prior schema before
v0.1).

**Total:** 52 engineer-days itemised (M0 2 · M1 7 · M2 6 · M3 5 · M4 5 · M5 6 · M5.5 4 · M6 7 · M7 6 ·
M8 4), **~60 with a 15% buffer**. Event sourcing is the single biggest reason M1 and M8 grew
(+4 and +1): the event store, deciders, projection framework, replay/rebuild tooling, and upcasters are
front-loaded infrastructure that the later milestones then consume. M0 grew by 1 (+1 day) for the
diagnostics baseline — subscriber, sinks, filter parser, redaction and its property test, the panic-hook
crash marker, and `doctor --logs` — because a log you cannot trust is worse than none, and wiring it at
bootstrap is what keeps the `info`-level discipline (bounded per run) from decaying into noise later.
M2 grew by 1 (+1 day) for the host
sampler, status-line readout, and `S` overlay, which were folded into a milestone that already builds the
status line, overlay stack, and event loop rather than opening a milestone of their own; M5.5 grew by 1
(+1 day) for the `.agents` roots and the skill kind, which are the same scan, the same trust gate, and the
same roster surface as the agents that milestone already delivers. M2, M6, and M7 each grew by 1 (+3 days)
for auto-estimation: the heuristic and the read-only card/column/header totals in M2, overridable
estimates plus the `T` forecast overlay in M6, and calibration, the Jev refinement, throughput, and the
`mta estimate` CLI in M7. M6 grew by 1 (+1 day) for searchable model selection: the shared query parser
lands in M3, while the picker, profile form, and the atomic config-writer path (the part that must not
corrupt a user's TOML) land in M6. Provider quirks, terminal plumbing,
and restart semantics are the usual overruns; they are inside these tasks, not extra scope.
The critical path is M1 → M3 → M4 → M5 → M5.5 → M6. With two people, M2 can run alongside M3 and M8 docs
can start during M6/M7.

### M9 — Future (v0.2+, not in this plan's commitment)

`mta daemon` (engine + HTTP/IPC API, TUI attaches as client), remote workers with the run protocol
(claim/lease/heartbeat, artifact upload), shared board over Postgres, MCP tool servers, integrations
(mail/calendar/docs), multi-user approvals. The domain/store/engine boundaries above are chosen so
this phase adds transport, not a rewrite.

---

## 6. Test strategy

| Layer | Approach |
|---|---|
| Domain | Unit + proptest over transition graphs; guard truth tables; serde round-trips |
| Event log & replay | `snapshot + tail == replay from seq 1`; projection rebuild is byte-identical; per-stream version contiguity and a skipped version rejected; `seq` monotonic with gaps tolerated; same `command_id` twice ⇒ one event set; optimistic-concurrency conflict rejects without appending; crash mid-effect re-issues exactly once; upcaster round-trip; deciders are clock/random/IO-free (a lint plus a property test over a fixed command sequence); a golden fixture log replays to a known board |
| Projections | `projection_offsets` never move backwards; a truncated rebuild matches the incremental one; `verify` detects a hand-corrupted row; FTS delete triggers rebuild the same index as a full rebuild |
| Store | Temp-dir SQLite, migrations from empty, reopen, FK/cascade, FTS, position renormalisation |
| Provider | `httpmock` fixtures (success, streaming edge cases, errors, retries), no network in CI |
| Decisions | fixtures for `noul`/`choice`/`score`/malformed answers, 402/404/429, timeouts; validation unit tests; `ScriptedDecisionModel` for policies; per-purpose policy tests (suggest vs apply thresholds) |
| Estimation heuristic | golden tables for tags, priority, dependencies, description steps, and child rollup; 15-minute rounding and the `[min_minutes, max_minutes]` clamp; monotone in description length; same inputs ⇒ same output; an `estimator_version` bump changes only new estimates; properties: never negative, never above the cap, never `0` for an estimated leaf |
| Estimation events & display | `TaskEstimated` is idempotent by `inputs_hash` (a second auto estimate appends nothing); a manual pin survives an auto re-estimate and an unrelated edit; clear → `?`; replay/rebuild reproduces every estimate **without invoking the estimator**; leaf-only totals count a parent+child once; filtered total reads `12h/38h`; `~`/`=`/`?` semantics; low-confidence and `insufficient history` labels; `T` overlay snapshots at four sizes; `doctor --estimate` agrees with the header |
| Estimate calibration & forecast | EWMA fixture with the fixed alpha and ratio clamp; a rebuild of `estimate_accuracy` is byte-identical; an actual with no prior estimate is ignored; run forecast groups by `(agent, model, tag)`; a finish date appears only after `throughput_min_days`; Jev `score` mapping at bucket boundaries; a disabled/under-confident/402 Jev leaves the heuristic and appends no second estimate |
| Agent definitions | fixtures for V2 lists + V1 maps, precedence/tombstones, nested IDs, malformed/hostile files; property test that agent rules never widen global policy; model-resolution matrix; trust-per-hash re-prompt |
| Model selection & search | query-parser table (free terms, `@provider`, `ctx>=`/`<=`/`=`, `tools`, `-negation`, quoted phrase, unknown token → free text); ranking is deterministic and favours exact alias → prefix → word boundary → subsequence, then configured/available/recent; discovery cache honours the TTL and `Ctrl-R`, a provider 401 leaves configured rows usable; an unconfigured row cannot be chosen until `ModelProfileForm` saves; picker snapshots at four sizes (empty/loading/many/no-match); click and key produce the same action; per-run override appears in `RunQueued` and is gone after restart, while a binding survives a reload; unknown, unprofiled, and wrong-modality aliases are rejected; a model without `tools` when the agent has tools warns in the run dialog |
| Config writer | comment/ordering preservation through `toml_edit`; validation happens before any rename; atomic replace + `.bak`; a hash mismatch after an external edit aborts without writing; a reload failure restores the backup and appends nothing; read-only/symlinked files are refused; no `api_key`/`Authorization` text is ever written; `[agents.model_bindings]` diff emits `AgentModelBound` once and an identical reload emits nothing; replay reproduces `effective_model` and `model_source` |
| Catalogue discovery (agents + skills) | all four roots and the documented precedence, including `.opencode/agents` vs `.agents/agents` vs `.agents/*.md` in one directory and `list --all` shadow reporting; **the black-box regression test** — a skill directory carrying `references/chapter_0N.md` yields exactly one `kind = skill` entry and zero agents, which fails the moment the glob widens to `.agents/**`; `ScanCatalogs` twice unchanged appends nothing; skill frontmatter `name` vs directory mismatch; `agents/openai.yaml` and `openai.yml`; oversized `SKILL.md`; an unrecognised skill root (`.opencode/skills/…`) reported once and not read |
| Skill attachment | `StartRun --skills` records `RunStarted.attached_skills[]` with bodies and `runs.skill_snapshot_json`; replaying that log needs no file on disk and is byte-identical after the `SKILL.md` is edited; over-cap, unknown, disabled, untrusted, and hash-mismatched attachments are each rejected without appending a run; a delegated child gets skills only when passed; `delegate` targeting a skill fails fast |
| Delegation | `ScriptedModel` child runs: success/failure/budget/cancel/depth/cycle/child-approval, no scheduler-slot deadlock, result truncation |
| Context compaction | estimator monotonicity + never-underestimates-by->15% on fixtures, trigger boundaries (179_999/180_000 for a 200K model; 199_999/200_000 for a 1M model), tool-call groups never split, system prompt + task + pending approval always retained, single active summary after N compactions, summariser 500 → `digest` fallback, provider `ContextOverflow` → one compaction + retry then `blocked`, no compaction mid-stream, compaction event sequence replays to exactly one summary on injected crash |
| Compaction policy config | `auto_compact = false` → no summary call but the request guard still holds; `on_overflow = block` → `blocked: context_overflow`; `summariser_failure = block` → `blocked: summariser_unavailable`; precedence run > agent > model > global for the effective trigger; `--compact=off` on a run beats a per-agent `auto_compact = true`; manual `C` compacts while auto is off; `doctor --context` reports the winning level |
| Engine | `ScriptedModel` + temp workspace; budgets, cancel, approvals, injection, loop detection, deadline firing under a fake clock |
| Deadlines & recovery | fake clock drives `retry_backoff`/`approval_expiry`/`run_timeout`/`schedule_due`; killing the process between append and projection replays cleanly; `CrashRecovered` expires orphaned approvals and marks interrupted runs; re-armed deadlines after restart match the log |
| Retention & GC | `gc` never prunes above the watermark; a pruned stream still rebuilds its projections from snapshots; `mta run export` precondition blocks pruning an unexported run; `doctor --events` reports lag and pending effects |
| TUI | `TestBackend` + `insta` snapshots; key-sequence harness; layout at 4 sizes; `NO_COLOR` |
| Mouse & hit map | rect→region resolution with z-order (overlay beats board, later draw wins), click yields the same `Action` as the documented key for every region, wheel clamping and no overscroll, scrollbar drag math, stale map after a resize is dropped, motion flood ignored unless `hover`, destructive approval click still opens the confirm dialog, mouse capture released on panic/SIGTERM/exit |
| Host metrics | `FakeHostMetrics` drives the status line and `S` overlay snapshots; first sample discarded (CPU needs a delta); warn thresholds and the header badge; the narrow-width degradation ladder (full readout → worst-metric glyph → omitted); synthetic mounts (`tmpfs`/`efivarfs`/`overlay`) excluded from the disk list; a stalled sampler shows `stale` and never blanks the widget; the sampler is absent from the event log (a full replay asserts zero host events beyond an enabled `HostPressureObserved`); `observe_pressure` is rising-edge only and respects its cool-down |
| Diagnostics | no-secret property test across every level with `log_prompts`/`log_tool_output` enabled; with the alternate screen active, zero bytes reach stdout/stderr and a simulated panic writes the crash marker and restores sinks; a full successful run emits ≤ N `info` records; records for a fixture run carry `run_id`/`seq`/`command_id` that join to the log; a synthetic flood respects `max_file_mb`/`retain_files`/`max_total_mb` with a prune `warn`; directive/`RUST_LOG` parser tables incl. unknown level → exit 2; `json` output parses and contains the documented fields; `cargo tree -p mta-domain` has no `tracing` |
| CLI/e2e | `assert_cmd` + scripted provider env var; golden transcript/DB assertions |
| Manual | tmux QA script per milestone (resize, paste, suspend, panic recovery, slow provider) |

CI gates: `fmt --check`, `clippy -D warnings`, `test`, `cargo deny check`, `cargo audit`, migration
test, `cargo insta test --check` (`INSTA_UPDATE=no`), `cargo tree -d` (fail on duplicate terminal
crates), and `scripts/size-report.sh --check` (target + release-binary budget). No live-provider calls
in CI; a nightly optional job runs the live smoke script against Ollama.

**Test doubles are first-class:** the deterministic fake chat model and fake decision model are part of
the public test kit so users can reproduce agent behaviour without spending tokens
(`mta run --dry-run --script file.json`).

---

## 7. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| OpenAI-compatible servers differ (tools, usage, SSE framing) | Runs fail or hang | Minimal wire subset, capability flags + degrade path, conformance fixtures, `doctor` probe, documented matrix |
| Runaway agent (loops, cost) | Money/time loss | Hard per-run budgets, loop detector, daily cap, visible cost, kill switch, supervised default |
| Over-trusting Jev decisions | Wrong routing/priority, automation bias | Advisory by default, `min_confidence`, explicit `apply_*` switches, show model+confidence, record outcomes for calibration, never auto-approve side effects |
| Estimates are wrong, stale, or silently drift | Bad planning, lost trust in the numbers | Provenance + confidence on every card, `?` rather than `0` when unknown, human effort and agent time calibrated from separate evidence, a manual pin that auto-estimation cannot overwrite, `insufficient history` instead of a fake finish date, and no gating of execution on an estimate; `mta estimate recompute` is explicit and `doctor --estimate` shows the sample counts |
| Estimation becomes a hidden poll or log spam | Event log churn, Jev spend, UI jitter | The estimator is event-driven (`EstimateTask` effect) with no sweep; identical `inputs_hash` appends nothing; one decision refinement in flight per card; `refine_with_decision` is a switch; a full replay reads stored estimates and never re-estimates |
| A picker edit corrupts the user's config | Lost config, broken startup, distrust in the UI | One `ConfigEditor` writer: validate before write, `toml_edit` to preserve comments/formatting, atomic temp+rename, one `.bak`, and a hash check that aborts on an external edit; a failed reload restores the backup; the editor refuses a symlink pointer swap and preserves the file mode; tests cover each failure |
| Live discovery is stale, slow, or leaks a key | Wrong model list, UI stalls, credential exposure | Discovery is opt-in, TTL-cached in memory, refreshed on open or `Ctrl-R`, never per keystroke and never a background poll; provider errors are per-row while configured models stay usable; requests reuse the provider's auth path and headers are never logged; no catalogue event, so replay is unaffected |
| A discovered model has unknown pricing/capabilities | Surprise cost or a silently degraded run | An unconfigured id cannot start: the profile form requires a context window and defaults capabilities conservatively; unknown pricing is shown as `unknown` (0), and the run dialog warns when the chosen model lacks tools or is too small for the prompt |
| Jev model id/contract drift (free ids change, MCP default id is stale) | Decisions fail | Resolve ids from `GET /v1/models?modality=systemone`, pin alias in config, `doctor` probe, fail-closed validation, deterministic fallback |
| Decision state leaks sensitive task content | Privacy | Explicit state assembly, redaction, `store_state` hash/excerpt modes, document provider data flow |
| Prompt injection via fetched content | Unauthorised actions | Untrusted-content wrapping, tool allowlist, approvals for side effects, injection tests |
| Untrusted project agent prompts (`.opencode/agents/*.md`, `.agents/agents/*.md`) | Privilege escalation, exfiltration | Hash-bound trust gate before first delegation, permissions restrict-only, provenance header, sandbox, injection fixtures |
| Untrusted project skill bodies (`.agents/skills/*/SKILL.md`) | Prompt injection into every run that attaches it | Same hash-bound trust gate as agents but at **attach time** (a run cannot start with an unapproved or hash-changed skill), provenance header naming the file and hash, `[skills].max_attached_bytes` bound, `skill` permission restricts which agents may attach what, and the text is visible in the run header for the human |
| A skill's `references/` corpus is scanned or attached as agents/prompt | Roster pollution, unbounded prompt, wasted scan | The agent glob is `.agents/agents/**` and never `.agents/**`; a skill directory is a black box except `SKILL.md` + `agents/openai.yaml`; resources are recorded by name only; a regression test fails if the glob widens |
| Attached skill bodies bloat the log | The event log's one unbounded table grows faster than expected | Bodies are bounded (64 KiB total per run, enforced at attach time with a hard rejection), a run attaches only at creation so there is no per-step growth, and `SkillChanged` is one event per edit rather than per scan |
| Runaway delegation (depth, fan-out, cost) | Spend/time blowup | Depth 1 default + cycle check, per-parent child limits, sequential children by default, parent-remaining-budget ceiling, visible nested spend |
| Context overflow on long runs | Provider 400s, aborted runs, wasted tokens | Pre-flight trigger `min(200K, 90% × window)`, calibrated estimator with 1.08 safety factor, rolling summaries (one active), tool-result truncation with artifact refs, emergency compaction on `ContextOverflow` with a single retry, hard `blocked` before an oversized request, `ctx %` visible in the run panel |
| Compaction misconfiguration across four override levels | Runs compact too early/late or block unexpectedly | `auto_compact` switch with independent `on_overflow` / `summariser_failure` actions; replace-not-merge overrides; effective value + winning level printed by `doctor --context`, `models list --context`, `agents show`, and the run badge; precedence and off-semantics covered by dedicated tests |
| Mouse capture breaks terminal selection or leaks on crash | User cannot drag-select after exit; shell left in mouse mode | Capture toggled by the same RAII guard as bracketed paste and restored on panic/SIGTERM/exit; `Shift`+drag passthrough documented; `mouse = false` gives a pure-keyboard session with native selection; `y`/OSC-52 copy; no mouse-only features, so keyboard parity is a test gate |
| OpenCode agent format drift (V1/V2 fields) | Roster stops loading or silently loses rules | Versioned contract in [agents.md](agents.md), fixtures per supported shape, V1 fallback, warnings not crashes, unknown fields preserved |
| TUI becomes an unmaintainable god-object | Slows delivery | Strict `update`/`view` split, view models, snapshot tests, custom widgets isolated |
| Narrow/small terminals, CJK widths | Broken layout | Width-aware truncation, breakpoints, wide-char snapshot tests |
| ratatui/crossterm version drift | Build breakage | Pin `ratatui = 0.30.2`, `ratatui-textarea = 0.9.3`, crossterm 0.29; committed lockfile; `cargo tree -d` gate; ADR for upgrades |
| Host-metrics dependency forces an MSRV bump (`sysinfo` 0.39 needs Rust 1.95, workspace is 1.94) | Surprise MSRV break in CI or a rushed toolchain bump | Pin `sysinfo = "=0.38.4"` (MSRV 1.88); the upgrade is a deliberate change that moves `rust-version` and `rust-toolchain.toml` together, and `cargo tree -d` catches duplication |
| Host sampling becomes a disguised poll or corrupts replay determinism | Log bloat, non-deterministic rebuilds, wasted CPU | Samples are ephemeral (`watch` channel, no log), sampling is decoupled from `repaint_ms`, CPU needs two refreshes so the first sample is dropped, `sample_ms` is clamped, and only an opt-in rising-edge debounced `HostPressureObserved` may be logged; a replay test asserts no other host event exists |
| SQLite lock contention / corruption | Data loss | One-connection write pool + read pool, per-connection pragmas, WAL, busy timeout, `quick_check` in doctor, backup command |
| Event log grows without bound (the only unbounded table) | Disk exhaustion, slow replay | `[events] retention_days` watermark GC that never prunes above a projection offset or a stream's last snapshot; `mta run export` mandatory before pruning that run; `doctor --events` reports growth; `snapshot_interval` bounds replay cost; size budgets cover the data dir |
| Replay is silently non-deterministic (a clock, RNG, or DB read inside a `decide`) | The log stops being the source of truth and rebuilds diverge | Deciders take time/ids from the command and store them in the payload; no clock/random/IO allowed in `decide` (lint + review gate); replay-determinism and `snapshot + tail == full replay` tests; `projections verify` diffs a scratch rebuild |
| Debugging moves up a level of indirection | Slow diagnosis, "where did this come from?" | `mta events show <id>` renders the correlation/causation chain; the TUI event-log overlay (`E`) shows the same; the diagnostics overlay (`O`) shows what the code was doing while those events were appended, greppable by `run_id`/`command_id`; `mta doctor --events` surfaces unreplayed events and pending effects; a documented "why is this card like this?" walkthrough in M8 |
| Diagnostics leak a secret (or the support bundle does) | Credential or private-data exposure in a file that gets shared | `Secret` type redacts on `Debug`/`Display`/`Serialize`; keys logged by `api_key_env` name only; `Authorization` never formatted; prompts and tool output opt-in; a property test asserts no secret at any level even with both opt-ins on; `doctor --bundle` is a redaction-audited artifact |
| A chatty `debug` session fills the disk | Disk exhaustion from diagnostics | Diagnostics are bounded like `target/`: `max_file_mb` early rotation, `retain_files`, a hard `max_total_mb` prune with a `warn`, lossy non-blocking writes that never stall the engine, nothing per token above `trace`, and `info` bounded per run by test |
| Projection lag looks like a UI bug | Confusing staleness, duplicated work | `projection_offsets` drives a status-line lag figure; the UI resumes from its own `last_seq` on `Lagged` rather than guessing; `verify` distinguishes "stale" from "wrong" |
| Schema evolution breaks old logs | Unreadable history, failed replay | Add-only payloads with `schema_version` per type, upcasters applied on read during replay, round-trip tests, and a rule that a fold tolerates unknown added fields but never guesses at changed meaning |
| `target/` grows unbounded / bloats CI caches | Disk exhaustion, cache eviction | size-aware profiles, `size-budget.toml`, `size-report --check` gate, `prune-target.sh`, separate coverage/dist target dirs |
| Silent zero-cost runs on usage-less local servers | Budget never trips | Token estimation fallback + `cost_estimated` flag, conservative multiplier, budget tests with a no-usage fixture |
| Restart/cross-process approval and run state | Runs stuck or double-executed | Commands validated in `decide` with expected per-stream version (reject without appending), hash-bound approvals, `CrashRecovered` replay + deadline re-arming, durable command log with `command_id` idempotency — no sweeper, no notify+poll |
| Secrets in logs/transcripts | Leak | Env/keyring first, redaction pass, no header logging, 0600 files, deny plaintext keys in docs |
| Scope creep toward multi-machine | v0.1 slips | Explicit non-goal, transport boundaries only, M9 gated on v0.1 metrics |
| Event sourcing front-loads infrastructure before visible feature work | M1 is larger and the first demo arrives later; temptation to cut corners on determinism | M1 is scoped to the log + deciders + `board`/`tasks` projections only (no AI), the CLI proves it end-to-end, and M2 can be built against a recorded event fixture in parallel |
| Non-deterministic LLM tests | Flaky CI | Scripted model for all engine/TUI/e2e tests; live tests opt-in |

---

## 8. Ordered backlog (first PRs)

1. `chore: workspace, rust-toolchain 1.94, profiles, size budgets/scripts, CI, lints, deny/audit, duplicate-dep gate`
2. `feat(domain): ids, task/board/agent/run types + complete state machines + event/command enums (+ proptest)`
3. `feat(store): event log + envelope, command log, projection framework with offsets, snapshots, FTS, board/tasks projections, GC watermark (+ replay/rebuild tests)`
4. `feat(cli): task/board commands through the command path; events tail|show, projections list|verify|rebuild, snapshot, replay --dry-run`
5. `feat(tui): app loop, layout, keymap, terminal lifecycle, snapshots`
6. `feat(tui): read-only board + cards + filters + 200-card bench (+ fixtures)`
7. `feat(tui): host metrics — HostMetrics trait + sysinfo sampler, status-line readout, S overlay (+ fake sampler tests)`
8. `feat(provider): config, types, capability flags, error normalisation`
9. `feat(provider): chat client + SSE parser + tool-call assembly (+ fixtures)`
10. `feat(provider): models list, doctor probe, retry/backoff`
11. `feat(provider): System One (Jev) decision client, typed validation, cost conversion (+ fixtures)`
12. `feat(engine): tool runtime — containment, SSRF/proxy policy, process groups (+ adversarial tests)`
13. `feat(engine): effect runner (at-least-once, idempotent, backoff deadlines), event-sourced run lifecycle, `EstimateTask` heuristic effect, decisions projection, workspace lifecycle, cost accounting, CrashRecovered (+ idempotency tests)`
14. `feat(engine): agent loop as a step machine over events, budgets + estimation, loop detector, result envelope`
15. `feat(engine): decide tool, approvals (hash-bound, deadline expiry), retries as new run streams, headless run + approval/decision CLI (+ e2e)`
16. `feat(engine): multi-root catalogue discovery — .opencode/agents + .agents/agents + .agents/*.md + globals, black-box skill dirs, SKILL.md parsing, skill:<id> stream + skills projection, trust gates for both kinds (+ fixtures incl. the references/ regression)`
17. `feat(engine): skill attachment — StartRun --skills, RunStarted.attached_skills[] with bodies, runs.skill_snapshot_json, byte cap, provenance header, rejections (+ replay-after-edit test)`
18. `feat(engine): delegate tool + child runs — parent linkage, budgets, cancel, depth/cycle (+ e2e)`
19. `feat(engine): context compaction — calibrated estimator, rolling summaries, overflow retry (+ fixtures)`
20. `feat(tui): card editor, assign picker with Jev ranking, palette, move guards, agent/model manager`
21. `feat(tui): run panel with decision steps, child runs, decision log, approval dialog, cancel/retry`
22. `feat(tui): mouse hit map — click, double-click, wheel, scrollbar drag, hover (+ parity tests)`
23. `feat(engine): event-reactive scheduler, timer wheel + deadlines, run slots, WIP policy, dependencies, auto-assign`
24. `feat(engine): decision policies (spec gate, priority, triage, apply thresholds), cron schedules, budget, digest`
25. `feat(cli): run export, backup, GC + retention watermark, doctor --events; docs + packaging; security review`
26. `feat(tui): event log overlay (E) with causation chain and projection lag in the status line`

27. `feat(engine): compensating commands for undo (`mta task undo`) once the log is live (post-v0.1)`
28. `feat(cli): diagnostics baseline — tracing subscriber, console + rotating file sinks, directive/`RUST_LOG` parser, `Secret` redaction + no-secret property test, panic-hook crash marker, mta-testkit log capture, `doctor --logs` (M0, [observability.md](observability.md))`
29. `feat(domain,tui): task estimation — heuristic estimator + `estimator_version`, `TaskEstimated`/`TaskEffortRecorded` events, `EstimateTask`/`SetTaskEstimate`/`ClearTaskEstimate`/`RecordTaskEffort` commands, `tasks` columns, card/column/header totals (+ property, golden, snapshot tests)` (M2)
30. `feat(tui): estimate editor (auto/pin/clear), actual-effort capture on accept, `T` forecast overlay with calibration and unestimated lists, pointer parity (+ snapshots)` (M6)
31. `feat(engine,cli): estimate calibration from actuals and run terminals, throughput + projected finish, Jev `task_estimate` refinement, `mta estimate`/`mta task estimate|actual`/`doctor --estimate` (+ fixtures)` (M7)
32. `feat(provider,cli): model-query parser (fuzzy + @provider + ctx/capability tokens), ranked `mta models list --search|--json`, `mta models show|refresh` (+ parser/ranking tables)` (M3)
33. `feat(engine,tui): searchable ModelPicker + ModelProfileForm, per-run override, `[agents.model_bindings]` → `AgentModelBound`, atomic `ConfigEditor` with hash check/.bak restore, `mta models add|set|unset` and `mta run --model` (+ snapshots, pointer parity, config-writer tests)` (M6)

Item 28 was added after this list was first cut, so it is numbered last to keep the numbers stable even
though it is M0 work: it is the diagnostics half of the bootstrap, and item 1's workspace/CI frame is
where it lands. Items 29–31 keep the same append-only numbering for the estimation feature; they land in
M2, M6, and M7 respectively, in the order above. Items 32–33 append the model-selection work: the shared
query parser and CLI search ride with the provider layer in M3, and the picker, profile form, and config
writer land with the interactive TUI in M6 ([ADR-0013](adr/0013-model-selection-and-search.md)).
**Definition of done (v0.1)**

- [ ] All §1.1 goals demonstrated by the demo script; no P0/P1 bugs open.
- [ ] Opening `mta` in a project with `.opencode/agents/*.md` lists those agents with provenance;
      a delegation runs as a child run with its own budget/transcript, a hostile agent file cannot widen
      policy or bypass the trust gate, and a malformed file never prevents startup.
- [ ] A long run against a 200K-window model compacts automatically before 200K prompt tokens, keeps a
      single rolling summary plus recent context, survives a summariser outage with a marked fallback,
      and blocks rather than sending an oversized request.
- [ ] Jev `noul`/`choice`/`score` decisions work against the configured provider; a malformed answer
      fails closed, and a low-confidence answer produces a human prompt rather than an action.
- [ ] Decision policies are advisory by default; `apply_*` paths and deterministic fallbacks are tested.
- [ ] Every persisted change in the demo is a validated command appended to the log; dropping and
      rebuilding every projection reproduces the board byte-for-byte (`mta projections verify` clean).
- [ ] The demo log replays deterministically (`mta replay --dry-run` clean; `snapshot + tail == full
      replay`), and no engine behaviour depends on a polling cadence: killing the engine mid-approval and
      restarting re-arms the same deadlines, re-issues pending effects exactly once, and resumes the same
      run.
- [ ] `mta events show <id>` explains a card's current state through its correlation/causation chain,
      and the TUI `E` overlay shows the same stream with projection lag in the status line.
- [ ] A failing run is diagnosable from the file log alone: its records carry the `run_id`/`command_id`
      that join them to `events`, retry attempts and durations are visible, no configured secret appears
      at any level (property test), the TUI wrote nothing to the terminal, and `mta doctor --logs` reports
      the resolved level, directives, sinks, and usage against the rotation caps.
- [ ] `cargo test` green on linux + macOS; clippy/deny clean; snapshot tests committed.
- [ ] `scripts/size-report.sh --check` passes; `target/` and the release binary are within the
      committed budgets, and a deliberate budget change is documented.
- [ ] Fresh install → configured provider → completed task documented in README.
- [ ] Terminal restored after panic, SIGINT, and provider hang (manually verified), **including mouse
      capture and bracketed paste**; with `[ui].mouse = true`, every documented keyboard action is
      reachable by pointer without bypassing a confirmation, and `mouse = false` loses no functionality.
- [ ] Running `mta` in a project with `.opencode/agents/*.md` **and** `.agents/` lists both kinds with
      provenance: agents in the roster, skills listed as attachable and **never** offered as an executor; a
      skill directory's `references/*.md` do not appear as agents; an untrusted skill cannot be attached
      until approved, and a run records the exact skill text it used.
- [ ] The status line shows live CPU/RAM/disk and `S` opens the full per-core / per-mount / top-process
      detail; a stalled sampler degrades to `stale` rather than blanking the widget; no host sample ever
      appears in the event log (`observe_pressure` off by default), and `mta doctor --host` agrees with
      the overlay.
- [ ] Every open card shows an effort estimate (or an explicit `?`); the header and column totals match
      `mta estimate`; a manual pin survives edits and auto re-estimation, and clearing it returns the card
      to automatic control; `T` shows throughput and a projected finish only when the history supports it;
      `mta replay --dry-run` reproduces every estimate without invoking the estimator or Jev.
- [ ] `M` opens a searchable model catalogue, a query like `ctx>=100k tools @openai` filters it, and a
      selection either sets an agent binding or applies to one run; choosing a live-discovered id that
      has no profile opens the profile form instead of starting a run, the choice survives a restart, and
      `mta agents show` reports `(binding)`/`(definition)`/`(inherit)` plus the shadowed value; killing
      the picker mid-write never leaves a partially written config.
- [ ] DB migration from empty and from every previously released tag passes (no prior tag exists at
      v0.1; the rule starts with v0.1.1).
- [ ] Security review of tool policies, secret handling, and injection tests signed off.
- [ ] Every model, tool, and decision call for the demo is visible in the exported run bundle
      (`mta run export <id>`), which is also the artifact preserved before any `mta gc` pruning.

---

## 9. How v0.2 multi-machine will attach (design constraint, not work)

- `mta-engine` already exposes: `Command`, `ChatModel`, `ToolRuntime`, the `EventLog`/`CommandBus`/
  `Projection` traits, and an event stream. v0.2 adds: `mta-daemon` (owns the log + scheduler, exposes
  WebSocket/HTTP), `mta-worker` (pulls a command/run assignment, streams its events back, executes tools
  locally), leases/heartbeats (already expressible as events + deadlines), artifact upload, and
  per-worker capability/allowlist policy.
- **The event log is the replication unit.** A worker subscribes to the log from a `seq` and appends its
  own events with the expected per-stream `version`; a lease is a deadline; a heartbeat is an event.
  Replicating an append-only ordered log is strictly easier than reconciling mutable tables, which is the
  main reason this architecture was chosen now rather than in v0.2.
- Changes required in v0.1 to keep that cheap: no UI-private types in engine APIs; every state change an
  event with a `correlation_id`/`causation_id` (already planned); runs/messages/steps keyed by globally
  unique, replay-stable ids (ULID/UUIDv7); tool runtime isolated behind a trait so a worker can swap local
  execution; no assumptions that the engine and TUI share memory in the domain layer; `seq` monotonic and
  gap-tolerant so a remote replica can never mistake a hole for a problem.
- The engine lease and "TUI embeds engine" are v0.1 implementation details, documented as such.

---

## Appendix A — Configuration example

```toml
# ~/.config/mta/config.toml
[general]
default_board = "work"
timezone = "Asia/Jakarta"          # defaults to system
max_concurrent_runs = 2
enforce_wip = false
daily_budget_usd = 5.0
currency_display = "USD"

[ui]
theme = "default"                  # default | mono | custom path
mouse = true                      # pointer: click, double-click, wheel, scrollbar drag
scroll_lines = 3                  # wheel-notch distance in the board, run panel, overlays
hover = false                     # highlight under the pointer (motion events are noisy)
double_click_ms = 400
repaint_ms = 250                  # presentation-only: relative times and spinners (§3.8)
date_format = "%Y-%m-%d"

[events]                            # the log is the source of truth (docs/events.md, ADR-0009)
retention_days = 90                 # 0 = keep forever; `mta gc` prunes below the watermark only
snapshot_interval = 500             # events per stream between snapshots (bounds replay cost)
rebuild_on_start = "verify"         # verify (default) | always | never
live_deltas = true                  # stream token deltas on an ephemeral channel, never in the log

[host]                              # system monitor (S); samples are ephemeral, never logged
enabled = true
sample_ms = 1000                    # clamped to a floor; decoupled from [ui].repaint_ms
top_processes = 5
cpu_warn_pct = 85
mem_warn_pct = 85
disk_warn_pct = 90
# mounts auto-detected: real filesystems only (tmpfs/devtmpfs/overlay/efivarfs excluded)
observe_pressure = false            # true: log a debounced HostPressureObserved crossing
observe_pressure_cooldown_s = 300   # rising-edge only — an audit breadcrumb, not a poll

[estimation]                        # auto effort + run forecast (PLAN.md §3.3, ADR-0012)
enabled = true
default_minutes = 60                # starting point when no signal exists
min_minutes = 15
max_minutes = 480                   # a card bigger than a day should be split
round_to_minutes = 15
workday_hours = 8                   # display only: 8h renders as 1d
tag_minutes = { code = 120, docs = 60, research = 90, admin = 30, review = 45, bug = 120, ops = 60 }
priority_multiplier = { "1" = 1.2, "2" = 1.0, "3" = 0.9 }
dependency_bonus_pct = 10           # per dependency, capped at 50%
description_step_chars = 400        # each `description_step_chars` adds `description_step_minutes`
description_step_minutes = 15
description_max_steps = 4
learn_from_actuals = true           # calibrate effort from TaskEffortRecorded, never from elapsed time
throughput_window_days = 14
throughput_min_days = 3             # fewer days ⇒ "insufficient history", no projected finish
forecast = true                     # run duration/cost forecast when an agent + history exist
refine_with_decision = true         # one Jev score question for vague/uncertain cards
refine_below_confidence = 0.55
# The EWMA alpha, ratio clamp, and size buckets are fixed constants in the estimator, not config:
# changing config must not change a projection rebuild (ADR-0012).

[model_selection]                   # searchable picker (ADR-0013); discovery is ephemeral, never logged
discover = true                    # query each enabled provider's GET /models
discovery_ttl_s = 300              # in-memory cache; Ctrl-R / `mta models refresh` forces a refresh
include_unconfigured = true        # show live ids with no profile (marked, not selectable until profiled)
recent_limit = 5                   # recent-use rows in the empty-query view
write_target = "auto"              # auto | global | project — where a picker edit is written

[logging]                           # diagnostics only — never the audit trail (observability.md, ADR-0011)
level = "info"                      # error | warn | info | debug | trace | off
directives = []                     # per-target overrides, e.g. ["mta_engine::effects=debug", "hyper=warn"]
format = "human"                    # human | json
ansi = "auto"                       # auto | always | never
console = true                      # stderr sink; auto-disabled while the TUI owns the screen
file = true                         # rotating file sink (forced on in TUI mode)
dir = ""                            # default $XDG_STATE_HOME/mta/logs
rotate = "daily"                    # daily | hourly | never
retain_files = 7                    # files kept per rotation series
max_file_mb = 16                    # rotate early rather than grow past this
max_total_mb = 128                  # hard cap; oldest files pruned beyond it (with a warn)
ring_lines = 2000                   # buffer feeding the O overlay; never persisted
log_prompts = false                 # include prompt/response text (off by default)
log_tool_output = false             # include tool stdout/stderr bodies
max_field_bytes = 2048              # truncate any recorded field
dedup_window_ms = 1000              # collapse identical repeated records into a count

[storage]
db = "~/.local/share/mta/mta.db"   # default; --db overrides
workspaces = "~/.local/share/mta/workspaces"

[[providers]]
name = "openai"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
kind = "openai-chat"               # wire protocol
timeout_secs = 120
max_retries = 3

[[providers]]
name = "local"
base_url = "http://127.0.0.1:11434/v1"
api_key_env = "LOCAL_LLM_KEY"      # can be empty for Ollama
kind = "openai-chat"
timeout_secs = 300
max_retries = 1

[[providers]]
name = "kenari"                    # Jev / System One decision provider
kind = "systemone"                 # not OpenAI chat: POST /v1/systemone
base_url = "https://kenari.id/v1"
api_key_env = "KENARI_API_KEY"
timeout_secs = 60
max_retries = 2

[[models]]
alias = "fast"
provider = "local"
model = "qwen3:8b"
context_window = 131_072            # prompt + completion; drives the compaction trigger (§3.6.2)
max_output_tokens = 8_192
compact_at_tokens = 100_000         # model-level override: trigger = min(100K, 0.90 × 131K) ≈ 100K
[models.params]
temperature = 0.2
[models.capabilities]
tools = false
json_mode = true
streaming = true
[models.pricing]                    # USD per 1M tokens; 0 for local
input = 0.0
output = 0.0

[[models]]
alias = "smart"
provider = "openai"
model = "gpt-5.1-mini"
context_window = 200_000            # 200K window → trigger at min(200K, 0.90 × 200K) = 180K
max_output_tokens = 8_192
[models.params]
temperature = 0.3
[models.capabilities]
tools = true
json_mode = true
streaming = true
[models.pricing]
input = 0.25
output = 2.00

[[models]]
alias = "jev"
provider = "kenari"
model = "jev-1-13:free"            # resolve via /v1/models?modality=systemone
modality = "systemone"
usd_idr_rate = 16000               # display/budget conversion only; raw IDR kept in decisions
[models.capabilities]
tools = false
json_mode = false
streaming = false

[decisions]
enabled = true
model = "jev"
min_confidence = 0.70
store_state = "excerpt"            # full | excerpt | hash
apply_auto_assign = false          # true = headless may apply above threshold
apply_priority = false
apply_failure_triage = false
approval_advice = true
spec_gate = true
spec_threshold = 0.60

[context]                             # auto-compaction: PLAN.md §3.6.2, ADR-0007
auto_compact = true                   # master switch; false = compact only when asked (C / :compact)
compact_at_tokens = 200_000           # absolute prompt-token trigger (ceiling for large windows)
compact_at_ratio = 0.90               # cap relative to the model's context_window
reserve_output_tokens = 8_192         # fallback when a model sets no max_output_tokens
keep_recent_messages = 6              # always kept after a compaction
keep_recent_tokens = 24_000
max_tool_result_tokens = 20_000       # single tool result truncation (full text stays in the step)
summary_target_tokens = 3_000
model = ""                            # summariser alias; empty = the run's own model (no extra egress)
on_overflow = "truncate"              # truncate | block when the request still does not fit
summariser_failure = "digest"         # digest | block when the summariser errors or times out

[agents]
summariser_model = "fast"          # handoff/digest summaries; context compaction uses [context].model
opencode_discovery = true          # read .opencode/agents/**/*.md (docs/agents.md)
agents_dir_discovery = true         # read .agents/agents/**/*.md + .agents/*.md (ADR-0010)
include_global = true              # ~/.config/opencode/agents and ~/.agents/agents
extra_paths = []                   # e.g. [".kilo/agent"] — same Markdown format
trust_project = "ask"              # ask | always | never (hash-bound trust gate, §6.3)
max_depth = 1                      # child runs cannot delegate by default
max_children = 8                   # delegation calls per root run
max_concurrent_children = 1        # sequential: children share one workspace
max_file_kib = 256
inherit_model = true               # unresolved agent model inherits the parent's
max_steps_cap = 40                 # clamps an agent file's `steps`
default_child_usd = 0.50           # cap for a child run with no max_usd_per_run
watch = true

# Model-only user overrides for any agent, including file-discovered ones (ADR-0013).
# Precedence: per-run override > this binding > the definition's `model` > inherit.
[agents.model_bindings]
# ops = "smart"                    # `mta models set ops smart`; never changes prompt/tools/permissions

[skills]                            # .agents/skills/*/SKILL.md — attachable, never executable (ADR-0010)
enabled = true
discovery = true                    # scan project + ~/.agents skill roots
trust_project = "ask"               # ask | always | never (hash-bound, §6.4)
max_attached_bytes = 65536          # total attached SKILL.md bytes per run, enforced at attach time
max_file_kib = 64
# references/, assets/, scripts/, evals/ are recorded by name and never read (agents.md §2.1)

[[agents.definitions]]
name = "ops"
role = "Operations assistant"
model = "smart"
decision_model = "jev"
# auto_compact / compact_at_tokens may be set here to override the model/global context policy
tags = ["inbox", "admin", "scheduling"]   # auto-assign labels (NOT skills — those are SKILL.md files)
tools = ["read_file", "list_dir", "write_file", "http_fetch", "decide", "ask_human", "finish"]
max_steps = 20
max_usd_per_run = 0.50
system_prompt = """
You are an operations assistant. Work only from the task description and provided context.
Use tools to gather facts; cite file paths and URLs. Never invent data. End with the JSON result
envelope. For classification, comparison, or a go/no-go call, use decide with typed questions
instead of guessing; if confidence is below the configured threshold, call ask_human and stop.
"""

[[schedule]]
name = "morning-triage"
cron = "0 7 * * 1-5"
column = "todo"
agent = "ops"
title = "Morning inbox triage"
description = "Classify unread mail and flag anything needing my decision."
tags = ["inbox", "daily"]
```

## Appendix B — OpenAI-compatibility checklist

| Item | v0.1 |
|---|---|
| `POST /chat/completions`, `stream: true` | required |
| `GET /models` | required for `doctor`/`models list` (graceful "unsupported" if 404) |
| Bearer auth / custom headers / no-auth | required |
| `tools` + `tool_calls` (parallel) | required when capability enabled |
| `usage` in stream | optional; estimate tokens and flag `cost_estimated` when absent |
| `response_format: json_object` | capability-gated |
| `max_completion_tokens` vs `max_tokens` | model-profile override |
| Context window / max output | configured per model (`context_window`, `max_output_tokens`); defaults 200K/8K with a `doctor` warning — never guessed per provider |
| Context-length 400 (`context_length_exceeded`, `maximum context length`, …) | classified `ContextOverflow`; one emergency compaction + one retry, else `blocked` (no blind retry) |
| Azure-style deployment paths / `api-key` header | v1.1 (stretch) |
| Anthropic/Gemini native APIs | out of scope (use a compatible gateway) |
| Embeddings / vision / audio | out of scope for v0.1 |

## Appendix C — Jev / System One checklist

| Item | v0.1 |
|---|---|
| `POST /systemone` with `state` + `questions` | required |
| Bearer `kn-` auth; configurable `base_url` | required |
| `GET /models?modality=systemone` | required for `models list` / `doctor` |
| `noul` (with true/false criteria) | required |
| `choice` (criteria map) + probabilities | required |
| `score` (ordered rubric) + legend | required |
| `box` / image assets | typed but rejected; v0.1.1 |
| Streaming | not supported by the API; no streaming path |
| `usage.input_tokens` / `output_tokens` | required in practice; if absent, mark cost unknown and do not invent |
| Billing | input tokens only; IDR pricing converted at configured rate; free model = 0 |
| Errors: 400 / 402 / 429 / 5xx | 400 fail closed, 402 disable provider, 429/5xx retry with backoff |
| Max 128 questions, 512 KiB body | validated client-side before send |
| Answer/type/range validation | required; malformed answers fail closed |
| Per-agent `decision_model` override | required |
| MCP `jev_decide` tool | not used in v0.1; direct HTTP provider |

---

## Open questions

1. **Task domain:** generic knowledge work (current assumption) or a specific job function whose
   tools/prompt packs should shape v0.1? This changes M4/M5's tool set and M6's demo.
2. **Multi-machine timing:** is v0.1 allowed to be single-machine (recommended), or is a daemon +
   remote workers required in the first release? The plan currently defers it to M9.
3. **Default autonomy:** supervised (recommended) vs "trusted workspace" mode with no approvals for
   file writes inside the workspace — the latter speeds demos but weakens the safety story.
4. **OS targets:** linux-only for v0.1? macOS CI is cheap to add; Windows needs crossterm/terminal QA.
5. **Provider defaults to test:** which hosted endpoint do you want as the reference provider
   (plus local Ollama), for the live smoke script and `docs/providers.md`?
6. **Jev default model:** `jev-1-13:free` (zero cost, rate-limited) or a paid `jev-1-13`/`jev-1-14` for
   production quality? Should a self-hosted System One endpoint be a supported target in v0.1?
7. **Jev authority:** keep all decisions advisory (current default), or allow `apply_auto_assign` /
   `apply_priority` in headless mode? Should a high-confidence `noul` ever replace an approval, or is
   that permanently out of scope?
8. **Decision privacy:** default `store_state` — full (debuggable), 500-char excerpt (balanced), or hash
   (private)? This affects `mta decisions` usefulness and the calibration story.
9. **License / distribution:** MIT/Apache-2.0 dual? `cargo-dist` binaries or `cargo install` only?
10. **Event-log retention default:** 90 days (current assumption) or keep everything forever for a
    personal board? The trade-off is audit completeness versus the only unbounded table in the design;
    `docs/events.md` §15.2 lists the open refinements (snapshot policy per stream, projection
    snapshotting, undo semantics).
11. **Whether to expose the log to plugins/MCP:** `mta events tail` is public, but should a tool be
    able to *subscribe* to the log (read-only) in v0.1, or is that a v0.2 integration surface?
