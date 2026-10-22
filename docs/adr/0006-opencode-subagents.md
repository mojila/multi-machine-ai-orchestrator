# ADR-0006: Reuse OpenCode agent definitions as the sub-agent roster

- **Status:** Accepted
- **Date:** 2026-10-21
- **Deciders:** project owner
- **Related:** [docs/agents.md](../agents.md),
  [PLAN.md §3.6.1](../PLAN.md#361-sub-agents-skills-and-file-defined-agents),
  [ADR-0004 Jev decision provider](0004-jev-decision-provider.md),
  [ADR-0010 agent and skill discovery](0010-agent-and-skill-discovery.md) (extends the roots below)

## Context

`mta` orchestrates work with named agents (system prompt + model + tool allowlist). Users who already run
OpenCode in a project have that roster on disk: `.opencode/agents/<id>.md` with YAML frontmatter
(`description`, `mode`, `model`, `permissions`, `steps`, `hidden`, `color`, …), and often a
`mode: primary` orchestrator plus `mode: subagent` specialists. Asking them to re-declare every agent in
`mta` config would duplicate prompts, drift immediately, and lose the value of opening `mta` in that
project.

Options considered:

1. **Shell out to `opencode run --agent <id>`.** Full runtime parity, but requires OpenCode installed,
   bypasses `mta`'s budgets/approvals/audit, duplicates session state, and makes the run non-deterministic
   across versions. Rejected.
2. **Define an `mta`-only agent format** (TOML/YAML under `.mta/agents/`). Clean, but zero reuse and a
   second thing for the user to maintain. Rejected as the *only* mechanism.
3. **Parse OpenCode Markdown agents natively and run them on `mta`'s runtime**, treating their permissions
   as restrictions on `mta` policy. Reuses the roster, keeps one execution/audit model.

## Decision

Adopt option 3, with these invariants:

1. `mta` discovers `.opencode/agents/**/*.md` (global `~/.config/opencode/agents` first, then project
   ancestors, then `mta` config definitions) and maps them to `mta` agents. Markdown body = system
   prompt; `description`, `mode`, `model`, `step` limits, `hidden`, `color`, `disabled` map directly.
2. **Permissions are monotonic restrictions.** `effective = strictest(global mta policy, agent rules)`;
   `allow` in an agent file can never widen `mta`'s policy. Both V2 ordered `permissions` lists and V1
   `permission` maps are accepted.
3. **Project agent files are untrusted input.** First delegation (and first after a hash change) requires
   an approval that shows the path, prompt excerpt, effective permissions, and resolved model. Trust is
   recorded per `(project, agent, sha256)`.
4. **Whole-definition replace** across layers, not OpenCode's field-level merge, so effective behaviour
   traces to exactly one source; `disabled` acts as a tombstone. Documented divergence.
5. **Delegation is a child run on the same task** (`parent_run_id`, `depth`, step kind `subagent_call`),
   with fresh context, own budget, no scheduler slot, hierarchical cancellation, and a depth limit
   (default 1) plus a cycle check. The child stays inside `mta`'s workspace sandbox.
6. `mta` never executes OpenCode, and never requires it to be installed.

## Consequences

**Positive:** opening `mta` in an existing OpenCode project immediately yields a meaningful roster; prompts
stay in one place; delegation is auditable and budgeted; the sandbox means a repo-supplied prompt cannot
silently rewrite the repo. Permission translation is explainable (`mta agents show` prints the winning
rule and origin).

**Negative:** a compatibility surface to maintain as OpenCode V2 evolves (`agents.md` §3 is the contract,
fixtures pin behaviour); some OpenCode features (MCP tools, LSP, plugins, `opencode.json` agents, model
variants) are not honoured, so agents relying on them underperform; whole-definition replace diverges from
OpenCode's merge semantics; project prompts require a trust step before first use. Skills are read as
attachments but never executed ([ADR-0010](0010-agent-and-skill-discovery.md), which also adds the `.agents`
roots) — a workflow that assumes a skill runtime does only as much as its instructions can do through
`mta`'s tools.

**Follow-ups:** re-evaluate `opencode.json(c)` `agents` entries and model variants after v0.1; track
OpenCode's agent docs for new frontmatter fields and extend the support matrix behind tests; measure how
often `inherit_model` masks misconfiguration and consider defaulting to `false` once discovery
diagnostics are mature.
