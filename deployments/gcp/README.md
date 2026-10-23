# One Forma agent on Google Cloud, with Terraform

This folder creates everything a single-tenant Forma agent needs on Google
Cloud: the same things the [step-by-step guide](https://germolinal.github.io/forma/docs/how-to/cloud-deployment.html)
creates with `gcloud`.

**Nothing is built.** The service runs Forma's published image,
`ghcr.io/germolinal/forma`, and reads your compiled agent and its
`runtime.yaml` from a small bucket mounted into it. Code execution, if you
turn it on, runs the published `ghcr.io/germolinal/forma-sandbox`.

| What | Where |
| --- | --- |
| The service that runs your agent (Cloud Run), its service account, and the bucket with `agent.yaml` and `runtime.yaml` | `main.tf` |
| The database that keeps conversations (Firestore with MongoDB compatibility), with the indexes Forma needs | `main.tf` |
| The secrets the agent reads (Secret Manager): only the containers, you add the values | `main.tf` |
| Optional: a bucket for workspaces, scrapbooks and knowledge bases | `workspaces.tf` |
| Optional: code execution, as a Cloud Run Job on a network without internet | `code-execution.tf` |

Use it if you know Terraform, or want your infrastructure written down.
Otherwise, the guide does the same thing one command at a time.

## Before you start

- [Terraform](https://developer.hashicorp.com/terraform/install) 1.5 or newer,
  and the [`gcloud` CLI](https://cloud.google.com/sdk/docs/install). No Docker.
- A Google Cloud project with billing enabled, and your own account allowed to
  administer it (e.g., the project's Owner).
- Your agent's project, working with `forma dev`, with a `runtime.yaml` that
  says `storage: mongo`, and on a hosted LLM provider (`vertex-ai` needs no API
  key here).

Copy this folder into your agent's project, e.g., as `deploy/gcp/`, so that
your infrastructure is versioned with your agent. The defaults expect
`agent.yaml` and `runtime.yaml` two folders up (`../../`).

Log in twice: once for you, once for Terraform.

```sh
gcloud auth login
gcloud auth application-default login
```

## 1. Compile the agent

The server reads a compiled agent. From your agent's project:

```sh
forma expand -c > agent.yaml
```

(Without the CLI: `docker run --rm -v "$PWD":/work -w /work ghcr.io/germolinal/forma-cli:<version> forma expand -c > agent.yaml`.)

## 2. Create the infrastructure

From this folder:

```sh
cp terraform.tfvars.example terraform.tfvars   # then edit it
terraform init
terraform apply
```

Leave `deploy = false` this first time: the secrets have no values yet, and a
service that reads an empty secret fails to start. Everything but the service
is created. (The database's indexes take a few minutes to build.)

## 3. Connect the database

Forma connects to Firestore as to MongoDB, with a username and a password.
The user is created with `gcloud` rather than Terraform, because its password
is shown only once, and would otherwise be kept in the Terraform state.

```sh
export PROJECT=my-gcp-project DATABASE=forma-db DB_USER=forma
export PROJECT_NUMBER=$(gcloud projects describe $PROJECT --format='value(projectNumber)')

# Prints the password: copy it.
gcloud firestore user-creds create $DB_USER --database=$DATABASE

# Lets that user read and write this database, and no other.
gcloud projects add-iam-policy-binding $PROJECT \
  --member="principal://firestore.googleapis.com/projects/$PROJECT_NUMBER/name/databases/$DATABASE/userCreds/$DB_USER" \
  --role=roles/datastore.user \
  --condition="expression=resource.name == \"projects/$PROJECT/databases/$DATABASE\",title=forma-db-only"
```

Then store the connection string, replacing `PASSWORD` with the one
`user-creds create` printed:

```sh
export DB_HOST=$(terraform output -raw database_host)
printf '%s' "mongodb://$DB_USER:PASSWORD@$DB_HOST:443/$DATABASE?loadBalanced=true&tls=true&authMechanism=SCRAM-SHA-256&retryWrites=false" \
  | gcloud secrets versions add sessions-db-url --data-file=-
```

## 4. Fill in the other secrets

One value per secret in `terraform output secrets`. For instance:

```sh
printf '%s' "$(openssl rand -hex 32)" | gcloud secrets versions add forma-agent-key --data-file=-
printf '%s' "sk-..." | gcloud secrets versions add openai-api-key --data-file=-
```

`printf` (rather than `echo`) keeps a newline out of the value.

## 5. Create the service

Set `deploy = true` in `terraform.tfvars`, and apply again:

```sh
terraform apply
terraform output url
```

That address is your agent. What to do with it depends on its client: check
"Talk to it" in the guide.

## Updating the agent

Compile it again (`forma expand -c > agent.yaml`) and `terraform apply`. The
new files are uploaded, and a new revision starts, which reads them. The same
goes for `runtime.yaml`.

To move to a newer Forma, change `forma_version`, update your CLI to the same
version, compile, and apply.

## Workspaces, knowledge bases and code execution

With `workspaces = true`, Terraform creates a bucket, gives the service its name
(`WORKSPACES_BUCKET`), and lets the agent read and write it, and sign URLs for
its files. Tell Forma, in your agent's `runtime.yaml`:

```yaml
workspaces:
  store: gcs
```

Knowledge bases (`google-knowledgebase`, `google-workspace` tools) that name
another `bucket` need the agent to be able to read it: list those buckets in
`knowledge_buckets`.

With `code_execution = true` (it needs `workspaces`), Terraform creates the
sandbox: a Cloud Run Job that runs `ghcr.io/germolinal/forma-sandbox` (the same
`forma_version`), as a service account with no roles, on a network without
internet, and lets the agent start it. Tell Forma:

```yaml
code_execution:
  sandbox: cloud-run-job
```

Then `terraform apply`: the new `runtime.yaml` is uploaded with the rest.

## Removing it

```sh
terraform destroy
```

The database is left behind on purpose (`deletion_policy = "ABANDON"`), so a
destroy can't take your conversations with it. Delete it in the console, or
with `gcloud firestore databases delete --database=forma-db`, if you mean to.
Deleting the whole project removes everything.
