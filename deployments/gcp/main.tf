# One Forma agent on Cloud Run: its service account, the database that keeps
# its conversations, its secrets, and the service. Workspaces and code
# execution are in their own files, and off by default.
#
# Nothing is built here. The service runs Forma's published image
# (ghcr.io/germolinal/forma), and reads your compiled agent and its runtime
# file from a small bucket mounted into it.

data "google_project" "this" {}

locals {
  apis = concat(
    [
      "run.googleapis.com",
      "secretmanager.googleapis.com",
      "firestore.googleapis.com",
      "storage.googleapis.com",
      "iam.googleapis.com",
    ],
    var.vertex_ai ? ["aiplatform.googleapis.com"] : [],
    var.workspaces ? ["iamcredentials.googleapis.com"] : [],
    var.code_execution ? ["compute.googleapis.com"] : [],
  )

  database_location = var.database_location != "" ? var.database_location : var.region

  # Where the agent's bucket is mounted in the container.
  agent_dir = "/agent"

  # The variables every revision gets, besides the secrets.
  service_env = merge(
    {
      GOOGLE_CLOUD_PROJECT  = var.project
      GOOGLE_CLOUD_LOCATION = var.region
      # Changes whenever either file does, so that uploading a new agent starts
      # a new revision, which reads it. (The server reads both files once, when
      # it starts.)
      FORMA_AGENT_FILES = substr(sha256("${filesha256(var.agent_file)}${filesha256(var.runtime_file)}"), 0, 16)
    },
    var.workspaces ? { WORKSPACES_BUCKET = google_storage_bucket.workspaces[0].name } : {},
    var.code_execution ? { CODE_EXECUTION_JOB = google_cloud_run_v2_job.code_execution[0].id } : {},
    var.env,
  )
}

resource "google_project_service" "apis" {
  for_each = toset(local.apis)

  service            = each.key
  disable_on_destroy = false
}

# ---- identity -----------------------------------------------------------------

# What the service runs as. It gets only what these files grant it.
resource "google_service_account" "agent" {
  account_id   = var.service
  display_name = "Forma agent (${var.service})"
  depends_on   = [google_project_service.apis]
}

resource "google_project_iam_member" "vertex_ai" {
  count = var.vertex_ai ? 1 : 0

  project = var.project
  role    = "roles/aiplatform.user"
  member  = "serviceAccount:${google_service_account.agent.email}"
}

# ---- the agent's files --------------------------------------------------------

# Your compiled agent (`forma expand -c > agent.yaml`) and its runtime.yaml.
# Neither holds secrets: those stay environment variables, below.
resource "google_storage_bucket" "agent" {
  name     = "${var.service}-agent-${data.google_project.this.number}"
  location = var.region

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = true

  depends_on = [google_project_service.apis]
}

resource "google_storage_bucket_object" "agent" {
  bucket = google_storage_bucket.agent.name
  name   = "agent.yaml"
  source = var.agent_file
}

resource "google_storage_bucket_object" "runtime" {
  bucket = google_storage_bucket.agent.name
  name   = "runtime.yaml"
  source = var.runtime_file
}

resource "google_storage_bucket_iam_member" "agent_reads_its_files" {
  bucket = google_storage_bucket.agent.name
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.agent.email}"
}

# ---- the database -------------------------------------------------------------

# Firestore with MongoDB compatibility (Enterprise edition). Forma connects to
# it as to MongoDB, with a username and password: see "Connect the database" in
# README.md, which also covers why that user is not created here.
resource "google_firestore_database" "forma" {
  provider = google-beta

  name             = var.database
  location_id      = local.database_location
  type             = "FIRESTORE_NATIVE"
  database_edition = "ENTERPRISE"

  # A `terraform destroy` forgets the database instead of deleting it, so your
  # conversations survive it. Delete it by hand if you mean to.
  deletion_policy = "ABANDON"

  depends_on = [google_project_service.apis]
}

# The queries Forma sends. Firestore creates no index by itself, and without
# them every message reads the whole collection.

# A conversation's messages, newest first: read on every message.
resource "google_firestore_index" "messages_by_session" {
  database    = google_firestore_database.forma.name
  collection  = "messages"
  api_scope   = "MONGODB_COMPATIBLE_API"
  query_scope = "COLLECTION_GROUP"
  density     = "DENSE"

  fields {
    field_path = "session_id"
    order      = "ASCENDING"
  }
  fields {
    field_path = "created_at"
    order      = "DESCENDING"
  }

  timeouts {
    create = "60m"
  }
}

