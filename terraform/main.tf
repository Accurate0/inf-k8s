terraform {
  required_providers {
    binarylane = {
      source  = "oscarhermoso/binarylane"
      version = "~> 0.10"
    }

    azurerm = {
      source  = "hashicorp/azurerm"
      version = ">= 4"
    }

    tls = {
      source  = "hashicorp/tls"
      version = ">= 4"
    }

    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = ">= 4"
    }

    aws = {
      source  = "hashicorp/aws"
      version = ">= 5"
    }

    github = {
      source  = "integrations/github"
      version = ">= 5.22.0"
    }

    infisical = {
      source  = "Infisical/infisical"
      version = "0.15.55"
    }
  }

  backend "s3" {
    key = "k8s/terraform.tfstate"
  }
}

provider "binarylane" {}
provider "azurerm" {
  features {}
}

variable "infisical_client_id" {
  type     = string
  nullable = false
}

variable "infisical_client_secret" {
  type     = string
  nullable = false
}

provider "infisical" {
  host = "https://vault.inf-k8s.net"
  auth = {
    universal = {
      client_id     = var.infisical_client_id
      client_secret = var.infisical_client_secret
    }
  }
}
