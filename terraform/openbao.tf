resource "azuread_application" "openbao-unseal" {
  display_name = "openbao-unseal"
}

resource "azuread_service_principal" "openbao-unseal" {
  client_id = azuread_application.openbao-unseal.client_id
}

resource "azuread_application_password" "openbao-unseal" {
  application_id = azuread_application.openbao-unseal.id
}

resource "azurerm_key_vault_key" "openbao-unseal" {
  name         = "openbao-unseal"
  key_vault_id = azurerm_key_vault.k8s-shared-vault.id
  key_type     = "RSA"
  key_size     = 2048
  key_opts     = ["wrapKey", "unwrapKey"]
}

resource "azurerm_key_vault_secret" "openbao-azure-client-id" {
  name         = "openbao-azure-client-id"
  value        = azuread_application.openbao-unseal.client_id
  key_vault_id = azurerm_key_vault.k8s-shared-vault.id
}

resource "azurerm_key_vault_secret" "openbao-azure-client-secret" {
  name         = "openbao-azure-client-secret"
  value        = azuread_application_password.openbao-unseal.value
  key_vault_id = azurerm_key_vault.k8s-shared-vault.id
}
