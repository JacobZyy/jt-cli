# Research: Vitest output normalization for an AI hook

- Query: How should `jt` extract and compact Vitest 4/5 failures, including coverage, without losing failure classes or injecting raw output?
- Scope: mixed; Vitest 4.1.10 source/runtime probes plus official Vitest 4 and current Vitest 5/main documentation
- Date: 2026-08-12

## Findings

### Decision

Use process exit status as final verdict. Merge three bounded evidence channels:

1. JSON reporter: totals, file/test identity, assertion stacks, suite-level messages, snapshot summary, optional `coverageMap`.
2. `tap-flat`: error name/message/location and assertion `actual`/`expected`; especially fixes Vitest 4.1.10 timeout's broken JSON message.
3. One BaseReporter fallback: `minimal`/`agent` on Vitest >=4.1, or `default` across Vitest 4/5. It is required for unhandled errors and useful for snapshot diffs. Capture it to a temporary file; never return it raw.

Cross-version command shape:

```text
vitest run --reporter=json --reporter=tap-flat --reporter=default \
  --outputFile.json=<temporary-report> --no-color
```

For a known Vitest >=4.1 installation, replace `default` with `minimal`. Explicit reporters disable automatic agent-reporter selection. `minimal`/`agent` was introduced in 4.1, so do not invoke it unconditionally against 4.0.

If implementation keeps JSON only, it must downgrade `exit != 0 && JSON success == true` to a generic runtime diagnostic. JSON alone cannot name unhandled errors, and it gives an unusable timeout message in 4.1.10.

### Confirmed 4.1.10 reporter contracts

Installed package: `/Users/jacobzha/Documents/workspace/okr-zhuanzhuan/nlab_eslint_config/node_modules/vitest`, version `4.1.10`.

- JSON is Jest-compatible and exposes numeric totals, `success`, `snapshot`, `testResults`, and optional `coverageMap`: `node_modules/vitest/dist/chunks/reporters.d.DtoKVV2s.d.ts:2291-2351`.
- JSON assertion identity is `fullName`; failures are `failureMessages`, populated from `error.stack || error.message`: `node_modules/vitest/dist/chunks/index.UpGiHP7g.js:3559-3584`.
- File/import/setup/suite failure text is only the first file error's message in `testResults[].message`: `node_modules/vitest/dist/chunks/index.UpGiHP7g.js:3587-3595`.
- JSON `success` checks discovered files plus failed suites/tests only. It does not receive unhandled errors or coverage-threshold status: `node_modules/vitest/dist/chunks/index.UpGiHP7g.js:3538-3553`.
- `coverageMap` is assigned only through `onCoverage` and serialized with the JSON result: `node_modules/vitest/dist/chunks/index.UpGiHP7g.js:3535-3537,3597-3612`.
- TAP-flat flattens file/suite/test names. TAP error blocks contain `name`, `message`, first `at`, and optional `actual`/`expected`: `node_modules/vitest/dist/chunks/index.UpGiHP7g.js:3896-3955,3967-3983`.
- BaseReporter prints `Unhandled Errors` and full error detail: `node_modules/vitest/dist/chunks/cli-api.BK8pd4xc.js:2020-2031`.
- Core marks unhandled errors as exit failure, independently of reporter JSON: `node_modules/vitest/dist/chunks/cli-api.BK8pd4xc.js:13913-13915`.
- Snapshot summary provides added/removed/unmatched/unchecked counts and affected files/keys: `node_modules/.pnpm/@vitest+snapshot@4.1.10/node_modules/@vitest/snapshot/dist/rawSnapshot.d-D_X3-62x.d.ts:177-195`.

### Failure matrix

| Failure class | JSON | TAP-flat | BaseReporter / output fallback | Normalized result |
|---|---|---|---|---|
| Assertion | Failed assertion, `fullName`, stack/message; no separate diff fields | Exact message, location, often `actual` and `expected` | Human diff and source frame | Merge by file + test; prefer TAP fields, cap each side |
| Snapshot mismatch | Failed assertion says snapshot key mismatched; `snapshot.unmatched/filesUnmatched` counts | Name/message/location, but probe had no expected/received payload | Minimal/default contains `- Expected/+ Received` diff | Report key/count; include only a small bounded diff excerpt if available |
| Import/setup failure | Failed file, empty assertions, `testResults[].message` | File-level error usually present | Full stack/source | Suite diagnostic, not “0 failed tests means pass” |
| `beforeAll`/`afterAll` | Failed file message; child tests may be skipped | 4.1.10 probe showed only skipped child for `beforeAll` | “Failed Suites” block | JSON suite message wins |
| `beforeEach`/`afterEach` | Same hook stack may repeat on every failed test | Repeated test blocks | Full detail | Group identical root fingerprint and list affected test count |
| Timeout | 4.1.10 probe returned only `Error: STACK_TRACE_ERROR` | Correct “Test timed out in 20ms…” plus location | Correct timeout text | Detect JSON placeholder; replace from TAP/output |
| Unhandled rejection/error | **Missing**; probe JSON said `success: true`, passed test | **Missing**; probe TAP said `ok` | BaseReporter prints unhandled type/message/frame | Exit status plus fallback block; separate from test failures |
| No matching files | `success: false`, all totals zero, empty `testResults` | Empty plan is not explanatory | “No test files found” plus filter | Discovery diagnostic; respect `passWithNoTests` via final exit status |
| Matched file with no tests | Failed file, zero assertions, message “No test suite found in file …” | File-level error | Same text | File discovery/collection diagnostic |
| Config/provider/runtime error | Report may not exist | May not initialize | stderr/stdout only | One bounded runtime diagnostic plus rerun command |

