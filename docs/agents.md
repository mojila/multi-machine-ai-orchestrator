# Sub-agents and skills: OpenCode agents and the `.agents` convention

`mta` can run in any directory, but when that directory is a project that already defines agents or
skills, those definitions become a **catalogue** the primary agent can use:

- **Agent definitions** — [OpenCode agents](https://opencode.ai/v2/docs/agents/) in
  `.opencode/agents/*.md`, and the tool-neutral `.agents/agents/**/*.md` — become the **sub-agent
  roster**: the primary agent can delegate part of a task to one, and each delegation runs as a child
  run with its own transcript, budget, and audit trail.
- **Skills** — `.agents/skills/<name>/SKILL.md`, the skill convention used by `.agents`-aware tools —
  are a **separate kind of catalogue entry**: a named Markdown workflow that can be *attached* to a run
  as extra context, but is **never selectable as the agent that executes a run**, because a skill
  defines no model, no permissions, and no tool policy (§2.2).

This document is the compatibility contract: what `mta` reads, how it maps fields, what it deliberately
does not support, and how delegation behaves. Design rationale:
[ADR-0006](adr/0006-opencode-subagents.md) (OpenCode agent compatibility) and
[ADR-0010](adr/0010-agent-and-skill-discovery.md) (multi-root discovery and the skill kind).

**Status:** design agreed, implemented in M5.5. Nothing here is a runtime dependency on OpenCode itself —
`mta` never shells out to `opencode`, and OpenCode does not need to be installed.

**Vocabulary.** In this document "agents" means delegation targets; "skills" means `.agents`-style
`SKILL.md` workflows. The word *tags* is used for the free-form labels that drive auto-assignment
(`agents.tags_json`) — never "skills", which would collide with the artefact kind defined here.

## 1. The flow in one paragraph

You run `mta` (or `mta tui --project ~/projects/arasyi`) inside a project containing
`.opencode/agents/*.md` or `.agents/`. On startup the engine discovers every agent file, validates its
frontmatter, maps OpenCode permissions onto `mta` tool policy, and records the roster in the `agents`
table with provenance (`source_path`, `source_hash`); it discovers `.agents/skills/*/SKILL.md` the same way
and records each as a `skill` entry it can attach to a run but never *run* (§2.2). A card's root agent can
call the new `delegate` tool; the engine starts a **child run** of the same task, bound to `parent_run_id`,
streaming its progress into the parent's run panel. Project agents and skills are untrusted input until you
approve them by content hash (§6.3, §6.4). The child inherits `mta`'s sandbox and approval model — an
OpenCode `edit: allow` rule never lets an agent mutate your repository behind your back.

## 2. Discovery

Two **kinds** are discovered from four **roots**. Every catalogue entry has a `kind` (`agent` or
`skill`) and lives in that kind's namespace.

| Root | Scope | Kind | Trust |
|---|---|---|---|
| `~/.agents/agents/**/*.md`, `~/.agents/*.md` | global, all projects | agent | trusted (user-owned) |
| `~/.agents/skills/*/SKILL.md` | global, all projects | skill | trusted (user-owned) |
| `$XDG_CONFIG_HOME/opencode/agents/**/*.md` (default `~/.config/opencode/agents`) | global, all projects | agent | trusted (user-owned) |
| `<project>/.opencode/agents/**/*.md` | project | agent | **untrusted until approved** (§6.3) |
| `<project>/.agents/agents/**/*.md`, `<project>/.agents/*.md` | project | agent | **untrusted until approved** (§6.3) |
| `<project>/.agents/skills/*/SKILL.md` | project | skill | **untrusted until approved** (§6.4) |
| `<project>/.opencode/agent/**/*.md` | project, **legacy V1 path** | agent | untrusted; deprecation warning |
| `[[agents.definitions]]` in `mta` config | project/global | agent | trusted (explicit) |

- **Project root:** the nearest git root at or above the opened directory; if the directory is not in a
  repository, only that directory is scanned. Ancestors between the opened directory and the project root
  are scanned too (farthest → nearest), so a monorepo can define agents at the root and override one in a
  package.
- **Agent ID:** path relative to the agents root, without `.md`, with `/` separators:
  `.opencode/agents/team/reviewer.md` → `team/reviewer`, `.agents/reviewer.md` → `reviewer`. IDs are
  case-sensitive and must match `^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$`; anything else is skipped with a
  diagnostic.
- **Skill ID:** the **directory name**: `.agents/skills/code-review/SKILL.md` → `code-review`. Frontmatter
  `name` must equal it; a mismatch emits a warning and the **directory wins**, so copy-pasted frontmatter
  cannot create a duplicate identity and editing the frontmatter cannot rename the entry. The same ID
  regex applies.
- **Separate namespaces.** An agent and a skill may share an ID; they shadow only within their own kind.
- **Ordering / precedence:** global (lowest) → project ancestors (farthest → nearest) → `mta` config
  definitions (highest). Within one directory the agent roots rank `.opencode/agents/` > `.agents/agents/`
  > `.agents/*.md` > legacy `.opencode/agent/`. Same ID from a higher layer replaces the lower definition
  **whole** (§6.1).
- **A skill directory is a black box except `SKILL.md`.** Discovery reads exactly two things per skill:
  `SKILL.md` and the optional `agents/openai.yaml|yml` beside it (§2.1). It **never** descends into
  `references/`, `assets/`, `scripts/`, or `evals/`. This is not an optimisation, it is a correctness
  rule. The installed `rust-best-practices` skill ships nine `references/chapter_0N.md` files, and a
  `.agents/**/*.md` glob would silently register each one as an agent named
  `skills/rust-best-practices/references/chapter_01`. (`.agents/` is installer-managed and gitignored —
  `skills-lock.json` pins it — so the committed regression fixture lives in
  `crates/mta-engine/tests/fixtures/skills/`, §11.) The agent glob is therefore exactly
  `.agents/agents/**/*.md` plus direct children `.agents/*.md` — never `.agents/**/*.md`.
- **Bounded scan:** one `SKILL.md` per skill, at most 200 entries per kind, 256 KiB per file (agents) /
  64 KiB per `SKILL.md`, UTF-8 only, no `.`-prefixed files or directories. Files are canonicalised; a
  symlink is read only if its target is inside the same root.
- **Reload:** directories are watched (`notify`, already a dependency for config reload). New runs use the
  new definitions; in-flight runs keep the snapshot they started with. `mta agents validate` performs a
  one-shot scan of every root and both kinds for CI and headless setups.
- **Not read in v0.1:** `agents` entries inside `opencode.json(c)`, plugin-provided agents, MCP tool
  servers, `reference/`-style nested resources as prompt content, or any other agent directory format.
  Discovering such a definition emits a single diagnostic naming the file, rather than silently ignoring
  it — and a `SKILL.md` under `.opencode/` (for example `.opencode/skills/<name>/SKILL.md`) is reported
  once as an unrecognised root rather than read.

### 2.1 The skill file (`SKILL.md`)

```
.agents/skills/code-review/
  SKILL.md              # required: YAML frontmatter + the workflow body
  agents/openai.yaml    # optional: UI metadata (or openai.yml)
  references/           # optional; NOT read at discovery — listed only
  assets/               # optional; listed only
  scripts/              # optional; listed only
```

**Frontmatter**

| Field | v0.1 | Mapping |
|---|---|---|
| `name` | yes | must equal the directory name (§2); mismatch warns, directory wins |
| `description` | yes | roster text: the one line shown in the picker and to the model when the skill is offered |
| `license`, `version`, and unknown fields | stored | kept in `definition_json`; one warning each; never fatal |
| `agents/openai.yaml` → `interface.{display_name, short_description, icon_small, icon_large}` | stored | presentation only; used by the roster, never sent to a model |

**Body** — the Markdown after the frontmatter is the skill's instruction text. When a skill is attached
to a run (§7.3) the body is wrapped in the same provenance header as an agent prompt, plus its
`source_hash`:

```
# Skill: code-review — source: .agents/skills/code-review/SKILL.md @ <hash>
```

The body is **bounded** (`[skills].max_attached_bytes`, default 64 KiB) and the bound is enforced at
attach time, not at discovery: a body over the limit is rejected with an actionable error naming the
file, never silently truncated.

**Not read:** `references/`, `assets/`, `scripts/`, `evals/`. Their *names* are recorded so
`mta skills show` can list them ("carries 9 reference documents, 1 script"), and an attached skill does
not pull them into the prompt. A skill whose instructions say "read `references/chapter_03.md`" is
therefore inert in v0.1 unless the run's own tools read that file — which the tool policy governs like
any other read.

### 2.2 Why skills are not agents

A skill file defines no `mode`, no `model`, no `permissions`, and no `tools`. Mapping one onto the agent
record would mean inventing a model and a policy that the artefact never expressed, and then letting a
file in a cloned repository pick them. So a skill is a **separate kind**:

- it appears in the roster (`A`) and in `mta agents list` with `kind = skill`, provenance, and trust
  state, next to agents;
- it can be **attached to a run** as extra context at run creation (§7.3);
- it can **never** be the agent that executes a run, and `delegate` refuses it with an actionable error
  naming the valid targets.

The one place a skill does carry authority is the OpenCode `skill` permission action in an agent file
(§4): that rule restricts *which* skills an agent may attach, and it only ever restricts.

## 3. Frontmatter support

Markdown body = system prompt. Frontmatter is YAML (mapping order is preserved, which permission maps
depend on).

| Field | v0.1 | Mapping |
|---|---|---|
| `description` | yes | roster text shown to the model and in the picker; also `agents.role` when `role` is absent |
| `mode` | yes | `primary` \| `subagent` \| `all`; see §3.1 |
| `model` | partial | `provider/model[#variant]` resolved against `mta` providers/models (§5); the variant is recorded but not applied |
| body / `system` | yes | system prompt, with a provenance header (`# Agent: <id> (<source path>)`) |
| `permissions` (V2 list) | yes | ordered `{action, resource, effect}` rules → §4 |
| `permission` (V1 map) | yes | action keys (`edit`, `bash`, `task`, `external_directory`, `"*"`) → §4.2 |
| `bash` / `edit` / `task` (V1 nested) | yes | pattern → effect maps; V1 `bash` → `shell`, `task` → `subagent` |
| `steps` / `maxSteps` | yes | per-run step cap, clamped to `[agents].max_steps_cap` (default 40) |
| `hidden` | yes | excluded from the roster and from delegation targets; still visible in `agents show` |
| `color` | yes | agent badge colour (`#rrggbb`; invalid values warn and fall back to the theme) |
| `disabled` / `disable` | yes | tombstone: removes an inherited definition with that ID |
| `tools` (V1 booleans) | partial | known tool names (`write`, `edit`, `patch`, `bash`, `read`, `grep`, `glob`, `webfetch`, `task`) become allow/deny rules; unknown keys warn and are ignored, never silently granted |
| `request` | stored | headers/body overlays are kept in the snapshot but not sent (OpenCode V2 does not send them either yet) |
| `temperature`, `top_p` | ignored | warning: set them on the `mta` model profile instead (`[models.params]`) |
| `prompt` (V1 file template) | unsupported | error: inline the prompt in the Markdown body |
| unknown fields | tolerated | preserved in `definition_json`, one warning each; never fatal |

A file with invalid YAML, an invalid `mode`, or a missing body is **skipped** (not the whole scan) and
surfaced by `mta agents list`/`doctor`. A malformed agent must never prevent `mta` from starting.

### 3.1 Modes

| OpenCode mode | `mta` behaviour |
|---|---|
| `primary` | may be chosen as a card's root agent; may **not** be launched by `delegate` |
| `subagent` | may only be launched by `delegate` |
| `all` | both |

This mirrors OpenCode: a `primary` agent is a session root, not a helper. The roster offered to
`delegate` therefore contains only visible `subagent`/`all` agents whose model resolves; untrusted
project agents appear but remain unlaunchable until the trust approval in §6.3 is recorded.

## 4. Permission translation

`mta` has its own tool policy (`allow` / `ask` / `deny` per tool, plus approval gating). OpenCode
permissions are applied as an **additional restriction layer**:

```
effective(tool) = strictest( mta_global_policy(tool), agent_rules(tool) )
strictest: deny > ask > allow
```

An agent rule can only make access stricter. `"*": allow` in an agent file grants nothing; it is a no-op
against `mta`'s policy. This is the single most important rule in this document.

| OpenCode action | `mta` tool(s) | Resource matched against |
|---|---|---|
| `read` | `read_file`, `list_dir` | project-relative or workspace-relative path |
| `glob`, `grep` | `list_dir`, `grep_files` | path glob |
| `edit` | `write_file`, `edit_file` | workspace-relative path (writes never reach the repo; see §9.2) |
| `shell`, `bash` | `run_command` | argv joined with single spaces (documented approximation; `mta` still executes argv arrays) |
| `webfetch`, `websearch` | `http_fetch`, `web_search` | URL/host glob |
| `subagent`, `task` | `delegate` | target agent ID |
| `skill` | skill attachment (§7.3) | skill ID glob |
| `external_directory` | writes outside the run workspace | always `deny` unless a staged-write approval exists; cannot be widened by an agent file |
| `"*"` | all of the above | all resources |

Rules are evaluated **last match wins** within one agent definition (V2 lists are ordered; V1 maps are
read in file order). If no rule matches an action, the effective value is `mta`'s global policy — not
`allow`. A `skill` rule is a restriction on *attachment* (§7.3), not on execution: `skill: { code-review:
allow, "*": deny }` means a run of this agent may attach `code-review` and nothing else, and an agent file
can never make an otherwise-denied skill attachable.

### 4.1 Worked example

```yaml
# arasyi/.opencode/agents/backend.md (abridged)
mode: subagent
permission:
  "*": allow
  external_directory: ask
  edit:
    "*": allow
    "*arasyi-kid-web*": deny
  bash:
    "*": allow
    "*rm *arasyi-kid-web*": deny
```

Result inside `mta`: `write_file`/`edit_file` are allowed by the agent but must still pass the global
policy (writes are workspace-only, so a path outside becomes a staged-write approval); any resource
matching `*arasyi-kid-web*` is `deny` and wins. `run_command` follows the same shape against the joined
argv. `external_directory: ask` cannot widen `mta`'s default deny — a staged write still requires the
normal approval, and the rule is shown in `mta agents show` as "no effect (above global policy)".

`mta agents show <id>` prints the effective table with, for every tool, the winning rule and its origin,
so this translation is never a mystery.

### 4.2 V1 `permission` maps

V1 files (the common case in existing projects) use a map instead of a list:

```yaml
permission:
  "*": allow
  edit:
    "*": allow
    "*.env": deny
  task:
    reviewer: allow
    "*": deny
```

`mta` expands the map into ordered rules (outer key order, then inner key order), mapping `bash` →
`shell` and `task` → `subagent`. V2 list rules are appended after expanded V1 rules when both are
present, so V2 syntax wins on ties. A key that is neither a known action nor a tool alias produces one
warning and is ignored.

## 5. Model resolution

`model: provider/model#variant` is resolved at run start, in this order:

1. an exact `mta` model alias equal to the full string (`provider/model`);
2. a `[[models]]` entry whose `provider` name and `model` id match (`providerID` = provider name,
   model id may itself contain `/`, which is why the split is on the **first** `/`);
3. a `[[providers]]` entry whose name matches, with the model id used verbatim;
4. otherwise the model is **unresolved**.

`#variant` is parsed and stored in the snapshot, then ignored with a warning (v0.1 has no variant
routing).

For an unresolved model: if `[agents].inherit_model = true` (default), the agent inherits the parent
run's model and the picker/roster shows an "inherits model" badge. With `inherit_model = false`, the
agent remains listed but cannot be launched, and `mta agents show` prints why. The engine never guesses
a provider.

Explicit `mta` config wins: `[[agents.definitions]]` with the same `name` can override `model` (and any
other field) regardless of the file's value.

## 6. Merge, overrides, trust

### 6.1 Whole-definition replace, not field merge

OpenCode merges agent definitions field by field and appends permission rules. `mta` deliberately does
**whole-definition replace** (the same rule it already uses for named config arrays): the winning
definition is exactly one file or one `[[agents.definitions]]` entry. Reasons: effective permissions
always trace to one source, a project cannot accidentally inherit a broad global grant, and
`mta agents show` is truthful without re-implementing another program's merge semantics. This is an
explicit divergence (§9.4).

Tombstones work across layers: a config entry with `disabled = true` removes an inherited file agent
with that ID. Shadowing is reported by `mta agents list --all` (`shadowed by <path>`).

### 6.2 Effective catalogue record

Discovered agent definitions are persisted in the existing `agents` table (extended in M5.5) with:

- `kind` = `agent` (the table's default; skills are a separate table, below)
- `source` = `opencode-global` | `opencode-project` | `agents-global` | `agents-project` | `config` | `db`
- `source_path`, `source_hash` (sha256 of the raw file), `definition_json` (normalised spec + warnings)
- `mode`, `tool_policy_json`, `last_seen_at`, `enabled`

Discovered skills are persisted in a `skills` projection with:

- `id`, `name` (equal to `id` by §2), `description`
- `source` = `agents-global` | `agents-project` | `config` (config-declared skills, if any)
- `source_path`, `source_hash`, `definition_json` (raw frontmatter + warnings),
  `interface_json` (the optional `agents/openai.yaml` metadata), `resources_json` (names and counts of
  `references/`, `assets/`, `scripts/`, `evals/` — **names only**, never their contents), `body_bytes`,
  `last_seen_at`, `enabled`

Removed files are disabled on the next scan, never deleted: old runs keep a valid FK and their audit
trail. A run also stores an `agent_snapshot_json` and a `skill_snapshot_json` (§7.3), so editing an agent
or skill file never rewrites history.

### 6.3 Trust gate for project agents

A repository you clone can ship `.opencode/agents/*.md` whose system prompt says anything. `mta` treats
project agents as untrusted input:

- `[agents].trust_project = "ask"` (default): the **first delegation** to a project agent (and the first
  after its content hash changes) suspends the child run at an approval showing: agent ID, source path,
  description, a prompt excerpt, the effective permission table, the resolved model, and the file hash.
  Approving records `(project_path, agent_id, source_hash)` in `agent_trust`; later launches with the same
  hash are not re-prompted.
- `"always"` skips the prompt (only sensible in a workspace you control); `"never"` makes project agents
  list-only — they can be inspected but not launched.
- Global agents and `[[agents.definitions]]` are trusted by definition and never prompted.
- Headless equivalent: `mta agents trust <id>` prints the hash and records the decision for the current
  project. Trust is per hash, so a pull request that edits an agent's prompt triggers a fresh review.

The prompt is also wrapped with a provenance header and a reminder that it is configuration, not an
instruction from the user: `# Agent: reviewer — source: .opencode/agents/reviewer.md @ <hash>`.

### 6.4 Trust gate for project skills

A cloned repository can ship `.agents/skills/*/SKILL.md` whose body says anything, and attaching a skill
puts that text in the prompt. Skills are therefore untrusted on exactly the same terms as project agents:

- `[skills].trust_project = "ask"` (default): the **first attachment** of a project skill, and the first
  after its content hash changes, prompts for approval showing the skill ID, source path, description, a
  body excerpt, the resolved byte size, the `skill`-permission interplay (which agents may attach it), and
  the file hash. Approving records `(project_path, skill_id, source_hash)` in `skill_trust` (the
  skill-kind mirror of `agent_trust`); later attachments with the same hash are not re-prompted.
- `"always"` skips the prompt; `"never"` makes project skills show-but-not-attachable.
- Global skills (`~/.agents/skills`) are trusted by definition and never prompted.
- Headless equivalent: `mta skills trust <id>` prints the hash and records the decision for the current
  project.

**The gate is at attach time, not mid-run — and that is deliberate.** An agent's trust prompt can suspend
the child run because delegation happens *inside* an open step. A skill is attached *before* the run
exists, so there is no step to suspend. Instead the decider validates the attachment: `StartRun` carrying
an untrusted skill, or a `source_hash` that does not match what was approved, is **rejected** (with a
`CommandRejected` naming the skill), and the TUI/CLI resolves trust before submitting the command. This
also means a run never begins with a skill whose body has changed under it.

`SkillTrusted`/`SkillTrustRevoked` are appended on the skill's own `skill:<id>` stream
([events.md §5.6](events.md#56-agents-agentid-and-skills-skillid)), so revocation is as auditable as
approval. Unlike agent trust there is no `SkillTrustRequested` event: the decision is taken outside a run,
so there is no in-run pending state and no deadline to arm.

## 7. Delegation runtime

### 7.1 The `delegate` tool

| Property | Value |
|---|---|
| Input | `{ "agent": "<id>", "instructions": "<what to do>", "context": "<optional handoff>", "timeout_secs": 600 }` |
| Availability | only when the roster has a launchable **agent** target (a skill is never a target), `delegate` is allowed by the caller's effective policy, `depth < max_depth`, and the target ID is not already in the ancestor chain |
| Output | `{ "run_id", "agent", "status", "summary", "artifacts": [...], "cost_usd", "truncated" }` |
| Side effects | creates a `runs` row (child), a `subagent_call` step on the parent, and events; never mutates the task's lane directly |

`instructions` are the child's user message. `context` is an explicit handoff summary — children start
with **fresh context**, like OpenCode subagents; they do not inherit the parent transcript. A child gets
its own system prompt, its own model (or the parent's, per §5), and its own tool policy.

### 7.2 Child runs

- `runs.parent_run_id` + `runs.depth` link the child to the parent; `steps.kind = 'subagent_call'`
  holds `{agent, instructions, child_run_id, status, summary}`. Both are visible in the parent's run
  panel; `Enter` opens the child transcript.
- The child is **not** a second card. It belongs to the same task, so the board does not fill with
  incidental delegation cards; agents that want a visible card use `create_subtask` (existing tool).
- Child runs do **not** take scheduler run slots — otherwise two delegating parents at
  `max_concurrent_runs = 2` would deadlock. Concurrency is bounded per parent
  (`[agents].max_concurrent_children`, default 1).
- The child shares the parent's run workspace. With the default of one child at a time, writes cannot
  interleave. Raising the limit is only safe for read-only agents until a per-path write lease lands.
- Cancellation is hierarchical: cancelling the parent cancels the child (and its tool processes) first;
  the child's terminal state is `cancelled`.
- Budgets: the child has its own cap (`agents.max_usd_per_run`, else `[agents].default_child_usd`), and
  the parent's remaining budget is a hard ceiling. A parent's displayed spend includes descendants;
  each row still stores only its own spend, so nothing is double-counted in aggregates.
- Approvals raised by the child suspend the **child** run at `awaiting_approval` (still the only
  suspension point); the parent's `delegate` step stays open and the header badge counts child
  approvals. Approving/denying works exactly as for a root run.
- The parent decides what a failed child means: `status = failed | budget_exhausted | cancelled` is
  returned in the tool result; the parent may retry, delegate to another agent, or abort. Failures never
  silently promote to a parent failure.
- Depth default `1`: a child cannot delegate further. Raising `max_depth` re-enables delegation subject
  to the cycle check (A → B → A is refused even at depth 2) and a per-root delegate counter
  (`max_children`, default 8).

### 7.3 Skill attachment

A skill is attached to a run **at creation**, not mid-run. Mid-run attachment would let the prompt
composition change between steps while the run's `version` stayed the same, so the same step could be
replayed with different instructions; attaching up front keeps a run's prompt inputs immutable and
replayable.

- **Where it is chosen:** `mta run <task-id> --skills code-review,solid`, the palette
  (`:run --skills …`), and the run-start dialog, which offers attachable skills grouped after the agent
  picker. The assign picker never offers a skill as an *executor*.
- **What is recorded:** `StartRun` carries `skill_ids[]`; `RunQueued` carries the ids, and
  `RunStarted.attached_skills[]` carries `{id, source_hash, body}` for each one. The body travels in the
  event for the same reason the agent system prompt does — replay must not depend on the file still
  existing or still saying the same thing. It lands in `runs.skill_snapshot_json`.
- **Bounding and failure:** total attached bytes are capped by `[skills].max_attached_bytes`
  (default 64 KiB) and the cap is enforced at attach time; a bound-exceeded attachment is rejected with an
  actionable error, never truncated silently. Attaching an unknown, disabled, or untrusted id is rejected
  by the decider (§6.4).
- **Prompt order:** system prompt (agent) → provenance header per attached skill → task → transcript. Each
  attached skill is preceded by its own `# Skill: <id> … @ <hash>` header (§2.1), so the model can always
  tell a skill's instructions from the user's.
- **Budget:** attached skill bodies count toward the context estimate like any other prompt content, so a
  large attachment can trigger compaction earlier — it is visible in the run panel as input tokens, not a
  hidden surcharge.
- **Children:** `delegate` may pass `skills[]` for the child; a child inherits **no** skills automatically.
  Its effective `skill` permission (§4) still applies.
- **Not in v0.1 (backlog):** attaching or detaching a skill mid-run, per-card default skills, and
  auto-attaching a skill when a task's tags or the diff suggest it.

## 8. Surfaces

**CLI**

```
mta agents list [--all] [--json] [--source <root>] [--kind agent|skill]
                                # one catalogue view; `kind` column distinguishes agents from skills
mta agents show <id>            # provenance, kind, mode, model resolution, effective permissions, warnings
mta agents validate             # scan every root + both kinds; exit 1 on errors (CI)
mta agents trust <id>           # headless trust for the current project (prints file hash)
mta agents add|edit             # existing mta-managed agents; unrelated to file discovery
mta skills list [--all] [--json]   # convenience: `agents list --kind skill`
mta skills show <id>            # description, body size, present resources, interface metadata, trust state
mta skills trust <id>           # headless trust for the current project (prints file hash)
mta run <task-id> --skills <id,...>   # attach skills at run creation (§7.3)
```

`--source` accepts `opencode-global | opencode-project | agents-global | agents-project | config | db`.

**TUI**

- `A` opens the catalogue roster: kind, ID, mode (agents) or size (skills), model (with "inherits" badge),
  source, trust state, warnings, permission summary. Agents and skills are grouped by kind; `Enter`
  inspects either without launching.
- The assign picker groups mta-managed agents first, then file agents; untrusted project agents show a
  lock glyph and a one-line reason. **Skills never appear as delegation targets** — a skill is offered only
  in the run-start dialog's attach row, where an untrusted skill shows the same lock glyph and cannot be
  attached until approved.
- The run panel renders `subagent_call` steps collapsed to `→ reviewer · succeeded · $0.03 · 42 s`, with
  `Enter` to open the child transcript; attached skills appear as a `skill · code-review @ <hash>` chip on
  the run header and are expandable to the exact text that entered the prompt.
- Card detail shows a "delegated runs" list (agent, status, duration, cost) for the latest run.

`mta doctor` reports discovery: roots scanned, agents and skills found/skipped per kind, warnings,
unresolved models, untrusted project entries, unrecognised skill roots, and — for each skill — which
resources exist but were not read.

## 9. Divergences from OpenCode

1. **No OpenCode runtime.** `mta` parses definitions; it does not shell out, import plugins, speak MCP, or
   use LSP. It *does* read `.agents`-style skills, but as **text** — a `SKILL.md` is attached to a prompt,
   never executed by a skill runtime, so a workflow that assumed one does only as much as its own
   instructions can do through `mta`'s tools.
2. **Sandbox, not the repo.** `mta` runs agents in `$XDG_DATA_HOME/mta/workspaces/<run-id>/` with the
   project as a read-only mount. `edit: allow` means "write in the workspace"; changes to real project
   files are staged artifacts plus an approval, exactly as for any other run. OpenCode edits the project
   directly.
3. **Permissions only restrict.** There is no path by which an agent file widens `mta`'s global policy.
4. **Whole-definition replace.** No field-level merge or rule appending (§6.1).
5. **Variants are inert.** `#high` is recorded, not routed.
6. **`opencode.json(c)` agents, plugins, MCP:** not read in v0.1. Skills **are** read — but only
   `.agents/skills/*/SKILL.md` (plus the optional `agents/openai.yaml`), and only as attachable text;
   `references/`, `assets/`, `scripts/`, and `evals/` are never read.
7. **`request` overlays** are stored but not sent.
8. **`prompt:` file templates** are rejected; inline the body.
9. **`temperature`/`top_p`** are ignored; use `[models.params]`.
10. **Single project scope per process.** Discovery follows the opened project, not a global session list.
11. **Skills are a separate kind, never delegation targets.** A skill has no model or policy, so `mta`
    refuses to *run* one; it can only be attached (§2.2, §7.3). A tool that offers skills as agents is
    inventing a policy the file never declared.

## 10. Diagnostics

| Condition | Behaviour |
|---|---|
| invalid YAML / bad `mode` | agent skipped; file + line reported; `mta` still starts |
| duplicate ID across roots | higher layer wins; `list --all` shows what was shadowed |
| unresolved model, `inherit_model = true` | agent listed with "inherits" badge; launch uses parent model |
| unresolved model, `inherit_model = false` | agent listed but unlaunchable with an explicit reason |
| unknown permission action/field | warning; unknown action ignored, unknown field preserved |
| project agent not trusted | child run suspends on a trust approval; `never` makes it list-only |
| target is `primary`/hidden/disabled | `delegate` fails fast with an actionable error listing valid targets |
| depth or cycle limit hit | `delegate` fails fast; run continues, parent sees the error |
| file too large / symlink escapes root / bad ID | skipped with a diagnostic; never read |
| skill frontmatter `name` ≠ directory name | warning; the directory name is the ID (§2) |
| `.md` file under `.agents/skills/<name>/{references,assets,scripts,evals}/` | never parsed as an agent; `doctor` names the skill directory and states it is a black box except `SKILL.md` |
| `SKILL.md` outside a recognised root (e.g. `.opencode/skills/`) | reported once by `doctor` as an unrecognised root; not read |
| skill body over `[skills].max_attached_bytes` | the attachment is rejected, naming the file and the size; never silently truncated |
| untrusted project skill | attachable only after approval; the run-start dialog shows a lock glyph; `mta skills trust <id>` for headless |
| skill named as a delegation target | `delegate` fails fast: "`code-review` is a skill (kind = skill); attachable agents: …" |

## 11. Test plan (M5.5)

Fixtures under `crates/mta-engine/tests/fixtures/agents/` (agents) and
`crates/mta-engine/tests/fixtures/skills/` (skills):

- V2 markdown (list permissions, nested team ID, `hidden`, `disabled`, `all` mode);
- V1 markdown (`permission` map, `bash`/`edit`/`task` nesting, `tools` booleans, `maxSteps`);
- malformed YAML, bad mode, missing body, oversized file, symlink escape, hostile ID (`../etc/passwd`);
- precedence: global vs project root vs nested package vs config override vs tombstone;
- **multi-root precedence:** one ID defined in `.opencode/agents/`, `.agents/agents/`, and
  `.agents/*.md` in the same directory resolves in the documented order (§2), and `list --all` shows each
  shadowed path;
- **the black-box regression test:** a fixture skill directory containing `references/chapter_01.md` …
  `references/chapter_09.md` scans to **exactly one** catalogue entry (`kind = skill`, id = the directory
  name) and **zero** agents. This is the test that fails if anyone "simplifies" the `.agents/agents/**`
  glob to `.agents/**`;
- skills: frontmatter `name` matching and mismatching the directory, missing `description`, oversized
  `SKILL.md`, `agents/openai.yaml` and `openai.yml` metadata, `references/`+`assets/`+`scripts/` recorded by
  name but not read, an unrecognised `SKILL.md` root reported once;
- model resolution matrix (alias hit, provider/model hit, provider-only, unresolved with both
  `inherit_model` values);
- permission matrix: OpenCode rules × mta global policy → effective table, including the
  "`allow` cannot widen" property test and the `skill` action restricting attachment;
- delegation: success, child failure, child budget exhaustion, child approval approve/deny/expiry,
  cycle A→B→A, depth limit, cancellation, trust prompt approve + hash change re-prompt, **and the refusal
  to delegate to a skill**;
- attachment: `StartRun` with a trusted skill records `RunStarted.attached_skills[]` with the body;
  replaying that log needs no file on disk and yields a byte-identical projection; a body over
  `[skills].max_attached_bytes` is rejected; `StartRun` with an untrusted or hash-mismatched skill is
  rejected without appending a run;
- TUI snapshots for the roster (both kinds, grouped), the untrusted badge, a skill chip on a run header,
  and a collapsed child step.
