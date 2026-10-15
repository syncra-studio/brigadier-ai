# A1: a read task's deliverable stays reachable after the task is disposed
id: a1
kind: deliverable
case: A1
timeout: 1800

## Request

```text
Have a scout (a read-only task) list every function in src/ that rounds money, one line each with its file:line, in a markdown file named rounding.md in its outputs. Link the file in your answer.
```

## Scoring

```json
{"rule": "links", "file": "rounding.md"}
```

## Known answer

BUILD-PLAN A1 "done when": a read task writes `outputs/rounding.md`, links it, reports and is disposed; every link to
it in the worker's text, the addendum the thread got and the thread's final answer resolves after disposal (a file
that exists, or Brigadier's stored copy). Precondition: a scout or research task stored `rounding.md` (the request
asks for a scout, so this is seeded by the request). Expected to fail on 0a05ca17 (links point at the disposed scratch).
