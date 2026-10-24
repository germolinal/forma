# The manager's API

`forma-manager` deploys agents to [multi-tenant runtimes](./multi-tenant.md), keeps users' OAuth tokens for MCP servers, and lists and edits users' conversations. It listens on `PORT` (default `3001`), and keeps everything in the MongoDB the runtimes read (`SESSIONS_DB_URL`). Starting it is in [Deployment](../how-to/deployment.md#multi-tenant-deployment).

Every field is in its OpenAPI description ([manager.json](./openapi/manager.json), `forma openapi manager`, or the [explorer](./explorer.html?api=manager)).

> **❗Important**: the manager authenticates nobody. It believes `X-User-ID`: it checks that a user owns what they change, but not that they are who the header says. Only your back end may reach it: keep it on a private network, and check that the user may do what they ask before calling it.

## At a glance

| Method | Path | Does |
| --- | --- | --- |
| `POST` | [`/tokens`](#tokens) | Keeps a user's OAuth token for an MCP server |
| `DELETE` | [`/tokens`](#tokens) | Forgets it |
| `POST` | [`/agents/check`](#agents) | Checks an agent, and reports what it uses |
| `POST` | [`/agents/generate`](#agents) | Writes an agent definition from a description |
| `POST` | [`/deployments`](#deployments) | Deploys an agent |
| `PUT` | [`/deployments`](#deployments) | Updates a deployed agent |
| `DELETE` | [`/deployments`](#deployments) | Deletes a deployment |
| `GET` | [`/sessions`](#sessions) | Lists a user's conversations with an agent |
| `GET` | [`/sessions/{session_id}`](#sessions) | Reads a conversation |
| `PATCH` | [`/sessions/{session_id}`](#sessions) | Renames it |
| `DELETE` | [`/sessions/{session_id}`](#sessions) | Deletes it, with its messages |

Successful answers are JSON (or empty); errors are a status, with a message in plain text. A body that is not valid JSON is a `400`; one that is JSON but not what the operation takes (a missing field, a wrong type) is a `422`; one without `Content-Type: application/json` is a `415`.

## The life of a deployment

```text
1. the user connects an MCP server ─► POST /tokens            (if the agent uses one)
2. your UI saves the agent ─────────► POST /agents/check      what it uses, and needs
3. the user deploys it ─────────────► POST /deployments       → deployment_id
4. people talk to it ───────────────► the runtime, with X-Deployment-ID: deployment_id
5. the author changes it ───────────► PUT /deployments        same id, same conversations
6. the author removes it ───────────► DELETE /deployments
```

Agents are sent as their *expanded* definition: an [Agent](../reference_docs/auto-agent.md) in JSON, with every `$file` inlined. Within an agent's project:

```sh
forma expand -f json > agent.json
```

Not compiled (no `-c`): the manager compiles it itself, with the tokens of the user deploying it.

## Tokens

An agent that uses an MCP server acts for a user, with their OAuth token for it. Your back end runs the OAuth flow (it has the user, and a browser to send them to), and gives the manager the token. It is stored encrypted (`FORMA_ENCRYPTION_KEY`), under the server's URL as agent definitions name it, and refreshed when it expires.

```sh
curl -X POST http://manager:3001/tokens -H "Content-Type: application/json" -d '{
  "user_id": "user-42",
  "mcp_url": "https://mcp.example.com/mcp",
  "token": {
    "client_id": "...", "client_secret": "...",
    "access_token": "...", "refresh_token": "...",
    "expires_at": "2026-10-24T12:00:00Z",
    "token_endpoint": "https://auth.example.com/oauth/token"
  }
}'
```

The token replaces the user's previous one for the same server; their tokens for other servers stay. `DELETE /tokens` (`{"user_id": ..., "mcp_url": ...}`) forgets one; forgetting a token the user does not have is not an error. Both answer `200` with no body.

## Agents

### `POST /agents/check`

Checks an agent without deploying it: the definition (ids, names), its compilation, and the compiled agent, as a runtime would check it before serving it. With `?mcp=connect` (the default), its MCP servers are connected to with the tokens of `X-User-ID`, as running it would; with `?mcp=unconnected`, they are not, so a user who has not connected them yet can still save the agent (the names of those servers' tools are then unknown, so whether they clash is not checked).

```sh
curl -X POST "http://manager:3001/agents/check?mcp=unconnected" \
  -H "X-User-ID: user-42" -H "Content-Type: application/json" -d @agent.json
```

It answers what the agent turned out to use:

```json
{
  "mcp_definitions": [{"url": "https://mcp.example.com/mcp", "auth": {"type": "oauth", "client_id": "MCP_CLIENT_ID", "scopes": []}}],
  "features": {
    "mcp_servers": ["https://mcp.example.com/mcp"],
    "scrapbook": false,
    "workspaces": [{"path": "my-bucket/handbook", "scopes": ["read"]}],
    "tool_kinds": ["mcp", "workspace"],
    "asks_first": true,
    "tools": [{"name": "search-handbook", "kind": "workspace", "bundle": "handbook"}],
    "models": [{"provider": "gcp.vertex_ai", "model": "gemini-3.5-flash"}]
  }
}
```

- `mcp_definitions`: the MCP servers the definition configures, with their authentication: what the user must connect before the agent can be deployed.
- `features.workspaces`: every workspace its tools reach, and how. Exactly what to grant a deployment of it (`credentials.workspaces`, below), and all it should be granted.
- `features.asks_first`: whether some tools wait for an approval: a client of it must handle [tool calls that wait](./runtime.md#tool-calls-that-wait-for-an-answer).

A definition that does not compile, or fails a check, is a `400`, whose message says why: show it to the agent's author.

### `POST /agents/generate`

Writes an agent definition from a description (`{"prompt": "An agent that ..."}`), with Gemini on Vertex AI (the manager then needs Google Cloud credentials). It checks and deploys nothing: check the result with `/agents/check` before deploying it.

## Deployments

### `POST /deployments`

```sh
curl -X POST http://manager:3001/deployments -H "X-User-ID: user-42" \
  -H "Content-Type: application/json" -d '{
  "definition": { "id": "support", "name": "Support", "start": { "nodes": [ ... ] } },
  "credentials": {
    "workspaces": [{"path": "my-bucket/handbook", "scopes": ["read"]}],
    "telegram_api": {"bot_token": "123:abc", "secret_token": "a-secret"}
  }
}'
```

```json
{"deployment_id": "6716a3c1f1a2b3c4d5e6f708"}
```

The agent is compiled with the tokens of `X-User-ID`, who owns the deployment, and stored. Runtimes serve it from now on, with no restart, to requests whose `X-Deployment-ID` is the `deployment_id`. Each call creates a new deployment, even for the same agent: update one with `PUT`. The definition must have an `id`.

`credentials` holds what the deployment needs to reach the world: the chat apps' secrets (`whatsapp_api`, `telegram_api`) and its workspace grants (`workspaces`). They are stored encrypted. Their keys are explained in [Running a multi-tenant platform](./multi-tenant.md#4-credentials).

A definition that does not compile is a `500` (`something went wrong when compiling the agent`): check agents with `/agents/check` first, which says why.

### `PUT /deployments`

```json
{"deployment_id": "6716a3c1f1a2b3c4d5e6f708", "definition": { ... }, "credentials": {"workspaces": [ ... ]}}
```

Replaces the deployment's agent with a new version, compiled with the tokens of `X-User-ID`, who must own it (otherwise `404`, as if it did not exist). Runtimes serve the new version from the next request on, in the same conversations. `credentials` replaces only the keys it names: send the `workspaces` the new version needs, and the chat apps' secrets stay.

### `DELETE /deployments`

```json
{"deployment_id": "6716a3c1f1a2b3c4d5e6f708"}
```

Deletes the deployment, which `X-User-ID` must own (otherwise `404`, as if it did not exist): runtimes answer `404` to requests for it from now on. Its conversations stay.

## Sessions

Conversations, as runtimes stored them. A conversation's `agent_id` is the deployment's id in a multi-tenant runtime, and the agent's `id` in a single-tenant one. A conversation belongs to the user who started it: another user's is a `404`, as if it did not exist.

### `GET /sessions?agent_id=...&n=20`

A user's (`X-User-ID`) conversations with an agent, the most recent first, at most `n` (20 by default), without their messages. Conversations nobody wrote in are left out (every `/v1/init` starts one).

```json
[{"id": "6716a3c1f1a2b3c4d5e6f710", "created_at": "2026-10-23T14:02:11Z", "display_name": "Refund for order A-77"}]
```

`display_name` is the name the user gave it, or else the start of its first message.

### `GET /sessions/{session_id}`

The whole conversation: every message (user, assistant, and tool results), the calls it waits on (`pending_tool_calls`), and what it keeps to resume them. To show a conversation again:

- Messages are in the order they were stored. An assistant message holds its text and files (`content`) and the calls it made (`tool_calls`, with `tool_types` and `tool_bundles` by call id, to show them as the stream did); each call's result is a `tool` message after it.
- Parts are those of [messages](./runtime.md#messages), plus `workspace_file`: a file attached from a workspace, by reference (`workspace_id`, `path`, `media_type`).
- `pending_tool_calls` lists the calls the conversation waits on (`approved: false` for those not answered yet): answer them with the runtime's [`/v1/resume`](./runtime.md#tool-calls-that-wait-for-an-answer).

### `PATCH /sessions/{session_id}`

`{"name": "Refunds"}` renames it (cut to 100 characters); an empty or missing `name` puts back the name derived from its first message. Answers `204`.

### `DELETE /sessions/{session_id}`

Deletes it, with all its messages. Answers `204`.
