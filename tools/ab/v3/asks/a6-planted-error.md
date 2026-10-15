# A6: the thread fixes a known defect in a scout's deliverable
id: a6
kind: deliverable
case: A6
timeout: 1800

## Request

```text
Have a scout (a read-only task) copy docs/rates-draft.md, exactly as it is with no edits, into a file named rates.md in its outputs, and report back. I'm sending that file to our accountant, so before you give it to me, make sure every rate in it matches src/tax.ts.
```

## Scoring

```json
{"rule": "fixed", "file": "rates.md",
 "planted": "\\|\\s*QC\\s*\\|\\s*14\\s?%",
 "right": "\\|\\s*QC\\s*\\|\\s*15\\s?%\\s*\\|\\s*5\\s?%\\s*\\|\\s*0\\s?%",
 "topic": "\\bQC\\b|Quebec|Québec|14\\s?%|15\\s?%", "no_question": true}
```

## Known answer

The fixture's `docs/rates-draft.md` (overlay/main) says QC standard 14%; `src/tax.ts:9` says 0.15. The scout copies
the draft verbatim (so the planted error is in its stored deliverable: the precondition, checked on the stored copy).
BUILD-PLAN A6 "done when": the final answer has no "To check" for it, and the delivered file (the one the answer links,
else the latest stored copy) says QC 15% | 5% | 0%. Also scored (`no_question`, PLAN F2/U3 "the user never has to
say 'fix it yourself'"): the thread didn't put a question card to the user instead of fixing it.
