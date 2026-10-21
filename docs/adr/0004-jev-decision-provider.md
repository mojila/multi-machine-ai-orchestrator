# ADR-0004: Jev is a first-class decision provider, not a chat provider

- **Status:** Proposed
- **Date:** 2026-10-21
- **Deciders:** project owner
- **Related:** [PLAN.md §3.5.1](../PLAN.md#351-decision-provider--jev--system-one),
  [providers.md](../providers.md)

## Context

Jev (kenari System One, model family `jev-1-13`/`jev-1-14`) answers **typed questions** about a
supplied state: `noul` (true/false with confidence), `choice` (one of a criteria map, with
probabilities), and `score` (an ordered rubric, with a scalar and legend). It has no free-form output,
no streaming, no tool calling, and a different endpoint (`POST /v1/systemone`, kenari's own contract).
It is exposed both as an HTTP API and as an MCP tool (`jev_decide`).

The orchestrator already has an OpenAI-compatible chat abstraction. The tempting shortcut is to treat
Jev as "just another model" behind `ChatModel`, or to ask a chat model to rate its own confidence. Both
are wrong: the wire shapes do not match, chat self-ratings are poorly calibrated, and a decision that
looks like chat invites the agent to treat it as free text.

## Decision

1. Add a second provider family, `kind = "systemone"`, with its own `DecisionModel` trait and typed
   request/response enums; it is configured like any other provider (`base_url`, `api_key_env`, model
   alias, capability flags).
2. Jev is used at explicit decision points: an agent tool (`decide`), and engine policies
   (`auto_assign`, `priority`, `approval_advice`, `failure_triage`, `spec_gate`).
3. Decisions are **advisory by default**. A policy applies one only when it is explicitly enabled
   (`apply_*`) and confidence clears `min_confidence`; decisions never authorise an approval or a
   tool call on their own.
4. Every decision is persisted (request, answers, confidence, model version, latency, IDR/USD cost,
   `applied`, later `outcome`) so behaviour is auditable and calibratable.
5. v0.1 talks HTTP directly. The MCP `jev_decide` tool is not a dependency of the core; MCP tool packs
   remain a v0.2 integration.
6. Text state only in v0.1; `box`/image questions are typed but rejected until the asset path exists.

## Consequences

**Positive:** Jev's cheap, calibrated answers can gate expensive runs and improve routing without a
second chat call; the audit trail explains every automated nudge; chat providers and decision providers
degrade independently (local-only chat still works with or without Jev); the design survives a future
self-hosted System One endpoint because only `base_url` changes.

**Negative:** a second wire protocol and validation surface to maintain; a new failure mode (decision
provider down) that every policy must fall back from; IDR→USD conversion adds an approximate cost path;
storing state raises privacy questions that need an explicit retention setting.

**Follow-ups:** record decision outcomes and publish a calibration report; consider `box`/image support;
evaluate an MCP adapter if users already run kenari's MCP server; revisit whether any `noul` decision may
ever replace a human approval (currently: no).
