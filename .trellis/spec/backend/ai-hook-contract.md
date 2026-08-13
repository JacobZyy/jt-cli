# AI Hook Contract

> Executable contract for project-local Codex hooks installed by `jt`.

## 1. Scope / Trigger

Apply this contract when changing `jt vitest ai-hook`, installed `.codex/hooks.json` groups, or
TypeScript templates under `templates/vitest-ai-hook/`. Hook input, patch paths, existing repository
configuration, state files, and subprocess output are trust boundaries.

## 2. Ownership Boundary

```text
jt vitest ai-hook --codex   # install/update templates and Codex config
jt vitest ai-hook --claude  # explicit unsupported error; no mutation
```

`jt` installs only. Runtime lives in target repository:

```text
.codex/hooks/jt-vitest/
├── pre-tool-use.ts
├── post-tool-use.ts
├── stop.ts
└── supporting TypeScript modules
```

Every owned template contains `jt-vitest-ai-hook`. Installer may replace marked files, but must reject
an unmarked collision. Runtime commands use project-local `tsx`; they never call back into `jt`.

## 3. Three-Stage Flow

1. `PreToolUse`: for Codex `apply_patch`, parse candidate paths and store pre-edit fingerprints.
2. `PostToolUse`: compare fingerprints; record only files whose content or existence changed.
3. `Stop`: gather all records for current repository/session/turn and invoke Vitest once.

State lives under `/tmp`, expires after 24 hours, and is keyed by repository, session, turn, and tool
use. Reject paths outside Git root and paths resolving through outside symlinks. Deleted files remain
eligible because they can affect related tests.

## 4. Vitest Contract

Resolve target repository's installed `vitest` package, then run:

```text
vitest related <all AI-edited files> --run --reporter=agent --coverage.enabled \
  --coverage.include=<each AI-edited file> --coverage.reporter=text \
  --silent --no-color --passWithNoTests
```

- Pass every changed file in one invocation.
- Run complete related test files; never filter individual test names.
- Exclude unrelated pre-existing working-tree changes.
- Use Vitest's native `agent` reporter; do not maintain a custom Vitest report parser.
- Force coverage for the AI-edited files and return the text report without creating report files.
- Inherit project coverage provider, exclusions, and thresholds. Do not pass `skipFull`; leave project
  configuration and Vitest's AI-agent default intact. A threshold failure blocks through Vitest's
  non-zero exit. Do not hard-code coverage policy or install dependencies.
- Bound captured output and process time.

## 5. Stop Results

| Condition | Result |
|-----------|--------|
| No files recorded | `{"continue":true}`; no Vitest process |
| Vitest unavailable | Continue with visible setup warning; clear turn state |
| Vitest exit `0` | Continue with bounded coverage text; clear turn state |
| First test/runtime failure | `{"decision":"block","reason":"..."}`; retain state for repair |
| Failure with `stop_hook_active=true` | Continue with retry-limit warning; clear turn state |

## 6. Installer Contract

- Resolve Git root from any subdirectory.
- Root `package.json` must directly declare `vitest` and `tsx`.
- Sync every owned template before merging three config groups.
- Migrate legacy `jt __vitest-hook` Stop group.
- Preserve unrelated top-level fields, events, and groups.
- Preserve existing JSON key order; write atomically; reject invalid JSON, symlinked paths, and
  concurrent changes.
- Re-running unchanged installation must be byte-stable.

## 7. Required Checks

- CLI: invalid arguments, non-Git root, missing tooling, deferred Claude, nested install,
  three-stage config, legacy migration, template sync, preservation, idempotence, invalid JSON,
  unowned collision, symlink refusal.
- Runtime fixture: no-op patch skip, changed-file collection, related full-suite invocation, passing
  cleanup, failure/repair retention, retry-limit cleanup, missing Vitest, outside-root rejection.
- Full Rust gates: `cargo fmt --check`, Clippy with warnings denied, `cargo test --locked`,
  `git diff --check`.
