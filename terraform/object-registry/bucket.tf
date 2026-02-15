resource "aws_s3_bucket" "object-registry-bucket" {
  bucket = "object-registry-inf-k8s"
}

resource "aws_s3_bucket_public_access_block" "object-registry-public-access-block" {
  bucket = aws_s3_bucket.object-registry-bucket.id

  block_public_acls       = true
  block_public_policy     = true
  restrict_public_buckets = true
  ignore_public_acls      = true
}

resource "aws_s3_bucket_versioning" "object-registry-versioning" {
  bucket = aws_s3_bucket.object-registry-bucket.id
  versioning_configuration {
    status = "Enabled"
  }
}


resource "aws_s3_bucket" "object-registry-public-keys-bucket" {
  bucket = "object-registry-public-keys-inf-k8s"
}

resource "aws_s3_bucket_public_access_block" "object-registry-public-keys-public-access-block" {
  bucket = aws_s3_bucket.object-registry-public-keys-bucket.id

  block_public_acls       = true
  block_public_policy     = true
  restrict_public_buckets = true
  ignore_public_acls      = true
}

resource "aws_s3_bucket_versioning" "object-registry-public-keys-versioning" {
  bucket = aws_s3_bucket.object-registry-public-keys-bucket.id
  versioning_configuration {
    status = "Enabled"
  }
}

