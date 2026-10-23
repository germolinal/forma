output "agent_bucket" {
  description = "The bucket holding agent.yaml and runtime.yaml, mounted into the service."
  value       = google_storage_bucket.agent.name
}

output "service_account" {
  description = "What the service runs as."
  value       = google_service_account.agent.email
}

output "database_host" {
  description = "The database's host, for SESSIONS_DB_URL."
  value       = "${google_firestore_database.forma.uid}.${local.database_location}.firestore.goog"
}

output "secrets" {
  description = "The secrets to fill in with gcloud, by environment variable."
  value       = var.secrets
}

output "workspaces_bucket" {
  description = "The workspaces bucket, if any."
  value       = var.workspaces ? google_storage_bucket.workspaces[0].name : null
}

output "url" {
  description = "The agent's address, once the service exists."
  value       = var.deploy ? google_cloud_run_v2_service.agent[0].uri : null
}
