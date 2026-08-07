# Add interactive GitHub repository bootstrap

## Goal

Let `jt repo cicd` recover from a missing Git repository or missing `origin` by offering a safe, minimal questionnaire that creates a public GitHub repository with `gh`, then resumes the existing CI/CD initializer.

## Background

- Current `jt repo cicd` is non-interactive and fails when Git metadata or `origin` is missing.
- Repositories with an existing GitHub `origin` must keep the current zero-prompt path.
- Package type, workspace layout, package manager, registry, and release behavior remain owned by the existing initializer.

## Requirements

- Run local, read-only project validation before offering any external mutation.
- Enter the questionnaire only when `origin` is missing.
- When stdin is not a TTY, fail with an actionable message instead of waiting for input.
- Require `gh` to be installed and authenticated for `github.com`.
- When `gh` is not authenticated, tell the user to run `gh auth login`; do not launch login automatically.
- Ask whether to create a public GitHub repository. Default to `No`.
- Collect the target as `OWNER/REPO`, defaulting to the authenticated user and current directory name.
- Collect an optional description, preferring existing package metadata as the default when available.
- Validate the planned repository identity against the existing publishable package metadata before creating the remote repository.
- Show a final preview and require a second confirmation. Default to `No`.
- Run `git init` only when the current directory is not already a Git worktree.
- Create the repository with `gh repo create OWNER/REPO --public --source=. --remote=origin` and an optional `--description` argument.
- Do not commit, push, add README, add license, add gitignore, or create private/internal repositories.
- Re-read and validate `origin` after `gh` succeeds, then run the existing release initializer unchanged.
- Never overwrite an existing non-GitHub `origin`.
- Never automatically delete a newly created `.git` directory or GitHub repository when a later step fails; report the retained state and failure.
- Preserve current OIDC release boundaries: no local versioning, tagging, registry publishing, or persistent npm/crates.io token.

## Out of Scope

- `gh` installation or `gh auth login` automation.
- Attaching an existing remote repository under a different remote name.
- Repository settings beyond public visibility and optional description.
- Package metadata edits, initial commits, pushes, branch protection, or Trusted Publisher setup.
- Private/internal GitHub repositories and non-GitHub hosts.

## Acceptance Criteria

- [ ] Existing valid GitHub `origin` follows the current non-interactive path without prompts.
- [ ] Missing `origin` in a TTY shows the minimal questionnaire and defaults both mutation confirmations to `No`.
- [ ] Declining either confirmation leaves Git, remotes, GitHub, and generated CI/CD files unchanged.
- [ ] Missing or unauthenticated `gh` produces an actionable error and performs no mutation.
- [ ] Non-TTY execution with missing `origin` fails promptly without reading stdin indefinitely.
- [ ] Confirmed creation initializes local Git only when needed, creates one public GitHub repository, adds `origin`, and does not push.
- [ ] The exact selected `OWNER/REPO` and optional description are passed as separate process arguments, never through a shell.
- [ ] Successful repository creation resumes existing Node/Rust/Turborepo/Cargo workspace initialization.
- [ ] Existing non-GitHub `origin`, unsupported projects, metadata mismatch, and `gh` failures do not overwrite or delete user state.
- [ ] Automated tests cover prompt cancellation, missing authentication, command construction, success continuation, and retained partial state.
- [ ] `cargo fmt --check`, clippy with warnings denied, and `cargo test --locked` pass.

## Notes

- Product flow approved in conversation on 2026-08-07.
