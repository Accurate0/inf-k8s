resource "aws_s3_bucket" "home-gateway-bucket" {
  bucket = "home-gateway-bucket"
}

resource "aws_s3_bucket_public_access_block" "home-gateway-bucket-public-block" {
  bucket = aws_s3_bucket.home-gateway-bucket.id

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
          "Resource" : [
            aws_s3_bucket.home-gateway-bucket.arn,
            "${aws_s3_bucket.home-gateway-bucket.arn}/*"
          ]
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

resource "github_actions_secret" "aws-key-id" {
  repository  = "home-gateway"
  secret_name = "AWS_ACCESS_KEY_ID"
  value       = aws_iam_access_key.home-gateway-access-key.id
}

resource "github_actions_secret" "aws-key-secret" {
  repository  = "home-gateway"
  secret_name = "AWS_SECRET_ACCESS_KEY"
  value       = aws_iam_access_key.home-gateway-access-key.secret
}

resource "google_project_service" "iam" {
  service            = "iam.googleapis.com"
  disable_on_destroy = false
}

resource "google_project_service" "firebase" {
  service            = "firebase.googleapis.com"
  disable_on_destroy = false
}

resource "google_project_service" "fcm" {
  service            = "fcm.googleapis.com"
  disable_on_destroy = false
}

resource "google_service_account" "home-gateway-fcm" {
  account_id   = "home-gateway-fcm"
  display_name = "home-gateway Firebase Cloud Messaging"
}

resource "google_project_iam_member" "home-gateway-fcm" {
  project = "home-gateway-22"
  role    = "roles/firebasecloudmessaging.admin"
  member  = "serviceAccount:${google_service_account.home-gateway-fcm.email}"
}

resource "google_service_account_key" "home-gateway-fcm" {
  service_account_id = google_service_account.home-gateway-fcm.name
}

resource "vault_kv_secret_v2" "home-gateway" {
  mount = vault_mount.kv.path
  name  = "home-gateway/terraform"
  data_json = jsonencode({
    AWS_ACCESS_KEY_ID                         = aws_iam_access_key.home-gateway-access-key.id
    AWS_SECRET_ACCESS_KEY                     = aws_iam_access_key.home-gateway-access-key.secret
    NOTIFY__ANDROID__FCM_SERVICE_ACCOUNT_JSON = base64decode(google_service_account_key.home-gateway-fcm.private_key)
  })
}
