# User Output and Logging

> Current project has no `log` or `tracing` dependency, persistent logs, structured events, or configurable log levels.

## Output Channels

- stdout: help, version, created/already-configured paths, successful progress, and final success summaries.
- stderr: `error:` messages, warnings, partial-failure details, and invalid-usage help.
- `cliclack`: interactive prompts, notes, spinners, cancellation, and completion for TTY-only workflows (`src/cli.rs`, `src/node/cli.rs`, `src/release.rs`).
- subprocess output: node commands capture it through `src/node/command.rs`; long-running bootstrap and upgrade installers intentionally inherit terminal output, while probes capture or discard output.

```rust
println!("created {}", path.display());
eprintln!("error: {error}");
```

## Error Detail

`CommandResult::require_success` selects the last non-empty stderr line, then stdout, then `command returned non-zero`. Before returning `AppError::Command`, it calls `redact`.

Redaction currently:

- replaces known secret values with `***`;
- removes URL query and fragment data;
- masks URL user-info before `@`;
- preserves enough command context and exit status to diagnose failure.

Proxy inheritance is allowlisted to standard proxy environment keys. Commands that require isolation can clear the parent environment before adding explicit values.

## What to Report

- Completed action and relevant safe path.
- Refusal reason before any destructive mutation.
- Failed action, exit status, and one concise redacted detail line.
- Partial-stage outcome when earlier mutations succeeded but later work failed.

## What Not to Report

- Passwords, tokens, cookies, proxy credentials, authenticated URLs, or URL query strings.
- Full environment maps.
- Unbounded captured subprocess stdout/stderr embedded in an error when one diagnostic line is enough. Interactive installers may stream progress directly.
- Invented timestamps, request IDs, JSON fields, or severity levels; no structured logging contract exists.

## Avoid

- Do not add a logging dependency for existing one-shot CLI output.
- Do not use stdout for errors consumed by scripts.
- Do not bypass `src/node/command.rs::redact` when surfacing captured node-command failures.
