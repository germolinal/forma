# The runtime's API

This is how your back end talks to a Forma agent: starting conversations, sending messages, reading answers (whole, or streamed), and answering the tool calls an agent waits on. It applies to the `sync`, `sse` and `ai-sdk-v5` clients; chat apps call the `whatsapp` and `telegram` clients themselves ([WhatsApp](../how-to/whatsapp.md), [Telegram](../how-to/telegram.md)).

Every field of every request and answer is in the OpenAPI descriptions ([sync](./openapi/sync.json), [sse](./openapi/sse.json), [ai-sdk-v5](./openapi/ai-sdk-v5.json); or `forma openapi <client>`, or the [explorer](./explorer.html)). This page explains how they fit together. For a hands-on start with `curl`, read [HTTP API](../how-to/http-api.md) first.

## At a glance

```text
GET  /v1/init     → {"session_id": "..."}                      start a conversation
POST /v1/chat     → the agent's answer (JSON, or a stream)     send the user's message
POST /v1/resume   → the agent's answer (JSON, or a stream)     answer a tool call it waits on
```

The runtime listens on `PORT` (default `8080`). Which client it is, how many messages the agent reads, and how large a request may be, are set in its [runtime file](../reference_docs/auto-formaruntime.md):

```yaml
client: sse                # sync, sse or ai-sdk-v5 (or whatsapp, telegram)
no_api_key: false          # the default: requests need X-API-Key
max_history_messages: 30   # the default: how many past messages the agent reads
max_request_mb: 25         # sse only: the largest /v1/chat body
storage: mongo             # where conversations are kept: mongo, file, or none
```

## Who is who: the headers

Forma checks one thing itself: the API key, which identifies your back end. Everything else it is *told*, and trusts.

| Header | Sent with | Required | What Forma does with it |
| --- | --- | --- | --- |
| `X-API-Key` | Every request | Unless `no_api_key: true` | Compares it with the server's `FORMA_AGENT_KEY`: missing is `401`, wrong is `403`. A server without `FORMA_AGENT_KEY` answers `500` to everything |
| `X-User-ID` | Every request | Yes (`/v1/init`: when conversations are kept) | Who the user is. A conversation belongs to the user who started it: anyone else gets `403` |
| `X-Session-ID` | `/v1/chat`, `/v1/resume` | When conversations are kept | Which conversation, as `/v1/init` returned it |
| `X-User-Roles` | `/v1/chat`, `/v1/resume` | No | Comma-separated roles (`admin, editor`): the tools [restricted to roles](../how-to/role-based-tools.md) (`just_for`) are only offered to users who have one |
| `X-Time-Zone` | `/v1/chat`, `/v1/resume` | No | The user's IANA time zone (`America/Santiago`), which the `current-date` tool answers in. It wins over the agent's `timezone`, and over whatever the LLM guesses. Ignored if it is not a time zone |
| `X-Deployment-ID` | Every request | Multi-tenant runtimes only | Which deployed agent answers (`404` if none). See [multi-tenant platforms](./multi-tenant.md) |
| `Content-Type` | `POST` | Yes | `application/json` |

> **❗Important**: because Forma believes these headers, only your back end may set them. Authenticate the user there, and send *their* id and roles; never pass on headers a client sent. And keep the API key on the server: it is not a user's credential.

## Conversations

A conversation (a *session*) is what the agent remembers: every message, its tool calls and their results, and whatever it is waiting for.

