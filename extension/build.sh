#!/usr/bin/env bash
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"
ZIP_NAME="extension.zip"
rm -f "$ZIP_NAME"
zip -r "$ZIP_NAME" . \
  --exclude "*.zip" \
  --exclude "build.sh" \
  --exclude "build.ps1" \
  --exclude ".DS_Store" \
  --exclude "PRIVACY.md" \
  --exclude "README.md" \
  --exclude "generate_icons.ps1" \
  --exclude "*.sh"
echo "Built: $SCRIPT_DIR/$ZIP_NAME"
