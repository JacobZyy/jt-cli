# jt

Personal Rust CLI. Name joins **J**acob and **T**aotao.

Bootstrap terminal tooling:

```bash
jt cli bootstrap
```

Interactive bootstrap supports macOS, Debian/Ubuntu, and WSL on x86_64 and ARM64. It installs:

- Fish or Zsh and Starship with Catppuccin Mocha.
- bat, eza, fd, ripgrep, fzf, btop, zoxide, jq, tldr, git-delta, and lazygit.
- Fish Git abbreviations/functions plus `proxy-on` and `proxy-off`.
- Zellij when selected.

The command asks before mutation, changes the default shell, writes only jt-managed shell files, backs up replaced files, and configures git-delta globally. Fish users can explicitly enable `proxy-on` at shell startup; this option defaults to No and validates GitHub through `http://127.0.0.1:7890` before exporting proxy variables. Ghostty, fonts, Node.js, fnm, and pnpm stay outside this bootstrap.

Bootstrap automatically loads `jt` completions in interactive Fish and Zsh shells when `jt` is on `PATH`. Load them manually when needed:

```fish
jt completions fish | source
```

```zsh
source <(jt completions zsh)
```

Install Ghostty on macOS:

```bash
jt ghostty install
```

This separate interactive command installs Ghostty and Maple Mono NF CN through Homebrew, then writes a jt-managed Ghostty configuration. It backs up replaced files and asks before mutation. Linux and WSL servers are rejected without installation.

Write project Zed settings from the live repository template:

```bash
jt zed-conf
```

The command finds the current Git repository root and writes `.zed/settings.json`. Existing
different content is backed up beside the file before an atomic update. The template is fetched
over HTTPS from [`templates/zed/settings.json`](templates/zed/settings.json) on `main`, so merging a
template-only change updates future command runs without releasing a new `jt` version.

Configure project-local AI-edit hooks:

```bash
jt ai-hook
```

The questionnaire selects checks (`Vitest`, `ESLint`) and agent terminals (`Codex`). Re-running shows
the installed selection; deselecting a check removes its maintained runner. Automation can provide
the final selection without a TTY:

```bash
jt ai-hook --checks vitest,eslint --agents codex
```

Run inside a Git repository whose root `package.json` directly declares `tsx` and each selected
tool. The command installs nothing. Workspace-package-only tooling is unsupported. `jt vitest` is a
reserved placeholder; old `jt vitest ai-hook` arguments are removed.

Installation writes maintained TypeScript runtime under `.codex/hooks/jt-ai-hook/`, then merges one
handler into each `.codex/hooks.json` stage. `PreToolUse` fingerprints candidate patches.
`PostToolUse` records content changes. `Stop` discovers direct `.ts` plugins under
`stop/runner/`, sorts them, and executes all concurrently. Built-in `vitest.ts` and `eslint.ts`
runners can be attached independently; custom runner files remain untouched. Existing unrelated
handlers remain, including handlers sharing a group with migrated entries. Old `jt-vitest` and
`nlab-eslint` handlers are replaced, preventing duplicate execution. Known legacy files bearing
their ownership markers are removed; unmarked, custom, and symlinked files remain. Empty legacy
directories are removed. Re-running is idempotent.
Review and trust project hooks with `/hooks`. Runtime does not call `jt`.

The Stop entry launches each tool through asynchronous child processes with `shell: false`. Both
runners receive `isInAIHook=true` and `NO_COLOR=1`; process, environment, exit state, Vite server,
and logger state stay isolated. One slow runner does not delay starting another.

Vitest runs `related` once over all AI-edited files with its native `agent` reporter. Coverage is
limited to edited files matching resolved project `coverage.include` and not matching
`coverage.exclude`; no match disables coverage. Provider, thresholds, and `skipFull` stay project
owned. A temporary `coverage-summary.json` is parsed into one Markdown table, then deleted. Raw
coverage output never reaches the model. ESLint checks only supported existing edited files and
returns at most 50 error diagnostics.

All runners finish before one combined result is produced. Success stays model-silent. Failures use
stable runner order and combine ESLint diagnostics with Vitest test/coverage sections. Coverage-only
failure returns the table and concise threshold conclusions. First failure blocks for repair; retry
continues once to prevent a Stop loop. Bounded details remain in `/tmp/jt-ai-hook-<repo>.jsonl`.

