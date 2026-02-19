resource "aws_iam_policy" "object-registry-lambda-access" {
  name        = "object-registry-lambda-access"
  description = "Access policy for object-registry application"
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
            aws_s3_bucket.object-registry-bucket.arn,
            "${aws_s3_bucket.object-registry-bucket.arn}/*",
            aws_s3_bucket.object-registry-public-keys-bucket.arn,
            "${aws_s3_bucket.object-registry-public-keys-bucket.arn}/*"
          ]
        },
        {
          "Effect" = "Allow",
          "Action" = [
            "secretsmanager:GetResourcePolicy",
            "secretsmanager:GetSecretValue",
            "secretsmanager:DescribeSecret",
            "secretsmanager:ListSecretVersionIds"
          ],
          "Resource" = [
            "${aws_secretsmanager_secret.jwt-secret.arn}",
          ]
        },
        {
          "Effect" : "Allow",
          "Action" : [
            "dynamodb:GetItem",
            "dynamodb:PutItem",
            "dynamodb:DeleteItem",
            "dynamodb:UpdateItem",
            "dynamodb:Query",
            "dynamodb:Scan",
            "dynamodb:BatchGetItem",
            "dynamodb:BatchWriteItem",
            "dynamodb:ConditionCheckItem"
          ],
          "Resource" : [
            "${aws_dynamodb_table.object-registry-keys.arn}",
            "${aws_dynamodb_table.object-registry-metadata.arn}",
            "${aws_dynamodb_table.object-registry-events.arn}",
            "${aws_dynamodb_table.object-registry-audit.arn}",
          ]
        }
      ]
    }
  )
}

data "aws_iam_policy_document" "object-registry-events-policy" {
  statement {
    effect = "Allow"

    principals {
      type        = "Service"
      identifiers = ["lambda.amazonaws.com"]
    }

    actions = ["sts:AssumeRole"]
  }
}


resource "aws_iam_role" "object-registry-lambda" {
  name = "object-registry-lambda-iam"

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

resource "aws_iam_role_policy_attachment" "object-registry-lambda-access" {
  role       = aws_iam_role.object-registry-lambda.name
  policy_arn = aws_iam_policy.object-registry-lambda-access.arn
}

resource "aws_iam_role_policy_attachment" "object-registry-lambda-basic-execution" {
  role       = aws_iam_role.object-registry-lambda.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
}

resource "aws_iam_role" "object-registry-events-iam" {
  name               = "object-registry-events-iam"
  assume_role_policy = data.aws_iam_policy_document.object-registry-events-policy.json
}

resource "aws_lambda_permission" "bucket-object-registry-events" {
  statement_id  = "AllowExecutionFromS3Bucket"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.events.arn
  principal     = "s3.amazonaws.com"
  source_arn    = aws_s3_bucket.object-registry-bucket.arn
}

resource "aws_s3_bucket_notification" "object-registry-bucket-notification" {
  bucket = aws_s3_bucket.object-registry-bucket.id

  lambda_function {
    lambda_function_arn = aws_lambda_function.events.arn
    events              = ["s3:ObjectCreated:*"]
  }

  depends_on = [aws_lambda_permission.bucket-object-registry-events]
}

