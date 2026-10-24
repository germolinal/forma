# Deployment

So far, we have been running Dory with the Forma CLI (`forma dev` and `forma serve`). The CLI is great for development, as it includes everything: hot reloading, a chat UI, evaluations, and more. However, this is not what you want in production.

In production, Forma agents are served by **lean servers**: builds of Forma without the development commands, published as **container images** with every release. How a server works is decided in two places:

1. **Which image you run** - Whether the server runs a single agent (`forma`) or many (`forma-multi-tenant`). This changes what the server *is*, so it is decided when Forma is compiled.
2. **In `runtime.yaml`** - How clients talk to your agent (e.g., a web app, WhatsApp, or another server), where conversations are kept, and what the agent's tools can reach. This is the same file the CLI reads, so what you tried with `forma serve` is what runs in production.

> **📌 Note**: The agent itself runs exactly the same in all of these. This is what lets you develop with the CLI and deploy with confidence.

## The images

Every release publishes five images to the GitHub Container Registry, for `linux/amd64` and `linux/arm64`:

| Image                                   | What it is                                                                                     |
| --------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `ghcr.io/germolinal/forma`              | A single-tenant server: **one agent**, read from a file. The base for your agent's image        |
| `ghcr.io/germolinal/forma-multi-tenant` | A multi-tenant server: **many agents**, read from the database on every request                 |
| `ghcr.io/germolinal/forma-manager`      | The manager, which deploys agents to multi-tenant servers                                       |
| `ghcr.io/germolinal/forma-cli`          | The CLI, for pipelines: compiling agents (`forma expand -c`) and evaluating them (`forma eval`) |
| `ghcr.io/germolinal/forma-sandbox`      | The sandbox where agents' code runs (the [`code`](../reference_docs/auto-codeexecution.md) tool): Python and shell, run by a Cloud Run Job (`code_execution: sandbox: cloud-run-job`) or by Docker (`local-docker`) |

Each is tagged with its version (e.g., `0.9.18`), its minor version (`0.9`, which moves with patch releases) and `latest`. **Pin a version in production** (or, better, a digest: `ghcr.io/germolinal/forma@sha256:...`), so that your image doesn't change because Forma released a new one. Use the same version for the CLI that compiles your agent and the server that runs it: `forma --version` tells you which one you develop with.

