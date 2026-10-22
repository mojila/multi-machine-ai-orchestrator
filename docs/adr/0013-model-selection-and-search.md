# ADR-0013: Searchable model selection, persisted as config

- **Status:** Proposed
- **Date:** 2026-10-22
- **Deciders:** @mojila
- **Related:** [PLAN.md §2.3](../PLAN.md#23-tui-specification), [PLAN.md §3.3](../PLAN.md#33-domain-model-and-state-machines),
  [PLAN.md §3.5](../PLAN.md#35-provider-layer--chat-and-typed-decisions),
  [PLAN.md §3.8](../PLAN.md#38-tui-architecture), [PLAN.md §3.9](../PLAN.md#39-configuration-and-secrets),
  [agents.md §5](../agents.md#5-model-resolution), [events.md §5.2](../events.md#52-run-life-cycle-runid),
  [events.md §5.6](../events.md#56-agents-agentid-and-skills-skillid),
  [ADR-0002](0002-version-pinning.md), [ADR-0009](0009-event-sourced-core.md),
  [ADR-0010](0010-agent-and-skill-discovery.md)

## Context

Models are the one part of the configuration a user changes constantly: trying a cheaper model, moving
an agent to a larger context window, or checking whether a local server has pulled a new tag. Today the
plan offers a read-only roster and expects the user to hand-edit `[[models]]` and an agent's `model`.
There is no picker, and `mta models list` has no search, so a provider exposing hundreds of ids is
navigated by eye.

The user asked for **model selection and search**. Two facts constrain the implementation:

- `[[agents.definitions]]` is a **whole-definition replace** layer (ADR-0010 §6.1). Writing a sparse
  `{name, model}` entry for a file-discovered agent would freeze its prompt, tools, and permissions at
  the moment the model was picked, and later edits to the file would silently stop applying. The same
  applies to rewriting an array element in place: it is safe only when mta already owns the full entry.
- Live `GET /models` results are **not** domain state. They carry no context window, pricing, or
  capability flags, they change without an event, and a replay must not depend on a provider being
  reachable. They belong with host metrics in the ephemeral category (ADR-0009 §8).

So the feature has two halves that must not be confused: an **ephemeral catalogue** for browsing and
searching, and a **durable choice** that is either a config binding or a per-run override.

Options considered:

1. **Keep the read-only roster; require `$EDITOR`.** Zero new surface, but the request is explicitly for
   selection and search. Rejected.
2. **DB-managed model choice (`AgentModelSelected` event).** Replayable, but models are identified by
   config aliases and priced by config profiles; a DB choice would still need a config profile and would
   split one concept across two sources of truth. Rejected.
3. **Picker over configured models only.** Simple, but hides exactly the models the user is trying to
   discover, and the provider already publishes them. Rejected.
4. **Picker over configured plus live discovery, with a profile gate for unconfigured entries;
   selection persisted in config; per-run override passed in the command.** Chosen.

## Decision

### 1. One picker, two scopes

A reusable `ModelPicker` component is used in three places:

- the **agent editor** — sets the agent's default model;
- the **run dialog** — sets a one-run override, shown as `Model: inherit (agent default)`;
- the standalone **model catalogue** (`M`) — browse, inspect, add a profile, or act on the selected
  card's agent.

The picker has two modes: `chat` (chat-completion models) and `systemone` (decision models). The mode is
derived from the provider's `kind`; the decision-model field in the agent editor opens `systemone` mode,
the chat-model field opens `chat` mode. Mixing the two is a validation error, not a filtered view.

Selection is **never** persisted from the run dialog. A per-run override lives in the `StartRun` command
and in the run's events only.

### 2. Sources, identity, and precedence

The catalogue is a merge of:

| Source | Durable? | Identity | Notes |
|---|---|---|---|
| `[[models]]` profiles | yes (config) | `alias` | Full metadata: provider, model id, context window, pricing, capabilities |
| `[[providers]]` + live `GET /models` | no (cache) | `provider/model-id` | Shallow: id and owner only; marked `not configured` |
| recent use | projected from `runs` | alias | Ranking signal only; not a new store |

An unconfigured entry **cannot be selected or started**. Choosing it opens a `ModelProfileForm`; saving
appends a `[[models]]` entry, reloads config, and only then completes the original selection. This is
what keeps every run tied to a known context window, price, and capability set. The form pre-fills what
discovery knows and requires a context-window value; pricing defaults to `0` with an `unknown` badge, and
capabilities default conservatively (`streaming = true`, `tools = false`, `json_mode = false`).

Resolution at run start, highest first:

1. the run's `model_alias` override (not persisted);
2. `[agents.model_bindings].<agent-name>` (config, user-owned);
3. the agent definition's `model` (file or `[[agents.definitions]]`);
4. inherit the parent/root run's model when `[agents].inherit_model = true`, otherwise unresolved.

A model binding is a **model-only layer**, deliberately not a definition: it cannot change a prompt, a
tool policy, permissions, or budgets, so it is safe to apply to a file-discovered agent without
triggering whole-definition replace. Precedence is deliberate — a local binding is the user's choice and
wins over the model declared by a project file; `mta agents show` prints the winner and what it shadows.

The publish path mirrors the existing config path: the engine's `ConfigEditor` writes the file
atomically, then submits `ReloadConfig`; the reload's diff appends `AgentModelBound`/`AgentModelUnbound`
on `agent:<id>`. Editing the file by hand produces the same events, so nothing depends on the UI.

### 3. Search

Search is local, in-memory, and pure; it never performs I/O per keystroke.

```
free terms          fuzzy subsequence over alias, model id, and provider
@provider           exact provider name (quote if it contains a space)
ctx>=100k           context window in tokens; also ctx<=, ctx=, suffixes k/m
tools / no-tools    capability filters
json / stream       json_mode / streaming capability filters
systemone / chat    modality filter
free                pricing input and output both zero
configured / unconfigured
available / unavailable
recent              in the recent-use set
-phrase             negate any token
"quoted phrase"     match the phrase literally
```

Unknown `key:value` tokens are treated as free text and the picker shows a small "unrecognised filter"
hint rather than silently matching nothing. Ranking is deterministic: exact alias, alias prefix,
word-boundary match, subsequence, then provider/id substring; ties break by configured, available,
recently used, then alias. An empty query is a useful default view (configured + available + recent)
rather than the raw provider order.

The same parser drives `mta models list --search` and `--json`, so a script and the TUI cannot disagree
about what a query means. The parser lives in `mta-provider` (pure, no I/O) and is shared by both.

### 4. Live discovery is ephemeral

The engine queries `GET {base_url}/models` for each enabled provider on demand, caches the result in
memory for `[model_selection].discovery_ttl_s` (default 300), and returns it through a `LoadModels`
query. `Ctrl-R` forces a refresh; opening the picker refreshes only a stale cache. There is no
background poll and no per-keystroke network call. A provider failure is a row group with its error
(`401`, `unreachable`) while configured models remain usable; partial failure never hides the rest.

Discovery has no event. Nothing about it is replayed, and model ids are not secrets; the request's
`Authorization` header is never logged (ADR-0011). The catalogue response is a view model, not a
projection, and a replay with the provider offline produces identical domain state.

### 5. Persistence is a config write, not a decider

`ConfigEditor` is the only writer of config. It:

- edits with `toml_edit`, preserving comments and ordering;
- validates the resulting document **before** writing (parse + config validation);
- writes a temporary file in the same directory, fsyncs, renames over the target, and keeps one `.bak`;
- compares the file hash captured when the picker opened; if the file changed underneath, it aborts and
  asks the user to reopen rather than clobbering an external edit;
- never writes a secret or an API key; only aliases, provider/model ids, and profile metadata;
- on a failed `ReloadConfig`, restores the backup and reloads the previous file, so the UI and the file
  cannot disagree;
- is never called from a decider. It runs in the engine/CLI path, and the durable result is the
  `ReloadConfig` diff.

Write target (`[model_selection].write_target`):

- `auto` (default) — the file that already defines the model profile or provider; else the project
  `.mta/config.toml` if present; else the user config;
- `global` — `$XDG_CONFIG_HOME/mta/config.toml`;
- `project` — `<cwd>/.mta/config.toml`, created if needed.

A config-managed agent's binding is a single key in `[agents.model_bindings]`; the engine never rewrites
a `[[agents.definitions]]` array element for a model change. This is the narrower mechanism that avoids
freezing a file agent, and the reason option 2's "write a definition override" was rejected in review.

### 6. Events and projections

| type | stream | payload |
|---|---|---|
| `AgentModelBound` | `agent:<id>` | `model_alias`, `origin = "config"`, `config_hash` |
| `AgentModelUnbound` | `agent:<id>` | `reason` |

Producer: `ReloadConfig`, diff-based — an unchanged binding appends nothing. `RunQueued` already carries
`model_alias`; it gains `model_source` (`override | binding | definition | inherit`) so a run explains
how its model was chosen. The `agents` projection gains `model_binding`, `effective_model`, and
`model_source`; `runs` gains `model_source`. `RunStarted` continues to snapshot the resolved model.

`StartRun` rejects a `model_alias` that is unknown, belongs to no profile, or has the wrong modality;
the UI resolves and validates before enabling the start action, so the rejection is a backstop.

### 7. Surfaces

- `M` opens the catalogue; `a`'s picker and the run dialog reuse it. `Enter` selects, `Ctrl-R` refreshes
  discovery, `Ctrl-U` clears, `Esc` clears then closes.
- `mta models list [--search <query>] [--all] [--json] [--modality chat|systemone] [--provider <p>]
  [--refresh]`, `mta models show <alias>`, `mta models add <provider/model> [--alias <a>] [--context N]
  [--tools] [--json-mode] [--price-in X --price-out Y]`, `mta models set|unset <agent> [alias]`, and
  `mta run --model <alias>`.
- `mta agents show` prints `model: smart (binding)` or `(definition)` / `(inherit)` and the shadowed
  value when a binding overrides a declaration.
- Selecting a model whose capabilities lack `tools` when the agent has tools, or whose context window is
  smaller than the current prompt, warns in the run dialog; it does not silently degrade the run.

### 8. Dependencies

`toml_edit = "=0.25.17+spec-1.1.0"` (MIT OR Apache-2.0, `rust-version = 1.85`, verified against
crates.io) is added directly for comment-preserving edits. It sits under the workspace MSRV of 1.94 and
adds a small parser/writer, not a new I/O stack. The config is still parsed into the same typed config;
`toml_edit` is used only for the write path. No other dependency is added: the query parser is
hand-written, like the task filter and the diagnostic directive parser.

## Consequences

- A user can find and choose a model without editing TOML, and can still edit TOML by hand — both paths
  produce the same `ReloadConfig` events.
- A file agent can be retargeted without freezing its definition, because the binding is not a
  definition.
- Unconfigured discovery results are usable only after a profile exists, so no run starts against an
  invented context window or a guessed price.
- The UI owns a cache but no new durable state; a restart loses the cache and re-discovers, which is
  correct for metadata that changes outside the log.
- Two config layers now concern models (`model` in a definition, `model_bindings` for the user's
  choice). This is the cost of not rewriting definitions; `agents show` and `doctor` make the winner
  explicit, and the ADR records why a single layer was rejected.
- `toml_edit` must be pinned and reviewed like every other pre-1.0 crate; a comment-preserving writer is
  the only safe way to edit a user's file, and the alternative (rewriting the document) loses comments.

## Alternatives considered

- **Sparse `[[agents.definitions]]` override** — rejected: whole-definition replace would freeze prompt,
  tools, and permissions at pick time.
- **Editing the array element in place** — safe only when mta owns the full entry; it cannot help
  file-discovered agents, so it would be a second mechanism with a different failure mode.
- **`AgentModelSelected` event as the source of truth** — rejected: the model's identity, price, and
  context window live in config; an event would need a profile anyway and would split one concept.
- **Auto-add every discovered model to config** — rejected: it writes hundreds of profiles the user did
  not choose and guesses pricing and capabilities.
- **Include `GET /models` results in the event log** — rejected: ephemeral, non-reproducible, and a
  replay would depend on network state (ADR-0009 §8).
- **Search on every keystroke via the provider** — rejected: latency, rate limits, and a hidden poll.
