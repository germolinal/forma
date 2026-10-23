# Deployment

So far, we have been running Dory with the Forma CLI (`forma dev` and `forma serve`). The CLI is great for development, as it includes everything: hot reloading, a chat UI, evaluations, and more. However, this is not what you want in production.

In production, Forma agents are served by **lean binaries**, compiled from the [Forma source code](https://github.com/germolinal/forma). How a server works is decided in two places:

1. **When compiling** - Whether the server runs a single agent or many, and whether the agent can change without restarting the server. These change what the server *is*, so they are [Cargo features](https://doc.rust-lang.org/cargo/reference/features.html).
2. **In `runtime.yaml`** - How clients talk to your agent (e.g., a web app, WhatsApp, or another server), where conversations are kept, and what the agent's tools can reach. This is the same file the CLI reads, so what you tried with `forma serve` is what runs in production.

> **📌 Note**: The agent itself runs exactly the same in all of these. This is what lets you develop with the CLI and deploy with confidence.

## Building a production server

The CLI is built with the default features (which include everything), so production builds start with `--no-default-features`:

```sh
# From within the forma repository.
# A single-tenant server
cargo build --release --bin forma --no-default-features
```

The binary will be at `./target/release/forma`. Notice that it only knows one command:

```sh
./target/release/forma --help
```

```text
Forma

Usage: forma <COMMAND>

Commands:
  serve  Runs a production server
  help   Print this message or the help of the given subcommand(s)
```

Two features change what is built:

| Feature        | What it does                                                                                                        |
| -------------- | ------------------------------------------------------------------------------------------------------------------- |
| `multi-tenant` | Serves **many agents**, stored in the database and chosen per request, instead of one agent read from a file. More on both below |
| `hot-reload`   | Lets an agent be replaced while the server is running                                                              |

The CLI uses `hot-reload` to reload your agent every time you save a file. Production servers do not watch any files, so there is no reason to add it to a production build yet. If you need to update agents without restarting anything, use a multi-tenant server: agents are read from the database on every request, so a redeployed agent answers the very next message.

## The runtime file

Production servers read the same `runtime.yaml` as the CLI (pass another one with `-r`), whose [reference](../reference_docs/auto-formaruntime.md) lists every setting. For instance:

```yaml
client: sse
max_history_messages: 20
storage: mongo
```

### 1. The client

How clients talk to your agent:

| `client`                                                | Used for                                                       |
| ------------------------------------------------------- | -------------------------------------------------------------- |
| [`sync`](../reference_docs/auto-openaisyncadapter.md)   | Machine-to-machine: send a message, receive the response       |
| [`sse`](../reference_docs/auto-openaisseadapter.md)     | Like `sync`, but streams progress (e.g., tool calls) as Server Sent Events |
| [`ai-sdk-v5`](../reference_docs/auto-aisdkv5adapter.md) | Web front-ends built with Vercel's AI SDK v5                   |
| [`telegram`](../reference_docs/auto-telegramadapter.md) | Telegram bots ([guide](./telegram.md))                         |
| [`whatsapp`](../reference_docs/auto-whatsappadapter.md) | WhatsApp Cloud API webhooks ([guide](./whatsapp.md))           |

Each client has options of its own (e.g., `max_history_messages`), which are written next to it.

> **❗Important**: `sync`, `sse` and `ai-sdk-v5` require an [API Key](./forma-agent-api-key.md), given in the `FORMA_AGENT_KEY` environment variable, unless `no_api_key: true`. That option works in production too: only use it if something in front of the server already checks who is calling.

### 2. The storage

Where conversations are kept. It has no default: every runtime file says it.

| `storage`  | Conversations are kept in...                                       | Configure with                                       |
| ---------- | ------------------------------------------------------------------ | ---------------------------------------------------- |
| `mongo`    | A MongoDB-compatible database (in a database called `forma-db`)    | `SESSIONS_DB_URL`, the database's connection string  |
| `file`     | Files, within a `./forma-db` directory (the same as the CLI)       | Nothing                                              |
| `none`     | Nowhere. Every message starts a new conversation, with no memory   | Nothing                                              |

`mongo` is what you want for most deployments, as it lets you run several copies of your server sharing the same conversations (remember: Forma agents are stateless). `file` is fine for a single server with a persistent disk, and `none` is useful for single-turn agents (e.g., one that classifies or summarises text).

### 3. Workspaces and code execution

Only needed if your agent has workspace tools, or runs code. Where workspaces are kept, and where code runs, is the server's choice, the same for every agent on it:

```yaml
workspaces:
  store: gcs                 # or `local`, with a `dir`
code_execution:
  sandbox: cloud-run-job     # or `local-docker`, with an `image`
```

Without them, the server keeps no workspaces and runs no code, and the tools that need them say so when they are called. Check [`Workspaces`](../reference_docs/auto-workspaces.md) and [`Sandbox`](../reference_docs/auto-sandbox.md) for the details.

> **📌 Note**: Secrets, and the names of resources that change from one environment to the next, are not written in `runtime.yaml`: they are environment variables (e.g., `SESSIONS_DB_URL`, `WORKSPACES_BUCKET`, `CODE_EXECUTION_JOB`). Check the [list below](#environment-variables).

## Single-tenant deployment

A single-tenant server serves exactly one agent. It is the simplest option: the agent is part of your deployment, versioned with the rest of your code.

### 1. Compile your agent into a single file

Production servers do not resolve the `$/prompts/...` references we have been using, and they do not build tools. Therefore, they need your agent as a single, compiled file, which you produce with `forma expand -c`:

```sh
# Within your agent's project
forma expand -c > agent.yaml
```

> **❗Important**: Do not forget the `-c`. Without it, tools are not built, and your agent will not be able to use them.

### 2. Run it

```sh
PORT=8080 FORMA_AGENT_KEY=key-not-for-production ./target/release/forma serve -a agent.yaml -r runtime.yaml
```

Note that:

* Production servers do not read your `.env` file. Environment variables have to be set by whatever runs the server (check the [list below](#environment-variables)).
* `-a` is the agent's file (`agent` by default), and `-r` the runtime file (`runtime.yaml` by default). If either has no extension, `.yaml` is added: `-a agent` and `-a agent.yaml` are the same.
* Production servers listen on every network interface (`0.0.0.0`), while the CLI only listens to requests coming from your own computer (`127.0.0.1`).
* Telemetry is sent to an OpenTelemetry collector by default (`-t otel`). Add `-t console` to just print logs. (Check the [Observability](./observability.md) section.)

You can now talk to it exactly like you did with `forma serve` (check the [API Key](./forma-agent-api-key.md#3-make-requests) section).

### 3. Containers

The [Forma repository](https://github.com/germolinal/forma) includes a `Dockerfile.channel` that compiles the binary, with the features you choose (none for a single-tenant server), and copies it into a minimal [distroless](https://github.com/GoogleContainerTools/distroless) image (no shell, no package manager). It does not contain any agent: it is meant to be used as a base image. Its command, `forma serve -r /config/runtime.yaml`, reads a runtime file mounted at `/config`: for a multi-tenant server (`--build-arg FEATURES=multi-tenant`), which has no agent file, mounting that file is all it needs.

```sh
# From within the forma repository
docker build -f Dockerfile.channel -t forma .
```

Or start from the images each release publishes, for `linux/amd64` and `linux/arm64`: `ghcr.io/germolinal/forma` (single-tenant), `ghcr.io/germolinal/forma-multi-tenant` and `ghcr.io/germolinal/forma-manager`, tagged with the version (e.g., `0.9.18`, `0.9`) and `latest`.

Then, in your agent's project, create a `Dockerfile` that adds your compiled agent and its runtime file:

```dockerfile
FROM forma:latest

# Produced by `forma expand -c > agent.yaml`
COPY ./agent.yaml /app/agent.yaml
COPY ./runtime.yaml /app/runtime.yaml

CMD ["/app/forma", "serve", "-t", "just-otel", "-a", "/app/agent.yaml", "-r", "/app/runtime.yaml"]
```

And build and run it (to run it on a cloud service instead, check [Deploying to the cloud](./cloud-deployment.md)):

```sh
docker build -t dory .
docker run -p 8080:8080 \
  -e FORMA_AGENT_KEY=key-not-for-production \
  -e SESSIONS_DB_URL="mongodb://..." \
  dory
```

## Multi-tenant deployment

A multi-tenant deployment serves many agents, possibly belonging to different users, from a single service. This is useful when your users create their own agents (e.g., in a web application), as you do not need to build and deploy a new server for each agent.

It has two components, which share the same database:

1. **The runtime** - A `forma` binary built with the `multi-tenant` feature. Instead of reading an agent from a file, it reads it from the database on every request. Its `runtime.yaml` says `storage: mongo`: it has to read agents from somewhere (`file` works too, for a single server).
2. **The manager** - A separate binary, `forma-manager`, with an HTTP API to deploy, update and delete agents (among other things). It keeps them in MongoDB.

```sh
# From within the forma repository.
# The runtime
cargo build --release --bin forma --no-default-features --features multi-tenant
# The manager. It needs the 'api' feature
cargo build --release --bin forma-manager --no-default-features --features api
```

> **❗Important**: The manager does not check API keys. It is meant to be called by your own back-end, and must not be exposed to the internet.

### 1. Run both services

```sh
# The manager listens on port 3001 by default
SESSIONS_DB_URL="mongodb://..." PORT=3001 ./target/release/forma-manager

# The runtime, with `storage: mongo` in runtime.yaml
SESSIONS_DB_URL="mongodb://..." PORT=8080 FORMA_AGENT_KEY=key-not-for-production ./target/release/forma serve -r runtime.yaml
```

> **📌 Note**: If deployments include credentials, or users connect to MCP servers, both services also need the same `FORMA_ENCRYPTION_KEY` (generate one with `openssl rand -base64 32`).

### 2. Deploy an agent

Send the agent's definition to the manager. The definition is the *expanded* agent (`forma expand -f json`, without `-c`), because the manager compiles it itself, using the tokens of the user who deploys it:

```sh
# Within your agent's project
forma expand -f json > definition.json
python3 -c 'import json; print(json.dumps({"definition": json.load(open("definition.json"))}))' > deploy.json

curl -X POST http://localhost:3001/deployments \
  -H "X-User-ID: owner" \
  -H "Content-Type: application/json" \
  -d @deploy.json
```

```json
{"deployment_id":"74fe248c-81a8-467c-b8c7-41f27ab5cff0"}
```

### 3. Talk to it

Requests to the runtime are just like those to a single-tenant server, plus an `X-Deployment-ID` header, indicating which agent should answer:

```sh
curl http://localhost:8080/v1/init \
  -H "X-API-Key: key-not-for-production" \
  -H "X-User-ID: user-id" \
  -H "X-Deployment-ID: 74fe248c-81a8-467c-b8c7-41f27ab5cff0"
```

```json
{"session_id":"4f26396a-7454-493a-8517-d308f1cc652a"}
```

A request without this header is rejected (`no x-deployment-id found in header`).

### 4. Update or delete an agent

Send a new definition with `PUT` (and the `deployment_id` you got earlier) to update an agent. As agents are read from the database on every request, the next message will already be answered by the new version.

```sh
curl -X PUT http://localhost:3001/deployments \
  -H "X-User-ID: owner" \
  -H "Content-Type: application/json" \
  -d '{"deployment_id": "74fe248c-81a8-467c-b8c7-41f27ab5cff0", "definition": { ... }}'
```

And delete it with `DELETE`:

```sh
curl -X DELETE http://localhost:3001/deployments \
  -H "Content-Type: application/json" \
  -d '{"deployment_id": "74fe248c-81a8-467c-b8c7-41f27ab5cff0"}'
```

### The manager's API

| Method   | Path                      | Purpose                                                                         |
| -------- | ------------------------- | ------------------------------------------------------------------------------- |
| `POST`   | `/deployments`            | Compile and deploy an agent. Returns its `deployment_id`                        |
| `PUT`    | `/deployments`            | Replace a deployed agent with a new definition                                  |
| `DELETE` | `/deployments`            | Delete a deployed agent                                                         |
| `POST`   | `/agents/check`           | Check that a definition compiles, without deploying it                          |
| `POST`   | `/agents/generate`        | Generate an agent definition from a natural language description               |
| `POST`   | `/tokens`                 | Store a user's OAuth token for an MCP server                                    |
| `DELETE` | `/tokens`                 | Remove a user's OAuth token for an MCP server                                   |
| `GET`    | `/sessions`               | List a user's conversations with an agent (`?agent_id=...&n=20`)                |
| `GET`    | `/sessions/{session_id}`  | Get a conversation, with all its messages                                       |
| `PATCH`  | `/sessions/{session_id}`  | Rename a conversation                                                           |
| `DELETE` | `/sessions/{session_id}`  | Delete a conversation                                                           |

Every endpoint that acts on behalf of a user expects the `X-User-ID` header.

## Environment variables

| Variable                              | Used by                     | Purpose                                                                 |
| ------------------------------------- | --------------------------- | ----------------------------------------------------------------------- |
| `PORT`                                | Runtime, manager            | The port to listen on (default: `8080` for the runtime, `3001` for the manager) |
| `FORMA_AGENT_KEY`                     | Runtime                     | The [API Key](./forma-agent-api-key.md) clients must send               |
| `SESSIONS_DB_URL`                     | `storage: mongo`, manager   | The connection string of the MongoDB-compatible database                |
| `FORMA_ENCRYPTION_KEY`                | Runtime, manager, CLI       | Encrypts stored credentials and OAuth tokens                            |
| `FORMA_LOGS`                          | Everything                  | Log level: `trace`, `debug`, `info` (default), `warn` or `error`         |
| `OTEL_EXPORTER_OTLP_ENDPOINT`         | Everything                  | Where to send traces and logs ([Observability](./observability.md))     |
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | Everything                  | Where to send metrics                                                   |
| `K_SERVICE`                           | Multi-tenant runtime        | The service name reported in traces (set automatically by Cloud Run)    |
| `WORKSPACES_BUCKET`                   | `workspaces: gcs`           | The bucket of the workspaces that name none, and of code runs           |
| `CODE_EXECUTION_JOB`                  | `code_execution: cloud-run-job` | The Cloud Run Job that runs code (with `GOOGLE_CLOUD_LOCATION`, if it is just the job's name) |
| `TELEGRAM_BOT_TOKEN`, `TELEGRAM_SECRET_TOKEN` | `client: telegram`  | Check [Your agent on Telegram](./telegram.md#2-create-the-agent)        |
| `WHATSAPP_PHONE_NUMBER_ID`, `WHATSAPP_ACCESS_TOKEN`, `WHATSAPP_VERIFY_TOKEN`, `WHATSAPP_APP_SECRET` | `client: whatsapp` | Check [Your agent on WhatsApp](./whatsapp.md#3-give-forma-the-credentials) |

LLM providers and tools need their own variables (e.g., `OPENAI_API_KEY`), which are documented in the [Reference Documentation](../reference_docs/auto-genaiclient.md).

## Summary

| You want...                                              | Build with                                  | `runtime.yaml`    | Run                                         |
| -------------------------------------------------------- | ------------------------------------------- | ----------------- | ------------------------------------------- |
| To develop, with hot reloading and a chat UI             | The default features (the CLI)              | Any               | `forma dev`, `forma serve`                  |
| One agent, several copies sharing conversations          | No features                                 | `storage: mongo`  | `forma serve -a agent.yaml`                 |
| One agent, one server, no database                       | No features                                 | `storage: file`   | `forma serve -a agent.yaml`                 |
| A single-turn agent, with no memory                      | No features                                 | `storage: none`   | `forma serve -a agent.yaml`                 |
| Many agents, deployed and updated without restarting     | `multi-tenant`, and `api` for `forma-manager` | `storage: mongo`  | `forma serve` + `forma-manager`             |
