resource "tailscale_acl" "tailnet" {
  acl = file("${path.module}/tailscale/policy.hujson")
}
