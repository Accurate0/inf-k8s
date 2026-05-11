#!/bin/bash
set -euo pipefail

export HOME=/tmp

: "${K8S_VERSION:?K8S_VERSION must be set (e.g. release-1.30, master, v1.30.0)}"

# Run CRD extractor script
bash /opt/CRDs-catalog/Utilities/crd-extractor.sh

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

echo "Uploading schemas to S3..."
aws s3 sync "$OUTPUT_DIR" "s3://${S3_BUCKET}/" \
  --endpoint-url "${S3_ENDPOINT}" \
  --delete \
  --exclude "master-standalone/*"

echo "Upload complete."
