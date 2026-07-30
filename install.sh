#!/bin/sh
set -eu

: "${HOME:?HOME is not set}"

REPOSITORY_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
INSTALL_DIR=${JT_INSTALL_DIR:-"$HOME/.local/bin"}
BIN_NAME=jt

if ! command -v cargo >/dev/null 2>&1; then
  printf '%s\n' 'cargo is required; install Rust from https://rustup.rs' >&2
  exit 1
fi

cargo build \
  --release \
  --locked \
  --manifest-path "$REPOSITORY_ROOT/Cargo.toml" \
  --target-dir "$REPOSITORY_ROOT/target"

mkdir -p "$INSTALL_DIR"
INSTALL_DIR=$(CDPATH= cd -- "$INSTALL_DIR" && pwd)
INSTALL_PATH="$INSTALL_DIR/$BIN_NAME"

install -m 755 "$REPOSITORY_ROOT/target/release/$BIN_NAME" "$INSTALL_PATH"
"$INSTALL_PATH" --help >/dev/null

printf '%s\n' "installed $BIN_NAME to $INSTALL_PATH"

case ":${PATH:-}:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    printf '%s\n' "warning: $INSTALL_DIR is not in PATH; run $INSTALL_PATH directly or add that directory to PATH" >&2
    ;;
esac
