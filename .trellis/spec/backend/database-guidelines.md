# Persistence Guidelines

> Current project has no database, ORM, query layer, schema, migration, or transaction system.

## Current Storage Model

`Cargo.toml` contains no database dependency or internal storage layer. Commands write files directly (shell and Zed configuration, generated workflows, icons, and executable replacement) and delegate package installation or Git configuration to subprocesses.

Do not invent database conventions for new work. If a future task introduces a database, update this file from that reviewed implementation.

## Existing File-Mutation Patterns

- Use `src/node/fs.rs::atomic_write` for managed files below a validated root. CLI commands pass HOME; Zed configuration passes the Git repository root. It rejects symlink targets, verifies expected prior content, writes a private sibling file, syncs it, then renames it.
- Validate targets before deletion with `src/node/fs.rs::ensure_safe_home_target`; never recursively remove an unresolved or broad path.
- Preserve user content with backups where commands update existing files. `src/zed.rs` reads, backs up, then atomically replaces settings.
- Use `OpenOptions::create_new(true)` when output must not overwrite an existing file. `src/release.rs` and `src/icon.rs` use this rule.
- Remove partial newly-created files after a failed write. `src/release.rs::init` does this before returning its error.
- For self-upgrade, verify candidate executable before replacement and restore previous executable if post-install verification fails (`src/upgrade.rs`).

Representative guarded write:

```rust
let current = read_optional(path)?;
atomic_write(home, path, current.as_deref(), content)?;
```

Representative create-only output:

```rust
OpenOptions::new()
    .write(true)
    .create_new(true)
    .open(&path)?;
```

## Database Sections

- Queries: not applicable.
- Migrations: not applicable.
- Table, column, and index naming: not applicable.
- Transactions: not applicable. Atomic file replacement and explicit rollback are current durability mechanisms.

## Avoid

- Do not replace guarded shared writes with blind `fs::write` on user-managed files.
- Do not overwrite generated output when existing content differs; report refusal.
- Do not call a command dry-run if it creates directories, writes files, or changes configuration.
- Do not add an ORM or storage abstraction for existing filesystem-only behavior.
