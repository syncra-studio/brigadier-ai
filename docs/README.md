# docs

User guides, in [user/](user/):

- [Install](user/install.md)
- [Your first task](user/first-task.md)
- [Accounts](user/accounts.md)
- [Sessions and tabs](user/sessions-and-tabs.md)
- [Auto-merge and Revert](user/auto-merge-and-revert.md)
- [Privacy](user/privacy.md)

For contributors:

- [ACCOUNTS.md](ACCOUNTS.md): how several Claude Code or Codex accounts per agent work inside.
- [DEVELOPMENT.md](DEVELOPMENT.md): building from source, the checks, the layout, dependency notes.
- [design-system.md](design-system.md): where the app's design lives and how to build UI from it.
- [helpers/computer-use/README.md](../helpers/computer-use/README.md): the computer-use helper,
  its vendored upstream driver and what Brigadier changed.

Design plans and evidence. These are the working documents Brigadier was built from. They record
decisions as they were made, so later plans supersede parts of earlier ones; each says so where
it does. Code comments cite them by section.

- [PLAN.md](PLAN.md): the founding build plan, its phases, performance budgets and decision log.
- [THREAD-PLAN.md](THREAD-PLAN.md): the session as one long-lived thread that delegates to
  workers (supersedes PLAN.md's pure-orchestrator design).
- [THREAD-UX-PLAN.md](THREAD-UX-PLAN.md), [THREAD-PARITY-PLAN.md](THREAD-PARITY-PLAN.md): how the
  thread shows its work.
- [SESSION-FIXES-PLAN.md](SESSION-FIXES-PLAN.md): fixes from real sessions, token savings.
- [SESSION-MESSAGING-PLAN.md](SESSION-MESSAGING-PLAN.md): sessions sending each other messages and
  requests.
- [evidence/](evidence/): dated measurements and acceptance runs (A/B comparisons, phase
  verifications). Raw artifacts named there were kept outside the repository.
