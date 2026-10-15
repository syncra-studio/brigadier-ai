# s2: are the tests green on main
id: s2
kind: status
case: v3
timeout: 900

## Request

```text
Do all the tests pass on `main` right now? How many tests are there?
```

## Scoring

```json
{"rule": "answer",
 "facts": [{"name": "81 tests", "re": "\\b81\\b"},
           {"name": "all pass", "re": "all\\b[^.\\n]{0,40}\\bpass|\\bpass(es|ed|ing)?\\b[^.\\n]{0,20}\\ball\\b|\\b81\\s*/\\s*81\\b|\\b0 fail|\\bno fail|\\bgreen\\b|\\byes\\b"}]}
```

## Known answer

`npm test` on main: 81 tests, 81 pass, 0 fail (and `tsc --noEmit` is clean).
