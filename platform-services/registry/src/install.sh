#!/bin/sh
# curl -fsSL https://registry.ortzi.cloud/install.sh | ORTZI_TOKEN=... sh
set -eu

registry="${ORTZI_REGISTRY_URL:-https://registry.ortzi.cloud}"
audience="${ORTZI_AUDIENCE:-registry.ortzi.cloud}"

fail() {
  echo "ortzi install: $*" >&2
  exit 1
}

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=linux-amd64 ;;
  Darwin-arm64) target=darwin-arm64 ;;
  *) fail "no ortzi build for $(uname -s) $(uname -m)" ;;
esac

if [ -n "${ORTZI_INSTALL_DIR:-}" ]; then
  dir="$ORTZI_INSTALL_DIR"
elif [ "$(id -u)" = 0 ]; then
  dir=/usr/local/bin
else
  dir="$HOME/.local/bin"
fi

command -v curl >/dev/null || fail "curl is required"

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

fetch() {
  printf 'header = "Authorization: Bearer %s"\n' "$token" | curl -fsSL -K - "$@"
}

release="$(fetch "$registry/cli/v1/packages/ortzi/latest?target=$target")" ||
  fail "could not read the latest ortzi from $registry"
version="$(printf '%s' "$release" | sed -n 's/^[^}]*"version":"\([^"]*\)".*/\1/p')"
sha256="$(printf '%s' "$release" | sed -n "s/.*\"target\":\"$target\",\"sha256\":\"\([0-9a-f]\{64\}\)\".*/\1/p")"
[ -n "$version" ] && [ -n "$sha256" ] || fail "unexpected response from $registry"

mkdir -p "$dir"
tmp="$(mktemp "$dir/.ortzi.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
fetch -o "$tmp" "$registry/cli/v1/packages/ortzi/versions/$version/artifacts/$target" ||
  fail "could not download ortzi $version"

if command -v sha256sum >/dev/null; then
  actual="$(sha256sum "$tmp" | cut -d' ' -f1)"
else
  actual="$(shasum -a 256 "$tmp" | cut -d' ' -f1)"
fi
[ "$actual" = "$sha256" ] || fail "checksum mismatch for ortzi $version"

chmod 755 "$tmp"
mv "$tmp" "$dir/ortzi"
trap - EXIT

echo "installed ortzi $version to $dir/ortzi"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "add $dir to your PATH" ;;
esac
