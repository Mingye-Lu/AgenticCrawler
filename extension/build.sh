#!/usr/bin/env bash
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"
ZIP_NAME="extension.zip"
rm -f "$ZIP_NAME"
zip -r "$ZIP_NAME" \
  manifest.json \
  background.js \
  options.html \
  options.js \
  icons \
  commands
echo "Built: $SCRIPT_DIR/$ZIP_NAME"
