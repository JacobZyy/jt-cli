#!/bin/sh
set -eu

SITE_URL=${1:-https://jacob-z.top/nlab-api}
TEST_HOME=$(mktemp -d "${TMPDIR:-/tmp}/nlab-api-install-test.XXXXXX")

cleanup() {
  rm -rf "$TEST_HOME"
}
trap cleanup EXIT HUP INT TERM

HOME="$TEST_HOME" NLAB_API_SITE_URL="$SITE_URL" sh "$(dirname "$0")/install.sh"

"$TEST_HOME/.local/bin/nlab-api" --version | grep -Eq '^nlab-api [0-9]+\.[0-9]+\.[0-9]+'
"$TEST_HOME/.local/bin/nlab-api" config --help | grep -q -- '--detect'
test -f "$TEST_HOME/.local/bin/.nlab-api-managed"
test -f "$TEST_HOME/.agents/skills/nlab-backend-bridge/SKILL.md"
test -f "$TEST_HOME/.agents/skills/nlab-backend-bridge/agents/openai.yaml"
grep -q '^name: nlab-backend-bridge$' "$TEST_HOME/.agents/skills/nlab-backend-bridge/SKILL.md"

printf '%s\n' "isolated install passed: $TEST_HOME"
