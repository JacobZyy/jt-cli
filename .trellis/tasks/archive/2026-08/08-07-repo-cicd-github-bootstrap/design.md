# Design: Interactive GitHub Repository Bootstrap

## Boundary

Keep `jt repo cicd` conditionally interactive:

- Existing GitHub `origin`: call current initializer directly.
- Missing `origin`: run repository bootstrap questionnaire, verify the created remote, then call current initializer.
- Existing non-GitHub `origin`: preserve current failure behavior.

The bootstrap owns Git/GitHub repository creation only. Existing release inspection and generated-file logic continue to own Node, Rust, workspace, registry, repository metadata, and file safety.

## Flow

1. Inspect current directory and locally validate supported publishable packages without mutation.
2. Inspect Git worktree state and `origin`.
3. If `origin` is valid GitHub, continue without prompts.
4. If `origin` is missing, require a TTY, `gh`, and authenticated `github.com` session.
5. Ask for creation consent, `OWNER/REPO`, optional description, and final confirmation.
6. Validate planned GitHub slug against package repository metadata before external mutation.
7. Run `git init` only when needed.
8. Run `gh repo create` with discrete arguments, public visibility, `--source=.`, and `--remote=origin`; omit `--push`.
9. Re-read `origin`; require the expected GitHub slug.
10. Resume existing release initialization.

## Safety

- Both confirmations default to `No`.
- Non-TTY execution never prompts.
- No shell command construction.
- No existing remote overwrite.
- No automatic deletion of local Git state or remote repositories after partial success.
- Local validation runs before GitHub creation to avoid orphan repositories for unsupported projects.
- Cancellation and preflight failures produce zero mutation.

## Compatibility

- Keep `jt repo cicd` command and existing success output.
- Reuse installed `cliclack` interaction style.
- Keep CI/CD generated file names and OIDC workflow unchanged.
- No new dependency or CLI flag.

## Failure States

- `gh` missing: show installation requirement.
- `gh` unauthenticated: show `gh auth login` instruction.
- `git init` failure: stop before GitHub creation.
- `gh repo create` failure: retain any newly initialized `.git`; report failure.
- Origin verification failure after creation: retain repository and remote state; do not run generated-file initialization.
- Generated-file initialization failure after remote creation: retain repository/origin and report exact failure.

## Testing Shape

Separate decision logic and process execution enough to inject prompt answers and fake `gh`/Git behavior. Keep abstraction local to the release feature; do not generalize all CLI prompting unless reuse materially reduces code.
