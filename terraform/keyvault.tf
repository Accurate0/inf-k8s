data "azurerm_client_config" "current" {}

data "azurerm_resource_group" "general-api-group" {
  name = "general-api-group"
}

resource "azurerm_key_vault" "k8s-shared-vault" {
  name                       = "k8s-shared-vault"
  location                   = data.azurerm_resource_group.general-api-group.location
  resource_group_name        = data.azurerm_resource_group.general-api-group.name
  tenant_id                  = data.azurerm_client_config.current.tenant_id
  sku_name                   = "standard"
  rbac_authorization_enabled = false
  purge_protection_enabled   = true

  # the identity running terraform manages secrets in the vault
  access_policy {
    tenant_id = data.azurerm_client_config.current.tenant_id
    object_id = data.azurerm_client_config.current.object_id

    key_permissions = ["Get", "List", "Create", "Delete", "Purge", "Recover", "GetRotationPolicy"]

    secret_permissions = ["Get", "List", "Set", "Delete", "Purge", "Recover"]
  }

  # my interactive access
  access_policy {
    tenant_id = data.azurerm_client_config.current.tenant_id
    object_id = "47245261-7eb5-4a37-9d1a-a3805201ddde" # me

    key_permissions = ["Get", "List", "Create", "Delete"]

    secret_permissions = ["Get", "List", "Set", "Delete", "Recover", "Restore"]
  }

  # external-secrets-operator reads secrets via its service principal
  access_policy {
    tenant_id = data.azurerm_client_config.current.tenant_id
    object_id = azuread_service_principal.eso-shared-vault.object_id

    secret_permissions = ["Get", "List"]
  }

  access_policy {
    tenant_id = data.azurerm_client_config.current.tenant_id
    object_id = azuread_service_principal.openbao-unseal.object_id

    key_permissions = ["Get", "WrapKey", "UnwrapKey"]
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
