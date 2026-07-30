# jt

Personal Rust CLI. Name joins **J**acob and **T**aotao.

First feature configures npm package release automation:

```bash
jt repo cicd
```

Command operates on current directory. It validates project, then creates only:

```text
.github/workflows/npm-release.yml
```

Generated file calls reusable workflow in this repository. Version changes, `CHANGELOG.md`, Git tags, GitHub Releases, and `npm publish` all run in GitHub Actions. `jt` never runs those release operations locally and never stores `NPM_TOKEN`.

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
```

Pass a directory as the fourth shell argument to override the destination:

```bash
jt icon 64 ./assets
jt icon svg ./assets
```

PNG sizes are `16`, `24`, `32`, `48`, `64`, `128`, `256`, `512`, and `1024`. A PNG selector writes the matching original `jt-<size>.png`; `svg` creates `jt.svg` from its embedded markup. Commands work offline, create the destination directory, and refuse to overwrite an existing file.

## Install

```bash
./install.sh
```

The script builds the current source with `cargo build --release --locked`, installs `jt` to `~/.local/bin`, and verifies the installed binary. Override the directory when needed:

```bash
JT_INSTALL_DIR=/path/to/bin ./install.sh
```

Install released source:

```bash
cargo install --git https://github.com/JacobZyy/jt-cli --tag v1.0.0 --locked
```

## Supported project

- Single npm package hosted on GitHub, default branch `main`
- Valid `package.json` with `name`, SemVer `version`, `repository`, `scripts.test`, and `scripts.build`
- Exactly one `package-lock.json` or `pnpm-lock.yaml`
- pnpm projects declare an exact `packageManager`, such as `pnpm@10.15.0`
- npm registry, with `publishConfig.registry` absent or `https://registry.npmjs.org`
- Publishable package: `private` is absent or `false`

GitLab, workspaces, Yarn, and Bun are rejected explicitly in this first version.

## One-time GitHub and npm setup

1. Run `jt repo cicd`, commit generated workflow on a feature branch, then merge it through a PR.
2. In GitHub Actions settings, grant workflows read/write permission and allow GitHub Actions to create pull requests.
3. In npm package settings, add GitHub Actions Trusted Publisher:
   - organization or user: package repository owner
   - repository: package repository name
   - workflow filename: `npm-release.yml`
   - allowed action: `npm publish`
4. Use Conventional Commits on `main`, for example `feat: add export command`.

npm validates caller workflow in consuming repository, not central reusable workflow. Trusted Publishing also requires `package.json.repository` to match GitHub repository. Workflow uses GitHub-hosted runner, Node.js 24, npm OIDC, and no publish token.

Release flow:

1. Merge a Conventional Commit PR into `main`.
2. Install, test, build.
3. release-please creates or updates Release PR from Conventional Commits.
4. Merge Release PR.
5. Same workflow validates again; release-please creates version, `CHANGELOG.md`, tag, and GitHub Release.
6. `release_created` publishes package to npm through Trusted Publishing.

Validation produces package tarball before release. Only isolated publish job receives OIDC permission; package lifecycle scripts are disabled during publish. If npm publish fails after GitHub Release creation, use **Re-run failed jobs** so publish reuses same release output and tarball.

Generated workflow pins reusable workflow to `@v1.0.0`; consuming repositories never execute mutable `main` with write and OIDC permissions.

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
