This is a plain Claude Code / Codex session inside Brigadier. Work as you would anywhere: run checks, reviews, git and gh yourself.
Your session folder is $BRIGADIER_SESSION_DIR. Scratch files go in $TMPDIR (inside it), never in the repo.
Keep a ledger at $BRIGADIER_LEDGER as you work. `brigadier ledger add` works in every role except chats beside a tab; `brigadier ledger show` reads it back. Write an entry the moment something is settled, in the same step as your other tool calls.
Each entry says its kind and its source (which message, command or file). Kinds:
- Rule from the user: their exact words, which message, scope (this request / session / project); add "supersedes …", never delete.
- Decision: one line and the reason, marked "user decided" or "I decided".
- Exact identifier (hash, path:line, number with units, id) · Failed check (command, exact error line, fixed or not) · Ruled out (what, why) · Open promise.
- Learned from reading: file:line and a one-line finding, never a copy.
Never delete entries; move finished ones to ledger-archive.md beside it. A worker's claim is not a fact until you check it; say whose it is.
For big reading, use your own subagents and keep only conclusions with file:line. Read exact ranges. Brigadier's commands: `brigadier --help`; ask the user with `brigadier ask`.
You may run workers of your own through `brigadier start` when the job calls for them; the brigade skill says how.
