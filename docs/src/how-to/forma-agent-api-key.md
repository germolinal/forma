# Forma Agents' API Key

At present, Forma agents do not authenticate *users*. It is the calling service who should do that, get the ID, and then pass it along with the session id. However, Forma agents can require an API Key of your choice. You can opt out.

The steps to set an API key are the following:

## 1. Configure your Runtime.

The API key is not a property of the agent, but of the [runtime](../reference_docs/auto-formaruntime.md) (i.e., of how clients talk to your agent). Therefore, it is configured in the `runtime.yaml` file.

By default, the `sync`, `sse` and `ai-sdk-v5` clients will require an API Key. If you want to opt out, you need to enable the flag `no_api_key: true`

```yaml
client: sync
no_api_key: true # <-- THIS
storage: file
```

> **📌 Note**: The `whatsapp` and `telegram` runtimes never use this API Key, as their clients are those platforms' webhooks. (Their deliveries must prove where they come from instead: WhatsApp's carry Meta's signature, and Telegram's the webhook's secret token. Check [Your agent on WhatsApp](./whatsapp.md) and [Your agent on Telegram](./telegram.md).) Also, `forma dev` never asks for an API Key, because its only client is the chat UI it serves itself.

> **❗Important**: Production servers read `runtime.yaml` too, so `no_api_key: true` turns the check off in production as well. Only use it if something in front of the server already checks who is calling. Check the [Deployment](./deployment.md) section for more details.

## 2. Assign an API Key

If enabled, Forma will be expecting an environment variable called `FORMA_AGENT_KEY` to be available. This can be set in your `.env` file during development, and however you prefer in production. For example:

```sh
# Example of an API Key in an .env
FORMA_AGENT_KEY=key-not-for-production
```

## 3. Make requests

When making requests, make sure you add your API key using the `X-API-Key` header. For instance, using the `sync` runtime, you would first create a session:

```sh
curl http://localhost:8080/v1/init \
  -H "X-API-Key: key-not-for-production" \
  -H "X-User-ID: user-id"
```

```json
{"session_id":"5bb0dd24-bb70-418b-8dc2-e37ddeaade89"}
```

And then send messages to it:

```sh
curl -X POST -i http://localhost:8080/v1/chat \
  -H "X-API-Key: key-not-for-production" \
  -H "X-User-ID: user-id" \
  -H "X-Session-ID: 5bb0dd24-bb70-418b-8dc2-e37ddeaade89" \
  -H "Content-Type: application/json" \
  -d '{"role":"user","content":"hey"}'
```

```json
{"status":"completed","data":{"role":"assistant","content":[{"type":"text","text":"How's your day going? Is there anything I can help you with or would you like to chat about something in particular?"}]}}
```

A request without a key is rejected with a `401 Unauthorized` status, and one with the wrong key with a `403 Forbidden`. Similarly, a session ID that does not exist is rejected with a `404 Not Found`, and one that belongs to a different user with a `403 Forbidden`.
