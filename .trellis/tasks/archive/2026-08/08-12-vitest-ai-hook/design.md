# Vitest AI hook design

## Boundaries

### Public command

`src/main.rs` extends the existing Clap subcommand tree:

```text
jt vitest ai-hook --codex
jt vitest ai-hook --claude
```

One required, mutually exclusive flag selects Codex or Claude. Codex dispatch calls installer. Claude dispatch returns explicit not-implemented failure. Clap rejects missing, combined, or unknown flags with usage error `2`; completion generation includes the visible `vitest` tree. Hidden `__vitest-hook` stays out of public help and completions.

### Feature module

Add `src/vitest.rs` with two narrow entry points:

- installer called by public command;
- hidden hook-runtime entry called only by generated Codex command.

Keep parsing, merge, execution, JSON formatting, and focused unit tests in this module. No shared abstraction until another command needs one.

### Installed hook

Installer writes only `.codex/hooks.json`. Owned group:

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "jt __vitest-hook",
            "timeout": 150,
            "statusMessage": "Running Vitest"
          }
        ]
      }
    ]
  }
}
```

Direct `jt` runtime avoids copied TypeScript/JavaScript, `tsx`, custom reporter, and new target dependencies. Hidden command stays absent from public help.

## Installation Flow

1. Resolve Git root with `git -C <cwd> rev-parse --show-toplevel`.
2. Read root `package.json` as a JSON object.
3. Accept only direct `vitest` declarations in `dependencies`, `devDependencies`, `peerDependencies`, or `optionalDependencies`.
4. Read and validate existing `.codex/hooks.json` before mutation.
5. Preserve all unrelated JSON. Find owned matcher group by exact handler command and replace it; append when absent.
6. Serialize stable pretty JSON with trailing newline.
7. Use existing guarded atomic filesystem writer. Refuse symlinked target/parents and concurrent content changes.
8. Report created/updated/already-configured state plus `/hooks` trust reminder.

No backup needed: merge is ownership-scoped, atomic, and preserves unrelated values. Invalid or unsafe input fails before write.

## Runtime Flow

1. Read bounded hook JSON from stdin. Validate event/input fields without panicking.
2. Resolve Git root from hook `cwd`.
3. Locate `<root>/node_modules/.bin/vitest`; missing executable becomes bounded setup feedback.
4. Create a temporary directory for JSON report, stdout, and stderr.
5. Spawn executable once with discrete arguments:

```text
run --reporter=json --reporter=tap-flat --reporter=default --outputFile.json=<temporary-report> --silent --no-color
```

6. Redirect combined TAP/default stdout and stderr to temporary files so subprocess output never reaches Codex directly. `--silent` suppresses test console logs.
7. Poll the child with `try_wait`; kill it after 120 seconds so the runtime can return bounded feedback before Codex's 150-second handler timeout.
8. Reject report above fixed size before JSON parsing.
9. Parse Jest-compatible JSON, TAP records, and only recognized terminal fallback blocks. Normalize, join, deduplicate, and group them before rendering.
10. Delete temporary directory through `tempfile` ownership.
11. Return one compact Codex JSON result.

## Normalization Contract

Primary sources:

- JSON totals and `testResults` establish failed files, suites, assertions, and suite/setup messages.
- TAP `not ok` records supply assertion message, source location, and expected/actual values. This repairs cases such as Vitest 4.1 timeout JSON containing only `STACK_TRACE_ERROR`.
- JSON `coverageMap`, when present, supplies file coverage and uncovered lines. Captured coverage threshold lines supply actual and required values.
- Default-reporter output is fallback only for unhandled errors, startup/config/provider/no-tests failures, coverage thresholds, and snapshot detail absent from JSON/TAP. Vitest 4.1 can return JSON `success=true` while an unhandled error makes process status nonzero.
- Process status is authoritative. JSON `success` describes test records only.

Normalization:

1. Strip ANSI/control sequences and normalize absolute root paths to repository-relative paths.
2. Parse stable records before any free-text fallback.
3. Derive one root-cause line. Prefer TAP `message`; then suite `message`; then first non-stack JSON failure line; then recognized terminal fallback line.
4. Preserve concise expected/actual scalars. Drop multiline diffs, code frames, dependency frames, passing logs, timing summaries, and repeated guidance.
5. Join JSON and TAP by normalized file plus test name. Deduplicate identical file/location/reason records.
6. Group test failures by file. Collapse equal causes with a count.
7. Compress uncovered line numbers into ranges such as `11-14, 27`.
8. Render all normalized records that fit final payload budget; append exact omitted record count only when safety ceiling is reached.

Example:

```text
Vitest：4 个文件失败，4 个测试失败
- test/assertion.test.js:2 `adds numbers`：期望 3，实际 2
- test/assertion.test.js:6 `throws domain error`：inventory already locked
- test/setup.test.js:2 `[setup]`：database fixture unavailable
覆盖率：
- src/math.ts：行覆盖率 72% < 80%；未覆盖 11-14, 27
```

Safety ceilings remain constants beside formatter tests, applied after normalization:

- maximum 8,000 characters in model-visible reason;
- maximum 16 MiB JSON report accepted for parsing;
- bounded captured-output scan/read size.

Final truncation is Unicode-safe. No per-item character slicing in normal formatting.

Results:

- pass: `{ "continue": true }`;
- first failure: `{ "decision": "block", "reason": "<bounded summary>" }`;
- repeated failure with `stop_hook_active=true`: `{ "continue": true, "systemMessage": "<bounded summary and retry-limit note>" }`;
- malformed hook input or execution failure: same one-continuation guard, with classified concise remediation.

## Compatibility and Safety

- Official Codex project hook discovery and `Stop` protocol drive config shape.
- Existing hook sources run together; installer touches only one owned group.
- Hook command resolves work from hook input/Git root, not shell launch directory.
- Vitest built-in JSON, TAP, and default reporters avoid version-coupled custom reporter APIs. All terminal output stays captured locally.
- Coverage remains controlled by target Vitest config. Hook does not force `--coverage`; it summarizes threshold failures only when project enables coverage.
- Root-only dependency/executable scope is deliberate first-version ceiling. Workspace routing can be added when a concrete repository requires it.
- No network, dependency installation, shell interpolation of user data, or recursive deletion.

## Rollback

Remove the matcher group whose handler command is `jt __vitest-hook`. Remove empty `.codex/hooks.json` only when it contains no other user data. No other files are installed.
