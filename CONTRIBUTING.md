# Contributing to Brigadier

Brigadier is maintained by one person. Every pull request costs review time, so the rules below
are strict and enforced by automation. Read all of this before you open anything. A pull request
that breaks a rule is closed without review, and opening one doesn't oblige anyone to review or
merge it.

## Contents

- [Issues](#issues)
- [Approval before code](#approval-before-code)
- [Pull requests](#pull-requests)
- [Evidence](#evidence)
- [Checks](#checks)
- [AI-assisted work](#ai-assisted-work)
- [What is never accepted](#what-is-never-accepted)
- [Conduct](#conduct)

## Issues

Issues are for reproducible bugs and for proposals. Use the forms; blank issues are off.

- **Bug report.** One bug per issue, reproduced on the latest `main` or the latest release. Give
  the version or commit, your OS, the agent CLIs and their versions, exact steps, what you
  expected, what happened, and the logs. Say what you already ruled out. "It doesn't work"
  issues are closed.
- **Proposal.** A change in behaviour or a new feature. Describe the problem you have, what you
  do about it today and what should happen instead. Don't propose an implementation before the
  problem is agreed.
- **Questions and support** aren't tracked here. Read the [README](README.md) and
  [docs/](docs/README.md) first.
- **Security problems** never go in a public issue: see [SECURITY.md](SECURITY.md).

Search open and closed issues first. Duplicates are closed. Don't comment "+1" or "any update?":
add a reaction, or new facts.

## Approval before code

Every pull request from someone without write access to this repository must close an issue
that the maintainer has **accepted for that contributor**. There are no exceptions for typos,
docs or "small" fixes.

1. Find or open an issue for the change.
2. Comment on it: "I'd like to take this", with a few lines on how you'd approach it. GitHub only
   lets the maintainer assign an issue to someone who has commented on it.
3. Wait. When the maintainer agrees, the issue gets the `accepted` label and is **assigned to
   you**. Both are needed. Agreement in a comment alone isn't approval.
4. Only then open the pull request, with `Fixes #<issue>` in its description.

Approval covers that one issue and that one contributor. It doesn't carry over to other issues,
and it lapses if the label or the assignment is removed.

The **PR gate** checks this whenever a pull request is opened, reopened, edited or pushed to, and
whenever its issue's label or assignment changes. A pull request that doesn't close an open,
`accepted` issue assigned to its author is closed with a comment saying why. Once you have
approval, open a new pull request. The maintainer can exempt a pull request with the
`keep-open` label.

## Pull requests

- **One problem per pull request.** Count causes, not files: two unrelated fixes are two pull
  requests. Larger than about 400 changed lines (not counting lockfiles, generated bindings or
  tests)? The size must have been agreed in the issue first.
- **Match the code around you.** Same naming, comment density and idioms, and reuse what exists.
  No new abstractions, dependencies or configuration unless the issue agreed to them.
- **Commits:** rebased on the latest `main`, no merge commits, each one building on its own.
  Write subjects as plain imperative sentences, like the existing history (`Keep the sidebar
  width when the window narrows`), not `fix: stuff` or `update file.ts`.
- **Fill in the template completely.** A pull request with an empty or boilerplate template is
  closed.
- **UI changes** need before-and-after screenshots or a short recording, in the pull request,
  never committed.
- **Respond to review yourself,** within two weeks. A pull request that sits waiting on its
  author is closed.

## Evidence

Show that the change works. "CI is green" and "tests pass" don't show that.

- **Bug fixes:** reproducible before-and-after evidence. Either an existing test that failed
  before your change and passes after it, or the exact steps you followed and what you observed
  before and after.
- **New tests** are opt-in in this project. Add a committed test only when the issue agreed to
  one.
- **Behaviour changes:** what you ran, on which OS, and what you saw.

## Checks

Run the full checks locally before you ask for review. All must pass:

```sh
tools/full-checks.sh
```

That runs `cargo fmt --all --check`, the TypeScript bindings check, the build, `cargo clippy
--workspace --all-targets -- -D warnings`, the Rust tests (`cargo test --locked --workspace
--lib --bins --tests`), `pnpm typecheck`, `pnpm lint` and `pnpm test`. CI then builds, clippies
and smoke-launches the app on macOS, Windows and Linux. CI doesn't run the Rust tests, so you
must. If your change is gated to one platform, run `tools/full-checks.sh --cross` too. Setup and
details: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## AI-assisted work

Using an AI assistant is fine. Shipping its output unread is not.

- **Disclose it.** The pull request template asks which tools and models you used, and for what.
  Write "None" if you used none. Hiding it is grounds for closing and blocking.
- **You're the author.** You have read every line, you can explain why each one is there, and
  you answer review questions in your own words. Don't paste an agent's reply as your answer.
- **Closed without review:** invented APIs, files or flags; changes nobody asked for in the same
  diff; code that obviously never ran; descriptions that restate the diff at length or claim
  checks that weren't run.

The same applies to issues: an AI-written bug report must still be reproduced by you, on your
machine, and say so.

## What is never accepted

- Features or behaviour changes without an accepted proposal.
- Drive-by refactors, renames, formatting churn, "cleanup", or reorganising files.
- Dependency bumps or new dependencies nobody asked for.
- Rewrites of the README, comments or docs for style, and translations.
- Changes that weaken the sandbox, the permission levels, secret redaction or the cleanup of what
  Brigadier creates, however they're framed.
- Generated or binary files that don't belong in the repository.

## Conduct

Be direct, factual and civil. Personal attacks, harassment, spam and repeated rule-breaking get
you blocked from the repository without further notice.

This project follows the [Code of Conduct](CODE_OF_CONDUCT.md). Report conduct problems to
hello@syncra.studio.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
