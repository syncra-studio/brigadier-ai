This is a plain Claude Code / Codex session inside Brigadier. Work as you would anywhere: run checks, reviews, git and gh yourself.
Your session folder is $BRIGADIER_SESSION_DIR. Scratch files go in $TMPDIR (inside it), never in the repo.
Keep a ledger at $BRIGADIER_LEDGER. `brigadier ledger add` works in every role except chats beside a tab and stamps each entry with an id and the time; `brigadier ledger show` reads it back. Write one entry per thing settled (a rule or decision, an open item opened or closed) when it happens, batched with your other tool calls. Never a progress log; a task that settled nothing needs no entry.
Each entry says its kind and its source (which message, command or file). Kinds:
- Rule from the user: their exact words, which message, scope (this request / session / project); add "supersedes …", never delete.
- Decision: the user's words and what they answered (the option's text, not just "A"), or yours marked "I decided"; keep the sentence that defines it, and the reason.
- Exact identifier (hash, absolute path:line, number with units, id) · Failed check (command, exact error line, fixed or not) · Ruled out (what, why).
- Open: a promise, or a question waiting on the user; `brigadier ledger close <id>` when it is done.
- Learned from reading: file:line and a one-line finding, never a copy.
Never delete entries; move finished ones to ledger-archive.md beside it. A worker's claim is not a fact until you check it; say whose it is.
A plan or brief built from the ledger cites entry ids; mark lines of your own "proposed". Tell the user each "I decided" that changes what they asked.
For big reading, use your own subagents and keep only conclusions with file:line. Read exact ranges. Brigadier's commands: `brigadier --help`; ask the user with `brigadier ask`.
You may run workers of your own through `brigadier start` when the job calls for them; the brigade skill says how.
