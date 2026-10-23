# Optional (var.workspaces): a bucket for workspaces, scrapbooks and knowledge
# bases, which runtime.yaml uses with `workspaces: store: gcs`. The service
# finds it through WORKSPACES_BUCKET (main.tf).

resource "google_storage_bucket" "workspaces" {
  count = var.workspaces ? 1 : 0

  name     = "workspaces-${data.google_project.this.number}"
  location = var.region

  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"

  # Code runs keep their files under runs/ for the agent to read back; whatever
  # is worth keeping is copied into a workspace. A day is the shortest age a
  # rule can express, and the prefix keeps it away from everything else.
  lifecycle_rule {
    condition {
      age            = 1
      matches_prefix = ["runs/"]
    }
    action {
      type = "Delete"
    }
  }

  depends_on = [google_project_service.apis]
}

# Read and write, on this bucket only (not every bucket in the project).
resource "google_storage_bucket_iam_member" "workspaces" {
  count = var.workspaces ? 1 : 0

  bucket = google_storage_bucket.workspaces[0].name
  role   = "roles/storage.objectUser"
  member = "serviceAccount:${google_service_account.agent.email}"
}

# Read only, on the knowledge buckets you name.
resource "google_storage_bucket_iam_member" "knowledge" {
  for_each = toset(var.knowledge_buckets)

  bucket = each.value
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.agent.email}"
}

# Forma signs short-lived URLs for files (downloads, and code runs' inputs and
# outputs) as the service account itself, through the IAM Credentials API, so
# no key ever exists. This lets it sign as itself, and as nobody else.
resource "google_service_account_iam_member" "self_signing" {
  count = var.workspaces ? 1 : 0

  service_account_id = google_service_account.agent.name
  role               = "roles/iam.serviceAccountTokenCreator"
  member             = "serviceAccount:${google_service_account.agent.email}"
}
