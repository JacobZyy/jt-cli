if command -v codegraph >/dev/null 2>&1; then
  if [ -L .codegraph ] || [ -L .codegraph/codegraph.db ] || [ -L .codegraph/codegraph.db-wal ] || [ -L .codegraph/codegraph.db-shm ]; then
    printf '%s\n' 'Refusing shared CodeGraph symlink; each worktree needs its own database.' >&2
    exit 1
  fi
  if [ "$worktree_root" != "$main_worktree_root" ] && [ .codegraph/codegraph.db -ef "$main_worktree_root/.codegraph/codegraph.db" ]; then
    printf '%s\n' 'Refusing hard-linked CodeGraph database.' >&2
    exit 1
  fi
  if [ ! -f .codegraph/codegraph.db ] && [ "$worktree_root" != "$main_worktree_root" ] && [ -f "$main_worktree_root/.codegraph/codegraph.db" ]; then
    if [ -L "$main_worktree_root/.codegraph" ] || [ -L "$main_worktree_root/.codegraph/codegraph.db" ] || [ -L "$main_worktree_root/.codegraph/codegraph.db-wal" ] || [ -L "$main_worktree_root/.codegraph/codegraph.db-shm" ]; then
      printf '%s\n' 'Refusing symlinked source CodeGraph database.' >&2
      exit 1
    fi
    if command -v sqlite3 >/dev/null 2>&1; then
      (
        set -eu
        mkdir -p .codegraph
        copy_dir="$(mktemp -d .codegraph/jt-copy.XXXXXX)"
        snapshot_dir=''
        locked=''
        trap 'rm -rf "$copy_dir"; if [ -n "$snapshot_dir" ]; then rm -rf "$snapshot_dir"; fi; if [ -n "$locked" ]; then rmdir "$locked"; fi' EXIT
        cache="$git_common_dir/jt-codex"
        for path in "$cache" "$cache/codegraph.db" "$cache/source-signature" "$cache/snapshot.lock"; do
          if [ -L "$path" ]; then printf '%s\n' "Refusing snapshot cache symlink: $path" >&2; exit 1; fi
        done
        mkdir -p "$cache"
        source="$main_worktree_root/.codegraph/codegraph.db"
        if mkdir "$cache/snapshot.lock" 2>/dev/null; then
          locked="$cache/snapshot.lock"
          # This key only controls cache freshness. SQLite creates the consistent
          # snapshot (including committed WAL pages); sync resolves branch drift.
          signature="$(cksum "$source"; if [ -f "$source-wal" ]; then cksum "$source-wal"; fi)"
          if [ ! -f "$cache/codegraph.db" ] || [ ! -f "$cache/source-signature" ] || [ "$signature" != "$(cat "$cache/source-signature")" ]; then
            snapshot_dir="$(mktemp -d "$cache/snapshot.XXXXXX")"
            (cd "$snapshot_dir" && sqlite3 -readonly "$source" -cmd '.timeout 10000' '.backup snapshot.db')
            sqlite3 "$snapshot_dir/snapshot.db" 'PRAGMA journal_mode=DELETE;' >/dev/null
            mv "$snapshot_dir/snapshot.db" "$cache/codegraph.db"
            printf '%s\n' "$signature" > "$snapshot_dir/source-signature"
            mv "$snapshot_dir/source-signature" "$cache/source-signature"
          fi
          # Never clone the live source database. This cache has no writer/WAL.
          if cp -c "$cache/codegraph.db" "$copy_dir/codegraph.db" 2>/dev/null || cp --reflink=always "$cache/codegraph.db" "$copy_dir/codegraph.db" 2>/dev/null; then
            printf '%s\n' 'CodeGraph: cloned consistent snapshot with copy-on-write.'
          else
            cp "$cache/codegraph.db" "$copy_dir/codegraph.db"
            printf '%s\n' 'CodeGraph: copied consistent snapshot (copy-on-write unavailable).'
          fi
        else
          # Concurrent snapshot preparation or interrupted cache lock: own backup.
          (cd "$copy_dir" && sqlite3 -readonly "$source" -cmd '.timeout 10000' '.backup codegraph.db')
          sqlite3 "$copy_dir/codegraph.db" 'PRAGMA journal_mode=DELETE;' >/dev/null
        fi
        # Publish without overwriting an index created by another setup process.
        ln "$copy_dir/codegraph.db" .codegraph/codegraph.db 2>/dev/null || test -f .codegraph/codegraph.db
      )
    fi
  fi
  if [ -f .codegraph/codegraph.db ]; then
    codegraph sync "$worktree_root"
  elif [ -f "$main_worktree_root/.codegraph/codegraph.db" ]; then
    codegraph init "$worktree_root"
  else
    printf '%s\n' 'CodeGraph: project no longer indexed; setup skipped.'
  fi
else
  printf '%s\n' 'CodeGraph unavailable; existing project index was not prepared.' >&2
fi
