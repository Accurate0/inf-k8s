#!/usr/bin/env bash
set -euo pipefail

# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmpl_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/templates/new-project"
cd "$repo_root"

routes_chart_version="${ROUTES_CHART_VERSION:-2.1.0}"
secrets_chart_version="${SECRETS_CHART_VERSION:-0.3.0}"
gateway_name="${GATEWAY_NAME:-public-gateway}"

render() {
  sed \
    -e "s|@@NAME@@|$name|g" \
    -e "s|@@LOCATION@@|$location|g" \
    -e "s|@@DIR@@|$dir|g" \
    -e "s|@@IMAGE@@|$image|g" \
    -e "s|@@IMAGE_TAG@@|$image_tag|g" \
    -e "s|@@REPLICAS@@|$replicas|g" \
    -e "s|@@CONTAINER_PORT@@|$container_port|g" \
    -e "s|@@SERVICE_PORT@@|$service_port|g" \
    -e "s|@@REPO_URL@@|$repo_url|g" \
    -e "s|@@GATEWAY@@|$gateway_name|g" \
    -e "s|@@CHART_VERSION@@|$routes_chart_version|g" \
    -e "s|@@SECRETS_CHART_VERSION@@|$secrets_chart_version|g" \
    -e "s|@@PROJECT_SLUG@@|$secret_project_slug|g" \
    -e "s|@@ROUTE_NAME@@|$name-route|g" \
    "$1" \
  | awk -v hosts="$hostnames_block" -v labels="$labels_block" \
        -v ports="$ports_block" -v resources="$resources_block" \
        -v route="$route_source_block" -v secrets="$secrets_source_block" '
      /@@HOSTNAMES@@/       { if (hosts != "")     print hosts;     next }
      /@@LABELS@@/          { if (labels != "")    print labels;    next }
      /@@PORTS@@/           { if (ports != "")     print ports;     next }
      /@@RESOURCES@@/       { if (resources != "") print resources; next }
      /@@ROUTE_SOURCE@@/    { if (route != "")     print route;     next }
      /@@SECRETS_SOURCE@@/  { if (secrets != "")   print secrets;   next }
      { print }
    '
}

build_resources() {
  printf '  - deployment.yaml\n  - namespace.yaml\n'
  $has_service && printf '  - service.yaml\n'
  return 0
}

generate_files() {
  mkdir -p "$dir/manifests"

  hostnames_block=""
  labels_block=""
  ports_block=""
  route_source_block=""
  secrets_source_block=""
  if $is_public; then
    hostnames_block="$(for h in $hosts; do printf '                - %s\n' "$h"; done)"
    labels_block="$({ printf '  labels:\n'; for h in $hosts; do printf '    gateway.inf-k8s.net/%s: "true"\n' "${h//./-}"; done; })"
  fi
  $has_service && ports_block="$(printf '          ports:\n            - containerPort: %s' "$container_port")"
  resources_block="$(build_resources)"

  $is_public && route_source_block="$(render "$tmpl_dir/source.route.yaml.tmpl")"
  $with_secrets && secrets_source_block="$(render "$tmpl_dir/source.secrets.yaml.tmpl")"

  render "$tmpl_dir/application.yaml.tmpl"    >"$dir/application.yaml"
  render "$tmpl_dir/namespace.yaml.tmpl"      >"$dir/manifests/namespace.yaml"
  render "$tmpl_dir/deployment.yaml.tmpl"     >"$dir/manifests/deployment.yaml"
  $has_service && render "$tmpl_dir/service.yaml.tmpl" >"$dir/manifests/service.yaml"
  render "$tmpl_dir/kustomization.yaml.tmpl"  >"$dir/manifests/kustomization.yaml"
  return 0
}

[[ "${BASH_SOURCE[0]}" == "${0}" ]] || return 0

for bin in gum kustomize; do
  command -v "$bin" >/dev/null || { err "missing required command: $bin"; exit 1; }
done

gum style --border rounded --padding "0 1" --border-foreground 212 "New ArgoCD project scaffolder"

name="$(gum input --prompt "App name (kebab-case): " --placeholder "my-app")"
[[ "$name" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$ ]] || { err "invalid name (must be lowercase kebab-case): $name"; exit 1; }

location="$(gum choose --header "Where does it live?" projects platform-services)"
dir="$location/$name"
[[ -e "$dir" ]] && { gum confirm "$dir already exists — overwrite?" || { info "Aborted."; exit 0; }; }

repo_url="$(gum input --prompt "Upstream repo: " --value "https://github.com/Accurate0/$name")"
image="$(gum input --prompt "Image: " --value "ghcr.io/accurate0/$name")"
image_tag="$(gum input --prompt "Image tag (newTag): " --value "latest")"
replicas="$(gum input --prompt "Replicas: " --value "1")"

kind="$(gum choose --header "Workload exposure" \
  "Public (HTTP route + Service)" \
  "Internal (Service only)" \
  "Worker (no Service)")"

has_service=true
is_public=false
case "$kind" in
  "Public"*)   is_public=true ;;
  "Internal"*) ;;
  "Worker"*)   has_service=false ;;
esac

container_port=""
service_port=""
hosts=""
if $has_service; then
  container_port="$(gum input --prompt "Container port: " --value "3000")"
  service_port="$(gum input --prompt "Service port: " --value "80")"
fi
if $is_public; then
  hosts="$(gum input --prompt "Hostnames (space-separated): " --value "$name.anurag.sh")"
  [[ -n "$hosts" ]] || { err "public apps need at least one hostname"; exit 1; }
fi

with_secrets=false
secret_project_slug=""
if gum confirm "Add an Infisical ExternalSecret?"; then
  with_secrets=true
  secret_project_slug="$(gum input --prompt "Infisical projectSlug: " --placeholder "my-app-xxxx")"
fi

summary="app:       $name
location:  $dir
repo:      $repo_url
image:     $image:$image_tag
replicas:  $replicas
exposure:  $kind"
$has_service && summary+="
ports:     $container_port -> svc $service_port"
$is_public && summary+="
hosts:     $hosts"
$with_secrets && summary+="
secrets:   Infisical ($secret_project_slug)"

gum style --border normal --padding "0 1" "$summary"
gum confirm "Generate these files?" || { info "Aborted."; exit 0; }

generate_files

info "${green}Created $dir${reset}"
gum style --border normal --padding "0 1" "$(find "$dir" -type f | sort)"

if gum confirm "Validate the generated manifests now?"; then
  ./scripts/validate-manifests.sh "$dir/manifests" "$dir/application.yaml"
fi

cat <<EOF

Next:
  git add $dir
  just $location apply $name      # or: kubectl apply -f $dir/application.yaml
EOF
