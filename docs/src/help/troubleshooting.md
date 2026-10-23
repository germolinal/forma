# Troubleshooting

Common errors, what they mean, and how to fix them. Most of them were found while writing this documentation.

## Defining agents

### `Runtime Error: missing field 'name'` (or `'description'`)

Every agent needs an `id`, a `name` and a `description` at the top of its file:

```yaml
id: dory
name: Dory
description: A helpful assistant
start:
  nodes:
    - llm:
        provider: ollama
```

### `Runtime Error: start.nodes[0]: unknown field 'system_promt', expected one of ...`

A field is misspelled, or it does not exist (anymore). The message tells you where (`start.nodes[0]`) and which fields are valid there. For example, `persist_sessions` no longer exists: conversations are always stored.

> **📌 Note**: Fields in `runtime.yaml` are checked the same way. If a runtime setting seems to have no effect, remember that `forma dev` ignores the `client` in `runtime.yaml` (and its options), and that the file is not hot-reloaded: restart `forma serve` or `forma dev` after changing it.

### The agent does not seem to use my settings

Run `forma expand` to see what your agent actually became, with every `$file` reference read and every default filled in. Add `-c` to also build the tools and the LLM clients.

## Running agents

### `Runtime Error: 'error sending request for url (http://127.0.0.1:11434/v1/chat/completions)' caused by 'client error (Connect)'`

Forma cannot reach the LLM provider. With the default agent, this means Ollama is not running. Start it (`ollama serve`) and make sure the model is downloaded (`ollama pull llama3.1:8b`).

### The port is already in use

`forma dev` and `forma serve` listen on the port in the `PORT` variable of your `.env` file (`8080` by default). Change it if another program is using that port.

### The agent ignores my tools, or behaves strangely with them

Small models (like the default `llama3.1:8b`) are unreliable at calling tools: they may skip them, call them when they should not, or ignore parts of their descriptions. Check what the model received and decided in the [traces](../how-to/dory-traces.md), improve the tool's name and description, and consider a larger model. Read more in [What tool calling really is](../learn/tool-calling.md#models-do-not-always-follow-the-rules).

### A restricted tool is never used in `forma dev`

Tools with `just_for` are only offered to users with one of those roles. Write the role in the **Roles** field at the top of the chat ([details](../how-to/role-based-tools.md#impersonating-roles-during-development)). With `forma serve`, send it in the `X-User-Roles` header.

### A workflow answers with only one of its nodes

That is what `output` is for: only the nodes it lists send their text to the user. The other nodes still run (you will see their steps and tool calls), but their answers are only read by the nodes that use them in their templates. Add a node to `output` if the user should read it too ([details](../how-to/workflows.md#2-lets-try-it)).

## Talking to the HTTP API

| Error                                         | Cause and fix                                                                                         |
| --------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `401 No API Key found`                        | Add the `X-API-Key` header (not `Authorization: Bearer`)                                               |
| `403 Incorrect API Key`                       | The key does not match `FORMA_AGENT_KEY` in the server's environment (or `.env`, with the CLI)        |
| `400 ... missing field 'role'`               | Messages need a role: `{"role":"user","content":"..."}`                                               |
| `400 no x-session-id found in header`         | Create a session with `GET /v1/init` first, and send its ID in `X-Session-ID`                          |
| `404 Session not found`                       | The session does not exist (e.g., you deleted `forma-db`, or are talking to a different server)        |
| `403 Session not owned by user`               | The session was created with a different `X-User-ID`                                                  |
| `409 Session busy`                            | The previous message of this conversation is still being processed. Wait for it, then retry          |
| `400 tool call '...' is not waiting for an answer` | The call was already answered, or a new message rejected it ([details](../how-to/human-in-the-loop.md)) |
| `405 Method Not Allowed` on `/v1/init`        | `/v1/init` is a `GET`, not a `POST`                                                                    |

## Evaluations

### `Failed to parse JSON: unknown field 'output', expected one of 'id', 'input', 'expected_output'`

Reference answers go in an `expected_output` field. Older datasets used `output`; rename it.

### `Eval Error: 1 metric is below the score it requires`

A metric has a `required_score`, and its mean fell below it on at least one dataset. This is meant for CI/CD. While you are iterating, add `--no-fail` ([details](../how-to/unit-evals.md#evaluations-in-cicd-required-scores)).

### Scores that do not make sense

Read the judge's explanations in `REPORT.md`, not only the scores. Judges make mistakes, especially small ones. Check [Writing metrics and datasets you can trust](../how-to/metrics-and-datasets.md).

### Rate limits (`429`, `RESOURCE_EXHAUSTED`)

Cloud providers limit how many requests you can make per minute. Evaluations run 25 examples at a time by default: lower it with `-b` (e.g., `-b 5`), and check the `Runs` and `Warning` columns of the report before comparing results, as failed judgements reduce the number of examples measured.

## Production

### `Input Error: could not load agent in 'agent.yaml': No such file or directory`

The server looks for `agent.yaml` in the directory it runs from. Pass the right file with `-a` (e.g., `forma serve -a path/to/my-agent.json`).

### `Input Error: the agent in 'agent.yaml' has no nodes`

The agent file was produced with `forma expand` instead of `forma expand -c`. Production servers need the compiled agent ([details](../how-to/deployment.md#1-compile-your-agent-into-a-single-file)).

### `no x-deployment-id found in header`

The server was built with the `multi-tenant` feature, so it needs to know which agent should answer: add an `X-Deployment-ID` header with an ID returned by `forma-manager` ([details](../how-to/deployment.md#multi-tenant-deployment)).
