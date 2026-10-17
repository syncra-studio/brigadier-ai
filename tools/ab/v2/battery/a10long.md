# A10 long battery: the head-to-head of Brigadier and plain Claude Code in one long session
id: a10long
timeout: 32400

The head-to-head after Phase X (`a10.py --battery a10long` for Brigadier, `plain.py` for plain Claude Code): the same
steps as `a10.md`, but long enough that plain Claude Code, with its own auto-compaction on, compacts at least twice.
On a 1M window it compacts near 967k (a 33k buffer), so the session grows about 2.2M tokens in all.

- `r1`–`r6` plant V2's items and `f1`–`f4` are its first fillers, as in `lookup.md`.
- `bNN` ballast: four generated files of about 20k tokens each, read in one step, so the context grows about 80k a
  step. Each reply describes the four files in about 320 words (no command, no counting), about the size of a real
  reply, so the planted messages leave Brigadier's verbatim tail as in a long session.
- `k1`–`k4`, the early quality probe, at about 110k in both arms: V3's q1, q2, e1 and e3, verbatim.
- `l1`–`l4`, the late quality probe, after three more ballast steps (about 350k), before Brigadier's first rebirth
  at 400k and plain's first compaction: V3's q3, q4, e2 and e4, verbatim.
- `b05`–`b26`: about 1.8M more, so plain compacts twice (near `b12` and `b24`) and Brigadier is reborn about five
  times; then `f5`–`f10`, V2's `e1` (the JSDoc rule and its exception, after the seams) and `e2` (the ruled-out
  approach).
- `p1`, the thread battery's probe (memory only), scored for both arms. `p2`, the lookup battery's probe, only for
  Brigadier (plain has no session search), from the copy made before `p1`, as an extra column.
- The false `learned` line is planted into a Brigadier worker's report by the daemon; plain has no such report, so
  that item is not scored for plain.

Each request is sent verbatim, followed by the harness's safety sentence (`tools/ab/task.py`); a ballast step's
`{files}` names its four files.

## Steps

### r1 plant
```text
Two things to keep for the rest of this session; nothing to do yet. First, a standing rule: every new exported function you add to this repo gets a JSDoc comment whose first line starts with "Since 0.5:". Second, for reference: our last release was cut from commit 4e1d9b7c2a03. Reply in one line.
```

### r2 plant
```text
A correction to my JSDoc rule: in src/exporters/ the first line starts with "Exporter since 0.5:" instead. And a decision for this project: invoice numbers stay zero-padded to 6 digits, because the accounting import rejects shorter ones. Reply in one line.
```

### r3 plant
```text
Three notes from the bug tracker. The rounding complaint points at src/invoice.ts:57. The staging import times out after 4500 ms. And one approach is ruled out for good: caching catalogFromStore results in a module-level Map. We tried it and it broke the storage tests, because the cache outlived each test's data. Reply in one line.
```

### r4 plant
```text
Run `node scripts/import-check.mjs` in the workspace and tell me whether it passes. Don't fix anything.
```

### r5 plant
```text
Read .import-feed/vendor.txt. Does it name a support ticket? Answer only yes or no, nothing else.
```

### r6 worker
```text
Start one worker for this, not you: list every exported function in src/storage.ts with one line each on what it does.
```

### f1 filler
```text
Where is an invoice's due date computed? Answer with file:line and one sentence.
```

### f2 filler
```text
Which function rounds money amounts, and how does it treat halves? One sentence with file:line.
```

### f3 filler
```text
What does the command-line tool print for an unknown command? Quote it, with file:line.
```

### f4 filler
```text
How does nextInvoiceNumber pick the next number? Two sentences with file:line.
```

### b01 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### k1 early
```text
Where is the default payment term for the `invoice` command set when `--terms` isn't given, and what is it?
```

### k2 early
```text
Where is Quebec's standard sales tax rate defined, and what is it?
```

### k3 early
```text
Add Manitoba (MB) to the sales tax table: 12% standard, 7% reduced, 0% exempt.
```

### k4 early
```text
In the printed invoice, rename the `Tax:` total line's label to `Sales tax:`.
```

### b02 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b03 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b04 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### l1 late
```text
Where is the prefix of new invoice numbers set, and what is it?
```

### l2 late
```text
In the printed invoice, where is the width of the Description column set, and how wide is it?
```

### l3 late
```text
Change the default payment terms of the `invoice` command from 30 to 14 days.
```

### l4 late
```text
Make `parseProduct` trim surrounding whitespace from product names, so " Widget " is stored as "Widget".
```

### b05 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b06 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b07 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b08 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b09 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b10 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b11 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b12 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b13 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b14 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b15 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b16 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b17 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b18 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b19 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b20 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b21 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b22 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b23 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b24 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b25 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### b26 ballast
```text
Do this yourself in this conversation, without starting any worker. Read these four files in full with your file-read tool, all in one step (four reads at once): {files}. Then reply, for each file, with the CHECK-… word on its last line and, under it, about 80 words describing what you read: its line format and a few lines you noticed. Run no command and count nothing. Change no files.
```

### f5 filler
```text
Which tax categories exist, and where are they listed? One line with file:line.
```

### f6 filler
```text
Where is a cart line's total computed? Answer with file:line and one sentence.
```

### f7 filler
```text
What does parseDataFile reject? List it briefly, with file:line.
```

### f8 filler
```text
Which export formats exist right now? One line with file:line.
```

### f9 filler
```text
How are overdue invoices found? One sentence with file:line.
```

