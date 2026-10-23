# Deploying to Google Cloud, step by step

This guide puts **one personal agent** on **Google Cloud**, from an empty account to an agent that answers on the internet. Every step is a command you can copy, and each one says what it does and how to check that it worked.

It is written for Google Cloud, using:

* **[Cloud Run](https://cloud.google.com/run)** to run the agent. It starts the agent when a message arrives, and stops it when there are none, so an agent nobody is talking to costs (almost) nothing.
* **[Firestore](https://cloud.google.com/firestore/mongodb-compatibility/docs/overview)** (with MongoDB compatibility) to keep conversations.
* **[Secret Manager](https://cloud.google.com/secret-manager)** to keep API keys and passwords.
* Optionally, **[Cloud Storage](https://cloud.google.com/storage)** buckets for workspaces and knowledge bases, and a **Cloud Run Job** that runs the agent's code in a sandbox.

You build nothing: Cloud Run runs Forma's published image (`ghcr.io/germolinal/forma`), and your agent reaches it as two files in a bucket.

> **💡 Tip**: Prefer infrastructure as code? The [`deployments/gcp`](https://github.com/germolinal/forma/tree/main/deployments/gcp) folder of the Forma repository creates the same things with Terraform. On another cloud, the [Deployment](./deployment.md) section explains what a server needs, and your project's `Dockerfile` builds an image that runs anywhere.

## What you will build

```text
                  ┌──────── Cloud Run service ────────┐
 you (web app, ──▶│  ghcr.io/germolinal/forma         │──▶ the LLM (e.g., Gemini)
 WhatsApp,        │  reads /agent/agent.yaml          │
 Telegram)        │    and /agent/runtime.yaml  ◀─────┼── bucket: your agent's files
                  └────────────────┬──────────────────┘
                                   ▼
                    Firestore database `forma-db`
                         (your conversations)
```

It takes about an hour the first time, most of it waiting for things to be created.

## 1. Install the tools, and log in

You need:

* **The Forma CLI**, the one you developed your agent with (`forma --version`). If you don't have it, check [Getting started](./getting-started.md#installing-the-forma-cli).
* **The `gcloud` CLI**. Follow [Google's installation guide](https://cloud.google.com/sdk/docs/install).
* **A Google account**, and a credit card for billing. A personal agent usually stays within, or close to, the free tiers, but Google asks for one anyway.

Log in:

```sh
gcloud auth login
```

> **✅ Check progress**: `gcloud auth list` shows your account, with a `*` next to it.

## 2. Create a project

A Google Cloud project holds everything this guide creates. Giving your agent its own project keeps it apart from anything else, and makes it easy to remove.

```sh
export PROJECT=my-forma-agent-1234   # Must be unique across all of Google Cloud: add some digits
gcloud projects create $PROJECT
gcloud config set project $PROJECT
```

Then link it to a billing account. The easiest way is in the console: open [Billing](https://console.cloud.google.com/billing/linkedaccount), choose your project, and link it. (Or, with the CLI: `gcloud billing accounts list`, then `gcloud billing projects link $PROJECT --billing-account=<ACCOUNT_ID>`.)

> **✅ Check progress**: `gcloud billing projects describe $PROJECT` says `billingEnabled: true`.

## 3. Write down your choices

Every command below uses these variables. If you open a new terminal, set them again.

```sh
export PROJECT=my-forma-agent-1234    # From step 2
export REGION=us-central1             # Where everything runs. Choose one close to you (e.g., europe-west1, southamerica-east1)
export SERVICE=dory                   # Your agent's name, in lowercase
export FORMA_VERSION=0.9.18           # Your Forma CLI's version (`forma --version`)
export PROJECT_NUMBER=$(gcloud projects describe $PROJECT --format='value(projectNumber)')
export SA=$SERVICE@$PROJECT.iam.gserviceaccount.com
export AGENT_BUCKET=$SERVICE-agent-$PROJECT_NUMBER
```

## 4. Turn on the services

A new project has most services turned off. Turn on the ones this guide uses:

```sh
gcloud services enable run.googleapis.com firestore.googleapis.com secretmanager.googleapis.com \
  storage.googleapis.com aiplatform.googleapis.com iam.googleapis.com
```

(It can take a minute or two.)

## 5. Create the database

Your agent's conversations are kept in a Firestore database that speaks MongoDB's language, which is what Forma talks to. Create it, named `forma-db` (the name Forma uses):

```sh
gcloud firestore databases create --database=forma-db --location=$REGION \
  --edition=enterprise --enable-mongodb-compatible-data-access
```

> **📌 Note**: Not every region has Firestore with MongoDB compatibility. If this fails, choose another from [the list](https://cloud.google.com/firestore/mongodb-compatibility/docs/locations) for `--location` (e.g., `nam5` for the United States, `eur3` for Europe), and use that same value wherever this step says `$REGION`.

Then create the three indexes Forma's queries need. Without them, every message makes the database read every conversation, and your agent gets slower over time:

```sh
index() {
  gcloud firestore indexes composite create --database=forma-db --collection-group=$1 \
    --query-scope=collection-group --density=dense --api-scope=mongodb-compatible-api "${@:2}" --async
}
index messages --field-config=field-path=session_id,order=ascending --field-config=field-path=created_at,order=descending
index messages --field-config=field-path=id,order=ascending --field-config=field-path=session_id,order=ascending
index sessions --field-config=field-path=user_id,order=ascending --field-config=field-path=agent_id,order=ascending --field-config=field-path=created_at,order=descending
```

They take a few minutes to build. You can carry on in the meantime.

Finally, create a user for Forma, with its password, and let it use this database (and no other):

```sh
gcloud firestore user-creds create forma --database=forma-db
```

This prints the user's password. **Copy it now**: it is shown only once. Then:

```sh
gcloud projects add-iam-policy-binding $PROJECT \
  --member="principal://firestore.googleapis.com/projects/$PROJECT_NUMBER/name/databases/forma-db/userCreds/forma" \
  --role=roles/datastore.user \
  --condition="expression=resource.name == \"projects/$PROJECT/databases/forma-db\",title=forma-db-only"
```

Now, put together the database's address (its *connection string*), which Forma reads from `SESSIONS_DB_URL`. Replace `PASSWORD` with the password you copied:

```sh
export DB_HOST=$(gcloud firestore databases describe --database=forma-db --format='value(uid)').$REGION.firestore.goog
export SESSIONS_DB_URL="mongodb://forma:PASSWORD@$DB_HOST:443/forma-db?loadBalanced=true&tls=true&authMechanism=SCRAM-SHA-256&retryWrites=false"
```

> **✅ Check progress**: `gcloud firestore indexes composite list --database=forma-db` lists three indexes. (`READY` once they are built.)

## 6. Get your agent ready

In your agent's project, three things:

**1. A hosted LLM.** Ollama runs on your computer, so it won't be there in the cloud. On Google Cloud, the simplest is Google's own models (`vertex-ai`), which need no API key: your agent uses its Google Cloud identity (step 8). For instance:

```yaml
llm:
  provider: vertex-ai
  model: gemini-2.5-flash
```

(Check [`VertexAI`](../reference_docs/auto-vertexai.md) for every option. Other providers work too; their API key goes in step 9.) Try the agent with `forma dev` after changing it.

**2. The runtime file.** In `runtime.yaml`, choose how you will talk to your agent (`client`), and keep conversations in the database:

```yaml
client: telegram     # or sse, sync, ai-sdk-v5, whatsapp
storage: mongo
```

| `client`    | Choose it for...                                                   | Secrets it needs (step 9)                             |
| ----------- | ------------------------------------------------------------------ | ----------------------------------------------------- |
| `telegram`  | A Telegram bot: the easiest way to chat with a personal agent ([guide](./telegram.md)) | `TELEGRAM_BOT_TOKEN`, `TELEGRAM_SECRET_TOKEN`         |
| `whatsapp`  | A WhatsApp number ([guide](./whatsapp.md))                        | `WHATSAPP_PHONE_NUMBER_ID`, `WHATSAPP_ACCESS_TOKEN`, `WHATSAPP_VERIFY_TOKEN`, `WHATSAPP_APP_SECRET` |
| `sse`, `sync` | Your own app or script, through the [HTTP API](./http-api.md)  | `FORMA_AGENT_KEY` ([details](./forma-agent-api-key.md)) |
| `ai-sdk-v5` | A web front-end built with Vercel's AI SDK                         | `FORMA_AGENT_KEY`                                     |

**3. The compiled agent.** The server reads your agent as a single, compiled file:

```sh
forma expand -c > agent.yaml
```

> **❗Important**: Do not forget the `-c`. Without it, the server refuses to start (`the agent ... has no nodes`).

## 7. Upload your agent's files

Create a bucket for `agent.yaml` and `runtime.yaml`, and copy them in. (Neither holds secrets: those go in step 9.)

```sh
gcloud storage buckets create gs://$AGENT_BUCKET --location=$REGION \
  --uniform-bucket-level-access --public-access-prevention
gcloud storage cp agent.yaml runtime.yaml gs://$AGENT_BUCKET/
```

> **✅ Check progress**: `gcloud storage ls gs://$AGENT_BUCKET` lists both files.

## 8. Give your agent an identity

Your agent runs as a *service account*: a Google Cloud identity of its own, which can only do what you allow it to. Create it, and let it read its files and use Google's models:

```sh
gcloud iam service-accounts create $SERVICE --display-name="Forma agent ($SERVICE)"

gcloud storage buckets add-iam-policy-binding gs://$AGENT_BUCKET \
  --member=serviceAccount:$SA --role=roles/storage.objectViewer
gcloud projects add-iam-policy-binding $PROJECT \
  --member=serviceAccount:$SA --role=roles/aiplatform.user
```

## 9. Store the secrets

Secrets go in Secret Manager, never in your files. Create one for the database's address, and one for each secret your client needs (the table in step 6) and your agent's tools need (e.g., `TAVILY_API_KEY` for web search, `OPENAI_API_KEY` for OpenAI's models). Then let your agent read them.

For instance, for a Telegram bot:

```sh
secret() {
  printf '%s' "$2" | gcloud secrets create "$1" --data-file=-
  gcloud secrets add-iam-policy-binding "$1" --member=serviceAccount:$SA --role=roles/secretmanager.secretAccessor
}

secret sessions-db-url       "$SESSIONS_DB_URL"
secret telegram-bot-token    "123456:ABC-..."              # From @BotFather
secret telegram-secret-token "$(openssl rand -hex 32)"     # A password you make up
```

And for `sse`, `sync` or `ai-sdk-v5`, instead of the Telegram ones:

```sh
secret forma-agent-key "$(openssl rand -hex 32)"           # The key your app will send
```

> **✅ Check progress**: `gcloud secrets list` shows them all.

## 10. Deploy

This starts your agent: Forma's published image, with your bucket mounted at `/agent`, and every secret given to it as an environment variable. Adjust the `--set-secrets` line to your secrets (`VARIABLE=secret-name:latest`, separated by commas):

```sh
gcloud run deploy $SERVICE \
  --image=ghcr.io/germolinal/forma:$FORMA_VERSION \
  --region=$REGION \
  --service-account=$SA \
  --execution-environment=gen2 \
  --add-volume=name=agent,type=cloud-storage,bucket=$AGENT_BUCKET,readonly=true \
  --add-volume-mount=volume=agent,mount-path=/agent \
  --command=/app/forma \
  --args=serve,-t,otel,-a,/agent/agent.yaml,-r,/agent/runtime.yaml \
  --set-env-vars=GOOGLE_CLOUD_PROJECT=$PROJECT,GOOGLE_CLOUD_LOCATION=$REGION \
  --set-secrets=SESSIONS_DB_URL=sessions-db-url:latest,TELEGRAM_BOT_TOKEN=telegram-bot-token:latest,TELEGRAM_SECRET_TOKEN=telegram-secret-token:latest \
  --timeout=3600 \
  --max-instances=2 \
  --allow-unauthenticated
```

What some of those mean:

| Flag                      | Why                                                                                                   |
| ------------------------- | ----------------------------------------------------------------------------------------------------- |
| `--add-volume`, `--add-volume-mount` | Your bucket appears in the container as the folder `/agent` (Cloud Storage volumes need `--execution-environment=gen2`) |
| `--command`, `--args`     | Runs Forma's server with your agent and runtime file                                                  |
| `--timeout=3600`          | An answer can take minutes (tools, long documents), and Cloud Run cuts requests after 5 minutes by default |
| `--max-instances=2`       | A ceiling on how many copies run at once: a personal agent needs very few, and it caps the bill        |
| `--allow-unauthenticated` | Telegram, WhatsApp and your apps reach the agent directly. It is still protected: by the platform's signature, or by the API key |

When it finishes, it prints your agent's address:

```text
Service URL: https://dory-123456789012.us-central1.run.app
```

> **✅ Check progress**: `gcloud run services logs read $SERVICE --region=$REGION` shows `Listening on 0.0.0.0:8080`.

## 11. Talk to it

```sh
export URL=$(gcloud run services describe $SERVICE --region=$REGION --format='value(status.url)')
```

* **Telegram**: tell Telegram where your agent is, with the bot's token and your secret token (check [step 4 of the Telegram guide](./telegram.md#4-tell-telegram-where-your-agent-is)):

  ```sh
  curl "https://api.telegram.org/bot<BOT_TOKEN>/setWebhook" \
    -d "url=$URL/v1/chat" -d "secret_token=<SECRET_TOKEN>"
  ```

  Then send your bot a message.
* **WhatsApp**: in Meta's App Dashboard, set the **Callback URL** to `$URL/v1/chat` (check [step 5 of the WhatsApp guide](./whatsapp.md#5-go-to-meta-and-connect-the-webhook)).
* **`sse`, `sync`, `ai-sdk-v5`**: call the [HTTP API](./http-api.md):

  ```sh
  export KEY=$(gcloud secrets versions access latest --secret=forma-agent-key)
  SESSION=$(curl -s $URL/v1/init -H "X-API-Key: $KEY" -H "X-User-ID: me" | sed 's/.*"session_id":"\([^"]*\)".*/\1/')
  curl -N -X POST $URL/v1/chat \
    -H "X-API-Key: $KEY" -H "X-User-ID: me" -H "X-Session-ID: $SESSION" \
    -H "Content-Type: application/json" \
    -d '{"role":"user","content":"Hi!"}'
  ```

> **✅ Check progress**: Does your agent answer? 🎉

## Extra A: workspaces and knowledge bases

Workspaces (folders of documents the agent reads and writes), scrapbooks (private notes per user) and knowledge bases (`google-knowledgebase`, `google-workspace` tools) live in a Cloud Storage bucket.

**1. Create the bucket**, and let your agent read and write it:

```sh
export WORKSPACES_BUCKET=workspaces-$PROJECT_NUMBER
gcloud storage buckets create gs://$WORKSPACES_BUCKET --location=$REGION \
  --uniform-bucket-level-access --public-access-prevention
gcloud storage buckets add-iam-policy-binding gs://$WORKSPACES_BUCKET \
  --member=serviceAccount:$SA --role=roles/storage.objectUser
```

**2. Let your agent sign links to its files.** It hands out short-lived links (to files it shares, and to code runs), signed as itself, so no key file ever exists:

```sh
gcloud services enable iamcredentials.googleapis.com
gcloud iam service-accounts add-iam-policy-binding $SA \
  --member=serviceAccount:$SA --role=roles/iam.serviceAccountTokenCreator
```

**3. Tell Forma**, in `runtime.yaml`, and upload it:

```yaml
workspaces:
  store: gcs
```

```sh
gcloud storage cp runtime.yaml gs://$AGENT_BUCKET/
```

**4. Give the service the bucket's name** (this also restarts it, so it reads the new `runtime.yaml`):

```sh
gcloud run services update $SERVICE --region=$REGION --update-env-vars=WORKSPACES_BUCKET=$WORKSPACES_BUCKET
```

To put documents in a knowledge base, copy them into the bucket under the workspace's folder (its `workspace_id`), e.g., `gcloud storage cp -r ./handbook gs://$WORKSPACES_BUCKET/`. A knowledge base in **another** bucket (its `bucket` field) needs read access there too: `gcloud storage buckets add-iam-policy-binding gs://<that-bucket> --member=serviceAccount:$SA --role=roles/storage.objectViewer`.

## Extra B: running code

The [`code`](../reference_docs/auto-codeexecution.md) tool lets your agent run Python and shell commands. On Google Cloud, each run is an execution of a Cloud Run Job, running Forma's published sandbox (`ghcr.io/germolinal/forma-sandbox`). It needs Extra A first: runs keep their files in the workspaces bucket.

Code written by an LLM is not trusted, so the sandbox gets nothing: an identity with no permissions at all, and a network with no internet. It reaches its files only through the links your agent signs for each run.

**1. An identity with no permissions:**

```sh
gcloud iam service-accounts create $SERVICE-sandbox --display-name="Forma sandbox (no roles, on purpose)"
```

**2. A network without internet.** It has no router to the internet, only a private path to Cloud Storage (for the signed links):

```sh
gcloud services enable compute.googleapis.com
gcloud compute networks create $SERVICE-sandbox --subnet-mode=custom
gcloud compute networks subnets create $SERVICE-sandbox --network=$SERVICE-sandbox \
  --region=$REGION --range=10.80.0.0/24 --enable-private-ip-google-access
```

**3. The job:**

```sh
gcloud run jobs create $SERVICE-code-execution \
  --image=ghcr.io/germolinal/forma-sandbox:$FORMA_VERSION \
  --region=$REGION \
  --service-account=$SERVICE-sandbox@$PROJECT.iam.gserviceaccount.com \
  --network=$SERVICE-sandbox --subnet=$SERVICE-sandbox --vpc-egress=all-traffic \
  --tasks=1 --max-retries=0 --task-timeout=360s --cpu=1 --memory=2Gi
```

**4. Let your agent start it, and follow its runs** (on this job only):

```sh
for role in roles/run.jobsExecutorWithOverrides roles/run.viewer; do
  gcloud run jobs add-iam-policy-binding $SERVICE-code-execution --region=$REGION \
    --member=serviceAccount:$SA --role=$role
done
```

**5. Keep what the code prints out of the logs** (it is your data):

```sh
gcloud logging sinks update _Default --add-exclusion=name=$SERVICE-sandbox-output,filter='resource.type="cloud_run_job" AND resource.labels.job_name="'$SERVICE'-code-execution" AND (log_id("run.googleapis.com/stdout") OR log_id("run.googleapis.com/stderr"))'
```

**6. Delete old runs' files after a day**, so the bucket doesn't grow forever:

```sh
cat > lifecycle.json <<'EOF'
{"rule": [{"action": {"type": "Delete"}, "condition": {"age": 1, "matchesPrefix": ["runs/"]}}]}
EOF
gcloud storage buckets update gs://$WORKSPACES_BUCKET --lifecycle-file=lifecycle.json
```

**7. Tell Forma**, in `runtime.yaml`, upload it, and give the service the job's name:

```yaml
code_execution:
  sandbox: cloud-run-job
```

```sh
gcloud storage cp runtime.yaml gs://$AGENT_BUCKET/
gcloud run services update $SERVICE --region=$REGION --update-env-vars=CODE_EXECUTION_JOB=$SERVICE-code-execution
```

## Updating your agent

Compile it again, upload it, and restart the service, which then reads the new file:

```sh
forma expand -c > agent.yaml
gcloud storage cp agent.yaml gs://$AGENT_BUCKET/
gcloud run services update $SERVICE --region=$REGION --update-env-vars=AGENT_UPDATED=$(date +%s)
```

(Changing a variable is what makes Cloud Run start a new copy; its value doesn't matter.) Conversations are in the database, so they carry on.

To move to a newer Forma, update your CLI, compile again, and deploy the new image (and, with Extra B, the new sandbox):

```sh
gcloud run services update $SERVICE --region=$REGION --image=ghcr.io/germolinal/forma:$FORMA_VERSION
gcloud run jobs update $SERVICE-code-execution --region=$REGION --image=ghcr.io/germolinal/forma-sandbox:$FORMA_VERSION
```

> **💡 Tip**: Before uploading a change, [evaluate it](./evaluations.md): it is a good moment to catch a prompt that got worse.

## Removing everything

The simplest way is to delete the project, which deletes everything in it (after a 30-day grace period):

```sh
gcloud projects delete $PROJECT
```

## If something does not work

| What you see                                                          | What to check                                                                                     |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| The deploy fails, and the logs say `could not load agent`             | The files are not in the bucket (step 7), or the agent cannot read it (step 8)                    |
| `the agent in '/agent/agent.yaml' has no nodes`                       | It was not compiled: `forma expand -c > agent.yaml` (step 6), and upload it again                 |
| `Missing environment variable: ...`                                   | That secret is not in `--set-secrets` (step 10). Each client fails at start without its own       |
| `Permission denied on secret`                                         | The agent cannot read a secret: the `add-iam-policy-binding` of step 9                            |
| Every message fails with a database error                             | `SESSIONS_DB_URL` (step 5): the password, the region in the host, or the user's permission        |
| Errors from Vertex AI (`403`, `PERMISSION_DENIED`)                    | `roles/aiplatform.user` (step 8), and `GOOGLE_CLOUD_PROJECT`/`GOOGLE_CLOUD_LOCATION` (step 10)    |
| Answers are cut, or the client sees a `504`                           | The request timeout: `--timeout=3600`                                                             |
| Answers get slower as conversations pile up                           | The database's indexes (step 5)                                                                   |
| Workspace tools fail with `WORKSPACES_BUCKET`, or links fail to sign  | Extra A: the variable, and the `serviceAccountTokenCreator` role                                  |
| Code runs fail to start                                               | Extra B: `CODE_EXECUTION_JOB`, and the two roles on the job                                       |

## Asking a coding assistant to do it

Each step above is a command, so a coding assistant (e.g., [Claude Code](https://claude.com/claude-code)) can do it for you, from your agent's project. Give it this page, and the decisions it cannot make for you:

```text
Deploy the Forma agent in this project to Google Cloud, following
https://germolinal.github.io/forma/docs/how-to/cloud-deployment.html

- Project: create a new one called my-forma-agent-1234, region us-central1
- Agent name: dory. Client: telegram (the bot token is in my .env)
- Use Vertex AI for the model
- No workspaces, no code execution

Ask me before creating anything that costs money, stop if something fails,
and show me how to check that the agent answers once it is deployed.
```

It is worth telling it, too:

* **Which client**, and what is on the other end (a Telegram bot, a WhatsApp number, your web app). It decides the secrets, and step 11.
* **Where the secrets are** (e.g., your `.env`), and to put them in Secret Manager rather than in files or in the command line history.
* **To stop if something fails**, rather than working around it: most failures are a missing permission or secret, which the table above names.
