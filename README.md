![Forma](website/public/logo.svg)

# Forma

An AI agent runtime: write the agent once, in YAML, and serve it over HTTP, to
web apps, on WhatsApp or on Telegram.

| Directory | What it is |
| --- | --- |
| `forma/` | The runtime, the CLI and the manager (Rust) |
| `docs/` | The documentation (mdBook) |
| `website/` | The website (Astro), published to GitHub Pages with the docs under `docs/` |

See `forma/README.md` to build it, and the
[documentation](https://germolinal.github.io/forma/docs/) to use it.

Releases are `v*` tags: a tag publishes the website and the documentation,
the CLI's binaries (GitHub Releases), and the container images
(`ghcr.io/germolinal/forma`, `forma-multi-tenant`, `forma-manager`,
`forma-cli` and `forma-sandbox`).

`deployments/gcp/` has Terraform for one agent on Google Cloud, using those
images.

## Building on Forma's APIs

Everything Forma serves is plain HTTP, described in OpenAPI 3.1. The
descriptions are generated from the code, and a test fails if the committed
ones fall behind it, so they are what the servers do. With them and the pages
below, you (or your coding agent) can build a front end, a back end that talks
to agents, or a whole platform that deploys them, bills for them and puts them
on WhatsApp and Telegram.

### What there is

| Server | Does | Called by |
| --- | --- | --- |
| **Runtime** (`forma serve`; images `forma`, `forma-multi-tenant`) | Serves an agent (or, multi-tenant, every deployed one): conversations, messages, tool calls that wait for an answer | Your back end |
| **Runtime as a webhook** (`client: whatsapp`, `telegram`) | Serves an agent on a chat app | WhatsApp, Telegram |
| **Manager** (`forma-manager`) | Deploys agents to multi-tenant runtimes, keeps users' OAuth tokens, lists and edits conversations | Your back end |

A runtime serves one client (`client:` in its `runtime.yaml`). The paths are
the same for every client (`/v1/init`, `/v1/chat`, `/v1/resume`); what they
take and return is not, so each has its own description.

### The descriptions

| API | In this repository | Online (from the next release) |
| --- | --- | --- |
| Runtime, `client: sse` (streamed) | [`docs/src/api/openapi/sse.json`](docs/src/api/openapi/sse.json) | [sse.json](https://germolinal.github.io/forma/docs/api/openapi/sse.json) |
| Runtime, `client: sync` (JSON) | [`docs/src/api/openapi/sync.json`](docs/src/api/openapi/sync.json) | [sync.json](https://germolinal.github.io/forma/docs/api/openapi/sync.json) |
| Runtime, `client: ai-sdk-v5` (Vercel's AI SDK) | [`docs/src/api/openapi/ai-sdk-v5.json`](docs/src/api/openapi/ai-sdk-v5.json) | [ai-sdk-v5.json](https://germolinal.github.io/forma/docs/api/openapi/ai-sdk-v5.json) |
| Runtime, `client: whatsapp` | [`docs/src/api/openapi/whatsapp.json`](docs/src/api/openapi/whatsapp.json) | [whatsapp.json](https://germolinal.github.io/forma/docs/api/openapi/whatsapp.json) |
| Runtime, `client: telegram` | [`docs/src/api/openapi/telegram.json`](docs/src/api/openapi/telegram.json) | [telegram.json](https://germolinal.github.io/forma/docs/api/openapi/telegram.json) |
| Manager | [`docs/src/api/openapi/manager.json`](docs/src/api/openapi/manager.json) | [manager.json](https://germolinal.github.io/forma/docs/api/openapi/manager.json) |

Raw files, for tools and agents that fetch:
`https://raw.githubusercontent.com/germolinal/forma/main/docs/src/api/openapi/<api>.json`.
Browse them in the [API explorer](https://germolinal.github.io/forma/docs/api/explorer.html),
or print the ones of the version you run with the CLI:

```sh
forma openapi sse              # sync, sse, ai-sdk-v5, whatsapp, telegram, manager
forma openapi manager -o forma-manager.json
```

### What to read, in order

The descriptions say what every field means; these pages say how the requests
fit together, and what OpenAPI cannot (how to read a stream, who is trusted
with what).

1. [`docs/src/api/index.md`](docs/src/api/index.md): what to call, and the
   trust model.
2. [`docs/src/api/runtime.md`](docs/src/api/runtime.md): the runtime's API.
   Headers, conversations, attachments, streams and their events, tool calls
   that wait for an approval or a client's result, usage, every error.
3. [`docs/src/api/multi-tenant.md`](docs/src/api/multi-tenant.md): what the
   proxy in front of a multi-tenant runtime must do. Trusted headers,
   webhooks, per-deployment credentials, the encryption key.
4. [`docs/src/api/manager.md`](docs/src/api/manager.md): deploying agents,
   users' OAuth tokens, conversation history.
5. [`docs/src/api/usage-and-billing.md`](docs/src/api/usage-and-billing.md):
   what each job consumed, and how to price it.
6. To write the agents themselves: the definition's reference
   ([`Agent`](https://germolinal.github.io/forma/docs/reference_docs/auto-agent.html),
   generated from the code by `make docs`), and the
   [guides](https://germolinal.github.io/forma/docs/).

### Five things to know first

- **Forma does not authenticate people.** The runtime checks an API key
  (`X-API-Key`), which identifies *your back end*, and believes the user,
  roles and deployment it is told (`X-User-ID`, `X-User-Roles`,
  `X-Deployment-ID`). The manager checks no key at all. Put your own back end
  in front, keep both off the internet, and never let a client set those
  headers.
- **Forma keeps the conversation.** Send only the user's new message; the
  agent reads the last `max_history_messages` of the conversation itself.
- **One message at a time.** A conversation busy answering refuses another
  message (`409`); with `sse`, `finish` means the turn is stored and the next
  can go.
- **Some tool calls wait.** A tool that needs an approval, or that your client
  runs (`delegated`), suspends the conversation; answer it with `/v1/resume`.
- **Every job reports its usage**, by provider, host and model, for billing.

## Licence

Forma is [MIT-licensed](LICENSE). The name "Forma" and its logo are not
covered by the licence: see [TRADEMARKS.md](TRADEMARKS.md).
