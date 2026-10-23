# Glossary

Terms used throughout this documentation, both general (about LLMs and AI agents) and specific to Forma.

**Agent** — In general, a program that uses an LLM to decide what to do, often by calling tools. In Forma, the definition (in YAML or JSON) of an AI agent: its workflows, nodes, prompts, tools and evaluations. See [Components](../documentation/building-blocks.md).

**API key** — In Forma, the secret a client sends in the `X-API-Key` header, compared with the server's `FORMA_AGENT_KEY`. Not to be confused with the API keys Forma uses to talk to LLM providers. See [Forma Agents' API Key](../how-to/forma-agent-api-key.md).

**Bundle** — A label grouping tools that come from the same place (e.g., the tools of one MCP server). It does not change behaviour; it helps clients show the tools.

**Cached tokens** — Input tokens that the LLM provider did not need to process again, because the request started exactly like a previous one. Faster and usually cheaper. See [Tokens](../learn/tokens.md#cached-tokens).

**Compiled agent** — The output of `forma expand -c`: the agent with every file reference resolved, every default filled in, and every tool built. It is what production servers run.

**Context window** — The maximum number of tokens a model can read in a single call. See [Tokens](../learn/tokens.md#the-context-window).

**Contextual (short-term) memory** — The conversation so far, sent back to the model on every call so it can "remember" it. See [Memory](../documentation/memory.md).

**Dataset** — A set of examples (inputs and, optionally, expected outputs) used to evaluate an agent. See [Evaluations](../how-to/evaluations.md).

**Delegated tool** — A tool that the client runs, instead of Forma. The session is suspended until the client sends back the result. See [Human in the loop](../how-to/human-in-the-loop.md).

**Deployment** — In a multi-tenant server, an agent stored in the database, identified by a `deployment_id`. See [Deployment](../how-to/deployment.md#multi-tenant-deployment).

**Evaluation (eval)** — Running an agent (or a part of it) on a dataset and grading its answers with metrics. See [Evaluations](../documentation/evals.md).

**Expected output** — The reference answer of an example in a dataset, available to metrics as `{{expected_output}}`.

**Hallucination** — An LLM stating something false or made up as if it were true.

**Human in the loop** — A design in which a person must approve some of the agent's actions before they happen. In Forma, `confirm_before_calling: true`. See [Human in the loop](../how-to/human-in-the-loop.md).

**Job** — One run of the agent, triggered by one message (or one resume). It has its own `job_id`, which you will find in traces and logs.

**Judge** — The LLM that grades an agent's answers according to a metric. See [Writing metrics and datasets](../how-to/metrics-and-datasets.md).

**LLM (Large Language Model)** — A model trained to predict text. In Forma, also the configuration of the client that talks to an LLM provider (`provider`, `model`, `temperature`, etc.).

**MCP (Model Context Protocol)** — An open protocol for exposing tools to AI agents through servers. Forma can use the tools of MCP servers with the [`mcp`](../reference_docs/auto-mcptool.md) tool, and authenticate to them with `forma login`.

**Metric** — A rubric (a template) used by a judge to grade one property of an answer, producing a score, a label and an explanation.

**Multi-tenant** — A server that runs many agents (possibly from different users), stored in a database and chosen per request. See [Deployment](../how-to/deployment.md).

**Node** — The basic unit of work in Forma: an LLM with a system prompt and, optionally, tools. It triages, runs tools, and repeats until it can answer. See [Components](../documentation/building-blocks.md#2-nodes).

**Persona** — An LLM role-playing a kind of user, used by `forma tester` to generate evaluation questions.

**Prompt injection** — Text (from a user, a document, a web page, or a tool result) that tries to make an LLM ignore its instructions. See [What tool calling really is](../learn/tool-calling.md#tools-are-an-attack-surface).

**Provider** — The service that runs the LLM: Ollama (locally), OpenAI-compatible APIs, Groq, or Vertex AI. See the [Reference Documentation](../reference_docs/auto-genaiclient.md).

**Role** — A label given to a user (in the `X-User-Roles` header) that grants access to restricted tools. See [Role Based Tool Access](../how-to/role-based-tools.md).

**Runtime** — The part of Forma that connects an agent to a channel (`sync`, `sse`, `aisdk-5`, `telegram`, `whatsapp`). The same agent can run on any runtime. See the [Reference Documentation](../reference_docs/auto-formaruntime.md).

**Session** — A conversation, with its messages and its state (e.g., tool calls waiting for approval). Created with `/v1/init`, and identified by the `X-Session-ID` header.

**Single-tenant** — A server that runs exactly one agent, read from a file when it starts.

**Summarisation** — The last LLM call of a node that reached its `max_iterations` limit while still calling tools: it tells the user what was found and asks whether to go on.

**Suspended session** — A session waiting for a tool call to be approved, or for a delegated tool's result. A new message, instead of an answer, rejects whatever is waiting, and the conversation goes on.

**System prompt** — The instructions given to an LLM at the beginning of every call, describing who it is and how it should behave. See [How an LLM sees a conversation](../learn/conversations.md).

**Template** — A text with fields (`{{name}}`) that are replaced with values at run time, like the output of another node. See [Templates](../how-to/templates.md).

**Temperature** — An LLM setting that controls how random its output is. `0` makes it (almost) deterministic, which is what you want for judges.

**Token** — The unit in which LLMs read and write text: a few characters, on average. See [Tokens](../learn/tokens.md).

**Tool** — An action an agent can ask to perform (search the web, call an API, run code). The LLM only *requests* the call; Forma runs it. See [What tool calling really is](../learn/tool-calling.md).

**Trace** — The record of everything that happened while processing a request: every node, LLM call, and tool call, with their inputs, outputs and durations. See [Observability](../how-to/dory-traces.md).

**Triage** — The step in which a node's LLM reads the conversation and decides whether to answer or to call tools.

**Workflow** — A set of nodes that depend on each other through their templates. Independent nodes run in parallel. See [Workflows](../how-to/workflows.md).

**Workflow tool** — A workflow exposed as a tool. It runs in a fresh session, receiving only the message rendered from its `input_prompt`.

**Workspace** — A collection of documents (in a Google Cloud Storage bucket, or a local directory) that an agent can read and, optionally, write, through tools. A **scrapbook** is a private workspace that belongs to each user. See the [Reference Documentation](../reference_docs/auto-googleworkspace.md).
