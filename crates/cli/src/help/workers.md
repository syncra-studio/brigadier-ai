Which worker:
- Give each part to the worker that owns its files or knows its area; track who owns what (`brigadier workers --json`).
- A follow-up from the user on a worker's area goes to that worker with `brigadier send`, not to a new one.
- A message the user sends mid-work comes as `[queued message <id> …]`; judge it soon with `brigadier queue <id>`: `join` when it belongs to this work, `worker <id>` when it belongs to a worker's, `wait` when it doesn't belong, even a quick question (it comes again when this work is done, and is judged again then), or `new-session` if it could run apart; one that comes while only background work is in progress you answer now (`join`; unjudged, it joins). Nothing else is judged for you: unjudged, it stays queued and you are asked once more. One `steered in by the user` is this work's: do it, and if it doesn't belong to this work, start a worker for it.
- A finished, unclosed worker takes related next work by `brigadier send`, cheaper while its context is useful, not near full; else start fresh. Close one once no related work is expected; a closed one can't resume (a successor continues from its branch).

One short job: four calls. Build or test work, one shell call each (a small edit you make yourself); `brigadier help commands` has every flag, so skip `--help`.
1. Look at the files, with `brigadier models` (and `brigadier ledger show --rules` if the user gave rules) in one call.
2. `brigadier start "<brief>" --work "<Work>" … && brigadier wait`: a bare wait in the foreground (your only worker).
3. On turn ended, done: check its evidence (`git log`, the output it reported). Other events: Waves.
4. Your answer. Close it unless related work is coming.

Waves:
1. Start the wave: up to N workers on own files.
2. Wait right after starting; never end a turn with workers running and no wait. One short job (minutes) and nothing else to do: run `brigadier wait <id>` in the foreground; it returns with the event. Longer: run `brigadier wait` as a background command and end your turn; it wakes you. Handle each event; wait again until all in:
   - turn ended: done: check its evidence. A question: answer it yourself when the plan, the user's words, the code or a web search settle it; otherwise ask the user once and relay with `brigadier send`.
   - needs input (a permission prompt; only you see it, the worker waits): judge it yourself, then `brigadier send <id> --allow` or `--deny <reason>` (`brigadier help approvals`).
   - usage limit: let it wait for the reset, or close it and restart the rest on the same Work's next row.
   - fresh context (compacted): check it still knows the brief, or resend.
   - failed or interrupted: `brigadier read <id> --last 10`; send it on or close it.
   - gone: read its branch; start a successor with "continue from branch X; `git log` first".
   - quiet (20 min silent): read its last 10; nudge or close it.
3. Next wave when this one is integrated.

A hand-off gives each worker's id, branch, state, and what you wait on.
