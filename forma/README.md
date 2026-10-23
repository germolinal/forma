![Forma Logo](./img/forma-logo.svg)

# Forma

**Build reliable, production-grade AI agents once... then deploy everywhere.**

Forma is an AI framework that separates what your agent *is* from where it *runs*. Define your agent in YAML or JSON, then deploy it across Telegram, WhatsApp, the Web, or any machine-to-machine channel. All from the same definition, with no code changes.

> Forma is currently focused on data pipelines and CX agents: answering questions and processing requests in a deterministic, auditable way. More autonomous use cases are on the roadmap.

This repo contains the code for both the **Forma CLI** (for development of AI Agents) and the **runtimes** (for serving the models in production).


## Installing the **Forma CLI**

Download it from the [latest release](https://github.com/germolinal/forma/releases/latest) (Linux and macOS):

```sh
# Pick your computer: aarch64-apple-darwin (Apple Silicon), x86_64-apple-darwin (Intel Mac),
# x86_64-unknown-linux-gnu or aarch64-unknown-linux-gnu
TARGET=aarch64-apple-darwin
curl -sL https://github.com/germolinal/forma/releases/latest/download/forma-$TARGET.tar.gz | tar -xz
sudo mv forma-$TARGET/forma /usr/local/bin/
forma --version
```

Or install it from this source code (you need Rust):

```sh
cargo install --path . --bin forma
```

Instructions on how to build the runtimes are available later in this document.


## Core concepts

### Agent vs. Runtime

Forma makes a hard separation between two things:

- **Agent** — A YAML file that fully describes your AI agent: its system prompts, tools, workflows, and evaluation suite.
- **Runtime** — A compiled binary that loads an agent and exposes it to a specific channel (Telegram, WhatsApp, Web, etc.).

The same agent definition runs across any runtime without modification.

### Stateless and scalable

Every runtime and agent is stateless. All sessions and message history are persisted in any MongoDB-compatible database. This means you can run as many runtime instances as you need — they all share the same consistent state.

### Workflows and Nodes

An agent executes a **Workflow** — a two-dimensional grid of **Nodes** that run sequentially (rows) or in parallel (columns within a row). Each Node:

1. **Triages** the input to decide whether tools are needed
2. **Executes** approved tools in parallel
3. **Summarises** the results into a final response

### Tools with real guarantees

Forma combines prompting with code-level enforcement. For example: if a tool is marked as requiring human approval before execution, that approval gate is **enforced in code** — not just prompted. Other guarantees include:

- Role-based access control on tools (`just_for`)
- Parallel tool execution within a Node
- Human-in-the-loop gates that suspend the session until approved

### Evaluation built in

Every agent, workflow, and node can have an evaluation suite attached. Evaluations use LLM-as-Judge metrics with template variable rendering. Run them from the CLI as part of your development loop: results are written to your project as JSON and as a Markdown report, or pushed to [Phoenix](https://phoenix.arize.com) for analytics.



## Why Forma

Building a production AI agent is mostly a **prompting** problem, not a software problem. Our design reflects that:

| Principle | What it means in practice |
|-----------|---------------------------|
| **Maximise prompting time** | The framework handles the plumbing — you focus on testing, tuning, and evaluating prompts |
| **Agents should be auditable** | YAML definitions are plain text; anyone on your team can read, diff, and review them |
| **Evaluate often** | The CLI has evaluation built in; changing a prompt without running evals should feel wrong |
| **Reduce iteration time** | Hot-reload in dev mode means no restarts when you tweak an agent |
| **Integrate with your infrastructure** | Stateless runtimes drop into any existing stack; MongoDB keeps state |
| **Monitor in production** | OpenTelemetry tracing is built in; stochastic systems need continuous observation |


## Documentation

Full documentation is available at [germolinal.github.io/forma/docs](https://germolinal.github.io/forma/docs/).

## Production servers

While the **Forma CLI** includes features that are useful for development (e.g., hot reloading, a chat UI, evaluations, and more), **production servers** are smaller builds, meant to serve a Forma agent in production. Every release publishes them as container images, for `linux/amd64` and `linux/arm64`:

| Image | What it is |
| --- | --- |
| `ghcr.io/germolinal/forma` | A single-tenant server: one agent, read from a file |
| `ghcr.io/germolinal/forma-multi-tenant` | A multi-tenant server: many agents, read from the database |
| `ghcr.io/germolinal/forma-manager` | Deploys agents to multi-tenant servers |
| `ghcr.io/germolinal/forma-cli` | The CLI, for compiling and evaluating agents in CI/CD |
| `ghcr.io/germolinal/forma-sandbox` | The sandbox where agents' code runs (the `code` tool) |

Everything else is decided in `runtime.yaml`, which production servers read just like `forma serve` does: how clients talk to the agent (`client`), where conversations are kept (`storage`), and where the agent's tools keep workspaces and run code.

```yaml
client: telegram   # or sync, sse, ai-sdk-v5, whatsapp
storage: mongo     # or file, none
```

| `client` | Channel | Description |
| --- | --- | --- |
| `sync` | Machine-to-machine | Send an OpenAI-compatible message, receive a response |
| `sse` | Machine-to-machine | Like `sync`, but streams Server-Sent Events for tool use and progress |
| `telegram` | Telegram | Chat with a Forma agent via Telegram |
| `whatsapp` | WhatsApp | Chat with a Forma agent via WhatsApp |
| `ai-sdk-v5` | Web | Connects to a frontend using Vercel AI SDK v5 |

`forma init` creates a `Dockerfile` that packages an agent on top of these images: the `forma-cli` image compiles the agent (`forma expand -c`), and the result is copied, with `runtime.yaml`, into the `forma` image.

```sh
docker build -t my-agent .
docker run -p 8080:8080 --env-file .env my-agent
```

To build the servers from source instead:

```sh
# A single-tenant server: one agent, read from a file
cargo build --release --bin forma --no-default-features
# A multi-tenant server: many agents, read from the database
cargo build --release --bin forma --no-default-features --features multi-tenant
# Then run it with a compiled agent (`forma expand -c > agent.yaml`)
./target/release/forma serve -a agent.yaml -r runtime.yaml
```

The [Deployment](https://germolinal.github.io/forma/docs/how-to/deployment.html) guide covers the rest: the images, multi-tenant servers and the manager, and every setting.

## Benchmarking

`make bench` measures what Forma itself costs: how long it adds to every
message, how many messages it answers per second, and how much CPU and memory
that takes. It runs the agent in `bench/agent` against a mock LLM, so a run
costs nothing and the LLM takes exactly as long as it is told to.

```sh
make bench                      # every target, default settings
BENCH_TARGETS=none make bench   # only the target that stores nothing
```

It needs Docker for the targets that store sessions: it starts a throwaway
MongoDB on port 27018 and removes it at the end, so your own database is never
touched. Each of those targets starts on an empty database filled with 50 000
messages of other conversations, as a database in use would be. On macOS it
also takes a CPU profile with `sample`, which comes with the system; on Linux
it uses `perf` when it is installed.

### What it does

1. Builds, in release mode, the mock LLM and the load generator (the
   `forma-bench` crate, in `bench/`), the CLI (to compile the agent) and one
   production runtime per target.
2. Starts the mock LLM, and then, for each target, starts Forma and sends it
   messages with `forma-load`, in three scenarios:
   - `1-latency`: one conversation at a time, with an LLM that answers at
     once, so what is left is Forma. Each message is also split into phases
     (see below).
   - `2-throughput`: 32 conversations at once, still with an instant LLM, for
     15 seconds: how many messages per second Forma can answer, and what each
     one costs in CPU. The CPU profile is taken here.
   - `3-realistic`: 64 conversations at once, with an LLM that takes 400 ms to
     the first token and then streams 40 tokens 20 ms apart, as a real one
     does.
3. Writes `bench/results/<date>-<commit>/report.md`, next to one JSON per
   scenario, Forma's logs and the CPU profile.

Every message gets the whole tool loop, as in production: the mock asks for
the agent's tool, the tool runs, and the mock answers with text. That is two
LLM calls per message. Each conversation is one session that keeps growing,
so its history fills `max_history_messages` as a long conversation does. Like
a real provider, the mock refuses a conversation in which a tool result does
not follow its call, so a history Forma got wrong shows up as errors.

Every target serves `client: sse`.

| Target     | Build            | `storage` | What it is                                                         |
| ---------- | ---------------- | --------- | ------------------------------------------------------------------ |
| `none`     | Single-tenant    | `none`    | Nothing is stored: Forma's own cost, without a database            |
| `mongo`    | Single-tenant    | `mongo`   | Sessions in MongoDB, one agent                                     |
| `mongo-mt` | `multi-tenant`   | `mongo`   | What production runs: the agent is read from the database too      |

### Settings

Set them as environment variables.

| Variable                       | Default                         | What it sets                                                       |
| ------------------------------ | ------------------------------- | ------------------------------------------------------------------ |
| `BENCH_TARGETS`                | `none mongo-mt`                 | The targets to run, from the table above                           |
| `BENCH_REQUESTS`               | `300`                           | Messages in `1-latency`                                            |
| `BENCH_DURATION_S`             | `15`                            | Seconds `2-throughput` and `3-realistic` last                      |
| `BENCH_CONCURRENCY`            | `32`                            | Conversations at once in `2-throughput`                            |
| `BENCH_REALISTIC_CONCURRENCY`  | `64`                            | Conversations at once in `3-realistic`                             |
| `BENCH_TELEMETRY`              | `console`                       | Forma's `--telemetry`. `otel` is what the production images use; it sends telemetry only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set |
| `BENCH_FORMA_LOGS`             | `warn`                          | Forma's `FORMA_LOGS`                                               |
| `BENCH_SESSIONS_DB_URL`        | (a throwaway container)         | A MongoDB of your own. The sessions a run opens are deleted after it, and nothing is seeded |
| `BENCH_DB_MESSAGES`            | `50000`                         | Messages of other conversations the throwaway database starts with |
| `BENCH_DB_INDEXES`             | `0`                             | `1` creates, in the throwaway database, the indexes recommended for the sessions database |
| `BENCH_MOCK_STRICT`            | `1`                             | `0` lets the mock accept conversations a real provider would refuse |
| `BENCH_BIN`                    | (build the working tree)        | A directory with `forma-<target>` and `forma-cli` binaries to measure instead, e.g. built from another branch |
| `BENCH_OUT`                    | `bench/results/<date>-<commit>` | Where the results go                                               |
| `MOCK_LLM_PORT`, `BENCH_FORMA_PORT`, `BENCH_MONGO_PORT` | `11500`, `18080`, `27018` | Ports                                         |

### Reading the report

There is one table per scenario, with a row per target:

- `req/s`: messages answered per second.
- `p50`, `p90`, `p99`: how long a whole answer took, from sending the message
  to the last byte of the stream.
- `first text`: how long until the first piece of the answer arrived.
- `CPU ms/req`: CPU time the Forma process used per message answered. It is
  the best number to compare between runs: it does not depend on how busy the
  machine is.
- `cores busy`, `RSS peak MB`, `threads peak`: the Forma process while the
  scenario ran.
- `errors`: messages that got no answer. `LLM refused` counts the requests
  the mock refused as malformed.

For the targets with MongoDB, a second table counts the round trips to the
database each message took, from the server's own counters (`serverStatus`).
Each is cheap here, next to the database; in production each one is a trip
across the network.

In `1-latency`, each message is lined up with the LLM calls Forma made for it
(the mock records when each one starts and ends), which splits it into what
happened before the first LLM call (reading the session and the agent,
storing the message), between calls (running the tool), after the last one
(storing the answer), and inside the LLM. `Forma overhead` is all of it but
the LLM. With an instant LLM, "inside the LLM" still includes Forma reading
the streamed answer.

Results move by about 10% from one run to the next, more with other programs
busy, so compare runs made on the same machine, and look at `CPU ms/req`
first. To compare two versions, build one, keep its binaries, and measure both:

```sh
make bench                                   # builds and measures the working tree
cp -R target/bench target/bench-before       # keep these binaries
git switch my-branch
make bench                                   # the branch
BENCH_BIN=target/bench-before make bench     # the first version again, same conditions
```

### The mock LLM on its own

The mock speaks OpenAI's chat completions API, streamed or not. Run it with
`make mock-llm` and point any agent at it:

```yaml
llm:
  provider: openai-v1
  endpoint: http://127.0.0.1:11500
  model: mock
```

(`openai-v1` sends `OPENAI_API_KEY`, so it must be set, to anything.) It calls
the first tool it is offered when the last message is the user's, and answers
with text otherwise. It refuses, with a 400, a conversation in which a tool
result does not follow its call, as OpenAI does; `--lenient` accepts anything.
Its behaviour can be changed while it runs:

```sh
curl -X POST localhost:11500/config -H 'content-type: application/json' \
  -d '{"ttft_ms": 400, "chunk_interval_ms": 20, "chunks": 40, "tool_calls": true, "strict": true}'
```

`GET /stats` counts what it served, `GET /timeline` lists every call with when
it started and ended, and `POST /reset` clears both.

It does not stand in for `vertex-ai`: Gemini is reached through Google's own
client, over gRPC, which the mock does not speak.

## Examples

Client examples for each runtime are available at [github.com/ai-forma/forma-examples](https://github.com/ai-forma/forma-examples).
