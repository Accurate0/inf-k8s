#!/bin/sh
# Re-pin build.prebuilt sha256 values in an upstream package's ortzi.yaml for its current version.
set -eu

dir="${1:?usage: upstream-digests.sh packages/upstream/<name>}"
manifest="$dir/ortzi.yaml"
version=$(sed -n 's/^  version: *//p' "$manifest" | head -n1)
[ -n "$version" ] || { echo "no package.version in $manifest" >&2; exit 1; }

if command -v sha256sum >/dev/null; then sha="sha256sum"; else sha="shasum -a 256"; fi

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

awk -v version="$version" -v sha="$sha" '
  /^ *url: / {
    url = $2
    gsub(/\$\{version\}/, version, url)
    cmd = "curl -fsSL \"" url "\" | " sha
    digest = ""
    if ((cmd | getline line) > 0) split(line, parts, " ")
    close(cmd)
    digest = parts[1]
    if (digest !~ /^[0-9a-f]{64}$/) { print "failed to hash " url > "/dev/stderr"; exit 1 }
  }
  /^ *sha256: / && digest != "" {
    sub(/sha256: .*/, "sha256: " digest)
    digest = ""
  }
  { print }
' "$manifest" > "$tmp"

cat "$tmp" > "$manifest"
echo "pinned $manifest at $version"