The three server images are built on [distroless](https://github.com/GoogleContainerTools/distroless): they contain the server and its runtime libraries, and nothing else (no shell, no package manager). `forma-cli` is built on Debian, so pipelines can run commands in it. What the server images expect:

| | `forma`, `forma-multi-tenant` | `forma-manager` |
| --- | --- | --- |
| The binary | `/app/forma` | `/app/forma-manager` |
| Default command | `/app/forma serve -t otel -r /config/runtime.yaml` | `/app/forma-manager` |
| The runtime file | Mounted (or copied) at `/config/runtime.yaml` | — |
| The agent | `forma`: a compiled agent, passed with `-a`. `forma-multi-tenant`: none, they are in the database | — |
| Port | `PORT`, or `8080` | `PORT`, or `3001` |

> **📌 Note**: `-t otel` prints logs to the container's output, and also sends them, with traces and metrics, to the OpenTelemetry collector at `OTEL_EXPORTER_OTLP_ENDPOINT`, if there is one (check [Observability](./observability.md)). Without a collector, it just prints.

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

A single-tenant server serves exactly one agent. It is the simplest option: the agent is part of your deployment, versioned with the rest of your code. You ship it as **your own image**: Forma's `forma` image, plus your compiled agent and its `runtime.yaml`.

### 1. Production servers need a compiled agent

Production servers do not resolve the `$/prompts/...` references we have been using, and they do not build tools. They need your agent as a single, compiled file, which `forma expand -c` produces:

```sh
# Within your agent's project
forma expand -c > agent.yaml
```

> **❗Important**: Do not forget the `-c`. Without it, tools are not built, and the server refuses to start (`the agent in 'agent.yaml' has no nodes`).

You rarely need to run this yourself: the `Dockerfile` below does it for you.

### 2. The `Dockerfile`

`forma init` creates a `Dockerfile` in your project. It builds your image in two steps: the `forma-cli` image compiles your agent, and only the result is copied into the `forma` image, which has no CLI and no shell.

```dockerfile
ARG FORMA_VERSION=latest
ARG FORMA_REGISTRY=ghcr.io/germolinal

# 1. Compile the agent
FROM ${FORMA_REGISTRY}/forma-cli:${FORMA_VERSION} AS compile
WORKDIR /work
COPY src ./src
COPY evals ./evals
RUN forma expand -c > agent.yaml

# 2. The production server, with the compiled agent and its runtime file
FROM ${FORMA_REGISTRY}/forma:${FORMA_VERSION}
COPY --from=compile /work/agent.yaml /app/agent.yaml
COPY runtime.yaml /config/runtime.yaml
CMD ["/app/forma", "serve", "-t", "otel", "-a", "/app/agent.yaml", "-r", "/config/runtime.yaml"]
```

(The file in your project has comments explaining each line. If your project was created before it existed, copy it from here.)

Before building it for production:

* **Pin `FORMA_VERSION`** to the version you develop with (e.g., `ARG FORMA_VERSION=0.9.18`).
* **Choose `storage: mongo`** in `runtime.yaml` (see [The storage](#2-the-storage)). With `storage: file`, conversations are kept inside the container, and lost whenever it is replaced.
* **Use a hosted LLM provider.** Ollama runs on your computer, not in the container.
* **MCP tools that need `forma login`** can't be compiled inside the build, because the tokens stay on your computer. Compile the agent yourself (`forma expand -c > agent.yaml`), and replace the first step with a `COPY agent.yaml /app/agent.yaml` in the second.

### 3. Build and run it

```sh
docker build -t dory .
docker run -p 8080:8080 \
  -e FORMA_AGENT_KEY=key-not-for-production \
  -e SESSIONS_DB_URL="mongodb://..." \
  dory
```

Note that:

* Production servers do not read your `.env` file. Environment variables have to be set by whatever runs the container (check the [list below](#environment-variables)). Locally, `docker run --env-file .env` passes them all at once.
* Secrets are never part of the image. Only the agent and the runtime file are.
* To change the runtime file without rebuilding, mount another one over it: `-v ./runtime.yaml:/config/runtime.yaml`.
* Production servers listen on every network interface (`0.0.0.0`), while the CLI only listens to requests coming from your own computer (`127.0.0.1`).
* `-t otel` prints logs, and sends them with traces and metrics to an OpenTelemetry collector when `OTEL_EXPORTER_OTLP_ENDPOINT` is set (check the [Observability](./observability.md) section).

You can now talk to it exactly like you did with `forma serve` (check the [API Key](./forma-agent-api-key.md#3-make-requests) section). To run it on Google Cloud, check [Deploying to Google Cloud](./cloud-deployment.md).

### Without building an image

You can also skip your own image, and give the published one your files when it starts: mount the compiled agent and the runtime file, and point the server at them.

```sh
forma expand -c > agent.yaml
docker run -p 8080:8080 --env-file .env \
  -v "$PWD/agent.yaml:/agent/agent.yaml:ro" \
  -v "$PWD/runtime.yaml:/agent/runtime.yaml:ro" \
  ghcr.io/germolinal/forma:0.9.18 \
  /app/forma serve -t otel -a /agent/agent.yaml -r /agent/runtime.yaml
```

Updating the agent is then replacing a file and restarting the container, with no build. It is how [Deploying to Google Cloud](./cloud-deployment.md) works, with the files in a bucket mounted into the service. Building your own image is better when something else should not be able to change the agent (an image is immutable, and can be pinned by digest), or when your platform cannot mount files.

### Compiling in a pipeline

The same two steps work in any CI/CD system that can run containers. For instance, to check that the agent compiles on every pull request, without building an image:

```sh
docker run --rm -v "$PWD":/work -w /work ghcr.io/germolinal/forma-cli:0.9.18 forma expand -c > /dev/null
```

`forma eval` runs in the same image (it needs the LLM providers' credentials).

### Without containers

Production servers are not published as plain binaries (only the CLI is). To run one outside a container, build it from the [Forma repository](https://github.com/germolinal/forma) (check [Building Forma from source](./building-forma.md)):

```sh
# From within the forma repository
cargo build --release --bin forma --no-default-features
```

And run it with your compiled agent:

```sh
PORT=8080 FORMA_AGENT_KEY=key-not-for-production ./target/release/forma serve -a agent.yaml -r runtime.yaml
```

`-a` is the agent's file (`agent` by default), and `-r` the runtime file (`runtime.yaml` by default). If either has no extension, `.yaml` is added: `-a agent` and `-a agent.yaml` are the same.

## Multi-tenant deployment

A multi-tenant deployment serves many agents, possibly belonging to different users, from a single service. This is useful when your users create their own agents (e.g., in a web application), as you do not need to build and deploy a new server for each agent.

It has two components, which share the same database:

1. **The runtime** - The `forma-multi-tenant` image. Instead of reading an agent from a file, it reads it from the database on every request. Its `runtime.yaml` says `storage: mongo`: it has to read agents from somewhere (`file` works too, for a single server).
2. **The manager** - The `forma-manager` image, with an HTTP API to deploy, update and delete agents (among other things). It keeps them in MongoDB.

Neither needs an image of your own: no agent is built into them.

> **❗Important**: The manager does not check API keys. It is meant to be called by your own back-end, and must not be exposed to the internet.

### 1. Run both services

```sh
export SESSIONS_DB_URL="mongodb://..."

# The manager listens on port 3001 by default
docker run -d -p 3001:3001 -e SESSIONS_DB_URL ghcr.io/germolinal/forma-manager:0.9.18

# The runtime, with `storage: mongo` in runtime.yaml, mounted where the image expects it
docker run -d -p 8080:8080 \
  -e SESSIONS_DB_URL -e FORMA_AGENT_KEY=key-not-for-production \
  -v "$PWD/runtime.yaml:/config/runtime.yaml:ro" \
  ghcr.io/germolinal/forma-multi-tenant:0.9.18
```

On a cloud service, the runtime file is mounted the same way, e.g., from a secret: Cloud Run mounts files only from Secret Manager.

To build them from source instead, use the `multi-tenant` feature for the runtime and `api` for the manager (check [Building Forma from source](./building-forma.md)):

```sh
cargo build --release --bin forma --no-default-features --features multi-tenant
cargo build --release --bin forma-manager --no-default-features --features api
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
{"deployment_id":"6716a3c1f1a2b3c4d5e6f708"}
```

### 3. Talk to it

Requests to the runtime are just like those to a single-tenant server, plus an `X-Deployment-ID` header, indicating which agent should answer:

```sh
curl http://localhost:8080/v1/init \
  -H "X-API-Key: key-not-for-production" \
  -H "X-User-ID: user-id" \
  -H "X-Deployment-ID: 6716a3c1f1a2b3c4d5e6f708"
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
  -d '{"deployment_id": "6716a3c1f1a2b3c4d5e6f708", "definition": { ... }}'
```

And delete it with `DELETE`:

```sh
curl -X DELETE http://localhost:3001/deployments \
  -H "X-User-ID: owner" \
  -H "Content-Type: application/json" \
  -d '{"deployment_id": "6716a3c1f1a2b3c4d5e6f708"}'
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

Every endpoint that acts on behalf of a user expects the `X-User-ID` header. Every request and answer is explained in [The manager's API](../api/manager.md), and described in OpenAPI (`forma openapi manager`).

## Environment variables

| Variable                              | Used by                     | Purpose                                                                 |
| ------------------------------------- | --------------------------- | ----------------------------------------------------------------------- |
| `PORT`                                | Runtime, manager            | The port to listen on (default: `8080` for the runtime, `3001` for the manager) |
| `FORMA_AGENT_KEY`                     | Runtime                     | The [API Key](./forma-agent-api-key.md) clients must send               |
| `SESSIONS_DB_URL`                     | `storage: mongo`, manager   | The connection string of the MongoDB-compatible database                |
| `FORMA_ENCRYPTION_KEY`                | Runtime, manager, CLI       | Encrypts stored credentials and OAuth tokens                            |
| `FORMA_LOGS`                          | Everything                  | Log level: `trace`, `debug`, `info` (default), `warn` or `error`         |
| `FORMA_TRACES`                        | Everything                  | Which spans are exported as traces, with the same syntax (default: `info`). Independent of `FORMA_LOGS`: a quiet log still exports every trace |
| `OTEL_EXPORTER_OTLP_ENDPOINT`         | Everything                  | Where to send traces and logs ([Observability](./observability.md))     |
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | Everything                  | Where to send metrics                                                   |
| `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT` | Everything       | `true` records conversations in traces (off by default): check [Observability](./observability.md#conversations-in-traces) |
| `OTEL_BSP_MAX_EXPORT_BATCH_SIZE`      | Everything                  | Spans per export to the collector (default: `64`). A standard OpenTelemetry variable, with a smaller default in Forma: check [Observability](./observability.md#export-batch-size) |
| `K_SERVICE`                           | Multi-tenant runtime        | The service name reported in traces (set automatically by Cloud Run)    |
| `WORKSPACES_BUCKET`                   | `workspaces: gcs`           | The bucket of the workspaces that name none, and of code runs           |
| `CODE_EXECUTION_JOB`                  | `code_execution: cloud-run-job` | The Cloud Run Job that runs code (with `GOOGLE_CLOUD_LOCATION`, if it is just the job's name) |
| `TELEGRAM_BOT_TOKEN`, `TELEGRAM_SECRET_TOKEN` | `client: telegram`  | Check [Your agent on Telegram](./telegram.md#2-create-the-agent)        |
| `WHATSAPP_PHONE_NUMBER_ID`, `WHATSAPP_ACCESS_TOKEN`, `WHATSAPP_VERIFY_TOKEN`, `WHATSAPP_APP_SECRET` | `client: whatsapp` | Check [Your agent on WhatsApp](./whatsapp.md#3-give-forma-the-credentials) |

LLM providers and tools need their own variables (e.g., `OPENAI_API_KEY`), which are documented in the [Reference Documentation](../reference_docs/auto-genaiclient.md).

## Summary

| You want...                                              | Run                                                      | `runtime.yaml`    |
| -------------------------------------------------------- | -------------------------------------------------------- | ----------------- |
| To develop, with hot reloading and a chat UI             | The CLI: `forma dev`, `forma serve`                      | Any               |
| One agent, several copies sharing conversations          | Your image, built on `forma` (the project's `Dockerfile`) | `storage: mongo`  |
| One agent, one server with a persistent disk, no database | Your image, built on `forma`                            | `storage: file`   |
| A single-turn agent, with no memory                      | Your image, built on `forma`                             | `storage: none`   |
| Many agents, deployed and updated without restarting     | `forma-multi-tenant` + `forma-manager`                   | `storage: mongo`  |
| To compile or evaluate agents in CI/CD                   | `forma-cli`                                              | —                 |

## Features, for builds from source

The images above cover every build Forma ships. If you compile Forma yourself, production builds start with `--no-default-features` (the CLI is built with the default features, which include everything), and two features change what is built:

| Feature        | What it does                                                                                                        |
| -------------- | ------------------------------------------------------------------------------------------------------------------- |
| `multi-tenant` | Serves **many agents**, stored in the database and chosen per request, instead of one agent read from a file (the `forma-multi-tenant` image) |
| `hot-reload`   | Lets an agent be replaced while the server is running                                                              |

The CLI uses `hot-reload` to reload your agent every time you save a file. Production servers do not watch any files, so there is no reason to add it to a production build yet. If you need to update agents without restarting anything, use a multi-tenant server: agents are read from the database on every request, so a redeployed agent answers the very next message.
