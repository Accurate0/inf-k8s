#!/usr/bin/env bash
set -euo pipefail

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

namespace="${ARGOCD_NAMESPACE:-argocd}"
unhealthy_only=false

usage() {
  cat <<'EOF'
Usage: argo-status.sh [--unhealthy] [-n NAMESPACE]
  --unhealthy        only show apps that aren't Synced + Healthy
  -n, --namespace    ArgoCD namespace (default: argocd)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --unhealthy) unhealthy_only=true; shift ;;
    -n|--namespace) namespace="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

for bin in kubectl jq; do
  command -v "$bin" >/dev/null || { err "missing required command: $bin"; exit 1; }
done

apps_json="$(kubectl get applications -n "$namespace" -o json)"

colour_for() {
  local health="$1" sync="$2"
  if [[ "$health" == "Degraded" || "$health" == "Missing" || "$health" == "Unknown" ]]; then
    printf '%s' "$red"
  elif [[ "$health" == "Healthy" && "$sync" == "Synced" ]]; then
    printf '%s' "$green"
  else
    printf '%s' "$yellow"
  fi
}

printf '%s%-28s %-18s %-10s %-10s %-10s%s\n' \
  "$bold" "NAME" "PROJECT" "SYNC" "HEALTH" "REVISION" "$reset"

bad_apps=0

while IFS=$'\t' read -r name project sync health revision; do
  is_clean=false
  [[ "$health" == "Healthy" && "$sync" == "Synced" ]] && is_clean=true

  if [[ "$health" == "Degraded" || "$health" == "Missing" || "$health" == "Unknown" ]]; then
    bad_apps=$(( bad_apps + 1 ))
  fi

  $unhealthy_only && $is_clean && continue

  c="$(colour_for "$health" "$sync")"
  printf '%s%-28s%s %-18s %-10s %-10s %-10s\n' \
    "$c" "$name" "$reset" "$project" "$sync" "$health" "${revision:0:8}"

  if ! $is_clean; then
    jq -r --arg n "$name" '
      .items[] | select(.metadata.name == $n) | .status.resources // [] | .[]
      | select((.health.status // "Healthy") != "Healthy" or (.status // "Synced") != "Synced")
      | "      - \(.kind)/\(.name) [sync=\(.status // "?") health=\(.health.status // "?")]"
    ' <<<"$apps_json"
  fi
done < <(jq -r '
  .items[] | [
    .metadata.name,
    (.spec.project // "default"),
    (.status.sync.status // "Unknown"),
    (.status.health.status // "Unknown"),
    (.status.sync.revision // .status.sync.revisions[0]? // "")
  ] | @tsv' <<<"$apps_json")

echo
if (( bad_apps > 0 )); then
  printf '%s%d app(s) Degraded/Missing/Unknown.%s\n' "$red" "$bad_apps" "$reset"
  exit 1
fi
printf '%sall apps healthy.%s\n' "$green" "$reset"
