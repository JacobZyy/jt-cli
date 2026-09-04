#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
OUTPUT_DIR=${1:-"$SCRIPT_DIR/dist"}

if [ -e "$OUTPUT_DIR" ]; then
  printf '%s\n' "output already exists: $OUTPUT_DIR" >&2
  exit 1
fi

mkdir -p "$OUTPUT_DIR/skill"
install -m 755 "$SCRIPT_DIR/install.sh" "$OUTPUT_DIR/install.sh"
install -m 644 "$SCRIPT_DIR/ai-install-prompt.txt" "$OUTPUT_DIR/ai-install-prompt.txt"
cp -R "$SCRIPT_DIR/skill/nlab-backend-bridge" "$OUTPUT_DIR/skill/nlab-backend-bridge"

COPYFILE_DISABLE=1 tar -czf "$OUTPUT_DIR/nlab-backend-bridge.tar.gz" \
  -C "$SCRIPT_DIR/skill" nlab-backend-bridge

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$OUTPUT_DIR" && sha256sum nlab-backend-bridge.tar.gz > nlab-backend-bridge.tar.gz.sha256)
elif command -v shasum >/dev/null 2>&1; then
  (cd "$OUTPUT_DIR" && shasum -a 256 nlab-backend-bridge.tar.gz > nlab-backend-bridge.tar.gz.sha256)
else
  printf '%s\n' 'sha256sum or shasum is required' >&2
  exit 1
fi

printf '%s\n' "built nLab API static bundle in $OUTPUT_DIR"
