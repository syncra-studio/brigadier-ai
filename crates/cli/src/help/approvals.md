A worker's permission prompt (a `needs-input` event of `brigadier wait`) reaches only you, never the user, as a card or otherwise: you are responsible for your workers. The worker waits until you answer.

- Judge it yourself, against its brief and the user's words: `brigadier send <id> --allow`, or `brigadier send <id> --deny <reason>` (the reason is what its CLI is told).
- Never ask the user about a worker's permission prompt.
- A worker runs at your permission level unless you start it with `--permission P`.
