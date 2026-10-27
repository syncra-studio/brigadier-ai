---
name: fusion
description: Only when the user starts Fusion in a top-level Chat, consult every installed CLI as workers, run one cross round, then answer with one merged recommendation.
---

# Fusion

Use it only when the user starts it in a top-level Chat: a `/fusion <question>` message, the Fusion button, or a click on its offer card. Never on your own. Sessions, side chats, tab chats and workers never run or offer Fusion. When a second opinion would help in a top-level Chat, offer it with `brigadier fusion offer "<question>"` and carry on; the offer starts nothing until the user clicks Ask.

## Consult

1. Gather the context yourself first: the files that bear on the question (file:line), and the ledger's rules and decisions that touch it (`brigadier ledger show --rules`, `brigadier ledger show --open`).
2. Run `brigadier models`. Start one consult per installed CLI with quota left, each through its own row: `fusion-claude`, `fusion-codex` (another CLI: `fusion-<agent>`). The row (the user's routing, else the shipped one) picks agent, model and effort. If its account is out of quota, take the same Work's next row the rules allow; with none, skip that CLI and say so.
3. Start them with the same brief, together while "Workers at once" (in `brigadier models`) allows, else one after another:
   `brigadier start "<brief>" --work fusion-claude --title "Fusion · <model>"`
   Run each `brigadier` command as it is, with nothing piped after it (`| tail`): at Accept edits Claude Code asks before such a command.
   The brief has:
   - the question;
   - the context, with file:line;
   - the rules and decisions that bear on it, verbatim with their ids;
   - this ask: "Answer only; change no file. Give your recommendation, its reasons, its risks, and your confidence (low, medium or high)."
4. Run `brigadier wait` on them in the background. Read each answer when its turn ends.
5. One cross round. With only one CLI ready, run one consult and no cross round; say so in the answer. Otherwise, send each consult the other consults' answers: `brigadier send <id> "Another model answered: <its answer>. Find what is wrong with it and with yours. What would you change in your recommendation? Answer only."` Wait for every consult again. Run no further rounds. In every case, `brigadier close` each consult.

## Answer

Write one answer yourself, as your own reply, in 3 to 5 short lines with no headings: the call and why; where the models agreed and where they differed, and which side you take; what fact or result would flip it; the models consulted, by name.

When the options compare on several points (prices, fees, scores, pros and cons, steps), add a visual below those lines: a table, a chart or a small tool, as `brigadier app visuals` says. Fusion runs only in top-level Chats, where visuals draw inline.

You judge; never count votes. If a consult failed or ran out of quota, say so, and answer from the remaining consults plus your own reading.
