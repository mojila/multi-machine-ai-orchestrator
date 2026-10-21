# ADR-0010: Discover agents and skills from multiple roots, with skills as a separate kind

- **Status:** Proposed
- **Date:** 2026-10-21
- **Deciders:** project owner
- **Related:** [docs/agents.md](../agents.md), [ADR-0006 OpenCode sub-agents](0006-opencode-subagents.md),
  [ADR-0009 event-sourced core](0009-event-sourced-core.md),
  [PLAN.md §3.6.1](../PLAN.md#361-sub-agents-skills-and-file-defined-agents)

## Context

ADR-0006 made `.opencode/agents/*.md` the sub-agent roster. Opening `mta` in a project then reveals
whatever that one convention holds — and on the machine this plan is written on, that is not the whole
story:

- project and global agent definitions live in `.opencode/agents/` (`arasyi`,
  `nuhs-agent-orchestrator`) **and** in `~/.config/opencode/agents/`;
- a second, tool-neutral convention is already in use: `.agents/` at the project root and `~/.agents/`
  globally, with `skills-lock.json` / `.skill-lock.json` recording installed skill sources and hashes;
- what `.agents/` actually contains is **skills**, not agents: `.agents/skills/<name>/SKILL.md` with
  `name` + `description` frontmatter, an optional `agents/openai.yaml` UI metadata file, and
  `references/`, `assets/`, `scripts/`, `evals/` directories.

So a request to "detect sub-agents from `.agents/`" lands on a convention whose contents are a *different
kind of artefact*. Three shapes were realistic:

1. **Scan `.agents/**/*.md` as agents.** A one-line glob — and wrong twice over: it finds nothing on this
   machine, and where skills do exist it registers every `references/chapter_0N.md` as an agent named
   `skills/rust-best-practices/references/chapter_01`.
2. **Parse skills into the agent record.** Skills define no `mode`, no `model`, no `permissions`, and no
   `tools`. Mapping one to an agent means inventing a model and a policy the file never declared, then
   letting a file in a cloned repository select them.
3. **Recognise both kinds from explicit roots, keeping them distinct.**

## Decision

Adopt option 3.

1. **Discovery is multi-root and multi-kind.** Agents come from `~/.agents/agents/**`, `~/.agents/*.md`,
   `~/.config/opencode/agents/**`, `<project>/.opencode/agents/**`, `<project>/.agents/agents/**`,
   `<project>/.agents/*.md`, the legacy `<project>/.opencode/agent/**`, and `[[agents.definitions]]`.
   Skills come from `~/.agents/skills/*/SKILL.md` and `<project>/.agents/skills/*/SKILL.md`. Precedence is
   global → project ancestors (farthest → nearest) → config, and within a directory
   `.opencode/agents/` > `.agents/agents/` > `.agents/*.md` > legacy `.opencode/agent/`.
2. **The globs are deliberately narrow.** The agent glob is `.agents/agents/**/*.md` plus direct children
   `.agents/*.md` — never `.agents/**/*.md`. A skill directory is a black box except `SKILL.md` and its
   optional `agents/openai.yaml`; `references/`, `assets/`, `scripts/`, `evals/` are recorded by name and
   never read. A regression test asserts that a skill carrying nine `references/*.md` yields exactly one
   catalogue entry and zero agents.
3. **Skills are a separate `kind`, never a delegation target.** A skill is listed in the catalogue with
   provenance and trust, and can be **attached to a run as extra context at run creation**. It can never
   be the agent that executes a run; `delegate` refuses it. Attaching is a run-creation input because
   mid-run attachment would let prompt composition change under a fixed stream `version`.
4. **Project entries of both kinds are untrusted input**, gated by `(project, id, sha256)` exactly as
   agents are (agents.md §6.3–6.4). Global entries are trusted by definition.
5. **The log is the record.** Skills get their own `skill:<id>` stream and a `skills` projection
   (events `SkillDiscovered`/`SkillChanged`/`SkillUnavailable`/`SkillTrusted`/`SkillTrustRevoked`), and
   `RunStarted.attached_skills[]` carries each attached body with its hash, so replay never re-reads the
   file. The filesystem scan is an effect that submits `ScanCatalogs`; it is not the source of truth.
6. **"Skills" stops meaning two things.** The free-form auto-assignment labels move to
   `agents.tags_json` / `tags` in config, so "skills" now means only the `SKILL.md` kind.

## Consequences

**Positive:** `mta` sees every agent a user actually has, in either convention, global or project; the
same work item can carry a reusable workflow (a skill) without pretending the workflow is a persona; and
the black-box rule means a skill's reference corpus cannot leak into the roster, the prompt, or the scan
cost. Naming is unambiguous again.

**Negative / costs:** a second compatibility surface to maintain, and one whose upstream format is less
settled than OpenCode's agents (agents.md §2.1 is the contract, fixtures pin behaviour). Precedence across
four roots needs to be documented and tested rather than inferred. A skill is a second artefact the run
model must carry (`skill_snapshot_json`), and a skill's instructions are inert when they reference files
`mta` deliberately does not read — a real limitation for skill collections like
`rust-best-practices`, which must be attached knowing the agent will not open `references/`.

**Follow-ups:** model variants stay inert (ADR-0006); decide whether `.opencode/skills/` should join the
recognised roots once its upstream shape is stable (today it is reported, not read); evaluate mid-run
attach/detach and per-card default skills after v0.1; revisit the `64 KiB` attachment cap once real skills
are attached in anger; auto-suggesting a skill from a task's tags is deliberately deferred.
