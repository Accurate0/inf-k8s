resource "aws_acm_certificate" "catalog-api" {
  domain_name       = "config-catalog.inf-k8s.net"
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}
