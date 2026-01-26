resource "aws_secretsmanager_secret" "jwt-secret" {
  name = "object-registry-jwt-secret"
}

resource "random_password" "secret-key" {
  length = 100
}

resource "aws_secretsmanager_secret_version" "jwt-secret" {
  secret_id     = aws_secretsmanager_secret.jwt-secret.id
  secret_string = base64encode(random_password.secret-key.result)
}


resource "infisical_secret" "config-catalog-jwt-shared" {
  for_each = toset([
    # home-gateway
    "759d2a91-e4da-4506-b61e-e415645aa3ae"
  ])
  name         = "CONFIG_CATALOG_JWT_SECRET"
  value        = base64encode(random_password.secret-key.result)
  env_slug     = "prod"
  workspace_id = each.value
  folder_path  = "/"
}
