#!/usr/bin/env bash
#
# Reboot k8s nodes one at a time: cordon + drain, wait for Longhorn volumes to
# detach, reboot over SSH, wait for a new boot id and the node to come back
# Ready, uncordon, wait for Longhorn to be healthy, then ask before moving on.
# Connection details come from the ansible inventory. Agents are done first,
# control/etcd nodes last.
#
#   scripts/rolling-reboot.sh                 # all nodes
#   scripts/rolling-reboot.sh k8s-optiplex-2  # only the named host(s)

set -euo pipefail

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
inventory="${INVENTORY:-$repo_root/ansible/kubernetes/inventory.yaml}"

drain_timeout="${DRAIN_TIMEOUT:-300}"
detach_timeout="${DETACH_TIMEOUT:-300}"
boot_timeout="${BOOT_TIMEOUT:-900}"
ready_timeout="${READY_TIMEOUT:-600}"
longhorn_timeout="${LONGHORN_TIMEOUT:-3600}"
poll="${POLL_INTERVAL:-5}"

confirm() { read -r -p "$* [y/N] " a; [[ "$a" =~ ^[Yy]$ ]]; }

hostvar() { jq -r --arg h "$1" --arg k "$2" '._meta.hostvars[$h][$k] // empty' <<<"$inventory_json"; }

node_name() { local n; n="$(hostvar "$1" k8s_node_name)"; echo "${n:-$1}"; }

remote() { ssh -o ConnectTimeout=10 -o BatchMode=yes -o StrictHostKeyChecking=accept-new "$target" "$@"; }

boot_id() { remote cat /proc/sys/kernel/random/boot_id 2>/dev/null || true; }

is_rebooted() {
  local id
  id="$(boot_id)"
  [[ -n "$id" && "$id" != "$old_boot_id" ]]
}

is_ready() {
  local status
  status="$(kubectl get node "$1" -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
  [[ "$status" == "True" ]]
}

volumes_attached_to() {
  kubectl -n longhorn-system get volumes.longhorn.io -o json 2>/dev/null \
    | jq -r --arg n "$1" '.items[] | select(.status.currentNodeID == $n) | "\(.status.kubernetesStatus.namespace)/\(.status.kubernetesStatus.pvcName)"'
}

no_volumes_attached() { [[ -z "$(volumes_attached_to "$1")" ]]; }

longhorn_healthy() {
  local bad
  bad="$(kubectl -n longhorn-system get volumes.longhorn.io -o json 2>/dev/null \
    | jq '[.items[] | select(.status.state == "attached" and .status.robustness != "healthy")] | length' || echo 1)"
  [[ "$bad" == "0" ]]
}

wait_until() {
  local label="$1" timeout="$2"; shift 2
  local waited=0
  until "$@"; do
    (( waited >= timeout )) && { printf '\n'; return 1; }
    sleep "$poll"; waited=$(( waited + poll ))
    printf '  ... waiting for %s (%ds/%ds)\r' "$label" "$waited" "$timeout"
  done
  printf '\n'
}

for bin in jq ansible-inventory kubectl ssh; do
  command -v "$bin" >/dev/null || { err "missing required command: $bin"; exit 1; }
done
[[ -f "$inventory" ]] || { err "inventory not found: $inventory"; exit 1; }

inventory_json="$(ansible-inventory -i "$inventory" --list)"

mapfile -t hosts < <(jq -r '(.agent.hosts // []) + (.control.hosts // []) | .[]' <<<"$inventory_json")

if [[ $# -gt 0 ]]; then
  requested=" $* "
  selected=()
  for h in "${hosts[@]}"; do
    [[ "$requested" == *" $h "* ]] && selected+=("$h")
  done
  hosts=("${selected[@]}")
  [[ ${#hosts[@]} -gt 0 ]] || { err "no agent/control hosts matched: $*"; exit 1; }
fi

info "Reboot plan (from $inventory):"
for h in "${hosts[@]}"; do
  printf '   %-16s  %s@%s  (node: %s)\n' \
    "$h" "$(hostvar "$h" ansible_user)" "$(hostvar "$h" ansible_host)" "$(node_name "$h")"
done
echo
confirm "Reboot these ${#hosts[@]} node(s), one at a time?" || { info "Aborted."; exit 0; }

total=${#hosts[@]}
position=0

for h in "${hosts[@]}"; do
  position=$(( position + 1 ))

  user="$(hostvar "$h" ansible_user)"
  addr="$(hostvar "$h" ansible_host)"
  node="$(node_name "$h")"
  target="$user@$addr"

  echo
  info "[$position/$total] $h ($node) via $target"

  if ! kubectl get node "$node" >/dev/null 2>&1; then
    warn "node '$node' is not in the cluster — skipping."
    continue
  fi

  if ! wait_until "Longhorn volumes to be healthy" "$longhorn_timeout" longhorn_healthy; then
    err "Longhorn volumes are not healthy."
    confirm "Reboot $node anyway?" || { err "Stopping."; exit 1; }
  fi

  old_boot_id="$(boot_id)"
  [[ -n "$old_boot_id" ]] || { err "could not read boot id from $target."; exit 1; }

  info "Cordoning and draining $node..."
  kubectl cordon "$node"
  if ! kubectl drain "$node" --ignore-daemonsets --delete-emptydir-data \
      --pod-selector='longhorn.io/component!=instance-manager' --timeout="${drain_timeout}s"; then
    warn "drain did not finish (a PDB is probably blocking — e.g. a CNPG primary; switch it over first)."
    confirm "Reboot $node without a complete drain?" || { kubectl uncordon "$node"; err "Stopping."; exit 1; }
  fi

  if ! wait_until "Longhorn volumes to detach from $node" "$detach_timeout" no_volumes_attached "$node"; then
    warn "still attached to $node:"
    volumes_attached_to "$node" | sed 's/^/     /'
    confirm "Reboot $node with these attached?" || { kubectl uncordon "$node"; err "Stopping."; exit 1; }
  fi

  reboot_cmd="sudo reboot"
  [[ "$user" == "root" ]] && reboot_cmd="reboot"

  info "Rebooting (the SSH connection dropping is expected)..."
  remote "$reboot_cmd" || true

  if ! wait_until "$node to boot (new boot id)" "$boot_timeout" is_rebooted; then
    err "$node did not come back with a new boot id within ${boot_timeout}s."
    confirm "Continue to the next node anyway (leaving $node cordoned)?" || { err "Stopping."; exit 1; }
    continue
  fi

  if ! wait_until "$node to come back Ready" "$ready_timeout" is_ready "$node"; then
    err "$node did not become Ready within ${ready_timeout}s."
    confirm "Continue to the next node anyway (leaving $node cordoned)?" || { err "Stopping."; exit 1; }
    continue
  fi

  kubectl uncordon "$node"
  info "${green}$node is Ready.${reset}"
  kubectl get node "$node" -o wide 2>/dev/null || true

  if ! wait_until "Longhorn volumes to be healthy" "$longhorn_timeout" longhorn_healthy; then
    warn "Longhorn volumes are still not healthy after ${longhorn_timeout}s."
  fi

  if (( position < total )); then
    echo
    confirm "Continue to the next node ($(( position + 1 ))/$total)?" || { info "Stopped after $h."; exit 0; }
  fi
done

echo
info "${green}Done — all selected nodes rebooted and Ready.${reset}"
