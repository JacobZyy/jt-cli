# Directory Structure

> Current Rust CLI module layout. This project has no HTTP backend, routes, controllers, or services.

## Layout

```text
src/
├── main.rs             # Argument matching, help text, exit-code boundary
├── cli.rs              # Interactive terminal and Ghostty bootstrap
├── icon.rs             # Embedded icon output
├── release.rs          # npm release-workflow initialization
├── upgrade.rs          # jt self-upgrade
├── zed.rs              # Repository-local Zed configuration
└── node/
    ├── mod.rs          # Node initialization orchestration
    ├── aggressive.rs   # Aggressive cleanup plan and execution
    ├── cleanup.rs      # Cleanup operations
    ├── cli.rs          # Injectable interactive UI
    ├── command.rs      # Process execution and secret redaction
    ├── context.rs      # Runtime context construction
    ├── error.rs        # Shared node error type
    ├── fs.rs           # Guarded filesystem mutations
    ├── inventory.rs    # Installed-tool discovery
    ├── model.rs        # Shared node data structures
    ├── nrm.rs          # npm registry handling
    ├── platform.rs     # OS and architecture detection
    └── shell.rs        # Shell configuration transforms
tests/cli.rs            # End-to-end CLI process tests
assets/cli/             # Embedded shell and terminal configuration
templates/zed/          # Embedded Zed JSON template
jt-icon/                # Embedded icon source files
```

## Module Rules

- Keep command dispatch, help text, and final `ExitCode` conversion in `src/main.rs`.
- Put a self-contained command in `src/<command>.rs`. Split into `src/<feature>/` only when it has several distinct concerns, as `src/node/` does.
- Keep orchestration in a feature root (`src/node/mod.rs`); keep process, filesystem, platform, UI, and model concerns in named modules.
- Keep helpers private unless another module needs them. Public functions form module boundaries, not a general utility API.
- Keep bundled data outside `src/`; load it with `include_str!` or `include_bytes!` from `assets/`, `templates/`, or `jt-icon/`.
- Keep focused unit tests beside implementation in `#[cfg(test)] mod tests`; put binary-level behavior in `tests/cli.rs`.

## Naming

- Files, modules, functions, and local variables: `snake_case`.
- Types and enum variants: `UpperCamelCase`.
- Constants: `UPPER_SNAKE_CASE`.
- Command modules use product or capability names (`upgrade`, `release`, `zed`), not generic `helpers` or `utils` buckets.

## Representative Paths

- `src/main.rs`: exact argument-shape dispatch and exit-code boundary.
- `src/node/mod.rs`: feature orchestration over small submodules.
- `src/node/command.rs`: shared process boundary with an injectable `Runner`.
- `src/node/fs.rs`: shared guarded mutation path.
- `tests/cli.rs`: spawned-binary assertions for stdout, stderr, and status.

## Avoid

- Do not introduce server-layer names such as route, controller, or repository; no such architecture exists here.
- Do not create a shared abstraction before multiple callers need it. Keep one-use logic in its command module.
- Do not move interactive prompts, filesystem mutation, and command execution into one large command handler; existing node code separates these boundaries for testing and safety.
