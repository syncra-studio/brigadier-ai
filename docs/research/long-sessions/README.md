# Long sessions, usage, speed and grounding: research (2026-10-14)

Research only; no product code changed. Code references are to `d669bca0`.

Read in this order:
1. `PLAN.md`: the prioritised improvement plan for small tasks and multi-hour sessions, each item with its expected
   effect, how to validate it at equal completed-task quality, and its risk. Start with its §0 and §4.
2. `WEAK-SPOTS.md`: Brigadier's weak spots with file:line evidence, from an independent code audit and from checks
   made for this research.
3. `DIGEST.md`: the evidence, graded. Vendor docs and CLI capabilities on the installed versions, the research
   literature, and practitioners. §5 says whether any item §7 rejected has new evidence.
4. `BASELINE.md`: what Brigadier does today, which later plans supersede `docs/PLAN.md` §7, and coverage of §7's
   "Next" items.
5. `PROBES.md`: the CLI probes run for this research (cache lifetime of `-p` sessions; cache across effort
   changes on Claude and Codex).
6. `sources/`: the full source notes behind the digest (`vendor.md`, `papers.md`, `practice.md`).
