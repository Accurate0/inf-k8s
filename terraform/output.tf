output "inventory" {
  sensitive = true
  value = yamlencode({
    "all" : {
      children : {
        control = {},
        proxy   = {}
        agent   = {}
      }
    }
    "control" : {
      hosts : { for s in binarylane_server.control : s.name => {
        "ansible_host" : s.public_ipv4_addresses[0],
        "ansible_user" : "root"
        }
      }
    }

    "proxy" : {
      hosts : { for s in binarylane_server.proxy : s.name => {
        "ansible_host" : s.public_ipv4_addresses[0]
        "ansible_user" : "root"
        }
      }
    }

    "agent" : {
      hosts : { for s in binarylane_server.agent : s.name => {
        "ansible_host" : s.public_ipv4_addresses[0]
        "ansible_user" : "root"
        }
      }
    }
  })
}

output "eso_shared_vault_client_id" {
  value = azuread_application.eso-shared-vault.client_id
}

output "eso_shared_vault_client_secret" {
  sensitive = true
  value     = azuread_application_password.eso-shared-vault.value
}
