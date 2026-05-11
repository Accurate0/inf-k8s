#!/bin/bash
set -euo pipefail

export HOME=/tmp

# Run CRD extractor script
bash /opt/CRDs-catalog/Utilities/crd-extractor.sh

OUTPUT_DIR="${HOME}/.datree/crdSchemas"

if [ ! -d "$OUTPUT_DIR" ]; then
  echo "ERROR: CRD schema output directory not found at $OUTPUT_DIR"
  exit 1
fi

echo "Uploading schemas to S3..."
aws s3 sync "$OUTPUT_DIR" "s3://${S3_BUCKET}/" \
  --endpoint-url "${S3_ENDPOINT}" \
  --delete

echo "Upload complete."
