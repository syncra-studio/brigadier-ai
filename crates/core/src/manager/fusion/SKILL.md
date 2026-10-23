---
name: fusion
description: When the user is unsure which way to take, or types /fusion <question>, consult one model per installed CLI as lean workers, run one cross round, then answer with one merged recommendation.
---

# Fusion

Use it when the user types `/fusion <question>`, or when they are uncertain: they say they are unsure, or ask which of several ways to take. Not for every reply. A question that the plan, the code or the user's own words already settle needs none.

## Consult

1. Gather the context yourself first: the files that bear on the question (file:line), and the ledger's rules and decisions that touch it (`brigadier ledger show --rules`, `brigadier ledger show --open`).
2. Run `brigadier models`. Start one consult per installed CLI with quota left, each through its own row: `fusion-claude`, `fusion-codex` (another CLI: `fusion-<agent>`). The row picks the strongest model and effort the user's rules allow. If that model is out of quota or refused, pass the strongest one left with `--model`.
3. Start them together, lean, with the same brief:
   `brigadier start "<brief>" --lean --work fusion-claude --title "Fusion · <model>"`
   The brief has:
   - the question;
   - the context, with file:line;
   - the rules and decisions that bear on it, verbatim with their ids;
   - this ask: "Answer only; change no file. Give your recommendation, its reasons, its risks, and your confidence (low, medium or high)."
4. Run `brigadier wait` on them in the background. Read each answer when its turn ends.
5. One cross round. Send each consult the other's answer: `brigadier send <id> "Another model answered: <its answer>. Find what is wrong with it and with yours. What would you change in your recommendation? Answer only."` Wait for both again. Run no further rounds, then `brigadier close` each.

## Answer

Write one answer yourself, as your own reply:

- **Recommendation:** the call.
- **Agreed:** where the models agreed.
- **Differed:** where they differed, which side you take and why.
- **Would change the call:** what fact or result would flip it.
- **Consulted:** the models, by name.

You judge; never count votes. If a consult failed or ran out of quota, say so, and answer from the other plus your own reading.
