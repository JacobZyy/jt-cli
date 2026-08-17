# AI Hook Contract

> Executable contract for project-local hooks installed by `jt ai-hook`.

## 1. CLI / Ownership

```text
jt ai-hook                                      # interactive final-state selection
jt ai-hook --checks vitest,eslint --agents codex
jt vitest                                       # reserved placeholder
```

- Questionnaire uses two optional multi-selects: checks and agent terminals.
- Current installation is preselected. Submitted selection is final state, so deselection detaches
  maintained runners.
- Non-interactive mode requires both `--checks` and `--agents`.
- Claude Code is deferred. `codex` is current agent choice.
- Root `package.json` must directly declare `tsx` and each selected check.
- Every owned template contains `jt-ai-hook`; unmarked collisions are rejected.

Runtime layout:

```text
.codex/hooks/jt-ai-hook/
├── pre-tool-use.ts
├── post-tool-use.ts
├── stop-entry.ts
└── stop/
    ├── process.ts
    ├── types.ts
    ├── runner/
    │   ├── eslint.ts
    │   └── vitest.ts
    └── support/vitest-coverage.ts
```

## 2. Shared Edit Collection

1. `PreToolUse`: for Codex `apply_patch`, parse candidate paths and store fingerprints.
2. `PostToolUse`: compare fingerprints; record only changed content or existence.
3. `Stop`: gather all records for repository/session/turn once, then dispatch runners.

State lives under `/tmp`, expires after 24 hours, and is keyed by repository, session, turn, and tool
use. Reject paths outside Git root and paths resolving through outside symlinks. Deleted files remain
eligible for Vitest related-test selection.

## 3. Stop Entry / Plugins

- Discover only direct regular `.ts` files under `stop/runner/`; ignore directories and symlinks.
- Sort runner filenames for deterministic reporting.
- Each module exports async `run(context)` and returns `passed`, `warning`, or `failed`.
- Start all runners through `Promise.allSettled`; one failure never cancels another runner.
- Stop entry alone owns state cleanup, `stop_hook_active`, logging, and Codex stdout JSON.
- Runners never write stdout or clear shared state.
- All passed: continue silently and clear state.
- Warnings only: continue with one combined setup warning and clear state.
- First failure: combine failed/warning sections, block, retain state for repair.
- Failure with `stop_hook_active=true`: combine results, continue, clear state to prevent loops.

## 4. Process Isolation

- Use `child_process.spawn` asynchronously with argv arrays and `shell: false`.
- Bound output and timeout each process. Terminate timed-out children.
- Every ESLint and Vitest process receives exact `isInAIHook=true` plus `NO_COLOR=1`.
- Never use in-process ESLint/Vitest APIs. Keep `process.env`, `process.exitCode`, logger, Vite server,
  and module state isolated. Parallel runners must represent real concurrent processes.

## 5. Vitest Runner

```text
vitest related <all AI-edited files> --run --reporter=agent --coverage.enabled \
  --coverage.include=<each eligible AI-edited file> --coverage.reporter=json-summary \
  --coverage.reportsDirectory=<temporary directory> --coverage.reportOnFailure \
  --silent --no-color --passWithNoTests
```

- Resolve project Vitest config in an isolated child process.
- Select `AI-edited files intersect coverage.include minus coverage.exclude`.
- No coverage-eligible file: pass `--coverage.enabled=false`.
- Inherit provider, thresholds, exclusions, and `skipFull`; never pass a `skipFull` override.
- Parse temporary `coverage-summary.json`, render one Markdown table, always delete temp directory.
- Passing output remains log-only. Coverage-only failure returns table plus threshold conclusions.
- Test/runtime failures retain bounded native `agent` output. Never return native coverage output.

## 6. ESLint Runner

- Check only existing supported edited files.
- Resolve project ESLint and invoke its Node CLI with JSON formatter and `--no-warn-ignored`.
- Parse structured results; return at most 50 severity-2 diagnostics.
- Missing ESLint is a warning. Config/runtime/JSON failures and lint errors fail runner.

## 7. Installer / Migration

- Resolve Git root from any subdirectory.
- Write selected built-in runner files; remove deselected owned built-in runner files.
- Preserve unknown custom runner files.
- Merge one shared PreToolUse, PostToolUse, and Stop group.
- Remove old `.codex/hooks/jt-vitest`, `jt __vitest-hook`, and
  `.codex/hooks/nlab-eslint` handlers at handler level. Preserve unrelated handlers in mixed groups.
- Reject invalid JSON, symlinked paths, unowned target collisions, and concurrent changes.
- Preserve unrelated top-level fields/events and JSON key order. Re-run must be byte-stable.

## 8. Required Checks

- CLI shape, no-TTY guidance, placeholder command, Git/dependency validation.
- Nested installation, old-hook migration, mixed-group preservation, one shared group per event.
- Runner attach/detach, custom-runner preservation, template ownership, idempotence.
- Async `spawn`, `shell: false`, `Promise.allSettled`, both runner environments.
- Structured Vitest coverage, project filtering/`skipFull`, compact failure output, temp cleanup.
- Invalid JSON, unowned collision, symlink refusal.
- `cargo fmt --check`, Clippy warnings denied, `cargo test --locked`, `git diff --check`.
