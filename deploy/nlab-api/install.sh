#!/bin/sh
set -eu

: "${HOME:?HOME is not set}"

REPOSITORY_URL=https://github.com/JacobZyy/jt-cli
SITE_URL=${NLAB_API_SITE_URL:-https://jacob-z.top/nlab-api}
INSTALL_DIR=${NLAB_API_INSTALL_DIR:-"$HOME/.local/bin"}
SKILL_ROOT=${NLAB_API_SKILL_ROOT:-"$HOME/.agents/skills"}
VERSION=${NLAB_API_VERSION:-latest}
REPLACE_PROJECT_SKILL=0

for argument in "$@"; do
  case "$argument" in
    --replace-project-skill) REPLACE_PROJECT_SKILL=1 ;;
    *)
      printf '%s\n' "unknown option: $argument" >&2
      exit 1
      ;;
  esac
done

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) TARGET=aarch64-apple-darwin ;;
  *)
    printf '%s\n' "unsupported platform: $(uname -s) $(uname -m); nlab-api currently supports macOS Apple Silicon" >&2
    exit 1
    ;;
esac

for command_name in curl tar install grep find mv mkdir rm mktemp chmod date uname dirname ln readlink; do
  command -v "$command_name" >/dev/null 2>&1 || {
    printf '%s\n' "$command_name is required" >&2
    exit 1
  }
done

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

NLAB_TEMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/nlab-api-bundle.XXXXXX")
CLI_STAGE_PATH="$INSTALL_DIR/.nlab-api.tmp.$$"
MARKER_STAGE_PATH="$INSTALL_DIR/.nlab-api-managed.tmp.$$"
SKILL_STAGE_PATH="$SKILL_ROOT/.nlab-backend-bridge.tmp.$$"
SKILL_NAME=nlab-backend-bridge
SKILL_PATH="$SKILL_ROOT/$SKILL_NAME"
CODEX_SKILL_PATH="$HOME/.codex/skills/$SKILL_NAME"
CLAUDE_SKILL_PATH="$HOME/.claude/skills/$SKILL_NAME"
OPENCODE_SKILL_PATH="$HOME/.config/opencode/skills/$SKILL_NAME"
SKILL_BACKUP_ROOT=
PROJECT_ROOT=
PROJECT_SKILL_FOUND=0
CLAUDE_LINK_REQUIRED=0

cleanup() {
  rm -rf "$NLAB_TEMP_DIR" "$SKILL_STAGE_PATH"
  rm -f "$CLI_STAGE_PATH" "$MARKER_STAGE_PATH"
}
trap cleanup EXIT HUP INT TERM

if command -v git >/dev/null 2>&1; then
  PROJECT_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || true)
fi
if command -v claude >/dev/null 2>&1 || [ -d "$HOME/.claude" ]; then
  CLAUDE_LINK_REQUIRED=1
fi

entry_exists() {
  [ -e "$1" ] || [ -L "$1" ]
}

check_project_entry() {
  if entry_exists "$1"; then
    PROJECT_SKILL_FOUND=1
    printf '%s\n' "project-local Skill found: $1" >&2
  fi
}

if [ -n "$PROJECT_ROOT" ]; then
  check_project_entry "$PROJECT_ROOT/.agents/skills/$SKILL_NAME"
  check_project_entry "$PROJECT_ROOT/.codex/skills/$SKILL_NAME"
  check_project_entry "$PROJECT_ROOT/.claude/skills/$SKILL_NAME"
  check_project_entry "$PROJECT_ROOT/.opencode/skills/$SKILL_NAME"
fi

if [ "$PROJECT_SKILL_FOUND" -eq 1 ] && [ "$REPLACE_PROJECT_SKILL" -ne 1 ]; then
  printf '%s\n' 'rerun with --replace-project-skill after reviewing the repository changes' >&2
  exit 1
fi

download() {
  curl -fsSL --proto '=https' --tlsv1.2 --retry 3 "$1" -o "$2"
}

verify_checksum() {
  checksum_dir=$1
  checksum_file=$2
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$checksum_dir" && sha256sum -c "$checksum_file")
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$checksum_dir" && shasum -a 256 -c "$checksum_file")
  else
    printf '%s\n' 'sha256sum or shasum is required' >&2
    exit 1
  fi
}

