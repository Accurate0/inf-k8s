#!/usr/bin/env bash
set -euo pipefail

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

script_path="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

schema_location="${SCHEMA_LOCATION:-}"
if [[ -z "$schema_location" ]]; then
  schema_location='https://k8s-schemas.anurag.sh/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json'
fi

cache="${KUBECONFORM_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/inf-k8s-schemas}"
mkdir -p "$cache"

read -r -a extra_flags <<<"${KUBECONFORM_FLAGS:-}"
kc=(kubeconform -strict -summary -ignore-missing-schemas -cache "$cache"
    -schema-location "$schema_location"
    -schema-location default
    "${extra_flags[@]}")

if [[ "${1:-}" == "__worker" ]]; then
  path="$3"
  if out="$(kustomize build "$path" 2>&1 | "${kc[@]}" - 2>&1)"; then
    printf '  %s✓%s kustomize %s\n' "$green" "$reset" "$path"
  else
    printf '  %s✗ kustomize %s%s\n%s\n' "$red" "$path" "$reset" "$out"; exit 1
  fi
  exit 0
fi

for bin in kubeconform kustomize; do
  command -v "$bin" >/dev/null || { err "missing required command: $bin"; exit 1; }
done

kustomize_dirs=()
app_files=()

if [[ $# -gt 0 ]]; then
  declare -A seen_dir=()
  declare -A seen_file=()
  for path in "$@"; do
    if [[ -d "$path" ]]; then
      if [[ -f "$path/kustomization.yaml" || -f "$path/kustomization.yml" ]]; then
        seen_dir["$path"]=1
      else
        err "skipping (no kustomization in dir): $path"
      fi
      continue
    fi
    [[ -f "$path" ]] || { err "skipping (not found): $path"; continue; }
    d="$(dirname "$path")"
    kdir=""
    while [[ "$d" != "." && "$d" != "/" ]]; do
      if [[ -f "$d/kustomization.yaml" || -f "$d/kustomization.yml" ]]; then kdir="$d"; break; fi
      d="$(dirname "$d")"
    done
    if [[ -n "$kdir" ]]; then seen_dir["$kdir"]=1; else seen_file["$path"]=1; fi
  done
  (( ${#seen_dir[@]} )) && kustomize_dirs=("${!seen_dir[@]}")
  (( ${#seen_file[@]} )) && app_files=("${!seen_file[@]}")
else
  while IFS= read -r kfile; do kustomize_dirs+=("$(dirname "$kfile")"); done \
    < <(find system-components platform-services projects -name kustomization.yaml 2>/dev/null | sort)
  while IFS= read -r afile; do app_files+=("$afile"); done \
    < <(find system-components platform-services projects \
        \( -name '*.application.yaml' -o -name 'application.yaml' \) 2>/dev/null | sort)
fi

jobs="${VALIDATE_JOBS:-8}"
rc=0

if (( ${#app_files[@]} )); then
  if out="$("${kc[@]}" "${app_files[@]}" 2>&1)"; then
    for f in "${app_files[@]}"; do printf '  %s✓%s manifest  %s\n' "$green" "$reset" "$f"; done
  else
    printf '%s\n' "$out"
    rc=1
  fi
fi

if (( ${#kustomize_dirs[@]} )); then
  "$script_path" __worker dir "${kustomize_dirs[0]}" || rc=1
  if (( ${#kustomize_dirs[@]} > 1 )); then
    printf '%s\0' "${kustomize_dirs[@]:1}" \
      | xargs -0 -r -P "$jobs" -n1 "$script_path" __worker dir || rc=1
  fi
fi

echo
if (( rc != 0 )); then
  err "validation failed."
  exit 1
fi
printf '%sall manifests valid.%s\n' "$green" "$reset"
