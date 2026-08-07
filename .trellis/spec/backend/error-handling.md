# Error Handling

> Propagate contextual errors inside modules. Print once at CLI boundary. Return meaningful exit status.

## Error Types

Node, bootstrap, Zed, and upgrade paths share `src/node/error.rs`:

```rust
pub type Result<T> = std::result::Result<T, AppError>;

pub enum AppError {
    Invalid(String),
    Io { action: String, path: Option<PathBuf>, source: io::Error },
    Command { action: String, status: i32, detail: String },
    Decode { action: String, detail: String },
    UnsafePath(String),
}
```

`src/release.rs` and `src/icon.rs` currently use `Result<_, String>`. Preserve this local split unless a task needs shared typed behavior; do not refactor only for uniformity.

## Propagation

- Return `Result` from fallible internal functions and use `?`.
- Convert I/O failures where they occur, keeping action and path context with `AppError::io`.
- Represent validation and trust-boundary refusal explicitly with `Invalid` or `UnsafePath`.
- Route subprocess failures through `CommandResult::require_success` when using `src/node/command.rs`. It keeps status plus last non-empty stderr/stdout line and redacts sensitive values.
- Model partial multi-stage work explicitly. Node cleanup uses stage outcomes and a final incomplete summary. GitHub repository bootstrap reports retained Git/GitHub state after partial failure. Neither path attempts unsafe blanket rollback.

## CLI Boundary

- Print operational errors once as `error: {error}` to stderr.
- Return `0` for success, `1` for operational failure, and `2` for invalid command shape or argument parsing where current commands distinguish usage errors.
- Keep reusable modules free from duplicate terminal printing; their caller owns final presentation.
- Interactive commands reject non-TTY use before mutation.

```rust
match result {
    Ok(()) => ExitCode::SUCCESS,
    Err(error) => {
        eprintln!("error: {error}");
        ExitCode::FAILURE
    }
}
```

## API Responses

Not applicable. `jt` is a local CLI and exposes no HTTP API.

## Avoid

- Do not `unwrap` or `expect` external input, filesystem, network, or subprocess results outside tests and proven invariants.
- Do not erase action, path, or subprocess status while mapping an error.
- Do not print an error at both module and CLI layers.
- Do not include raw credentials, tokens, proxy URLs, or query strings in command errors.
