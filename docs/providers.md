# Provider compatibility

`mta` speaks two wire protocols: the OpenAI **chat completions** API for generation, and kenari
**System One** for typed decisions (Jev). This page records what has actually been verified, on which
version, and any quirks. It is a test log, not marketing.

**Current status:** planning stage — the app does not exist yet. One **manual** probe of the System One
contract was run on 2026-10-21 (see below); all matrix rows are otherwise `untested` until the M3
conformance suite and live smoke script produce results.

## Matrix

| Provider / server | Wire | Streaming text | Tools | Usage in stream | `/models` | Status |
|---|---|---|---|---|---|---|
| OpenAI | `openai-chat` | — | — | — | — | untested |
| OpenRouter | `openai-chat` | — | — | — | — | untested |
| Groq | `openai-chat` | — | — | — | — | untested |
| Ollama (`/v1`) | `openai-chat` | — | — | — | — | untested |
| vLLM | `openai-chat` | — | — | — | — | untested |
| LM Studio | `openai-chat` | — | — | — | — | untested |
| kenari System One (Jev) | `systemone` | n/a | n/a | n/a (usage in body) | yes, `?modality=systemone` | contract probed manually 2026-10-21; app client pending |

Legend: `yes`, `no`, `partial`, `n/a`, `untested`. Record the server version next to a `yes`.

## Test procedure

Conformance tests (no network, run in CI) cover framing and error handling with `httpmock` fixtures:

```sh
cargo test -p mta-provider
```

Live probe (opt-in, requires a configured provider):

```sh
mta doctor --provider <name>        # config, DB, GET /models, one-token completion, capability report
mta models list [--modality systemone]   # what the endpoint actually advertises
mta chat "reply with the word ok"   # streaming smoke
mta decide --model jev --state-file ctx.json --questions q.json   # typed decision smoke
```

A chat provider is only marked `yes` in the matrix after: streaming text works, one tool call
round-trips, errors surface as `ProviderError` (no panic), and cancellation stops the stream. A decision
provider is marked after `noul`, `choice`, and `score` answers validate against the contract, a
malformed answer fails closed, and 402/404 behaviour is correct. Usage is separately marked because many
local servers omit it while streaming; `mta` then estimates tokens and flags the run `cost_estimated`
(see [PLAN.md §3.7](PLAN.md#37-orchestration-policy-engine)).

## System One (Jev) decisions

Jev is a decision model, not a chat model. One call asks up to 128 typed questions about a `state` and
returns one typed answer per question; there is no streaming and no tool calling.

```bash
curl https://kenari.id/v1/systemone \
  -H "Authorization: Bearer $KENARI_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "jev-1-13:free",
    "state": "The report is due Friday; finance numbers are missing.",
    "questions": {
      "ready":   { "type": "noul",   "instructions": "Is the report ready to submit?",
                   "criteria": { "true": "complete", "false": "material work missing" } },
      "next":    { "type": "choice", "instructions": "Best next action?",
                   "criteria": { "a": "wait", "b": "draft finance section" } },
      "urgency": { "type": "score",  "instructions": "Urgency?",
                   "criteria": ["low", "normal", "high", "critical"] }
    }
  }'
```

Verified response shape (2026-10-21, `jev-1-13:free`; answered by `jev-1.13.0`):

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "ready":   { "type": "noul",   "noul": 0.02 },
    "next":    { "type": "choice", "choice": "b", "confidence": 0.97,
                 "probabilities": { "a": 0, "b": 0.98 } },
    "urgency": { "type": "score",  "score": 2.13, "confidence": 0.81,
                 "legend": { "0": "low", "1": "normal", "2": "high", "3": "critical" },
                 "probabilities": { "0": 0, "1": 0.03, "2": 0.81, "3": 0.16 } }
  },
  "usage": { "input_tokens": 12, "output_tokens": 20 }
}
```

- Discovery: `GET /v1/models?modality=systemone`; a plain `GET /v1/models` omits these models.
- Billing: input tokens only; output tokens are reported and not billed. Prices are IDR; free models
  (`:free`) cost Rp 0. `mta` stores raw IDR alongside a converted micro-USD amount.
- `box`/image questions and `assets` are out of scope for v0.1; the client rejects them explicitly
  rather than silently dropping fields.
- Full design and policy use: [PLAN.md §3.5.1](PLAN.md#351-decision-provider--jev--system-one).

## Known quirks

Add an entry per provider when something needs a workaround. Keep it factual and reproducible.

### kenari System One — manual probe, 2026-10-21

- **Observed:** model ids use a colon. `jev-1-13-free` is rejected with HTTP 400 ("not a System One
  model"); valid ids at probe time were `clef-flash:free`, `clef:free`, `jev-1-13`, `jev-1-13:free`,
  `jev-1-14`.
- **Discovery:** `GET /v1/models` does not list these; use `?modality=systemone`.
- **Response:** no streaming. `usage` is present (`input_tokens`, `output_tokens`). `noul` is a bare
  scalar; `choice` returns the chosen key plus `confidence` and `probabilities`; `score` returns a
  scalar plus `confidence`, `legend`, and `probabilities`.
- **Workaround / config:** always resolve the model id from discovery or pin it in config; never rely
  on a tool's default id. Validate answer type and ranges before use.
- **Verified on:** 2026-10-21, free model, one `noul` + `choice` + `score` request. No `mta` client
  existed yet; this is a contract probe, not an app-level test.

```md
### <provider> <version>
- Observed: ...
- Request shape: ...
- Workaround / config: ...
- Verified on: YYYY-MM-DD by <name>, `mta doctor` output attached to the issue
```

## Chat compatibility rules we rely on

- `POST {base_url}/chat/completions`, `stream: true`, optional `tools`, optional
  `response_format: json_object`.
- `Authorization: Bearer <key>` by default; per-provider extra headers for gateways.
- Tolerate `content: null` alongside `tool_calls`, missing `usage`, SSE comments, `data: [DONE]`, and
  JSON split across chunk boundaries.
- Never send a field the model profile has not declared (`tools`, `json_mode`); degrade to a
  no-tool/JSON-in-text prompt and label the run `degraded`.
- No retry after the first assistant delta; close the step `aborted` instead.

## Decision rules we rely on

- `POST {base_url}/systemone`, bearer `kn-` key, no streaming, no tools; `state` plus a `questions` map.
- Only `noul`, `choice`, and `score` in v0.1; reject `box`/images rather than dropping them.
- Validate every answer before use (id present once, type matches, choice within criteria, score within
  range, confidence in `0..=1`); a malformed answer is an error, not something to guess around.
- 400 → fail closed; 402 → disable provider and use deterministic policy; 429/5xx → retry with backoff.
- Decisions are advisory: output never authorises a tool, an approval, or a lane change without the
  configured policy and confidence threshold.

## Security notes

- API keys come from environment variables or a `0600` key file; never commit them or put them in
  board data.
- Provider base URLs are trusted configuration: redirects are disabled and environment proxies are
  ignored unless explicitly enabled.
- Tool output from `http_fetch` is untrusted input; it is wrapped and cannot trigger tool execution on
  its own.