- **Starting one.** `GET /v1/init` creates it and returns its id. It never ends: start a new one when the user wants a fresh start. Listing, reading, renaming and deleting conversations is [the manager's](./manager.md#sessions) job.
- **What the agent reads.** Each message is answered with the last `max_history_messages` messages of the conversation (30 by default), so long conversations do not dilute the system prompt or grow the bill forever. The stored conversation keeps everything.
- **One message at a time.** While the agent answers a message, the conversation is busy: another `/v1/chat` or `/v1/resume` on it is refused with `409`, before anything is stored. Wait for the answer (or for `finish`, when streaming), and send it again. A run that never finishes (a crashed server) frees the conversation after an hour.
- **Without storage.** With `storage: none`, nothing is kept: `/v1/init` returns a placeholder, `X-Session-ID` is not needed, and every message is a conversation of its own. `/v1/resume` is then impossible (`400`), so tools that wait for an answer cannot be used.

> **❗Important**: keep the request open until the answer is complete. The agent's answer is stored at the end of the request, so if your client disconnects mid-answer (or a proxy times it out), the user's message is stored without it. Streams send no keep-alive comments: give proxies a read timeout longer than your agent's slowest answer.

## Messages

### `sync` and `sse`: OpenAI's chat messages

`/v1/chat` takes one message, the user's, in the shape of [OpenAI's chat messages](https://platform.openai.com/docs/api-reference/chat/create): `role` is `user`, and `content` is a string, or a list of parts.

```json
{"role": "user", "content": "What day is it?"}
```

```json
{
  "role": "user",
  "content": [
    {"type": "text", "text": "What does this receipt say?"},
    {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo..."}},
    {"type": "file", "file": {"file_data": "data:application/pdf;base64,JVBERi0...", "filename": "receipt.pdf"}},
    {"type": "input_audio", "input_audio": {"data": "UklGRi...", "format": "wav"}}
  ]
}
```

| Part | Carries | Notes |
| --- | --- | --- |
| `text` | `text` | |
| `image_url` | `image_url.url`: a `data:` URL, or a URL the LLM provider can reach; `detail` (`low`, `auto`, `high`) | Large pictures are shrunk before they reach the model |
| `file` | `file.file_data`: a `data:` URL; `filename` | PDFs, documents, and audio in any container (`audio/ogg`, ...) |
| `input_audio` | `input_audio.data` (bare base64) and `format` (`wav`, `mp3`, `flac`, `opus`, `pcm16`) | |

Whether a model can read a given kind of part is up to its provider. Attachments travel inline, as base64, which inflates them by a third: the whole body may be **2 MB** with `sync`, and `max_request_mb` (25 MB by default) with `sse`; a larger one is refused with `413`.

The message must be the user's, and have content: otherwise it is refused with `400` and a JSON body, `{"error": "..."}`. Any `id` you add to it is ignored.

### `ai-sdk-v5`: the AI SDK's `UIMessage`

The `ai-sdk-v5` client takes the message as the AI SDK's `useChat` sends it, an `id`, a `role` and its `parts`, but **only the last one**: Forma keeps the conversation, while the AI SDK's default is to send all of it. Set the transport's `prepareSendMessagesRequest` to send `messages.at(-1)` (see the [AI SDK example](../examples/clients/vercel-aisdk-5-persist.md)).

```json
{"id": "msg_1", "role": "user", "parts": [{"type": "text", "text": "What day is it?"}]}
```

Parts are the AI SDK's `text` and `file` parts, which is what `useChat` sends; the OpenAI parts above work too.

```json
{"type": "file", "mediaType": "image/png", "url": "data:image/png;base64,iVBORw0KGgo...", "filename": "receipt.png"}
```

A picture's `url` can be a `data:` URL, or a URL the model's provider can reach. Any other file must be inline, as a `data:` URL: Forma does not fetch links, and refuses a message with one (`400`, saying which file). Files in answers (`/v1/resume`'s message) come back as AI SDK `file` parts too.

## Answers

### `sync`: one JSON object

`/v1/chat` waits until the agent is done, and answers with a `status`:

```json
{"status": "completed", "data": {"role": "assistant", "content": [{"type": "text", "text": "Today is Friday."}]}, "usage": {"tokens": [...], "tools": [], "job_id": "d557..."}}
```

```json
{"status": "suspended", "data": {"session_id": "6ce2...", "tool_call_id": "call_vet2smzd", "tool_name": "issue-refund", "arguments": {"order_id": "A-77", "amount": 35}}, "usage": {...}}
```

`completed` carries the assistant's message: everything the agent said, in order, one text part per node of the `start` workflow that answered, and any files it produced (`image_url` or `file` parts). Its tool calls are not in it. `suspended` means the agent stopped at a call that needs an answer (see [below](#tool-calls-that-wait-for-an-answer)). `/v1/resume` adds a third status, `still_suspended`. Every answer also says what the job consumed, under `usage` (see [Usage](#usage)).

### Streams

The `sse` and `ai-sdk-v5` clients answer `/v1/chat` with a stream of [Server-Sent Events](https://html.spec.whatwg.org/multipage/server-sent-events.html) (`Content-Type: text/event-stream`). Each event is a `data:` line holding one JSON object, followed by a blank line:

```text
data: {"type":"start","message_id":"d557..."}

data: {"type":"text_delta","id":"d557...","delta":"Today is Friday."}

```

What a stream always does:

1. Its first event is `start`, whose `message_id` names the job.
2. Then come events, in the order things happen: text and reasoning as it is written, tool calls and their results, files, sources.
3. Then `finish`: the job is done **and its turn is stored**, so the user can send the next message.
4. The last line has no `type`: it is the job's [usage](#usage).

An `error` event instead ends the stream at once, without `finish` or usage: the agent failed (its provider could not be reached, a tool failed hard, or Forma itself stopped unexpectedly, which is a bug). A stream never ends in silence: if it ends without `finish`, it ends with `error`. Errors found before the agent starts (a missing header, a busy conversation) are not streamed: they are answered with a status, like any request.

Browsers' `EventSource` cannot send a `POST`, so read the stream with `fetch`. In TypeScript, for instance:

```ts
const res = await fetch(`${FORMA_URL}/v1/chat`, {
  method: "POST",
  headers: {
    "Content-Type": "application/json",
    "X-API-Key": process.env.FORMA_AGENT_KEY!,
    "X-User-ID": user.id,
    "X-Session-ID": sessionId,
  },
  body: JSON.stringify({ role: "user", content: text }),
});
if (!res.ok) throw new Error(`${res.status}: ${await res.text()}`);

const reader = res.body!.pipeThrough(new TextDecoderStream()).getReader();
let buffer = "";
for (;;) {
  const { value, done } = await reader.read();
  if (done) break;
  buffer += value;
  let end;
  while ((end = buffer.indexOf("\n\n")) >= 0) {
    const frame = buffer.slice(0, end);
    buffer = buffer.slice(end + 2);
    const data = frame
      .split("\n")
      .filter((l) => l.startsWith("data:"))
      .map((l) => l.slice(5).trimStart())
      .join("\n");
    if (!data) continue;
    const line = JSON.parse(data);
    if ("type" in line) onEvent(line); // an event
    else onUsage(line); // the last line: what the job consumed
  }
}
```

#### `sse` events

`type` is in `snake_case`. Every field is in the `SSEvent` schema.

| `type` | Fields | Meaning |
| --- | --- | --- |
| `start` | `message_id` | The job started |
| `start_step` | `id` | A node of the workflow started working (`id` is the node's) |
| `text_start`, `text_delta`, `text_end` | `id`, `delta` | Text being written: append the `delta`s of the same `id`. Whether it arrives in many deltas or in one depends on the node's `stream` setting and on the provider |
| `reasoning_start`, `reasoning_delta`, `reasoning_end` | `id`, `delta` | The model's reasoning, for models that expose it |
| `tool_input_start` | `tool_call_id`, `tool_name`, `tool_type`, `bundle` | A tool is about to be called. `tool_type` is what it is (`api-call`, `mcp`, `workspace`, `code-execution`, ...), and `bundle` the MCP server or definition it comes from: what a UI decides how to show it by |
| `tool_input_available` | `tool_call_id`, `tool_name`, `tool_kind`, `input` | The call's arguments |
| `tool_output_available` | `tool_call_id`, `output` | What the call returned |
| `file_available` | `id`, `part` | A file produced in the turn (a generated picture, a fetched document): an `image_url` or `file` part |
| `reference` | `title`, `url`, `snippet`, `extractive_answer` | A document used as a source |
| `grounding_score` | `score` | How well the answer is grounded in its sources |
| `request_feedback` | `tool_call_id`, `tool_name`, `arguments`, `node_id`, `from_root`, `tool_type`, `bundle` | The agent stopped at this call, which needs an answer: see [below](#tool-calls-that-wait-for-an-answer). `finish` follows |
| `still_suspended` | | The answer to a `/v1/resume`: other calls of the turn still wait for an answer |
| `error` | `error` | The agent failed. The stream ends |
| `finish` | | The job is done, and its turn stored |

The `SSEvent` schema also lists `usage` and `tool_usage`: the runtime adds them up into the last line, and never streams them.

#### `ai-sdk-v5` events

The AI SDK's [UI message stream](https://ai-sdk.dev/docs/ai-sdk-ui/stream-protocol), which `useChat` reads as it is. `type` is in `kebab-case`, and field names in `camelCase`. Every field is in the `AiSdkV5Event` schema.

| `type` | From the `sse` event |
| --- | --- |
| `start` (`messageId`) | `start` |
| `text-start`, `text-delta`, `text-end` | The same |
| `reasoning-start`, `reasoning-delta`, `reasoning-end` | The same |
| `tool-input-start` (`toolCallId`, `toolName`) | `tool_input_start` |
| `tool-input-available` (`toolCallId`, `toolName`, `input`) | `tool_input_available`, **and** `request_feedback`: a call that needs an answer is one that gets no `tool-output-available` |
| `tool-output-available` (`toolCallId`, `output`) | `tool_output_available` |
| `file` (`url`, `mediaType`) | `file_available`, as a `data:` URL |
| `data-reference` (`data`: `title`, `url`, `snippet`, `extractiveAnswer`) | `reference` |
| `data-grounding-score` (`data`: `score`) | `grounding_score` |
| `data-still-suspended` (`data`: `{}`) | `still_suspended` |
| `error` (`errorText`) | `error` |
| `finish` | `finish` |

`start_step` has no equivalent, and is not sent.

## Tool calls that wait for an answer

Some calls are not the agent's to make alone ([human in the loop](../how-to/human-in-the-loop.md)):

| The tool | Who runs it | What your client sends back |
| --- | --- | --- |
| Any tool with `confirm_before_calling: true` | Forma, once approved | An approval, with the arguments to run it with (the agent's, or the user's corrections); or a rejection |
| A [`delegated`](../reference_docs/auto-delegatedtool.md) tool (`tool_type: delegated`) | Your client | The tool's `result`; or a rejection |

When the agent reaches one, it stops, the conversation is *suspended*, and the answer says so:

| Client | How a call that waits is announced |
| --- | --- |
| `sync` | `"status": "suspended"`, with the call |
| `sse` | A `request_feedback` event, then `finish` |
| `ai-sdk-v5` | A `tool-input-available` that gets no output, then `finish` |

Answer it with `/v1/resume`, on the same conversation (same headers):

```json
{"tool_call_id": "call_vet2smzd", "reject": false, "arguments": {"order_id": "A-77", "amount": 35}}
```

```json
{"tool_call_id": "call_1kze06mp", "reject": false, "result": {"items": []}}
```

```json
{"tool_call_id": "call_vet2smzd", "reject": true}
```

- **`arguments`** approves the call: Forma runs the tool with them.
- **`result`** answers a delegated tool: it is the call's result, as if the tool had returned it.
- **`reject: true`** rejects it: the tool does not run, and the agent is told that the user rejected it.

Send exactly one of `arguments` and `result`, unless rejecting (`400` otherwise). Each call is answered once: answering one that is not waiting (answered already, rejected, or unknown) is a `400`.

**A turn can stop at several calls at once** (an agent may call tools in parallel). The agent goes on only once every one of them is answered: until then, each `/v1/resume` answers `still_suspended` (`sync`, `ai-sdk-v5`), or streams a single `still_suspended` event (`sse`). The last answer resumes the agent, and is answered like `/v1/chat`: completed, or suspended at a new call.

**A new message moves on.** If the user writes instead of answering, whatever was waiting is rejected (it never runs), and the agent answers the new message, knowing that. Nothing has to be cleaned up.

**After a reload**, a client finds the calls a conversation waits on in the manager's [`GET /sessions/{id}`](./manager.md#sessions) (`pending_tool_calls`).

The `ai-sdk-v5` client answers `/v1/resume` with JSON, as `sync` does (its message in the AI SDK's shape, and `usage`), not with a stream.

## Usage

Every job reports what it consumed, so a platform can bill for it without knowing anything about providers:

```json
{
  "tokens": [
    {"provider": "gcp.vertex_ai", "server_address": "aiplatform.googleapis.com",
     "model": "gemini-3.5-flash", "calls": 2,
     "input_tokens": 1336, "cached_input_tokens": 1024, "output_tokens": 39,
     "reasoning_tokens": 0, "input_audio_tokens": 0, "output_audio_tokens": 0}
  ],
  "tools": ["current-date", "api-call"],
  "tool_usage": [{"tool_kind": "code-execution", "unit": "second", "quantity": 3}],
  "job_id": "d5573bd1-3237-4565-aa47-5ce474310004"
}
```

- `tokens` has one entry per provider (`ollama`, `openai`, `groq`, `gcp.vertex_ai`: OpenTelemetry's names), server (`server_address`: the host, which tells OpenAI from other OpenAI-compatible servers) and model, with every call made, including those of inner nodes and workflow tools, whose text is never streamed. `cached_input_tokens` and `input_audio_tokens` are *part of* `input_tokens`, and `reasoning_tokens` and `output_audio_tokens` part of `output_tokens`: they are priced differently, but must not be added twice. Results served from the session's cache cost nothing, and are not counted.
- `tools` lists the kind of every tool called, once per call.
- `tool_usage` (absent when empty) is what tools consumed in their own units: the seconds a code-execution sandbox ran, for instance.

Where it is reported:

| Client | Where |
| --- | --- |
| `sse`, `ai-sdk-v5` | The last line of the stream of `/v1/chat`, and of `/v1/resume` (`sse`) |
| `whatsapp`, `telegram` | The webhook's answer: a list, one entry per job the delivery started |
| `sync`; `ai-sdk-v5`'s `/v1/resume` | `usage`, next to `status` and `data` (empty for `still_suspended`, which runs nothing) |

A job that fails reports nothing to the client, but its spend is still recorded in the runtime's [telemetry](../how-to/observability.md). How to price all of it: [Usage and billing](./usage-and-billing.md).

## Errors

Errors are answered with a status, and a message in plain text, except for a `/v1/chat` body that is not a user message, which gets a JSON `{"error": "..."}`.

| Status | Why | What to do |
| --- | --- | --- |
| `400` | The `/v1/chat` body is not JSON, not a user message, or has no content (JSON body) | Fix the request |
| `400` | A header is missing (`no x-session-id found in header`) | Send it |
| `400` | `/v1/resume`: the call is not waiting for an answer, or `arguments`/`result` are both there or both missing; or the body is not valid JSON | Read the conversation again: the call was answered, or a new message rejected it |
| `401` | No `X-API-Key` | |
| `403` | Wrong `X-API-Key` (`Incorrect API Key`) | |
| `403` | The conversation is another user's (`Session not owned by user`) | |
| `404` | No such conversation (`Session not found`), or, multi-tenant, deployment (`Agent deployment not found`) | |
| `409` | The conversation is busy (`Session busy`) | Wait for the answer that is running, and send it again |
| `413` | The body is too large (2 MB; `max_request_mb` with `sse`) | Send smaller attachments |
| `415` | `/v1/resume` without `Content-Type: application/json` | |
| `422` | `/v1/resume`: the body is JSON, but not a resume request (a missing `tool_call_id` or `reject`) | |
| `500` | The agent failed (its LLM provider could not be reached, a tool failed hard), or the server has no `FORMA_AGENT_KEY` | Check the runtime's logs |

## Next

- [Running a multi-tenant platform](./multi-tenant.md): one runtime for many agents, behind your proxy.
- [The manager's API](./manager.md): deploying agents, and reading conversations back.
