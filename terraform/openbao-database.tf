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
    allowed_roles           = module.apps.database_names
  }
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

moved {
  from = vault_database_secret_backend_static_role.database["dawarich"]
  to   = module.apps.vault_database_secret_backend_static_role.dawarich
}

moved {
  from = vault_policy.database["dawarich"]
  to   = module.apps.vault_policy.dawarich
}

moved {
  from = vault_kubernetes_auth_backend_role.database["dawarich"]
  to   = module.apps.vault_kubernetes_auth_backend_role.dawarich
}

moved {
  from = vault_database_secret_backend_static_role.database["forgejo"]
  to   = module.apps.vault_database_secret_backend_static_role.forgejo
}

moved {
  from = vault_policy.database["forgejo"]
  to   = module.apps.vault_policy.forgejo
}

moved {
  from = vault_kubernetes_auth_backend_role.database["forgejo"]
  to   = module.apps.vault_kubernetes_auth_backend_role.forgejo
}

moved {
  from = vault_database_secret_backend_static_role.database["firefly"]
  to   = module.apps.vault_database_secret_backend_static_role.firefly
}

moved {
  from = vault_policy.database["firefly"]
  to   = module.apps.vault_policy.firefly
}

moved {
  from = vault_kubernetes_auth_backend_role.database["firefly"]
  to   = module.apps.vault_kubernetes_auth_backend_role.firefly
}

moved {
  from = vault_database_secret_backend_static_role.database["immich"]
  to   = module.apps.vault_database_secret_backend_static_role.immich
}

moved {
  from = vault_policy.database["immich"]
  to   = module.apps.vault_policy.immich
}

moved {
  from = vault_kubernetes_auth_backend_role.database["immich"]
  to   = module.apps.vault_kubernetes_auth_backend_role.immich
}
