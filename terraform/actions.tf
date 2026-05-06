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

resource "github_actions_secret" "ci-image-updater" {
  for_each    = local.repos
  repository  = each.value
  secret_name = "FORGEJO_CI_IMAGE_UPDATER"
  value       = module.forgejo-ci-image-updater-appkey.secret_value
}
