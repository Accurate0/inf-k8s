locals {
  cloudflare_account_id = "45c8799279b4d50c2126b6188d93045c"
}

data "cloudflare_account_api_token_permission_groups_list" "r2_write" {
  account_id = local.cloudflare_account_id
  name       = "Workers R2 Storage Bucket Item Write"
  scope      = "com.cloudflare.edge.r2.bucket"
}

module "apps" {
  source = "./apps"

  database_mount            = vault_database_secrets_mount.database.path
  database_name             = vault_database_secrets_mount.database.postgresql[0].name
  kubernetes_auth_path      = vault_auth_backend.kubernetes.path
  kv_mount                  = vault_mount.kv.path
  cloudflare_account_id     = local.cloudflare_account_id
  r2_write_permission_group = data.cloudflare_account_api_token_permission_groups_list.r2_write.result[0].id
}
