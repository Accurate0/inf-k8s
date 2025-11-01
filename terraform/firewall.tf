resource "binarylane_server_firewall_rules" "control_firewall" {
  count     = local.control_count
  server_id = binarylane_server.control[count.index].id

  firewall_rules = [
    {
      description           = "SSH"
      protocol              = "all"
      source_addresses      = ["0.0.0.0/0"],
      destination_addresses = binarylane_server.control[count.index].public_ipv4_addresses
      destination_ports     = ["22"]
      action                = "accept"
    },
    {
      description           = "ping"
      protocol              = "icmp"
      source_addresses      = ["0.0.0.0/0"]
      destination_addresses = binarylane_server.control[count.index].public_ipv4_addresses
      destination_ports     = []
      action                = "accept"
    },
    {
      description           = "block remaining"
      protocol              = "all"
      source_addresses      = ["0.0.0.0/0"]
      destination_addresses = binarylane_server.control[count.index].public_ipv4_addresses
      destination_ports     = ["49152:65535", "1024:49151"]
      action                = "drop"
    },
  ]
}

resource "binarylane_server_firewall_rules" "proxy_firewall" {
  count     = local.proxy_count
  server_id = binarylane_server.proxy[count.index].id

  firewall_rules = [
    {
      description           = "SSH"
      protocol              = "all"
      source_addresses      = ["0.0.0.0/0"],
      destination_addresses = binarylane_server.proxy[count.index].public_ipv4_addresses
      destination_ports     = ["22"]
      action                = "accept"
    },
    {
      description           = "HTTP/S"
      protocol              = "all"
      source_addresses      = ["0.0.0.0/0"]
      destination_addresses = binarylane_server.proxy[count.index].public_ipv4_addresses
      destination_ports     = ["80", "443"]
      action                = "accept"
    },
    {
      description           = "allow kubectl"
      protocol              = "all"
      source_addresses      = ["0.0.0.0/0"]
      destination_addresses = binarylane_server.proxy[count.index].public_ipv4_addresses
      destination_ports     = ["6443"]
      action                = "accept"
    },
    {
      description           = "ping"
      protocol              = "icmp"
      source_addresses      = ["0.0.0.0/0"]
      destination_addresses = binarylane_server.proxy[count.index].public_ipv4_addresses
      destination_ports     = []
      action                = "accept"
    },
    {
      description           = "block remaining"
      protocol              = "all"
      source_addresses      = ["0.0.0.0/0"]
      destination_addresses = binarylane_server.proxy[count.index].public_ipv4_addresses
      destination_ports     = ["49152:65535", "1024:49151"]
      action                = "drop"
    },
  ]
}

resource "binarylane_server_firewall_rules" "uptime_firewall" {
  server_id = binarylane_server.uptime.id

  firewall_rules = [
    {
      description           = "SSH"
      protocol              = "all"
      source_addresses      = ["0.0.0.0/0"],
      destination_addresses = binarylane_server.uptime.public_ipv4_addresses
      destination_ports     = ["22"]
      action                = "accept"
    },
    # {
    #   description           = "HTTP/S"
    #   protocol              = "all"
    #   source_addresses      = ["0.0.0.0/0"]
    #   destination_addresses = binarylane_server.uptime.public_ipv4_addresses
    #   destination_ports     = ["80", "443"]
    #   action                = "accept"
    # },
    {
      description           = "block remaining"
      protocol              = "all"
      source_addresses      = ["0.0.0.0/0"]
      destination_addresses = binarylane_server.uptime.public_ipv4_addresses
      destination_ports     = ["49152:65535", "1024:49151"]
      action                = "drop"
    },
  ]
}
