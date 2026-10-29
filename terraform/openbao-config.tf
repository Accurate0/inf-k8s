resource "vault_mount" "kv" {
  path = "kv"
  type = "kv"
  options = {
    version = "2"
  }
}

resource "vault_auth_backend" "kubernetes" {
  type = "kubernetes"
}

resource "vault_kubernetes_auth_backend_config" "kubernetes" {
  backend         = vault_auth_backend.kubernetes.path
  kubernetes_host = "https://kubernetes.default.svc"
}

resource "vault_policy" "eso-read" {
  name   = "eso-read"
  policy = <<-EOT
    path "kv/data/{{identity.entity.aliases.${vault_auth_backend.kubernetes.accessor}.metadata.service_account_namespace}}" {
      capabilities = ["read"]
    }

    path "kv/data/{{identity.entity.aliases.${vault_auth_backend.kubernetes.accessor}.metadata.service_account_namespace}}/*" {
      capabilities = ["read"]
    }
  EOT
}

resource "vault_kubernetes_auth_backend_role" "eso" {
  backend                          = vault_auth_backend.kubernetes.path
  role_name                        = "eso"
  bound_service_account_names      = ["openbao-secrets"]
  bound_service_account_namespaces = ["*"]
  token_policies                   = [vault_policy.eso-read.name]
  token_ttl                        = 3600
}

resource "vault_policy" "admin" {
  name   = "admin"
  policy = <<-EOT
    path "*" {
      capabilities = ["create", "read", "update", "patch", "delete", "list", "sudo"]
    }
  EOT
}

resource "vault_jwt_auth_backend" "oidc" {
  path               = "oidc"
  type               = "oidc"
  oidc_discovery_url = "https://idm.anurag.sh/oauth2/openid/openbao"
  oidc_client_id     = "openbao"
  oidc_client_secret = var.OPENBAO_OIDC_CLIENT_SECRET
  jwt_supported_algs = ["ES256"]
  default_role       = "default"

  tune {
    listing_visibility = "unauth"
    token_type         = "default-service"
  }
}

resource "vault_jwt_auth_backend_role" "default" {
  backend               = vault_jwt_auth_backend.oidc.path
  role_name             = "default"
  role_type             = "oidc"
  user_claim            = "preferred_username"
  groups_claim          = "groups"
  bound_claims          = { groups = "openbao_admins@idm.anurag.sh" }
  oidc_scopes           = ["openid", "email", "profile", "groups"]
  allowed_redirect_uris = ["https://vault.ortzi.cloud/ui/vault/auth/oidc/oidc/callback"]
  token_ttl             = 28800
}

resource "vault_identity_group" "openbao_admins" {
  name     = "openbao_admins"
  type     = "external"
  policies = [vault_policy.admin.name]
}

resource "vault_identity_group_alias" "openbao_admins" {
  name           = "openbao_admins@idm.anurag.sh"
  mount_accessor = vault_jwt_auth_backend.oidc.accessor
  canonical_id   = vault_identity_group.openbao_admins.id
}

resource "vault_jwt_auth_backend" "cli" {
  path               = "jwt"
  type               = "jwt"
  oidc_discovery_url = "https://idm.anurag.sh/oauth2/openid/openbao-cli"
  bound_issuer       = "https://idm.anurag.sh/oauth2/openid/openbao-cli"
  jwt_supported_algs = ["ES256"]
  default_role       = "cli"
}

resource "vault_jwt_auth_backend_role" "cli" {
  backend         = vault_jwt_auth_backend.cli.path
  role_name       = "cli"
  role_type       = "jwt"
  bound_audiences = ["openbao-cli"]
  user_claim      = "sub"
  token_policies  = [vault_policy.admin.name]
  token_ttl       = 900
  token_max_ttl   = 3600
}