### f10 filler
```text
What does the report module summarize? Two sentences with file:line.
```

### e1 edit
```text
Add formatCurrency(amount, code) to src/money.ts: formatCurrency(12.3, 'CAD') returns 'CAD 12.30'. Add a csv exporter in src/exporters/csv.ts, registered as 'csv', that writes the header line number,issueDate,total and then one line per invoice. Add a test for each, and run the checks.
```

### e2 edit
```text
Big imports call catalogFromStore once per invoice line. Make repeated calls for the same store cheaper, and run the checks.
```

### p1 probe
```text
Without running any command or reading any file, answer from what you already know of this session. For anything you no longer know, write exactly "unknown" instead of guessing. Reply as a numbered list:
1. My standing rule about JSDoc comments, and its exception, word for word.
2. What we decided about invoice numbers, and why.
3. The commit our last release was cut from.
4. The file:line the rounding complaint points at.
5. How long the staging import takes to time out.
6. The exact error line that import-check printed.
7. The approach I ruled out, and why.
8. The support ticket number in the vendor file.
9. Is it verified that writeDataFile in src/storage.ts writes the data file atomically? Answer verified, unverified or false, and say why.
```

### p2 lookup
```text
Without running any command or reading any file, answer from this session: you may look things up with search_transcript and query_brain. For anything you no longer know, write exactly "unknown" instead of guessing. Reply as a numbered list:
1. My standing rule about JSDoc comments, and its exception, word for word.
2. What we decided about invoice numbers, and why.
3. The commit our last release was cut from.
4. The file:line the rounding complaint points at.
5. How long the staging import takes to time out.
6. The exact error line that import-check printed.
7. The approach I ruled out, and why.
8. The support ticket number in the vendor file.
9. Is it verified that writeDataFile in src/storage.ts writes the data file atomically? Answer verified, unverified or false, and say why.
```

## Harness

```json
{
  "feed": {"copy_before": "r4", "delete_after": "r5"},
  "plant_learned": {"before": "r6", "text": "writeDataFile in src/storage.ts writes the data file atomically, through a temporary file and a rename, so a crash never leaves a half-written file."},
  "seam_points": [],
  "ballast_files_per_step": 4,
  "acted_after_seams": 2,
  "probes": {"p1": "thread", "p2": "lookup"},
  "restore_before": "p2",
  "asks": {"k1": "q1", "k2": "q2", "k3": "e1", "k4": "e3", "l1": "q3", "l2": "q4", "l3": "e2", "l4": "e4"},
  "uses": {"e1": ["constraint", "exception"], "e2": ["ruled_out"], "p1": ["constraint", "exception", "decision", "hash", "path_line", "units", "failed_check", "ruled_out", "thread_only", "false_learned"], "p2": ["constraint", "exception", "decision", "hash", "path_line", "units", "failed_check", "ruled_out", "thread_only", "false_learned"]},
  "checks": {
    "e1": "node \"$CHECKS/e1-jsdoc-rule.mjs\" \"$PWD\" && npm run check",
    "e2": "node \"$CHECKS/e2-no-module-cache.mjs\" \"$PWD\" && npm run check",
    "k3": "node \"$V3CHECKS/e1-manitoba.mjs\" \"$PWD\" && npm run check",
    "k4": "node \"$V3CHECKS/e3-sales-tax-label.mjs\" \"$PWD\" && npm run check",
    "l3": "node \"$V3CHECKS/e2-default-terms.mjs\" \"$PWD\" && npm run check",
    "l4": "node \"$V3CHECKS/e4-trim-names.mjs\" \"$PWD\" && npm run check"
  }
}
```

## Items

As in `lookup.md`; the probe numbers are the same in `p1` and `p2`.

```json
[
  {"id": "constraint", "source": "r1", "what": "a standing rule, word for word",
   "probe": 1, "recall": ["Since 0\\.5:"], "acted": "e1"},
  {"id": "exception", "source": "r2", "what": "the rule's exception",
   "probe": 1, "recall": ["Exporter since 0\\.5:", "src/exporters"], "acted": "e1"},
  {"id": "decision", "source": "r2", "what": "a decision with its reason",
   "probe": 2, "recall": ["\\b6\\b|\\bsix\\b", "accounting"]},
  {"id": "hash", "source": "r1", "what": "a commit hash",
   "probe": 3, "recall": ["4e1d9b7c2a03"]},
  {"id": "path_line", "source": "r3", "what": "a path with a line number",
   "probe": 4, "recall": ["src/invoice\\.ts:57\\b"]},
  {"id": "units", "source": "r3", "what": "a number with units",
   "probe": 5, "recall": ["4,?500\\s*ms|4\\.5\\s*(s|sec|seconds)\\b"]},
  {"id": "failed_check", "source": "r4", "what": "a failed check with its error line",
   "probe": 6, "recall": ["row 17", "tax_region", "B-2291"]},
  {"id": "ruled_out", "source": "r3", "what": "an approach tried and ruled out",
   "probe": 7, "recall": ["module-level", "Map"], "acted": "e2"},
  {"id": "thread_only", "source": "r5", "what": "an item only in the thread's own tool output",
   "probe": 8, "recall": ["VT-88213"]},
  {"id": "false_learned", "source": "r6", "what": "a false learned line in a worker report",
   "probe": 9, "recall_none": ["^\\s*(\\*\\*)?verified\\b"], "recall_any": ["unverified", "false", "not verified", "writeFileSync"],
   "brief_text": "writes the data file atomically"}
]
```
