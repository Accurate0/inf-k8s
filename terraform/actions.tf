locals {
  repos = toset([
    "replybot",
    "tldr-bot",
    "ozb",
    "maccas-api",
    "anurag.sh",
    "bom",
    "inf-k8s",
    "solar-panels",
    "home-gateway",
  ])

  janitor_bot_webhook_repos = toset([
    "tldr-bot",
    "ozb",
    "maccas-api",
    "anurag.sh",
    "bom",
    "inf-k8s",
    "solar-panels",
    "home-gateway",
  ])
}

module "forgejo-ci-image-updater-appkey" {
  source      = "./modules/keyvault-value-output"
  secret_name = "forgejo-ci-image-updater"
}

module "forgejo-renovate-token" {
  source      = "./modules/keyvault-value-output"
  secret_name = "forgejo-renovate-token"
}

module "janitor-bot-github-webhook-secret" {
  source      = "./modules/keyvault-value-output"
  secret_name = "janitor-bot-github-webhook-secret"
}

resource "github_repository_webhook" "janitor-bot" {
  for_each   = local.janitor_bot_webhook_repos
  repository = each.value
  events     = ["workflow_run", "check_run", "check_suite", "status", "push"]
  active     = true

  configuration {
    url          = "https://janitor-bot.anurag.sh/github/webhook"
    content_type = "json"
    insecure_ssl = false
    secret       = module.janitor-bot-github-webhook-secret.secret_value
  }
}

resource "github_actions_secret" "ci-image-updater" {
  for_each    = local.repos
  repository  = each.value
  secret_name = "FORGEJO_CI_IMAGE_UPDATER"
  value       = module.forgejo-ci-image-updater-appkey.secret_value
}

resource "github_actions_secret" "forgejo-renovate-token" {
  repository  = "inf-k8s"
  secret_name = "FORGEJO_RENOVATE_TOKEN"
  value       = module.forgejo-renovate-token.secret_value
}
