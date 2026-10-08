# A/B response policy (both arms, THREAD-PLAN phase 6)

Written before the first phase-6 arm. It applies identically to the Brigadier arm (`recorder.py`) and
to the /delegator arm (the watcher, through `dlg_say.sh`). Nothing else is said to an arm.

| The arm asks | The answer |
|---|---|
| Any question (a question card; the coordinator asking the user something and waiting) | "Go with your recommendation." |
| A plan or outline approval (plan card, outline card; "Go ahead?" in the tab) | "Go ahead." (a card: approve) |
| Landing a task, or an action approval that doesn't push | approve |
| A merge into the base asked of the user (in words, in either arm: Brigadier has no merge card) | never approved: left unanswered (Brigadier) or "Leave it on its branch; don't merge." (tab) |
| Anything that pushes or opens a PR | denied: "No push: leave the work on its branch." |

- The Delegator's own run approvals inside /delegator (its workers' questions) are the coordinator's
  business, not the user's; the watcher only answers what the coordinator asks the user.
- Every answer is logged with its time: Brigadier in `<arm>/rec/cards.jsonl`, /delegator in
  `<arm>/interventions.jsonl`. The report counts them per arm.
- A tab prompt the CLI itself shows (folder trust, bypass-permissions warning) is answered by
  `dlg_start.sh` before t0 and isn't an intervention.
