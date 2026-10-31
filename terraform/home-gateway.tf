resource "github_actions_secret" "aws-key-id" {
  repository  = "home-gateway"
  secret_name = "AWS_ACCESS_KEY_ID"
  value       = module.apps.s3_access_keys["home-gateway-storage"].id
}

resource "github_actions_secret" "aws-key-secret" {
  repository  = "home-gateway"
  secret_name = "AWS_SECRET_ACCESS_KEY"
  value       = module.apps.s3_access_keys["home-gateway-storage"].secret
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
