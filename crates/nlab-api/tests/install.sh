#!/bin/sh
set -eu

REPOSITORY_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/nlab-api-installer-test.XXXXXX")
cleanup() {
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT HUP INT TERM

FAKE_BIN="$TEST_ROOT/fake-bin"
INSTALL_DIR="$TEST_ROOT/bin"
LOG="$TEST_ROOT/curl.log"
mkdir -p "$FAKE_BIN"

printf '%s\n' '#!/bin/sh' 'case "$1" in -s) echo Darwin ;; -m) echo arm64 ;; esac' > "$FAKE_BIN/uname"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$FAKE_BIN/sha256sum"
printf '%s\n' \
  '#!/bin/sh' \
  'for argument do' \
  '  case "$argument" in https://*) url=$argument ;; esac' \
  '  if [ "${previous:-}" = -o ]; then destination=$argument; fi' \
  '  previous=$argument' \
  'done' \
  ': > "$destination"' \
  'printf "%s\n" "$url" >> "$TEST_LOG"' > "$FAKE_BIN/curl"
printf '%s\n' \
  '#!/bin/sh' \
  'destination=$4' \
  'printf "%s\n" "#!/bin/sh" "printf '\''nlab-api 1.10.0\\n'\''" > "$destination/nlab-api"' \
  'chmod +x "$destination/nlab-api"' > "$FAKE_BIN/tar"
chmod +x "$FAKE_BIN"/*

PATH="$FAKE_BIN:$PATH" \
HOME="$TEST_ROOT/home" \
NLAB_API_INSTALL_DIR="$INSTALL_DIR" \
NLAB_API_VERSION=1.10.0 \
TEST_LOG="$LOG" \
  sh "$REPOSITORY_ROOT/install-nlab-api.sh" >/dev/null

[ "$("$INSTALL_DIR/nlab-api" --version)" = 'nlab-api 1.10.0' ]
[ "$(cat "$INSTALL_DIR/.nlab-api-managed")" = 'nlab-api installer v1' ]
grep -q '/releases/download/v1.10.0/nlab-api-aarch64-apple-darwin.tar.gz' "$LOG"

rm "$INSTALL_DIR/nlab-api"
ln -s "$TEST_ROOT/elsewhere" "$INSTALL_DIR/nlab-api"
if PATH="$FAKE_BIN:$PATH" \
  HOME="$TEST_ROOT/home" \
  NLAB_API_INSTALL_DIR="$INSTALL_DIR" \
  NLAB_API_VERSION=1.10.0 \
  TEST_LOG="$LOG" \
  sh "$REPOSITORY_ROOT/install-nlab-api.sh" >"$TEST_ROOT/stdout" 2>"$TEST_ROOT/stderr"; then
  printf '%s\n' 'installer replaced symlink' >&2
  exit 1
fi
grep -q 'refusing to replace non-regular file' "$TEST_ROOT/stderr"
