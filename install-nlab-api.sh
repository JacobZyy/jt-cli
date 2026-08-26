#!/bin/sh
set -eu

: "${HOME:?HOME is not set}"

REPOSITORY_URL=https://github.com/JacobZyy/jt-cli
INSTALL_DIR=${NLAB_API_INSTALL_DIR:-"$HOME/.local/bin"}
VERSION=${NLAB_API_VERSION:-latest}

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) TARGET=aarch64-apple-darwin ;;
  *)
    printf '%s\n' "unsupported platform: $(uname -s) $(uname -m)" >&2
    exit 1
    ;;
esac

ARCHIVE="nlab-api-$TARGET.tar.gz"
case "$VERSION" in
  latest) DOWNLOAD_ROOT="$REPOSITORY_URL/releases/latest/download" ;;
  v[0-9]* | [0-9]*)
    TAG=${VERSION#v}
    printf '%s\n' "$TAG" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$' || {
      printf '%s\n' "invalid NLAB_API_VERSION: $VERSION" >&2
      exit 1
    }
    DOWNLOAD_ROOT="$REPOSITORY_URL/releases/download/v$TAG"
    ;;
  *)
    printf '%s\n' "invalid NLAB_API_VERSION: $VERSION" >&2
    exit 1
    ;;
esac

command -v curl >/dev/null 2>&1 || {
  printf '%s\n' 'curl is required' >&2
  exit 1
}
command -v tar >/dev/null 2>&1 || {
  printf '%s\n' 'tar is required' >&2
  exit 1
}
command -v install >/dev/null 2>&1 || {
  printf '%s\n' 'install is required' >&2
  exit 1
}

NLAB_TEMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/nlab-api.XXXXXX")
NLAB_STAGE_PATH="$INSTALL_DIR/.nlab-api.tmp.$$"
NLAB_MARKER_STAGE="$INSTALL_DIR/.nlab-api-managed.tmp.$$"
cleanup() {
  rm -rf "$NLAB_TEMP_DIR"
  rm -f "$NLAB_STAGE_PATH"
  rm -f "$NLAB_MARKER_STAGE"
}
trap cleanup EXIT HUP INT TERM

curl -fsSL --proto '=https' --tlsv1.2 --retry 3 \
  "$DOWNLOAD_ROOT/$ARCHIVE" -o "$NLAB_TEMP_DIR/$ARCHIVE"
curl -fsSL --proto '=https' --tlsv1.2 --retry 3 \
  "$DOWNLOAD_ROOT/$ARCHIVE.sha256" -o "$NLAB_TEMP_DIR/$ARCHIVE.sha256"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$NLAB_TEMP_DIR" && sha256sum -c "$ARCHIVE.sha256")
elif command -v shasum >/dev/null 2>&1; then
  (cd "$NLAB_TEMP_DIR" && shasum -a 256 -c "$ARCHIVE.sha256")
else
  printf '%s\n' 'sha256sum or shasum is required' >&2
  exit 1
fi

tar -xzf "$NLAB_TEMP_DIR/$ARCHIVE" -C "$NLAB_TEMP_DIR"
ACTUAL_VERSION=$("$NLAB_TEMP_DIR/nlab-api" --version)
case "$ACTUAL_VERSION" in
  "nlab-api "*) ;;
  *)
    printf '%s\n' "invalid downloaded binary version: $ACTUAL_VERSION" >&2
    exit 1
    ;;
esac
if [ "$VERSION" != latest ] && [ "$ACTUAL_VERSION" != "nlab-api $TAG" ]; then
  printf '%s\n' "downloaded $ACTUAL_VERSION; expected nlab-api $TAG" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
INSTALL_PATH="$INSTALL_DIR/nlab-api"
MARKER_PATH="$INSTALL_DIR/.nlab-api-managed"
if [ -L "$INSTALL_PATH" ] || { [ -e "$INSTALL_PATH" ] && [ ! -f "$INSTALL_PATH" ]; }; then
  printf '%s\n' "refusing to replace non-regular file: $INSTALL_PATH" >&2
  exit 1
fi
if [ -L "$MARKER_PATH" ] || { [ -e "$MARKER_PATH" ] && [ ! -f "$MARKER_PATH" ]; }; then
  printf '%s\n' "refusing to replace non-regular file: $MARKER_PATH" >&2
  exit 1
fi
install -m 755 "$NLAB_TEMP_DIR/nlab-api" "$NLAB_STAGE_PATH"
printf '%s\n' 'nlab-api installer v1' > "$NLAB_MARKER_STAGE"
chmod 644 "$NLAB_MARKER_STAGE"
mv -f "$NLAB_STAGE_PATH" "$INSTALL_PATH"
mv -f "$NLAB_MARKER_STAGE" "$MARKER_PATH"

printf '%s\n' "installed nlab-api to $INSTALL_PATH"
