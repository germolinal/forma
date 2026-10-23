# How a request is processed

The [Components](./building-blocks.md) section describes the pieces of a Forma agent: LLMs, Nodes, Workflows and Agents. This section puts them in motion. It follows a single message from the moment a client sends it, to the moment the answer is stored, so you know exactly what happens (and in what order) every time someone talks to your agent.

Knowing this pays off in three situations: when you read [traces](../how-to/dory-traces.md), when something fails and you need to know *where*, and when you design an agent and need to decide what goes into a prompt, a tool, or a separate node.

## The big picture

```text
 Client                Runtime                     Agent                        LLM provider
   │                      │                          │                               │
   │── message ──────────►│ 1. check API key, headers│                               │
   │                      │ 2. load agent & session  │                               │
   │                      │ 3. refuse if busy or     │                               │
   │                      │    waiting for a human   │                               │
   │                      │ 4. store user message    │                               │
   │                      │── run ──────────────────►│ 5. run workflow, layer by layer│
   │                      │                          │    each node:                  │
   │                      │                          │    ├─ render system prompt     │
   │                      │                          │    ├─ pick allowed tools       │
   │                      │                          │    ├─ triage ─────────────────►│
   │                      │                          │    │◄────── text or tool calls │
   │                      │                          │    ├─ run tools (in parallel)  │
   │                      │                          │    └─ triage again...         │
   │◄── events (stream) ──│◄── events ───────────────│                               │
   │                      │ 6. store the answer      │                               │
   │◄── final answer ─────│                          │                               │
```

Let's go through it step by step.

## 1. The runtime receives the message

A message arrives through one of the [runtimes](../reference_docs/auto-formaruntime.md) (e.g., `sync`, `sse`, `telegram`). Runtimes translate between the client's format (a WhatsApp webhook, an AI SDK message, an OpenAI-style message) and Forma's own internal format. From this point on, the agent does not know—or care—which channel the message came from.

Before anything else, the runtime reads the request's headers:

