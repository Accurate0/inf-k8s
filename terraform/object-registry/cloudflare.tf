variable "cloudflare_zone_id" {
  type      = string
  sensitive = true
  default   = "8d993ee38980642089a2ebad74531806"
}

resource "cloudflare_dns_record" "validation-record-api" {
  for_each = {
    for item in aws_acm_certificate.object-registry-api.domain_validation_options : item.domain_name => {
      name   = item.resource_record_name
      record = item.resource_record_value
      type   = item.resource_record_type
    }
  }

  zone_id = var.cloudflare_zone_id
  proxied = false
  name    = each.value.name
  type    = each.value.type
  content = each.value.record
  ttl     = 1

  lifecycle {
    ignore_changes = [content]
  }
}

resource "cloudflare_dns_record" "object-registry-api" {
  zone_id = var.cloudflare_zone_id
  proxied = false
  name    = "object-registry"
  type    = "CNAME"
  content = aws_apigatewayv2_domain_name.this.domain_name_configuration[0].target_domain_name
  ttl     = 1
}

resource "cloudflare_dns_record" "aws-api-issue" {
  zone_id = var.cloudflare_zone_id
  name    = "object-registry"
  data = {
    flags = 0
    tag   = "issue"
    value = "awstrust.com"
  }
  type = "CAA"
  ttl  = 1
}
