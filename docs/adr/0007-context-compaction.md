# ADR-0007: Pre-flight rolling context compaction with a calibrated estimator

- **Status:** Proposed
- **Date:** 2026-10-21
- **Deciders:** project owner
- **Related:** [PLAN.md §3.6.2](../PLAN.md#362-context-assembly-and-auto-compaction),
  [ADR-0004 Jev decision provider](0004-jev-decision-provider.md),
  [docs/agents.md](../agents.md)

## Context

Runs are long: many LLM calls, tool results that can be tens of thousands of tokens, and iterative
follow-ups. Every OpenAI-compatible provider has a hard prompt limit (200K is the common case for
frontier models, 128K–1M elsewhere) and charges for whatever we send. Without a policy the failure modes
are: a hard 400 mid-run (`context_length_exceeded`) that wastes the whole run, silently dropping content
so the model "forgets" an approval it must respect, or unbounded cost growth as every step re-sends a
larger transcript.

`mta` also needs the transcript to remain a faithful audit artifact, and needs the estimator to be free
of model-specific tokeniser dependencies (`tiktoken-rs` etc. add build size and only cover some models —
see [ADR-0005](0005-build-size-budgets.md)).

## Options

1. **Truncate oldest messages (sliding window).** Cheap and predictable, but loses objectives and
   decisions silently, and can split a tool-call group from its results → invalid requests.
2. **Hard-stop at the limit.** Safe, but ends runs that a summary would have completed.
3. **Summarise the old span with a model.** Preserves intent; costs an extra call; the summary is
   untrusted model output that must not become a system instruction.
4. **Depend on each provider's server-side compaction/truncation.** Not available across local servers and
   gateways; behaviour is opaque and unauditable.

## Decision

Option 3, with option 1 only as an explicit, marked fallback, and option 2 as the final guard:

1. **Trigger before the wall.** Effective trigger
   `T = min(compact_at_tokens (default 200_000), floor(context_window × compact_at_ratio (default 0.90)))`
   and hard budget `B = context_window − max_output_tokens − 1_024`. For a 200K model that means
   compact at 180K and never send a 200K prompt on a 200K window.
2. **Estimate, don't tokenise.** `usage.prompt_tokens` is authoritative for the last request; the next
   request is estimated from serialized bytes with a per-model EWMA calibration and a 1.08 safety
   factor. Costs nothing, adds no dependency, and is labelled `estimated` in the UI.
3. **Pre-flight only, between steps.** Never mid-stream. Context is assembled and fitted before an
   `llm_call`; a step in flight is never mutated.
4. **Rolling, single active summary.** Each compaction folds the previous summary plus the newly old
   span into one new summary, so context size does not grow with run length and context assembly is
   `system → summary → messages after through_seq`.
5. **The summary is data, never policy.** It is stored as a `user`-role message with
   `kind = 'compaction_summary'`, wrapped in delimiters labelled as model-generated earlier context.
   Only the agent's own system prompt and `mta` configuration carry instruction authority.
6. **Atomic and auditable.** Summary message, `compacted` flags, `compactions` row, and run counters are
   written in one transaction; every `llm_call` step records the message seq range it sent, so prompts
   can be reconstructed for audit and cost analysis.
7. **Degrade per policy, never by accident.** Two independent knobs: `summariser_failure`
   (`digest` | `block`) governs a failing summariser — default `digest` emits a deterministic, visibly
   lossy summary built from DB metadata; `on_overflow` (`truncate` | `block`) governs a request that
   still does not fit after fitting — `truncate` replaces the largest tool results with head/tail plus a
   reference to the full text kept on the step, `block` ends the run with `blocked: context_overflow`
   before sending anything. Provider `ContextOverflow` → one emergency compaction + one retry; there is
   never a blind retry of an unchanged oversized prompt.
8. **Default summariser is the run's own model**, so enabling compaction does not silently send
   transcripts to a second provider; choosing a cheaper summariser is explicit configuration.
9. **The policy itself is configurable at four levels** — global `[context]`, per `[[models]]`, per
   `[[agents.definitions]]`, and per run (`mta run --compact=off --compact-at N`, TUI `:compact on|off`)
   — with **replace, not merge** semantics and the effective value plus its source exposed by
   `doctor --context` and the run badge. `auto_compact = false` disables model-driven compaction without
   disabling the request guard, and **manual compaction is always available** while a run is active, so
   "off" means "compact when I say", never "fail on the first big run". Three behaviours stay separate on
   purpose: threshold, overflow action, and summariser-failure action.

## Consequences

**Positive:** long runs complete instead of 400-ing; spend per step is bounded; the transcript stays
complete in SQLite while the model sees a compacted view; estimator calibration is observable in
`doctor`; no tokeniser dependency. Because the policy is configurable per run/model/agent, users keep
control: a local 131K model, a 200K hosted model, and a 1M gateway model can each have their own trigger
without a fork, and `auto_compact = false` gives a deterministic, model-free run shape when summaries are
unwanted. The same machinery gives sub-agent runs (ADR-0006) an independent context budget, since each
child compacts against its own model.

**Negative:** summaries are lossy — details dropped at 60% of a run are gone from the model's view, and
the fallback digest is more lossy still (marked in the UI). An extra model call is spent per compaction
and must be accounted for in budgets. Estimator error can still trigger a provider `ContextOverflow`; the
single-retry rule bounds the cost but a run can still end `blocked`. Between four override levels, three
behaviours, and effective-value reporting, the policy has a real configuration surface: precedence must
stay documented (`doctor --context` is the tie-breaker) and is a test target, or the flexibility becomes
its own bug class.

**Follow-ups:** decide whether to adopt a small tokeniser once model coverage is universal (opt-in,
behind a feature flag, measured against the size budget); expose `mta runs context <id>` to show the
trigger/calibration history; consider named context profiles if four override levels prove too granular
in practice; evaluate hierarchical summaries for very long-lived scheduled tasks.
