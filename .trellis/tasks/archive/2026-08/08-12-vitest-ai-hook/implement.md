# Vitest AI hook implementation plan

## Implementation

- [x] Add `vitest` module and CLI/help dispatch for `--codex`, deferred `--claude`, and hidden runtime command.
- [x] Implement Git-root and root `package.json` Vitest prerequisite checks before mutation.
- [x] Implement ownership-scoped `.codex/hooks.json` merge using existing guarded atomic writes.
- [x] Implement hidden Stop runtime using root `node_modules/.bin/vitest`, built-in JSON, `tap-flat`, and default reporters, `--silent`, temporary captured files, 120-second internal timeout, and no shell command construction.
- [x] Implement JSON/TAP/recognized-terminal normalization: assertion, suite/setup, snapshot, timeout, no-tests/config, unhandled/runtime, and coverage threshold categories; use process status as final verdict.
- [x] Join and deduplicate records by file/test/location, group repeated root causes, reduce expected/actual values, and compress uncovered line ranges.
- [x] Render concise report with one final safety budget and single-continuation retry guard; never include raw output, stack collections, code frames, or diffs.
- [x] Add focused unit tests for detection, merge preservation/idempotence, unsafe/invalid inputs, real Vitest JSON/TAP fixtures, classification, joining, deduplication, coverage ranges, Unicode-safe final truncation, runtime errors, and retry limit.
- [x] Add CLI integration tests for help, invalid shapes, missing Vitest no-mutation, deferred Claude, nested installation, safe/idempotent merge, invalid JSON, and symlink refusal.
- [x] Update `README.md` command documentation and limitations.

## Validation

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
git diff --check
python3 ./.trellis/scripts/task.py validate .trellis/tasks/08-12-vitest-ai-hook
```

Manual fixture check after automated tests:

```bash
jt vitest ai-hook --codex
```

Inspect generated `.codex/hooks.json`; do not trust or execute repository hooks automatically.

## Review Gates

- No product edit before final plan approval and `task.py start`.
- Re-read current Codex Hooks docs and research artifact if hook schema changes during implementation.
- Review exact staged paths before local commit; preserve unrelated changes.
- No push or PR without separate user authorization.

## Rollback Points

- CLI/config merge can be reverted independently before runtime work.
- Runtime is reachable only through owned hook command; removing owned Stop group disables feature.
- If Vitest JSON differs from expected schema, fail with bounded runtime feedback; never fall back to raw output.
