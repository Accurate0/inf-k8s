#!/bin/sh
set -eu

target=/etc/haproxy/cloudflare-ips.txt
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

curl -fsS --max-time 30 https://api.cloudflare.com/client/v4/ips \
  | jq -er 'select(.success == true) | .result.ipv4_cidrs + .result.ipv6_cidrs | select(length > 0) | .[]' > "$tmp"

if cmp -s "$tmp" "$target"; then
  exit 0
fi

install -m 0644 -o root -g root "$tmp" "$target"

if systemctl is-active --quiet haproxy; then
  systemctl reload haproxy
fi
