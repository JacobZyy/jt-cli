# jt

Personal Rust CLI. Name joins **J**acob and **T**aotao.

First feature configures npm package release automation:

```bash
jt release init
```

Command operates on current directory. It validates project, then creates only:

```text
.github/workflows/npm-release.yml
```

Generated file calls reusable workflow in this repository. Version changes, `CHANGELOG.md`, Git tags, GitHub Releases, and `npm publish` all run in GitHub Actions. `jt` never runs those release operations locally and never stores `NPM_TOKEN`.

## Install

```bash
cargo install --git https://github.com/JacobZyy/jt-cli
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

1. Run `jt release init`, commit generated workflow, push it to `main`.
2. In GitHub Actions settings, grant workflows read/write permission and allow GitHub Actions to create pull requests.
3. In npm package settings, add GitHub Actions Trusted Publisher:
   - organization or user: package repository owner
   - repository: package repository name
   - workflow filename: `npm-release.yml`
   - allowed action: `npm publish`
4. Use Conventional Commits on `main`, for example `feat: add export command`.

npm validates caller workflow in consuming repository, not central reusable workflow. Trusted Publishing also requires `package.json.repository` to match GitHub repository. Workflow uses GitHub-hosted runner, Node.js 24, npm OIDC, and no publish token.

Release flow:

1. Push to `main`.
2. Install, test, build.
3. release-please creates or updates Release PR from Conventional Commits.
4. Merge Release PR.
5. Same workflow validates again; release-please creates version, `CHANGELOG.md`, tag, and GitHub Release.
6. `release_created` publishes package to npm through Trusted Publishing.

Validation produces package tarball before release. Only isolated publish job receives OIDC permission; package lifecycle scripts are disabled during publish. If npm publish fails after GitHub Release creation, use **Re-run failed jobs** so publish reuses same release output and tarball.

Generated workflow pins reusable workflow to `@v0.1.0`; consuming repositories never execute mutable `main` with write and OIDC permissions.

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
