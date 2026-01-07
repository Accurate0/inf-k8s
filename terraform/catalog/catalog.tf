resource "aws_s3_bucket" "catalog-bucket" {
  bucket = "config-catalog-inf-k8s"
}

resource "aws_s3_bucket_public_access_block" "catalog-public-access-block" {
  bucket = aws_s3_bucket.catalog-bucket.id

  block_public_acls       = true
  block_public_policy     = true
  restrict_public_buckets = true
  ignore_public_acls      = true
}

resource "aws_s3_bucket_versioning" "catalog-versioning" {
  bucket = aws_s3_bucket.catalog-bucket.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_iam_policy" "catalog-lambda-access" {
  name        = "catalog-lambda-access"
  description = "Access policy for catalog application"
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
            aws_s3_bucket.catalog-bucket.arn,
            "${aws_s3_bucket.catalog-bucket.arn}/*"
          ]
        }
      ]
    }
  )
}

data "aws_iam_policy_document" "catalog-events-policy" {
  statement {
    effect = "Allow"

    principals {
      type        = "Service"
      identifiers = ["lambda.amazonaws.com"]
    }

    actions = ["sts:AssumeRole"]
  }
}


resource "aws_iam_role" "catalog-lambda" {
  name = "catalog-lambda-iam"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action = "sts:AssumeRole"
      Effect = "Allow"
      Sid    = ""
      Principal = {
        Service = "lambda.amazonaws.com"
      }
      }
    ]
  })
}

resource "aws_iam_role_policy_attachment" "catalog-lambda-access" {
  role       = aws_iam_role.catalog-lambda.name
  policy_arn = aws_iam_policy.catalog-lambda-access.arn
}

resource "aws_iam_role_policy_attachment" "catalog-lambda-basic-execution" {
  role       = aws_iam_role.catalog-lambda.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
}

resource "aws_iam_role" "catalog-events-iam" {
  name               = "catalog-events-iam"
  assume_role_policy = data.aws_iam_policy_document.catalog-events-policy.json
}

resource "aws_lambda_permission" "bucket-catalog-events" {
  statement_id  = "AllowExecutionFromS3Bucket"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.events.arn
  principal     = "s3.amazonaws.com"
  source_arn    = aws_s3_bucket.catalog-bucket.arn
}

resource "aws_s3_bucket_notification" "catalog-bucket-notification" {
  bucket = aws_s3_bucket.catalog-bucket.id

  lambda_function {
    lambda_function_arn = aws_lambda_function.events.arn
    events              = ["s3:ObjectCreated:*"]
  }

  depends_on = [aws_lambda_permission.bucket-catalog-events]
}

data "archive_file" "dummy" {
  type        = "zip"
  output_path = "${path.module}/lambda_function_payload.zip"

  source {
    content  = "dummy"
    filename = "dummy.txt"
  }
}

resource "aws_lambda_function" "events" {
  function_name = "catalog-events"
  handler       = "bootstrap"
  role          = aws_iam_role.catalog-lambda.arn
  filename      = data.archive_file.dummy.output_path
  timeout       = 30
  memory_size   = 256
  runtime       = "provided.al2"
  environment {
    variables = {
      "AWS_LAMBDA_EXEC_WRAPPER" = "/opt/bootstrap"
      "RUST_LOG"                = "info"
      "AWS_LWA_PORT"            = "8000"
      "PORT"                    = "8000"
    }
  }
}

resource "aws_lambda_function" "api" {
  function_name = "catalog-api"
  handler       = "bootstrap"
  role          = aws_iam_role.catalog-lambda.arn
  filename      = data.archive_file.dummy.output_path
  timeout       = 30
  memory_size   = 256
  runtime       = "provided.al2"
  environment {
    variables = {
      "AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH" = "true"
      "AWS_LAMBDA_EXEC_WRAPPER"              = "/opt/bootstrap"
      "RUST_LOG"                             = "info"
      "AWS_LWA_PORT"                         = "8000"
      "PORT"                                 = "8000"
    }
  }
}
