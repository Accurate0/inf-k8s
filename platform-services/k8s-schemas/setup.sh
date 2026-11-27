#!/bin/bash
set -euo pipefail

DIR="$(dirname "$0")"
VENV="$RUNNER_TEMP/k8s-schemas-venv"

if ! python3 -m venv "$VENV"; then
  apt-get update
  apt-get install -y python3-venv
  python3 -m venv "$VENV"
fi

"$VENV/bin/pip" install --no-cache-dir -r "$DIR/requirements.txt"
"$VENV/bin/pip" install --no-cache-dir --no-deps openapi2jsonschema

echo "$VENV/bin" >> "$GITHUB_PATH"
