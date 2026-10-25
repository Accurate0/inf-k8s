resource "tailscale_acl" "tailnet" {
  acl = file("${path.module}/tailscale/policy.hujson")
}

data "tailscale_device" "k8s_proxy" {
  hostname = "k8s-proxy-1"
}

resource "tailscale_device_tags" "k8s_proxy" {
  device_id = data.tailscale_device.k8s_proxy.node_id
  tags      = ["tag:k8s-proxy"]

  depends_on = [tailscale_acl.tailnet]
}