# One message, by its id: reactions and delivery statuses.
resource "google_firestore_index" "messages_by_id" {
  database    = google_firestore_database.forma.name
  collection  = "messages"
  api_scope   = "MONGODB_COMPATIBLE_API"
  query_scope = "COLLECTION_GROUP"
  density     = "DENSE"

  fields {
    field_path = "id"
    order      = "ASCENDING"
  }
  fields {
    field_path = "session_id"
    order      = "ASCENDING"
  }

  timeouts {
    create = "60m"
  }
}

# A user's conversations with the agent, newest first.
resource "google_firestore_index" "sessions_by_user" {
  database    = google_firestore_database.forma.name
  collection  = "sessions"
  api_scope   = "MONGODB_COMPATIBLE_API"
  query_scope = "COLLECTION_GROUP"
  density     = "DENSE"

  fields {
    field_path = "user_id"
    order      = "ASCENDING"
  }
  fields {
    field_path = "agent_id"
    order      = "ASCENDING"
  }
  fields {
    field_path = "created_at"
    order      = "DESCENDING"
  }
}

# ---- secrets ------------------------------------------------------------------

# Only the containers: their values are added with gcloud (README.md), so they
# never reach the Terraform state.
resource "google_secret_manager_secret" "env" {
  for_each = var.secrets

  secret_id = each.value

  replication {
    auto {}
  }

  depends_on = [google_project_service.apis]
}

resource "google_secret_manager_secret_iam_member" "env" {
  for_each = var.secrets

  secret_id = google_secret_manager_secret.env[each.key].id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.agent.email}"
}

# ---- the service --------------------------------------------------------------

# Created once `deploy` is true: a revision that reads a secret with no value
# fails to start, so the secrets are filled in first.
resource "google_cloud_run_v2_service" "agent" {
  count = var.deploy ? 1 : 0

  name                = var.service
  location            = var.region
  deletion_protection = false
  ingress             = "INGRESS_TRAFFIC_ALL"

  template {
    service_account = google_service_account.agent.email
    # Cloud Storage volumes need the second generation.
    execution_environment = "EXECUTION_ENVIRONMENT_GEN2"

    # An answer can take minutes (tools, long documents, code runs). Cloud
    # Run's default (5 minutes) cuts those off; an hour is the most it allows.
    timeout = "3600s"

    scaling {
      min_instance_count = 0
      max_instance_count = var.max_instances
    }

    volumes {
      name = "agent"
      gcs {
        bucket    = google_storage_bucket.agent.name
        read_only = true
      }
    }

    containers {
      # Forma's server, as published: nothing of yours is built into it.
      image   = "ghcr.io/germolinal/forma:${var.forma_version}"
      command = ["/app/forma"]
      args = [
        "serve",
        "-t", "otel",
        "-a", "${local.agent_dir}/agent.yaml",
        "-r", "${local.agent_dir}/runtime.yaml",
      ]

      ports {
        container_port = 8080
      }

      volume_mounts {
        name       = "agent"
        mount_path = local.agent_dir
      }

      dynamic "env" {
        for_each = local.service_env
        content {
          name  = env.key
          value = env.value
        }
      }

      dynamic "env" {
        for_each = var.secrets
        content {
          name = env.key
          value_source {
            secret_key_ref {
              secret  = google_secret_manager_secret.env[env.key].secret_id
              version = "latest"
            }
          }
        }
      }
    }
  }

  # Cloud Run refuses a revision whose identity cannot read its secrets, and
  # the files must be in the bucket before the server starts.
  depends_on = [
    google_secret_manager_secret_iam_member.env,
    google_storage_bucket_iam_member.agent_reads_its_files,
    google_storage_bucket_object.agent,
    google_storage_bucket_object.runtime,
  ]
}

# Clients reach the service directly. It is still protected: by the API key
# (sync, sse, ai-sdk-v5) or by the platform's signature (whatsapp, telegram).
resource "google_cloud_run_v2_service_iam_member" "public" {
  count = var.deploy && var.public ? 1 : 0

  name     = google_cloud_run_v2_service.agent[0].name
  location = var.region
  role     = "roles/run.invoker"
  member   = "allUsers"
}
