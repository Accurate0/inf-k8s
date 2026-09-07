#!/usr/bin/env bash
set -euo pipefail

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

pve_host="${UNRAID_BENCH_PVE:-pve.internal}"
pve_mount="${UNRAID_BENCH_PVE_MOUNT:-/mnt/pve/unraid}"
nfs_source="${UNRAID_BENCH_NFS:-nas.internal:/mnt/user/media}"
local_mount="${UNRAID_BENCH_MOUNT:-/mnt/unraid}"
size_gb=4
tier=all

usage() {
  cat <<'EOF'
Usage: unraid-bench.sh [--tier 2|3|all] [--size GB] [options]

Sequential throughput against the Unraid array, in two tiers:
  tier 2  from pve over NFS on vmbr0, no physical network in the path
  tier 3  from this machine over the LAN, as a client actually sees it

Options:
  --tier N        2, 3 or all (default: all)
  --size GB       test file size in GiB (default: 4)
  --pve HOST      Proxmox host for tier 2 (default: pve.internal)
  --pve-mount P   NFS mountpoint on the Proxmox host (default: /mnt/pve/unraid)
  --nfs SRC       NFS source for tier 3 (default: nas.internal:/mnt/user/media)
  --mount P       local mountpoint for tier 3 (default: /mnt/unraid)
  -h, --help      this text

Size must exceed the Unraid VM's RAM to defeat page cache; the default 4G
relies on O_DIRECT instead, so raise it if the numbers look impossibly good.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tier) tier="$2"; shift 2 ;;
    --size) size_gb="$2"; shift 2 ;;
    --pve) pve_host="$2"; shift 2 ;;
    --pve-mount) pve_mount="$2"; shift 2 ;;
    --nfs) nfs_source="$2"; shift 2 ;;
    --mount) local_mount="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

[[ "$tier" =~ ^(2|3|all)$ ]] || { err "--tier must be 2, 3 or all"; exit 2; }
if ! [[ "$size_gb" =~ ^[0-9]+$ ]] || (( size_gb < 1 )); then
  err "--size must be a positive integer"
  exit 2
fi

bench_body() {
  cat <<'SNIP'
set -euo pipefail
dir="$1"; gb="$2"
f="$dir/.unraid-bench.$$"
trap 'rm -f "$f"' EXIT
oflag=direct
dd if=/dev/zero of="$f" bs=1M count=1 oflag=direct status=none 2>/dev/null || oflag=dsync
rm -f "$f"
sync
t0=$(date +%s%N)
dd if=/dev/zero of="$f" bs=1M count=$((gb * 1024)) oflag="$oflag" conv=fsync status=none
t1=$(date +%s%N)
[ -w /proc/sys/vm/drop_caches ] && sync && echo 3 >/proc/sys/vm/drop_caches 2>/dev/null || true
t2=$(date +%s%N)
dd if="$f" of=/dev/null bs=1M iflag=direct status=none 2>/dev/null ||
  dd if="$f" of=/dev/null bs=1M status=none
t3=$(date +%s%N)
awk -v b=$((gb * 1024 * 1024 * 1024)) -v w=$((t1 - t0)) -v r=$((t3 - t2)) \
  'BEGIN { printf "%.1f %.1f\n", b / (w / 1e9) / 1e6, b / (r / 1e9) / 1e6 }'
SNIP
}

results=()

record() {
  results+=("$1|$2|$3")
}

run_tier2() {
  info "tier 2: $pve_host -> $pve_mount (NFS over vmbr0)"

  ssh -o BatchMode=yes -o ConnectTimeout=10 "root@$pve_host" true 2>/dev/null ||
    { err "cannot ssh to root@$pve_host"; return 1; }

  # shellcheck disable=SC2029
  ssh "root@$pve_host" "findmnt -M '$pve_mount'" >/dev/null 2>&1 ||
    { err "$pve_mount is not mounted on $pve_host"; return 1; }

  local out
  out="$(bench_body | ssh "root@$pve_host" bash -s -- "$pve_mount" "$size_gb")"
  record "tier 2  pve over NFS" "${out%% *}" "$(echo "$out" | awk '{print $2}')"
}

run_tier3() {
  info "tier 3: $(hostname) -> $local_mount (NFS over the LAN)"

  local unmount_after=false
  if ! findmnt -M "$local_mount" >/dev/null 2>&1; then
    warn "$local_mount not mounted, mounting $nfs_source (needs sudo)"
    sudo mkdir -p "$local_mount"
    sudo mount -t nfs4 -o vers=4.2,nconnect=4 "$nfs_source" "$local_mount"
    unmount_after=true
  fi

  local host_ip route
  host_ip="$(getent hosts "${nfs_source%%:*}" | awk '{print $1; exit}')"
  if [[ -n "$host_ip" ]]; then
    route="$(ip -br route get "$host_ip" 2>/dev/null | awk '{for(i=1;i<=NF;i++) if($i=="dev") print $(i+1)}')"
    [[ "$route" == wl* ]] && warn "path is over $route (wireless) - this measures the link, not the array"
  fi

  local out
  out="$(bench_body | sudo bash -s -- "$local_mount" "$size_gb")"
  record "tier 3  this host over LAN" "${out%% *}" "$(echo "$out" | awk '{print $2}')"

  $unmount_after && sudo umount "$local_mount"
}

info "sequential throughput, ${size_gb}G test file"

(( size_gb < 16 )) && warn "${size_gb}G may fit in the Unraid server's page cache; use --size 16 or more for honest numbers"

case "$tier" in
  2) run_tier2 ;;
  3) run_tier3 ;;
  all) run_tier2 || true; run_tier3 || true ;;
esac

(( ${#results[@]} )) || { err "no tiers completed"; exit 1; }

printf '\n%s%-26s %12s %12s%s\n' "$bold" "path" "write MB/s" "read MB/s" "$reset"
for row in "${results[@]}"; do
  IFS='|' read -r label write_mbps read_mbps <<<"$row"
  printf '%-26s %12s %12s\n' "$label" "$write_mbps" "$read_mbps"
done

cat <<'EOF'

Unraid does not stripe, so reads run at single-disk speed. Parity writes are
read-modify-write unless Settings -> Disk Settings -> md_write_method is set
to reconstruct write. A large gap between tiers 2 and 3 is the network, not
the array.
EOF
