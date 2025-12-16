resource "aws_s3_bucket" "home-gateway-image-bucket" {
  bucket = "home-gateway-image-bucket"
}

resource "aws_s3_bucket_public_access_block" "home-gateway-image-bucket-public-block" {
  bucket = aws_s3_bucket.home-gateway-image-bucket.id

  block_public_acls       = true
  block_public_policy     = true
  restrict_public_buckets = true
  ignore_public_acls      = true
}

resource "aws_iam_user" "home-gateway-user" {
  name = "home-gateway"
}

resource "aws_iam_policy" "home-gateway-policy" {
  name        = "home-gateway-access"
  description = "Access policy for home-gateway application"
  policy = jsonencode(
    {
      "Version" : "2012-10-17",
      "Statement" : [
        {
          "Sid" : "AllowBucketAccess",
          "Effect" : "Allow",
          "Action" : [
            "s3:DeleteObject",
            "s3:GetObject",
            "s3:PutObject",
            "s3:ListBucket"
          ],
          "Resource" : aws_s3_bucket.home-gateway-image-bucket.arn
        }
      ]
    }
  )
}

resource "aws_iam_user_policy_attachment" "policy-attachment" {
  user       = aws_iam_user.home-gateway-user.name
  policy_arn = aws_iam_policy.home-gateway-policy.arn
}

resource "aws_iam_access_key" "home-gateway-access-key" {
  user = aws_iam_user.home-gateway-user.name
}

resource "infisical_secret" "aws_key_id" {
  name         = "AWS_ACCESS_KEY_ID"
  value        = aws_iam_access_key.home-gateway-access-key.id
  env_slug     = "prod"
  workspace_id = "759d2a91-e4da-4506-b61e-e415645aa3ae"
  folder_path  = "/"
}

resource "infisical_secret" "aws_key_secret" {
  name         = "AWS_SECRET_ACCESS_KEY"
  value        = aws_iam_access_key.home-gateway-access-key.secret
  env_slug     = "prod"
  workspace_id = "759d2a91-e4da-4506-b61e-e415645aa3ae"
  folder_path  = "/"
}