ensure_skill_backup_root() {
  if [ -z "$SKILL_BACKUP_ROOT" ]; then
    SKILL_BACKUP_ROOT="$HOME/.agents/skill-backups/$SKILL_NAME-$(date '+%Y%m%d%H%M%S')-$$"
    mkdir -p "$SKILL_BACKUP_ROOT" || return 1
  fi
}

backup_skill_entry() {
  skill_entry=$1
  backup_name=$2
  if entry_exists "$skill_entry"; then
    ensure_skill_backup_root || return 1
    mv "$skill_entry" "$SKILL_BACKUP_ROOT/$backup_name" || return 1
  fi
}

backup_old_skills() {
  if [ -n "$PROJECT_ROOT" ] && [ "$REPLACE_PROJECT_SKILL" -eq 1 ]; then
    backup_skill_entry "$PROJECT_ROOT/.agents/skills/$SKILL_NAME" project-agents || return 1
    backup_skill_entry "$PROJECT_ROOT/.codex/skills/$SKILL_NAME" project-codex || return 1
    backup_skill_entry "$PROJECT_ROOT/.claude/skills/$SKILL_NAME" project-claude || return 1
    backup_skill_entry "$PROJECT_ROOT/.opencode/skills/$SKILL_NAME" project-opencode || return 1
  fi
  backup_skill_entry "$SKILL_PATH" user-agents || return 1
  backup_skill_entry "$CODEX_SKILL_PATH" user-codex || return 1
  backup_skill_entry "$CLAUDE_SKILL_PATH" user-claude || return 1
  backup_skill_entry "$OPENCODE_SKILL_PATH" user-opencode || return 1
}

restore_skill_entry() {
  skill_entry=$1
  backup_name=$2
  backup_entry="$SKILL_BACKUP_ROOT/$backup_name"
  if entry_exists "$backup_entry"; then
    rm -rf "$skill_entry"
    mkdir -p "$(dirname "$skill_entry")"
    mv "$backup_entry" "$skill_entry"
  fi
}

restore_old_skills() {
  [ -n "$SKILL_BACKUP_ROOT" ] || return 0
  restore_skill_entry "$SKILL_PATH" user-agents
  restore_skill_entry "$CODEX_SKILL_PATH" user-codex
  restore_skill_entry "$CLAUDE_SKILL_PATH" user-claude
  restore_skill_entry "$OPENCODE_SKILL_PATH" user-opencode
  if [ -n "$PROJECT_ROOT" ]; then
    restore_skill_entry "$PROJECT_ROOT/.agents/skills/$SKILL_NAME" project-agents
    restore_skill_entry "$PROJECT_ROOT/.codex/skills/$SKILL_NAME" project-codex
    restore_skill_entry "$PROJECT_ROOT/.claude/skills/$SKILL_NAME" project-claude
    restore_skill_entry "$PROJECT_ROOT/.opencode/skills/$SKILL_NAME" project-opencode
  fi
}

install_current_skill() {
  mv "$SKILL_STAGE_PATH" "$SKILL_PATH" || return 1
  if [ "$CLAUDE_LINK_REQUIRED" -eq 1 ]; then
    mkdir -p "$(dirname "$CLAUDE_SKILL_PATH")" || return 1
    ln -s "$SKILL_PATH" "$CLAUDE_SKILL_PATH" || return 1
  fi
  test -f "$SKILL_PATH/SKILL.md" || return 1
  if [ "$CLAUDE_LINK_REQUIRED" -eq 1 ]; then
    test -L "$CLAUDE_SKILL_PATH" || return 1
    test "$(readlink "$CLAUDE_SKILL_PATH")" = "$SKILL_PATH" || return 1
  fi
}

printf '%s\n' 'Downloading nlab-api...'
download "$DOWNLOAD_ROOT/$ARCHIVE" "$NLAB_TEMP_DIR/$ARCHIVE"
download "$DOWNLOAD_ROOT/$ARCHIVE.sha256" "$NLAB_TEMP_DIR/$ARCHIVE.sha256"
verify_checksum "$NLAB_TEMP_DIR" "$ARCHIVE.sha256"
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

