# Quality Guidelines

> Small Rust CLI changes. Standard library and existing modules first. Safety checks stay explicit.

## Required Gates

README and CI define the same checks:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

`.githooks/pre-commit` runs `cargo test --locked --quiet`. GitHub CI remains required because local hooks can be bypassed.

## Required Patterns

- Keep `unsafe_code = "forbid"` from `Cargo.toml`.
- Reuse existing command, filesystem, error, platform, and prompt boundaries before adding helpers or dependencies.
- Validate external inputs, paths, symlinks, current file content, platform, and TTY requirements before mutation.
- Make filesystem output recoverable: create-only writes, atomic replacement, backup, cleanup, or rollback as appropriate.
- Redact secrets before including subprocess details in errors.
- Keep exit status, stdout, and stderr stable; they are CLI contracts covered by integration tests.
- Keep generated GitHub Actions references immutable when write or OIDC permissions are involved.

## Testing

- Put focused unit tests beside implementation in `#[cfg(test)] mod tests`.
- Use `tests/cli.rs` for spawned-binary behavior: argument shape, status, stdout, stderr, TTY rejection, and mutation safety.
- Use `tempfile` for isolated filesystem tests.
- Use existing injectable boundaries such as `Runner` and `Prompter` instead of invoking real package managers or prompting in unit tests.
- Add one regression test for non-trivial branches, parsers, destructive paths, money/security paths, or a fixed bug. Avoid test scaffolding for trivial one-line changes.

Current baseline: 124 unit tests and 9 CLI integration tests.

## Review Checklist

- Change sits at shared root cause and covers sibling callers.
- No new abstraction or dependency duplicates stdlib or existing code.
- Help text, command matching, exit code, and tests agree.
- User files cannot be silently overwritten, followed through symlinks, or deleted outside validated scope.
- Partial failures leave recoverable state and report accurate outcome.
- Error/output text contains no secrets.
- Formatting, Clippy with warnings denied, and locked tests pass.

## Forbidden Patterns

- `unsafe` Rust.
- Blind overwrite or recursive deletion of unresolved user paths.
- Raw secret-bearing command output in errors.
- Network, package-manager, or destructive filesystem work in unit tests when an existing injectable boundary covers it.
- Speculative interfaces, factories, configuration, or helpers with one caller.
