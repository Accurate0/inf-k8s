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
}

resource "vault_jwt_auth_backend_role" "default" {
  backend               = vault_jwt_auth_backend.oidc.path
  role_name             = "default"
  role_type             = "oidc"
  user_claim            = "preferred_username"
  groups_claim          = "groups"
  oidc_scopes           = ["openid", "email", "profile", "groups"]
  allowed_redirect_uris = ["https://openbao.inf-k8s.net/ui/vault/auth/oidc/oidc/callback"]
  token_ttl             = 28800
}

resource "vault_identity_group" "platform_admins" {
  name     = "platform_admins"
  type     = "external"
  policies = [vault_policy.admin.name]
}

resource "vault_identity_group_alias" "platform_admins" {
  name           = "platform_admins@idm.anurag.sh"
  mount_accessor = vault_jwt_auth_backend.oidc.accessor
  canonical_id   = vault_identity_group.platform_admins.id
}

data "vault_identity_group" "platform_admins" {
  group_name = "platform_admins"
}

import {
  to = vault_policy.admin
  id = "admin"
}

import {
  to = vault_jwt_auth_backend.oidc
  id = "oidc"
}

import {
  to = vault_jwt_auth_backend_role.default
  id = "auth/oidc/role/default"
}

import {
  to = vault_identity_group.platform_admins
  id = data.vault_identity_group.platform_admins.group_id
}

import {
  to = vault_identity_group_alias.platform_admins
  id = data.vault_identity_group.platform_admins.alias_id
}
