---
name: jt-development-workflow
description: Develop, fix, refactor, test, review, or document jt-cli through its branch, Git hook, CI, and protected-main PR flow. Use for changes to Rust code, tests, docs, GitHub Actions, hooks, release automation, or repository configuration in jt-cli.
---

# JT Development Workflow

Keep smallest correct diff. Preserve user work. Treat CI as final gate.

## 1. Inspect

- Run `git status --short --branch` before editing.
- Preserve unrelated tracked and untracked files. Never stage all files in a mixed worktree.
- Use CodeGraph first when `.codegraph/` exists. Otherwise inspect source normally.
- Read full affected flow and existing tests before choosing a fix.
- Re-read workflow, hook, and branch settings when behavior depends on mutable repository state.

## 2. Branch

- Never commit or push directly to `main`.
- Create `codex/<short-task>` before first edit unless user chose another branch.
- Base new work on current `origin/main` only when doing so will not overwrite or strand local changes.

## 3. Change

- Reuse existing code, Rust standard library, platform features, then installed dependencies.
- Avoid speculative abstractions, dependencies, configuration, and generated boilerplate.
- Fix shared root cause. Add smallest runnable regression test for non-trivial logic.
- Keep docs and examples aligned with changed behavior.

## 4. Validate

Run full local gate:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

- Repository pre-commit hook runs `cargo test --locked --quiet`. Never bypass it with `--no-verify`.
- If hook is inactive, run tests directly and report activation command:

```bash
ln -s ../../.githooks/pre-commit .git/hooks/pre-commit
```

- Local hooks are bypassable. Required GitHub CI remains authoritative.

## 5. Commit and PR

- Commit, push, open a PR, or merge only when user scope authorizes that action.
- Stage exact intended paths. Review `git diff --cached` before committing.
- Use Conventional Commits for commit messages and squash-merge PR titles, for example `feat: add config validation`.
- Push task branch. Open PR into `main`. Wait for every required check.
- Update branch when required. Merge only through PR. Never force-push `main`.

## Release Boundaries

- `.github/workflows/npm-release.yml` is central reusable workflow for npm consumer repositories, not jt-cli's own package release pipeline.
- `.github/workflows/release.yml` runs release-please for jt-cli itself. Its Release PR owns Cargo version, `CHANGELOG.md`, tag, and GitHub Release.
- `jt repo cicd` generates thin caller workflow in consumer repository.
- Never run local versioning, tagging, GitHub Release creation, or `npm publish`.
- Never request, store, or pass `NPM_TOKEN`. npm publishing uses Trusted Publishing/OIDC.
- Change versions, tags, or release behavior only under explicit release scope.

## Node Init Boundaries

- `jt node init` is interactive, global, and intentionally aggressive.
- First confirmation precedes every mutation. Second confirmation precedes old-environment cleanup.
- Keep cleanup preview and executable action set identical.
- Block destructive cleanup when fresh inventory has diagnostics.
- Require proven registry provenance before migrating globals; unknown PNPM/Bun provenance stays report-only.
- Fresh-read target Vite+ globals after cleanup confirmation. Missing or changed candidates block deletion.
- Stop all provider deletion when any shell cleanup write fails.
- Delete only exact dedicated known nvm/fnm roots. Keep custom roots and unowned launchers report-only.
- Include runtime set in post-confirmation revalidation.
- Preserve path, symlink, concurrent-write, and unknown-content guards.

## Finish

- Confirm intended diff only.
- Report checks run and exact results.
- Report remaining uncommitted files or blocked CI.