Inspect those local traces in the web console:

```bash
pnpm install
pnpm --filter ai-hook-console dev
```

Open `http://localhost:3000`. The console lists sessions with status, trigger counts, context,
and code paths. Each detail page renders hook messages as Markdown, shows the session transcript,
and reconstructs `apply_patch` diffs from Codex session JSONL. It reads `/tmp/jt-ai-hook-*.jsonl`,
`~/.codex/sessions`, and `~/.codex/archived_sessions` without modifying them. Set
`AI_HOOK_LOG_DIR` or `CODEX_HOME` to use different local directories.

On macOS, install the production server as a login service:

```bash
pnpm --filter ai-hook-console service:install
```

The LaunchAgent listens only on `127.0.0.1:3100`, starts at login, and restarts after failure.
After changing console code, rebuild and restart it with one command:

```bash
pnpm --filter ai-hook-console service:restart
```

Use `service:status` to inspect it or `service:uninstall` to remove the LaunchAgent. Runtime logs
are stored in `~/Library/Logs/jt-ai-hook-console.log` and
`~/Library/Logs/jt-ai-hook-console.error.log`.

The repository remains a Rust crate at the root. pnpm and Turborepo manage the web workspaces:

```text
apps/ai-hook-console      Next.js App Router console
packages/ai-hook-core     AI-hook and Codex JSONL reader
packages/ui               shared shadcn/ui components and theme
```

Session data can contain private code and conversation text. Keep this console bound to a trusted
local environment unless authentication is added.

Initialize one frontend project from its real build, TypeScript, request, response-envelope, output,
and backend Facade layout:

```bash
jt nlab-api init \
  --project /path/to/frontend \
  --repo-path /path/to/backend \
  --branch feature-branch \
  --app-name service_name

jt nlab-api generate --project /path/to/frontend
```

`init` writes `.nlab/nlab-api.config.json`, then adds idempotent build-tool and TypeScript aliases. Current
support targets Vite projects with an exported `nlabRequest` adapter. Vitest configs that merge the
Vite config inherit those aliases without a duplicate edit. Existing `src/api` or `src/service`
layout selects the matching preset; `--layout` overrides it.

`generate` loads that config, runs `codegraph init` or `codegraph sync` once, reads the resulting
SQLite index in read-only mode, parses Java with Tree-sitter, and builds one deterministic contract
IR. Rust generates Draft OpenAPI 3.1, TypeScript DTO files, separate enum files, and API clients that
reuse the detected request adapter. The same command then queries testserver ZGateway on a best-effort
basis, migrates business imports from the fixed previous `.nlab` snapshot, optionally generates Mock
files when `mock.enabled` is true, promotes the stable OpenAPI snapshot, and writes one final report.
It does not invoke Bun, Node.js, Orval, Python, frontend typecheck, tests, builds, or lint. A DTO field
uses a complete code-provenance enum first, then a complete linked Java enum, then explicit comment or
annotation values; otherwise it remains its original scalar type.

Generated output includes:

```text
src/service/**/*.ts
src/types/service-type/**/*.ts
src/types/service-enums/**/*.ts
.nlab/nlab-api.config.json
.nlab/contract-ir.json
.nlab/openapi.json
.nlab/frontend-manifest.json
.nlab/replacement-map.json
.nlab/generate-report.json
```

The backend must remain on the configured branch, but generation reads its current working-tree Java
and POM content. The frontend must remain outside the backend repository. Existing non-generated files,
symlink path escapes, config drift, incomplete CodeGraph state, branch movement, ambiguous Facade
overloads, and incomplete schema references stop the run. A legacy bridge manifest marked
`service-paths` permits its owned generated files to be replaced during the first Facade-layout
generation. Missing enums, external value sources, Gateway errors, missing routes, removed unused
operations, and Mock being disabled do not stop generation; they remain in
`.nlab/generate-report.json`. stdout contains one final JSON result. stderr contains only stage and
percentage progress events outside a TTY; a TTY shows one progress bar. The overall deadline defaults
to 1200 seconds.

Configure Node.js and Rust package release automation:

```bash
jt repo cicd
```

