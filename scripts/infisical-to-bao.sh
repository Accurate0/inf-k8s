#!/usr/bin/env bash
set -euo pipefail

recursive=false
azure=false
while getopts "ra" opt; do
  case $opt in
    r) recursive=true ;;
    a) azure=true ;;
    *) exit 1 ;;
  esac
done
shift $((OPTIND - 1))

if [ $# -lt 3 ]; then
  echo "usage: $0 [-r] [-a] <namespace> <name> <slug[:path]>..." >&2
  exit 1
fi

if [ "$azure" = false ]; then
  : "${BAO_TOKEN:?BAO_TOKEN must be set}"
fi

namespace=$1
name=$2
shift 2

infisical_url=${INFISICAL_URL:-https://vault.inf-k8s.net}

client_id=$(kubectl -n infisical get secret universal-auth-credentials -o jsonpath='{.data.clientId}' | base64 -d)
client_secret=$(kubectl -n infisical get secret universal-auth-credentials -o jsonpath='{.data.clientSecret}' | base64 -d)

token=$(curl -sSf -X POST "$infisical_url/api/v1/auth/universal-auth/login" \
  -H 'Content-Type: application/json' \
  -d "$(jq -n --arg id "$client_id" --arg secret "$client_secret" '{clientId: $id, clientSecret: $secret}')" |
  jq -r .accessToken)

merged='{}'
for source in "$@"; do
  slug=${source%%:*}
  path=/
  if [[ $source == *:* ]]; then
    path=${source#*:}
  fi

  secrets=$(curl -sSf -G "$infisical_url/api/v3/secrets/raw" \
    -H "Authorization: Bearer $token" \
    --data-urlencode "workspaceSlug=$slug" \
    --data-urlencode "environment=prod" \
    --data-urlencode "secretPath=$path" \
    --data-urlencode "recursive=$recursive" \
    --data-urlencode "expandSecretReferences=true" \
    --data-urlencode "include_imports=true" |
    jq '([.imports[]?.secrets[]] + .secrets) | map({key: .secretKey, value: .secretValue}) | from_entries')

  echo "$slug:$path -> $(jq length <<<"$secrets") keys" >&2
  merged=$(jq -n --argjson a "$merged" --argjson b "$secrets" '$a + $b')
done

if [ "$azure" = true ]; then
  az keyvault secret set --vault-name k8s-shared-vault --name "$name" --value "$(jq -c . <<<"$merged")" --output none
  echo "wrote $(jq length <<<"$merged") keys to azure k8s-shared-vault/$name" >&2
  exit 0
fi

key="kv/$namespace/$name"
if [ "$name" = "$namespace" ]; then
  key="kv/$namespace"
fi

pod=$(kubectl -n openbao get pod -l openbao-active=true -o name)
kubectl -n openbao exec -i "$pod" -- env BAO_TOKEN="$BAO_TOKEN" bao kv put "$key" - <<<"$merged"
echo "wrote $(jq length <<<"$merged") keys to $key" >&2
