resource "aws_dynamodb_table" "object-registry-keys" {
  name                        = "object-registry-keys"
  billing_mode                = "PAY_PER_REQUEST"
  hash_key                    = "key_id"
  deletion_protection_enabled = true

  attribute {
    name = "key_id"
    type = "S"
  }

  ttl {
    enabled        = true
    attribute_name = "ttl"
  }

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_dynamodb_table" "object-registry-events" {
  name                        = "object-registry-events"
  billing_mode                = "PAY_PER_REQUEST"
  hash_key                    = "id"
  deletion_protection_enabled = true

  attribute {
    name = "id"
    type = "S"
  }

  lifecycle {
    prevent_destroy = true
  }
}
resource "aws_dynamodb_table" "object-registry-metadata" {
  name                        = "object-registry-metadata"
  billing_mode                = "PAY_PER_REQUEST"
  hash_key                    = "object_key"
  deletion_protection_enabled = true

  attribute {
    name = "object_key"
    type = "S"
  }

  lifecycle {
    prevent_destroy = true
  }
}