Probe-specific evidence, all with Vitest 4.1.10:

- Assertion JSON contained the assertion stack but not separate expected/actual; TAP-flat contained both values.
- Setup-file error and import error produced failed `testResults[].message` with zero assertions.
- `beforeAll` produced one failed suite, zero failed tests, and one skipped test.
- Unhandled rejection produced process exit `1` while JSON reported `success: true`; neither JSON nor TAP-flat named the rejection.
- No-match run still wrote valid JSON with zero files/tests and `success: false`.

### Coverage rules

Answer to “does coverage data exist without `--coverage`?”: only if config has `coverage.enabled: true`. If neither CLI nor config enables coverage, no collection happens and Vitest 4.1.10's `coverageMap` property is omitted by `JSON.stringify` (not an empty authoritative map). Never treat a stale `coverage/` directory as current-run evidence.

- Coverage defaults to disabled: `node_modules/vitest/dist/chunks/defaults.9aQKnqFk.js:15-28`.
- When enabled, JSON receives the Istanbul-compatible coverage map before `onTestRunEnd`: `node_modules/vitest/dist/chunks/cli-api.BK8pd4xc.js:12601-12607`.
- Provider reporting and threshold checks occur after JSON `onTestRunEnd`: `node_modules/vitest/dist/chunks/cli-api.BK8pd4xc.js:13604-13608`. Therefore JSON `success: true` can coexist with final exit `1` from a coverage threshold.
- Positive threshold message shape is `ERROR: Coverage for <metric> (<actual>%) does not meet <scope> threshold (<required>%) [for <file>]`; negative thresholds use `ERROR: Uncovered <metric> (<count>) exceed <scope> threshold (<limit>) [for <file>]`: `node_modules/vitest/dist/chunks/coverage.DM_a_rWm.js:857-903`.
- `thresholds.perFile` appends a repository-relative file path. Glob threshold scope is quoted; global scope is `global`.
- Vitest 4 coverage defaults generate `text`, `html`, `clover`, and `json` artifacts: `node_modules/vitest/dist/chunks/defaults.9aQKnqFk.js:20-28`. The JSON artifact/JSON reporter `coverageMap` contains per-file Istanbul maps (`statementMap/s`, `fnMap/f`, `branchMap/b`). Derive uncovered lines from zero-count statements grouped by start line, then compact consecutive lines to ranges. Prefer current-run `coverage-final.json` if deliberately generated; otherwise use reporter `coverageMap`.
- Coverage artifacts do not encode configured threshold failures. Pair them with final exit status and bounded `ERROR: Coverage…` / `ERROR: Uncovered…` lines.
- When tests fail and `coverage.reportOnFailure` is false (default), normal coverage artifacts may not be generated. Do not make them mandatory for test-failure reporting.

### Stable extraction and merge algorithm

1. Validate size and schema, then parse JSON. Ignore unknown keys for Vitest 4/5 compatibility.
2. Build structured diagnostics from failed assertions first; add failed files with non-empty `message` even when `numFailedTests == 0`.
3. Parse bounded TAP-flat blocks. Join to JSON by normalized repository-relative file + full test name. Replace placeholder/empty JSON messages; add TAP location and capped expected/actual.
4. Parse only known fallback markers from captured output: unhandled blocks, no-tests line, snapshot headline/small diff, coverage threshold lines, and fatal config/provider messages. Do not inject arbitrary captured tails as test details.
5. Final verdict is process status. JSON `success` is descriptive test state, never exit authority.
6. Deduplicate before applying count/character limits.

Recommended semantic keys:

- assertion/snapshot/timeout: `kind + file + fullName`;
- suite/import/setup/hook: `kind + file + normalized message + first repository frame`;
- repeated hook failures: `file + normalized message + first repository frame`, with affected test names/count;
- unhandled: `error type + normalized message + first repository frame`;
- coverage threshold: `scope(global|glob|file) + metric + file?`;
- uncovered lines: `file`;
- discovery/runtime: one per normalized message class.

Normalization:

- strip ANSI and unsafe controls; normalize `\\` to `/`; replace Git root with repository-relative paths;
- retain first repository frame, discard dependency frames by default;
- collapse whitespace only outside expected/actual/diff payloads;
- fingerprint before truncation, then cap values and whole report;
- group line numbers into ranges (`12-14, 28`);
- count omitted diagnostics after semantic dedup, not raw assertion count.

### Compact Chinese report examples

Assertion plus diff:

