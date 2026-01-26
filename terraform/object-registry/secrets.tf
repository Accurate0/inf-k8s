resource "aws_secretsmanager_secret" "jwt-secret" {
  name = "object-registry-jwt-secret"
}

resource "random_password" "secret-key" {
  length = 100
}

resource "aws_secretsmanager_secret_version" "jwt-secret" {
  secret_id     = aws_secretsmanager_secret.jwt-secret.id
  secret_string = tls_private_key.jwt-private-key.private_key_pem
}

resource "tls_private_key" "jwt-private-key" {
  algorithm = "RSA"
  rsa_bits  = 4096
}

resource "aws_s3_object" "object" {
  bucket  = "object-registry-inf-k8s"
  key     = "public-keys/77d442a2-e6f9-4e16-b48b-020443aa7515.pem"
  content = tls_private_key.jwt-private-key.public_key_pem
  etag    = md5(tls_private_key.jwt-private-key.public_key_pem)
}
