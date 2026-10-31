terraform {
  required_providers {
    aws = {
      source = "hashicorp/aws"
    }

    cloudflare = {
      source = "cloudflare/cloudflare"
    }

    vault = {
      source = "hashicorp/vault"
    }
  }
}

variable "database_mount" {
  type = string
}

variable "database_name" {
  type = string
}

variable "kubernetes_auth_path" {
  type = string
}

variable "kv_mount" {
  type = string
}

variable "cloudflare_account_id" {
  type = string
}

variable "r2_write_permission_group" {
  type = string
}
