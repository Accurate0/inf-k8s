resource "aws_acm_certificate" "catalog-api" {
  domain_name       = "object-registry.inf-k8s.net"
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}
