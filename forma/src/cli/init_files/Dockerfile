# Packages this agent as a production server, in two steps: the Forma CLI
# compiles the agent (`forma expand -c`), and the result is copied into
# Forma's production image, which has no shell and no CLI.
#
#   docker build -t my-agent .
#   docker run -p 8080:8080 --env-file .env my-agent
#
# Secrets (API keys, tokens, SESSIONS_DB_URL) are never part of the image: pass
# them as environment variables when the container starts.

# The Forma version that compiles and serves the agent. Pin it to the one you
# develop with (`forma --version`), so the image does not change under you.
ARG FORMA_VERSION=latest
# Where Forma's images come from. Change it to use your own builds.
ARG FORMA_REGISTRY=ghcr.io/germolinal

# 1. Compile the agent: resolve the `$` references, build the tools, and run
#    the same checks `forma serve` runs before serving it.
FROM ${FORMA_REGISTRY}/forma-cli:${FORMA_VERSION} AS compile
WORKDIR /work
# `src/agent.yaml` points into `evals/` (the metrics), so both are needed.
COPY src ./src
COPY evals ./evals
# Agents with MCP tools that need `forma login` cannot be compiled here (the
# tokens stay on your computer): compile them with `forma expand -c >
# agent.yaml` instead, and replace this step with `COPY agent.yaml .`.
RUN forma expand -c > agent.yaml

# 2. The production server, with the compiled agent and its runtime file.
FROM ${FORMA_REGISTRY}/forma:${FORMA_VERSION}
COPY --from=compile /work/agent.yaml /app/agent.yaml
# Choose `storage: mongo` (with SESSIONS_DB_URL) in runtime.yaml before
# deploying: with `file`, conversations are lost whenever the container is
# replaced. To change the runtime without rebuilding, mount another file here.
COPY runtime.yaml /config/runtime.yaml
# `-t otel` prints logs, which most container services collect, and also sends
# them, with traces and metrics, to the OpenTelemetry collector at
# OTEL_EXPORTER_OTLP_ENDPOINT, if there is one.
CMD ["/app/forma", "serve", "-t", "otel", "-a", "/app/agent.yaml", "-r", "/config/runtime.yaml"]
