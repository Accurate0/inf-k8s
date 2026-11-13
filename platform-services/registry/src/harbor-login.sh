#!/bin/sh
# curl -fsSL https://registry.anurag.sh/harbor-login.sh | sh -s -- containers:push charts:pull
set -eu

registry="${ORTZI_REGISTRY_URL:-https://registry.anurag.sh}"
audience="${ORTZI_AUDIENCE:-registry.anurag.sh}"
config_dir="${DOCKER_CONFIG:-$HOME/.docker}"

fail() {
  echo "harbor login: $*" >&2
  exit 1
}

[ "$#" -gt 0 ] || fail "usage: harbor-login.sh <project>:<pull|push>[,<pull|push>] ..."
command -v curl >/dev/null || fail "curl is required"

access=""
for spec in "$@"; do
  project="${spec%%:*}"
  actions="${spec#*:}"
  [ "$project" != "$spec" ] && [ -n "$project" ] && [ -n "$actions" ] ||
    fail "$spec is not <project>:<actions>"
  quoted="$(printf '%s' "$actions" | sed 's/[^a-z,]//g; s/\([a-z][a-z]*\)/"\1"/g')"
  access="${access:+$access,}{\"project\":\"$project\",\"actions\":[$quoted]}"
done

token="${ORTZI_TOKEN:-}"
if [ -z "$token" ] && [ -n "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ]; then
  [ -n "${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-}" ] ||
    fail "ACTIONS_ID_TOKEN_REQUEST_TOKEN is unset; add \`enable-openid-connect: true\` to the workflow"
  case "$ACTIONS_ID_TOKEN_REQUEST_URL" in
    *\?*) separator='&' ;;
    *) separator='?' ;;
  esac
  token="$(printf 'header = "Authorization: bearer %s"\n' "$ACTIONS_ID_TOKEN_REQUEST_TOKEN" |
    curl -fsSL -K - "${ACTIONS_ID_TOKEN_REQUEST_URL}${separator}audience=${audience}" |
    sed -n 's/.*"value":"\([^"]*\)".*/\1/p')"
fi
[ -n "$token" ] || fail "set ORTZI_TOKEN, or run in forgejo actions with \`enable-openid-connect: true\`"

response="$(printf 'header = "Authorization: Bearer %s"\n' "$token" |
  curl -sSL -K - -X POST -H 'content-type: application/json' \
    -d "{\"access\":[$access]}" -w '\n%{http_code}' "$registry/api/v1/harbor/token")" ||
  fail "could not reach $registry"
status="$(printf '%s' "$response" | tail -n 1)"
body="$(printf '%s' "$response" | sed '$d')"
[ "$status" = 200 ] || fail "exchange refused ($status): $body"

field() {
  printf '%s' "$body" | sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p"
}
host="$(field registry)"
username="$(field username)"
password="$(field password)"
[ -n "$host" ] && [ -n "$username" ] && [ -n "$password" ] || fail "unexpected response from $registry"

auth="$(printf '%s:%s' "$username" "$password" | base64 | tr -d '\n')"
mkdir -p "$config_dir"
config="$config_dir/config.json"
if [ -s "$config" ]; then
  command -v jq >/dev/null || fail "$config exists and jq is not installed to merge into it"
  jq --arg host "$host" --arg auth "$auth" '.auths[$host] = {auth: $auth}' "$config" >"$config.tmp"
  mv "$config.tmp" "$config"
else
  printf '{"auths":{"%s":{"auth":"%s"}}}\n' "$host" "$auth" >"$config"
fi
chmod 600 "$config"

if [ -n "${GITHUB_ENV:-}" ]; then
  echo "HARBOR_ROBOT=$username" >>"$GITHUB_ENV"
fi
echo "logged in to $host as $username (expires $(field expires_at))"
