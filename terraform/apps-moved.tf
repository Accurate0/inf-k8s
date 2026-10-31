moved {
  from = vault_database_secret_backend_static_role.database["ai-gateway"]
  to   = module.apps.vault_database_secret_backend_static_role.ai-gateway
}

moved {
  from = vault_policy.database["ai-gateway"]
  to   = module.apps.vault_policy.ai-gateway
}

moved {
  from = vault_kubernetes_auth_backend_role.database["ai-gateway"]
  to   = module.apps.vault_kubernetes_auth_backend_role.ai-gateway
}

moved {
  from = vault_database_secret_backend_static_role.database["bom"]
  to   = module.apps.vault_database_secret_backend_static_role.bom
}

moved {
  from = vault_policy.database["bom"]
  to   = module.apps.vault_policy.bom
}

moved {
  from = vault_kubernetes_auth_backend_role.database["bom"]
  to   = module.apps.vault_kubernetes_auth_backend_role.bom
}

moved {
  from = vault_database_secret_backend_static_role.database["feature-flags"]
  to   = module.apps.vault_database_secret_backend_static_role.feature-flags
}

moved {
  from = vault_policy.database["feature-flags"]
  to   = module.apps.vault_policy.feature-flags
}

moved {
  from = vault_kubernetes_auth_backend_role.database["feature-flags"]
  to   = module.apps.vault_kubernetes_auth_backend_role.feature-flags
}

moved {
  from = vault_database_secret_backend_static_role.database["home-gateway"]
  to   = module.apps.vault_database_secret_backend_static_role.home-gateway
}

moved {
  from = vault_policy.database["home-gateway"]
  to   = module.apps.vault_policy.home-gateway
}

moved {
  from = vault_kubernetes_auth_backend_role.database["home-gateway"]
  to   = module.apps.vault_kubernetes_auth_backend_role.home-gateway
}

moved {
  from = vault_database_secret_backend_static_role.database["registry"]
  to   = module.apps.vault_database_secret_backend_static_role.registry
}

moved {
  from = vault_policy.database["registry"]
  to   = module.apps.vault_policy.registry
}

moved {
  from = vault_kubernetes_auth_backend_role.database["registry"]
  to   = module.apps.vault_kubernetes_auth_backend_role.registry
}

moved {
  from = vault_database_secret_backend_static_role.database["waf-manager"]
  to   = module.apps.vault_database_secret_backend_static_role.waf-manager
}

moved {
  from = vault_policy.database["waf-manager"]
  to   = module.apps.vault_policy.waf-manager
}

moved {
  from = vault_kubernetes_auth_backend_role.database["waf-manager"]
  to   = module.apps.vault_kubernetes_auth_backend_role.waf-manager
}
