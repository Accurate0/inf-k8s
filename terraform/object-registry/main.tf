terraform {
  required_providers {
    cloudflare = {
      source = "cloudflare/cloudflare"
    }

    aws = {
      source = "hashicorp/aws"
    }

    infisical = {
      source = "Infisical/infisical"
    }
  }
}

// TODO: use dynamodb and provision keys for each namespace
// keys are used for namespace access
// JWT is only used for full access?
