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
}

module "forgejo-ci-image-updater-appkey" {
  source      = "./modules/keyvault-value-output"
  secret_name = "forgejo-ci-image-updater"
}

module "forgejo-renovate-token" {
  source      = "./modules/keyvault-value-output"
  secret_name = "forgejo-renovate-token"
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
