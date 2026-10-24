# Building on Forma's APIs

Everything Forma serves is plain HTTP, described in [OpenAPI](https://spec.openapis.org/oas/v3.1.0) and generated from Forma's own code, so the description is always what the server does. This section is the reference for building around Forma: a front end, a back end that talks to agents, a platform that deploys them, or a service that bills for them.

> **💡 Using a coding agent?** Give it the OpenAPI description of the API you build on (below), and the page that explains it ([the runtime's](./runtime.md), or [the manager's](./manager.md)). The descriptions say what every field means; the pages say how the requests fit together.

## What there is to call

| Server | What it does | Who calls it | Explained in |
| --- | --- | --- | --- |
| **The runtime** (`forma serve`, the `forma` and `forma-multi-tenant` images) | Serves an agent: conversations, messages, and the tool calls they wait on | Your back end | [The runtime's API](./runtime.md) |
| **The runtime, as a webhook** (`client: whatsapp` or `telegram`) | Serves an agent on a chat app | WhatsApp (Meta) or Telegram | [WhatsApp](../how-to/whatsapp.md), [Telegram](../how-to/telegram.md) |
| **The manager** (`forma-manager`) | Deploys agents to multi-tenant runtimes, keeps users' OAuth tokens, lists and edits conversations | Your back end | [The manager's API](./manager.md) |

A runtime serves one kind of client, which its [runtime file](../reference_docs/auto-formaruntime.md) names (`client:`). The paths are the same for every client (`/v1/init`, `/v1/chat`, `/v1/resume`), but what they take and return is not, so each client has a description of its own:

| `client:` | Answers with | For |
| --- | --- | --- |
| `sync` | One JSON object, once the agent is done | Back ends that ask an agent something |
| `sse` | A stream of [Server-Sent Events](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events): what the agent does, as it does it | Apps that show the agent at work |
| `ai-sdk-v5` | The stream of [Vercel's AI SDK](https://ai-sdk.dev/) (v5) | Front ends built with the AI SDK |
| `whatsapp` | Messages over WhatsApp | A WhatsApp number |
| `telegram` | Messages over Telegram | A Telegram bot |

## How the pieces fit

```text
                        ┌──────────────┐  deploys agents  ┌───────────────────┐
  people ──► your app ─►│ your back end│ ───────────────► │   forma-manager   │
                        │  (auth, UI)  │                   └─────────┬─────────┘
                        └──────┬───────┘                             │ MongoDB
                               │ X-API-Key, X-User-ID, ...           │
                               ▼                                     ▼
                        ┌──────────────┐                   ┌───────────────────┐
                        │ forma runtime│ ◄── reads agents, │  agents, sessions │
                        │ (one client) │     conversations │   tokens          │
                        └──────────────┘                   └───────────────────┘
```

**Forma does not authenticate people.** The runtime checks an API key, which identifies *your back end*; it then trusts the user your back end names (`X-User-ID`), their roles (`X-User-Roles`), and, in a multi-tenant runtime, the agent (`X-Deployment-ID`). The manager checks nothing at all. So:

- Put a back end of your own between people and Forma. It authenticates them, and calls Forma for them.
- Never send the API key to a browser or an app, and never let a client choose the headers Forma trusts.
- Keep the manager on a private network.

Neither server sends CORS headers, so browsers cannot call them from another origin anyway: requests come from servers.

## The descriptions

Download them, or print them with the CLI. Each is a single OpenAPI 3.1 file, with every schema in it.

| API | Download | CLI |
| --- | --- | --- |
| Runtime, `client: sync` | [sync.json](./openapi/sync.json) | `forma openapi sync` |
| Runtime, `client: sse` | [sse.json](./openapi/sse.json) | `forma openapi sse` |
| Runtime, `client: ai-sdk-v5` | [ai-sdk-v5.json](./openapi/ai-sdk-v5.json) | `forma openapi ai-sdk-v5` |
| Runtime, `client: whatsapp` | [whatsapp.json](./openapi/whatsapp.json) | `forma openapi whatsapp` |
| Runtime, `client: telegram` | [telegram.json](./openapi/telegram.json) | `forma openapi telegram` |
| Manager | [manager.json](./openapi/manager.json) | `forma openapi manager` |

Browse them in the **[API explorer](./explorer.html)**, or write one to a file (`forma openapi sse -o forma-sse.json`) to generate a client. For instance, TypeScript types with [openapi-typescript](https://openapi-ts.dev/):

```sh
forma openapi sse -o forma-sse.json
npx openapi-typescript forma-sse.json -o src/forma-sse.d.ts
```

The `forma` CLI prints the descriptions of its own version: use the CLI of the version you run, as the APIs may change between versions.

### What OpenAPI cannot say

- **Streams.** OpenAPI 3.1 has no way to describe the items of a stream. A streamed answer is described as `text/event-stream`, with the schema of *one* `data:` line (`SseLine` for `sse`, `AiSdkV5Line` for `ai-sdk-v5`). How to read the stream is in [the runtime's API](./runtime.md#streams).
- **Agent definitions.** The manager takes and returns agents as they are written (`forma expand -f json`). Their schema is in the [reference documentation](../reference_docs/auto-agent.md), not in the manager's description, where they are plain objects.
- **What chat apps send.** The webhooks take WhatsApp's and Telegram's own payloads, which those platforms document.

## Where to go next

- [The runtime's API](./runtime.md): conversations, messages, streams, tool calls that wait for an answer, usage, and errors.
- [Running a multi-tenant platform](./multi-tenant.md): what the proxy in front of a multi-tenant runtime has to do.
- [The manager's API](./manager.md): deploying agents, users' tokens, and conversation history.
- [Usage and billing](./usage-and-billing.md): what each job reports it consumed, and how to price it.
- [HTTP API](../how-to/http-api.md): a hands-on walk through the runtime's API, with `curl`.
