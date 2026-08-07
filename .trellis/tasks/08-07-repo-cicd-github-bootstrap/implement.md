# Implementation Plan

1. Read backend filesystem, error, output, and quality guidelines.
2. Refactor project inspection only as needed to run local validation before an origin exists and validate against a planned GitHub slug.
3. Add conditional missing-origin questionnaire using existing `cliclack` dependency and `std::io::IsTerminal`.
4. Add `gh` availability/authentication checks and shell-free repository creation.
5. Verify the created `origin`, then reuse existing generated-file initializer.
6. Add focused unit/integration tests for every mutation boundary and unchanged existing-origin behavior.
7. Update README command behavior and recovery instructions.
8. Run:

   ```bash
   cargo fmt --check
   cargo clippy --locked --all-targets --all-features -- -D warnings
   cargo test --locked
   git diff --check
   ```

9. Run full Trellis check before proposing commits.

## Likely Files

- `src/main.rs`
- `src/release.rs`
- `tests/cli.rs`
- `README.md`

## Review Gates

- No prompt for existing GitHub `origin`.
- No external mutation before final confirmation.
- No `--push` or shell invocation.
- No rollback that deletes external or pre-existing state.
- Existing Node/Rust/Turborepo/Cargo workspace tests remain green.

## Rollback

Revert feature files only. Never delete a GitHub repository or local `.git` created during manual acceptance testing.
