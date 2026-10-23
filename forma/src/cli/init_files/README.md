# Forma Agent starter

This project was created with `forma init`. It contains a working agent called
Dory and a working evaluation for it. Modify both to suit your needs.

## What is here

```
src/agent.yaml               the agent: nodes, LLMs, tools, evaluations
src/prompts/agent_prompt.md  Dory's system prompt
evals/personas/wanderer.yaml a roleplaying LLM that invents test questions
evals/data/wanderer.json     the questions it produced
evals/metrics/politeness.md  the rubric a judge grades answers against
evals/results/               written by `forma eval` (git-ignored)
runtime.yaml                 the transport the agent is served over
Dockerfile                   packages the agent as a production server
agent-dev-guidelines.md      guidelines for developing agents
dev/docker-compose.yaml      optional Phoenix, for traces and eval results
.env                         ports, API keys, and other settings
```

Sessions are kept on disk in `./forma-db`, so the agent remembers a
conversation without any database to set up.

## Installing the `Forma CLI`

If you are reading this, you probably have it already: try `forma --version`.
Otherwise, download it from the
[latest release](https://github.com/germolinal/forma/releases/latest).

## Getting started

Dory and its judge both run on [Ollama](https://ollama.com) by default, so
nothing here needs an API key — but Ollama does need to be running:

```sh
ollama serve
ollama pull llama3.1:8b
```

You can change the AI Provider by changing the `llm.provider` and `llm.model` fields. You'll need to authenticate, either by using an API key (OpenAI, Groq) or by logging into Google Cloud using `gcloud auth application-default login` (vertex-ai).

### Talk to it

```sh
forma dev
```

This starts a development server with hot reloading and a chat UI in the
browser. Use `forma serve` instead to exercise the transport configured in
`runtime.yaml` — the one your agent will actually ship on. (`forma dev` always
speaks Server Sent Events, because that is what the bundled UI understands.)

### Check what you built

```sh
forma expand | head -40
```

This prints the agent with every `$file` reference inlined and every omitted
field filled in with its default. It is worth running after each edit to
`agent.yaml`: a misspelled or unknown key is rejected with an
`unknown field` error that says where it is. Add `-c` to compile the agent too,
which also checks the tools and the LLM settings.

### Evaluate it

An agent that demos well is not the same as an agent you can trust. The
evaluation loop is the difference, and this project ships with one already
wired up.

```sh
# 1. Ask the 'wanderer' persona for five questions a new visitor might ask.
#    This replaces evals/data/wanderer.json, so curate a copy if you edit it.
forma tester -f wanderer.yaml -n 5

# 2. Check Forma can see the dataset (it reads ./evals/data directly).
forma dataset-list

# 3. Ask Dory all five, and have a judge grade the answers.
forma eval -a -e "my first experiment"
```

Results are written to `./evals/results/` as they are produced:

- `<run>/REPORT.md` — every question, answer and judgement, worst first.
  Start here.
- `evals/results/README.md` — one row per run, most recent first. This is the
  table that tells you whether a change to your prompt helped.
- `<run>/agent-dory/experiment.json` — the same, unabridged.

Then change one thing in `src/prompts/agent_prompt.md`, run
`forma eval -a -e "my second experiment"`, and compare. Changing two things at
once makes the difference impossible to attribute.

`forma eval --list` shows everything that can carry evaluations — the agent,
its workflows, its nodes and their LLMs — so you can test one step of a large
agent rather than all of it.

### Traces and dashboards (optional)

[Arize Phoenix](https://arize.com/docs/phoenix) can collect traces, and can
hold datasets and evaluation results instead of the filesystem:

```sh
docker compose -f dev/docker-compose.yaml up   # UI at http://localhost:6006
forma eval -a --platform phoenix
```

### Ship it

The `Dockerfile` packages the agent as a production server: it compiles the
agent with the Forma CLI image, and copies the result into Forma's production
image.

```sh
docker build -t dory .
docker run -p 8080:8080 --env-file .env dory
```

Before deploying, switch to a hosted LLM provider (Ollama runs on your
computer) and to `storage: mongo` in `runtime.yaml`, and pin `FORMA_VERSION`
in the `Dockerfile` to the version you develop with. The
[Deployment](https://germolinal.github.io/forma/docs/how-to/deployment.html)
guide has the details, and
[Deploying to the cloud](https://germolinal.github.io/forma/docs/how-to/cloud-deployment.html)
puts it on Cloud Run, end to end.
