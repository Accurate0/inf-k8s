#!/usr/bin/env bash
set -euo pipefail

if [ $# -lt 3 ]; then
  echo "usage: $0 <namespace> <name> <slug[:path]>..." >&2
  exit 1
fi

: "${BAO_TOKEN:?BAO_TOKEN must be set}"

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
    --data-urlencode "expandSecretReferences=true" \
    --data-urlencode "include_imports=true" |
    jq '([.imports[]?.secrets[]] + .secrets) | map({key: .secretKey, value: .secretValue}) | from_entries')

  echo "$slug:$path -> $(jq length <<<"$secrets") keys" >&2
  merged=$(jq -n --argjson a "$merged" --argjson b "$secrets" '$a + $b')
done

pod=$(kubectl -n openbao get pod -l openbao-active=true -o name)
kubectl -n openbao exec -i "$pod" -- env BAO_TOKEN="$BAO_TOKEN" bao kv put "kv/$namespace/$name" - <<<"$merged"
echo "wrote $(jq length <<<"$merged") keys to kv/$namespace/$name" >&2
