#!/usr/bin/env bash
#
# Reboot k8s nodes one at a time: reboot over SSH, wait for the node to come
# back Ready, then ask before moving on. Connection details come from the
# ansible inventory. Agents are done first, control/etcd nodes last.
#
#   scripts/rolling-reboot.sh                 # all nodes
#   scripts/rolling-reboot.sh k8s-optiplex-2  # only the named host(s)

set -euo pipefail

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
inventory="${INVENTORY:-$repo_root/ansible/inventory.yaml}"

notready_timeout="${NOTREADY_TIMEOUT:-120}"
ready_timeout="${READY_TIMEOUT:-600}"
poll="${POLL_INTERVAL:-5}"

confirm() { read -r -p "$* [y/N] " a; [[ "$a" =~ ^[Yy]$ ]]; }

hostvar() { jq -r --arg h "$1" --arg k "$2" '._meta.hostvars[$h][$k] // empty' <<<"$inventory_json"; }

node_name() { local n; n="$(hostvar "$1" k8s_node_name)"; echo "${n:-$1}"; }

is_ready() {
  local status
  status="$(kubectl get node "$1" -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
  [[ "$status" == "True" ]]
}
is_down() { ! is_ready "$1"; }

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

  echo
  info "[$position/$total] $h ($node) via $user@$addr"

  if ! kubectl get node "$node" >/dev/null 2>&1; then
    warn "node '$node' is not in the cluster — skipping."
    continue
  fi

  reboot_cmd="sudo reboot"
  [[ "$user" == "root" ]] && reboot_cmd="reboot"

  info "Rebooting (the SSH connection dropping is expected)..."
  ssh -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new "$user@$addr" "$reboot_cmd" || true

  if ! wait_until "$node to go down" "$notready_timeout" is_down "$node"; then
    warn "$node never went NotReady (quick reboot?) — checking that it's healthy anyway."
  fi

  if ! wait_until "$node to come back Ready" "$ready_timeout" is_ready "$node"; then
    err "$node did not become Ready within ${ready_timeout}s."
    confirm "Continue to the next node anyway?" || { err "Stopping."; exit 1; }
    continue
  fi

  info "${green}$node is Ready.${reset}"
  kubectl get node "$node" -o wide 2>/dev/null || true

  if (( position < total )); then
    echo
    confirm "Continue to the next node ($(( position + 1 ))/$total)?" || { info "Stopped after $h."; exit 0; }
  fi
done

echo
info "${green}Done — all selected nodes rebooted and Ready.${reset}"
