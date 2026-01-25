resource "aws_apigatewayv2_domain_name" "this" {
  domain_name = "object-registry.inf-k8s.net"

  domain_name_configuration {
    certificate_arn = aws_acm_certificate.catalog-api.arn
    endpoint_type   = "REGIONAL"
    security_policy = "TLS_1_2"
  }
}

resource "aws_apigatewayv2_api_mapping" "this" {
  api_id          = aws_apigatewayv2_api.this.id
  domain_name     = aws_apigatewayv2_domain_name.this.id
  stage           = "v1"
  api_mapping_key = "v1"
}

resource "aws_apigatewayv2_api" "this" {
  name                         = "Config Catalog API"
  protocol_type                = "HTTP"
  disable_execute_api_endpoint = true
  ip_address_type              = "dualstack"
}

resource "aws_apigatewayv2_integration" "this" {
  api_id           = aws_apigatewayv2_api.this.id
  integration_type = "AWS_PROXY"

  integration_method     = "POST"
  integration_uri        = aws_lambda_function.api.invoke_arn
  payload_format_version = "2.0"

  request_parameters = {
    "overwrite:path" = "$request.path"
  }
}

resource "aws_apigatewayv2_stage" "this" {
  api_id      = aws_apigatewayv2_api.this.id
  name        = "v1"
  auto_deploy = true
}

resource "aws_lambda_permission" "api-gateway" {
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.api.function_name
  principal     = "apigateway.amazonaws.com"

  source_arn = "${aws_apigatewayv2_api.this.execution_arn}/*/*"
}

