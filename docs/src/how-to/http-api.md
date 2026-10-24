# HTTP API

This page describes how clients talk to a Forma agent over HTTP, using the `sync` and `sse` [runtimes](../reference_docs/auto-formaruntime.md). It is what you need to build your own back-end integration, or your own front-end.

> **📌 Note**: This is a hands-on walk through the API. Its full reference, with the OpenAPI descriptions you can generate clients from (`forma openapi sse`), is in [The runtime's API](../api/runtime.md).

> **📌 Note**: The `ai-sdk-v5` client uses the same three endpoints, but speaks the protocol of [Vercel's AI SDK](https://ai-sdk.dev/) (check the [client examples](../examples/clients/clients-intro.md)). The `telegram` and `whatsapp` runtimes receive webhooks from those platforms instead, so you never call them yourself.

## Headers

| Header            | Required                       | Purpose                                                                                       |
| ----------------- | ------------------------------ | --------------------------------------------------------------------------------------------- |
| `X-API-Key`       | Unless `no_api_key: true`      | Must match the server's `FORMA_AGENT_KEY` ([details](./forma-agent-api-key.md))                |
| `X-User-ID`       | Yes                            | The ID of the user. Forma does not verify it: your back-end must                              |
| `X-Session-ID`    | For `/v1/chat` and `/v1/resume`| The conversation, as returned by `/v1/init`                                                    |
| `X-User-Roles`    | No                             | Comma-separated roles, for [role based tool access](./role-based-tools.md) (e.g., `admin, editor`) |
| `X-Time-Zone`     | No                             | The user's IANA time zone (e.g., `America/Santiago`). Takes precedence over the agent's `timezone` |
| `X-Deployment-ID` | In multi-tenant servers        | Which agent should answer ([details](./deployment.md#multi-tenant-deployment))                 |
| `Content-Type`    | For `POST` requests            | `application/json`                                                                            |

## Endpoints

### `GET /v1/init` — start a conversation

Creates a new session for the user, and returns its ID.

```sh
curl http://localhost:8080/v1/init \
  -H "X-API-Key: key-not-for-production" \
  -H "X-User-ID: user-id"
```

```json
{"session_id":"5bb0dd24-bb70-418b-8dc2-e37ddeaade89"}
```

> **📌 Note**: Servers that do not keep conversations (`storage: none` in `runtime.yaml`) return a placeholder ID that you can send back, or not.

### `POST /v1/chat` — send a message

Sends a user message to the conversation in `X-Session-ID`. The body is a message in the [OpenAI chat format](https://platform.openai.com/docs/api-reference/chat/create): a `role` (always `user`) and a `content`, which can be a string:

```json
{"role":"user","content":"What day of the week is it today?"}
```

Or a list of parts, which is how images, audio and files are sent:

```json
{"role":"user","content":[{"type":"text","text":"Say hello in French, in one word."}]}
```

What comes back depends on the runtime (see [Responses](#responses-sync) below).

If the session is suspended (waiting for an approval, or for a delegated tool's result), the new message moves the conversation on: whatever was waiting is rejected (it never runs), and the agent answers the new message, knowing that.

### `POST /v1/resume` — answer a suspended session

When the agent wants to call a tool that needs [human approval](./human-in-the-loop.md), or a delegated tool, the session is suspended. This endpoint resumes it. The body is one of:

```json
{"tool_call_id": "call_vet2smzd", "reject": false, "arguments": {"order_id": "A-77", "amount": 35}}
```

```json
{"tool_call_id": "call_1kze06mp", "reject": false, "result": {"items": []}}
```

```json
{"tool_call_id": "call_vet2smzd", "reject": true}
```

The first approves a call that Forma will run, with these arguments (which may differ from the ones the agent proposed); the second answers a delegated tool with its result; the third rejects the call. `arguments` and `result` cannot be sent together.

A rejected call does not run: the agent is told that the user rejected it, and answers. So, once every pending call has been answered, `/v1/resume` replies like `/v1/chat` does, whether calls were approved or rejected, including when the agent stops again at another call that needs an answer. Each call can be answered only once.

## Responses (`sync`)

The `sync` runtime waits until the agent is done, and returns a single JSON object with a `status` and, sometimes, some `data`.

| `status`          | Returned by             | Meaning                                                                                  |
| ----------------- | ----------------------- | ---------------------------------------------------------------------------------------- |
| `completed`       | `/v1/chat`, `/v1/resume`| `data` is the assistant's message                                                         |
| `suspended`       | `/v1/chat`, `/v1/resume`| `data` describes the tool call waiting for approval: `session_id`, `tool_call_id`, `tool_name`, `arguments` |
| `still_suspended` | `/v1/resume`            | Other tool calls are still waiting for an answer                                          |

A completed answer looks like this:

```json
{"status":"completed","data":{"role":"assistant","content":[{"type":"text","text":"Nice to meet you, Peter! How are you doing today?"}]},"usage":{"tokens":[{"provider":"ollama","model":"llama3.1:8b","calls":1,"input_tokens":312,"cached_input_tokens":0,"output_tokens":14,"reasoning_tokens":0,"input_audio_tokens":0,"output_audio_tokens":0}],"tools":[],"job_id":"5e0c..."}}
```

`usage` is what the job consumed (see [Usage](../api/runtime.md#usage)).

> **📌 Note**: When the `start` workflow has several nodes, `content` contains one text part per node that answered, in order.

## Responses (`sse`)

The `sse` runtime streams [Server-Sent Events](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events): one `data:` line per event, each with a JSON object whose `type` says what happened. This is a real stream, from an agent that used the `current-date` tool (use `curl -N` to see events as they arrive):

```text
data: {"type":"start","message_id":"d5573bd1-3237-4565-aa47-5ce474310004"}

data: {"type":"start_step","id":"reply"}

data: {"type":"text_start","id":"d5573bd1-3237-4565-aa47-5ce474310004"}

data: {"type":"text_end","id":"d5573bd1-3237-4565-aa47-5ce474310004"}

data: {"type":"tool_input_start","tool_call_id":"call_zo5uwij9","tool_name":"current-date","tool_type":"current-date","bundle":null}

data: {"type":"tool_input_available","tool_call_id":"call_zo5uwij9","tool_name":"current-date","tool_kind":"current-date","input":{"timezone":"America/New_York"}}

data: {"type":"tool_output_available","tool_call_id":"call_zo5uwij9","output":{"date":"2026-10-20","time":"11:52","timezone":"America/Santiago","timezone_source":"the user's device","weekday":"Tuesday"}}

data: {"type":"text_start","id":"d5573bd1-3237-4565-aa47-5ce474310004"}

data: {"type":"text_delta","id":"d5573bd1-3237-4565-aa47-5ce474310004","delta":"Based on the output from the tool call, today's day of the week is Tuesday."}

data: {"type":"text_end","id":"d5573bd1-3237-4565-aa47-5ce474310004"}

data: {"type":"finish"}

data: {"tokens":[{"provider":"ollama","model":"llama3.1:8b","calls":2,"input_tokens":336,"cached_input_tokens":65,"output_tokens":39,"reasoning_tokens":0,"input_audio_tokens":0,"output_audio_tokens":0}],"tools":["current-date"],"job_id":"d5573bd1-3237-4565-aa47-5ce474310004"}
```

> **💡 Did you notice?** The LLM guessed a time zone (`America/New_York`), but the request had an `X-Time-Zone: America/Santiago` header, and the header won. The LLM is never trusted with something the client knows for sure.

### Events

| `type`                  | Meaning                                                                                       |
| ----------------------- | --------------------------------------------------------------------------------------------- |
| `start`                 | A job started. `message_id` is the job's ID                                                    |
| `start_step`            | A node started working (`id` is the node's ID)                                                 |
| `text_start`, `text_delta`, `text_end` | Text being written. Join the `delta`s to get the full text                     |
| `reasoning_start`, `reasoning_delta`, `reasoning_end` | The model's reasoning, for models that expose it                |
| `tool_input_start`      | A tool is about to be called. `tool_type` and `bundle` help a UI decide how to show it         |
| `tool_input_available`  | The arguments of the tool call                                                                 |
| `tool_output_available` | What the tool returned                                                                         |
| `file_available`        | A file produced during the turn (e.g., a generated image)                                      |
| `reference`             | A document used as a source (title, URL, snippet)                                              |
| `request_feedback`      | The session was suspended: this tool call needs approval (or, for delegated tools, a result). Answer it with `/v1/resume` |
| `still_suspended`       | Result of a `/v1/resume`: other tool calls are still waiting for an answer                     |
| `error`                 | Something went wrong. `error` describes it                                                     |
| `finish`                | The job is done                                                                               |

After `finish`, a last line (without a `type`) reports what the job consumed: the tokens used per provider and model (including the calls made by inner nodes and workflow tools, which are not streamed), and the tools that were called.

A suspension looks like this:

```text
data: {"type":"request_feedback","tool_call_id":"call_mym0as62","node_id":"reply","tool_name":"issue-refund","arguments":{"amount":"12.0","order_id":"C-9","reason":"Order arrived late"},"from_root":true,"tool_type":"api-call","bundle":null}

data: {"type":"finish"}
```

> **📌 Note**: Streaming in Forma is about *states* (a tool started, a tool finished), not necessarily about *tokens*. Whether text arrives in many small `text_delta`s or in a single one depends on the node's `stream` setting and on the LLM provider.

## Errors

Errors are returned with an HTTP status code and a message (as plain text, except for malformed bodies, which are described in a JSON object with an `error` field).

| Status | Example message                                  | Meaning                                                                       |
| ------ | ------------------------------------------------ | ----------------------------------------------------------------------------- |
| `400`  | `` {"error":"Failed to deserialize the JSON body ... missing field `role`"} `` | The request body is malformed                   |
| `400`  | `no x-session-id found in header`                | A required header is missing                                                   |
| `400`  | `tool call '...' is not waiting for an answer`   | `/v1/resume` for a call that does not exist, was already answered, or that a new message rejected |
| `401`  | `No API Key found`                               | The `X-API-Key` header is missing                                              |
| `403`  | `Incorrect API Key`                              | The API key is wrong                                                           |
| `403`  | `Session not owned by user: ...`                 | The session belongs to a different `X-User-ID`                                 |
| `404`  | `Session not found: ...`                         | The session does not exist                                                     |
| `404`  | `Agent deployment not found: ...`                | Multi-tenant only: the `X-Deployment-ID` does not exist                        |
| `409`  | `Session busy: ...`                              | Another message of this conversation is still being processed. Wait, and retry |
| `500`  | `Runtime Error: 'error sending request for url ...'` | The agent failed (e.g., the LLM provider could not be reached)             |
