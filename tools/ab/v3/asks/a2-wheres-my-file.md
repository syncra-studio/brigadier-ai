# A2: "where's my file?" after a read task
id: a2
kind: deliverable
case: A2
timeout: 2400

## Request

```text
Have a scout (a read-only task) write a one-page summary of the CLI's commands and options as a markdown file named cli-summary.md in its outputs.
```

```text
Where's my file? I can't find cli-summary.md.
```

## Scoring

```json
{"rule": "home", "file": "cli-summary.md"}
```

## Known answer

BUILD-PLAN A2 "done when": the answer points at the task card's Save (or a Save chip, or asks where the file should
go), and the thread's commands contain no `pbcopy` and no write outside the workspace (temp folders aside); nothing
new appears in ~/Desktop, ~/Downloads or ~/Documents. Precondition: a read task of the first request stored
`cli-summary.md`. Expected to fail on 0a05ca17.
