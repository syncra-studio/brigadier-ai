Pull what you need; nothing is put in for you.
- `brigadier context`: your brief, decisions, open questions, ledger, transcript, skills, MCP servers, project and routing. Run it when you start and after any compaction.
- `brigadier context skills <name>` prints a skill whole; a worker's CLI loads none, so it pulls the one it needs here.
- `brigadier context mcps` lists the setup's MCP servers; `brigadier mcp tools <server>` and `brigadier mcp call <server> <tool> '<json>'` reach any of them, loaded at launch or not. `brigadier start --mcp NAME` loads one into a worker's CLI for heavy use.
- A worker reads its session's thread with `brigadier context transcript --session`.
- `brigadier ledger show --rules` and `--open`: the user's rules and the open items.
- `brigadier models`: workers at once, routing rows and rules, models, quota left.
