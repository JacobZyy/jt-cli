# Add Vitest AI hook command

## Goal

Add `jt vitest ai-hook --codex` so a Vitest repository can install a project-local Codex `Stop` hook. The hook runs the repository's Vitest suite once, converts Vitest results into a short file-oriented failure report for one repair pass, and never injects raw test output into model context.

## Background

- `jt` uses a Clap subcommand tree in `src/main.rs`; the new command must preserve its validation, help, and shell-completion behavior.
- Repository-local Codex hooks live in `.codex/hooks.json`. A `Stop` hook receives JSON on stdin and must return JSON on stdout.
- Vitest provides a Jest-compatible JSON reporter plus machine-oriented TAP reporters. JSON carries totals, suite failures, and optional coverage data; TAP carries assertion messages, locations, and expected/actual values that JSON can lose for timeout failures.
- Clap, `serde_json`, and `tempfile` already exist in `jt`; this feature requires no new dependency.
- Current scope implements Codex only. Claude remains a declared but unsupported target.

## Requirements

### R1. CLI contract

- Add `jt vitest ai-hook --codex`.
- Recognize `jt vitest ai-hook --claude`, report that Claude support is not implemented, return failure, and perform no mutation.
- Reject missing, combined, or unknown environment flags as invalid usage.
- List the new command and both target flags in help.

### R2. Vitest prerequisite

- Resolve the current Git repository root before inspection or mutation.
- Treat the repository as Vitest-enabled only when its root `package.json` declares `vitest` in a dependency field.
- If the repository is not Vitest-enabled, stop before writing `.codex` files and return a concise actionable error.
- Do not install Vitest, Node.js, dependencies, or package-manager tooling.

### R3. Codex hook installation

- Create or merge `.codex/hooks.json` at the Git root.
- Add one owned `Stop` command hook without removing unrelated hook events, matcher groups, handlers, or top-level fields.
- Re-running the command must be idempotent and must upgrade the owned hook definition in place.
- Reject invalid existing JSON and unsafe symlinked write paths before mutation.
- Tell the user to review and trust the repository hook with `/hooks`.

### R4. Vitest execution

- The installed `Stop` hook must invoke `jt`'s maintained runtime, not generate a second package-manager integration.
- Run the repository-local Vitest executable once in non-watch mode with built-in JSON, `tap-flat`, and terminal reporters.
- Run from the Git root, even when Codex starts in a nested directory.
- Remove temporary report and captured-output files after every run.

### R5. Semantic AI feedback

- Passing tests return `{ "continue": true }` without full Vitest output.
- Parse JSON, TAP, terminal fallback, and process status into typed failures: test assertion, suite/setup, snapshot, timeout, unhandled/runtime, no-tests/config, and coverage threshold.
- Extract only repository-relative file, line, test name, root-cause message, and useful expected/actual values.
- Merge duplicate records from JSON and TAP, group by file, collapse repeated causes, and compress uncovered line numbers into ranges.
- Coverage threshold failures report actual versus required coverage. When JSON supplies a coverage map, report affected files and uncovered line ranges.
- Strip ANSI/control sequences, progress, passed-test output, console logs, timings, code frames, diffs, dependency frames, and repeated stack traces.
- Runtime/setup failures return one classified diagnostic plus a command the agent can run for full local detail.
- Treat process status as final verdict; JSON `success` alone cannot prove success because unhandled errors and coverage thresholds may only change process status.
- Never return raw stdout, stderr, JSON reports, or stack collections to Codex. A final total-size ceiling and omitted-count marker remain only as a malformed/adversarial-output safety net after semantic normalization.
- On the first failure, request one continuation with `decision: "block"`. If `stop_hook_active` is already true and tests still fail, allow stop with a warning to prevent an infinite loop.

### R6. Documentation and compatibility

- Document installation, Vitest prerequisite, `/hooks` trust, automatic execution, normalized report shape, coverage behavior, retry limit, and deferred Claude support in `README.md`.
- Keep existing commands and their exit-code behavior unchanged.
- Add no Rust dependency for this feature and no target-repository dependency.

## Acceptance Criteria

- [x] AC1: `jt --help` lists `jt vitest ai-hook <--codex|--claude>`; existing help contracts still pass.
- [x] AC2: `jt vitest ai-hook --codex` outside Git or without root-declared Vitest exits nonzero and writes no `.codex` files.
- [x] AC3: Codex installation creates a valid `.codex/hooks.json`, preserves unrelated content, replaces only its owned `Stop` group, and is byte-stable on the second run.
- [x] AC4: `--claude` exits nonzero with an explicit deferred-support message and writes nothing.
- [x] AC5: Hook runtime executes repository-local `vitest run` with JSON reporter/output file from Git root.
- [x] AC6: Passing report continues silently; failing report continues Codex once with a grouped, deduplicated file/reason report; repeated failure cannot loop indefinitely.
- [x] AC7: Assertion, suite/setup, snapshot, timeout, unhandled error, no-tests/config, coverage threshold, missing executable, oversized/invalid report, and process timeout produce classified feedback without raw output injection; nonzero process status never becomes a false pass.
- [x] AC8: Unit and CLI tests cover prerequisite detection, safe/idempotent merge, JSON/TAP normalization and joining, coverage line-range formatting, final safety truncation, retry behavior, and argument dispatch.
- [x] AC9: `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings`, `cargo test --locked`, and `git diff --check` pass.

## Out of Scope

- Claude hook installation or runtime.
- Installing or upgrading Vitest or package-manager dependencies.
- Workspace-package-only Vitest installations when the Git-root package does not declare `vitest`.
- Changed-file or related-test selection; the first version runs the root suite.
- Forcing coverage on, changing coverage thresholds/reporters, custom reporters, watch mode, CI configuration, or Vitest config changes. Project-enabled coverage is summarized when available.
- Other test frameworks.
