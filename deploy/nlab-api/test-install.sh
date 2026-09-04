#!/bin/sh
set -eu

SITE_URL=${1:-https://jacob-z.top/nlab-api}
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TEST_HOME=$(mktemp -d "${TMPDIR:-/tmp}/nlab-api-install-test.XXXXXX")

cleanup() {
  rm -rf "$TEST_HOME"
}
trap cleanup EXIT HUP INT TERM

OLD_SKILL_SOURCE="$TEST_HOME/old-skill/nlab-backend-bridge"
mkdir -p "$OLD_SKILL_SOURCE" \
  "$TEST_HOME/.agents/skills" \
  "$TEST_HOME/.codex/skills" \
  "$TEST_HOME/.claude/skills" \
  "$TEST_HOME/.config/opencode/skills" \
  "$TEST_HOME/bin" \
  "$TEST_HOME/project/.agents/skills/nlab-backend-bridge"
printf '%s\n' 'old user Skill' > "$OLD_SKILL_SOURCE/SKILL.md"
printf '%s\n' 'old project Skill' > "$TEST_HOME/project/.agents/skills/nlab-backend-bridge/SKILL.md"
ln -s "$OLD_SKILL_SOURCE" "$TEST_HOME/.agents/skills/nlab-backend-bridge"
ln -s "$OLD_SKILL_SOURCE" "$TEST_HOME/.codex/skills/nlab-backend-bridge"
ln -s "$OLD_SKILL_SOURCE" "$TEST_HOME/.claude/skills/nlab-backend-bridge"
ln -s "$OLD_SKILL_SOURCE" "$TEST_HOME/.config/opencode/skills/nlab-backend-bridge"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$TEST_HOME/bin/claude"
chmod 755 "$TEST_HOME/bin/claude"
git -C "$TEST_HOME/project" init -q

if (
  cd "$TEST_HOME/project"
  HOME="$TEST_HOME" PATH="$TEST_HOME/bin:$PATH" NLAB_API_SITE_URL="$SITE_URL" sh "$SCRIPT_DIR/install.sh"
) > "$TEST_HOME/refusal.log" 2>&1; then
  printf '%s\n' 'installer replaced a project Skill without explicit approval' >&2
  exit 1
fi
grep -q -- '--replace-project-skill' "$TEST_HOME/refusal.log"

(
  cd "$TEST_HOME/project"
  HOME="$TEST_HOME" PATH="$TEST_HOME/bin:$PATH" NLAB_API_SITE_URL="$SITE_URL" \
    sh "$SCRIPT_DIR/install.sh" --replace-project-skill
)

"$TEST_HOME/.local/bin/nlab-api" --version | grep -Eq '^nlab-api [0-9]+\.[0-9]+\.[0-9]+'
"$TEST_HOME/.local/bin/nlab-api" config --help | grep -q -- '--detect'
test -f "$TEST_HOME/.local/bin/.nlab-api-managed"
test -f "$TEST_HOME/.agents/skills/nlab-backend-bridge/SKILL.md"
test -f "$TEST_HOME/.agents/skills/nlab-backend-bridge/agents/openai.yaml"
grep -q '^name: nlab-backend-bridge$' "$TEST_HOME/.agents/skills/nlab-backend-bridge/SKILL.md"
test ! -e "$TEST_HOME/.codex/skills/nlab-backend-bridge"
test ! -L "$TEST_HOME/.codex/skills/nlab-backend-bridge"
test ! -e "$TEST_HOME/.config/opencode/skills/nlab-backend-bridge"
test ! -L "$TEST_HOME/.config/opencode/skills/nlab-backend-bridge"
test -L "$TEST_HOME/.claude/skills/nlab-backend-bridge"
test "$(readlink "$TEST_HOME/.claude/skills/nlab-backend-bridge")" = \
  "$TEST_HOME/.agents/skills/nlab-backend-bridge"
test ! -e "$TEST_HOME/project/.agents/skills/nlab-backend-bridge"
test ! -L "$TEST_HOME/project/.agents/skills/nlab-backend-bridge"

BACKUP_DIR=$(find "$TEST_HOME/.agents/skill-backups" -mindepth 1 -maxdepth 1 -type d | head -n 1)
test -L "$BACKUP_DIR/user-agents"
test -L "$BACKUP_DIR/user-codex"
test -L "$BACKUP_DIR/user-claude"
test -L "$BACKUP_DIR/user-opencode"
test -f "$BACKUP_DIR/project-agents/SKILL.md"

printf '%s\n' "isolated install passed: $TEST_HOME"
