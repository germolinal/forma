# Deploying to the cloud

The [Deployment](./deployment.md) section explains what a production server is made of: Forma's `forma` image, your compiled agent, and a `runtime.yaml` saying how clients talk to it and where conversations are kept. This guide puts one in the cloud, end to end: your agent in a container, running on a managed service, keeping its conversations in a database, and answering on the internet.

We will use [Google Cloud Run](https://cloud.google.com/run), which runs containers without managing servers. Nothing here depends on it, though: any service that runs a container and gives it a public HTTPS address works the same way (check [Other container services](#other-container-services)).

> **💡 Tip**: This guide is written so that every step can be followed as it is, by you or by a coding assistant. Check [Asking a coding assistant to do it](#asking-a-coding-assistant-to-do-it).

## What you will build

```text
          ┌──────────────── Cloud Run service ─────────────────┐
client ──▶│  /app/forma serve -a /app/agent.yaml               │──▶ LLM provider
 (web app,│  (Forma's server, with your compiled agent)        │
WhatsApp, └───────────────────────┬────────────────────────────┘
Telegram)                         ▼
                        MongoDB-compatible database
                        (conversations, in `forma-db`)
```

* **One container image**, built by your project's `Dockerfile`: Forma's published `forma` image, plus your compiled agent and its `runtime.yaml`.
* **One database**, so that conversations survive restarts, and several copies of the container can share them (Forma agents are stateless).
* **Secrets** (API keys, tokens, the database's address) kept out of the image, and given to the container when it starts.

## 0. Before you start

You will need:

* Your agent's project, working with `forma dev` (check [Dory, your first Forma agent](./dory.md)), with the `Dockerfile` that `forma init` creates (if your project is older, copy it from [Deployment](./deployment.md#2-the-dockerfile))
* [Docker](https://docs.docker.com/get-docker/), and the [`gcloud` CLI](https://cloud.google.com/sdk/docs/install), logged in (`gcloud auth login`)
* A Google Cloud project, with billing enabled
* A MongoDB-compatible database you can reach from the internet, and its connection string (e.g., a free [MongoDB Atlas](https://www.mongodb.com/atlas) cluster), plus [`mongosh`](https://www.mongodb.com/docs/mongodb-shell/)

Then, decide three things, and write them down as shell variables. Every command below uses them:

```sh
export PROJECT=my-gcp-project      # Your Google Cloud project's ID
export REGION=us-central1          # Where the service runs
export SERVICE=dory                # The name of the Cloud Run service
export CLIENT=sse                  # How clients talk to it: sync, sse, ai-sdk-v5, whatsapp or telegram
export IMAGE=$REGION-docker.pkg.dev/$PROJECT/forma/$SERVICE
```

| `CLIENT`    | Choose it for...                                                   | Its own variables                                     |
| ----------- | ------------------------------------------------------------------ | ----------------------------------------------------- |
| `sync`      | A back-end that sends a message and waits for the answer          | `FORMA_AGENT_KEY` ([details](./forma-agent-api-key.md)) |
| `sse`       | The same, streaming progress ([HTTP API](./http-api.md))          | `FORMA_AGENT_KEY`                                     |
| `ai-sdk-v5` | A web front-end built with Vercel's AI SDK                         | `FORMA_AGENT_KEY`                                     |
| `whatsapp`  | A WhatsApp number ([guide](./whatsapp.md))                        | `WHATSAPP_PHONE_NUMBER_ID`, `WHATSAPP_ACCESS_TOKEN`, `WHATSAPP_VERIFY_TOKEN`, `WHATSAPP_APP_SECRET` |
| `telegram`  | A Telegram bot ([guide](./telegram.md))                           | `TELEGRAM_BOT_TOKEN`, `TELEGRAM_SECRET_TOKEN`         |

And turn on the services this guide uses:

```sh
gcloud config set project $PROJECT
gcloud services enable run.googleapis.com artifactregistry.googleapis.com secretmanager.googleapis.com
```

## 1. Get your agent ready

In your agent's project, make sure `runtime.yaml` names your client, and keeps conversations in the database (the CLI keeps them in files):

```yaml
client: sse          # your $CLIENT
storage: mongo
```

In the `Dockerfile`, pin Forma's version to the one you develop with (`forma --version`), so the image doesn't change when Forma releases a new one:

```dockerfile
ARG FORMA_VERSION=0.9.18
```

Then, check that the agent compiles, and what it needs at runtime:

```sh
forma expand -c > /dev/null
```

Every LLM provider and tool your agent uses reads its own variables (e.g., `OPENAI_API_KEY` for OpenAI, `TAVILY_API_KEY` for web search; check the [Reference Documentation](../reference_docs/auto-genaiclient.md)). Write them down: they go in step 4.

> **📌 Note**: An agent on Ollama will not work in the cloud as it is, as Ollama runs on your computer. Use a hosted provider (e.g., `vertex-ai` or `openai`) before deploying.

## 2. Build the image

The `Dockerfile` compiles your agent with the `forma-cli` image, and copies it, with `runtime.yaml`, into the `forma` image. Build it, and push it to a registry Cloud Run can read from:

```sh
gcloud artifacts repositories create forma --repository-format=docker --location=$REGION
gcloud auth configure-docker $REGION-docker.pkg.dev

docker build --platform linux/amd64 -t $IMAGE .
docker push $IMAGE
```

(The repository only needs to be created once.)

> **📌 Note**: Cloud Run runs `linux/amd64` images, hence `--platform linux/amd64`. Forma's images are published for both `linux/amd64` and `linux/arm64`, so this is quick on any computer: nothing is compiled, apart from your agent.

The image's command, `forma serve -t console`, sends logs to the container's output, which Cloud Run collects. (To send traces and metrics to an OpenTelemetry collector as well, use `-t otel`, and check [Observability and Monitoring](./observability.md).)

## 3. Prepare the database

Forma keeps everything in a database called `forma-db`, and creates what it needs as it goes. It does not create indexes, though, and without them every message makes the database read every conversation. Create them once, with `mongosh`:

```sh
export SESSIONS_DB_URL='mongodb+srv://user:password@cluster0.example.mongodb.net/'

mongosh "$SESSIONS_DB_URL" --eval '
  const d = db.getSiblingDB("forma-db");
  d.messages.createIndex({ session_id: 1, created_at: -1 });
  d.messages.createIndex({ id: 1, session_id: 1 });
  d.sessions.createIndex({ user_id: 1, agent_id: 1, created_at: -1 });
'
```

Here, `SESSIONS_DB_URL` is your database's connection string. Make sure the database accepts connections from Cloud Run: on MongoDB Atlas, allow access from anywhere (`0.0.0.0/0`) in **Network Access**, and rely on the user and password, or connect through a private network.

## 4. Store the secrets

Secrets go in [Secret Manager](https://cloud.google.com/secret-manager), never in the image. Create one per variable: the database's address, your client's own variables (step 0), and those your agent needs (step 1). For example, for the `sse` client:

```sh
secret() { printf '%s' "$2" | gcloud secrets create "$1" --data-file=- ; }

secret sessions-db-url "$SESSIONS_DB_URL"
secret forma-agent-key "$(openssl rand -hex 32)"   # The API key clients will send
secret openai-api-key  "sk-..."
```

Then, create the identity the service runs as, and let it read them:

```sh
gcloud iam service-accounts create $SERVICE
export SA=$SERVICE@$PROJECT.iam.gserviceaccount.com

for s in sessions-db-url forma-agent-key openai-api-key; do
  gcloud secrets add-iam-policy-binding $s \
    --member serviceAccount:$SA --role roles/secretmanager.secretAccessor
done
```

> **📌 Note**: If your agent uses Google's models (`vertex-ai`) or other Google Cloud services, it uses this identity too, rather than an API key. Grant it what it needs (e.g., `roles/aiplatform.user` for Vertex AI), and set `GOOGLE_CLOUD_PROJECT` and `GOOGLE_CLOUD_LOCATION` in the next step.

## 5. Deploy

```sh
gcloud run deploy $SERVICE \
  --image $IMAGE \
  --region $REGION \
  --service-account $SA \
  --allow-unauthenticated \
  --timeout 3600 \
  --set-env-vars GOOGLE_CLOUD_PROJECT=$PROJECT,GOOGLE_CLOUD_LOCATION=$REGION \
  --set-secrets SESSIONS_DB_URL=sessions-db-url:latest,FORMA_AGENT_KEY=forma-agent-key:latest,OPENAI_API_KEY=openai-api-key:latest
```

Each `--set-secrets` entry is `VARIABLE=secret-name:version`: add one for every secret of step 4.

| Flag                      | Why                                                                                                   |
| ------------------------- | ----------------------------------------------------------------------------------------------------- |
| `--allow-unauthenticated` | Clients reach the service directly. It is still protected: by the API key (`sync`, `sse`, `ai-sdk-v5`), or by the platform's signature (`whatsapp`, `telegram`) |
| `--timeout 3600`          | An answer can take minutes (tools, long documents), and Cloud Run cuts requests after 5 minutes by default    |

Cloud Run sets `PORT` itself, and the server listens on it. When the deploy finishes, it prints the service's address:

```text
Service URL: https://dory-123456789012.us-central1.run.app
```

> **✅ Check progress**: Look at the logs, with `gcloud run services logs read $SERVICE --region $REGION`. You should see `Listening on 0.0.0.0:8080`.

## 6. Talk to it

What comes next depends on the client:

* **`sync`, `sse` and `ai-sdk-v5`**: your clients call the service's address. Try it with the [HTTP API](./http-api.md):

  ```sh
  export URL=https://dory-123456789012.us-central1.run.app
  export KEY=$(gcloud secrets versions access latest --secret forma-agent-key)

  SESSION=$(curl -s $URL/v1/init -H "X-API-Key: $KEY" -H "X-User-ID: me" | sed 's/.*"session_id":"\([^"]*\)".*/\1/')
  curl -N -X POST $URL/v1/chat \
    -H "X-API-Key: $KEY" -H "X-User-ID: me" -H "X-Session-ID: $SESSION" \
    -H "Content-Type: application/json" \
    -d '{"role":"user","content":"Hi!"}'
  ```

* **`whatsapp`**: in Meta's App Dashboard, set the **Callback URL** to `$URL/v1/chat` (check [step 5 of the WhatsApp guide](./whatsapp.md#5-go-to-meta-and-connect-the-webhook)).
* **`telegram`**: register `$URL/v1/chat` with `setWebhook`, with your secret token (check [step 4 of the Telegram guide](./telegram.md#4-tell-telegram-where-your-agent-is)).

> **✅ Check progress**: Does your agent answer?

## Updating your agent

Every change to your agent is a new image, deployed on top of the old one:

```sh
docker build --platform linux/amd64 -t $IMAGE .
docker push $IMAGE
gcloud run deploy $SERVICE --image $IMAGE --region $REGION
```

Conversations are in the database, so they carry on. The new version answers the next message: for a while, there may be messages being answered by the old one.

To move to a newer Forma, change `FORMA_VERSION` in the `Dockerfile`, and update your CLI to the same version: the agent you test is then the agent you ship.

> **💡 Tip**: Before deploying a change, [evaluate it](./evaluations.md): a new image is a good moment to catch a prompt that got worse. In a CI/CD pipeline, the `ghcr.io/germolinal/forma-cli` image runs `forma eval` (check [Deployment](./deployment.md#compiling-in-a-pipeline)).

## Other container services

Every step above, except steps 4 and 5, is the same anywhere. Whatever service you use, it needs to:

1. **Run a container** (Forma's images exist for `linux/amd64` and `linux/arm64`; build yours for the service's architecture), and give it a public HTTPS address (WhatsApp and Telegram refuse anything else).
2. **Pass the variables** of steps 1, 4 and the table in step 0 to the container, preferably from its own secrets store.
3. **Set `PORT`**, or let the server use its default (`8080`), and send requests there.
4. **Allow long requests**: an hour, if possible. A request cut short is an answer lost.
5. **Reach the database**, and the LLM provider.

## If something does not work

| What you see                                                          | What to check                                                                                     |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `docker build` fails at `forma expand -c`                             | The agent does not compile: run `forma expand -c` in your project to see why (step 1). MCP tools that need `forma login` can't be compiled in the build: check [Deployment](./deployment.md#2-the-dockerfile) |
| `the agent in '/app/agent.yaml' has no nodes`                         | The agent in the image was not compiled: did you replace `forma expand -c` with a copy of an uncompiled file? |
| `exec format error`, or the container does not start                  | The image is not `linux/amd64`: build with `--platform linux/amd64`                               |
| `Missing environment variable: ...`                                   | That variable is not set in step 5. Each client fails at start without its own (step 0)           |
| The service starts, but every message fails with a database error     | `SESSIONS_DB_URL` is wrong, or the database does not accept connections from the service          |
| `Permission denied on secret`                                         | The service account cannot read a secret: grant it `roles/secretmanager.secretAccessor` (step 4)  |
| Answers are cut, or the client sees a `504`                           | The request timeout: deploy with `--timeout 3600`                                                 |
| Answers get slower as conversations pile up                           | The database's indexes (step 3)                                                                   |

## Asking a coding assistant to do it

Each step above is a command, so a coding assistant (e.g., [Claude Code](https://claude.com/claude-code)) can do it for you, from your agent's project. Give it this page, and the decisions it cannot make for you:

```text
Deploy the Forma agent in this project to Google Cloud Run, following
https://germolinal.github.io/forma/docs/how-to/cloud-deployment.html

- Project: my-gcp-project, region: us-central1, service: dory
- Client: telegram
- Database: the connection string is in the SESSIONS_DB_URL variable
- Secrets the agent needs: OPENAI_API_KEY (in my .env)

Ask me before creating anything that costs money, and show me how to check
that the agent answers once it is deployed.
```

It is worth telling it, too:

* **Which client**, and what is on the other end (your web app, a WhatsApp number, a Telegram bot). It decides the variables, and step 6.
* **Where the secrets are** (e.g., your `.env`), and to put them in Secret Manager rather than in the image or in the command line history.
* **To stop if something fails**, rather than working around it: most failures are a missing variable or permission, which the table above names.
