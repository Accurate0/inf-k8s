#!/bin/bash
set -euo pipefail

: "${K8S_VERSION:?K8S_VERSION must be set (e.g. release-1.30, master, v1.30.0)}"

DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$DIR/../.." && pwd)"
WORK="${RUNNER_TEMP:-$(mktemp -d)}/k8s-schemas"
rm -rf "$WORK" "${HOME}/.datree"
mkdir -p "$WORK"

CATALOG_DIR="$WORK/CRDs-catalog"
git clone --depth 1 https://github.com/datreeio/CRDs-catalog.git "$CATALOG_DIR"

bash "$CATALOG_DIR/Utilities/crd-extractor.sh"

OUTPUT_DIR="${HOME}/.datree/crdSchemas"
if [ ! -d "$OUTPUT_DIR" ]; then
  echo "ERROR: CRD schema output directory not found at $OUTPUT_DIR"
  exit 1
fi

echo "Fetching k8s swagger.json for ${K8S_VERSION}..."
SWAGGER_FILE="$WORK/k8s-swagger.json"
curl -fsSL \
  "https://raw.githubusercontent.com/kubernetes/kubernetes/${K8S_VERSION}/api/openapi-spec/swagger.json" \
  -o "$SWAGGER_FILE"

echo "Generating built-in k8s schemas..."
BUILTIN_TMP_DIR="$WORK/k8s-builtin-schemas"
openapi2jsonschema --kubernetes --stand-alone --expanded -o "$BUILTIN_TMP_DIR" "$SWAGGER_FILE"
python3 "$DIR/reorganize-schemas.py" "$BUILTIN_TMP_DIR" "$OUTPUT_DIR"

echo "Publishing helm chart values schemas..."
python3 "$DIR/patch-helmrelease-schema.py" "$OUTPUT_DIR" "$ROOT/charts"
cp "$ROOT/packages/ortzi-app/app.schema.json" "$OUTPUT_DIR/app.json"

echo "Building index.json..."
python3 "$DIR/build-index.py" "$OUTPUT_DIR"

echo "Uploading schemas to S3..."
aws s3 sync "$OUTPUT_DIR" "s3://${S3_BUCKET}/" \
  --endpoint-url "${S3_ENDPOINT}" \
  --delete \
  --exclude "master-standalone/*" \
  --exclude "version.json"

aws s3 cp "$OUTPUT_DIR/version.json" "s3://${S3_BUCKET}/version.json" \
  --endpoint-url "${S3_ENDPOINT}"

echo "Upload complete."