```text
Vitest 失败：1 文件，1/18 测试失败
- test/cart.test.ts › 合计金额：对象不相等
  test/cart.test.ts:42:18
  实际 { total: 99 }；期望 { total: 100 }
修复后运行：vitest run
```

Suite/hook grouping:

```text
Vitest 失败：1 套件，2 个测试受影响
- [共享失败] test/api.test.ts：BEFORE_EACH_BOOM
  test/api.test.ts:8:9；影响“创建订单”等 2 项
```

Snapshot, timeout, unhandled, and discovery:

```text
Vitest 失败：4 类问题
- test/view.test.ts › 默认视图：快照 `默认视图 1` 不一致
- test/poll.test.ts › 等待完成：20ms 超时（test/poll.test.ts:13:5）
- [未处理拒绝] UNHANDLED_BOOM（test/jobs.test.ts:27:18）
- [发现] 未找到匹配测试文件
```

Coverage:

```text
Vitest 测试通过，但覆盖率未达标
- 全局 lines：72.4% < 80%
- src/parser.ts lines：41.2% < 80%；未覆盖 12-14, 28
```

### Version boundary

- Vitest 4.1 introduced `minimal`/`agent` and automatic AI-agent detection. It is terminal-oriented, not a replacement for JSON.
- Explicit JSON `outputFile` is the stable choice across Vitest 4 and Vitest 5/main. Vitest 5 changes default artifact locations, but an explicit temporary path avoids coupling to them.
- Custom reporter APIs are documented as advanced and may change within a minor. Built-in JSON/TAP/default reporters avoid shipping a version-coupled reporter.
- Vitest 5/main behavior was checked from official docs, not locally executed. Parser should tolerate absent optional fields and unknown additions.

### Files found

- `/Users/jacobzha/Documents/workspace/okr-zhuanzhuan/nlab_eslint_config/node_modules/vitest/dist/chunks/index.UpGiHP7g.js` — installed JSON, TAP, minimal, and other reporter implementations.
- `/Users/jacobzha/Documents/workspace/okr-zhuanzhuan/nlab_eslint_config/node_modules/vitest/dist/chunks/cli-api.BK8pd4xc.js` — run ordering, unhandled-error exit handling, reporter callbacks, coverage reporting.
- `/Users/jacobzha/Documents/workspace/okr-zhuanzhuan/nlab_eslint_config/node_modules/vitest/dist/chunks/coverage.DM_a_rWm.js` — threshold comparison and emitted error formats.
- `/Users/jacobzha/Documents/workspace/okr-zhuanzhuan/nlab_eslint_config/node_modules/vitest/dist/chunks/defaults.9aQKnqFk.js` — coverage defaults.
- `/Users/jacobzha/Documents/workspace/okr-zhuanzhuan/nlab_eslint_config/node_modules/vitest/dist/chunks/reporters.d.DtoKVV2s.d.ts` — JSON report types.
- `.trellis/tasks/08-12-vitest-ai-hook/prd.md` — bounded-feedback requirements and coverage out-of-scope boundary.
- `.trellis/tasks/08-12-vitest-ai-hook/design.md` — proposed JSON-only runtime and limits.
- `.trellis/tasks/08-12-vitest-ai-hook/research/implementation-contracts.md` — prior hook/reporter research.

### External references

- Vitest 4 reporters: <https://v4.vitest.dev/guide/reporters>
- Vitest 4 CLI/coverage flags: <https://v4.vitest.dev/guide/cli>
- Vitest 4 coverage guide: <https://v4.vitest.dev/guide/coverage>
- Vitest 4.1 agent reporter announcement: <https://vitest.dev/blog/vitest-4-1>
- Vitest 4.1.10 JSON reporter source: <https://github.com/vitest-dev/vitest/blob/v4.1.10/packages/vitest/src/node/reporters/json.ts>
- Current Vitest reporter docs: <https://main.vitest.dev/guide/reporters>
- Current Vitest migration guide: <https://main.vitest.dev/guide/migration.html>

### Related specs

- `.trellis/spec/backend/error-handling.md` — preserve actionable error context without leaking unbounded subprocess output.
- `.trellis/spec/backend/logging-guidelines.md` — stdout/stderr and redaction are CLI contracts.
- `.trellis/spec/backend/quality-guidelines.md` — external-input validation and focused parser regression tests.

## Caveats / Not Found

- JSON-only design cannot faithfully report unhandled errors or 4.1.10 timeouts. This is a confirmed gap, not a hypothetical compatibility concern.
- TAP-flat does not carry unhandled errors and did not carry a `beforeAll` suite error in the probe; it is enrichment, not primary truth.
- Snapshot expected/received payload was absent from both JSON and TAP-flat in the probe. A bounded BaseReporter excerpt is needed if the compact report must show that diff.
- No machine-readable built-in artifact records “threshold X failed”; final process status plus bounded provider error lines remain necessary.
- Coverage provider was not installed in the reference repository, so coverage runtime output was verified from installed Vitest source and official docs rather than an end-to-end local coverage run.
- Probe covered Node pool/common failures, not browser mode, typecheck mode, shards, or custom reporters.
