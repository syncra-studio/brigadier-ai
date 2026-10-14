# docs

User guides:

- [COMPUTER-USE.md](COMPUTER-USE.md): computer use on macOS: turning it on, what workers can and
  can't do, safety.
- [ACCOUNTS.md](ACCOUNTS.md): several Claude Code or Codex accounts per agent.

For contributors:

- [DEVELOPMENT.md](DEVELOPMENT.md): building from source, the checks, the layout, dependency notes.

Design plans and evidence. These are the working documents Brigadier was built from. They record
decisions as they were made, so later plans supersede parts of earlier ones; each says so where
it does. Code comments cite them by section.

- [PLAN.md](PLAN.md): the founding build plan, its phases, performance budgets and decision log.
- [THREAD-PLAN.md](THREAD-PLAN.md): the session as one long-lived thread that delegates to
  workers (supersedes PLAN.md's pure-orchestrator design).
- [THREAD-UX-PLAN.md](THREAD-UX-PLAN.md), [THREAD-PARITY-PLAN.md](THREAD-PARITY-PLAN.md): how the
  thread shows its work.
- [SESSION-FIXES-PLAN.md](SESSION-FIXES-PLAN.md): fixes from real sessions, token savings.
- [COMPUTER-USE-PLAN.md](COMPUTER-USE-PLAN.md): computer use's design, benchmark and measurements.
- [research/](research/): background research.
- [evidence/](evidence/): dated measurements and acceptance runs (A/B comparisons, phase
  verifications). Raw artifacts named there were kept outside the repository.
