# AI Hook Contract

> Executable contract for project-local Codex hooks maintained by `jt`.

## 1. Scope / Trigger

Apply this contract when adding or changing `jt vitest ai-hook`, its installed `.codex/hooks.json` handler, or hidden hook runtime. Hook input, subprocess output, and existing repository configuration are trust boundaries.

## 2. Signatures

```text
jt vitest ai-hook --codex   # install/update Codex hook
jt vitest ai-hook --claude  # explicit unsupported error; no mutation
jt __vitest-hook            # hidden Stop runtime; JSON stdin/stdout
```

Installed handler:

```json
{
  "type": "command",
  "command": "jt __vitest-hook",
  "timeout": 150,
  "statusMessage": "Running Vitest"
}
```

Vitest execution uses repository-local `node_modules/.bin/vitest` once:

```text
run --reporter=json --reporter=tap-flat --reporter=default --silent --no-color --outputFile.json=<temporary-file>
```

## 3. Contracts

- Install from any Git subdirectory; resolve Git root before reading or writing.
- Root `package.json` must directly declare `vitest` in a dependency field. Never install dependencies.
- Merge only handler command `jt __vitest-hook`; preserve unrelated top-level fields, events, groups, and sibling handlers. Write atomically; reject symlinked targets/parents and concurrent changes.
- Stop input is bounded JSON. Read `cwd` and `stop_hook_active`.
- Process status is final verdict. Reporter `success` is descriptive only; unhandled errors and coverage thresholds can leave it `true` while process exits nonzero.
- Merge JSON totals/suites/coverage, TAP test/message/location/expected/actual, and recognized terminal fallback blocks. Never return raw stdout/stderr, stacks, code frames, diffs, or console logs.
- Coverage stays controlled by target config. Report coverageMap uncovered lines only when current terminal output proves a threshold failure.
- First failure returns `{"decision":"block","reason":"..."}`. When `stop_hook_active=true`, return `{"continue":true,"systemMessage":"...retry limit..."}`.
- Passing run returns `{"continue":true}`. Model-visible text stays within 8,000 Unicode characters and drops whole diagnostics with an omitted count.

## 4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| Outside Git / root Vitest absent | Exit `1`; no `.codex` mutation |
| Missing/combined/unknown target flag | Exit `2`; no mutation |
| `--claude` | Exit `1`; explicit deferred message; no mutation |
| Invalid hooks JSON / symlink path / concurrent change | Exit `1`; preserve original data |
| Vitest exit `0` | Continue silently, regardless of stale reporter metadata |
| Assertion/suite/snapshot/timeout/unhandled/coverage failure | One semantic grouped report |
| Report missing/invalid/oversized or process timeout | Classified bounded setup/runtime report |
| Repeated Stop failure | Continue with warning; never block twice |

## 5. Good / Base / Bad Cases

- Good: `test/cart.test.ts:42 合计金额: expected 100; received 99`.
- Base: passing suite returns only `{"continue":true}`.
- Bad: copying full Vitest stdout, dependency stacks, snapshots, or diffs into `reason`.
- Bad: trusting JSON `success=true` when process status is nonzero.
- Bad: forcing `--coverage` or rewriting target Vitest config.

## 6. Tests Required

- CLI: help, invalid shapes, non-Git, missing root dependency, deferred Claude, nested-directory install, preservation, idempotence, invalid JSON, symlink refusal.
- Formatter fixtures: assertion expected/received, suite/setup, timeout placeholder repaired from TAP, snapshot, unhandled error with repository frame, coverage threshold plus compact uncovered ranges.
- Safety: process-status authority, bounded input/report/reason, Unicode boundary, single retry, no raw stack/dependency path.
- Full gates: `cargo fmt --check`, Clippy with warnings denied, `cargo test --locked`, `git diff --check`.

## 7. Wrong vs Correct

Wrong:

```rust
if report["success"] == true { continue_run() }
return block(raw_stdout);
```

Correct:

```rust
if process_status.success() { continue_run() }
return block(render(join(json, tap, recognized_terminal_blocks)));
```

Reason: JSON omits some timeout detail, unhandled errors, and final coverage-threshold status. Structured multi-source normalization preserves cause without context explosion.
