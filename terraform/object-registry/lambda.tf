data "archive_file" "dummy" {
  type        = "zip"
  output_path = "${path.module}/lambda_function_payload.zip"

  source {
    content  = "dummy"
    filename = "dummy.txt"
  }
}

resource "aws_lambda_function" "events" {
  function_name = "object-registry-events"
  handler       = "bootstrap"
  role          = aws_iam_role.object-registry-lambda.arn
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
  function_name = "object-registry-api"
  handler       = "bootstrap"
  role          = aws_iam_role.object-registry-lambda.arn
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
