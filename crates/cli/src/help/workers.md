Which worker:
- Give each part to the worker that owns its files or knows its area; track who owns what (`brigadier workers --json`).
- A follow-up from the user on a worker's area goes to that worker with `brigadier send`, not to a new one.
- A message `steered in by the user` mid-work is this work's: do it, and if it doesn't belong to this work, start a worker for it.
- One going the wrong way: `brigadier pause <id>` ends its turn now and holds what you `send` it; `brigadier resume <id> "<note>"` gives it that and the note, then "Continue where you stopped.". `brigadier close <id>` stops one for good; `brigadier read <id>` still reads it. Each reply carries the worker's state and progress: nothing to check after it.
- Several workers: the user's "the second one" is the second of those at work now, in the order you started them, and "the tests one" the one with that job (titles and numbers are in `brigadier state`): act on it, no question.
- A finished, unclosed worker takes related next work by `brigadier send`, cheaper while its context is useful, not near full; else start fresh. Close one once no related work is expected; a closed one can't resume (a successor continues from its branch).

One short job: four calls. Build or test work, one shell call each (a small edit you make yourself); this topic has briefs and routing too; `brigadier start "<brief>" --work "<Work>" --title "<2-4 words>"` is all a start needs, so skip `--help` and `help commands`.
1. Look at the files; `brigadier state` already gave the routing and the rules.
2. `brigadier start "<brief>" --work "<Work>" … && brigadier wait`: a bare wait in the foreground (your only worker).
3. On turn ended, done: check its evidence (`git log`, the output it reported). Other events: Waves.
4. Your answer. Close it unless related work is coming.

Waves:
1. Start the wave: up to N workers on own files.
2. Each event reaches you by itself once your turn ends: end your turn. One short job (minutes) and nothing else to do: `brigadier wait <id>` in the foreground returns with it. Handle each event until all in:
   - turn ended: done: check its evidence; separate work it turned up goes to the user as a session offer (`brigadier session propose`), never a question in text. A question: answer it yourself when the plan, the user's words, the code or a web search settle it; otherwise ask the user once and relay with `brigadier send`.
   - needs input (a permission prompt; only you see it, the worker waits): judge it yourself, then `brigadier send <id> --allow` or `--deny <reason>` (`brigadier help approvals`).
   - usage limit: let it wait for the reset, or move it now with `brigadier switch <id>` (the next account or routing row; `--agent`, `--model`, `--effort` name one). Also for one stuck on its model.
   - failed or interrupted: `brigadier read <id> --last 10`; send it on or close it.
   - gone: read its branch; start a successor with "continue from branch X; `git log` first".
   - quiet (20 min silent, in a turn): read its last 10; nudge or close it.
   - stalled (a background command, no new output and almost no CPU for 10 min; told once per stall): say so in the thread, read its last 10, and have the worker look at the command. Nothing was stopped. A deliberate wait like `sleep 900` reads the same; a command doing real work never does.
   - starved (a compile 10 min behind the build slots, or far under one core on an overloaded machine; told once): not hung. Say so in the thread; pause or finish your other builds and stress runs if they are the load, else tell the user what holds it.
   - any event where Brigadier, its CLI or the worker acts abnormally (a crash or error that isn't the project's, stuck, looping, repeating, ending wrong): handle it as above, and offer the user feedback once (`brigadier feedback offer`).
3. Next wave when this one is integrated.
