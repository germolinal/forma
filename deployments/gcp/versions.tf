terraform {
  required_version = ">= 1.5"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.0"
    }
    google-beta = {
      source  = "hashicorp/google-beta"
      version = "~> 6.0"
    }
  }

  # State is kept in a local file (terraform.tfstate), which is fine for one
  # person. To share it, or keep it off your laptop, use a bucket instead:
  #
  #   backend "gcs" {
  #     bucket = "my-terraform-state"
  #     prefix = "forma"
  #   }
}

provider "google" {
  project = var.project
  region  = var.region
}

provider "google-beta" {
  project = var.project
  region  = var.region
}
