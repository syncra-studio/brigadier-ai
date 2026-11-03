All through `brigadier` (`--help`, `--json` on all):

| Command | Use |
|---|---|
| `brigadier models` | Workers at once, routing rows and rules, models, quota left. |
| `brigadier start "<brief>" --title "2-4 words" [--work KIND] [--agent AGENT] [--model M] [--effort E] [--base B] [--local] [--repo PATH] [--permission P] [--mcp NAME]… [--with computer-use] [--reviewing ID\|session] [--mockup]` | Start a worker in its own worktree off yours (or `--base`), at your permission level; prints its id. |
| `brigadier wait [<id>…] [--timeout S]` | Wait for a worker's next event. |
| `brigadier read <id> [--last N]` | Its last message or N items. |
| `brigadier send <id> "<text>"` / `--allow` / `--deny <reason>` | Message a worker (queued mid-turn), or answer its permission prompt. |
| `brigadier workers` / `brigadier close <id>` | List yours; close one. |
| `brigadier pause <id> ["<note>"]` / `brigadier resume <id> ["<note>"]` | End its turn now, holding what you send; go on where it stopped. |
| `brigadier switch <id> [--agent A] [--model M] [--effort E]` | Move it to the next account or routing row, or the one named. |
| `brigadier sessions [--all]` | Other sessions; message one if asked. |
| `brigadier fusion offer "<question>"` | In a top-level Chat only, offer a second opinion from every installed CLI; nothing starts until the user clicks Ask. |
| `brigadier session propose --title "2-4 words" "<self-contained brief>"` | In a project session, work apart from it, offered as a new session on a card: a call, never a promise. |
| `brigadier ask --question Q [--option O]… [--question …]` | One card: no question tool, or a worker's question (in the background). |
| `brigadier preview start --purpose user [--name N] [--workdir D] [--env K=V] <command>`, `log [--wait S]`, `stop` | Run an app the user sees; `log` says if up. |
