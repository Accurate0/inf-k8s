resource "aws_acm_certificate" "object-registry-api" {
  domain_name       = "object-registry.inf-k8s.net"
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}