| Header            | Purpose                                                                                      |
| ----------------- | -------------------------------------------------------------------------------------------- |
| `X-API-Key`       | Must match `FORMA_AGENT_KEY`, unless the runtime has `no_api_key: true` ([details](../how-to/forma-agent-api-key.md)) |
| `X-User-ID`       | Who is talking. Forma does not verify it: your back-end does ([why](./architecture.md))       |
| `X-Session-ID`    | Which conversation this message belongs to                                                    |
| `X-User-Roles`    | Which [restricted tools](../how-to/role-based-tools.md) this user may use                     |
| `X-Time-Zone`     | The user's time zone (e.g., `America/Santiago`), used by the `current-date` tool              |
| `X-Deployment-ID` | Which agent should answer, in [multi-tenant](../how-to/deployment.md#multi-tenant-deployment) deployments |

## 2. The agent and the session are loaded

In a single-tenant server, the agent is already in memory. In a multi-tenant server, it is read from the database on every request (which is why a redeployed agent answers the very next message).

Then, the conversation is loaded from storage. Not all of it, though: only the latest messages (30, by default; check `max_history_messages` in the [runtime settings](../reference_docs/auto-formaruntime.md)). Older messages are still stored, but the LLM will not see them. (Read more on why in [Tokens, context windows and cost](../learn/tokens.md).)

> **📌 Note**: Forma checks that the session belongs to the user in `X-User-ID`. If it does not, the request is rejected with a `403 Forbidden`.

## 3. Some requests are refused

Forma refuses a new message, with a `409 Conflict`, when **the session is busy**: another message in the same conversation is still being processed. Forma processes one message per conversation at a time, because two runs writing to the same history would produce a conversation that makes no sense. (If a run crashes without finishing, its claim on the session expires after one hour.)

This check happens *before* the message is stored, so a refused message never appears in the history as if the agent had been asked it.

> **📌 Note**: On the `whatsapp` and `telegram` runtimes, a message that finds the session busy waits a little for its turn (15 seconds), before it is refused, for the app to deliver it again later. And since these apps sometimes deliver a message twice, each one is stored under the app's own ID, and a message that is already stored is not answered again.

A **suspended** session, on the other hand, does take new messages. It is waiting for a human to approve a tool call, or for the client to run a [delegated tool](../how-to/human-in-the-loop.md), but a new message moves the conversation on: whatever was waiting is rejected (it never runs), and the agent answers the new message, knowing that.

## 4. The message is stored and a job starts

The user's message is appended to the session, and a **job** starts. A job is one run of the agent, with its own ID (the `job_id` you will see in traces and logs). The first event sent to the client is a `start` event, carrying that ID.

## 5. The agent runs its workflow

The agent runs its `start` [workflow](./building-blocks.md#3-workflows). Workflows are executed in **layers**: Forma reads the [templates](../how-to/templates.md) in your nodes' system prompts to find out which node depends on which, and groups them. For example, this workflow:

```yaml
start:
  output: verdict
  nodes:
    - id: grammar
      llm:
        provider: ollama
      system_prompt: List the grammar mistakes in the user's text. Be brief.
    - id: tone
      llm:
        provider: ollama
      system_prompt: Describe the tone of the user's text in one sentence.
    - id: verdict
      llm:
        provider: ollama
      system_prompt: |
        Write a two-line review of a text, based on these notes.

        Grammar: {{grammar}}

        Tone: {{tone}}
```

Becomes this plan (you can check it with `forma expand -c`):

```yaml
plan:
- - id: grammar   # Layer 1: these two run at the same time...
  - id: tone
- - id: verdict   # Layer 2: ...and this one waits for both
```

Nodes within a layer run **in parallel**; layers run **one after the other**. This is why you never need to say "run these in parallel": independent nodes simply are.

### Inside each node

Every node follows the same steps:

1. **Render the system prompt.** Template fields like `{{grammar}}` are replaced with the outputs of earlier nodes (or with the agent's `variables`).
2. **Pick the tools this user may use.** Tools with a `just_for` list are removed if the user does not have one of the roles. The LLM will never know they existed.
3. **Triage.** The LLM receives the system prompt, the conversation, and the descriptions of the available tools. It answers with either *text* (a reply) or *tool calls* (a request to run some tools, with some arguments).
4. **Run the tools.** If the LLM asked for tools, Forma checks each call:
   * Tools with `confirm_before_calling: true`, and `delegated` tools, are **not** run. The session is suspended, and the client is asked to approve the call (or to run it). The job ends here. (Read more in [Human in the loop](../how-to/human-in-the-loop.md).)
   * Every other tool is run, **all of them in parallel**. Their results are added to the conversation.
5. **Triage again.** The LLM sees the tools' results, and decides again: reply, or call more tools. This loop repeats until the LLM replies with text, which becomes the node's answer.
6. **Summarise, only if needed.** If the loop runs `max_iterations` times (10, by default) and the LLM still wants more tools, Forma stops it. An LLM is called one last time, without tools, to tell the user what was found, what is missing, and to ask whether to go on.

> **📌 Note**: A [workflow tool](../reference_docs/auto-workflowtool.md) runs its own workflow, with these same steps, but in a **fresh session**: it does not see the conversation, only the message rendered from its `input_prompt`.

## 6. Events are streamed, and the answer is stored

While all of this happens, the agent emits **events**: a node started, text was written, a tool was called with these inputs, a tool returned this output, and so on. Streaming clients (`sse`, `ai-sdk-v5`) send these to the client as they happen, which is how a chat UI can show "searching the web..." while the agent works. The `sync` runtime waits for the end, and returns only the final message. (Check the [HTTP API](../how-to/http-api.md) for the exact format.)

Every node reports its steps and tool calls, but only the nodes listed in the workflow's `output` send their text. In the example above, the user reads the `verdict`; the notes on grammar and tone are only for the node that reads them.

Forma also emits a usage event for every LLM call (including those made by inner nodes and workflow tools, which never reach the client), so you can know exactly how many tokens a request cost.

When the workflow finishes, a `finish` event is sent, the agent's answer is appended to the session, and the session is released for the next message.

## What this means when designing an agent

* **Every node is a separate LLM call (or several).** A workflow with three nodes is at least three calls, with their cost and latency. Put nodes in parallel when you can: you pay the same, but wait less.
* **Tool calls are decided, not guaranteed.** The LLM chooses whether to call a tool. If something *must* happen every time, it should be a node, not a tool.
* **What the LLM does not see does not exist.** Messages beyond `max_history_messages`, tools removed by `just_for`, and the conversation (for workflow tools) are simply not there. This is a feature: it is how Forma enforces rules in code rather than in prompts.
* **Slow tools make slow agents.** A node waits for all its tools to finish before triaging again.
