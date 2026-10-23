# Optional (var.code_execution): the Cloud Run Job that runs the agent's code,
# one execution per run, which runtime.yaml uses with `code_execution: sandbox:
# cloud-run-job`. The service finds it through CODE_EXECUTION_JOB (main.tf).
#
# What makes it safe to run code an LLM wrote:
#
# - The job runs as a service account with NO roles. Everything a run can touch
#   comes from URLs the agent signs for that one run (its inputs, its logs, its
#   own out/ folder), which expire.
# - The job's traffic goes into a network with no Cloud NAT, so it has no
#   internet. Private Google Access keeps storage.googleapis.com reachable for
#   those URLs. (Other Google APIs are reachable too; closing that needs VPC
#   Service Controls.)
# - Nothing the code prints is kept in Cloud Logging.

check "code_execution_needs_workspaces" {
  assert {
    condition     = !var.code_execution || var.workspaces
    error_message = "code_execution needs workspaces: runs keep their files in the workspaces bucket."
  }
}

locals {
  code_execution_job = "${var.service}-code-execution"
}

resource "google_service_account" "sandbox" {
  count = var.code_execution ? 1 : 0

  account_id   = "${var.service}-sandbox"
  display_name = "Forma code-execution sandbox (no roles, on purpose)"
  depends_on   = [google_project_service.apis]
}

# ---- a network without internet -----------------------------------------------

resource "google_compute_network" "sandbox" {
  count = var.code_execution ? 1 : 0

  name                    = "${var.service}-sandbox"
  auto_create_subnetworks = false
  depends_on              = [google_project_service.apis]
}

# No router and no Cloud NAT in this network, ever: that is what keeps runs off
# the internet.
resource "google_compute_subnetwork" "sandbox" {
  count = var.code_execution ? 1 : 0

  name                     = "${var.service}-sandbox"
  region                   = var.region
  network                  = google_compute_network.sandbox[0].id
  ip_cidr_range            = "10.80.0.0/24"
  private_ip_google_access = true
}

# ---- the job ------------------------------------------------------------------

resource "google_cloud_run_v2_job" "code_execution" {
  count = var.code_execution ? 1 : 0

  name                = local.code_execution_job
  location            = var.region
  deletion_protection = false

  template {
    task_count  = 1
    parallelism = 1

    template {
      service_account = google_service_account.sandbox[0].email
      # The agent overrides it per run: a command gets 5 minutes, and the rest
      # is for uploading its results.
      timeout = "360s"
      # A failed run is reported to the agent, never retried behind its back.
      max_retries = 0

      containers {
        # Forma's published sandbox, the same version as the server.
        image = "ghcr.io/germolinal/forma-sandbox:${var.forma_version}"

        resources {
          limits = {
            cpu    = "1"
            memory = "2Gi"
          }
        }
      }

      vpc_access {
        network_interfaces {
          network    = google_compute_network.sandbox[0].id
          subnetwork = google_compute_subnetwork.sandbox[0].id
        }
        egress = "ALL_TRAFFIC"
      }
    }
  }

  depends_on = [google_project_service.apis]
}

# ---- who may do what ----------------------------------------------------------

# The agent starts the job, with a per-run manifest and timeout. On this job only.
resource "google_cloud_run_v2_job_iam_member" "agent_runs" {
  count = var.code_execution ? 1 : 0

  name     = google_cloud_run_v2_job.code_execution[0].name
  location = var.region
  role     = "roles/run.jobsExecutorWithOverrides"
  member   = "serviceAccount:${google_service_account.agent.email}"
}

# And follows the execution, which the role above leaves out. Read only, on
# this job only.
resource "google_cloud_run_v2_job_iam_member" "agent_follows" {
  count = var.code_execution ? 1 : 0

  name     = google_cloud_run_v2_job.code_execution[0].name
  location = var.region
  role     = "roles/run.viewer"
  member   = "serviceAccount:${google_service_account.agent.email}"
}

# ---- logs ---------------------------------------------------------------------

# What the code prints is the user's data: keep it out of Cloud Logging. The
# platform's own logs for the job (start, exit, out of memory) are kept.
resource "google_logging_project_exclusion" "sandbox_output" {
  count = var.code_execution ? 1 : 0

  name        = "${var.service}-sandbox-output"
  description = "Keeps code runs' stdout and stderr out of Cloud Logging"
  filter      = "resource.type=\"cloud_run_job\" AND resource.labels.job_name=\"${local.code_execution_job}\" AND (log_id(\"run.googleapis.com/stdout\") OR log_id(\"run.googleapis.com/stderr\"))"
}
