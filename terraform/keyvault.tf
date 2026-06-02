data "azurerm_client_config" "current" {}

data "azurerm_resource_group" "general-api-group" {
  name = "general-api-group"
}

resource "azurerm_key_vault" "k8s-shared-vault" {
  name                = "k8s-shared-vault"
  location            = data.azurerm_resource_group.general-api-group.location
  resource_group_name = data.azurerm_resource_group.general-api-group.name
  tenant_id           = data.azurerm_client_config.current.tenant_id
  sku_name            = "standard"

  # the identity running terraform manages secrets in the vault
  access_policy {
    tenant_id = data.azurerm_client_config.current.tenant_id
    object_id = data.azurerm_client_config.current.object_id

    secret_permissions = ["Get", "List", "Set", "Delete", "Purge", "Recover"]
  }

  # external-secrets-operator reads secrets via its service principal
  access_policy {
    tenant_id = data.azurerm_client_config.current.tenant_id
    object_id = azuread_service_principal.eso-shared-vault.object_id

    secret_permissions = ["Get", "List"]
  }
}

# service principal external-secrets uses to authenticate to the vault
resource "azuread_application" "eso-shared-vault" {
  display_name = "eso-k8s-shared-vault"
}

resource "azuread_service_principal" "eso-shared-vault" {
  client_id = azuread_application.eso-shared-vault.client_id
}

resource "azuread_application_password" "eso-shared-vault" {
  application_id = azuread_application.eso-shared-vault.id
}

# pre-existing infisical project (slug external-secrets-u2b0) that holds the
# service principal credentials ESO pulls into the cluster
locals {
  infisical_external_secrets_project_id = "cda01656-1403-4d33-aed0-1adda3ca43ea"
}

# TODO: FIXME
# resource "infisical_secret" "azure-client-id" {
#   name         = "AZURE_CLIENT_ID"
#   value        = azuread_application.eso-shared-vault.client_id
#   env_slug     = "prod"
#   workspace_id = local.infisical_external_secrets_project_id
#   folder_path  = "/"
# }
#
# resource "infisical_secret" "azure-client-secret" {
#   name         = "AZURE_CLIENT_SECRET"
#   value        = azuread_application_password.eso-shared-vault.value
#   env_slug     = "prod"
#   workspace_id = local.infisical_external_secrets_project_id
#   folder_path  = "/"
# }
