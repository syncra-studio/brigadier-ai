# V2 lookup battery: the thread battery, its probe allowing the session's own search
id: lookup
timeout: 5400

Several requests in one session (PLAN §0 V2, BUILD-PLAN D1.0). Steps `r1`–`r6` plant every item. The fillers then
push them far out of the thread's verbatim tail (a rebirth briefing keeps 6 exchanges whole, `M/rebirth.rs`
`MIN_EXCHANGES`). `e1` and `e2` act on the constraint, its exception and the ruled-out approach. `p1` asks for
every item. The seams under test happen between the plants and the probe, at least twice each (`run.py --seam`).

The same steps and items as `thread.md`; only the probe differs. There the thread answers from memory alone, which
measures what the briefings keep. Here it may look things up in the session's own transcript and Brain, as it would
in use (BUILD-PLAN D2.4: the thread-only item is found after two rebirths), but still runs no command and reads no
file, so nothing on disk can answer for it.

Each request is sent verbatim, followed by the harness's safety sentence (`tools/ab/task.py`).

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

### e1 edit
```text
Add formatCurrency(amount, code) to src/money.ts: formatCurrency(12.3, 'CAD') returns 'CAD 12.30'. Add a csv exporter in src/exporters/csv.ts, registered as 'csv', that writes the header line number,issueDate,total and then one line per invoice. Add a test for each, and run the checks.
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

### e2 edit
```text
Big imports call catalogFromStore once per invoice line. Make repeated calls for the same store cheaper, and run the checks.
```

### f9 filler
```text
How are overdue invoices found? One sentence with file:line.
```

### f10 filler
```text
What does the report module summarize? Two sentences with file:line.
```

### p1 probe
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
  "seam_points": ["f3", "f8"],
  "uses": {"e1": ["constraint", "exception"], "e2": ["ruled_out"], "p1": ["constraint", "exception", "decision", "hash", "path_line", "units", "failed_check", "ruled_out", "thread_only", "false_learned"]},
  "checks": {
    "e1": "node \"$CHECKS/e1-jsdoc-rule.mjs\" \"$PWD\" && npm run check",
    "e2": "node \"$CHECKS/e2-no-module-cache.mjs\" \"$PWD\" && npm run check"
  }
}
```

## Items

Each item passes or fails on its own (`score.py`). `source` is the step that planted it; its words must have left the
verbatim tail of every briefing before the step that uses it, or the item is scored "not tested" for that use.

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