printf '%s\n' 'Downloading nlab-backend-bridge Skill...'
SKILL_ARCHIVE=nlab-backend-bridge.tar.gz
download "$SITE_URL/$SKILL_ARCHIVE" "$NLAB_TEMP_DIR/$SKILL_ARCHIVE"
download "$SITE_URL/$SKILL_ARCHIVE.sha256" "$NLAB_TEMP_DIR/$SKILL_ARCHIVE.sha256"
verify_checksum "$NLAB_TEMP_DIR" "$SKILL_ARCHIVE.sha256"

tar -tzf "$NLAB_TEMP_DIR/$SKILL_ARCHIVE" > "$NLAB_TEMP_DIR/skill-members.txt"
while IFS= read -r member; do
  case "$member" in
    nlab-backend-bridge/ | \
    nlab-backend-bridge/SKILL.md | \
    nlab-backend-bridge/agents/ | \
    nlab-backend-bridge/agents/openai.yaml) ;;
    *)
      printf '%s\n' "unexpected Skill archive member: $member" >&2
      exit 1
      ;;
  esac
done < "$NLAB_TEMP_DIR/skill-members.txt"

tar -xzf "$NLAB_TEMP_DIR/$SKILL_ARCHIVE" -C "$NLAB_TEMP_DIR"
SKILL_SOURCE="$NLAB_TEMP_DIR/nlab-backend-bridge"
test -f "$SKILL_SOURCE/SKILL.md" || {
  printf '%s\n' 'Skill archive is missing SKILL.md' >&2
  exit 1
}
test -f "$SKILL_SOURCE/agents/openai.yaml" || {
  printf '%s\n' 'Skill archive is missing agents/openai.yaml' >&2
  exit 1
}
if find "$SKILL_SOURCE" -type l | grep -q .; then
  printf '%s\n' 'Skill archive contains a symbolic link' >&2
  exit 1
fi
grep -Eq '^name: nlab-backend-bridge$' "$SKILL_SOURCE/SKILL.md" || {
  printf '%s\n' 'Skill identity check failed' >&2
  exit 1
}

mkdir -p "$INSTALL_DIR" "$SKILL_ROOT"
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
install -m 755 "$NLAB_TEMP_DIR/nlab-api" "$CLI_STAGE_PATH"
printf '%s\n' 'nlab-api installer v1' > "$MARKER_STAGE_PATH"
chmod 644 "$MARKER_STAGE_PATH"
mkdir "$SKILL_STAGE_PATH"
install -m 644 "$SKILL_SOURCE/SKILL.md" "$SKILL_STAGE_PATH/SKILL.md"
mkdir "$SKILL_STAGE_PATH/agents"
install -m 644 "$SKILL_SOURCE/agents/openai.yaml" "$SKILL_STAGE_PATH/agents/openai.yaml"

if ! backup_old_skills; then
  restore_old_skills
  printf '%s\n' 'failed to back up previous Skill entries' >&2
  exit 1
fi

if ! install_current_skill; then
  rm -rf "$SKILL_PATH" "$CLAUDE_SKILL_PATH"
  restore_old_skills
  printf '%s\n' "failed to install Skill to $SKILL_PATH" >&2
  exit 1
fi

mv -f "$CLI_STAGE_PATH" "$INSTALL_PATH"
mv -f "$MARKER_STAGE_PATH" "$MARKER_PATH"

printf '%s\n' "installed $ACTUAL_VERSION to $INSTALL_PATH"
printf '%s\n' "installed nlab-backend-bridge Skill to $SKILL_PATH"
printf '%s\n' "Codex discovers the Skill directly from $SKILL_ROOT"
if [ "$CLAUDE_LINK_REQUIRED" -eq 1 ]; then
  printf '%s\n' "linked Claude Code Skill at $CLAUDE_SKILL_PATH"
fi
if [ -n "$SKILL_BACKUP_ROOT" ]; then
  printf '%s\n' "previous Skill entries backed up to $SKILL_BACKUP_ROOT"
fi
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) printf '%s\n' "add $INSTALL_DIR to PATH, then restart your AI client so it discovers the Skill" ;;
esac
