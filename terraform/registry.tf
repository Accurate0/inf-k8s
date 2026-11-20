resource "tls_private_key" "registry-signing" {
  algorithm   = "ECDSA"
  ecdsa_curve = "P256"
}

resource "vault_kv_secret_v2" "registry-signing" {
  mount = vault_mount.kv.path
  name  = "registry/signing"
  data_json = jsonencode({
    SIGNING_KEY = tls_private_key.registry-signing.private_key_pem_pkcs8
  })
}

resource "vault_kv_secret_v2" "flux-cosign" {
  mount = vault_mount.kv.path
  name  = "flux-system/cosign"
  data_json = jsonencode({
    "cosign.pub" = tls_private_key.registry-signing.public_key_pem
  })
}
