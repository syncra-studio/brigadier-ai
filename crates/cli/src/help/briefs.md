Brief: five lines.
1. Target: the repo area and the outcome.
2. Change: what to build or fix, and decisions made (quote the user's words when they matter).
3. Do not touch: files, areas or behaviour that must stay.
4. Files: the exact files or names it owns. Two workers never share a file; two that need one run in order or get exact names up front.
5. Acceptance: first a demo the user sees (a preview with `--purpose user`, plus `--open /page` when the page isn't the server's root, whose tab opens itself; screenshots or real output, whatever the work is), relayed to the user at once and judged by what ran, not by tests; related tests once per fix only after their go, then the exact commands that must pass; CI runs the full suite, and a full local run only when the user asks or CI cannot cover it; "commit, merge into my branch (below), then say done with the commit ids and each command's last lines".

Add "Ask me (your parent) if something blocks you; don't guess on scope."
Start it with `--title "<its job in 2-4 words>"` ("Add timed marks"): its name in the thread and on the Workers card, never a place ("repo root") or a file.

- The brief carries the rules that bear on its job, verbatim, with their ledger ids (`brigadier ledger show --rules`).
- Its own research notes, check logs and evidence screenshots go in its $TMPDIR, never its record; a file the user asked for goes where they said.
- A plan or brief built from decisions cites their ids. Lines of your own say "proposed", and a gap the decisions don't cover goes to the user, never a silent fill.

A worker is naked: its CLI loads none of your setup but the project's instruction file, MCP servers only by `--mcp` (for heavy use), `--with computer-use` to see an app; it pulls skills with `brigadier skill`, decisions and the rest with `brigadier context`, and calls any MCP server of the setup with `brigadier mcp`. In a Chat it works in `--repo <path>` (a worktree, or the checkout with `--local`), else in its own folder without git.
