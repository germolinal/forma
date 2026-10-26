# Deploy one agent on one machine

A personal Forma agent does not need a database or a bucket when it runs as **one copy on one machine with a persistent disk**. Conversations, notes and workspace files can all live in a directory on that disk.

This is a good first production deployment for one person, a family, or a small team. Rent a small virtual machine, or use a computer you control that stays on. Give it a domain name so that Telegram or WhatsApp can call it over HTTPS.

```text
Telegram / WhatsApp ── HTTPS ── your machine
                                  ├── Caddy (HTTPS)
                                  ├── Forma (one container)
                                  └── /data (persistent disk)
                                       ├── forma-db/       conversations
                                       └── workspaces/      notes and files
```

There is deliberately no MongoDB in this design. In `storage: file`, Forma keeps sessions in `./forma-db`; a local workspace store keeps its files in a directory you choose. It is safe only when exactly one Forma container writes to that disk. Do not scale it to several replicas.

> **Important:** a persistent disk survives a container restart; it is not a backup. Back up the whole data directory before relying on this setup.

## What you need

- A Linux machine that is always on, with a persistent disk. A small VM is enough for many personal agents.
- A domain name, such as `agent.example.com`, whose DNS `A`/`AAAA` record points to the machine's public IP address.
- Docker Engine and the Docker Compose plugin installed on the machine.
- An agent project made with `forma init`, with its `Dockerfile`, `src/`, `evals/`, and `runtime.yaml`.
- An LLM provider key.

Keep the machine up to date. In its provider firewall—and in the machine firewall—allow only TCP ports **80** and **443** to the internet. Allow SSH only from your own IP or, preferably, administer it through Tailscale or the provider's browser console.

## 1. Configure persistent local storage

In the agent project's `runtime.yaml`, use file sessions and an absolute directory below `/data` for workspaces:

```yaml
client: telegram # or whatsapp
storage: file
workspaces:
  store: local
  dir: /data/workspaces
```

Omit `workspaces` if the agent does not use workspace or note tools. Do not enable code execution in this setup.

The Compose file below sets the container's working directory to `/data`, so `storage: file` writes conversations to `/data/forma-db`.

Pin the `FORMA_VERSION` in the project's `Dockerfile` to the Forma version you tested, rather than leaving it as `latest`:

```dockerfile
ARG FORMA_VERSION=0.9.18
```

Use the version reported by `forma --version`; the number above is only an example.

## 2. Add a public HTTPS front door

Create `Caddyfile` next to the project's Dockerfile. Replace `agent.example.com` with your domain:

```caddyfile
agent.example.com {
  reverse_proxy forma:8080
}
```

Caddy obtains and renews the TLS certificate automatically. The domain must already point at the machine, and ports 80 and 443 must be reachable, before starting it.

Create `compose.yaml`:

```yaml
services:
  forma:
    build: .
    restart: unless-stopped
    env_file: .env
    working_dir: /data
    volumes:
      - ./data:/data
    expose:
      - "8080"

  caddy:
    image: caddy:2.8
    restart: unless-stopped
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy_data:/data
      - caddy_config:/config
    depends_on:
      - forma

volumes:
  caddy_data:
  caddy_config:
```

`forma` has no published host port: only Caddy can reach it. Caddy is the only public process.

## 3. Add secrets without committing them

Create a `.env` file on the machine, and make it readable only by the account that runs Docker:

```sh
umask 077
nano .env
chmod 600 .env
```

For Telegram, it contains at least:

```dotenv
TELEGRAM_BOT_TOKEN=...
TELEGRAM_SECRET_TOKEN=use-a-long-random-secret
OPENAI_API_KEY=...
```

For WhatsApp, use the four `WHATSAPP_*` variables in [Your agent on WhatsApp](./whatsapp.md), plus the LLM provider key. Add `.env` and `data/` to `.gitignore`; neither belongs in Git.

A channel-only agent does not expose a browser API. If you also use `client: sse`, `sync`, or `ai-sdk-v5`, set a long random `FORMA_AGENT_KEY` and put an authenticated application or identity-aware proxy in front of it. Do **not** build a browser page that sends that key to Forma. See [Forma Agents' API Key](./forma-agent-api-key.md).

## 4. Start and test

From the project directory on the machine:

```sh
docker compose up -d --build
docker compose logs -f forma
```

For Telegram, register `https://agent.example.com/v1/chat` as the webhook URL, using the same `TELEGRAM_SECRET_TOKEN`; follow [Tell Telegram where the agent is](./telegram.md#4-tell-telegram-where-the-agent-is). For WhatsApp, configure the same URL as its callback URL and follow [Your agent on WhatsApp](./whatsapp.md).

Send a harmless message. Then restart the container:

```sh
docker compose restart forma
```

The agent should remember the conversation. Confirm that `data/forma-db/` and, if used, `data/workspaces/` exist on the host.

## Security and recovery checklist

- Use a separate, unprivileged Linux account for this deployment; do not run day-to-day administration as `root`.
- Keep Docker, the OS, and the pinned Forma version updated. Upgrade Forma deliberately: test, change the version, rebuild, and restart.
- Protect SSH with keys and MFA at the VM provider. Disable password SSH login if you know how to administer the machine safely.
- Keep only 80/443 public. Do not publish port 8080, databases, or Docker's socket.
- Use long random webhook secrets. Telegram's secret token and WhatsApp's signature verification protect the public webhook from arbitrary requests; never turn those checks off.
- Give the LLM provider a budget limit/alert. Treat a leaked provider key as urgent: revoke and replace it.
- Back up `./data` automatically to another machine or encrypted object storage. Test restoring it to a new machine. The backup contains conversations and workspace files, so encrypt it and restrict access.
- If the machine is lost or compromised, create new provider and channel secrets, restore `data/` onto a new machine, and re-register the webhook.

## Limits of this option

This is intentionally a single-machine design: no high availability, no horizontal scaling, and no provider-managed database. A machine or disk outage stops the agent until it is restored. When you need multiple running copies, a managed conversation database, or durable object storage independent of one machine, move to the [general deployment architecture](./deployment.md) or the [Google Cloud guide](./cloud-deployment.md).
