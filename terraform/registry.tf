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
