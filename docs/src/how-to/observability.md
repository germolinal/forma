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

## Log Levels

Control the verbosity of Forma logs with FORMA_LOGS variables:


```sh
export FORMA_LOGS=info # or debug | trace | warn | error | info (default)
```

* `trace` – Most detailed, includes every span and step.
* `debug` – Useful for development; shows workflow decisions and tool calls.
* `info` – Production-friendly; high-level events and errors only.
* `warn` - logs warnings and errors
* `error` - logs only errors

### Quick Start with Phoenix

If you want to analyze AI-specific traces:
* Run a local Phoenix Arize instance (Dockerized). The project created by `forma init` does this with `docker compose -f ./dev/docker-compose.yaml up`.
* Point OTEL_EXPORTER_OTLP_ENDPOINT to it (Phoenix listens on `http://localhost:4317`, which is also the default).
* Run your agent with `-t otel` (e.g., `forma dev -t otel`).
* Interact with your agent and watch traces appear in Phoenix, at [http://localhost:6006](http://localhost:6006).