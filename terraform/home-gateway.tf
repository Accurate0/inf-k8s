data "forgejo_repository" "home-gateway" {
  owner = "anurag"
  name  = "home-gateway"
}

ephemeral "vault_kv_secret_v2" "home-gateway-ci" {
  mount = vault_mount.kv.path
  name  = "home-gateway/ci"
}

locals {
  home_gateway_ci_secrets_version = 1
}

resource "forgejo_repository_action_secret" "home-gateway-aws-key-id" {
  repository_id = data.forgejo_repository.home-gateway.id
  name          = "AWS_ACCESS_KEY_ID"
  data          = module.apps.s3_access_keys["home-gateway-storage"].id
}

resource "forgejo_repository_action_secret" "home-gateway-aws-key-secret" {
  repository_id = data.forgejo_repository.home-gateway.id
  name          = "AWS_SECRET_ACCESS_KEY"
  data          = module.apps.s3_access_keys["home-gateway-storage"].secret
}

resource "forgejo_repository_action_secret" "home-gateway-ci" {
  for_each        = toset(["HOME_GATEWAY_API_KEY", "WIFI_SSID", "WIFI_PASSWORD"])
  repository_id   = data.forgejo_repository.home-gateway.id
  name            = each.value
  data_wo         = ephemeral.vault_kv_secret_v2.home-gateway-ci.data[each.value]
  data_wo_version = local.home_gateway_ci_secrets_version
}

ephemeral "vault_kv_secret_v2" "github-readonly" {
  mount = vault_mount.kv.path
  name  = "github/readonly"
}

resource "forgejo_repository_action_secret" "home-gateway-readonly-gh-token" {
  repository_id   = data.forgejo_repository.home-gateway.id
  name            = "READONLY_GH_TOKEN"
  data_wo         = ephemeral.vault_kv_secret_v2.github-readonly.data["READONLY_GH_TOKEN"]
  data_wo_version = 1
}

resource "google_project_service" "iam" {
  service            = "iam.googleapis.com"
  disable_on_destroy = false
}

resource "google_project_service" "firebase" {
  service            = "firebase.googleapis.com"
  disable_on_destroy = false
}

resource "google_project_service" "fcm" {
  service            = "fcm.googleapis.com"
  disable_on_destroy = false
}

resource "google_service_account" "home-gateway-fcm" {
  account_id   = "home-gateway-fcm"
  display_name = "home-gateway Firebase Cloud Messaging"
}

resource "google_project_iam_member" "home-gateway-fcm" {
  project = "home-gateway-22"
  role    = "roles/firebasecloudmessaging.admin"
  member  = "serviceAccount:${google_service_account.home-gateway-fcm.email}"
}

resource "google_service_account_key" "home-gateway-fcm" {
  service_account_id = google_service_account.home-gateway-fcm.name
}

resource "vault_kv_secret_v2" "home-gateway" {
  mount = vault_mount.kv.path
  name  = "home-gateway/terraform"
  data_json = jsonencode({
    AWS_ACCESS_KEY_ID                         = module.apps.s3_access_keys["home-gateway-storage"].id
    AWS_SECRET_ACCESS_KEY                     = module.apps.s3_access_keys["home-gateway-storage"].secret
    NOTIFY__ANDROID__FCM_SERVICE_ACCOUNT_JSON = base64decode(google_service_account_key.home-gateway-fcm.private_key)
  })
}
