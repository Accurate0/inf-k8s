locals {
  endpoints = [
    {
      "method" : "GET",
      "url" : "/{bucket}/{object}"
    },
    {
      "method" : "PUT",
      "url" : "/{bucket}/{object}"
    },
    {
      "method" : "HEAD",
      "url" : "/{bucket}/{object}"
    },
    {
      "method" : "DELETE",
      "url" : "/{bucket}/{object}"
    },
    {
      "method" : "GET",
      "url" : "/{bucket}"
    },
    {
      "method" : "GET",
      "url" : "/api/namespaces"
    },
    {
      "method" : "GET",
      "url" : "/api/audit"
    },
    {
      "method" : "GET",
      "url" : "/api/{namespace}/{object}"
    },
    {
      "method" : "GET",
      "url" : "/api/{namespace}"
    },
    {
      "method" : "PUT",
      "url" : "/api/{namespace}/{object}"
    },
    {
      "method" : "DELETE",
      "url" : "/api/{namespace}/{object}"
    },
    {
      "method" : "GET",
      "url" : "/api/events/{namespace}"
    },
    {
      "method" : "POST",
      "url" : "/api/events/{namespace}"
    },
    {
      "method" : "PUT",
      "url" : "/api/events/{namespace}/{id}"
    },
    {
      "method" : "DELETE",
      "url" : "/api/events/{namespace}/{id}"
    },
    {
      "method" : "GET",
      "url" : "/api/health",
    },
    {
      "method" : "GET",
      "url" : "/.well-known/jwks",
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
