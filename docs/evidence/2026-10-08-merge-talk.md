# Conversational merge (2026-10-08)

THREAD-PLAN.md §5, user decision 6. Branch `merge-talk` from `thread-build` `fef7b14a`. Claude
Code 2.1.293. The live checks ran on a dev `brigadierd` built from the branch, with a scratch
`BRIGADIER_DATA_DIR` (`/tmp/w26/data`) and scratch repo (`/tmp/w26/repo`, Full access), started by
`tools/ab/startd.sh` and driven over IPC (`tools/ab/bipc.py`). The thread was Claude Sonnet, low
effort. Computer Use was not used: its `health_report` failed ("computer-use daemon failed to
start … not listening"), so every check was scripted. `tools/ab/recorder.py` recorded every event.

## Done-when

| Done when | Result |
|---|---|
| A live session lands a change and the thread proposes the merge in words, with no card | **Pass** (session 1) |
| "no" or silence leaves it unmerged | **Pass** (session 1) |
| "yes, merge it" merges | **Pass** (session 1) |
| A direct "merge it" with no proposal merges | **Pass** on the second build (sessions 3 and 4); **missed** on the first (session 2), fixed in `a8191b3d` |
| The tool refuses without the user's words | **Pass**, in tests (below) |

## Live

**Session 1** (`01a11ab9-b46a-70a5-85da-6a7e3fed2201`):

| The user | The thread | `main` |
|---|---|---|
| "Create a file NOTES.md … and commit it. It's tiny: do it yourself, no workers." | Committed `c83dc4c` on `brigadier/3fed2201/session`, then: "Merge `brigadier/3fed2201/session` into `main`?" | `06a2717 Start` |
| "no" | "Understood, I won't merge. The commit stays on `brigadier/3fed2201/session`, and `main` is unchanged." | `06a2717 Start` |
| *(silence while the review ended clean, then 60 s more)* | — | `06a2717 Start` |
| "yes, merge it" | `finish_session` → "I merged `brigadier/3fed2201/session` into `main`. … The background review of the commit found nothing. The session branch and its worktree are removed." | `c83dc4c Add NOTES.md` |

**Session 2**, first build: "Create OTHER.md … commit it yourself, then merge it into main." The
thread committed `6b47859` but asked "Merge `brigadier/77d37b24/session` into `main`?" instead of
merging. It never called `finish_session`: the prompt read as "propose first". `a8191b3d` says a
merge asked for together with the work is consent. The session was left unmerged.

**Session 3**, rebuilt daemon: "Create THIRD.md … commit it yourself, then merge it into main." →
committed and merged in the same turn: "… merged `brigadier/9ef5bb82/session` into `main`. … The
background review of the merge is still running, and I'll tell you if it finds anything." `main`
= `6d9841c Add THIRD.md`.

**Session 4**: "Create FOURTH.md … Reply with just the commit hash" → `dc972a7…` (no proposal).
Then "merge it" → "Merged `brigadier/f75ce02f/session` into `main` at dc972a7bad. The other-vendor
review of that commit is still running." `main` = `dc972a7 Add FOURTH.md`.

Across all four sessions the recorder saw **0** `approvalUpdated` events: no card of any kind. Each
merge recorded one `Merged` orchestrator step with the consenting message's id (`askedIn`).

## Tests

- `merge_consent::tests` (6): direct asks; a plain yes only after a proposal that names the
  branch or base; refusals:
  - a no, a hold or a negation ("don't", "wouldn't");
  - a condition ("if", "once", "after", "as soon as");
  - a question;
  - words not in the message;
  - a yes inside a longer message.
- `thread_tests::the_merge_is_asked_in_words_and_happens_only_on_the_users_yes`: the thread's
  calls before the user answered, on "No, not yet." and on "Hmm, don't merge it" are refused, and
  nothing merges. "OK, yes, merge it" merges. A second call with the same words is refused
  ("already asked for a merge"). There is no approval card and exactly one `Merged` step.
- `thread_tests::a_wait_sent_while_the_merge_is_prepared_stops_it`: the merge pauses once
  prepared (a test hook), the user sends "wait!", and the last look before landing refuses
  ("wrote again"); nothing merges.
- `thread_tests::a_merge_counts_the_review_of_the_threads_own_commit` and
  `tests::a_review_still_running_at_the_merge_reports_its_findings_after_it`: the review started
  before the merge counts with the merged work, the answer says it still runs, and its findings
  reach the thread after the merge.
- `reviewStatus.test.ts`: the context card's lines. The merged work keeps "Review running…" and
  then its outcome until the next merge; work after the merge gets its own line.
