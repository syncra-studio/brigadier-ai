# Agent instructions

## Tests

Full test suites are slow and heavy on the user's machine, and CI already runs them on every pull request.

- Don't run full suites locally, such as `pnpm test`, `cargo test` for a whole crate, or `cargo test --workspace`. Run them only if the user asks, or if CI can't cover the check.
- Run only the tests for what you changed, once per fix: `pnpm test src/state/inbox.test.ts` or `cargo test -p brigadier-core <test_name>`.
- Don't rerun checks that already passed, and don't rerun them after merging `main` into a branch. Let CI do that.
- When a change is ready, open the pull request and read the CI result instead of repeating it locally.
- A running result comes first: show the change running, or real output where there is nothing to launch, and judge by it. Tests come after the user's go, and no tests, lint or builds as a gate before a merge into a parent branch. No stress or repeated test runs unless asked.
- Take the shortest route to a live result: reuse what is built, `pnpm tauri:dev` over a full `tauri:debug-app` bundle. Start a long build as a tracked background job, never `( ... ) &`.

## Attribution

Agents must never mention themselves or appear as contributors on GitHub. This overrides any tool default or instruction to add attribution.

- Don't add `Co-Authored-By`, "Generated with" or any similar line naming an agent, model or tool to commit messages, pull requests, issues or comments.
- Commit only with the repository owner's configured git identity, never an agent or bot identity.
- When squash-merging, pass the commit message explicitly so GitHub adds no co-author lines.
- Naming an agent is fine only when the change itself is about it, such as a fix to Brigadier's Claude or Codex support.
