locals {
  endpoints = [
    {
      "method" : "GET",
      "url" : "/namespaces"
    },
    {
      "method" : "GET",
      "url" : "/{namespace}/{object}"
    },
    {
      "method" : "GET",
      "url" : "/{namespace}"
    },
    {
      "method" : "PUT",
      "url" : "/{namespace}/{object}"
    },
    {
      "method" : "DELETE",
      "url" : "/{namespace}/{object}"
    },
    {
      "method" : "GET",
      "url" : "/events/{namespace}"
    },
    {
      "method" : "POST",
      "url" : "/events/{namespace}"
    },
    {
      "method" : "PUT",
      "url" : "/events/{namespace}/{id}"
    },
    {
      "method" : "DELETE",
      "url" : "/events/{namespace}/{id}"
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
