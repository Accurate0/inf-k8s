output "aws_lambda_arns" {
  value = [aws_lambda_function.api.arn, aws_lambda_function.events.arn]
}
