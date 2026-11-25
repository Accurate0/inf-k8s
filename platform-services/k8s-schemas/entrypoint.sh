#!/bin/bash
set -euo pipefail

export HOME=/tmp

MODE="${1:-full}"
SOURCE_DIR=/schemas-src

if [ "$MODE" = repo ]; then
  echo "Publishing app.json and helm chart values schemas..."
  REPO_OUTPUT_DIR="/tmp/repo-schemas"
  mkdir -p "$REPO_OUTPUT_DIR/charts"
  cp "$SOURCE_DIR/app.json" "$REPO_OUTPUT_DIR/app.json"
  for schema in "$SOURCE_DIR"/charts/*/values.schema.json; do
    cp "$schema" "$REPO_OUTPUT_DIR/charts/$(basename "$(dirname "$schema")").json"
  done

  aws s3 sync "$REPO_OUTPUT_DIR" "s3://${S3_BUCKET}/" --endpoint-url "${S3_ENDPOINT}"
  echo "Upload complete."
  exit 0
fi

: "${K8S_VERSION:?K8S_VERSION must be set (e.g. release-1.30, master, v1.30.0)}"

CATALOG_DIR="/tmp/CRDs-catalog"
rm -rf "$CATALOG_DIR" "${HOME}/.datree"
git clone --depth 1 https://github.com/datreeio/CRDs-catalog.git "$CATALOG_DIR"

# Run CRD extractor script
bash "$CATALOG_DIR/Utilities/crd-extractor.sh"

OUTPUT_DIR="${HOME}/.datree/crdSchemas"

if [ ! -d "$OUTPUT_DIR" ]; then
  echo "ERROR: CRD schema output directory not found at $OUTPUT_DIR"
  exit 1
fi

echo "Fetching k8s swagger.json for ${K8S_VERSION}..."
SWAGGER_FILE="/tmp/k8s-swagger.json"
curl -fsSL \
  "https://raw.githubusercontent.com/kubernetes/kubernetes/${K8S_VERSION}/api/openapi-spec/swagger.json" \
  -o "$SWAGGER_FILE"

echo "Generating built-in k8s schemas..."
BUILTIN_TMP_DIR="/tmp/k8s-builtin-schemas"
rm -rf "$BUILTIN_TMP_DIR"
openapi2jsonschema --kubernetes --stand-alone --expanded -o "$BUILTIN_TMP_DIR" "$SWAGGER_FILE"
python3 /reorganize-schemas.py "$BUILTIN_TMP_DIR" "$OUTPUT_DIR"

echo "Publishing helm chart values schemas..."
python3 /patch-helmrelease-schema.py "$OUTPUT_DIR" "$SOURCE_DIR/charts"
cp "$SOURCE_DIR/app.json" "$OUTPUT_DIR/app.json"

echo "Building index.json..."
python3 /build-index.py "$OUTPUT_DIR"

echo "Uploading schemas to S3..."
aws s3 sync "$OUTPUT_DIR" "s3://${S3_BUCKET}/" \
  --endpoint-url "${S3_ENDPOINT}" \
  --delete \
  --exclude "master-standalone/*" \
  --exclude "version.json"

aws s3 cp "$OUTPUT_DIR/version.json" "s3://${S3_BUCKET}/version.json" \
  --endpoint-url "${S3_ENDPOINT}"

echo "Upload complete."
