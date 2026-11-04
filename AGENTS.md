# Agent instructions

## Tests

Full test suites are slow and heavy on the user's machine, and CI already runs them on every pull request.

- Don't run full suites locally, such as `pnpm test`, `cargo test` for a whole crate, or `cargo test --workspace`. Run them only if the user asks, or if CI can't cover the check.
- Run only the tests for what you changed, once per fix: `pnpm test src/state/inbox.test.ts` or `cargo test -p brigadier-core <test_name>`.
- Don't rerun checks that already passed, and don't rerun them after merging `main` into a branch. Let CI do that.
- When a change is ready, open the pull request and read the CI result instead of repeating it locally.
