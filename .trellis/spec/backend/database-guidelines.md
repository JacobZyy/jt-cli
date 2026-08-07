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

## Scenario: Interactive GitHub Repository Bootstrap

### 1. Scope / Trigger

`jt repo cicd` may create a public GitHub repository when the project has no GitHub `origin`. This path crosses local Git, GitHub CLI, package metadata, and generated release files, so all validation happens before mutation.

### 2. Signatures

- Entry: `jt repo cicd`
- Read-only probes: `git rev-parse --show-toplevel`, `git remote get-url origin`, `gh --version`, `gh auth status --hostname github.com`, `gh api user --hostname github.com --jq .login`
- Mutations: `git init`, then `gh repo create OWNER/REPO --public --source=. --remote=origin [--description TEXT]`

### 3. Contracts

- Prompt only on TTY when `origin` is missing. Both confirmations default to No.
- `OWNER/REPO` must match every publishable package repository declaration.
- Creation is public, pins `GH_HOST=github.com`, uses discrete subprocess arguments, and never adds `--push`.
- Git and `gh repo create --source=.` commands remove `GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`, `GIT_CEILING_DIRECTORIES`, `GIT_DISCOVERY_ACROSS_FILESYSTEM`, and `GIT_IMPLICIT_WORK_TREE`.
- Later failure retains any initialized `.git`, created GitHub repository, and added `origin`; error text names retained state.

### 4. Validation & Error Matrix

- Non-TTY plus missing origin: fail before prompts or mutation.
- Missing `gh`: request installation; do not initialize Git.
- Unauthenticated `gh`: instruct `gh auth login`; never launch login.
- Existing non-GitHub origin: reject; never replace it.
- Existing `.git` plus failed Git probe: reject; never run `git init`.
- Package repository mismatch: reject before `git init` or GitHub creation.
- Failure after mutation: preserve created state and report it for safe retry.

### 5. Good / Base / Bad Cases

- Good: missing Git repository, authenticated `gh`, matching metadata, two approvals; create repository and resume release initialization.
- Base: valid GitHub origin; keep existing non-interactive release initialization.
- Bad: cancellation, invalid slug, mismatched metadata, redirected Git environment, or non-GitHub origin; perform no repository mutation.

### 6. Tests Required

- Unit: assert both cancellation points cause zero mutation.
- Unit: assert missing/authentication failures occur before prompt and `git init`.
- Unit: assert exact `gh repo create` arguments, `GH_HOST`, removed `GIT_*` keys, and absence of `--push`.
- Unit: assert success resumes file generation; partial failures retain and report Git/GitHub state.
- CLI: null stdin plus missing origin exits nonzero without creating `.git` or release files.

### 7. Wrong vs Correct

Wrong: shell command string, inherited `GH_HOST`/`GIT_*`, automatic login, implicit push, or rollback deleting remote/local state.

Correct: validated `CommandSpec` arguments, pinned GitHub host, isolated Git environment, explicit confirmations, no push, recoverable retained state.

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
