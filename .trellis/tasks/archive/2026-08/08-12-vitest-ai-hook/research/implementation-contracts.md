# Vitest Codex hook research

## Sources inspected

- Current `jt` Clap command tree, Rust modules, integration tests, atomic file writer, output/error conventions, CI, and Git hook.
- `/Users/jacobzha/Documents/workspace/okr-zhuanzhuan/nlab_eslint_config` modular Codex/Claude AI-hook installer and Stop runtime.
- Official Codex Hooks documentation: <https://learn.chatgpt.com/docs/hooks>.
- Official Vitest reporters and CLI documentation: <https://vitest.dev/guide/reporters>, <https://vitest.dev/guide/cli>.
- Installed Vitest 4.1.10 JSON reporter implementation and a successful JSON-output probe in the reference repository.

## Confirmed contracts

### jt

- `src/main.rs` owns Clap subcommands, help/completion generation, and final exit codes.
- Self-contained commands belong in `src/<command>.rs`; binary behavior belongs in `tests/cli.rs`.
- `crate::node::fs::atomic_write` already provides expected-content checks, symlink refusal, private temporary siblings, fsync, and atomic rename below a validated root.
- Clap, `serde_json`, and `tempfile` are installed. New parser/runtime dependencies are unnecessary.

### Codex

- Trusted repositories discover project-local `.codex/hooks.json`; hook definitions require review/trust through `/hooks`.
- `Stop` receives JSON including `cwd`, `turn_id`, and `stop_hook_active` on stdin. Exit `0` stdout must be JSON.
- `{ "decision": "block", "reason": "..." }` continues the turn using `reason` as a new prompt.
- `stop_hook_active=true` identifies a turn already continued by Stop and enables an explicit loop guard.
- Repository hook commands should resolve work from Git root because Codex can start in a subdirectory.
- Codex spills oversized model-visible hook output, but multiple hook outputs accumulate. Local formatting/caps still reduce context use.

### Vitest

- `vitest run` is non-watch execution.
- `--reporter=json --outputFile=<path>` emits Jest-compatible JSON to a file instead of returning the full report through stdout.
- Current JSON includes totals, `success`, `testResults`, suite `name/status/message`, assertion `fullName/status`, `failureMessages`, and optional `coverageMap`.
- JSON is necessary but insufficient: Vitest 4.1 timeout JSON can contain only `STACK_TRACE_ERROR`; unhandled errors and coverage threshold failures can leave JSON `success=true` while process status is nonzero.
- Built-in `tap-flat` supplies stable assertion message/location/expected/actual fields. Captured default-reporter output supplies recognized unhandled/startup/coverage fallback blocks.

### Reference AI hook

- Reference installer preserves unrelated hook groups, replaces only owned groups, resolves scripts through Git root, and uses Stop retry limiting.
- Reference ESLint runtime caps diagnostic count, runtime detail, parse-error output, and subprocess buffer.
- Vitest does not need PreToolUse/PostToolUse state when first-version behavior runs the full root suite on every Stop.

## Recommended minimum design

- Write only one owned Stop group into `.codex/hooks.json`.
- Point it to hidden `jt __vitest-hook`; keep runtime in tested Rust instead of copying a TypeScript/JavaScript bundle or adding `tsx`.
- Detect root-declared `vitest` before installation. Do not install dependencies.
- Execute root `node_modules/.bin/vitest` once with JSON, `tap-flat`, and default reporters. Redirect stdout/stderr to temporary files.
- Merge structured JSON/TAP records; parse only recognized fallback blocks. Group and deduplicate by file/root cause before rendering. Apply one final safety budget after semantic normalization.
- Block once on failure. Re-run during the continuation; pass normally if repaired, otherwise warn and stop when `stop_hook_active=true`.

## Risks and deliberate ceilings

- Root-only dependency/executable lookup excludes workspace-package-only Vitest setups. This avoids guessing workspace routing; add workspace selection when a concrete repository requires it.
- Installed config depends on `jt` remaining available in hook `PATH`. This matches the command used to install it and avoids a copied runtime that can drift.
- Vitest may fail before producing JSON. Parse only recognized startup/config/provider/no-tests/unhandled markers plus one repository frame; never inject an arbitrary captured tail.
- An external Codex hook timeout cannot format output after Codex kills it. Runtime should enforce a shorter internal timeout so it can kill Vitest and return bounded feedback first.
