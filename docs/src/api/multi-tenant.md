# Running a multi-tenant platform

A multi-tenant runtime (the `forma-multi-tenant` image, or a build with the `multi-tenant` feature) serves every agent the [manager](./manager.md) deployed, from one server: each request says which one it is for. It is how a platform offers agents to many users without running a server per agent. How to start one is in [Deployment](../how-to/deployment.md#multi-tenant-deployment); this page is the contract between it and what sits in front of it.

```text
  people ─► your app ─► ┌─────────────────────┐ ─ X-Deployment-ID ─► ┌──────────────────────┐
                        │ your proxy/back end │   X-User-ID, ...     │ forma-multi-tenant   │
 WhatsApp ─ webhook ──► │  authenticates,     │ ───────────────────► │ (one per client:     │
 Telegram ─ webhook ──► │  routes, bills      │                      │  sse, whatsapp, ...) │
                        └─────────┬───────────┘                      └──────────┬───────────┘
                                  │ deploys, lists conversations                │
                                  ▼                                             ▼
                        ┌─────────────────────┐        MongoDB        ┌──────────────────────┐
                        │   forma-manager     │ ────────────────────► │ deployments, sessions│
                        └─────────────────────┘                       │ tokens (encrypted)   │
                                                                      └──────────────────────┘
```

The runtime does not authenticate anybody. **Your proxy is the security boundary**: it is the only thing that may reach the runtime and the manager, and it must do everything below.

## 1. One runtime per client

A runtime serves one client (`client:` in its runtime file), so a platform with a web app and WhatsApp runs two: one with `client: sse` (or `sync`, or `ai-sdk-v5`), and one with `client: whatsapp`. They share the database, and so the deployments and the conversations. Every runtime needs `storage: mongo`; the manager reads and writes the same database.

## 2. The headers the runtime trusts

| Header | Set by your proxy to | Never |
| --- | --- | --- |
| `X-Deployment-ID` | The deployment the request is for, from your own records (which agent the user opened, which webhook URL was called) | Pass on what a client sent |
| `X-User-ID` | The authenticated user | Pass on what a client sent |
| `X-User-Roles` | The user's roles, for [tools restricted to roles](../how-to/role-based-tools.md) | Pass on what a client sent |
| `X-Time-Zone` | The user's time zone, if you know it (a client's is fine: it only changes what the date tool answers) | |
| `X-Session-ID` | The conversation. The runtime checks that it belongs to `X-User-ID` | |
| `X-API-Key` | The runtime's `FORMA_AGENT_KEY` (one key, for every deployment) | Send it to a client |

**Strip every `X-` header a client sent before adding yours.** Otherwise a user can talk to someone else's agent, or claim a role they do not have. Forma checks that a conversation belongs to the user it is told about, so a forged `X-User-ID` is a forged identity.

Does the user own the deployment they talk to, or may they use it? That is your decision, made before the request reaches Forma: the runtime serves any deployment it is asked for.

## 3. Webhooks

Chat apps call a URL per bot or number, and know nothing of deployments. So give each deployment a URL of its own on your proxy, and forward what arrives there:

1. **To the runtime of that client** (`client: whatsapp` or `telegram`), at `/v1/chat`, with the same method (WhatsApp verifies its webhook with a `GET`).
2. **With `X-Deployment-ID`**, the deployment the URL belongs to.
3. **With the body unchanged, and the platform's headers kept**: the runtime checks WhatsApp's signature (`X-Hub-Signature-256`, an HMAC of the exact body) and Telegram's secret token (`X-Telegram-Bot-Api-Secret-Token`) itself. Re-encoding the JSON breaks the signature.
4. **Answering the platform with what the runtime answered**: a `409` or `500` asks the platform to deliver again, which is what is wanted (a message is answered once, however often it arrives).

Each deployment's chat-app secrets are its own, given to the manager when deploying it (`credentials`, below). Registering the webhook with the platform is your job too:

- **Telegram**: call `setWebhook` with the deployment's URL and its `secret_token`, when deploying it: `https://api.telegram.org/bot{bot_token}/setWebhook?url={its URL}&secret_token={secret_token}`.
- **WhatsApp**: set the callback URL and verify token in Meta's App Dashboard (or through Meta's API). Meta then calls the URL with a `GET`, which the runtime answers if the verify token is the deployment's.

## 4. Credentials

What a deployment needs to reach the outside world travels with it: `credentials` in [`POST /deployments`](./manager.md#deployments), a map stored encrypted.

| Key | For | Holds |
| --- | --- | --- |
| `whatsapp_api` | `client: whatsapp` | `phone_number_id`, `access_token`, `verify_token`, `app_secret` |
| `telegram_api` | `client: telegram` | `bot_token`, `secret_token` |
| `workspaces` | Tools that read or write [workspaces](../reference_docs/auto-workspaces.md) | A list of `{"path": "<bucket>/<workspace id>", "scopes": ["read", "write"]}`: exactly what the agent may reach. [`POST /agents/check`](./manager.md#agents) reports what it needs (`features.workspaces`) |

A redeploy replaces only the keys it names, so a channel's secrets survive a new version of the agent.

Credentials and users' OAuth tokens are encrypted with `FORMA_ENCRYPTION_KEY` (32 random bytes, base64: `openssl rand -base64 32`). **The manager and every runtime need the same key**: the manager encrypts, the runtimes decrypt. Losing it loses every stored credential and token.

## 5. MCP servers and users' tokens

Agents that use MCP servers act for a user, with that user's OAuth tokens. Your back end runs the OAuth flow with the MCP server (it knows the user, and has a browser to send them to), and gives the tokens to the manager ([`POST /tokens`](./manager.md#tokens)), under the server's URL. The manager compiles agents with the deploying user's tokens (to discover the servers' tools), and the runtime calls them with the tokens of the user who writes, refreshing them when they expire.

## 6. Usage and billing

The runtime reports what each job consumed, by provider and model, and by tool (see [Usage](./runtime.md#usage)): the last line of every stream, and the answer to every webhook. A proxy that bills reads it there, and attributes it to the deployment and the user it sent the request for. JSON answers (`sync`, and `ai-sdk-v5`'s `/v1/resume`) carry it as `usage`.

## Checklist

- [ ] The runtime and the manager are reachable only from your proxy.
- [ ] Every `X-` header from clients is stripped; `X-Deployment-ID`, `X-User-ID` and `X-User-Roles` come from your own authentication.
- [ ] Your proxy checks that the user may use the deployment before forwarding.
- [ ] The API key never leaves your servers.
- [ ] Webhooks are forwarded byte for byte, with the platform's headers.
- [ ] `FORMA_ENCRYPTION_KEY` is the same everywhere, and backed up.
- [ ] Proxy timeouts are longer than your slowest agent's answer, and streams are not buffered.
