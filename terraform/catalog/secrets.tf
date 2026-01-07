resource "aws_secretsmanager_secret" "jwt-secret" {
  name = "config-catalog-jwt-secret"
}

resource "random_password" "secret-key" {
  length = 100
}

resource "aws_secretsmanager_secret_version" "jwt-secret" {
  secret_id     = aws_secretsmanager_secret.jwt-secret.id
  secret_string = base64encode(random_password.secret-key.result)
}
