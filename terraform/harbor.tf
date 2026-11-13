ephemeral "vault_kv_secret_v2" "harbor" {
  mount    = vault_mount.kv.path
  mount_id = vault_mount.kv.id
  name     = "registry/generated/harbor-generated"
}

provider "harbor" {
  url      = "https://registry.anurag.sh"
  username = "admin"
  password = ephemeral.vault_kv_secret_v2.harbor.data["HARBOR_ADMIN_PASSWORD"]
}

locals {
  harbor_proxies = {
    ghcr = {
      provider_name = "github"
      endpoint_url  = "https://ghcr.io"
    }
    dockerhub = {
      provider_name = "docker-hub"
      endpoint_url  = "https://hub.docker.com"
    }
    quay = {
      provider_name = "docker-registry"
      endpoint_url  = "https://quay.io"
    }
  }
}

resource "harbor_registry" "proxy" {
  for_each = local.harbor_proxies

  name          = each.key
  provider_name = each.value.provider_name
  endpoint_url  = each.value.endpoint_url
}

resource "harbor_project" "proxy" {
  for_each = local.harbor_proxies

  name        = each.key
  public      = false
  registry_id = harbor_registry.proxy[each.key].registry_id
}

ephemeral "random_password" "harbor-node-pull" {
  length  = 32
  special = false
}

resource "harbor_robot_account" "node-pull" {
  name              = "node-pull"
  level             = "system"
  secret_wo         = ephemeral.random_password.harbor-node-pull.result
  secret_wo_version = 1

  permissions {
    kind      = "project"
    namespace = harbor_project.own["containers"].name

    access {
      action   = "pull"
      resource = "repository"
    }
  }

  dynamic "permissions" {
    for_each = harbor_project.proxy

    content {
      kind      = "project"
      namespace = permissions.value.name

      access {
        action   = "pull"
        resource = "repository"
      }
    }
  }
}

resource "vault_kv_secret_v2" "harbor-node-pull" {
  mount = vault_mount.kv.path
  name  = "harbor/node-pull"
  data_json_wo = jsonencode({
    USERNAME = harbor_robot_account.node-pull.full_name
    PASSWORD = ephemeral.random_password.harbor-node-pull.result
  })
  data_json_wo_version = 1
}

resource "harbor_project" "own" {
  for_each = toset(["charts", "containers"])

  name   = each.key
  public = false
}

ephemeral "random_password" "harbor-forgejo" {
  length  = 32
  special = false
}

resource "harbor_robot_account" "forgejo" {
  name              = "forgejo"
  level             = "system"
  secret_wo         = ephemeral.random_password.harbor-forgejo.result
  secret_wo_version = 1

  dynamic "permissions" {
    for_each = harbor_project.own

    content {
      kind      = "project"
      namespace = permissions.value.name

      access {
        action   = "pull"
        resource = "repository"
      }

      access {
        action   = "push"
        resource = "repository"
      }
    }
  }

  dynamic "permissions" {
    for_each = harbor_project.proxy

    content {
      kind      = "project"
      namespace = permissions.value.name

      access {
        action   = "pull"
        resource = "repository"
      }
    }
  }
}

resource "vault_kv_secret_v2" "harbor-forgejo" {
  mount = vault_mount.kv.path
  name  = "harbor/forgejo"
  data_json_wo = jsonencode({
    USERNAME = harbor_robot_account.forgejo.full_name
    PASSWORD = ephemeral.random_password.harbor-forgejo.result
  })
  data_json_wo_version = 1
}

ephemeral "random_password" "harbor-argocd" {
  length  = 32
  special = false
}

resource "harbor_robot_account" "argocd" {
  name              = "argocd"
  level             = "system"
  secret_wo         = ephemeral.random_password.harbor-argocd.result
  secret_wo_version = 1

  permissions {
    kind      = "project"
    namespace = harbor_project.own["charts"].name

    access {
      action   = "pull"
      resource = "repository"
    }
  }
}

resource "vault_kv_secret_v2" "harbor-argocd" {
  mount = vault_mount.kv.path
  name  = "argocd/harbor-repo-creds"
  data_json_wo = jsonencode({
    USERNAME = harbor_robot_account.argocd.full_name
    PASSWORD = ephemeral.random_password.harbor-argocd.result
  })
  data_json_wo_version = 1
}

resource "harbor_project" "ortzi" {
  for_each = toset(["ortzi-packages", "ortzi-catalog"])

  name   = each.key
  public = false
}

resource "harbor_immutable_tag_rule" "ortzi-releases" {
  project_id    = harbor_project.ortzi["ortzi-packages"].id
  repo_matching = "**"
  tag_matching  = "*.*.*"
}

ephemeral "random_password" "harbor-exchange" {
  length  = 32
  special = false
}

resource "harbor_robot_account" "exchange" {
  name              = "exchange"
  description       = "registry service: mints short-lived robots and writes the ortzi catalog"
  level             = "system"
  secret_wo         = ephemeral.random_password.harbor-exchange.result
  secret_wo_version = 1

  dynamic "permissions" {
    for_each = merge(harbor_project.own, harbor_project.proxy, harbor_project.ortzi)

    content {
      kind      = "project"
      namespace = permissions.value.name

      access {
        action   = "pull"
        resource = "repository"
      }

      dynamic "access" {
        for_each = startswith(permissions.value.name, "ortzi-") ? ["push"] : []

        content {
          action   = access.value
          resource = "repository"
        }
      }

      dynamic "access" {
        for_each = ["create", "list", "read", "delete"]

        content {
          action   = access.value
          resource = "robot"
        }
      }
    }
  }
}

resource "vault_kv_secret_v2" "harbor-exchange" {
  mount = vault_mount.kv.path
  name  = "registry/harbor-exchange"
  data_json_wo = jsonencode({
    HARBOR_USERNAME = harbor_robot_account.exchange.full_name
    HARBOR_PASSWORD = ephemeral.random_password.harbor-exchange.result
  })
  data_json_wo_version = 1
}
