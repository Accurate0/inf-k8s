data "cloudflare_zone" "ortzi" {
  filter = {
    name = "ortzi.cloud"
  }
}

resource "cloudflare_dns_record" "agent_ortzi" {
  count   = local.agent_count
  zone_id = data.cloudflare_zone.ortzi.id
  name    = "k8s-agent-${count.index + 1}.host"
  content = binarylane_server.agent[count.index].public_ipv4_addresses[0]
  type    = "A"
  ttl     = 1
}

resource "cloudflare_dns_record" "external_ortzi" {
  count   = local.proxy_count
  zone_id = data.cloudflare_zone.ortzi.id
  name    = "external"
  content = binarylane_server.proxy[count.index].public_ipv4_addresses[0]
  proxied = false
  type    = "A"
  ttl     = 1
}

resource "cloudflare_dns_record" "external_ipv6_ortzi" {
  count   = local.proxy_count
  zone_id = data.cloudflare_zone.ortzi.id
  name    = "external"
  content = binarylane_server.proxy[count.index].public_ipv6_addresses[0]
  proxied = false
  type    = "AAAA"
  ttl     = 1
}

# Tailscale IPs of the nodes, plus the internal
# gateway entrypoint (k8s-proxy-1's Tailscale IP) for the apex and wildcard.
locals {
  ortzi_tailscale_records = {
    "k8s-optiplex-4.host" = "100.122.213.30"
    "k8s-optiplex-5.host" = "100.72.50.9"
    "k8s-pve-1.host"      = "100.125.61.29"
    "k8s-proxy-1.host"    = "100.98.212.43"
    "@"                   = "100.98.212.43"
    "*"                   = "100.98.212.43"
  }
}

resource "cloudflare_dns_record" "tailscale_ortzi" {
  for_each = local.ortzi_tailscale_records
  zone_id  = data.cloudflare_zone.ortzi.id
  name     = each.key
  content  = each.value
  proxied  = false
  type     = "A"
  ttl      = 1
}

data "cloudflare_zone" "anurag_sh" {
  filter = {
    name = "anurag.sh"
  }
}

# lan clients reach registry.anurag.sh directly, and firefox sends the cloudflare-ech.com
# outer sni from the zone's https record, which the lan gateway has no certificate for
resource "cloudflare_zone_setting" "anurag_sh_ech" {
  zone_id    = data.cloudflare_zone.anurag_sh.id
  setting_id = "ech"
  value      = "off"
}
