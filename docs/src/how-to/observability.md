# Observability and Monitoring

Observability is built into every Forma container image.  
By default, agents ship with [OpenTelemetry](https://opentelemetry.io/) instrumentation, extended with [OpenInference](https://arize-ai.github.io/openinference) standards.  

This means:
- You can send traces, logs, and metrics to **any OTEL-compatible backend** (Grafana, Tempo, Datadog, etc.).
- You can analyze AI-specific traces with [Phoenix Arize](https://phoenix.arize.com).
- You can integrate Forma agents into your existing observability stack with no additional coding.



## What you will collect

- **Traces** – Each request and workflow execution is traced, including LLM calls and tool invocations.  
- **Metrics** – Counters and histograms for request counts, latencies, and error rates.  
- **Logs** – Structured logs at configurable levels (`trace`, `debug`, `info`).  

We intentionally **do not log our dependencies** (like LLM provider SDKs). Instead, we propagate their errors and report them in a clean, standardized way. That way, your logs reflect *your agent’s behavior*, not library internals.



## Configuration

### Where telemetry goes

Every `forma` command that runs your agent accepts a `-t` (or `--telemetry`) option, with three possible values:

| Value       | Logs are printed to the console | Traces, metrics and logs are sent to an OpenTelemetry collector |
| ----------- | ------------------------------- | --------------------------------------------------------------- |
| `console`   | ✅                               | ❌                                                               |
| `otel`      | ✅                               | ✅                                                               |
| `just-otel` | Only warnings and errors (production builds); nothing (the CLI) | ✅                                                  |

The CLI (`forma dev`, `forma serve`) defaults to `console`, so nothing is sent anywhere unless you ask for it (e.g., `forma dev -t otel`). Production servers, and the official container images, use `otel`: without `OTEL_EXPORTER_OTLP_ENDPOINT`, that only prints. (Check the [Deployment](./deployment.md) section.)

The rest is configured via environment variables.

### OTEL Collector

Point your agent to an OpenTelemetry Collector (local or remote):

```sh
# gRPC endpoint (default)
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4317"

# Optional: separate metrics endpoint
export OTEL_EXPORTER_OTLP_METRICS_ENDPOINT="http://localhost:4317/v1/metrics"
```

### Logs, traces and export batches

Three variables control how much telemetry Forma produces, and how it is sent:

| Variable                         | Default | What it controls                                                                 |
| -------------------------------- | ------- | -------------------------------------------------------------------------------- |
| `FORMA_LOGS`                     | `info`  | What is *logged*: to the console, and as OpenTelemetry log records               |
| `FORMA_TRACES`                   | `info`  | Which spans are exported as *traces*. Independent of `FORMA_LOGS`                 |
| `OTEL_BSP_MAX_EXPORT_BATCH_SIZE` | `64`    | How many spans are sent to the collector in each export (a standard OpenTelemetry variable) |

`FORMA_LOGS` and `FORMA_TRACES` take the same values:

* `trace` – Most detailed, includes every span and step.
* `debug` – Useful for development; shows workflow decisions and tool calls.
* `info` – Production-friendly; high-level events and errors only.
* `warn` – Warnings and errors.
* `error` – Errors only.
* `off` – Nothing.

They also accept per-module directives, as in [`EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html) (e.g., `info,agent=debug`).

Because the two are separate, a production server can keep a quiet log and still send every trace, or the other way around:

```sh
export FORMA_LOGS=warn     # few log lines
export FORMA_TRACES=info   # every trace (the default)

# Or: logs, but no traces at all
export FORMA_TRACES=off
```

#### Export batch size

Forma sends traces in batches. `OTEL_BSP_MAX_EXPORT_BATCH_SIZE` is not Forma's own setting: it is one of the [standard OpenTelemetry SDK variables](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/#batch-span-processor), which the SDK reads (along with `OTEL_BSP_MAX_QUEUE_SIZE`, `OTEL_BSP_SCHEDULE_DELAY` and `OTEL_BSP_EXPORT_TIMEOUT`). What is Forma's is the default: **64 spans** instead of the SDK's 512.

The reason is the size of an agent's spans. An LLM span carries the conversation it was given, often tens of KB, and 512 of them can exceed a collector's gRPC message limit (4 MiB by default, in the OpenTelemetry Collector and in most vendors). A collector refuses such a batch whole: every span in it is lost, and the spans waiting to be sent pile up in memory (up to 2,048 by default). Smaller batches stay under the limit.

Change it only if you need to:

* **Lower** (e.g., `16`) if your conversations are very long, or your collector's limit is smaller, and you still see exports refused.
* **Higher** if your collector accepts larger messages (e.g., `max_recv_msg_size_mib` in the OpenTelemetry Collector's OTLP receiver) and you want fewer, larger exports.

A refused export shows up in Forma's logs as a warning from `opentelemetry-otlp`, often with the gRPC code `ResourceExhausted`.

### Quick Start with Phoenix

If you want to analyze AI-specific traces:
* Run a local Phoenix Arize instance (Dockerized). The project created by `forma init` does this with `docker compose -f ./dev/docker-compose.yaml up`.
* Point OTEL_EXPORTER_OTLP_ENDPOINT to it (Phoenix listens on `http://localhost:4317`, which is also the default).
* Run your agent with `-t otel` (e.g., `forma dev -t otel`).
* Interact with your agent and watch traces appear in Phoenix, at [http://localhost:6006](http://localhost:6006).