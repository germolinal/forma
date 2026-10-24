# Usage and billing

Every job (every message answered, every resumed tool call) reports what it consumed: tokens, by provider and model, and the tools it called. This page is for whoever turns those numbers into a bill, or into a budget: what is reported, where, what each field means, and how to price it.

## What is reported

```json
{
  "tokens": [
    {"provider": "gcp.vertex_ai", "server_address": "aiplatform.googleapis.com",
     "model": "gemini-3.5-flash", "calls": 2,
     "input_tokens": 1336, "cached_input_tokens": 1024, "output_tokens": 39,
     "reasoning_tokens": 0, "input_audio_tokens": 0, "output_audio_tokens": 0}
  ],
  "tools": ["current-date", "code-execution"],
  "tool_usage": [{"tool_kind": "code-execution", "unit": "second", "quantity": 3}],
  "job_id": "d5573bd1-3237-4565-aa47-5ce474310004"
}
```

| Field | What it is |
| --- | --- |
| `tokens` | One entry per **provider, server and model**, adding up every call made to it in the job: the agent's own, and those of inner nodes and workflow tools, whose text is never streamed |
| `tokens[].server_address` | The host the calls went to (`api.openai.com`, `openrouter.ai`, `localhost`) |
| `tokens[].calls` | Round trips to the provider (some plans bill per request) |
| `tokens[].input_tokens`, `output_tokens` | Every token sent, and every token generated |
| `tokens[].cached_input_tokens`, `input_audio_tokens` | **Part of** `input_tokens`, priced differently (cached input is discounted; audio costs more) |
| `tokens[].reasoning_tokens`, `output_audio_tokens` | **Part of** `output_tokens` (reasoning is billed as output) |
| `tools` | The kind of every tool called, once per call |
| `tool_usage` | What tools consumed in units of their own, summed per kind and unit (absent when nothing) |
| `job_id` | The job, as its `start` event and its traces name it |

Results Forma serves from a conversation's cache cost nothing, and are not counted. A job that fails reports nothing to the client; what it spent until then is still in the [metrics](#metrics).

## Where it is reported

| Client | Where |
| --- | --- |
| `sse`, `ai-sdk-v5` | The last line of every stream (`/v1/chat`; and `/v1/resume`, for `sse`): the one without a `type` |
| `sync` | `usage`, next to `status` and `data`, in every answer |
| `ai-sdk-v5`'s `/v1/resume` | `usage`, next to `status` and `data` |
| `whatsapp`, `telegram` | The webhook's answer: a list, one entry per job the delivery started |

A job that only answers a tool call while others wait (`still_suspended`) runs nothing, and reports empty usage. Usage is reported per job, not per user or deployment: whoever forwards the request (your [proxy](./multi-tenant.md#6-usage-and-billing)) knows those, and attributes it.

## Pricing tokens

The price of a token depends on **who served it** and **which model**: the key is `(provider, server_address, model)`.

### `provider`

Providers are named as [OpenTelemetry's GenAI conventions](https://opentelemetry.io/docs/specs/semconv/gen-ai/) name them (`gen_ai.provider.name`), the names traces and observability tools use, which are not the names agent files use:

| In the agent file (`llm.provider`) | Reported as |
| --- | --- |
| `ollama` | `ollama` |
| `openai-v1` | `openai` |
| `groq` | `groq` |
| `vertex-ai` | `gcp.vertex_ai` |
| (the `google-gen-image` tool) | `gcp.vertex_ai` |

### `server_address`

`provider` names the API a call spoke, not the company that bills it: `openai-v1` reaches any OpenAI-compatible server (OpenAI itself, OpenRouter, Together, a vLLM server of your own), and every one of them is `openai`. `server_address`, the host the call went to (OpenTelemetry's `server.address`), tells them apart:

| `provider` | `server_address` | Who bills |
| --- | --- | --- |
| `openai` | `api.openai.com` | OpenAI |
| `openai` | `openrouter.ai` | OpenRouter |
| `openai` | `localhost`, or your own host | Nobody: your servers |
| `gcp.vertex_ai` | `aiplatform.googleapis.com`, or `<region>-aiplatform.googleapis.com` | Google Cloud (prices can differ by region) |
| `groq` | `api.groq.com` | Groq |
| `ollama` | Where your Ollama runs | Nobody: your servers |

Keep a table of the hosts your agents use, and what each charges.

### `model`

The model the provider says it *served*, which is not always the one configured: an alias can be reported as the dated version that answered. Price on that: it is what the provider bills. Image generation (`google-gen-image`) reports its tokens under its own model, as an entry of its own.

### The arithmetic

The subsets are already in their totals, so never add them:

```text
cost = Σ over tokens[]:
         (input_tokens − cached_input_tokens − input_audio_tokens) × input price
       + cached_input_tokens × cached input price
       + input_audio_tokens  × audio input price
       + (output_tokens − output_audio_tokens) × output price      (reasoning included)
       + output_audio_tokens × audio output price
       + calls × per-request price                                   (if your plan has one)
```

Ollama, running on your own machines, costs nothing per token.

## Pricing tools

Some tools cost money beyond the tokens: they call paid APIs, or run on paid infrastructure. `tools` lists every call by kind; `tool_usage` adds what is metered in units.

| Kind | Costs | Reported |
| --- | --- | --- |
| `code-execution` | The sandbox's run time (a Cloud Run job, or your Docker host) | Each call in `tools`, and its seconds in `tool_usage` (`unit: second`) |
| `google-gen-image` | Vertex AI tokens | Each call in `tools`, and its tokens in `tokens` |
| `tavily-web-search`, `google-agent-search` | A paid search API, per request | Each call in `tools` |
| `mailgun` | Mailgun, per email | Each call in `tools` |
| `mcp`, `api-call`, `fetch` | Whatever the server called charges | Each call in `tools` |
| `knowledgebase`, `workspace` | Storage reads and writes (Google Cloud Storage) | Each call in `tools` |
| `template`, `workflow`, `current-date`, `delegated` | Nothing of their own (a workflow's inner calls are counted as theirs) | Each call in `tools` |

## Metrics

The runtime also exports what it consumed as OpenTelemetry metrics (meter `forma`), when it [exports telemetry](../how-to/observability.md). They are for dashboards and budgets: they add up the whole server, with no user, conversation or deployment labels (one series per user would not scale), so per-customer billing reads the reported usage above instead.

| Metric | Labels | Counts |
| --- | --- | --- |
| `forma.agent.runs` | `mode` (`new`, `resume`), `outcome` (`completed`, `suspended`, `errored`, `dropped`: the client left, or a timeout cut the request) | Jobs |
| `forma.llm.calls` | `provider`, `server_address`, `model` | Provider round trips |
| `forma.llm.tokens` | `provider`, `server_address`, `model`, `token_type` (`input`, `output`) | Tokens. The two types are disjoint: their sum is the total |
| `forma.llm.tokens.subset` | `provider`, `server_address`, `model`, `token_type` (`cached_input`, `reasoning`, `input_audio`, `output_audio`) | The parts of `forma.llm.tokens` priced differently. Never add them to it |
| `forma.tool.calls` | `tool_kind` | Tool calls |
| `forma.tool.usage` | `tool_kind`, `unit` | What tools consumed, in their unit |

Jobs that fail, or whose request goes away, are counted too (`errored`, `dropped`), with what they spent until then.
