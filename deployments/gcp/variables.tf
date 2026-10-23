variable "project" {
  description = "The Google Cloud project's ID."
  type        = string
}

variable "region" {
  description = "Where the service, the registry and the buckets are."
  type        = string
  default     = "us-central1"
}

variable "service" {
  description = "The name of the Cloud Run service, and of the service account it runs as."
  type        = string
  default     = "forma-agent"
}

variable "forma_version" {
  description = <<-EOT
    The version of Forma's published images (ghcr.io/germolinal/forma and
    forma-sandbox). Pin it to the one you develop with (`forma --version`).
  EOT
  type        = string
  default     = "latest"
}

variable "agent_file" {
  description = "Your compiled agent: the output of `forma expand -c > agent.yaml`, in your agent's project."
  type        = string
  default     = "../../agent.yaml"
}

variable "runtime_file" {
  description = "Your agent's runtime.yaml. It must say `storage: mongo`."
  type        = string
  default     = "../../runtime.yaml"
}

variable "deploy" {
  description = <<-EOT
    Create the service. Leave it false on the first apply: a service whose
    secrets have no values yet fails to start. Set it to true once they do.
  EOT
  type        = bool
  default     = false
}

# ---- the database -------------------------------------------------------------

variable "database" {
  description = "The Firestore (MongoDB compatible) database that keeps conversations. Keep `forma-db`, the name Forma uses."
  type        = string
  default     = "forma-db"
}

variable "database_location" {
  description = "Where the database is. Empty means the region; a multi-region (nam5, eur3) is also valid."
  type        = string
  default     = ""
}

# ---- secrets and settings -----------------------------------------------------

variable "secrets" {
  description = <<-EOT
    Environment variables read from Secret Manager, as VARIABLE = secret name.
    Terraform creates the (empty) secrets and lets the service read them; you
    add their values with gcloud, so no secret ever reaches the Terraform state.
    SESSIONS_DB_URL is needed always; add your client's (e.g., FORMA_AGENT_KEY,
    TELEGRAM_BOT_TOKEN) and your agent's (e.g., OPENAI_API_KEY).
  EOT
  type        = map(string)
  default = {
    SESSIONS_DB_URL = "sessions-db-url"
    FORMA_AGENT_KEY = "forma-agent-key"
  }
}

variable "env" {
  description = "Plain (not secret) environment variables for the service."
  type        = map(string)
  default     = {}
}

variable "vertex_ai" {
  description = "Let the agent use Google's models (vertex-ai) and other Vertex AI services, as its service account: no API key."
  type        = bool
  default     = true
}

# ---- optional: workspaces and knowledge bases ---------------------------------

variable "workspaces" {
  description = <<-EOT
    Create a bucket for workspaces, scrapbooks and knowledge bases (`workspaces:
    store: gcs` in runtime.yaml), and let the agent read and write it.
  EOT
  type        = bool
  default     = false
}

variable "knowledge_buckets" {
  description = "Other, existing buckets the agent reads documents from (e.g., a `google-knowledgebase` tool's `bucket`)."
  type        = list(string)
  default     = []
}

# ---- optional: code execution -------------------------------------------------

variable "code_execution" {
  description = <<-EOT
    Create the Cloud Run Job that runs the agent's code (`code_execution:
    sandbox: cloud-run-job` in runtime.yaml), on a network without internet.
    Needs `workspaces`.
  EOT
  type        = bool
  default     = false
}

# ---- the service --------------------------------------------------------------

variable "max_instances" {
  description = "The most copies of the service that may run at once."
  type        = number
  default     = 2
}

variable "public" {
  description = <<-EOT
    Let anyone on the internet reach the service. Needed for WhatsApp, Telegram
    and web clients; the service still checks the API key, or the platform's
    signature, on every request.
  EOT
  type        = bool
  default     = true
}
