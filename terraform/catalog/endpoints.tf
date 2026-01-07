locals {
  endpoints = [
    {
      "method" : "GET",
      "url" : "/{namespace}/{configObject}"
    },
    {
      "method" : "PUT",
      "url" : "/{namespace}/{configObject}"
    },
    {
      "method" : "GET",
      "url" : "/health",
    },
  ]
}


resource "aws_apigatewayv2_route" "this" {
  for_each = { for x in local.endpoints : "${x.method} ${x.url}" => x }

  api_id             = aws_apigatewayv2_api.this.id
  route_key          = "${each.value.method} ${each.value.url}"
  target             = "integrations/${aws_apigatewayv2_integration.this.id}"
  operation_name     = each.key
  authorization_type = "NONE"
}