Command operates on current project root. It discovers and validates publishable Node.js packages
and Cargo workspace members before making changes. With an existing GitHub `origin`, behavior stays
non-interactive. When Git or `origin` is missing, an interactive terminal can create one public
GitHub repository through an installed, authenticated GitHub CLI (`gh`). Run `gh auth login` first
when needed.

Repository creation asks twice, defaults both confirmations to **No**, and validates selected
`OWNER/REPO` against package metadata. It runs `git init` only after final confirmation and only
when needed, then calls `gh repo create` without committing or pushing. A failed creation or later
initialization keeps any local Git repository, GitHub repository, and `origin` already created so
re-running command can continue safely.

After origin validation, command creates:

```text
.github/workflows/npm-release.yml
release-please-config.json
.release-please-manifest.json
```

The generated caller invokes the versioned reusable workflow in this repository. release-please
owns versions, changelogs, Git tags, and GitHub Releases. npm packages publish through npm Trusted
Publishing; Rust crates publish to crates.io through Trusted Publishing. `jt` never versions,
tags, or publishes locally and never stores npm or crates.io tokens.

Existing `release-please-config.json` and `.release-please-manifest.json` files are preserved so a
repository can own custom release configuration. An existing different caller workflow is never
overwritten.

Initialize global Node/pnpm environment through Vite+:

```bash
jt node init
```

This interactive command supports macOS/Linux on x86_64 and ARM64. macOS Apple Silicon needs Rosetta 2 for the Node 14 x64 runtime. It:

1. Installs or reuses Vite+, then prepares Node `14.21.3`, `16.19.0`, `20.11.0`, `22.21.0`, and `24.15.0`.
2. Installs default pnpm, prewarms pnpm `7.4.1`, `9.7.0`, and `10.12.4`, then installs nrm.
3. Configures npm mirror, nrm `taobao`/`zz`, and Vite+ shell loaders.
4. Migrates globals whose npm inventory proves an npmjs/npmmirror registry source to Vite+.
5. Freshly scans old nvm/fnm/Homebrew/pnpm/Bun state. It shows every cleanup action and asks again before deleting anything.

Both confirmations default to **No**. Canceling first confirmation makes no changes. Incomplete inventory blocks cleanup. Existing Vite+ installation and additive changes remain when a later stage fails or cleanup is declined.

PNPM/Bun package manifests do not prove install provenance. `jt` reports those globals as unreconstructable and retains PNPM_HOME/Bun targets instead of guessing a registry package or Bun global directory. After cleanup confirmation, `jt` fresh-checks both legacy inventory and every target Vite+ global before deleting old providers.

Recursive deletion is limited to exact known dedicated nvm/fnm roots. Custom manager roots and unverified `fnm` wrappers are reported and retained. Shell cleanup follows actual removable providers; a failed shell write stops all later provider deletion.

Write one JT icon into `./public`:

```bash
jt icon 64
jt icon svg
jt icon animated
```

Pass a directory as the fourth shell argument to override the destination:

```bash
jt icon 64 ./assets
jt icon svg ./assets
jt icon animated ./assets
```

PNG sizes are `16`, `24`, `32`, `48`, `64`, `128`, `256`, `512`, and `1024`. A PNG selector writes the matching original `jt-<size>.png`; `svg` creates `jt.svg`; `animated` creates `jt-animated.svg`. Commands use embedded sources, work offline, create the destination directory, and refuse to overwrite an existing file.

## Install

```bash
./install.sh
```

The script builds the current source with `cargo build --release --locked`, installs `jt` to `~/.local/bin`, and verifies the installed binary. Override the directory when needed:

```bash
JT_INSTALL_DIR=/path/to/bin ./install.sh
```

