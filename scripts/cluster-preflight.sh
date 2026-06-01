#!/usr/bin/env bash
set -euo pipefail

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

quiet=false
[[ "${1:-}" == "--quiet" || "${1:-}" == "-q" ]] && quiet=true

section() { printf '\n%s== %s ==%s\n' "$bold" "$*" "$reset"; }
ok()   { $quiet || printf '  %s✓%s %s\n' "$green" "$reset" "$*"; }
warn() { printf '  %s! %s%s\n' "$yellow" "$*" "$reset"; warns=$(( warns + 1 )); }
fail() { printf '  %s✗ %s%s\n' "$red" "$*" "$reset"; fails=$(( fails + 1 )); }

for bin in kubectl jq; do
  command -v "$bin" >/dev/null || { err "missing required command: $bin"; exit 1; }
done

warns=0
fails=0

have_resource() { kubectl get "$1" -A >/dev/null 2>&1; }

section "Nodes"
nodes_json="$(kubectl get nodes -o json)"
while IFS=$'\t' read -r name ready pressures; do
  if [[ "$ready" != "True" ]]; then
    fail "$name not Ready (Ready=$ready)"
  elif [[ -n "$pressures" ]]; then
    warn "$name under pressure: $pressures"
  else
    ok "$name Ready"
  fi
done < <(jq -r '
  .items[] | [
    .metadata.name,
    ([.status.conditions[] | select(.type=="Ready") | .status][0] // "Unknown"),
    ([.status.conditions[]
      | select(.type|test("Pressure$")) | select(.status=="True") | .type]
      | join(","))
  ] | @tsv' <<<"$nodes_json")

section "Pods"
pods_json="$(kubectl get pods -A -o json)"
bad_pods="$(jq -r '
  .items[]
  | (.status.phase) as $phase
  | (.status.containerStatuses // []) as $cs
  | ($cs[] | .state.waiting.reason // empty) as $waiting
  | select(
      ($phase != "Running" and $phase != "Succeeded")
      or ($waiting | test("CrashLoopBackOff|ImagePullBackOff|ErrImagePull|CreateContainerError"))
    )
  | "\(.metadata.namespace)/\(.metadata.name) [\($phase)\(if $waiting != "" then " " + $waiting else "" end)]"
' <<<"$pods_json" | sort -u)"
if [[ -n "$bad_pods" ]]; then
  while IFS= read -r line; do fail "$line"; done <<<"$bad_pods"
else
  ok "all pods Running/Succeeded"
fi

section "Longhorn volumes"
if have_resource volumes.longhorn.io; then
  bad_vols="$(kubectl get volumes.longhorn.io -A -o json | jq -r '
    .items[] | select((.status.robustness // "unknown") != "healthy")
    | "\(.metadata.name) [robustness=\(.status.robustness // "?") state=\(.status.state // "?")]"')"
  if [[ -n "$bad_vols" ]]; then
    while IFS= read -r line; do warn "$line"; done <<<"$bad_vols"
  else
    ok "all volumes healthy"
  fi
else
  ok "longhorn CRD not present — skipped"
fi

section "Certificates"
if have_resource certificates.cert-manager.io; then
  bad_certs="$(kubectl get certificates.cert-manager.io -A -o json | jq -r '
    .items[]
    | ([.status.conditions[]? | select(.type=="Ready") | .status][0] // "Unknown") as $ready
    | select($ready != "True")
    | "\(.metadata.namespace)/\(.metadata.name) [Ready=\($ready)]"')"
  if [[ -n "$bad_certs" ]]; then
    while IFS= read -r line; do warn "$line"; done <<<"$bad_certs"
  else
    ok "all certificates Ready"
  fi
else
  ok "cert-manager CRD not present — skipped"
fi

section "Summary"
if (( fails > 0 )); then
  printf '%s%d failure(s), %d warning(s).%s\n' "$red" "$fails" "$warns" "$reset"
  exit 1
elif (( warns > 0 )); then
  printf '%s%d warning(s), no failures.%s\n' "$yellow" "$warns" "$reset"
  exit 0
fi
printf '%scluster looks healthy.%s\n' "$green" "$reset"
