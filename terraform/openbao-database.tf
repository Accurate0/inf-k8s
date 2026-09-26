resource "random_password" "openbao-db-admin" {
  length  = 48
  special = false
}

resource "azurerm_key_vault_secret" "openbao-db-admin" {
  name = "openbao-db-admin"
  value = jsonencode({
    username = "openbao_db_admin"
    password = random_password.openbao-db-admin.result
  })
  key_vault_id = azurerm_key_vault.k8s-shared-vault.id
}

resource "vault_database_secrets_mount" "database" {
  path = "database"

  postgresql {
    name                    = "postgresql"
    connection_url          = "postgresql://{{username}}:{{password}}@cloudnative-pg-cluster-rw.cnpg-system:5432/postgres"
    username                = "openbao_db_admin"
    password                = random_password.openbao-db-admin.result
    password_authentication = "scram-sha-256"
    allowed_roles           = keys(local.databases)
  }
}

resource "vault_database_secret_backend_static_role" "database" {
  for_each = local.databases

  backend         = vault_database_secrets_mount.database.path
  name            = each.key
  db_name         = vault_database_secrets_mount.database.postgresql[0].name
  username        = each.value.role
  rotation_period = 315360000
}

resource "vault_policy" "database" {
  for_each = local.databases

  name   = "db-${each.key}"
  policy = <<-EOT
    path "${vault_database_secrets_mount.database.path}/static-creds/${each.key}" {
      capabilities = ["read"]
    }
  EOT
}

resource "vault_kubernetes_auth_backend_role" "database" {
  for_each = local.databases

  backend                          = vault_auth_backend.kubernetes.path
  role_name                        = "db-${each.key}"
  bound_service_account_names      = ["openbao-database"]
  bound_service_account_namespaces = [each.value.namespace]
  token_policies                   = [vault_policy.database[each.key].name]
  token_ttl                        = 3600
}

resource "random_password" "openbao-database" {
  length  = 48
  special = false
}

resource "azurerm_key_vault_secret" "openbao-database" {
  name = "openbao-database"
  value = jsonencode({
    username = "openbao"
    password = random_password.openbao-database.result
  })
  key_vault_id = azurerm_key_vault.k8s-shared-vault.id
}

locals {
  databases = {
    bom           = { namespace = "bom", role = "bom" }
    home-gateway  = { namespace = "home-gateway", role = "home_gateway" }
    waf-manager   = { namespace = "waf-manager", role = "waf_manager" }
    feature-flags = { namespace = "feature-flags", role = "feature_flags" }
    ai-gateway    = { namespace = "ai-gateway", role = "ai_gateway" }
    dawarich      = { namespace = "dawarich", role = "dawarich_2" }
    forgejo       = { namespace = "forgejo", role = "forgejo" }
  }
}