Install a published release, replacing `vX.Y.Z` with a tag from [GitHub Releases](https://github.com/JacobZyy/jt-cli/releases):

```bash
cargo install --git https://github.com/JacobZyy/jt-cli --tag vX.Y.Z --locked --root "$HOME/.local"
```

Upgrade an existing user-owned installation:

```bash
jt upgrade --check
jt upgrade
jt upgrade --dry-run --force
```

`jt upgrade` resolves an exact published GitHub Release and pins its Git commit, asks Cargo to build it in a temporary directory, verifies the staged binary, then atomically replaces `~/.local/bin/jt`. A failed post-install version check restores the previous binary. `--force` reinstalls the current version. The command never runs a shell or `sudo`. Other paths and package-manager shims must use their original installer or manager. Cargo and Git remain required; prebuilt release assets and persistent rollback are not implemented yet.

## Supported repositories

- GitHub repository containing Node.js packages, Rust crates, or both
- Standalone package, Turborepo, npm/pnpm workspace, or Cargo workspace
- Turborepo validation runs `turbo run test build`; Cargo workspace validation also runs directly
- npm or pnpm; pnpm projects declare an exact `packageManager`, such as `pnpm@10.15.0`
- Lockfile optional for npm; an explicit `packageManager` resolves repositories with stale or
  multiple lockfiles
- Publishable Node.js packages use the npm registry, with `publishConfig.registry` absent or
  `https://registry.npmjs.org`
- Publishable Rust packages use crates.io, with Cargo `publish` absent or including only
  `crates-io`
- Every publishable package declares same GitHub repository, matching existing `origin` or selected
  `OWNER/REPO`

Private Turborepo roots, private Node.js workspace packages, and Cargo packages with
`publish = false` are allowed and skipped. Yarn, Bun, Deno, GitLab, non-npm Node registries, other
Cargo registries, and ecosystems other than Node.js/Rust report an explicit unsupported error.

Turborepo is the monorepo build system supported here. Turbopack is a Next.js bundler, not a
monorepo manager.

## One-time GitHub, npm, and crates.io setup

1. Run `jt repo cicd`, commit generated workflow on a feature branch, then merge it through a PR.
2. In GitHub Actions settings, grant workflows read/write permission and allow GitHub Actions to
   create pull requests.
3. For every npm package, add a GitHub Actions Trusted Publisher in npm package settings:
   - organization or user: package repository owner
   - repository: package repository name
   - workflow filename: `npm-release.yml`
   - allowed action: `npm publish`
4. Publish every new crate manually once, then configure its crates.io Trusted Publisher for the
   same repository and `.github/workflows/npm-release.yml`. crates.io cannot use Trusted Publishing
   for a crate's first release.
5. Use Conventional Commits on the repository default branch, for example
   `feat: add export command`.

npm and crates.io authenticate through OIDC on GitHub-hosted runners. npm validates the caller
workflow in the consuming repository, not the central reusable workflow. The workflow uses Node.js
24 and no persistent publish token.

Release flow:

1. Merge a Conventional Commit PR into the default branch.
2. Install, test, and build every detected ecosystem. Turborepo orchestrates Node.js workspace
   tasks; Cargo validates the Rust workspace.
3. release-please creates or updates one manifest Release PR for all changed packages.
4. Merge Release PR.
5. Same workflow validates again; release-please creates versions, `CHANGELOG.md` files, tags, and
   GitHub Releases.
6. Released npm packages publish to npm. Released Cargo workspace packages publish to crates.io in
   dependency order through release-plz.

Validation produces npm tarballs before release. Only isolated publish jobs receive OIDC
permission; npm package lifecycle scripts are disabled during packing and publishing. If registry
publishing fails after GitHub Release creation, use **Re-run failed jobs**.

Generated workflow pins reusable workflow to `@v1.4.0`; consuming repositories never execute
mutable `main` with write and OIDC permissions.

## Releasing jt

Push features through a Conventional Commit PR. `.github/workflows/release.yml` runs release-please on `main`; its Release PR owns `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, the Git tag, and the GitHub Release. Never version or tag jt locally.

## Development

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

## Git hook

Activate repository pre-commit hook once per clone:

```bash
ln -s ../../.githooks/pre-commit .git/hooks/pre-commit
```

Every commit runs `cargo test --locked --quiet`. Failed tests abort commit. GitHub CI remains final required check because local hooks can be bypassed.

## Commit policy

Use Conventional Commits because release-please derives versions and changelog entries from commits on `main`. Git does not activate repository hooks automatically, and `--no-verify` can bypass them. For squash merges, enforce Conventional Commit PR titles with GitHub rules or a server-side PR-title check.

release-please currently uses built-in `GITHUB_TOKEN`. GitHub suppresses workflows triggered by its Release PR. Repositories requiring PR checks need a bot bypass until optional GitHub App token support is added.

## License

MIT
